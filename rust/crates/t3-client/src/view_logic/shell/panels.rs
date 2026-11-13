//! Right-panel surfaces and local transitions. Returned closed surfaces are remembered by the caller
//! with a host-generated id. Session terminals/browser tabs are reopened through their owning service.
//! Unknown persisted surface kinds retain their payload but cannot enter reopen history.
use super::navigation::{encode_component, legacy_thread_key, parse_legacy_thread_key};
use crate::{types::ThreadKey, view_logic::JsTrim};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use t3_contracts::{base_schemas::ThreadId, chat_attachment::ChatFileAttachment};
pub const STORAGE_KEY: &str = "t3code:right-panel-state:v2";
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceTarget {
    pub host_id: String,
    pub device_id: String,
    pub platform: String,
    pub name: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Surface {
    pub id: String,
    #[serde(flatten)]
    pub content: SurfaceContent,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum SurfaceContent {
    Diff,
    Files {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        folder_trail: Option<String>,
    },
    File {
        relative_path: String,
        reveal_line: Option<i64>,
        reveal_request_id: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attachment: Option<ChatFileAttachment>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        folder_trail: Option<String>,
    },
    Preview {
        resource_id: Option<String>,
    },
    Device {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<DeviceTarget>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    Terminal {
        resource_id: String,
        terminal_ids: Vec<String>,
        active_terminal_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        split_direction: Option<String>,
    },
    PullRequest {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        environment_id: Option<String>,
        project_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host: Option<String>,
        repository: String,
        number: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reveal_url: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail_tab: Option<String>,
    },
    PullRequests,
    SideChat {
        thread_id: ThreadId,
    },
    #[serde(untagged)]
    Unknown {
        #[serde(flatten)]
        fields: BTreeMap<String, serde_json::Value>,
    },
}
impl Surface {
    pub fn kind(&self) -> &'static str {
        match self.content {
            SurfaceContent::Diff => "diff",
            SurfaceContent::Files { .. } => "files",
            SurfaceContent::File { .. } => "file",
            SurfaceContent::Preview { .. } => "preview",
            SurfaceContent::Device { .. } => "device",
            SurfaceContent::Terminal { .. } => "terminal",
            SurfaceContent::PullRequest { .. } => "pull-request",
            SurfaceContent::PullRequests => "pull-requests",
            SurfaceContent::SideChat { .. } => "side-chat",
            SurfaceContent::Unknown { .. } => "unknown",
        }
    }
    pub fn singleton(kind: &str) -> Option<Self> {
        let content = match kind {
            "diff" => SurfaceContent::Diff,
            "files" => SurfaceContent::Files { folder_trail: None },
            "device" => SurfaceContent::Device {
                target: None,
                title: None,
            },
            "pull-requests" => SurfaceContent::PullRequests,
            _ => return None,
        };
        Some(Self {
            id: kind.into(),
            content,
        })
    }
    pub fn browser(tab: Option<&str>) -> Self {
        Self {
            id: format!("browser:{}", tab.filter(|t| !t.is_empty()).unwrap_or("new")),
            content: SurfaceContent::Preview {
                resource_id: tab.filter(|t| !t.is_empty()).map(str::to_owned),
            },
        }
    }
    pub fn file(path: &str, line: Option<f64>, request: u64) -> Self {
        Self {
            id: format!("file:{path}"),
            content: SurfaceContent::File {
                relative_path: path.into(),
                reveal_line: line
                    .filter(|n| n.is_finite())
                    .map(|n| n.trunc().max(1.0) as i64),
                reveal_request_id: request,
                attachment: None,
                folder_trail: None,
            },
        }
    }
    pub fn terminal(id: &str) -> Self {
        Self {
            id: format!("terminal:{id}"),
            content: SurfaceContent::Terminal {
                resource_id: id.into(),
                terminal_ids: vec![id.into()],
                active_terminal_id: id.into(),
                split_direction: None,
            },
        }
    }
    pub fn without_reveal(mut self) -> Self {
        if let SurfaceContent::PullRequest { reveal_url, .. } = &mut self.content {
            *reveal_url = None;
        }
        self
    }
    pub fn can_remember(&self) -> bool {
        !matches!(
            self.content,
            SurfaceContent::Unknown { .. }
                | SurfaceContent::Terminal { .. }
                | SurfaceContent::Preview {
                    resource_id: Some(_)
                }
        )
    }
    pub fn pull_request(
        environment: Option<&str>,
        project: &str,
        host: Option<&str>,
        repository: &str,
        number: u64,
    ) -> Self {
        let scope = environment
            .map(|e| format!("{}:", encode_component(e)))
            .unwrap_or_default();
        let host = host.map(str::to_lowercase);
        let host_key = host
            .as_ref()
            .map(|h| format!("{}:", encode_component(h)))
            .unwrap_or_default();
        Self {
            id: format!(
                "pull-request:{scope}{}:{host_key}{}:{number}",
                encode_component(project),
                encode_component(repository)
            ),
            content: SurfaceContent::PullRequest {
                environment_id: environment.map(str::to_owned),
                project_id: project.into(),
                host,
                repository: repository.into(),
                number,
                url: None,
                reveal_url: None,
                detail_tab: None,
            },
        }
    }
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Panel {
    pub is_open: bool,
    pub active_surface_id: Option<String>,
    pub surfaces: Vec<Surface>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dismissed_device_surface_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maximized: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proactive_diff_turn_id: Option<String>,
}
impl Panel {
    pub fn selected(&self) -> Option<&Surface> {
        self.surfaces
            .iter()
            .find(|s| Some(&s.id) == self.active_surface_id.as_ref())
    }
    pub fn active(&self) -> Option<&Surface> {
        self.is_open.then(|| self.selected()).flatten()
    }
    fn upsert(&mut self, surface: Surface, activate: bool) {
        self.is_open = true;
        if activate {
            self.active_surface_id = Some(surface.id.clone());
        }
        if !self.surfaces.iter().any(|s| s.id == surface.id) {
            self.surfaces.push(surface);
        }
    }
    fn activate(&mut self, id: &str) {
        if self.surfaces.iter().any(|s| s.id == id) {
            self.is_open = true;
            self.active_surface_id = Some(id.into());
        }
    }
    fn close_surface(&mut self, id: &str) {
        let Some(i) = self.surfaces.iter().position(|s| s.id == id) else {
            return;
        };
        self.surfaces.remove(i);
        self.is_open &= !self.surfaces.is_empty();
        if self.active_surface_id.as_deref() == Some(id) {
            self.active_surface_id = self
                .surfaces
                .get(i.min(self.surfaces.len().saturating_sub(1)))
                .map(|s| s.id.clone());
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadPanelVisibility {
    pub inline_open: bool,
    pub popover_open: bool,
}
impl Default for ThreadPanelVisibility {
    fn default() -> Self {
        Self {
            inline_open: true,
            popover_open: false,
        }
    }
}
#[derive(Debug, Clone)]
pub enum Action {
    Open(String),
    OpenSurface(Surface),
    OpenBrowser(Option<String>),
    OpenFile {
        path: String,
        line: Option<f64>,
    },
    NavigateFolder {
        surface: String,
        path: String,
        trail: String,
        keep_source: bool,
    },
    OpenAttachment(ChatFileAttachment),
    OpenDevice {
        target: DeviceTarget,
        automatic: bool,
    },
    RenameDevice {
        surface: String,
        title: String,
    },
    SplitTerminal {
        surface: String,
        terminal: String,
        vertical: bool,
    },
    ActivateTerminal {
        surface: String,
        terminal: String,
    },
    CloseTerminal {
        surface: String,
        terminal: String,
    },
    Activate(String),
    Reveal(String),
    CloseSurface(String),
    CloseOthers(String),
    CloseRight(String),
    CloseAll,
    Move {
        surface: String,
        to: isize,
    },
    ReconcileBrowsers {
        tabs: Vec<String>,
        hidden: BTreeSet<String>,
    },
    ReconcileFiles {
        workspace_available: bool,
    },
    Show,
    Close,
    ToggleVisibility,
    Toggle(String),
    Maximize(bool),
    SetThreadPanel {
        inline: bool,
        open: bool,
    },
    ToggleThreadPanel {
        inline: bool,
    },
    DetailTab {
        surface: String,
        tab: String,
    },
    SettleReveal {
        surface: String,
        url: String,
    },
}
impl Action {
    fn automatic(&self) -> bool {
        matches!(
            self,
            Self::Reveal(_)
                | Self::Move { .. }
                | Self::ReconcileBrowsers { .. }
                | Self::ReconcileFiles { .. }
                | Self::Maximize(_)
                | Self::DetailTab { .. }
                | Self::SettleReveal { .. }
                | Self::SetThreadPanel { .. }
                | Self::ToggleThreadPanel { .. }
                | Self::OpenDevice {
                    automatic: true,
                    ..
                }
        )
    }
    fn closes_view(&self) -> bool {
        matches!(
            self,
            Self::CloseSurface(_)
                | Self::CloseOthers(_)
                | Self::CloseRight(_)
                | Self::CloseAll
                | Self::CloseTerminal { .. }
        )
    }
}
#[derive(Debug, Clone, Default)]
pub struct PanelStore {
    pub by_thread: BTreeMap<ThreadKey, Panel>,
    pub visibility: BTreeMap<ThreadKey, ThreadPanelVisibility>,
    pub user_revision: BTreeMap<ThreadKey, u64>,
    pub close_revision: BTreeMap<ThreadKey, u64>,
    pub pending_maximize: Option<ThreadKey>,
    hosts: BTreeMap<ThreadKey, (u64, ThreadKey)>,
    next_host: u64,
}
impl PanelStore {
    pub fn host(&mut self, guest: ThreadKey, host: ThreadKey) -> u64 {
        self.next_host += 1;
        self.hosts.insert(guest, (self.next_host, host));
        self.next_host
    }
    pub fn unhost(&mut self, guest: &ThreadKey, lease: u64) {
        if self.hosts.get(guest).is_some_and(|(id, _)| *id == lease) {
            self.hosts.remove(guest);
        }
    }
    pub fn host_ref<'a>(&'a self, key: &'a ThreadKey) -> &'a ThreadKey {
        self.hosts.get(key).map_or(key, |(_, host)| host)
    }
    pub fn user_revision(&self, key: &ThreadKey) -> u64 {
        self.user_revision.get(key).copied().unwrap_or(0)
    }
    pub fn mark_user_action(&mut self, key: &ThreadKey) -> u64 {
        let n = self.user_revision.entry(key.clone()).or_default();
        *n += 1;
        *n
    }
    pub fn consume_maximize(&mut self, key: &ThreadKey) -> bool {
        if self.pending_maximize.as_ref() != Some(key) {
            return false;
        }
        self.pending_maximize = None;
        true
    }
    pub fn open_proactive(
        &mut self,
        key: &ThreadKey,
        surface: Surface,
        turn: Option<&str>,
        expected: u64,
        activate_existing: bool,
    ) -> bool {
        if let Some(turn) = turn {
            let current = self.by_thread.get(key);
            if current.and_then(|p| p.proactive_diff_turn_id.as_deref()) == Some(turn) {
                return false;
            }
            let active = current.and_then(Panel::active).map(Surface::kind);
            let open = self.user_revision(key) == expected
                && !matches!(active, Some("diff" | "pull-request" | "pull-requests"));
            let mut p = current.cloned().unwrap_or_default();
            if open {
                p.upsert(surface, true);
            }
            p.proactive_diff_turn_id = Some(turn.into());
            self.commit(key.clone(), p);
            return open;
        }
        if self.user_revision(key) != expected
            || (!activate_existing
                && self
                    .by_thread
                    .get(key)
                    .is_some_and(|p| p.surfaces.iter().any(|s| s.id == surface.id)))
        {
            return false;
        }
        let mut p = self.by_thread.get(key).cloned().unwrap_or_default();
        p.upsert(surface, true);
        self.commit(key.clone(), p);
        true
    }
    fn commit(&mut self, key: ThreadKey, panel: Panel) {
        let was_open = self.by_thread.get(&key).is_some_and(|p| p.is_open);
        let visibility = self.visibility.entry(key.clone()).or_default();
        if !was_open && panel.is_open {
            visibility.popover_open = false;
        } else if was_open && !panel.is_open && visibility.popover_open && !visibility.inline_open {
            visibility.inline_open = true;
        }
        if *visibility == ThreadPanelVisibility::default() {
            self.visibility.remove(&key);
        }
        if !panel.is_open
            && panel.active_surface_id.is_none()
            && panel.surfaces.is_empty()
            && panel
                .dismissed_device_surface_ids
                .as_ref()
                .is_none_or(Vec::is_empty)
            && panel.proactive_diff_turn_id.is_none()
        {
            self.by_thread.remove(&key);
        } else {
            self.by_thread.insert(key, panel);
        }
    }
    /// Closed surfaces are in source order with the previously active surface last (remembering
    /// each at the head of history makes it the first reopened after a bulk close).
    pub fn apply(&mut self, key: &ThreadKey, action: Action) -> Vec<Surface> {
        let automatic = action.automatic();
        let closes = action.closes_view();
        let hosted = !automatic && !matches!(action, Action::Move { .. });
        let key = if hosted {
            self.host_ref(key).clone()
        } else {
            key.clone()
        };
        let before = self.by_thread.get(&key).cloned().unwrap_or_default();
        let mut panel = before.clone();
        if !automatic {
            self.mark_user_action(&key);
        }
        if closes {
            *self.close_revision.entry(key.clone()).or_default() += 1;
        }
        match action {
            Action::Open(kind) => {
                if kind == "preview" {
                    let surface = panel
                        .surfaces
                        .iter()
                        .find(|s| s.kind() == "preview")
                        .cloned()
                        .unwrap_or_else(|| Surface::browser(None));
                    panel.upsert(surface, true);
                } else if let Some(s) = Surface::singleton(&kind) {
                    panel.upsert(s, true);
                }
            }
            Action::OpenSurface(surface) => {
                let id = surface.id.clone();
                let update = matches!(
                    &surface.content,
                    SurfaceContent::PullRequest { url: Some(_), .. }
                        | SurfaceContent::PullRequest {
                            reveal_url: Some(_),
                            ..
                        }
                );
                panel.upsert(surface.clone(), true);
                if update {
                    for s in &mut panel.surfaces {
                        if s.id == id {
                            let mut next = surface.clone();
                            if let (
                                SurfaceContent::PullRequest {
                                    detail_tab: old, ..
                                },
                                SurfaceContent::PullRequest {
                                    detail_tab: new, ..
                                },
                            ) = (&s.content, &mut next.content)
                                && new.is_none()
                            {
                                new.clone_from(old);
                            }
                            *s = next;
                        }
                    }
                }
            }
            Action::OpenBrowser(tab) => {
                if tab.as_ref().is_some_and(|t| !t.is_empty()) {
                    panel.surfaces.retain(|s| s.id != "browser:new");
                }
                panel.upsert(Surface::browser(tab.as_deref()), true);
            }
            Action::OpenFile { path, line } => {
                if path == "." {
                    if let Some(s) = Surface::singleton("files") {
                        panel.upsert(s, true);
                    }
                } else {
                    let root = path.len() >= 3
                        && path.as_bytes()[0].is_ascii_alphabetic()
                        && path.as_bytes()[1] == b':'
                        && path[2..].chars().all(|c| c == '/');
                    let normalized = if root {
                        path.as_str()
                    } else {
                        let trimmed = path.trim_end_matches('/');
                        if trimmed.is_empty() { &path } else { trimmed }
                    };
                    panel.surfaces.retain(|s| s.kind() != "files");
                    let id = format!("file:{normalized}");
                    let request = panel
                        .surfaces
                        .iter()
                        .find_map(|s| match &s.content {
                            SurfaceContent::File {
                                reveal_request_id, ..
                            } if s.id == id => Some(*reveal_request_id),
                            _ => None,
                        })
                        .unwrap_or(0)
                        + 1;
                    let surface = Surface::file(normalized, line, request);
                    if let Some(s) = panel.surfaces.iter_mut().find(|s| s.id == id) {
                        *s = surface.clone();
                    }
                    panel.upsert(surface, true);
                }
            }
            Action::NavigateFolder {
                surface,
                path,
                trail,
                keep_source,
            } => {
                if let Some(index) = panel.surfaces.iter().position(|s| s.id == surface) {
                    let target = if path.is_empty() {
                        Surface {
                            id: "files".into(),
                            content: SurfaceContent::Files {
                                folder_trail: (trail != path).then_some(trail),
                            },
                        }
                    } else {
                        let mut s = Surface::file(&path, None, 1);
                        if let SurfaceContent::File { folder_trail, .. } = &mut s.content {
                            *folder_trail = (trail != path).then_some(trail);
                        }
                        s
                    };
                    let existing = panel
                        .surfaces
                        .iter()
                        .enumerate()
                        .any(|(i, s)| i != index && s.id == target.id);
                    let mut rows = vec![];
                    for (i, s) in panel.surfaces.into_iter().enumerate() {
                        if i == index {
                            if keep_source {
                                rows.push(s);
                            }
                            if !existing {
                                rows.push(target.clone());
                            }
                        } else if existing || target.kind() != "file" || s.kind() != "files" {
                            rows.push(s);
                        }
                    }
                    panel.surfaces = rows;
                    panel.is_open = true;
                    panel.active_surface_id = Some(target.id);
                }
            }
            Action::OpenAttachment(attachment) => {
                panel.surfaces.retain(|s| s.kind() != "files");
                let surface = Surface {
                    id: format!("attachment:{}", attachment.id),
                    content: SurfaceContent::File {
                        relative_path: attachment.name.clone(),
                        reveal_line: None,
                        reveal_request_id: 0,
                        attachment: Some(attachment),
                        folder_trail: None,
                    },
                };
                panel.upsert(surface, true);
            }
            Action::OpenDevice { target, automatic } => {
                let id = format!(
                    "device:{}:{}",
                    encode_component(&target.host_id),
                    encode_component(&target.device_id)
                );
                if !(automatic
                    && panel
                        .dismissed_device_surface_ids
                        .as_ref()
                        .is_some_and(|ids| ids.contains(&id)))
                {
                    let surface = panel
                        .surfaces
                        .iter()
                        .find(|s| s.id == id)
                        .cloned()
                        .unwrap_or(Surface {
                            id: id.clone(),
                            content: SurfaceContent::Device {
                                target: Some(target),
                                title: None,
                            },
                        });
                    let existing = panel.surfaces.iter().any(|s| s.id == id);
                    if existing {
                        panel.surfaces.retain(|s| s.id != "device");
                    } else {
                        for s in &mut panel.surfaces {
                            if s.id == "device" {
                                *s = surface.clone();
                            }
                        }
                    }
                    if let Some(ids) = &mut panel.dismissed_device_surface_ids {
                        ids.retain(|s| s != &id);
                    } else {
                        panel.dismissed_device_surface_ids = Some(vec![]);
                    }
                    panel.upsert(surface, true);
                }
            }
            Action::RenameDevice { surface, title } => {
                if let Some(s) = panel.surfaces.iter_mut().find(|s| s.id == surface)
                    && let SurfaceContent::Device {
                        target,
                        title: label,
                    } = &mut s.content
                {
                    *label = Some(if !title.js_trim().is_empty() {
                        title.js_trim().into()
                    } else {
                        target.as_ref().map_or("Device", |t| t.name.as_str()).into()
                    });
                }
            }
            Action::SplitTerminal {
                surface,
                terminal,
                vertical,
            } => {
                panel.is_open = true;
                panel.active_surface_id = Some(surface.clone());
                if let Some(s) = panel.surfaces.iter_mut().find(|s| s.id == surface)
                    && let SurfaceContent::Terminal {
                        terminal_ids,
                        active_terminal_id,
                        split_direction,
                        ..
                    } = &mut s.content
                {
                    if !terminal_ids.contains(&terminal) {
                        terminal_ids.push(terminal.clone());
                    }
                    *active_terminal_id = terminal;
                    *split_direction = vertical.then(|| "vertical".into());
                }
            }
            Action::ActivateTerminal { surface, terminal } => {
                panel.active_surface_id = Some(surface.clone());
                if let Some(s) = panel.surfaces.iter_mut().find(|s| s.id == surface)
                    && let SurfaceContent::Terminal {
                        terminal_ids,
                        active_terminal_id,
                        ..
                    } = &mut s.content
                    && terminal_ids.contains(&terminal)
                {
                    *active_terminal_id = terminal;
                }
            }
            Action::CloseTerminal { surface, terminal } => {
                let mut remove = false;
                if let Some(s) = panel.surfaces.iter_mut().find(|s| s.id == surface)
                    && let SurfaceContent::Terminal {
                        terminal_ids,
                        active_terminal_id,
                        ..
                    } = &mut s.content
                {
                    terminal_ids.retain(|t| t != &terminal);
                    remove = terminal_ids.is_empty();
                    if *active_terminal_id == terminal {
                        *active_terminal_id = terminal_ids.last().cloned().unwrap_or_default();
                    }
                }
                if remove {
                    panel.close_surface(&surface);
                }
            }
            Action::Activate(id) | Action::Reveal(id) => panel.activate(&id),
            Action::CloseSurface(id) => panel.close_surface(&id),
            Action::CloseOthers(id) => {
                if let Some(s) = panel.surfaces.iter().find(|s| s.id == id).cloned() {
                    panel.surfaces = vec![s];
                    panel.active_surface_id = Some(id);
                    panel.is_open = true;
                }
            }
            Action::CloseRight(id) => {
                if let Some(index) = panel.surfaces.iter().position(|s| s.id == id) {
                    panel.surfaces.truncate(index + 1);
                    if !panel
                        .surfaces
                        .iter()
                        .any(|s| Some(&s.id) == panel.active_surface_id.as_ref())
                    {
                        panel.active_surface_id = Some(id);
                    }
                }
            }
            Action::CloseAll => {
                if !panel.surfaces.is_empty() {
                    panel.surfaces.clear();
                    panel.is_open = false;
                    panel.active_surface_id = None;
                }
            }
            Action::Move { surface, to } => {
                if let Some(index) = panel.surfaces.iter().position(|s| s.id == surface) {
                    let s = panel.surfaces.remove(index);
                    let to = if to < 0 {
                        panel.surfaces.len().saturating_sub(to.unsigned_abs())
                    } else {
                        (to as usize).min(panel.surfaces.len())
                    };
                    panel.surfaces.insert(to, s);
                }
            }
            Action::ReconcileBrowsers { tabs, hidden } => {
                let had_browser = panel.surfaces.iter().any(|s| {
                    matches!(
                        s.content,
                        SurfaceContent::Preview {
                            resource_id: Some(_)
                        }
                    )
                });
                panel.surfaces.retain(|s| match &s.content {
                    SurfaceContent::Preview {
                        resource_id: Some(id),
                    } => tabs.contains(id),
                    SurfaceContent::Preview { resource_id: None } => false,
                    _ => true,
                });
                for tab in tabs {
                    let s = Surface::browser(Some(&tab));
                    if !hidden.contains(&tab) && !panel.surfaces.iter().any(|r| r.id == s.id) {
                        panel.surfaces.push(s);
                    }
                }
                if panel.surfaces.is_empty() && had_browser {
                    panel.is_open = false;
                }
                if !panel
                    .surfaces
                    .iter()
                    .any(|s| Some(&s.id) == panel.active_surface_id.as_ref())
                {
                    panel.active_surface_id = panel
                        .surfaces
                        .iter()
                        .find(|s| s.kind() == "preview")
                        .or_else(|| panel.surfaces.first())
                        .map(|s| s.id.clone());
                }
            }
            Action::ReconcileFiles {
                workspace_available,
            } => {
                if !workspace_available {
                    panel.surfaces.retain(|s| {
                        !matches!(
                            s.content,
                            SurfaceContent::Files { .. }
                                | SurfaceContent::File {
                                    attachment: None,
                                    ..
                                }
                        )
                    });
                    if panel.surfaces.is_empty() {
                        panel.is_open = false;
                    }
                    if !panel
                        .surfaces
                        .iter()
                        .any(|s| Some(&s.id) == panel.active_surface_id.as_ref())
                    {
                        panel.active_surface_id = panel.surfaces.last().map(|s| s.id.clone());
                    }
                }
            }
            Action::Show => panel.is_open = true,
            Action::Close => panel.is_open = false,
            Action::ToggleVisibility => panel.is_open = !panel.is_open,
            Action::Toggle(kind) => {
                if panel.active().is_some_and(|s| s.kind() == kind) {
                    panel.is_open = false;
                } else if kind == "preview" {
                    let s = panel
                        .surfaces
                        .iter()
                        .find(|s| s.kind() == "preview")
                        .cloned()
                        .unwrap_or_else(|| Surface::browser(None));
                    panel.upsert(s, true);
                } else if let Some(s) = Surface::singleton(&kind) {
                    panel.upsert(s, true);
                }
            }
            Action::Maximize(value) => panel.maximized = value.then_some(true),
            Action::SetThreadPanel { inline, open } => {
                let v = self.visibility.entry(key.clone()).or_default();
                if inline {
                    v.inline_open = open;
                } else {
                    v.popover_open = open;
                }
            }
            Action::ToggleThreadPanel { inline } => {
                let v = self.visibility.entry(key.clone()).or_default();
                if inline {
                    v.inline_open = !v.inline_open;
                } else {
                    v.popover_open = !v.popover_open;
                }
            }
            Action::DetailTab { surface, tab } => {
                if let Some(s) = panel.surfaces.iter_mut().find(|s| s.id == surface)
                    && let SurfaceContent::PullRequest { detail_tab, .. } = &mut s.content
                {
                    *detail_tab = Some(tab);
                }
            }
            Action::SettleReveal { surface, url } => {
                if let Some(s) = panel.surfaces.iter_mut().find(|s| s.id == surface)
                    && let SurfaceContent::PullRequest { reveal_url, .. } = &mut s.content
                    && reveal_url.as_ref() == Some(&url)
                {
                    *reveal_url = None;
                }
            }
        }
        if !automatic {
            let removed = before
                .surfaces
                .iter()
                .filter(|s| s.kind() == "device" && !panel.surfaces.iter().any(|r| r.id == s.id));
            for s in removed {
                let ids = panel.dismissed_device_surface_ids.get_or_insert_default();
                if !ids.contains(&s.id) {
                    ids.push(s.id.clone());
                }
            }
        }
        let mut closed = if closes {
            before
                .surfaces
                .iter()
                .filter(|s| s.can_remember() && !panel.surfaces.iter().any(|r| r.id == s.id))
                .cloned()
                .collect::<Vec<_>>()
        } else {
            vec![]
        };
        closed.sort_by_key(|s| Some(&s.id) == before.active_surface_id.as_ref());
        self.commit(key, panel);
        closed.into_iter().map(Surface::without_reveal).collect()
    }
    pub fn remove(&mut self, key: &ThreadKey) {
        self.by_thread.remove(key);
        self.visibility.remove(key);
        self.user_revision.remove(key);
        self.close_revision.remove(key);
    }
}
#[path = "panel_persistence.rs"]
mod persistence;
