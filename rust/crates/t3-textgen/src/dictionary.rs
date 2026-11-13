use crate::text::utf16_len;
use regex::Regex;
use t3_contracts::speech::SpeechCustomWord;

fn space(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}
pub fn normalize_custom_words(words: &[SpeechCustomWord]) -> Vec<SpeechCustomWord> {
    let mut seen = std::collections::HashSet::new();
    let mut output = Vec::new();
    for word in words {
        let term = space(&word.term);
        if term.is_empty() || utf16_len(&term) > 50 || !seen.insert(term.to_lowercase()) {
            continue;
        }
        let mut aliases = Vec::new();
        for alias in &word.aliases {
            let alias = space(alias);
            if !alias.is_empty() && utf16_len(&alias) <= 50 && seen.insert(alias.to_lowercase()) {
                aliases.push(alias);
            }
        }
        aliases.truncate(8);
        output.push(SpeechCustomWord { term, aliases });
    }
    output.truncate(100);
    output
}
fn word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_' || matches!(ch, '\u{300}'..='\u{36f}')
}
fn boundary(text: &str, start: usize, end: usize) -> bool {
    text[..start]
        .chars()
        .next_back()
        .is_none_or(|c| !word_char(c))
        && text[end..].chars().next().is_none_or(|c| !word_char(c))
}
fn key(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}
fn ascii_key(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|b| b.is_ascii_alphanumeric())
}
fn soundex(value: &str) -> String {
    let upper = value.to_uppercase();
    let mut chars = upper.chars();
    let Some(first) = chars.next() else {
        return "0000".into();
    };
    let code = |c| match c {
        'B' | 'F' | 'P' | 'V' => '1',
        'C' | 'G' | 'J' | 'K' | 'Q' | 'S' | 'X' | 'Z' => '2',
        'D' | 'T' => '3',
        'L' => '4',
        'M' | 'N' => '5',
        'R' => '6',
        _ => '0',
    };
    let mut previous = code(first);
    let mut result = String::from(first);
    for ch in chars {
        let next = code(ch);
        if next != '0' && next != previous {
            result.push(next);
        }
        previous = next;
    }
    result.push_str("000");
    result.chars().take(4).collect()
}
fn distance(left: &str, right: &str) -> usize {
    let mut previous = (0..=right.len()).collect::<Vec<_>>();
    for (i, a) in left.bytes().enumerate() {
        let mut current = vec![i + 1];
        for (j, b) in right.bytes().enumerate() {
            current.push(
                (current[j] + 1)
                    .min(previous[j + 1] + 1)
                    .min(previous[j] + usize::from(a != b)),
            );
        }
        previous = current;
    }
    previous[right.len()]
}
fn preserve_case(original: &str, replacement: &str) -> String {
    let letters = original
        .chars()
        .filter(|c| c.is_alphabetic())
        .collect::<Vec<_>>();
    if !letters.is_empty() && letters.iter().all(|c| c.is_uppercase()) {
        return replacement.to_uppercase();
    }
    if letters.first().is_some_and(|c| c.is_uppercase()) {
        let mut chars = replacement.chars();
        return chars.next().map_or_else(String::new, |c| {
            c.to_uppercase().collect::<String>() + chars.as_str()
        });
    }
    replacement.into()
}

