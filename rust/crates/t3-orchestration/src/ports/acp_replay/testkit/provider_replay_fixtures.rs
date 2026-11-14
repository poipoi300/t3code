use crate::{
    ports::acp_replay::testkit::fixtures::index::RECORDED_FIXTURES,
    testing::replay::{RecordedFixture, ReplayEntry, ReplayError, ReplayTranscript, decode_ndjson},
};
use t3_contracts::orchestration_v2::{OrchestrationV2DomainEvent, ProviderReplayTranscriptHeader};

pub const DRIVERS_WITH_REPLAY_FIXTURE_FILES: &[&str] = &[
    "acpRegistry",
    "claudeAgent",
    "codex",
    "cursor",
    "fx",
    "grok",
    "kiro",
    "muse",
    "opencode",
    "pi",
];

pub struct ReplayFixture {
    pub recording: &'static RecordedFixture,
    pub transcript: ReplayTranscript,
}

pub fn read_replay_fixture_transcript(file: &str) -> Result<ReplayTranscript, ReplayError> {
    let recording = RECORDED_FIXTURES
        .iter()
        .find(|recording| recording.file == file)
        .ok_or_else(|| ReplayError::Fixture {
            fixture: file.into(),
            detail: "recording is not registered".into(),
        })?;
    decode_ndjson(recording.ndjson)
}

pub fn replay_fixtures(driver: &str) -> Result<Vec<ReplayFixture>, ReplayError> {
    let mut fixtures = Vec::new();
    for recording in RECORDED_FIXTURES {
        let header = recording
            .ndjson
            .lines()
            .find(|line| !line.trim().is_empty())
            .ok_or(ReplayError::Empty)?;
        let header: ProviderReplayTranscriptHeader =
            serde_json::from_str(header).map_err(|error| ReplayError::Record {
                line: 1,
                detail: error.to_string(),
            })?;
        if !DRIVERS_WITH_REPLAY_FIXTURE_FILES.contains(&header.provider.0.as_str()) {
            return Err(ReplayError::Fixture {
                fixture: recording.file.into(),
                detail: format!("{} has no ReplayFixtures test file", header.provider.0),
            });
        }
        if header.provider.0 == driver {
            fixtures.push(ReplayFixture {
                recording,
                transcript: decode_ndjson(recording.ndjson)?,
            });
        }
    }
    Ok(fixtures)
}

pub fn find_replay_fixture(name: &str, driver: &str) -> Result<Option<ReplayFixture>, ReplayError> {
    Ok(replay_fixtures(driver)?
        .into_iter()
        .find(|fixture| fixture.recording.name == name))
}

pub fn transcript_entries_through_label(
    mut transcript: ReplayTranscript,
    label: Option<&str>,
) -> Result<ReplayTranscript, ReplayError> {
    let Some(label) = label else {
        return Ok(transcript);
    };
    let index = transcript.entries.iter().position(|entry| matches!(entry,
        ReplayEntry::EmitInbound(entry) if entry.label.as_ref().and_then(Option::as_ref).is_some_and(|value|value.0 == label)
    )).ok_or_else(||ReplayError::Fixture { fixture: transcript.scenario.0.clone(), detail: format!("missing inbound transcript label {label}") })?;
    transcript.entries.truncate(index + 1);
    Ok(transcript)
}

pub fn is_streaming_assistant_event(event: &OrchestrationV2DomainEvent) -> bool {
    use OrchestrationV2DomainEvent::*;
    match event {
        NodeUpdated(event) => {
            event.payload.kind
                == t3_contracts::orchestration_v2::ExecutionNodeKind::AssistantMessage
                && event.payload.status
                    == t3_contracts::orchestration_v2::ExecutionNodeStatus::Running
        }
        MessageUpdated(event) => {
            event.payload.role == t3_contracts::orchestration_v2::ConversationMessageRole::Assistant
                && event.payload.streaming
        }
        TurnItemUpdated(event) => {
            matches!(&event.payload, t3_contracts::orchestration_v2::OrchestrationV2TurnItem::AssistantMessage(item) if item.streaming)
        }
        _ => false,
    }
}

