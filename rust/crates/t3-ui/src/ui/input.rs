//! `components/ui/input.tsx`: a single-line field bound to a signal.

use nsh_core::prelude::*;

use crate::theme::{metrics, palette};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InputSize {
    #[default]
    Default,
    Lg,
}

/// A text field. `placeholder` and `label` are already translated; `label` is the accessible name.
pub fn text_field(
    value: Signal<String>,
    placeholder: String,
    label: String,
    size: InputSize,
    on_submit: impl Fn() + 'static,
) -> impl IntoElement {
    field(value, placeholder, label, size, false, || false, on_submit)
}

/// `<Input type="password" disabled={..}>`: the same field with what is typed hidden.
pub fn masked_text_field(
    value: Signal<String>,
    placeholder: String,
    label: String,
    disabled: impl Fn() -> bool + 'static,
    on_submit: impl Fn() + 'static,
) -> impl IntoElement {
    field(value, placeholder, label, InputSize::Default, true, disabled, on_submit)
}

fn field(
    value: Signal<String>,
    placeholder: String,
    label: String,
    size: InputSize,
    masked: bool,
    disabled: impl Fn() -> bool + 'static,
    on_submit: impl Fn() + 'static,
) -> impl IntoElement {
    let height = move || {
        let m = metrics();
        px(match size {
            InputSize::Default => m.control,
            InputSize::Lg => m.control_lg,
        })
    };
    nsh_base::input()
        .masked(masked)
        .disabled(disabled)
        .value(move || value.get())
        .on_change(move |text: &str| value.set(text.to_owned()))
        .on_submit(move |_: &str| on_submit())
        .submit_behavior(SubmitBehavior::Submit)
        .placeholder(placeholder)
        .placeholder_color(move || palette().placeholder.dark)
        .label(SharedString::from(label))
        .h(height)
        .w_full()
        .px(px(11.0))
        .rounded(move || px(metrics().radius))
        .border_1()
        .border_color(move || palette().input)
        .bg(move || AdaptiveColor {
            light: palette().background.light,
            dark: palette().input.opacity(0.32).dark,
        })
        .text_size(move || px(metrics().text))
        .text_color(move || palette().foreground)
        .focus_visible(|s| s.ring(px(2.0)).ring_inset().ring_color(palette().ring))
}
