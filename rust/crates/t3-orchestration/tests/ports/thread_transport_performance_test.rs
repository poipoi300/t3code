use serde_json::json;
use t3_orchestration::{
    api::*, error::Result, ports::transport::wire_projection::project_domain_event_for_wire,
};
#[test]
fn command_output_is_omitted_from_live_rpc_without_mutating_the_persisted_source() -> Result<()> {
    let source = json!({"id":"event-item-0","type":"turn-item.updated","threadId":"thread-perf","occurredAt":"2026-06-20T00:00:00.000Z","payload":{"id":"item-0","type":"command_execution","threadId":"thread-perf","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":"Command 0","input":"cmd-0","output":"x".repeat(8192),"exitCode":0,"startedAt":"2026-06-20T00:00:00.000Z","completedAt":"2026-06-20T00:00:00.000Z","updatedAt":"2026-06-20T00:00:00.000Z"}});
    let mut projected: OrchestrationV2DomainEvent = serde_json::from_value(source.clone())?;
    project_domain_event_for_wire(&mut projected)?;
    let projected = t3_contracts::stored::to_value(&projected)?;
    let encode = |event| {
        serde_json::to_vec(&json!({"_tag":"Chunk","requestId":0,"values":[{"kind":"event","sequence":1,"event":event}]})).unwrap().len()
    };
    let original_bytes = encode(source.clone());
    let projected_bytes = encode(projected.clone());
    assert_eq!(original_bytes, 8790);
    assert!(projected_bytes <= 1024);
    assert!(projected_bytes * 10 <= original_bytes);
    assert_eq!(source["payload"]["output"].as_str().unwrap().len(), 8192);
    assert!(projected["payload"].get("output").is_none());
    Ok(())
}
