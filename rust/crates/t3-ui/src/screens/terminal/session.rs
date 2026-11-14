//! Port of apps/web/src/components/ThreadTerminalDrawer.tsx TerminalViewport and
//! TerminalSessionViewport: one pane's session (attach or observe, from
//! `apps/web/src/state/terminalSessions.ts`) written into its surface, input and grid sent
//! back, and the exit handled.
//!
//! Output arrives in bursts: notices coalesce, and the surface is written once per display
//! frame with only what it has not shown (`terminal_drawer::synchronize_terminal_session`).
use nsh_core::prelude::*;
use std::{cell::RefCell, rc::Rc, time::Duration};
use t3_client::{
    ClientError,
    services::{
        terminal::{TerminalOutputCursor, TerminalOutputState, TerminalStatus},
        terminal_sessions::{TerminalLaunch, TerminalSessionView},
    },
    types::ThreadKey,
    view_logic::session::terminal_drawer::{
        SessionMark, TerminalSurfaceSink, should_handle_terminal_exit,
        synchronize_terminal_session, system_message, writable_session_generation,
    },
};

use super::emulator::{TerminalSurface, terminal_surface};
use crate::{i18n::t, runtime::services::terminal::TerminalServices};

use crate::runtime::tasks::watch;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitNotice {
    Closed,
    Exited,
}

/// What a session notice asks of the pane besides writing output.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PumpEffects {
    /// A new writable session has not seen the surface's grid yet.
    pub resend_size: bool,
    /// The shell ended while shown: close its tab once the message is written.
    pub exited: bool,
}

/// The pane's view of its session between notices.
#[derive(Debug, Clone)]
pub struct SessionPump {
    cursor: TerminalOutputCursor,
    mark: SessionMark,
    synchronized: TerminalStatus,
    handled_exit: bool,
    writable: Option<u64>,
}

impl SessionPump {
    /// The surface is new: replay the retained output, then any error.
    pub fn start(
        view: &TerminalSessionView,
        output: &TerminalOutputState,
        sink: &mut impl TerminalSurfaceSink,
        can_operate: bool,
        exit_text: impl Fn(ExitNotice) -> String,
    ) -> (Self, PumpEffects) {
        let initial = output.read(TerminalOutputCursor::default());
        if let t3_client::services::terminal::TerminalOutputUpdate::Reset { data, .. } = &initial
            && !data.is_empty()
        {
            sink.reset_and_write(data);
        }
        if let Some(error) = &view.error {
            sink.write(&system_message(error));
        }
        let mut pump = Self {
            cursor: initial.cursor(),
            mark: view.mark.clone(),
            synchronized: TerminalStatus::Closed,
            handled_exit: false,
            writable: writable_session_generation(
                view.version,
                view.status,
                view.generation,
                can_operate,
            ),
        };
        let exited = pump.sync_status(view.status, view.version, sink, &exit_text);
        (
            pump,
            PumpEffects {
                resend_size: false,
                exited,
            },
        )
    }

    /// A session notice: the grid for a new writable session, the status, then new output and
    /// a new error.
    pub fn update(
        &mut self,
        view: &TerminalSessionView,
        output: &TerminalOutputState,
        sink: &mut impl TerminalSurfaceSink,
        can_operate: bool,
        exit_text: impl Fn(ExitNotice) -> String,
    ) -> PumpEffects {
        let writable =
            writable_session_generation(view.version, view.status, view.generation, can_operate);
        let resend_size = writable != self.writable && writable.is_some();
        self.writable = writable;
        let exited = self.sync_status(view.status, view.version, sink, &exit_text);
        self.cursor =
            synchronize_terminal_session(sink, &self.mark, &view.mark, output, self.cursor);
        self.mark = view.mark.clone();
        PumpEffects {
            resend_size,
            exited,
        }
    }

    fn sync_status(
        &mut self,
        status: TerminalStatus,
        version: u64,
        sink: &mut impl TerminalSurfaceSink,
        exit_text: &impl Fn(ExitNotice) -> String,
    ) -> bool {
        let mut exited = false;
        if version > 0 && status == TerminalStatus::Running {
            self.handled_exit = false;
        } else if should_handle_terminal_exit(status, self.synchronized, self.handled_exit, version)
        {
            self.handled_exit = true;
            let notice = if status == TerminalStatus::Closed {
                ExitNotice::Closed
            } else {
                ExitNotice::Exited
            };
            sink.write(&system_message(&exit_text(notice)));
            exited = true;
        }
        if version > 0 {
            self.synchronized = status;
        }
        exited
    }

    /// Whether the exit is still to be handled (a restart since cancels it).
    pub fn exit_pending(&self) -> bool {
        self.handled_exit
    }

    /// Whether the session accepts the measured grid.
    pub fn writable(&self) -> bool {
        self.writable.is_some()
    }
}

fn exit_text(notice: ExitNotice) -> String {
    match notice {
        ExitNotice::Closed => t("Terminal closed"),
        ExitNotice::Exited => t("Process exited"),
    }
}

