#![allow(clippy::unwrap_used)]
use base64::Engine;
use futures::{FutureExt, executor::block_on, future::LocalBoxFuture, stream::LocalBoxStream};
use serde::{Serialize, Serializer};
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    rc::Rc,
};
use t3_client::{
    persistence::*,
    platform::*,
    testing::FakePlatform,
    types::{EnvironmentId, ThreadId},
};

type Values = BTreeMap<StorageKey, Vec<u8>>;
#[derive(Default, Clone)]
struct Memory {
    ordinary: Rc<RefCell<Values>>,
    secure: Rc<RefCell<Values>>,
    fail_write_number: Rc<Cell<Option<usize>>>,
    yields: Rc<Cell<usize>>,
    fake: FakePlatform,
}
impl Memory {
    fn area(&self, area: StorageArea) -> &RefCell<Values> {
        match area {
            StorageArea::Ordinary => &self.ordinary,
            StorageArea::Secure => &self.secure,
        }
    }
    fn has(&self, area: StorageArea, kind: &str) -> bool {
        self.area(area).borrow().keys().any(|k| k.namespace == kind)
    }
}
impl Storage for Memory {
    fn get(&self, area: StorageArea, key: StorageKey) -> HostFuture<'_, Option<Vec<u8>>> {
        async move { Ok(self.area(area).borrow().get(&key).cloned()) }.boxed_local()
    }
    fn write(&self, area: StorageArea, writes: Vec<StorageWrite>) -> HostFuture<'_, ()> {
        async move {
            let mut values = self.area(area).borrow_mut();
            for write in writes {
                if let Some(v) = write.value {
                    values.insert(write.key, v);
                } else {
                    values.remove(&write.key);
                }
            }
            Ok(())
        }
        .boxed_local()
    }
    fn list(&self, area: StorageArea, namespace: String) -> HostFuture<'_, Vec<StorageKey>> {
        async move {
            Ok(self
                .area(area)
                .borrow()
                .keys()
                .filter(|k| k.namespace == namespace)
                .cloned()
                .collect())
        }
        .boxed_local()
    }
    fn transaction(&self, transaction: StorageTransaction) -> HostFuture<'_, bool> {
        async move {
            for c in transaction.conditions {
                if self.area(c.area).borrow().get(&c.key) != c.expected.as_ref() {
                    return Ok(false);
                }
            }
            let mut ordinary = self.ordinary.borrow().clone();
            let mut secure = self.secure.borrow().clone();
            for (index, (area, write)) in transaction
                .secure
                .into_iter()
                .map(|w| (StorageArea::Secure, w))
                .chain(
                    transaction
                        .ordinary
                        .into_iter()
                        .map(|w| (StorageArea::Ordinary, w)),
                )
                .enumerate()
            {
                if self.fail_write_number.get() == Some(index) {
                    return Err(PlatformError::Storage("injected failure".into()));
                }
                let values = if area == StorageArea::Ordinary {
                    &mut ordinary
                } else {
                    &mut secure
                };
                if let Some(value) = write.value {
                    values.insert(write.key, value);
                } else {
                    values.remove(&write.key);
                }
            }
            *self.ordinary.borrow_mut() = ordinary;
            *self.secure.borrow_mut() = secure;
            Ok(true)
        }
        .boxed_local()
    }
}
impl Platform for Memory {
    fn now_epoch_ms(&self) -> u64 {
        self.fake.now_epoch_ms()
    }
    fn monotonic_ms(&self) -> u64 {
        self.fake.monotonic_ms()
    }
    fn random_bytes(&self, length: usize) -> Result<Vec<u8>, PlatformError> {
        self.fake.random_bytes(length)
    }
    fn sleep(&self, ms: u64) -> LocalBoxFuture<'_, ()> {
        self.fake.sleep(ms)
    }
    fn yield_to_host(&self) -> LocalBoxFuture<'_, ()> {
        async {
            self.yields.set(self.yields.get() + 1);
        }
        .boxed_local()
    }
    fn events(&self) -> LocalBoxStream<'static, PlatformEvent> {
        self.fake.events()
    }
    fn http(&self, request: HttpRequest) -> HostFuture<'_, HttpResponse> {
        self.fake.http(request)
    }
    fn websocket(&self, request: SocketRequest) -> HostFuture<'_, Box<dyn WebSocket>> {
        self.fake.websocket(request)
    }
    fn host(&self, action: HostAction) -> HostFuture<'_, HostReply> {
        self.fake.host(action)
    }
}

