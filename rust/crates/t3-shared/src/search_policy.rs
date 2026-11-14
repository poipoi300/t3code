//! Message narrowing and ranking policy from ThreadSearch.ts and shared/paletteSearch.ts.
use crate::Result;
use t3_contracts::{base_schemas::*, thread_search::*};
#[derive(Clone)]
pub struct MessageHit {
    pub thread_id: ThreadId,
    pub project_id: ProjectId,
    pub message_id: MessageId,
    pub source: OrchestrationThreadSearchSource,
    pub text: String,
    pub message_created_at: Option<IsoDateTime>,
    pub quality: f64,
    pub offset: usize,
    pub hits: u64,
    pub opens: Option<serde_json::Value>,
    pub activity_at: Option<IsoDateTime>,
}
#[derive(Clone)]
pub struct TitleCandidate {
    pub thread: serde_json::Value,
    pub project_title: String,
    pub latest_user_message_at: Option<IsoDateTime>,
}
#[derive(Debug, serde::Serialize)]
pub struct MessageSearchRequest {
    pub match_query: Option<String>,
    pub substrings: Vec<String>,
    pub short_spellings: Vec<Vec<String>>,
}
pub use crate::palette_search as palette;
use palette::{SearchPullRequest, ThreadSubject};
use regex::Regex;
use serde_json::Value;
use unicode_general_category::GeneralCategory;
use unicode_normalization::UnicodeNormalization;

