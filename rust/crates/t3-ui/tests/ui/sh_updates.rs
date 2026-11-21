//! Provider updates through the real client and the footer entry.
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
use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;
#[component]
fn UpdateFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let owner = Owner::current();
    let spawn_client: Spawner = Rc::new(move |f| {
        owner.run(|| {
            spawn(f);
        })
    });
    let server = FakeEnvironment::new("machine", spawn_client.clone());
    server.set_permissions(&["orchestration:read", "providers:manage"]);
    let provider = json!({"instanceId":"codex","driver":"codex","enabled":true,"installed":true,"version":"1.0.0","status":"ready","auth":{"status":"authenticated"},"checkedAt":"2026-06-20T00:00:00Z","models":[],"skills":[],"slashCommands":[],"versionAdvisory":{"status":"behind_latest","currentVersion":"1.0.0","latestVersion":"2.0.0","updateCommand":"managed update","canUpdate":true,"checkedAt":null,"message":null}});
    server.edit_config(|c| {
        c["environment"]["serverVersion"] = json!("1.0.0");
        let mut alternate = provider.clone();
        alternate["instanceId"] = json!("codex-another");
        c["providers"] = json!([alternate, provider]);
    });
    server.register("server.refreshProviders", CallKind::Unary);
    server.reply(
        "server.refreshProviders",
        Ok(json!({"providers":[provider.clone()]})),
    );
    server.register("server.updateProvider", CallKind::Unary);
    let mut settled = provider;
    settled["installedVersion"] = json!("2.0.0");
    settled["versionAdvisory"]["status"] = json!("current");
    settled["updateState"] = json!({"status":"succeeded","startedAt":null,"finishedAt":null,"message":"Restart the server","output":null});
    server.reply("server.updateProvider", Ok(json!({"providers":[settled]})));
    let client = ClientRuntime::new(
        server.platform(),
        ClientStore::default(),
        OrchestrationV2CreationSource::Web,
        spawn_client.clone(),
    );
    let pairing = client.pair(server.pairing());
    spawn_client(
        async move {
            pairing.await.expect("pair update environment");
        }
        .boxed_local(),
    );
    let received = signal("");
    let store = client.store().clone();
    on_key("ctrl+shift+r", move |_| {
        let snapshot = store.snapshot();
        let (id, env) = snapshot
            .environments
            .first_key_value()
            .expect("paired machine");
        let mut config = env.config.as_deref().expect("machine config").clone();
        config.environment.capabilities.usage_model_aliases =
            Some(config.environment.capabilities.usage_model_aliases != Some(true));
        assert!(store.set_config(id, env.session, config));
        received.set("Config refreshed");
    });
    let inspected = stored(0);
    let wire = server.clone();
    on_key("ctrl+shift+n", move |_| {
        assert!(
            wire.calls("server.updateProvider").is_empty(),
            "installation requires confirmation"
        );
        inspected.replace(inspected.get() + 1);
        received.set(if inspected.get() == 1 {
            "No install before confirmation"
        } else {
            "No install after Cancel"
        });
    });
    on_key("ctrl+shift+i", move |_| {
        assert_eq!(inspected.get(), 2, "both pre-install wire inspections ran");
        assert_eq!(
            server.calls("server.updateProvider"),
            [json!({"provider":"codex","instanceId":"codex"})]
        );
        assert_eq!(
            server.calls("server.refreshProviders"),
            [json!({"refreshModels":true})]
        );
        for blocked in [
            "server.updateServer",
            "server.updateServerWithProgress",
            "server.commitDesktopUpdate",
        ] {
            assert!(server.calls(blocked).is_empty());
        }
        received.set("One provider updated");
    });
    view()
        .flex_1()
        .child(t3_ui::screens::updates::footer_for(&client, Some("1.1.0")))
        .child(text(move || received.get()))
}
fn run(window: (f32, f32)) {
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<UpdateFixture>())
    };
    if window == DESKTOP {
        config.environment.idiom = nsh_core::platform::Idiom::Desktop;
        config.environment.os = nsh_core::platform::Os::Linux;
    }
    let mut app = Headless::start(config).expect("headless updates");
    app.wait_for("Update all")
        .expect("provider update discovered");
    assert!(app.screen().shows("Fake host · 1.0.0 → 1.1.0"));
    let notice = app
        .screen()
        .id("Fake host · 1.0.0 → 1.1.0")
        .expect("version notice");
    app.press_key("ctrl+shift+r")
        .expect("change unrelated setting");
    app.wait_for("Config refreshed").expect("config delivered");
    assert_eq!(app.screen().id("Fake host · 1.0.0 → 1.1.0"), Ok(notice));
    app.press("role:button[Check for updates]")
        .expect("refresh provider advisories");
    app.press("role:button[Update all]")
        .expect("review updates");
    app.wait_for("Update 1 machine?").expect("confirmation");
    assert!(app.screen().shows("1 provider update"));
    app.press_key("ctrl+shift+n")
        .expect("inspect wire before confirmation");
    assert_eq!(app.screen().count("role:alertdialog"), Ok(1));
    if window == PHONE {
        app.press("role:alertdialog >> role:button[Cancel]")
            .expect("tap Cancel");
    } else {
        app.press_key("enter")
            .expect("activate initially focused Cancel");
    }
    app.wait_for_gone("role:alertdialog", std::time::Duration::from_secs(2))
        .expect("cancel closed confirmation");
    app.press_key("ctrl+shift+n")
        .expect("inspect wire after Cancel");
    app.wait_for("No install after Cancel")
        .expect("both inspections delivered");
    app.press("role:button[Update all]").expect("review again");
    app.press("role:button[Update]").expect("confirm");
    app.wait_for("Provider updated")
        .expect("authoritative result");
    app.wait_for("Fake host · Codex: Restart the server")
        .expect("scoped restart instruction");
    app.press_key("ctrl+shift+i").expect("check wire calls");
    app.wait_for("One provider updated")
        .expect("deduplicated driver");
}
#[test]
fn desktop_confirms_and_updates_provider() {
    run(DESKTOP)
}
#[test]
fn phone_confirms_and_updates_provider() {
    run(PHONE)
}
