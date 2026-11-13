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
//! `toasts`/`anchored_toasts` also accept an active-thread selector, for resolved draft routes.
//! An anchored toast needs a live `ElementRef`; copy helpers silently ignore a missing anchor.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use futures::{StreamExt, channel::mpsc};
use nsh_base::toast::{Anchor, SwipeDirection, ToastMotion, ToastStackState};
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
    /// `None` is persistent (the web's `timeout: 0`).
    pub timeout: Option<Duration>,
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
}

impl PartialEq for Entry {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && Rc::ptr_eq(&self.options, &other.options)
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
        let callback = self.0.borrow().entries.iter().find(|entry| entry.id == id)
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
    Entry { id, options: Rc::new(options), remaining: Rc::new(RefCell::new(remaining)) }
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
    let anchor = anchor?.clone();
    Some(anchored_toast_manager().add(ToastOptions {
        title: "Copied!".into(),
        timeout: Some(Duration::from_millis(ANCHORED_COPY_TOAST_TIMEOUT_MS)),
        anchor: Some(anchor),
        data: ThreadToastData { tooltip_style: true, ..Default::default() },
        ..Default::default()
    }))
}

/// Port of apps/web/src/components/ui/anchoredCopyToast.ts showAnchoredCopyErrorToast.
pub fn show_anchored_copy_error_toast(anchor: Option<&ElementRef>, error: &str) -> Option<ToastId> {
    Some(anchored_toast_manager().add(ToastOptions {
        title: "Failed to copy".into(),
        variant: Variant::Error,
        description: Some(error.into()),
        anchor: Some(anchor?.clone()),
        ..Default::default()
    }))
}

/// Port of apps/web/src/components/ui/toast.logic.ts shouldRenderThreadScopedToast.
pub fn should_render_thread_scoped_toast(data: &ThreadToastData, active: Option<&ThreadKey>) -> bool {
    if let Some(thread) = &data.thread_ref {
        return active == Some(thread);
    }
    data.thread_id.as_ref().is_none_or(|id| active.is_some_and(|key| &key.thread == id))
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
    let height = |value: f32| if value.is_finite() && value > 0.0 { value } else { 0.0 };
    let (mut full_index, mut full_offset, mut live_index, mut live_offset) = (0, 0.0, 0, 0.0);
    let items = toasts.iter().map(|toast| {
        let item = if toast.ending { (full_index, full_offset) } else { (live_index, live_offset) };
        full_index += 1;
        full_offset += height(toast.height);
        if !toast.ending {
            live_index += 1;
            live_offset += height(toast.height);
        }
        item
    }).collect();
    VisibleToastLayout {
        frontmost_height: toasts.iter().find(|toast| !toast.ending).map_or(0.0, |toast| height(toast.height)),
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
        Self { remaining: match (timeout, visible) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }, started: None }
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
    (children, toasts(toast_manager(), position, active_thread()))
}

pub fn anchored_toast_provider(children: impl IntoElement) -> impl IntoElement {
    (children, anchored_toasts(anchored_toast_manager(), active_thread()))
}

