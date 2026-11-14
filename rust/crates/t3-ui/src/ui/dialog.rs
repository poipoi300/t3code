//! Port of apps/web/src/components/ui/dialog.tsx Dialog and alert-dialog.tsx AlertDialog.
//!
//! `dialog(open, |state| children)` and `alert_dialog(open, |state| children)` are roots;
//! `open` is caller-owned (use `signal(default_open)` for an uncontrolled root). All slots take
//! the root's `DialogState`. `dialog_popup(state, PopupOptions, || children)` owns the portal,
//! backdrop and viewport; `alert_dialog_popup` does the same without a dismiss button. The
//! `*_content` and `*_overlay` names are aliases, as in TSX. `Variant::{Default, Media}` and
//! `FooterVariant::{Default, Bare}` are the source's variants; there is no popup Size in TSX.
//! Button slots take the existing `ButtonSize` and `ButtonVariant`; `ActionVariant` excludes
//! destructive variants, just as `DialogActionProps` does. Text slots receive English keys
//! (closures allow dynamic text); ButtonContent carries already translated labels.
//!
//! `PopupOptions::initial_focus` overrides initial focus, and `popup_ref` preserves a caller ref.
//! Coarse-pointer opens focus the popup to avoid the software keyboard. Automatic action focus
//! needs the frozen button primitive to accept an ElementRef; until then NSH chooses the first
//! enabled control. Custom rendered actions can use `state.register_action(ref, disabled)`.
//! Custom rendered controls use `state.register_control(ref, disabled)` for fallback focus.
//! Modal focus trapping and restoration belong to NSH. Custom alert content
//! uses `nsh_base::dialog().role(AlertDialog)`: NSH's `alert_dialog()` is a system alert with no
//! element slots and cannot transcribe the web tree. Portals are NSH modal layers; arbitrary DOM
//! portal containers and render-prop element replacement have no cross-platform equivalent.

use std::rc::Rc;

use nsh_core::animation::{Easing, MotionChannel};
use nsh_core::element::{IntoChild, Timing, desktop::title_bar};
use nsh_core::prelude::*;

