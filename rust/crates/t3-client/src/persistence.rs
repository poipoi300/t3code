//! Cross-shell record codecs and read-only imports. Hosts store bytes; policy lives here.
use crate::{
    platform::{
        LegacyStorage, Platform, PlatformError, Storage, StorageArea, StorageCondition, StorageKey,
        StorageTransaction, StorageWrite,
    },
    types::{EnvironmentId, ThreadId},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    rc::{Rc, Weak},
};
use t3_contracts::{
    git::VcsListRefsResult, orchestration_v2::*, orchestration_v2_id_table::IdOrIndex,
    server::ServerConfig,
};
pub const FORMAT_VERSION: u32 = 1;
pub const REMOTE_DPOP_TOKEN_KIND: &str = "authorization.remote-dpop";
pub const RELAY_DPOP_TOKEN_KIND: &str = "authorization.relay-dpop";

pub fn remote_dpop_token_key(environment: &EnvironmentId) -> StorageKey {
    StorageKey::new(REMOTE_DPOP_TOKEN_KIND, Some(environment.clone()), "current")
}
pub fn relay_dpop_cache_key(relay_url: &str, client_id: &str) -> Result<StorageKey> {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    if !matches!(client_id, "t3-web" | "t3-mobile") {
        return invalid("relay client id");
    }
    let origin = crate::relay::normalize_secure_relay_url(relay_url)
        .map_err(|_| PersistenceError::Invalid("relay URL"))?;
    let hash = URL_SAFE_NO_PAD.encode(Sha256::digest(origin.as_bytes()));
    Ok(StorageKey::new(
        RELAY_DPOP_TOKEN_KIND,
        None,
        format!("{hash}:{client_id}"),
    ))
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record<T> {
    pub format_version: u32,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_id: Option<EnvironmentId>,
    pub record_id: String,
    pub payload_version: u32,
    pub payload: T,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellOrigin {
    pub server_version: String,
    pub database_migration: Option<i64>,
    pub omit_idle_subagents: bool,
    pub omit_older_settled: bool,
    pub snapshot_at: u64,
}
impl ShellOrigin {
    pub fn resumable(&self, expected: &Self, now: u64) -> bool {
        self.server_version == expected.server_version
            && self.database_migration == expected.database_migration
            && self.omit_idle_subagents == expected.omit_idle_subagents
            && self.omit_older_settled == expected.omit_older_settled
            && now >= self.snapshot_at
            && now - self.snapshot_at < 43_200_000
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MigrationJournal {
    pub source: String,
    pub source_version: u32,
    pub checksum: String,
    pub completed: bool,
}

pub const SHELL_CACHE_SCHEMA_VERSION: u32 = 3;
pub const THREAD_CACHE_SCHEMA_VERSION: u32 = 5;
pub const COMPACT_THREAD_CACHE_SCHEMA_VERSION: u32 = 6;
pub const THREAD_CACHE_MAX_JSON_LENGTH: usize = 4 * 1024 * 1024;
pub const ROWS_PER_CHUNK: usize = 32;
pub const QUERY_MAX_ROWS: usize = 256;
pub const QUERY_MAX_BYTES: usize = 32 * 1024 * 1024;
pub const QUERY_MAX_PAYLOAD_BYTES: usize = 4 * 1024 * 1024;
pub const QUERY_MAX_AGE_MS: u64 = 30 * 24 * 60 * 60_000;
pub const QUERY_PRUNE_BATCH_ROWS: usize = 32;
pub const QUERY_PRUNE_BATCH_BYTES: usize = 1024 * 1024;
pub const ROUTING_KEY_PREFIX: &str = "t3code:github-routing:";
pub const ROUTING_CHANGED_EVENT: &str = "t3code:github-routing-changed";

#[derive(Debug, thiserror::Error)]
pub enum PersistenceError {
    #[error(transparent)]
    Platform(#[from] PlatformError),
    #[error("invalid persisted {0}")]
    Invalid(&'static str),
    #[error("persistence changed concurrently")]
    Conflict,
    #[error("attachment resources are missing")]
    MissingResources,
}
type Result<T> = std::result::Result<T, PersistenceError>;
fn parse<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    serde_json::from_slice(bytes).map_err(|_| PersistenceError::Invalid("JSON"))
}
fn encode<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(|_| PersistenceError::Invalid("JSON encoding"))
}
fn invalid<T>(kind: &'static str) -> Result<T> {
    Err(PersistenceError::Invalid(kind))
}

impl<T> std::fmt::Debug for Record<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Record")
            .field("kind", &self.kind)
            .field("environment_id", &self.environment_id)
            .field("record_id", &self.record_id)
            .field("payload_version", &self.payload_version)
            .finish_non_exhaustive()
    }
}
impl<T> Record<T> {
    pub fn new(
        kind: impl Into<String>,
        environment_id: Option<EnvironmentId>,
        record_id: impl Into<String>,
        payload_version: u32,
        payload: T,
    ) -> Self {
        Self {
            format_version: FORMAT_VERSION,
            kind: kind.into(),
            environment_id,
            record_id: record_id.into(),
            payload_version,
            payload,
        }
    }
    pub fn key(&self) -> StorageKey {
        StorageKey::new(&self.kind, self.environment_id.clone(), &self.record_id)
    }
}
impl<T: Serialize> Record<T> {
    pub fn encode(&self) -> Result<Vec<u8>> {
        if self.format_version != FORMAT_VERSION
            || self.kind.is_empty()
            || self.record_id.is_empty()
            || self.payload_version == 0
        {
            return invalid("record envelope");
        }
        encode(self)
    }
}
pub fn decode_record<T: DeserializeOwned>(
    bytes: &[u8],
    key: &StorageKey,
    payload_version: u32,
) -> Result<Record<T>> {
    let record: Record<T> = parse(bytes)?;
    if record.format_version != FORMAT_VERSION
        || record.key() != *key
        || record.payload_version != payload_version
    {
        return invalid("record envelope");
    }
    Ok(record)
}
pub async fn load_record<T: DeserializeOwned>(
    storage: &(impl Storage + ?Sized),
    area: StorageArea,
    key: &StorageKey,
    payload_version: u32,
) -> Result<Option<Record<T>>> {
    storage
        .get(area, key.clone())
        .await?
        .map(|bytes| decode_record(&bytes, key, payload_version))
        .transpose()
}
pub async fn save_record<T: Serialize>(
    storage: &(impl Storage + ?Sized),
    area: StorageArea,
    record: &Record<T>,
) -> Result<()> {
    storage
        .write(
            area,
            vec![StorageWrite {
                key: record.key(),
                value: Some(record.encode()?),
            }],
        )
        .await?;
    Ok(())
}

/// Reassembles the legacy catalog view from the shared public and secure records.
pub async fn load_connection_catalog(
    storage: &(impl Storage + ?Sized),
) -> Result<Option<ConnectionCatalogDocument>> {
    let key = StorageKey::new("connection-catalog", None, "current");
    let Some(public) = load_record::<Value>(storage, StorageArea::Ordinary, &key, 1).await? else {
        return Ok(None);
    };
    let targets: Vec<Value> = serde_json::from_value(public.payload["targets"].clone())
        .map_err(|_| PersistenceError::Invalid("catalog targets"))?;
    let disabled_environment_ids =
        serde_json::from_value(public.payload["disabledEnvironmentIds"].clone())
            .map_err(|_| PersistenceError::Invalid("catalog disabled environments"))?;
    let mut doc = ConnectionCatalogDocument {
        targets,
        disabled_environment_ids,
        ..ConnectionCatalogDocument::default()
    };
    let ids: BTreeSet<_> = doc
        .targets
        .iter()
        .filter_map(|v| v["connectionId"].as_str().map(str::to_owned))
        .collect();
    let environments: BTreeSet<_> = doc
        .targets
        .iter()
        .filter_map(|v| v["environmentId"].as_str().map(|v| EnvironmentId(v.into())))
        .collect();
    for id in ids {
        for (kind, rows) in [
            ("connection-profile", &mut doc.profiles),
            ("connection-credential", &mut doc.credentials),
        ] {
            if let Some(record) = load_record::<Value>(
                storage,
                StorageArea::Secure,
                &StorageKey::new(kind, None, &id),
                1,
            )
            .await?
            {
                rows.push(record.payload);
            }
        }
    }
    for environment in environments {
        if let Some(record) = load_record::<Value>(
            storage,
            StorageArea::Secure,
            &remote_dpop_token_key(&environment),
            1,
        )
        .await?
        {
            doc.remote_dpop_tokens.push(record.payload);
        }
        if let Some(record) = load_record::<RoutingGrant>(
            storage,
            StorageArea::Ordinary,
            &StorageKey::new("routing-grant", Some(environment), "current"),
            1,
        )
        .await?
        {
            doc.github_routing_permissions
                .get_or_insert_with(Vec::new)
                .push(record.payload.grant);
        }
    }
    if let Some(record) = load_record::<BTreeMap<String, Value>>(
        storage,
        StorageArea::Secure,
        &StorageKey::new("connection-catalog-extra", None, "current"),
        1,
    )
    .await?
    {
        doc.extra = record.payload;
    }
    decode_catalog(&encode(&doc)?)?;
    Ok(Some(doc))
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredShellSnapshot {
    pub schema_version: u32,
    pub environment_id: EnvironmentId,
    pub snapshot: OrchestrationV2ShellSnapshot,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<ShellOrigin>,
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredThreadSnapshot {
    pub schema_version: u32,
    pub environment_id: EnvironmentId,
    pub thread_id: ThreadId,
    pub snapshot: OrchestrationV2ThreadDetailSnapshot,
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredServerConfig {
    pub schema_version: u32,
    pub environment_id: EnvironmentId,
    pub config: ServerConfig,
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredVcsRefs {
    pub schema_version: u32,
    pub environment_id: EnvironmentId,
    pub cwd: String,
    pub refs: VcsListRefsResult,
}
pub fn decode_shell_snapshot(
    bytes: &[u8],
    environment: &EnvironmentId,
) -> Result<StoredShellSnapshot> {
    let mut raw: Value = parse(bytes)?;
    let snapshot = raw
        .as_object_mut()
        .and_then(|v| v.get_mut("snapshot"))
        .ok_or(PersistenceError::Invalid("shell snapshot"))?;
    normalize_shell_models(snapshot)?;
    let stored: StoredShellSnapshot =
        serde_json::from_value(raw).map_err(|_| PersistenceError::Invalid("shell cache"))?;
    if stored.schema_version != SHELL_CACHE_SCHEMA_VERSION || stored.environment_id != *environment
    {
        return invalid("shell cache identity/version");
    }
    Ok(stored)
}
pub fn decode_server_config(
    bytes: &[u8],
    environment: &EnvironmentId,
) -> Result<StoredServerConfig> {
    let stored: StoredServerConfig = parse(bytes)?;
    if stored.schema_version != 1 || stored.environment_id != *environment {
        return invalid("server config identity/version");
    }
    Ok(stored)
}
pub fn decode_vcs_refs(
    bytes: &[u8],
    environment: &EnvironmentId,
    cwd: &str,
) -> Result<StoredVcsRefs> {
    let stored: StoredVcsRefs = parse(bytes)?;
    if stored.schema_version != 1 || stored.environment_id != *environment || stored.cwd != cwd {
        return invalid("VCS refs identity/version");
    }
    Ok(stored)
}
pub fn decode_thread_snapshot(
    bytes: &[u8],
    environment: &EnvironmentId,
    thread: &ThreadId,
) -> Result<StoredThreadSnapshot> {
    let raw: Value = parse(bytes)?;
    let version = raw["schemaVersion"].as_u64();
    if !matches!(version, Some(5 | 6))
        || raw["environmentId"].as_str() != Some(&environment.0)
        || raw["threadId"].as_str() != Some(&thread.0)
    {
        return invalid("thread cache identity/version");
    }
    let mut raw_snapshot = raw["snapshot"].clone();
    let projection = raw_snapshot
        .as_object_mut()
        .and_then(|v| v.get_mut("projection"))
        .ok_or(PersistenceError::Invalid("thread projection"))?;
    normalize_projection_models(projection)?;
    let mut snapshot = crate::state::decoding::full_snapshot(raw_snapshot)
        .map_err(|_| PersistenceError::Invalid("thread cache snapshot"))?;
    if version == Some(6) {
        let metadata = &raw["snapshot"];
        let omitted = metadata["turnItemsOmitLocalVisible"]
            .as_bool()
            .ok_or(PersistenceError::Invalid("compact marker"))?;
        let count = metadata["turnItemCount"]
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .ok_or(PersistenceError::Invalid("compact count"))?;
        let visible = metadata["visibleTurnItemCount"]
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .ok_or(PersistenceError::Invalid("compact visible count"))?;
        snapshot.projection = crate::state::decoding::compact_projection(
            snapshot.projection,
            thread,
            omitted,
            count,
            visible,
        )
        .map_err(|_| PersistenceError::Invalid("compact projection integrity"))?;
    }
    if snapshot.projection.thread.id != IdOrIndex::Id(thread.clone()) {
        return invalid("thread projection identity");
    }
    Ok(StoredThreadSnapshot {
        schema_version: THREAD_CACHE_SCHEMA_VERSION,
        environment_id: environment.clone(),
        thread_id: thread.clone(),
        snapshot,
    })
}

struct EncodedRow<T> {
    source: Weak<T>,
    bytes: Vec<u8>,
}
pub struct RowEncoder<T> {
    rows: BTreeMap<usize, EncodedRow<T>>,
}
impl<T> Default for RowEncoder<T> {
    fn default() -> Self {
        Self {
            rows: BTreeMap::new(),
        }
    }
}
impl<T: Serialize> RowEncoder<T> {
    pub fn new() -> Self {
        Self::default()
    }
    pub async fn encode(
        &mut self,
        platform: &(impl Platform + ?Sized),
        rows: &[Rc<T>],
    ) -> Result<Vec<Value>> {
        self.rows.retain(|_, row| row.source.strong_count() > 0);
        let mut misses = 0;
        let mut output = Vec::with_capacity(rows.len());
        for row in rows {
            let address = Rc::as_ptr(row) as usize;
            if let std::collections::btree_map::Entry::Vacant(entry) = self.rows.entry(address) {
                if misses != 0 && misses % ROWS_PER_CHUNK == 0 {
                    platform.yield_to_host().await;
                }
                let bytes = encode(row.as_ref())?;
                entry.insert(EncodedRow {
                    source: Rc::downgrade(row),
                    bytes,
                });
                misses += 1;
            }
            let cached = self
                .rows
                .get(&address)
                .ok_or(PersistenceError::Invalid("row encoding"))?;
            output.push(parse(&cached.bytes)?);
        }
        if misses > 0 {
            platform.yield_to_host().await;
        }
        Ok(output)
    }
}

pub async fn encode_shell_snapshot(
    platform: &(impl Platform + ?Sized),
    stored: &StoredShellSnapshot,
) -> Result<Vec<u8>> {
    if stored.schema_version != SHELL_CACHE_SCHEMA_VERSION {
        return invalid("shell cache version");
    }
    let snapshot = &stored.snapshot;
    let mut raw = json!({"schemaVersion": stored.schema_version, "environmentId": stored.environment_id, "snapshot": {"schemaVersion": snapshot.schema_version, "snapshotSequence": snapshot.snapshot_sequence, "projects": snapshot.projects, "threads": [], "archivedThreads": []}});
    if let Some(older) = &snapshot.older_settled {
        raw["snapshot"]["olderSettled"] =
            serde_json::to_value(older).map_err(|_| PersistenceError::Invalid("older settled"))?;
    }
    if let Some(origin) = &stored.origin {
        raw["origin"] =
            serde_json::to_value(origin).map_err(|_| PersistenceError::Invalid("shell origin"))?;
    }
    let mut worked = false;
    for (name, rows) in [
        ("threads", &stored.snapshot.threads),
        ("archivedThreads", &stored.snapshot.archived_threads),
    ] {
        let mut encoded = Vec::with_capacity(rows.len());
        for chunk in rows.chunks(ROWS_PER_CHUNK) {
            if worked {
                platform.yield_to_host().await;
            }
            worked = true;
            for row in chunk {
                encoded.push(
                    serde_json::to_value(row)
                        .map_err(|_| PersistenceError::Invalid("shell row"))?,
                );
            }
        }
        raw["snapshot"][name] = Value::Array(encoded);
    }
    if worked {
        platform.yield_to_host().await;
    }
    normalize_shell_models(&mut raw["snapshot"])?;
    let bytes = encode(&raw)?;
    decode_shell_snapshot(&bytes, &stored.environment_id)?;
    Ok(bytes)
}
pub async fn encode_thread_snapshot(
    platform: &(impl Platform + ?Sized),
    stored: &StoredThreadSnapshot,
    max_json_length: usize,
) -> Result<Option<Vec<u8>>> {
    if stored.schema_version != THREAD_CACHE_SCHEMA_VERSION
        || stored.snapshot.projection.thread.id != IdOrIndex::Id(stored.thread_id.clone())
    {
        return invalid("thread cache identity/version");
    }
    let p = &stored.snapshot.projection;
    let mut worked = false;
    let mut length = 0;
    let mut raw = json!({"schemaVersion": stored.schema_version, "environmentId": stored.environment_id, "threadId": stored.thread_id, "snapshot": {"snapshotSequence": stored.snapshot.snapshot_sequence, "projection": {
        "thread": p.thread, "runs": p.runs, "attempts": p.attempts, "nodes": p.nodes, "subagents": p.subagents,
        "providerSessions": p.provider_sessions, "providerThreads": p.provider_threads, "providerTurns": p.provider_turns,
        "runtimeRequests": p.runtime_requests, "messages": p.messages, "plans": p.plans, "checkpointScopes": p.checkpoint_scopes,
        "checkpoints": p.checkpoints, "contextHandoffs": p.context_handoffs, "contextTransfers": p.context_transfers,
        "updatedAt": p.updated_at, "turnItems": [], "visibleTurnItems": []
    }}});
    if let Some(cursor) = &stored.snapshot.history_cursor {
        raw["snapshot"]["historyCursor"] = json!(cursor);
    }
    if let Some(more) = stored.snapshot.has_more_history {
        raw["snapshot"]["hasMoreHistory"] = json!(more);
    }
    if let Some(ordinal) = &stored.snapshot.latest_local_turn_ordinal {
        raw["snapshot"]["latestLocalTurnOrdinal"] = json!(ordinal);
    }
    let mut items = Vec::with_capacity(p.turn_items.len());
    for chunk in p.turn_items.chunks(ROWS_PER_CHUNK) {
        if worked {
            platform.yield_to_host().await;
        }
        worked = true;
        for row in chunk {
            let mut item =
                serde_json::to_value(row).map_err(|_| PersistenceError::Invalid("turn item"))?;
            normalize_model_field(&mut item, "modelSelection")?;
            let bytes = encode(&item)?;
            length += utf16_length(&bytes)?;
            if length > max_json_length {
                return Ok(None);
            }
            items.push(item);
        }
    }
    let mut rows = Vec::with_capacity(p.visible_turn_items.len());
    for chunk in p.visible_turn_items.chunks(ROWS_PER_CHUNK) {
        if worked {
            platform.yield_to_host().await;
        }
        worked = true;
        for row in chunk {
            let mut item = serde_json::to_value(row)
                .map_err(|_| PersistenceError::Invalid("visible turn item"))?;
            normalize_model_field(&mut item["item"], "modelSelection")?;
            let bytes = encode(&item)?;
            length += utf16_length(&bytes)?;
            if length > max_json_length {
                return Ok(None);
            }
            rows.push(item);
        }
    }
    raw["snapshot"]["projection"]["turnItems"] = Value::Array(items);
    raw["snapshot"]["projection"]["visibleTurnItems"] = Value::Array(rows);
    if worked {
        platform.yield_to_host().await;
    }
    normalize_projection_models(&mut raw["snapshot"]["projection"])?;
    let bytes = encode(&raw)?;
    Ok((utf16_length(&bytes)? <= max_json_length).then_some(bytes))
}
fn utf16_length(bytes: &[u8]) -> Result<usize> {
    Ok(std::str::from_utf8(bytes)
        .map_err(|_| PersistenceError::Invalid("UTF-8"))?
        .encode_utf16()
        .count())
}
fn normalize_model_field(value: &mut Value, field: &str) -> Result<()> {
    if let Some(model) = value.get_mut(field).filter(|v| !v.is_null()) {
        let name: t3_contracts::base_schemas::TrimmedNonEmptyString =
            serde_json::from_value(model["model"].clone())
                .map_err(|_| PersistenceError::Invalid("cache model selection"))?;
        model["model"] = serde_json::to_value(name)
            .map_err(|_| PersistenceError::Invalid("cache model selection"))?;
    }
    Ok(())
}
fn normalize_shell_models(snapshot: &mut Value) -> Result<()> {
    if !snapshot.is_object() {
        return invalid("shell snapshot");
    }
    for name in ["threads", "archivedThreads"] {
        if let Some(rows) = snapshot[name].as_array_mut() {
            for row in rows {
                normalize_model_field(row, "modelSelection")?;
            }
        }
    }
    if let Some(rows) = snapshot["projects"].as_array_mut() {
        for row in rows {
            normalize_model_field(row, "defaultModelSelection")?;
        }
    }
    Ok(())
}
fn normalize_projection_models(projection: &mut Value) -> Result<()> {
    if !projection.is_object() {
        return invalid("thread projection");
    }
    normalize_model_field(&mut projection["thread"], "modelSelection")?;
    for name in [
        "runs",
        "attempts",
        "nodes",
        "providerSessions",
        "providerThreads",
        "providerTurns",
        "turnItems",
    ] {
        if let Some(rows) = projection[name].as_array_mut() {
            for row in rows {
                normalize_model_field(row, "modelSelection")?;
            }
        }
    }
    if let Some(rows) = projection["visibleTurnItems"].as_array_mut() {
        for row in rows {
            normalize_model_field(&mut row["item"], "modelSelection")?;
        }
    }
    Ok(())
}

#[derive(Default)]
pub struct ThreadCacheWriteOrder {
    latest: u64,
    threads: BTreeMap<(EnvironmentId, ThreadId), u64>,
    environments: BTreeMap<EnvironmentId, u64>,
}
#[derive(Debug, Clone)]
pub struct CacheWriteTicket {
    environment: EnvironmentId,
    thread: ThreadId,
    ticket: u64,
}
impl ThreadCacheWriteOrder {
    pub fn begin_save(&mut self, environment: EnvironmentId, thread: ThreadId) -> CacheWriteTicket {
        self.latest += 1;
        self.threads
            .insert((environment.clone(), thread.clone()), self.latest);
        CacheWriteTicket {
            environment,
            thread,
            ticket: self.latest,
        }
    }
    pub fn is_current(&self, ticket: &CacheWriteTicket) -> bool {
        self.threads
            .get(&(ticket.environment.clone(), ticket.thread.clone()))
            == Some(&ticket.ticket)
            && self
                .environments
                .get(&ticket.environment)
                .copied()
                .unwrap_or(0)
                < ticket.ticket
    }
    pub fn remove_thread(&mut self, environment: EnvironmentId, thread: ThreadId) {
        self.begin_save(environment, thread);
    }
    pub fn clear_environment(&mut self, environment: EnvironmentId) {
        self.latest += 1;
        self.environments.insert(environment, self.latest);
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionCatalogDocument {
    pub schema_version: u32,
    pub targets: Vec<Value>,
    pub profiles: Vec<Value>,
    pub credentials: Vec<Value>,
    pub remote_dpop_tokens: Vec<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github_routing_permissions: Option<Vec<RoutingPermission>>,
    #[serde(default)]
    pub disabled_environment_ids: Vec<EnvironmentId>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}
impl Default for ConnectionCatalogDocument {
    fn default() -> Self {
        Self {
            schema_version: 1,
            targets: vec![],
            profiles: vec![],
            credentials: vec![],
            remote_dpop_tokens: vec![],
            github_routing_permissions: None,
            disabled_environment_ids: vec![],
            extra: BTreeMap::new(),
        }
    }
}
impl std::fmt::Debug for ConnectionCatalogDocument {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectionCatalogDocument")
            .field("targets", &self.targets.len())
            .field("profiles", &self.profiles.len())
            .finish_non_exhaustive()
    }
}
pub fn decode_catalog(bytes: &[u8]) -> Result<ConnectionCatalogDocument> {
    let doc: ConnectionCatalogDocument = parse(bytes)?;
    if doc.schema_version != 1
        || doc.targets.iter().any(|v| !valid_target(v))
        || doc.profiles.iter().any(|v| !valid_profile(v))
        || doc.credentials.iter().any(|v| {
            !has_strings(v, &["connectionId"])
                || v["credential"]["_tag"] != "BearerConnectionCredential"
                || !has_strings(&v["credential"], &["token"])
        })
        || doc.remote_dpop_tokens.iter().any(|v| {
            !has_strings(
                v,
                &["environmentId", "label", "accessToken", "dpopThumbprint"],
            ) || !has_strings(&v["endpoint"], &["httpBaseUrl", "wsBaseUrl"])
                || !v["expiresAtEpochMs"].is_number()
        })
    {
        return invalid("connection catalog");
    }
    Ok(doc)
}
fn has_strings(v: &Value, fields: &[&str]) -> bool {
    fields.iter().all(|key| v[*key].is_string())
}
fn valid_target(v: &Value) -> bool {
    has_strings(v, &["environmentId", "label"])
        && match v["_tag"].as_str() {
            Some("BearerConnectionTarget" | "SshConnectionTarget") => v["connectionId"].is_string(),
            Some("RelayConnectionTarget") => true,
            _ => false,
        }
}
fn valid_profile(v: &Value) -> bool {
    has_strings(v, &["connectionId", "environmentId", "label"])
        && match v["_tag"].as_str() {
            Some("BearerConnectionProfile") => has_strings(v, &["httpBaseUrl", "wsBaseUrl"]),
            Some("SshConnectionProfile") => serde_json::from_value::<
                t3_contracts::ipc::DesktopSshEnvironmentTargetSchema,
            >(v["target"].clone())
            .is_ok(),
            _ => false,
        }
}
pub fn replace_catalog_value(values: &mut Vec<Value>, field: &str, next: Value) {
    values.retain(|v| v[field] != next[field]);
    values.push(next);
}
pub fn catalog_routes<'a>(
    doc: &'a ConnectionCatalogDocument,
    environment: &EnvironmentId,
) -> Vec<&'a Value> {
    doc.targets
        .iter()
        .filter(|v| v["environmentId"] == environment.0)
        .collect()
}
fn route_key(v: &Value) -> &Value {
    v.get("connectionId").unwrap_or(&v["_tag"])
}
pub fn set_routes_in_catalog(
    doc: &mut ConnectionCatalogDocument,
    environment: &EnvironmentId,
    routes: Vec<Value>,
) -> Result<()> {
    if routes
        .iter()
        .any(|v| !valid_target(v) || v["environmentId"] != environment.0)
    {
        return invalid("connection routes");
    }
    let removed: Vec<_> = catalog_routes(doc, environment)
        .into_iter()
        .filter(|old| !routes.iter().any(|new| route_key(old) == route_key(new)))
        .cloned()
        .collect();
    let connection_ids: BTreeSet<_> = removed
        .iter()
        .filter_map(|v| v["connectionId"].as_str())
        .collect();
    doc.profiles.retain(|v| {
        v["connectionId"]
            .as_str()
            .is_none_or(|id| !connection_ids.contains(id))
    });
    doc.credentials.retain(|v| {
        v["connectionId"]
            .as_str()
            .is_none_or(|id| !connection_ids.contains(id))
    });
    if removed.iter().any(|v| v["_tag"] == "RelayConnectionTarget") {
        doc.remote_dpop_tokens
            .retain(|v| v["environmentId"] != environment.0);
    }
    let index = doc
        .targets
        .iter()
        .position(|v| v["environmentId"] == environment.0)
        .unwrap_or(doc.targets.len());
    doc.targets.retain(|v| v["environmentId"] != environment.0);
    doc.targets.splice(index..index, routes);
    Ok(())
}
pub fn register_connection_in_catalog(
    doc: &mut ConnectionCatalogDocument,
    registration: &Value,
    routes: Vec<Value>,
) -> Result<()> {
    let env = EnvironmentId(
        registration["target"]["environmentId"]
            .as_str()
            .ok_or(PersistenceError::Invalid("registration identity"))?
            .into(),
    );
    let mut next = doc.clone();
    set_routes_in_catalog(&mut next, &env, routes)?;
    match registration["_tag"].as_str() {
        Some("RelayConnectionRegistration") => {}
        Some("BearerConnectionRegistration" | "SshConnectionRegistration") => {
            replace_catalog_value(
                &mut next.profiles,
                "connectionId",
                registration["profile"].clone(),
            );
            replace_catalog_value(
                &mut next.credentials,
                "connectionId",
                json!({"connectionId": registration["target"]["connectionId"], "credential": registration["credential"]}),
            );
        }
        _ => return invalid("connection registration"),
    }
    decode_catalog(&encode(&next)?)?;
    *doc = next;
    Ok(())
}
pub fn remove_connection_from_catalog(
    doc: &mut ConnectionCatalogDocument,
    environment: &EnvironmentId,
) -> Result<()> {
    set_routes_in_catalog(doc, environment, vec![])?;
    doc.remote_dpop_tokens
        .retain(|v| v["environmentId"] != environment.0);
    doc.disabled_environment_ids.retain(|v| v != environment);
    if let Some(grants) = &mut doc.github_routing_permissions {
        grants.retain(|v| v.environment_id != *environment);
    }
    Ok(())
}
pub fn set_connection_enabled_in_catalog(
    doc: &mut ConnectionCatalogDocument,
    environment: &EnvironmentId,
    enabled: bool,
) {
    doc.disabled_environment_ids.retain(|v| v != environment);
    if !enabled && !catalog_routes(doc, environment).is_empty() {
        doc.disabled_environment_ids.push(environment.clone());
    }
}
pub fn put_remote_dpop_token_in_catalog(doc: &mut ConnectionCatalogDocument, token: Value) {
    if doc.targets.iter().any(|v| {
        v["_tag"] == "RelayConnectionTarget" && v["environmentId"] == token["environmentId"]
    }) {
        replace_catalog_value(&mut doc.remote_dpop_tokens, "environmentId", token);
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RoutingAccess {
    #[default]
    Off,
    Read,
    ReadWrite,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutingPermission {
    pub environment_id: EnvironmentId,
    pub connection_key: String,
    pub permission: RoutingAccess,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutingGrant {
    pub revision: u64,
    pub grant: RoutingPermission,
}
pub async fn load_routing_grant(
    storage: &(impl Storage + ?Sized),
    environment: &EnvironmentId,
) -> Result<Option<RoutingGrant>> {
    let key = StorageKey::new("routing-grant", Some(environment.clone()), "current");
    let grant = storage
        .get(StorageArea::Ordinary, key.clone())
        .await?
        .map(|bytes| decode_record::<RoutingGrant>(&bytes, &key, 1).map(|v| v.payload))
        .transpose()?;
    if grant
        .as_ref()
        .is_some_and(|v| v.grant.environment_id != *environment)
    {
        return invalid("routing environment identity");
    }
    Ok(grant)
}
pub async fn compare_exchange_routing_grant(
    storage: &(impl Storage + ?Sized),
    environment: &EnvironmentId,
    expected_revision: Option<u64>,
    connection_key: String,
    permission: RoutingAccess,
) -> Result<RoutingGrant> {
    let key = StorageKey::new("routing-grant", Some(environment.clone()), "current");
    let old = storage.get(StorageArea::Ordinary, key.clone()).await?;
    let previous = old
        .as_ref()
        .map(|bytes| decode_record::<RoutingGrant>(bytes, &key, 1))
        .transpose()?;
    if previous
        .as_ref()
        .is_some_and(|v| v.payload.grant.environment_id != *environment)
    {
        return invalid("routing environment identity");
    }
    if previous.as_ref().map(|v| v.payload.revision) != expected_revision {
        return Err(PersistenceError::Conflict);
    }
    let revision = expected_revision
        .unwrap_or(0)
        .checked_add(1)
        .ok_or(PersistenceError::Invalid("routing revision"))?;
    let grant = RoutingGrant {
        revision,
        grant: RoutingPermission {
            environment_id: environment.clone(),
            connection_key,
            permission,
        },
    };
    let record = Record::new(
        "routing-grant",
        Some(environment.clone()),
        "current",
        1,
        &grant,
    );
    let committed = storage
        .transaction(StorageTransaction {
            conditions: vec![StorageCondition {
                area: StorageArea::Ordinary,
                key: key.clone(),
                expected: old,
            }],
            ordinary: vec![StorageWrite {
                key,
                value: Some(record.encode()?),
            }],
            secure: vec![],
        })
        .await?;
    if !committed {
        return Err(PersistenceError::Conflict);
    }
    Ok(grant)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryCacheRecord {
    pub environment_id: EnvironmentId,
    pub payload: String,
    pub updated_at: u64,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryBatch {
    pub rows: usize,
    pub bytes: usize,
}
pub fn prune_query_cache(
    rows: &mut BTreeMap<String, QueryCacheRecord>,
    now: u64,
    reserve_batch: bool,
) {
    let max_rows = QUERY_MAX_ROWS
        - if reserve_batch {
            QUERY_PRUNE_BATCH_ROWS
        } else {
            0
        };
    let max_bytes = QUERY_MAX_BYTES
        - if reserve_batch {
            QUERY_PRUNE_BATCH_BYTES
        } else {
            0
        };
    let mut order: Vec<_> = rows
        .iter()
        .map(|(key, row)| (key.clone(), row.updated_at, row.payload.len()))
        .collect();
    order.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let mut kept = 0;
    let mut bytes = 0;
    for (key, updated, size) in order {
        if size > QUERY_MAX_PAYLOAD_BYTES
            || updated < now.saturating_sub(QUERY_MAX_AGE_MS)
            || kept >= max_rows
            || bytes + size > max_bytes
        {
            rows.remove(&key);
        } else {
            kept += 1;
            bytes += size;
        }
    }
}

pub struct EnvironmentCacheStore<P> {
    platform: P,
    write_order: RefCell<ThreadCacheWriteOrder>,
    thread_writes: futures::lock::Mutex<()>,
}
impl<P: Platform> EnvironmentCacheStore<P> {
    pub fn new(platform: P) -> Self {
        Self {
            platform,
            write_order: RefCell::new(ThreadCacheWriteOrder::default()),
            thread_writes: futures::lock::Mutex::new(()),
        }
    }
    pub fn platform(&self) -> &P {
        &self.platform
    }
    async fn load<T: DeserializeOwned>(
        &self,
        key: StorageKey,
        version: u32,
        decode: impl FnOnce(&[u8]) -> Result<T>,
    ) -> Result<Option<T>> {
        let Some(bytes) = self
            .platform
            .get(StorageArea::Ordinary, key.clone())
            .await?
        else {
            return Ok(None);
        };
        let result = decode_record::<Value>(&bytes, &key, version)
            .and_then(|r| decode(&encode(&r.payload)?));
        match result {
            Ok(v) => Ok(Some(v)),
            Err(_) => {
                // Conditional removal cannot delete a newer save that raced this read.
                let _ = self
                    .platform
                    .transaction(StorageTransaction {
                        conditions: vec![StorageCondition {
                            area: StorageArea::Ordinary,
                            key: key.clone(),
                            expected: Some(bytes),
                        }],
                        ordinary: vec![StorageWrite { key, value: None }],
                        secure: vec![],
                    })
                    .await?;
                Ok(None)
            }
        }
    }
    async fn save<T: Serialize>(&self, record: Record<T>) -> Result<()> {
        self.platform
            .write(
                StorageArea::Ordinary,
                vec![StorageWrite {
                    key: record.key(),
                    value: Some(record.encode()?),
                }],
            )
            .await?;
        Ok(())
    }
    pub async fn load_shell(
        &self,
        environment: &EnvironmentId,
    ) -> Result<Option<StoredShellSnapshot>> {
        self.load(
            StorageKey::new("cache.shell", Some(environment.clone()), "snapshot"),
            3,
            |b| decode_shell_snapshot(b, environment),
        )
        .await
    }
    pub async fn save_shell(&self, stored: &StoredShellSnapshot) -> Result<()> {
        let bytes = encode_shell_snapshot(&self.platform, stored).await?;
        self.save(Record::new(
            "cache.shell",
            Some(stored.environment_id.clone()),
            "snapshot",
            3,
            parse::<Value>(&bytes)?,
        ))
        .await
    }
    pub async fn load_thread(
        &self,
        environment: &EnvironmentId,
        thread: &ThreadId,
    ) -> Result<Option<StoredThreadSnapshot>> {
        self.load(
            StorageKey::new("cache.thread", Some(environment.clone()), &thread.0),
            5,
            |b| decode_thread_snapshot(b, environment, thread),
        )
        .await
    }
    pub async fn save_thread(&self, stored: &StoredThreadSnapshot) -> Result<()> {
        let ticket = self
            .write_order
            .borrow_mut()
            .begin_save(stored.environment_id.clone(), stored.thread_id.clone());
        let encoded =
            encode_thread_snapshot(&self.platform, stored, THREAD_CACHE_MAX_JSON_LENGTH).await?;
        let _write = self.thread_writes.lock().await;
        if !self.write_order.borrow().is_current(&ticket) {
            return Ok(());
        }
        let key = StorageKey::new(
            "cache.thread",
            Some(stored.environment_id.clone()),
            &stored.thread_id.0,
        );
        let value = encoded
            .map(|bytes| -> Result<_> {
                Record::new(
                    "cache.thread",
                    Some(stored.environment_id.clone()),
                    &stored.thread_id.0,
                    5,
                    parse::<Value>(&bytes)?,
                )
                .encode()
            })
            .transpose()?;
        self.platform
            .write(StorageArea::Ordinary, vec![StorageWrite { key, value }])
            .await?;
        Ok(())
    }
    pub async fn remove_thread(
        &self,
        environment: &EnvironmentId,
        thread: &ThreadId,
    ) -> Result<()> {
        self.write_order
            .borrow_mut()
            .remove_thread(environment.clone(), thread.clone());
        let _write = self.thread_writes.lock().await;
        self.platform
            .write(
                StorageArea::Ordinary,
                vec![StorageWrite {
                    key: StorageKey::new("cache.thread", Some(environment.clone()), &thread.0),
                    value: None,
                }],
            )
            .await?;
        Ok(())
    }
    pub async fn load_server_config(
        &self,
        environment: &EnvironmentId,
    ) -> Result<Option<StoredServerConfig>> {
        self.load(
            StorageKey::new("cache.server-config", Some(environment.clone()), "config"),
            1,
            |b| decode_server_config(b, environment),
        )
        .await
    }
    pub async fn save_server_config(&self, stored: &StoredServerConfig) -> Result<()> {
        decode_server_config(&encode(stored)?, &stored.environment_id)?;
        self.save(Record::new(
            "cache.server-config",
            Some(stored.environment_id.clone()),
            "config",
            1,
            stored,
        ))
        .await
    }
    pub async fn load_vcs_refs(
        &self,
        environment: &EnvironmentId,
        cwd: &str,
    ) -> Result<Option<StoredVcsRefs>> {
        self.load(
            StorageKey::new("cache.vcs-refs", Some(environment.clone()), cwd),
            1,
            |b| decode_vcs_refs(b, environment, cwd),
        )
        .await
    }
    pub async fn save_vcs_refs(&self, stored: &StoredVcsRefs) -> Result<()> {
        decode_vcs_refs(&encode(stored)?, &stored.environment_id, &stored.cwd)?;
        self.save(Record::new(
            "cache.vcs-refs",
            Some(stored.environment_id.clone()),
            &stored.cwd,
            1,
            stored,
        ))
        .await
    }
    pub async fn remove_vcs_refs(&self, environment: &EnvironmentId, cwd: &str) -> Result<()> {
        self.platform
            .write(
                StorageArea::Ordinary,
                vec![StorageWrite {
                    key: StorageKey::new("cache.vcs-refs", Some(environment.clone()), cwd),
                    value: None,
                }],
            )
            .await?;
        Ok(())
    }
    pub async fn clear_vcs_refs(&self, environment: &EnvironmentId) -> Result<()> {
        self.clear_namespaces(environment, &["cache.vcs-refs"])
            .await
    }
    pub async fn load_query(
        &self,
        environment: &EnvironmentId,
        id: &str,
    ) -> Result<Option<String>> {
        let row = self
            .load(
                StorageKey::new("cache.pull-request", Some(environment.clone()), id),
                1,
                |bytes| {
                    let row: QueryCacheRecord = parse(bytes)?;
                    if row.environment_id != *environment {
                        return invalid("query cache identity");
                    }
                    Ok(row)
                },
            )
            .await?;
        Ok(row
            .filter(|r| {
                r.updated_at
                    >= self
                        .platform
                        .now_epoch_ms()
                        .saturating_sub(QUERY_MAX_AGE_MS)
                    && r.payload.len() <= QUERY_MAX_PAYLOAD_BYTES
            })
            .map(|r| r.payload))
    }
    pub async fn save_query(
        &self,
        environment: &EnvironmentId,
        id: &str,
        payload: String,
    ) -> Result<()> {
        let batch_key = StorageKey::new("cache.query-batch", Some(environment.clone()), "batch");
        let old_batch = self
            .platform
            .get(StorageArea::Ordinary, batch_key.clone())
            .await?;
        let batch = old_batch
            .as_ref()
            .and_then(|b| decode_record::<QueryBatch>(b, &batch_key, 1).ok())
            .map(|r| r.payload);
        let key = StorageKey::new("cache.pull-request", Some(environment.clone()), id);
        let old = self
            .platform
            .get(StorageArea::Ordinary, key.clone())
            .await?;
        let mut transaction = StorageTransaction {
            conditions: vec![
                StorageCondition {
                    area: StorageArea::Ordinary,
                    key: batch_key.clone(),
                    expected: old_batch,
                },
                StorageCondition {
                    area: StorageArea::Ordinary,
                    key: key.clone(),
                    expected: old,
                },
            ],
            ..StorageTransaction::default()
        };
        if payload.len() > QUERY_MAX_PAYLOAD_BYTES {
            transaction.ordinary.push(StorageWrite { key, value: None });
        } else {
            let next_batch = QueryBatch {
                rows: batch.as_ref().map_or(0, |b| b.rows) + 1,
                bytes: batch.as_ref().map_or(0, |b| b.bytes) + payload.len(),
            };
            let prune = batch.is_none()
                || next_batch.rows >= QUERY_PRUNE_BATCH_ROWS
                || next_batch.bytes >= QUERY_PRUNE_BATCH_BYTES;
            let now = self.platform.now_epoch_ms();
            let row = QueryCacheRecord {
                environment_id: environment.clone(),
                payload,
                updated_at: now,
            };
            if prune {
                let mut rows = BTreeMap::new();
                for existing_key in self
                    .platform
                    .list(StorageArea::Ordinary, "cache.pull-request".into())
                    .await?
                {
                    if existing_key.environment.as_ref() != Some(environment) || existing_key == key
                    {
                        continue;
                    }
                    let bytes = self
                        .platform
                        .get(StorageArea::Ordinary, existing_key.clone())
                        .await?;
                    if let Some(bytes) = &bytes {
                        if let Ok(record) =
                            decode_record::<QueryCacheRecord>(bytes, &existing_key, 1)
                        {
                            rows.insert(existing_key.record.clone(), record.payload);
                        }
                        transaction.conditions.push(StorageCondition {
                            area: StorageArea::Ordinary,
                            key: existing_key.clone(),
                            expected: Some(bytes.clone()),
                        });
                    }
                    transaction.ordinary.push(StorageWrite {
                        key: existing_key,
                        value: None,
                    });
                }
                rows.insert(id.into(), row);
                prune_query_cache(&mut rows, now, true);
                for (id, row) in rows {
                    let record =
                        Record::new("cache.pull-request", Some(environment.clone()), id, 1, row);
                    transaction.ordinary.push(StorageWrite {
                        key: record.key(),
                        value: Some(record.encode()?),
                    });
                }
                transaction.ordinary.push(StorageWrite {
                    key: batch_key.clone(),
                    value: Some(
                        Record::new(
                            "cache.query-batch",
                            Some(environment.clone()),
                            "batch",
                            1,
                            QueryBatch::default(),
                        )
                        .encode()?,
                    ),
                });
            } else {
                transaction.ordinary.push(StorageWrite {
                    key,
                    value: Some(
                        Record::new("cache.pull-request", Some(environment.clone()), id, 1, row)
                            .encode()?,
                    ),
                });
                transaction.ordinary.push(StorageWrite {
                    key: batch_key.clone(),
                    value: Some(
                        Record::new(
                            "cache.query-batch",
                            Some(environment.clone()),
                            "batch",
                            1,
                            next_batch,
                        )
                        .encode()?,
                    ),
                });
            }
        }
        if !self.platform.transaction(transaction).await? {
            return Err(PersistenceError::Conflict);
        }
        Ok(())
    }
    pub async fn clear(&self, environment: &EnvironmentId) -> Result<()> {
        self.write_order
            .borrow_mut()
            .clear_environment(environment.clone());
        let _write = self.thread_writes.lock().await;
        self.clear_namespaces(
            environment,
            &[
                "cache.shell",
                "cache.thread",
                "cache.server-config",
                "cache.vcs-refs",
                "cache.pull-request",
                "cache.query-batch",
                "cache.project-favicon",
            ],
        )
        .await
    }
    async fn clear_namespaces(
        &self,
        environment: &EnvironmentId,
        namespaces: &[&str],
    ) -> Result<()> {
        let mut writes = Vec::new();
        for namespace in namespaces {
            for key in self
                .platform
                .list(StorageArea::Ordinary, (*namespace).into())
                .await?
            {
                if key.environment.as_ref() == Some(environment) {
                    writes.push(StorageWrite { key, value: None });
                }
            }
        }
        self.platform.write(StorageArea::Ordinary, writes).await?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "format", rename_all = "kebab-case")]
pub enum LegacyLocation {
    WebIndexedDb {
        database: String,
        store: String,
        key: String,
    },
    WebLocalStorage {
        key: String,
    },
    WebSessionStorage {
        key: String,
    },
    MobileSecure {
        key: String,
    },
    MobileCache {
        environment_id: EnvironmentId,
        kind: String,
        cache_key: String,
        schema_version: u32,
        updated_at: u64,
    },
    MobilePreferences {
        updated_at: u64,
    },
    MobileDocument {
        path: String,
    },
    /// Nonexportable WebCrypto keys stay in their host key store; only this reference migrates.
    WebCryptoKey {
        reference: String,
    },
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacyEntry {
    pub location: LegacyLocation,
    pub bytes: Vec<u8>,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacyNamespace {
    pub source: String,
    pub source_version: u32,
    pub namespace: String,
    pub entries: Vec<LegacyEntry>,
    pub resources: BTreeMap<String, Vec<u8>>,
}
pub async fn read_legacy_namespace(
    host: &(impl LegacyStorage + ?Sized),
    source: String,
    source_version: u32,
    namespace: String,
    locations: Vec<(String, LegacyLocation)>,
) -> Result<LegacyNamespace> {
    let mut entries = Vec::new();
    let mut resources = BTreeMap::new();
    for (key, location) in locations {
        let Some(bytes) = host
            .read_legacy(source.clone(), namespace.clone(), key)
            .await?
        else {
            continue;
        };
        let entry = LegacyEntry { location, bytes };
        if let Ok(records) = read_legacy_entry(&entry) {
            for record in records {
                for reference in attachment_references(&record.record.payload) {
                    if let std::collections::btree_map::Entry::Vacant(entry) =
                        resources.entry(reference.clone())
                    {
                        let bytes = host
                            .read_legacy_resource(source.clone(), reference.clone())
                            .await?
                            .ok_or(PersistenceError::MissingResources)?;
                        entry.insert(bytes);
                    }
                }
            }
        }
        entries.push(entry);
    }
    Ok(LegacyNamespace {
        source,
        source_version,
        namespace,
        entries,
        resources,
    })
}
impl std::fmt::Debug for LegacyNamespace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LegacyNamespace")
            .field("source", &self.source)
            .field("namespace", &self.namespace)
            .field("entries", &self.entries.len())
            .field("resources", &self.resources.len())
            .finish()
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportStatus {
    Imported,
    AlreadyImported,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportReport {
    pub status: ImportStatus,
    pub imported: usize,
    pub quarantined: usize,
    pub discarded_caches: usize,
    pub checksum: String,
}
#[derive(Clone)]
pub struct ImportedRecord {
    pub area: StorageArea,
    pub record: Record<Value>,
}
impl std::fmt::Debug for ImportedRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImportedRecord")
            .field("area", &self.area)
            .field("record", &self.record)
            .finish()
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuarantinedRecord {
    pub source: String,
    pub source_version: u32,
    pub location: LegacyLocation,
    pub reason: String,
    pub bytes_base64: String,
}

pub fn namespace_checksum(namespace: &LegacyNamespace) -> Result<String> {
    // Enumeration order is not identity; every location and resource byte participates in the digest.
    let mut entries = namespace
        .entries
        .iter()
        .map(encode)
        .collect::<Result<Vec<_>>>()?;
    entries.sort();
    let bytes = encode(&(
        &namespace.source,
        namespace.source_version,
        &namespace.namespace,
        entries,
        &namespace.resources,
    ))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
fn tuple_key<T: Serialize>(parts: &T) -> Result<String> {
    String::from_utf8(encode(parts)?).map_err(|_| PersistenceError::Invalid("record key"))
}
pub fn import_journal_key(namespace: &LegacyNamespace, checksum: &str) -> Result<StorageKey> {
    Ok(StorageKey::new(
        "import-journal",
        None,
        tuple_key(&(
            &namespace.source,
            namespace.source_version,
            &namespace.namespace,
            checksum,
        ))?,
    ))
}
fn imported(
    kind: &str,
    environment: Option<EnvironmentId>,
    id: impl Into<String>,
    version: u32,
    payload: Value,
    area: StorageArea,
) -> ImportedRecord {
    ImportedRecord {
        area,
        record: Record::new(kind, environment, id, version, payload),
    }
}

/// All writes, backups, attachments and the completion journal share one host transaction.
pub async fn import_namespace(
    storage: &(impl Storage + ?Sized),
    namespace: &LegacyNamespace,
) -> Result<ImportReport> {
    use base64::Engine;
    let checksum = namespace_checksum(namespace)?;
    let journal_key = import_journal_key(namespace, &checksum)?;
    let old_journal = storage
        .get(StorageArea::Ordinary, journal_key.clone())
        .await?;
    if let Some(bytes) = &old_journal {
        let record = decode_record::<MigrationJournal>(bytes, &journal_key, 1)?;
        if record.payload.completed
            && record.payload.source == namespace.source
            && record.payload.source_version == namespace.source_version
            && record.payload.checksum == checksum
        {
            return Ok(ImportReport {
                status: ImportStatus::AlreadyImported,
                imported: 0,
                quarantined: 0,
                discarded_caches: 0,
                checksum,
            });
        }
    }
    let mut report = ImportReport {
        status: ImportStatus::Imported,
        imported: 0,
        quarantined: 0,
        discarded_caches: 0,
        checksum: checksum.clone(),
    };
    let mut candidates = Vec::new();
    let mut entries = namespace.entries.iter().collect::<Vec<_>>();
    entries.sort_by_cached_key(|entry| tuple_key(&entry.location).unwrap_or_default());
    for entry in entries {
        let location_id = tuple_key(&(
            &namespace.source,
            namespace.source_version,
            &namespace.namespace,
            &entry.location,
            &checksum,
        ))?;
        candidates.push(imported("import-backup", None, &location_id, 1, json!({ "source": namespace.source, "location": entry.location, "bytesBase64": base64::engine::general_purpose::STANDARD.encode(&entry.bytes) }), StorageArea::Secure));
        match read_legacy_entry(entry) {
            Ok(records) => {
                for record in &records {
                    let references = attachment_references(&record.record.payload);
                    if references
                        .iter()
                        .any(|reference| !namespace.resources.contains_key(reference))
                    {
                        return Err(PersistenceError::MissingResources);
                    }
                }
                report.imported += records.len();
                candidates.extend(records);
            }
            Err(_) if is_derived_location(&entry.location) => {
                report.discarded_caches += 1;
            }
            Err(_) => {
                report.quarantined += 1;
                let quarantine = QuarantinedRecord {
                    source: namespace.source.clone(),
                    source_version: namespace.source_version,
                    location: entry.location.clone(),
                    reason: "invalid or unsupported user record".into(),
                    bytes_base64: base64::engine::general_purpose::STANDARD.encode(&entry.bytes),
                };
                candidates.push(imported(
                    "quarantine",
                    None,
                    location_id,
                    1,
                    serde_json::to_value(quarantine)
                        .map_err(|_| PersistenceError::Invalid("quarantine"))?,
                    StorageArea::Secure,
                ));
            }
        }
    }
    for (reference, bytes) in &namespace.resources {
        candidates.push(imported("attachment", None, reference, 1, json!({"reference": reference, "bytesBase64": base64::engine::general_purpose::STANDARD.encode(bytes)}), StorageArea::Ordinary));
    }
    let mut transaction = StorageTransaction {
        conditions: vec![StorageCondition {
            area: StorageArea::Ordinary,
            key: journal_key.clone(),
            expected: old_journal,
        }],
        ..StorageTransaction::default()
    };
    let mut planned: BTreeMap<(u8, StorageKey), Vec<u8>> = BTreeMap::new();
    for candidate in candidates {
        let key = candidate.record.key();
        let bytes = candidate.record.encode()?;
        let area_index = if candidate.area == StorageArea::Ordinary {
            0
        } else {
            1
        };
        let identity = (area_index, key.clone());
        let current = match planned.get(&identity) {
            Some(v) => Some(v.clone()),
            None => {
                let current = storage.get(candidate.area, key.clone()).await?;
                transaction.conditions.push(StorageCondition {
                    area: candidate.area,
                    key: key.clone(),
                    expected: current.clone(),
                });
                current
            }
        };
        if current.as_ref() == Some(&bytes) {
            continue;
        }
        if let Some(old) = current {
            if candidate.record.kind == "attachment" {
                return Err(PersistenceError::Conflict);
            }
            if candidate.record.kind == "preferences.mobile" && newer_preferences(&bytes, &old) {
                planned.insert(identity, bytes);
                continue;
            }
            // An edited destination always wins; keep the incoming alternative recoverable.
            let conflict_key = StorageKey::new(
                "import-conflict",
                key.environment.clone(),
                tuple_key(&(
                    &namespace.source,
                    &checksum,
                    &key,
                    format!("{:x}", Sha256::digest(&bytes)),
                ))?,
            );
            planned.insert((1, conflict_key.clone()), Record::new("import-conflict", conflict_key.environment.clone(), &conflict_key.record, 1, json!({"destination": key, "bytesBase64": base64::engine::general_purpose::STANDARD.encode(bytes)})).encode()?);
            report.quarantined += 1;
        } else {
            planned.insert(identity, bytes);
        }
    }
    for ((area, key), bytes) in planned {
        let writes = if area == 0 {
            &mut transaction.ordinary
        } else {
            &mut transaction.secure
        };
        writes.push(StorageWrite {
            key,
            value: Some(bytes),
        });
    }
    let journal = MigrationJournal {
        source: namespace.source.clone(),
        source_version: namespace.source_version,
        checksum,
        completed: true,
    };
    transaction.ordinary.push(StorageWrite {
        key: journal_key.clone(),
        value: Some(Record::new("import-journal", None, &journal_key.record, 1, journal).encode()?),
    });
    if !storage.transaction(transaction).await? {
        return Err(PersistenceError::Conflict);
    }
    Ok(report)
}
fn newer_preferences(incoming: &[u8], existing: &[u8]) -> bool {
    let new: std::result::Result<Record<Value>, _> = serde_json::from_slice(incoming);
    let old: std::result::Result<Record<Value>, _> = serde_json::from_slice(existing);
    match (new, old) {
        (Ok(new), Ok(old)) => {
            new.payload["updatedAt"].as_u64().unwrap_or(0)
                > old.payload["updatedAt"].as_u64().unwrap_or(0)
        }
        _ => false,
    }
}
fn attachment_references(value: &Value) -> BTreeSet<String> {
    fn visit(value: &Value, output: &mut BTreeSet<String>) {
        match value {
            Value::Array(values) => {
                for v in values {
                    visit(v, output);
                }
            }
            Value::Object(values) => {
                if let Some(uri) = values.get("fileUri").and_then(Value::as_str)
                    && !uri.is_empty()
                {
                    output.insert(uri.into());
                }
                for v in values.values() {
                    visit(v, output);
                }
            }
            _ => {}
        }
    }
    let mut output = BTreeSet::new();
    visit(value, &mut output);
    output
}
fn is_derived_location(location: &LegacyLocation) -> bool {
    match location {
        LegacyLocation::MobileCache { .. } => true,
        LegacyLocation::WebIndexedDb {
            store, database, ..
        } => {
            database == "t3code:project-favicons"
                || (database == "t3code:connection-runtime"
                    && matches!(
                        store.as_str(),
                        "shell" | "thread" | "server-config" | "vcs-refs" | "query"
                    ))
        }
        _ => false,
    }
}

pub fn read_legacy_entry(entry: &LegacyEntry) -> Result<Vec<ImportedRecord>> {
    match &entry.location {
        LegacyLocation::WebIndexedDb {
            database,
            store,
            key,
        } => read_web_indexed_db(database, store, key, &entry.bytes),
        LegacyLocation::MobileCache {
            environment_id,
            kind,
            cache_key,
            schema_version,
            updated_at,
        } => read_mobile_cache(
            environment_id,
            kind,
            cache_key,
            *schema_version,
            *updated_at,
            &entry.bytes,
        )
        .map(|v| vec![v]),
        LegacyLocation::MobilePreferences { updated_at } => {
            Ok(vec![read_mobile_preferences(&entry.bytes, *updated_at)?])
        }
        LegacyLocation::MobileSecure { key } => read_mobile_secure(key, &entry.bytes),
        LegacyLocation::MobileDocument { path } => read_mobile_document(path, &entry.bytes),
        LegacyLocation::WebLocalStorage { key } => read_web_storage(key, &entry.bytes, false),
        LegacyLocation::WebSessionStorage { key } => read_web_storage(key, &entry.bytes, true),
        LegacyLocation::WebCryptoKey { reference } => Ok(vec![imported(
            "signing-key-reference",
            None,
            "dpop",
            1,
            json!({"store": "t3code:cloud-auth", "key": "relay-dpop-proof-key", "reference": reference}),
            StorageArea::Secure,
        )]),
    }
}

pub fn read_web_indexed_db(
    database: &str,
    store: &str,
    key: &str,
    bytes: &[u8],
) -> Result<Vec<ImportedRecord>> {
    if database == "t3code:project-favicons" && store == "images" {
        return Ok(vec![imported(
            "cache.project-favicon",
            None,
            key,
            1,
            parse(bytes)?,
            StorageArea::Ordinary,
        )]);
    }
    if database != "t3code:connection-runtime" {
        return invalid("legacy database");
    }
    if store == "catalog" {
        if key == "document" {
            return split_catalog(decode_catalog(bytes)?);
        }
        return invalid("catalog key");
    }
    let raw: Value = parse(bytes)?;
    if store == "query" {
        if let Ok(batch) = serde_json::from_value::<QueryBatch>(raw.clone()) {
            let env = key
                .strip_suffix(":batch")
                .ok_or(PersistenceError::Invalid("query batch key"))?;
            return Ok(vec![imported(
                "cache.query-batch",
                Some(EnvironmentId(env.into())),
                "batch",
                1,
                serde_json::to_value(batch)
                    .map_err(|_| PersistenceError::Invalid("query batch"))?,
                StorageArea::Ordinary,
            )]);
        }
        let row: QueryCacheRecord = parse(bytes)?;
        let prefix = format!("{}:v1:", row.environment_id.0);
        let id = key
            .strip_prefix(&prefix)
            .ok_or(PersistenceError::Invalid("query identity"))?;
        return Ok(vec![imported(
            "cache.pull-request",
            Some(row.environment_id.clone()),
            id,
            1,
            raw,
            StorageArea::Ordinary,
        )]);
    }
    let env = EnvironmentId(
        raw["environmentId"]
            .as_str()
            .ok_or(PersistenceError::Invalid("cache environment"))?
            .into(),
    );
    let (kind, id, version, payload) = match store {
        "shell" if key == env.0 => {
            decode_shell_snapshot(bytes, &env)?;
            ("shell", "snapshot".to_string(), 3, raw)
        }
        "thread" => {
            let thread = ThreadId(
                raw["threadId"]
                    .as_str()
                    .ok_or(PersistenceError::Invalid("thread id"))?
                    .into(),
            );
            if key != format!("{}:{}", env.0, thread.0) {
                return invalid("thread key");
            }
            let stored = decode_thread_snapshot(bytes, &env, &thread)?;
            (
                "thread",
                thread.0,
                5,
                serde_json::to_value(stored)
                    .map_err(|_| PersistenceError::Invalid("thread cache"))?,
            )
        }
        "server-config" if key == env.0 => {
            decode_server_config(bytes, &env)?;
            ("server-config", "config".to_string(), 1, raw)
        }
        "vcs-refs" => {
            let cwd = raw["cwd"]
                .as_str()
                .ok_or(PersistenceError::Invalid("refs cwd"))?
                .to_string();
            if key != format!("{}:{}", env.0, cwd) {
                return invalid("refs key");
            }
            decode_vcs_refs(bytes, &env, &cwd)?;
            ("vcs-refs", cwd, 1, raw)
        }
        _ => return invalid("legacy cache store/key"),
    };
    Ok(vec![imported(
        &format!("cache.{kind}"),
        Some(env),
        id,
        version,
        payload,
        StorageArea::Ordinary,
    )])
}
pub fn read_mobile_cache(
    environment: &EnvironmentId,
    kind: &str,
    key: &str,
    schema_version: u32,
    updated_at: u64,
    bytes: &[u8],
) -> Result<ImportedRecord> {
    let raw: Value = parse(bytes)?;
    if matches!(kind, "shell" | "thread" | "server-config" | "vcs-refs")
        && raw["schemaVersion"].as_u64() != Some(u64::from(schema_version))
    {
        return invalid("mobile cache row/envelope version");
    }
    let (version, payload) = match kind {
        "shell" if key == "snapshot" && schema_version == 3 => {
            decode_shell_snapshot(bytes, environment)?;
            (3, raw)
        }
        "thread" if matches!(schema_version, 5 | 6) => {
            let stored = decode_thread_snapshot(bytes, environment, &ThreadId(key.into()))?;
            (
                5,
                serde_json::to_value(stored)
                    .map_err(|_| PersistenceError::Invalid("thread cache"))?,
            )
        }
        "server-config" if key == "config" && schema_version == 1 => {
            decode_server_config(bytes, environment)?;
            (1, raw)
        }
        "vcs-refs" if schema_version == 1 => {
            decode_vcs_refs(bytes, environment, key)?;
            (1, raw)
        }
        "pull-request" if schema_version == 1 => (
            1,
            json!({"environmentId": environment, "payload": std::str::from_utf8(bytes).map_err(|_| PersistenceError::Invalid("query payload"))?, "updatedAt": updated_at}),
        ),
        "project-favicon" if schema_version == 1 => (1, raw),
        _ => return invalid("mobile cache kind/key/version"),
    };
    Ok(imported(
        &format!("cache.{kind}"),
        Some(environment.clone()),
        key,
        version,
        payload,
        StorageArea::Ordinary,
    ))
}
pub fn read_mobile_preferences(bytes: &[u8], updated_at: u64) -> Result<ImportedRecord> {
    let preferences: Value = parse(bytes)?;
    if !preferences.is_object() {
        return invalid("mobile preferences");
    }
    Ok(imported(
        "preferences.mobile",
        None,
        "current",
        1,
        json!({"updatedAt": updated_at, "preferences": preferences}),
        StorageArea::Ordinary,
    ))
}
pub fn read_mobile_secure(key: &str, bytes: &[u8]) -> Result<Vec<ImportedRecord>> {
    match key {
        "t3code.connection-catalog.v1" => split_catalog(decode_catalog(bytes)?),
        "t3code.connections" => split_catalog(migrate_legacy_connections(bytes)?),
        "t3code.preferences" => Ok(vec![read_mobile_preferences(bytes, 0)?]),
        "t3code.preferences.fallback" => {
            let raw: Value = parse(bytes)?;
            let payload = raw["payload"]
                .as_str()
                .ok_or(PersistenceError::Invalid("preferences fallback"))?;
            let updated = raw["updatedAt"]
                .as_u64()
                .ok_or(PersistenceError::Invalid("preferences timestamp"))?;
            Ok(vec![read_mobile_preferences(payload.as_bytes(), updated)?])
        }
        "t3code.agent-awareness.device-id" => {
            let id =
                std::str::from_utf8(bytes).map_err(|_| PersistenceError::Invalid("device id"))?;
            if id.trim().is_empty() {
                return invalid("device id");
            }
            Ok(vec![imported(
                "device-identity",
                None,
                "agent-awareness",
                1,
                json!(id),
                StorageArea::Secure,
            )])
        }
        "t3code.agent-awareness.registration" => {
            let raw: Value = parse(bytes)?;
            if !has_strings(&raw, &["identity", "signature"]) {
                return invalid("agent registration");
            }
            Ok(vec![imported(
                "agent-registration",
                None,
                "current",
                1,
                raw,
                StorageArea::Secure,
            )])
        }
        "t3code.recent-thread-shortcuts" => {
            let raw: Value = parse(bytes)?;
            if raw["threads"].as_array().is_none_or(|rows| {
                rows.iter()
                    .any(|row| !has_strings(row, &["environmentId", "threadId", "title"]))
            }) {
                return invalid("recent shortcuts");
            }
            Ok(vec![imported(
                "recent-thread-shortcuts",
                None,
                "current",
                1,
                raw,
                StorageArea::Ordinary,
            )])
        }
        "t3code.cloud.dpop-proof-key" => {
            let raw: Value = parse(bytes)?;
            if raw["kty"] != "EC" || raw["crv"] != "P-256" || !has_strings(&raw, &["x", "y", "d"]) {
                return invalid("DPoP key");
            }
            Ok(vec![imported(
                "signing-key",
                None,
                "dpop",
                1,
                raw,
                StorageArea::Secure,
            )])
        }
        "t3code.cloud.relay-access-tokens" => {
            let raw: Value = parse(bytes)?;
            if raw["version"] != 1
                || raw["entries"].as_array().is_none_or(|rows| {
                    rows.iter().any(|v| {
                        !has_strings(
                            v,
                            &[
                                "accountId",
                                "clientId",
                                "relayUrl",
                                "thumbprint",
                                "accessToken",
                            ],
                        ) || !v["scopes"].is_array()
                            || !v["expiresAtMillis"].is_number()
                    })
                })
            {
                return invalid("managed relay tokens");
            }
            let entries = raw["entries"]
                .as_array()
                .ok_or(PersistenceError::Invalid("relay token entries"))?;
            let mut groups: BTreeMap<StorageKey, Vec<Value>> = BTreeMap::new();
            for entry in entries {
                serde_json::from_value::<crate::relay::RelayAccessTokenCacheEntry>(entry.clone())
                    .map_err(|_| PersistenceError::Invalid("relay token entry"))?;
                let url = entry["relayUrl"]
                    .as_str()
                    .ok_or(PersistenceError::Invalid("relay URL"))?;
                let client = entry["clientId"]
                    .as_str()
                    .ok_or(PersistenceError::Invalid("relay client id"))?;
                let key = relay_dpop_cache_key(url, client)?;
                let mut entry = entry.clone();
                entry["relayUrl"] = json!(
                    crate::relay::normalize_secure_relay_url(url)
                        .map_err(|_| PersistenceError::Invalid("relay URL"))?
                );
                groups.entry(key).or_default().push(entry);
            }
            Ok(groups
                .into_iter()
                .map(|(key, entries)| {
                    imported(
                        RELAY_DPOP_TOKEN_KIND,
                        None,
                        key.record,
                        1,
                        Value::Array(entries),
                        StorageArea::Secure,
                    )
                })
                .collect())
        }
        _ => Ok(vec![imported(
            "legacy.mobile-secure",
            None,
            key,
            1,
            parse(bytes)?,
            StorageArea::Secure,
        )]),
    }
}
fn split_catalog(mut doc: ConnectionCatalogDocument) -> Result<Vec<ImportedRecord>> {
    let mut records = vec![imported(
        "connection-catalog-source",
        None,
        "current",
        1,
        serde_json::to_value(&doc).map_err(|_| PersistenceError::Invalid("catalog source"))?,
        StorageArea::Secure,
    )];
    for credential in doc.credentials.drain(..) {
        let id = credential["connectionId"]
            .as_str()
            .ok_or(PersistenceError::Invalid("credential id"))?
            .to_string();
        records.push(imported(
            "connection-credential",
            None,
            id,
            1,
            credential,
            StorageArea::Secure,
        ));
    }
    for token in doc.remote_dpop_tokens.drain(..) {
        let env = EnvironmentId(
            token["environmentId"]
                .as_str()
                .ok_or(PersistenceError::Invalid("token identity"))?
                .into(),
        );
        records.push(imported(
            REMOTE_DPOP_TOKEN_KIND,
            Some(env),
            "current",
            1,
            token,
            StorageArea::Secure,
        ));
    }
    if let Some(grants) = doc.github_routing_permissions.take() {
        for grant in grants {
            let env = grant.environment_id.clone();
            records.push(imported(
                "routing-grant",
                Some(env),
                "current",
                1,
                serde_json::to_value(RoutingGrant { revision: 1, grant })
                    .map_err(|_| PersistenceError::Invalid("routing grant"))?,
                StorageArea::Ordinary,
            ));
        }
    }
    let unknown = std::mem::take(&mut doc.extra);
    if !unknown.is_empty() {
        records.push(imported(
            "connection-catalog-extra",
            None,
            "current",
            1,
            json!(unknown),
            StorageArea::Secure,
        ));
    }
    // Profile URLs may contain credentials; public metadata references secure profiles by id.
    for profile in doc.profiles.drain(..) {
        let id = profile["connectionId"]
            .as_str()
            .ok_or(PersistenceError::Invalid("profile id"))?
            .to_string();
        records.push(imported(
            "connection-profile",
            None,
            id,
            1,
            profile,
            StorageArea::Secure,
        ));
    }
    let targets = doc.targets.iter().map(|target| {
        let mut public = json!({"_tag": target["_tag"], "environmentId": target["environmentId"], "label": target["label"]});
        if let Some(id) = target.get("connectionId") { public["connectionId"] = id.clone(); } public
    }).collect::<Vec<_>>();
    records.push(imported("connection-catalog", None, "current", 1, json!({"schemaVersion": 1, "targets": targets, "disabledEnvironmentIds": doc.disabled_environment_ids}), StorageArea::Ordinary));
    Ok(records)
}
pub fn migrate_legacy_connections(bytes: &[u8]) -> Result<ConnectionCatalogDocument> {
    let raw: Value = parse(bytes)?;
    if !raw.is_object() {
        return invalid("legacy connections");
    }
    let empty = vec![];
    let connections = match raw.get("connections") {
        None => &empty,
        Some(v) => v
            .as_array()
            .ok_or(PersistenceError::Invalid("legacy connections"))?,
    };
    let mut doc = ConnectionCatalogDocument::default();
    for connection in connections {
        if !has_strings(
            connection,
            &[
                "environmentId",
                "environmentLabel",
                "pairingUrl",
                "displayUrl",
                "httpBaseUrl",
                "wsBaseUrl",
            ],
        ) || !(connection["bearerToken"].is_string()
            || connection.get("bearerToken") == Some(&Value::Null))
        {
            return invalid("legacy connection");
        }
        let env = EnvironmentId(
            connection["environmentId"]
                .as_str()
                .ok_or(PersistenceError::Invalid("legacy environment"))?
                .into(),
        );
        let relay =
            connection["relayManaged"] == true || connection["authenticationMethod"] == "dpop";
        let registration = if relay {
            json!({"_tag": "RelayConnectionRegistration", "target": {"_tag": "RelayConnectionTarget", "environmentId": env, "label": connection["environmentLabel"]}})
        } else {
            let Some(token) = connection["bearerToken"]
                .as_str()
                .filter(|token| !token.trim().is_empty())
            else {
                continue;
            };
            let id = format!("bearer:{}", env.0);
            json!({"_tag": "BearerConnectionRegistration", "target": {"_tag": "BearerConnectionTarget", "environmentId": env, "label": connection["environmentLabel"], "connectionId": id}, "profile": {"_tag": "BearerConnectionProfile", "environmentId": env, "label": connection["environmentLabel"], "connectionId": id, "httpBaseUrl": connection["httpBaseUrl"], "wsBaseUrl": connection["wsBaseUrl"]}, "credential": {"_tag": "BearerConnectionCredential", "token": token}})
        };
        register_connection_in_catalog(
            &mut doc,
            &registration,
            vec![registration["target"].clone()],
        )?;
    }
    Ok(doc)
}

pub fn read_mobile_document(path: &str, bytes: &[u8]) -> Result<Vec<ImportedRecord>> {
    if path == "voice-audio-grant-id" {
        let id = std::str::from_utf8(bytes)
            .map_err(|_| PersistenceError::Invalid("voice grant marker"))?;
        return Ok(vec![imported(
            "voice-grant-marker",
            None,
            "mobile",
            1,
            json!(id),
            StorageArea::Ordinary,
        )]);
    }
    let raw: Value = parse(bytes)?;
    if let Some(file) = path
        .strip_prefix("thread-outbox/")
        .and_then(|p| p.strip_suffix(".json"))
    {
        let message = decode_queued_message(bytes)?;
        let message_id = message["messageId"]
            .as_str()
            .ok_or(PersistenceError::Invalid("outbox message id"))?;
        if encode_uri_component(message_id) != file {
            return invalid("outbox filename identity");
        }
        let env = EnvironmentId(
            message["environmentId"]
                .as_str()
                .ok_or(PersistenceError::Invalid("outbox environment"))?
                .into(),
        );
        return Ok(vec![imported(
            "outbox",
            Some(env),
            message_id,
            1,
            message.clone(),
            StorageArea::Ordinary,
        )]);
    }
    match path {
        "composer-drafts/drafts.json" => {
            if raw["schemaVersion"] != 1
                || raw["drafts"]
                    .as_object()
                    .is_none_or(|drafts| drafts.values().any(|draft| !valid_mobile_draft(draft)))
            {
                return invalid("composer drafts");
            }
            if let Some(accounts) = raw.get("signedOutDrafts") {
                let accounts = accounts
                    .as_object()
                    .ok_or(PersistenceError::Invalid("signed out drafts"))?;
                for account in accounts.values() {
                    if account["drafts"].as_object().is_none_or(|drafts| {
                        drafts.values().any(|draft| !valid_mobile_draft(draft))
                    }) || account["queuedMessages"].as_array().is_none_or(|messages| {
                        messages
                            .iter()
                            .any(|m| encode(m).and_then(|b| decode_queued_message(&b)).is_err())
                    }) {
                        return invalid("signed out draft contents");
                    }
                }
            }
            Ok(vec![imported(
                "composer-drafts.mobile",
                None,
                "current",
                1,
                raw,
                StorageArea::Ordinary,
            )])
        }
        "pull-request-drafts/drafts.json" => {
            if raw["version"] != 1
                || raw["drafts"].as_object().is_none_or(|drafts| {
                    drafts.values().any(|draft| {
                        !has_strings(draft, &["summary", "comment"])
                            || !draft["comments"].is_array()
                    })
                })
            {
                return invalid("review drafts");
            }
            Ok(vec![imported(
                "review-drafts",
                None,
                "current",
                1,
                raw,
                StorageArea::Ordinary,
            )])
        }
        "voice-audio-grant-id" => Ok(vec![imported(
            "voice-grant",
            None,
            "mobile",
            1,
            raw,
            StorageArea::Ordinary,
        )]),
        _ if path.starts_with("incoming-shares/") => {
            if raw["schemaVersion"] != 1
                || !has_strings(&raw, &["id", "createdAt", "text"])
                || raw["attachments"]
                    .as_array()
                    .is_none_or(|a| !a.iter().all(valid_attachment))
                || raw["warnings"]
                    .as_array()
                    .is_none_or(|a| !a.iter().all(Value::is_string))
            {
                return invalid("incoming share");
            }
            Ok(vec![imported(
                "incoming-share",
                None,
                path,
                1,
                raw,
                StorageArea::Ordinary,
            )])
        }
        _ => Ok(vec![imported(
            "legacy.mobile-document",
            None,
            path,
            1,
            raw,
            StorageArea::Secure,
        )]),
    }
}
pub fn decode_queued_message(bytes: &[u8]) -> Result<Value> {
    let raw: Value = parse(bytes)?;
    crate::thread_views::outbox::decode_message(raw.clone())
        .map_err(|_| PersistenceError::Invalid("queued message"))?;
    Ok(raw)
}
fn valid_mobile_draft(draft: &Value) -> bool {
    draft["text"].is_string()
        && draft["attachments"]
            .as_array()
            .is_some_and(|a| a.iter().all(valid_attachment))
        && valid_draft_settings(draft)
        && draft
            .get("workspaceSelection")
            .is_none_or(valid_workspace_selection)
        && draft
            .get("importedShareIds")
            .is_none_or(|v| v.as_array().is_some_and(|v| v.iter().all(Value::is_string)))
        && draft
            .get("project")
            .is_none_or(|v| has_strings(v, &["environmentId", "projectId", "createdAt"]))
}
fn valid_draft_settings(value: &Value) -> bool {
    fn optional<T: DeserializeOwned>(value: &Value, key: &str) -> bool {
        value
            .get(key)
            .is_none_or(|v| serde_json::from_value::<T>(v.clone()).is_ok())
    }
    optional::<t3_contracts::model_selection::ModelSelection>(value, "modelSelection")
        && optional::<t3_contracts::provider_policy::RuntimeMode>(value, "runtimeMode")
        && optional::<t3_contracts::provider_policy::ProviderInteractionMode>(
            value,
            "interactionMode",
        )
        && value.get("freshContext").is_none_or(Value::is_boolean)
        && value
            .get("context")
            .is_none_or(|v| v["version"] == 1 && v["records"].is_array())
}
fn valid_workspace_selection(value: &Value) -> bool {
    matches!(
        value
            .get("mode")
            .or_else(|| value.get("workspaceMode"))
            .and_then(Value::as_str),
        Some("local" | "worktree")
    ) && ["branch", "worktreePath"].iter().all(|key| {
        value
            .get(*key)
            .is_some_and(|v| v.is_null() || v.is_string())
    }) && value.get("startFromOrigin").is_none_or(Value::is_boolean)
        && value
            .get("pullRequestReference")
            .is_none_or(Value::is_string)
}
fn valid_attachment(value: &Value) -> bool {
    crate::thread_views::outbox::DraftAttachment::decode(value.clone()).is_ok()
}
pub fn encode_uri_component(value: &str) -> String {
    let mut output = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            output.push(byte as char);
        } else {
            output.push('%');
            output.push(char::from(b"0123456789ABCDEF"[(byte >> 4) as usize]));
            output.push(char::from(b"0123456789ABCDEF"[(byte & 15) as usize]));
        }
    }
    output
}

pub fn read_web_storage(key: &str, bytes: &[u8], session: bool) -> Result<Vec<ImportedRecord>> {
    let text =
        std::str::from_utf8(bytes).map_err(|_| PersistenceError::Invalid("web storage UTF-8"))?;
    if let Some(environment) = key.strip_prefix(ROUTING_KEY_PREFIX) {
        let grant: RoutingPermission = parse(bytes)?;
        if grant.environment_id.0 != environment {
            return invalid("routing grant identity");
        }
        return Ok(vec![imported(
            "routing-grant",
            Some(grant.environment_id.clone()),
            "current",
            1,
            json!(RoutingGrant { revision: 1, grant }),
            StorageArea::Ordinary,
        )]);
    }
    let value = match serde_json::from_slice::<Value>(bytes) {
        _ if web_string_key(key) => Value::String(text.into()),
        Ok(value) => value,
        Err(_) => return invalid("web storage JSON"),
    };
    let (kind, version) = match key {
        "t3code:composer-drafts:v1" => {
            let version = value["version"]
                .as_u64()
                .ok_or(PersistenceError::Invalid("web draft version"))?;
            if version > 9 || !value["state"].is_object() {
                return invalid("web draft document");
            }
            ("composer-drafts.web", 9)
        }
        "t3code:prompt-stash:v2" | "t3code:prompt-stash:v1" => {
            if !value["state"]["entries"].is_array() && !value["entries"].is_array() {
                return invalid("prompt stash");
            }
            ("prompt-stash", if key.ends_with("v2") { 2 } else { 1 })
        }
        "t3code:right-panel-state:v2" => ("panel-state", 14),
        "t3code:terminal-state:v1" => ("terminal-state", 1),
        "t3code:ui-state:v1" => ("ui-state", 1),
        "t3code:client-settings:v1" => ("client-settings", 1),
        "t3code:themes:v1" => ("theme-library", 1),
        "t3code:browser-history:v1" => ("browser-history", 1),
        "t3code:browser-favicons:v1" => ("browser-favicons", 1),
        _ => (
            if session {
                "legacy.web-session"
            } else {
                "preferences.web"
            },
            1,
        ),
    };
    // Keep the original key and complete payload until its feature owns a typed migration.
    Ok(vec![imported(
        kind,
        None,
        if kind == "composer-drafts.web" {
            "current"
        } else {
            key
        },
        version,
        value,
        StorageArea::Ordinary,
    )])
}
fn web_string_key(key: &str) -> bool {
    matches!(
        key,
        "t3code:theme"
            | "t3code:theme-appearance-mode"
            | "t3code:theme-follow-system"
            | "t3code:last-enabled-project-grouping-mode"
            | "t3code:analytics-device-id"
            | "t3.backgroundActivity.clientId"
            | "t3code:last-editor"
            | "t3code:snap-shot-setup-resume:v1"
            | "t3code:remote-open-hint-seen"
            | "t3code:github-star-prompt:sent"
            | "t3code:github-star-prompt:dismissed"
            | "t3code:boot-failed-at"
            | "t3code:chunk-load-reloaded"
    ) || key.starts_with("t3code:default-theme-applied:v2:")
}
