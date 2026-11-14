#![allow(clippy::unwrap_used)]
mod support;
use async_trait::async_trait;
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use support::*;
use t3_contracts::{
    agent_sessions::*,
    base_schemas::{ProjectId, ThreadId},
    provider_instance::ProviderInstanceId,
    provider_policy::RuntimeMode,
};
use t3_orchestration::{
    api::ProjectionReader, ports::decisions::thread_command_executor::KeyedLanes,
    stores::StoreDatabase,
};
use t3_project::{sessions::*, transcript::parse_agent_session_transcript, *};

struct Inventory {
    file: std::path::PathBuf,
    root: String,
    source: AgentSessionImportSource,
    worktree: Option<SessionWorktree>,
}
#[async_trait]
impl SessionInventory for Inventory {
    async fn candidates(&self) -> Result<SessionCandidates> {
        Ok(SessionCandidates {
            candidates: vec![SessionCandidate {
                workspace_root: self.root.clone(),
                source: AgentSessionSource::Codex,
                thread_count: 1,
                last_active_at: Some(t3_contracts::wire::WireDateTime(
                    "2026-01-01T00:00:00.000Z".into(),
                )),
                git: None,
            }],
            truncated: false,
        })
    }
    async fn recent_threads(
        &self,
        _: &str,
        completed: &[AgentSessionImportSource],
    ) -> Result<RecentThreads> {
        Ok(Box::pin(futures_util::stream::iter(
            self.recent(completed)?.into_iter().map(Ok),
        )))
    }
}
impl Inventory {
    fn recent(&self, completed: &[AgentSessionImportSource]) -> Result<Vec<RecentSession>> {
        if completed
            .iter()
            .any(|s| same_transcript_identity(s, &self.source))
        {
            return Ok(vec![RecentSession::AlreadyImported(self.source.clone())]);
        }
        let content = std::fs::read_to_string(&self.file).unwrap();
        let thread = parse_agent_session_transcript(
            AgentSessionSource::Codex,
            ProviderInstanceId("codex".into()),
            "filename",
            "2026-01-01T00:00:00.000Z",
            &content,
        )
        .unwrap();
        Ok(vec![RecentSession::Importable {
            thread: Box::new(thread),
            source: self.source.clone(),
            worktree: self.worktree.clone(),
        }])
    }
}
struct RuntimeStore {
    database: StoreDatabase,
    fail_once: AtomicBool,
}
#[async_trait]
impl SessionImportStore for RuntimeStore {
    async fn completed_sources(&self, root: &str) -> Result<Vec<AgentSessionImportSource>> {
        let root = root.to_owned();
        Ok(self
            .database
            .read(move |db| {
                let mut sources = vec![];
                let mut statement =
                    db.prepare("SELECT runtime_payload_json FROM provider_session_runtime")?;
                for value in statement.query_map([], |r| r.get::<_, Option<String>>(0))? {
                    let value: serde_json::Value =
                        serde_json::from_str(&value?.unwrap_or("null".into()))?;
                    if value["cwd"] == root
                        && let Some(v) = value.get("importedTranscripts")
                    {
                        sources.extend(serde_json::from_value::<Vec<AgentSessionImportSource>>(
                            v.clone(),
                        )?);
                    }
                }
                Ok(sources)
            })
            .await?)
    }
    async fn recorded_sources(&self, id: &ThreadId) -> Result<Vec<AgentSessionImportSource>> {
        let id = id.clone();
        Ok(self.database.read(move |db| {
            use t3_persistence::rusqlite::OptionalExtension;
            let payload:Option<Option<String>>=db.query_row("SELECT runtime_payload_json FROM provider_session_runtime WHERE thread_id=?1",[id.0],|r|r.get(0)).optional()?;
            let value:serde_json::Value=serde_json::from_str(&payload.flatten().unwrap_or("null".into()))?;
            Ok(value.get("importedTranscripts").map(|v|serde_json::from_value(v.clone())).transpose()?.unwrap_or_default())
        }).await?)
    }
    async fn persist_runtime(
        &self,
        id: &ThreadId,
        source: &AgentSessionImportSource,
        thread: &t3_project::transcript::SessionThread,
        root: &str,
        _: RuntimeMode,
    ) -> Result<()> {
        if self.fail_once.swap(false, Ordering::AcqRel) {
            return Err(Error::Unavailable("fixture runtime failure"));
        }
        let id = id.clone();
        let source = source.clone();
        let at = thread.updated_at.clone();
        let root = root.to_owned();
        self.database.transaction(move |db| {
            db.execute("INSERT INTO provider_session_runtime(thread_id,provider_name,provider_instance_id,adapter_key,runtime_mode,status,last_seen_at,resume_cursor_json,runtime_payload_json) VALUES(?1,'codex',?2,'codex','full-access','stopped',?3,?4,?5) ON CONFLICT(thread_id) DO NOTHING",
                t3_persistence::rusqlite::params![id.0,source.provider_instance_id.0,at,json!({"threadId":source.provider_session_id}).to_string(),json!({"cwd":root}).to_string()])?;
            Ok(())
        }).await?;
        Ok(())
    }
    async fn record_imported_transcript(
        &self,
        id: &ThreadId,
        source: &AgentSessionImportSource,
    ) -> Result<()> {
        let id = id.clone();
        let source = source.clone();
        self.database.transaction(move |db| {
            let payload:String=db.query_row("SELECT runtime_payload_json FROM provider_session_runtime WHERE thread_id=?1",[&id.0],|r|r.get(0))?;
            let mut payload:serde_json::Value=serde_json::from_str(&payload)?;
            let mut sources:Vec<AgentSessionImportSource>=payload.get("importedTranscripts").map(|v|serde_json::from_value(v.clone())).transpose()?.unwrap_or_default();
            if !sources.iter().any(|s|same_transcript_identity(s,&source)){sources.push(source);}
            payload["importedTranscripts"]=serde_json::to_value(sources)?;
            db.execute("UPDATE provider_session_runtime SET runtime_payload_json=?1 WHERE thread_id=?2",t3_persistence::rusqlite::params![payload.to_string(),id.0])?;Ok(())
        }).await?;
        Ok(())
    }
    async fn policy_runtime_mode(&self, _: &ProjectId) -> Result<Option<RuntimeMode>> {
        Ok(None)
    }
}
async fn importer(f: &Fixture, fail_once: bool) -> (AgentSessionImporter, AgentSessionImportInput) {
    importer_at(f, fail_once, None).await
}
async fn importer_at(
    f: &Fixture,
    fail_once: bool,
    worktree: Option<SessionWorktree>,
) -> (AgentSessionImporter, AgentSessionImportInput) {
    let root = f.root("project");
    f.service
        .create(f.create("p", "create", &root))
        .await
        .unwrap();
    let provider_home = f.root("fixture-provider-home");
    let sessions = provider_home.join(".codex/sessions/2026/01/01");
    std::fs::create_dir_all(&sessions).unwrap();
    let file = sessions.join("rollout-2026-01-01T00-00-00-session.jsonl");
    let contents=[json!({"type":"session_meta","payload":{"id":"session","cwd":root}}),json!({"type":"event_msg","payload":{"type":"user_message","message":"A fixture request"}}),json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Fixture answer"}]}})].iter().map(|v|v.to_string()).collect::<Vec<_>>().join("\n");
    std::fs::write(&file, &contents).unwrap();
    let source=serde_json::from_value(json!({"provider":"codex","providerInstanceId":"codex","providerSessionId":"session","filePath":file,"size":contents.len(),"mtimeMs":1000,"device":1,"inode":"1","birthtimeMs":0})).unwrap();
    let scanner = AgentSessionScanner {
        inventory: Arc::new(Inventory {
            file,
            root: root.to_string_lossy().into(),
            source,
            worktree,
        }),
        projects: f.service.clone(),
        clock: f.clock.clone(),
        home: provider_home,
        temporary_root: f.root("tmp"),
        base_dir: f.root("base"),
        worktrees_dir: f.root("worktrees"),
    };
    let service = AgentSessionImporter {
        scanner,
        projects: f.service.clone(),
        projections: f.projections.clone(),
        sink: Arc::new(f.sink.clone()),
        runtimes: Arc::new(RuntimeStore {
            database: f.database.clone(),
            fail_once: AtomicBool::new(fail_once),
        }),
        lanes: Arc::new(KeyedLanes::default()),
    };
    let input =
        serde_json::from_value(json!({"projectId":"p","expectedWorkspaceRoot":root})).unwrap();
    (service, input)
}

#[tokio::test]
async fn imported_linked_worktree_keeps_its_path_branch_and_runtime_cwd() {
    let f = Fixture::new();
    let linked = f.temp.path().join("linked");
    let (service, input) = importer_at(
        &f,
        false,
        Some(SessionWorktree {
            path: linked.to_string_lossy().into(),
            branch: Some("feature".into()),
        }),
    )
    .await;
    let project = f
        .service
        .get_by_id(&input.project_id, false)
        .await
        .unwrap()
        .unwrap();
    let git = t3_git::Git::new(&project.workspace_root.0)
        .with_environment(git_environment(f.temp.path()));
    git.execute(&["init"], Default::default()).await.unwrap();
    std::fs::write(
        std::path::Path::new(&project.workspace_root.0).join("file"),
        "fixture",
    )
    .unwrap();
    git.execute(&["add", "file"], Default::default())
        .await
        .unwrap();
    git.execute(&["commit", "-m", "fixture"], Default::default())
        .await
        .unwrap();
    git.execute(
        &[
            "worktree",
            "add",
            "-b",
            "feature",
            &linked.to_string_lossy(),
        ],
        Default::default(),
    )
    .await
    .unwrap();
    assert_eq!(service.import(input).await.unwrap().imported_count, 1);
    let thread = f
        .projections
        .projection(ThreadId("import:codex:session".into()))
        .await
        .unwrap()
        .unwrap();
    let value = serde_json::to_value(thread.thread).unwrap();
    assert_eq!(value["worktreePath"], linked.to_string_lossy().as_ref());
    assert_eq!(value["branch"], "feature");
    let cwd = f.database.read(|db| {
        let raw: String = db.query_row("SELECT runtime_payload_json FROM provider_session_runtime WHERE thread_id='import:codex:session'", [], |r| r.get(0))?;
        Ok(serde_json::from_str::<serde_json::Value>(&raw)?["cwd"].clone())
    }).await.unwrap();
    assert_eq!(cwd, linked.to_string_lossy().as_ref());
    f.shutdown().await;
}
#[tokio::test]
async fn imports_fixture_history_to_real_projections_and_retries_without_duplication() {
    let f = Fixture::new();
    let (service, input) = importer(&f, false).await;
    let result = service.import(input.clone()).await.unwrap();
    assert_eq!((result.imported_count, result.skipped_count), (1, 0));
    let thread = f
        .projections
        .projection(ThreadId("import:codex:session".into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(thread.messages.len(), 2);
    assert_eq!(thread.turn_items.len(), 2);
    assert!(thread.runs.is_empty());
    assert_eq!(thread.provider_threads.len(), 1);
    assert_eq!(thread.messages[0].text, "A fixture request");
    assert_eq!(thread.messages[1].text, "Fixture answer");
    assert_eq!(service.import(input).await.unwrap().imported_count, 1);
    let count = f
        .database
        .read(|db| {
            Ok(db.query_row(
                "SELECT COUNT(*) FROM orchestration_events WHERE aggregate_kind='thread'",
                [],
                |r| r.get::<_, i64>(0),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(count, 6);
    f.shutdown().await;
}
#[tokio::test]
async fn repairs_runtime_after_interrupted_import_and_rejects_changed_project_root() {
    let f = Fixture::new();
    let (service, input) = importer(&f, true).await;
    let first = service.import(input.clone()).await.unwrap();
    assert_eq!((first.imported_count, first.skipped_count), (0, 1));
    assert!(
        f.projections
            .thread(ThreadId("import:codex:session".into()))
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(
        service.import(input.clone()).await.unwrap().imported_count,
        1
    );
    let mut changed = input;
    changed.expected_workspace_root =
        Some(Some(f.root("other").to_string_lossy().into_owned().into()));
    assert!(service.import(changed).await.is_err());
    assert_eq!(
        service
            .runtimes
            .recorded_sources(&ThreadId("import:codex:session".into()))
            .await
            .unwrap()
            .len(),
        1
    );
    f.shutdown().await;
}
#[tokio::test]
async fn scan_merges_known_project_and_marks_it_imported() {
    let f = Fixture::new();
    let (service, _) = importer(&f, false).await;
    let result = service.scanner.scan().await.unwrap();
    assert_eq!(result.candidates.len(), 1);
    assert_eq!(
        result.candidates[0].project_id,
        Some(Some(ProjectId("p".into())))
    );
    assert!(result.candidates[0].already_imported);
    assert_eq!(result.candidates[0].thread_count, 1);
    f.shutdown().await;
}

/// Imports run in full access unless a policy ceiling exists; then in the project's default mode,
/// lowered to the ceiling when it sits above it (`AgentSessionImporter.ts`).
#[tokio::test]
async fn imports_run_in_the_projects_default_mode_under_a_policy_ceiling() {
    let fixture = Fixture::new();
    let dir = tempfile::tempdir().unwrap();
    let store = |name: &str, mode: &str, ceiling| {
        let path = dir.path().join(format!("{name}.json"));
        std::fs::write(&path, json!({"defaultRuntimeMode": mode}).to_string()).unwrap();
        t3_project::import_store::RuntimeImportStore::new(
            fixture.database.clone(),
            t3_config::settings::SettingsService::new(path),
            ceiling,
        )
    };
    let project = ProjectId("project:ceiling".into());
    let mode = |store: t3_project::import_store::RuntimeImportStore| {
        let project = project.clone();
        async move { store.policy_runtime_mode(&project).await.unwrap() }
    };
    assert_eq!(mode(store("none", "approval-required", None)).await, None);
    assert_eq!(
        mode(store(
            "above",
            "full-access",
            Some(RuntimeMode::ApprovalRequired)
        ))
        .await,
        Some(RuntimeMode::ApprovalRequired)
    );
    assert_eq!(
        mode(store("below", "approval-required", Some(RuntimeMode::Auto))).await,
        Some(RuntimeMode::ApprovalRequired)
    );
}
