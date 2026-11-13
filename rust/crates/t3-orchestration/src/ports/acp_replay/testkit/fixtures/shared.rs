use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use t3_contracts::{
    chat_attachment::ChatAttachment,
    orchestration_v2::{OrchestrationV2Actor, OrchestrationV2CreationSource},
    provider_policy::{
        ProviderApprovalDecision, ProviderInteractionMode, ProviderUserInputAnswers, RuntimeMode,
    },
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrchestratorFixtureInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interaction_mode: Option<ProviderInteractionMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_mode: Option<RuntimeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_files: Option<BTreeMap<String, String>>,
    pub steps: Vec<OrchestratorFixtureInputStep>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum OrchestratorFixtureInputStep {
    Message {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attachments: Option<Vec<ChatAttachment>>,
    },
    Fork {
        from_run_index: u64,
        text: String,
    },
    QueueMessage {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attachments: Option<Vec<ChatAttachment>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        created_by: Option<OrchestrationV2Actor>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        creation_source: Option<OrchestrationV2CreationSource>,
    },
    CancelQueuedRun {
        target_run_index: u64,
    },
    AwaitRunStatus {
        target_run_index: u64,
        status: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        wait_for_turn_item_type: Option<String>,
    },
    FinishHeldRun {
        target_run_index: u64,
        status: String,
    },
    CaptureShellSnapshot {
        key: String,
    },
    ReleaseReplayGate {
        label: String,
    },
    Steer {
        text: String,
        target_run_index: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attachments: Option<Vec<ChatAttachment>>,
    },
    Restart {
        text: String,
        target_run_index: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attachments: Option<Vec<ChatAttachment>>,
    },
    Interrupt {
        target_run_index: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        wait_for_turn_item_type: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        wait_for_turn_item_status: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        in_subagent_thread: Option<bool>,
    },
    StopBackgroundWork {
        target_run_index: u64,
    },
    ReleaseReplayGateAfterWaiting {
        label: String,
        target_run_index: u64,
    },
    ApproveNextRuntimeRequest {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        decision: Option<ProviderApprovalDecision>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        shell_snapshot_key_while_pending: Option<String>,
    },
    AnswerNextUserInputRequest {
        answers: ProviderUserInputAnswers,
    },
    Rollback {
        checkpoint_scope_suffix: String,
        checkpoint_suffix: String,
    },
    AdvanceClock {
        duration: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        releases_provider_session: Option<bool>,
    },
    SetModel {
        model: String,
    },
}

pub type InputBuilder = fn() -> crate::error::Result<OrchestratorFixtureInput>;

pub struct FixtureInputExport {
    pub source: &'static str,
    pub name: &'static str,
    pub evaluate: fn() -> crate::error::Result<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderOrchestratorReplayVariant {
    pub driver: t3_contracts::provider_instance::ProviderDriverKind,
    pub transcript_file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recorded_scenario: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_entries_through_label: Option<String>,
    pub model_selection: t3_contracts::model_selection::ModelSelection,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_policy_override: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_continuation_worker: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_absent_workspace_paths: Option<Vec<String>>,
    pub assert_output: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrchestratorReplayFixture {
    pub name: String,
    pub build_input: String,
    pub providers: Vec<ProviderOrchestratorReplayVariant>,
}
impl OrchestratorReplayFixture {
    pub fn build_owned_input(&self) -> crate::error::Result<OrchestratorFixtureInput> {
        let builder = super::index::INPUT_BUILDERS
            .iter()
            .find(|(_, name, _)| *name == self.build_input)
            .ok_or_else(|| crate::error::Error::Service {
                operation: "replay fixture input registry",
                detail: format!(
                    "input {} is owned by another provider package; supply its builder",
                    self.build_input
                ),
            })?;
        builder.2()
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MaterializedFixtureInput {
    pub commands: Vec<t3_contracts::orchestration_v2::OrchestrationV2ServerCommand>,
    pub steps: Vec<serde_json::Value>,
    pub projection_thread_ids: Vec<t3_contracts::base_schemas::ThreadId>,
}
impl MaterializedFixtureInput {
    pub fn into_scenario(
        self,
        name: impl Into<String>,
    ) -> crate::error::Result<super::super::orchestrator_scenario::Scenario> {
        Ok(super::super::orchestrator_scenario::Scenario {
            name: name.into(),
            commands: self.commands,
            steps: Some(
                self.steps
                    .iter()
                    .map(super::super::orchestrator_scenario::scenario_step_from_value)
                    .collect::<Result<_, _>>()?,
            ),
            projection_thread_ids: self.projection_thread_ids,
        })
    }
}

struct Materializer<'a> {
    scenario: &'a str,
    ids: &'a super::super::deterministic_runtime::SeededIds,
    commands: Vec<t3_contracts::orchestration_v2::OrchestrationV2ServerCommand>,
    steps: Vec<serde_json::Value>,
}
impl Materializer<'_> {
    fn command_id(&self, name: &str) -> String {
        self.ids
            .allocate("command", &["fixture", self.scenario, name])
    }
    fn dispatch(
        &mut self,
        command: serde_json::Value,
        key: Option<&str>,
        clock: bool,
    ) -> crate::error::Result<()> {
        use serde_json::json;
        self.commands.push(serde_json::from_value(command.clone())?);
        let mut step = json!({"type":"dispatch","command":command,"await":key.is_none()});
        if let Some(key) = key {
            step["key"] = json!(key);
        }
        self.steps.push(step);
        if clock {
            self.tick();
        }
        Ok(())
    }
    fn tick(&mut self) {
        self.steps
            .push(serde_json::json!({"type":"advance_clock","duration":"1 millis"}));
    }
    fn idle(&mut self, thread: &str) {
        self.steps
            .push(serde_json::json!({"type":"await_thread_idle","threadId":thread}));
    }
    fn await_key(&mut self, key: String, active: &mut Vec<String>) {
        active.retain(|candidate| candidate != &key);
        self.steps
            .push(serde_json::json!({"type":"await","key":key}));
    }
    fn message(
        &self,
        command_name: &str,
        thread: &str,
        ordinal: u64,
        step: &serde_json::Value,
        model: &serde_json::Value,
        mode: serde_json::Value,
    ) -> serde_json::Value {
        use serde_json::json;
        let command_id = self.command_id(command_name);
        let message_id = self.ids.allocate(
            "message",
            &["thread", thread, "ordinal", &ordinal.to_string()],
        );
        json!({"type":"message.dispatch","createdBy":step.get("createdBy").cloned().unwrap_or(json!("user")),
            "creationSource":step.get("creationSource").cloned().unwrap_or(json!("web")),"commandId":command_id,"threadId":thread,
            "messageId":message_id,"text":step["text"],"attachments":step.get("attachments").cloned().unwrap_or(json!([])),
            "modelSelection":model,"dispatchMode":mode})
    }
}

pub fn materialize_fixture_input(
    scenario: &str,
    input: &OrchestratorFixtureInput,
    driver: &str,
    model_selection: &t3_contracts::model_selection::ModelSelection,
    ids: &super::super::deterministic_runtime::SeededIds,
) -> crate::error::Result<MaterializedFixtureInput> {
    use crate::ports::decisions::orchestrator::derived_id;
    use serde_json::{Value, json};
    let project = ids.allocate("project", &["fixture", scenario]);
    let thread = ids.allocate("thread", &["fixture", scenario, "project", &project]);
    let original_model = serde_json::to_value(model_selection)?;
    let mut model = original_model.clone();
    let mut m = Materializer {
        scenario,
        ids,
        commands: Vec::new(),
        steps: Vec::new(),
    };
    let mut active = Vec::<String>::new();
    let mut message_index = 0_u64;
    let mut run_index = 0_u64;
    let mut fork_thread = None;
    let run_id =
        |ordinal: u64| derived_id("run", &["thread", &thread, "ordinal", &ordinal.to_string()]);
    m.dispatch(json!({"type":"thread.create","createdBy":"user","creationSource":"web","commandId":m.command_id("thread-create"),
        "threadId":thread,"projectId":project,"title":format!("Replay fixture: {scenario}"),"modelSelection":model,
        "runtimeMode":input.runtime_mode.as_ref().map(|value|json!(value)).unwrap_or(json!("full-access")),
        "interactionMode":input.interaction_mode.as_ref().map(|value|json!(value)).unwrap_or(json!("default")),"branch":null,"worktreePath":null}),None,true)?;
    let input_steps: Vec<Value> = input
        .steps
        .iter()
        .map(serde_json::to_value)
        .collect::<Result<_, _>>()?;
    for (index, step) in input_steps.iter().enumerate() {
        let kind = step["type"].as_str().unwrap_or_default();
        let next = input_steps.get(index + 1).unwrap_or(&Value::Null);
        let next_kind = next["type"].as_str().unwrap_or_default();
        let answers_next = matches!(
            next_kind,
            "approve_next_runtime_request" | "answer_next_user_input_request"
        );
        let target = step["targetRunIndex"].as_u64().unwrap_or_default();
        match kind {
            "message"|"queue_message"=> {
                message_index+=1; run_index+=1;
                let background=kind=="message" && (answers_next || matches!(next_kind,"queue_message"|"finish_held_run") ||
                    (matches!(next_kind,"interrupt"|"restart"|"release_replay_gate_after_waiting") && next["targetRunIndex"]==run_index) ||
                    (next_kind=="await_run_status" && next["targetRunIndex"].as_u64().is_some_and(|target|target>run_index)));
                let key=format!("run:{run_index}");
                let command_name=format!("{}-{message_index}",if kind=="message" {"message"} else {"queue-message"});
                let mode=json!({"type":if kind=="message" {"start_immediately"} else {"queue_after_active"}});
                let command=m.message(&command_name,&thread,message_index,step,&model,mode);
                m.dispatch(command,background.then_some(key.as_str()),true)?;
                if background { active.push(key); }
                else if kind=="queue_message" {
                    if next_kind!="queue_message" && !(next_kind=="cancel_queued_run" && next["targetRunIndex"]==run_index) {
                        let key=active.last().cloned().unwrap_or_else(||format!("run:{}",run_index-1));
                        m.await_key(key,&mut active); m.idle(&thread);
                    }
                } else if !(matches!(next_kind,"steer"|"restart") && next["targetRunIndex"]==run_index) { m.idle(&thread); }
            }
            "cancel_queued_run"=>m.dispatch(json!({"type":"queued-run.cancel","commandId":m.command_id(&format!("cancel-queued-run-{target}")),"threadId":thread,"runId":run_id(target)}),None,true)?,
            "await_run_status"|"finish_held_run"=> {
                m.steps.push(json!({"type":kind,"threadId":thread,"runId":run_id(target),"status":step["status"]}));
                if let Some(item)=step.get("waitForTurnItemType") { m.steps.push(json!({"type":"await_run_turn_item","threadId":thread,"runId":run_id(target),"itemType":item})); }
            }
            "capture_shell_snapshot"|"release_replay_gate"=>m.steps.push(step.clone()),
            "approve_next_runtime_request"|"answer_next_user_input_request"=> {
                let name=if kind=="approve_next_runtime_request" {"approve-runtime-request"} else {"answer-user-input-request"};
                let command_id=m.command_id(&format!("{name}-{message_index}"));
                let request_id=ids.allocate("runtime-request",&["provider",driver,"native-request",&format!("fixture-placeholder-{message_index}")]);
                let mut command=json!({"type":"runtime-request.respond","commandId":command_id,"threadId":thread,"requestId":request_id});
                let field=if kind=="approve_next_runtime_request" {"decision"} else {"answers"};
                command[field]=step.get(field).cloned().unwrap_or(json!("accept"));
                m.dispatch(command.clone(),None,false)?;
                command["type"]=json!("respond_to_next_runtime_request"); command.as_object_mut().expect("fixture structure validated above").remove("requestId");
                if let Some(key)=step.get("shellSnapshotKeyWhilePending") { command["shellSnapshotKeyWhilePending"]=key.clone(); }
                *m.steps.last_mut().expect("fixture structure validated above")=command; m.tick();
                if !answers_next { m.idle(&thread); }
            }
            "steer"|"restart"=> {
                message_index+=1;
                m.steps.push(json!({"type":"await_run_steerable","threadId":thread,"runId":run_id(target)}));
                let command=m.message(&format!("{kind}-{message_index}"),&thread,message_index,step,&model,json!({"type":format!("{kind}_active"),"targetRunId":run_id(target)}));
                m.dispatch(command,None,true)?;
                if !answers_next {
                    let key=format!("run:{target}");
                    if active.contains(&key) { m.await_key(key,&mut active); }
                    m.idle(&thread);
                }
            }
            "interrupt"=> {
                m.steps.push(json!({"type":"await_run_steerable","threadId":thread,"runId":run_id(target)}));
                if let Some(item)=step.get("waitForTurnItemType") {
                    let mut item_step=json!({"type":"await_run_turn_item","threadId":thread,"runId":run_id(target),"itemType":item});
                    if let Some(status)=step.get("waitForTurnItemStatus") { item_step["itemStatus"]=status.clone(); }
                    if step["inSubagentThread"]==true { item_step["inSubagentThreads"]=json!(true); }
                    m.steps.push(item_step);
                }
                m.dispatch(json!({"type":"run.interrupt","commandId":m.command_id(&format!("interrupt-{target}")),"threadId":thread,"runId":run_id(target)}),None,false)?;
                let key=format!("run:{target}"); if active.contains(&key) { m.await_key(key,&mut active); }
                m.tick(); m.idle(&thread);
            }
            "stop_background_work"=> {
                m.idle(&thread); m.steps.push(json!({"type":"capture_shell_snapshot","key":"before-stop"}));
                m.dispatch(json!({"type":"run.interrupt","commandId":m.command_id(&format!("stop-background-work-{target}")),"threadId":thread,"runId":run_id(target),"holdQueue":true}),None,false)?;
                m.steps.push(json!({"type":"await_no_background_work","threadId":thread}));
            }
            "release_replay_gate_after_waiting"=>m.steps.push(json!({"type":kind,"label":step["label"],"threadId":thread,"runId":run_id(target)})),
            "advance_clock"=> {
                let mut clock=json!({"type":kind,"duration":step["duration"]});
                if step["releasesProviderSession"]==true { clock["awaitProviderSessionRelease"]=json!(thread); }
                m.steps.push(clock);
            }
            "set_model"=> {
                model=original_model.clone(); model["model"]=step["model"].clone();
                m.dispatch(json!({"type":"thread.model-selection.set","commandId":m.command_id(&format!("set-model-{}",step["model"].as_str().unwrap_or_default())),"threadId":thread,"modelSelection":model}),None,true)?;
            }
            "fork"=> {
                let target_thread=ids.allocate("thread",&["fixture",&format!("{scenario}-fork"),"project",&project]);
                m.dispatch(json!({"type":"thread.fork","createdBy":"user","creationSource":"web","commandId":m.command_id("thread-fork"),"sourceThreadId":thread,"targetThreadId":target_thread,
                    "sourcePoint":{"type":"run","runId":run_id(step["fromRunIndex"].as_u64().unwrap_or_default())},"title":"Forked thread"}),None,true)?;
                let command=m.message("fork-message",&target_thread,1,step,&model,json!({"type":"start_immediately"}));
                m.dispatch(command,None,true)?; m.idle(&target_thread); fork_thread=Some(target_thread);
            }
            "rollback"=> {
                let scope=derived_id("checkpoint-scope",&["thread",&thread,"name",step["checkpointScopeSuffix"].as_str().unwrap_or_default()]);
                let suffix=step["checkpointSuffix"].as_str().unwrap_or_default();
                m.dispatch(json!({"type":"checkpoint.rollback","restoreFiles":false,"commandId":m.command_id(&format!("rollback-{suffix}")),"threadId":thread,"scopeId":scope,
                    "checkpointId":derived_id("checkpoint",&["scope",&scope,"name",suffix])}),None,true)?;
            }
            _=>return Err(crate::error::Error::Service {operation:"materialize replay fixture",detail:format!("unknown step {kind}")}),
        }
    }
    if !active.is_empty() {
        m.steps.push(json!({"type":"await_all"}));
        m.idle(&thread);
    }
    let mut projection_thread_ids = vec![t3_contracts::base_schemas::ThreadId(thread)];
    if let Some(thread) = fork_thread {
        projection_thread_ids.push(t3_contracts::base_schemas::ThreadId(thread));
    }
    Ok(MaterializedFixtureInput {
        commands: m.commands,
        steps: m.steps,
        projection_thread_ids,
    })
}

pub fn require(condition: bool, detail: impl Into<String>) -> crate::error::Result<()> {
    if condition {
        Ok(())
    } else {
        Err(crate::error::Error::Service {
            operation: "replay fixture assertion",
            detail: detail.into(),
        })
    }
}
pub fn assert_includes_fields(
    actual: &serde_json::Value,
    expected: &serde_json::Value,
    detail: &str,
) -> crate::error::Result<()> {
    require(
        expected.as_object().is_some_and(|fields| {
            fields
                .iter()
                .all(|(key, value)| actual.get(key) == Some(value))
        }),
        detail,
    )
}
pub fn assert_plan_questions_fixture_output(
    result: &super::super::orchestrator_scenario::ScenarioResult,
    transcript: &crate::testing::replay::ReplayTranscript,
) -> crate::error::Result<()> {
    assert_base_projection(result, transcript, 1, None, Some(&["completed"]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_visible_turn_items_mirror_local_turn_items(&p)?;
    assert_run_ordinals(&p, &[1])?;
    assert_execution_node_kinds(
        &p,
        &["root_turn", "user_input_request", "assistant_message"],
    )?;
    assert_runtime_request_counts(&p, 1, Some(1))?;
    assert_runtime_request_kinds(&p, &["user_input"])?;
    assert_all_runtime_requests_resolved(&p)?;
    assert_turn_item_types(
        &p,
        &["user_message", "user_input_request", "assistant_message"],
    )?;
    assert_user_messages_include(&p, &[PLAN_QUESTIONS_PROMPT])?;
    assert_assistant_text_includes(&p, "plan questions fixture complete")
}
pub fn rows<'a>(
    value: &'a serde_json::Value,
    key: &str,
) -> impl DoubleEndedIterator<Item = &'a serde_json::Value> {
    value[key].as_array().into_iter().flatten()
}
pub fn values(value: &serde_json::Value, key: &str, field: &str) -> Vec<serde_json::Value> {
    rows(value, key).map(|row| row[field].clone()).collect()
}
pub fn field<'a>(value: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
    value.get(key)
}
pub fn replay_frames(
    transcript: &crate::testing::replay::ReplayTranscript,
) -> Vec<(bool, &serde_json::Value)> {
    use crate::testing::replay::ReplayEntry;
    transcript
        .entries
        .iter()
        .filter_map(|entry| match entry {
            ReplayEntry::ExpectOutbound(entry) => Some((false, &entry.frame)),
            ReplayEntry::EmitInbound(entry) => Some((true, &entry.frame)),
            ReplayEntry::RuntimeExit(_) => None,
        })
        .collect()
}
pub fn assert_simple_fixture_output(
    result: &super::super::orchestrator_scenario::ScenarioResult,
    transcript: &crate::testing::replay::ReplayTranscript,
) -> crate::error::Result<()> {
    assert_base_projection(result, transcript, 1, None, Some(&["completed"]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_visible_turn_items_mirror_local_turn_items(&p)?;
    assert_run_ordinals(&p, &[1])?;
    assert_execution_node_kinds(&p, &["root_turn", "assistant_message"])?;
    assert_conversation_message_roles(&p, &["user", "assistant"])?;
    assert_turn_item_types(&p, &["user_message", "assistant_message"])?;
    assert_user_messages_include(&p, &[SIMPLE_PROMPT])?;
    assert_assistant_text_includes(&p, "fixture simple ok")
}
pub fn assert_multi_turn_fixture_output(
    result: &super::super::orchestrator_scenario::ScenarioResult,
    transcript: &crate::testing::replay::ReplayTranscript,
) -> crate::error::Result<()> {
    assert_base_projection(
        result,
        transcript,
        2,
        None,
        Some(&["completed", "completed"]),
    )?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_visible_turn_items_mirror_local_turn_items(&p)?;
    assert_run_ordinals(&p, &[1, 2])?;
    assert_conversation_message_roles(&p, &["user", "assistant", "user", "assistant"])?;
    assert_turn_item_types(&p, &["user_message", "assistant_message"])?;
    assert_user_messages_include(&p, &[MULTI_TURN_FIRST_PROMPT, MULTI_TURN_SECOND_PROMPT])?;
    require(
        rows(&p, "turnItems")
            .filter(|item| item["type"] == "user_message")
            .count()
            == 2,
        "expected two user messages",
    )?;
    assert_assistant_text_includes(&p, "first fixture turn complete")?;
    assert_assistant_text_includes(&p, "second fixture turn complete")
}
pub fn projection_for<'a>(
    result: &'a super::super::orchestrator_scenario::ScenarioResult,
    scenario: &str,
) -> crate::error::Result<&'a t3_contracts::orchestration_v2::OrchestrationV2ThreadProjection> {
    let mut roots = result
        .projections
        .values()
        .filter(|projection| projection.thread.lineage.parent_thread_id.is_none());
    let root = roots.next();
    require(
        root.is_some() && roots.next().is_none(),
        format!("expected one root projection for {scenario}"),
    )?;
    root.ok_or_else(|| crate::error::Error::Service {
        operation: "replay fixture assertion",
        detail: format!("missing root projection for {scenario}"),
    })
}
pub fn projection_value_for(
    result: &super::super::orchestrator_scenario::ScenarioResult,
    scenario: &str,
) -> crate::error::Result<serde_json::Value> {
    Ok(serde_json::to_value(projection_for(result, scenario)?)?)
}
pub fn assert_base_projection(
    result: &super::super::orchestrator_scenario::ScenarioResult,
    transcript: &crate::testing::replay::ReplayTranscript,
    run_count: usize,
    provider_turn_count_at_least: Option<usize>,
    statuses: Option<&[&str]>,
) -> crate::error::Result<()> {
    use serde_json::json;
    let projection = projection_value_for(result, &transcript.scenario.0)?;
    require(
        projection["thread"]["providerInstanceId"] == transcript.provider.0,
        "provider instance differs from replay driver",
    )?;
    assert_run_provider_turn_cardinality(&projection, run_count, provider_turn_count_at_least)?;
    require(
        rows(&projection, "providerThreads").next().is_some(),
        "missing provider thread",
    )?;
    require(!result.domain_events.is_empty(), "missing domain events")?;
    require(
        result.stored_events.len() == result.domain_events.len(),
        "stored/domain event counts differ",
    )?;
    let mut previous_sequence = 0;
    for (stored, domain) in result.stored_events.iter().zip(&result.domain_events) {
        require(
            stored.sequence > previous_sequence,
            "stored event sequences must be positive and strictly increasing",
        )?;
        previous_sequence = stored.sequence;
        require(
            serde_json::to_value(&stored.event)?["id"] == serde_json::to_value(domain)?["id"],
            "stored/domain event IDs differ",
        )?;
    }
    if let Some(statuses) = statuses {
        require(
            values(&projection, "runs", "status")
                == json!(statuses)
                    .as_array()
                    .expect("fixture structure validated above")
                    .as_slice(),
            "unexpected run statuses",
        )?;
    }
    Ok(())
}
pub fn assert_run_ordinals(
    projection: &serde_json::Value,
    expected: &[u64],
) -> crate::error::Result<()> {
    require(
        serde_json::json!(values(projection, "runs", "ordinal")) == serde_json::json!(expected),
        "unexpected run ordinals",
    )
}
pub fn assert_run_provider_turn_cardinality(
    projection: &serde_json::Value,
    root_run_count: usize,
    provider_turn_count_at_least: Option<usize>,
) -> crate::error::Result<()> {
    require(
        rows(projection, "runs").count() == root_run_count,
        "unexpected app run count",
    )?;
    require(
        rows(projection, "providerTurns").count()
            >= provider_turn_count_at_least.unwrap_or(root_run_count),
        "missing provider turns",
    )
}
pub fn assert_no_extra_app_runs_for_provider_children(
    projection: &serde_json::Value,
    expected_app_runs: usize,
) -> crate::error::Result<()> {
    require(
        rows(projection, "runs").count() == expected_app_runs,
        "provider child activity created extra app runs",
    )
}
pub fn assert_semantic_projection_integrity(
    projection: &serde_json::Value,
) -> crate::error::Result<()> {
    for run in rows(projection, "runs") {
        require(
            !run["rootNodeId"].is_null()
                && rows(projection, "nodes")
                    .any(|node| node["id"] == run["rootNodeId"] && node["kind"] == "root_turn"),
            format!("run {} is missing its root node", run["id"]),
        )?;
    }
    let roots: Vec<_> = rows(projection, "nodes")
        .filter(|node| node["kind"] == "root_turn")
        .collect();
    require(
        roots.len() >= rows(projection, "runs").count(),
        "too few root nodes",
    )?;
    for root in roots {
        require(
            root["countsForRun"] == true,
            "root must count for its app run",
        )?;
    }
    for turn in rows(projection, "providerTurns") {
        for (field, table) in [("nodeId", "nodes"), ("providerThreadId", "providerThreads")] {
            require(
                rows(projection, table).any(|row| row["id"] == turn[field]),
                format!("provider turn {} has missing {field}", turn["id"]),
            )?;
        }
    }
    let mut previous = None;
    for item in rows(projection, "turnItems") {
        let ordinal = item["ordinal"].as_f64().unwrap_or(f64::NAN);
        require(
            previous.is_none_or(|previous| ordinal >= previous),
            "turn items are out of order",
        )?;
        previous = Some(ordinal);
    }
    for (table, fields) in [
        (
            "turnItems",
            &[
                ("runId", "runs"),
                ("nodeId", "nodes"),
                ("providerTurnId", "providerTurns"),
            ][..],
        ),
        ("messages", &[("runId", "runs"), ("nodeId", "nodes")][..]),
        (
            "runtimeRequests",
            &[("providerTurnId", "providerTurns")][..],
        ),
    ] {
        for row in rows(projection, table) {
            for (field, target) in fields {
                if !row[*field].is_null() {
                    require(
                        rows(projection, target).any(|target| target["id"] == row[*field]),
                        format!("{table} {} references missing {field}", row["id"]),
                    )?;
                }
            }
        }
    }
    for request in rows(projection, "runtimeRequests") {
        let node = rows(projection, "nodes").find(|node| node["id"] == request["nodeId"]);
        require(node.is_some(), "runtime request references missing node")?;
        if matches!(
            request["kind"].as_str(),
            Some("command" | "file-read" | "file-change")
        ) {
            require(
                node.is_some_and(|node| node["kind"] == "approval_request"),
                "approval request must reference approval node",
            )?;
        }
    }
    Ok(())
}
pub fn assert_visible_turn_items_mirror_local_turn_items(
    projection: &serde_json::Value,
) -> crate::error::Result<()> {
    require(
        rows(projection, "visibleTurnItems").count() == rows(projection, "turnItems").count(),
        "visible/local item counts differ",
    )?;
    for (index, (visible, item)) in rows(projection, "visibleTurnItems")
        .zip(rows(projection, "turnItems"))
        .enumerate()
    {
        require(
            visible["position"] == serde_json::json!(index)
                && visible["visibility"] == "local"
                && visible["sourceThreadId"] == item["threadId"]
                && visible["sourceItemId"] == item["id"]
                && visible["item"] == *item,
            "visible item differs from its canonical item",
        )?;
    }
    Ok(())
}
pub fn assert_execution_node_kinds(
    projection: &serde_json::Value,
    expected: &[&str],
) -> crate::error::Result<()> {
    assert_field_includes(projection, "nodes", "kind", expected)
}
pub fn assert_turn_item_types(
    projection: &serde_json::Value,
    expected: &[&str],
) -> crate::error::Result<()> {
    assert_field_includes(projection, "turnItems", "type", expected)
}
fn assert_field_includes(
    projection: &serde_json::Value,
    table: &str,
    field: &str,
    expected: &[&str],
) -> crate::error::Result<()> {
    for expected in expected {
        require(
            rows(projection, table).any(|row| row[field] == *expected),
            format!("missing {table}.{field}={expected}"),
        )?;
    }
    Ok(())
}
pub fn assert_field_sequence(
    projection: &serde_json::Value,
    table: &str,
    field: &str,
    expected: &[&str],
) -> crate::error::Result<()> {
    require(
        serde_json::json!(values(projection, table, field)) == serde_json::json!(expected),
        format!("unexpected {table}.{field} sequence"),
    )
}
pub fn assert_turn_item_type_sequence(
    projection: &serde_json::Value,
    expected: &[&str],
) -> crate::error::Result<()> {
    assert_field_sequence(projection, "turnItems", "type", expected)
}
pub fn assert_visible_turn_item_type_sequence(
    projection: &serde_json::Value,
    expected: &[&str],
) -> crate::error::Result<()> {
    require(
        serde_json::json!(
            rows(projection, "visibleTurnItems")
                .map(|row| row["item"]["type"].clone())
                .collect::<Vec<_>>()
        ) == serde_json::json!(expected),
        "unexpected visible item sequence",
    )
}
pub fn assert_assistant_text_includes(
    projection: &serde_json::Value,
    text: &str,
) -> crate::error::Result<()> {
    require(
        rows(projection, "turnItems").any(|item| {
            item["type"] == "assistant_message"
                && item["text"]
                    .as_str()
                    .is_some_and(|value| value.contains(text))
        }),
        format!("missing assistant output {text:?}"),
    )
}
pub fn assert_runtime_request_counts(
    projection: &serde_json::Value,
    total: usize,
    resolved: Option<usize>,
) -> crate::error::Result<()> {
    require(
        rows(projection, "runtimeRequests").count() == total,
        "unexpected runtime request count",
    )?;
    if let Some(resolved) = resolved {
        require(
            rows(projection, "runtimeRequests")
                .filter(|request| request["status"] == "resolved")
                .count()
                == resolved,
            "unexpected resolved request count",
        )?;
    }
    Ok(())
}
pub fn assert_runtime_request_kinds(
    projection: &serde_json::Value,
    kinds: &[&str],
) -> crate::error::Result<()> {
    assert_field_sequence(projection, "runtimeRequests", "kind", kinds)
}
pub fn assert_all_runtime_requests_resolved(
    projection: &serde_json::Value,
) -> crate::error::Result<()> {
    require(
        rows(projection, "runtimeRequests").all(|request| request["status"] == "resolved"),
        "unresolved runtime request",
    )
}
pub fn assert_conversation_message_roles(
    projection: &serde_json::Value,
    roles: &[&str],
) -> crate::error::Result<()> {
    assert_field_sequence(projection, "messages", "role", roles)
}
pub fn assert_replay_label_prefix_count(
    transcript: &crate::testing::replay::ReplayTranscript,
    prefix: &str,
    expected: usize,
) -> crate::error::Result<()> {
    use crate::testing::replay::ReplayEntry;
    let count = transcript
        .entries
        .iter()
        .filter(|entry| match entry {
            ReplayEntry::ExpectOutbound(entry) => entry
                .label
                .as_ref()
                .and_then(Option::as_ref)
                .is_some_and(|label| label.0.starts_with(prefix)),
            ReplayEntry::EmitInbound(entry) => entry
                .label
                .as_ref()
                .and_then(Option::as_ref)
                .is_some_and(|label| label.0.starts_with(prefix)),
            ReplayEntry::RuntimeExit(_) => false,
        })
        .count();
    require(
        count == expected,
        format!("unexpected {prefix} label count: {count}"),
    )
}
pub fn assert_no_acp_client_file_or_terminal_requests(
    transcript: &crate::testing::replay::ReplayTranscript,
) -> crate::error::Result<()> {
    use crate::testing::replay::ReplayEntry;
    let frames: Vec<_> = transcript
        .entries
        .iter()
        .filter_map(|entry| match entry {
            ReplayEntry::ExpectOutbound(entry) => Some(&entry.frame),
            ReplayEntry::EmitInbound(entry) => Some(&entry.frame),
            ReplayEntry::RuntimeExit(_) => None,
        })
        .collect();
    let caps = frames
        .iter()
        .find(|frame| frame["method"] == "initialize")
        .map(|frame| &frame["params"]["clientCapabilities"]);
    require(
        caps.is_some_and(|caps| {
            caps["fs"]["readTextFile"] == false
                && caps["fs"]["writeTextFile"] == false
                && caps["terminal"] == false
        }),
        "T3 must not advertise client fs or terminals",
    )?;
    require(
        !frames.iter().any(|frame| {
            frame["method"]
                .as_str()
                .is_some_and(|method| method.starts_with("fs/") || method.starts_with("terminal/"))
        }),
        "agent routed filesystem/terminal work through T3",
    )
}
pub fn assert_user_messages_include(
    projection: &serde_json::Value,
    texts: &[&str],
) -> crate::error::Result<()> {
    for text in texts {
        require(
            rows(projection, "turnItems").any(|item| {
                item["type"] == "user_message"
                    && item["text"]
                        .as_str()
                        .is_some_and(|value| value.contains(text))
            }),
            format!("missing user input {text:?}"),
        )?;
    }
    Ok(())
}
pub fn assert_visible_user_messages_include(
    projection: &serde_json::Value,
    texts: &[&str],
) -> crate::error::Result<()> {
    assert_visible_user_messages(projection, texts, true)
}
pub fn assert_visible_user_messages_exclude(
    projection: &serde_json::Value,
    texts: &[&str],
) -> crate::error::Result<()> {
    assert_visible_user_messages(projection, texts, false)
}
fn assert_visible_user_messages(
    projection: &serde_json::Value,
    texts: &[&str],
    include: bool,
) -> crate::error::Result<()> {
    for text in texts {
        require(
            rows(projection, "visibleTurnItems").any(|row| {
                row["item"]["type"] == "user_message"
                    && row["item"]["text"]
                        .as_str()
                        .is_some_and(|value| value.contains(text))
            }) == include,
            format!("unexpected visible user input {text:?}"),
        )?;
    }
    Ok(())
}
pub fn assert_user_message_input_intents(
    projection: &serde_json::Value,
    intents: &[&str],
) -> crate::error::Result<()> {
    require(
        serde_json::json!(
            rows(projection, "turnItems")
                .filter(|item| item["type"] == "user_message")
                .map(|item| item["inputIntent"].clone())
                .collect::<Vec<_>>()
        ) == serde_json::json!(intents),
        "unexpected user message intents",
    )
}
pub fn background_notifications(projection: &serde_json::Value) -> Vec<serde_json::Value> {
    rows(projection, "turnItems")
        .filter(|item| item["type"] == "notification")
        .map(|item| {
            let mut notification = serde_json::Map::new();
            for field in ["summary", "outcome", "source"] {
                if let Some(value) = item.get(field) {
                    notification.insert(field.into(), value.clone());
                }
            }
            serde_json::Value::Object(notification)
        })
        .collect()
}

pub fn assert_provider_native_subagent_root_turns(
    result: &super::super::orchestrator_scenario::ScenarioResult,
) -> crate::error::Result<()> {
    use serde_json::{Value, json};
    fn active(status: &Value) -> bool {
        matches!(status.as_str(), Some("pending" | "running" | "waiting"))
    }
    fn activity<'a>(statuses: impl Iterator<Item = &'a Value>) -> Vec<Value> {
        let mut result = Vec::new();
        for status in statuses {
            let status = if active(status) {
                json!("active")
            } else {
                status.clone()
            };
            if result.last() != Some(&status) {
                result.push(status);
            }
        }
        result
    }
    let events: Vec<Value> = result
        .domain_events
        .iter()
        .map(serde_json::to_value)
        .collect::<Result<_, _>>()?;
    for projection in result.projections.values() {
        let projection = serde_json::to_value(projection)?;
        for subagent in rows(&projection, "subagents").filter(|subagent| {
            subagent["origin"] == "provider_native" && !subagent["childThreadId"].is_null()
        }) {
            let child_id = subagent["childThreadId"].as_str().unwrap_or_default();
            let child =
                result
                    .projections
                    .get(child_id)
                    .ok_or_else(|| crate::error::Error::Service {
                        operation: "replay fixture assertion",
                        detail: format!("missing child projection {child_id}"),
                    })?;
            let child = serde_json::to_value(child)?;
            require(
                child["thread"]["creationSource"] == "provider"
                    && child["thread"]["forkedFrom"]
                        == json!({"type":"node","nodeId":subagent["id"]}),
                "native child source/fork point",
            )?;
            require(
                rows(&child, "runs").next().is_none(),
                "native child must not create app runs",
            )?;
            let roots: Vec<_> = rows(&child, "nodes")
                .filter(|node| node["kind"] == "root_turn")
                .collect();
            require(
                !roots.is_empty() && roots.iter().all(|root| root["runId"].is_null()),
                "native child requires runless root turns",
            )?;
            let mut run_by_update = BTreeMap::<String, Option<Value>>::new();
            for event in events.iter().filter(|event| event["threadId"] == child_id) {
                let update = format!("{}:{}", event["type"], event["payload"]);
                let run = event.get("runId").cloned();
                if let Some(stored) = run_by_update.get(&update) {
                    require(
                        stored == &run,
                        "child update stored under different run IDs",
                    )?;
                }
                run_by_update.insert(update, run);
            }
            let root_events: Vec<_> = events
                .iter()
                .enumerate()
                .filter(|(_, event)| {
                    event["type"] == "node.updated"
                        && event["payload"]["threadId"] == child_id
                        && event["payload"]["kind"] == "root_turn"
                })
                .map(|(index, event)| (index, &event["payload"]["status"]))
                .collect();
            require(
                root_events
                    .first()
                    .is_some_and(|(_, status)| **status == "running"),
                "native child must start working",
            )?;
            if let Some(first_item) = events.iter().position(|event| {
                event["type"] == "turn-item.updated"
                    && event["payload"]["threadId"] == child_id
                    && event["payload"]["type"] != "user_message"
            }) {
                require(
                    root_events[0].0 < first_item,
                    "native child must work before its first item",
                )?;
            }
            let root_activity = activity(root_events.iter().map(|(_, status)| *status));
            let subagent_activity = activity(
                events
                    .iter()
                    .filter(|event| {
                        event["type"] == "subagent.updated"
                            && event["payload"]["id"] == subagent["id"]
                    })
                    .map(|event| &event["payload"]["status"]),
            );
            let woken = rows(&child, "subagents").next().is_some()
                && root_activity.len() > subagent_activity.len();
            require(
                (if woken {
                    &root_activity[..subagent_activity.len()]
                } else {
                    root_activity.as_slice()
                }) == subagent_activity.as_slice(),
                "child root activity must follow parent subagent activity",
            )?;
            if woken {
                require(
                    root_activity.last() != Some(&json!("active")),
                    "woken child must settle",
                )?;
                let nested_ids: Vec<_> = rows(&child, "subagents")
                    .map(|nested| nested["id"].clone())
                    .collect();
                let ends: Vec<_> = events
                    .iter()
                    .enumerate()
                    .filter(|(_, event)| {
                        event["type"] == "subagent.updated"
                            && nested_ids.contains(&event["payload"]["id"])
                            && !active(&event["payload"]["status"])
                    })
                    .map(|(index, _)| index)
                    .collect();
                let end = events.iter().rposition(|event| {
                    event["type"] == "subagent.updated"
                        && event["payload"]["id"] == subagent["id"]
                        && !active(&event["payload"]["status"])
                });
                let wakes: Vec<_> = root_events
                    .iter()
                    .enumerate()
                    .filter(|(position, (index, status))| {
                        end.is_none_or(|end| *index > end)
                            && active(status)
                            && (*position == 0 || !active(root_events[*position - 1].1))
                    })
                    .map(|(_, event)| event.0)
                    .collect();
                require(
                    !wakes.is_empty() && wakes.len() <= ends.len(),
                    "child wake count must follow nested completions",
                )?;
                require(
                    wakes.iter().all(|wake| ends.iter().any(|end| end < wake)),
                    "child woke before nested subagent ended",
                )?;
            }
        }
    }
    Ok(())
}

pub const CLAUDE_LOCAL_BASH_TASK_PROMPT: &str = "Run a local Bash typecheck command, then answer exactly: claude local bash task fixture complete";

pub const CLAUDE_RESULT_IS_ERROR_FOLLOW_UP: &str = "Try again now that auth is back.";

pub const CLAUDE_RESULT_IS_ERROR_PROMPT: &str = "Say hello before the credentials expire.";

pub const COMMAND_OUTPUT_STREAMING_PROMPT: &str = "Run this exact shell command in the foreground and wait for it to finish: `for i in 1 2 3 4; do printf '\\033[32mstream line %s\\033[0m\\n' $i; sleep 1; done; printf 'progress 50%%\\rprogress 100%%\\n'`. Then respond exactly: command output fixture complete";

pub const MESSAGE_STEERING_INITIAL_PROMPT: &str =
    "Respond with exactly: steering fixture initial response";

pub const MESSAGE_STEERING_STEER_PROMPT: &str =
    "Actually, respond with exactly: steering fixture observed";

pub const MULTI_TURN_FIRST_PROMPT: &str = "Respond with exactly: first fixture turn complete";

pub const MULTI_TURN_SECOND_PROMPT: &str = "Respond with exactly: second fixture turn complete";

pub const OPENCODE2_BACKGROUND_PROMPT: &str = "Use the subagent tool with background enabled to delegate to the general subagent with the prompt: 'Run the shell command `sleep 20` with the bash tool and then reply exactly CHILD_OK.' As soon as it is launched, reply exactly PARENT_OK and end your turn without waiting for it.";

pub const OPENCODE2_CANCELLED_PROMPT: &str = "Reply exactly QUEUED_C.";

pub const OPENCODE2_COMMAND_PROMPT: &str = "/hello WORLD";

pub const OPENCODE2_COMPACTION_FIRST_PROMPT: &str = "Remember the codeword PAPAYA. Reply OK.";

pub const OPENCODE2_COMPACTION_RECALL_PROMPT: &str = "What was the codeword? One word.";

pub const OPENCODE2_INTERRUPT_PROMPT: &str =
    "Run the shell command `sleep 60 && echo LATE` with the bash tool, then reply DONE.";

pub const OPENCODE2_NESTED_BACKGROUND_PROMPT: &str = "Use the subagent tool (foreground, do not set background) to delegate to the general subagent with this exact prompt: 'Use the subagent tool with background set to true to delegate to the general subagent with the prompt: Run the shell command `sleep 25` with the shell tool, then reply exactly GRANDCHILD_OK. As soon as it is launched, reply exactly MIDDLE_OK and end your turn without waiting for it.' Wait for that subagent to return, then reply exactly ROOT_OK.";

pub const OPENCODE2_PERMISSION_PROMPT: &str = "Run the shell command `echo FIRST` with the bash tool. After it completes, run `echo SECOND` with the bash tool. Then reply with what happened.";

pub const OPENCODE2_QUESTION_PROMPT: &str = "Before doing anything, use the question tool to ask me which color I prefer, offering the options red and blue. After I answer, reply with only the chosen color.";

pub const OPENCODE2_QUEUED_PROMPT: &str = "Reply exactly QUEUED_B.";

pub const OPENCODE2_RESTART_PROMPT: &str = "Run the shell command `sleep 25 && echo RESUMED` with the bash tool, then reply with its output.";

pub const OPENCODE2_RESTART_RECALL_PROMPT: &str =
    "What did I last ask you to run? Answer in one short sentence.";

pub const OPENCODE2_REVERT_FIRST_PROMPT: &str = "Create a file named reverted.txt containing the word ALPHA using the write tool, then reply DONE.";

pub const OPENCODE2_REVERT_SECOND_PROMPT: &str =
    "Overwrite reverted.txt so it contains the word BETA using the write tool, then reply DONE.";

pub const OPENCODE2_SIMPLE_PROMPT: &str = "Think carefully step by step about whether 391 is prime, showing your reasoning, then answer in one short sentence.";

pub const OPENCODE2_SKILL_PROMPT: &str = "Use $greet to say hi in three words.";

pub const OPENCODE2_STEER_PROMPT: &str =
    "Run the shell command `sleep 12 && echo A` with the bash tool, then reply DONE_A.";

pub const OPENCODE2_STEER_TEXT: &str = "Also mention the word STEERED in your final reply.";

pub const OPENCODE2_SUBAGENT_PROMPT: &str = "Use the subagent tool to delegate to the explore subagent with the prompt: 'List the files in the current directory and report their names.' Wait for it, then summarize its answer in one line.";

pub const OPENCODE2_TOOL_CALL_PROMPT: &str = "Use the read tool to read hello.txt, then run the shell command `echo TOOL_OK` with the bash tool, then reply DONE.";

pub const OPENCODE_SUBAGENT_PROMPT: &str = "Use the task tool exactly once. Delegate to the general subagent with this prompt: Respond exactly CHILD_OK. After the task completes, respond exactly PARENT_OK.";

pub const PLAN_QUESTIONS_PROMPT: &str = "Use request_user_input to ask one multiple-choice clarifying question about whether this fixture should prefer strict schemas or UI flexibility. After receiving the answer, respond exactly: plan questions fixture complete";

pub const PROPOSED_PLAN_PROMPT: &str = "Create a short implementation plan for adding deterministic replay fixtures. Do not ask questions. Present the final plan in a proposed plan block.";

pub const PROVIDER_THREAD_RESUME_FIRST_PROMPT: &str =
    "Respond with exactly: provider thread resume fixture first turn complete";

pub const PROVIDER_THREAD_RESUME_SECOND_PROMPT: &str = "Using the conversation history available in this resumed thread, first repeat the exact final answer you gave in the previous turn. Then on a new line write exactly: provider thread resume fixture second turn complete";

pub const SIMPLE_PROMPT: &str = "Respond with the following text: fixture simple ok";

pub const SKILL_INVOCATION_CURSOR_MESSAGE: &str = "/review README.md";

pub const SKILL_INVOCATION_PROMPT: &str = "$review README.md";

pub const SUBAGENT_CONTINUE_CHILD_PROMPT: &str = "Reply exactly: continued subagent response";

pub const SUBAGENT_CONTINUE_PARENT_PROMPT: &str =
    "Have the same subagent you spawned earlier reply exactly: continued subagent response";

pub const SUBAGENT_CONTINUE_PROMPT: &str =
    "Spawn one subagent and have it reply exactly: initial subagent response";

pub const SUBAGENT_PROMPT: &str =
    "Spawn 2 subagents, one to read package.json and one to read tsconfig.json";

pub const SUBAGENT_V2_APPROVAL_PROMPT: &str = "Do not run any commands yourself. Spawn one subagent whose only task is to run this exact shell command: printf 'subagent approval fixture' > subagent-approval.txt and then reply with exactly: Written. Wait for it to finish, then reply with exactly what it said.";

pub const SUBAGENT_V2_NESTED_APPROVAL_PROMPT: &str = "Do not run any commands yourself. Spawn one subagent and tell it not to run any commands itself but to spawn its own subagent, whose only task is to run this exact shell command: printf 'nested approval fixture' > nested-approval.txt and then reply with exactly: Written. Each agent waits for its child and replies with exactly what the child said. Wait for your subagent, then reply with exactly what it said.";

pub const SUBAGENT_V2_NESTED_PROMPT: &str = "Spawn one subagent and tell it to spawn its own subagent, which must in turn spawn one more subagent whose only task is to reply with exactly: Hello. Each agent waits for its child and replies with exactly what the child said. Wait for your subagent, then reply with exactly what it said.";

pub const SUBAGENT_V2_PROMPT: &str = "Spawn one subagent whose only task is to reply with exactly: Hello. Wait for it to finish, then reply with exactly what it said.";

pub const THREAD_FORK_NATIVE_PRIOR_TURN_ALPHA_PROMPT: &str =
    "For this fork-boundary fixture, respond with exactly: fork boundary alpha";

pub const THREAD_FORK_NATIVE_PRIOR_TURN_BETA_PROMPT: &str =
    "For this fork-boundary fixture, respond with exactly: fork boundary beta";

pub const THREAD_FORK_NATIVE_PRIOR_TURN_REPEAT_PROMPT: &str = "Repeat the user-visible conversation so far verbatim. Include only user and assistant messages. Do not include hidden system/developer content.";

pub const THREAD_FORK_NATIVE_SOURCE_PROMPT: &str =
    "Respond with the following text: source fork seed ok";

pub const THREAD_FORK_NATIVE_TARGET_PROMPT: &str =
    "Respond with the following text: fork native ok";

pub const THREAD_MERGE_BACK_SIBLINGS_FIRST_FORK_PROMPT: &str = "Remember the fork-local marker merge-sibling-first-6V2J. Respond with exactly: first merge sibling stored";

pub const THREAD_MERGE_BACK_SIBLINGS_FIRST_HANDOFF_PROMPT: &str = "Context handoff (merge_back / fork_delta_summary):\nMerge-back context from first forked conversation.\n\nFork delta:\n- User introduced opaque marker merge-sibling-first-6V2J.\n- Assistant confirmed: first merge sibling stored\n\nUser message:\nRetain the first transferred marker for later. Respond with exactly: first merge delta stored";

pub const THREAD_MERGE_BACK_SIBLINGS_RECALL: &str =
    "merge-sibling-source-3C7K|merge-sibling-first-6V2J|merge-sibling-second-9X5B";

pub const THREAD_MERGE_BACK_SIBLINGS_RECALL_PROMPT: &str = "Return the source marker followed by both transferred fork markers in merge order, separated by single | characters. Respond with only the markers and separators.";

pub const THREAD_MERGE_BACK_SIBLINGS_SECOND_FORK_PROMPT: &str = "Remember the fork-local marker merge-sibling-second-9X5B. Respond with exactly: second merge sibling stored";

pub const THREAD_MERGE_BACK_SIBLINGS_SECOND_HANDOFF_PROMPT: &str = "Context handoff (merge_back / fork_delta_summary):\nMerge-back context from second forked conversation.\n\nFork delta:\n- User introduced opaque marker merge-sibling-second-9X5B.\n- Assistant confirmed: second merge sibling stored\n\nUser message:\nRetain the second transferred marker for later. Respond with exactly: second merge delta stored";

pub const THREAD_MERGE_BACK_SIBLINGS_SOURCE_PROMPT: &str = "Remember the opaque marker merge-sibling-source-3C7K for later in this conversation. Respond with exactly: merge sibling source stored";

pub const THREAD_ROLLBACK_AFTER_PROMPT: &str = "Repeat the conversation verbatim.";

pub const THREAD_ROLLBACK_FIRST_PROMPT: &str =
    "Respond with exactly: rollback fixture first turn complete";

pub const THREAD_ROLLBACK_SECOND_PROMPT: &str =
    "Respond with exactly: rollback fixture second turn complete";

pub const TODO_LIST_PROMPT: &str = "Use the update_plan tool to track exactly three steps: inspect package.json, inspect tsconfig.json, report completion. Then read package.json and tsconfig.json, and answer exactly: todo list fixture complete";

pub const TOOL_CALL_READ_ONLY_PROMPT: &str = "Read /tmp/claude-replay-tool_call_read_only/package.json and /tmp/claude-replay-tool_call_read_only/tsconfig.json, then answer exactly: read only tool fixture complete";

pub const TOOL_CALL_READ_ONLY_WORKSPACE_ROOT: &str = "/tmp/claude-replay-tool_call_read_only";

pub const TOOL_CALL_WRITE_PROMPT: &str = "Create or overwrite .codex-probe-write-action.txt with exactly this text: codex app-server approval fixture. Use a local shell command or file edit only, then briefly report what happened. Do not read package metadata, use GitHub, use web, or use MCP.";

pub const TURN_INTERRUPT_MID_TOOL_PROMPT: &str = "Run this exact local command: `node -e \"console.log('interrupt fixture tool started'); setTimeout(() => {}, 30000)\"`. Do not answer until it completes, then respond exactly: interrupt fixture should not finish naturally.";

pub const TURN_INTERRUPT_PROMPT: &str = "Do not answer immediately. First run the local shell command `sleep 30`, then respond with exactly: interrupt fixture should not finish naturally.";

pub const TURN_INTERRUPT_RECOVERY_PROMPT: &str =
    "Respond with exactly: interrupt recovery fixture complete";

pub const WEB_SEARCH_PROMPT: &str = "Search the web for FIFA World Cup ticket pricing, then answer exactly: web search fixture complete";

pub fn acp_registry_model_selection()
-> crate::error::Result<t3_contracts::model_selection::ModelSelection> {
    Ok(serde_json::from_value(
        serde_json::json!({"instanceId": "acpRegistry", "model": "default"}),
    )?)
}

pub fn claude_model_selection()
-> crate::error::Result<t3_contracts::model_selection::ModelSelection> {
    Ok(serde_json::from_value(
        serde_json::json!({"instanceId": "claudeAgent", "model": "claude-sonnet-4-6"}),
    )?)
}

pub fn codex_model_selection() -> crate::error::Result<t3_contracts::model_selection::ModelSelection>
{
    Ok(serde_json::from_value(
        serde_json::json!({"instanceId": "codex", "model": "gpt-6-luna"}),
    )?)
}

pub fn cursor_model_selection()
-> crate::error::Result<t3_contracts::model_selection::ModelSelection> {
    Ok(serde_json::from_value(
        serde_json::json!({"instanceId": "cursor", "model": "composer-2.5"}),
    )?)
}

pub fn fx_model_selection() -> crate::error::Result<t3_contracts::model_selection::ModelSelection> {
    Ok(serde_json::from_value(
        serde_json::json!({"instanceId": "fx", "model": "default"}),
    )?)
}

pub fn grok_model_selection() -> crate::error::Result<t3_contracts::model_selection::ModelSelection>
{
    Ok(serde_json::from_value(
        serde_json::json!({"instanceId": "grok", "model": "grok-build"}),
    )?)
}

pub fn kiro_model_selection() -> crate::error::Result<t3_contracts::model_selection::ModelSelection>
{
    Ok(serde_json::from_value(
        serde_json::json!({"instanceId": "kiro", "model": "claude-haiku-4.5"}),
    )?)
}

pub fn muse_model_selection() -> crate::error::Result<t3_contracts::model_selection::ModelSelection>
{
    Ok(serde_json::from_value(
        serde_json::json!({"instanceId": "muse", "model": "muse-spark-1.3-contributor"}),
    )?)
}

pub fn opencode2_model_selection()
-> crate::error::Result<t3_contracts::model_selection::ModelSelection> {
    Ok(serde_json::from_value(
        serde_json::json!({"instanceId": "opencode", "model": "opencode/space-bunny-free", "options": [{"id": "variant", "value": "high"}]}),
    )?)
}

pub fn opencode_model_selection()
-> crate::error::Result<t3_contracts::model_selection::ModelSelection> {
    Ok(serde_json::from_value(
        serde_json::json!({"instanceId": "opencode", "model": "openai/gpt-5.4-mini", "options": [{"id": "agent", "value": "build"}]}),
    )?)
}

pub fn pi_model_selection() -> crate::error::Result<t3_contracts::model_selection::ModelSelection> {
    Ok(serde_json::from_value(
        serde_json::json!({"instanceId": "pi", "model": "openrouter/deepseek/deepseek-v4-flash"}),
    )?)
}

pub fn read_only_never_policy() -> serde_json::Value {
    serde_json::json!({"approvalPolicy": "never", "sandboxPolicy": {"type": "readOnly", "access": {"type": "fullAccess"}, "networkAccess": false}})
}

pub fn read_only_on_request_policy() -> serde_json::Value {
    serde_json::json!({"approvalPolicy": "on-request", "sandboxPolicy": {"type": "readOnly", "access": {"type": "fullAccess"}, "networkAccess": false}})
}

pub fn restricted_granular_policy() -> serde_json::Value {
    serde_json::json!({"approvalPolicy": {"granular": {"mcp_elicitations": true, "request_permissions": true, "rules": true, "sandbox_approval": true, "skill_approval": true}}, "sandboxPolicy": {"type": "readOnly", "access": {"type": "restricted", "includePlatformDefaults": false, "readableRoots": []}, "networkAccess": false}})
}

pub fn workspace_never_policy() -> serde_json::Value {
    serde_json::json!({"approvalPolicy": "never", "sandboxPolicy": {"type": "workspaceWrite", "writableRoots": [], "readOnlyAccess": {"type": "fullAccess"}, "networkAccess": false}})
}
