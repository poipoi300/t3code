use super::super::outbox::uploaded_reference;
use serde_json::{Value, json};
fn strings(value: &Value, fields: &[&str]) -> bool {
    value.is_object() && fields.iter().all(|field| value[*field].is_string())
}
pub(super) fn image(value: &Value) -> bool {
    strings(value, &["id", "name", "mimeType"])
        && value["id"] != ""
        && (uploaded_reference(value).is_some()
            || value["dataUrl"]
                .as_str()
                .is_some_and(|data| !data.is_empty()))
        && value["sizeBytes"].is_number()
}
pub(super) fn file(value: &Value) -> bool {
    strings(value, &["id", "name", "mimeType"])
        && value["sizeBytes"].is_number()
        && ["attachmentId", "environmentId"]
            .iter()
            .all(|field| value.get(*field).is_none_or(Value::is_string))
        && value
            .get("environmentId")
            .is_none_or(valid::<t3_contracts::base_schemas::EnvironmentId>)
        && value
            .get("source")
            .is_none_or(valid::<t3_contracts::chat_attachment::PastedTextAttachmentSource>)
}
pub(super) fn terminal(value: &Value) -> Option<Value> {
    if !strings(
        value,
        &["id", "threadId", "createdAt", "terminalId", "terminalLabel"],
    ) || ["id", "threadId", "createdAt"]
        .iter()
        .any(|field| value[*field].as_str().is_none_or(str::is_empty))
        || ["terminalId", "terminalLabel"].iter().any(|field| {
            value[*field]
                .as_str()
                .is_none_or(|text| text.trim().is_empty())
        })
    {
        return None;
    }
    let start = value["lineStart"].as_f64()?.floor().max(1.0);
    let end = value["lineEnd"].as_f64()?.floor().max(start);
    let mut next = value.clone();
    next["id"] = json!(super::web_drafts::context_id(value["id"].as_str()?));
    for field in ["terminalId", "terminalLabel"] {
        next[field] = json!(value[field].as_str()?.trim());
    }
    next["lineStart"] = serde_json::from_str(&t3_contracts::wire::format_js_number(start)).ok()?;
    next["lineEnd"] = serde_json::from_str(&t3_contracts::wire::format_js_number(end)).ok()?;
    if next.get("text").is_some_and(|value| !value.is_string())
        && let Some(fields) = next.as_object_mut()
    {
        fields.remove("text");
    }
    Some(next)
}
pub(super) fn review(value: &Value) -> bool {
    strings(
        value,
        &[
            "id",
            "sectionId",
            "sectionTitle",
            "filePath",
            "rangeLabel",
            "text",
            "diff",
        ],
    ) && ["startIndex", "endIndex"]
        .iter()
        .all(|field| value[*field].is_number())
        && value.get("fenceLanguage").is_none_or(Value::is_string)
        && value.get("selection").is_none_or(|selection| {
            ["start", "end"]
                .iter()
                .all(|field| selection[*field].is_number())
                && ["side", "endSide"].iter().all(|field| {
                    selection[*field]
                        .as_str()
                        .is_some_and(|side| matches!(side, "additions" | "deletions"))
                })
        })
        && value
            .get("pullRequest")
            .is_none_or(valid::<t3_contracts::composer_context::PullRequestContextMetadata>)
}
fn valid<T: serde::de::DeserializeOwned>(value: &Value) -> bool {
    serde_json::from_value::<T>(value.clone()).is_ok()
}
pub(super) fn fields(value: &Value) -> Value {
    let mut next = value.clone();
    for (key, predicate) in [
        ("attachments", image as fn(&Value) -> bool),
        ("files", file),
        ("reviewComments", review),
        (
            "threadContexts",
            valid::<t3_contracts::composer_context::ThreadContextRecord>,
        ),
        (
            "environmentContexts",
            valid::<t3_contracts::composer_context::EnvironmentContextRecord>,
        ),
        (
            "previewAnnotations",
            valid::<t3_contracts::ipc::PreviewAnnotationPayloadSchema>,
        ),
    ] {
        let values = value[key]
            .as_array()
            .map(|values| {
                values
                    .iter()
                    .filter(|entry| predicate(entry))
                    .cloned()
                    .map(|mut entry| {
                        if key == "attachments"
                            && entry.get("source").is_some_and(|source| {
                                !valid::<t3_contracts::chat_attachment::SnapShotSource>(source)
                            })
                            && let Some(fields) = entry.as_object_mut()
                        {
                            fields.remove("source");
                        }
                        entry
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if value.get(key).is_some() || key == "attachments" {
            next[key] = values.into();
        }
    }
    let terminals = value["terminalContexts"]
        .as_array()
        .map(|values| values.iter().filter_map(terminal).collect::<Vec<_>>())
        .unwrap_or_default();
    if value.get("terminalContexts").is_some() {
        next["terminalContexts"] = terminals.into();
    }
    let mut annotations = next["previewAnnotations"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for element in value["elementContexts"].as_array().into_iter().flatten() {
        let (Some(id), Some(picked_at)) = (element["id"].as_str(), element["pickedAt"].as_str())
        else {
            continue;
        };
        if !valid::<t3_contracts::composer_context::ElementContextDetails>(element)
            || annotations.iter().any(|annotation| annotation["id"] == id)
        {
            continue;
        }
        let mut target = element.clone();
        target["stack"] = json!([]);
        annotations.push(json!({"id":id,"pageUrl":element["pageUrl"],"pageTitle":element["pageTitle"],"comment":"","elements":[{"id":id,"element":target,"rect":{"x":0,"y":0,"width":0,"height":0}}],"regions":[],"strokes":[],"styleChanges":[],"screenshot":null,"createdAt":picked_at}));
    }
    if !annotations.is_empty() {
        next["previewAnnotations"] = annotations.into();
    }
    if next
        .get("replyTo")
        .is_some_and(|value| !valid::<t3_contracts::composer_context::ResponseContextRecord>(value))
        && let Some(fields) = next.as_object_mut()
    {
        fields.remove("replyTo");
    }
    if next
        .get("runtimeMode")
        .is_some_and(|value| !valid::<t3_contracts::provider_policy::RuntimeMode>(value))
        && let Some(fields) = next.as_object_mut()
    {
        fields.remove("runtimeMode");
    }
    if next.get("interactionMode").is_some_and(|value| {
        !valid::<t3_contracts::provider_policy::ProviderInteractionMode>(value)
    }) && let Some(fields) = next.as_object_mut()
    {
        fields.remove("interactionMode");
    }
    next
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_persisted_fields_are_filtered_without_panics_and_valid_payload_bytes_survive() {
        let image = json!({"id":"image","name":"image.png","mimeType":"image/png","sizeBytes":1,"dataUrl":"not-decodable-yet","future":true});
        let file = json!({"id":"file","name":"file.txt","mimeType":"text/plain","sizeBytes":-2,"attachmentId":"server-file","environmentId":"e"});
        let normalized = super::super::web_drafts::normalize_state(
            &json!({"draftsByThreadId":{"empty":{"prompt":"","attachments":"bad","terminalContexts":"bad","provider":"bogus-provider","modelOptions":"bad"},"valid":{"prompt":"keep","attachments":[null,1,image],"files":[false,file],"terminalContexts":[null,"bad",{"id":"terminal","threadId":"thread","createdAt":"then","terminalId":" terminal ","terminalLabel":" Shell ","lineStart":0.5,"lineEnd":0}],"runtimeMode":"invalid","interactionMode":"invalid","replyTo":{"bad":true},"futureDraft":true},"spaces":{"prompt":"  ","attachments":[]}},"draftThreadsByThreadId":"bad","projectDraftThreadIdByProjectKey":"bad"}),
            "now",
            true,
        );
        assert!(normalized["draftsByThreadKey"].get("empty").is_none());
        assert_eq!(normalized["draftThreadsByThreadKey"], json!({}));
        let draft = &normalized["draftsByThreadKey"]["valid"];
        assert_eq!(draft["attachments"], json!([image]));
        assert_eq!(draft["files"], json!([file]));
        assert_eq!(draft["terminalContexts"][0]["terminalId"], "terminal");
        assert_eq!(draft["terminalContexts"][0]["lineStart"], 1);
        assert_eq!(draft["futureDraft"], true);
        assert!(draft.get("runtimeMode").is_none());
        assert!(draft.get("replyTo").is_none());
        assert_eq!(normalized["draftsByThreadKey"]["spaces"]["prompt"], "  ");
    }
    #[test]
    fn legacy_elements_become_annotations_once_and_their_old_references_are_remapped() {
        let element = json!({"id":"picked","pickedAt":"then","pageUrl":"https://example.com","pageTitle":null,"tagName":"button","selector":null,"htmlPreview":"<button/>","componentName":null,"source":null,"styles":""});
        let input = json!({"draftsByThreadKey":{"e:t":{"prompt":"[button](t3-context://v1/preview-annotation/annotation-picked)","attachments":[],"elementContexts":[element,element]}}});
        let normalized = super::super::web_drafts::normalize_state(&input, "now", true);
        let draft = &normalized["draftsByThreadKey"]["e:t"];
        assert_eq!(
            draft["previewAnnotations"]
                .as_array()
                .expect("annotations")
                .len(),
            1
        );
        assert_eq!(
            draft["previewAnnotations"][0]["elements"][0]["element"]["stack"],
            json!([])
        );
        assert_eq!(
            draft["prompt"],
            "[button](t3-context://v1/preview-annotation/preview-annotation_picked)"
        );
        assert_eq!(
            super::super::web_drafts::normalize_state(&normalized, "later", true),
            normalized
        );
    }
}