pub struct PreparedReplayFixture {
    pub transcript: ReplayTranscript,
    pub scenario: super::orchestrator_scenario::Scenario,
    pub fixture_input: super::fixtures::shared::OrchestratorFixtureInput,
    pub variant: super::fixtures::shared::ProviderOrchestratorReplayVariant,
    pub workspace: std::path::PathBuf,
    pub runtime_policy_override: serde_json::Value,
}
pub fn fixture_cases(
    driver: &str,
) -> crate::error::Result<
    Vec<(
        super::fixtures::shared::OrchestratorReplayFixture,
        super::fixtures::shared::ProviderOrchestratorReplayVariant,
    )>,
> {
    let mut cases = Vec::new();
    for fixture in super::fixtures::index::replay_fixture_definitions()? {
        for variant in &fixture.providers {
            super::fixtures::shared::require(
                DRIVERS_WITH_REPLAY_FIXTURE_FILES.contains(&variant.driver.0.as_str()),
                format!(
                    "{}/{} has no ReplayFixtures test file",
                    fixture.name, variant.driver.0
                ),
            )?;
            if variant.driver.0 == driver {
                cases.push((fixture.clone(), variant.clone()));
            }
        }
    }
    Ok(cases)
}
pub fn prepare_fixture_case(
    fixture: &super::fixtures::shared::OrchestratorReplayFixture,
    variant: super::fixtures::shared::ProviderOrchestratorReplayVariant,
    fixture_input: super::fixtures::shared::OrchestratorFixtureInput,
    workspace: std::path::PathBuf,
    build_instructions: impl Fn(&str, &str) -> String,
    transform_workspace: impl FnOnce(
        ReplayTranscript,
        &std::path::Path,
    ) -> crate::error::Result<ReplayTranscript>,
) -> crate::error::Result<PreparedReplayFixture> {
    use super::{
        deterministic_runtime::{DEFAULT_RANDOM_SEED, SeededIds},
        fixtures::shared::materialize_fixture_input,
        replay_runtime_instructions::materialize_replay_transcript_runtime_instructions,
    };
    let transcript = read_replay_fixture_transcript(&variant.transcript_file).map_err(|error| {
        crate::error::Error::Service {
            operation: "read replay fixture",
            detail: error.to_string(),
        }
    })?;
    let transcript = transcript_entries_through_label(
        transcript,
        variant.transcript_entries_through_label.as_deref(),
    )
    .map_err(|error| crate::error::Error::Service {
        operation: "truncate replay fixture",
        detail: error.to_string(),
    })?;
    let transcript = materialize_replay_transcript_runtime_instructions(
        transcript,
        &variant.driver.0,
        &variant.model_selection.model,
        build_instructions,
    );
    let transcript = transform_workspace(transcript, &workspace)?;
    let materialized = materialize_fixture_input(
        &fixture.name,
        &fixture_input,
        &variant.driver.0,
        &variant.model_selection,
        &SeededIds::new(DEFAULT_RANDOM_SEED),
    )?;
    let scenario = materialized.into_scenario(format!("{}/{}", fixture.name, variant.driver.0))?;
    let mut runtime_policy_override = variant
        .runtime_policy_override
        .clone()
        .unwrap_or_else(|| serde_json::json!({}));
    runtime_policy_override["cwd"] = serde_json::json!(workspace);
    Ok(PreparedReplayFixture {
        transcript,
        scenario,
        fixture_input,
        variant,
        workspace,
        runtime_policy_override,
    })
}
pub async fn run_prepared_fixture(
    prepared: PreparedReplayFixture,
    services: &super::orchestrator_scenario::ScenarioServices,
    assert_output: impl FnOnce(
        &super::orchestrator_scenario::ScenarioResult,
        &ReplayTranscript,
    ) -> crate::error::Result<()>,
) -> crate::error::Result<super::orchestrator_scenario::ScenarioResult> {
    use super::fixtures::shared::{assert_provider_native_subagent_root_turns, require, rows};
    let result = super::orchestrator_scenario::run_scenario(prepared.scenario, services).await?;
    assert_output(&result, &prepared.transcript)?;
    assert_provider_native_subagent_root_turns(&result)?;
    if let Some(paths) = prepared.variant.expected_absent_workspace_paths {
        for path in paths {
            require(
                !prepared.workspace.join(&path).try_exists()?,
                format!("fixture must not create {path}"),
            )?;
        }
    }
    require(
        !result
            .domain_events
            .iter()
            .any(is_streaming_assistant_event),
        "buffered delivery persisted streaming assistant artifacts",
    )?;
    let root = result
        .projections
        .values()
        .find(|projection| projection.thread.lineage.parent_thread_id.is_none())
        .ok_or_else(|| crate::error::Error::Service {
            operation: "fixture result",
            detail: "missing root projection".into(),
        })?;
    let mut model = prepared.variant.model_selection;
    if let Some(super::fixtures::shared::OrchestratorFixtureInputStep::SetModel {
        model: selected,
    }) = prepared.fixture_input.steps.iter().rev().find(|step| {
        matches!(
            step,
            super::fixtures::shared::OrchestratorFixtureInputStep::SetModel { .. }
        )
    }) {
        model.model = selected.clone();
    }
    let root = serde_json::to_value(root)?;
    require(
        rows(&root, "runs")
            .next_back()
            .is_some_and(|run| run["modelSelection"] == serde_json::json!(model)),
        "latest run has incorrect model selection",
    )?;
    if rows(&root, "runs").any(|run| run["status"] == "completed") {
        require(
            rows(&root, "checkpoints").any(|checkpoint| {
                checkpoint["ordinalWithinScope"] == 0
                    && checkpoint["appRunOrdinal"].is_null()
                    && checkpoint["status"] == "ready"
            }),
            "completed thread has no addressable thread-start checkpoint",
        )?;
    }
    Ok(result)
}

