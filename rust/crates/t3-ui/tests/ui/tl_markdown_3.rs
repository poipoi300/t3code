//! TL-MARKDOWN-3: parsed rich blocks and their real controls at both reference sizes.

use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use std::rc::Rc;
use t3_ui::screens::thread::markdown::rich::{
    MarkdownImageState, MarkdownMediaProps, asset_media_props, chat_markdown_image,
    chat_markdown_video, markdown_details, markdown_mermaid_code_block, markdown_table_block,
};

use crate::support::{DESKTOP, PHONE};

const SOURCE: &str = "| Step | Tool | Result |\n| --- | :---: | ---: |\n| Read | `read` | 1 file |\n| Edit | `edit` | +4 lines |\n| Test | `execute` | 12 passed |";

#[component]
fn TableFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let source = signal(SOURCE.to_owned());
    let block = computed(move || Rc::<str>::from(source.get()));
    view()
        .w_full()
        .h_full()
        .child(
            nsh_base::button()
                .accessibility_label(t3_ui::i18n::t("Change table"))
                .on_press(move || source.set(SOURCE.replace("12 passed", "13 passed")))
                .child(text(t3_ui::i18n::t("Change table"))),
        )
        .child(markdown_table_block(block))
}

#[test]
fn table_copy_menu_uses_current_parsed_cells_and_both_formats_at_both_sizes() {
    for window in [DESKTOP, PHONE] {
        let mut app = Headless::start(EngineConfig {
            ui_sets: t3_ui::registries(),
            viewport: size(px(window.0), px(window.1)),
            clock: nsh_core::time::Clock::manual(std::time::SystemTime::UNIX_EPOCH),
            ..EngineConfig::new(Root::of::<TableFixture>())
        })
        .expect("table fixture");
        app.wait_for("12 passed").expect("table parsed and mounted");
        app.press("role:button[Copy table]")
            .expect("open table menu");
        app.press("role:menuitem[Copy as Markdown]")
            .expect("copy Markdown");
        assert_eq!(
            app.screen().clipboard(),
            Some(
                "| Step | Tool | Result |\n| --- | :---: | ---: |\n| Read | `read` | 1 file |\n| Edit | `edit` | +4 lines |\n| Test | `execute` | 12 passed |"
            )
        );
        app.press("role:button[Change table]")
            .expect("new tail block");
        app.wait_for("13 passed").expect("new cells on screen");
        app.press("role:button[Copied]")
            .expect("open menu after copying");
        app.press("role:menuitem[Copy as CSV]").expect("copy CSV");
        assert_eq!(
            app.screen().clipboard(),
            Some("Step,Tool,Result\nRead,read,1 file\nEdit,edit,+4 lines\nTest,execute,13 passed")
        );
    }
}

#[component]
fn DeniedVideoFixture() -> impl IntoElement {
    use futures::FutureExt;
    t3_ui::theme::install(&Default::default());
    let denied = t3_client::services::assets::AssetUrlState::Failure {
        missing_file_access: true,
    };
    let props = computed(move || {
        asset_media_props(
            &denied,
            None,
            "",
            MarkdownMediaProps {
                alt: "Demo".into(),
                ..Default::default()
            },
        )
    });
    view().w_full().h_full().child(chat_markdown_video(
        props,
        Some(Rc::new(|| async { Err("denied".into()) }.boxed_local())),
    ))
}

#[test]
fn denied_host_video_explains_file_access_and_has_no_retry_at_both_sizes() {
    for window in [DESKTOP, PHONE] {
        let mut app = Headless::start(EngineConfig {
            ui_sets: t3_ui::registries(),
            viewport: size(px(window.0), px(window.1)),
            ..EngineConfig::new(Root::of::<DeniedVideoFixture>())
        })
        .expect("denied video fixture");
        app.wait_for(&format!(
            "Video unavailable · Demo · {}",
            t3_client::services::assets::MISSING_FILE_ACCESS_MESSAGE
        ))
        .expect("permission explanation");
        assert!(
            app.screen()
                .texts()
                .iter()
                .any(|text| text.starts_with("Video unavailable"))
        );
        assert!(app.press("role:button[Retry video]").is_err());
        assert!(
            !app.screen()
                .matches("role:button[Retry video]")
                .expect("denied refresh control is absent")
        );
        assert!(!app.screen().shows("Loading video"));
    }
}

