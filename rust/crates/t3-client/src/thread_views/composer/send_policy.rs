use super::delivery_policy::ThreadSettings;
use crate::thread_views::outbox::QueuedMessage;
use t3_contracts::{base_schemas::ProjectId, provider_policy::RuntimeMode, server::ServerConfig};

fn rank(mode: RuntimeMode) -> u8 {
    match mode {
        RuntimeMode::ApprovalRequired => 0,
        RuntimeMode::AutoAcceptEdits => 1,
        RuntimeMode::Auto => 2,
        RuntimeMode::FullAccess => 3,
    }
}
fn mode_label(mode: RuntimeMode) -> &'static str {
    match mode {
        RuntimeMode::ApprovalRequired => "Supervised",
        RuntimeMode::AutoAcceptEdits => "Auto-accept edits",
        RuntimeMode::Auto => "Auto",
        RuntimeMode::FullAccess => "Full access",
    }
}
pub(super) fn block_reason(
    config: &ServerConfig,
    project: Option<&ProjectId>,
    message: &QueuedMessage,
    settings: &ThreadSettings,
) -> Option<String> {
    let selection = &settings.model_selection;
    let instance = &selection.instance_id;
    let provider = config
        .providers
        .0
        .iter()
        .find(|provider| &provider.instance_id == instance);
    let configured = config.settings.provider_instances.get(instance);
    let override_enabled = project
        .and_then(|id| config.settings.project_settings_overrides.get(id))
        .and_then(|overrides| overrides.provider_instances.as_ref())
        .and_then(|instances| instances.get(instance))
        .and_then(|instance| instance.enabled);
    let enabled = override_enabled.unwrap_or_else(|| {
        configured.map_or(
            matches!(instance.0.as_str(), "codex" | "claudeAgent"),
            |instance| {
                let flag = instance
                    .config
                    .as_ref()
                    .and_then(|config| config.get("enabled"))
                    .and_then(serde_json::Value::as_bool);
                instance.enabled != Some(false)
                    && flag != Some(false)
                    && instance.enabled.or(flag).unwrap_or(!matches!(
                        instance.driver.0.as_str(),
                        "cursor"
                            | "grok"
                            | "fx"
                            | "kiro"
                            | "muse"
                            | "pi"
                            | "opencode"
                            | "antigravity"
                    ))
            },
        )
    });
    if override_enabled == Some(false) || provider.is_some() && !enabled {
        return Some("Model turned off for this project. Turn it back on from web or desktop, or choose another model.".into());
    }
    let driver = provider
        .map(|provider| &provider.driver)
        .or_else(|| configured.map(|instance| &instance.driver));
    if let Some(policy) = &config.managed_policy {
        if let Some(ceiling) = policy
            .limits
            .as_ref()
            .and_then(|limits| limits.max_runtime_mode)
            && rank(settings.runtime_mode) > rank(ceiling)
        {
            return Some(format!(
                "{} limits threads to {}.",
                policy.organization.0,
                mode_label(ceiling)
            ));
        }
        if let Some(allowed) = policy
            .limits
            .as_ref()
            .and_then(|limits| limits.providers.as_ref())
            .and_then(|providers| providers.allowed_drivers.as_ref())
            && driver.is_some_and(|driver| !allowed.contains(driver))
        {
            return Some(format!(
                "{}'s policy does not allow this provider. Choose another model.",
                policy.organization.0
            ));
        }
    }
    if driver.is_some_and(|driver| driver.0 == "antigravity")
        && provider.is_none_or(|provider| {
            !provider.installed
                || serde_json::to_value(&provider.auth)
                    .ok()
                    .is_some_and(|auth| auth["status"] == "unauthenticated")
                || serde_json::to_value(provider.availability)
                    .ok()
                    .is_some_and(|value| value == "unavailable")
                || !provider
                    .models
                    .iter()
                    .any(|model| model.slug.0 == selection.model)
        })
    {
        return Some("Antigravity model unavailable. Set up Antigravity on web or desktop, or choose another model.".into());
    }
    if message.attachments.len() > t3_contracts::chat_attachment::PROVIDER_SEND_TURN_MAX_ATTACHMENTS
    {
        return Some("Too many attachments. Remove some before sending.".into());
    }
    let max = config
        .environment
        .capabilities
        .file_attachments
        .as_ref()
        .and_then(|capability| u64::try_from(capability.max_upload_bytes).ok());
    if let crate::thread_views::outbox::DispatchStep::Restore(reason) =
        crate::thread_views::outbox::dispatch_step(
            crate::thread_views::outbox::DeliveryAction::Send,
            &message.attachments,
            Some(max),
        )
    {
        return Some(reason);
    }
    if let Some(image) = message.attachments.iter().find(|attachment| {
        !attachment.is_file()
            && attachment.size_bytes()
                > t3_contracts::chat_attachment::PROVIDER_SEND_TURN_MAX_IMAGE_BYTES as f64
    }) {
        return Some(format!("'{}' exceeds the 10 MB image limit.", image.name()));
    }
    None
}
