//! Port of apps/web/src/components/ComposerPromptEditorTiptap.tsx handleKeyDown and chat/composerCommandKey.ts.

use serde_json::{Value, json};

use super::{
    block_shortcuts, direction, editor, markdown_source,
    policy::{self, KeyEvent, SendShortcut, SubmissionContext},
    structure::{self, Rewrite},
};
use crate::thread_views::{
    code::{CodeBlock, CodeResult},
    document::{Fence, Selection},
    lists,
    rich_text::Mark,
};

#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub rich: bool,
    pub literal: bool,
    pub send_shortcut: SendShortcut,
    pub software_keyboard: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            rich: false,
            literal: false,
            send_shortcut: SendShortcut::Enter,
            software_keyboard: false,
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum Action {
    Pass,
    Consume,
    Submit,
    Rewrite(Rewrite),
    Split {
        rewrite: Rewrite,
        preserve_marks: bool,
    },
    Lift(Rewrite),
}

#[derive(Debug, PartialEq)]
pub enum CaretAction {
    Marks(Vec<Mark>),
    Move(usize),
}

/// Port of ComposerPromptEditorTiptap.tsx caret keys and composer-rich-text-doc.ts styled stops.
pub fn caret_key(
    doc: &Value,
    selection: Selection,
    event: &KeyEvent<'_>,
    stored_marks: &[Value],
) -> Option<CaretAction> {
    if !selection.collapsed()
        || event.shift
        || event.alt
        || event.meta
        || event.ctrl
        || event.composing
        || !matches!(
            event.key,
            "ArrowLeft" | "ArrowRight" | "ArrowUp" | "ArrowDown"
        )
    {
        return None;
    }
    let backwards = if direction::caret_tree_direction(doc, selection.head)
        == Some(direction::Direction::Rtl)
    {
        "ArrowRight"
    } else {
        "ArrowLeft"
    };
    let backwards = event.key == backwards || event.key == "ArrowUp";
    let horizontal = matches!(event.key, "ArrowLeft" | "ArrowRight");
    let blocks = editor::textblocks(doc);
    let edge = if backwards {
        blocks.first().map(|(start, _)| *start)
    } else {
        blocks
            .last()
            .map(|(start, node)| start + editor::node_size(node) - 2)
    };
    let (start, node) = block(doc, selection)?;
    let offset = selection.head - start;
    let mut pos = 0;
    let mut before = None;
    let mut after = None;
    for child in editor::children(&node) {
        let end = pos + editor::node_size(child);
        if offset > pos && offset <= end {
            before = Some(child);
        }
        if offset >= pos && offset < end {
            after = Some(child);
        }
        pos = end;
    }
    let marks_before = before.map(editor::marks).unwrap_or_default();
    let marks_after = after.map(editor::marks).unwrap_or_default();
    let current = editor::marks(&json!({"marks":stored_marks}));
    if (horizontal || edge == Some(selection.head))
        && let Some(marks) =
            editor::step_caret_across_styled_edge(&marks_before, &marks_after, &current, backwards)
    {
        return Some(CaretAction::Marks(marks));
    }
    let adjacent = if backwards { before } else { after }?;
    (horizontal && editor::name(adjacent).starts_with("composer-")).then(|| {
        CaretAction::Move(if backwards {
            selection.head - editor::node_size(adjacent)
        } else {
            selection.head + editor::node_size(adjacent)
        })
    })
}

pub fn needs_document(event: &KeyEvent<'_>) -> bool {
    matches!(
        event.key,
        "Enter"
            | "Tab"
            | "Backspace"
            | "Delete"
            | "ArrowLeft"
            | "ArrowRight"
            | "ArrowUp"
            | "ArrowDown"
    ) || suppressed_chord(event)
}

fn suppressed_chord(event: &KeyEvent<'_>) -> bool {
    if !(event.ctrl || event.meta) {
        return false;
    }
    let chord = format!(
        "Mod-{}{}{}",
        if event.alt { "Alt-" } else { "" },
        if event.shift { "Shift-" } else { "" },
        event.key.to_ascii_lowercase(),
    );
    block_shortcuts::suppress_block_chord(&chord)
}