use super::button::{ButtonContent, ButtonSize, ButtonVariant, button};
use super::icon::Icon;
use crate::i18n::t;
use crate::state::layout::{Pointer, layout_class, pointer};
use crate::theme::{cn, tw};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Variant {
    #[default]
    Default,
    Media,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FooterVariant {
    #[default]
    Default,
    Bare,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ActionVariant {
    #[default]
    Default,
    Ghost,
    GhostMuted,
    Link,
    Outline,
    Secondary,
    WarningOutline,
}

impl From<ActionVariant> for ButtonVariant {
    fn from(variant: ActionVariant) -> Self {
        match variant {
            ActionVariant::Default => Self::Default,
            ActionVariant::Ghost => Self::Ghost,
            ActionVariant::GhostMuted => Self::GhostMuted,
            ActionVariant::Link => Self::Link,
            ActionVariant::Outline => Self::Outline,
            ActionVariant::Secondary => Self::Secondary,
            ActionVariant::WarningOutline => Self::WarningOutline,
        }
    }
}

#[derive(Clone, Copy)]
pub struct PopupOptions {
    pub variant: Variant,
    pub show_close_button: bool,
    pub bottom_stick_on_mobile: bool,
    pub initial_focus: Option<ElementRef>,
    pub popup_ref: Option<ElementRef>,
}

/// AlertDialogPopup has neither a media variant nor a built-in close button in TSX.
#[derive(Clone, Copy)]
pub struct AlertPopupOptions {
    pub bottom_stick_on_mobile: bool,
    pub initial_focus: Option<ElementRef>,
    pub popup_ref: Option<ElementRef>,
}

impl Default for AlertPopupOptions {
    fn default() -> Self {
        Self {
            bottom_stick_on_mobile: true,
            initial_focus: None,
            popup_ref: None,
        }
    }
}

impl Default for PopupOptions {
    fn default() -> Self {
        Self {
            variant: Variant::Default,
            show_close_button: true,
            bottom_stick_on_mobile: true,
            initial_focus: None,
            popup_ref: None,
        }
    }
}

type TextSource = Rc<dyn Fn() -> String>;

struct FocusAction {
    target: ElementRef,
    disabled: Rc<dyn Fn() -> bool>,
}

#[derive(Clone, Default)]
struct Ancestors(Vec<Signal<usize>>);

/// A root's state, captured by its controls rather than looked up in event handlers.
#[derive(Clone, Copy)]
pub struct DialogState {
    pub open: Signal<bool>,
    alert: bool,
    touch: Signal<bool>,
    title: Signal<Option<TextSource>>,
    description: Signal<Option<TextSource>>,
    has_header: Signal<bool>,
    has_panel: Signal<bool>,
    bare_footer: Signal<bool>,
    nested: Signal<usize>,
    ancestors: Stored<Ancestors>,
    actions: Stored<Vec<FocusAction>>,
    controls: Stored<Vec<FocusAction>>,
}

impl DialogState {
    fn new(open: Signal<bool>, alert: bool) -> Self {
        Self {
            open,
            alert,
            touch: signal(false),
            title: signal_with(None, SignalEquals::Never),
            description: signal_with(None, SignalEquals::Never),
            has_header: signal(false),
            has_panel: signal(false),
            bare_footer: signal(false),
            nested: signal(0),
            ancestors: stored(use_context::<Ancestors>().unwrap_or_default()),
            actions: stored(Vec::new()),
            controls: stored(Vec::new()),
        }
    }

    pub fn show(self) {
        self.touch.set(pointer() == Pointer::Coarse);
        self.open.set(true);
    }

    pub fn close(self) {
        self.open.set(false);
    }

    /// Registers a custom rendered DialogAction for the source's initial-focus priority.
    /// `disabled` includes both disabled and aria-disabled; detached refs are ignored.
    pub fn register_action(self, target: ElementRef, disabled: impl Fn() -> bool + 'static) {
        self.actions.with(|actions| {
            actions.push(FocusAction {
                target,
                disabled: Rc::new(disabled),
            })
        });
    }

    /// Registers a custom control in tree order for Base UI's first-control fallback.
    pub fn register_control(self, target: ElementRef, disabled: impl Fn() -> bool + 'static) {
        self.controls.with(|controls| {
            controls.push(FocusAction {
                target,
                disabled: Rc::new(disabled),
            })
        });
    }
}

pub fn dialog<E: IntoElement>(
    open: Signal<bool>,
    children: impl FnOnce(DialogState) -> E,
) -> impl IntoElement {
    children(DialogState::new(open, false))
}

/// Port of alert-dialog.tsx AlertDialog.
pub fn alert_dialog<E: IntoElement>(
    open: Signal<bool>,
    children: impl FnOnce(DialogState) -> E,
) -> impl IntoElement {
    children(DialogState::new(open, true))
}

pub fn dialog_trigger(state: DialogState, source: &str) -> impl IntoElement {
    let source = source.to_owned();
    let label = source.clone();
    // The source's bare Trigger has no Button styling.
    nsh_base::button()
        .test_id("dialog-trigger")
        .accessibility_label(move || Some(SharedString::from(t(&label))))
        .on_press(move || state.show())
        .child(text(move || SharedString::from(t(&source))))
}

pub fn dialog_close(
    state: DialogState,
    content: ButtonContent,
    variant: ButtonVariant,
    size: ButtonSize,
    disabled: impl Fn() -> bool + 'static,
) -> impl IntoElement {
    button(
        content,
        variant,
        size,
        move || !state.open.get() || disabled(),
        move || state.close(),
    )
}

pub fn dialog_action(
    state: DialogState,
    content: ButtonContent,
    variant: ActionVariant,
    size: ButtonSize,
    disabled: impl Fn() -> bool + 'static,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    let disabled = Rc::new(disabled);
    let for_press = disabled.clone();
    button(
        content,
        variant.into(),
        size,
        move || !state.open.get() || disabled(),
        move || {
            if state.open.get_untracked() && !for_press() {
                on_press();
            }
        },
    )
}

pub use dialog_action as alert_dialog_action;
pub use dialog_close as alert_dialog_close;

fn transition(motion: Motion, open: bool, offset: f32, backdrop: bool) -> Animation {
    let timing = Timing::new(if reduce_motion() { 0.0 } else { 200.0 }, Easing::EaseInOut);
    let mut tracks = vec![Animation::timing(
        &motion.value(MotionChannel::Opacity),
        if open { 1.0 } else { 0.0 },
        timing,
    )];
    if !backdrop {
        tracks.push(Animation::timing(
            &motion.value(MotionChannel::Scale),
            if open { 1.0 } else { 0.98 },
            timing,
        ));
        tracks.push(Animation::timing(
            &motion.value(MotionChannel::TranslateY),
            if open { 0.0 } else { offset },
            timing,
        ));
    }
    // CSS transitions reverse from their current value when an exit is interrupted.
    Animation::parallel(tracks)
}

/// RetainedWhileClosed: NSH must retain the modal layer itself through its exit animation.
fn retained_while_closed<M: 'static, E: IntoChild<M> + 'static>(
    state: DialogState,
    options: PopupOptions,
    children: impl Fn() -> E + 'static,
) -> impl IntoElement {
    let mounted = signal(false);
    let generation = stored(0u64);
    let popup_motion = motion();
    let backdrop_motion = motion();
    let nested_opacity = animated_value(1.0);
    effect(move || {
        let nested = state.nested.get();
        let sheet = options.bottom_stick_on_mobile && layout_class().is_compact();
        let target = if sheet {
            if nested == 0 { 1.0 } else { 0.0 }
        } else {
            1.0 - 0.1 * nested as f32
        };
        drop(nested_opacity.timing(
            target,
            if reduce_motion() { 0.0 } else { 200.0 },
            Easing::EaseInOut,
        ));
    });
    effect(move || {
        let open = state.open.get();
        let sheet = untracked(|| options.bottom_stick_on_mobile && layout_class().is_compact());
        let offset = if sheet {
            16.0
        } else if state.ancestors.with(|a| !a.0.is_empty()) {
            32.0
        } else {
            0.0
        };
        generation.with(|value| *value += 1);
        let current = generation.get();
        if open {
            if !mounted.get_untracked() {
                popup_motion.value(MotionChannel::Opacity).set(0.0);
                popup_motion.value(MotionChannel::Scale).set(0.98);
                popup_motion.value(MotionChannel::TranslateY).set(offset);
                backdrop_motion.value(MotionChannel::Opacity).set(0.0);
                mounted.set(true);
            }
            drop(transition(popup_motion, true, offset, false).start());
            drop(transition(backdrop_motion, true, offset, true).start());
        } else if mounted.get_untracked() {
            let ended = transition(popup_motion, false, offset, false).start();
            drop(transition(backdrop_motion, false, offset, true).start());
            spawn(async move {
                let _ = ended.await;
                if generation.get() == current && !state.open.get_untracked() {
                    mounted.set(false);
                }
            });
        }
    });
    when(mounted, move || {
        untracked(|| {
            state.has_header.set(false);
            state.has_panel.set(false);
            state.bare_footer.set(false);
            state.title.set(None);
            state.description.set(None);
            state.actions.with(Vec::clear);
            state.controls.with(Vec::clear);
            let mut ancestors = state.ancestors.get();
            for count in &ancestors.0 {
                count.update(|n| *n += 1);
            }
            let inherited = ancestors.clone();
            cleanup(move || {
                for count in &inherited.0 {
                    // Removing an outer popup can dispose an ancestor before its nested layer.
                    if let Some(value) = count.try_get() {
                        let _ = count.try_set(value.saturating_sub(1));
                    }
                }
            });
            ancestors.0.push(state.nested);
            provide_context(ancestors);
            let popup_ref = options.popup_ref.unwrap_or_else(element_ref);
            let content = children();
            let popup = popup_surface(state, options, popup_ref)
                .bind_opacity(
                    &popup_motion
                        .value(MotionChannel::Opacity)
                        .mul(&nested_opacity),
                )
                .bind_scale(&popup_motion.value(MotionChannel::Scale))
                .bind_translate_y(&popup_motion.value(MotionChannel::TranslateY))
                .child(content)
                .child((options.show_close_button && !state.alert).then(|| {
                    view()
                        .app_class(tw("absolute end-2 top-2"))
                        .child(dialog_close(
                            state,
                            ButtonContent::Icon {
                                icon: Icon::X,
                                label: t("Close"),
                            },
                            ButtonVariant::Ghost,
                            ButtonSize::Icon,
                            || false,
                        ))
                }));
            let backdrop = backdrop_surface(state, options.variant).bind_motion(&backdrop_motion);
            dialog_portal(
                state,
                options,
                popup_ref,
                backdrop,
                dialog_viewport(options, popup),
            )
        })
    })
}

pub fn dialog_popup<M: 'static, E: IntoChild<M> + 'static>(
    state: DialogState,
    options: PopupOptions,
    children: impl Fn() -> E + 'static,
) -> impl IntoElement {
    retained_while_closed(state, options, children)
}

/// Port of alert-dialog.tsx AlertDialogPopup.
pub fn alert_dialog_popup<M: 'static, E: IntoChild<M> + 'static>(
    state: DialogState,
    options: AlertPopupOptions,
    children: impl Fn() -> E + 'static,
) -> impl IntoElement {
    dialog_popup(
        state,
        PopupOptions {
            variant: Variant::Default,
            show_close_button: false,
            bottom_stick_on_mobile: options.bottom_stick_on_mobile,
            initial_focus: options.initial_focus,
            popup_ref: options.popup_ref,
        },
        children,
    )
}

pub use alert_dialog_popup as alert_dialog_content;
pub use dialog_popup as dialog_content;

pub fn dialog_portal(
    state: DialogState,
    options: PopupOptions,
    popup_ref: ElementRef,
    backdrop: impl IntoElement,
    viewport: impl IntoElement,
) -> impl IntoElement {
    // Native modal cancellation is mediated so its layer remains mounted for the 200 ms exit.
    nsh_base::dialog()
        .role(if state.alert {
            AccessibilityRole::AlertDialog
        } else {
            AccessibilityRole::Dialog
        })
        .animation(ModalAnimation::None)
        .close_on_escape(false)
        .close_on_backdrop_press(false)
        .close_on_swipe(false)
        .on_cancel(move || state.close())
        .on_open(move || {
            if let Some(target) = options.initial_focus {
                target.focus();
            } else if state.touch.get_untracked() || pointer() == Pointer::Coarse {
                popup_ref.focus();
            } else if let Some(target) = state.actions.with(|actions| {
                actions
                    .iter()
                    .find(|action| action.target.node().is_some() && !(action.disabled)())
                    .map(|action| action.target)
            }) {
                target.focus();
            } else if let Some(target) = state.controls.with(|controls| {
                controls
                    .iter()
                    .find(|control| control.target.node().is_some() && !(control.disabled)())
                    .map(|control| control.target)
            }) {
                target.focus();
            }
        })
        .label(move || {
            SharedString::from(
                state
                    .title
                    .with(|source| source.as_ref().map(|f| t(&f())).unwrap_or_default()),
            )
        })
        .accessibility_description(move || {
            SharedString::from(
                state
                    .description
                    .with(|source| source.as_ref().map(|f| t(&f())).unwrap_or_default()),
            )
        })
        .app_class(tw("w-full h-full"))
        .child(backdrop)
        .child(viewport)
}

pub use dialog_portal as alert_dialog_portal;

fn backdrop_surface(state: DialogState, variant: Variant) -> nsh_core::element::View {
    view()
        .test_id(if state.alert {
            "alert-dialog-backdrop"
        } else {
            "dialog-backdrop"
        })
        .accessible(false)
        .on_press(move || {
            if !state.alert {
                state.close();
            }
        })
        // index.css @utility dialog-backdrop, expanded at its use site.
        .app_class(tw(&cn([
            Some("fixed inset-0 z-50 bg-background/60 dark:bg-background/64 backdrop-blur-[4px]"),
            (variant == Variant::Media).then_some("z-[60] bg-black/75 backdrop-blur-none"),
        ])))
}

pub fn dialog_backdrop(state: DialogState, variant: Variant) -> impl IntoElement {
    backdrop_surface(state, variant)
}

pub use dialog_backdrop as dialog_overlay;

/// Port of alert-dialog.tsx AlertDialogBackdrop.
pub fn alert_dialog_backdrop(state: DialogState) -> impl IntoElement {
    backdrop_surface(state, Variant::Default)
}

pub use alert_dialog_backdrop as alert_dialog_overlay;

pub fn dialog_viewport<M>(options: PopupOptions, children: impl IntoChild<M>) -> impl IntoElement {
    view()
        .test_id("dialog-viewport")
        .pointer_events(nsh_core::PointerEvents::BoxNone)
        .app_class(tw(&cn([
            Some("fixed inset-0 z-50 grid grid-rows-[1fr_auto_1fr] justify-items-center p-4"),
            options
                .bottom_stick_on_mobile
                .then_some("max-sm:grid-rows-[1fr_auto] max-sm:p-0 max-sm:pt-12"),
            (options.variant == Variant::Media)
                .then_some("z-[60] grid-rows-1 place-items-center px-4 pb-6"),
        ])))
        // title_bar().top is the shared native-titlebar-height equivalent; app-region drops.
        .when(options.variant == Variant::Media, |v| {
            v.pt(move || px(title_bar().top.0 + 24.0))
        })
        .child(children)
}

pub use dialog_viewport as alert_dialog_viewport;

fn popup_surface(
    state: DialogState,
    options: PopupOptions,
    popup_ref: ElementRef,
) -> nsh_core::element::View {
    // dialog-styles.ts: nested variables are reactive typed transforms; will-change and app-region drop.
    view()
        .test_id(if state.alert { "alert-dialog-popup" } else { "dialog-popup" })
        .element_ref(&popup_ref)
        .focusable(move || state.touch.get() || pointer() == Pointer::Coarse)
        .disabled(move || !state.open.get())
        .pointer_events(move || if state.open.get() { nsh_core::PointerEvents::Auto } else { nsh_core::PointerEvents::None })
        .app_class(move || {
            let sheet = options.bottom_stick_on_mobile && layout_class().is_compact();
            let classes = cn([
                Some("relative flex min-h-0 w-full min-w-0 flex-col outline-none duration-200 ease-in-out row-start-2 text-popover-foreground"),
                (options.variant == Variant::Default).then_some("rounded-2xl border max-h-full max-w-lg"),
                // index.css @utility dialog-glass; NSH has no backdrop-filter support query.
                (options.variant == Variant::Default).then_some("bg-background/80 backdrop-blur-[12px] dark:backdrop-blur-[16px] backdrop-saturate-[1.14] dark:backdrop-saturate-[1.08] border-contrast-foreground/10 dark:border-white/8 shadow-[0_24px_64px_-24px_rgb(0_0_0/65%)] dark:shadow-[inset_0_1px_rgb(255_255_255/4%),0_24px_72px_-20px_rgb(0_0_0/90%)]"),
                (options.variant == Variant::Media).then_some("w-auto overflow-visible rounded-none border border-transparent bg-transparent p-0 shadow-none"),
                sheet.then_some("max-w-none rounded-none border-x-0 border-t border-b-0"),
                (state.nested.get() > 0).then_some("origin-top"),
            ]);
            Some(tw(&classes))
        })
        // NSH cannot parse transition-[scale,opacity,translate]; its transform flag covers both.
        .inline_style(nsh_core::element::Style::default().with(
            nsh_core::StyleKey::TransitionProperty,
            nsh_core::PropertyValue::TransitionProps(nsh_core::TransitionProps(
                nsh_core::TransitionProps::TRANSFORM | nsh_core::TransitionProps::OPACITY,
            )),
        ))
        // max-h-[92vh] / max-w-[92vw] use the same reactive viewport lengths on every target.
        .when(options.variant == Variant::Media, |v| v
            .max_h(move || px(window().height.0 * 0.92))
            .max_w(move || if options.bottom_stick_on_mobile && layout_class().is_compact() {
                auto()
            } else {
                px(window().width.0 * 0.92).into()
            }))
        .translate_y(move || px(-20.0 * state.nested.get() as f32))
        .scale(move || 1.0 - 0.1 * state.nested.get() as f32)
}

pub fn dialog_header<M>(state: DialogState, children: impl IntoChild<M>) -> impl IntoElement {
    state.has_header.set(true);
    cleanup(move || {
        let _ = state.has_header.try_set(false);
    });
    view()
        .test_id("dialog-header")
        // The source's in-:has(dialog-panel) is resolved from the registered slots.
        .app_class(move || {
            Some(tw(&cn([
                Some("flex flex-col gap-2 p-6"),
                state.has_panel.get().then_some("pb-3"),
                Some("max-sm:pb-4"),
            ])))
        })
        .child(children)
}

/// Port of alert-dialog.tsx AlertDialogHeader.
pub fn alert_dialog_header<M>(
    _state: DialogState,
    children: impl IntoChild<M>,
) -> impl IntoElement {
    view()
        .test_id("alert-dialog-header")
        .app_class(tw(
            "flex min-h-0 flex-col gap-2 p-6 text-center max-sm:pb-4 sm:text-left",
        ))
        .child(children)
}

pub fn dialog_footer<M>(
    state: DialogState,
    variant: FooterVariant,
    children: impl IntoChild<M>,
) -> impl IntoElement {
    footer(state, variant, false, children)
}

/// Port of alert-dialog.tsx AlertDialogFooter.
pub fn alert_dialog_footer<M>(
    state: DialogState,
    variant: FooterVariant,
    children: impl IntoChild<M>,
) -> impl IntoElement {
    footer(state, variant, true, children)
}

fn footer<M>(
    state: DialogState,
    variant: FooterVariant,
    alert: bool,
    children: impl IntoChild<M>,
) -> impl IntoElement {
    state.bare_footer.set(variant == FooterVariant::Bare);
    cleanup(move || {
        let _ = state.bare_footer.try_set(false);
    });
    view()
        .test_id(if alert {
            "alert-dialog-footer"
        } else {
            "dialog-footer"
        })
        .app_class(tw(&cn([
            Some("flex flex-col-reverse gap-2 px-6 sm:flex-row sm:justify-end"),
            (variant == FooterVariant::Default).then_some("border-t bg-muted/72 py-4"),
            (variant == FooterVariant::Bare && alert).then_some("pb-6"),
            (variant == FooterVariant::Bare && !alert).then_some("py-4"),
        ])))
        // sm:rounded-b-[calc(var(--radius-2xl)-1px)] is a token length minus one point.
        .rounded_b(move || {
            px(if layout_class().is_compact() {
                0.0
            } else {
                (tokens().number("radius-2xl").unwrap_or(16.0) - 1.0).max(0.0)
            })
        })
        .child(children)
}

pub fn dialog_title(state: DialogState, source: impl Fn() -> String + 'static) -> impl IntoElement {
    title_text(state, source).app_class(tw("font-semibold text-xl leading-snug"))
}

fn title_text(
    state: DialogState,
    source: impl Fn() -> String + 'static,
) -> nsh_core::element::Text {
    let source: TextSource = Rc::new(source);
    register_text(state.title, source.clone());
    // wrap-anywhere drops: NSH wraps text at words and preserves newlines.
    text(move || SharedString::from(t(&source())))
        .role(AccessibilityRole::Header)
        .accessibility_level(2u32)
}

/// Port of alert-dialog.tsx AlertDialogTitle.
pub fn alert_dialog_title(
    state: DialogState,
    source: impl Fn() -> String + 'static,
) -> impl IntoElement {
    title_text(state, source).app_class(tw("shrink-0 font-semibold text-xl leading-snug"))
}

pub fn dialog_description(
    state: DialogState,
    source: impl Fn() -> String + 'static,
) -> impl IntoElement {
    description_text(state, source).app_class(tw("text-muted-foreground text-sm"))
}

fn description_text(
    state: DialogState,
    source: impl Fn() -> String + 'static,
) -> nsh_core::element::Text {
    let source: TextSource = Rc::new(source);
    register_text(state.description, source.clone());
    text(move || SharedString::from(t(&source())))
}

fn register_text(slot: Signal<Option<TextSource>>, source: TextSource) {
    slot.set(Some(source.clone()));
    cleanup(move || {
        if slot
            .try_get()
            .flatten()
            .is_some_and(|current| Rc::ptr_eq(&current, &source))
        {
            let _ = slot.try_set(None);
        }
    });
}

/// Port of alert-dialog.tsx AlertDialogDescription.
pub fn alert_dialog_description(
    state: DialogState,
    source: impl Fn() -> String + 'static,
) -> impl IntoElement {
    // overflow-y-auto becomes scroll; wrap-anywhere and overscroll-contain drop per the port table.
    scroll()
        .test_id("alert-dialog-description")
        .app_class(tw("min-h-0"))
        // max-h-[min(24rem,60dvh)] is the lesser of 24 rem and 60% of the viewport.
        .max_h(move || {
            px((24.0 * tokens().number("text-base").unwrap_or(16.0)).min(window().height.0 * 0.6))
        })
        .child(
            description_text(state, source)
                .select_text()
                .app_class(tw("text-muted-foreground text-sm select-text")),
        )
}

pub fn dialog_panel<M>(state: DialogState, children: impl IntoChild<M>) -> impl IntoElement {
    state.has_panel.set(true);
    cleanup(move || {
        let _ = state.has_panel.try_set(false);
    });
    // ScrollArea's fade is UI-SCROLL's; native scrolling is the required overflow equivalent.
    scroll().app_class(tw("min-h-0 shrink")).child(
        view()
            .test_id("dialog-panel")
            .app_class(move || {
                let classes = cn([
                    Some("space-y-4 p-6"),
                    state.has_header.get().then_some("pt-1"),
                    state.bare_footer.get().then_some("pb-1"),
                ]);
                Some(tw(&classes))
            })
            .child(children),
    )
}
