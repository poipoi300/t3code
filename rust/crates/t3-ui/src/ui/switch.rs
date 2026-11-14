//! Port of apps/web/src/components/ui/switch.tsx Switch.
//!
//! Also ports checkbox.tsx (Checkbox, CheckboxMark), radio-group.tsx (RadioGroup,
//! Radio, RadioGroupItem) and label.tsx (Label). All controls are controlled:
//! `ControlState` takes reactive checked, mixed, disabled and invalid props; callbacks
//! request a value and never mutate caller state. Labels take English source keys
//! and translate reactively. `Size::{Default, Sm}` belongs to Switch;
//! `Variant::{Default, Contrast}` belongs to Checkbox and CheckboxMark. Other
//! components have no variant/size in TSX. Return values intentionally hide styling.
//! RadioGroup accepts child elements; NSH builds them in its roving-focus context.
//! Label takes child elements and an activation callback (NSH has no HTML `htmlFor`).

use nsh_core::{
    PropertyValue, StyleDecl, StyleKey, TransitionProps,
    element::{ImageSrc, IntoChild, Style},
    prelude::*,
};

use crate::i18n::ts;
use crate::theme::{Density, Scope, cn, metrics, palette, tw, tw_with};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Size {
    #[default]
    Default,
    Sm,
}

pub use self::Size as SwitchSize;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Variant {
    #[default]
    Default,
    Contrast,
}

pub use self::Variant as CheckboxVariant;

#[derive(Clone, Copy, Debug, Default)]
pub struct ControlState {
    pub checked: Prop<bool>,
    /// Switch's `mixed` and Checkbox's `indeterminate`; ignored by Radio.
    pub mixed: Prop<bool>,
    pub disabled: Prop<bool>,
    /// Visual invalid state; NSH does not expose invalid semantics on these controls.
    pub invalid: Prop<bool>,
}

impl ControlState {
    fn attrs(self) -> Vec<(&'static str, &'static str)> {
        self.attrs_with_checked(self.checked.get() && !self.mixed.get())
    }

    fn attrs_with_checked(self, checked: bool) -> Vec<(&'static str, &'static str)> {
        let mut attrs = vec![if checked {
            ("data-checked", "")
        } else {
            ("data-unchecked", "")
        }];
        if self.mixed.get() {
            attrs.extend([("data-mixed", ""), ("data-indeterminate", "")]);
        }
        if self.disabled.get() {
            attrs.push(("data-disabled", ""));
        }
        if self.invalid.get() {
            attrs.push(("aria-invalid", "true"));
        }
        attrs
    }
}

fn thumb_size(size: Size) -> f32 {
    match (size, Density::of(window().width.0)) {
        (Size::Default, Density::Large) => 20.0,
        (Size::Default, Density::Regular) | (Size::Sm, Density::Large) => 16.0,
        (Size::Sm, Density::Regular) => 14.0,
    }
}

pub(crate) fn control_hit_slop(width: f32, height: f32) -> EdgeInsets {
    let target = metrics().hit_target;
    EdgeInsets {
        top: px(((target - height) / 2.0).max(0.0)),
        bottom: px(((target - height) / 2.0).max(0.0)),
        left: px(((target - width) / 2.0).max(0.0)),
        right: px(((target - width) / 2.0).max(0.0)),
    }
}

// NSH's class parser lacks arbitrary transition-property lists; the native style
// accepts the same property mask without widening it to layout changes.
pub(crate) fn transition_style(properties: u8, ease: Option<[f32; 4]>) -> Style {
    let properties = if reduce_motion() { 0 } else { properties };
    let mut declarations = vec![StyleDecl::Set(
        StyleKey::TransitionProperty,
        PropertyValue::TransitionProps(TransitionProps(properties)),
    )];
    if let Some(ease) = ease {
        declarations.push(StyleDecl::Set(
            StyleKey::TransitionTimingFunction,
            PropertyValue::Easing(ease.map(Into::into)),
        ));
    }
    Style::from_decls(declarations)
}

