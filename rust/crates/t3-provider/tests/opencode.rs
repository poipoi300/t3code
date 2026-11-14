#![allow(clippy::unwrap_used)]
mod common;
use serde_json::{Value, json};
use t3_provider::{catalog, opencode_catalog::*};
#[test]
fn connected_v1_models_use_primary_agents_and_variants_and_mark_only_older_releases_of_each_provider_family()
 {
    let providers = json!({"connected":["anthropic","other"],"all":[
        {"id":"anthropic","name":"Anthropic","models":{
            "old":{"id":"old","name":"Older","family":"opus","release_date":"2026-01-01","variants":{"low":{},"high":{}}},
            "new":{"id":"new","name":"Newest","family":"opus","release_date":"2026-06-01","variants":{"low":{},"high":{}}},
            "deprecated":{"id":"deprecated","name":"Deprecated","family":"opus","release_date":"2027-01-01","status":"deprecated"}
        }},
        {"id":"other","name":"Other","models":{"same":{"id":"same","name":"Other provider","family":"opus","release_date":"2025-01-01","variants":{}}}},
        {"id":"disconnected","models":{"hidden":{"id":"hidden","name":"Hidden"}}}
    ]});
    let agents = vec![
        json!({"name":"subagent","mode":"subagent"}),
        json!({"name":"hidden","mode":"primary","hidden":true}),
        json!({"name":"plan","mode":"primary"}),
        json!({"name":"build","mode":"all"}),
    ];
    let models = v1_models(&providers, &agents, &[json!("custom")])
        .unwrap()
        .unwrap();
    assert_eq!(models.len(), 5);
    let get = |id: &str| models.iter().find(|model| model.slug.0 == id).unwrap();
    assert_eq!(get("anthropic/old").is_legacy, Some(Some(true)));
    assert_eq!(get("anthropic/deprecated").is_legacy, Some(Some(true)));
    assert!(get("anthropic/new").is_legacy.is_none());
    assert!(get("other/same").is_legacy.is_none());
    let caps = serde_json::to_value(get("anthropic/new").capabilities.as_ref().unwrap()).unwrap();
    assert_eq!(caps["optionDescriptors"][0]["currentValue"], "high");
    assert_eq!(caps["optionDescriptors"][1]["currentValue"], "build");
    assert_eq!(
        caps["optionDescriptors"][1]["options"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        catalog::descriptors(get("custom").capabilities.as_ref().unwrap()).len(),
        2
    );
    assert!(
        v1_models(&json!({"connected":[],"all":[]}), &[], &[])
            .unwrap()
            .is_none()
    );
}
#[test]
fn v2_subprovider_labels_do_not_repeat_opencode_and_model_options_exclude_v1_agents() {
    let catalog:Vec<OpenCodeModel>=serde_json::from_value(json!([
        {"providerID":"opencode","id":"a","name":"Alpha","providerName":"OpenCode","variants":[{"id":"low"},{"id":"medium"}]},
        {"providerID":"openai","id":"b","name":"Beta","variants":[{"id":"high"}]}
    ])).unwrap();
    let models = v2_models(&catalog, &[]);
    assert!(models[0].sub_provider.is_none());
    assert_eq!(
        models[1].sub_provider.as_ref().unwrap().as_ref().unwrap().0,
        "openai"
    );
    let caps = serde_json::to_value(models[0].capabilities.as_ref().unwrap()).unwrap();
    assert_eq!(caps["optionDescriptors"].as_array().unwrap().len(), 1);
    assert_eq!(caps["optionDescriptors"][0]["currentValue"], "medium");
}
#[test]
fn inventories_keep_compact_deduplicate_commands_exclude_v1_skill_commands_and_preserve_v2_skill_identity()
 {
    let native: Vec<Value> = vec![
        json!({"name":"compact"}),
        json!({"name":"review","source":"skill","hints":[]}),
        json!({"name":" review ","description":" Review code ","hints":["file","scope"]}),
        json!({"name":"review","hints":[]}),
    ];
    let legacy = serde_json::to_value(commands(&native, false).unwrap()).unwrap();
    assert_eq!(legacy.as_array().unwrap().len(), 2);
    assert_eq!(legacy[1]["name"], "review");
    assert_eq!(legacy[1]["input"]["hint"], "file scope");
    let skills=skills(&[json!({"id":"stable-id","name":"Readable name","path":"/explicit/skill","description":" Description "})],true).unwrap();
    let skill = serde_json::to_value(&skills[0]).unwrap();
    assert_eq!(skill["name"], "stable-id");
    assert_eq!(skill["displayName"], "Readable name");
    assert_eq!(skill["shortDescription"], "Description");
}

use async_trait::async_trait;
use std::{collections::VecDeque, sync::Arc};
use t3_provider::{
    environment::Environment,
    opencode_loader::*,
    sensitive::{ServiceError, ServiceResult},
};
use tokio::sync::Mutex;
struct Source(Mutex<VecDeque<ServiceResult<Vec<OpenCodeModel>>>>);
#[async_trait]
impl OpenCodeModels for Source {
    async fn list(&self) -> ServiceResult<Vec<OpenCodeModel>> {
        match self.0.lock().await.pop_front() {
            Some(value) => value,
            None => std::future::pending().await,
        }
    }
}
fn catalog(slugs: &[&str]) -> Vec<OpenCodeModel> {
    slugs
        .iter()
        .map(|slug| {
            serde_json::from_value(
                json!({"providerID":"openai","id":slug,"name":slug,"variants":[]}),
            )
            .unwrap()
        })
        .collect()
}
#[tokio::test(start_paused = true)]
async fn cold_v2_catalogs_wait_for_two_agreeing_reads_and_warm_catalogs_accept_new_progress() {
    let partial = catalog(&["one"]);
    let full = catalog(&["one", "two"]);
    let source = Arc::new(Source(Mutex::new(
        vec![Ok(partial), Ok(full.clone()), Ok(full.clone())].into(),
    )));
    let loader = OpenCodeModelLoader::new(source.clone(), &Environment::new());
    assert_eq!(loader.load().await.unwrap(), full);
    let newest = catalog(&["one", "two", "three"]);
    source.0.lock().await.extend([Ok(newest.clone())]);
    assert_eq!(loader.load().await.unwrap(), newest);
    source.0.lock().await.extend([Ok(vec![])]);
    assert_eq!(loader.load().await.unwrap(), newest);
    source.0.lock().await.extend([Err(ServiceError::Network)]);
    assert_eq!(loader.load().await.unwrap_err(), ServiceError::Network);
}
#[tokio::test(start_paused = true)]
async fn strict_directory_catalogs_never_publish_a_partial_inventory_as_settled() {
    let source = Arc::new(Source(Mutex::new(
        vec![Ok(catalog(&["one"])), Err(ServiceError::Network)].into(),
    )));
    let loader = OpenCodeModelLoader::new(source.clone(), &Environment::new());
    assert!(loader.read_settled().await.is_none());
    source
        .0
        .lock()
        .await
        .extend([Ok(catalog(&["one"])), Ok(catalog(&["one"]))]);
    assert_eq!(loader.read_settled().await.unwrap().len(), 1);
}

use t3_provider::{opencode_http::*, sensitive::Sensitive, transport::*};
struct Http {
    replies: Mutex<std::collections::BTreeMap<String, RawHttpResponse>>,
    calls: Mutex<Vec<HttpRequest>>,
    slow_names: bool,
    slow_models: bool,
}
#[async_trait]
impl HttpTransport for Http {
    async fn send(&self, _: HttpRequest) -> ServiceResult<HttpResponse> {
        panic!("OpenCode requires response identity metadata")
    }
    async fn raw(&self, request: HttpRequest) -> ServiceResult<RawHttpResponse> {
        assert_eq!(request.method, "GET");
        let path = url::Url::parse(&request.url).unwrap().path().to_owned();
        self.calls.lock().await.push(request);
        if self.slow_models && path.ends_with("/api/model") {
            tokio::time::sleep(std::time::Duration::from_secs(6)).await;
        }
        if self.slow_names && path.ends_with("/api/provider") {
            return std::future::pending().await;
        }
        self.replies
            .lock()
            .await
            .get(&path)
            .cloned()
            .ok_or(ServiceError::Network)
    }
}
fn raw(status: u16, body: Value) -> RawHttpResponse {
    RawHttpResponse {
        status,
        content_type: Some("application/json; charset=utf-8".into()),
        body: Sensitive::new(serde_json::to_vec(&body).unwrap()),
    }
}
#[tokio::test]
async fn opencode_http_requires_version_identity_and_preserves_basic_passwords_and_directory_queries()
 {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let http = Arc::new(Http {
        replies: Mutex::new(std::collections::BTreeMap::from([
            ("/nested/api/info".into(), raw(404, json!({}))),
            (
                "/nested/global/health".into(),
                raw(200, json!({"healthy":true,"version":"1.14.19"})),
            ),
            (
                "/nested/provider".into(),
                raw(200, json!({"all":[],"connected":[]})),
            ),
        ])),
        calls: Mutex::new(vec![]),
        slow_names: false,
        slow_models: false,
    });
    let client = OpenCodeHttp::new(
        http.clone(),
        "https://server.invalid/nested",
        Some(Sensitive::new("sëcret-päss".into())),
    )
    .unwrap();
    assert_eq!(
        client.probe(&Environment::new()).await.unwrap().generation,
        1
    );
    let inventory = client
        .inventory_v1("C:/workspace é", std::time::Duration::from_secs(5))
        .await
        .unwrap();
    assert!(inventory.agents.is_empty());
    assert!(inventory.skills.is_empty());
    let calls = http.calls.lock().await;
    assert_eq!(
        calls[0].headers["authorization"].expose(),
        &format!("Basic {}", STANDARD.encode("opencode:sëcret-päss"))
    );
    let provider = calls
        .iter()
        .find(|request| request.url.contains("/provider?"))
        .unwrap();
    assert_eq!(
        url::Url::parse(&provider.url)
            .unwrap()
            .query_pairs()
            .find(|(key, _)| key == "directory")
            .unwrap()
            .1,
        "C:/workspace é"
    );
    assert!(!format!("{client:?} {calls:?}").contains("sëcret-päss"));
    drop(calls);
    http.replies.lock().await.insert(
        "/nested/api/info".into(),
        RawHttpResponse {
            status: 401,
            content_type: None,
            body: Sensitive::new(b"never include this secret".to_vec()),
        },
    );
    let error = client.probe(&Environment::new()).await.unwrap_err();
    assert_eq!(error, OpenCodeError::Authentication);
    assert!(!format!("{error:?} {error}").contains("never include"));
}
#[tokio::test(start_paused = true)]
async fn optional_v2_provider_labels_cannot_stall_models_but_directory_catalogs_require_them() {
    let http = Arc::new(Http {
        replies: Mutex::new(std::collections::BTreeMap::from([(
            "/api/model".into(),
            raw(
                200,
                json!({"data":[{"providerID":"openai","id":"model","name":"Model","variants":[],"time":{"released":1000},"status":"active"}]}),
            ),
        )])),
        calls: Mutex::new(vec![]),
        slow_names: true,
        slow_models: false,
    });
    let connection = Arc::new(
        OpenCodeHttp::new(
            http.clone(),
            "https://server.invalid",
            Some(Sensitive::new("secret".into())),
        )
        .unwrap(),
    );
    let mut source = HttpModels {
        connection,
        directory: "/workspace".into(),
        strict_names: false,
    };
    let models = source.list().await.unwrap();
    assert_eq!(models.len(), 1);
    assert!(models[0].provider_name.is_none());
    let calls = http.calls.lock().await;
    assert!(calls.iter().all(|request| {
        url::Url::parse(&request.url)
            .unwrap()
            .query_pairs()
            .any(|(key, value)| key == "location[directory]" && value == "/workspace")
    }));
    drop(calls);
    source.strict_names = true;
    assert!(source.list().await.is_err());
}

#[tokio::test]
async fn owned_opencode_servers_use_private_environment_share_borrowers_and_stop_only_their_captured_child()
 {
    use common::Fixture;
    use t3_provider::opencode_runtime::*;
    let fixture = Fixture::new(json!({"protocol":"opencode","generation":2,"version":"2.0.28"}));
    let environment_log = fixture.directory.path().join("environment.json");
    fixture.write(json!({"protocol":"opencode","generation":2,"version":"2.0.28","environmentLog":environment_log}));
    let mut environment = fixture.environment.clone();
    environment.insert(
        "OPENCODE_SERVER_PASSWORD".into(),
        "never-log-inherited-password".into(),
    );
    let launch = OpenCodeLaunch {
        binary: fixture.binary(),
        directory: fixture.directory.path().into(),
        environment,
        server_url: String::new(),
        server_password: None,
    };
    assert!(!format!("{launch:?}").contains("never-log"));
    let runtime = OpenCodeRuntime::new(launch, Arc::new(ReqwestTransport::new().unwrap()));
    fixture.write(json!({"protocol":"opencode","generation":1,"version":"1.10.0","environmentLog":environment_log}));
    assert!(runtime.acquire().await.is_err());
    assert!(
        !fixture
            .calls()
            .iter()
            .flatten()
            .any(|argument| argument == "serve")
    );
    fixture.write(json!({"protocol":"opencode","generation":2,"version":"2.0.28","environmentLog":environment_log}));
    let first = runtime.acquire().await.unwrap();
    let second = runtime.acquire().await.unwrap();
    assert!(!first.external);
    assert!(Arc::ptr_eq(&first.http, &second.http));
    assert_eq!(runtime.forget().await.unwrap_err(), ServiceError::Busy);
    let exit = first.process_exit.clone().unwrap();
    let environment: Value =
        serde_json::from_slice(&std::fs::read(environment_log).unwrap()).unwrap();
    assert_eq!(
        environment,
        json!({"hasV2Password":true,"hasV1Password":false,"emptyConfig":true})
    );
    let controls: Vec<_> = fixture
        .calls()
        .into_iter()
        .filter(|call| call.first().is_some_and(|argument| argument == "serve"))
        .collect();
    assert_eq!(controls.len(), 1);
    assert!(
        controls[0]
            .iter()
            .all(|argument| !argument.contains("password"))
    );
    drop(first);
    drop(second);
    runtime.forget().await.unwrap();
    assert!(exit.is_cancelled());
    let restarted = runtime.acquire().await.unwrap();
    assert!(!restarted.process_exit.as_ref().unwrap().is_cancelled());
    runtime.close().await;
    assert!(restarted.process_exit.as_ref().unwrap().is_cancelled());
    assert!(
        !fixture
            .calls()
            .iter()
            .flatten()
            .any(|argument| argument.starts_with("session/")
                || argument == "prompt"
                || argument == "run")
    );
}

struct NoEvents;
#[async_trait]
impl t3_provider::opencode_workspace::WorkspaceEvents for NoEvents {
    async fn subscribe(
        &self,
        _: HttpRequest,
    ) -> ServiceResult<t3_provider::opencode_workspace::ScanSubscription> {
        Err(ServiceError::Unsupported)
    }
}
fn provider(http: Arc<Http>, enabled: bool) -> t3_provider::opencode_provider::OpenCodeProvider {
    use t3_provider::{
        environment::Platform, instances::InstanceSpec, native_provider::NativeProviderContext,
        opencode_runtime::*,
    };
    t3_provider::opencode_provider::OpenCodeProvider::new(
        Arc::new(InstanceSpec {
            instance_id: "opencode".into(),
            driver: "opencode".into(),
            display_name: None,
            accent_color: None,
            variables: vec![],
            enabled,
            config: json!({"customModels":["custom"]}),
        }),
        NativeProviderContext {
            environment: Environment::new(),
            home: "/explicit-unused-home".into(),
            cwd: "/workspace".into(),
            platform: Platform::Windows,
            managed_mcp: false,
            server_version: "0.0.0".into(),
        },
        OpenCodeRuntime::new(
            OpenCodeLaunch {
                binary: "never-run-on-remote".into(),
                directory: "/workspace".into(),
                environment: Environment::new(),
                server_url: "https://remote.invalid".into(),
                server_password: Some(Sensitive::new("secret".into())),
            },
            http,
        ),
        Arc::new(NoEvents),
        Arc::new(SystemClock),
        Arc::new(t3_provider::catalog::Manifest::bundled().unwrap()),
    )
}
#[tokio::test(start_paused = true)]
async fn opencode_status_distinguishes_connected_empty_and_rejected_catalogs_and_keeps_workspace_models_separate()
 {
    use t3_provider::service::ProviderInstanceService;
    let http = Arc::new(Http {
        replies: Mutex::new(std::collections::BTreeMap::from([
            ("/api/info".into(), raw(404, json!({}))),
            (
                "/global/health".into(),
                raw(200, json!({"healthy":true,"version":"1.14.19"})),
            ),
            (
                "/provider".into(),
                raw(
                    200,
                    json!({"all":[{"id":"anthropic","models":{"a":{"id":"a","name":"Alpha"}}}],"connected":["anthropic"]}),
                ),
            ),
            ("/agent".into(), raw(200, json!([]))),
            ("/skill".into(), raw(200, json!([]))),
            ("/command".into(), raw(200, json!([]))),
        ])),
        calls: Mutex::new(vec![]),
        slow_names: false,
        slow_models: false,
    });
    let service = provider(http.clone(), true);
    let snapshot = serde_json::to_value(service.snapshot(None, true).await.unwrap()).unwrap();
    assert_eq!(snapshot["status"], "ready");
    assert_eq!(snapshot["auth"]["status"], "authenticated");
    assert!(snapshot.get("installedVersion").is_none());
    assert_eq!(snapshot["showInteractionModeToggle"], false);
    http.replies.lock().await.insert(
        "/provider".into(),
        raw(200, json!({"all":[],"connected":["anthropic"]})),
    );
    let snapshot = serde_json::to_value(service.snapshot(None, true).await.unwrap()).unwrap();
    assert_eq!(snapshot["status"], "warning");
    assert_eq!(snapshot["auth"]["status"], "unknown");
    assert_eq!(snapshot["models"].as_array().unwrap().len(), 1);
    http.replies.lock().await.insert(
        "/provider".into(),
        raw(401, json!({"secret":"never-in-error"})),
    );
    let snapshot = serde_json::to_value(service.snapshot(None, true).await.unwrap()).unwrap();
    assert_eq!(snapshot["status"], "error");
    assert_eq!(snapshot["auth"]["status"], "unauthenticated");
    assert!(!snapshot.to_string().contains("never-in-error"));
    http.replies.lock().await.extend([
        ("/api/info".into(),raw(200,json!({"pid":100,"version":"2.0.28"}))),
        ("/api/model".into(),raw(200,json!({"data":[{"providerID":"openai","id":"machine","name":"Machine","variants":[],"time":{"released":1000}}]}))),
        ("/api/provider".into(),raw(200,json!({"data":[{"id":"openai","name":"OpenAI"}]}))),
        ("/api/skill".into(),raw(200,json!({"data":[]}))),
        ("/api/command".into(),raw(200,json!({"data":[{"name":"review"}]}))),
    ]);
    let machine = service.snapshot(None, true).await.unwrap();
    assert_eq!(machine.models[0].slug.0, "openai/machine");
    let json = serde_json::to_value(&machine).unwrap();
    assert_eq!(json["showInteractionModeToggle"], true);
    http.replies.lock().await.insert("/api/model".into(),raw(200,json!({"data":[{"providerID":"openai","id":"project","name":"Project","variants":[],"time":{"released":1000}}]})));
    let workspace = service.snapshot(Some("/project"), true).await.unwrap();
    assert_eq!(workspace.models, machine.models);
    let inventory = &workspace.workspace_snapshots.unwrap()[0];
    assert_eq!(
        inventory.models.as_ref().unwrap().as_ref().unwrap()[0]
            .slug
            .0,
        "openai/project"
    );
    assert_eq!(inventory.slash_commands.len(), 2);
    assert_eq!(inventory.slash_commands_pending, Some(Some(false)));
    let disabled = provider(http.clone(), false);
    let before = http.calls.lock().await.len();
    assert!(!disabled.snapshot(None, true).await.unwrap().installed);
    assert_eq!(before, http.calls.lock().await.len());
}

#[tokio::test]
async fn workspace_inventory_subscribes_before_boot_and_waits_for_both_matching_scan_events() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use t3_provider::opencode_workspace::*;
    struct Events {
        subscribed: Arc<AtomicBool>,
        send: tokio::sync::mpsc::Sender<ScanEvent>,
        receive: Mutex<Option<tokio::sync::mpsc::Receiver<ScanEvent>>>,
    }
    #[async_trait]
    impl WorkspaceEvents for Events {
        async fn subscribe(&self, request: HttpRequest) -> ServiceResult<ScanSubscription> {
            assert!(request.url.ends_with("/api/event"));
            self.subscribed.store(true, Ordering::SeqCst);
            Ok(ScanSubscription::new(
                self.receive.lock().await.take().unwrap(),
            ))
        }
    }
    struct Scanning {
        events: Arc<Events>,
        reads: AtomicUsize,
    }
    #[async_trait]
    impl HttpTransport for Scanning {
        async fn send(&self, _: HttpRequest) -> ServiceResult<HttpResponse> {
            unreachable!()
        }
        async fn raw(&self, request: HttpRequest) -> ServiceResult<RawHttpResponse> {
            assert!(self.events.subscribed.load(Ordering::SeqCst));
            let path = url::Url::parse(&request.url).unwrap().path().to_owned();
            let value = match path.as_str() {
                "/api/debug/location" => json!([]),
                "/api/provider" => json!({"data":[]}),
                "/api/skill" => json!({"data":[]}),
                "/api/command" => {
                    let read = self.reads.fetch_add(1, Ordering::SeqCst);
                    if read == 0 {
                        for (directory, kind) in [
                            ("/unrelated", ScanKind::Skills),
                            ("/project", ScanKind::Commands),
                            ("/project", ScanKind::Skills),
                        ] {
                            self.events
                                .send
                                .send(ScanEvent {
                                    directory: directory.into(),
                                    kind,
                                })
                                .await
                                .unwrap();
                        }
                        json!({"data":[]})
                    } else {
                        json!({"data":[{"name":"finished"}]})
                    }
                }
                _ => panic!("unexpected path"),
            };
            Ok(raw(200, value))
        }
    }
    let (send, receive) = tokio::sync::mpsc::channel(8);
    let events = Arc::new(Events {
        subscribed: Arc::new(AtomicBool::new(false)),
        send,
        receive: Mutex::new(Some(receive)),
    });
    let http = OpenCodeHttp::new(
        Arc::new(Scanning {
            events: events.clone(),
            reads: AtomicUsize::new(0),
        }),
        "http://local.invalid",
        None,
    )
    .unwrap();
    let (_, commands) = load(&http, &*events, "/project", &Environment::new())
        .await
        .unwrap();
    assert_eq!(
        commands,
        json!([{"name":"finished"}]).as_array().unwrap().clone()
    );
}

