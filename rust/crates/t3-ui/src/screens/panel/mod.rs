//! Port of rightPanelStore.ts, RightPanelTabs.tsx, ChatRightPanel.tsx and ThreadDetailsCard.tsx.

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

use std::{collections::BTreeMap, rc::Rc};

use nsh_core::element::{
    AnyValue, PropError, PropKind, PropSpec, SignalEquals, TsTypes, signal_with,
};
use nsh_core::prelude::*;
use t3_client::types::ThreadKey;
pub use t3_client::view_logic::shell::panels::{
    Action, Panel, PanelStore, Surface, SurfaceContent,
};

use crate::i18n::{Text as I18nText, Value, t, tr};
use crate::state::layout::LayoutClass;
use crate::theme::{cn, palette, tw};
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};
use crate::ui::icon::{Icon, icon, icon_with_class};
use crate::ui::menu::{self, ItemOptions, MenuOptions};
use crate::ui::panel::{panel, panel_layout};
use crate::ui::tabs::{panel_tab_close_button, tab, tabs};

/// TERM-1 provides its controlled drawer and content under the workspace owner.
#[derive(Clone)]
pub struct TerminalRegistration {
    pub control: crate::screens::thread::header::TerminalControl,
    pub render: Rc<dyn Fn(ThreadKey) -> AnyElement>,
}

/// GIT-2 supplies the branch row with its client workspace/VCS eligibility checks.
#[derive(Clone)]
pub struct BranchRegistration(pub Rc<dyn Fn(ThreadKey) -> AnyElement>);

/// GIT-1 supplies its scoped control; its client service owns scratch/VCS eligibility.
#[derive(Clone)]
pub struct GitActionsRegistration(pub Rc<dyn Fn(ThreadKey) -> AnyElement>);

#[derive(Clone, Copy)]
struct WorkspaceSession {
    host: PanelHost,
    widths: Signal<BTreeMap<ThreadKey, f32>>,
    history: Signal<t3_client::view_logic::shell::closed_views::ClosedViews>,
}

impl Default for WorkspaceSession {
    fn default() -> Self {
        use t3_client::view_logic::shell::closed_views::{ClosedView, ClosedViews};
        let history = signal_with(ClosedViews::default(), SignalEquals::Never);
        let sequence = signal(0_u64);
        let host = PanelHost::new(PanelStore::default(), move |change| {
            for surface in change.closed.into_iter().filter(Surface::can_remember) {
                sequence.update(|id| *id += 1);
                history.update(|history| {
                    history.remember(
                        sequence.get_untracked().to_string(),
                        ClosedView::PanelTab {
                            thread_ref: t3_contracts::environment::ScopedThreadRef {
                                environment_id: change.thread.environment.clone(),
                                thread_id: change.thread.thread.clone(),
                            },
                            surface,
                        },
                    );
                });
            }
        });
        Self {
            host,
            widths: signal(BTreeMap::new()),
            history,
        }
    }
}

/// Closed-tab history for the default in-memory workspace session.
pub fn workspace_panel_history() -> Signal<t3_client::view_logic::shell::closed_views::ClosedViews>
{
    global::<WorkspaceSession>().get_untracked().history
}

/// The thread screen mounts this entry point; it owns demand, header, canvas and panel targets.
pub fn thread_workspace(key: ThreadKey) -> impl IntoElement {
    mounted_thread_workspace()
        .environment(SharedString::from(key.environment.0))
        .thread(SharedString::from(key.thread.0))
}

/// Registrations and the mounted workspace share this host; a restored runtime context wins.
pub fn workspace_panel_host() -> PanelHost {
    use_context::<PanelHost>().unwrap_or_else(|| global::<WorkspaceSession>().get_untracked().host)
}

#[derive(Clone, PartialEq, Props)]
struct MountedThreadWorkspaceProps {
    environment: SharedString,
    thread: SharedString,
}

