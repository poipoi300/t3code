//! Sharing & access through a paired runtime with no access-management grant.
use crate::set_shell::{Remote, start_connected};
use crate::support::{DESKTOP, PHONE};
use t3_ui::state::nav::SettingsPage;

fn narrow_session(window: (f32, f32)) {
    let mut app = start_connected(window, SettingsPage::Access, Remote::default());
    app.press("role:button[Permissions]")
        .expect("session permissions");
    app.wait_for("These permissions apply to this client’s current connection.")
        .expect("selected environment session");
    app.wait_for("Change environment settings")
        .expect("granted scope");
    assert!(!app.screen().shows("Authorized clients"));
    assert!(!app.screen().shows("Revoke"));
}

#[test]
fn session_permissions_desktop() {
    narrow_session(DESKTOP);
}

#[test]
fn session_permissions_phone() {
    narrow_session(PHONE);
}

use futures::{FutureExt, StreamExt, channel::mpsc, stream::LocalBoxStream};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use serde_json::{Value, json};
use std::{cell::RefCell, rc::Rc};
use t3_client::platform::{HttpRequest, HttpResponse};
use t3_client::{
    ClientStore,
    composition::{ClientRuntime, Spawner},
    platform::*,
    testing::{
        Frame,
        environment::{EnvironmentPlatform, FakeEnvironment},
    },
};
use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;
use t3_ui::{
    runtime::{Rt, host::Choice},
    screens::settings::shell,
    state::nav::{Navigator, Route},
    theme,
};

type StreamReply = (Value, mpsc::UnboundedSender<Frame>);

