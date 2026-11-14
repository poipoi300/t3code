//! Port of apps/web/src/components/ui/number-field.tsx NumberField and its slots.
//!
//! `number_field(props, Size, on_change, |state| children)` is the root;
//! `number_field_group(state, children)` and `number_field_input(state)` are slots.
//! `number_field_control(props, Size, on_change)` composes the usual group/input.
//! `NumberFieldProps::value` takes `None` for an empty input.
//! NSH treats an initially empty value as uncontrolled until the first number;
//! refusing that first edit needs an upstream explicit controlled-mode prop.
//! `Size::{Sm, Default, Lg}` matches TSX; there is no Variant.
//! The public group accepts child elements. NSH NumberInput owns input and step-button
//! behavior together: `number_field_input` renders these three slots as one unit;
//! decrement/increment functions provide their decorative glyph slots.
//! NSH hides step buttons from assistive technology; the spinbutton supplies its
//! increment/decrement accessibility actions instead.
//! `slider(SliderProps, on_change, on_change_end)` ports AppearanceSettingsPanel's
//! range input and index.css `.settings-slider`; no web slider.tsx exists.
//! Source labels and placeholders translate reactively; callers cannot restyle controls.

use nsh_core::{element::ImageSrc, prelude::*};

use crate::i18n::ts;
use crate::theme::{cn, tw};

pub use nsh_base::SliderValue;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Size {
    Sm,
    #[default]
    Default,
    Lg,
}

pub use self::Size as NumberFieldSize;

#[derive(Clone, Copy)]
pub struct NumberFieldState {
    props: Stored<NumberFieldProps>,
    size: Size,
    on_change: Stored<std::rc::Rc<dyn Fn(Option<f64>)>>,
}

#[derive(Clone, Debug)]
pub struct NumberFieldProps {
    pub label: SharedString,
    pub placeholder: Option<SharedString>,
    pub value: Prop<Option<f64>>,
    pub min: Prop<Option<f64>>,
    pub max: Prop<Option<f64>>,
    pub step: Prop<f64>,
    pub disabled: Prop<bool>,
    pub readonly: Prop<bool>,
    /// Group styling; NSH does not yet expose an inner-input invalid-state prop.
    pub invalid: Prop<bool>,
}

impl NumberFieldProps {
    pub fn new(label: impl Into<SharedString>, value: impl Into<Prop<Option<f64>>>) -> Self {
        Self {
            label: label.into(),
            placeholder: None,
            value: value.into(),
            min: None.into(),
            max: None.into(),
            step: 1.0.into(),
            disabled: false.into(),
            readonly: false.into(),
            invalid: false.into(),
        }
    }
}

pub fn number_field<E: IntoElement>(
    props: NumberFieldProps,
    size: Size,
    on_change: impl Fn(Option<f64>) + 'static,
    children: impl FnOnce(NumberFieldState) -> E,
) -> impl IntoElement {
    let state = NumberFieldState {
        props: stored(props),
        size,
        on_change: stored(std::rc::Rc::new(on_change)),
    };
    view()
        .app_class(tw("flex w-full flex-col items-start gap-2"))
        .child(children(state))
}

/// The standard NumberField > NumberFieldGroup > input/decrement/increment composition.
pub fn number_field_control(
    props: NumberFieldProps,
    size: Size,
    on_change: impl Fn(Option<f64>) + 'static,
) -> impl IntoElement {
    number_field(props, size, on_change, |state| {
        number_field_group(state, number_field_input(state))
    })
}

/// Port of NumberFieldGroup; selector variants resolve from the input's invalid state.
pub fn number_field_group(state: NumberFieldState, children: impl IntoElement) -> impl IntoElement {
    let props = state.props.get();
    let disabled = props.disabled;
    let invalid = props.invalid;
    view().disabled(disabled).app_class(move || Some(tw(&cn([
        Some("group relative flex w-full justify-between rounded-lg border border-input bg-background text-base text-foreground shadow-xs/5 ring-ring/24 transition-shadow focus-within:border-ring focus-within:ring-[3px] focus-within:ring-inset focus-within:shadow-none sm:text-sm dark:bg-input/32"),
        invalid.get().then_some("border-destructive/36 focus-within:border-destructive/64 focus-within:ring-destructive/48 dark:ring-destructive/24"),
        disabled.get().then_some("pointer-events-none opacity-64 shadow-none"),
        invalid.get().then_some("shadow-none"),
    ]))))
        // ::before is the first child. bg-clip-padding and browser autofill's
        // five-million-second transition have no NSH native-input equivalent.
        .child(view().app_class(move || Some(tw(&cn([
            Some("pointer-events-none absolute inset-0 rounded-[calc(var(--radius-lg)-1px)] group-focus-within:shadow-none"),
            (!disabled.get() && !invalid.get()).then_some("shadow-[0_1px_rgb(0_0_0/4%)] dark:shadow-[0_-1px_rgb(255_255_255/6%)]"),
        ])))))
        .child(children)
}

