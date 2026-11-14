#![allow(clippy::unwrap_used)]
mod common;
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use t3_contracts::{provider_policy::RuntimeMode, server::ServerProviderState};
use t3_provider::{
    catalog::{self, Manifest},
    environment::Platform,
    grok::*,
    instances::InstanceSpec,
    native_provider::{NativeProviderContext, ProcessExecutableProbe},
    sensitive::*,
    service::ProviderInstanceService,
    transport::*,
};
struct NoHttp;
#[async_trait]
impl HttpTransport for NoHttp {
    async fn send(&self, _: HttpRequest) -> ServiceResult<HttpResponse> {
        panic!("status fixture has no persisted Grok billing credentials")
    }
}
fn provider(fixture: &common::Fixture, enabled: bool, api_key: bool) -> GrokProvider {
    GrokProvider::new(
        Arc::new(InstanceSpec {
            instance_id: "grok-work".into(),
            driver: "grok".into(),
            display_name: None,
            accent_color: None,
            enabled,
            config: json!({"binaryPath":fixture.binary(),"customModels":["custom"]}),
            variables: if api_key {
                serde_json::from_value(
                    json!([{"name":"XAI_API_KEY","value":"never-key-secret","sensitive":true}]),
                )
                .unwrap()
            } else {
                vec![]
            },
        }),
        NativeProviderContext {
            environment: fixture.environment.clone(),
            home: fixture.directory.path().into(),
            cwd: fixture.directory.path().into(),
            platform: Platform::current(),
            managed_mcp: false,
            server_version: "0.1.0".into(),
        },
        Arc::new(ProcessExecutableProbe),
        Arc::new(ProcessGrokMetadata),
        Arc::new(NoHttp),
        Arc::new(SystemClock),
        Arc::new(Manifest::bundled().unwrap()).into(),
    )
    .with_system_config_directory(fixture.directory.path().join("system-config"))
}
#[test]
fn grok_cli_and_initialize_catalogs_keep_native_identity_defaults_options_and_command_exclusions() {
    let cli = parse_models_output(
        "Not authenticated\nYou are logged in\n* grok-4-build (default)\n- grok-4-build\n- custom_MODEL\n",
    );
    assert_eq!(cli.authenticated, Some(true));
    assert_eq!(cli.models.len(), 2);
    assert_eq!(cli.models[0].name.0, "Grok 4 build");
    assert_eq!(cli.models[0].is_default, Some(Some(true)));
    let initialized = json!({"_meta":{"modelState":{"currentModelId":"grok-build","availableModels":[{"modelId":"grok-build","name":"Build","_meta":{"supportsReasoningEffort":true,"reasoningEffort":"high","reasoningEfforts":[{"value":"high","label":"High","default":true},{"value":"low","label":"Low"}]}},{"modelId":"grok-build","name":"Duplicate"}]},"availableCommands":[{"name":"review","description":"Review","input":{"hint":" file "}},{"name":"context","description":"No output"},{"name":"always-approve","description":"Unsafe toggle"},{"name":"compact","description":"Native compact"},{"name":"REVIEW","description":"Separate"}]}});
    let models = initialize_models(&initialized);
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].is_default, Some(Some(true)));
    let descriptors = catalog::descriptors(models[0].capabilities.as_ref().unwrap());
    assert_eq!(
        serde_json::to_value(descriptors).unwrap()[0]["options"][0]["id"],
        "low"
    );
    let commands = slash_commands(&initialized);
    assert_eq!(
        commands
            .iter()
            .map(|command| command.name.0.as_str())
            .collect::<Vec<_>>(),
        ["compact", "review", "REVIEW"]
    );
    assert_eq!(
        commands[0]
            .description
            .as_ref()
            .and_then(Option::as_ref)
            .unwrap()
            .0,
        "Native compact"
    );
    assert_eq!(spawn_args(None), ["agent", "stdio"]);
    assert_eq!(
        spawn_args(Some(RuntimeMode::Auto)),
        ["--permission-mode", "auto", "agent", "stdio"]
    );
    assert_eq!(
        spawn_args(Some(RuntimeMode::AutoAcceptEdits)),
        ["--permission-mode", "default", "agent", "stdio"]
    );
    assert_eq!(
        spawn_args(Some(RuntimeMode::FullAccess)),
        ["agent", "--always-approve", "stdio"]
    );
}
#[tokio::test]
async fn grok_status_rechecks_expired_login_and_reads_initialize_metadata_without_authenticating_or_creating_sessions()
 {
    let fixture = common::Fixture::new(
        json!({"protocol":"grok","version":"grok 1.0.52","expectedInitializeMeta":{"clientType":"extension"},"expectedEnvironment":{"MISE_OFFLINE":"1","GROK_DISABLE_AUTOUPDATER":"1"},"grokModels":["Not logged in\n- old","You are logged in\n- cli-model"],"inspect":{"skills":[{"name":"review","description":"Review","userInvocable":false,"source":{"path":"/recorded/skill","type":"project"}}]},"initialize":{"protocolVersion":1,"agentCapabilities":{},"authMethods":[],"_meta":{"modelState":{"currentModelId":"acp-model","availableModels":[{"modelId":"acp-model","name":"ACP model"}]},"availableCommands":[{"name":"review","description":"Review"}]}}}),
    );
    let source = provider(&fixture, true, false);
    assert_eq!(
        source.initial_snapshot().unwrap().status,
        ServerProviderState::Warning
    );
    assert!(fixture.calls().is_empty());
    let snapshot = source.snapshot(None, false).await.unwrap();
    let value = serde_json::to_value(&snapshot).unwrap();
    assert_eq!(snapshot.status, ServerProviderState::Ready);
    assert_eq!(value["auth"]["type"], "cached_token");
    assert_eq!(snapshot.models[0].slug.0, "acp-model");
    assert_eq!(snapshot.skills.len(), 1);
    assert!(!snapshot.skills[0].enabled);
    assert_eq!(snapshot.slash_commands.len(), 2);
    let calls = fixture.calls();
    assert_eq!(
        calls.iter().filter(|call| call == &&vec!["models"]).count(),
        2
    );
    assert!(calls.iter().any(|call| call == &["agent", "stdio"]));
    assert!(!calls.iter().flatten().any(|method| method == "session/new"
        || method == "authenticate"
        || method == "session/prompt"));
    source
        .snapshot(Some(fixture.directory.path().to_str().unwrap()), false)
        .await
        .unwrap();
    let after = fixture.calls();
    assert_eq!(after.len(), calls.len() + 1);
    assert_eq!(after.last().unwrap(), &["inspect", "--json"]);
}
#[tokio::test]
async fn grok_disabled_and_signed_out_states_do_not_infer_readiness_and_keys_override_cli_login_text()
 {
    for (enabled, key, models_exit, expected) in [
        (false, false, 0, "disabled"),
        (true, false, 0, "error"),
        (true, true, 0, "ready"),
        (true, false, 1, "ready"),
    ] {
        let fixture = common::Fixture::new(
            json!({"protocol":"grok","version":"grok 1.0.52","grokModels":"Not authenticated\n- model","modelsExit":models_exit,"initialize":{"protocolVersion":1,"agentCapabilities":{},"_meta":{"modelState":{"currentModelId":"model","availableModels":[{"modelId":"model","name":"Model"}]}}}}),
        );
        let value = serde_json::to_value(
            provider(&fixture, enabled, key)
                .snapshot(None, false)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(value["status"], expected);
        if !enabled {
            assert!(fixture.calls().is_empty());
        } else if key {
            assert_eq!(value["auth"]["type"], "api_key");
        } else if models_exit != 0 {
            assert_eq!(value["auth"]["status"], "unknown");
        } else {
            assert_eq!(value["auth"]["status"], "unauthenticated");
        }
    }
}
