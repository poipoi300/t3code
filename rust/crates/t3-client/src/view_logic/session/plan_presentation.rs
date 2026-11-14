//! Incremental ProposedPlanCard text facts, following proposedPlanText.ts and proposedPlan.ts.
use super::text::{proposed_plan_title, strip_displayed_plan_markdown};
use std::rc::Rc;
use t3_contracts::wire::trim_js;

#[derive(Default)]
pub struct PlanTextState {
    seen: usize,
    units: usize,
    newlines: usize,
    title_probe: usize,
    title: Option<Rc<String>>,
    header_stage: u8,
    header_scan: usize,
    display_start: Option<usize>,
    tail_start: usize,
    tail: String,
}
pub struct PlanTextUpdate {
    pub reset: bool,
    pub title: Option<Rc<String>>,
    pub collapsible: bool,
    pub display_start: usize,
}
impl PlanTextState {
    pub fn update(&mut self, source: &str, streaming: bool) -> PlanTextUpdate {
        let reset = source.len() < self.seen
            || source.get(self.tail_start..self.seen) != Some(self.tail.as_str());
        if reset {
            *self = Self::default();
        }
        let appended = &source[self.seen..];
        if self.units <= 900 {
            self.units += appended.encode_utf16().take(901).count();
        }
        if self.newlines <= 19 {
            self.newlines += appended.bytes().filter(|c| *c == b'\n').take(20).count();
        }
        let title = if self.title.is_none() {
            let tail = &source[self.title_probe..];
            let title = proposed_plan_title(tail).map(|title| Rc::new(title.to_owned()));
            if let Some(end) = tail.rfind('\n') {
                self.title =
                    proposed_plan_title(&tail[..end]).map(|title| Rc::new(title.to_owned()));
                self.title_probe += end + 1;
            }
            title
        } else {
            self.title.clone()
        };
        let display_start = self.display_prefix(source, streaming);
        self.tail_start = appended
            .rfind('\n')
            .map_or(self.tail_start, |at| self.seen + at + 1);
        self.tail = source[self.tail_start..].to_owned();
        self.seen = source.len();
        PlanTextUpdate {
            reset,
            title,
            collapsible: self.units > 900 || self.newlines >= 20,
            display_start,
        }
    }
    fn display_prefix(&mut self, source: &str, streaming: bool) -> usize {
        if let Some(start) = self.display_start {
            return start;
        }
        loop {
            let remaining = &source[self.header_scan..];
            let newline = remaining.find('\n');
            let line = &remaining[..newline.unwrap_or(remaining.len())];
            let blank = trim_js(line).is_empty();
            let heading = proposed_plan_title(line);
            let skip = match self.header_stage {
                0 => blank || (heading.is_some() && strip_displayed_plan_markdown(line).is_empty()),
                1 => {
                    blank || heading.is_some_and(|heading| heading.eq_ignore_ascii_case("summary"))
                }
                _ => blank,
            };
            if !skip {
                if newline.is_some() || !streaming {
                    self.display_start = Some(self.header_scan);
                }
                return self.header_scan;
            }
            let Some(end) = newline else {
                return source.len();
            };
            if self.header_stage == 0 {
                self.header_stage = 1;
            } else if self.header_stage == 1 && !blank {
                self.header_stage = 2;
            }
            self.header_scan += end + 1;
        }
    }
}

#[derive(Clone, Copy, Default)]
pub struct PreviewState {
    visible: usize,
    pending_newlines: usize,
}
pub fn preview_chunk(raw: &str, mut state: PreviewState) -> (String, PreviewState) {
    if state.visible > 10 {
        return (String::new(), state);
    }
    let mut preview = String::new();
    for raw_line in raw.split_inclusive('\n') {
        let newline = raw_line.ends_with('\n');
        let line =
            raw_line.trim_end_matches(|c: char| trim_js(c.encode_utf8(&mut [0; 4])).is_empty());
        if trim_js(line).is_empty() {
            if newline && state.visible > 0 {
                state.pending_newlines += 1;
            }
            continue;
        }
        state.visible += 1;
        if state.visible > 10 {
            preview.push_str("\n\n...");
            state.pending_newlines = 0;
            break;
        }
        preview.extend(std::iter::repeat_n('\n', state.pending_newlines));
        preview.push_str(line);
        state.pending_newlines = usize::from(newline);
    }
    (preview, state)
}

#[derive(Clone, Copy)]
pub struct PlanCodeFence {
    marker: u8,
    length: usize,
    indent: usize,
}

fn fence_run(line: &str) -> Option<(PlanCodeFence, &str)> {
    let body = line.trim_start_matches(' ');
    if line.len() - body.len() > 3 {
        return None;
    }
    let marker = *body.as_bytes().first()?;
    if marker != b'`' && marker != b'~' {
        return None;
    }
    let length = body.bytes().take_while(|byte| *byte == marker).count();
    (length >= 3).then(|| {
        (
            PlanCodeFence {
                marker,
                length,
                indent: line.len() - body.len(),
            },
            &body[length..],
        )
    })
}

pub fn plan_code_fence(raw: &str) -> Option<PlanCodeFence> {
    fence_run(raw.lines().find(|line| !trim_js(line).is_empty())?).map(|(fence, _)| fence)
}

