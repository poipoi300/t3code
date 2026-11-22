use super::{
    queries::{CommandLane, QueryRegistry, command_lane},
    routing::{PullRequestRouter, RoutingRegistry},
};
use crate::{
    platform::Platform,
    rpc::RpcCallError,
    types::EnvironmentId,
    workspace::pull_requests::{QueryKey, QueryKind},
};
use futures::{
    FutureExt,
    channel::oneshot,
    future::{LocalBoxFuture, Shared, WeakShared},
};
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, VecDeque},
    rc::Rc,
};
use t3_contracts::{pull_request::*, rpc::*};

struct PendingQuery<'a> {
    registry: &'a RefCell<QueryRegistry>,
    key: &'a QueryKey,
    ticket: crate::workspace::pull_requests::QueryTicket,
}
impl Drop for PendingQuery<'_> {
    fn drop(&mut self) {
        self.registry.borrow_mut().discard(self.key, self.ticket);
    }
}

#[derive(Default)]
struct Lane {
    owner: u64,
    waiting: VecDeque<(u64, oneshot::Sender<()>)>,
}
#[derive(Default)]
struct Scheduler {
    lanes: RefCell<BTreeMap<String, Lane>>,
    sequence: Cell<u64>,
}
struct Permit {
    scheduler: Rc<Scheduler>,
    lane: String,
    id: u64,
}
impl Drop for Permit {
    fn drop(&mut self) {
        let mut lanes = self.scheduler.lanes.borrow_mut();
        let Some(lane) = lanes.get_mut(&self.lane) else {
            return;
        };
        if lane.owner != self.id {
            lane.waiting.retain(|(id, _)| *id != self.id);
            return;
        }
        while let Some((id, next)) = lane.waiting.pop_front() {
            lane.owner = id;
            if next.send(()).is_ok() {
                return;
            }
        }
        lanes.remove(&self.lane);
    }
}
impl Scheduler {
    async fn acquire(self: &Rc<Self>, key: String) -> Result<Permit, Rc<RpcCallError>> {
        self.sequence.set(self.sequence.get() + 1);
        let id = self.sequence.get();
        let receiver = {
            let mut lanes = self.lanes.borrow_mut();
            if let Some(lane) = lanes.get_mut(&key) {
                let (tx, rx) = oneshot::channel();
                lane.waiting.push_back((id, tx));
                Some(rx)
            } else {
                lanes.insert(
                    key.clone(),
                    Lane {
                        owner: id,
                        waiting: VecDeque::new(),
                    },
                );
                None
            }
        };
        let permit = Permit {
            scheduler: self.clone(),
            lane: key,
            id,
        };
        if let Some(receiver) = receiver {
            receiver
                .await
                .map_err(|_| Rc::new(RpcCallError::Interrupted))?;
        }
        Ok(permit)
    }
}
type CommandResult = Result<Value, Rc<RpcCallError>>;
type SharedCommand = Shared<LocalBoxFuture<'static, CommandResult>>;
type WeakCommand = WeakShared<LocalBoxFuture<'static, CommandResult>>;
pub type MergeMethodResolver =
    Rc<dyn Fn(&PullRequestDetail) -> Result<PullRequestMergeMethod, String>>;