#[tokio::test]
async fn workspace_sse_reads_fragmented_utf8_and_crlf_frames_with_private_basic_auth() {
    use t3_provider::opencode_workspace::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (accepted, receive) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut bytes = Vec::new();
        while !bytes.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            socket.read_exact(&mut byte).await.unwrap();
            bytes.push(byte[0]);
        }
        let request = String::from_utf8(bytes).unwrap();
        assert!(request.starts_with("GET /api/event "));
        assert!(
            request
                .to_lowercase()
                .contains("authorization: basic b3blbmnvzgu6c2vjcmv0")
        );
        let body = ":keepalive\r\ndata: {\"type\":\"command.updated\",\"location\":{\"directory\":\"/é\"}}\r\n\r\ndata: {\"type\":\"skill.updated\",\r\ndata: \"location\":{\"directory\":\"/é\"}}\r\n\r\n";
        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n",body.len()).as_bytes()).await.unwrap();
        let _ = accepted.send(());
        for part in body.as_bytes().chunks(3) {
            socket.write_all(part).await.unwrap();
        }
    });
    let http = OpenCodeHttp::new(
        Arc::new(ReqwestTransport::new().unwrap()),
        &format!("http://{address}"),
        Some(Sensitive::new("secret".into())),
    )
    .unwrap();
    let mut subscription = HttpWorkspaceEvents::new()
        .unwrap()
        .subscribe(http.event_request())
        .await
        .unwrap();
    receive.await.unwrap();
    let first = subscription.events.recv().await.unwrap();
    assert!(matches!(first.kind, ScanKind::Commands));
    assert_eq!(first.directory, "/é");
    let second = subscription.events.recv().await.unwrap();
    assert!(matches!(second.kind, ScanKind::Skills));
    assert_eq!(second.directory, "/é");
    server.await.unwrap();
}

