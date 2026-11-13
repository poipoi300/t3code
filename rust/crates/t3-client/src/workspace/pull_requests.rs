use super::ProjectKey;
use crate::{
    ClientError,
    platform::{Storage, StorageArea, StorageKey, StorageWrite},
    types::{EnvironmentId, Generation},
};
use serde::{Serialize, de::DeserializeOwned};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use t3_contracts::pull_request::{
    PullRequestRef, PullRequestRoutingIdentityResult, PullRequestRoutingResult,
};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PullRequestKey {
    pub project: ProjectKey,
    pub host: Option<String>,
    pub repository: String,
    pub number: u64,
    pub expected_account_id: Option<String>,
}
impl PullRequestKey {
    pub fn new(environment: EnvironmentId, reference: &PullRequestRef) -> Self {
        Self {
            project: ProjectKey {
                environment,
                project: reference.project_id.clone(),
            },
            host: reference
                .host
                .as_ref()
                .and_then(Option::as_ref)
                .map(|v| v.0.to_lowercase()),
            repository: reference.repository.0.to_lowercase(),
            number: reference.number,
            expected_account_id: reference
                .expected_account_id
                .as_ref()
                .and_then(Option::as_ref)
                .map(|v| v.0.clone()),
        }
    }
    pub fn matches_invalidation(&self, filter: &Self) -> bool {
        self.project == filter.project
            && self.repository == filter.repository
            && self.number == filter.number
            && (filter.host.is_none() || self.host == filter.host)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum QueryKind {
    List,
    ListStats,
    Summary,
    Stack,
    Activity,
    Detail,
    Preview,
    LabelCandidates,
    ReviewerCandidates,
    ReviewGuide,
    ChangeBreakdown,
    Diff,
    Checks,
    FilterOptions,
    FilesViewed,
    LinkedThreads,
    ThreadComments,
    DiffFileContents,
    LinkedSummary,
    PreviewCard,
    ReviewerSearch,
    LinkedThreadsUncached,
}
impl QueryKind {
    pub fn stale_time_ms(self) -> u64 {
        match self {
            Self::List => 30_000,
            Self::Checks => 45_000,
            Self::FilterOptions => 300_000,
            Self::FilesViewed => 15_000,
            Self::LinkedThreads
            | Self::LinkedThreadsUncached
            | Self::ThreadComments
            | Self::DiffFileContents => 0,
            _ => 60_000,
        }
    }
    pub fn persisted(self) -> bool {
        matches!(
            self,
            Self::Summary
                | Self::Detail
                | Self::Preview
                | Self::List
                | Self::Activity
                | Self::Checks
                | Self::ReviewGuide
                | Self::Diff
                | Self::FilesViewed
                | Self::ListStats
                | Self::LinkedSummary
                | Self::PreviewCard
                | Self::LinkedThreads
                | Self::ChangeBreakdown
        )
    }
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct QueryKey {
    pub environment: EnvironmentId,
    pub kind: QueryKind,
    pub input: String,
}
impl QueryKey {
    pub fn new(
        environment: EnvironmentId,
        kind: QueryKind,
        input: &impl Serialize,
    ) -> Result<Self, ClientError> {
        Ok(Self {
            environment,
            kind,
            input: serde_json::to_string(input)?,
        })
    }
    pub fn reference(
        environment: EnvironmentId,
        kind: QueryKind,
        reference: &PullRequestRef,
    ) -> Result<Self, ClientError> {
        Self::new(environment, kind, reference)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueryTicket {
    pub session: Generation,
    pub revision: u64,
}
#[derive(Debug, Clone)]
pub struct CachedQuery<T> {
    pub value: Option<T>,
    pub error: Option<String>,
    pub loading: bool,
    pub updated_at: Option<u64>,
    revision: u64,
    session: Generation,
}
impl<T> Default for CachedQuery<T> {
    fn default() -> Self {
        Self {
            value: None,
            error: None,
            loading: false,
            updated_at: None,
            revision: 0,
            session: Generation(0),
        }
    }
}
impl<T> CachedQuery<T> {
    pub fn ticket(&self) -> QueryTicket {
        QueryTicket {
            session: self.session,
            revision: self.revision,
        }
    }
    pub fn begin(&mut self, session: Generation) -> QueryTicket {
        self.begin_revision(session, self.revision + 1)
    }
    pub(crate) fn begin_revision(&mut self, session: Generation, revision: u64) -> QueryTicket {
        self.revision = revision;
        self.session = session;
        self.loading = true;
        self.error = None;
        QueryTicket {
            session,
            revision: self.revision,
        }
    }
    pub fn finish(&mut self, ticket: QueryTicket, result: Result<T, String>, now: u64) -> bool {
        if !self.loading || ticket.session != self.session || ticket.revision != self.revision {
            return false;
        }
        self.loading = false;
        match result {
            Ok(value) => {
                self.value = Some(value);
                self.error = None;
                self.updated_at = Some(now);
            }
            Err(error) => self.error = Some(error),
        }
        true
    }
    pub fn invalidate(&mut self, session: Generation) {
        self.revision += 1;
        self.session = session;
        self.loading = false;
        self.updated_at = None;
    }
    pub fn patch(&mut self, update: impl FnOnce(&mut T)) {
        self.revision += 1;
        self.loading = false;
        if let Some(value) = &mut self.value {
            update(value);
        }
    }
    pub fn is_stale(&self, kind: QueryKind, now: u64) -> bool {
        self.updated_at
            .is_none_or(|updated| now.saturating_sub(updated) >= kind.stale_time_ms())
    }
}

pub const QUERY_MAX_ROWS: usize = 256;
pub const QUERY_MAX_BYTES: usize = 32 * 1024 * 1024;
pub const QUERY_MAX_PAYLOAD_BYTES: usize = 4 * 1024 * 1024;
pub const QUERY_MAX_AGE_MS: u64 = 30 * 24 * 60 * 60_000;
pub const QUERY_PRUNE_BATCH_ROWS: usize = 32;
pub const QUERY_PRUNE_BATCH_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredQuery {
    pub environment_id: EnvironmentId,
    pub payload: String,
    pub updated_at: u64,
}
#[derive(Default)]
pub struct PullRequestQueryCache {
    rows: BTreeMap<(EnvironmentId, String), StoredQuery>,
    batches: BTreeMap<EnvironmentId, (usize, usize)>,
}
impl PullRequestQueryCache {
    pub fn load<T: DeserializeOwned>(
        &self,
        environment: &EnvironmentId,
        key: &str,
        now: u64,
    ) -> Option<T> {
        let row = self.rows.get(&(environment.clone(), key.into()))?;
        if row.environment_id != *environment
            || row.updated_at < now.saturating_sub(QUERY_MAX_AGE_MS)
        {
            return None;
        }
        serde_json::from_str(&row.payload).ok()
    }
    pub fn save(&mut self, environment: EnvironmentId, key: String, payload: String, now: u64) {
        let bytes = payload.len();
        let row_key = (environment.clone(), key);
        if bytes > QUERY_MAX_PAYLOAD_BYTES {
            self.rows.remove(&row_key);
            return;
        }
        self.rows.insert(
            row_key,
            StoredQuery {
                environment_id: environment.clone(),
                payload,
                updated_at: now,
            },
        );
        let had_batch = self.batches.contains_key(&environment);
        let batch = self.batches.entry(environment.clone()).or_default();
        batch.0 += 1;
        batch.1 += bytes;
        if had_batch && batch.0 < QUERY_PRUNE_BATCH_ROWS && batch.1 < QUERY_PRUNE_BATCH_BYTES {
            return;
        }
        self.prune(environment, now);
    }
    fn prune(&mut self, environment: EnvironmentId, now: u64) {
        let mut ordered: Vec<_> = self
            .rows
            .iter()
            .filter(|((env, _), _)| env == &environment)
            .map(|(key, row)| (key.clone(), row.updated_at, row.payload.len()))
            .collect();
        ordered.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| b.0.cmp(&a.0)));
        let mut total = 0;
        for (index, (key, updated, bytes)) in ordered.into_iter().enumerate() {
            total += bytes;
            if index >= QUERY_MAX_ROWS - QUERY_PRUNE_BATCH_ROWS
                || total > QUERY_MAX_BYTES - QUERY_PRUNE_BATCH_BYTES
                || updated < now.saturating_sub(QUERY_MAX_AGE_MS)
            {
                self.rows.remove(&key);
            }
        }
        self.batches.insert(environment, (0, 0));
    }
    pub fn row_count(&self, environment: &EnvironmentId) -> usize {
        self.rows
            .keys()
            .filter(|(env, _)| env == environment)
            .count()
    }
    pub fn payload_bytes(&self, environment: &EnvironmentId) -> usize {
        self.rows
            .iter()
            .filter(|((env, _), _)| env == environment)
            .map(|(_, row)| row.payload.len())
            .sum()
    }
    pub fn remove_environment(&mut self, environment: &EnvironmentId) {
        self.rows.retain(|(env, _), _| env != environment);
        self.batches.remove(environment);
    }
    pub async fn restore_environment(
        &mut self,
        storage: &impl Storage,
        environment: &EnvironmentId,
        now: u64,
    ) -> Result<(), ClientError> {
        let keys = storage
            .list(StorageArea::Ordinary, "pull-request-query".into())
            .await?;
        for key in keys
            .into_iter()
            .filter(|k| k.environment.as_ref() == Some(environment))
        {
            let Some(bytes) = storage.get(StorageArea::Ordinary, key.clone()).await? else {
                continue;
            };
            let row = serde_json::from_slice::<StoredQuery>(&bytes)
                .ok()
                .filter(|row| {
                    row.environment_id == *environment
                        && row.payload.len() <= QUERY_MAX_PAYLOAD_BYTES
                        && row.updated_at >= now.saturating_sub(QUERY_MAX_AGE_MS)
                });
            let Some(row) = row else {
                continue;
            };
            let row_key = (environment.clone(), key.record);
            if self
                .rows
                .get(&row_key)
                .is_none_or(|held| row.updated_at > held.updated_at)
            {
                self.rows.insert(row_key, row);
            }
            if self.row_count(environment) > QUERY_MAX_ROWS
                || self.payload_bytes(environment) > QUERY_MAX_BYTES
            {
                self.prune(environment.clone(), now);
            }
        }
        self.prune(environment.clone(), now);
        self.persist_environment(storage, environment).await
    }
    pub async fn persist_environment(
        &self,
        storage: &impl Storage,
        environment: &EnvironmentId,
    ) -> Result<(), ClientError> {
        let existing = storage
            .list(StorageArea::Ordinary, "pull-request-query".into())
            .await?;
        let mut writes: Vec<_> = existing
            .into_iter()
            .filter(|k| {
                k.environment.as_ref() == Some(environment)
                    && !self
                        .rows
                        .contains_key(&(environment.clone(), k.record.clone()))
            })
            .map(|key| StorageWrite { key, value: None })
            .collect();
        for ((env, key), row) in &self.rows {
            if env == environment {
                writes.push(StorageWrite {
                    key: StorageKey {
                        namespace: "pull-request-query".into(),
                        environment: Some(env.clone()),
                        record: key.clone(),
                    },
                    value: Some(serde_json::to_vec(row)?),
                });
            }
        }
        storage.write(StorageArea::Ordinary, writes).await?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutingPermission {
    Off,
    Read,
    ReadWrite,
}
#[derive(Debug, Clone)]
pub struct RoutingEnvironment {
    pub environment: EnvironmentId,
    pub enabled: bool,
    pub supported: bool,
    pub connected: bool,
    pub local: bool,
    pub checks_supported: bool,
    pub connection_key: Option<String>,
    pub granted_connection_key: Option<String>,
    pub permission: RoutingPermission,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteOperation {
    Read(QueryKind),
    Write,
    SetFilesViewed,
    ReviewGuide,
}
impl RouteOperation {
    fn is_write(self) -> bool {
        matches!(self, Self::Write | Self::SetFilesViewed)
    }
    fn routable(self) -> bool {
        !matches!(
            self,
            Self::ReviewGuide
                | Self::Read(
                    QueryKind::List
                        | QueryKind::ListStats
                        | QueryKind::ReviewGuide
                        | QueryKind::FilterOptions
                        | QueryKind::LinkedThreads
                        | QueryKind::Diff
                )
        )
    }
}
pub fn routing_allowed(
    origin: &RoutingEnvironment,
    target: &RoutingEnvironment,
    operation: RouteOperation,
) -> bool {
    [origin, target].iter().all(|e| {
        e.enabled
            && e.supported
            && e.connection_key.is_some()
            && e.connection_key == e.granted_connection_key
            && if operation.is_write() {
                e.permission == RoutingPermission::ReadWrite
            } else {
                e.permission != RoutingPermission::Off
            }
    })
}
pub fn route_candidates(
    origin: &RoutingEnvironment,
    entries: &[RoutingEnvironment],
    operation: RouteOperation,
) -> Vec<EnvironmentId> {
    if !operation.routable() {
        return vec![origin.environment.clone()];
    }
    let alternatives: Vec<_> = entries
        .iter()
        .filter(|e| {
            e.environment != origin.environment
                && e.connected
                && routing_allowed(origin, e, operation)
                && (operation != RouteOperation::Read(QueryKind::Checks) || e.checks_supported)
        })
        .collect();
    if alternatives.is_empty() {
        return vec![origin.environment.clone()];
    }
    let mut candidates = Vec::new();
    if origin.local && operation.is_write() {
        candidates.push(origin.environment.clone());
    }
    candidates.extend(
        alternatives
            .iter()
            .filter(|e| e.local)
            .map(|e| e.environment.clone()),
    );
    if !origin.local && operation.is_write() {
        candidates.push(origin.environment.clone());
    }
    candidates.extend(
        alternatives
            .iter()
            .filter(|e| !e.local)
            .map(|e| e.environment.clone()),
    );
    if !operation.is_write() {
        candidates.push(origin.environment.clone());
    }
    candidates
}
pub fn routing_identity_matches(
    origin: &PullRequestRoutingResult,
    alternate: &PullRequestRoutingIdentityResult,
) -> bool {
    origin.provider == alternate.provider
        && origin.host.0.eq_ignore_ascii_case(&alternate.host.0)
        && origin.account_id == alternate.account_id
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteFailure {
    UnavailableBeforeDispatch,
    UnregisteredBeforeDispatch,
    IdentityRejectedBeforeDispatch,
    AfterDispatch,
    Interrupted,
}
pub fn can_retry_route(operation: RouteOperation, failure: RouteFailure) -> bool {
    failure != RouteFailure::Interrupted
        && (!operation.is_write()
            || matches!(
                failure,
                RouteFailure::UnavailableBeforeDispatch
                    | RouteFailure::UnregisteredBeforeDispatch
                    | RouteFailure::IdentityRejectedBeforeDispatch
            ))
}
#[derive(Default)]
pub struct RoutedReads {
    pub(crate) entries: VecDeque<(PullRequestKey, BTreeSet<EnvironmentId>)>,
    references: BTreeMap<PullRequestKey, PullRequestRef>,
}

pub fn stat_persistence_key(reference: &PullRequestRef) -> Result<String, ClientError> {
    Ok(serde_json::to_string(&(
        "pull-request-stat",
        &reference.project_id,
        reference
            .host
            .as_ref()
            .and_then(Option::as_ref)
            .map_or("", |h| h.0.as_str()),
        &reference.repository,
        reference.number,
    ))?)
}
impl PullRequestQueryCache {
    pub fn load_stats(
        &self,
        environment: &EnvironmentId,
        input: &t3_contracts::pull_request::PullRequestListStatsInput,
        now: u64,
    ) -> Option<t3_contracts::pull_request::PullRequestListStatsResult> {
        let stats: Vec<_> = input
            .refs
            .iter()
            .filter_map(|reference| {
                self.load(environment, &stat_persistence_key(reference).ok()?, now)
            })
            .collect();
        (!stats.is_empty())
            .then_some(t3_contracts::pull_request::PullRequestListStatsResult { stats })
    }
    pub fn save_stats(
        &mut self,
        environment: EnvironmentId,
        input: &t3_contracts::pull_request::PullRequestListStatsInput,
        result: &t3_contracts::pull_request::PullRequestListStatsResult,
        now: u64,
    ) -> Result<(), ClientError> {
        for stat in &result.stats {
            if let Some(reference) = input.refs.iter().find(|reference| {
                reference.project_id == stat.project_id
                    && reference.repository == stat.repository
                    && reference.number == stat.number
            }) {
                self.save(
                    environment.clone(),
                    stat_persistence_key(reference)?,
                    serde_json::to_string(stat)?,
                    now,
                );
            }
        }
        Ok(())
    }
}
impl RoutedReads {
    pub fn record_reference(
        &mut self,
        origin: EnvironmentId,
        reference: PullRequestRef,
        destination: EnvironmentId,
    ) {
        let key = PullRequestKey::new(origin, &reference);
        self.record(key.clone(), destination);
        self.references.insert(key, reference);
    }
    pub(crate) fn reference(&self, key: &PullRequestKey) -> PullRequestRef {
        self.references
            .get(key)
            .cloned()
            .unwrap_or_else(|| PullRequestRef {
                project_id: key.project.project.clone(),
                host: key
                    .host
                    .clone()
                    .map(|value| Some(t3_contracts::base_schemas::TrimmedNonEmptyString(value))),
                repository: t3_contracts::base_schemas::TrimmedNonEmptyString(
                    key.repository.clone(),
                ),
                number: key.number,
                expected_account_id: key
                    .expected_account_id
                    .clone()
                    .map(|value| Some(t3_contracts::base_schemas::TrimmedNonEmptyString(value))),
                allow_stale: None,
            })
    }
    pub fn record(&mut self, reference: PullRequestKey, destination: EnvironmentId) {
        if let Some((_, targets)) = self.entries.iter_mut().find(|(key, _)| key == &reference) {
            targets.insert(destination);
            return;
        }
        if self.entries.len() >= 256
            && let Some((key, _)) = self.entries.pop_front()
        {
            self.references.remove(&key);
        }
        self.entries
            .push_back((reference, BTreeSet::from([destination])));
    }
    pub fn targets(
        &self,
        origin: &EnvironmentId,
        filter: Option<&PullRequestKey>,
    ) -> BTreeSet<EnvironmentId> {
        self.entries
            .iter()
            .filter(|(key, _)| {
                &key.project.environment == origin
                    && filter.is_none_or(|f| key.matches_invalidation(f))
            })
            .flat_map(|(_, targets)| targets.iter().cloned())
            .collect()
    }
    pub fn remove_environment(&mut self, environment: &EnvironmentId) {
        self.references
            .retain(|key, _| &key.project.environment != environment);
        self.entries
            .retain(|(key, _)| &key.project.environment != environment);
        for (_, targets) in &mut self.entries {
            targets.remove(environment);
        }
    }
}
