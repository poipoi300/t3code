use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub const GROK_MONITOR_PROMPT: &str = r#"Use the Monitor tool to watch this command: 'for i in 1 2 3; do sleep 8; echo tick $i; done'. Do not wait for the monitor to finish; as soon as it has started, end your turn by replying exactly ROOT_DONE."#;

pub const GROK_MONITOR_TICKS: &[&str] = &[r#"tick 1"#, r#"tick 2"#, r#"tick 3"#];

pub fn grok_monitor_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"steps":[{"type":"message","text":"Use the Monitor tool to watch this command: 'for i in 1 2 3; do sleep 8; echo tick $i; done'. Do not wait for the monitor to finish; as soon as it has started, end your turn by replying exactly ROOT_DONE."},{"type":"finish_held_run","targetRunIndex":1,"status":"completed"},{"type":"release_replay_gate","label":"notification:session/update:agent_message_chunk:notifications-01a0d754-c2a6-7e51-b5d9-710c43051847"},{"type":"finish_held_run","targetRunIndex":2,"status":"completed"}]}"#,
    )?)
}
