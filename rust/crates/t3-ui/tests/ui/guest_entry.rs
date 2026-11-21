//! GUEST-4: the two ways to "Share with guest" from the real app, over a paired fake server: the
//! thread's context menu and the command palette. Each opens the one dialog, which asks the server
//! for a link for that thread only.

use std::rc::Rc;

use futures::FutureExt;
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use serde_json::json;
use t3_client::{
    ClientStore,
    composition::{ClientRuntime, Spawner},
    testing::environment::{FakeEnvironment, shell},
};
use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;
use t3_ui::{
    app::{app_tree, provide_client_services},
    runtime::{Rt, host::Choice},
    screens::sidebar::row::RowThreadActionsRuntime,
    state::nav::{Route, navigator},
};

use crate::support::{DESKTOP, PHONE};

const URL: &str = "https://box.example/guest#share1.signature";

/// Sharing is switched on in Labs and the session may create links, unless `enabled` says not.
fn workspace(enabled: bool, scopes: &[&str], home: bool) -> impl IntoElement {
    let owner = Owner::current();
    let spawner: Spawner = Rc::new(move |future| {
        owner.run(|| {
            spawn(future);
        });
    });
    let server = FakeEnvironment::new("remote", spawner.clone());
    server.set_permissions(scopes);
    server.set_shell(shell(&[("alpha", "Alpha thread")]));
    server.register("guestShare.create", t3_client::testing::CallKind::Unary);
    server.reply(
        "guestShare.create",
        Ok(json!({"url": URL, "share": {
            "id": "share1", "host": "box.example", "createdAt": "2026-06-20T00:00:00.000Z",
            "expiresAt": "2026-06-21T00:00:00.000Z", "guestName": null, "messages": []}})),
    );
    server.edit_config(|config| config["settings"]["guestSharingEnabled"] = json!(enabled));
    let platform = server.platform();
    let store = ClientStore::default();
    let client = ClientRuntime::new(
        platform,
        store.clone(),
        OrchestrationV2CreationSource::Web,
        spawner.clone(),
    );
    let pairing = client.pair(server.pairing());
    spawner(
        async move {
            pairing.await.expect("pair fixture environment");
        }
        .boxed_local(),
    );
    let thread = t3_client::types::ThreadKey {
        environment: t3_client::types::EnvironmentId("remote".into()),
        thread: t3_contracts::base_schemas::ThreadId("alpha".into()),
    };
    let runtime = Rt::new(store, Rc::new(client.clone()), Choice::Client);
    let created = server.clone();
    let requests = signal(String::new());
    on_key("f9", move |_| {
        requests.set(
            created
                .calls("guestShare.create")
                .into_iter()
                .map(|payload| payload["threadId"].as_str().unwrap_or_default().to_owned())
                .collect::<Vec<_>>()
                .join(","),
        );
    });
    let route = if home {
        Route::Home
    } else {
        Route::Thread(thread)
    };
    let app = app_tree(runtime, route, move || {
        provide_client_services(&client);
        provide_context(RowThreadActionsRuntime(Rc::new(client)));
        let _ = navigator();
    });
    view()
        .w_full()
        .h_full()
        .child(app)
        .child(text(move || format!("Link requests: {}", requests.get())))
}

#[component]
fn Allowed() -> impl IntoElement {
    workspace(
        true,
        &[
            "orchestration:read",
            "orchestration:operate",
            "access:write",
        ],
        false,
    )
}

/// The thread list on its own, as a phone shows it before a thread is opened.
#[component]
fn AllowedOnTheList() -> impl IntoElement {
    workspace(
        true,
        &[
            "orchestration:read",
            "orchestration:operate",
            "access:write",
        ],
        true,
    )
}

#[component]
fn BetaOff() -> impl IntoElement {
    workspace(
        false,
        &[
            "orchestration:read",
            "orchestration:operate",
            "access:write",
        ],
        false,
    )
}

#[component]
fn NoLinkScope() -> impl IntoElement {
    workspace(
        true,
        &["orchestration:read", "orchestration:operate"],
        false,
    )
}

fn start(window: (f32, f32), root: Root) -> Headless {
    let mut fixtures = Registry::new();
    fixtures.register::<Allowed>();
    fixtures.register::<AllowedOnTheList>();
    fixtures.register::<BetaOff>();
    fixtures.register::<NoLinkScope>();
    let mut sets = t3_ui::registries();
    sets.push(fixtures);
    let mut config = EngineConfig {
        ui_sets: sets,
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(root)
    };
    if window == DESKTOP {
        config.environment.os = nsh_core::platform::Os::Linux;
        config.environment.idiom = nsh_core::platform::Idiom::Desktop;
    }
    let mut app = Headless::start(config).expect("workspace starts");
    app.wait_for("role:button[Alpha thread, Project]")
        .expect("paired thread row");
    app
}

fn open_row_menu(app: &mut Headless, window: (f32, f32)) {
    let row = "role:button[Alpha thread, Project]";
    if window == PHONE {
        app.long_press(row).expect("open touch menu");
    } else {
        app.right_click(row).expect("open context menu");
    }
}

fn create_from_the_thread_menu(window: (f32, f32)) {
    let mut app = start(window, Root::of::<AllowedOnTheList>());
    open_row_menu(&mut app, window);
    app.press("role:menuitem[Share with guest]")
        .expect("the menu offers sharing");
    app.wait_for("The link works once and expires in 24 hours.")
        .expect("the dialog opened");
    app.press("role:button[Create link]").expect("create");
    app.wait_for("label:Guest link").expect("the link field");
    app.press_key("f9").expect("read the request log");
    app.wait_for("text:Link requests: alpha")
        .expect("one request, for this thread only");
}

#[test]
fn the_thread_menu_shares_the_thread_at_both_widths() {
    create_from_the_thread_menu(DESKTOP);
    create_from_the_thread_menu(PHONE);
}

#[test]
fn the_thread_menu_hides_sharing_while_the_beta_is_off_or_without_link_access() {
    for root in [Root::of::<BetaOff>(), Root::of::<NoLinkScope>()] {
        let mut app = start(DESKTOP, root);
        open_row_menu(&mut app, DESKTOP);
        app.wait_for("role:menuitem[Rename thread]")
            .expect("the menu is open");
        assert!(!app.screen().shows("Share with guest"));
    }
}

#[test]
fn the_command_palette_shares_the_active_thread() {
    let mut app = start(DESKTOP, Root::of::<Allowed>());
    app.press_key("mod+k").expect("open command palette");
    app.type_text("role:combobox[Command palette]", "share with guest")
        .expect("filter the commands");
    app.press("role:option[Share with guest]")
        .expect("the palette offers sharing");
    app.wait_for("The link works once and expires in 24 hours.")
        .expect("the dialog opened and the palette closed");
    assert!(!app.screen().shows("role:combobox[Command palette]"));
    app.press("role:button[Create link]").expect("create");
    app.wait_for("label:Guest link").expect("the link field");
    app.press_key("f9").expect("read the request log");
    app.wait_for("text:Link requests: alpha")
        .expect("one request, for the active thread");
}

#[test]
fn the_command_palette_hides_sharing_while_the_beta_is_off() {
    let mut app = start(DESKTOP, Root::of::<BetaOff>());
    app.press_key("mod+k").expect("open command palette");
    app.type_text("role:combobox[Command palette]", "share with guest")
        .expect("filter the commands");
    // The Labs setting still matches the query; the command does not.
    app.wait_for("Guest links").expect("the filtered list");
    assert!(!app.screen().shows("role:option[Share with guest]"));
}
