//! Local UI preferences. Decode legacy bytes without deleting or rewriting their source.
//! Storage/debounce/unload listeners are host-owned; `encode` preserves the v1 JSON shape.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
pub const STORAGE_KEY: &str = "t3code:ui-state:v1";
pub const LEGACY_STORAGE_KEYS: &[&str] = &[
    "t3code:renderer-state:v8",
    "t3code:renderer-state:v7",
    "t3code:renderer-state:v6",
    "t3code:renderer-state:v5",
    "t3code:renderer-state:v4",
    "t3code:renderer-state:v3",
    "codething:renderer-state:v4",
    "codething:renderer-state:v3",
    "codething:renderer-state:v2",
    "codething:renderer-state:v1",
];
pub fn legacy_cwd_key(cwd: &str) -> String {
    format!(
        "legacy-project-cwd:{}",
        crate::workspace::projects::normalize_project_path_for_comparison(cwd)
    )
}
pub(super) fn strings(value: &Value) -> Vec<String> {
    let mut seen = BTreeSet::new();
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|s| !s.is_empty() && seen.insert(*s))
        .map(str::to_owned)
        .collect()
}
fn booleans(value: &Value) -> BTreeMap<String, bool> {
    value
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(key, _)| !key.is_empty())
        .filter_map(|(key, value)| value.as_bool().map(|v| (key.clone(), v)))
        .collect()
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiState {
    pub project_expanded_by_id: BTreeMap<String, bool>,
    pub project_order: Vec<String>,
    pub sidebar_project_scope_keys: Vec<String>,
    pub thread_last_visited_at_by_id: BTreeMap<String, String>,
    pub thread_changed_files_expanded_by_id: BTreeMap<String, BTreeMap<String, bool>>,
    pub default_advertised_endpoint_key: Option<String>,
    pub pull_request_merge_method: t3_contracts::pull_request::PullRequestMergeMethod,
    #[serde(flatten)]
    pub unknown: BTreeMap<String, Value>,
}
impl Default for UiState {
    fn default() -> Self {
        Self {
            project_expanded_by_id: BTreeMap::new(),
            project_order: vec![],
            sidebar_project_scope_keys: vec![],
            thread_last_visited_at_by_id: BTreeMap::new(),
            thread_changed_files_expanded_by_id: BTreeMap::new(),
            default_advertised_endpoint_key: None,
            pull_request_merge_method: t3_contracts::pull_request::PullRequestMergeMethod::Merge,
            unknown: BTreeMap::new(),
        }
    }
}
impl UiState {
    pub fn decode(
        raw: &str,
        parse: &impl Fn(&str) -> Option<i64>,
    ) -> Result<Self, serde_json::Error> {
        Ok(Self::from_value(serde_json::from_str(raw)?, parse))
    }
    pub fn from_value(value: Value, parse: &impl Fn(&str) -> Option<i64>) -> Self {
        let mut state = Self::default();
        let Some(object) = value.as_object() else {
            return state;
        };
        state.unknown = object.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        if !object.contains_key("projectExpandedById") {
            for cwd in strings(&value["collapsedProjectCwds"]) {
                state
                    .project_expanded_by_id
                    .insert(legacy_cwd_key(&cwd), false);
            }
            let expanded = strings(&value["expandedProjectCwds"]);
            for cwd in &expanded {
                state
                    .project_expanded_by_id
                    .insert(legacy_cwd_key(cwd), true);
            }
            if !value["collapsedProjectCwds"].is_array() && !expanded.is_empty() {
                state
                    .project_expanded_by_id
                    .insert("legacy-project-expansion-default".into(), false);
            }
        } else {
            state.project_expanded_by_id = booleans(&value["projectExpandedById"]);
        }
        state.project_order = if object.contains_key("projectOrder") {
            strings(&value["projectOrder"])
        } else {
            strings(&value["projectOrderCwds"])
                .iter()
                .map(|s| legacy_cwd_key(s))
                .collect()
        };
        state.sidebar_project_scope_keys = if value["sidebarProjectScopeKeys"].is_null() {
            strings(&json!([value["sidebarProjectScopeKey"]]))
        } else {
            strings(&value["sidebarProjectScopeKeys"])
        };
        state.thread_last_visited_at_by_id = value["threadLastVisitedAtById"]
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(key, _)| !key.is_empty())
            .filter_map(|(key, v)| {
                v.as_str()
                    .filter(|s| !s.is_empty() && parse(s).is_some())
                    .map(|s| (key.clone(), s.into()))
            })
            .collect();
        if value["threadChangedFilesExpansionVersion"] == 2 {
            state.thread_changed_files_expanded_by_id = value["threadChangedFilesExpandedById"]
                .as_object()
                .into_iter()
                .flatten()
                .filter(|(key, _)| !key.is_empty())
                .map(|(key, v)| (key.clone(), booleans(v)))
                .filter(|(_, v)| !v.is_empty())
                .collect();
        }
        state.default_advertised_endpoint_key = value["defaultAdvertisedEndpointKey"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        state.pull_request_merge_method =
            serde_json::from_value(value["pullRequestMergeMethod"].clone())
                .unwrap_or(t3_contracts::pull_request::PullRequestMergeMethod::Merge);
        for key in [
            "projectExpandedById",
            "projectOrder",
            "sidebarProjectScopeKeys",
            "sidebarProjectScopeKey",
            "threadLastVisitedAtById",
            "threadChangedFilesExpandedById",
            "threadChangedFilesExpansionVersion",
            "defaultAdvertisedEndpointKey",
            "pullRequestMergeMethod",
        ] {
            state.unknown.remove(key);
        }
        state
    }
    pub fn encode(&self) -> Result<String, serde_json::Error> {
        let mut value = serde_json::to_value(self)?;
        value["threadChangedFilesExpansionVersion"] = json!(2);
        value["sidebarProjectScopeKey"] = if self.sidebar_project_scope_keys.len() == 1 {
            json!(self.sidebar_project_scope_keys[0])
        } else {
            Value::Null
        };
        serde_json::to_string(&value)
    }
    pub fn mark_visited(
        &mut self,
        key: &str,
        at: &str,
        parse: &impl Fn(&str) -> Option<i64>,
    ) -> bool {
        let Some(stamp) = parse(at) else {
            return false;
        };
        if self
            .thread_last_visited_at_by_id
            .get(key)
            .and_then(|s| parse(s))
            .is_some_and(|old| old >= stamp)
        {
            return false;
        }
        self.thread_last_visited_at_by_id
            .insert(key.into(), at.into());
        true
    }
    pub fn mark_unread(
        &mut self,
        key: &str,
        completed: Option<&str>,
        parse: &impl Fn(&str) -> Option<i64>,
        iso: impl Fn(i64) -> Option<String>,
    ) -> bool {
        let Some(at) = completed
            .and_then(parse)
            .and_then(|v| v.checked_sub(1))
            .and_then(iso)
        else {
            return false;
        };
        if self.thread_last_visited_at_by_id.get(key) == Some(&at) {
            return false;
        }
        self.thread_last_visited_at_by_id.insert(key.into(), at);
        true
    }
    pub fn set_project_expanded(&mut self, keys: &[String], expanded: bool) {
        for key in keys {
            self.project_expanded_by_id.insert(key.clone(), expanded);
        }
    }
    pub fn set_changed_files_expanded(&mut self, thread: &str, turn: &str, expanded: bool) {
        self.thread_changed_files_expanded_by_id
            .entry(thread.into())
            .or_default()
            .insert(turn.into(), expanded);
    }
    pub fn set_scope(&mut self, keys: &[String]) {
        self.sidebar_project_scope_keys = strings(&json!(keys));
    }
    pub fn set_default_endpoint(&mut self, key: Option<&str>) {
        self.default_advertised_endpoint_key = key.filter(|k| !k.is_empty()).map(str::to_owned);
    }
    pub fn reorder_projects(&mut self, current: &[String], dragged: &[String], target: &[String]) {
        if dragged.is_empty() || dragged.iter().all(|k| target.contains(k)) {
            return;
        }
        let Some(target_index) = current.iter().position(|k| target.contains(k)) else {
            return;
        };
        let mut before = 0;
        let mut removed = vec![];
        let mut kept = vec![];
        for (i, key) in current.iter().enumerate() {
            if dragged.contains(key) {
                before += usize::from(i < target_index);
                removed.push(key.clone());
            } else {
                kept.push(key.clone());
            }
        }
        if removed.is_empty() {
            return;
        }
        let index = (target_index - before.saturating_sub(1)).min(kept.len());
        kept.splice(index..index, removed);
        self.project_order = kept;
    }
}
pub use super::sidebar::resolve_project_expanded;