fn subscribe(manager: &ToastManager) -> Signal<Vec<Entry>> {
    let (initial, mut updates) = manager.subscribe();
    let entries = signal(initial);
    spawn(async move {
        while let Some(next) = updates.next().await {
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
        entries.get().into_iter().filter(|entry| should_render_thread_scoped_toast(&entry.options.data, active.as_ref())).collect::<Vec<_>>()
    });
    let stack = ToastStackState::new();
    let focused = signal(false);
    let focus = element_ref();
    focus.on_focus_in(move || focused.set(true));
    focus.on_focus_out(move || focused.set(false));
    let cancel = manager.clone();
    overlay()
        .test_id("toast-portal")
        .child(nsh_base::toast_stack()
            .reference(&focus)
            .placement(position.anchor())
            .state(stack)
            // NSH measures and resolves the web's per-toast CSS variables in ToastStack.
            .motion(ToastMotion { collapsed_peek: px(12.0), expanded_gap: px(12.0), collapsed_scale_step: 0.1, ..ToastMotion::sonner() })
            .app_class(tw("absolute z-100 mx-auto max-w-90"))
            .w(move || px((window().width.0 - inset() * 2.0).clamp(0.0, 360.0)))
            .top(move || if is_top(position) { px(inset() + safe_area().top + 32.0) } else { auto() })
            .bottom(move || if is_top(position) { auto() } else { px(inset() + safe_area().bottom) })
            .left(move || edge(position, true))
            .right(move || edge(position, false))
            .on_cancel(move || {
                if let Some(entry) = visible.get_untracked().last() { cancel.close(entry.id); }
            })
            .child(each(visible, |entry| entry.id, move |item, index| {
                let manager = manager.clone();
                let expanded = move || stack.is_expanded() || focused.get();
                toast_root(item, manager, Some(position), expanded, move || {
                    let count = visible.with(Vec::len);
                    should_hide_collapsed_toast_content(count.saturating_sub(index.get() + 1), count) && !expanded()
                })
            })))
}

fn inset() -> f32 { if window().width.0 < 640.0 { 16.0 } else { 32.0 } }
fn is_top(position: Position) -> bool { matches!(position, Position::TopLeft | Position::TopCenter | Position::TopRight) }
fn edge(position: Position, left: bool) -> Length {
    match position {
        Position::TopCenter | Position::BottomCenter => px((window().width.0 - (window().width.0 - inset()*2.0).clamp(0.0, 360.0))/2.0).into(),
        Position::TopLeft | Position::BottomLeft if left => px(inset()).into(),
        Position::TopRight | Position::BottomRight if !left => px(inset()).into(),
        _ => auto(),
    }
}

pub fn anchored_toasts(manager: ToastManager, active: impl Fn() -> Option<ThreadKey> + 'static) -> impl IntoElement {
    let entries = subscribe(&manager);
    let visible = computed(move || {
        let active = active();
        entries.get().into_iter().filter(|entry| entry.options.anchor.is_some() && should_render_thread_scoped_toast(&entry.options.data, active.as_ref())).collect::<Vec<_>>()
    });
    view().test_id("toast-viewport-anchored").child(each(visible, |entry| entry.id, move |item, _| {
        let anchor = item.get_untracked().options.anchor.clone().expect("filtered anchor");
        let hovered = signal(false);
        let focused = signal(false);
        let focus = element_ref();
        focus.on_focus_in(move || focused.set(true));
        focus.on_focus_out(move || focused.set(false));
        overlay().anchor(&anchor)
            .side(nsh_core::overlay::Side::Top)
            .side_offset(move || px(item.get().options.side_offset))
            .app_class(tw("z-100 max-w-64"))
            .reference(&focus)
            .on_mouse_enter(move || hovered.set(true))
            .on_mouse_leave(move || hovered.set(false))
            .child(toast_root(item, manager.clone(), None, move || hovered.get() || focused.get(), || false))
    }))
}

fn thread_toast_visible_auto_dismiss(item: ReadSignal<Entry>, manager: ToastManager, paused: impl Fn() -> bool + 'static) {
    effect(move || {
        let entry = item.get();
        let paused = paused() || lifecycle() != Lifecycle::Active;
        let now = nsh_core::time::now();
        entry.remaining.borrow_mut().pause(now);
        if paused { return; }
        let remaining = entry.remaining.borrow().remaining;
        if let Some(remaining) = remaining {
            entry.remaining.borrow_mut().started = Some(now);
            let close = manager.clone();
            let id = entry.id;
            let timer = set_timeout(remaining, move || { close.close(id); });
            cleanup(move || {
                timer.cancel();
                entry.remaining.borrow_mut().pause(nsh_core::time::now());
            });
        }
    });
}

fn toast_root(
    item: ReadSignal<Entry>, manager: ToastManager, position: Option<Position>,
    paused: impl Fn() -> bool + 'static, hidden: impl Fn() -> bool + 'static,
) -> impl IntoElement {
    thread_toast_visible_auto_dismiss(item, manager.clone(), paused);
    let dismiss = manager.clone();
    let tooltip = move || item.get().options.data.tooltip_style && position.is_none();
    nsh_base::toast()
        .transition_status(nsh_base::toast::ToastTransitionStatus::Present)
        .swipe_direction(position.map(Position::swipe))
        .accessibility_label(move || Some(SharedString::from(announcement(&item.get().options))))
        .on_dismiss_request(move || { dismiss.close(item.get_untracked().id); })
        // index.css @utility dropdown-glass: NSH has no color-mix/backdrop saturation.
        .app_class(move || tw(&cn([
            Some("relative overflow-visible text-popover-foreground shadow-xl shadow-black/25 bg-popover border border-foreground/10"),
            Some(if tooltip() { "rounded-md text-xs" } else { "rounded-lg" }),
        ])))
        .child(dynamic(move || {
            let entry = item.get();
            let manager = manager.clone();
            untracked(|| {
                if tooltip() {
                    view().app_class(tw("pointer-events-auto px-2 py-1"))
                        .child(text(t(&entry.options.title)).test_id("toast-title")).into_element()
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
                        .child(toast_body_content(options, descriptor, manager, id)))
                    .into_element()
                }
            })
        }))
}

