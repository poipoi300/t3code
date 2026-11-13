//! Connection status: a static dot and its label. Nothing animates; the label says what is happening.

use nsh_core::prelude::*;
use t3_client::connection::ConnectionState;
use t3_client::connection::{PresentationPhase, connection_status_text, present_connection_state};

use super::text::{TextRole, Tone, label};
use crate::i18n::t;

pub fn tone_of(phase: PresentationPhase) -> Tone {
    match phase {
        PresentationPhase::Connected => Tone::Success,
        PresentationPhase::Connecting | PresentationPhase::Reconnecting => Tone::Warning,
        PresentationPhase::Error | PresentationPhase::Unsupported => Tone::Destructive,
        PresentationPhase::Available | PresentationPhase::Offline => Tone::Muted,
    }
}

/// The client's presentation of `state`, translated at display.
pub fn connection_status(state: impl Fn() -> ConnectionState + Copy + 'static) -> impl IntoElement {
    let presentation = move || present_connection_state(&state());
    view()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .child(
            view()
                .w(px(8.0))
                .h(px(8.0))
                .rounded_full()
                .bg(move || tone_of(presentation().phase).color()),
        )
        .child(label(TextRole::Caption, Tone::Muted, move || {
            t(&connection_status_text(&presentation()))
        }))
        .role(AccessibilityRole::Text)
        .label(move || SharedString::from(t(&connection_status_text(&presentation()))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_failure_phase_reads_as_destructive_and_progress_as_warning() {
        assert_eq!(tone_of(PresentationPhase::Error), Tone::Destructive);
        assert_eq!(tone_of(PresentationPhase::Unsupported), Tone::Destructive);
        assert_eq!(tone_of(PresentationPhase::Reconnecting), Tone::Warning);
        assert_eq!(tone_of(PresentationPhase::Connected), Tone::Success);
    }
}