pub fn switch(
    source: impl Into<SharedString>,
    state: ControlState,
    size: Size,
    on_change: impl Fn(bool) + 'static,
) -> impl IntoElement {
    let source = stored(source.into());
    // NSH Switch has no mixed state; Checkbox supplies Mixed with a Switch role.
    // Its mandatory indicator is the track; next value still negates checked.
    let props = nsh_base::CheckboxProps {
        role: Some(AccessibilityRole::Switch).into(),
        checked: state.checked,
        indeterminate: state.mixed,
        disabled: state.disabled,
        accessibility_label: Prop::bind(move || Some(source.with(|s| ts(s)))),
        hit_slop: Prop::bind(move || {
            control_hit_slop(thumb_size(size) * 2.0 - 2.0, thumb_size(size) + 2.0)
        }),
        indicator_class: Prop::bind(move || {
            Some(tw_with(
                Scope::Root,
                "inline-flex shrink-0 cursor-pointer items-center rounded-full p-[2px] outline-none duration-200 data-checked:bg-primary data-unchecked:bg-input data-disabled:cursor-not-allowed data-disabled:opacity-64 data-[mixed]:bg-input",
                &state.attrs_with_checked(state.checked.get()),
            ))
        }),
        // Subtree --thumb-size and calc lengths resolve at the web's sm breakpoint.
        // Direct Props accept bound Style values, unlike NSH's part-style setters.
        indicator_style: Prop::bind(move || {
            Style::from(
                nsh_core::element::StyleRefinement::default()
                    .h(px(thumb_size(size) + 2.0))
                    .w(px(thumb_size(size) * 2.0 - 2.0)),
            )
            .then(&transition_style(
                TransitionProps::COLORS | TransitionProps::SHADOW,
                None,
            ))
        }),
        ..Props::initial()
    };
    nsh_base::Checkbox::element(props)
        .on_event(move |event: &nsh_base::CheckboxEvent| {
            if matches!(event, nsh_base::CheckboxEvent::Change(_)) && !state.disabled.get_untracked() {
                on_change(!state.checked.get_untracked());
            }
        })
        .app_class(tw("group inline-flex shrink-0 outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-background"))
        .fill("indicator", view()
            // NSH has no elliptical radius and uses one clock for translate/scale/origin:
            // retain 150 ms transforms without the source's separate delayed scale clock.
            .app_class(move || Some(tw_with(Scope::Root,
                &cn([Some("pointer-events-none block shrink-0 origin-left rounded-full bg-background shadow-sm/5 dark:data-unchecked:bg-muted-foreground data-checked:origin-right transition-transform duration-150 motion-reduce:transition-none"),
                    (!state.disabled.get()).then_some("group-active:scale-x-110 group-active/form-label:scale-x-110"),
                    state.mixed.get().then_some("opacity-70")]), &state.attrs_with_checked(state.checked.get()))))
            .size(move || px(thumb_size(size) - 2.0))
            .translate_x(move || {
                let thumb = thumb_size(size);
                px(if state.mixed.get() { (thumb - 4.0) / 2.0 }
                    else if state.checked.get() { thumb - 4.0 } else { 0.0 })
            }))
}

fn choice_root(state: ControlState, variant: Variant, round: bool) -> SharedString {
    // not-* and selector shadow resets are conditions; bg-clip-padding has no
    // NSH paint meaning (border and background already share this box).
    let classes = cn([
        Some(
            "group relative inline-flex size-4.5 shrink-0 items-center justify-center border border-input bg-background shadow-xs/5 outline-none transition-shadow focus-visible:outline-2 focus-visible:outline-solid focus-visible:-outline-offset-2 focus-visible:outline-ring aria-invalid:border-destructive/36 focus-visible:aria-invalid:border-destructive/64 focus-visible:aria-invalid:outline-destructive/48 data-disabled:opacity-64 sm:size-4 dark:focus-visible:aria-invalid:outline-destructive/24",
        ),
        Some(if round {
            "rounded-full"
        } else {
            "rounded-[.25rem]"
        }),
        (!state.checked.get() || state.mixed.get()).then_some("dark:bg-input/32"),
        (state.disabled.get()
            || (state.checked.get() && !state.mixed.get())
            || state.invalid.get())
        .then_some("shadow-none"),
        (variant == Variant::Contrast).then_some("border-foreground/55"),
        // NSH's component props have no aria-invalid; resolve its paint here.
        state.invalid.get().then_some("border-destructive/36 focus-visible:border-destructive/64 focus-visible:outline-destructive/48 dark:focus-visible:outline-destructive/24"),
    ]);
    tw_with(Scope::Root, &classes, &state.attrs())
}

fn choice_before(state: ControlState, round: bool) -> impl IntoElement {
    view().app_class(move || {
        Some(tw(&cn([
            Some("pointer-events-none absolute inset-0"),
            Some(if round {
                "rounded-full"
            } else {
                "rounded-[3px]"
            }),
            (!state.disabled.get()
                && (!state.checked.get() || state.mixed.get())
                && !state.invalid.get())
            .then_some("shadow-[0_1px_rgb(0_0_0/4%)] dark:shadow-[0_-1px_rgb(255_255_255/6%)]"),
        ])))
    })
}

fn choice_focus_classes(state: ControlState) -> String {
    // Resolve outline-inherit from the parent, and replace its token-valued inset
    // shadow with the equivalent three-point inset ring (neither parses in NSH).
    cn([
        Some("group-focus-visible:ring-[3px] group-focus-visible:ring-inset group-focus-visible:ring-background group-focus-visible:outline-2 group-focus-visible:outline-solid group-focus-visible:-outline-offset-2 group-focus-visible:outline-ring"),
        state.invalid.get().then_some("group-focus-visible:outline-destructive/48 dark:group-focus-visible:outline-destructive/24"),
    ])
}

