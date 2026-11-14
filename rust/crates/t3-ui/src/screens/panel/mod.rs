//! Port of apps/web/src/rightPanelStore.ts and apps/web/src/components/RightPanelTabs.tsx.

// Declared by the integrator (ui-packages.py); the owner edits below this block.
pub mod branch;
pub mod checkpoints;
pub mod details;
pub mod diff;
pub mod diff_tree;
pub mod file_formats;
pub mod file_viewer;
pub mod files_tree;
pub mod git_actions;
pub mod review_comments;
// End of declarations.

use std::rc::Rc;

use nsh_core::element::{
    AnyValue, PropError, PropKind, PropSpec, SignalEquals, TsTypes, signal_with,
};
use nsh_core::prelude::*;
use t3_client::types::ThreadKey;
pub use t3_client::view_logic::shell::panels::{
    Action, Panel, PanelStore, Surface, SurfaceContent,
};

use crate::i18n::{Value, t, tr};
use crate::state::layout::LayoutClass;
use crate::theme::{cn, palette, tw};
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};
use crate::ui::icon::{Icon, icon, icon_with_class};
use crate::ui::menu::{self, ItemOptions, MenuOptions};
use crate::ui::panel::{panel, panel_layout};
use crate::ui::tabs::{panel_tab_close_button, tab, tabs};

/// Created above the shell's adaptive replacement boundary. The client owns every transition.
#[derive(Clone, Copy)]
pub struct PanelHost {
    state: Signal<PanelStore>,
    changed: Stored<Rc<dyn Fn(PanelChange)>>,
    new_surface: Signal<Option<ThreadKey>>,
}

impl PartialEq for PanelHost {
    fn eq(&self, other: &Self) -> bool {
        self.state == other.state
    }
}

/// The runtime persists the client record and passes closed tabs to its reopen-history service.
pub struct PanelChange {
    pub thread: ThreadKey,
    pub state: PanelStore,
    pub closed: Vec<Surface>,
}

impl PanelHost {
    pub fn new(state: PanelStore, changed: impl Fn(PanelChange) + 'static) -> Self {
        Self {
            state: signal_with(state, SignalEquals::Never),
            changed: stored(Rc::new(changed)),
            new_surface: signal(None),
        }
    }

    pub fn apply(self, thread: &ThreadKey, action: Action) {
        let mut closed = Vec::new();
        self.state
            .update(|store| closed = store.apply(thread, action));
        let state = self.state.get_untracked();
        let thread = state.host_ref(thread).clone();
        (self.changed.get())(PanelChange {
            thread,
            state,
            closed,
        });
    }

    pub fn panel(self, thread: ThreadKey) -> Memo<Panel> {
        computed(move || {
            self.state
                .with(|store| store.by_thread.get(&thread).cloned().unwrap_or_default())
        })
    }

    /// `inline` comes from the canvas layout's details-card presentation, not the platform.
    pub fn thread_panel_open(self, thread: &ThreadKey, inline: bool) -> bool {
        self.state.with(|store| {
            let visibility = store.visibility.get(thread).copied().unwrap_or_default();
            if inline {
                visibility.inline_open
            } else {
                visibility.popover_open
            }
        })
    }

    pub fn toggle_thread_panel(self, thread: &ThreadKey, inline: bool) {
        self.apply(thread, Action::ToggleThreadPanel { inline });
    }

    pub fn request_new_surface(self, thread: &ThreadKey) {
        if self.state.with_untracked(|store| {
            store
                .by_thread
                .get(thread)
                .is_some_and(|panel| panel.is_open)
        }) {
            self.new_surface.set(Some(thread.clone()));
        }
    }
}