pub struct PullRequestService<P, R> {
    platform: Rc<P>,
    registry: Rc<R>,
    pub router: PullRequestRouter<P, R>,
    pub queries: Rc<RefCell<QueryRegistry>>,
    scheduler: Rc<Scheduler>,
    flights: RefCell<BTreeMap<String, WeakCommand>>,
}
impl<P: Platform + 'static, R: RoutingRegistry + 'static> PullRequestService<P, R> {
    pub fn new(platform: Rc<P>, registry: Rc<R>) -> Rc<Self> {
        let queries = registry.routing_state().queries.clone();
        Rc::new(Self {
            router: PullRequestRouter::new(platform.clone(), registry.clone()),
            platform,
            registry,
            queries,
            scheduler: Rc::new(Scheduler::default()),
            flights: RefCell::new(BTreeMap::new()),
        })
    }
    pub async fn load(&self, key: &QueryKey) -> Result<bool, Rc<RpcCallError>> {
        let session = self
            .registry
            .generation(&key.environment)
            .ok_or_else(|| Rc::new(RpcCallError::Interrupted))?;
        let tag = key.kind.rpc_tag().ok_or_else(|| {
            Rc::new(RpcCallError::NotSent(crate::ClientError::Unsupported(
                "diff uses DiffLoader",
            )))
        })?;
        let (ticket, input) = {
            let mut queries = self.queries.borrow_mut();
            let input = queries
                .get(key)
                .ok_or_else(|| Rc::new(RpcCallError::Interrupted))?
                .input
                .clone();
            let ticket = queries
                .begin(key, session)
                .ok_or_else(|| Rc::new(RpcCallError::Interrupted))?;
            (ticket, input)
        };
        let _pending = PendingQuery {
            registry: &self.queries,
            key,
            ticket,
        };
        let result = self
            .router
            .request_value(&key.environment, tag, input)
            .await;
        if self.registry.generation(&key.environment) != Some(session) {
            return Ok(false);
        }
        match result {
            Ok(value) => self
                .queries
                .borrow_mut()
                .finish(key, ticket, Ok(value), self.platform.now_epoch_ms())
                .map_err(|e| Rc::new(RpcCallError::NotSent(e))),
            Err(error) => {
                let _ = self.queries.borrow_mut().finish(
                    key,
                    ticket,
                    Err(error.to_string()),
                    self.platform.now_epoch_ms(),
                );
                Err(Rc::new(error))
            }
        }
    }
    pub fn command(
        self: &Rc<Self>,
        environment: EnvironmentId,
        tag: String,
        input: Value,
        resolve_merge_method: Option<MergeMethodResolver>,
    ) -> LocalBoxFuture<'static, CommandResult> {
        let service = self.clone();
        let lane = command_lane(&environment, &tag, &input);
        if let CommandLane::SingleFlight(key) = lane {
            let existing = self
                .flights
                .borrow()
                .get(&key)
                .and_then(WeakShared::upgrade);
            let future: SharedCommand = existing.unwrap_or_else(|| {
                async move {
                    service
                        .execute(environment, tag, input, resolve_merge_method, None)
                        .await
                }
                .boxed_local()
                .shared()
            });
            if let Some(weak) = future.downgrade() {
                let mut flights = self.flights.borrow_mut();
                flights.retain(|_, future| future.upgrade().is_some());
                flights.insert(key, weak);
            }
            return future.boxed_local();
        }
        let key = match lane {
            CommandLane::SerialEnvironment(env) => Some(json!(["environment", env]).to_string()),
            CommandLane::SerialViewed(env, project, repository, number) => {
                Some(json!(["viewed", env, project, repository, number]).to_string())
            }
            _ => None,
        };
        async move {
            service
                .execute(environment, tag, input, resolve_merge_method, key)
                .await
        }
        .boxed_local()
    }
    async fn execute(
        &self,
        environment: EnvironmentId,
        tag: String,
        mut input: Value,
        resolver: Option<MergeMethodResolver>,
        lane: Option<String>,
    ) -> CommandResult {
        let session = self
            .registry
            .generation(&environment)
            .ok_or_else(|| Rc::new(RpcCallError::Interrupted))?;
        let _permit = if let Some(lane) = lane {
            Some(self.scheduler.acquire(lane).await?)
        } else {
            None
        };
        if self.registry.generation(&environment) != Some(session) {
            return Err(Rc::new(RpcCallError::Interrupted));
        }
        self.registry
            .authorize(&environment, &tag, &input)
            .map_err(Rc::new)?;
        let before = self.queries.borrow().reads_in_flight();
        if tag == "pullRequests.runAction" {
            if input["action"] == "merge"
                && let Some(resolver) = resolver
            {
                let mut reference: PullRequestRef = serde_json::from_value(input.clone())
                    .map_err(|_| Rc::new(RpcCallError::InvalidResponse))?;
                reference.allow_stale = Some(Some(false));
                let detail = self
                    .router
                    .request::<WsPullRequestsDetailRpc>(&environment, reference.clone())
                    .await
                    .map_err(Rc::new)?;
                let fail = |detail: &str| {
                    Rc::new(RpcCallError::Rejected(vec![
                        json!({"_tag": "PullRequestOperationError", "operation": "runAction", "detail": detail}),
                    ]))
                };
                if detail.state != PullRequestState::Open
                    || detail.is_draft
                    || !detail
                        .capabilities
                        .actions
                        .contains(&PullRequestAction::Merge)
                    || !detail
                        .viewer_permissions
                        .actions
                        .contains(&PullRequestAction::Merge)
                {
                    return Err(fail("This pull request cannot be merged."));
                }
                if detail.capabilities.stack_actions == Some(Some(true))
                    && self
                        .router
                        .request::<WsPullRequestsStackRpc>(&environment, reference)
                        .await
                        .map_err(Rc::new)?
                        .is_some()
                {
                    return Err(fail("Open this pull request to merge its stack."));
                }
                input["mergeMethod"] = json!(resolver(&detail).map_err(|error| fail(&error))?);
            }
            input["replyWithSummary"] = json!(true);
        }
        let result = self
            .router
            .request_value(&environment, &tag, input.clone())
            .await
            .map_err(Rc::new)?;
        if self.registry.generation(&environment) != Some(session) {
            return Err(Rc::new(RpcCallError::Interrupted));
        }
        let mut queries = self.queries.borrow_mut();
        if tag == "pullRequests.runAction" {
            let reference = serde_json::from_value(input.clone())
                .map_err(|_| Rc::new(RpcCallError::InvalidResponse))?;
            let reply: Option<PullRequestSummary> = serde_json::from_value(result.clone())
                .map_err(|_| Rc::new(RpcCallError::InvalidResponse))?;
            queries
                .apply_action_reply(
                    &environment,
                    &reference,
                    reply.as_ref(),
                    &before,
                    self.platform.now_epoch_ms(),
                )
                .map_err(|e| Rc::new(RpcCallError::NotSent(e)))?;
        }
        queries
            .command_succeeded(&environment, &tag, &input, &result, session)
            .map_err(|e| Rc::new(RpcCallError::NotSent(e)))?;
        Ok(result)
    }
    pub fn refresh(&self, environment: &EnvironmentId, linked_only: bool) {
        if let Some(session) = self.registry.generation(environment) {
            self.queries
                .borrow_mut()
                .server_refresh(environment, session, linked_only);
        }
    }
    pub fn remove_environment(&self, environment: &EnvironmentId) {
        self.router.remove_environment(environment);
        self.queries.borrow_mut().remove_environment(environment);
    }
}

