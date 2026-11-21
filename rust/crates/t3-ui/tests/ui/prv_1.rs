//! Preview surface from the workspace host through the paired environment's typed RPCs.

use crate::{
    pr_2::app_at,
    support::{DESKTOP, PHONE},
};
use futures::FutureExt;
use nsh_core::prelude::*;
use nsh_engine::Root;
use serde_json::json;
use std::rc::Rc;
use t3_client::{
    ClientStore,
    composition::{ClientRuntime, Spawner},
    testing::{
        CallKind,
        environment::{FakeEnvironment, NOW},
    },
    types::{EnvironmentId, ThreadId, ThreadKey},
};
use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;
use t3_ui::{
    app::provide_client_services,
    runtime::{Rt, host::Choice},
    screens::panel::{PanelRegistry, SurfaceKind, panel_host, right_panel_tabs},
    state::nav::{Navigator, Route},
};

#[component]
fn BrowserFixture() -> impl IntoElement {
    browser_fixture_content(false)
}

#[component]
fn ExistingBrowserFixture() -> impl IntoElement {
    browser_fixture_content(true)
}

fn browser_fixture_content(existing: bool) -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let owner = Owner::current();
    let run: Spawner = Rc::new(move |task| {
        owner.run(|| {
            spawn(task);
        });
    });
    let server = FakeEnvironment::new("preview-host", run.clone());
    server.set_permissions(&["orchestration:read", "preview:operate"]);
    for method in ["preview.list", "preview.open", "preview.close"] {
        server.register(method, CallKind::Unary);
    }
    for method in ["subscribePreviewEvents", "subscribeDiscoveredLocalServers"] {
        server.register(method, CallKind::Stream);
    }
    let sessions = if existing {
        vec![json!({
            "threadId":"thread","tabId":"server-browser","runtime":"server","updatedAt":NOW,
            "canGoBack":false,"canGoForward":false,"navStatus":{"_tag":"Success","url":"https://existing.example/","title":"Existing page"}
        })]
    } else {
        vec![json!({
            "threadId":"thread","tabId":"background-browser","runtime":"server","updatedAt":NOW,
            "canGoBack":false,"canGoForward":false,"reveal":false,"navStatus":{"_tag":"Idle"}
        })]
    };
    let list = json!({"sessions": sessions, "serverEpoch": "preview-epoch", "revision": 1});
    if existing {
        let reply = stored(Some(server.defer_reply("preview.list")));
        let list = stored(list.clone());
        let mut early = sessions[0].clone();
        early["tabId"] = json!("early-browser");
        server.stream_snapshot(
            "subscribePreviewEvents",
            vec![json!({
                "type":"opened", "threadId":"thread", "tabId":"early-browser", "createdAt":NOW,
                "serverEpoch":"preview-epoch", "revision":2, "snapshot":early
            })],
        );
        on_key("f8", move |_| {
            reply
                .with(Option::take)
                .expect("pending list")
                .send(Ok(list.get()))
                .expect("list remains subscribed");
        });
    }
    server.reply("preview.list", Ok(list));
    server.stream_snapshot("subscribeDiscoveredLocalServers", vec![json!({
        "servers": [{
            "host":"localhost", "port":5173, "url":"http://localhost:5173/",
            "processName":"fixture-dev-server", "pid":101, "terminal":{"threadId":"thread","terminalId":"term"}
        }],
        "scannedAt": NOW
    })]);
    for (tab, url) in [
        ("discovered", "http://localhost:5173/"),
        ("typed", "https://example.test/"),
    ] {
        server.reply("preview.open", Ok(json!({
            "threadId":"thread", "tabId":tab, "navStatus":{"_tag":"Success","url":url,"title":"Fixture page"},
            "canGoBack":false,"canGoForward":false,"runtime":"server","updatedAt":NOW
        })));
        server.reply("preview.close", Ok(json!(null)));
        if tab == "discovered" {
            server.reply(
                "preview.open",
                Err(json!({
                    "_tag": "PreviewInvalidUrlError",
                    "inputLength": 21,
                    "reason": "parse",
                    "cause": null
                })),
            );
        }
    }
    let store = ClientStore::default();
    let client = ClientRuntime::new(
        server.platform(),
        store.clone(),
        OrchestrationV2CreationSource::Web,
        run.clone(),
    );
    provide_context(Rt::new(store, Rc::new(client.clone()), Choice::Client));
    provide_context(Navigator::new(Route::Home));
    let paired = signal(false);
    let pair = client.pair(server.pairing());
    run(async move {
        pair.await.expect("pair preview host");
        paired.set(true);
    }
    .boxed_local());
    provide_client_services(&client);
    let registry = use_context::<PanelRegistry>().expect("app panel registry");
    let host = panel_host();
    let key = stored(ThreadKey {
        environment: EnvironmentId("preview-host".into()),
        thread: ThreadId("thread".into()),
    });
    let verified = signal(false);
    on_key("f9", move |_| {
        let calls = server.calls("preview.open");
        if existing {
            assert!(calls.is_empty());
            verified.set(true);
            return;
        }
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[0]["url"], "http://localhost:5173/");
        assert_eq!(calls[1]["url"], "https://refused.example/");
        assert_eq!(calls[2]["url"], "https://example.test/");
        assert_eq!(
            server.calls("preview.close"),
            vec![json!({"threadId":"thread","tabId":"discovered"})]
        );
        assert!(
            calls
                .iter()
                .all(|call| call["runtime"] == "server" && call["threadId"] == "thread")
        );
        assert!(!server.calls("subscribeDiscoveredLocalServers").is_empty());
        verified.set(true);
    });
    view()
        .w_full()
        .h_full()
        .child(when(paired, move || {
            nsh_base::button()
                .accessibility_label("Open browser")
                .on_press(move || registry.open(SurfaceKind::Browser, &key.get()))
                .child(text("Open browser"))
        }))
        .child(when(verified, || text("Verified preview requests")))
        .child(right_panel_tabs(host, key.get(), registry, view()))
}

