#![allow(clippy::unwrap_used)]
use serde_json::{Value, json};
use t3_mcp::output::*;

fn snapshot() -> Value {
    json!({"url":"https://example.com/","title":"Title","visibleText":"Example","interactiveElements":[{"name":"Click","locator":"aria-ref=1"}],"consoleEntries":[],"networkEntries":[],"actionTimeline":[],"accessibilityTree":"- button Click","screenshot":{"mimeType":"image/png","data":"cG5n","width":10,"height":5}})
}
#[tokio::test]
async fn images_are_opt_in_for_browser_snapshots_and_always_delivered_for_image_tools() {
    for input in [
        json!({}),
        json!({"includeImage":false}),
        json!({"includeImage":null}),
    ] {
        let result = success("preview_snapshot", &input, snapshot(), None)
            .await
            .unwrap();
        assert!(
            !result["content"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p["type"] == "image")
        );
        assert!(
            result["structuredContent"]["screenshot"]
                .get("data")
                .is_none()
        );
    }
    for name in ["preview_snapshot", "device_screenshot", "html_preview"] {
        let result = success(name, &json!({"includeImage":true}), snapshot(), None)
            .await
            .unwrap();
        assert_eq!(
            result["content"].as_array().unwrap().last().unwrap()["data"],
            "cG5n"
        );
    }
}
#[test]
fn wide_snapshot_text_logs_and_identifiers_are_bounded_before_losing_locators() {
    let mut metadata = snapshot();
    metadata.as_object_mut().unwrap().remove("screenshot");
    metadata["visibleText"] = json!("界".repeat(100_000));
    metadata["accessibilityTree"] = json!("- button Click\n".repeat(10_000));
    metadata["title"] = json!("x".repeat(100_000));
    for key in ["consoleEntries", "networkEntries", "actionTimeline"] {
        metadata[key] = json!(
            (0..100)
                .map(|_| json!({"message":"x".repeat(2000)}))
                .collect::<Vec<_>>()
        );
    }
    let (bounded, omitted) = bound_snapshot(metadata);
    assert!(bounded.to_string().len() <= MAX_SNAPSHOT_TEXT_BYTES);
    assert!(
        !bounded["interactiveElements"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(!omitted.is_empty());
}

#[test]
fn image_failures_expose_only_server_authored_recovery_messages() {
    for (name, text) in [
        ("preview_snapshot", "Preview snapshot failed: ForeignError."),
        ("html_preview", "HTML preview failed."),
        ("device_screenshot", "Device screenshot failed."),
    ] {
        let result = failure(
            name,
            t3_mcp::McpError::Domain {
                tag: "ForeignError".into(),
                message: "page-controlled output".into(),
                fields: json!({}),
            },
        );
        assert_eq!(result["content"][0]["text"], text);
        assert!(!result.to_string().contains("page-controlled output"));
    }
    let result = failure(
        "device_screenshot",
        t3_mcp::McpError::Domain {
            tag: "DeviceOperationError".into(),
            message: "Device rejected screenshot.".into(),
            fields: json!({"reason":"hub_rejected"}),
        },
    );
    assert_eq!(
        result["content"][0]["text"],
        "Device rejected screenshot. Call device_list to check the device state, and device_open if it needs to be reopened."
    );
}
