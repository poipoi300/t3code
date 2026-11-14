//! Port of apps/web/src/components/chat/ChatHeader.tsx ChatHeader.

use std::rc::Rc;

use futures::{StreamExt, future::LocalBoxFuture, stream::LocalBoxStream};
use nsh_core::prelude::*;
use t3_client::types::{EnvironmentId, ThreadKey};
use t3_client::view_logic::session::text::{TextDirection, text_direction};
use t3_client::workspace::ProjectKey;
use t3_contracts::orchestration_project::OrchestrationProjectShell;
use t3_contracts::orchestration_v2::AppThreadLineageRelationshipToParent;

use crate::i18n::{Text as I18nText, Value, t, tr};
use crate::runtime::{Shared, rt};
use crate::screens::panel::{Action, PanelHost};
use crate::screens::sidebar::actions::{self, ThreadActionMenu};
use crate::state::layout::layout_class;
use crate::state::nav::{Route, navigator};
use crate::theme::tw;
use crate::ui::avatar::{ProjectFaviconProject, project_favicon};
use crate::ui::badge;
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};
use crate::ui::icon::{Icon, icon_with_class};
use crate::ui::menu::{self, MenuOptions};
use crate::ui::toast::{self, ToastOptions};
use crate::ui::toggle::{Content, Size, Variant, toggle};
use crate::ui::tooltip::{TooltipOptions, tooltip, tooltip_create_handle};

/// Projection of the client-owned thread operations and project identity.
#[derive(Clone, Debug, PartialEq)]
pub struct ThreadHeaderState {
    pub can_operate: bool,
    pub unavailable_reason: Option<I18nText>,
    pub menu: Option<ThreadActionMenu>,
    pub is_scratch: Option<bool>,
}

impl Default for ThreadHeaderState {
    fn default() -> Self {
        Self {
            can_operate: false,
            unavailable_reason: Some("This connection cannot change threads.".into()),
            menu: None,
            is_scratch: None,
        }
    }
}

/// Titles are client English keys; details distinguish catalogue keys from server content.
#[derive(Clone, Debug)]
pub enum HeaderError {
    Interrupted,
    Failed {
        title: String,
        detail: Option<I18nText>,
    },
}

pub trait ThreadHeaderService {
    /// Emits the current state first; dropping the stream releases its subscription.
    fn watch(&self, key: ThreadKey) -> LocalBoxStream<'static, Rc<ThreadHeaderState>>;
    fn rename_thread(
        &self,
        key: ThreadKey,
        title: String,
    ) -> LocalBoxFuture<'static, Result<(), HeaderError>>;
    fn perform_action(
        &self,
        key: ThreadKey,
        action: String,
    ) -> LocalBoxFuture<'static, Result<(), HeaderError>>;
    fn close_guest_share(&self, key: ThreadKey)
    -> LocalBoxFuture<'static, Result<(), HeaderError>>;
}

#[derive(Clone)]
pub struct ThreadHeaderRuntime(pub Rc<dyn ThreadHeaderService>);

pub trait ThreadHeaderNavigationService {
    /// Current availability first; dropping the stream releases its subscription.
    fn watch(&self, project: ProjectKey) -> LocalBoxStream<'static, Option<I18nText>>;
    fn new_thread_in_project(
        &self,
        project: ProjectKey,
    ) -> LocalBoxFuture<'static, Result<crate::screens::new_thread::Destination, HeaderError>>;
}

#[derive(Clone)]
pub struct ThreadHeaderNavigation(pub Rc<dyn ThreadHeaderNavigationService>);

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

#[derive(Clone, PartialEq)]
struct GuestShare {
    expires_at: String,
    name: Option<String>,
}

#[derive(Clone, Copy)]
struct HeaderController {
    owner: Owner,
    key: Stored<ThreadKey>,
    service: Option<Stored<Rc<dyn ThreadHeaderService>>>,
    state: Signal<Rc<ThreadHeaderState>>,
    renaming: Signal<Option<String>>,
    draft: Signal<String>,
    menu_open: Signal<bool>,
    context_open: Signal<bool>,
    closing_share: Signal<bool>,
}