pub type ExternalOutputAssertion = dyn Fn(
    &str,
    &super::orchestrator_scenario::ScenarioResult,
    &ReplayTranscript,
) -> crate::error::Result<()>;
#[derive(Default)]
pub struct OutputAssertionServices<'a> {
    pub pending_work:
        Option<&'a dyn super::fixtures::grok_monitor::output::PendingBackgroundWorkDeriver>,
    pub terminal:
        Option<&'a dyn super::fixtures::command_output_streaming::output::TerminalOutputAssertions>,
    pub external: Option<&'a ExternalOutputAssertion>,
}
pub fn assert_fixture_output(
    name: &str,
    result: &super::orchestrator_scenario::ScenarioResult,
    transcript: &ReplayTranscript,
    services: &OutputAssertionServices<'_>,
) -> crate::error::Result<()> {
    match name {
        "assertAcpTitledMultiSelectOutput"=>super::fixtures::acp_titled_multiselect::output::assert_acp_titled_multi_select_output(result,transcript),
        "assertCommandOutputStreamingOutput"=>super::fixtures::command_output_streaming::output::assert_command_output_streaming_output(result,transcript,services.terminal.ok_or(crate::error::Error::Unsupported("TerminalOutputAssertions"))?),
        "assertFxEarlyStopOutput"=>super::fixtures::fx_early_stop::fx_output::assert_fx_early_stop_output(result,transcript),
        "assertFxSubagentOutput"=>super::fixtures::fx_subagent::fx_output::assert_fx_subagent_output(result,transcript),
        "assertGrokAutoBlockedCommandOutput"=>super::fixtures::grok_auto_blocked_command::output::assert_grok_auto_blocked_command_output(result,transcript),
        "assertGrokBackgroundBashOutput"=>super::fixtures::grok_background_bash::output::assert_grok_background_bash_output(result,transcript),
        "assertGrokBackgroundBashFastWakeOutput"=>super::fixtures::grok_background_bash_fast_wake::output::assert_grok_background_bash_fast_wake_output(result,transcript),
        "assertGrokBackgroundSubagentOutput"=>super::fixtures::grok_background_subagent::output::assert_grok_background_subagent_output(result,transcript),
        "assertGrokMonitorOutput"=>super::fixtures::grok_monitor::output::assert_grok_monitor_output(result,transcript,services.pending_work.ok_or(crate::error::Error::Unsupported("PendingBackgroundWorkDeriver"))?),
        "assertGrokPromptErrorOutput"=>super::fixtures::grok_prompt_error::output::assert_grok_prompt_error_output(result,transcript),
        "assertGrokSubagentLineageOutput"=>super::fixtures::grok_subagent_lineage::output::assert_grok_subagent_lineage_output(result,transcript),
        "assertKiroCompactionOutput"=>super::fixtures::kiro_compaction::output::assert_kiro_compaction_output(result,transcript),
        "assertKiroEffortOutput"=>super::fixtures::kiro_effort::output::assert_kiro_effort_output(result,transcript),
        "assertKiroForkFromTurnOutput"=>super::fixtures::kiro_fork_from_turn::output::assert_kiro_fork_from_turn_output(result,transcript),
        "assertKiroMessageSteeringOutput"=>super::fixtures::kiro_message_steering::output::assert_kiro_message_steering_output(result,transcript),
        "assertKiroModelSwitchOutput"=>super::fixtures::kiro_model_switch::output::assert_kiro_model_switch_output(result,transcript),
        "assertKiroSupervisedWriteOutput"=>super::fixtures::kiro_permission_modes::output::assert_kiro_supervised_write_output(result,transcript),
        "assertKiroFullAccessWriteOutput"=>super::fixtures::kiro_permission_modes::output::assert_kiro_full_access_write_output(result,transcript),
        "assertKiroSubagentInterruptOutput"=>super::fixtures::kiro_subagent_interrupt::output::assert_kiro_subagent_interrupt_output(result,transcript),
        "assertMalformedSessionUpdateOutput"=>super::fixtures::malformed_session_update::output::assert_malformed_session_update_output(result,transcript),
        "assertCursorMessageSteeringOutput"=>super::fixtures::message_steering::cursor_output::assert_cursor_message_steering_output(result,transcript),
        "assertGrokMessageSteeringOutput"=>super::fixtures::message_steering::grok_output::assert_grok_message_steering_output(result,transcript),
        "assertPiMessageSteeringOutput"=>super::fixtures::message_steering::pi_output::assert_pi_message_steering_output(result,transcript),
        "assertMuseMultiTurnOutput"=>super::fixtures::multi_turn::muse_output::assert_muse_multi_turn_output(result,transcript),
        "assertPiMultiTurnOutput"=>super::fixtures::multi_turn::pi_output::assert_pi_multi_turn_output(result,transcript),
        "assertMusePermissionOutput"=>super::fixtures::muse_permission::output::assert_muse_permission_output(result,transcript),
        "assertMuseSubagentApprovalOutput"=>super::fixtures::muse_subagent_approval::output::assert_muse_subagent_approval_output(result,transcript),
        "assertMuseSubagentInterruptOutput"=>super::fixtures::muse_subagent_interrupt::output::assert_muse_subagent_interrupt_output(result,transcript),
        "assertMuseWorkflowOutput"=>super::fixtures::muse_workflow::output::assert_muse_workflow_output(result,transcript),
        "assertOpenCode2BackgroundOutput"=>super::fixtures::opencode2_background::output::assert_open_code2_background_output(result,transcript),
        "assertOpenCode2CommandOutput"=>super::fixtures::opencode2_command::output::assert_open_code2_command_output(result,transcript),
        "assertOpenCode2CompactionOutput"=>super::fixtures::opencode2_compaction::output::assert_open_code2_compaction_output(result,transcript),
        "assertOpenCode2InboxOutput"=>super::fixtures::opencode2_inbox::output::assert_open_code2_inbox_output(result,transcript),
        "assertOpenCode2InterruptOutput"=>super::fixtures::opencode2_interrupt::output::assert_open_code2_interrupt_output(result,transcript),
        "assertOpenCode2MissingStepUsageOutput"=>super::fixtures::opencode2_missing_step_usage::output::assert_open_code2_missing_step_usage_output(result,transcript),
        "assertOpenCode2NestedBackgroundOutput"=>super::fixtures::opencode2_nested_background::output::assert_open_code2_nested_background_output(result,transcript),
        "assertOpenCode2PermissionOutput"=>super::fixtures::opencode2_permission::output::assert_open_code2_permission_output(result,transcript),
        "assertOpenCode2QuestionOutput"=>super::fixtures::opencode2_question::output::assert_open_code2_question_output(result,transcript),
        "assertOpenCode2ResumeAfterRestartOutput"=>super::fixtures::opencode2_resume_after_restart::output::assert_open_code2_resume_after_restart_output(result,transcript),
        "assertOpenCode2RevertOutput"=>super::fixtures::opencode2_revert::output::assert_open_code2_revert_output(result,transcript),
        "assertOpenCode2SimpleOutput"=>super::fixtures::opencode2_simple::output::assert_open_code2_simple_output(result,transcript),
        "assertOpenCode2SkillOutput"=>super::fixtures::opencode2_skill::output::assert_open_code2_skill_output(result,transcript),
        "assertOpenCode2SubagentOutput"=>super::fixtures::opencode2_subagent::output::assert_open_code2_subagent_output(result,transcript),
        "assertOpenCode2ToolCallOutput"=>super::fixtures::opencode2_tool_call::output::assert_open_code2_tool_call_output(result,transcript),
        "assertOpenCodeChildApprovalOutput"=>super::fixtures::opencode_child_approval::output::assert_open_code_child_approval_output(result,transcript),
        "assertOpenCodeRunningChildApprovalOutput"=>super::fixtures::opencode_running_child_approval::output::assert_open_code_running_child_approval_output(result,transcript),
        "assertOpenCodeSubagentOutput"=>super::fixtures::opencode_subagent::output::assert_open_code_subagent_output(result,transcript),
        "assertPiCompactionOutput"=>super::fixtures::pi_compaction::output::assert_pi_compaction_output(result,transcript),
        "assertOpenCodePlanQuestionsOutput"=>super::fixtures::plan_questions::opencode_output::assert_open_code_plan_questions_output(result,transcript),
        "assertProposedPlanCursorOutput"=>super::fixtures::proposed_plan::cursor_output::assert_proposed_plan_cursor_output(result,transcript),
        "assertPiProviderThreadResumeOutput"=>super::fixtures::provider_thread_resume::pi_output::assert_pi_provider_thread_resume_output(result,transcript),
        "assertPiSettledTokenUsage"=>super::fixtures::simple::pi_output::assert_pi_settled_token_usage(result,transcript),
        "assertPiSimpleOutput"=>super::fixtures::simple::pi_output::assert_pi_simple_output(result,transcript),
        "assertSkillInvocationCursorOutput"=>super::fixtures::skill_invocation::cursor_output::assert_skill_invocation_cursor_output(result,transcript),
        "assertStopBackgroundWorkAfterFailedTurnOutput"=>super::fixtures::stop_background_work_after_failed_turn::output::assert_stop_background_work_after_failed_turn_output(result,transcript),
        "assertCursorSubagentOutput"=>super::fixtures::subagent::cursor_output::assert_cursor_subagent_output(result,transcript),
        "assertKiroSubagentOutput"=>super::fixtures::subagent::kiro_output::assert_kiro_subagent_output(result,transcript),
        "assertMuseSubagentV2Output"=>super::fixtures::subagent_v2::muse_output::assert_muse_subagent_v2_output(result,transcript),
        "assertPiThreadRollbackOutput"=>super::fixtures::thread_rollback::pi_output::assert_pi_thread_rollback_output(result,transcript),
        "assertPiThreadRollbackAfterStopOutput"=>super::fixtures::thread_rollback_after_stop::pi_output::assert_pi_thread_rollback_after_stop_output(result,transcript),
        "assertTodoListCursorOutput"=>super::fixtures::todo_list::cursor_output::assert_todo_list_cursor_output(result,transcript),
        "assertTodoListGrokOutput"=>super::fixtures::todo_list::grok_output::assert_todo_list_grok_output(result,transcript),
        "assertToolCallDeniedWriteFxOutput"=>super::fixtures::tool_call_denied_write::fx_output::assert_tool_call_denied_write_fx_output(result,transcript),
        "assertToolCallReadOnlyCursorOutput"=>super::fixtures::tool_call_read_only::cursor_output::assert_tool_call_read_only_cursor_output(result,transcript),
        "assertToolCallReadOnlyOnRequestFxOutput"=>super::fixtures::tool_call_read_only_on_request::fx_output::assert_tool_call_read_only_on_request_fx_output(result,transcript),
        "assertToolCallReadOnlyOnRequestOutput"=>super::fixtures::tool_call_read_only_on_request::output::assert_tool_call_read_only_on_request_output(result,transcript),
        "assertToolCallReadOnlyOnRequestGrokOutput"=>super::fixtures::tool_call_read_only_on_request::output::assert_tool_call_read_only_on_request_grok_output(result,transcript),
        "assertToolCallReadOnlyOnRequestKiroOutput"=>super::fixtures::tool_call_read_only_on_request::output::assert_tool_call_read_only_on_request_kiro_output(result,transcript),
        "assertMuseTurnInterruptOutput"=>super::fixtures::turn_interrupt::muse_output::assert_muse_turn_interrupt_output(result,transcript),
        "assertTurnInterruptMidToolCursorOutput"=>super::fixtures::turn_interrupt_mid_tool::cursor_output::assert_turn_interrupt_mid_tool_cursor_output(result,transcript),
        "assertTurnInterruptMidToolKiroOutput"=>super::fixtures::turn_interrupt_mid_tool::kiro_output::assert_turn_interrupt_mid_tool_kiro_output(result,transcript),
        "assertTurnInterruptMidToolPiOutput"=>super::fixtures::turn_interrupt_mid_tool::pi_output::assert_turn_interrupt_mid_tool_pi_output(result,transcript),
        name=>match services.external {Some(assertion)=>assertion(name,result,transcript),None=>Err(crate::error::Error::Service {operation:"fixture output registry",detail:format!("output assertion {name} belongs to another provider package; supply its assertion")})},
    }
}