pub struct TextCorrector {
    keys: Vec<(String, String)>,
    spellings: Vec<(String, String)>,
    alias_pattern: Option<Regex>,
}
impl TextCorrector {
    pub fn new(words: &[String], dictionary: &[SpeechCustomWord]) -> Self {
        let mut keys = Vec::new();
        for word in words {
            let mut sources = vec![word.clone()];
            if word.contains('&') {
                sources.push(word.replace('&', " and "));
            }
            let mut known = std::collections::HashSet::new();
            for source in sources {
                let candidate = key(&source);
                let lost =
                    candidate != source.split_whitespace().collect::<String>().to_lowercase();
                if candidate.len() < 2 && lost {
                    continue;
                }
                if ascii_key(&candidate) && known.insert(candidate.clone()) {
                    keys.push((word.clone(), candidate));
                }
            }
        }
        let mut spellings = dictionary
            .iter()
            .flat_map(|w| {
                std::iter::once(&w.term)
                    .chain(&w.aliases)
                    .map(|s| (s.clone(), w.term.clone()))
            })
            .collect::<Vec<_>>();
        spellings.sort_by_key(|(s, _)| std::cmp::Reverse(utf16_len(s)));
        let alias_pattern = if spellings.is_empty() {
            None
        } else {
            Regex::new(&format!(
                "(?i:{})",
                spellings
                    .iter()
                    .map(|(s, _)| regex::escape(s))
                    .collect::<Vec<_>>()
                    .join("|")
            ))
            .ok()
        };
        Self {
            keys,
            spellings,
            alias_pattern,
        }
    }
    fn best_match(&self, phrase: &str) -> Option<(&str, f64, String)> {
        let candidate = key(phrase);
        let suffix = if phrase.to_lowercase().ends_with("'s") {
            "'s"
        } else if phrase.to_lowercase().ends_with("’s") {
            "’s"
        } else {
            ""
        };
        let stem = if suffix.is_empty() {
            candidate.as_str()
        } else {
            &candidate[..candidate.len().saturating_sub(1)]
        };
        if !ascii_key(stem) || stem.len() > 50 {
            return None;
        }
        let inner = Regex::new("['’]s\\s").expect("possessive pattern");
        let inner_count = inner.find_iter(phrase).count();
        let mut best = None;
        for (word, custom) in &self.keys {
            if inner_count > 0 && inner_count > inner.find_iter(word).count() {
                continue;
            }
            let keep = !suffix.is_empty()
                && !word.to_lowercase().ends_with("'s")
                && !word.to_lowercase().ends_with("’s");
            let candidate = if keep { stem } else { &candidate };
            if candidate.len().min(custom.len()) < 4 && candidate != custom {
                continue;
            }
            let longest = candidate.len().max(custom.len());
            if candidate.len().abs_diff(custom.len()) as f64 > (longest as f64 * 0.25).max(2.0) {
                continue;
            }
            let mut score = distance(candidate, custom) as f64 / longest as f64;
            if candidate.bytes().all(|c| c.is_ascii_alphabetic())
                && custom.bytes().all(|c| c.is_ascii_alphabetic())
                && soundex(candidate) == soundex(custom)
            {
                score *= 0.3;
            }
            if score < 0.18 && best.as_ref().is_none_or(|(_, s, _)| score < *s) {
                best = Some((
                    word.as_str(),
                    score,
                    if keep {
                        suffix.to_owned()
                    } else {
                        String::new()
                    },
                ));
            }
        }
        best
    }
    fn fuzzy(&self, text: &str) -> String {
        if self.keys.is_empty() {
            return text.into();
        }
        let pattern =
            Regex::new(r"[\p{L}\p{M}\p{N}](?:\S*[\p{L}\p{M}\p{N}])?").expect("token pattern");
        let allowed = Regex::new(r"^[\p{L}\p{M}\p{N}'’-]+$").expect("token spelling");
        let gap = Regex::new(r"^[\t\p{Zs}]+$").expect("horizontal space");
        let tokens = pattern
            .find_iter(text)
            .filter(|m| allowed.is_match(m.as_str()))
            .collect::<Vec<_>>();
        let mut output = String::new();
        let mut cursor = 0;
        let mut index = 0;
        while index < tokens.len() {
            let mut best = None::<(usize, &str, f64, String)>;
            for count in (1..=4.min(tokens.len() - index)).rev() {
                let slice = &tokens[index..index + count];
                if slice
                    .windows(2)
                    .any(|pair| !gap.is_match(&text[pair[0].end()..pair[1].start()]))
                {
                    continue;
                }
                if let Some((word, score, suffix)) =
                    self.best_match(&text[slice[0].start()..slice[count - 1].end()])
                    && best
                        .as_ref()
                        .is_none_or(|(n, _, s, _)| score < *s || score == *s && count < *n)
                {
                    best = Some((count, word, score, suffix));
                }
            }
            if let Some((count, word, _, suffix)) = best {
                let first = tokens[index];
                output.push_str(&text[cursor..first.start()]);
                output.push_str(&preserve_case(first.as_str(), word));
                output.push_str(&suffix);
                cursor = tokens[index + count - 1].end();
                index += count;
            } else {
                index += 1;
            }
        }
        output.push_str(&text[cursor..]);
        output
    }
    pub fn apply(&self, text: &str) -> String {
        let Some(pattern) = &self.alias_pattern else {
            return self.fuzzy(text);
        };
        let mut output = String::new();
        let mut cursor = 0;
        for m in pattern.find_iter(text) {
            if !boundary(text, m.start(), m.end()) {
                continue;
            }
            output.push_str(&self.fuzzy(&text[cursor..m.start()]));
            output.push_str(
                self.spellings
                    .iter()
                    .rev()
                    .find(|(spelling, _)| spelling.to_lowercase() == m.as_str().to_lowercase())
                    .map_or(m.as_str(), |(_, term)| term.as_str()),
            );
            cursor = m.end();
        }
        output.push_str(&self.fuzzy(&text[cursor..]));
        output
    }
}
pub fn apply_aliases(text: &str, words: &[SpeechCustomWord]) -> String {
    TextCorrector::new(&[], words).apply(text)
}

