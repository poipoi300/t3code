//! Source control settings over a paired client and scripted forge RPCs.

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

#[component]
fn SourceControlFixture() -> impl IntoElement {
    theme::install(&theme::ThemeChoice::default());
    provide_context(Navigator::new(Route::Settings(SettingsPage::SourceControl)));
    let owner = Owner::current();
    let spawner: Spawner = Rc::new(move |future| {
        owner.run(|| {
            spawn(future);
        });
    });
    let server = FakeEnvironment::new("forge", spawner.clone());
    server.set_permissions(&["orchestration:read", "settings:write"]);
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
            "kind":"github", "label":"GitHub", "executable":"gh", "status":"available",
            "version":empty, "installHint":"Install gh.", "detail":empty, "appSignIn":true,
            "auth":{"status":"unauthenticated","account":empty,"host":empty,"detail":empty}
        }]
    });
    for _ in 0..3 {
        server.reply("server.discoverSourceControl", Ok(discovery.clone()));
    }
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
        server.platform(),
        store.clone(),
        OrchestrationV2CreationSource::Web,
        spawner.clone(),
    );
    let pairing = client.pair(server.pairing());
    spawner(
        async move {
            pairing.await.expect("pair the forge settings fixture");
        }
        .boxed_local(),
    );
    let result = signal("");
    on_key("ctrl+shift+u", move |_| {
        let _keep_reply_pending = &pending;
        let access = server.calls("sourceControl.appAccess");
        if access == [json!({"provider":"github","host":"github.com","repository":"team/project"})]
        {
            result.set("App access request verified");
        }
        let calls = server.calls("server.updateSettings");
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
        .child(text(move || result.get()))
        .child(untracked(|| {
            shell::screen_for(SettingsPage::SourceControl, &client)
        }))
}

fn start(window: (f32, f32)) -> Headless {
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<SourceControlFixture>())
    };
    if window == DESKTOP {
        config.environment.os = nsh_core::platform::Os::Linux;
        config.environment.idiom = nsh_core::platform::Idiom::Desktop;
    }
    let mut app = Headless::start(config).expect("headless source control settings");
    app.wait_for("role:button[Toggle GitHub details]")
        .expect("the paired forge discovery result");
    app.press("role:button[Toggle GitHub details]")
        .expect("expand GitHub settings");
    app
}

fn replace_token(window: (f32, f32)) {
    let mut app = start(window);
    app.wait_for("label:GitHub token")
        .expect("the masked replacement field");
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
fn app_access_uses_the_selected_project_repository() {
    let mut app = start(DESKTOP);
    app.press("role:button[/^Project scope:/]")
        .expect("pick a project scope");
    app.press("role:menuitemradio[Project]")
        .expect("select the repository's project");
    app.wait_for("role:button[Toggle GitHub details]")
        .expect("load the selected environment");
    app.press("role:button[Toggle GitHub details]")
        .expect("show GitHub details");
    app.wait_for("The GitHub app does not have access to this repository.")
        .expect("show app access status");
    app.press_key("ctrl+shift+u")
        .expect("read the app access request");
    app.wait_for("App access request verified")
        .expect("request used the scoped project's repository");
}