impl HeaderController {
    fn new(key: ThreadKey) -> Self {
        let service = use_context::<ThreadHeaderRuntime>()
            .or_else(|| {
                rt().backend()
                    .client()
                    .map(|client| crate::runtime::services::thread_ops::thread_header(&client))
            })
            .map(|runtime| stored(runtime.0));
        let state = signal(Rc::new(ThreadHeaderState::default()));
        if let Some(service) = service {
            let mut changes = service.get().watch(key.clone());
            spawn(async move {
                while let Some(next) = changes.next().await {
                    state.set(next);
                }
                state.set(Rc::new(ThreadHeaderState::default()));
            });
        }
        let this = Self {
            owner: Owner::current(),
            key: stored(key),
            service,
            state,
            renaming: signal(None),
            draft: signal(String::new()),
            menu_open: signal(false),
            context_open: signal(false),
            closing_share: signal(false),
        };
        // External permission changes cancel an editor that can no longer submit.
        effect(move || {
            if !this.state.with(|state| state.can_operate) {
                this.renaming.set(None);
            }
        });
        this
    }

    fn begin_rename(self, title: String) {
        if self.service.is_some()
            && self
                .state
                .with_untracked(|state| state.can_operate && state.unavailable_reason.is_none())
        {
            self.menu_open.set(false);
            self.context_open.set(false);
            self.draft.set(title.clone());
            self.renaming.set(Some(title));
        }
    }

    fn commit_rename(self) {
        let Some(original) = self.renaming.get_untracked() else {
            return;
        };
        self.renaming.set(None);
        if !self
            .state
            .with_untracked(|state| state.can_operate && state.unavailable_reason.is_none())
        {
            return;
        }
        let title = t3_contracts::wire::trim_js(&self.draft.get_untracked()).to_owned();
        if title.is_empty() {
            toast::toast_manager().add(ToastOptions {
                variant: toast::Variant::Warning,
                title: "Thread title cannot be empty".into(),
                ..Default::default()
            });
        } else if title != original
            && let Some(service) = self.service
        {
            self.owner.run(move || {
                spawn(async move {
                    if let Err(error) = service.get().rename_thread(self.key.get(), title).await {
                        report_header_error(error, Some("Failed to rename thread"));
                    }
                })
            });
        }
    }

    fn dispatch(self, action: String, title: String) {
        self.menu_open.set(false);
        self.context_open.set(false);
        if action == "rename" {
            // The closing native menu restores focus before the new input takes it.
            self.owner
                .run(move || after_answer(move || self.begin_rename(title)));
            return;
        }
        if self
            .state
            .with_untracked(|state| state.unavailable_reason.is_some())
        {
            return;
        }
        if let Some(service) = self.service {
            self.owner.run(move || {
                spawn(async move {
                    if let Err(error) = service.get().perform_action(self.key.get(), action).await {
                        report_header_error(error, None);
                    }
                })
            });
        }
    }
}

