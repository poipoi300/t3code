//! Project folder controls for the grouped sidebar.

use std::{collections::BTreeMap, rc::Rc};

use super::{actions::ThreadActionMenu, row::RowThreadActions};
use crate::{
    i18n::{Value, t, tr},
    runtime::{Shared, tasks::TaskOwner},
    state::nav::{Navigator, Route},
    theme::{palette, tw},
    ui::{
        avatar::{ProjectFaviconProject, project_favicon},
        icon::{Icon, icon},
    },
};
use nsh_core::prelude::*;
use t3_client::{
    services::settings::scope::logical_project_scope_key,
    view_logic::shell::sidebar::{
        folder_expansion_keys, presentation::SidebarFolder, resolve_project_expanded,
    },
    workspace::projects::ProjectGroup,
};
use t3_contracts::ipc::ContextMenuItemSchema;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Preferences {
    pub folders: bool,
    pub working: bool,
    pub multiline: bool,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            folders: false,
            working: true,
            multiline: false,
        }
    }
}
impl From<&t3_contracts::settings::ClientSettingsSchema> for Preferences {
    fn from(settings: &t3_contracts::settings::ClientSettingsSchema) -> Self {
        Self {
            folders: settings.boring_sidebar_enabled,
            working: settings.sidebar_working_shelf_enabled && !settings.boring_sidebar_enabled,
            multiline: settings.boring_sidebar_multiline_rows,
        }
    }
}

pub(super) fn aliases(group: &ProjectGroup) -> Vec<String> {
    folder_expansion_keys(
        &logical_project_scope_key(&group.key),
        &group
            .members
            .iter()
            .map(|member| {
                (
                    member.physical_key().legacy_preference_key(),
                    member.project.workspace_root.0.clone(),
                )
            })
            .collect::<Vec<_>>(),
    )
}

pub(super) fn expanded(group: &ProjectGroup, expansion: &BTreeMap<String, bool>) -> bool {
    resolve_project_expanded(expansion, &aliases(group))
}