const NOW: &str = "2026-07-29T12:00:00.000Z";
fn env() -> EnvironmentId {
    EnvironmentId("env:with:colons".into())
}
fn thread() -> ThreadId {
    ThreadId("thread:with:colons".into())
}
fn thread_json() -> Value {
    json!({"id":thread(),"projectId":"project-1","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":thread(),"parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"mobile","createdAt":NOW,"updatedAt":NOW,"archivedAt":null,"settledOverride":null,"settledAt":null,"lastVisitedAt":null,"deletedAt":null})
}
fn notice(n: usize, inherited: bool) -> Value {
    json!({"id":format!("notice-{n}"),"type":"system_notice","threadId":if inherited { "parent".to_string() } else { thread().0 },"runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":n,"status":"completed","title":null,"startedAt":NOW,"completedAt":null,"updatedAt":NOW,"message":format!("Notice {n} 🦀")})
}
fn projection(count: usize) -> Value {
    let items: Vec<_> = (0..count).map(|n| notice(n, false)).collect();
    let mut rows = vec![
        json!({"position":0,"visibility":"inherited","sourceThreadId":"parent","sourceItemId":"notice-1000","item":notice(1000,true)}),
    ];
    rows.extend(items.iter().enumerate().map(|(n,item)| json!({"position":n+1,"visibility":"local","sourceThreadId":thread(),"sourceItemId":item["id"],"item":item})));
    json!({"thread":thread_json(),"runs":[],"attempts":[],"nodes":[],"subagents":[],"providerSessions":[],"providerThreads":[],"providerTurns":[],"runtimeRequests":[],"messages":[],"plans":[],"turnItems":items,"checkpointScopes":[],"checkpoints":[],"contextHandoffs":[],"contextTransfers":[],"visibleTurnItems":rows,"updatedAt":NOW})
}
fn full_thread(count: usize) -> Value {
    json!({"schemaVersion":5,"environmentId":env(),"threadId":thread(),"snapshot":{"snapshotSequence":7,"projection":projection(count),"historyCursor":null,"hasMoreHistory":false}})
}
fn shell() -> Value {
    let mut row = thread_json();
    row.as_object_mut().unwrap().extend(json!({"latestRunId":null,"activeRunId":null,"status":"idle","pendingRuntimeRequest":null,"latestVisibleMessage":null,"latestUserMessageAt":NOW,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0}).as_object().unwrap().clone());
    json!({"schemaVersion":3,"environmentId":env(),"snapshot":{"schemaVersion":1,"snapshotSequence":3,"threads":[row],"archivedThreads":[],"projects":[]}})
}
fn bearer() -> Value {
    json!({"_tag":"BearerConnectionTarget","environmentId":env(),"label":"Remote","connectionId":"bearer-1"})
}
fn relay() -> Value {
    json!({"_tag":"RelayConnectionTarget","environmentId":env(),"label":"Remote"})
}
fn profile() -> Value {
    json!({"_tag":"BearerConnectionProfile","environmentId":env(),"label":"Remote","connectionId":"bearer-1","httpBaseUrl":"https://remote.test","wsBaseUrl":"wss://remote.test"})
}
fn credential() -> Value {
    json!({"_tag":"BearerConnectionCredential","token":"SECRET_BEARER"})
}
fn remote_token() -> Value {
    json!({"environmentId":env(),"label":"Remote","endpoint":{"httpBaseUrl":"https://remote.test","wsBaseUrl":"wss://remote.test","providerKind":"cloudflare_tunnel"},"accessToken":"SECRET_DPOP","expiresAtEpochMs":1_000_000,"dpopThumbprint":"thumbprint"})
}
fn registration() -> Value {
    json!({"_tag":"BearerConnectionRegistration","target":bearer(),"profile":profile(),"credential":credential()})
}
fn catalog() -> Value {
    json!({"schemaVersion":1,"targets":[bearer(),relay()],"profiles":[profile()],"credentials":[{"connectionId":"bearer-1","credential":credential()}],"remoteDpopTokens":[remote_token()],"githubRoutingPermissions":[{"environmentId":env(),"connectionKey":"endpoint-bound","permission":"read-write"}],"disabledEnvironmentIds":[env()],"futureField":{"keep":true}})
}
fn entry(location: LegacyLocation, raw: Value) -> LegacyEntry {
    LegacyEntry {
        location,
        bytes: serde_json::to_vec(&raw).unwrap(),
    }
}
fn namespace(entries: Vec<LegacyEntry>) -> LegacyNamespace {
    LegacyNamespace {
        source: "fixture-host".into(),
        source_version: 5,
        namespace: "user-data".into(),
        entries,
        resources: BTreeMap::new(),
    }
}
fn catalog_entry() -> LegacyEntry {
    entry(
        LegacyLocation::MobileSecure {
            key: "t3code.connection-catalog.v1".into(),
        },
        catalog(),
    )
}
fn outbox(version: u32) -> Value {
    json!({"schemaVersion":version,"environmentId":env(),"threadId":thread(),"messageId":"message:/日本","commandId":"command-1","text":"unsent text","attachments":[],"createdAt":NOW,"dispatchMode":"queue","freshContext":true,"futureField":{"keep":true}})
}

#[test]
fn envelope_and_tuple_keys_validate_identity_without_delimiter_collisions() {
    let a = StorageKey::new("draft", Some(EnvironmentId("a:b".into())), "c");
    let b = StorageKey::new("draft", Some(EnvironmentId("a".into())), "b:c");
    assert_ne!(a.encode_tuple().unwrap(), b.encode_tuple().unwrap());
    assert_eq!(
        StorageKey::decode_tuple(&a.encode_tuple().unwrap()).unwrap(),
        a
    );
    let record = Record::new(
        "draft",
        a.environment.clone(),
        "c",
        1,
        json!({"unknown":[null,1,"🦀"]}),
    );
    let bytes = record.encode().unwrap();
    assert_eq!(
        decode_record::<Value>(&bytes, &a, 1).unwrap().payload,
        record.payload
    );
    assert!(decode_record::<Value>(&bytes, &b, 1).is_err());
    assert!(decode_record::<Value>(&bytes, &a, 2).is_err());
    let mut future = record.clone();
    future.format_version = 2;
    assert!(future.encode().is_err());
    assert!(!format!("{record:?}").contains("unknown"));
}

#[test]
fn platform_debug_and_public_jwk_do_not_disclose_credentials() {
    let secret = "SECRET_TOKEN";
    for error in [
        PlatformError::Network("MARKER_SECRET".into()),
        PlatformError::Storage("MARKER_SECRET".into()),
        PlatformError::Denied("MARKER_SECRET".into()),
        PlatformError::Unsupported("MARKER_SECRET".into()),
        PlatformError::InvalidData("MARKER_SECRET".into()),
    ] {
        assert!(!format!("{error:?} {error}").contains("MARKER_SECRET"));
    }
    assert!(
        matches!(PlatformError::Network("MARKER_SECRET".into()), PlatformError::Network(payload) if payload == "MARKER_SECRET")
    );
    let request = HttpRequest {
        method: "POST".into(),
        url: format!("https://user:{secret}@host/path?wsTicket={secret}&dpop={secret}"),
        headers: BTreeMap::from([("Authorization".into(), secret.into())]),
        body: secret.as_bytes().to_vec(),
        credentials: Credentials::Include,
        max_response_bytes: 100,
    };
    let response = HttpResponse {
        status: 200,
        headers: BTreeMap::from([("Set-Cookie".into(), secret.into())]),
        body: secret.as_bytes().to_vec(),
    };
    for debug in [
        format!("{request:?}"),
        format!("{response:?}"),
        format!("{:?}", HostReply::Document(json!({"token":secret}))),
        format!("{:?}", HostReply::Text(Some(secret.into()))),
        format!("{:?}", HostAction::OpenUrl(request.url.clone())),
        format!(
            "{:?}",
            StorageWrite {
                key: StorageKey::new("credential", None, "current"),
                value: Some(secret.as_bytes().to_vec())
            }
        ),
    ] {
        assert!(!debug.contains(secret), "{debug}");
    }
    block_on(async {
        let platform = FakePlatform::default();
        platform.push_host(Ok(HostReply::Document(
            json!({"kty":"EC","crv":"P-256","x":"x","y":"y","privateExtra":secret}),
        )));
        assert_eq!(
            platform.dpop_public_jwk().await.unwrap(),
            json!({"kty":"EC","crv":"P-256","x":"x","y":"y"})
        );
        platform.push_host(Ok(HostReply::Document(
            json!({"kty":"EC","crv":"P-256","x":"x","y":"y","d":secret}),
        )));
        assert!(platform.dpop_public_jwk().await.is_err());
        assert!(
            platform
                .transaction(StorageTransaction::default())
                .await
                .unwrap()
        );
    });
}

#[test]
fn catalog_compatibility_separates_secrets_and_preserves_disabled_and_unknown_fields() {
    let records = read_legacy_entry(&catalog_entry()).unwrap();
    let remote = records
        .iter()
        .find(|record| record.record.kind == REMOTE_DPOP_TOKEN_KIND)
        .unwrap();
    assert_eq!(remote.record.key(), remote_dpop_token_key(&env()));
    assert_eq!(remote.area, StorageArea::Secure);
    assert_eq!(remote.record.payload_version, 1);
    let token = decode_record::<t3_client::authorization::RemoteDpopAccessToken>(
        &remote.record.encode().unwrap(),
        &remote_dpop_token_key(&env()),
        1,
    )
    .unwrap();
    assert_eq!(token.payload.access_token.expose(), "SECRET_DPOP");
    let ordinary = records
        .iter()
        .filter(|r| r.area == StorageArea::Ordinary)
        .map(|r| String::from_utf8(r.record.encode().unwrap()).unwrap())
        .collect::<String>();
    assert!(!ordinary.contains("SECRET_"));
    assert!(!ordinary.contains("httpBaseUrl"));
    assert!(
        records
            .iter()
            .any(|r| r.record.kind == "connection-catalog-extra"
                && r.record.payload["futureField"]["keep"] == true)
    );
    assert!(records.iter().any(|r| r.record.kind == "connection-catalog"
        && r.record.payload["disabledEnvironmentIds"] == json!([env()])));
    let mut raw = catalog();
    raw.as_object_mut()
        .unwrap()
        .remove("disabledEnvironmentIds");
    assert!(
        decode_catalog(&serde_json::to_vec(&raw).unwrap())
            .unwrap()
            .disabled_environment_ids
            .is_empty()
    );
}

#[test]
fn relay_import_groups_secure_records_by_normalized_origin_and_client() {
    let token = |account: &str, url: &str, client: &str| {
        json!({
            "accountId": account, "clientId": client, "relayUrl": url, "thumbprint": "proof-key",
            "scopes": ["environment:connect"], "accessToken": format!("SECRET_{account}"), "expiresAtMillis": 9_000_000,
        })
    };
    let raw = json!({"version": 1, "entries": [
        token("one", "https://RELAY.example.test:443/", "t3-mobile"),
        token("two", "https://relay.example.test", "t3-mobile"),
        token("web", "https://relay.example.test", "t3-web"),
        token("other", "https://other.example.test", "t3-mobile"),
    ]});
    let records = read_mobile_secure(
        "t3code.cloud.relay-access-tokens",
        &serde_json::to_vec(&raw).unwrap(),
    )
    .unwrap();
    assert_eq!(records.len(), 3);
    let mobile_key = relay_dpop_cache_key("https://relay.example.test", "t3-mobile").unwrap();
    assert_eq!(
        mobile_key,
        relay_dpop_cache_key("https://RELAY.example.test:443/", "t3-mobile").unwrap()
    );
    assert_ne!(
        mobile_key,
        relay_dpop_cache_key("https://relay.example.test", "t3-web").unwrap()
    );
    let mobile = records
        .iter()
        .find(|record| record.record.key() == mobile_key)
        .unwrap();
    let decoded = decode_record::<Vec<t3_client::relay::RelayAccessTokenCacheEntry>>(
        &mobile.record.encode().unwrap(),
        &mobile_key,
        1,
    )
    .unwrap();
    assert_eq!(decoded.payload.len(), 2);
    assert_eq!(decoded.payload[0].relay_url, "https://relay.example.test");
    assert_eq!(decoded.payload[1].account_id, "two");
    assert!(
        records
            .iter()
            .all(|record| record.area == StorageArea::Secure
                && record.record.kind == RELAY_DPOP_TOKEN_KIND
                && record.record.payload_version == 1
                && record.record.payload.is_array())
    );
    assert!(relay_dpop_cache_key("http://relay.example.test", "t3-web").is_err());
    assert!(relay_dpop_cache_key("https://relay.example.test", "unknown-client").is_err());
}

#[test]
fn catalog_route_mutations_preserve_order_and_remove_only_owned_metadata() {
    let mut doc = ConnectionCatalogDocument::default();
    doc.remote_dpop_tokens.push(remote_token());
    register_connection_in_catalog(&mut doc, &registration(), vec![bearer()]).unwrap();
    let other = json!({"_tag":"RelayConnectionTarget","environmentId":"other","label":"Other"});
    doc.targets.push(other.clone());
    set_routes_in_catalog(&mut doc, &env(), vec![bearer(), relay()]).unwrap();
    assert_eq!(doc.targets, vec![bearer(), relay(), other.clone()]);
    assert_eq!(doc.credentials.len(), 1);
    set_connection_enabled_in_catalog(&mut doc, &env(), false);
    set_connection_enabled_in_catalog(&mut doc, &env(), false);
    assert_eq!(doc.disabled_environment_ids, vec![env()]);
    set_connection_enabled_in_catalog(&mut doc, &env(), true);
    assert!(doc.disabled_environment_ids.is_empty());
    set_routes_in_catalog(&mut doc, &env(), vec![relay()]).unwrap();
    assert!(doc.credentials.is_empty());
    assert!(doc.profiles.is_empty());
    assert_eq!(doc.remote_dpop_tokens.len(), 1);
    put_remote_dpop_token_in_catalog(&mut doc, remote_token());
    assert_eq!(doc.remote_dpop_tokens.len(), 1);
    remove_connection_from_catalog(&mut doc, &env()).unwrap();
    assert_eq!(doc.targets, vec![other]);
    assert!(doc.remote_dpop_tokens.is_empty());
    put_remote_dpop_token_in_catalog(&mut doc, remote_token());
    assert!(doc.remote_dpop_tokens.is_empty());
}

#[test]
fn replacing_relay_drops_its_token_but_replacing_bearer_keeps_reusable_token() {
    let mut doc = decode_catalog(&serde_json::to_vec(&catalog()).unwrap()).unwrap();
    set_routes_in_catalog(&mut doc, &env(), vec![bearer()]).unwrap();
    assert!(doc.remote_dpop_tokens.is_empty());
    assert_eq!(doc.credentials.len(), 1);
    let mut doc = ConnectionCatalogDocument::default();
    doc.remote_dpop_tokens.push(remote_token());
    register_connection_in_catalog(&mut doc, &registration(), vec![bearer()]).unwrap();
    register_connection_in_catalog(
        &mut doc,
        &json!({"_tag":"RelayConnectionRegistration","target":relay()}),
        vec![relay()],
    )
    .unwrap();
    assert_eq!(doc.remote_dpop_tokens.len(), 1);
    assert!(doc.credentials.is_empty());
}

#[test]
fn legacy_mobile_connections_preserve_source_and_migrate_relay_and_bearer() {
    let connections = json!({"connections":[{"environmentId":"relay","environmentLabel":"Relay","pairingUrl":"https://relay","displayUrl":"https://relay","httpBaseUrl":"https://relay","wsBaseUrl":"wss://relay","bearerToken":null,"authenticationMethod":"dpop","dpopAccessToken":"old-token"},{"environmentId":"bearer","environmentLabel":"Bearer","pairingUrl":"https://bearer","displayUrl":"https://bearer","httpBaseUrl":"https://bearer","wsBaseUrl":"wss://bearer","bearerToken":"SECRET_BEARER"}]});
    let bytes = serde_json::to_vec(&connections).unwrap();
    let before = bytes.clone();
    let doc = migrate_legacy_connections(&bytes).unwrap();
    assert_eq!(doc.targets.len(), 2);
    assert_eq!(doc.credentials.len(), 1);
    assert_eq!(bytes, before);
    assert_eq!(doc.targets[0]["_tag"], "RelayConnectionTarget");
    assert_eq!(doc.profiles[0]["connectionId"], "bearer:bearer");
}

#[test]
fn full_thread_and_compact_mobile_cache_round_trip_notices_and_inherited_rows() {
    let full = full_thread(40);
    let bytes = serde_json::to_vec(&full).unwrap();
    let expected = decode_thread_snapshot(&bytes, &env(), &thread()).unwrap();
    let mut compact = full.clone();
    compact["schemaVersion"] = json!(6);
    compact["snapshot"]["turnItemsOmitLocalVisible"] = json!(true);
    compact["snapshot"]["turnItemCount"] = json!(40);
    compact["snapshot"]["visibleTurnItemCount"] = json!(41);
    compact["snapshot"]["projection"]["turnItems"] = json!([]);
    let decoded =
        decode_thread_snapshot(&serde_json::to_vec(&compact).unwrap(), &env(), &thread()).unwrap();
    assert_eq!(decoded.snapshot, expected.snapshot);
    assert_eq!(decoded.schema_version, 5);
    for schema in [1, 3, 4, 7] {
        let mut old = full.clone();
        old["schemaVersion"] = json!(schema);
        assert!(
            decode_thread_snapshot(&serde_json::to_vec(&old).unwrap(), &env(), &thread()).is_err()
        );
    }
    assert!(decode_thread_snapshot(&bytes, &EnvironmentId("other".into()), &thread()).is_err());
    for raw in [
        Value::Null,
        json!(1),
        json!(true),
        json!([]),
        json!({"schemaVersion":5,"environmentId":env(),"threadId":thread(),"snapshot":1}),
    ] {
        let bytes = serde_json::to_vec(&raw).unwrap();
        assert!(decode_thread_snapshot(&bytes, &env(), &thread()).is_err());
        assert!(decode_shell_snapshot(&bytes, &env()).is_err());
    }
}

#[test]
fn compact_reader_rejects_every_identity_count_order_and_membership_corruption() {
    let mut good = full_thread(2);
    good["schemaVersion"] = json!(6);
    good["snapshot"]["turnItemsOmitLocalVisible"] = json!(true);
    good["snapshot"]["turnItemCount"] = json!(2);
    good["snapshot"]["visibleTurnItemCount"] = json!(3);
    good["snapshot"]["projection"]["turnItems"] = json!([]);
    let edits = [
        ("/snapshot/turnItemCount", json!(3)),
        ("/snapshot/visibleTurnItemCount", json!(2)),
        ("/snapshot/turnItemsOmitLocalVisible", json!(null)),
        ("/snapshot/projection/thread/id", json!("other")),
        (
            "/snapshot/projection/visibleTurnItems/1/sourceItemId",
            json!("wrong"),
        ),
        (
            "/snapshot/projection/visibleTurnItems/1/sourceThreadId",
            json!("other"),
        ),
        ("/snapshot/projection/visibleTurnItems/1/position", json!(0)),
        (
            "/snapshot/projection/visibleTurnItems/1/visibility",
            json!("inherited"),
        ),
        (
            "/snapshot/projection/visibleTurnItems/0/visibility",
            json!("local"),
        ),
        (
            "/snapshot/projection/visibleTurnItems/2/item/id",
            json!("notice-0"),
        ),
    ];
    for (path, value) in edits {
        let mut bad = good.clone();
        *bad.pointer_mut(path).unwrap() = value;
        assert!(
            decode_thread_snapshot(&serde_json::to_vec(&bad).unwrap(), &env(), &thread()).is_err(),
            "{path}"
        );
    }
}

#[test]
fn cooperative_cache_encoders_match_plain_json_across_chunks_and_budget_unicode_correctly() {
    block_on(async {
        let platform = Memory::default();
        let stored: StoredThreadSnapshot = serde_json::from_value(full_thread(70)).unwrap();
        let actual = encode_thread_snapshot(&platform, &stored, THREAD_CACHE_MAX_JSON_LENGTH)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&actual).unwrap(),
            serde_json::to_value(&stored).unwrap()
        );
        assert!(platform.yields.get() >= 4);
        let mut untrimmed = stored.clone();
        untrimmed.snapshot.projection.thread.model_selection.model = "  gpt-5.4  ".into();
        let normalized =
            encode_thread_snapshot(&platform, &untrimmed, THREAD_CACHE_MAX_JSON_LENGTH)
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&normalized).unwrap()["snapshot"]["projection"]["thread"]
                ["modelSelection"]["model"],
            "gpt-5.4"
        );
        let length = std::str::from_utf8(&actual).unwrap().encode_utf16().count();
        assert!(
            encode_thread_snapshot(&platform, &stored, length)
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            encode_thread_snapshot(&platform, &stored, length - 1)
                .await
                .unwrap()
                .is_none()
        );
        let mut raw = shell();
        let row = raw["snapshot"]["threads"][0].clone();
        raw["snapshot"]["threads"] = Value::Array(
            (0..65)
                .map(|n| {
                    let mut row = row.clone();
                    row["id"] = json!(format!("thread-{n}"));
                    row
                })
                .collect(),
        );
        raw["snapshot"]["archivedThreads"] = json!([row]);
        let stored: StoredShellSnapshot = serde_json::from_value(raw).unwrap();
        let actual = encode_shell_snapshot(&platform, &stored).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&actual).unwrap(),
            serde_json::to_value(&stored).unwrap()
        );
    });
}

