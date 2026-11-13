use serde_json::{Value, json};
use t3_orchestration::ports::runs_ingestion::workflow_agent_answers::*;

fn metadata() -> WorkflowAgentMetadata {
    WorkflowAgentMetadata {
        message_id: Some("a".into()),
        effort: Some("max".into()),
        speed: Some("standard".into()),
    }
}

#[test]
fn split_assistant_messages_preserve_effort_and_speed_across_reads() {
    let first = merge_workflow_agent_metadata(
        &[
            json!({"type":"assistant","effort":"xhigh","message":{"id":"a","content":[],"usage":{"speed":"standard"}}}),
        ],
        WorkflowAgentMetadata::default(),
    );
    assert_eq!(
        first,
        WorkflowAgentMetadata {
            message_id: Some("a".into()),
            effort: Some("xhigh".into()),
            speed: Some("standard".into())
        }
    );
    assert_eq!(
        merge_workflow_agent_metadata(
            &[
                json!({"type":"assistant","message":{"id":"a","content":[{"type":"text","text":"Done"}]}})
            ],
            first.clone()
        ),
        first
    );
    assert_eq!(merge_workflow_agent_metadata(&[], first.clone()), first);
}

#[test]
fn bookkeeping_api_errors_and_invalid_fields_do_not_clear_prior_metadata() {
    assert_eq!(
        merge_workflow_agent_metadata(
            &[
                json!({"type":"user","effort":"high","message":{"usage":{"speed":"fast"}}}),
                json!({"type":"assistant","isMeta":true,"effort":"high"}),
                json!({"type":"assistant","isApiErrorMessage":true,"message":{"id":"error"}}),
                json!({"type":"assistant","effort":1,"message":{"id":"a","usage":{"speed":false}}}),
                Value::Null
            ],
            metadata()
        ),
        metadata()
    );
}

#[test]
fn new_responses_clear_omitted_metadata_with_or_without_a_message_id() {
    for id in [Some("b"), None] {
        assert_eq!(
            merge_workflow_agent_metadata(
                &[json!({"type":"assistant","message":{"id":id,"content":[]}})],
                metadata()
            ),
            WorkflowAgentMetadata {
                message_id: id.map(str::to_owned),
                ..Default::default()
            }
        );
    }
}
