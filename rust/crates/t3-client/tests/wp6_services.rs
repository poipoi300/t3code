#![allow(clippy::unwrap_used)]
use futures::{
    FutureExt, StreamExt, channel::oneshot, executor::block_on, future::LocalBoxFuture,
    stream::LocalBoxStream,
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::{cell::RefCell, collections::VecDeque, rc::Rc};
use t3_client::{
    ClientError,
    platform::{Platform, Storage, StorageArea, StorageKey, StorageWrite},
    rpc::{RpcEndpoint, RpcMethod},
    services::{
        assets::*, config::*, filesystem::*, policy::*, providers::*, resources::*,
        runtime::Services, scheduling::*, settings::*, terminal::*, updates::*, usage::*,
    },
    testing::{FakePlatform, RpcScript, ScriptedRpc},
    types::{EnvironmentId, Generation, ThreadId, ThreadKey},
};
use t3_contracts::{
    assets::{AssetCreateUrlResult, AssetResource},
    filesystem::{FilesystemBrowseEntry, FilesystemEntryKind, FilesystemEntryMetadata},
    provider_instance::ProviderInstanceId,
    rpc::WsServerGetNpmReleaseRpc,
    server::*,
    settings::{ServerSettings, ServerSettingsPatch},
    terminal::*,
    usage::{UsageSummary, UsageSummaryInput},
};

fn env(id: &str) -> EnvironmentId {
    EnvironmentId(id.into())
}
fn thread(environment: &str, id: &str) -> ThreadKey {
    ThreadKey {
        environment: env(environment),
        thread: ThreadId(id.into()),
    }
}
fn decode<T: DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).unwrap()
}
fn config() -> ServerConfig {
    let mut config: ServerConfig = serde_json::from_str(
        include_str!("../../../fixtures/samples/ServerConfig.jsonl")
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    config.environment.environment_id = env("one");
    config.environment.server_version.0 = "1".into();
    config.settings = ServerSettings::default();
    config
}
fn provider(instance: &str, fields: Value) -> ServerProvider {
    let mut value = json!({"instanceId":instance,"driver":"codex","enabled":true,"installed":true,"version":null,"status":"ready","auth":{"status":"authenticated"},"checkedAt":"2026-04-01T00:00:00Z","models":[],"slashCommands":[],"skills":[]});
    value
        .as_object_mut()
        .unwrap()
        .extend(fields.as_object().unwrap().clone());
    decode(value)
}
fn snapshot(config: ServerConfig) -> ServerConfigStreamEvent {
    decode(json!({"version":1,"type":"snapshot","config":config}))
}
fn terminal_snapshot(history: &str) -> TerminalSessionSnapshot {
    decode(
        json!({"threadId":"thread","terminalId":"term","cwd":"/repo","worktreePath":null,"status":"running","pid":123,"history":history,"exitCode":null,"exitSignal":null,"label":"Terminal 1","updatedAt":"2026-04-01T00:00:00.000Z"}),
    )
}
fn attach_event(kind: &str, fields: Value) -> TerminalAttachStreamEvent {
    let mut value = json!({"type":kind,"threadId":"thread","terminalId":"term"});
    value
        .as_object_mut()
        .unwrap()
        .extend(fields.as_object().unwrap().clone());
    decode(value)
}
fn metadata(kind: &str, bytes: Option<u64>) -> Option<FilesystemEntryMetadata> {
    Some(decode(json!({"kind":kind,"byteLength":bytes.unwrap_or(1)})))
}
fn asset(n: u64, revision: Option<&str>) -> AssetCreateUrlResult {
    let mut asset = json!({"relativeUrl":format!("/asset/{n}"),"expiresAt":3_600_000});
    if let Some(revision) = revision {
        asset["fileRevision"] = json!(revision);
    }
    decode(asset)
}
fn usage_summary(refreshing: &[&str]) -> UsageSummary {
    decode(
        json!({"contractVersion":6,"readAt":"2026-09-05T12:00:00Z","timeZone":"UTC","sinceDay":"2026-09-05","untilDay":"2026-09-05","buckets":[],"sources":refreshing.iter().map(|provider| json!({"fingerprint":{"hostId":"host","provider":provider,"resolvedHomePath":"/home","volumeId":"1:1"},"status":"ok","scannedFiles":1,"skippedFiles":0,"malformedRecords":0,"distinctSessions":1,"message":null,"refreshing":true})).collect::<Vec<_>>(),"pricing":{"status":"fresh","source":"test","fetchedAt":null,"knownModels":1},"scanDurationMs":1}),
    )
}
struct NumberMethod;
impl RpcMethod for NumberMethod {
    type Input = u64;
    type Output = u64;
    const NAME: &'static str = "test.number";
}
#[derive(Default)]
struct DeferredRpc {
    replies: RefCell<VecDeque<oneshot::Receiver<Value>>>,
}
impl DeferredRpc {
    fn enqueue(&self) -> oneshot::Sender<Value> {
        let (tx, rx) = oneshot::channel();
        self.replies.borrow_mut().push_back(rx);
        tx
    }
}
impl RpcEndpoint for DeferredRpc {
    fn call<M: RpcMethod>(
        &self,
        _input: M::Input,
    ) -> LocalBoxFuture<'_, Result<M::Output, ClientError>> {
        let reply = self.replies.borrow_mut().pop_front().unwrap();
        async move {
            Ok(serde_json::from_value(
                reply.await.map_err(|_| ClientError::Cancelled)?,
            )?)
        }
        .boxed_local()
    }
    fn subscribe<M: RpcMethod>(
        &self,
        _: M::Input,
    ) -> LocalBoxStream<'static, Result<M::Output, ClientError>> {
        futures::stream::empty::<Value>()
            .map(|value| Ok(serde_json::from_value(value)?))
            .boxed_local()
    }
}