#[test]
fn row_encoder_reuses_only_live_successful_immutable_rows() {
    struct Counted {
        calls: Rc<Cell<usize>>,
        fail: Cell<bool>,
        value: u32,
    }
    impl Serialize for Counted {
        fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            self.calls.set(self.calls.get() + 1);
            if self.fail.get() {
                return Err(serde::ser::Error::custom("bad row"));
            }
            s.serialize_u32(self.value)
        }
    }
    block_on(async {
        let platform = Memory::default();
        let calls = Rc::new(Cell::new(0));
        let row = Rc::new(Counted {
            calls: calls.clone(),
            fail: Cell::new(true),
            value: 1,
        });
        let mut encoder = RowEncoder::new();
        assert!(
            encoder
                .encode(&platform, std::slice::from_ref(&row))
                .await
                .is_err()
        );
        assert!(
            encoder
                .encode(&platform, std::slice::from_ref(&row))
                .await
                .is_err()
        );
        assert_eq!(calls.get(), 2);
        row.fail.set(false);
        assert_eq!(
            encoder
                .encode(&platform, std::slice::from_ref(&row))
                .await
                .unwrap(),
            vec![json!(1)]
        );
        let yields = platform.yields.get();
        encoder
            .encode(&platform, std::slice::from_ref(&row))
            .await
            .unwrap();
        assert_eq!(calls.get(), 3);
        assert_eq!(platform.yields.get(), yields);
        let replacement = Rc::new(Counted {
            calls: calls.clone(),
            fail: Cell::new(false),
            value: 2,
        });
        assert_eq!(
            encoder
                .encode(&platform, &[replacement, row])
                .await
                .unwrap(),
            vec![json!(2), json!(1)]
        );
    });
}