#[component]
fn MountedThreadWorkspace(props: MountedThreadWorkspaceProps) -> impl IntoElement {
    use crate::screens::thread::{composer, header, timeline};
    use t3_client::workspace::chat_canvas::{
        resolve_chat_canvas_lane, resolve_thread_details_card_layout,
    };

    let key = ThreadKey {
        environment: t3_client::types::EnvironmentId(props.environment.to_string()),
        thread: t3_client::types::ThreadId(props.thread.to_string()),
    };
    let runtime = crate::runtime::rt();
    let backend = runtime.backend();
    backend.open_thread(&key);
    let closing = key.clone();
    cleanup(move || backend.close_thread(&closing));

    // NSH globals create their signals under a detached owner, above the shell replacement.
    let session = global::<WorkspaceSession>().get_untracked();
    let host = workspace_panel_host();
    provide_context(host);
    let selected = stored(key.clone());
    let row_width = signal(0.0);
    let canvas_size = signal((0.0, 0.0));
    let measured_card = signal(false);
    let padding = computed(move || {
        if crate::state::layout::layout_class().is_compact() {
            12.0
        } else {
            48.0
        }
    });
    let placement = computed(move || {
        resolve_thread_details_card_layout(
            canvas_size.get(),
            (padding.get(), 640.0),
            None,
            false,
            0.0,
        )
    });
    let inline = computed(move || placement.with(Option::is_some));
    // TERM's drawer of this thread over the root's terminal service; absent on the demo
    // backend, where the header's toggle stays disabled.
    let services = use_context::<crate::runtime::services::terminal::TerminalServices>();
    debug_assert!(
        services.is_some() || runtime.backend().client().is_none(),
        "TerminalServices is not provided: app::provide_client_services provides it with the client"
    );
    let terminal = services.map(|services| {
        crate::screens::terminal::drawer::terminal_registration(services, key.clone())
    });
    provide_context(header::HeaderLayout {
        details_inline: Prop::bind(move || inline.get()),
        terminal: terminal.as_ref().map(|terminal| terminal.control.clone()),
    });
    let registry = use_context::<PanelRegistry>();
    debug_assert!(
        registry.is_some(),
        "PanelRegistry is not provided: app::provide_client_services (or provide_without_client)          provides it above the routes"
    );
    let registry = registry.unwrap_or_else(|| PanelRegistry::new(vec![]));
    let thread_id = key.thread.clone();
    let shell = runtime.store.shell(key.environment.clone());
    let has_project = computed(move || {
        shell.with(|shell| {
            shell.as_ref().is_some_and(|shell| {
                shell.threads.iter().any(|t| {
                    t.id == thread_id && shell.projects.iter().any(|p| p.id == t.project_id)
                })
            })
        })
    });
    let lane = computed(move || {
        let card_left =
            (inline.get() && measured_card.get() && host.thread_panel_open(&selected.get(), true))
                .then(|| placement.get().map(|p| p.x))
                .flatten();
        resolve_chat_canvas_lane(
            canvas_size.get().0,
            padding.get(),
            tokens().number("chat-content-max-width").unwrap_or(736.0),
            card_left,
        )
    });
    let details_height = Prop::bind(move || {
        placement
            .get()
            .map_or((canvas_size.get().1 - 52.0).max(0.0), |p| p.height)
    });
    let make_slots = move || details_slots(selected.get());
    let inline_details = details::thread_details_target(
        host,
        key.clone(),
        Prop::bind(move || inline.get()),
        details_height,
        make_slots(),
    );
    let header = nsh_base::popover()
        .open(move || Some(!inline.get() && host.thread_panel_open(&selected.get(), false)))
        .on_open_change(move |event: &nsh_base::PopoverEvent| {
            // The header's details button owns the toggle; native trigger presses must not toggle twice.
            if let nsh_base::PopoverEvent::OpenChange(open, reason) = event
                && *reason != nsh_base::OpenChangeReason::TriggerPress
            {
                host.apply(
                    &selected.get(),
                    Action::SetThreadPanel {
                        inline: false,
                        open: *open,
                    },
                );
            }
        })
        .placement(Placement::Bottom)
        .align(Align::End)
        .offset(px(0.0))
        .trigger(header::thread_header(key.clone()))
        .child(dynamic(move || {
            (!inline.get()).then(|| {
                untracked(|| {
                    view()
                        .w(px(280.0))
                        .max_w(move || px((window().width.0 - 24.0).max(0.0)))
                        .child(details::thread_details_panel(
                            selected.get(),
                            details_height,
                            make_slots(),
                        ))
                        .into_element()
                })
            })
        }));
    let canvas = view()
        .app_class(tw("relative flex min-h-0 min-w-0 flex-1"))
        .test_id("thread-workspace-canvas")
        .on_layout(move |bounds: &Bounds<Points>| {
            canvas_size.set((bounds.size.width.0, bounds.size.height.0))
        })
        .child(
            view()
                .flex_1()
                .min_h(px(0.0))
                .min_w(px(0.0))
                .pr(move || px(lane.get().inset_end))
                .child(timeline::timeline(key.clone()))
                .child(composer::composer(key.clone()))
                // The drawer shows itself while the thread's drawer is open; it stays mounted so
                // its chords (`terminal.toggle`) work while it is closed.
                .child(terminal.map(|terminal| (terminal.render)(key.clone()))),
        )
        .child(
            view()
                .app_class(move || {
                    tw(if inline.get() {
                        "absolute z-20"
                    } else {
                        "absolute z-20 hidden"
                    })
                })
                .right(px(12.0))
                .top(move || px(placement.get().map_or(12.0, |p| p.y)))
                .w(px(280.0))
                .max_h(move || px(details_height.get()))
                .on_layout(move |bounds: &Bounds<Points>| {
                    measured_card.set(bounds.size.height.0 > 0.0)
                })
                .child(inline_details),
        );
    let workspace = view()
        .flex_1()
        .min_h(px(0.0))
        .min_w(px(0.0))
        .bg(move || palette().background)
        .child(header)
        .child(canvas);
    view()
        .flex_1()
        .min_h(px(0.0))
        .min_w(px(0.0))
        .on_layout(move |bounds: &Bounds<Points>| row_width.set(bounds.size.width.0))
        .child(
            chat_right_panel(
                host,
                key.clone(),
                Prop::bind(move || row_width.get()),
                Prop::bind(move || {
                    session
                        .widths
                        .with(|widths| widths.get(&selected.get()).copied().unwrap_or(540.0))
                }),
                150,
                move |width| {
                    session.widths.update(|widths| {
                        widths.insert(selected.get(), width);
                    })
                },
            )
            .fill("workspace", workspace)
            .child(right_panel_tabs(
                host,
                key.clone(),
                registry,
                header::panel_layout_controls(key, has_project),
            )),
        )
}

