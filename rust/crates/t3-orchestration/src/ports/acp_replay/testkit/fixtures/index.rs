use crate::testing::replay::RecordedFixture;

pub const RECORDED_FIXTURES: &[RecordedFixture] = &[
    RecordedFixture {
        name: "acp_elicitation",
        file: "acp_elicitation/registry_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/acp_elicitation/registry_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "acp_titled_multiselect",
        file: "acp_titled_multiselect/registry_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/acp_titled_multiselect/registry_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "claude_background_monitor_wake",
        file: "claude_background_monitor_wake/claude_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/claude_background_monitor_wake/claude_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "claude_background_subagent_after_root",
        file: "claude_background_subagent_after_root/claude_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/claude_background_subagent_after_root/claude_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "claude_background_subagent_lifecycle",
        file: "claude_background_subagent_lifecycle/claude_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/claude_background_subagent_lifecycle/claude_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "claude_background_task_after_root",
        file: "claude_background_task_after_root/claude_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/claude_background_task_after_root/claude_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "claude_background_task_interrupt",
        file: "claude_background_task_interrupt/claude_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/claude_background_task_interrupt/claude_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "claude_background_task_wake",
        file: "claude_background_task_wake/claude_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/claude_background_task_wake/claude_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "claude_background_wake_before_queued_prompt",
        file: "claude_background_wake_before_queued_prompt/claude_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/claude_background_wake_before_queued_prompt/claude_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "claude_background_wake_before_queued_prompt_no_echo",
        file: "claude_background_wake_before_queued_prompt_no_echo/claude_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/claude_background_wake_before_queued_prompt_no_echo/claude_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "claude_compact_after_peer_turn",
        file: "claude_compact_after_peer_turn/claude_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/claude_compact_after_peer_turn/claude_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "claude_compact_after_peer_turn_no_echo",
        file: "claude_compact_after_peer_turn_no_echo/claude_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/claude_compact_after_peer_turn_no_echo/claude_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "claude_compact_after_resume_wake",
        file: "claude_compact_after_resume_wake/claude_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/claude_compact_after_resume_wake/claude_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "claude_idle_resume",
        file: "claude_idle_resume/claude_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/claude_idle_resume/claude_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "claude_local_bash_task",
        file: "claude_local_bash_task/claude_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/claude_local_bash_task/claude_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "claude_mcp_tool_presentation",
        file: "claude_mcp_tool_presentation/claude_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/claude_mcp_tool_presentation/claude_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "claude_nested_background_subagent_wake",
        file: "claude_nested_background_subagent_wake/claude_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/claude_nested_background_subagent_wake/claude_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "claude_nested_subagent_model",
        file: "claude_nested_subagent_model/claude_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/claude_nested_subagent_model/claude_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "claude_result_is_error",
        file: "claude_result_is_error/claude_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/claude_result_is_error/claude_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "claude_subagent_resume_after_restart",
        file: "claude_subagent_resume_after_restart/claude_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/claude_subagent_resume_after_restart/claude_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "codex_model_reroute",
        file: "codex_model_reroute/codex_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/codex_model_reroute/codex_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "command_output_streaming",
        file: "command_output_streaming/claude_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/command_output_streaming/claude_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "command_output_streaming",
        file: "command_output_streaming/codex_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/command_output_streaming/codex_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "delegated_task_status",
        file: "delegated_task_status/codex_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/delegated_task_status/codex_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "fx_early_stop",
        file: "fx_early_stop/fx_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/fx_early_stop/fx_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "fx_subagent",
        file: "fx_subagent/fx_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/fx_subagent/fx_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "grok_auto_blocked_command",
        file: "grok_auto_blocked_command/grok_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/grok_auto_blocked_command/grok_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "grok_background_bash",
        file: "grok_background_bash/grok_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/grok_background_bash/grok_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "grok_background_bash_fast_wake",
        file: "grok_background_bash_fast_wake/grok_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/grok_background_bash_fast_wake/grok_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "grok_background_subagent",
        file: "grok_background_subagent/grok_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/grok_background_subagent/grok_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "grok_monitor",
        file: "grok_monitor/grok_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/grok_monitor/grok_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "grok_prompt_error",
        file: "grok_prompt_error/grok_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/grok_prompt_error/grok_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "grok_subagent_lineage",
        file: "grok_subagent_lineage/grok_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/grok_subagent_lineage/grok_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "kiro_cloud_config",
        file: "kiro_cloud_config/kiro_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/kiro_cloud_config/kiro_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "kiro_compaction",
        file: "kiro_compaction/kiro_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/kiro_compaction/kiro_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "kiro_effort",
        file: "kiro_effort/kiro_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/kiro_effort/kiro_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "kiro_fork_from_turn",
        file: "kiro_fork_from_turn/kiro_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/kiro_fork_from_turn/kiro_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "kiro_message_steering",
        file: "kiro_message_steering/kiro_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/kiro_message_steering/kiro_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "kiro_model_switch",
        file: "kiro_model_switch/kiro_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/kiro_model_switch/kiro_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "kiro_permission_modes",
        file: "kiro_permission_modes/kiro_full_access_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/kiro_permission_modes/kiro_full_access_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "kiro_permission_modes",
        file: "kiro_permission_modes/kiro_supervised_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/kiro_permission_modes/kiro_supervised_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "kiro_subagent_interrupt",
        file: "kiro_subagent_interrupt/kiro_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/kiro_subagent_interrupt/kiro_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "kiro_unknown_model",
        file: "kiro_unknown_model/kiro_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/kiro_unknown_model/kiro_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "malformed_session_update",
        file: "malformed_session_update/registry_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/malformed_session_update/registry_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "message_steering",
        file: "message_steering/claude_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/message_steering/claude_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "message_steering",
        file: "message_steering/codex_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/message_steering/codex_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "message_steering",
        file: "message_steering/cursor_restart_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/message_steering/cursor_restart_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "message_steering",
        file: "message_steering/cursor_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/message_steering/cursor_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "message_steering",
        file: "message_steering/fx_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/message_steering/fx_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "message_steering",
        file: "message_steering/grok_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/message_steering/grok_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "message_steering",
        file: "message_steering/pi_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/message_steering/pi_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "message_steering",
        file: "message_steering/registry_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/message_steering/registry_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "multi_turn",
        file: "multi_turn/claude_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/multi_turn/claude_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "multi_turn",
        file: "multi_turn/codex_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/multi_turn/codex_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "multi_turn",
        file: "multi_turn/cursor_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/multi_turn/cursor_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "multi_turn",
        file: "multi_turn/fx_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/multi_turn/fx_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "multi_turn",
        file: "multi_turn/grok_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/multi_turn/grok_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "multi_turn",
        file: "multi_turn/kiro_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/multi_turn/kiro_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "multi_turn",
        file: "multi_turn/muse_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/multi_turn/muse_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "multi_turn",
        file: "multi_turn/pi_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/multi_turn/pi_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "multi_turn",
        file: "multi_turn/registry_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/multi_turn/registry_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "multi_turn_restart",
        file: "multi_turn_restart/claude_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/multi_turn_restart/claude_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "muse_permission",
        file: "muse_permission/muse_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/muse_permission/muse_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "muse_subagent_approval",
        file: "muse_subagent_approval/muse_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/muse_subagent_approval/muse_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "muse_subagent_interrupt",
        file: "muse_subagent_interrupt/muse_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/muse_subagent_interrupt/muse_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "muse_workflow",
        file: "muse_workflow/muse_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/muse_workflow/muse_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "opencode2_background",
        file: "opencode2_background/opencode_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/opencode2_background/opencode_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "opencode2_command",
        file: "opencode2_command/opencode_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/opencode2_command/opencode_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "opencode2_compaction",
        file: "opencode2_compaction/opencode_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/opencode2_compaction/opencode_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "opencode2_fork",
        file: "opencode2_fork/opencode_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/opencode2_fork/opencode_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "opencode2_inbox",
        file: "opencode2_inbox/opencode_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/opencode2_inbox/opencode_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "opencode2_interrupt",
        file: "opencode2_interrupt/opencode_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/opencode2_interrupt/opencode_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "opencode2_missing_step_usage",
        file: "opencode2_missing_step_usage/opencode_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/opencode2_missing_step_usage/opencode_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "opencode2_nested_background",
        file: "opencode2_nested_background/opencode_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/opencode2_nested_background/opencode_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "opencode2_permission",
        file: "opencode2_permission/opencode_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/opencode2_permission/opencode_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "opencode2_question",
        file: "opencode2_question/opencode_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/opencode2_question/opencode_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "opencode2_resume_after_restart",
        file: "opencode2_resume_after_restart/opencode_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/opencode2_resume_after_restart/opencode_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "opencode2_revert",
        file: "opencode2_revert/opencode_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/opencode2_revert/opencode_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "opencode2_simple",
        file: "opencode2_simple/opencode_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/opencode2_simple/opencode_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "opencode2_skill",
        file: "opencode2_skill/opencode_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/opencode2_skill/opencode_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "opencode2_subagent",
        file: "opencode2_subagent/opencode_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/opencode2_subagent/opencode_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "opencode2_tool_call",
        file: "opencode2_tool_call/opencode_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/opencode2_tool_call/opencode_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "opencode_child_approval",
        file: "opencode_child_approval/opencode_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/opencode_child_approval/opencode_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "opencode_running_child_approval",
        file: "opencode_running_child_approval/opencode_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/opencode_running_child_approval/opencode_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "opencode_subagent",
        file: "opencode_subagent/opencode_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/opencode_subagent/opencode_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "pi_compaction",
        file: "pi_compaction/pi_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/pi_compaction/pi_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "plan_questions",
        file: "plan_questions/codex_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/plan_questions/codex_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "plan_questions",
        file: "plan_questions/grok_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/plan_questions/grok_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "plan_questions",
        file: "plan_questions/opencode_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/plan_questions/opencode_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "proposed_plan",
        file: "proposed_plan/codex_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/proposed_plan/codex_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "proposed_plan",
        file: "proposed_plan/cursor_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/proposed_plan/cursor_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "provider_thread_resume",
        file: "provider_thread_resume/codex_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/provider_thread_resume/codex_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "provider_thread_resume",
        file: "provider_thread_resume/cursor_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/provider_thread_resume/cursor_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "provider_thread_resume",
        file: "provider_thread_resume/pi_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/provider_thread_resume/pi_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "queued_turn",
        file: "queued_turn/claude_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/queued_turn/claude_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "queued_turn",
        file: "queued_turn/codex_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/queued_turn/codex_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "queued_turn",
        file: "queued_turn/cursor_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/queued_turn/cursor_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "queued_turn",
        file: "queued_turn/fx_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/queued_turn/fx_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "queued_turn",
        file: "queued_turn/grok_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/queued_turn/grok_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "queued_turn",
        file: "queued_turn/kiro_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/queued_turn/kiro_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "queued_turn",
        file: "queued_turn/registry_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/queued_turn/registry_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "simple",
        file: "simple/claude_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/simple/claude_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "simple",
        file: "simple/codex_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/simple/codex_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "simple",
        file: "simple/cursor_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/simple/cursor_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "simple",
        file: "simple/fx_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/simple/fx_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "simple",
        file: "simple/grok_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/simple/grok_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "simple",
        file: "simple/kiro_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/simple/kiro_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "simple",
        file: "simple/muse_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/simple/muse_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "simple",
        file: "simple/opencode_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/simple/opencode_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "simple",
        file: "simple/pi_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/simple/pi_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "simple",
        file: "simple/registry_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/simple/registry_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "skill_invocation",
        file: "skill_invocation/cursor_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/skill_invocation/cursor_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "stop_background_work_after_failed_turn",
        file: "stop_background_work_after_failed_turn/registry_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/stop_background_work_after_failed_turn/registry_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "subagent",
        file: "subagent/claude_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/subagent/claude_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "subagent",
        file: "subagent/codex_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/subagent/codex_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "subagent",
        file: "subagent/cursor_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/subagent/cursor_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "subagent",
        file: "subagent/kiro_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/subagent/kiro_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "subagent_continue",
        file: "subagent_continue/codex_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/subagent_continue/codex_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "subagent_v2",
        file: "subagent_v2/codex_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/subagent_v2/codex_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "subagent_v2",
        file: "subagent_v2/muse_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/subagent_v2/muse_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "subagent_v2_approval",
        file: "subagent_v2_approval/codex_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/subagent_v2_approval/codex_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "subagent_v2_nested",
        file: "subagent_v2_nested/codex_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/subagent_v2_nested/codex_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "subagent_v2_nested_approval",
        file: "subagent_v2_nested_approval/codex_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/subagent_v2_nested_approval/codex_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "thread_fork_native",
        file: "thread_fork_native/claude_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/thread_fork_native/claude_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "thread_fork_native_fork_local_rollback",
        file: "thread_fork_native_fork_local_rollback/claude_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/thread_fork_native_fork_local_rollback/claude_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "thread_fork_native_prior_turn",
        file: "thread_fork_native_prior_turn/claude_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/thread_fork_native_prior_turn/claude_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "thread_fork_native_prior_turn",
        file: "thread_fork_native_prior_turn/codex_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/thread_fork_native_prior_turn/codex_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "thread_merge_back_siblings",
        file: "thread_merge_back_siblings/claude_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/thread_merge_back_siblings/claude_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "thread_merge_back_siblings",
        file: "thread_merge_back_siblings/codex_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/thread_merge_back_siblings/codex_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "thread_rollback",
        file: "thread_rollback/claude_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/thread_rollback/claude_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "thread_rollback",
        file: "thread_rollback/codex_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/thread_rollback/codex_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "thread_rollback",
        file: "thread_rollback/pi_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/thread_rollback/pi_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "thread_rollback_after_restart",
        file: "thread_rollback_after_restart/codex_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/thread_rollback_after_restart/codex_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "thread_rollback_after_stop",
        file: "thread_rollback_after_stop/pi_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/thread_rollback_after_stop/pi_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "thread_rollback_to_stopped_turn",
        file: "thread_rollback_to_stopped_turn/codex_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/thread_rollback_to_stopped_turn/codex_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "todo_list",
        file: "todo_list/codex_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/todo_list/codex_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "todo_list",
        file: "todo_list/cursor_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/todo_list/cursor_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "todo_list",
        file: "todo_list/grok_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/todo_list/grok_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "todo_list",
        file: "todo_list/registry_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/todo_list/registry_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "tool_call_denied_write",
        file: "tool_call_denied_write/claude_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/tool_call_denied_write/claude_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "tool_call_denied_write",
        file: "tool_call_denied_write/fx_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/tool_call_denied_write/fx_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "tool_call_read_only",
        file: "tool_call_read_only/claude_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/tool_call_read_only/claude_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "tool_call_read_only",
        file: "tool_call_read_only/cursor_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/tool_call_read_only/cursor_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "tool_call_read_only",
        file: "tool_call_read_only/grok_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/tool_call_read_only/grok_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "tool_call_read_only",
        file: "tool_call_read_only/registry_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/tool_call_read_only/registry_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "tool_call_read_only_on_request",
        file: "tool_call_read_only_on_request/claude_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/tool_call_read_only_on_request/claude_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "tool_call_read_only_on_request",
        file: "tool_call_read_only_on_request/codex_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/tool_call_read_only_on_request/codex_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "tool_call_read_only_on_request",
        file: "tool_call_read_only_on_request/fx_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/tool_call_read_only_on_request/fx_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "tool_call_read_only_on_request",
        file: "tool_call_read_only_on_request/grok_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/tool_call_read_only_on_request/grok_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "tool_call_read_only_on_request",
        file: "tool_call_read_only_on_request/kiro_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/tool_call_read_only_on_request/kiro_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "tool_call_read_only_on_request",
        file: "tool_call_read_only_on_request/registry_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/tool_call_read_only_on_request/registry_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "tool_call_restricted_granular",
        file: "tool_call_restricted_granular/claude_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/tool_call_restricted_granular/claude_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "tool_call_restricted_granular",
        file: "tool_call_restricted_granular/codex_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/tool_call_restricted_granular/codex_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "tool_call_workspace_never",
        file: "tool_call_workspace_never/claude_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/tool_call_workspace_never/claude_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "tool_call_workspace_never",
        file: "tool_call_workspace_never/codex_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/tool_call_workspace_never/codex_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "turn_interrupt",
        file: "turn_interrupt/claude_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/turn_interrupt/claude_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "turn_interrupt",
        file: "turn_interrupt/codex_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/turn_interrupt/codex_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "turn_interrupt",
        file: "turn_interrupt/fx_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/turn_interrupt/fx_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "turn_interrupt",
        file: "turn_interrupt/grok_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/turn_interrupt/grok_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "turn_interrupt",
        file: "turn_interrupt/kiro_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/turn_interrupt/kiro_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "turn_interrupt",
        file: "turn_interrupt/muse_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/turn_interrupt/muse_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "turn_interrupt",
        file: "turn_interrupt/opencode_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/turn_interrupt/opencode_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "turn_interrupt",
        file: "turn_interrupt/registry_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/turn_interrupt/registry_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "turn_interrupt_mid_tool",
        file: "turn_interrupt_mid_tool/claude_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/turn_interrupt_mid_tool/claude_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "turn_interrupt_mid_tool",
        file: "turn_interrupt_mid_tool/codex_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/turn_interrupt_mid_tool/codex_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "turn_interrupt_mid_tool",
        file: "turn_interrupt_mid_tool/cursor_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/turn_interrupt_mid_tool/cursor_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "turn_interrupt_mid_tool",
        file: "turn_interrupt_mid_tool/kiro_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/turn_interrupt_mid_tool/kiro_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "turn_interrupt_mid_tool",
        file: "turn_interrupt_mid_tool/pi_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/turn_interrupt_mid_tool/pi_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "turn_interrupt_restart",
        file: "turn_interrupt_restart/claude_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/turn_interrupt_restart/claude_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "web_search",
        file: "web_search/claude_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/web_search/claude_transcript.ndjson"
        )),
    },
    RecordedFixture {
        name: "web_search",
        file: "web_search/codex_transcript.ndjson",
        ndjson: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/server/src/orchestration-v2/testkit/fixtures/web_search/codex_transcript.ndjson"
        )),
    },
];

