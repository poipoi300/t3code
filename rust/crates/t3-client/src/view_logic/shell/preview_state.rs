//! Browser session UI state, stale-event guards, local close suppression and port filtering.
//! Desktop overlay data is opaque host-supplied `O: Clone + PartialEq`: no desktop wire schema
//! or rendering types are duplicated here. The host subscribes only to changed thread keys.
use crate::{
    types::{ThreadId, ThreadKey},
    view_logic::JsTrim,
};
use std::collections::{BTreeMap, BTreeSet};
use t3_contracts::preview::{
    CONFIGURED_LOCAL_SERVER_URLS_MAX_ITEMS, DiscoveredLocalServer, PREVIEW_URL_MAX_LENGTH,
    PreviewEvent, PreviewListResult, PreviewNavStatus as Nav, PreviewSessionSnapshot as Snapshot,
};
pub const RECENT_URL_LIMIT: usize = 10;
#[derive(Debug, Clone, PartialEq)]
pub struct ThreadPreview<O> {
    pub sessions: BTreeMap<String, Snapshot>,
    pub session_order: Vec<String>,
    pub suppressed: BTreeSet<String>,
    pub active_tab: Option<String>,
    pub desktop: BTreeMap<String, O>,
    pub recent_urls: Vec<String>,
    pub list_loaded: bool,
    pub server_epoch: Option<String>,
    pub server_revision: u64,
}
impl<O> Default for ThreadPreview<O> {
    fn default() -> Self {
        Self {
            sessions: BTreeMap::new(),
            session_order: vec![],
            suppressed: BTreeSet::new(),
            active_tab: None,
            desktop: BTreeMap::new(),
            recent_urls: vec![],
            list_loaded: false,
            server_epoch: None,
            server_revision: 0,
        }
    }
}
impl<O: Clone + PartialEq> ThreadPreview<O> {
    pub fn snapshot(&self) -> Option<&Snapshot> {
        self.active_tab
            .as_ref()
            .and_then(|id| self.sessions.get(id))
    }
    pub fn desktop_overlay(&self) -> Option<&O> {
        self.active_tab.as_ref().and_then(|id| self.desktop.get(id))
    }
    fn remember(&mut self, snapshot: &Snapshot) {
        if let Nav::Loading { url, .. } | Nav::Success { url, .. } | Nav::LoadFailed { url, .. } =
            &snapshot.nav_status
        {
            self.remember_url(&url.0);
        }
    }
    pub fn remember_url(&mut self, url: &str) {
        if url.js_trim().is_empty() {
            return;
        }
        self.recent_urls.retain(|u| u != url);
        self.recent_urls.insert(0, url.into());
        self.recent_urls.truncate(RECENT_URL_LIMIT);
    }
    fn insert(&mut self, snapshot: Snapshot) {
        let id = snapshot.tab_id.0.clone();
        if !self.sessions.contains_key(&id) {
            self.session_order.push(id.clone());
        }
        self.sessions.insert(id, snapshot);
    }
    fn latest(&self) -> Option<String> {
        self.session_order
            .iter()
            .filter_map(|id| self.sessions.get(id).map(|s| (id, &s.updated_at)))
            .max_by(|(_, a), (_, b)| a.cmp(b))
            .map(|(id, _)| id.clone())
    }
    fn remove(&mut self, id: &str) {
        if self.sessions.remove(id).is_none() {
            return;
        }
        self.session_order.retain(|s| s != id);
        self.desktop.remove(id);
        if self.active_tab.as_deref() == Some(id) {
            self.active_tab = self.latest();
        }
        if self
            .active_tab
            .as_ref()
            .is_none_or(|id| !self.sessions.contains_key(id))
        {
            self.active_tab = self.latest();
        }
    }
    pub fn apply_snapshot(&mut self, snapshot: Option<Snapshot>) -> bool {
        let before = self.clone();
        let Some(snapshot) = snapshot else {
            if self.snapshot().is_some() {
                self.sessions.clear();
                self.session_order.clear();
                self.desktop.clear();
                self.active_tab = None;
            }
            return *self != before;
        };
        if self.suppressed.contains(&snapshot.tab_id.0)
            || self
                .sessions
                .get(&snapshot.tab_id.0)
                .is_some_and(|old| old.updated_at > snapshot.updated_at)
        {
            return false;
        }
        self.remember(&snapshot);
        self.active_tab = Some(snapshot.tab_id.0.clone());
        self.insert(snapshot);
        *self != before
    }
    pub fn update_snapshot(&mut self, snapshot: Snapshot) -> bool {
        if self.suppressed.contains(&snapshot.tab_id.0)
            || self
                .sessions
                .get(&snapshot.tab_id.0)
                .is_some_and(|old| old.updated_at > snapshot.updated_at)
        {
            return false;
        }
        let before = self.clone();
        self.remember(&snapshot);
        let id = snapshot.tab_id.0.clone();
        self.insert(snapshot);
        if self
            .active_tab
            .as_ref()
            .is_none_or(|id| !self.sessions.contains_key(id))
        {
            self.active_tab = Some(id);
        }
        *self != before
    }
    pub fn apply_event(&mut self, event: PreviewEvent) -> bool {
        let (epoch, revision) = match &event {
            PreviewEvent::Opened {
                server_epoch,
                revision,
                ..
            }
            | PreviewEvent::Navigated {
                server_epoch,
                revision,
                ..
            }
            | PreviewEvent::Resized {
                server_epoch,
                revision,
                ..
            }
            | PreviewEvent::Failed {
                server_epoch,
                revision,
                ..
            }
            | PreviewEvent::Closed {
                server_epoch,
                revision,
                ..
            } => (server_epoch.0.clone(), *revision),
        };
        if self.server_epoch.as_ref().is_some_and(|e| e != &epoch)
            || revision < self.server_revision
        {
            return false;
        }
        let before = self.clone();
        let opened = matches!(event, PreviewEvent::Opened { .. });
        match event {
            PreviewEvent::Opened { snapshot, .. }
            | PreviewEvent::Navigated { snapshot, .. }
            | PreviewEvent::Resized { snapshot, .. } => {
                if !self.suppressed.contains(&snapshot.tab_id.0) {
                    self.remember(&snapshot);
                    let id = snapshot.tab_id.0.clone();
                    self.insert(snapshot);
                    if opened || self.active_tab.is_none() {
                        self.active_tab = Some(id);
                    }
                }
            }
            PreviewEvent::Failed {
                tab_id,
                url,
                title,
                code,
                description,
                download,
                created_at,
                ..
            } => {
                if let Some(s) = self.sessions.get_mut(&tab_id.0) {
                    s.nav_status = Nav::LoadFailed {
                        url,
                        title,
                        code,
                        description,
                        download,
                    };
                    s.updated_at = created_at;
                }
            }
            PreviewEvent::Closed { tab_id, .. } => {
                self.remove(&tab_id.0);
                self.suppressed.remove(&tab_id.0);
            }
        }
        self.server_epoch = Some(epoch);
        self.server_revision = revision;
        *self != before
    }
    pub fn reconcile(&mut self, result: PreviewListResult) -> bool {
        let same = self.server_epoch.as_deref() == Some(&result.server_epoch.0);
        if same && result.revision < self.server_revision {
            return false;
        }
        let before = self.clone();
        let suppressed = if same {
            self.suppressed.clone()
        } else {
            BTreeSet::new()
        };
        let mut sessions = BTreeMap::new();
        let mut order = vec![];
        let mut listed = BTreeSet::new();
        for snapshot in result.sessions {
            let id = snapshot.tab_id.0.clone();
            listed.insert(id.clone());
            if suppressed.contains(&id) {
                continue;
            }
            let next = if same {
                self.sessions
                    .get(&id)
                    .filter(|s| s.updated_at > snapshot.updated_at)
                    .cloned()
                    .unwrap_or(snapshot)
            } else {
                snapshot
            };
            self.remember(&next);
            if !sessions.contains_key(&id) {
                order.push(id.clone());
            }
            sessions.insert(id, next);
        }
        self.sessions = sessions;
        self.session_order = order;
        if self
            .active_tab
            .as_ref()
            .is_none_or(|id| !self.sessions.contains_key(id))
        {
            self.active_tab = self.latest();
        }
        if same {
            self.desktop.retain(|id, _| self.sessions.contains_key(id));
        } else {
            self.desktop.clear();
        }
        self.suppressed = suppressed
            .into_iter()
            .filter(|id| listed.contains(id))
            .collect();
        self.list_loaded = true;
        self.server_epoch = Some(result.server_epoch.0);
        self.server_revision = result.revision;
        *self != before
    }
    pub fn set_desktop(&mut self, id: &str, overlay: Option<O>) -> bool {
        if self.desktop.get(id) == overlay.as_ref() {
            return false;
        }
        if let Some(overlay) = overlay {
            self.desktop.insert(id.into(), overlay);
        } else {
            self.desktop.remove(id);
        }
        true
    }
    pub fn begin_close(&mut self, id: &str) -> bool {
        let before = self.clone();
        self.suppressed.insert(id.into());
        self.remove(id);
        *self != before
    }
    pub fn cancel_close(&mut self, id: &str, snapshot: Option<Snapshot>) -> bool {
        if !self.suppressed.remove(id) {
            return false;
        }
        if let Some(snapshot) = snapshot {
            self.remember(&snapshot);
            self.active_tab = Some(snapshot.tab_id.0.clone());
            self.insert(snapshot);
        }
        true
    }
    pub fn set_active(&mut self, id: &str) -> bool {
        if !self.sessions.contains_key(id) || self.active_tab.as_deref() == Some(id) {
            return false;
        }
        self.active_tab = Some(id.into());
        true
    }
    pub fn clear(&mut self) -> bool {
        if self.sessions.is_empty() {
            return false;
        }
        *self = Self::default();
        true
    }
}
#[derive(Debug, Clone)]
pub struct PreviewStore<O> {
    pub threads: BTreeMap<ThreadKey, ThreadPreview<O>>,
}
impl<O> Default for PreviewStore<O> {
    fn default() -> Self {
        Self {
            threads: BTreeMap::new(),
        }
    }
}
impl<O: Clone + PartialEq> PreviewStore<O> {
    pub fn active_sessions(&self) -> impl Iterator<Item = (&ThreadKey, &ThreadPreview<O>)> {
        self.threads.iter().filter(|(_, s)| !s.sessions.is_empty())
    }
    pub fn update(
        &mut self,
        key: ThreadKey,
        update: impl FnOnce(&mut ThreadPreview<O>) -> bool,
    ) -> bool {
        update(self.threads.entry(key).or_default())
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabWait {
    Ready,
    Waiting,
    Expired,
}
pub fn tab_wait<O>(state: &ThreadPreview<O>, id: &str, deadline: u64, now: u64) -> TabWait {
    if state.sessions.contains_key(id) {
        TabWait::Ready
    } else if now >= deadline {
        TabWait::Expired
    } else {
        TabWait::Waiting
    }
}
pub fn bound_configured_urls(urls: &[String]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut bounded = vec![];
    for raw in urls {
        if raw.is_empty()
            || raw.encode_utf16().count() > PREVIEW_URL_MAX_LENGTH
            || raw.js_trim() != raw
        {
            continue;
        }
        let Ok(url) = url::Url::parse(raw) else {
            continue;
        };
        if !matches!(url.scheme(), "http" | "https")
            || !matches!(
                url.host_str(),
                Some("localhost" | "127.0.0.1" | "0.0.0.0" | "::1" | "[::1]")
            )
            || url.as_str().encode_utf16().count() > PREVIEW_URL_MAX_LENGTH
        {
            continue;
        }
        let mut resource = url.clone();
        resource.set_fragment(None);
        if seen.insert(resource.to_string()) {
            bounded.push(url.to_string());
            if bounded.len() >= CONFIGURED_LOCAL_SERVER_URLS_MAX_ITEMS {
                break;
            }
        }
    }
    bounded
}
pub fn thread_discovered_ports<'a>(
    servers: &'a [DiscoveredLocalServer],
    thread: Option<&ThreadId>,
) -> Vec<&'a DiscoveredLocalServer> {
    let Some(thread) = thread else {
        return vec![];
    };
    servers
        .iter()
        .filter(|s| {
            s.terminal.as_ref().is_some_and(|s| &s.thread_id == thread)
                || s.agent
                    .as_ref()
                    .and_then(Option::as_ref)
                    .is_some_and(|s| &s.thread_id == thread)
        })
        .collect()
}
