use super::*;
use crate::view_logic::shell::navigation::is_thread_path;

#[test]
fn normalization_decodes_escaped_static_characters_and_preserves_segment_boundaries() {
    assert_eq!(
        normalize_app_path("/%73ettings/%67eneral?prompt=123#source"),
        "/settings/general?prompt=123#source"
    );
    assert_eq!(
        normalize_app_path("/env%20space/mcp%3Aexample%2Fpart%252F"),
        "/env%20space/mcp%3Aexample%2Fpart%252F"
    );
    assert_eq!(normalize_app_path("/e/%"), "/e/%");
    assert_eq!(normalize_app_path("/e/%5Bid%5D-%c3%a9"), "/e/[id]-%C3%A9");
    assert_eq!(normalize_app_path("/e/%5c%25%2f"), "/e/%5c%25%2f");
    assert_eq!(normalize_app_path("/%0a/evil"), "/evil");
    assert_eq!(normalize_app_path("/e/%c3%a9%ZZ"), "/e/%c3%a9%ZZ");
    assert_eq!(normalize_app_path("/e/café"), "/e/caf%C3%A9");
    assert_eq!(normalize_app_path("/e/%F0%9F%98%80"), "/e/%F0%9F%98%80");
    assert_eq!(normalize_app_path("/e/%C0%AF"), "/e/%C0%AF");
}

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
    let policy = RoutePolicy::for_client(true);
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
        ("t3code-dev://settings/chat", "/settings/chat"),
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
    assert_eq!(path_of("new"), "new");
    assert_eq!(
        initial_route(Some("new"), true),
        Route::NotFound("new".into())
    );
}

#[test]
fn each_launch_address_lands_where_the_web_router_sends_it_in_each_state() {
    // Unknown: before the first restore. Signed out: unpaired, or signed in to T3 Connect with
    // no environment yet. Signed in: an environment is saved, a relay one included.
    let unknown: Option<RoutePolicy> = None;
    let signed_out = Some(RoutePolicy::for_client(false));
    let signed_in = Some(RoutePolicy::for_client(true));
    let pairing = "/pair?host=https%3A%2F%2Fbox.example&label=Box#token=abc";
    let project = "/projects/env%3Arepo";
    for (launch, before_restore, unpaired, paired) in [
        ("/", "/", "/pair", "/"),
        ("/new?prompt=hi", "/new?prompt=hi", "/pair", "/new?prompt=hi"),
        ("/e/t?diff=1", "/e/t?diff=1", "/pair", "/e/t?diff=1"),
        ("/threads/e/t", "/e/t", "/pair", "/e/t"),
        ("/draft/d", "/draft/d", "/pair", "/draft/d"),
        ("/pull-requests", "/pull-requests", "/pair", "/pull-requests"),
        ("/settings", "/settings/general", "/pair", "/settings/general"),
        ("/settings/beta", "/settings/labs", "/pair", "/settings/labs"),
        ("/settings/secrets", "/settings/secrets", "/pair", "/settings/general"),
        ("/welcome", "/welcome", "/pair", "/welcome"),
        ("/pair", "/pair", "/pair", "/"),
        // A pairing launch stays on `/pair` even when already paired.
        (pairing, pairing, pairing, pairing),
        ("/pair#token=abc", "/pair#token=abc", "/pair#token=abc", "/"),
        // An agent approval or connect-CLI launch: hosted static app only, which this is not.
        ("/approve-agent?code=x", "/approve-agent?code=x", "/", "/"),
        ("/connect?state=s", "/connect?state=s", "/", "/"),
        ("/connect-agent", "/connect-agent", "/connect-agent", "/connect-agent"),
        ("/guest", "/guest", "/guest", "/guest"),
        (project, project, "/pair", "/settings/projects?project=env%3Arepo"),
        ("/nope", "/nope", "/nope", "/nope"),
    ] {
        for (state, policy, expected) in [
            ("before restore", unknown, before_restore),
            ("unpaired", signed_out, unpaired),
            ("paired", signed_in, paired),
        ] {
            let landed = Location::resolve(launch, policy).href();
            assert_eq!(landed, expected, "{launch} {state}");
            assert_eq!(
                Location::resolve(&landed, policy).href(),
                landed,
                "{launch} {state}: the redirect settles"
            );
        }
    }
}
