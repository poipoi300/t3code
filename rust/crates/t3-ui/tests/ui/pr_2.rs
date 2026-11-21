//! Selected pull request detail through the real app and a paired server.
use crate::{
    pr_1::{answer, entry},
    support::{DESKTOP, PHONE},
};
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

fn detail() -> Value {
    let mut v = entry(14, "Selected detail from host", "open", 27);
    v["capabilities"] = json!({"actions":[],"mergeMethods":[],"diff":true,"comment":false,"search":true,"deleteBranch":false,"review":{"inlineComment":false,"reply":false,"resolve":false,"verdicts":[]},"reviewers":{"request":false,"listCandidates":false}});
    v["viewerPermissions"] = json!({"actions":[],"comment":false,"resolve":false,"verdicts":[],"requestReviewers":false});
    v["workspaceRoot"] = json!("/project");
    v["body"] = json!("A description supplied by the forge.");
    v["changedFiles"] = json!(3);
    v["mergedAt"] = Value::Null;
    v["closedAt"] = Value::Null;
    v["reviewers"] = json!([]);
    v["checks"] = json!([{"name":"Linux build","status":"failure","description":"One test failed","url":"https://github.com/team/widget/actions/runs/11"}]);
    v["mergeCapabilities"] = json!({"merge":false,"squash":false,"rebase":false});
    v
}

#[component]
fn DetailFixture() -> impl IntoElement {
    let owner = Owner::current();
    let spawn_in: Spawner = Rc::new(move |f| {
        owner.run(|| {
            spawn(f);
        })
    });
    let server = FakeEnvironment::new("forge", spawn_in.clone());
    server.set_permissions(&["orchestration:read"]);
    server.reply(
        "pullRequests.list",
        Ok(answer(vec![entry(14, "List title", "open", 0)])),
    );
    for _ in 0..2 {
        server.reply("pullRequests.detail", Ok(detail()));
        server.reply("pullRequests.linkedThreads",Ok(json!({"threads":[{"id":"linked","projectId":"project","title":"Related discussion","archivedAt":null}]})));
    }
    let store = ClientStore::default();
    let client = ClientRuntime::new(
        server.platform(),
        store.clone(),
        OrchestrationV2CreationSource::Web,
        spawn_in.clone(),
    );
    let pair = client.pair(server.pairing());
    spawn_in(
        async move {
            pair.await.expect("pair");
        }
        .boxed_local(),
    );
    let linked_route = signal(false);
    let checked = signal(false);
    on_key("f9", move |_| {
        for tag in ["pullRequests.detail", "pullRequests.linkedThreads"] {
            let calls = server.calls(tag);
            assert_eq!(
                json!(calls),
                json!([{"projectId":"project","repository":"team/widget","number":14,"host":"github.com"}]),
                "{tag}"
            );
        }
        checked.set(true);
    });
    view()
        .w_full()
        .h_full()
        .child(view().flex_1().min_h(px(0.)).child(t3_ui::app::app_tree(
            Rt::new(store, Rc::new(client.clone()), Choice::Client),
            Route::PullRequests,
            move || {
                t3_ui::app::provide_client_services(&client);
                let nav=t3_ui::state::nav::navigator();
                effect(move||linked_route.set(matches!(nav.top(), Route::Thread(key) if key.environment.0=="forge" && key.thread.0=="linked")));
            },
        )))
        .child(text(move|| if linked_route.get() {"Opened linked thread"} else {""}))
        .child(text(
            move || if checked.get() { "Wire verified" } else { "" },
        ))
}

fn app_at(window: (f32, f32), root: Root) -> Headless {
    let mut cfg = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(root)
    };
    if window == DESKTOP {
        cfg.environment.os = nsh_core::platform::Os::Linux;
        cfg.environment.idiom = nsh_core::platform::Idiom::Desktop;
    }
    Headless::start(cfg).expect("start app")
}

fn shows_detail(window: (f32, f32)) {
    let mut app = app_at(window, Root::of::<DetailFixture>());
    app.wait_for("List title").expect("live PR row");
    app.press("role:button[Open pull request #14]")
        .expect("select the PR");
    app.wait_for("Selected detail from host")
        .expect("detail title");
    app.wait_for("3 files").expect("size from the detail read");
    app.wait_for("A description supplied by the forge.")
        .expect("host markdown");
    app.wait_for("role:link[Open pull request #14 on host]")
        .expect("forge link");
    app.wait_for("role:link[team/widget]")
        .expect("repository link");
    app.press("role:button[Checks: 1 of 1 failing]")
        .expect("open check details");
    app.wait_for("Linux build").expect("individual check");
    app.wait_for("role:link[Linux build]")
        .expect("check run link");
    app.wait_for("Related discussion").expect("linked thread");
    app.press_key("f9").expect("inspect read requests");
    app.wait_for("Wire verified").expect("scope");
    app.press("role:button[Back to pull requests]")
        .expect("close detail");
    app.wait_for("List title").expect("return to list");
    app.wait_for_gone("Selected detail from host", nsh_testing::WAIT)
        .expect("detail sheet closed");
    app.press("role:button[Open pull request #14]")
        .expect("reopen PR");
    app.wait_for("Related discussion")
        .expect("linked discussion");
    app.press("role:button[Related discussion]")
        .expect("follow scoped thread link");
    app.wait_for("Opened linked thread")
        .expect("navigation carries server and thread");
}
#[test]
fn desktop_reads_selected_pr_detail() {
    shows_detail(DESKTOP)
}
#[test]
fn phone_reads_selected_pr_detail() {
    shows_detail(PHONE)
}

