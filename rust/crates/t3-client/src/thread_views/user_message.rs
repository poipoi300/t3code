//! Ports userMessage.ts and TimelineUserMessageRow.tsx presentation, without rendering or I/O.

use super::messages_timeline::{MessagesTimelineRow, MessagesTimelineRowContent};
use crate::view_logic::{
    JsTrim,
    context_references::collect_references,
    is_js_whitespace,
    session::{
        ChatMessage, MessageAttachment,
        timestamps::{TimestampHost, TimestampPresenter},
    },
};
use serde_json::Value;
use std::{collections::BTreeSet, rc::Rc};
use t3_contracts::{
    chat_attachment::{ChatAttachment, ChatFileAttachment},
    composer_context::{COMPOSER_CONTEXT_LABEL_MAX_CHARS, ComposerContextRecord},
    settings::TimestampFormat,
};

pub const ATTACHMENT_ONLY_BOOTSTRAP_PROMPT: &str = "[User attached one or more files without additional text. Respond using the conversation context and the attached files.]";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Attribution {
    Own,
    Automation,
    Agent,
    T3Code,
    Guest(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Presentation {
    pub text: String,
    pub attribution: Attribution,
    pub scheduled_task_id: Option<String>,
}

pub fn is_attachment_only_bootstrap_prompt(text: &str) -> bool {
    let text = if text
        .get(..11)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("Ultrathink:"))
    {
        text[11..].js_trim_start()
    } else {
        text
    };
    text == ATTACHMENT_ONLY_BOOTSTRAP_PROMPT
}

pub fn resolve_user_message_presentation(message: &ChatMessage) -> Presentation {
    let own = || Presentation {
        text: message.text.clone(),
        attribution: Attribution::Own,
        scheduled_task_id: None,
    };
    if message.role != "user" {
        return own();
    }
    if let Some(guest) = message
        .metadata
        .get("context")
        .and_then(|context| context.get("guest"))
        && let (Some(name), Some(text)) = (guest["name"].as_str(), guest["text"].as_str())
    {
        return Presentation {
            text: text.into(),
            attribution: Attribution::Guest(name.into()),
            scheduled_task_id: None,
        };
    }
    if let Some(id) = message
        .metadata
        .get("scheduledTaskId")
        .and_then(Value::as_str)
    {
        return Presentation {
            text: message.text.clone(),
            attribution: Attribution::Automation,
            scheduled_task_id: Some(id.into()),
        };
    }
    let agent = message.metadata.get("createdBy").and_then(Value::as_str) == Some("agent");
    if let Some(body) = legacy_automation_body(&message.text) {
        let task = legacy_task_id(&message_id(message));
        if agent || task.is_some() {
            return Presentation {
                text: body.into(),
                attribution: Attribution::Automation,
                scheduled_task_id: task,
            };
        }
    }
    Presentation {
        text: if is_attachment_only_bootstrap_prompt(&message.text) {
            String::new()
        } else {
            message.text.clone()
        },
        attribution: if !agent {
            Attribution::Own
        } else if message
            .metadata
            .get("creationSource")
            .and_then(Value::as_str)
            == Some("server")
        {
            Attribution::T3Code
        } else {
            Attribution::Agent
        },
        scheduled_task_id: None,
    }
}

fn legacy_automation_body(text: &str) -> Option<&str> {
    let rest = text.strip_prefix("[Triggered by schedule task: ")?;
    let (header, rest) = rest.split_once('\n')?;
    let header = header
        .strip_suffix('\r')
        .unwrap_or(header)
        .strip_suffix(']')?;
    if header.is_empty() || header.contains('\r') {
        return None;
    }
    rest.strip_prefix("\r\n")
        .or_else(|| rest.strip_prefix('\n'))
}

fn legacy_task_id(id: &str) -> Option<String> {
    let rest = id.strip_prefix("scheduled-task-message:")?;
    let (rest, trigger) = rest.rsplit_once(':')?;
    if !matches!(trigger, "scheduled" | "manual") {
        return None;
    }
    let (task, timestamp) = rest.rsplit_once(':')?;
    (!task.is_empty() && !timestamp.is_empty() && timestamp.bytes().all(|b| b.is_ascii_digit()))
        .then(|| task.into())
}

pub fn message_id(message: &ChatMessage) -> String {
    serde_json::to_value(&message.id)
        .ok()
        .map_or_else(String::new, |id| {
            id.as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| id.to_string())
        })
}

