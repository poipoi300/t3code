use super::super::{Registry, register};
use t3_contracts::thread_metadata_mcp::*;

pub fn register_module(registry: &mut Registry) {
    register::<ThreadMetadataMcpAction>(registry, "ThreadMetadataMcpAction");
    register::<ThreadMetadataMcpUpdateInput>(registry, "ThreadMetadataMcpUpdateInput");
    register::<ThreadMetadataMcpUpdateResult>(registry, "ThreadMetadataMcpUpdateResult");
}

#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("thread_metadata_mcp", register_module)
}

#[test]
fn validates_action_and_utf16_lengths() -> Result<(), serde_json::Error> {
    use serde_json::{from_value, json};
    for input in [
        json!({"action": "rename", "title": " New title "}),
        json!({"action": "rename", "title": "x".repeat(512)}),
        json!({"action": "rename", "title": "😀".repeat(256)}),
        json!({"action": "regenerate_title", "title": null}),
        json!({"action": "regenerate_title", "clientRequestId": "x".repeat(256)}),
    ] {
        from_value::<ThreadMetadataMcpUpdateInput>(input)?;
    }
    for input in [
        json!({"action": "rename"}),
        json!({"action": "rename", "title": null}),
        json!({"action": "rename", "title": " "}),
        json!({"action": "regenerate_title", "title": "Title"}),
        json!({"action": "rename", "title": "x".repeat(513)}),
        json!({"action": "rename", "title": "😀".repeat(257)}),
        json!({"action": "regenerate_title", "clientRequestId": "x".repeat(257)}),
    ] {
        assert!(
            from_value::<ThreadMetadataMcpUpdateInput>(input.clone()).is_err(),
            "{input}"
        );
    }
    assert!(
        serde_json::from_str::<ThreadMetadataMcpUpdateInput>(
            r#"{"action":"regenerate_title","clientRequestId":"\ud800"}"#
        )
        .is_err()
    );
    Ok(())
}
