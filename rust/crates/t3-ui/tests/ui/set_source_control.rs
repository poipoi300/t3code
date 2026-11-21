//! Source control settings over a paired client and scripted forge RPCs.

use crate::support::{DESKTOP, PHONE};
use futures::FutureExt;
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use serde_json::json;
use std::{
    collections::BTreeMap,
    rc::Rc,
    sync::{Arc, Mutex},
};
use t3_client::{
    ClientStore,
    composition::{ClientRuntime, Spawner},
    testing::{
        CallKind,
        environment::{FakeEnvironment, shell as fixture_shell},
    },
};
use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;
use t3_ui::{
    runtime::{Rt, host::Choice},
    screens::settings::shell,
    state::nav::{Navigator, Route, SettingsPage},
    theme,
};

const FIXTURE_TOKEN: &str = "fixture-only-credential";

#[derive(Clone, PartialEq, Props)]
struct SourceControlFixtureProps {
    read_only: bool,
    multiple: bool,
    missing_cli: bool,
}

#[component]
fn SourceControlFixture(props: SourceControlFixtureProps) -> impl IntoElement {
    theme::install(&theme::ThemeChoice::default());
    provide_context(Navigator::new(Route::Settings(SettingsPage::SourceControl)));
    let owner = Owner::current();
    let spawner: Spawner = Rc::new(move |future| {
        owner.run(|| {
            spawn(future);
        });
    });
    let server = FakeEnvironment::new("forge", spawner.clone());
    server.set_permissions(if props.read_only {
        &["orchestration:read"]
    } else {
        &["orchestration:read", "settings:write"]
    });
    let mut snapshot = fixture_shell(&[("thread", "Thread")]);
    snapshot["projects"][0]["repositoryIdentity"] = json!({
        "canonicalKey":"github.com/team/project",
        "locator":{"source":"git-remote","remoteName":"origin","remoteUrl":"https://github.com/team/project.git"},
        "displayName":"Project","name":"project"
    });
    server.set_shell(snapshot);
    server.register("sourceControl.appAccess", CallKind::Unary);
    server.reply(
        "sourceControl.appAccess",
        Ok(json!({"status":"not-covered"})),
    );
    server.edit_config(|config| {
        config["settings"]["sourceControlHosts"]["github"] = json!({
            "tokens": {"github.com":"••••••","code.example.test":"••••••"}
        });
    });
    let mut saved = serde_json::to_value(t3_contracts::settings::ServerSettings::default())
        .expect("saved settings fixture");
    saved["sourceControlHosts"]["github"] = json!({
        "tokens": {"github.com":"••••••","code.example.test":"••••••"}
    });
    server.reply("server.updateSettings", Ok(saved));
    server.register("server.discoverSourceControl", CallKind::Unary);
    let empty = json!({"_tag":"None"});
    let discovery = json!({
        "versionControlSystems": [],
        "sourceControlProviders": [{
            "kind":"github", "label":"GitHub", "executable":"gh", "status":if props.missing_cli {"missing"} else {"available"},
            "version":empty, "installHint":"Install gh.", "detail":empty, "appSignIn":true,
            "auth":{"status":"unauthenticated","account":empty,"host":empty,"detail":empty}
        }]
    });
    for _ in 0..3 {
        server.reply("server.discoverSourceControl", Ok(discovery.clone()));
    }
    let other = FakeEnvironment::at_origin("other", "https://other.test", spawner.clone());
    other.set_permissions(&["orchestration:read", "settings:write"]);
    other.register("server.discoverSourceControl", CallKind::Unary);
    let mut other_discovery = discovery;
    other_discovery["sourceControlProviders"][0]["label"] = json!("Other GitHub");
    other.reply("server.discoverSourceControl", Ok(other_discovery));
    for method in [
        "sourceControl.signInStart",
        "sourceControl.signInWait",
        "sourceControl.signInCancel",
    ] {
        server.register(method, CallKind::Unary);
    }
    server.reply("sourceControl.signInStart", Ok(json!({
        "userCode":"ABCD-EFGH","verificationUri":"https://github.com/login/device","expiresAt":1800000000000_u64
    })));
    let pending = server.defer_reply("sourceControl.signInWait");
    server.reply("sourceControl.signInCancel", Ok(json!(null)));

    let store = ClientStore::default();
    let client = ClientRuntime::new(
        server.platform().with_environment(&other),
        store.clone(),
        OrchestrationV2CreationSource::Web,
        spawner.clone(),
    );
    let first = server.pairing();
    let second = props.multiple.then(|| other.pairing());
    let pairing_client = client.clone();
    let ready = signal(false);
    spawner(
        async move {
            pairing_client
                .pair(first)
                .await
                .expect("pair the forge settings fixture");
            if let Some(second) = second {
                pairing_client
                    .pair(second)
                    .await
                    .expect("pair the second machine");
            }
            ready.set(true);
        }
        .boxed_local(),
    );
    let result = signal("");
    on_key("ctrl+shift+u", move |_| {
        let _keep_reply_pending = &pending;
        let access = server.calls("sourceControl.appAccess");
        let calls = server.calls("server.updateSettings");
        let other_calls = other.calls("server.updateSettings");
        if props.read_only {
            assert!(calls.is_empty());
            assert!(server.calls("sourceControl.signInStart").is_empty());
            result.set("No source control write sent");
        } else if props.multiple {
            let expected = |calls: &[serde_json::Value]| {
                calls.len() == 1
                    && calls[0]["patch"]["offeredSourceControlProviders"]["github"] == false
            };
            if expected(&calls) && other_calls.is_empty() {
                result.set("Offer saved on GitHub machine only");
            } else if expected(&other_calls) && calls.is_empty() {
                result.set("Offer saved on Other GitHub machine only");
            } else {
                result.set("Offer was not scoped to one machine");
            }
        } else if access.is_empty() {
            result.set("No app access request sent");
        }
        if calls.iter().any(|call| {
            call["patch"]["sourceControlWritingStyle"]["mode"] == "conventional_commits"
        }) {
            result.set("Writing mode verified");
        }
        if calls.iter().any(|call| {
            let tokens = &call["patch"]["sourceControlHosts"]["github"]["tokens"];
            tokens["github.com"] == FIXTURE_TOKEN && tokens["code.example.test"] == "••••••"
        }) {
            result.set("Token request verified");
        }
        let cancel = server.calls("sourceControl.signInCancel");
        if cancel == [json!({"provider":"github","host":"github.com"})]
            && server.calls("sourceControl.signInWait") == cancel
            && server.calls("sourceControl.signInStart") == cancel
        {
            result.set("Sign-in cancellation verified");
        }
    });
    provide_context(Rt::new(store, Rc::new(client.clone()), Choice::Client));
    view()
        .flex_1()
        .child(when(move || ready.get(), || text("Fixture paired")))
        .child(text(move || result.get()))
        .child(untracked(|| {
            shell::screen_for(SettingsPage::SourceControl, &client)
        }))
}

