//! Port of apps/web/src/components/ComposerPromptEditorTiptap.tsx handleKeyDown and chat/composerCommandKey.ts.

use std::sync::{Arc, Mutex};

use nsh_base::rich_editor::{
    commands::{
        base::{lift, split_block_keep_marks},
        helpers::is_active,
    },
    model::{Fragment, Node},
    state::{EditorState, KeyEvent, Plugin, PluginSpec, StateError, TextSelection, Transaction},
};
use nsh_core::prelude::*;
use serde_json::json;
use t3_client::thread_views::{
    composer::{
        code_pairs::CodePairs,
        keyboard::{self, Action, Options},
        policy::{self, SendShortcut},
        structure::Rewrite,
    },
    document::Selection,
    rich_text::Mark,
};

use super::editor::{ComposerCommandKeyCallback, ComposerEditor, ComposerSubmit};

#[derive(Clone, Copy)]
pub struct ComposerKeyProps {
    pub send_shortcut: Prop<SendShortcut>,
    pub software_keyboard_open: Prop<bool>,
    pub disabled: Prop<bool>,
    pub on_command_key_down: Prop<Option<ComposerCommandKeyCallback>>,
}

impl Default for ComposerKeyProps {
    fn default() -> Self {
        Self {
            send_shortcut: Prop::new(SendShortcut::Enter),
            software_keyboard_open: Prop::new(false),
            disabled: Prop::new(false),
            on_command_key_down: Prop::new(None),
        }
    }
}

/// Install before the document adapter; pass the host's command callback here once.
pub fn composer_key_plugin(editor: &ComposerEditor, props: ComposerKeyProps) -> Plugin {
    Arc::new(ComposerKeys {
        props,
        submit: editor.submitter(),
        pairs: editor.code_pairs(),
    })
}

struct ComposerKeys {
    props: ComposerKeyProps,
    submit: ComposerSubmit,
    pairs: Arc<Mutex<CodePairs>>,
}

impl PluginSpec for ComposerKeys {
    fn append_transaction(
        &self,
        trs: &[Transaction],
        old: &EditorState,
        new: &EditorState,
    ) -> Option<Transaction> {
        if self.props.disabled.get()
            || new.schema().mark_type("code").is_none()
            || trs.iter().any(|tr| tr.doc_changed())
            || old.selection().from() == new.selection().from()
        {
            return None;
        }
        let rewrite = self
            .pairs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .caret_moved(
                &new.doc().to_json(),
                Selection {
                    anchor: old.selection().anchor(),
                    head: old.selection().head(),
                },
                Selection {
                    anchor: new.selection().anchor(),
                    head: new.selection().head(),
                },
                false,
            )?;
        let result = rewrite_transaction(
            new,
            &Rewrite {
                document: rewrite.document,
                selection: rewrite.selection,
            },
        )
        .and_then(|mut tr| {
            let marks = native_marks(new, rewrite.stored_marks)?;
            tr.set_stored_marks(Some(marks));
            Ok(tr)
        });
        Some(match result {
            Ok(tr) => tr,
            Err(error) => {
                let mut tr = new.tr();
                tr.set_meta("composerError", json!(error.to_string()));
                tr
            }
        })
    }

    fn handle_key_down(
        &self,
        state: &EditorState,
        event: &KeyEvent,
        dispatch: &mut dyn FnMut(Transaction),
    ) -> bool {
        let key = policy::KeyEvent {
            key: &event.key,
            meta: event.meta,
            ctrl: event.ctrl,
            alt: event.alt,
            shift: event.shift,
            // NSH defers composing keys before plugins, but exposes neither repeat nor keyCode.
            ..Default::default()
        };
        let command = self.props.on_command_key_down.get();
        let mut run_command = |in_list| {
            command
                .as_ref()
                .is_some_and(|command| command.call(event, in_list))
        };
        if self.props.disabled.get() {
            return false;
        }
        if !keyboard::needs_document(&key) {
            return run_command(false);
        }
        let document = state.doc().to_json();
        let selection = Selection {
            anchor: state.selection().anchor(),
            head: state.selection().head(),
        };
        let marks = state
            .stored_marks()
            .map(<[_]>::to_vec)
            .unwrap_or_else(|| state.selection().resolved_from().marks());
        if let Some(caret) = keyboard::caret_key(
            &document,
            selection,
            &key,
            &marks.iter().map(|mark| mark.to_json()).collect::<Vec<_>>(),
        ) {
            let result = match caret {
                keyboard::CaretAction::Marks(marks) => native_marks(state, marks).map(|marks| {
                    let mut tr = state.tr();
                    tr.set_stored_marks(Some(marks));
                    tr
                }),
                keyboard::CaretAction::Move(position) => {
                    let mut tr = state.tr();
                    tr.set_selection(TextSelection::create(tr.doc(), position, None));
                    Ok(tr)
                }
            };
            match result {
                Ok(tr) => dispatch(tr),
                Err(error) => {
                    let mut tr = state.tr();
                    tr.set_meta("composerError", json!(error.to_string()));
                    dispatch(tr);
                }
            }
            return true;
        }
        let attrs = Default::default();
        let in_list = event.key == "Enter"
            && (is_active(state, Some("listItem"), &attrs)
                || is_active(state, Some("taskItem"), &attrs));
        let action = keyboard::resolve(
            &document,
            selection,
            &key,
            Options {
                rich: state.schema().node_type("codeBlock").is_some(),
                literal: state.schema().node_type("composer-mention").is_none(),
                send_shortcut: self.props.send_shortcut.get(),
                software_keyboard: self.props.software_keyboard_open.get(),
            },
            in_list,
            &mut run_command,
        );
        let result = match action {
            Action::Pass => return false,
            Action::Consume => return true,
            Action::Submit => {
                // A refused/missing send callback must never fall through and edit the draft.
                self.submit.submit(state);
                return true;
            }
            Action::Rewrite(rewrite) => rewrite_transaction(state, &rewrite),
            Action::Split(rewrite) => native_command(state, &rewrite, split_block_keep_marks),
            Action::Lift(rewrite) => native_command(state, &rewrite, lift),
        };
        match result {
            Ok(mut tr) => {
                tr.scroll_into_view();
                dispatch(tr);
            }
            Err(error) => {
                let mut tr = state.tr();
                tr.set_meta("composerError", json!(error.to_string()));
                dispatch(tr);
            }
        }
        true
    }
}