pub fn find_recording(name: &str, file: &str) -> Option<&'static RecordedFixture> {
    RECORDED_FIXTURES
        .iter()
        .find(|fixture| fixture.name == name && fixture.file.rsplit('/').next() == Some(file))
}

pub const INPUT_BUILDERS: &[(&str, &str, super::shared::InputBuilder)] = &[
    ("apps/server/src/orchestration-v2/testkit/fixtures/acp_titled_multiselect/input.ts", "acpTitledMultiSelectInput", super::acp_titled_multiselect::input::acp_titled_multi_select_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/command_output_streaming/input.ts", "commandOutputStreamingInput", super::command_output_streaming::input::command_output_streaming_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/fx_early_stop/input.ts", "fxEarlyStopInput", super::fx_early_stop::input::fx_early_stop_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/fx_subagent/input.ts", "fxSubagentInput", super::fx_subagent::input::fx_subagent_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/grok_auto_blocked_command/input.ts", "grokAutoBlockedCommandInput", super::grok_auto_blocked_command::input::grok_auto_blocked_command_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/grok_background_bash/input.ts", "grokBackgroundBashInput", super::grok_background_bash::input::grok_background_bash_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/grok_background_bash_fast_wake/input.ts", "grokBackgroundBashFastWakeInput", super::grok_background_bash_fast_wake::input::grok_background_bash_fast_wake_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/grok_background_subagent/input.ts", "grokBackgroundSubagentInput", super::grok_background_subagent::input::grok_background_subagent_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/grok_monitor/input.ts", "grokMonitorInput", super::grok_monitor::input::grok_monitor_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/grok_prompt_error/input.ts", "grokPromptErrorInput", super::grok_prompt_error::input::grok_prompt_error_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/grok_subagent_lineage/input.ts", "grokSubagentLineageInput", super::grok_subagent_lineage::input::grok_subagent_lineage_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/kiro_compaction/input.ts", "kiroCompactionInput", super::kiro_compaction::input::kiro_compaction_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/kiro_effort/input.ts", "kiroEffortInput", super::kiro_effort::input::kiro_effort_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/kiro_fork_from_turn/input.ts", "kiroForkFromTurnInput", super::kiro_fork_from_turn::input::kiro_fork_from_turn_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/kiro_message_steering/input.ts", "kiroMessageSteeringInput", super::kiro_message_steering::input::kiro_message_steering_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/kiro_model_switch/input.ts", "kiroModelSwitchInput", super::kiro_model_switch::input::kiro_model_switch_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/kiro_permission_modes/input.ts", "kiroFullAccessWriteInput", super::kiro_permission_modes::input::kiro_full_access_write_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/kiro_permission_modes/input.ts", "kiroSupervisedWriteInput", super::kiro_permission_modes::input::kiro_supervised_write_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/kiro_subagent_interrupt/input.ts", "kiroSubagentInterruptInput", super::kiro_subagent_interrupt::input::kiro_subagent_interrupt_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/malformed_session_update/input.ts", "malformedSessionUpdateInput", super::malformed_session_update::input::malformed_session_update_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/message_steering/input.ts", "messageRestartInput", super::message_steering::input::message_restart_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/message_steering/input.ts", "messageSteeringInput", super::message_steering::input::message_steering_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/multi_turn/input.ts", "multiTurnInput", super::multi_turn::input::multi_turn_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/muse_permission/input.ts", "musePermissionInput", super::muse_permission::input::muse_permission_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/muse_subagent_approval/input.ts", "museSubagentApprovalInput", super::muse_subagent_approval::input::muse_subagent_approval_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/muse_subagent_interrupt/input.ts", "museSubagentInterruptInput", super::muse_subagent_interrupt::input::muse_subagent_interrupt_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/muse_workflow/input.ts", "museWorkflowInput", super::muse_workflow::input::muse_workflow_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/opencode2_background/input.ts", "openCode2BackgroundInput", super::opencode2_background::input::open_code2_background_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/opencode2_command/input.ts", "openCode2CommandInput", super::opencode2_command::input::open_code2_command_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/opencode2_compaction/input.ts", "openCode2CompactionInput", super::opencode2_compaction::input::open_code2_compaction_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/opencode2_inbox/input.ts", "openCode2InboxInput", super::opencode2_inbox::input::open_code2_inbox_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/opencode2_interrupt/input.ts", "openCode2InterruptInput", super::opencode2_interrupt::input::open_code2_interrupt_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/opencode2_nested_background/input.ts", "openCode2NestedBackgroundInput", super::opencode2_nested_background::input::open_code2_nested_background_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/opencode2_permission/input.ts", "openCode2PermissionInput", super::opencode2_permission::input::open_code2_permission_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/opencode2_question/input.ts", "openCode2QuestionInput", super::opencode2_question::input::open_code2_question_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/opencode2_resume_after_restart/input.ts", "openCode2ResumeAfterRestartInput", super::opencode2_resume_after_restart::input::open_code2_resume_after_restart_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/opencode2_revert/input.ts", "openCode2RevertInput", super::opencode2_revert::input::open_code2_revert_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/opencode2_simple/input.ts", "openCode2SimpleInput", super::opencode2_simple::input::open_code2_simple_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/opencode2_skill/input.ts", "openCode2SkillInput", super::opencode2_skill::input::open_code2_skill_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/opencode2_subagent/input.ts", "openCode2SubagentInput", super::opencode2_subagent::input::open_code2_subagent_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/opencode2_tool_call/input.ts", "openCode2ToolCallInput", super::opencode2_tool_call::input::open_code2_tool_call_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/opencode_child_approval/input.ts", "openCodeChildApprovalInput", super::opencode_child_approval::input::open_code_child_approval_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/opencode_running_child_approval/input.ts", "openCodeRunningChildApprovalInput", super::opencode_running_child_approval::input::open_code_running_child_approval_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/opencode_subagent/input.ts", "openCodeSubagentInput", super::opencode_subagent::input::open_code_subagent_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/pi_compaction/input.ts", "piCompactionInput", super::pi_compaction::input::pi_compaction_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/plan_questions/input.ts", "planQuestionsInput", super::plan_questions::input::plan_questions_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/proposed_plan/input.ts", "proposedPlanInput", super::proposed_plan::input::proposed_plan_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/provider_thread_resume/input.ts", "providerThreadResumeInput", super::provider_thread_resume::input::provider_thread_resume_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/queued_cancelled_while_active/input.ts", "queuedCancelledWhileActiveInput", super::queued_cancelled_while_active::input::queued_cancelled_while_active_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/queued_turn/input.ts", "queuedTurnInput", super::queued_turn::input::queued_turn_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/simple/input.ts", "simpleInput", super::simple::input::simple_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/skill_invocation/input.ts", "skillInvocationInput", super::skill_invocation::input::skill_invocation_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/stop_background_work_after_failed_turn/input.ts", "stopBackgroundWorkAfterFailedTurnInput", super::stop_background_work_after_failed_turn::input::stop_background_work_after_failed_turn_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/stop_background_work_after_failed_turn/input.ts", "stopBackgroundWorkAfterReleaseInput", super::stop_background_work_after_failed_turn::input::stop_background_work_after_release_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/subagent/input.ts", "subagentInput", super::subagent::input::subagent_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/subagent_continue/input.ts", "subagentContinueInput", super::subagent_continue::input::subagent_continue_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/subagent_v2/input.ts", "subagentV2Input", super::subagent_v2::input::subagent_v2_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/subagent_v2/input.ts", "subagentV2NestedInput", super::subagent_v2::input::subagent_v2_nested_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/subagent_v2_approval/input.ts", "subagentV2ApprovalInput", super::subagent_v2_approval::input::subagent_v2_approval_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/subagent_v2_nested_approval/input.ts", "subagentV2NestedApprovalInput", super::subagent_v2_nested_approval::input::subagent_v2_nested_approval_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/thread_rollback/input.ts", "threadRollbackInput", super::thread_rollback::input::thread_rollback_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/thread_rollback_after_restart/input.ts", "threadRollbackAfterRestartInput", super::thread_rollback_after_restart::input::thread_rollback_after_restart_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/thread_rollback_after_stop/input.ts", "threadRollbackAfterStopInput", super::thread_rollback_after_stop::input::thread_rollback_after_stop_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/thread_rollback_to_stopped_turn/input.ts", "threadRollbackToStoppedTurnInput", super::thread_rollback_to_stopped_turn::input::thread_rollback_to_stopped_turn_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/todo_list/input.ts", "todoListInput", super::todo_list::input::todo_list_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/tool_call_denied_write/input.ts", "toolCallDeniedWriteInput", super::tool_call_denied_write::input::tool_call_denied_write_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/tool_call_read_only/input.ts", "toolCallReadOnlyInput", super::tool_call_read_only::input::tool_call_read_only_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/tool_call_read_only_on_request/input.ts", "toolCallReadOnlyOnRequestInput", super::tool_call_read_only_on_request::input::tool_call_read_only_on_request_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/tool_call_read_only_on_request/input.ts", "toolCallReadOnlyOnRequestKiroInput", super::tool_call_read_only_on_request::input::tool_call_read_only_on_request_kiro_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/tool_call_restricted_granular/input.ts", "toolCallRestrictedGranularInput", super::tool_call_restricted_granular::input::tool_call_restricted_granular_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/tool_call_workspace_never/input.ts", "toolCallWorkspaceNeverInput", super::tool_call_workspace_never::input::tool_call_workspace_never_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/turn_interrupt/input.ts", "turnInterruptInput", super::turn_interrupt::input::turn_interrupt_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/turn_interrupt_mid_tool/input.ts", "turnInterruptMidToolInput", super::turn_interrupt_mid_tool::input::turn_interrupt_mid_tool_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/turn_interrupt_mid_tool/input.ts", "turnInterruptMidToolKiroInput", super::turn_interrupt_mid_tool::input::turn_interrupt_mid_tool_kiro_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/turn_interrupt_restart/input.ts", "turnInterruptRestartInput", super::turn_interrupt_restart::input::turn_interrupt_restart_input),
    ("apps/server/src/orchestration-v2/testkit/fixtures/web_search/input.ts", "webSearchInput", super::web_search::input::web_search_input),
];

