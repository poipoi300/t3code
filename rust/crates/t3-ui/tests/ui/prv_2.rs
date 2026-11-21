//! The mounted panel paints only JPEGs and drives the browser viewer's scoped socket.

use crate::{
    pr_2::app_at,
    support::{DESKTOP, PHONE},
};
use futures::{FutureExt, future::LocalBoxFuture, stream::LocalBoxStream};
use nsh_core::prelude::*;
use nsh_engine::Root;
use serde_json::json;
use std::rc::Rc;
use t3_client::{
    ClientStore,
    composition::{ClientRuntime, Spawner},
    platform::{
        HostAction, HostFuture, HostReply, HttpRequest, HttpResponse, Platform, PlatformError,
        PlatformEvent, SocketRequest, Storage, StorageArea, StorageKey, StorageTransaction,
        StorageWrite, WebSocket,
    },
    preview::test_support::BrowserWire,
    testing::{
        CallKind,
        environment::{EnvironmentPlatform, FakeEnvironment, NOW},
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

#[derive(Clone)]
struct StreamPlatform {
    environment: EnvironmentPlatform,
    browser: BrowserWire,
}

impl Storage for StreamPlatform {
    fn get(&self, area: StorageArea, key: StorageKey) -> HostFuture<'_, Option<Vec<u8>>> {
        self.environment.get(area, key)
    }
    fn write(&self, area: StorageArea, writes: Vec<StorageWrite>) -> HostFuture<'_, ()> {
        self.environment.write(area, writes)
    }
    fn list(&self, area: StorageArea, namespace: String) -> HostFuture<'_, Vec<StorageKey>> {
        self.environment.list(area, namespace)
    }
    fn transaction(&self, transaction: StorageTransaction) -> HostFuture<'_, bool> {
        self.environment.transaction(transaction)
    }
}

impl Platform for StreamPlatform {
    fn now_epoch_ms(&self) -> u64 {
        self.environment.now_epoch_ms()
    }
    fn monotonic_ms(&self) -> u64 {
        self.environment.monotonic_ms()
    }
    fn random_bytes(&self, len: usize) -> Result<Vec<u8>, PlatformError> {
        self.environment.random_bytes(len)
    }
    fn sleep(&self, ms: u64) -> LocalBoxFuture<'_, ()> {
        self.environment.sleep(ms)
    }
    fn yield_to_host(&self) -> LocalBoxFuture<'_, ()> {
        self.environment.yield_to_host()
    }
    fn events(&self) -> LocalBoxStream<'static, PlatformEvent> {
        self.environment.events()
    }
    fn http(&self, request: HttpRequest) -> HostFuture<'_, HttpResponse> {
        if request.method == "POST" && request.url.ends_with("/api/auth/websocket-ticket/media") {
            async move {
                assert!(
                    request
                        .headers
                        .keys()
                        .any(|key| key.eq_ignore_ascii_case("authorization"))
                );
                Ok(HttpResponse {
                    status: 200,
                    headers: Default::default(),
                    body: br#"{"ticket":"preview-ticket","expiresAt":"2036-01-01T00:00:00.000Z"}"#
                        .to_vec(),
                })
            }
            .boxed_local()
        } else {
            self.environment.http(request)
        }
    }
    fn websocket(&self, request: SocketRequest) -> HostFuture<'_, Box<dyn WebSocket>> {
        if request.url.contains("/api/preview-stream/ws?") {
            futures::future::ready(self.browser.connect(request)).boxed_local()
        } else {
            self.environment.websocket(request)
        }
    }
    fn host(&self, action: HostAction) -> HostFuture<'_, HostReply> {
        self.environment.host(action)
    }
}

