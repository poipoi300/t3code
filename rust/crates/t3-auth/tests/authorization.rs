#![allow(clippy::unwrap_used)]
use serde_json::json;
use t3_auth::{Error, authorization::*};
use t3_contracts::{auth::AuthEnvironmentScope as S, rpc::*};
fn payload<M: RpcMethod>(value: serde_json::Value) -> serde_json::Value {
    let typed: M::Payload = serde_json::from_value(value).unwrap();
    serde_json::to_value(typed).unwrap()
}

#[test]
fn every_typescript_rpc_scope_matches_the_checked_in_source() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let rpc = std::fs::read_to_string(root.join("packages/contracts/src/rpc.ts")).unwrap();
    let orch =
        std::fs::read_to_string(root.join("packages/contracts/src/orchestrationV2.ts")).unwrap();
    let mut constants = std::collections::BTreeMap::new();
    let declaration = regex::Regex::new(r#"(\w+):\s*"([^"]+)""#).unwrap();
    for (name, source) in [("WS_METHODS", rpc), ("ORCHESTRATION_V2_WS_METHODS", orch)] {
        let body = source
            .split(&format!("export const {name} = {{"))
            .nth(1)
            .unwrap()
            .split("} as const")
            .next()
            .unwrap();
        for capture in declaration.captures_iter(body) {
            constants.insert(format!("{}.{}", name, &capture[1]), capture[2].to_owned());
        }
    }
    let mut actual = std::collections::BTreeMap::new();
    let declarations = regex::Regex::new(r"\[([\w.]+)\]: Auth(\w+)Scope").unwrap();
    let scopes: std::collections::BTreeMap<_, _> = [
        ("AccessRead", "access:read"),
        ("AccessWrite", "access:write"),
        ("EnvironmentMaintain", "environment:maintain"),
        ("SettingsWrite", "settings:write"),
        ("ProvidersManage", "providers:manage"),
        ("OrchestrationRead", "orchestration:read"),
        ("OrchestrationOperate", "orchestration:operate"),
        ("FilesystemRead", "filesystem:read"),
        ("FilesystemWrite", "filesystem:write"),
        ("PreviewOperate", "preview:operate"),
        ("RelayRead", "relay:read"),
        ("RelayWrite", "relay:write"),
        ("DiagnosticsRead", "diagnostics:read"),
        ("TerminalRead", "terminal:read"),
        ("TerminalOperate", "terminal:operate"),
        ("SourceControlWrite", "source-control:write"),
    ]
    .into();
    for path in [
        "packages/contracts/src/clientRpcPermissions.ts",
        "apps/server/src/auth/RpcAuthorization.ts",
    ] {
        let source = std::fs::read_to_string(root.join(path)).unwrap();
        let source = source.split("} as const").next().unwrap();
        for c in declarations.captures_iter(source) {
            actual.insert(constants[&c[1]].clone(), scopes[&c[2]].to_owned());
        }
    }
    let expected: std::collections::BTreeMap<_, _> = RPC_REQUIRED_SCOPES
        .iter()
        .map(|(m, s)| (m.to_string(), scope_name(*s)))
        .collect();
    assert_eq!(actual, expected);
    assert_eq!(RPC_REQUIRED_SCOPES.len(), 220);
}

