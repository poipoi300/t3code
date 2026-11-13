use crate::{
    api::{Clock, CommandDispatcher, EffectWorker, EventCommitter, ProjectionReader},
    error::{Error, Result},
    stores::{
        StoreDatabase,
        event_store::{EventFilter, EventStore},
    },
    testing::{ManualClock, replay::ReplayGate},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use t3_contracts::{
    base_schemas::*,
    orchestration_v2::*,
    provider_policy::{ProviderApprovalDecision, ProviderUserInputAnswers},
};
use tokio::task::JoinSet;

pub struct Scenario {
    pub name: String,
    pub commands: Vec<OrchestrationV2ServerCommand>,
    pub steps: Option<Vec<ScenarioStep>>,
    pub projection_thread_ids: Vec<ThreadId>,
}
pub enum ScenarioStep {
    Dispatch {
        command: Box<OrchestrationV2ServerCommand>,
        wait: bool,
        key: Option<String>,
    },
    Await(String),
    AwaitAll,
    AwaitThreadIdle(ThreadId),
    AwaitNoBackgroundWork(ThreadId),
    AwaitRunStatus {
        thread_id: ThreadId,
        run_id: RunId,
        status: String,
    },
    FinishHeldRun {
        thread_id: ThreadId,
        run_id: RunId,
        status: String,
    },
    AwaitRunSteerable {
        thread_id: ThreadId,
        run_id: RunId,
    },
    AwaitRunTurnItem {
        thread_id: ThreadId,
        run_id: RunId,
        item_type: String,
        item_status: Option<String>,
        in_subagent_threads: bool,
    },
    ReleaseReplayGate(String),
    ReleaseReplayGateAfterWaiting {
        label: String,
        thread_id: ThreadId,
        run_id: RunId,
    },
    AdvanceClock {
        duration: Duration,
        now: String,
    },
    AdvanceClockBy {
        duration: Duration,
        await_provider_session_release: Option<ThreadId>,
    },
    CaptureShellSnapshot(String),
    RespondToNextRuntimeRequest {
        thread_id: ThreadId,
        command_id: CommandId,
        decision: Option<ProviderApprovalDecision>,
        answers: Option<ProviderUserInputAnswers>,
        shell_snapshot_key: Option<String>,
    },
}
#[derive(Clone)]
pub struct ScenarioServices {
    pub database: StoreDatabase,
    pub dispatcher: Arc<dyn CommandDispatcher>,
    pub committer: Arc<dyn EventCommitter>,
    pub projections: Arc<dyn ProjectionReader>,
    pub worker: Arc<dyn EffectWorker>,
    pub clock: Arc<ManualClock>,
    pub gate: ReplayGate,
}

struct ReleaseReplayGate(ReplayGate);
impl Drop for ReleaseReplayGate {
    fn drop(&mut self) {
        self.0.release_all();
    }
}
pub struct ScenarioResult {
    pub stored_events: Vec<OrchestrationV2StoredEvent>,
    pub domain_events: Vec<OrchestrationV2DomainEvent>,
    pub projections: BTreeMap<String, OrchestrationV2ThreadProjection>,
    pub shell_snapshot: OrchestrationV2ShellSnapshot,
    pub captured_shell_snapshots: BTreeMap<String, OrchestrationV2ShellSnapshot>,
    pub command_output:
        Option<BTreeMap<TurnItemId, super::command_output_recorder::RecordedCommandOutput<Value>>>,
}

fn step_error(scenario: &str, step: impl std::fmt::Display) -> Error {
    Error::Service {
        operation: "replay scenario",
        detail: format!("invalid step {step} in {scenario}"),
    }
}

async fn wait_for_projection(
    services: &ScenarioServices,
    thread_id: ThreadId,
    predicate: impl Fn(&Value) -> bool,
) -> Result<OrchestrationV2ThreadProjection> {
    let mut events = services.committer.subscribe();
    loop {
        if let Some(projection) = services.projections.projection(thread_id.clone()).await?
            && predicate(&serde_json::to_value(&projection)?)
        {
            return Ok(projection);
        }
        events.recv().await.map_err(|error| match error {
            tokio::sync::broadcast::error::RecvError::Lagged(_) => Error::Lagged,
            _ => Error::Service {
                operation: "replay scenario",
                detail: "committed event stream closed".into(),
            },
        })?;
    }
}
fn array<'a>(value: &'a Value, key: &str) -> impl Iterator<Item = &'a Value> {
    value[key].as_array().into_iter().flatten()
}
fn find_run<'a>(projection: &'a Value, run_id: &RunId) -> Option<&'a Value> {
    array(projection, "runs").find(|run| run["id"] == run_id.0)
}

