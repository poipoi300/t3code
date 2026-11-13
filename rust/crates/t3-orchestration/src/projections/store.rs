use crate::{
    api::*,
    error::{Error, Result},
    projections::{control, read, write},
    stores::StoreDatabase,
};
use async_trait::async_trait;
use t3_persistence::rusqlite::{Connection, params};

#[derive(Clone)]
pub struct ProjectionStore {
    pub database: StoreDatabase,
    find: Option<crate::ports::transport::thread_find::ThreadFind>,
}
pub type SqliteProjectionStore = ProjectionStore;
impl ProjectionStore {
    pub fn new(database: StoreDatabase) -> Self {
        Self {
            database,
            find: None,
        }
    }
    pub fn with_thread_find_text(
        mut self,
        text: std::sync::Arc<dyn crate::ports::transport::thread_find::ThreadFindText>,
    ) -> Self {
        self.find = Some(crate::ports::transport::thread_find::ThreadFind::new(
            std::sync::Arc::new(Self::new(self.database.clone())),
            text,
        ));
        self
    }
    pub async fn open_remote_delegated_tasks(&self) -> Result<Vec<read::RemoteDelegatedTask>> {
        self.read_snapshot(read::open_remote_delegated_tasks).await
    }
    pub async fn agent_awareness_snapshot(
        &self,
        id: Option<ThreadId>,
    ) -> Result<read::AgentAwarenessRead> {
        let raw = self
            .read_snapshot(move |db| read::read_awareness_rows(db, id.as_ref()))
            .await?;
        read::decode_awareness_rows(raw)
    }
    pub async fn pull_request_snapshot(
        &self,
        id: Option<ThreadId>,
        unsettled_only: bool,
    ) -> Result<read::PullRequestRead> {
        self.read_snapshot(move |db| read::pull_request_snapshot(db, id.as_ref(), unsettled_only))
            .await
    }
    pub async fn count_threads(&self) -> Result<u64> {
        self.read_snapshot(read::count_threads).await
    }
    pub async fn active_subagent_thread_ids(&self) -> Result<Vec<ThreadId>> {
        self.read_snapshot(read::active_subagent_thread_ids).await
    }
    pub async fn find_guest_share_thread_id(&self, id: GuestShareId) -> Result<Option<ThreadId>> {
        self.read_snapshot(move |db| read::find_guest_share_thread_id(db, &id))
            .await
    }
    pub async fn thread_worktree_paths(
        &self,
        include_archived: bool,
    ) -> Result<Vec<read::ThreadWorktreePath>> {
        self.read_snapshot(move |db| read::thread_worktree_paths(db, include_archived))
            .await
    }
    pub async fn thread_titles(&self, ids: Vec<ThreadId>) -> Result<OrchestrationV2ThreadTitles> {
        self.read_snapshot(move |db| read::thread_titles(db, &ids))
            .await
    }
    pub async fn checkpoint_context(&self, id: ThreadId) -> Result<control::CheckpointContext> {
        self.read_snapshot(move |db| control::checkpoint_context(db, &id))
            .await
    }
    pub async fn limit_recovery_candidates(
        &self,
        now: IsoDateTime,
        auto_resume: bool,
        snooze: bool,
    ) -> Result<Vec<read::LimitRecoveryCandidate>> {
        self.read_snapshot(move |db| read::limit_recovery_candidates(db, &now, auto_resume, snooze))
            .await
    }
    pub async fn limit_recovery_wake_at(&self, now: IsoDateTime) -> Result<Option<i64>> {
        self.read_snapshot(move |db| read::limit_recovery_wake_at(db, &now))
            .await
    }
    pub async fn runtime_response_context(
        &self,
        id: ThreadId,
        request_id: RuntimeRequestId,
    ) -> Result<Option<ControlContext>> {
        self.read_snapshot(move |db| control::runtime_response_context(db, &id, &request_id))
            .await
    }
    pub async fn thread_link_metadata(
        &self,
        ids: Vec<ThreadId>,
    ) -> Result<std::collections::BTreeMap<ThreadId, OrchestrationV2ThreadLinkMetadata>> {
        self.read_snapshot(move |db| read::thread_link_metadata(db, &ids))
            .await
    }
    pub async fn older_settled_threads(
        &self,
        input: OrchestrationV2GetOlderSettledThreadsInput,
    ) -> Result<OrchestrationV2OlderSettledThreadsPage> {
        let raw = self
            .read_snapshot(move |db| read::read_older_settled_rows(db, &input))
            .await?;
        read::decode_older_settled_rows(raw)
    }
    pub async fn read_snapshot<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        // StoreDatabase::read already holds one read transaction for the whole closure.
        self.database.read(f).await
    }
    pub async fn message_count(&self, thread_id: ThreadId) -> Result<u64> {
        self.read_snapshot(move |db| {
            let count: i64 = db.query_row(
                "SELECT COUNT(*) FROM orchestration_v2_projection_messages WHERE thread_id=?1",
                [thread_id.0],
                |row| row.get(0),
            )?;
            Ok(count as u64)
        })
        .await
    }
    pub async fn subagent_count(&self, thread_id: ThreadId) -> Result<u64> {
        self.read_snapshot(move |db| {
            let count: i64 = db.query_row(
                "SELECT COUNT(*) FROM orchestration_v2_projection_subagents WHERE thread_id=?1",
                [thread_id.0],
                |row| row.get(0),
            )?;
            Ok(count as u64)
        })
        .await
    }
    pub async fn item_count(&self, thread_id: ThreadId) -> Result<u64> {
        self.read_snapshot(move|db|{let count:i64=db.query_row("SELECT COALESCE(SUM(item_count),0) FROM orchestration_v2_projection_turn_item_counts WHERE thread_id=?1",[thread_id.0],|row|row.get(0))?;Ok(count as u64)}).await
    }
    pub async fn turn_start_context(
        &self,
        thread_id: ThreadId,
        run_id: RunId,
    ) -> Result<control::TurnStartContext> {
        self.read_snapshot(move |db| control::turn_start_context(db, &thread_id, &run_id))
            .await
    }
    pub async fn recovery_candidates(
        &self,
        kind: read::RecoveryKind,
        include: Option<Vec<ProjectId>>,
        exclude: Option<Vec<ProjectId>>,
    ) -> Result<Vec<ThreadId>> {
        self.read_snapshot(move |db| {
            read::recovery_thread_ids(db, kind, include.as_deref(), exclude.as_deref())
        })
        .await
    }
    pub async fn settlement_candidates(
        &self,
        thread_id: Option<ThreadId>,
    ) -> Result<Vec<OrchestrationV2ThreadShell>> {
        self.read_snapshot(move |db| read::settlement_candidates(db, thread_id.as_ref()))
            .await
    }
    pub async fn threads_with_pull_requests(
        &self,
        thread_id: Option<ThreadId>,
    ) -> Result<Vec<OrchestrationV2AppThread>> {
        self.read_snapshot(move |db| read::threads_with_pull_requests(db, thread_id.as_ref()))
            .await
    }
}
impl ProjectionWriter for ProjectionStore {
    fn apply(
        &self,
        tx: &t3_persistence::rusqlite::Transaction<'_>,
        event: &OrchestrationV2StoredEvent,
    ) -> Result<()> {
        write::apply(tx, event)
    }
}
#[async_trait]
impl ProjectionReader for ProjectionStore {
    async fn thread_titles(&self, ids: Vec<ThreadId>) -> Result<OrchestrationV2ThreadTitles> {
        ProjectionStore::thread_titles(self, ids).await
    }
    async fn thread_link_metadata(
        &self,
        ids: Vec<ThreadId>,
    ) -> Result<std::collections::BTreeMap<ThreadId, OrchestrationV2ThreadLinkMetadata>> {
        ProjectionStore::thread_link_metadata(self, ids).await
    }
    async fn older_settled_threads(
        &self,
        input: OrchestrationV2GetOlderSettledThreadsInput,
    ) -> Result<OrchestrationV2OlderSettledThreadsPage> {
        ProjectionStore::older_settled_threads(self, input).await
    }
    async fn runtime_response_context(
        &self,
        id: ThreadId,
        request_id: RuntimeRequestId,
    ) -> Result<Option<ControlContext>> {
        ProjectionStore::runtime_response_context(self, id, request_id).await
    }
    async fn checkpoint_context(&self, id: ThreadId) -> Result<CheckpointContext> {
        ProjectionStore::checkpoint_context(self, id).await
    }

