//! Headless command-palette host-command tests: availability, execution, clipboard, RPCs, and
//! deduplication over a paired fake server (`FakeEnvironment` + `app_tree` + `nsh-testing`).

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use futures::{FutureExt, future::LocalBoxFuture, stream::LocalBoxStream};
use nsh_core::{keymap::When, prelude::*};
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use serde_json::json;
use t3_client::{
    ClientStore,
    composition::{ClientRuntime, Spawner},
    platform::{
        HostAction, HostFuture, HostReply, HttpRequest, HttpResponse, MimeData, Platform,
        PlatformError, PlatformEvent, SocketRequest, Storage, StorageArea, StorageKey,
        StorageTransaction, StorageWrite, WebSocket,
    },
    testing::{
        CallKind,
        environment::{EnvironmentPlatform, FakeEnvironment, NOW, shell},
    },
    types::{EnvironmentId, ProjectId, ThreadId, ThreadKey},
    workspace::ProjectKey,
};
use t3_contracts::{
    orchestration_v2::OrchestrationV2CreationSource,
    rpc::{
        RpcMethod, WsOrchestrationV2DispatchCommandRpc, WsProjectsMutateRpc,
        WsServerRefreshProvidersRpc,
    },
};
use t3_ui::{
    app::{app_tree, provide_client_services},
    runtime::{Rt, host::Choice},
    screens::sidebar::row::RowThreadActionsRuntime,
    state::nav::{Route, navigator},
};

use crate::support::DESKTOP;

#[derive(Clone)]
struct ClipboardPlatform {
    inner: EnvironmentPlatform,
    writes: Rc<RefCell<Vec<Vec<MimeData>>>>,
    fail_clipboard: Rc<Cell<bool>>,
}

impl ClipboardPlatform {
    fn new(inner: EnvironmentPlatform) -> Self {
        Self {
            inner,
            writes: Rc::default(),
            fail_clipboard: Rc::default(),
        }
    }

    /// What the HTML flavour of the last write that carried a thread chip says, decoded.
    fn fragment_summary(&self) -> String {
        let writes = self.writes.borrow();
        let Some(html) = writes.iter().rev().find_map(|batch| {
            batch
                .iter()
                .find(|mime| mime.mime == "text/html")
                .and_then(|mime| String::from_utf8(mime.bytes.clone()).ok())
        }) else {
            return "none".into();
        };
        let encoded = html
            .split("data-t3-context-fragment=\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .unwrap_or_default();
        let mut bytes = Vec::new();
        let raw = encoded.as_bytes();
        let mut at = 0;
        while at < raw.len() {
            if raw[at] == b'%' && at + 2 < raw.len() {
                bytes.push(u8::from_str_radix(&encoded[at + 1..at + 3], 16).unwrap_or(b'?'));
                at += 3;
            } else {
                bytes.push(raw[at]);
                at += 1;
            }
        }
        let fragment: serde_json::Value =
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        let record = &fragment["records"][0];
        format!(
            "v{} source={} kind={} env={} thread={} label={} title={}",
            fragment["version"],
            fragment["source"]["environmentId"].as_str().unwrap_or(""),
            record["kind"].as_str().unwrap_or(""),
            record["environmentId"].as_str().unwrap_or(""),
            record["threadId"].as_str().unwrap_or(""),
            record["label"].as_str().unwrap_or(""),
            record["title"].as_str().unwrap_or(""),
        )
    }

    fn plain_texts(&self) -> Vec<String> {
        self.writes
            .borrow()
            .iter()
            .filter_map(|batch| {
                batch
                    .iter()
                    .find(|mime| mime.mime == "text/plain")
                    .and_then(|mime| String::from_utf8(mime.bytes.clone()).ok())
            })
            .collect()
    }
}

impl Storage for ClipboardPlatform {
    fn get(&self, area: StorageArea, key: StorageKey) -> HostFuture<'_, Option<Vec<u8>>> {
        self.inner.get(area, key)
    }

    fn write(&self, area: StorageArea, writes: Vec<StorageWrite>) -> HostFuture<'_, ()> {
        self.inner.write(area, writes)
    }

    fn list(&self, area: StorageArea, namespace: String) -> HostFuture<'_, Vec<StorageKey>> {
        self.inner.list(area, namespace)
    }

    fn transaction(&self, transaction: StorageTransaction) -> HostFuture<'_, bool> {
        self.inner.transaction(transaction)
    }
}

impl Platform for ClipboardPlatform {
    fn can_provision_ssh(&self) -> bool {
        self.inner.can_provision_ssh()
    }

    fn now_epoch_ms(&self) -> u64 {
        self.inner.now_epoch_ms()
    }

    fn monotonic_ms(&self) -> u64 {
        self.inner.monotonic_ms()
    }

    fn random_bytes(&self, length: usize) -> Result<Vec<u8>, PlatformError> {
        self.inner.random_bytes(length)
    }

