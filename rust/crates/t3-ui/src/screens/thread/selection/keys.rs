//! Port of apps/web/src/keybindings.ts shortcut matching and ChatMarkdown.tsx handleCopy, scoped to the retained timeline.

use std::ops::Range;

use nsh_core::element::ClipboardItem;
use nsh_core::keymap::Keystroke;
use nsh_core::prelude::*;
use t3_client::view_logic::shell::keybindings::{self, KeyEvent};

use super::model::{
    BlockPosition, SelectedSource, SelectionDocument, SelectionError, SelectionPoint,
    SelectionRange,
};
use crate::keys::bindings::{Keybindings, client_event};
use crate::screens::thread::blocks::flatten::{BlockDelta, BlockId, StreamBlocks};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimelineCommand {
    SelectAll,
    Copy,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SelectionFocus {
    pub timeline: bool,
    pub editable: bool,
    pub terminal: bool,
    pub preview: bool,
    pub modal: bool,
    pub binding_capture: bool,
}

impl SelectionFocus {
    fn eligible(self) -> bool {
        self.timeline
            && !self.editable
            && !self.terminal
            && !self.preview
            && !self.modal
            && !self.binding_capture
    }
}

/// Native defaults own these chords outside this scope, particularly in the composer/terminal.
pub fn timeline_shortcut(
    event: &KeyEvent,
    focus: SelectionFocus,
    platform: &str,
) -> Option<TimelineCommand> {
    if !focus.eligible() || event.editable_focus || !event.is_keydown() || event.alt_graph {
        return None;
    }
    [
        ("mod+a", TimelineCommand::SelectAll),
        ("mod+c", TimelineCommand::Copy),
    ]
    .into_iter()
    .find_map(|(chord, command)| {
        keybindings::parse_shortcut(chord)
            .filter(|shortcut| keybindings::matches_shortcut(event, shortcut, platform))
            .map(|_| command)
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopyError {
    Selection(SelectionError),
    SerializerUnavailable,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CopyStatus {
    #[default]
    Idle,
    Empty,
    Written,
    Failed(CopyError),
}

// TL-SELECT-2 owns Markdown/HTML serialization; raw source or native text is not equivalent.
fn selection_clipboard_payload(
    _selection: &[SelectedSource],
) -> Result<Option<ClipboardItem>, CopyError> {
    Err(CopyError::SerializerUnavailable)
}

/// One reactive handle per thread above recycled rows, shared with the block list and find scope.
#[derive(Clone, Copy)]
pub struct TimelineSelection {
    model: Stored<SelectionDocument>,
    revision: Signal<u64>,
    pub copy_status: Signal<CopyStatus>,
}

impl Default for TimelineSelection {
    fn default() -> Self {
        Self::new()
    }
}

impl TimelineSelection {
    pub fn new() -> Self {
        Self {
            model: stored(SelectionDocument::default()),
            revision: signal(0),
            copy_status: signal(CopyStatus::Idle),
        }
    }

    pub fn apply_delta(self, id: &str, delta: &BlockDelta) -> Result<(), SelectionError> {
        let result = self.model.with(|model| model.apply_delta(id, delta));
        if result.is_ok() {
            self.changed();
        }
        result
    }

    pub fn retain_document(self, id: &str, document: StreamBlocks) -> Result<(), SelectionError> {
        let result = self.model.with(|model| model.retain_document(id, document));
        if result.is_ok() {
            self.changed();
        }
        result
    }

    pub fn set_order(self, order: &[String]) -> Result<(), SelectionError> {
        let result = self.model.with(|model| model.set_order(order));
        if result.is_ok() {
            self.changed();
        }
        result
    }

    pub fn point(self, position: BlockPosition) -> Result<SelectionPoint, SelectionError> {
        self.model.with(|model| model.point(position))
    }

    pub fn select(
        self,
        anchor: SelectionPoint,
        focus: SelectionPoint,
    ) -> Result<(), SelectionError> {
        let result = self.model.with(|model| model.select(anchor, focus));
        if result.is_ok() {
            self.changed();
        }
        result
    }

    pub fn select_all(self) -> bool {
        let selected = self.model.with(SelectionDocument::select_all);
        self.changed();
        selected
    }

    pub fn clear(self) {
        self.model.with(SelectionDocument::clear);
        self.changed();
    }

    pub fn selection(self) -> Option<SelectionRange> {
        self.revision.get();
        self.model.with(|model| model.selection())
    }

    pub fn has_selection(self) -> bool {
        self.revision.get();
        self.model.with(|model| model.has_selection())
    }

    pub fn block_range(self, id: &BlockId) -> Option<Range<usize>> {
        self.revision.get();
        self.model.with(|model| model.block_range(id))
    }

    pub fn selected_sources(self) -> Result<Vec<SelectedSource>, SelectionError> {
        self.model.with(|model| model.selected_sources())
    }

    /// Called directly by keys/menu actions; no success state until a payload reaches NSH.
    pub fn copy(self) -> Result<bool, CopyError> {
        let result = self
            .selected_sources()
            .map_err(CopyError::Selection)
            .and_then(|selected| {
                if selected.is_empty() {
                    return Ok(None);
                }
                selection_clipboard_payload(&selected)
            });
        match result {
            Ok(Some(payload)) => {
                set_clipboard_item(payload);
                self.copy_status.set(CopyStatus::Written);
                Ok(true)
            }
            Ok(None) => {
                self.copy_status.set(CopyStatus::Empty);
                Ok(false)
            }
            Err(error) => {
                self.copy_status.set(CopyStatus::Failed(error));
                Err(error)
            }
        }
    }

    fn changed(self) {
        self.copy_status.set(CopyStatus::Idle);
        self.revision
            .update(|revision| *revision = revision.wrapping_add(1));
    }

    pub fn dispatch(self, event: &KeyEvent, focus: SelectionFocus, platform: &str) -> bool {
        match timeline_shortcut(event, focus, platform) {
            Some(TimelineCommand::SelectAll) => {
                self.select_all();
                true
            }
            Some(TimelineCommand::Copy) if self.has_selection() => {
                // Keep an unavailable copy from falling through to native mounted-only copying.
                let _ = self.copy();
                true
            }
            _ => false,
        }
    }
}

/// Wrap only the timeline, never the composer or app shell. No window-global A/C bindings.
pub fn timeline_selection_scope(
    selection: TimelineSelection,
    children: impl IntoElement,
) -> impl IntoElement {
    let focused = signal(false);
    let keys = use_context::<Keybindings>();
    let platform = if apple_keys() { "MacIntel" } else { "Linux" };
    let focus = move || {
        let context = keys.map(|keys| keys.context.read());
        let flag = |name| context.as_ref().is_some_and(|context| context.get(name));
        SelectionFocus {
            timeline: focused.get(),
            editable: flag("editableFocus"),
            terminal: flag("terminalFocus"),
            preview: flag("previewFocus"),
            modal: crate::ui::confirm::is_confirm_dialog_active(),
            binding_capture: flag("keybindingCapture"),
        }
    };
    let all = Keystroke::parse_with("mod+a", apple_keys()).ok();
    let copy = Keystroke::parse_with("mod+c", apple_keys()).ok();
    // KeyCapture consumes before native defaults; returning from on_key_down cannot veto them.
    view()
        .flex_1()
        .min_h(px(0.0))
        .focusable(true)
        .test_id("timeline-selection")
        .selectable(true)
        .on_focus(move || focused.set(true))
        .on_blur(move || focused.set(false))
        .capture_keys(move || {
            if !focus().eligible() {
                return KeyCapture::None;
            }
            let selected = selection.has_selection();
            KeyCapture::keys(all.into_iter().chain(selected.then_some(copy).flatten()))
        })
        .on_key_down(move |raw: &nsh_core::KeyEvent| {
            if raw.default_prevented {
                selection.dispatch(&client_event(raw, true, false), focus(), platform);
            }
        })
        .child(children)
}

#[cfg(test)]
mod tests {
    use super::*;
    use keybindings::Modifiers;

    fn event(key: &str, apple: bool) -> KeyEvent {
        KeyEvent {
            key: key.into(),
            modifiers: Modifiers {
                meta: apple,
                ctrl: !apple,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn exact_mod_chords_share_the_clients_keyboard_conventions() {
        let focus = SelectionFocus {
            timeline: true,
            ..Default::default()
        };
        for (platform, apple) in [
            ("MacIntel", true),
            ("iPad", true),
            ("Win32", false),
            ("Linux", false),
        ] {
            assert_eq!(
                timeline_shortcut(&event("A", apple), focus, platform),
                Some(TimelineCommand::SelectAll)
            );
            assert_eq!(
                timeline_shortcut(&event("c", apple), focus, platform),
                Some(TimelineCommand::Copy)
            );
            assert_eq!(
                timeline_shortcut(&event("c", !apple), focus, platform),
                None
            );
            for modifier in [
                Modifiers {
                    shift: true,
                    ..event("c", apple).modifiers
                },
                Modifiers {
                    alt: true,
                    ..event("c", apple).modifiers
                },
                Modifiers {
                    ctrl: true,
                    meta: true,
                    ..Default::default()
                },
            ] {
                assert_eq!(
                    timeline_shortcut(
                        &KeyEvent {
                            modifiers: modifier,
                            ..event("c", apple)
                        },
                        focus,
                        platform
                    ),
                    None
                );
            }
        }
    }

    #[test]
    fn inactive_editable_terminal_preview_modal_and_released_keys_are_never_claimed() {
        let active = SelectionFocus {
            timeline: true,
            ..Default::default()
        };
        for focus in [
            SelectionFocus::default(),
            SelectionFocus {
                editable: true,
                ..active
            },
            SelectionFocus {
                terminal: true,
                ..active
            },
            SelectionFocus {
                preview: true,
                ..active
            },
            SelectionFocus {
                modal: true,
                ..active
            },
            SelectionFocus {
                binding_capture: true,
                ..active
            },
        ] {
            assert_eq!(timeline_shortcut(&event("a", false), focus, "Win32"), None);
            assert_eq!(timeline_shortcut(&event("c", false), focus, "Win32"), None);
        }
        for event in [
            KeyEvent {
                event_type: Some("keyup".into()),
                ..event("c", false)
            },
            KeyEvent {
                editable_focus: true,
                ..event("a", false)
            },
            KeyEvent {
                alt_graph: true,
                ..event("c", false)
            },
        ] {
            assert_eq!(timeline_shortcut(&event, active, "Win32"), None);
        }
        assert_eq!(
            timeline_shortcut(
                &KeyEvent {
                    key: "Escape".into(),
                    ..Default::default()
                },
                active,
                "Win32"
            ),
            None
        );
    }
}
