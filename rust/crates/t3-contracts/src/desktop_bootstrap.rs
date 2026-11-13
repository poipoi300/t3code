use crate::base_schemas::TrimmedNonEmptyString;
use serde::{Deserialize, Serialize};
pub const SERVER_EXIT_CODE_STATE_DIR_OWNED: i32 = 78;
pub const SERVER_EXIT_CODE_MANAGED_POLICY: i32 = 77;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DesktopBackendMode {
    #[serde(rename = "desktop")]
    Desktop,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopBackendBootstrap {
    pub mode: DesktopBackendMode,
    pub no_browser: bool,
    pub port: u16,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub t3_home: Option<Option<String>>,
    pub host: String,
    pub desktop_bootstrap_token: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub desktop_bootstrap_secret: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub shell_environment_prepared: Option<bool>,
    pub tailscale_serve_enabled: bool,
    pub tailscale_serve_port: u16,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub otlp_traces_url: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub otlp_metrics_url: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub otlp_logs_url: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub desktop_telemetry_fd: Option<u64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub desktop_telemetry_control_fd: Option<u64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub desktop_app_update: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub desktop_browser_fd: Option<u64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub desktop_browser_control_fd: Option<u64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub resource_monitor_path: Option<TrimmedNonEmptyString>,
}

crate::redacted_debug!(DesktopBackendBootstrap { mode, no_browser, port, t3_home, host, shell_environment_prepared, tailscale_serve_enabled, tailscale_serve_port, otlp_traces_url, otlp_metrics_url, otlp_logs_url, desktop_telemetry_fd, desktop_telemetry_control_fd, desktop_app_update, desktop_browser_fd, desktop_browser_control_fd, resource_monitor_path } redact { desktop_bootstrap_token, desktop_bootstrap_secret });