#[test]
fn tickets_prevent_old_saves_after_new_save_remove_or_environment_clear() {
    let mut order = ThreadCacheWriteOrder::default();
    let old = order.begin_save(env(), thread());
    let new = order.begin_save(env(), thread());
    assert!(!order.is_current(&old));
    assert!(order.is_current(&new));
    let other = order.begin_save(EnvironmentId("other".into()), thread());
    order.remove_thread(env(), thread());
    assert!(!order.is_current(&new));
    assert!(order.is_current(&other));
    let new = order.begin_save(env(), thread());
    order.clear_environment(env());
    assert!(!order.is_current(&new));
    assert!(order.is_current(&other));
    let after = order.begin_save(env(), thread());
    assert!(order.is_current(&after));
}

#[test]
fn failed_import_is_atomic_and_retry_journals_once_after_all_writes() {
    block_on(async {
        let storage = Memory::default();
        let input = namespace(vec![catalog_entry()]);
        let original = input.clone();
        for failure in [0, 1, 3, 6] {
            storage.fail_write_number.set(Some(failure));
            assert!(import_namespace(&storage, &input).await.is_err());
            assert!(storage.ordinary.borrow().is_empty());
            assert!(storage.secure.borrow().is_empty());
        }
        storage.fail_write_number.set(None);
        let report = import_namespace(&storage, &input).await.unwrap();
        assert_eq!(report.status, ImportStatus::Imported);
        assert!(storage.has(StorageArea::Ordinary, "import-journal"));
        assert!(storage.has(StorageArea::Secure, "connection-credential"));
        assert!(
            !storage.has(StorageArea::Secure, "import-backup"),
            "a parsed entry keeps no raw copy of its credentials"
        );
        let restored = load_connection_catalog(&storage)
            .await
            .unwrap()
            .unwrap()
            .document;
        assert_eq!(restored.targets, vec![bearer(), relay()]);
        assert_eq!(restored.credentials[0]["credential"], credential());
        assert_eq!(restored.remote_dpop_tokens, vec![remote_token()]);
        assert_eq!(restored.disabled_environment_ids, vec![env()]);
        assert_eq!(restored.extra["futureField"], json!({"keep":true}));
        let ordinary = storage.ordinary.borrow().clone();
        let secure = storage.secure.borrow().clone();
        assert_eq!(
            import_namespace(&storage, &input).await.unwrap().status,
            ImportStatus::AlreadyImported
        );
        assert_eq!(*storage.ordinary.borrow(), ordinary);
        assert_eq!(*storage.secure.borrow(), secure);
        assert_eq!(input, original);
    });
}

