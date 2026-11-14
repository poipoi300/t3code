//! Bounded metadata extraction from shared/toolOutput.ts, htmlRender.ts and mcpApp.ts.
use crate::Result;
use serde_json::{Value, json};
const MAX_DYNAMIC_VALUE_BYTES: usize = 16_384;
fn encoded_bytes(value: &impl serde::Serialize) -> Result<usize> {
    Ok(serde_json::to_vec(value)?.len())
}
struct ReadBudget {
    bytes: usize,
    nodes: usize,
    exceeded: bool,
}
fn bounded_metadata(record: &serde_json::Map<String, Value>) -> Value {
    let mut data = serde_json::Map::new();
    if let Some(text) = record
        .get("sentMessage")
        .and_then(Value::as_str)
        .filter(|s| !t3_contracts::wire::trim_js(s).is_empty() && s.encode_utf16().count() <= 4000)
    {
        data.insert("sentMessage".into(), text.into());
    }
    for key in ["threadId", "messageId", "taskId", "scheduledTaskId"] {
        if let Some(id) = record.get(key).and_then(bounded_id) {
            data.insert(key.into(), id.into());
        }
    }
    if record.get("status").and_then(Value::as_str) == Some("rolled_back") {
        data.insert("status".into(), "rolled_back".into());
    }
    for key in ["linkedCount", "unlinkedCount"] {
        if let Some(count) = record
            .get(key)
            .and_then(Value::as_i64)
            .filter(|n| n.unsigned_abs() <= 9_007_199_254_740_991)
        {
            data.insert(key.into(), count.into());
        }
    }
    if let Some(id) = record
        .get("thread")
        .and_then(|thread| thread.get("threadId"))
        .and_then(bounded_id)
    {
        data.insert("thread".into(), json!({"threadId":id}));
    }
    if let Some(threads) = record.get("threads").and_then(Value::as_array) {
        let rows: Option<Vec<_>> = (threads.len() <= 100)
            .then(|| {
                threads
                    .iter()
                    .map(|row| {
                        let mut entry = serde_json::Map::new();
                        if let Some(id) = row.get("threadId").and_then(bounded_id) {
                            entry.insert("threadId".into(), id.into());
                        }
                        if row.get("status").and_then(Value::as_str) == Some("rolled_back") {
                            entry.insert("status".into(), "rolled_back".into());
                        }
                        (!entry.is_empty()).then_some(Value::Object(entry))
                    })
                    .collect()
            })
            .flatten();
        data.insert(
            "threads".into(),
            rows.unwrap_or_else(|| vec![Value::Null]).into(),
        );
    }
    if let Some(app) = record.get("t3McpApp").and_then(read_app) {
        data.insert("t3McpApp".into(), app);
    }
    if let Some(html) = record.get("htmlRender").and_then(read_html_render) {
        data.insert("htmlRender".into(), html);
    }
    data.into()
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
        Value::Array(blocks) => {
            if blocks.len() > 32 {
                budget.exceeded = true;
                return (None, false);
            }
            let mut data = None;
            let mut failed = false;
            for block in blocks {
                let text = block.get("text").unwrap_or(&Value::Null);
                let (next, failure) =
                    read_result(text.get("text").unwrap_or(text), budget, depth + 1);
                if data.is_none() {
                    data = next;
                }
                failed |= failure;
                if budget.exceeded {
                    break;
                }
            }
            (data, failed)
        }
        Value::Object(record) => {
            let failed = record.get("isError") == Some(&Value::Bool(true))
                || record.get("is_error") == Some(&Value::Bool(true))
                || record.get("_tag").and_then(Value::as_str) == Some("OrchestratorMcpFailure")
                || record.get("error").is_some_and(|v| !v.is_null());
            if let Some(content) = record
                .get("structuredContent")
                .filter(|v| !v.is_null())
                .or_else(|| record.get("content"))
            {
                let (data, nested) = read_result(content, budget, depth + 1);
                (data, failed || nested)
            } else {
                // Copy only permitted metadata; never clone a transcript-sized output envelope.
                (Some(bounded_metadata(record)), failed)
            }
        }
        _ => (None, false),
    }
}
fn bounded_id(value: &Value) -> Option<&str> {
    value.as_str().filter(|s| {
        !t3_contracts::wire::trim_js(s).is_empty() && s.encode_utf16().take(257).count() <= 256
    })
}

