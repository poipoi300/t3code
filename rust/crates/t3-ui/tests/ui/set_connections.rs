//! Saved environments through the mounted settings route and a paired runtime.
use crate::set_shell::{Remote, start_connected};
use crate::support::{DESKTOP, PHONE};
use t3_ui::state::nav::SettingsPage;

fn saved_environment(window: (f32, f32)) {
    let mut app = start_connected(window, SettingsPage::Connections, Remote::default());
    app.wait_for("role:button[Routes]")
        .expect("paired environment");
    app.press("role:button[Routes]")
        .expect("expand saved routes");
    app.wait_for("server.test").expect("the paired route");
    app.press("role:button[Permissions]")
        .expect("expand current grants");
    app.wait_for("These permissions apply to your active route. Other routes may have different permissions.").expect("live session");
    app.wait_for("Manage access").expect("access scope");
    app.wait_for("Not granted").expect("narrow session");
    app.press("role:switch[Switch off Fake host]")
        .expect("disable");
    app.wait_for("role:switch[Switch on Fake host]")
        .expect("saved while off");
    app.press("role:switch[Switch on Fake host]")
        .expect("enable");
    app.wait_for("role:switch[Switch off Fake host]")
        .expect("re-enabled");
}

#[test]
fn saved_routes_and_permissions_desktop() {
    saved_environment(DESKTOP);
}

#[test]
fn saved_routes_and_permissions_phone() {
    saved_environment(PHONE);
}

use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use std::rc::Rc;
use t3_client::{
    ClientStore,
    composition::{ClientRuntime, Spawner},
    platform::{NetworkStatus, PlatformEvent},
    testing::environment::FakeEnvironment,
};
use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;
use t3_ui::{
    runtime::{Rt, host::Choice},
    screens::settings::shell,
    state::nav::{Navigator, Route},
    theme,
};

#[derive(Clone, PartialEq, Props)]
struct ManagedProps {
    write: bool,
    refuse_unlink: bool,
    ssh_provisioning: bool,
}

