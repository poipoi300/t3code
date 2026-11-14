#![cfg(not(target_arch = "wasm32"))]
use serde_json::Value;
use std::{path::Path, process::Command};

#[test]
fn current_typescript_timeline_matches_the_ported_corpus() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("repository root");
    let output = Command::new("node")
        .current_dir(root)
        .args([
            "rust/crates/t3-client/src/thread_views/messages_timeline/export-tests.mjs",
            "--stdout",
        ])
        .output()
        .expect("Node is required for the TypeScript differential check");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let fresh: Value = serde_json::from_slice(&output.stdout).expect("TypeScript corpus");
    let saved: Value = serde_json::from_str(include_str!(
        "../src/thread_views/messages_timeline/typescript-cases.json"
    ))
    .expect("saved corpus");
    assert_eq!(
        fresh["hashes"], saved["hashes"],
        "timeline source or tests changed; regenerate and review the Rust parity cases"
    );
    assert_eq!(
        fresh, saved,
        "current TypeScript outputs differ from the ported corpus"
    );
}