#[test]
fn resource_refresh_preserves_value_and_equal_answers_share_identity() {
    let resource = Resource::new(ResourcePolicy::default(), Generation(1));
    let _reader = resource.observe(0);
    assert!(resource.needs_refresh(0));
    let first = resource.begin();
    resource.finish(first, json!({"answer":1}), 0, false);
    let previous = resource.snapshot().value.unwrap();
    assert!(!resource.needs_refresh(29_999));
    assert!(resource.needs_refresh(30_000));
    let next = resource.begin();
    assert!(resource.snapshot().loading);
    assert_eq!(
        resource.snapshot().value.as_deref(),
        Some(previous.as_ref())
    );
    resource.finish(next, json!({"answer":1}), 30_000, false);
    assert!(Rc::ptr_eq(
        &previous,
        resource.snapshot().value.as_ref().unwrap()
    ));
    let failed = resource.begin();
    resource.fail(failed, &ClientError::Disconnected, 31_000);
    assert_eq!(
        resource.snapshot().value.as_deref(),
        Some(previous.as_ref())
    );
    assert!(!resource.snapshot().loading);
}
#[test]
fn resource_provisional_values_do_not_stay_waiting_after_synchronous_answer() {
    let resource = Resource::new(ResourcePolicy::default(), Generation(1));
    let token = resource.begin();
    resource.emit(token, 1);
    assert!(resource.snapshot().loading);
    resource.finish(token, 2, 0, false);
    assert!(!resource.snapshot().loading);
    assert_eq!(resource.snapshot().value.as_deref(), Some(&2));
}
#[test]
fn resource_generation_and_invalidation_reject_late_answers() {
    let resource = Resource::new(ResourcePolicy::default(), Generation(1));
    let token = resource.begin();
    resource.replace_session(Generation(2));
    assert!(!resource.finish(token, 1, 0, false));
    let token = resource.begin();
    resource.invalidate();
    assert!(!resource.finish(token, 2, 0, false));
    assert!(resource.snapshot().value.is_none());
}
#[test]
fn resource_observers_coalesce_and_idle_invalidation_waits_for_demand() {
    let resource = Resource::new(ResourcePolicy::default(), Generation(1));
    let mut reader = resource.observe(0);
    let _initial = reader.next().now_or_never().unwrap();
    for value in 0..5 {
        let token = resource.begin();
        resource.finish(token, value, 0, false);
    }
    assert_eq!(
        reader.next().now_or_never(),
        Some(Some(resource.snapshot().revision))
    );
    assert!(reader.next().now_or_never().is_none());
    reader.tick(100);
    drop(reader);
    resource.invalidate();
    assert!(!resource.needs_refresh(101));
    let _reader = resource.observe(101);
    assert!(resource.needs_refresh(101));
}
#[test]
fn resource_completed_values_stop_refreshing_and_have_a_separate_ttl() {
    let resource = Resource::new(
        ResourcePolicy {
            idle_ttl_ms: 10,
            refresh_ms: Some(1),
            completed_ttl_ms: Some(20),
            ..Default::default()
        },
        Generation(1),
    );
    let mut reader = resource.observe(0);
    let token = resource.begin();
    resource.finish(token, 1, 0, true);
    assert!(!resource.needs_refresh(100));
    assert!(!resource.interval_due(100));
    reader.tick(10);
    drop(reader);
    assert!(!resource.expired(29));
    assert!(resource.expired(30));
}
#[test]
fn dropped_requests_release_loading_and_session_changes_cancel_the_rpc() {
    let platform = FakePlatform::default();
    let endpoint = DeferredRpc::default();
    let _tx = endpoint.enqueue();
    let resource = Resource::new(ResourcePolicy::default(), Generation(1));
    let mut request = Box::pin(resource.execute::<NumberMethod, _, _>(&endpoint, &platform, 0));
    assert!(request.as_mut().now_or_never().is_none());
    assert!(resource.snapshot().loading);
    drop(request);
    assert!(!resource.snapshot().loading);
    let tx = endpoint.enqueue();
    let mut request = Box::pin(resource.execute::<NumberMethod, _, _>(&endpoint, &platform, 0));
    assert!(request.as_mut().now_or_never().is_none());
    resource.replace_session(Generation(2));
    assert!(matches!(block_on(request), Err(ClientError::Cancelled)));
    assert!(tx.send(json!(4)).is_err());
}
#[test]
fn server_answer_hook_runs_before_publishing_and_not_for_provisional_values() {
    let platform = FakePlatform::default();
    let endpoint = ScriptedRpc::default();
    endpoint.scripts.borrow_mut().push_back(RpcScript {
        method: "test.number".into(),
        input: json!(0),
        replies: vec![Ok(json!(4))],
    });
    let resource = Resource::new(ResourcePolicy::default(), Generation(1));
    block_on(
        resource.execute_with::<NumberMethod, _, _>(&endpoint, &platform, 0, |value| {
            assert_eq!(*value, 4);
            assert!(resource.snapshot().value.is_none());
        }),
    )
    .unwrap();
    assert_eq!(resource.snapshot().value.as_deref(), Some(&4));
}
#[test]
fn resource_keys_keep_environment_and_arguments_separate() {
    let mut registry = ResourceRegistry::default();
    let first = registry
        .query::<NumberMethod>(env("a"), &1, ResourcePolicy::default())
        .unwrap();
    let other = registry
        .query::<NumberMethod>(env("b"), &1, ResourcePolicy::default())
        .unwrap();
    let same = registry
        .query::<NumberMethod>(env("a"), &1, ResourcePolicy::default())
        .unwrap();
    let token = first.begin();
    first.finish(token, 7, 0, false);
    assert_eq!(same.snapshot().value.as_deref(), Some(&7));
    assert!(other.snapshot().value.is_none());
    let stale = first.begin();
    registry.remove_environment(&env("a"));
    assert!(!first.finish(stale, 8, 0, false));
}
#[test]
fn query_persistence_is_typed_scoped_and_ignores_corruption() {
    block_on(async {
        let platform = FakePlatform::default();
        let cache = QueryPersistence {
            platform: &platform,
        };
        cache
            .save(env("one"), "open".into(), &json!({"title":"saved"}))
            .await
            .unwrap();
        assert_eq!(
            cache
                .load::<Value>(env("one"), "open".into())
                .await
                .unwrap(),
            Some(json!({"title":"saved"}))
        );
        assert!(
            cache
                .load::<Value>(env("two"), "open".into())
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            cache
                .load::<Value>(env("one"), "closed".into())
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            cache
                .load::<u64>(env("one"), "open".into())
                .await
                .unwrap()
                .is_none()
        );
        platform
            .write(
                StorageArea::Ordinary,
                vec![StorageWrite {
                    key: StorageKey {
                        namespace: "query".into(),
                        environment: Some(env("one")),
                        record: "broken".into(),
                    },
                    value: Some(b"not json".to_vec()),
                }],
            )
            .await
            .unwrap();
        assert!(
            cache
                .load::<Value>(env("one"), "broken".into())
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            query_persistence_key(&json!({"cursor":null}), "query").unwrap(),
            "[\"query\",{\"cursor\":null}]"
        );
    });
}

