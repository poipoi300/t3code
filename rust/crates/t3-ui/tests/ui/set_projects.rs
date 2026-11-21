//! Project settings through the production settings shell and a paired client.

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
    testing::environment::{FakeEnvironment, shell},
};
use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;
use t3_ui::{
    runtime::{Rt, host::Choice},
    screens::settings::shell as settings_shell,
    state::nav::{Navigator, Route, SettingsPage},
    theme,
};

#[derive(Clone, PartialEq, Props)]
struct ProjectsFixtureProps {
    imports_script: bool,
}

#[component]
fn ProjectsFixture(props: ProjectsFixtureProps) -> impl IntoElement {
    theme::install(&theme::ThemeChoice::default());
    provide_context(Navigator::new(Route::Settings(SettingsPage::Projects)));
    let owner = Owner::current();
    let spawner: Spawner = Rc::new(move |future| {
        owner.run(|| {
            spawn(future);
        });
    });
    let server = FakeEnvironment::new("machine", spawner.clone());
    server.set_permissions(&[
        "orchestration:read",
        "orchestration:operate",
        "settings:write",
        "filesystem:read",
    ]);
    server.edit_config(|config| {
        config["environment"]["capabilities"]["projectSettingsOverrides"] = json!(true);
        config["environment"]["capabilities"]["projectOrganization"] = json!(true);
    });
    let (name, command, id) = if props.imports_script {
        ("Lint", "bun lint", "lint")
    } else {
        ("Test", "bun test", "test")
    };
    let mut saved = serde_json::to_value(t3_contracts::settings::ServerSettings::default())
        .expect("server settings response");
    saved["projectSettingsOverrides"]["project"] = json!({
        "defaultThreadEnvMode":"worktree",
        "defaultProjectScripts":[{
            "id":id, "name":name, "command":command, "icon":"play", "runOnWorktreeCreate":false
        }]
    });
    server.reply("server.updateSettings", Ok(saved));
    let mut snapshot = shell(&[("thread", "Thread")]);
    snapshot["projects"][0]["repositoryIdentity"] = json!({
        "canonicalKey":"github.com/team/project",
        "locator":{"source":"git-remote","remoteName":"origin","remoteUrl":"https://github.com/team/project.git"},
        "displayName":"Project","name":"project"
    });
    server.set_shell(snapshot.clone());
    let mut renamed = snapshot["projects"][0].clone();
    renamed["title"] = json!("Project renamed");
    renamed["deletedAt"] = json!(null);
    server.register("projects.mutate", t3_client::testing::CallKind::Unary);
    server.reply("projects.mutate", Ok(renamed));
    server.register("projects.readFile", t3_client::testing::CallKind::Unary);
    let manifest = r#"{"scripts":[{"name":"Lint","command":"bun lint"}]}"#;
    server.reply("projects.readFile", Ok(json!({
        "relativePath":"t3.json", "contents":manifest, "byteLength":manifest.len(), "truncated":false
    })));
    server.register(
        "server.upsertKeybinding",
        t3_client::testing::CallKind::Unary,
    );
    server.reply(
        "server.upsertKeybinding",
        Ok(json!({"keybindings":[],"issues":[]})),
    );
    server.reply(
        "projects.searchEntries",
        Ok(json!({
            "entries": [{"path":"assets/logo.svg","kind":"file"}], "truncated": false
        })),
    );
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
            pairing.await.expect("pair the settings environment");
        }
        .boxed_local(),
    );

    let recorded = signal("");
    on_key("ctrl+shift+u", move |_| {
        if server
            .calls("server.upsertKeybinding")
            .iter()
            .any(|call| call["key"] == "ctrl+alt+l" && call["command"] == "script.test.run")
        {
            recorded.set("Shortcut received");
            return;
        }
        for call in server.calls("server.updateSettings") {
            let project = &call["patch"]["projectSettingsOverrides"]["project"];
            if project["defaultThreadEnvMode"] == "worktree" {
                recorded.set("Workspace default received");
                return;
            }
            if project["defaultProjectScripts"][0]["name"] == "Lint"
                && project["defaultProjectScripts"][0]["command"] == "bun lint"
            {
                recorded.set("Imported action received");
                return;
            }
            if project["defaultProjectScripts"][0]["name"] == "Test"
                && project["defaultProjectScripts"][0]["command"] == "bun test"
            {
                recorded.set("Action received");
                return;
            }
        }
        let project_calls = server.calls("projects.mutate");
        if let Some(call) = project_calls.last() {
            assert_eq!(project_calls.len(), 1);
            if call["type"] == "project.delete"
                && call["force"] == true
                && call["projectId"] == "project"
            {
                recorded.set("Removal received");
                return;
            }
            assert_eq!(call["type"], "project.update");
            assert_eq!(call["projectId"], "project");
            if call["title"] == "Project renamed" {
                recorded.set("Rename received");
            } else if call["projectIcon"] == json!({"kind":"emoji","emoji":"🚀"})
                && call["faviconPath"].is_null()
            {
                recorded.set("Icon received");
            } else if call["faviconPath"] == "assets/logo.svg" && call["projectIcon"].is_null() {
                let searches = server.calls("projects.searchEntries");
                assert!(
                    searches.iter().any(|search| search["imageOnly"] == true
                        && search["cwd"] == "/workspace/project")
                );
                recorded.set("File selection received");
            } else {
                recorded.set("Unexpected project mutation");
            }
        } else {
            recorded.set("No project mutation received");
        }
    });
    provide_context(Rt::new(store, Rc::new(client.clone()), Choice::Client));
    view()
        .flex_1()
        .child(text(move || recorded.get()))
        .child(untracked(|| {
            settings_shell::screen_for(SettingsPage::Projects, &client)
        }))
        .child(t3_ui::ui::confirm::confirm_dialog_host())
}