/// Read in the component body and capture for header/keybinding event handlers.
pub fn panel_host() -> PanelHost {
    use_context::<PanelHost>().expect("The workspace provides its persistent PanelHost")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceKind {
    Browser,
    Terminal,
    Files,
    Diff,
    PullRequest,
    PullRequests,
    Device,
    SideChat,
}

impl SurfaceKind {
    const ALL: [Self; 8] = [
        Self::Browser,
        Self::Terminal,
        Self::Files,
        Self::Diff,
        Self::PullRequest,
        Self::PullRequests,
        Self::Device,
        Self::SideChat,
    ];

    fn of(surface: &Surface) -> Option<Self> {
        Some(match surface.content {
            SurfaceContent::Preview { .. } => Self::Browser,
            SurfaceContent::Terminal { .. } => Self::Terminal,
            SurfaceContent::Files { .. } | SurfaceContent::File { .. } => Self::Files,
            SurfaceContent::Diff => Self::Diff,
            SurfaceContent::PullRequest { .. } => Self::PullRequest,
            SurfaceContent::PullRequests => Self::PullRequests,
            SurfaceContent::Device { .. } => Self::Device,
            SurfaceContent::SideChat { .. } => Self::SideChat,
            SurfaceContent::Unknown { .. } => return None,
        })
    }

    fn label(self) -> &'static str {
        match self {
            Self::Browser => "Browser",
            Self::Terminal => "Terminal",
            Self::Files => "Files",
            Self::Diff => "Diff",
            Self::PullRequest => "Pull request",
            Self::PullRequests => "Linked pull requests",
            Self::Device => "Device",
            Self::SideChat => "Side chat",
        }
    }

    fn icon(self) -> Icon {
        match self {
            Self::Browser => Icon::Earth,
            Self::Terminal => Icon::SquareTerminal,
            Self::Files => Icon::Files,
            Self::Diff => Icon::FileDiff,
            Self::PullRequest => Icon::GitPullRequest,
            Self::PullRequests => Icon::Link,
            Self::Device => Icon::Smartphone,
            Self::SideChat => Icon::MessagesSquare,
        }
    }

    fn shortcut(self) -> &'static str {
        match self {
            Self::Browser => "B",
            Self::Terminal => "T",
            Self::Files => "F",
            Self::Diff => "D",
            Self::PullRequest => "P",
            Self::PullRequests => "L",
            Self::Device => "M",
            Self::SideChat => "S",
        }
    }
}

/// Owners register once when composing the workspace. `None` means available; errors stay visible.
type SurfaceTitle = Rc<dyn Fn(&Surface) -> String>;
type SurfacePending = Rc<dyn Fn(&Surface) -> bool>;
type SurfaceGlyph = Rc<dyn Fn(&Surface) -> AnyElement>;
type SurfaceView = Rc<dyn Fn(SurfaceContext) -> AnyElement>;

pub struct PanelRegistration {
    pub kind: SurfaceKind,
    pub unavailable: Prop<Option<String>>,
    pub open: Rc<dyn Fn(ThreadKey)>,
    pub render: SurfaceView,
    /// Live service metadata: browser title, terminal label, device name, or side-chat status.
    pub title: Option<SurfaceTitle>,
    pub pending: Option<SurfacePending>,
    pub glyph: Option<SurfaceGlyph>,
    /// Feature controls (for example browser audio) and menu rows precede the shared tab actions.
    pub tab_controls: Option<SurfaceView>,
    pub tab_menu: Option<SurfaceView>,
}

impl PanelRegistration {
    pub fn new(
        kind: SurfaceKind,
        unavailable: Prop<Option<String>>,
        open: impl Fn(ThreadKey) + 'static,
        render: impl Fn(SurfaceContext) -> AnyElement + 'static,
    ) -> Self {
        Self {
            kind,
            unavailable,
            open: Rc::new(open),
            render: Rc::new(render),
            title: None,
            pending: None,
            glyph: None,
            tab_controls: None,
            tab_menu: None,
        }
    }
}

#[derive(Clone, Copy)]
pub struct PanelRegistry(Stored<Vec<Rc<PanelRegistration>>>);

impl PanelRegistry {
    pub fn new(registrations: Vec<PanelRegistration>) -> Self {
        let mut kinds = Vec::new();
        for registration in &registrations {
            assert!(
                !kinds.contains(&registration.kind),
                "Register each panel kind once"
            );
            kinds.push(registration.kind);
        }
        Self(stored(registrations.into_iter().map(Rc::new).collect()))
    }

    fn registration(self, kind: SurfaceKind) -> Option<Rc<PanelRegistration>> {
        self.0
            .with(|rows| rows.iter().find(|row| row.kind == kind).cloned())
    }

