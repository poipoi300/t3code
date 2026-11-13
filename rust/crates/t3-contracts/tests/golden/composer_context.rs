use super::super::{Registry, register};
use t3_contracts::composer_context::*;
pub fn register_module(registry: &mut Registry) {
    register::<ComposerContextId>(registry, "ComposerContextId");
    register::<ComposerContextKind>(registry, "ComposerContextKind");
    register::<ComposerContextRecord>(registry, "ComposerContextRecord");
    register::<ComposerContextReferenceId>(registry, "ComposerContextReferenceId");
    register::<ElementContextDetails>(registry, "ElementContextDetails");
    register::<ElementContextRecord>(registry, "ElementContextRecord");
    register::<ElementContextSource>(registry, "ElementContextSource");
    register::<EnvironmentContextRecord>(registry, "EnvironmentContextRecord");
    register::<FileContextRecord>(registry, "FileContextRecord");
    register::<GuestMessageAttribution>(registry, "GuestMessageAttribution");
    register::<ImageContextRecord>(registry, "ImageContextRecord");
    register::<KnownComposerContextRecord>(registry, "KnownComposerContextRecord");
    register::<MentionContextRecord>(registry, "MentionContextRecord");
    register::<OrchestrationMessageContext>(registry, "OrchestrationMessageContext");
    register::<PreviewAnnotationContextRecord>(registry, "PreviewAnnotationContextRecord");
    register::<PullRequestContextMetadata>(registry, "PullRequestContextMetadata");
    register::<ResponseContextRecord>(registry, "ResponseContextRecord");
    register::<ReviewCommentContextRecord>(registry, "ReviewCommentContextRecord");
    register::<SkillContextRecord>(registry, "SkillContextRecord");
    register::<TerminalContextRecord>(registry, "TerminalContextRecord");
    register::<ThreadContextRecord>(registry, "ThreadContextRecord");
    register::<UnknownContextRecord>(registry, "UnknownContextRecord");
}
#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("composer_context", register_module)
}

#[test]
fn lenient_context_filters_invalid_records_and_rejects_duplicate_ids()
-> Result<(), serde_json::Error> {
    use serde_json::json;
    let mention = json!({"version": 1, "contextId": "mention", "label": "File", "kind": "mention", "path": "src/main.rs"});
    let future = json!({"version": 1, "contextId": "future", "label": "Future", "kind": "future", "payload": {"number": 9007199254740991_u64}});
    let malformed =
        json!({"version": 1, "contextId": "bad", "label": "Bad", "kind": "mention", "payload": {}});
    let reversed = json!({"version": 1, "contextId": "reversed", "label": "Terminal", "kind": "terminal", "terminalId": "term", "terminalLabel": "Shell", "lineStart": 3, "lineEnd": 1, "text": "output"});
    let value = json!({"version": 1, "records": [mention, future, malformed, reversed, null]});
    let decoded: OrchestrationMessageContext = serde_json::from_value(value)?;
    assert_eq!(
        serde_json::to_value(decoded)?,
        json!({"version": 1, "records": [mention, future]})
    );
    assert!(serde_json::from_value::<UnknownContextRecord>(malformed).is_err());
    assert!(serde_json::from_value::<TerminalContextRecord>(reversed).is_err());
    assert!(
        serde_json::from_value::<OrchestrationMessageContext>(
            json!({"version": 1, "records": [mention, mention]})
        )
        .is_err()
    );
    assert!(
        serde_json::from_value::<OrchestrationMessageContext>(
            json!({"version": 1, "records": vec![json!(null); 201]})
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn unknown_payload_limit_counts_utf16_code_units() -> Result<(), serde_json::Error> {
    use serde_json::json;
    let mut value = json!({"version": 1, "contextId": "future", "label": "Future", "kind": "future", "payload": "😀".repeat(31_999)});
    let record: UnknownContextRecord = serde_json::from_value(value.clone())?;
    assert_eq!(serde_json::to_value(record)?, value);
    value["payload"] = json!("😀".repeat(32_000));
    assert!(serde_json::from_value::<UnknownContextRecord>(value).is_err());
    Ok(())
}
