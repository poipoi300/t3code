//! Environment HTTP request, response, and error wire contracts.

use serde::{Deserialize, Serialize};

use crate::{
    auth::{AuthEnvironmentScope, EncodedNumber},
    base_schemas::{DpopFailureReason, TrimmedNonEmptyString},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnvironmentRequestInvalidReason {
    #[serde(rename = "invalid_scope")]
    InvalidScope,
    #[serde(rename = "scope_not_granted")]
    ScopeNotGranted,
    #[serde(rename = "invalid_command")]
    InvalidCommand,
    #[serde(rename = "invalid_history_cursor")]
    InvalidHistoryCursor,
    #[serde(rename = "thread_transcript_too_large")]
    ThreadTranscriptTooLarge,
    #[serde(rename = "invalid_audio")]
    InvalidAudio,
    #[serde(rename = "speech_unavailable")]
    SpeechUnavailable,
    #[serde(rename = "speech_busy")]
    SpeechBusy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnvironmentAuthInvalidReason {
    #[serde(rename = "missing_credential")]
    MissingCredential,
    #[serde(rename = "invalid_credential")]
    InvalidCredential,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnvironmentOperationForbiddenReason {
    #[serde(rename = "current_session_revoke_not_allowed")]
    CurrentSessionRevokeNotAllowed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnvironmentInternalErrorReason {
    #[serde(rename = "bootstrap_validation_failed")]
    BootstrapValidationFailed,
    #[serde(rename = "browser_session_issuance_failed")]
    BrowserSessionIssuanceFailed,
    #[serde(rename = "browser_session_cookie_failed")]
    BrowserSessionCookieFailed,
    #[serde(rename = "access_token_issuance_failed")]
    AccessTokenIssuanceFailed,
    #[serde(rename = "websocket_ticket_issuance_failed")]
    WebsocketTicketIssuanceFailed,
    #[serde(rename = "pairing_credential_issuance_failed")]
    PairingCredentialIssuanceFailed,
    #[serde(rename = "pairing_links_load_failed")]
    PairingLinksLoadFailed,
    #[serde(rename = "pairing_link_revoke_failed")]
    PairingLinkRevokeFailed,
    #[serde(rename = "client_sessions_load_failed")]
    ClientSessionsLoadFailed,
    #[serde(rename = "client_session_revoke_failed")]
    ClientSessionRevokeFailed,
    #[serde(rename = "project_snapshot_failed")]
    ProjectSnapshotFailed,
    #[serde(rename = "project_mutation_failed")]
    ProjectMutationFailed,
    #[serde(rename = "orchestration_snapshot_failed")]
    OrchestrationSnapshotFailed,
    #[serde(rename = "orchestration_thread_snapshot_failed")]
    OrchestrationThreadSnapshotFailed,
    #[serde(rename = "orchestration_thread_bounded_snapshot_failed")]
    OrchestrationThreadBoundedSnapshotFailed,
    #[serde(rename = "orchestration_thread_history_failed")]
    OrchestrationThreadHistoryFailed,
    #[serde(rename = "internal_error")]
    InternalError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnvironmentResourceNotFoundReason {
    #[serde(rename = "thread_not_found")]
    ThreadNotFound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnvironmentRequestInvalidErrorTag {
    #[serde(rename = "EnvironmentRequestInvalidError")]
    EnvironmentRequestInvalidError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnvironmentRequestInvalidErrorCode {
    #[serde(rename = "invalid_request")]
    InvalidRequest,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentRequestInvalidError {
    #[serde(rename = "_tag")]
    pub tag: EnvironmentRequestInvalidErrorTag,
    pub code: EnvironmentRequestInvalidErrorCode,
    pub reason: EnvironmentRequestInvalidReason,
    pub trace_id: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnvironmentAuthInvalidErrorTag {
    #[serde(rename = "EnvironmentAuthInvalidError")]
    EnvironmentAuthInvalidError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnvironmentAuthInvalidErrorCode {
    #[serde(rename = "auth_invalid")]
    AuthInvalid,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentAuthInvalidError {
    #[serde(rename = "_tag")]
    pub tag: EnvironmentAuthInvalidErrorTag,
    pub code: EnvironmentAuthInvalidErrorCode,
    pub reason: EnvironmentAuthInvalidReason,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub dpop_failure_reason: Option<DpopFailureReason>,
    pub trace_id: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnvironmentScopeRequiredErrorTag {
    #[serde(rename = "EnvironmentScopeRequiredError")]
    EnvironmentScopeRequiredError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnvironmentScopeRequiredErrorCode {
    #[serde(rename = "insufficient_scope")]
    InsufficientScope,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentScopeRequiredError {
    #[serde(rename = "_tag")]
    pub tag: EnvironmentScopeRequiredErrorTag,
    pub code: EnvironmentScopeRequiredErrorCode,
    pub required_scope: AuthEnvironmentScope,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub required_permission: Option<String>,
    pub trace_id: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnvironmentOperationForbiddenErrorTag {
    #[serde(rename = "EnvironmentOperationForbiddenError")]
    EnvironmentOperationForbiddenError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnvironmentOperationForbiddenErrorCode {
    #[serde(rename = "operation_forbidden")]
    OperationForbidden,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentOperationForbiddenError {
    #[serde(rename = "_tag")]
    pub tag: EnvironmentOperationForbiddenErrorTag,
    pub code: EnvironmentOperationForbiddenErrorCode,
    pub reason: EnvironmentOperationForbiddenReason,
    pub trace_id: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnvironmentInternalErrorTag {
    #[serde(rename = "EnvironmentInternalError")]
    EnvironmentInternalError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnvironmentInternalErrorCode {
    #[serde(rename = "internal_error")]
    InternalError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentInternalError {
    #[serde(rename = "_tag")]
    pub tag: EnvironmentInternalErrorTag,
    pub code: EnvironmentInternalErrorCode,
    pub reason: EnvironmentInternalErrorReason,
    pub trace_id: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnvironmentResourceNotFoundErrorTag {
    #[serde(rename = "EnvironmentResourceNotFoundError")]
    EnvironmentResourceNotFoundError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnvironmentResourceNotFoundErrorCode {
    #[serde(rename = "not_found")]
    NotFound,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentResourceNotFoundError {
    #[serde(rename = "_tag")]
    pub tag: EnvironmentResourceNotFoundErrorTag,
    pub code: EnvironmentResourceNotFoundErrorCode,
    pub reason: EnvironmentResourceNotFoundReason,
    pub trace_id: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EnvironmentHttpCommonError {
    EnvironmentRequestInvalidError(EnvironmentRequestInvalidError),
    EnvironmentAuthInvalidError(EnvironmentAuthInvalidError),
    EnvironmentScopeRequiredError(EnvironmentScopeRequiredError),
    EnvironmentOperationForbiddenError(EnvironmentOperationForbiddenError),
    EnvironmentInternalError(EnvironmentInternalError),
    EnvironmentResourceNotFoundError(EnvironmentResourceNotFoundError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnvironmentHttpBadRequestErrorTag {
    #[serde(rename = "EnvironmentHttpBadRequestError")]
    EnvironmentHttpBadRequestError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentHttpBadRequestError {
    #[serde(rename = "_tag")]
    pub tag: EnvironmentHttpBadRequestErrorTag,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnvironmentHttpUnauthorizedErrorTag {
    #[serde(rename = "EnvironmentHttpUnauthorizedError")]
    EnvironmentHttpUnauthorizedError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentHttpUnauthorizedError {
    #[serde(rename = "_tag")]
    pub tag: EnvironmentHttpUnauthorizedErrorTag,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnvironmentHttpForbiddenErrorTag {
    #[serde(rename = "EnvironmentHttpForbiddenError")]
    EnvironmentHttpForbiddenError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentHttpForbiddenError {
    #[serde(rename = "_tag")]
    pub tag: EnvironmentHttpForbiddenErrorTag,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnvironmentHttpInternalServerErrorTag {
    #[serde(rename = "EnvironmentHttpInternalServerError")]
    EnvironmentHttpInternalServerError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentHttpInternalServerError {
    #[serde(rename = "_tag")]
    pub tag: EnvironmentHttpInternalServerErrorTag,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnvironmentHttpConflictErrorTag {
    #[serde(rename = "EnvironmentHttpConflictError")]
    EnvironmentHttpConflictError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentHttpConflictError {
    #[serde(rename = "_tag")]
    pub tag: EnvironmentHttpConflictErrorTag,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnvironmentCloudEndpointUnavailableErrorTag {
    #[serde(rename = "EnvironmentCloudEndpointUnavailableError")]
    EnvironmentCloudEndpointUnavailableError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentCloudEndpointUnavailableError {
    #[serde(rename = "_tag")]
    pub tag: EnvironmentCloudEndpointUnavailableErrorTag,
    pub message: String,
    pub endpoint_runtime_status: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentCloudRelayConfigResult {
    pub ok: bool,
    pub endpoint_runtime_status: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnvironmentConnectReachabilityCause {
    #[serde(rename = "not_linked")]
    NotLinked,
    #[serde(rename = "connector_not_running")]
    ConnectorNotRunning,
    #[serde(rename = "connector_not_ready")]
    ConnectorNotReady,
    #[serde(rename = "edge_unreachable")]
    EdgeUnreachable,
    #[serde(rename = "no_connector")]
    NoConnector,
    #[serde(rename = "origin_unreachable")]
    OriginUnreachable,
    #[serde(rename = "dns_failed")]
    DnsFailed,
    #[serde(rename = "timeout")]
    Timeout,
    #[serde(rename = "unexpected_response")]
    UnexpectedResponse,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status")]
pub enum EnvironmentConnectReachability {
    #[serde(rename = "reachable", rename_all = "camelCase")]
    Reachable {
        checked_at: String,
        #[serde(with = "crate::wire::nullable")]
        ready_connections: Option<i64>,
        #[serde(with = "crate::wire::nullable")]
        protocol: Option<String>,
    },
    #[serde(rename = "unreachable", rename_all = "camelCase")]
    Unreachable {
        checked_at: String,
        cause: EnvironmentConnectReachabilityCause,
        reason: String,
        #[serde(with = "crate::wire::nullable")]
        hint: Option<String>,
        fix: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentCloudLinkStateResult {
    pub linked: bool,
    #[serde(with = "crate::wire::nullable")]
    pub cloud_user_id: Option<String>,
    #[serde(with = "crate::wire::nullable")]
    pub relay_url: Option<String>,
    #[serde(with = "crate::wire::nullable")]
    pub relay_issuer: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub managed_tunnel_active: Option<Option<bool>>,
    pub publish_agent_activity: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub hold_webhooks_while_offline: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub link_revoked: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub reachability: Option<Option<EnvironmentConnectReachability>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tunnel_hostname: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub linkable_here: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentCloudPreferencesRequest {
    pub publish_agent_activity: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub hold_webhooks_while_offline: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthPairingLinkRevokeResult {
    pub revoked: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthClientSessionRevokeResult {
    pub revoked: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthOtherClientSessionsRevokeResult {
    pub revoked_count: EncodedNumber,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentOrchestrationShellSnapshotQuery {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub omit_idle_subagents: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub omit_older_settled: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub resume_from: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentOrchestrationThreadBoundedSnapshotQuery {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub defer_work: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub compact_turn_items: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub dedupe_bodies: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub compact_ids: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnvironmentOrchestrationThreadHistoryView {
    #[serde(rename = "conversation")]
    Conversation,
    #[serde(rename = "activity")]
    Activity,
    #[serde(rename = "work")]
    Work,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentOrchestrationThreadHistoryQuery {
    pub cursor: TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub through_entry_id: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub view: Option<Option<EnvironmentOrchestrationThreadHistoryView>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub defer_work: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub compact_ids: Option<String>,
}
