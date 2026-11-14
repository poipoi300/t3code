//! Port of apps/web/src/components/chat/OpenInPicker.tsx OpenInPicker with displayMode="panel".

use std::rc::Rc;

use futures::{future::LocalBoxFuture, stream::LocalBoxStream};
use nsh_core::prelude::*;
use t3_client::ClientError;
use t3_client::workspace::git_panel::editors::{EditorPickerSnapshot, RemoteOpenState};
use t3_contracts::editor::EditorId;

use crate::i18n::{Value, t, tr};
use crate::screens::panel::git_actions::thread_details_control;
use crate::theme::tw;
use crate::ui::icon::{Icon, icon_with_class};
use crate::ui::menu::{self, ItemOptions, MenuOptions};
use crate::ui::tooltip::{Variant as TooltipVariant, tooltip_popup};

/// The client's `EditorPicker`, bound by delegation in `runtime/services/git_panel.rs`.
pub trait EditorPickerBackend {
    /// The current snapshot first, then after every change.
    fn watch(&self) -> LocalBoxStream<'static, Result<EditorPickerSnapshot, ClientError>>;
    fn select_editor(&self, editor: EditorId) -> LocalBoxFuture<'static, Result<(), ClientError>>;
    fn open_editor(&self, editor: EditorId) -> LocalBoxFuture<'static, Result<bool, ClientError>>;
}

/// `resolveOpenInOptions`'s icons. The catalogue has none of the editor brand marks (Cursor, VS
/// Code, Zed, JetBrains, Finder, File Explorer: requested from UI-ICONS); only the generic file
/// manager's FolderClosedIcon exists, and the panel falls back to its generic open mark.
fn editor_icon(editor: EditorId, label: &str) -> Option<Icon> {
    (editor == EditorId::FileManager && label == "Files").then_some(Icon::FolderClosed)
}

fn preferred_label(snapshot: &EditorPickerSnapshot) -> Option<String> {
    let preferred = snapshot.preferred?;
    snapshot
        .options
        .iter()
        .find(|o| o.editor == preferred)
        .map(|o| o.label.clone())
}

/// `primaryLabel` in panel mode.
fn primary_label(snapshot: &EditorPickerSnapshot) -> String {
    match preferred_label(snapshot) {
        Some(label) => tr("Open in {editor}", &[("editor", Value::Text(label))]),
        None => t("Open in editor"),
    }
}

/// `isHostEditorDenied`: a local host this connection may not operate. The client folds the
/// operate grant into `can_open`, which also needs a target and an editor.
fn host_editor_denied(snapshot: &EditorPickerSnapshot) -> bool {
    snapshot.remote == RemoteOpenState::LocalExec
        && snapshot.target_path.is_some()
        && snapshot.preferred.is_some()
        && !snapshot.can_open
}

/// `canOpenEditor`.
fn can_open_editor(snapshot: &EditorPickerSnapshot) -> bool {
    snapshot.remote != RemoteOpenState::RemoteUnavailable && !host_editor_denied(snapshot)
}

/// `unavailableReason`.
fn unavailable_reason(snapshot: &EditorPickerSnapshot) -> Option<String> {
    let environment = Value::Text(snapshot.environment_label.clone());
    match snapshot.remote {
        RemoteOpenState::RemoteUnavailable => {
            Some(tr("No SSH route to {environment}", &[("environment", environment)]))
        }
        RemoteOpenState::RemoteLinks { .. } if snapshot.options.is_empty() => Some(tr(
            "Open from a computer to edit on {environment}",
            &[("environment", environment)],
        )),
        _ => None,
    }
}

pub(super) fn open_in_picker(
    backend: Stored<Rc<dyn EditorPickerBackend>>,
    snapshot: Signal<Option<Rc<EditorPickerSnapshot>>>,
) -> impl IntoElement {
    let menu_open = signal(false);
    let current = move || snapshot.get().unwrap_or_else(empty_snapshot);
    let label = computed(move || primary_label(&current()));
    // Retained details content closes its menu when the host can no longer operate the editor.
    effect(move || {
        if host_editor_denied(&current()) {
            menu_open.set(false);
        }
    });
    view()
        .role(AccessibilityRole::Group)
        .label(t("Open in editor"))
        .app_class(move || {
            Some(tw(if menu_open.get() {
                "group/details-split flex w-full items-center rounded-lg hover:!bg-black/[0.035] dark:hover:!bg-white/[0.08] bg-black/[0.035] dark:bg-white/[0.08]"
            } else {
                "group/details-split flex w-full items-center rounded-lg hover:!bg-black/[0.035] dark:hover:!bg-white/[0.08]"
            }))
        })
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .tooltip(tooltip_popup(TooltipVariant::Default, text(move || label.get())).into_element())
                .child(thread_details_control(
                    "primary",
                    Prop::bind(move || label.get().into()),
                    Prop::bind(move || !current().can_open),
                    false,
                    view()
                        .app_class(tw("flex min-w-0 items-center gap-2.5"))
                        .child(dynamic(move || {
                            let snapshot = current();
                            // The panel's generic mark when no editor is chosen or its brand mark
                            // is missing from the catalogue (UI-ICONS).
                            let icon = snapshot
                                .preferred
                                .zip(preferred_label(&snapshot))
                                .and_then(|(editor, label)| editor_icon(editor, &label))
                                .unwrap_or(Icon::SquareArrowOutUpRight);
                            Some(icon_with_class(icon, "size-4 shrink-0 text-muted-foreground"))
                        }))
                        .child(text(move || label.get()).app_class(tw("ml-0 min-w-0 truncate"))),
                    move || {
                        let snapshot = current();
                        let (Some(editor), Some(name)) = (snapshot.preferred, preferred_label(&snapshot)) else {
                            return;
                        };
                        let backend = backend.get();
                        spawn(async move {
                            if let Ok(true) = backend.open_editor(editor).await {
                                announce_for_accessibility(tr("Opened in {editor}", &[("editor", Value::Text(name))]));
                            }
                        });
                    },
                )),
        )
        .child(view().app_class(move || {
            Some(tw(if menu_open.get() {
                "h-4 w-px shrink-0 bg-border/65"
            } else {
                "h-4 w-px shrink-0 bg-transparent group-hover/details-split:bg-border/65"
            }))
        }))
        .child(
            view()
                .tooltip(tooltip_popup(TooltipVariant::Default, text(t("Choose editor"))).into_element())
                .child(menu::menu(
                    thread_details_control(
                        "secondary",
                        t("Choose editor").into(),
                        Prop::bind(move || host_editor_denied(&current())),
                        false,
                        icon_with_class(Icon::ChevronDown, "size-4 shrink-0 text-muted-foreground"),
                        || {},
                    ),
                    MenuOptions {
                        open: Some(menu_open),
                        align: Align::End,
                        ..Default::default()
                    },
                    menu::menu_popup_with(move || editor_items(backend, current())),
                )),
        )
}