pub fn preview_code_text(
    preview: &str,
    first: bool,
    last: bool,
    fence: Option<PlanCodeFence>,
) -> String {
    let mut code = preview;
    let indent = if let Some(fence) = fence {
        if first {
            code = code
                .trim_start_matches(['\r', '\n'])
                .split_once('\n')
                .map_or("", |(_, body)| body);
        }
        if last {
            let trimmed = code.trim_end_matches(['\r', '\n']);
            let (body, closing) = trimmed.rsplit_once('\n').unwrap_or(("", trimmed));
            if fence_run(closing).is_some_and(|(closing, suffix)| {
                closing.marker == fence.marker
                    && closing.length >= fence.length
                    && trim_js(suffix).is_empty()
            }) {
                code = body;
            }
        }
        fence.indent
    } else {
        4
    };
    code.split_inclusive('\n')
        .map(|line| {
            let mut columns = 0;
            let mut offset = 0;
            for byte in line.bytes() {
                if columns >= indent {
                    break;
                }
                match byte {
                    b' ' => columns += 1,
                    b'\t' => columns += 4 - columns % 4,
                    _ => break,
                }
                offset += 1;
            }
            let mut text = " ".repeat(columns.saturating_sub(indent));
            text.push_str(&line[offset..]);
            text
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view_logic::session::text::collapsed_proposed_plan_preview;

    #[test]
    fn incremental_header_skips_match_the_client_across_every_append_boundary() {
        for source in [
            "# Title\n\n## Summary\n\nBody\n\nLater\n",
            "\n\n# A body heading\n\nText",
            "# Title\n\n\n\n\n\n\n\n## Summary\n\nBody",
            "Plain first line\n\n## Summary\nBody",
            "# Title\r\n\r\n## Summary\r\n\r\n😀 body",
        ] {
            let mut render = PlanTextState::default();
            for end in source
                .char_indices()
                .map(|(at, _)| at)
                .chain([source.len()])
            {
                let source = &source[..end];
                let at = render.display_prefix(source, true);
                assert_eq!(
                    source[at..].replace("\r\n", "\n").trim_end(),
                    strip_displayed_plan_markdown(source).as_str(),
                    "prefix {source:?}"
                );
            }
        }
    }

    #[test]
    fn block_previews_match_client_nonempty_line_limits_and_keep_one_ellipsis() {
        for body in [
            "First\n\nSecond",
            "a\n\nb\n\nc\n\nd\n\ne\n\nf\n\ng\n\nh\n\ni\n\nj\n\nk\n\nl\n",
            "```rust\nlet x = 1;\n```\n\nNext",
            "a  \r\n\r\nb  \r\n",
        ] {
            let mut state = PreviewState::default();
            let preview = body
                .split_inclusive("\n\n")
                .map(|block| {
                    let (preview, after) = preview_chunk(block, state);
                    state = after;
                    preview
                })
                .collect::<String>();
            assert_eq!(
                preview.trim_end(),
                collapsed_proposed_plan_preview(body, 10)
            );
        }
    }

    #[test]
    fn streamed_metadata_matches_plan_helpers_and_resets_on_replacement() {
        let source = format!("# 😀 Settings\n\n## Summary\n\n{}", "body\n".repeat(25));
        let mut state = PlanTextState::default();
        for end in source
            .char_indices()
            .map(|(at, _)| at)
            .chain([source.len()])
        {
            let source = &source[..end];
            let update = state.update(source, true);
            assert!(!update.reset);
            assert_eq!(
                update.title.as_deref().map(String::as_str),
                proposed_plan_title(source)
            );
            assert_eq!(
                update.collapsible,
                source.encode_utf16().count() > 900 || source.split('\n').count() > 20
            );
            assert_eq!(
                source[update.display_start..]
                    .replace("\r\n", "\n")
                    .trim_end(),
                strip_displayed_plan_markdown(source)
            );
        }
        let update = state.update("# Replacement\n\nText", false);
        assert!(update.reset);
        assert_eq!(
            update.title.as_deref().map(String::as_str),
            Some("Replacement")
        );
        assert!(!update.collapsible);
        assert_eq!(update.display_start, "# Replacement\n\n".len());
        for source in ["😀".repeat(450), "😀".repeat(451)] {
            assert_eq!(
                PlanTextState::default().update(&source, false).collapsible,
                source.encode_utf16().count() > 900
            );
        }
    }

    #[test]
    fn code_previews_keep_continuations_as_code_and_remove_only_fence_lines() {
        assert_eq!(
            preview_code_text(
                "  ```rust\n  let x = 1;\n  ```",
                true,
                true,
                plan_code_fence("  ```rust")
            ),
            "let x = 1;"
        );
        assert_eq!(
            preview_code_text(" \tlet x = 1;", true, true, None),
            "let x = 1;"
        );
        let fence = plan_code_fence("```rust");
        assert_eq!(
            preview_code_text("```rust\nlet a = 1;\n", true, false, fence),
            "let a = 1;\n"
        );
        assert_eq!(
            preview_code_text("\n\n```rust\nlet a = 1;\n", true, false, fence),
            "let a = 1;\n"
        );
        assert_eq!(
            preview_code_text("let b = 2;\n\n...", false, false, fence),
            "let b = 2;\n\n..."
        );
        assert_eq!(
            preview_code_text("let c = 3;\n```\n", false, true, fence),
            "let c = 3;"
        );
        assert_eq!(
            preview_code_text(
                "~~~rust\nlet d = 4;\n~~~\n",
                true,
                true,
                plan_code_fence("~~~rust")
            ),
            "let d = 4;"
        );
        assert_eq!(preview_code_text("```rust", true, false, fence), "");
        assert_eq!(
            preview_code_text(
                "let c = 3;\n````",
                false,
                true,
                plan_code_fence("`````rust")
            ),
            "let c = 3;\n````"
        );
        assert_eq!(
            preview_code_text(
                "    let a = 1;\n    ```",
                true,
                true,
                plan_code_fence("    let a = 1;")
            ),
            "let a = 1;\n```"
        );
    }
}