#[component]
fn RetryVideoFixture() -> impl IntoElement {
    use futures::{FutureExt, channel::oneshot};
    t3_ui::theme::install(&Default::default());
    let props = signal(MarkdownMediaProps {
        source_failed: true,
        ..Default::default()
    });
    let pending = stored(None::<oneshot::Sender<Result<(), String>>>);
    let dropped = signal(false);
    let retries = signal(0u32);
    let retry = Rc::new(move || {
        retries.update(|count| *count += 1);
        let (sender, receiver) = oneshot::channel();
        pending.replace(Some(sender));
        props.update(|props| props.source_failed = false);
        async move { receiver.await.unwrap_or_else(|_| Err("cancelled".into())) }.boxed_local()
    });
    view()
        .w_full()
        .h_full()
        .child(
            nsh_base::button()
                .accessibility_label(t3_ui::i18n::t("Fail source"))
                .on_press(move || props.update(|props| props.source_failed = true))
                .child(text(t3_ui::i18n::t("Fail source"))),
        )
        .child(
            nsh_base::button()
                .accessibility_label(t3_ui::i18n::t("Finish retry"))
                .on_press(move || {
                    if let Some(sender) = pending.replace(None) {
                        dropped.set(sender.send(Err("offline".into())).is_err());
                    }
                })
                .child(text(t3_ui::i18n::t("Finish retry"))),
        )
        .child(text(move || {
            format!("retries:{};dropped:{}", retries.get(), dropped.get())
        }))
        .child(chat_markdown_video(
            computed(move || props.get()),
            Some(retry),
        ))
}

#[test]
fn retry_survives_the_failure_to_loading_transition_and_a_failed_retry_can_repeat() {
    for window in [DESKTOP, PHONE] {
        let mut app = Headless::start(EngineConfig {
            ui_sets: t3_ui::registries(),
            viewport: size(px(window.0), px(window.1)),
            ..EngineConfig::new(Root::of::<RetryVideoFixture>())
        })
        .expect("retry fixture");
        app.wait_for("Video unavailable").expect("failed source");
        app.press("role:button[Retry video]")
            .expect("start signing");
        app.wait_for("role:group[Loading video]")
            .expect("resource transitions to loading");
        app.press("role:button[Fail source]")
            .expect("service failure arrives");
        app.press("role:button[Finish retry]")
            .expect("settle signing attempt");
        app.wait_for("text:retries:1;dropped:false")
            .expect("retry future remained alive across the visual branch change");
        app.press("role:button[Retry video]")
            .expect("a rejected retry did not stick in Retrying");
        app.wait_for("text:retries:2;dropped:false")
            .expect("another attempt starts");
    }
}

#[component]
fn ImageFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let source = signal(MarkdownMediaProps {
        src: Some("https://cdn.test/shot.png".into()),
        alt: "Shot".into(),
        ..Default::default()
    });
    view()
        .w_full()
        .h_full()
        .child(
            nsh_base::button()
                .accessibility_label(t3_ui::i18n::t("Change source"))
                .on_press(move || {
                    source.update(|p| p.src = Some("https://cdn.test/next.png".into()))
                })
                .child(text(t3_ui::i18n::t("Change source"))),
        )
        .child(chat_markdown_image(
            computed(move || source.get()),
            MarkdownImageState::default(),
            true,
        ))
}

#[test]
fn failed_images_show_the_web_fallback_and_a_new_source_recovers_the_loading_state() {
    for window in [DESKTOP, PHONE] {
        let mut app = Headless::start(EngineConfig {
            ui_sets: t3_ui::registries(),
            viewport: size(px(window.0), px(window.1)),
            ..EngineConfig::new(Root::of::<ImageFixture>())
        })
        .expect("image fixture");
        app.wait_for("role:group[Loading image]")
            .expect("static loading frame");
        app.fail_images("https://cdn.test/shot.png")
            .expect("actual image load failure");
        app.wait_for("Image unavailable · Shot")
            .expect("web error label and literal alt");
        app.press("role:button[Change source]")
            .expect("new media source");
        app.wait_for("role:group[Loading image]")
            .expect("old URL's failure does not poison the new URL");
        assert!(
            !app.screen()
                .texts()
                .iter()
                .any(|text| text.starts_with("Image unavailable"))
        );
    }
}

#[component]
fn DetailsFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let open = signal(false);
    let body =
        nsh_base::text_view::Document::new(vec![nsh_base::text_view::Block::paragraph(vec![
            nsh_base::text_view::Inline::text("Fold body"),
        ])]);
    view().w_full().h_full().child(markdown_details(
        open,
        vec![nsh_base::text_view::Inline::text("Authored summary")],
        body,
    ))
}

#[test]
fn details_open_and_close_their_own_panel_and_report_both_accessible_states() {
    for window in [DESKTOP, PHONE] {
        let mut config = EngineConfig {
            ui_sets: t3_ui::registries(),
            viewport: size(px(window.0), px(window.1)),
            ..EngineConfig::new(Root::of::<DetailsFixture>())
        };
        config.environment.reduce_motion = true;
        let mut app = Headless::start(config).expect("details fixture");
        app.wait_for("Authored summary").expect("summary");
        assert!(!app.screen().shows("Fold body"));
        app.press("role:button[Authored summary]").expect("expand");
        app.wait_for("Fold body").expect("body appears");
        assert_eq!(
            app.screen()
                .accessibility_state("role:button[Authored summary]")
                .expect("state")
                .expanded,
            Some(true)
        );
        app.press("role:button[Authored summary]").expect("close");
        assert!(!app.screen().shows("Fold body"));
        assert_eq!(
            app.screen()
                .accessibility_state("role:button[Authored summary]")
                .expect("state")
                .expanded,
            Some(false)
        );
    }
}