    fn reason(self, kind: SurfaceKind) -> Option<String> {
        self.registration(kind).map_or_else(
            || Some(t("This surface is not available on this host.")),
            |row| row.unavailable.get().map(|reason| t(&reason)),
        )
    }

    pub fn open(self, kind: SurfaceKind, thread: &ThreadKey) {
        if let Some(row) = self.registration(kind)
            && row.unavailable.get_untracked().is_none()
        {
            (row.open)(thread.clone());
        }
    }

    fn kinds(self) -> Vec<SurfaceKind> {
        SurfaceKind::ALL
            .into_iter()
            .filter(|kind| *kind != SurfaceKind::SideChat || self.registration(*kind).is_some())
            .collect()
    }
}

#[derive(Clone)]
pub struct SurfaceContext {
    pub host: PanelHost,
    pub thread: ThreadKey,
    pub surface: Memo<Option<Surface>>,
    /// Owners suspend input, subscriptions and shortcuts while their retained view is hidden.
    pub visible: Memo<bool>,
}

/// Port of chat/ChatRightPanel.tsx. Keep this owner above adaptive shell replacement.
#[derive(Clone)]
pub struct ChatRightPanelProps {
    data: Option<Rc<ChatRightPanelData>>,
    workspace: Option<Slot>,
    children: Prop<Children>,
}

impl PartialEq for ChatRightPanelProps {
    fn eq(&self, other: &Self) -> bool {
        let same = match (&self.data, &other.data) {
            (Some(a), Some(b)) => Rc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        same && self.workspace == other.workspace && self.children == other.children
    }
}

#[derive(Clone)]
struct ChatRightPanelData {
    host: PanelHost,
    thread: ThreadKey,
    row_width: Prop<f32>,
    width: Prop<f32>,
    animation_duration_ms: u32,
    width_changed: Rc<dyn Fn(f32)>,
}

// The client handles are Rust-only; exposing them as JavaScript values would lose ownership.
impl Props for ChatRightPanelProps {
    fn spec(_: &mut TsTypes) -> Vec<PropSpec> {
        [
            ("workspace", PropKind::Slot),
            ("children", PropKind::Children),
        ]
        .into_iter()
        .map(|(name, kind)| PropSpec {
            name,
            ts: "JSX.Element".into(),
            optional: true,
            doc: "",
            kind,
            decode: None,
        })
        .collect()
    }
    fn initial() -> Self {
        Self {
            data: None,
            workspace: None,
            children: Children::default().into(),
        }
    }
    fn set(&mut self, name: &str, value: Option<AnyValue>) -> Result<bool, PropError> {
        ().set(name, value)
    }
    fn set_slot(&mut self, name: &str, slot: Option<Slot>) -> bool {
        if name != "workspace" {
            return false;
        }
        let changed = self.workspace != slot;
        self.workspace = slot;
        changed
    }
    fn set_children(&mut self, children: Children) -> bool {
        let changed = self.children.get_untracked() != children;
        self.children = children.into();
        changed
    }
}

pub struct ChatRightPanel;

impl Component for ChatRightPanel {
    type Props = ChatRightPanelProps;
    type Event = NoEvent;

