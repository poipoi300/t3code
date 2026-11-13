//! Secret-bearing contract types never print their secrets through `Debug`.

use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::{collections::BTreeMap, fmt::Debug, fs, path::Path};
use t3_contracts::{
    auth::*, desktop_bootstrap::*, guest_share::*, ipc::*, peer_link::*, provider_instance::*,
    provider_setup::*, relay::*, scheduled_task::*, secret_request::*, server::*,
    settings_secret_value::*,
};

const MARKER: &str = "MARKER-SECRET-7f3a";

fn fixtures() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures"))
}

fn first_sample(schema: &str) -> Option<Value> {
    let manifest: Value =
        serde_json::from_str(&fs::read_to_string(fixtures().join("manifest.json")).ok()?).ok()?;
    let file = manifest["samples"]
        .as_array()?
        .iter()
        .find(|entry| entry["schema"] == schema)?["file"]
        .as_str()?
        .to_owned();
    let text = fs::read_to_string(fixtures().join(file)).ok()?;
    serde_json::from_str(text.lines().next()?).ok()
}

fn camel(snake: &str) -> String {
    let mut out = String::new();
    let mut upper = false;
    for c in snake.chars() {
        if c == '_' {
            upper = true;
        } else if upper {
            out.extend(c.to_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// Loads `T` from its first golden sample (or `fallback`) with every secret
/// field set to the marker, then checks `Debug` hides it.
fn check<T: DeserializeOwned + Debug>(schema: &str, secrets: &[&str], fallback: Value) {
    let mut sample = first_sample(schema).unwrap_or(fallback);
    let object = sample
        .as_object_mut()
        .unwrap_or_else(|| panic!("{schema}: sample is not an object"));
    for secret in secrets {
        // Some OAuth bodies keep snake_case keys; the unused spelling is ignored on decode.
        object.insert((*secret).to_owned(), json!(MARKER));
        object.insert(camel(secret), json!(MARKER));
    }
    let value: T = serde_json::from_value(sample.clone())
        .unwrap_or_else(|error| panic!("{schema}: {error}: {sample}"));
    let printed = format!("{value:?}");
    assert!(
        serde_json::to_string(&sample).is_ok_and(|text| text.contains(MARKER)),
        "{schema}: marker not loaded"
    );
    assert!(
        !printed.contains(MARKER),
        "{schema} leaks a secret: {printed}"
    );
    assert!(printed.contains("[REDACTED]"), "{schema}: {printed}");
}

#[test]
fn secret_fields_are_redacted() {
    check::<AuthBrowserSessionRequest>("AuthBrowserSessionRequest", &["credential"], json!({}));
    check::<AuthTokenExchangeRequest>("AuthTokenExchangeRequest", &["subject_token"], json!({}));
    check::<AuthAccessTokenResult>("AuthAccessTokenResult", &["access_token"], json!({}));
    check::<AuthWebSocketTicketResult>("AuthWebSocketTicketResult", &["ticket"], json!({}));
    check::<AuthPairingCredentialResult>("AuthPairingCredentialResult", &["credential"], json!({}));
    check::<AuthMcpRegisteredClient>("AuthMcpRegisteredClient", &["client_id"], json!({}));
    check::<AuthMcpAuthorizationRequest>(
        "AuthMcpAuthorizationRequest",
        &["state", "client_id"],
        json!({}),
    );
    check::<AuthMcpApprovalDetails>("AuthMcpApprovalDetails", &["csrf_token"], json!({}));
    check::<AuthMcpApprovalRedirect>("AuthMcpApprovalRedirect", &["redirect_to"], json!({}));
    check::<AuthMcpApprovalPairingCode>(
        "AuthMcpApprovalPairingCode",
        &["code"],
        json!({"_tag": "pairing-code", "access": "full-access"}),
    );
    check::<AuthMcpApprovalBrowserSession>(
        "AuthMcpApprovalBrowserSession",
        &["csrf_token"],
        json!({"_tag": "browser-session", "access": "full-access"}),
    );
    check::<AuthMcpTokenRequest>(
        "AuthMcpTokenRequest",
        &["code", "code_verifier", "client_id"],
        json!({}),
    );
    check::<AuthMcpTokenResult>("AuthMcpTokenResult", &["access_token"], json!({}));
    check::<DesktopBackendBootstrap>(
        "DesktopBackendBootstrap",
        &["desktop_bootstrap_token", "desktop_bootstrap_secret"],
        json!({}),
    );
    check::<GuestRedeemRequest>("GuestRedeemRequest", &["credential"], json!({}));
    check::<DesktopEnvironmentBootstrapSchema>(
        "DesktopEnvironmentBootstrapSchema",
        &["bootstrap_token"],
        json!({}),
    );
    check::<DesktopSshBearerBootstrapInputSchema>(
        "DesktopSshBearerBootstrapInputSchema",
        &["credential"],
        json!({}),
    );
    check::<DesktopSshBearerRequestInputSchema>(
        "DesktopSshBearerRequestInputSchema",
        &["bearer_token"],
        json!({}),
    );
    check::<DesktopSshEnvironmentBootstrapSchema>(
        "DesktopSshEnvironmentBootstrapSchema",
        &["pairing_token"],
        json!({}),
    );
    check::<DesktopSshPasswordPromptResolutionInputSchema>(
        "DesktopSshPasswordPromptResolutionInputSchema",
        &["password"],
        json!({}),
    );
    check::<PeerLinkCreateInput>("PeerLinkCreateInput", &["pairing_code"], json!({}));
    check::<PeerLinkAnswerLink>(
        "PeerLinkAnswerLink",
        &["pairing_code"],
        json!({"type": "link", "urls": ["https://example.test"], "access": "full-access"}),
    );
    check::<ChatGptReconnectProfile>("ChatGptReconnectProfile", &["id_token_hint"], json!({}));
    check::<ChatGptTransferredCredentials>(
        "ChatGptTransferredCredentials",
        &["access_token", "refresh_token", "id_token"],
        json!({"clientId": "c", "issuer": "i", "expiresAt": 1, "earliestRefreshAt": null, "email": null, "scopes": [], "subject": "s"}),
    );
    check::<RelayDeviceRegistrationRequest>(
        "RelayDeviceRegistrationRequest",
        &["push_token", "push_to_start_token"],
        json!({}),
    );
    check::<RelayLiveActivityRegistrationRequest>(
        "RelayLiveActivityRegistrationRequest",
        &["activity_push_token"],
        json!({}),
    );
    check::<RelayManagedEndpointRuntimeConfig>(
        "RelayManagedEndpointRuntimeConfig",
        &["connector_token"],
        json!({}),
    );
    check::<RelayManagedEndpointRecoveryRequest>(
        "RelayManagedEndpointRecoveryRequest",
        &["proof"],
        json!({}),
    );
    check::<RelayManagedEndpointRecoveryRegistrationRequest>(
        "RelayManagedEndpointRecoveryRegistrationRequest",
        &["proof"],
        json!({}),
    );
    check::<RelayEnvironmentConfigRequest>(
        "RelayEnvironmentConfigRequest",
        &["environment_credential"],
        json!({}),
    );
    check::<RelayAgentActivityPublishRequest>(
        "RelayAgentActivityPublishRequest",
        &["proof"],
        json!({}),
    );
    check::<RelayEnvironmentLinkRequest>("RelayEnvironmentLinkRequest", &["proof"], json!({}));
    check::<RelayEnvironmentLinkResponse>(
        "RelayEnvironmentLinkResponse",
        &["environment_credential"],
        json!({}),
    );
    check::<RelayDpopAccessTokenRequest>(
        "RelayDpopAccessTokenRequest",
        &["subject_token"],
        json!({}),
    );
    check::<RelayDpopAccessTokenResponse>(
        "RelayDpopAccessTokenResponse",
        &["access_token"],
        json!({}),
    );
    check::<RelayBearerRequestHeaders>("RelayBearerRequestHeaders", &["authorization"], json!({}));
    check::<RelayDpopProofRequestHeaders>("RelayDpopProofRequestHeaders", &["dpop"], json!({}));
    check::<RelayDpopRequestHeaders>(
        "RelayDpopRequestHeaders",
        &["authorization", "dpop"],
        json!({}),
    );
    check::<RelayEnvironmentConnectResponse>(
        "RelayEnvironmentConnectResponse",
        &["credential"],
        json!({}),
    );
    check::<RelayCloudMintCredentialProofPayload>(
        "RelayCloudMintCredentialProofPayload",
        &["nonce"],
        json!({}),
    );
    check::<RelayCloudMintCredentialRequest>(
        "RelayCloudMintCredentialRequest",
        &["proof"],
        json!({}),
    );
    check::<RelayCloudEnvironmentHealthProofPayload>(
        "RelayCloudEnvironmentHealthProofPayload",
        &["nonce"],
        json!({}),
    );
    check::<RelayCloudEnvironmentHealthRequest>(
        "RelayCloudEnvironmentHealthRequest",
        &["proof"],
        json!({}),
    );
    check::<RelayEnvironmentMintResponseProofPayload>(
        "RelayEnvironmentMintResponseProofPayload",
        &["request_nonce", "credential"],
        json!({}),
    );
    check::<RelayEnvironmentMintResponse>(
        "RelayEnvironmentMintResponse",
        &["credential", "proof"],
        json!({}),
    );
    check::<RelayEnvironmentHealthResponseProofPayload>(
        "RelayEnvironmentHealthResponseProofPayload",
        &["request_nonce"],
        json!({}),
    );
    check::<RelayEnvironmentHealthResponse>(
        "RelayEnvironmentHealthResponse",
        &["proof"],
        json!({}),
    );
    check::<ScheduledTaskUpsertScheduleWebhookSignature>(
        "ScheduledTaskUpsertScheduleWebhookSignature",
        &["secret"],
        json!({"header": "x-signature", "encoding": "hex", "prefix": ""}),
    );
    check::<ServerSelfUpdateResult>(
        "ServerSelfUpdateResult",
        &["desktop_update_token"],
        json!({}),
    );
    check::<SecretRequestAnswer>("SecretRequestAnswer", &["secret"], json!({"type": "save"}));
    // Not from the golden sample: only sensitive values are hidden.
    check::<ProviderInstanceEnvironmentVariable>(
        "ProviderInstanceEnvironmentVariable (sensitive)",
        &["value"],
        json!({"name": "API_KEY", "sensitive": true}),
    );
    check::<ResolvedProviderInstanceEnvironmentVariable>(
        "ResolvedProviderInstanceEnvironmentVariable",
        &["value"],
        json!({"name": "API_KEY", "sensitive": true}),
    );
}

#[test]
fn secret_values_redact_only_what_is_secret() -> Result<(), serde_json::Error> {
    let literal: SettingsSecretValue = serde_json::from_value(json!(MARKER))?;
    assert_eq!(format!("{literal:?}"), "Literal([REDACTED])");
    let plain: ProviderInstanceEnvironmentVariable =
        serde_json::from_value(json!({"name": "PATH", "value": "/usr/bin", "sensitive": false}))?;
    assert!(format!("{plain:?}").contains("/usr/bin"));
    let referenced: ProviderInstanceEnvironmentVariable = serde_json::from_value(json!({
        "name": "API_KEY", "value": MARKER, "sensitive": false,
        "secret": {"provider": "vault", "reference": "key"}
    }))?;
    assert!(!format!("{referenced:?}").contains(MARKER));
    Ok(())
}

/// A string field whose name says it carries a secret must sit in a type
/// with a redacting `Debug`. Extend `NOT_SECRET` only with a reason.
#[test]
fn secret_named_string_fields_have_redacting_debug() {
    const NOT_SECRET: &[(&str, &str, &str)] = &[
        ("ipc.rs", "code", "KeyboardEvent.code"),
        ("resource_telemetry.rs", "code", "error code"),
        ("core.rs", "code", "provider error code"),
        (
            "provider_setup.rs",
            "client_id",
            "public OAuth client id of the ChatGPT app",
        ),
    ];
    let secret_name = |name: &str| {
        let last = name.rsplit('_').next().unwrap_or(name);
        matches!(
            last,
            "token"
                | "secret"
                | "password"
                | "credential"
                | "verifier"
                | "proof"
                | "nonce"
                | "authorization"
                | "ticket"
                | "dpop"
        ) || name.ends_with("token_hint")
            || matches!(
                name,
                "code" | "state" | "client_id" | "redirect_to" | "pairing_code"
            )
    };
    // `SettingsSecretValue` redacts its literal itself.
    let string_type = |ty: &str| {
        let base = ty.trim_end_matches(',').trim_end_matches('>');
        base.ends_with("String") || base.ends_with("Proof")
    };
    let mut redacted = Vec::new();
    let mut fields: Vec<(String, String, String)> = Vec::new();
    let src = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src"));
    let mut stack = vec![src.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
            let path = entry.map(|e| e.path()).unwrap_or_default();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let file = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_owned();
            let text = fs::read_to_string(&path).unwrap_or_default();
            let mut current = String::new();
            for line in text.lines() {
                let line = line.trim();
                if let Some(rest) = line.strip_prefix("crate::redacted_debug!(") {
                    redacted.push(
                        rest.split_whitespace()
                            .next()
                            .unwrap_or_default()
                            .to_owned(),
                    );
                }
                if let Some(rest) = line.strip_prefix("impl std::fmt::Debug for ") {
                    redacted.push(rest.trim_end_matches(" {").to_owned());
                }
                if let Some(rest) = line.strip_prefix("pub struct ") {
                    current = rest
                        .split(|c: char| !c.is_alphanumeric() && c != '_')
                        .next()
                        .unwrap_or_default()
                        .to_owned();
                } else if let Some(rest) = line.strip_prefix("pub ")
                    && let Some((name, ty)) = rest.split_once(": ")
                    && secret_name(name)
                    && string_type(ty)
                    && !NOT_SECRET.iter().any(|(f, n, _)| *f == file && *n == name)
                {
                    fields.push((file.clone(), current.clone(), name.to_owned()));
                }
            }
        }
    }
    let leaks: BTreeMap<_, _> = fields
        .into_iter()
        .filter(|(_, owner, _)| !redacted.contains(owner))
        .map(|(file, owner, name)| (format!("{file}:{owner}"), name))
        .collect();
    assert!(
        leaks.is_empty(),
        "secret fields with a plain Debug: {leaks:#?}"
    );
}