fn checkbox_indicator(state: ControlState) -> impl IntoElement {
    view().app_class(move || {
        let mut attrs = state.attrs();
        attrs.retain(|(key, _)| *key != "data-unchecked" || !state.mixed.get());
        let classes = cn([Some("-inset-px absolute flex items-center justify-center rounded-[.25rem] text-primary-foreground data-unchecked:hidden data-checked:bg-primary data-indeterminate:text-foreground"), Some(&choice_focus_classes(state))]);
        Some(tw_with(Scope::Root, &classes, &attrs))
    })
        // The parent's focus selector becomes NSH's group-focus-visible.
        .child(svg(move || ImageSrc::svg_markup(if state.mixed.get() {
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><path d="M5.252 12h13.496"/></svg>"#
        } else {
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><path d="M5.252 12.7 10.2 18.63 18.748 5.37"/></svg>"#
        })).app_class(tw("size-3.5 text-current sm:size-3"))
            .text_color(move || if state.mixed.get() { palette().foreground } else { palette().primary_foreground })
            .accessible(false))
}

/// Port of apps/web/src/components/ui/checkbox.tsx Checkbox.
pub fn checkbox(
    source: impl Into<SharedString>,
    state: ControlState,
    variant: Variant,
    on_change: impl Fn(bool) + 'static,
) -> impl IntoElement {
    let source = stored(source.into());
    nsh_base::checkbox()
        .checked(state.checked)
        .indeterminate(state.mixed)
        .disabled(state.disabled)
        .accessibility_label(move || Some(source.with(|s| ts(s))))
        .on_change(move |value: &bool| {
            if !state.disabled.get_untracked() {
                on_change(*value);
            }
        })
        .app_class(move || Some(choice_root(state, variant, false)))
        .hit_slop(move || {
            let side = if metrics().control == 36.0 {
                18.0
            } else {
                16.0
            };
            control_hit_slop(side, side)
        })
        // NSH's mandatory indicator wraps the TSX before layer and indicator.
        .indicator_class(tw("absolute inset-0"))
        .indicator(
            view()
                .app_class(tw("absolute inset-0"))
                .child(choice_before(state, false))
                .child(checkbox_indicator(state)),
        )
}

/// Port of apps/web/src/components/ui/checkbox.tsx CheckboxMark; decorative and inert.
pub fn checkbox_mark(checked: impl Into<Prop<bool>>, variant: Variant) -> impl IntoElement {
    let state = ControlState {
        checked: checked.into(),
        ..Default::default()
    };
    view()
        .accessible(false)
        .accessibility_elements_hidden(true)
        .app_class(move || Some(choice_root(state, variant, false)))
        .child(choice_before(state, false))
        .child(checkbox_indicator(state))
}

/// Port of apps/web/src/components/ui/radio-group.tsx RadioGroup.
pub fn radio_group<M>(
    source: impl Into<SharedString>,
    children: impl IntoChild<M>,
) -> impl IntoElement {
    let source = stored(source.into());
    nsh_base::radio_group()
        .accessibility_label(move || Some(source.with(|s| ts(s))))
        .app_class(tw("flex flex-col gap-3"))
        .child(children)
}

/// Port of apps/web/src/components/ui/radio-group.tsx Radio (also RadioGroupItem).
pub fn radio(
    source: impl Into<SharedString>,
    state: ControlState,
    on_change: impl Fn(bool) + 'static,
) -> impl IntoElement {
    let source = stored(source.into());
    let state = ControlState {
        mixed: false.into(),
        ..state
    };
    nsh_base::radio().checked(state.checked).disabled(state.disabled)
        .accessibility_label(move || Some(source.with(|s| ts(s))))
        .on_change(move |value: &bool| { if !state.disabled.get_untracked() { on_change(*value); } })
        .app_class(move || Some(choice_root(state, Variant::Default, true)))
        .hit_slop(move || { let side = if metrics().control == 36.0 { 18.0 } else { 16.0 }; control_hit_slop(side, side) })
        .child(choice_before(state, true))
        .child(view().app_class(move || {
            let classes = cn([Some("-inset-px absolute flex size-4.5 items-center justify-center rounded-full data-unchecked:hidden data-checked:bg-primary sm:size-4"), Some(&choice_focus_classes(state))]);
            Some(tw_with(Scope::Root, &classes, &state.attrs()))
        })

            // The indicator's ::before is a real first child.
            .child(view().app_class(tw("size-2 rounded-full bg-primary-foreground sm:size-1.5"))))
}

pub use radio as radio_group_item;

/// Port of apps/web/src/components/ui/label.tsx Label. The callback activates its associated
/// control; use its disabled guard too (NSH has no HTML label association).
pub fn label<M>(children: impl IntoChild<M>, activate: impl Fn() + 'static) -> impl IntoElement {
    view()
        .app_class(tw(
            "group/form-label inline-flex items-center gap-2 text-base/4.5 sm:text-sm/4 font-medium text-foreground",
        ))
        .on_press(activate)
        .focusable(false)
        .child(children)
}
