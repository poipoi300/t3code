//! Routes: every screen of the product, its address, and the stack that shows it.
//!
//! One `Route` value drives every layout: a wide window shows the sidebar beside the top route, a
//! phone shows the stack as pages. The same paths as the web app, so links keep working.

use nsh_core::prelude::*;
use percent_encoding::{AsciiSet, CONTROLS, percent_decode_str, utf8_percent_encode};
use t3_client::types::{EnvironmentId, ThreadId, ThreadKey};

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

const SEGMENT: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'/')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'`')
    .add(b'{')
    .add(b'}');

/// `decodeURIComponent`: a `%` not followed by two hex digits, or bytes that are not UTF-8, fail.
fn decode_segment(segment: &str) -> Option<String> {
    let bytes = segment.as_bytes();
    let well_formed = bytes.iter().enumerate().all(|(index, byte)| {
        *byte != b'%'
            || bytes
                .get(index + 1..index + 3)
                .is_some_and(|hex| hex.iter().all(u8::is_ascii_hexdigit))
    });
    if !well_formed {
        return None;
    }
    percent_decode_str(segment)
        .decode_utf8()
        .ok()
        .map(|decoded| decoded.into_owned())
}

fn encode(segment: &str) -> String {
    utf8_percent_encode(segment, SEGMENT).to_string()
}

impl Route {
    /// The address of the route, as the web app writes it.
    pub fn path(&self) -> String {
        match self {
            Self::Home => "/".into(),
            Self::NewThread => "/new".into(),
            Self::Draft(id) => format!("/draft/{}", encode(id)),
            Self::Thread(key) => {
                format!("/{}/{}", encode(&key.environment.0), encode(&key.thread.0))
            }
            Self::ThreadPanel(key, panel) => format!(
                "/{}/{}/{}",
                encode(&key.environment.0),
                encode(&key.thread.0),
                panel.slug()
            ),
            Self::ProjectLink(key) => format!("/projects/{}", encode(key)),
            Self::AddProject => "/projects/new".into(),
            Self::PullRequests => "/pull-requests".into(),
            Self::Usage => "/usage".into(),
            Self::Archived => "/archived".into(),
            Self::Search => "/search".into(),
            Self::Settings(page) => format!("/settings/{}", page.slug()),
            Self::Welcome => "/welcome".into(),
            Self::Pair => "/pair".into(),
            Self::Connect => "/connect".into(),
            Self::ConnectAgent => "/connect-agent".into(),
            Self::ApproveAgent => "/approve-agent".into(),
            Self::Guest => "/guest".into(),
            Self::NotFound(path) => path.clone(),
        }
    }

