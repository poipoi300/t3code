//! Shared paletteSearch.ts matching, with UTF-16 highlight offsets; native transport adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TextRange {
    pub start: usize,
    pub end: usize,
}
pub fn is_js_whitespace(c: char) -> bool {
    t3_contracts::wire::trim_js(&c.to_string()).is_empty()
}
trait JsTrim {
    fn js_trim(&self) -> &str;
}
impl JsTrim for str {
    fn js_trim(&self) -> &str {
        t3_contracts::wire::trim_js(self)
    }
}
use std::cmp::Ordering;
use unicode_general_category::{GeneralCategory as Category, get_general_category};
use unicode_normalization::UnicodeNormalization;

pub(super) fn category(c: char) -> Category {
    get_general_category(c)
}
fn letter(c: char) -> bool {
    matches!(
        category(c),
        Category::UppercaseLetter
            | Category::LowercaseLetter
            | Category::TitlecaseLetter
            | Category::ModifierLetter
            | Category::OtherLetter
    )
}
fn digit(c: char) -> bool {
    matches!(
        category(c),
        Category::DecimalNumber | Category::LetterNumber | Category::OtherNumber
    )
}
fn alnum(c: char) -> bool {
    letter(c) || digit(c)
}
pub(super) fn alnum_unit(c: u16) -> bool {
    char::from_u32(u32::from(c)).is_some_and(alnum)
}
pub(super) fn symbol(c: char) -> bool {
    category(c) == Category::OtherSymbol
}
pub(super) fn cjk(c: char) -> bool {
    matches!(u32::from(c),0x3040..=0x30ff|0x3400..=0x4dbf|0x4e00..=0x9fff|0xf900..=0xfaff)
}
fn lower(c: char) -> bool {
    category(c) == Category::LowercaseLetter
}
fn upper(c: char) -> bool {
    category(c) == Category::UppercaseLetter
}
pub(super) fn word_start(
    before: Option<char>,
    previous: Option<char>,
    c: char,
    next: Option<char>,
) -> bool {
    let apostrophe = previous.is_some_and(|p| matches!(p, '\'' | '’')) && before.is_some_and(alnum);
    (alnum(c) || symbol(c))
        && !apostrophe
        && (previous.is_none_or(|p| !alnum(p))
            || previous.is_some_and(|p| lower(p) && upper(c))
            || previous.is_some_and(|p| upper(p) && upper(c) && next.is_some_and(lower))
            || previous.is_some_and(|p| digit(p) != digit(c))
            || cjk(c))
}
pub fn fold_in_place(text: &str) -> String {
    text.chars()
        .map(|c| {
            if !('\u{80}'..='\u{ffff}').contains(&c) {
                return c.to_string();
            }
            let folded = c
                .to_string()
                .nfkd()
                .filter(|c| {
                    !matches!(
                        category(*c),
                        Category::NonspacingMark | Category::SpacingMark | Category::EnclosingMark
                    )
                })
                .collect::<String>()
                .to_lowercase()
                .replace('ς', "σ")
                .replace(['‘', '’'], "'");
            if folded.encode_utf16().count() == 1 {
                folded
            } else {
                c.to_string()
            }
        })
        .collect::<String>()
        .to_lowercase()
}
pub fn fold_query(query: &str) -> String {
    let quote = |c| matches!(c, '"' | '\'' | '`' | '“' | '”' | '‘' | '’');
    let mut query = query;
    let start = query.trim_start_matches(is_js_whitespace);
    if start.chars().next().is_some_and(quote) {
        query = start.trim_start_matches(quote);
    }
    let end = query.trim_end_matches(is_js_whitespace);
    if end.chars().last().is_some_and(quote) {
        query = end.trim_end_matches(quote);
    }
    fold_in_place(&query.nfc().collect::<String>())
        .split(is_js_whitespace)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}