#[component]
fn StreamFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let owner = Owner::current();
    let run: Spawner = Rc::new(move |task| {
        owner.run(|| {
            spawn(task);
        });
    });
    let server = FakeEnvironment::new("stream-host", run.clone());
    server.set_permissions(&["orchestration:read", "preview:operate"]);
    for method in ["preview.list", "preview.open"] {
        server.register(method, CallKind::Unary);
    }
    for method in ["subscribePreviewEvents", "subscribeDiscoveredLocalServers"] {
        server.register(method, CallKind::Stream);
    }
    let snapshot = json!({
        "threadId":"thread","tabId":"stream-tab","runtime":"server","updatedAt":NOW,
        "canGoBack":false,"canGoForward":false,"navStatus":{"_tag":"Success","url":"https://example.test/","title":"Fixture page"}
    });
    server.reply(
        "preview.list",
        Ok(json!({"sessions":[], "serverEpoch":"preview-epoch","revision":0})),
    );
    server.reply(
        "preview.list",
        Ok(json!({"sessions":[snapshot.clone()], "serverEpoch":"preview-epoch","revision":1})),
    );
    server.reply("preview.open", Ok(snapshot.clone()));
    let updates = [
        server.stream_updates("subscribePreviewEvents"),
        server.stream_updates("subscribePreviewEvents"),
    ];
    on_key("f6", move |_| {
        let mut snapshot = snapshot.clone();
        snapshot["navStatus"]["url"] = json!("https://redirect.example/");
        snapshot["canGoBack"] = json!(true);
        let event = json!({
            "type":"navigated","threadId":"thread","tabId":"stream-tab","createdAt":NOW,
            "serverEpoch":"preview-epoch","revision":2,"snapshot":snapshot
        });
        let delivered = updates
            .iter()
            .filter(|stream| stream.unbounded_send(event.clone()).is_ok())
            .count();
        assert!(delivered > 0);
    });

    let browser = BrowserWire::default();
    assert!(browser.text(
        json!({"type":"control","controller":"you","generation":1,"canOperate":true,"dialog":null})
    ));
    assert!(browser.text(json!({"type":"viewport","width":800,"height":600})));
    assert!(
        browser.binary(b"<script>throw new Error('never interpret page HTML')</script>".to_vec())
    );

    let store = ClientStore::default();
    let client = ClientRuntime::new(
        StreamPlatform {
            environment: server.platform(),
            browser: browser.clone(),
        },
        store.clone(),
        OrchestrationV2CreationSource::Web,
        run.clone(),
    );
    provide_context(Rt::new(store, Rc::new(client.clone()), Choice::Client));
    provide_context(Navigator::new(Route::Home));
    let paired = signal(false);
    let pair = client.pair(server.pairing());
    run(async move {
        pair.await.expect("pair stream host");
        paired.set(true);
    }
    .boxed_local());
    provide_client_services(&client);
    let registry = use_context::<PanelRegistry>().expect("app panel registry");
    let host = panel_host();
    let key = stored(ThreadKey {
        environment: EnvironmentId("stream-host".into()),
        thread: ThreadId("thread".into()),
    });
    let fixture = browser.clone();
    on_key("f8", move |_| {
        assert!(fixture.binary(include_bytes!("prv_2.jpg").to_vec()));
    });
    let fixture = browser.clone();
    on_key("f4", move |_| {
        assert!(fixture.peer_close(1008, "policy"));
    });
    let fixture = browser.clone();
    on_key("f5", move |_| {
        assert!(fixture.text(json!({"type":"clipboard","text":"selected page text"})));
    });
    let fixture = browser.clone();
    on_key("f7", move |_| {
        let inputs = fixture.sent();
        let probe = inputs
            .iter()
            .rev()
            .find(|input| input["type"] == "probe")
            .expect("tap submitted a probe");
        assert!(
            fixture.text(json!({"type":"probe","x":probe["x"],"y":probe["y"],"editable":true}))
        );
    });
    let mobile = t3_ui::state::layout::pointer() == t3_ui::state::layout::Pointer::Coarse;
    let verified = signal(false);
    on_key("f9", move |_| {
        let requests = browser.requests();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].starts_with(
            "wss://server.test/api/preview-stream/ws?threadId=thread&tabId=stream-tab&"
        ));
        assert!(requests[0].contains("wsTicket=preview-ticket"));
        let inputs = browser.sent();
        for (kind, action) in [
            ("mouse", "down"),
            ("mouse", "up"),
            ("key", "down"),
            ("key", "up"),
            ("wheel", ""),
            ("resize", ""),
            ("ack", ""),
        ] {
            assert!(
                inputs.iter().any(|input| input["type"] == kind
                    && (action.is_empty() || input["action"] == action)),
                "missing {kind} {action}: {inputs:?}"
            );
        }
        let key = if mobile { "Enter" } else { "a" };
        for action in ["down", "up"] {
            assert!(inputs.iter().any(|input| input["type"] == "key"
                && input["key"] == key
                && input["action"] == action));
        }
        let mut resizes = inputs.iter().filter(|input| input["type"] == "resize");
        let resize = resizes.next().expect("viewport");
        assert!(
            resizes.next().is_none(),
            "initial layout should send one resize"
        );
        let (width, height) = (
            resize["width"].as_u64().expect("width"),
            resize["height"].as_u64().expect("height"),
        );
        assert!((100..=3840).contains(&width) && (100..=2160).contains(&height));
        assert!(
            inputs
                .iter()
                .filter(|input| matches!(input["type"].as_str(), Some("mouse" | "wheel")))
                .all(|input| {
                    let x = input["x"].as_f64().expect("page x");
                    let y = input["y"].as_f64().expect("page y");
                    (0.0..800.0).contains(&x) && (0.0..600.0).contains(&y)
                })
        );
        assert_eq!(
            inputs.iter().filter(|input| input["type"] == "ack").count(),
            2
        );
        if mobile {
            for action in ["down", "up"] {
                assert!(inputs.iter().any(|input| input["type"] == "key"
                    && input["key"] == "Backspace"
                    && input["action"] == action));
            }
            assert!(
                inputs
                    .iter()
                    .any(|input| input["type"] == "text" && input["text"] == "b")
            );
        }
        if !mobile {
            assert!(inputs.iter().any(
                |input| input["type"] == "navigate" && input["url"] == "https://typed.example/"
            ));
            for contents in [
                "local Ctrl paste",
                "local Insert paste",
                "local Command paste",
            ] {
                assert!(
                    inputs
                        .iter()
                        .any(|input| input["type"] == "text" && input["text"] == contents)
                );
            }
            assert!(!inputs.iter().any(|input| input["type"] == "key"
                && matches!(input["key"].as_str(), Some("v" | "V" | "Insert"))));
        }
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
        .child(when(verified, || text("Verified browser viewer requests")))
        .child(right_panel_tabs(host, key.get(), registry, view()))
}

