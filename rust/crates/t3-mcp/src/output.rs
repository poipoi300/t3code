use crate::{McpError, Result, decode::cut_utf16};
use async_trait::async_trait;
use serde_json::{Value, json};

pub const MAX_SNAPSHOT_TEXT_BYTES: usize = 20_000;

#[async_trait]
pub trait ArtifactStore: Send + Sync {
    async fn save_screenshot(&self, page_url: &str, base64_png: &str) -> Result<String>;
}

fn text(value: &Value) -> Value {
    json!({"type":"text", "text":value.to_string()})
}

pub fn failure(name: &str, error: McpError) -> Value {
    if let McpError::NotAvailable { tool, method } = &error {
        return json!({"isError":true,"structuredContent":{"available":false,"tool":tool,"missingServiceMethod":method,"error":error.encoded()},"content":[text(&error.encoded())]});
    }
    if matches!(error, McpError::Internal) {
        return json!({"isError":true,"content":[{"type":"text","text":"Tool execution failed due to an internal server error."}]});
    }
    if matches!(
        name,
        "preview_snapshot" | "device_screenshot" | "html_preview"
    ) {
        let operation = if name == "html_preview" {
            "preview"
        } else if name == "device_screenshot" {
            "screenshot"
        } else {
            "snapshot"
        };
        let tag = match &error {
            McpError::Domain { tag, .. } => tag.clone(),
            McpError::Failure { .. } => "OrchestratorMcpFailure".into(),
            McpError::InvalidInput(_) => "AiError".into(),
            McpError::Internal => format!("{name}Error"),
            McpError::NotAvailable { .. } => "OrchestratorMcpFailure".into(),
        };
        let trusted = |kind: &str| crate::catalog::catalog().error_tags[kind].contains(&tag);
        let message = match (name, &error) {
            (_, McpError::InvalidInput(message)) => Some(message.clone()),
            ("preview_snapshot", McpError::Domain { message, .. })
                if trusted("preview") || tag == "PreviewScreenshotSaveError" =>
            {
                Some(message.clone())
            }
            ("preview_snapshot", _) => None,
            (_, McpError::Failure { message, .. }) => Some(message.clone()),
            (
                "device_screenshot",
                McpError::Domain {
                    message, fields, ..
                },
            ) if trusted("device") => Some(
                if tag == "DeviceOperationError" && fields["reason"] == "hub_rejected" {
                    format!(
                        "{message} Call device_list to check the device state, and device_open if it needs to be reopened."
                    )
                } else {
                    message.clone()
                },
            ),
            ("html_preview", _) => Some("HTML preview failed.".into()),
            _ => Some("Device screenshot failed.".into()),
        };
        let mut detail = json!({"_tag":tag, "operation":operation, "failureCount":1});
        if let Some(message) = &message {
            detail["message"] = json!(message);
        }
        let message = if name == "preview_snapshot" {
            format!(
                "Preview snapshot failed: {}",
                message.unwrap_or_else(|| format!("{tag}."))
            )
        } else {
            message.unwrap_or_else(|| format!("{name} failed."))
        };
        return json!({"isError":true, "structuredContent":{"error":detail}, "content":[{"type":"text", "text":message}]});
    }
    match error {
        McpError::InvalidInput(message) => {
            json!({"isError":true,"content":[{"type":"text","text":message}]})
        }
        McpError::Internal => {
            json!({"isError":true,"content":[{"type":"text","text":"Tool execution failed due to an internal server error."}]})
        }
        error
            if crate::catalog::catalog()
                .definitions
                .get(name)
                .is_some_and(|definition| definition.failure_mode != "return") =>
        {
            json!({"isError":true,"content":[{"type":"text","text":error.to_string()}]})
        }
        error => json!({"isError":true,"content":[text(&error.encoded())]}),
    }
}

pub async fn success(
    name: &str,
    input: &Value,
    mut value: Value,
    artifacts: Option<&dyn ArtifactStore>,
) -> Result<Value> {
    if !matches!(
        name,
        "preview_snapshot" | "device_screenshot" | "html_preview"
    ) {
        return Ok(json!({"isError":false,"structuredContent":value,"content":[text(&value)]}));
    }
    let object = value.as_object_mut().ok_or(McpError::Internal)?;
    let mut screenshot = object.remove("screenshot").ok_or(McpError::Internal)?;
    let full = object.remove("fullScreenshot");
    let png = screenshot
        .as_object_mut()
        .and_then(|s| s.remove("data"))
        .and_then(|s| s.as_str().map(str::to_owned))
        .ok_or(McpError::Internal)?;
    if name == "preview_snapshot" && input["save"] == true {
        let full_png = full
            .as_ref()
            .and_then(|f| f["data"].as_str())
            .unwrap_or(&png);
        let store = artifacts.ok_or(McpError::Internal)?;
        let path = match store
            .save_screenshot(value["url"].as_str().unwrap_or_default(), full_png)
            .await
        {
            Ok(path) => path,
            Err(error) => return Ok(failure(name, error)),
        };
        if input["includeImage"] != true {
            let saved = json!({"url": cut_utf16(value["url"].as_str().unwrap_or_default(),2048), "screenshotPath":path});
            return Ok(json!({"isError":false,"structuredContent":saved,"content":[text(&saved)]}));
        }
        value["screenshotPath"] = json!(path);
    }
    value["screenshot"] = screenshot;
    let mut content = Vec::new();
    let structured = if name == "preview_snapshot" {
        content.push(text(
            &json!({"url":cut_utf16(value["url"].as_str().unwrap_or_default(),2048)}),
        ));
        let (mut bounded, omitted) = bound_snapshot(value);
        content.push(text(&bounded));
        if !omitted.is_empty() {
            content.push(json!({"type":"text","text":format!("Snapshot text was bounded. Omitted: {}.", omitted.join("; "))}));
            bounded["omitted"] = json!(omitted);
        }
        bounded
    } else {
        content.push(text(&value));
        value
    };
    if name != "preview_snapshot" || input["includeImage"] == true {
        content.push(json!({"type":"image","data":png,"mimeType":"image/png"}));
    }
    Ok(json!({"isError":false,"structuredContent":structured,"content":content}))
}

