#[cfg(windows)]
#[tokio::test(start_paused = true)]
async fn cached_readers_continue_during_locked_file_saves() {
    use serde_json::json;
    use std::os::windows::fs::OpenOptionsExt;
    use t3_config::{home::HomeLayout, keybindings::KeybindingsService, settings::SettingsService};
    let temp = tempfile::tempdir().unwrap();
    let home = HomeLayout::derive(temp.path(), false, true);
    let settings = SettingsService::new(home.settings());
    let initial = serde_json::from_value(json!({"threadTitleInstructions":"before"})).unwrap();
    settings.update(&initial).await.unwrap();
    let keys = KeybindingsService::new(home.keybindings());
    keys.sync_defaults().await.unwrap();
    let before_keys = keys.snapshot().await.unwrap();
    let locked_settings = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(home.settings())
        .unwrap();
    let locked_keys = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(home.keybindings())
        .unwrap();
    let patch = serde_json::from_value(json!({"threadTitleInstructions":"after"})).unwrap();
    let save_settings = settings.update(&patch);
    let save_keys = keys.upsert(
        serde_json::from_value(json!({"key":"alt+q","command":"chat.new"})).unwrap(),
        None,
    );
    tokio::pin!(save_settings, save_keys);
    tokio::select! { biased; r = &mut save_settings => panic!("save did not block: {r:?}"), _ = async {} => () }
    tokio::select! { biased; r = &mut save_keys => panic!("save did not block: {r:?}"), _ = async {} => () }
    let readers = async {
        for _ in 0..4 {
            let (s, k) = tokio::join!(settings.snapshot(), keys.snapshot());
            assert_eq!(
                serde_json::to_value(s.unwrap()).unwrap()["threadTitleInstructions"],
                "before"
            );
            assert_eq!(k.unwrap(), before_keys);
        }
    };
    tokio::select! { biased; _ = readers => (), _ = async {} => panic!("cached readers blocked behind save") }
    drop((locked_settings, locked_keys));
    let (s, k) = tokio::join!(save_settings, save_keys);
    assert_eq!(
        serde_json::to_value(s.unwrap()).unwrap()["threadTitleInstructions"],
        "after"
    );
    assert!(
        k.unwrap()
            .keybindings
            .0
            .iter()
            .any(|r| r.shortcut.key == "q")
    );
}