async fn advance_clock(services: &ScenarioServices, duration: Duration) -> Result<()> {
    use crate::ports::decisions::orchestrator::{epoch_millis, iso_from_epoch_millis};
    let now = epoch_millis(&services.clock.now().0)
        .ok_or_else(|| step_error("clock", "invalid ISO time"))?;
    let millis = i64::try_from(duration.as_millis())
        .map_err(|_| step_error("clock", "duration overflow"))?;
    let advanced = now
        .checked_add(millis)
        .ok_or_else(|| step_error("clock", "time overflow"))?;
    services.clock.set(&iso_from_epoch_millis(advanced));
    tokio::time::advance(duration).await;
    Ok(())
}

pub fn duration_from_fixture(value: &Value) -> Result<Duration> {
    if let Some(millis) = value.as_f64()
        && millis.is_finite()
        && millis >= 0.0
    {
        return Duration::try_from_secs_f64(millis / 1000.0)
            .map_err(|error| step_error("clock", error));
    }
    if let Some(value) = value.as_str()
        && let Some((amount, unit)) = value.split_once(' ')
        && let Ok(amount) = amount.parse::<f64>()
    {
        let scale = match unit {
            "nano" | "nanos" => 1e-9,
            "micro" | "micros" => 1e-6,
            "milli" | "millis" => 1e-3,
            "second" | "seconds" => 1.0,
            "minute" | "minutes" => 60.0,
            "hour" | "hours" => 3600.0,
            "day" | "days" => 86400.0,
            _ => return Err(step_error("clock", value)),
        };
        return Duration::try_from_secs_f64(amount * scale)
            .map_err(|error| step_error("clock", error));
    }
    Err(step_error("clock", value))
}

pub fn scenario_step_from_value(value: &Value) -> Result<ScenarioStep> {
    let string = |key: &str| {
        value[key]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| step_error("materialized fixture", format!("missing {key}")))
    };
    let thread = || string("threadId").map(ThreadId);
    let run = || string("runId").map(RunId);
    fn optional<T: serde::de::DeserializeOwned>(value: &Value, key: &str) -> Result<Option<T>> {
        Ok(value
            .get(key)
            .map(|value| serde_json::from_value(value.clone()))
            .transpose()?)
    }
    Ok(match string("type")?.as_str() {
        "dispatch" => ScenarioStep::Dispatch {
            command: Box::new(serde_json::from_value(value["command"].clone())?),
            wait: value["await"].as_bool().unwrap_or(true),
            key: optional(value, "key")?,
        },
        "await" => ScenarioStep::Await(string("key")?),
        "await_all" => ScenarioStep::AwaitAll,
        "await_thread_idle" => ScenarioStep::AwaitThreadIdle(thread()?),
        "await_no_background_work" => ScenarioStep::AwaitNoBackgroundWork(thread()?),
        "await_run_status" => ScenarioStep::AwaitRunStatus {
            thread_id: thread()?,
            run_id: run()?,
            status: string("status")?,
        },
        "finish_held_run" => ScenarioStep::FinishHeldRun {
            thread_id: thread()?,
            run_id: run()?,
            status: string("status")?,
        },
        "await_run_steerable" => ScenarioStep::AwaitRunSteerable {
            thread_id: thread()?,
            run_id: run()?,
        },
        "await_run_turn_item" => ScenarioStep::AwaitRunTurnItem {
            thread_id: thread()?,
            run_id: run()?,
            item_type: string("itemType")?,
            item_status: optional(value, "itemStatus")?,
            in_subagent_threads: value["inSubagentThreads"] == true,
        },
        "release_replay_gate" => ScenarioStep::ReleaseReplayGate(string("label")?),
        "release_replay_gate_after_waiting" => ScenarioStep::ReleaseReplayGateAfterWaiting {
            label: string("label")?,
            thread_id: thread()?,
            run_id: run()?,
        },
        "advance_clock" => ScenarioStep::AdvanceClockBy {
            duration: duration_from_fixture(&value["duration"])?,
            await_provider_session_release: optional(value, "awaitProviderSessionRelease")?,
        },
        "capture_shell_snapshot" => ScenarioStep::CaptureShellSnapshot(string("key")?),
        "respond_to_next_runtime_request" => ScenarioStep::RespondToNextRuntimeRequest {
            thread_id: thread()?,
            command_id: CommandId(string("commandId")?),
            decision: optional(value, "decision")?,
            answers: optional(value, "answers")?,
            shell_snapshot_key: optional(value, "shellSnapshotKeyWhilePending")?,
        },
        kind => return Err(step_error("materialized fixture", kind)),
    })
}