#[test]
fn config_updates_without_a_snapshot_do_not_create_partial_state() {
    let event = decode(json!({"version":1,"type":"providerStatuses","payload":{"providers":[]}}));
    assert!(
        apply_server_config_projection(None, event)
            .unwrap()
            .is_none()
    );
}
#[test]
fn config_carries_published_machine_state_across_snapshots_and_strips_it_from_cache() {
    let mut config = config();
    config.environment_themes = Some(Some(vec![decode(
        json!({"id":"nightfall","name":"Nightfall","appearance":"dark","canvas":"#1a1b26","accent":"#7aa2f7"}),
    )]));
    let first = apply_server_config_projection(None, snapshot(config))
        .unwrap()
        .unwrap();
    let next = apply_server_config_projection(Some(&first), snapshot(self::config()))
        .unwrap()
        .unwrap();
    assert_eq!(
        next.config.environment_themes,
        first.config.environment_themes
    );
    assert!(cacheable_config(&next.config).environment_themes.is_none());
    assert!(cacheable_config(&next.config).usage_limit_sources.is_none());
    let cleared = apply_server_config_projection(
        Some(&next),
        decode(json!({"version":1,"type":"environmentThemesUpdated","payload":{"themes":[]}})),
    )
    .unwrap()
    .unwrap();
    assert!(cleared.config.environment_themes.is_none());
}
#[test]
fn config_reuses_equal_roots_and_keeps_held_catalogs_across_digests() {
    let mut config = config();
    config.providers.0.push(provider("codex", json!({"catalogDigest":"d1","loadedCatalogDigest":"d1","slashCommands":[{"name":"compact"}],"skills":[{"name":"review","path":"/review","enabled":true}]})));
    let first = apply_server_config_projection(None, snapshot(config.clone()))
        .unwrap()
        .unwrap();
    let same = apply_server_config_projection(Some(&first), snapshot(config.clone()))
        .unwrap()
        .unwrap();
    assert!(Rc::ptr_eq(&first.config, &same.config));
    let changed = provider("codex", json!({"catalogDigest":"d2"}));
    config.providers.0 = vec![changed.clone()];
    let next = apply_server_config_projection(Some(&first), snapshot(config))
        .unwrap()
        .unwrap();
    assert_eq!(
        next.config.providers.0[0].skills,
        first.config.providers.0[0].skills
    );
    assert_eq!(
        next.config.providers.0[0]
            .loaded_catalog_digest
            .as_ref()
            .unwrap()
            .0,
        "d1"
    );
    let mut delta = serde_json::to_value(changed).unwrap();
    for field in ["models", "skills", "slashCommands"] {
        delta.as_object_mut().unwrap().remove(field);
    }
    let next = apply_server_config_projection(
        Some(&next),
        decode(json!({"version":1,"type":"providerStatusDeltas","payload":{"providers":[delta]}})),
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        next.config.providers.0[0].skills,
        first.config.providers.0[0].skills
    );
    let without_base = apply_provider_deltas(&[], &[decode(delta)]).unwrap();
    assert!(without_base.is_empty());
}
#[test]
fn config_settings_equal_updates_keep_identity_and_version_selects_initial_config() {
    let mut first = apply_server_config_projection(None, snapshot(config()))
        .unwrap()
        .unwrap();
    let next = apply_server_config_projection(Some(&first), decode(json!({"version":1,"type":"settingsUpdated","payload":{"settings":first.config.settings}}))).unwrap().unwrap();
    assert!(Rc::ptr_eq(&first.config, &next.config));
    let mut initial = config();
    initial.environment.server_version.0 = "2".into();
    assert_eq!(
        resolve_server_config(Some(&first), Some(&initial))
            .unwrap()
            .environment
            .server_version
            .0,
        "2"
    );
    first.source = ConfigSource::Cache;
    assert!(std::ptr::eq(
        resolve_server_config(Some(&first), Some(&initial)).unwrap(),
        &initial
    ));
    assert!(std::ptr::eq(
        resolve_server_config(Some(&next), None).unwrap(),
        next.config.as_ref()
    ));
}
#[test]
fn workspace_catalogs_are_retired_by_basis_changes_but_not_health_or_catalog_loading() {
    let instance = ProviderInstanceId("codex".into());
    let snapshot: ServerProviderWorkspaceSnapshot = decode(
        json!({"cwd":"/a","checkedAt":"2026-04-01T00:00:00Z","slashCommands":[],"skills":[]}),
    );
    let base = provider("codex", json!({"catalogDigest":"d1"}));
    let mut response = base.clone();
    response.workspace_snapshots = Some(vec![snapshot.clone()]);
    let mut fetched = Vec::new();
    assert!(record_workspace_snapshot(
        &mut fetched,
        &instance,
        "/a",
        false,
        &[response.clone()],
        Some(&base)
    ));
    assert!(!record_workspace_snapshot(
        &mut fetched,
        &instance,
        "/a",
        false,
        &[response],
        Some(&base)
    ));
    let mut config = config();
    config
        .environment
        .capabilities
        .provider_workspace_snapshots_on_demand = Some(true);
    config.providers.0 = vec![base.clone()];
    assert_eq!(
        with_workspace_snapshots(&config, &fetched).providers.0[0].workspace_snapshots,
        Some(vec![snapshot.clone()])
    );
    config.providers.0[0].slash_commands = vec![decode(json!({"name":"loaded"}))];
    assert_eq!(
        with_workspace_snapshots(&config, &fetched).providers.0[0].workspace_snapshots,
        Some(vec![snapshot])
    );
    config.providers.0[0].catalog_digest.as_mut().unwrap().0 = "d2".into();
    assert!(
        with_workspace_snapshots(&config, &fetched).providers.0[0]
            .workspace_snapshots
            .is_none()
    );
    config
        .environment
        .capabilities
        .provider_workspace_snapshots_on_demand = Some(false);
    assert_eq!(with_workspace_snapshots(&config, &fetched), config);
    assert!(record_workspace_snapshot(
        &mut fetched,
        &instance,
        "/a",
        true,
        &[],
        Some(&base)
    ));
    assert!(fetched.is_empty());
}
#[test]
fn fetched_machine_catalogs_apply_only_when_the_live_digest_matches() {
    let service = ServerConfigService::new(env("one"));
    service.begin_session(Generation(1));
    service.record_catalog(
        Generation(1),
        FetchedMachineCatalog {
            instance_id: ProviderInstanceId("codex".into()),
            digest: "d2".into(),
            slash_commands: vec![decode(json!({"name":"compact"}))],
            skills: vec![],
        },
    );
    let mut config = config();
    config.providers.0 = vec![provider("codex", json!({"catalogDigest":"d1"}))];
    service
        .apply(Generation(1), snapshot(config.clone()))
        .unwrap();
    assert!(
        service.snapshot().unwrap().config.providers.0[0]
            .slash_commands
            .is_empty()
    );
    config.providers.0[0].catalog_digest.as_mut().unwrap().0 = "d2".into();
    service.apply(Generation(1), snapshot(config)).unwrap();
    assert_eq!(
        service.snapshot().unwrap().config.providers.0[0].slash_commands[0]
            .name
            .0,
        "compact"
    );
    assert!(!service.record_catalog(
        Generation(0),
        FetchedMachineCatalog {
            instance_id: ProviderInstanceId("codex".into()),
            digest: "d3".into(),
            slash_commands: vec![],
            skills: vec![]
        }
    ));
}
#[test]
fn config_cache_persists_latest_projection_and_late_restore_never_replaces_live() {
    block_on(async {
        let platform = FakePlatform::default();
        let service = ServerConfigService::new(env("one"));
        service.restore(config());
        service.begin_session(Generation(1));
        let mut config = config();
        config.cwd.0 = "/latest".into();
        service
            .apply(Generation(1), snapshot(config.clone()))
            .unwrap();
        service.flush(&platform).await.unwrap();
        let restored = ServerConfigService::new(env("one"));
        assert!(restored.load(&platform).await.unwrap());
        assert_eq!(restored.snapshot().unwrap().config.cwd.0, "/latest");
        assert!(!service.restore(self::config()));
        service.begin_session(Generation(2));
        assert!(
            !service
                .apply(Generation(1), snapshot(self::config()))
                .unwrap()
        );
        assert!(
            !ServerConfigService::new(env("other"))
                .load(&platform)
                .await
                .unwrap()
        );
    });
}

