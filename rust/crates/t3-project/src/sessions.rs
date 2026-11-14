use crate::{Error, ProjectService, Result, transcript::SessionThread};
use async_trait::async_trait;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::Arc,
};
use t3_contracts::{
    agent_sessions::*,
    base_schemas::{ProjectId, ThreadId},
    provider_policy::RuntimeMode,
};
use t3_orchestration::{
    api::{
        Clock, CommitGuard, CommitOptions, CommitRequest, Decision, EventCommitter,
        ProjectionReader,
    },
    ports::decisions::{orchestrator::derived_id, thread_command_executor::KeyedLanes},
};

pub struct SessionCandidate {
    pub workspace_root: String,
    pub source: AgentSessionSource,
    pub thread_count: u64,
    pub last_active_at: Option<t3_contracts::base_schemas::IsoDateTime>,
    pub git: Option<AgentSessionProjectGit>,
}
pub struct SessionCandidates {
    pub candidates: Vec<SessionCandidate>,
    pub truncated: bool,
}
pub enum RecentSession {
    Importable {
        thread: Box<SessionThread>,
        source: AgentSessionImportSource,
        worktree: Option<SessionWorktree>,
    },
    AlreadyImported(AgentSessionImportSource),
    Duplicate(AgentSessionImportSource),
    Skipped,
}
#[derive(Debug, Clone)]
pub struct SessionWorktree {
    pub path: String,
    pub branch: Option<String>,
}
/// Native discovery/streaming adapters must come from workspace/provider composition.
/// Implementations enforce scanner byte/operation budgets, regular-file reads and identity checks.
#[async_trait]
pub trait SessionInventory: Send + Sync {
    async fn candidates(&self) -> Result<SessionCandidates>;
    async fn recent_threads(
        &self,
        root: &str,
        completed: &[AgentSessionImportSource],
    ) -> Result<Vec<RecentSession>>;
}
#[derive(Clone)]
pub struct AgentSessionScanner {
    pub inventory: Arc<dyn SessionInventory>,
    pub projects: ProjectService,
    pub clock: Arc<dyn Clock>,
    pub home: PathBuf,
    pub temporary_root: PathBuf,
    pub base_dir: PathBuf,
    pub worktrees_dir: PathBuf,
}
fn path_key(path: &str) -> String {
    let value = path.replace('\\', "/");
    if cfg!(windows) {
        value.to_lowercase()
    } else {
        value
    }
}
impl AgentSessionScanner {
    fn excluded(&self, path: &std::path::Path) -> bool {
        let path = path_key(&path.to_string_lossy());
        [&self.home, &self.temporary_root]
            .iter()
            .any(|p| path == path_key(&p.to_string_lossy()))
            || ["/tmp", "/private/tmp"].contains(&path.as_str())
            || [
                self.home.join("Downloads"),
                self.home.join("Documents/Codex"),
                self.base_dir.clone(),
                self.worktrees_dir.clone(),
            ]
            .iter()
            .any(|p| {
                let p = path_key(&p.to_string_lossy());
                path == p || path.starts_with(&format!("{}/", p.trim_end_matches('/')))
            })
            || path.contains("/.t3/worktrees/")
    }
    pub async fn scan(&self) -> Result<AgentSessionScanResult> {
        let collected = self.inventory.candidates().await?;
        let mut merged: BTreeMap<String, AgentSessionProjectCandidate> = BTreeMap::new();
        let existing = self.projects.snapshot().await?.projects;
        let mut projects = BTreeMap::new();
        for p in existing {
            let key = t3_workspace::paths::canonicalize(&p.workspace_root.0)
                .map(|p| path_key(&p.to_string_lossy()))
                .unwrap_or_else(|_| path_key(&p.workspace_root.0));
            projects.insert(key, p);
        }
        for c in collected.candidates {
            let expanded = t3_workspace::paths::expand_home(c.workspace_root.trim(), &self.home);
            if !expanded.is_absolute() || self.excluded(&expanded) {
                continue;
            }
            let Ok(real) = t3_workspace::paths::canonicalize(&expanded) else {
                continue;
            };
            if self.excluded(&real)
                || t3_workspace::paths::normalize_root(&real.to_string_lossy(), &self.home, false)
                    .await
                    .is_err()
            {
                continue;
            }
            let key = path_key(&real.to_string_lossy());
            let imported = projects.get(&key);
            let path = imported
                .map(|p| p.workspace_root.0.clone())
                .unwrap_or(c.workspace_root);
            let entry = merged
                .entry(key)
                .or_insert_with(|| AgentSessionProjectCandidate {
                    path: path.clone().into(),
                    title: std::path::Path::new(&path)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.clone())
                        .into(),
                    project_id: imported.map(|p| Some(p.id.clone())),
                    sources: vec![],
                    thread_count: 0,
                    last_active_at: None,
                    already_imported: imported.is_some(),
                    git: Some(c.git.clone()),
                });
            if !entry.sources.contains(&c.source) {
                entry.sources.push(c.source);
            }
            entry.thread_count = entry.thread_count.saturating_add(c.thread_count);
            if c.last_active_at.as_ref().map(|d| &d.0) > entry.last_active_at.as_ref().map(|d| &d.0)
            {
                entry.last_active_at = c.last_active_at;
            }
        }
        let mut candidates: Vec<_> = merged.into_values().collect();
        candidates.sort_by(|a, b| {
            b.last_active_at
                .as_ref()
                .map(|d| &d.0)
                .cmp(&a.last_active_at.as_ref().map(|d| &d.0))
                .then_with(|| a.path.0.cmp(&b.path.0))
        });
        Ok(AgentSessionScanResult {
            candidates,
            scanned_at: self.clock.now(),
            truncated: collected.truncated.then_some(Some(true)),
        })
    }
}