fn details_slots(key: ThreadKey) -> details::VersionControlSlots {
    let branch =
        use_context::<BranchRegistration>().map(|registration| (registration.0)(key.clone()));
    let git_actions =
        use_context::<GitActionsRegistration>().map(|registration| (registration.0)(key));
    details::VersionControlSlots {
        branch,
        git_actions,
    }
}

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

/// The runtime binding receives the client record and actual closed tabs for persistence/history.
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
type SurfaceTitle = Rc<dyn Fn(&Surface) -> SharedString>;
type SurfacePending = Rc<dyn Fn(&Surface) -> bool>;
type SurfaceGlyph = Rc<dyn Fn(&Surface) -> AnyElement>;
type SurfaceView = Rc<dyn Fn(SurfaceContext) -> AnyElement>;

pub struct PanelRegistration {
    pub kind: SurfaceKind,
    pub unavailable: Prop<Option<I18nText>>,
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
        unavailable: Prop<Option<I18nText>>,
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
        self.registration(kind).and_then(|row| {
            row.unavailable
                .get()
                .map(|reason| reason.resolve().to_string())
        })
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
            .filter(|kind| self.registration(*kind).is_some())
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
    let frames = signal(BTreeMap::<String, Bounds<Points>>::new());
    effect(move || {
        let ids = surfaces.with(|surfaces| {
            surfaces
                .iter()
                .map(|surface| surface.id.clone())
                .collect::<std::collections::BTreeSet<_>>()
        });
        frames.update(|frames| frames.retain(|id, _| ids.contains(id)));
    });
    let strip_width = signal(0.0_f32);
    let viewport_width = signal(0.0_f32);
    let scroll_offset = signal(0.0_f32);
    let strip_scroll = element_ref();
    let overflow = computed(move || strip_width.get() - viewport_width.get() > 1.0);
    effect(move || {
        let active = state.with(|p| p.active_surface_id.clone());
        let bounds = frames.with(|frames| active.as_ref().and_then(|id| frames.get(id).copied()));
        if !overflow.get() {
            return;
        }
        if let Some(bounds) = bounds {
            let left = bounds.origin.x.0;
            let right = bounds.right().0;
            let offset = scroll_offset.get_untracked();
            let width = viewport_width.get_untracked();
            let next = if left < offset {
                left
            } else if right > offset + width {
                right - width
            } else {
                offset
            };
            if next != offset {
                strip_scroll.scroll_to_offset(px(next), false);
            }
        }
    });
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
            .on_layout(move |bounds: &Bounds<Points>| strip_width.set(bounds.size.width.0))
            .child(each(
                move || surfaces.get(),
                |surface: &Surface| surface.id.clone(),
                move |surface, _| {
                    untracked(|| {
                        sortable_tab_list(
                            host,
                            thread.get(),
                            state,
                            surface,
                            frames,
                            viewport_width,
                            scroll_offset,
                            registry,
                        )
                    })
                },
            ))
            .child(add),
    );
    let header = view().app_class(tw("flex h-[var(--workspace-topbar-height)] min-h-[var(--workspace-topbar-height)] shrink-0 items-center gap-1 pl-2 pr-3"))
        // Native scroll owns touch/wheel gestures; CSS ScrollArea's DOM observer is unnecessary.
        .child(scroll().horizontal().flex_1().min_w(px(0.0)).test_id("panel-tabs-scroll")
            .element_ref(&strip_scroll)
            .on_layout(move |bounds: &Bounds<Points>| viewport_width.set(bounds.size.width.0))
            .on_scroll(move |event: &ScrollEvent| scroll_offset.set(event.offset.x.0))
            .child(strip))
        .child(when(move || overflow.get(), move || view().app_class(tw("flex shrink-0 items-center gap-0.5"))
            .label(t("Scroll panel tabs"))
            .child(button(ButtonContent::Icon { icon: Icon::ChevronLeft, label: t("Scroll tabs left") }, ButtonVariant::Ghost, ButtonSize::IconXs,
                move || scroll_offset.get() <= 1.0,
                move || strip_scroll.scroll_to_offset(px((scroll_offset.get_untracked() - (viewport_width.get_untracked() * 0.75).max(120.0)).max(0.0)), !reduce_motion())))
            .child(button(ButtonContent::Icon { icon: Icon::ChevronRight, label: t("Scroll tabs right") }, ButtonVariant::Ghost, ButtonSize::IconXs,
                move || scroll_offset.get() + viewport_width.get() >= strip_width.get() - 1.0,
                move || strip_scroll.scroll_to_offset(px((scroll_offset.get_untracked() + (viewport_width.get_untracked() * 0.75).max(120.0)).min((strip_width.get_untracked() - viewport_width.get_untracked()).max(0.0))), !reduce_motion())))))
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
                            let selected = computed(move || {
                                state.with(|p| {
                                    p.active_surface_id.as_deref() == Some(id.get().as_str())
                                })
                            });
                            let visible =
                                computed(move || state.with(|p| p.is_open && selected.get()));
                            let context = SurfaceContext {
                                host,
                                thread: thread.get(),
                                surface: current,
                                visible,
                            };
                            retained_surface()
                                // The sheet retains its selected content while its exit animation ends.
                                .visible(selected)
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
        None => view().into_element(),
    }
}