#[component]
fn SidebarPairing() -> impl IntoElement {
    let owner = Owner::current();
    let spawner: Spawner = Rc::new(move |future| {
        owner.run(|| {
            spawn(future);
        })
    });
    let server = FakeEnvironment::new("forge", spawner.clone());
    server.reply(
        "pullRequests.list",
        Ok(answer(vec![entry(14, "Sidebar PR", "open", 0)])),
    );
    let store = ClientStore::default();
    let client = ClientRuntime::new(
        server.platform(),
        store.clone(),
        OrchestrationV2CreationSource::Web,
        spawner.clone(),
    );
    let pairing = client.clone();
    let paired = signal(false);
    on_key_with(
        "f7",
        KeyOptions {
            when: nsh_core::keymap::When::Always,
            ..Default::default()
        },
        move |_| {
            let task = pairing.pair(server.pairing());
            spawner(
                async move {
                    task.await.expect("pair sidebar server");
                    paired.set(true);
                }
                .boxed_local(),
            );
        },
    );
    view()
        .w_full()
        .h_full()
        .child(view().flex_1().min_h(px(0.)).child(t3_ui::app::app_tree(
            Rt::new(store, Rc::new(client.clone()), Choice::Client),
            Route::Home,
            move || t3_ui::app::provide_client_services(&client),
        )))
        .child(text(move || {
            if paired.get() {
                "Server paired"
            } else {
                "Ready to pair"
            }
        }))
}
fn enters_from_sidebar(window: (f32, f32)) {
    let mut app = app_at(window, Root::of::<SidebarPairing>());
    app.wait_for("Ready to pair").expect("sidebar mounted");
    assert!(
        !app.screen()
            .matches("role:button[Pull requests]")
            .expect("no configured server")
    );
    app.press_key("f7").expect("connect server");
    app.wait_for("Server paired")
        .expect("server supplied config");
    app.wait_for("role:button[Pull requests]")
        .expect("configured catalog");
    app.press("role:button[Pull requests]")
        .expect("configured server enables PRs");
    app.wait_for("role:button[Sort pull requests]")
        .expect("sidebar opened route");
}
#[test]
fn desktop_enters_pr_list_from_sidebar() {
    enters_from_sidebar(DESKTOP)
}
#[test]
fn phone_enters_pr_list_from_sidebar() {
    enters_from_sidebar(PHONE)
}

#[component]
fn ScopedLink() -> impl IntoElement {
    let owner = Owner::current();
    let spawner: Spawner = Rc::new(move |f| {
        owner.run(|| {
            spawn(f);
        })
    });
    let first = FakeEnvironment::new("first", spawner.clone());
    let second = FakeEnvironment::at_origin("second", "https://other.test", spawner.clone());
    let mut wrong = detail();
    wrong["title"] = json!("Wrong server");
    first.reply("pullRequests.detail", Ok(wrong));
    second.reply("pullRequests.detail", Ok(detail()));
    let linked = json!({"threads":[]});
    first.reply("pullRequests.linkedThreads", Ok(linked.clone()));
    second.reply("pullRequests.linkedThreads", Ok(linked));
    let store = ClientStore::default();
    let client = ClientRuntime::new(
        first.platform().with_environment(&second),
        store.clone(),
        OrchestrationV2CreationSource::Web,
        spawner.clone(),
    );
    let pair = client.pair(first.pairing());
    spawner(
        async move {
            pair.await.expect("pair first");
        }
        .boxed_local(),
    );
    let later = client.clone();
    let second_input = second.pairing();
    on_key("f7", move |_| {
        let f = later.pair(second_input.clone());
        spawner(
            async move {
                f.await.expect("pair scoped server");
            }
            .boxed_local(),
        );
    });
    let seen = signal(String::new());
    on_key("f9", move |_| {
        seen.set(format!(
            "Detail calls {} {}",
            first.calls("pullRequests.detail").len(),
            second.calls("pullRequests.detail").len()
        ))
    });
    view().w_full().h_full().child(view().flex_1().min_h(px(0.)).child(t3_ui::app::app_tree(Rt::new(store,Rc::new(client.clone()),Choice::Client),Route::PullRequests,move||{
        use t3_ui::state::url::{UrlState,Location};
        UrlState::install(Location::parse("/pull-requests?environmentId=second&projectId=project&host=github.com&repository=team%2Fwidget&number=14"),t3_ui::state::nav::navigator(),Rc::new(crate::pr_1::StaticUrl));
        t3_ui::app::provide_client_services(&client);
    }))).child(text(move||seen.get()))
}

#[test]
fn shared_link_waits_for_its_scoped_server_before_reading_detail() {
    let mut app = Headless::start(EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(1280.), px(800.)),
        ..EngineConfig::new(Root::of::<ScopedLink>())
    })
    .expect("scoped app");
    app.wait_for("Project").expect("first server loaded");
    app.press_key("f9").expect("read initial calls");
    app.wait_for("Detail calls 0 0")
        .expect("wait for named server");
    app.press_key("f7").expect("connect named server");
    app.wait_for("Selected detail from host")
        .expect("scoped deep link");
    app.press_key("f9").expect("read calls");
    app.wait_for("Detail calls 0 1")
        .expect("only the intended server was queried");
}
