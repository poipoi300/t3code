//! Mounted Maintenance settings over a paired environment.
use crate::support::{DESKTOP, PHONE};
use futures::FutureExt;
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use serde_json::json;
use std::rc::Rc;
use t3_client::{
    ClientStore,
    composition::{ClientRuntime, Spawner},
    testing::{CallKind, environment::FakeEnvironment},
};
use t3_ui::{
    runtime::{Rt, host::Choice},
    screens::settings::shell,
    state::nav::{Navigator, Route, SettingsPage},
    theme,
};

#[derive(Clone, PartialEq, Props)]
struct FixtureProps {
    maintain: bool,
    cleanup: bool,
    labs: bool,
    write: bool,
    run: bool,
    rules: bool,
}

#[derive(Clone, Copy, PartialEq, Default)]
enum Gate {
    #[default]
    Normal,
    NoRules,
    ReadOnly,
    RetentionOnly,
}

#[component]
fn Fixture(props: FixtureProps) -> impl IntoElement {
    theme::install(&theme::ThemeChoice::default());
    let page = if props.labs {
        SettingsPage::Labs
    } else {
        SettingsPage::Maintenance
    };
    provide_context(Navigator::new(Route::Settings(page)));
    let owner = Owner::current();
    let spawner: Spawner = Rc::new(move |future| {
        owner.run(|| {
            spawn(future);
        })
    });
    let server = FakeEnvironment::new("machine", spawner.clone());
    server.set_shell(t3_client::testing::environment::shell(&[]));
    let mut scopes = vec!["orchestration:read"];
    if props.write {
        scopes.push("settings:write");
    }
    if props.maintain {
        scopes.push("environment:maintain");
    }
    server.set_permissions(&scopes);
    let observed = server.clone();
    server.edit_config(|config| {
        config["environment"]["capabilities"]["databaseStorage"] = json!(true);
        config["environment"]["capabilities"]["storageCleanup"] = json!(props.cleanup);
        config["environment"]["capabilities"]["storageCleanupRun"] =
            json!(props.cleanup && props.run);
        config["settings"]["storageCleanup"]["logsAfterDays"] = json!(14);
        config["settings"]["storageCleanup"]["browserArtifactsAfterDays"] = json!(null);
        if !props.rules {
            let rules = &mut config["settings"]["storageCleanup"];
            for key in [
                "logsAfterDays",
                "worktreeAfterDays",
                "worktreeSettledAfterDays",
            ] {
                rules[key] = json!(null);
            }
            for key in ["worktreeOnDelete", "worktreeOnMerge", "worktreeUnchanged"] {
                rules[key] = json!(false);
            }
            config["settings"]["projectSettingsOverrides"] = json!({});
        }
    });
    server.register("server.getDatabaseStorage", CallKind::Stream);
    server.stream_snapshot(
        "server.getDatabaseStorage",
        vec![json!({
            "databaseBytes":8589934592u64, "reclaimableBytes":2147483648u64,
            "incrementalVacuumEnabled":false, "canReclaim":true,
            "compactCommand":"t3 doctor --reclaim-database-space"
        })],
    );
    server.register("server.reclaimDatabaseSpace", CallKind::Unary);
    server.reply("server.reclaimDatabaseSpace", Ok(json!(null)));
    server.register("server.getStorageCleanupReport", CallKind::Stream);
    server.stream_snapshot("server.getStorageCleanupReport", vec![json!(null)]);
    server.register("server.runStorageCleanup", CallKind::Unary);
    server.reply("server.runStorageCleanup", Ok(json!({
        "trigger":"manual","startedAt":"2026-11-21T12:00:00Z","finishedAt":"2026-11-21T12:01:00Z",
        "entries":[],"counts":{"removed":0,"kept":0,"failed":0},"omittedCount":0
    })));
    let store = ClientStore::default();
    let client = ClientRuntime::new(
        server.platform(),
        store.clone(),
        t3_contracts::orchestration_v2::OrchestrationV2CreationSource::Web,
        spawner.clone(),
    );
    let paired = signal(false);
    let pairing = client.pair(server.pairing());
    spawner(
        async move {
            pairing.await.expect("pair fixture");
            paired.set(true);
        }
        .boxed_local(),
    );
    provide_context(Rt::new(store, Rc::new(client.clone()), Choice::Client));
    let calls = signal(String::new());
    on_key("ctrl+shift+u", move |_| {
        calls.set(format!(
            "Reclaims {} cleanups {}",
            server.calls("server.reclaimDatabaseSpace").len(),
            server.calls("server.runStorageCleanup").len()
        ))
    });
    on_key("ctrl+shift+p", move |_| {
        let writes = observed.calls("server.updateSettings");
        if let Some(last) = writes.last() {
            let patch = &last["patch"];
            if patch
                == &json!({"backgroundActivity":{"profile":"performance","overrides":{},"schemaVersion":1}})
            {
                calls.set("Performance preset written".into());
            } else {
                calls.set(patch.to_string());
            }
        } else {
            calls.set("No settings writes".into());
        }
    });
    view()
        .flex_1()
        .child(text(move || calls.get()))
        .child(text(
            move || if paired.get() { "Paired fixture" } else { "" },
        ))
        .child(untracked(|| shell::screen_for(page, &client)))
}

