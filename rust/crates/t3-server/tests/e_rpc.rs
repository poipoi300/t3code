#![cfg(all(
    feature = "local-services",
    feature = "workspace",
    feature = "terminal"
))]
#![allow(clippy::unwrap_used)]
mod e_common;
use e_common::*;
use serde_json::{Value, json};
use std::sync::atomic::Ordering;
use t3_rpc::{Cause, Exit};

fn cases(root: &std::path::Path) -> Vec<(&'static str, Value, bool)> {
    vec![
        (
            "projects.listEntries",
            json!({"cwd":root,"directoryPath":""}),
            false,
        ),
        ("projects.listIgnoredFiles", json!({"cwd":root}), false),
        (
            "projects.readFile",
            json!({"cwd":root,"relativePath":"hello.txt"}),
            false,
        ),
        (
            "projects.searchContents",
            json!({"cwd":root,"query":"hello","limit":10,"caseSensitive":false,"wholeWord":false,"useRegex":false}),
            false,
        ),
        (
            "projects.searchEntries",
            json!({"cwd":root,"query":"hello","limit":10}),
            false,
        ),
        (
            "projects.writeFile",
            json!({"cwd":root,"relativePath":"new.txt","contents":"blocked"}),
            false,
        ),
        (
            "filesystem.browse",
            json!({"partialPath":format!("{}/", root.display())}),
            false,
        ),
        (
            "filesystem.getMetadata",
            json!({"paths":[root.join("hello.txt")]}),
            false,
        ),
        (
            "assets.createUrl",
            json!({"resource":{"_tag":"draft-workspace-file","cwd":root,"path":"hello.txt"}}),
            false,
        ),
        (
            "assets.persistChatAttachments",
            json!({"threadId":"thread-e","messageId":"message-e","attachments":[]}),
            false,
        ),
        (
            "attachments.createUploadUrl",
            json!({"type":"file","name":"hello.txt","mimeType":"text/plain","sizeBytes":3}),
            false,
        ),
        (
            "attachments.delete",
            json!({"attachmentId":"pending-00000000-0000-0000-0000-000000000000-txt"}),
            false,
        ),
        (
            "terminal.open",
            json!({"threadId":"thread-e","terminalId":"term-1","cwd":root}),
            false,
        ),
        (
            "terminal.attach",
            json!({"threadId":"thread-e","terminalId":"term-1","cwd":root}),
            true,
        ),
        (
            "terminal.observe",
            json!({"threadId":"thread-e","terminalId":"term-1"}),
            true,
        ),
        (
            "terminal.write",
            json!({"threadId":"thread-e","terminalId":"term-1","data":"blocked\r"}),
            false,
        ),
        (
            "terminal.resize",
            json!({"threadId":"thread-e","terminalId":"term-1","cols":80,"rows":24}),
            false,
        ),
        (
            "terminal.clear",
            json!({"threadId":"thread-e","terminalId":"term-1"}),
            false,
        ),
        (
            "terminal.close",
            json!({"threadId":"thread-e","terminalId":"term-1"}),
            false,
        ),
        ("subscribeTerminalMetadata", json!({}), true),
    ]
}
#[tokio::test]
async fn every_owned_method_rejects_unauthenticated_unscoped_and_malformed_calls() {
    let fixture = Fixture::new().await;
    let scoped = fixture.scoped(&["settings:write"]).await;
    let (mut denied, mut denied_client) = fixture.socket(&scoped).await;
    let (mut admin, mut admin_client) = fixture.socket(&fixture.cookie).await;
    for (method, payload, stream) in cases(&fixture.root) {
        let unauthenticated = tokio_tungstenite::connect_async(&fixture.ws)
            .await
            .unwrap_err();
        match unauthenticated {
            tokio_tungstenite::tungstenite::Error::Http(response) => {
                assert_eq!(response.status(), 401, "{method}")
            }
            other => panic!("{method}: {other:?}"),
        }
        let exit = call(&mut denied, &mut denied_client, method, payload, stream).await;
        let permission = match method {
            "projects.writeFile" => "filesystem:write",
            "assets.persistChatAttachments"
            | "attachments.createUploadUrl"
            | "attachments.delete" => "orchestration:operate",
            "terminal.observe" | "subscribeTerminalMetadata" => "terminal:read",
            method if method.starts_with("terminal.") => "terminal:operate",
            _ => "filesystem:read",
        };
        match exit {
            Exit::Failure { cause } => assert!(
                matches!(&cause[..], [Cause::Fail { error }] if error["_tag"] == "EnvironmentAuthorizationError" && error["requiredPermission"] == permission),
                "{method}: {cause:?}"
            ),
            other => panic!("{method} accepted without its scope: {other:?}"),
        }
        let exit = call(&mut admin, &mut admin_client, method, Value::Null, stream).await;
        match exit {
            Exit::Failure { cause } => assert!(
                matches!(&cause[..], [Cause::Die { defect }] if defect["_tag"] == "InvalidPayload" && defect["method"] == method),
                "{method}: {cause:?}"
            ),
            other => panic!("{method} accepted malformed payload: {other:?}"),
        }
    }
    assert!(!fixture.root.join("new.txt").exists());
    let mut metadata = fixture.terminal.manager.subscribe_metadata();
    let snapshot = serde_json::to_value(metadata.recv().await.unwrap()).unwrap();
    assert_eq!(snapshot["terminals"], json!([]));
    drop(metadata);
    denied.close(None).await.unwrap();
    admin.close(None).await.unwrap();
    fixture.close().await;
}

