#![allow(clippy::unwrap_used)]
#[tokio::test]
async fn browser_helpers_run_against_fake_pages_and_real_refused_socks_connections() {
    let mut options = t3_process::ProcessOptions::new("node").args([
        "--test",
        "--test-concurrency=1",
        "driver/tests/page.test.mjs",
        "driver/tests/worker.test.mjs",
        "driver/tests/recording-frames.test.mjs",
        "driver/tests/proxy.test.mjs",
        "driver/tests/contexts.test.mjs",
        "driver/tests/resources.test.mjs",
        "driver/tests/desktop-viewport.test.mjs",
        "driver/tests/screencast.test.mjs",
    ]);
    options.cwd = Some(env!("CARGO_MANIFEST_DIR").into());
    options.timeout = Some(std::time::Duration::from_secs(30));
    options.check_exit = false;
    let output = t3_process::run(options).await.unwrap();
    assert_eq!(
        output.code,
        Some(0),
        "{}\n{}",
        output.stdout_text(),
        output.stderr_text()
    );
}