#[test]
fn import_quarantines_exact_invalid_user_bytes_and_only_discards_derived_caches() {
    block_on(async {
        let storage = Memory::default();
        let corrupt = b"{broken SECRET_CREDENTIAL".to_vec();
        let input = namespace(vec![
            LegacyEntry {
                location: LegacyLocation::MobileSecure {
                    key: "t3code.connection-catalog.v1".into(),
                },
                bytes: corrupt.clone(),
            },
            LegacyEntry {
                location: LegacyLocation::MobileDocument {
                    path: "composer-drafts/drafts.json".into(),
                },
                bytes: corrupt.clone(),
            },
            LegacyEntry {
                location: LegacyLocation::MobileCache {
                    environment_id: env(),
                    kind: "thread".into(),
                    cache_key: thread().0,
                    schema_version: 4,
                    updated_at: 0,
                },
                bytes: corrupt.clone(),
            },
        ]);
        let report = import_namespace(&storage, &input).await.unwrap();
        assert_eq!(report.quarantined, 2);
        assert_eq!(report.discarded_caches, 1);
        let values = storage.secure.borrow();
        let quarantines: Vec<_> = values
            .iter()
            .filter(|(k, _)| k.namespace == "quarantine")
            .collect();
        assert_eq!(quarantines.len(), 2);
        for (key, bytes) in quarantines {
            let record = decode_record::<QuarantinedRecord>(bytes, key, 1).unwrap();
            assert_eq!(
                base64::engine::general_purpose::STANDARD
                    .decode(record.payload.bytes_base64)
                    .unwrap(),
                corrupt
            );
        }
        assert!(
            !storage
                .ordinary
                .borrow()
                .values()
                .any(|b| String::from_utf8_lossy(b).contains("SECRET_CREDENTIAL"))
        );
    });
}

