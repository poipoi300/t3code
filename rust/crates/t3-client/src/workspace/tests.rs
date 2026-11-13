use super::{branch::*, projects::*, pull_requests::*, review::*, vcs::*, *};
use super::{git_actions::*, pr_assignment::*, pr_list::*, sidebar::*};
use crate::platform::{HostFuture, PlatformError, Storage, StorageArea, StorageKey, StorageWrite};
use crate::types::Generation;
use futures::{FutureExt, executor::block_on};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
    task::Poll,
};
use t3_contracts::pull_request::*;

fn decode<T: DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).unwrap()
}
fn env(value: &str) -> EnvironmentId {
    EnvironmentId(value.into())
}
fn project(id: &str, path: &str) -> EnvironmentProject {
    EnvironmentProject {
        environment: env("local"),
        is_scratch: false,
        project: decode(
            json!({"id":id,"title":id,"workspaceRoot":path,"repositoryIdentity":{"canonicalKey":"github.com/t3/code","locator":{"source":"git-remote","remoteName":"origin","remoteUrl":"https://github.com/t3/code.git"},"displayName":"T3 Code","name":"code"},"defaultModelSelection":null,"scripts":[],"createdAt":"2026-07-01T00:00:00.000Z","updatedAt":"2026-07-01T00:00:00.000Z"}),
        ),
    }
}
fn pr(environment: &str) -> PullRequestKey {
    PullRequestKey::new(env(environment), &reference())
}
fn reference() -> PullRequestRef {
    decode(json!({"projectId":"project","repository":"T3/Code","number":1}))
}
fn scope(environment: &str) -> ViewedScopeKey {
    ViewedScopeKey {
        pull_request: pr(environment),
        allow_stale: None,
    }
}
fn press(path: &str, viewed: bool) -> PullRequestSetFilesViewedInputFiles {
    decode(json!({"path":path,"viewed":viewed}))
}
fn answer(state: &str) -> Rc<PullRequestFilesViewedResult> {
    Rc::new(decode(
        json!({"files":[{"path":"a.ts","state":state}],"truncated":false}),
    ))
}
fn checkout(environment: &str, cwd: &str) -> CheckoutKey {
    CheckoutKey {
        environment: env(environment),
        cwd: cwd.into(),
    }
}
fn refs_input() -> t3_contracts::git::VcsListRefsInput {
    decode(json!({"cwd":"/repo","limit":100}))
}
fn refs() -> t3_contracts::git::VcsListRefsResult {
    decode(
        json!({"refs":[],"isRepo":true,"hasPrimaryRemote":false,"nextCursor":null,"totalCount":0}),
    )
}

#[derive(Default)]
struct MemoryStorage {
    values: RefCell<BTreeMap<StorageKey, Vec<u8>>>,
    fail: Cell<bool>,
}
impl Storage for MemoryStorage {
    fn get(&self, _area: StorageArea, key: StorageKey) -> HostFuture<'_, Option<Vec<u8>>> {
        async move { Ok(self.values.borrow().get(&key).cloned()) }.boxed_local()
    }
    fn write(&self, _area: StorageArea, writes: Vec<StorageWrite>) -> HostFuture<'_, ()> {
        async move {
            if self.fail.get() {
                return Err(PlatformError::Storage("scripted".into()));
            }
            for write in writes {
                if let Some(value) = write.value {
                    self.values.borrow_mut().insert(write.key, value);
                } else {
                    self.values.borrow_mut().remove(&write.key);
                }
            }
            Ok(())
        }
        .boxed_local()
    }
    fn list(&self, _area: StorageArea, namespace: String) -> HostFuture<'_, Vec<StorageKey>> {
        async move {
            Ok(self
                .values
                .borrow()
                .keys()
                .filter(|key| key.namespace == namespace)
                .cloned()
                .collect())
        }
        .boxed_local()
    }
}