fn read_app(value: &Value) -> Option<Value> {
    let mut app = serde_json::Map::new();
    for key in ["attachmentId", "server", "tool"] {
        app.insert(key.into(), bounded_id(value.get(key)?)?.into());
    }
    let uri = value.get("resourceUri")?.as_str()?;
    if uri.encode_utf16().take(4097).count() > 4096 || !uri.starts_with("ui://") {
        return None;
    }
    app.insert("resourceUri".into(), uri.into());
    if let Some(value) = value.get("prefersBorder").filter(|v| v.is_boolean()) {
        app.insert("prefersBorder".into(), value.clone());
    }
    let mut permissions = serde_json::Map::new();
    for key in ["camera", "microphone", "geolocation", "clipboardWrite"] {
        if value
            .get("permissions")
            .and_then(|p| p.get(key))
            .is_some_and(Value::is_object)
        {
            permissions.insert(key.into(), json!({}));
        }
    }
    if !permissions.is_empty() {
        app.insert("permissions".into(), permissions.into());
    }
    let mut csp = serde_json::Map::new();
    for key in [
        "connectDomains",
        "resourceDomains",
        "frameDomains",
        "baseUriDomains",
    ] {
        if let Some(domains) = value
            .get("csp")
            .and_then(|csp| csp.get(key))
            .and_then(Value::as_array)
        {
            let domains: Vec<_> = domains
                .iter()
                .filter_map(Value::as_str)
                .filter(|domain| valid_domain(domain))
                .take(32)
                .map(|domain| Value::String(domain.into()))
                .collect();
            if !domains.is_empty() {
                csp.insert(key.into(), domains.into());
            }
        }
    }
    if !csp.is_empty() {
        app.insert("csp".into(), csp.into());
    }
    Some(app.into())
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
    host.split('.').all(|label| {
        !label.is_empty()
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    })
}
fn read_html_render(value: &Value) -> Option<Value> {
    let id = value.get("attachmentId")?.as_str()?;
    if id.is_empty() || id.encode_utf16().count() > 256 {
        return None;
    }
    let title = value.get("title")?.as_str()?;
    let title = t3_contracts::wire::trim_js(title);
    let title: String = title
        .chars()
        .scan(0, |units, ch| {
            *units += ch.len_utf16();
            (*units <= 200).then_some(ch)
        })
        .collect();
    let height = value.get("height")?.as_f64()?.clamp(80.0, 2000.0).round();
    let mut output = json!({"attachmentId":id,"title":if title.is_empty(){"HTML"}else{&title},"height":height as u64});
    if let Some(heights) = value
        .get("heights")
        .and_then(Value::as_array)
        .filter(|v| !v.is_empty() && v.len() <= 24)
    {
        let parsed: Option<Vec<_>> = heights
            .iter()
            .map(|entry| {
                let entry = entry.as_array().filter(|e| e.len() == 2)?;
                let width = entry[0].as_u64().filter(|w| (1..=10000).contains(w))?;
                let height = entry[1].as_f64()?.clamp(80.0, 2000.0).round() as u64;
                Some((width, height))
            })
            .collect();
        if let Some(mut parsed) = parsed {
            parsed.sort_by_key(|entry| entry.0);
            output["heights"] = parsed
                .into_iter()
                .map(|(width, height)| json!([width, height]))
                .collect::<Vec<_>>()
                .into();
        }
    }
    Some(output)
}