// Add the auth stream and its HTTP endpoints to the existing paired server transport.
// Forward every other frame to the ordinary server, including real settings mutations.
#[derive(Clone)]
struct AccessWire {
    platform: EnvironmentPlatform,
    snapshot: Value,
    session_status: Rc<std::cell::Cell<u16>>,
    relay_failure: Rc<std::cell::Cell<bool>>,
    delay_approval: Rc<std::cell::Cell<bool>>,
    calls: Rc<RefCell<Vec<(String, Value)>>>,
    stream: Rc<RefCell<Option<StreamReply>>>,
    config_stream: Rc<RefCell<Option<StreamReply>>>,
}
struct AccessSocket {
    socket: Box<dyn WebSocket>,
    wire: AccessWire,
    sender: mpsc::UnboundedSender<Frame>,
    incoming: mpsc::UnboundedReceiver<Frame>,
}
impl WebSocket for AccessSocket {
    fn send(&mut self, frame: Frame) -> HostFuture<'_, ()> {
        if let Frame::Text(raw) = &frame
            && let Ok(message) = serde_json::from_str::<Value>(raw)
        {
            if message["tag"] == "subscribeServerConfig" {
                self.wire
                    .config_stream
                    .replace(Some((message["id"].clone(), self.sender.clone())));
            }
            if message["tag"] == "subscribeAuthAccess" {
                self.wire
                    .stream
                    .replace(Some((message["id"].clone(), self.sender.clone())));
                self.wire.event(json!({"version":1,"revision":1,"type":"snapshot","payload":self.wire.snapshot}));
                return async { Ok(()) }.boxed_local();
            }
            if message["tag"] == "server.updateSettings" {
                self.wire
                    .calls
                    .borrow_mut()
                    .push(("setting".into(), message["payload"].clone()));
            }
            if self
                .wire
                .stream
                .borrow()
                .as_ref()
                .is_some_and(|(id, _)| id == &message["requestId"])
            {
                return async { Ok(()) }.boxed_local();
            }
        }
        self.socket.send(frame)
    }
    fn receive(&mut self) -> HostFuture<'_, Option<Frame>> {
        async {
            futures::select_biased! {
                frame=self.incoming.next().fuse()=>Ok(frame),
                frame=self.socket.receive().fuse()=>frame,
            }
        }
        .boxed_local()
    }
    fn close(&mut self) -> HostFuture<'_, ()> {
        self.socket.close()
    }
}
impl AccessWire {
    fn event(&self, event: Value) {
        if let Some((id, tx)) = self.stream.borrow().as_ref() {
            let _ = tx.unbounded_send(Frame::Text(
                json!({"_tag":"Chunk","requestId":id,"values":[event]}).to_string(),
            ));
        }
    }
}
impl Storage for AccessWire {
    fn get(&self, a: StorageArea, k: StorageKey) -> HostFuture<'_, Option<Vec<u8>>> {
        self.platform.get(a, k)
    }
    fn write(&self, a: StorageArea, w: Vec<StorageWrite>) -> HostFuture<'_, ()> {
        self.platform.write(a, w)
    }
    fn list(&self, a: StorageArea, n: String) -> HostFuture<'_, Vec<StorageKey>> {
        self.platform.list(a, n)
    }
    fn transaction(&self, t: StorageTransaction) -> HostFuture<'_, bool> {
        self.platform.transaction(t)
    }
}
impl Platform for AccessWire {
    fn now_epoch_ms(&self) -> u64 {
        self.platform.now_epoch_ms()
    }
    fn monotonic_ms(&self) -> u64 {
        self.platform.monotonic_ms()
    }
    fn random_bytes(&self, n: usize) -> Result<Vec<u8>, PlatformError> {
        self.platform.random_bytes(n)
    }
    fn sleep(&self, n: u64) -> futures::future::LocalBoxFuture<'_, ()> {
        self.platform.sleep(n)
    }
    fn yield_to_host(&self) -> futures::future::LocalBoxFuture<'_, ()> {
        self.platform.yield_to_host()
    }
    fn events(&self) -> LocalBoxStream<'static, PlatformEvent> {
        self.platform.events()
    }
    fn host(&self, a: HostAction) -> HostFuture<'_, HostReply> {
        self.platform.host(a)
    }
    fn websocket(&self, r: SocketRequest) -> HostFuture<'_, Box<dyn WebSocket>> {
        async move {
            let socket = self.platform.websocket(r).await?;
            let (sender, incoming) = mpsc::unbounded();
            Ok(Box::new(AccessSocket {
                socket,
                wire: self.clone(),
                sender,
                incoming,
            }) as Box<dyn WebSocket>)
        }
        .boxed_local()
    }
    fn http(&self, r: HttpRequest) -> HostFuture<'_, HttpResponse> {
        if r.url == "https://relay.test/v1/environments" && self.relay_failure.get() {
            return async {
                Ok(HttpResponse {
                    status: 503,
                    headers: Default::default(),
                    body: b"private-relay-error-marker".to_vec(),
                })
            }
            .boxed_local();
        }
        if r.url.ends_with("/api/auth/session") && self.session_status.get() != 0 {
            let status = self.session_status.get();
            return async move {Ok(HttpResponse{status,headers:Default::default(),body:json!({"authenticated":false,"auth":{"policy":"remote-reachable","bootstrapMethods":[],"sessionMethods":[],"sessionCookieName":"session"}}).to_string().into_bytes()})}.boxed_local();
        }
        if r.method == "POST" && r.url.ends_with("/api/auth/pairing-token") {
            let body = serde_json::from_slice(&r.body).unwrap_or(Value::Null);
            self.calls.borrow_mut().push((r.url.clone(), body));
            return async{Ok(HttpResponse{status:200,headers:Default::default(),body:json!({"id":"temporary","credential":"fixture-mint","expiresAt":"2030-01-01T00:00:00.000Z"}).to_string().into_bytes()})}.boxed_local();
        }
        if r.method == "POST"
            && let Some(path) = r.url.strip_prefix("https://server.test/api/auth/")
            && matches!(
                path,
                "clients/revoke"
                    | "clients/revoke-others"
                    | "pairing-links/revoke"
                    | "connect-devices/approve"
                    | "connect-devices/revoke"
            )
        {
            let body = serde_json::from_slice(&r.body).unwrap_or(Value::Null);
            self.calls
                .borrow_mut()
                .push((path.to_owned(), body.clone()));
            if path == "clients/revoke" {
                self.event(json!({"version":1,"revision":2,"type":"clientRemoved","payload":body}));
            }
            if path == "pairing-links/revoke" {
                self.event(
                    json!({"version":1,"revision":3,"type":"pairingLinkRemoved","payload":body}),
                );
            }
            let reply = match path {
                "connect-devices/approve" => json!({"approved":true}),
                "clients/revoke-others" => json!({"revokedCount":2}),
                _ => json!({"revoked":true}),
            };
            let hold = path == "connect-devices/approve" && self.delay_approval.get();
            return async move {
                if hold {
                    self.platform.sleep(1_000).await;
                }
                Ok(HttpResponse {
                    status: 200,
                    headers: Default::default(),
                    body: reply.to_string().into_bytes(),
                })
            }
            .boxed_local();
        }
        self.platform.http(r)
    }
}