pub(super) fn header(
    folder: Memo<Option<Shared<SidebarFolder>>>,
    expansion: Signal<BTreeMap<String, bool>>,
    client: Option<Rc<dyn RowThreadActions>>,
    nav: Navigator,
    tasks: TaskOwner,
    state: Stored<Option<crate::runtime::services::shell::SidebarShelvesStore>>,
    hidden_count: Memo<usize>,
) -> impl IntoElement {
    let group = computed(move || {
        folder.with(|folder| folder.as_ref().map(|folder| folder.group.group.clone()))
    });
    let opened = computed(move || {
        group.with(|group| {
            group
                .as_ref()
                .is_some_and(|group| expansion.with(|expansion| expanded(group, expansion)))
        })
    });
    let label = move || {
        group.with(|group| {
            group
                .as_ref()
                .map(|group| group.label.clone())
                .unwrap_or_default()
        })
    };
    let can_pin = client.clone();
    let menu = computed(move || {
        let group = group.get()?;
        let mut items = vec![
            item(
                "new-thread",
                "New thread",
                Some("pencil"),
                can_pin.is_none(),
            ),
            item("copy-path", "Copy path", Some("copy"), false),
        ];
        if can_pin
            .as_ref()
            .is_some_and(|client| client.can_pin_project(&group.member_project_refs))
        {
            items.push(if group.pinned {
                item("unpin", "Unpin project", Some("pin-off"), false)
            } else {
                item("pin", "Pin project", Some("pin"), false)
            });
        }
        items.push(item(
            "project-settings",
            "Project settings",
            Some("settings"),
            false,
        ));
        Some(ThreadActionMenu {
            items,
            unavailable_reason: None,
        })
    });
    let show_menu = move |id: String| {
        let Some(group) = group.get_untracked() else {
            return;
        };
        match id.as_str() {
            "new-thread" => start(client.clone(), group.representative.key(), nav, tasks),
            "copy-path" => set_clipboard(group.representative.project.workspace_root.0),
            "project-settings" => {
                nav.open(Route::ProjectLink(logical_project_scope_key(&group.key)))
            }
            "pin" | "unpin" => {
                if let Some(client) = &client {
                    let op = client.pin_project(group.member_project_refs, id == "pin");
                    tasks.spawn(async move {
                        if let Err(error) = op.await {
                            super::actions::action_failed("project-pin", error);
                        }
                    });
                }
            }
            _ => {}
        }
    };
    let show_menu = stored(Rc::new(show_menu));
    let favicon = dynamic(move || {
        group.get().map(|group| {
            untracked(|| {
                let project = ProjectFaviconProject::from_shell(
                    group.representative.environment,
                    &group.representative.project,
                );
                project_favicon(move || project.clone(), |_| None, "size-4 shrink-0", None)
                    .into_element()
            })
        })
    });
    let trigger = view()
        .role(AccessibilityRole::Button)
        .focusable(true)
        .label(move || SharedString::from(label()))
        .accessibility_state(move || AccessibilityState { expanded: Some(opened.get()), ..Default::default() })
        .on_press(move || {
            let Some(group) = group.get_untracked() else { return; };
            let keys = aliases(&group);
            let next = !opened.get_untracked();
            expansion.update(|current| current.extend(keys.iter().cloned().map(|key| (key, next))));
            if let Some(store) = state.get() {
                store.save_project_expansion(keys, next);
            }
        })
        .app_class(tw("flex h-8 min-w-0 flex-1 items-center gap-2.5 rounded-[var(--control-radius)] px-2 text-left hover:bg-sidebar-row-hover"))
        .child(favicon)
        .child(text(label).app_class(tw("min-w-0 flex-1 truncate text-sm font-medium")))
        .child(when(move || !opened.get() && hidden_count.get() > 0, move || {
            let dot = computed(move || folder.with(|folder| folder.as_ref().and_then(|folder| folder.dot)));
            view().app_class(tw("flex items-center gap-1.5"))
                .child(when(move || dot.get().is_some(), move || {
                    view().role(AccessibilityRole::Text)
                        .label(move || t(match dot.get() {
                            Some("attention") => "Needs you",
                            Some("failed") => "Failed",
                            Some("unread") => "Done",
                            _ => "Working",
                        }))
                        .app_class(move || Some(match dot.get() {
                            Some("attention") => tw("size-1.5 rounded-full bg-warning"),
                            Some("failed") => tw("size-1.5 rounded-full bg-error"),
                            Some("unread") => tw("size-1.5 rounded-full bg-success"),
                            _ => tw("size-1.5 rounded-full bg-info"),
                        }))
                }))
                .child(text(move || hidden_count.get().to_string()).app_class(tw("text-xs tabular-nums text-muted-foreground")))
        }))
        .child(dynamic(move || icon(if opened.get() { Icon::ChevronDown } else { Icon::ChevronRight }, 12.0, move || palette().muted_foreground).into_element()));
    view().test_id("sidebar-project-folder").app_class(tw("flex w-full min-w-0 items-center gap-1"))
        .child(super::actions::sidebar_thread_context_menu(trigger, menu, move |id| show_menu.get()(id)))
        .child(nsh_base::button()
            .accessibility_label(move || Some(SharedString::from(tr("New thread in {project}", &[("project", Value::Text(label()))]))))
            .app_class(tw("flex size-7 shrink-0 items-center justify-center rounded-[var(--control-radius)] hover:bg-sidebar-row-hover"))
            .on_press(move || show_menu.get()("new-thread".into()))
            .child(icon(Icon::SquarePen, 14.0, move || palette().sidebar_icon)))
}

fn item(id: &str, label: &str, icon: Option<&str>, disabled: bool) -> ContextMenuItemSchema {
    ContextMenuItemSchema {
        id: id.into(),
        label: label.into(),
        icon: icon.map(str::to_owned),
        disabled: Some(disabled),
        detail: None,
        destructive: None,
        header: None,
        separator_before: None,
        children: None,
        checked: None,
    }
}

fn start(
    client: Option<Rc<dyn RowThreadActions>>,
    project: t3_client::workspace::ProjectKey,
    nav: Navigator,
    tasks: TaskOwner,
) {
    if let Some(client) = client {
        tasks.spawn(async move {
            let route = nav.top();
            match client.project_draft(project).await {
                Ok(draft) if nav.top() == route => nav.open(Route::Draft(draft)),
                Err(error) => super::actions::action_failed("new-thread-in-project", error),
                _ => {}
            }
        });
    }
}
