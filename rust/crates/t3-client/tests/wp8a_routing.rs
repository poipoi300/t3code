#![allow(clippy::unwrap_used)]

use futures::{
    FutureExt,
    channel::oneshot,
    executor::block_on,
    future::{LocalBoxFuture, pending},
};
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, BTreeSet, VecDeque},
    future::Future,
    pin::Pin,
    rc::Rc,
    task::{Context, Poll},
};
use t3_client::{
    rpc::RpcCallError,
    testing::FakePlatform,
    types::{EnvironmentId, Generation},
    workspace::{pr_views::routing::*, pull_requests::*},
};

fn env(name: &str) -> EnvironmentId {
    EnvironmentId(name.into())
}
fn entry(name: &str, local: bool) -> RoutingEnvironment {
    RoutingEnvironment {
        environment: env(name),
        enabled: true,
        supported: true,
        connected: true,
        local,
        checks_supported: true,
        connection_key: Some(name.into()),
        granted_connection_key: Some(name.into()),
        permission: RoutingPermission::ReadWrite,
    }
}
fn reference() -> Value {
    json!({"projectId":"project","repository":"acme/web","number":7})
}
fn identity() -> Value {
    json!({"provider":"github","host":"github.com","accountId":"123","viewer":"maria","projectTitle":"Source project","workspaceRoot":"/source"})
}
fn poll<F: Future>(future: &mut Pin<Box<F>>) -> Poll<F::Output> {
    future
        .as_mut()
        .poll(&mut Context::from_waker(futures::task::noop_waker_ref()))
}
type Handler = dyn Fn(&Registry, &EnvironmentId, &str, &Value) -> Result<Value, RpcCallError>;
type Reply = LocalBoxFuture<'static, Result<Value, RpcCallError>>;
struct Registry {
    entries: RefCell<Vec<RoutingEnvironment>>,
    generations: RefCell<BTreeMap<EnvironmentId, Generation>>,
    calls: RefCell<Vec<(EnvironmentId, String, Value)>>,
    handler: Box<Handler>,
    stall: RefCell<Option<String>>,
    state: Rc<RoutingState>,
    replies: RefCell<BTreeMap<String, VecDeque<Reply>>>,
    denied: RefCell<BTreeSet<EnvironmentId>>,
    bindings_current: Cell<bool>,
}
impl Registry {
    fn new(
        entries: Vec<RoutingEnvironment>,
        handler: impl Fn(&Registry, &EnvironmentId, &str, &Value) -> Result<Value, RpcCallError>
        + 'static,
    ) -> Rc<Self> {
        Rc::new(Self {
            generations: RefCell::new(
                entries
                    .iter()
                    .map(|e| (e.environment.clone(), Generation(1)))
                    .collect(),
            ),
            entries: RefCell::new(entries),
            calls: RefCell::new(Vec::new()),
            handler: Box::new(handler),
            stall: RefCell::new(None),
            state: Rc::default(),
            replies: RefCell::new(BTreeMap::new()),
            denied: RefCell::new(BTreeSet::new()),
            bindings_current: Cell::new(true),
        })
    }
    fn tags(&self) -> Vec<String> {
        self.calls
            .borrow()
            .iter()
            .map(|(env, tag, _)| format!("{}:{tag}", env.0))
            .collect()
    }
    fn check_permission(&self, environment: &EnvironmentId, tag: &str) -> Result<(), RpcCallError> {
        if tag == "pullRequests.runAction" && self.denied.borrow().contains(environment) {
            Err(RpcCallError::NotSent(
                t3_client::ClientError::PermissionDenied { method: tag.into() },
            ))
        } else {
            Ok(())
        }
    }
    fn gate(&self, tag: &str) -> oneshot::Sender<Result<Value, RpcCallError>> {
        let (tx, rx) = oneshot::channel();
        self.replies
            .borrow_mut()
            .entry(tag.into())
            .or_default()
            .push_back(async { rx.await.unwrap_or(Err(RpcCallError::Interrupted)) }.boxed_local());
        tx
    }
}
impl RoutingRegistry for Registry {
    fn environments(&self) -> Vec<RoutingEnvironment> {
        self.entries.borrow().clone()
    }
    fn generation(&self, env: &EnvironmentId) -> Option<Generation> {
        self.generations.borrow().get(env).copied()
    }
    fn routing_state(&self) -> Rc<RoutingState> {
        self.state.clone()
    }
    fn route_binding_current<'a>(
        &'a self,
        _origin: &'a EnvironmentId,
        _destination: &'a EnvironmentId,
    ) -> LocalBoxFuture<'a, bool> {
        async { self.bindings_current.get() }.boxed_local()
    }
    fn authorize(
        &self,
        environment: &EnvironmentId,
        tag: &str,
        _input: &Value,
    ) -> Result<(), RpcCallError> {
        self.check_permission(environment, tag)
    }
    fn request(
        &self,
        environment: EnvironmentId,
        tag: String,
        input: Value,
    ) -> LocalBoxFuture<'_, Result<Value, RpcCallError>> {
        async move {
            self.check_permission(&environment, &tag)?;
            self.calls
                .borrow_mut()
                .push((environment.clone(), tag.clone(), input.clone()));
            let reply = self
                .replies
                .borrow_mut()
                .get_mut(&tag)
                .and_then(VecDeque::pop_front);
            if let Some(reply) = reply {
                return reply.await;
            }
            if self.stall.borrow().as_ref() == Some(&tag) {
                return pending().await;
            }
            (self.handler)(self, &environment, &tag, &input)
        }
        .boxed_local()
    }
}

