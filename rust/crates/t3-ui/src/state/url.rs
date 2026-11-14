//! Port of apps/web/src/router.ts, threadRoutes.ts, navigationHistory.ts and sidebar/mainAppLocation.ts.

use std::rc::Rc;

use futures::{StreamExt, channel::mpsc, future::LocalBoxFuture, stream::LocalBoxStream};
use nsh_core::prelude::*;
use percent_encoding::{NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use t3_client::types::{EnvironmentId, ThreadId, ThreadKey};
use t3_client::view_logic::shell::navigation::{RouteTarget, encode_component, route_target};

use super::nav::{Navigator, Route, SettingsPage};
use crate::i18n::{Text, t, tr};
use crate::screens::{add_project, connect, new_thread, thread};
use crate::theme::tw;
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};

/// Supplied by the client's auth/config projection, never inferred from a hostname or platform.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RoutePolicy {
    pub authenticated_or_hosted: bool,
    pub secrets_enabled: bool,
    pub connect_cli_enabled: bool,
    pub approve_agent_enabled: bool,
}

/// Raw search and hash belong to the location, rather than the route identity. In particular,
/// `/new?prompt=123` must reach the prompt-link client without JSON coercion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub route: Route,
    pub pathname: String,
    pub search: String,
    pub hash: String,
}

impl Location {
    pub fn parse(path: &str) -> Self {
        let (before_hash, hash) = path.split_once('#').unwrap_or((path, ""));
        let (pathname, search) = before_hash.split_once('?').unwrap_or((before_hash, ""));
        Self {
            route: parse(pathname),
            pathname: pathname.into(),
            search: if before_hash.contains('?') {
                format!("?{search}")
            } else {
                String::new()
            },
            hash: if path.contains('#') {
                format!("#{hash}")
            } else {
                String::new()
            },
        }
    }

    pub fn href(&self) -> String {
        format!("{}{}{}", self.pathname, self.search, self.hash)
    }

    pub fn target(&self) -> Option<RouteTarget> {
        match &self.route {
            Route::Thread(key) => route_target(Some(&key.environment.0), Some(&key.thread.0), None),
            Route::Draft(id) => route_target(None, None, Some(id)),
            _ => None,
        }
    }

    /// `beforeLoad` redirects replace history; callers compare the returned href to the input.
    pub fn resolve(path: &str, policy: RoutePolicy) -> Self {
        let mut location = Self::parse(path);
        let pathname = location.pathname.trim_end_matches('/');
        let redirect = match &location.route {
            Route::Settings(SettingsPage::Secrets) if !policy.secrets_enabled => {
                return Self::parse("/settings/general");
            }
            Route::Settings(page) if pathname == "/settings" => {
                return Self::parse(&format!("/settings/{}", page.slug()));
            }
            Route::Settings(page)
                if matches!(
                    pathname,
                    "/settings/integrations" | "/settings/storage" | "/settings/beta"
                ) =>
            {
                Some(format!("/settings/{}", page.slug()))
            }
            Route::Thread(key)
                if pathname
                    .get(..9)
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case("/threads/")) =>
            {
                path_for(&Route::Thread(key.clone()))
            }
            Route::ProjectLink(key) => {
                return if policy.authenticated_or_hosted {
                    Self::parse(&format!(
                        "/settings/projects?project={}",
                        encode_search_string(key)
                    ))
                } else {
                    Self::parse("/pair")
                };
            }
            Route::Connect if !policy.connect_cli_enabled => return Self::parse("/"),
            Route::ApproveAgent if !policy.approve_agent_enabled => return Self::parse("/"),
            _ => None,
        };
        if let Some(pathname) = redirect {
            location.pathname = pathname;
        }
        location
    }
}

fn decode_segment(segment: &str) -> Option<String> {
    let bytes = segment.as_bytes();
    if !bytes.iter().enumerate().all(|(index, byte)| {
        *byte != b'%'
            || bytes
                .get(index + 1..index + 3)
                .is_some_and(|hex| hex.iter().all(u8::is_ascii_hexdigit))
    }) {
        return None;
    }
    percent_decode_str(segment)
        .decode_utf8()
        .ok()
        .map(|value| value.into_owned())
}

