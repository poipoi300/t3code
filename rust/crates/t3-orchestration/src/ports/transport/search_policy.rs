//! Concrete palette search policy. Manifest dependencies are requested from the seam owner.
use crate::error::Result;
use crate::ports::transport::thread_search::{
    MessageHit, MessageSearchRequest, ThreadSearchPolicy, TitleCandidate,
};
use regex::Regex;
use serde_json::Value;
use unicode_general_category::GeneralCategory;
use unicode_normalization::UnicodeNormalization;
#[path = "palette_search.rs"]
pub mod palette;
use palette::{SearchPullRequest, ThreadSubject};

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
        return false;
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
    value
        .and_then(crate::ports::decisions::orchestrator::epoch_millis)
        .map(|ms| ms as f64)
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
fn picked_at(subject: &ThreadSubject, query: &str, now: f64) -> f64 {
    let query = palette::picked_query(query);
    let Some(opens) = &subject.opens else {
        return 0.0;
    };
    let mut picks: Vec<_> = opens
        .picks
        .iter()
        .filter(|pick| {
            now - pick.at.as_f64().unwrap_or(0.0) <= 21.0 * 86400000.0
                && (pick.query == query
                    || (query.encode_utf16().count() >= 3 && pick.query.starts_with(&query)))
        })
        .collect();
    picks.sort_by(|a, b| {
        (b.query == query).cmp(&(a.query == query)).then_with(|| {
            b.at.as_f64()
                .unwrap_or(0.0)
                .total_cmp(&a.at.as_f64().unwrap_or(0.0))
        })
    });
    picks
        .first()
        .map_or(0.0, |pick| pick.at.as_f64().unwrap_or(0.0))
}
fn usage(subject: &ThreadSubject, now: f64) -> f64 {
    let decay = |age: f64| 2f64.powf(-age.max(0.0) / (14.0 * 86400000.0));
    let recency = subject.activity_at.map_or(0.0, |at| decay(now - at));
    let opens = subject.opens.as_ref().map_or(0.0, |opens| {
        opens.score.value() * decay(now - opens.opened_at.as_f64().unwrap_or(0.0))
    });
    (1.0 + recency + 0.5 * (1.0 + opens).log2()).min(2.5)
}
impl ThreadSearchPolicy for NativeThreadSearchPolicy {
    fn message_search(&self, query: &str, index: bool) -> Option<MessageSearchRequest> {
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
    fn shown_message(&self, text: &str) -> String {
        static LINKS: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
            Regex::new(r"!?\[([^\]\n]*)\]\([^)\s]*\)").expect("constant displayed-link regex")
        });
        LINKS.replace_all(text, "$1").into_owned()
    }
    fn match_message(&self, text: &str, query: &str) -> Option<(f64, usize)> {
        self.matcher(query).matches(text)
    }
    fn rank_messages(
        &self,
        hits: &[MessageHit],
        query: &str,
        now: i64,
        picks: bool,
    ) -> Result<Vec<usize>> {
        let mut rows: Vec<_> = hits
            .iter()
            .enumerate()
            .map(|(index, hit)| {
                let subject = ThreadSubject {
                    title: String::new(),
                    activity_at: activity(hit.activity_at.as_ref().map(|date| date.0.as_str())),
                    opens: opens(hit.opens.as_ref()),
                    ..Default::default()
                };
                (
                    index,
                    hit,
                    if picks {
                        picked_at(&subject, query, now as f64)
                    } else {
                        0.0
                    },
                    (1.0 + (1.0 + hit.hits as f64).log2()) * usage(&subject, now as f64),
                )
            })
            .collect();
        rows.sort_by(|a, b| {
            (b.1.quality >= 0.8)
                .cmp(&(a.1.quality >= 0.8))
                .then_with(|| b.2.total_cmp(&a.2))
                .then_with(|| b.1.quality.total_cmp(&a.1.quality))
                .then_with(|| b.3.total_cmp(&a.3))
                .then_with(|| {
                    b.1.message_created_at
                        .as_ref()
                        .map(|date| &date.0)
                        .cmp(&a.1.message_created_at.as_ref().map(|date| &date.0))
                })
        });
        Ok(rows.into_iter().map(|row| row.0).collect())
    }
    fn rank_titles(
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_message_ranking_matches_typescript_picks_quality_hits_and_recency() {
        let cases: Vec<Value> = serde_json::from_str(include_str!(
            "../../../tests/ports/fixtures/transport_message_rank.json"
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
            "../../../tests/ports/fixtures/transport_message_match.json"
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
        let cases: Vec<Value> = serde_json::from_str(include_str!(
            "../../../tests/ports/fixtures/transport_title_rank.json"
        ))
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
