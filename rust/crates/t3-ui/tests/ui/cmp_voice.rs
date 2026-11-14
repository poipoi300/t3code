//! CMP-VOICE: unsupported capture remains inert when the control is pressed.

use crate::support::{DESKTOP, PHONE};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use std::rc::Rc;
use t3_client::voice::{VoiceInputState, VoicePhase};
use t3_ui::{
    i18n::Text,
    screens::composer::voice::{ComposerSpeechButtonProps, composer_speech_button},
};

#[component]
fn VoiceHarness() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let available = signal(false);
    let disabled = signal(false);
    let phase = signal(VoiceInputState::default());
    let starts = signal(0usize);
    view()
        .w_full()
        .p(px(16.0))
        .child(composer_speech_button(ComposerSpeechButtonProps {
            state: Prop::bind(move || phase.get()),
            disabled: Prop::bind(move || disabled.get()),
            compact: false,
            available: Prop::bind(move || available.get()),
            unavailable_reason: Prop::new(Text::client_key(
                "Voice transcription is not available.",
            )),
            shortcut: Prop::new(None),
            on_start: Rc::new(move || starts.update(|starts| *starts += 1)),
        }))
        .child(
            nsh_base::button()
                .accessibility_label(Some("Enable capture".into()))
                .child(text(String::from("Enable capture")))
                .on_press(move || available.set(true)),
        )
        .child(
            nsh_base::button()
                .accessibility_label(Some("Disable editing".into()))
                .child(text(String::from("Disable editing")))
                .on_press(move || disabled.set(true)),
        )
        .child(
            nsh_base::button()
                .accessibility_label(Some("Preparing".into()))
                .child(text(String::from("Preparing")))
                .on_press(move || {
                    phase.set(VoiceInputState {
                        phase: VoicePhase::Preparing,
                        ..Default::default()
                    })
                }),
        )
        .child(text(move || format!("Starts {}", starts.get())))
}

#[test]
fn microphone_requires_both_capture_capability_and_edit_permission() {
    for window in [DESKTOP, PHONE] {
        let mut app = Headless::start(EngineConfig {
            ui_sets: t3_ui::registries(),
            viewport: size(px(window.0), px(window.1)),
            ..EngineConfig::new(Root::of::<VoiceHarness>())
        })
        .expect("voice harness");
        app.wait_for("role:button[Start voice input]")
            .expect("unavailable control remains visible");
        assert!(
            !app.screen()
                .enabled("role:button[Start voice input]")
                .expect("button")
        );
        assert!(
            app.press("role:button[Start voice input]")
                .expect_err("unsupported capture")
                .contains("disabled")
        );
        assert!(app.screen().shows("Starts 0"));
        app.press("role:button[Enable capture]")
            .expect("capability");
        app.press("role:button[Start voice input]")
            .expect("enabled press");
        app.wait_for("Starts 1").expect("one start");
        app.press("role:button[Disable editing]").expect("deny");
        assert!(
            app.press("role:button[Start voice input]")
                .expect_err("editing disabled")
                .contains("disabled")
        );
        assert!(app.screen().shows("Starts 1"));
        app.press("role:button[Preparing]").expect("phase changes");
        assert!(
            !app.screen()
                .matches("role:button[Start voice input]")
                .expect("microphone selector"),
            "the idle microphone is absent during capture"
        );
    }
}
