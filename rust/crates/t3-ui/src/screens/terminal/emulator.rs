//! Port of the surface contract `ThreadTerminalDrawer.tsx` drives in
//! `apps/web/src/terminal/ghostty/surface.ts` (`GhosttyTerminalSurface`): output in
//! (`write`, `resetAndWrite`), selection, focus, and input and grid changes out.
//!
//! The emulator is NSH's `Terminal` (its own `VtEmulator` until `alacritty_terminal` lands behind
//! `TerminalEmulatorFactory`; `reports/ui-term-3.md`). It draws the grid with each platform's text
//! engine and owns the keyboard, input method and soft keyboard, so one surface serves desktop,
//! web and mobile. Selection is the platform's own over its rows, so the drawer's selection
//! comparison sees none (TERM-4).
use nsh_base::{Terminal, TerminalApi, TerminalEvent};
use nsh_core::prelude::*;
use std::{cell::RefCell, rc::Rc};
use t3_client::{
    services::terminal::TerminalGrid, view_logic::session::terminal_drawer::TerminalSurfaceSink,
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
    /// The emulator, once its element is made.
    terminal: Option<ComponentRef<Terminal>>,
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

impl TerminalSurface {
    fn api(&self) -> Option<Rc<TerminalApi>> {
        self.0
            .borrow()
            .terminal
            .as_ref()
            .and_then(|terminal| terminal.api::<TerminalApi>())
    }
    /// Takes the keyboard focus (showing the software keyboard).
    pub fn focus(&self) {
        if let Some(api) = self.api() {
            api.focus();
        }
    }
}

impl TerminalSurfaceSink for TerminalSurface {
    fn reset_and_write(&mut self, data: &str) {
        if let Some(api) = self.api() {
            api.reset();
            api.write(data.to_owned());
        }
    }
    fn write(&mut self, data: &str) {
        if let Some(api) = self.api() {
            api.write(data.to_owned());
        }
    }
    fn selection_cell_count(&self) -> usize {
        0
    }
    fn selection(&self) -> String {
        String::new()
    }
    fn clear_selection(&mut self) {}
}

/// The pane the emulator draws in: the web's viewport container
/// (`relative h-full w-full overflow-hidden bg-(--terminal-background)`) holding the terminal.
pub fn terminal_surface(surface: TerminalSurface, label: String) -> impl IntoElement {
    let terminal = component_ref::<Terminal>();
    surface.0.borrow_mut().terminal = Some(terminal);
    let (input, grid) = (surface.clone(), surface);
    view()
        .app_class(tw(
            "relative h-full w-full overflow-hidden bg-(--terminal-background)",
        ))
        .child(
            nsh_base::terminal()
                .component_ref(&terminal)
                .accessibility_label(Some(SharedString::from(label)))
                .on_data(move |data: &SharedString| input.send(data.to_string()))
                .on_resize(move |event: &TerminalEvent| {
                    if let TerminalEvent::Resize(cols, rows) = event {
                        grid.report_grid(TerminalGrid {
                            cols: u64::from(*cols),
                            rows: u64::from(*rows),
                        });
                    }
                })
                .size_full(),
        )
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
