use super::super::outbox::{QueuedMessage, retry_delay_ms};
use crate::types::{EnvironmentId, ThreadKey};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use t3_contracts::{
    base_schemas::MessageId,
    model::{
        ProviderOptionDescriptor, ProviderOptionSelection, ProviderOptionSelectionValue,
        ProviderOptionSelections,
    },
    model_selection::ModelSelection,
    provider_policy::{ProviderInteractionMode, RuntimeMode},
    server::ServerProvider,
};

#[derive(Debug, Clone, PartialEq)]
pub struct ThreadSettings {
    pub model_selection: ModelSelection,
    pub runtime_mode: RuntimeMode,
    pub interaction_mode: ProviderInteractionMode,
}
pub fn resolve_settings(
    message: &QueuedMessage,
    thread: &ThreadSettings,
    providers: &[ServerProvider],
) -> ThreadSettings {
    let mut model = message
        .model_selection
        .as_ref()
        .unwrap_or(&thread.model_selection)
        .clone();
    let provider = providers
        .iter()
        .find(|p| p.instance_id == model.instance_id);
    let capabilities = provider
        .and_then(|p| p.models.iter().find(|m| m.slug.0 == model.model))
        .and_then(|m| m.capabilities.as_ref());
    let explicit = model
        .options
        .as_ref()
        .is_some_and(|o| o.0.iter().any(|o| o.id == "fastMode"));
    if !explicit
        && capabilities
            .and_then(|c| c.option_descriptors.as_ref())
            .and_then(Option::as_ref)
            .is_some_and(|descriptors| {
                descriptors
                    .iter()
                    .any(|d| matches!(d,ProviderOptionDescriptor::Boolean(d) if d.id=="fastMode"))
            })
    {
        model
            .options
            .get_or_insert_with(|| ProviderOptionSelections(vec![]))
            .0
            .push(ProviderOptionSelection {
                id: "fastMode".into(),
                value: ProviderOptionSelectionValue::Boolean(false),
            });
    }
    ThreadSettings {
        model_selection: model,
        runtime_mode: message.runtime_mode.unwrap_or(thread.runtime_mode),
        interaction_mode: if provider
            .is_some_and(|p| p.show_interaction_mode_toggle == Some(Some(false)))
        {
            ProviderInteractionMode::Default
        } else {
            message.interaction_mode.unwrap_or(thread.interaction_mode)
        },
    }
}
const NETWORK_HINT: &str = "Your DNS or firewall may be blocking T3 Connect. Try another network, such as a different Wi-Fi or mobile data.";
const UNRESPONSIVE_HINT: &str = "It may be overloaded, asleep, or off the network.";
fn word_contains(message: &str, pattern: &str) -> bool {
    message.match_indices(pattern).any(|(at, _)| {
        message[..at]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_')
            && message[at + pattern.len()..]
                .chars()
                .next()
                .is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_')
    })
}
pub fn transport_error_message(message: &str) -> bool {
    let message = message.trim().to_ascii_lowercase();
    if [
        "socketcloseerror",
        "socketopenerror",
        "socket is not connected",
        "clientprotocolerror",
        "rpcclienterror",
        "ping timeout",
    ]
    .iter()
    .any(|p| word_contains(&message, p))
        || message.contains("unable to connect to the t3 server websocket.")
    {
        return true;
    }
    let network = format!(" {}", NETWORK_HINT.to_ascii_lowercase());
    let suffix = message.strip_suffix(&network).unwrap_or(&message);
    if [
        "is not connected.",
        "disconnected.",
        "stopped responding.",
        "could not establish a websocket connection.",
    ]
    .iter()
    .any(|p| {
        suffix.ends_with(p)
            && suffix[..suffix.len() - p.len()]
                .chars()
                .next_back()
                .is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_')
    }) {
        return true;
    }
    let hint = format!(" {}", UNRESPONSIVE_HINT.to_ascii_lowercase());
    if let Some(prefix) = message
        .strip_suffix(&hint)
        .and_then(|m| m.strip_suffix("s."))
        && let Some((_, seconds)) = prefix.rsplit_once("stopped responding for ")
    {
        return !seconds.is_empty() && seconds.bytes().all(|b| b.is_ascii_digit());
    }
    false
}
pub fn should_retry(error: &Value) -> bool {
    match error.get("_tag").and_then(Value::as_str) {
        Some(
            "OrchestrationDispatchCommandError"
            | "OrchestrationV2DispatchCommandError"
            | "EnvironmentAuthorizationError",
        ) => false,
        Some(
            "ConnectionTransientError"
            | "RpcClientError"
            | "EnvironmentRpcUnavailableError"
            | "EnvironmentNotRegisteredError",
        ) => true,
        _ => error
            .as_str()
            .or_else(|| error.get("message").and_then(Value::as_str))
            .is_some_and(transport_error_message),
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandStage {
    SettingsSync,
    StartTurn,
    Upload,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureAction {
    Retry,
    Restore,
}
pub fn failure_action(stage: CommandStage, error: &Value, interrupted: bool) -> FailureAction {
    if stage == CommandStage::SettingsSync || interrupted || should_retry(error) {
        FailureAction::Retry
    } else {
        FailureAction::Restore
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Retry {
    pub attempt: u32,
    pub not_before: u64,
}
#[derive(Debug, Default)]
pub struct DeliverySchedule {
    pub retries: BTreeMap<MessageId, Retry>,
    pub editing: BTreeSet<MessageId>,
    pub blocked: BTreeSet<MessageId>,
    in_flight: Option<MessageId>,
    disposed: bool,
}
impl DeliverySchedule {
    pub fn retry(&mut self, id: MessageId, now: u64) {
        let attempt = self
            .retries
            .get(&id)
            .map_or(1, |r| r.attempt.saturating_add(1));
        self.retries.insert(
            id,
            Retry {
                attempt,
                not_before: now.saturating_add(retry_delay_ms(attempt)),
            },
        );
    }
    pub fn next<'a>(
        &self,
        messages: &'a [QueuedMessage],
        now: u64,
        operable: &BTreeSet<EnvironmentId>,
    ) -> Option<&'a QueuedMessage> {
        if self.disposed || self.in_flight.is_some() {
            return None;
        }
        let mut seen = BTreeSet::<ThreadKey>::new();
        messages
            .iter()
            .filter(|message| seen.insert(message.thread_key()))
            .find(|m| {
                operable.contains(&m.environment_id)
                    && !self.editing.contains(&m.message_id)
                    && !self.blocked.contains(&m.message_id)
                    && self
                        .retries
                        .get(&m.message_id)
                        .is_none_or(|r| r.not_before <= now)
            })
    }
    pub fn begin(&mut self, id: MessageId) -> bool {
        if self.disposed || self.in_flight.is_some() {
            false
        } else {
            self.in_flight = Some(id);
            true
        }
    }
    pub fn finish(&mut self, id: &MessageId, accepted: bool) {
        if self.in_flight.as_ref() == Some(id) {
            self.in_flight = None;
        }
        if accepted {
            self.retries.remove(id);
        }
    }
    pub fn deadline(&self) -> Option<u64> {
        if self.disposed {
            None
        } else {
            self.retries
                .iter()
                .filter(|(id, _)| !self.editing.contains(*id) && !self.blocked.contains(*id))
                .map(|(_, r)| r.not_before)
                .min()
        }
    }
    pub fn wake(&mut self, environment: &EnvironmentId, messages: &[QueuedMessage]) {
        for message in messages.iter().filter(|m| &m.environment_id == environment) {
            self.retries.remove(&message.message_id);
        }
    }
    pub fn dispose(&mut self) {
        self.disposed = true;
        self.in_flight = None;
        self.retries.clear();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn full_retry_classification() {
        for tag in [
            "ConnectionTransientError",
            "RpcClientError",
            "EnvironmentRpcUnavailableError",
            "EnvironmentNotRegisteredError",
        ] {
            assert!(should_retry(&json!({"_tag":tag})));
        }
        for tag in [
            "OrchestrationDispatchCommandError",
            "OrchestrationV2DispatchCommandError",
            "EnvironmentAuthorizationError",
        ] {
            assert!(!should_retry(
                &json!({"_tag":tag,"message":"SocketCloseError"})
            ));
        }
        for message in [
            "SocketCloseError",
            "Socket is not connected",
            "host disconnected.",
            "ping timeout",
            "Unable to connect to the T3 server WebSocket.",
        ] {
            assert!(transport_error_message(message), "{message}");
        }
        for message in [
            "invalid payload",
            "notdisconnected.",
            "host disconnected. This needs review.",
            "",
        ] {
            assert!(!transport_error_message(message), "{message}");
        }
        assert_eq!(
            failure_action(
                CommandStage::SettingsSync,
                &json!({"_tag":"EnvironmentAuthorizationError"}),
                false
            ),
            FailureAction::Retry
        );
        assert_eq!(
            failure_action(CommandStage::StartTurn, &json!("invalid payload"), false),
            FailureAction::Restore
        );
        assert_eq!(
            failure_action(CommandStage::StartTurn, &json!("invalid payload"), true),
            FailureAction::Retry
        );
    }
    #[test]
    fn bounded_retries_and_disposal() {
        let mut schedule = DeliverySchedule::default();
        let id = MessageId("m".into());
        schedule.retry(id.clone(), 100);
        assert_eq!(schedule.deadline(), Some(1100));
        schedule.retry(id.clone(), 200);
        assert_eq!(schedule.deadline(), Some(2200));
        schedule.editing.insert(id.clone());
        assert_eq!(schedule.deadline(), None);
        schedule.editing.clear();
        assert!(schedule.begin(id.clone()));
        assert!(!schedule.begin(MessageId("other".into())));
        schedule.finish(&id, true);
        assert!(schedule.retries.is_empty());
        schedule.dispose();
        assert!(!schedule.begin(id));
    }
}
