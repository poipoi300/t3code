//! The prompt recalled by chat/composerPromptHistory.ts (without dangling context chips).
use t3_contracts::wire::trim_js;
fn trim_end(value: &str) -> &str {
    value.trim_end_matches(crate::view_logic::is_js_whitespace)
}
fn strip_review(prompt: &str) -> &str {
    let mut ranges = vec![];
    let mut cursor = 0;
    while let Some(start) = prompt[cursor..].find("<review_comment").map(|i| i + cursor) {
        let suffix = &prompt[start + 15..];
        if suffix
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            cursor = start + 15;
            continue;
        }
        let Some(close) = suffix.find('>') else {
            break;
        };
        let content = start + 15 + close + 1;
        let Some(end) = prompt[content..]
            .find("</review_comment>")
            .map(|i| i + content + 17)
        else {
            break;
        };
        ranges.push((start, end));
        cursor = end;
    }
    let mut cut = prompt.len();
    for (start, end) in ranges.into_iter().rev() {
        if !trim_js(&prompt[end..cut]).is_empty() {
            break;
        }
        cut = start;
    }
    trim_end(&prompt[..cut])
}
fn remove_label(prompt: &mut String, header: &str) {
    let Some((name, lines)) = header
        .rsplit_once(" lines ")
        .or_else(|| header.rsplit_once(" line "))
    else {
        return;
    };
    if name.is_empty()
        || lines.is_empty()
        || !lines
            .split('-')
            .all(|p| !p.is_empty() && p.bytes().all(|c| c.is_ascii_digit()))
        || lines.matches('-').count() > 1
    {
        return;
    }
    let label = format!(
        "@{}:{lines}",
        trim_js(name)
            .to_lowercase()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join("-")
    );
    let found = prompt
        .match_indices(&label)
        .find(|(start, _)| {
            let before = prompt[..*start].chars().next_back();
            let after = prompt[*start + label.len()..].chars().next();
            !before.is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '@' | '.' | '-'))
                && !after.is_some_and(|c| c.is_ascii_digit() || c == '-')
        })
        .map(|(i, _)| i);
    if let Some(start) = found {
        let mut end = start + label.len();
        let mut start = start;
        if prompt.as_bytes().get(end) == Some(&b' ') {
            end += 1;
        } else if start > 0 && prompt.as_bytes()[start - 1] == b' ' {
            start -= 1;
        }
        prompt.replace_range(start..end, "");
    }
}
pub fn recallable_prompt(text: &str) -> String {
    let trimmed = trim_js(text);
    let mut prompt = trimmed
        .strip_prefix("Ultrathink:\n")
        .unwrap_or(trimmed)
        .to_string();
    loop {
        let reviewed = strip_review(&prompt);
        if reviewed.len() != prompt.len() {
            prompt = reviewed.into();
            continue;
        }
        let mut legacy = None;
        for tag in ["terminal_context", "element_context", "preview_annotation"] {
            let closing = format!("\n</{tag}>");
            let clean = trim_end(&prompt);
            if !clean.ends_with(&closing) {
                continue;
            }
            let opening = format!("<{tag}>\n");
            let Some(start) = clean.find(&opening) else {
                continue;
            };
            let body = &clean[start + opening.len()..clean.len() - closing.len()];
            let headers: Vec<String> = body
                .lines()
                .filter_map(|l| l.strip_prefix("- ")?.strip_suffix(':').map(str::to_owned))
                .collect();
            if tag == "preview_annotation"
                && body.lines().any(|l| trim_js(l) == "<preview_annotation>")
                && !body.lines().any(|l| {
                    ["Id: ", "Page: ", "Comment: ", "Targets: "]
                        .iter()
                        .any(|prefix| l.starts_with(prefix))
                })
            {
                continue;
            }
            if tag != "preview_annotation" && headers.is_empty() {
                continue;
            }
            legacy = Some((start, tag, headers));
            break;
        }
        let Some((start, tag, headers)) = legacy else {
            break;
        };
        prompt = trim_end(&prompt[..start]).into();
        if tag == "terminal_context" {
            for header in headers {
                remove_label(&mut prompt, &header);
            }
        }
    }
    let ranges: Vec<_> = crate::view_logic::context_references::collect_references(&prompt)
        .into_iter()
        .map(|r| (r.start, r.end))
        .collect();
    for (mut start, mut end) in ranges.into_iter().rev() {
        if prompt.as_bytes().get(end) == Some(&b' ') {
            end += 1;
        } else if start > 0 && prompt.as_bytes()[start - 1] == b' ' {
            start -= 1;
        }
        prompt.replace_range(start..end, "");
    }
    let trimmed = trim_js(&prompt);
    if trimmed == crate::thread_views::user_message::ATTACHMENT_ONLY_BOOTSTRAP_PROMPT
        || trimmed.starts_with(crate::view_logic::session::text::PLAN_IMPLEMENTATION_PROMPT_PREFIX)
    {
        String::new()
    } else {
        trimmed.into()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_annotation_literal_and_nested_cases() {
        for comment in [
            "Render <preview_annotation>",
            "Render\n<preview_annotation>\nhere",
            "Render &lt;preview_annotation&gt;",
        ] {
            assert_eq!(
                recallable_prompt(&format!(
                    "Prompt\n<preview_annotation>\nComment: {comment}\n</preview_annotation>"
                )),
                "Prompt"
            );
        }
        let text = "Prompt\n<preview_annotation>\nouter literal\n<preview_annotation>\ninner\n</preview_annotation>";
        assert_eq!(recallable_prompt(text), text);
    }
    #[test]
    fn source_legacy_context_and_exact_inline_terminal_labels() {
        for (typed, expected) in [
            ("Look at @terminal-1:4 please", "Look at please"),
            (
                "@terminal-1:4 typed twice: @terminal-1:4\n    indented  code",
                "typed twice: @terminal-1:4\n    indented  code",
            ),
            (
                "see @terminal-1:40 and @terminal-1:4-12 then @terminal-1:4",
                "see @terminal-1:40 and @terminal-1:4-12 then",
            ),
            ("email@terminal-1:4", "email@terminal-1:4"),
        ] {
            assert_eq!(
                recallable_prompt(&format!(
                    "{typed}\n\n<terminal_context>\n- Terminal 1 line 4:\n  4 | ls\n</terminal_context>"
                )),
                expected
            );
        }
        assert_eq!(
            recallable_prompt(
                "Ultrathink:\nInvestigate this\n\n<terminal_context>\n- Terminal 1 lines 12-13:\n  output\n</terminal_context>\n\n<element_context>\n- <button>:\n  url: https://example.com\n</element_context>"
            ),
            "Investigate this"
        );
    }
    #[test]
    fn source_review_suffix_and_dangling_context_references() {
        let block =
            "<review_comment filePath=\"src/app.ts\">Keep this configurable.</review_comment>";
        let middle = format!("Before\n\n{block}\n\nAfter");
        assert_eq!(recallable_prompt(&middle), middle);
        assert_eq!(recallable_prompt(&format!("{middle}\n\n{block}")), middle);
        assert_eq!(
            recallable_prompt(&format!("Please update this.\n\n{block}")),
            "Please update this."
        );
        assert_eq!(
            recallable_prompt(
                "Look at [Terminal](t3-context://v1/terminal/term-1) please\n    indented  code\n![image](t3-context://v1/image/img-1)"
            ),
            "Look at please\n    indented  code"
        );
        assert_eq!(
            recallable_prompt("![image](t3-context://v1/image/img-1)"),
            ""
        );
    }
}
