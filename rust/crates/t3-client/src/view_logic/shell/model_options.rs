use super::*;
pub fn model_capabilities(
    provider: &ServerProvider,
    model: Option<&str>,
    include_plan: bool,
) -> ModelCapabilities {
    let mut caps = crate::view_logic::chat_view::resolve_selectable_model(
        &provider.driver.0,
        model,
        &provider.models,
    )
    .and_then(|m| m.capabilities.clone())
    .unwrap_or(ModelCapabilities {
        option_descriptors: Some(Some(vec![])),
    });
    if !include_plan && let Some(Some(descriptors)) = &mut caps.option_descriptors {
        descriptors.retain_mut(|d| {
            let ProviderOptionDescriptor::Select(d) = d else {
                return true;
            };
            if d.id != "agent" {
                return true;
            }
            d.options.retain(|o| o.id != "plan");
            if d.options.is_empty() {
                return false;
            }
            if !d
                .current_value
                .as_ref()
                .and_then(Option::as_ref)
                .is_some_and(|s| d.options.iter().any(|o| &o.id == s))
            {
                d.current_value = d
                    .options
                    .iter()
                    .find(|o| o.is_default == Some(Some(true)))
                    .or_else(|| d.options.first())
                    .map(|o| Some(o.id.clone()));
            }
            true
        });
    }
    caps
}
pub fn dispatch_options(
    provider: &ServerProvider,
    model: &str,
    options: Option<&ProviderOptionSelections>,
    include_plan: bool,
) -> Option<ProviderOptionSelections> {
    if matches!(provider.driver.0.as_str(), "opencode" | "cursor")
        && crate::view_logic::chat_view::resolve_selectable_model(
            &provider.driver.0,
            Some(model),
            &provider.models,
        )
        .is_none()
    {
        let preserved = options
            .into_iter()
            .flat_map(|o| &o.0)
            .filter(|o| {
                include_plan
                    || o.id != "agent"
                    || o.value != ProviderOptionSelectionValue::String("plan".into())
            })
            .cloned()
            .collect::<Vec<_>>();
        return (!preserved.is_empty()).then_some(ProviderOptionSelections(preserved));
    }
    let caps = model_capabilities(provider, Some(model), include_plan);
    let descriptors = caps
        .option_descriptors
        .as_ref()
        .and_then(Option::as_deref)
        .unwrap_or(&[]);
    let mut options = options.map(|o| o.0.clone()).unwrap_or_default();
    let raw = |id: &str, options: &[ProviderOptionSelection]| {
        options.iter().find(|o| o.id == id).map(|o| o.value.clone())
    };
    if raw("effort", &options) == Some(ProviderOptionSelectionValue::String("ultracode".into()))
        && descriptors
            .iter()
            .any(|d| matches!(d,ProviderOptionDescriptor::Boolean(d)if d.id=="ultracode"))
    {
        for o in &mut options {
            if o.id == "effort"
                && o.value == ProviderOptionSelectionValue::String("ultracode".into())
            {
                o.value = ProviderOptionSelectionValue::String("xhigh".into());
            }
        }
        if raw("ultracode", &options).is_none() {
            options.push(ProviderOptionSelection {
                id: "ultracode".into(),
                value: ProviderOptionSelectionValue::Boolean(true),
            });
        }
    }
    if raw("fastMode", &options).is_none()
        && descriptors
            .iter()
            .any(|d| matches!(d,ProviderOptionDescriptor::Boolean(d)if d.id=="fastMode"))
    {
        options.push(ProviderOptionSelection {
            id: "fastMode".into(),
            value: ProviderOptionSelectionValue::Boolean(false),
        });
    }
    let normalized = descriptors
        .iter()
        .filter_map(|descriptor| match descriptor {
            ProviderOptionDescriptor::Boolean(d) => {
                let stated = raw(&d.id, &options)?;
                let value = if let ProviderOptionSelectionValue::Boolean(value) = stated {
                    Some(value)
                } else {
                    d.current_value.flatten()
                };
                Some(ProviderOptionSelection {
                    id: d.id.clone(),
                    value: ProviderOptionSelectionValue::Boolean(value?),
                })
            }
            ProviderOptionDescriptor::Select(d) => {
                let stated = raw(&d.id, &options)?;
                let default = || {
                    d.current_value
                        .as_ref()
                        .and_then(Option::as_deref)
                        .or_else(|| {
                            d.options
                                .iter()
                                .find(|o| o.is_default == Some(Some(true)))
                                .map(|o| o.id.as_str())
                        })
                };
                let value = match &stated {
                    ProviderOptionSelectionValue::String(s) => {
                        let s = s.js_trim();
                        if s.is_empty() {
                            default()
                        } else if d.options.is_empty() {
                            Some(s)
                        } else if d
                            .prompt_injected_values
                            .as_ref()
                            .and_then(Option::as_ref)
                            .is_some_and(|v| v.iter().any(|v| v == s))
                            && d.options.iter().any(|o| o.id == s)
                        {
                            d.options
                                .iter()
                                .find(|o| o.is_default == Some(Some(true)))
                                .map(|o| o.id.as_str())
                        } else if d.options.iter().any(|o| o.id == s) {
                            Some(s)
                        } else {
                            default()
                        }
                    }
                    _ => default(),
                };
                Some(ProviderOptionSelection {
                    id: d.id.clone(),
                    value: ProviderOptionSelectionValue::String(value?.into()),
                })
            }
        })
        .collect::<Vec<_>>();
    (!normalized.is_empty()).then_some(ProviderOptionSelections(normalized))
}
fn version(word: &str) -> bool {
    let word = word.strip_prefix(['v', 'V']).unwrap_or(word);
    let word = word
        .strip_suffix(|c: char| c.is_ascii_alphabetic())
        .unwrap_or(word);
    !word.is_empty()
        && word
            .split('.')
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}
fn non_version_words(text: &str) -> Vec<String> {
    let mut inside = false;
    let mut stripped = String::new();
    for c in text.chars() {
        if c == '(' && !inside {
            inside = true;
            stripped.push(' ');
        } else if c == ')' && inside {
            inside = false;
        } else if !inside {
            stripped.push(c);
        }
    }
    stripped
        .split(|c| crate::view_logic::is_js_whitespace(c) || c == '-')
        .filter(|s| !s.is_empty() && !version(s))
        .map(str::to_owned)
        .collect()
}
fn names_model(name: &str) -> bool {
    non_version_words(name).iter().any(|word| {
        !matches!(
            word.to_lowercase().as_str(),
            "pro"
                | "flash"
                | "mini"
                | "max"
                | "lite"
                | "nano"
                | "ultra"
                | "plus"
                | "fast"
                | "turbo"
                | "preview"
                | "latest"
                | "thinking"
                | "reasoning"
                | "build"
                | "code"
                | "image"
                | "vision"
                | "audio"
                | "live"
                | "custom"
                | "tools"
                | "exp"
                | "experimental"
                | "chat"
                | "tts"
                | "stt"
                | "non"
                | "transcribe"
        )
    })
}
pub fn short_name(name: &str, explicit: Option<&str>, driver: &str) -> Option<String> {
    if let Some(explicit) = explicit.filter(|s| !s.is_empty()) {
        return Some(explicit.into());
    }
    let unprefixed = name
        .split_once(": ")
        .filter(|(vendor, rest)| {
            !rest
                .chars()
                .next()
                .is_none_or(crate::view_logic::is_js_whitespace)
                && vendor.len() <= 25
                && vendor
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_uppercase)
                && vendor
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.&- ".contains(&b))
        })
        .map_or(name, |(_, rest)| rest);
    let family = ["Claude ", "GPT-", "GPT ", "Gemini ", "Muse ", "Grok "]
        .into_iter()
        .find(|prefix| unprefixed.starts_with(prefix));
    let native = |prefix: &str| {
        matches!(
            (driver, prefix),
            ("codex", "GPT-" | "GPT ")
                | ("claudeAgent", "Claude ")
                | ("antigravity", "Gemini ")
                | ("cursor" | "grok", "Grok ")
                | ("muse", "Muse ")
        )
    };
    let short = family
        .and_then(|prefix| {
            let rest = unprefixed[prefix.len()..].js_trim();
            let keep = if native(prefix) {
                !non_version_words(rest).is_empty()
            } else {
                names_model(rest)
            };
            keep.then(|| {
                if rest.chars().any(crate::view_logic::is_js_whitespace) {
                    rest.into()
                } else {
                    rest.replace('-', " ")
                }
            })
        })
        .unwrap_or_else(|| {
            if names_model(unprefixed) {
                unprefixed.into()
            } else {
                name.into()
            }
        });
    (short != name).then_some(short)
}