#[test]
fn opencode_permission_modes_keep_auto_supervised_and_sandbox_overrides_and_plan_rules_ordered() {
    use t3_contracts::provider_policy::{ProviderInteractionMode::*, RuntimeMode::*};
    use t3_provider::modes::*;
    assert_eq!(
        opencode_v1(Auto, None, None),
        opencode_v1(ApprovalRequired, None, None)
    );
    assert_eq!(
        opencode_v1(FullAccess, None, None),
        json!([{"permission":"*","pattern":"*","action":"allow"}])
            .as_array()
            .unwrap()
            .clone()
    );
    let sandbox =
        json!({"type":"workspaceWrite","writableRoots":["/project/"],"networkAccess":true});
    let never = json!("never");
    let restricted = opencode_v1(Auto, Some(&never), Some(&sandbox));
    assert!(restricted.contains(&json!({"permission":"read","pattern":"*.env","action":"deny"})));
    assert!(restricted.contains(
        &json!({"permission":"external_directory","pattern":"/project/*","action":"allow"})
    ));
    assert!(restricted.contains(&json!({"permission":"webfetch","pattern":"*","action":"allow"})));
    let plan = opencode_v2(
        Auto,
        Plan,
        &[json!({"action":"edit","effect":"allow","resource":"*"})],
        &[json!({"action":"edit","effect":"allow","resource":"/plan"})],
    );
    assert_eq!(
        plan[0],
        json!({"action":"shell","effect":"ask","resource":"*"})
    );
    assert_eq!(
        plan[4],
        json!({"action":"edit","effect":"deny","resource":"*"})
    );
    assert_eq!(plan[5]["resource"], "/plan");
}