fn stream_at(width: (f32, f32)) {
    let mut app = app_at(width, Root::of::<StreamFixture>());
    app.wait_for("role:button[Open browser]").expect("paired");
    app.press("role:button[Open browser]")
        .expect("open browser");
    app.type_text("label:Search or enter URL", "https://example.test/")
        .expect("address");
    app.press_key("Enter").expect("navigate");
    app.wait_for("You have control")
        .expect("viewer control arrived");
    assert_eq!(
        app.screen()
            .count("id:preview-jpeg")
            .expect("JPEG selector"),
        0
    );
    app.press_key("f8").expect("arrive with JPEG");
    app.wait_for("id:preview-jpeg")
        .expect("display only the JPEG");
    app.press("id:preview-page").expect("click page");
    app.wait(std::time::Duration::from_millis(160))
        .expect("viewer layout");
    if width == PHONE {
        app.drag("id:preview-page", point(px(0.0), px(-24.0)))
            .expect("pan page");
        app.press_key("f7").expect("reply to the editability probe");
        app.press_key("b")
            .expect("input committed by the focused text field");
        app.press_key("Backspace")
            .expect("delete through the mobile field");
        app.press_key("Enter")
            .expect("submit through the mobile field");
    } else {
        app.clear("label:Search or enter URL")
            .expect("clear current address");
        app.type_text("label:Search or enter URL", "https://typed.example/")
            .expect("draft new address");
        app.press_key("f6")
            .expect("receive remote navigation while typing");
        app.wait_until(
            "navigation event applied while editing",
            std::time::Duration::from_secs(2),
            |screen| screen.enabled("role:button[Back]") == Ok(true),
        )
        .expect("remote navigation updated browser history");
        app.press_key("Enter").expect("navigate to the draft");
        app.press("id:preview-page").expect("focus browser surface");
        app.press_key("a").expect("forward hardware key");
        app.set_clipboard(Some("local Ctrl paste".into()));
        app.press_key("ctrl+v")
            .expect("paste local text using Control");
        app.set_clipboard(Some("local Insert paste".into()));
        app.press_key("shift+Insert")
            .expect("paste local text using Insert");
        app.set_clipboard(Some("local Command paste".into()));
        app.press_key("cmd+v")
            .expect("paste local text using Command");
        app.press_key("ctrl+c").expect("request a page copy");
        app.clear("label:Search or enter URL")
            .expect("leave the browser surface");
        app.set_clipboard(Some("newer local text".into()));
        app.press("id:preview-page").expect("return to the browser");
        app.press_key("ctrl+c")
            .expect("attempt a second copy before the first reply");
        app.press_key("f5").expect("receive a stale copy reply");
        assert_eq!(app.screen().clipboard(), Some("newer local text"));
        app.press("id:preview-page").expect("focus before copying");
        app.press_key("ctrl+c").expect("copy from page");
        app.press_key("f5").expect("receive copied text from page");
        assert_eq!(app.screen().clipboard(), Some("selected page text"));
        app.wheel("id:preview-page", point(px(0.0), px(24.0)))
            .expect("scroll page");
    }
    if width == DESKTOP {
        app.press_keys(&["a"; 500])
            .expect("deliver a 500-event input burst");
        assert!(
            app.screen()
                .matches("id:preview-jpeg")
                .expect("JPEG selector"),
            "burst broke the stream"
        );
    }
    app.press_key("f9").expect("verify socket");
    app.wait_for("Verified browser viewer requests")
        .expect("requests reached socket");
    app.press_key("f4")
        .expect("peer closes the socket for policy");
    app.wait_for("Pair this client again with preview access to control browser previews.")
        .expect("policy closure state");
}

#[test]
fn desktop_streams_a_preview_and_forwards_input() {
    stream_at(DESKTOP);
}

#[test]
fn phone_streams_a_preview_and_forwards_input() {
    stream_at(PHONE);
}