#[component]
fn Managed(props: ManagedProps) -> impl IntoElement {
    theme::install(&theme::ThemeChoice::default());
    provide_context(Navigator::new(Route::Settings(SettingsPage::Connections)));
    let owner = Owner::current();
    let spawner: Spawner = Rc::new(move |task| {
        owner.run(|| {
            spawn(task);
        })
    });
    let server = FakeEnvironment::new("env", spawner.clone());
    server.set_permissions(if props.write {
        &["orchestration:read", "access:read", "access:write"][..]
    } else {
        &["orchestration:read", "access:read"][..]
    });
    server.edit_config(|config| {
        config["environment"]["capabilities"]["peerLinks"] = serde_json::json!(true);
        config["settings"]["linkedEnvironmentsEnabled"] = serde_json::json!(true);
    });
    server.register("peerLinks.link", t3_client::testing::CallKind::Unary);
    server.register("peerLinks.list", t3_client::testing::CallKind::Unary);
    server.register("peerLinks.unlink", t3_client::testing::CallKind::Unary);
    server.reply("peerLinks.list", Ok(serde_json::json!({"links":[{
        "environmentId":"peer","label":"Build host","urls":["https://build.test/path?token=secret-marker"],
        "access":"read-only","linkedAt":"2026-10-01T00:00:00Z","expiresAt":"2030-01-01T00:00:00Z",
        "lastReachedAt":null,"lastError":null,"status":"reachable"
    }]})));
    server.reply(
        "peerLinks.unlink",
        if props.refuse_unlink {
            Err(serde_json::json!({"_tag":"PeerLinkError","reason":"unknown_link","message":"private-rejection-marker"}))
        } else {
            Ok(serde_json::json!({"removed":true}))
        },
    );
    server.reply("peerLinks.list", Ok(serde_json::json!({"links":[]})));
    server.reply("peerLinks.link", Ok(serde_json::json!({
        "environmentId":"new-peer","label":"New host","urls":["https://new.test"],"access":"approval-required",
        "linkedAt":"2026-10-01T00:00:00Z","expiresAt":"2030-01-01T00:00:00Z","lastReachedAt":null,"lastError":null
    })));
    let observed = server.clone();
    let sent = signal(None::<String>);
    on_key("ctrl+shift+l", move |_| {
        let calls = observed.calls("peerLinks.link");
        sent.set(calls.last().map(|c| {
            format!(
                "Link request {} {} manual={} code={}",
                c["url"].as_str().unwrap_or_default(),
                c["access"].as_str().unwrap_or_default(),
                c.get("expectedEnvironmentId").is_none() && c.get("alternateUrls").is_none(),
                c["pairingCode"] == "fixture-only"
            )
        }));
    });
    let mut platform = server.platform();
    platform.ssh_provisioning = props.ssh_provisioning;
    let network = platform.fake.clone();
    on_key("ctrl+shift+o", move |_| {
        network.emit(PlatformEvent::Network(NetworkStatus::Offline))
    });
    let store = ClientStore::default();
    let client = ClientRuntime::new(
        platform,
        store.clone(),
        OrchestrationV2CreationSource::Web,
        spawner,
    );
    let short_code = std::rc::Rc::new(std::cell::Cell::new(false));
    let short = short_code.clone();
    on_key("ctrl+shift+x", move |_| short.set(true));
    let diagnostic_client = client.clone();
    on_key("ctrl+shift+d", move |_| {
        use t3_client::connection::{ConnectionFailure, FailureCause, FailureKind, Phase};
        let mut entry = diagnostic_client
            .environments()
            .into_iter()
            .next()
            .expect("paired machine");
        let mut failure =
            ConnectionFailure::new(FailureKind::ApprovalPending, "fixture-error-marker");
        failure.trace_id = Some("fixture-trace-marker".into());
        failure.verification_code = Some(
            if short_code.get() {
                "a"
            } else {
                "fixture-invalid-code"
            }
            .into(),
        );
        failure.hop = Some(t3_client::connection::ConnectionHop::Server);
        failure.cause = Some(FailureCause::ApprovalPending);
        entry.state.phase = Phase::Blocked;
        entry.state.last_failure = Some(failure);
        diagnostic_client
            .store()
            .set_connection(entry.id, entry.state);
    });
    let restarting = client.clone();
    on_key("ctrl+shift+r", move |_| {
        spawn(restarting.reconnect(&t3_client::types::EnvironmentId("env".into())));
    });
    provide_context(Rt::new(store, Rc::new(client.clone()), Choice::Client));
    spawn(client.pair(server.pairing()));
    view()
        .child(text(move || sent.get().unwrap_or_default()))
        .flex_1()
        .child(untracked(|| {
            shell::screen_for(SettingsPage::Connections, &client)
        }))
        .child(t3_ui::ui::confirm::confirm_dialog_host())
}

fn managed_app(window: (f32, f32)) -> Headless {
    managed_with_grant(window, true)
}

fn managed_with_grant(window: (f32, f32), write: bool) -> Headless {
    managed_fixture(window, write, false)
}
fn managed_fixture(window: (f32, f32), write: bool, refuse_unlink: bool) -> Headless {
    managed_fixture_on(window, write, refuse_unlink, Default::default(), false)
}

fn managed_fixture_on(
    window: (f32, f32),
    write: bool,
    refuse_unlink: bool,
    os: nsh_core::platform::Os,
    ssh_provisioning: bool,
) -> Headless {
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::new::<Managed>(move || ManagedProps {
            write,
            refuse_unlink,
            ssh_provisioning,
        }))
    };
    config.environment.idiom = if window == PHONE {
        nsh_core::platform::Idiom::Phone
    } else {
        nsh_core::platform::Idiom::Desktop
    };
    config.environment.os = os;
    Headless::start(config).expect("headless connections")
}