#[test]
fn outbox_versions_keep_stable_ids_unknown_fields_and_attachments_in_same_commit() {
    block_on(async {
        for version in 1..=4 {
            let storage = Memory::default();
            let mut message = outbox(version);
            message["attachments"] = json!([{"id":"image-1","type":"image","name":"capture.png","mimeType":"image/png","sizeBytes":4,"previewUri":"preview://derived","fileUri":"file://old-private/attachment.png"}]);
            let path = format!(
                "thread-outbox/{}.json",
                encode_uri_component(message["messageId"].as_str().unwrap())
            );
            let mut input = namespace(vec![entry(
                LegacyLocation::MobileDocument { path },
                message.clone(),
            )]);
            assert!(matches!(
                import_namespace(&storage, &input).await,
                Err(PersistenceError::MissingResources)
            ));
            assert!(storage.ordinary.borrow().is_empty());
            assert!(storage.secure.borrow().is_empty());
            input.resources.insert(
                "file://old-private/attachment.png".into(),
                vec![0, 255, 1, 2],
            );
            import_namespace(&storage, &input).await.unwrap();
            let key = StorageKey::new("outbox", Some(env()), "message:/日本");
            let bytes = storage
                .get(StorageArea::Ordinary, key.clone())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                decode_record::<Value>(&bytes, &key, 1).unwrap().payload,
                message
            );
            let loaded = t3_client::thread_views::outbox::load(&storage)
                .await
                .unwrap();
            assert!(loaded.errors.is_empty());
            assert_eq!(loaded.messages.len(), 1);
            assert!(storage.has(StorageArea::Ordinary, "attachment"));
            assert_eq!(
                import_namespace(&storage, &input).await.unwrap().status,
                ImportStatus::AlreadyImported
            );
            let before = storage.ordinary.borrow().clone();
            let mut changed_resource = input.clone();
            changed_resource
                .resources
                .insert("file://old-private/attachment.png".into(), vec![9, 9]);
            assert!(matches!(
                import_namespace(&storage, &changed_resource).await,
                Err(PersistenceError::Conflict)
            ));
            assert_eq!(*storage.ordinary.borrow(), before);
        }
    });
}

#[test]
fn import_retains_existing_edits_and_same_message_id_conflicts_without_duplication() {
    block_on(async {
        let storage = Memory::default();
        let message = outbox(3);
        let path = format!(
            "thread-outbox/{}.json",
            encode_uri_component(message["messageId"].as_str().unwrap())
        );
        let input = namespace(vec![entry(
            LegacyLocation::MobileDocument { path: path.clone() },
            message.clone(),
        )]);
        import_namespace(&storage, &input).await.unwrap();
        let mut changed = message;
        changed["text"] = json!("different old client contents");
        let mut next = namespace(vec![entry(
            LegacyLocation::MobileDocument { path },
            changed,
        )]);
        next.source_version = 6;
        let report = import_namespace(&storage, &next).await.unwrap();
        assert!(report.quarantined > 0);
        assert!(storage.has(StorageArea::Secure, "import-conflict"));
        assert_eq!(
            storage
                .ordinary
                .borrow()
                .keys()
                .filter(|k| k.namespace == "outbox")
                .count(),
            1
        );
        let key = StorageKey::new("outbox", Some(env()), "message:/日本");
        let bytes = storage
            .get(StorageArea::Ordinary, key.clone())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            decode_record::<Value>(&bytes, &key, 1).unwrap().payload["text"],
            "unsent text"
        );
    });
}

#[test]
fn preferences_choose_newest_valid_record_preserve_unknown_fields_and_keep_sources() {
    block_on(async {
        let storage = Memory::default();
        let input = namespace(vec![
            entry(
                LegacyLocation::MobileSecure {
                    key: "t3code.preferences".into(),
                },
                json!({"theme":"legacy"}),
            ),
            entry(
                LegacyLocation::MobilePreferences { updated_at: 20 },
                json!({"theme":"db","unknown":{"keep":true}}),
            ),
            entry(
                LegacyLocation::MobileSecure {
                    key: "t3code.preferences.fallback".into(),
                },
                json!({"updatedAt":30,"payload":"{\"theme\":\"fallback\",\"future\":[1,2]}"}),
            ),
        ]);
        import_namespace(&storage, &input).await.unwrap();
        let key = StorageKey::new("preferences.mobile", None, "current");
        let bytes = storage
            .get(StorageArea::Ordinary, key.clone())
            .await
            .unwrap()
            .unwrap();
        let record = decode_record::<Value>(&bytes, &key, 1).unwrap();
        assert_eq!(record.payload["updatedAt"], 30);
        assert_eq!(record.payload["preferences"]["future"], json!([1, 2]));
        let input = namespace(vec![
            entry(
                LegacyLocation::MobileSecure {
                    key: "t3code.preferences.fallback".into(),
                },
                json!({"updatedAt":99,"payload":"invalid"}),
            ),
            entry(
                LegacyLocation::MobilePreferences { updated_at: 40 },
                json!({"theme":"valid"}),
            ),
        ]);
        import_namespace(&storage, &input).await.unwrap();
        let bytes = storage
            .get(StorageArea::Ordinary, key.clone())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            decode_record::<Value>(&bytes, &key, 1).unwrap().payload["preferences"]["theme"],
            "valid"
        );
        assert!(storage.has(StorageArea::Secure, "quarantine"));
    });
}