#[derive(Default)]
pub struct NativeThreadSearchPolicy {
    matchers:
        std::sync::Mutex<std::collections::VecDeque<(String, std::sync::Arc<PreparedMatcher>)>>,
}
struct PreparedMatcher {
    long: Vec<String>,
    tokens: Vec<String>,
    units: Vec<Vec<u16>>,
    normalized: Vec<Vec<u16>>,
    phrase: Option<Regex>,
    folded_phrase: Option<Regex>,
    non_ascii: bool,
}
impl PreparedMatcher {
    fn new(query: &str) -> Self {
        let (long, short, words) = message_tokens(query);
        let tokens: Vec<_> = long.iter().chain(&short).cloned().collect();
        let normalized = tokens
            .iter()
            .map(|token| {
                palette::fold_in_place(token)
                    .chars()
                    .filter(|c| !marks(*c))
                    .collect::<String>()
                    .encode_utf16()
                    .collect()
            })
            .collect();
        let folded_words: Vec<_> = words
            .iter()
            .map(|word| {
                palette::fold_in_place(word)
                    .chars()
                    .filter(|c| !marks(*c))
                    .collect()
            })
            .collect();
        Self {
            long,
            units: tokens
                .iter()
                .map(|token| token.encode_utf16().collect())
                .collect(),
            tokens,
            normalized,
            phrase: phrase_regex(&words),
            folded_phrase: phrase_regex(&folded_words),
            non_ascii: !query.is_ascii(),
        }
    }
}
impl NativeThreadSearchPolicy {
    fn matcher(&self, query: &str) -> std::sync::Arc<PreparedMatcher> {
        let mut cache = self
            .matchers
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(index) = cache.iter().position(|(key, _)| key == query)
            && let Some(entry) = cache.remove(index)
        {
            let matcher = entry.1.clone();
            cache.push_back(entry);
            return matcher;
        }
        let matcher = std::sync::Arc::new(PreparedMatcher::new(query));
        if cache.len() == 8 {
            cache.pop_front();
        }
        cache.push_back((query.into(), matcher.clone()));
        matcher
    }
}
fn js_space(c: char) -> bool {
    palette::is_js_whitespace(c)
}
fn quotes(text: &str) -> &str {
    text.trim_matches(js_space)
        .trim_matches(['\'', '"', '`', '“', '”', '‘', '’'])
}
fn marks(c: char) -> bool {
    matches!(
        palette::category(c),
        GeneralCategory::NonspacingMark
            | GeneralCategory::SpacingMark
            | GeneralCategory::EnclosingMark
    )
}
pub fn message_tokens(query: &str) -> (Vec<String>, Vec<String>, Vec<String>) {
    let words: Vec<_> = quotes(query)
        .replace(['‘', '’'], "'")
        .nfc()
        .collect::<String>()
        .to_lowercase()
        .split(js_space)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    let searchable = |word: &&String| {
        word.chars().count() >= 3 || (word.chars().count() == 2 && word.chars().any(palette::cjk))
    };
    (
        words.iter().filter(searchable).cloned().collect(),
        words
            .iter()
            .filter(|word| !searchable(word))
            .cloned()
            .collect(),
        words,
    )
}
fn spellings(word: &str) -> Vec<String> {
    let mut result = vec![String::new()];
    for c in word.chars() {
        let original = c.to_string();
        let plain = original.nfd().filter(|c| !marks(*c)).collect::<String>();
        let variants = [
            original.clone(),
            original.to_uppercase(),
            plain.clone(),
            plain.to_uppercase(),
        ];
        result = result
            .into_iter()
            .flat_map(|prefix| {
                variants
                    .iter()
                    .map(move |variant| format!("{prefix}{variant}"))
            })
            .collect();
    }
    let mut seen = std::collections::BTreeSet::new();
    result.retain(|value| seen.insert(value.clone()));
    result
}
fn unit_char(unit: Option<&u16>) -> Option<char> {
    unit.and_then(|unit| char::from_u32(u32::from(*unit)))
}
fn starts(text: &[u16], index: usize) -> bool {
    let Some(c) = unit_char(text.get(index)) else {
        // JavaScript's per-unit character class treats a surrogate as punctuation.
        return text.get(index).is_some();
    };
    if !palette::alnum_unit(text[index]) && !palette::symbol(c) {
        return true;
    }
    palette::word_start(
        index.checked_sub(2).and_then(|i| unit_char(text.get(i))),
        index.checked_sub(1).and_then(|i| unit_char(text.get(i))),
        c,
        unit_char(text.get(index + 1)),
    )
}
fn ends(text: &[u16], index: usize) -> bool {
    index >= text.len() || !palette::alnum_unit(text[index]) || starts(text, index)
}
fn positions(
    text: &[u16],
    folded: &[u16],
    token: &[u16],
    first_only: bool,
) -> Option<(f64, Vec<usize>)> {
    if token.is_empty() || token.len() > folded.len() {
        return None;
    }
    let mut quality = 0.0;
    let mut places = vec![];
    let mut run_start = 0;
    let mut run_end = 0;
    let mut hex = false;
    for index in 0..=folded.len() - token.len() {
        if folded[index..index + token.len()] != *token {
            continue;
        }
        if index + token.len() > run_end {
            run_start = index;
            while run_start > 0 && palette::alnum_unit(text[run_start - 1]) {
                run_start -= 1;
            }
            run_end = index + token.len();
            while run_end < text.len() && palette::alnum_unit(text[run_end]) {
                run_end += 1;
            }
            let run = &text[run_start..run_end];
            hex = run.len() >= 7
                && run.iter().any(|ch| (48..=57).contains(ch))
                && run.iter().all(|ch| matches!(*ch,48..=57|65..=70|97..=102));
        }
        if hex && run_start < index {
            continue;
        }
        let found = if !starts(text, index) {
            0.3
        } else if ends(text, index + token.len()) {
            1.0
        } else {
            0.8
        };
        if found > quality {
            quality = found;
            places = vec![index];
            if first_only && quality == 1.0 {
                return Some((quality, places));
            }
        } else if !first_only && found == quality {
            places.push(index);
        }
    }
    (!places.is_empty()).then_some((quality, places))
}
fn phrase_regex(words: &[String]) -> Option<Regex> {
    if words.len() < 2 {
        return None;
    }
    let pattern = format!(
        r"(?i)(?:^|[^\p{{L}}\p{{N}}])({})(?:$|[^\p{{L}}\p{{N}}])",
        words
            .iter()
            .map(|word| regex::escape(word))
            .collect::<Vec<_>>()
            .join(r"\s+")
    );
    Regex::new(&pattern).ok()
}
fn phrase(text: &str, regex: Option<&Regex>) -> Option<usize> {
    let captures = regex?.captures(text)?;
    let hit = captures.get(1)?;
    Some(text[..hit.start()].encode_utf16().count())
}
pub fn match_message(text: &str, query: &str) -> Option<(f64, usize)> {
    PreparedMatcher::new(query).matches(text)
}
impl PreparedMatcher {
    fn matches(&self, text: &str) -> Option<(f64, usize)> {
        let tokens = &self.tokens;
        let original: Vec<_> = text.encode_utf16().collect();
        let lower = text.to_lowercase();
        let plain = if lower.encode_utf16().count() == original.len() {
            lower
        } else {
            palette::fold_in_place(text)
        };
        let plain_units: Vec<_> = plain.encode_utf16().collect();
        let mut folded = None;
        let mut quality: f64 = 1.0;
        let mut all = vec![];
        for (index, token) in tokens.iter().enumerate() {
            let token_units = &self.units[index];
            let mut found = positions(&original, &plain_units, token_units, tokens.len() == 1);
            if found.as_ref().is_none_or(|(quality, _)| *quality != 1.0)
                && (!token.is_ascii() || !text.is_ascii())
            {
                let normalized = &self.normalized[index];
                if !normalized.is_empty() {
                    let field = folded.get_or_insert_with(|| palette::fold_in_place(text));
                    found = positions(
                        &original,
                        &field.encode_utf16().collect::<Vec<_>>(),
                        normalized,
                        tokens.len() == 1,
                    );
                }
            }
            let (found, places) = found?;
            if found == 0.3 && token.chars().count() < 4 {
                return None;
            }
            if self.long.contains(token) {
                quality = quality.min(found);
            }
            all.push(places);
        }
        if let Some(at) = phrase(&plain.replace(['‘', '’'], "'"), self.phrase.as_ref()) {
            return Some((quality + 0.25, at));
        }
        if (self.non_ascii || !text.is_ascii())
            && let Some(at) = phrase(
                &folded
                    .unwrap_or_else(|| palette::fold_in_place(text))
                    .replace(['‘', '’'], "'"),
                self.folded_phrase.as_ref(),
            )
        {
            return Some((quality + 0.25, at));
        }
        let mut hits: Vec<_> = all
            .iter()
            .enumerate()
            .flat_map(|(word, places)| places.iter().map(move |at| (*at, word)))
            .collect();
        hits.sort_by_key(|hit| hit.0);
        let mut counts = vec![0; all.len()];
        let mut covered = 0;
        let mut left = 0;
        let mut best = (hits.first().map_or(0, |hit| hit.0), usize::MAX);
        for (index, word) in &hits {
            counts[*word] += 1;
            if counts[*word] == 1 {
                covered += 1;
            }
            while covered == all.len() && !all.is_empty() {
                let (first, first_word) = hits[left];
                if index - first < best.1 {
                    best = (first, index - first);
                }
                counts[first_word] -= 1;
                if counts[first_word] == 0 {
                    covered -= 1;
                }
                left += 1;
            }
        }
        Some((
            quality
                + if self.long.len() > 1 && best.1 <= 60 {
                    0.1
                } else {
                    0.0
                },
            best.0,
        ))
    }
}
fn activity(value: Option<&str>) -> Option<f64> {
    value.and_then(epoch_millis).map(|ms| ms as f64)
}
fn opens(
    value: Option<&Value>,
) -> Option<t3_contracts::orchestration_v2::OrchestrationV2ThreadOpens> {
    value
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
}
fn subject(row: &TitleCandidate, environment: Option<&str>) -> ThreadSubject {
    let thread = &row.thread;
    let prs = thread_search_pull_requests(thread);
    ThreadSubject {
        id: thread["id"].as_str().map(str::to_owned),
        title: thread["title"].as_str().unwrap_or("").into(),
        branch: thread["branch"].as_str().map(str::to_owned),
        project_title: Some(row.project_title.clone()),
        environment_label: environment.map(str::to_owned),
        pull_requests: prs,
        activity_at: activity(
            row.latest_user_message_at
                .as_ref()
                .map(|date| date.0.as_str())
                .or(thread["createdAt"].as_str()),
        ),
        opens: opens(thread.get("opens")),
    }
}
impl NativeThreadSearchPolicy {
    pub fn message_search(&self, query: &str, index: bool) -> Option<MessageSearchRequest> {
        let (long, short, _) = message_tokens(query);
        if long.is_empty() {
            return None;
        }
        let indexed = index && long.iter().all(|word| word.chars().count() >= 3);
        let narrowing: Vec<_> = short
            .iter()
            .filter(|word| word.is_ascii())
            .cloned()
            .collect();
        Some(MessageSearchRequest {
            match_query: indexed.then(|| {
                long.iter()
                    .map(|word| format!("\"{}\"", word.replace('"', "\"\"")))
                    .collect::<Vec<_>>()
                    .join(" ")
            }),
            substrings: if indexed {
                narrowing
            } else {
                long.into_iter().chain(narrowing).collect()
            },
            short_spellings: short
                .iter()
                .filter(|word| !word.is_ascii())
                .map(|word| spellings(word))
                .collect(),
        })
    }
    pub fn shown_message(&self, text: &str) -> String {
        static LINKS: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
            Regex::new(r"!?\[([^\]\n]*)\]\([^)\s]*\)").expect("constant displayed-link regex")
        });
        LINKS.replace_all(text, "$1").into_owned()
    }
    pub fn match_message(&self, text: &str, query: &str) -> Option<(f64, usize)> {
        self.matcher(query).matches(text)
    }
    pub fn rank_messages(
        &self,
        hits: &[MessageHit],
        query: &str,
        now: i64,
        picks: bool,
    ) -> Result<Vec<usize>> {
        let candidates = hits
            .iter()
            .map(|hit| palette::MessageMatchCandidate {
                subject: Some(ThreadSubject {
                    activity_at: activity(hit.activity_at.as_ref().map(|date| date.0.as_str())),
                    opens: opens(hit.opens.as_ref()),
                    ..Default::default()
                }),
                quality: Some(hit.quality),
                hits: Some(hit.hits),
                message_created_at: hit.message_created_at.as_ref().map(|date| date.0.clone()),
            })
            .collect::<Vec<_>>();
        Ok(palette::rank_message_matches(
            &candidates,
            query,
            now as f64,
            picks,
            false,
        ))
    }

    pub fn rank_titles(
        &self,
        rows: &[TitleCandidate],
        query: &str,
        environment: Option<&str>,
        now: i64,
        picks: bool,
    ) -> Result<Vec<usize>> {
        let query = palette::fold_query(query);
        let subjects: Vec<_> = rows
            .iter()
            .map(|row| subject(row, environment))
            .enumerate()
            .collect();
        Ok(palette::rank_items(&subjects, |(_, subject), typos| {
            palette::score_thread(&query, subject, now as f64, typos, picks)
        })
        .into_iter()
        .map(|((index, _), _)| *index)
        .collect())
    }
}