fn edit_routes(window: (f32, f32)) {
    let mut app = managed_app(window);
    app.wait_for("role:button[Routes]").expect("paired machine");
    app.press("role:button[Routes]").expect("show routes");
    app.press("role:button[Add route]").expect("route form");
    app.wait_for("Add a route to Fake host")
        .expect("bound to existing machine");
    app.type_text("label:Host", "https://alternate.test")
        .expect("alternate origin");
    app.type_text("label:Pairing code", "pairing-token")
        .expect("pairing credential");
    app.press("role:dialog[Add a route to Fake host] >> role:button[Add route]")
        .expect("pair second address");
    app.wait_for("role:button[2 routes]")
        .expect("same machine now has two routes");
    if app
        .screen()
        .count("role:button[Remove server.test route]")
        .expect("route controls")
        == 0
    {
        app.press("role:button[2 routes]")
            .expect("show both routes");
    }
    let first = app
        .screen()
        .count("role:button[Move server.test to position 2]")
        .expect("route order")
        == 1;
    let (change, after) = if first {
        (
            "role:button[Move server.test to position 2]",
            "role:button[Move server.test to position 1]",
        )
    } else {
        (
            "role:button[Move server.test to position 1]",
            "role:button[Move server.test to position 2]",
        )
    };
    app.press(change).expect("change route preference");
    app.wait_for(after)
        .expect("new preference in the live catalog");
    app.press("role:button[Remove server.test route]")
        .expect("confirm before removal");
    app.press("role:button[Cancel]").expect("cancel removal");
    app.wait_for("role:button[2 routes]")
        .expect("both preserved");
    app.press("role:button[Remove server.test route]")
        .expect("ask again");
    app.press("role:button[Confirm]")
        .expect("confirm selected route");
    app.wait_for("role:button[Routes]").expect("one route left");
}

fn diagnose(window: (f32, f32)) {
    let mut app = managed_app(window);
    app.wait_for("text:https://server.test · Connected")
        .expect("connected");
    app.press_key("ctrl+shift+o").expect("device goes offline");
    app.press_key("ctrl+shift+r")
        .expect("restart after connectivity was lost");
    app.wait_for("This device is offline")
        .expect("supervisor diagnosis");
    app.press("role:button[Details]").expect("open diagnosis");
    app.wait_for("Connect this device to the internet. T3 Code reconnects on its own.")
        .expect("offline remedy");
    app.wait_for("role:button[Retry now]")
        .expect("reconnect action");
}

#[test]
fn edit_saved_routes_desktop() {
    edit_routes(DESKTOP);
}
#[test]
fn edit_saved_routes_phone() {
    edit_routes(PHONE);
}
#[test]
fn diagnosis_desktop() {
    diagnose(DESKTOP);
}
#[test]
fn diagnosis_phone() {
    diagnose(PHONE);
}

fn forget_peer(window: (f32, f32)) {
    let mut app = managed_app(window);
    app.wait_for("Build host")
        .expect("linked host from guarded RPC");
    app.wait_for("text:https://build.test")
        .expect("show destination origin");
    assert!(
        app.screen()
            .texts()
            .iter()
            .all(|value| !value.contains("secret-marker"))
    );
    app.press("role:button[Forget]").expect("open confirmation");
    app.press("role:button[Cancel]").expect("keep link");
    app.wait_for("Build host").expect("still linked");
    app.press("role:button[Forget]").expect("confirm again");
    app.press("role:button[Confirm]").expect("unlink");
    app.wait_for("No linked environments")
        .expect("list refreshed after unlink");
}
#[test]
fn forget_peer_desktop() {
    forget_peer(DESKTOP);
}
#[test]
fn forget_peer_phone() {
    forget_peer(PHONE);
}

fn forget_environment(window: (f32, f32)) {
    let mut app = managed_app(window);
    app.wait_for("role:button[More actions for Fake host]")
        .expect("saved environment");
    app.press("role:button[More actions for Fake host]")
        .expect("actions menu");
    app.press("Remove from this device…")
        .expect("forget action");
    app.wait_for("This forgets its pairing, credentials, and cached threads here. Switch it off instead to keep it saved.").expect("consequences before removal");
    app.press("role:button[Cancel]").expect("keep pairing");
    app.press("role:button[More actions for Fake host]")
        .expect("saved machine remains");
    app.press("Remove from this device…")
        .expect("forget action");
    app.press("role:button[Confirm]")
        .expect("forget this device's pairing");
    app.wait_for("No saved remote environments")
        .expect("removed from catalog");
}
#[test]
fn forget_environment_desktop() {
    forget_environment(DESKTOP);
}
#[test]
fn forget_environment_phone() {
    forget_environment(PHONE);
}