fn announcement(options: &ToastOptions) -> String {
    match &options.description {
        Some(description) => format!("{}: {}", t(&options.title), t(description)),
        None => t(&options.title),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BodyDescriptor { stacked: bool, copy_error: bool, trailing: bool }

fn has_visible_toast_action(action: Option<&Action>) -> bool {
    action.is_some_and(|action| !action.label.is_empty())
}

fn derive_toast_body_descriptor(options: &ToastOptions) -> BodyDescriptor {
    let action = has_visible_toast_action(options.action.as_ref());
    let copy_error = options.variant == Variant::Error && options.description.is_some() && !options.data.hide_copy_button;
    BodyDescriptor {
        stacked: action && options.data.action_layout == ActionLayout::StackedEnd,
        copy_error,
        trailing: copy_error || action || !options.data.additional_actions.is_empty() || options.data.secondary_action.is_some(),
    }
}

fn toast_body_content(options: Rc<ToastOptions>, body: BodyDescriptor, manager: ToastManager, id: ToastId) -> impl IntoElement {
    let leading = options.data.leading_icon.as_ref().map(|slot| slot());
    // Selector classes move to their icon child; h-lh is the 20px text-sm line height.
    let glyph = match options.variant {
        Variant::Default => None,
        Variant::Error => Some((Icon::CircleAlert, palette().destructive)),
        Variant::Success => Some((Icon::Check, palette().success)),
        Variant::Warning => Some((Icon::CircleAlert, palette().warning)),
        Variant::Info => Some((Icon::CircleAlert, palette().info)),
        Variant::Loading => Some((Icon::Ellipsis, palette().muted_foreground)),
    };
    let controls = options.clone();
    (
        view().app_class(tw(&cn([Some("flex min-h-0 min-w-0 flex-col gap-0.5"), Some(if body.stacked { "pr-5" } else { "flex-1" })])))
            .child(view().app_class(tw(&cn([Some("flex min-w-0 gap-2"), (!body.stacked && !body.trailing).then_some("pr-6")])))
                .child(leading.map(|leading| view().app_class(tw("flex h-5 w-4 shrink-0 items-center justify-center")).child(leading)))
                .child((options.data.leading_icon.is_none()).then(|| glyph.map(|(glyph, color)| view().app_class(tw("h-5 w-4 shrink-0")).child(icon(glyph, 16.0, move || color)))))
                // wrap-break-word is omitted: NSH preserves newlines and wraps at words.
                .child(text(t(&options.title)).test_id("toast-title").app_class(tw("min-w-0 font-medium"))))
            .child(toast_description_and_expandable(options)),
        body.trailing.then(|| view().app_class(tw(&cn([Some("flex items-center gap-1.5"), Some(if body.stacked { "w-full justify-end" } else { "shrink-0" })])))
            .child(body.copy_error.then(|| copy_error_button(controls.description.clone().unwrap_or_default())))
            .children(controls.data.additional_actions.iter().map(|(_, action)| toast_action(action, controls.data.secondary_action_variant.unwrap_or(ButtonVariant::Outline), None)))
            .child(controls.data.secondary_action.as_ref().map(|action| toast_action(action, controls.data.secondary_action_variant.unwrap_or(ButtonVariant::Outline), None)))
            .child(controls.action.as_ref().filter(|action| has_visible_toast_action(Some(action))).map(|action| toast_action(action, controls.data.action_variant.unwrap_or_default(), Some((manager, id))))))
    )
}

fn toast_action(action: &Action, variant: ButtonVariant, close: Option<(ToastManager, ToastId)>) -> impl IntoElement {
    let action = action.clone();
    let disabled = action.disabled;
    button(ButtonContent::Label(t(&action.label)), variant, ButtonSize::Xs, move || disabled, move || {
        if action.disabled { return; }
        (action.on_press)();
        if let Some((manager, id)) = &close { manager.close(*id); }
    })
}

fn copy_error_button(source: String) -> impl IntoElement {
    // ButtonSize lacks the web's icon-micro seam; use the fixed 12px glyph in its 24px target.
    // NSH's clipboard write is fire-and-forget: do not claim success without an acknowledgement.
    nsh_base::button().accessibility_label(Some(SharedString::from(t("Copy error"))))
        .app_class(tw("inline-flex size-6 shrink-0 items-center justify-center rounded-md text-muted-foreground/80 hover:bg-transparent hover:text-muted-foreground focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"))
        .on_press(move || set_clipboard(source.clone()))
        .child(icon(Icon::Copy, 12.0, || palette().muted_foreground))
}

fn error_description_clamped(options: &ToastOptions) -> bool {
    options.variant == Variant::Error && options.description.as_ref().is_some_and(|description| description.encode_utf16().count() >= 180)
}

fn description(options: &ToastOptions) -> impl IntoElement {
    let clamped = error_description_clamped(options);
    text(t(options.description.as_deref().unwrap_or_default()))
        .test_id("toast-description")
        .select_text()
        .app_class(tw("min-w-0 select-text text-muted-foreground"))
        .number_of_lines(if clamped { 4 } else { 0 })
}

fn toast_description_and_expandable(options: Rc<ToastOptions>) -> impl IntoElement {
    if options.data.expandable_content.is_none() {
        return description(&options).into_element();
    }
    if !options.data.expandable_description_trigger {
        return (description(&options), toast_expandable_section(options)).into_element();
    }
    let open = signal(false);
    let labels = options.clone();
    let content = options.data.expandable_content.clone();
    (
        nsh_base::button()
            .accessibility_label(move || Some(SharedString::from(expand_label(&labels, open.get()))))
            .app_class(tw("group flex min-w-0 w-full cursor-pointer select-none items-start gap-1.5 rounded-sm text-left outline-none ring-offset-background transition-colors hover:bg-muted/40 focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-background"))
            .on_press(move || open.update(|value| *value = !*value))
            .child(view().app_class(tw("min-w-0 flex-1")).child(description(&options)))
            .child(expand_chevron(open)),
        dynamic(move || open.get().then(|| expandable_panel(content.as_ref()).into_element()))
    ).into_element()
}

fn expand_label(options: &ToastOptions, open: bool) -> String {
    t(if open { options.data.collapse_label.as_deref().unwrap_or("Hide details") } else { options.data.expand_label.as_deref().unwrap_or("Show details") })
}

fn expand_chevron(open: Signal<bool>) -> impl IntoElement {
    // ChevronUp is absent from UI-ICONS; rotate the matching down chevron.
    view().app_class(tw("size-3.5 shrink-0 opacity-80"))
        .rotate(move || if open.get() { 180.0 } else { 0.0 })
        .child(icon(Icon::ChevronDown, 14.0, || palette().muted_foreground))
}

fn toast_expandable_section(options: Rc<ToastOptions>) -> impl IntoElement {
    let open = signal(false);
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
    scroll().app_class(tw("mt-2 max-h-40 min-h-0 pr-0.5 select-text"))
        .child(content.map(|slot| slot()))
}
