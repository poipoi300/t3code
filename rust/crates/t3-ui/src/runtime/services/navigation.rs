//! Thin adapters from the shared navigation services to UI contracts.
use crate::{
    i18n::Text,
    state::url::{UrlClient, UrlRequest, UrlSnapshot},
};
use futures::{FutureExt, StreamExt, future::LocalBoxFuture, stream::LocalBoxStream};
use std::rc::Rc;
use t3_client::{composition::ClientRuntime, platform::Platform};
struct UrlBinding<P>(t3_client::navigation::UrlClient<P>);
pub fn url_client<P: Platform + Clone + 'static>(runtime: &ClientRuntime<P>) -> Rc<dyn UrlClient> {
    Rc::new(UrlBinding(runtime.navigation()))
}
fn error(error: t3_client::ClientError) -> Text {
    match error {
        t3_client::ClientError::Rejected { message, detail } => Text::content(
            t3_client::error::rejection_text(&message, detail.as_deref()),
        ),
        error => Text::client_key(error.to_string()),
    }
}
impl<P: Platform + Clone + 'static> UrlClient for UrlBinding<P> {
    fn watch(&self) -> LocalBoxStream<'static, UrlSnapshot> {
        self.0
            .watch()
            .take_while(|item| futures::future::ready(item.is_ok()))
            .filter_map(|item| async { item.ok() })
            .boxed_local()
    }
    fn navigate(&self, request: UrlRequest) -> LocalBoxFuture<'static, Result<(), Text>> {
        let operation = self.0.navigate(request);
        async move { operation.await.map_err(error) }.boxed_local()
    }
}
pub(crate) fn client_route(route: crate::state::nav::Route) -> t3_client::navigation::Route {
    match route {
        crate::state::nav::Route::Home => t3_client::navigation::Route::Home,
        crate::state::nav::Route::NewThread => t3_client::navigation::Route::NewThread,
        crate::state::nav::Route::AddProject => t3_client::navigation::Route::AddProject,
        crate::state::nav::Route::PullRequests => t3_client::navigation::Route::PullRequests,
        crate::state::nav::Route::Usage => t3_client::navigation::Route::Usage,
        crate::state::nav::Route::Archived => t3_client::navigation::Route::Archived,
        crate::state::nav::Route::Search => t3_client::navigation::Route::Search,
        crate::state::nav::Route::Welcome => t3_client::navigation::Route::Welcome,
        crate::state::nav::Route::Pair => t3_client::navigation::Route::Pair,
        crate::state::nav::Route::Connect => t3_client::navigation::Route::Connect,
        crate::state::nav::Route::ConnectAgent => t3_client::navigation::Route::ConnectAgent,
        crate::state::nav::Route::ApproveAgent => t3_client::navigation::Route::ApproveAgent,
        crate::state::nav::Route::Guest => t3_client::navigation::Route::Guest,
        crate::state::nav::Route::Draft(value) => t3_client::navigation::Route::Draft(value),
        crate::state::nav::Route::Thread(value) => t3_client::navigation::Route::Thread(value),
        crate::state::nav::Route::ProjectLink(value) => {
            t3_client::navigation::Route::ProjectLink(value)
        }
        crate::state::nav::Route::NotFound(value) => t3_client::navigation::Route::NotFound(value),
        crate::state::nav::Route::Settings(page) => {
            t3_client::navigation::Route::Settings(match page {
                crate::state::nav::SettingsPage::General => {
                    t3_client::navigation::SettingsPage::General
                }
                crate::state::nav::SettingsPage::Appearance => {
                    t3_client::navigation::SettingsPage::Appearance
                }
                crate::state::nav::SettingsPage::Chat => t3_client::navigation::SettingsPage::Chat,
                crate::state::nav::SettingsPage::Diffs => {
                    t3_client::navigation::SettingsPage::Diffs
                }
                crate::state::nav::SettingsPage::Threads => {
                    t3_client::navigation::SettingsPage::Threads
                }
                crate::state::nav::SettingsPage::Worktrees => {
                    t3_client::navigation::SettingsPage::Worktrees
                }
                crate::state::nav::SettingsPage::Projects => {
                    t3_client::navigation::SettingsPage::Projects
                }
                crate::state::nav::SettingsPage::Providers => {
                    t3_client::navigation::SettingsPage::Providers
                }
                crate::state::nav::SettingsPage::SourceControl => {
                    t3_client::navigation::SettingsPage::SourceControl
                }
                crate::state::nav::SettingsPage::Secrets => {
                    t3_client::navigation::SettingsPage::Secrets
                }
                crate::state::nav::SettingsPage::ScheduledTasks => {
                    t3_client::navigation::SettingsPage::ScheduledTasks
                }
                crate::state::nav::SettingsPage::Browser => {
                    t3_client::navigation::SettingsPage::Browser
                }
                crate::state::nav::SettingsPage::Devices => {
                    t3_client::navigation::SettingsPage::Devices
                }
                crate::state::nav::SettingsPage::Connections => {
                    t3_client::navigation::SettingsPage::Connections
                }
                crate::state::nav::SettingsPage::Access => {
                    t3_client::navigation::SettingsPage::Access
                }
                crate::state::nav::SettingsPage::Voice => {
                    t3_client::navigation::SettingsPage::Voice
                }
                crate::state::nav::SettingsPage::Keybindings => {
                    t3_client::navigation::SettingsPage::Keybindings
                }
                crate::state::nav::SettingsPage::Archived => {
                    t3_client::navigation::SettingsPage::Archived
                }
                crate::state::nav::SettingsPage::Diagnostics => {
                    t3_client::navigation::SettingsPage::Diagnostics
                }
                crate::state::nav::SettingsPage::Maintenance => {
                    t3_client::navigation::SettingsPage::Maintenance
                }
                crate::state::nav::SettingsPage::Labs => t3_client::navigation::SettingsPage::Labs,
                crate::state::nav::SettingsPage::About => {
                    t3_client::navigation::SettingsPage::About
                }
                crate::state::nav::SettingsPage::Licenses => {
                    t3_client::navigation::SettingsPage::Licenses
                }
                crate::state::nav::SettingsPage::SnapShot => {
                    t3_client::navigation::SettingsPage::SnapShot
                }
            })
        }
        crate::state::nav::Route::ThreadPanel(key, panel) => {
            t3_client::navigation::Route::ThreadPanel(
                key,
                match panel {
                    crate::state::nav::ThreadPanel::Files => {
                        t3_client::navigation::ThreadPanel::Files
                    }
                    crate::state::nav::ThreadPanel::Diff => {
                        t3_client::navigation::ThreadPanel::Diff
                    }
                    crate::state::nav::ThreadPanel::Terminal => {
                        t3_client::navigation::ThreadPanel::Terminal
                    }
                    crate::state::nav::ThreadPanel::Browser => {
                        t3_client::navigation::ThreadPanel::Browser
                    }
                    crate::state::nav::ThreadPanel::Device => {
                        t3_client::navigation::ThreadPanel::Device
                    }
                    crate::state::nav::ThreadPanel::PullRequest => {
                        t3_client::navigation::ThreadPanel::PullRequest
                    }
                },
            )
        }
    }
}
pub(crate) fn ui_route(route: t3_client::navigation::Route) -> crate::state::nav::Route {
    match route {
        t3_client::navigation::Route::Home => crate::state::nav::Route::Home,
        t3_client::navigation::Route::NewThread => crate::state::nav::Route::NewThread,
        t3_client::navigation::Route::AddProject => crate::state::nav::Route::AddProject,
        t3_client::navigation::Route::PullRequests => crate::state::nav::Route::PullRequests,
        t3_client::navigation::Route::Usage => crate::state::nav::Route::Usage,
        t3_client::navigation::Route::Archived => crate::state::nav::Route::Archived,
        t3_client::navigation::Route::Search => crate::state::nav::Route::Search,
        t3_client::navigation::Route::Welcome => crate::state::nav::Route::Welcome,
        t3_client::navigation::Route::Pair => crate::state::nav::Route::Pair,
        t3_client::navigation::Route::Connect => crate::state::nav::Route::Connect,
        t3_client::navigation::Route::ConnectAgent => crate::state::nav::Route::ConnectAgent,
        t3_client::navigation::Route::ApproveAgent => crate::state::nav::Route::ApproveAgent,
        t3_client::navigation::Route::Guest => crate::state::nav::Route::Guest,
        t3_client::navigation::Route::Draft(value) => crate::state::nav::Route::Draft(value),
        t3_client::navigation::Route::Thread(value) => crate::state::nav::Route::Thread(value),
        t3_client::navigation::Route::ProjectLink(value) => {
            crate::state::nav::Route::ProjectLink(value)
        }
        t3_client::navigation::Route::NotFound(value) => crate::state::nav::Route::NotFound(value),
        t3_client::navigation::Route::Settings(page) => {
            crate::state::nav::Route::Settings(match page {
                t3_client::navigation::SettingsPage::General => {
                    crate::state::nav::SettingsPage::General
                }
                t3_client::navigation::SettingsPage::Appearance => {
                    crate::state::nav::SettingsPage::Appearance
                }
                t3_client::navigation::SettingsPage::Chat => crate::state::nav::SettingsPage::Chat,
                t3_client::navigation::SettingsPage::Diffs => {
                    crate::state::nav::SettingsPage::Diffs
                }
                t3_client::navigation::SettingsPage::Threads => {
                    crate::state::nav::SettingsPage::Threads
                }
                t3_client::navigation::SettingsPage::Worktrees => {
                    crate::state::nav::SettingsPage::Worktrees
                }
                t3_client::navigation::SettingsPage::Projects => {
                    crate::state::nav::SettingsPage::Projects
                }
                t3_client::navigation::SettingsPage::Providers => {
                    crate::state::nav::SettingsPage::Providers
                }
                t3_client::navigation::SettingsPage::SourceControl => {
                    crate::state::nav::SettingsPage::SourceControl
                }
                t3_client::navigation::SettingsPage::Secrets => {
                    crate::state::nav::SettingsPage::Secrets
                }
                t3_client::navigation::SettingsPage::ScheduledTasks => {
                    crate::state::nav::SettingsPage::ScheduledTasks
                }
                t3_client::navigation::SettingsPage::Browser => {
                    crate::state::nav::SettingsPage::Browser
                }
                t3_client::navigation::SettingsPage::Devices => {
                    crate::state::nav::SettingsPage::Devices
                }
                t3_client::navigation::SettingsPage::Connections => {
                    crate::state::nav::SettingsPage::Connections
                }
                t3_client::navigation::SettingsPage::Access => {
                    crate::state::nav::SettingsPage::Access
                }
                t3_client::navigation::SettingsPage::Voice => {
                    crate::state::nav::SettingsPage::Voice
                }
                t3_client::navigation::SettingsPage::Keybindings => {
                    crate::state::nav::SettingsPage::Keybindings
                }
                t3_client::navigation::SettingsPage::Archived => {
                    crate::state::nav::SettingsPage::Archived
                }
                t3_client::navigation::SettingsPage::Diagnostics => {
                    crate::state::nav::SettingsPage::Diagnostics
                }
                t3_client::navigation::SettingsPage::Maintenance => {
                    crate::state::nav::SettingsPage::Maintenance
                }
                t3_client::navigation::SettingsPage::Labs => crate::state::nav::SettingsPage::Labs,
                t3_client::navigation::SettingsPage::About => {
                    crate::state::nav::SettingsPage::About
                }
                t3_client::navigation::SettingsPage::Licenses => {
                    crate::state::nav::SettingsPage::Licenses
                }
                t3_client::navigation::SettingsPage::SnapShot => {
                    crate::state::nav::SettingsPage::SnapShot
                }
            })
        }
        t3_client::navigation::Route::ThreadPanel(key, panel) => {
            crate::state::nav::Route::ThreadPanel(
                key,
                match panel {
                    t3_client::navigation::ThreadPanel::Files => {
                        crate::state::nav::ThreadPanel::Files
                    }
                    t3_client::navigation::ThreadPanel::Diff => {
                        crate::state::nav::ThreadPanel::Diff
                    }
                    t3_client::navigation::ThreadPanel::Terminal => {
                        crate::state::nav::ThreadPanel::Terminal
                    }
                    t3_client::navigation::ThreadPanel::Browser => {
                        crate::state::nav::ThreadPanel::Browser
                    }
                    t3_client::navigation::ThreadPanel::Device => {
                        crate::state::nav::ThreadPanel::Device
                    }
                    t3_client::navigation::ThreadPanel::PullRequest => {
                        crate::state::nav::ThreadPanel::PullRequest
                    }
                },
            )
        }
    }
}
