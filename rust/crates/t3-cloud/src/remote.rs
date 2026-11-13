use crate::{
    Error, Result,
    http::Sensitive,
    link_requests::{ActiveParent, LinkCaller, component},
    peer::{CallLimits, PeerCall, PeerLinks, PeerMcpClient},
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{collections::HashSet, sync::Arc, time::Duration};
use t3_contracts::{
    base_schemas::{EnvironmentId, NodeId, ProjectId, ThreadId},
    model_selection::ModelSelection,
    orchestration_v2::{
        InternalCommandDelegatedTaskRemoteComplete, InternalCommandDelegatedTaskRemoteRequest,
        OrchestrationV2LinkedSender, OrchestrationV2RemoteTaskChild,
    },
    orchestrator_mcp::{
        OrchestratorMcpDelegateTaskInput, OrchestratorMcpFailureCode as Code,
        OrchestratorMcpInteractionMode, OrchestratorMcpIsolation, OrchestratorMcpRuntimeMode,
        OrchestratorMcpTaskCancelResult, OrchestratorMcpThreadSendResult,
        OrchestratorMcpThreadWaitResult,
    },
    provider_policy::{ProviderInteractionMode, RuntimeMode},
};
use tokio::sync::broadcast;
pub type TaskKey = (ThreadId, NodeId);
pub struct RemoteParent {
    pub thread_id: ThreadId,
    pub project_id: ProjectId,
    pub provider_instance_id: t3_contracts::provider_instance::ProviderInstanceId,
    pub title: String,
    pub model_selection: ModelSelection,
    pub limits: CallLimits,
    pub active: Option<ActiveParent>,
    pub repository_key: Option<String>,
}
#[derive(Clone)]
pub struct RemoteTask {
    pub key: TaskKey,
    pub remote: OrchestrationV2RemoteTaskChild,
    pub status: String,
    pub result: Option<String>,
    pub model_selection: ModelSelection,
    pub limits: CallLimits,
}
#[async_trait]
pub trait RemoteTaskStore: Send + Sync {
    async fn parent(&self, thread: &ThreadId) -> Result<RemoteParent>;
    /// Query this one task, never the parent's whole task list per follower.
    async fn task(&self, key: &TaskKey) -> Result<Option<RemoteTask>>;
    async fn outstanding(&self) -> Result<Vec<TaskKey>>;
    /// Subscribe before scanning/querying so a Stop cannot slip between the two.
    fn subscribe(&self) -> broadcast::Receiver<TaskKey>;
    async fn request(&self, command: InternalCommandDelegatedTaskRemoteRequest) -> Result<NodeId>;
    /// Apply the normal decider: a concurrent Stop/result wins and stopCount fences old acks.
    async fn complete(&self, command: InternalCommandDelegatedTaskRemoteComplete) -> Result<()>;
    /// Whether a later result run belongs to another task on the same remote thread.
    async fn shared_result_run(&self, key: &TaskKey) -> Result<bool>;
    async fn finished_owner(
        &self,
        parent: &ThreadId,
        environment: &EnvironmentId,
        thread: &ThreadId,
    ) -> Result<Option<RemoteTask>>;
}
pub struct RemoteCall {
    pub environment_id: EnvironmentId,
    pub tool: &'static str,
    pub arguments: Value,
    pub limits: CallLimits,
    pub namespace: String,
    pub sender: Option<OrchestrationV2LinkedSender>,
}
#[async_trait]
pub trait RemoteCalls: Send + Sync {
    async fn call(&self, call: RemoteCall) -> Result<Sensitive<Value>>;
}
#[async_trait]
impl RemoteCalls for PeerMcpClient {
    async fn call(&self, call: RemoteCall) -> Result<Sensitive<Value>> {
        self.call(PeerCall {
            environment_id: &call.environment_id.0,
            tool: call.tool,
            arguments: call.arguments,
            limits: call.limits,
            namespace: &call.namespace,
            sender: call.sender,
            linked_origin: false,
        })
        .await
    }
}
pub struct RemoteDelegation {
    environment_id: EnvironmentId,
    store: Arc<dyn RemoteTaskStore>,
    calls: Arc<dyn RemoteCalls>,
    links: Arc<PeerLinks>,
    wake: broadcast::Sender<TaskKey>,
}
impl RemoteDelegation {
    pub fn new(
        environment_id: EnvironmentId,
        store: Arc<dyn RemoteTaskStore>,
        calls: Arc<dyn RemoteCalls>,
        links: Arc<PeerLinks>,
    ) -> Arc<Self> {
        Arc::new(Self {
            environment_id,
            store,
            calls,
            links,
            wake: broadcast::channel(128).0,
        })
    }
    fn fail(code: Code) -> Error {
        Error::PeerFailure { code }
    }
    fn call(
        &self,
        parent: &RemoteParent,
        environment: EnvironmentId,
        tool: &'static str,
        arguments: Value,
        namespace: &str,
    ) -> RemoteCall {
        RemoteCall {
            environment_id: environment,
            tool,
            arguments,
            limits: parent.limits.clone(),
            namespace: namespace.into(),
            sender: Some(OrchestrationV2LinkedSender {
                environment_id: self.environment_id.clone(),
                thread_id: parent.thread_id.clone(),
                title: parent.title.clone(),
            }),
        }
    }
    pub async fn delegate(
        &self,
        caller: &LinkCaller,
        input: OrchestratorMcpDelegateTaskInput,
    ) -> Result<NodeId> {
        if caller.linked_origin {
            return Err(Error::Identity);
        }
        let parent = self.store.parent(&caller.thread_id).await?;
        let active = parent
            .active
            .as_ref()
            .filter(|active| active.provider_instance_id == caller.provider_instance_id)
            .ok_or_else(|| Self::fail(Code::ParentNotActive))?;
        if input.isolation.flatten() == Some(OrchestratorMcpIsolation::Worktree) {
            return Err(Self::fail(Code::InvalidRequest));
        }
        let target = input
            .target
            .clone()
            .flatten()
            .ok_or_else(|| Self::fail(Code::TargetRequired))?;
        let environment = target
            .environment_id
            .clone()
            .flatten()
            .ok_or_else(|| Self::fail(Code::TargetRequired))?;
        let mut instance = target.provider_instance_id.clone().flatten();
        if let Some(driver) = target.driver_kind.clone().flatten() {
            let providers = self
                .calls
                .call(self.call(
                    &parent,
                    environment.clone(),
                    "orchestrator_capabilities",
                    json!({}),
                    &caller.namespace,
                ))
                .await?
                .into_inner();
            let providers = providers["providers"].as_array().ok_or(Error::Protocol)?;
            if let Some(named) = &instance {
                if providers
                    .iter()
                    .find(|v| v["providerInstanceId"] == named.0)
                    .is_some_and(|v| v["driverKind"] != driver.0)
                {
                    return Err(Self::fail(Code::InvalidRequest));
                }
            } else {
                let eligible = providers
                    .iter()
                    .filter(|v| v["driverKind"] == driver.0 && v["canRunChildTask"] == true)
                    .collect::<Vec<_>>();
                let chosen = eligible
                    .iter()
                    .find(|v| v["providerInstanceId"] == parent.model_selection.instance_id.0)
                    .or_else(|| eligible.first())
                    .ok_or_else(|| Self::fail(Code::ProviderUnavailable))?;
                instance = Some(crate::decode(chosen["providerInstanceId"].clone())?);
            }
        }
        let instance = instance.unwrap_or_else(|| parent.model_selection.instance_id.clone());
        let model = target
            .model
            .clone()
            .flatten()
            .map(|v| v.0)
            .or_else(|| {
                (instance == parent.model_selection.instance_id)
                    .then(|| parent.model_selection.model.clone())
            })
            .ok_or_else(|| Self::fail(Code::TargetRequired))?;
        let mut selection = json!({"instanceId":instance,"model":model});
        if let Some(options) = target.options.clone().flatten() {
            selection["options"] = crate::encode(&options)?;
        } else if instance == parent.model_selection.instance_id
            && model == parent.model_selection.model
            && let Some(options) = &parent.model_selection.options
        {
            selection["options"] = crate::encode(options)?;
        }
        let project = if let Some(project) = target.project_id.flatten() {
            project
        } else {
            let repository = parent
                .repository_key
                .as_ref()
                .ok_or_else(|| Self::fail(Code::TargetRequired))?;
            let mut cursor = Some(0_u64);
            let mut projects = Vec::new();
            let mut seen = HashSet::new();
            while let Some(next) = cursor {
                if !seen.insert(next) {
                    return Err(Error::Protocol);
                }
                let page = self
                    .calls
                    .call(self.call(
                        &parent,
                        environment.clone(),
                        "t3_project_list",
                        json!({"cursor":next,"limit":100}),
                        &caller.namespace,
                    ))
                    .await?
                    .into_inner();
                for project in page["projects"].as_array().ok_or(Error::Protocol)? {
                    if project["repositoryIdentity"]["canonicalKey"] == *repository {
                        projects.push(crate::decode::<ProjectId>(project["id"].clone())?);
                    }
                }
                cursor = page["nextCursor"].as_u64();
            }
            if projects.len() != 1 {
                return Err(Self::fail(Code::InvalidRequest));
            }
            projects.remove(0)
        };
        let role = input
            .role
            .map(|v| crate::encode(&v))
            .transpose()?
            .and_then(|v| v.as_str().map(str::to_owned));
        let prompt = match role.as_deref() {
            None | Some("general") => input.task.0.clone(),
            Some(role) => format!(
                "Act as the {role} sub-agent for this task.\n\n{}",
                input.task.0
            ),
        };
        let key = input
            .client_request_id
            .clone()
            .flatten()
            .map(|v| v.0.0)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let title = input
            .title
            .clone()
            .flatten()
            .map(|v| v.0)
            .unwrap_or_else(|| parent.title.clone());
        let mut arguments = json!({"projectId":project,"title":title,"modelSelection":selection,"message":prompt,"clientRequestId":format!("delegate:{}:{key}",parent.thread_id.0),"delegatedFrom":{"environmentId":self.environment_id,"threadId":parent.thread_id,"title":parent.title}});
        if let Some(OrchestratorMcpRuntimeMode::Mode(mode)) = input.runtime_mode.flatten() {
            if runtime_rank(mode) > runtime_rank(parent.limits.runtime_mode) {
                return Err(Self::fail(Code::RuntimeModeEscalationDenied));
            }
            arguments["runtimeMode"] = crate::encode(&mode)?;
        }
        if let Some(OrchestratorMcpInteractionMode::Mode(mode)) = input.interaction_mode.flatten() {
            if parent.limits.interaction_mode == ProviderInteractionMode::Plan
                && mode != ProviderInteractionMode::Plan
            {
                return Err(Self::fail(Code::InteractionModeEscalationDenied));
            }
            arguments["interactionMode"] = crate::encode(&mode)?;
        }
        let launched = self
            .calls
            .call(self.call(
                &parent,
                environment.clone(),
                "t3_thread_launch",
                arguments,
                &caller.namespace,
            ))
            .await?
            .into_inner();
        let remote: OrchestrationV2RemoteTaskChild = crate::decode(
            json!({"environmentId":environment,"threadId":launched["threadId"],"label":self.links.get(&environment.0).await?.map(|link|link.label).unwrap_or_else(||environment.0.clone()),"title":title}),
        )?;
        let remote = OrchestrationV2RemoteTaskChild {
            run_id: if launched["runId"].is_null() {
                None
            } else {
                Some(Some(crate::decode(launched["runId"].clone())?))
            },
            ..remote
        };
        let command: InternalCommandDelegatedTaskRemoteRequest = crate::decode(
            json!({"commandId":format!("command:mcp:{}:remote-delegate:{}",component(&caller.namespace),component(&key)),"parentThreadId":parent.thread_id,"parentRunId":active.run_id,"parentNodeId":active.node_id,"task":prompt,"driver":"remote","modelSelection":launched["modelSelection"],"remoteChild":remote,"completionWake":if crate::encode(&input.mode)?==json!("wait"){"settled_only"}else{"always"}}),
        )?;
        let command = InternalCommandDelegatedTaskRemoteRequest {
            title: input.title,
            ..command
        };
        let task = match self.store.request(command).await {
            Ok(task) => task,
            Err(error) => {
                let temporary = RemoteTask {
                    key: (parent.thread_id.clone(), NodeId("unrecorded".into())),
                    remote,
                    status: "running".into(),
                    result: None,
                    model_selection: parent.model_selection.clone(),
                    limits: parent.limits.clone(),
                };
                let _ = self.stop_there(&temporary, "stop", None).await;
                return Err(error);
            }
        };
        let _ = self.wake.send((parent.thread_id, task.clone()));
        Ok(task)
    }
    fn follow_call(&self, task: &RemoteTask, tool: &'static str, arguments: Value) -> RemoteCall {
        RemoteCall {
            environment_id: task.remote.environment_id.clone(),
            tool,
            arguments,
            limits: task.limits.clone(),
            namespace: format!("remote-task:{}", task.key.0.0),
            sender: None,
        }
    }
    async fn stop_there(&self, task: &RemoteTask, key: &str, reason: Option<&str>) -> Result<()> {
        let mut arguments = json!({"threadId":task.remote.thread_id,"stop":true,"clientRequestId":format!("{key}:{}",task.remote.thread_id.0)});
        if let Some(reason) = reason {
            arguments["reason"] = json!(reason);
        }
        self.calls
            .call(self.follow_call(task, "t3_thread_interrupt", arguments))
            .await
            .map(|_| ())
    }
    async fn complete(
        &self,
        task: &RemoteTask,
        status: &str,
        result: String,
        stopped: Option<&str>,
    ) -> Result<()> {
        let mut command = json!({"commandId":format!("command:remote-task-complete:{}:{}",task.key.1.0,uuid::Uuid::new_v4()),"parentThreadId":task.key.0,"taskId":task.key.1,"status":terminal_status(status),"result":result});
        if let Some(stopped) = stopped {
            command["stoppedThere"] = json!(stopped);
        }
        if let Some(Some(count)) = &task.remote.stop_count {
            command["stopCount"] = crate::encode(count)?;
        }
        match self.store.complete(crate::decode(command)?).await {
            Ok(()) => Ok(()),
            Err(error) => {
                if self
                    .store
                    .task(&task.key)
                    .await?
                    .is_some_and(|task| task.result.is_some())
                {
                    Ok(())
                } else {
                    Err(error)
                }
            }
        }
    }
    pub async fn follow_once(&self, key: &TaskKey) -> Result<bool> {
        let mut changes = self.store.subscribe();
        let Some(task) = self.store.task(key).await? else {
            return Ok(true);
        };
        if let Some(result) = &task.result {
            if task.remote.stopped_there.flatten() == Some(false)
                || matches!(task.status.as_str(), "cancelled" | "interrupted")
                    && task.remote.stopped_there.flatten() != Some(true)
            {
                let count = task
                    .remote
                    .stop_count
                    .flatten()
                    .map(|v| format!("stop-{v}"))
                    .unwrap_or_else(|| "stop".into());
                let stop = task
                    .remote
                    .run_id
                    .clone()
                    .flatten()
                    .map(|run| format!("{count}:{}", run.0))
                    .unwrap_or(count);
                match self.stop_there(&task, &stop, None).await {
                    Err(Error::PeerFailure {
                        code:
                            Code::RuntimeModeEscalationDenied | Code::InteractionModeEscalationDenied,
                    })
                    | Ok(()) => {}
                    Err(error) => return Err(error),
                }
                self.complete(&task, &task.status, result.clone(), Some("stopped"))
                    .await?;
            }
            return Ok(true);
        }
        let mut arguments = json!({"threadId":task.remote.thread_id,"timeoutMs":600_000,"untilDelegatedWorkSettled":true});
        if let Some(Some(run)) = &task.remote.run_id {
            arguments["runId"] = crate::encode(run)?;
        }
        let changed = async {
            loop {
                match changes.recv().await {
                    Ok(changed) if &changed == key => return,
                    Err(broadcast::error::RecvError::Lagged(_)) => return,
                    Err(broadcast::error::RecvError::Closed) => std::future::pending::<()>().await,
                    _ => {}
                }
            }
        };
        let response = tokio::select! {response=self.calls.call(self.follow_call(&task,"t3_thread_wait",arguments))=>response,_=changed=>return Ok(false)};
        let waited: OrchestratorMcpThreadWaitResult = match response {
            Ok(response) => crate::decode(response.into_inner())?,
            Err(error) => {
                let final_error = matches!(
                    &error,
                    Error::NotLinked
                        | Error::Expired
                        | Error::PeerFailure {
                            code: Code::CapabilityDenied | Code::ThreadNotFound | Code::RunNotFound
                        }
                ) || matches!(
                    &error,
                    Error::PeerFailure {
                        code: Code::InvalidRequest
                    }
                ) && self
                    .links
                    .get(&task.remote.environment_id.0)
                    .await?
                    .is_none();
                if !final_error {
                    return Err(error);
                }
                self.complete(&task, "failed", error.to_string(), Some("stopped"))
                    .await?;
                return Ok(true);
            }
        };
        if waited.timed_out {
            return Ok(false);
        }
        let mut status = crate::encode(&waited.status)?
            .as_str()
            .ok_or(Error::Protocol)?
            .to_owned();
        let mut run = waited.run_id.map(|v| v.0);
        if !matches!(
            status.as_str(),
            "completed" | "failed" | "cancelled" | "interrupted" | "rolled_back"
        ) && run.is_some()
        {
            return Ok(false);
        }
        if !self.store.shared_result_run(key).await?
            && let Some(Some(result_run)) = waited.result_run
        {
            run = Some(result_run.run_id.0);
            status = crate::encode(&result_run.status)?
                .as_str()
                .ok_or(Error::Protocol)?
                .into();
        }
        let result = self
            .last_reply(&task, run.as_deref(), status == "failed")
            .await?;
        let status = if run.is_none() && result.is_none() {
            "failed"
        } else {
            &status
        };
        self.complete(
            &task,
            status,
            result.unwrap_or_else(|| {
                if run.is_none() {
                    "The thread in the linked environment has no run to follow.".into()
                } else {
                    "(The thread in the linked environment ended without a reply.)".into()
                }
            }),
            Some("ran-out"),
        )
        .await?;
        Ok(self
            .store
            .task(key)
            .await?
            .is_none_or(|task| task.remote.stopped_there.flatten() == Some(true)))
    }
    async fn last_reply(
        &self,
        task: &RemoteTask,
        run: Option<&str>,
        failed: bool,
    ) -> Result<Option<String>> {
        let mut cursor = None;
        let mut seen = HashSet::new();
        let mut reply = None;
        let mut error = None;
        let mut model = None;
        loop {
            let mut args = json!({"threadId":task.remote.thread_id,"view":if failed{"activity"}else{"messages"},"limit":100,"runLimit":50,"maxCharsPerItem":20_000});
            if let Some(cursor) = cursor {
                args["afterPosition"] = json!(cursor);
            }
            let page = self
                .calls
                .call(self.follow_call(task, "t3_thread_read", args))
                .await?
                .into_inner();
            if model.is_none() {
                model = page["recentRuns"]
                    .as_array()
                    .and_then(|runs| runs.iter().find(|value| value["runId"].as_str() == run))
                    .cloned();
            }
            for item in page["items"].as_array().ok_or(Error::Protocol)? {
                if run.is_some() && item["runId"].as_str() != run {
                    continue;
                }
                if item["type"] == "assistant_message" {
                    reply = Some(item.clone());
                }
                if failed && item["type"] == "error" {
                    error = Some(item.clone());
                }
            }
            if page["hasMore"] != true {
                break;
            }
            cursor = page["nextPosition"].as_u64();
            let Some(next) = cursor else { break };
            if !seen.insert(next) {
                return Err(Error::Protocol);
            }
        }
        let Some(item) = error.or(reply) else {
            return Ok(None);
        };
        let mut text = item["text"].as_str().unwrap_or("").to_owned();
        let mut offset = if item["textTruncated"] == true {
            item["nextTextOffset"].as_u64()
        } else {
            None
        };
        let mut offsets = HashSet::new();
        while let Some(next) = offset {
            if !offsets.insert(next) {
                return Err(Error::Protocol);
            }
            let page=self.calls.call(self.follow_call(task,"t3_thread_read",json!({"threadId":task.remote.thread_id,"itemId":item["itemId"],"textOffset":next,"maxCharsPerItem":20_000}))).await?.into_inner();
            let Some(part) = page["items"].as_array().and_then(|items| items.first()) else {
                break;
            };
            text.push_str(part["text"].as_str().unwrap_or(""));
            offset = if part["textTruncated"] == true {
                part["nextTextOffset"].as_u64()
            } else {
                None
            };
        }
        if let Some(model) = model
            && let (Some(requested), Some(executed)) =
                (model["model"].as_str(), model["executedModel"].as_str())
            && requested != executed
        {
            text = format!(
                "Requested model {requested}; the provider reported running {executed}.\n\n{text}"
            );
        }
        Ok(Some(text))
    }
    pub async fn cancel(
        &self,
        caller: &LinkCaller,
        key: &TaskKey,
        reason: Option<&str>,
        client_request_id: Option<&str>,
    ) -> Result<OrchestratorMcpTaskCancelResult> {
        if caller.linked_origin || caller.thread_id != key.0 {
            return Err(Error::Identity);
        }
        let task = self
            .store
            .task(key)
            .await?
            .ok_or_else(|| Self::fail(Code::TaskNotFound))?;
        let stop = format!(
            "cancel:{}:{}",
            component(&caller.namespace),
            client_request_id
                .map(str::to_owned)
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
        );
        let stopped = self.stop_there(&task, &stop, reason).await;
        if let Some(result) = &task.result {
            if stopped.is_ok() {
                let _ = self
                    .complete(&task, &task.status, result.clone(), Some("stopped"))
                    .await;
            }
            return crate::decode(json!({"taskId":key.1,"status":terminal_status(&task.status)}));
        }
        let result = reason
            .map(|reason| format!("Cancelled: {reason}"))
            .unwrap_or_else(|| "Cancelled.".into());
        let result = if stopped.is_err() {
            format!(
                "{result} It could not be stopped in {} and may still run there.",
                task.remote.label
            )
        } else {
            result
        };
        self.complete(
            &task,
            "cancelled",
            result,
            stopped.is_ok().then_some("stopped"),
        )
        .await?;
        let _ = self.wake.send(key.clone());
        crate::decode(json!({"taskId":key.1,"status":"cancel_requested"}))
    }
    pub async fn follow_up(
        &self,
        caller: &LinkCaller,
        environment: EnvironmentId,
        message: &str,
        sent: OrchestratorMcpThreadSendResult,
    ) -> Result<OrchestratorMcpThreadSendResult> {
        if caller.linked_origin {
            return Err(Error::Identity);
        }
        let delivery = crate::encode(&sent.delivery)?;
        if delivery != "started" && delivery != "queued" {
            return Ok(sent);
        }
        // The host returns the finished owner as a narrow task query, then ordinary request records it.
        let Some(previous) = self
            .store
            .finished_owner(&caller.thread_id, &environment, &sent.thread_id)
            .await?
        else {
            return Ok(sent);
        };
        let parent = self.store.parent(&caller.thread_id).await?;
        let Some(active) = parent.active else {
            return Ok(sent);
        };
        let command: InternalCommandDelegatedTaskRemoteRequest = crate::decode(
            json!({"commandId":format!("command:remote-followup:{}:{}:{}",component(&caller.thread_id.0),component(&environment.0),component(&sent.message_id.0)),"parentThreadId":caller.thread_id,"parentRunId":active.run_id,"parentNodeId":active.node_id,"task":message,"driver":"remote","modelSelection":previous.model_selection,"remoteChild":{"environmentId":environment,"threadId":sent.thread_id,"label":previous.remote.label,"runId":sent.run_id},"completionWake":"always"}),
        )?;
        let id = self.store.request(command).await?;
        let _ = self.wake.send((caller.thread_id.clone(), id.clone()));
        let mut sent = sent;
        sent.task_id = Some(Some(id));
        Ok(sent)
    }
    pub fn start(self: &Arc<Self>) -> RemoteRunner {
        let service = self.clone();
        let cancel = t3_process::CancellationToken::new();
        let cancelled = cancel.clone();
        let mut events = self.store.subscribe();
        let mut own = self.wake.subscribe();
        let task = tokio::spawn(async move {
            let mut pending = service.store.outstanding().await?;
            let mut active = HashSet::new();
            let mut again = HashSet::new();
            let mut tasks = tokio::task::JoinSet::new();
            loop {
                for key in pending.drain(..) {
                    if active.insert(key.clone()) {
                        let service = service.clone();
                        let cancel = cancelled.clone();
                        tasks.spawn(async move {let mut delay=2_u64;loop {let result=tokio::select! {_=cancel.cancelled()=>break,result=service.follow_once(&key)=>result};match result {Ok(true)=>break,Ok(false)=>{delay=2;},Err(_)=>{tokio::select!{_=cancel.cancelled()=>break,_=tokio::time::sleep(Duration::from_secs(delay))=>{}}delay=(delay*2).min(300);}}}key});
                    } else {
                        again.insert(key);
                    }
                }
                tokio::select! {
                    _=cancelled.cancelled()=>{while tasks.join_next().await.is_some(){}return Ok(());},
                    event=events.recv()=>match event {Ok(key)=>pending.push(key),Err(broadcast::error::RecvError::Lagged(_))=>pending.extend(service.store.outstanding().await?),Err(broadcast::error::RecvError::Closed)=>return Err(Error::Storage)},
                    event=own.recv()=>if let Ok(key)=event{pending.push(key)},
                    result=tasks.join_next(),if !tasks.is_empty()=>if let Some(Ok(key))=result{active.remove(&key);if again.remove(&key){pending.push(key);}},
                }
            }
        });
        RemoteRunner { cancel, task }
    }
}
pub struct RemoteRunner {
    cancel: t3_process::CancellationToken,
    task: tokio::task::JoinHandle<Result<()>>,
}
impl RemoteRunner {
    pub async fn shutdown(mut self) -> Result<()> {
        self.cancel.cancel();
        (&mut self.task).await.map_err(|_| Error::Process)?
    }
}
impl Drop for RemoteRunner {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
fn terminal_status(status: &str) -> &str {
    match status {
        "completed" | "failed" | "interrupted" => status,
        _ => "cancelled",
    }
}
fn runtime_rank(mode: RuntimeMode) -> u8 {
    match mode {
        RuntimeMode::ApprovalRequired => 0,
        RuntimeMode::AutoAcceptEdits => 1,
        RuntimeMode::Auto => 2,
        RuntimeMode::FullAccess => 3,
    }
}