#[component]
fn MermaidFixture() -> impl IntoElement {
    use t3_ui::screens::thread::markdown::code_block::{CodeBlockProps, CodeBlockState};
    t3_ui::theme::install(&Default::default());
    let props = computed(|| CodeBlockProps {
        code: "flowchart LR\n  A --> B\n".into(),
        language: "mermaid".into(),
        ..Default::default()
    });
    view()
        .w_full()
        .h_full()
        .child(markdown_mermaid_code_block(props, CodeBlockState::new()))
}

#[test]
fn mermaid_source_fallback_copies_exact_code_without_a_dead_diagram_action() {
    for window in [DESKTOP, PHONE] {
        let mut app = Headless::start(EngineConfig {
            ui_sets: t3_ui::registries(),
            viewport: size(px(window.0), px(window.1)),
            ..EngineConfig::new(Root::of::<MermaidFixture>())
        })
        .expect("Mermaid source fixture");
        app.wait_for("flowchart LR\n  A --> B\n")
            .expect("source fallback");
        app.press("role:button[Copy code]")
            .expect("copy exact source through real code chrome");
        assert_eq!(app.screen().clipboard(), Some("flowchart LR\n  A --> B\n"));
        assert!(app.press("role:button[Show diagram]").is_err());
        assert!(app.press("role:button[Expand diagram]").is_err());
    }
}

#[derive(Clone, PartialEq, Props)]
struct RootProps {
    math: bool,
    source: String,
}

/// The shared Markdown root every caller uses (`markdown::base::chat_markdown`), with the client's
/// math setting as the timeline provides it.
#[component]
fn MarkdownRootFixture(props: RootProps) -> impl IntoElement {
    use t3_ui::screens::thread::markdown::base::{ChatMarkdownSettings, chat_markdown};
    t3_ui::theme::install(&Default::default());
    provide_context(t3_ui::theme::text_view_parts());
    let math = props.math;
    provide_context(ChatMarkdownSettings {
        math_enabled: computed(move || math),
    });
    let source = props.source.clone();
    view()
        .w_full()
        .h_full()
        .child(chat_markdown(move || source.clone(), || false))
}

fn root(math: bool, source: &'static str) -> Headless {
    let mut sets = t3_ui::registries();
    let mut fixtures = Registry::new();
    fixtures.register::<MarkdownRootFixture>();
    sets.push(fixtures);
    Headless::start(EngineConfig {
        ui_sets: sets,
        viewport: size(px(DESKTOP.0), px(DESKTOP.1)),
        ..EngineConfig::new(Root::new::<MarkdownRootFixture>(move || RootProps {
            math,
            source: source.to_owned(),
        }))
    })
    .expect("markdown root")
}

/// Math renders through MarkdownMath only with `mathRenderingEnabled`; off (the default), `$x^2$`
/// stays the text it is, as the web without remark-math.
#[test]
fn math_renders_only_when_the_setting_is_on() {
    let mut off = root(false, "Area $x^2$ here");
    off.wait_for(r"text:/\$x\^2\$/").expect("literal dollars");
    assert_eq!(off.screen().count("label:Inline equation"), Ok(0));
    let mut on = root(true, "Area $x^2$ here");
    on.wait_for("label:Inline equation").expect("MarkdownMath");
    assert!(!on.screen().shows(r"text:/\$x\^2\$/"));
}

/// A remote image is ChatMarkdownImage (labelled with its alt); a source the client cannot
/// resolve shows ChatMarkdownImage's unavailable fallback, not a broken image.
#[test]
fn images_render_through_the_chat_markdown_image() {
    let mut local = root(false, "Shot: ![logo](./docs/logo.png)");
    local
        .wait_for(r"text:/^Image unavailable/")
        .expect("the unavailable fallback, not a broken image");
    let mut remote = root(false, "Shot: ![logo](https://example.com/logo.png)");
    remote.wait_for("text:/^Shot:/").expect("rendered");
    assert!(
        !remote.screen().shows(r"text:/^Image unavailable/"),
        "a direct source is drawn, not refused"
    );
}

#[test]
fn default_markdown_requests_the_authored_remote_image() {
    let mut app = root(false, "![authored logo](https://example.com/authored.png)");
    app.fail_images("https://example.com/authored.png")
        .expect("fail the exact authored URL");
    app.wait_for("Image unavailable · authored logo")
        .expect("default renderer received its direct image error");
}