fn start(window: (f32, f32)) -> Headless {
    let mut app = start_fixture(window, false, false);
    expand_github(&mut app);
    app
}

fn start_fixture(window: (f32, f32), read_only: bool, multiple: bool) -> Headless {
    start_configured_fixture(
        window,
        SourceControlFixtureProps {
            read_only,
            multiple,
            missing_cli: false,
        },
    )
}

fn start_configured_fixture(window: (f32, f32), props: SourceControlFixtureProps) -> Headless {
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::new::<SourceControlFixture>(move || props.clone()))
    };
    if window == DESKTOP {
        config.environment.os = nsh_core::platform::Os::Linux;
        config.environment.idiom = nsh_core::platform::Idiom::Desktop;
    }
    let mut app = Headless::start(config).expect("headless source control settings");
    app.wait_for("Fixture paired")
        .expect("all fixture environments ready");
    app
}

fn expand_github(app: &mut Headless) {
    app.wait_for("role:button[Toggle GitHub details]")
        .expect("the paired forge discovery result");
    app.press("role:button[Toggle GitHub details]")
        .expect("expand GitHub settings");
}

fn replace_token(window: (f32, f32)) {
    use nsh_core::MountItem;

    let mut app = start_fixture(window, false, false);
    let input_configs = Arc::new(Mutex::new(BTreeMap::new()));
    let observed = input_configs.clone();
    app.observe(move |_, batch| {
        for item in &batch.items {
            if let MountItem::Input { id, config } = item {
                observed
                    .lock()
                    .expect("input configs")
                    .insert(*id, config.secure);
            }
        }
    });
    expand_github(&mut app);
    app.wait_for("label:GitHub token")
        .expect("the masked replacement field");
    let id = app.screen().id("label:GitHub token").expect("token input");
    assert_eq!(
        input_configs.lock().expect("input configs").get(&id),
        Some(&true)
    );
    app.type_text("label:GitHub token", FIXTURE_TOKEN)
        .expect("enter a replacement");
    assert!(!app.screen().texts().contains(&FIXTURE_TOKEN));
    app.press("role:button[Save]")
        .expect("save the replacement");
    app.settle();
    app.press_key("ctrl+shift+u")
        .expect("check the credential write");
    app.wait_for("Token request verified")
        .expect("replacement and sibling marker reached the environment");
}