/// Before the first snapshot: nothing to open and no menu reason to show.
fn empty_snapshot() -> Rc<EditorPickerSnapshot> {
    Rc::new(EditorPickerSnapshot {
        options: Vec::new(),
        preferred: None,
        remote: RemoteOpenState::LocalExec,
        can_open: false,
        unavailable_reason: None,
        hint_seen: true,
        shortcut: None,
        target_path: None,
        environment_label: String::new(),
    })
}

/// `editorItems` with the toolbar presentation: choosing an item sets the preferred editor.
fn editor_items(
    backend: Stored<Rc<dyn EditorPickerBackend>>,
    snapshot: Rc<EditorPickerSnapshot>,
) -> AnyElement {
    if let Some(reason) = unavailable_reason(&snapshot) {
        return disabled_item(reason).into_element();
    }
    let enabled = snapshot.target_path.is_some() && can_open_editor(&snapshot);
    let mut items: Vec<AnyElement> = Vec::new();
    if snapshot.options.is_empty() {
        items.push(disabled_item(t("No installed editors found")).into_element());
    }
    for option in &snapshot.options {
        let editor = option.editor;
        let shortcut = (Some(editor) == snapshot.preferred)
            .then(|| snapshot.shortcut.clone())
            .flatten();
        items.push(
            menu::menu_item(
                option.label.clone(),
                ItemOptions {
                    disabled: (!enabled).into(),
                    ..Default::default()
                },
                view()
                    .app_class(tw("flex w-full items-center gap-2"))
                    .child(editor_icon(editor, &option.label).map(|icon| {
                        icon_with_class(icon, if editor == EditorId::FileManager { "size-4 text-muted-foreground" } else { "size-4 text-foreground opacity-100" })
                    }))
                    .child(menu::menu_item_label(option.label.clone()))
                    .child(shortcut.map(menu::menu_shortcut)),
                move || {
                    let backend = backend.get();
                    spawn(async move {
                        let _ = backend.select_editor(editor).await;
                    });
                },
            )
            .into_element(),
        );
    }
    if matches!(snapshot.remote, RemoteOpenState::RemoteLinks { .. }) && !snapshot.hint_seen {
        items.push(
            disabled_item(tr(
                "Opens over SSH. Needs your key on {environment}",
                &[("environment", Value::Text(snapshot.environment_label.clone()))],
            ))
            .into_element(),
        );
    }
    view().child(items).into_element()
}

fn disabled_item(label: String) -> impl IntoElement {
    menu::menu_item(
        label.clone(),
        ItemOptions {
            disabled: true.into(),
            ..Default::default()
        },
        menu::menu_item_label(label),
        || {},
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use t3_client::workspace::git_panel::editors::EditorOption;

    fn snapshot(remote: RemoteOpenState, can_open: bool) -> EditorPickerSnapshot {
        EditorPickerSnapshot {
            options: vec![EditorOption {
                editor: EditorId::Vscode,
                label: "VS Code".into(),
                icon_identity: "vscode".into(),
            }],
            preferred: Some(EditorId::Vscode),
            remote,
            can_open,
            unavailable_reason: None,
            hint_seen: false,
            shortcut: None,
            target_path: Some("/repo".into()),
            environment_label: "Studio".into(),
        }
    }

    #[test]
    fn host_denial_and_unavailable_routes_follow_open_in_picker() {
        let local = snapshot(RemoteOpenState::LocalExec, true);
        assert!(can_open_editor(&local));
        assert_eq!(primary_label(&local), "Open in VS Code");
        let denied = snapshot(RemoteOpenState::LocalExec, false);
        assert!(host_editor_denied(&denied));
        assert!(!can_open_editor(&denied));
        let unreachable = snapshot(RemoteOpenState::RemoteUnavailable, false);
        assert!(!host_editor_denied(&unreachable));
        assert_eq!(unavailable_reason(&unreachable), Some("No SSH route to Studio".into()));
        let mut links = snapshot(
            RemoteOpenState::RemoteLinks { host: "studio".into(), username: None },
            true,
        );
        assert_eq!(unavailable_reason(&links), None);
        links.options.clear();
        links.preferred = None;
        assert_eq!(primary_label(&links), "Open in editor");
        assert_eq!(
            unavailable_reason(&links),
            Some("Open from a computer to edit on Studio".into())
        );
    }
}
