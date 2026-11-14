//! packages/client-runtime/src/modelSearch.ts; ties retain the catalog's explicit order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PickerSearchItem {
    pub driver_kind: String,
    pub provider_display_name: String,
    pub name: String,
    pub short_name: Option<String>,
    pub sub_provider: Option<String>,
    pub is_favorite: bool,
}
fn normalize(value: &str) -> String {
    value
        .trim_matches(crate::view_logic::is_js_whitespace)
        .to_lowercase()
}
fn words(value: &str) -> String {
    normalize(value)
        .split(crate::view_logic::is_js_whitespace)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}
fn score_token(field: &str, token: &str, base: i64) -> Option<i64> {
    if field.is_empty() || token.is_empty() {
        return None;
    }
    if field == token {
        return Some(base);
    }
    if field.starts_with(token) {
        return Some(base + 2);
    }
    if [" ", "-", "_", "/"]
        .iter()
        .any(|marker| field.contains(&format!("{marker}{token}")))
    {
        return Some(base + 4);
    }
    if field.contains(token) {
        return Some(base + 6);
    }
    let token = token.encode_utf16().collect::<Vec<_>>();
    if token.len() < 3 {
        return None;
    }
    let (mut matched, mut first, mut previous, mut gaps) = (0usize, None, None, 0usize);
    for (index, unit) in field.encode_utf16().enumerate() {
        if token[matched] != unit {
            continue;
        }
        first.get_or_insert(index);
        if let Some(previous) = previous {
            gaps += index - previous - 1;
        }
        previous = Some(index);
        matched += 1;
        if matched == token.len() {
            return Some(base + 100 + (gaps * 3 + index - first? + 1 - token.len()) as i64);
        }
    }
    None
}
pub fn score_model_picker_search(model: &PickerSearchItem, query: &str) -> Option<i64> {
    let query = words(query);
    if query.is_empty() {
        return Some(0);
    }
    if query == words(&model.name)
        || query == words(model.short_name.as_deref().unwrap_or_default())
    {
        return Some(-1000);
    }
    let mut fields = vec![normalize(&model.name)];
    if let Some(short) = model.short_name.as_ref().filter(|s| !s.is_empty()) {
        fields.push(normalize(short));
    }
    if let Some(sub) = model.sub_provider.as_ref().filter(|s| !s.is_empty()) {
        fields.push(normalize(sub));
    }
    fields.push(normalize(&model.driver_kind));
    fields.push(normalize(&model.provider_display_name));
    fields.push(normalize(
        &[
            Some(model.name.as_str()),
            model.short_name.as_deref(),
            model.sub_provider.as_deref(),
            Some(model.driver_kind.as_str()),
            Some(model.provider_display_name.as_str()),
        ]
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" "),
    ));
    let score = query.split(' ').try_fold(0i64, |score, token| {
        fields
            .iter()
            .enumerate()
            .filter_map(|(index, field)| score_token(field, token, index as i64 * 10))
            .min()
            .map(|matched| score + matched)
    })?;
    Some(score - if model.is_favorite { 24 } else { 0 })
}
pub fn rank_model_picker_search(items: &[PickerSearchItem], query: &str) -> Vec<usize> {
    let mut ranked = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            score_model_picker_search(item, query).map(|score| (score, !item.is_favorite, index))
        })
        .collect::<Vec<_>>();
    ranked.sort_unstable();
    ranked.into_iter().map(|(_, _, index)| index).collect()
}
