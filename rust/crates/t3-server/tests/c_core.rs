#![cfg(feature = "local-services")]
#![allow(clippy::unwrap_used)]
mod c_support;
use c_support::*;
use serde_json::{Value, json};
use t3_rpc::{CallEvent, CallKind, Request};

fn keybindings_file(fx: &Fixture) -> Value {
    let raw = std::fs::read_to_string(fx.temp.path().join("userdata/keybindings.json")).unwrap();
    serde_json::from_str(&raw).unwrap()
}

fn has_rule(rules: &Value, key: &str, command: &str) -> bool {
    rules
        .as_array()
        .unwrap()
        .iter()
        .any(|rule| rule["key"] == key && rule["command"] == command)
}

fn has_resolved(config: &Value, command: &str, key: &str) -> bool {
    config.as_array().unwrap().iter().any(|rule| {
        rule["command"] == command
            && rule["shortcut"]["key"] == key
            && rule["shortcut"]["ctrlKey"] == true
            && rule["shortcut"]["altKey"] == true
            && rule["shortcut"]["shiftKey"] == true
    })
}

#[tokio::test]
async fn keybindings_upsert_replace_and_remove_write_the_file_and_notify_subscribers() {
    let fx = Fixture::start().await;
    let (mut socket, mut client) = fx.connect(&fx.cookie, "").await;
    let (mut calls, mut calls_client) = fx.connect(&fx.cookie, "").await;
    client
        .start("subscribeServerConfig", json!({}), CallKind::Stream)
        .unwrap();
    assert_eq!(chunk(&mut socket, &mut client).await["type"], "snapshot");

    let added = unary_ok(
        &mut calls,
        &mut calls_client,
        "server.upsertKeybinding",
        json!({"key": "ctrl+alt+shift+f11", "command": "chat.new"}),
    )
    .await;
    assert!(
        has_resolved(&added["keybindings"], "chat.new", "f11"),
        "{added}"
    );
    assert_eq!(added["issues"], json!([]));
    assert!(has_rule(
        &keybindings_file(&fx),
        "ctrl+alt+shift+f11",
        "chat.new"
    ));
    let update = chunk(&mut socket, &mut client).await;
    assert_eq!(update["type"], "keybindingsUpdated");
    assert!(has_resolved(
        &update["payload"]["keybindings"],
        "chat.new",
        "f11"
    ));

    let replaced = unary_ok(
        &mut calls,
        &mut calls_client,
        "server.upsertKeybinding",
        json!({
            "key": "ctrl+alt+shift+f12", "command": "chat.new",
            "replace": {"key": "ctrl+alt+shift+f11", "command": "chat.new"}
        }),
    )
    .await;
    assert!(has_resolved(&replaced["keybindings"], "chat.new", "f12"));
    assert!(!has_resolved(&replaced["keybindings"], "chat.new", "f11"));
    let file = keybindings_file(&fx);
    assert!(has_rule(&file, "ctrl+alt+shift+f12", "chat.new"));
    assert!(!has_rule(&file, "ctrl+alt+shift+f11", "chat.new"));

    let removed = unary_ok(
        &mut calls,
        &mut calls_client,
        "server.removeKeybinding",
        json!({"key": "ctrl+alt+shift+f12", "command": "chat.new"}),
    )
    .await;
    assert!(!has_resolved(&removed["keybindings"], "chat.new", "f12"));
    assert!(!has_rule(
        &keybindings_file(&fx),
        "ctrl+alt+shift+f12",
        "chat.new"
    ));
    fx.shutdown().await;
}

