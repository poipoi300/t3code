#![allow(clippy::unwrap_used)]
use serde_json::json;
use std::{path::PathBuf, time::Duration};
use t3_preview::{
    BrowserDriver, PlaywrightDriver,
    driver::{BrowserCommand, PlaywrightOptions, TabKey},
};
#[tokio::test]
async fn driver_can_open_resize_navigate_and_close_through_real_worker_with_fake_playwright() {
    exercise_worker("worker-fixture.cjs").await;
}
#[tokio::test]
async fn unknown_and_malformed_events_do_not_disconnect_valid_worker_requests() {
    exercise_worker("events-fixture.cjs").await;
}
async fn exercise_worker(module: &str) {
    let root = tempfile::tempdir().unwrap();
    let driver = PlaywrightDriver::start(PlaywrightOptions {
        node: "node".into(),
        cwd: env!("CARGO_MANIFEST_DIR").into(),
        executable: root.path().join("never-launch-chrome"),
        profiles_dir: root.path().join("profiles"),
        downloads_dir: root.path().join("downloads"),
        attachments_dir: root.path().join("attachments"),
        playwright_module: Some(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("driver/tests")
                .join(module),
        ),
        sandbox: true,
    })
    .await
    .unwrap();
    let mut events = driver.subscribe();
    let snapshot=serde_json::from_value(json!({"threadId":"thread","tabId":"one","navStatus":{"_tag":"Idle"},"canGoBack":false,"canGoForward":false,"viewport":{"_tag":"fill"},"runtime":"server","updatedAt":"2026-01-01T00:00:00Z"})).unwrap();
    let opened = driver
        .call(
            BrowserCommand::Open {
                snapshot: Box::new(snapshot),
                desktop_endpoint: None,
                isolated: true,
                popup_id: None,
                load_error: None,
                crashed_url: None,
            },
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    assert_eq!(opened["viewport"], json!({"width":1280,"height":800}));
    assert_eq!(
        opened["_workerGeneration"].as_u64(),
        Some(driver.generation())
    );
    let key = TabKey::new("thread", "one");
    let navigated = driver
        .call(
            BrowserCommand::Operate {
                key: key.clone(),
                operation: "navigate".into(),
                input: json!({"url":"https://example.com/","readiness":"load"}),
                timeout_ms: 1000,
                generation: 0,
            },
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    assert_eq!(navigated["url"], "https://example.com/");
    assert_eq!(navigated["title"], "Fixture page");
    let t3_preview::driver::BrowserEvent::Worker { generation, .. } = events.recv().await.unwrap()
    else {
        panic!("missing worker generation")
    };
    assert_eq!(generation, driver.generation());
    let resized = driver
        .call(
            BrowserCommand::Operate {
                key: key.clone(),
                operation: "resize".into(),
                input: json!({"setting":{"_tag":"freeform","width":390,"height":844}}),
                timeout_ms: 1000,
                generation: 0,
            },
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    assert_eq!(resized["viewport"], json!({"width":390,"height":844}));
    driver
        .call(
            BrowserCommand::Close {
                key: key.clone(),
                desktop: false,
                unload: true,
            },
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    assert_eq!(
        driver
            .call(
                BrowserCommand::Operate {
                    key,
                    operation: "status".into(),
                    input: json!({}),
                    timeout_ms: 1000,
                    generation: 0
                },
                Duration::from_secs(5)
            )
            .await
            .unwrap_err()
            .tag(),
        "PreviewAutomationTabNotFoundError"
    );
    driver
        .call(BrowserCommand::Shutdown, Duration::from_secs(5))
        .await
        .unwrap();
    assert!(!driver.is_connected());
    assert!(std::iter::from_fn(|| events.try_recv().ok()).any(|event| {
        matches!(event, t3_preview::driver::BrowserEvent::Worker {generation,message}
            if generation == driver.generation() && matches!(*message,t3_preview::driver::BrowserEvent::Disconnected))
    }), "shutdown returned before the worker's output pipe closed");
    assert!(!root.path().join("profiles").exists());
}
