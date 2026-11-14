use super::tool_output_image_bytes::{read_tool_output_image, tool_output_image_blocks};
use crate::error::Result;
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    io::{self, Write},
    sync::{Arc, Mutex, Weak},
};
use t3_contracts::{orchestration_v2::*, orchestration_v2_id_table::IdOrIndex};

pub const MAX_WIRE_CHECKPOINT_FILES: usize = 200;
pub const MAX_DETAIL_STRING_BYTES: usize = 32_768;
pub const MAX_DYNAMIC_VALUE_BYTES: usize = 16_384;
const SEND_TO_USER_MAX_LENGTH: usize = 4_000;
pub const MAX_ON_DEMAND_BYTES: usize = 256 * 1024;
pub const TRUNCATION_MARKER: &str = "\n… output truncated for transport";

struct ByteCounter(usize);
impl Write for ByteCounter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
pub fn encoded_bytes(value: &impl Serialize) -> Result<usize> {
    let mut counter = ByteCounter(0);
    serde_json::to_writer(&mut counter, value)?;
    Ok(counter.0)
}

pub fn truncate_detail(text: &mut String, max_bytes: usize) {
    if text.len() <= max_bytes {
        return;
    }
    let budget = max_bytes.saturating_sub(TRUNCATION_MARKER.len());
    let mut boundary = budget;
    while !text.is_char_boundary(boundary) {
        boundary -= 1;
    }
    text.truncate(boundary);
    if boundary == budget && text.ends_with('\u{fffd}') {
        text.pop();
    }
    text.push_str(TRUNCATION_MARKER);
}
fn js_whitespace(ch: char) -> bool {
    matches!(ch,'\u{0009}'..='\u{000d}'|' '| '\u{00a0}'|'\u{1680}'|'\u{2000}'..='\u{200a}'|'\u{2028}'|'\u{2029}'|'\u{202f}'|'\u{205f}'|'\u{3000}'|'\u{feff}')
}
fn truncate_optional(text: &mut Option<Option<String>>, max_bytes: usize) {
    if let Some(Some(text)) = text {
        truncate_detail(text, max_bytes);
    }
}
pub fn cap_checkpoint_files(
    files: &mut Vec<OrchestrationV2CheckpointFileSummary>,
    total: &mut Option<OrchestrationV2CheckpointFileTotals>,
) {
    if files.len() <= MAX_WIRE_CHECKPOINT_FILES || total.is_some() {
        return;
    }
    *total = Some(OrchestrationV2CheckpointFileTotals {
        count: files.len() as u64,
        additions: files.iter().map(|f| f.additions).sum(),
        deletions: files.iter().map(|f| f.deletions).sum(),
    });
    files.truncate(MAX_WIRE_CHECKPOINT_FILES);
}
pub fn cap_projection_checkpoint_files_for_wire(projection: &mut OrchestrationV2ThreadProjection) {
    for checkpoint in &mut projection.checkpoints {
        cap_checkpoint_files(&mut checkpoint.files, &mut checkpoint.files_total);
    }
    for item in projection.turn_items.iter_mut().chain(
        projection
            .visible_turn_items
            .iter_mut()
            .map(|r| &mut r.item),
    ) {
        if let OrchestrationV2TurnItem::Checkpoint(item) = item {
            cap_checkpoint_files(&mut item.files, &mut item.files_total);
        }
    }
}

pub fn bound_file_change_for_storage(item: &mut OrchestrationV2TurnItem) {
    if let OrchestrationV2TurnItem::FileChange(item) = item {
        truncate_optional(&mut item.diff_str, MAX_DETAIL_STRING_BYTES);
        truncate_optional(&mut item.old_str, MAX_DETAIL_STRING_BYTES);
        truncate_optional(&mut item.new_str, MAX_DETAIL_STRING_BYTES);
    }
}

fn summarize_dynamic_value(value: &mut Value) -> Result<()> {
    let serialized = if let Value::String(text) = value
        && text.len() > MAX_DYNAMIC_VALUE_BYTES
    {
        std::mem::take(text)
    } else {
        let encoded = serde_json::to_string(value)?;
        if encoded.len() <= MAX_DYNAMIC_VALUE_BYTES {
            return Ok(());
        }
        match value {
            Value::String(text) => std::mem::take(text),
            _ => encoded,
        }
    };
    let line = serialized
        .trim_start_matches(js_whitespace)
        .split('\n')
        .next()
        .unwrap_or("");
    let mut first = String::new();
    let mut pending_space = false;
    let mut units = 0;
    for ch in line.chars() {
        if js_whitespace(ch) {
            pending_space = true;
            continue;
        }
        if pending_space {
            first.push(' ');
            units += 1;
        }
        first.push(ch);
        units += ch.len_utf16();
        pending_space = false;
        if units > 160 {
            break;
        }
    }
    if first.is_empty() {
        first = "Large tool output".into();
    }
    if units > 160 {
        let mut end = 0;
        let mut units = 0;
        for (index, ch) in first.char_indices() {
            if units + ch.len_utf16() > 159 {
                break;
            }
            units += ch.len_utf16();
            end = index + ch.len_utf8();
        }
        first.truncate(end);
        first = first.trim_end_matches(js_whitespace).to_owned();
        first.push('…');
    }
    *value = json!({"summary":first,"truncated":true});
    Ok(())
}

