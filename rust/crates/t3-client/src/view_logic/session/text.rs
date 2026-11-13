use crate::view_logic::{JsTrim, is_js_whitespace};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TextDirection {
    Ltr,
    Rtl,
}
fn rtl(ch: char) -> bool {
    matches!(ch, '\u{0590}'..='\u{08ff}' | '\u{fb1d}'..='\u{fdff}' | '\u{fe70}'..='\u{feff}' | '\u{10800}'..='\u{10fff}' | '\u{1e800}'..='\u{1efff}')
}
fn letter(ch: char) -> bool {
    use unicode_general_category::{GeneralCategory::*, get_general_category};
    matches!(
        get_general_category(ch),
        UppercaseLetter | LowercaseLetter | TitlecaseLetter | ModifierLetter | OtherLetter
    )
}
fn majority(
    units: impl IntoIterator<Item = Option<TextDirection>>,
) -> (Option<TextDirection>, bool) {
    let (mut balance, mut first) = (0_i64, None);
    for direction in units.into_iter().flatten() {
        first.get_or_insert(direction);
        balance += if direction == TextDirection::Rtl {
            1
        } else {
            -1
        };
    }
    (
        if balance == 0 {
            first
        } else {
            Some(if balance > 0 {
                TextDirection::Rtl
            } else {
                TextDirection::Ltr
            })
        },
        balance == 0,
    )
}
fn words(text: &str) -> Option<TextDirection> {
    majority(text.split(is_js_whitespace).map(|word| {
        word.chars().find(|c| letter(*c)).map(|c| {
            if rtl(c) {
                TextDirection::Rtl
            } else {
                TextDirection::Ltr
            }
        })
    }))
    .0
}
pub fn text_direction(text: &str) -> Option<TextDirection> {
    if !text.chars().any(rtl) {
        return text.chars().any(letter).then_some(TextDirection::Ltr);
    }
    let mut sentences = vec![];
    let mut start = 0;
    for (index, ch) in text.char_indices() {
        if ch == '\n'
            || (is_js_whitespace(ch)
                && text[..index]
                    .chars()
                    .next_back()
                    .is_some_and(|c| matches!(c, '.' | '!' | '?' | '؟'))
                && !["e.g.", "i.e."].iter().any(|a| {
                    text[..index]
                        .get(index.saturating_sub(a.len())..)
                        .is_some_and(|s| s.eq_ignore_ascii_case(a))
                }))
        {
            sentences.push(words(&text[start..index]));
            start = index + ch.len_utf8();
        }
    }
    sentences.push(words(&text[start..]));
    let (direction, tie) = majority(sentences);
    if tie { words(text) } else { direction }
}
pub fn markdown_prose_direction(markdown: &str) -> Option<TextDirection> {
    let mut prose = String::new();
    let mut index = 0;
    while index < markdown.len() {
        let tail = &markdown[index..];
        let marker = tail.as_bytes()[0];
        if matches!(marker, b'`' | b'~') {
            let count = tail.bytes().take_while(|c| *c == marker).count();
            if marker == b'`' || count >= 3 {
                let fence = &tail[..count];
                if let Some(end) = tail[count..]
                    .find(fence)
                    .filter(|end| count >= 3 || !tail[count..count + end].contains('\n'))
                {
                    index += count + end + count;
                    prose.push(' ');
                    continue;
                }
                if count >= 3 {
                    break;
                }
            }
        }
        if tail.starts_with("http://") || tail.starts_with("https://") {
            index += tail.find(is_js_whitespace).unwrap_or(tail.len());
            prose.push(' ');
            continue;
        }
        if let Some(ch) = tail.chars().next() {
            prose.push(ch);
            index += ch.len_utf8();
        }
    }
    text_direction(&prose)
}
fn heading(line: &str) -> Option<&str> {
    let indent = line.chars().take_while(|c| is_js_whitespace(*c)).count();
    if indent > 3 {
        return None;
    }
    let line = line.js_trim_start();
    let hashes = line.bytes().take_while(|c| *c == b'#').count();
    (1..=6).contains(&hashes).then_some(())?;
    let title = line.get(hashes..)?;
    title
        .starts_with(is_js_whitespace)
        .then(|| title.js_trim())
        .filter(|s| !s.is_empty())
}
pub fn proposed_plan_title(markdown: &str) -> Option<&str> {
    markdown.lines().find_map(heading)
}
pub fn strip_displayed_plan_markdown(markdown: &str) -> String {
    let lines: Vec<_> = markdown
        .js_trim_end()
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect();
    let mut start = usize::from(lines.first().is_some_and(|l| heading(l).is_some()));
    while lines.get(start).is_some_and(|l| l.js_trim().is_empty()) {
        start += 1;
    }
    if lines
        .get(start)
        .and_then(|l| heading(l))
        .is_some_and(|s| s.eq_ignore_ascii_case("summary"))
    {
        start += 1;
        while lines.get(start).is_some_and(|l| l.js_trim().is_empty()) {
            start += 1;
        }
    }
    lines[start.min(lines.len())..].join("\n")
}
pub fn collapsed_proposed_plan_preview(markdown: &str, max_lines: usize) -> String {
    let stripped = strip_displayed_plan_markdown(markdown);
    let mut preview = vec![];
    let mut visible = 0;
    let mut more = false;
    for line in stripped.js_trim_end().split('\n') {
        let line = line.js_trim_end();
        let nonempty = !line.js_trim().is_empty();
        if nonempty && visible >= max_lines {
            more = true;
            break;
        }
        preview.push(line);
        visible += usize::from(nonempty);
    }
    while preview.last().is_some_and(|s| s.js_trim().is_empty()) {
        preview.pop();
    }
    if preview.is_empty() {
        return proposed_plan_title(markdown)
            .unwrap_or("Plan preview unavailable.")
            .into();
    }
    if more {
        preview.extend(["", "..."]);
    }
    preview.join("\n")
}
pub fn proposed_plan_markdown_filename(markdown: &str) -> String {
    let mut slug = String::new();
    let mut separator = false;
    for ch in proposed_plan_title(markdown)
        .unwrap_or("plan")
        .to_lowercase()
        .chars()
    {
        if "`'\".,!?()[]{}".contains(ch) {
            continue;
        }
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            if separator && !slug.is_empty() {
                slug.push('-');
            }
            separator = false;
            slug.push(ch);
        } else {
            separator = true;
        }
    }
    format!("{}.md", if slug.is_empty() { "plan" } else { &slug })
}
pub fn normalize_plan_markdown_for_export(markdown: &str) -> String {
    format!("{}\n", markdown.js_trim_end())
}
pub const PLAN_IMPLEMENTATION_PROMPT_PREFIX: &str = "PLEASE IMPLEMENT THIS PLAN:\n";

pub fn thread_notification_tag(environment: &str, thread: &str) -> String {
    let hash = format!("{environment}:{thread}")
        .encode_utf16()
        .fold(0xcbf29ce484222325_u64, |hash, unit| {
            (hash ^ u64::from(unit)).wrapping_mul(0x100000001b3)
        });
    format!("{hash:016x}")
}
pub fn has_notification_sound(mode: &str) -> bool {
    matches!(mode, "sound" | "notifications-and-sound")
}
pub fn has_desktop_notifications(mode: &str) -> bool {
    matches!(mode, "notifications" | "notifications-and-sound")
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum NotificationSound {
    Completion,
    Input,
}
#[derive(Default)]
pub struct NotificationSoundSlots(std::collections::BTreeMap<NotificationSound, f64>);
impl NotificationSoundSlots {
    pub fn start(
        &mut self,
        sound: NotificationSound,
        now: f64,
        duration: f64,
        running: bool,
        should_play: bool,
    ) -> bool {
        if !running || !should_play || self.0.get(&sound).is_some_and(|until| now < *until) {
            return false;
        }
        self.0.insert(sound, now + duration);
        true
    }
}
