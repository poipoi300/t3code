//! Port of apps/web/src/components/ui/toast.tsx ToastProvider, Toasts and AnchoredToasts.
//!
//! Mount `toast_provider(children, Position::TopRight)` and `anchored_toast_provider(children)`
//! once inside the navigator context. `toast_manager()` and `anchored_toast_manager()` buffer
//! additions before mounting. `ToastManager::add`, `update` and `close` use stable `ToastId`s;
//! `dismiss` additionally invokes `ThreadToastData::on_close`, as the web close button does.
//! `Variant` names the web toast types; the web defines no toast size variant.
//! `ToastOptions` supplies title/description, action and priority; `ThreadToastData` supplies
//! thread scope, leading-icon/expandable slots, secondary actions and inline/stacked-end layout.
//! Strings are English source keys and are translated when displayed. Slots build fresh elements.
//! Optional title/description slots replace their text; keep the text fields for announcements.
//! `toasts`/`anchored_toasts` also accept an active-thread selector, for resolved draft routes.
//! An anchored toast needs a live `ElementRef`; copy helpers silently ignore a missing anchor.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use futures::{StreamExt, channel::mpsc};
use nsh_base::{Anchor, SwipeDirection, ToastMotion, ToastStackState};
use nsh_core::{platform::Lifecycle, prelude::*};
use t3_client::types::{ThreadId, ThreadKey};

use super::button::{ButtonContent, ButtonSize, ButtonVariant, button};
use super::icon::{Icon, icon};
use crate::i18n::t;
use crate::state::nav::Navigator;
use crate::theme::{cn, palette, tw};