fn fixture_snapshot() -> Value {
    const NOW: &str = "2026-01-05T01:02:03.000Z";
    let session = |id: &str, name: &str, subject: &str, current: bool| json!({"sessionId":id,"subject":subject,"scopes":["access:read","access:write"],"permissions":["filesystem:read"],"method":"bearer-access-token","client":{"label":name,"deviceType":"desktop"},"issuedAt":NOW,"expiresAt":NOW,"lastConnectedAt":null,"connected":current,"current":current});
    let request = |id: &str, code: &str| json!({"id":id,"scopes":[],"subject":"cloud-connect","createdAt":NOW,"expiresAt":NOW,"connectDeviceRequest":{"verificationCode":code,"client":{"label":"Pending tablet","deviceType":"tablet"}}});
    json!({"clientSessions":[session("self-id","Workstation","local",true),session("ordinary-id","Third-party client","agent",false),session("cloud-id","Connect laptop","cloud-connect",false)],
    "pairingLinks":[request("request-a","ABCD-2345"),request("request-b","CDEF-6789")],"connectDevices":[{"proofKeyThumbprint":"private-thumbprint-marker","verificationCode":"PQRS-6789","client":{"label":"Trusted phone","deviceType":"mobile"},"approvedAt":NOW}]})
}
#[derive(Clone, PartialEq, Props)]
struct AccessFixtureProps {
    #[prop(default = true)]
    read: bool,
    #[prop(default = true)]
    write: bool,
    approval: bool,
    save_fails: bool,
    legacy: bool,
    access_only: bool,
}
#[component]
fn AccessFixture(props: AccessFixtureProps) -> impl IntoElement {
    theme::install(&theme::ThemeChoice::default());
    provide_context(Navigator::new(Route::Settings(SettingsPage::Access)));
    let owner = Owner::current();
    let spawner: Spawner = Rc::new(move |task| {
        owner.run(|| {
            spawn(task);
        })
    });
    let server = FakeEnvironment::new("env", spawner.clone());
    server.set_permissions(if props.access_only {
        &["access:read", "access:write"][..]
    } else if !props.read {
        &["orchestration:read", "access:write"][..]
    } else if props.write {
        &["orchestration:read", "access:read", "access:write"][..]
    } else {
        &["orchestration:read", "access:read"][..]
    });
    server.edit_config(|c| {
        if props.legacy || props.access_only {
            c["settings"]
                .as_object_mut()
                .expect("settings")
                .remove("requireConnectDeviceApproval");
        } else {
            c["settings"]["requireConnectDeviceApproval"] = props.approval.into();
        }
    });
    if props.save_fails {
        server.reply(
            "server.updateSettings",
            Err(json!({"_tag":"Error","message":"fixture failed"})),
        );
    }
    server.reply(
        "server.updateSettings",
        Ok(
            serde_json::to_value(t3_contracts::settings::ServerSettings {
                require_connect_device_approval: true,
                ..Default::default()
            })
            .expect("settings"),
        ),
    );
    let mut snapshot = fixture_snapshot();
    if props.legacy {
        snapshot
            .as_object_mut()
            .expect("snapshot")
            .remove("connectDevices");
    }
    let wire = AccessWire {
        platform: server.platform(),
        snapshot,
        session_status: Default::default(),
        relay_failure: Default::default(),
        delay_approval: Default::default(),
        calls: Default::default(),
        stream: Default::default(),
        config_stream: Default::default(),
    };
    let store = ClientStore::default();
    let client = ClientRuntime::new(
        wire.clone(),
        store.clone(),
        OrchestrationV2CreationSource::Web,
        spawner,
    );
    provide_context(Rt::new(store, Rc::new(client.clone()), Choice::Client));
    spawn(client.pair(server.pairing()));
    let expected_config_reads = signal(None);
    let config_server = server.clone();
    on_key("ctrl+shift+g", move |_| {
        expected_config_reads.set(Some(config_server.calls("server.getConfig").len()));
        for _ in 0..3 {
            config_server.reply(
                "server.getConfig",
                Err(json!({"_tag":"Error","message":"configuration unavailable"})),
            );
        }
    });
    let config_stream = wire.config_stream.clone();
    for (key, value) in [("ctrl+shift+t", Some(false)), ("ctrl+shift+u", None)] {
        let config_stream = config_stream.clone();
        on_key(key, move |_| {
            let mut settings =
                serde_json::to_value(t3_contracts::settings::ServerSettings::default())
                    .expect("serialize fixture settings");
            let fields = settings.as_object_mut().expect("settings object");
            if let Some(value) = value {
                fields.insert("requireConnectDeviceApproval".into(), json!(value));
            } else {
                fields.remove("requireConnectDeviceApproval");
            }
            let stream = config_stream.borrow();
            let (id, tx) = stream.as_ref().expect("connected config subscription");
            let event =
                json!({"version":1,"type":"settingsUpdated","payload":{"settings":settings}});
            tx.unbounded_send(Frame::Text(
                json!({"_tag":"Chunk","requestId":id,"values":[event]}).to_string(),
            ))
            .expect("deliver config update");
        });
    }
    let delay = wire.delay_approval.clone();
    on_key("ctrl+shift+h", move |_| delay.set(true));
    let fail = wire.session_status.clone();
    on_key("ctrl+shift+f", move |_| fail.set(503));
    let reject = wire.session_status.clone();
    on_key("ctrl+shift+j", move |_| reject.set(200));
    let recovered = wire.clone();
    on_key("ctrl+shift+y", move |_| {
        recovered.session_status.set(0);
        recovered.platform.fake.advance(2_001);
    });
    let check = client.clone();
    let paired = computed(move || {
        t3_ui::runtime::rt().store.track_environments();
        check
            .environments()
            .into_iter()
            .any(|e| e.state.phase == t3_client::connection::Phase::Connected)
    });
    let malformed = wire.clone();
    on_key("ctrl+shift+i", move |_| {
        let mut payload = malformed.snapshot.clone();
        payload["pairingLinks"][0]["connectDeviceRequest"]["verificationCode"] =
            "private-code-marker".into();
        malformed.event(json!({"version":1,"revision":8,"type":"snapshot","payload":payload}));
    });
    let audit = signal(String::new());
    on_key("ctrl+shift+l", move |_| {
        let calls = wire.calls.borrow();
        let log = calls
            .iter()
            .map(|(path, body)| match path.as_str() {
                "setting" => format!("setting={}", body["patch"]["requireConnectDeviceApproval"]),
                "clients/revoke" => {
                    format!("client={}", body["sessionId"].as_str().unwrap_or("missing"))
                }
                "connect-devices/approve" => format!(
                    "approve={}",
                    body["pairingLinkId"].as_str().unwrap_or("missing")
                ),
                "pairing-links/revoke" => {
                    format!("deny={}", body["id"].as_str().unwrap_or("missing"))
                }
                "connect-devices/revoke" => format!(
                    "device={}",
                    body["proofKeyThumbprint"] == "private-thumbprint-marker"
                ),
                other => other.to_owned(),
            })
            .collect::<Vec<_>>()
            .join(",");
        if expected_config_reads
            .get_untracked()
            .is_some_and(|n| server.calls("server.getConfig").len() != n)
        {
            audit.set("Unexpected config read during revocation".into());
        } else {
            audit.set(format!("Audit [{log}]"));
        }
    });
    view()
        .flex_1()
        .child(text(
            move || {
                if paired.get() { "Online fixture" } else { "" }
            },
        ))
        .child(text(move || audit.get()))
        .child(untracked(|| {
            shell::screen_for(SettingsPage::Access, &client)
        }))
}
fn access_app(w: (f32, f32), props: AccessFixtureProps) -> Headless {
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(w.0), px(w.1)),
        ..EngineConfig::new(Root::new::<AccessFixture>(move || props.clone()))
    };
    config.environment.os = if w == PHONE {
        nsh_core::platform::Os::Android
    } else {
        nsh_core::platform::Os::Linux
    };
    config.environment.reduce_motion = true;
    config.environment.idiom = if w == PHONE {
        nsh_core::platform::Idiom::Phone
    } else {
        nsh_core::platform::Idiom::Desktop
    };
    Headless::start(config).expect("access fixture")
}
fn audit(app: &mut Headless, expected: &str) {
    app.press_key("ctrl+shift+l")
        .expect("read sanitized wire calls");
    app.wait_for(&format!("Audit [{expected}]"))
        .expect("ordered request bodies");
}
fn clients(w: (f32, f32)) {
    let mut app = access_app(w, Default::default());
    app.wait_for("role:button[Authorized clients]")
        .expect("authorized clients");
    app.press("role:button[Authorized clients]")
        .expect("expand");
    app.wait_for("This device").expect("current session");
    app.wait_for("3 clients").expect("authorized session count");
    let summary = "id:access-client-0 >> role:button[Client scopes: show 1 scope]";
    if w == DESKTOP {
        app.hover(summary).expect("hover for effective grants");
    } else {
        app.press(summary).expect("tap for effective grants");
    }
    app.wait_for("text:filesystem:read")
        .expect("permissions override scopes");
    if w == PHONE {
        app.press(summary).expect("dismiss scope details");
    } else {
        app.press_key("escape").expect("dismiss scope details");
    }
    assert!(!app.screen().shows("Granted scopes"));
    assert_eq!(
        app.screen()
            .count("id:access-client-0 >> role:button[Revoke]")
            .expect("current"),
        0
    );
    app.press("id:access-client-1 >> role:button[Revoke]")
        .expect("ordinary client immediate");
    audit(&mut app, "client=ordinary-id");
    app.wait_for("Connect laptop")
        .expect("other client survives delta");
    assert!(!app.screen().shows("Third-party client"));
    app.press("id:access-client-1 >> role:button[Revoke]")
        .expect("connect revocation prompt");
    app.wait_for("Revoke Connect laptop?")
        .expect("specific device");
    app.press("role:button[Cancel]").expect("keep access");
    audit(&mut app, "client=ordinary-id");
    app.press("id:access-client-1 >> role:button[Revoke]")
        .expect("open again");
    app.press("role:alertdialog >> role:button[Revoke]")
        .expect("save approval then revoke");
    audit(&mut app, "client=ordinary-id,setting=true,client=cloud-id");
}
fn devices(w: (f32, f32)) {
    let mut app = access_app(
        w,
        AccessFixtureProps {
            approval: true,
            ..Default::default()
        },
    );
    app.wait_for("Waiting for approval · ABCD-2345")
        .expect("comparison code");
    assert!(
        app.screen()
            .texts()
            .iter()
            .all(|value| !value.contains("private-thumbprint-marker"))
    );
    app.press("id:access-request-0 >> role:button[Approve]")
        .expect("approve matching device");
    app.press("id:access-request-1 >> role:button[Deny]")
        .expect("deny other request");
    app.press("id:access-trusted-0 >> role:button[Revoke]")
        .expect("approval already on");
    audit(&mut app, "approve=request-a,deny=request-b,device=true");
}
fn bulk(w: (f32, f32)) {
    let mut app = access_app(
        w,
        AccessFixtureProps {
            save_fails: true,
            ..Default::default()
        },
    );
    app.wait_for("role:button[Revoke others]")
        .expect("bulk control");
    app.press("role:button[Revoke others]")
        .expect("bulk confirmation");
    app.wait_for("Revoke all other clients?")
        .expect("warn about Connect devices");
    app.press("role:alertdialog >> role:button[Revoke]")
        .expect("attempt to require approval");
    audit(&mut app, "setting=true");
    app.wait_for("Revoke all other clients?")
        .expect("failed setting preserves pending revoke");
    app.press("role:checkbox[Require approval for new devices]")
        .expect("explicitly skip approval");
    app.press("role:alertdialog >> role:button[Revoke]")
        .expect("confirm bulk revocation");
    audit(&mut app, "setting=true,clients/revoke-others");
}
fn readonly(w: (f32, f32)) {
    let mut app = access_app(
        w,
        AccessFixtureProps {
            write: false,
            legacy: true,
            ..Default::default()
        },
    );
    app.wait_for("role:button[Authorized clients]")
        .expect("readable");
    app.press("role:button[Authorized clients]")
        .expect("show records");
    app.wait_for("Connect laptop").expect("readable record");
    assert!(
        app.press("id:access-client-1 >> role:button[Revoke]")
            .is_err()
    );
    assert!(!app.screen().shows("T3 Connect devices"));
    assert!(!app.screen().shows("Revoke others"));
    audit(&mut app, "");
}
macro_rules! widths {
    ($d:ident,$p:ident,$run:ident) => {
        #[test]
        fn $d() {
            $run(DESKTOP)
        }
        #[test]
        fn $p() {
            $run(PHONE)
        }
    };
}
widths!(authorized_desktop, authorized_phone, clients);
widths!(devices_desktop, devices_phone, devices);
widths!(bulk_desktop, bulk_phone, bulk);
widths!(readonly_desktop, readonly_phone, readonly);