pub fn sender_thread_key(
    message: &ChatMessage,
    active_environment: &crate::types::EnvironmentId,
) -> Option<crate::types::ThreadKey> {
    use crate::types::{EnvironmentId, ThreadId, ThreadKey};
    if let Some(sender) = message.metadata.get("linkedSender") {
        return Some(ThreadKey {
            environment: EnvironmentId(sender["environmentId"].as_str()?.into()),
            thread: ThreadId(sender["threadId"].as_str()?.into()),
        });
    }
    Some(ThreadKey {
        environment: active_environment.clone(),
        thread: ThreadId(message.metadata.get("senderThreadId")?.as_str()?.into()),
    })
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Grouping {
    pub grouped_with_previous: bool,
    pub grouped_with_next: bool,
    pub continues_steer: bool,
}

pub fn resolve_user_message_grouping(
    message: Option<&ChatMessage>,
    previous: Option<&ChatMessage>,
    next: Option<&ChatMessage>,
) -> Grouping {
    let own = |message: Option<&ChatMessage>| {
        message.is_some_and(|m| {
            m.role == "user" && resolve_user_message_presentation(m).attribution == Attribution::Own
        })
    };
    let steer = |message: Option<&ChatMessage>| {
        message.is_some_and(|m| matches!(m.input_intent, Some(t3_contracts::orchestration_v2::OrchestrationV2UserMessageInputIntent::Steer | t3_contracts::orchestration_v2::OrchestrationV2UserMessageInputIntent::PromotedQueuedToSteer)))
    };
    let previous_own = own(message) && own(previous);
    Grouping {
        grouped_with_previous: previous_own,
        grouped_with_next: own(message) && own(next),
        continues_steer: previous_own && steer(message) && steer(previous),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Reference {
    pub kind: String,
    pub context_id: String,
    pub label: String,
    pub source: String,
    pub start: usize,
    pub end: usize,
}

fn sanitize_label(label: &str, kind: &str) -> String {
    let mut cleaned = String::new();
    let mut separator = false;
    for ch in label.chars() {
        if is_js_whitespace(ch) || matches!(ch, '[' | ']' | '\\') {
            separator = !cleaned.is_empty();
            continue;
        }
        if separator {
            cleaned.push(' ');
        }
        cleaned.push(ch);
        separator = false;
    }
    let mut units = 0;
    let end = cleaned.char_indices().find_map(|(index, ch)| {
        units += ch.len_utf16();
        (units > COMPOSER_CONTEXT_LABEL_MAX_CHARS).then_some(index)
    });
    if let Some(end) = end {
        cleaned.truncate(end);
    }
    if cleaned.is_empty() {
        kind.into()
    } else {
        cleaned
    }
}

pub fn context_references(text: &str) -> Vec<Reference> {
    collect_references(text)
        .into_iter()
        .map(|reference| {
            let source = &text[reference.start..reference.end];
            let label = source
                .split_once('[')
                .and_then(|(_, tail)| tail.split_once("](t3-context:"))
                .map_or(reference.kind, |(label, _)| label);
            Reference {
                kind: reference.kind.into(),
                context_id: reference.context_id.into(),
                label: sanitize_label(label, reference.kind),
                source: source.into(),
                start: reference.start,
                end: reference.end,
            }
        })
        .collect()
}

pub fn replace_references(text: &str, replace: impl Fn(&Reference) -> String) -> String {
    let mut result = String::new();
    let mut cursor = 0;
    for reference in context_references(text) {
        result.push_str(&text[cursor..reference.start]);
        result.push_str(&replace(&reference));
        cursor = reference.end;
    }
    result.push_str(&text[cursor..]);
    result
}

pub fn context_records(message: &ChatMessage) -> Vec<ComposerContextRecord> {
    message
        .metadata
        .get("context")
        .and_then(|context| context.get("records"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|record| serde_json::from_value(record.clone()).ok())
        .collect()
}

fn attachment_record(record: &ComposerContextRecord) -> Option<(&str, &str)> {
    match record {
        ComposerContextRecord::Image(record) => {
            Some((&record.context_id.0, &record.attachment_id.0))
        }
        ComposerContextRecord::File(record) => {
            Some((&record.context_id.0, &record.attachment_id.0))
        }
        _ => None,
    }
}

fn generic_mime(mime: &str) -> bool {
    matches!(
        mime,
        "" | "application/octet-stream"
            | "binary/octet-stream"
            | "application/unknown"
            | "video/mp2t"
            | "video/vnd.dlna.mpeg-tts"
    )
}
fn mime(mime: &str) -> String {
    mime.split(';')
        .next()
        .unwrap_or_default()
        .js_trim()
        .to_ascii_lowercase()
}
fn extension(name: &str) -> &str {
    name.rsplit_once('.').map_or("", |(_, extension)| extension)
}

fn image_file(file: &ChatFileAttachment) -> bool {
    let mime = mime(&file.mime_type);
    matches!(
        mime.as_str(),
        "image/png" | "image/jpeg" | "image/gif" | "image/webp"
    ) || generic_mime(&mime)
        && matches!(
            extension(&file.name)
                .js_trim()
                .to_ascii_lowercase()
                .as_str(),
            "png" | "jpg" | "jpeg" | "gif" | "webp"
        )
}
pub fn is_video_file(file: &ChatFileAttachment) -> bool {
    let mime = mime(&file.mime_type);
    mime.starts_with("video/") && !generic_mime(&mime)
        || generic_mime(&mime)
            && matches!(
                extension(&file.name).to_ascii_lowercase().as_str(),
                "avi" | "m4v" | "mkv" | "mov" | "mp4" | "ogv" | "webm"
            )
}
pub fn is_audio_file(file: &ChatFileAttachment) -> bool {
    let mime = mime(&file.mime_type);
    mime.starts_with("audio/")
        || (generic_mime(&mime) || mime == "text/plain")
            && matches!(
                extension(&file.name).to_ascii_lowercase().as_str(),
                "mp3"
                    | "wav"
                    | "ogg"
                    | "oga"
                    | "flac"
                    | "aac"
                    | "m4a"
                    | "opus"
                    | "aiff"
                    | "webm-audio"
                    | "weba"
            )
}

pub fn resolve_user_message_file_cards(message: &ChatMessage) -> Option<Vec<ChatFileAttachment>> {
    let attachments = message.attachments.as_deref()?;
    let files: Vec<_> = attachments
        .iter()
        .filter_map(|attachment| match &attachment.attachment {
            ChatAttachment::File(file)
                if !image_file(file) && !is_video_file(file) && !is_audio_file(file) =>
            {
                Some(file.clone())
            }
            _ => None,
        })
        .collect();
    if files.is_empty() || files.len() != attachments.len() {
        return None;
    }
    let records = context_records(message);
    let remaining = replace_references(&message.text, |reference| {
        if reference.kind == "file" && records.iter().any(|record| matches!(record, ComposerContextRecord::File(file) if file.context_id.0 == reference.context_id && files.iter().any(|attachment| attachment.id == file.attachment_id.0))) { String::new() }
        else { reference.label.clone() }
    });
    remaining.js_trim().is_empty().then_some(files)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Intent {
    pub promoted: bool,
    pub unread: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UserRow {
    pub id: String,
    pub text: String,
    pub plain_text: String,
    pub copy_text: String,
    pub prompt_link: Option<String>,
    pub attribution: Attribution,
    pub summary: String,
    pub timestamp: String,
    pub timestamp_tooltip: String,
    pub status: Option<String>,
    pub intent: Option<Intent>,
    pub continues_steer: bool,
    pub grouped_with_previous: bool,
    pub grouped_with_next: bool,
    pub sender_label: Option<String>,
    pub can_open_sender: bool,
    pub scheduled_task_title: Option<String>,
    pub has_scheduled_task: bool,
    pub scheduled_task_id: Option<String>,
    pub revert_turn_count: Option<usize>,
    pub fresh_context: bool,
    pub model_switch: bool,
    pub workflow_opening: bool,
    pub has_media: bool,
    pub has_audio: bool,
    pub has_files: bool,
    pub media_only: bool,
    pub media_above_text: bool,
    pub file_cards_only: bool,
    pub audio_only: bool,
    pub images: Vec<MessageAttachment>,
    pub preview_images: Vec<MessageAttachment>,
    pub videos: Vec<MessageAttachment>,
    pub audio: Vec<MessageAttachment>,
    pub files: Vec<MessageAttachment>,
    pub unknown: Vec<MessageAttachment>,
    pub records: Vec<ComposerContextRecord>,
    pub references: Vec<Reference>,
    pub message: Rc<ChatMessage>,
}

pub fn present_user_message_row(
    row: &MessagesTimelineRow,
    host: impl TimestampHost,
    format: &TimestampFormat,
    now_ms: i64,
) -> Option<UserRow> {
    let MessagesTimelineRowContent::Message {
        message,
        projected_item,
        revert_turn_count,
        grouped_with_previous,
        grouped_with_next,
        continues_steer,
        ..
    } = &row.content
    else {
        return None;
    };
    if message.role != "user" {
        return None;
    }
    let presentation = resolve_user_message_presentation(message);
    let records = context_records(message);
    let references = context_references(&presentation.text);
    let chipped: BTreeSet<_> = references
        .iter()
        .flat_map(|reference| {
            records
                .iter()
                .filter_map(attachment_record)
                .filter_map(move |(context, attachment)| {
                    (context == reference.context_id).then_some(attachment.to_owned())
                })
        })
        .collect();
    let mut images = Vec::new();
    let mut preview_images = Vec::new();
    let mut videos = Vec::new();
    let mut audio = Vec::new();
    let mut all_files = Vec::new();
    let mut unknown = Vec::new();
    for attachment in message.attachments.as_deref().into_iter().flatten() {
        match &attachment.attachment {
            ChatAttachment::Image(image) if image.name.starts_with("preview-annotation-") => {
                preview_images.push(attachment.clone())
            }
            ChatAttachment::Image(_) => images.push(attachment.clone()),
            ChatAttachment::File(file) if is_video_file(file) => videos.push(attachment.clone()),
            ChatAttachment::File(file) if is_audio_file(file) => audio.push(attachment.clone()),
            ChatAttachment::File(_) => all_files.push(attachment.clone()),
            ChatAttachment::Unknown(_) => unknown.push(attachment.clone()),
        }
    }
    let file_id = |attachment: &MessageAttachment| {
        match &attachment.attachment {
            ChatAttachment::File(file) => file.id.as_str(),
            _ => "",
        }
        .to_owned()
    };
    let unchipped: Vec<_> = all_files
        .iter()
        .filter(|file| !chipped.contains(&file_id(file)))
        .cloned()
        .collect();
    let standalone = !matches!(
        presentation.attribution,
        Attribution::Agent | Attribution::Guest(_)
    );
    let media_only = standalone
        && presentation.text.js_trim().is_empty()
        && audio.is_empty()
        && unchipped.is_empty()
        && unknown.is_empty()
        && images.len() + videos.len() > 0;
    let media_above_text = standalone && !media_only && images.len() + videos.len() > 0;
    let only_references = |files: &[MessageAttachment]| {
        replace_references(&presentation.text, |reference| {
            if records
                .iter()
                .filter_map(attachment_record)
                .any(|(context, attachment)| {
                    context == reference.context_id
                        && files.iter().any(|file| file_id(file) == attachment)
                })
            {
                String::new()
            } else {
                reference.label.clone()
            }
        })
        .js_trim()
        .is_empty()
    };
    let file_cards_only = standalone
        && !all_files.is_empty()
        && images.len() + videos.len() + audio.len() + unknown.len() == 0
        && only_references(&all_files);
    let audio_only = standalone
        && !audio.is_empty()
        && images.len() + videos.len() + all_files.len() + unknown.len() == 0
        && only_references(&audio);
    let plain_text = replace_references(&presentation.text, |reference| reference.label.clone());
    let summary = plain_text
        .split('\n')
        .find(|line| !line.js_trim().is_empty())
        .unwrap_or_default()
        .js_trim()
        .to_owned();
    let item = projected_item
        .as_ref()
        .and_then(|item| serde_json::to_value(item).ok())
        .unwrap_or(Value::Null);
    let status = item["item"]["status"].as_str().map(str::to_owned);
    let intent = message.input_intent.as_ref().and_then(|intent| match intent {
        t3_contracts::orchestration_v2::OrchestrationV2UserMessageInputIntent::Steer => Some(Intent { promoted: false, unread: status.as_deref() == Some("pending") }),
        t3_contracts::orchestration_v2::OrchestrationV2UserMessageInputIntent::PromotedQueuedToSteer => Some(Intent { promoted: true, unread: status.as_deref() == Some("pending") }), _ => None,
    });
    let sender_label = message
        .metadata
        .get("linkedSender")
        .and_then(|sender| sender["title"].as_str())
        .map(JsTrim::js_trim)
        .filter(|title| !title.is_empty())
        .map(str::to_owned);
    let copy_text =
        if media_only && presentation.text.is_empty() {
            records
                .iter()
                .filter_map(|record| {
                    let (context_id, attachment_id) = attachment_record(record)?;
                    if !images.iter().chain(&videos).any(|attachment| {
                        match &attachment.attachment {
                            ChatAttachment::Image(image) => image.id == attachment_id,
                            ChatAttachment::File(file) => file.id == attachment_id,
                            _ => false,
                        }
                    }) {
                        return None;
                    }
                    let record = serde_json::to_value(record).ok()?;
                    let kind = record["kind"].as_str()?;
                    let label = sanitize_label(record["label"].as_str()?, kind);
                    Some(format!(
                        "{}[{label}](t3-context://v1/{kind}/{context_id})",
                        if kind == "image" { "!" } else { "" }
                    ))
                })
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            presentation.text.clone()
        };
    let timestamps = TimestampPresenter(host);
    Some(UserRow {
        id: message_id(message),
        text: presentation.text.clone(),
        copy_text,
        prompt_link: prompt_link_for(&plain_text),
        plain_text,
        attribution: presentation.attribution,
        summary,
        timestamp: timestamps.day_aware(&message.created_at, format, now_ms, None, false),
        timestamp_tooltip: timestamps.tooltip(&message.created_at, format),
        status,
        intent,
        continues_steer: continues_steer.unwrap_or(false),
        grouped_with_previous: grouped_with_previous.unwrap_or(false),
        grouped_with_next: grouped_with_next.unwrap_or(false),
        sender_label,
        can_open_sender: message.metadata.contains_key("senderThreadId")
            || message.metadata.contains_key("linkedSender"),
        scheduled_task_title: None,
        has_scheduled_task: presentation.scheduled_task_id.is_some(),
        scheduled_task_id: presentation.scheduled_task_id,
        revert_turn_count: revert_turn_count.and_then(|count| usize::try_from(count).ok()),
        fresh_context: item["item"]["freshContext"] == true,
        model_switch: item["item"]
            .get("modelSwitch")
            .is_some_and(|value| !value.is_null()),
        workflow_opening: false,
        has_media: !images.is_empty() || !videos.is_empty(),
        has_audio: !audio.is_empty(),
        has_files: !unchipped.is_empty() || !unknown.is_empty() || file_cards_only,
        media_only,
        media_above_text,
        file_cards_only,
        audio_only,
        images,
        preview_images,
        videos,
        audio,
        files: if file_cards_only {
            all_files
        } else {
            unchipped
        },
        unknown,
        records,
        references,
        message: message.clone(),
    })
}

pub fn prompt_link_for(text: &str) -> Option<String> {
    let text = if is_attachment_only_bootstrap_prompt(text) {
        ""
    } else {
        text.js_trim()
    };
    if text.is_empty() || text.encode_utf16().count() > 8_000 {
        return None;
    }
    let mut url = url::Url::parse("https://app.t3.codes/new").ok()?;
    url.query_pairs_mut().append_pair("prompt", text);
    Some(url.into())
}

#[cfg(test)]
#[path = "user_message_tests.rs"]
mod tests;