    async fn turn_start_context(
        &self,
        thread_id: ThreadId,
        run_id: RunId,
    ) -> Result<TurnStartContext> {
        ProjectionStore::turn_start_context(self, thread_id, run_id).await
    }
    async fn subscriber_shell_snapshot(
        &self,
        select: Option<ShellIndexSelection>,
    ) -> Result<SubscriberShellRead> {
        let (raw, projects, index, sequence) = self.read_snapshot(move |db| {
            let (index, thread_ids) = if let Some(select) = select {
                let index = read::active_shell_index(db)?;
                let ids = select(&index);
                (Some(index), Some(ids))
            } else { (None, None) };
            let raw = read::read_shell_rows(db, &ShellQuery {thread_ids,..Default::default()},true)?;
            let projects = crate::stores::projects::list_shells(db,None)?;
            let sequence=db.query_row::<i64,_,_>("SELECT COALESCE(MAX(sequence),0) FROM orchestration_events WHERE application_event_version=2",[],|row|row.get(0))? as u64;
            Ok((raw,projects,index,sequence))
        }).await?;
        let shell = read::decode_shell_rows(raw)?;
        Ok(SubscriberShellRead {
            snapshot: OrchestrationV2ShellSnapshot {
                schema_version: 2,
                snapshot_sequence: sequence,
                threads: shell.threads,
                archived_threads: vec![],
                projects,
                older_settled: None,
            },
            index,
        })
    }
    async fn query_records(&self, query: ProjectionRecordQuery) -> Result<ProjectionRecords> {
        self.read_snapshot(move |db| read::query_records(db, &query))
            .await
    }
    async fn message_count(&self, thread_id: ThreadId, filter: MessageFilter) -> Result<u64> {
        self.read_snapshot(move |db| read::message_count(db, &thread_id, &filter))
            .await
    }
    async fn provider_target(&self, query: ProviderTargetQuery) -> Result<Option<ControlContext>> {
        self.read_snapshot(move |db| control::provider_target(db, &query))
            .await
    }
    async fn runtime_recovery(
        &self,
        thread_id: ThreadId,
    ) -> Result<OrchestrationV2ThreadProjection> {
        self.read_snapshot(move |db| control::runtime_recovery(db, &thread_id))
            .await
    }
    async fn records(
        &self,
        thread_id: ThreadId,
        fields: Vec<RecordKind>,
        filter: RecordFilter,
    ) -> Result<ProjectionRecords> {
        self.read_snapshot(move |db| read::records(db, &thread_id, &fields, &filter))
            .await
    }
    async fn snapshot(
        &self,
        thread_id: ThreadId,
        window: Option<SnapshotWindow>,
    ) -> Result<ProjectionSnapshot> {
        self.read_snapshot(move |db| match window {
            Some(window) => read::snapshot_window(db, &thread_id, &window),
            None => read::snapshot(db, &thread_id),
        })
        .await
    }
    async fn history(&self, query: HistoryQuery) -> Result<OrchestrationV2ThreadHistoryPage> {
        self.read_snapshot(move |db| read::history(db, &query))
            .await
    }
    async fn search(
        &self,
        query: OrchestrationV2SearchThreadInput,
    ) -> Result<OrchestrationV2SearchThreadResult> {
        self.find
            .as_ref()
            .ok_or(Error::Unsupported("thread find text backend"))?
            .search_thread(query)
            .await
    }
    async fn thread_shells(&self, query: ShellQuery) -> Result<OrchestrationV2ThreadShellSnapshot> {
        let raw = self
            .read_snapshot(move |db| read::read_shell_rows(db, &query, true))
            .await?;
        read::decode_shell_rows(raw)
    }
    async fn checkpoint_capture(
        &self,
        thread_id: ThreadId,
        run_id: RunId,
        scope_id: CheckpointScopeId,
    ) -> Result<CheckpointCaptureContext> {
        self.read_snapshot(move |db| {
            control::checkpoint_capture(db, &thread_id, &run_id, &scope_id)
        })
        .await
    }
    async fn checkpoint_rollback(
        &self,
        thread_id: ThreadId,
        provider_thread_id: ProviderThreadId,
        checkpoint_id: CheckpointId,
        scope_id: CheckpointScopeId,
    ) -> Result<CheckpointRollbackContext> {
        self.read_snapshot(move |db| {
            control::checkpoint_rollback(
                db,
                &thread_id,
                &provider_thread_id,
                &checkpoint_id,
                &scope_id,
            )
        })
        .await
    }
    async fn runtime_request(
        &self,
        thread_id: ThreadId,
        request_id: RuntimeRequestId,
    ) -> Result<Option<OrchestrationV2RuntimeRequest>> {
        self.read_snapshot(move|db|{read::require_thread(db,&thread_id)?;read::one(db,"SELECT payload_json FROM orchestration_v2_projection_runtime_requests WHERE thread_id=?1 AND runtime_request_id=?2",params![thread_id.0,request_id.0])}).await
    }
    async fn turn_item(
        &self,
        thread_id: ThreadId,
        item_id: TurnItemId,
    ) -> Result<Option<OrchestrationV2TurnItem>> {
        self.read_snapshot(move|db|read::one(db,"SELECT payload_json FROM orchestration_v2_projection_turn_items WHERE thread_id=?1 AND turn_item_id=?2",params![thread_id.0,item_id.0])).await
    }
    async fn plan(
        &self,
        thread_id: ThreadId,
        plan_id: PlanId,
    ) -> Result<Option<OrchestrationV2PlanArtifact>> {
        self.read_snapshot(move|db|read::one(db,"SELECT payload_json FROM orchestration_v2_projection_plans WHERE thread_id=?1 AND plan_id=?2",params![thread_id.0,plan_id.0])).await
    }
    async fn held_queued_runs(&self, thread_id: ThreadId) -> Result<Vec<OrchestrationV2Run>> {
        self.read_snapshot(move|db|read::rows(db,"SELECT payload_json FROM orchestration_v2_projection_runs WHERE thread_id=?1 AND status='queued' AND json_extract(payload_json,'$.queueHeld') IS 1 ORDER BY ordinal ASC",[&thread_id.0])).await
    }
    async fn app_owned_child_threads(&self, thread_id: ThreadId) -> Result<Vec<ThreadId>> {
        self.read_snapshot(move|db|{
        let mut statement=db.prepare("SELECT child_thread_id FROM orchestration_v2_projection_subagents WHERE thread_id=?1 AND origin='app_owned' AND child_thread_id IS NOT NULL ORDER BY COALESCE(started_at,''),subagent_id ASC")?;
        let mut ids=Vec::new();for id in statement.query_map([&thread_id.0],|row|Ok(ThreadId(row.get(0)?)))?{let id=id?;if !ids.contains(&id){ids.push(id);}}Ok(ids)
    }).await
    }
    async fn pending_delegated_completion(&self, child_thread_id: ThreadId) -> Result<bool> {
        self.read_snapshot(move|db|Ok(db.query_row("SELECT EXISTS(SELECT 1 FROM orchestration_v2_projection_subagents AS task LEFT JOIN orchestration_v2_projection_runs AS owner ON owner.run_id=task.run_id WHERE task.child_thread_id=?1 AND task.origin='app_owned' AND (json_extract(task.payload_json,'$.result') IS NULL OR (task.completion_delivery_state='claimed' AND json_extract(owner.payload_json,'$.delegatedCompletion.disposition')='open' AND json_type(owner.payload_json,'$.delegatedCompletion.delivery')='object')))",[&child_thread_id.0],|row|row.get(0))?)).await
    }
    async fn turn_start_history(
        &self,
        thread_id: ThreadId,
        run_ids: Option<Vec<RunId>>,
    ) -> Result<Vec<OrchestrationV2TurnItem>> {
        self.read_snapshot(move|db|read::rows(db,"SELECT payload_json FROM orchestration_v2_projection_turn_items WHERE thread_id=?1 AND type IN ('user_message','assistant_message','command_execution','error','run_interrupt_result','file_change','proposed_plan') AND (?2 IS NULL OR run_id IN (SELECT value FROM json_each(?2))) ORDER BY ordinal ASC,turn_item_id ASC",params![thread_id.0,run_ids.map(|ids|serde_json::to_string(&ids)).transpose()?])).await
    }
    async fn unpaired_interrupt(
        &self,
        thread_id: ThreadId,
        request_id: TurnItemId,
        result_id: TurnItemId,
    ) -> Result<bool> {
        self.read_snapshot(move|db|Ok(db.query_row("SELECT EXISTS(SELECT 1 FROM orchestration_v2_projection_turn_items WHERE thread_id=?1 AND turn_item_id=?2) AND NOT EXISTS(SELECT 1 FROM orchestration_v2_projection_turn_items WHERE thread_id=?1 AND turn_item_id=?3)",params![thread_id.0,request_id.0,result_id.0],|row|row.get(0))?)).await
    }
    async fn attachment_ids(&self, thread_id: ThreadId) -> Result<Vec<String>> {
        self.read_snapshot(move |db| read::attachment_ids(db, &thread_id))
            .await
    }
    async fn transcript_byte_floor(&self, thread_id: ThreadId, limit: u64) -> Result<u64> {
        self.read_snapshot(move|db|{
        let count:i64=db.query_row("WITH tool_rows AS (SELECT payload_json FROM orchestration_v2_projection_turn_items WHERE thread_id=?1 AND type IN ('command_execution','dynamic_tool') AND (run_id IS NULL OR run_id NOT IN (SELECT run_id FROM orchestration_v2_projection_runs WHERE thread_id=?1 AND status='rolled_back'))) SELECT CASE WHEN (SELECT COALESCE(SUM(octet_length(payload_json)),0) FROM tool_rows)<=?2 THEN 0 ELSE (SELECT COALESCE(SUM(CASE WHEN json_valid(payload_json) THEN (SELECT COALESCE(SUM(octet_length(leaf.value)),0) FROM json_tree(tool_rows.payload_json,'$.output') AS leaf WHERE leaf.type='text') ELSE 0 END),0) FROM tool_rows) END",params![thread_id.0,read::integer(limit)?],|row|row.get(0))?;Ok(count as u64)
    }).await
    }
    async fn thread(&self, thread_id: ThreadId) -> Result<Option<OrchestrationV2AppThread>> {
        self.read_snapshot(move |db| read::thread(db, &thread_id))
            .await
    }
    async fn projection(
        &self,
        thread_id: ThreadId,
    ) -> Result<Option<OrchestrationV2ThreadProjection>> {
        self.read_snapshot(move |db| {
            if read::thread(db, &thread_id)?.is_some() {
                Ok(Some(read::projection(db, &thread_id)?))
            } else {
                Ok(None)
            }
        })
        .await
    }
    async fn shell(&self, thread_id: ThreadId) -> Result<Option<OrchestrationV2ThreadShell>> {
        let shells = self
            .thread_shells(ShellQuery {
                archived: true,
                thread_ids: Some(vec![thread_id]),
                ..Default::default()
            })
            .await?;
        Ok(shells
            .threads
            .into_iter()
            .chain(shells.archived_threads)
            .next())
    }
    async fn shell_snapshot(
        &self,
        project_ids: Option<Vec<ProjectId>>,
        root_thread_id: Option<ThreadId>,
    ) -> Result<OrchestrationV2ShellSnapshot> {
        let (raw,projects)=self.read_snapshot(move |db| {
            let thread_ids=if let Some(ids)=&project_ids {
                let json=t3_contracts::stored::to_string(ids)?;
                let mut statement=db.prepare("SELECT thread_id FROM orchestration_v2_projection_threads WHERE project_id IN(SELECT value FROM json_each(?1)) AND deleted_at IS NULL")?;
                Some(statement.query_map([json],|r|Ok(ThreadId(r.get(0)?)))?.collect::<std::result::Result<Vec<_>,_>>()?)
            } else {None};
            let raw=read::read_shell_rows(db,&ShellQuery{archived:true,family_of:root_thread_id,thread_ids,..Default::default()},true)?;
            let projects=crate::stores::projects::list_shells(db,project_ids.as_deref())?;
            Ok((raw,projects))
        }).await?;
        let snapshot = read::decode_shell_rows(raw)?;
        Ok(OrchestrationV2ShellSnapshot {
            schema_version: 2,
            snapshot_sequence: snapshot.snapshot_sequence,
            threads: snapshot.threads,
            archived_threads: snapshot.archived_threads,
            projects,
            older_settled: None,
        })
    }
    async fn control(&self, query: ControlQuery) -> Result<Option<ControlContext>> {
        self.read_snapshot(move |db| control::control(db, &query))
            .await
    }
    async fn timeline(&self, query: TimelineQuery) -> Result<TimelinePage> {
        self.read_snapshot(move |db| read::timeline(db, &query))
            .await
    }
    async fn run(&self, thread_id: ThreadId, run_id: RunId) -> Result<Option<OrchestrationV2Run>> {
        self.read_snapshot(move|db|{read::require_thread(db,&thread_id)?;read::one(db,"SELECT payload_json FROM orchestration_v2_projection_runs WHERE thread_id=?1 AND run_id=?2",params![thread_id.0,run_id.0])}).await
    }
    async fn run_message(
        &self,
        thread_id: ThreadId,
        run_id: RunId,
    ) -> Result<Option<OrchestrationV2ConversationMessage>> {
        self.read_snapshot(move|db|{read::require_thread(db,&thread_id)?;read::one(db,"SELECT message.payload_json FROM orchestration_v2_projection_messages AS message JOIN orchestration_v2_projection_runs AS run ON message.message_id=json_extract(run.payload_json,'$.userMessageId') WHERE run.thread_id=?1 AND run.run_id=?2 AND message.thread_id=?1",params![thread_id.0,run_id.0])}).await
    }
    async fn can_start_queued_run(&self, thread_id: ThreadId, _run_id: RunId) -> Result<bool> {
        self.read_snapshot(move|db|{read::require_thread(db,&thread_id)?;Ok(db.query_row("SELECT EXISTS(SELECT 1 FROM orchestration_v2_projection_threads AS thread WHERE thread.thread_id=?1 AND thread.archived_at IS NULL AND thread.deleted_at IS NULL AND EXISTS(SELECT 1 FROM orchestration_v2_projection_runs WHERE thread_id=?1 AND status='queued' AND json_extract(payload_json,'$.queueHeld') IS NOT 1) AND NOT EXISTS(SELECT 1 FROM orchestration_v2_projection_runs WHERE thread_id=?1 AND status IN ('preparing','starting','running','waiting')))",[&thread_id.0],|row|row.get(0))?)}).await
    }
    async fn recovery_thread_ids(&self) -> Result<Vec<ThreadId>> {
        self.read_snapshot(|db| {
            read::recovery_thread_ids(db, read::RecoveryKind::Runtime, None, None)
        })
        .await
    }
    async fn unreadable_thread_ids(&self) -> Result<Vec<ThreadId>> {
        self.read_snapshot(read::unreadable_thread_ids).await
    }
    async fn next_turn_item_ordinal(
        &self,
        thread_id: ThreadId,
        _run_id: Option<RunId>,
    ) -> Result<u64> {
        self.read_snapshot(move|db|{let ordinal:i64=db.query_row("SELECT COALESCE(MAX(ordinal),0)+1 FROM orchestration_v2_projection_turn_items WHERE thread_id=?1",[&thread_id.0],|row|row.get(0))?;Ok(ordinal as u64)}).await
    }
}

#[async_trait]
impl crate::ports::transport::thread_find::ThreadFindSnapshotReader for ProjectionStore {
    async fn snapshot(
        &self,
        thread_id: ThreadId,
    ) -> Result<crate::ports::transport::thread_find::FindSnapshot> {
        self.read_snapshot(move |db| read::thread_find_snapshot(db, &thread_id))
            .await
    }
}