pub fn remove_filler_words(text: &str, language: Option<&str>, cue: Option<&str>) -> String {
    let mut words = vec![
        "uh", "uhm", "umm", "uhh", "uhhh", "ehh", "ehm", "ahm", "hmm", "hm", "mmm", "хм", "ммм",
    ];
    match language
        .unwrap_or("")
        .split(['-', '_'])
        .next()
        .unwrap_or("")
        .to_lowercase()
        .as_str()
    {
        "en" => words.extend(["um", "ah", "eh"]),
        "de" => words.extend(["äh", "ähm"]),
        "fr" => words.push("euh"),
        _ => (),
    }
    words.sort_by_key(|word| std::cmp::Reverse(word.len()));
    let cue = cue.unwrap_or("").trim();
    let mut alternatives = Vec::new();
    if !cue.is_empty() {
        alternatives.push(
            cue.split_whitespace()
                .map(regex::escape)
                .collect::<Vec<_>>()
                .join("\\s+"),
        );
    }
    alternatives.push(format!(
        "(?:{})[,.]?",
        words
            .iter()
            .map(|w| regex::escape(w))
            .collect::<Vec<_>>()
            .join("|")
    ));
    let pattern = Regex::new(&format!("(?i:{})", alternatives.join("|"))).expect("filler pattern");
    let cue_key = space(cue).to_lowercase();
    let mut output = String::new();
    let mut cursor = 0;
    let fallback = Regex::new(&format!(
        "(?i:(?:{})[,.]?)",
        words
            .iter()
            .map(|w| regex::escape(w))
            .collect::<Vec<_>>()
            .join("|")
    ))
    .expect("filler fallback");
    let mut search = 0;
    while let Some(found) = pattern.find_at(text, search) {
        let valid = |m: regex::Match<'_>| {
            let end = m.end() - usize::from(m.as_str().ends_with([',', '.']));
            boundary(text, m.start(), end)
        };
        let candidate = if valid(found) {
            Some(found)
        } else {
            fallback
                .find_at(text, found.start())
                .filter(|m| m.start() == found.start() && valid(*m))
        };
        let Some(m) = candidate else {
            search = found.start()
                + text[found.start()..]
                    .chars()
                    .next()
                    .map_or(1, char::len_utf8);
            continue;
        };
        output.push_str(&text[cursor..m.start()]);
        if space(m.as_str()).to_lowercase() == cue_key && !cue_key.is_empty() {
            output.push_str(m.as_str());
        }
        cursor = m.end();
        search = m.end();
    }
    output.push_str(&text[cursor..]);
    let spaces = Regex::new(r"[^\S\r\n]{2,}").expect("space pattern");
    let output = spaces.replace_all(&output, " ");
    let lines = Regex::new(r"[^\S\r\n]*([\r\n])[^\S\r\n]*").expect("line pattern");
    lines.replace_all(&output, "$1").trim().to_owned()
}