    fn view(props: Self::Props, _: Scope<NoEvent>) -> impl IntoElement {
        match props.data {
            Some(ref data) => {
                chat_right_panel_frame(props.clone(), data.as_ref().clone()).into_element()
            }
            None => view().into_element(),
        }
    }
}

/// Fill `workspace` with the thread tree and pass `right_panel_tabs(...)` as the non-lazy child.
pub fn chat_right_panel(
    host: PanelHost,
    thread: ThreadKey,
    row_width: Prop<f32>,
    width: Prop<f32>,
    animation_duration_ms: u32,
    width_changed: impl Fn(f32) + 'static,
) -> ComponentElement<ChatRightPanel> {
    ChatRightPanel::element(ChatRightPanelProps {
        data: Some(Rc::new(ChatRightPanelData {
            host,
            thread,
            row_width,
            width,
            animation_duration_ms,
            width_changed: Rc::new(width_changed),
        })),
        ..ChatRightPanelProps::initial()
    })
}

fn chat_right_panel_frame(
    slots: ChatRightPanelProps,
    props: ChatRightPanelData,
) -> impl IntoElement {
    let state = props.host.panel(props.thread.clone());
    let thread = stored(props.thread);
    let width_changed = stored(props.width_changed);
    let placement = panel_layout();
    let chosen_width = signal(None::<f32>);
    let drag_start = signal(0.0);
    let width = computed(move || {
        inline_width(
            chosen_width.get().unwrap_or_else(|| props.width.get()),
            window().width.0,
            props.row_width.get(),
        )
    });
    view().flex_1().min_h(px(0.0)).child(dynamic(move || {
        let placement = placement.get()?;
        Some(untracked(|| {
            let surface = panel(
                move || state.with(|p| p.is_open),
                props.animation_duration_ms,
                move || props.host.apply(&thread.get(), Action::Close),
                slots.children,
            );
            if placement == LayoutClass::Compact {
                // The panel's page must be a direct stack child; an owning wrapper is a phantom page.
                nsh_base::nav_stack().flex_1()
                    .child(nsh_base::nav_page().header_visible(false).title(t("Thread")).child(view().flex_1().slot(slots.workspace, |slot| slot)))
                    .child(surface).into_element()
            } else {
                view().flex_1().flex_row().min_h(px(0.0))
                    .child(view().flex_1().min_w(px(0.0))
                        .app_class(move || Some(tw(if placement == LayoutClass::Wide && state.with(|p| p.is_open && p.maximized == Some(true)) { "hidden" } else { "flex" })))
                        .slot(slots.workspace, |slot| slot))
                    .child(view().h_full().min_h(px(0.0)).test_id("workspace-panel-host")
                        .app_class(move || Some(tw(if placement == LayoutClass::Wide && state.with(|p| p.maximized != Some(true)) { "relative shrink-0 border-l border-border" } else { "relative" })))
                        .w(move || if placement == LayoutClass::Wide && state.with(|p| p.is_open) {
                            px(if state.with(|p| p.maximized == Some(true)) { props.row_width.get() } else { width.get() })
                        } else { px(0.0) })
                        .child(surface)
                        .child(when(move || placement == LayoutClass::Wide && state.with(|p| p.is_open && p.maximized != Some(true)), move || {
                            // UI-RESIZE is not implemented yet; NSH supplies native pointer capture.
                            nsh_base::resize_handle()
                                .test_id("workspace-panel-resize")
                                .edge(Some(nsh_base::HandleEdge::Leading))
                                .accessibility_label(Some(SharedString::from(t("Resize workspace panel"))))
                                .on_drag_start(move || drag_start.set(width.get_untracked()))
                                .on_drag_move(move |offset: &Points| chosen_width.set(Some(inline_width(drag_start.get_untracked() - offset.0, window().width.0, props.row_width.get_untracked()))))
                                .on_drag_end(move |offset: &Points| {
                                    let next = inline_width(drag_start.get_untracked() - offset.0, window().width.0, props.row_width.get_untracked());
                                    chosen_width.set(Some(next));
                                    if next != drag_start.get_untracked() { (width_changed.get())(next); }
                                })
                                .child(view().app_class(tw("pointer-events-none h-full w-px bg-transparent group-hover:bg-border group-focus-visible:bg-primary/60 group-active:bg-primary/60")))
                        }))).into_element()
            }
        }))
    }))
}

/// usePreviewPanelInlineSize.ts: retain a 360px sibling and the 70% viewport cap.
pub fn inline_width(requested: f32, viewport: f32, row: f32) -> f32 {
    let max = (viewport * 0.7).floor().min(row.floor() - 360.0).max(360.0);
    let requested = if requested.is_finite() {
        requested
    } else {
        540.0
    };
    requested.clamp(360.0, max)
}

/// Retained target for the details/drawer toggles. Their owners supply layout and client state.
#[derive(Clone, PartialEq, Props)]
pub struct PanelTargetProps {
    pub open: Prop<bool>,
    pub children: Prop<Children>,
}

#[component]
pub fn PanelTarget(props: PanelTargetProps) -> impl IntoElement {
    view()
        .min_h(px(0.0))
        .child(when(move || props.open.get(), move || props.children))
}

/// Port of RightPanelTabs.tsx RightPanelTabs; layout controls remain TL-HEADER's slot.
pub fn right_panel_tabs(
    host: PanelHost,
    thread: ThreadKey,
    registry: PanelRegistry,
    layout_controls: impl IntoElement,
) -> impl IntoElement {
    let thread = stored(thread);
    let state = host.panel(thread.get());
    let surfaces = computed(move || state.with(|p| p.surfaces.clone()));
    let menu_open = signal(false);
    effect(move || {
        if !state.with(|p| p.is_open) {
            menu_open.set(false);
        }
        if host.new_surface.get().as_ref() == Some(&thread.get()) {
            host.new_surface.set(None);
            if state.with(|p| p.is_open) {
                menu_open.set(true);
            }
        }
    });
    let add = menu::menu(
        button(
            ButtonContent::Icon {
                icon: Icon::Plus,
                label: t("Add panel surface"),
            },
            ButtonVariant::GhostMuted,
            ButtonSize::IconXs,
            move || !state.with(|p| p.is_open),
            || {},
        ),
        MenuOptions {
            open: Some(menu_open),
            align: Align::Start,
            side_offset: 6.0,
            ..Default::default()
        },
        menu::menu_popup_with(move || {
            view().child(
                registry
                    .kinds()
                    .into_iter()
                    .map(move |kind| surface_menu_item(registry, thread.get(), kind)),
            )
        }),
    );
    let strip = tabs(
        t("Workspace panel tabs"),
        view()
            .app_class(tw("flex h-full w-max min-w-full items-center gap-1"))
            .child(each(
                move || surfaces.get(),
                |surface: &Surface| surface.id.clone(),
                move |surface, _| {
                    untracked(|| right_panel_tab(host, thread.get(), registry, state, surface))
                },
            ))
            .child(add),
    );
    let header = view().app_class(tw("flex h-[var(--workspace-topbar-height)] min-h-[var(--workspace-topbar-height)] shrink-0 items-center gap-1 pl-2 pr-3"))
        // Native scroll owns touch/wheel gestures; CSS ScrollArea's DOM observer is unnecessary.
        .child(scroll().horizontal().flex_1().min_w(px(0.0)).test_id("panel-tabs-scroll").child(strip))
        .child(layout_controls);
    view()
        .app_class(tw(
            "relative flex h-full min-h-0 min-w-0 max-w-full flex-col self-stretch bg-background",
        ))
        .label(move || {
            SharedString::from(state.with(|p| {
                p.selected().map_or_else(
                    || t("Workspace panel"),
                    |surface| {
                        tr(
                            "Workspace panel: {title}",
                            &[("title", Value::Text(surface_title(surface, registry)))],
                        )
                    },
                )
            }))
        })
        .child(header)
        .child(
            view()
                .app_class(tw("flex min-h-0 flex-1 flex-col"))
                .child(when(
                    move || state.with(|p| p.active_surface_id.is_none()),
                    move || right_panel_empty_state(registry, thread.get(), state, menu_open),
                ))
                .child(each(
                    move || surfaces.get(),
                    |surface: &Surface| surface.id.clone(),
                    move |surface, _| {
                        untracked(|| {
                            let id = stored(surface.get_untracked().id);
                            let current = computed(move || {
                                state
                                    .with(|p| p.surfaces.iter().find(|s| s.id == id.get()).cloned())
                            });
                            let visible = computed(move || {
                                state.with(|p| {
                                    p.is_open
                                        && p.active_surface_id.as_deref() == Some(id.get().as_str())
                                })
                            });
                            let context = SurfaceContext {
                                host,
                                thread: thread.get(),
                                surface: current,
                                visible,
                            };
                            retained_surface()
                                .visible(visible)
                                .child(right_panel_surface_content(registry, context))
                        })
                    },
                )),
        )
}

#[derive(Clone, PartialEq, Props)]
struct RetainedSurfaceProps {
    visible: Prop<bool>,
    children: Prop<Children>,
}

#[component]
fn RetainedSurface(props: RetainedSurfaceProps) -> impl IntoElement {
    view()
        .flex_1()
        .min_h(px(0.0))
        .app_class(move || {
            Some(tw(if props.visible.get() {
                "flex"
            } else {
                "hidden"
            }))
        })
        .child(props.children)
}

/// Port of chat/RightPanelSurfaceContent.tsx. Each registered renderer is built once per tab.
pub fn right_panel_surface_content(registry: PanelRegistry, context: SurfaceContext) -> AnyElement {
    let registration = context
        .surface
        .get_untracked()
        .as_ref()
        .and_then(SurfaceKind::of)
        .and_then(|kind| registry.registration(kind));
    match registration {
        Some(row) => (row.render)(context),
        None => view()
            .app_class(tw("flex min-h-0 flex-1 items-center justify-center px-6"))
            .child(text(t("This surface is not available on this host.")))
            .into_element(),
    }
}

fn right_panel_tab(
    host: PanelHost,
    thread: ThreadKey,
    registry: PanelRegistry,
    state: Memo<Panel>,
    surface: ReadSignal<Surface>,
) -> impl IntoElement {
    let thread = stored(thread);
    let id = stored(surface.get_untracked().id);
    let active = computed(move || state.with(|p| p.active_surface_id.as_ref() == Some(&id.get())));
    let title = move || surface.with(|surface| surface_title(surface, registry));
    let current = computed(move || Some(surface.get()));
    let visible = computed(move || {
        state.with(|p| p.is_open && p.active_surface_id.as_ref() == Some(&id.get()))
    });
    let context = stored(SurfaceContext {
        host,
        thread: thread.get(),
        surface: current,
        visible,
    });
    let registration =
        SurfaceKind::of(&surface.get_untracked()).and_then(|kind| registry.registration(kind));
    let menu_rows = stored(registration.as_ref().and_then(|row| row.tab_menu.clone()));
    let controls = registration
        .and_then(|row| row.tab_controls.clone())
        .map(|render| render(context.get()));
    let menu = menu::menu_popup_with(move || {
        view()
            .child(menu_rows.get().map(|render| render(context.get())))
            .child(
                [
                    ("Close", Action::CloseSurface(id.get())),
                    ("Close others", Action::CloseOthers(id.get())),
                    ("Close to the right", Action::CloseRight(id.get())),
                    ("Close all", Action::CloseAll),
                ]
                .into_iter()
                .map(move |(label, action)| {
                    let action = stored(action);
                    menu::menu_item(
                        t(label),
                        ItemOptions {
                            disabled: Prop::bind(move || {
                                state.with(|p| match action.get() {
                                    Action::CloseOthers(_) => p.surfaces.len() <= 1,
                                    Action::CloseRight(_) => {
                                        p.surfaces.last().is_none_or(|s| s.id == id.get())
                                    }
                                    Action::CloseAll => p.surfaces.is_empty(),
                                    _ => false,
                                })
                            }),
                            ..Default::default()
                        },
                        text(t(label)),
                        move || host.apply(&thread.get(), action.get()),
                    )
                }),
            )
    });
    menu::context_menu(tab(title(), move || active.get(), || false, move || host.apply(&thread.get(), Action::Activate(id.get())),
        view().on_mouse_up(move |event: &MouseEvent| { if event.button == Some(nsh_core::MouseButton::Middle) { host.apply(&thread.get(), Action::CloseSurface(id.get())); } }).app_class(move || Some(tw(&cn([Some("cursor-pointer group/tab flex h-6 max-w-36 shrink-0 items-center gap-0.5 rounded-md pr-2 pl-1.5 text-xs"), Some(if active.get() { "bg-accent text-foreground" } else { "text-muted-foreground hover:bg-accent/60 hover:text-foreground" })]))))
            .child(panel_tab_close_button(surface_icon(surface, registry), tr("Close {title}", &[("title", Value::Text(title()))]), move || host.apply(&thread.get(), Action::CloseSurface(id.get())), None))
            .child(controls)
            .child(text(move || SharedString::from(title())).app_class(tw("truncate")))), MenuOptions::default(), || false, menu)
}

fn surface_menu_item(
    registry: PanelRegistry,
    thread: ThreadKey,
    kind: SurfaceKind,
) -> impl IntoElement {
    let thread = stored(thread);
    menu::menu_item(
        t(kind.label()),
        ItemOptions {
            disabled: Prop::bind(move || registry.reason(kind).is_some()),
            ..Default::default()
        },
        view()
            .app_class(tw("flex min-w-0 flex-1 items-center gap-2"))
            .child(text(t(kind.label())))
            .child(
                text(move || SharedString::from(registry.reason(kind).unwrap_or_default()))
                    .app_class(tw("text-muted-foreground text-xs")),
            )
            .child(menu::menu_shortcut(t(kind.shortcut()))),
        move || registry.open(kind, &thread.get()),
    )
}

fn right_panel_empty_state(
    registry: PanelRegistry,
    thread: ThreadKey,
    state: Memo<Panel>,
    menu_open: Signal<bool>,
) -> impl IntoElement {
    let thread = stored(thread);
    let highlight = signal(None::<SurfaceKind>);
    let targets = stored(
        registry
            .kinds()
            .into_iter()
            .map(|kind| (kind, element_ref()))
            .collect::<Vec<_>>(),
    );
    // Window-level launcher shortcuts stop for editors and the add-surface popup.
    effect(move || {
        if !state.with(|p| p.is_open && p.active_surface_id.is_none()) || menu_open.get() {
            return;
        }
        for kind in registry
            .kinds()
            .into_iter()
            .filter(|kind| registry.reason(*kind).is_none())
        {
            for shortcut in [
                kind.shortcut().to_lowercase(),
                format!("shift+{}", kind.shortcut().to_lowercase()),
            ] {
                on_key_with(
                    &shortcut,
                    KeyOptions {
                        when: nsh_core::keymap::When::NotEditing,
                        ..Default::default()
                    },
                    move |_| registry.open(kind, &thread.get()),
                );
            }
        }
    });
    // Native focus selects launcher rows; Enter then uses the focused button's native activation.
    let keys = nsh_core::KeyCapture::keys(
        [
            nsh_core::keymap::Key::Up,
            nsh_core::keymap::Key::Down,
            nsh_core::keymap::Key::Left,
            nsh_core::keymap::Key::Right,
        ]
        .into_iter()
        .chain(['b', 't', 'f', 'd', 'p', 'l', 'm', 's'].map(nsh_core::keymap::Key::Char))
        .map(|key| nsh_core::keymap::Keystroke {
            key,
            modifiers: Default::default(),
        }),
    );
    // overflow-y-auto is native scroll; scrollbar-gutter has no native meaning.
    scroll().flex_1().min_h(px(0.0)).child(
        view().label(t("Open a surface")).test_id("panel-surface-launcher").focusable(true)
            .capture_keys(keys)
            .on_key_down(move |event: &KeyEvent| {
                if event.modifiers.ctrl || event.modifiers.alt || event.modifiers.cmd { return; }
                let available = targets.with(|rows| rows.iter().filter(|(kind, _)| registry.reason(*kind).is_none()).copied().collect::<Vec<_>>());
                if available.is_empty() { return; }
                let reverse = matches!(event.key.as_ref(), "ArrowUp" | "ArrowLeft");
                if reverse || matches!(event.key.as_ref(), "ArrowDown" | "ArrowRight") {
                    let current = available.iter().position(|(kind, _)| Some(*kind) == highlight.get_untracked());
                    let next = if reverse { current.map_or(available.len() - 1, |i| (i + available.len() - 1) % available.len()) }
                        else { current.map_or(0, |i| (i + 1) % available.len()) };
                    highlight.set(Some(available[next].0));
                    available[next].1.focus();
                } else if let Some((kind, _)) = available.iter().find(|(kind, _)| kind.shortcut().eq_ignore_ascii_case(event.key.as_ref())) {
                    registry.open(*kind, &thread.get());
                }
            })
            .app_class(tw("flex min-h-0 flex-1 items-center justify-center px-6 pb-(--workspace-topbar-height)"))
            .child(view().app_class(tw("w-full max-w-xs py-6"))
                .child(text(t("Open a surface")).app_class(tw("mb-3 text-center font-medium text-foreground text-sm")))
                .child(view().app_class(tw("flex flex-col gap-0.5")).child(targets.get().into_iter().map(move |(kind, target)| {
                    nsh_base::button().accessibility_label(t(kind.label())).element_ref(&target)
                        .disabled(move || registry.reason(kind).is_some())
                        .on_press(move || registry.open(kind, &thread.get()))
                        .child(view()
                            .on_mouse_enter(move |_: &MouseEvent| highlight.set(Some(kind)))
                            .on_mouse_leave(move |_: &MouseEvent| { if highlight.get_untracked() == Some(kind) { highlight.set(None); } })
                            .app_class(move || Some(tw(&cn([
                                Some("flex h-8 w-full items-center gap-2.5 rounded-(--control-radius) px-2.5 text-left text-sm"),
                                Some(if registry.reason(kind).is_some() { "cursor-default opacity-50" } else { "cursor-pointer hover:bg-accent/60" }),
                                (highlight.get() == Some(kind) && registry.reason(kind).is_none()).then_some("bg-accent/60"),
                            ]))))
                            .child(icon(kind.icon(), 16.0, move || palette().foreground))
                            .child(text(t(kind.label())).app_class(tw("min-w-0 flex-1 truncate")))
                            .child(text(move || SharedString::from(registry.reason(kind).unwrap_or_default())).app_class(tw("text-xs text-muted-foreground")))
                            .child(text(t(kind.shortcut()))))
                }))))
    )
}

fn surface_title(surface: &Surface, registry: PanelRegistry) -> String {
    if let Some(row) = SurfaceKind::of(surface).and_then(|kind| registry.registration(kind))
        && let Some(title) = &row.title
    {
        return t(&title(surface));
    }
    match &surface.content {
        SurfaceContent::File { relative_path, .. } => relative_path
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(relative_path)
            .to_owned(),
        SurfaceContent::PullRequest { number, .. } => {
            tr("#{number}", &[("number", Value::Int(*number as i64))])
        }
        SurfaceContent::PullRequests => t("Pull requests"),
        SurfaceContent::Device { target, title } => title
            .clone()
            .or_else(|| target.as_ref().map(|target| target.name.clone()))
            .unwrap_or_else(|| t("Device")),
        _ => t(SurfaceKind::of(surface).map_or("Unknown surface", SurfaceKind::label)),
    }
}

fn surface_icon(surface: ReadSignal<Surface>, registry: PanelRegistry) -> impl IntoElement {
    dynamic(move || {
        let surface = surface.get();
        let row = SurfaceKind::of(&surface).and_then(|kind| registry.registration(kind));
        if let Some(glyph) = row.as_ref().and_then(|row| row.glyph.as_ref()) {
            return glyph(&surface);
        }
        let which = match surface.content {
            SurfaceContent::Files { .. } => Icon::Files,
            SurfaceContent::File { .. } => Icon::FileText,
            SurfaceContent::Diff => Icon::FileDiff,
            SurfaceContent::Preview { .. } => Icon::Earth,
            SurfaceContent::Terminal { .. } => Icon::SquareTerminal,
            SurfaceContent::Device { .. } => Icon::Smartphone,
            SurfaceContent::PullRequest { .. } => Icon::GitPullRequest,
            SurfaceContent::PullRequests => Icon::Link,
            SurfaceContent::SideChat { .. } => Icon::MessagesSquare,
            SurfaceContent::Unknown { .. } => Icon::CircleAlert,
        };
        view()
            .app_class(tw(
                "relative flex size-3 shrink-0 items-center justify-center",
            ))
            .child(icon_with_class(which, "size-3 shrink-0"))
            .child(
                row.and_then(|row| row.pending.clone())
                    .filter(|pending| pending(&surface))
                    .map(|_| {
                        view().app_class(tw(
                            "absolute -right-0.5 -bottom-0.5 size-1.5 rounded-full bg-current",
                        ))
                    }),
            )
            .into_element()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_width_preserves_the_sibling_and_viewport_caps_without_inverting_bounds() {
        for (requested, viewport, row, expected) in [
            (540.0, 1280.0, 1024.0, 540.0),
            (900.0, 1280.0, 1024.0, 664.0),
            (900.0, 1280.0, 1280.0, 896.0),
            (540.0, 1024.0, 600.0, 360.0),
            (10.0, 1280.0, 1024.0, 360.0),
            (f32::NAN, 1280.0, 1024.0, 540.0),
        ] {
            assert_eq!(inline_width(requested, viewport, row), expected);
        }
    }
}