fn block(doc: &Value, selection: Selection) -> Option<(usize, Value)> {
    editor::textblocks(doc).into_iter().find(|(start, node)| {
        selection.from() >= *start && selection.to() <= start + editor::node_size(node) - 2
    })
}

fn replace_block(doc: &mut Value, start: usize, replacement: &[Value]) -> bool {
    fn walk(node: &mut Value, pos: usize, start: usize, replacement: &[Value]) -> bool {
        let mut offset = pos + usize::from(editor::name(node) != "doc");
        let Some(children) = node.get_mut("content").and_then(Value::as_array_mut) else {
            return false;
        };
        for index in 0..children.len() {
            let size = editor::node_size(&children[index]);
            if offset + 1 == start {
                children.splice(index..=index, replacement.iter().cloned());
                return true;
            }
            if walk(&mut children[index], offset, start, replacement) {
                return true;
            }
            offset += size;
        }
        false
    }
    walk(doc, 0, start, replacement)
}

fn code_block(node: &Value) -> CodeBlock {
    CodeBlock {
        opening: Fence {
            fence: node["attrs"]["fence"].as_str().unwrap_or("```").into(),
            language: node["attrs"]["language"]
                .as_str()
                .unwrap_or_default()
                .into(),
        },
        text: editor::children(node)
            .iter()
            .filter_map(|n| n["text"].as_str())
            .collect(),
        close: node["attrs"]["close"].as_str().unwrap_or_default().into(),
    }
}

fn code_node(block: &CodeBlock) -> Value {
    json!({"type":"codeBlock","attrs":{"fence":block.opening.fence,"language":block.opening.language,"close":block.close},
        "content":if block.text.is_empty(){vec![]}else{vec![json!({"type":"text","text":block.text})]}})
}

/// Port of apps/web/src/composer-code-block.ts code key commands.
fn code_key(doc: &Value, selection: Selection, event: &KeyEvent<'_>) -> Option<Rewrite> {
    let (start, node) = block(doc, selection)?;
    if editor::name(&node) != "codeBlock" {
        return None;
    }
    let relative = Selection {
        anchor: selection.anchor - start,
        head: selection.head - start,
    };
    let code = code_block(&node);
    let result = if event.key == "Tab" {
        code.indent(relative, event.shift)?
    } else {
        code.enter(relative)?
    };
    let (nodes, selection) = match result {
        CodeResult::Edited { block, selection } => {
            let selection = if event.key == "Tab" && block.text != code.text {
                Selection {
                    anchor: selection.from(),
                    head: selection.to(),
                }
            } else {
                selection
            };
            (
                vec![code_node(&block)],
                Selection {
                    anchor: start + selection.anchor,
                    head: start + selection.head,
                },
            )
        }
        CodeResult::Exited { block, paragraphs } => {
            let code = code_node(&block);
            let caret = start + editor::node_size(&code);
            let mut nodes = vec![code];
            nodes.extend(paragraphs.into_iter().map(|line| json!({"type":"paragraph","content":if line.is_empty(){vec![]}else{vec![json!({"type":"text","text":line})]}})));
            (nodes, Selection::caret(caret))
        }
    };
    let mut document = doc.clone();
    replace_block(&mut document, start, &nodes).then_some(Rewrite {
        document,
        selection,
    })
}

/// Port of apps/web/src/composer-code-block.ts convertCodeFenceOnEnter.
fn opening_fence(doc: &Value, selection: Selection) -> Option<Rewrite> {
    let (start, node) = block(doc, selection)?;
    if editor::name(&node) != "paragraph" {
        return None;
    }
    let mut pos = 1;
    let top_level = editor::children(doc).iter().any(|child| {
        let matches = pos == start;
        pos += editor::node_size(child);
        matches
    });
    let text = editor::serialize_selection(doc, start, start + editor::node_size(&node) - 2)?;
    let relative = Selection {
        anchor: selection.anchor - start,
        head: selection.head - start,
    };
    let code = CodeBlock::from_opening_paragraph(&text, relative, top_level)?;
    let mut document = doc.clone();
    replace_block(&mut document, start, &[code_node(&code)]).then_some(Rewrite {
        document,
        selection: Selection::caret(start),
    })
}