pub fn epoch_millis(value: &str) -> Option<i64> {
    fn number(value: &str, range: std::ops::Range<usize>) -> Option<i64> {
        let digits = value.get(range)?;
        digits
            .bytes()
            .all(|byte| byte.is_ascii_digit())
            .then(|| digits.parse().ok())
            .flatten()
    }
    let bytes = value.as_bytes();
    let expanded = matches!(bytes.first(), Some(b'+' | b'-'));
    let shift = if expanded { 3 } else { 0 };
    let year = if expanded {
        let magnitude = number(value, 1..7)?;
        if bytes[0] == b'-' {
            if magnitude == 0 {
                return None;
            }
            -magnitude
        } else {
            magnitude
        }
    } else {
        number(value, 0..4)?
    };
    let month = number(value, 5 + shift..7 + shift)?;
    let day = number(value, 8 + shift..10 + shift)?;
    if bytes.get(4 + shift) != Some(&b'-')
        || bytes.get(7 + shift) != Some(&b'-')
        || !(1..=12).contains(&month)
    {
        return None;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if !(1..=days).contains(&day) {
        return None;
    }
    let adjusted_year = year - i64::from(month <= 2);
    let era = adjusted_year.div_euclid(400);
    let yoe = adjusted_year - era * 400;
    let shifted_month = month + if month > 2 { -3 } else { 9 };
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + (153 * shifted_month + 2) / 5 + day - 1;
    let date = (era * 146097 + doe - 719468) * 86_400_000;
    if value.len() == 10 + shift {
        return (date.abs() <= 8_640_000_000_000_000).then_some(date);
    }
    if !matches!(bytes.get(10 + shift), Some(b'T' | b't' | b' '))
        || bytes.get(13 + shift) != Some(&b':')
        || bytes.get(16 + shift) != Some(&b':')
    {
        return None;
    }
    let hour = number(value, 11 + shift..13 + shift)?;
    let minute = number(value, 14 + shift..16 + shift)?;
    let second = number(value, 17 + shift..19 + shift)?;
    if !(0..=23).contains(&hour) || !(0..=59).contains(&minute) || !(0..=59).contains(&second) {
        return None;
    }
    let mut end = 19 + shift;
    let mut millis = 0;
    if bytes.get(end) == Some(&b'.') {
        end += 1;
        let start = end;
        while bytes.get(end).is_some_and(u8::is_ascii_digit) {
            if end - start < 3 {
                millis = millis * 10 + i64::from(bytes[end] - b'0');
            }
            end += 1;
        }
        if end == start {
            return None;
        }
        for _ in end - start..3 {
            millis *= 10;
        }
    }
    let offset = match bytes.get(end) {
        Some(b'Z' | b'z') if end + 1 == bytes.len() => 0,
        Some(b'+' | b'-') => {
            let offset_hour = number(value, end + 1..end + 3)?;
            let colon = usize::from(bytes.get(end + 3) == Some(&b':'));
            let offset_minute = number(value, end + 3 + colon..end + 5 + colon)?;
            if end + 5 + colon != bytes.len() || offset_hour > 23 || offset_minute > 59 {
                return None;
            }
            (offset_hour * 60 + offset_minute) * 60_000 * if bytes[end] == b'+' { 1 } else { -1 }
        }
        _ => return None,
    };
    let millis = date + hour * 3_600_000 + minute * 60_000 + second * 1000 + millis - offset;
    (millis.abs() <= 8_640_000_000_000_000).then_some(millis)
}

pub fn thread_search_pull_requests(thread: &Value) -> Vec<SearchPullRequest> {
    let entry = |link: &Value, title: Option<String>| -> Option<SearchPullRequest> {
        let number = link["number"].as_u64()?;
        Some(SearchPullRequest {
            number,
            title,
            references: vec![
                format!(
                    "{}#{number}",
                    link["repository"].as_str().unwrap_or_default()
                ),
                link["url"].as_str().unwrap_or_default().into(),
            ],
        })
    };
    let mut prs: Vec<_> = entry(&thread["branchPullRequest"], None)
        .into_iter()
        .collect();
    if let Some(links) = thread["pullRequests"]
        .as_array()
        .filter(|links| !links.is_empty())
    {
        let mut visible: Vec<_> = links
            .iter()
            .filter(|link| link["source"] != "stack-dismissed")
            .collect();
        let updated = |link: &&Value| {
            activity(
                link["snapshot"]["updatedAt"]
                    .as_str()
                    .or(link["linkedAt"].as_str()),
            )
            .unwrap_or(0.0)
        };
        visible.sort_by(|left, right| updated(right).total_cmp(&updated(left)));
        prs.extend(visible.into_iter().enumerate().filter_map(|(index, link)| {
            entry(
                link,
                (index < 3)
                    .then(|| link["snapshot"]["title"].as_str().map(str::to_owned))
                    .flatten(),
            )
        }));
    } else {
        prs.extend(entry(&thread["linkedPullRequest"], None));
    }
    prs
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_message_ranking_matches_typescript_picks_quality_hits_and_recency() {
        let cases: Vec<Value> = serde_json::from_str(include_str!(
            "../tests/fixtures/transport_message_rank.json"
        ))
        .unwrap();
        for case in cases {
            let hits: Vec<_> = case["hits"]
                .as_array()
                .unwrap()
                .iter()
                .map(|hit| MessageHit {
                    thread_id: serde_json::from_value(hit["threadId"].clone()).unwrap(),
                    project_id: serde_json::from_value(hit["projectId"].clone()).unwrap(),
                    message_id: serde_json::from_value(hit["messageId"].clone()).unwrap(),
                    source: serde_json::from_value(hit["source"].clone()).unwrap(),
                    text: hit["text"].as_str().unwrap().into(),
                    message_created_at: serde_json::from_value(hit["messageCreatedAt"].clone())
                        .unwrap(),
                    quality: hit["quality"].as_f64().unwrap(),
                    offset: 0,
                    hits: hit["hits"].as_u64().unwrap(),
                    opens: (!hit["opens"].is_null()).then(|| hit["opens"].clone()),
                    activity_at: serde_json::from_value(hit["activityAt"].clone()).unwrap(),
                })
                .collect();
            let indices = NativeThreadSearchPolicy::default()
                .rank_messages(
                    &hits,
                    case["query"].as_str().unwrap(),
                    case["now"].as_i64().unwrap(),
                    case["picks"].as_bool().unwrap(),
                )
                .unwrap();
            assert_eq!(
                serde_json::to_value(indices).unwrap(),
                case["expected"],
                "query={},picks={}",
                case["query"],
                case["picks"]
            );
        }
    }
    #[test]
    fn native_message_matches_typescript_tokens_quality_and_offsets() {
        let cases: Vec<Value> = serde_json::from_str(include_str!(
            "../tests/fixtures/transport_message_match.json"
        ))
        .unwrap();
        for case in cases {
            let text = case["text"].as_str().unwrap();
            let query = case["query"].as_str().unwrap();
            let (long, short, words) = message_tokens(query);
            assert_eq!(
                serde_json::json!({"long":long,"short":short,"words":words}),
                case["tokens"],
                "tokens: {query}"
            );
            let actual = match_message(text, query);
            let expected = case["expected"]["quality"].as_f64().map(|quality| {
                (
                    quality,
                    case["expected"]["offset"].as_u64().unwrap() as usize,
                )
            });
            assert_eq!(actual, expected, "query={query}, text={text}");
        }
    }
    #[test]
    fn native_title_rank_matches_typescript() {
        let cases: Vec<Value> =
            serde_json::from_str(include_str!("../tests/fixtures/transport_title_rank.json"))
                .unwrap();
        for case in cases {
            let rows: Vec<_> = case["rows"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| TitleCandidate {
                    thread: row["thread"].clone(),
                    project_title: row["projectTitle"].as_str().unwrap().into(),
                    latest_user_message_at: row["latestUserMessageAt"]
                        .as_str()
                        .map(|s| t3_contracts::wire::WireDateTime(s.into())),
                })
                .collect();
            let query = case["query"].as_str().unwrap();
            let actual = NativeThreadSearchPolicy::default()
                .rank_titles(&rows, query, None, case["now"].as_i64().unwrap(), false)
                .unwrap();
            assert_eq!(
                serde_json::to_value(actual).unwrap(),
                case["expected"],
                "{query}"
            );
        }
    }
}
