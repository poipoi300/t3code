use crate::{thread_views::threads::presentation::ConnectionPhase, types::EnvironmentId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkingStatus {
    pub environment_id: EnvironmentId,
    pub started_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionTone {
    Reconnecting,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionFloatingStatus {
    pub tone: ConnectionTone,
    pub label: String,
}

pub fn connection_floating_status(
    phase: ConnectionPhase,
    error: Option<&str>,
    awaiting_approval: bool,
    label: Option<&str>,
) -> Option<ConnectionFloatingStatus> {
    use ConnectionPhase::*;
    let environment = label.unwrap_or("Environment");
    let tone = if matches!(phase, Connecting | Reconnecting) {
        ConnectionTone::Reconnecting
    } else {
        ConnectionTone::Unavailable
    };
    let label = match phase {
        Reconnecting if awaiting_approval => {
            format!("Waiting for {environment} to approve this device")
        }
        Connecting | Reconnecting if error.is_none() => format!("Reconnecting to {environment}..."),
        Connecting | Reconnecting => format!("Failed to connect. Retrying {environment}..."),
        Offline => "You are offline".into(),
        Unsupported => "Client not supported".into(),
        Error => match error.filter(|s| !s.is_empty()) {
            Some(error) => format!("Failed to connect to {environment}: {error}"),
            None => format!("Failed to connect to {environment}"),
        },
        Available => format!("{environment} is not connected"),
        Connected => return None,
    };
    Some(ConnectionFloatingStatus { tone, label })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn approval_wait_and_connection_labels() {
        use ConnectionPhase::*;
        for (phase, error, approval, label, expected, tone) in [
            (
                Reconnecting,
                Some("pending"),
                true,
                Some("Host"),
                "Waiting for Host to approve this device",
                ConnectionTone::Reconnecting,
            ),
            (
                Reconnecting,
                None,
                true,
                None,
                "Waiting for Environment to approve this device",
                ConnectionTone::Reconnecting,
            ),
            (
                Connecting,
                None,
                false,
                None,
                "Reconnecting to Environment...",
                ConnectionTone::Reconnecting,
            ),
            (
                Reconnecting,
                Some("x"),
                false,
                Some("Host"),
                "Failed to connect. Retrying Host...",
                ConnectionTone::Reconnecting,
            ),
            (
                Offline,
                None,
                false,
                None,
                "You are offline",
                ConnectionTone::Unavailable,
            ),
            (
                Unsupported,
                None,
                false,
                None,
                "Client not supported",
                ConnectionTone::Unavailable,
            ),
            (
                Error,
                Some("refused"),
                false,
                Some("Host"),
                "Failed to connect to Host: refused",
                ConnectionTone::Unavailable,
            ),
            (
                Error,
                None,
                false,
                Some("Host"),
                "Failed to connect to Host",
                ConnectionTone::Unavailable,
            ),
            (
                Available,
                None,
                false,
                Some("Host"),
                "Host is not connected",
                ConnectionTone::Unavailable,
            ),
        ] {
            assert_eq!(
                connection_floating_status(phase, error, approval, label),
                Some(ConnectionFloatingStatus {
                    tone,
                    label: expected.into()
                })
            );
        }
        assert_eq!(
            connection_floating_status(Connected, None, true, None),
            None
        );
    }
}