pub async fn run_scenario(
    scenario: Scenario,
    services: &ScenarioServices,
) -> Result<ScenarioResult> {
    let release_gate = ReleaseReplayGate(services.gate.clone());
    let after_sequence = services.committer.latest_sequence(None).await? as i64;
    let steps = scenario.steps.unwrap_or_else(|| {
        scenario
            .commands
            .into_iter()
            .map(|command| ScenarioStep::Dispatch {
                command: Box::new(command),
                wait: true,
                key: None,
            })
            .collect()
    });
    let mut projection_thread_ids = scenario.projection_thread_ids;
    if projection_thread_ids.is_empty() {
        for step in &steps {
            if let ScenarioStep::Dispatch { command, .. } = step {
                let command = serde_json::to_value(command)?;
                for field in [
                    "threadId",
                    "parentThreadId",
                    "sourceThreadId",
                    "targetThreadId",
                ] {
                    if let Some(id) = command[field].as_str() {
                        let thread_id = ThreadId(id.into());
                        if !projection_thread_ids.contains(&thread_id) {
                            projection_thread_ids.push(thread_id);
                        }
                    }
                }
            }
        }
    }
    let mut dispatches = JoinSet::new();
    let mut keys = Vec::<String>::new();
    let mut anonymous_dispatch_index = 0;
    let mut consumed_finish_receipts = 0;
    let mut results = BTreeMap::new();
    let mut captured = BTreeMap::new();
    let result = async {
        for step in steps {
            match step {
                ScenarioStep::Dispatch { command, wait, key } => {
                    if wait { services.dispatcher.dispatch(*command).await?; }
                    else {
                        anonymous_dispatch_index+=1;
                        let key = key.unwrap_or_else(|| format!("dispatch:{anonymous_dispatch_index}"));
                        if keys.contains(&key) { return Err(step_error(&scenario.name, format!("duplicate dispatch:{key}"))); }
                        keys.push(key.clone());
                        let dispatcher = services.dispatcher.clone();
                        dispatches.spawn(async move { (key, dispatcher.dispatch(*command).await) });
                    }
                }
                ScenarioStep::Await(key) => {
                    if !keys.contains(&key) { return Err(step_error(&scenario.name,format!("await:{key}"))); }
                    while !results.contains_key(&key) {
                        let (finished, result) = dispatches.join_next().await.ok_or_else(||step_error(&scenario.name,"missing dispatch"))?.map_err(|e|step_error(&scenario.name,e))?;
                        results.insert(finished, result);
                    }
                    results.remove(&key).ok_or_else(||step_error(&scenario.name,"missing dispatch result"))??;
                    keys.retain(|candidate|candidate!=&key);
                }
                ScenarioStep::AwaitAll => {
                    while let Some(result) = dispatches.join_next().await { let (key,result) = result.map_err(|e|step_error(&scenario.name,e))?; results.insert(key,result); }
                    for key in keys.drain(..) { results.remove(&key).ok_or_else(||step_error(&scenario.name,"missing dispatch result"))??; }
                }
                ScenarioStep::AwaitThreadIdle(thread) => { wait_for_projection(services,thread,|p| !array(p,"runs").any(|run|matches!(run["status"].as_str(),Some("preparing"|"queued"|"starting"|"running"|"waiting")))).await?; }
                ScenarioStep::AwaitNoBackgroundWork(thread) => {
                    let mut events=services.committer.subscribe();
                    loop {
                        let shell=services.projections.shell(thread.clone()).await?;
                        if shell.as_ref().is_none_or(|shell|serde_json::to_value(shell).ok().is_some_and(|value|array(&value,"pendingBackgroundTasks").next().is_none())) { break; }
                        events.recv().await.map_err(|error|step_error(&scenario.name,error))?;
                    }
                }
                ScenarioStep::AwaitRunStatus { thread_id, run_id, status } => { wait_for_projection(services,thread_id,|p|find_run(p,&run_id).is_some_and(|run|run["status"] == status)).await?; }
                ScenarioStep::FinishHeldRun { thread_id,run_id,status }=> {
                    loop {
                        let duration=services.gate.wait_for_finish_armed(consumed_finish_receipts).await;
                        consumed_finish_receipts=services.gate.finish_armed_count();
                        advance_clock(services,duration).await?;
                        let settled=tokio::select! {
                            result=wait_for_projection(services,thread_id.clone(),|p|find_run(p,&run_id).is_some_and(|run|run["status"]==status))=>{result?;true},
                            _=services.gate.wait_for_finish_armed(consumed_finish_receipts)=>false,
                        };
                        if settled { consumed_finish_receipts=services.gate.finish_armed_count();break; }
                    }
                }
                ScenarioStep::AwaitRunSteerable { thread_id, run_id } => { wait_for_projection(services,thread_id,|p|find_run(p,&run_id).is_some_and(|run|run["status"] == "running" && !run["activeAttemptId"].is_null() && array(p,"providerTurns").any(|turn|turn["runAttemptId"] == run["activeAttemptId"] && turn["status"] == "running"))).await?; }
                ScenarioStep::AwaitRunTurnItem { thread_id, run_id, item_type, item_status, in_subagent_threads } => {
                    let matches=|item:&Value|item["type"]==item_type && item_status.as_ref().is_none_or(|status|item["status"]==*status);
                    if !in_subagent_threads {
                        wait_for_projection(services,thread_id,|p|array(p,"turnItems").any(|item|item["runId"]==run_id.0 && matches(item))).await?;
                    } else {
                        let mut events=services.committer.subscribe();
                        loop {
                            let projection=services.projections.projection(thread_id.clone()).await?.ok_or(Error::ThreadNotFound(thread_id.clone()))?;
                            let projection=serde_json::to_value(projection)?;
                            let mut found=false;
                            for child in array(&projection,"subagents").filter(|child|child["runId"]==run_id.0).filter_map(|child|child["childThreadId"].as_str()) {
                                if let Some(child)=services.projections.projection(ThreadId(child.into())).await? {
                                    found |= array(&serde_json::to_value(child)?,"turnItems").any(matches);
                                }
                            }
                            if found {break;}
                            events.recv().await.map_err(|error|step_error(&scenario.name,error))?;
                        }
                    }
                }
                ScenarioStep::ReleaseReplayGate(label) => { if !services.gate.wait_for_reached(&label).await { return Err(step_error(&scenario.name,format!("release:{label}"))); } services.gate.release(&label); }
                ScenarioStep::ReleaseReplayGateAfterWaiting { label, thread_id, run_id } => {
                    if !services.gate.wait_for_reached(&label).await { return Err(step_error(&scenario.name,format!("gate:{label}"))); }
                    wait_for_projection(services,thread_id.clone(),|p| find_run(p,&run_id).is_some_and(|run| run["status"] == "completed" && array(p,"providerThreads").any(|thread|thread["id"] == run["providerThreadId"] && thread["pendingBackgroundTasks"].as_array().is_some_and(|tasks|!tasks.is_empty())))).await?;
                    services.gate.release(&label);
                    wait_for_projection(services,thread_id,|p| find_run(p,&run_id).is_some_and(|run|array(p,"providerThreads").any(|thread|thread["id"]==run["providerThreadId"] && thread["status"]=="idle" && array(thread,"pendingBackgroundTasks").next().is_none()))).await?;
                }
                ScenarioStep::AdvanceClock { duration, now } => { services.clock.set(&now); tokio::time::advance(duration).await; }
                ScenarioStep::AdvanceClockBy {duration,await_provider_session_release}=> {
                    let mut events=services.committer.subscribe();
                    advance_clock(services,duration).await?;
                    if let Some(thread)=await_provider_session_release {
                        loop {
                            let event=events.recv().await.map_err(|error|step_error(&scenario.name,error))?;
                            let event=serde_json::to_value(event)?;
                            if event["event"]["type"]=="provider-session.updated" && event["event"]["threadId"]==thread.0 && event["event"]["payload"]["status"]=="stopped" {break;}
                        }
                    }
                }
                ScenarioStep::CaptureShellSnapshot(key) => { captured.insert(key,services.projections.shell_snapshot(None,None).await?); }
                ScenarioStep::RespondToNextRuntimeRequest { thread_id, command_id, decision, answers, shell_snapshot_key } => {
                    let projection = wait_for_projection(services,thread_id.clone(),|p|array(p,"runtimeRequests").any(|request|request["status"] == "pending")).await?;
                    if let Some(key) = shell_snapshot_key { captured.insert(key,services.projections.shell_snapshot(None,None).await?); }
                    let projection = serde_json::to_value(projection)?;
                    let request = array(&projection,"runtimeRequests").find(|request|request["status"] == "pending").ok_or_else(||step_error(&scenario.name,"pending request vanished"))?;
                    let mut command = json!({"type":"runtime-request.respond","commandId":command_id,"threadId":thread_id,"requestId":request["id"]});
                    if let Some(decision) = decision { command["decision"] = json!(decision); }
                    if let Some(answers) = answers { command["answers"] = json!(answers); }
                    services.dispatcher.dispatch(serde_json::from_value(command)?).await?;
                }
            }
        }
        while let Some(result) = dispatches.join_next().await { let (key,result) = result.map_err(|e|step_error(&scenario.name,e))?; results.insert(key,result); }
        for key in keys.drain(..) { results.remove(&key).ok_or_else(||step_error(&scenario.name,"missing dispatch result"))??; }
        services.worker.drain(10000).await?;
        let stored_events = services.database.read(move |db|EventStore::new().read(db,&EventFilter { after_sequence, ..Default::default() })).await?;
        let shell_snapshot = services.projections.shell_snapshot(None,None).await?;
        for thread in &shell_snapshot.threads {
            if !projection_thread_ids.contains(&thread.id) { projection_thread_ids.push(thread.id.clone()); }
        }
        let mut projections = BTreeMap::new();
        for thread in projection_thread_ids { let projection = services.projections.projection(thread.clone()).await?.ok_or(Error::ThreadNotFound(thread.clone()))?; projections.insert(thread.0,projection); }
        Ok(ScenarioResult { domain_events: stored_events.iter().map(|stored|stored.event.clone()).collect(), stored_events, projections, shell_snapshot, captured_shell_snapshots: captured,command_output:None })
    }.await;
    drop(release_gate);
    dispatches.shutdown().await;
    result
}

pub async fn run_with_command_output<State: Clone + serde::Serialize>(
    scenario: Scenario,
    services: &ScenarioServices,
    recorder: &super::command_output_recorder::CommandOutputRecorder<State>,
) -> Result<ScenarioResult> {
    let mut result = run_scenario(scenario, services).await?;
    result.command_output = Some(
        recorder
            .snapshot()
            .into_iter()
            .map(|(id, record)| {
                Ok((
                    id,
                    super::command_output_recorder::RecordedCommandOutput {
                        chunks: record.chunks,
                        output: serde_json::to_value(record.output)?,
                    },
                ))
            })
            .collect::<Result<_>>()?,
    );
    Ok(result)
}
