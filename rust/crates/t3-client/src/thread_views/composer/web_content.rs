//! Attachment mutations return preview releases for the host resource owner to apply.
use super::{
    web_drafts::{context_link, ensure_reference, kind_context_id},
    web_models::{draft, save},
};
use crate::view_logic::context_references::{collect_references, remove_reference};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use t3_contracts::chat_attachment::PROVIDER_SEND_TURN_MAX_ATTACHMENTS;

fn array(value: &Value, key: &str) -> Vec<Value> {
    value[key].as_array().cloned().unwrap_or_default()
}
fn string<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or_default()
}
pub fn dedup_key(file: &Value) -> String {
    let size = file["sizeBytes"]
        .as_f64()
        .map(t3_contracts::wire::format_js_number)
        .unwrap_or_default();
    format!(
        "{}\0{size}\0{}",
        string(file, "mimeType"),
        string(file, "name")
    )
}
pub fn needs_reattach(file: &Value) -> bool {
    file.get("file") == Some(&Value::Null) && file.get("uploadedAttachmentId").is_none()
}
fn video(file: &Value) -> bool {
    let mime = string(file, "mimeType")
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if mime.starts_with("video/")
        && !matches!(mime.as_str(), "video/mp2t" | "video/vnd.dlna.mpeg-tts")
    {
        return true;
    }
    if !matches!(
        mime.as_str(),
        "" | "application/octet-stream"
            | "binary/octet-stream"
            | "application/unknown"
            | "video/mp2t"
            | "video/vnd.dlna.mpeg-tts"
    ) {
        return false;
    }
    string(file, "name")
        .rsplit_once('.')
        .is_some_and(|(_, extension)| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "avi" | "m4v" | "mp4" | "mkv" | "mov" | "ogv" | "webm"
            )
        })
}
fn dng(file: &Value) -> bool {
    string(file, "name").to_ascii_lowercase().ends_with(".dng")
        || matches!(
            string(file, "mimeType")
                .split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase()
                .as_str(),
            "image/dng" | "image/x-dng" | "image/x-adobe-dng"
        )
}
pub fn reattach_marker_matches(marker: &Value, file: &Value) -> bool {
    marker["name"] == file["name"]
        && marker["sizeBytes"] == file["sizeBytes"]
        && (marker["mimeType"] == file["mimeType"]
            || (dng(marker) && dng(file))
            || (matches!(
                string(marker, "mimeType").to_ascii_lowercase().as_str(),
                "" | "application/octet-stream"
            ) && video(marker)
                && video(file)))
}
#[derive(Debug, Default, PartialEq, Eq)]
pub struct AttachmentChanges {
    pub accepted_ids: Vec<String>,
    pub release_previews: Vec<String>,
}
pub fn add_images(
    state: &mut Value,
    key: &str,
    incoming: &[Value],
    allow_duplicates: bool,
) -> AttachmentChanges {
    let mut result = AttachmentChanges::default();
    if key.is_empty() || incoming.is_empty() {
        return result;
    }
    let mut next = draft(state, key);
    let mut images = array(&next, "images");
    let mut ids: BTreeSet<_> = images
        .iter()
        .map(|image| string(image, "id").to_owned())
        .collect();
    let mut keys: BTreeSet<_> = images.iter().map(dedup_key).collect();
    let mut previews: BTreeSet<_> = images
        .iter()
        .map(|image| string(image, "previewUrl").to_owned())
        .collect();
    let files = array(&next, "files").len();
    for image in incoming {
        let id = string(image, "id");
        let preview = string(image, "previewUrl");
        let image_key = dedup_key(image);
        if ids.contains(id)
            || (!allow_duplicates && keys.contains(&image_key))
            || images.len() + files >= PROVIDER_SEND_TURN_MAX_ATTACHMENTS
        {
            if preview.starts_with("blob:") && !previews.contains(preview) {
                result.release_previews.push(preview.into());
            }
            continue;
        }
        ids.insert(id.to_owned());
        keys.insert(image_key);
        previews.insert(preview.to_owned());
        result.accepted_ids.push(id.to_owned());
        images.push(image.clone());
    }
    if !result.accepted_ids.is_empty() {
        next["images"] = json!(images);
        save(state, key, next);
    }
    result
}
pub fn add_files(
    state: &mut Value,
    key: &str,
    incoming: &[Value],
    allow_duplicates: bool,
    append_reference: bool,
) -> Vec<String> {
    if key.is_empty() || incoming.is_empty() {
        return Vec::new();
    }
    let mut next = draft(state, key);
    let files = array(&next, "files");
    let mut ids: BTreeSet<_> = files
        .iter()
        .map(|file| string(file, "id").to_owned())
        .collect();
    let mut known: BTreeMap<_, _> = files
        .iter()
        .map(|file| (dedup_key(file), file.clone()))
        .collect();
    let mut accepted = Vec::new();
    let mut replacements = BTreeMap::new();
    let mut replacement_order = Vec::new();
    for file in incoming {
        let id = string(file, "id");
        if ids.contains(id) {
            continue;
        }
        let file_key = dedup_key(file);
        let duplicate = (!allow_duplicates)
            .then(|| {
                known.get(&file_key).or_else(|| {
                    files.iter().find(|candidate| {
                        needs_reattach(candidate)
                            && !replacements.contains_key(string(candidate, "id"))
                            && reattach_marker_matches(candidate, file)
                    })
                })
            })
            .flatten();
        if let Some(duplicate) = duplicate {
            if needs_reattach(duplicate) && !replacements.contains_key(string(duplicate, "id")) {
                replacement_order.push(string(duplicate, "id").to_owned());
                replacements.insert(string(duplicate, "id").to_owned(), file.clone());
                ids.insert(id.to_owned());
                known.insert(file_key, file.clone());
            }
            continue;
        }
        if files.len() + array(&next, "images").len() + accepted.len()
            >= PROVIDER_SEND_TURN_MAX_ATTACHMENTS
        {
            break;
        }
        accepted.push(file.clone());
        ids.insert(id.to_owned());
        known.insert(file_key, file.clone());
    }
    if accepted.is_empty() && replacements.is_empty() {
        return Vec::new();
    }
    let mut prompt = string(&next, "prompt").to_owned();
    for reference in collect_references(&prompt.clone()).into_iter().rev() {
        let Some(original) = files
            .iter()
            .find(|file| kind_context_id("file", string(file, "id")) == reference.context_id)
        else {
            continue;
        };
        if let Some(replacement) = replacements.get(string(original, "id")) {
            prompt.replace_range(
                reference.start..reference.end,
                &context_link(
                    "file",
                    &kind_context_id("file", string(replacement, "id")),
                    string(replacement, "name"),
                ),
            );
        }
    }
    if append_reference {
        for file in accepted.iter().chain(
            replacement_order
                .iter()
                .filter_map(|id| replacements.get(id)),
        ) {
            ensure_reference(
                &mut prompt,
                "file",
                &kind_context_id("file", string(file, "id")),
                string(file, "name"),
            );
        }
    }
    let accepted_ids = accepted
        .iter()
        .map(|file| string(file, "id").to_owned())
        .collect();
    let mut files: Vec<_> = files
        .into_iter()
        .map(|file| {
            replacements
                .get(string(&file, "id"))
                .cloned()
                .unwrap_or(file)
        })
        .collect();
    files.extend(accepted);
    next["files"] = json!(files);
    next["prompt"] = json!(prompt);
    save(state, key, next);
    accepted_ids
}
pub fn remove_file(state: &mut Value, key: &str, id: &str) -> bool {
    let mut next = draft(state, key);
    let mut files = array(&next, "files");
    let len = files.len();
    files.retain(|file| string(file, "id") != id);
    if files.len() == len {
        return false;
    }
    next["files"] = json!(files);
    next["prompt"] = json!(remove_reference(
        string(&next, "prompt"),
        &kind_context_id("file", id)
    ));
    save(state, key, next);
    true
}
pub fn remove_image(state: &mut Value, key: &str, id: &str) -> Vec<String> {
    let Some(existing) = state["draftsByThreadKey"].get(key) else {
        return Vec::new();
    };
    let mut next = existing.clone();
    let previews = array(existing, "images")
        .into_iter()
        .filter(|image| string(image, "id") == id)
        .take(1)
        .map(|image| string(&image, "previewUrl").to_owned())
        .filter(|preview| preview.starts_with("blob:"))
        .collect();
    for field in ["images", "persistedAttachments"] {
        next[field] = json!(
            array(&next, field)
                .into_iter()
                .filter(|item| string(item, "id") != id)
                .collect::<Vec<_>>()
        );
    }
    next["nonPersistedImageIds"] = json!(
        array(&next, "nonPersistedImageIds")
            .into_iter()
            .filter(|value| value != id)
            .collect::<Vec<_>>()
    );
    next["prompt"] = json!(remove_reference(
        string(&next, "prompt"),
        &kind_context_id("image", id)
    ));
    save(state, key, next);
    previews
}
pub fn set_file_upload(
    state: &mut Value,
    key: &str,
    id: &str,
    environment: &str,
    attachment: &str,
) -> bool {
    let Some(files) = state["draftsByThreadKey"][key]["files"].as_array_mut() else {
        return false;
    };
    let Some(file) = files.iter_mut().find(|file| string(file, "id") == id) else {
        return false;
    };
    if file["uploadEnvironmentId"] == environment && file["uploadedAttachmentId"] == attachment {
        return false;
    }
    file["uploadEnvironmentId"] = json!(environment);
    file["uploadedAttachmentId"] = json!(attachment);
    true
}
pub fn mark_file_upload_missing(
    state: &mut Value,
    key: &str,
    id: &str,
    environment: &str,
    attachment: &str,
) -> bool {
    let Some(files) = state["draftsByThreadKey"][key]["files"].as_array_mut() else {
        return false;
    };
    let Some(file) = files.iter_mut().find(|file| string(file, "id") == id) else {
        return false;
    };
    if file.get("file") != Some(&Value::Null)
        || file["uploadEnvironmentId"] != environment
        || file["uploadedAttachmentId"] != attachment
    {
        return false;
    }
    if let Some(fields) = file.as_object_mut() {
        fields.remove("uploadEnvironmentId");
        fields.remove("uploadedAttachmentId");
    }
    true
}
fn terminal_prompt(mut prompt: String, draft: &Value) -> String {
    for context in array(draft, "terminalContexts") {
        let start = &context["lineStart"];
        let end = &context["lineEnd"];
        let range = if start == end {
            format!("line {start}")
        } else {
            format!("lines {start}-{end}")
        };
        ensure_reference(
            &mut prompt,
            "terminal",
            &kind_context_id("terminal", string(&context, "id")),
            &format!("{} {range}", string(&context, "terminalLabel")),
        );
    }
    prompt
}
pub fn move_prompt_and_images(
    state: &mut Value,
    from: &str,
    to: &str,
    destination_environment: Option<&str>,
) -> bool {
    if from.is_empty() || to.is_empty() || from == to {
        return false;
    }
    let Some(source) = state["draftsByThreadKey"].get(from).cloned() else {
        return false;
    };
    let destination = draft(state, to);
    let mut source_next = source.clone();
    let mut destination_next = destination.clone();
    let mut images = array(&destination, "images");
    let mut files = array(&destination, "files");
    let slots = PROVIDER_SEND_TURN_MAX_ATTACHMENTS.saturating_sub(images.len() + files.len());
    let moved_images = array(&source, "images")
        .into_iter()
        .take(slots)
        .collect::<Vec<_>>();
    let image_ids: BTreeSet<_> = moved_images
        .iter()
        .map(|image| string(image, "id").to_owned())
        .collect();
    let file_ids: BTreeSet<_> = files
        .iter()
        .map(|file| string(file, "id").to_owned())
        .collect();
    let file_keys: BTreeSet<_> = files.iter().map(dedup_key).collect();
    let moved_files = array(&source, "files")
        .into_iter()
        .filter(|file| {
            (!file["file"].is_null()
                || destination_environment
                    .is_some_and(|environment| file["uploadEnvironmentId"] == environment))
                && !file_ids.contains(string(file, "id"))
                && !file_keys.contains(&dedup_key(file))
        })
        .take(slots.saturating_sub(moved_images.len()))
        .map(|mut file| {
            if !file["file"].is_null()
                && file.get("uploadEnvironmentId").is_some()
                && file["uploadEnvironmentId"].as_str() != destination_environment
                && let Some(fields) = file.as_object_mut()
            {
                fields.remove("uploadEnvironmentId");
                fields.remove("uploadedAttachmentId");
            }
            file
        })
        .collect::<Vec<_>>();
    let moved_ids: BTreeSet<_> = moved_files
        .iter()
        .map(|file| string(file, "id").to_owned())
        .collect();
    images.extend(moved_images);
    files.extend(moved_files);
    destination_next["images"] = json!(images);
    destination_next["files"] = json!(files);
    destination_next["prompt"] = json!(terminal_prompt(
        crate::view_logic::context_references::strip_references(string(&source, "prompt")),
        &destination
    ));
    source_next["prompt"] = json!(terminal_prompt(String::new(), &source));
    source_next["images"] = json!(
        array(&source, "images")
            .into_iter()
            .filter(|image| !image_ids.contains(string(image, "id")))
            .collect::<Vec<_>>()
    );
    source_next["files"] = json!(
        array(&source, "files")
            .into_iter()
            .filter(|file| !moved_ids.contains(string(file, "id")))
            .collect::<Vec<_>>()
    );
    for field in ["nonPersistedImageIds", "persistedAttachments"] {
        let mut retained = array(&destination, field);
        for value in array(&source, field) {
            let id = if field == "nonPersistedImageIds" {
                value.as_str().unwrap_or_default()
            } else {
                string(&value, "id")
            };
            if image_ids.contains(id) {
                retained.push(value);
            }
        }
        destination_next[field] = json!(retained);
        source_next[field] = json!(
            array(&source, field)
                .into_iter()
                .filter(
                    |value| !image_ids.contains(if field == "nonPersistedImageIds" {
                        value.as_str().unwrap_or_default()
                    } else {
                        string(value, "id")
                    })
                )
                .collect::<Vec<_>>()
        );
    }
    save(state, from, source_next);
    save(state, to, destination_next);
    true
}
pub fn clear_prompt_and_images(state: &mut Value, key: &str) -> Vec<String> {
    use super::web_context::Kind;
    let Some(current) = state["draftsByThreadKey"].get(key).cloned() else {
        return Vec::new();
    };
    let previews = array(&current, "images")
        .iter()
        .filter_map(|image| image["previewUrl"].as_str())
        .filter(|preview| preview.starts_with("blob:"))
        .map(str::to_owned)
        .collect();
    let mut next = current.clone();
    let mut prompt = String::new();
    for kind in [
        Kind::Terminal,
        Kind::Review,
        Kind::Thread,
        Kind::Environment,
        Kind::Preview,
    ] {
        for context in array(&current, kind.field()) {
            ensure_reference(
                &mut prompt,
                kind.wire(),
                &kind.id(&context),
                &kind.label(&context),
            );
        }
    }
    next["prompt"] = json!(prompt);
    for field in [
        "images",
        "files",
        "nonPersistedImageIds",
        "persistedAttachments",
    ] {
        next[field] = json!([]);
    }
    save(state, key, next);
    previews
}
pub fn persisted_attachment_ids(bytes: Option<&[u8]>, key: &str) -> BTreeSet<String> {
    bytes
        .and_then(|bytes| crate::thread_views::drafts::SavedDraftDocument::decode_web(bytes).ok())
        .filter(|document| document.raw()["version"] == crate::thread_views::drafts::WEB_VERSION)
        .and_then(|document| {
            document
                .drafts()
                .and_then(|drafts| drafts.get(key))
                .cloned()
        })
        .map(|draft| {
            array(&draft, "attachments")
                .iter()
                .filter_map(|attachment| attachment["id"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}
pub fn sync_persisted_attachments(state: &mut Value, key: &str, captured: &[Value]) -> bool {
    let Some(current) = state["draftsByThreadKey"].get(key).cloned() else {
        return false;
    };
    let ids: BTreeSet<_> = captured
        .iter()
        .map(|attachment| string(attachment, "id"))
        .collect();
    let mut next = current.clone();
    next["persistedAttachments"] = json!(captured);
    next["nonPersistedImageIds"] = json!(
        array(&current, "nonPersistedImageIds")
            .into_iter()
            .filter(|id| !ids.contains(id.as_str().unwrap_or_default()))
            .collect::<Vec<_>>()
    );
    let changed = next != current;
    save(state, key, next);
    changed
}
pub fn verify_persisted_attachments(
    state: &mut Value,
    key: &str,
    captured: &[Value],
    persisted: &BTreeSet<String>,
) -> bool {
    let Some(current) = state["draftsByThreadKey"].get(key).cloned() else {
        return false;
    };
    let images = array(&current, "images");
    let ids: BTreeSet<_> = images.iter().map(|image| string(image, "id")).collect();
    let mut next = current.clone();
    next["persistedAttachments"] = json!(
        captured
            .iter()
            .filter(|attachment| ids.contains(string(attachment, "id"))
                && persisted.contains(string(attachment, "id")))
            .cloned()
            .collect::<Vec<_>>()
    );
    next["nonPersistedImageIds"] = json!(
        images
            .iter()
            .map(|image| string(image, "id"))
            .filter(|id| !persisted.contains(*id))
            .collect::<Vec<_>>()
    );
    let changed = next != current;
    save(state, key, next);
    changed
}
pub fn clear_persisted_attachments(state: &mut Value, key: &str) -> bool {
    let Some(current) = state["draftsByThreadKey"].get(key).cloned() else {
        return false;
    };
    let mut next = current.clone();
    next["persistedAttachments"] = json!([]);
    next["nonPersistedImageIds"] = json!([]);
    let changed = next != current;
    save(state, key, next);
    changed
}