    /// The route at `address` (a path, optionally with `?query` or `#hash`, which routes ignore:
    /// screens read their own query). Invalid percent-encoding is not found, never a panic.
    pub fn parse(address: &str) -> Self {
        let path = address.split(['?', '#']).next().unwrap_or("");
        let trimmed = path
            .strip_suffix('/')
            .filter(|p| !p.is_empty())
            .unwrap_or(path);
        let raw: Vec<&str> = trimmed
            .strip_prefix('/')
            .unwrap_or(trimmed)
            .split('/')
            .collect();
        let decoded: Option<Vec<String>> = raw.iter().map(|s| decode_segment(s)).collect();
        let Some(segments) = decoded else {
            return Self::NotFound(path.into());
        };
        let s: Vec<&str> = segments.iter().map(String::as_str).collect();
        let thread = |environment: &str, thread: &str| ThreadKey {
            environment: EnvironmentId(environment.into()),
            thread: ThreadId(thread.into()),
        };
        match s.as_slice() {
            [""] => Self::Home,
            ["new"] => Self::NewThread,
            ["draft", id] if !id.is_empty() => Self::Draft((*id).into()),
            ["projects", "new"] => Self::AddProject,
            ["projects", key] if !key.is_empty() => Self::ProjectLink((*key).into()),
            ["pull-requests"] => Self::PullRequests,
            ["usage"] => Self::Usage,
            ["archived"] => Self::Archived,
            ["search"] => Self::Search,
            ["settings"] => Self::Settings(SettingsPage::General),
            ["settings", slug] => SettingsPage::from_slug(slug)
                .map_or_else(|| Self::NotFound(path.into()), Self::Settings),
            ["welcome"] => Self::Welcome,
            ["pair"] => Self::Pair,
            ["connect"] => Self::Connect,
            ["connect-agent"] => Self::ConnectAgent,
            ["approve-agent"] => Self::ApproveAgent,
            ["guest"] => Self::Guest,
            ["threads", environment, id] if !environment.is_empty() && !id.is_empty() => {
                Self::Thread(thread(environment, id))
            }
            [environment, id] if !environment.is_empty() && !id.is_empty() => {
                Self::Thread(thread(environment, id))
            }
            [environment, id, panel] if !environment.is_empty() && !id.is_empty() => {
                ThreadPanel::ALL
                    .into_iter()
                    .find(|p| p.slug() == *panel)
                    .map_or_else(
                        || Self::NotFound(path.into()),
                        |p| Self::ThreadPanel(thread(environment, id), p),
                    )
            }
            _ => Self::NotFound(path.into()),
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

/// The path (with query and hash) of a launch address: a page URL, a `t3code://` link or a path.
pub fn path_of(address: &str) -> String {
    let lower = address.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        let rest = &address[address.find("://").map_or(0, |i| i + 3)..];
        return rest.find(['/', '?', '#']).map_or_else(
            || "/".to_owned(),
            |i| {
                let tail = &rest[i..];
                if tail.starts_with('/') {
                    tail.to_owned()
                } else {
                    format!("/{tail}")
                }
            },
        );
    }
    match address.find("://") {
        Some(i) => format!("/{}", &address[i + 3..]),
        None if address.starts_with('/') => address.to_owned(),
        None => format!("/{address}"),
    }
}

/// Where the app opens: the launch address's route when it can be shown, else Home, and the
/// pairing screen while no environment is paired (an explicit `/pair` always shows it).
pub fn initial_route(address: Option<&str>, paired: bool) -> Route {
    let route = address.map_or(Route::Home, |address| Route::parse(&path_of(address)));
    if route.is_standalone() || paired {
        route
    } else {
        Route::Pair
    }
}

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

    fn key(environment: &str, thread: &str) -> ThreadKey {
        ThreadKey {
            environment: EnvironmentId(environment.into()),
            thread: ThreadId(thread.into()),
        }
    }

    #[test]
    fn every_route_round_trips_through_its_path() {
        let mut routes = vec![
            Route::Home,
            Route::NewThread,
            Route::Draft("d 1".into()),
            Route::Thread(key("env/a", "thread?1#x")),
            Route::ProjectLink("[\"env\",\"/repo\"]".into()),
            Route::AddProject,
            Route::PullRequests,
            Route::Usage,
            Route::Archived,
            Route::Search,
            Route::Welcome,
            Route::Pair,
            Route::Connect,
            Route::ConnectAgent,
            Route::ApproveAgent,
            Route::Guest,
        ];
        routes.extend(SettingsPage::ALL.into_iter().map(Route::Settings));
        routes.extend(
            ThreadPanel::ALL
                .into_iter()
                .map(|p| Route::ThreadPanel(key("e", "t"), p)),
        );
        for route in routes {
            assert_eq!(Route::parse(&route.path()), route, "{}", route.path());
        }
    }

    #[test]
    fn legacy_and_renamed_addresses_land_where_the_web_app_sends_them() {
        assert_eq!(Route::parse("/threads/e/t"), Route::Thread(key("e", "t")));
        assert_eq!(
            Route::parse("/settings"),
            Route::Settings(SettingsPage::General)
        );
        assert_eq!(
            Route::parse("/settings/beta"),
            Route::Settings(SettingsPage::Labs)
        );
        assert_eq!(
            Route::parse("/settings/storage"),
            Route::Settings(SettingsPage::Maintenance)
        );
        assert_eq!(
            Route::parse("/settings/integrations"),
            Route::Settings(SettingsPage::Browser)
        );
        assert_eq!(Route::parse("/e/t/"), Route::Thread(key("e", "t")));
        assert_eq!(Route::parse("/pair?token=x#host=y"), Route::Pair);
    }

    #[test]
    fn malformed_addresses_are_not_found_rather_than_misrouted() {
        assert_eq!(Route::parse("/e/%ZZ"), Route::NotFound("/e/%ZZ".into()));
        assert_eq!(Route::parse("/e/abc%"), Route::NotFound("/e/abc%".into()));
        assert_eq!(Route::parse("/e/%4"), Route::NotFound("/e/%4".into()));
        assert_eq!(Route::parse("/e/a%20b"), Route::Thread(key("e", "a b")));
        assert_eq!(
            Route::parse("/%C0%AF/t"),
            Route::NotFound("/%C0%AF/t".into())
        );
        assert_eq!(
            Route::parse("/settings/nope"),
            Route::NotFound("/settings/nope".into())
        );
        assert_eq!(
            Route::parse("/e/t/nope"),
            Route::NotFound("/e/t/nope".into())
        );
        assert_eq!(Route::parse("/a/b/c/d"), Route::NotFound("/a/b/c/d".into()));
        assert_eq!(Route::parse("//t"), Route::NotFound("//t".into()));
        assert_eq!(Route::parse("//e/t"), Route::NotFound("//e/t".into()));
        assert_eq!(Route::parse("e/t"), Route::Thread(key("e", "t")));
    }

    #[test]
    fn the_launch_address_picks_the_first_route_and_unpaired_apps_start_at_pairing() {
        assert_eq!(initial_route(None, true), Route::Home);
        assert_eq!(initial_route(None, false), Route::Pair);
        let thread = "https://app.t3.codes/e/t";
        assert_eq!(
            initial_route(Some(thread), true),
            Route::Thread(key("e", "t"))
        );
        assert_eq!(initial_route(Some(thread), false), Route::Pair);
        assert_eq!(
            initial_route(Some("https://app.t3.codes/pair#token=x"), true),
            Route::Pair
        );
        assert_eq!(
            initial_route(Some("http://127.0.0.1:18765/?x=1"), true),
            Route::Home
        );
        assert_eq!(
            initial_route(Some("t3code://settings/chat"), true),
            Route::Settings(SettingsPage::Chat)
        );
        assert_eq!(initial_route(Some("HTTPS://h.test"), true), Route::Home);
        assert_eq!(path_of("https://h.test?q=1"), "/?q=1");
        assert_eq!(path_of("new"), "/new");
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
