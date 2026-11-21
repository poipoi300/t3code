//! Headless Appearance, Diffs and Worktrees tests through the real client.

use nsh_core::prelude::*;
use t3_ui::state::nav::SettingsPage;

use crate::set_shell::{Remote, ReviewFixture, start, start_connected, start_connected_with};
use crate::support::{DESKTOP, PHONE};

#[test]
fn appearance_saves_math_rendering_and_resets_it_at_both_sizes() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window, SettingsPage::Appearance);
        app.wait_for("role:button[Use dark mode]")
            .expect("appearance page");
        let math = "role:switch[Render math]";
        for _ in 0..6 {
            if app.screen().matches(math).expect("math selector") {
                break;
            }
            app.scroll("id:settings-page-content", nsh_core::prelude::px(350.0))
                .expect("scroll to the interface controls");
        }
        app.wait_for(math).expect("math row");
        app.press("role:switch[Render math]").expect("toggle");
        let reset = "role:button[Reset math rendering to default]";
        app.wait_for(reset).expect("saved non-default setting");
        app.press(reset).expect("restore default");
        app.wait_for_gone(reset, std::time::Duration::from_secs(5))
            .expect("the client saved the default");
    }
}

#[test]
fn diffs_saves_whitespace_and_resets_it_at_both_sizes() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window, SettingsPage::Diffs);
        let toggle = "role:switch[Hide whitespace changes by default]";
        app.wait_for(toggle).expect("diff whitespace row");
        app.press(toggle).expect("toggle");
        let reset = "role:button[Reset diff whitespace changes to default]";
        app.wait_for(reset).expect("saved non-default setting");
        app.press(reset).expect("restore default");
        app.wait_for_gone(reset, std::time::Duration::from_secs(5))
            .expect("the client saved the default");
    }
}

#[test]
fn worktrees_saves_start_from_remote_on_the_environment_at_both_sizes() {
    for window in [DESKTOP, PHONE] {
        let mut app = start_connected(window, SettingsPage::Worktrees, Remote::default());
        let toggle = "role:switch[Start new worktrees from remote by default]";
        app.wait_for(toggle).expect("worktree creation row");
        app.press(toggle).expect("toggle");
        app.press_key("ctrl+shift+u")
            .expect("read fake server calls");
        app.wait_for("Updates 1").expect("one server update");
        app.wait_for(r#"text:/"newWorktreesStartFromOrigin":true/"#)
            .expect("patch written to the environment");
    }
}

#[test]
fn diffs_saves_the_split_layout_at_both_sizes() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window, SettingsPage::Diffs);
        app.press("role:combobox[Diff layout]")
            .expect("layout picker");
        app.wait_for("role:option[Split]")
            .expect("side-by-side option");
        app.press("role:option[Split]").expect("pick split");
        app.wait_for("role:button[Reset diff layout to default]")
            .expect("layout persisted");
    }
}

fn scroll_to(app: &mut nsh_testing::Headless, selector: &str) {
    for _ in 0..15 {
        if app.screen().matches(selector).expect("selector") {
            break;
        }
        app.scroll("id:settings-page-content", nsh_core::prelude::px(250.0))
            .expect("scroll settings");
    }
    app.wait_for(selector).expect("settings control");
}

fn cleanup_fixture(window: (f32, f32)) -> nsh_testing::Headless {
    start_connected_with(
        window,
        SettingsPage::Worktrees,
        Remote::default(),
        ReviewFixture::WorktreeCleanup,
    )
}

#[test]
fn review_appearance_ignores_a_device_preference_loaded_after_a_local_choice() {
    for window in [DESKTOP, PHONE] {
        let mut app = start_connected_with(
            window,
            SettingsPage::Appearance,
            Remote::default(),
            ReviewFixture::SlowAppearance,
        );
        let dark = "role:button[Use dark mode]";
        app.press(dark)
            .expect("choose dark while the saved mode is loading");
        app.press_key("ctrl+shift+e")
            .expect("finish loading the old light mode");
        app.wait_until(
            "dark choice survives the delayed read",
            nsh_testing::WAIT,
            |screen| screen.selected(dark) == Ok(true),
        )
        .expect("dark mode");
    }
}