#[tokio::test(start_paused = true)]
async fn model_reads_use_the_callers_budget_instead_of_an_inner_five_second_cap() {
    let http = Arc::new(Http {
        replies: Mutex::new(std::collections::BTreeMap::from([
            (
                "/api/model".into(),
                raw(
                    200,
                    json!({"data":[{"providerID":"openai","id":"slow","name":"Slow","variants":[],"time":{"released":1000},"status":"active"}]}),
                ),
            ),
            ("/api/provider".into(), raw(200, json!({"data":[]}))),
        ])),
        calls: Mutex::new(vec![]),
        slow_names: false,
        slow_models: true,
    });
    let source = HttpModels {
        connection: Arc::new(OpenCodeHttp::new(http, "https://fixture.invalid", None).unwrap()),
        directory: "fixture".into(),
        strict_names: false,
    };
    assert_eq!(source.list().await.unwrap().len(), 1);
}
#[tokio::test(start_paused = true)]
async fn cold_catalog_retries_only_plugin_activation_with_a_bounded_initial_read() {
    let full = catalog(&["one", "two"]);
    let source = Arc::new(Source(Mutex::new(
        vec![
            Err(ServiceError::ActivatingPlugins),
            Err(ServiceError::ActivatingPlugins),
            Ok(full.clone()),
            Ok(full.clone()),
        ]
        .into(),
    )));
    let loader = OpenCodeModelLoader::new(source.clone(), &Environment::new());
    assert_eq!(loader.load().await.unwrap(), full);
    assert_eq!(loader.load().await.unwrap(), full); // an unanswered initial read retains the warm cache
    source.0.lock().await.push_back(Err(ServiceError::Response));
    assert_eq!(loader.load().await.unwrap_err(), ServiceError::Response);
    source
        .0
        .lock()
        .await
        .push_back(Err(ServiceError::ActivatingPlugins));
    assert!(loader.read_settled().await.is_none());
}

