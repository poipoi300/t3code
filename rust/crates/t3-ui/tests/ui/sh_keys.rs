//! SH-KEYS: production config subscription and native binding replacement.

use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use serde_json::json;
use t3_client::ClientStore;
use t3_client::connection::{ConnectionState, Phase as ConnectionPhase};
use t3_client::types::EnvironmentId;
use t3_client::view_logic::shell::keybindings::compile_config;
use t3_contracts::server::ServerConfig;
use t3_ui::keys::bindings::{Keybindings, Phase, Scope, bindings_host, install_shell_commands};
use t3_ui::runtime::backend::EnvironmentEntry;
use t3_ui::state::nav::{Navigator, Route};
use t3_ui::state::panels::Panels;

use crate::support::{DESKTOP, PHONE};

fn config(environment: &EnvironmentId, key: &str, when: &str) -> ServerConfig {
    let mut config: ServerConfig = serde_json::from_str(
        include_str!("../../../../fixtures/samples/ServerConfig.jsonl")
            .lines()
            .next()
            .expect("config sample"),
    )
    .expect("canonical config");
    config.environment.environment_id = environment.clone();
    config.keybindings.0 = compile_config(&[serde_json::from_value(json!({
        "key": key, "command": "sidebar.toggle", "when": when,
    }))
    .expect("custom shortcut")]);
    config
}

#[component]
fn KeybindingsFixture() -> impl IntoElement {
    let store = ClientStore::default();
    let environment = EnvironmentId("a".into());
    let generation = store.begin_session(environment.clone());
    let catalogue = signal(vec![EnvironmentEntry {
        id: environment.clone(),
        label: "a".into(),
        state: ConnectionState {
            phase: ConnectionPhase::Connected,
            ..Default::default()
        },
    }]);
    let primary = signal(None);
    let keys = Keybindings::attach(store.clone(), catalogue, primary);
    let panels = Panels::new();
    let nav = Navigator::new(Route::Home);
    install_shell_commands(keys, panels, nav);
    let custom = environment.clone();
    let source = store.clone();
    let reset = environment.clone();
    let source_reset = store.clone();
    let scoped = signal(false);
    let count = signal(0);
    let custom_count = count;
    let editor = element_ref();
    keys.context.focus_within(editor, "editableFocus");
    view()
        .w_full()
        .h_full()
        .child(bindings_host(keys))
        .child(text(move || {
            SharedString::from(if panels.sidebar_open.get() {
                "sidebar open"
            } else {
                "sidebar closed"
            })
        }))
        .child(text(move || format!("newest {}", count.get())))
        .child(
            nsh_base::button()
                .test_id("remap")
                .on_press(move || {
                    assert!(source.set_config(
                        &custom,
                        generation,
                        config(&custom, "mod+y", "!editableFocus")
                    ));
                })
                .child(text(SharedString::from("remap"))),
        )
        .child(
            nsh_base::button()
                .test_id("restore")
                .on_press(move || {
                    assert!(source_reset.set_config(
                        &reset,
                        generation,
                        config(&reset, "mod+b", "true")
                    ));
                })
                .child(text(SharedString::from("restore"))),
        )
        .child(nsh_base::input().element_ref(&editor).test_id("editing"))
        .child(
            nsh_base::button()
                .test_id("primary-loading")
                .on_press(move || primary.set(Some(EnvironmentId("loading".into()))))
                .child(text(SharedString::from("primary"))),
        )
        .child(
            nsh_base::button()
                .test_id("remove-primary")
                .on_press(move || primary.set(None))
                .child(text(SharedString::from("remove primary"))),
        )
        .child(
            nsh_base::button()
                .test_id("new-scope")
                .on_press(move || scoped.update(|value| *value = !*value))
                .child(text(SharedString::from("scope"))),
        )
        .child(when(
            move || scoped.get(),
            move || {
                keys.register(
                    Scope {
                        phase: Phase::Capture,
                        ..Default::default()
                    }
                    .command("sidebar.toggle", move |event| {
                        event.claim();
                        custom_count.update(|count| *count += 1);
                        true
                    }),
                );
                view()
            },
        ))
}

fn start(window: (f32, f32)) -> Headless {
    let mut app = Headless::start(EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<KeybindingsFixture>())
    })
    .expect("keys engine");
    app.first_render().expect("bindings mounted");
    app
}

#[test]
fn config_edits_replace_defaults_and_editable_when_conditions_remove_native_bindings() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window);
        app.press_key("mod+b").expect("default shortcut");
        app.wait_for("sidebar closed").expect("default runs");
        app.press("id:remap").expect("server config arrives");
        app.press_key("mod+b").expect("old chord");
        assert!(app.screen().shows("sidebar closed"));
        app.press_key("mod+y").expect("new chord");
        app.wait_for("sidebar open")
            .expect("configured command runs");
        app.type_text("id:editing", "text").expect("editable focus");
        app.press_key("mod+y").expect("guarded chord");
        assert!(app.screen().shows("sidebar open"));
        app.press("id:restore")
            .expect("server removes editable guard");
        app.press_key("mod+b").expect("new config");
        app.wait_for("sidebar closed").expect("no stale guard");
    }
}

#[test]
fn primary_loading_stays_authoritative_and_scope_cleanup_restores_the_older_handler() {
    let mut app = start(DESKTOP);
    app.press("id:remap").expect("custom config");
    app.press("id:primary-loading").expect("primary selected");
    app.press_key("mod+y").expect("unrelated custom chord");
    assert!(app.screen().shows("sidebar open"));
    app.press_key("mod+b").expect("primary defaults");
    app.wait_for("sidebar closed")
        .expect("defaults while primary loads");
    app.press("id:remove-primary").expect("primary removed");
    app.press("id:new-scope").expect("mount newest scope");
    app.press_key("mod+y").expect("newest wins");
    app.wait_for("newest 1").expect("newest ran once");
    assert!(app.screen().shows("sidebar closed"));
    app.press("id:new-scope").expect("unmount newest scope");
    app.press_key("mod+y").expect("older handler restored");
    app.wait_for("sidebar open").expect("older scope runs");
    assert!(app.screen().shows("newest 1"));
}

#[test]
fn raw_event_adapter_preserves_physical_identity_repeats_and_releases() {
    let mut raw = nsh_core::KeyEvent::default();
    raw.key = "ф".into();
    raw.code = "KeyA".into();
    raw.repeat = true;
    raw.modifiers.cmd = true;
    let event = t3_ui::keys::bindings::client_event(&raw, false, true);
    assert_eq!(event.key, "ф");
    assert_eq!(event.code, "KeyA");
    assert_eq!(event.event_type.as_deref(), Some("keyup"));
    assert!(event.repeat && event.editable_focus && event.modifiers.meta);
    assert!(!event.modifiers.ctrl);
}
