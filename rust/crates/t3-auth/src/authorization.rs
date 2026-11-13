use crate::{Error, Result};
use serde_json::Value;
use t3_contracts::auth::{
    AuthEnvironmentScope as Scope, AuthMcpClientAccess as Access, EnvironmentAuthorizationError,
    EnvironmentAuthorizationErrorTag,
};
#[path = "rpc_payloads.rs"]
mod payloads;
#[path = "rpc_scopes.rs"]
mod table;
pub use table::RPC_REQUIRED_SCOPES;

pub const STANDARD_SCOPES: &[Scope] = &[
    Scope::OrchestrationRead,
    Scope::OrchestrationOperate,
    Scope::SettingsWrite,
    Scope::ProvidersManage,
    Scope::EnvironmentMaintain,
    Scope::PreviewOperate,
    Scope::DiagnosticsRead,
    Scope::TerminalRead,
    Scope::TerminalOperate,
    Scope::SourceControlWrite,
    Scope::FilesystemRead,
    Scope::FilesystemWrite,
    Scope::RelayRead,
    Scope::AccessRead,
    Scope::AccessWrite,
    Scope::RelayWrite,
];
pub fn mcp_scopes(access: Access) -> Vec<Scope> {
    match access {
        Access::ReadOnly => vec![Scope::OrchestrationRead],
        Access::FullAccess => vec![
            Scope::OrchestrationRead,
            Scope::OrchestrationOperate,
            Scope::SettingsWrite,
            Scope::SourceControlWrite,
        ],
        _ => vec![Scope::OrchestrationRead, Scope::OrchestrationOperate],
    }
}
pub fn scope_name(scope: Scope) -> String {
    serde_json::to_value(scope)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}
pub fn legacy_parent(scope: Scope) -> Scope {
    match scope {
        Scope::FilesystemRead | Scope::DiagnosticsRead => Scope::OrchestrationRead,
        Scope::SettingsWrite
        | Scope::ProvidersManage
        | Scope::EnvironmentMaintain
        | Scope::PreviewOperate
        | Scope::SourceControlWrite
        | Scope::FilesystemWrite => Scope::OrchestrationOperate,
        Scope::TerminalRead => Scope::TerminalOperate,
        s => s,
    }
}
pub fn legacy_scopes(scopes: &[Scope]) -> Vec<Scope> {
    scopes
        .iter()
        .copied()
        .filter(|s| {
            matches!(
                s,
                Scope::OrchestrationRead
                    | Scope::OrchestrationOperate
                    | Scope::TerminalOperate
                    | Scope::ReviewWrite
                    | Scope::AccessRead
                    | Scope::AccessWrite
                    | Scope::RelayRead
                    | Scope::RelayWrite
            )
        })
        .collect()
}
pub fn scope_error(scope: Scope) -> Error {
    Error::Authorization(EnvironmentAuthorizationError {
        _tag: EnvironmentAuthorizationErrorTag::EnvironmentAuthorizationError,
        message: format!(
            "The authenticated token is missing required scope: {}.",
            scope_name(scope)
        ),
        required_scope: legacy_parent(scope),
        required_permission: Some(scope_name(scope)),
    })
}
pub fn require_scope(scopes: &[Scope], scope: Scope) -> Result<()> {
    if scopes.contains(&scope) {
        Ok(())
    } else {
        Err(scope_error(scope))
    }
}
pub fn required_scope_for_rpc_method(method: &str) -> Result<Scope> {
    table::required_scope(method).ok_or_else(|| {
        Error::UnknownRpc(format!(
            "RPC method {method} has no declared authorization scope."
        ))
    })
}

pub fn required_scope_for_device_list(input: &Value) -> Result<Scope> {
    if !input.is_object() {
        return Err(Error::InvalidPayload(
            "device.list payload must be an object".into(),
        ));
    }
    let _: t3_contracts::device::DeviceListInput = serde::Deserialize::deserialize(input)
        .map_err(|_| Error::InvalidPayload("device.list payload does not decode".into()))?;
    Ok(device_scope(input))
}
fn device_scope(input: &Value) -> Scope {
    if input
        .get("retryHostId")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty())
        || input
            .get("updateTool")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty())
    {
        Scope::OrchestrationOperate
    } else {
        Scope::OrchestrationRead
    }
}

