#![cfg(feature = "local-services")]
#![allow(clippy::unwrap_used)]
mod c_support;
use c_support::*;
use serde_json::Value;

/// The product's real Effect RPC client (`WsRpcGroup`, `RpcClient.makeProtocolSocket`, the
/// frame-part joiner) against this server: a method answered with the structured
/// `NotImplemented` defect fails only that call, for unary calls and streams alike, and the
/// same session keeps serving. This is the evidence for the plan's compatibility question.
#[tokio::test]
async fn the_real_effect_client_handles_unsupported_methods_without_breaking_the_session() {
    let fx = Fixture::start().await;
    let ticket = fx.ticket(&fx.cookie, "rpc").await;
    let url = fx.ws_url(&format!("&wsTicket={ticket}&frameParts=1"));
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let output = tokio::task::spawn_blocking(move || {
        std::process::Command::new("node")
            .arg("rust/crates/t3-server/tests/c_effect_client.ts")
            .arg(url)
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
    assert!(
        output.status.success() && report["ok"] == true,
        "{report}\n{stderr}"
    );
    assert_eq!(report["unsupportedUnary"]["reasons"][0]["_tag"], "Die");
    assert_eq!(
        report["unsupportedUnary"]["reasons"][0]["value"]["_tag"],
        "NotImplemented"
    );
    assert_eq!(report["unsupportedStream"]["reasons"][0]["_tag"], "Die");
    for step in [
        "firstProbe",
        "probeAfterUnaryDefect",
        "probeAfterStreamDefect",
        "configSnapshot",
        "getConfig",
        "lastProbe",
    ] {
        assert_eq!(report[step], "ok", "{step}: {report}");
    }
    fx.shutdown().await;
}
