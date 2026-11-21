use t3_contracts::device::*;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, thiserror::Error)]
pub enum Error {
    #[error("{reason}")]
    GatewayStatus { status: u16, reason: String },
    #[error("{}", .0.reason)]
    EasAccount(DeviceEasAccountError),
    #[error("{}", .0.reason)]
    SimlockIdentity(DeviceSimlockIdentityError),
    #[error("{reason}")]
    Device {
        wire: Box<DeviceError>,
        reason: String,
    },
    #[error("{0}")]
    Unavailable(String),
    #[error("Device {device_id} is on more than one host: {hosts:?}. Pass hostId.")]
    Ambiguous {
        device_id: String,
        hosts: Vec<String>,
    },
    #[error("Device {device_id} is not on {host}. It is on {hosts:?}; pass that hostId.")]
    OtherHost {
        device_id: String,
        host: String,
        hosts: Vec<String>,
    },
    #[error("Device operation {0} was cancelled.")]
    Cancelled(&'static str),
}

impl Error {
    pub fn host(id: &str, label: Option<&str>, reason: impl Into<String>) -> Self {
        Self::host_with_cause(id, label, reason.into(), None)
    }
    pub(crate) fn needs_node(id: &str, label: &str, feature: &str, cause_feature: &str) -> Self {
        let message = |feature: &str| {
            format!(
                "{feature} requires Node.js. Install Node.js and make sure node is on PATH, then retry."
            )
        };
        Self::host_with_cause(
            id,
            Some(label),
            message(feature),
            Some(serde_json::json!({
                "name": "NodeRuntimeUnavailableError",
                "message": message(cause_feature)
            })),
        )
    }
    fn host_with_cause(
        id: &str,
        label: Option<&str>,
        reason: String,
        cause: Option<serde_json::Value>,
    ) -> Self {
        // TypeScript reports the cause's message in Failed status; the RPC retains its
        // feature-specific host reason.
        let detail = cause
            .as_ref()
            .and_then(|value| value["message"].as_str())
            .unwrap_or(&reason)
            .to_owned();
        Self::Device {
            wire: Box::new(DeviceError::DeviceHostUnavailableError(
                DeviceHostUnavailableError {
                    _tag: DeviceHostUnavailableErrorTag::Error,
                    host_id: id.into(),
                    host_label: label.map(|s| Some(s.into())),
                    reason,
                    cause: cause.map(Some),
                },
            )),
            reason: detail,
        }
    }
    pub fn operation(operation: &str, reason: DeviceOperationErrorReason) -> Self {
        Self::Device {
            reason: format!("Device {operation} failed ({reason:?})."),
            wire: Box::new(DeviceError::DeviceOperationError(DeviceOperationError {
                _tag: DeviceOperationErrorTag::Error,
                operation: operation.into(),
                reason,
                exit_code: None,
                cause: serde_json::Value::Null,
            })),
        }
    }
    pub fn command(operation: &str, code: i32) -> Self {
        let mut e = Self::operation(operation, DeviceOperationErrorReason::CommandFailed);
        if let Self::Device { wire, reason } = &mut e {
            *reason = format!("Device {operation} failed (exit code {code}).");
            if let DeviceError::DeviceOperationError(e) = wire.as_mut() {
                e.exit_code = Some(Some(t3_contracts::speech::SpeechNumber::Finite(
                    code.into(),
                )));
            }
        }
        e
    }
    pub fn not_found(host: &str, id: &str) -> Self {
        Self::Device {
            wire: Box::new(DeviceError::DeviceNotFoundError(DeviceNotFoundError {
                _tag: DeviceNotFoundErrorTag::Error,
                host_id: host.into(),
                device_id: id.into(),
            })),
            reason: format!("Device {id} was not found on host {host}."),
        }
    }
    pub fn platform(host: &str, platform: DevicePlatform, reason: impl Into<String>) -> Self {
        let reason = reason.into();
        Self::Device {
            wire: Box::new(DeviceError::DevicePlatformUnavailableError(
                DevicePlatformUnavailableError {
                    _tag: DevicePlatformUnavailableErrorTag::Error,
                    host_id: host.into(),
                    platform,
                    reason: reason.clone(),
                },
            )),
            reason,
        }
    }
    pub fn unsupported(operation: &str, platform: DevicePlatform, missing_helper: bool) -> Self {
        Self::Device {
            wire: Box::new(DeviceError::DeviceActionUnavailableError(
                DeviceActionUnavailableError {
                    _tag: DeviceActionUnavailableErrorTag::Error,
                    operation: operation.into(),
                    platform,
                    reason: if missing_helper {
                        DeviceActionUnavailableErrorReason::HelperMissing
                    } else {
                        DeviceActionUnavailableErrorReason::Unsupported
                    },
                },
            )),
            reason: format!("Device action {operation} is unavailable."),
        }
    }
    pub fn boot(
        host: &str,
        id: &str,
        reason: DeviceBootErrorReason,
        diagnostics: Option<String>,
    ) -> Self {
        let diagnostics = diagnostics.map(|s| bounded(&s, 2000));
        Self::Device {
            reason: format!(
                "Device {id} failed to boot ({reason:?}). {}",
                diagnostics.as_deref().unwrap_or("")
            ),
            wire: Box::new(DeviceError::DeviceBootError(DeviceBootError {
                _tag: DeviceBootErrorTag::Error,
                host_id: host.into(),
                device_id: id.into(),
                reason,
                diagnostics: diagnostics.map(Some),
                cause: serde_json::Value::Null,
            })),
        }
    }
}

pub fn bounded(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.into()
    } else {
        text.chars()
            .take(max.saturating_sub(1))
            .chain(std::iter::once('…'))
            .collect()
    }
}

pub fn invalid(operation: &str) -> Error {
    Error::operation(operation, DeviceOperationErrorReason::InvalidPayload)
}

pub fn validate_arg(value: &str, operation: &str) -> Result<()> {
    if value.is_empty() || value.chars().any(|c| c == '\0' || c == '\r' || c == '\n') {
        return Err(invalid(operation));
    }
    Ok(())
}

pub fn validate_selector(value: &str, operation: &str) -> Result<()> {
    validate_arg(value, operation)?;
    if value.starts_with('-') {
        return Err(invalid(operation));
    }
    Ok(())
}
