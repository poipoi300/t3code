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
    database: bool,
    mixed: bool,
    peer_cleanup: bool,
    relay_and_fast_checkout: bool,
}

#[derive(Clone, Copy, PartialEq, Default)]
pub(super) enum Gate {
    #[default]
    Normal,
    NoRules,
    ReadOnly,
    RetentionOnly,
    NoDatabase,
    MixedRetention,
    MixedUnsupported,
    RelayAndFastCheckout,
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
        config["environment"]["capabilities"]["databaseStorage"] = json!(props.database);
        config["environment"]["capabilities"]["fastWorktreeCheckout"] =
            json!(props.relay_and_fast_checkout);
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
    let database = server.stream_updates("server.getDatabaseStorage");
    let status = |bytes: u64, direct: bool| {
        json!({
            "databaseBytes":8589934592u64, "reclaimableBytes":bytes,
            "incrementalVacuumEnabled":false, "canReclaim":direct,
            "compactCommand":"t3 doctor --reclaim-database-space"
        })
    };
    database
        .unbounded_send(status(2 << 30, true))
        .expect("initial database status");
    for (key, bytes, direct) in [
        ("ctrl+shift+d", 3 << 30, true),
        ("ctrl+shift+h", 0, true),
        ("ctrl+shift+r", 3 << 30, false),
    ] {
        let database = database.clone();
        on_key(key, move |_| {
            database
                .unbounded_send(status(bytes, direct))
                .expect("refresh database status");
        });
    }
    server.register("server.reclaimDatabaseSpace", CallKind::Unary);
    server.reply("server.reclaimDatabaseSpace", Ok(json!(null)));
    server.register("server.getStorageCleanupReport", CallKind::Stream);
    server.stream_snapshot("server.getStorageCleanupReport", vec![json!(null)]);
    server.register("server.runStorageCleanup", CallKind::Unary);
    server.reply("server.runStorageCleanup", Ok(json!({
        "trigger":"manual","startedAt":"2026-11-21T12:00:00Z","finishedAt":"2026-11-21T12:01:00Z",
        "entries":[],"counts":{"removed":0,"kept":0,"failed":0},"omittedCount":0
    })));
    let other = FakeEnvironment::at_origin("other", "https://other.test", spawner.clone());
    let mut platform = server.platform();
    if props.mixed {
        other.set_shell(t3_client::testing::environment::shell(&[]));
        other.set_permissions(&scopes);
        other.edit_config(|config| {
            let caps = &mut config["environment"]["capabilities"];
            caps["databaseStorage"] = json!(false);
            caps["storageCleanup"] = json!(props.peer_cleanup);
            caps["storageCleanupRun"] = json!(false);
        });
        for (tag, kind) in [
            ("server.getDatabaseStorage", CallKind::Stream),
            ("server.getStorageCleanupReport", CallKind::Stream),
            ("server.runStorageCleanup", CallKind::Unary),
        ] {
            other.register(tag, kind);
        }
        platform = platform.with_environment(&other);
    }
    let store = ClientStore::default();
    let source = t3_contracts::orchestration_v2::OrchestrationV2CreationSource::Web;
    let client = if props.relay_and_fast_checkout {
        server.sign_in(false);
        ClientRuntime::with_relay(
            platform,
            store.clone(),
            source,
            spawner.clone(),
            FakeEnvironment::relay_config(),
        )
    } else {
        ClientRuntime::new(platform, store.clone(), source, spawner.clone())
    };
    let paired = signal(false);
    let pairing_client = client.clone();
    let first = server.pairing();
    let second = props.mixed.then(|| other.pairing());
    spawner(
        async move {
            pairing_client.pair(first).await.expect("pair fixture");
            if let Some(second) = second {
                pairing_client
                    .pair(second)
                    .await
                    .expect("pair other fixture");
            }
            paired.set(true);
        }
        .boxed_local(),
    );
    provide_context(Rt::new(store, Rc::new(client.clone()), Choice::Client));
    let calls = signal(String::new());
    let prefs = client.settings();
    let subscriptions = server.clone();
    on_key("ctrl+shift+s", move |_| {
        let subscriptions_for = |peer: &FakeEnvironment| {
            format!(
                "database={} report={} runs={}",
                !peer.calls("server.getDatabaseStorage").is_empty(),
                !peer.calls("server.getStorageCleanupReport").is_empty(),
                peer.calls("server.runStorageCleanup").len()
            )
        };
        calls.set(format!(
            "first {} other {}",
            subscriptions_for(&subscriptions),
            subscriptions_for(&other)
        ));
    });
    on_key("ctrl+shift+b", move |_| {
        let local = prefs.client_settings();
        calls.set(format!(
            "Local sidebar={} multiline={}",
            local.boring_sidebar_enabled, local.boring_sidebar_multiline_rows
        ));
    });
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
        .child(when(
            move || paired.get(),
            move || untracked(|| shell::screen_for(page, &client)),
        ))
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
pub(super) fn start_gate(
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
            database: gate != Gate::NoDatabase,
            mixed: matches!(gate, Gate::MixedRetention | Gate::MixedUnsupported),
            peer_cleanup: gate == Gate::MixedRetention,
            relay_and_fast_checkout: gate == Gate::RelayAndFastCheckout,
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

pub(super) fn has_control(app: &Headless, selector: &str) -> bool {
    app.screen()
        .matches(selector)
        .expect("valid control selector")
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
fn status_update_preserves_the_open_reclaim_confirmation() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window, true, true);
        app.wait_for("role:button[Reclaim]")
            .expect("initial storage status");
        app.press("role:button[Reclaim]")
            .expect("open confirmation");
        app.wait_for("role:button[Restart and reclaim]")
            .expect("confirmation");
        app.press_key("ctrl+shift+d").expect("update status");
        app.wait_for("3.0 GiB on Fake host")
            .expect("updated storage row");
        assert!(has_control(&app, "role:button[Restart and reclaim]"));
        app.press("role:button[Cancel]")
            .expect("cancel updated confirmation");
        app.press_key("ctrl+shift+u").expect("read calls");
        app.wait_for("Reclaims 0 cleanups 0")
            .expect("no reclaim on update");
    }
}
#[test]
fn reclaim_eligibility_returning_requires_a_new_confirmation() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window, true, true);
        for key in ["ctrl+shift+h", "ctrl+shift+r"] {
            app.wait_for("role:button[Reclaim]")
                .expect("reclaim available");
            app.press("role:button[Reclaim]").expect("open warning");
            app.wait_for("role:button[Restart and reclaim]")
                .expect("warning visible");
            app.press_key(key).expect("withdraw direct reclaim");
            app.wait_for_gone("role:button[Reclaim]", std::time::Duration::from_secs(5))
                .expect("reclaim unavailable");
            app.press_key("ctrl+shift+d")
                .expect("restore direct reclaim");
            app.wait_for("role:button[Reclaim]")
                .expect("reclaim available again");
            assert!(!has_control(&app, "role:button[Restart and reclaim]"));
        }
    }
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
fn maintenance_permission_and_storage_capabilities_are_independent() {
    for window in [DESKTOP, PHONE] {
        let mut restricted = start(window, false, true);
        restricted
            .wait_for("role:button[Copy command]")
            .expect("offline command");
        assert!(!has_control(&restricted, "role:button[Reclaim]"));
        assert!(has_control(&restricted, "role:button[Delete now]"));
        observe_storage(
            &mut restricted,
            "first database=true report=true runs=0 other database=false report=false runs=0",
        );

        let mut no_database = start_gate(window, true, true, false, Gate::NoDatabase);
        no_database
            .wait_for("role:button[Delete now]")
            .expect("cleanup advertised");
        assert!(!has_control(&no_database, "role:button[Reclaim]"));
        assert!(!has_control(&no_database, "role:button[Copy command]"));
        observe_storage(
            &mut no_database,
            "first database=false report=true runs=0 other database=false report=false runs=0",
        );

        let mut no_cleanup = start(window, true, false);
        no_cleanup
            .wait_for("role:button[Reclaim]")
            .expect("database advertised");
        assert!(!has_control(&no_cleanup, "role:button[Delete now]"));
        observe_storage(
            &mut no_cleanup,
            "first database=true report=false runs=0 other database=false report=false runs=0",
        );
    }
}

fn observe_storage(app: &mut Headless, expected: &str) {
    app.settle();
    app.press_key("ctrl+shift+s")
        .expect("observe storage requests");
    app.wait_for(expected)
        .expect("per-environment storage requests");
}

#[test]
fn mixed_environment_capabilities_limit_subscriptions_and_cleanup_targets() {
    for window in [DESKTOP, PHONE] {
        let mut app = start_gate(window, true, true, false, Gate::MixedRetention);
        app.wait_for("role:button[Reclaim]")
            .expect("first database notice");
        app.press("role:button[Delete now]")
            .expect("run only on supported environment");
        observe_storage(
            &mut app,
            "first database=true report=true runs=1 other database=false report=false runs=0",
        );

        let mut app = start_gate(window, true, true, false, Gate::MixedUnsupported);
        app.wait_for("role:button[Reclaim]")
            .expect("first database notice");
        assert!(!has_control(&app, "role:button[Delete now]"));
        observe_storage(
            &mut app,
            "first database=true report=false runs=0 other database=false report=false runs=0",
        );
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
        assert!(!has_control(&app, "role:button[Delete now]"));
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
                assert!(!has_control(&app, query));
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
