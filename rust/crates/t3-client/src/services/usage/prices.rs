//! A price or mapping edit applies to each destination's own settings.
use super::super::settings::scoped::SettingsPatch;
use serde_json::{Value, json};
use t3_contracts::{base_schemas::TrimmedNonEmptyString, server::ServerConfig};
pub const FIELDS: [&str; 4] = [
    "inputCostPerMillionTokens",
    "outputCostPerMillionTokens",
    "cacheReadCostPerMillionTokens",
    "cacheWriteCostPerMillionTokens",
];
#[derive(Clone, Default, Debug)]
pub struct PriceEdit {
    pub model: String,
    pub rates: [Option<String>; 4],
    pub alias: Option<String>,
    pub removed: bool,
}
pub fn cells(config: &ServerConfig, model: &str) -> [String; 5] {
    let id = TrimmedNonEmptyString(model.into());
    let p = config.settings.usage_price_overrides.get(&id);
    let rates = p
        .map(|p| {
            [
                Some(&p.input_cost_per_million_tokens),
                Some(&p.output_cost_per_million_tokens),
                p.cache_read_cost_per_million_tokens.as_ref(),
                p.cache_write_cost_per_million_tokens.as_ref(),
            ]
        })
        .unwrap_or([None; 4]);
    let value = |i: usize| rates[i].map(|n| n.value().to_string()).unwrap_or_default();
    [
        value(0),
        value(1),
        value(2),
        value(3),
        config
            .settings
            .usage_model_aliases
            .get(&id)
            .map(|s| s.0.clone())
            .unwrap_or_default(),
    ]
}
pub fn patch(config: &ServerConfig, edit: &PriceEdit) -> Result<SettingsPatch, String> {
    let model = edit.model.trim();
    if model.is_empty() {
        return Err("Enter a model ID.".into());
    }
    let current = cells(config, model);
    let mut patch = SettingsPatch::new();
    let alias = edit.alias.as_deref().unwrap_or(&current[4]).trim();
    let has_alias = !current[4].is_empty();
    let has_price = !current[0].is_empty();
    if edit.removed {
        if has_price {
            patch.insert("usagePriceOverrides".into(), json!({model:Value::Null}));
        }
        if has_alias {
            patch.insert("usageModelAliases".into(), json!({model:Value::Null}));
        }
        return Ok(patch);
    }
    if !alias.is_empty() {
        if edit.rates.iter().flatten().any(|s| !s.trim().is_empty()) {
            return Err("Clear the mapping before setting prices.".into());
        }
        if config.environment.capabilities.usage_model_aliases != Some(true) {
            return Err("Update server to map models".into());
        }
        let mut seen = std::collections::BTreeSet::from([model]);
        let mut next = alias;
        loop {
            if !seen.insert(next) {
                return Err(format!(
                    "Model mappings cannot form a cycle on {}.",
                    config.environment.label.0
                ));
            }
            match config
                .settings
                .usage_model_aliases
                .get(&TrimmedNonEmptyString(next.into()))
            {
                Some(to) => next = &to.0,
                None => break,
            }
        }
        if current[4] != alias {
            patch.insert("usageModelAliases".into(), json!({model:alias}));
        }
        if has_price {
            patch.insert("usagePriceOverrides".into(), json!({model:Value::Null}));
        }
        return Ok(patch);
    }
    if has_alias {
        patch.insert("usageModelAliases".into(), json!({model:Value::Null}));
    }
    if has_alias && !has_price && edit.rates.iter().flatten().all(|s| s.trim().is_empty()) {
        return Ok(patch);
    }
    let mut price = serde_json::Map::new();
    for (i, key) in FIELDS.iter().enumerate() {
        let raw = edit.rates[i]
            .as_deref()
            .unwrap_or(&current[i])
            .trim()
            .replace(',', ".");
        if raw.is_empty() {
            if i < 2 {
                return Err(format!(
                    "{} is required on {}.",
                    if i == 0 { "Input" } else { "Output" },
                    config.environment.label.0
                ));
            }
            continue;
        }
        let value = raw
            .parse::<f64>()
            .ok()
            .filter(|n| n.is_finite() && *n >= 0.0 && !raw.starts_with(['+', '-']))
            .ok_or_else(|| "Use non-negative numbers for prices.".to_owned())?;
        price.insert((*key).into(), json!(value));
    }
    patch.insert("usagePriceOverrides".into(), json!({model:price}));
    Ok(patch)
}
