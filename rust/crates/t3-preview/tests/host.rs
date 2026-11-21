#![allow(clippy::unwrap_used)]
use std::{path::PathBuf, sync::Arc};
use t3_preview::{
    host::{HostInputs, PreviewHost, RuntimeInputs},
    html::MemoryHtmlRecovery,
};
fn inputs(root: &std::path::Path) -> HostInputs {
    HostInputs {
        runtime: RuntimeInputs {
            node: Some(PathBuf::from("node")),
            module_root: root.into(),
            playwright_module: None,
            electron_run_as_node: false,
        },
        base_dir: root.join("base"),
        state_dir: root.join("state"),
        attachments_dir: root.join("attachments"),
        secrets_dir: root.join("secrets"),
        sandbox: true,
        desktop: Default::default(),
        recovery: Arc::new(MemoryHtmlRecovery::default()),
    }
}
#[tokio::test]
async fn startup_refuses_missing_runtime_and_missing_playwright_before_first_use() {
    let root = tempfile::tempdir().unwrap();
    let mut input = inputs(root.path());
    input.runtime.node = None;
    let error = PreviewHost::new(input).await.err().unwrap();
    assert_eq!(error.tag(), "PreviewHostConfigurationError");
    assert!(error.to_string().contains("Node runtime"));
    let mut input = inputs(root.path());
    input.runtime.node = Some(root.path().join("missing-node"));
    assert!(
        PreviewHost::new(input)
            .await
            .err()
            .unwrap()
            .to_string()
            .contains("Node runtime")
    );
    let mut input = inputs(root.path());
    input.runtime.playwright_module = Some(root.path().join("missing-playwright.cjs"));
    let error = PreviewHost::new(input).await.err().unwrap();
    assert_eq!(error.tag(), "PreviewHostConfigurationError");
    assert!(error.to_string().contains("Playwright"));
}
#[tokio::test]
async fn startup_resolves_from_package_anchor_and_checks_api_without_launching_browser() {
    let root = tempfile::tempdir().unwrap();
    let module = root.path().join("node_modules/playwright-core");
    std::fs::create_dir_all(&module).unwrap();
    std::fs::write(module.join("package.json"), r#"{"main":"index.cjs"}"#).unwrap();
    std::fs::write(module.join("index.cjs"),"const forbidden=()=>{throw Error('MUST NOT LAUNCH')};exports.chromium={launch:forbidden,launchPersistentContext:forbidden,connectOverCDP:forbidden};").unwrap();
    let blocked = inputs(root.path());
    std::fs::create_dir_all(&blocked.state_dir).unwrap();
    let obstruction = blocked.state_dir.join("server-browser");
    std::fs::write(&obstruction, "preserve").unwrap();
    let fallback = t3_preview::host::PreviewServices::start(blocked, false)
        .await
        .unwrap();
    assert!(
        fallback
            .preview
            .manager()
            .list(serde_json::from_value(serde_json::json!({"threadId":"thread"})).unwrap())
            .await
            .sessions
            .is_empty()
    );
    let error = fallback
        .preview
        .driver()
        .call(
            t3_preview::driver::BrowserCommand::ViewerAck {
                key: t3_preview::driver::TabKey::new("missing", "tab"),
                viewer_id: "none".into(),
                frame_id: 0,
            },
            std::time::Duration::from_secs(1),
        )
        .await
        .unwrap_err();
    assert_eq!(error.tag(), "PreviewAutomationExecutionError");
    fallback.preview.shutdown().await.unwrap();
    std::fs::remove_file(obstruction).unwrap();
    let host = PreviewHost::new(inputs(root.path())).await.unwrap();
    assert!(host.runtime.playwright_module.ends_with("index.cjs"));
    assert!(host.runtime.node.is_absolute());
    let mut electron = inputs(root.path());
    electron.runtime.electron_run_as_node = true;
    let electron = PreviewHost::new(electron).await.unwrap();
    assert_eq!(
        electron
            .runtime
            .environment
            .get(std::ffi::OsStr::new("ELECTRON_RUN_AS_NODE"))
            .and_then(Option::as_ref)
            .unwrap(),
        "1"
    );
    let generation = electron.preview.driver().prepare().await.unwrap();
    let response = electron
        .preview
        .driver()
        .call_on(
            generation,
            t3_preview::driver::BrowserCommand::ViewerAck {
                key: t3_preview::driver::TabKey::new("missing", "tab"),
                viewer_id: "none".into(),
                frame_id: 0,
            },
            std::time::Duration::from_secs(5),
        )
        .await
        .unwrap();
    assert_eq!(response, serde_json::json!({}));
    electron.preview.shutdown().await.unwrap();
    assert!(
        host.preview
            .manager()
            .list(serde_json::from_value(serde_json::json!({"threadId":"thread"})).unwrap())
            .await
            .sessions
            .is_empty()
    );
    std::fs::write(module.join("index.cjs"), "exports.chromium={};").unwrap();
    assert!(
        PreviewHost::new(inputs(root.path()))
            .await
            .err()
            .unwrap()
            .to_string()
            .contains("Playwright")
    );
}