pub const OWNED_OUTPUT_ASSERTIONS: &[&str] = &[
    "assertAcpTitledMultiSelectOutput",
    "assertCommandOutputStreamingOutput",
    "assertFxEarlyStopOutput",
    "assertFxSubagentOutput",
    "assertGrokAutoBlockedCommandOutput",
    "assertGrokBackgroundBashOutput",
    "assertGrokBackgroundBashFastWakeOutput",
    "assertGrokBackgroundSubagentOutput",
    "assertGrokMonitorOutput",
    "assertGrokPromptErrorOutput",
    "assertGrokSubagentLineageOutput",
    "assertKiroCompactionOutput",
    "assertKiroEffortOutput",
    "assertKiroForkFromTurnOutput",
    "assertKiroMessageSteeringOutput",
    "assertKiroModelSwitchOutput",
    "assertKiroSupervisedWriteOutput",
    "assertKiroFullAccessWriteOutput",
    "assertKiroSubagentInterruptOutput",
    "assertMalformedSessionUpdateOutput",
    "assertCursorMessageSteeringOutput",
    "assertGrokMessageSteeringOutput",
    "assertPiMessageSteeringOutput",
    "assertMuseMultiTurnOutput",
    "assertPiMultiTurnOutput",
    "assertMusePermissionOutput",
    "assertMuseSubagentApprovalOutput",
    "assertMuseSubagentInterruptOutput",
    "assertMuseWorkflowOutput",
    "assertOpenCode2BackgroundOutput",
    "assertOpenCode2CommandOutput",
    "assertOpenCode2CompactionOutput",
    "assertOpenCode2InboxOutput",
    "assertOpenCode2InterruptOutput",
    "assertOpenCode2MissingStepUsageOutput",
    "assertOpenCode2NestedBackgroundOutput",
    "assertOpenCode2PermissionOutput",
    "assertOpenCode2QuestionOutput",
    "assertOpenCode2ResumeAfterRestartOutput",
    "assertOpenCode2RevertOutput",
    "assertOpenCode2SimpleOutput",
    "assertOpenCode2SkillOutput",
    "assertOpenCode2SubagentOutput",
    "assertOpenCode2ToolCallOutput",
    "assertOpenCodeChildApprovalOutput",
    "assertOpenCodeRunningChildApprovalOutput",
    "assertOpenCodeSubagentOutput",
    "assertPiCompactionOutput",
    "assertOpenCodePlanQuestionsOutput",
    "assertProposedPlanCursorOutput",
    "assertPiProviderThreadResumeOutput",
    "assertPiSettledTokenUsage",
    "assertPiSimpleOutput",
    "assertSkillInvocationCursorOutput",
    "assertStopBackgroundWorkAfterFailedTurnOutput",
    "assertCursorSubagentOutput",
    "assertKiroSubagentOutput",
    "assertMuseSubagentV2Output",
    "assertPiThreadRollbackOutput",
    "assertPiThreadRollbackAfterStopOutput",
    "assertTodoListCursorOutput",
    "assertTodoListGrokOutput",
    "assertToolCallDeniedWriteFxOutput",
    "assertToolCallReadOnlyCursorOutput",
    "assertToolCallReadOnlyOnRequestFxOutput",
    "assertToolCallReadOnlyOnRequestOutput",
    "assertToolCallReadOnlyOnRequestGrokOutput",
    "assertToolCallReadOnlyOnRequestKiroOutput",
    "assertMuseTurnInterruptOutput",
    "assertTurnInterruptMidToolCursorOutput",
    "assertTurnInterruptMidToolKiroOutput",
    "assertTurnInterruptMidToolPiOutput",
];

