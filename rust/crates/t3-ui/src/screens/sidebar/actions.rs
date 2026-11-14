//! Port of apps/web/src/components/sidebar/SidebarThreadRow.tsx ThreadRenameInput.
//!
//! Context-menu items come from the client's port of threadActionMenu.logic.ts. Until the
//! runtime exposes that operation resource, `None` renders disabled actions with a reason.

use std::rc::Rc;

use nsh_core::{element::ImageSrc, keymap::When, prelude::*};
use t3_contracts::ipc::ContextMenuItemSchema;

use crate::i18n::{Value, t, tr};
use crate::theme::{Scope, cn, tw, tw_with};
use crate::ui::menu::{self, ItemOptions, MenuOptions, Variant};

/// The client supplies availability and already-authorized menu items; the UI never grants it.
#[derive(Clone, Debug, PartialEq)]
pub struct ThreadActionMenu {
    pub items: Vec<ContextMenuItemSchema>,
    pub unavailable_reason: Option<String>,
}

/// Port of Sidebar.tsx handleThreadContextMenu's api.contextMenu.show presentation.
/// NSH's shared context menu replaces the web/native imperative menu host.
pub fn sidebar_thread_context_menu(
    trigger: impl IntoElement,
    model: Memo<Option<ThreadActionMenu>>,
    on_action: impl Fn(String) + 'static,
) -> impl IntoElement {
    let on_action: Rc<dyn Fn(String)> = Rc::new(on_action);
    menu::context_menu(
        trigger,
        // UI-MENU owns popup sizing; the web fallback's sizing preset is requested in the hand-off.
        MenuOptions::default(),
        || false,
        menu::menu_popup_with(move || {
            let on_action = on_action.clone();
            dynamic(move || {
                let current = model.get();
                let items = current
                    .as_ref()
                    .map(|model| model.items.clone())
                    .unwrap_or_else(unavailable_items);
                let reason = current
                    .as_ref()
                    .and_then(|model| model.unavailable_reason.as_deref())
                    .map(t)
                    .or_else(|| {
                        current
                            .is_none()
                            .then(|| t("Thread actions are not available on this client."))
                    });
                view()
                    .child(reason.map(|reason| menu::menu_group_label(reason, false)))
                    .children(
                        items
                            .into_iter()
                            .map(|item| action_item(item, model, on_action.clone(), false)),
                    )
            })
        }),
    )
}

fn unavailable_items() -> Vec<ContextMenuItemSchema> {
    [
        ("pin", "Pin thread", false, false),
        ("rename", "Rename thread", true, false),
        ("archive", "Archive thread", true, false),
        ("delete", "Delete", false, true),
    ]
    .into_iter()
    .map(
        |(id, label, separator, destructive)| ContextMenuItemSchema {
            id: id.into(),
            label: label.into(),
            detail: None,
            destructive: Some(destructive),
            disabled: Some(true),
            header: None,
            icon: Some(
                match id {
                    "rename" => "pencil",
                    "delete" => "trash",
                    _ => id,
                }
                .into(),
            ),
            separator_before: Some(separator),
            checked: None,
            children: None,
        },
    )
    .collect()
}

fn enabled(items: &[ContextMenuItemSchema], id: &str, parent_disabled: bool) -> bool {
    items.iter().any(|item| {
        let disabled = parent_disabled || item.disabled == Some(true) || item.header == Some(true);
        if item.id == id {
            !disabled && item.children.is_none()
        } else {
            item.children
                .as_ref()
                .is_some_and(|children| enabled(children, id, disabled))
        }
    })
}