/// The sidebar shelves' expanded state and the sidebar width, each its own record as in the web
/// app (`Sidebar.tsx`, `threadSidebarWidth.ts`).
pub const SETTLED_SHELF_EXPANDED_KEY: &str = "t3code:sidebar:settled-expanded";
pub const SNOOZED_SHELF_EXPANDED_KEY: &str = "t3code:sidebar:snoozed-expanded";
pub const WORKING_SHELF_EXPANDED_KEY: &str = "t3code:sidebar:working-expanded";
pub const SIDEBAR_WIDTH_KEY: &str = "chat_thread_sidebar_width";
/// Records live in ordinary storage under this namespace, keyed by their web storage key.
pub const PREFERENCES_NAMESPACE: &str = "ui-preferences";

#[derive(Debug, Clone, Default, PartialEq)]
pub struct UiPreferences {
    pub state: UiState,
    pub settled_shelf_expanded: bool,
    pub snoozed_shelf_expanded: bool,
    pub working_shelf_expanded: bool,
    pub sidebar_width: Option<f64>,
}

fn preference_key(record: &str) -> crate::platform::StorageKey {
    crate::platform::StorageKey {
        namespace: PREFERENCES_NAMESPACE.into(),
        environment: None,
        record: record.into(),
    }
}

impl UiPreferences {
    /// Reads the v1 record, else the newest legacy record, like `readPersistedState`. Unreadable
    /// records read as defaults and stay in storage untouched.
    pub async fn load(
        storage: &(impl crate::platform::Storage + ?Sized),
    ) -> Result<Self, crate::platform::PlatformError> {
        use crate::platform::StorageArea::Ordinary;
        let parse = |value: &str| crate::time::epoch_ms(value);
        let mut state = None;
        for record in std::iter::once(STORAGE_KEY).chain(LEGACY_STORAGE_KEYS.iter().copied()) {
            if let Some(bytes) = storage.get(Ordinary, preference_key(record)).await? {
                state = Some(
                    std::str::from_utf8(&bytes)
                        .ok()
                        .and_then(|raw| UiState::decode(raw, &parse).ok())
                        .unwrap_or_default(),
                );
                break;
            }
        }
        let flag = |bytes: Option<Vec<u8>>| {
            bytes
                .and_then(|bytes| serde_json::from_slice::<bool>(&bytes).ok())
                .unwrap_or(false)
        };
        Ok(Self {
            state: state.unwrap_or_default(),
            settled_shelf_expanded: flag(
                storage
                    .get(Ordinary, preference_key(SETTLED_SHELF_EXPANDED_KEY))
                    .await?,
            ),
            snoozed_shelf_expanded: flag(
                storage
                    .get(Ordinary, preference_key(SNOOZED_SHELF_EXPANDED_KEY))
                    .await?,
            ),
            working_shelf_expanded: flag(
                storage
                    .get(Ordinary, preference_key(WORKING_SHELF_EXPANDED_KEY))
                    .await?,
            ),
            sidebar_width: storage
                .get(Ordinary, preference_key(SIDEBAR_WIDTH_KEY))
                .await?
                .and_then(|bytes| serde_json::from_slice::<f64>(&bytes).ok())
                .filter(|width| width.is_finite()),
        })
    }