#[test]
fn terminal_output_keeps_unicode_byte_tail_and_utf16_cursor_offsets() {
    let mut output = TerminalOutputState::default();
    output.reset("é界🙂end", 12);
    let mut cursor = output.read(TerminalOutputCursor::default()).cursor();
    assert_eq!(cursor.offset, 7);
    for (data, expected, bytes) in [
        ("x", "界🙂endx", 11),
        ("yz", "🙂endxyz", 10),
        ("abc", "endxyzabc", 9),
    ] {
        output.append(data, 12);
        let update = output.read(cursor);
        assert_eq!(output.text(), expected);
        assert_eq!(output.retained_bytes, bytes);
        assert!(
            matches!(&update, TerminalOutputUpdate::Append { data: appended, .. } if appended == data)
        );
        cursor = update.cursor();
    }
}
#[test]
fn terminal_buffer_replays_unread_output_and_resets_only_cursors_that_fell_behind() {
    let mut output = TerminalOutputState::default();
    output.reset("", 512);
    let stale = output.read(TerminalOutputCursor::default()).cursor();
    output.append("hello", 512);
    let caught_up = output.read(stale).cursor();
    output.append(" world", 8);
    assert!(
        matches!(output.read(caught_up), TerminalOutputUpdate::Append { data, .. } if data == " world")
    );
    assert!(
        matches!(output.read(stale), TerminalOutputUpdate::Reset { data, .. } if data == "lo world")
    );
    assert_eq!(output.retained_bytes, 8);
    let cursor = output.read(stale).cursor();
    for data in [" one", " two", " three"] {
        output.append(data, 512);
    }
    let update = output.read(cursor);
    assert!(
        matches!(&update, TerminalOutputUpdate::Append { data, .. } if data == " one two three")
    );
    assert!(matches!(
        output.read(update.cursor()),
        TerminalOutputUpdate::None(_)
    ));
}
#[test]
fn terminal_chunks_preserve_bom_and_compaction_preserves_every_unread_character() {
    let mut output = TerminalOutputState::default();
    output.reset("discard\u{feff}tail", 7);
    assert_eq!(output.text(), "\u{feff}tail");
    output.reset("", 500_000);
    let cursor = output.read(TerminalOutputCursor::default()).cursor();
    let text = format!("{}\u{feff}{}", "x".repeat(16_384), "y".repeat(20));
    output.append(&text, 500_000);
    assert_eq!(output.text(), text);
    assert_eq!(output.retained_bytes, text.len());
    assert!(
        matches!(output.read(cursor), TerminalOutputUpdate::Append { data, .. } if data == text)
    );
    output.reset("", 500_000);
    let mut cursor = output.read(TerminalOutputCursor::default()).cursor();
    let mut expected = String::new();
    for index in 0..1200 {
        let text = ["x", "é", "界", "🙂"][index % 4];
        output.append(text, 500_000);
        if index == 99 {
            cursor = output.read(cursor).cursor();
        }
        if index >= 100 {
            expected.push_str(text);
        }
    }
    assert!(output.chunks.len() <= 1024);
    assert!(
        matches!(output.read(cursor), TerminalOutputUpdate::Append { data, .. } if data == expected)
    );
}
#[test]
fn terminal_reattach_restart_and_fresh_seeds_reset_while_clear_erases_in_place() {
    let mut generations = AttachGenerations::default();
    let mut buffer = generations.seed();
    let snapshot = attach_event("snapshot", json!({"snapshot":terminal_snapshot("hello")}));
    buffer.apply(&snapshot, 100, &mut generations);
    let cursor = buffer.output.read(TerminalOutputCursor::default()).cursor();
    let initial = buffer.output.generation;
    buffer.apply(&snapshot, 100, &mut generations);
    assert_ne!(buffer.output.generation, initial);
    assert_eq!(buffer.lifecycle_version, 1);
    assert!(matches!(
        buffer.output.read(cursor),
        TerminalOutputUpdate::Reset { .. }
    ));
    let cursor = buffer.output.read(cursor).cursor();
    buffer.apply(&attach_event("cleared", json!({})), 100, &mut generations);
    assert!(
        matches!(buffer.output.read(cursor), TerminalOutputUpdate::Append { data, .. } if data == CLEAR_TERMINAL_TEXT)
    );
    assert_eq!(
        buffer.output.replay(),
        format!("hello{CLEAR_TERMINAL_TEXT}")
    );
    assert_eq!(buffer.output.text(), "");
    buffer.apply(
        &attach_event("restarted", json!({"snapshot":terminal_snapshot("hello")})),
        100,
        &mut generations,
    );
    assert_eq!(buffer.lifecycle_version, 2);
    let mut fresh = generations.seed();
    fresh.apply(&snapshot, 100, &mut generations);
    assert_ne!(fresh.output.generation, initial);
    assert_eq!(fresh.lifecycle_version, 0);
}
#[test]
fn terminal_grid_activity_does_not_change_output_and_live_status_beats_metadata() {
    let mut generations = AttachGenerations::default();
    let mut buffer = generations.seed();
    let mut snapshot = terminal_snapshot("hello");
    snapshot.cols = Some(Some(130));
    snapshot.rows = Some(Some(15));
    buffer.apply(
        &attach_event("snapshot", json!({"snapshot":snapshot})),
        100,
        &mut generations,
    );
    let output = buffer.output.clone();
    buffer.apply(
        &attach_event(
            "activity",
            json!({"hasRunningSubprocess":false,"label":"Term","cols":54,"rows":15}),
        ),
        100,
        &mut generations,
    );
    assert_eq!(buffer.grid, Some(TerminalGrid { cols: 54, rows: 15 }));
    assert_eq!(buffer.output, output);
    buffer.apply(
        &attach_event("error", json!({"message":"Terminal disconnected."})),
        100,
        &mut generations,
    );
    let summary: TerminalSummary = decode(
        json!({"threadId":"thread","terminalId":"term","cwd":"/repo","worktreePath":null,"status":"running","pid":123,"exitCode":null,"exitSignal":null,"updatedAt":"2026-04-01T00:00:00.000Z","hasRunningSubprocess":false,"label":"Term"}),
    );
    let session = combine_terminal_session(Some(summary), buffer);
    assert_eq!(session.status, TerminalStatus::Error);
    assert_eq!(
        session.buffer.error.as_deref(),
        Some("Terminal disconnected.")
    );
}
#[test]
fn terminal_sessions_resize_before_input_replay_on_generation_and_report_live_restart() {
    let grid = TerminalGrid {
        cols: 120,
        rows: 40,
    };
    let mut session = TerminalSession::new(grid, Some("pnpm dev\r".into()));
    let mut buffer = TerminalBufferState {
        status: TerminalStatus::Running,
        version: 1,
        ..Default::default()
    };
    buffer.output.generation = 1;
    assert_eq!(
        session.attachment(&buffer, false, true, true),
        vec![
            TerminalSessionAction::LiveChanged(true),
            TerminalSessionAction::Resize(grid),
            TerminalSessionAction::Write("pnpm dev\r".into())
        ]
    );
    assert!(session.attachment(&buffer, false, true, true).is_empty());
    let grid = TerminalGrid {
        cols: 100,
        rows: 30,
    };
    assert_eq!(
        session.report_grid(grid, true),
        Some(TerminalSessionAction::Resize(grid))
    );
    assert_eq!(session.report_grid(grid, true), None);
    buffer.output.generation = 2;
    assert_eq!(
        session.attachment(&buffer, false, true, true),
        vec![TerminalSessionAction::Resize(grid)]
    );
    buffer.lifecycle_version = 1;
    assert_eq!(
        session.attachment(&buffer, false, true, true),
        vec![TerminalSessionAction::LiveChanged(true)]
    );
    buffer.status = TerminalStatus::Exited;
    assert_eq!(
        session.attachment(&buffer, false, true, true),
        vec![
            TerminalSessionAction::LiveChanged(false),
            TerminalSessionAction::CloseAndReportExit
        ]
    );
    assert!(session.attachment(&buffer, false, true, true).is_empty());
}
#[test]
fn terminal_lifecycle_preserves_observed_history_after_permission_grants() {
    let ended = TerminalBufferState {
        status: TerminalStatus::Exited,
        version: 1,
        ..Default::default()
    };
    let grid = TerminalGrid { cols: 80, rows: 24 };
    let mut observed = TerminalSession::new(grid, None);
    observed.observe();
    assert!(observed.attachment(&ended, false, true, true).is_empty());
    let mut revoked = TerminalSession::new(grid, None);
    let running = TerminalBufferState {
        status: TerminalStatus::Running,
        version: 1,
        ..Default::default()
    };
    revoked.attachment(&running, false, true, true);
    revoked.detach(false);
    assert!(revoked.attachment(&ended, false, true, true).is_empty());
    let mut detached = TerminalSession::new(grid, None);
    detached.attachment(&running, false, true, true);
    detached.detach(true);
    assert_eq!(
        detached.attachment(&ended, false, true, true),
        vec![TerminalSessionAction::Reopen]
    );
    let mut explicit = TerminalSession::new(grid, None);
    assert_eq!(
        explicit.attachment(&ended, false, true, true),
        vec![TerminalSessionAction::Reopen]
    );
    assert!(explicit.attachment(&ended, false, true, true).is_empty());
    explicit
        .lifecycle
        .transition(TerminalLifecycleEvent::ReopenFailed);
    assert_eq!(
        explicit.attachment(&ended, false, true, true),
        vec![TerminalSessionAction::Reopen]
    );
}
#[test]
fn terminal_attach_retries_initial_input_unchanged_until_the_first_reply() {
    let attach: TerminalAttachInput =
        decode(json!({"threadId":"thread","terminalId":"term","cwd":"/repo"}));
    let mut resume = TerminalAttachResume::default();
    assert!(resume.input(attach.clone()).resume.is_none());
    assert!(resume.input(attach.clone()).resume.is_none());
    resume.received();
    assert_eq!(resume.input(attach).resume, Some(Some(true)));
}