fn action_item(
    item: ContextMenuItemSchema,
    model: Memo<Option<ThreadActionMenu>>,
    on_action: Rc<dyn Fn(String)>,
    parent_disabled: bool,
) -> AnyElement {
    let id = item.id.clone();
    let variant = if item.destructive == Some(true) {
        Variant::Destructive
    } else {
        Variant::Default
    };
    let disabled = parent_disabled || item.disabled == Some(true);
    let options = ItemOptions {
        variant,
        disabled: Prop::bind(move || {
            disabled
                || model.with(|model| {
                    model
                        .as_ref()
                        .is_none_or(|model| model.unavailable_reason.is_some())
                })
        }),
        ..Default::default()
    };
    let slots = item.clone();
    let content = dynamic(move || {
        let mut children = Vec::new();
        if slots.checked.is_none()
            && let Some(path) = slots.icon.as_deref().and_then(action_icon)
        {
            children.push(
                glyph(path)
                    .app_class(tw(&cn([
                        Some("size-4.5 shrink-0 sm:size-4"),
                        (variant != Variant::Destructive).then_some("text-muted-foreground"),
                    ])))
                    .into_element(),
            );
        }
        children.push(menu::menu_item_label(t(&slots.label)).into_element());
        if let Some(detail) = &slots.detail {
            children.push(menu::menu_item_detail(t(detail)).into_element());
        }
        children
    });
    let row = if item.header == Some(true) {
        menu::menu_group_label(t(&item.label), false).into_element()
    } else if let Some(children) = item.children {
        menu::menu_sub(
            menu::menu_sub_trigger(t(&item.label), options, content),
            move || options.disabled.get(),
            menu::menu_sub_popup(
                view().children(
                    children
                        .into_iter()
                        .map(|child| action_item(child, model, on_action.clone(), disabled)),
                ),
            ),
        )
        .into_element()
    } else {
        let activate = move || {
            // Re-read the client resource at activation: a permission or connection may change.
            if model.with(|model| {
                model.as_ref().is_some_and(|model| {
                    model.unavailable_reason.is_none() && enabled(&model.items, &id, false)
                })
            }) {
                on_action(id.clone());
            }
        };
        if let Some(checked) = item.checked {
            // UI-MENU supplies its checked glyph; a fallback-specific indicator needs a primitive slot.
            menu::menu_checkbox_item(
                t(&item.label),
                move || checked,
                menu::CheckboxVariant::Icon,
                options,
                content,
                move |_| activate(),
            )
            .into_element()
        } else {
            menu::menu_item(t(&item.label), options, content, activate).into_element()
        }
    };
    view()
        .child((item.separator_before == Some(true)).then(menu::menu_separator))
        .child(row)
        .into_element()
}

/// Mounted for one rename session, matching the source's settledRef lifetime.
pub fn thread_rename_input(
    title: Signal<String>,
    on_commit: impl Fn() + 'static,
    on_cancel: impl Fn() + 'static,
) -> impl IntoElement {
    let settled = signal(false);
    let composing = signal(false);
    let node = element_ref();
    let focused = signal(false);
    let cancel: Rc<dyn Fn()> = Rc::new(on_cancel);
    let commit: Rc<dyn Fn()> = Rc::new(on_commit);
    let submit = commit.clone();
    // NSH's raw key listener is not cross-platform; its focused keymap handles Escape.
    effect(move || {
        if focused.get() && !composing.get() {
            let cancel = cancel.clone();
            on_key_with(
                "escape",
                KeyOptions {
                    when: When::Always,
                    ..Default::default()
                },
                move |_| {
                    if !settled.get_untracked() {
                        settled.set(true);
                        cancel();
                    }
                },
            );
        }
    });
    nsh_base::input()
        .element_ref(&node)
        .auto_focus(true)
        .value(move || title.get())
        .label(t("Thread title"))
        .on_change(move |value: &str| title.set(value.to_owned()))
        .on_focus(move || {
            focused.set(true);
            node.set_selection(0, title.get_untracked().encode_utf16().count() as u32);
        })
        .on_composition_change(move |event: &Composition| composing.set(event.composing))
        .submit_behavior(SubmitBehavior::Submit)
        .on_submit(move |_: &str| {
            if !composing.get_untracked() && !settled.get_untracked() {
                settled.set(true);
                submit();
            }
        })
        .on_blur(move || {
            focused.set(false);
            if !settled.get_untracked() {
                commit();
            }
        })
        .app_class(tw("min-w-0 flex-1 rounded-sm border border-input bg-card px-1 text-sm font-medium text-card-foreground outline-none focus:border-foreground"))
}

/// Client-owned notice projection; no action is undoable merely because this view exists.
#[derive(Clone, Debug, PartialEq)]
pub struct ThreadUndoNotice {
    pub action: String,
    pub count: usize,
    pub shortcut: Option<String>,
}