#[test]
fn replaces_the_selected_host_token_on_desktop() {
    replace_token(DESKTOP);
}

#[test]
fn replaces_the_selected_host_token_on_phone() {
    replace_token(PHONE);
}

#[test]
fn cancels_an_in_progress_forge_sign_in() {
    let mut app = start(DESKTOP);
    app.press("role:button[Sign in with GitHub]")
        .expect("start sign-in");
    app.wait_for("ABCD-EFGH").expect("the server's device code");
    app.press("role:button[Cancel]").expect("cancel sign-in");
    app.settle();
    app.press_key("ctrl+shift+u")
        .expect("check the forge requests");
    app.wait_for("Sign-in cancellation verified")
        .expect("all operations targeted the same forge");
}

#[test]
fn writes_repository_writing_mode_to_the_selected_environment() {
    let mut app = start(DESKTOP);
    app.scroll("id:settings-page-content", px(1250.0))
        .expect("reach writing settings");
    app.press("role:combobox[Commit and pull request writing]")
        .expect("open writing styles");
    app.press("role:option[Conventional Commits]")
        .expect("choose Conventional Commits");
    app.settle();
    app.press_key("ctrl+shift+u").expect("read fixture writes");
    app.wait_for("Writing mode verified")
        .expect("the selected environment received the writing mode");
}

#[test]
fn app_access_stays_on_the_pull_request_page() {
    let mut app = start(DESKTOP);
    app.press("role:button[/^Project scope:/]")
        .expect("pick a project scope");
    app.press("role:menuitemradio[Project]")
        .expect("select the repository's project");
    app.wait_for("role:button[Toggle GitHub details]")
        .expect("load the selected environment");
    app.press("role:button[Toggle GitHub details]")
        .expect("show GitHub details");
    app.settle();
    app.press_key("ctrl+shift+u")
        .expect("read the app access request");
    app.wait_for("No app access request sent")
        .expect("settings leaves the app access notice to pull request details");
}

#[test]
fn offer_updates_only_the_displayed_environment() {
    let mut app = start_fixture(DESKTOP, false, true);
    app.wait_for("role:switch[/^Offer .*GitHub when/]")
        .expect("discovered forge");
    let label = if app.screen().shows("Other GitHub") {
        "Other GitHub"
    } else {
        "GitHub"
    };
    app.press(&format!(
        "role:switch[Offer {label} when adding or publishing a project]"
    ))
    .expect("toggle offer");
    app.settle();
    app.press_key("ctrl+shift+u")
        .expect("read both environments' writes");
    app.wait_for(&format!("Offer saved on {label} machine only"))
        .expect("only the displayed machine was updated");
}

#[test]
fn read_only_forge_settings_cannot_send_writes() {
    let mut app = start_fixture(PHONE, true, false);
    expand_github(&mut app);
    for control in [
        "label:GitHub token",
        "role:button[Save]",
        "role:button[Sign in with GitHub]",
        "role:switch[/^Offer GitHub when/]",
    ] {
        assert!(
            !app.screen()
                .enabled(control)
                .expect("read-only control is mounted"),
            "{control} is enabled"
        );
        let _ = app.press(control);
    }
    let _ = app.type_text("label:GitHub token", FIXTURE_TOKEN);
    app.press_key("ctrl+shift+u").expect("check read-only RPCs");
    app.wait_for("No source control write sent")
        .expect("no writes reached the server");
}

#[test]
fn missing_cli_still_allows_tokens_but_hides_cli_accounts() {
    let mut app = start_configured_fixture(
        DESKTOP,
        SourceControlFixtureProps {
            read_only: false,
            multiple: false,
            missing_cli: true,
        },
    );
    expand_github(&mut app);
    app.wait_for("label:GitHub token")
        .expect("personal tokens are still supported");
    assert!(!app.screen().shows(
        "Sign in with gh auth login on the server host, then rescan to choose accounts here."
    ));
}
