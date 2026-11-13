use super::super::pull_requests::*;
use crate::{
    ClientError,
    platform::Platform,
    rpc::{RpcCallError, RpcEndpoint, RpcMethod},
    types::{EnvironmentId, Generation},
};
use futures::{
    FutureExt, StreamExt,
    future::{Either, LocalBoxFuture, Shared, WeakShared, select},
    stream,
};
use serde_json::{Value, json};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};
use t3_contracts::{pull_request::*, rpc::*};

pub const METADATA_TIMEOUT_MS: u64 = 2_000;
pub const READ_TIMEOUT_MS: u64 = 30_000;
pub const INVALIDATION_TIMEOUT_MS: u64 = 1_000;

pub fn operation(tag: &str) -> Option<RouteOperation> {
    Some(match tag.strip_prefix("pullRequests.")? {
        "summary" => RouteOperation::Read(QueryKind::Summary),
        "stack" => RouteOperation::Read(QueryKind::Stack),
        "detail" => RouteOperation::Read(QueryKind::Detail),
        "preview" => RouteOperation::Read(QueryKind::Preview),
        "checks" => RouteOperation::Read(QueryKind::Checks),
        "activity" => RouteOperation::Read(QueryKind::Activity),
        "threadComments" => RouteOperation::Read(QueryKind::ThreadComments),
        "diffFileContents" => RouteOperation::Read(QueryKind::DiffFileContents),
        "filesViewed" => RouteOperation::Read(QueryKind::FilesViewed),
        "changeBreakdown" => RouteOperation::Read(QueryKind::ChangeBreakdown),
        "reviewerCandidates" => RouteOperation::Read(QueryKind::ReviewerCandidates),
        "labelCandidates" => RouteOperation::Read(QueryKind::LabelCandidates),
        "setFilesViewed" => RouteOperation::SetFilesViewed,
        "runAction"
        | "update"
        | "comment"
        | "updateComment"
        | "submitReview"
        | "replyToThread"
        | "setReaction"
        | "setThreadResolution"
        | "requestReviewers"
        | "setLabels" => RouteOperation::Write,
        _ => return None,
    })
}

pub trait RoutingRegistry {
    fn environments(&self) -> Vec<RoutingEnvironment>;
    fn generation(&self, environment: &EnvironmentId) -> Option<Generation>;
    fn routing_state(&self) -> Rc<RoutingState>;
    fn route_binding_current<'a>(
        &'a self,
        origin: &'a EnvironmentId,
        destination: &'a EnvironmentId,
    ) -> LocalBoxFuture<'a, bool>;
    fn authorize(
        &self,
        environment: &EnvironmentId,
        tag: &str,
        input: &Value,
    ) -> Result<(), RpcCallError>;
    fn request(
        &self,
        environment: EnvironmentId,
        tag: String,
        input: Value,
    ) -> LocalBoxFuture<'_, Result<Value, RpcCallError>>;
}

