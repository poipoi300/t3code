use serde_json::Value;
use t3_contracts::orchestration_v2::OrchestrationV2TurnItem;

pub const MAX_TOOL_OUTPUT_IMAGES: usize = 8;
pub const MAX_TOOL_OUTPUT_IMAGE_BASE64_LENGTH: usize =
    (t3_contracts::chat_attachment::PROVIDER_SEND_TURN_MAX_IMAGE_BYTES as usize).div_ceil(3) * 4;

pub fn read_tool_output_image(block: &Value) -> Option<(&str, Option<&str>)> {
    if block.get("type")?.as_str()? != "image" {
        return None;
    }
    let (mime, data) = if let Some(source) = block.get("source").filter(|v| v.is_object()) {
        if source.get("type").and_then(Value::as_str) != Some("base64") {
            return None;
        }
        (
            source.get("media_type")?.as_str()?,
            source.get("data").and_then(Value::as_str),
        )
    } else {
        (
            block.get("mimeType")?.as_str()?,
            block.get("data").and_then(Value::as_str),
        )
    };
    matches!(
        mime.to_ascii_lowercase().as_str(),
        "image/png" | "image/jpeg" | "image/gif" | "image/webp"
    )
    .then_some((mime, data))
}

pub fn tool_output_image_blocks(output: &Value) -> Vec<&Value> {
    let blocks = output
        .as_array()
        .or_else(|| output.get("content").and_then(Value::as_array));
    match blocks {
        Some(blocks) => blocks
            .iter()
            .filter(|v| read_tool_output_image(v).is_some())
            .take(MAX_TOOL_OUTPUT_IMAGES)
            .collect(),
        None => {
            if read_tool_output_image(output).is_some() {
                vec![output]
            } else {
                vec![]
            }
        }
    }
}

fn is_base64_body(text: &str) -> bool {
    let body = text.trim_end_matches('=');
    text.len() >= 64
        && text.len() - body.len() <= 2
        && !body.is_empty()
        && body
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"+/_-".contains(&b))
}

fn strip(value: &mut Value, key: Option<&str>, served: &[usize], depth: usize) {
    if depth > 32 {
        return;
    }
    let identity = value as *const Value as usize;
    if served.contains(&identity) {
        return;
    }
    match value {
        Value::Array(entries) => {
            for entry in entries {
                strip(entry, None, served, depth + 1);
            }
        }
        Value::Object(record) => {
            let kind = record.get("type").and_then(Value::as_str);
            let has_mime = ["mimeType", "mime_type", "media_type", "type"]
                .iter()
                .any(|key| {
                    record
                        .get(*key)
                        .and_then(Value::as_str)
                        .is_some_and(|s| s.to_ascii_lowercase().starts_with("image/"))
                });
            let image_body = kind == Some("image")
                || key == Some("image")
                || (has_mime
                    && (kind.is_none()
                        || kind == Some("base64")
                        || kind.is_some_and(|s| s.to_ascii_lowercase().starts_with("image/"))));
            let mut size = None;
            for (entry_key, entry) in record.iter_mut() {
                if image_body
                    && matches!(entry_key.as_str(), "data" | "blob" | "base64")
                    && let Some(text) = entry.as_str().filter(|text| is_base64_body(text))
                {
                    size = Some(text.trim_end_matches('=').len() * 3 / 4);
                    continue;
                }
                strip(entry, Some(entry_key), served, depth + 1);
            }
            if let Some(size) = size {
                record.retain(|key, value| {
                    !(image_body
                        && matches!(key.as_str(), "data" | "blob" | "base64")
                        && value.as_str().is_some_and(is_base64_body))
                });
                record.insert("sizeBytes".into(), size.into());
            }
        }
        _ => {}
    }
}

/// Mutates owned provider output; references used to recognize served blocks stay valid until visited.
pub fn strip_unserved_output_image_bytes(output: &mut Value) {
    let served: Vec<_> = tool_output_image_blocks(output)
        .into_iter()
        .filter(|block| {
            read_tool_output_image(block).is_some_and(|(_, data)| {
                data.is_none_or(|s| s.len() <= MAX_TOOL_OUTPUT_IMAGE_BASE64_LENGTH)
            })
        })
        .map(|v| v as *const Value as usize)
        .collect();
    strip(output, None, &served, 0);
}

pub fn strip_unserved_tool_output_image_bytes(item: &mut OrchestrationV2TurnItem) {
    if let OrchestrationV2TurnItem::DynamicTool(tool) = item
        && let Some(Some(output)) = &mut tool.output
    {
        strip_unserved_output_image_bytes(output);
    }
}