fn length(value: &Value) -> usize {
    value.as_str().map_or(0, |s| s.encode_utf16().count())
}
fn cut_field(value: &mut Value, key: &str, cap: usize) {
    if let Some(s) = value[key].as_str() {
        value[key] = json!(cut_utf16(s, cap));
    }
}
fn half(length: usize) -> usize {
    if length > 2000 {
        (length / 2).max(2000)
    } else {
        length / 2
    }
}

pub fn bound_snapshot(mut value: Value) -> (Value, Vec<String>) {
    let mut omitted = Vec::new();
    let tree = value
        .as_object_mut()
        .and_then(|object| object.remove("accessibilityTree"));
    let tree_text = tree.as_ref().and_then(Value::as_str).map(str::to_owned);
    if tree.is_some() && tree_text.is_none() {
        omitted.push(
            "accessibilityTree (use interactiveElements locators or preview_evaluate)".into(),
        );
    }
    if length(&value["url"]) > 2048 || length(&value["title"]) > 2048 {
        omitted.push("url or title after 2048 characters".into());
    }
    cut_field(&mut value, "url", 2048);
    cut_field(&mut value, "title", 2048);
    if let Some(elements) = value["interactiveElements"].as_array_mut() {
        if elements.iter().any(|e| length(&e["name"]) > 200) {
            omitted.push("element names longer than 200 characters".into());
        }
        for element in elements {
            cut_field(element, "name", 200);
        }
    }
    for (key, label) in [
        ("consoleEntries", "console entries"),
        ("networkEntries", "network entries"),
        ("actionTimeline", "action timeline entries"),
    ] {
        if let Some(entries) = value[key].as_array_mut() {
            if entries.len() > 40 {
                omitted.push(format!("{} older {label}", entries.len() - 40));
                entries.drain(..entries.len() - 40);
            }
            if entries.iter().any(|e| {
                e.as_object()
                    .is_some_and(|o| o.values().any(|s| length(s) > 500))
            }) {
                omitted.push(format!("{label} text after 500 characters"));
            }
            for entry in entries {
                if let Some(object) = entry.as_object_mut() {
                    for v in object.values_mut() {
                        if let Some(s) = v.as_str() {
                            *v = json!(cut_utf16(s, 500));
                        }
                    }
                }
            }
        }
    }
    let visible = value["visibleText"].as_str().unwrap_or_default().to_owned();
    let mut visible_len = visible.encode_utf16().count().min(8000);
    let mut tree_len = tree_text
        .as_ref()
        .map_or(0, |s| s.encode_utf16().count().min(20_000));
    let order = [
        "actionTimeline",
        "networkEntries",
        "consoleEntries",
        "interactiveElements",
    ];
    let original = order.map(|key| value[key].as_array().map_or(0, Vec::len));
    let mut dropped = [0; 4];
    loop {
        value["visibleText"] = json!(cut_utf16(&visible, visible_len));
        if let Some(tree) = &tree_text {
            value["accessibilityTree"] = json!(cut_utf16(tree, tree_len));
        }
        if value.to_string().len() <= MAX_SNAPSHOT_TEXT_BYTES {
            break;
        }
        let logs = order[..3]
            .iter()
            .position(|key| value[*key].as_array().is_some_and(|a| !a.is_empty()));
        if let Some(index) = logs {
            shed(&mut value, order[index], &mut dropped[index]);
        } else if visible_len > 2000 {
            visible_len = half(visible_len);
        } else if tree_len > 2000 {
            tree_len = half(tree_len);
        } else if visible_len > 0 {
            visible_len = half(visible_len);
        } else if value["interactiveElements"]
            .as_array()
            .is_some_and(|a| !a.is_empty())
        {
            shed(&mut value, "interactiveElements", &mut dropped[3]);
        } else if tree_len > 0 {
            tree_len = half(tree_len);
        } else {
            break;
        }
    }
    if visible_len < visible.encode_utf16().count() {
        omitted.push(format!(
            "visibleText after {visible_len} characters (use preview_evaluate for more)"
        ));
    }
    if tree_text
        .as_ref()
        .is_some_and(|s| tree_len < s.encode_utf16().count())
    {
        omitted.push(format!("accessibilityTree after {tree_len} characters"));
    }
    for i in 0..4 {
        if dropped[i] > 0 {
            omitted.push(format!("{} of {} {}", dropped[i], original[i], order[i]));
        }
    }
    (value, omitted)
}

fn shed(value: &mut Value, key: &str, dropped: &mut usize) {
    if let Some(array) = value[key].as_array_mut() {
        let keep = array.len() / 2;
        *dropped += array.len() - keep;
        if key == "interactiveElements" {
            array.truncate(keep);
        } else {
            array.drain(..array.len() - keep);
        }
    }
}