fn start_project(window: (f32, f32)) -> Headless {
    start_project_with_reply(window, false)
}

fn start_project_with_reply(window: (f32, f32), imports_script: bool) -> Headless {
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::new::<ProjectsFixture>(move || ProjectsFixtureProps {
            imports_script,
        }))
    };
    if window == DESKTOP {
        config.environment.os = nsh_core::platform::Os::Linux;
        config.environment.idiom = nsh_core::platform::Idiom::Desktop;
    }
    let mut app = Headless::start(config).expect("headless projects route");
    app.wait_for("role:button[Project]")
        .expect("the paired project in the settings list");
    app.press("role:button[Project]").expect("select project");
    app
}

fn rename_project(window: (f32, f32)) {
    let mut app = start_project(window);
    app.wait_for("label:Project name")
        .expect("the selected project's rename field");
    app.type_text("label:Project name", " renamed")
        .expect("edit the project title");
    app.press_key("enter").expect("submit the edited title");
    app.settle();
    app.press_key("ctrl+shift+u").expect("observe the mutation");
    app.wait_for("Rename received")
        .expect("the rename reached the paired environment");
}

#[test]
fn scoped_project_rename_reaches_the_environment_on_desktop() {
    rename_project(DESKTOP);
}

#[test]
fn scoped_project_rename_reaches_the_environment_on_phone() {
    rename_project(PHONE);
}

#[test]
fn emoji_picker_clears_favicon_path_on_desktop() {
    let mut app = start_project(DESKTOP);
    app.press("role:button[Choose a project icon]")
        .expect("open the icon picker");
    app.press("role:button[Emoji]").expect("switch to emoji");
    app.press("role:button[Rocket]").expect("pick an emoji");
    app.press("role:button[Save icon]").expect("save the emoji");
    app.settle();
    app.press_key("ctrl+shift+u").expect("observe the mutation");
    app.wait_for("Icon received")
        .expect("icon and cleared file sent together");
}

#[test]
fn image_picker_searches_selected_checkout_and_clears_custom_icon_on_phone() {
    let mut app = start_project(PHONE);
    app.press("role:button[Choose a project icon file]")
        .expect("open the file picker");
    app.wait_for("role:button[logo.svg]")
        .expect("the image search response");
    app.press("role:button[logo.svg]")
        .expect("use the in-project file");
    app.settle();
    app.press_key("ctrl+shift+u").expect("observe the mutation");
    app.wait_for("File selection received")
        .expect("path and cleared icon sent together");
}

