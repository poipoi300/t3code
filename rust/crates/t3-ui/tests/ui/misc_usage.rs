//! Usage through the real paired client, with reports arriving over its socket.
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
use t3_ui::{
    screens::usage,
    state::nav::{Navigator, Route},
    theme,
};

#[component]
fn UsageFixture() -> impl IntoElement {
    theme::install(&theme::ThemeChoice::default());
    provide_context(Navigator::new(Route::Usage));
    let owner = Owner::current();
    let spawner: Spawner = Rc::new(move |f| {
        owner.run(|| {
            spawn(f);
        })
    });
    let server = FakeEnvironment::new("machine", spawner.clone());
    server.platform().fake.advance(1_781_913_600_000);
    server.set_permissions(&["orchestration:read", "diagnostics:read", "settings:write"]);
    server.edit_config(|config| {
        config["usageLimitSources"]=json!([{"id":"hub","kind":"cliproxy","label":"Accounts","checkedAt":"2026-06-20T00:00:00Z","accounts":[{"id":"personal","driver":"codex","usageLimits":{"checkedAt":"2026-06-20T00:00:00Z","windows":[{"id":"week","kind":"weekly","label":"Weekly plan","usedPercent":32,"resetsAt":"2026-06-21T00:00:00Z","windowDurationMins":10080}]}}]}]);
    });
    server.register("server.getUsageSummary", CallKind::Unary);
    for _ in 0..3 {
        server.reply("server.getUsageSummary",Ok(json!({
            "contractVersion":6,"readAt":"2026-06-20T00:00:00Z","timeZone":"UTC","sinceDay":"2026-05-22","untilDay":"2026-06-20",
            "sources":[{"fingerprint":{"hostId":"host","provider":"codex","resolvedHomePath":"/transcripts","volumeId":"disk"},"status":"ok","scannedFiles":1,"skippedFiles":0,"malformedRecords":0,"distinctSessions":1,"message":null}],
            "buckets":[{"provider":"codex","day":"2026-06-20","model":"fixture-model","costUsd":3.5,"cacheSavingsUsd":0.25,"costSource":"modelPriced","records":1,"unpricedRecords":0,"sessions":1,"totals":{"uncachedInputTokens":50,"cachedInputTokens":30,"cacheCreationTokens":0,"outputTokens":20,"reasoningTokens":10}}],
            "pricing":{"status":"fresh","source":"fixture","knownModels":1,"fetchedAt":null},"scanDurationMs":1
        })));
    }
    let mut saved =
        serde_json::to_value(t3_contracts::settings::ServerSettings::default()).unwrap();
    saved["usagePriceOverrides"] =
        json!({"local-model":{"inputCostPerMillionTokens":0.5,"outputCostPerMillionTokens":2}});
    server.reply("server.updateSettings", Ok(saved.clone()));
    server.register("server.refreshProviders", CallKind::Unary);
    for _ in 0..2 {
        server.reply("server.refreshProviders", Ok(json!({"providers":[]})));
    }
    let client = ClientRuntime::new(
        server.platform(),
        ClientStore::default(),
        OrchestrationV2CreationSource::Web,
        spawner.clone(),
    );
    let config_store = client.store().clone();
    let seen = signal("");
    let limits_wire = server.clone();
    on_key("ctrl+shift+l", move |_| {
        assert_eq!(
            limits_wire.calls("server.refreshProviders"),
            [json!({}), json!({})]
        );
        seen.set("Limits refreshed twice");
    });
    let wire = server.clone();
    on_key("ctrl+shift+i", move |_| {
        assert_eq!(
            wire.calls("server.updateSettings"),
            [
                json!({"patch":{"usagePriceOverrides":{"local-model":{"inputCostPerMillionTokens":0.5,"outputCostPerMillionTokens":2}}}})
            ]
        );
        // Deliver the settings notification independently of the mutation reply, as the server does.
        let snapshot = config_store.snapshot();
        let (id, env) = snapshot.environments.first_key_value().unwrap();
        let mut config = env.config.as_deref().unwrap().clone();
        config.settings = serde_json::from_value(saved.clone()).unwrap();
        assert!(config_store.set_config(id, env.session, config));
        seen.set("Price saved to machine");
    });
    let pairing = client.pair(server.pairing());
    spawner(
        async move {
            pairing.await.expect("pair usage server");
        }
        .boxed_local(),
    );
    view()
        .flex_1()
        .child(usage::screen_for(&client))
        .child(text(move || seen.get()))
}

fn start(window: (f32, f32)) -> Headless {
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<UsageFixture>())
    };
    if window == DESKTOP {
        config.environment.os = nsh_core::platform::Os::Linux;
        config.environment.idiom = nsh_core::platform::Idiom::Desktop;
    }
    Headless::start(config).expect("headless usage")
}
fn browse(window: (f32, f32)) {
    let mut app = start(window);
    app.wait_for("Weekly plan").expect("paired provider limit");
    assert!(app.screen().shows("68%"));
    app.press("role:button[personal]")
        .expect("inspect account limit");
    app.wait_for("resets in 1d 0h")
        .expect("reset relative to the client clock");
    assert!(app.screen().shows("14% time remaining"));
    app.press("label:Refresh limits").expect("refresh limits");
    app.press_key("ctrl+shift+l")
        .expect("check limit refreshes");
    app.wait_for("Limits refreshed twice")
        .expect("automatic and manual refresh");
    if window == PHONE {
        app.press("label:Usage metric")
            .expect("compact metric picker");
    }
    app.press("Cost").expect("cost metric");
    app.wait_for("fixture-model").expect("server summary");
    assert!(app.screen().shows("$3.50"));
    if window == DESKTOP {
        app.hover("label:Daily cost").expect("inspect chart");
        app.wait_for("Codex · $0.00")
            .expect("chart point breakdown");
    }
    app.press("fixture-model").expect("model detail");
    app.wait_for("Cache hit").expect("model stats");
    assert!(app.screen().shows("100"));
    assert!(app.screen().shows("Tokens by type"));
}
#[test]
fn desktop_reads_limits_usage_and_model_detail() {
    browse(DESKTOP);
}
#[test]
fn phone_reads_limits_usage_and_model_detail() {
    browse(PHONE);
}

fn edit_price(window: (f32, f32)) {
    let mut app = start(window);
    app.wait_for("Weekly plan").expect("connected");
    app.press("role:button[All environments]")
        .expect("scope menu");
    app.press("Model prices").expect("open price editor");
    app.wait_for("Custom model prices").expect("price dialog");
    app.press("label:Add model price").expect("new price");
    app.type_text("label:Model ID", "local-model")
        .expect("model id");
    app.type_text("label:Input", "0.5").expect("input rate");
    app.type_text("label:Output", "2").expect("output rate");
    app.press("role:button[Save]").expect("save price");
    app.wait_for("Fake host: Saved")
        .expect("confirmed settings write");
    app.press_key("ctrl+shift+i")
        .expect("check server mutation");
    app.wait_for("Price saved to machine").expect("exact patch");
    app.wait_for("role:button[local-model]")
        .expect("saved model appears in the price list");
}
#[test]
fn desktop_edits_model_prices() {
    edit_price(DESKTOP);
}
#[test]
fn phone_edits_model_prices() {
    edit_price(PHONE);
}