pub type Slot = Rc<dyn Fn() -> AnyElement>;
pub type Callback = Rc<dyn Fn()>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Variant {
    #[default]
    Default,
    Error,
    Warning,
    Success,
    Info,
    Loading,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Position {
    TopLeft,
    TopCenter,
    #[default]
    TopRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
}

impl Position {
    fn anchor(self) -> Anchor {
        match self {
            Self::TopLeft => Anchor::TopLeft,
            Self::TopCenter => Anchor::TopCenter,
            Self::TopRight => Anchor::TopRight,
            Self::BottomLeft => Anchor::BottomLeft,
            Self::BottomCenter => Anchor::BottomCenter,
            Self::BottomRight => Anchor::BottomRight,
        }
    }

    fn swipe(self) -> SwipeDirection {
        match self {
            Self::TopLeft | Self::BottomLeft => SwipeDirection::Left,
            Self::TopRight | Self::BottomRight => SwipeDirection::Right,
            Self::TopCenter => SwipeDirection::Up,
            Self::BottomCenter => SwipeDirection::Down,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ActionLayout {
    #[default]
    Inline,
    StackedEnd,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Priority {
    #[default]
    Low,
    High,
}

#[derive(Clone)]
pub struct Action {
    pub label: String,
    pub disabled: bool,
    pub on_press: Callback,
}

#[derive(Clone, Default)]
pub struct ThreadToastData {
    pub thread_ref: Option<ThreadKey>,
    pub thread_id: Option<ThreadId>,
    pub leading_icon: Option<Slot>,
    pub tooltip_style: bool,
    pub on_close: Option<Callback>,
    pub dismiss_after_visible: Option<Duration>,
    pub hide_copy_button: bool,
    pub additional_actions: Vec<(String, Action)>,
    pub secondary_action: Option<Action>,
    pub secondary_action_variant: Option<ButtonVariant>,
    pub expandable_content: Option<Slot>,
    pub expand_label: Option<String>,
    pub collapse_label: Option<String>,
    pub expandable_description_trigger: bool,
    pub action_layout: ActionLayout,
    pub action_variant: Option<ButtonVariant>,
}

#[derive(Clone)]
pub struct ToastOptions {
    pub variant: Variant,
    pub title: String,
    pub description: Option<String>,
    pub title_content: Option<Slot>,
    pub description_content: Option<Slot>,
    /// `None` is persistent (the web's `timeout: 0`).
    pub timeout: Option<Duration>,
    /// NSH currently announces both priorities through its alert toast primitive.
    pub priority: Priority,
    pub action: Option<Action>,
    pub data: ThreadToastData,
    pub anchor: Option<ElementRef>,
    pub side_offset: f32,
}

impl Default for ToastOptions {
    fn default() -> Self {
        Self {
            variant: Variant::Default,
            title: String::new(),
            description: None,
            title_content: None,
            description_content: None,
            timeout: Some(Duration::from_secs(5)),
            priority: Priority::Low,
            action: None,
            data: ThreadToastData::default(),
            anchor: None,
            side_offset: 4.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ToastId(u64);

#[derive(Default)]
struct ManagerState {
    next: u64,
    entries: Vec<Entry>,
    listeners: Vec<mpsc::UnboundedSender<Vec<Entry>>>,
}

#[derive(Clone)]
struct Entry {
    id: ToastId,
    options: Rc<ToastOptions>,
    remaining: Rc<RefCell<VisibleTimeout>>,
    ending: bool,
}

impl PartialEq for Entry {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.ending == other.ending
            && Rc::ptr_eq(&self.options, &other.options)
    }
}

#[derive(Clone, Default)]
pub struct ToastManager(Rc<RefCell<ManagerState>>);

impl ManagerState {
    fn notify(&mut self) {
        let entries = &self.entries;
        self.listeners
            .retain_mut(|listener| listener.unbounded_send(entries.clone()).is_ok());
    }
}

impl ToastManager {
    pub fn add(&self, options: ToastOptions) -> ToastId {
        let mut state = self.0.borrow_mut();
        state.next += 1;
        let id = ToastId(state.next);
        state.entries.push(entry(id, options));
        state.notify();
        id
    }

    /// Replaces all options, including cleared actions, without changing the toast's order.
    pub fn update(&self, id: ToastId, options: ToastOptions) -> bool {
        let mut state = self.0.borrow_mut();
        let Some(current) = state.entries.iter_mut().find(|entry| entry.id == id) else {
            return false;
        };
        *current = entry(id, options);
        state.notify();
        true
    }

    pub fn close(&self, id: ToastId) -> bool {
        let mut state = self.0.borrow_mut();
        let before = state.entries.len();
        state.entries.retain(|entry| entry.id != id);
        let changed = state.entries.len() != before;
        if changed {
            state.notify();
        }
        changed
    }

    /// Explicit dismissal invokes `on_close`; timeout and programmatic close do not.
    pub fn dismiss(&self, id: ToastId) {
        let callback = self
            .0
            .borrow()
            .entries
            .iter()
            .find(|entry| entry.id == id)
            .and_then(|entry| entry.options.data.on_close.clone());
        if let Some(callback) = callback {
            callback();
        }
        self.close(id);
    }

    fn subscribe(&self) -> (Vec<Entry>, mpsc::UnboundedReceiver<Vec<Entry>>) {
        let (sender, receiver) = mpsc::unbounded();
        let mut state = self.0.borrow_mut();
        state.listeners.push(sender);
        (state.entries.clone(), receiver)
    }
}

fn entry(id: ToastId, options: ToastOptions) -> Entry {
    let remaining = VisibleTimeout::new(options.timeout, options.data.dismiss_after_visible);
    Entry {
        id,
        options: Rc::new(options),
        remaining: Rc::new(RefCell::new(remaining)),
        ending: false,
    }
}

thread_local! {
    static MANAGER: ToastManager = ToastManager::default();
    static ANCHORED_MANAGER: ToastManager = ToastManager::default();
}

pub fn toast_manager() -> ToastManager {
    MANAGER.with(Clone::clone)
}

pub fn anchored_toast_manager() -> ToastManager {
    ANCHORED_MANAGER.with(Clone::clone)
}

/// Port of apps/web/src/components/ui/toastHelpers.ts stackedThreadToast.
pub fn stacked_thread_toast(mut options: ToastOptions) -> ToastOptions {
    options.data.action_layout = ActionLayout::StackedEnd;
    options
}

pub const ANCHORED_COPY_TOAST_TIMEOUT_MS: u64 = 1000;

/// Port of apps/web/src/components/ui/anchoredCopyToast.ts showAnchoredCopySuccessToast.
pub fn show_anchored_copy_success_toast(anchor: Option<&ElementRef>) -> Option<ToastId> {
    let anchor = *anchor?;
    anchor.node()?;
    Some(anchored_toast_manager().add(ToastOptions {
        title: "Copied!".into(),
        timeout: Some(Duration::from_millis(ANCHORED_COPY_TOAST_TIMEOUT_MS)),
        anchor: Some(anchor),
        data: ThreadToastData {
            tooltip_style: true,
            ..Default::default()
        },
        ..Default::default()
    }))
}

/// Port of apps/web/src/components/ui/anchoredCopyToast.ts showAnchoredCopyErrorToast.
pub fn show_anchored_copy_error_toast(anchor: Option<&ElementRef>, error: &str) -> Option<ToastId> {
    let anchor = *anchor?;
    anchor.node()?;
    Some(anchored_toast_manager().add(ToastOptions {
        title: "Failed to copy".into(),
        variant: Variant::Error,
        description: Some(error.into()),
        anchor: Some(anchor),
        ..Default::default()
    }))
}

/// Port of apps/web/src/components/ui/toast.logic.ts shouldRenderThreadScopedToast.
pub fn should_render_thread_scoped_toast(
    data: &ThreadToastData,
    active: Option<&ThreadKey>,
) -> bool {
    if let Some(thread) = &data.thread_ref {
        return active == Some(thread);
    }
    data.thread_id
        .as_ref()
        .filter(|id| !id.0.is_empty())
        .is_none_or(|id| active.is_some_and(|key| &key.thread == id))
}

/// Port of apps/web/src/components/ui/toast.logic.ts shouldHideCollapsedToastContent.
pub fn should_hide_collapsed_toast_content(index: usize, count: usize) -> bool {
    count > 1 && index > 0
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LayoutInput {
    pub height: f32,
    pub ending: bool,
}

#[derive(Debug, PartialEq)]
pub struct VisibleToastLayout {
    pub frontmost_height: f32,
    pub items: Vec<(usize, f32)>,
}

/// Port of apps/web/src/components/ui/toast.logic.ts buildVisibleToastLayout; newest first.
pub fn build_visible_toast_layout(toasts: &[LayoutInput]) -> VisibleToastLayout {
    let height = |value: f32| {
        if value.is_finite() && value > 0.0 {
            value
        } else {
            0.0
        }
    };
    let (mut full_index, mut full_offset, mut live_index, mut live_offset) = (0, 0.0, 0, 0.0);
    let items = toasts
        .iter()
        .map(|toast| {
            let item = if toast.ending {
                (full_index, full_offset)
            } else {
                (live_index, live_offset)
            };
            full_index += 1;
            full_offset += height(toast.height);
            if !toast.ending {
                live_index += 1;
                live_offset += height(toast.height);
            }
            item
        })
        .collect();
    VisibleToastLayout {
        frontmost_height: toasts
            .iter()
            .find(|toast| !toast.ending)
            .map_or(0.0, |toast| height(toast.height)),
        items,
    }
}

#[derive(Debug)]
struct VisibleTimeout {
    remaining: Option<Duration>,
    started: Option<nsh_core::time::Instant>,
}

impl VisibleTimeout {
    fn new(timeout: Option<Duration>, visible: Option<Duration>) -> Self {
        let timeout = timeout.filter(|value| !value.is_zero());
        let visible = visible.filter(|value| !value.is_zero());
        Self {
            remaining: match (timeout, visible) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            },
            started: None,
        }
    }

    fn pause(&mut self, now: nsh_core::time::Instant) {
        if let (Some(started), Some(remaining)) = (self.started.take(), &mut self.remaining) {
            *remaining = remaining.saturating_sub(now.saturating_duration_since(started));
        }
    }
}

fn active_thread() -> impl Fn() -> Option<ThreadKey> + Clone {
    let nav = use_context::<Navigator>();
    move || nav.and_then(|nav| nav.top().thread().cloned())
}

pub fn toast_provider(children: impl IntoElement, position: Position) -> impl IntoElement {
    fragment(vec![
        children.into_element(),
        toasts(toast_manager(), position, active_thread()).into_element(),
    ])
}

pub fn anchored_toast_provider(children: impl IntoElement) -> impl IntoElement {
    fragment(vec![
        children.into_element(),
        anchored_toasts(anchored_toast_manager(), active_thread()).into_element(),
    ])
}

fn fragment(children: Vec<AnyElement>) -> AnyElement {
    let children = RefCell::new(Some(children));
    dynamic(move || children.borrow_mut().take().unwrap_or_default())
}

fn subscribe(manager: &ToastManager) -> Signal<Vec<Entry>> {
    let (initial, mut updates) = manager.subscribe();
    let entries = signal(initial);
    spawn(async move {
        while let Some(mut next) = updates.next().await {
            for (index, old) in entries.get_untracked().into_iter().enumerate() {
                if next.iter().any(|entry| entry.id == old.id) {
                    continue;
                }
                let mut exiting = old.clone();
                exiting.ending = true;
                next.insert(index.min(next.len()), exiting);
                if !old.ending {
                    let delay = if reduce_motion() {
                        Duration::ZERO
                    } else {
                        Duration::from_millis(500)
                    };
                    set_timeout(delay, move || {
                        entries.update(|shown| shown.retain(|entry| entry.id != old.id))
                    });
                }
            }
            entries.set(next);
        }
    });
    entries
}

/// `Toasts`; `active` must include the environment, including for resolved draft sessions.
pub fn toasts(
    manager: ToastManager,
    position: Position,
    active: impl Fn() -> Option<ThreadKey> + 'static,
) -> impl IntoElement {
    let entries = subscribe(&manager);
    let visible = computed(move || {
        let active = active();
        entries
            .get()
            .into_iter()
            .filter(|entry| should_render_thread_scoped_toast(&entry.options.data, active.as_ref()))
            .collect::<Vec<_>>()
    });
    let stack = ToastStackState::new();
    let focused = signal(false);
    let focus = element_ref();
    focus.on_focus_in(move || focused.set(true));
    focus.on_focus_out(move || focused.set(false));
    let cancel = manager.clone();
    // A portal covers the root; its empty space must pass input through to the app.
    overlay()
        .absolute()
        .inset_0()
        .pointer_events(nsh_core::PointerEvents::BoxNone)
        .test_id("toast-portal")
        .child(
            nsh_base::toast_stack()
                .pointer_events(nsh_core::PointerEvents::Auto)
                .element_ref(&focus)
                .placement(position.anchor())
                .state(stack)
                // NSH measures and resolves the web's per-toast CSS variables in ToastStack.
                .motion(ToastMotion {
                    collapsed_peek: px(12.0),
                    expanded_gap: px(12.0),
                    collapsed_scale_step: 0.1,
                    ..ToastMotion::sonner()
                })
                .app_class(tw("absolute z-100 mx-auto max-w-90"))
                .w(move || px((window().width.0 - inset() * 2.0).clamp(0.0, 360.0)))
                .top(move || {
                    if is_top(position) {
                        Length::from(px(inset()
                            + safe_area().top.0
                            + tokens().number("workspace-topbar-height").unwrap_or(32.0)))
                    } else {
                        auto()
                    }
                })
                .bottom(move || {
                    if is_top(position) {
                        auto()
                    } else {
                        Length::from(px(inset() + safe_area().bottom.0))
                    }
                })
                .left(move || edge(position, true))
                .right(move || edge(position, false))
                .on_cancel(move || {
                    if let Some(entry) = visible.get_untracked().last() {
                        cancel.close(entry.id);
                    }
                })
                .child(each(
                    visible,
                    |entry| entry.id,
                    move |item, index| {
                        let manager = manager.clone();
                        let expanded = move || stack.is_expanded() || focused.get();
                        toast_root(item, manager, Some(position), expanded, move || {
                            let count = visible.with(Vec::len);
                            should_hide_collapsed_toast_content(
                                count.saturating_sub(index.get() + 1),
                                count,
                            ) && !expanded()
                        })
                    },
                )),
        )
}

fn inset() -> f32 {
    if window().width.0 < 640.0 { 16.0 } else { 32.0 }
}
fn is_top(position: Position) -> bool {
    matches!(
        position,
        Position::TopLeft | Position::TopCenter | Position::TopRight
    )
}
fn edge(position: Position, left: bool) -> Length {
    match position {
        Position::TopCenter | Position::BottomCenter => {
            px((window().width.0 - (window().width.0 - inset() * 2.0).clamp(0.0, 360.0)) / 2.0)
                .into()
        }
        Position::TopLeft | Position::BottomLeft if left => px(inset()).into(),
        Position::TopRight | Position::BottomRight if !left => px(inset()).into(),
        _ => auto(),
    }
}

pub fn anchored_toasts(
    manager: ToastManager,
    active: impl Fn() -> Option<ThreadKey> + 'static,
) -> impl IntoElement {
    let entries = subscribe(&manager);
    let visible = computed(move || {
        let active = active();
        entries
            .get()
            .into_iter()
            .filter(|entry| {
                entry
                    .options
                    .anchor
                    .is_some_and(|anchor| anchor.node().is_some())
                    && should_render_thread_scoped_toast(&entry.options.data, active.as_ref())
            })
            .collect::<Vec<_>>()
    });
    view().test_id("toast-viewport-anchored").child(each(
        visible,
        |entry| entry.id,
        move |item, _| {
            let anchor = item
                .get_untracked()
                .options
                .anchor
                .expect("filtered anchor");
            let hovered = signal(false);
            let focused = signal(false);
            let focus = element_ref();
            focus.on_focus_in(move || focused.set(true));
            focus.on_focus_out(move || focused.set(false));
            overlay()
                .anchor(&anchor)
                .placement(Placement::Bottom)
                .offset(move || px(item.get().options.side_offset))
                .app_class(tw("z-100 max-w-64"))
                .element_ref(&focus)
                .on_mouse_enter(move |_: &MouseEvent| hovered.set(true))
                .on_mouse_leave(move |_: &MouseEvent| hovered.set(false))
                .child(toast_root(
                    item,
                    manager.clone(),
                    None,
                    move || hovered.get() || focused.get(),
                    || false,
                ))
        },
    ))
}

fn thread_toast_visible_auto_dismiss(
    item: ReadSignal<Entry>,
    manager: ToastManager,
    paused: impl Fn() -> bool + 'static,
) {
    effect(move || {
        let entry = item.get();
        let paused = paused() || lifecycle() != Lifecycle::Active || entry.ending;
        let now = nsh_core::time::now();
        entry.remaining.borrow_mut().pause(now);
        if paused {
            return;
        }
        let remaining = entry.remaining.borrow().remaining;
        if let Some(remaining) = remaining {
            entry.remaining.borrow_mut().started = Some(now);
            let close = manager.clone();
            let id = entry.id;
            let timer = set_timeout(remaining, move || {
                close.close(id);
            });
            cleanup(move || {
                timer.cancel();
                entry.remaining.borrow_mut().pause(nsh_core::time::now());
            });
        }
    });
}

fn toast_root(
    item: ReadSignal<Entry>,
    manager: ToastManager,
    position: Option<Position>,
    paused: impl Fn() -> bool + 'static,
    hidden: impl Fn() -> bool + 'static,
) -> impl IntoElement {
    thread_toast_visible_auto_dismiss(item, manager.clone(), paused);
    let dismiss = manager.clone();
    let tooltip = move || item.get().options.data.tooltip_style && position.is_none();
    let hidden = Rc::new(hidden);
    let body_state = BodyState {
        description_open: signal(false),
        section_open: signal(false),
    };
    let announced = stored(None::<String>);
    effect(move || {
        let message = announcement(&item.get().options);
        if announced
            .replace(Some(message.clone()))
            .is_some_and(|previous| previous != message)
        {
            announce_for_accessibility(message);
        }
    });
    nsh_base::toast()
        .pointer_events(move || if item.get().ending { nsh_core::PointerEvents::None } else { nsh_core::PointerEvents::Auto })
        .transition_status(move || if item.get().ending { nsh_base::ToastTransitionStatus::Ending } else { nsh_base::ToastTransitionStatus::Starting })
        .starting_class(move || Some(tw(if reduce_motion() { "opacity-0" } else { "opacity-0 scale-98" })))
        .ending_class(move || Some(tw(if reduce_motion() { "opacity-0" } else { "opacity-0 scale-98" })))
        .swipe_direction(position.map(Position::swipe))
        .accessibility_label(move || Some(SharedString::from(announcement(&item.get().options))))
        .on_dismiss_request(move || { dismiss.close(item.get_untracked().id); })
        // index.css @utility dropdown-glass: color-mix with transparent reduces to alpha.
        .app_class(move || tw(&cn([
            Some("relative overflow-visible text-popover-foreground shadow-xl shadow-black/25 border border-contrast-foreground/10"),
            (!reduce_motion()).then_some("transition-[scale,opacity] duration-500"),
            Some(if tooltip() { "rounded-md text-xs" } else { "rounded-lg" }),
        ])))
        // The frozen theme seam omits numeric/percentage tokens; stock index.css values until it exports them.
        .bg(move || palette().popover.opacity(0.18 + 0.82 * tokens().number("glass-opacity").unwrap_or(80.0) / 100.0))
        .style_with(move |style| style
            .backdrop_blur(px(tokens().number("glass-blur").unwrap_or(12.0)))
            .backdrop_saturate(tokens().number("glass-saturation").unwrap_or(if dark() { 1.08 } else { 1.14 })))
        .child(dynamic(move || {
            let entry = item.get();
            let manager = manager.clone();
            let hidden = hidden.clone();
            let inert = hidden.clone();
            untracked(|| {
                if tooltip() {
                    let title = entry.options.title.clone();
                    view().app_class(tw("pointer-events-auto px-2 py-1"))
                        .child(entry.options.title_content.as_ref().map(|slot| slot()).unwrap_or_else(|| text(move || SharedString::from(t(&title))).test_id("toast-title").into_element())).into_element()
                } else {
                    let id = entry.id;
                    let options = entry.options.clone();
                    let descriptor = derive_toast_body_descriptor(&options);
                    let close = manager.clone();
                    view().child(
                        view().app_class(tw("absolute z-20 -top-1.5 -right-1.5")).child(
                            nsh_base::button().accessibility_label(Some(SharedString::from(t("Dismiss notification"))))
                                .test_id("toast-close")
                                .app_class(tw("inline-flex size-6 shrink-0 cursor-pointer items-center justify-center rounded-full border border-border/60 bg-popover/92 text-muted-foreground shadow-sm outline-none backdrop-blur-sm transition-[color,background-color,box-shadow] hover:bg-popover hover:text-foreground focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-background"))
                                .on_press(move || close.dismiss(id))
                                .child(icon(Icon::X, 12.0, || palette().muted_foreground))
                        )
                    ).child(view().app_class(tw(&cn([
                        // [overflow-x:clip] has no NSH equivalent; preserve visible detail panels.
                        Some("pointer-events-auto min-h-0 overflow-y-visible pl-3.5 text-sm"),
                        Some(if descriptor.stacked { "flex flex-col gap-2 py-2.5 pr-3.5" } else if descriptor.trailing { "py-3 flex items-center justify-between gap-1.5 pr-6" } else { "py-3 flex items-center justify-between gap-1.5 pr-3.5" }),
                    ])))
                        .opacity(move || if hidden() { 0.0_f32 } else { 1.0 })
                        .pointer_events(move || if inert() { nsh_core::PointerEvents::None } else { nsh_core::PointerEvents::Auto })
                        .child(toast_body_content(options, descriptor, manager, id, body_state)))
                    .into_element()
                }
            })
        }))
        // The web's after: pseudo-element bridges its 12px stack gap for hover interaction.
        .child(position.map(|position| view().app_class(tw(if is_top(position) {
            "absolute left-0 h-[13px] w-full top-full"
        } else { "absolute left-0 h-[13px] w-full bottom-full" }))))
}

fn announcement(options: &ToastOptions) -> String {
    match &options.description {
        Some(description) => format!("{}: {}", t(&options.title), t(description)),
        None => t(&options.title),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BodyDescriptor {
    stacked: bool,
    copy_error: bool,
    trailing: bool,
}

#[derive(Clone, Copy)]
struct BodyState {
    description_open: Signal<bool>,
    section_open: Signal<bool>,
}

fn has_visible_toast_action(action: Option<&Action>) -> bool {
    action.is_some_and(|action| !action.label.is_empty())
}

fn derive_toast_body_descriptor(options: &ToastOptions) -> BodyDescriptor {
    let action = has_visible_toast_action(options.action.as_ref());
    let copy_error = options.variant == Variant::Error
        && options.description.is_some()
        && options.description_content.is_none()
        && !options.data.hide_copy_button;
    BodyDescriptor {
        stacked: action && options.data.action_layout == ActionLayout::StackedEnd,
        copy_error,
        trailing: copy_error
            || action
            || !options.data.additional_actions.is_empty()
            || options.data.secondary_action.is_some(),
    }
}

fn toast_body_content(
    options: Rc<ToastOptions>,
    body: BodyDescriptor,
    manager: ToastManager,
    id: ToastId,
    state: BodyState,
) -> impl IntoElement {
    let leading = options.data.leading_icon.as_ref().map(|slot| slot());
    // Selector classes move to their icon child; h-lh is the 20px text-sm line height.
    let glyph = if leading.is_some() {
        None
    } else {
        toast_icon(options.variant)
    };
    let controls = options.clone();
    let main = view()
        .app_class(tw(&cn([
            Some("flex min-h-0 min-w-0 flex-col gap-0.5"),
            Some(if body.stacked { "pr-5" } else { "flex-1" }),
        ])))
        .child(
            view()
                .app_class(tw(&cn([
                    Some("flex min-w-0 gap-2"),
                    (!body.stacked && !body.trailing).then_some("pr-6"),
                ])))
                .child(leading.map(|leading| {
                    view()
                        .app_class(tw("flex h-5 w-4 shrink-0 items-center justify-center"))
                        .child(leading)
                }))
                .child(glyph.map(|glyph| view().app_class(tw("h-5 w-4 shrink-0")).child(glyph)))
                // wrap-break-word is omitted: NSH preserves newlines and wraps at words.
                .child(toast_title(options.clone())),
        )
        .child(toast_description_and_expandable(options, state));
    let trailing = body.trailing.then(|| {
        view()
            .app_class(tw(&cn([
                Some("flex items-center gap-1.5"),
                Some(if body.stacked {
                    "w-full justify-end"
                } else {
                    "shrink-0"
                }),
            ])))
            .child(
                body.copy_error
                    .then(|| copy_error_button(controls.description.clone().unwrap_or_default())),
            )
            .children(controls.data.additional_actions.iter().map(|(_, action)| {
                toast_action(
                    action,
                    controls
                        .data
                        .secondary_action_variant
                        .unwrap_or(ButtonVariant::Outline),
                    None,
                )
            }))
            .child(controls.data.secondary_action.as_ref().map(|action| {
                toast_action(
                    action,
                    controls
                        .data
                        .secondary_action_variant
                        .unwrap_or(ButtonVariant::Outline),
                    None,
                )
            }))
            .child(
                controls
                    .action
                    .as_ref()
                    .filter(|action| has_visible_toast_action(Some(action)))
                    .map(|action| {
                        toast_action(
                            action,
                            controls.data.action_variant.unwrap_or_default(),
                            Some((manager, id)),
                        )
                    }),
            )
    });
    let mut children = vec![main.into_element()];
    if let Some(trailing) = trailing {
        children.push(trailing.into_element());
    }
    fragment(children)
}

fn toast_icon(variant: Variant) -> Option<AnyElement> {
    // Lucide 0.564.0's matching icon nodes, inline until UI-ICONS exposes these assets.
    let nodes = match variant {
        Variant::Default => return None,
        Variant::Error => {
            r#"<circle cx="12" cy="12" r="10"/><path d="M12 8v4"/><path d="M12 16h.01"/>"#
        }
        Variant::Info => {
            r#"<circle cx="12" cy="12" r="10"/><path d="M12 16v-4"/><path d="M12 8h.01"/>"#
        }
        Variant::Success => r#"<circle cx="12" cy="12" r="10"/><path d="m9 12 2 2 4-4"/>"#,
        Variant::Warning => {
            r#"<path d="m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3"/><path d="M12 9v4"/><path d="M12 17h.01"/>"#
        }
        // The shared UI forbids continuously repainting loading animations.
        Variant::Loading => r#"<path d="M21 12a9 9 0 1 1-6.219-8.56"/>"#,
    };
    let markup = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">{nodes}</svg>"#
    );
    Some(
        svg(nsh_core::element::ImageSrc::svg_markup(&markup))
            .w(px(16.0))
            .h(px(20.0))
            .flex_shrink_0()
            .accessible(false)
            .opacity(if variant == Variant::Loading {
                0.8_f32
            } else {
                1.0
            })
            .text_color(move || match variant {
                Variant::Error => palette().destructive,
                Variant::Info => palette().info,
                Variant::Success => palette().success,
                Variant::Warning => palette().warning,
                _ => palette().popover_foreground,
            })
            .into_element(),
    )
}

fn toast_title(options: Rc<ToastOptions>) -> impl IntoElement {
    view()
        .test_id("toast-title")
        .app_class(tw("min-w-0 font-medium"))
        .child(if let Some(slot) = &options.title_content {
            slot()
        } else {
            text(move || SharedString::from(t(&options.title))).into_element()
        })
}

fn toast_action(
    action: &Action,
    variant: ButtonVariant,
    close: Option<(ToastManager, ToastId)>,
) -> impl IntoElement {
    let action = action.clone();
    let disabled = action.disabled;
    button(
        ButtonContent::Label(t(&action.label)),
        variant,
        ButtonSize::Xs,
        move || disabled,
        move || {
            if action.disabled {
                return;
            }
            (action.on_press)();
            if let Some((manager, id)) = &close {
                manager.close(*id);
            }
        },
    )
}

fn copy_error_button(source: String) -> impl IntoElement {
    // ButtonSize lacks icon-micro; the source's fixed equivalent is a 20px target and 12px glyph.
    // NSH's clipboard write is fire-and-forget: do not claim success without an acknowledgement.
    nsh_base::button().accessibility_label(Some(SharedString::from(t("Copy error"))))
        .app_class(tw("inline-flex size-5 shrink-0 items-center justify-center rounded-md text-muted-foreground/80 hover:bg-transparent hover:text-muted-foreground focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"))
        .hit_slop(move || EdgeInsets::all(px(((crate::theme::metrics().hit_target - 20.0) / 2.0).max(0.0))))
        .on_press(move || {
            if !source.is_empty() { set_clipboard(source.clone()); }
        })
        .child(icon(Icon::Copy, 12.0, || palette().muted_foreground))
}

fn error_description_clamped(options: &ToastOptions) -> bool {
    options.variant == Variant::Error
        && options.description_content.is_none()
        && options
            .description
            .as_ref()
            .is_some_and(|description| description.encode_utf16().count() >= 180)
}

fn description(options: &ToastOptions) -> impl IntoElement {
    let clamped = error_description_clamped(options);
    let source = options.description.clone().unwrap_or_default();
    if let Some(slot) = &options.description_content {
        return view()
            .test_id("toast-description")
            .app_class(tw("min-w-0 select-text text-muted-foreground"))
            .child(slot())
            .into_element();
    }
    if source.is_empty() {
        return fragment(Vec::new());
    }
    text(move || SharedString::from(t(&source)))
        .test_id("toast-description")
        .select_text()
        .app_class(tw("min-w-0 select-text text-muted-foreground"))
        .number_of_lines(if clamped { 4 } else { 0 })
        .into_element()
}

fn toast_description_and_expandable(
    options: Rc<ToastOptions>,
    state: BodyState,
) -> impl IntoElement {
    if options.data.expandable_content.is_none() {
        return description(&options).into_element();
    }
    if !options.data.expandable_description_trigger {
        return fragment(vec![
            description(&options).into_element(),
            toast_expandable_section(options, state.section_open).into_element(),
        ]);
    }
    let open = state.description_open;
    let labels = options.clone();
    let content = options.data.expandable_content.clone();
    fragment(vec![
        nsh_base::button()
            .accessibility_label(move || Some(SharedString::from(expand_label(&labels, open.get()))))
            .app_class(tw("group flex min-w-0 w-full cursor-pointer select-none items-start gap-1.5 rounded-sm text-left outline-none ring-offset-background transition-colors hover:bg-muted/40 focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-background"))
            .on_press(move || open.update(|value| *value = !*value))
            .child(view().app_class(tw("min-w-0 flex-1")).child(description(&options)))
            .child(expand_chevron(open)).into_element(),
        dynamic(move || open.get().then(|| expandable_panel(content.as_ref()).into_element()))
    ])
}

fn expand_label(options: &ToastOptions, open: bool) -> String {
    t(if open {
        options
            .data
            .collapse_label
            .as_deref()
            .unwrap_or("Hide details")
    } else {
        options
            .data
            .expand_label
            .as_deref()
            .unwrap_or("Show details")
    })
}

fn expand_chevron(open: Signal<bool>) -> impl IntoElement {
    // ChevronUp is absent from UI-ICONS; rotate the matching down chevron.
    view()
        .app_class(tw("size-3.5 shrink-0 opacity-80"))
        .rotate(move || {
            radians(if open.get() {
                std::f32::consts::PI
            } else {
                0.0
            })
        })
        .child(icon(Icon::ChevronDown, 14.0, || palette().muted_foreground))
}

fn toast_expandable_section(options: Rc<ToastOptions>, open: Signal<bool>) -> impl IntoElement {
    let labels = options.clone();
    let content = options.data.expandable_content.clone();
    view().app_class(tw("min-w-0"))
        .child(nsh_base::button()
            .accessibility_label(move || Some(SharedString::from(expand_label(&labels, open.get()))))
            .app_class(tw("inline-flex cursor-pointer items-center gap-1 rounded-md py-0.5 text-left text-xs font-medium text-muted-foreground transition-colors hover:text-foreground focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"))
            .on_press(move || open.update(|value| *value = !*value))
            .child(expand_chevron(open))
            .child(text(move || SharedString::from(expand_label(&options, open.get())))))
        .child(dynamic(move || open.get().then(|| expandable_panel(content.as_ref()).into_element())))
}

fn expandable_panel(content: Option<&Slot>) -> impl IntoElement {
    // overflow-y-auto becomes scroll(); overscroll-contain has no NSH meaning.
    scroll()
        .app_class(tw("mt-2 max-h-40 min-h-0 pr-0.5 select-text"))
        .child(content.map(|slot| slot()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use t3_client::types::EnvironmentId;

    fn key(environment: &str, thread: &str) -> ThreadKey {
        ThreadKey {
            environment: EnvironmentId(environment.into()),
            thread: ThreadId(thread.into()),
        }
    }

    #[test]
    fn scoped_refs_take_precedence_over_legacy_ids_and_never_cross_environments() {
        let mut data = ThreadToastData::default();
        assert!(should_render_thread_scoped_toast(&data, None));
        data.thread_id = Some(ThreadId(String::new()));
        assert!(should_render_thread_scoped_toast(&data, None));
        data.thread_id = Some(ThreadId("t".into()));
        assert!(!should_render_thread_scoped_toast(&data, None));
        assert!(should_render_thread_scoped_toast(
            &data,
            Some(&key("a", "t"))
        ));
        data.thread_ref = Some(key("b", "t"));
        assert!(!should_render_thread_scoped_toast(
            &data,
            Some(&key("a", "t"))
        ));
        assert!(should_render_thread_scoped_toast(
            &data,
            Some(&key("b", "t"))
        ));
        assert!(!should_render_thread_scoped_toast(
            &data,
            Some(&key("b", "u"))
        ));
    }

    #[test]
    fn closing_rows_do_not_leave_a_gap_in_the_live_stack() {
        let layout = build_visible_toast_layout(&[
            LayoutInput {
                height: 45.0,
                ending: true,
            },
            LayoutInput {
                height: 60.0,
                ending: false,
            },
            LayoutInput {
                height: 35.0,
                ending: true,
            },
            LayoutInput {
                height: 70.0,
                ending: false,
            },
        ]);
        assert_eq!(layout.frontmost_height, 60.0);
        assert_eq!(
            layout.items,
            vec![(0, 0.0), (0, 0.0), (2, 105.0), (1, 60.0)]
        );
        assert!(!should_hide_collapsed_toast_content(0, 4));
        assert!(should_hide_collapsed_toast_content(1, 4));
        assert!(!should_hide_collapsed_toast_content(1, 1));
        assert_eq!(build_visible_toast_layout(&[]).frontmost_height, 0.0);
        for height in [f32::NAN, f32::INFINITY, -1.0, 0.0] {
            let layout = build_visible_toast_layout(&[LayoutInput {
                height,
                ending: false,
            }]);
            assert_eq!(layout.frontmost_height, 0.0);
            assert_eq!(layout.items, vec![(0, 0.0)]);
        }
    }

    #[test]
    fn empty_primary_actions_clear_stacking_without_hiding_secondary_controls() {
        let action = Action {
            label: "Retry".into(),
            disabled: false,
            on_press: Rc::new(|| {}),
        };
        let mut options = stacked_thread_toast(ToastOptions {
            action: Some(action.clone()),
            ..Default::default()
        });
        assert!(derive_toast_body_descriptor(&options).stacked);
        options.action.as_mut().expect("action").label.clear();
        assert_eq!(
            derive_toast_body_descriptor(&options),
            BodyDescriptor {
                stacked: false,
                copy_error: false,
                trailing: false
            }
        );
        options.data.secondary_action = Some(action);
        assert_eq!(
            derive_toast_body_descriptor(&options),
            BodyDescriptor {
                stacked: false,
                copy_error: false,
                trailing: true
            }
        );
        options.variant = Variant::Error;
        options.description = Some("".into());
        assert!(derive_toast_body_descriptor(&options).copy_error);
        options.data.hide_copy_button = true;
        assert!(!derive_toast_body_descriptor(&options).copy_error);
        options.data.hide_copy_button = false;
        options.description_content = Some(Rc::new(|| text(t("Rich description")).into_element()));
        assert!(!derive_toast_body_descriptor(&options).copy_error);
    }

    #[test]
    fn buffered_updates_and_closes_are_replayed_as_current_state_to_each_new_host() {
        let manager = ToastManager::default();
        let id = manager.add(ToastOptions {
            title: "Before mount".into(),
            ..Default::default()
        });
        assert!(manager.update(
            id,
            ToastOptions {
                title: "Updated before mount".into(),
                ..Default::default()
            }
        ));
        let (initial, mut updates) = manager.subscribe();
        assert_eq!(initial.len(), 1);
        assert_eq!(initial[0].id, id);
        assert_eq!(initial[0].options.title, "Updated before mount");
        let second = manager.add(ToastOptions::default());
        assert_eq!(
            futures::executor::block_on(updates.next())
                .expect("addition")
                .len(),
            2
        );
        assert!(manager.close(id));
        let next = futures::executor::block_on(updates.next()).expect("close");
        assert_eq!(next.len(), 1);
        assert_eq!(next[0].id, second);
        assert!(!manager.update(id, ToastOptions::default()));
        assert!(!manager.close(id));
        assert_eq!(manager.subscribe().0.len(), 1);
    }

    #[test]
    fn explicit_dismissal_calls_on_close_outside_the_manager_borrow() {
        let manager = ToastManager::default();
        let calls = Rc::new(Cell::new(0));
        let observed = calls.clone();
        let reentrant = manager.clone();
        let id = manager.add(ToastOptions {
            data: ThreadToastData {
                on_close: Some(Rc::new(move || {
                    observed.set(observed.get() + 1);
                    reentrant.add(ToastOptions {
                        title: "Follow-up".into(),
                        ..Default::default()
                    });
                })),
                ..Default::default()
            },
            ..Default::default()
        });
        manager.dismiss(id);
        manager.dismiss(id);
        assert_eq!(calls.get(), 1);
        let remaining = manager.subscribe().0;
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].options.title, "Follow-up");
        manager.close(remaining[0].id);
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn visible_timeout_retains_only_the_unspent_budget_across_pause_and_remount() {
        let clock = nsh_core::time::Clock::manual(std::time::SystemTime::UNIX_EPOCH);
        let start = clock.now();
        let mut timeout = VisibleTimeout::new(None, Some(Duration::from_secs(5)));
        timeout.started = Some(start);
        timeout.pause(start + Duration::from_secs(2));
        assert_eq!(timeout.remaining, Some(Duration::from_secs(3)));
        timeout.pause(start + Duration::from_secs(100));
        assert_eq!(timeout.remaining, Some(Duration::from_secs(3)));
        timeout.started = Some(start + Duration::from_secs(100));
        timeout.pause(start + Duration::from_secs(104));
        assert_eq!(timeout.remaining, Some(Duration::ZERO));
        assert_eq!(
            VisibleTimeout::new(Some(Duration::ZERO), None).remaining,
            None
        );
        assert_eq!(
            VisibleTimeout::new(Some(Duration::from_secs(2)), Some(Duration::from_secs(5)))
                .remaining,
            Some(Duration::from_secs(2))
        );
    }

    #[test]
    fn long_error_clamping_uses_the_webs_utf16_length_and_only_error_descriptions() {
        let mut options = ToastOptions {
            variant: Variant::Error,
            description: Some("x".repeat(179)),
            ..Default::default()
        };
        assert!(!error_description_clamped(&options));
        options.description = Some("🦀".repeat(90));
        assert!(error_description_clamped(&options));
        options.variant = Variant::Info;
        assert!(!error_description_clamped(&options));
    }

    #[test]
    fn anchored_copy_without_a_mounted_target_does_not_enqueue_feedback() {
        assert!(show_anchored_copy_success_toast(None).is_none());
        assert!(show_anchored_copy_error_toast(None, "Clipboard denied").is_none());
        assert!(anchored_toast_manager().subscribe().0.is_empty());
    }
}