#[test]
fn project_paths_preserve_unix_case_and_normalize_windows_roots() {
    for (input, expected) in [
        (" C:/Repo/ ", "c:\\repo"),
        ("C:\\", "c:\\"),
        ("/Repo/", "/Repo"),
        ("/a\\", "/a\\"),
        ("\\\\Host\\Share/", "\\\\host\\share"),
        ("/", "/"),
    ] {
        assert_eq!(normalize_project_path_for_comparison(input), expected);
    }
}
#[test]
fn grouping_preserves_clone_members_and_every_registration() {
    let mut stale = project("stale", "/repo");
    stale.project.repository_identity = None;
    let mut fresh = project("fresh", "/repo/");
    fresh.project.updated_at.0 = "2026-07-02T00:00:00.000Z".into();
    let sibling = project("sibling", "/other");
    let groups = build_project_groups(
        &[stale, fresh.clone(), sibling],
        &GroupingSettings::default(),
        None,
        false,
    );
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].members.len(), 2);
    assert_eq!(groups[0].member_project_refs.len(), 3);
    assert_eq!(groups[0].representative, fresh);
}
#[test]
fn grouping_uses_latest_identity_bearing_duplicate_when_winner_has_none() {
    let mut stale = project("stale", "/repo");
    stale
        .project
        .repository_identity
        .as_mut()
        .unwrap()
        .as_mut()
        .unwrap()
        .canonical_key
        .0 = "old".into();
    let mut identified = project("identified", "/repo/");
    identified.project.updated_at.0 = "2026-07-02T00:00:00Z".into();
    let mut winner = project("winner", "/repo");
    winner.project.repository_identity = None;
    winner.project.updated_at.0 = "2026-07-03T00:00:00Z".into();
    let groups = build_project_groups(
        &[stale, identified, winner, project("sibling", "/other")],
        &GroupingSettings::default(),
        None,
        false,
    );
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].members[0].project.id.0, "winner");
}
#[test]
fn grouping_prefers_requested_environment_and_ties_break_by_id() {
    let a = project("a", "/repo");
    let mut z = project("z", "/repo");
    z.environment = env("remote");
    let groups = build_project_groups(
        &[a.clone(), z.clone()],
        &GroupingSettings::default(),
        Some(&env("remote")),
        false,
    );
    assert_eq!(groups[0].representative, z);
    assert_eq!(
        environment_presence(&groups[0], Some(&env("local"))),
        EnvironmentPresence::Mixed
    );
    let mut b = a.clone();
    b.project.id.0 = "b".into();
    assert_eq!(
        build_project_groups(&[b, a], &GroupingSettings::default(), None, false)[0]
            .representative
            .project
            .id
            .0,
        "b"
    );
}
#[test]
fn grouping_respects_overrides_forks_paths_and_custom_titles() {
    let mut a = project("a", "/repo/pkg");
    a.project
        .repository_identity
        .as_mut()
        .unwrap()
        .as_mut()
        .unwrap()
        .root_path = Some(decode(json!("/repo")));
    let mut settings = GroupingSettings {
        mode: GroupingMode::RepositoryPath,
        ..Default::default()
    };
    assert_eq!(
        logical_project_key(&a, &settings, false),
        LogicalProjectKey::Repository("github.com/t3/code::pkg".into())
    );
    settings
        .overrides
        .insert(a.physical_key(), GroupingMode::Separate);
    assert!(matches!(
        logical_project_key(&a, &settings, false),
        LogicalProjectKey::Physical(_)
    ));
    let mut b = project("b", "/other");
    for p in [&mut a, &mut b] {
        p.project.title.0 = "code".into();
        p.project.has_custom_title = Some(Some(true));
    }
    assert_eq!(
        build_project_groups(
            &[a.clone(), b.clone()],
            &GroupingSettings::default(),
            None,
            false
        )[0]
        .label,
        "code"
    );
    b.project
        .repository_identity
        .as_mut()
        .unwrap()
        .as_mut()
        .unwrap()
        .origin = Some(decode(
        json!({"canonicalKey":"fork/code","displayName":"Fork"}),
    ));
    assert_eq!(
        build_project_groups(&[a, b], &GroupingSettings::default(), None, false).len(),
        2
    );
}
#[test]
fn scratch_grouping_and_scope_migration_do_not_swallow_ordinary_projects() {
    for mode in [
        GroupingMode::Repository,
        GroupingMode::RepositoryPath,
        GroupingMode::Separate,
    ] {
        let mut a = project("a", "/scratch");
        a.is_scratch = true;
        a.project.repository_identity = None;
        let mut b = a.clone();
        b.environment = env("remote");
        let settings = GroupingSettings {
            mode,
            ..Default::default()
        };
        assert_eq!(
            build_project_groups(&[a.clone(), b.clone()], &settings, None, false).len(),
            2
        );
        let groups = build_project_groups(
            &[a.clone(), b.clone(), project("ordinary", "/repo")],
            &settings,
            None,
            true,
        );
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].label, "No project");
        assert_eq!(groups[0].member_project_refs.len(), 2);
        assert_eq!(
            resolve_scratch_scope(
                Some(LogicalProjectKey::Physical(a.physical_key())),
                &[a, b],
                &settings
            ),
            Some(LogicalProjectKey::Scratch)
        );
    }
}
#[test]
fn pins_and_archives_keep_disagreeing_machines_visible() {
    let mut a = project("a", "/repo");
    a.project.pinned_at = Some(Some(decode(json!("2026-07-01T00:00:00Z"))));
    a.project.archived_at = a.project.pinned_at.clone();
    let b = project("b", "/other");
    let groups = build_project_groups(&[a, b], &GroupingSettings::default(), None, false);
    assert!(groups[0].pinned);
    assert!(!groups[0].archived);
    assert_eq!(pickable_project_groups(&groups).len(), 1);
}
#[test]
fn group_order_and_picker_targets_use_environment_with_project_id() {
    let a = project("same", "/repo");
    let mut b = a.clone();
    b.environment = env("remote");
    let mut projects = vec![a, b.clone()];
    let settings = GroupingSettings {
        mode: GroupingMode::Separate,
        ..Default::default()
    };
    let mut groups = build_project_groups(&projects, &settings, None, false);
    groups.reverse();
    sort_projects_by_group_order(&mut projects, &groups);
    assert_eq!(projects[0], b);
    assert_eq!(picker_target(&groups[0], Some(&b.key())), &b);
}
#[test]
fn vcs_commands_are_fifo_per_checkout_and_cancel_queued_requests() {
    block_on(async {
        let scheduler = VcsCommandScheduler::default();
        let key = checkout("local", "/repo");
        let first = scheduler.acquire(key.clone()).await;
        let mut second = scheduler.acquire(key.clone());
        let third = scheduler.acquire(key);
        assert!(matches!(futures::poll!(&mut second), Poll::Pending));
        drop(second);
        let other = scheduler.acquire(checkout("remote", "/repo")).await;
        drop(other);
        drop(first);
        drop(third.await);
    });
}
#[test]
fn vcs_revalidation_shares_connections_and_debounces_wake() {
    let mut cadence = VcsStatusRevalidation::default();
    cadence.wake_debounce_ms = 250;
    let key = checkout("local", "/repo");
    assert!(cadence.subscribe(key.clone(), Some(true)));
    assert!(!cadence.subscribe(key.clone(), Some(true)));
    assert!(cadence.connection_changed(&env("local"), false).is_empty());
    assert_eq!(
        cadence.connection_changed(&env("local"), true),
        vec![key.clone()]
    );
    cadence.wake(0);
    cadence.wake(100);
    assert!(cadence.take_due(349).is_empty());
    assert_eq!(cadence.take_due(350), vec![key.clone()]);
    cadence.wake(400);
    cadence.unsubscribe(&key);
    assert_eq!(cadence.next_due(), Some(650));
    cadence.unsubscribe(&key);
    assert_eq!(cadence.next_due(), None);
}
#[test]
fn vcs_revalidation_waits_for_connection_and_keeps_environments_separate() {
    let mut cadence = VcsStatusRevalidation::default();
    let local = checkout("local", "/repo");
    let remote = checkout("remote", "/repo");
    assert!(!cadence.subscribe(local.clone(), Some(false)));
    assert!(!cadence.subscribe(remote, Some(false)));
    assert_eq!(cadence.connection_changed(&env("local"), true), vec![local]);
}
#[test]
fn vcs_refs_invalidation_rejects_stale_commit_and_clears_whole_environment() {
    block_on(async {
        let storage = MemoryStorage::default();
        let cache = VcsRefsCache::default();
        let local = checkout("local", "/repo");
        let remote = checkout("remote", "/repo");
        assert!(
            cache
                .commit_refresh(&storage, &local, &refs(), 0, true)
                .await
        );
        assert!(
            cache
                .commit_refresh(&storage, &remote, &refs(), 0, true)
                .await
        );
        assert!(cache.load(&storage, &local, &refs_input()).await.is_some());
        assert!(cache.invalidate_cached(&storage, &local.environment).await);
        assert!(
            !cache
                .commit_refresh(&storage, &local, &refs(), 0, true)
                .await
        );
        assert!(cache.load(&storage, &local, &refs_input()).await.is_none());
        assert!(cache.load(&storage, &remote, &refs_input()).await.is_some());
        assert!(cache.bypass_server_snapshot(&local, 1));
        assert!(
            cache
                .commit_refresh(&storage, &local, &refs(), 1, true)
                .await
        );
        assert!(!cache.bypass_server_snapshot(&local, 1));
    });
}
#[test]
fn refs_failed_clear_disables_hydration_until_recovered() {
    block_on(async {
        let storage = MemoryStorage::default();
        let cache = VcsRefsCache::default();
        let key = checkout("local", "/repo");
        cache.commit_refresh(&storage, &key, &refs(), 0, true).await;
        storage.fail.set(true);
        assert!(!cache.invalidate_cached(&storage, &key.environment).await);
        assert!(!cache.state(&key.environment).persisted_cache_readable);
        assert!(cache.load(&storage, &key, &refs_input()).await.is_none());
        storage.fail.set(false);
        assert!(cache.commit_refresh(&storage, &key, &refs(), 1, true).await);
        assert!(cache.state(&key.environment).persisted_cache_readable);
    });
}
#[test]
fn refs_filtered_pages_never_use_offline_full_list() {
    let mut input = refs_input();
    assert!(can_use_refs_cache(&input));
    input.query = Some(Some(decode(json!("a"))));
    assert!(!can_use_refs_cache(&input));
    input.query = None;
    input.cursor = Some(Some(0));
    assert!(!can_use_refs_cache(&input));
    assert_eq!(
        [0, 1, 2, 5, 32].map(refs_retry_delay_ms),
        [1000, 2000, 4000, 30000, 30000]
    );
}
#[test]
fn pr_keys_separate_environment_host_account_and_hostless_identity() {
    let a = pr("local");
    assert_ne!(a, pr("remote"));
    let mut reference = reference();
    reference.host = Some(Some(decode(json!("GITHUB.COM"))));
    let host = PullRequestKey::new(env("local"), &reference);
    assert_ne!(a, host);
    reference.host = Some(Some(decode(json!("github.com"))));
    assert_eq!(host, PullRequestKey::new(env("local"), &reference));
    reference.expected_account_id = Some(Some(decode(json!("other"))));
    assert_ne!(host, PullRequestKey::new(env("local"), &reference));
}
#[test]
fn queries_preserve_readable_value_on_failure_and_reject_old_generations() {
    let mut query = CachedQuery::default();
    let first = query.begin(Generation(1));
    assert!(query.finish(first, Ok(1), 0));
    let old = query.begin(Generation(1));
    let current = query.begin(Generation(2));
    assert!(!query.finish(old, Ok(0), 1));
    assert!(query.finish(current, Err("offline".into()), 1));
    assert_eq!(query.value, Some(1));
    assert_eq!(query.error.as_deref(), Some("offline"));
    let pending = query.begin(Generation(2));
    query.patch(|v| *v = 2);
    assert!(!query.finish(pending, Ok(0), 2));
    assert_eq!(query.value, Some(2));
}
#[test]
fn query_staleness_matches_each_resource_policy() {
    let mut query = CachedQuery::default();
    let ticket = query.begin(Generation(1));
    query.finish(ticket, Ok(()), 0);
    for (kind, stale) in [
        (QueryKind::List, 30000),
        (QueryKind::Checks, 45000),
        (QueryKind::FilterOptions, 300000),
        (QueryKind::FilesViewed, 15000),
        (QueryKind::Summary, 60000),
    ] {
        assert!(!query.is_stale(kind, stale - 1));
        assert!(query.is_stale(kind, stale));
    }
    assert!(query.is_stale(QueryKind::LinkedThreads, 0));
}
#[test]
fn query_cache_bounds_are_per_environment_and_count_utf8_bytes() {
    let mut cache = PullRequestQueryCache::default();
    for i in 0..600 {
        cache.save(env("local"), i.to_string(), format!("{i}"), i);
        assert!(cache.row_count(&env("local")) <= QUERY_MAX_ROWS);
    }
    cache.save(env("remote"), "text".into(), "\"é\"".into(), 0);
    assert_eq!(cache.payload_bytes(&env("remote")), 4);
    assert_eq!(
        cache.load::<String>(&env("remote"), "text", 0),
        Some("é".into())
    );
    cache.remove_environment(&env("local"));
    assert_eq!(cache.row_count(&env("local")), 0);
    assert_eq!(cache.row_count(&env("remote")), 1);
}
#[test]
fn query_cache_large_replacement_removes_old_copy_and_expiry_is_inclusive() {
    let mut cache = PullRequestQueryCache::default();
    cache.save(env("local"), "a".into(), "1".into(), 0);
    assert_eq!(
        cache.load::<u8>(&env("local"), "a", QUERY_MAX_AGE_MS),
        Some(1)
    );
    assert_eq!(
        cache.load::<u8>(&env("local"), "a", QUERY_MAX_AGE_MS + 1),
        None
    );
    cache.save(
        env("local"),
        "a".into(),
        "x".repeat(QUERY_MAX_PAYLOAD_BYTES + 1),
        1,
    );
    assert_eq!(cache.row_count(&env("local")), 0);
    for i in 0..20 {
        cache.save(
            env("local"),
            i.to_string(),
            "x".repeat(QUERY_MAX_PAYLOAD_BYTES),
            i,
        );
        assert!(cache.payload_bytes(&env("local")) <= QUERY_MAX_BYTES);
    }
}
#[test]
fn query_cache_persistence_removes_evicted_rows_atomically() {
    block_on(async {
        let mut cache = PullRequestQueryCache::default();
        let storage = MemoryStorage::default();
        cache.save(env("local"), "a".into(), "1".into(), 0);
        cache
            .persist_environment(&storage, &env("local"))
            .await
            .unwrap();
        assert_eq!(storage.values.borrow().len(), 1);
        cache.remove_environment(&env("local"));
        cache
            .persist_environment(&storage, &env("local"))
            .await
            .unwrap();
        assert!(storage.values.borrow().is_empty());
    });
}
fn route(id: &str, local: bool) -> RoutingEnvironment {
    RoutingEnvironment {
        environment: env(id),
        enabled: true,
        supported: true,
        connected: true,
        local,
        checks_supported: true,
        connection_key: Some(id.into()),
        granted_connection_key: Some(id.into()),
        permission: RoutingPermission::ReadWrite,
    }
}
#[test]
fn routing_prefers_local_and_keeps_guide_on_origin() {
    let source = route("source", false);
    let local = route("local", true);
    let remote = route("remote", false);
    let entries = vec![remote, local];
    assert_eq!(
        route_candidates(&source, &entries, RouteOperation::Read(QueryKind::Detail)),
        vec![env("local"), env("remote"), env("source")]
    );
    assert_eq!(
        route_candidates(&source, &entries, RouteOperation::Write),
        vec![env("local"), env("source"), env("remote")]
    );
    assert_eq!(
        route_candidates(&source, &entries, RouteOperation::ReviewGuide),
        vec![env("source")]
    );
}
#[test]
fn routing_rechecks_endpoint_grants_and_account_identity() {
    let source = route("source", true);
    let mut target = route("target", true);
    target.permission = RoutingPermission::Read;
    assert!(routing_allowed(
        &source,
        &target,
        RouteOperation::Read(QueryKind::Summary)
    ));
    assert!(!routing_allowed(&source, &target, RouteOperation::Write));
    target.connection_key = Some("replaced".into());
    assert!(!routing_allowed(
        &source,
        &target,
        RouteOperation::Read(QueryKind::Summary)
    ));
    let identity: PullRequestRoutingResult = decode(
        json!({"accountId":"a","host":"github.com","provider":"github","viewer":"user","projectTitle":"Project","workspaceRoot":"/repo"}),
    );
    let mut alternate: PullRequestRoutingIdentityResult =
        decode(json!({"accountId":"a","host":"GITHUB.COM","provider":"github","viewer":"user"}));
    assert!(routing_identity_matches(&identity, &alternate));
    alternate.account_id.0 = "b".into();
    assert!(!routing_identity_matches(&identity, &alternate));
}
#[test]
fn writes_never_retry_ambiguous_dispatch_and_cancellation_never_falls_back() {
    assert!(!can_retry_route(
        RouteOperation::Write,
        RouteFailure::AfterDispatch
    ));
    assert!(can_retry_route(
        RouteOperation::Read(QueryKind::Detail),
        RouteFailure::AfterDispatch
    ));
    assert!(can_retry_route(
        RouteOperation::Write,
        RouteFailure::IdentityRejectedBeforeDispatch
    ));
    assert!(!can_retry_route(
        RouteOperation::Read(QueryKind::Detail),
        RouteFailure::Interrupted
    ));
}
#[test]
fn routed_reads_are_capped_and_invalidation_is_environment_scoped() {
    let mut reads = RoutedReads::default();
    for i in 0..257 {
        let mut key = pr("local");
        key.number = i;
        reads.record(key, env(&format!("target-{i}")));
    }
    assert_eq!(reads.targets(&env("local"), None).len(), 256);
    assert!(
        !reads
            .targets(&env("local"), None)
            .contains(&env("target-0"))
    );
    assert!(reads.targets(&env("remote"), None).is_empty());
}
#[test]
fn review_submission_preserves_new_comments_and_revised_summary() {
    let mut store = PullRequestReviewStore::default();
    let key = pr("local");
    let comment =
        || decode(json!({"path":"a.ts","body":"Comment","position":{"kind":"added","newLine":1}}));
    let first = store.add_comment(key.clone(), comment());
    let second = store.add_comment(key.clone(), comment());
    assert_ne!(first, second);
    store.remove_comments(&key, &BTreeSet::from([first]));
    assert_eq!(store.comments(&key).len(), 1);
    store.set_summary(key.clone(), "submitted".into());
    store.set_summary(key.clone(), "revised".into());
    store.clear_submitted_summary(&key, "submitted");
    assert_eq!(store.summary(&key), Some("revised"));
    assert!(store.comments(&pr("remote")).is_empty());
    store.clear_submitted_summary(&key, "revised");
    assert_eq!(store.summary(&key), None);
}
#[test]
fn viewed_batches_debounce_and_retire_after_distinct_post_ack_answer() {
    let mut store = FilesViewedStore::default();
    let key = scope("local");
    let initial = answer("unviewed");
    store.subscribe(key.clone(), Some(initial.clone()));
    store.set_viewed(&key, press("a.ts", true), 0);
    assert!(store.take_due(399).is_empty());
    let batch = store.take_due(400).remove(0);
    assert_eq!(batch.files.len(), 1);
    assert!(store.complete(&batch, ViewedWriteResult::Success).refresh);
    store.answer(&key, Some(initial));
    assert_eq!(store.overlay(&key).unwrap().get("a.ts"), Some(&true));
    store.answer(&key, Some(answer("dismissed")));
    assert!(store.overlay(&key).unwrap().is_empty());
}
#[test]
fn viewed_latest_press_keeps_revision_and_insert_order() {
    let mut store = FilesViewedStore::default();
    let key = scope("local");
    store.subscribe(key.clone(), None);
    store.set_viewed(&key, press("b.ts", true), 0);
    store.set_viewed(&key, press("a.ts", true), 10);
    let mut file = press("b.ts", false);
    file.revision = Some(Some(decode(json!("revision-2"))));
    store.set_viewed(&key, file.clone(), 20);
    assert!(store.take_due(419).is_empty());
    let batch = store.take_due(420).remove(0);
    assert_eq!(batch.files, vec![file, press("a.ts", true)]);
}
#[test]
fn viewed_failure_reverts_only_owned_paths_and_reports_non_interruption() {
    let mut store = FilesViewedStore::default();
    let key = scope("local");
    store.subscribe(key.clone(), None);
    store.set_viewed(&key, press("a.ts", true), 0);
    let first = store.flush(&key).unwrap();
    store.set_viewed(&key, press("a.ts", false), 1);
    assert_eq!(
        store.complete(&first, ViewedWriteResult::Failure),
        ViewedCompletion::default()
    );
    assert_eq!(store.overlay(&key).unwrap().get("a.ts"), Some(&false));
    let second = store.flush(&key).unwrap();
    assert_eq!(
        store.complete(&second, ViewedWriteResult::Failure),
        ViewedCompletion {
            refresh: true,
            report_failure: true
        }
    );
    assert!(store.overlay(&key).unwrap().is_empty());
}
#[test]
fn viewed_older_request_cannot_revert_newer_identical_press() {
    let mut store = FilesViewedStore::default();
    let key = scope("local");
    store.subscribe(key.clone(), None);
    store.set_viewed(&key, press("a.ts", true), 0);
    let old = store.flush(&key).unwrap();
    store.set_viewed(&key, press("a.ts", true), 1);
    let new = store.flush(&key).unwrap();
    assert_eq!(
        store.complete(&old, ViewedWriteResult::Failure),
        ViewedCompletion::default()
    );
    assert_eq!(store.overlay(&key).unwrap().get("a.ts"), Some(&true));
    store.complete(&new, ViewedWriteResult::Success);
}
#[test]
fn viewed_scope_last_release_flushes_and_late_completion_cannot_touch_reopened_scope() {
    let mut store = FilesViewedStore::default();
    let key = scope("local");
    store.subscribe(key.clone(), None);
    store.subscribe(key.clone(), None);
    store.set_viewed(&key, press("a.ts", true), 0);
    assert!(store.unsubscribe(&key).is_none());
    let old = store.unsubscribe(&key).unwrap();
    assert!(store.overlay(&key).is_none());
    store.subscribe(key.clone(), None);
    store.set_viewed(&key, press("a.ts", false), 1);
    assert_eq!(
        store.complete(&old, ViewedWriteResult::Failure),
        ViewedCompletion {
            refresh: true,
            report_failure: false
        }
    );
    assert_eq!(store.overlay(&key).unwrap().get("a.ts"), Some(&false));
}
#[test]
fn viewed_agreeing_queued_answer_does_not_retire_press() {
    let mut store = FilesViewedStore::default();
    let key = scope("local");
    store.subscribe(key.clone(), Some(answer("unviewed")));
    store.set_viewed(&key, press("a.ts", true), 0);
    let agreeing = answer("viewed");
    store.answer(&key, Some(agreeing.clone()));
    assert!(!store.overlay(&key).unwrap().is_empty());
    let batch = store.flush(&key).unwrap();
    store.complete(&batch, ViewedWriteResult::Success);
    store.answer(&key, Some(agreeing));
    assert!(!store.overlay(&key).unwrap().is_empty());
    store.answer(&key, Some(answer("unviewed")));
    assert!(store.overlay(&key).unwrap().is_empty());
}
#[test]
fn viewed_overlay_helpers_keep_dismissed_and_optimistic_states_distinct() {
    let states = to_file_viewed_states(Some(&answer("dismissed"))).unwrap();
    assert!(is_stale_viewed_state(states.get("a.ts")));
    assert!(!is_file_viewed("a.ts", Some(&states), &BTreeMap::new()));
    let mut overlay = BTreeMap::from([("a.ts".into(), true)]);
    assert!(is_file_viewed("a.ts", Some(&states), &overlay));
    settle_file_viewed_overlay(
        &mut overlay,
        Some(&states),
        &BTreeSet::from(["a.ts".into()]),
        &BTreeSet::from(["a.ts".into()]),
    );
    assert_eq!(overlay.len(), 1);
    settle_file_viewed_overlay(
        &mut overlay,
        Some(&states),
        &BTreeSet::new(),
        &BTreeSet::from(["a.ts".into()]),
    );
    assert!(overlay.is_empty());
}
#[test]
fn diff_fold_choices_are_absolute_and_ranges_include_both_endpoints() {
    let choices = BTreeMap::from([("a".into(), false), ("b".into(), true)]);
    assert!(!is_file_diff_collapsed(
        "a",
        Some(DiffFoldOverride::Folded),
        &choices,
        true
    ));
    assert!(is_file_diff_collapsed(
        "b",
        Some(DiffFoldOverride::Expanded),
        &choices,
        false
    ));
    assert!(is_file_diff_collapsed(
        "c",
        Some(DiffFoldOverride::Expanded),
        &choices,
        true
    ));
    let files = ["c", "a", "b", "d"];
    assert_eq!(
        file_viewed_range(&files, "b", Some("c"), |v| v),
        &files[..3]
    );
    assert_eq!(
        file_viewed_range(&files, "c", Some("b"), |v| v),
        &files[..3]
    );
    assert_eq!(file_viewed_range(&files, "a", None, |v| v), &files[1..2]);
    assert!(file_viewed_range(&files, "missing", None, |v| v).is_empty());
}
#[test]
fn branch_toolbar_keeps_draft_checkout_mode_and_pending_branch_priority() {
    assert_eq!(
        effective_env_mode(Some("/worktree"), false, Some(EnvMode::Worktree), false),
        EnvMode::Local
    );
    assert_eq!(
        effective_env_mode(None, true, None, true),
        EnvMode::Worktree
    );
    assert_eq!(
        branch_toolbar_value(EnvMode::Worktree, None, Some("next"), Some("main")),
        Some("next")
    );
    assert_eq!(
        branch_toolbar_value(EnvMode::Worktree, Some("/wt"), Some("old"), Some("live")),
        Some("live")
    );
    assert_eq!(
        branch_trigger_label(None, EnvMode::Worktree, Some("feature"), Some(false), true),
        "From feature (remote)"
    );
    assert_eq!(
        local_checkout_branch_mismatch(EnvMode::Local, None, Some("a"), Some("b")),
        Some(("a", "b"))
    );
    assert_eq!(
        local_checkout_branch_mismatch(EnvMode::Worktree, None, Some("a"), Some("b")),
        None
    );
}
#[test]
fn branch_picker_reuses_checkout_and_switches_default_back_to_project() {
    let mut reference: t3_contracts::git::VcsRef =
        decode(json!({"name":"main","current":false,"isDefault":true,"worktreePath":"/repo"}));
    let target = branch_selection_target("/repo/pkg", None, Some("/wt"), &reference);
    assert!(target.reuse_existing_worktree);
    assert_eq!(target.next_worktree_path, None);
    reference.worktree_path = Some(decode(json!("/other")));
    assert_eq!(
        branch_selection_target("/repo/pkg", None, None, &reference).next_worktree_path,
        Some(decode(json!("/other")))
    );
    reference.worktree_path = None;
    assert_eq!(
        branch_selection_target("/repo", None, Some("/wt"), &reference).checkout_cwd,
        "/repo"
    );
    assert!(include_branch_picker_item(
        "feature/new-name",
        "new name",
        None,
        None
    ));
    assert!(include_branch_picker_item(
        "create",
        "unmatched",
        Some("create"),
        None
    ));
    assert!(worktree_change_blocked(true, Some("a"), Some("b")));
}
#[test]
fn branch_environment_labels_hysteresis_and_previous_worktree_selection() {
    assert_eq!(
        environment_option_label(true, "env", Some("local"), Some("Laptop")),
        "Laptop"
    );
    assert_eq!(
        environment_option_label(true, "env", None, Some(" Local environment ")),
        "This device"
    );
    assert_eq!(
        environment_option_label(false, "remote", None, None),
        "remote"
    );
    assert!(context_labels_compact(true, 100.0, 115.0));
    assert!(!context_labels_compact(true, 100.0, 116.0));
    let threads = [
        WorktreeThread {
            branch: Some("archived"),
            path: Some("/old"),
            updated_at: "2026-07-03T00:00:00Z",
            archived: true,
        },
        WorktreeThread {
            branch: Some("feature"),
            path: Some("/wt"),
            updated_at: "2026-07-02T00:00:00Z",
            archived: false,
        },
        WorktreeThread {
            branch: None,
            path: Some("/bad"),
            updated_at: "bad",
            archived: false,
        },
    ];
    assert_eq!(previous_worktree_seed(&threads, None).unwrap().path, "/wt");
    assert!(previous_worktree_seed(&threads, Some("/wt")).is_none());
}
fn status() -> t3_contracts::git::VcsStatusResult {
    decode(
        json!({"isRepo":true,"hasPrimaryRemote":true,"isDefaultRef":false,"refName":"feature","hasWorkingTreeChanges":false,"workingTree":{"files":[],"insertions":0,"deletions":0},"hasUpstream":true,"aheadCount":0,"behindCount":0,"pr":null}),
    )
}
#[test]
fn git_quick_action_covers_dirty_clean_diverged_and_publishing_states() {
    use t3_contracts::git::GitStackedAction;
    let terminology = ChangeRequestTerminology {
        singular: "merge request",
        short_label: "MR",
    };
    let mut s = status();
    s.has_working_tree_changes = true;
    assert_eq!(
        quick_action(Some(&s), false, false, true, &terminology).kind,
        QuickActionKind::Run(GitStackedAction::CommitPushPr)
    );
    assert_eq!(
        quick_action(Some(&s), false, true, true, &terminology).kind,
        QuickActionKind::Run(GitStackedAction::CommitPush)
    );
    s.behind_count = 1;
    assert_eq!(
        quick_action(Some(&s), false, false, true, &terminology).kind,
        QuickActionKind::Run(GitStackedAction::Commit)
    );
    s.has_working_tree_changes = false;
    s.ahead_count = 1;
    assert!(quick_action(Some(&s), false, false, true, &terminology).disabled);
    s.ahead_count = 0;
    assert_eq!(
        quick_action(Some(&s), false, false, true, &terminology).kind,
        QuickActionKind::Pull
    );
    s.behind_count = 0;
    s.has_upstream = false;
    assert_eq!(
        quick_action(Some(&s), false, false, false, &terminology).kind,
        QuickActionKind::Publish
    );
    s.ahead_count = 1;
    assert_eq!(
        quick_action(Some(&s), false, false, true, &terminology).kind,
        QuickActionKind::Run(GitStackedAction::CreatePr)
    );
    assert!(quick_action(Some(&s), true, false, true, &terminology).disabled);
}
#[test]
fn git_menu_permits_push_with_changes_and_creation_against_default_delta() {
    let terminology = ChangeRequestTerminology {
        singular: "pull request",
        short_label: "PR",
    };
    let mut s = status();
    s.ahead_of_default_count = Some(Some(2));
    s.has_working_tree_changes = true;
    s.ahead_count = 1;
    let items = menu_items(Some(&s), false, true, &terminology);
    assert!(items.iter().all(|i| !i.disabled));
    assert_eq!(menu_items(Some(&s), false, false, &terminology).len(), 1);
    assert!(usable_menu(&items, true, false, false));
    assert!(!usable_menu(&items, false, false, false));
    s.has_working_tree_changes = false;
    s.ahead_count = 0;
    assert_eq!(
        quick_action(Some(&s), false, false, true, &terminology).label,
        "Create PR"
    );
}
#[test]
fn git_changes_totals_prefer_preview_then_nonzero_branch_then_working_tree() {
    let mut s = status();
    s.working_tree.insertions = 8;
    s.branch_changes = Some(Some(decode(
        json!({"baseRef":"main","insertions":0,"deletions":0}),
    )));
    assert_eq!(changes_totals(Some(&s), None).unwrap().insertions, 8);
    s.branch_changes
        .as_mut()
        .unwrap()
        .as_mut()
        .unwrap()
        .insertions = 3;
    assert_eq!(changes_totals(Some(&s), None).unwrap().insertions, 3);
    assert_eq!(
        changes_totals(
            Some(&s),
            Some(ChangesTotals {
                insertions: 0,
                deletions: 0,
                base_ref: Some("other".into())
            })
        )
        .unwrap()
        .base_ref,
        Some("other".into())
    );
}
#[test]
fn git_live_branch_updates_ignore_detached_and_temporary_regressions() {
    let mut s = status();
    s.ref_name = None;
    assert_eq!(live_thread_branch_update(Some("feature"), Some(&s)), None);
    s.ref_name = Some(decode(json!("t3-0123abcd")));
    assert_eq!(live_thread_branch_update(Some("feature"), Some(&s)), None);
    s.ref_name = Some(decode(json!("renamed")));
    assert_eq!(
        live_thread_branch_update(Some("feature"), Some(&s)),
        Some(Some("renamed".into()))
    );
}
#[test]
fn git_progress_and_confirmation_preserve_phase_and_host_terminology() {
    use t3_contracts::git::GitStackedAction;
    assert!(requires_default_branch_confirmation(
        &GitStackedAction::CommitPush,
        true
    ));
    assert!(!requires_default_branch_confirmation(
        &GitStackedAction::Commit,
        true
    ));
    let copy = default_branch_dialog_copy(
        &GitStackedAction::CreatePr,
        "main",
        true,
        &ChangeRequestTerminology {
            singular: "merge request",
            short_label: "MR",
        },
    );
    assert!(copy.title.contains("MR"));
    assert!(copy.description.contains("merge request"));
    let progress = progress_presentation(ProgressInput {
        running: true,
        operation: Some("run_change_request"),
        label: Some("Running source control action"),
        output: Some(" output "),
        phase_started: Some(10),
        hook_started: Some(20),
    })
    .unwrap();
    assert_eq!(progress.started_at, Some(20));
    assert_eq!(progress.output, Some("output".into()));
    assert_eq!(format_elapsed(Some(0), 61000), Some("1m 1s".into()));
    assert_eq!(result_dismiss_after_visible_ms(false), None);
}
#[test]
fn status_stream_parts_keep_remote_status_and_support_remote_before_snapshot() {
    let s = status();
    let remote: t3_contracts::git::VcsStatusStreamEvent = decode(
        json!({"_tag":"remoteUpdated","remote":{"hasUpstream":true,"aheadCount":2,"behindCount":1,"pr":null}}),
    );
    let first = apply_status_event(None, remote);
    assert!(first.is_repo);
    assert_eq!(first.ahead_count, 2);
    assert_eq!(first.ref_name, None);
    let local = serde_json::to_value(&s).unwrap();
    let event = decode(json!({"_tag":"localUpdated","local":local}));
    let next = apply_status_event(Some(&first), event);
    assert_eq!(next.ahead_count, 2);
    assert_eq!(next.ref_name, s.ref_name);
}
fn provider(host: &str, configured: bool, projects: Vec<&str>) -> PullRequestProviderSummary {
    decode(
        json!({"host":host,"kind":"github","searchesOnHost":true,"projectCount":projects.len().max(1),"projectIds":projects,"configured":configured,"detail":null}),
    )
}
#[test]
fn host_observations_merge_success_and_ignore_old_reads_after_reset() {
    let local = env("local");
    let failed = vec![provider("github.com", false, vec!["a"])];
    let success = vec![provider("GITHUB.COM", true, vec!["a"])];
    let mut hosts = BTreeMap::new();
    remember_hosts(
        &mut hosts,
        &[
            HostObservation {
                environment: &local,
                providers: &failed,
                observed_at: 1,
            },
            HostObservation {
                environment: &local,
                providers: &success,
                observed_at: 1,
            },
        ],
    );
    assert!(hosts[&local].hosts["github.com"].configured);
    remember_hosts(
        &mut hosts,
        &[HostObservation {
            environment: &local,
            providers: &failed,
            observed_at: 0,
        }],
    );
    assert!(hosts[&local].hosts["github.com"].configured);
    reset_hosts(&mut hosts);
    remember_hosts(
        &mut hosts,
        &[HostObservation {
            environment: &local,
            providers: &failed,
            observed_at: 1,
        }],
    );
    assert!(hosts[&local].hosts.is_empty());
}
#[test]
fn pr_assignment_prefers_named_server_then_readable_copy_and_retains_worktrees() {
    let a = project("a", "/repo");
    let mut b = project("b", "/repo");
    b.environment = env("remote");
    let c = project("worktree", "/wt");
    let projects = vec![b.clone(), a.clone(), c];
    let environments = vec![env("local"), env("remote")];
    let assignment = assign_projects(&projects, &environments, None, &BTreeMap::new(), |_| {
        "github.com".into()
    });
    assert_eq!(assignment[&env("local")].len(), 2);
    assert!(!assignment.contains_key(&env("remote")));
    let assignment = assign_projects(
        &projects,
        &environments,
        Some(&env("remote")),
        &BTreeMap::new(),
        |_| "github.com".into(),
    );
    assert_eq!(assignment[&env("remote")].len(), 1);
    let mut hosts = BTreeMap::new();
    hosts.insert(
        env("remote"),
        HostRead {
            hosts: BTreeMap::from([(
                "github.com".into(),
                HostAvailability {
                    configured: false,
                    observed_at: 1,
                },
            )]),
            ..Default::default()
        },
    );
    let assignment = assign_projects(
        &projects,
        &environments,
        Some(&env("remote")),
        &hosts,
        |_| "github.com".into(),
    );
    assert_eq!(assignment[&env("local")].len(), 2);
}
#[test]
fn pr_assignment_keeps_unknown_repositories_and_chunks_partial_queries() {
    let mut projects = vec![project("shared", "/repo")];
    for i in 0..205 {
        let mut p = project(&format!("u{i}"), &format!("/{i}"));
        p.environment = env("remote");
        p.project.repository_identity = None;
        projects.push(p);
    }
    let mut duplicate = project("duplicate", "/other");
    duplicate.environment = env("remote");
    projects.push(duplicate);
    let queries = project_queries(
        &projects,
        &[env("local"), env("remote")],
        &BTreeMap::new(),
        |_| "github.com".into(),
    );
    let remote: Vec<_> = queries
        .iter()
        .filter(|q| q.environment == env("remote"))
        .collect();
    assert_eq!(remote.len(), 3);
    assert_eq!(remote[0].project_ids.as_ref().unwrap().len(), 100);
    assert_eq!(remote[2].project_ids.as_ref().unwrap().len(), 5);
}
#[test]
fn pr_pickable_environments_keep_own_worktree_and_one_copy_per_server() {
    let a = project("a", "/repo");
    let own = project("own", "/worktree");
    let mut b = project("b", "/remote");
    b.environment = env("remote");
    let mut c = b.clone();
    c.project.id.0 = "c".into();
    let environments = vec![
        EnvironmentLabel {
            environment: env("local"),
            label: "Local".into(),
            machine: None,
        },
        EnvironmentLabel {
            environment: env("remote"),
            label: "Remote".into(),
            machine: None,
        },
    ];
    let choices = pickable_environments(&own.key(), &[a, own.clone(), b, c], &environments);
    assert_eq!(choices.len(), 2);
    assert_eq!(choices[0].project, own.key());
    assert_eq!(choices[1].project.project.0, "b");
}
#[test]
fn sidebar_snapshots_keep_remote_labels_and_preferred_picker_target() {
    let a = project("a", "/repo");
    let mut b = project("b", "/remote");
    b.environment = env("remote");
    let groups = build_sidebar_snapshots(
        &[a, b.clone()],
        &GroupingSettings::default(),
        false,
        Some(&env("local")),
        |id| Some(id.0.clone()),
        |_| true,
        |_| false,
    );
    assert!(groups_span_environments(&groups));
    assert_eq!(groups[0].remote_environment_labels, vec!["remote"]);
    assert!(groups[0].all_remote_members_are_desktop_local);
    assert!(!groups[0].all_remote_members_are_wsl);
    assert_eq!(picker_entries(&groups, Some(&b.key()))[0].target, &b);
}
#[test]
fn sidebar_scope_retains_reconnecting_projects_and_migrates_scratch() {
    let mut p = project("scratch", "/scratch");
    p.is_scratch = true;
    p.project.repository_identity = None;
    let key = LogicalProjectKey::Physical(p.physical_key());
    let settings = GroupingSettings::default();
    let groups = build_sidebar_snapshots(
        &[p.clone()],
        &settings,
        true,
        None,
        |_| None,
        |_| false,
        |_| false,
    );
    assert_eq!(
        resolve_scope(&groups, Some(key), &settings, false),
        Some(LogicalProjectKey::Scratch)
    );
    let missing = LogicalProjectKey::Repository("missing".into());
    assert_eq!(
        resolve_scope(&groups, Some(missing.clone()), &settings, false),
        Some(missing.clone())
    );
    assert_eq!(resolve_scope(&groups, Some(missing), &settings, true), None);
    assert_eq!(
        settings_key(&groups[0], &settings, &[p.clone()], Some(&p.key())),
        LogicalProjectKey::Physical(p.physical_key())
    );
}
fn row(number: u64) -> EnvironmentPullRequestEntry {
    EnvironmentPullRequestEntry {
        environment: env("local"),
        entry: decode(
            json!({"provider":"github","host":"github.com","projectId":"project","projectTitle":"Project","repository":"T3/Code","number":number,"title":format!("Fix task {number}"),"url":format!("https://github.com/T3/Code/pull/{number}"),"author":{"login":"Alice","name":null,"avatarUrl":null},"headBranch":"feature","baseBranch":"main","state":"open","isDraft":false,"mergeability":"mergeable","additions":0,"deletions":0,"createdAt":"2026-07-01T00:00:00Z","updatedAt":"2026-07-02T00:00:00Z","viewerReviewRequested":false,"labels":[{"name":"Bug","color":"ff0000"}]}),
        ),
    }
}
#[test]
fn pr_filters_support_label_disjunction_exclusions_and_current_viewer() {
    let entry = row(7);
    let filters = decode(json!({"draft":"hide","author":" ME ","labels":[["missing"," bug "]]}));
    assert!(matches_filters(&entry.entry, &filters, Some("Alice")));
    let filters = decode(json!({"excludedLabels":["bug"]}));
    assert!(!matches_filters(&entry.entry, &filters, None));
    assert!(matches_query(&entry.entry, "#7"));
    assert_eq!(score_match(&entry.entry, "7"), 100);
    assert_eq!(score_match(&entry.entry, "fix task 7"), 90);
}
#[test]
fn pr_involvement_groups_use_scoped_viewers_and_prioritize_author() {
    let mut a = row(1);
    a.entry.viewer_review_requested = true;
    let mut b = row(2);
    b.environment = env("remote");
    b.entry.viewer_review_requested = true;
    let entries = vec![a, b];
    let viewers = Viewers::from([
        ((Some(env("local")), "github.com".into()), "alice".into()),
        ((Some(env("remote")), "github.com".into()), "bob".into()),
    ]);
    let groups = group_by_involvement(&entries, &viewers);
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].key, GroupKey::Authored);
    assert_eq!(groups[1].key, GroupKey::ReviewRequested);
    assert_eq!(
        filter_by_involvement(&entries, &viewers, &PullRequestInvolvement::Authored).len(),
        1
    );
}
#[test]
fn pr_ranking_preserves_search_and_sorts_measured_sizes_before_unknown() {
    let mut a = row(1);
    a.entry.checks_state = Some(Some(PullRequestChecksState::Passing));
    a.entry.review_decision = Some(Some(PullRequestReviewDecision::Approved));
    let mut b = row(2);
    b.entry.additions = 10;
    let entries = vec![b, a];
    let mut groups = group_by_involvement(&entries, &BTreeMap::new());
    sort_groups(
        &mut groups,
        ListSort::Ready,
        "",
        |e| e.entry.additions > 0,
        &PullRequestInvolvement::All,
    );
    assert_eq!(groups[0].entries[0].entry.number, 1);
    sort_groups(
        &mut groups,
        ListSort::Smallest,
        "",
        |e| e.entry.additions > 0,
        &PullRequestInvolvement::All,
    );
    assert_eq!(groups[0].entries[0].entry.number, 2);
    assert_eq!(label_color(Some(" #fF0010 ")), Some("#fF0010".into()));
    assert_eq!(label_color(Some("red")), None);
}
#[test]
fn pr_override_settlement_requires_returned_row_and_guards_newer_host_reply() {
    let entry = row(1);
    let mut overrides = BTreeMap::new();
    overrides.insert(
        entry.key(),
        override_after_action(
            &entry.entry,
            &PullRequestAction::Close,
            0,
            "2026-07-03T00:00:00Z",
            1,
            None,
        )
        .unwrap(),
    );
    assert!(
        apply_overrides(
            std::slice::from_ref(&entry),
            &overrides,
            &PullRequestListState::Open
        )
        .is_empty()
    );
    settle_overrides(&mut overrides, &[], 61000);
    assert_eq!(overrides.len(), 1);
    settle_overrides(&mut overrides, std::slice::from_ref(&entry), 61000);
    assert!(overrides.is_empty());
    let before = Rc::new(entry.clone());
    let reused = reuse_entries(std::slice::from_ref(&before), vec![Rc::new(entry)]);
    assert!(Rc::ptr_eq(&before, &reused[0]));
}
#[test]
fn pr_scope_resolution_never_guesses_same_project_id_on_two_servers() {
    let a = project("same", "/repo");
    let mut b = a.clone();
    b.environment = env("remote");
    let projects = vec![a, b.clone()];
    assert!(find_scoped_project(&projects, None, Some(&b.project.id)).is_none());
    assert_eq!(
        find_scoped_project(&projects, Some(&env("remote")), Some(&b.project.id)),
        Some(&b)
    );
    assert_eq!(
        query_environment_ids(
            &[env("local"), env("remote")],
            &projects,
            Some(&b.key()),
            Some(&b.project.id),
            true
        ),
        vec![env("remote")]
    );
}
#[test]
fn diff_hunk_line_ranges_keep_sides_and_zero_count_hunks_distinct() {
    let hunks = [
        HunkRange {
            deletion_start: 10,
            deletion_count: 3,
            addition_start: 10,
            addition_count: 5,
        },
        HunkRange {
            deletion_start: 40,
            deletion_count: 0,
            addition_start: 42,
            addition_count: 2,
        },
    ];
    assert!(is_line_in_file_diff(
        &hunks,
        &PullRequestDiffSide::Right,
        14
    ));
    assert!(!is_line_in_file_diff(
        &hunks,
        &PullRequestDiffSide::Right,
        15
    ));
    assert!(is_line_in_file_diff(&hunks, &PullRequestDiffSide::Left, 12));
    assert!(!is_line_in_file_diff(
        &hunks,
        &PullRequestDiffSide::Left,
        40
    ));
    assert!(is_line_in_file_diff(
        &hunks,
        &PullRequestDiffSide::Right,
        43
    ));
}
#[test]
fn temporary_worktree_detection_keeps_legacy_names_and_validates_uuid_version() {
    for name in [
        "t3/01234567",
        "t3-abcdef12",
        " T3CODE-ABCDEF12 ",
        "t3code/01234567-1234-4567-89ab-0123456789ab",
    ] {
        assert!(is_temporary_worktree_branch(name), "{name}");
    }
    for name in [
        "t3/0123456",
        "t3/012345678",
        "t3code/01234567-1234-3567-89ab-0123456789ab",
        "feature/t3-01234567",
    ] {
        assert!(!is_temporary_worktree_branch(name), "{name}");
    }
}
#[test]
fn query_cache_restore_validates_environment_and_retains_newer_memory_answer() {
    block_on(async {
        let storage = MemoryStorage::default();
        let mut cache = PullRequestQueryCache::default();
        cache.save(env("local"), "a".into(), "1".into(), 1);
        cache
            .persist_environment(&storage, &env("local"))
            .await
            .unwrap();
        let bad = StorageKey {
            namespace: "pull-request-query".into(),
            environment: Some(env("local")),
            record: "wrong-env".into(),
        };
        storage.values.borrow_mut().insert(
            bad,
            serde_json::to_vec(&StoredQuery {
                environment_id: env("remote"),
                payload: "9".into(),
                updated_at: 5,
            })
            .unwrap(),
        );
        let mut restored = PullRequestQueryCache::default();
        restored.save(env("local"), "a".into(), "2".into(), 2);
        restored
            .restore_environment(&storage, &env("local"), 5)
            .await
            .unwrap();
        assert_eq!(restored.load::<u8>(&env("local"), "a", 5), Some(2));
        assert_eq!(restored.row_count(&env("local")), 1);
        assert_eq!(storage.values.borrow().len(), 1);
    });
}
#[test]
fn pr_reply_override_retains_host_observation_time_in_visible_row() {
    let entry = row(1);
    let reply: PullRequestSummary = decode(
        json!({"provider":"github","projectId":"project","repository":"T3/Code","number":1,"title":"Updated","url":"https://github.com/T3/Code/pull/1","state":"closed","headBranch":"feature","baseBranch":"main","updatedAt":"2026-07-03T00:00:00Z","observedAt":123}),
    );
    let held = override_after_action(
        &entry.entry,
        &PullRequestAction::Close,
        0,
        "2026-07-03T00:00:00Z",
        1,
        Some(&reply),
    )
    .unwrap();
    let overrides = BTreeMap::from([(entry.key(), held)]);
    let rows = apply_overrides(&[entry], &overrides, &PullRequestListState::All);
    assert_eq!(
        rows[0]
            .entry
            .observed_at
            .as_ref()
            .and_then(Option::as_ref)
            .and_then(|v| v.as_f64()),
        Some(123.0)
    );
}