/// The web route tree, including its legacy thread route and settings aliases. UI actions such
/// as Add Project have no address: `/projects/new` is a project key, not an add-project page.
pub fn parse(address: &str) -> Route {
    let pathname = address.split(['?', '#']).next().unwrap_or("");
    if pathname.starts_with("//") {
        return Route::NotFound(pathname.into());
    }
    let path = pathname
        .strip_suffix('/')
        .filter(|path| !path.is_empty())
        .unwrap_or(pathname);
    let Some(path) = path.strip_prefix('/') else {
        return Route::NotFound(pathname.into());
    };
    let Some(parts) = path
        .split('/')
        .map(decode_segment)
        .collect::<Option<Vec<_>>>()
    else {
        return Route::NotFound(pathname.into());
    };
    let parts = parts.iter().map(String::as_str).collect::<Vec<_>>();
    let key = |environment: &str, thread: &str| ThreadKey {
        environment: EnvironmentId(environment.into()),
        thread: ThreadId(thread.into()),
    };
    match parts.as_slice() {
        [page] => match page.to_ascii_lowercase().as_str() {
            "" => Route::Home,
            "new" => Route::NewThread,
            "settings" => Route::Settings(SettingsPage::General),
            "pull-requests" => Route::PullRequests,
            "usage" => Route::Usage,
            "welcome" => Route::Welcome,
            "pair" => Route::Pair,
            "connect" => Route::Connect,
            "connect-agent" => Route::ConnectAgent,
            "approve-agent" => Route::ApproveAgent,
            "guest" => Route::Guest,
            _ => Route::NotFound(pathname.into()),
        },
        [head, id] if head.eq_ignore_ascii_case("draft") && !id.is_empty() => {
            Route::Draft((*id).into())
        }
        [head, id] if head.eq_ignore_ascii_case("projects") && !id.is_empty() => {
            Route::ProjectLink((*id).into())
        }
        [head, slug] if head.eq_ignore_ascii_case("settings") && !slug.is_empty() => {
            SettingsPage::from_slug(&slug.to_ascii_lowercase())
                .map_or_else(|| Route::Thread(key(head, slug)), Route::Settings)
        }
        [head, environment, id]
            if head.eq_ignore_ascii_case("threads")
                && !environment.is_empty()
                && !id.is_empty() =>
        {
            Route::Thread(key(environment, id))
        }
        [environment, id] if !environment.is_empty() && !id.is_empty() => {
            Route::Thread(key(environment, id))
        }
        _ => Route::NotFound(pathname.into()),
    }
}

/// Addresses only for web routes. Panels, palette search and Add Project are UI state, not new
/// URL paths. Native navigation uses the same table rather than introducing link aliases.
pub fn path_for(route: &Route) -> Option<String> {
    Some(match route {
        Route::Home => "/".into(),
        Route::NewThread => "/new".into(),
        Route::Draft(id) => format!("/draft/{}", encode_component(id)),
        Route::Thread(key) => format!(
            "/{}/{}",
            encode_component(&key.environment.0),
            encode_component(&key.thread.0)
        ),
        Route::ProjectLink(key) => format!("/projects/{}", encode_component(key)),
        Route::Settings(page) => format!("/settings/{}", page.slug()),
        Route::PullRequests => "/pull-requests".into(),
        Route::Usage => "/usage".into(),
        Route::Welcome => "/welcome".into(),
        Route::Pair => "/pair".into(),
        Route::Connect => "/connect".into(),
        Route::ConnectAgent => "/connect-agent".into(),
        Route::ApproveAgent => "/approve-agent".into(),
        Route::Guest => "/guest".into(),
        Route::NotFound(path) => path.clone(),
        Route::AddProject | Route::Search | Route::Archived | Route::ThreadPanel(_, _) => {
            return None;
        }
    })
}