    /// Atomically updates the `t3code:ui-state:v1` record via CAS without clobbering concurrent
    /// writers. Corrupt v1 records recover to defaults just like `readPersistedState` in TSX.
    pub async fn update_state(
        storage: &(impl crate::platform::Storage + ?Sized),
        mut update: impl FnMut(&mut UiState),
    ) -> Result<UiState, crate::platform::PlatformError> {
        use crate::platform::{
            PlatformError, StorageArea, StorageCondition, StorageTransaction, StorageWrite,
        };
        let key = preference_key(STORAGE_KEY);
        let invalid = |error: &dyn std::fmt::Display| PlatformError::InvalidData(error.to_string());
        for _ in 0..8 {
            let expected = storage.get(StorageArea::Ordinary, key.clone()).await?;
            let mut state = if let Some(raw) = expected.as_deref() {
                std::str::from_utf8(raw)
                    .ok()
                    .and_then(|s| UiState::decode(s, &crate::time::epoch_ms).ok())
                    .unwrap_or_default()
            } else {
                Self::load(storage).await?.state
            };
            update(&mut state);
            let value = state
                .encode()
                .map_err(|error| invalid(&error))?
                .into_bytes();
            if storage
                .transaction(StorageTransaction {
                    conditions: vec![StorageCondition {
                        area: StorageArea::Ordinary,
                        key: key.clone(),
                        expected,
                    }],
                    ordinary: vec![StorageWrite {
                        key: key.clone(),
                        value: Some(value),
                    }],
                    secure: vec![],
                })
                .await?
            {
                return Ok(state);
            }
        }
        Err(PlatformError::Storage(
            "UI preferences changed repeatedly while saving.".into(),
        ))
    }

