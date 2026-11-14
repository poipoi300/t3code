//! Port of the surface contract `ThreadTerminalDrawer.tsx` drives in
//! `apps/web/src/terminal/ghostty/surface.ts` (`GhosttyTerminalSurface`): output in
//! (`write`, `resetAndWrite`), selection, focus, and input and grid changes out.
//!
//! The emulator behind it is not in the workspace yet. `t3-ui` builds `nsh-base` without its
//! `term` feature, so `nsh_base::Terminal` and `TerminalEmulatorFactory` are not compiled, and
//! `alacritty_terminal` (with the wasm cfg patch) is not a dependency. Until both land
//! (`reports/ui-term-3.md` has the manifest change and the element body), the surface keeps
//! the web's pane and drops output: it never accumulates the stream, so nothing grows with
//! the scrollback.
use nsh_core::prelude::*;
use std::{cell::RefCell, rc::Rc};
use t3_client::{
    services::terminal::TerminalGrid,
    view_logic::session::terminal_drawer::TerminalSurfaceSink,
};

use crate::theme::tw;

type DataListener = Rc<dyn Fn(String)>;
type GridListener = Rc<dyn Fn(TerminalGrid)>;

#[derive(Default)]
struct SurfaceInner {
    /// The grid the emulator last measured, `None` before its first layout.
    grid: Option<TerminalGrid>,
    on_data: Option<DataListener>,
    on_resize: Option<GridListener>,
    read_only: bool,
}

/// One pane's emulator surface, shared by the pane's view and its session.
#[derive(Clone, Default)]
pub struct TerminalSurface(Rc<RefCell<SurfaceInner>>);

impl TerminalSurface {
    /// What the user types or pastes (and the emulator's answers to the program's queries).
    pub fn on_data(&self, listener: impl Fn(String) + 'static) {
        self.0.borrow_mut().on_data = Some(Rc::new(listener));
    }
    /// The grid the emulator fits to its box, first once it is laid out.
    pub fn on_resize(&self, listener: impl Fn(TerminalGrid) + 'static) {
        self.0.borrow_mut().on_resize = Some(Rc::new(listener));
    }
    /// Without operate permission keys are not sent (the web's `input.readOnly`).
    pub fn set_read_only(&self, read_only: bool) {
        self.0.borrow_mut().read_only = read_only;
    }
    /// Reports the measured grid again, for a session that has not seen it yet (a new PTY or
    /// a reconnect).
    pub fn resend_size(&self) {
        let (grid, listener) = {
            let inner = self.0.borrow();
            (inner.grid, inner.on_resize.clone())
        };
        if let (Some(grid), Some(listener)) = (grid, listener) {
            listener(grid);
        }
    }
    /// The emulator measured `grid`.
    pub fn report_grid(&self, grid: TerminalGrid) {
        let listener = {
            let mut inner = self.0.borrow_mut();
            if inner.grid == Some(grid) {
                return;
            }
            inner.grid = Some(grid);
            inner.on_resize.clone()
        };
        if let Some(listener) = listener {
            listener(grid);
        }
    }
    /// The emulator produced input for the program.
    pub fn send(&self, data: String) {
        let listener = {
            let inner = self.0.borrow();
            (!inner.read_only && !data.is_empty())
                .then(|| inner.on_data.clone())
                .flatten()
        };
        if let Some(listener) = listener {
            listener(data);
        }
    }
}

impl TerminalSurfaceSink for TerminalSurface {
    fn reset_and_write(&mut self, _data: &str) {}
    fn write(&mut self, _data: &str) {}
    fn selection_cell_count(&self) -> usize {
        0
    }
    fn selection(&self) -> String {
        String::new()
    }
    fn clear_selection(&mut self) {}
}

/// The pane the emulator draws in: the web's viewport container
/// (`relative h-full w-full overflow-hidden bg-(--terminal-background)`).
pub fn terminal_surface(_surface: TerminalSurface, label: String) -> impl IntoElement {
    view()
        .app_class(tw(
            "relative h-full w-full overflow-hidden bg-(--terminal-background)",
        ))
        .accessibility_label(Some(SharedString::from(label)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn input_and_grid_reach_the_session_once_and_read_only_drops_keys() {
        let surface = TerminalSurface::default();
        let sent = Rc::new(RefCell::new(Vec::new()));
        let grids = Rc::new(Cell::new(0));
        let (into, count) = (sent.clone(), grids.clone());
        surface.on_data(move |data| into.borrow_mut().push(data));
        surface.on_resize(move |_| count.set(count.get() + 1));
        surface.resend_size();
        assert_eq!(grids.get(), 0, "nothing measured yet");
        let grid = TerminalGrid { cols: 80, rows: 24 };
        surface.report_grid(grid);
        surface.report_grid(grid);
        assert_eq!(grids.get(), 1, "an unchanged grid is not reported again");
        surface.resend_size();
        assert_eq!(grids.get(), 2);
        surface.send("ls\r".into());
        surface.set_read_only(true);
        surface.send("rm\r".into());
        assert_eq!(*sent.borrow(), ["ls\r"]);
    }
}
