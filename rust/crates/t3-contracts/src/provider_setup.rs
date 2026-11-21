use crate::base_schemas::TrimmedNonEmptyString;
use crate::base_schemas::{EnvironmentId, IsoDateTime};
use crate::provider_instance::{ProviderDriverKind, ProviderInstanceId};
use crate::wire::JsNumber;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSetupInput {
    pub instance_id: ProviderInstanceId,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ProviderAuthMethodType {
    #[serde(rename = "agent")]
    Agent,
    #[serde(rename = "terminal")]
    Terminal,
    #[serde(rename = "credentials")]
    Credentials,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderAuthMethod {
    pub id: TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub account_email: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub can_remove: Option<bool>,
    pub name: TrimmedNonEmptyString,
    #[serde(with = "crate::wire::nullable")]
    pub description: Option<String>,
    pub r#type: ProviderAuthMethodType,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCredentialField {
    pub name: TrimmedNonEmptyString,
    pub label: TrimmedNonEmptyString,
    pub secret: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ProviderAuthInteraction {
    Browser {
        id: TrimmedNonEmptyString,
        #[serde(deserialize_with = "deserialize_sign_in_url")]
        url: TrimmedNonEmptyString,
        requires_consent: bool,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::optional"
        )]
        accepts_callback: Option<bool>,
    },
    DeviceCode {
        id: TrimmedNonEmptyString,
        #[serde(deserialize_with = "deserialize_sign_in_url")]
        url: TrimmedNonEmptyString,
        user_code: TrimmedNonEmptyString,
    },
    Terminal {
        id: TrimmedNonEmptyString,
        output: String,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::optional"
        )]
        output_offset: Option<u64>,
    },
    Credentials {
        id: TrimmedNonEmptyString,
        fields: Vec<ProviderCredentialField>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ProviderBrowserAuthAction {
    #[serde(rename = "accept")]
    Accept,
    #[serde(rename = "decline")]
    Decline,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderAuthTerminalSize {
    pub cols: u64,
    pub rows: u64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ProviderAuthResponse {
    Browser {
        action: ProviderBrowserAuthAction,
    },
    Terminal {
        data: String,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::optional"
        )]
        size: Option<ProviderAuthTerminalSize>,
    },
    Credentials {
        values: BTreeMap<TrimmedNonEmptyString, String>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ProviderAuthCallbackMode {
    #[serde(rename = "server")]
    Server,
    #[serde(rename = "client")]
    Client,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderAuthStartInput {
    pub instance_id: ProviderInstanceId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub method_id: Option<TrimmedNonEmptyString>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub return_url: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub callback_mode: Option<ProviderAuthCallbackMode>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderAuthRemoveMethodInput {
    pub instance_id: ProviderInstanceId,
    pub method_id: TrimmedNonEmptyString,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderAuthRespondInput {
    pub instance_id: ProviderInstanceId,
    pub flow_id: TrimmedNonEmptyString,
    pub interaction_id: TrimmedNonEmptyString,
    pub response: ProviderAuthResponse,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ProviderAuthPhase {
    #[serde(rename = "idle")]
    Idle,
    #[serde(rename = "starting")]
    Starting,
    #[serde(rename = "waiting")]
    Waiting,
    #[serde(rename = "verifying")]
    Verifying,
    #[serde(rename = "succeeded")]
    Succeeded,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "cancelled")]
    Cancelled,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ProviderCredentialOwner {
    #[serde(rename = "provider")]
    Provider,
    #[serde(rename = "t3")]
    T3,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderAuthState {
    pub instance_id: ProviderInstanceId,
    pub phase: ProviderAuthPhase,
    #[serde(with = "crate::wire::nullable")]
    pub flow_id: Option<TrimmedNonEmptyString>,
    #[serde(with = "authorization_url")]
    pub authorization_url: Option<String>,
    #[serde(with = "crate::wire::nullable")]
    pub expires_at: Option<IsoDateTime>,
    #[serde(with = "crate::wire::nullable")]
    pub message: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_auth_methods"
    )]
    pub methods: Option<Vec<ProviderAuthMethod>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::lenient"
    )]
    pub interaction: Option<Option<ProviderAuthInteraction>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::lenient"
    )]
    pub credential_owner: Option<ProviderCredentialOwner>,
}
fn deserialize_auth_methods<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<Vec<ProviderAuthMethod>>, D::Error> {
    crate::wire::deserialize_lenient_array(d).map(Some)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderAuthCompleteInput {
    pub instance_id: ProviderInstanceId,
    pub flow_id: TrimmedNonEmptyString,
    pub callback_url: TrimmedNonEmptyString,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderAuthCancelInput {
    pub instance_id: ProviderInstanceId,
    pub flow_id: TrimmedNonEmptyString,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ProviderInstallPhase {
    #[serde(rename = "idle")]
    Idle,
    #[serde(rename = "downloading")]
    Downloading,
    #[serde(rename = "extracting")]
    Extracting,
    #[serde(rename = "verifying")]
    Verifying,
    #[serde(rename = "succeeded")]
    Succeeded,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "cancelled")]
    Cancelled,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ProviderInstallSource {
    #[serde(rename = "managed")]
    Managed,
    #[serde(rename = "local")]
    Local,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInstallState {
    pub driver: ProviderDriverKind,
    #[serde(with = "crate::wire::nullable")]
    pub operation_id: Option<TrimmedNonEmptyString>,
    pub phase: ProviderInstallPhase,
    pub downloaded_bytes: u64,
    #[serde(with = "crate::wire::nullable")]
    pub total_bytes: Option<u64>,
    #[serde(with = "crate::wire::nullable")]
    pub version: Option<TrimmedNonEmptyString>,
    #[serde(with = "crate::wire::nullable")]
    pub installed_version: Option<TrimmedNonEmptyString>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub executable_path: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub source: Option<Option<ProviderInstallSource>>,
    pub can_remove: bool,
    #[serde(with = "crate::wire::nullable")]
    pub message: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInstallCancelInput {
    pub instance_id: ProviderInstanceId,
    pub operation_id: TrimmedNonEmptyString,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ProviderSetupErrorTag {
    #[serde(rename = "ProviderSetupError")]
    Error,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSetupError {
    #[serde(rename = "_tag")]
    pub _tag: ProviderSetupErrorTag,
    pub instance_id: ProviderInstanceId,
    pub operation: String,
    pub detail: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub cause: Option<Option<Value>>,
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatGptReconnectProfile {
    #[serde(deserialize_with = "reconnect_client_id")]
    pub client_id: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub subject: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub email: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "reconnect_redirect"
    )]
    pub redirect_uri: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub connection_label: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub sharing_enabled: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "id_token_hint"
    )]
    pub id_token_hint: Option<String>,
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatGptTransferredCredentials {
    pub client_id: String,
    #[serde(deserialize_with = "nonempty_token")]
    pub access_token: String,
    #[serde(deserialize_with = "refresh_token")]
    pub refresh_token: Option<String>,
    #[serde(deserialize_with = "nonempty_token")]
    pub id_token: String,
    pub issuer: String,
    pub expires_at: JsNumber,
    #[serde(with = "crate::wire::nullable")]
    pub earliest_refresh_at: Option<JsNumber>,
    pub scopes: Vec<String>,
    pub subject: String,
    #[serde(with = "crate::wire::nullable")]
    pub email: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatGptTransferredProfile {
    pub registration: ChatGptReconnectProfile,
    pub credentials: ChatGptTransferredCredentials,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatGptReconnectProfileInput {
    pub instance_id: ProviderInstanceId,
    pub method_id: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatGptImportProfileInput {
    pub instance_id: ProviderInstanceId,
    pub profile: ChatGptTransferredProfile,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatGptHandoffInput {
    pub instance_id: ProviderInstanceId,
    pub environment_id: EnvironmentId,
    #[serde(deserialize_with = "bounded_string::<_, 128>")]
    pub attempt_id: String,
    #[serde(deserialize_with = "bounded_string::<_, 4096>")]
    pub return_url: String,
    #[serde(with = "crate::wire::nullable")]
    pub profile: Option<ChatGptReconnectProfile>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "lowercase")]
pub enum ChatGptHandoffState {
    Auth { state: ProviderAuthState },
    Finished { profile: ChatGptTransferredProfile },
}

crate::redacted_debug!(ChatGptReconnectProfile { client_id, subject, email, redirect_uri, connection_label, sharing_enabled } redact { id_token_hint });
crate::redacted_debug!(ChatGptTransferredCredentials { client_id, issuer, expires_at, earliest_refresh_at, scopes, subject, email } redact { access_token, refresh_token, id_token });

fn bounded_string<'de, D: serde::Deserializer<'de>, const MAX: usize>(
    d: D,
) -> Result<String, D::Error> {
    let value = String::deserialize(d)?;
    if value.encode_utf16().count() > MAX {
        return Err(serde::de::Error::custom("string exceeds maximum length"));
    }
    Ok(value)
}

fn nonempty_token<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let token = bounded_string::<_, 16384>(d)?;
    if token.is_empty() {
        return Err(serde::de::Error::custom("token must not be empty"));
    }
    Ok(token)
}

fn refresh_token<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    let token = Option::<String>::deserialize(d)?;
    if token
        .as_ref()
        .is_some_and(|s| s.encode_utf16().count() > 16384)
    {
        return Err(serde::de::Error::custom("token exceeds maximum length"));
    }
    Ok(token)
}

fn id_token_hint<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    bounded_string::<_, 16384>(d).map(Some)
}

fn reconnect_client_id<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let id = String::deserialize(d)?;
    if !id.strip_prefix("oaiapp_").is_some_and(|suffix| {
        !suffix.is_empty()
            && suffix
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    }) {
        return Err(serde::de::Error::custom("invalid reconnect client ID"));
    }
    Ok(id)
}

fn reconnect_redirect<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    let uri = String::deserialize(d)?;
    let port = uri
        .strip_prefix("http://localhost:")
        .or_else(|| uri.strip_prefix("http://127.0.0.1:"))
        .and_then(|path| path.strip_suffix("/auth/callback"));
    if !port.is_some_and(|p| {
        (1..=5).contains(&p.len()) && !p.starts_with('0') && p.bytes().all(|b| b.is_ascii_digit())
    }) {
        return Err(serde::de::Error::custom("invalid reconnect redirect URI"));
    }
    Ok(Some(uri))
}

#[derive(Deserialize)]
#[serde(try_from = "TrimmedNonEmptyString")]
struct SignInUrl(TrimmedNonEmptyString);
impl TryFrom<TrimmedNonEmptyString> for SignInUrl {
    type Error = &'static str;
    fn try_from(value: TrimmedNonEmptyString) -> Result<Self, Self::Error> {
        let prefix = value.0.get(..7).unwrap_or_default();
        let secure_prefix = value.0.get(..8).unwrap_or_default();
        if value.0.encode_utf16().count() <= 16_384
            && (prefix.eq_ignore_ascii_case("http://")
                || secure_prefix.eq_ignore_ascii_case("https://"))
        {
            Ok(Self(value))
        } else {
            Err("expected an HTTP(S) sign-in URL of at most 16384 UTF-16 units")
        }
    }
}
fn deserialize_sign_in_url<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<TrimmedNonEmptyString, D::Error> {
    SignInUrl::deserialize(d).map(|v| v.0)
}
mod authorization_url {
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
        crate::wire::nullable::deserialize::<D, super::SignInUrl>(d).map(|v| v.map(|v| v.0.0))
    }
    pub use crate::wire::nullable::serialize;
}