pub async fn load_registered_diff(
    registry: &RefCell<QueryRegistry>,
    key: &QueryKey,
    session: crate::types::Generation,
    loader: &super::diff_http::DiffLoader<'_>,
    prepared: &crate::authorization::AuthorizedEnvironment,
    relay_target: bool,
    current: impl Fn() -> bool,
) -> Result<bool, super::diff_http::DiffLoadError> {
    if key.kind != QueryKind::Diff {
        return Err(super::diff_http::DiffLoadError::InvalidResponse);
    }
    let (ticket, input) = {
        let mut registry = registry.borrow_mut();
        let input = registry
            .get(key)
            .ok_or(super::diff_http::DiffLoadError::Cancelled)?
            .input
            .clone();
        let input = serde_json::from_value(input)
            .map_err(|_| super::diff_http::DiffLoadError::InvalidResponse)?;
        let ticket = registry
            .begin(key, session)
            .ok_or(super::diff_http::DiffLoadError::Cancelled)?;
        (ticket, input)
    };
    let _pending = PendingQuery {
        registry,
        key,
        ticket,
    };
    let result = loader
        .load(prepared, &input, relay_target, &current, None)
        .await;
    if !current() {
        return Ok(false);
    }
    let value = match &result {
        Ok(value) => serde_json::to_value(value)
            .map_err(|_| super::diff_http::DiffLoadError::InvalidResponse),
        Err(error) => {
            let _ = registry.borrow_mut().finish(
                key,
                ticket,
                Err(error.to_string()),
                loader.platform.now_epoch_ms(),
            );
            return result.map(|_| false);
        }
    }?;
    registry
        .borrow_mut()
        .finish(key, ticket, Ok(value), loader.platform.now_epoch_ms())
        .map_err(|_| super::diff_http::DiffLoadError::InvalidResponse)
}

