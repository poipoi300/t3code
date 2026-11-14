//! Routes: every screen of the product, its address, and the stack that shows it.
//!
//! One `Route` value drives every layout: a wide window shows the sidebar beside the top route, a
//! phone shows the stack as pages. The same paths as the web app, so links keep working.

use nsh_core::prelude::*;
pub use t3_client::navigation::{Route, SettingsPage, ThreadPanel, initial_route, path_of};

/// The route stack. The bottom is always [`Route::Home`]; the top is what the main area shows.
#[derive(Clone, Copy)]
pub struct Navigator {
    pub stack: Signal<Vec<Route>>,
}

impl Navigator {
    pub fn new(initial: Route) -> Self {
        Self {
            stack: signal(stack_for(initial)),
        }
    }

    pub fn top(&self) -> Route {
        self.stack
            .with(|s| s.last().cloned().unwrap_or(Route::Home))
    }

    /// Opens `route` as the main content: a thread or a top-level page replaces what sat above
    /// Home; a thread panel goes over its thread.
    pub fn open(&self, route: Route) {
        self.stack.update(|stack| *stack = next_stack(stack, route));
    }

    pub fn back(&self) {
        self.stack.update(|stack| {
            if stack.len() > 1 {
                stack.pop();
            }
        });
    }

    /// The user left pages natively (swipe back, the Android back button): keep `len` pages.
    pub fn truncate(&self, len: usize) {
        self.stack.update(|stack| stack.truncate(len.max(1)));
    }
}

pub fn stack_for(route: Route) -> Vec<Route> {
    next_stack(&[Route::Home], route)
}

pub fn next_stack(current: &[Route], route: Route) -> Vec<Route> {
    match route {
        Route::Home => vec![Route::Home],
        Route::ThreadPanel(ref key, _) => {
            let base = Route::Thread(key.clone());
            vec![Route::Home, base, route]
        }
        Route::Settings(_) => {
            let mut stack: Vec<Route> = current
                .iter()
                .take_while(|r| !matches!(r, Route::Settings(_)))
                .cloned()
                .collect();
            if stack.is_empty() {
                stack.push(Route::Home);
            }
            stack.push(route);
            stack
        }
        other => vec![Route::Home, other],
    }
}

/// Provided by `App`.
pub fn navigator() -> Navigator {
    use_context::<Navigator>().expect("App provides the navigator")
}

#[cfg(test)]
mod tests {
    use super::*;
    use t3_client::types::{EnvironmentId, ThreadId, ThreadKey};

    fn key(environment: &str, thread: &str) -> ThreadKey {
        ThreadKey {
            environment: EnvironmentId(environment.into()),
            thread: ThreadId(thread.into()),
        }
    }

    #[test]
    fn opening_keeps_home_at_the_bottom_and_settings_over_their_origin() {
        let thread = Route::Thread(key("e", "t"));
        let stack = stack_for(thread.clone());
        assert_eq!(stack, vec![Route::Home, thread.clone()]);
        let other = Route::Thread(key("e", "u"));
        assert_eq!(next_stack(&stack, other.clone()), vec![Route::Home, other]);
        let settings = next_stack(&stack, Route::Settings(SettingsPage::General));
        assert_eq!(
            settings,
            vec![
                Route::Home,
                thread.clone(),
                Route::Settings(SettingsPage::General)
            ]
        );
        let page = next_stack(&settings, Route::Settings(SettingsPage::Chat));
        assert_eq!(
            page,
            vec![
                Route::Home,
                thread.clone(),
                Route::Settings(SettingsPage::Chat)
            ]
        );
        let panel = Route::ThreadPanel(key("e", "t"), ThreadPanel::Diff);
        assert_eq!(
            next_stack(&page, panel.clone()),
            vec![Route::Home, thread, panel]
        );
        assert_eq!(next_stack(&page, Route::Home), vec![Route::Home]);
    }
}
