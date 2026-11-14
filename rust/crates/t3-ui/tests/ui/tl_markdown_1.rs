//! TL-MARKDOWN-1: wrap defaults, copy outcomes and guarded shell actions through NSH.

use std::rc::Rc;
use std::time::{Duration, SystemTime};

use futures::FutureExt;
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use t3_ui::i18n::t;
use t3_ui::screens::thread::markdown::code_block::{
    CodeBlockOptions, CodeBlockProps, CodeBlockState, markdown_code_block,
};

use crate::support::{DESKTOP, PHONE};

#[component]
fn CodeBlockFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let props = signal(CodeBlockProps {
        code: "echo hello\n".into(),
        language: "bash".into(),
        default_wrapped: false,
        run_shell_command_available: true,
        ..Default::default()
    });
    let current = computed(move || props.get());
    let state = CodeBlockState::new();
    let fail = signal(false);
    let count = signal(0);
    let last_command = signal(String::new());
    let options = CodeBlockOptions {
        on_run_shell_command: Some(Rc::new(move |command| {
            count.update(|count| *count += 1);
            last_command.set(command);
        })),
        copy: Some(Rc::new(move |code| {
            async move {
                if fail.get_untracked() {
                    Err("Clipboard permission denied".into())
                } else {
                    set_clipboard(code.clone());
                    if clipboard_string().await == code {
                        Ok(())
                    } else {
                        Err("Clipboard did not receive the code".into())
                    }
                }
            }
            .boxed_local()
        })),
        ..Default::default()
    };
    view().w_full().h_full().child(
        view()
            .app_class(t3_ui::theme::tw("p-3 gap-2"))
            .child(control("Change wrap default", move || {
                props.update(|p| p.default_wrapped = !p.default_wrapped)
            }))
            .child(control("Stream block", move || {
                props.update(|p| p.is_streaming = !p.is_streaming)
            }))
            .child(control("Unclosed fence", move || {
                props.update(|p| p.run_shell_command_available = false)
            }))
            .child(control("Unsafe command", move || {
                props.update(|p| p.code = "echo one\necho two\n".into())
            }))
            .child(control("New code", move || {
                props.update(|p| p.code = "echo changed\n".into())
            }))
            .child(control("Fail clipboard", move || fail.set(true)))
            .child(control("Diagram", move || {
                props.update(|p| p.diagram = !p.diagram)
            }))
            .child(control("Hide wrap", move || {
                props.update(|p| p.can_wrap = false)
            }))
            .child(markdown_code_block(
                current,
                state,
                options,
                move |wrapped| {
                    view()
                        .app_class(t3_ui::theme::tw("px-3 pb-3 pt-1.5"))
                        .child(
                            text(move || SharedString::from(current.with(|p| p.code.clone())))
                                .select_text(),
                        )
                        .child(text(move || {
                            SharedString::from(if wrapped.get() {
                                "wrapped"
                            } else {
                                "unwrapped"
                            })
                        }))
                        .into_element()
                },
            ))
            .child(text(move || {
                SharedString::from(format!("Runs: {} {}", count.get(), last_command.get()))
            }))
            .child(text(move || {
                SharedString::from(state.copy_error.get().unwrap_or_default())
            })),
    )
}

fn control(label: &'static str, action: impl Fn() + 'static) -> impl IntoElement {
    nsh_base::button()
        .accessibility_label(Some(t(label).into()))
        .on_press(action)
        .child(text(t(label)))
}

fn start(window: (f32, f32)) -> Headless {
    let mut app = Headless::start(EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        clock: nsh_core::time::Clock::manual(SystemTime::UNIX_EPOCH),
        ..EngineConfig::new(Root::of::<CodeBlockFixture>())
    })
    .expect("code block fixture");
    app.wait_for("echo hello").expect("fixture mounted");
    app
}