/// Port of NumberFieldInput; NSH exposes input and stepping controls as one primitive.
pub fn number_field_input(state: NumberFieldState) -> impl IntoElement {
    let props = state.props.get();
    let size = state.size;
    let disabled = props.disabled;
    let readonly = props.readonly;
    let label = stored(props.label);
    let placeholder = stored(props.placeholder);
    nsh_base::number_input().value(props.value).min(props.min).max(props.max).step(props.step)
        .disabled(disabled).readonly(readonly)
        .accessibility_label(move || Some(label.with(|s| ts(s))))
        .placeholder(move || placeholder.with(|s| s.as_ref().map(|s| ts(s))))
        .on_change(move |value: &Option<f64>| {
            if !disabled.get_untracked() && !readonly.get_untracked() { state.on_change.with(|change| change(*value)); }
        })
        .app_class(tw("relative flex w-full min-w-0 items-stretch"))
        .input_class(tw(&cn([
            Some("h-8.5 w-full min-w-0 grow bg-transparent px-[11px] text-center tabular-nums leading-8.5 outline-none sm:h-7.5 sm:leading-7.5"),
            (size == Size::Sm).then_some("h-7.5 px-[9px] leading-7.5 sm:h-6.5 sm:leading-8.5"),
            (size == Size::Lg).then_some("h-9.5 leading-9.5 sm:h-8.5 sm:leading-8.5"),
        ])))
        .decrement_button_class(step_button_class(size, false))
        .increment_button_class(step_button_class(size, true))
        .decrement_button(number_field_decrement())
        .increment_button(number_field_increment())
}

fn step_button_class(size: Size, increment: bool) -> SharedString {
    // Ancestor size selector resolves from Size; --spacing(n) is 4n px.
    tw(&cn([
        Some(
            "relative flex shrink-0 cursor-pointer items-center justify-center px-[11px] transition-colors hover:bg-accent",
        ),
        Some(if increment {
            "rounded-e-[calc(var(--radius-lg)-1px)]"
        } else {
            "rounded-s-[calc(var(--radius-lg)-1px)]"
        }),
        (size == Size::Sm).then_some("px-[9px]"),
    ]))
}

fn step_icon(increment: bool) -> impl IntoElement {
    view().app_class(tw("relative flex items-center justify-center"))
        .child(svg(ImageSrc::svg_markup(if increment {
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M5 12h14M12 5v14"/></svg>"#
    } else {
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M5 12h14"/></svg>"#
    })).app_class(tw("size-4.5 sm:size-4 pointer-events-none shrink-0"))
        .text_color(move || crate::theme::palette().foreground).accessible(false))
        // The button's pointer-coarse ::after becomes a last hit layer in its
        // mandatory NSH content slot, centred on the button's glyph.
        .child(view().accessible(false).app_class(move || Some(tw(
            if crate::state::layout::pointer() == crate::state::layout::Pointer::Coarse {
                "absolute inset-0 m-auto size-full min-h-11 min-w-11"
            } else { "hidden" }
        ))))
}

/// Port of NumberFieldDecrement's MinusIcon content (NSH owns its button).
pub fn number_field_decrement() -> impl IntoElement {
    step_icon(false)
}

/// Port of NumberFieldIncrement's PlusIcon content (NSH owns its button).
pub fn number_field_increment() -> impl IntoElement {
    step_icon(true)
}

#[derive(Clone, Debug)]
pub struct SliderProps {
    pub label: SharedString,
    pub value: Prop<f64>,
    pub min: Prop<f64>,
    pub max: Prop<f64>,
    pub step: Prop<f64>,
    pub disabled: Prop<bool>,
}

impl SliderProps {
    pub fn new(label: impl Into<SharedString>, value: impl Into<Prop<f64>>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            min: 0.0.into(),
            max: 100.0.into(),
            step: 1.0.into(),
            disabled: false.into(),
        }
    }
}

/// Port of AppearanceSettingsPanel.tsx range input; on_change is live, as on the web.
pub fn slider(
    props: SliderProps,
    on_change: impl Fn(f64) + 'static,
    on_change_end: impl Fn(f64) + 'static,
) -> impl IntoElement {
    let source = stored(props.label);
    let disabled = props.disabled;
    nsh_base::slider().value(move || Some(SliderValue::Single(props.value.get())))
        .min(props.min).max(props.max).step(props.step).disabled(disabled)
        .accessibility_label(move || Some(source.with(|s| ts(s))))
        .on_change(move |value: &SliderValue| { if !disabled.get_untracked() { on_change(value.end()); } })
        .on_change_end(move |value: &SliderValue| { if !disabled.get_untracked() { on_change_end(value.end()); } })
        .app_class(tw("min-w-0 flex-1 outline-none disabled:opacity-64"))
        // index.css .settings-slider vendor pseudo-elements become NSH parts.
        // Native range fill replaces the browser gradient's half-thumb fill offset.
        .track_class(tw("h-6 justify-center cursor-pointer"))
        .indicator_class(tw("h-1.5 rounded-full bg-muted-foreground/22 ring-1 ring-inset ring-border/55"))
        .range_class(tw("rounded-full bg-primary"))
        .thumb_class(tw("size-4 border-2 border-primary rounded-full bg-background shadow-[0_1px_2px_rgb(0_0_0/12%)] shadow-foreground/12 group-hover:scale-[1.08] group-hover:shadow-[0_1px_3px_rgb(0_0_0/20%)] group-hover:shadow-foreground/20 active:scale-[0.94] focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-background transition-[transform,box-shadow] duration-120 motion-reduce:transition-none"))
}