#[test]
fn workspace_default_writes_only_the_selected_checkout_override() {
    let mut app = start_project(DESKTOP);
    app.press("role:combobox[Default workspace]")
        .expect("open workspace default");
    app.press("role:option[New worktree]")
        .expect("use a worktree");
    app.settle();
    app.press_key("ctrl+shift+u").expect("observe the mutation");
    app.wait_for("Workspace default received")
        .expect("override written to the paired environment");
}

#[test]
fn regrouping_keeps_the_physical_checkout_selected() {
    let mut app = start_project(DESKTOP);
    app.scroll("id:settings-page-content", px(510.0))
        .expect("show checkout controls");
    app.press("role:combobox[/^Grouping rule for/]")
        .expect("open checkout grouping");
    app.press("role:option[Keep separate]")
        .expect("switch to a physical project key");
    app.wait_for("role:button[Choose a project icon]")
        .expect("the regrouped project remains selected");
    app.scroll("id:settings-page-content", px(510.0))
        .expect("show checkout controls again");
    app.wait_for("Keep separate")
        .expect("the override survived regrouping");
}

fn add_action(window: (f32, f32)) {
    let mut app = start_project(window);
    app.scroll("id:settings-page-content", px(380.0))
        .expect("show the Actions section");
    app.press("role:button[Add action]")
        .expect("open the action editor");
    app.type_text("label:Name", "Test")
        .expect("enter an action name");
    app.type_text("label:Command", "bun test")
        .expect("enter an action command");
    app.press("role:button[Save action]")
        .expect("save the action");
    app.settle();
    app.press_key("ctrl+shift+u")
        .expect("observe the scoped settings patch");
    app.wait_for("Action received")
        .expect("the action reached the paired checkout");
}

#[test]
fn project_actions_save_scoped_scripts_on_desktop() {
    add_action(DESKTOP);
}

#[test]
fn project_actions_save_scoped_scripts_on_phone() {
    add_action(PHONE);
}

#[test]
fn removing_a_project_requires_confirmation() {
    let mut app = start_project(DESKTOP);
    app.scroll("id:settings-page-content", px(1100.0))
        .expect("show the danger section");
    app.press("role:button[Remove project]")
        .expect("request project deletion");
    app.wait_for("text:/Remove project.*and delete its 1 thread/")
        .expect("confirmation counts visible threads");
    app.press_key("ctrl+shift+u")
        .expect("check for an early mutation");
    app.wait_for("No project mutation received")
        .expect("wait for confirmation before removing anything");
    app.press("role:button[Confirm]")
        .expect("confirm the removal");
    app.settle();
    app.press_key("ctrl+shift+u").expect("observe the mutation");
    app.wait_for("Removal received")
        .expect("scoped deletion reached the environment");
}

#[test]
fn editing_an_action_can_save_a_shortcut_on_its_environment() {
    let mut app = start_project(DESKTOP);
    app.scroll("id:settings-page-content", px(380.0))
        .expect("show the Actions section");
    app.press("role:button[Add action]")
        .expect("open the action editor");
    app.type_text("label:Name", "Test")
        .expect("enter an action name");
    app.type_text("label:Command", "bun test")
        .expect("enter an action command");
    app.type_text("label:Keybinding", "ctrl+alt+l")
        .expect("enter a shortcut");
    app.press("role:button[Save action]")
        .expect("save the action and its shortcut");
    app.settle();
    app.press_key("ctrl+shift+u")
        .expect("observe the shortcut write");
    app.wait_for("Shortcut received")
        .expect("the shortcut reached the selected environment");
}

#[test]
fn project_actions_import_a_t3_json_script_on_phone() {
    let mut app = start_project_with_reply(PHONE, true);
    app.scroll("id:settings-page-content", px(380.0))
        .expect("show the Actions section");
    app.press("role:button[Import scripts]")
        .expect("open import choices");
    app.press("role:menuitem[Lint]")
        .expect("import the project script");
    app.settle();
    app.press_key("ctrl+shift+u")
        .expect("observe the scoped settings patch");
    app.wait_for("Imported action received")
        .expect("the imported action reached the checkout");
}
