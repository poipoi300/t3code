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
#[derive(Clone, Default, Debug, PartialEq)]
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
        (config.environment.capabilities.usage_model_aliases == Some(true))
            .then(|| config.settings.usage_model_aliases.get(&id))
            .flatten()
            .map(|s| s.0.clone())
            .unwrap_or_default(),
    ]
}
#[derive(Clone, Debug, PartialEq)]
pub enum PriceCell {
    Value(String),
    Mixed,
    Unavailable,
}

pub fn shared_cells(configs: &[Option<&ServerConfig>], model: &str) -> [PriceCell; 5] {
    let rows: Vec<_> = configs
        .iter()
        .map(|config| config.map(|c| cells(c, model)))
        .collect();
    std::array::from_fn(|i| {
        if rows.iter().any(Option::is_none)
            || (i == 4
                && configs
                    .iter()
                    .flatten()
                    .any(|c| c.environment.capabilities.usage_model_aliases != Some(true)))
        {
            return PriceCell::Unavailable;
        }
        let mut values = rows.iter().flatten().map(|row| &row[i]);
        let first = values.next().cloned().unwrap_or_default();
        if values.any(|value| *value != first) {
            PriceCell::Mixed
        } else {
            PriceCell::Value(first)
        }
    })
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
        if edit.alias.is_none() && edit.rates.iter().any(Option::is_some) {
            return Err(format!(
                "{} maps this model. Remove it from \"Apply to\" or clear its mapping first.",
                config.environment.label.0
            ));
        }
        if config.environment.capabilities.usage_model_aliases != Some(true) {
            return Err("Update server to map models".into());
        }
        if alias == model {
            return Err("Map to a different model.".into());
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
    if FIELDS.iter().enumerate().any(|(i, field)| {
        current[i].parse::<f64>().ok() != price.get(*field).and_then(Value::as_f64)
    }) {
        patch.insert("usagePriceOverrides".into(), json!({model:price}));
    }
    Ok(patch)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config(label: &str, aliases: bool, rate: u32) -> ServerConfig {
        let sample = include_str!("../../../../../fixtures/samples/ServerConfig.jsonl")
            .lines()
            .next()
            .expect("sample");
        let mut c: Value = serde_json::from_str(sample).expect("sample json");
        c["environment"]["label"] = json!(label);
        c["environment"]["capabilities"]["usageModelAliases"] = json!(aliases);
        c["settings"]["usagePriceOverrides"] =
            json!({"one":{"inputCostPerMillionTokens":rate,"outputCostPerMillionTokens":2}});
        c["settings"]["usageModelAliases"] = json!({"two":"one"});
        serde_json::from_value(c).expect("test config")
    }
    #[test]
    fn mapping_validation_is_per_destination_and_capability() {
        let online = config("Laptop", true, 1);
        let legacy = config("Legacy", false, 2);
        let mapping = PriceEdit {
            model: "one".into(),
            alias: Some("two".into()),
            ..Default::default()
        };
        assert_eq!(
            patch(&online, &mapping),
            Err("Model mappings cannot form a cycle on Laptop.".into())
        );
        assert_eq!(
            patch(&legacy, &mapping),
            Err("Update server to map models".into())
        );
        let same = PriceEdit {
            alias: Some("one".into()),
            ..mapping
        };
        assert_eq!(
            patch(&online, &same),
            Err("Map to a different model.".into())
        );
        assert_eq!(cells(&legacy, "two")[4], "");
        let remove = PriceEdit {
            model: "two".into(),
            removed: true,
            ..Default::default()
        };
        assert!(
            patch(&legacy, &remove)
                .expect("nothing to remove")
                .is_empty()
        );
    }
    #[test]
    fn shared_cells_distinguish_mixed_rates_and_unavailable_aliases() {
        let first = config("Home", true, 1);
        let second = config("Work", false, 3);
        let cells = shared_cells(&[Some(&first), Some(&second)], "one");
        assert_eq!(cells[0], PriceCell::Mixed);
        assert_eq!(cells[1], PriceCell::Value("2".into()));
        assert_eq!(cells[4], PriceCell::Unavailable);
        assert_eq!(
            shared_cells(&[Some(&first), None], "one")[0],
            PriceCell::Unavailable
        );
    }
    #[test]
    fn mixed_untouched_fields_keep_each_destinations_rate() {
        let edit = PriceEdit {
            model: "one".into(),
            rates: [None, Some("4".into()), None, None],
            ..Default::default()
        };
        for rate in [1, 3] {
            let c = config("Work", true, rate);
            let patch = patch(&c, &edit).expect("valid price");
            assert_eq!(
                patch["usagePriceOverrides"]["one"]["inputCostPerMillionTokens"].as_f64(),
                Some(f64::from(rate))
            );
            assert_eq!(
                patch["usagePriceOverrides"]["one"]["outputCostPerMillionTokens"].as_f64(),
                Some(4.)
            );
            assert!(
                super::patch(
                    &c,
                    &PriceEdit {
                        model: "one".into(),
                        ..Default::default()
                    }
                )
                .expect("unchanged")
                .is_empty()
            );
        }
    }
}