/// Uses the same guarded endpoint as ordinary client commands.
pub async fn endpoint_request<E: RpcEndpoint>(
    endpoint: &E,
    tag: &str,
    input: Value,
) -> Result<Value, RpcCallError> {
    async fn call<E: RpcEndpoint, M: RpcMethod>(
        endpoint: &E,
        input: Value,
    ) -> Result<Value, RpcCallError>
    where
        M::Input: serde::de::DeserializeOwned,
        M::Output: serde::Serialize + 'static,
    {
        let input = serde_json::from_value(input).map_err(|_| RpcCallError::InvalidResponse)?;
        serde_json::to_value(endpoint.command::<M>(input).await?)
            .map_err(|_| RpcCallError::InvalidResponse)
    }
    macro_rules! dispatch {
        ($($method:ty),* $(,)?) => { match tag {
            $(<$method as RpcMethod>::NAME => call::<E, $method>(endpoint, input).await,)*
            _ => Err(RpcCallError::NotSent(ClientError::Unsupported("pull request RPC"))),
        }};
    }
    dispatch!(
        WsPullRequestsListRpc,
        WsPullRequestsListStatsRpc,
        WsPullRequestsFilterOptionsRpc,
        WsPullRequestsRoutingRpc,
        WsPullRequestsRoutingIdentityRpc,
        WsPullRequestsSummaryRpc,
        WsPullRequestsStackRpc,
        WsPullRequestsLinkedThreadsRpc,
        WsPullRequestsDetailRpc,
        WsPullRequestsPreviewRpc,
        WsPullRequestsChecksRpc,
        WsPullRequestsActivityRpc,
        WsPullRequestsReviewGuideRpc,
        WsPullRequestsGenerateReviewGuideRpc,
        WsPullRequestsThreadCommentsRpc,
        WsPullRequestsDiffFileContentsRpc,
        WsPullRequestsFilesViewedRpc,
        WsPullRequestsChangeBreakdownRpc,
        WsPullRequestsSetFilesViewedRpc,
        WsPullRequestsRunActionRpc,
        WsPullRequestsUpdateRpc,
        WsPullRequestsCommentRpc,
        WsPullRequestsUpdateCommentRpc,
        WsPullRequestsSubmitReviewRpc,
        WsPullRequestsReplyToThreadRpc,
        WsPullRequestsSetThreadResolutionRpc,
        WsPullRequestsSetReactionRpc,
        WsPullRequestsInvalidateRpc,
        WsPullRequestsReportStateRpc,
        WsPullRequestsReviewerCandidatesRpc,
        WsPullRequestsRequestReviewersRpc,
        WsPullRequestsLabelCandidatesRpc,
        WsPullRequestsSetLabelsRpc
    )
}

type Probe = Shared<LocalBoxFuture<'static, Result<Option<Value>, ()>>>;
type WeakProbe = WeakShared<LocalBoxFuture<'static, Result<Option<Value>, ()>>>;
type ProbeKey = (EnvironmentId, u64, String, String);

#[derive(Default)]
pub struct RoutingState {
    pending: RefCell<BTreeMap<ProbeKey, WeakProbe>>,
    reads: RefCell<RoutedReads>,
    pub queries: Rc<RefCell<super::queries::QueryRegistry>>,
}

pub struct PullRequestRouter<P, R> {
    platform: Rc<P>,
    registry: Rc<R>,
    state: Rc<RoutingState>,
    reports: RefCell<Vec<LocalBoxFuture<'static, ()>>>,
}

impl<P: Platform + 'static, R: RoutingRegistry + 'static> PullRequestRouter<P, R> {
    pub fn new(platform: Rc<P>, registry: Rc<R>) -> Self {
        let state = registry.routing_state();
        Self {
            platform,
            registry,
            state,
            reports: RefCell::new(Vec::new()),
        }
    }

