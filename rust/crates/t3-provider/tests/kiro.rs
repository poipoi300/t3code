#![allow(clippy::unwrap_used)]
mod common;
use serde_json::json;
use std::sync::Arc;
use t3_provider::{
    catalog::{self, Manifest},
    environment::Platform,
    instances::InstanceSpec,
    kiro::*,
    native_provider::{NativeProviderContext, ProcessExecutableProbe},
    service::ProviderInstanceService,
    transport::SystemClock,
};
fn provider(fixture: &common::Fixture, enabled: bool, key: bool) -> KiroProvider {
    KiroProvider::new(
        Arc::new(InstanceSpec {
            instance_id: "kiro-work".into(),
            driver: "kiro".into(),
            display_name: None,
            accent_color: None,
            enabled,
            config: json!({"binaryPath":fixture.binary(),"customModels":["custom"]}),
            variables: if key {
                serde_json::from_value(
                    json!([{"name":"KIRO_API_KEY","value":"never-kiro-key","sensitive":true}]),
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
        Arc::new(ProcessKiroMetadata),
        Arc::new(SystemClock),
        Arc::new(Manifest::bundled().unwrap()).into(),
    )
}
#[tokio::test]
async fn model_and_agent_probes_run_only_after_whoami_confirms_sign_in_even_when_a_key_is_configured()
 {
    for (enabled, key, exit, whoami, expected, probe) in [
        (false, false, 0, "{}", "disabled", false),
        (true, false, 1, "{\"account\":null}", "error", false),
        (true, true, 1, "unrecognized", "warning", false),
        (
            true,
            false,
            0,
            "{\"accountType\":\"IdentityCenter\",\"email\":\"kiro@example.invalid\"}\nProfile: work",
            "ready",
            true,
        ),
    ] {
        let fixture = common::Fixture::new(
            json!({"protocol":"kiro","version":"kiro-cli 2.32.0","whoami":whoami,"whoamiExit":exit,"expectedEnvironment":{"MISE_OFFLINE":"1"},"kiroModels":{"default_model":"claude-opus-5.5","models":[{"model_id":"claude-opus-5.5","model_name":"Opus"},{"model_id":"kimi-k3","model_name":"Kimi"}]},"kiroTemplate":{"configOptions":[{"id":"mode","type":"select","currentValue":"work","options":[{"value":"default","name":"Default"},{"value":"work","name":"Work","description":"Company agent"}]}]}}),
        );
        let source = provider(&fixture, enabled, key);
        let initial = source.initial_snapshot().unwrap();
        assert_eq!(initial.installed, enabled);
        assert!(fixture.calls().is_empty());
        let snapshot = source.snapshot(None, false).await.unwrap();
        let value = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(value["status"], expected);
        assert_eq!(
            value["setup"],
            json!({"canAuthenticate":false,"canInstall":false})
        );
        assert_eq!(snapshot.slash_commands[0].name.0, "compact");
        assert_eq!(value["supportsTextGeneration"], false);
        assert_eq!(value["versionAdvisory"]["canUpdate"], false);
        let calls = fixture.calls();
        if !enabled {
            assert!(calls.is_empty());
        }
        assert_eq!(
            calls
                .iter()
                .any(|call| call == &["chat", "--list-models", "--format", "json"]),
            probe
        );
        assert_eq!(
            calls.iter().any(|call| call == &["_kiro/config/template"]),
            probe
        );
        assert!(!calls.iter().flatten().any(|method| method == "session/new"
            || method == "authenticate"
            || method == "session/prompt"));
        if probe {
            assert!(calls.iter().any(|call| call == ACP_ARGS));
            assert_eq!(default_model_id(&snapshot.models), "claude-opus-5.5");
            assert_eq!(snapshot.models[0].slug.0, "default");
            for model in &snapshot.models {
                assert!(
                    catalog::descriptors(model.capabilities.as_ref().unwrap())
                        .iter()
                        .any(|descriptor| catalog::option_id(descriptor) == "mode")
                );
            }
            assert_eq!(value["auth"]["email"], "kiro@example.invalid");
        } else if enabled {
            assert!(calls.len() <= 2);
        }
        assert!(source.maintenance(false).await.unwrap().update.is_none());
    }
}