fn cached_revoke_policy(w: (f32, f32)) {
    for access_only in [true, false] {
        let mut app = access_app(
            w,
            AccessFixtureProps {
                access_only,
                ..Default::default()
            },
        );
        app.wait_for("role:button[Authorized clients]")
            .expect("authorized");
        app.press("role:button[Authorized clients]")
            .expect("expand");
        app.wait_for("Connect laptop").expect("access snapshot");
        app.press_key("ctrl+shift+g")
            .expect("later config reads unavailable");
        let mut sent = Vec::new();
        for (button, expected) in [
            (
                "id:access-client-2 >> role:button[Revoke]",
                "client=cloud-id",
            ),
            ("id:access-trusted-0 >> role:button[Revoke]", "device=true"),
            ("role:button[Revoke others]", "clients/revoke-others"),
        ] {
            app.press(button).expect("authorized revoke");
            if !access_only {
                app.wait_for("role:alertdialog")
                    .expect("cached explicit false requires confirmation");
                audit(&mut app, &sent.join(","));
                app.press("role:checkbox[Require approval for new devices]")
                    .expect("skip saving");
                app.press("role:alertdialog >> role:button[Revoke]")
                    .expect("confirm");
            }
            sent.push(expected);
            audit(&mut app, &sent.join(","));
        }
    }
}
widths!(
    cached_policy_desktop,
    cached_policy_phone,
    cached_revoke_policy
);