/// Port of RightPanelTabs.tsx SortableTabList; native responder capture replaces dnd-kit.
#[allow(clippy::too_many_arguments)]
fn sortable_tab_list(
    host: PanelHost,
    thread: ThreadKey,
    state: Memo<Panel>,
    surface: ReadSignal<Surface>,
    frames: Signal<BTreeMap<String, Bounds<Points>>>,
    viewport: Signal<f32>,
    scroll: Signal<f32>,
    registry: PanelRegistry,
) -> impl IntoElement {
    let thread = stored(thread);
    let id = stored(surface.get_untracked().id);
    let offset = signal(0.0_f32);
    let active = signal(false);
    let clamp = move |delta: f32| {
        frames.with_untracked(|frames| {
            frames.get(&id.get()).map_or(0.0, |bounds| {
                let min = scroll.get_untracked() - bounds.origin.x.0;
                let max =
                    (scroll.get_untracked() + viewport.get_untracked() - bounds.right().0).max(min);
                delta.clamp(min, max)
            })
        })
    };
    view()
        .test_id(&format!("panel-tab-{}", id.get()))
        .on_layout(move |bounds: &Bounds<Points>| {
            frames.update(|frames| {
                frames.insert(id.get(), *bounds);
            })
        })
        .translate_x(move || px(offset.get()))
        .z_index(move || if active.get() { 10 } else { 0 })
        .claim_touches(TouchClaim::on_move().ask(move |request: &ClaimRequest| {
            if request
                .event
                .gesture
                .delta
                .x
                .0
                .hypot(request.event.gesture.delta.y.0)
                > 6.0
            {
                request.claim();
            }
        }))
        .keep_touches(true)
        .on_responder_grant(move |_: &ResponderEvent| active.set(true))
        .on_responder_move(move |event: &ResponderEvent| offset.set(clamp(event.gesture.delta.x.0)))
        .on_responder_release(move |event: &ResponderEvent| {
            let delta = clamp(event.gesture.delta.x.0);
            let target = frames.with_untracked(|frames| {
                let center = frames.get(&id.get())?.center().x.0 + delta;
                state.with_untracked(|panel| {
                    panel
                        .surfaces
                        .iter()
                        .enumerate()
                        .filter_map(|(index, surface)| {
                            let distance = (frames.get(&surface.id)?.center().x.0 - center).abs();
                            Some((index, distance))
                        })
                        .min_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.0.cmp(&b.0)))
                        .map(|(index, _)| index)
                })
            });
            offset.set(0.0);
            active.set(false);
            if let Some(to) = target
                && state.with_untracked(|panel| {
                    panel
                        .surfaces
                        .get(to)
                        .is_some_and(|surface| surface.id != id.get())
                })
            {
                host.apply(
                    &thread.get(),
                    Action::Move {
                        surface: id.get(),
                        to: to as isize,
                    },
                );
            }
        })
        .on_responder_terminate(move |_: &ResponderEvent| {
            offset.set(0.0);
            active.set(false);
        })
        .child(right_panel_tab(
            host,
            thread.get(),
            registry,
            state,
            surface,
        ))
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
                        I18nText::from(label).resolve().to_string(),
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
                        text(I18nText::from(label).resolve().to_string()),
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
        I18nText::from(kind.label()).resolve().to_string(),
        ItemOptions {
            disabled: Prop::bind(move || registry.reason(kind).is_some()),
            ..Default::default()
        },
        view()
            .app_class(tw("flex min-w-0 flex-1 items-center gap-2"))
            .child(text(I18nText::from(kind.label()).resolve().to_string()))
            .child(
                text(move || SharedString::from(registry.reason(kind).unwrap_or_default()))
                    .app_class(tw("text-muted-foreground text-xs")),
            )
            .child(menu::menu_shortcut(kind.shortcut().to_owned())),
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
                    nsh_base::button().accessibility_label(I18nText::from(kind.label()).resolve().to_string()).element_ref(&target)
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
                            .child(text(I18nText::from(kind.label()).resolve().to_string()).app_class(tw("min-w-0 flex-1 truncate")))
                            .child(text(move || SharedString::from(registry.reason(kind).unwrap_or_default())).app_class(tw("text-xs text-muted-foreground")))
                            .child(text(kind.shortcut().to_owned())))
                }))))
    )
}