/// A failed command's message written into the terminal, unless it was interrupted.
fn report(surface: &TerminalSurface, error: ClientError, fallback: String) {
    if matches!(error, ClientError::Cancelled) {
        return;
    }
    let message = match &error {
        ClientError::Rejected { message, .. } => message.clone(),
        _ => fallback,
    };
    surface.clone().write(&system_message(&message));
}

/// One pane: the session of `terminal`, attached where it launches. A new launch (another
/// directory or environment) is a new session, as the web keys its viewport.
pub fn terminal_viewport(
    services: TerminalServices,
    key: ThreadKey,
    terminal: String,
    label: String,
    launch: TerminalLaunch,
    on_session_exited: Rc<dyn Fn()>,
) -> impl IntoElement {
    let lease = services.0.session(&key, &terminal, &launch);
    let surface = TerminalSurface::default();
    let pump: Rc<RefCell<Option<SessionPump>>> = Rc::default();
    surface.set_read_only(!services.0.can_operate(&key));
    {
        let (services, key, terminal, target) = (
            services.clone(),
            key.clone(),
            terminal.clone(),
            surface.clone(),
        );
        surface.on_data(move |data| {
            if !services.0.can_operate(&key) {
                return;
            }
            let written = services.0.write(&key, &terminal, data);
            let target = target.clone();
            watch(async move {
                if let Err(error) = written.await {
                    report(&target, error, t("Terminal write failed"));
                }
            });
        });
    }
    {
        let (services, key, terminal, pump) = (
            services.clone(),
            key.clone(),
            terminal.clone(),
            pump.clone(),
        );
        surface.on_resize(move |grid| {
            if !pump.borrow().as_ref().is_some_and(SessionPump::writable) {
                return;
            }
            let resized = services.0.resize(&key, &terminal, grid);
            watch(async move {
                let _ = resized.await;
            });
        });
    }
    let exited = {
        let (services, key, pump) = (services.clone(), key.clone(), pump.clone());
        Rc::new(move || {
            let (services, key, pump, on_exit) = (
                services.clone(),
                key.clone(),
                pump.clone(),
                on_session_exited.clone(),
            );
            // The web defers the close by a task so the exit message paints first.
            set_timeout(Duration::ZERO, move || {
                if pump
                    .borrow()
                    .as_ref()
                    .is_some_and(SessionPump::exit_pending)
                    && services.0.can_operate(&key)
                {
                    on_exit();
                }
            });
        })
    };
    {
        let mut sink = surface.clone();
        let can_operate = {
            let (services, key) = (services.clone(), key.clone());
            move || services.0.can_operate(&key)
        };
        let mut changes = lease.changes();
        let pump = pump.clone();
        // Started in the task, after the surface's terminal is mounted, so the replay reaches it.
        watch(async move {
            use futures::StreamExt;
            let view = lease.view();
            let (started, effects) = lease.with_output(|output| {
                SessionPump::start(&view, output, &mut sink, can_operate(), exit_text)
            });
            pump.replace(Some(started));
            if effects.exited {
                exited();
            }
            while changes.next().await.is_some() {
                // Everything that arrives before the frame is written in it, once.
                next_frame().await;
                let view = lease.view();
                sink.set_read_only(!can_operate());
                let effects = lease.with_output(|output| {
                    pump.borrow_mut()
                        .as_mut()
                        .map(|pump| pump.update(&view, output, &mut sink, can_operate(), exit_text))
                });
                let Some(effects) = effects else { return };
                if effects.resend_size {
                    sink.resend_size();
                }
                if effects.exited {
                    exited();
                }
            }
        });
    }
    terminal_surface(surface, label)
}

#[cfg(test)]
mod tests {
    use super::*;
    use t3_client::services::terminal::{AttachGenerations, TerminalBufferState};
    use t3_client::services::terminal_sessions::SessionMode;
    use t3_contracts::terminal::TerminalAttachStreamEvent;

    /// Counts the work the pump does on the surface.
    #[derive(Default)]
    struct Counted {
        writes: Vec<String>,
        resets: usize,
        bytes: usize,
    }
    impl TerminalSurfaceSink for Counted {
        fn reset_and_write(&mut self, data: &str) {
            self.resets += 1;
            self.bytes += data.len();
        }
        fn write(&mut self, data: &str) {
            self.bytes += data.len();
            self.writes.push(data.into());
        }
        fn selection_cell_count(&self) -> usize {
            0
        }
        fn selection(&self) -> String {
            String::new()
        }
        fn clear_selection(&mut self) {}
    }

    fn event(value: serde_json::Value) -> TerminalAttachStreamEvent {
        serde_json::from_value(value).unwrap_or_else(|error| panic!("{error}"))
    }
    fn snapshot(history: &str) -> TerminalAttachStreamEvent {
        event(
            serde_json::json!({"type":"snapshot","snapshot":{"threadId":"t","terminalId":"term-1","cwd":"/","worktreePath":null,"status":"running","pid":1,"history":history,"exitCode":null,"exitSignal":null,"label":"","updatedAt":"2026-01-01T00:00:00.000Z"}}),
        )
    }
    fn output(data: &str) -> TerminalAttachStreamEvent {
        event(serde_json::json!({"type":"output","threadId":"t","terminalId":"term-1","data":data}))
    }
    fn view_of(buffer: &TerminalBufferState) -> TerminalSessionView {
        TerminalSessionView {
            status: buffer.status,
            version: buffer.version,
            error: buffer.error.clone(),
            grid: buffer.grid,
            generation: buffer.output.generation,
            lifecycle_version: buffer.lifecycle_version,
            has_running_subprocess: false,
            mode: SessionMode::Attached,
            mark: SessionMark::of(buffer.version, &buffer.output, buffer.error.as_deref()),
        }
    }
    fn text(notice: ExitNotice) -> String {
        format!("{notice:?}")
    }