    fn sleep(&self, milliseconds: u64) -> LocalBoxFuture<'_, ()> {
        self.inner.sleep(milliseconds)
    }

    fn yield_to_host(&self) -> LocalBoxFuture<'_, ()> {
        self.inner.yield_to_host()
    }

    fn events(&self) -> LocalBoxStream<'static, PlatformEvent> {
        self.inner.events()
    }

    fn set_route_policy(&self, policy: t3_client::navigation::RoutePolicy) {
        self.inner.set_route_policy(policy);
    }

    fn http(&self, request: HttpRequest) -> HostFuture<'_, HttpResponse> {
        self.inner.http(request)
    }

    fn websocket(&self, request: SocketRequest) -> HostFuture<'_, Box<dyn WebSocket>> {
        self.inner.websocket(request)
    }

    fn host(&self, action: HostAction) -> HostFuture<'_, HostReply> {
        match action {
            HostAction::ClipboardWrite(data) => {
                if self.fail_clipboard.get() {
                    return async { Err(PlatformError::Unsupported("clipboard disabled".into())) }
                        .boxed_local();
                }
                self.writes.borrow_mut().push(data);
                async { Ok(HostReply::Unit) }.boxed_local()
            }
            other => self.inner.host(other),
        }
    }
}

#[derive(Clone)]
struct Scenario {
    home: bool,
    draft_only: bool,
    has_provider_session: bool,
    github_repo: bool,
    linked_pr: bool,
    thread_find: bool,
    project_org: bool,
    project_pinned: bool,
    operate_scope: bool,
    guest_sharing: bool,
    initial_prompt: Option<&'static str>,
    hosted_app_url: Option<&'static str>,
    thread_title: &'static str,
}

impl Default for Scenario {
    fn default() -> Self {
        Self {
            home: false,
            draft_only: false,
            has_provider_session: false,
            github_repo: true,
            linked_pr: false,
            thread_find: false,
            project_org: true,
            project_pinned: false,
            operate_scope: true,
            guest_sharing: true,
            initial_prompt: None,
            hosted_app_url: None,
            thread_title: "Alpha thread",
        }
    }
}

fn thread_key() -> ThreadKey {
    ThreadKey {
        environment: EnvironmentId("remote".into()),
        thread: ThreadId("alpha".into()),
    }
}

fn project_key() -> ProjectKey {
    ProjectKey {
        environment: EnvironmentId("remote".into()),
        project: ProjectId("project".into()),
    }
}