#[test]
fn metadata_batches_deduplicate_bound_size_and_do_not_mix_environments() {
    let service = FileMetadataService::default();
    service.begin_session(env("one"), Generation(1));
    service.begin_session(env("two"), Generation(1));
    for index in 0..130 {
        service.enqueue(env("one"), &format!("/file/{index}"), None, 0);
    }
    let other = service.enqueue(env("two"), "/file/1", None, 0);
    service.enqueue(env("one"), "/file/1", None, 0);
    let mut sizes = Vec::new();
    while let Some(batch) = service.next_batch(&env("one")) {
        sizes.push(batch.paths.len());
        let count = batch.paths.len();
        service.complete(batch, vec![None; count], 0);
    }
    assert_eq!(sizes, vec![64, 64, 2]);
    assert!(other.snapshot().value.is_none());
    let batch = service.next_batch(&env("two")).unwrap();
    assert_eq!(batch.paths, vec!["/file/1"]);
    service.complete(batch, vec![metadata("file", Some(8))], 0);
    assert_eq!(
        other
            .snapshot()
            .value
            .unwrap()
            .as_ref()
            .as_ref()
            .unwrap()
            .byte_length,
        Some(8)
    );
    service.enqueue(env("one"), "/file/1", None, 100);
    assert!(service.next_batch(&env("one")).is_none());
}
#[test]
fn metadata_invalidation_retries_only_changed_paths_and_guards_read_seeding() {
    let service = FileMetadataService::default();
    service.begin_session(env("one"), Generation(1));
    let first = service.enqueue(env("one"), "/a", None, 0);
    let second = service.enqueue(env("one"), "/b", None, 0);
    let token = service.seed_token(&env("one")).unwrap();
    let batch = service.next_batch(&env("one")).unwrap();
    service.invalidate(&env("one"), "/a");
    service.complete(
        batch,
        vec![metadata("file", Some(1)), metadata("file", Some(2))],
        0,
    );
    assert!(first.snapshot().value.is_none());
    assert!(second.snapshot().value.is_some());
    let batch = service.next_batch(&env("one")).unwrap();
    assert_eq!(batch.paths, vec!["/a"]);
    service.complete(batch, vec![metadata("file", Some(3))], 0);
    assert!(!service.remember_file(&env("one"), token, "/", "a", 99, 0));
    assert_eq!(
        first
            .snapshot()
            .value
            .unwrap()
            .as_ref()
            .as_ref()
            .unwrap()
            .byte_length,
        Some(3)
    );
    service.invalidate(&env("one"), "/a");
    assert!(service.next_batch(&env("one")).is_none());
    service.enqueue(env("one"), "/a", None, 0);
    assert_eq!(service.next_batch(&env("one")).unwrap().paths, vec!["/a"]);
}
#[test]
fn retained_metadata_keeps_known_entries_but_misses_expire_and_reconnect_discards_both() {
    let service = FileMetadataService::default();
    service.begin_session(env("one"), Generation(1));
    let key = thread("one", "thread");
    let _retained = service.retain_thread(key.clone());
    let known = service.enqueue(env("one"), "/known", Some(&key), 0);
    let missing = service.enqueue(env("one"), "/missing", Some(&key), 0);
    let batch = service.next_batch(&env("one")).unwrap();
    service.complete(batch, vec![metadata("file", Some(5)), None], 0);
    assert!(known.snapshot().value.is_some());
    assert_eq!(missing.snapshot().value.as_deref(), Some(&None));
    service.enqueue(env("one"), "/known", Some(&key), 60_001);
    service.enqueue(env("one"), "/missing", Some(&key), 60_001);
    assert_eq!(
        service.next_batch(&env("one")).unwrap().paths,
        vec!["/missing"]
    );
    service.begin_session(env("one"), Generation(2));
    service.enqueue(env("one"), "/known", Some(&key), 60_001);
    assert_eq!(
        service.next_batch(&env("one")).unwrap().paths,
        vec!["/known"]
    );
}
#[test]
fn metadata_search_read_seeds_preserve_literal_colons_windows_keys_and_richer_values() {
    let service = FileMetadataService::default();
    service.begin_session(env("one"), Generation(1));
    let token = service.seed_token(&env("one")).unwrap();
    service.remember_entries(
        &env("one"),
        token,
        "/workspace",
        [("report:123".into(), FilesystemEntryKind::Directory)],
        0,
    );
    let known = service.enqueue(env("one"), "/workspace/report:123", None, 0);
    assert_eq!(
        known
            .snapshot()
            .value
            .unwrap()
            .as_ref()
            .as_ref()
            .unwrap()
            .kind,
        FilesystemEntryKind::Directory
    );
    service.enqueue(env("one"), "/workspace/report", None, 0);
    assert_eq!(
        service.next_batch(&env("one")).unwrap().paths,
        vec!["/workspace/report"]
    );
    service.remember_file(&env("one"), token, "C:\\Work", "Dir/File.txt", 42, 0);
    let known = service.enqueue(env("one"), "c:/work/dir/file.txt", None, 0);
    assert_eq!(
        known
            .snapshot()
            .value
            .unwrap()
            .as_ref()
            .as_ref()
            .unwrap()
            .byte_length,
        Some(42)
    );
    assert_eq!(normalize_path("C:/Work/"), "c:\\work");
    assert_eq!(
        workspace_file_path("/repo", "literal:12:3"),
        "/repo/literal:12:3"
    );
}
#[test]
fn failed_metadata_batches_settle_and_retry_after_access_is_granted() {
    let service = FileMetadataService::default();
    service.begin_session(env("one"), Generation(1));
    let resource = service.enqueue(env("one"), "/a", None, 0);
    let batch = service.next_batch(&env("one")).unwrap();
    service.fail(
        batch,
        &ClientError::PermissionDenied {
            method: "filesystem.getMetadata".into(),
        },
        0,
    );
    assert!(resource.snapshot().error.is_some());
    assert!(!resource.snapshot().loading);
    service.enqueue(env("one"), "/a", None, 0);
    let batch = service.next_batch(&env("one")).unwrap();
    service.complete(batch, vec![None], 0);
    assert!(resource.snapshot().error.is_none());
    assert_eq!(resource.snapshot().value.as_deref(), Some(&None));
}