/// Global list queries run on each selected server. The session endpoint enforces
/// the live grant and decodes the server's typed list result.
impl<P: Platform + Clone + 'static> crate::composition::ClientRuntime<P> {
    pub async fn pull_request_list(
        &self,
        environment: &EnvironmentId,
        input: PullRequestListInput,
    ) -> Result<PullRequestListResult, RpcCallError> {
        use crate::rpc::RpcEndpoint;
        self.pr_session(environment)?
            .command::<WsPullRequestsListRpc>(input)
            .await
    }

    pub async fn pull_request_filter_options(
        &self,
        environment: &EnvironmentId,
        input: PullRequestFilterOptionsInput,
    ) -> Result<PullRequestFilterOptions, RpcCallError> {
        use crate::rpc::RpcEndpoint;
        self.pr_session(environment)?
            .command::<WsPullRequestsFilterOptionsRpc>(input)
            .await
    }

    fn pr_session(
        &self,
        environment: &EnvironmentId,
    ) -> Result<crate::rpc::SessionEndpoint, RpcCallError> {
        self.client(environment)
            .and_then(|client| client.session())
            .ok_or(RpcCallError::NotSent(crate::ClientError::Disconnected))
    }

    pub async fn pull_request_detail(
        &self,
        env: &EnvironmentId,
        reference: PullRequestRef,
    ) -> Result<PullRequestDetail, RpcCallError> {
        use crate::rpc::RpcEndpoint;
        self.pr_session(env)?
            .command::<WsPullRequestsDetailRpc>(reference)
            .await
    }

    pub async fn pull_request_linked_threads(
        &self,
        env: &EnvironmentId,
        reference: PullRequestRef,
    ) -> Result<PullRequestLinkedThreadsResult, RpcCallError> {
        use crate::rpc::RpcEndpoint;
        self.pr_session(env)?
            .command::<WsPullRequestsLinkedThreadsRpc>(reference)
            .await
    }
}

/// Parameters for `PullRequestReviewSession::run_action_with`, supporting stack and branch updates.
#[derive(Debug, Clone)]
pub struct PullRequestActionOptions {
    pub action: PullRequestAction,
    pub merge_method: Option<PullRequestMergeMethod>,
    pub update_method: Option<PullRequestUpdateMethod>,
    pub number_override: Option<t3_contracts::base_schemas::PositiveInt>,
    pub stack_number: Option<t3_contracts::base_schemas::PositiveInt>,
    pub expected_stack_heads: Option<Vec<PullRequestStackHead>>,
}

#[derive(Clone)]
pub struct PullRequestReviewSession {
    connection: Rc<dyn Fn() -> Option<crate::connection::ConnectedClient>>,
    changes: Rc<dyn Fn() -> crate::store::Subscription>,
    reference: PullRequestRef,
}
impl<P: Platform + Clone + 'static> crate::composition::ClientRuntime<P> {
    pub fn pull_request_review_session(
        &self,
        env: &EnvironmentId,
        reference: PullRequestRef,
    ) -> PullRequestReviewSession {
        let runtime = self.clone();
        let env = env.clone();
        let watched = env.clone();
        let store = self.store().clone();
        PullRequestReviewSession {
            connection: Rc::new(move || runtime.client(&env)),
            changes: Rc::new(move || store.subscribe_environment(&watched)),
            reference,
        }
    }
}
impl PullRequestReviewSession {
    pub fn changes(&self) -> crate::store::Subscription {
        (self.changes)()
    }
    fn endpoint(&self) -> Result<crate::rpc::SessionEndpoint, RpcCallError> {
        (self.connection)()
            .and_then(|c| c.session())
            .ok_or(RpcCallError::NotSent(crate::ClientError::Disconnected))
    }
    pub fn can_write(&self) -> bool {
        self.endpoint().ok().and_then(|e| {
            e.grants_scope(t3_contracts::auth::AuthEnvironmentScope::SourceControlWrite)
        }) == Some(true)
    }
    pub async fn stack(&self) -> Result<Option<PullRequestStack>, RpcCallError> {
        use crate::rpc::RpcEndpoint;
        self.endpoint()?
            .command::<WsPullRequestsStackRpc>(self.reference.clone())
            .await
    }
    pub async fn guide(
        &self,
    ) -> Result<t3_contracts::pull_request_review_guide::PullRequestReviewGuideResult, RpcCallError>
    {
        use crate::rpc::RpcEndpoint;
        self.endpoint()?
            .command::<WsPullRequestsReviewGuideRpc>(self.reference.clone())
            .await
    }
    pub async fn run_action(
        &self,
        action: PullRequestAction,
        merge_method: Option<PullRequestMergeMethod>,
    ) -> Result<Option<PullRequestSummary>, RpcCallError> {
        self.run_action_with(PullRequestActionOptions {
            action,
            merge_method,
            update_method: None,
            number_override: None,
            stack_number: None,
            expected_stack_heads: None,
        })
        .await
    }