fn surface_title(surface: &Surface, registry: PanelRegistry) -> String {
    if let Some(row) = SurfaceKind::of(surface).and_then(|kind| registry.registration(kind))
        && let Some(title) = &row.title
    {
        return title(surface).to_string();
    }
    match &surface.content {
        SurfaceContent::File { relative_path, .. } => relative_path
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(relative_path)
            .to_owned(),
        SurfaceContent::PullRequest { number, .. } => {
            tr("#{number}", &[("number", Value::Text(number.to_string()))])
        }
        SurfaceContent::PullRequests => t("Pull requests"),
        SurfaceContent::Device { target, title } => title
            .clone()
            .or_else(|| target.as_ref().map(|target| target.name.clone()))
            .unwrap_or_else(|| t("Device")),
        _ => SurfaceKind::of(surface)
            .map(|kind| I18nText::from(kind.label()).resolve().to_string())
            .unwrap_or_default(),
    }
}

fn surface_icon(surface: ReadSignal<Surface>, registry: PanelRegistry) -> impl IntoElement {
    dynamic(move || {
        let surface = surface.get();
        let row = SurfaceKind::of(&surface).and_then(|kind| registry.registration(kind));
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
            .child(row.as_ref().and_then(|row| row.glyph.as_ref()).map_or_else(
                || icon_with_class(which, "size-3 shrink-0").into_element(),
                |glyph| glyph(&surface),
            ))
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