fn start(window: (f32, f32), maintain: bool, cleanup: bool) -> Headless {
    start_page(window, maintain, cleanup, false)
}
pub(super) fn start_page(
    window: (f32, f32),
    maintain: bool,
    cleanup: bool,
    labs: bool,
) -> Headless {
    start_gate(window, maintain, cleanup, labs, Gate::Normal)
}
fn start_gate(
    window: (f32, f32),
    maintain: bool,
    cleanup: bool,
    labs: bool,
    gate: Gate,
) -> Headless {
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::new::<Fixture>(move || FixtureProps {
            maintain,
            cleanup,
            labs,
            write: gate != Gate::ReadOnly,
            run: gate != Gate::RetentionOnly,
            rules: gate != Gate::NoRules,
        }))
    };
    config.environment.idiom = if window == PHONE {
        nsh_core::platform::Idiom::Phone
    } else {
        nsh_core::platform::Idiom::Desktop
    };
    let mut app = Headless::start(config).expect("mounted settings route");
    app.wait_for("Paired fixture")
        .expect("writable fixture is paired");
    app
}

fn reclaim(window: (f32, f32)) {
    let mut app = start(window, true, true);
    app.wait_for("role:button[Reclaim]")
        .expect("database recommends reclaim");
    app.press("role:button[Reclaim]").expect("open warning");
    app.wait_for("Restart and reclaim").expect("confirmation");
    app.press("role:button[Cancel]").expect("cancel restart");
    app.press_key("ctrl+shift+u").expect("observe environment");
    app.wait_for("Reclaims 0 cleanups 0")
        .expect("cancel sends nothing");
    app.press("role:button[Reclaim]").expect("reopen warning");
    app.press("role:button[Restart and reclaim]")
        .expect("confirm restart");
    app.wait_for("Restarting…").expect("pending restart");
    app.press_key("ctrl+shift+u").expect("observe environment");
    app.wait_for("Reclaims 1 cleanups 0").expect("one reclaim");
    app.press("role:button[Delete now]")
        .expect("manual cleanup");
    app.settle();
    app.press_key("ctrl+shift+u").expect("observe cleanup");
    app.wait_for("Reclaims 1 cleanups 1").expect("one cleanup");
}
#[test]
fn desktop_reclaim_confirmation_and_cleanup() {
    reclaim(DESKTOP);
}
#[test]
fn phone_reclaim_confirmation_and_cleanup() {
    reclaim(PHONE);
}
#[test]
fn restricted_connection_offers_offline_command_and_hides_unsupported_cleanup() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window, false, false);
        app.wait_for("role:button[Copy command]")
            .expect("manual repair for a restricted session");
        assert!(!app.screen().shows("role:button[Reclaim]"));
        assert!(!app.screen().shows("role:button[Delete now]"));
    }
}

#[test]
fn retention_requires_a_committed_first_age_and_background_presets_write_scoped_settings() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window, true, true);
        app.wait_for("role:button[Reclaim]")
            .expect("paired environment");
        app.press("role:switch[Delete old browser artifacts]")
            .expect("start retention draft");
        app.press("role:button[Increase Delete old browser artifacts]")
            .expect("adjust draft");
        app.press_key("ctrl+shift+p")
            .expect("observe before commit");
        app.wait_for("No settings writes")
            .expect("enabling and stepping must not trigger a sweep");
        app.type_text("label:Delete old browser artifacts in days", "")
            .expect("focus age");
        app.press_key("enter").expect("commit first age");
        app.settle();
        app.press_key("ctrl+shift+p").expect("observe age");
        app.wait_for(r#"{"storageCleanup":{"browserArtifactsAfterDays":9}}"#)
            .expect("only the chosen age");
        app.press("role:combobox[Background activity profile]")
            .expect("open profile list");
        app.press("role:option[Performance]")
            .expect("select profile");
        app.settle();
        app.press_key("ctrl+shift+p").expect("observe profile");
        app.wait_for("Performance preset written")
            .expect("profile patch");
    }
}

#[test]
fn project_scope_excludes_storage_actions_and_reports() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window, true, true);
        app.wait_for("role:button[Reclaim]").unwrap();
        app.press("role:button[Project scope: All projects]")
            .unwrap();
        app.press("role:menuitemradio[Project]").unwrap();
        app.wait_for_gone("role:button[Reclaim]", std::time::Duration::from_secs(5))
            .unwrap();
        assert!(!app.screen().shows("role:button[Delete now]"));
        assert!(!app.screen().shows("Delete old rotated logs"));
        app.press_key("ctrl+shift+u").unwrap();
        app.wait_for("Reclaims 0 cleanups 0").unwrap();
        app.press("role:button[Project scope: Project]").unwrap();
        app.press("role:menuitemradio[All projects]").unwrap();
    }
}
#[test]
fn manual_cleanup_requires_run_capability_rules_and_write_access() {
    for window in [DESKTOP, PHONE] {
        for gate in [Gate::NoRules, Gate::ReadOnly, Gate::RetentionOnly] {
            let mut app = start_gate(window, true, true, false, gate);
            app.wait_for("role:button[Reclaim]").unwrap();
            let query = "role:button[Delete now]";
            if gate == Gate::RetentionOnly {
                assert!(!app.screen().shows(query));
            } else {
                assert!(
                    !app.screen()
                        .enabled(query)
                        .expect("disabled cleanup action")
                );
            }
            app.press_key("ctrl+shift+u").unwrap();
            app.wait_for("Reclaims 0 cleanups 0").unwrap();
        }
    }
}
