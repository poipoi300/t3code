mod support {
    use crate::checkpoint_service_test::support::{self as checkpoints, *};
    use async_trait::async_trait;
    use serde_json::{Value, json};
    use std::{
        collections::HashSet,
        path::PathBuf,
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
    };
    use t3_contracts::{
        provider_instance::ProviderInstanceId,
        provider_policy::{ProviderInteractionMode, RuntimeMode},
    };
    use t3_orchestration::{
        api::*,
        decision::engine::PureDecisionEngine,
        error::{Error, Result},
        ports::{
            checkpoints_forks::{
                checkpoint_capture_service::domain_event, thread_launch_service::*,
            },
            decisions::{dispatch_mode_limit::DispatchModes, orchestrator::command_identity},
        },
        stores::receipts::{self, AggregateId, CommandReceipt, ReceiptStatus},
    };
    use tokio::sync::{Semaphore, oneshot};
    use tokio_util::sync::CancellationToken;

    pub struct Commands {
        pub h: Arc<Harness>,
        pub calls: Mutex<Vec<(Value, Option<DispatchModes>)>>,
        lane: tokio::sync::Mutex<()>,
        pub reject_message: AtomicBool,
        pub reject_release: AtomicBool,
    }
    #[async_trait]
    impl LaunchCommands for Commands {
        async fn dispatch(
            &self,
            command: OrchestrationV2ServerCommand,
            limit: Option<DispatchModes>,
        ) -> Result<CommandReceipt> {
            let _lane = self.lane.lock().await;
            let (id, kind, thread_id) = command_identity(&command)?;
            let value = serde_json::to_value(&command)?;
            self.calls
                .lock()
                .expect("calls")
                .push((value.clone(), limit));
            let receipt_id = id.clone();
            if let Some(receipt) = self
                .h
                .db
                .stores
                .read(move |db| receipts::get(db, &receipt_id))
                .await?
            {
                if receipt.aggregate != AggregateId::Thread(thread_id.clone())
                    || receipt.command_type != kind
                {
                    return Err(Error::Rejected {
                        command_id: id,
                        reason: "receipt identity mismatch".into(),
                    });
                }
                return Ok(receipt);
            }
            if (kind == "message.dispatch" && self.reject_message.load(Ordering::SeqCst))
                || (kind == "prepared-run.release" && self.reject_release.load(Ordering::SeqCst))
            {
                return Err(Error::Unsupported("scripted command rejection"));
            }
            let projection = self.h.reader.projection(thread_id.clone()).await?;
            let decision = match kind.as_str() {
                // This command fake supplies preparation outcomes; the tests exercise launch coordination.
                "message.dispatch" => {
                    let mut run = serde_json::to_value(checkpoints::run(1, "preparing"))?;
                    run["threadId"] = json!(thread_id);
                    run["id"] = json!(format!("run:{}", id.0));
                    run["userMessageId"] = value["messageId"].clone();
                    run["rootNodeId"] = Value::Null;
                    run["workspacePreparation"] =
                        value["dispatchMode"]["workspaceStrategy"].clone();
                    Decision {
                        events: vec![domain_event(
                            self.h.ids.as_ref(),
                            &thread_id,
                            &time(),
                            "run.created",
                            run,
                            json!({}),
                        )?],
                        ..Default::default()
                    }
                }
                "prepared-run.release" => {
                    let mut run = self
                        .h
                        .reader
                        .run(
                            thread_id.clone(),
                            serde_json::from_value(value["runId"].clone())?,
                        )
                        .await?
                        .expect("prepared run");
                    run.status = OrchestrationV2RunStatus::Starting;
                    Decision {
                        events: vec![domain_event(
                            self.h.ids.as_ref(),
                            &thread_id,
                            &time(),
                            "run.updated",
                            run,
                            json!({}),
                        )?],
                        ..Default::default()
                    }
                }
                "prepared-run.progress" => {
                    let run = self
                        .h
                        .reader
                        .run(
                            thread_id.clone(),
                            serde_json::from_value(value["runId"].clone())?,
                        )
                        .await?
                        .expect("prepared run");
                    Decision {
                        events: vec![domain_event(
                            self.h.ids.as_ref(),
                            &thread_id,
                            &time(),
                            "run.updated",
                            run,
                            json!({}),
                        )?],
                        ..Default::default()
                    }
                }
                "prepared-run.fail" => {
                    let mut run = self
                        .h
                        .reader
                        .run(
                            thread_id.clone(),
                            serde_json::from_value(value["runId"].clone())?,
                        )
                        .await?
                        .expect("prepared run");
                    if run.status == OrchestrationV2RunStatus::Preparing {
                        run.status = OrchestrationV2RunStatus::Failed;
                        Decision {
                            events: vec![domain_event(
                                self.h.ids.as_ref(),
                                &thread_id,
                                &time(),
                                "run.updated",
                                run,
                                json!({}),
                            )?],
                            ..Default::default()
                        }
                    } else {
                        Decision::default()
                    }
                }
                _ => PureDecisionEngine.decide(
                    &DecisionContext {
                        projection,
                        now: time(),
                        ids: (0..100)
                            .map(|_| (IdKind::Event, self.h.ids.next(IdKind::Event)))
                            .collect(),
                        capabilities: None,
                    },
                    &command,
                )?,
            };
            let result = self
                .h
                .sink
                .commit(CommitRequest {
                    receipt: Some(CommandReceipt {
                        command_id: id.clone(),
                        aggregate: AggregateId::Thread(thread_id),
                        command_type: kind,
                        accepted_at: time().0,
                        result_sequence: 0,
                        status: ReceiptStatus::Accepted,
                        error: None,
                    }),
                    guard: CommitGuard::None,
                    decision,
                })
                .await?;
            result.receipt.ok_or(Error::Stale)
        }
    }

    type SetupCompletion = oneshot::Receiver<Result<Option<i32>>>;

    pub struct Environment {
        pub scratch: AtomicBool,
        pub folder: Mutex<Option<PathBuf>>,
        pub repository: AtomicBool,
        pub head: AtomicBool,
        pub probe_failure: AtomicBool,
        pub branches: Mutex<Vec<String>>,
        pub refs: Mutex<HashSet<String>>,
        pub remote: AtomicBool,
        pub fetched: AtomicBool,
        pub repository_gate: Mutex<Option<Arc<Gate>>>,
        pub commit_gate: Mutex<Option<Arc<Gate>>>,
        pub branches_gate: Mutex<Option<Arc<Gate>>>,
        pub setup_count: AtomicUsize,
        pub setup_entered: Semaphore,
        pub setup_gate: Mutex<Option<Arc<Gate>>>,
        pub preflight_gate: Mutex<Option<Arc<Gate>>>,
        pub rename_gate: Mutex<Option<Arc<Gate>>>,
        pub naming_gate: Mutex<Option<Arc<Gate>>>,
        pub setup_completion: Mutex<Option<(bool, SetupCompletion)>>,
        pub rename_failure: AtomicBool,
        pub closed: AtomicUsize,
        pub folder_count: AtomicUsize,
    }
    impl Default for Environment {
        fn default() -> Self {
            Self {
                scratch: AtomicBool::new(false),
                folder: Mutex::new(None),
                repository: AtomicBool::new(true),
                head: AtomicBool::new(true),
                probe_failure: AtomicBool::new(false),
                branches: Mutex::new(vec![]),
                refs: Mutex::new(HashSet::from(["main".into()])),
                remote: AtomicBool::new(false),
                fetched: AtomicBool::new(true),
                repository_gate: Mutex::new(None),
                commit_gate: Mutex::new(None),
                branches_gate: Mutex::new(None),
                setup_count: AtomicUsize::new(0),
                setup_entered: Semaphore::new(0),
                setup_gate: Mutex::new(None),
                preflight_gate: Mutex::new(None),
                rename_gate: Mutex::new(None),
                naming_gate: Mutex::new(None),
                setup_completion: Mutex::new(None),
                rename_failure: AtomicBool::new(false),
                closed: AtomicUsize::new(0),
                folder_count: AtomicUsize::new(0),
            }
        }
    }
    pub async fn gate(gate: Option<Arc<Gate>>, cancellation: CancellationToken) -> Result<()> {
        if let Some(gate) = gate {
            gate.entered.add_permits(1);
            tokio::select! {_=cancellation.cancelled()=>return Err(Error::Cancelled),permit=gate.release.acquire()=>permit.expect("gate").forget()}
        }
        Ok(())
    }
    #[async_trait]
    impl LaunchEnvironment for Environment {
        async fn is_scratch(&self, _: &ProjectId) -> Result<bool> {
            Ok(self.scratch.load(Ordering::SeqCst))
        }
        async fn reject_during_clone(&self, _: &ProjectId) -> Result<()> {
            let wait = self.preflight_gate.lock().expect("preflight").clone();
            gate(wait, CancellationToken::new()).await?;
            Ok(())
        }
        async fn assert_provider_enabled(
            &self,
            _: &ProjectId,
            _: &CommandId,
            _: &ProviderInstanceId,
        ) -> Result<()> {
            Ok(())
        }
        async fn folder_for_thread(
            &self,
            _: &ProjectId,
            _: &ThreadId,
            _: &str,
        ) -> Result<Option<PathBuf>> {
            self.folder_count.fetch_add(1, Ordering::SeqCst);
            Ok(self.folder.lock().expect("folder").clone())
        }
        async fn repository_root(&self, cwd: PathBuf) -> Result<Option<PathBuf>> {
            let wait = self
                .repository_gate
                .lock()
                .expect("repository gate")
                .clone();
            gate(wait, CancellationToken::new()).await?;
            Ok(self.repository.load(Ordering::SeqCst).then_some(cwd))
        }
        async fn has_commit(&self, _: PathBuf, reference: &str) -> Result<bool> {
            let wait = self.commit_gate.lock().expect("commit gate").clone();
            gate(wait, CancellationToken::new()).await?;
            if self.probe_failure.load(Ordering::SeqCst) {
                return Err(Error::Unsupported("reference probing unavailable"));
            }
            Ok(if reference == "HEAD" {
                self.head.load(Ordering::SeqCst)
            } else {
                self.refs.lock().expect("refs").contains(reference)
            })
        }
        async fn has_ref(&self, _: PathBuf, reference: &str) -> Result<bool> {
            Ok(self.refs.lock().expect("refs").contains(reference))
        }
        async fn remote_for_base(&self, _: PathBuf, _: &str) -> Result<Option<String>> {
            Ok(self
                .remote
                .load(Ordering::SeqCst)
                .then(|| "upstream".into()))
        }
        async fn fetch_worktree_base(&self, _: PathBuf, _: &str, _: &str) -> Result<bool> {
            Ok(self.fetched.load(Ordering::SeqCst))
        }
        async fn local_branches(&self, _: PathBuf) -> Result<Vec<String>> {
            let wait = self.branches_gate.lock().expect("branches gate").clone();
            gate(wait, CancellationToken::new()).await?;
            Ok(self.branches.lock().expect("branches").clone())
        }
        async fn temporary_branch(&self, _: &ProjectId) -> Result<String> {
            Ok("t3/1234abcd".into())
        }
        async fn generate_branch(
            &self,
            _: &ProjectId,
            _: PathBuf,
            _: &InitialMessage,
            cancellation: CancellationToken,
        ) -> Result<GeneratedBranch> {
            let wait = self.naming_gate.lock().expect("naming").clone();
            gate(wait, cancellation).await?;
            if self.rename_failure.load(Ordering::SeqCst) {
                return Err(Error::Unsupported("branch naming failed"));
            }
            Ok(GeneratedBranch {
                branch: "feature/generated".into(),
                exact_name: true,
            })
        }
        async fn rename_branch(
            &self,
            _: PathBuf,
            _: &str,
            generated: GeneratedBranch,
            cancellation: CancellationToken,
        ) -> Result<String> {
            let wait = self.rename_gate.lock().expect("rename").clone();
            gate(wait, cancellation).await?;
            Ok(generated.branch)
        }
        async fn setup(&self, _: SetupInput, cancellation: CancellationToken) -> Result<SetupRun> {
            self.setup_count.fetch_add(1, Ordering::SeqCst);
            self.setup_entered.add_permits(1);
            let wait = self.setup_gate.lock().expect("setup").clone();
            gate(wait, cancellation).await?;
            Ok(
                match self.setup_completion.lock().expect("completion").take() {
                    Some((asynchronous, completion)) => SetupRun::Started {
                        terminal_id: "setup-terminal".into(),
                        asynchronous,
                        completion: Some(completion),
                    },
                    None => SetupRun::Skipped,
                },
            )
        }
        async fn close_terminal(&self, _: &ThreadId, _: &str) -> Result<()> {
            self.closed.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    pub struct Workspaces {
        pub root: PathBuf,
        pub prepares: AtomicUsize,
        pub removes: AtomicUsize,
        pub gate: Mutex<Option<Arc<Gate>>>,
    }
    #[async_trait]
    impl WorkspaceBackend for Workspaces {
        async fn prepare(
            &self,
            _: ProjectId,
            thread: ThreadId,
            strategy: OrchestrationV2ThreadLaunchWorkspaceStrategy,
            cancellation: CancellationToken,
        ) -> Result<Workspace> {
            self.prepares.fetch_add(1, Ordering::SeqCst);
            let wait = self.gate.lock().expect("prepare").clone();
            gate(wait, cancellation).await?;
            Ok(match strategy {
                OrchestrationV2ThreadLaunchWorkspaceStrategy::Root(s) => Workspace {
                    cwd: self.root.clone(),
                    branch: s
                        .branch
                        .flatten()
                        .map(|s| s.0)
                        .or_else(|| Some("checkout".into())),
                    worktree_path: None,
                },
                OrchestrationV2ThreadLaunchWorkspaceStrategy::ExistingWorktree(s) => Workspace {
                    cwd: PathBuf::from(&s.worktree_path.0),
                    worktree_path: Some(PathBuf::from(s.worktree_path.0)),
                    branch: s.branch.flatten().map(|s| s.0),
                },
                OrchestrationV2ThreadLaunchWorkspaceStrategy::Worktree(s) => {
                    let path = self.root.join(thread.0.replace(':', "-"));
                    Workspace {
                        cwd: path.clone(),
                        worktree_path: Some(path),
                        branch: s.branch.flatten().map(|s| s.0),
                    }
                }
            })
        }
        async fn remove(&self, _: Workspace, _: CancellationToken) -> Result<()> {
            self.removes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn current_branch(&self, _: PathBuf) -> Result<Option<String>> {
            Ok(Some("checkout".into()))
        }
    }

    pub struct LaunchHarness {
        pub h: Arc<Harness>,
        pub commands: Arc<Commands>,
        pub environment: Arc<Environment>,
        pub workspaces: Arc<Workspaces>,
        pub service: ThreadLaunchService,
        pub temp: tempfile::TempDir,
    }
    impl LaunchHarness {
        pub async fn milestone<T>(
            &self,
            name: &str,
            milestone: impl std::future::Future<Output = T>,
        ) -> Result<T> {
            tokio::select! {
                biased;
                result=milestone=>Ok(result),
                _=self.service.drain()=>{
                    let calls=self.commands.calls.lock().expect("calls");
                    let failure=calls.iter().rev().find(|(command,_)|command["type"]=="prepared-run.fail").map(|(command,_)|&command["failure"]);
                    Err(Error::Service {operation:"test.launch-preparation",detail:format!("Preparation ended before {name}: {failure:?}")})
                },
            }
        }
        pub async fn new() -> Result<Self> {
            let temp = tempfile::tempdir()?;
            let h = Arc::new(Harness::new().await?);
            let mut project = serde_json::to_value(t3_orchestration::testing::project_event(
                "project:event",
                "project:test",
            )?)?;
            project["payload"]["workspaceRoot"] = json!(temp.path());
            let project = serde_json::from_value(project)?;
            h.db.stores
                .transaction(move |db| t3_orchestration::stores::projects::apply(db, &project))
                .await?;
            let commands = Arc::new(Commands {
                h: h.clone(),
                calls: Mutex::new(vec![]),
                lane: tokio::sync::Mutex::new(()),
                reject_message: AtomicBool::new(false),
                reject_release: AtomicBool::new(false),
            });
            let environment = Arc::new(Environment::default());
            let workspaces = Arc::new(Workspaces {
                root: temp.path().to_owned(),
                prepares: AtomicUsize::new(0),
                removes: AtomicUsize::new(0),
                gate: Mutex::new(None),
            });
            let service = ThreadLaunchService::new(LaunchServices {
                database: h.db.stores.clone(),
                projections: h.reader.clone(),
                events: h.sink.clone(),
                commands: commands.clone(),
                workspaces: workspaces.clone(),
                environment: environment.clone(),
                ids: h.ids.clone(),
                clock: h.clock.clone(),
            });
            Ok(Self {
                h,
                commands,
                environment,
                workspaces,
                service,
                temp,
            })
        }
        pub fn input(&self, id: &str, strategy: Value, message: bool) -> LaunchInput {
            LaunchInput {
                command_id: CommandId(id.into()),
                thread_id: None,
                reuse_existing_thread: false,
                project_id: ProjectId("project:test".into()),
                title: "Launch title".into(),
                generate_title: false,
                model_selection: thread("thread:test", None).model_selection,
                runtime_mode: RuntimeMode::FullAccess,
                interaction_mode: ProviderInteractionMode::Default,
                workspace_strategy: serde_json::from_value(strategy).expect("strategy"),
                initial_message: message.then(|| InitialMessage {
                    message_id: None,
                    scheduled_task_id: None,
                    sender_thread_id: None,
                    text: "Build it".into(),
                    attachments: vec![],
                    context: None,
                }),
                imported_native_thread: None,
                created_by: OrchestrationV2Actor::User,
                creation_source: OrchestrationV2CreationSource::Web,
                link_origin: None,
                delegated_from: None,
            }
        }
    }
    pub fn blocked() -> Arc<Gate> {
        Arc::new(Gate {
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
        })
    }
}

#[tokio::test]
async fn explicit_branch_preparation_does_not_probe_temporary_branch_namespace() -> Result<()> {
    let h = LaunchHarness::new().await?;
    h.environment.probe_failure.store(true, Ordering::SeqCst);
    let result = h
        .service
        .launch(h.input(
            "explicit",
            json!({"type":"worktree","baseRef":"main","branch":"feature/parser"}),
            true,
        ))
        .await?;
    h.service.drain().await;
    let projection =
        h.h.reader
            .projection(result.thread_id)
            .await?
            .expect("thread");
    assert_eq!(
        projection.runs[0].status,
        OrchestrationV2RunStatus::Starting
    );
    assert_eq!(
        projection.thread.branch.map(|b| b.0),
        Some("feature/parser".into())
    );
    Ok(())
}

use serde_json::json;
use std::sync::{Arc, atomic::Ordering};
use support::*;
use t3_orchestration::ports::checkpoints_forks::thread_launch_service::{
    LaunchProgress, LaunchServices, LaunchWorkspacePreparation, PreparationInput,
    PreparationObserver, SetupStage, StageStatus, StageUpdate, ThreadLaunchService,
};
use t3_orchestration::{
    api::*,
    error::{Error, Result},
    ports::checkpoints_forks::checkpoint_restore_safety::expanded_id,
};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn root_launch_records_checkout_and_replays_without_reallocating_or_preparing() -> Result<()>
{
    let h = LaunchHarness::new().await?;
    let input = h.input("launch:root", json!({"type":"root"}), false);
    let first = h.service.launch(input.clone()).await?;
    h.service.drain().await;
    let replay = h.service.launch(input).await?;
    h.service.drain().await;
    assert!(!first.resumed);
    assert!(replay.resumed);
    assert_eq!(first.thread_id, replay.thread_id);
    assert_eq!(
        replay
            .projection
            .thread
            .branch
            .as_ref()
            .map(|b| b.0.as_str()),
        Some("checkout")
    );
    assert_eq!(h.workspaces.prepares.load(Ordering::SeqCst), 1);
    assert_eq!(h.environment.folder_count.load(Ordering::SeqCst), 1);
    assert_eq!(h.environment.setup_count.load(Ordering::SeqCst), 0);
    h.service.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn initial_message_preserves_attribution_and_agent_mode_limit_and_title_seed() -> Result<()> {
    let h = LaunchHarness::new().await?;
    let mut input = h.input("launch:message", json!({"type":"root"}), true);
    input.created_by = OrchestrationV2Actor::Agent;
    input.generate_title = true;
    let message = input.initial_message.as_mut().expect("message");
    message.scheduled_task_id = Some(ScheduledTaskId("scheduled:1".into()));
    message.sender_thread_id = Some(ThreadId("sender:1".into()));
    let result = h.service.launch(input.clone()).await?;
    h.service.drain().await;
    assert!(result.sequence.is_some());
    let replay = h.service.launch(input).await?;
    h.service.drain().await;
    assert!(replay.resumed);
    let calls = h.commands.calls.lock().expect("calls");
    let (message, limit) = calls
        .iter()
        .find(|(c, _)| c["type"] == "message.dispatch")
        .expect("message");
    assert_eq!(message["scheduledTaskId"], "scheduled:1");
    assert_eq!(message["senderThreadId"], "sender:1");
    assert_eq!(message["titleSeed"], "Launch title");
    assert!(limit.is_some());
    assert_eq!(h.workspaces.prepares.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn independent_launches_prepare_concurrently_and_same_command_prepares_once() -> Result<()> {
    let h = LaunchHarness::new().await?;
    let gate = blocked();
    *h.workspaces.gate.lock().expect("prepare") = Some(gate.clone());
    let input = h.input(
        "launch:a",
        json!({"type":"worktree","baseRef":"main"}),
        true,
    );
    let other = h.input(
        "launch:b",
        json!({"type":"worktree","baseRef":"main"}),
        true,
    );
    let (a, replay, b) = tokio::join!(
        h.service.launch(input.clone()),
        h.service.launch(input),
        h.service.launch(other)
    );
    let (a, replay, b) = (a?, replay?, b?);
    assert_eq!(a.thread_id, replay.thread_id);
    assert_ne!(a.resumed, replay.resumed);
    assert_ne!(a.thread_id, b.thread_id);
    h.milestone("both preparations entered", async {
        gate.entered
            .acquire_many(2)
            .await
            .expect("both preparations entered")
            .forget();
    })
    .await?;
    assert_eq!(h.workspaces.prepares.load(Ordering::SeqCst), 2);
    gate.release.add_permits(2);
    h.service.drain().await;
    Ok(())
}

#[tokio::test]
async fn scratch_worktree_request_allocates_own_folder_and_persists_root_preparation() -> Result<()>
{
    let h = LaunchHarness::new().await?;
    h.environment.scratch.store(true, Ordering::SeqCst);
    let folder = h.temp.path().join("scratch");
    *h.environment.folder.lock().expect("folder") = Some(folder.clone());
    let result = h
        .service
        .launch(h.input("scratch", json!({"type":"worktree","baseRef":"main"}), true))
        .await?;
    h.service.drain().await;
    let thread = h.h.reader.thread(result.thread_id).await?.expect("thread");
    assert_eq!(
        thread.worktree_path.map(|p| p.0),
        Some(folder.to_string_lossy().into_owned())
    );
    let calls = h.commands.calls.lock().expect("calls");
    let message = &calls
        .iter()
        .find(|(c, _)| c["type"] == "message.dispatch")
        .expect("message")
        .0;
    assert_eq!(
        message["dispatchMode"]["workspaceStrategy"]["type"],
        "existing_worktree"
    );
    assert_eq!(h.environment.setup_count.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn worktree_base_validation_matches_git_exceptions_and_branch_namespace_conflicts()
-> Result<()> {
    let h = LaunchHarness::new().await?;
    for (base, from_origin, head, remote, fetched, accepted) in [
        ("main", false, true, false, true, true),
        ("missing", false, true, false, true, false),
        ("missing", false, false, false, true, true),
        ("missing", true, true, true, true, true),
        ("origin/deleted", true, true, true, false, false),
        (":/initial", false, true, false, true, true),
    ] {
        h.environment.head.store(head, Ordering::SeqCst);
        h.environment.remote.store(remote, Ordering::SeqCst);
        h.environment.fetched.store(fetched, Ordering::SeqCst);
        let strategy = serde_json::from_value(
            json!({"type":"worktree","baseRef":base,"startFromOrigin":from_origin}),
        )?;
        assert_eq!(
            h.service
                .check_worktree_base(ProjectId("project:test".into()), &strategy, None)
                .await
                .is_ok(),
            accepted,
            "{base}"
        );
    }
    h.environment.head.store(true, Ordering::SeqCst);
    for (existing, requested) in [("feature", "feature/child"), ("feature/child", "feature")] {
        *h.environment.branches.lock().expect("branches") = vec![existing.into()];
        let strategy =
            serde_json::from_value(json!({"type":"worktree","baseRef":"main","branch":requested}))?;
        assert!(
            h.service
                .check_worktree_base(ProjectId("project:test".into()), &strategy, None)
                .await
                .is_err()
        );
    }
    h.environment.repository.store(false, Ordering::SeqCst);
    let strategy = serde_json::from_value(json!({"type":"worktree","baseRef":"main"}))?;
    assert!(
        h.service
            .check_worktree_base(ProjectId("project:test".into()), &strategy, None)
            .await
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn setup_exit_and_branch_rename_finish_before_releasing_run() -> Result<()> {
    let h = LaunchHarness::new().await?;
    let gate = blocked();
    *h.environment.rename_gate.lock().expect("rename") = Some(gate.clone());
    let (finish, completion) = tokio::sync::oneshot::channel();
    *h.environment.setup_completion.lock().expect("completion") = Some((false, completion));
    let result = h
        .service
        .launch(h.input("rename", json!({"type":"worktree","baseRef":"main"}), true))
        .await?;
    h.milestone("naming entered", async {
        gate.entered
            .acquire()
            .await
            .expect("naming entered")
            .forget();
    })
    .await?;
    // Exercise the same nonblocking reservation cleanup takes while the rename is in flight.
    use t3_orchestration::ports::checkpoints_forks::checkpoint_restore_safety::WorkspaceReservations;
    let path = h.workspaces.root.join(result.thread_id.0.replace(':', "-"));
    std::fs::create_dir_all(&path)?;
    let sentinel = path.join("keep");
    std::fs::write(&sentinel, "keep")?;
    if let Some(_permit) = WorkspaceReservations::global().try_reserve_cleanup(&path)? {
        std::fs::remove_file(&sentinel)?;
    }
    assert!(
        sentinel.exists(),
        "cleanup obtained deletion permission during branch rename"
    );
    finish.send(Ok(Some(0))).expect("setup completion");
    assert!(
        !h.commands
            .calls
            .lock()
            .expect("calls")
            .iter()
            .any(|(c, _)| c["type"] == "prepared-run.release")
    );
    gate.release.add_permits(1);
    h.service.drain().await;
    let thread = h.h.reader.thread(result.thread_id).await?.expect("thread");
    assert_eq!(thread.branch.map(|b| b.0), Some("feature/generated".into()));
    let calls = h.commands.calls.lock().expect("calls");
    let rename = calls
        .iter()
        .position(|(c, _)| c["commandId"] == "rename:branch-rename")
        .expect("rename");
    let release = calls
        .iter()
        .position(|(c, _)| c["type"] == "prepared-run.release")
        .expect("release");
    assert!(rename < release);
    Ok(())
}

#[tokio::test]
async fn rename_failure_records_notice_and_keeps_temporary_branch() -> Result<()> {
    let h = LaunchHarness::new().await?;
    h.environment.rename_failure.store(true, Ordering::SeqCst);
    let result = h
        .service
        .launch(h.input(
            "rename:failed",
            json!({"type":"worktree","baseRef":"main"}),
            true,
        ))
        .await?;
    h.service.drain().await;
    let projection =
        h.h.reader
            .projection(result.thread_id)
            .await?
            .expect("projection");
    assert_eq!(
        projection.thread.branch.map(|b| b.0),
        Some("t3/1234abcd".into())
    );
    assert!(
        projection
            .turn_items
            .iter()
            .any(|i| serde_json::to_value(i).expect("item")["title"] == "Branch not renamed")
    );
    assert_eq!(
        projection.runs[0].status,
        OrchestrationV2RunStatus::Starting
    );
    Ok(())
}

#[tokio::test]
async fn asynchronous_setup_releases_before_exit_and_ignores_nonzero_exit() -> Result<()> {
    for exit in [0, 1] {
        let h = LaunchHarness::new().await?;
        let (finish, completion) = tokio::sync::oneshot::channel();
        *h.environment.setup_completion.lock().expect("completion") = Some((true, completion));
        let mut events = h.h.sink.subscribe();
        let result = h
            .service
            .launch(h.input(
                "async",
                json!({"type":"worktree","baseRef":"main","branch":"explicit"}),
                true,
            ))
            .await?;
        h.milestone("prepared run released", async { loop {
            let event = events.recv().await.expect("event");
            if let t3_contracts::application_event::ApplicationStoredEvent::Orchestration(stored) =
                event
                && matches!(stored.event,OrchestrationV2DomainEvent::RunUpdated(e) if e.payload.status==OrchestrationV2RunStatus::Starting)
            {
                break;
            }
        }}).await?;
        finish.send(Ok(Some(exit))).expect("completion");
        h.service.drain().await;
        let projection =
            h.h.reader
                .projection(result.thread_id)
                .await?
                .expect("projection");
        assert_eq!(
            projection.runs[0].status,
            OrchestrationV2RunStatus::Starting
        );
        assert_eq!(h.workspaces.removes.load(Ordering::SeqCst), 0);
    }
    Ok(())
}

#[tokio::test]
async fn interrupted_preparation_closes_setup_and_cleans_only_owned_worktree() -> Result<()> {
    let h = LaunchHarness::new().await?;
    let (finish, completion) = tokio::sync::oneshot::channel();
    *h.environment.setup_completion.lock().expect("completion") = Some((false, completion));
    let result = h
        .service
        .launch(h.input(
            "cancel",
            json!({"type":"worktree","baseRef":"main","branch":"explicit"}),
            true,
        ))
        .await?;
    h.milestone("setup entered", async {
        h.environment
            .setup_entered
            .acquire()
            .await
            .expect("setup entered")
            .forget();
    })
    .await?;
    let mut run =
        h.h.reader
            .projection(result.thread_id.clone())
            .await?
            .expect("projection")
            .runs[0]
            .clone();
    run.status = OrchestrationV2RunStatus::Interrupted;
    h.h.seed("run.updated", run).await?;
    h.service.drain().await;
    drop(finish);
    let projected = h.h.reader.thread(result.thread_id).await?.expect("thread");
    assert!(projected.worktree_path.is_none());
    assert_eq!(h.workspaces.removes.load(Ordering::SeqCst), 1);
    assert_eq!(h.environment.closed.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn rejected_initial_message_keeps_reused_thread_title_and_does_not_prepare() -> Result<()> {
    let h = LaunchHarness::new().await?;
    h.commands.reject_message.store(true, Ordering::SeqCst);
    let mut input = h.input("reuse", json!({"type":"root"}), true);
    input.thread_id = Some(ThreadId("thread:test".into()));
    input.reuse_existing_thread = true;
    input.generate_title = true;
    assert!(h.service.launch(input).await.is_err());
    h.service.drain().await;
    assert_eq!(
        h.h.reader
            .thread(ThreadId("thread:test".into()))
            .await?
            .expect("thread")
            .title
            .0,
        "Source"
    );
    assert_eq!(h.workspaces.prepares.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn preparation_retry_reuses_recorded_renamed_worktree() -> Result<()> {
    let h = LaunchHarness::new().await?;
    let path = h.temp.path().join("existing");
    h.h.seed(
        "thread.created",
        crate::checkpoint_service_test::support::thread("thread:test", Some(&path)),
    )
    .await?;
    let mut run = crate::checkpoint_service_test::support::run(1, "preparing");
    run.workspace_preparation = Some(Some(serde_json::from_value(
        json!({"type":"worktree","baseRef":"main"}),
    )?));
    h.h.seed("run.created", &run).await?;
    h.service
        .prepare_run(PreparationInput {
            command_id: CommandId("retry".into()),
            thread_id: ThreadId("thread:test".into()),
            run_id: expanded_id(&run.id)?,
        })
        .await;
    h.service.drain().await;
    let thread =
        h.h.reader
            .thread(ThreadId("thread:test".into()))
            .await?
            .expect("thread");
    assert_eq!(thread.branch.map(|b| b.0), Some("feature/test".into()));
    assert_eq!(h.workspaces.removes.load(Ordering::SeqCst), 0);
    assert!(
        h.commands
            .calls
            .lock()
            .expect("calls")
            .iter()
            .all(|(c, _)| c["type"] != "thread.metadata.update")
    );
    Ok(())
}

#[tokio::test]
async fn cancellation_during_branch_naming_closes_the_already_started_setup_terminal() -> Result<()>
{
    let h = LaunchHarness::new().await?;
    let naming = blocked();
    *h.environment.naming_gate.lock().expect("naming") = Some(naming.clone());
    let (finish, completion) = tokio::sync::oneshot::channel();
    *h.environment.setup_completion.lock().expect("completion") = Some((false, completion));
    h.service
        .launch(h.input(
            "cancel-naming",
            json!({"type":"worktree","baseRef":"main"}),
            true,
        ))
        .await?;
    h.milestone("naming entered", async {
        naming
            .entered
            .acquire()
            .await
            .expect("naming entered")
            .forget();
    })
    .await?;
    h.milestone("setup entered", async {
        h.environment
            .setup_entered
            .acquire()
            .await
            .expect("setup entered")
            .forget();
    })
    .await?;
    h.service.shutdown().await;
    drop(finish);
    assert_eq!(h.environment.closed.load(Ordering::SeqCst), 1);
    assert_eq!(h.workspaces.removes.load(Ordering::SeqCst), 1);
    assert!(
        !h.commands
            .calls
            .lock()
            .expect("calls")
            .iter()
            .any(|(c, _)| c["type"] == "prepared-run.release")
    );
    Ok(())
}
#[tokio::test]
async fn temporary_branch_preflight_accepts_current_and_legacy_names() -> Result<()> {
    let h = LaunchHarness::new().await?;
    for (branch, existing, temporary) in [
        ("t3/1234abcd", "t3", true),
        ("t3code/1234abcd", "t3code", true),
        (
            "t3code/12345678-1234-4234-8234-123456789abc",
            "t3code",
            true,
        ),
        ("t3/1234ABCD", "t3", true),
        ("t3/too-long-to-be-temporary", "t3", false),
    ] {
        *h.environment.branches.lock().expect("branches") = vec![existing.into()];
        let strategy =
            serde_json::from_value(json!({"type":"worktree","baseRef":"main","branch":branch}))?;
        assert_eq!(
            h.service
                .check_worktree_base(ProjectId("project:test".into()), &strategy, None)
                .await
                .is_ok(),
            temporary,
            "{branch}"
        );
    }
    Ok(())
}
#[tokio::test]
async fn nonzero_synchronous_setup_persists_failed_run_and_retains_recorded_workspace() -> Result<()>
{
    let h = LaunchHarness::new().await?;
    let (finish, completion) = tokio::sync::oneshot::channel();
    *h.environment.setup_completion.lock().expect("completion") = Some((false, completion));
    let result = h
        .service
        .launch(h.input(
            "setup:failed",
            json!({"type":"worktree","baseRef":"main","branch":"explicit"}),
            true,
        ))
        .await?;
    finish.send(Ok(Some(1))).expect("completion");
    h.service.drain().await;
    let projection =
        h.h.reader
            .projection(result.thread_id)
            .await?
            .expect("projection");
    assert_eq!(projection.runs[0].status, OrchestrationV2RunStatus::Failed);
    assert!(projection.thread.worktree_path.is_some());
    assert_eq!(h.workspaces.removes.load(Ordering::SeqCst), 0);
    assert!(
        !h.commands
            .calls
            .lock()
            .expect("calls")
            .iter()
            .any(|(c, _)| c["type"] == "prepared-run.release")
    );
    Ok(())
}

#[derive(Default)]
struct Progress(std::sync::Mutex<Vec<StageUpdate>>);
#[async_trait::async_trait]
impl LaunchProgress for Progress {
    async fn update(&self, _: &ThreadId, update: StageUpdate) -> Result<()> {
        self.0.lock().expect("progress").push(update);
        Ok(())
    }
}
struct PartialCheckout(std::path::PathBuf);
#[async_trait::async_trait]
impl LaunchWorkspacePreparation for PartialCheckout {
    async fn prepare(
        &self,
        _: ProjectId,
        _: ThreadId,
        _: OrchestrationV2ThreadLaunchWorkspaceStrategy,
        observer: Arc<PreparationObserver>,
        _: CancellationToken,
    ) -> Result<Workspace> {
        observer.claimed(Workspace {
            cwd: self.0.clone(),
            worktree_path: Some(self.0.clone()),
            branch: Some("partial".into()),
        })?;
        observer
            .update(StageUpdate {
                stage: SetupStage::Submodules,
                status: StageStatus::Error,
                percent: Some(42.0),
                detail: Some("submodule checkout failed".into()),
            })
            .await?;
        Err(t3_orchestration::error::Error::Service {
            operation: "checkout",
            detail: "submodule checkout failed".into(),
        })
    }
}
#[tokio::test]
async fn failed_partial_checkout_reports_host_progress_and_cleans_the_claimed_path() -> Result<()> {
    let mut h = LaunchHarness::new().await?;
    let progress = Arc::new(Progress::default());
    let service = ThreadLaunchService::with_preparation(
        LaunchServices {
            database: h.h.db.stores.clone(),
            projections: h.h.reader.clone(),
            events: h.h.sink.clone(),
            commands: h.commands.clone(),
            workspaces: h.workspaces.clone(),
            environment: h.environment.clone(),
            ids: h.h.ids.clone(),
            clock: h.h.clock.clone(),
        },
        Arc::new(PartialCheckout(h.temp.path().join("partial"))),
        progress.clone(),
    );
    let result = service
        .launch(h.input(
            "partial",
            json!({"type":"worktree","baseRef":"main","branch":"explicit"}),
            true,
        ))
        .await?;
    h.service = service.clone();
    let early = h
        .milestone("setup entered", std::future::pending::<()>())
        .await
        .expect_err("preparation failed before milestone");
    assert!(early.to_string().contains("submodule checkout failed"));
    assert_eq!(h.workspaces.removes.load(Ordering::SeqCst), 1);
    let projection =
        h.h.reader
            .projection(result.thread_id)
            .await?
            .expect("thread");
    assert_eq!(projection.runs[0].status, OrchestrationV2RunStatus::Failed);
    assert!(projection.thread.worktree_path.is_none());
    let updates = progress.0.lock().expect("progress");
    assert_eq!(updates.len(), 2);
    assert_eq!(updates[1].stage, SetupStage::Submodules);
    assert_eq!(updates[1].status, StageStatus::Error);
    assert_eq!(updates[1].percent, Some(42.0));
    assert_eq!(
        updates[1].detail.as_deref(),
        Some("submodule checkout failed")
    );
    assert!(
        !h.commands
            .calls
            .lock()
            .expect("calls")
            .iter()
            .any(|(c, _)| c["type"] == "prepared-run.release")
    );
    Ok(())
}
#[tokio::test]
async fn shutdown_waits_for_foreground_launch_and_rejects_future_launches() -> Result<()> {
    let h = LaunchHarness::new().await?;
    let gate = blocked();
    *h.environment.preflight_gate.lock().expect("preflight") = Some(gate.clone());
    let input = h.input("shutdown", json!({"type":"root"}), false);
    let service = h.service.clone();
    let mut launch = tokio::spawn(async move { service.launch(input).await });
    tokio::select! {
        biased;
        permit=gate.entered.acquire()=>permit.expect("entered").forget(),
        result=&mut launch=>match result.expect("launch task") {Ok(_)=>panic!("launch ended before preflight"),Err(error)=>panic!("launch failed before preflight: {error}")},
    }
    let mut shutdown = Box::pin(h.service.shutdown());
    assert!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(shutdown.as_mut().poll(cx).is_pending()))
            .await
    );
    gate.release.add_permits(1);
    let _ = launch.await.expect("launch");
    shutdown.await;
    let before = h.commands.calls.lock().expect("calls").len();
    assert!(matches!(
        h.service
            .launch(h.input("after-shutdown", json!({"type":"root"}), false))
            .await,
        Err(t3_orchestration::error::Error::Cancelled)
    ));
    assert!(matches!(
        h.service
            .retry_preparation(PreparationInput {
                command_id: CommandId("closed:retry".into()),
                thread_id: ThreadId("closed:thread".into()),
                run_id: RunId("closed:run".into())
            })
            .await,
        Err(t3_orchestration::error::Error::Cancelled)
    ));
    assert_eq!(h.commands.calls.lock().expect("calls").len(), before);
    Ok(())
}

#[tokio::test]
async fn launch_preflight_overlaps_independent_checks_and_keeps_error_priority() -> Result<()> {
    let h = LaunchHarness::new().await?;
    let repo = blocked();
    let commit = blocked();
    let branches = blocked();
    *h.environment.repository_gate.lock().expect("repo") = Some(repo.clone());
    *h.environment.commit_gate.lock().expect("commit") = Some(commit.clone());
    *h.environment.branches_gate.lock().expect("branches") = Some(branches.clone());
    h.environment.repository.store(false, Ordering::SeqCst);
    *h.environment.branches.lock().expect("branches") = vec!["feature".into()];
    let strategy = serde_json::from_value(
        json!({"type":"worktree","baseRef":"missing","branch":"feature/child"}),
    )?;
    let service = h.service.clone();
    let mut check = tokio::spawn(async move {
        service
            .check_worktree_base(ProjectId("project:test".into()), &strategy, None)
            .await
    });
    tokio::select! {permit=repo.entered.acquire()=>permit.expect("repository entered").forget(), result=&mut check=>panic!("preflight ended before gate: {result:?}")};
    assert_eq!(
        commit.entered.available_permits(),
        1,
        "base checked alongside repository"
    );
    assert_eq!(
        branches.entered.available_permits(),
        1,
        "branch checked alongside repository"
    );
    repo.release.add_permits(1);
    commit.release.add_permits(2);
    branches.release.add_permits(1);
    assert!(matches!(
        check.await.expect("preflight"),
        Err(Error::Service {
            operation: "thread-launch.not-repository",
            ..
        })
    ));
    Ok(())
}

struct ReplyFailure {
    inner: Arc<dyn ProjectionReader>,
}
#[async_trait::async_trait]
impl ProjectionReader for ReplyFailure {
    async fn projection(&self, _: ThreadId) -> Result<Option<OrchestrationV2ThreadProjection>> {
        Err(Error::Unsupported("scripted reply projection failure"))
    }
    async fn records(
        &self,
        thread_id: ThreadId,
        fields: Vec<RecordKind>,
        filter: RecordFilter,
    ) -> Result<ProjectionRecords> {
        self.inner.records(thread_id, fields, filter).await
    }
    async fn snapshot(
        &self,
        thread_id: ThreadId,
        window: Option<SnapshotWindow>,
    ) -> Result<ProjectionSnapshot> {
        self.inner.snapshot(thread_id, window).await
    }
    async fn history(&self, query: HistoryQuery) -> Result<OrchestrationV2ThreadHistoryPage> {
        self.inner.history(query).await
    }
    async fn search(
        &self,
        query: OrchestrationV2SearchThreadInput,
    ) -> Result<OrchestrationV2SearchThreadResult> {
        self.inner.search(query).await
    }
    async fn thread_shells(&self, query: ShellQuery) -> Result<OrchestrationV2ThreadShellSnapshot> {
        self.inner.thread_shells(query).await
    }
    async fn checkpoint_capture(
        &self,
        thread_id: ThreadId,
        run_id: RunId,
        scope_id: CheckpointScopeId,
    ) -> Result<CheckpointCaptureContext> {
        self.inner
            .checkpoint_capture(thread_id, run_id, scope_id)
            .await
    }
    async fn checkpoint_rollback(
        &self,
        thread_id: ThreadId,
        provider_thread_id: ProviderThreadId,
        checkpoint_id: CheckpointId,
        scope_id: CheckpointScopeId,
    ) -> Result<CheckpointRollbackContext> {
        self.inner
            .checkpoint_rollback(thread_id, provider_thread_id, checkpoint_id, scope_id)
            .await
    }
    async fn runtime_request(
        &self,
        thread_id: ThreadId,
        request_id: RuntimeRequestId,
    ) -> Result<Option<OrchestrationV2RuntimeRequest>> {
        self.inner.runtime_request(thread_id, request_id).await
    }
    async fn turn_item(
        &self,
        thread_id: ThreadId,
        item_id: TurnItemId,
    ) -> Result<Option<OrchestrationV2TurnItem>> {
        self.inner.turn_item(thread_id, item_id).await
    }
    async fn plan(
        &self,
        thread_id: ThreadId,
        plan_id: PlanId,
    ) -> Result<Option<OrchestrationV2PlanArtifact>> {
        self.inner.plan(thread_id, plan_id).await
    }
    async fn held_queued_runs(&self, thread_id: ThreadId) -> Result<Vec<OrchestrationV2Run>> {
        self.inner.held_queued_runs(thread_id).await
    }
    async fn app_owned_child_threads(&self, thread_id: ThreadId) -> Result<Vec<ThreadId>> {
        self.inner.app_owned_child_threads(thread_id).await
    }
    async fn pending_delegated_completion(&self, child_thread_id: ThreadId) -> Result<bool> {
        self.inner
            .pending_delegated_completion(child_thread_id)
            .await
    }
    async fn turn_start_history(
        &self,
        thread_id: ThreadId,
        run_ids: Option<Vec<RunId>>,
    ) -> Result<Vec<OrchestrationV2TurnItem>> {
        self.inner.turn_start_history(thread_id, run_ids).await
    }
    async fn unpaired_interrupt(
        &self,
        thread_id: ThreadId,
        request_id: TurnItemId,
        result_id: TurnItemId,
    ) -> Result<bool> {
        self.inner
            .unpaired_interrupt(thread_id, request_id, result_id)
            .await
    }
    async fn attachment_ids(&self, thread_id: ThreadId) -> Result<Vec<String>> {
        self.inner.attachment_ids(thread_id).await
    }
    async fn transcript_byte_floor(&self, thread_id: ThreadId, limit: u64) -> Result<u64> {
        self.inner.transcript_byte_floor(thread_id, limit).await
    }
    async fn thread(&self, thread_id: ThreadId) -> Result<Option<OrchestrationV2AppThread>> {
        self.inner.thread(thread_id).await
    }
    async fn shell(&self, thread_id: ThreadId) -> Result<Option<OrchestrationV2ThreadShell>> {
        self.inner.shell(thread_id).await
    }
    async fn shell_snapshot(
        &self,
        project_ids: Option<Vec<ProjectId>>,
        root_thread_id: Option<ThreadId>,
    ) -> Result<OrchestrationV2ShellSnapshot> {
        self.inner.shell_snapshot(project_ids, root_thread_id).await
    }
    async fn control(&self, query: ControlQuery) -> Result<Option<ControlContext>> {
        self.inner.control(query).await
    }
    async fn timeline(&self, query: TimelineQuery) -> Result<TimelinePage> {
        self.inner.timeline(query).await
    }
    async fn run(&self, thread_id: ThreadId, run_id: RunId) -> Result<Option<OrchestrationV2Run>> {
        self.inner.run(thread_id, run_id).await
    }
    async fn run_message(
        &self,
        thread_id: ThreadId,
        run_id: RunId,
    ) -> Result<Option<OrchestrationV2ConversationMessage>> {
        self.inner.run_message(thread_id, run_id).await
    }
    async fn can_start_queued_run(&self, thread_id: ThreadId, run_id: RunId) -> Result<bool> {
        self.inner.can_start_queued_run(thread_id, run_id).await
    }
    async fn recovery_thread_ids(&self) -> Result<Vec<ThreadId>> {
        self.inner.recovery_thread_ids().await
    }
    async fn unreadable_thread_ids(&self) -> Result<Vec<ThreadId>> {
        self.inner.unreadable_thread_ids().await
    }
    async fn next_turn_item_ordinal(
        &self,
        thread_id: ThreadId,
        run_id: Option<RunId>,
    ) -> Result<u64> {
        self.inner.next_turn_item_ordinal(thread_id, run_id).await
    }
}
#[tokio::test]
async fn preparation_starts_even_when_the_full_reply_projection_fails() -> Result<()> {
    let h = LaunchHarness::new().await?;
    let service = ThreadLaunchService::new(LaunchServices {
        database: h.h.db.stores.clone(),
        projections: Arc::new(ReplyFailure {
            inner: h.h.reader.clone(),
        }),
        events: h.h.sink.clone(),
        commands: h.commands.clone(),
        workspaces: h.workspaces.clone(),
        environment: h.environment.clone(),
        ids: h.h.ids.clone(),
        clock: h.h.clock.clone(),
    });
    let result = service
        .launch(h.input("reply-failure", json!({"type":"root"}), true))
        .await;
    assert!(matches!(
        result,
        Err(Error::Unsupported("scripted reply projection failure"))
    ));
    service.drain().await;
    assert_eq!(
        h.workspaces.prepares.load(Ordering::SeqCst),
        1,
        "preparation was scheduled before reading the full reply"
    );
    Ok(())
}