    pub async fn run_action_with(
        &self,
        options: PullRequestActionOptions,
    ) -> Result<Option<PullRequestSummary>, RpcCallError> {
        use crate::rpc::RpcEndpoint;
        let r = &self.reference;
        self.endpoint()?
            .command::<WsPullRequestsRunActionRpc>(PullRequestActionInput {
                project_id: r.project_id.clone(),
                repository: r.repository.clone(),
                number: options.number_override.unwrap_or(r.number),
                host: r.host.clone(),
                expected_account_id: r.expected_account_id.clone(),
                allow_stale: r.allow_stale,
                action: options.action,
                merge_method: options.merge_method.map(Some),
                update_method: options.update_method.map(Some),
                stack_number: options.stack_number.map(Some),
                expected_stack_heads: options.expected_stack_heads.map(Some),
                reply_with_summary: Some(Some(PullRequestActionInputReplyWithSummary)),
            })
            .await
    }

    pub async fn reviewer_candidates(
        &self,
        query: Option<t3_contracts::base_schemas::TrimmedNonEmptyString>,
    ) -> Result<PullRequestReviewerCandidateList, RpcCallError> {
        use crate::rpc::RpcEndpoint;
        let r = &self.reference;
        self.endpoint()?
            .command::<WsPullRequestsReviewerCandidatesRpc>(PullRequestReviewerCandidatesInput {
                project_id: r.project_id.clone(),
                host: r.host.clone(),
                expected_account_id: r.expected_account_id.clone(),
                allow_stale: r.allow_stale,
                repository: r.repository.clone(),
                number: r.number,
                query: query.map(Some),
            })
            .await
    }

    pub async fn request_reviewers(
        &self,
        reviewers: Vec<PullRequestReviewerRequestInputReviewers>,
        requested: bool,
    ) -> Result<(), RpcCallError> {
        use crate::rpc::RpcEndpoint;
        let r = &self.reference;
        self.endpoint()?
            .command::<WsPullRequestsRequestReviewersRpc>(PullRequestReviewerRequestInput {
                project_id: r.project_id.clone(),
                host: r.host.clone(),
                expected_account_id: r.expected_account_id.clone(),
                allow_stale: r.allow_stale,
                repository: r.repository.clone(),
                number: r.number,
                reviewers,
                requested,
            })
            .await
    }

    pub async fn label_candidates(&self) -> Result<PullRequestLabelCandidateList, RpcCallError> {
        use crate::rpc::RpcEndpoint;
        self.endpoint()?
            .command::<WsPullRequestsLabelCandidatesRpc>(self.reference.clone())
            .await
    }

    pub async fn set_labels(
        &self,
        labels: Vec<t3_contracts::base_schemas::TrimmedNonEmptyString>,
        applied: bool,
    ) -> Result<(), RpcCallError> {
        use crate::rpc::RpcEndpoint;
        let r = &self.reference;
        self.endpoint()?
            .command::<WsPullRequestsSetLabelsRpc>(PullRequestLabelChangeInput {
                project_id: r.project_id.clone(),
                host: r.host.clone(),
                expected_account_id: r.expected_account_id.clone(),
                allow_stale: r.allow_stale,
                repository: r.repository.clone(),
                number: r.number,
                labels,
                applied,
            })
            .await
    }