/// Port of apps/web/src/components/chat/composerCommandKey.ts applyListEdit.
fn list_edit(
    doc: &Value,
    selection: Selection,
    event: &KeyEvent<'_>,
    options: Options,
) -> Option<Rewrite> {
    if options.literal || !selection.collapsed() {
        return None;
    }
    let map = editor::serialize_document(doc);
    let cursor = map.flat_to_markdown(map.pm_to_flat(selection.head));
    let edit = match (event.key, event.shift) {
        ("Enter", _) => lists::continuation_for_enter(&map.value, cursor),
        ("Tab", true) => lists::outdent_for_tab(&map.value, cursor, cursor),
        ("Tab", false) => lists::indent_for_tab(&map.value, cursor, cursor),
        _ => None,
    }?;
    let value = edit.apply(&map.value)?;
    let atoms: Vec<_> = editor::textblocks(doc)
        .into_iter()
        .flat_map(|(_, block)| editor::children(&block).to_vec())
        .collect();
    let document = editor::build_document(
        &value,
        editor::BuildOptions {
            styling: options.rich,
            literal: options.literal,
            ..Default::default()
        },
        |name| {
            atoms
                .iter()
                .find(|node| node["type"] == "composer-skill" && node["attrs"]["skillName"] == name)
                .map(|node| {
                    (
                        node["attrs"]["skillLabel"].as_str().unwrap_or(name).into(),
                        node["attrs"]["skillDescription"]
                            .as_str()
                            .map(str::to_owned),
                    )
                })
        },
        |instance, model| {
            atoms.iter().any(|node| {
                node["type"] == "composer-model"
                    && node["attrs"]["instanceId"] == instance
                    && node["attrs"]["model"] == model
            })
        },
    );
    let after = editor::serialize_document(&document);
    let caret = after.flat_to_pm(after.markdown_to_flat(edit.cursor_after));
    Some(Rewrite {
        document,
        selection: Selection::caret(caret),
    })
}

