use super::*;
use crate::view_logic::JsTrim;
use serde_json::json;
use std::{collections::BTreeSet, rc::Weak};
use t3_contracts::assets::AssetResource;

fn attachment_resource(id: &str) -> AssetResource {
    AssetResource::Attachment {
        attachment_id: TrimmedNonEmptyString(id.into()),
        file_name: None,
        mime_type: None,
        disposition: None,
        render_intent: None,
    }
}
pub fn select_message_image_resources(attachments: &[MessageAttachment]) -> Vec<AssetResource> {
    let mut seen = BTreeSet::new();
    attachments
        .iter()
        .filter_map(|a| match &a.attachment {
            ChatAttachment::Image(image)
                if !a
                    .preview_url
                    .as_deref()
                    .is_some_and(|u| u.starts_with("blob:") || u.starts_with("data:"))
                    && seen.insert(&image.id) =>
            {
                Some(attachment_resource(&image.id))
            }
            _ => None,
        })
        .collect()
}
pub fn select_handoff_image_resources(
    messages: &[Rc<ChatMessage>],
    handoffs: &BTreeMap<String, Vec<String>>,
) -> Vec<AssetResource> {
    let mut seen = BTreeSet::new();
    let mut result = vec![];
    for message in messages {
        if message.role != "user" || handoffs.get(&id_key(&message.id)).is_none_or(Vec::is_empty) {
            continue;
        }
        for a in message.attachments.iter().flat_map(|a| a.iter()) {
            if let ChatAttachment::Image(image) = &a.attachment
                && seen.insert(&image.id)
            {
                result.push(attachment_resource(&image.id));
            }
        }
    }
    result
}
#[derive(Default)]
pub struct MessageAttachmentPreviewProjector {
    entries: Vec<(Weak<ChatMessage>, Rc<ChatMessage>)>,
}
impl MessageAttachmentPreviewProjector {
    pub fn project(
        &mut self,
        message: Rc<ChatMessage>,
        preview_url_for: impl Fn(&MessageAttachment) -> Option<String>,
    ) -> Rc<ChatMessage> {
        self.entries.retain(|(source, _)| source.strong_count() > 0);
        let Some(source) = message.attachments.as_ref().filter(|a| !a.is_empty()) else {
            return message;
        };
        let previous = self
            .entries
            .iter()
            .find(|(s, _)| s.ptr_eq(&Rc::downgrade(&message)))
            .map(|(_, m)| m);
        let attachments: Vec<_> = source
            .iter()
            .map(|a| {
                let next = preview_url_for(a)
                    .filter(|s| !s.is_empty() && Some(s) != a.preview_url.as_ref());
                if next.is_none() {
                    a.clone()
                } else {
                    MessageAttachment {
                        attachment: a.attachment.clone(),
                        preview_url: next,
                    }
                }
            })
            .collect();
        if attachments == **source {
            self.entries
                .retain(|(s, _)| !s.ptr_eq(&Rc::downgrade(&message)));
            return message;
        }
        if let Some(previous) = previous
            && previous.attachments.as_deref() == Some(&attachments)
        {
            return previous.clone();
        }
        let mut projected = (*message).clone();
        projected.attachments = Some(Rc::new(attachments));
        let result = Rc::new(projected);
        self.entries
            .retain(|(s, _)| !s.ptr_eq(&Rc::downgrade(&message)));
        self.entries.push((Rc::downgrade(&message), result.clone()));
        result
    }
}
#[derive(Default)]
struct ReadBudget {
    bytes: usize,
    nodes: usize,
    exceeded: bool,
}
fn read_result(value: &Value, budget: &mut ReadBudget, depth: usize) -> (Option<Value>, bool) {
    if depth > 4 || budget.nodes == 0 {
        budget.exceeded = true;
        return (None, false);
    }
    budget.nodes -= 1;
    match value {
        Value::String(text) => {
            if text.len() > budget.bytes {
                budget.exceeded = true;
                return (None, false);
            }
            budget.bytes -= text.len();
            serde_json::from_str(text)
                .ok()
                .map_or((None, false), |v| read_result(&v, budget, depth + 1))
        }
        Value::Array(values) => {
            if values.len() > 32 {
                budget.exceeded = true;
                return (None, false);
            }
            let mut data = None;
            let mut failed = false;
            for block in values {
                let text = &block["text"];
                let text = if text.is_object() {
                    text.get("text").unwrap_or(text)
                } else {
                    text
                };
                let result = read_result(text, budget, depth + 1);
                if data.is_none() {
                    data = result.0;
                }
                failed |= result.1;
                if budget.exceeded {
                    break;
                }
            }
            (data, failed)
        }
        Value::Object(obj) => {
            let failed = value["isError"] == true
                || value["is_error"] == true
                || value["_tag"] == "OrchestratorMcpFailure"
                || obj.get("error").is_some_and(|v| !v.is_null());
            if let Some(content) = obj
                .get("structuredContent")
                .filter(|v| !v.is_null())
                .or_else(|| obj.get("content"))
            {
                let result = read_result(content, budget, depth + 1);
                (result.0, failed || result.1)
            } else {
                (Some(value.clone()), failed)
            }
        }
        _ => (None, false),
    }
}
fn tool_result(item: &TurnItemDynamicTool) -> (Option<Value>, bool) {
    let mut budget = ReadBudget {
        bytes: 16384,
        nodes: 128,
        exceeded: false,
    };
    let result = item
        .output
        .as_ref()
        .and_then(Option::as_ref)
        .map_or((None, false), |v| read_result(v, &mut budget, 0));
    (if budget.exceeded { None } else { result.0 }, result.1)
}
pub fn html_render_from_tool_item(item: &TurnItemDynamicTool) -> Option<Value> {
    let name = item.tool_name.as_ref()?.0.as_str();
    if name != "execute"
        && name != "html_render"
        && !matches!(
            name,
            "mcp__t3__html_render"
                | "mcp__t3_code__html_render"
                | "mcp__t3-code__html_render"
                | "t3.html_render"
                | "t3_code.html_render"
        )
    {
        return None;
    }
    let (output, failed) = tool_result(item);
    if failed {
        return None;
    }
    let output = output?;
    read_html_render_reference(&output["htmlRender"])
}
fn read_html_render_reference(value: &Value) -> Option<Value> {
    let id = value["attachmentId"]
        .as_str()
        .filter(|s| !s.is_empty() && s.encode_utf16().count() <= 256)?;
    let title = value["title"].as_str()?;
    let height = value["height"].as_f64()?;
    let clamp = |n: f64| n.round().clamp(80., 2000.) as u64;
    let mut result = json!({"attachmentId": id, "title": if title.js_trim().is_empty() {"HTML".into()} else {title.js_trim().chars().take(200).collect::<String>()}, "height": clamp(height)});
    if let Some(values) = value["heights"]
        .as_array()
        .filter(|h| !h.is_empty() && h.len() <= 24)
    {
        let heights: Option<Vec<_>> = values
            .iter()
            .map(|v| {
                let pair = v.as_array().filter(|a| a.len() == 2)?;
                let width = pair[0].as_u64().filter(|w| (1..=10000).contains(w))?;
                Some((width, clamp(pair[1].as_f64()?)))
            })
            .collect();
        if let Some(mut heights) = heights {
            heights.sort_by_key(|p| p.0);
            result["heights"] = json!(heights);
        }
    }
    Some(result)
}
pub fn mcp_app_from_tool_item(item: &TurnItemDynamicTool) -> Option<Value> {
    let (output, _) = tool_result(item);
    let output = output?;
    let value = output.get("t3McpApp")?;
    let bounded = |k: &str| {
        value[k]
            .as_str()
            .filter(|s| !s.js_trim().is_empty() && s.encode_utf16().count() <= 256)
    };
    let id = bounded("attachmentId")?;
    let server = bounded("server")?;
    let tool = bounded("tool")?;
    let uri = value["resourceUri"]
        .as_str()
        .filter(|s| s.starts_with("ui://") && s.encode_utf16().count() <= 4096)?;
    let part = |s: &str| {
        s.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '_' | '-') {
                    c
                } else {
                    '_'
                }
            })
            .collect::<String>()
    };
    let server_part = part(server);
    let server_part = if server.starts_with("claude.ai ") {
        server_part
            .split('_')
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("_")
    } else {
        server_part
    };
    let name = item.tool_name.as_ref()?.0.as_str();
    if name != format!("{server}.{tool}") && name != format!("mcp__{server_part}__{}", part(tool)) {
        return None;
    }
    let mut result = json!({"attachmentId":id,"server":server,"tool":tool,"resourceUri":uri});
    if let Some(border) = value["prefersBorder"].as_bool() {
        result["prefersBorder"] = json!(border);
    }
    let mut csp = serde_json::Map::new();
    for key in [
        "connectDomains",
        "resourceDomains",
        "frameDomains",
        "baseUriDomains",
    ] {
        if let Some(domains) = value["csp"][key].as_array() {
            let domains: Vec<_> = domains
                .iter()
                .filter_map(Value::as_str)
                .filter(|s| valid_domain(s))
                .take(32)
                .collect();
            if !domains.is_empty() {
                csp.insert(key.into(), json!(domains));
            }
        }
    }
    if !csp.is_empty() {
        result["csp"] = Value::Object(csp);
    }
    let mut permissions = serde_json::Map::new();
    for key in ["camera", "microphone", "geolocation", "clipboardWrite"] {
        if value["permissions"][key].is_object() {
            permissions.insert(key.into(), json!({}));
        }
    }
    if !permissions.is_empty() {
        result["permissions"] = Value::Object(permissions);
    }
    Some(result)
}
fn valid_domain(domain: &str) -> bool {
    if domain.len() > 256 {
        return false;
    }
    let Some((scheme, host)) = domain.split_once("://") else {
        return false;
    };
    if !matches!(
        scheme.to_ascii_lowercase().as_str(),
        "http" | "https" | "ws" | "wss"
    ) {
        return false;
    }
    let host = host.strip_prefix("*.").unwrap_or(host);
    let host = if let Some((host, port)) = host.rsplit_once(':') {
        if port.is_empty() || port.len() > 5 || !port.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
        host
    } else {
        host
    };
    host.split('.')
        .all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
}
