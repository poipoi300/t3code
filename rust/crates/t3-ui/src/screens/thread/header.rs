//! Port of apps/web/src/components/chat/ChatHeader.tsx ChatHeader.

use std::rc::Rc;

use nsh_core::prelude::*;
use t3_client::types::{EnvironmentId, ThreadKey};
use t3_client::view_logic::session::text::{TextDirection, text_direction};
use t3_contracts::orchestration_project::OrchestrationProjectShell;
use t3_contracts::orchestration_v2::AppThreadLineageRelationshipToParent;

use crate::i18n::{Text as I18nText, Value, t, tr};
use crate::runtime::{Shared, rt};
use crate::screens::panel::{Action, PanelHost};
use crate::state::layout::layout_class;
use crate::state::nav::{Route, navigator};
use crate::theme::tw;
use crate::ui::avatar::{ProjectFaviconProject, project_favicon};
use crate::ui::badge;
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};
use crate::ui::icon::{Icon, icon_with_class};
use crate::ui::toggle::{Content, Size, Variant, toggle};
use crate::ui::tooltip::{TooltipOptions, tooltip, tooltip_create_handle};

/// Canvas mode and TERM-1's controlled drawer binding, provided by the workspace owner.
/// The header never creates a second panel or terminal visibility store.
#[derive(Clone)]
pub struct HeaderLayout {
    pub details_inline: Prop<bool>,
    pub terminal: Option<TerminalControl>,
}

#[derive(Clone)]
pub struct TerminalControl {
    pub open: Prop<bool>,
    pub available: Prop<bool>,
    pub on_toggle: Rc<dyn Fn()>,
}

#[derive(Clone, PartialEq)]
struct Breadcrumb {
    title: String,
    project: Option<Rc<OrchestrationProjectShell>>,
    parent: Option<(ThreadKey, String)>,
    origin: Option<String>,
}

/// Compatibility with the slice's title-only caller; canonical rows come from its route.
pub fn header(titles: Memo<Option<(String, Option<String>)>>) -> impl IntoElement {
    let nav = navigator();
    let key = computed(move || nav.top().thread().cloned());
    view().app_class(tw("shrink-0")).child(dynamic(move || {
        let key = key.get();
        untracked(|| match key {
            Some(key) => thread_header(key).into_element(),
            None => text(move || {
                titles.get().map_or_else(
                    || t("Loading thread…").into(),
                    |(title, _)| SharedString::from(title),
                )
            })
            .into_element(),
        })
    }))
}

pub fn thread_header(key: ThreadKey) -> impl IntoElement {
    let runtime = rt();
    let nav = navigator();
    let shell = runtime.store.shell(key.environment.clone());
    let thread = key.thread.clone();
    let environment = key.environment.clone();
    let breadcrumb = computed(move || {
        shell.with(|shell| {
            let shell = shell.as_ref()?;
            let row = shell.threads.iter().find(|row| row.id == thread)?;
            let project = shell
                .projects
                .iter()
                .find(|p| p.id == row.project_id)
                .cloned();
            let parent = (row.lineage.relationship_to_parent
                == Some(AppThreadLineageRelationshipToParent::Subagent))
            .then_some(row.lineage.parent_thread_id.as_ref())
            .flatten()
            .map(|id| {
                let title = shell
                    .threads
                    .iter()
                    .chain(shell.archived_threads.iter())
                    .find(|row| &row.id == id)
                    .map_or_else(|| t("Parent thread"), |row| row.title.clone());
                (
                    ThreadKey {
                        environment: environment.clone(),
                        thread: id.clone(),
                    },
                    title,
                )
            });
            Some(Breadcrumb {
                title: row.title.clone(),
                project,
                parent,
                origin: row
                    .link_origin
                    .as_ref()
                    .and_then(Option::as_ref)
                    .map(|origin| origin.label.clone())
                    .filter(|label| !label.is_empty()),
            })
        })
    });
    let has_project =
        computed(move || breadcrumb.with(|b| b.as_ref().is_some_and(|b| b.project.is_some())));
    view()
        // Electron drag/window reservations belong to the shell; no native overflow-clip-margin.
        .app_class(tw("relative bg-background flex h-[var(--workspace-topbar-height)] min-h-[var(--workspace-topbar-height)] shrink-0 items-center px-3 sm:px-5"))
        .child(when(
            move || layout_class().is_compact() || has_thread_below(&nav.stack.get()),
            move || button(
                ButtonContent::Icon { icon: Icon::ArrowLeft, label: t("Back") },
                ButtonVariant::Ghost, ButtonSize::Icon, || false, move || nav.back(),
            ),
        ))
        .child(chat_header(breadcrumb, key.environment.clone()))
        // The web fixes this group to window chrome; the shared header is its native anchor.
        .child(panel_layout_controls(key, has_project))
}

/// Port of mobile/components/ui/AppBar.tsx's threadBelow condition (095611d1b5).
fn has_thread_below(stack: &[Route]) -> bool {
    matches!(stack, [.., Route::Thread(_), Route::Thread(_)])
}