fn live_revoke_policy(w: (f32, f32)) {
    let mut app = access_app(
        w,
        AccessFixtureProps {
            approval: true,
            ..Default::default()
        },
    );
    app.wait_for("role:button[Revoke others]")
        .expect("access snapshot");
    app.press_key("ctrl+shift+t").expect("turn approval off");
    app.press("role:button[Revoke others]")
        .expect("bulk revoke");
    app.wait_for("Revoke all other clients?")
        .expect("updated policy requires confirmation");
    app.press("role:button[Cancel]")
        .expect("keep other clients");
    audit(&mut app, "");
    app.press_key("ctrl+shift+u")
        .expect("legacy settings received");
    app.press("role:button[Revoke others]")
        .expect("bulk revoke");
    audit(&mut app, "clients/revoke-others");
}
widths!(live_policy_desktop, live_policy_phone, live_revoke_policy);

fn permission_failure(w: (f32, f32)) {
    for (key, message) in [
        (
            "ctrl+shift+f",
            "Could not verify your permissions. Retrying automatically.",
        ),
        (
            "ctrl+shift+j",
            "This environment did not accept your session. Reconnect to try again.",
        ),
    ] {
        let mut app = access_app(
            w,
            AccessFixtureProps {
                approval: true,
                ..Default::default()
            },
        );
        app.wait_for("role:button[Permissions]").expect("paired");
        app.wait_for("Online fixture").expect("verified connection");
        app.press_key(key).expect("session response changes");
        app.press("role:button[Permissions]")
            .expect("fetch current permissions");
        app.wait_for(message).expect("verification outcome");
        if key == "ctrl+shift+f" {
            app.press_key("ctrl+shift+y").expect("pass retry deadline");
            app.wait_for("These permissions apply to this client’s current connection.")
                .expect("retried automatically");
        }
    }
}
widths!(verification_desktop, verification_phone, permission_failure);