#[cfg(feature = "acp-adapter")]
pub mod native {
    use super::*;
    use crate::{
        api::{Clock, IdGenerator},
        error::{Error, Result},
        ports::{
            acp_replay::testkit::{
                deterministic_runtime::{DEFAULT_RANDOM_SEED, FIXTURE_TIME, SeededIds},
                fixtures::shared::{
                    OrchestratorFixtureInput, OrchestratorReplayFixture,
                    ProviderOrchestratorReplayVariant,
                },
                orchestrator_scenario::ScenarioResult,
                provider_replay_harness::{
                    ReplayFixtureHostFactory, ReplayFixtureHostInput,
                    scenario_services_for_runtime, start_fixture_runtime,
                },
            },
            sessions_recovery::provider_adapter_registry::AdapterRegistry,
        },
        provider::{ProviderAdapter, ProviderEvent},
        testing::{ManualClock, TestDatabase, replay::ReplayGate},
    };
    use std::{
        path::{Path, PathBuf},
        sync::Arc,
    };

    pub type ProviderEventObserver = Arc<dyn Fn(&ProviderEvent) + Send + Sync>;
    pub type ProviderBatchObserver = Arc<dyn Fn(usize) + Send + Sync>;

    pub struct FixtureAdapterInput {
        pub transcript: ReplayTranscript,
        pub variant: ProviderOrchestratorReplayVariant,
        pub workspace: PathBuf,
        pub clock: Arc<dyn Clock>,
        pub ids: Arc<dyn IdGenerator>,
        pub gate: ReplayGate,
        pub observe: Option<ProviderEventObserver>,
    }
    #[async_trait::async_trait]
    pub trait FixtureAdapter: Send + Sync {
        fn adapter(&self) -> Arc<dyn ProviderAdapter>;
        fn assert_complete(&self) -> Result<()>;
        async fn shutdown(&self) -> Result<()>;
    }
    // Local shim: the native factory must preserve its strict transport and any supplied event tap.
    #[async_trait::async_trait]
    pub trait FixtureAdapterFactory: Send + Sync {
        async fn create(&self, input: FixtureAdapterInput) -> Result<Arc<dyn FixtureAdapter>>;
        async fn assert_live(
            &self,
            _services: &super::super::orchestrator_scenario::ScenarioServices,
            _result: &ScenarioResult,
        ) -> Result<()> {
            Ok(())
        }
        fn runtime_instructions(&self, harness: &str, model: &str) -> String;
        fn materialize_workspace(
            &self,
            transcript: ReplayTranscript,
            workspace: &Path,
        ) -> Result<ReplayTranscript>;
        fn build_input(
            &self,
            fixture: &OrchestratorReplayFixture,
        ) -> Result<OrchestratorFixtureInput> {
            fixture.build_owned_input()
        }
        fn assert_output(
            &self,
            name: &str,
            result: &ScenarioResult,
            transcript: &ReplayTranscript,
        ) -> Result<()>;
    }
    #[derive(Default)]
    pub struct FixtureObservations {
        pub provider_events: Option<ProviderEventObserver>,
        pub provider_batches: Option<ProviderBatchObserver>,
    }