pub const INPUT_EXPORTS: &[super::shared::FixtureInputExport] = &[
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/acp_titled_multiselect/input.ts",
        name: "ACP_TITLED_MULTISELECT_PROMPT",
        evaluate: || {
            Ok(serde_json::json!(
                super::acp_titled_multiselect::input::ACP_TITLED_MULTISELECT_PROMPT
            ))
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/acp_titled_multiselect/input.ts",
        name: "acpTitledMultiSelectInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::acp_titled_multiselect::input::acp_titled_multi_select_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/command_output_streaming/input.ts",
        name: "commandOutputStreamingInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::command_output_streaming::input::command_output_streaming_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/fx_early_stop/input.ts",
        name: "fxEarlyStopInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::fx_early_stop::input::fx_early_stop_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/fx_subagent/input.ts",
        name: "fxSubagentInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::fx_subagent::input::fx_subagent_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/grok_auto_blocked_command/input.ts",
        name: "GROK_AUTO_BLOCKED_COMMAND_PROMPT",
        evaluate: || {
            Ok(serde_json::json!(
                super::grok_auto_blocked_command::input::GROK_AUTO_BLOCKED_COMMAND_PROMPT
            ))
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/grok_auto_blocked_command/input.ts",
        name: "GROK_AUTO_BLOCKED_PATH",
        evaluate: || {
            Ok(serde_json::json!(
                super::grok_auto_blocked_command::input::GROK_AUTO_BLOCKED_PATH
            ))
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/grok_auto_blocked_command/input.ts",
        name: "GROK_AUTO_PENDING_SHELL_KEY",
        evaluate: || {
            Ok(serde_json::json!(
                super::grok_auto_blocked_command::input::GROK_AUTO_PENDING_SHELL_KEY
            ))
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/grok_auto_blocked_command/input.ts",
        name: "GROK_AUTO_ROUTINE_MARKER",
        evaluate: || {
            Ok(serde_json::json!(
                super::grok_auto_blocked_command::input::GROK_AUTO_ROUTINE_MARKER
            ))
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/grok_auto_blocked_command/input.ts",
        name: "grokAutoBlockedCommandInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::grok_auto_blocked_command::input::grok_auto_blocked_command_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/grok_background_bash/input.ts",
        name: "GROK_BACKGROUND_BASH_PROMPT",
        evaluate: || {
            Ok(serde_json::json!(
                super::grok_background_bash::input::GROK_BACKGROUND_BASH_PROMPT
            ))
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/grok_background_bash/input.ts",
        name: "GROK_BACKGROUND_BASH_TICKS",
        evaluate: || {
            Ok(serde_json::json!(
                super::grok_background_bash::input::GROK_BACKGROUND_BASH_TICKS
            ))
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/grok_background_bash/input.ts",
        name: "grokBackgroundBashInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::grok_background_bash::input::grok_background_bash_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/grok_background_bash_fast_wake/input.ts",
        name: "grokBackgroundBashFastWakeInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::grok_background_bash_fast_wake::input::grok_background_bash_fast_wake_input(
                )?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/grok_background_subagent/input.ts",
        name: "GROK_BACKGROUND_SUBAGENT_PROMPT",
        evaluate: || {
            Ok(serde_json::json!(
                super::grok_background_subagent::input::GROK_BACKGROUND_SUBAGENT_PROMPT
            ))
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/grok_background_subagent/input.ts",
        name: "grokBackgroundSubagentInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::grok_background_subagent::input::grok_background_subagent_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/grok_monitor/input.ts",
        name: "GROK_MONITOR_PROMPT",
        evaluate: || {
            Ok(serde_json::json!(
                super::grok_monitor::input::GROK_MONITOR_PROMPT
            ))
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/grok_monitor/input.ts",
        name: "GROK_MONITOR_TICKS",
        evaluate: || {
            Ok(serde_json::json!(
                super::grok_monitor::input::GROK_MONITOR_TICKS
            ))
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/grok_monitor/input.ts",
        name: "grokMonitorInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::grok_monitor::input::grok_monitor_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/grok_prompt_error/input.ts",
        name: "GROK_PROMPT_ERROR_FOLLOW_UP",
        evaluate: || {
            Ok(serde_json::json!(
                super::grok_prompt_error::input::GROK_PROMPT_ERROR_FOLLOW_UP
            ))
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/grok_prompt_error/input.ts",
        name: "GROK_PROMPT_ERROR_PROMPT",
        evaluate: || {
            Ok(serde_json::json!(
                super::grok_prompt_error::input::GROK_PROMPT_ERROR_PROMPT
            ))
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/grok_prompt_error/input.ts",
        name: "grokPromptErrorInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::grok_prompt_error::input::grok_prompt_error_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/grok_subagent_lineage/input.ts",
        name: "grokSubagentLineageInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::grok_subagent_lineage::input::grok_subagent_lineage_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/kiro_compaction/input.ts",
        name: "KIRO_COMPACTION_MARKER",
        evaluate: || {
            Ok(serde_json::json!(
                super::kiro_compaction::input::KIRO_COMPACTION_MARKER
            ))
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/kiro_compaction/input.ts",
        name: "kiroCompactionInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::kiro_compaction::input::kiro_compaction_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/kiro_effort/input.ts",
        name: "KIRO_EFFORT_LEVEL",
        evaluate: || {
            Ok(serde_json::json!(
                super::kiro_effort::input::KIRO_EFFORT_LEVEL
            ))
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/kiro_effort/input.ts",
        name: "KIRO_EFFORT_MODEL",
        evaluate: || {
            Ok(serde_json::json!(
                super::kiro_effort::input::KIRO_EFFORT_MODEL
            ))
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/kiro_effort/input.ts",
        name: "KIRO_EFFORT_PROMPT",
        evaluate: || {
            Ok(serde_json::json!(
                super::kiro_effort::input::KIRO_EFFORT_PROMPT
            ))
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/kiro_effort/input.ts",
        name: "kiroEffortInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::kiro_effort::input::kiro_effort_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/kiro_fork_from_turn/input.ts",
        name: "kiroForkFromTurnInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::kiro_fork_from_turn::input::kiro_fork_from_turn_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/kiro_message_steering/input.ts",
        name: "KIRO_MESSAGE_STEERING_PROMPT",
        evaluate: || {
            Ok(serde_json::json!(
                super::kiro_message_steering::input::KIRO_MESSAGE_STEERING_PROMPT
            ))
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/kiro_message_steering/input.ts",
        name: "kiroMessageSteeringInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::kiro_message_steering::input::kiro_message_steering_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/kiro_model_switch/input.ts",
        name: "KIRO_MODEL_SWITCH_FIRST_PROMPT",
        evaluate: || {
            Ok(serde_json::json!(
                super::kiro_model_switch::input::KIRO_MODEL_SWITCH_FIRST_PROMPT
            ))
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/kiro_model_switch/input.ts",
        name: "KIRO_MODEL_SWITCH_SECOND_PROMPT",
        evaluate: || {
            Ok(serde_json::json!(
                super::kiro_model_switch::input::KIRO_MODEL_SWITCH_SECOND_PROMPT
            ))
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/kiro_model_switch/input.ts",
        name: "KIRO_MODEL_SWITCH_TARGET",
        evaluate: || {
            Ok(serde_json::json!(
                super::kiro_model_switch::input::KIRO_MODEL_SWITCH_TARGET
            ))
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/kiro_model_switch/input.ts",
        name: "kiroModelSwitchInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::kiro_model_switch::input::kiro_model_switch_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/kiro_permission_modes/input.ts",
        name: "KIRO_PERMISSION_PROMPT",
        evaluate: || {
            Ok(serde_json::json!(
                super::kiro_permission_modes::input::KIRO_PERMISSION_PROMPT
            ))
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/kiro_permission_modes/input.ts",
        name: "kiroFullAccessWriteInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::kiro_permission_modes::input::kiro_full_access_write_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/kiro_permission_modes/input.ts",
        name: "kiroSupervisedWriteInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::kiro_permission_modes::input::kiro_supervised_write_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/kiro_subagent_interrupt/input.ts",
        name: "kiroSubagentInterruptInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::kiro_subagent_interrupt::input::kiro_subagent_interrupt_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/malformed_session_update/input.ts",
        name: "MALFORMED_SESSION_UPDATE_PROMPT",
        evaluate: || {
            Ok(serde_json::json!(
                super::malformed_session_update::input::MALFORMED_SESSION_UPDATE_PROMPT
            ))
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/malformed_session_update/input.ts",
        name: "malformedSessionUpdateInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::malformed_session_update::input::malformed_session_update_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/message_steering/input.ts",
        name: "messageRestartInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::message_steering::input::message_restart_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/message_steering/input.ts",
        name: "messageSteeringInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::message_steering::input::message_steering_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/multi_turn/input.ts",
        name: "multiTurnInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::multi_turn::input::multi_turn_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/muse_permission/input.ts",
        name: "MUSE_PERMISSION_APPROVED_FILE",
        evaluate: || {
            Ok(serde_json::json!(
                super::muse_permission::input::MUSE_PERMISSION_APPROVED_FILE
            ))
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/muse_permission/input.ts",
        name: "MUSE_PERMISSION_DECLINED_FILE",
        evaluate: || {
            Ok(serde_json::json!(
                super::muse_permission::input::MUSE_PERMISSION_DECLINED_FILE
            ))
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/muse_permission/input.ts",
        name: "musePermissionInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::muse_permission::input::muse_permission_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/muse_subagent_approval/input.ts",
        name: "museSubagentApprovalInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::muse_subagent_approval::input::muse_subagent_approval_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/muse_subagent_interrupt/input.ts",
        name: "museSubagentInterruptInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::muse_subagent_interrupt::input::muse_subagent_interrupt_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/muse_workflow/input.ts",
        name: "museWorkflowInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::muse_workflow::input::muse_workflow_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/opencode2_background/input.ts",
        name: "openCode2BackgroundInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::opencode2_background::input::open_code2_background_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/opencode2_command/input.ts",
        name: "openCode2CommandInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::opencode2_command::input::open_code2_command_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/opencode2_compaction/input.ts",
        name: "openCode2CompactionInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::opencode2_compaction::input::open_code2_compaction_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/opencode2_inbox/input.ts",
        name: "openCode2InboxInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::opencode2_inbox::input::open_code2_inbox_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/opencode2_interrupt/input.ts",
        name: "openCode2InterruptInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::opencode2_interrupt::input::open_code2_interrupt_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/opencode2_nested_background/input.ts",
        name: "openCode2NestedBackgroundInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::opencode2_nested_background::input::open_code2_nested_background_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/opencode2_permission/input.ts",
        name: "openCode2PermissionInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::opencode2_permission::input::open_code2_permission_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/opencode2_question/input.ts",
        name: "openCode2QuestionInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::opencode2_question::input::open_code2_question_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/opencode2_resume_after_restart/input.ts",
        name: "openCode2ResumeAfterRestartInput",
        evaluate: || {
            Ok(serde_json::to_value(super::opencode2_resume_after_restart::input::open_code2_resume_after_restart_input()?)?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/opencode2_revert/input.ts",
        name: "openCode2RevertInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::opencode2_revert::input::open_code2_revert_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/opencode2_simple/input.ts",
        name: "openCode2SimpleInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::opencode2_simple::input::open_code2_simple_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/opencode2_skill/input.ts",
        name: "openCode2SkillInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::opencode2_skill::input::open_code2_skill_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/opencode2_subagent/input.ts",
        name: "openCode2SubagentInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::opencode2_subagent::input::open_code2_subagent_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/opencode2_tool_call/input.ts",
        name: "openCode2ToolCallInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::opencode2_tool_call::input::open_code2_tool_call_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/opencode_child_approval/input.ts",
        name: "openCodeChildApprovalInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::opencode_child_approval::input::open_code_child_approval_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/opencode_running_child_approval/input.ts",
        name: "OPENCODE_RUNNING_CHILD_APPROVAL_PENDING_SHELL_KEY",
        evaluate: || {
            Ok(serde_json::json!(super::opencode_running_child_approval::input::OPENCODE_RUNNING_CHILD_APPROVAL_PENDING_SHELL_KEY))
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/opencode_running_child_approval/input.ts",
        name: "openCodeRunningChildApprovalInput",
        evaluate: || {
            Ok(serde_json::to_value(super::opencode_running_child_approval::input::open_code_running_child_approval_input()?)?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/opencode_subagent/input.ts",
        name: "openCodeSubagentInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::opencode_subagent::input::open_code_subagent_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/pi_compaction/input.ts",
        name: "PI_COMPACTION_MARKER",
        evaluate: || {
            Ok(serde_json::json!(
                super::pi_compaction::input::PI_COMPACTION_MARKER
            ))
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/pi_compaction/input.ts",
        name: "piCompactionInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::pi_compaction::input::pi_compaction_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/plan_questions/input.ts",
        name: "planQuestionsInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::plan_questions::input::plan_questions_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/proposed_plan/input.ts",
        name: "proposedPlanInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::proposed_plan::input::proposed_plan_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/provider_thread_resume/input.ts",
        name: "providerThreadResumeInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::provider_thread_resume::input::provider_thread_resume_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/queued_cancelled_while_active/input.ts",
        name: "ACTIVE_RUN_SHELL_SNAPSHOT_KEY",
        evaluate: || {
            Ok(serde_json::json!(
                super::queued_cancelled_while_active::input::ACTIVE_RUN_SHELL_SNAPSHOT_KEY
            ))
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/queued_cancelled_while_active/input.ts",
        name: "queuedCancelledWhileActiveInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::queued_cancelled_while_active::input::queued_cancelled_while_active_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/queued_turn/input.ts",
        name: "queuedTurnInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::queued_turn::input::queued_turn_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/simple/input.ts",
        name: "simpleInput",
        evaluate: || Ok(serde_json::to_value(super::simple::input::simple_input()?)?),
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/skill_invocation/input.ts",
        name: "SKILL_INVOCATION_FINAL",
        evaluate: || {
            Ok(serde_json::json!(
                super::skill_invocation::input::SKILL_INVOCATION_FINAL
            ))
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/skill_invocation/input.ts",
        name: "skillInvocationInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::skill_invocation::input::skill_invocation_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/stop_background_work_after_failed_turn/input.ts",
        name: "STOP_BACKGROUND_WORK_AFTER_FAILED_TURN_PROMPT",
        evaluate: || {
            Ok(serde_json::json!(super::stop_background_work_after_failed_turn::input::STOP_BACKGROUND_WORK_AFTER_FAILED_TURN_PROMPT))
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/stop_background_work_after_failed_turn/input.ts",
        name: "stopBackgroundWorkAfterFailedTurnInput",
        evaluate: || {
            Ok(serde_json::to_value(super::stop_background_work_after_failed_turn::input::stop_background_work_after_failed_turn_input()?)?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/stop_background_work_after_failed_turn/input.ts",
        name: "stopBackgroundWorkAfterReleaseInput",
        evaluate: || {
            Ok(serde_json::to_value(super::stop_background_work_after_failed_turn::input::stop_background_work_after_release_input()?)?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/subagent/input.ts",
        name: "subagentInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::subagent::input::subagent_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/subagent_continue/input.ts",
        name: "subagentContinueInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::subagent_continue::input::subagent_continue_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/subagent_v2/input.ts",
        name: "subagentV2Input",
        evaluate: || {
            Ok(serde_json::to_value(
                super::subagent_v2::input::subagent_v2_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/subagent_v2/input.ts",
        name: "subagentV2NestedInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::subagent_v2::input::subagent_v2_nested_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/subagent_v2_approval/input.ts",
        name: "SUBAGENT_APPROVAL_PENDING_SHELL_KEY",
        evaluate: || {
            Ok(serde_json::json!(
                super::subagent_v2_approval::input::SUBAGENT_APPROVAL_PENDING_SHELL_KEY
            ))
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/subagent_v2_approval/input.ts",
        name: "SUBAGENT_V2_APPROVAL_POLICY",
        evaluate: || Ok(super::subagent_v2_approval::input::subagent_v2_approval_policy()),
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/subagent_v2_approval/input.ts",
        name: "subagentV2ApprovalInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::subagent_v2_approval::input::subagent_v2_approval_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/subagent_v2_nested_approval/input.ts",
        name: "NESTED_SUBAGENT_APPROVAL_PENDING_SHELL_KEY",
        evaluate: || {
            Ok(serde_json::json!(super::subagent_v2_nested_approval::input::NESTED_SUBAGENT_APPROVAL_PENDING_SHELL_KEY))
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/subagent_v2_nested_approval/input.ts",
        name: "subagentV2NestedApprovalInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::subagent_v2_nested_approval::input::subagent_v2_nested_approval_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/thread_rollback/input.ts",
        name: "threadRollbackInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::thread_rollback::input::thread_rollback_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/thread_rollback_after_restart/input.ts",
        name: "threadRollbackAfterRestartInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::thread_rollback_after_restart::input::thread_rollback_after_restart_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/thread_rollback_after_stop/input.ts",
        name: "threadRollbackAfterStopInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::thread_rollback_after_stop::input::thread_rollback_after_stop_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/thread_rollback_to_stopped_turn/input.ts",
        name: "threadRollbackToStoppedTurnInput",
        evaluate: || {
            Ok(serde_json::to_value(super::thread_rollback_to_stopped_turn::input::thread_rollback_to_stopped_turn_input()?)?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/todo_list/input.ts",
        name: "todoListInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::todo_list::input::todo_list_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/tool_call_denied_write/input.ts",
        name: "DENIED_WRITE_POLICY",
        evaluate: || Ok(super::tool_call_denied_write::input::denied_write_policy()),
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/tool_call_denied_write/input.ts",
        name: "TOOL_CALL_DENIED_WRITE_PROMPT",
        evaluate: || {
            Ok(serde_json::json!(
                super::tool_call_denied_write::input::TOOL_CALL_DENIED_WRITE_PROMPT
            ))
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/tool_call_denied_write/input.ts",
        name: "TOOL_CALL_DENIED_WRITE_TARGET",
        evaluate: || {
            Ok(serde_json::json!(
                super::tool_call_denied_write::input::TOOL_CALL_DENIED_WRITE_TARGET
            ))
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/tool_call_denied_write/input.ts",
        name: "toolCallDeniedWriteInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::tool_call_denied_write::input::tool_call_denied_write_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/tool_call_read_only/input.ts",
        name: "toolCallReadOnlyInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::tool_call_read_only::input::tool_call_read_only_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/tool_call_read_only_on_request/input.ts",
        name: "toolCallReadOnlyOnRequestInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::tool_call_read_only_on_request::input::tool_call_read_only_on_request_input(
                )?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/tool_call_read_only_on_request/input.ts",
        name: "toolCallReadOnlyOnRequestKiroInput",
        evaluate: || {
            Ok(serde_json::to_value(super::tool_call_read_only_on_request::input::tool_call_read_only_on_request_kiro_input()?)?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/tool_call_restricted_granular/input.ts",
        name: "toolCallRestrictedGranularInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::tool_call_restricted_granular::input::tool_call_restricted_granular_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/tool_call_workspace_never/input.ts",
        name: "toolCallWorkspaceNeverInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::tool_call_workspace_never::input::tool_call_workspace_never_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/turn_interrupt/input.ts",
        name: "turnInterruptInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::turn_interrupt::input::turn_interrupt_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/turn_interrupt_mid_tool/input.ts",
        name: "turnInterruptMidToolInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::turn_interrupt_mid_tool::input::turn_interrupt_mid_tool_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/turn_interrupt_mid_tool/input.ts",
        name: "turnInterruptMidToolKiroInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::turn_interrupt_mid_tool::input::turn_interrupt_mid_tool_kiro_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/turn_interrupt_restart/input.ts",
        name: "turnInterruptRestartInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::turn_interrupt_restart::input::turn_interrupt_restart_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/web_search/input.ts",
        name: "webSearchInput",
        evaluate: || {
            Ok(serde_json::to_value(
                super::web_search::input::web_search_input()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "ACP_REGISTRY_MODEL_SELECTION",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::acp_registry_model_selection()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "CLAUDE_LOCAL_BASH_TASK_PROMPT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::CLAUDE_LOCAL_BASH_TASK_PROMPT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "CLAUDE_MODEL_SELECTION",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::claude_model_selection()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "CLAUDE_RESULT_IS_ERROR_FOLLOW_UP",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::CLAUDE_RESULT_IS_ERROR_FOLLOW_UP,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "CLAUDE_RESULT_IS_ERROR_PROMPT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::CLAUDE_RESULT_IS_ERROR_PROMPT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "CODEX_MODEL_SELECTION",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::codex_model_selection()?
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "COMMAND_OUTPUT_STREAMING_PROMPT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::COMMAND_OUTPUT_STREAMING_PROMPT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "CURSOR_MODEL_SELECTION",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::cursor_model_selection()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "FX_MODEL_SELECTION",
        evaluate: || Ok(serde_json::to_value(super::shared::fx_model_selection()?)?),
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "GROK_MODEL_SELECTION",
        evaluate: || Ok(serde_json::to_value(super::shared::grok_model_selection()?)?),
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "KIRO_MODEL_SELECTION",
        evaluate: || Ok(serde_json::to_value(super::shared::kiro_model_selection()?)?),
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "MESSAGE_STEERING_INITIAL_PROMPT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::MESSAGE_STEERING_INITIAL_PROMPT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "MESSAGE_STEERING_STEER_PROMPT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::MESSAGE_STEERING_STEER_PROMPT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "MULTI_TURN_FIRST_PROMPT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::MULTI_TURN_FIRST_PROMPT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "MULTI_TURN_SECOND_PROMPT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::MULTI_TURN_SECOND_PROMPT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "MUSE_MODEL_SELECTION",
        evaluate: || Ok(serde_json::to_value(super::shared::muse_model_selection()?)?),
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "OPENCODE2_BACKGROUND_PROMPT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::OPENCODE2_BACKGROUND_PROMPT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "OPENCODE2_CANCELLED_PROMPT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::OPENCODE2_CANCELLED_PROMPT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "OPENCODE2_COMMAND_PROMPT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::OPENCODE2_COMMAND_PROMPT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "OPENCODE2_COMPACTION_FIRST_PROMPT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::OPENCODE2_COMPACTION_FIRST_PROMPT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "OPENCODE2_COMPACTION_RECALL_PROMPT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::OPENCODE2_COMPACTION_RECALL_PROMPT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "OPENCODE2_INTERRUPT_PROMPT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::OPENCODE2_INTERRUPT_PROMPT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "OPENCODE2_MODEL_SELECTION",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::opencode2_model_selection()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "OPENCODE2_NESTED_BACKGROUND_PROMPT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::OPENCODE2_NESTED_BACKGROUND_PROMPT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "OPENCODE2_PERMISSION_PROMPT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::OPENCODE2_PERMISSION_PROMPT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "OPENCODE2_QUESTION_PROMPT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::OPENCODE2_QUESTION_PROMPT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "OPENCODE2_QUEUED_PROMPT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::OPENCODE2_QUEUED_PROMPT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "OPENCODE2_RESTART_PROMPT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::OPENCODE2_RESTART_PROMPT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "OPENCODE2_RESTART_RECALL_PROMPT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::OPENCODE2_RESTART_RECALL_PROMPT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "OPENCODE2_REVERT_FIRST_PROMPT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::OPENCODE2_REVERT_FIRST_PROMPT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "OPENCODE2_REVERT_SECOND_PROMPT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::OPENCODE2_REVERT_SECOND_PROMPT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "OPENCODE2_SIMPLE_PROMPT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::OPENCODE2_SIMPLE_PROMPT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "OPENCODE2_SKILL_PROMPT",
        evaluate: || Ok(serde_json::to_value(super::shared::OPENCODE2_SKILL_PROMPT)?),
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "OPENCODE2_STEER_PROMPT",
        evaluate: || Ok(serde_json::to_value(super::shared::OPENCODE2_STEER_PROMPT)?),
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "OPENCODE2_STEER_TEXT",
        evaluate: || Ok(serde_json::to_value(super::shared::OPENCODE2_STEER_TEXT)?),
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "OPENCODE2_SUBAGENT_PROMPT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::OPENCODE2_SUBAGENT_PROMPT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "OPENCODE2_TOOL_CALL_PROMPT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::OPENCODE2_TOOL_CALL_PROMPT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "OPENCODE_MODEL_SELECTION",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::opencode_model_selection()?,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "OPENCODE_SUBAGENT_PROMPT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::OPENCODE_SUBAGENT_PROMPT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "PI_MODEL_SELECTION",
        evaluate: || Ok(serde_json::to_value(super::shared::pi_model_selection()?)?),
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "PLAN_QUESTIONS_PROMPT",
        evaluate: || Ok(serde_json::to_value(super::shared::PLAN_QUESTIONS_PROMPT)?),
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "PROPOSED_PLAN_PROMPT",
        evaluate: || Ok(serde_json::to_value(super::shared::PROPOSED_PLAN_PROMPT)?),
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "PROVIDER_THREAD_RESUME_FIRST_PROMPT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::PROVIDER_THREAD_RESUME_FIRST_PROMPT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "PROVIDER_THREAD_RESUME_SECOND_PROMPT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::PROVIDER_THREAD_RESUME_SECOND_PROMPT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "READ_ONLY_NEVER_POLICY",
        evaluate: || Ok(super::shared::read_only_never_policy()),
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "READ_ONLY_ON_REQUEST_POLICY",
        evaluate: || Ok(super::shared::read_only_on_request_policy()),
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "RESTRICTED_GRANULAR_POLICY",
        evaluate: || Ok(super::shared::restricted_granular_policy()),
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "SIMPLE_PROMPT",
        evaluate: || Ok(serde_json::to_value(super::shared::SIMPLE_PROMPT)?),
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "SKILL_INVOCATION_CURSOR_MESSAGE",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::SKILL_INVOCATION_CURSOR_MESSAGE,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "SKILL_INVOCATION_PROMPT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::SKILL_INVOCATION_PROMPT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "SUBAGENT_CONTINUE_CHILD_PROMPT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::SUBAGENT_CONTINUE_CHILD_PROMPT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "SUBAGENT_CONTINUE_PARENT_PROMPT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::SUBAGENT_CONTINUE_PARENT_PROMPT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "SUBAGENT_CONTINUE_PROMPT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::SUBAGENT_CONTINUE_PROMPT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "SUBAGENT_PROMPT",
        evaluate: || Ok(serde_json::to_value(super::shared::SUBAGENT_PROMPT)?),
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "SUBAGENT_V2_APPROVAL_PROMPT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::SUBAGENT_V2_APPROVAL_PROMPT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "SUBAGENT_V2_NESTED_APPROVAL_PROMPT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::SUBAGENT_V2_NESTED_APPROVAL_PROMPT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "SUBAGENT_V2_NESTED_PROMPT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::SUBAGENT_V2_NESTED_PROMPT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "SUBAGENT_V2_PROMPT",
        evaluate: || Ok(serde_json::to_value(super::shared::SUBAGENT_V2_PROMPT)?),
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "THREAD_FORK_NATIVE_PRIOR_TURN_ALPHA_PROMPT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::THREAD_FORK_NATIVE_PRIOR_TURN_ALPHA_PROMPT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "THREAD_FORK_NATIVE_PRIOR_TURN_BETA_PROMPT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::THREAD_FORK_NATIVE_PRIOR_TURN_BETA_PROMPT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "THREAD_FORK_NATIVE_PRIOR_TURN_REPEAT_PROMPT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::THREAD_FORK_NATIVE_PRIOR_TURN_REPEAT_PROMPT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "THREAD_FORK_NATIVE_SOURCE_PROMPT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::THREAD_FORK_NATIVE_SOURCE_PROMPT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "THREAD_FORK_NATIVE_TARGET_PROMPT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::THREAD_FORK_NATIVE_TARGET_PROMPT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "THREAD_MERGE_BACK_SIBLINGS_FIRST_FORK_PROMPT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::THREAD_MERGE_BACK_SIBLINGS_FIRST_FORK_PROMPT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "THREAD_MERGE_BACK_SIBLINGS_FIRST_HANDOFF_PROMPT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::THREAD_MERGE_BACK_SIBLINGS_FIRST_HANDOFF_PROMPT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "THREAD_MERGE_BACK_SIBLINGS_RECALL",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::THREAD_MERGE_BACK_SIBLINGS_RECALL,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "THREAD_MERGE_BACK_SIBLINGS_RECALL_PROMPT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::THREAD_MERGE_BACK_SIBLINGS_RECALL_PROMPT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "THREAD_MERGE_BACK_SIBLINGS_SECOND_FORK_PROMPT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::THREAD_MERGE_BACK_SIBLINGS_SECOND_FORK_PROMPT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "THREAD_MERGE_BACK_SIBLINGS_SECOND_HANDOFF_PROMPT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::THREAD_MERGE_BACK_SIBLINGS_SECOND_HANDOFF_PROMPT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "THREAD_MERGE_BACK_SIBLINGS_SOURCE_PROMPT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::THREAD_MERGE_BACK_SIBLINGS_SOURCE_PROMPT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "THREAD_ROLLBACK_AFTER_PROMPT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::THREAD_ROLLBACK_AFTER_PROMPT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "THREAD_ROLLBACK_FIRST_PROMPT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::THREAD_ROLLBACK_FIRST_PROMPT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "THREAD_ROLLBACK_SECOND_PROMPT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::THREAD_ROLLBACK_SECOND_PROMPT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "TODO_LIST_PROMPT",
        evaluate: || Ok(serde_json::to_value(super::shared::TODO_LIST_PROMPT)?),
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "TOOL_CALL_READ_ONLY_PROMPT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::TOOL_CALL_READ_ONLY_PROMPT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "TOOL_CALL_READ_ONLY_WORKSPACE_ROOT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::TOOL_CALL_READ_ONLY_WORKSPACE_ROOT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "TOOL_CALL_WRITE_PROMPT",
        evaluate: || Ok(serde_json::to_value(super::shared::TOOL_CALL_WRITE_PROMPT)?),
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "TURN_INTERRUPT_MID_TOOL_PROMPT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::TURN_INTERRUPT_MID_TOOL_PROMPT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "TURN_INTERRUPT_PROMPT",
        evaluate: || Ok(serde_json::to_value(super::shared::TURN_INTERRUPT_PROMPT)?),
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "TURN_INTERRUPT_RECOVERY_PROMPT",
        evaluate: || {
            Ok(serde_json::to_value(
                super::shared::TURN_INTERRUPT_RECOVERY_PROMPT,
            )?)
        },
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "WEB_SEARCH_PROMPT",
        evaluate: || Ok(serde_json::to_value(super::shared::WEB_SEARCH_PROMPT)?),
    },
    super::shared::FixtureInputExport {
        source: "apps/server/src/orchestration-v2/testkit/fixtures/shared.ts",
        name: "WORKSPACE_NEVER_POLICY",
        evaluate: || Ok(super::shared::workspace_never_policy()),
    },
];

pub fn replay_fixture_definitions()
-> crate::error::Result<Vec<super::shared::OrchestratorReplayFixture>> {
    Ok(serde_json::from_str(
        r###"[{"name":"claude_background_subagent_after_root","buildInput":"claudeBackgroundSubagentAfterRootInput","providers":[{"driver":"claudeAgent","transcriptFile":"claude_background_subagent_after_root/claude_transcript.ndjson","modelSelection":{"instanceId":"claudeAgent","model":"claude-sonnet-4-6"},"runContinuationWorker":true,"assertOutput":"assertClaudeBackgroundSubagentAfterRootOutput"}]},{"name":"claude_background_task_after_root","buildInput":"claudeBackgroundTaskAfterRootInput","providers":[{"driver":"claudeAgent","transcriptFile":"claude_background_task_after_root/claude_transcript.ndjson","modelSelection":{"instanceId":"claudeAgent","model":"claude-sonnet-4-6"},"runContinuationWorker":true,"assertOutput":"assertClaudeBackgroundTaskAfterRootOutput"}]},{"name":"claude_background_subagent_lifecycle","buildInput":"claudeBackgroundSubagentLifecycleInput","providers":[{"driver":"claudeAgent","transcriptFile":"claude_background_subagent_lifecycle/claude_transcript.ndjson","modelSelection":{"instanceId":"claudeAgent","model":"claude-sonnet-4-6"},"runContinuationWorker":true,"assertOutput":"assertClaudeBackgroundSubagentLifecycleOutput"}]},{"name":"claude_background_task_interrupt","buildInput":"claudeBackgroundTaskInterruptInput","providers":[{"driver":"claudeAgent","transcriptFile":"claude_background_task_interrupt/claude_transcript.ndjson","modelSelection":{"instanceId":"claudeAgent","model":"claude-sonnet-4-6"},"runContinuationWorker":true,"assertOutput":"assertClaudeBackgroundTaskInterruptOutput"}]},{"name":"claude_background_wake_before_queued_prompt","buildInput":"claudeBackgroundWakeBeforeQueuedPromptInput","providers":[{"driver":"claudeAgent","transcriptFile":"claude_background_wake_before_queued_prompt/claude_transcript.ndjson","modelSelection":{"instanceId":"claudeAgent","model":"claude-sonnet-4-6"},"runContinuationWorker":true,"assertOutput":"assertClaudeBackgroundWakeBeforeQueuedPromptOutput"}]},{"name":"claude_background_wake_before_queued_prompt_no_echo","buildInput":"claudeBackgroundWakeBeforeQueuedPromptNoEchoInput","providers":[{"driver":"claudeAgent","transcriptFile":"claude_background_wake_before_queued_prompt_no_echo/claude_transcript.ndjson","modelSelection":{"instanceId":"claudeAgent","model":"claude-sonnet-4-6"},"runContinuationWorker":true,"assertOutput":"assertClaudeBackgroundWakeBeforeQueuedPromptNoEchoOutput"}]},{"name":"claude_compact_after_resume_wake","buildInput":"claudeCompactAfterResumeWakeInput","providers":[{"driver":"claudeAgent","transcriptFile":"claude_compact_after_resume_wake/claude_transcript.ndjson","modelSelection":{"instanceId":"claudeAgent","model":"claude-sonnet-4-6"},"runContinuationWorker":true,"assertOutput":"assertClaudeCompactAfterResumeWakeOutput"}]},{"name":"claude_compact_after_peer_turn","buildInput":"claudeCompactAfterPeerTurnInput","providers":[{"driver":"claudeAgent","transcriptFile":"claude_compact_after_peer_turn/claude_transcript.ndjson","modelSelection":{"instanceId":"claudeAgent","model":"claude-sonnet-4-6"},"runContinuationWorker":true,"assertOutput":"assertClaudeCompactAfterPeerTurnOutput"}]},{"name":"claude_compact_after_peer_turn_no_echo","buildInput":"claudeCompactAfterPeerTurnNoEchoInput","providers":[{"driver":"claudeAgent","transcriptFile":"claude_compact_after_peer_turn_no_echo/claude_transcript.ndjson","modelSelection":{"instanceId":"claudeAgent","model":"claude-sonnet-4-6"},"runContinuationWorker":true,"assertOutput":"assertClaudeCompactAfterPeerTurnNoEchoOutput"}]},{"name":"claude_background_monitor_wake","buildInput":"claudeBackgroundMonitorWakeInput","providers":[{"driver":"claudeAgent","transcriptFile":"claude_background_monitor_wake/claude_transcript.ndjson","modelSelection":{"instanceId":"claudeAgent","model":"claude-sonnet-4-6"},"runContinuationWorker":true,"assertOutput":"assertClaudeBackgroundMonitorWakeOutput"}]},{"name":"claude_background_task_wake","buildInput":"claudeBackgroundTaskWakeInput","providers":[{"driver":"claudeAgent","transcriptFile":"claude_background_task_wake/claude_transcript.ndjson","modelSelection":{"instanceId":"claudeAgent","model":"claude-sonnet-4-6"},"runContinuationWorker":true,"assertOutput":"assertClaudeBackgroundTaskWakeOutput"}]},{"name":"claude_local_bash_task","buildInput":"claudeLocalBashTaskInput","providers":[{"driver":"claudeAgent","transcriptFile":"claude_local_bash_task/claude_transcript.ndjson","modelSelection":{"instanceId":"claudeAgent","model":"claude-sonnet-4-6"},"assertOutput":"assertClaudeLocalBashTaskOutput"}]},{"name":"claude_nested_background_subagent_wake","buildInput":"claudeNestedBackgroundSubagentWakeInput","providers":[{"driver":"claudeAgent","transcriptFile":"claude_nested_background_subagent_wake/claude_transcript.ndjson","modelSelection":{"instanceId":"claudeAgent","model":"claude-sonnet-4-6"},"runContinuationWorker":true,"assertOutput":"assertClaudeNestedBackgroundSubagentWakeOutput"}]},{"name":"claude_nested_subagent_model","buildInput":"claudeNestedSubagentModelInput","providers":[{"driver":"claudeAgent","transcriptFile":"claude_nested_subagent_model/claude_transcript.ndjson","modelSelection":{"instanceId":"claudeAgent","model":"claude-sonnet-4-6"},"assertOutput":"assertClaudeNestedSubagentModelOutput"}]},{"name":"claude_mcp_tool_presentation","buildInput":"claudeMcpToolPresentationInput","providers":[{"driver":"claudeAgent","transcriptFile":"claude_mcp_tool_presentation/claude_transcript.ndjson","modelSelection":{"instanceId":"claudeAgent","model":"claude-sonnet-4-6"},"assertOutput":"assertClaudeMcpToolPresentationOutput"}]},{"name":"claude_idle_resume","buildInput":"claudeIdleResumeInput","providers":[{"driver":"claudeAgent","transcriptFile":"claude_idle_resume/claude_transcript.ndjson","modelSelection":{"instanceId":"claudeAgent","model":"claude-sonnet-4-6"},"assertOutput":"assertClaudeIdleResumeOutput"}]},{"name":"claude_result_is_error","buildInput":"claudeResultIsErrorInput","providers":[{"driver":"claudeAgent","transcriptFile":"claude_result_is_error/claude_transcript.ndjson","modelSelection":{"instanceId":"claudeAgent","model":"claude-sonnet-4-6"},"assertOutput":"assertClaudeResultIsErrorOutput"}]},{"name":"grok_auto_blocked_command","buildInput":"grokAutoBlockedCommandInput","providers":[{"driver":"grok","transcriptFile":"grok_auto_blocked_command/grok_transcript.ndjson","modelSelection":{"instanceId":"grok","model":"grok-4.7-build-fast"},"assertOutput":"assertGrokAutoBlockedCommandOutput"}]},{"name":"kiro_tool_call_read_only_on_request","buildInput":"toolCallReadOnlyOnRequestKiroInput","providers":[{"driver":"kiro","transcriptFile":"tool_call_read_only_on_request/kiro_transcript.ndjson","modelSelection":{"instanceId":"kiro","model":"claude-haiku-4.5"},"runtimePolicyOverride":{"approvalPolicy":"on-request","sandboxPolicy":{"type":"readOnly","access":{"type":"fullAccess"},"networkAccess":false}},"assertOutput":"assertToolCallReadOnlyOnRequestKiroOutput"}]},{"name":"kiro_message_steering","buildInput":"kiroMessageSteeringInput","providers":[{"driver":"kiro","transcriptFile":"kiro_message_steering/kiro_transcript.ndjson","modelSelection":{"instanceId":"kiro","model":"claude-haiku-4.5"},"assertOutput":"assertKiroMessageSteeringOutput"}]},{"name":"kiro_fork_from_turn","buildInput":"kiroForkFromTurnInput","providers":[{"driver":"kiro","transcriptFile":"kiro_fork_from_turn/kiro_transcript.ndjson","modelSelection":{"instanceId":"kiro","model":"claude-haiku-4.5"},"assertOutput":"assertKiroForkFromTurnOutput"}]},{"name":"kiro_compaction","buildInput":"kiroCompactionInput","providers":[{"driver":"kiro","transcriptFile":"kiro_compaction/kiro_transcript.ndjson","modelSelection":{"instanceId":"kiro","model":"claude-haiku-4.5"},"assertOutput":"assertKiroCompactionOutput"}]},{"name":"kiro_model_switch","buildInput":"kiroModelSwitchInput","providers":[{"driver":"kiro","transcriptFile":"kiro_model_switch/kiro_transcript.ndjson","modelSelection":{"instanceId":"kiro","model":"claude-haiku-4.5"},"assertOutput":"assertKiroModelSwitchOutput"}]},{"name":"kiro_effort","buildInput":"kiroEffortInput","providers":[{"driver":"kiro","transcriptFile":"kiro_effort/kiro_transcript.ndjson","modelSelection":{"instanceId":"kiro","model":"claude-opus-5.5","options":[{"id":"reasoningEffort","value":"high"}]},"assertOutput":"assertKiroEffortOutput"}]},{"name":"kiro_cloud_config","buildInput":"turnInterruptInput","providers":[{"driver":"kiro","transcriptFile":"kiro_cloud_config/kiro_transcript.ndjson","modelSelection":{"instanceId":"kiro","model":"claude-haiku-4.5"},"runtimePolicyOverride":{"approvalPolicy":"never","sandboxPolicy":{"type":"workspaceWrite","writableRoots":[],"readOnlyAccess":{"type":"fullAccess"},"networkAccess":false}},"assertOutput":"assertTurnInterruptOutput"}]},{"name":"kiro_supervised_write","buildInput":"kiroSupervisedWriteInput","providers":[{"driver":"kiro","transcriptFile":"kiro_permission_modes/kiro_supervised_transcript.ndjson","modelSelection":{"instanceId":"kiro","model":"claude-haiku-4.5"},"assertOutput":"assertKiroSupervisedWriteOutput"}]},{"name":"kiro_full_access_write","buildInput":"kiroFullAccessWriteInput","providers":[{"driver":"kiro","transcriptFile":"kiro_permission_modes/kiro_full_access_transcript.ndjson","modelSelection":{"instanceId":"kiro","model":"claude-haiku-4.5"},"assertOutput":"assertKiroFullAccessWriteOutput"}]},{"name":"kiro_subagent_interrupt","buildInput":"kiroSubagentInterruptInput","providers":[{"driver":"kiro","transcriptFile":"kiro_subagent_interrupt/kiro_transcript.ndjson","modelSelection":{"instanceId":"kiro","model":"claude-haiku-4.5"},"assertOutput":"assertKiroSubagentInterruptOutput"}]},{"name":"grok_background_bash","buildInput":"grokBackgroundBashInput","providers":[{"driver":"grok","transcriptFile":"grok_background_bash/grok_transcript.ndjson","modelSelection":{"instanceId":"grok","model":"grok-4.7-build-fast"},"runContinuationWorker":true,"assertOutput":"assertGrokBackgroundBashOutput"}]},{"name":"grok_background_bash_fast_wake","buildInput":"grokBackgroundBashFastWakeInput","providers":[{"driver":"grok","transcriptFile":"grok_background_bash_fast_wake/grok_transcript.ndjson","modelSelection":{"instanceId":"grok","model":"grok-4.7-build-fast"},"runContinuationWorker":true,"assertOutput":"assertGrokBackgroundBashFastWakeOutput"}]},{"name":"grok_background_subagent","buildInput":"grokBackgroundSubagentInput","providers":[{"driver":"grok","transcriptFile":"grok_background_subagent/grok_transcript.ndjson","modelSelection":{"instanceId":"grok","model":"grok-build"},"runContinuationWorker":true,"assertOutput":"assertGrokBackgroundSubagentOutput"}]},{"name":"grok_monitor","buildInput":"grokMonitorInput","providers":[{"driver":"grok","transcriptFile":"grok_monitor/grok_transcript.ndjson","modelSelection":{"instanceId":"grok","model":"grok-build"},"runContinuationWorker":true,"assertOutput":"assertGrokMonitorOutput"}]},{"name":"grok_prompt_error","buildInput":"grokPromptErrorInput","providers":[{"driver":"grok","transcriptFile":"grok_prompt_error/grok_transcript.ndjson","modelSelection":{"instanceId":"grok","model":"grok-build"},"assertOutput":"assertGrokPromptErrorOutput"}]},{"name":"grok_subagent_lineage","buildInput":"grokSubagentLineageInput","providers":[{"driver":"grok","transcriptFile":"grok_subagent_lineage/grok_transcript.ndjson","modelSelection":{"instanceId":"grok","model":"grok-composer-2.5-fast"},"assertOutput":"assertGrokSubagentLineageOutput"}]},{"name":"acp_elicitation","buildInput":"planQuestionsInput","providers":[{"driver":"acpRegistry","transcriptFile":"acp_elicitation/registry_transcript.ndjson","modelSelection":{"instanceId":"acpRegistry","model":"default"},"runtimePolicyOverride":{"approvalPolicy":"never","sandboxPolicy":{"type":"readOnly","access":{"type":"fullAccess"},"networkAccess":false}},"assertOutput":"assertPlanQuestionsOutput"}]},{"name":"malformed_session_update","buildInput":"malformedSessionUpdateInput","providers":[{"driver":"acpRegistry","transcriptFile":"malformed_session_update/registry_transcript.ndjson","modelSelection":{"instanceId":"acpRegistry","model":"default"},"assertOutput":"assertMalformedSessionUpdateOutput"}]},{"name":"acp_titled_multiselect","buildInput":"acpTitledMultiSelectInput","providers":[{"driver":"acpRegistry","transcriptFile":"acp_titled_multiselect/registry_transcript.ndjson","modelSelection":{"instanceId":"acpRegistry","model":"default"},"runtimePolicyOverride":{"approvalPolicy":"never","sandboxPolicy":{"type":"readOnly","access":{"type":"fullAccess"},"networkAccess":false}},"assertOutput":"assertAcpTitledMultiSelectOutput"}]},{"name":"simple","buildInput":"simpleInput","providers":[{"driver":"codex","transcriptFile":"simple/codex_transcript.ndjson","modelSelection":{"instanceId":"codex","model":"gpt-6-luna"},"assertOutput":"assertSimpleOutput"},{"driver":"claudeAgent","transcriptFile":"simple/claude_transcript.ndjson","modelSelection":{"instanceId":"claudeAgent","model":"claude-sonnet-4-6"},"assertOutput":"assertSimpleClaudeOutput"},{"driver":"cursor","transcriptFile":"simple/cursor_transcript.ndjson","modelSelection":{"instanceId":"cursor","model":"composer-2.5"},"assertOutput":"assertSimpleOutput"},{"driver":"grok","transcriptFile":"simple/grok_transcript.ndjson","modelSelection":{"instanceId":"grok","model":"grok-build"},"assertOutput":"assertSimpleOutput"},{"driver":"fx","transcriptFile":"simple/fx_transcript.ndjson","modelSelection":{"instanceId":"fx","model":"default"},"assertOutput":"assertSimpleOutput"},{"driver":"kiro","transcriptFile":"simple/kiro_transcript.ndjson","modelSelection":{"instanceId":"kiro","model":"claude-haiku-4.5"},"assertOutput":"assertSimpleOutput"},{"driver":"acpRegistry","transcriptFile":"simple/registry_transcript.ndjson","modelSelection":{"instanceId":"acpRegistry","model":"default"},"assertOutput":"assertSimpleOutput"},{"driver":"opencode","transcriptFile":"simple/opencode_transcript.ndjson","modelSelection":{"instanceId":"opencode","model":"openai/gpt-5.4-mini","options":[{"id":"agent","value":"build"}]},"assertOutput":"assertSimpleOutput"},{"driver":"pi","transcriptFile":"simple/pi_transcript.ndjson","modelSelection":{"instanceId":"pi","model":"openrouter/deepseek/deepseek-v4-flash"},"assertOutput":"assertPiSimpleOutput"},{"driver":"muse","transcriptFile":"simple/muse_transcript.ndjson","modelSelection":{"instanceId":"muse","model":"muse-spark-1.3-contributor"},"assertOutput":"assertSimpleOutput"}]},{"name":"skill_invocation","buildInput":"skillInvocationInput","providers":[{"driver":"cursor","transcriptFile":"skill_invocation/cursor_transcript.ndjson","modelSelection":{"instanceId":"cursor","model":"composer-2.5"},"assertOutput":"assertSkillInvocationCursorOutput"}]},{"name":"tool_call_denied_write","buildInput":"toolCallDeniedWriteInput","providers":[{"driver":"claudeAgent","transcriptFile":"tool_call_denied_write/claude_transcript.ndjson","modelSelection":{"instanceId":"claudeAgent","model":"claude-sonnet-4-6"},"runtimePolicyOverride":{"approvalPolicy":"on-request","sandboxPolicy":{"type":"workspaceWrite","writableRoots":[],"readOnlyAccess":{"type":"fullAccess"},"networkAccess":false}},"expectedAbsentWorkspacePaths":[".claude-denied-write.txt"],"assertOutput":"assertToolCallDeniedWriteClaudeOutput"},{"driver":"fx","transcriptFile":"tool_call_denied_write/fx_transcript.ndjson","modelSelection":{"instanceId":"fx","model":"default"},"runtimePolicyOverride":{"approvalPolicy":"on-request","sandboxPolicy":{"type":"workspaceWrite","writableRoots":[],"readOnlyAccess":{"type":"fullAccess"},"networkAccess":false}},"expectedAbsentWorkspacePaths":[".claude-denied-write.txt"],"assertOutput":"assertToolCallDeniedWriteFxOutput"}]},{"name":"tool_call_read_only","buildInput":"toolCallReadOnlyInput","providers":[{"driver":"claudeAgent","transcriptFile":"tool_call_read_only/claude_transcript.ndjson","modelSelection":{"instanceId":"claudeAgent","model":"claude-sonnet-4-6"},"runtimePolicyOverride":{"approvalPolicy":"never","sandboxPolicy":{"type":"readOnly","access":{"type":"fullAccess"},"networkAccess":false}},"assertOutput":"assertToolCallReadOnlyClaudeOutput"},{"driver":"cursor","transcriptFile":"tool_call_read_only/cursor_transcript.ndjson","modelSelection":{"instanceId":"cursor","model":"composer-2.5"},"runtimePolicyOverride":{"approvalPolicy":"never","sandboxPolicy":{"type":"readOnly","access":{"type":"fullAccess"},"networkAccess":false}},"assertOutput":"assertToolCallReadOnlyCursorOutput"},{"driver":"grok","transcriptFile":"tool_call_read_only/grok_transcript.ndjson","modelSelection":{"instanceId":"grok","model":"grok-build"},"runtimePolicyOverride":{"approvalPolicy":"never","sandboxPolicy":{"type":"readOnly","access":{"type":"fullAccess"},"networkAccess":false}},"assertOutput":"assertToolCallReadOnlyCursorOutput"},{"driver":"acpRegistry","transcriptFile":"tool_call_read_only/registry_transcript.ndjson","modelSelection":{"instanceId":"acpRegistry","model":"default"},"runtimePolicyOverride":{"approvalPolicy":"never","sandboxPolicy":{"type":"readOnly","access":{"type":"fullAccess"},"networkAccess":false}},"assertOutput":"assertToolCallReadOnlyCursorOutput"}]},{"name":"tool_call_read_only_on_request","buildInput":"toolCallReadOnlyOnRequestInput","providers":[{"driver":"codex","transcriptFile":"tool_call_read_only_on_request/codex_transcript.ndjson","modelSelection":{"instanceId":"codex","model":"gpt-6-sol"},"runtimePolicyOverride":{"approvalPolicy":"on-request","sandboxPolicy":{"type":"readOnly","access":{"type":"fullAccess"},"networkAccess":false}},"assertOutput":"assertToolCallReadOnlyOnRequestOutput"},{"driver":"claudeAgent","transcriptFile":"tool_call_read_only_on_request/claude_transcript.ndjson","modelSelection":{"instanceId":"claudeAgent","model":"claude-sonnet-4-6"},"runtimePolicyOverride":{"approvalPolicy":"on-request","sandboxPolicy":{"type":"readOnly","access":{"type":"fullAccess"},"networkAccess":false}},"assertOutput":"assertToolCallReadOnlyOnRequestOutput"},{"driver":"grok","transcriptFile":"tool_call_read_only_on_request/grok_transcript.ndjson","modelSelection":{"instanceId":"grok","model":"grok-build"},"runtimePolicyOverride":{"approvalPolicy":"on-request","sandboxPolicy":{"type":"readOnly","access":{"type":"fullAccess"},"networkAccess":false}},"assertOutput":"assertToolCallReadOnlyOnRequestGrokOutput"},{"driver":"fx","transcriptFile":"tool_call_read_only_on_request/fx_transcript.ndjson","modelSelection":{"instanceId":"fx","model":"default"},"runtimePolicyOverride":{"approvalPolicy":"on-request","sandboxPolicy":{"type":"readOnly","access":{"type":"fullAccess"},"networkAccess":false}},"assertOutput":"assertToolCallReadOnlyOnRequestFxOutput"},{"driver":"acpRegistry","transcriptFile":"tool_call_read_only_on_request/registry_transcript.ndjson","modelSelection":{"instanceId":"acpRegistry","model":"default"},"runtimePolicyOverride":{"approvalPolicy":"on-request","sandboxPolicy":{"type":"readOnly","access":{"type":"fullAccess"},"networkAccess":false}},"assertOutput":"assertToolCallReadOnlyOnRequestOutput"}]},{"name":"tool_call_workspace_never","buildInput":"toolCallWorkspaceNeverInput","providers":[{"driver":"codex","transcriptFile":"tool_call_workspace_never/codex_transcript.ndjson","modelSelection":{"instanceId":"codex","model":"gpt-6-luna"},"runtimePolicyOverride":{"approvalPolicy":"never","sandboxPolicy":{"type":"workspaceWrite","writableRoots":[],"readOnlyAccess":{"type":"fullAccess"},"networkAccess":false}},"assertOutput":"assertToolCallWorkspaceNeverOutput"},{"driver":"claudeAgent","transcriptFile":"tool_call_workspace_never/claude_transcript.ndjson","modelSelection":{"instanceId":"claudeAgent","model":"claude-sonnet-4-6"},"runtimePolicyOverride":{"approvalPolicy":"never","sandboxPolicy":{"type":"workspaceWrite","writableRoots":[],"readOnlyAccess":{"type":"fullAccess"},"networkAccess":false}},"assertOutput":"assertToolCallWorkspaceNeverClaudeOutput"}]},{"name":"tool_call_restricted_granular","buildInput":"toolCallRestrictedGranularInput","providers":[{"driver":"codex","transcriptFile":"tool_call_restricted_granular/codex_transcript.ndjson","modelSelection":{"instanceId":"codex","model":"gpt-5.6-terra"},"runtimePolicyOverride":{"approvalPolicy":{"granular":{"mcp_elicitations":true,"request_permissions":true,"rules":true,"sandbox_approval":true,"skill_approval":true}},"sandboxPolicy":{"type":"readOnly","access":{"type":"restricted","includePlatformDefaults":false,"readableRoots":[]},"networkAccess":false}},"assertOutput":"assertToolCallRestrictedGranularOutput"},{"driver":"claudeAgent","transcriptFile":"tool_call_restricted_granular/claude_transcript.ndjson","modelSelection":{"instanceId":"claudeAgent","model":"claude-sonnet-4-6"},"runtimePolicyOverride":{"approvalPolicy":{"granular":{"mcp_elicitations":true,"request_permissions":true,"rules":true,"sandbox_approval":true,"skill_approval":true}},"sandboxPolicy":{"type":"readOnly","access":{"type":"restricted","includePlatformDefaults":false,"readableRoots":[]},"networkAccess":false}},"assertOutput":"assertToolCallRestrictedGranularClaudeOutput"}]},{"name":"subagent","buildInput":"subagentInput","providers":[{"driver":"codex","transcriptFile":"subagent/codex_transcript.ndjson","modelSelection":{"instanceId":"codex","model":"gpt-5.6-luna"},"runtimePolicyOverride":{"approvalPolicy":"on-request","sandboxPolicy":{"type":"readOnly","access":{"type":"fullAccess"},"networkAccess":false}},"assertOutput":"assertSubagentOutput"},{"driver":"claudeAgent","transcriptFile":"subagent/claude_transcript.ndjson","modelSelection":{"instanceId":"claudeAgent","model":"claude-sonnet-4-6"},"assertOutput":"assertClaudeSubagentOutput"},{"driver":"cursor","transcriptFile":"subagent/cursor_transcript.ndjson","modelSelection":{"instanceId":"cursor","model":"composer-2.5"},"runtimePolicyOverride":{"approvalPolicy":"never","sandboxPolicy":{"type":"readOnly","access":{"type":"fullAccess"},"networkAccess":false}},"assertOutput":"assertCursorSubagentOutput"},{"driver":"kiro","transcriptFile":"subagent/kiro_transcript.ndjson","modelSelection":{"instanceId":"kiro","model":"claude-haiku-4.5"},"assertOutput":"assertKiroSubagentOutput"}]},{"name":"subagent_continue","buildInput":"subagentContinueInput","providers":[{"driver":"codex","transcriptFile":"subagent_continue/codex_transcript.ndjson","modelSelection":{"instanceId":"codex","model":"gpt-5.6-luna"},"assertOutput":"assertSubagentContinueOutput"}]},{"name":"subagent_v2","buildInput":"subagentV2Input","providers":[{"driver":"codex","transcriptFile":"subagent_v2/codex_transcript.ndjson","modelSelection":{"instanceId":"codex","model":"gpt-6-luna"},"assertOutput":"assertSubagentV2Output"},{"driver":"muse","transcriptFile":"subagent_v2/muse_transcript.ndjson","modelSelection":{"instanceId":"muse","model":"muse-spark-1.3-contributor"},"assertOutput":"assertMuseSubagentV2Output"}]},{"name":"subagent_v2_approval","buildInput":"subagentV2ApprovalInput","providers":[{"driver":"codex","transcriptFile":"subagent_v2_approval/codex_transcript.ndjson","modelSelection":{"instanceId":"codex","model":"gpt-6-luna"},"runtimePolicyOverride":{"approvalPolicy":"untrusted","sandboxPolicy":{"type":"readOnly"}},"assertOutput":"assertSubagentV2ApprovalOutput"}]},{"name":"subagent_v2_nested","buildInput":"subagentV2NestedInput","providers":[{"driver":"codex","transcriptFile":"subagent_v2_nested/codex_transcript.ndjson","modelSelection":{"instanceId":"codex","model":"gpt-6-luna"},"assertOutput":"assertSubagentV2NestedOutput"}]},{"name":"subagent_v2_nested_approval","buildInput":"subagentV2NestedApprovalInput","providers":[{"driver":"codex","transcriptFile":"subagent_v2_nested_approval/codex_transcript.ndjson","modelSelection":{"instanceId":"codex","model":"gpt-6-luna"},"runtimePolicyOverride":{"approvalPolicy":"untrusted","sandboxPolicy":{"type":"readOnly"}},"assertOutput":"assertSubagentV2NestedApprovalOutput"}]},{"name":"opencode_subagent","buildInput":"openCodeSubagentInput","providers":[{"driver":"opencode","transcriptFile":"opencode_subagent/opencode_transcript.ndjson","modelSelection":{"instanceId":"opencode","model":"openai/gpt-5.4-mini","options":[{"id":"agent","value":"build"}]},"assertOutput":"assertOpenCodeSubagentOutput"}]},{"name":"opencode2_simple","buildInput":"openCode2SimpleInput","providers":[{"driver":"opencode","transcriptFile":"opencode2_simple/opencode_transcript.ndjson","modelSelection":{"instanceId":"opencode","model":"opencode/space-bunny-free","options":[{"id":"variant","value":"high"}]},"assertOutput":"assertOpenCode2SimpleOutput"}]},{"name":"opencode2_missing_step_usage","buildInput":"openCode2SimpleInput","providers":[{"driver":"opencode","transcriptFile":"opencode2_missing_step_usage/opencode_transcript.ndjson","modelSelection":{"instanceId":"opencode","model":"opencode/space-bunny-free","options":[{"id":"variant","value":"high"}]},"assertOutput":"assertOpenCode2MissingStepUsageOutput"}]},{"name":"opencode2_tool_call","buildInput":"openCode2ToolCallInput","providers":[{"driver":"opencode","transcriptFile":"opencode2_tool_call/opencode_transcript.ndjson","modelSelection":{"instanceId":"opencode","model":"opencode/big-pickle"},"assertOutput":"assertOpenCode2ToolCallOutput"}]},{"name":"opencode2_interrupt","buildInput":"openCode2InterruptInput","providers":[{"driver":"opencode","transcriptFile":"opencode2_interrupt/opencode_transcript.ndjson","modelSelection":{"instanceId":"opencode","model":"opencode/big-pickle"},"assertOutput":"assertOpenCode2InterruptOutput"}]},{"name":"opencode2_inbox","buildInput":"openCode2InboxInput","providers":[{"driver":"opencode","transcriptFile":"opencode2_inbox/opencode_transcript.ndjson","modelSelection":{"instanceId":"opencode","model":"opencode/big-pickle"},"assertOutput":"assertOpenCode2InboxOutput"}]},{"name":"opencode2_revert","buildInput":"openCode2RevertInput","providers":[{"driver":"opencode","transcriptFile":"opencode2_revert/opencode_transcript.ndjson","modelSelection":{"instanceId":"opencode","model":"opencode/big-pickle"},"assertOutput":"assertOpenCode2RevertOutput"}]},{"name":"opencode2_permission","buildInput":"openCode2PermissionInput","providers":[{"driver":"opencode","transcriptFile":"opencode2_permission/opencode_transcript.ndjson","modelSelection":{"instanceId":"opencode","model":"opencode/big-pickle"},"assertOutput":"assertOpenCode2PermissionOutput"}]},{"name":"opencode2_question","buildInput":"openCode2QuestionInput","providers":[{"driver":"opencode","transcriptFile":"opencode2_question/opencode_transcript.ndjson","modelSelection":{"instanceId":"opencode","model":"opencode/big-pickle"},"assertOutput":"assertOpenCode2QuestionOutput"}]},{"name":"opencode2_subagent","buildInput":"openCode2SubagentInput","providers":[{"driver":"opencode","transcriptFile":"opencode2_subagent/opencode_transcript.ndjson","modelSelection":{"instanceId":"opencode","model":"opencode/big-pickle"},"assertOutput":"assertOpenCode2SubagentOutput"}]},{"name":"opencode2_background","buildInput":"openCode2BackgroundInput","providers":[{"driver":"opencode","transcriptFile":"opencode2_background/opencode_transcript.ndjson","modelSelection":{"instanceId":"opencode","model":"opencode/big-pickle"},"runContinuationWorker":true,"assertOutput":"assertOpenCode2BackgroundOutput"}]},{"name":"opencode2_nested_background","buildInput":"openCode2NestedBackgroundInput","providers":[{"driver":"opencode","transcriptFile":"opencode2_nested_background/opencode_transcript.ndjson","modelSelection":{"instanceId":"opencode","model":"openrouter/deepseek/deepseek-v4-flash"},"assertOutput":"assertOpenCode2NestedBackgroundOutput"}]},{"name":"opencode2_compaction","buildInput":"openCode2CompactionInput","providers":[{"driver":"opencode","transcriptFile":"opencode2_compaction/opencode_transcript.ndjson","modelSelection":{"instanceId":"opencode","model":"opencode/big-pickle"},"assertOutput":"assertOpenCode2CompactionOutput"}]},{"name":"opencode2_resume_after_restart","buildInput":"openCode2ResumeAfterRestartInput","providers":[{"driver":"opencode","transcriptFile":"opencode2_resume_after_restart/opencode_transcript.ndjson","modelSelection":{"instanceId":"opencode","model":"opencode/big-pickle"},"assertOutput":"assertOpenCode2ResumeAfterRestartOutput"}]},{"name":"opencode2_command","buildInput":"openCode2CommandInput","providers":[{"driver":"opencode","transcriptFile":"opencode2_command/opencode_transcript.ndjson","modelSelection":{"instanceId":"opencode","model":"openrouter/deepseek/deepseek-v4-flash"},"assertOutput":"assertOpenCode2CommandOutput"}]},{"name":"opencode2_skill","buildInput":"openCode2SkillInput","providers":[{"driver":"opencode","transcriptFile":"opencode2_skill/opencode_transcript.ndjson","modelSelection":{"instanceId":"opencode","model":"openrouter/deepseek/deepseek-v4-flash"},"assertOutput":"assertOpenCode2SkillOutput"}]},{"name":"opencode_child_approval","buildInput":"openCodeChildApprovalInput","providers":[{"driver":"opencode","transcriptFile":"opencode_child_approval/opencode_transcript.ndjson","modelSelection":{"instanceId":"opencode","model":"openai/gpt-5.4-mini","options":[{"id":"agent","value":"build"}]},"assertOutput":"assertOpenCodeChildApprovalOutput"}]},{"name":"opencode_running_child_approval","buildInput":"openCodeRunningChildApprovalInput","providers":[{"driver":"opencode","transcriptFile":"opencode_running_child_approval/opencode_transcript.ndjson","modelSelection":{"instanceId":"opencode","model":"openai/gpt-5.4-mini","options":[{"id":"agent","value":"build"}]},"assertOutput":"assertOpenCodeRunningChildApprovalOutput"}]},{"name":"codex_model_reroute","buildInput":"multiTurnInput","providers":[{"driver":"codex","transcriptFile":"codex_model_reroute/codex_transcript.ndjson","modelSelection":{"instanceId":"codex","model":"gpt-6-luna"},"assertOutput":"assertCodexModelRerouteOutput"}]},{"name":"multi_turn","buildInput":"multiTurnInput","providers":[{"driver":"codex","transcriptFile":"multi_turn/codex_transcript.ndjson","modelSelection":{"instanceId":"codex","model":"gpt-6-luna"},"assertOutput":"assertMultiTurnOutput"},{"driver":"claudeAgent","transcriptFile":"multi_turn/claude_transcript.ndjson","modelSelection":{"instanceId":"claudeAgent","model":"claude-sonnet-4-6"},"assertOutput":"assertMultiTurnClaudeOutput"},{"driver":"cursor","transcriptFile":"multi_turn/cursor_transcript.ndjson","modelSelection":{"instanceId":"cursor","model":"composer-2.5"},"assertOutput":"assertMultiTurnOutput"},{"driver":"grok","transcriptFile":"multi_turn/grok_transcript.ndjson","modelSelection":{"instanceId":"grok","model":"grok-build"},"assertOutput":"assertMultiTurnOutput"},{"driver":"fx","transcriptFile":"multi_turn/fx_transcript.ndjson","modelSelection":{"instanceId":"fx","model":"default"},"assertOutput":"assertMultiTurnOutput"},{"driver":"kiro","transcriptFile":"multi_turn/kiro_transcript.ndjson","modelSelection":{"instanceId":"kiro","model":"claude-haiku-4.5"},"assertOutput":"assertMultiTurnOutput"},{"driver":"acpRegistry","transcriptFile":"multi_turn/registry_transcript.ndjson","modelSelection":{"instanceId":"acpRegistry","model":"default"},"assertOutput":"assertMultiTurnOutput"},{"driver":"pi","transcriptFile":"multi_turn/pi_transcript.ndjson","modelSelection":{"instanceId":"pi","model":"openrouter/deepseek/deepseek-v4-flash"},"assertOutput":"assertPiMultiTurnOutput"},{"driver":"muse","transcriptFile":"multi_turn/muse_transcript.ndjson","modelSelection":{"instanceId":"muse","model":"muse-spark-1.3-contributor"},"assertOutput":"assertMuseMultiTurnOutput"}]},{"name":"muse_permission","buildInput":"musePermissionInput","providers":[{"driver":"muse","transcriptFile":"muse_permission/muse_transcript.ndjson","modelSelection":{"instanceId":"muse","model":"muse-spark-1.3-contributor"},"expectedAbsentWorkspacePaths":["muse-declined.txt"],"assertOutput":"assertMusePermissionOutput"}]},{"name":"muse_subagent_approval","buildInput":"museSubagentApprovalInput","providers":[{"driver":"muse","transcriptFile":"muse_subagent_approval/muse_transcript.ndjson","modelSelection":{"instanceId":"muse","model":"muse-spark-1.3-contributor"},"assertOutput":"assertMuseSubagentApprovalOutput"}]},{"name":"muse_subagent_interrupt","buildInput":"museSubagentInterruptInput","providers":[{"driver":"muse","transcriptFile":"muse_subagent_interrupt/muse_transcript.ndjson","modelSelection":{"instanceId":"muse","model":"muse-spark-1.3-contributor"},"assertOutput":"assertMuseSubagentInterruptOutput"}]},{"name":"muse_workflow","buildInput":"museWorkflowInput","providers":[{"driver":"muse","transcriptFile":"muse_workflow/muse_transcript.ndjson","modelSelection":{"instanceId":"muse","model":"muse-spark-1.3-contributor"},"runContinuationWorker":true,"assertOutput":"assertMuseWorkflowOutput"}]},{"name":"pi_compaction","buildInput":"piCompactionInput","providers":[{"driver":"pi","transcriptFile":"pi_compaction/pi_transcript.ndjson","modelSelection":{"instanceId":"pi","model":"openrouter/deepseek/deepseek-v4-flash"},"assertOutput":"assertPiCompactionOutput"}]},{"name":"provider_thread_resume","buildInput":"providerThreadResumeInput","providers":[{"driver":"pi","transcriptFile":"provider_thread_resume/pi_transcript.ndjson","modelSelection":{"instanceId":"pi","model":"openrouter/deepseek/deepseek-v4-flash"},"assertOutput":"assertPiProviderThreadResumeOutput"}]},{"name":"multi_turn_restart","buildInput":"multiTurnInput","providers":[{"driver":"claudeAgent","transcriptFile":"multi_turn_restart/claude_transcript.ndjson","modelSelection":{"instanceId":"claudeAgent","model":"claude-sonnet-4-6"},"assertOutput":"assertMultiTurnClaudeOutput"}]},{"name":"queued_cancelled_while_active","buildInput":"queuedCancelledWhileActiveInput","providers":[{"driver":"codex","transcriptFile":"queued_turn/codex_transcript.ndjson","recordedScenario":"queued_turn","transcriptEntriesThroughLabel":"turn/completed","modelSelection":{"instanceId":"codex","model":"gpt-6-luna"},"assertOutput":"assertQueuedCancelledWhileActiveOutput"}]},{"name":"queued_turn","buildInput":"queuedTurnInput","providers":[{"driver":"codex","transcriptFile":"queued_turn/codex_transcript.ndjson","modelSelection":{"instanceId":"codex","model":"gpt-6-luna"},"assertOutput":"assertQueuedTurnOutput"},{"driver":"claudeAgent","transcriptFile":"queued_turn/claude_transcript.ndjson","modelSelection":{"instanceId":"claudeAgent","model":"claude-sonnet-4-6"},"assertOutput":"assertQueuedTurnOutput"},{"driver":"cursor","transcriptFile":"queued_turn/cursor_transcript.ndjson","modelSelection":{"instanceId":"cursor","model":"composer-2.5"},"assertOutput":"assertQueuedTurnOutput"},{"driver":"grok","transcriptFile":"queued_turn/grok_transcript.ndjson","modelSelection":{"instanceId":"grok","model":"grok-build"},"assertOutput":"assertQueuedTurnOutput"},{"driver":"fx","transcriptFile":"queued_turn/fx_transcript.ndjson","modelSelection":{"instanceId":"fx","model":"default"},"assertOutput":"assertQueuedTurnOutput"},{"driver":"kiro","transcriptFile":"queued_turn/kiro_transcript.ndjson","modelSelection":{"instanceId":"kiro","model":"claude-haiku-4.5"},"assertOutput":"assertQueuedTurnOutput"},{"driver":"acpRegistry","transcriptFile":"queued_turn/registry_transcript.ndjson","modelSelection":{"instanceId":"acpRegistry","model":"default"},"assertOutput":"assertQueuedTurnOutput"}]},{"name":"todo_list","buildInput":"todoListInput","providers":[{"driver":"codex","transcriptFile":"todo_list/codex_transcript.ndjson","modelSelection":{"instanceId":"codex","model":"gpt-6-luna"},"runtimePolicyOverride":{"approvalPolicy":"never","sandboxPolicy":{"type":"readOnly","access":{"type":"fullAccess"},"networkAccess":false}},"assertOutput":"assertTodoListOutput"},{"driver":"cursor","transcriptFile":"todo_list/cursor_transcript.ndjson","modelSelection":{"instanceId":"cursor","model":"composer-2.5"},"runtimePolicyOverride":{"approvalPolicy":"never","sandboxPolicy":{"type":"readOnly","access":{"type":"fullAccess"},"networkAccess":false}},"assertOutput":"assertTodoListCursorOutput"},{"driver":"grok","transcriptFile":"todo_list/grok_transcript.ndjson","modelSelection":{"instanceId":"grok","model":"grok-build"},"assertOutput":"assertTodoListGrokOutput"},{"driver":"acpRegistry","transcriptFile":"todo_list/registry_transcript.ndjson","modelSelection":{"instanceId":"acpRegistry","model":"default"},"assertOutput":"assertTodoListGrokOutput"}]},{"name":"web_search","buildInput":"webSearchInput","providers":[{"driver":"codex","transcriptFile":"web_search/codex_transcript.ndjson","modelSelection":{"instanceId":"codex","model":"gpt-6-luna"},"assertOutput":"assertWebSearchOutput"},{"driver":"claudeAgent","transcriptFile":"web_search/claude_transcript.ndjson","modelSelection":{"instanceId":"claudeAgent","model":"claude-sonnet-4-6"},"assertOutput":"assertClaudeWebSearchOutput"}]},{"name":"plan_questions","buildInput":"planQuestionsInput","providers":[{"driver":"codex","transcriptFile":"plan_questions/codex_transcript.ndjson","modelSelection":{"instanceId":"codex","model":"gpt-6-luna"},"runtimePolicyOverride":{"approvalPolicy":"never","sandboxPolicy":{"type":"readOnly","access":{"type":"fullAccess"},"networkAccess":false}},"assertOutput":"assertCodexPlanQuestionsOutput"},{"driver":"grok","transcriptFile":"plan_questions/grok_transcript.ndjson","modelSelection":{"instanceId":"grok","model":"grok-build"},"runtimePolicyOverride":{"approvalPolicy":"never","sandboxPolicy":{"type":"readOnly","access":{"type":"fullAccess"},"networkAccess":false}},"assertOutput":"assertPlanQuestionsOutput"},{"driver":"opencode","transcriptFile":"plan_questions/opencode_transcript.ndjson","modelSelection":{"instanceId":"opencode","model":"openai/gpt-5.4-mini","options":[{"id":"agent","value":"build"}]},"runtimePolicyOverride":{"approvalPolicy":"never","sandboxPolicy":{"type":"readOnly","access":{"type":"fullAccess"},"networkAccess":false}},"assertOutput":"assertOpenCodePlanQuestionsOutput"}]},{"name":"proposed_plan","buildInput":"proposedPlanInput","providers":[{"driver":"codex","transcriptFile":"proposed_plan/codex_transcript.ndjson","modelSelection":{"instanceId":"codex","model":"gpt-6-luna"},"runtimePolicyOverride":{"approvalPolicy":"never","sandboxPolicy":{"type":"readOnly","access":{"type":"fullAccess"},"networkAccess":false}},"assertOutput":"assertProposedPlanOutput"},{"driver":"cursor","transcriptFile":"proposed_plan/cursor_transcript.ndjson","modelSelection":{"instanceId":"cursor","model":"composer-2.5"},"runtimePolicyOverride":{"approvalPolicy":"never","sandboxPolicy":{"type":"readOnly","access":{"type":"fullAccess"},"networkAccess":false}},"assertOutput":"assertProposedPlanCursorOutput"}]},{"name":"message_steering","buildInput":"messageSteeringInput","providers":[{"driver":"codex","transcriptFile":"message_steering/codex_transcript.ndjson","modelSelection":{"instanceId":"codex","model":"gpt-6-luna"},"assertOutput":"assertMessageSteeringOutput"},{"driver":"claudeAgent","transcriptFile":"message_steering/claude_transcript.ndjson","modelSelection":{"instanceId":"claudeAgent","model":"claude-sonnet-4-6"},"assertOutput":"assertClaudeMessageSteeringOutput"},{"driver":"cursor","transcriptFile":"message_steering/cursor_transcript.ndjson","modelSelection":{"instanceId":"cursor","model":"composer-2.5"},"assertOutput":"assertCursorMessageSteeringOutput"},{"driver":"grok","transcriptFile":"message_steering/grok_transcript.ndjson","modelSelection":{"instanceId":"grok","model":"grok-build"},"assertOutput":"assertGrokMessageSteeringOutput"},{"driver":"fx","transcriptFile":"message_steering/fx_transcript.ndjson","modelSelection":{"instanceId":"fx","model":"default"},"assertOutput":"assertGrokMessageSteeringOutput"},{"driver":"acpRegistry","transcriptFile":"message_steering/registry_transcript.ndjson","modelSelection":{"instanceId":"acpRegistry","model":"default"},"assertOutput":"assertGrokMessageSteeringOutput"},{"driver":"pi","transcriptFile":"message_steering/pi_transcript.ndjson","modelSelection":{"instanceId":"pi","model":"openrouter/deepseek/deepseek-v4-flash"},"assertOutput":"assertPiMessageSteeringOutput"}]},{"name":"turn_interrupt","buildInput":"turnInterruptInput","providers":[{"driver":"codex","transcriptFile":"turn_interrupt/codex_transcript.ndjson","modelSelection":{"instanceId":"codex","model":"gpt-6-luna"},"runtimePolicyOverride":{"approvalPolicy":"never","sandboxPolicy":{"type":"workspaceWrite","writableRoots":[],"readOnlyAccess":{"type":"fullAccess"},"networkAccess":false}},"assertOutput":"assertTurnInterruptOutput"},{"driver":"claudeAgent","transcriptFile":"turn_interrupt/claude_transcript.ndjson","modelSelection":{"instanceId":"claudeAgent","model":"claude-sonnet-4-6"},"runtimePolicyOverride":{"approvalPolicy":"never","sandboxPolicy":{"type":"workspaceWrite","writableRoots":[],"readOnlyAccess":{"type":"fullAccess"},"networkAccess":false}},"assertOutput":"assertTurnInterruptClaudeOutput"},{"driver":"grok","transcriptFile":"turn_interrupt/grok_transcript.ndjson","modelSelection":{"instanceId":"grok","model":"grok-build"},"runtimePolicyOverride":{"approvalPolicy":"never","sandboxPolicy":{"type":"workspaceWrite","writableRoots":[],"readOnlyAccess":{"type":"fullAccess"},"networkAccess":false}},"assertOutput":"assertTurnInterruptOutput"},{"driver":"fx","transcriptFile":"turn_interrupt/fx_transcript.ndjson","modelSelection":{"instanceId":"fx","model":"default"},"runtimePolicyOverride":{"approvalPolicy":"never","sandboxPolicy":{"type":"workspaceWrite","writableRoots":[],"readOnlyAccess":{"type":"fullAccess"},"networkAccess":false}},"assertOutput":"assertTurnInterruptOutput"},{"driver":"kiro","transcriptFile":"turn_interrupt/kiro_transcript.ndjson","modelSelection":{"instanceId":"kiro","model":"claude-haiku-4.5"},"runtimePolicyOverride":{"approvalPolicy":"never","sandboxPolicy":{"type":"workspaceWrite","writableRoots":[],"readOnlyAccess":{"type":"fullAccess"},"networkAccess":false}},"assertOutput":"assertTurnInterruptOutput"},{"driver":"acpRegistry","transcriptFile":"turn_interrupt/registry_transcript.ndjson","modelSelection":{"instanceId":"acpRegistry","model":"default"},"runtimePolicyOverride":{"approvalPolicy":"never","sandboxPolicy":{"type":"workspaceWrite","writableRoots":[],"readOnlyAccess":{"type":"fullAccess"},"networkAccess":false}},"assertOutput":"assertTurnInterruptOutput"},{"driver":"opencode","transcriptFile":"turn_interrupt/opencode_transcript.ndjson","modelSelection":{"instanceId":"opencode","model":"openai/gpt-5.4-mini","options":[{"id":"agent","value":"build"}]},"runtimePolicyOverride":{"approvalPolicy":"never","sandboxPolicy":{"type":"workspaceWrite","writableRoots":[],"readOnlyAccess":{"type":"fullAccess"},"networkAccess":false}},"assertOutput":"assertTurnInterruptOutput"},{"driver":"muse","transcriptFile":"turn_interrupt/muse_transcript.ndjson","modelSelection":{"instanceId":"muse","model":"muse-spark-1.3-contributor"},"assertOutput":"assertMuseTurnInterruptOutput"}]},{"name":"fx_early_stop","buildInput":"fxEarlyStopInput","providers":[{"driver":"fx","transcriptFile":"fx_early_stop/fx_transcript.ndjson","modelSelection":{"instanceId":"fx","model":"default"},"runtimePolicyOverride":{"approvalPolicy":"never","sandboxPolicy":{"type":"workspaceWrite","writableRoots":[],"readOnlyAccess":{"type":"fullAccess"},"networkAccess":false}},"assertOutput":"assertFxEarlyStopOutput"}]},{"name":"fx_subagent","buildInput":"fxSubagentInput","providers":[{"driver":"fx","transcriptFile":"fx_subagent/fx_transcript.ndjson","modelSelection":{"instanceId":"fx","model":"default"},"assertOutput":"assertFxSubagentOutput"}]},{"name":"command_output_streaming","buildInput":"commandOutputStreamingInput","providers":[{"driver":"codex","transcriptFile":"command_output_streaming/codex_transcript.ndjson","modelSelection":{"instanceId":"codex","model":"gpt-6-luna"},"runtimePolicyOverride":{"approvalPolicy":"never","sandboxPolicy":{"type":"workspaceWrite","writableRoots":[],"readOnlyAccess":{"type":"fullAccess"},"networkAccess":false}},"assertOutput":"assertCommandOutputStreamingOutput"},{"driver":"claudeAgent","transcriptFile":"command_output_streaming/claude_transcript.ndjson","modelSelection":{"instanceId":"claudeAgent","model":"claude-sonnet-4-6"},"assertOutput":"assertCommandOutputStreamingOutput"}]},{"name":"turn_interrupt_mid_tool","buildInput":"turnInterruptMidToolInput","providers":[{"driver":"codex","transcriptFile":"turn_interrupt_mid_tool/codex_transcript.ndjson","modelSelection":{"instanceId":"codex","model":"gpt-6-luna"},"runtimePolicyOverride":{"approvalPolicy":"never","sandboxPolicy":{"type":"workspaceWrite","writableRoots":[],"readOnlyAccess":{"type":"fullAccess"},"networkAccess":false}},"assertOutput":"assertTurnInterruptMidToolCodexOutput"},{"driver":"claudeAgent","transcriptFile":"turn_interrupt_mid_tool/claude_transcript.ndjson","modelSelection":{"instanceId":"claudeAgent","model":"claude-sonnet-4-6"},"runtimePolicyOverride":{"approvalPolicy":"never","sandboxPolicy":{"type":"workspaceWrite","writableRoots":[],"readOnlyAccess":{"type":"fullAccess"},"networkAccess":false}},"assertOutput":"assertTurnInterruptMidToolClaudeOutput"},{"driver":"cursor","transcriptFile":"turn_interrupt_mid_tool/cursor_transcript.ndjson","modelSelection":{"instanceId":"cursor","model":"composer-2.5"},"runtimePolicyOverride":{"approvalPolicy":"never","sandboxPolicy":{"type":"workspaceWrite","writableRoots":[],"readOnlyAccess":{"type":"fullAccess"},"networkAccess":false}},"assertOutput":"assertTurnInterruptMidToolCursorOutput"},{"driver":"pi","transcriptFile":"turn_interrupt_mid_tool/pi_transcript.ndjson","modelSelection":{"instanceId":"pi","model":"openrouter/deepseek/deepseek-v4-flash"},"assertOutput":"assertTurnInterruptMidToolPiOutput"}]},{"name":"kiro_turn_interrupt_mid_tool","buildInput":"turnInterruptMidToolKiroInput","providers":[{"driver":"kiro","transcriptFile":"turn_interrupt_mid_tool/kiro_transcript.ndjson","modelSelection":{"instanceId":"kiro","model":"claude-haiku-4.5"},"assertOutput":"assertTurnInterruptMidToolKiroOutput"}]},{"name":"turn_interrupt_restart","buildInput":"turnInterruptRestartInput","providers":[{"driver":"claudeAgent","transcriptFile":"turn_interrupt_restart/claude_transcript.ndjson","modelSelection":{"instanceId":"claudeAgent","model":"claude-sonnet-4-6"},"runtimePolicyOverride":{"approvalPolicy":"never","sandboxPolicy":{"type":"workspaceWrite","writableRoots":[],"readOnlyAccess":{"type":"fullAccess"},"networkAccess":false}},"assertOutput":"assertTurnInterruptRestartClaudeOutput"}]},{"name":"thread_rollback","buildInput":"threadRollbackInput","providers":[{"driver":"codex","transcriptFile":"thread_rollback/codex_transcript.ndjson","modelSelection":{"instanceId":"codex","model":"gpt-6-luna"},"assertOutput":"assertThreadRollbackOutput"},{"driver":"claudeAgent","transcriptFile":"thread_rollback/claude_transcript.ndjson","modelSelection":{"instanceId":"claudeAgent","model":"claude-sonnet-4-6"},"assertOutput":"assertClaudeThreadRollbackOutput"},{"driver":"pi","transcriptFile":"thread_rollback/pi_transcript.ndjson","modelSelection":{"instanceId":"pi","model":"openrouter/deepseek/deepseek-v4-flash"},"assertOutput":"assertPiThreadRollbackOutput"}]},{"name":"thread_rollback_after_restart","buildInput":"threadRollbackAfterRestartInput","providers":[{"driver":"codex","transcriptFile":"thread_rollback_after_restart/codex_transcript.ndjson","modelSelection":{"instanceId":"codex","model":"gpt-6-luna"},"assertOutput":"assertThreadRollbackAfterRestartOutput"}]},{"name":"thread_rollback_after_stop","buildInput":"threadRollbackAfterStopInput","providers":[{"driver":"pi","transcriptFile":"thread_rollback_after_stop/pi_transcript.ndjson","modelSelection":{"instanceId":"pi","model":"openrouter/deepseek/deepseek-v4-flash"},"assertOutput":"assertPiThreadRollbackAfterStopOutput"}]},{"name":"thread_rollback_to_stopped_turn","buildInput":"threadRollbackToStoppedTurnInput","providers":[{"driver":"codex","transcriptFile":"thread_rollback_to_stopped_turn/codex_transcript.ndjson","modelSelection":{"instanceId":"codex","model":"gpt-6-luna"},"runtimePolicyOverride":{"approvalPolicy":"never","sandboxPolicy":{"type":"workspaceWrite","writableRoots":[],"readOnlyAccess":{"type":"fullAccess"},"networkAccess":false}},"assertOutput":"assertThreadRollbackToStoppedTurnOutput"}]},{"name":"stop_background_work_after_failed_turn","buildInput":"stopBackgroundWorkAfterFailedTurnInput","providers":[{"driver":"acpRegistry","transcriptFile":"stop_background_work_after_failed_turn/registry_transcript.ndjson","modelSelection":{"instanceId":"acpRegistry","model":"default"},"assertOutput":"assertStopBackgroundWorkAfterFailedTurnOutput"}]},{"name":"stop_background_work_after_release","buildInput":"stopBackgroundWorkAfterReleaseInput","providers":[{"driver":"acpRegistry","transcriptFile":"stop_background_work_after_failed_turn/registry_transcript.ndjson","recordedScenario":"stop_background_work_after_failed_turn","modelSelection":{"instanceId":"acpRegistry","model":"default"},"assertOutput":"assertStopBackgroundWorkAfterFailedTurnOutput"}]}]"###,
    )?)
}