#[test]
fn review_thread_details_menu_disables_reset_at_default_and_restores_changed_items() {
    for window in [DESKTOP, PHONE] {
        let mut app = start_connected(window, SettingsPage::Appearance, Remote::default());
        let button = "role:button[/^Thread details:/]";
        scroll_to(&mut app, button);
        app.press(button).expect("open thread details");
        let reset = "role:menuitem[Reset to default]";
        app.wait_for(reset).expect("reset in the menu");
        assert!(
            !app.screen()
                .enabled(reset)
                .expect("reset disabled at default")
        );
        app.press("role:menuitemcheckbox[Environment and worktree]")
            .expect("hide item");
        assert!(
            app.screen()
                .enabled(reset)
                .expect("reset enabled after change")
        );
        app.press(reset).expect("restore default");
        app.wait_for("role:button[Thread details: 10 of 12 on]")
            .expect("default items");
    }
}

#[test]
fn review_worktree_retention_hides_before_the_server_replies_and_restores_on_failure() {
    use nsh_core::Checked;
    for window in [DESKTOP, PHONE] {
        let mut app = cleanup_fixture(window);
        let toggle = "role:switch[Delete inactive worktrees]";
        scroll_to(&mut app, toggle);
        assert_eq!(
            app.screen()
                .accessibility_state(toggle)
                .expect("retention")
                .checked,
            Some(Checked::True)
        );
        app.press(toggle)
            .expect("switch off while the update is in flight");
        assert_eq!(
            app.screen()
                .accessibility_state(toggle)
                .expect("retention")
                .checked,
            Some(Checked::False)
        );
        app.press_key("ctrl+shift+u").expect("read pending RPC");
        app.wait_for(r#"text:/"worktreeAfterDays":null/"#)
            .expect("sent null");
        app.press_key("ctrl+shift+e")
            .expect("server rejects update");
        app.wait_until(
            "server rejection restores the value",
            nsh_testing::WAIT,
            |screen| {
                screen
                    .accessibility_state(toggle)
                    .is_ok_and(|state| state.checked == Some(Checked::True))
            },
        )
        .expect("retention restored on failure");
    }
}

#[test]
fn review_worktree_keep_when_has_no_selected_option_when_mixed() {
    for window in [DESKTOP, PHONE] {
        let mut app = cleanup_fixture(window);
        let toggle = "role:switch[Delete inactive worktrees]";
        scroll_to(&mut app, toggle);
        let advanced = "id:storage-worktrees >> text:Advanced";
        app.scroll("id:settings-page-content", nsh_core::prelude::px(240.0))
            .expect("move to cleanup advanced");
        app.press(advanced).expect("open cleanup advanced");
        let keep = "role:combobox[Keep worktrees with local changes]";
        scroll_to(&mut app, keep);
        app.press(keep).expect("mixed value can be changed");
        for option in [
            "Any local files",
            "Uncommitted changes",
            "Edited tracked files",
        ] {
            assert_eq!(
                app.screen().selected(&format!("role:option[{option}]")),
                Ok(false)
            );
        }
    }
}

#[test]
fn review_settled_worktree_retention_warns_when_a_selected_machine_is_offline() {
    for window in [DESKTOP, PHONE] {
        let mut app = cleanup_fixture(window);
        scroll_to(&mut app, "role:switch[Delete inactive worktrees]");
        app.scroll("id:settings-page-content", nsh_core::prelude::px(240.0))
            .expect("move to cleanup advanced");
        app.press("id:storage-worktrees >> text:Advanced")
            .expect("open cleanup advanced");
        scroll_to(&mut app, "role:switch[Delete settled worktrees]");
        app.press_key("ctrl+shift+o")
            .expect("take a machine offline");
        app.wait_for("Offline machines will not get this setting")
            .expect("settled rule warns about the offline machine");
    }
}

#[test]
fn themes_mixing_changes_only_the_picked_half_at_both_sizes() {
    for size in [DESKTOP, PHONE] {
        let mut app = start_connected(size, SettingsPage::Appearance, Remote::default());
        let light = "role:button[Use Ocean light mode]";
        let dark = "role:button[Use Ocean dark mode]";
        scroll_to(&mut app, light);
        app.press(light).expect("select the light half");
        assert_eq!(app.screen().selected(light), Ok(true));
        assert_eq!(app.screen().selected(dark), Ok(false));
        app.press_key("ctrl+shift+y").expect("read device mix");
        app.wait_for(r#"text:/Saved themes .*"light":"ocean"/"#)
            .expect("mix written separately");
    }
}

#[test]
fn themes_custom_json_import_can_be_edited_at_both_sizes() {
    for size in [DESKTOP, PHONE] {
        let mut app = start_connected(size, SettingsPage::Appearance, Remote::default());
        app.press("role:button[Add theme]").expect("open importer");
        let mut theme = t3_theme::palette::built_in_theme("ocean")
            .expect("Ocean")
            .clone();
        theme.id = "test-aurora".into();
        theme.label = "Aurora".into();
        let json = t3_theme::palette::serialize_theme_file(&theme).expect("valid theme file");
        let mut paste = nsh_core::PasteEvent::default();
        paste.text = json;
        app.paste("label:Theme JSON", &paste).expect("paste theme");
        app.press("id:theme-import-submit").expect("import");
        let edit = "role:button[Edit Aurora]";
        scroll_to(&mut app, edit);
        app.press(edit).expect("edit imported theme");
        app.type_text("label:Theme name", " II").expect("rename");
        app.press("role:button[Save changes]").expect("save theme");
        scroll_to(&mut app, "role:button[Edit Aurora II]");
        app.press_key("ctrl+shift+y").expect("read device library");
        app.wait_for("text:/Saved themes .*Aurora II/")
            .expect("edited theme persisted");
    }
}

#[test]
fn themes_open_picker_uses_the_newly_typed_color_for_a_hue_drag() {
    use nsh_core::prelude::{point, px};
    let mut app = start_connected(DESKTOP, SettingsPage::Appearance, Remote::default());
    app.press("role:button[Create theme]").expect("open editor");
    app.press("role:button[Pick Accent color]")
        .expect("open the picker");
    app.clear("label:Accent").expect("clear the current value");
    app.type_text("label:Accent", "#808080")
        .expect("enter a neutral gray");
    app.drag("role:adjustable[Accent hue]", point(px(400.0), px(0.0)))
        .expect("adjust hue");
    assert_eq!(app.screen().input_text("label:Accent"), Ok(Some("#808080")));
}

#[component]
fn ThemeLaunch() -> impl IntoElement {
    use std::{cell::RefCell, rc::Rc};
    use t3_client::{
        ClientStore,
        composition::{ClientRuntime, Spawner},
        platform::StorageKey,
        testing::environment::FakeEnvironment,
    };
    use t3_ui::{
        runtime::{Rt, host::Choice},
        state::nav::Route,
    };
    let tasks: Spawner = Rc::new(|future| {
        spawn(future);
    });
    let env = FakeEnvironment::new("env", tasks.clone());
    let held = RefCell::new(Some(env.defer_read(StorageKey::new(
        "ui-preference",
        None,
        "t3code:theme",
    ))));
    on_key("ctrl+shift+e", move |_| {
        if let Some(reply) = held.borrow_mut().take() {
            let _ = reply.send(Some(b"ocean".to_vec()));
        }
    });
    let delayed_save =
        RefCell::<Option<futures::channel::oneshot::Sender<Option<Vec<u8>>>>>::new(None);
    let writes = env.clone();
    // The first press reserves the next library read in the save transaction, the second releases it.
    on_key("ctrl+shift+k", move |_| {
        let mut gate = delayed_save.borrow_mut();
        if let Some(reply) = gate.take() {
            let _ = reply.send(None);
        } else {
            *gate =
                Some(writes.defer_read(StorageKey::new("ui-preference", None, "t3code:themes:v1")));
        }
    });
    let store = ClientStore::default();
    let client = ClientRuntime::new(
        env.platform(),
        store.clone(),
        t3_contracts::orchestration_v2::OrchestrationV2CreationSource::Web,
        tasks,
    );
    let rt = Rt::new(store, Rc::new(client.clone()), Choice::Client);
    let ocean = t3_ui::theme::tokens_for(&t3_ui::theme::ThemeChoice {
        theme: "ocean".into(),
        ..Default::default()
    })
    .color("background")
    .expect("Ocean background");
    view()
        .size_full()
        .child(text(move || {
            if t3_ui::theme::has_theme_id()
                && nsh_core::prelude::tokens().color("background") == Some(ocean)
            {
                "Ocean painted"
            } else {
                "Waiting for catalogue"
            }
        }))
        .child(t3_ui::app::app_tree(
            rt,
            Route::Settings(SettingsPage::General),
            move || t3_ui::app::provide_client_services(&client),
        ))
}

fn start_theme_launch(viewport: (f32, f32)) -> nsh_testing::Headless {
    use nsh_core::prelude::*;
    use nsh_engine::{EngineConfig, Root};
    nsh_testing::Headless::start(EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(viewport.0), px(viewport.1)),
        ..EngineConfig::new(Root::of::<ThemeLaunch>())
    })
    .expect("start application root")
}