    pub async fn list_refs(
        &self,
        cwd: t3_contracts::base_schemas::TrimmedNonEmptyString,
    ) -> Result<t3_contracts::git::VcsListRefsResult, RpcCallError> {
        use crate::rpc::RpcEndpoint;
        self.endpoint()?
            .command::<WsVcsListRefsRpc>(t3_contracts::git::VcsListRefsInput {
                cwd,
                query: None,
                cursor: None,
                include_matching_remote_refs: Some(Some(true)),
                ref_kind: None,
                refresh: None,
                limit: Some(Some(2)),
            })
            .await
    }
    pub async fn activity(&self) -> Result<PullRequestActivity, RpcCallError> {
        use crate::rpc::RpcEndpoint;
        self.endpoint()?
            .command::<WsPullRequestsActivityRpc>(self.reference.clone())
            .await
    }
    pub async fn files_viewed(&self) -> Result<PullRequestFilesViewedResult, RpcCallError> {
        use crate::rpc::RpcEndpoint;
        self.endpoint()?
            .command::<WsPullRequestsFilesViewedRpc>(self.reference.clone())
            .await
    }
    pub async fn diff(
        &self,
        cursor: Option<t3_contracts::base_schemas::TrimmedNonEmptyString>,
    ) -> Result<PullRequestDiffResult, super::diff_http::DiffLoadError> {
        let input = PullRequestDiffInput {
            project_id: self.reference.project_id.clone(),
            host: self.reference.host.clone(),
            expected_account_id: self.reference.expected_account_id.clone(),
            allow_stale: self.reference.allow_stale,
            repository: self.reference.repository.clone(),
            number: self.reference.number,
            cursor: cursor.map(Some),
            commit: None,
        };
        let connection = (self.connection)().ok_or(super::diff_http::DiffLoadError::Cancelled)?;
        let endpoint = connection
            .session()
            .ok_or(super::diff_http::DiffLoadError::Cancelled)?;
        let http = connection
            .pull_request_http()
            .ok_or(super::diff_http::DiffLoadError::Cancelled)?;
        let authorized = || {
            endpoint.grants_scope(t3_contracts::auth::AuthEnvironmentScope::OrchestrationRead)
                == Some(true)
        };
        if !authorized() {
            return Err(super::diff_http::DiffLoadError::PermissionDenied);
        }
        http.pull_request_diff(&input, authorized).await
    }
    pub async fn thread_comments(
        &self,
        thread_id: t3_contracts::base_schemas::TrimmedNonEmptyString,
        cursor: t3_contracts::base_schemas::TrimmedNonEmptyString,
    ) -> Result<PullRequestThreadCommentsResult, RpcCallError> {
        use crate::rpc::RpcEndpoint;
        let r = &self.reference;
        self.endpoint()?
            .command::<WsPullRequestsThreadCommentsRpc>(PullRequestThreadCommentsInput {
                project_id: r.project_id.clone(),
                host: r.host.clone(),
                expected_account_id: r.expected_account_id.clone(),
                allow_stale: r.allow_stale,
                repository: r.repository.clone(),
                number: r.number,
                thread_id,
                cursor,
            })
            .await
    }
    pub async fn set_viewed(
        &self,
        file: PullRequestSetFilesViewedInputFiles,
    ) -> Result<(), RpcCallError> {
        use crate::rpc::RpcEndpoint;
        self.endpoint()?
            .command::<WsPullRequestsSetFilesViewedRpc>(PullRequestSetFilesViewedInput {
                project_id: self.reference.project_id.clone(),
                host: self.reference.host.clone(),
                expected_account_id: self.reference.expected_account_id.clone(),
                allow_stale: self.reference.allow_stale,
                repository: self.reference.repository.clone(),
                number: self.reference.number,
                files: vec![file],
            })
            .await
    }
    pub async fn comment(
        &self,
        thread: Option<t3_contracts::base_schemas::TrimmedNonEmptyString>,
        body: String,
    ) -> Result<(), RpcCallError> {
        use crate::rpc::RpcEndpoint;
        if let Some(thread_id) = thread {
            self.endpoint()?
                .command::<WsPullRequestsReplyToThreadRpc>(PullRequestThreadReplyInput {
                    project_id: self.reference.project_id.clone(),
                    host: self.reference.host.clone(),
                    expected_account_id: self.reference.expected_account_id.clone(),
                    allow_stale: self.reference.allow_stale,
                    repository: self.reference.repository.clone(),
                    number: self.reference.number,
                    thread_id,
                    body,
                })
                .await
        } else {
            self.endpoint()?
                .command::<WsPullRequestsCommentRpc>(PullRequestCommentInput {
                    project_id: self.reference.project_id.clone(),
                    host: self.reference.host.clone(),
                    expected_account_id: self.reference.expected_account_id.clone(),
                    allow_stale: self.reference.allow_stale,
                    repository: self.reference.repository.clone(),
                    number: self.reference.number,
                    body,
                })
                .await
        }
    }
}

