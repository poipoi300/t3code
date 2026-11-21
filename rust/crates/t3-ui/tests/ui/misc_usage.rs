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
    let mut saved = serde_json::to_value(t3_contracts::settings::ServerSettings::default())
        .expect("serializable default settings");
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
    let changes = config_store.clone();
    on_key("ctrl+shift+r", move |_| {
        let snapshot = changes.snapshot();
        let (id, env) = snapshot
            .environments
            .first_key_value()
            .expect("paired environment");
        let mut config = env.config.as_deref().expect("configured").clone();
        config.environment.capabilities.usage_model_aliases =
            Some(config.environment.capabilities.usage_model_aliases != Some(true));
        assert!(changes.set_config(id, env.session, config));
        seen.set("Capabilities refreshed");
    });
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
        let (id, env) = snapshot
            .environments
            .first_key_value()
            .expect("paired machine");
        let mut config = env.config.as_deref().expect("loaded config").clone();
        config.settings = serde_json::from_value(saved.clone()).expect("valid saved settings");
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
    assert!(app.screen().shows("Via Accounts"));
    app.press_key("ctrl+shift+r")
        .expect("refresh unrelated settings");
    app.wait_for("Capabilities refreshed")
        .expect("config update delivered");
    assert!(
        app.screen().shows("14% time remaining"),
        "expanded account keeps its state"
    );
    assert!(app.screen().shows("+32% of pool"));
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
    app.press_key("ctrl+shift+r").expect("publish config");
    app.wait_for("Capabilities refreshed")
        .expect("refresh delivered");
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

#[component]
fn MixedFixture() -> impl IntoElement {
    theme::install(&theme::ThemeChoice::default());
    provide_context(Navigator::new(Route::Usage));
    let owner = Owner::current();
    let spawner: Spawner = Rc::new(move |f| {
        owner.run(|| {
            spawn(f);
        })
    });
    let first = FakeEnvironment::new("a", spawner.clone());
    let second = FakeEnvironment::at_origin("b", "https://other.test", spawner.clone());
    for (server, label, alias) in [(&first, "Home", true), (&second, "Work", false)] {
        server.set_permissions(&["orchestration:read", "diagnostics:read", "settings:write"]);
        server.edit_config(|c|{
            c["environment"]["label"]=json!(label);
            c["environment"]["capabilities"]["usageModelAliases"]=json!(alias);
            c["usageLimitSources"]=json!([{"id":"hub","kind":"cliproxy","label":"Hub","checkedAt":"2026-06-20T00:00:00Z","accounts":[]}]);
        });
        server.register("server.getUsageSummary", CallKind::Unary);
        for _ in 0..3 {
            server.reply(
                "server.getUsageSummary",
                Err(json!({"message":"internal diagnostic details"})),
            );
        }
        let mut saved = serde_json::to_value(t3_contracts::settings::ServerSettings::default())
            .expect("default settings");
        saved["usageModelAliases"] = json!({"short":"canonical"});
        server.reply("server.updateSettings", Ok(saved));
    }
    let client = ClientRuntime::new(
        first.platform().with_environment(&second),
        ClientStore::default(),
        OrchestrationV2CreationSource::Web,
        spawner.clone(),
    );
    let paired = client.clone();
    let a = first.pairing();
    let b = second.pairing();
    let ready = signal("");
    spawner(
        async move {
            paired.pair(a).await.expect("pair Home");
            paired.pair(b).await.expect("pair Work");
            ready.set("Both machines paired");
        }
        .boxed_local(),
    );
    let store = client.store().clone();
    let status = signal("");
    on_key("ctrl+shift+r", move |_| {
        let snap = store.snapshot();
        let (id, env) = snap
            .environments
            .iter()
            .find(|(id, _)| id.0 == "b")
            .expect("Work");
        let mut config = env.config.as_deref().expect("loaded Work").clone();
        config.environment.capabilities.usage_model_aliases = Some(true);
        assert!(store.set_config(id, env.session, config));
        status.set("Work upgraded");
    });
    on_key("ctrl+shift+i", move |_| {
        let expected = json!({"patch":{"usageModelAliases":{"short":"canonical"}}});
        for server in [&first, &second] {
            assert_eq!(
                server.calls("server.updateSettings").as_slice(),
                std::slice::from_ref(&expected)
            );
        }
        status.set("Both destinations saved once");
    });
    view()
        .flex_1()
        .child(usage::screen_for(&client))
        .child(text(move || status.get()))
        .child(text(move || ready.get()))
}
fn mixed(window: (f32, f32)) -> Headless {
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<MixedFixture>())
    };
    if window == DESKTOP {
        config.environment.os = nsh_core::platform::Os::Linux;
        config.environment.idiom = nsh_core::platform::Idiom::Desktop;
    }
    Headless::start(config).expect("headless mixed environment usage")
}
fn partially_saved(window: (f32, f32)) -> Headless {
    let mut app = mixed(window);
    app.wait_for("Both machines paired").expect("paired both");
    app.press("role:button[All environments]").expect("menu");
    app.press("Model prices").expect("prices");
    app.press("label:Add model price").expect("add price");
    app.type_text("label:Model ID", "short")
        .expect("enter model");
    app.type_text("label:Map to", "canonical")
        .expect("enter alias");
    app.press("role:button[Save]").expect("save");
    app.wait_for("role:button[Retry failed]")
        .expect("Work refused; can retry");
    app
}
#[test]
fn editing_after_a_partial_save_starts_a_new_attempt() {
    for window in [DESKTOP, PHONE] {
        let mut app = partially_saved(window);
        app.type_text("label:Map to", "-revised")
            .expect("edit after refusal");
        app.wait_for("role:button[Save]")
            .expect("new draft can be saved to all targets");
        assert_eq!(app.screen().count("role:button[Retry failed]"), Ok(0));
    }
}
#[test]
fn refused_destination_retries_independently() {
    for window in [DESKTOP, PHONE] {
        let mut app = partially_saved(window);
        app.press_key("ctrl+shift+r").expect("upgrade Work");
        app.wait_for("Work upgraded").expect("new settings");
        app.press("role:button[Retry failed]").expect("retry Work");
        app.wait_for("Fake host: Saved\nFake host: Saved")
            .expect("both statuses retained");
        app.press_key("ctrl+shift+i").expect("inspect writes");
        app.wait_for("Both destinations saved once")
            .expect("no duplicate writes");
    }
}
#[test]
fn failed_summaries_use_public_copy() {
    for window in [DESKTOP, PHONE] {
        let mut app = mixed(window);
        app.wait_for("Both machines paired")
            .expect("paired environments");
        if window == PHONE {
            app.press("label:Usage metric").expect("metric select");
        }
        app.press("Cost").expect("cost metric");
        app.wait_for("Fake host: This environment could not report usage.")
            .expect("public failure");
    }
}
