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