#[test]
fn revisioned_assets_share_first_inflight_mint_and_remint_after_edits_and_reverts() {
    let resource: AssetResource =
        decode(json!({"_tag":"workspace-file","threadId":"thread","path":"docs/diagram.png"}));
    let key = asset_resource_key(env("one"), &resource).unwrap();
    let mut revisions = RevisionedAssets::default();
    assert_eq!(
        revisions.read(key.clone(), Some("v1".into()), false),
        AssetRevisionAction::Mint
    );
    revisions.settled(&key, asset(1, None), 0);
    assert_eq!(
        revisions.read(key.clone(), Some("v1".into()), false),
        AssetRevisionAction::Share
    );
    assert_eq!(
        revisions.read(key.clone(), Some("v2".into()), false),
        AssetRevisionAction::Mint
    );
    assert_eq!(
        revisions.read(key.clone(), Some("v1".into()), false),
        AssetRevisionAction::Mint
    );
    let other = asset_resource_key(env("two"), &resource).unwrap();
    assert_eq!(
        revisions.read(other.clone(), Some("v1".into()), true),
        AssetRevisionAction::Share
    );
    assert_eq!(
        revisions.read(other.clone(), Some("v2".into()), true),
        AssetRevisionAction::MintAfterCurrent
    );
    assert!(revisions.settled(&other, asset(2, None), 0).1);
    assert_eq!(
        revisions.read(other, Some("v2".into()), false),
        AssetRevisionAction::Share
    );
}
#[test]
fn unchanged_file_revisions_keep_the_shown_url_until_expiry_is_near() {
    let shown = Rc::new(asset(1, Some("r1")));
    assert!(Rc::ptr_eq(
        &keep_unchanged_file(&shown, asset(2, Some("r1")), 0),
        &shown
    ));
    assert!(!Rc::ptr_eq(
        &keep_unchanged_file(&shown, asset(2, Some("r2")), 0),
        &shown
    ));
    assert!(!Rc::ptr_eq(
        &keep_unchanged_file(&shown, asset(2, Some("r1")), 3_300_000),
        &shown
    ));
    assert!(!Rc::ptr_eq(
        &keep_unchanged_file(&shown, asset(2, None), 0),
        &shown
    ));
}
#[test]
fn favicon_urls_are_scoped_by_environment_workspace_and_selected_icon() {
    let mut cache = ProjectFaviconUrls::default();
    let key = ProjectFaviconKey {
        environment: env("one"),
        cwd: "/workspace".into(),
        path: None,
    };
    let state = t3_client::types::QueryState {
        value: Some(Rc::new(asset(1, None))),
        loading: false,
        error: None,
        revision: 0,
    };
    assert_eq!(
        cache.resolve(key.clone(), &state, Some("https://remote.test")),
        Some("https://remote.test/asset/1")
    );
    let failed = t3_client::types::QueryState {
        value: None,
        loading: false,
        error: Some("disconnected".into()),
        revision: 1,
    };
    assert_eq!(
        cache.resolve(key.clone(), &failed, None),
        Some("https://remote.test/asset/1")
    );
    assert_eq!(
        cache.resolve(
            ProjectFaviconKey {
                environment: env("two"),
                ..key.clone()
            },
            &failed,
            None
        ),
        None
    );
    assert_eq!(
        cache.resolve(
            ProjectFaviconKey {
                cwd: "/other".into(),
                ..key.clone()
            },
            &failed,
            None
        ),
        None
    );
    assert_eq!(
        cache.resolve(
            ProjectFaviconKey {
                path: Some("brand.svg".into()),
                ..key
            },
            &failed,
            None
        ),
        None
    );
}
#[test]
fn asset_access_requires_file_grants_only_for_host_file_resources() {
    let resource: AssetResource =
        decode(json!({"_tag":"workspace-file","threadId":"thread","path":"a.png"}));
    let pending = ReadAccess {
        allowed: false,
        pending: true,
        error: None,
    };
    assert_eq!(
        blocked_asset_url(Some(&resource), &pending),
        Some(AssetUrlState::Loading)
    );
    let denied = ReadAccess {
        pending: false,
        ..pending
    };
    assert_eq!(
        blocked_asset_url(Some(&resource), &denied),
        Some(AssetUrlState::Failure {
            missing_file_access: true
        })
    );
    let attachment: AssetResource = decode(json!({"_tag":"attachment","attachmentId":"a"}));
    assert_eq!(blocked_asset_url(Some(&attachment), &denied), None);
    assert!(parse_asset_collection_key("not-json").is_err());
    assert!(parse_asset_collection_key("[42,[]]").is_err());
    assert_eq!(
        resolve_asset_url("https://remote.test/root/", "../a"),
        Some("https://remote.test/a".into())
    );
}
#[test]
fn local_asset_fallback_keeps_relative_paths_and_host_only_media_on_the_remote() {
    let error = decode(
        json!({"_tag":"AssetWorkspaceAssetNotFoundError","resource":{"_tag":"media-file","threadId":"thread","path":"docs/report.png"}}),
    );
    for (path, host_only, expected) in [
        ("docs/report.png", false, false),
        ("/repo/docs/report.png", false, true),
        ("C:\\repo\\report.png", false, true),
        ("/report.png", true, false),
        ("/file.mp3", false, false),
        ("/file.txt", false, false),
    ] {
        let mut resource = json!({"_tag":"media-file","threadId":"thread","path":path});
        if host_only {
            resource["hostOnly"] = json!(true);
        }
        assert_eq!(
            can_fallback_to_local_media(&decode(resource), &error),
            expected
        );
    }
}
#[test]
fn local_limiters_are_fair_independent_and_release_slots_when_cancelled() {
    let limit = LocalLimiter::new(2);
    let first = block_on(limit.acquire());
    let second = block_on(limit.acquire());
    let mut waiting = Box::pin(limit.acquire());
    assert!(waiting.as_mut().now_or_never().is_none());
    let other = LocalLimiter::new(1);
    let _other = block_on(other.acquire());
    drop(first);
    let _third = block_on(waiting);
    let mut cancelled = Box::pin(limit.acquire());
    assert!(cancelled.as_mut().now_or_never().is_none());
    drop(cancelled);
    drop(second);
    let _fourth = block_on(limit.acquire());
}
#[test]
fn attachment_limits_and_sizes_round_at_the_display_boundary() {
    assert_eq!(
        clamp_file_attachment_upload_bytes(u64::MAX),
        50 * 1024 * 1024
    );
    for (bytes, label) in [
        (0, "1 KB"),
        (1, "1 KB"),
        (1024, "1 KB"),
        (1_048_064, "1.0 MB"),
        (1_048_576, "1.0 MB"),
    ] {
        assert_eq!(format_attachment_size(bytes), label);
    }
    assert_eq!(
        file_attachment_too_large_message("large.txt", 2 * 1024 * 1024),
        "'large.txt' exceeds the 2 MB attachment limit."
    );
    assert_eq!(
        file_attachment_too_large_message("a", 1),
        "'a' exceeds the 1 byte attachment limit."
    );
    assert!(is_attachment_not_found(
        &json!({"_tag":"AssetAttachmentNotFoundError"})
    ));
    assert!(!is_attachment_not_found(&json!({"_tag":"Other"})));
}
#[test]
fn browse_completion_handles_case_unicode_hidden_entries_and_latest_navigation() {
    let entries = |names: &[&str]| {
        names
            .iter()
            .map(|name| {
                decode::<FilesystemBrowseEntry>(
                    json!({"name":name,"fullPath":format!("/repo/{name}")}),
                )
            })
            .collect::<Vec<_>>()
    };
    for (names, filter, expected) in [
        (vec!["Creations"], "Creation", Some(("Creations", true))),
        (vec!["OpenSourced", "OpenAI"], "op", Some(("Open", false))),
        (vec!["Open", "Opal"], "Op", None),
        (vec!["openAI", "OpenSource"], "o", Some(("open", false))),
        (vec!["😀one", "😁two"], "", None),
        (vec!["ΟΣα", "οσβ"], "ο", None),
        (vec!["İfoo", "İfar"], "i\u{307}", Some(("İf", false))),
        (vec!["😀one", "😀other"], "😀", Some(("😀o", false))),
    ] {
        assert_eq!(
            browse_tab_completion(&entries(&names), filter),
            expected.map(|(name, complete)| (name.into(), complete))
        );
    }
    let entries = entries(&[".git", "Git", "github"]);
    let (visible, exact) = filter_browse_entries(&entries, "Git");
    assert_eq!(visible.len(), 2);
    assert_eq!(exact.unwrap().name.0, "Git");
    let mut navigation = BrowseNavigation::default();
    let first = navigation.begin();
    let second = navigation.begin();
    assert!(!navigation.can_commit(first));
    assert!(navigation.can_commit(second));
    navigation.invalidate();
    assert!(!navigation.can_commit(second));
}
fn lane(group: &str) -> CommandLane {
    CommandLane {
        environment: env("one"),
        group: group.into(),
        arguments: "null".into(),
    }
}
#[test]
fn serial_commands_wait_for_the_active_call_and_release_the_lane_before_outer_work() {
    let scheduler = CommandScheduler::default();
    let (tx, rx) = oneshot::channel();
    let calls = Rc::new(RefCell::new(Vec::new()));
    let first_calls = calls.clone();
    let mut first = scheduler.schedule(lane("serial"), Concurrency::Serial, async move {
        first_calls.borrow_mut().push(1);
        rx.await.unwrap();
        Ok(json!(1))
    });
    let second_calls = calls.clone();
    let mut second = scheduler.schedule(lane("serial"), Concurrency::Serial, async move {
        second_calls.borrow_mut().push(2);
        Ok(json!(2))
    });
    assert!(first.as_mut().now_or_never().is_none());
    assert!(second.as_mut().now_or_never().is_none());
    assert_eq!(*calls.borrow(), vec![1]);
    tx.send(()).unwrap();
    assert_eq!(block_on(first).unwrap().as_ref(), &json!(1));
    assert_eq!(block_on(second).unwrap().as_ref(), &json!(2));
    assert_eq!(*calls.borrow(), vec![1, 2]);
}
#[test]
fn singleflight_shares_results_and_latest_coalesces_only_pending_commands() {
    let scheduler = CommandScheduler::default();
    let (tx, rx) = oneshot::channel();
    let mut first = scheduler.schedule(lane("flight"), Concurrency::SingleFlight, async move {
        rx.await.unwrap();
        Ok(json!(1))
    });
    assert!(first.as_mut().now_or_never().is_none());
    let second = scheduler.schedule(lane("flight"), Concurrency::SingleFlight, async {
        panic!("duplicate singleflight executed");
        #[allow(unreachable_code)]
        Ok(json!(2))
    });
    tx.send(()).unwrap();
    let first = block_on(first).unwrap();
    let second = block_on(second).unwrap();
    assert!(Rc::ptr_eq(&first, &second));
    let (tx, rx) = oneshot::channel();
    let mut active = scheduler.schedule(lane("latest"), Concurrency::Latest, async move {
        rx.await.unwrap();
        Ok(json!(1))
    });
    assert!(active.as_mut().now_or_never().is_none());
    let middle = scheduler.schedule(lane("latest"), Concurrency::Latest, async {
        panic!("replaced pending command executed");
        #[allow(unreachable_code)]
        Ok(json!(2))
    });
    let latest = scheduler.schedule(lane("latest"), Concurrency::Latest, async { Ok(json!(3)) });
    tx.send(()).unwrap();
    assert_eq!(block_on(active).unwrap().as_ref(), &json!(1));
    assert_eq!(block_on(middle).unwrap().as_ref(), &json!(3));
    assert_eq!(block_on(latest).unwrap().as_ref(), &json!(3));
}
#[test]
fn cancelling_all_scheduled_callers_releases_the_inflight_work() {
    for mode in [Concurrency::SingleFlight, Concurrency::Latest] {
        let scheduler = CommandScheduler::default();
        let (tx, rx) = oneshot::channel::<()>();
        let mut call = scheduler.schedule(lane("cancel"), mode, async move {
            rx.await.unwrap();
            Ok(Value::Null)
        });
        assert!(call.as_mut().now_or_never().is_none());
        drop(call);
        assert!(tx.is_canceled(), "{mode:?} retained a cancelled command");
    }
}
#[test]
fn command_policy_keeps_long_running_commands_off_shared_config_and_device_lanes() {
    let (config, mode) = command_lane(env("one"), "server.updateSettings", &json!({})).unwrap();
    assert_eq!(mode, Concurrency::Serial);
    let (run, mode) = command_lane(env("one"), "scheduledTasks.runNow", &json!({})).unwrap();
    assert_eq!(mode, Concurrency::Parallel);
    assert_ne!(config.group, run.group);
    let (first, _) = command_lane(
        env("one"),
        "device.open",
        &json!({"threadId":"a","deviceId":"eas"}),
    )
    .unwrap();
    let (second, _) = command_lane(
        env("one"),
        "device.open",
        &json!({"threadId":"b","deviceId":"eas"}),
    )
    .unwrap();
    assert_ne!(first, second);
    let disable = |provider| {
        command_lane(
            env("one"),
            "server.disableAcpRegistryProvider",
            &json!({"instanceId":"registry","projectId":"project","providerId":provider}),
        )
        .unwrap()
        .0
    };
    assert_ne!(disable("first"), disable("second"));
    assert_eq!(resource_policy("filesystem.browse").stale_ms, 0);
    assert_eq!(
        resource_policy("subscribeDiscoveredLocalServers").idle_ttl_ms,
        0
    );
}