    #[test]
    fn a_burst_is_written_once_and_costs_only_the_new_bytes() {
        let mut generations = AttachGenerations::default();
        let mut buffer = generations.seed();
        let scrollback = "x".repeat(64 * 1024);
        buffer.apply(&snapshot(&scrollback), 1 << 21, &mut generations);
        let mut sink = Counted::default();
        let (mut pump, effects) =
            SessionPump::start(&view_of(&buffer), &buffer.output, &mut sink, true, text);
        assert_eq!((sink.resets, sink.bytes), (1, scrollback.len()));
        assert!(!effects.exited);
        // A burst of 1,000 chunks lands between two frames.
        for _ in 0..1_000 {
            buffer.apply(&output("ab"), 1 << 21, &mut generations);
        }
        sink.bytes = 0;
        pump.update(&view_of(&buffer), &buffer.output, &mut sink, true, text);
        assert_eq!(sink.writes.len(), 1, "one write per frame");
        assert_eq!(sink.bytes, 2_000, "no work proportional to the scrollback");
        assert_eq!(sink.resets, 1);
        // A frame with nothing new does no work.
        sink.bytes = 0;
        pump.update(&view_of(&buffer), &buffer.output, &mut sink, true, text);
        assert_eq!((sink.writes.len(), sink.bytes), (1, 0));
    }

    #[test]
    fn a_new_writable_generation_resends_the_grid_and_an_exit_closes_once() {
        let mut generations = AttachGenerations::default();
        let pending = generations.seed();
        let mut sink = Counted::default();
        let (mut pump, _) =
            SessionPump::start(&view_of(&pending), &pending.output, &mut sink, true, text);
        assert!(!pump.writable());
        let mut buffer = pending.clone();
        buffer.apply(&snapshot("$ "), 1 << 20, &mut generations);
        let effects = pump.update(&view_of(&buffer), &buffer.output, &mut sink, true, text);
        assert!(effects.resend_size && pump.writable());
        // Read-only viewers never resize the shell.
        let mut observer = SessionPump::start(
            &view_of(&pending),
            &pending.output,
            &mut Counted::default(),
            false,
            text,
        )
        .0;
        assert!(
            !observer
                .update(
                    &view_of(&buffer),
                    &buffer.output,
                    &mut Counted::default(),
                    false,
                    text
                )
                .resend_size
        );

        buffer.apply(
            &event(serde_json::json!({"type":"exited","threadId":"t","terminalId":"term-1","exitCode":0,"exitSignal":null})),
            1 << 20,
            &mut generations,
        );
        let effects = pump.update(&view_of(&buffer), &buffer.output, &mut sink, true, text);
        assert!(effects.exited && pump.exit_pending());
        assert_eq!(
            sink.writes.last().map(String::as_str),
            Some("\r\n[terminal] Exited\r\n")
        );
        let again = pump.update(&view_of(&buffer), &buffer.output, &mut sink, true, text);
        assert!(!again.exited, "an exit is handled once");
        // A restart cancels the pending close.
        buffer.apply(&snapshot("$ "), 1 << 20, &mut generations);
        pump.update(&view_of(&buffer), &buffer.output, &mut sink, true, text);
        assert!(!pump.exit_pending());
    }

    #[test]
    fn an_exit_that_landed_before_the_surface_is_handled_when_it_starts() {
        let mut generations = AttachGenerations::default();
        let mut buffer = generations.seed();
        buffer.apply(&snapshot("done"), 1 << 20, &mut generations);
        buffer.apply(
            &event(serde_json::json!({"type":"exited","threadId":"t","terminalId":"term-1","exitCode":0,"exitSignal":null})),
            1 << 20,
            &mut generations,
        );
        let mut sink = Counted::default();
        let (_, effects) =
            SessionPump::start(&view_of(&buffer), &buffer.output, &mut sink, true, text);
        assert!(effects.exited);
        assert_eq!(sink.writes, ["\r\n[terminal] Exited\r\n"]);
        // The surface starts from "closed", as the web's does: a session already closed then is
        // not a new exit.
        let mut closed = generations.seed();
        closed.apply(&snapshot("done"), 1 << 20, &mut generations);
        closed.apply(
            &event(serde_json::json!({"type":"closed","threadId":"t","terminalId":"term-1"})),
            1 << 20,
            &mut generations,
        );
        let (_, effects) = SessionPump::start(
            &view_of(&closed),
            &closed.output,
            &mut Counted::default(),
            true,
            text,
        );
        assert!(!effects.exited);
    }
}
