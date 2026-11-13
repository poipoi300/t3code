use crate::environment::ExecutionEnvironmentDescriptor;
use crate::{
    auth::EncodedNumber,
    base_schemas::{DpopFailureReason, EnvironmentId, ThreadId, TrimmedNonEmptyString},
};
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayAgentAwarenessPlatform {
    #[serde(rename = "ios")]
    Ios,
    #[serde(rename = "android")]
    Android,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayAgentAwarenessPhase {
    #[serde(rename = "starting")]
    Starting,
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "waiting_for_approval")]
    WaitingForApproval,
    #[serde(rename = "waiting_for_input")]
    WaitingForInput,
    #[serde(rename = "completed")]
    Completed,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "stale")]
    Stale,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayAgentAwarenessPreferences {
    #[serde(rename = "liveActivitiesEnabled")]
    pub live_activities_enabled: bool,
    #[serde(rename = "notificationsEnabled")]
    pub notifications_enabled: bool,
    #[serde(rename = "notifyOnApproval")]
    pub notify_on_approval: bool,
    #[serde(rename = "notifyOnInput")]
    pub notify_on_input: bool,
    #[serde(rename = "notifyOnCompletion")]
    pub notify_on_completion: bool,
    #[serde(rename = "notifyOnFailure")]
    pub notify_on_failure: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayApnsEnvironment {
    #[serde(rename = "sandbox")]
    Sandbox,
    #[serde(rename = "production")]
    Production,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayDeviceRegistrationRequest {
    #[serde(rename = "deviceId")]
    pub device_id: TrimmedNonEmptyString,
    #[serde(rename = "label")]
    pub label: TrimmedNonEmptyString,
    #[serde(rename = "platform")]
    pub platform: RelayAgentAwarenessPlatform,
    #[serde(
        rename = "iosMajorVersion",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub ios_major_version: Option<Option<i64>>,
    #[serde(
        rename = "androidApiLevel",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub android_api_level: Option<Option<i64>>,
    #[serde(
        rename = "appVersion",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub app_version: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        rename = "bundleId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub bundle_id: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        rename = "apsEnvironment",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub aps_environment: Option<Option<RelayApnsEnvironment>>,
    #[serde(
        rename = "pushToken",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub push_token: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        rename = "pushToStartToken",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub push_to_start_token: Option<Option<TrimmedNonEmptyString>>,
    #[serde(rename = "preferences")]
    pub preferences: RelayAgentAwarenessPreferences,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayDeviceNotifications {
    #[serde(rename = "enabled")]
    pub enabled: bool,
    #[serde(rename = "notifyOnApproval")]
    pub notify_on_approval: bool,
    #[serde(rename = "notifyOnInput")]
    pub notify_on_input: bool,
    #[serde(rename = "notifyOnCompletion")]
    pub notify_on_completion: bool,
    #[serde(rename = "notifyOnFailure")]
    pub notify_on_failure: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayDeviceLiveActivities {
    #[serde(rename = "enabled")]
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayClientDeviceRecord {
    #[serde(rename = "deviceId")]
    pub device_id: TrimmedNonEmptyString,
    #[serde(rename = "label")]
    pub label: TrimmedNonEmptyString,
    #[serde(rename = "platform")]
    pub platform: RelayAgentAwarenessPlatform,
    #[serde(rename = "iosMajorVersion", with = "crate::wire::nullable")]
    pub ios_major_version: Option<i64>,
    #[serde(
        rename = "androidApiLevel",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub android_api_level: Option<Option<i64>>,
    #[serde(rename = "appVersion", with = "crate::wire::nullable")]
    pub app_version: Option<TrimmedNonEmptyString>,
    #[serde(rename = "notifications")]
    pub notifications: RelayDeviceNotifications,
    #[serde(rename = "liveActivities")]
    pub live_activities: RelayDeviceLiveActivities,
    #[serde(rename = "updatedAt")]
    pub updated_at: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayIosPlatform {
    #[serde(rename = "ios")]
    Ios,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayClientDeviceRecordV1 {
    #[serde(rename = "deviceId")]
    pub device_id: TrimmedNonEmptyString,
    #[serde(rename = "label")]
    pub label: TrimmedNonEmptyString,
    #[serde(rename = "platform")]
    pub platform: RelayIosPlatform,
    #[serde(rename = "iosMajorVersion")]
    pub ios_major_version: i64,
    #[serde(
        rename = "androidApiLevel",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub android_api_level: Option<Option<i64>>,
    #[serde(rename = "appVersion", with = "crate::wire::nullable")]
    pub app_version: Option<TrimmedNonEmptyString>,
    #[serde(rename = "notifications")]
    pub notifications: RelayDeviceNotifications,
    #[serde(rename = "liveActivities")]
    pub live_activities: RelayDeviceLiveActivities,
    #[serde(rename = "updatedAt")]
    pub updated_at: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayListDevicesResponse {
    #[serde(rename = "devices")]
    pub devices: Vec<RelayClientDeviceRecord>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayListDevicesResponseV1 {
    #[serde(rename = "devices")]
    pub devices: Vec<RelayClientDeviceRecordV1>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayLiveActivityRegistrationRequest {
    #[serde(rename = "deviceId")]
    pub device_id: TrimmedNonEmptyString,
    #[serde(rename = "activityPushToken")]
    pub activity_push_token: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayDeviceUnregistrationParams {
    #[serde(rename = "deviceId")]
    pub device_id: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayAgentActivityState {
    #[serde(rename = "environmentId")]
    pub environment_id: EnvironmentId,
    #[serde(rename = "threadId")]
    pub thread_id: ThreadId,
    #[serde(rename = "projectTitle")]
    pub project_title: TrimmedNonEmptyString,
    #[serde(rename = "threadTitle")]
    pub thread_title: TrimmedNonEmptyString,
    #[serde(rename = "phase")]
    pub phase: RelayAgentAwarenessPhase,
    #[serde(rename = "headline")]
    pub headline: TrimmedNonEmptyString,
    #[serde(
        rename = "detail",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub detail: Option<Option<TrimmedNonEmptyString>>,
    #[serde(rename = "modelTitle")]
    pub model_title: TrimmedNonEmptyString,
    #[serde(rename = "updatedAt")]
    pub updated_at: TrimmedNonEmptyString,
    #[serde(rename = "deepLink")]
    pub deep_link: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayAgentActivityAggregateRow {
    #[serde(rename = "environmentId")]
    pub environment_id: EnvironmentId,
    #[serde(rename = "threadId")]
    pub thread_id: ThreadId,
    #[serde(rename = "projectTitle")]
    pub project_title: TrimmedNonEmptyString,
    #[serde(rename = "threadTitle")]
    pub thread_title: TrimmedNonEmptyString,
    #[serde(rename = "modelTitle")]
    pub model_title: TrimmedNonEmptyString,
    #[serde(rename = "phase")]
    pub phase: RelayAgentAwarenessPhase,
    #[serde(rename = "status")]
    pub status: TrimmedNonEmptyString,
    #[serde(rename = "updatedAt")]
    pub updated_at: TrimmedNonEmptyString,
    #[serde(rename = "deepLink")]
    pub deep_link: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayAgentActivityAggregateState {
    #[serde(rename = "title")]
    pub title: TrimmedNonEmptyString,
    #[serde(rename = "subtitle")]
    pub subtitle: TrimmedNonEmptyString,
    #[serde(rename = "activeCount")]
    pub active_count: u64,
    #[serde(rename = "updatedAt")]
    pub updated_at: TrimmedNonEmptyString,
    #[serde(rename = "activities")]
    pub activities: Vec<RelayAgentActivityAggregateRow>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayManagedEndpointProviderKind {
    #[serde(rename = "manual")]
    Manual,
    #[serde(rename = "cloudflare_tunnel")]
    CloudflareTunnel,
    #[serde(rename = "t3_relay")]
    T3Relay,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayManagedEndpoint {
    #[serde(rename = "httpBaseUrl")]
    pub http_base_url: TrimmedNonEmptyString,
    #[serde(rename = "wsBaseUrl")]
    pub ws_base_url: TrimmedNonEmptyString,
    #[serde(rename = "providerKind")]
    pub provider_kind: RelayManagedEndpointProviderKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayManagedEndpointOrigin {
    #[serde(rename = "localHttpHost")]
    pub local_http_host: TrimmedNonEmptyString,
    #[serde(rename = "localHttpPort")]
    pub local_http_port: u16,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayManagedEndpointRuntimeConfig {
    #[serde(rename = "providerKind")]
    pub provider_kind: RelayManagedEndpointProviderKind,
    #[serde(rename = "connectorToken")]
    pub connector_token: TrimmedNonEmptyString,
    #[serde(
        rename = "tunnelId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tunnel_id: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        rename = "tunnelName",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tunnel_name: Option<Option<TrimmedNonEmptyString>>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayManagedEndpointRecoveryRequest {
    #[serde(rename = "cloudUserId")]
    pub cloud_user_id: TrimmedNonEmptyString,
    #[serde(rename = "origin")]
    pub origin: RelayManagedEndpointOrigin,
    #[serde(rename = "proof")]
    pub proof: TrimmedNonEmptyString,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayManagedEndpointRecoveryRegistrationRequest {
    #[serde(rename = "cloudUserId")]
    pub cloud_user_id: TrimmedNonEmptyString,
    #[serde(rename = "tunnelId")]
    pub tunnel_id: TrimmedNonEmptyString,
    #[serde(rename = "origin")]
    pub origin: RelayManagedEndpointOrigin,
    #[serde(rename = "proof")]
    pub proof: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayManagedEndpointRecoveryStatus {
    #[serde(rename = "ready")]
    Ready,
    #[serde(rename = "recovery_required")]
    RecoveryRequired,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayManagedEndpointRecoveryRegistrationResponse {
    #[serde(rename = "status")]
    pub status: RelayManagedEndpointRecoveryStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayManagedEndpointRecoveryResponse {
    #[serde(rename = "endpoint")]
    pub endpoint: RelayManagedEndpoint,
    #[serde(rename = "endpointRuntime")]
    pub endpoint_runtime: RelayManagedEndpointRuntimeConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayLinkProofRequest {
    #[serde(rename = "challenge")]
    pub challenge: String,
    #[serde(rename = "relayIssuer")]
    pub relay_issuer: String,
    #[serde(rename = "endpoint")]
    pub endpoint: RelayManagedEndpoint,
    #[serde(rename = "origin")]
    pub origin: RelayManagedEndpointOrigin,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayEnvironmentConfigRequest {
    #[serde(rename = "relayUrl")]
    pub relay_url: String,
    #[serde(
        rename = "relayIssuer",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub relay_issuer: Option<Option<String>>,
    #[serde(rename = "cloudUserId")]
    pub cloud_user_id: String,
    #[serde(rename = "environmentCredential")]
    pub environment_credential: String,
    #[serde(rename = "cloudMintPublicKey")]
    pub cloud_mint_public_key: String,
    #[serde(rename = "endpointRuntime", with = "crate::wire::nullable")]
    pub endpoint_runtime: Option<RelayManagedEndpointRuntimeConfig>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayManagedEndpointRegisterProofPayloadAction {
    #[serde(rename = "register")]
    Register,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayManagedEndpointRegisterProofPayload {
    #[serde(rename = "iss")]
    pub iss: TrimmedNonEmptyString,
    #[serde(rename = "aud")]
    pub aud: TrimmedNonEmptyString,
    #[serde(rename = "sub")]
    pub sub: TrimmedNonEmptyString,
    #[serde(rename = "jti")]
    pub jti: TrimmedNonEmptyString,
    #[serde(rename = "iat")]
    pub iat: i64,
    #[serde(rename = "exp")]
    pub exp: i64,
    #[serde(rename = "action")]
    pub action: RelayManagedEndpointRegisterProofPayloadAction,
    #[serde(rename = "environmentId")]
    pub environment_id: EnvironmentId,
    #[serde(rename = "cloudUserId")]
    pub cloud_user_id: TrimmedNonEmptyString,
    #[serde(rename = "tunnelId")]
    pub tunnel_id: TrimmedNonEmptyString,
    #[serde(rename = "origin")]
    pub origin: RelayManagedEndpointOrigin,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayManagedEndpointRecoverProofPayloadAction {
    #[serde(rename = "recover")]
    Recover,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayManagedEndpointRecoverProofPayload {
    #[serde(rename = "iss")]
    pub iss: TrimmedNonEmptyString,
    #[serde(rename = "aud")]
    pub aud: TrimmedNonEmptyString,
    #[serde(rename = "sub")]
    pub sub: TrimmedNonEmptyString,
    #[serde(rename = "jti")]
    pub jti: TrimmedNonEmptyString,
    #[serde(rename = "iat")]
    pub iat: i64,
    #[serde(rename = "exp")]
    pub exp: i64,
    #[serde(rename = "action")]
    pub action: RelayManagedEndpointRecoverProofPayloadAction,
    #[serde(rename = "environmentId")]
    pub environment_id: EnvironmentId,
    #[serde(rename = "cloudUserId")]
    pub cloud_user_id: TrimmedNonEmptyString,
    #[serde(rename = "origin")]
    pub origin: RelayManagedEndpointOrigin,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RelayManagedEndpointRecoveryProofPayload {
    RelayManagedEndpointRegisterProofPayload(RelayManagedEndpointRegisterProofPayload),
    RelayManagedEndpointRecoverProofPayload(RelayManagedEndpointRecoverProofPayload),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayAgentActivityPublishProofPayload {
    #[serde(rename = "iss")]
    pub iss: TrimmedNonEmptyString,
    #[serde(rename = "aud")]
    pub aud: TrimmedNonEmptyString,
    #[serde(rename = "sub")]
    pub sub: TrimmedNonEmptyString,
    #[serde(rename = "jti")]
    pub jti: TrimmedNonEmptyString,
    #[serde(rename = "iat")]
    pub iat: i64,
    #[serde(rename = "exp")]
    pub exp: i64,
    #[serde(rename = "environmentId")]
    pub environment_id: EnvironmentId,
    #[serde(rename = "threadId")]
    pub thread_id: ThreadId,
    #[serde(rename = "state", with = "crate::wire::nullable")]
    pub state: Option<RelayAgentActivityState>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayAgentActivityPublishRequest {
    #[serde(rename = "state", with = "crate::wire::nullable")]
    pub state: Option<RelayAgentActivityState>,
    #[serde(rename = "proof")]
    pub proof: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayEnvironmentLinkScope {
    #[serde(rename = "agent_activity_notifications")]
    AgentActivityNotifications,
    #[serde(rename = "managed_tunnels")]
    ManagedTunnels,
}

pub type RelayEnvironmentLinkProof = TrimmedNonEmptyString;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayEnvironmentLinkChallengeRequest {
    #[serde(rename = "notificationsEnabled")]
    pub notifications_enabled: bool,
    #[serde(rename = "liveActivitiesEnabled")]
    pub live_activities_enabled: bool,
    #[serde(rename = "managedTunnelsEnabled")]
    pub managed_tunnels_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayEnvironmentLinkChallengeResponse {
    #[serde(rename = "challenge")]
    pub challenge: TrimmedNonEmptyString,
    #[serde(rename = "expiresAt")]
    pub expires_at: TrimmedNonEmptyString,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayEnvironmentLinkRequest {
    #[serde(
        rename = "deviceId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub device_id: Option<Option<TrimmedNonEmptyString>>,
    #[serde(rename = "proof")]
    pub proof: RelayEnvironmentLinkProof,
    #[serde(rename = "notificationsEnabled")]
    pub notifications_enabled: bool,
    #[serde(rename = "liveActivitiesEnabled")]
    pub live_activities_enabled: bool,
    #[serde(rename = "managedTunnelsEnabled")]
    pub managed_tunnels_enabled: bool,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayEnvironmentLinkResponse {
    #[serde(rename = "ok")]
    pub ok: bool,
    #[serde(rename = "cloudUserId")]
    pub cloud_user_id: TrimmedNonEmptyString,
    #[serde(rename = "environmentId")]
    pub environment_id: EnvironmentId,
    #[serde(rename = "endpoint")]
    pub endpoint: RelayManagedEndpoint,
    #[serde(rename = "endpointRuntime", with = "crate::wire::nullable")]
    pub endpoint_runtime: Option<RelayManagedEndpointRuntimeConfig>,
    #[serde(rename = "relayIssuer")]
    pub relay_issuer: TrimmedNonEmptyString,
    #[serde(rename = "environmentCredential")]
    pub environment_credential: TrimmedNonEmptyString,
    #[serde(rename = "cloudMintPublicKey")]
    pub cloud_mint_public_key: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayEnvironmentLinkProofInvalidReason {
    #[serde(rename = "invalid_signature_or_scope")]
    InvalidSignatureOrScope,
    #[serde(rename = "descriptor_mismatch")]
    DescriptorMismatch,
    #[serde(rename = "replayed_nonce")]
    ReplayedNonce,
    #[serde(rename = "challenge_invalid")]
    ChallengeInvalid,
    #[serde(rename = "origin_not_allowed")]
    OriginNotAllowed,
    #[serde(rename = "endpoint_not_secure")]
    EndpointNotSecure,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayEnvironmentLinkFailedReason {
    #[serde(rename = "link_persistence_failed")]
    LinkPersistenceFailed,
    #[serde(rename = "credential_persistence_failed")]
    CredentialPersistenceFailed,
    #[serde(rename = "replay_persistence_failed")]
    ReplayPersistenceFailed,
    #[serde(rename = "internal_error")]
    InternalError,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayEnvironmentLinkUnavailableReason {
    #[serde(rename = "managed_endpoint_not_configured")]
    ManagedEndpointNotConfigured,
    #[serde(rename = "managed_endpoint_provisioning_failed")]
    ManagedEndpointProvisioningFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayEnvironmentEndpointUnavailableReason {
    #[serde(rename = "endpoint_request_failed")]
    EndpointRequestFailed,
    #[serde(rename = "endpoint_response_invalid")]
    EndpointResponseInvalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayEnvironmentEndpointFailure {
    #[serde(rename = "tunnel_down")]
    TunnelDown,
    #[serde(rename = "origin_unreachable")]
    OriginUnreachable,
    #[serde(rename = "dns")]
    Dns,
    #[serde(rename = "tls")]
    Tls,
    #[serde(rename = "refused")]
    Refused,
    #[serde(rename = "timeout")]
    Timeout,
    #[serde(rename = "invalid_response")]
    InvalidResponse,
    #[serde(rename = "unknown")]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayAgentActivityPublishProofInvalidReason {
    #[serde(rename = "invalid_signature_or_payload")]
    InvalidSignatureOrPayload,
    #[serde(rename = "replayed_nonce")]
    ReplayedNonce,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayAuthInvalidReason {
    #[serde(rename = "missing_bearer")]
    MissingBearer,
    #[serde(rename = "invalid_bearer")]
    InvalidBearer,
    #[serde(rename = "invalid_dpop")]
    InvalidDpop,
    #[serde(rename = "not_authorized")]
    NotAuthorized,
}

pub type RelayDpopFailureReason = DpopFailureReason;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayInternalErrorReason {
    #[serde(rename = "database_unavailable")]
    DatabaseUnavailable,
    #[serde(rename = "persistence_failed")]
    PersistenceFailed,
    #[serde(rename = "upstream_unavailable")]
    UpstreamUnavailable,
    #[serde(rename = "internal_error")]
    InternalError,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayEnvironmentConnectNotAuthorizedReason {
    #[serde(rename = "client_proof_key_thumbprint_missing")]
    ClientProofKeyThumbprintMissing,
    #[serde(rename = "environment_link_not_found")]
    EnvironmentLinkNotFound,
    #[serde(rename = "endpoint_provider_not_managed")]
    EndpointProviderNotManaged,
    #[serde(rename = "managed_endpoint_allocation_not_found")]
    ManagedEndpointAllocationNotFound,
    #[serde(rename = "managed_endpoint_base_domain_not_configured")]
    ManagedEndpointBaseDomainNotConfigured,
    #[serde(rename = "managed_endpoint_allocation_not_ready")]
    ManagedEndpointAllocationNotReady,
    #[serde(rename = "managed_endpoint_hostname_invalid")]
    ManagedEndpointHostnameInvalid,
    #[serde(rename = "managed_endpoint_mismatch")]
    ManagedEndpointMismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayAuthInvalidErrorCode {
    #[serde(rename = "auth_invalid")]
    AuthInvalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayAuthInvalidErrorTag {
    #[serde(rename = "RelayAuthInvalidError")]
    RelayAuthInvalidError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayAuthInvalidError {
    #[serde(rename = "_tag")]
    pub _tag: RelayAuthInvalidErrorTag,
    #[serde(rename = "code")]
    pub code: RelayAuthInvalidErrorCode,
    #[serde(rename = "reason")]
    pub reason: RelayAuthInvalidReason,
    #[serde(
        rename = "dpopFailureReason",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub dpop_failure_reason: Option<RelayDpopFailureReason>,
    #[serde(rename = "traceId")]
    pub trace_id: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayEnvironmentLinkProofExpiredErrorCode {
    #[serde(rename = "environment_link_proof_expired")]
    EnvironmentLinkProofExpired,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayEnvironmentLinkProofExpiredErrorTag {
    #[serde(rename = "RelayEnvironmentLinkProofExpiredError")]
    RelayEnvironmentLinkProofExpiredError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayEnvironmentLinkProofExpiredError {
    #[serde(rename = "_tag")]
    pub _tag: RelayEnvironmentLinkProofExpiredErrorTag,
    #[serde(rename = "code")]
    pub code: RelayEnvironmentLinkProofExpiredErrorCode,
    #[serde(rename = "traceId")]
    pub trace_id: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayEnvironmentLinkProofInvalidErrorCode {
    #[serde(rename = "environment_link_proof_invalid")]
    EnvironmentLinkProofInvalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayEnvironmentLinkProofInvalidErrorTag {
    #[serde(rename = "RelayEnvironmentLinkProofInvalidError")]
    RelayEnvironmentLinkProofInvalidError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayEnvironmentLinkProofInvalidError {
    #[serde(rename = "_tag")]
    pub _tag: RelayEnvironmentLinkProofInvalidErrorTag,
    #[serde(rename = "code")]
    pub code: RelayEnvironmentLinkProofInvalidErrorCode,
    #[serde(rename = "reason")]
    pub reason: RelayEnvironmentLinkProofInvalidReason,
    #[serde(rename = "traceId")]
    pub trace_id: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayEnvironmentConnectNotAuthorizedErrorCode {
    #[serde(rename = "environment_connect_not_authorized")]
    EnvironmentConnectNotAuthorized,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayEnvironmentConnectNotAuthorizedErrorTag {
    #[serde(rename = "RelayEnvironmentConnectNotAuthorizedError")]
    RelayEnvironmentConnectNotAuthorizedError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayEnvironmentConnectNotAuthorizedError {
    #[serde(rename = "_tag")]
    pub _tag: RelayEnvironmentConnectNotAuthorizedErrorTag,
    #[serde(rename = "code")]
    pub code: RelayEnvironmentConnectNotAuthorizedErrorCode,
    #[serde(
        rename = "reason",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub reason: Option<Option<RelayEnvironmentConnectNotAuthorizedReason>>,
    #[serde(rename = "traceId")]
    pub trace_id: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayEnvironmentEndpointUnavailableErrorCode {
    #[serde(rename = "environment_endpoint_unavailable")]
    EnvironmentEndpointUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayEnvironmentEndpointUnavailableErrorTag {
    #[serde(rename = "RelayEnvironmentEndpointUnavailableError")]
    RelayEnvironmentEndpointUnavailableError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayEnvironmentEndpointUnavailableError {
    #[serde(rename = "_tag")]
    pub _tag: RelayEnvironmentEndpointUnavailableErrorTag,
    #[serde(rename = "code")]
    pub code: RelayEnvironmentEndpointUnavailableErrorCode,
    #[serde(rename = "reason")]
    pub reason: RelayEnvironmentEndpointUnavailableReason,
    #[serde(
        rename = "endpointFailure",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub endpoint_failure: Option<RelayEnvironmentEndpointFailure>,
    #[serde(
        rename = "endpointStatus",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub endpoint_status: Option<EncodedNumber>,
    #[serde(rename = "traceId")]
    pub trace_id: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayEnvironmentEndpointTimedOutErrorCode {
    #[serde(rename = "environment_endpoint_timed_out")]
    EnvironmentEndpointTimedOut,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayEnvironmentEndpointTimedOutErrorTag {
    #[serde(rename = "RelayEnvironmentEndpointTimedOutError")]
    RelayEnvironmentEndpointTimedOutError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayEnvironmentEndpointTimedOutError {
    #[serde(rename = "_tag")]
    pub _tag: RelayEnvironmentEndpointTimedOutErrorTag,
    #[serde(rename = "code")]
    pub code: RelayEnvironmentEndpointTimedOutErrorCode,
    #[serde(rename = "traceId")]
    pub trace_id: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayEnvironmentLinkFailedErrorCode {
    #[serde(rename = "environment_link_failed")]
    EnvironmentLinkFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayEnvironmentLinkFailedErrorTag {
    #[serde(rename = "RelayEnvironmentLinkFailedError")]
    RelayEnvironmentLinkFailedError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayEnvironmentLinkFailedError {
    #[serde(rename = "_tag")]
    pub _tag: RelayEnvironmentLinkFailedErrorTag,
    #[serde(rename = "code")]
    pub code: RelayEnvironmentLinkFailedErrorCode,
    #[serde(rename = "reason")]
    pub reason: RelayEnvironmentLinkFailedReason,
    #[serde(rename = "traceId")]
    pub trace_id: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayEnvironmentLinkUnavailableErrorCode {
    #[serde(rename = "environment_link_unavailable")]
    EnvironmentLinkUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayEnvironmentLinkUnavailableErrorTag {
    #[serde(rename = "RelayEnvironmentLinkUnavailableError")]
    RelayEnvironmentLinkUnavailableError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayEnvironmentLinkUnavailableError {
    #[serde(rename = "_tag")]
    pub _tag: RelayEnvironmentLinkUnavailableErrorTag,
    #[serde(rename = "code")]
    pub code: RelayEnvironmentLinkUnavailableErrorCode,
    #[serde(rename = "reason")]
    pub reason: RelayEnvironmentLinkUnavailableReason,
    #[serde(
        rename = "provisioningStage",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub provisioning_stage: Option<TrimmedNonEmptyString>,
    #[serde(rename = "traceId")]
    pub trace_id: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayEnvironmentLinkLimitExceededErrorCode {
    #[serde(rename = "environment_link_limit_exceeded")]
    EnvironmentLinkLimitExceeded,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayEnvironmentLinkLimitExceededErrorTag {
    #[serde(rename = "RelayEnvironmentLinkLimitExceededError")]
    RelayEnvironmentLinkLimitExceededError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayEnvironmentLinkLimitExceededError {
    #[serde(rename = "_tag")]
    pub _tag: RelayEnvironmentLinkLimitExceededErrorTag,
    #[serde(rename = "code")]
    pub code: RelayEnvironmentLinkLimitExceededErrorCode,
    #[serde(rename = "maxTunnels")]
    pub max_tunnels: EncodedNumber,
    #[serde(rename = "traceId")]
    pub trace_id: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayAgentActivityPublishProofExpiredErrorCode {
    #[serde(rename = "agent_activity_publish_proof_expired")]
    AgentActivityPublishProofExpired,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayAgentActivityPublishProofExpiredErrorTag {
    #[serde(rename = "RelayAgentActivityPublishProofExpiredError")]
    RelayAgentActivityPublishProofExpiredError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayAgentActivityPublishProofExpiredError {
    #[serde(rename = "_tag")]
    pub _tag: RelayAgentActivityPublishProofExpiredErrorTag,
    #[serde(rename = "code")]
    pub code: RelayAgentActivityPublishProofExpiredErrorCode,
    #[serde(rename = "traceId")]
    pub trace_id: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayAgentActivityPublishProofInvalidErrorCode {
    #[serde(rename = "agent_activity_publish_proof_invalid")]
    AgentActivityPublishProofInvalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayAgentActivityPublishProofInvalidErrorTag {
    #[serde(rename = "RelayAgentActivityPublishProofInvalidError")]
    RelayAgentActivityPublishProofInvalidError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayAgentActivityPublishProofInvalidError {
    #[serde(rename = "_tag")]
    pub _tag: RelayAgentActivityPublishProofInvalidErrorTag,
    #[serde(rename = "code")]
    pub code: RelayAgentActivityPublishProofInvalidErrorCode,
    #[serde(rename = "reason")]
    pub reason: RelayAgentActivityPublishProofInvalidReason,
    #[serde(rename = "traceId")]
    pub trace_id: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayInternalErrorCode {
    #[serde(rename = "internal_error")]
    InternalError,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayInternalErrorTag {
    #[serde(rename = "RelayInternalError")]
    RelayInternalError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayInternalError {
    #[serde(rename = "_tag")]
    pub _tag: RelayInternalErrorTag,
    #[serde(rename = "code")]
    pub code: RelayInternalErrorCode,
    #[serde(rename = "reason")]
    pub reason: RelayInternalErrorReason,
    #[serde(rename = "traceId")]
    pub trace_id: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RelayProtectedError {
    RelayAuthInvalidError(RelayAuthInvalidError),
    RelayEnvironmentLinkProofExpiredError(RelayEnvironmentLinkProofExpiredError),
    RelayEnvironmentLinkProofInvalidError(RelayEnvironmentLinkProofInvalidError),
    RelayEnvironmentConnectNotAuthorizedError(RelayEnvironmentConnectNotAuthorizedError),
    RelayEnvironmentEndpointUnavailableError(RelayEnvironmentEndpointUnavailableError),
    RelayEnvironmentEndpointTimedOutError(RelayEnvironmentEndpointTimedOutError),
    RelayEnvironmentLinkFailedError(RelayEnvironmentLinkFailedError),
    RelayEnvironmentLinkUnavailableError(RelayEnvironmentLinkUnavailableError),
    RelayEnvironmentLinkLimitExceededError(RelayEnvironmentLinkLimitExceededError),
    RelayAgentActivityPublishProofExpiredError(RelayAgentActivityPublishProofExpiredError),
    RelayAgentActivityPublishProofInvalidError(RelayAgentActivityPublishProofInvalidError),
    RelayInternalError(RelayInternalError),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayClientEnvironmentRecord {
    #[serde(rename = "environmentId")]
    pub environment_id: EnvironmentId,
    #[serde(rename = "label")]
    pub label: TrimmedNonEmptyString,
    #[serde(rename = "endpoint")]
    pub endpoint: RelayManagedEndpoint,
    #[serde(rename = "linkedAt")]
    pub linked_at: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayListEnvironmentsResponse {
    #[serde(rename = "environments")]
    pub environments: Vec<RelayClientEnvironmentRecord>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayEnvironmentConnectRequest {
    #[serde(
        rename = "deviceId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub device_id: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        rename = "clientKeyThumbprint",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub client_key_thumbprint: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        rename = "clientProofKeyThumbprint",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub client_proof_key_thumbprint: Option<Option<TrimmedNonEmptyString>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayDpopAccessTokenScope {
    #[serde(rename = "environment:connect")]
    EnvironmentConnect,
    #[serde(rename = "environment:status")]
    EnvironmentStatus,
    #[serde(rename = "mobile:registration")]
    MobileRegistration,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayPublicClientId {
    #[serde(rename = "t3-mobile")]
    T3Mobile,
    #[serde(rename = "t3-web")]
    T3Web,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayDpopTokenExchangeGrantType {
    #[serde(rename = "urn:ietf:params:oauth:grant-type:token-exchange")]
    UrnIetfParamsOauthGrantTypeTokenExchange,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayJwtSubjectTokenType {
    #[serde(rename = "urn:ietf:params:oauth:token-type:jwt")]
    UrnIetfParamsOauthTokenTypeJwt,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayAccessTokenType {
    #[serde(rename = "urn:ietf:params:oauth:token-type:access_token")]
    UrnIetfParamsOauthTokenTypeAccessToken,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayDPoPTokenType {
    #[serde(rename = "DPoP")]
    DPoP,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayNoneAuthMethod {
    #[serde(rename = "none")]
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelaySigningAlgorithm {
    #[serde(rename = "ES256")]
    ES256,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayDpopAccessTokenRequest {
    #[serde(rename = "grant_type")]
    pub grant_type: RelayDpopTokenExchangeGrantType,
    #[serde(rename = "subject_token")]
    pub subject_token: TrimmedNonEmptyString,
    #[serde(rename = "subject_token_type")]
    pub subject_token_type: RelayJwtSubjectTokenType,
    #[serde(rename = "requested_token_type")]
    pub requested_token_type: RelayAccessTokenType,
    #[serde(rename = "resource")]
    pub resource: TrimmedNonEmptyString,
    #[serde(rename = "scope")]
    pub scope: TrimmedNonEmptyString,
    #[serde(rename = "client_id")]
    pub client_id: RelayPublicClientId,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayDpopAccessTokenResponse {
    #[serde(rename = "access_token")]
    pub access_token: TrimmedNonEmptyString,
    #[serde(rename = "issued_token_type")]
    pub issued_token_type: RelayAccessTokenType,
    #[serde(rename = "token_type")]
    pub token_type: RelayDPoPTokenType,
    #[serde(rename = "expires_in")]
    pub expires_in: u64,
    #[serde(rename = "scope")]
    pub scope: TrimmedNonEmptyString,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayBearerRequestHeaders {
    #[serde(rename = "authorization")]
    pub authorization: TrimmedNonEmptyString,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayDpopProofRequestHeaders {
    #[serde(rename = "dpop")]
    pub dpop: TrimmedNonEmptyString,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayDpopRequestHeaders {
    #[serde(rename = "authorization")]
    pub authorization: TrimmedNonEmptyString,
    #[serde(rename = "dpop")]
    pub dpop: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayAuthorizationServerMetadata {
    #[serde(rename = "issuer")]
    pub issuer: TrimmedNonEmptyString,
    #[serde(rename = "token_endpoint")]
    pub token_endpoint: TrimmedNonEmptyString,
    #[serde(rename = "grant_types_supported")]
    pub grant_types_supported: Vec<RelayDpopTokenExchangeGrantType>,
    #[serde(rename = "token_endpoint_auth_methods_supported")]
    pub token_endpoint_auth_methods_supported: Vec<RelayNoneAuthMethod>,
    #[serde(rename = "dpop_signing_alg_values_supported")]
    pub dpop_signing_alg_values_supported: Vec<RelaySigningAlgorithm>,
    #[serde(rename = "scopes_supported")]
    pub scopes_supported: Vec<RelayDpopAccessTokenScope>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayProtectedResourceMetadata {
    #[serde(rename = "resource")]
    pub resource: TrimmedNonEmptyString,
    #[serde(rename = "authorization_servers")]
    pub authorization_servers: Vec<TrimmedNonEmptyString>,
    #[serde(rename = "scopes_supported")]
    pub scopes_supported: Vec<RelayDpopAccessTokenScope>,
    #[serde(rename = "dpop_bound_access_tokens_required")]
    pub dpop_bound_access_tokens_required: bool,
    #[serde(rename = "dpop_signing_alg_values_supported")]
    pub dpop_signing_alg_values_supported: Vec<RelaySigningAlgorithm>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayEnvironmentUnlinkParams {
    #[serde(rename = "environmentId")]
    pub environment_id: EnvironmentId,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayEnvironmentConnectResponse {
    #[serde(rename = "environmentId")]
    pub environment_id: EnvironmentId,
    #[serde(rename = "endpoint")]
    pub endpoint: RelayManagedEndpoint,
    #[serde(rename = "credential")]
    pub credential: TrimmedNonEmptyString,
    #[serde(rename = "expiresAt")]
    pub expires_at: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayEnvironmentStatusValue {
    #[serde(rename = "online")]
    Online,
    #[serde(rename = "offline")]
    Offline,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayEnvironmentOfflineReason {
    #[serde(rename = "tunnel_released")]
    TunnelReleased,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayConfirmation {
    #[serde(rename = "jkt")]
    pub jkt: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayConnectScope {
    #[serde(rename = "environment:connect")]
    EnvironmentConnect,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayStatusScope {
    #[serde(rename = "environment:status")]
    EnvironmentStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayOnlineStatus {
    #[serde(rename = "online")]
    Online,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayCloudMintCredentialProofPayload {
    #[serde(rename = "iss")]
    pub iss: TrimmedNonEmptyString,
    #[serde(rename = "aud")]
    pub aud: TrimmedNonEmptyString,
    #[serde(rename = "sub")]
    pub sub: TrimmedNonEmptyString,
    #[serde(rename = "jti")]
    pub jti: TrimmedNonEmptyString,
    #[serde(rename = "iat")]
    pub iat: i64,
    #[serde(rename = "exp")]
    pub exp: i64,
    #[serde(rename = "environmentId")]
    pub environment_id: EnvironmentId,
    #[serde(rename = "clientProofKeyThumbprint")]
    pub client_proof_key_thumbprint: TrimmedNonEmptyString,
    #[serde(rename = "cnf")]
    pub cnf: RelayConfirmation,
    #[serde(
        rename = "deviceId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub device_id: Option<Option<TrimmedNonEmptyString>>,
    #[serde(rename = "nonce")]
    pub nonce: TrimmedNonEmptyString,
    #[serde(rename = "scope")]
    pub scope: Vec<RelayConnectScope>,
}

pub type RelayCloudMintCredentialProof = TrimmedNonEmptyString;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayCloudMintCredentialRequest {
    #[serde(rename = "proof")]
    pub proof: RelayCloudMintCredentialProof,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayCloudEnvironmentHealthProofPayload {
    #[serde(rename = "iss")]
    pub iss: TrimmedNonEmptyString,
    #[serde(rename = "aud")]
    pub aud: TrimmedNonEmptyString,
    #[serde(rename = "sub")]
    pub sub: TrimmedNonEmptyString,
    #[serde(rename = "jti")]
    pub jti: TrimmedNonEmptyString,
    #[serde(rename = "iat")]
    pub iat: i64,
    #[serde(rename = "exp")]
    pub exp: i64,
    #[serde(rename = "environmentId")]
    pub environment_id: EnvironmentId,
    #[serde(rename = "nonce")]
    pub nonce: TrimmedNonEmptyString,
    #[serde(rename = "scope")]
    pub scope: Vec<RelayStatusScope>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayHookDeliveryProofPayload {
    #[serde(rename = "iss")]
    pub iss: TrimmedNonEmptyString,
    #[serde(rename = "aud")]
    pub aud: TrimmedNonEmptyString,
    #[serde(rename = "sub")]
    pub sub: TrimmedNonEmptyString,
    #[serde(rename = "jti")]
    pub jti: TrimmedNonEmptyString,
    #[serde(rename = "iat")]
    pub iat: i64,
    #[serde(rename = "exp")]
    pub exp: i64,
    #[serde(rename = "environmentId")]
    pub environment_id: EnvironmentId,
    #[serde(rename = "deliveryId")]
    pub delivery_id: TrimmedNonEmptyString,
    #[serde(rename = "receivedAt")]
    pub received_at: TrimmedNonEmptyString,
    #[serde(rename = "hookId")]
    pub hook_id: TrimmedNonEmptyString,
}

pub type RelayCloudEnvironmentHealthProof = TrimmedNonEmptyString;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayCloudEnvironmentHealthRequest {
    #[serde(rename = "proof")]
    pub proof: RelayCloudEnvironmentHealthProof,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayEnvironmentMintResponseProofPayload {
    #[serde(rename = "iss")]
    pub iss: TrimmedNonEmptyString,
    #[serde(rename = "aud")]
    pub aud: TrimmedNonEmptyString,
    #[serde(rename = "sub")]
    pub sub: TrimmedNonEmptyString,
    #[serde(rename = "jti")]
    pub jti: TrimmedNonEmptyString,
    #[serde(rename = "iat")]
    pub iat: i64,
    #[serde(rename = "exp")]
    pub exp: i64,
    #[serde(rename = "environmentId")]
    pub environment_id: EnvironmentId,
    #[serde(rename = "clientProofKeyThumbprint")]
    pub client_proof_key_thumbprint: TrimmedNonEmptyString,
    #[serde(rename = "requestNonce")]
    pub request_nonce: TrimmedNonEmptyString,
    #[serde(rename = "credential")]
    pub credential: TrimmedNonEmptyString,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayEnvironmentMintResponse {
    #[serde(rename = "credential")]
    pub credential: TrimmedNonEmptyString,
    #[serde(rename = "expiresAt")]
    pub expires_at: TrimmedNonEmptyString,
    #[serde(rename = "proof")]
    pub proof: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayDeliveryKind {
    #[serde(rename = "live_activity_start")]
    LiveActivityStart,
    #[serde(rename = "live_activity_update")]
    LiveActivityUpdate,
    #[serde(rename = "live_activity_end")]
    LiveActivityEnd,
    #[serde(rename = "push_notification")]
    PushNotification,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayDeliveryResult {
    #[serde(rename = "deviceId")]
    pub device_id: TrimmedNonEmptyString,
    #[serde(rename = "kind")]
    pub kind: RelayDeliveryKind,
    #[serde(rename = "ok")]
    pub ok: bool,
    #[serde(
        rename = "queued",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub queued: Option<Option<bool>>,
    #[serde(rename = "apnsStatus", with = "crate::wire::nullable")]
    pub apns_status: Option<EncodedNumber>,
    #[serde(rename = "apnsReason", with = "crate::wire::nullable")]
    pub apns_reason: Option<String>,
    #[serde(rename = "apnsId", with = "crate::wire::nullable")]
    pub apns_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayOkResponse {
    #[serde(rename = "ok")]
    pub ok: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayT3ChatPlan {
    #[serde(rename = "free")]
    Free,
    #[serde(rename = "pro")]
    Pro,
    #[serde(rename = "premier")]
    Premier,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayT3ChatLink {
    #[serde(rename = "email")]
    pub email: String,
    #[serde(rename = "plan")]
    pub plan: RelayT3ChatPlan,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayT3ChatLinkResponse {
    #[serde(rename = "available")]
    pub available: bool,
    #[serde(rename = "link", with = "crate::wire::nullable")]
    pub link: Option<RelayT3ChatLink>,
    #[serde(
        rename = "maxTunnelsOverride",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub max_tunnels_override: Option<EncodedNumber>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayT3ChatLinkRequestResponse {
    #[serde(rename = "url")]
    pub url: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayPublishResponse {
    #[serde(rename = "ok")]
    pub ok: bool,
    #[serde(rename = "deliveries")]
    pub deliveries: Vec<RelayDeliveryResult>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayService {
    #[serde(rename = "relay")]
    Relay,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayHealthResponse {
    #[serde(rename = "ok")]
    pub ok: bool,
    #[serde(rename = "service")]
    pub service: RelayService,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayAgentActivitySnapshotResponse {
    #[serde(rename = "aggregate", with = "crate::wire::nullable")]
    pub aggregate: Option<RelayAgentActivityAggregateState>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayEnvironmentLinkPreferencesRequest {
    #[serde(rename = "holdWebhooksWhileOffline")]
    pub hold_webhooks_while_offline: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayWakeHeldHooksResponse {
    #[serde(rename = "pending")]
    pub pending: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RelayMobileTelemetrySignal {
    #[serde(rename = "logs")]
    Logs,
    #[serde(rename = "metrics")]
    Metrics,
    #[serde(rename = "traces")]
    Traces,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayEnvironmentLinkProofPayload {
    #[serde(rename = "iss")]
    pub iss: TrimmedNonEmptyString,
    #[serde(rename = "aud")]
    pub aud: TrimmedNonEmptyString,
    #[serde(rename = "sub")]
    pub sub: TrimmedNonEmptyString,
    #[serde(rename = "jti")]
    pub jti: TrimmedNonEmptyString,
    #[serde(rename = "iat")]
    pub iat: i64,
    #[serde(rename = "exp")]
    pub exp: i64,
    #[serde(rename = "challenge")]
    pub challenge: TrimmedNonEmptyString,
    #[serde(rename = "descriptor")]
    pub descriptor: ExecutionEnvironmentDescriptor,
    #[serde(rename = "environmentId")]
    pub environment_id: EnvironmentId,
    #[serde(rename = "environmentPublicKey")]
    pub environment_public_key: TrimmedNonEmptyString,
    #[serde(rename = "endpoint")]
    pub endpoint: RelayManagedEndpoint,
    #[serde(rename = "origin")]
    pub origin: RelayManagedEndpointOrigin,
    #[serde(rename = "scopes")]
    pub scopes: Vec<RelayEnvironmentLinkScope>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayEnvironmentStatusResponse {
    #[serde(rename = "environmentId")]
    pub environment_id: EnvironmentId,
    #[serde(rename = "endpoint")]
    pub endpoint: RelayManagedEndpoint,
    #[serde(rename = "status")]
    pub status: RelayEnvironmentStatusValue,
    #[serde(rename = "checkedAt")]
    pub checked_at: TrimmedNonEmptyString,
    #[serde(
        rename = "descriptor",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub descriptor: Option<Option<ExecutionEnvironmentDescriptor>>,
    #[serde(
        rename = "error",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub error: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        rename = "endpointFailure",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub endpoint_failure: Option<Option<RelayEnvironmentEndpointFailure>>,
    #[serde(
        rename = "endpointStatus",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub endpoint_status: Option<Option<EncodedNumber>>,
    #[serde(
        rename = "offlineReason",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub offline_reason: Option<Option<RelayEnvironmentOfflineReason>>,
    #[serde(
        rename = "traceId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub trace_id: Option<Option<TrimmedNonEmptyString>>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayEnvironmentHealthResponseProofPayload {
    #[serde(rename = "iss")]
    pub iss: TrimmedNonEmptyString,
    #[serde(rename = "aud")]
    pub aud: TrimmedNonEmptyString,
    #[serde(rename = "sub")]
    pub sub: TrimmedNonEmptyString,
    #[serde(rename = "jti")]
    pub jti: TrimmedNonEmptyString,
    #[serde(rename = "iat")]
    pub iat: i64,
    #[serde(rename = "exp")]
    pub exp: i64,
    #[serde(rename = "environmentId")]
    pub environment_id: EnvironmentId,
    #[serde(rename = "requestNonce")]
    pub request_nonce: TrimmedNonEmptyString,
    #[serde(rename = "status")]
    pub status: RelayOnlineStatus,
    #[serde(rename = "descriptor")]
    pub descriptor: ExecutionEnvironmentDescriptor,
    #[serde(rename = "checkedAt")]
    pub checked_at: TrimmedNonEmptyString,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayEnvironmentHealthResponse {
    #[serde(rename = "environmentId")]
    pub environment_id: EnvironmentId,
    #[serde(rename = "status")]
    pub status: RelayOnlineStatus,
    #[serde(rename = "descriptor")]
    pub descriptor: ExecutionEnvironmentDescriptor,
    #[serde(rename = "checkedAt")]
    pub checked_at: TrimmedNonEmptyString,
    #[serde(rename = "proof")]
    pub proof: TrimmedNonEmptyString,
}

crate::redacted_debug!(RelayDeviceRegistrationRequest { device_id, label, platform, ios_major_version, android_api_level, app_version, bundle_id, aps_environment, preferences } redact { push_token, push_to_start_token });
crate::redacted_debug!(RelayLiveActivityRegistrationRequest { device_id } redact { activity_push_token });
crate::redacted_debug!(RelayManagedEndpointRuntimeConfig { provider_kind, tunnel_id, tunnel_name } redact { connector_token });
crate::redacted_debug!(RelayManagedEndpointRecoveryRequest { cloud_user_id, origin } redact { proof });
crate::redacted_debug!(RelayManagedEndpointRecoveryRegistrationRequest { cloud_user_id, tunnel_id, origin } redact { proof });
crate::redacted_debug!(RelayEnvironmentConfigRequest { relay_url, relay_issuer, cloud_user_id, cloud_mint_public_key, endpoint_runtime } redact { environment_credential });
crate::redacted_debug!(RelayAgentActivityPublishRequest { state } redact { proof });
crate::redacted_debug!(RelayEnvironmentLinkRequest { device_id, notifications_enabled, live_activities_enabled, managed_tunnels_enabled } redact { proof });
crate::redacted_debug!(RelayEnvironmentLinkResponse { ok, cloud_user_id, environment_id, endpoint, endpoint_runtime, relay_issuer, cloud_mint_public_key } redact { environment_credential });
crate::redacted_debug!(RelayDpopAccessTokenRequest { grant_type, subject_token_type, requested_token_type, resource, scope, client_id } redact { subject_token });
crate::redacted_debug!(RelayDpopAccessTokenResponse { issued_token_type, token_type, expires_in, scope } redact { access_token });
crate::redacted_debug!(RelayBearerRequestHeaders {  } redact { authorization });
crate::redacted_debug!(RelayDpopProofRequestHeaders {  } redact { dpop });
crate::redacted_debug!(RelayDpopRequestHeaders {  } redact { authorization, dpop });
crate::redacted_debug!(RelayEnvironmentConnectResponse { environment_id, endpoint, expires_at } redact { credential });
crate::redacted_debug!(RelayCloudMintCredentialProofPayload { iss, aud, sub, jti, iat, exp, environment_id, client_proof_key_thumbprint, cnf, device_id, scope } redact { nonce });
crate::redacted_debug!(RelayCloudMintCredentialRequest {  } redact { proof });
crate::redacted_debug!(RelayCloudEnvironmentHealthProofPayload { iss, aud, sub, jti, iat, exp, environment_id, scope } redact { nonce });
crate::redacted_debug!(RelayCloudEnvironmentHealthRequest {  } redact { proof });
crate::redacted_debug!(RelayEnvironmentMintResponseProofPayload { iss, aud, sub, jti, iat, exp, environment_id, client_proof_key_thumbprint } redact { request_nonce, credential });
crate::redacted_debug!(RelayEnvironmentMintResponse { expires_at } redact { credential, proof });
crate::redacted_debug!(RelayEnvironmentHealthResponseProofPayload { iss, aud, sub, jti, iat, exp, environment_id, status, descriptor, checked_at } redact { request_nonce });
crate::redacted_debug!(RelayEnvironmentHealthResponse { environment_id, status, descriptor, checked_at } redact { proof });
