use t3_config::{home::HomeLayout, keybindings::KeybindingsService, settings::SettingsService};

#[tokio::test(start_paused = true)]
async fn both_watchers_reload_edits_through_a_symlinked_home() {
    let temp = tempfile::tempdir().unwrap();
    let real = temp.path().join("real");
    let linked = temp.path().join("home");
    std::fs::create_dir(&real).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(&real, &linked).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&real, &linked).unwrap();
    let home = HomeLayout::derive(&linked, false, true);
    let settings = SettingsService::new(home.settings());
    let keys = KeybindingsService::new(home.keybindings());
    let _settings_watch = settings.start().await.unwrap();
    let _keys_watch = keys.start().await.unwrap();
    let mut settings_events = settings.subscribe_changes();
    let mut keys_events = keys.subscribe_changes();
    let canonical = std::fs::canonicalize(real.join("userdata")).unwrap();
    std::fs::write(
        canonical.join("settings.json"),
        r#"{"threadTitleInstructions":"canonical edit"}"#,
    )
    .unwrap();
    std::fs::write(
        canonical.join("keybindings.json"),
        r#"[{"key":"alt+q","command":"chat.new"}]"#,
    )
    .unwrap();
    loop {
        let state = settings_events.recv().await.unwrap();
        if serde_json::to_value(state).unwrap()["threadTitleInstructions"] == "canonical edit" {
            break;
        }
    }
    loop {
        let state = keys_events.recv().await.unwrap();
        if state.keybindings.0.iter().any(|r| r.shortcut.key == "q") {
            break;
        }
    }
}
