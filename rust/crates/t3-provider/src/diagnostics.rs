use crate::sensitive::{Sensitive, ServiceError};

#[track_caller]
pub(crate) fn failure<E: 'static>(
    operation: &'static str,
    error: ServiceError,
    cause: E,
) -> ServiceError {
    record(
        operation,
        error,
        cause,
        std::panic::Location::caller().line(),
    )
}
fn record<E: 'static>(
    operation: &'static str,
    error: ServiceError,
    cause: E,
    source_line: u32,
) -> ServiceError {
    let source_kind = cause_kind(&cause);
    tracing::warn!(operation, source_line, source_kind, category = %error, cause_type = std::any::type_name::<E>(), cause = ?Sensitive::new(cause), "Provider operation failed");
    error
}
#[track_caller]
pub(crate) fn mapped<E: 'static, C: Copy + Into<ServiceError>>(
    operation: &'static str,
    error: C,
) -> impl FnOnce(E) -> C {
    let line = std::panic::Location::caller().line();
    move |cause| {
        let _ = record(operation, error.into(), cause, line);
        error
    }
}

fn cause_kind(cause: &dyn std::any::Any) -> &'static str {
    if let Some(error) = cause.downcast_ref::<std::io::Error>() {
        return match error.kind() {
            std::io::ErrorKind::NotFound => "not_found",
            std::io::ErrorKind::PermissionDenied => "permission_denied",
            std::io::ErrorKind::TimedOut => "timed_out",
            std::io::ErrorKind::AlreadyExists => "already_exists",
            _ => "io",
        };
    }
    if let Some(error) = cause.downcast_ref::<reqwest::Error>() {
        return if error.is_timeout() {
            "http_timeout"
        } else if error.is_connect() {
            "http_connect"
        } else if error.is_decode() {
            "http_decode"
        } else {
            "http"
        };
    }
    if let Some(error) = cause.downcast_ref::<serde_json::Error>() {
        return match error.classify() {
            serde_json::error::Category::Io => "json_io",
            serde_json::error::Category::Syntax => "json_syntax",
            serde_json::error::Category::Data => "json_data",
            serde_json::error::Category::Eof => "json_eof",
        };
    }
    if let Some(error) = cause.downcast_ref::<t3_process::ProcessError>() {
        return match error {
            t3_process::ProcessError::NotFound { .. } => "command_not_found",
            t3_process::ProcessError::Spawn { .. } => "process_spawn",
            t3_process::ProcessError::Io { .. } => "process_io",
            t3_process::ProcessError::NonZeroExit { .. } => "process_exit",
            t3_process::ProcessError::Timeout { .. } => "process_timeout",
            t3_process::ProcessError::Cancelled => "process_cancelled",
            t3_process::ProcessError::OutputLimit { .. } => "process_output_limit",
        };
    }
    "redacted"
}