/// The callback is the host's suggestion/application command handler, between code keys and send.
/// Closing-marker marks follow apps/web/src/composer-markdown-source.ts collapseShownMarkdownSource.
pub fn resolve(
    doc: &Value,
    selection: Selection,
    event: &KeyEvent<'_>,
    options: Options,
    in_list: bool,
    mut command: impl FnMut(bool) -> bool,
) -> Action {
    if event.composing || event.key_code == 229 {
        return if event.key == "Enter" {
            Action::Consume
        } else {
            Action::Pass
        };
    }
    if event.key == "Tab"
        && !event.meta
        && !event.ctrl
        && let Some(rewrite) = code_key(doc, selection, event)
    {
        return Action::Rewrite(rewrite);
    }
    if event.key == "Enter"
        && options.rich
        && !event.meta
        && !event.ctrl
        && let Some(rewrite) =
            code_key(doc, selection, event).or_else(|| opening_fence(doc, selection))
    {
        return Action::Rewrite(rewrite);
    }
    if command(in_list) {
        return Action::Consume;
    }
    if options.rich
        && !event.meta
        && !event.ctrl
        && !event.alt
        && !event.shift
        && matches!(event.key, "Backspace" | "Delete")
    {
        let collapsed = markdown_source::collapse(doc, selection, false);
        let rewrite = if event.key == "Backspace" {
            structure::backspace(&collapsed.document, collapsed.selection)
        } else {
            structure::delete(&collapsed.document, collapsed.selection)
        };
        return rewrite.map_or(Action::Pass, Action::Rewrite);
    }
    if suppressed_chord(event) {
        return Action::Consume;
    }
    if !matches!(event.key, "Enter" | "Tab") {
        return Action::Pass;
    }
    let map = editor::serialize_document(doc);
    if policy::submission_intent(
        event,
        None,
        SubmissionContext {
            software_keyboard: options.software_keyboard,
            ..Default::default()
        },
        options.send_shortcut,
        &map.value,
    )
    .is_some()
    {
        return Action::Submit;
    }
    if event.key == "Tab" {
        return list_edit(doc, selection, event, options).map_or(Action::Pass, Action::Rewrite);
    }
    if !in_list && let Some(rewrite) = list_edit(doc, selection, event, options) {
        return Action::Rewrite(rewrite);
    }
    let collapsed = markdown_source::collapse(doc, selection, false);
    if in_list && let Some(rewrite) = structure::enter(&collapsed.document, collapsed.selection) {
        return Action::Rewrite(rewrite);
    }
    if options.rich
        && selection.collapsed()
        && let Some((start, node)) = block(&collapsed.document, collapsed.selection)
        && editor::node_size(&node) == 2
        && editor::children(&collapsed.document)
            .iter()
            .scan(0, |pos, node| {
                let begins = *pos;
                *pos += editor::node_size(node);
                Some((begins, node))
            })
            .any(|(pos, node)| {
                editor::name(node) == "blockquote"
                    && start > pos
                    && start < pos + editor::node_size(node)
            })
    {
        return Action::Lift(Rewrite {
            document: collapsed.document,
            selection: collapsed.selection,
        });
    }
    Action::Split {
        rewrite: Rewrite {
            document: collapsed.document,
            selection: collapsed.selection,
        },
        preserve_marks: selection.collapsed()
            && markdown_source::source_regions(doc)
                .iter()
                .any(|range| range.to == selection.from()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thread_views::document::utf16_len;

    fn document(value: &str, rich: bool) -> Value {
        editor::build_document(
            value,
            editor::BuildOptions {
                styling: rich,
                ..Default::default()
            },
            |_| None,
            |_, _| false,
        )
    }
    fn caret(doc: &Value, offset: usize) -> Selection {
        let map = editor::serialize_document(doc);
        Selection::caret(map.flat_to_pm(map.markdown_to_flat(offset)))
    }
    fn key(key: &str) -> KeyEvent<'_> {
        KeyEvent {
            key,
            ..Default::default()
        }
    }
    fn rewritten(action: Action) -> Rewrite {
        let Action::Rewrite(rewrite) = action else {
            panic!("expected client rewrite, got {action:?}")
        };
        rewrite
    }

    #[test]
    fn submission_uses_the_existing_client_preference_and_ime_policy() {
        for (shortcut, text, ctrl, expected) in [
            (SendShortcut::Enter, "a\nb", false, true),
            (SendShortcut::ModEnter, "a", false, false),
            (SendShortcut::ModEnter, "a", true, true),
            (SendShortcut::ModEnterMultiline, "a", false, true),
            (SendShortcut::ModEnterMultiline, "a\nb", false, false),
            (SendShortcut::ModEnterMultiline, "a\nb", true, true),
        ] {
            let doc = document(text, false);
            let action = resolve(
                &doc,
                caret(&doc, utf16_len(text)),
                &KeyEvent {
                    ctrl,
                    ..key("Enter")
                },
                Options {
                    send_shortcut: shortcut,
                    ..Default::default()
                },
                false,
                |_| false,
            );
            assert_eq!(action == Action::Submit, expected, "{shortcut:?} {text:?}");
        }
        let doc = document("x", false);
        for event in [
            KeyEvent {
                composing: true,
                ..key("Enter")
            },
            KeyEvent {
                key_code: 229,
                ..key("Enter")
            },
        ] {
            assert_eq!(
                resolve(
                    &doc,
                    caret(&doc, 1),
                    &event,
                    Options::default(),
                    false,
                    |_| panic!("IME must not reach commands")
                ),
                Action::Consume
            );
        }
        for event in [
            KeyEvent {
                repeat: true,
                ..key("Enter")
            },
            KeyEvent {
                shift: true,
                ..key("Enter")
            },
            KeyEvent {
                alt: true,
                ..key("Enter")
            },
        ] {
            assert!(matches!(
                resolve(
                    &doc,
                    caret(&doc, 1),
                    &event,
                    Options::default(),
                    false,
                    |_| false
                ),
                Action::Split { .. }
            ));
        }
    }

    #[test]
    fn code_keys_and_top_level_fences_precede_commands_and_submission() {
        let options = Options {
            rich: true,
            ..Default::default()
        };
        let doc = document("```ts\n  body()\n```", true);
        let at = caret(&doc, utf16_len("```ts\n  body()"));
        let rewrite = rewritten(resolve(&doc, at, &key("Enter"), options, false, |_| {
            panic!("code has priority")
        }));
        assert_eq!(
            editor::serialize_document(&rewrite.document).value,
            "```ts\n  body()\n  \n```"
        );
        let rewrite = rewritten(resolve(&doc, at, &key("Tab"), options, false, |_| {
            panic!("code tab has priority")
        }));
        assert_eq!(
            editor::serialize_document(&rewrite.document).value,
            "```ts\n  body()  \n```"
        );
        for opening in ["```", "```ts", "~~~py", "````"] {
            // Typed fences remain paragraphs until Enter, unlike restored Markdown.
            let doc = json!({"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":opening}]}]});
            let rewrite = rewritten(resolve(
                &doc,
                Selection::caret(1 + utf16_len(opening)),
                &key("Enter"),
                options,
                false,
                |_| panic!("fence has priority"),
            ));
            assert_eq!(rewrite.document["content"][0]["type"], "codeBlock");
            assert_eq!(rewrite.selection, Selection::caret(1));
        }
        let doc = document("```ts\nx\n```", true);
        assert_eq!(
            resolve(
                &doc,
                caret(&doc, 7),
                &KeyEvent {
                    ctrl: true,
                    ..key("Enter")
                },
                options,
                false,
                |_| false
            ),
            Action::Submit
        );
    }

    #[test]
    fn list_continuation_and_tab_use_expanded_utf16_source_offsets() {
        for (value, at, event, expected) in [
            ("- 😀", 4, key("Enter"), "- 😀\n- "),
            ("- [x] done", 10, key("Enter"), "- [x] done\n- [ ] "),
            ("1. foobar", 5, key("Enter"), "1. fo\n2. obar"),
            ("- ", 2, key("Enter"), ""),
            ("1. a\n2. b", 9, key("Tab"), "1. a\n   1. b"),
            (
                "1. a\n   1. b\n2. c",
                12,
                KeyEvent {
                    shift: true,
                    ..key("Tab")
                },
                "1. a\n2. b\n3. c",
            ),
        ] {
            let doc = document(value, false);
            let options = Options {
                send_shortcut: SendShortcut::ModEnter,
                ..Default::default()
            };
            let rewrite = rewritten(resolve(
                &doc,
                caret(&doc, at),
                &event,
                options,
                false,
                |_| false,
            ));
            assert_eq!(
                editor::serialize_document(&rewrite.document).value,
                expected
            );
        }
    }

    #[test]
    fn rich_lists_split_without_losing_marks_or_checked_state_and_empty_items_lift() {
        let options = Options {
            rich: true,
            send_shortcut: SendShortcut::ModEnter,
            ..Default::default()
        };
        for (value, expected) in [
            ("- **word**", "- **word**\n- "),
            ("- [x] done", "- [x] done\n- [ ] "),
            (
                "1. parent\n   1. child\n      1. ",
                "1. parent\n   1. child\n   2. ",
            ),
        ] {
            let doc = document(value, true);
            let rewrite = rewritten(resolve(
                &doc,
                caret(&doc, utf16_len(value)),
                &key("Enter"),
                options,
                true,
                |_| false,
            ));
            assert_eq!(
                editor::serialize_document(&rewrite.document).value,
                expected
            );
        }
    }

    #[test]
    fn commands_keep_precedence_over_send_and_block_shortcuts() {
        let doc = document("> keep this text", true);
        for event in [
            key("Enter"),
            KeyEvent {
                ctrl: true,
                shift: true,
                ..key("8")
            },
            KeyEvent {
                ctrl: true,
                alt: true,
                ..key("1")
            },
        ] {
            assert_eq!(
                resolve(
                    &doc,
                    caret(&doc, 16),
                    &event,
                    Options {
                        rich: true,
                        ..Default::default()
                    },
                    false,
                    |_| true
                ),
                Action::Consume
            );
        }
        let event = KeyEvent {
            ctrl: true,
            shift: true,
            ..key("8")
        };
        assert_eq!(
            resolve(
                &doc,
                caret(&doc, 16),
                &event,
                Options {
                    rich: true,
                    ..Default::default()
                },
                false,
                |_| false
            ),
            Action::Consume
        );
    }

    #[test]
    fn literal_answers_skip_lists_and_software_keyboard_uses_newlines() {
        let doc = document("- text", false);
        let options = Options {
            literal: true,
            software_keyboard: true,
            ..Default::default()
        };
        assert!(matches!(
            resolve(&doc, caret(&doc, 6), &key("Enter"), options, false, |_| {
                false
            }),
            Action::Split { .. }
        ));
        assert_eq!(
            resolve(&doc, caret(&doc, 6), &key("Tab"), options, false, |_| false),
            Action::Pass
        );
        assert_eq!(
            resolve(
                &doc,
                caret(&doc, 6),
                &KeyEvent {
                    ctrl: true,
                    ..key("Enter")
                },
                options,
                false,
                |_| false
            ),
            Action::Submit
        );
    }

    #[test]
    fn boundary_deletion_unwraps_fences_and_joins_lists_without_losing_source() {
        let options = Options {
            rich: true,
            ..Default::default()
        };
        for (value, at, expected) in [
            ("- a\n```\nl1\nl2\n```", 8, "- a\nl1\nl2"),
            ("p\n```ts\nl1\n```", 8, "p\nl1"),
            ("- a\n  - [ ] x\n  - [ ] y", 22, "- a\n  - [ ] xy"),
        ] {
            let doc = document(value, true);
            let rewrite = rewritten(resolve(
                &doc,
                caret(&doc, at),
                &key("Backspace"),
                options,
                false,
                |_| false,
            ));
            assert_eq!(
                editor::serialize_document(&rewrite.document).value,
                expected
            );
        }
    }

    #[test]
    fn styled_caret_stops_precede_atom_steps_and_follow_line_direction() {
        let shown = json!({"type":"doc","content":[{"type":"paragraph","content":[
            {"type":"text","text":"**word**","marks":[{"type":"markdownSource"}]}
        ]}]});
        assert!(matches!(
            resolve(
                &shown,
                Selection::caret(9),
                &KeyEvent {
                    shift: true,
                    ..key("Enter")
                },
                Options {
                    rich: true,
                    ..Default::default()
                },
                false,
                |_| false
            ),
            Action::Split {
                preserve_marks: true,
                ..
            }
        ));
        let doc = json!({"type":"doc","content":[{"type":"paragraph","content":[
            {"type":"text","text":"x","marks":[{"type":"bold"}]},
            {"type":"composer-mention","attrs":{"path":"a.md","source":"@a.md"}},
            {"type":"text","text":" tail"}
        ]}]});
        assert_eq!(
            caret_key(
                &doc,
                Selection::caret(2),
                &key("ArrowRight"),
                &[json!({"type":"bold"})]
            ),
            Some(CaretAction::Marks(vec![]))
        );
        assert_eq!(
            caret_key(&doc, Selection::caret(2), &key("ArrowRight"), &[]),
            Some(CaretAction::Move(3))
        );
        assert_eq!(
            caret_key(&doc, Selection::caret(3), &key("ArrowLeft"), &[]),
            Some(CaretAction::Move(2))
        );
        assert!(
            caret_key(
                &doc,
                Selection::caret(2),
                &KeyEvent {
                    shift: true,
                    ..key("ArrowRight")
                },
                &[]
            )
            .is_none()
        );
        let rtl = json!({"type":"doc","content":[{"type":"paragraph","content":[
            {"type":"text","text":"א"},
            {"type":"composer-mention","attrs":{"path":"a.md","source":"@a.md"}}
        ]}]});
        assert_eq!(
            caret_key(&rtl, Selection::caret(3), &key("ArrowRight"), &[]),
            Some(CaretAction::Move(2))
        );
    }
}
