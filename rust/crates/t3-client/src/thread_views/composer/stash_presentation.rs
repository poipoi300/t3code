//! Port of apps/web/src/components/chat/ComposerStashMenu.tsx's non-visual entry presentation.

use super::citations;
use crate::view_logic::shell::stash::Entry;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Snippet {
    Prompt(String),
    Empty,
    Attachments { count: usize, kind: AttachmentKind },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttachmentKind {
    Image,
    File,
    Attachment,
}

fn js_space(c: char) -> bool {
    matches!(c, '\u{9}'..='\u{d}' | '\u{20}' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}')
}

/// packages/shared/src/assistantCitations.ts assistantCitationsToPlainText.
pub(crate) fn plain_text(prompt: &str) -> String {
    let source: Vec<_> = prompt.encode_utf16().collect();
    let mut result = String::new();
    let mut cursor = 0;
    for found in citations::collect(prompt) {
        result.push_str(&String::from_utf16_lossy(
            &source[cursor..found.range.start],
        ));
        result.push_str(&found.citation.text);
        if let Some(Some(comment)) = &found.citation.comment {
            result.push_str("\nComment: ");
            result.push_str(comment);
        }
        cursor = found.range.end;
    }
    result.push_str(&String::from_utf16_lossy(&source[cursor..]));
    result
}

pub fn stash_entry_snippet(entry: &Entry) -> Snippet {
    let trimmed = plain_text(&entry.prompt)
        .split(js_space)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if !trimmed.is_empty() {
        let units: Vec<_> = trimmed.encode_utf16().collect();
        return Snippet::Prompt(if units.len() > 90 {
            format!("{}…", String::from_utf16_lossy(&units[..90]))
        } else {
            trimmed
        });
    }
    let images = entry.attachments.len() + entry.dropped_image_names.len();
    let files = entry.files.as_ref().map_or(0, Vec::len);
    if images + files == 0 {
        return Snippet::Empty;
    }
    Snippet::Attachments {
        count: images + files,
        kind: if images > 0 && files > 0 {
            AttachmentKind::Attachment
        } else if files > 0 {
            AttachmentKind::File
        } else {
            AttachmentKind::Image
        },
    }
}

pub fn missing_image_count(entry: &Entry) -> usize {
    entry.dropped_image_names.len() + entry.unreadable_image_names.as_ref().map_or(0, Vec::len)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry(prompt: &str) -> Entry {
        serde_json::from_value(json!({"id":"stash", "createdAt":"2026-11-14T00:00:00Z", "prompt":prompt, "attachments":[], "droppedImageNames":[]})).expect("entry")
    }

    #[test]
    fn snippets_follow_js_whitespace_and_ninety_utf16_units() {
        assert_eq!(
            stash_entry_snippet(&entry("\u{feff} hello\n\tworld ")),
            Snippet::Prompt("hello world".into())
        );
        assert_eq!(
            stash_entry_snippet(&entry(&"x".repeat(90))),
            Snippet::Prompt("x".repeat(90))
        );
        assert_eq!(
            stash_entry_snippet(&entry(&format!("{}😀z", "x".repeat(89)))),
            Snippet::Prompt(format!("{}�…", "x".repeat(89)))
        );
        assert_eq!(
            stash_entry_snippet(&entry("\u{85}x")),
            Snippet::Prompt("\u{85}x".into())
        );
    }

    #[test]
    fn citations_and_comments_are_readable_but_invalid_links_remain_literal() {
        let citation = serde_json::from_value(json!({"version":1,"environmentId":"e","threadId":"t","messageId":"m","text":"Quoted 😀", "comment":"My\ncomment", "start":0,"end":9,"prefix":"","suffix":""})).expect("citation");
        assert_eq!(
            stash_entry_snippet(&entry(&format!(
                "before {} after",
                citations::serialize(&citation)
            ))),
            Snippet::Prompt("before Quoted 😀 Comment: My comment after".into())
        );
        let invalid = "[Assistant quote](t3-citation://v1/bad)";
        assert_eq!(
            stash_entry_snippet(&entry(invalid)),
            Snippet::Prompt(invalid.into())
        );
    }

    #[test]
    fn attachment_only_fallback_and_missing_counts_preserve_web_policy() {
        let mut value = entry("");
        assert_eq!(stash_entry_snippet(&value), Snippet::Empty);
        value.unreadable_image_names = Some(vec!["unreadable".into()]);
        value.pending_image_count = Some(2.0);
        assert_eq!(stash_entry_snippet(&value), Snippet::Empty);
        value.dropped_image_names.push("large".into());
        assert_eq!(
            stash_entry_snippet(&value),
            Snippet::Attachments {
                count: 1,
                kind: AttachmentKind::Image
            }
        );
        value.files = Some(vec![serde_json::from_value(json!({"id":"f","name":"f.pdf","mimeType":"application/pdf","sizeBytes":1,"attachmentId":"upload","environmentId":"e"})).expect("file")]);
        assert_eq!(
            stash_entry_snippet(&value),
            Snippet::Attachments {
                count: 2,
                kind: AttachmentKind::Attachment
            }
        );
        assert_eq!(missing_image_count(&value), 2);
        value.dropped_image_names.clear();
        assert_eq!(
            stash_entry_snippet(&value),
            Snippet::Attachments {
                count: 1,
                kind: AttachmentKind::File
            }
        );
    }
}