#[derive(Clone, PartialEq, Props)]
struct MachinesProps {
    source_write: bool,
    target_write: bool,
    relay_only: bool,
}
#[component]
fn Machines(props: MachinesProps) -> impl IntoElement {
    use t3_client::{services::settings::PeerDestination, types::EnvironmentId};
    use t3_contracts::auth::AuthMcpClientAccess;
    theme::install(&theme::ThemeChoice::default());
    provide_context(Navigator::new(Route::Settings(SettingsPage::Connections)));
    let owner = Owner::current();
    let spawner: Spawner = Rc::new(move |task| {
        owner.run(|| {
            spawn(task);
        })
    });
    let target = FakeEnvironment::new("target", spawner.clone());
    let source = FakeEnvironment::at_origin("source", "https://source.test", spawner.clone());
    for (host, write) in [(&source, props.source_write), (&target, props.target_write)] {
        host.set_permissions(if write {
            &["access:read", "access:write", "orchestration:read"][..]
        } else {
            &["access:read", "orchestration:read"][..]
        });
    }
    source.edit_config(|c| {
        c["environment"]["capabilities"]["peerLinks"] = true.into();
        c["settings"]["linkedEnvironmentsEnabled"] = true.into();
    });
    source.register("peerLinks.list", t3_client::testing::CallKind::Unary);
    source.reply("peerLinks.list", Ok(json!({"links":[]})));
    source.register("peerLinks.link", t3_client::testing::CallKind::Unary);
    source.reply("peerLinks.link",Ok(json!({"environmentId":"target","label":"Fake host","urls":["https://server.test"],"access":"approval-required","linkedAt":"2026-01-01T00:00:00Z","expiresAt":"2030-01-01T00:00:00Z","lastReachedAt":null,"lastError":null})));
    let wire = AccessWire {
        platform: target.platform().with_environment(&source),
        snapshot: Value::Null,
        session_status: Default::default(),
        relay_failure: Default::default(),
        delay_approval: Default::default(),
        calls: Default::default(),
        stream: Default::default(),
        config_stream: Default::default(),
    };
    let store = ClientStore::default();
    let client = ClientRuntime::with_relay(
        wire.clone(),
        store.clone(),
        OrchestrationV2CreationSource::Web,
        spawner,
        FakeEnvironment::relay_config(),
    );
    provide_context(Rt::new(store, Rc::new(client.clone()), Choice::Client));
    let c = client.clone();
    let first = source.pairing();
    let second = target.pairing();
    let ready = signal(false);
    spawn(async move {
        c.pair(first).await.expect("source");
        if !props.relay_only {
            c.pair(second).await.expect("target");
        }
        ready.set(true);
    });
    let c = client.clone();
    let result = signal(String::new());
    on_key("ctrl+shift+m", move |_| {
        let work = c.link_settings_peer(
            EnvironmentId("source".into()),
            PeerDestination::Machine(EnvironmentId("target".into())),
            None,
            AuthMcpClientAccess::ApprovalRequired,
        );
        let wire = wire.clone();
        let server = source.clone();
        spawn(async move {
            let ok = work.await.is_ok();
            let calls = wire.calls.borrow();
            let mints = calls
                .iter()
                .filter(|(url, _)| url.ends_with("/api/auth/pairing-token"))
                .count();
            let correct = calls.iter().any(|(url, body)| {
                url == "https://server.test/api/auth/pairing-token"
                    && body["label"] == "T3 Code link"
            });
            let links = server.calls("peerLinks.link");
            let bound = links.iter().any(|v| {
                v["expectedEnvironmentId"] == "target"
                    && v["expectedLabel"] == "Fake host"
                    && v["pairingCode"] == "fixture-mint"
            });
            result.set(format!(
                "Mint {mints} directed {correct} links {} bound {bound} success {ok}",
                links.len()
            ));
        });
    });
    on_key("ctrl+shift+s", move |_| target.sign_in(true));
    view()
        .flex_1()
        .child(text(move || {
            if ready.get() {
                "Machines paired"
            } else {
                "Pairing"
            }
        }))
        .child(text(move || result.get()))
        .child(untracked(|| {
            shell::screen_for(SettingsPage::Connections, &client)
        }))
}
fn machine_app(
    w: (f32, f32),
    source_write: bool,
    target_write: bool,
    relay_only: bool,
) -> Headless {
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(w.0), px(w.1)),
        ..EngineConfig::new(Root::new::<Machines>(move || MachinesProps {
            source_write,
            target_write,
            relay_only,
        }))
    };
    config.environment.idiom = if w == PHONE {
        nsh_core::platform::Idiom::Phone
    } else {
        nsh_core::platform::Idiom::Desktop
    };
    Headless::start(config).expect("machine fixture")
}
fn run_machine(w: (f32, f32), source: bool, target: bool) {
    let mut app = machine_app(w, source, target, false);
    app.wait_for("Machines paired").expect("both paired");
    app.wait_for("No linked environments")
        .expect("source connected");
    app.press_key("ctrl+shift+m").expect("request known target");
    let expected = if source && target {
        "Mint 1 directed true links 1 bound true success true"
    } else {
        "Mint 0 directed false links 0 bound false success false"
    };
    app.wait_for(expected)
        .expect("mint and link require both destination and source scopes");
}
fn known_machine(w: (f32, f32)) {
    run_machine(w, true, true);
    run_machine(w, true, false);
}
fn source_read_only(w: (f32, f32)) {
    run_machine(w, false, true);
}
widths!(known_machine_desktop, known_machine_phone, known_machine);
widths!(
    source_read_only_desktop,
    source_read_only_phone,
    source_read_only
);
fn relay_on_open(w: (f32, f32)) {
    let mut app = machine_app(w, true, false, true);
    app.wait_for("No linked environments")
        .expect("source connected");
    app.press_key("ctrl+shift+s")
        .expect("cloud account now available");
    app.press("role:button[Link]").expect("open form");
    app.wait_for("role:combobox[Machine]")
        .expect("discovered machine arrives while form open");
}
widths!(relay_on_open_desktop, relay_on_open_phone, relay_on_open);

