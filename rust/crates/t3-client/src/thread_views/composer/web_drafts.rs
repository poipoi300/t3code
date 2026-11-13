//! Web draft migrations keep unrecognized data alongside the normalized editable fields.
use super::super::drafts::web_has_user_content;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use t3_contracts::{
    model::{DEFAULT_MODEL, DEFAULT_MODEL_BY_PROVIDER, MODEL_SLUG_ALIASES_BY_PROVIDER},
    model_selection::ModelSelection,
};

fn record(value: &Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap_or_default()
}
fn string<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key)?.as_str()
}
fn nonempty<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    string(value, key).filter(|s| !s.is_empty())
}
fn array(value: &Value, key: &str) -> Vec<Value> {
    value
        .get(key)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}
fn scoped(key: &str) -> Option<(&str, &str)> {
    key.split_once(':')
        .filter(|(e, id)| !e.is_empty() && !id.is_empty())
}
fn provider(value: &Value) -> Option<&str> {
    value.as_str().filter(|s| {
        s.len() <= 64
            && s.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    })
}
pub fn coerce_options(value: &Value) -> Vec<Value> {
    match value {
        Value::Array(values) => values
            .iter()
            .filter(|v| {
                nonempty(v, "id").is_some() && (v["value"].is_string() || v["value"].is_boolean())
            })
            .map(|v| json!({"id":v["id"],"value":v["value"]}))
            .collect(),
        Value::Object(values) => values
            .iter()
            .filter(|(_, v)| v.is_string() || v.is_boolean())
            .map(|(id, value)| json!({"id":id,"value":value}))
            .collect(),
        _ => vec![],
    }
}
fn legacy_options(value: &Value, active: Option<&str>, legacy: &Value) -> Map<String, Value> {
    let mut result = Map::new();
    for kind in ["codex", "claudeAgent", "cursor", "opencode"] {
        let mut options = coerce_options(&value[kind]);
        if kind == "codex" && active == Some("codex") {
            for (id, value) in [
                (
                    "reasoningEffort",
                    legacy
                        .get("effort")
                        .filter(|v| v.as_str().is_some_and(|s| !s.is_empty()))
                        .cloned(),
                ),
                (
                    "fastMode",
                    (legacy["codexFastMode"] == true || legacy["serviceTier"] == "fast")
                        .then_some(json!(true)),
                ),
            ] {
                if let Some(value) = value
                    && !options.iter().any(|v| v["id"] == id)
                {
                    options.push(json!({"id":id,"value":value}));
                }
            }
        }
        if !options.is_empty() {
            result.insert(kind.into(), options.into());
        }
    }
    result
}
fn normalize_selection(
    candidate: &Value,
    legacy: &Value,
    options: &Map<String, Value>,
) -> Option<Value> {
    let instance = candidate
        .get("instanceId")
        .or_else(|| candidate.get("provider"))
        .or_else(|| legacy.get("provider"))
        .and_then(provider)?;
    let model = candidate
        .get("model")
        .or_else(|| legacy.get("model"))?
        .as_str()?
        .trim();
    if model.is_empty() {
        return None;
    }
    let kind = string(candidate, "provider")
        .or_else(|| string(legacy, "provider"))
        .unwrap_or("codex");
    let model = if candidate.get("instanceId").is_some() {
        model
    } else {
        MODEL_SLUG_ALIASES_BY_PROVIDER
            .iter()
            .find(|(k, _)| *k == kind)
            .and_then(|(_, aliases)| aliases.iter().find(|(alias, _)| *alias == model))
            .map_or(model, |(_, slug)| slug)
    };
    let selected = if candidate["options"].is_array() || candidate["options"].is_object() {
        coerce_options(&candidate["options"])
    } else {
        options.get(instance).map_or_else(Vec::new, coerce_options)
    };
    let mut value = json!({"instanceId":instance,"model":model});
    if !selected.is_empty() {
        value["options"] = selected.into();
    }
    serde_json::from_value::<ModelSelection>(value.clone())
        .ok()
        .map(|_| value)
}
fn model_state(
    draft: &Value,
    map_key: &str,
    active_key: &str,
    selection_key: &str,
    legacy: &Value,
) -> (Map<String, Value>, Value) {
    if let Some(values) = draft.get(map_key).and_then(Value::as_object) {
        let picks = values
            .iter()
            .filter_map(|(id, v)| {
                serde_json::from_value::<ModelSelection>(v.clone())
                    .ok()
                    .and_then(|selection| serde_json::to_value(selection).ok())
                    .map(|v| (id.clone(), v))
            })
            .collect();
        return (
            picks,
            draft
                .get(active_key)
                .and_then(provider)
                .map_or(Value::Null, |s| json!(s)),
        );
    }
    let mut options = legacy_options(&legacy["modelOptions"], None, legacy);
    let mut selection = normalize_selection(&draft[selection_key], legacy, &options);
    if let Some(pick) = &selection {
        let instance = string(pick, "instanceId");
        let extras = legacy_options(&legacy["modelOptions"], instance, legacy);
        options.extend(extras);
        if let Some(instance) = instance
            && pick.get("options").is_some()
        {
            options.insert(instance.into(), pick["options"].clone());
        }
        selection = normalize_selection(&draft[selection_key], legacy, &options);
        if let Some(pick) = selection.as_mut()
            && let Some(opts) = options.get(string(pick, "instanceId").unwrap_or_default())
        {
            pick["options"] = opts.clone();
        }
    }
    let mut picks = Map::new();
    for (instance, opts) in options {
        let model = selection
            .as_ref()
            .filter(|s| s["instanceId"] == instance)
            .and_then(|s| string(s, "model"))
            .unwrap_or_else(|| {
                DEFAULT_MODEL_BY_PROVIDER
                    .iter()
                    .find(|(k, _)| *k == instance)
                    .map_or(DEFAULT_MODEL, |(_, v)| *v)
            });
        picks.insert(
            instance.clone(),
            json!({"instanceId":instance,"model":model,"options":opts}),
        );
    }
    let active = selection
        .as_ref()
        .map_or(Value::Null, |s| s["instanceId"].clone());
    if let Some(selection) = selection {
        picks.insert(
            string(&selection, "instanceId").unwrap_or_default().into(),
            selection,
        );
    }
    (picks, active)
}
pub fn context_id(producer: &str) -> String {
    if !producer.is_empty()
        && producer.len() <= 128
        && producer
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    {
        return producer.into();
    }
    let mut slug = String::new();
    let mut dash = false;
    for c in producer.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '_' | '-') {
            slug.push(c);
            dash = false;
        } else if !dash {
            slug.push('-');
            dash = true;
        }
    }
    let slug: String = slug.trim_matches('-').chars().take(48).collect();
    let units: Vec<_> = producer.encode_utf16().collect();
    let mut forward = 0x811c9dc5u32;
    let mut reverse = 0x9dc5811cu32;
    for i in 0..units.len() {
        forward = (forward ^ u32::from(units[i])).wrapping_mul(0x01000193);
        reverse = (reverse ^ u32::from(units[units.len() - 1 - i])).wrapping_mul(0x01000193);
    }
    format!(
        "{}-{forward:08x}{reverse:08x}",
        if slug.is_empty() { "ctx" } else { &slug }
    )
}
pub fn kind_context_id(kind: &str, id: &str) -> String {
    context_id(&format!("{kind}_{id}"))
}
pub(super) fn context_link(kind: &str, id: &str, label: &str) -> String {
    let clean = label
        .replace(['[', ']', '\\', '\r', '\n'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "{}[{}](t3-context://v1/{kind}/{id})",
        if kind == "image" { "!" } else { "" },
        if clean.is_empty() { kind } else { &clean }
    )
}
pub(super) fn ensure_reference(prompt: &mut String, kind: &str, id: &str, label: &str) {
    if prompt.contains(&format!("](t3-context://v1/{kind}/{id})")) {
        return;
    }
    if prompt
        .chars()
        .next_back()
        .is_some_and(|c| !matches!(c, ' ' | '\n' | '\r' | '\t'))
    {
        prompt.push(' ');
    }
    prompt.push_str(&context_link(kind, id, label));
    prompt.push(' ');
}
fn normalize_draft(value: &Value) -> Value {
    let value = super::web_normalize::fields(value);
    let value = &value;
    let mut result = record(value);
    let mut prompt = string(value, "prompt").unwrap_or_default().to_string();
    let mut ids = BTreeMap::new();
    for (kind, key) in [
        ("image", "attachments"),
        ("file", "files"),
        ("terminal", "terminalContexts"),
        ("review-comment", "reviewComments"),
        ("preview-annotation", "previewAnnotations"),
    ] {
        for entry in array(value, key) {
            let Some(id) = string(&entry, "id") else {
                continue;
            };
            let next = kind_context_id(kind, id);
            for prior in [id.to_string(), context_id(id), next.clone()] {
                ids.insert(format!("{kind}/{prior}"), next.clone());
            }
            if kind == "preview-annotation" {
                ids.insert(
                    format!("{kind}/{}", context_id(&format!("annotation-{id}"))),
                    next,
                );
            }
        }
    }
    for (old, new) in ids {
        prompt = prompt.replace(
            &format!("t3-context://v1/{old})"),
            &format!(
                "t3-context://v1/{}/{new})",
                old.split('/').next().unwrap_or_default()
            ),
        );
    }
    let terminals = array(value, "terminalContexts");
    let references: Vec<_> = terminals
        .iter()
        .filter_map(|t| {
            let id = kind_context_id("terminal", string(t, "id")?);
            let start = t3_contracts::wire::format_js_number(t["lineStart"].as_f64()?);
            let end = t3_contracts::wire::format_js_number(t["lineEnd"].as_f64()?);
            let label = format!(
                "{} {}",
                string(t, "terminalLabel")?,
                if start == end {
                    format!("line {start}")
                } else {
                    format!("lines {start}-{end}")
                }
            );
            Some((id, label))
        })
        .collect();
    let mut placeholders = references.iter();
    prompt = prompt
        .chars()
        .map(|c| {
            if c == '\u{fffc}' {
                placeholders.next().map_or_else(String::new, |(id, label)| {
                    context_link("terminal", id, label)
                })
            } else {
                c.to_string()
            }
        })
        .collect();
    for (id, label) in references {
        ensure_reference(&mut prompt, "terminal", &id, &label);
    }
    result.insert("prompt".into(), prompt.into());
    result.entry("attachments").or_insert_with(|| json!([]));
    if !terminals.is_empty() {
        result.insert("terminalContexts".into(), terminals.into());
    }
    let (picks, active) = model_state(
        value,
        "modelSelectionByProvider",
        "activeProvider",
        "modelSelection",
        value,
    );
    for field in [
        "modelSelectionByProvider",
        "activeProvider",
        "modelSelectionExplicit",
    ] {
        result.remove(field);
    }
    if !picks.is_empty() || !active.is_null() {
        result.insert("modelSelectionByProvider".into(), picks.into());
        result.insert("activeProvider".into(), active);
        if value["modelSelectionByProvider"].is_object() && value["modelSelectionExplicit"] == true
        {
            result.insert("modelSelectionExplicit".into(), json!(true));
        }
    }
    for field in [
        "files",
        "terminalContexts",
        "previewAnnotations",
        "reviewComments",
        "threadContexts",
        "environmentContexts",
    ] {
        if result
            .get(field)
            .is_some_and(|value| value.as_array().is_some_and(Vec::is_empty))
        {
            result.remove(field);
        }
    }
    Value::Object(result)
}
pub fn normalize_state(state: &Value, now: &str, strip_legacy_seeds: bool) -> Value {
    let mut result = record(state);
    let mappings = [
        "logicalProjectDraftThreadKeyByLogicalProjectKey",
        "projectDraftThreadKeyByProjectKey",
        "projectDraftThreadIdByProjectKey",
        "projectDraftThreadIdByProjectId",
    ]
    .iter()
    .find_map(|k| state.get(*k).filter(|v| !v.is_null()))
    .map_or_else(Map::new, record);
    let source_sessions = state
        .get("draftThreadsByThreadKey")
        .or_else(|| state.get("draftThreadsByThreadId"))
        .map_or_else(Map::new, record);
    let mut environments = BTreeMap::new();
    for (project, thread) in &mappings {
        if let (Some((environment, _)), Some(thread)) = (scoped(project), thread.as_str()) {
            environments.insert(thread.to_string(), environment.to_string());
        }
    }
    let mut sessions = Map::new();
    for (key, value) in source_sessions {
        let mut session = record(&value);
        let environment = scoped(&key)
            .map(|(e, _)| e)
            .or_else(|| nonempty(&value, "environmentId"))
            .map(str::to_string)
            .or_else(|| environments.get(&key).cloned());
        let Some(environment) = environment else {
            continue;
        };
        let Some(project) = nonempty(&value, "projectId") else {
            continue;
        };
        let thread = scoped(&key)
            .map(|(_, t)| t)
            .or_else(|| nonempty(&value, "threadId"))
            .unwrap_or(&key);
        session.insert("threadId".into(), thread.into());
        session.insert("environmentId".into(), environment.clone().into());
        session.insert(
            "logicalProjectKey".into(),
            nonempty(&value, "logicalProjectKey")
                .map(str::to_string)
                .unwrap_or_else(|| {
                    if scoped(&key).is_some() {
                        format!("{environment}:{project}")
                    } else {
                        key.clone()
                    }
                })
                .into(),
        );
        session.insert(
            "createdAt".into(),
            nonempty(&value, "createdAt").unwrap_or(now).into(),
        );
        if serde_json::from_value::<t3_contracts::provider_policy::RuntimeMode>(
            value["runtimeMode"].clone(),
        )
        .is_err()
        {
            session.insert("runtimeMode".into(), json!("full-access"));
        }
        if !matches!(string(&value, "interactionMode"), Some("default" | "plan")) {
            session.insert("interactionMode".into(), json!("default"));
        }
        for field in ["branch", "worktreePath"] {
            if !value[field].is_string() {
                session.insert(field.into(), Value::Null);
            }
        }
        if !matches!(string(&value, "envMode"), Some("local" | "worktree")) {
            session.insert(
                "envMode".into(),
                json!(if nonempty(&value, "worktreePath").is_some() {
                    "worktree"
                } else {
                    "local"
                }),
            );
        }
        session.insert(
            "startFromOrigin".into(),
            json!(value["startFromOrigin"] == true),
        );
        session.entry("promotedTo").or_insert(Value::Null);
        sessions.insert(key, session.into());
    }
    for (logical, thread) in &mappings {
        let Some(thread) = thread.as_str().filter(|s| !s.is_empty()) else {
            continue;
        };
        if let Some(existing) = sessions.get_mut(thread) {
            if existing["logicalProjectKey"] == *logical {
                continue;
            }
            if let Some((environment, project)) = scoped(logical) {
                existing["environmentId"] = json!(environment);
                existing["projectId"] = json!(project);
            }
            existing["logicalProjectKey"] = json!(logical);
        } else if let Some((environment, project)) = scoped(logical) {
            sessions.insert(thread.into(),json!({"threadId":scoped(thread).map_or(thread,|(_,id)|id),"environmentId":environment,"projectId":project,"logicalProjectKey":logical,"createdAt":now,"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null,"envMode":"local","startFromOrigin":false,"promotedTo":null}));
        }
    }
    for (key, session) in &sessions {
        if let Some((_, id)) = scoped(key)
            && let Some(environment) = string(session, "environmentId")
        {
            environments.insert(id.into(), environment.into());
        }
    }
    let source = state
        .get("draftsByThreadKey")
        .or_else(|| state.get("draftsByThreadId"))
        .map_or_else(Map::new, record);
    let mut drafts = Map::new();
    for (key, value) in source {
        if !value.is_object() {
            continue;
        }
        let target = if scoped(&key).is_none() && !sessions.contains_key(&key) {
            environments
                .get(&key)
                .map_or_else(|| key.clone(), |e| format!("{e}:{key}"))
        } else {
            key.clone()
        };
        let mut draft = normalize_draft(&value);
        if !web_has_user_content(&draft)
            && string(&draft, "prompt").is_none_or(str::is_empty)
            && ["runtimeMode", "interactionMode", "activeProvider"]
                .iter()
                .all(|field| draft.get(*field).is_none_or(Value::is_null))
            && record(&draft["modelSelectionByProvider"]).is_empty()
        {
            continue;
        }
        if strip_legacy_seeds
            && sessions.contains_key(&target)
            && draft["modelSelectionExplicit"] != true
            && !web_has_user_content(&draft)
            && let Some(object) = draft.as_object_mut()
        {
            for field in [
                "modelSelectionByProvider",
                "activeProvider",
                "modelSelectionExplicit",
            ] {
                object.remove(field);
            }
            if object.get("runtimeMode").is_none_or(Value::is_null)
                && object.get("interactionMode").is_none_or(Value::is_null)
            {
                continue;
            }
        }
        drafts.insert(target, draft);
    }
    let legacy = json!({"provider":state.get("stickyProvider").cloned().unwrap_or(json!("codex")),"model":state["stickyModel"],"modelOptions":state["stickyModelOptions"]});
    let (picks, mut active) = model_state(
        state,
        "stickyModelSelectionByProvider",
        "stickyActiveProvider",
        "stickyModelSelection",
        &legacy,
    );
    if state.get("stickyModelSelectionByProvider").is_none() {
        active = state
            .get("stickyProvider")
            .and_then(provider)
            .map_or(Value::Null, |s| json!(s));
    }
    let mut memory = Map::new();
    if let Some(values) = state
        .get("stickyOptionsByModelByProvider")
        .and_then(Value::as_object)
    {
        for (instance, models) in values {
            for (model, options) in record(models) {
                if options.as_array().is_some_and(|a| !a.is_empty()) {
                    memory.entry(instance.clone()).or_insert_with(|| json!({}))[model] = options;
                }
            }
        }
    } else {
        for (instance, pick) in &picks {
            if pick["options"].as_array().is_some_and(|a| !a.is_empty()) {
                memory.insert(
                    instance.clone(),
                    json!({string(pick,"model").unwrap_or_default():pick["options"]}),
                );
            }
        }
    }
    result.insert("draftsByThreadKey".into(), drafts.into());
    result.insert("draftThreadsByThreadKey".into(), sessions.into());
    result.insert(
        "logicalProjectDraftThreadKeyByLogicalProjectKey".into(),
        mappings.into(),
    );
    result.insert("stickyModelSelectionByProvider".into(), picks.into());
    result.insert("stickyActiveProvider".into(), active);
    result.insert("stickyOptionsByModelByProvider".into(), memory.into());
    Value::Object(result)
}

pub(super) fn review_label(comment: &Value) -> String {
    let section = string(comment, "sectionId").unwrap_or_default();
    let file = string(comment, "filePath").unwrap_or_default();
    let pull_request = comment.get("pullRequest").filter(|value| !value.is_null());
    let legacy_number = file
        .strip_prefix("PR #")
        .filter(|number| !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit()));
    let summary = pull_request.is_some()
        || (section.starts_with("pull-request:")
            && string(comment, "diff")
                .unwrap_or_default()
                .trim()
                .is_empty()
            && legacy_number.is_some());
    if summary {
        if let Some(number) = pull_request.and_then(|value| value.get("number")) {
            return format!("#{number}");
        }
        if let Some(number) = legacy_number {
            return format!("#{number}");
        }
    }
    let range = string(comment, "rangeLabel").unwrap_or_default();
    let normalized = range
        .chars()
        .next()
        .filter(|sign| matches!(sign, '+' | '-'))
        .and_then(|sign| {
            let numbers = range[1..].split_once(" to ");
            let (first, last) = numbers.map_or((&range[1..], None), |(first, last)| {
                (first, last.strip_prefix(sign))
            });
            let digits =
                |value: &str| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit());
            if !digits(first) || numbers.is_some() && last.is_none_or(|last| !digits(last)) {
                return None;
            }
            Some(format!(
                "L{first}{}{}",
                last.map_or(String::new(), |last| format!(" to L{last}")),
                if sign == '-' { " (before)" } else { "" }
            ))
        })
        .unwrap_or_else(|| range.into());
    let prefix = section
        .strip_prefix("pull-request:")
        .filter(|number| !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit()))
        .map_or(String::new(), |number| format!("#{number} "));
    format!(
        "{prefix}{} {normalized}",
        file.rsplit(['\\', '/']).next().unwrap_or(file)
    )
}
pub(super) fn preview_label(annotation: &Value) -> String {
    let comment = string(annotation, "comment")
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if comment.is_empty() {
        return string(annotation, "pageTitle")
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .unwrap_or("Preview annotation")
            .into();
    }
    if super::super::document::utf16_len(&comment) <= 48 {
        return comment;
    }
    let mut length = 0;
    let prefix: String = comment
        .chars()
        .take_while(|ch| {
            length += ch.len_utf16();
            length <= 47
        })
        .collect();
    format!("{prefix}…")
}
pub fn hydrate_draft(value: &Value) -> Value {
    let mut draft = record(value);
    let mut prompt = string(value, "prompt").unwrap_or_default().to_string();
    draft.insert(
        "persistedAttachments".into(),
        array(value, "attachments").into(),
    );
    draft.insert("nonPersistedImageIds".into(), json!([]));
    let images: Vec<_> = array(value, "attachments")
        .into_iter()
        .filter(|attachment| hydrate_image(attachment).is_some())
        .map(|mut attachment| {
            attachment["type"] = json!("image");
            attachment["previewUrl"] = attachment["dataUrl"].clone();
            attachment
        })
        .collect();
    draft.insert("images".into(), images.into());
    let files: Vec<_> = array(value, "files")
        .into_iter()
        .map(|mut file| {
            file["type"] = json!("file");
            file["file"] = Value::Null;
            if file["attachmentId"].is_string() && file["environmentId"].is_string() {
                file["uploadedAttachmentId"] = file["attachmentId"].clone();
                file["uploadEnvironmentId"] = file["environmentId"].clone();
            }
            file
        })
        .collect();
    draft.insert("files".into(), files.into());
    for (key, kind, label) in [
        (
            "reviewComments",
            "review-comment",
            review_label as fn(&Value) -> String,
        ),
        ("previewAnnotations", "preview-annotation", preview_label),
    ] {
        for context in array(value, key) {
            if let Some(id) = string(&context, "id") {
                ensure_reference(
                    &mut prompt,
                    kind,
                    &kind_context_id(kind, id),
                    &label(&context),
                );
            }
        }
    }
    for (key, kind) in [
        ("threadContexts", "thread"),
        ("environmentContexts", "environment"),
    ] {
        for context in array(value, key) {
            if let (Some(id), Some(label)) =
                (string(&context, "contextId"), string(&context, "label"))
            {
                ensure_reference(&mut prompt, kind, id, label);
            }
        }
    }
    for file in array(value, "files") {
        if let Some(id) = string(&file, "id") {
            ensure_reference(
                &mut prompt,
                "file",
                &kind_context_id("file", id),
                string(&file, "name").unwrap_or("file"),
            );
        }
    }
    let terminals: Vec<_> = array(value, "terminalContexts")
        .into_iter()
        .map(|mut terminal| {
            if !terminal["text"].is_string() {
                terminal["text"] = json!("");
            }
            terminal
        })
        .collect();
    draft.insert("terminalContexts".into(), terminals.into());
    draft.insert("prompt".into(), prompt.into());
    for key in [
        "terminalContexts",
        "previewAnnotations",
        "reviewComments",
        "threadContexts",
        "environmentContexts",
    ] {
        draft.entry(key).or_insert_with(|| json!([]));
    }
    draft
        .entry("modelSelectionByProvider")
        .or_insert_with(|| json!({}));
    for key in [
        "replyTo",
        "activeProvider",
        "runtimeMode",
        "interactionMode",
    ] {
        draft.entry(key).or_insert(Value::Null);
    }
    Value::Object(draft)
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HydratedImage {
    pub id: String,
    pub name: String,
    pub mime_type: String,
    pub bytes: Vec<u8>,
    pub preview_url: String,
}
pub fn hydrate_image(attachment: &Value) -> Option<HydratedImage> {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let data = string(attachment, "dataUrl")?;
    let (header, payload) = data.split_once(',')?;
    if payload.is_empty() {
        return None;
    }
    let mut mime = string(attachment, "mimeType")?.to_string();
    let bytes = if header.contains(";base64") {
        let compact: String = payload
            .chars()
            .filter(|c| !matches!(c, '\t' | '\n' | '\u{c}' | '\r' | ' '))
            .collect();
        let mut padded = compact;
        while !padded.len().is_multiple_of(4) {
            padded.push('=');
        }
        STANDARD.decode(padded).ok()?
    } else {
        let mut decoded = Vec::new();
        let mut i = 0;
        while i < payload.len() {
            if payload.as_bytes()[i] == b'%' {
                decoded.push(u8::from_str_radix(payload.get(i + 1..i + 3)?, 16).ok()?);
                i += 3;
            } else {
                decoded.push(payload.as_bytes()[i]);
                i += 1;
            }
        }
        String::from_utf8(decoded.clone()).ok()?;
        if let Some(raw) = header
            .strip_prefix("data:")
            .and_then(|s| s.split_once(';'))
            .map(|(mime, _)| mime)
            .filter(|s| !s.is_empty())
        {
            mime = raw.into();
        }
        decoded
    };
    Some(HydratedImage {
        id: string(attachment, "id")?.into(),
        name: string(attachment, "name")?.into(),
        mime_type: mime,
        bytes,
        preview_url: data.into(),
    })
}
pub fn partialize_state(state: &Value, open_previews: &BTreeSet<String>) -> Value {
    let mut result = record(state);
    let mut sessions = record(&state["draftThreadsByThreadKey"]);
    let mut drafts = record(&state["draftsByThreadKey"]);
    let mapped: BTreeSet<_> = record(&state["logicalProjectDraftThreadKeyByLogicalProjectKey"])
        .values()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    let discard: Vec<_> = sessions
        .iter()
        .filter(|(key, session)| {
            !mapped.contains(*key)
                && session["promotedTo"].is_null()
                && !web_has_user_content(&state["draftsByThreadKey"][*key])
                && !open_previews.contains(*key)
        })
        .map(|(key, _)| key.clone())
        .collect();
    for key in discard {
        sessions.remove(&key);
        drafts.remove(&key);
    }
    for value in drafts.values_mut() {
        if !value.is_object() {
            continue;
        }
        if let Some(persisted) = value.get("persistedAttachments").cloned() {
            value["attachments"] = persisted;
        }
        let files: Vec<_> = array(value, "files")
            .into_iter()
            .map(|mut file| {
                if file["uploadedAttachmentId"].is_string()
                    && file["uploadEnvironmentId"].is_string()
                {
                    file["attachmentId"] = file["uploadedAttachmentId"].clone();
                    file["environmentId"] = file["uploadEnvironmentId"].clone();
                }
                if let Some(object) = file.as_object_mut() {
                    for key in [
                        "file",
                        "previewUrl",
                        "uploadedAttachmentId",
                        "uploadEnvironmentId",
                        "type",
                    ] {
                        object.remove(key);
                    }
                }
                file
            })
            .collect();
        value["files"] = files.into();
        if let Some(object) = value.as_object_mut() {
            for key in ["images", "persistedAttachments", "nonPersistedImageIds"] {
                object.remove(key);
            }
            for key in ["runtimeMode", "interactionMode", "replyTo"] {
                if object.get(key).is_some_and(Value::is_null) {
                    object.remove(key);
                }
            }
        }
    }
    result.insert("draftsByThreadKey".into(), drafts.into());
    result.insert("draftThreadsByThreadKey".into(), sessions.into());
    result.into()
}
pub fn mark_promoting(state: &mut Value, draft_key: &str, destination: Option<&Value>) -> bool {
    let Some(session) = state
        .get_mut("draftThreadsByThreadKey")
        .and_then(|v| v.get_mut(draft_key))
        .filter(|v| v.is_object())
    else {
        return false;
    };
    let destination = destination.cloned().unwrap_or_else(
        || json!({"environmentId":session["environmentId"],"threadId":session["threadId"]}),
    );
    if session["promotedTo"] == destination {
        return false;
    }
    session["promotedTo"] = destination;
    true
}
pub fn finalize_promotion(state: &mut Value, draft_key: &str) -> bool {
    let Some(destination) = state
        .get("draftThreadsByThreadKey")
        .and_then(|v| v.get(draft_key))
        .and_then(|s| s.get("promotedTo"))
        .filter(|v| !v.is_null())
        .cloned()
    else {
        return false;
    };
    let (Some(environment), Some(thread)) = (
        string(&destination, "environmentId"),
        string(&destination, "threadId"),
    ) else {
        return false;
    };
    let target = format!("{environment}:{thread}");
    if let Some(sessions) = state["draftThreadsByThreadKey"].as_object_mut() {
        sessions.remove(draft_key);
    }
    if let Some(drafts) = state["draftsByThreadKey"].as_object_mut()
        && let Some(draft) = drafts.remove(draft_key)
    {
        drafts.insert(target, draft);
    }
    if let Some(mappings) = state["logicalProjectDraftThreadKeyByLogicalProjectKey"].as_object_mut()
    {
        mappings.retain(|_, value| value != draft_key);
    }
    if state.get("promotedDraftRedirects").is_none() {
        state["promotedDraftRedirects"] = json!({});
    }
    state["promotedDraftRedirects"][draft_key] = destination;
    true
}
pub fn restore_failed_session(
    state: &mut Value,
    draft_key: &str,
    snapshot: &Value,
    thread_id: &str,
    now: &str,
) {
    let mut session = state["draftThreadsByThreadKey"]
        .get(draft_key)
        .cloned()
        .unwrap_or_else(|| snapshot.clone());
    session["threadId"] = json!(thread_id);
    session["promotedTo"] = Value::Null;
    if snapshot["threadId"] != thread_id {
        session["createdAt"] = json!(now);
    }
    if !state["draftThreadsByThreadKey"].is_object() {
        state["draftThreadsByThreadKey"] = json!({});
    }
    state["draftThreadsByThreadKey"][draft_key] = session;
}
pub fn promotion_ready(shell: Option<&Value>, background_pending: bool) -> bool {
    if background_pending {
        return false;
    }
    shell.is_some_and(|shell| {
        !shell["latestUserMessageAt"].is_null()
            || !shell["latestRun"]["startedAt"].is_null()
            || matches!(
                shell["latestRun"]["status"].as_str(),
                Some("failed" | "interrupted" | "cancelled")
            )
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_models_options_aliases_and_custom_instances() {
        let state = normalize_state(
            &json!({"draftsByThreadId":{"a":{"prompt":"keep","modelSelection":{"provider":"codex","model":"5.3"},"effort":"high","serviceTier":"fast"},"b":{"prompt":"custom","modelSelection":{"instanceId":"secondary","model":" 5.3 ","options":{"fastMode":false}}}},"stickyProvider":"cursor","stickyModel":"composer"}),
            "now",
            true,
        );
        assert_eq!(
            state["draftsByThreadKey"]["a"]["modelSelectionByProvider"]["codex"]["model"],
            "gpt-5.3-codex"
        );
        assert_eq!(
            state["draftsByThreadKey"]["a"]["modelSelectionByProvider"]["codex"]["options"],
            json!([{"id":"reasoningEffort","value":"high"},{"id":"fastMode","value":true}])
        );
        assert_eq!(
            state["draftsByThreadKey"]["b"]["modelSelectionByProvider"]["secondary"]["model"],
            "5.3"
        );
        assert_eq!(
            state["stickyModelSelectionByProvider"]["cursor"]["model"],
            "composer-2"
        );
    }
    #[test]
    fn session_scope_recovery_and_context_migration() {
        let state = normalize_state(
            &json!({"draftThreadsByThreadId":{"e:t":{"projectId":"p"}},"projectDraftThreadIdByProjectKey":{"e:p":"e:t"},"draftsByThreadId":{"t":{"prompt":"look ￼","terminalContexts":[{"id":"x","threadId":"t","terminalId":" tty ","terminalLabel":" Term ","createdAt":"old","lineStart":0.2,"lineEnd":0}]}}}),
            "now",
            true,
        );
        assert_eq!(state["draftThreadsByThreadKey"]["e:t"]["createdAt"], "now");
        assert!(state["draftsByThreadKey"].get("t").is_none());
        assert_eq!(
            state["draftsByThreadKey"]["e:t"]["prompt"],
            "look [Term line 1](t3-context://v1/terminal/terminal_x)"
        );
    }
    #[test]
    fn image_hydration_keeps_failed_originals() {
        let data = json!({"id":"a","name":"x","mimeType":"image/png","sizeBytes":3,"dataUrl":"data:image/png;base64,YWJj"});
        assert_eq!(hydrate_image(&data).expect("base64").bytes, b"abc");
        let raw = json!({"attachments":[data,{"dataUrl":"bad"}],"files":[{"id":"f","attachmentId":"upload","environmentId":"e"}]});
        let hydrated = hydrate_draft(&raw);
        assert_eq!(
            hydrated["persistedAttachments"]
                .as_array()
                .expect("persisted")
                .len(),
            2
        );
        assert_eq!(hydrated["images"].as_array().expect("images").len(), 1);
        assert_eq!(hydrated["files"][0]["uploadedAttachmentId"], "upload");
        assert!(hydrated["files"][0]["file"].is_null());
    }
    #[test]
    fn partialization_and_promotion_lifecycle() {
        let mut state = normalize_state(
            &json!({"draftThreadsByThreadId":{"draft":{"threadId":"t","environmentId":"e","projectId":"p"}},"draftsByThreadId":{"draft":{"prompt":"text"}},"projectDraftThreadIdByProjectKey":{"logical":"draft"}}),
            "now",
            false,
        );
        assert!(mark_promoting(
            &mut state,
            "draft",
            Some(&json!({"environmentId":"other","threadId":"real"}))
        ));
        assert!(
            partialize_state(&state, &BTreeSet::new())["draftThreadsByThreadKey"]
                .get("draft")
                .is_some()
        );
        assert!(finalize_promotion(&mut state, "draft"));
        assert!(!finalize_promotion(&mut state, "draft"));
        assert_eq!(state["draftsByThreadKey"]["other:real"]["prompt"], "text");
        assert_eq!(state["promotedDraftRedirects"]["draft"]["threadId"], "real");
        assert!(
            state["logicalProjectDraftThreadKeyByLogicalProjectKey"]
                .as_object()
                .expect("mappings")
                .is_empty()
        );
        assert!(promotion_ready(
            Some(&json!({"latestRun":{"status":"failed"}})),
            false
        ));
        assert!(!promotion_ready(
            Some(&json!({"latestUserMessageAt":"now"})),
            true
        ));
    }
}
