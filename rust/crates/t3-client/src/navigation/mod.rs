//! Shared web/native/mobile route tree and history decisions.
use crate::types::{EnvironmentId, ThreadId, ThreadKey};
pub use crate::view_logic::shell::navigation::{
    Direction, HistoryAvailability, RouteTarget, encode_component, route_target,
};
use percent_encoding::{NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
pub mod link_targets;
#[cfg(test)]
mod link_targets_tests;
pub mod new_thread;
pub mod project_flow;
pub mod ignored_selection;
mod normalize;
pub mod prompt_links;
pub use normalize::normalize_app_path;
mod service;
pub use service::UrlClient;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SettingsPage {
    General,
    Appearance,
    Chat,
    Diffs,
    Threads,
    Worktrees,
    Projects,
    Providers,
    SourceControl,
    Secrets,
    ScheduledTasks,
    Browser,
    Devices,
    Connections,
    Access,
    Voice,
    Keybindings,
    Archived,
    Diagnostics,
    Maintenance,
    Labs,
    About,
    Licenses,
    SnapShot,
}

impl SettingsPage {
    pub const ALL: [SettingsPage; 24] = [
        Self::General,
        Self::Appearance,
        Self::Chat,
        Self::Diffs,
        Self::Threads,
        Self::Worktrees,
        Self::Projects,
        Self::Providers,
        Self::SourceControl,
        Self::Secrets,
        Self::ScheduledTasks,
        Self::Browser,
        Self::Devices,
        Self::Connections,
        Self::Access,
        Self::Voice,
        Self::Keybindings,
        Self::Archived,
        Self::Diagnostics,
        Self::Maintenance,
        Self::Labs,
        Self::About,
        Self::Licenses,
        Self::SnapShot,
    ];

    pub fn slug(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Appearance => "appearance",
            Self::Chat => "chat",
            Self::Diffs => "diffs",
            Self::Threads => "threads",
            Self::Worktrees => "worktrees",
            Self::Projects => "projects",
            Self::Providers => "providers",
            Self::SourceControl => "source-control",
            Self::Secrets => "secrets",
            Self::ScheduledTasks => "scheduled-tasks",
            Self::Browser => "browser",
            Self::Devices => "devices",
            Self::Connections => "connections",
            Self::Access => "access",
            Self::Voice => "voice",
            Self::Keybindings => "keybindings",
            Self::Archived => "archived",
            Self::Diagnostics => "diagnostics",
            Self::Maintenance => "maintenance",
            Self::Labs => "labs",
            Self::About => "about",
            Self::Licenses => "open-source-licenses",
            Self::SnapShot => "snap-shot",
        }
    }

    /// `RENAMED_SETTINGS_PATHS` of `settingsSearch.ts`; anchor-based moves belong to the settings
    /// search package.
    pub fn from_slug(slug: &str) -> Option<Self> {
        match slug {
            "integrations" => Some(Self::Browser),
            "storage" => Some(Self::Maintenance),
            "beta" => Some(Self::Labs),
            _ => Self::ALL.into_iter().find(|page| page.slug() == slug),
        }
    }
}

/// A page of a thread that the phone shows as its own screen and wider layouts show in a panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ThreadPanel {
    Files,
    Diff,
    Terminal,
    Browser,
    Device,
    PullRequest,
}

impl ThreadPanel {
    pub const ALL: [ThreadPanel; 6] = [
        Self::Files,
        Self::Diff,
        Self::Terminal,
        Self::Browser,
        Self::Device,
        Self::PullRequest,
    ];

    pub fn slug(self) -> &'static str {
        match self {
            Self::Files => "files",
            Self::Diff => "diff",
            Self::Terminal => "terminal",
            Self::Browser => "browser",
            Self::Device => "device",
            Self::PullRequest => "pull-request",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    /// The chat landing state; on a phone, the thread list.
    Home,
    NewThread,
    Draft(String),
    Thread(ThreadKey),
    ThreadPanel(ThreadKey, ThreadPanel),
    /// `/projects/$projectKey`: a project link, with pairing handled by its screen.
    ProjectLink(String),
    AddProject,
    PullRequests,
    Usage,
    Archived,
    Search,
    Settings(SettingsPage),
    Welcome,
    Pair,
    Connect,
    ConnectAgent,
    ApproveAgent,
    Guest,
    NotFound(String),
}

/// Supplied by the client's auth/config projection, never inferred from a hostname or platform.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
pub(crate) fn encode_search_string(value: &str) -> String {
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
    if is_app_scheme(scheme) {
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

pub(crate) fn is_app_scheme(scheme: &str) -> bool {
    let scheme = scheme.to_ascii_lowercase();
    scheme == "t3code"
        || scheme.strip_prefix("t3code-").is_some_and(|suffix| {
            !suffix.is_empty() && suffix.bytes().all(|ch| ch.is_ascii_lowercase())
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

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AppUrlRequest {
    Push(String),
    Replace(String),
    Go(i64),
    CloseModal(CloseModal),
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AppUrlSnapshot {
    pub path: String,
    pub history_index: u64,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub policy: RoutePolicy,
}

impl Route {
    pub fn parse(address: &str) -> Self {
        parse(address)
    }
    pub fn address(&self) -> Option<String> {
        path_for(self)
    }
    pub fn path(&self) -> String {
        self.address().unwrap_or_else(|| self.stack_key())
    }
    pub fn stack_key(&self) -> String {
        match self {
            Self::AddProject => "@add-project".into(),
            Self::Search => "@search".into(),
            Self::Archived => "@archived".into(),
            Self::ThreadPanel(key, panel) => format!(
                "@panel/{}/{}/{}",
                encode_component(&key.environment.0),
                encode_component(&key.thread.0),
                panel.slug()
            ),
            _ => path_for(self).unwrap_or_default(),
        }
    }
    /// The thread this route belongs to, if any: what the sidebar marks selected.
    pub fn thread(&self) -> Option<&ThreadKey> {
        match self {
            Self::Thread(key) | Self::ThreadPanel(key, _) => Some(key),
            _ => None,
        }
    }

    /// Routes shown without the workspace shell (no sidebar).
    pub fn is_standalone(&self) -> bool {
        matches!(
            self,
            Self::Welcome
                | Self::Pair
                | Self::Connect
                | Self::ConnectAgent
                | Self::ApproveAgent
                | Self::Guest
        )
    }
}

#[cfg(test)]
mod tests;

/// Extracts a claimed app path; invalid addresses remain invalid for Route::parse.
pub fn path_of(address: &str) -> String {
    app_path(address).unwrap_or_else(|| address.into())
}
pub fn initial_route(address: Option<&str>, paired: bool) -> Route {
    let route = address.map_or(Route::Home, |address| {
        Route::parse(&normalize_app_path(&path_of(address)))
    });
    if route.is_standalone() || paired {
        route
    } else {
        Route::Pair
    }
}