#[component]
fn RelayFixture() -> impl IntoElement {
    theme::install(&theme::ThemeChoice::default());
    provide_context(Navigator::new(Route::Settings(SettingsPage::Connections)));
    let owner = Owner::current();
    let spawner: Spawner = Rc::new(move |task| {
        owner.run(|| {
            spawn(task);
        })
    });
    let server = FakeEnvironment::new("relay-env", spawner.clone());
    server.sign_in(true);
    let store = ClientStore::default();
    let client = ClientRuntime::with_relay(
        server.platform(),
        store.clone(),
        OrchestrationV2CreationSource::Web,
        spawner,
        FakeEnvironment::relay_config(),
    );
    provide_context(Rt::new(store, Rc::new(client.clone()), Choice::Client));
    spawn(client.restore());
    view().flex_1().child(untracked(|| {
        shell::screen_for(SettingsPage::Connections, &client)
    }))
}
fn relay(window: (f32, f32)) {
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<RelayFixture>())
    };
    config.environment.idiom = if window == PHONE {
        nsh_core::platform::Idiom::Phone
    } else {
        nsh_core::platform::Idiom::Desktop
    };
    let mut app = Headless::start(config).expect("headless relay settings");
    app.wait_for("role:button[Add]").expect("account discovery");
    app.press("role:button[Add]").expect("add the relay route");
    app.wait_for("T3 Connect · Connected")
        .expect("connected through real relay handshake");
    app.press("role:button[More actions for Fake host]")
        .expect("saved actions");
    app.press("Remove from this device…")
        .expect("account-aware removal dialog");
    app.wait_for("It stays on your T3 Connect account and keeps its host space. Deregister it in T3 Connect settings to free it.").expect("deregistration consequences");
    app.press("role:button[Cancel]")
        .expect("keep the registration");
    app.wait_for("T3 Connect · Connected")
        .expect("relay connection survives cancellation");
}
#[test]
fn relay_desktop() {
    relay(DESKTOP);
}
#[test]
fn relay_phone() {
    relay(PHONE);
}

fn link_peer(window: (f32, f32)) {
    let mut app = managed_app(window);
    app.wait_for("role:button[Link]")
        .expect("write-capable peer host");
    app.press("role:button[Link]").expect("link form");
    app.wait_for("Link an environment").expect("new link");
    app.type_text("label:Address", "https://new.test")
        .expect("enter address");
    app.wait_for("This app cannot confirm it is one of your machines. The pairing code you paste is sent there.").expect("warn before sharing a code");
    app.type_text("label:Pairing code", "fixture-only")
        .expect("enter user-supplied code");
    app.press("role:dialog[Link an environment] >> role:button[Link]")
        .expect("request supervised access");
    app.press_key("ctrl+shift+l").expect("inspect wire request");
    app.wait_for("text:Link request https://new.test approval-required manual=true code=true")
        .expect("typed address receives only the supplied code");
}
#[test]
fn link_peer_desktop() {
    link_peer(DESKTOP);
}
#[test]
fn link_peer_phone() {
    link_peer(PHONE);
}

fn read_only_host(window: (f32, f32)) {
    let mut app = managed_with_grant(window, false);
    app.wait_for("Build host")
        .expect("access:read loads existing peers");
    for action in ["role:button[Link]", "role:button[Forget]"] {
        assert_eq!(app.screen().count(action).expect("inspect peer actions"), 0);
    }
}
#[test]
fn read_only_host_desktop() {
    read_only_host(DESKTOP);
}
#[test]
fn read_only_host_phone() {
    read_only_host(PHONE);
}