pub fn compact_dynamic_tool_output(value: &Value) -> Result<Option<Value>> {
    let mut budget = ReadBudget {
        bytes: MAX_DYNAMIC_VALUE_BYTES,
        nodes: 128,
        exceeded: false,
    };
    let (data, failed) = read_result(value, &mut budget, 0);
    let mut output = serde_json::Map::new();
    if failed {
        output.insert("isError".into(), true.into());
    }
    if !budget.exceeded
        && let Some(data) = data
    {
        if let Some(text) = data.get("sentMessage") {
            output.insert("sentMessage".into(), text.clone());
        }
        for key in ["threadId", "messageId", "taskId", "scheduledTaskId"] {
            if let Some(id) = data.get(key).and_then(bounded_id) {
                output.insert(key.into(), id.into());
            }
        }
        if data.get("status").and_then(Value::as_str) == Some("rolled_back") {
            output.insert("status".into(), "rolled_back".into());
        }
        for key in ["linkedCount", "unlinkedCount"] {
            if let Some(count) = data
                .get(key)
                .and_then(Value::as_i64)
                .filter(|v| v.unsigned_abs() <= 9_007_199_254_740_991)
            {
                output.insert(key.into(), count.into());
            }
        }
        if let Some(id) = data
            .get("thread")
            .and_then(|t| t.get("threadId"))
            .and_then(bounded_id)
        {
            output.insert("thread".into(), json!({"threadId": id}));
        }
        if let Some(app) = data.get("t3McpApp").and_then(read_app) {
            output.insert("t3McpApp".into(), app);
        }
        if let Some(html) = data.get("htmlRender").and_then(read_html_render) {
            output.insert("htmlRender".into(), html);
        }
        if let Some(threads) = data.get("threads").and_then(Value::as_array) {
            let rows: Option<Vec<_>> = (threads.len() <= 100)
                .then(|| {
                    threads
                        .iter()
                        .map(|row| {
                            let id = row.get("threadId").and_then(bounded_id);
                            let rolled_back =
                                row.get("status").and_then(Value::as_str) == Some("rolled_back");
                            if id.is_none() && !rolled_back {
                                return None;
                            }
                            let mut row = serde_json::Map::new();
                            if let Some(id) = id {
                                row.insert("threadId".into(), id.into());
                            }
                            if rolled_back {
                                row.insert("status".into(), "rolled_back".into());
                            }
                            Some(Value::Object(row))
                        })
                        .collect()
                })
                .flatten();
            if let Some(rows) = rows {
                output.insert("threads".into(), rows.into());
            } else {
                output.remove("threadId");
                output.remove("status");
            }
        }
    }
    if encoded_bytes(&output)? > 8192 {
        output.remove("threads");
        output.remove("threadId");
        output.remove("status");
    }
    if encoded_bytes(&output)? > 8192
        && let Some(app) = output.get_mut("t3McpApp").and_then(Value::as_object_mut)
    {
        app.remove("csp");
    }
    if encoded_bytes(&output)? > 8192 {
        output.remove("t3McpApp");
    }
    Ok((!output.is_empty()).then_some(output.into()))
}

use crate::regex;
fn space(c: char) -> bool {
    t3_contracts::wire::trim_js(&c.to_string()).is_empty()
}
fn html_tool_name(name: &str) -> bool {
    let label = regex(r"(?i)\s+(?:complete|completed)\s*$").replace(name, "");
    let label = label.trim_matches(space);
    if let Some(parts) = regex(r"(?i)^mcp__(.+?)__(.+)$").captures(label) {
        return matches!(
            parts[1].to_ascii_lowercase().as_str(),
            "t3-code" | "t3_code" | "t3code" | "t3-code-direct" | "t3_code_direct"
        ) && &parts[2] == "html_render";
    }
    let namespace =
        regex(r"(?i)^(?:t3[-_]code[-_]direct|t3-code|t3_code|t3code)(?:[.:/]|\s*·\s*)(.+)$");
    if let Some(parts) = namespace.captures(label) {
        return &parts[1] == "html_render";
    }
    let prefixed = regex(r"(?i)^(?:mcp[-_]{1,2})?t3[-_ ]?code(?:__|[-_.:/ ])(.+)$");
    let candidate = prefixed
        .captures(label)
        .and_then(|parts| parts.get(1))
        .map_or(label, |part| part.as_str());
    candidate == "html_render"
        || (label
            .get(..8)
            .is_some_and(|head| head.eq_ignore_ascii_case("t3-code-"))
            && label.ends_with("_html_render"))
}

pub fn html_render_from_tool_item(item: &Value) -> Result<Option<Value>> {
    let name = item["toolName"].as_str().unwrap_or_default();
    if name != "execute" && !html_tool_name(name) {
        return Ok(None);
    }
    Ok(compact_dynamic_tool_output(&item["output"])?
        .filter(|output| output["isError"] != true)
        .and_then(|output| output.get("htmlRender").cloned()))
}
pub fn claude_mcp_tool_name(server: &str, tool: &str) -> String {
    let part = |value: &str| {
        value
            .encode_utf16()
            .map(|u| {
                let c = char::from_u32(u32::from(u)).unwrap_or('_');
                if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect::<String>()
    };
    let mut server_part = part(server);
    if server.starts_with("claude.ai ") {
        server_part = regex(r"_+")
            .replace_all(&server_part, "_")
            .trim_matches('_')
            .into();
    }
    format!("mcp__{}__{}", server_part, part(tool))
}
pub fn mcp_app_from_tool_item(item: &Value) -> Result<Option<Value>> {
    let Some(output) = compact_dynamic_tool_output(&item["output"])? else {
        return Ok(None);
    };
    let Some(app) = output.get("t3McpApp") else {
        return Ok(None);
    };
    let server = app["server"].as_str().unwrap_or_default();
    let tool = app["tool"].as_str().unwrap_or_default();
    let name = item["toolName"].as_str().unwrap_or_default();
    Ok(
        (name == format!("{server}.{tool}") || name == claude_mcp_tool_name(server, tool))
            .then(|| app.clone()),
    )
}
