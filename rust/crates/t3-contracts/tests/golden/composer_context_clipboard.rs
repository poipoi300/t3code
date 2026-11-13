use super::super::{Registry, register};
use t3_contracts::composer_context_clipboard::*;
pub fn register_module(registry: &mut Registry) {
    register::<ComposerContextClipboardFragment>(registry, "ComposerContextClipboardFragment");
}
#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("composer_context_clipboard", register_module)
}

#[test]
fn clipboard_filters_records_without_imposing_message_uniqueness() -> Result<(), serde_json::Error>
{
    use serde_json::json;
    let record = json!({"version": 1, "contextId": "file", "label": "File", "kind": "mention", "path": "src/main.rs"});
    let value = json!({"version": 1, "source": {"environmentId": "environment", "threadId": null}, "records": [record, null, record]});
    let decoded: ComposerContextClipboardFragment = serde_json::from_value(value)?;
    assert_eq!(
        serde_json::to_value(decoded)?,
        json!({"version": 1, "source": {"environmentId": "environment", "threadId": null}, "records": [record, record]})
    );
    assert!(serde_json::from_value::<ComposerContextClipboardFragment>(json!({"version": 1, "source": {"environmentId": "environment"}, "records": vec![json!(null); 201]})).is_err());
    Ok(())
}
