//! TL-BLOCKS-2: real tokenization and chunk rows under TL-MARKDOWN-1's chrome.

use std::cell::RefCell;
use std::rc::Rc;

use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use t3_ui::i18n::{Text, t};
use t3_ui::screens::thread::blocks::code::{
    CodeHighlightState, HighlightStatus, code_chunk_row, highlighted_chunk,
};
use t3_ui::screens::thread::blocks::flatten::{BlockKind, MessageFlattener};
use t3_ui::screens::thread::markdown::code_block::{
    CodeBlockOptions, CodeBlockProps, CodeBlockState, markdown_code_block,
};

use crate::support::{DESKTOP, PHONE};

fn first_line() -> String {
    format!("export const stable = \"{}\";", "café 😀 ".repeat(40))
}

#[component]
fn CodeChunkFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    set_color_scheme(Appearance::Light);
    let source = signal(format!("{}\nlet pending", first_line()));
    let streaming = signal(true);
    let language = signal("typescript".to_owned());
    let flatten = Rc::new(RefCell::new(MessageFlattener::new("fixture")));
    let chunk = computed(move || {
        let blocks = flatten.borrow_mut().replace(
            "fixture",
            &format!("```{}\n{}\n```", language.get(), source.get()),
        );
        blocks.rows.iter().find_map(|block| match &block.kind {
            BlockKind::Code(chunk) => Some(chunk.clone()),
            _ => None,
        })
    });
    let syntax_state = CodeHighlightState::new();
    let highlighted = highlighted_chunk(
        chunk,
        computed(move || streaming.get()),
        syntax_state.clone(),
    );
    let props = computed(move || CodeBlockProps {
        code: source.get(),
        language: language.get(),
        is_streaming: streaming.get(),
        default_wrapped: false,
        ..Default::default()
    });
    let state = CodeBlockState::new();
    view().w_full().h_full().child(
        view()
            .app_class(t3_ui::theme::tw("p-3 gap-2"))
            .child(control("Append code", move || {
                source.update(|s| s.push_str(" = 2;"))
            }))
            .child(control("Finish stream", move || streaming.set(false)))
            .child(control("Edit first line", move || {
                source.update(|s| *s = s.replacen("stable", "edited", 1))
            }))
            .child(control("Empty code", move || source.set(String::new())))
            .child(control("Unknown language", move || {
                syntax_state.clear();
                language.set("unknown-language-23912".into());
            }))
            .child(control("Dark theme", || set_color_scheme(Appearance::Dark)))
            .child(control("Custom light theme", || {
                t3_ui::theme::install(&t3_ui::theme::ThemeChoice {
                    theme: "t3-chat".into(),
                    mode: t3_theme::palette::AppearanceMode::Light,
                    ..Default::default()
                });
            }))
            .child(markdown_code_block(
                props,
                state,
                CodeBlockOptions::default(),
                move |wrapped| code_chunk_row(chunk, highlighted, wrapped).into_element(),
            ))
            .child(text(move || {
                SharedString::from(
                    if highlighted.with(|h| {
                        h.as_ref()
                            .is_some_and(|h| h.status == HighlightStatus::Unavailable)
                    }) {
                        t("Loading fixture syntax")
                    } else {
                        t("Highlighting ready")
                    },
                )
            })),
    )
}

fn control(label: &'static str, action: impl Fn() + 'static) -> impl IntoElement {
    let label = Text::from(label).resolve();
    nsh_base::button()
        .accessibility_label(Some(label.clone()))
        .on_press(action)
        .child(text(label))
}

fn start(window: (f32, f32)) -> Headless {
    let mut app = Headless::start(EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<CodeChunkFixture>())
    })
    .expect("chunk fixture");
    app.wait_for("let pending").expect("chunk painted");
    app.wait_for("Highlighting ready")
        .expect("real syntax loaded");
    app
}

#[test]
fn streaming_and_finish_preserve_the_completed_line_node_at_both_sizes() {
    for size in [PHONE, DESKTOP] {
        let mut app = start(size);
        let selector = format!("text:{}", first_line());
        let completed = app.screen().id(&selector).expect("completed line node");
        app.press("role:button[Append code]").expect("append");
        app.wait_for("let pending = 2;").expect("tail updated");
        assert_eq!(
            app.screen().id(&selector).expect("retained line"),
            completed
        );
        app.press("role:button[Finish stream]").expect("finish");
        app.wait_for("Highlighting ready")
            .expect("finished content");
        assert_eq!(
            app.screen().id(&selector).expect("finished line"),
            completed
        );
        app.press("role:button[Edit first line]").expect("edit");
        app.wait_for(&first_line().replacen("stable", "edited", 1))
            .expect("edited source shown");
        assert!(!app.screen().shows(&first_line()));
        app.press("role:button[Empty code]").expect("truncate");
        app.wait_for("Highlighting ready").expect("empty resolved");
        assert!(!app.screen().shows("let pending"));
    }
}

#[test]
fn syntax_palette_changes_and_unknown_language_keeps_plain_source() {
    for size in [PHONE, DESKTOP] {
        let mut app = start(size);
        let light = app
            .screen()
            .span_style("text:export")
            .expect("keyword style")
            .color;
        app.press("role:button[Dark theme]")
            .expect("appearance change");
        app.wait_for("Highlighting ready").expect("still ready");
        let dark = app
            .screen()
            .span_style("text:export")
            .expect("recolored keyword")
            .color;
        assert_ne!(light, dark);
        app.press("role:button[Custom light theme]")
            .expect("custom light theme");
        app.wait_for("Highlighting ready")
            .expect("custom theme syntax resolved");
        assert_eq!(
            light,
            app.screen()
                .span_style("text:export")
                .expect("light syntax under custom theme")
                .color
        );
        app.press("role:button[Unknown language]")
            .expect("unsupported grammar");
        app.wait_for("Highlighting ready")
            .expect("plain fallback resolved");
        assert!(app.screen().shows(&first_line()));
        assert!(app.screen().shows("let pending"));
    }
}

#[test]
fn wrap_control_reflows_the_actual_rows_without_adding_a_scroll_container() {
    for size in [PHONE, DESKTOP] {
        let mut app = start(size);
        let selector = format!("text:{}", first_line());
        let node = app.screen().id(&selector).expect("line");
        let line_height = |app: &Headless| {
            app.screen()
                .layout_nodes()
                .into_iter()
                .find(|n| n.id == node)
                .expect("line layout")
                .bounds
                .size
                .height
                .0
        };
        let unwrapped = line_height(&app);
        app.press("role:button[Wrap lines]").expect("enable wrap");
        app.wait_for("Disable line wrap")
            .expect("wrap action updated");
        assert_eq!(app.screen().id(&selector).expect("same line"), node);
        assert!(
            line_height(&app) > unwrapped,
            "long source wraps in the real text row"
        );
        assert!(
            app.screen()
                .layout_nodes()
                .iter()
                .all(|node| node.scroll.is_none())
        );
        app.press("role:button[Disable line wrap]")
            .expect("disable wrap");
        app.wait_for("Wrap lines").expect("reverse state");
        assert_eq!(line_height(&app), unwrapped);
    }
}
