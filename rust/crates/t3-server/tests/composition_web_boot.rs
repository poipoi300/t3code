//! Package A: the TypeScript web client's real start-up sequence against this server.
//!
//! `composition_web_boot_client.ts` runs the product's client-runtime code in the web app's
//! order and classifies each step. The printed table is the server's to-do list for booting
//! the existing web client (`cargo test --test composition_web_boot -- --nocapture`).
#![cfg(feature = "local-services")]
#![allow(clippy::unwrap_used, clippy::expect_used)]
use serde_json::Value;
use t3_server::adapters::config::{ConfigInputs, ServerFlags};

/// Steps the web client cannot boot without; each must stay `served`.
const BOOT_CRITICAL: &[&str] = &[
    "GET /api/auth/session",
    "POST /api/auth/browser-session",
    "GET /.well-known/t3/environment",
    "server.probe",
    "subscribeServerLifecycle",
];

#[tokio::test]
async fn the_web_client_start_up_sequence_reports_each_step() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let server = t3_server::runtime::start(ConfigInputs {
        flags: ServerFlags {
            base_dir: Some(home.to_string_lossy().into_owned()),
            host: Some("127.0.0.1".into()),
            port: Some(0),
            ..Default::default()
        },
        env: Default::default(),
        home: home.into(),
        process_cwd: home.into(),
        bootstrap: None,
        headless: true,
        force_auto_bootstrap: Some(false),
        service_managed: false,
        cli_log_level: None,
        automatic_port: None,
        persisted_observability: Default::default(),
        static_dir: None,
    })
    .await
    .unwrap();
    let base = format!("http://{}/", server.address);
    let credential = server.pairing_credential.clone();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let output = tokio::task::spawn_blocking(move || {
        std::process::Command::new("node")
            .arg("rust/crates/t3-server/tests/composition_web_boot_client.ts")
            .args([base, credential])
            .current_dir(root)
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report: Value = serde_json::from_str(stdout.lines().last().unwrap_or("null"))
        .unwrap_or_else(|_| panic!("no report\nstdout: {stdout}\nstderr: {stderr}"));
    let steps = report["steps"].as_array().cloned().unwrap_or_default();

    println!("web client start-up against the Rust server:");
    for s in &steps {
        let missing = s["missing"]
            .as_str()
            .map(|m| format!(" (missing: {m})"))
            .unwrap_or_default();
        println!(
            "{:>2}. {:<50} {:<11} {}{missing}",
            s["step"],
            s["call"].as_str().unwrap_or(""),
            s["status"].as_str().unwrap_or(""),
            s["detail"].as_str().unwrap_or("")
        );
    }
    let todo: Vec<_> = steps.iter().filter(|s| s["status"] != "served").collect();
    println!("to do: {} of {} steps", todo.len(), steps.len());
    println!(
        "RPC methods served: {} of {}",
        server.rpc_methods_served,
        t3_contracts::rpc::WS_METHODS.len()
    );

    assert!(
        report["fatal"].is_null(),
        "script aborted: {}\n{stderr}",
        report["fatal"]
    );
    for s in &steps {
        assert_ne!(s["status"], "failed", "a step failed outright: {s}");
        if BOOT_CRITICAL.contains(&s["call"].as_str().unwrap_or("")) {
            assert_eq!(s["status"], "served", "boot-critical step regressed: {s}");
        }
    }
    assert!(steps.len() >= 10, "the sequence stopped early: {report}");
    server.shutdown().await.unwrap();
}
