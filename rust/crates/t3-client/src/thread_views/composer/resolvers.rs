//! Port of apps/web/src/components/ComposerPromptEditorTiptap.tsx `composerSkillMeta` and the
//! `isKnownModel` lookup over `modelMentionKeysAtom`, the editor's chip recognition inputs.
use std::collections::BTreeSet;

use t3_contracts::server::ServerProviderSkill;

use super::model_service::ComposerModelsView;
use crate::view_logic::JsTrim;

/// Chip presentation for `$name`, or None when the provider does not offer it to users.
pub fn skill_meta(skills: &[ServerProviderSkill], name: &str) -> Option<(String, Option<String>)> {
    let skill = skills.iter().find(|skill| {
        skill.name.0 == name && skill.enabled && skill.user_invocable != Some(Some(false))
    })?;
    let raw = serde_json::to_value(skill).ok()?;
    let text = |field: &str| {
        raw[field]
            .as_str()
            .map(|value| value.js_trim().to_owned())
            .filter(|value| !value.is_empty())
    };
    Some((
        text("displayName").unwrap_or_else(|| skill.name.0.clone()),
        text("shortDescription").or_else(|| text("description")),
    ))
}

/// The selected provider's skills, as the composer's `skills` prop receives them.
pub fn selected_provider_skills(view: &ComposerModelsView) -> Vec<ServerProviderSkill> {
    view.providers
        .iter()
        .find(|provider| provider.snapshot.instance_id == view.selection.instance_id)
        .map(|provider| provider.snapshot.skills.clone())
        .unwrap_or_default()
}

/// `modelMentionKey(instanceId, model)` for every model the environment's providers offer.
pub fn model_mention_keys(view: &ComposerModelsView) -> BTreeSet<(String, String)> {
    view.providers
        .iter()
        .flat_map(|provider| {
            provider
                .options
                .iter()
                .map(move |option| (provider.snapshot.instance_id.0.clone(), option.slug.clone()))
        })
        .collect()
}
