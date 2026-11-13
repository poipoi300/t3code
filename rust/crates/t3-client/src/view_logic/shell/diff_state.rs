//! Persisted diff selection plus session-only fold/whitespace preferences.
use crate::{types::ThreadKey, view_logic::JsTrim};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use t3_contracts::base_schemas::{RunId, ThreadId, TurnItemId};
pub const STORAGE_KEY: &str = "t3code:diff-panel-state:v1";
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum Selection {
    ToolCall {
        thread_id: ThreadId,
        item_id: TurnItemId,
        revision: String,
    },
    Branch {
        base_ref: Option<String>,
    },
    Unstaged,
    Commit {
        sha: String,
        base_ref: Option<String>,
    },
    Turn {
        turn_id: RunId,
        file_path: Option<String>,
        reveal_request_id: u64,
    },
}
impl Default for Selection {
    fn default() -> Self {
        Self::Branch { base_ref: None }
    }
}
#[derive(Debug, Clone, Default)]
pub struct DiffStore {
    pub by_thread: BTreeMap<ThreadKey, Selection>,
    pub branch_base: BTreeMap<ThreadKey, Option<String>>,
    pub collapsed: BTreeMap<String, BTreeSet<String>>,
    pub ignore_whitespace: Option<bool>,
}
impl DiffStore {
    fn base(&self, key: &ThreadKey) -> Option<String> {
        match self.by_thread.get(key) {
            Some(Selection::Branch { base_ref } | Selection::Commit { base_ref, .. }) => {
                base_ref.clone()
            }
            _ => self.branch_base.get(key).cloned().flatten(),
        }
    }
    pub fn select_git(&mut self, key: ThreadKey, unstaged: bool) {
        let base = self.base(&key);
        if matches!(
            self.by_thread.get(&key),
            Some(Selection::Branch { .. } | Selection::Commit { .. })
        ) {
            self.branch_base.insert(key.clone(), base.clone());
        }
        self.by_thread.insert(
            key,
            if unstaged {
                Selection::Unstaged
            } else {
                Selection::Branch { base_ref: base }
            },
        );
    }
    pub fn select_base(&mut self, key: ThreadKey, base: Option<&str>) {
        let base = base
            .map(JsTrim::js_trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        self.branch_base.insert(key.clone(), base.clone());
        self.by_thread
            .insert(key, Selection::Branch { base_ref: base });
    }
    pub fn select_turn(&mut self, key: ThreadKey, turn_id: RunId, file_path: Option<&str>) {
        let count = match self.by_thread.get(&key) {
            Some(Selection::Turn {
                reveal_request_id, ..
            }) => reveal_request_id + 1,
            _ => 1,
        };
        self.by_thread.insert(
            key,
            Selection::Turn {
                turn_id,
                file_path: file_path
                    .map(JsTrim::js_trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned),
                reveal_request_id: count,
            },
        );
    }
    pub fn select_commit(&mut self, key: ThreadKey, sha: String) {
        let base = self.base(&key);
        self.branch_base.insert(key.clone(), base.clone());
        self.by_thread.insert(
            key,
            Selection::Commit {
                sha,
                base_ref: base,
            },
        );
    }
    pub fn reconcile_turn(&mut self, key: &ThreadKey, available: &[RunId]) {
        if let Some(Selection::Turn { turn_id, .. }) = self.by_thread.get_mut(key)
            && !available.contains(turn_id)
            && let Some(latest) = available.first()
        {
            *turn_id = latest.clone();
        }
    }
    pub fn remove(&mut self, key: &ThreadKey) {
        self.by_thread.remove(key);
        self.branch_base.remove(key);
    }
    pub fn update_collapsed(
        &mut self,
        key: String,
        defaults: &BTreeSet<String>,
        update: impl FnOnce(&BTreeSet<String>) -> BTreeSet<String>,
    ) {
        self.collapsed.insert(
            key.clone(),
            update(self.collapsed.get(&key).unwrap_or(defaults)),
        );
    }
    pub fn encode(&self) -> Result<String, serde_json::Error> {
        use super::navigation::legacy_thread_key;
        let selections = self
            .by_thread
            .iter()
            .map(|(k, v)| (legacy_thread_key(k), v))
            .collect::<BTreeMap<_, _>>();
        let bases = self
            .branch_base
            .iter()
            .map(|(k, v)| (legacy_thread_key(k), v))
            .collect::<BTreeMap<_, _>>();
        serde_json::to_string(
            &serde_json::json!({"version":2,"state":{"byThreadKey":selections,"branchBaseRefByThreadKey":bases}}),
        )
    }
    pub fn decode(raw: &str) -> Result<Self, serde_json::Error> {
        let v: serde_json::Value = serde_json::from_str(raw)?;
        let state = v.get("state").unwrap_or(&v);
        let mut store = Self::default();
        for (k, v) in state["byThreadKey"].as_object().into_iter().flatten() {
            if let Some(key) = super::navigation::parse_legacy_thread_key(k)
                && let Ok(value) = serde_json::from_value(v.clone())
            {
                store.by_thread.insert(key, value);
            }
        }
        for (k, v) in state["branchBaseRefByThreadKey"]
            .as_object()
            .into_iter()
            .flatten()
        {
            if let Some(key) = super::navigation::parse_legacy_thread_key(k)
                && let Ok(value) = serde_json::from_value(v.clone())
            {
                store.branch_base.insert(key, value);
            }
        }
        Ok(store)
    }
}
pub fn refresh_tool_call_target(
    target: Selection,
    items: &[t3_contracts::orchestration_v2::OrchestrationV2ProjectedTurnItem],
) -> Selection {
    let Selection::ToolCall {
        thread_id,
        item_id,
        revision,
    } = target
    else {
        return target;
    };
    let next=items.iter().find(|r|matches!(&r.source_thread_id,t3_contracts::orchestration_v2_id_table::IdOrIndex::Id(id)if id==&thread_id)&&matches!(&r.source_item_id,t3_contracts::orchestration_v2_id_table::IdOrIndex::Id(id)if id==&item_id)).filter(|r|matches!(r.item,t3_contracts::orchestration_v2::OrchestrationV2TurnItem::FileChange(_))).map(|r|crate::work_log::turn_item_detail_revision(&r.item)).unwrap_or(revision);
    Selection::ToolCall {
        thread_id,
        item_id,
        revision: next,
    }
}