#[tokio::test]
async fn a_keybinding_write_against_an_unreadable_file_fails_with_the_typed_error() {
    let fx = Fixture::start().await;
    let (mut socket, mut client) = fx.connect(&fx.cookie, "").await;
    let path = fx.temp.path().join("userdata/keybindings.json");
    std::fs::write(&path, "{broken").unwrap();
    let cause = first_cause(
        unary(
            &mut socket,
            &mut client,
            "server.upsertKeybinding",
            json!({"key": "ctrl+alt+shift+f11", "command": "chat.new"}),
        )
        .await,
    );
    assert_eq!(cause["_tag"], "Fail", "{cause}");
    assert_eq!(cause["error"]["_tag"], "KeybindingsConfigParseError");
    assert!(
        cause["error"]["configPath"]
            .as_str()
            .unwrap()
            .ends_with("keybindings.json")
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{broken");
    fx.shutdown().await;
}

#[tokio::test]
async fn settings_updates_persist_return_the_settings_and_notify_subscribers() {
    let fx = Fixture::start().await;
    let (mut socket, mut client) = fx.connect(&fx.cookie, "").await;
    let (mut calls, mut calls_client) = fx.connect(&fx.cookie, "").await;
    client
        .start("subscribeServerConfig", json!({}), CallKind::Stream)
        .unwrap();
    let snapshot = chunk(&mut socket, &mut client).await;
    assert_eq!(snapshot["config"]["settings"]["defaultAutoPull"], false);

    let settings = unary_ok(
        &mut calls,
        &mut calls_client,
        "server.updateSettings",
        json!({"patch": {"defaultAutoPull": true}}),
    )
    .await;
    assert_eq!(settings["defaultAutoPull"], true);
    let saved: Value = serde_json::from_str(
        &std::fs::read_to_string(fx.temp.path().join("userdata/settings.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(saved["defaultAutoPull"], true);
    let update = chunk(&mut socket, &mut client).await;
    assert_eq!(update["type"], "settingsUpdated");
    assert_eq!(update["payload"]["settings"]["defaultAutoPull"], true);
    let config = unary_ok(&mut calls, &mut calls_client, "server.getConfig", json!({})).await;
    assert_eq!(config["settings"]["defaultAutoPull"], true);
    fx.shutdown().await;
}

#[tokio::test]
async fn a_settings_write_against_an_unreadable_file_fails_with_the_typed_error() {
    let fx = Fixture::start().await;
    let (mut socket, mut client) = fx.connect(&fx.cookie, "").await;
    let path = fx.temp.path().join("userdata/settings.json");
    std::fs::write(&path, "{broken").unwrap();
    let cause = first_cause(
        unary(
            &mut socket,
            &mut client,
            "server.updateSettings",
            json!({"patch": {"defaultAutoPull": true}}),
        )
        .await,
    );
    assert_eq!(cause["_tag"], "Fail", "{cause}");
    assert_eq!(cause["error"]["_tag"], "ServerSettingsError");
    assert_eq!(cause["error"]["operation"], "read-file");
    assert!(
        cause["error"]["settingsPath"]
            .as_str()
            .unwrap()
            .ends_with("settings.json")
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{broken");
    fx.shutdown().await;
}

#[tokio::test]
async fn scopes_guard_every_method_including_discarded_and_unsupported_calls() {
    let fx = Fixture::start().await;
    let reader = fx.scoped_cookie(&["orchestration:read"]).await;
    let writer = fx.scoped_cookie(&["settings:write"]).await;
    let (mut read_socket, mut read_client) = fx.connect(&reader, "").await;
    let (mut write_socket, mut write_client) = fx.connect(&writer, "").await;
    let denied = |cause: Value| {
        assert_eq!(cause["_tag"], "Fail", "{cause}");
        assert_eq!(
            cause["error"]["_tag"], "EnvironmentAuthorizationError",
            "{cause}"
        );
    };

    // Implemented writes are refused without settings:write, and nothing is written.
    for (tag, payload) in [
        (
            "server.updateSettings",
            json!({"patch": {"defaultAutoPull": true}}),
        ),
        (
            "server.upsertKeybinding",
            json!({"key": "ctrl+alt+shift+f11", "command": "chat.new"}),
        ),
        (
            "server.removeKeybinding",
            json!({"key": "ctrl+alt+shift+f11", "command": "chat.new"}),
        ),
        // Unsupported methods are guarded before they report that they are unsupported.
        ("server.runStorageCleanup", json!({})),
    ] {
        denied(first_cause(
            unary(&mut read_socket, &mut read_client, tag, payload).await,
        ));
    }
    assert!(
        !fx.temp.path().join("userdata/settings.json").exists()
            || !std::fs::read_to_string(fx.temp.path().join("userdata/settings.json"))
                .unwrap()
                .contains("defaultAutoPull\":true")
    );

    // A discarded call is acknowledged only after it passes the guard.
    for (tag, payload, kind) in [
        (
            "server.updateSettings",
            json!({"patch": {}}),
            CallKind::Unary,
        ),
        ("server.runStorageCleanup", json!({}), CallKind::Unary),
        ("subscribeAuthAccess", json!({}), CallKind::Stream),
    ] {
        let mut request = Request::new(100u64.into(), tag, payload);
        request.discard = Some(t3_rpc::True);
        read_client.start_request(request, kind).unwrap();
        match event(&mut read_socket, &mut read_client).await {
            CallEvent::Exit { exit, .. } => denied(first_cause(exit)),
            other => panic!("unexpected discard result {other:?}"),
        }
    }

    // A patch that touches providers needs a second scope on top of settings:write.
    denied(first_cause(
        unary(
            &mut write_socket,
            &mut write_client,
            "server.updateSettings",
            json!({"patch": {"providerInstances": {}}}),
        )
        .await,
    ));
    unary_ok(
        &mut write_socket,
        &mut write_client,
        "server.updateSettings",
        json!({"patch": {"defaultAutoPull": true}}),
    )
    .await;

    // With the scope, an unsupported method reports its structured defect, not a typed failure.
    let cause = first_cause(
        unary(
            &mut write_socket,
            &mut write_client,
            "server.runStorageCleanup",
            json!({}),
        )
        .await,
    );
    assert_eq!(cause["_tag"], "Die", "{cause}");
    assert_eq!(cause["defect"]["_tag"], "NotImplemented");
    assert_eq!(cause["defect"]["method"], "server.runStorageCleanup");
    // A discarded unsupported call still completes with that defect once it passes the guard.
    let mut request = Request::new(101u64.into(), "server.runStorageCleanup", json!({}));
    request.discard = Some(t3_rpc::True);
    write_client
        .start_request(request, CallKind::Unary)
        .unwrap();
    match event(&mut write_socket, &mut write_client).await {
        CallEvent::Exit { exit, .. } => {
            assert_eq!(first_cause(exit)["defect"]["_tag"], "NotImplemented");
        }
        other => panic!("unexpected {other:?}"),
    }
    // An unsupported stream ends at once with the same defect. It needs orchestration:read.
    let (mut admin, mut admin_client) = fx.connect(&fx.cookie, "").await;
    admin_client
        .start(
            "server.getStorageCleanupReport",
            json!({}),
            CallKind::Stream,
        )
        .unwrap();
    match event(&mut admin, &mut admin_client).await {
        CallEvent::Exit { exit, .. } => {
            let cause = first_cause(exit);
            assert_eq!(cause["_tag"], "Die", "{cause}");
            assert_eq!(cause["defect"]["_tag"], "NotImplemented");
        }
        other => panic!("unexpected {other:?}"),
    }
    fx.shutdown().await;
}

#[tokio::test]
async fn the_lifecycle_stream_replays_welcome_then_ready_with_their_sequence() {
    let fx = Fixture::start().await;
    let (mut socket, mut client) = fx.connect(&fx.cookie, "").await;
    client
        .start("subscribeServerLifecycle", json!({}), CallKind::Stream)
        .unwrap();
    let welcome = chunk(&mut socket, &mut client).await;
    assert_eq!(welcome["type"], "welcome");
    assert_eq!(welcome["version"], 1);
    assert_eq!(welcome["sequence"], 1);
    assert_eq!(
        welcome["payload"]["cwd"].as_str().unwrap(),
        fx.temp.path().to_string_lossy()
    );
    assert_eq!(
        welcome["payload"]["environment"]["orchestrationProtocolVersion"],
        2
    );
    let ready = chunk(&mut socket, &mut client).await;
    assert_eq!(ready["type"], "ready");
    assert_eq!(ready["sequence"], 2);
    assert!(ready["payload"]["at"].is_string());
    fx.shutdown().await;
}

#[tokio::test]
async fn secret_references_are_masked_unless_the_socket_declares_the_capability() {
    let fx = Fixture::start().await;
    // A saved reference in the settings file; the server reads it on its next reload.
    let (mut socket, mut client) = fx.connect(&fx.cookie, "").await;
    client
        .start("subscribeServerConfig", json!({}), CallKind::Stream)
        .unwrap();
    chunk(&mut socket, &mut client).await;
    let reference = json!({"provider": "onepassword", "reference": "op://vault/item/field"});
    std::fs::write(
        fx.temp.path().join("userdata/settings.json"),
        json!({"providerInstances": {"antigravity_ref": {
            "driver": "antigravity", "config": {"apiKey": reference}
        }}})
        .to_string(),
    )
    .unwrap();
    // The watcher reports the new file on the stream that this plain client holds.
    let update = chunk(&mut socket, &mut client).await;
    assert_eq!(update["type"], "settingsUpdated");
    let masked =
        &update["payload"]["settings"]["providerInstances"]["antigravity_ref"]["config"]["apiKey"];
    assert_ne!(
        masked, &reference,
        "a plain client must not see the reference: {update}"
    );

    let (mut capable, mut capable_client) = fx
        .connect(
            &fx.cookie,
            "&clientCapabilities=other,settingsSecretReferences",
        )
        .await;
    let config = unary_ok(
        &mut capable,
        &mut capable_client,
        "server.getConfig",
        json!({}),
    )
    .await;
    assert_eq!(
        config["settings"]["providerInstances"]["antigravity_ref"]["config"]["apiKey"], reference,
        "{config}"
    );
    let plain = unary_ok(&mut socket, &mut client, "server.getConfig", json!({})).await;
    assert_ne!(
        plain["settings"]["providerInstances"]["antigravity_ref"]["config"]["apiKey"],
        reference
    );
    fx.shutdown().await;
}

#[test]
fn the_core_module_registers_what_it_declares() {
    let registry = t3_server::handlers::registry().unwrap();
    for tag in t3_server::handlers::core::REGISTERED {
        assert_eq!(registry.disposition(tag), Some(true), "{tag}");
    }
    assert_eq!(
        registry.disposition("server.runStorageCleanup"),
        Some(false)
    );
}