// TanStack stringifySearchWith JSON-quotes strings that JSON.parse would otherwise coerce.
fn encode_search_string(value: &str) -> String {
    // RawValue accepts JSON numbers outside f64's range, just as JSON.parse does.
    let value = if serde_json::from_str::<Box<serde_json::value::RawValue>>(value).is_ok() {
        serde_json::Value::String(value.into()).to_string()
    } else {
        value.to_owned()
    };
    const FORM: &percent_encoding::AsciiSet = &NON_ALPHANUMERIC
        .remove(b'*')
        .remove(b'-')
        .remove(b'.')
        .remove(b'_');
    utf8_percent_encode(&value, FORM)
        .to_string()
        .replace("%20", "+")
}

/// The client/platform validates origins and claimed schemes before supplying an app path.
/// Do not strip arbitrary schemes here: e.g. `javascript://settings` is not an app link.
pub fn app_path(address: &str) -> Option<String> {
    if address.starts_with('/') && !address.starts_with("//") {
        return Some(address.into());
    }
    let (scheme, rest) = address.split_once("://")?;
    if scheme.eq_ignore_ascii_case("t3code") {
        return (!rest.starts_with('/')).then(|| format!("/{rest}"));
    }
    if !scheme.eq_ignore_ascii_case("https") && !scheme.eq_ignore_ascii_case("http") {
        return None;
    }
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    if end == 0 {
        return None;
    }
    let tail = &rest[end..];
    Some(if tail.starts_with('/') {
        tail.into()
    } else {
        format!("/{tail}")
    })
}

pub fn is_app_modal_page(pathname: &str) -> bool {
    pathname == "/settings" || pathname.starts_with("/settings/") || pathname == "/usage"
}

pub fn is_sidebar_utility_page(pathname: &str) -> bool {
    pathname.starts_with("/projects/") || pathname == "/pull-requests"
}

