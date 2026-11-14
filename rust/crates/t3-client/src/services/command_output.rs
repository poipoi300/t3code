//! One command row's output while it is open. Ports `packages/client-runtime/src/state/
//! commandOutput.ts` and the `commandOutput` subscription rules of `state/orchestration.ts`:
//! subscribed only while a row holds a lease, ended once the command settles, and a settled
//! command's final output kept for five minutes so reopening the row does not refetch.
use crate::ClientError;
use futures::{Stream, StreamExt, channel::mpsc, stream::LocalBoxStream};
use std::{cell::RefCell, rc::Rc};
use t3_contracts::orchestration_v2::{CommandOutputFrameKind, OrchestrationV2CommandOutputFrame};
use t3_shared::terminal_output::{
    TERMINAL_OUTPUT_TAIL_CHARS, TerminalOutputState, append_terminal_output,
};

/// How long a settled command's output stays cached after its last reader closes.
pub const COMPLETED_OUTPUT_TTL_MS: u64 = 5 * 60_000;

/// One command row's output as a client holds it.
#[derive(Clone)]
pub struct CommandOutputView {
    pub output: TerminalOutputState,
    /// False once the command settled and `output` is final.
    pub running: bool,
}

impl Default for CommandOutputView {
    fn default() -> Self {
        Self {
            output: TerminalOutputState::default(),
            running: true,
        }
    }
}

/// Applies one server frame. A `replace` carries the whole bounded tail (the first frame, a
/// viewer that fell behind, or the final output); an `append` is raw terminal text normalized
/// with the same rules the server uses.
pub fn apply_command_output_frame(
    view: &CommandOutputView,
    frame: &OrchestrationV2CommandOutputFrame,
) -> CommandOutputView {
    let mut base = match frame.kind {
        CommandOutputFrameKind::Replace => TerminalOutputState::default(),
        CommandOutputFrameKind::Append => view.output.clone(),
    };
    base.truncated |= frame.truncated;
    CommandOutputView {
        output: append_terminal_output(&base, &frame.text, TERMINAL_OUTPUT_TAIL_CHARS),
        running: frame.running,
    }
}

/// What every lease of one command row reads.
#[derive(Default)]
pub struct CommandOutputCell {
    view: RefCell<Option<CommandOutputView>>,
    error: RefCell<Option<String>>,
    listeners: RefCell<Vec<mpsc::UnboundedSender<()>>>,
}

impl CommandOutputCell {
    /// A cell that already holds a settled command's final output.
    pub fn settled(view: CommandOutputView) -> Self {
        Self {
            view: RefCell::new(Some(view)),
            ..Self::default()
        }
    }
    /// `None` until the first frame arrives (loading).
    pub fn view(&self) -> Option<CommandOutputView> {
        self.view.borrow().clone()
    }
    /// Why the output could not be loaded, until a later frame arrives.
    pub fn error(&self) -> Option<String> {
        self.error.borrow().clone()
    }
    pub fn changes(&self) -> LocalBoxStream<'static, ()> {
        let (sender, receiver) = mpsc::unbounded();
        self.listeners.borrow_mut().push(sender);
        receiver.boxed_local()
    }
    fn notify(&self) {
        self.listeners
            .borrow_mut()
            .retain(|listener| listener.unbounded_send(()).is_ok());
    }
}

/// Feeds `frames` (each session's subscription, resubscribed after a reconnect) into `cell`
/// until the command settles; returns its final output, or `None` if the frames ended first.
pub async fn drive_command_output(
    cell: Rc<CommandOutputCell>,
    mut frames: impl Stream<Item = Result<OrchestrationV2CommandOutputFrame, ClientError>> + Unpin,
) -> Option<CommandOutputView> {
    while let Some(frame) = frames.next().await {
        match frame {
            Ok(frame) => {
                let next = {
                    let current = cell.view.borrow();
                    apply_command_output_frame(
                        current.as_ref().unwrap_or(&CommandOutputView::default()),
                        &frame,
                    )
                };
                let settled = !next.running;
                *cell.view.borrow_mut() = Some(next.clone());
                cell.error.borrow_mut().take();
                cell.notify();
                if settled {
                    return Some(next);
                }
            }
            Err(error) => {
                *cell.error.borrow_mut() = Some(error.to_string());
                cell.notify();
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(kind: CommandOutputFrameKind, text: &str, running: bool) -> OrchestrationV2CommandOutputFrame {
        OrchestrationV2CommandOutputFrame {
            kind,
            text: text.into(),
            truncated: false,
            running,
        }
    }

    #[test]
    fn continues_an_escape_and_a_redraw_split_across_appends() {
        // commandOutput.test.ts
        let view = apply_command_output_frame(
            &CommandOutputView::default(),
            &frame(CommandOutputFrameKind::Replace, "install\n12%\u{1b}[3", true),
        );
        assert_eq!(view.output.text, "install\n12%");
        let view = apply_command_output_frame(
            &view,
            &frame(CommandOutputFrameKind::Append, "2m\r100%\u{1b}[0m\n", true),
        );
        // The colour opened before the redraw survives it.
        assert_eq!(view.output.text, "install\n\u{1b}[0;32m100%\u{1b}[0m\n");
        assert!(view.running);
    }

    #[test]
    fn a_replace_starts_over_and_truncation_sticks_until_then() {
        let mut truncated = frame(CommandOutputFrameKind::Replace, "old", true);
        truncated.truncated = true;
        let view = apply_command_output_frame(&CommandOutputView::default(), &truncated);
        let view = apply_command_output_frame(
            &view,
            &frame(CommandOutputFrameKind::Append, " more", true),
        );
        assert!(view.output.truncated);
        assert_eq!(view.output.text, "old more");
        let view = apply_command_output_frame(
            &view,
            &frame(CommandOutputFrameKind::Replace, "final\n", false),
        );
        assert_eq!(view.output.text, "final\n");
        assert!(!view.output.truncated);
        assert!(!view.running);
    }
}