/// Decode the method contract again at the execution boundary; malformed input never weakens scopes.
pub fn required_scopes_for_rpc_call(method: &str, input: &Value) -> Result<Vec<Scope>> {
    let base = required_scope_for_rpc_method(method)?;
    let canonical = payloads::decode(method, input)?;
    let input = canonical.as_ref().unwrap_or(input);
    if method == "device.list" {
        let inspect_only = input.get("inspectOnly").and_then(Value::as_bool) == Some(true);
        let updating = input
            .get("updateTool")
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty());
        if (!inspect_only || updating) && device_scope(input) == Scope::OrchestrationOperate {
            return Ok(vec![base, Scope::OrchestrationOperate]);
        }
    }
    if method == "server.retryResourceTelemetry" {
        return Ok(vec![Scope::EnvironmentMaintain, Scope::DiagnosticsRead]);
    }
    if method == "assets.createUrl" {
        let resource = input
            .get("resource")
            .ok_or_else(|| Error::InvalidPayload("resource is required".into()))?;
        let tag = resource
            .get("_tag")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::InvalidPayload("resource._tag is required".into()))?;
        let host = matches!(
            tag,
            "workspace-file" | "media-file" | "draft-workspace-file"
        ) || (tag == "native-app-icon"
            && resource.pointer("/app/_tag").and_then(Value::as_str) == Some("app-path"));
        return Ok(vec![if host {
            Scope::FilesystemRead
        } else {
            Scope::OrchestrationRead
        }]);
    }
    if method == "server.updateSettings" {
        let patch = input
            .get("patch")
            .and_then(Value::as_object)
            .ok_or_else(|| Error::InvalidPayload("patch is required".into()))?;
        let changes_providers = patch.keys().any(|k| provider_key(k));
        let changes_settings = patch.keys().any(|k| !provider_key(k))
            || patch
                .get("secretProviders")
                .and_then(Value::as_object)
                .is_some_and(|p| p.values().any(Value::is_null));
        let mutation = input.get("providerInstanceMutation").is_some();
        if patch.is_empty() && mutation {
            return Ok(vec![Scope::ProvidersManage]);
        }
        let mut scopes = Vec::new();
        if changes_settings || !changes_providers {
            scopes.push(Scope::SettingsWrite);
        }
        if changes_providers || mutation {
            scopes.push(Scope::ProvidersManage);
        }
        return Ok(scopes);
    }
    if method == "projects.mutate" {
        let mut scopes = vec![Scope::OrchestrationOperate];
        if input.get("type").and_then(Value::as_str) != Some("project.delete")
            && input.get("scripts").is_some()
        {
            scopes.push(Scope::SettingsWrite);
        }
        return Ok(scopes);
    }
    let guarded =
        t3_contracts::client_rpc_permissions::client_rpc_required_scopes(method, Some(input))
            .map_err(|_| Error::InvalidPayload(format!("{method} payload does not decode")))?;
    if !guarded.is_empty() {
        return Ok(guarded);
    }
    Ok(vec![base])
}
fn provider_key(key: &str) -> bool {
    matches!(
        key,
        "providers"
            | "providerInstances"
            | "usageLimitSources"
            | "textGenerators"
            | "secretProviders"
    )
}
pub fn authorize_rpc(scopes: &[Scope], method: &str, input: &Value) -> Result<()> {
    for required in required_scopes_for_rpc_call(method, input)? {
        require_scope(scopes, required)?;
    }
    Ok(())
}
/// Call for each execution, including the initial execution of a stream.
pub async fn execute_rpc<T>(
    scopes: &[Scope],
    method: &str,
    input: &Value,
    handler: impl AsyncFnOnce() -> Result<T>,
) -> Result<T> {
    authorize_rpc(scopes, method, input)?;
    handler().await
}
pub fn validate_delegation(scopes: &[Scope], delegated: &[Scope]) -> Result<()> {
    require_scope(scopes, Scope::AccessWrite)?;
    if delegated.is_empty()
        || delegated
            .iter()
            .enumerate()
            .any(|(i, s)| *s == Scope::ReviewWrite || delegated[..i].contains(s))
    {
        return Err(Error::InvalidScope);
    }
    for required in delegated {
        require_scope(scopes, *required)?;
    }
    Ok(())
}
