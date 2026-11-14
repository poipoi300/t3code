//! Literal, non-overlapping find matches with JavaScript UTF-16 offsets.
fn fold(text: &str) -> String {
    let characters = |value: String| value.replace('ς', "σ").replace(['‘', '’'], "'");
    let lowered = characters(text.to_lowercase());
    if lowered.encode_utf16().count() == text.encode_utf16().count() {
        return lowered;
    }
    text.chars()
        .map(|c| {
            let lower = characters(c.to_lowercase().collect());
            if lower.encode_utf16().count() == c.len_utf16() {
                lower
            } else {
                c.to_string()
            }
        })
        .collect()
}
pub fn find_thread_search_occurrences(text: &str, query: &str) -> Vec<usize> {
    if text.is_empty() || query.is_empty() {
        return vec![];
    }
    let text = fold(text);
    let query = fold(query);
    let mut previous = 0;
    let mut units = 0;
    text.match_indices(&query)
        .map(|(offset, _)| {
            units += text[previous..offset].encode_utf16().count();
            previous = offset;
            units
        })
        .collect()
}
pub fn count_thread_search_occurrences(text: &str, query: &str) -> u64 {
    if text.is_empty() || query.is_empty() {
        return 0;
    }
    fold(text).matches(&fold(query)).count() as u64
}