#[derive(Debug, Clone)]
pub struct Field {
    pub text: Vec<u16>,
    starts: Vec<bool>,
}
impl Field {
    pub fn new(source: &str) -> Self {
        let chars = source.chars().collect::<Vec<_>>();
        let mut starts = vec![false; source.encode_utf16().count()];
        let mut at = 0;
        for (i, &c) in chars.iter().enumerate() {
            starts[at] = word_start(
                i.checked_sub(2).and_then(|i| chars.get(i)).copied(),
                i.checked_sub(1).and_then(|i| chars.get(i)).copied(),
                c,
                chars.get(i + 1).copied(),
            );
            at += c.len_utf16();
        }
        Self {
            text: fold_in_place(source).encode_utf16().collect(),
            starts,
        }
    }
    fn starts(&self, i: usize) -> bool {
        self.starts.get(i).copied().unwrap_or(false)
    }
    fn word_end(&self, i: usize) -> usize {
        let mut end = i + 1;
        while end < self.text.len() && !self.starts(end) && alnum_unit(self.text[end]) {
            end += 1;
        }
        end
    }
}
#[derive(Debug, Clone)]
pub struct WeightedField {
    pub field: Field,
    pub weight: f64,
    pub whole_words: bool,
    pub keyword: bool,
    pub context: bool,
    pub mid_word: bool,
}
impl WeightedField {
    pub fn new(text: &str, weight: f64) -> Self {
        Self {
            field: Field::new(text),
            weight,
            whole_words: false,
            keyword: false,
            context: false,
            mid_word: false,
        }
    }
}
#[derive(Debug, Clone)]
struct TokenMatch {
    score: f64,
    range: TextRange,
    letters: Vec<TextRange>,
}
fn one_edit(a: &[u16], b: &[u16], swap_only: bool) -> bool {
    if a.len().abs_diff(b.len()) > usize::from(!swap_only) {
        return false;
    }
    let i = a.iter().zip(b).take_while(|(a, b)| a == b).count();
    let a = &a[i..];
    let b = &b[i..];
    let swapped = a.len() > 1 && b.len() > 1 && a[0] == b[1] && a[1] == b[0] && a[2..] == b[2..];
    if swap_only {
        return swapped;
    }
    swapped
        || a == b
        || a.get(1..).unwrap_or_default() == b.get(1..).unwrap_or_default()
        || a.get(1..).unwrap_or_default() == b
        || a == b.get(1..).unwrap_or_default()
}
fn joined(field: &Field, token: &[u16]) -> Option<TokenMatch> {
    for start in 0..field.text.len() {
        if !field.starts(start) || field.text[start] != token[0] {
            continue;
        }
        let mut runs = vec![];
        let mut at = start;
        let mut rest = token;
        while !rest.is_empty() && at < field.text.len() {
            let end = field.word_end(at);
            if rest.len() <= end - at && field.text[at..].starts_with(rest) {
                runs.push(TextRange {
                    start: at,
                    end: at + rest.len(),
                });
                rest = &[];
            } else if rest.starts_with(&field.text[at..end]) {
                runs.push(TextRange { start: at, end });
                rest = &rest[end - at..];
                at = end;
                while at < field.text.len() && !field.starts(at) {
                    at += 1;
                }
            } else {
                break;
            }
        }
        let Some(last) = runs.last() else {
            continue;
        };
        if !rest.is_empty() || runs.len() < 2 {
            continue;
        }
        if last.end == field.word_end(last.start)
            || last.end - last.start >= 3
            || Some(start) == field.starts.iter().position(|v| *v)
            || runs[..runs.len() - 1].iter().all(|r| r.end - r.start >= 3)
        {
            return Some(TokenMatch {
                score: 0.6,
                range: TextRange {
                    start,
                    end: last.end,
                },
                letters: runs,
            });
        }
    }
    None
}
fn match_token(
    field: &Field,
    token: &[u16],
    typos: bool,
    primary: bool,
    mid_word: bool,
) -> Option<TokenMatch> {
    if token.is_empty() {
        return None;
    }
    let mut best = None;
    let own = char::from_u32(u32::from(token[0])).is_none_or(|c| !alnum(c) && !symbol(c));
    for i in 0..field.text.len() {
        if !field.text[i..].starts_with(token) {
            continue;
        }
        if field.starts(i) || own {
            let end = i + token.len();
            let whole =
                end >= field.text.len() || field.starts(end) || !alnum_unit(field.text[end]);
            let mut run_end = end;
            while run_end < field.text.len() && alnum_unit(field.text[run_end]) {
                run_end += 1;
            }
            let score = if whole {
                1.0
            } else {
                0.7 + 0.2 * token.len() as f64 / (run_end - i) as f64
            };
            if best.as_ref().is_none_or(|b: &TokenMatch| score > b.score) {
                best = Some(TokenMatch {
                    score,
                    range: TextRange { start: i, end },
                    letters: vec![],
                });
            }
            if whole {
                return best;
            }
        } else if mid_word && token.len() >= 4 && best.is_none() {
            best = Some(TokenMatch {
                score: 0.3,
                range: TextRange {
                    start: i,
                    end: i + token.len(),
                },
                letters: vec![],
            });
        }
    }
    if primary
        && best.as_ref().is_none_or(|b| b.score <= 0.3)
        && let Some(found) = joined(field, token)
    {
        return Some(found);
    }
    if best.as_ref().is_some_and(|b| b.score > 0.3) || !typos || token.len() < 3 {
        return best;
    }
    if String::from_utf16_lossy(token).chars().any(letter) {
        for i in 0..field.text.len() {
            if !field.starts(i) {
                continue;
            }
            if field.text[i] != token[0]
                && !(token.len() > 3
                    && field.text[i] == token[1]
                    && field.text.get(i + 1) == Some(&token[0]))
            {
                continue;
            }
            let mut end = i + 1;
            while end < field.text.len() && alnum_unit(field.text[end]) {
                end += 1;
            }
            let short = token.len() <= 4;
            let lengths = if short {
                vec![token.len(), token.len() + 1]
            } else {
                vec![token.len(), token.len() - 1, token.len() + 1]
            };
            for len in lengths {
                if len <= end - i
                    && one_edit(token, &field.text[i..i + len], short && len == token.len())
                {
                    return Some(TokenMatch {
                        score: 0.45,
                        range: TextRange {
                            start: i,
                            end: i + len,
                        },
                        letters: vec![],
                    });
                }
            }
        }
    }
    best
}
fn filler(token: &str) -> bool {
    matches!(
        token,
        "a" | "an" | "and" | "for" | "in" | "of" | "on" | "the" | "to" | "with"
    )
}
pub fn merge_ranges(mut ranges: Vec<TextRange>) -> Vec<TextRange> {
    ranges.sort_by_key(|r| r.start);
    let mut merged: Vec<TextRange> = vec![];
    for range in ranges {
        if let Some(last) = merged.last_mut().filter(|r| range.start <= r.end) {
            last.end = last.end.max(range.end);
        } else {
            merged.push(range);
        }
    }
    merged
}
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SearchMatch {
    pub score: f64,
    pub quality: f64,
    pub whole_words: bool,
    pub strong: bool,
    pub ranges: Vec<TextRange>,
    pub group_quality: Option<f64>,
    pub picked_at: Option<f64>,
    pub picked_exactly: bool,
    pub hint: Option<MatchHint>,
}
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MatchHint {
    pub label: String,
    pub text: String,
    pub ranges: Vec<TextRange>,
}
pub fn match_fields(query: &str, fields: &[WeightedField], typos: bool) -> Option<SearchMatch> {
    if query.is_empty() {
        return None;
    }
    let tokens = query
        .split(' ')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>();
    if tokens.is_empty() {
        return None;
    }
    let optional = tokens.iter().any(|t| !filler(t));
    let mut ranges = vec![];
    let mut total = 0.0;
    let mut counted = 0;
    let mut not_whole = false;
    let mut strong = true;
    for token in &tokens {
        let units = token.encode_utf16().collect::<Vec<_>>();
        let mut best: f64 = 0.0;
        let mut best_primary = None;
        let mut boundary = false;
        for (i, weighted) in fields.iter().enumerate() {
            if weighted.field.text.is_empty() {
                continue;
            }
            if best >= weighted.weight && i != 0 && boundary {
                continue;
            }
            let Some(found) = match_token(
                &weighted.field,
                &units,
                typos && !weighted.keyword,
                i == 0,
                weighted.mid_word,
            ) else {
                continue;
            };
            if found.score
                < if weighted.whole_words {
                    1.0
                } else if weighted.keyword {
                    0.7
                } else {
                    0.0
                }
            {
                continue;
            }
            boundary |= found.score >= 0.7 && !weighted.context;
            best = best.max(found.score * weighted.weight);
            if i == 0 {
                best_primary = Some(found);
            }
        }
        if best == 0.0 {
            if optional && filler(token) {
                continue;
            }
            return None;
        }
        strong &= boundary;
        if let Some(found) = best_primary {
            if found.letters.is_empty() {
                ranges.push(found.range);
            } else {
                ranges.extend(found.letters);
            }
        }
        total += best;
        counted += 1;
        if filler(token) {
            not_whole |= !optional;
        } else {
            not_whole |= units.len() < 3 || best < 1.0;
        }
    }
    let mut quality = total / f64::from(counted);
    if let Some(primary) = fields.first() {
        let text = String::from_utf16_lossy(&primary.field.text);
        if text == query {
            quality += 0.5;
        } else if tokens.len() > 1 && text.contains(query) {
            quality += 0.25;
        }
    }
    Some(SearchMatch {
        score: quality,
        quality,
        whole_words: !not_whole,
        strong,
        ranges: merge_ranges(ranges),
        ..Default::default()
    })
}
pub fn score_terms(query: &str, terms: &[String], typos: bool) -> Option<SearchMatch> {
    let meaningful = query
        .split(' ')
        .filter(|t| !filler(t))
        .collect::<Vec<_>>()
        .join(" ");
    let short = if meaningful.is_empty() {
        query
    } else {
        &meaningful
    }
    .encode_utf16()
    .count()
        < 4;
    let fields = terms
        .iter()
        .enumerate()
        .map(|(i, text)| {
            let mut field = WeightedField::new(
                text,
                if i == 0 {
                    1.0
                } else if short {
                    0.8
                } else {
                    0.6
                },
            );
            if i != 0 {
                field.whole_words = short;
                field.keyword = true;
            }
            field
        })
        .collect::<Vec<_>>();
    let mut found = match_fields(query, &fields, typos)?;
    found.whole_words = found.quality >= 1.0;
    if found.ranges.is_empty() {
        found.group_quality = Some(found.quality.min(0.65));
    }
    Some(found)
}
pub fn match_class(quality: f64) -> u8 {
    if quality >= 1.5 {
        3
    } else if quality >= 0.7 {
        2
    } else if quality >= 0.4 {
        1
    } else {
        0
    }
}
pub fn compare_matches(a: &SearchMatch, b: &SearchMatch) -> Ordering {
    let exact = |m: &SearchMatch| {
        if m.picked_exactly {
            m.picked_at.unwrap_or(0.0)
        } else {
            0.0
        }
    };
    match_class(b.quality)
        .min(2)
        .cmp(&match_class(a.quality).min(2))
        .then_with(|| exact(b).total_cmp(&exact(a)))
        .then(match_class(b.quality).cmp(&match_class(a.quality)))
        .then_with(|| {
            b.picked_at
                .unwrap_or(0.0)
                .total_cmp(&a.picked_at.unwrap_or(0.0))
        })
        .then(b.whole_words.cmp(&a.whole_words))
        .then_with(|| b.score.total_cmp(&a.score))
}
pub fn rank_items<T>(
    items: &[T],
    score: impl Fn(&T, bool) -> Option<SearchMatch>,
) -> Vec<(&T, SearchMatch)> {
    let rank = |typos| {
        items
            .iter()
            .filter_map(|t| score(t, typos).map(|m| (t, m)))
            .collect::<Vec<_>>()
    };
    let mut rows = rank(false);
    if !rows.iter().any(|(_, m)| m.strong) {
        rows = rank(true);
    }
    rows.sort_by(|(_, a), (_, b)| compare_matches(a, b));
    rows
}