fn native_marks(
    state: &EditorState,
    marks: Vec<Mark>,
) -> Result<Vec<nsh_base::rich_editor::model::Mark>, StateError> {
    marks
        .into_iter()
        .map(|mark| {
            let name = match mark {
                Mark::Bold => "bold",
                Mark::Italic => "italic",
                Mark::Strike => "strike",
                Mark::Code => "code",
            };
            state
                .schema()
                .mark(name, None)
                .map_err(|error| StateError::Invalid(error.to_string()))
        })
        .collect()
}

fn rewrite_transaction(state: &EditorState, rewrite: &Rewrite) -> Result<Transaction, StateError> {
    let node = state
        .schema()
        .node_from_json(&rewrite.document)
        .map_err(|error| StateError::Invalid(error.to_string()))?;
    node.check()
        .map_err(|error| StateError::Invalid(error.to_string()))?;
    native_transaction(state, &node, rewrite.selection)
}

// Native diffs preserve mapping of CMP-EDITOR-1's tracked backtick pairs and transaction history.
fn native_transaction(
    state: &EditorState,
    doc: &Node,
    selection: Selection,
) -> Result<Transaction, StateError> {
    if selection.anchor > doc.content().size() || selection.head > doc.content().size() {
        return Err(StateError::Invalid(
            "Composer selection is outside the document.".into(),
        ));
    }
    let mut tr = state.tr();
    if let Some(start) = state.doc().content().find_diff_start(doc.content(), 0) {
        let (old_end, new_end) = diff_end(
            state.doc().content(),
            doc.content(),
            state.doc().content().size(),
            doc.content().size(),
        );
        let overlap = start.saturating_sub(old_end.min(new_end));
        let slice = doc
            .slice(start, new_end + overlap, false)
            .map_err(|error| StateError::Invalid(error.to_string()))?;
        tr.replace(start, old_end + overlap, slice)
            .map_err(|error| StateError::Invalid(error.to_string()))?;
    }
    tr.set_selection(TextSelection::create(
        tr.doc(),
        selection.anchor,
        Some(selection.head),
    ));
    Ok(tr)
}

fn diff_end(left: &Fragment, right: &Fragment, mut a: usize, mut b: usize) -> (usize, usize) {
    let mut old = left.iter().rev();
    let mut new = right.iter().rev();
    while let (Some(left), Some(right)) = (old.next(), new.next()) {
        if left.eq(right) {
            a -= left.node_size();
            b -= right.node_size();
            continue;
        }
        if !left.same_markup(right) {
            return (a, b);
        }
        if let (Some(left), Some(right)) = (left.text(), right.text()) {
            let shared = left
                .chars()
                .rev()
                .zip(right.chars().rev())
                .take_while(|(a, b)| a == b)
                .map(|(ch, _)| ch.len_utf16())
                .sum::<usize>();
            return (a - shared, b - shared);
        }
        if left.content().size() > 0 || right.content().size() > 0 {
            return diff_end(left.content(), right.content(), a - 1, b - 1);
        }
        return (a, b);
    }
    (a, b)
}

type NativeCommand = for<'a> fn(&EditorState, Option<&'a mut dyn FnMut(Transaction)>) -> bool;

fn native_command(
    state: &EditorState,
    rewrite: &Rewrite,
    command: NativeCommand,
) -> Result<Transaction, StateError> {
    let prepared = state.apply(rewrite_transaction(state, rewrite)?)?;
    let mut output = None;
    command(&prepared, Some(&mut |tr| output = Some(tr)));
    let Some(tr) = output else {
        return Ok(state.tr());
    };
    let mut result = native_transaction(
        state,
        tr.doc(),
        Selection {
            anchor: tr.selection().anchor(),
            head: tr.selection().head(),
        },
    )?;
    result.set_stored_marks(tr.stored_marks().map(<[_]>::to_vec));
    Ok(result)
}
