//! Port of apps/web/src/components/chat/ComposerSpeechButton.tsx ComposerSpeechButton.

use std::rc::Rc;

use nsh_core::prelude::*;
use t3_client::voice::{VoiceInputState, VoicePhase};

use crate::i18n::{Text, Value, tr, ts};
use crate::theme::tw;
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};
use crate::ui::icon::Icon;
use crate::ui::tooltip::{TooltipOptions, tooltip, tooltip_create_handle};

#[derive(Clone)]
pub struct ComposerSpeechButtonProps {
    pub state: Prop<VoiceInputState>,
    pub disabled: Prop<bool>,
    pub compact: bool,
    /// Only a composed client capture service can advertise this capability.
    pub available: Prop<bool>,
    pub unavailable_reason: Prop<Text>,
    pub shortcut: Prop<Option<String>>,
    pub on_start: Rc<dyn Fn()>,
}

pub fn composer_speech_button(props: ComposerSpeechButtonProps) -> impl IntoElement {
    let idle = computed(move || props.state.get().phase == VoicePhase::Idle);
    when(idle, move || {
        let props = props.clone();
        let hint = tooltip_create_handle();
        let disabled = props.disabled;
        let available = props.available;
        let reason = props.unavailable_reason;
        let shortcut = props.shortcut;
        let start = props.on_start.clone();
        view()
            .app_class(tw("shrink-0"))
            .accessibility_description(move || {
                if !available.get() {
                    props.unavailable_reason.get().resolve()
                } else {
                    SharedString::default()
                }
            })
            .tooltip(tooltip(
                hint,
                TooltipOptions::default(),
                text(move || {
                    if !available.get() {
                        reason.get().resolve()
                    } else {
                        shortcut
                            .get()
                            .filter(|shortcut| !shortcut.is_empty())
                            .map_or_else(
                                || ts("Start voice input"),
                                |shortcut| {
                                    tr(
                                        "Start voice input ({shortcut})",
                                        &[("shortcut", Value::from(shortcut))],
                                    )
                                    .into()
                                },
                            )
                    }
                }),
            ))
            // Button has no keep-focus/pointer-down prop; restoring editor focus is a host seam.
            .child(button(
                ButtonContent::Icon {
                    icon: Icon::Mic,
                    label: ts("Start voice input").to_string(),
                },
                ButtonVariant::Ghost,
                if props.compact {
                    ButtonSize::IconXs
                } else {
                    ButtonSize::IconSm
                },
                move || disabled.get() || !available.get(),
                move || {
                    if !disabled.get_untracked() && available.get_untracked() {
                        start();
                    }
                },
            ))
    })
}

/// The source's unavailable capability, until Platform supplies bounded streaming PCM capture.
/// See client-seam-requests.md: basic start_recording/stop_recording cannot supply this contract.
pub fn composer_voice_input() -> impl IntoElement {
    composer_speech_button(ComposerSpeechButtonProps {
        state: Prop::new(VoiceInputState::default()),
        disabled: Prop::new(true),
        compact: false,
        available: Prop::new(false),
        unavailable_reason: Prop::new(Text::client_key("Voice transcription is not available.")),
        shortcut: Prop::new(None),
        on_start: Rc::new(|| {}),
    })
}