fn browse(width: (f32, f32)) {
    let mut app = app_at(width, Root::of::<BrowserFixture>());
    app.wait_for("role:button[Open browser]").expect("paired");
    app.press("role:button[Open browser]")
        .expect("open browser surface");
    app.wait_for("label:Search or enter URL")
        .expect("the registered browser address field");
    app.wait_for("Local servers")
        .expect("discovery subscription");
    app.press("role:button[Open http://localhost:5173/]")
        .expect("open discovered server");
    app.wait_for("role:button[Close browser tab]")
        .expect("opened server tab");
    app.press("role:button[Close browser tab]")
        .expect("close server tab");
    app.press("role:button[Open browser]")
        .expect("open a fresh browser surface");
    app.type_text("label:Search or enter URL", "https://refused.example/")
        .expect("typed address");
    app.press_key("Enter").expect("submit refused address");
    app.wait_for("Enter an http or https URL.")
        .expect("typed RPC refusal");
    app.clear("label:Search or enter URL")
        .expect("clear refused address");
    app.type_text("label:Search or enter URL", "https://example.test/")
        .expect("corrected address");
    app.press_key("Enter").expect("submit address");
    app.wait_for("role:button[Close browser tab]")
        .expect("new server tab");
    app.press_key("f9")
        .expect("verify environment-scoped commands");
    app.wait_for("Verified preview requests")
        .expect("RPC transcript verified");
}

#[test]
fn desktop_opens_discovers_and_closes_a_browser() {
    browse(DESKTOP);
}

#[test]
fn phone_opens_discovers_and_closes_a_browser() {
    browse(PHONE);
}

fn restores_existing_browser(width: (f32, f32)) {
    let mut app = app_at(width, Root::of::<ExistingBrowserFixture>());
    app.wait_for("role:button[Open browser]").expect("paired");
    app.press("role:button[Open browser]").expect("open panel");
    app.wait_for("Local servers")
        .expect("the empty pane stays open while the list is pending");
    assert!(!app.screen().shows("role:button[Close browser tab]"));
    app.press_key("f8")
        .expect("receive the initial session list");
    app.wait_for("role:button[Close browser tab]")
        .expect("existing tab restored");
    assert_eq!(
        app.screen()
            .count("role:tab[Browser]")
            .expect("browser tabs"),
        2
    );
    app.press_key("f9").expect("verify browser was restored");
    app.wait_for("Verified preview requests")
        .expect("an existing tab was reused");
}

#[test]
fn desktop_restores_the_hosts_existing_tab() {
    restores_existing_browser(DESKTOP);
}

#[test]
fn phone_restores_the_hosts_existing_tab() {
    restores_existing_browser(PHONE);
}