fn without_assistant_citation_hash(href: &str) -> &str {
    match href.find('#') {
        Some(index) if href[index + 1..].starts_with("assistant-citation=") => &href[..index],
        _ => href,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CloseModal {
    Push(String),
    /// The host waits for the traversal to resolve, then pushes `href`, pruning forward entries.
    GoThenPush {
        delta: i64,
        href: String,
    },
}

/// Port of sidebar/mainAppLocation.ts's module state. History indices are supplied by the host;
/// the native route stack is not a substitute for browser back/forward history.
#[derive(Clone, Debug, Default)]
pub struct MainAppLocation {
    main_href: Option<String>,
    modal_return_href: Option<String>,
    modal_return_index: Option<u64>,
    modal_return_target: Option<RouteTarget>,
}

impl MainAppLocation {
    pub fn record(&mut self, location: &Location, history_index: u64) {
        if is_app_modal_page(&location.pathname)
            || matches!(
                location.pathname.as_str(),
                "/pair" | "/connect" | "/connect-agent" | "/guest"
            )
        {
            return;
        }
        let href = location.href();
        let return_href = without_assistant_citation_hash(&href);
        self.modal_return_href = Some(return_href.into());
        self.modal_return_index = (return_href == href).then_some(history_index);
        self.modal_return_target = location.target();
        if !is_sidebar_utility_page(&location.pathname) {
            self.main_href = Some(return_href.into());
        }
    }

    pub fn main_href(&self) -> &str {
        self.main_href.as_deref().unwrap_or("/")
    }

    pub fn modal_return_target(&self) -> Option<&RouteTarget> {
        self.modal_return_target.as_ref()
    }

    pub fn close_modal(&self, history_index: u64) -> CloseModal {
        let href = self
            .modal_return_href
            .as_deref()
            .unwrap_or(self.main_href())
            .to_owned();
        let index = self.modal_return_index.unwrap_or(0);
        match (index, history_index.checked_sub(index)) {
            (1.., Some(steps)) if steps > 0 => i64::try_from(steps)
                .ok()
                .and_then(|steps| steps.checked_add(1))
                .map_or_else(
                    || CloseModal::Push(href.clone()),
                    |steps| CloseModal::GoThenPush {
                        delta: -steps,
                        href: href.clone(),
                    },
                ),
            _ => CloseModal::Push(href),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UrlRequest {
    Push(String),
    Replace(String),
    Go(i64),
    CloseModal(CloseModal),
}

#[derive(Clone, Debug)]
pub struct UrlSnapshot {
    pub path: String,
    pub history_index: u64,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub policy: RoutePolicy,
}

/// Adapter for the client/platform URL service requested in ui-sh-routes.md. Its current-first
/// stream merges address changes and claimed native links; dropping it releases the listener.
pub trait UrlClient {
    fn watch(&self) -> LocalBoxStream<'static, UrlSnapshot>;
    fn navigate(&self, request: UrlRequest) -> LocalBoxFuture<'static, Result<(), Text>>;
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
            tracker: stored(MainAppLocation::default()),
            requests: stored(requests),
        };
        let mut changes = client.watch();
        spawn(async move {
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
        spawn(async move {
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
                    state.history_index.set(change.history_index);
                    state.can_go_back.set(change.can_go_back);
                    state.can_go_forward.set(change.can_go_forward);
                    state.accept(location, navigator);
                });
            }
            batch(|| {
                state.available.set(false);
                state.can_go_back.set(false);
                state.can_go_forward.set(false);
                state.error.set(Some(Text::client_key(
                    t3_client::platform::PlatformError::Closed.to_string(),
                )));
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
            dynamic(move || {
                let search = urls.location.with(|location| location.search.clone());
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

#[cfg(test)]
mod tests {
    use super::*;
    use t3_client::view_logic::shell::navigation::is_thread_path;

    fn key(environment: &str, thread: &str) -> ThreadKey {
        ThreadKey {
            environment: EnvironmentId(environment.into()),
            thread: ThreadId(thread.into()),
        }
    }

    #[test]
    fn route_parameters_follow_the_webs_component_encoding() {
        let thread = key("env space", "mcp:example/part");
        let path = path_for(&Route::Thread(thread.clone())).expect("addressed route");
        assert_eq!(path, "/env%20space/mcp%3Aexample%2Fpart");
        assert_eq!(parse(&path), Route::Thread(thread.clone()));
        assert!(is_thread_path(&path, &thread));
        assert!(is_thread_path(&format!("{path}/"), &thread));
        assert!(!is_thread_path(
            "/env%20space/mcp%253Aexample%2Fpart",
            &thread
        ));
        assert_eq!(
            parse("/env%20space/mcp%253Aexample"),
            Route::Thread(key("env space", "mcp%3Aexample"))
        );
        for path in [
            "/e/%",
            "/e/%4",
            "/e/%ZZ",
            "/%C0%AF/t",
            "/e/t/extra",
            "//t",
            "e/t",
        ] {
            assert_eq!(parse(path), Route::NotFound(path.into()), "{path}");
        }
        assert_eq!(path_for(&Route::AddProject), None);
        assert_eq!(parse("/projects/new"), Route::ProjectLink("new".into()));
        assert_eq!(
            parse("/settings/nope"),
            Route::Thread(key("settings", "nope"))
        );
        assert_eq!(
            parse("/SETTINGS/GENERAL"),
            Route::Settings(SettingsPage::General)
        );
        assert_eq!(parse("/DRAFT/KeepCase"), Route::Draft("KeepCase".into()));
        assert_eq!(parse("/threads/e"), Route::Thread(key("threads", "e")));
        assert_eq!(parse("/archived"), Route::NotFound("/archived".into()));
    }

    #[test]
    fn every_addressed_route_has_the_same_parameters_after_navigation() {
        let mut routes = vec![
            Route::Home,
            Route::NewThread,
            Route::Draft("123 ?#/%".into()),
            Route::Thread(key("env", "mcp:x")),
            Route::ProjectLink("[\"env\",\"/repo\"]".into()),
            Route::PullRequests,
            Route::Usage,
            Route::Welcome,
            Route::Pair,
            Route::Connect,
            Route::ConnectAgent,
            Route::ApproveAgent,
            Route::Guest,
        ];
        routes.extend(SettingsPage::ALL.into_iter().map(Route::Settings));
        for route in routes {
            let path = path_for(&route).expect("web route");
            assert_eq!(parse(&path), route, "{path}");
        }
    }

    #[test]
    fn redirects_match_the_captured_settings_aliases_and_preserve_legacy_thread_search() {
        let policy = RoutePolicy::default();
        for (from, to) in [
            ("/settings", "/settings/general"),
            ("/settings/beta", "/settings/labs"),
            ("/settings/storage", "/settings/maintenance"),
            ("/settings/integrations", "/settings/browser"),
            ("/settings/secrets", "/settings/general"),
        ] {
            assert_eq!(Location::resolve(from, policy).href(), to);
            assert_eq!(Location::resolve(to, policy).href(), to, "redirect settles");
        }
        assert_eq!(
            Location::resolve("/settings/beta?machine=env#section", policy).href(),
            "/settings/labs?machine=env#section"
        );
        assert_eq!(
            Location::resolve("/threads/e/mcp%3Ax?diff=1#source", policy).href(),
            "/e/mcp%3Ax?diff=1#source"
        );
        assert_eq!(Location::resolve("/connect", policy).route, Route::Home);
        assert_eq!(
            Location::resolve("/approve-agent", policy).route,
            Route::Home
        );
        let enabled = RoutePolicy {
            authenticated_or_hosted: true,
            secrets_enabled: true,
            connect_cli_enabled: true,
            approve_agent_enabled: true,
        };
        for path in ["/settings/secrets", "/connect", "/approve-agent"] {
            assert_eq!(Location::resolve(path, enabled).href(), path);
        }
    }

    #[test]
    fn settings_paths_match_the_reference_manifests_resolved_paths() {
        use SettingsPage::*;
        for (page, expected) in [
            (General, "/settings/general"),
            (Appearance, "/settings/appearance"),
            (Chat, "/settings/chat"),
            (Diffs, "/settings/diffs"),
            (Threads, "/settings/threads"),
            (Worktrees, "/settings/worktrees"),
            (Projects, "/settings/projects"),
            (Providers, "/settings/providers"),
            (SourceControl, "/settings/source-control"),
            (Secrets, "/settings/secrets"),
            (ScheduledTasks, "/settings/scheduled-tasks"),
            (Browser, "/settings/browser"),
            (Devices, "/settings/devices"),
            (Connections, "/settings/connections"),
            (Access, "/settings/access"),
            (Voice, "/settings/voice"),
            (Keybindings, "/settings/keybindings"),
            (Archived, "/settings/archived"),
            (Diagnostics, "/settings/diagnostics"),
            (Maintenance, "/settings/maintenance"),
            (Labs, "/settings/labs"),
            (About, "/settings/about"),
            (Licenses, "/settings/open-source-licenses"),
            (SnapShot, "/settings/snap-shot"),
        ] {
            assert_eq!(path_for(&Route::Settings(page)).as_deref(), Some(expected));
            assert_eq!(parse(expected), Route::Settings(page));
        }
    }

    #[test]
    fn project_links_redirect_with_a_string_project_key_and_never_open_add_project() {
        assert_eq!(
            Location::resolve("/projects/new", RoutePolicy::default()).href(),
            "/pair"
        );
        let policy = RoutePolicy {
            authenticated_or_hosted: true,
            ..Default::default()
        };
        for (path, expected) in [
            ("/projects/new", "/settings/projects?project=new"),
            ("/projects/123", "/settings/projects?project=%22123%22"),
            (
                "/projects/env%3Apath%20with%20space",
                "/settings/projects?project=env%3Apath+with+space",
            ),
            (
                "/projects/%5B1e999%5D",
                "/settings/projects?project=%22%5B1e999%5D%22",
            ),
            (
                "/projects/!~'()*",
                "/settings/projects?project=%21%7E%27%28%29*",
            ),
        ] {
            let location = Location::resolve(path, policy);
            assert_eq!(location.href(), expected);
            assert_eq!(location.route, Route::Settings(SettingsPage::Projects));
        }
    }

    #[test]
    fn launch_paths_preserve_raw_queries_and_pairing_fragments() {
        for (url, expected) in [
            (
                "t3code://new?prompt=123&prompt=x%2By#source",
                "/new?prompt=123&prompt=x%2By#source",
            ),
            (
                "https://app.t3.codes/pair#token=abc&host=https%3A%2F%2Fh",
                "/pair#token=abc&host=https%3A%2F%2Fh",
            ),
            ("HTTPS://h.test?x=1", "/?x=1"),
            ("t3code://settings/chat", "/settings/chat"),
        ] {
            let path = app_path(url).expect("claimed app address");
            assert_eq!(path, expected);
            assert_eq!(Location::parse(&path).href(), expected);
        }
        let link = Location::parse("/new?prompt=123&prompt=x%2By#?ignored");
        assert_eq!(link.search, "?prompt=123&prompt=x%2By");
        assert_eq!(link.hash, "#?ignored");
        for url in [
            "javascript://settings",
            "file://settings",
            "//evil.test/pair",
            "t3code:///pair",
            "https:///pair",
        ] {
            assert_eq!(app_path(url), None);
        }
    }

    #[test]
    fn modal_and_sidebar_returns_drop_only_the_citation_hash() {
        let mut tracker = MainAppLocation::default();
        let citation = Location::parse("/e/t?diff=1#assistant-citation=encoded");
        tracker.record(&citation, 1);
        tracker.record(&Location::parse("/usage"), 2);
        assert_eq!(
            tracker.close_modal(2),
            CloseModal::Push("/e/t?diff=1".into())
        );
        assert_eq!(
            tracker.modal_return_target(),
            Some(&RouteTarget::Server(key("e", "t")))
        );
        tracker.record(&Location::parse("/pull-requests?state=open"), 3);
        assert_eq!(tracker.main_href(), "/e/t?diff=1");
        assert_eq!(
            tracker.close_modal(4),
            CloseModal::GoThenPush {
                delta: -2,
                href: "/pull-requests?state=open".into()
            }
        );
        tracker.record(&Location::parse("/draft/saved#normal-anchor"), 5);
        tracker.record(&Location::parse("/pair#token=private"), 6);
        tracker.record(&Location::parse("/guest#token=private"), 7);
        assert_eq!(tracker.main_href(), "/draft/saved#normal-anchor");
        assert_eq!(
            tracker.modal_return_target(),
            Some(&RouteTarget::Draft("saved".into()))
        );
    }

    #[test]
    fn closing_settings_prunes_its_back_and_forward_stops_without_duplicating_the_thread() {
        let mut tracker = MainAppLocation::default();
        let mut history = vec![
            "/e/one".to_owned(),
            "/e/two".to_owned(),
            "/settings/general".to_owned(),
            "/settings/chat".to_owned(),
        ];
        tracker.record(&Location::parse(&history[0]), 0);
        tracker.record(&Location::parse(&history[1]), 1);
        tracker.record(&Location::parse(&history[2]), 2);
        tracker.record(&Location::parse(&history[3]), 3);
        let CloseModal::GoThenPush { delta, href } = tracker.close_modal(3) else {
            panic!("must traverse before pushing");
        };
        assert_eq!(delta, -3);
        let before_return = (3_i64 + delta) as usize;
        history.truncate(before_return + 1);
        history.push(href);
        assert_eq!(history, ["/e/one", "/e/two"]);
        assert_eq!(tracker.close_modal(1), CloseModal::Push("/e/two".into()));
        assert_eq!(
            MainAppLocation::default().close_modal(0),
            CloseModal::Push("/".into())
        );
    }
}
