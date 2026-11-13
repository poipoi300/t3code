use serde_json::{Value, json};
use t3_config::{home::HomeLayout, keybindings::*};
use t3_contracts::keybindings::KeybindingRule;

fn rule(key: &str, command: &str, when: Option<&str>) -> KeybindingRule {
    let mut value = json!({"key":key,"command":command});
    if let Some(when) = when {
        value["when"] = json!(when);
    }
    serde_json::from_value(value).unwrap()
}
fn write(path: &std::path::Path, value: Value) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
}
#[test]
fn shortcuts_and_when_parser() {
    assert_eq!(parse_shortcut("mod++").unwrap().key, "+");
    assert_eq!(
        encode_shortcut(&parse_shortcut("cmd+control+option+SHIFT+space").unwrap()),
        "meta+ctrl+alt+shift+space"
    );
    assert!(parse_shortcut("mod+a+b").is_none());
    assert!(parse_shortcut("mod").is_none());
    assert!(parse_when("a &&").is_none());
    assert_eq!(
        encode_when(&parse_when("a&&(!b||c)").unwrap()),
        "(a && (!(b) || c))"
    );
    assert!(parse_when(&format!("{}a{}", "(".repeat(65), ")".repeat(65))).is_none());
    assert!(parse_when(&format!("{}a", "!".repeat(65))).is_none());
    assert_eq!(default_bindings().len(), 88);
}
#[tokio::test]
async fn bootstrap_invalid_and_newer_entries() {
    let temp = tempfile::tempdir().unwrap();
    let home = HomeLayout::derive(temp.path(), false, true);
    let path = home.keybindings();
    let service = KeybindingsService::new(&path);
    service.sync_defaults().await.unwrap();
    assert_eq!(service.snapshot().await.unwrap().keybindings.0.len(), 88);
    assert!(
        std::fs::read_to_string(home.keybindings_migrations())
            .unwrap()
            .contains("composer.sendBackground:mod+enter")
    );
    std::fs::write(&path, "[ broken").unwrap();
    let invalid = service.reload().await.unwrap();
    assert!(matches!(invalid.issues[0], ConfigIssue::MalformedConfig(_)));
    assert_eq!(invalid.keybindings.0.len(), 88);
    assert!(
        service
            .upsert(rule("mod+y", "chat.new", None), None)
            .await
            .is_err()
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "[ broken");
    let unknown = json!({"key":"mod+?","command":"future.command","newField":1});
    write(
        &path,
        json!([unknown,{"key":"mod+a+b","command":"chat.new"},{"key":"mod+y","command":"chat.new"}]),
    );
    assert_eq!(service.reload().await.unwrap().issues.len(), 2);
    service
        .upsert(rule("mod+q", "sidebar.toggle", None), None)
        .await
        .unwrap();
    let raw: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(raw[0], unknown);
}
#[tokio::test]
async fn normalized_edit_identity_and_concurrency() {
    let temp = tempfile::tempdir().unwrap();
    let path = HomeLayout::derive(temp.path(), false, true).keybindings();
    write(
        &path,
        json!([{"key":"cmd+k","command":"chat.new","when":"a&&b"},{"key":"mod+o","command":"chat.new"}]),
    );
    let service = KeybindingsService::new(&path);
    service
        .upsert(
            rule("mod+l", "chat.new", None),
            Some(rule("meta+k", "chat.new", Some("a && b"))),
        )
        .await
        .unwrap();
    service
        .remove(&rule("mod+l", "chat.new", None))
        .await
        .unwrap();
    let a = service.upsert(rule("alt+1", "chat.new", None), None);
    let b = service.upsert(rule("alt+2", "chat.new", None), None);
    let (a, b) = tokio::join!(a, b);
    a.unwrap();
    b.unwrap();
    let raw: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(raw.len(), 3);
    service
        .upsert(
            rule("mod+o", "chat.new", None),
            Some(rule("alt+2", "chat.new", None)),
        )
        .await
        .unwrap();
    let raw: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(raw.len(), 2);
}
#[tokio::test]
async fn late_default_once_and_full_file_pending() {
    let temp = tempfile::tempdir().unwrap();
    let home = HomeLayout::derive(temp.path(), false, true);
    let path = home.keybindings();
    let alongside = json!({"key":"mod+alt+enter","command":"composer.sendBackground","when":"composerFocus && draftThreadRoute"});
    let mut rules = vec![alongside.clone()];
    rules.extend((0..255).map(|i| json!({"key":format!("alt+f{i}"),"command":"chat.new"})));
    write(&path, json!(rules));
    let service = KeybindingsService::new(&path);
    service.sync_defaults().await.unwrap();
    assert!(!home.keybindings_migrations().exists());
    std::fs::write(home.keybindings_migrations(), "older\nolder\n").unwrap();
    write(&path, json!([alongside]));
    service.sync_defaults().await.unwrap();
    assert_eq!(
        std::fs::read_to_string(home.keybindings_migrations()).unwrap(),
        "older\ncomposer.sendBackground:mod+enter\n"
    );
    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(raw.contains("mod+enter"));
    service
        .remove(&rule(
            "mod+enter",
            "composer.sendBackground",
            Some("composerFocus && draftThreadRoute"),
        ))
        .await
        .unwrap();
    service.sync_defaults().await.unwrap();
    assert!(
        !std::fs::read_to_string(&path)
            .unwrap()
            .contains("\"key\": \"mod+enter\",\n    \"command\": \"composer.sendBackground\"")
    );
}
#[tokio::test(start_paused = true)]
async fn external_edits_emit_issues_and_recover() {
    let temp = tempfile::tempdir().unwrap();
    let path = HomeLayout::derive(temp.path(), false, true).keybindings();
    let service = KeybindingsService::new(&path);
    let _watcher = service.start().await.unwrap();
    let mut events = service.subscribe_changes();
    std::fs::write(&path, "{bad}").unwrap();
    let event = events.recv().await.unwrap();
    assert!(!event.issues.is_empty());
    write(&path, json!([{"key":"alt+q","command":"chat.new"}]));
    loop {
        let event = events.recv().await.unwrap();
        if event.issues.is_empty() {
            assert!(event.keybindings.0.iter().any(|r| r.shortcut.key == "q"));
            break;
        }
    }
}