    pub fn take_background(&self) -> Vec<LocalBoxFuture<'static, ()>> {
        std::mem::take(&mut *self.reports.borrow_mut())
    }

    fn catalog_allowed(
        &self,
        origin: &EnvironmentId,
        destination: &EnvironmentId,
        op: RouteOperation,
    ) -> bool {
        let entries = self.registry.environments();
        let source = entries.iter().find(|e| &e.environment == origin);
        let target = entries.iter().find(|e| &e.environment == destination);
        source
            .zip(target)
            .is_some_and(|(a, b)| routing_allowed(a, b, op))
    }
    async fn allowed(
        &self,
        origin: &EnvironmentId,
        destination: &EnvironmentId,
        op: RouteOperation,
    ) -> bool {
        let source = self.registry.generation(origin);
        let target = self.registry.generation(destination);
        self.catalog_allowed(origin, destination, op)
            && self
                .registry
                .route_binding_current(origin, destination)
                .await
            && source.is_some()
            && source == self.registry.generation(origin)
            && target.is_some()
            && target == self.registry.generation(destination)
            && self.catalog_allowed(origin, destination, op)
    }

    async fn send(
        &self,
        environment: &EnvironmentId,
        tag: &str,
        input: Value,
        timeout: Option<u64>,
    ) -> Result<Value, RpcCallError> {
        let generation = self.registry.generation(environment);
        let request = self
            .registry
            .request(environment.clone(), tag.into(), input);
        let result = if let Some(timeout) = timeout {
            match select(request, self.platform.sleep(timeout)).await {
                Either::Left((result, _)) => result,
                Either::Right(_) => Err(RpcCallError::Unknown),
            }
        } else {
            request.await
        };
        if generation != self.registry.generation(environment) {
            return Err(RpcCallError::Interrupted);
        }
        result
    }

    async fn probe(
        &self,
        environment: &EnvironmentId,
        tag: &str,
        input: Value,
        key: String,
        share: bool,
    ) -> Result<Option<Value>, RpcCallError> {
        let Some(generation) = self.registry.generation(environment) else {
            return Ok(None);
        };
        let cache_key = (environment.clone(), generation.0, tag.into(), key);
        let held = if share {
            self.state
                .pending
                .borrow()
                .get(&cache_key)
                .and_then(WeakShared::upgrade)
        } else {
            None
        };
        let future: Probe = held.unwrap_or_else(|| {
            let platform = self.platform.clone();
            let registry = self.registry.clone();
            let environment = environment.clone();
            let tag = tag.to_owned();
            async move {
                let request = registry.request(environment.clone(), tag, input);
                let result = match select(request, platform.sleep(METADATA_TIMEOUT_MS)).await {
                    Either::Left((Ok(value), _)) => Ok(Some(value)),
                    Either::Left((Err(RpcCallError::Interrupted), _)) => Err(()),
                    _ => Ok(None),
                };
                if registry.generation(&environment) != Some(generation) {
                    return Err(());
                }
                result
            }
            .boxed_local()
            .shared()
        });
        if share {
            let mut pending = self.state.pending.borrow_mut();
            pending.retain(|_, value| value.upgrade().is_some());
            if let Some(weak) = future.downgrade() {
                pending.insert(cache_key.clone(), weak);
            }
        }
        let result = future.await;
        if share {
            self.state.pending.borrow_mut().remove(&cache_key);
        }
        result.map_err(|()| RpcCallError::Interrupted)
    }

    pub async fn request<M: RpcMethod>(
        &self,
        origin: &EnvironmentId,
        input: M::Input,
    ) -> Result<M::Output, RpcCallError> {
        let input = serde_json::to_value(input).map_err(|_| RpcCallError::InvalidResponse)?;
        let value = self.request_value(origin, M::NAME, input).await?;
        serde_json::from_value(value).map_err(|_| RpcCallError::InvalidResponse)
    }

    pub async fn request_value(
        &self,
        origin: &EnvironmentId,
        tag: &str,
        input: Value,
    ) -> Result<Value, RpcCallError> {
        self.registry.authorize(origin, tag, &input)?;
        let generation = self.registry.generation(origin);
        let result = self.request_inner(origin, tag, input).await;
        if self.registry.generation(origin) != generation {
            return Err(RpcCallError::Interrupted);
        }
        result
    }

    async fn request_inner(
        &self,
        origin: &EnvironmentId,
        tag: &str,
        input: Value,
    ) -> Result<Value, RpcCallError> {
        let generation = self.registry.generation(origin);
        if tag == "pullRequests.invalidate" {
            let result = self.send(origin, tag, input.clone(), None).await?;
            if let Ok(invalidation) =
                serde_json::from_value::<PullRequestInvalidateInput>(input.clone())
            {
                self.invalidate_explicit(origin, invalidation, input).await;
            }
            return Ok(result);
        }
        let Some(op) = operation(tag) else {
            return self.send(origin, tag, input, None).await;
        };
        let Ok(reference) = serde_json::from_value::<PullRequestRef>(input.clone()) else {
            return self.send(origin, tag, input, None).await;
        };
        let entries = self.registry.environments();
        let Some(source) = entries.iter().find(|e| &e.environment == origin) else {
            return self.send(origin, tag, input, None).await;
        };
        let read = matches!(op, RouteOperation::Read(_));
        let mut allowed = false;
        for entry in &entries {
            if &entry.environment != origin && self.allowed(origin, &entry.environment, op).await {
                allowed = true;
                break;
            }
        }
        if generation != self.registry.generation(origin) {
            return Err(RpcCallError::Interrupted);
        }
        if !allowed {
            return self.finish_direct(origin, tag, input, &reference, op).await;
        }
        let mut strict = input.clone();
        if read {
            strict["allowStale"] = json!(false);
        }
        let mut source_result = None;
        if read && source.local {
            let result = self
                .send(origin, tag, strict.clone(), Some(READ_TIMEOUT_MS))
                .await;
            if result.is_ok() || matches!(result, Err(RpcCallError::Interrupted)) {
                return result;
            }
            source_result = Some(result);
        }
        let result = self
            .routed(
                origin,
                tag,
                strict,
                &reference,
                op,
                source,
                &entries,
                source_result,
            )
            .await;
        if read
            && result.is_err()
            && !matches!(result, Err(RpcCallError::Interrupted))
            && reference.allow_stale != Some(Some(false))
            && matches!(tag, "pullRequests.summary" | "pullRequests.detail")
        {
            return self.send(origin, tag, input, None).await;
        }
        result
    }

    async fn finish_direct(
        &self,
        origin: &EnvironmentId,
        tag: &str,
        input: Value,
        reference: &PullRequestRef,
        op: RouteOperation,
    ) -> Result<Value, RpcCallError> {
        let result = self.send(origin, tag, input, None).await;
        if op == RouteOperation::SetFilesViewed || (op == RouteOperation::Write && result.is_ok()) {
            self.invalidate_readers(origin, reference, op).await;
        }
        result
    }

    async fn source_fallback(
        &self,
        origin: &EnvironmentId,
        tag: &str,
        input: Value,
        reference: &PullRequestRef,
        op: RouteOperation,
    ) -> Result<Value, RpcCallError> {
        if matches!(op, RouteOperation::Read(_)) {
            self.send(origin, tag, input, Some(READ_TIMEOUT_MS)).await
        } else {
            self.finish_direct(origin, tag, input, reference, op).await
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn routed(
        &self,
        origin: &EnvironmentId,
        tag: &str,
        input: Value,
        reference: &PullRequestRef,
        op: RouteOperation,
        source: &RoutingEnvironment,
        entries: &[RoutingEnvironment],
        mut source_result: Option<Result<Value, RpcCallError>>,
    ) -> Result<Value, RpcCallError> {
        let generation = self.registry.generation(origin);
        let read = matches!(op, RouteOperation::Read(_));
        let candidates = route_candidates(source, entries, op);
        if candidates == [origin.clone()] {
            return if let Some(result) = source_result {
                result
            } else {
                self.source_fallback(origin, tag, input, reference, op)
                    .await
            };
        }
        let key = serde_json::to_string(&(
            reference.project_id.clone(),
            reference
                .host
                .as_ref()
                .and_then(Option::as_ref)
                .map(|s| s.0.to_lowercase()),
            reference.repository.0.to_lowercase(),
        ))
        .map_err(|_| RpcCallError::InvalidResponse)?;
        let metadata = self
            .probe(
                origin,
                "pullRequests.routing",
                serde_json::to_value(reference).map_err(|_| RpcCallError::InvalidResponse)?,
                key,
                read,
            )
            .await?;
        if self.registry.generation(origin) != generation {
            return Err(RpcCallError::Interrupted);
        }
        let Some(identity) = metadata
            .and_then(|value| serde_json::from_value::<PullRequestRoutingResult>(value).ok())
        else {
            return if let Some(result) = source_result {
                result
            } else {
                self.source_fallback(origin, tag, input, reference, op)
                    .await
            };
        };
        let mut routed_input = input;
        routed_input["host"] = json!(identity.host);
        routed_input["expectedAccountId"] = json!(identity.account_id);
        let mut last = None;
        for (index, destination) in candidates.iter().enumerate() {
            if destination != origin {
                if !self.allowed(origin, destination, op).await {
                    continue;
                }
                let alternate = self
                    .probe(
                        destination,
                        "pullRequests.routingIdentity",
                        json!({"host": identity.host}),
                        identity.host.0.to_lowercase(),
                        read,
                    )
                    .await?;
                if self.registry.generation(origin) != generation {
                    return Err(RpcCallError::Interrupted);
                }
                let verified = alternate.and_then(|v| {
                    serde_json::from_value::<PullRequestRoutingIdentityResult>(v).ok()
                });
                if !verified
                    .as_ref()
                    .is_some_and(|v| routing_identity_matches(&identity, v))
                    || !self.allowed(origin, destination, op).await
                {
                    continue;
                }
            }
            if generation != self.registry.generation(origin) {
                return Err(RpcCallError::Interrupted);
            }
            let result = if destination == origin && source_result.is_some() {
                source_result.take().ok_or(RpcCallError::Defect)?
            } else {
                self.send(
                    destination,
                    tag,
                    routed_input.clone(),
                    read.then_some(READ_TIMEOUT_MS),
                )
                .await
            };
            match result {
                Ok(mut value) => {
                    let mut recorded = reference.clone();
                    recorded.host = Some(Some(identity.host.clone()));
                    self.state.reads.borrow_mut().record_reference(
                        origin.clone(),
                        recorded,
                        destination.clone(),
                    );
                    if destination != origin && value.get("state").is_some() {
                        self.report_state(origin.clone(), reference, &value);
                    }
                    if value.get("projectId").is_some() {
                        value["projectId"] = json!(reference.project_id);
                        if tag == "pullRequests.detail" {
                            value["projectTitle"] = json!(identity.project_title);
                            value["workspaceRoot"] = json!(identity.workspace_root);
                        }
                    }
                    if !read {
                        self.invalidate_readers(origin, reference, op).await;
                    }
                    return Ok(value);
                }
                Err(error) => {
                    let retry =
                        can_retry_route(op, failure(&error)) && index + 1 < candidates.len();
                    last = Some(error);
                    if !retry {
                        break;
                    }
                }
            }
        }
        if op == RouteOperation::SetFilesViewed {
            self.invalidate_readers(origin, reference, op).await;
        }
        if let Some(error) = last {
            Err(error)
        } else {
            self.source_fallback(origin, tag, routed_input, reference, op)
                .await
        }
    }

    fn report_state(&self, origin: EnvironmentId, reference: &PullRequestRef, value: &Value) {
        let mut reference = reference.clone();
        reference.allow_stale = None;
        reference.expected_account_id = None;
        let mut input = json!({"reference": reference, "state": value["state"]});
        if let Some(draft) = value.get("isDraft") {
            input["isDraft"] = draft.clone();
        }
        let registry = self.registry.clone();
        let platform = self.platform.clone();
        let generation = registry.generation(&origin);
        self.reports.borrow_mut().push(
            async move {
                if registry.generation(&origin) != generation {
                    return;
                }
                let _ = select(
                    registry.request(origin, "pullRequests.reportState".into(), input),
                    platform.sleep(5_000),
                )
                .await;
            }
            .boxed_local(),
        );
    }

    async fn invalidate_explicit(
        &self,
        origin: &EnvironmentId,
        input: PullRequestInvalidateInput,
        raw: Value,
    ) {
        let filter = input
            .reference
            .as_ref()
            .and_then(Option::as_ref)
            .map(|r| PullRequestKey::new(origin.clone(), r));
        let mut jobs = BTreeMap::new();
        {
            let reads = self.state.reads.borrow();
            for (key, targets) in &reads.entries {
                if &key.project.environment != origin
                    || filter
                        .as_ref()
                        .is_some_and(|filter| !key.matches_invalidation(filter))
                {
                    continue;
                }
                let mut value = raw.clone();
                if filter.is_some() {
                    value["reference"] = json!(reads.reference(key));
                }
                for target in targets {
                    jobs.insert(target.clone(), vec![value.clone()]);
                }
                if filter.is_some() {
                    jobs.insert(origin.clone(), vec![value]);
                }
            }
        }
        self.invalidate_targets(origin, jobs.into_iter().collect())
            .await;
    }

    async fn invalidate_readers(
        &self,
        origin: &EnvironmentId,
        reference: &PullRequestRef,
        op: RouteOperation,
    ) {
        let filter = PullRequestKey::new(origin.clone(), reference);
        let mut by_target = BTreeMap::<EnvironmentId, Vec<Value>>::new();
        by_target.insert(origin.clone(), vec![json!(reference)]);
        {
            let reads = self.state.reads.borrow();
            if reads.entries.is_empty() {
                return;
            }
            for (key, targets) in &reads.entries {
                if !key.matches_invalidation(&filter) {
                    continue;
                }
                let recorded = reads.reference(key);
                for target in targets.iter().chain(std::iter::once(origin)) {
                    let refs = by_target.entry(target.clone()).or_default();
                    let value = json!(recorded);
                    if !refs.iter().any(|v| v.get("host") == value.get("host")) {
                        refs.push(value);
                    }
                }
            }
        }
        let jobs = by_target
            .into_iter()
            .map(|(target, refs)| {
                let mut inputs: Vec<_> = refs
                    .into_iter()
                    .map(|reference| {
                        if op == RouteOperation::SetFilesViewed {
                            json!({"reference": reference, "filesViewedOnly": true})
                        } else {
                            json!({"reference": reference})
                        }
                    })
                    .collect();
                if op != RouteOperation::SetFilesViewed {
                    inputs.push(json!({}));
                }
                (target, inputs)
            })
            .collect();
        self.invalidate_targets(origin, jobs).await;
    }

    async fn invalidate_targets(
        &self,
        origin: &EnvironmentId,
        jobs: Vec<(EnvironmentId, Vec<Value>)>,
    ) {
        stream::iter(jobs)
            .for_each_concurrent(4, |(target, inputs)| async move {
                if &target != origin
                    && !self
                        .allowed(origin, &target, RouteOperation::Read(QueryKind::Summary))
                        .await
                {
                    return;
                }
                stream::iter(inputs)
                    .for_each_concurrent(3, |input| {
                        let target = &target;
                        async move {
                            let _ = self
                                .send(
                                    target,
                                    "pullRequests.invalidate",
                                    input,
                                    Some(INVALIDATION_TIMEOUT_MS),
                                )
                                .await;
                        }
                    })
                    .await;
            })
            .await;
    }

    pub fn remove_environment(&self, environment: &EnvironmentId) {
        self.state
            .reads
            .borrow_mut()
            .remove_environment(environment);
        self.state
            .pending
            .borrow_mut()
            .retain(|(env, _, _, _), _| env != environment);
    }
}

fn failure(error: &RpcCallError) -> RouteFailure {
    match error {
        RpcCallError::Interrupted | RpcCallError::NotSent(ClientError::Cancelled) => {
            RouteFailure::Interrupted
        }
        RpcCallError::NotSent(ClientError::Disconnected) => RouteFailure::UnavailableBeforeDispatch,
        RpcCallError::Rejected(errors)
            if errors
                .iter()
                .any(|e| e["_tag"] == "EnvironmentRpcUnavailableError") =>
        {
            RouteFailure::UnavailableBeforeDispatch
        }
        RpcCallError::Rejected(errors)
            if errors
                .iter()
                .any(|e| e["_tag"] == "EnvironmentNotRegisteredError") =>
        {
            RouteFailure::UnregisteredBeforeDispatch
        }
        RpcCallError::Rejected(errors)
            if errors.iter().any(|e| {
                e["_tag"] == "PullRequestOperationError" && e["operation"] == "routeIdentity"
            }) =>
        {
            RouteFailure::IdentityRejectedBeforeDispatch
        }
        _ => RouteFailure::AfterDispatch,
    }
}