#[cfg(test)]
mod review_session_tests {
    use super::*;
    use crate::{
        ClientStore,
        composition::{ClientRuntime, Spawner},
        testing::{CallKind, environment::FakeEnvironment},
    };
    use futures::{executor::LocalPool, task::LocalSpawnExt};
    use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;

    #[test]
    fn read_only_review_sessions_can_read_guides_but_cannot_dispatch_lifecycle_writes() {
        let mut pool = LocalPool::new();
        let worker = pool.spawner();
        let spawn: Spawner = Rc::new(move |f| worker.spawn_local(f).expect("spawn"));
        let server = FakeEnvironment::new("read-only", spawn.clone());
        server.set_permissions(&["orchestration:read"]);
        server.register("pullRequests.runAction", CallKind::Unary);
        server.register("pullRequests.reviewGuide", CallKind::Unary);
        server.reply(
            "pullRequests.reviewGuide",
            Ok(json!({"currentHeadSha":"revision","guide":null,"run":null})),
        );
        let runtime = ClientRuntime::new(
            server.platform(),
            ClientStore::default(),
            OrchestrationV2CreationSource::Web,
            spawn,
        );
        let environment = pool
            .run_until(runtime.pair(server.pairing()))
            .expect("pair");
        pool.run_until_stalled();
        let reference = json!({"projectId":"project","repository":"team/widget","number":14,"host":"github.com"});
        let session = runtime.pull_request_review_session(
            &environment,
            serde_json::from_value(reference.clone()).expect("PR"),
        );
        assert!(pool.run_until(session.guide()).is_ok());
        assert_eq!(server.calls("pullRequests.reviewGuide"), [reference]);
        assert!(!session.can_write());
        assert!(
            pool.run_until(session.run_action(PullRequestAction::Close, None))
                .is_err()
        );
        assert!(server.calls("pullRequests.runAction").is_empty());
    }

