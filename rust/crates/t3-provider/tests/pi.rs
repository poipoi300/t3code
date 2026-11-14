#![allow(clippy::unwrap_used)]
mod common;
use serde_json::{Value, json};
use std::{path::Path, sync::Arc};
use t3_contracts::model::ProviderOptionDescriptor;
use t3_provider::{
    catalog::{Manifest, descriptors},
    environment::{Environment, Platform},
    instances::InstanceSpec,
    native_provider::{NativeProviderContext, ProcessExecutableProbe},
    pi::PiProvider,
    pi_catalog,
    pi_launch::*,
    pi_probe::ProcessPiProbe,
    service::ProviderInstanceService,
    transport::SystemClock,
};
fn provider(fixture: &common::Fixture, launch: &str) -> PiProvider {
    PiProvider::new(
        Arc::new(InstanceSpec {
            instance_id: "pi-work".into(),
            driver: "pi".into(),
            enabled: true,
            display_name: None,
            accent_color: None,
            config: json!({"binaryPath":fixture.binary(),"launchArgs":launch,"customModels":[]}),
            variables: vec![],
        }),
        NativeProviderContext {
            environment: fixture.environment.clone(),
            home: fixture.directory.path().into(),
            cwd: fixture.directory.path().into(),
            platform: Platform::current(),
            managed_mcp: false,
            server_version: "1".into(),
        },
        Arc::new(ProcessExecutableProbe),
        Arc::new(ProcessPiProbe),
        Arc::new(SystemClock),
        Arc::new(Manifest::bundled().unwrap()).into(),
    )
}
#[tokio::test]
async fn offline_rpc_catalog_distinguishes_extension_models_and_refines_only_unavailable_default_credentials()
 {
    let fixture = common::Fixture::new(
        json!({"protocol":"pi","version":"pi 0.84.1","piState":{"model":{"provider":"missing","id":"active"},"thinkingLevel":"max"},"piModels":{"models":[{"provider":"api/provider","id":"model/id","name":"Model","reasoning":true,"thinkingLevelMap":{"xhigh":true,"max":null},"serviceTiers":[{"id":"fast","name":"Fast","description":"Low latency"}]},{"provider":"api/provider","id":"model/id"}]},"piTextModels":{"models":[]},"piCommands":{"commands":[{"name":"compact","description":"Replaced"},{"name":"review","description":"Review"},{"name":"skill:work","source":"skill","sourceInfo":{"path":"skill/path","scope":"workspace"},"interface":{"displayName":"Work","shortDescription":"Do work"}}]}}),
    );
    let source = provider(&fixture, "--extension extension.ts --tools read");
    assert!(fixture.calls().is_empty());
    let snapshot = source.snapshot(None, false).await.unwrap();
    let value = serde_json::to_value(&snapshot).unwrap();
    assert_eq!(value["status"], "ready");
    assert_eq!(value["auth"]["status"], "authenticated");
    assert_eq!(value["supportsTextGeneration"], false);
    assert_eq!(snapshot.models.len(), 2);
    assert_eq!(snapshot.models[0].slug.0, "default");
    assert_eq!(value["models"][1]["supportsTextGeneration"], false);
    assert_eq!(value["skills"][0]["scope"], "project");
    assert_eq!(value["skills"][0]["displayName"], "Work");
    assert_eq!(
        value["slashCommands"][0]["input"]["hint"],
        "Optional instructions"
    );
    assert!(
        value["message"]
            .as_str()
            .unwrap()
            .contains("has no credentials configured")
    );
    let calls = fixture.calls();
    assert!(
        calls
            .iter()
            .filter(|args| args.iter().any(|arg| arg == "rpc"))
            .all(|args| args.iter().any(|arg| arg == "--offline")
                && args.iter().any(|arg| arg == "--no-session"))
    );
    assert!(calls.iter().any(|args| args
        == &[
            "auth",
            "check",
            "--provider",
            "missing",
            "--json",
            "--no-refresh"
        ]));
    let text = calls
        .iter()
        .find(|args| args.iter().any(|arg| arg == "--no-extensions"))
        .unwrap();
    assert!(
        !text
            .iter()
            .any(|arg| arg == "extension.ts" || arg == "--tools")
    );
    assert_eq!(text.last().unwrap(), "--no-tools");
    let snapshot = source
        .snapshot(Some(fixture.directory.path().to_str().unwrap()), false)
        .await
        .unwrap();
    assert_eq!(snapshot.workspace_snapshots.unwrap()[0].skills.len(), 1);
    assert_eq!(
        fixture
            .calls()
            .iter()
            .filter(|args| args == &&vec!["--version".to_owned()])
            .count(),
        1
    );
    let source = provider(&fixture, "--api-key secret-never-format");
    let snapshot = source.snapshot(None, false).await.unwrap();
    assert!(
        !serde_json::to_string(&snapshot)
            .unwrap()
            .contains("secret-never-format")
    );
    assert_eq!(
        fixture
            .calls()
            .iter()
            .filter(|args| args.first().is_some_and(|arg| arg == "auth"))
            .count(),
        1
    );
}
#[test]
fn launch_validation_keeps_credentials_private_and_restrictions_follow_user_flags() {
    for invalid in [
        "--mode=rpc",
        "--provider one",
        "--export",
        "-- resume",
        "@prompt",
        "-z",
        "--model",
    ] {
        assert!(resolve_launch_args(invalid).is_err(), "{invalid}");
    }
    let args=resolve_launch_args("--provider='api/provider' --model \"model/id\" --api-key=secret-never-format --custom=ok --extension ext.ts --tools write --exclude-tools read").unwrap();
    assert!(!format!("{args:?}").contains("secret-never-format"));
    let environment = Environment::from([
        ("T3_MCP_BEARER_TOKEN".into(), "secret-never-format".into()),
        ("T3_PI_MCP_EXTENSION_PATH".into(), "old".into()),
    ]);
    let launch = build_launch(LaunchOptions {
        binary: "pi",
        args: args.expose(),
        cwd: Path::new("workspace"),
        environment: &environment,
        session_path: None,
        ephemeral: true,
        short_lived: true,
        disable_extensions: true,
        disable_tools: true,
        extension_path: Some("injected.ts"),
        runtime_mode: Some("full-access"),
        mcp: None,
    });
    assert!(!format!("{launch:?}").contains("secret-never-format"));
    assert!(
        !launch
            .environment
            .expose()
            .contains_key("T3_MCP_BEARER_TOKEN")
    );
    assert_eq!(
        &launch.args.expose()[launch.args.expose().len() - 3..],
        ["--offline", "--no-extensions", "--no-tools"]
    );
    assert!(
        !launch
            .args
            .expose()
            .iter()
            .any(|arg| arg == "ext.ts" || arg == "injected.ts" || arg == "write")
    );
    assert_eq!(
        pi_catalog::resolve_model_slug(
            "api/provider/model/id",
            &json!([{"provider":"api/provider","id":"model/id"}])
        ),
        Some(("api/provider".into(), "model/id".into()))
    );
    assert_eq!(
        pi_catalog::compact_command(" /compact  guidance "),
        Some(Some("guidance".into()))
    );
    assert_eq!(pi_catalog::compact_command("/compact-extra"), None);
}
#[test]
fn thinking_levels_require_model_opt_in_and_active_default_clamps_to_available_levels() {
    let caps = pi_catalog::model_capabilities(
        &json!({"reasoning":true,"thinkingLevelMap":{"medium":null,"xhigh":"mapped","max":null},"serviceTiers":[{"id":"default"},{"id":"fast"},{"id":"fast"}]}),
        Some("max"),
    );
    assert!(
        matches!(&descriptors(&caps)[0],ProviderOptionDescriptor::Select(d)if d.current_value.is_none()&&d.options.last().unwrap().id=="xhigh"&&d.options.last().unwrap().is_default==Some(Some(true))&&!d.options.iter().any(|choice|choice.id=="max"||choice.id=="medium"))
    );
    assert!(
        matches!(&descriptors(&caps)[1],ProviderOptionDescriptor::Select(d)if d.options.len()==2&&d.current_value==Some(Some("default".into())))
    );
    assert!(descriptors(&pi_catalog::model_capabilities(&Value::Null, None)).is_empty());
}