#[test]
fn static_sensitive_scopes_and_unknown_method_errors() {
    for method in ["server.notRegistered", "toString", "constructor"] {
        assert_eq!(
            required_scope_for_rpc_method(method)
                .unwrap_err()
                .to_string(),
            format!("RPC method {method} has no declared authorization scope.")
        );
    }
    for (method, scope) in RPC_REQUIRED_SCOPES {
        let parsed = *scope;
        assert_eq!(required_scope_for_rpc_method(method).unwrap(), parsed);
    }
    for m in ["terminal.observe", "subscribeTerminalMetadata"] {
        assert_eq!(required_scope_for_rpc_method(m).unwrap(), S::TerminalRead);
    }
    for m in [
        "terminal.attach",
        "terminal.open",
        "terminal.write",
        "terminal.resize",
        "terminal.clear",
        "terminal.close",
        "preview.stopLocalServer",
    ] {
        assert_eq!(
            required_scope_for_rpc_method(m).unwrap(),
            S::TerminalOperate
        );
    }
    for m in ["subscribeVcsBranch", "vcs.refreshBranch"] {
        assert_eq!(
            required_scope_for_rpc_method(m).unwrap(),
            S::OrchestrationRead
        );
    }
    for m in [
        "scheduledTasks.listWebhookDeliveries",
        "scheduledTasks.getWebhookDelivery",
        "scheduledTasks.rotateWebhookToken",
        "server.prepareTriage",
    ] {
        assert_eq!(
            required_scope_for_rpc_method(m).unwrap(),
            S::OrchestrationOperate
        );
    }
    assert_eq!(
        required_scope_for_device_list(&json!({})).unwrap(),
        S::OrchestrationRead
    );
    for input in [
        json!({"retryHostId":"host"}),
        json!({"updateTool":"agent","inspectOnly":true}),
        json!({"updateTool":"hub"}),
    ] {
        assert_eq!(
            required_scope_for_device_list(&input).unwrap(),
            S::OrchestrationOperate
        );
    }
    assert!(
        authorize_rpc(
            &[S::OrchestrationRead],
            "device.list",
            &payload::<WsDeviceListRpc>(json!({"retryHostId":"host"}))
        )
        .is_err()
    );
    assert!(
        authorize_rpc(
            &[S::OrchestrationRead],
            "device.list",
            &payload::<WsDeviceListRpc>(json!({"retryHostId":"host","inspectOnly":true}))
        )
        .is_ok()
    );
    assert!(
        authorize_rpc(
            &[S::OrchestrationRead],
            "device.list",
            &payload::<WsDeviceListRpc>(json!({"updateTool":"agent","inspectOnly":true}))
        )
        .is_err()
    );
}

#[tokio::test]
async fn guards_deny_before_handlers_and_apply_at_each_call_including_stream_start() {
    let mut calls = 0;
    for scopes in [
        vec![S::OrchestrationRead],
        vec![S::OrchestrationRead, S::EnvironmentMaintain],
    ] {
        let result = execute_rpc(
            &scopes,
            "server.retryResourceTelemetry",
            &json!({}),
            async || {
                calls += 1;
                Ok(())
            },
        )
        .await;
        let expected = if scopes.contains(&S::EnvironmentMaintain) {
            S::DiagnosticsRead
        } else {
            S::EnvironmentMaintain
        };
        match result {
            Err(Error::Authorization(e)) => {
                assert_eq!(e.required_permission, Some(scope_name(expected)))
            }
            _ => panic!(),
        }
    }
    for method in ["subscribeVcsBranch", "subscribeVcsStatus"] {
        assert!(
            execute_rpc(&[], method, &json!({"cwd":"/repo"}), async || {
                calls += 1;
                Ok(())
            })
            .await
            .is_err()
        );
        execute_rpc(
            &[S::OrchestrationRead],
            method,
            &json!({"cwd":"/repo"}),
            async || {
                calls += 1;
                Ok(())
            },
        )
        .await
        .unwrap();
    }
    assert_eq!(calls, 2);
    assert!(
        execute_rpc(
            &[S::OrchestrationRead],
            "server.runStorageCleanup",
            &json!({}),
            async || {
                calls += 1;
                Ok(())
            }
        )
        .await
        .is_err()
    );
    assert_eq!(calls, 2);
}