fn route_menu(w: (f32, f32)) {
    let mut app = managed_app(w);
    app.wait_for("Build host")
        .expect("connected and list loaded");
    app.press("role:button[Routes]").expect("expand routes");
    app.press("role:button[More actions for Fake host]")
        .expect("open menu");
    app.press("role:menuitem[Hide routes]")
        .expect("menu reflects expanded routes");
    app.press("role:button[More actions for Fake host]")
        .expect("reopen");
    app.wait_for("role:menuitem[Routes]")
        .expect("menu reflects closed routes");
}
#[test]
fn route_menu_desktop() {
    route_menu(DESKTOP)
}
#[test]
fn route_menu_phone() {
    route_menu(PHONE)
}
fn refused_unlink(w: (f32, f32)) {
    let mut app = managed_fixture(w, true, true);
    app.wait_for("Build host").expect("connected");
    app.press("role:button[Forget]").expect("forget");
    app.press("role:button[Confirm]").expect("confirm");
    app.wait_for("This environment no longer has that link.")
        .expect("specific safe rejection");
    assert!(
        app.screen()
            .texts()
            .iter()
            .all(|value| !value.contains("private-rejection-marker"))
    );
}
#[test]
fn refused_unlink_desktop() {
    refused_unlink(DESKTOP)
}
#[test]
fn refused_unlink_phone() {
    refused_unlink(PHONE)
}

fn scrubbed_diagnosis(w: (f32, f32)) {
    let mut app = managed_app(w);
    app.wait_for("Build host").expect("paired machine");
    app.press_key("ctrl+shift+d")
        .expect("pending approval with diagnostic metadata");
    app.press("role:button[Details]")
        .expect("inspect diagnosis");
    app.wait_for("What to try").expect("diagnosis visible");
    app.press("role:button[Copy diagnostics]")
        .expect("copy visible report");
    let screen = app.screen();
    let copied = screen.clipboard().expect("copied");
    for marker in [
        "fixture-error-marker",
        "fixture-trace-marker",
        "fixture-invalid-code",
    ] {
        assert!(screen.texts().iter().all(|value| !value.contains(marker)));
        assert!(!copied.contains(marker), "excluded from clipboard");
    }
    assert!(screen.shows("Or run `t3 auth connect-device approve <code>` on Fake host."));
    assert!(copied.contains("Route: Direct"));
    app.press_key("ctrl+shift+x")
        .expect("select short comparison value");
    app.press_key("ctrl+shift+d")
        .expect("update failed connection");
    app.wait_for("Or run `t3 auth connect-device approve <code>` on Fake host.")
        .expect("redaction preserves ordinary words");
}
#[test]
fn scrubbed_diagnosis_desktop() {
    scrubbed_diagnosis(DESKTOP)
}
#[test]
fn scrubbed_diagnosis_phone() {
    scrubbed_diagnosis(PHONE)
}

#[test]
fn ssh_route_rejects_invalid_port_in_the_desktop_dialog() {
    let mut app = managed_fixture_on(DESKTOP, true, false, nsh_core::platform::Os::Linux, true);
    app.wait_for("role:button[Routes]").expect("paired machine");
    app.press("role:button[Routes]").expect("show routes");
    app.press("role:button[Add route]").expect("add route");
    app.press("role:button[SSH]").expect("use SSH");
    app.type_text("label:SSH host or alias", "builder@devbox")
        .expect("SSH host");
    app.type_text("label:Port", "65536").expect("invalid port");
    app.press("role:dialog[Add a route to Fake host] >> role:button[Add route]")
        .expect("submit target");
    app.wait_for("SSH port must be between 1 and 65535.")
        .expect("correct the port in place");
}

#[test]
fn phone_pairing_uses_the_remote_fields() {
    let mut app = managed_app(PHONE);
    app.wait_for("role:button[Routes]").expect("paired machine");
    app.press("role:button[Routes]").expect("show routes");
    app.press("role:button[Add route]").expect("add route");
    app.wait_for("label:Pairing code").expect("remote pairing");
    assert_eq!(app.screen().count("role:button[SSH]").unwrap(), 0);
}

#[test]
fn desktop_without_ssh_provisioning_offers_remote_pairing() {
    let mut app = managed_fixture_on(DESKTOP, true, false, nsh_core::platform::Os::Linux, false);
    app.wait_for("role:button[Routes]").expect("paired machine");
    app.press("role:button[Routes]").expect("show routes");
    app.press("role:button[Add route]").expect("add route");
    app.wait_for("label:Pairing code").expect("remote pairing");
    assert_eq!(app.screen().count("role:button[SSH]").unwrap(), 0);
}
