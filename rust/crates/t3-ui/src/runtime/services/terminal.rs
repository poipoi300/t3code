//! Binding of the client's terminal service (`t3_client::services::terminal_sessions`) to the
//! terminal lane (TERM-*). Pure delegation: the rules live in the client.
use futures::future::LocalBoxFuture;
use std::rc::Rc;
use t3_client::{
    ClientError,
    platform::Platform,
    services::{
        terminal::TerminalGrid,
        terminal_sessions::{
            LaunchLocation, Placement, TerminalDrawerLease, TerminalLaunch, TerminalSessionLease,
            TerminalSessions,
        },
    },
    types::ThreadKey,
};

/// What the drawer, its panes and their sessions ask of the client.
pub trait TerminalClient {
    fn load(&self) -> LocalBoxFuture<'static, Result<(), ClientError>>;
    fn drawer(&self, key: &ThreadKey, launch: Option<LaunchLocation>) -> TerminalDrawerLease;
    fn session(&self, key: &ThreadKey, terminal: &str, launch: &TerminalLaunch) -> TerminalSessionLease;
    fn can_operate(&self, key: &ThreadKey) -> bool;
    fn can_read(&self, key: &ThreadKey) -> bool;
    fn set_open(&self, key: &ThreadKey, open: bool);
    fn toggle(&self, key: &ThreadKey);
    fn set_height(&self, key: &ThreadKey, height: f64);
    fn activate(&self, key: &ThreadKey, terminal: &str);
    /// Places a new tab or split and returns its id with the open still running.
    fn launch(
        &self,
        key: &ThreadKey,
        placement: Placement,
        location: Option<LaunchLocation>,
    ) -> Option<(String, LocalBoxFuture<'static, Result<(), ClientError>>)>;
    fn close_terminal(&self, key: &ThreadKey, terminal: &str) -> Option<LocalBoxFuture<'static, ()>>;
    fn write(&self, key: &ThreadKey, terminal: &str, data: String) -> LocalBoxFuture<'static, Result<(), ClientError>>;
    fn resize(&self, key: &ThreadKey, terminal: &str, grid: TerminalGrid) -> LocalBoxFuture<'static, Result<(), ClientError>>;
}

impl<P: Platform + Clone + 'static> TerminalClient for TerminalSessions<P> {
    fn load(&self) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        TerminalSessions::load(self)
    }
    fn drawer(&self, key: &ThreadKey, launch: Option<LaunchLocation>) -> TerminalDrawerLease {
        TerminalSessions::drawer(self, key, launch)
    }
    fn session(&self, key: &ThreadKey, terminal: &str, launch: &TerminalLaunch) -> TerminalSessionLease {
        TerminalSessions::session(
            self,
            &key.environment,
            TerminalSessions::<P>::attach_input(key, terminal, launch),
        )
    }
    fn can_operate(&self, key: &ThreadKey) -> bool {
        TerminalSessions::can_operate(self, &key.environment)
    }
    fn can_read(&self, key: &ThreadKey) -> bool {
        TerminalSessions::can_read(self, &key.environment)
    }
    fn set_open(&self, key: &ThreadKey, open: bool) {
        TerminalSessions::set_open(self, key, open);
    }
    fn toggle(&self, key: &ThreadKey) {
        TerminalSessions::toggle(self, key);
    }
    fn set_height(&self, key: &ThreadKey, height: f64) {
        TerminalSessions::set_height(self, key, height);
    }
    fn activate(&self, key: &ThreadKey, terminal: &str) {
        TerminalSessions::activate(self, key, terminal);
    }
    fn launch(
        &self,
        key: &ThreadKey,
        placement: Placement,
        location: Option<LaunchLocation>,
    ) -> Option<(String, LocalBoxFuture<'static, Result<(), ClientError>>)> {
        use futures::FutureExt;
        self.launch_drawer_terminal(key, placement, location)
            .map(|(id, opened)| (id, opened.map(|result| result.map(|_| ())).boxed_local()))
    }
    fn close_terminal(&self, key: &ThreadKey, terminal: &str) -> Option<LocalBoxFuture<'static, ()>> {
        self.close_drawer_terminal(key, terminal)
    }
    fn write(&self, key: &ThreadKey, terminal: &str, data: String) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        TerminalSessions::write(self, key, terminal, data)
    }
    fn resize(&self, key: &ThreadKey, terminal: &str, grid: TerminalGrid) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        TerminalSessions::resize(self, key, terminal, grid)
    }
}

/// The terminal service the lane's views share.
#[derive(Clone)]
pub struct TerminalServices(pub Rc<dyn TerminalClient>);

impl TerminalServices {
    pub fn new<P: Platform + Clone + 'static>(service: TerminalSessions<P>) -> Self {
        Self(Rc::new(service))
    }
}

/// The host's terminal service, `None` on the demo backend. Read in a component body.
pub fn host_terminals() -> Option<TerminalServices> {
    crate::runtime::rt()
        .backend()
        .client()
        .map(|client| TerminalServices::new(client.terminals()))
}