#[test]
fn settings_guards_separate_provider_mutations_and_secret_provider_removal() {
    let mutation = json!({"operation":"remove","instanceId":"codex_work"});
    for input in [
        json!({"patch":{},"providerInstanceMutation":mutation}),
        json!({"patch":{"providerInstances":{}}}),
        json!({"patch":{"usageLimitSources":{}}}),
        json!({"patch":{"textGenerators":{}}}),
    ] {
        assert!(
            authorize_rpc(
                &[S::ProvidersManage],
                "server.updateSettings",
                &payload::<WsServerUpdateSettingsRpc>(input.clone())
            )
            .is_ok()
        );
        assert!(
            authorize_rpc(
                &[S::SettingsWrite],
                "server.updateSettings",
                &payload::<WsServerUpdateSettingsRpc>(input.clone())
            )
            .is_err()
        );
    }
    for input in [
        json!({"patch":{"defaultRuntimeMode":"full-access"},"providerInstanceMutation":mutation}),
        json!({"patch":{"secretProviders":{"personal":null}}}),
    ] {
        assert!(
            authorize_rpc(
                &[S::ProvidersManage],
                "server.updateSettings",
                &payload::<WsServerUpdateSettingsRpc>(input.clone())
            )
            .is_err()
        );
        assert!(
            authorize_rpc(
                &[S::SettingsWrite],
                "server.updateSettings",
                &payload::<WsServerUpdateSettingsRpc>(input.clone())
            )
            .is_err()
        );
        assert!(
            authorize_rpc(
                &[S::SettingsWrite, S::ProvidersManage],
                "server.updateSettings",
                &payload::<WsServerUpdateSettingsRpc>(input.clone())
            )
            .is_ok()
        );
    }
    assert_eq!(
        required_scopes_for_rpc_call(
            "server.updateSettings",
            &payload::<WsServerUpdateSettingsRpc>(json!({"patch":{}}))
        )
        .unwrap(),
        vec![S::SettingsWrite]
    );
    // Upstream a2edd281df: a settings grant cannot turn off T3 Connect device approval.
    let approval = payload::<WsServerUpdateSettingsRpc>(
        json!({"patch":{"requireConnectDeviceApproval":false}}),
    );
    assert!(authorize_rpc(&[S::SettingsWrite], "server.updateSettings", &approval).is_err());
    assert!(authorize_rpc(&[S::AccessWrite], "server.updateSettings", &approval).is_ok());
    assert_eq!(
        required_scopes_for_rpc_call(
            "server.updateSettings",
            &payload::<WsServerUpdateSettingsRpc>(
                json!({"patch":{"requireConnectDeviceApproval":true,"defaultRuntimeMode":"full-access"}})
            )
        )
        .unwrap(),
        vec![S::SettingsWrite, S::AccessWrite]
    );
    assert_eq!(
        required_scopes_for_rpc_call(
            "projects.mutate",
            &payload::<WsProjectsMutateRpc>(json!({"type":"project.update","commandId":"command","projectId":"project","scripts":[]}))
        )
        .unwrap(),
        vec![S::OrchestrationOperate, S::SettingsWrite]
    );
}

#[test]
fn thread_and_asset_guards_preserve_all_dynamic_requirements() {
    assert!(
        authorize_rpc(
            &[S::SourceControlWrite],
            "git.preparePullRequestThread",
            &payload::<WsGitPreparePullRequestThreadRpc>(
                json!({"mode":"worktree","threadId":"thread","cwd":"/repo","reference":"1"})
            )
        )
        .is_err()
    );
    for action in ["commit", "create_pr", "commit_push_pr"] {
        let input = payload::<WsGitRunStackedActionRpc>(
            json!({"cwd":"/repo","actionId":"action","action":action}),
        );
        assert!(authorize_rpc(&[S::SourceControlWrite], "git.runStackedAction", &input).is_ok());
        let input = payload::<WsGitRunStackedActionRpc>(
            json!({"cwd":"/repo","actionId":"action","action":action,"threadId":"thread"}),
        );
        assert_eq!(
            authorize_rpc(&[S::SourceControlWrite], "git.runStackedAction", &input).is_ok(),
            action == "commit"
        );
        assert!(
            authorize_rpc(
                &[S::SourceControlWrite, S::OrchestrationRead],
                "git.runStackedAction",
                &input
            )
            .is_ok()
        );
    }
    for tag in [
        "attachment",
        "native-app-icon",
        "workspace-file",
        "media-file",
        "draft-workspace-file",
    ] {
        let input = payload::<WsAssetsCreateUrlRpc>(
            json!({"resource":{"_tag":tag,"threadId":"thread","cwd":"/repo","path":"/file","attachmentId":"attachment","app":{"_tag":"app-id","appId":"app"}}}),
        );
        assert_eq!(
            authorize_rpc(&[S::OrchestrationRead], "assets.createUrl", &input).is_ok(),
            matches!(tag, "attachment" | "native-app-icon")
        );
    }
    match authorize_rpc(
        &[S::OrchestrationRead],
        "assets.createUrl",
        &payload::<WsAssetsCreateUrlRpc>(
            json!({"resource":{"_tag":"native-app-icon","app":{"_tag":"app-path","path":"/app"}}}),
        ),
    ) {
        Err(Error::Authorization(e)) => {
            assert_eq!(e.required_scope, S::OrchestrationRead);
            assert_eq!(e.required_permission.as_deref(), Some("filesystem:read"));
        }
        _ => panic!(),
    }
    assert!(validate_delegation(STANDARD_SCOPES, &[S::OrchestrationRead]).is_ok());
    for bad in [
        vec![],
        vec![S::OrchestrationRead, S::OrchestrationRead],
        vec![S::ReviewWrite],
    ] {
        assert!(validate_delegation(STANDARD_SCOPES, &bad).is_err());
    }
}