#[tokio::test]
async fn workspace_model_metadata_caches_compact_entries_and_missing_models_but_not_failures() {
    use t3_provider::opencode_metadata::OpenCodeModelMetadata;
    let http = Arc::new(Http {
        replies: Mutex::new(std::collections::BTreeMap::from([(
            "/provider".into(),
            raw(503, json!({"message":"never echo a private payload"})),
        )])),
        calls: Mutex::new(vec![]),
        slow_names: false,
        slow_models: false,
    });
    let cache = OpenCodeModelMetadata::new(
        Arc::new(OpenCodeHttp::new(http.clone(), "https://fixture.invalid", None).unwrap()),
        "workspace".into(),
    );
    assert_eq!(
        cache
            .resolve_variant("openai", "model", Some("high"))
            .await
            .as_deref(),
        Some("high")
    );
    http.replies.lock().await.insert("/provider".into(), raw(200, json!({"all":[{"id":"openai","models":{
        "model":{"variants":{"low":{},"high":{}},"limit":{"input":200000,"context":1000000}},
        "fallback":{"limit":{"input":null,"context":128000}}
    }}]})));
    assert_eq!(
        cache.read("openai", "model").await.unwrap().context_window,
        Some(200000.0)
    );
    assert_eq!(
        cache
            .read("openai", "fallback")
            .await
            .unwrap()
            .context_window,
        Some(128000.0)
    );
    assert_eq!(
        cache
            .resolve_variant("openai", "model", Some("invalid"))
            .await,
        None
    );
    assert_eq!(
        cache
            .resolve_variant("openai", "missing", Some("persisted"))
            .await
            .as_deref(),
        Some("persisted")
    );
    http.replies.lock().await.clear();
    assert!(cache.read("openai", "missing").await.is_none());
    assert_eq!(
        cache
            .resolve_variant("openai", "model", Some("high"))
            .await
            .as_deref(),
        Some("high")
    );
    assert_eq!(http.calls.lock().await.len(), 4);
}
#[tokio::test]
async fn only_structured_plugin_unavailable_errors_are_retryable_and_error_payloads_stay_private() {
    let http = Arc::new(Http {
        replies: Mutex::new(std::collections::BTreeMap::new()),
        calls: Mutex::new(vec![]),
        slow_names: false,
        slow_models: false,
    });
    let client = OpenCodeHttp::new(http.clone(), "https://fixture.invalid", None).unwrap();
    for (body, expected) in [
        (
            json!({"_tag":"ServiceUnavailableError","service":"plugin","message":"private payload"}),
            OpenCodeError::ActivatingPlugins,
        ),
        (
            json!({"_tag":"ServiceUnavailableError","service":"other","message":"private payload"}),
            OpenCodeError::Unavailable,
        ),
        (
            json!({"service":"plugin","message":"private payload"}),
            OpenCodeError::Unavailable,
        ),
    ] {
        http.replies
            .lock()
            .await
            .insert("/api/model".into(), raw(503, body));
        let error = client
            .get(
                "/api/model",
                Some("fixture"),
                2,
                std::time::Duration::from_secs(5),
            )
            .await
            .unwrap_err();
        assert_eq!(error, expected);
        assert!(!format!("{error:?} {error}").contains("private payload"));
    }
}