#[test]
fn changed_wrap_default_updates_a_mounted_block_and_a_user_override_wins_at_both_sizes() {
    for size in [DESKTOP, PHONE] {
        let mut app = start(size);
        app.wait_for("unwrapped").expect("initial default");
        app.press("role:button[Change wrap default]")
            .expect("setting changes");
        app.wait_for("wrapped")
            .expect("new default reaches the mounted content");
        app.press("role:button[Disable line wrap]")
            .expect("override");
        app.wait_for("unwrapped").expect("override reaches content");
        app.press("role:button[Change wrap default]")
            .expect("false default");
        app.press("role:button[Change wrap default]")
            .expect("true default");
        assert!(
            app.screen().shows("unwrapped"),
            "local override survives setting changes"
        );
        app.press("role:button[Wrap lines]")
            .expect("reverse override");
        app.wait_for("wrapped").expect("wrap enabled again");
        app.press("role:button[Hide wrap]")
            .expect("canWrap changes");
        assert!(app.press("role:button[Disable line wrap]").is_err());
    }
}

#[test]
fn copy_preserves_source_resets_feedback_and_does_not_report_failure_as_success() {
    for size in [DESKTOP, PHONE] {
        let mut app = start(size);
        app.press("role:button[Copy code]").expect("copy");
        assert_eq!(app.screen().clipboard(), Some("echo hello\n"));
        app.press("role:button[Copied]").expect("copy again");
        app.run_for(Duration::from_millis(1199))
            .expect("advance feedback clock");
        app.press("role:button[Copied]")
            .expect("feedback survives until reset and restarts");
        app.run_for(Duration::from_millis(1200))
            .expect("feedback expires");
        app.press("role:button[New code]").expect("new source");
        app.press("role:button[Copy code]")
            .expect("copy latest source");
        assert_eq!(app.screen().clipboard(), Some("echo changed\n"));
        app.press("role:button[Fail clipboard]")
            .expect("denied permission");
        app.press("role:button[Copied]")
            .expect("attempt denied copy");
        app.wait_for("Clipboard permission denied")
            .expect("failure retained");
        app.run_for(Duration::from_millis(1200))
            .expect("previous successful feedback expires");
        assert!(
            app.press("role:button[Copied]").is_err(),
            "no invented success"
        );
        app.press("role:button[Copy code]")
            .expect("retry remains available");
    }
}

#[test]
fn shell_actions_follow_streaming_and_current_source_and_diagrams_only_keep_copy() {
    for size in [DESKTOP, PHONE] {
        let mut app = start(size);
        app.press("role:button[Stream block]").expect("streaming");
        assert!(app.press("role:button[Run in terminal]").is_err());
        app.press("role:button[Stream block]").expect("finished");
        app.press("role:button[Run in terminal]")
            .expect("run complete block");
        app.wait_for("Runs: 1 echo hello")
            .expect("trimmed command exactly once");
        app.press("role:button[New code]").expect("replace source");
        app.press("role:button[Run in terminal]")
            .expect("run current source");
        app.wait_for("Runs: 2 echo changed")
            .expect("no stale source");
        app.press("role:button[Unsafe command]")
            .expect("multiple lines");
        assert!(app.press("role:button[Run in terminal]").is_err());
        app.press("role:button[New code]").expect("safe source");
        app.press("role:button[Unclosed fence]")
            .expect("caller withholds shell action for an unclosed fence");
        assert!(app.press("role:button[Run in terminal]").is_err());
        app.press("role:button[Unsafe command]")
            .expect("diagram source");
        app.press("role:button[Diagram]").expect("diagram");
        assert!(app.press("role:button[Wrap lines]").is_err());
        app.press("role:button[Copy code]")
            .expect("diagram source can still copy");
        assert_eq!(app.screen().clipboard(), Some("echo one\necho two\n"));
        app.press("role:button[Diagram]").expect("code view");
        app.press("role:button[Wrap lines]").expect("wrap restored");
    }
}

#[component]
fn NativeClipboardFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let source = signal(CodeBlockProps {
        code: "keep the final newline\n".into(),
        ..Default::default()
    });
    let props = computed(move || source.get());
    view()
        .w_full()
        .h_full()
        .child(control("Empty code", move || {
            source.update(|p| p.code.clear())
        }))
        .child(markdown_code_block(
            props,
            CodeBlockState::new(),
            CodeBlockOptions::default(),
            move |_| {
                text(move || SharedString::from(props.with(|p| p.code.clone()))).into_element()
            },
        ))
}

