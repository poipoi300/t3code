use crate::{UsageRecord, UsageSpeed, UsageTokenTotals};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenRates {
    pub input_cost_per_token: f64,
    pub output_cost_per_token: f64,
    pub cache_read_cost_per_token: f64,
    pub cache_creation_cost_per_token: f64,
    pub cache_creation1h_cost_per_token: f64,
}
#[derive(Debug, Clone, PartialEq)]
pub struct SpeedRates {
    pub standard: TokenRates,
    pub fast: Option<TokenRates>,
    pub ultrafast: Option<TokenRates>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ContextTier {
    pub threshold_tokens: u64,
    pub rate: SpeedRates,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ModelRate {
    pub speeds: SpeedRates,
    pub context_tiers: Vec<ContextTier>,
}
pub type RateTable = IndexMap<String, ModelRate>;

fn num(v: &Value, key: &str) -> Option<f64> {
    v[key].as_f64().filter(|n| n.is_finite())
}
fn read_tokens(
    v: &Value,
    suffix: &str,
    base: Option<&TokenRates>,
    output_fallback: Option<f64>,
) -> Option<TokenRates> {
    let input = num(v, &format!("input_cost_per_token{suffix}"))?;
    let output = num(v, &format!("output_cost_per_token{suffix}")).or(output_fallback)?;
    let cache = |name: &str, previous: fn(&TokenRates) -> f64, fallback: f64| {
        num(v, &format!("{name}{suffix}")).unwrap_or_else(|| {
            base.filter(|b| b.input_cost_per_token > 0.0)
                .map(|b| previous(b) / b.input_cost_per_token * input)
                .unwrap_or(fallback)
        })
    };
    let creation = cache(
        "cache_creation_input_token_cost",
        |b| b.cache_creation_cost_per_token,
        input,
    );
    Some(TokenRates {
        input_cost_per_token: input,
        output_cost_per_token: output,
        cache_read_cost_per_token: cache(
            "cache_read_input_token_cost",
            |b| b.cache_read_cost_per_token,
            input,
        ),
        cache_creation_cost_per_token: creation,
        cache_creation1h_cost_per_token: cache(
            "cache_creation_input_token_cost_above_1hr",
            |b| b.cache_creation1h_cost_per_token,
            creation,
        ),
    })
}
fn scale(r: &TokenRates, n: f64) -> TokenRates {
    TokenRates {
        input_cost_per_token: r.input_cost_per_token * n,
        output_cost_per_token: r.output_cost_per_token * n,
        cache_read_cost_per_token: r.cache_read_cost_per_token * n,
        cache_creation_cost_per_token: r.cache_creation_cost_per_token * n,
        cache_creation1h_cost_per_token: r.cache_creation1h_cost_per_token * n,
    }
}
fn read_speeds(v: &Value, standard: TokenRates, suffix: &str) -> SpeedRates {
    let fast = v["provider_specific_entry"]["fast"]
        .as_f64()
        .filter(|n| n.is_finite() && *n > 0.0)
        .map(|n| scale(&standard, n))
        .or_else(|| read_tokens(v, &format!("{suffix}_priority"), Some(&standard), None));
    let ultrafast = read_tokens(v, &format!("{suffix}_ultrafast"), Some(&standard), None);
    SpeedRates {
        standard,
        fast,
        ultrafast,
    }
}
fn merge_speeds(a: &SpeedRates, b: &SpeedRates) -> Option<SpeedRates> {
    if a.standard != b.standard
        || (a.fast.is_some() && b.fast.is_some() && a.fast != b.fast)
        || (a.ultrafast.is_some() && b.ultrafast.is_some() && a.ultrafast != b.ultrafast)
    {
        return None;
    }
    Some(SpeedRates {
        standard: a.standard.clone(),
        fast: a.fast.clone().or(b.fast.clone()),
        ultrafast: a.ultrafast.clone().or(b.ultrafast.clone()),
    })
}
fn merge_rates(a: &ModelRate, b: &ModelRate) -> Option<ModelRate> {
    let speeds = merge_speeds(&a.speeds, &b.speeds)?;
    let context_tiers = if a.context_tiers.is_empty() {
        b.context_tiers.clone()
    } else if b.context_tiers.is_empty() {
        a.context_tiers.clone()
    } else {
        if a.context_tiers.len() != b.context_tiers.len() {
            return None;
        }
        a.context_tiers
            .iter()
            .zip(&b.context_tiers)
            .map(|(a, b)| {
                if a.threshold_tokens != b.threshold_tokens {
                    return None;
                }
                Some(ContextTier {
                    threshold_tokens: a.threshold_tokens,
                    rate: merge_speeds(&a.rate, &b.rate)?,
                })
            })
            .collect::<Option<Vec<_>>>()?
    };
    Some(ModelRate {
        speeds,
        context_tiers,
    })
}

pub fn parse_rate_table(document: &Value) -> RateTable {
    let mut table = RateTable::new();
    let Some(models) = document.as_object() else {
        return table;
    };
    for (name, entry) in models {
        let Some(base) = read_tokens(entry, "", None, None) else {
            continue;
        };
        let key = name.trim().to_lowercase();
        if key.is_empty() {
            continue;
        }
        let mut thresholds = std::collections::BTreeSet::new();
        for k in entry.as_object().into_iter().flat_map(|o| o.keys()) {
            let Some(rest) = k.strip_prefix("input_cost_per_token_above_") else {
                continue;
            };
            let Some((count, tail)) = rest.split_once("k_tokens") else {
                continue;
            };
            if count.len() > 7
                || !count.bytes().all(|b| b.is_ascii_digit())
                || !["", "_priority", "_ultrafast"].contains(&tail)
            {
                continue;
            }
            if let Ok(n) = count.parse::<u64>() {
                thresholds.insert(n);
            }
        }
        let tiers = thresholds
            .into_iter()
            .rev()
            .map(|n| {
                let suffix = format!("_above_{n}k_tokens");
                let standard = read_tokens(
                    entry,
                    &suffix,
                    Some(&base),
                    Some(base.output_cost_per_token),
                )
                .unwrap_or(base.clone());
                ContextTier {
                    threshold_tokens: n * 1000,
                    rate: read_speeds(entry, standard, &suffix),
                }
            })
            .collect();
        table.insert(
            key,
            ModelRate {
                speeds: read_speeds(entry, base, ""),
                context_tiers: tiers,
            },
        );
    }
    let mut candidates: IndexMap<String, (Option<ModelRate>, bool)> = IndexMap::new();
    for (key, rate) in &table {
        let alias = key
            .rsplit_once('/')
            .map(|(_, b)| b)
            .unwrap_or_else(|| key.strip_prefix("anthropic.").unwrap_or(key));
        if alias.is_empty() || alias == key || table.contains_key(alias) {
            continue;
        }
        let first = key.split_once('/').is_some_and(|(p, _)| {
            [
                "anthropic",
                "deepseek",
                "gemini",
                "meta",
                "mistral",
                "openai",
                "xai",
            ]
            .contains(&p)
        });
        match candidates.get_mut(alias) {
            None => {
                candidates.insert(alias.into(), (Some(rate.clone()), first));
            }
            Some((held, held_first)) => {
                if first && !*held_first {
                    *held = Some(rate.clone());
                    *held_first = true;
                } else if first == *held_first {
                    *held = held.as_ref().and_then(|a| merge_rates(a, rate));
                }
            }
        }
    }
    for (alias, (rate, _)) in candidates {
        if let Some(rate) = rate {
            table.insert(alias, rate);
        }
    }
    table
}

pub fn lookup_rate<'a>(table: &'a RateTable, model: &str) -> Option<&'a ModelRate> {
    let normalized = model.trim().to_lowercase();
    let key = normalized.split('[').next().unwrap_or("");
    let bare = key.rsplit('/').next().unwrap_or("");
    if [
        "",
        "<synthetic>",
        "synthetic",
        "opus",
        "sonnet",
        "haiku",
        "fable",
    ]
    .contains(&bare)
    {
        return None;
    }
    table.get(key)
}
fn record_rate<'a>(
    table: &'a RateTable,
    record: &UsageRecord,
    overrides: &'a RateTable,
) -> Option<&'a ModelRate> {
    overrides
        .get(record.model.trim())
        .or_else(|| {
            record
                .rate_model
                .as_ref()
                .and_then(|m| lookup_rate(table, m))
        })
        .or_else(|| lookup_rate(table, &record.model))
}
fn rates_at<'a>(rate: &'a ModelRate, speed: UsageSpeed, t: &UsageTokenTotals) -> &'a TokenRates {
    let prompt = t
        .uncached_input_tokens
        .saturating_add(t.cached_input_tokens)
        .saturating_add(t.cache_creation_tokens);
    let tier = rate
        .context_tiers
        .iter()
        .find(|tier| prompt > tier.threshold_tokens)
        .map(|t| &t.rate);
    let speed_rate = |r: &'a SpeedRates| match speed {
        UsageSpeed::Standard => Some(&r.standard),
        UsageSpeed::Fast => r.fast.as_ref(),
        UsageSpeed::Ultrafast => r.ultrafast.as_ref(),
    };
    if speed == UsageSpeed::Standard {
        return &tier.unwrap_or(&rate.speeds).standard;
    }
    tier.and_then(speed_rate)
        .or_else(|| speed_rate(&rate.speeds))
        .unwrap_or(&tier.unwrap_or(&rate.speeds).standard)
}
pub fn create_override_rate_table(document: &Value) -> RateTable {
    document
        .as_object()
        .into_iter()
        .flat_map(|m| m.iter())
        .filter_map(|(name, v)| {
            let input = num(v, "inputCostPerMillionTokens")? / 1e6;
            let output = num(v, "outputCostPerMillionTokens")? / 1e6;
            let write = num(v, "cacheWriteCostPerMillionTokens")
                .map(|n| n / 1e6)
                .unwrap_or(input);
            Some((
                name.trim().into(),
                ModelRate {
                    speeds: SpeedRates {
                        standard: TokenRates {
                            input_cost_per_token: input,
                            output_cost_per_token: output,
                            cache_read_cost_per_token: num(v, "cacheReadCostPerMillionTokens")
                                .map(|n| n / 1e6)
                                .unwrap_or(input),
                            cache_creation_cost_per_token: write,
                            cache_creation1h_cost_per_token: write,
                        },
                        fast: None,
                        ultrafast: None,
                    },
                    context_tiers: vec![],
                },
            ))
        })
        .collect()
}