#[test]
fn themes_editor_preserves_its_draft_when_another_route_is_opened() {
    for window in [DESKTOP, PHONE] {
        let mut app = start_theme_launch(window);
        app.wait_for("Waiting for catalogue")
            .expect("launch fixture mounted");
        app.press_key("ctrl+shift+e").expect("release saved theme");
        app.wait_for("role:button[Appearance]")
            .expect("settings navigation");
        app.press("role:button[Appearance]")
            .expect("open Appearance");
        app.press("role:button[Create theme]").expect("open editor");
        app.type_text("label:Theme name", "Across routes")
            .expect("enter draft");
        app.press("role:button[Minimize the theme editor]")
            .expect("minimize editor");
        app.press("role:button[Chat]").expect("visit Chat settings");
        app.press("role:button[Expand the theme editor]")
            .expect("reopen editor over Chat");
        assert_eq!(
            app.screen().input_text("label:Theme name"),
            Ok(Some("Across routes"))
        );
        app.press_key("ctrl+shift+k").expect("hold save");
        app.press("role:button[Create theme]")
            .expect("save from Chat");
        app.press("role:button[Minimize the theme editor]")
            .expect("minimize pending save");
        app.press("role:button[Close]").expect("close Settings");
        app.wait_for_gone("id:settings-page-content", nsh_testing::WAIT)
            .expect("back in the shell");
        app.wait_for("role:button[Expand the theme editor]")
            .expect("root host retained the editor");
        app.press_key("ctrl+shift+k").expect("let the save finish");
        app.wait_for_gone("id:theme-editor-panel", nsh_testing::WAIT)
            .expect("saved editor closed in its new host");
    }
}

