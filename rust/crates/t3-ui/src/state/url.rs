//! Port of apps/web/src/router.ts, threadRoutes.ts, navigationHistory.ts and sidebar/mainAppLocation.ts.

use std::rc::Rc;

use futures::{StreamExt, channel::mpsc, future::LocalBoxFuture, stream::LocalBoxStream};
use nsh_core::prelude::*;

use super::nav::{Navigator, Route};
use crate::i18n::{Text, t, tr};
use crate::screens::{add_project, connect, new_thread, thread};
use crate::theme::tw;
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};

pub use t3_client::navigation::{
    AppUrlRequest as UrlRequest, AppUrlSnapshot as UrlSnapshot, CloseModal, Location,
    MainAppLocation, RoutePolicy, app_path, is_app_modal_page, is_sidebar_utility_page, parse,
    path_for,
};

/// Adapter for the client/platform URL service requested in ui-sh-routes.md. Its current-first
/// stream merges address changes and claimed native links; dropping it releases the listener.
pub trait UrlClient {
    fn watch(&self) -> LocalBoxStream<'static, UrlSnapshot>;
    fn navigate(&self, request: UrlRequest) -> LocalBoxFuture<'static, Result<(), Text>>;
    fn stream_error(&self) -> Option<Text> {
        None
    }
}

/// Root-owned navigation: tasks and the service lease end with App. Host history remains the
/// authority; accepting a warm link updates both location and the selected native route.
#[derive(Clone, Copy)]
pub struct UrlState {
    pub location: Signal<Location>,
    pub error: Signal<Option<Text>>,
    pub available: Signal<bool>,
    pub can_go_back: Signal<bool>,
    pub can_go_forward: Signal<bool>,
    history_index: Signal<u64>,
    history_entry: Signal<Option<String>>,
    tracker: Stored<MainAppLocation>,
    requests: Stored<mpsc::UnboundedSender<UrlRequest>>,
}

impl UrlState {
    pub fn install(location: Location, navigator: Navigator, client: Rc<dyn UrlClient>) -> Self {
        let (requests, mut receiver) = mpsc::unbounded();
        let state = Self {
            location: signal(location),
            error: signal(None),
            available: signal(true),
            can_go_back: signal(false),
            can_go_forward: signal(false),
            history_index: signal(0),
            history_entry: signal(None),
            tracker: stored(MainAppLocation::default()),
            requests: stored(requests),
        };
        let mut changes = client.watch();
        let observer = client.clone();
        crate::runtime::tasks::watch(async move {
            while let Some(request) = receiver.next().await {
                if !state.available.get_untracked() {
                    continue;
                }
                state.error.set(None);
                let requested_route = match &request {
                    UrlRequest::Push(path) | UrlRequest::Replace(path) => Some(parse(path)),
                    _ => None,
                };
                if let Err(error) = client.navigate(request).await {
                    state.error.set(Some(error));
                    if requested_route.as_ref() == Some(&navigator.top()) {
                        let location = state.location.get_untracked();
                        state.accept(location, navigator);
                    }
                }
            }
        });
        crate::runtime::tasks::watch(async move {
            while let Some(change) = changes.next().await {
                let location = Location::resolve(&change.path, change.policy);
                if location.href() != change.path {
                    state.request(UrlRequest::Replace(location.href()));
                    continue;
                }
                state
                    .tracker
                    .with(|tracker| tracker.record(&location, change.history_index));
                batch(|| {
                    state.history_entry.set(Some(change.entry_identity()));
                    state.history_index.set(change.history_index);
                    state.can_go_back.set(change.can_go_back);
                    state.can_go_forward.set(change.can_go_forward);
                    state.accept(location, navigator);
                });
            }
            batch(|| {
                if state.history_entry.get_untracked().is_none() {
                    state
                        .history_entry
                        .set(Some(t3_client::navigation::history_entry_identity(
                            None,
                            state.history_index.get_untracked(),
                        )));
                }
                state.available.set(false);
                state.can_go_back.set(false);
                state.can_go_forward.set(false);
                state
                    .error
                    .set(Some(observer.stream_error().unwrap_or_else(|| {
                        Text::client_key(t3_client::platform::PlatformError::Closed.to_string())
                    })));
            });
        });
        effect(move || {
            let route = navigator.top();
            let resolved = state.location.with(|location| location.route.clone());
            let available = state.available.get();
            if route != resolved
                && let Some(path) = path_for(&route)
            {
                if available {
                    state.request(UrlRequest::Push(path));
                } else {
                    navigator.open(resolved);
                }
            }
        });
        provide_context(state);
        state
    }