#[test]
fn service_serializes_writes_and_continues_after_a_refused_close() {
    use t3_client::workspace::pr_views::service::PullRequestService;
    let registry = Registry::new(vec![entry("origin", true)], |_, _, tag, input| {
        if tag == "pullRequests.runAction" && input["number"] == 2 {
            return Err(RpcCallError::Rejected(vec![
                json!({"_tag":"MutationRefused"}),
            ]));
        }
        Ok(Value::Null)
    });
    let gate = registry.gate("pullRequests.runAction");
    let service = PullRequestService::new(Rc::new(FakePlatform::default()), registry.clone());
    let mut input = reference();
    input["action"] = json!("close");
    let mut second = input.clone();
    second["number"] = json!(2);
    let mut third = input.clone();
    third["number"] = json!(3);
    let mut first =
        Box::pin(service.command(env("origin"), "pullRequests.runAction".into(), input, None));
    let mut second =
        Box::pin(service.command(env("origin"), "pullRequests.runAction".into(), second, None));
    let mut third =
        Box::pin(service.command(env("origin"), "pullRequests.runAction".into(), third, None));
    assert!(poll(&mut first).is_pending());
    assert!(poll(&mut second).is_pending());
    assert!(poll(&mut third).is_pending());
    assert_eq!(registry.calls.borrow().len(), 1);
    gate.send(Ok(Value::Null)).unwrap();
    assert!(block_on(first).is_ok());
    assert!(block_on(second).is_err());
    assert!(block_on(third).is_ok());
    assert_eq!(
        registry
            .calls
            .borrow()
            .iter()
            .map(|(_, _, input)| input["number"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        vec![7, 2, 3]
    );
    assert!(
        registry
            .calls
            .borrow()
            .iter()
            .all(|(_, _, input)| input["replyWithSummary"] == true)
    );
}

#[test]
fn service_single_flight_shares_a_read_but_keeps_other_hosts_separate() {
    use t3_client::workspace::pr_views::service::PullRequestService;
    let registry = Registry::new(vec![entry("origin", true)], default_handler);
    let gate = registry.gate("pullRequests.diffFileContents");
    let service = PullRequestService::new(Rc::new(FakePlatform::default()), registry.clone());
    let input = reference();
    let mut alternate = input.clone();
    alternate["host"] = json!("other.test");
    let mut first = Box::pin(service.command(
        env("origin"),
        "pullRequests.diffFileContents".into(),
        input.clone(),
        None,
    ));
    let mut waiter = Box::pin(service.command(
        env("origin"),
        "pullRequests.diffFileContents".into(),
        input,
        None,
    ));
    assert!(poll(&mut first).is_pending());
    assert!(poll(&mut waiter).is_pending());
    assert_eq!(registry.calls.borrow().len(), 1);
    assert!(
        block_on(service.command(
            env("origin"),
            "pullRequests.diffFileContents".into(),
            alternate,
            None
        ))
        .is_ok()
    );
    assert_eq!(registry.calls.borrow().len(), 2);
    gate.send(Ok(json!({"oldContents":"old","newContents":"new"})))
        .unwrap();
    drop(first);
    assert_eq!(block_on(waiter).unwrap()["newContents"], "new");
}

#[test]
fn service_cancels_a_queued_write_from_an_old_session_and_publishes_guides_before_refresh() {
    use t3_client::workspace::pr_views::service::PullRequestService;
    let registry = Registry::new(vec![entry("origin", true)], default_handler);
    let gate = registry.gate("pullRequests.runAction");
    let service = PullRequestService::new(Rc::new(FakePlatform::default()), registry.clone());
    let mut first = Box::pin(service.command(
        env("origin"),
        "pullRequests.runAction".into(),
        reference(),
        None,
    ));
    let mut queued = Box::pin(service.command(
        env("origin"),
        "pullRequests.comment".into(),
        reference(),
        None,
    ));
    assert!(poll(&mut first).is_pending());
    assert!(poll(&mut queued).is_pending());
    registry
        .generations
        .borrow_mut()
        .insert(env("origin"), Generation(2));
    gate.send(Ok(Value::Null)).unwrap();
    assert!(block_on(first).is_err());
    assert!(block_on(queued).is_err());
    assert_eq!(registry.calls.borrow().len(), 1);
    let key = service
        .queries
        .borrow_mut()
        .register(env("origin"), QueryKind::ReviewGuide, &reference())
        .unwrap();
    let ticket = service
        .queries
        .borrow_mut()
        .begin(&key, Generation(2))
        .unwrap();
    service
        .queries
        .borrow_mut()
        .finish(&key, ticket, Ok(json!({"run":null})), 0)
        .unwrap();
    let guide =
        json!({"run":{"_tag":"Running","headSha":"head","startedAt":"2026-07-01T00:00:00Z"}});
    service
        .queries
        .borrow_mut()
        .command_succeeded(
            &env("origin"),
            "pullRequests.generateReviewGuide",
            &reference(),
            &guide,
            Generation(2),
        )
        .unwrap();
    assert_eq!(
        service
            .queries
            .borrow()
            .get(&key)
            .unwrap()
            .state
            .value
            .as_ref()
            .unwrap()["run"]["_tag"],
        "Running"
    );
    assert!(
        service
            .queries
            .borrow()
            .get(&key)
            .unwrap()
            .refresh_requested
    );
    let other_factory = PullRequestService::new(Rc::new(FakePlatform::default()), registry);
    assert!(Rc::ptr_eq(&service.queries, &other_factory.queries));
}
fn default_handler(
    _: &Registry,
    _: &EnvironmentId,
    tag: &str,
    _: &Value,
) -> Result<Value, RpcCallError> {
    Ok(
        if matches!(tag, "pullRequests.routing" | "pullRequests.routingIdentity") {
            identity()
        } else {
            Value::Null
        },
    )
}

#[test]
fn routing_scenarios_preserve_source_order_identity_guards_and_ambiguous_writes() {
    for scenario in [
        "local-alternate",
        "mismatch",
        "ambiguous",
        "fast-local-read",
        "single",
        "local-origin",
        "old-origin",
        "old-alternate",
        "revoked",
        "disabled",
        "disabled-during-probe",
        "switched-source",
        "changed-profile",
    ] {
        let scenario = scenario.to_owned();
        let case = scenario.clone();
        let local_origin = matches!(
            scenario.as_str(),
            "fast-local-read" | "local-origin" | "switched-source"
        );
        let mut entries = vec![entry("origin", local_origin)];
        if scenario != "single" {
            entries.push(entry("alternate", true));
        }
        if scenario == "disabled" {
            entries[1].enabled = false;
        }
        let registry = Registry::new(entries, move |registry, env, tag, input| {
            let alternate = env.0 == "alternate";
            if matches!(tag, "pullRequests.routing" | "pullRequests.routingIdentity") {
                if alternate && case == "changed-profile" {
                    registry.bindings_current.set(false);
                }
                if alternate && matches!(case.as_str(), "revoked" | "disabled-during-probe") {
                    let mut entries = registry.entries.borrow_mut();
                    if case == "revoked" {
                        entries[1].permission = RoutingPermission::Off;
                    } else {
                        entries[1].enabled = false;
                    }
                }
                if !alternate && case == "old-origin" || alternate && case == "old-alternate" {
                    return Err(RpcCallError::Defect);
                }
                let mut value = identity();
                if alternate && matches!(case.as_str(), "mismatch" | "switched-source") {
                    value["accountId"] = json!("456");
                }
                return Ok(value);
            }
            if tag == "pullRequests.runAction" && alternate && case == "ambiguous" {
                return Err(RpcCallError::Unknown);
            }
            if tag == "pullRequests.runAction"
                && !alternate
                && case == "switched-source"
                && input.get("expectedAccountId").is_some()
            {
                return Err(RpcCallError::Rejected(vec![
                    json!({"_tag":"PullRequestOperationError","operation":"routeIdentity"}),
                ]));
            }
            Ok(Value::Null)
        });
        let router = PullRequestRouter::new(Rc::new(FakePlatform::default()), registry.clone());
        let reading = scenario == "fast-local-read";
        let tag = if reading {
            "pullRequests.summary"
        } else {
            "pullRequests.runAction"
        };
        let result = block_on(router.request_value(&env("origin"), tag, reference()));
        assert_eq!(
            result.is_err(),
            matches!(scenario.as_str(), "ambiguous" | "switched-source"),
            "{scenario}: {result:?}"
        );
        let writes: Vec<_> = registry
            .calls
            .borrow()
            .iter()
            .filter(|(_, t, _)| t == tag)
            .map(|(e, _, _)| e.0.clone())
            .collect();
        assert_eq!(
            writes,
            vec![
                if matches!(scenario.as_str(), "local-alternate" | "ambiguous") {
                    "alternate"
                } else {
                    "origin"
                }
            ],
            "{scenario}"
        );
        if reading || scenario == "single" || scenario == "disabled" {
            assert_eq!(registry.tags().len(), 1, "{scenario}");
        }
    }
}

#[test]
fn routed_details_keep_source_metadata_and_report_state_without_account_or_stale_flags() {
    for draft in [None, Some(true), Some(false)] {
        let registry = Registry::new(
            vec![entry("origin", false), entry("alternate", true)],
            move |registry, env, tag, input| {
                if tag == "pullRequests.detail" {
                    return Ok(
                        json!({"projectId":"alternate-project","projectTitle":"Alternate","workspaceRoot":"/alternate","state":"closed","isDraft":draft}),
                    );
                }
                default_handler(registry, env, tag, input)
            },
        );
        let router = PullRequestRouter::new(Rc::new(FakePlatform::default()), registry.clone());
        let result =
            block_on(router.request_value(&env("origin"), "pullRequests.detail", reference()))
                .unwrap();
        assert_eq!(result["projectId"], "project");
        assert_eq!(result["projectTitle"], "Source project");
        assert_eq!(result["workspaceRoot"], "/source");
        for background in router.take_background() {
            block_on(background);
        }
        let calls = registry.calls.borrow();
        let report = &calls
            .iter()
            .find(|(_, tag, _)| tag == "pullRequests.reportState")
            .unwrap()
            .2;
        assert_eq!(report["reference"], reference());
        assert_eq!(report["state"], "closed");
    }
}

#[test]
fn failed_viewed_batches_invalidate_prior_readers_across_router_instances() {
    let registry = Registry::new(
        vec![entry("origin", false), entry("alternate", true)],
        |registry, env, tag, input| {
            if tag == "pullRequests.setFilesViewed" {
                return Err(RpcCallError::Unknown);
            }
            default_handler(registry, env, tag, input)
        },
    );
    let platform = Rc::new(FakePlatform::default());
    block_on(
        PullRequestRouter::new(platform.clone(), registry.clone()).request_value(
            &env("origin"),
            "pullRequests.filesViewed",
            reference(),
        ),
    )
    .unwrap();
    let router = PullRequestRouter::new(platform, registry.clone());
    assert!(
        block_on(router.request_value(&env("origin"), "pullRequests.setFilesViewed", reference()))
            .is_err()
    );
    let calls = registry.calls.borrow();
    for target in ["origin", "alternate"] {
        assert!(calls.iter().any(|(env, tag, input)| env.0 == target
            && tag == "pullRequests.invalidate"
            && input["filesViewedOnly"] == true));
    }
}

#[test]
fn injected_timeout_cancels_metadata_and_late_sessions_never_publish() {
    let registry = Registry::new(
        vec![entry("origin", false), entry("alternate", true)],
        default_handler,
    );
    *registry.stall.borrow_mut() = Some("pullRequests.routing".into());
    let platform = Rc::new(FakePlatform::default());
    let router = PullRequestRouter::new(platform.clone(), registry.clone());
    let origin = env("origin");
    let mut request = Box::pin(router.request_value(&origin, "pullRequests.summary", reference()));
    assert!(matches!(poll(&mut request), Poll::Pending));
    platform.advance(METADATA_TIMEOUT_MS);
    assert!(block_on(request).is_ok());
    assert_eq!(platform.pending_timers(), 0);
    registry
        .generations
        .borrow_mut()
        .insert(env("origin"), Generation(2));
    let origin = env("origin");
    let mut next = Box::pin(router.request_value(&origin, "pullRequests.summary", reference()));
    assert!(matches!(poll(&mut next), Poll::Pending));
    registry
        .generations
        .borrow_mut()
        .insert(env("origin"), Generation(3));
    platform.advance(METADATA_TIMEOUT_MS);
    assert!(matches!(block_on(next), Err(RpcCallError::Interrupted)));
}

#[test]
fn metadata_single_flight_survives_one_reader_being_dropped() {
    let probes = Rc::new(Cell::new(0));
    let counter = probes.clone();
    let registry = Registry::new(
        vec![entry("origin", false), entry("alternate", true)],
        move |registry, env, tag, input| {
            if tag == "pullRequests.routing" {
                counter.set(counter.get() + 1);
            }
            default_handler(registry, env, tag, input)
        },
    );
    *registry.stall.borrow_mut() = Some("pullRequests.routing".into());
    let platform = Rc::new(FakePlatform::default());
    let router = PullRequestRouter::new(platform.clone(), registry.clone());
    let origin = env("origin");
    let mut first = Box::pin(router.request_value(&origin, "pullRequests.summary", reference()));
    let mut waiter = Box::pin(router.request_value(&origin, "pullRequests.summary", reference()));
    assert!(matches!(poll(&mut first), Poll::Pending));
    assert!(matches!(poll(&mut waiter), Poll::Pending));
    assert_eq!(
        registry
            .calls
            .borrow()
            .iter()
            .filter(|(_, tag, _)| tag == "pullRequests.routing")
            .count(),
        1
    );
    drop(first);
    platform.advance(METADATA_TIMEOUT_MS);
    assert!(block_on(waiter).is_ok());
    assert_eq!(platform.pending_timers(), 0);
}

#[test]
fn dropping_a_query_load_releases_its_ticket_for_a_new_subscription() {
    use t3_client::workspace::pr_views::service::PullRequestService;
    let registry = Registry::new(vec![entry("origin", true)], default_handler);
    let _gate = registry.gate("pullRequests.preview");
    let service = PullRequestService::new(Rc::new(FakePlatform::default()), registry);
    let key = service
        .queries
        .borrow_mut()
        .register(env("origin"), QueryKind::Preview, &reference())
        .unwrap();
    let mut load = Box::pin(service.load(&key));
    assert!(poll(&mut load).is_pending());
    assert!(service.queries.borrow().get(&key).unwrap().state.loading);
    drop(load);
    assert!(!service.queries.borrow().get(&key).unwrap().state.loading);
    assert!(
        service
            .queries
            .borrow()
            .get(&key)
            .unwrap()
            .refresh_requested
    );
}

#[test]
fn strict_source_reads_keep_the_deadline_when_an_alternate_cannot_be_used() {
    for missing_identity in [false, true] {
        let mut alternate = entry("alternate", true);
        alternate.connected = missing_identity;
        let registry = Registry::new(
            vec![entry("origin", false), alternate],
            move |registry, env, tag, input| {
                if missing_identity && tag == "pullRequests.routing" {
                    return Err(RpcCallError::Defect);
                }
                default_handler(registry, env, tag, input)
            },
        );
        *registry.stall.borrow_mut() = Some("pullRequests.summary".into());
        let platform = Rc::new(FakePlatform::default());
        let router = PullRequestRouter::new(platform.clone(), registry);
        let origin = env("origin");
        let mut input = reference();
        input["allowStale"] = json!(false);
        let mut request = Box::pin(router.request_value(&origin, "pullRequests.summary", input));
        assert!(poll(&mut request).is_pending());
        platform.advance(READ_TIMEOUT_MS);
        assert!(matches!(poll(&mut request), Poll::Ready(Err(_))));
    }
}

#[test]
fn routed_writes_require_current_origin_and_destination_permissions() {
    use t3_client::workspace::pr_views::service::PullRequestService;
    for denied in ["origin", "alternate"] {
        let registry = Registry::new(
            vec![entry("origin", false), entry("alternate", true)],
            default_handler,
        );
        registry.denied.borrow_mut().insert(env(denied));
        let service = PullRequestService::new(Rc::new(FakePlatform::default()), registry.clone());
        let mut input = reference();
        input["action"] = json!("close");
        let result =
            block_on(service.command(env("origin"), "pullRequests.runAction".into(), input, None));
        if denied == "origin" {
            assert!(result.is_err());
            assert!(registry.calls.borrow().is_empty());
        } else {
            assert!(result.is_err());
            assert!(
                registry
                    .calls
                    .borrow()
                    .iter()
                    .all(|(_, tag, _)| tag != "pullRequests.runAction")
            );
        }
    }
}

#[test]
fn merge_preparation_uses_fresh_details_and_rejects_drafts_stacks_and_missing_permissions() {
    use t3_client::workspace::pr_views::service::{MergeMethodResolver, PullRequestService};
    use t3_contracts::pull_request::PullRequestMergeMethod;
    for scenario in [
        "merge",
        "closed",
        "draft",
        "no-permission",
        "stack",
        "resolver-error",
    ] {
        let registry = Registry::new(vec![entry("origin", true)], move |_, _, tag, input| {
            if tag == "pullRequests.detail" {
                assert_eq!(input["allowStale"], false);
                return Ok(
                    json!({"provider":"github","capabilities":{"diff":true,"comment":true,"actions":["merge"],"mergeMethods":["squash"],"search":true,"review":{"inlineComment":false,"reply":false,"resolve":false,"verdicts":[]},"reviewers":{"request":false,"listCandidates":false},"stackActions":scenario=="stack"},
                    "viewerPermissions":{"actions":if scenario=="no-permission" { vec![] } else {vec!["merge"]},"comment":true,"resolve":false,"verdicts":[],"requestReviewers":false},
                    "projectId":"project","projectTitle":"Project","workspaceRoot":"/source","repository":"acme/web","number":7,"title":"Title","body":"","url":"https://github.com/acme/web/pull/7","author":null,
                    "state":if scenario=="closed" {"closed"} else {"open"},"isDraft":scenario=="draft","mergeability":"mergeable","additions":0,"deletions":0,"changedFiles":0,"headBranch":"feature","baseBranch":"main",
                    "createdAt":"2026-07-01T00:00:00Z","updatedAt":"2026-07-02T00:00:00Z","mergedAt":null,"closedAt":null,"reviewers":[],"labels":[],"checks":[],"mergeCapabilities":{"merge":true,"squash":true,"rebase":false}}),
                );
            }
            if tag == "pullRequests.stack" {
                return Ok(
                    json!({"id":"stack","number":7,"url":"https://github.com/acme/web/pull/7","base":"main","layers":[]}),
                );
            }
            Ok(Value::Null)
        });
        let service = PullRequestService::new(Rc::new(FakePlatform::default()), registry.clone());
        let resolver: MergeMethodResolver = Rc::new(move |_| {
            if scenario == "resolver-error" {
                Err("Could not choose a merge method.".into())
            } else {
                Ok(PullRequestMergeMethod::Squash)
            }
        });
        let mut input = reference();
        input["action"] = json!("merge");
        let result = block_on(service.command(
            env("origin"),
            "pullRequests.runAction".into(),
            input,
            Some(resolver),
        ));
        assert_eq!(
            result.is_ok(),
            scenario == "merge",
            "{scenario}: {result:?}"
        );
        let calls = registry.calls.borrow();
        let mutations: Vec<_> = calls
            .iter()
            .filter(|(_, tag, _)| tag == "pullRequests.runAction")
            .collect();
        if scenario == "merge" {
            assert_eq!(mutations.len(), 1);
            assert_eq!(mutations[0].2["mergeMethod"], "squash");
        } else {
            assert!(mutations.is_empty());
        }
    }
}
