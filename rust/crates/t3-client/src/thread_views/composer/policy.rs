use super::super::document::{self, Selection, utf16_byte_offset, utf16_len};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerKind {
    Path,
    PullRequest,
    SlashCommand,
    Skill,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trigger {
    pub kind: TriggerKind,
    pub query: String,
    pub range: Selection,
}
pub fn composer_menu_search_key(kind: TriggerKind, query: &str) -> (TriggerKind, String) {
    (
        kind,
        crate::view_logic::JsTrim::js_trim(query).to_lowercase(),
    )
}

pub fn currency_amount(query: &str) -> bool {
    let query = query.strip_prefix('.').unwrap_or(query);
    if !query.as_bytes().first().is_some_and(u8::is_ascii_digit) {
        return false;
    }
    let end = query
        .bytes()
        .take_while(|b| b.is_ascii_digit() || b",._".contains(b))
        .count();
    let mut rest = &query[end..];
    if rest.starts_with(['k', 'K', 'm', 'M', 'b', 'B', 't', 'T']) {
        rest = &rest[1..];
    } else if rest.starts_with(['e', 'E']) {
        let digits = rest[1..].bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return false;
        }
        rest = &rest[1 + digits..];
    }
    rest.bytes().all(|b| b".,;:!? )".contains(&b) && b != b' ')
}
fn space(c: char) -> bool {
    matches!(c, ' ' | '\n' | '\t' | '\r')
}
pub fn detect_trigger(text: &str, cursor: usize) -> Option<Trigger> {
    let cursor = cursor.min(utf16_len(text));
    let byte = utf16_byte_offset(text, cursor)?;
    let start = text[..byte].rfind(space).map_or(0, |i| i + 1);
    let token = &text[start..byte];
    let (kind, query) = if let Some(query) = token.strip_prefix('/') {
        let end = text[byte..].find(space).map_or(text.len(), |i| i + byte);
        if text[start + 1..end].contains(['/', '\\']) {
            return None;
        }
        (TriggerKind::SlashCommand, query)
    } else if let Some(query) = token.strip_prefix('#') {
        let mut chars = query.chars();
        if let Some(first) = chars.next()
            && (!first.is_alphanumeric()
                || !chars.all(|c| c.is_alphanumeric() || matches!(c, '_' | '-')))
        {
            return None;
        }
        (TriggerKind::PullRequest, query)
    } else if let Some(first) = token.chars().next().filter(|c| document::currency(*c)) {
        let query = &token[first.len_utf8()..];
        if currency_amount(query) {
            return None;
        }
        (TriggerKind::Skill, query)
    } else {
        (TriggerKind::Path, token.strip_prefix('@')?)
    };
    Some(Trigger {
        kind,
        query: query.into(),
        range: Selection {
            anchor: utf16_len(&text[..start]),
            head: cursor,
        },
    })
}
pub fn path_suggestion_current(
    search_environment: &str,
    search_cwd: &str,
    search_query: &str,
    trigger: Option<&Trigger>,
    environment: Option<&str>,
    cwd: Option<&str>,
) -> bool {
    trigger.is_some_and(|t| t.kind == TriggerKind::Path && t.query.trim() == search_query)
        && environment == Some(search_environment)
        && cwd == Some(search_cwd)
}
pub fn pasted_path_query_length(before: &str, pasted: &str, active: bool) -> usize {
    if !active
        || !detect_trigger(before, utf16_len(before)).is_some_and(|t| t.kind == TriggerKind::Path)
    {
        return 0;
    }
    pasted
        .chars()
        .take_while(|c| !c.is_whitespace() && *c != '\u{feff}')
        .map(char::len_utf16)
        .sum()
}
pub fn extend_replacement_for_space(text: &str, end: usize, replacement: &str) -> usize {
    end + usize::from(
        replacement.ends_with(' ')
            && utf16_byte_offset(text, end).is_some_and(|b| text[b..].starts_with(' ')),
    )
}
pub fn state_at_prompt_end(text: &str, literal: bool) -> (usize, Option<Trigger>) {
    (
        document::collapse_cursor(text, utf16_len(text), literal),
        if literal {
            None
        } else {
            detect_trigger(text, utf16_len(text))
        },
    )
}
pub fn clear_command(text: &str) -> bool {
    text.trim().eq_ignore_ascii_case("/clear")
}
pub fn side_chat_command(text: &str) -> Option<&str> {
    let trimmed = text.trim();
    let end = trimmed.find(char::is_whitespace).unwrap_or(trimmed.len());
    ["/side", "/btw"]
        .iter()
        .any(|cmd| trimmed[..end].eq_ignore_ascii_case(cmd))
        .then(|| trimmed[end..].trim())
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendShortcut {
    Enter,
    ModEnter,
    ModEnterMultiline,
}
pub fn requires_modifier(shortcut: SendShortcut, prompt: &str) -> bool {
    shortcut == SendShortcut::ModEnter
        || (shortcut == SendShortcut::ModEnterMultiline && prompt.contains(['\r', '\n']))
}
#[derive(Debug, Clone, Default)]
pub struct KeyEvent<'a> {
    pub key: &'a str,
    pub meta: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub composing: bool,
    pub key_code: u32,
    pub repeat: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmissionIntent {
    Foreground,
    Background,
    Alternate,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedCommand {
    SendAlternate,
    SendBackground,
    SendAndNewThread,
    Other,
}
#[derive(Debug, Clone, Copy, Default)]
pub struct SubmissionContext {
    pub software_keyboard: bool,
    pub draft_thread: bool,
    pub running: bool,
}
/// Shortcut resolution belongs to the shared keybinding service; this applies composer policy to its result.
pub fn submission_intent(
    event: &KeyEvent<'_>,
    command: Option<ResolvedCommand>,
    context: SubmissionContext,
    shortcut: SendShortcut,
    prompt: &str,
) -> Option<SubmissionIntent> {
    if event.composing
        || event.key_code == 229
        || event.repeat
        || (context.software_keyboard && event.key == "Enter" && !event.meta && !event.ctrl)
    {
        return None;
    }
    match command {
        Some(ResolvedCommand::SendAlternate) if context.running => {
            return Some(SubmissionIntent::Alternate);
        }
        Some(ResolvedCommand::SendBackground) if context.draft_thread => {
            return Some(SubmissionIntent::Background);
        }
        Some(ResolvedCommand::SendAndNewThread) if !context.draft_thread => {
            return Some(SubmissionIntent::Background);
        }
        Some(_) => return None,
        None => {}
    }
    if event.key != "Enter"
        || event.shift
        || event.alt
        || (requires_modifier(shortcut, prompt) && !event.meta && !event.ctrl)
    {
        None
    } else {
        Some(SubmissionIntent::Foreground)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn triggers_paths_money_and_unicode() {
        for (text, expected) in [
            ("hello @src", Some((TriggerKind::Path, "src"))),
            ("/#", Some((TriggerKind::SlashCommand, "#"))),
            ("/home/file", None),
            ("#é12_-", Some((TriggerKind::PullRequest, "é12_-"))),
            ("#-x", None),
            ("€9.99", None),
            ("$10k.", None),
            ("$1e3)", None),
            ("$rust", Some((TriggerKind::Skill, "rust"))),
        ] {
            let actual = detect_trigger(text, utf16_len(text));
            assert_eq!(
                actual.as_ref().map(|t| (t.kind, t.query.as_str())),
                expected,
                "{text}"
            );
        }
        assert!(detect_trigger("/dir/file", 4).is_none());
        assert_eq!(
            detect_trigger("😀 @x", 5).map(|t| t.range),
            Some(Selection { anchor: 3, head: 5 })
        );
    }
    #[test]
    fn keyboard_policy() {
        let mut event = KeyEvent {
            key: "Enter",
            ..Default::default()
        };
        for (shortcut, prompt, expected) in [
            (SendShortcut::Enter, "a\nb", true),
            (SendShortcut::ModEnter, "a", false),
            (SendShortcut::ModEnterMultiline, "a", true),
            (SendShortcut::ModEnterMultiline, "a\nb", false),
        ] {
            assert_eq!(
                submission_intent(&event, None, SubmissionContext::default(), shortcut, prompt)
                    .is_some(),
                expected
            );
        }
        event.ctrl = true;
        assert_eq!(
            submission_intent(
                &event,
                None,
                SubmissionContext::default(),
                SendShortcut::ModEnter,
                ""
            ),
            Some(SubmissionIntent::Foreground)
        );
        for composing in [true, false] {
            event.composing = composing;
            event.key_code = if composing { 0 } else { 229 };
            assert_eq!(
                submission_intent(
                    &event,
                    Some(ResolvedCommand::SendAlternate),
                    SubmissionContext {
                        running: true,
                        ..Default::default()
                    },
                    SendShortcut::Enter,
                    ""
                ),
                None
            );
        }
    }
    #[test]
    fn paste_and_commands() {
        assert_eq!(pasted_path_query_length("@", "path name", true), 4);
        assert_eq!(pasted_path_query_length("hello", "path", true), 0);
        assert!(clear_command(" /CLEAR \n"));
        assert!(!clear_command("/clear x"));
        assert_eq!(side_chat_command("/btw a\nb"), Some("a\nb"));
        assert_eq!(side_chat_command("/sideways"), None);
    }
}
