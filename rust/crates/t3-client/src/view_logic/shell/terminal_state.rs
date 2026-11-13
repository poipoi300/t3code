//! Client-local terminal arrangement, suppression and v4 legacy persistence.
//! No terminal I/O lives here. Server session reconciliation cannot reopen a locally closed tab.
use crate::{types::ThreadKey, view_logic::JsTrim};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
pub const STORAGE_KEY: &str = "t3code:terminal-state:v1";
pub const DEFAULT_HEIGHT: f64 = 280.0;
pub const DEFAULT_TERMINAL: &str = "term-1";
pub const MAX_PER_GROUP: usize = 4;
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Group {
    pub id: String,
    pub terminal_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split_direction: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TerminalState {
    pub terminal_open: bool,
    pub terminal_height: f64,
    pub terminal_ids: Vec<String>,
    pub active_terminal_id: String,
    pub terminal_groups: Vec<Group>,
    pub active_terminal_group_id: String,
}
impl Default for TerminalState {
    fn default() -> Self {
        Self {
            terminal_open: false,
            terminal_height: DEFAULT_HEIGHT,
            terminal_ids: vec![],
            active_terminal_id: String::new(),
            terminal_groups: vec![],
            active_terminal_group_id: String::new(),
        }
    }
}
fn normalize_ids(ids: &[String]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    ids.iter()
        .map(|s| s.js_trim())
        .filter(|s| !s.is_empty() && seen.insert(*s))
        .map(str::to_owned)
        .collect()
}
fn unique(base: String, used: &mut BTreeSet<String>) -> String {
    let mut name = base.clone();
    let mut i = 2;
    while !used.insert(name.clone()) {
        name = format!("{base}-{i}");
        i += 1;
    }
    name
}
impl TerminalState {
    pub fn normalize(&mut self) {
        self.terminal_ids = normalize_ids(&self.terminal_ids);
        if !self.terminal_ids.contains(&self.active_terminal_id) {
            self.active_terminal_id = self.terminal_ids.first().cloned().unwrap_or_default();
        }
        if !self.terminal_height.is_finite() || self.terminal_height <= 0.0 {
            self.terminal_height = DEFAULT_HEIGHT;
        }
        let mut assigned = BTreeSet::new();
        let mut groups = vec![];
        let mut used = BTreeSet::new();
        for group in &self.terminal_groups {
            let ids = normalize_ids(&group.terminal_ids)
                .into_iter()
                .filter(|id| self.terminal_ids.contains(id) && !assigned.contains(id))
                .collect::<Vec<_>>();
            if ids.is_empty() {
                continue;
            }
            assigned.extend(ids.iter().cloned());
            let base = if group.id.js_trim().is_empty() {
                format!("group-{}", ids[0])
            } else {
                group.id.js_trim().into()
            };
            groups.push(Group {
                id: unique(base, &mut used),
                terminal_ids: ids,
                split_direction: (group.split_direction.as_deref() == Some("vertical"))
                    .then(|| "vertical".into()),
            });
        }
        for id in &self.terminal_ids {
            if assigned.insert(id.clone()) {
                groups.push(Group {
                    id: unique(format!("group-{id}"), &mut used),
                    terminal_ids: vec![id.clone()],
                    split_direction: None,
                });
            }
        }
        self.terminal_groups = groups;
        if !self
            .terminal_groups
            .iter()
            .any(|g| g.id == self.active_terminal_group_id)
        {
            self.active_terminal_group_id = self
                .terminal_groups
                .iter()
                .find(|g| g.terminal_ids.contains(&self.active_terminal_id))
                .or_else(|| self.terminal_groups.first())
                .map(|g| g.id.clone())
                .unwrap_or_default();
        }
    }
    pub fn set_open(&mut self, open: bool) {
        self.normalize();
        if open && self.terminal_ids.is_empty() {
            self.upsert(DEFAULT_TERMINAL, false, false);
        } else {
            self.terminal_open = open;
        }
    }
    pub fn set_height(&mut self, height: f64) {
        self.normalize();
        if height.is_finite() && height > 0.0 {
            self.terminal_height = height;
        }
    }
    pub fn upsert(&mut self, id: &str, split: bool, vertical: bool) {
        self.normalize();
        if id.js_trim().is_empty() {
            return;
        }
        let split = split && !self.terminal_ids.is_empty();
        let is_new = !self.terminal_ids.iter().any(|s| s == id);
        let mut next = self.clone();
        if is_new {
            next.terminal_ids.push(id.into());
        }
        for group in &mut next.terminal_groups {
            group.terminal_ids.retain(|t| t != id);
        }
        next.terminal_groups.retain(|g| !g.terminal_ids.is_empty());
        if !split {
            let mut used = next.terminal_groups.iter().map(|g| g.id.clone()).collect();
            let group = Group {
                id: unique(format!("group-{id}"), &mut used),
                terminal_ids: vec![id.into()],
                split_direction: None,
            };
            next.active_terminal_group_id = group.id.clone();
            next.terminal_groups.push(group);
        } else {
            let index = next
                .terminal_groups
                .iter()
                .position(|g| g.id == self.active_terminal_group_id)
                .or_else(|| {
                    next.terminal_groups
                        .iter()
                        .position(|g| g.terminal_ids.contains(&self.active_terminal_id))
                });
            let index = match index {
                Some(i) => i,
                None => {
                    let mut used = next.terminal_groups.iter().map(|g| g.id.clone()).collect();
                    next.terminal_groups.push(Group {
                        id: unique(format!("group-{}", self.active_terminal_id), &mut used),
                        terminal_ids: vec![self.active_terminal_id.clone()],
                        split_direction: None,
                    });
                    next.terminal_groups.len() - 1
                }
            };
            let group = &mut next.terminal_groups[index];
            if is_new && group.terminal_ids.len() >= MAX_PER_GROUP {
                return;
            }
            if !group.terminal_ids.iter().any(|t| t == id) {
                let index = group
                    .terminal_ids
                    .iter()
                    .position(|t| t == &self.active_terminal_id)
                    .map_or(group.terminal_ids.len(), |i| i + 1);
                group.terminal_ids.insert(index, id.into());
            }
            group.split_direction = vertical.then(|| "vertical".into());
            next.active_terminal_group_id = group.id.clone();
        }
        next.terminal_open = true;
        next.active_terminal_id = id.into();
        next.normalize();
        *self = next;
    }
    pub fn activate(&mut self, id: &str) {
        self.normalize();
        if self.terminal_ids.iter().any(|t| t == id) {
            self.active_terminal_id = id.into();
            if let Some(group) = self
                .terminal_groups
                .iter()
                .find(|g| g.terminal_ids.contains(&self.active_terminal_id))
            {
                self.active_terminal_group_id = group.id.clone();
            }
        }
    }
    pub fn close(&mut self, id: &str) {
        self.normalize();
        let Some(index) = self.terminal_ids.iter().position(|t| t == id) else {
            return;
        };
        let remaining = self
            .terminal_ids
            .iter()
            .filter(|t| *t != id)
            .cloned()
            .collect::<Vec<_>>();
        if remaining.is_empty() {
            *self = Self::default();
            return;
        }
        if self.active_terminal_id == id {
            let group = self
                .terminal_groups
                .iter()
                .find(|g| g.terminal_ids.iter().any(|t| t == id));
            let neighbor = group.and_then(|g| {
                let i = g.terminal_ids.iter().position(|t| t == id)?;
                g.terminal_ids
                    .get(i + 1)
                    .or_else(|| i.checked_sub(1).and_then(|i| g.terminal_ids.get(i)))
            });
            self.active_terminal_id = neighbor
                .cloned()
                .unwrap_or_else(|| remaining[index.min(remaining.len() - 1)].clone());
        }
        self.terminal_ids = remaining;
        for group in &mut self.terminal_groups {
            group.terminal_ids.retain(|t| t != id);
        }
        self.terminal_groups.retain(|g| !g.terminal_ids.is_empty());
        self.active_terminal_group_id = self
            .terminal_groups
            .iter()
            .find(|g| g.terminal_ids.contains(&self.active_terminal_id))
            .map(|g| g.id.clone())
            .unwrap_or_default();
        self.normalize();
    }
    pub fn reconcile(&mut self, ids: &[String]) {
        self.normalize();
        if self.terminal_ids == ids {
            return;
        }
        self.terminal_ids = ids.to_vec();
        if !ids.contains(&self.active_terminal_id) {
            self.active_terminal_id = ids.first().cloned().unwrap_or_default();
        }
        self.active_terminal_group_id = self
            .terminal_groups
            .iter()
            .find(|g| g.terminal_ids.contains(&self.active_terminal_id))
            .map(|g| g.id.clone())
            .unwrap_or_default();
        self.normalize();
    }
}
#[derive(Debug, Clone, Default)]
pub struct TerminalStore {
    pub by_thread: BTreeMap<ThreadKey, TerminalState>,
    pub suppressed: BTreeMap<ThreadKey, Vec<String>>,
}
impl TerminalStore {
    pub fn decode(raw: &str) -> Result<Self, serde_json::Error> {
        let value: serde_json::Value = serde_json::from_str(raw)?;
        let state = value.get("state").unwrap_or(&value);
        let records = state
            .get("terminalUiStateByThreadKey")
            .filter(|v| !v.is_null())
            .or_else(|| state.get("terminalStateByThreadKey"));
        let mut store = Self::default();
        for (key, value) in records
            .and_then(serde_json::Value::as_object)
            .into_iter()
            .flatten()
        {
            if let Some(key) = super::navigation::parse_legacy_thread_key(key)
                && let Ok(value) = serde_json::from_value(value.clone())
            {
                store.by_thread.insert(key, value);
            }
        }
        Ok(store)
    }
    pub fn encode(&self) -> Result<String, serde_json::Error> {
        let rows = self
            .by_thread
            .iter()
            .map(|(key, state)| (super::navigation::legacy_thread_key(key), state))
            .collect::<BTreeMap<_, _>>();
        serde_json::to_string(
            &serde_json::json!({"version":4,"state":{"terminalUiStateByThreadKey":rows}}),
        )
    }
    pub fn update(&mut self, key: &ThreadKey, update: impl FnOnce(&mut TerminalState)) {
        if key.thread.0.is_empty() {
            return;
        }
        let state = self.by_thread.entry(key.clone()).or_default();
        update(state);
        let mut normalized = state.clone();
        normalized.normalize();
        if normalized == TerminalState::default() {
            self.by_thread.remove(key);
        }
    }
    pub fn set_open(&mut self, key: &ThreadKey, open: bool) {
        let empty = self
            .by_thread
            .get(key)
            .is_none_or(|s| s.terminal_ids.is_empty());
        self.update(key, |s| s.set_open(open));
        if open && empty {
            self.unsuppress(key, DEFAULT_TERMINAL);
        }
    }
    pub fn upsert(&mut self, key: &ThreadKey, id: &str, split: bool, vertical: bool) {
        self.update(key, |s| s.upsert(id, split, vertical));
        self.unsuppress(key, id);
    }
    pub fn ensure(&mut self, key: &ThreadKey, id: &str, open: bool, active: bool) {
        self.update(key, |s| {
            let previous = (
                s.active_terminal_id.clone(),
                s.active_terminal_group_id.clone(),
            );
            if !s.terminal_ids.iter().any(|t| t == id) {
                s.upsert(id, false, false);
            }
            if active {
                s.activate(id);
            } else {
                (s.active_terminal_id, s.active_terminal_group_id) = previous;
            }
            if open {
                s.set_open(true);
            }
            s.normalize();
        });
        self.unsuppress(key, id);
    }
    pub fn close(&mut self, key: &ThreadKey, id: &str) {
        self.update(key, |s| s.close(id));
        let id = id.js_trim();
        if !id.is_empty() {
            let ids = self.suppressed.entry(key.clone()).or_default();
            if !ids.iter().any(|t| t == id) {
                ids.push(id.into());
            }
        }
    }
    fn unsuppress(&mut self, key: &ThreadKey, id: &str) {
        if let Some(ids) = self.suppressed.get_mut(key) {
            ids.retain(|t| t != id.js_trim());
            if ids.is_empty() {
                self.suppressed.remove(key);
            }
        }
    }
    pub fn reconcile(&mut self, key: &ThreadKey, ids: &[String]) {
        let ids = ids
            .iter()
            .filter(|id| self.suppressed.get(key).is_none_or(|s| !s.contains(id)))
            .cloned()
            .collect::<Vec<_>>();
        self.update(key, |s| s.reconcile(&ids));
    }
    pub fn remove(&mut self, key: &ThreadKey) {
        self.by_thread.remove(key);
        self.suppressed.remove(key);
    }
    pub fn retain(&mut self, keys: &BTreeSet<ThreadKey>) {
        self.by_thread.retain(|key, _| keys.contains(key));
        self.suppressed.retain(|key, _| keys.contains(key));
    }
}
