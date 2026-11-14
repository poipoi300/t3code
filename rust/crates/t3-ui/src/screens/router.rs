//! Route to screen. Frozen: the integrator edits it. Every route calls the screen function of the
//! package that owns it; a package that has not landed shows `ui::placeholder::not_built`.

use nsh_core::prelude::*;

use super::{
    add_project, agent, archived, connect, guest, home, new_thread, not_found, onboarding,
    project_link, pull_requests, search, settings, thread, usage,
};
use crate::state::nav::{Route, ThreadPanel};
use crate::ui::placeholder::not_built;

pub fn screen(route: &Route) -> AnyElement {
    match route {
        Route::Home => home::screen(),
        Route::NewThread => new_thread::screen(None),
        Route::Draft(id) => new_thread::screen(Some(id)),
        Route::Thread(key) => thread::screen(key.clone()),
        Route::ThreadPanel(_, panel) => not_built(match panel {
            ThreadPanel::Files => "Files",
            ThreadPanel::Diff => "Diff",
            ThreadPanel::Terminal => "Terminal",
            ThreadPanel::Browser => "Browser",
            ThreadPanel::Device => "Device",
            ThreadPanel::PullRequest => "Pull request",
        }),
        Route::ProjectLink(key) => project_link::screen(key),
        Route::AddProject => add_project::screen(),
        Route::PullRequests => pull_requests::list::screen(),
        Route::Usage => usage::screen(),
        Route::Archived => archived::screen(),
        Route::Search => search::screen(),
        Route::Settings(page) => settings::shell::screen(*page),
        Route::Welcome => onboarding::welcome::screen(),
        Route::Pair | Route::Connect => connect::screen(),
        Route::ConnectAgent | Route::ApproveAgent => agent::screen(),
        Route::Guest => guest::screen(),
        Route::NotFound(path) => not_found::screen(path),
    }
}