    pub async fn run_fixture_case(
        fixture: &OrchestratorReplayFixture,
        variant: ProviderOrchestratorReplayVariant,
        factory: &dyn FixtureAdapterFactory,
        host: &dyn ReplayFixtureHostFactory,
        observations: FixtureObservations,
    ) -> Result<ScenarioResult> {
        let workspace = tempfile::Builder::new()
            .prefix("t3-orchestration-fixture-")
            .tempdir()?;
        let input = factory.build_input(fixture)?;
        if let Some(files) = &input.workspace_files {
            for (path, text) in files {
                let path = workspace.path().join(path);
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(path, text)?;
            }
        }
        host.prepare_workspace(&fixture.name, workspace.path())
            .await?;
        let prepared = prepare_fixture_case(
            fixture,
            variant,
            input,
            workspace.path().into(),
            |harness, model| factory.runtime_instructions(harness, model),
            |transcript, workspace| factory.materialize_workspace(transcript, workspace),
        )?;
        let clock = Arc::new(ManualClock::new(FIXTURE_TIME));
        let ids = Arc::new(SeededIds::new(DEFAULT_RANDOM_SEED));
        let database = TestDatabase::temporary()?;
        let labels: std::collections::BTreeSet<_> = prepared
            .scenario
            .steps
            .iter()
            .flatten()
            .filter_map(|step| {
                use crate::ports::acp_replay::testkit::orchestrator_scenario::ScenarioStep;
                match step {
                    ScenarioStep::ReleaseReplayGate(label)
                    | ScenarioStep::ReleaseReplayGateAfterWaiting { label, .. } => {
                        Some(label.clone())
                    }
                    _ => None,
                }
            })
            .collect();
        let gate = ReplayGate::new(labels).map_err(|error| Error::Service {
            operation: "fixture replay gates",
            detail: error.to_string(),
        })?;
        let adapter = factory
            .create(FixtureAdapterInput {
                transcript: prepared.transcript.clone(),
                variant: prepared.variant.clone(),
                workspace: workspace.path().into(),
                clock: clock.clone(),
                ids: ids.clone(),
                gate: gate.clone(),
                observe: observations.provider_events,
            })
            .await?;
        let runtime = match start_fixture_runtime(
            ReplayFixtureHostInput {
                database: database.stores.clone(),
                providers: Arc::new(AdapterRegistry::from_adapters(vec![adapter.adapter()])),
                clock: clock.clone(),
                ids,
                workspace: workspace.path().into(),
                runtime_policy_override: prepared.runtime_policy_override.clone(),
                run_continuation_worker: prepared.variant.run_continuation_worker.unwrap_or(false),
                provider_batch_observer: observations.provider_batches,
            },
            host,
        )
        .await
        {
            Ok(runtime) => runtime,
            Err(error) => {
                let _ = adapter.shutdown().await;
                return Err(error);
            }
        };
        let result = async {
            let create = prepared.scenario.commands.first().ok_or(Error::Unsupported("fixture thread.create command"))?;
            let command = serde_json::to_value(create)?;
            runtime.project_command(serde_json::from_value(serde_json::json!({
                "type":"project.create", "commandId":format!("fixture-project:{}",fixture.name),
                "projectId":command["projectId"], "title":format!("Replay fixture: {}",fixture.name),
                "workspaceRoot":workspace.path(),
            }))?).await?;
            let services = scenario_services_for_runtime(runtime.clone(), database.stores.clone(), clock, gate);
            let assertion = prepared.variant.assert_output.clone();
            let result = run_prepared_fixture(prepared, &services, |result, transcript| factory.assert_output(&assertion, result, transcript)).await?;
            factory.assert_live(&services, &result).await?;
            Ok::<_, Error>(result)
        }.await;
        let shutdown = runtime.shutdown().await;
        let complete = adapter.assert_complete();
        let adapter_shutdown = adapter.shutdown().await;
        let result = result?;
        shutdown?;
        complete?;
        adapter_shutdown?;
        Ok(result)
    }

    pub async fn run_fixture_suite(
        driver: &str,
        factory: &dyn FixtureAdapterFactory,
        host: &dyn ReplayFixtureHostFactory,
    ) -> Result<usize> {
        let mut count = 0;
        for (fixture, variant) in fixture_cases(driver)? {
            run_fixture_case(
                &fixture,
                variant,
                factory,
                host,
                FixtureObservations::default(),
            )
            .await?;
            count += 1;
        }
        Ok(count)
    }
}