    #[test]
    fn pickers_filter_options_stack_actions_and_merge_method_persist() {
        let mut pool = LocalPool::new();
        let worker = pool.spawner();
        let spawn: Spawner = Rc::new(move |f| worker.spawn_local(f).expect("spawn"));
        let server = FakeEnvironment::new("forge", spawn.clone());
        server.set_permissions(&["orchestration:read", "source-control:write"]);
        for tag in [
            "pullRequests.filterOptions",
            "pullRequests.reviewerCandidates",
            "pullRequests.requestReviewers",
            "pullRequests.labelCandidates",
            "pullRequests.setLabels",
            "pullRequests.runAction",
            "vcs.listRefs",
        ] {
            server.register(tag, CallKind::Unary);
        }
        server.reply(
            "pullRequests.filterOptions",
            Ok(json!({"labels":[],"authors":[{"login":"octocat","name":null,"avatarUrl":null}],"references":[],"truncated":false})),
        );
        server.reply(
            "pullRequests.reviewerCandidates",
            Ok(json!({"candidates":[{"id":"u1","kind":"user","login":"carol","name":"Carol","avatarUrl":null,"isRequested":false}],"truncated":false})),
        );
        server.reply("pullRequests.requestReviewers", Ok(Value::Null));
        server.reply(
            "pullRequests.labelCandidates",
            Ok(json!({"candidates":[{"name":"bug","color":"d73a4a","description":"Broken","isApplied":false}],"truncated":false})),
        );
        server.reply("pullRequests.setLabels", Ok(Value::Null));
        server.reply("pullRequests.runAction", Ok(Value::Null));
        server.reply(
            "vcs.listRefs",
            Ok(json!({
                "refs": [{
                    "name": "origin/main",
                    "current": false,
                    "isDefault": true,
                    "isRemote": true,
                    "remoteName": "origin",
                    "worktreePath": null
                }],
                "isRepo": true,
                "hasPrimaryRemote": true,
                "nextCursor": null,
                "totalCount": 1
            })),
        );

        let runtime = ClientRuntime::new(
            server.platform(),
            ClientStore::default(),
            OrchestrationV2CreationSource::Web,
            spawn,
        );
        let environment = pool
            .run_until(runtime.pair(server.pairing()))
            .expect("pair");
        pool.run_until_stalled();

        let opts = pool
            .run_until(runtime.pull_request_filter_options(
                &environment,
                PullRequestFilterOptionsInput {
                    kind: PullRequestFilterOptionKind::Authors,
                    project_id: None,
                    project_ids: None,
                    host: None,
                    query: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                        "octo".into(),
                    ))),
                    sigil: None,
                },
            ))
            .expect("filter options");
        assert_eq!(opts.authors.len(), 1);

        let reference: PullRequestRef = serde_json::from_value(
            json!({"projectId":"project","repository":"team/widget","number":14,"host":"github.com"}),
        )
        .expect("ref");
        let session = runtime.pull_request_review_session(&environment, reference);

        let reviewers = pool
            .run_until(session.reviewer_candidates(None))
            .expect("reviewers");
        assert_eq!(reviewers.candidates[0].login.0, "carol");
        pool.run_until(session.request_reviewers(
            vec![PullRequestReviewerRequestInputReviewers {
                id: t3_contracts::base_schemas::TrimmedNonEmptyString("u1".into()),
                kind: PullRequestReviewerKind::User,
            }],
            true,
        ))
        .expect("request reviewers");

        let labels = pool.run_until(session.label_candidates()).expect("labels");
        assert_eq!(labels.candidates[0].name.0, "bug");
        pool.run_until(session.set_labels(
            vec![t3_contracts::base_schemas::TrimmedNonEmptyString(
                "bug".into(),
            )],
            true,
        ))
        .expect("set labels");

        pool.run_until(session.run_action_with(PullRequestActionOptions {
            action: PullRequestAction::Merge,
            merge_method: Some(PullRequestMergeMethod::Squash),
            update_method: None,
            number_override: Some(15),
            stack_number: Some(42),
            expected_stack_heads: Some(vec![PullRequestStackHead {
                number: 14,
                head_sha: t3_contracts::base_schemas::TrimmedNonEmptyString("sha14".into()),
            }]),
        }))
        .expect("run action");

        let refs = pool
            .run_until(
                session.list_refs(t3_contracts::base_schemas::TrimmedNonEmptyString(
                    "/repo".into(),
                )),
            )
            .expect("list refs");
        assert_eq!(refs.refs.len(), 1);

        pool.run_until(runtime.set_pull_request_merge_method(PullRequestMergeMethod::Rebase))
            .expect("save method");
        assert_eq!(
            pool.run_until(runtime.pull_request_merge_method())
                .expect("read method"),
            PullRequestMergeMethod::Rebase
        );

        assert_eq!(
            server.calls("pullRequests.runAction"),
            [json!({
                "projectId": "project",
                "host": "github.com",
                "repository": "team/widget",
                "number": 15,
                "action": "merge",
                "mergeMethod": "squash",
                "stackNumber": 42,
                "expectedStackHeads": [{"number": 14, "headSha": "sha14"}],
                "replyWithSummary": true
            })]
        );
    }
}