fn report_header_error(error: HeaderError, title: Option<&str>) {
    if let HeaderError::Failed {
        title: failure_title,
        detail,
    } = error
    {
        toast::toast_manager().add(ToastOptions {
            variant: toast::Variant::Error,
            title: title.unwrap_or(&failure_title).to_owned(),
            description_content: detail.map(|detail| {
                Rc::new(move || text(detail.resolve()).into_element()) as toast::Slot
            }),
            ..Default::default()
        });
    }
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
                titles
                    .get()
                    .map_or_else(SharedString::default, |(title, _)| {
                        SharedString::from(title)
                    })
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
            .and_then(|id| {
                let title = shell
                    .threads
                    .iter()
                    .chain(shell.archived_threads.iter())
                    .find(|row| &row.id == id)
                    .map(|row| row.title.clone())?;
                Some((
                    ThreadKey {
                        environment: environment.clone(),
                        thread: id.clone(),
                    },
                    title,
                ))
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
    let workspace_panel = use_context::<PanelHost>().map(|host| host.panel(key.clone()));
    let controller = HeaderController::new(key.clone());
    let projection = runtime.store.thread(key.clone());
    let guest = computed(move || {
        projection.with(|projection| {
            projection
                .as_ref()
                .and_then(|projection| {
                    projection
                        .thread
                        .guest_share
                        .as_ref()
                        .and_then(Option::as_ref)
                })
                .map(|share| GuestShare {
                    expires_at: share.expires_at.0.clone(),
                    name: share.guest_name.as_ref().map(|name| name.0.clone()),
                })
        })
    });
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
        // ContextMenu adds a native wrapper; reserve the breadcrumb's share on that wrapper.
        .child(view().app_class(tw("min-w-0 flex-1")).child(chat_header(breadcrumb, key.environment.clone(), controller, guest)))
        // The web fixes this group to window chrome; the shared header is its native anchor.
        .child(when(
            move || !(layout_class() == crate::state::layout::LayoutClass::Wide
                && workspace_panel.is_some_and(|panel| panel.with(|panel| panel.is_open))),
            move || panel_layout_controls(key.clone(), has_project),
        ))
}

/// Port of mobile/components/ui/AppBar.tsx's threadBelow condition (095611d1b5).
fn has_thread_below(stack: &[Route]) -> bool {
    matches!(stack, [.., Route::Thread(_), Route::Thread(_)])
}

fn chat_header(
    breadcrumb: Memo<Option<Breadcrumb>>,
    environment: EnvironmentId,
    controller: HeaderController,
    guest: Memo<Option<GuestShare>>,
) -> impl IntoElement {
    let nav = navigator();
    let project = computed(move || {
        breadcrumb.with(|b| b.as_ref().and_then(|b| b.project.clone()).map(Shared))
    });
    let parent = computed(move || breadcrumb.with(|b| b.as_ref().and_then(|b| b.parent.clone())));
    let origin = computed(move || {
        if controller.renaming.with(Option::is_some) {
            None
        } else {
            breadcrumb.with(|b| b.as_ref().and_then(|b| b.origin.clone()))
        }
    });
    let available_width = signal(0.0_f32);
    let collapse_parent = computed(move || {
        let available = available_width.get();
        let Some((_, parent_title)) = parent.get() else {
            return false;
        };
        if available <= 0.0 {
            return false;
        }
        let tokens = tokens();
        let family = tokens
            .font(nsh_core::element::FONT_SANS)
            .map_or("sans-serif", |name| name.as_ref());
        let font = format!("500 14px {family}");
        let title_width =
            breadcrumb.with(|b| b.as_ref().map_or(0.0, |b| measure_text(&font, &b.title).0)) + 18.0;
        let project_width = project.with(|project| {
            project.as_ref().map(|project| {
                let name = if controller
                    .state
                    .with(|state| state.is_scratch == Some(true))
                {
                    t("No project")
                } else {
                    project.title.0.clone()
                };
                measure_text(&font, &name).0.min(160.0) + 20.0
            })
        });
        let separators = if project_width.is_some() { 2.0 } else { 1.0 };
        let gaps = if project_width.is_some() { 4.0 } else { 2.0 };
        let gap = if layout_class().is_compact() {
            8.0
        } else {
            12.0
        };
        title_width
            + measure_text(&font, &parent_title).0
            + project_width.unwrap_or(0.0)
            + separators * measure_text(&format!("400 14px {family}"), "/").0
            + gaps * gap
            > available
    });
    let content = view()
        // Sibling controls reserve their actual width instead of the web's pr-10/pr-24.
        .app_class(tw("flex min-w-0 flex-1 items-center gap-2 sm:gap-3"))
        .child(view()
            .role(AccessibilityRole::Navigation)
            .label(move || t("Thread breadcrumb"))
            .app_class(tw("min-w-0 flex-1 overflow-clip"))
            // Native lists/text omit list-none, text-box and its supports padding hint.
            .child(view()
                .app_class(tw("m-0 flex min-w-0 items-center gap-2 p-0 text-sm sm:gap-3"))
                .on_layout(move |bounds: &Bounds<Points>| available_width.set(bounds.size.width.0))
                .child(dynamic(move || {
                    let project = project.get();
                    let environment = environment.clone();
                    project.map(|project| untracked(|| {
                        vec![
                            project_breadcrumb(project.0, environment, controller).into_element(),
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
                                .on_press_down(move || controller.renaming.set(None))
                                .on_press(move || nav.open(Route::Thread(key.clone())))
                                .app_class(tw("inline-flex min-w-0 max-w-full cursor-pointer items-center rounded-sm text-muted-foreground transition-colors hover:text-foreground focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"))
                                .child(text(SharedString::from(title)).accessible(false).app_class(move || tw(if collapse_parent.get() { "block min-w-0 truncate w-0" } else { "block min-w-0 truncate" })))
                                .child(when(move || collapse_parent.get(), || text(SharedString::from("...")).accessible(false).test_id("parent-breadcrumb-ellipsis"))))
                            .into_element(),
                        workspace_breadcrumb_separator().into_element(),
                    ])).unwrap_or_default()
                }))
                .child(view()
                    .app_class(tw("flex min-w-0 items-center font-medium text-foreground min-w-10 flex-1"))
                    .child(thread_title(breadcrumb, controller))
                    .child(dynamic(move || origin.get().map(|origin| untracked(|| {
                        let description = origin.clone();
                        view().app_class(tw("flex shrink-0"))
                            .tooltip(tooltip(tooltip_create_handle(), TooltipOptions::default(), text(move || tr("Started by an agent on {origin} through a linked environment.", &[("origin", Value::Text(description.clone()))]))))
                            .child(badge::badge(badge::Props { variant: badge::Variant::Info, size: badge::Size::Sm, ..Default::default() }, vec![badge::Child::Element(text(move || tr("From {origin}", &[("origin", Value::Text(origin.clone()))])).into_element())]))
                    }))))),
            ),
        )
        .child(guest_share_indicator(guest, controller));
    menu::context_menu(
        content.on_context_menu(move |_: &ContextMenuEvent| controller.menu_open.set(false)),
        MenuOptions {
            open: Some(controller.context_open),
            ..Default::default()
        },
        move || {
            controller.renaming.with(Option::is_some)
                || controller.service.is_none()
                || controller
                    .state
                    .with(|state| state.unavailable_reason.is_some() || state.menu.is_none())
        },
        header_menu_popup(breadcrumb, controller),
    )
}

fn header_menu_popup(
    breadcrumb: Memo<Option<Breadcrumb>>,
    controller: HeaderController,
) -> impl IntoElement {
    let model = computed(move || controller.state.with(|state| state.menu.clone()));
    actions::thread_action_menu_content(model, move |action| {
        let title = breadcrumb.with(|b| b.as_ref().map_or_else(String::new, |b| b.title.clone()));
        controller.dispatch(action, title);
    })
}

fn thread_title(
    breadcrumb: Memo<Option<Breadcrumb>>,
    controller: HeaderController,
) -> impl IntoElement {
    let ready = computed(move || breadcrumb.with(Option::is_some));
    let title = computed(move || {
        breadcrumb.with(|b| b.as_ref().map_or_else(String::new, |b| b.title.clone()))
    });
    let unavailable = computed(move || {
        controller.service.is_none()
            || controller
                .state
                .with(|state| state.unavailable_reason.is_some() || state.menu.is_none())
    });
    let chevron_bounds = signal(Bounds::new(
        nsh_core::geometry::point(px(0.0), px(0.0)),
        size(px(0.0), px(0.0)),
    ));
    view().app_class(tw("min-w-0"))
        .child(when(ready, move || {
            dynamic(move || {
                let editing = controller.renaming.with(Option::is_some);
                untracked(|| {
                    if editing {
                        let node = element_ref();
                        let focused = stored(false);
                        actions::thread_rename_input_with_style(controller.draft, move || controller.commit_rename(), move || controller.renaming.set(None), actions::ThreadRenameInputStyle::Header, Some(node))
                            // Native menu focus return can follow autofocus in the same mount batch.
                            .on_layout(move |_: &Bounds<Points>| {
                                if !focused.get() { focused.replace(true); node.focus(); }
                            })
                            .into_element()
                    } else {
                        // ButtonEvent::Press erases click count/modifiers; the native button-role view retains them.
                        let trigger = view().role(AccessibilityRole::Button).focusable(true)
                            .accessibility_state(move || nsh_core::AccessibilityState {
                                has_popup: true,
                                expanded: Some(controller.menu_open.get()),
                                ..Default::default()
                            })
                            .test_id("thread-title-actions")
                            .label(move || SharedString::from(tr("Thread actions for {title}", &[("title", Value::Text(title.get()))])))
                            .disabled(move || unavailable.get())
                            .on_press(move |event: &PressEvent| {
                                if unavailable.get_untracked() { return; }
                                if event.clicks > 1 {
                                    if event.clicks == 2 && event.modifiers == Default::default() && !chevron_bounds.get_untracked().contains(&event.position) {
                                        controller.begin_rename(title.get_untracked());
                                    }
                                } else {
                                    controller.context_open.set(false);
                                    controller.menu_open.set(true);
                                }
                            })
                            .on_context_menu(move |_: &ContextMenuEvent| controller.context_open.set(false))
                            .app_class(tw("group/thread-title inline-flex min-w-0 max-w-full cursor-pointer items-center gap-1 rounded-sm text-left focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"))
                            .child(view().role(AccessibilityRole::Header).accessibility_level(2u32).app_class(tw("min-w-0"))
                                .direction(move || match text_direction(&title.get()) { Some(TextDirection::Ltr) => LayoutDirection::Ltr, Some(TextDirection::Rtl) => LayoutDirection::Rtl, None => LayoutDirection::Inherit })
                                .child(text(move || title.get()).app_class(tw("block min-w-0 truncate"))))
                            .child(view().on_layout(move |bounds: &Bounds<Points>| chevron_bounds.set(*bounds)).app_class(tw("size-3.5 shrink-0"))
                                .child(icon_with_class(Icon::ChevronDown, "size-3.5 shrink-0 text-muted-foreground opacity-0 transition-opacity group-hover/thread-title:opacity-100 group-focus-visible/thread-title:opacity-100")));
                        view().app_class(tw("min-w-0"))
                            .tooltip(tooltip(tooltip_create_handle(), TooltipOptions::default(), text(move || controller.state.with(|state| state.unavailable_reason.as_ref().map_or_else(|| title.get(), |reason| reason.resolve().to_string())))))
                            .child(menu::context_menu(trigger, MenuOptions { open: Some(controller.menu_open), align: Align::Start, ..Default::default() }, move || unavailable.get(), header_menu_popup(breadcrumb, controller)))
                            .into_element()
                    }
                })
            })
        }))
}

fn project_breadcrumb(
    project: Rc<OrchestrationProjectShell>,
    environment: EnvironmentId,
    controller: HeaderController,
) -> impl IntoElement {
    let project_key = stored(ProjectKey {
        environment: environment.clone(),
        project: project.id.clone(),
    });
    let navigation = use_context::<ThreadHeaderNavigation>().map(|service| stored(service.0));
    let unavailable = signal(Some(I18nText::from(
        "This connection cannot change threads.",
    )));
    if let Some(navigation) = navigation {
        let mut changes = navigation.get().watch(project_key.get());
        spawn(async move {
            while let Some(reason) = changes.next().await {
                unavailable.set(reason);
            }
            unavailable.set(Some("This connection cannot change threads.".into()));
        });
    }
    let name = stored(project.title.0.clone());
    let display_name = move || {
        if controller
            .state
            .with(|state| state.is_scratch == Some(true))
        {
            t("No project")
        } else {
            name.get()
        }
    };
    let favicon = ProjectFaviconProject::from_shell(environment, &project);
    let nav = navigator();
    view().app_class(tw("flex min-w-0 items-center font-medium shrink-0 text-muted-foreground shrink"))
        .tooltip(tooltip(tooltip_create_handle(), TooltipOptions::default(), text(move || unavailable.get().map_or_else(|| tr("New thread in {project}", &[("project", Value::Text(display_name()))]), |reason| reason.resolve().to_string()))))
        .child(nsh_base::button()
            .disabled(move || unavailable.with(Option::is_some))
            .accessibility_label(move || Some(tr("New thread in {project}", &[("project", Value::Text(display_name()))]).into()))
            .on_press_down(move || controller.renaming.set(None))
            .on_press(move || {
                if unavailable.get_untracked().is_none() && let Some(navigation) = navigation {
                    let navigation = navigation.get();
                    let project_key = project_key.get();
                    controller.owner.run(move || spawn(async move {
                        match navigation.new_thread_in_project(project_key).await {
                            Ok(destination) => nav.open(destination.route()),
                            Err(error) => report_header_error(error, None),
                        }
                    }));
                }
            })
            .app_class(tw("inline-flex min-w-0 max-w-full cursor-pointer items-center gap-1.5 rounded-sm text-muted-foreground transition-colors hover:text-foreground focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"))
            // The shared favicon resource reader remains the assets runtime owner's binding.
            .child(project_favicon(move || favicon.clone(), |_| None, "size-3.5", None))
            .child(text(display_name).app_class(tw("block min-w-0 truncate max-w-40"))))
}

/// Port of chat/GuestShareControls.tsx GuestShareIndicator and GuestShareStatus.
fn guest_share_indicator(
    share: Memo<Option<GuestShare>>,
    controller: HeaderController,
) -> impl IntoElement {
    view().child(dynamic(move || {
        share.get().map(|_| {
            untracked(|| {
                let status = move || {
                    share.with(|share| {
                        let Some(share) = share else {
                            return String::new();
                        };
                        let expired = t3_client::services::config::timestamp_ms(&share.expires_at)
                            .is_some_and(|expires| {
                                expires.div_euclid(60_000)
                                    <= crate::state::clock::now_ms().div_euclid(60_000)
                            });
                        if expired {
                            t("Guest link expired")
                        } else if let Some(name) = &share.name {
                            tr("{name} is a guest", &[("name", Value::Text(name.clone()))])
                        } else {
                            t("Guest link active")
                        }
                    })
                };
                view()
                    .app_class(tw(
                        "flex shrink-0 items-center gap-1 text-muted-foreground text-xs",
                    ))
                    .child(icon_with_class(Icon::User, "size-3.5"))
                    .child(text(status))
                    .child(
                        view()
                            .tooltip(tooltip(
                                tooltip_create_handle(),
                                TooltipOptions::default(),
                                text(move || {
                                    controller.state.with(|state| {
                                        state.unavailable_reason.as_ref().map_or_else(
                                            || t("Stop sharing"),
                                            |reason| reason.resolve().to_string(),
                                        )
                                    })
                                }),
                            ))
                            .child(button(
                                ButtonContent::Label(t("Stop sharing")),
                                ButtonVariant::GhostMuted,
                                ButtonSize::Xs,
                                move || {
                                    controller.service.is_none()
                                        || controller.closing_share.get()
                                        || controller
                                            .state
                                            .with(|state| state.unavailable_reason.is_some())
                                },
                                move || {
                                    if controller.closing_share.get_untracked()
                                        || controller.state.with_untracked(|state| {
                                            state.unavailable_reason.is_some()
                                        })
                                    {
                                        return;
                                    }
                                    if let Some(service) = controller.service {
                                        controller.closing_share.set(true);
                                        controller.owner.run(move || {
                                            spawn(async move {
                                                let result = service
                                                    .get()
                                                    .close_guest_share(controller.key.get())
                                                    .await;
                                                controller.closing_share.set(false);
                                                if let Err(error @ HeaderError::Failed { .. }) = result {
                                                    log::warn!("[atom-command] stop guest share failed: {error:?}");
                                                }
                                            })
                                        });
                                    }
                                },
                            )),
                    )
            })
        })
    }))
}

/// Port of apps/web/src/components/WorkspaceBreadcrumb.tsx WorkspaceBreadcrumbSeparator.
fn workspace_breadcrumb_separator() -> impl IntoElement {
    view()
        .accessible(false)
        .app_class(tw("flex shrink-0 items-center text-icon-muted"))
        .child(text(SharedString::from("/")).app_class(tw("block min-w-0 truncate")))
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
