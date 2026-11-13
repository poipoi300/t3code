//! Closed-view history and reopen plans. Browser recreation is an injected async operation.
use super::panels::{Action, Panel, PanelStore, Surface, SurfaceContent};
use crate::types::ThreadKey;
use serde::{Deserialize, Serialize};
use t3_contracts::{environment::ScopedThreadRef, preview::PreviewSessionSnapshot};
pub const STORAGE_KEY: &str = "t3code:closed-views:v2";
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum ClosedView {
    PanelTab {
        thread_ref: ScopedThreadRef,
        surface: Surface,
    },
    Browser {
        thread_ref: ScopedThreadRef,
        snapshot: PreviewSessionSnapshot,
    },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    #[serde(flatten)]
    pub view: ClosedView,
}
impl ClosedView {
    pub fn thread(&self) -> &ScopedThreadRef {
        match self {
            Self::PanelTab { thread_ref, .. } | Self::Browser { thread_ref, .. } => thread_ref,
        }
    }
    pub fn key(&self) -> ThreadKey {
        let t = self.thread();
        ThreadKey {
            environment: t.environment_id.clone(),
            thread: t.thread_id.clone(),
        }
    }
    fn persistent(&self) -> bool {
        !matches!(self,Self::Browser{snapshot,..}if snapshot.profile_id.as_ref().and_then(Option::as_ref).is_some_and(|p|p==t3_contracts::browser_profile::INCOGNITO_BROWSER_PROFILE_ID))
    }
    fn valid(&self) -> bool {
        match self {
            Self::Browser { .. } => true,
            Self::PanelTab { surface, .. } => match &surface.content {
                SurfaceContent::Terminal { .. } | SurfaceContent::Unknown { .. } => false,
                SurfaceContent::Diff
                | SurfaceContent::Files { .. }
                | SurfaceContent::PullRequests => surface.id == surface.kind(),
                SurfaceContent::SideChat { thread_id } => {
                    surface.id == format!("side-chat:{}", thread_id.0)
                }
                SurfaceContent::PullRequest { number, .. } => {
                    *number > 0 && *number <= 9_007_199_254_740_991
                }
                SurfaceContent::Device { target, .. } => target
                    .as_ref()
                    .is_none_or(|t| matches!(t.platform.as_str(), "ios" | "android")),
                _ => true,
            },
        }
    }
    fn same_target(&self, other: &Self) -> bool {
        if self.thread() != other.thread() {
            return false;
        }
        match (self, other) {
            (Self::PanelTab { surface: a, .. }, Self::PanelTab { surface: b, .. }) => a.id == b.id,
            (Self::Browser { snapshot: a, .. }, Self::Browser { snapshot: b, .. }) => {
                a.tab_id == b.tab_id
            }
            _ => false,
        }
    }
}
#[derive(Debug, Clone, Default)]
pub struct ClosedViews {
    pub entries: Vec<Entry>,
}
impl ClosedViews {
    pub fn remember(&mut self, id: String, view: ClosedView) {
        self.entries.retain(|e| !e.view.same_target(&view));
        self.entries.insert(0, Entry { id, view });
        self.entries.truncate(20);
    }
    pub fn defer(&mut self, id: &str) {
        if let Some(i) = self.entries.iter().position(|e| e.id == id) {
            let e = self.entries.remove(i);
            self.entries.push(e);
        }
    }
    pub fn remove(&mut self, id: &str) {
        self.entries.retain(|e| e.id != id);
    }
    pub fn decode(raw: &str) -> Result<Self, serde_json::Error> {
        let v: serde_json::Value = serde_json::from_str(raw)?;
        let state = v.get("state").unwrap_or(&v);
        Ok(Self {
            entries: state["entries"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|e| serde_json::from_value::<Entry>(e.clone()).ok())
                .filter(|e| e.view.valid() && e.view.persistent())
                .collect(),
        })
    }
    pub fn encode(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(
            &serde_json::json!({"version":2,"state":{"entries":self.entries.iter().filter(|e|e.view.persistent()).collect::<Vec<_>>()}}),
        )
    }
}
pub struct OwnerState<'a> {
    pub environment_known: bool,
    pub catalog_ready: bool,
    pub owner_exists: bool,
    pub side_chat_exists: bool,
    pub shell_live: bool,
    pub panel: &'a Panel,
}
pub struct ReopenPlan<'a> {
    pub drop: Vec<&'a Entry>,
    pub restore: Option<&'a Entry>,
}
pub fn plan_next_reopen<'a, 'b>(
    entries: &'a [Entry],
    owner: impl Fn(&Entry) -> OwnerState<'b>,
) -> ReopenPlan<'a> {
    let mut drop = vec![];
    for entry in entries {
        let owner = owner(entry);
        if !owner.environment_known {
            if !owner.catalog_ready {
                break;
            }
            drop.push(entry);
            continue;
        }
        if !owner.owner_exists || !owner.side_chat_exists {
            if owner.shell_live {
                continue;
            }
            break;
        }
        let open=match &entry.view{ClosedView::PanelTab{surface,..}=>owner.panel.is_open&&owner.panel.surfaces.iter().any(|s|s.id==surface.id),ClosedView::Browser{snapshot,..}=>owner.panel.surfaces.iter().any(|s|matches!(&s.content,SurfaceContent::Preview{resource_id:Some(id)}if id==&snapshot.tab_id.0))};
        if open {
            drop.push(entry);
            continue;
        }
        return ReopenPlan {
            drop,
            restore: Some(entry),
        };
    }
    ReopenPlan {
        drop,
        restore: None,
    }
}
/// The browser callback returns its new server-assigned tab id only after a successful open.
pub async fn reopen<E>(
    view: &ClosedView,
    panels: &mut PanelStore,
    workspace_available: bool,
    open_browser: impl AsyncFnOnce(&PreviewSessionSnapshot) -> Result<String, E>,
) -> Result<bool, E> {
    let key = view.key();
    match view {
        ClosedView::Browser { snapshot, .. } => {
            let tab = open_browser(snapshot).await?;
            panels.apply(&key, Action::OpenBrowser(Some(tab)));
        }
        ClosedView::PanelTab { surface, .. } => {
            if !workspace_available
                && matches!(
                    surface.content,
                    SurfaceContent::Files { .. }
                        | SurfaceContent::File {
                            attachment: None,
                            ..
                        }
                )
            {
                return Ok(false);
            }
            let action = match &surface.content {
                SurfaceContent::Preview {
                    resource_id: Some(_),
                } => return Ok(false),
                SurfaceContent::Preview { resource_id: None } => Action::OpenBrowser(None),
                SurfaceContent::File {
                    relative_path,
                    reveal_line,
                    attachment,
                    folder_trail,
                    ..
                } => {
                    if let Some(attachment) = attachment {
                        Action::OpenAttachment(attachment.clone())
                    } else {
                        panels.apply(
                            &key,
                            Action::OpenFile {
                                path: relative_path.clone(),
                                line: reveal_line.map(|n| n as f64),
                            },
                        );
                        if let Some(trail) = folder_trail {
                            panels.apply(
                                &key,
                                Action::NavigateFolder {
                                    surface: surface.id.clone(),
                                    path: relative_path.clone(),
                                    trail: trail.clone(),
                                    keep_source: false,
                                },
                            );
                        }
                        return Ok(true);
                    }
                }
                SurfaceContent::Files { folder_trail } => {
                    let exists = panels
                        .by_thread
                        .get(panels.host_ref(&key))
                        .is_some_and(|p| p.surfaces.iter().any(|s| s.kind() == "files"));
                    panels.apply(&key, Action::Open("files".into()));
                    if !exists && let Some(trail) = folder_trail {
                        panels.apply(
                            &key,
                            Action::NavigateFolder {
                                surface: surface.id.clone(),
                                path: String::new(),
                                trail: trail.clone(),
                                keep_source: false,
                            },
                        );
                    }
                    return Ok(true);
                }
                SurfaceContent::Device {
                    target: Some(target),
                    title,
                } => {
                    panels.apply(
                        &key,
                        Action::OpenDevice {
                            target: target.clone(),
                            automatic: false,
                        },
                    );
                    if let Some(title) = title {
                        panels.apply(
                            &key,
                            Action::RenameDevice {
                                surface: surface.id.clone(),
                                title: title.clone(),
                            },
                        );
                    }
                    return Ok(true);
                }
                _ => Action::OpenSurface(surface.clone().without_reveal()),
            };
            panels.apply(&key, action);
        }
    }
    Ok(true)
}
pub fn pull_requests_search_for_restore(
    mut previous: serde_json::Value,
    selected: Option<&Surface>,
) -> serde_json::Value {
    let Some(object) = previous.as_object_mut() else {
        return previous;
    };
    for key in [
        "repository",
        "number",
        "selectedProjectId",
        "selectedHost",
        "selectedEnvironmentId",
    ] {
        object.remove(key);
    }
    for (key, default) in [("involvement", "all"), ("state", "open")] {
        if object.get(key).is_none_or(serde_json::Value::is_null) {
            object.insert(key.into(), default.into());
        }
    }
    if let Some(Surface {
        content:
            SurfaceContent::PullRequest {
                environment_id,
                project_id,
                host,
                repository,
                number,
                ..
            },
        ..
    }) = selected
    {
        object.insert("repository".into(), repository.clone().into());
        object.insert("number".into(), (*number).into());
        object.insert("selectedProjectId".into(), project_id.clone().into());
        if let Some(host) = host {
            object.insert("selectedHost".into(), host.clone().into());
        }
        if let Some(environment) = environment_id {
            object.insert("selectedEnvironmentId".into(), environment.clone().into());
        }
    }
    previous
}