pub struct RankedGroup<'a, G, T> {
    pub group: &'a G,
    pub ranked: Vec<(&'a T, SearchMatch)>,
}
pub fn rank_groups<'a, G, T>(
    groups: &'a [(G, Vec<T>)],
    score: impl Fn(&T, bool, &G) -> Option<SearchMatch>,
) -> Vec<RankedGroup<'a, G, T>> {
    let rank = |typos| {
        groups
            .iter()
            .map(|(group, items)| RankedGroup {
                group,
                ranked: items
                    .iter()
                    .filter_map(|item| score(item, typos, group).map(|m| (item, m)))
                    .collect(),
            })
            .collect::<Vec<_>>()
    };
    let mut ranked = rank(false);
    if !ranked
        .iter()
        .any(|g| g.ranked.iter().any(|(_, m)| m.strong))
    {
        ranked = rank(true);
    }
    ranked.retain(|g| !g.ranked.is_empty());
    for group in &mut ranked {
        group.ranked.sort_by(|(_, a), (_, b)| compare_matches(a, b));
    }
    let best = |g: &RankedGroup<'_, G, T>| {
        g.ranked
            .iter()
            .map(|(_, m)| match_class(m.group_quality.unwrap_or(m.quality)))
            .max()
            .unwrap_or(0)
    };
    ranked.sort_by_key(|g| std::cmp::Reverse(best(g)));
    ranked
}
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchPullRequest {
    pub number: u64,
    pub title: Option<String>,
    #[serde(default)]
    pub references: Vec<String>,
}
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadSubject {
    pub id: Option<String>,
    pub title: String,
    pub branch: Option<String>,
    pub project_title: Option<String>,
    pub environment_label: Option<String>,
    #[serde(default)]
    pub pull_requests: Vec<SearchPullRequest>,
    pub activity_at: Option<f64>,
    pub opens: Option<t3_contracts::orchestration_v2::OrchestrationV2ThreadOpens>,
}
fn searchable_branch(branch: &str) -> &str {
    for prefix in ["t3/", "t3code/"] {
        if let Some(rest) = branch.strip_prefix(prefix)
            && rest.chars().next().is_some_and(|c| !is_js_whitespace(c))
        {
            return rest;
        }
    }
    if let Some(rest) = branch.strip_prefix("t3-")
        && rest.len() == 8
        && rest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return rest;
    }
    branch
}
fn field_query(query: &str) -> String {
    let mut query = query;
    for prefix in ["pr", "pull request", "pull"] {
        if let Some(rest) = query.strip_prefix(prefix) {
            let rest = rest.trim_start_matches(is_js_whitespace);
            let digits = rest.strip_prefix('#').unwrap_or(rest);
            if digits.as_bytes().first().is_some_and(u8::is_ascii_digit) {
                query = rest;
                break;
            }
        }
    }
    if let Some(rest) = query.strip_prefix('#') {
        query = rest.trim_start_matches(is_js_whitespace);
    }
    searchable_branch(query)
        .split(' ')
        .map(|t| {
            t.strip_prefix("t3-")
                .filter(|rest| {
                    !rest.is_empty()
                        && rest.len() <= 8
                        && rest
                            .bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                })
                .unwrap_or(t)
        })
        .collect::<Vec<_>>()
        .join(" ")
}
pub fn picked_query(query: &str) -> String {
    let folded = fold_query(query);
    let mut units = folded.encode_utf16().take(32).collect::<Vec<_>>();
    if units.last().is_some_and(|c| (0xd800..=0xdbff).contains(c)) {
        units.pop();
    }
    String::from_utf16_lossy(&units).js_trim().into()
}