#[test]
fn usage_progress_undims_on_answers_names_slow_sources_and_ignores_offline_environments() {
    let summary = usage_summary(&[]);
    let partial = usage_summary(&["cursor"]);
    let environment = |label, pending| UsageEnvironment {
        label,
        connected: true,
        pending,
        error: None,
        summary: Some(&summary),
    };
    let label = |provider: &t3_contracts::usage::UsageProviderKind| {
        serde_json::to_value(provider)
            .unwrap()
            .as_str()
            .unwrap()
            .to_string()
    };
    assert_eq!(
        usage_progress(
            &[environment("a", true), environment("b", true)],
            false,
            label
        ),
        UsageProgress {
            dimmed: true,
            label: Some("Updating 2 environments…".into())
        }
    );
    assert_eq!(
        usage_progress(
            &[environment("a", false), environment("b", true)],
            false,
            label
        ),
        UsageProgress {
            dimmed: false,
            label: Some("Updating b…".into())
        }
    );
    let partway = UsageEnvironment {
        label: "a",
        connected: true,
        pending: true,
        error: None,
        summary: Some(&partial),
    };
    assert_eq!(
        usage_progress(&[partway.clone(), environment("b", false)], false, label)
            .label
            .as_deref(),
        Some("Updating cursor…")
    );
    let offline = UsageEnvironment {
        connected: false,
        ..environment("offline", true)
    };
    assert_eq!(
        usage_environment_progress(&offline, false),
        UsageEnvironmentProgress::Inactive
    );
    let loading = usage_loading_state(&[environment("a", false), offline], false);
    assert!(!loading.partial);
    let loading = usage_loading_state(&[partway], false);
    assert!(loading.partial);
    assert!(!loading.every_provider);
    assert_eq!(loading.providers.len(), 1);
}
#[test]
fn usage_reads_provisional_plain_then_awaited_and_retains_the_plain_answer_on_failure() {
    block_on(async {
        let platform = FakePlatform::default();
        let endpoint = ScriptedRpc::default();
        let input: UsageSummaryInput =
            decode(json!({"sinceDay":"2026-09-05","untilDay":"2026-09-05","timeZone":"UTC"}));
        let mut provisional = serde_json::to_value(&input).unwrap();
        provisional["provisional"] = json!(true);
        let mut awaited = serde_json::to_value(&input).unwrap();
        awaited["awaitRefresh"] = json!(true);
        let partial = usage_summary(&["cursor"]);
        for (input, reply) in [
            (provisional, Ok(json!(partial))),
            (json!(input), Ok(json!(partial))),
            (awaited, Err(ClientError::Disconnected)),
        ] {
            endpoint.scripts.borrow_mut().push_back(RpcScript {
                method: "server.getUsageSummary".into(),
                input,
                replies: vec![reply],
            });
        }
        let resource = Resource::new(ResourcePolicy::default(), Generation(1));
        read_usage(&resource, &endpoint, &platform, input)
            .await
            .unwrap();
        assert!(!resource.snapshot().loading);
        assert_eq!(resource.snapshot().value.as_deref(), Some(&partial));
        assert!(resource.snapshot().error.is_none());
        assert!(endpoint.scripts.borrow().is_empty());
    });
}
#[test]
fn shared_settings_filter_checks_target_provider_identity_and_enabled_state() {
    let patch: ServerSettingsPatch =
        decode(json!({"continueThreadsAfterServerUpdate":true,"sidebarAutoSettleAfterDays":7}));
    let filtered =
        serde_json::to_value(filter_shared_server_patch(patch, None, None, false).unwrap())
            .unwrap();
    assert!(filtered.get("continueThreadsAfterServerUpdate").is_none());
    assert_eq!(filtered["sidebarAutoSettleAfterDays"], 7);
    for availability in ["missing", "disabled", "different-driver", "enabled"] {
        let mut source = ServerSettings::default();
        let instance = ProviderInstanceId("codex_personal".into());
        source.provider_instances.insert(
            instance.clone(),
            decode(json!({"driver":"codex","enabled":true,"config":{}})),
        );
        let mut target = source.clone();
        match availability {
            "missing" => {
                target.provider_instances.remove(&instance);
            }
            "disabled" => {
                target
                    .provider_instances
                    .get_mut(&instance)
                    .unwrap()
                    .enabled = Some(false)
            }
            "different-driver" => {
                target
                    .provider_instances
                    .get_mut(&instance)
                    .unwrap()
                    .driver
                    .0 = "claudeAgent".into()
            }
            _ => {}
        }
        let patch: ServerSettingsPatch = decode(
            json!({"sidebarAutoSettleAfterDays":7,"textGenerationModelSelection":{"instanceId":"codex_personal","model":"gpt-5.6-luna","options":[{"id":"reasoningEffort","value":"low"}]}}),
        );
        let result = serde_json::to_value(
            filter_shared_server_patch(patch.clone(), Some(&target), Some(&source), false).unwrap(),
        )
        .unwrap();
        assert_eq!(
            result.get("textGenerationModelSelection").is_some(),
            availability == "enabled"
        );
        if availability == "enabled" {
            let result = serde_json::to_value(
                filter_shared_server_patch(patch, Some(&target), None, false).unwrap(),
            )
            .unwrap();
            assert!(result.get("textGenerationModelSelection").is_some());
        }
    }
}
#[test]
fn provider_initials_accents_and_badges_distinguish_accounts_without_splitting_emoji() {
    assert_eq!(
        provider_instance_display_name(&provider("work__--codex", json!({}))),
        "Work Codex"
    );
    for (label, initials) in [
        ("Codex (hub)", "CH"),
        ("work_codex", "WC"),
        ("Theo's Codex", "TC"),
        ("Codex 'hub'", "CH"),
        ("‘hub’", "HU"),
        ("(hub)", "HU"),
        ("😀 Work", "😀W"),
        ("😀", "😀"),
        ("Codex‿hub", "CH"),
        ("Codex+hub", "CO"),
        ("Theo’s Codex", "TC"),
    ] {
        assert_eq!(provider_instance_initials(label), initials);
    }
    assert!(normalize_provider_accent(Some("blue")).is_none());
    for (accent, foreground) in [
        ("#b5f23d", "#000000"),
        ("#2563eb", "#ffffff"),
        ("#777777", "#000000"),
    ] {
        assert_eq!(provider_accent_foreground(accent), foreground);
    }
    let a = InstanceBadge {
        driver: "acpRegistry",
        accent: None,
        acp_agent: Some("mistral"),
        enabled: true,
    };
    let b = InstanceBadge {
        driver: "acpRegistry",
        accent: None,
        acp_agent: Some("devin"),
        enabled: true,
    };
    assert!(!should_show_instance_badge(&a, &[InstanceBadge { ..a }, b]));
    let enabled = InstanceBadge {
        driver: "codex",
        accent: None,
        acp_agent: None,
        enabled: true,
    };
    let disabled = InstanceBadge {
        enabled: false,
        ..enabled
    };
    assert!(!should_show_instance_badge(
        &enabled,
        &[InstanceBadge { ..enabled }, InstanceBadge { ..disabled }]
    ));
    assert!(should_show_instance_badge(
        &disabled,
        &[enabled, InstanceBadge { ..disabled }]
    ));
}
#[test]
fn update_outcomes_correlate_ids_and_reject_rollbacks_or_foreign_outcomes() {
    let result: ServerSelfUpdateResult =
        decode(json!({"targetVersion":"2","method":"boot-service","updateId":"update-1"}));
    let mut descriptor = config().environment;
    descriptor.server_version.0 = "2".into();
    let mut ready: ServerLifecycleStreamReadyEvent = decode(
        json!({"version":1,"sequence":1,"type":"ready","payload":{"at":"2026-08-01T00:00:00Z","environment":descriptor,"updateOutcome":{"id":"update-1","fromVersion":"1","targetVersion":"2","status":"committed"}}}),
    );
    assert!(matches_update_ready(&result, &ready));
    validate_update_ready(&result, &ready).unwrap();
    ready.payload.update_outcome.as_mut().unwrap().status =
        ServerSelfUpdateOutcomeStatus::RolledBack;
    ready.payload.update_outcome.as_mut().unwrap().reason = Some(decode(json!("prepared-timeout")));
    assert!(
        validate_update_ready(&result, &ready)
            .unwrap_err()
            .to_string()
            .contains("prepared-timeout")
    );
    ready.payload.update_outcome.as_mut().unwrap().id.0 = "foreign".into();
    assert!(!matches_update_ready(&result, &ready));
    assert!(matches_update_resume(
        &result,
        &ready,
        Some("older-process")
    ));
    assert!(validate_update_ready(&result, &ready).is_err());
    let no_token: ServerSelfUpdateResult =
        decode(json!({"targetVersion":"3","method":"desktop-app"}));
    assert!(!matches_update_resume(&no_token, &ready, None));
    let token: ServerSelfUpdateResult =
        decode(json!({"targetVersion":"3","method":"desktop-app","desktopUpdateToken":"token"}));
    assert!(matches_update_resume(&token, &ready, None));
}
#[test]
fn update_progress_and_reconnect_observer_do_not_confuse_old_ready_with_a_restart() {
    let event: ServerSelfUpdateProgressEvent = decode(
        json!({"type":"complete","result":{"targetVersion":"2","method":"desktop-app","desktopUpdateToken":"token"}}),
    );
    assert!(matches!(
        update_state_for_progress("1", "2", &event),
        ServerUpdateState::Running {
            stage: ServerUpdateStage::Installing,
            ..
        }
    ));
    let failed = ServerUpdateState::Failed {
        stage: ServerUpdateStage::Installing,
        from_version: "1".into(),
        target_version: "2".into(),
        message: "failed".into(),
    };
    assert_eq!(
        update_state_for_server_version(failed.clone(), Some("1")),
        failed
    );
    assert_eq!(
        update_state_for_server_version(failed, Some("2")),
        ServerUpdateState::Idle
    );
    let mut observer = ReconnectObserver::default();
    assert!(!observer.update(true));
    assert!(!observer.update(false));
    assert!(observer.update(true));
    assert_eq!(timestamp_ms("1970-01-01T00:00:00.000Z"), Some(0));
    assert_eq!(timestamp_ms("1970-01-01T01:00:00+01:00"), Some(0));
    assert!(timestamp_ms("bad").is_none());
}