/// Port of apps/web/src/components/sidebar/SidebarThreadUndoNotice.tsx SidebarThreadUndoNotice.
pub fn sidebar_thread_undo_notice(
    notice: Memo<Option<ThreadUndoNotice>>,
    on_undo: impl Fn() + 'static,
) -> impl IntoElement {
    let on_undo: Rc<dyn Fn()> = Rc::new(on_undo);
    let width = signal(0.0_f32);
    dynamic(move || {
        let on_undo = on_undo.clone();
        notice.get().map(|notice_value| {
            let summary = if notice_value.count == 1 {
                t(&notice_value.action)
            } else {
                tr("{action} {count}", &[
                    ("action", Value::Text(t(&notice_value.action))),
                    ("count", Value::Int(notice_value.count as i64)),
                ])
            };
            let label = notice_value.shortcut.map_or_else(|| t("Undo"), |shortcut| {
                tr("{shortcut} to undo", &[("shortcut", Value::Text(shortcut))])
            });
            // NSH has no transitionend/starting: presence ends immediately, including accessibility.
            view()
                .on_layout(move |bounds: &Bounds<Points>| width.set(bounds.size.width.0))
                .app_class(tw_with(Scope::Sidebar, "flex min-w-0 flex-1", &[]))
                .child(view().app_class(tw_with(Scope::Sidebar,
                    "flex h-6 min-w-0 flex-1 flex-row-reverse flex-wrap gap-x-1 overflow-hidden text-2xs leading-6 whitespace-nowrap text-sidebar-muted-foreground transition-opacity duration-200 ease-out", &[]))
                    // InlineButton is not exposed by the frozen button primitive; request in hand-off.
                    .child(nsh_base::button()
                        .accessibility_label(label.clone())
                        .disabled(move || notice.get().is_none())
                        .on_press(move || { if notice.get_untracked().is_some() { on_undo(); } })
                        .app_class(tw("inline-flex shrink-0 cursor-pointer items-center gap-0.5 font-medium underline-offset-2 text-center whitespace-nowrap hover:underline focus-visible:outline-2 focus-visible:outline-ring disabled:pointer-events-none disabled:opacity-64 text-foreground min-w-0 shrink"))
                        .child(view().app_class(tw("flex h-6 min-w-0 flex-wrap justify-end overflow-hidden"))
                            // @max-[1rem] reads the SidebarMenuItem container, measured above.
                            .child(glyph(UNDO).app_class(move || tw(&cn([Some("-ml-4 h-6 w-4"), (width.get() <= 16.0).then_some("invisible")]))))
                            .child(text(label))))
                    .child(text(tr("{summary},", &[("summary", Value::Text(summary))]))))
        })
    })
}