#[derive(Clone, PartialEq, Props)]
struct CloudProps {
    direct: bool,
}
#[component]
fn Cloud(props: CloudProps) -> impl IntoElement {
    theme::install(&theme::ThemeChoice::default());
    provide_context(Navigator::new(Route::Settings(SettingsPage::Connections)));
    let owner = Owner::current();
    let spawner: Spawner = Rc::new(move |task| {
        owner.run(|| {
            spawn(task);
        })
    });
    let cloud = FakeEnvironment::new("target", spawner.clone());
    cloud.sign_in(true);
    let wire = AccessWire {
        platform: cloud.platform(),
        snapshot: Value::Null,
        session_status: Default::default(),
        relay_failure: Default::default(),
        delay_approval: Default::default(),
        calls: Default::default(),
        stream: Default::default(),
        config_stream: Default::default(),
    };
    wire.relay_failure.set(!props.direct);
    let repair = wire.relay_failure.clone();
    on_key("ctrl+shift+y", move |_| repair.set(false));
    let store = ClientStore::default();
    let client = ClientRuntime::with_relay(
        wire,
        store.clone(),
        OrchestrationV2CreationSource::Web,
        spawner,
        FakeEnvironment::relay_config(),
    );
    provide_context(Rt::new(store, Rc::new(client.clone()), Choice::Client));
    let ready = signal(!props.direct);
    if props.direct {
        let c = client.clone();
        spawn(async move {
            c.pair(cloud.pairing()).await.expect("direct route");
            ready.set(true);
        });
    }
    let client = stored(client);
    view().flex_1().child(when(
        move || ready.get(),
        move || shell::screen_for(SettingsPage::Connections, &client.get()),
    ))
}
fn cloud_app(w: (f32, f32), direct: bool) -> Headless {
    let mut cfg = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(w.0), px(w.1)),
        ..EngineConfig::new(Root::new::<Cloud>(move || CloudProps { direct }))
    };
    cfg.environment.idiom = if w == PHONE {
        nsh_core::platform::Idiom::Phone
    } else {
        nsh_core::platform::Idiom::Desktop
    };
    Headless::start(cfg).expect("cloud fixture")
}
fn cloud_retry(w: (f32, f32)) {
    let mut app = cloud_app(w, false);
    app.wait_for("Could not load T3 Connect environments")
        .expect("initial discovery failure");
    assert!(
        app.screen()
            .texts()
            .iter()
            .all(|value| !value.contains("private-relay-error-marker"))
    );
    assert!(!app.screen().shows("No saved remote environments"));
    app.press_key("ctrl+shift+y").expect("relay restored");
    app.press("role:button[Try again]")
        .expect("refresh discovery");
    app.wait_for("T3 Connect · Not added · Relay online")
        .expect("fresh discovery");
}
widths!(cloud_retry_desktop, cloud_retry_phone, cloud_retry);
fn cloud_fallback(w: (f32, f32)) {
    let mut app = cloud_app(w, true);
    app.wait_for("T3 Connect · Saved without T3 Connect · Relay online")
        .expect("direct route eligible for relay");
    app.press("role:button[Add route]")
        .expect("save relay fallback");
    app.wait_for("role:button[2 routes]")
        .expect("both routes saved");
    assert!(!app.screen().shows("Saved without T3 Connect"));
}
widths!(cloud_fallback_desktop, cloud_fallback_phone, cloud_fallback);

