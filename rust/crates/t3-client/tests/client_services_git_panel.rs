#![allow(clippy::unwrap_used)]
mod support {
    pub mod fake_server;
}
use futures::{FutureExt, StreamExt};
use serde_json::json;
use std::{cell::RefCell, rc::Rc};
use support::fake_server::*;
use t3_client::{
    ClientError, ClientStore,
    composition::ClientRuntime,
    connection::PairingInput,
    permissions::SessionGrants,
    types::ThreadKey,
    workspace::git_panel::{actions::*, providers},
};
use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;
use t3_rpc::{Exit, HandlerOutput};

fn setup(h: &mut Harness) -> ClientRuntime<TestPlatform> {
    h.server().state.borrow_mut().extra_methods = t3_contracts::rpc::WS_METHODS
        .iter()
        .filter(|m| {
            matches!(
                m.tag,
                "vcs.pull"
                    | "vcs.init"
                    | "vcs.refreshStatus"
                    | "subscribeVcsStatus"
                    | "subscribeProjectClones"
                    | "vcs.listRefs"
                    | "vcs.switchRef"
                    | "vcs.createRef"
                    | "server.discoverSourceControl"
                    | "review.getDiffPreview"
                    | "sourceControl.publishRepository"
                    | "git.runStackedAction"
                    | "projects.readFile"
                    | "shell.openInEditor"
                    | "server.upsertKeybinding"
                    | "server.removeKeybinding"
                    | "terminal.open"
                    | "terminal.write"
            )
        })
        .map(|m| {
            (
                m.tag,
                if m.streams {
                    t3_rpc::CallKind::Stream
                } else {
                    t3_rpc::CallKind::Unary
                },
            )
        })
        .collect();
    let runtime = ClientRuntime::new(
        h.platform.clone(),
        ClientStore::default(),
        OrchestrationV2CreationSource::Web,
        h.spawner(),
    );
    h.run(runtime.pair(PairingInput {
        pairing_url: Some(format!("{ORIGIN}/pair#token=pairing-token")),
        ..PairingInput::default()
    }))
    .unwrap();
    h.settle();
    runtime
        .client(&environment("env"))
        .unwrap()
        .session()
        .unwrap()
        .update_grants(SessionGrants {
            valid: true,
            scopes: [
                "orchestration:read",
                "orchestration:operate",
                "source-control:write",
                "filesystem:read",
                "settings:write",
                "terminal:operate",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        });
    runtime
}
fn scope() -> GitActionsScope {
    GitActionsScope {
        thread: ThreadKey {
            environment: environment("env"),
            thread: thread("t1"),
        },
        cwd: "/repo".into(),
        draft_id: None,
    }
}

#[test]
fn commands_preserve_typed_failure_and_recheck_live_grants() {
    let mut h = Harness::new("env");
    let runtime = setup(&mut h);
    let calls = Rc::new(RefCell::new(Vec::new()));
    let recorded = calls.clone();
    h.server().state.borrow_mut().extra_rpc = Some(Rc::new(move |request| {
        if request.tag != "vcs.pull" {
            return None;
        }
        recorded.borrow_mut().push(request.payload.clone());
        Some(HandlerOutput::Unary(async { Exit::fail(json!({"_tag":"GitCommandError","operation":"pull","command":"git pull","cwd":"/repo","detail":"upstream refused"})) }.boxed_local()))
    }));
    let service = runtime.git_panel();
    let error = h.run(service.pull(&scope())).unwrap_err();
    assert!(matches!(error, ClientError::Rejected { .. }), "{error:?}");
    assert!(error.to_string().contains("upstream refused"));
    let future = service.pull(&scope());
    runtime
        .client(&environment("env"))
        .unwrap()
        .session()
        .unwrap()
        .update_grants(SessionGrants {
            valid: true,
            scopes: Default::default(),
        });
    assert!(matches!(
        h.run(future),
        Err(ClientError::PermissionDenied { .. })
    ));
    assert_eq!(calls.borrow().len(), 1);
}

#[test]
fn unsupported_ref_listing_surfaces_error_and_finishes_loading() {
    let mut h = Harness::new("env");
    let runtime = setup(&mut h);
    h.server().state.borrow_mut().extra_rpc = Some(Rc::new(|request| {
        (request.tag == "vcs.listRefs").then(|| {
            HandlerOutput::Unary(
                async { Exit::die("Unknown request tag: vcs.listRefs") }.boxed_local(),
            )
        })
    }));
    let selector = runtime.branch_selector(&scope().thread);
    h.settle();
    let snapshot = selector.snapshot();
    assert!(snapshot.unsupported);
    assert!(snapshot.error.is_some());
    assert!(!snapshot.loading);
    assert!(!snapshot.pending);
    assert!(snapshot.refs.is_empty());
}

#[test]
fn pull_success_and_publish_destination_are_composed_without_raw_ui_rpc() {
    let mut h = Harness::new("env");
    let runtime = setup(&mut h);
    h.server().state.borrow_mut().extra_rpc = Some(Rc::new(|request| {
        (request.tag == "vcs.pull").then(|| HandlerOutput::Unary(async { Exit::success(json!({"status":"pulled","refName":"feature","upstreamRef":"origin/feature"})) }.boxed_local()))
    }));
    assert_eq!(
        h.run(runtime.git_panel().pull(&scope()))
            .unwrap()
            .ref_name
            .0,
        "feature"
    );
    let disconnected = GitActionsScope {
        thread: ThreadKey {
            environment: environment("other"),
            thread: thread("t1"),
        },
        ..scope()
    };
    assert!(matches!(
        h.run(runtime.git_panel().pull(&disconnected)),
        Err(ClientError::Disconnected)
    ));
}

#[test]
fn stacked_actions_use_distinct_transport_ids_and_the_existing_default_ref_policy() {
    use t3_contracts::git::GitStackedAction;
    let mut h = Harness::new("env");
    let runtime = setup(&mut h);
    let requests = Rc::new(RefCell::new(Vec::new()));
    let recorded = requests.clone();
    h.server().state.borrow_mut().extra_rpc = Some(Rc::new(move |request| {
        if request.tag != "git.runStackedAction" {
            return None;
        }
        recorded.borrow_mut().push(request.payload.clone());
        let mut result: serde_json::Value = serde_json::from_str(
            include_str!("../../../fixtures/samples/GitRunStackedActionResult.jsonl")
                .lines()
                .next()
                .unwrap(),
        )
        .unwrap();
        result["action"] = request.payload["action"].clone();
        let event = json!({"kind":"action_finished","actionId":request.payload["actionId"],"cwd":request.payload["cwd"],"action":request.payload["action"],"result":result});
        Some(HandlerOutput::Stream(
            futures::stream::once(async move { Ok(vec![event]) }).boxed_local(),
        ))
    }));
    for action in [
        GitStackedAction::Commit,
        GitStackedAction::Push,
        GitStackedAction::CommitPush,
        GitStackedAction::CreatePr,
        GitStackedAction::CommitPushPr,
    ] {
        assert_eq!(
            t3_client::workspace::git_actions::requires_default_branch_confirmation(&action, true),
            action != GitStackedAction::Commit
        );
        assert!(
            !t3_client::workspace::git_actions::requires_default_branch_confirmation(
                &action, false
            )
        );
        let result = h
            .run(runtime.git_panel().run_action(
                &scope(),
                GitActionRequest {
                    action: action.clone(),
                    commit_message: Some("Chosen commit".into()),
                    feature_branch: false,
                    file_paths: Some(vec!["src/file.rs".into()]),
                },
            ))
            .unwrap();
        assert_eq!(result.action, action);
    }
    let requests = requests.borrow();
    assert_eq!(requests.len(), 5);
    let ids: std::collections::BTreeSet<_> = requests
        .iter()
        .map(|p| p["actionId"].as_str().unwrap())
        .collect();
    assert_eq!(ids.len(), 5);
    assert!(requests.iter().all(|p| p["threadId"] == "t1"
        && p["commitMessage"] == "Chosen commit"
        && p["filePaths"] == json!(["src/file.rs"])));
}

#[test]
fn status_watch_emits_current_then_updates_and_releases_the_server_stream() {
    let mut h = Harness::new("env");
    let runtime = setup(&mut h);
    let (send, receive) = futures::channel::mpsc::unbounded();
    let receive = Rc::new(RefCell::new(Some(receive)));
    let open = Rc::new(std::cell::Cell::new(0));
    let observed = open.clone();
    let refreshes = Rc::new(std::cell::Cell::new(0));
    let observed_refreshes = refreshes.clone();
    struct Guard(Rc<std::cell::Cell<usize>>);
    impl Drop for Guard {
        fn drop(&mut self) {
            self.0.set(self.0.get() - 1);
        }
    }
    h.server().state.borrow_mut().extra_rpc =
        Some(Rc::new(move |request| match request.tag.as_str() {
            "vcs.refreshStatus" => {
                observed_refreshes.set(observed_refreshes.get() + 1);
                Some(HandlerOutput::Unary(
                    async { Exit::success(json!({})) }.boxed_local(),
                ))
            }
            "subscribeVcsStatus" => {
                observed.set(observed.get() + 1);
                let guard = Guard(observed.clone());
                let stream = receive
                    .borrow_mut()
                    .take()
                    .unwrap()
                    .map(move |value| {
                        let _ = &guard;
                        Ok(vec![value])
                    })
                    .boxed_local();
                Some(HandlerOutput::Stream(stream))
            }
            _ => None,
        }));
    let mut watch = runtime.git_panel().watch_actions(
        &scope(),
        GitActionsWatch {
            show_actions: false,
            show_changes: false,
        },
    );
    assert!(
        futures::executor::block_on(watch.next())
            .unwrap()
            .status
            .is_none()
    );
    h.settle();
    assert_eq!(
        open.get(),
        1,
        "status error: {:?}, calls: {:?}",
        futures::executor::block_on(watch.next()).and_then(|s| s.status_error),
        h.server().state.borrow().rpc_calls
    );
    let mut status: serde_json::Value = serde_json::from_str(
        include_str!("../../../fixtures/samples/VcsStatusResult.jsonl")
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    status["refName"] = json!("feature");
    send.unbounded_send(json!({"_tag":"snapshot","local":status,"remote":null}))
        .unwrap();
    h.settle();
    let latest = futures::executor::block_on(watch.next()).unwrap();
    assert_eq!(
        latest.status.unwrap().ref_name.as_ref().unwrap().0,
        "feature"
    );
    let before = refreshes.get();
    h.platform
        .fake
        .emit(t3_client::platform::PlatformEvent::Foreground);
    h.platform
        .fake
        .emit(t3_client::platform::PlatformEvent::Foreground);
    h.settle();
    assert_eq!(refreshes.get(), before);
    h.advance(250);
    assert_eq!(refreshes.get(), before + 1);
    drop(watch);
    h.settle();
    assert_eq!(open.get(), 0);
}

#[test]
fn unknown_hosts_keep_generic_change_request_terms_and_gitcafe_has_its_mark() {
    assert_eq!(providers::presentation(None).short_label, "PR");
    assert_eq!(
        providers::presentation(Some("gitlab")).singular,
        "merge request"
    );
    let unknown = providers::presentation(Some("new-forge"));
    assert_eq!(unknown.short_label, "change request");
    assert!(unknown.icon_svg.is_none());
    assert!(
        providers::publish_providers()
            .iter()
            .find(|p| p.kind == "gitcafe")
            .unwrap()
            .icon_svg
            .contains("#a78bfa")
    );
}

#[test]
fn branch_checkout_uses_remote_result_and_rolls_back_a_refused_optimistic_selection() {
    let mut h = Harness::new("env");
    let runtime = setup(&mut h);
    let calls = Rc::new(RefCell::new(Vec::new()));
    let recorded = calls.clone();
    h.server().state.borrow_mut().extra_rpc = Some(Rc::new(move |request| {
        recorded
            .borrow_mut()
            .push((request.tag.clone(), request.payload.clone()));
        match request.tag.as_str() {
            "vcs.listRefs" => Some(HandlerOutput::Unary(async { Exit::success(json!({"refs":[{"name":"origin/feature","isRemote":true,"current":false,"isDefault":false,"worktreePath":null}],"isRepo":true,"hasPrimaryRemote":true,"nextCursor":null,"totalCount":1})) }.boxed_local())),
            "vcs.switchRef" => Some(HandlerOutput::Unary(async { Exit::success(json!({"refName":"local-feature"})) }.boxed_local())),
            "orchestration.dispatchCommand" => Some(HandlerOutput::Unary(async { Exit::success(json!({"sequence":1})) }.boxed_local())),
            _ => None,
        }
    }));
    let selector = runtime.git_panel().branch_selector(&scope().thread);
    h.settle();
    h.run(selector.select_ref("origin/feature".into())).unwrap();
    assert!(
        calls.borrow().iter().any(
            |(tag, p)| tag == "orchestration.dispatchCommand" && p["branch"] == "local-feature"
        )
    );
    let before = selector.snapshot().branch.clone();
    let (release, wait) = futures::channel::oneshot::channel::<()>();
    let wait = Rc::new(RefCell::new(Some(wait)));
    h.server().state.borrow_mut().extra_rpc = Some(Rc::new(move |request| {
        if request.tag != "vcs.switchRef" {
            return None;
        }
        let wait = wait.borrow_mut().take().unwrap();
        Some(HandlerOutput::Unary(async move {
            wait.await.unwrap();
            Exit::fail(json!({"_tag":"GitCommandError","operation":"switchRef","command":"git switch origin/feature","cwd":"/repo","detail":"checkout refused"}))
        }.boxed_local()))
    }));
    let pending = selector.select_ref("origin/feature".into());
    let result = Rc::new(RefCell::new(None));
    let received = result.clone();
    h.spawn(async move {
        *received.borrow_mut() = Some(pending.await);
    });
    h.settle();
    assert!(selector.snapshot().pending);
    assert_eq!(selector.snapshot().branch.as_deref(), Some("feature"));
    release.send(()).unwrap();
    h.settle();
    assert!(matches!(
        result.borrow_mut().take().unwrap(),
        Err(ClientError::Rejected { .. })
    ));
    assert!(!selector.snapshot().pending);
    assert_eq!(selector.snapshot().branch, before);
    let (release, wait) = futures::channel::oneshot::channel::<()>();
    let wait = Rc::new(RefCell::new(Some(wait)));
    let recorded = calls.clone();
    h.server().state.borrow_mut().extra_rpc = Some(Rc::new(move |request| {
        if request.tag == "vcs.switchRef" {
            let wait = wait.borrow_mut().take().unwrap();
            return Some(HandlerOutput::Unary(
                async move {
                    wait.await.unwrap();
                    Exit::success(json!({"refName":"after-dismissal"}))
                }
                .boxed_local(),
            ));
        }
        if request.tag == "orchestration.dispatchCommand" {
            recorded
                .borrow_mut()
                .push((request.tag.clone(), request.payload.clone()));
            return Some(HandlerOutput::Unary(
                async { Exit::success(json!({"sequence":2})) }.boxed_local(),
            ));
        }
        None
    }));
    assert!(
        selector
            .select_ref("origin/feature".into())
            .now_or_never()
            .is_none()
    );
    h.settle();
    assert!(selector.snapshot().pending);
    release.send(()).unwrap();
    h.settle();
    assert!(!selector.snapshot().pending);
    assert!(
        calls
            .borrow()
            .iter()
            .any(|(tag, p)| tag == "orchestration.dispatchCommand"
                && p["branch"] == "after-dismissal")
    );
    let future = selector.select_ref("origin/feature".into());
    runtime
        .client(&environment("env"))
        .unwrap()
        .session()
        .unwrap()
        .update_grants(SessionGrants {
            valid: true,
            scopes: Default::default(),
        });
    assert!(matches!(
        h.run(future),
        Err(ClientError::PermissionDenied { .. })
    ));
    assert!(!selector.snapshot().pending);
}

#[test]
fn details_visibility_migrates_legacy_usage_and_persists_only_nondefault_choices() {
    use t3_client::workspace::git_panel::details::*;
    use t3_contracts::settings::ClientSettingsSchema;
    for (legacy, overrides, usage, context) in [
        (false, vec![], false, false),
        (true, vec![], true, true),
        (true, vec![("usage", false)], false, true),
        (
            false,
            vec![("usage", true), ("retired", false)],
            true,
            false,
        ),
    ] {
        let settings = ClientSettingsSchema {
            thread_details_usage_enabled: legacy,
            thread_details_items: overrides.into_iter().map(|(k, v)| (k.into(), v)).collect(),
            ..Default::default()
        };
        let visible = resolve_thread_details_visibility(&settings);
        assert_eq!(visible["usage"], usage);
        assert_eq!(visible["context"], context);
        assert!(visible["servers"]);
    }
    let mut h = Harness::new("env");
    let runtime = setup(&mut h);
    h.run(
        runtime
            .git_panel()
            .set_details_item("servers".into(), false),
    )
    .unwrap();
    assert_eq!(
        runtime
            .composer_preferences()
            .snapshot()
            .thread_details_items
            .get("servers"),
        Some(&false)
    );
    h.run(runtime.git_panel().set_details_item("servers".into(), true))
        .unwrap();
    assert!(
        runtime
            .composer_preferences()
            .snapshot()
            .thread_details_items
            .is_empty()
    );
    assert_eq!(
        resolve_thread_details_density(200.0, 300.0, 180.0),
        DetailsDensity::Compact
    );
    assert_eq!(
        resolve_thread_details_density(160.0, 300.0, 180.0),
        DetailsDensity::Essential
    );
}

#[test]
fn remote_editor_urls_preserve_ssh_usernames_windows_paths_and_file_positions() {
    use t3_client::workspace::git_panel::editors::*;
    use t3_contracts::editor::EditorId;
    for (editor, path, file, expected) in [
        (
            EditorId::Vscode,
            "C:\\Users\\theo",
            false,
            "vscode://vscode-remote/ssh-remote+sol/C%3A/Users/theo",
        ),
        (
            EditorId::Zed,
            "C:\\Users\\theo",
            false,
            "zed://ssh/sol/Users/theo",
        ),
        (
            EditorId::Zed,
            "/C:/project",
            false,
            "zed://ssh/sol/C%3A/project",
        ),
        (
            EditorId::Cursor,
            "/tmp/my file.md",
            true,
            "cursor://vscode-remote/ssh-remote+sol/tmp/my%20file.md:1:1",
        ),
        (
            EditorId::Pycharm,
            "/tmp/my file.md",
            true,
            "jetbrains://gateway/ssh/environment?h=sol&launchIde=true&ideHint=PY&projectHint=%2Ftmp%2Fmy+file.md",
        ),
    ] {
        assert_eq!(
            remote_open_url(editor, "sol", None, path, file).as_deref(),
            Some(expected)
        );
    }
    for (alias, user) in [
        ("server", Some("admin-user")),
        ("admin-user@server", None),
        ("old-user@server", Some("admin-user")),
        ("admin-user@server", Some("  ")),
    ] {
        assert_eq!(
            ssh_host(alias, user),
            ("server".into(), Some("admin-user".into()))
        );
    }
    assert!(remote_fallback_editors("Android", true).is_empty());
    assert_eq!(
        remote_fallback_editors("MacIntel", false),
        vec![EditorId::Vscode]
    );
    assert!(should_show_open_in_picker(
        Some("Project"),
        &environment("host"),
        Some(&environment("wsl")),
        Some(&environment("host")),
        &RemoteOpenState::LocalExec
    ));
    assert!(!should_show_open_in_picker(
        Some("Project"),
        &environment("other"),
        Some(&environment("wsl")),
        Some(&environment("host")),
        &RemoteOpenState::LocalExec
    ));
}

#[test]
fn remote_editor_refusal_does_not_mark_hint_seen_and_accepted_launch_does() {
    use t3_client::platform::HostReply;
    use t3_contracts::editor::EditorId;
    let mut h = Harness::new("env");
    let runtime = setup(&mut h);
    let (generation, mut config) = {
        let snapshot = runtime.store().snapshot();
        let env = &snapshot.environments[&environment("env")];
        (env.session, env.config.as_ref().unwrap().as_ref().clone())
    };
    config.remote_open_targets = Some(t3_contracts::server::ServerCompatibleArray(vec![
        serde_json::from_value(json!({"kind":"tailscale","host":"sol.example"})).unwrap(),
    ]));
    runtime
        .store()
        .set_config(&environment("env"), generation, config);
    let picker = runtime.editor_picker(&scope().thread);
    h.platform
        .fake
        .push_host(Err(t3_client::platform::PlatformError::Denied(
            "scheme not registered".into(),
        )));
    assert!(!h.run(picker.open_editor(EditorId::Vscode)).unwrap());
    assert!(!h.run(picker.snapshot()).unwrap().hint_seen);
    h.platform.fake.push_host(Ok(HostReply::Unit));
    assert!(h.run(picker.open_editor(EditorId::Vscode)).unwrap());
    let snapshot = h.run(picker.snapshot()).unwrap();
    assert!(snapshot.hint_seen);
    assert_eq!(snapshot.preferred, Some(EditorId::Vscode));
}

#[test]
fn scripts_parse_import_defaults_filter_duplicates_and_save_through_server_settings() {
    use t3_client::workspace::git_panel::scripts::*;
    let file=parse_project_file(r#"{/* comment */"scripts":[{"name":"Start","command":"bun dev", "runOnWorktreeCreate":true,"async":false,"previewUrl":"http://localhost:3000",},],}"#).unwrap();
    let imported = file.scripts.unwrap().remove(0);
    let input = import_input(imported.clone(), true);
    assert!(input.script.wait_for_setup);
    assert!(!input.script.auto_open_preview);
    assert_eq!(input.keybinding, Some(None));
    let mut h = Harness::new("env");
    let runtime = setup(&mut h);
    let settings = Rc::new(RefCell::new(
        serde_json::to_value(t3_contracts::settings::ServerSettings::default()).unwrap(),
    ));
    let recorded = settings.clone();
    let terminal_calls = Rc::new(RefCell::new(Vec::new()));
    let recorded_terminal_calls = terminal_calls.clone();
    h.server().state.borrow_mut().extra_rpc = Some(Rc::new(move |request| {
        if request.tag == "terminal.open" {
            recorded_terminal_calls
                .borrow_mut()
                .push((request.tag.clone(), request.payload.clone()));
            let mut snapshot: serde_json::Value = serde_json::from_str(
                include_str!("../../../fixtures/samples/TerminalSessionSnapshot.jsonl")
                    .lines()
                    .next()
                    .unwrap(),
            )
            .unwrap();
            snapshot["threadId"] = request.payload["threadId"].clone();
            snapshot["terminalId"] = request.payload["terminalId"].clone();
            return Some(HandlerOutput::Unary(
                async move { Exit::success(snapshot) }.boxed_local(),
            ));
        }
        if request.tag == "terminal.write" {
            recorded_terminal_calls
                .borrow_mut()
                .push((request.tag.clone(), request.payload.clone()));
            return Some(HandlerOutput::Unary(
                async { Exit::success(serde_json::Value::Null) }.boxed_local(),
            ));
        }
        if request.tag != "server.updateSettings" {
            return None;
        }
        let mut settings = recorded.borrow_mut();
        for (k, v) in request.payload["patch"].as_object().unwrap() {
            settings[k] = v.clone();
        }
        let result = settings.clone();
        Some(HandlerOutput::Unary(
            async move { Exit::success(result) }.boxed_local(),
        ))
    }));
    let scripts = runtime.git_panel().project_scripts(&scope().thread);
    h.run(scripts.add(input)).unwrap();
    let snapshot = h.run(scripts.snapshot()).unwrap();
    let saved = snapshot
        .scripts
        .iter()
        .find(|s| s.name.0 == "Start")
        .unwrap();
    assert!(importable_scripts(&[imported], &snapshot.scripts).is_empty());
    let id = saved.id.0.clone();
    let terminal = h.run(scripts.run(id.clone())).unwrap();
    let calls = terminal_calls.borrow();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].0, "terminal.open");
    assert_eq!(calls[0].1["cwd"], "/workspace/project");
    assert_eq!(
        calls[0].1["env"]["T3CODE_PROJECT_ROOT"],
        "/workspace/project"
    );
    assert_eq!(calls[1].0, "terminal.write");
    assert_eq!(calls[1].1["terminalId"], terminal);
    assert_eq!(calls[1].1["data"], " bun dev\r");
    drop(calls);
    h.run(scripts.delete(id.clone())).unwrap();
    assert!(
        !h.run(scripts.snapshot())
            .unwrap()
            .scripts
            .iter()
            .any(|s| s.id.0 == id)
    );
    assert_eq!(
        reusable_script_terminal(
            &["panel".into(), "busy".into(), "idle".into()],
            "panel",
            &["panel".into()].into_iter().collect(),
            &["busy".into()]
        ),
        Some("idle".into())
    );
}