#[derive(Debug, Clone)]
pub struct PricedUsage {
    pub cost_usd: f64,
    pub cost_source: &'static str,
    pub category_cost_usd: Option<[f64; 4]>,
    pub speed_premium_usd: f64,
}
fn categories(r: &UsageRecord, rate: &TokenRates) -> [f64; 4] {
    let t = &r.totals;
    let one_hour = r
        .cache_creation1h_tokens
        .unwrap_or(0)
        .min(t.cache_creation_tokens);
    [
        t.uncached_input_tokens as f64 * rate.input_cost_per_token,
        t.cached_input_tokens as f64 * rate.cache_read_cost_per_token,
        (t.cache_creation_tokens - one_hour) as f64 * rate.cache_creation_cost_per_token
            + one_hour as f64 * rate.cache_creation1h_cost_per_token,
        t.output_tokens as f64 * rate.output_cost_per_token,
    ]
}
pub fn price_usage(table: &RateTable, r: &UsageRecord, overrides: &RateTable) -> PricedUsage {
    let reported = r
        .reported_cost_usd
        .filter(|n| n.is_finite() && !overrides.contains_key(r.model.trim()));
    let unsplit = |cost, source| PricedUsage {
        cost_usd: cost,
        cost_source: source,
        category_cost_usd: None,
        speed_premium_usd: 0.0,
    };
    let Some(rate) = record_rate(table, r, overrides) else {
        return reported
            .map(|n| unsplit(n, "providerReported"))
            .unwrap_or(unsplit(0.0, "unpriced"));
    };
    let list = categories(r, rates_at(rate, r.speed, &r.totals));
    let sum: f64 = list.iter().sum();
    if let Some(reported) = reported
        && sum <= 0.0
    {
        return unsplit(reported, "providerReported");
    }
    let scale = reported.map(|n| n / sum).unwrap_or(1.0);
    let premium = if r.speed == UsageSpeed::Standard {
        0.0
    } else {
        sum - categories(r, rates_at(rate, UsageSpeed::Standard, &r.totals))
            .iter()
            .sum::<f64>()
    };
    PricedUsage {
        cost_usd: reported.unwrap_or(sum),
        cost_source: if reported.is_some() {
            "providerReported"
        } else {
            "modelPriced"
        },
        category_cost_usd: Some(list.map(|n| n * scale)),
        speed_premium_usd: premium * scale,
    }
}
pub fn cache_savings_usd(table: &RateTable, r: &UsageRecord, overrides: &RateTable) -> f64 {
    record_rate(table, r, overrides)
        .map(|rate| {
            let rate = rates_at(rate, r.speed, &r.totals);
            r.totals.cached_input_tokens as f64
                * (rate.input_cost_per_token - rate.cache_read_cost_per_token)
        })
        .unwrap_or(0.0)
}