#[test]
fn default_copy_shows_feedback_for_1200_ms_and_an_empty_block_copies_nothing() {
    for window in [DESKTOP, PHONE] {
        let mut app = Headless::start(EngineConfig {
            ui_sets: t3_ui::registries(),
            viewport: size(px(window.0), px(window.1)),
            clock: nsh_core::time::Clock::manual(SystemTime::UNIX_EPOCH),
            ..EngineConfig::new(Root::of::<NativeClipboardFixture>())
        })
        .expect("native clipboard fixture");
        app.wait_for("keep the final newline").expect("mounted");
        app.press("role:button[Copy code]")
            .expect("copy using the default NSH path");
        assert_eq!(app.screen().clipboard(), Some("keep the final newline\n"));
        assert!(
            app.screen()
                .matches("role:button[Copied]")
                .expect("component owns feedback")
        );
        app.run_for(Duration::from_millis(1199))
            .expect("before reset");
        assert!(
            app.screen()
                .matches("role:button[Copied]")
                .expect("feedback remains")
        );
        app.run_for(Duration::from_millis(1)).expect("1200ms reset");
        assert!(
            app.screen()
                .matches("role:button[Copy code]")
                .expect("feedback reset")
        );
        app.press("role:button[Empty code]").expect("empty source");
        app.set_clipboard(Some("sentinel".into()));
        app.press("role:button[Copy code]").expect("empty copy");
        assert_eq!(app.screen().clipboard(), Some("sentinel"));
        assert!(
            !app.screen()
                .matches("role:button[Copied]")
                .expect("copied state")
        );
    }
}

#[component]
fn MetadataFixture() -> impl IntoElement {
    let mut tokens = t3_ui::theme::tokens_for(&Default::default());
    tokens.set_color("foreground", AdaptiveColor::rgb(0x00ff00, 0x00ff00));
    tokens.set_color(
        "contrast-foreground",
        AdaptiveColor::rgb(0xff0000, 0xff0000),
    );
    tokens.set_color("code-foreground", AdaptiveColor::rgb(0x0000ff, 0x0000ff));
    set_tokens(tokens);
    set_color_scheme(Appearance::Light);
    t3_ui::i18n::set_language("fr", &[]);
    let source = signal(CodeBlockProps {
        code: "literal".into(),
        language: "Settings".into(),
        fence_title: Some("Settings".into()),
        ..Default::default()
    });
    let props = computed(move || source.get());
    view()
        .w_full()
        .h_full()
        .direction(LayoutDirection::Rtl)
        .child(text(t("Settings")))
        .child(control("Use themed host", move || {
            source.update(|p| p.themed = true)
        }))
        .child(control("Use language label", move || {
            source.update(|p| p.fence_title = None)
        }))
        .child(markdown_code_block(
            props,
            CodeBlockState::new(),
            CodeBlockOptions::default(),
            |_| view().into_element(),
        ))
}

#[test]
fn metadata_stays_literal_in_another_locale_and_header_color_follows_the_host() {
    for window in [DESKTOP, PHONE] {
        let mut app = Headless::start(EngineConfig {
            ui_sets: t3_ui::registries(),
            viewport: size(px(window.0), px(window.1)),
            ..EngineConfig::new(Root::of::<MetadataFixture>())
        })
        .expect("metadata fixture");
        app.wait_for("Settings").expect("literal fence title");
        app.wait_for("Paramètres").expect("French UI is active");
        assert!(app.screen().texts().contains(&"Settings"));
        assert_eq!(
            app.screen()
                .span_style("text:Settings")
                .expect("stock header color")
                .color,
            nsh_core::Color(0xff0000b8)
        );
        app.press("role:button[Use themed host]")
            .expect("theme host changes");
        assert_eq!(
            app.screen()
                .span_style("text:Settings")
                .expect("themed header color")
                .color,
            nsh_core::Color(0x0000ffb8)
        );
        app.press("role:button[Use language label]")
            .expect("language instead of title");
        assert!(
            app.screen().texts().contains(&"Settings"),
            "a language matching a translation key remains literal"
        );
    }
}