struct ReadBudget {
    bytes: usize,
    nodes: usize,
    exceeded: bool,
}
fn bounded_metadata(record: &serde_json::Map<String, Value>) -> Value {
    let mut data = serde_json::Map::new();
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
    if let Some(message) = record.get("sentMessage").and_then(bounded_sent_message) {
        data.insert("sentMessage".into(), message.into());
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
fn bounded_sent_message(value: &Value) -> Option<&str> {
    value.as_str().filter(|message| {
        message
            .encode_utf16()
            .take(SEND_TO_USER_MAX_LENGTH + 1)
            .count()
            <= SEND_TO_USER_MAX_LENGTH
            && !t3_contracts::wire::trim_js(message).is_empty()
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

pub(crate) fn compact_dynamic_tool_output(value: &Value) -> Result<Option<Value>> {
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
        if let Some(message) = data.get("sentMessage").and_then(bounded_sent_message) {
            output.insert("sentMessage".into(), message.into());
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

pub fn tool_output_indicates_failure(text: &str) -> bool {
    let text = text.to_lowercase();
    [
        "file not found",
        "no files found",
        "enoent",
        "no such file",
        "commandnotfoundexception",
        "command not found",
        "is not recognized as the name of a cmdlet",
        "a parameter cannot be found that matches parameter name",
    ]
    .iter()
    .any(|needle| text.contains(needle))
        || (text.contains("cannot find path") && text.contains("because it does not exist"))
        || (text.contains("is not recognized") && text.contains("the term '"))
        || text.match_indices("exit code").any(|(start, _)| {
            let suffix = &text[start + 9..];
            let spaced = suffix.chars().next().is_some_and(js_whitespace);
            let rest = suffix.trim_start_matches(js_whitespace);
            let colon = rest.starts_with(':');
            let rest = if let Some(rest) = rest.strip_prefix(':') {
                rest.trim_start_matches(js_whitespace)
            } else if spaced {
                rest
            } else {
                return false;
            };
            if !rest
                .as_bytes()
                .first()
                .is_some_and(|b| matches!(b, b'1'..=b'9'))
            {
                return false;
            }
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            let with_exit = spaced
                && !colon
                && (text[..start].ends_with("exit with ")
                    || text[..start].ends_with("exited with "));
            with_exit
                || rest
                    .as_bytes()
                    .get(digits)
                    .is_none_or(|b| !b.is_ascii_alphanumeric() && *b != b'_')
        })
}

pub fn project_turn_item_for_wire(item: &mut OrchestrationV2TurnItem) -> Result<()> {
    match item {
        OrchestrationV2TurnItem::Checkpoint(item) => {
            cap_checkpoint_files(&mut item.files, &mut item.files_total)
        }
        OrchestrationV2TurnItem::Handoff(item) => item.summary = None,
        OrchestrationV2TurnItem::CommandExecution(item) => {
            let failed = match item.exit_code {
                Some(Some(code)) => code != 0,
                Some(None) => true,
                None => {
                    item.output_indicates_failure == Some(Some(true))
                        || item
                            .output
                            .as_ref()
                            .and_then(Option::as_ref)
                            .is_some_and(|text| {
                                let prefix: String = text
                                    .chars()
                                    .scan(0, |units, ch| {
                                        *units += ch.len_utf16();
                                        (*units <= MAX_DETAIL_STRING_BYTES).then_some(ch)
                                    })
                                    .collect();
                                tool_output_indicates_failure(&prefix)
                            })
                }
            };
            if failed {
                item.output_indicates_failure = Some(Some(true));
            }
            if item
                .output
                .as_ref()
                .and_then(Option::as_ref)
                .is_some_and(|s| !t3_contracts::wire::trim_js(s).is_empty())
            {
                item.output_omitted = Some(Some(true));
            }
            item.output = None;
        }
        OrchestrationV2TurnItem::FileChange(item) => {
            if item.status == OrchestrationV2TurnItemStatus::Failed
                && item
                    .diff_str
                    .as_ref()
                    .and_then(Option::as_ref)
                    .is_some_and(|s| !t3_contracts::wire::trim_js(s).is_empty())
            {
                truncate_optional(&mut item.diff_str, MAX_DETAIL_STRING_BYTES);
            } else {
                item.diff_str = None;
            }
            item.old_str = None;
            item.new_str = None;
        }
        OrchestrationV2TurnItem::Subagent(item) => {
            truncate_detail(&mut item.prompt, MAX_DETAIL_STRING_BYTES);
            truncate_optional(&mut item.progress, MAX_DETAIL_STRING_BYTES);
            if let Some(result) = &mut item.result {
                truncate_detail(result, MAX_DETAIL_STRING_BYTES);
            }
        }
        OrchestrationV2TurnItem::DynamicTool(item) => {
            summarize_dynamic_value(&mut item.input)?;
            if let Some(Some(raw)) = item.output.take() {
                let present = match &raw {
                    Value::Null => false,
                    Value::String(s) => !t3_contracts::wire::trim_js(s).is_empty(),
                    Value::Array(v) => !v.is_empty(),
                    Value::Object(v) => !v.is_empty(),
                    _ => true,
                };
                item.output = compact_dynamic_tool_output(&raw)?.map(Some);
                if present {
                    item.output_omitted = Some(Some(true));
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn bound_dynamic_value(value: &mut Value) -> Result<()> {
    if let Value::String(text) = value {
        truncate_detail(text, MAX_ON_DEMAND_BYTES);
    } else if encoded_bytes(value)? > MAX_ON_DEMAND_BYTES {
        let mut text = serde_json::to_string(value)?;
        truncate_detail(&mut text, MAX_ON_DEMAND_BYTES);
        *value = text.into();
    }
    Ok(())
}
fn omit_image_data(output: &mut Value) {
    let omit = |block: &mut Value| {
        if let Some((mime, Some(_))) = read_tool_output_image(block) {
            *block = json!({"type":"image","mimeType":mime.to_ascii_lowercase()});
        }
    };
    if let Some(blocks) = output.as_array_mut() {
        blocks.iter_mut().for_each(omit);
    } else if let Some(blocks) = output.get_mut("content").and_then(Value::as_array_mut) {
        blocks.iter_mut().for_each(omit);
    } else {
        omit(output);
    }
}
fn bound_tool_result(output: &mut Value) -> Result<()> {
    omit_image_data(output);
    let images: Vec<_> = tool_output_image_blocks(output)
        .into_iter()
        .filter_map(|block| {
            read_tool_output_image(block)
                .map(|(mime, _)| json!({"type":"image","mimeType":mime.to_ascii_lowercase()}))
        })
        .collect();
    let was_string = output.is_string();
    bound_dynamic_value(output)?;
    if output.is_string() && !was_string && !images.is_empty() {
        let mut blocks = vec![json!({"type":"text","text":std::mem::take(output)})];
        blocks.extend(images);
        *output = blocks.into();
    }
    Ok(())
}
pub fn project_turn_item_for_detail(item: &mut OrchestrationV2TurnItem) -> Result<()> {
    match item {
        OrchestrationV2TurnItem::CommandExecution(item) => {
            truncate_detail(&mut item.input, MAX_ON_DEMAND_BYTES);
            truncate_optional(&mut item.output, MAX_ON_DEMAND_BYTES);
        }
        OrchestrationV2TurnItem::DynamicTool(item) => {
            bound_dynamic_value(&mut item.input)?;
            if let Some(Some(output)) = &mut item.output {
                if let Some(app) = output.get("t3McpApp").and_then(read_app)
                    && let Some(result) = output.get_mut("result")
                {
                    if encoded_bytes(result)? > MAX_ON_DEMAND_BYTES {
                        bound_tool_result(result)?;
                    }
                    *output = json!({"t3McpApp":app,"result":std::mem::take(result)});
                } else {
                    bound_tool_result(output)?;
                }
            }
        }
        OrchestrationV2TurnItem::Subagent(item) => {
            truncate_detail(&mut item.prompt, MAX_ON_DEMAND_BYTES);
            truncate_optional(&mut item.progress, MAX_ON_DEMAND_BYTES);
            if let Some(result) = &mut item.result {
                truncate_detail(result, MAX_ON_DEMAND_BYTES);
            }
        }
        OrchestrationV2TurnItem::FileChange(item) => {
            let has_diff = item
                .diff_str
                .as_ref()
                .and_then(Option::as_ref)
                .is_some_and(|s| !t3_contracts::wire::trim_js(s).is_empty());
            truncate_optional(&mut item.diff_str, MAX_ON_DEMAND_BYTES);
            if has_diff {
                item.old_str = None;
                item.new_str = None;
            } else {
                truncate_optional(&mut item.old_str, MAX_ON_DEMAND_BYTES);
                truncate_optional(&mut item.new_str, MAX_ON_DEMAND_BYTES);
            }
        }
        OrchestrationV2TurnItem::Handoff(item) => item.summary = None,
        _ => {}
    }
    Ok(())
}
pub fn project_context_handoff_for_wire(handoff: &mut OrchestrationV2ContextHandoff) {
    handoff.history = None;
    handoff.delivery = None;
    handoff.summary_text.clear();
}
pub fn project_thread_projection_for_wire(
    projection: &mut OrchestrationV2ThreadProjection,
) -> Result<()> {
    for checkpoint in &mut projection.checkpoints {
        cap_checkpoint_files(&mut checkpoint.files, &mut checkpoint.files_total);
    }
    for handoff in &mut projection.context_handoffs {
        project_context_handoff_for_wire(handoff);
    }
    let mut projected = std::collections::BTreeMap::<_, OrchestrationV2TurnItem>::new();
    for item in projection.turn_items.iter_mut().chain(
        projection
            .visible_turn_items
            .iter_mut()
            .map(|row| &mut row.item),
    ) {
        use super::thread_history_paging::{id_text, item_id, item_thread_id};
        let key = (id_text(item_thread_id(item)), id_text(item_id(item)));
        if let Some(previous) = projected.get(&key) {
            *item = previous.clone();
        } else {
            project_turn_item_for_wire(item)?;
            projected.insert(key, item.clone());
        }
    }
    Ok(())
}
pub fn project_domain_event_for_wire(event: &mut OrchestrationV2DomainEvent) -> Result<()> {
    match event {
        OrchestrationV2DomainEvent::TurnItemUpdated(event) => {
            project_turn_item_for_wire(&mut event.payload)?
        }
        OrchestrationV2DomainEvent::ContextHandoffUpdated(event) => {
            project_context_handoff_for_wire(&mut event.payload)
        }
        OrchestrationV2DomainEvent::CheckpointCaptured(event) => {
            cap_checkpoint_files(&mut event.payload.files, &mut event.payload.files_total)
        }
        _ => {}
    }
    Ok(())
}

type CachedEvent = (
    Weak<OrchestrationV2StoredEvent>,
    Weak<OrchestrationV2StoredEvent>,
);
#[derive(Default)]
struct CachedEvents {
    entries: std::collections::HashMap<usize, CachedEvent>,
    next_sweep: usize,
}
#[derive(Default)]
pub struct WireEventCache(Mutex<CachedEvents>);
impl WireEventCache {
    pub fn project(
        &self,
        stored: &Arc<OrchestrationV2StoredEvent>,
    ) -> Result<Arc<OrchestrationV2StoredEvent>> {
        let mut cache = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let key = Arc::as_ptr(stored) as usize;
        if let Some(projected) = cache
            .entries
            .get(&key)
            .and_then(|(_, projected)| projected.upgrade())
        {
            return Ok(projected);
        }
        if cache.entries.len() >= cache.next_sweep {
            cache.entries.retain(|_, (source, projected)| {
                source.strong_count() > 0 && projected.strong_count() > 0
            });
            cache.next_sweep = cache.entries.len().saturating_mul(2).max(4096);
        }
        let mut projected = (**stored).clone();
        project_domain_event_for_wire(&mut projected.event)?;
        let projected = if projected == **stored {
            stored.clone()
        } else {
            Arc::new(projected)
        };
        cache
            .entries
            .insert(key, (Arc::downgrade(stored), Arc::downgrade(&projected)));
        Ok(projected)
    }
}

pub fn thread_stream_event_item(
    stored: OrchestrationV2StoredEvent,
) -> OrchestrationV2ThreadStreamItem {
    OrchestrationV2ThreadStreamItem::Event(Box::new(ThreadStreamItemMember3 {
        kind: ThreadStreamItemMember3Kind::Event,
        sequence: stored.sequence,
        event: stored.event,
        ids: None,
    }))
}

type SubagentBodies = (String, Option<String>);
#[derive(Default)]
pub struct LiveBodyDeduper {
    messages: VecDeque<(IdOrIndex<t3_contracts::base_schemas::MessageId>, String)>,
    checkpoints: VecDeque<(
        IdOrIndex<t3_contracts::base_schemas::CheckpointId>,
        Vec<OrchestrationV2CheckpointFileSummary>,
    )>,
    subagents: VecDeque<(
        IdOrIndex<t3_contracts::base_schemas::NodeId>,
        SubagentBodies,
    )>,
}
fn remember<K: PartialEq, V>(memory: &mut VecDeque<(K, V)>, key: K, value: V) {
    memory.retain(|(id, _)| *id != key);
    memory.push_back((key, value));
    if memory.len() > 32 {
        memory.pop_front();
    }
}
fn dedupe_subagent(
    prompt: &mut String,
    result: &mut Option<String>,
    prompt_omitted: &mut Option<LiteralTrue>,
    result_omitted: &mut Option<LiteralTrue>,
    previous: Option<&SubagentBodies>,
) {
    if let Some((old_prompt, old_result)) = previous {
        if !prompt.is_empty() && prompt == old_prompt {
            prompt.clear();
            *prompt_omitted = Some(LiteralTrue);
        }
        if result.is_some() && result == old_result {
            *result = None;
            *result_omitted = Some(LiteralTrue);
        }
    }
}
impl LiveBodyDeduper {
    pub fn dedupe_item(&self, item: &mut OrchestrationV2TurnItem) {
        macro_rules! dedupe_message {
            ($item:expr) => {
                if !$item.text.is_empty()
                    && self
                        .messages
                        .iter()
                        .any(|(id, text)| *id == $item.message_id && *text == $item.text)
                {
                    $item.text.clear();
                    $item.text_omitted = Some(LiteralTrue);
                }
            };
        }
        match item {
            OrchestrationV2TurnItem::UserMessage(item) => dedupe_message!(item),
            OrchestrationV2TurnItem::AssistantMessage(item) => dedupe_message!(item),
            OrchestrationV2TurnItem::Checkpoint(item) => {
                if !item.files.is_empty()
                    && self
                        .checkpoints
                        .iter()
                        .any(|(id, files)| *id == item.checkpoint_id && *files == item.files)
                {
                    item.files.clear();
                    item.files_omitted = Some(LiteralTrue);
                }
            }
            OrchestrationV2TurnItem::Subagent(item) => dedupe_subagent(
                &mut item.prompt,
                &mut item.result,
                &mut item.prompt_omitted,
                &mut item.result_omitted,
                self.subagents
                    .iter()
                    .find(|(id, _)| *id == item.subagent_id)
                    .map(|(_, body)| body),
            ),
            _ => {}
        }
    }
    pub fn observe_snapshot(&mut self, projection: &OrchestrationV2ThreadProjection) {
        for message in &projection.messages {
            remember(&mut self.messages, message.id.clone(), message.text.clone());
        }
        for checkpoint in &projection.checkpoints {
            remember(
                &mut self.checkpoints,
                checkpoint.id.clone(),
                checkpoint.files.clone(),
            );
        }
        for subagent in &projection.subagents {
            remember(
                &mut self.subagents,
                subagent.id.clone(),
                (subagent.prompt.clone(), subagent.result.clone()),
            );
        }
    }
    pub fn apply(&mut self, item: &mut OrchestrationV2ThreadStreamItem) {
        match item {
            OrchestrationV2ThreadStreamItem::Snapshot(item) => {
                self.observe_snapshot(&item.projection)
            }
            OrchestrationV2ThreadStreamItem::Event(item) => match &mut item.event {
                OrchestrationV2DomainEvent::MessageUpdated(event) => remember(
                    &mut self.messages,
                    event.payload.id.clone(),
                    event.payload.text.clone(),
                ),
                OrchestrationV2DomainEvent::CheckpointCaptured(event) => remember(
                    &mut self.checkpoints,
                    event.payload.id.clone(),
                    event.payload.files.clone(),
                ),
                OrchestrationV2DomainEvent::SubagentUpdated(event) => {
                    let body = (event.payload.prompt.clone(), event.payload.result.clone());
                    dedupe_subagent(
                        &mut event.payload.prompt,
                        &mut event.payload.result,
                        &mut event.payload.prompt_omitted,
                        &mut event.payload.result_omitted,
                        self.subagents
                            .iter()
                            .find(|(id, _)| *id == event.payload.id)
                            .map(|(_, body)| body),
                    );
                    remember(&mut self.subagents, event.payload.id.clone(), body);
                }
                OrchestrationV2DomainEvent::TurnItemUpdated(event) => {
                    self.dedupe_item(&mut event.payload)
                }
                _ => {}
            },
            _ => {}
        }
    }
}
