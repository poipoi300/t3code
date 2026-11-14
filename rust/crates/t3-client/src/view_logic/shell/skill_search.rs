//! Provider skill search. Costs match JavaScript UTF-16 indices; locale tie-breaking is injected.
use crate::view_logic::JsTrim;
use std::{cmp::Ordering, collections::BTreeSet};
use t3_contracts::server::ServerProviderSkill;
fn label(s: &ServerProviderSkill) -> &str {
    s.display_name
        .as_ref()
        .and_then(Option::as_ref)
        .map(|s| s.0.js_trim())
        .filter(|s| !s.is_empty())
        .unwrap_or(&s.name.0)
}
fn subsequence(value: &[u16], query: &[u16]) -> Option<usize> {
    let mut q = 0;
    let mut first = None;
    let mut previous = None;
    let mut gaps = 0;
    for (i, c) in value.iter().enumerate() {
        if Some(c) != query.get(q) {
            continue;
        }
        first.get_or_insert(i);
        if let Some(before) = previous {
            gaps += i - before - 1;
        }
        previous = Some(i);
        q += 1;
        if q == query.len() {
            return Some(
                first.unwrap_or(0) * 2 + gaps * 3 + i - first.unwrap_or(0) + 1 - query.len()
                    + 64.min(value.len().saturating_sub(query.len())),
            );
        }
    }
    None
}
pub(crate) fn score(
    value: &str,
    query: &str,
    bases: (
        usize,
        Option<usize>,
        Option<usize>,
        Option<usize>,
        Option<usize>,
    ),
    markers: &[char],
) -> Option<usize> {
    if value.is_empty() || query.is_empty() {
        return None;
    }
    let value = value.encode_utf16().collect::<Vec<_>>();
    let query = query.encode_utf16().collect::<Vec<_>>();
    let penalty = 64.min(value.len().saturating_sub(query.len()));
    if value == query {
        return Some(bases.0);
    }
    if value.starts_with(&query)
        && let Some(base) = bases.1
    {
        return Some(base + penalty);
    }
    if let Some(base) = bases.2 {
        let index = value.windows(query.len() + 1).position(|s| {
            markers.iter().any(|m| u32::from(*m) == u32::from(s[0])) && s[1..] == query
        });
        if let Some(index) = index {
            return Some(base + (index + 1) * 2 + penalty);
        }
    }
    if let Some(base) = bases.3
        && let Some(index) = value.windows(query.len()).position(|s| s == query)
    {
        return Some(base + index * 2 + penalty);
    }
    bases
        .4
        .and_then(|base| subsequence(&value, &query).map(|cost| base + cost))
}
pub fn score_skill(skill: &ServerProviderSkill, normalized_query: &str) -> Option<usize> {
    let optional = |s: &Option<Option<t3_contracts::base_schemas::TrimmedNonEmptyString>>| {
        s.as_ref()
            .and_then(Option::as_ref)
            .map(|s| s.0.to_lowercase())
            .unwrap_or_default()
    };
    [
        score(
            &skill.name.0.to_lowercase(),
            normalized_query,
            (0, Some(2), Some(4), Some(6), Some(100)),
            &['-', '_', '/'],
        ),
        score(
            &label(skill).to_lowercase(),
            normalized_query,
            (1, Some(3), Some(5), Some(7), Some(110)),
            &[' ', '-', '_', '/'],
        ),
        score(
            &optional(&skill.short_description),
            normalized_query,
            (20, Some(22), Some(24), Some(26), None),
            &[' ', '-', '_', '/'],
        ),
        score(
            &optional(&skill.description),
            normalized_query,
            (30, Some(32), Some(34), Some(36), None),
            &[' ', '-', '_', '/'],
        ),
        score(
            &optional(&skill.scope),
            normalized_query,
            (40, Some(42), None, Some(44), None),
            &[],
        ),
    ]
    .into_iter()
    .flatten()
    .min()
}
pub fn search_skills<'a>(
    skills: &'a [ServerProviderSkill],
    query: &str,
    limit: Option<usize>,
    compare: impl Fn(&str, &str) -> Ordering,
) -> Vec<&'a ServerProviderSkill> {
    let mut seen = BTreeSet::new();
    let enabled = skills
        .iter()
        .filter(|s| {
            s.enabled
                && s.user_invocable != Some(Some(false))
                && seen.insert(s.name.0.js_trim().to_lowercase())
        })
        .collect::<Vec<_>>();
    let query = query
        .js_trim()
        .trim_start_matches(|c| {
            unicode_general_category::get_general_category(c)
                == unicode_general_category::GeneralCategory::CurrencySymbol
        })
        .to_lowercase();
    if query.is_empty() {
        return enabled;
    }
    let mut rows = enabled
        .into_iter()
        .enumerate()
        .filter_map(|(i, s)| {
            score_skill(s, &query).map(|score| {
                (
                    s,
                    score,
                    format!("{}\0{}", label(s).to_lowercase(), s.name.0),
                    i,
                )
            })
        })
        .collect::<Vec<_>>();
    rows.sort_by(|(_, a, at, ai), (_, b, bt, bi)| {
        a.cmp(b).then_with(|| compare(at, bt)).then(ai.cmp(bi))
    });
    rows.into_iter()
        .take(limit.unwrap_or(usize::MAX))
        .map(|(s, _, _, _)| s)
        .collect()
}