#[test]
fn endpoint_routing_revocation_uses_compare_exchange_and_survives_reimport() {
    block_on(async {
        let storage = Memory::default();
        let grant = compare_exchange_routing_grant(
            &storage,
            &env(),
            None,
            "endpoint-1".into(),
            RoutingAccess::ReadWrite,
        )
        .await
        .unwrap();
        let revoked = compare_exchange_routing_grant(
            &storage,
            &env(),
            Some(grant.revision),
            "endpoint-1".into(),
            RoutingAccess::Off,
        )
        .await
        .unwrap();
        assert_eq!(revoked.revision, 2);
        assert!(matches!(
            compare_exchange_routing_grant(
                &storage,
                &env(),
                Some(grant.revision),
                "endpoint-1".into(),
                RoutingAccess::ReadWrite
            )
            .await,
            Err(PersistenceError::Conflict)
        ));
        let input = namespace(vec![entry(
            LegacyLocation::WebLocalStorage {
                key: format!("{ROUTING_KEY_PREFIX}{}", env().0),
            },
            json!({"environmentId":env(),"connectionKey":"endpoint-1","permission":"read-write"}),
        )]);
        import_namespace(&storage, &input).await.unwrap();
        assert_eq!(
            load_routing_grant(&storage, &env())
                .await
                .unwrap()
                .unwrap()
                .grant
                .permission,
            RoutingAccess::Off
        );
    });
}

#[test]
fn query_caps_ttl_utf8_and_oversized_replacement_match_mobile_and_web() {
    block_on(async {
        let storage = Memory::default();
        let cache = EnvironmentCacheStore::new(storage.clone());
        for n in 0..300 {
            cache
                .save_query(&env(), &format!("row-{n:03}"), "🦀".repeat(10))
                .await
                .unwrap();
        }
        assert!(
            storage
                .ordinary
                .borrow()
                .keys()
                .filter(|k| k.namespace == "cache.pull-request")
                .count()
                <= QUERY_MAX_ROWS
        );
        cache
            .save_query(&env(), "large", "small".into())
            .await
            .unwrap();
        cache
            .save_query(&env(), "large", "x".repeat(QUERY_MAX_PAYLOAD_BYTES + 1))
            .await
            .unwrap();
        assert!(cache.load_query(&env(), "large").await.unwrap().is_none());
        cache
            .save_query(&env(), "recent", "value".into())
            .await
            .unwrap();
        storage.fake.advance(QUERY_MAX_AGE_MS + 1);
        assert!(cache.load_query(&env(), "recent").await.unwrap().is_none());
        let mut rows = BTreeMap::from([
            (
                "old".into(),
                QueryCacheRecord {
                    environment_id: env(),
                    payload: "x".into(),
                    updated_at: 0,
                },
            ),
            (
                "new".into(),
                QueryCacheRecord {
                    environment_id: env(),
                    payload: "🦀".into(),
                    updated_at: QUERY_MAX_AGE_MS + 1,
                },
            ),
        ]);
        prune_query_cache(&mut rows, QUERY_MAX_AGE_MS + 1, false);
        assert_eq!(rows.keys().cloned().collect::<Vec<_>>(), vec!["new"]);
    });
}

#[test]
fn caches_round_trip_and_clear_only_derived_environment_records() {
    block_on(async {
        let storage = Memory::default();
        let cache = EnvironmentCacheStore::new(storage.clone());
        let shell: StoredShellSnapshot = serde_json::from_value(shell()).unwrap();
        let thread: StoredThreadSnapshot = serde_json::from_value(full_thread(3)).unwrap();
        cache.save_shell(&shell).await.unwrap();
        cache.save_thread(&thread).await.unwrap();
        assert_eq!(
            cache
                .load_thread(&env(), &thread.thread_id)
                .await
                .unwrap()
                .unwrap()
                .snapshot,
            thread.snapshot
        );
        assert!(cache.load_shell(&env()).await.unwrap().is_some());
        let key = StorageKey::new("outbox", Some(env()), "keep");
        storage
            .write(
                StorageArea::Ordinary,
                vec![StorageWrite {
                    key: key.clone(),
                    value: Some(vec![1, 2, 3]),
                }],
            )
            .await
            .unwrap();
        let other = StorageKey::new("cache.thread", Some(EnvironmentId("other".into())), "keep");
        storage
            .write(
                StorageArea::Ordinary,
                vec![StorageWrite {
                    key: other.clone(),
                    value: Some(vec![1]),
                }],
            )
            .await
            .unwrap();
        cache.clear(&env()).await.unwrap();
        assert!(cache.load_shell(&env()).await.unwrap().is_none());
        assert!(
            storage
                .get(StorageArea::Ordinary, key)
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            storage
                .get(StorageArea::Ordinary, other)
                .await
                .unwrap()
                .is_some()
        );
        let corrupt = StorageKey::new("cache.thread", Some(env()), thread.thread_id.0.clone());
        storage
            .write(
                StorageArea::Ordinary,
                vec![StorageWrite {
                    key: corrupt.clone(),
                    value: Some(b"{invalid".to_vec()),
                }],
            )
            .await
            .unwrap();
        assert!(
            cache
                .load_thread(&env(), &thread.thread_id)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            storage
                .get(StorageArea::Ordinary, corrupt)
                .await
                .unwrap()
                .is_none()
        );
    });
}

