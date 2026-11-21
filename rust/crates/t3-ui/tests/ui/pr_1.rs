//! PullRequestsPage through the production router and a paired RPC client.

use crate::support::{DESKTOP, PHONE};
use futures::FutureExt;
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use serde_json::{Value, json};
use std::rc::Rc;
use t3_client::{
    ClientStore,
    composition::{ClientRuntime, Spawner},
    testing::environment::FakeEnvironment,
};
use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;
use t3_ui::{
    runtime::{Rt, host::Choice},
    state::nav::Route,
};

pub(super) fn entry(number: u64, title: &str, state: &str, size: u64) -> Value {
    json!({"provider":"github","host":"github.com","projectId":"project","projectTitle":"Project",
        "repository":"team/widget","number":number,"title":title,"url":format!("https://github.com/team/widget/pull/{number}"),
        "author":{"login":"alice","name":null,"avatarUrl":null},"headBranch":"feature","baseBranch":"main",
        "state":state,"isDraft":false,"mergeability":"mergeable","additions":size,"deletions":0,
        "createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-02T00:00:00Z",
        "viewerReviewRequested":false,"labels":[]})
}

pub(super) fn answer(entries: Vec<Value>) -> Value {
    json!({"viewers":{"github.com":"alice"},"providers":[],"entries":entries,"errors":[],"truncated":false,"nextCursors":{}})
}

fn expected_requests() -> Value {
    json!([
        {"involvement":"all","state":"open","limit":99},
        {"involvement":"all","state":"open","limit":99,"cursors":{"project":"page-two"}},
        {"involvement":"all","state":"closed","limit":99},
        {"involvement":"reviewing","state":"closed","limit":99},
        {"involvement":"reviewing","state":"closed","limit":99,"filters":{"labels":[["bug"]]},"query":"fix"}
    ])
}

#[component]
fn PrFixture() -> impl IntoElement {
    let owner = Owner::current();
    let spawner: Spawner = Rc::new(move |future| {
        owner.run(|| {
            spawn(future);
        });
    });
    let server = FakeEnvironment::new("review-machine", spawner.clone());
    server.set_permissions(&["orchestration:read"]);
    let mut first = answer(vec![
        entry(14, "Small change", "open", 2),
        entry(18, "Large change", "open", 80),
    ]);
    first["truncated"] = json!(true);
    first["nextCursors"] = json!({"project":"page-two"});
    server.reply("pullRequests.list", Ok(first));
    server.reply(
        "pullRequests.list",
        Ok(answer(vec![entry(12, "Earlier change", "open", 5)])),
    );
    server.reply(
        "pullRequests.list",
        Ok(answer(vec![entry(7, "Closed change", "closed", 1)])),
    );
    server.reply("pullRequests.list", Ok(answer(vec![])));
    server.reply("pullRequests.list", Ok(answer(vec![])));
    let store = ClientStore::default();
    let client = ClientRuntime::new(
        server.platform(),
        store.clone(),
        OrchestrationV2CreationSource::Web,
        spawner.clone(),
    );
    let pairing = client.pair(server.pairing());
    spawner(
        async move {
            pairing.await.expect("pair the PR server");
        }
        .boxed_local(),
    );
    let calls = signal(String::new());
    on_key("ctrl+shift+u", move |_| {
        assert_eq!(
            json!(server.calls("pullRequests.list")),
            expected_requests()
        );
        calls.set("Wire requests matched".to_owned());
    });
    let runtime = Rt::new(store, Rc::new(client.clone()), Choice::Client);
    view()
        .flex_1()
        .child(t3_ui::app::app_tree(
            runtime,
            Route::PullRequests,
            move || t3_ui::app::provide_client_services(&client),
        ))
        .child(text(move || calls.get()))
}