fn glyph(path: &'static str) -> nsh_core::element::Svg {
    svg(ImageSrc::svg_markup(&format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">{path}</svg>"#)))
        .accessible(false)
}

// Port of apps/web/src/contextMenuFallback.ts ICON_PATHS.
fn action_icon(name: &str) -> Option<&'static str> {
    match name {
        "archive" => Some(
            r#"<rect width="20" height="5" x="2" y="3" rx="1"/><path d="M4 8v11a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8"/><path d="M10 12h4"/>"#,
        ),
        "check" => Some(r#"<path d="M20 6 9 17l-5-5"/>"#),
        "timer" => Some(
            r#"<line x1="10" x2="14" y1="2" y2="2"/><line x1="12" x2="15" y1="14" y2="11"/><circle cx="12" cy="14" r="8"/>"#,
        ),
        "chevron-right" => Some(r#"<path d="m9 19 7-7-7-7"/>"#),
        "circle-check" => Some(r#"<circle cx="12" cy="12" r="10"/><path d="m9 12 2 2 4-4"/>"#),
        "clock" => Some(r#"<path d="M12 6v6l4 2"/><circle cx="12" cy="12" r="10"/>"#),
        "pencil" => Some(
            r#"<path d="M21.174 6.812a1 1 0 0 0-3.986-3.987L3.842 16.174a2 2 0 0 0-.5.83l-1.321 4.352a.5.5 0 0 0 .623.622l4.353-1.32a2 2 0 0 0 .83-.497z"/><path d="m15 5 4 4"/>"#,
        ),
        "copy" => Some(
            r#"<rect width="14" height="14" x="8" y="8" rx="2" ry="2"/><path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/>"#,
        ),
        "folder" => Some(
            r#"<path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"/>"#,
        ),
        "git-branch" => Some(
            r#"<line x1="6" x2="6" y1="3" y2="15"/><circle cx="18" cy="6" r="3"/><circle cx="6" cy="18" r="3"/><path d="M18 9a9 9 0 0 1-9 9"/>"#,
        ),
        "hash" => Some(
            r#"<line x1="4" x2="20" y1="9" y2="9"/><line x1="4" x2="20" y1="15" y2="15"/><line x1="10" x2="8" y1="3" y2="21"/><line x1="16" x2="14" y1="3" y2="21"/>"#,
        ),
        "mail-open" => Some(
            r#"<path d="M21.2 8.4c.5.38.8.97.8 1.6v10a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V10a2 2 0 0 1 .8-1.6l8-6a2 2 0 0 1 2.4 0l8 6Z"/><path d="m22 10-8.97 5.7a1.94 1.94 0 0 1-2.06 0L2 10"/>"#,
        ),
        "message-square" => Some(
            r#"<path d="M22 17a2 2 0 0 1-2 2H6.828a2 2 0 0 0-1.414.586l-2.202 2.202A.71.71 0 0 1 2 21.286V5a2 2 0 0 1 2-2h16a2 2 0 0 1 2 2z"/>"#,
        ),
        "message-square-plus" => Some(
            r#"<path d="M22 17a2 2 0 0 1-2 2H6.828a2 2 0 0 0-1.414.586l-2.202 2.202A.71.71 0 0 1 2 21.286V5a2 2 0 0 1 2-2h16a2 2 0 0 1 2 2z"/><path d="M12 8v6"/><path d="M9 11h6"/>"#,
        ),
        "pin" => Some(
            r#"<path d="M12 17v5"/><path d="M9 10.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24V16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-.76a2 2 0 0 0-1.11-1.79l-1.78-.9A2 2 0 0 1 15 10.76V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H8a2 2 0 0 0 0 4 1 1 0 0 1 1 1z"/>"#,
        ),
        "pin-off" => Some(
            r#"<path d="M12 17v5"/><path d="M15 9.34V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H7.89"/><path d="m2 2 20 20"/><path d="M9 9v1.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24V16a1 1 0 0 0 1 1h11"/>"#,
        ),
        "refresh-cw" => Some(
            r#"<path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/><path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16"/><path d="M8 16H3v5"/>"#,
        ),
        "settings" => Some(
            r#"<path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.38a2 2 0 0 0-.73-2.73l-.15-.09a2 2 0 0 1-1-1.74v-.51a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z"/><circle cx="12" cy="12" r="3"/>"#,
        ),
        "user-plus" => Some(
            r#"<path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2"/><circle cx="9" cy="7" r="4"/><line x1="19" x2="19" y1="8" y2="14"/><line x1="22" x2="16" y1="11" y2="11"/>"#,
        ),
        "folder-plus" => Some(
            r#"<path d="M12 10v6"/><path d="M9 13h6"/><path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"/>"#,
        ),
        "folder-tree" => Some(
            r#"<path d="M20 10a1 1 0 0 0 1-1V6a1 1 0 0 0-1-1h-2.5a1 1 0 0 1-.8-.4l-.9-1.2A1 1 0 0 0 15 3h-2a1 1 0 0 0-1 1v5a1 1 0 0 0 1 1Z"/><path d="M20 21a1 1 0 0 0 1-1v-3a1 1 0 0 0-1-1h-2.9a1 1 0 0 1-.88-.55l-.42-.85a1 1 0 0 0-.92-.6H13a1 1 0 0 0-1 1v5a1 1 0 0 0 1 1Z"/><path d="M3 5a2 2 0 0 0 2 2h3"/><path d="M3 3v13a2 2 0 0 0 2 2h3"/>"#,
        ),
        "text-search" => Some(
            r#"<path d="M21 5H3"/><path d="M10 12H3"/><path d="M10 19H3"/><circle cx="17" cy="15" r="3"/><path d="m21 19-1.9-1.9"/>"#,
        ),
        "trash" => Some(
            r#"<path d="M3 6h18"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6"/><path d="M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/><line x1="10" x2="10" y1="11" y2="17"/><line x1="14" x2="14" y1="11" y2="17"/>"#,
        ),
        _ => None,
    }
}

const UNDO: &str = r#"<path d="M9 14 4 9l5-5"/><path d="M4 9h10.5a5.5 5.5 0 0 1 0 11H11"/>"#;