#[test]
fn browse_path_derives_navigation_without_accepting_windows_paths_on_other_hosts() {
    let path = filesystem_browse_path("~/projects/t3", "", true);
    assert_eq!(path.directory_path, "~/projects/");
    assert_eq!(path.filter_query, "t3");
    assert_eq!(path.parent_path.as_deref(), Some("~/"));
    assert!(path.can_browse_up);
    assert!(!filesystem_browse_path("C:\\Users\\test", "MacIntel", true).browsing);
    assert!(!filesystem_browse_path("~/projects/", "", false).browsing);
    assert!(!filesystem_browse_path("/", "", true).can_browse_up);
    assert_eq!(
        filesystem_browse_path("C:\\Users\\test", "Win32", true)
            .parent_path
            .as_deref(),
        Some("C:\\")
    );
}
#[test]
fn usage_limits_refreshes_share_manual_waiters_and_throttle_automatic_refreshes() {
    let refresh = UsageLimitsRefresh::default();
    let platform = Rc::new(FakePlatform::default());
    let (tx, rx) = oneshot::channel::<Value>();
    let mut first = refresh.refresh(env("one"), platform.clone(), true, async move {
        Ok(rx.await.unwrap())
    });
    assert!(first.as_mut().now_or_never().is_none());
    let manual = refresh.refresh(env("one"), platform.clone(), false, async {
        panic!("manual duplicated the in-flight refresh");
        #[allow(unreachable_code)]
        Ok(Value::Null)
    });
    assert!(
        block_on(refresh.refresh(env("one"), platform.clone(), true, async {
            Ok(Value::Null)
        }))
        .unwrap()
        .is_none()
    );
    tx.send(json!("limits")).unwrap();
    assert_eq!(block_on(first).unwrap().unwrap().as_ref(), &json!("limits"));
    assert_eq!(
        block_on(manual).unwrap().unwrap().as_ref(),
        &json!("limits")
    );
    assert!(
        block_on(refresh.refresh(env("one"), platform.clone(), true, async {
            Ok(Value::Null)
        }))
        .unwrap()
        .is_none()
    );
    platform.advance(300_000);
    assert_eq!(platform.monotonic_ms(), 300_000);
    assert!(
        block_on(refresh.refresh(env("one"), platform, true, async { Ok(json!("new")) }))
            .unwrap()
            .is_some()
    );
}
#[test]
fn services_invalidate_usage_only_for_price_or_alias_changes_and_close_removed_configs() {
    let services = Services::default();
    services.begin_session(env("one"), Generation(1));
    services
        .apply_server_config(env("one"), Generation(1), snapshot(config()))
        .unwrap();
    let input: UsageSummaryInput =
        decode(json!({"sinceDay":"2026-09-05","untilDay":"2026-09-05","timeZone":"UTC"}));
    let usage = services
        .query::<t3_contracts::rpc::WsServerGetUsageSummaryRpc>(env("one"), &input)
        .unwrap();
    let token = usage.begin();
    let mut config = config();
    config.settings.usage_price_overrides.insert(decode(json!("custom-model")),decode(json!({"inputCostPerMillionTokens":7,"outputCostPerMillionTokens":8,"cacheReadCostPerMillionTokens":1,"cacheWriteCostPerMillionTokens":2})));
    services
        .apply_server_config(env("one"), Generation(1), snapshot(config))
        .unwrap();
    assert!(!usage.finish(token, usage_summary(&[]), 0, false));
    let held = services.config(env("one"));
    services.remove_environment(&env("one"));
    assert!(!held.restore(self::config()));
    assert!(block_on(held.flush(&FakePlatform::default())).is_err());
}
#[test]
fn service_commands_require_a_current_session_and_check_it_again_before_execution() {
    let services = Services::default();
    let endpoint = Rc::new(ScriptedRpc::default());
    assert!(
        services
            .command::<WsServerGetNpmReleaseRpc, _>(
                env("one"),
                endpoint.clone(),
                decode(json!({"spec":"latest"}))
            )
            .is_err()
    );
    services.begin_session(env("one"), Generation(1));
    let future = services
        .command::<WsServerGetNpmReleaseRpc, _>(
            env("one"),
            endpoint.clone(),
            decode(json!({"spec":"latest"})),
        )
        .unwrap();
    services.begin_session(env("one"), Generation(2));
    assert!(matches!(
        block_on(future).unwrap_err().as_ref(),
        ClientError::Cancelled
    ));
    assert!(endpoint.scripts.borrow().is_empty());
}
