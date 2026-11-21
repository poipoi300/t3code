//! The mounted panel paints only JPEGs and drives the browser viewer's scoped socket.

use crate::{
    pr_2::app_at,
    support::{DESKTOP, PHONE},
};
use futures::{FutureExt, future::LocalBoxFuture, stream::LocalBoxStream};
use nsh_core::{platform::Lifecycle, prelude::*};
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
    retry: Option<BrowserWire>,
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
            let browser = if let Some(retry) = &self.retry
                && !self.browser.requests().is_empty()
            {
                retry
            } else {
                &self.browser
            };
            futures::future::ready(browser.connect(request)).boxed_local()
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
    stream_fixture_content(false)
}

#[component]
fn NodeRequiredFixture() -> impl IntoElement {
    stream_fixture_content(true)
}

fn stream_fixture_content(node_required: bool) -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let owner = Owner::current();
    let run: Spawner = Rc::new(move |task| {
        owner.run(|| {
            spawn(task);
        });
    });
    let server = FakeEnvironment::new("stream-host", run.clone());
    server.set_permissions(&[
        "orchestration:read",
        "orchestration:operate",
        "preview:operate",
    ]);
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
    let retry = node_required.then(BrowserWire::default);
    if node_required {
        assert!(browser.peer_close(
            1011,
            r#"{"_tag":"PreviewStreamNodeRequired","message":"server diagnostic"}"#,
        ));
    }
    let available = retry.as_ref().unwrap_or(&browser);
    assert!(available.text(
        json!({"type":"control","controller":"you","generation":1,"canOperate":true,"dialog":null})
    ));
    assert!(available.text(json!({"type":"viewport","width":800,"height":600})));
    assert!(
        available.binary(b"<script>throw new Error('never interpret page HTML')</script>".to_vec())
    );

    let store = ClientStore::default();
    let client = ClientRuntime::new(
        StreamPlatform {
            environment: server.platform(),
            browser: browser.clone(),
            retry: retry.clone(),
        },
        store.clone(),
        OrchestrationV2CreationSource::Web,
        run.clone(),
    );
    provide_context(Rt::new(store, Rc::new(client.clone()), Choice::Client));
    provide_context(Navigator::new(Route::Home));
    let paired = signal(false);
    let pair = client.pair(server.pairing());
    let drafts = client.composer_drafts();
    run(async move {
        drafts.load().await.expect("load composer drafts");
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
    let reply_index = stored(0);
    on_key("f5", move |_| {
        let index = reply_index.with(|index| {
            let current = *index;
            *index += 1;
            current
        });
        let (gesture, text) = [
            (0, "old selection"),
            (1, "selected page text"),
            (2, ""),
            (2, "replayed text"),
            (3, "reply after leaving the app"),
        ][index];
        let inputs = fixture.sent();
        let request = inputs
            .iter()
            .filter(|input| {
                input["type"] == "key"
                    && input["action"] == "down"
                    && matches!(input["key"].as_str(), Some("c" | "x"))
            })
            .nth(gesture)
            .expect("copy or cut key sent");
        let request_id = request["requestId"].as_str().expect("gesture ID");
        assert!(!request_id.is_empty());
        assert!(fixture.text(json!({"type":"clipboard","text":text,"requestId":request_id})));
    });
    let fixture = browser.clone();
    on_key("f3", move |_| {
        assert!(fixture.text(json!({"type":"clipboard","text":"untagged text"})));
        assert!(
            fixture.text(json!({"type":"clipboard","text":"unknown text","requestId":"unknown"}))
        );
    });
    if let Some(retry) = retry {
        let fixture = browser.clone();
        on_key("f2", move |_| {
            for wire in [&fixture, &retry] {
                let requests = wire.requests();
                assert_eq!(requests.len(), 1);
                assert!(requests[0].contains("nodeRequirement=close"));
                assert!(requests[0].contains("threadId=thread&tabId=stream-tab"));
            }
        });
    }
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
    let verified_annotation = signal(false);
    let drafts = client.composer_drafts();
    let annotation_wire = browser.clone();
    on_key("f1", move |_| {
        let target = key.get();
        let document = drafts.document();
        let draft = &document.borrow()["draftsByThreadKey"][drafts.thread_draft_key(&target)];
        verify_region_draft(draft);
        assert!(
            !annotation_wire
                .sent()
                .iter()
                .any(|input| input["type"] == "mouse" || input["type"] == "wheel")
        );
        verified_annotation.set(true);
    });
    let empty_annotation = signal(false);
    let drafts = client.composer_drafts();
    on_key("f2", move |_| {
        let target = key.get();
        let document = drafts.document();
        let draft = &document.borrow()["draftsByThreadKey"][drafts.thread_draft_key(&target)];
        assert!(
            draft["previewAnnotations"]
                .as_array()
                .is_none_or(Vec::is_empty)
        );
        assert!(draft["images"].as_array().is_none_or(Vec::is_empty));
        empty_annotation.set(true);
    });
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
        .child(when(verified_annotation, || {
            text("Verified preview annotation")
        }))
        .child(when(empty_annotation, || text("Draft has no annotation")))
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
        app.press_key("ctrl+x").expect("cut before the first reply");
        app.press_key("f3").expect("receive unrelated replies");
        assert_eq!(app.screen().clipboard(), Some("newer local text"));
        app.press_key("f5").expect("receive the old copy reply");
        assert_eq!(app.screen().clipboard(), Some("newer local text"));
        app.press_key("f5").expect("receive the matching cut reply");
        assert_eq!(app.screen().clipboard(), Some("selected page text"));
        app.press_key("ctrl+c").expect("copy with no selection");
        app.press_key("f5").expect("complete an empty selection");
        app.press_key("f5").expect("receive a duplicate reply");
        assert_eq!(app.screen().clipboard(), Some("selected page text"));
        app.press_key("ctrl+c")
            .expect("request copy before switching apps");
        app.set_environment("leave app", |env| env.lifecycle = Lifecycle::Inactive)
            .expect("leave the app without changing the focused field");
        app.set_clipboard(Some("copied in another app".into()));
        app.set_environment("return to app", |env| env.lifecycle = Lifecycle::Active)
            .expect("return to the preview");
        app.press_key("f5").expect("receive the retired copy reply");
        assert_eq!(app.screen().clipboard(), Some("copied in another app"));
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

fn retry_after_installing_node(width: (f32, f32)) {
    let mut app = app_at(width, Root::of::<NodeRequiredFixture>());
    app.wait_for("role:button[Open browser]").expect("paired");
    app.press("role:button[Open browser]")
        .expect("open browser");
    app.type_text("label:Search or enter URL", "https://example.test/")
        .expect("address");
    app.press_key("Enter").expect("open viewer");
    app.wait_for("Server browser automation requires Node.js and playwright-core.")
        .expect("show the host requirement");
    app.press("role:button[Try again]")
        .expect("retry the same tab");
    app.wait_for("You have control")
        .expect("viewer connected after retry");
    app.press_key("f2")
        .expect("verify diagnostic opt-in on both requests");
}

#[test]
fn desktop_shows_the_node_requirement_and_retries() {
    retry_after_installing_node(DESKTOP);
}

#[test]
fn phone_shows_the_node_requirement_and_retries() {
    retry_after_installing_node(PHONE);
}

fn verify_region_draft(draft: &serde_json::Value) {
    let annotation = &draft["previewAnnotations"][0];
    let image = &draft["images"][0];
    assert_eq!(annotation["id"], image["id"]);
    assert_eq!(annotation["pageUrl"], "https://example.test/");
    assert!(
        annotation["comment"]
            .as_str()
            .expect("annotation note")
            .contains("Check this region")
    );
    // The fixture frame is 8 x 6 pixels of an 800 x 600 CSS page, so one frame pixel is 100 CSS
    // pixels each way. The crop is the marked region of the frame.
    let rect = &annotation["regions"][0]["rect"];
    let screenshot = &annotation["screenshot"];
    assert_eq!(&screenshot["cropRect"], rect);
    let (crop_w, crop_h) = (
        screenshot["width"].as_f64().expect("crop width"),
        screenshot["height"].as_f64().expect("crop height"),
    );
    assert!((1.0..=8.0).contains(&crop_w) && (1.0..=6.0).contains(&crop_h));
    let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
    let number = |name: &str| rect[name].as_f64().expect("rect number");
    assert!(close(number("width"), crop_w * 100.0));
    assert!(close(number("height"), crop_h * 100.0));
    let (frame_x, frame_y) = (number("x") / 100.0, number("y") / 100.0);
    assert!(close(frame_x, frame_x.round()) && close(frame_y, frame_y.round()));
    assert!(frame_x + crop_w <= 8.0 + 1e-9 && frame_y + crop_h <= 6.0 + 1e-9);
    assert!(
        screenshot["dataUrl"]
            .as_str()
            .expect("screenshot data")
            .starts_with("data:image/png;base64,")
    );
    assert!(
        annotation["createdAt"]
            .as_str()
            .expect("created at")
            .ends_with('Z')
    );
    assert_eq!(image["mimeType"], "image/png");
    assert!(
        image["previewUrl"]
            .as_str()
            .expect("image source")
            .starts_with("data:image/png;base64,")
    );
}

fn highlight_preview_region(width: (f32, f32), live: bool) {
    mark_preview_region(width, live, Finish::Button);
}

#[derive(Clone, Copy)]
enum Finish {
    Button,
    EnterInNote,
    EscapeInNote,
}

fn mark_preview_region(width: (f32, f32), live: bool, finish: Finish) {
    let root = if live {
        Root::of::<LiveAnnotationFixture>()
    } else {
        Root::of::<StreamFixture>()
    };
    let mut app = app_at(width, root);
    app.wait_for("role:button[Open browser]").expect("paired");
    app.press("role:button[Open browser]")
        .expect("open browser");
    app.type_text("label:Search or enter URL", "https://example.test/")
        .expect("address");
    app.press_key("Enter").expect("open viewer");
    if live {
        app.wait_for("role:button[Take control]")
            .expect("real viewer connected");
        app.press("role:button[Take control]")
            .expect("take control");
    }
    app.wait_for("You have control").expect("viewer control");
    if !live {
        app.press_key("f8").expect("stream a JPEG");
    }
    app.wait_for("id:preview-jpeg")
        .expect("stream frame visible");
    app.press("role:button[Annotate preview]")
        .expect("open annotation controls");
    app.drag("id:preview-region-picker", point(px(40.0), px(30.0)))
        .expect("highlight part of the frame");
    app.wait_for("id:preview-region-mark")
        .expect("show the highlighted region");
    app.type_text("label:Annotation note", "Check this region")
        .expect("type an annotation note");
    match finish {
        Finish::Button => app
            .press("role:button[Attach annotation]")
            .expect("add the region and capture to the thread draft"),
        Finish::EnterInNote => app
            .press_key("Enter")
            .expect("Enter attaches from the note"),
        Finish::EscapeInNote => {
            app.press_key("escape").expect("Escape leaves the editor");
            app.wait_for("role:button[Annotate preview]")
                .expect("back on the stream");
            assert!(
                !app.screen()
                    .matches("id:preview-region-picker")
                    .expect("picker selector")
            );
            app.press_key("f2").expect("verify the draft is untouched");
            app.wait_for("Draft has no annotation")
                .expect("an escaped editor attaches nothing");
            return;
        }
    }
    app.press_key("f1").expect("verify draft transport");
    app.wait_for("Verified preview annotation")
        .expect("draft owns capture and region");
}

#[test]
fn annotation_highlights_the_stream_at_desktop_width() {
    highlight_preview_region(DESKTOP, false);
}

#[test]
fn annotation_highlights_the_stream_at_phone_width() {
    highlight_preview_region(PHONE, false);
}

#[test]
fn enter_in_the_note_attaches_like_the_web_editor() {
    mark_preview_region(DESKTOP, false, Finish::EnterInNote);
}

#[test]
fn escape_in_the_note_leaves_without_attaching() {
    mark_preview_region(DESKTOP, false, Finish::EscapeInNote);
}

// A private server harness supplies a pairing URL written to a mode-0600 file.
// Each ignored test uses its own server and ephemeral client key store.
#[component]
fn LiveAnnotationFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let info_path = std::env::var_os("T3_PREVIEW_TEST_INFO").expect("private fixture info file");
    let info: serde_json::Value =
        serde_json::from_slice(&std::fs::read(info_path).expect("read fixture info"))
            .expect("fixture info JSON");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .expect("test network runtime");
    let native = runtime
        .block_on(t3_platform::NativePlatform::open(
            t3_platform::NativeConfig::new(
                info["clientDir"]
                    .as_str()
                    .expect("private client directory"),
                Box::<t3_platform::EphemeralSecretStore>::default(),
            ),
        ))
        .expect("open test native platform");
    let _runtime = stored(runtime);
    let owner = Owner::current();
    let run: Spawner = Rc::new(move |task| {
        owner.run(|| {
            spawn(task);
        });
    });
    let store = ClientStore::default();
    let client = ClientRuntime::new(
        native,
        store.clone(),
        OrchestrationV2CreationSource::Web,
        run.clone(),
    );
    provide_context(Rt::new(store, Rc::new(client.clone()), Choice::Client));
    provide_context(Navigator::new(Route::Home));
    provide_client_services(&client);
    let registry = use_context::<PanelRegistry>().expect("app panel registry");
    let host = panel_host();
    let paired = signal(None::<ThreadKey>);
    let pair = client.pair(t3_client::connection::PairingInput {
        pairing_url: Some(info["pairingUrl"].as_str().expect("pairing URL").into()),
        ..Default::default()
    });
    let drafts = client.composer_drafts();
    let loading = drafts.clone();
    run(async move {
        loading.load().await.expect("load real composer drafts");
        let environment = pair.await.expect("pair real preview host");
        paired.set(Some(ThreadKey {
            environment,
            thread: ThreadId("thread".into()),
        }));
    }
    .boxed_local());
    let verified = signal(false);
    on_key("f1", move |_| {
        let key = paired.get_untracked().expect("paired test environment");
        let document = drafts.document();
        verify_region_draft(&document.borrow()["draftsByThreadKey"][drafts.thread_draft_key(&key)]);
        verified.set(true);
    });
    view()
        .w_full()
        .h_full()
        .child(when(
            move || paired.with(Option::is_some),
            move || {
                let key = stored(paired.get().expect("paired fixture"));
                view()
                    .flex_1()
                    .child(
                        nsh_base::button()
                            .accessibility_label("Open browser")
                            .on_press(move || registry.open(SurfaceKind::Browser, &key.get()))
                            .child(text("Open browser")),
                    )
                    .child(right_panel_tabs(host, key.get(), registry, view()))
            },
        ))
        .child(when(verified, || text("Verified preview annotation")))
}

#[test]
#[ignore = "requires a private real preview server and worker"]
fn annotation_highlights_live_viewer_at_desktop_width() {
    highlight_preview_region(DESKTOP, true);
}

#[test]
#[ignore = "requires a private real preview server and worker"]
fn annotation_highlights_live_viewer_at_phone_width() {
    highlight_preview_region(PHONE, true);
}