    /// Reads the persisted pull-request merge method from `t3code:ui-state:v1` (or legacy fallback).
    pub async fn read_pull_request_merge_method(
        storage: &(impl crate::platform::Storage + ?Sized),
    ) -> Result<t3_contracts::pull_request::PullRequestMergeMethod, crate::platform::PlatformError>
    {
        Ok(Self::load(storage).await?.state.pull_request_merge_method)
    }

    /// Atomically updates `pullRequestMergeMethod` in `t3code:ui-state:v1` without clobbering
    /// concurrent sidebar preference writes.
    pub async fn save_pull_request_merge_method(
        storage: &(impl crate::platform::Storage + ?Sized),
        method: t3_contracts::pull_request::PullRequestMergeMethod,
    ) -> Result<(), crate::platform::PlatformError> {
        Self::update_state(storage, |state| {
            state.pull_request_merge_method = method.clone();
        })
        .await
        .map(|_| ())
    }

    /// Writes every record in one atomic write. Legacy records are left for older clients.
    pub async fn save(
        &self,
        storage: &(impl crate::platform::Storage + ?Sized),
    ) -> Result<(), crate::platform::PlatformError> {
        let invalid = |error: serde_json::Error| {
            crate::platform::PlatformError::InvalidData(error.to_string())
        };
        let write = |record: &str, value: Option<Vec<u8>>| crate::platform::StorageWrite {
            key: preference_key(record),
            value,
        };
        storage
            .write(
                crate::platform::StorageArea::Ordinary,
                vec![
                    write(
                        STORAGE_KEY,
                        Some(self.state.encode().map_err(invalid)?.into_bytes()),
                    ),
                    write(
                        SETTLED_SHELF_EXPANDED_KEY,
                        Some(serde_json::to_vec(&self.settled_shelf_expanded).map_err(invalid)?),
                    ),
                    write(
                        SNOOZED_SHELF_EXPANDED_KEY,
                        Some(serde_json::to_vec(&self.snoozed_shelf_expanded).map_err(invalid)?),
                    ),
                    write(
                        WORKING_SHELF_EXPANDED_KEY,
                        Some(serde_json::to_vec(&self.working_shelf_expanded).map_err(invalid)?),
                    ),
                    write(
                        SIDEBAR_WIDTH_KEY,
                        self.sidebar_width
                            .filter(|width| width.is_finite())
                            .map(|width| serde_json::to_vec(&width))
                            .transpose()
                            .map_err(invalid)?,
                    ),
                ],
            )
            .await
    }
}