fn build_workspace(scenario: Scenario) -> impl IntoElement {
    let owner = Owner::current();
    let spawner: Spawner = Rc::new(move |future| {
        owner.run(|| {
            spawn(future);
        });
    });
    let server = FakeEnvironment::new("remote", spawner.clone());
    if scenario.operate_scope {
        server.set_permissions(&[
            "orchestration:read",
            "orchestration:operate",
            "access:write",
        ]);
    } else {
        server.set_permissions(&["orchestration:read"]);
    }
    let mut sh = shell(&[("alpha", scenario.thread_title)]);
    sh["projects"][0]["repositoryIdentity"] = if scenario.github_repo {
        json!({
            "canonicalKey": "github.com/t3/code",
            "provider": "github",
            "owner": "t3",
            "name": "code",
            "displayName": "t3/code",
            "webUrl": "https://github.com/t3/code",
            "locator": {
                "source": "git-remote",
                "remoteName": "origin",
                "remoteUrl": "https://github.com/t3/code.git"
            }
        })
    } else {
        json!(null)
    };
    sh["projects"][0]["pinnedAt"] = if scenario.project_pinned {
        json!(NOW)
    } else {
        json!(null)
    };
    if scenario.has_provider_session {
        sh["threads"][0]["latestRunId"] = json!("run-1");
        sh["threads"][0]["status"] = json!("completed");
        sh["threads"][0]["providerInstanceId"] = json!("claude-1");
    }
    if scenario.linked_pr {
        sh["threads"][0]["pullRequests"] = json!([{
            "host": "github.com",
            "repository": "t3/code",
            "number": 42,
            "url": "https://github.com/t3/code/pull/42",
            "source": "manual",
            "linkedAt": NOW,
            "snapshot": null,
            "stack": null
        }]);
    }
    server.set_shell(sh);

    let mut proj = t3_client::testing::environment::projection(
        "alpha",
        scenario.thread_title,
        json!([]),
        json!([]),
    );
    if scenario.has_provider_session {
        proj["thread"]["latestRunId"] = json!("run-1");
        proj["thread"]["status"] = json!("completed");
        proj["thread"]["providerInstanceId"] = json!("claude-1");
        let mut run: serde_json::Value = serde_json::from_str(
            include_str!("../../../../fixtures/samples/OrchestrationV2Run.jsonl")
                .lines()
                .next()
                .expect("sample run"),
        )
        .expect("parse sample run");
        run["id"] = json!("run-1");
        run["threadId"] = json!("alpha");
        run["ordinal"] = json!(1);
        run["providerInstanceId"] = json!("claude-1");
        run["modelSelection"] =
            json!({ "provider": "codex", "instanceId": "codex", "model": "gpt-5.4" });
        run["status"] = json!("completed");
        run["requestedAt"] = json!(NOW);
        run["startedAt"] = json!(NOW);
        run["completedAt"] = json!(NOW);
        proj["runs"] = json!([run]);
        let mut session: serde_json::Value = serde_json::from_str(
            include_str!("../../../../fixtures/samples/OrchestrationV2ProviderSession.jsonl")
                .lines()
                .next()
                .expect("sample provider session"),
        )
        .expect("parse sample provider session");
        session["id"] = json!("sess-1");
        session["driver"] = json!("codex");
        session["providerInstanceId"] = json!("claude-1");
        session["status"] = json!("ready");
        session["cwd"] = json!("/repo");
        session["model"] = json!("gpt-5.4");
        session["createdAt"] = json!(NOW);
        session["updatedAt"] = json!(NOW);
        session["lastError"] = json!(null);
        proj["providerSessions"] = json!([session]);
    }
    server.set_thread("alpha", proj);

    server.register(WsServerRefreshProvidersRpc::TAG, CallKind::Unary);
    server.reply(
        WsServerRefreshProvidersRpc::TAG,
        Ok(json!({ "providers": [] })),
    );
    server.register(WsProjectsMutateRpc::TAG, CallKind::Unary);
    server.reply(WsProjectsMutateRpc::TAG, Ok(json!({ "sequence": 1 })));
    server.edit_config(|config| {
        config["settings"]["guestSharingEnabled"] = json!(scenario.guest_sharing);
        config["threadFind"] = json!(scenario.thread_find);
        config["environment"]["capabilities"]["projectOrganization"] = json!(scenario.project_org);
        config["providers"] = json!([
            {
                "instanceId": "codex",
                "driver": "codex",
                "displayName": "Codex",
                "enabled": true,
                "installed": true,
                "version": null,
                "status": "ready",
                "auth": { "status": "authenticated" },
                "checkedAt": NOW,
                "models": [
                    {
                        "slug": "gpt-5.4",
                        "name": "GPT-5.4",
                        "isCustom": false,
                        "capabilities": {}
                    }
                ],
                "slashCommands": [],
                "skills": []
            }
        ]);
    });

    let platform = ClipboardPlatform::new(server.platform());
    let store = ClientStore::default();
    let client = ClientRuntime::new(
        platform.clone(),
        store.clone(),
        OrchestrationV2CreationSource::Web,
        spawner.clone(),
    );
    client.set_hosted_app_url(scenario.hosted_app_url.map(str::to_owned));
    let pairing = client.pair(server.pairing());
    let initial_prompt = scenario.initial_prompt;
    let draft_only = scenario.draft_only;
    let key = thread_key();
    let runtime = Rt::new(store, Rc::new(client.clone()), Choice::Client);

    let draft_id_slot = Rc::new(RefCell::new(None::<String>));
    let draft_ready = signal(None::<String>);
    spawner(
        async move {
            pairing.await.expect("pair fixture environment");
        }
        .boxed_local(),
    );

    if let Some(prompt) = initial_prompt
        && !draft_only
    {
        let _ = client
            .composer_drafts()
            .set_prompt(&key, prompt.to_owned(), prompt.len());
    }

    let route = if scenario.home || draft_only {
        Route::Home
    } else {
        Route::Thread(key.clone())
    };

    let diag_dispatch = signal(String::new());
    let diag_refresh = signal(String::new());
    let diag_mutate = signal(String::new());
    let diag_clip = signal(String::new());
    let diag_dups = signal(String::new());
    let diag_server = server.clone();
    let diag_platform = platform.clone();
    on_key_with(
        "f9",
        KeyOptions {
            when: When::Always,
            ..Default::default()
        },
        move |_| {
            let dispatches = diag_server
                .calls("orchestration.dispatchCommand")
                .into_iter()
                .filter(|p| p["type"] != "thread.visit")
                .map(|p| {
                    let ty = p["type"].as_str().unwrap_or_default();
                    let extra = p["providerSessionId"]
                        .as_str()
                        .or_else(|| p["url"].as_str())
                        .unwrap_or("");
                    format!("{ty}({extra})")
                })
                .collect::<Vec<_>>()
                .join(",");
            let refreshes = diag_server
                .calls("server.refreshProviders")
                .into_iter()
                .map(|p| {
                    format!(
                        "{}:{}:fresh={}",
                        p["instanceId"].as_str().unwrap_or(""),
                        p["cwd"].as_str().unwrap_or(""),
                        p["fresh"]
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            let mutations = diag_server
                .calls("projects.mutate")
                .into_iter()
                .map(|p| {
                    format!(
                        "{}:pinned={}",
                        p["type"].as_str().unwrap_or(""),
                        p["pinned"]
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            let clips = diag_platform.plain_texts().join("|");
            let html = diag_platform.fragment_summary();
            diag_dispatch.set(format!("Dispatch: [{dispatches}]"));
            diag_refresh.set(format!("Refresh: [{refreshes}]"));
            diag_mutate.set(format!("Mutate: [{mutations}]"));
            diag_clip.set(format!("Clip: [{clips}]"));
            diag_dups.set(format!("Fragment: {html}"));
        },
    );

    let fail_toggle = platform.clone();
    on_key_with(
        "f8",
        KeyOptions {
            when: When::Always,
            ..Default::default()
        },
        move |_| {
            fail_toggle.fail_clipboard.set(true);
        },
    );

    let fail_dispatch_server = server.clone();
    on_key_with(
        "f5",
        KeyOptions {
            when: When::Always,
            ..Default::default()
        },
        move |_| {
            fail_dispatch_server.reply(
                WsOrchestrationV2DispatchCommandRpc::TAG,
                Err(json!({
                    "_tag": "OrchestrationV2DispatchCommandError",
                    "commandId": "cmd-1",
                    "commandType": "thread.pull-request.link",
                    "message": "Server rejected command"
                })),
            );
        },
    );

    let fail_pin_server = server.clone();
    on_key_with(
        "f4",
        KeyOptions {
            when: When::Always,
            ..Default::default()
        },
        move |_| {
            fail_pin_server.reply(
                WsProjectsMutateRpc::TAG,
                Err(json!({
                    "_tag": "OrchestrationV2DispatchCommandError",
                    "commandId": "cmd-1",
                    "commandType": "project.update",
                    "message": "Pin rejected"
                })),
            );
        },
    );

    let prompt_client = client.clone();
    on_key_with(
        "f7",
        KeyOptions {
            when: When::Always,
            ..Default::default()
        },
        move |_| {
            let _ = prompt_client
                .composer_drafts()
                .set_prompt(&key, "Investigate bug".into(), 15);
        },
    );

    let f6_client = client.clone();
    let f6_spawner = spawner.clone();
    let f6_ready = draft_ready;
    on_key_with(
        "f6",
        KeyOptions {
            when: When::Always,
            ..Default::default()
        },
        move |_| {
            let f6_client = f6_client.clone();
            f6_spawner(
                async move {
                    let id = f6_client
                        .new_threads()
                        .open_project(project_key(), None, None)
                        .await
                        .expect("open project draft");
                    if let Some(prompt) = initial_prompt {
                        let _ = f6_client.composer_drafts().set_prompt_key(
                            &id,
                            prompt.to_owned(),
                            prompt.len(),
                        );
                    }
                    f6_ready.set(Some(id));
                }
                .boxed_local(),
            );
        },
    );

    let app = app_tree(runtime, route, move || {
        provide_client_services(&client);
        provide_context(RowThreadActionsRuntime(Rc::new(client)));
        let nav = navigator();
        effect(move || {
            if let Some(id) = draft_ready.get()
                && *draft_id_slot.borrow() != Some(id.clone())
            {
                *draft_id_slot.borrow_mut() = Some(id.clone());
                nav.open(Route::Draft(id));
            }
        });
    });
    view()
        .w_full()
        .h_full()
        .child(app)
        .child(text(move || diag_dispatch.get()))
        .child(text(move || diag_refresh.get()))
        .child(text(move || diag_mutate.get()))
        .child(text(move || diag_clip.get()))
        .child(text(move || diag_dups.get()))
        .child(text(move || {
            if draft_ready.get().is_some() {
                "Draft: ready".to_string()
            } else {
                String::new()
            }
        }))
}

#[component]
fn ThreadWithSession() -> impl IntoElement {
    build_workspace(Scenario {
        has_provider_session: true,
        linked_pr: false,
        thread_find: true,
        ..Default::default()
    })
}

#[component]
fn ThreadNoSessionNoRepo() -> impl IntoElement {
    build_workspace(Scenario {
        has_provider_session: false,
        github_repo: false,
        linked_pr: false,
        thread_find: false,
        project_org: false,
        ..Default::default()
    })
}

#[component]
fn ThreadWithLinkedPr() -> impl IntoElement {
    build_workspace(Scenario {
        linked_pr: true,
        project_pinned: true,
        ..Default::default()
    })
}

#[component]
fn ThreadReadOnly() -> impl IntoElement {
    build_workspace(Scenario {
        operate_scope: false,
        ..Default::default()
    })
}

#[component]
fn ThreadAtStaging() -> impl IntoElement {
    build_workspace(Scenario {
        hosted_app_url: Some("https://staging.example.test"),
        thread_find: true,
        ..Default::default()
    })
}

#[component]
fn ThreadWithAwkwardTitle() -> impl IntoElement {
    build_workspace(Scenario {
        thread_title: "[wip]\nfoo",
        ..Default::default()
    })
}

#[component]
fn HomeView() -> impl IntoElement {
    build_workspace(Scenario {
        home: true,
        ..Default::default()
    })
}

#[component]
fn DraftWithPrompt() -> impl IntoElement {
    build_workspace(Scenario {
        draft_only: true,
        github_repo: false,
        initial_prompt: Some("Investigate bug"),
        ..Default::default()
    })
}

fn start(root: Root) -> Headless {
    start_waiting(root, "role:button[Alpha thread, Project]")
}

fn start_waiting(root: Root, row: &str) -> Headless {
    let mut fixtures = Registry::new();
    fixtures.register::<ThreadWithSession>();
    fixtures.register::<ThreadNoSessionNoRepo>();
    fixtures.register::<ThreadWithLinkedPr>();
    fixtures.register::<ThreadReadOnly>();
    fixtures.register::<ThreadAtStaging>();
    fixtures.register::<ThreadWithAwkwardTitle>();
    fixtures.register::<HomeView>();
    fixtures.register::<DraftWithPrompt>();
    let mut sets = t3_ui::registries();
    sets.push(fixtures);
    let mut config = EngineConfig {
        ui_sets: sets,
        viewport: size(px(DESKTOP.0), px(DESKTOP.1)),
        ..EngineConfig::new(root)
    };
    config.environment.os = nsh_core::platform::Os::Linux;
    config.environment.idiom = nsh_core::platform::Idiom::Desktop;
    let mut app = Headless::start(config).expect("workspace starts");
    app.wait_for(row).expect("paired thread row");
    app
}

fn open_palette_and_filter(app: &mut Headless, query: &str) {
    app.press_key("mod+k").expect("open command palette");
    app.wait_for("role:combobox[Command palette]")
        .expect("command palette input");
    // A reopened palette restores the last search; start from an empty query.
    app.clear("role:combobox[Command palette]")
        .expect("clear the restored query");
    app.type_text("role:combobox[Command palette]", query)
        .expect("filter commands");
}

#[test]
fn restart_session_offered_on_thread_detaches_session_and_refreshes_and_hidden_on_home() {
    let mut home = start(Root::of::<HomeView>());
    open_palette_and_filter(&mut home, "restart agent session");
    assert!(!home.screen().shows("role:option[Restart agent session]"));

    let mut app = start(Root::of::<ThreadWithSession>());
    open_palette_and_filter(&mut app, "restart agent session");
    app.wait_for("role:option[Restart agent session]")
        .expect("offered on active thread");
    app.press("role:option[Restart agent session]")
        .expect("activate restart session");
    app.wait_for("Agent session will restart")
        .expect("restart toast shown");
    assert!(!app.screen().shows("role:combobox[Command palette]"));

    app.press_key("f9").expect("read diagnostics");
    app.wait_for("text:Dispatch: [provider-session.detach(sess-1)]")
        .expect("provider session detached");
    app.wait_for("text:Refresh: [claude-1:/workspace/project:fresh=true]")
        .expect("providers refreshed fresh with runtime providerInstanceId");

    open_palette_and_filter(&mut app, "show linked pull requests");
    app.wait_for("Show linked pull requests")
        .expect("offered (disabled without links) on an active thread");
    app.clear("role:combobox[Command palette]")
        .expect("clear filter before next search");

    // Failed detach surfaces error toast instead of swallowing the failure.
    app.press_key("f5").expect("script dispatch failure");
    app.type_text("role:combobox[Command palette]", "restart agent session")
        .expect("filter restart agent session");
    app.wait_for("role:option[Restart agent session]")
        .expect("restart option offered");
    app.press("role:option[Restart agent session]")
        .expect("activate restart session during scripted failure");
    app.wait_for("Command failed")
        .expect("error toast displayed on detach failure");
}

#[test]
fn restart_session_without_provider_session_skips_detach_and_refreshes() {
    let mut app = start(Root::of::<ThreadNoSessionNoRepo>());
    open_palette_and_filter(&mut app, "restart agent session");
    app.wait_for("role:option[Restart agent session]")
        .expect("offered on thread without active runtime");
    app.press("role:option[Restart agent session]")
        .expect("activate restart session");
    app.wait_for("Agent session will restart")
        .expect("restart toast shown");
    app.press_key("f9").expect("read diagnostics");
    app.wait_for("text:Dispatch: []")
        .expect("no detach dispatched when no active session");
    app.wait_for("text:Refresh: [codex:/workspace/project:fresh=true]")
        .expect("providers refreshed fresh");
}

#[test]
fn copy_reference_offered_on_thread_writes_hosted_url_and_pr_link_without_duplication() {
    let mut home = start(Root::of::<HomeView>());
    open_palette_and_filter(&mut home, "copy thread");
    assert!(!home.screen().shows("role:option[Copy thread]"));

    let mut app = start(Root::of::<ThreadWithSession>());
    open_palette_and_filter(&mut app, "copy thread");
    app.wait_for("role:option[Copy thread]")
        .expect("Copy thread offered");
    assert_eq!(app.screen().count("role:option[Copy thread]"), Ok(1));
    app.press("role:option[Copy thread]")
        .expect("activate Copy thread");
    app.wait_for("Thread copied").expect("Thread copied toast");
    app.press_key("f9").expect("read diagnostics");
    app.wait_for("text:Clip: [https://app.t3.codes/threads/remote/alpha]")
        .expect("bare hosted thread URL copied");

    let mut pr_app = start(Root::of::<ThreadWithLinkedPr>());
    open_palette_and_filter(&mut pr_app, "copy pr link");
    pr_app
        .wait_for("role:option[Copy PR link]")
        .expect("Copy PR link offered when PR is linked");
    assert!(!pr_app.screen().shows("role:option[Copy thread]"));
    pr_app
        .press("role:option[Copy PR link]")
        .expect("activate Copy PR link");
    pr_app
        .wait_for("PR link copied")
        .expect("PR link copied toast");
    pr_app.press_key("f9").expect("read diagnostics");
    pr_app
        .wait_for("text:Clip: [https://github.com/t3/code/pull/42]")
        .expect("PR URL copied");
}

#[test]
fn link_pull_request_visibility_validation_and_dispatch() {
    let mut home = start(Root::of::<HomeView>());
    open_palette_and_filter(&mut home, "link pull request");
    assert!(
        !home
            .screen()
            .shows("role:option[Link pull request to thread]")
    );

    let mut app = start(Root::of::<ThreadWithSession>());
    open_palette_and_filter(&mut app, "link pull request");
    app.wait_for("role:option[Link pull request to thread]")
        .expect("Link pull request offered");
    app.press("role:option[Link pull request to thread]")
        .expect("open Link pull request dialog");
    app.wait_for("Link pull request")
        .expect("dialog opened after palette closed");
    assert!(!app.screen().shows("role:combobox[Command palette]"));

    // Dialog auto-focuses the reference input so immediate keystrokes land there.
    app.press_key("4")
        .expect("type directly into initial focus");
    app.press_key("2")
        .expect("type second digit into initial focus");
    app.wait_for("github.com/t3/code #42")
        .expect("initial focus received input without targeting locator");
    app.clear("label:Pull request reference")
        .expect("clear focused test input");

    // Invalid reference keeps Link disabled and shows validation message.
    app.type_text("label:Pull request reference", "not-a-pr")
        .expect("type invalid input");
    app.press_key("enter").expect("submit invalid input");
    app.wait_for("Use a pull request URL, 123, or #123.")
        .expect("validation feedback displayed");

    // `#42` resolves against github.com/t3/code, surfaces server error on RPC failure, and closes on retry success.
    app.clear("label:Pull request reference")
        .expect("clear invalid input");
    app.type_text("label:Pull request reference", "#42")
        .expect("type PR number");
    app.wait_for("github.com/t3/code #42")
        .expect("preview target displayed");
    app.press_key("f5").expect("script dispatch failure");
    app.press("role:button[Link]").expect("submit failing link");
    app.wait_for("Server rejected command")
        .expect("RPC failure shown in dialog alert");
    app.wait_for("role:button[Link]")
        .expect("Link button restored from Linking... after failure");
    app.press("role:button[Link]").expect("retry link");
    app.press_key("f9").expect("read diagnostics");
    app.wait_for(
        "text:Dispatch: [thread.pull-request.link(https://github.com/t3/code/pull/42),thread.pull-request.link(https://github.com/t3/code/pull/42)]",
    )
    .expect("linked pull request dispatched on retry");
    assert!(!app.screen().shows("role:button[Link]"));

    // Without repository identity, bare `#42` fails with helpful guidance while full URL succeeds.
    let mut no_repo = start(Root::of::<ThreadNoSessionNoRepo>());
    open_palette_and_filter(&mut no_repo, "link pull request");
    no_repo
        .press("role:option[Link pull request to thread]")
        .expect("open dialog");
    no_repo
        .wait_for("label:Pull request reference")
        .expect("dialog input");
    no_repo
        .type_text("label:Pull request reference", "#42")
        .expect("type bare PR number");
    no_repo
        .wait_for("Paste the full pull request URL; this thread has no repository to resolve the number against.")
        .expect("missing repository explanation");
    no_repo
        .clear("label:Pull request reference")
        .expect("clear bare PR number");
    no_repo
        .type_text(
            "label:Pull request reference",
            "https://github.com/acme/web/pull/7",
        )
        .expect("type full PR URL");
    no_repo
        .wait_for("github.com/acme/web #7")
        .expect("full URL resolved");
    no_repo.press("role:button[Link]").expect("submit full URL");
    no_repo.press_key("f9").expect("read diagnostics");
    no_repo
        .wait_for("text:Dispatch: [thread.pull-request.link(https://github.com/acme/web/pull/7)]")
        .expect("full PR URL linked");
}

#[test]
fn copy_prompt_and_copy_prompt_context_availability_and_clipboard() {
    // Empty prompt hides both commands.
    let mut empty = start(Root::of::<ThreadWithSession>());
    open_palette_and_filter(&mut empty, "copy prompt link");
    assert!(!empty.screen().shows("role:option[Copy prompt link]"));
    assert!(
        !empty
            .screen()
            .shows("role:option[Copy prompt link with repo and model]")
    );
    empty.press_key("escape").expect("close palette");

    // Setting a prompt on a thread with a GitHub repo offers both commands and copies them.
    empty.press_key("f7").expect("set draft prompt");
    open_palette_and_filter(&mut empty, "copy prompt link");
    empty
        .wait_for("role:option[Copy prompt link]")
        .expect("plain Copy prompt link offered");
    empty
        .wait_for("role:option[Copy prompt link with repo and model]")
        .expect("context Copy prompt link offered");
    empty
        .press("role:option[Copy prompt link with repo and model]")
        .expect("copy prompt link with context");
    empty
        .wait_for("Prompt link copied")
        .expect("Prompt link copied toast");
    empty.press_key("f9").expect("read diagnostics");
    empty
        .wait_for("text:Clip: [https://app.t3.codes/new?prompt=Investigate+bug&repo=t3%2Fcode&provider=codex&model=gpt-5.4]")
        .expect("prompt link with repo and model copied");

    // Draft route without GitHub repo offers plain copy prompt link only.
    let mut draft = start(Root::of::<DraftWithPrompt>());
    draft
        .wait_for("role:button[Alpha thread, Project]")
        .expect("shell snapshot arrived");
    draft.press_key("f6").expect("open draft");
    draft.wait_for("Draft: ready").expect("draft opened");
    open_palette_and_filter(&mut draft, "copy prompt link");
    draft
        .wait_for("role:option[Copy prompt link]")
        .expect("plain Copy prompt link offered on draft");
    assert!(
        !draft
            .screen()
            .shows("role:option[Copy prompt link with repo and model]")
    );
    draft
        .press("role:option[Copy prompt link]")
        .expect("copy plain prompt link");
    draft
        .wait_for("Prompt link copied")
        .expect("Prompt link copied toast");
    draft.press_key("f9").expect("read diagnostics");
    draft
        .wait_for("text:Clip: [https://app.t3.codes/new?prompt=Investigate+bug]")
        .expect("plain prompt link copied");

    let mut no_repo = start(Root::of::<ThreadNoSessionNoRepo>());
    no_repo.press_key("f7").expect("set draft prompt");
    open_palette_and_filter(&mut no_repo, "copy prompt link");
    no_repo
        .wait_for("role:option[Copy prompt link]")
        .expect("plain Copy prompt link offered without github repo");
    assert!(
        !no_repo
            .screen()
            .shows("role:option[Copy prompt link with repo and model]")
    );

    // Clipboard write error shows an error toast instead of success.
    no_repo.press_key("f8").expect("force clipboard failure");
    no_repo
        .press("role:option[Copy prompt link]")
        .expect("trigger failing clipboard write");
    no_repo
        .wait_for("Could not copy the prompt link")
        .expect("error toast displayed on clipboard failure");
}

#[test]
fn find_thread_pin_project_dedup_and_share_guest_regression() {
    // Capability-gated find/pin hidden when disabled; no duplicates in catalogue.
    let mut restricted = start(Root::of::<ThreadNoSessionNoRepo>());
    open_palette_and_filter(&mut restricted, "find in current thread");
    assert!(
        !restricted
            .screen()
            .shows("role:option[Find in current thread]")
    );
    restricted.press_key("escape").expect("close palette");
    open_palette_and_filter(&mut restricted, "pin project");
    assert!(!restricted.screen().shows("role:option[Pin project]"));
    restricted.press_key("escape").expect("close palette");

    // Without orchestration:operate, Pin project is hidden.
    let mut readonly = start(Root::of::<ThreadReadOnly>());
    open_palette_and_filter(&mut readonly, "pin project");
    assert!(!readonly.screen().shows("role:option[Pin project]"));

    // Thread with threadFind + projectOrganization offers Find, Pin/Unpin, and ShareGuest.
    let mut app = start(Root::of::<ThreadWithSession>());
    open_palette_and_filter(&mut app, "find in current thread");
    app.wait_for("role:option[Find in current thread]")
        .expect("Find in current thread offered");
    app.press("role:option[Find in current thread]")
        .expect("activate Find in current thread");
    app.wait_for("Find in thread")
        .expect("thread find bar opened");
    // The dialog restores focus as it closes; typing must still land in the find input.
    app.press_key("w").expect("type into the find bar");
    app.press_key("i").expect("type into the find bar");
    app.wait_for("text:wi")
        .expect("find input kept focus after the palette closed");
    app.press_key("escape").expect("close find bar");

    open_palette_and_filter(&mut app, "pin project");
    app.wait_for("role:option[Pin project]")
        .expect("Pin project offered");
    app.press("role:option[Pin project]")
        .expect("activate Pin project");
    app.press_key("f9").expect("check project mutation");
    app.wait_for("text:Mutate: [project.update:pinned=true]")
        .expect("project pinned via RPC");

    // Already pinned project shows Unpin project.
    let mut pinned = start(Root::of::<ThreadWithLinkedPr>());
    open_palette_and_filter(&mut pinned, "unpin project");
    pinned
        .wait_for("role:option[Unpin project]")
        .expect("Unpin project offered when pinned");
    pinned
        .press("role:option[Unpin project]")
        .expect("activate Unpin project");
    pinned.press_key("f9").expect("check project mutation");
    pinned
        .wait_for("text:Mutate: [project.update:pinned=false]")
        .expect("project unpinned via RPC");

    // Share with guest still works cleanly.
    open_palette_and_filter(&mut app, "share with guest");
    app.wait_for("role:option[Share with guest]")
        .expect("Share with guest offered");
    app.press("role:option[Share with guest]")
        .expect("open guest share dialog");
    app.wait_for("The link works once and expires in 24 hours.")
        .expect("guest share dialog opened");
}

fn assert_offered_once(app: &mut Headless, query: &str, option: &str) {
    open_palette_and_filter(app, query);
    app.wait_for(option).expect("command offered");
    assert_eq!(app.screen().count(option), Ok(1), "{option} listed once");
    app.press_key("escape").expect("close palette");
}

#[test]
fn every_host_command_is_listed_exactly_once_where_offered() {
    let mut app = start(Root::of::<ThreadWithLinkedPr>());
    app.press_key("f7").expect("set draft prompt");
    for (query, option) in [
        ("copy prompt link", "role:option[Copy prompt link]"),
        (
            "repo and model",
            "role:option[Copy prompt link with repo and model]",
        ),
        ("copy pr link", "role:option[Copy PR link]"),
        (
            "link pull request",
            "role:option[Link pull request to thread]",
        ),
        (
            "show linked pull requests",
            "role:option[Show linked pull requests]",
        ),
        ("share with guest", "role:option[Share with guest]"),
        (
            "restart agent session",
            "role:option[Restart agent session]",
        ),
        ("unpin project", "role:option[Unpin project]"),
    ] {
        assert_offered_once(&mut app, query, option);
    }
}

#[test]
fn thread_commands_are_hidden_on_a_draft() {
    let mut draft = start(Root::of::<DraftWithPrompt>());
    draft.press_key("f6").expect("open draft");
    draft.wait_for("Draft: ready").expect("draft opened");
    for (query, option) in [
        (
            "find in current thread",
            "role:option[Find in current thread]",
        ),
        (
            "restart agent session",
            "role:option[Restart agent session]",
        ),
        ("share with guest", "role:option[Share with guest]"),
        ("copy thread", "role:option[Copy thread]"),
        (
            "link pull request",
            "role:option[Link pull request to thread]",
        ),
    ] {
        open_palette_and_filter(&mut draft, query);
        assert!(!draft.screen().shows(option), "{option} hidden on a draft");
        draft.press_key("escape").expect("close palette");
    }
}

#[test]
fn copied_links_use_the_configured_hosted_app_url() {
    let mut app = start(Root::of::<ThreadAtStaging>());
    app.press_key("f7").expect("set draft prompt");
    open_palette_and_filter(&mut app, "copy thread");
    app.press("role:option[Copy thread]").expect("copy thread");
    app.wait_for("Thread copied").expect("toast");
    open_palette_and_filter(&mut app, "copy prompt link with repo");
    app.press("role:option[Copy prompt link with repo and model]")
        .expect("copy prompt link with context");
    app.wait_for("Prompt link copied").expect("toast");
    open_palette_and_filter(&mut app, "copy prompt link");
    app.press("role:option[Copy prompt link]")
        .expect("copy plain prompt link");
    app.press_key("f9").expect("read diagnostics");
    app.wait_for(
        "text:Clip: [https://staging.example.test/threads/remote/alpha|https://staging.example.test/new?prompt=Investigate+bug&repo=t3%2Fcode&provider=codex&model=gpt-5.4|https://staging.example.test/new?prompt=Investigate+bug]",
    )
    .expect("every copied link points at the configured origin");
}

#[test]
fn copy_thread_also_writes_the_thread_chip_fragment_with_a_sanitised_label() {
    let mut app = start_waiting(Root::of::<ThreadWithAwkwardTitle>(), "text:[wip]\nfoo");
    open_palette_and_filter(&mut app, "copy thread");
    app.press("role:option[Copy thread]").expect("copy thread");
    app.wait_for("Thread copied").expect("toast");
    app.wait_for("wip foo")
        .expect("the toast shows the sanitised label, not the raw title");
    app.press_key("f9").expect("read diagnostics");
    app.wait_for(
        "text:Fragment: v1 source=remote kind=thread env=remote thread=alpha label=wip foo title=wip foo",
    )
    .expect("the HTML flavour carries the thread chip fragment");

    let mut plain = start(Root::of::<ThreadWithSession>());
    open_palette_and_filter(&mut plain, "copy thread");
    plain
        .press("role:option[Copy thread]")
        .expect("copy thread");
    plain.wait_for("Thread copied").expect("toast");
    plain.press_key("f9").expect("read diagnostics");
    plain
        .wait_for("text:Fragment: v1 source=remote kind=thread env=remote thread=alpha label=Alpha thread title=Alpha thread")
        .expect("an ordinary title is kept as the label");
}

#[test]
fn the_copy_reference_shortcut_copies_like_the_palette_row() {
    let mut app = start(Root::of::<ThreadWithSession>());
    app.press_key("mod+shift+c")
        .expect("copy reference shortcut");
    app.wait_for("Thread copied")
        .expect("the advertised shortcut runs the command");
    app.press_key("f9").expect("read diagnostics");
    app.wait_for("text:Clip: [https://app.t3.codes/threads/remote/alpha]")
        .expect("hosted thread URL copied");

    let mut pr = start(Root::of::<ThreadWithLinkedPr>());
    pr.press_key("mod+shift+c")
        .expect("copy reference shortcut");
    pr.wait_for("PR link copied")
        .expect("a linked pull request is copied instead");
}

#[test]
fn pin_project_on_home_falls_back_to_the_default_project() {
    let mut home = start(Root::of::<HomeView>());
    open_palette_and_filter(&mut home, "pin project");
    home.wait_for("role:option[Pin project]")
        .expect("offered on Home through the default project");
    home.press("role:option[Pin project]").expect("pin");
    home.press_key("f9").expect("check project mutation");
    home.wait_for("text:Mutate: [project.update:pinned=true]")
        .expect("default project pinned via RPC");
}

#[test]
fn pin_and_unpin_failures_use_the_pin_specific_titles() {
    let mut app = start(Root::of::<ThreadWithSession>());
    app.press_key("f4").expect("script a pin failure");
    open_palette_and_filter(&mut app, "pin project");
    app.press("role:option[Pin project]").expect("pin");
    app.wait_for("Failed to pin project")
        .expect("pin failure title");

    let mut pinned = start(Root::of::<ThreadWithLinkedPr>());
    pinned.press_key("f4").expect("script an unpin failure");
    open_palette_and_filter(&mut pinned, "unpin project");
    pinned.press("role:option[Unpin project]").expect("unpin");
    pinned
        .wait_for("Failed to unpin project")
        .expect("unpin failure title");
}

#[test]
fn show_linked_pull_requests_is_gated_on_visible_links_and_opens_the_panel() {
    let mut none = start(Root::of::<ThreadWithSession>());
    open_palette_and_filter(&mut none, "show linked pull requests");
    none.wait_for("Show linked pull requests")
        .expect("listed even without links, as the web lists it");
    assert!(
        !none
            .screen()
            .shows("role:option[Show linked pull requests]"),
        "the row is disabled, not an actionable option"
    );

    let mut linked = start(Root::of::<ThreadWithLinkedPr>());
    open_palette_and_filter(&mut linked, "show linked pull requests");
    linked
        .press("role:option[Show linked pull requests]")
        .expect("open the pull requests panel");
    linked
        .wait_for("role:tab[Pull requests]")
        .expect("the thread's pull requests panel opened");
    assert!(!linked.screen().shows("role:combobox[Command palette]"));
}
