use crate::view_logic::{JsTrim, is_js_whitespace, session::work::wire_value};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use t3_contracts::{
    assets::AssetResource, orchestration_v2::OrchestrationV2TurnItem,
    provider_policy::UserInputAttachmentAnswerPayload,
};

fn pretty(value: &Value) -> Option<String> {
    if value.is_null()
        || value.as_str().is_some_and(|s| s.js_trim().is_empty())
        || value.as_object().is_some_and(|o| o.is_empty())
        || value.as_array().is_some_and(|a| a.is_empty())
    {
        return None;
    }
    serde_json::to_string_pretty(value).ok()
}
pub fn is_json_tool_text(text: &str) -> bool {
    if serde_json::from_str::<Value>(text).is_ok() {
        return true;
    }
    let docs = split_documents(text.js_trim());
    docs.len() > 1
        && docs
            .iter()
            .all(|s| serde_json::from_str::<Value>(s).is_ok())
}
fn split_documents(text: &str) -> Vec<&str> {
    let mut result = vec![];
    let mut start = 0;
    let mut cursor = 0;
    while let Some(index) = text[cursor..].find('\n') {
        let index = cursor + index;
        let next = text[index + 1..]
            .find(|c| !is_js_whitespace(c))
            .map_or(text.len(), |n| index + 1 + n);
        if text[index + 1..next].contains('\n') {
            result.push(&text[start..index]);
            start = next;
        }
        cursor = next;
        if cursor >= text.len() {
            break;
        }
    }
    result.push(&text[start..]);
    result
}
fn pretty_text(text: &str) -> String {
    let trimmed = text.js_trim();
    if !trimmed.starts_with(['[', '{']) {
        return text.into();
    }
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        return serde_json::to_string_pretty(&value).unwrap_or_else(|_| text.into());
    }
    let values: Option<Vec<_>> = trimmed
        .split('\n')
        .filter(|s| !s.js_trim().is_empty())
        .map(|s| serde_json::from_str::<Value>(s.js_trim()).ok())
        .collect();
    values.map_or_else(
        || text.into(),
        |values| {
            values
                .iter()
                .filter_map(|v| serde_json::to_string_pretty(v).ok())
                .collect::<Vec<_>>()
                .join("\n\n")
        },
    )
}
fn output_blocks(value: &Value) -> Vec<&Value> {
    value
        .as_array()
        .or_else(|| value.get("content").and_then(Value::as_array))
        .map_or_else(|| vec![value], |a| a.iter().collect())
}
fn image_block(value: &Value) -> bool {
    if value["type"] != "image" {
        return false;
    }
    let source = value.get("source").filter(|s| s.is_object());
    if source.is_some_and(|s| s["type"] != "base64") {
        return false;
    }
    let mime = source.map_or(&value["mimeType"], |s| &s["media_type"]);
    mime.as_str().is_some_and(|s| {
        matches!(
            s.to_ascii_lowercase().as_str(),
            "image/png" | "image/jpeg" | "image/gif" | "image/webp"
        )
    })
}
fn previewed_images(value: &Value) -> Vec<&Value> {
    output_blocks(value)
        .into_iter()
        .filter(|b| image_block(b))
        .take(8)
        .collect()
}
fn text_from_blocks(value: &Value, depth: usize, images: &[&Value]) -> Option<String> {
    if depth > 4 {
        return None;
    }
    if let Some(s) = value.as_str() {
        return Some(s.into());
    }
    if let Some(values) = value.as_array() {
        return values
            .iter()
            .map(|v| text_from_blocks(v, depth + 1, images))
            .collect::<Option<Vec<_>>>()
            .map(|parts| {
                parts
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n")
            });
    }
    let object = value.as_object()?;
    match value["type"].as_str() {
        Some("text") => return value["text"].as_str().map(str::to_string),
        Some("image") => {
            return Some(
                if images.iter().any(|v| std::ptr::eq(*v, value)) {
                    ""
                } else {
                    "[image]"
                }
                .into(),
            );
        }
        Some("resource_link") => return value["uri"].as_str().map(str::to_string),
        Some("resource") if value["resource"].is_object() => {
            return value["resource"]["text"]
                .as_str()
                .or_else(|| value["resource"]["uri"].as_str())
                .map(str::to_string);
        }
        _ => {}
    }
    let keys: Vec<_> = object
        .keys()
        .filter(|s| !matches!(s.as_str(), "isError" | "is_error"))
        .collect();
    if keys.len() == 1 && keys[0] == "content" {
        return text_from_blocks(&value["content"], depth + 1, images);
    }
    if keys.len() == 2 && object.contains_key("content") && object.contains_key("structuredContent")
    {
        let text = text_from_blocks(&value["content"], depth + 1, images);
        let blocks: Vec<_> = value["content"].as_array().map_or_else(Vec::new, |blocks| {
            blocks.iter().filter(|b| b["type"] == "image").collect()
        });
        if let Some(structured) = pretty(&value["structuredContent"])
            && (!blocks.is_empty() || text.as_ref().is_none_or(|s| s.js_trim().is_empty()))
        {
            let markers = blocks
                .into_iter()
                .map(|b| text_from_blocks(b, depth + 2, images))
                .collect::<Option<Vec<_>>>()?
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            return Some(if markers.is_empty() {
                structured
            } else {
                format!("{structured}\n{markers}")
            });
        }
        return text.filter(|s| !s.js_trim().is_empty());
    }
    None
}
fn format_tool_value(value: &Value, has_image_preview: bool, preview: bool) -> Option<String> {
    if value.is_null() || (has_image_preview && value["type"] == "image") {
        return None;
    }
    let images = if preview {
        previewed_images(value)
    } else {
        vec![]
    };
    if let Some(text) = text_from_blocks(value, 0, &images) {
        return (!text.js_trim().is_empty()).then(|| pretty_text(&text));
    }
    pretty(value)
}
#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallLines {
    pub command: Option<String>,
    pub args: Option<Vec<(String, String)>>,
    pub args_text: Option<String>,
}
pub fn tool_call_lines(command: Option<&str>, args: &Value) -> ToolCallLines {
    if let Some(command) = command {
        return ToolCallLines {
            command: (!command.js_trim().is_empty()).then(|| command.js_trim().into()),
            args: None,
            args_text: None,
        };
    }
    if let Some(object) = args.as_object().filter(|_| !is_summarized(args)) {
        let entries: Vec<_> = object
            .iter()
            .map(|(key, value)| {
                (
                    key.clone(),
                    value
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .map_or_else(|| value.to_string(), str::to_string),
                )
            })
            .collect();
        return ToolCallLines {
            command: None,
            args: (!entries.is_empty()).then_some(entries),
            args_text: None,
        };
    }
    ToolCallLines {
        command: None,
        args: None,
        args_text: format_tool_value(args, false, true),
    }
}
fn is_summarized(value: &Value) -> bool {
    value["truncated"] == true && value["summary"].is_string()
}
pub fn turn_item_detail_revision(item: &OrchestrationV2TurnItem) -> String {
    let value = wire_value(item);
    if matches!(
        value["status"].as_str(),
        Some("idle" | "pending" | "running" | "waiting")
    ) {
        "live".into()
    } else {
        value["updatedAt"].as_str().unwrap_or_default().into()
    }
}
pub fn turn_item_needs_detail_fetch(item: &OrchestrationV2TurnItem) -> bool {
    let v = wire_value(item);
    match v["type"].as_str() {
        Some("command_execution") => v["outputOmitted"] == true,
        Some("dynamic_tool") => v["outputOmitted"] == true || is_summarized(&v["input"]),
        _ => false,
    }
}
pub fn turn_item_output_text(item: &OrchestrationV2TurnItem) -> Option<String> {
    let v = wire_value(item);
    match v["type"].as_str()? {
        "command_execution" => {
            let output = v["output"].as_str().filter(|s| !s.js_trim().is_empty())?;
            if output.js_trim_start().starts_with("{\"stdout\"")
                && let Ok(parsed) = serde_json::from_str::<Value>(output.js_trim())
                && parsed["stdout"].is_string()
                && parsed["stderr"].is_string()
                && parsed["interrupted"].is_boolean()
            {
                let output = ["stdout", "stderr"]
                    .iter()
                    .filter_map(|key| parsed[*key].as_str().filter(|s| !s.js_trim().is_empty()))
                    .collect::<Vec<_>>()
                    .join("\n");
                return (!output.is_empty()).then_some(output);
            }
            Some(output.into())
        }
        "dynamic_tool" => {
            if v["outputOmitted"] == true {
                return None;
            }
            let output = &v["output"];
            if output.is_object()
                && output.get("t3McpApp").is_some()
                && let Some(result) = output.get("result")
            {
                if !result.is_object() {
                    return format_tool_value(result, false, false);
                }
                let structured = result.get("structuredContent").filter(|s| {
                    !s.is_null()
                        && !s.as_object().is_some_and(|o| o.is_empty())
                        && !s.as_array().is_some_and(|a| a.is_empty())
                });
                if let Some(structured) = structured {
                    if !previewed_images(&result["content"]).is_empty() {
                        return format_tool_value(
                            &serde_json::json!({"structuredContent":structured,"content":result["content"]}),
                            false,
                            false,
                        );
                    }
                    return format_tool_value(structured, false, false);
                }
                return format_tool_value(&result["content"], false, false);
            }
            format_tool_value(output, v.get("viewedImagePath").is_some(), true)
        }
        "file_search" | "web_search" => {
            let results = v["results"].as_array().filter(|r| !r.is_empty())?;
            let web = v["type"] == "web_search";
            Some(
                results
                    .iter()
                    .map(|r| {
                        if web {
                            let title = r["title"].as_str().unwrap_or_default();
                            [
                                Some(if title.js_trim().is_empty() {
                                    r["url"].as_str().unwrap_or_default()
                                } else {
                                    title.js_trim()
                                }),
                                (!title.is_empty()).then(|| r["url"].as_str()).flatten(),
                                r["snippet"].as_str().map(str::js_trim),
                            ]
                            .into_iter()
                            .flatten()
                            .filter(|s| !s.is_empty())
                            .collect::<Vec<_>>()
                            .join("\n")
                        } else {
                            let mut text = r["fileName"].as_str().unwrap_or_default().to_string();
                            if let Some(line) = r.get("line") {
                                text.push_str(&format!(":{line}"));
                            }
                            if let Some(preview) = r["preview"]
                                .as_str()
                                .map(str::js_trim)
                                .filter(|s| !s.is_empty())
                            {
                                text.push('\n');
                                text.push_str(preview);
                            }
                            text
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(if web { "\n\n" } else { "\n" }),
            )
        }
        _ => None,
    }
}
pub fn turn_item_output_images(item: &OrchestrationV2TurnItem) -> Vec<AssetResource> {
    let v = wire_value(item);
    if v["type"] != "dynamic_tool" || v["outputOmitted"] == true {
        return vec![];
    }
    previewed_images(&v["output"]).iter().enumerate().filter_map(|(index,_)|serde_json::from_value(serde_json::json!({"_tag":"tool-output-image","threadId":v["threadId"],"itemId":v["id"],"index":index})).ok()).collect()
}
pub fn turn_item_has_detail(item: &OrchestrationV2TurnItem) -> bool {
    let v = wire_value(item);
    let text = |key: &str| v[key].as_str().is_some_and(|s| !s.js_trim().is_empty());
    let array = |key: &str| v[key].as_array().is_some_and(|a| !a.is_empty());
    match v["type"].as_str().unwrap_or_default() {
        "reasoning" => text("text"),
        "command_execution" => {
            text("input")
                || text("output")
                || v["outputOmitted"] == true
                || v.get("exitCode").is_some_and(|n| n != 0)
        }
        "file_change" | "checkpoint" | "fork" | "handoff" => true,
        "file_search" => array("results") || text("pattern"),
        "web_search" => array("results") || array("patterns"),
        "dynamic_tool" => {
            let lines = tool_call_lines(None, &v["input"]);
            v["outputOmitted"] == true || lines.args.is_some() || lines.args_text.is_some()
        }
        "approval_request" => text("prompt"),
        "user_input_request" => array("questions"),
        "notification" => text("detail"),
        "system_notice" => text("message"),
        "error" => v["failure"]["message"]
            .as_str()
            .is_some_and(|s| !s.js_trim().is_empty()),
        "proposed_plan" => text("markdown"),
        "todo_list" => array("steps"),
        "subagent" => !v["childThreadId"].is_null(),
        _ => false,
    }
}
pub fn tool_item_for_display(item: &OrchestrationV2TurnItem) -> Value {
    let mut v = wire_value(item);
    let kind = v["type"].as_str().unwrap_or_default().to_string();
    let failed = v["status"] == "failed";
    if let Some(o) = v.as_object_mut() {
        match kind.as_str() {
            "command_execution" | "dynamic_tool" => {
                o.remove("output");
            }
            "file_change" => {
                o.remove("oldStr");
                o.remove("newStr");
                if !failed
                    || !o
                        .get("diffStr")
                        .and_then(Value::as_str)
                        .is_some_and(|s| !s.js_trim().is_empty())
                {
                    o.remove("diffStr");
                }
            }
            _ => {}
        }
    }
    v
}
pub fn question_answer_text(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        return crate::view_logic::context_references::provider_text_for_display(text);
    }
    if let Some(values) = value.as_array() {
        return values
            .iter()
            .map(question_answer_text)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(", ");
    }
    value
        .get("answers")
        .map_or_else(String::new, question_answer_text)
}
pub(crate) fn collapse_whitespace(text: &str) -> String {
    text.split(is_js_whitespace)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}
pub fn question_text_preview(answer: &UserInputAttachmentAnswerPayload) -> String {
    answer
        .question_text_by_id
        .as_ref()
        .and_then(Option::as_ref)
        .map_or_else(String::new, |map| {
            map.values()
                .map(|s| collapse_whitespace(s))
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" · ")
        })
}
pub fn question_answer_preview(answer: &UserInputAttachmentAnswerPayload) -> String {
    let answers: Vec<_> = answer
        .answers
        .values()
        .map(question_answer_text)
        .filter(|s| !s.is_empty())
        .collect();
    if !answers.is_empty() {
        return collapse_whitespace(&answers.join(" · "));
    }
    let names: Vec<_> = answer
        .attachments_by_question_id
        .values()
        .flatten()
        .map(|a| {
            wire_value(a)["name"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    if !names.is_empty() {
        return collapse_whitespace(&names.join(", "));
    }
    question_text_preview(answer)
}
pub fn has_question_answer(answer: &UserInputAttachmentAnswerPayload) -> bool {
    answer
        .answers
        .values()
        .any(|v| !question_answer_text(v).is_empty())
        || answer
            .attachments_by_question_id
            .values()
            .any(|v| !v.is_empty())
}