#[test]
fn themes_root_waits_for_saved_palette_before_mounting_general() {
    let mut app = start_theme_launch(DESKTOP);
    app.wait_for("Waiting for catalogue")
        .expect("storage pending");
    assert!(
        !app.screen().shows("Notifications"),
        "General painted before the saved palette arrived"
    );
    app.press_key("ctrl+shift+e").expect("release saved theme");
    app.wait_for("Notifications").expect("General mounted");
    assert!(
        app.screen().shows("Ocean painted"),
        "first mounted route has saved palette"
    );
}

#[test]
fn themes_device_formats_keep_library_halves_and_legacy_follow_flag() {
    use serde_json::json;
    use std::rc::Rc;
    use t3_client::{ClientStore, composition::ClientRuntime, testing::FakePlatform};
    let pool = futures::executor::LocalPool::new();
    let client = ClientRuntime::new(
        FakePlatform::default(),
        ClientStore::default(),
        t3_contracts::orchestration_v2::OrchestrationV2CreationSource::Web,
        Rc::new(|_| {}),
    );
    let mut pool = pool;
    let settings = client.settings();
    for (key, value) in [
        ("t3code:themes:v1", json!([{"future": 42}])),
        ("t3code:theme-halves:v1", json!({"dark":"ocean"})),
        ("t3code:theme-follow-system", json!(false)),
    ] {
        pool.run_until(settings.write_device_preference(key, value.clone()))
            .expect("typed theme setting");
        assert_eq!(
            pool.run_until(settings.read_device_preference(key)),
            Some(value)
        );
        assert!(
            pool.run_until(settings.write_device_preference(key, json!("false")))
                .is_err()
        );
    }
}

#[test]
fn themes_host_s_published_palette_picks_its_one_available_half_at_both_sizes() {
    for size in [DESKTOP, PHONE] {
        let mut app = start_connected_with(
            size,
            SettingsPage::Appearance,
            Remote::default(),
            ReviewFixture::PublishedThemes,
        );
        let whole = "role:button[Use Lab dusk theme]";
        scroll_to(&mut app, whole);
        app.press(whole).expect("choose the published palette");
        assert_eq!(
            app.screen().selected("role:button[Use Lab dusk dark mode]"),
            Ok(true)
        );
        assert!(
            !app.screen()
                .matches("role:button[Use Lab dusk light mode]")
                .unwrap()
        );
    }
}