pub fn record_thread_open(
    previous: serde_json::Value,
    at: f64,
    query: Option<&str>,
) -> serde_json::Value {
    use serde_json::{Value, json};
    let pick = picked_query(query.unwrap_or_default());
    let mut picks: Vec<_> = previous["picks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| at - p["at"].as_f64().unwrap_or(0.0) <= 21.0 * 86400000.0)
        .cloned()
        .collect();
    if !pick.is_empty() {
        picks.retain(|p| p["query"] != pick);
        picks.insert(0, json!({"query":pick,"at":at}));
        picks.truncate(3);
    }
    if !previous.is_null() && at - previous["openedAt"].as_f64().unwrap_or(0.0) < 600000.0 {
        let mut previous = previous;
        previous["picks"] = Value::Array(picks);
        return previous;
    }
    let score = if previous.is_null() {
        1.0
    } else {
        previous["score"].as_f64().unwrap_or(0.0)
            * decay(at - previous["openedAt"].as_f64().unwrap_or(0.0))
            + 1.0
    };
    json!({"openedAt":at,"score":score,"picks":picks})
}

pub fn find_search_token_ranges(text: &str, query: &str) -> Vec<TextRange> {
    let (long, _, _) = crate::search_policy::message_tokens(query);
    let field = Field::new(text);
    let mut tokens = std::collections::BTreeMap::new();
    for token in long {
        tokens.insert(fold_query(&token), token.chars().count() < 4);
    }
    let mut ranges = vec![];
    for (token, needs_start) in tokens {
        let token: Vec<_> = token.encode_utf16().collect();
        if token.is_empty() {
            continue;
        }
        let own = char::from_u32(u32::from(token[0])).is_none_or(|c| !alnum(c) && !symbol(c));
        let mut at = 0;
        while at + token.len() <= field.text.len() {
            if field.text[at..].starts_with(&token) && (!needs_start || own || field.starts(at)) {
                ranges.push(TextRange {
                    start: at,
                    end: at + token.len(),
                });
                at += token.len();
            } else {
                at += 1;
            }
        }
    }
    merge_ranges(ranges)
}
fn decay(age: f64) -> f64 {
    2.0_f64.powf(-age.max(0.0) / (14.0 * 86_400_000.0))
}
pub fn recent_use(thread: &ThreadSubject) -> f64 {
    thread.activity_at.unwrap_or(0.0).max(
        thread
            .opens
            .as_ref()
            .map_or(0.0, |o| o.opened_at.as_f64().unwrap_or(0.0)),
    )
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageMatchCandidate {
    pub subject: Option<ThreadSubject>,
    pub quality: Option<f64>,
    pub hits: Option<u64>,
    pub message_created_at: Option<String>,
}

pub fn rank_message_matches(
    matches: &[MessageMatchCandidate],
    query: &str,
    now: f64,
    picks: bool,
    without_title_matches: bool,
) -> Vec<usize> {
    let picked_query = picked_query(query);
    let mut rows: Vec<_> = matches
        .iter()
        .enumerate()
        .filter_map(|(index, m)| {
            let subject = m.subject.as_ref()?;
            if without_title_matches && score_thread(query, subject, now, false, false).is_some() {
                return None;
            }
            let quality = m.quality.unwrap_or(1.0);
            let pick = if picks {
                subject
                    .opens
                    .as_ref()
                    .and_then(|opens| {
                        opens
                            .picks
                            .iter()
                            .filter(|p| now - p.at.value() <= 21.0 * 86400000.0)
                            .filter(|p| {
                                p.query == picked_query
                                    || (picked_query.encode_utf16().count() >= 3
                                        && p.query.starts_with(&picked_query))
                            })
                            .max_by(|a, b| {
                                (a.query == picked_query)
                                    .cmp(&(b.query == picked_query))
                                    .then_with(|| a.at.value().total_cmp(&b.at.value()))
                            })
                    })
                    .map_or(0.0, |p| p.at.value())
            } else {
                0.0
            };
            let recency = subject.activity_at.map_or(0.0, |at| decay(now - at));
            let opens = subject
                .opens
                .as_ref()
                .map_or(0.0, |o| o.score.value() * decay(now - o.opened_at.value()));
            let score = (1.0 + (1.0 + m.hits.unwrap_or(1) as f64).log2())
                * (1.0 + recency + 0.5 * (1.0 + opens).log2()).min(2.5);
            Some((
                index,
                quality,
                pick,
                score,
                m.message_created_at.as_deref().unwrap_or_default(),
            ))
        })
        .collect();
    rows.sort_by(|a, b| {
        (b.1 >= 0.8)
            .cmp(&(a.1 >= 0.8))
            .then_with(|| b.2.total_cmp(&a.2))
            .then_with(|| b.1.total_cmp(&a.1))
            .then_with(|| b.3.total_cmp(&a.3))
            .then_with(|| b.4.cmp(a.4))
    });
    rows.into_iter().map(|r| r.0).collect()
}
fn hint(query: &str, thread: &ThreadSubject, typos: bool) -> Option<MatchHint> {
    let mut candidates = thread
        .pull_requests
        .iter()
        .map(|pr| {
            (
                "PR",
                format!(
                    "#{}{}",
                    pr.number,
                    pr.title
                        .as_ref()
                        .filter(|s| !s.is_empty())
                        .map_or(String::new(), |s| format!(" {s}"))
                ),
            )
        })
        .collect::<Vec<_>>();
    if let Some(branch) = &thread.branch {
        candidates.push(("Branch", searchable_branch(branch).into()));
    }
    if let Some(title) = &thread.project_title {
        candidates.push(("Project", title.clone()));
    }
    if let Some(label) = &thread.environment_label {
        candidates.push(("Environment", label.clone()));
    }
    let find = |query: &str| {
        candidates.iter().find_map(|(label, text)| {
            match_fields(query, &[WeightedField::new(text, 1.0)], typos).map(|m| MatchHint {
                label: (*label).into(),
                text: text.clone(),
                ranges: m.ranges,
            })
        })
    };
    if let Some(hint) = find(query) {
        return Some(hint);
    }
    let contexts = [
        thread.project_title.as_deref(),
        thread.environment_label.as_deref(),
    ];
    let rest = query
        .split(' ')
        .filter(|token| {
            !contexts
                .into_iter()
                .flatten()
                .any(|text| match_fields(token, &[WeightedField::new(text, 1.0)], typos).is_some())
        })
        .collect::<Vec<_>>()
        .join(" ");
    if rest.is_empty() || rest == query {
        None
    } else {
        find(&rest)
    }
}
pub fn score_thread(
    query: &str,
    thread: &ThreadSubject,
    now: f64,
    typos: bool,
    picks: bool,
) -> Option<SearchMatch> {
    let mut title = WeightedField::new(&thread.title, 1.0);
    title.mid_word = true;
    let mut fields = vec![title];
    for pr in &thread.pull_requests {
        fields.push(WeightedField::new(&format!("#{}", pr.number), 1.0));
        if let Some(title) = &pr.title {
            fields.push(WeightedField::new(title, 0.5));
        }
    }
    if let Some(branch) = &thread.branch {
        fields.push(WeightedField::new(searchable_branch(branch), 0.6));
    }
    for text in [&thread.project_title, &thread.environment_label]
        .into_iter()
        .flatten()
    {
        let mut field = WeightedField::new(text, 0.25);
        field.context = true;
        fields.push(field);
    }
    let field_query = field_query(query);
    let matched = match_fields(&field_query, &fields, typos);
    let reference = matched.is_none()
        && query.encode_utf16().count() >= 8
        && thread
            .id
            .iter()
            .chain(thread.pull_requests.iter().flat_map(|pr| &pr.references))
            .any(|r| {
                let text = r.to_lowercase();
                let not_cut = |text: &str, end: usize| {
                    text.get(end..)
                        .and_then(|s| s.chars().next())
                        .is_none_or(|c| !digit(c))
                };
                if query.chars().any(digit) {
                    text.find(query)
                        .is_some_and(|at| not_cut(&text, at + query.len()))
                        || (query.starts_with(&text) && not_cut(query, text.len()))
                } else {
                    text.starts_with(query)
                }
            });
    let mut found = matched.or_else(|| {
        reference.then_some(SearchMatch {
            score: 0.6,
            quality: 0.6,
            strong: true,
            ..Default::default()
        })
    })?;
    if found.ranges.is_empty() {
        found.hint = if reference {
            Some(MatchHint {
                label: "ID or link".into(),
                text: query.into(),
                ranges: vec![],
            })
        } else {
            hint(&field_query, thread, typos)
        };
        if !reference && field_query.contains(' ') && found.hint.is_none() {
            return None;
        }
    }
    let recency = thread.activity_at.map_or(0.0, |at| decay(now - at));
    let opens = thread.opens.as_ref().map_or(0.0, |o| {
        o.score.value() * decay(now - o.opened_at.as_f64().unwrap_or(0.0))
    });
    found.score = found.quality * (1.0 + recency + 0.5 * (1.0 + opens).log2()).min(2.5);
    found.group_quality = Some(found.quality.min(1.49));
    if picks && let Some(opens) = &thread.opens {
        let query = picked_query(query);
        let pick = opens
            .picks
            .iter()
            .filter(|p| now - p.at.as_f64().unwrap_or(0.0) <= 21.0 * 86_400_000.0)
            .filter(|p| {
                p.query == query
                    || (query.encode_utf16().count() >= 3 && p.query.starts_with(&query))
            })
            .max_by(|a, b| {
                (a.query == query).cmp(&(b.query == query)).then_with(|| {
                    a.at.as_f64()
                        .unwrap_or(0.0)
                        .total_cmp(&b.at.as_f64().unwrap_or(0.0))
                })
            });
        if let Some(pick) = pick {
            found.picked_at = Some(pick.at.as_f64().unwrap_or(0.0));
            found.picked_exactly = pick.query == query;
        }
    }
    Some(found)
}