fn chat_header(
    breadcrumb: Memo<Option<Breadcrumb>>,
    environment: EnvironmentId,
) -> impl IntoElement {
    let nav = navigator();
    let project = computed(move || {
        breadcrumb.with(|b| b.as_ref().and_then(|b| b.project.clone()).map(Shared))
    });
    let parent = computed(move || breadcrumb.with(|b| b.as_ref().and_then(|b| b.parent.clone())));
    let origin = computed(move || breadcrumb.with(|b| b.as_ref().and_then(|b| b.origin.clone())));
    view()
        // Sibling controls reserve their actual width instead of the web's pr-10/pr-24.
        .app_class(tw("flex min-w-0 flex-1 items-center gap-2 sm:gap-3"))
        .child(view()
            .role(AccessibilityRole::Navigation)
            .label(move || t("Thread breadcrumb"))
            .app_class(tw("min-w-0 flex-1 overflow-clip"))
            // Native lists/text omit list-none, text-box and its supports padding hint.
            .child(view()
                .app_class(tw("m-0 flex min-w-0 items-center gap-2 p-0 text-sm sm:gap-3"))
                .child(dynamic(move || {
                    let project = project.get();
                    let environment = environment.clone();
                    project.map(|project| untracked(|| {
                        let project = ProjectFaviconProject::from_shell(environment, &project);
                        let name = project.title.clone();
                        vec![
                            view()
                                .app_class(tw("flex min-w-0 items-center font-medium shrink-0 text-muted-foreground shrink"))
                                .tooltip(tooltip(tooltip_create_handle(), TooltipOptions::default(), text(SharedString::from(tr("New thread in {project}", &[("project", Value::Text(name.clone()))])))))
                                // MISC-NEW/runtime must expose project-scoped creation before this enables.
                                .child(nsh_base::button()
                                    .disabled(true)
                                    .accessibility_label(Some(SharedString::from(tr("New thread in {project}", &[("project", Value::Text(name.clone()))]))))
                                    .app_class(tw("inline-flex min-w-0 max-w-full cursor-pointer items-center gap-1.5 rounded-sm text-muted-foreground transition-colors hover:text-foreground focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"))
                                    // UI-AVATAR's cached resource reader awaits runtime integration.
                                    .child(project_favicon(move || project.clone(), |_| None, "size-3.5", None))
                                    .child(text(SharedString::from(name)).app_class(tw("block min-w-0 truncate max-w-40"))))
                                .into_element(),
                            workspace_breadcrumb_separator().into_element(),
                        ]
                    })).unwrap_or_default()
                }))
                .child(dynamic(move || {
                    let parent = parent.get();
                    parent.map(|(key, title)| untracked(|| vec![
                        view()
                            .app_class(tw("flex min-w-0 items-center font-medium shrink-0 text-muted-foreground"))
                            .tooltip(tooltip(tooltip_create_handle(), TooltipOptions::default(), text(SharedString::from(title.clone()))))
                            .child(nsh_base::button()
                                .accessibility_label(Some(SharedString::from(tr("Open parent thread: {title}", &[("title", Value::Text(title.clone()))]))))
                                .on_press(move || nav.open(Route::Thread(key.clone())))
                                .app_class(tw("inline-flex min-w-0 max-w-full cursor-pointer items-center rounded-sm text-muted-foreground transition-colors hover:text-foreground focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"))
                                .child(text(SharedString::from(title)).app_class(tw("block min-w-0 truncate"))))
                            .into_element(),
                        workspace_breadcrumb_separator().into_element(),
                    ])).unwrap_or_default()
                }))
                .child(view()
                    .app_class(tw("flex min-w-0 items-center font-medium text-foreground min-w-10 flex-1"))
                    // Backend has no title-menu/rename capability yet.
                    .child(view()
                        .role(AccessibilityRole::Header)
                        .accessibility_level(2u32)
                        .app_class(tw("min-w-0 flex-1"))
                        .direction(move || breadcrumb.with(|b| match b.as_ref().and_then(|b| text_direction(&b.title)) {
                            Some(TextDirection::Ltr) => LayoutDirection::Ltr,
                            Some(TextDirection::Rtl) => LayoutDirection::Rtl,
                            None => LayoutDirection::Inherit,
                        }))
                        .tooltip(tooltip(tooltip_create_handle(), TooltipOptions::default(), text(move || breadcrumb.with(|b| b.as_ref().map_or_else(|| t("Loading thread…"), |b| b.title.clone())))))
                        .child(text(move || breadcrumb.with(|b| b.as_ref().map_or_else(|| t("Loading thread…"), |b| b.title.clone())))
                            .app_class(tw("block min-w-0 truncate"))))
                    .child(dynamic(move || origin.get().map(|origin| untracked(|| {
                        let description = origin.clone();
                        view().app_class(tw("flex shrink-0"))
                            .tooltip(tooltip(tooltip_create_handle(), TooltipOptions::default(), text(move || tr("Started by an agent on {origin} through a linked environment.", &[("origin", Value::Text(description.clone()))]))))
                            .child(badge::badge(badge::Props { variant: badge::Variant::Info, size: badge::Size::Sm, ..Default::default() }, vec![badge::Child::Element(text(move || tr("From {origin}", &[("origin", Value::Text(origin.clone()))])).into_element())]))
                    }))))),
            ),
        )
}