fn browse(window: (f32, f32)) {
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<PrFixture>())
    };
    if window == DESKTOP {
        config.environment.os = nsh_core::platform::Os::Linux;
        config.environment.idiom = nsh_core::platform::Idiom::Desktop;
    }
    let mut app = Headless::start(config).expect("headless PR route");
    app.wait_for("Small change")
        .expect("paired server's open requests");
    let row_y = |app: &Headless, title: &str| {
        app.screen()
            .layout_nodes()
            .into_iter()
            .find(|node| Some(node.id) == app.screen().id(title).ok())
            .expect("row")
            .bounds
            .origin
            .y
    };
    assert!(row_y(&app, "Small change") < row_y(&app, "Large change"));
    app.press("role:button[Sort pull requests]")
        .expect("sort menu");
    app.press("Largest shown").expect("size sort");
    app.settle();
    assert!(row_y(&app, "Large change") < row_y(&app, "Small change"));
    app.press("role:button[Load more pull requests]")
        .expect("load another host page");
    app.wait_for("Earlier change").expect("next-page response");
    assert!(app.screen().shows("Small change"));
    app.press("role:button[Filters]").expect("filters");
    app.press("State").expect("state menu");
    app.press("Closed").expect("closed filter");
    app.wait_for("Closed change").expect("filtered host reply");
    assert!(!app.screen().matches("Small change").expect("old result"));
    app.press("role:button[Filters]").expect("filters");
    app.press("Involvement").expect("involvement menu");
    app.press("Reviewing").expect("review requests");
    app.wait_for("Nothing under these filters")
        .expect("empty filtered answer");
    app.type_text("label:Search pull requests", "label:bug fix")
        .expect("type qualified query");
    app.press_key("Enter").expect("submit query");
    app.wait_for("Nothing matches “label:bug fix”")
        .expect("search answer");
    app.press_key("ctrl+shift+u")
        .expect("read the actual wire requests");
    app.wait_for("Wire requests matched")
        .expect("server filters, local sort");
}

#[test]
fn desktop_browses_filters_and_sorts_pull_requests() {
    browse(DESKTOP);
}
#[test]
fn phone_browses_filters_and_sorts_pull_requests() {
    browse(PHONE);
}

pub(super) struct StaticUrl;
impl t3_ui::state::url::UrlClient for StaticUrl {
    fn watch(&self) -> futures::stream::LocalBoxStream<'static, t3_ui::state::url::UrlSnapshot> {
        Box::pin(futures::stream::pending())
    }
    fn navigate(
        &self,
        _: t3_ui::state::url::UrlRequest,
    ) -> futures::future::LocalBoxFuture<'static, Result<(), t3_ui::i18n::Text>> {
        Box::pin(async { Ok(()) })
    }
}

#[component]
fn ScopedRoute() -> impl IntoElement {
    let owner = Owner::current();
    let spawner: Spawner = Rc::new(move |f| {
        owner.run(|| {
            spawn(f);
        })
    });
    let main = FakeEnvironment::new("first", spawner.clone());
    let other = FakeEnvironment::at_origin("second", "https://other.test", spawner.clone());
    let mut other_shell = t3_client::testing::environment::shell(&[]);
    other_shell["projects"][0]["id"] = json!("unrelated");
    other.set_shell(other_shell);
    main.reply(
        "pullRequests.list",
        Ok(answer(vec![entry(1, "Owned project", "open", 1)])),
    );
    other.reply("pullRequests.list", Ok(answer(vec![])));
    let store = ClientStore::default();
    let client = ClientRuntime::new(
        main.platform().with_environment(&other),
        store.clone(),
        OrchestrationV2CreationSource::Web,
        spawner.clone(),
    );
    let runtime = Rt::new(store, Rc::new(client.clone()), Choice::Client);
    let first = client.pair(main.pairing());
    let second = other.pairing();
    let paired = signal(false);
    let connecting = client.clone();
    spawner(
        async move {
            first.await.expect("pair first");
            connecting.pair(second).await.expect("pair second");
            paired.set(true);
        }
        .boxed_local(),
    );
    let requests = signal(String::new());
    on_key("ctrl+shift+u", move |_| {
        requests.set(format!(
            "first {} second {}",
            main.calls("pullRequests.list").len(),
            other.calls("pullRequests.list").len()
        ))
    });
    view()
        .flex_1()
        .child(when(paired, move || {
            let client = client.clone();
            t3_ui::app::app_tree(runtime, Route::PullRequests, move || {
                t3_ui::state::url::UrlState::install(
                    t3_ui::state::url::Location::parse(
                        "/pull-requests?environmentId=removed&projectId=project&q=private",
                    ),
                    t3_ui::state::nav::navigator(),
                    Rc::new(StaticUrl),
                );
                t3_ui::app::provide_client_services(&client);
            })
        }))
        .child(text(move || requests.get()))
}

#[test]
fn project_search_uses_its_owner_after_a_saved_server_is_removed() {
    let config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(DESKTOP.0), px(DESKTOP.1)),
        ..EngineConfig::new(Root::of::<ScopedRoute>())
    };
    let mut app = Headless::start(config).expect("scoped route");
    app.wait_for("Owned project")
        .expect("known project's server answered");
    app.press_key("ctrl+shift+u").expect("inspect both servers");
    app.wait_for("first 1 second 0")
        .expect("unrelated server never received the scoped search");
}