    pub fn entry_identity(self) -> Option<String> {
        self.history_entry.get()
    }

    pub fn request(self, request: UrlRequest) {
        if !self.available.get_untracked() {
            return;
        }
        if self
            .requests
            .with(|sender| sender.unbounded_send(request))
            .is_err()
        {
            self.error.set(Some(Text::client_key(
                t3_client::platform::PlatformError::Closed.to_string(),
            )));
        }
    }

    pub fn go(self, direction: t3_client::view_logic::shell::navigation::Direction) -> bool {
        use t3_client::view_logic::shell::navigation::{Direction, HistoryAvailability};
        let availability = HistoryAvailability {
            back: self.can_go_back.get_untracked(),
            forward: self.can_go_forward.get_untracked(),
        };
        if !availability.can_navigate(direction) {
            return false;
        }
        self.request(UrlRequest::Go(match direction {
            Direction::Back => -1,
            Direction::Forward => 1,
        }));
        true
    }

    pub fn close_modal(self) {
        let request = self
            .tracker
            .with(|tracker| tracker.close_modal(self.history_index.get_untracked()));
        self.request(UrlRequest::CloseModal(request));
    }

    pub fn navigate_to_main_app(self) {
        let href = self.tracker.with(|tracker| tracker.main_href().to_owned());
        self.request(UrlRequest::Push(href));
    }

    pub fn accept(self, location: Location, navigator: Navigator) {
        batch(|| {
            if navigator.top() != location.route {
                navigator.open(location.route.clone());
            }
            self.location.set(location);
        });
    }
}

/// The route registration seam for app.rs/router.rs. Unbuilt destinations use the web root's
/// not-found view, rather than fabricated feature placeholders. Add Project is an in-app flow.
pub fn screen(route: &Route) -> AnyElement {
    untracked(|| match route {
        Route::Home => new_thread::screen(None),
        Route::Draft(id) => new_thread::screen(Some(id)),
        Route::NewThread => {
            let urls = use_context::<UrlState>().expect("App provides the URL service");
            let search = computed(move || urls.location.with(|location| location.search.clone()));
            dynamic(move || {
                let search = search.get();
                untracked(|| new_thread::new_thread_link_route(&search))
            })
            .into_element()
        }
        Route::AddProject => add_project::screen(),
        Route::Thread(key) => thread::screen(key.clone()),
        Route::Pair => connect::screen(),
        _ => root_route_not_found_view().into_element(),
    })
}

/// Port of apps/web/src/routes/__root.tsx RootRouteNotFoundView.
pub fn root_route_not_found_view() -> impl IntoElement {
    let urls = use_context::<UrlState>().expect("App provides the URL service");
    // NSH has no Main landmark role; Header is its heading role.
    view()
        .app_class(tw("flex min-h-0 min-w-0 flex-1 items-center justify-center p-6"))
        .child(view().app_class(tw("flex max-w-sm flex-col items-center gap-4 text-center"))
            .child(text(move || t("Page not found")).role(AccessibilityRole::Header).app_class(tw("text-lg font-medium text-foreground")))
            .child(text(move || tr("This link doesn't point to a page in {appName}. Go home to choose a project or start a thread.", &[("appName", t3_i18n::Value::Text("T3 Code".into()))])).app_class(tw("text-sm text-muted-foreground")))
            // A closed URL service disables navigation; its client error is exposed on UrlState.
            .child(dynamic(move || button(ButtonContent::Label(t("Go home")), ButtonVariant::Default, ButtonSize::Default, move || !urls.available.get(), move || urls.request(UrlRequest::Replace("/".into()))).into_element())))
}