/// Port of apps/web/src/components/WorkspaceBreadcrumb.tsx WorkspaceBreadcrumbSeparator.
fn workspace_breadcrumb_separator() -> impl IntoElement {
    view()
        .accessible(false)
        .app_class(tw("flex shrink-0 items-center text-icon-muted"))
        .child(text(t("/")).app_class(tw("block min-w-0 truncate")))
}

/// Port of apps/web/src/components/chat/PanelLayoutControls.tsx PanelLayoutControls.
pub fn panel_layout_controls(key: ThreadKey, has_project: Memo<bool>) -> impl IntoElement {
    let host = use_context::<PanelHost>();
    let layout = stored(use_context::<HeaderLayout>());
    let key = stored(key);
    let panel = host.map(|host| host.panel(key.get()));
    view()
        .app_class(tw("flex h-full shrink-0 items-center gap-1"))
        .child(panel_toggle(
            Icon::SquareMenu,
            "Toggle thread details panel",
            "Toggle thread details",
            move || {
                host.is_some_and(|host| {
                    layout.with(|layout| {
                        layout.as_ref().is_some_and(|l| {
                            host.thread_panel_open(&key.get(), l.details_inline.get())
                        })
                    })
                })
            },
            move || host.is_none() || layout.with(|layout| layout.is_none()),
            move || {
                if let (Some(host), Some(layout)) = (host, layout.get()) {
                    host.toggle_thread_panel(&key.get(), layout.details_inline.get_untracked());
                }
            },
        ))
        .child(panel_toggle(
            Icon::PanelBottom,
            "Toggle terminal drawer",
            "Terminal drawer is unavailable",
            move || {
                layout.with(|l| {
                    l.as_ref()
                        .and_then(|l| l.terminal.as_ref())
                        .is_some_and(|t| t.open.get())
                })
            },
            move || {
                !has_project.get()
                    || layout.with(|l| {
                        l.as_ref()
                            .and_then(|l| l.terminal.as_ref())
                            .is_none_or(|t| !t.available.get())
                    })
            },
            move || {
                if has_project.get_untracked()
                    && let Some(terminal) = layout.get().and_then(|l| l.terminal)
                    && terminal.available.get_untracked()
                {
                    (terminal.on_toggle)();
                }
            },
        ))
        .child(panel_toggle(
            Icon::PanelRight,
            "Toggle right panel",
            "Right panel is unavailable",
            move || panel.is_some_and(|panel| panel.with(|p| p.is_open)),
            move || host.is_none() || !has_project.get(),
            move || {
                if has_project.get_untracked()
                    && let Some(host) = host
                {
                    host.apply(&key.get(), Action::ToggleVisibility);
                }
            },
        ))
}

fn panel_toggle(
    glyph: Icon,
    label: &'static str,
    hint: &'static str,
    pressed: impl Fn() -> bool + 'static,
    disabled: impl Fn() -> bool + 'static,
    on_toggle: impl Fn() + 'static,
) -> impl IntoElement {
    let handle = tooltip_create_handle();
    let disabled = Rc::new(disabled);
    let disabled_read = disabled.clone();
    let disabled_event = disabled.clone();
    view()
        .app_class(tw("flex shrink-0"))
        .tooltip(tooltip(
            handle,
            TooltipOptions {
                side: Placement::Bottom,
                ..Default::default()
            },
            text(move || {
                I18nText::from(if disabled() {
                    hint
                } else if glyph == Icon::SquareMenu {
                    "Toggle thread details"
                } else {
                    label
                })
                .resolve()
            }),
        ))
        .child(toggle(
            Content::slot(
                I18nText::from(label).resolve().to_string(),
                icon_with_class(glyph, "size-4"),
            ),
            Variant::Ghost,
            Size::Sm,
            pressed,
            move || disabled_read(),
            move |_| {
                if !disabled_event() {
                    on_toggle();
                }
            },
        ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use t3_client::types::ThreadId;

    #[test]
    fn a_thread_above_another_thread_keeps_back_even_with_an_adjacent_list() {
        let thread = |id: &str| {
            Route::Thread(ThreadKey {
                environment: EnvironmentId("e".into()),
                thread: ThreadId(id.into()),
            })
        };
        assert!(has_thread_below(&[
            Route::Home,
            thread("parent"),
            thread("child")
        ]));
        assert!(!has_thread_below(&[Route::Home, thread("child")]));
        assert!(!has_thread_below(&[
            Route::Home,
            thread("parent"),
            Route::ThreadPanel(
                ThreadKey {
                    environment: EnvironmentId("e".into()),
                    thread: ThreadId("child".into())
                },
                crate::state::nav::ThreadPanel::Files
            )
        ]));
    }
}
