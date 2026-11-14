//! Port of apps/web/src/components/chat/SubagentTooltipContent.tsx selection presentation.

use t3_contracts::{
    model::ProviderOptionSelectionValue, model_selection::ModelSelection, server::ServerProvider,
};

use super::threads::execution_presentation::REASONING_EFFORT_OPTION_IDS;
use super::threads::subagent_display::{
    MetadataInput, SubagentMetadata, resolve_subagent_metadata,
};

pub fn inspector_metadata(input: MetadataInput<'_>) -> SubagentMetadata {
    let provider = input.provider;
    let mut metadata = resolve_subagent_metadata(input, format_model_slug_name);
    if let Some(provider) = provider
        && let Some(model) = provider
            .models
            .iter()
            .find(|m| Some(m.slug.0.as_str()) == metadata.model_slug.as_deref())
    {
        metadata.model_label =
            super::timeline_events::display_model_name(model, Some(&provider.driver.0));
    }
    metadata
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Speed {
    Fast,
    Ultrafast,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Traits {
    pub effort: Option<String>,
    pub speed: Option<Speed>,
}

/// Port of packages/shared/src/model.ts formatModelSlugName and formatCodexModelName.
pub fn format_model_slug_name(slug: &str) -> String {
    let separator = slug.rfind('/').map_or(0, |at| at + 1);
    let (prefix, name) = slug.split_at(separator);
    let lower = name.to_ascii_lowercase();
    if lower
        .strip_prefix("gpt-")
        .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
    {
        let mut label = String::from("GPT");
        let mut after_dash = false;
        for ch in name[3..].chars() {
            label.push(if after_dash && ch.is_ascii_lowercase() {
                ch.to_ascii_uppercase()
            } else {
                ch
            });
            after_dash = ch == '-';
        }
        return format!("{prefix}{label}");
    }
    let family = [
        "claude-opus-",
        "claude-sonnet-",
        "claude-haiku-",
        "claude-fable-",
        "gemini-",
        "grok-",
        "composer-",
    ]
    .into_iter()
    .find(|family| {
        lower
            .strip_prefix(family)
            .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
    });
    let Some(family) = family else {
        return slug.into();
    };
    let mut label = name.to_owned();
    if family.starts_with("claude-") {
        let rest = &name[family.len()..];
        let major = rest.bytes().take_while(u8::is_ascii_digit).count();
        if let Some(minor) = rest[major..].strip_prefix('-') {
            let digits = minor.bytes().take_while(u8::is_ascii_digit).count();
            if (1..=2).contains(&digits)
                && minor[digits..]
                    .chars()
                    .next()
                    .is_none_or(|ch| matches!(ch, '-' | '['))
            {
                label.replace_range(family.len() + major..family.len() + major + 1, ".");
            }
        }
    }
    let label = label
        .split('-')
        .map(|part| {
            let mut chars = part.chars();
            chars.next().map_or_else(String::new, |first| {
                // charAt(0) uppercases one UTF-16 unit, leaving astral initials unchanged.
                if first.len_utf16() > 1 {
                    part.to_owned()
                } else {
                    first.to_uppercase().chain(chars).collect()
                }
            })
        })
        .collect::<Vec<_>>()
        .join(" ");
    format!("{prefix}{label}")
}

pub fn inspector_traits(
    selection: Option<&ModelSelection>,
    model_slug: Option<&str>,
    provider: Option<&ServerProvider>,
    instance: &str,
) -> Traits {
    let Some(selection) = selection else {
        return Traits::default();
    };
    let resolved = provider
        .and_then(|p| {
            crate::view_logic::chat_view::resolve_selectable_model(
                &p.driver.0,
                Some(&selection.model),
                &p.models,
            )
        })
        .map_or(selection.model.trim(), |m| m.slug.0.as_str());
    if Some(resolved) != model_slug || selection.instance_id.0 != instance {
        return Traits::default();
    }
    let value = |id: &str| {
        selection
            .options
            .as_ref()?
            .0
            .iter()
            .find(|o| o.id == id)
            .map(|o| &o.value)
    };
    let effort = REASONING_EFFORT_OPTION_IDS
        .iter()
        .find_map(|id| match value(id) {
            Some(ProviderOptionSelectionValue::String(s)) if !s.is_empty() => Some(s.clone()),
            _ => None,
        });
    let speed = provider
        .and_then(|p| {
            p.models
                .iter()
                .find(|m| Some(m.slug.0.as_str()) == model_slug)
        })
        .and_then(|m| m.capabilities.as_ref())
        .and_then(|c| c.option_descriptors.as_ref()?.as_ref())
        .and_then(|descriptors| {
            descriptors.iter().find_map(|descriptor| {
                use t3_contracts::model::ProviderOptionDescriptor;
                match descriptor {
                    ProviderOptionDescriptor::Boolean(d) if d.id == "fastMode" => {
                        match value(&d.id) {
                            Some(ProviderOptionSelectionValue::Boolean(true)) => {
                                Some(Some(Speed::Fast))
                            }
                            Some(ProviderOptionSelectionValue::Boolean(false)) => Some(None),
                            _ => None,
                        }
                    }
                    ProviderOptionDescriptor::Select(d) if d.id == "serviceTier" => {
                        match value(&d.id) {
                            Some(ProviderOptionSelectionValue::String(v))
                                if d.options.iter().any(|o| o.id == *v) =>
                            {
                                let fast = d.options.iter().find(|o| o.label == "Fast");
                                let ultra = d.options.iter().find(|o| o.label == "Ultrafast");
                                if (v == "default" && (fast.is_some() || ultra.is_some()))
                                    || fast.is_some_and(|o| o.id == *v)
                                    || ultra.is_some_and(|o| o.id == *v)
                                {
                                    Some(if ultra.is_some_and(|o| o.id == *v) {
                                        Some(Speed::Ultrafast)
                                    } else if fast.is_some_and(|o| o.id == *v) {
                                        Some(Speed::Fast)
                                    } else {
                                        None
                                    })
                                } else {
                                    None
                                }
                            }
                            _ => None,
                        }
                    }
                    _ => None,
                }
            })
        })
        .flatten();
    Traits { effort, speed }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn metadata_uses_catalog_short_names_and_formats_uncatalogued_slugs() {
        let provider = serde_json::from_value(json!({"instanceId":"account","driver":"codex","enabled":true,"installed":true,"version":null,"status":"ready","auth":{"status":"authenticated"},"checkedAt":"2026-11-13T12:34:00Z","models":[{"slug":"gpt-6.1-sol","name":"GPT-6.1-Sol","isCustom":false,"capabilities":null}],"slashCommands":[],"skills":[]})).expect("provider");
        for (model, provider, expected) in [
            (Some("gpt-6.1-sol"), Some(&provider), "6.1 Sol"),
            (Some("gpt-6.1-sol"), None, "GPT-6.1-Sol"),
            (Some("Copy"), None, "Copy"),
            (None, None, "Not reported"),
        ] {
            let metadata = inspector_metadata(MetadataInput {
                model,
                selection: None,
                provider,
                parent_thread: None,
                child_thread: None,
                parent_project: None,
                child_project: None,
            });
            assert_eq!(metadata.model_label, expected);
        }
    }

    #[test]
    fn unknown_catalog_models_use_the_source_slug_formatting_without_changing_provider_prefixes() {
        for (slug, label) in [
            ("openai/gpt-6.1-sol", "openai/GPT-6.1-Sol"),
            ("GPT-6-CODEX", "GPT-6-CODEX"),
            ("gpt-custom", "gpt-custom"),
            (
                "vendor/claude-haiku-4-5-20251001",
                "vendor/Claude Haiku 4.5 20251001",
            ),
            ("claude-opus-4-12[1m]", "Claude Opus 4.12[1m]"),
            ("claude-fable-5-1", "Claude Fable 5.1"),
            ("claude-sonnet-4-123", "Claude Sonnet 4 123"),
            ("claude-opus-4-5x", "Claude Opus 4 5x"),
            ("claude-other-4-5", "claude-other-4-5"),
            ("gemini-3-1-pro", "Gemini 3 1 Pro"),
            ("grok-4-fast", "Grok 4 Fast"),
            ("composer-2", "Composer 2"),
            ("my-model", "my-model"),
            ("Copy", "Copy"),
            ("claude-opus-4-𐐨", "Claude Opus 4 𐐨"),
        ] {
            assert_eq!(format_model_slug_name(slug), label);
        }
    }

    #[test]
    fn traits_require_both_the_reported_model_and_account_and_keep_raw_effort() {
        let selection = serde_json::from_value(json!({"instanceId":"account","model":"model","options":[{"id":"effort","value":"Settings"}]})).expect("selection");
        assert_eq!(
            inspector_traits(Some(&selection), Some("model"), None, "account")
                .effort
                .as_deref(),
            Some("Settings")
        );
        for (slug, instance) in [
            (Some("other"), "account"),
            (Some("model"), "other"),
            (None, "account"),
        ] {
            assert_eq!(
                inspector_traits(Some(&selection), slug, None, instance),
                Traits::default()
            );
        }
    }

    #[test]
    fn speed_uses_saved_valid_values_and_the_first_matching_descriptor() {
        let provider = serde_json::from_value(json!({"instanceId":"account","driver":"codex","enabled":true,"installed":true,"version":null,"status":"ready","auth":{"status":"authenticated"},"checkedAt":"2026-11-13T12:34:00Z","models":[{"slug":"model","name":"Model","isCustom":false,"capabilities":{"optionDescriptors":[{"id":"fastMode","type":"boolean","label":"Fast"},{"id":"serviceTier","type":"select","label":"Tier","options":[{"id":"default","label":"Normal"},{"id":"fast","label":"Fast"},{"id":"ultra","label":"Ultrafast"}]}]}}],"slashCommands":[],"skills":[]})).expect("provider");
        for (options, expected) in [
            (json!([{"id":"fastMode","value":true}]), Some(Speed::Fast)),
            (
                json!([{"id":"serviceTier","value":"ultra"}]),
                Some(Speed::Ultrafast),
            ),
            (json!([{"id":"serviceTier","value":"invalid"}]), None),
            (json!([{"id":"serviceTier","value":"default"}]), None),
            (
                json!([{"id":"fastMode","value":false},{"id":"serviceTier","value":"ultra"}]),
                None,
            ),
        ] {
            let selection = serde_json::from_value(
                json!({"instanceId":"account","model":"model","options":options}),
            )
            .expect("selection");
            assert_eq!(
                inspector_traits(Some(&selection), Some("model"), Some(&provider), "account").speed,
                expected
            );
        }
    }
}