#[test]
fn legacy_keys_use_embedded_identity_and_actual_mobile_snapshot_and_config_names() {
    let raw = shell();
    let bytes = serde_json::to_vec(&raw).unwrap();
    assert!(read_web_indexed_db("t3code:connection-runtime", "shell", &env().0, &bytes).is_ok());
    assert!(read_mobile_cache(&env(), "shell", "snapshot", 3, 0, &bytes).is_ok());
    assert!(read_mobile_cache(&env(), "shell", "current", 3, 0, &bytes).is_err());
    let raw = full_thread(2);
    let bytes = serde_json::to_vec(&raw).unwrap();
    assert!(
        read_web_indexed_db(
            "t3code:connection-runtime",
            "thread",
            &format!("{}:{}", env().0, thread().0),
            &bytes
        )
        .is_ok()
    );
    assert!(read_web_indexed_db("t3code:connection-runtime", "thread", "wrong", &bytes).is_err());
}

#[test]
fn broad_web_preferences_drafts_and_mobile_security_formats_preserve_original_payloads() {
    let cases = [
        (
            "t3code:composer-drafts:v1",
            json!({"version":9,"state":{"drafts":{"thread":{"prompt":"keep","future":1}}}}),
        ),
        (
            "t3code:prompt-stash:v2",
            json!({"version":2,"state":{"entries":[{"id":"keep","future":true}]}}),
        ),
        (
            "t3code:right-panel-state:v2",
            json!({"version":14,"state":{"keep":true}}),
        ),
        (
            "t3code:ui-state:v1",
            json!({"threadLastVisitedAtById":{"thread":NOW},"future":true}),
        ),
        ("t3code:themes:v1", json!({"future":[1,2]})),
        (
            "t3.pullRequests.preferences",
            json!({"filters":{"keep":true}}),
        ),
        (
            "codething:renderer-state:v1",
            json!({"collapsedProjectCwds":["/repo"]}),
        ),
    ];
    for (key, raw) in cases {
        let records = read_web_storage(key, &serde_json::to_vec(&raw).unwrap(), false).unwrap();
        assert_eq!(records[0].record.payload, raw);
        if key == "t3code:composer-drafts:v1" {
            let document = t3_client::thread_views::drafts::SavedDraftDocument::decode_web(
                &records[0].record.encode().unwrap(),
            )
            .unwrap();
            assert_eq!(document.raw(), &raw);
            assert_eq!(records[0].record.record_id, "current");
        }
    }
    assert_eq!(
        read_web_storage("t3code:theme", b"custom-theme", false).unwrap()[0]
            .record
            .payload,
        "custom-theme"
    );
    assert!(read_web_storage("t3code:composer-drafts:v1", b"broken", false).is_err());
    let registration =
        json!({"identity":"device","signature":"sig","pushToStartToken":"token","future":true});
    assert_eq!(
        read_mobile_secure(
            "t3code.agent-awareness.registration",
            &serde_json::to_vec(&registration).unwrap()
        )
        .unwrap()[0]
            .record
            .payload,
        registration
    );
    assert_eq!(
        read_mobile_document("voice-audio-grant-id", b"grant-1").unwrap()[0]
            .record
            .payload,
        "grant-1"
    );
}

#[test]
fn checksum_covers_all_raw_and_resource_bytes_independent_of_enumeration_order() {
    let mut a = namespace(vec![
        catalog_entry(),
        entry(
            LegacyLocation::MobileSecure {
                key: "t3code.preferences".into(),
            },
            json!({"future":true}),
        ),
    ]);
    let digest = namespace_checksum(&a).unwrap();
    a.entries.reverse();
    assert_eq!(namespace_checksum(&a).unwrap(), digest);
    a.resources.insert("attachment".into(), vec![0, 255]);
    assert_ne!(namespace_checksum(&a).unwrap(), digest);
}

#[test]
fn shell_origin_resume_requires_exact_origin_options_and_12_hour_freshness() {
    let origin = ShellOrigin {
        server_version: "1".into(),
        database_migration: Some(5),
        omit_idle_subagents: true,
        omit_older_settled: true,
        snapshot_at: 100,
    };
    assert!(origin.resumable(&origin, 100));
    assert!(origin.resumable(&origin, 43_200_099));
    assert!(!origin.resumable(&origin, 43_200_100));
    assert!(!origin.resumable(&origin, 99));
    let mut expected = origin.clone();
    expected.omit_older_settled = false;
    assert!(!origin.resumable(&expected, 100));
    expected = origin.clone();
    expected.database_migration = Some(6);
    assert!(!origin.resumable(&expected, 100));
}

#[test]
fn an_import_drops_older_raw_backups_and_removal_purges_copies_of_a_token() {
    // Platform review M2.
    block_on(async {
        use base64::Engine;
        let storage = Memory::default();
        let token = credential()["token"].as_str().unwrap().to_owned();
        let encode = |value: &Value| {
            base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(value).unwrap())
        };
        // An older build's backup, and a quarantined entry, each holding the token.
        let copy = |namespace: &str, id: &str, raw: &Value| StorageWrite {
            key: StorageKey::new(namespace, None, id),
            value: Some(
                t3_client::persistence::Record::new(
                    namespace,
                    None,
                    id,
                    1,
                    json!({"bytesBase64": encode(raw)}),
                )
                .encode()
                .unwrap(),
            ),
        };
        storage
            .write(
                StorageArea::Secure,
                vec![copy("import-backup", "old", &catalog())],
            )
            .await
            .unwrap();
        import_namespace(&storage, &namespace(vec![catalog_entry()]))
            .await
            .unwrap();
        assert!(!storage.has(StorageArea::Secure, "import-backup"));

        storage
            .write(
                StorageArea::Secure,
                vec![
                    copy("quarantine", "bad", &catalog()),
                    copy(
                        "quarantine",
                        "unrelated",
                        &json!({"environmentId": "other"}),
                    ),
                ],
            )
            .await
            .unwrap();
        let purged =
            t3_client::persistence::purge_environment_copies(&storage, &env(), &[token.clone()])
                .await
                .unwrap();
        assert_eq!(purged, 1);
        let quarantined = storage
            .list(StorageArea::Secure, "quarantine".into())
            .await
            .unwrap();
        assert_eq!(quarantined.len(), 1, "other environments' quarantine stays");
        assert_eq!(quarantined[0].record, "unrelated");
    });
}