#[tokio::test]
async fn filesystem_calls_use_fresh_authority_and_keep_paths_inside_workspace() {
    let fixture = Fixture::new().await;
    let (mut socket, mut client) = fixture.socket(&fixture.cookie).await;
    let read = success(
        call(
            &mut socket,
            &mut client,
            "projects.readFile",
            json!({"cwd":fixture.root,"relativePath":"hello.txt"}),
            false,
        )
        .await,
    );
    assert_eq!(read["contents"], "hello workspace\n");
    success(
        call(
            &mut socket,
            &mut client,
            "projects.writeFile",
            json!({"cwd":fixture.root,"relativePath":"new.txt","contents":"changed\n"}),
            false,
        )
        .await,
    );
    assert_eq!(
        std::fs::read_to_string(fixture.root.join("new.txt")).unwrap(),
        "changed\n"
    );
    let listed = success(
        call(
            &mut socket,
            &mut client,
            "projects.listEntries",
            json!({"cwd":fixture.root,"directoryPath":""}),
            false,
        )
        .await,
    );
    assert!(
        listed["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["path"] == "new.txt"),
        "{listed}"
    );
    std::fs::write(fixture.temp.path().join("outside.txt"), "private").unwrap();
    for path in ["../outside.txt", ".git/config"] {
        let result = call(
            &mut socket,
            &mut client,
            "projects.writeFile",
            json!({"cwd":fixture.root,"relativePath":path,"contents":"escape"}),
            false,
        )
        .await;
        assert!(matches!(result, Exit::Failure { .. }), "{path}: {result:?}");
    }
    assert_eq!(
        std::fs::read_to_string(fixture.temp.path().join("outside.txt")).unwrap(),
        "private"
    );
    fixture.allowed.store(false, Ordering::SeqCst);
    let result = call(
        &mut socket,
        &mut client,
        "projects.readFile",
        json!({"cwd":fixture.root,"relativePath":"hello.txt"}),
        false,
    )
    .await;
    match result {
        Exit::Failure { cause } => assert!(
            matches!(&cause[..], [Cause::Fail { error }] if error["_tag"] == "ProjectReadFileError")
        ),
        other => panic!("revoked root accepted: {other:?}"),
    }
    socket.close(None).await.unwrap();
    fixture.close().await;
}