fn legacy_revoke(w: (f32, f32)) {
    let mut app = access_app(
        w,
        AccessFixtureProps {
            legacy: true,
            ..Default::default()
        },
    );
    app.wait_for("role:button[Authorized clients]")
        .expect("legacy client list");
    app.press("role:button[Authorized clients]")
        .expect("expand");
    app.wait_for("Connect laptop").expect("listed");
    app.press("id:access-client-2 >> role:button[Revoke]")
        .expect("legacy revocation");
    audit(&mut app, "client=cloud-id");
    assert_eq!(app.screen().count("role:alertdialog"), Ok(0));
}
widths!(legacy_revoke_desktop, legacy_revoke_phone, legacy_revoke);
fn unknown_code(w: (f32, f32)) {
    let mut app = access_app(
        w,
        AccessFixtureProps {
            approval: true,
            ..Default::default()
        },
    );
    app.wait_for("Waiting for approval · ABCD-2345")
        .expect("initial snapshot");
    app.press_key("ctrl+shift+i")
        .expect("replace code by malformed value");
    app.wait_for("id:access-request-0 >> text:Waiting for approval")
        .expect("safe label");
    assert!(
        app.screen()
            .texts()
            .iter()
            .all(|value| !value.contains("private-code-marker"))
    );
    assert!(
        app.press("id:access-request-0 >> role:button[Approve]")
            .is_err()
    );
    audit(&mut app, "");
    app.press("id:access-request-0 >> role:button[Deny]")
        .expect("still deniable");
    audit(&mut app, "deny=request-a");
}
widths!(unknown_code_desktop, unknown_code_phone, unknown_code);
fn write_only_access(w: (f32, f32)) {
    let mut app = access_app(
        w,
        AccessFixtureProps {
            read: false,
            write: true,
            approval: false,
            save_fails: false,
            ..Default::default()
        },
    );
    app.wait_for("role:button[Authorized clients]")
        .expect("write scoped");
    app.press("role:button[Authorized clients]")
        .expect("expand");
    app.wait_for("This connection can create access links but cannot view authorized clients.")
        .expect("no read access");
    assert!(!app.screen().shows("Workstation"));
    assert!(!app.screen().shows("Waiting for approval"));
    app.press("role:button[Revoke others]")
        .expect("write-only bulk action");
    audit(&mut app, "clients/revoke-others");
}
widths!(
    write_only_access_desktop,
    write_only_access_phone,
    write_only_access
);

fn independent_requests(w: (f32, f32)) {
    let mut app = access_app(
        w,
        AccessFixtureProps {
            approval: true,
            ..Default::default()
        },
    );
    app.wait_for("Waiting for approval · ABCD-2345")
        .expect("ready");
    app.press_key("ctrl+shift+h").expect("hold first approval");
    app.press("id:access-request-0 >> role:button[Approve]")
        .expect("approve");
    app.wait_for("id:access-request-0 >> text:Saving…")
        .expect("individual pending state");
    assert!(
        app.press("id:access-request-0 >> role:button[Deny]")
            .is_err()
    );
    app.press("id:access-request-1 >> role:button[Deny]")
        .expect("other request stays actionable");
    audit(&mut app, "approve=request-a,deny=request-b");
    app.press_key("ctrl+shift+y").expect("finish first request");
    app.wait_for("id:access-request-0 >> role:button[Approve]")
        .expect("first row ready again");
}
widths!(
    independent_requests_desktop,
    independent_requests_phone,
    independent_requests
);