#[async_trait]
pub trait SessionImportStore: Send + Sync {
    async fn completed_sources(
        &self,
        workspace_root: &str,
    ) -> Result<Vec<AgentSessionImportSource>>;
    async fn recorded_sources(&self, thread: &ThreadId) -> Result<Vec<AgentSessionImportSource>>;
    async fn persist_runtime(
        &self,
        thread: &ThreadId,
        source: &AgentSessionImportSource,
        session: &SessionThread,
        workspace_root: &str,
        mode: RuntimeMode,
    ) -> Result<()>;
    async fn record_imported_transcript(
        &self,
        thread: &ThreadId,
        source: &AgentSessionImportSource,
    ) -> Result<()>;
    /// None means no policy ceiling, and therefore the TypeScript default full-access mode.
    async fn policy_runtime_mode(&self, project: &ProjectId) -> Result<Option<RuntimeMode>>;
}
pub struct AgentSessionImporter {
    pub scanner: AgentSessionScanner,
    pub projects: ProjectService,
    pub projections: Arc<dyn ProjectionReader>,
    pub sink: Arc<dyn EventCommitter>,
    pub runtimes: Arc<dyn SessionImportStore>,
    pub lanes: Arc<KeyedLanes>,
}
struct ImportTarget<'a> {
    project: &'a ProjectId,
    root: &'a str,
    worktree: Option<&'a SessionWorktree>,
}
pub fn same_transcript_identity(
    a: &AgentSessionImportSource,
    b: &AgentSessionImportSource,
) -> bool {
    fn inode(v: &Option<AgentSessionInode>) -> Option<Option<String>> {
        match v {
            None => Some(None),
            Some(AgentSessionInode::String(s)) => Some(Some(s.clone())),
            Some(AgentSessionInode::SpeechNumber(n)) => serde_json::to_value(n)
                .ok()
                .and_then(|v| v.as_f64())
                .filter(|n| n.is_finite() && n.fract() == 0. && n.abs() <= 9_007_199_254_740_991.)
                .map(|n| Some(format!("{n:.0}"))),
        }
    }
    let ia = inode(&a.inode);
    let ib = inode(&b.inode);
    ia.is_some()
        && ia == ib
        && a.file_path == b.file_path
        && a.size == b.size
        && a.mtime_ms == b.mtime_ms
        && a.device == b.device
        && a.birthtime_ms == b.birthtime_ms
}
pub fn is_native_history_pending(history_origin: Option<&str>, has_run: bool) -> bool {
    history_origin != Some("v1_import") && !has_run
}
impl AgentSessionImporter {
    pub async fn import(&self, input: AgentSessionImportInput) -> Result<AgentSessionImportResult> {
        let project = self
            .projects
            .get_by_id(&input.project_id, false)
            .await?
            .ok_or_else(|| Error::NotFound(input.project_id.0.clone()))?;
        if let Some(Some(expected)) = input.expected_workspace_root
            && path_key(&expected.0) != path_key(&project.workspace_root.0)
        {
            return Err(Error::Invalid(
                "The project's workspace root changed.".into(),
            ));
        }
        let mode = self
            .runtimes
            .policy_runtime_mode(&input.project_id)
            .await?
            .unwrap_or(RuntimeMode::FullAccess);
        let completed = self
            .runtimes
            .completed_sources(&project.workspace_root.0)
            .await?;
        let outcomes = self
            .scanner
            .inventory
            .recent_threads(&project.workspace_root.0, &completed)
            .await?;
        let mut ids = BTreeSet::new();
        let mut result = AgentSessionImportResult {
            imported_count: 0,
            skipped_count: 0,
        };
        for outcome in outcomes {
            let (source, thread, worktree) = match outcome {
                RecentSession::Skipped => {
                    result.skipped_count += 1;
                    continue;
                }
                RecentSession::AlreadyImported(source) => {
                    ids.insert(format!(
                        "import:{}:{}",
                        source.provider_instance_id.0, source.provider_session_id.0
                    ));
                    result.imported_count += 1;
                    continue;
                }
                RecentSession::Duplicate(source) => {
                    let id = format!(
                        "import:{}:{}",
                        source.provider_instance_id.0, source.provider_session_id.0
                    );
                    if ids.contains(&id) {
                        let _ = self
                            .runtimes
                            .record_imported_transcript(&ThreadId(id), &source)
                            .await;
                    }
                    continue;
                }
                RecentSession::Importable {
                    thread,
                    source,
                    worktree,
                } => (source, thread, worktree),
            };
            let id = ThreadId(format!(
                "import:{}:{}",
                source.provider_instance_id.0, source.provider_session_id.0
            ));
            let _guard = self.lanes.acquire([id.0.clone()]).await;
            match self
                .import_one(
                    &id,
                    ImportTarget {
                        project: &input.project_id,
                        root: &project.workspace_root.0,
                        worktree: worktree.as_ref(),
                    },
                    &source,
                    &thread,
                    mode,
                )
                .await
            {
                Ok(Some(true)) => {
                    ids.insert(id.0);
                    result.imported_count += 1;
                }
                Ok(None) => {}
                _ => result.skipped_count += 1,
            }
        }
        Ok(result)
    }
    async fn import_one(
        &self,
        id: &ThreadId,
        target: ImportTarget<'_>,
        source: &AgentSessionImportSource,
        thread: &SessionThread,
        mode: RuntimeMode,
    ) -> Result<Option<bool>> {
        if thread.source == AgentSessionSource::ClaudeAgent {
            let parsed = uuid::Uuid::parse_str(&thread.provider_session_id).ok();
            if !parsed.is_some_and(|id| {
                matches!(id.get_version_num(), 1..=8) && id.get_variant() == uuid::Variant::RFC4122
            }) {
                return Err(Error::Invalid("Session cannot be resumed.".into()));
            }
        }
        if let Some(existing) = self.projections.thread(id.clone()).await? {
            use t3_orchestration::ports::checkpoints_forks::checkpoint_restore_safety::expanded_id;
            if expanded_id(&existing.project_id)? != *target.project
                || existing.history_origin.as_ref().is_none_or(|h| {
                    serde_json::to_value(h).ok() != Some(serde_json::json!("v1_import"))
                })
            {
                return Err(Error::Invalid(
                    "Imported thread belongs to another project or has non-imported activity."
                        .into(),
                ));
            }
            let recorded = self.runtimes.recorded_sources(id).await?;
            if !recorded.is_empty() && !recorded.iter().any(|r| same_transcript_identity(r, source))
            {
                return Ok(Some(false));
            }
        } else {
            let (events, provider_thread) =
                import_events(id, target.project, source, thread, mode, target.worktree)?;
            let result = self
                .sink
                .commit_with_options(
                    CommitRequest {
                        receipt: None,
                        decision: Decision {
                            events,
                            ..Default::default()
                        },
                        guard: CommitGuard::None,
                    },
                    CommitOptions {
                        unless_native_thread_owned: Some(provider_thread),
                        ..Default::default()
                    },
                )
                .await?;
            if !result.committed {
                return Ok(None);
            }
        }
        self.runtimes
            .persist_runtime(
                id,
                source,
                thread,
                target
                    .worktree
                    .map_or(target.root, |worktree| worktree.path.as_str()),
                mode,
            )
            .await?;
        self.runtimes.record_imported_transcript(id, source).await?;
        Ok(Some(true))
    }
}
fn import_events(
    id: &ThreadId,
    project: &ProjectId,
    source: &AgentSessionImportSource,
    thread: &SessionThread,
    mode: RuntimeMode,
    worktree: Option<&SessionWorktree>,
) -> Result<(
    Vec<t3_contracts::orchestration_v2::OrchestrationV2DomainEvent>,
    t3_contracts::orchestration_v2::OrchestrationV2ProviderThread,
)> {
    use serde_json::json;
    let driver = match thread.source {
        AgentSessionSource::ClaudeAgent => "claudeAgent",
        AgentSessionSource::Codex => "codex",
        AgentSessionSource::Pi => "pi",
    };
    let native = if thread.source == AgentSessionSource::Pi {
        &source.file_path.0
    } else {
        &thread.provider_session_id
    };
    let provider_id = derived_id(
        "provider-thread",
        &[
            "provider",
            driver,
            "provider-instance",
            &source.provider_instance_id.0,
            "native-thread",
            native,
        ],
    );
    let model = thread.model.as_deref().unwrap_or_else(|| {
        t3_contracts::model::DEFAULT_MODEL_BY_PROVIDER
            .iter()
            .find(|(k, _)| *k == driver)
            .map(|(_, m)| *m)
            .unwrap_or(t3_contracts::model::DEFAULT_MODEL)
    });
    let app = json!({"createdBy":"system","creationSource":"server","id":id,"projectId":project,"title":if thread.title.trim().is_empty(){"Untitled thread"}else{&thread.title},"providerInstanceId":source.provider_instance_id,"modelSelection":{"instanceId":source.provider_instance_id,"model":model},"runtimeMode":mode,"interactionMode":"default","branch":worktree.and_then(|worktree|worktree.branch.as_deref()),"worktreePath":worktree.map(|worktree|worktree.path.as_str()),"linkedPullRequest":null,"branchPullRequest":null,"activeProviderThreadId":provider_id,"historyOrigin":"v1_import","lineage":{"parentThreadId":null,"relationshipToParent":null,"rootThreadId":id},"forkedFrom":null,"createdAt":thread.created_at,"updatedAt":thread.updated_at,"archivedAt":null,"settledOverride":"settled","settledAt":thread.updated_at,"unsettledAt":null,"snoozedUntil":null,"snoozedAt":null,"pinnedAt":null,"pinOrderKey":null,"activeOrderKey":null,"lastVisitedAt":null,"deletedAt":null});
    let provider = json!({"id":provider_id,"driver":driver,"providerInstanceId":source.provider_instance_id,"providerSessionId":null,"appThreadId":id,"ownerNodeId":null,"nativeThreadRef":{"driver":driver,"nativeId":native,"strength":"strong"},"nativeConversationHeadRef":null,"status":"idle","firstRunOrdinal":null,"lastRunOrdinal":null,"handoffIds":[],"forkedFrom":null,"pendingBackgroundTasks":[],"createdAt":thread.created_at,"updatedAt":thread.updated_at});
    let mut events = vec![
        json!({"id":format!("agent-session-import:v2:thread:{}:created",id.0),"type":"thread.created","threadId":id,"providerInstanceId":source.provider_instance_id,"occurredAt":thread.created_at,"payload":app}),
    ];
    if thread.omitted_message_count > 0 {
        let item_id = format!("agent-session-import:v2:history-truncated:{}", id.0);
        let payload = json!({"id":item_id,"threadId":id,"runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":0,"status":"completed","title":null,"startedAt":thread.created_at,"completedAt":thread.created_at,"updatedAt":thread.created_at,"type":"system_notice","message":format!("{} earlier {} not imported. This thread keeps the first user message and the most recent history. The original session transcript is unchanged.",thread.omitted_message_count,if thread.omitted_message_count==1{"message was"}else{"messages were"})});
        events.push(json!({"id":item_id,"type":"turn-item.updated","threadId":id,"occurredAt":thread.created_at,"payload":payload}));
    }
    for (index, m) in thread.messages.iter().enumerate() {
        let suffix = format!("{index:06}");
        let message_id = format!("{}:{suffix}", id.0);
        let item_id = format!("agent-session-import:v2:turn-item:{}:{suffix}", id.0);
        let mut item = json!({"id":item_id,"threadId":id,"runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":index+1,"status":"completed","title":null,"startedAt":m.created_at,"completedAt":m.created_at,"updatedAt":m.created_at,"type":if m.role=="user"{"user_message"}else{"assistant_message"},"messageId":message_id,"text":m.text});
        if m.role == "user" {
            item["createdBy"] = json!("user");
            item["creationSource"] = json!("server");
            item["inputIntent"] = json!("turn_start");
            item["attachments"] = json!([]);
        } else {
            item["streaming"] = json!(false);
        }
        events.push(json!({"id":format!("agent-session-import:v2:message:{}:{suffix}",id.0),"type":"message.updated","threadId":id,"occurredAt":m.created_at,"payload":{"createdBy":if m.role=="user"{"user"}else{"agent"},"creationSource":"server","id":message_id,"threadId":id,"runId":null,"nodeId":null,"role":m.role,"text":m.text,"attachments":[],"streaming":false,"createdAt":m.created_at,"updatedAt":m.created_at}}));
        events.push(json!({"id":item_id,"type":"turn-item.updated","threadId":id,"occurredAt":m.created_at,"payload":item}));
    }
    events.push(json!({"id":format!("agent-session-import:v2:provider-thread:{provider_id}"),"type":"provider-thread.updated","threadId":id,"driver":driver,"providerInstanceId":source.provider_instance_id,"occurredAt":thread.updated_at,"payload":provider}));
    Ok((
        events
            .into_iter()
            .map(serde_json::from_value)
            .collect::<std::result::Result<_, _>>()?,
        serde_json::from_value(provider)?,
    ))
}
