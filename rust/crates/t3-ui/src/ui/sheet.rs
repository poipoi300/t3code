//! Port of apps/web/src/components/ui/sheet.tsx Sheet and its slots.
//!
//! `sheet(open, label, |state| ...)` creates state; `sheet_popup(state, options, || ...)`
//! owns backdrop, modal focus trap/restoration, viewport and close control. All four sides and
//! default/inset variants use the web geometry. `keep_mounted` preserves the content owner
//! across closing, including drafts and scroll offsets. `transition_duration_ms=0` is instant.
//! Open requests are accepted by default; `on_open_change` lets callers decline dismissal.
//! Text slots accept `Text`; layout belongs on a parent, never on a returned primitive.

use crate::i18n::{Text, t};
use crate::theme::tw;
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};
use crate::ui::icon::Icon;
use nsh_core::{keymap::When, prelude::*};
use std::{cell::RefCell, rc::Rc};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Side {
    Top,
    Bottom,
    Left,
    #[default]
    Right,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Variant {
    #[default]
    Default,
    Inset,
}

#[derive(Clone, Copy)]
pub struct PopupOptions {
    pub side: Side,
    pub variant: Variant,
    pub show_close_button: bool,
    pub keep_mounted: bool,
    pub transition_duration_ms: u32,
    pub initial_focus: Option<ElementRef>,
}

impl Default for PopupOptions {
    fn default() -> Self {
        Self {
            side: Side::Right,
            variant: Variant::Default,
            show_close_button: true,
            keep_mounted: false,
            transition_duration_ms: 200,
            initial_focus: None,
        }
    }
}

type OpenChange = Rc<dyn Fn(bool)>;

#[derive(Clone, Copy)]
pub struct SheetState {
    pub open: Signal<bool>,
    title: Signal<Text>,
    description: Signal<Option<Text>>,
    open_change: Stored<Option<OpenChange>>,
}

impl SheetState {
    pub fn on_open_change(self, callback: impl Fn(bool) + 'static) {
        self.open_change.replace(Some(Rc::new(callback)));
    }

    pub fn set_open(self, open: bool) {
        if self.open.get_untracked() == open {
            return;
        }
        if let Some(callback) = self.open_change.get() {
            callback(open);
        } else {
            self.open.set(open);
        }
    }
}

pub fn sheet<E: IntoElement>(
    open: Signal<bool>,
    label: impl Into<Text>,
    children: impl FnOnce(SheetState) -> E,
) -> impl IntoElement {
    children(SheetState {
        open,
        title: signal(label.into()),
        description: signal(None),
        open_change: stored(None),
    })
}

pub fn sheet_trigger(
    state: SheetState,
    label: impl Into<Text>,
    children: impl IntoElement,
) -> impl IntoElement {
    let label = label.into();
    nsh_base::button()
        .accessibility_label(move || Some(label.resolve()))
        .on_press(move || state.set_open(true))
        .child(children)
}

pub fn sheet_close(
    state: SheetState,
    content: ButtonContent,
    variant: ButtonVariant,
    size: ButtonSize,
) -> impl IntoElement {
    button(
        content,
        variant,
        size,
        move || !state.open.get(),
        move || state.set_open(false),
    )
}

pub fn sheet_popup<E: IntoElement>(
    state: SheetState,
    options: PopupOptions,
    children: impl Fn() -> E + 'static,
) -> impl IntoElement {
    let shown = signal(false);
    let progress = animated_value(0.0);
    effect(move || {
        let open = state.open.get();
        if !open && !shown.get_untracked() {
            return;
        }
        if open {
            shown.set(true);
        }
        let duration = if reduce_motion() {
            0
        } else {
            options.transition_duration_ms
        };
        if duration == 0 {
            progress.set(if open { 1.0 } else { 0.0 });
            shown.set(open);
            return;
        }
        let end = progress.timing(
            if open { 1.0 } else { 0.0 },
            duration as f32,
            Easing::EaseInOut,
        );
        watch(async move {
            if end.await && !state.open.get_untracked() {
                shown.set(false);
            }
        });
    });
    let children = Rc::new(children);
    scoped(move || {
        provide_context(SheetContext {
            state,
            options,
            shown,
            progress,
        });
        if options.keep_mounted {
            // A non-lazy slot is owned by this host, independently of the temporary modal placement.
            retained_sheet().child(children()).into_element()
        } else {
            when(shown, move || {
                untracked(|| sheet_portal(state, options, progress, children()))
            })
            .into_element()
        }
    })
}

pub use sheet_popup as sheet_content;

#[derive(Clone, PartialEq, Props)]
struct RetainedSheetProps {
    children: Prop<Children>,
}

#[derive(Clone, Copy)]
struct SheetContext {
    state: SheetState,
    options: PopupOptions,
    shown: Signal<bool>,
    progress: AnimatedValue,
}

type ScopeBuilder = Box<dyn FnOnce() -> AnyElement>;
#[derive(Clone, Default)]
struct SheetScopeProps(Rc<RefCell<Option<ScopeBuilder>>>);
impl PartialEq for SheetScopeProps {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}
impl Props for SheetScopeProps {
    fn spec(_: &mut nsh_core::element::TsTypes) -> Vec<nsh_core::element::PropSpec> {
        Vec::new()
    }
    fn initial() -> Self {
        Self::default()
    }
    fn set(
        &mut self,
        name: &str,
        value: Option<nsh_core::element::AnyValue>,
    ) -> Result<bool, nsh_core::element::PropError> {
        ().set(name, value)
    }
    fn set_slot(&mut self, _: &str, _: Option<Slot>) -> bool {
        false
    }
    fn set_children(&mut self, _: Children) -> bool {
        false
    }
}
struct SheetScope;
impl Component for SheetScope {
    type Props = SheetScopeProps;
    type Event = nsh_core::element::NoEvent;
    fn view(props: Self::Props, _: Scope<Self::Event>) -> impl IntoElement {
        props
            .0
            .borrow_mut()
            .take()
            .map(|build| build())
            .unwrap_or_else(|| view().into_element())
    }
}
fn scoped(build: impl FnOnce() -> AnyElement + 'static) -> impl IntoElement {
    SheetScope::element(SheetScopeProps(Rc::new(RefCell::new(Some(Box::new(
        build,
    ))))))
}

#[component]
fn RetainedSheet(props: RetainedSheetProps) -> impl IntoElement {
    let context = use_context::<SheetContext>().expect("sheet scope");
    when(context.shown, move || {
        untracked(|| {
            sheet_portal(
                context.state,
                context.options,
                context.progress,
                props.children,
            )
        })
    })
}

pub fn sheet_portal<M>(
    state: SheetState,
    options: PopupOptions,
    progress: AnimatedValue,
    children: impl nsh_core::element::IntoChild<M>,
) -> impl IntoElement {
    let popup_ref = element_ref();
    provide_context(KeyboardContext {
        state,
        popup: popup_ref,
        initial: options.initial_focus,
    });
    // Base Sheet exposes no accessible title, focus trap, keepMounted, duration or 32px tween.
    // Its Dialog/modal supplies focus behavior; explicit overlays transcribe the web sheet.
    nsh_base::dialog().label(move || state.title.get().resolve())
        .accessibility_description(move || state.description.get().map(|d| d.resolve()).unwrap_or_default())
        .animation(ModalAnimation::None).close_on_escape(false)
        .close_on_backdrop_press(false).close_on_swipe(false)
        .on_cancel(move || state.set_open(false))
        .child(sheet_backdrop(state, progress))
        .child(sheet_viewport(options.side, options.variant,
            overlay().element_ref(&popup_ref).test_id("sheet-popup")
                .label(move || state.title.get().resolve())
                .accessibility_description(move || state.description.get().map(|d| d.resolve()).unwrap_or_default())
                .focusable(true)
                // Native measured bounds replace max-h-full and the side-dependent width calc.
                .app_class(tw("relative flex min-h-0 min-w-0 flex-col bg-popover text-popover-foreground shadow-lg/5"))
                .style_with(move |s| {
                    let inset = if options.variant == Variant::Inset && window().width.0 >= 640.0 { 16.0 } else { 0.0 };
                    let s = s.max_h(px((window().height.0 - 2.0 * inset).max(0.0)));
                    match options.side {
                        Side::Right => s.top(px(inset)).bottom(px(inset)).right(px(inset))
                            .w(px((window().width.0 - 48.0 - 2.0 * inset).clamp(0.0, 448.0))).border_l(px(1.0)),
                        Side::Left => s.top(px(inset)).bottom(px(inset)).left(px(inset))
                            .w(px((window().width.0 - 48.0 - 2.0 * inset).clamp(0.0, 448.0))).border_r(px(1.0)),
                        Side::Top => s.top(px(inset)).left(px(inset)).right(px(inset))
                            .max_h(px((window().height.0 - 48.0 - inset * 2.0).max(0.0))).border_b(px(1.0)),
                        Side::Bottom => s.bottom(px(inset)).left(px(inset)).right(px(inset))
                            .max_h(px((window().height.0 - 48.0 - inset * 2.0).max(0.0))).border_t(px(1.0)),
                    }
                })
                .when(options.variant == Variant::Inset, |popup| popup.app_class(tw("sm:rounded-2xl sm:border")))
                .bind_opacity(&progress)
                .bind_translate_x(&progress.interpolate(&[0.0, 1.0], &[match options.side { Side::Left => -32.0, Side::Right => 32.0, _ => 0.0 }, 0.0]))
                .bind_translate_y(&progress.interpolate(&[0.0, 1.0], &[match options.side { Side::Top => -32.0, Side::Bottom => 32.0, _ => 0.0 }, 0.0]))
                .disabled(move || !state.open.get())
                .pointer_events(move || if state.open.get() { PointerEvents::Auto } else { PointerEvents::None })
                .child(view().accessible(false).app_class(tw("pointer-events-none absolute inset-0 shadow-[0_1px_rgb(0_0_0/4%)] dark:shadow-[0_-1px_rgb(255_255_255/6%)] max-sm:hidden"))
                    .when(options.variant == Variant::Inset, |v| v.app_class(tw("hidden"))))
                .child(sheet_keyboard().child(children))
                .child(options.show_close_button.then(|| view().app_class(tw("absolute end-2 top-2"))
                    .child(sheet_close(state, ButtonContent::Icon { icon: Icon::X, label: t("Close") }, ButtonVariant::Ghost, ButtonSize::Icon))))))
}

#[derive(Clone, PartialEq, Props)]
struct SheetKeyboardProps {
    children: Prop<Children>,
}
#[derive(Clone, Copy)]
struct KeyboardContext {
    state: SheetState,
    popup: ElementRef,
    initial: Option<ElementRef>,
}

#[component]
fn SheetKeyboard(props: SheetKeyboardProps) -> impl IntoElement {
    let context = use_context::<KeyboardContext>().expect("sheet popup context");
    // Register inside the modal's WindowRef; editor Escape must reach the sheet too.
    on_key_with(
        "escape",
        KeyOptions {
            when: When::Always,
            ..Default::default()
        },
        move |_| context.state.set_open(false),
    );
    after_answer(move || {
        if context.state.open.get_untracked() {
            context.initial.unwrap_or(context.popup).focus();
        }
    });
    view()
        .app_class(tw("flex min-h-0 flex-col"))
        .child(props.children)
}

pub fn sheet_backdrop(state: SheetState, progress: AnimatedValue) -> impl IntoElement {
    overlay()
        .inset_0()
        .focusable(false)
        .window_drag(false)
        .test_id("sheet-backdrop")
        .app_class(tw("bg-background/60 backdrop-blur-xs"))
        .bind_opacity(&progress)
        .on_press(move || state.set_open(false))
        .accessible(false)
}

pub use sheet_backdrop as sheet_overlay;

use crate::runtime::tasks::watch;

pub fn sheet_viewport(
    side: Side,
    variant: Variant,
    children: impl IntoElement,
) -> impl IntoElement {
    // Native overlay coordinates replace the web's full-window grid and portal z-index.
    view()
        .w_full()
        .h_full()
        .window_drag(false)
        .app_class(tw(match side {
            Side::Top => "pb-12",
            Side::Bottom => "pt-12",
            Side::Left => "flex justify-start",
            Side::Right => "flex justify-end",
        }))
        .when(variant == Variant::Inset, |v| v.app_class(tw("sm:p-4")))
        .child(children)
}

pub fn sheet_header(
    has_panel: impl IntoReactive<bool>,
    children: impl IntoElement,
) -> impl IntoElement {
    let has_panel = has_panel.into_reactive();
    view()
        .app_class(move || {
            tw(if has_panel.get() {
                "flex flex-col gap-2 p-6 pb-3 max-sm:pb-4"
            } else {
                "flex flex-col gap-2 p-6 max-sm:pb-4"
            })
        })
        .child(children)
}

pub fn sheet_title(state: SheetState, label: impl IntoReactive<Text>) -> impl IntoElement {
    let label = label.into_reactive();
    let source = label.clone();
    effect(move || state.title.set(source.get()));
    text(move || label.get().resolve())
        .role(AccessibilityRole::Header)
        .app_class(tw("font-semibold text-xl leading-snug"))
}

pub fn sheet_description(state: SheetState, label: impl IntoReactive<Text>) -> impl IntoElement {
    let label = label.into_reactive();
    let source = label.clone();
    effect(move || state.description.set(Some(source.get())));
    text(move || label.get().resolve()).app_class(tw("text-muted-foreground text-sm"))
}
