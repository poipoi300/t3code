#![allow(clippy::unwrap_used)]
use futures_util::FutureExt;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use t3_contracts::preview::*;
use t3_contracts::preview_automation::PreviewAutomationControlReason as Reason;
use t3_preview::{
    Error, PreviewManager,
    activity::PreviewAgentActivity,
    control::{Action, SessionControl},
};
use tokio::sync::oneshot;

fn open(thread: &str, url: Option<&str>, runtime: Option<PreviewRuntime>) -> PreviewOpenInput {
    serde_json::from_value(json!({"threadId":thread,"url":url,"runtime":runtime})).unwrap()
}
fn revision(event: &PreviewEvent) -> u64 {
    match event {
        PreviewEvent::Opened { revision, .. }
        | PreviewEvent::Navigated { revision, .. }
        | PreviewEvent::Failed { revision, .. }
        | PreviewEvent::Resized { revision, .. }
        | PreviewEvent::Closed { revision, .. } => *revision,
    }
}

#[tokio::test]
async fn normalized_open_and_global_event_order_are_atomic() {
    let manager = PreviewManager::default();
    let mut first = manager.subscribe_events();
    let mut second = manager.subscribe_events();
    let opened = manager
        .open(open(
            "a",
            Some("localhost:5173"),
            Some(PreviewRuntime::Server),
        ))
        .await
        .unwrap();
    let list = manager
        .list(PreviewListInput {
            thread_id: "a".into(),
        })
        .await;
    assert!(opened.tab_id.as_str().starts_with("tab_1_"));
    assert!(opened.tab_id.as_str().ends_with(list.server_epoch.as_str()));
    assert_eq!(
        opened.nav_status,
        PreviewNavStatus::Loading {
            url: "http://localhost:5173/".into(),
            title: "".into()
        }
    );
    assert_eq!(first.recv().await.unwrap(), second.recv().await.unwrap());
    assert_eq!(list.revision, 1);
    let mut tasks = Vec::new();
    for n in 0..40 {
        let manager = manager.clone();
        tasks.push(tokio::spawn(async move {
            manager
                .open(open(&format!("t{n}"), None, None))
                .await
                .unwrap()
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }
    for n in 2..=41 {
        assert_eq!(revision(&first.recv().await.unwrap()), n);
    }
    assert_eq!(
        manager
            .list(PreviewListInput {
                thread_id: "a".into()
            })
            .await
            .revision,
        41
    );
}

#[tokio::test]
async fn navigation_preserves_profile_viewport_and_reveal_request() {
    let manager = PreviewManager::default();
    let mut events = manager.subscribe_events();
    let mut input = open("a", None, None);
    input.profile_id = Some(Some("work".into()));
    let tab = manager.open(input).await.unwrap();
    let viewport = PreviewViewportSetting::Freeform {
        width: 1024,
        height: 768,
    };
    manager
        .resize(PreviewResizeInput {
            thread_id: "a".into(),
            tab_id: tab.tab_id.clone(),
            viewport: viewport.clone(),
        })
        .await
        .unwrap();
    manager
        .request_reveal("a", tab.tab_id.as_str(), true)
        .await
        .unwrap();
    let before = manager
        .list(PreviewListInput {
            thread_id: "a".into(),
        })
        .await
        .sessions
        .remove(0);
    manager
        .report_status(PreviewReportStatusInput {
            thread_id: "a".into(),
            tab_id: tab.tab_id.clone(),
            nav_status: PreviewNavStatus::Success {
                url: "https://example.com/".into(),
                title: "Ready".into(),
            },
            can_go_back: true,
            can_go_forward: false,
        })
        .await
        .unwrap();
    let after = manager
        .list(PreviewListInput {
            thread_id: "a".into(),
        })
        .await
        .sessions
        .remove(0);
    assert_eq!(after.profile_id, Some(Some("work".into())));
    assert_eq!(after.viewport, Some(Some(viewport)));
    assert_eq!(after.reveal_request, before.reveal_request);
    assert!(after.can_go_back);
    manager
        .request_reveal("a", tab.tab_id.as_str(), false)
        .await
        .unwrap();
    assert_ne!(
        manager
            .list(PreviewListInput {
                thread_id: "a".into()
            })
            .await
            .sessions[0]
            .reveal_request,
        before.reveal_request
    );
    for n in 1..=5 {
        assert_eq!(revision(&events.recv().await.unwrap()), n);
    }
}

#[tokio::test]
async fn client_cannot_report_server_status_or_change_any_state() {
    let manager = PreviewManager::default();
    let tab = manager
        .open(open("a", None, Some(PreviewRuntime::Server)))
        .await
        .unwrap();
    let mut events = manager.subscribe_events();
    let before = manager
        .list(PreviewListInput {
            thread_id: "a".into(),
        })
        .await;
    let error = manager
        .report_status(PreviewReportStatusInput {
            thread_id: "a".into(),
            tab_id: tab.tab_id,
            nav_status: PreviewNavStatus::Success {
                url: "https://attacker.test/".into(),
                title: "Forged".into(),
            },
            can_go_back: true,
            can_go_forward: true,
        })
        .await
        .unwrap_err();
    assert!(matches!(error, Error::ControlRequired));
    assert_eq!(
        manager
            .list(PreviewListInput {
                thread_id: "a".into()
            })
            .await,
        before
    );
    assert!(events.try_recv().is_err());
}

#[tokio::test]
async fn failures_and_reload_requests_preserve_wire_fields() {
    let manager = PreviewManager::default();
    let tab = manager.open(open("a", None, None)).await.unwrap();
    let mut events = manager.subscribe_events();
    manager
        .report_status(PreviewReportStatusInput {
            thread_id: "a".into(),
            tab_id: tab.tab_id.clone(),
            nav_status: PreviewNavStatus::LoadFailed {
                url: "https://example.com/file".into(),
                title: "".into(),
                code: -3,
                description: "ERR_ABORTED".into(),
                download: Some(Some(PreviewFileDownload {
                    id: "download".into(),
                    file_name: "paper.pdf".into(),
                })),
            },
            can_go_back: false,
            can_go_forward: false,
        })
        .await
        .unwrap();
    assert!(
        matches!(events.recv().await.unwrap(),PreviewEvent::Failed {download:Some(Some(PreviewFileDownload {file_name,..})),..} if file_name=="paper.pdf")
    );
    manager.adjust(serde_json::from_value(json!({"threadId":"a","tabId":tab.tab_id,"hardReload":true,"clear":"cache","colorScheme":"dark","zoomFactor":1.25})).unwrap()).await.unwrap();
    let PreviewEvent::Resized {
        request: Some(Some(request)),
        snapshot,
        ..
    } = events.recv().await.unwrap()
    else {
        panic!("expected resized")
    };
    assert_eq!(request.clear, Some(Some(PreviewClearTarget::Cache)));
    assert_eq!(request.hard_reload, Some(Some(true)));
    assert_eq!(
        snapshot.color_scheme,
        Some(Some(PreviewAppearancePreference::Dark))
    );
    assert_eq!(snapshot.zoom_factor.flatten().unwrap().value(), 1.25);
}

#[tokio::test]
async fn close_is_thread_scoped_idempotent_and_individually_revisioned() {
    let manager = PreviewManager::default();
    let a = manager.open(open("a", None, None)).await.unwrap();
    manager.open(open("a", None, None)).await.unwrap();
    manager.open(open("b", None, None)).await.unwrap();
    let mut events = manager.subscribe_events();
    manager
        .close(PreviewCloseInput {
            thread_id: "b".into(),
            tab_id: Some(Some(a.tab_id)),
        })
        .await
        .unwrap();
    assert!(events.try_recv().is_err());
    manager
        .close(PreviewCloseInput {
            thread_id: "a".into(),
            tab_id: None,
        })
        .await
        .unwrap();
    assert_eq!(revision(&events.recv().await.unwrap()), 4);
    assert_eq!(revision(&events.recv().await.unwrap()), 5);
    manager
        .close(PreviewCloseInput {
            thread_id: "a".into(),
            tab_id: None,
        })
        .await
        .unwrap();
    assert!(events.try_recv().is_err());
    assert_eq!(
        manager
            .list(PreviewListInput {
                thread_id: "b".into()
            })
            .await
            .sessions
            .len(),
        1
    );
}

#[tokio::test]
async fn headless_is_monotone_and_announce_preserves_snapshot_timestamp() {
    let manager = PreviewManager::default();
    let tab = manager.open(open("a", None, None)).await.unwrap();
    let mut events = manager.subscribe_events();
    manager
        .mark_headless("a", tab.tab_id.as_str())
        .await
        .unwrap();
    let first = events.recv().await.unwrap();
    assert_eq!(revision(&first), 2);
    manager
        .mark_headless("a", tab.tab_id.as_str())
        .await
        .unwrap();
    assert!(events.try_recv().is_err());
    let before = manager
        .list(PreviewListInput {
            thread_id: "a".into(),
        })
        .await
        .sessions
        .remove(0);
    manager.announce("a", tab.tab_id.as_str()).await.unwrap();
    let PreviewEvent::Resized { snapshot, .. } = events.recv().await.unwrap() else {
        panic!()
    };
    assert_eq!(snapshot, before);
}

#[tokio::test]
async fn invalid_input_and_cross_thread_lookups_emit_nothing() {
    let manager = PreviewManager::default();
    let mut events = manager.subscribe_events();
    assert!(
        manager
            .open(open("a", Some("file:///secret"), None))
            .await
            .is_err()
    );
    assert!(events.try_recv().is_err());
    let tab = manager.open(open("a", None, None)).await.unwrap();
    events.recv().await.unwrap();
    assert!(matches!(
        manager
            .resize(PreviewResizeInput {
                thread_id: "b".into(),
                tab_id: tab.tab_id.clone(),
                viewport: PreviewViewportSetting::Fill {}
            })
            .await,
        Err(Error::TabNotFound)
    ));
    assert!(
        manager
            .resize(PreviewResizeInput {
                thread_id: "a".into(),
                tab_id: tab.tab_id,
                viewport: PreviewViewportSetting::Freeform {
                    width: 3840,
                    height: 3840
                }
            })
            .await
            .is_err()
    );
    assert!(events.try_recv().is_err());
}

#[tokio::test]
async fn ownership_handoff_drains_running_work_and_revokes_queued_actions() {
    let control = SessionControl::new(Some("agent".into()));
    let events = Arc::new(Mutex::new(Vec::new()));
    let (started, start) = oneshot::channel();
    let (finish, done) = oneshot::channel();
    let log = events.clone();
    let running = control.agent(
        "agent",
        None,
        async move {
            log.lock().unwrap().push("agent started");
            started.send(()).unwrap();
            done.await.unwrap();
            log.lock().unwrap().push("agent finished");
            Ok(json!("finished"))
        }
        .boxed(),
    );
    start.await.unwrap();
    let log = events.clone();
    let stale = control.agent(
        "agent",
        None,
        async move {
            log.lock().unwrap().push("STALE");
            Ok(Value::Null)
        }
        .boxed(),
    );
    let taking = control.take("viewer").unwrap();
    assert_eq!(control.controller().as_deref(), Some("viewer"));
    assert_eq!(control.generation(), 1);
    assert!(matches!(
        control
            .agent("agent", None, async { Ok(Value::Null) }.boxed())
            .await,
        Err(Error::Control(Reason::HumanControl))
    ));
    let log = events.clone();
    let typing = control.human(
        "viewer",
        async move {
            log.lock().unwrap().push("human typed");
            Ok(Value::Null)
        }
        .boxed(),
    );
    finish.send(()).unwrap();
    assert_eq!(running.await.unwrap(), json!("finished"));
    assert!(matches!(
        stale.await,
        Err(Error::Control(Reason::Interrupted))
    ));
    taking.await.unwrap();
    typing.await.unwrap();
    assert_eq!(
        *events.lock().unwrap(),
        vec!["agent started", "agent finished", "human typed"]
    );
}

#[tokio::test]
async fn another_viewer_cannot_steal_release_or_replace_queued_input() {
    let control = SessionControl::new(Some("a".into()));
    control.take("viewer").unwrap().await.unwrap();
    assert!(control.take("other").is_err());
    assert!(
        control
            .release("other", async { Ok(Value::Null) }.boxed())
            .is_err()
    );
    control
        .disconnect("other", async { Ok(Value::Null) }.boxed())
        .await
        .unwrap();
    assert_eq!(control.controller().as_deref(), Some("viewer"));
    assert!(matches!(
        control
            .agent("wrong", None, async { Ok(Value::Null) }.boxed())
            .await,
        Err(Error::Control(Reason::AgentMismatch))
    ));
}

#[tokio::test]
async fn adjacent_hover_work_is_coalesced_but_click_keeps_order() {
    let control = SessionControl::new(Some("agent".into()));
    control.take("viewer").unwrap().await.unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let (started, start) = oneshot::channel();
    let (finish, done) = oneshot::channel();
    let running = control.human(
        "viewer",
        async move {
            started.send(()).unwrap();
            done.await.unwrap();
            Ok(Value::Null)
        }
        .boxed(),
    );
    start.await.unwrap();
    let log = events.clone();
    let action: Action = Arc::new(move |value| {
        let log = log.clone();
        async move {
            log.lock().unwrap().push(value);
            Ok(Value::Null)
        }
        .boxed()
    });
    let mut waiting = vec![];
    for n in 0..120 {
        waiting.push(control.human_latest(
            "viewer",
            "hover:0",
            json!(n),
            action.clone(),
            |_, next| next,
        ));
    }
    let log = events.clone();
    let click = control.human(
        "viewer",
        async move {
            log.lock().unwrap().push(json!("click"));
            Ok(Value::Null)
        }
        .boxed(),
    );
    let after = control.human_latest("viewer", "hover:0", json!(200), action, |_, next| next);
    finish.send(()).unwrap();
    running.await.unwrap();
    for work in waiting {
        work.await.unwrap();
    }
    click.await.unwrap();
    after.await.unwrap();
    assert_eq!(
        *events.lock().unwrap(),
        vec![json!(119), json!("click"), json!(200)]
    );
}

#[tokio::test]
async fn release_and_retaking_never_revive_stale_work() {
    let control = SessionControl::new(Some("agent".into()));
    let (finish, done) = oneshot::channel();
    let (started, start) = oneshot::channel();
    let running = control.agent(
        "agent",
        None,
        async move {
            started.send(()).unwrap();
            done.await.unwrap();
            Ok(Value::Null)
        }
        .boxed(),
    );
    start.await.unwrap();
    let stale = control.agent("agent", None, async { Ok(json!("stale")) }.boxed());
    let taken = control.take("viewer").unwrap();
    let human = control.human("viewer", async { Ok(json!("stale human")) }.boxed());
    let released = control
        .release("viewer", async { Ok(Value::Null) }.boxed())
        .unwrap();
    let fresh = control.agent("agent", None, async { Ok(json!("fresh")) }.boxed());
    finish.send(()).unwrap();
    running.await.unwrap();
    for result in [stale.await, taken.await, human.await] {
        assert!(matches!(result, Err(Error::Control(Reason::Interrupted))));
    }
    released.await.unwrap();
    assert_eq!(fresh.await.unwrap(), json!("fresh"));
}

#[tokio::test]
async fn tracked_navigation_returns_early_but_holds_later_actions() {
    let control = SessionControl::new(None);
    let (finish, done) = oneshot::channel();
    let events = Arc::new(Mutex::new(Vec::new()));
    let inner = control.clone();
    let log = events.clone();
    let first = control.agent(
        "any",
        None,
        async move {
            inner.track(
                async move {
                    let _ = done.await;
                    log.lock().unwrap().push("committed");
                }
                .boxed(),
            );
            Ok(json!("started"))
        }
        .boxed(),
    );
    let log = events.clone();
    let next = control.agent(
        "another",
        None,
        async move {
            log.lock().unwrap().push("next");
            Ok(Value::Null)
        }
        .boxed(),
    );
    assert_eq!(first.await.unwrap(), json!("started"));
    assert!(events.lock().unwrap().is_empty());
    finish.send(()).unwrap();
    next.await.unwrap();
    assert_eq!(*events.lock().unwrap(), vec!["committed", "next"]);
}

#[tokio::test(start_paused = true)]
async fn expired_queued_action_is_refused_without_running_it() {
    let control = SessionControl::new(None);
    let (finish, done) = oneshot::channel();
    let (started, start) = oneshot::channel();
    let running = control.agent(
        "a",
        None,
        async move {
            started.send(()).unwrap();
            let _ = done.await;
            Ok(Value::Null)
        }
        .boxed(),
    );
    start.await.unwrap();
    let expired = control.agent(
        "a",
        Some(tokio::time::Instant::now() + std::time::Duration::from_secs(1)),
        async { panic!("expired action ran") }.boxed(),
    );
    tokio::time::advance(std::time::Duration::from_secs(2)).await;
    finish.send(()).unwrap();
    running.await.unwrap();
    assert!(matches!(expired.await, Err(Error::Timeout(_))));
}

#[tokio::test]
async fn close_preserves_running_result_and_refuses_all_queued_and_new_work() {
    let control = SessionControl::new(None);
    let (finish, done) = oneshot::channel();
    let (started, start) = oneshot::channel();
    let running = control.agent(
        "a",
        None,
        async move {
            started.send(()).unwrap();
            let _ = done.await;
            Ok(json!(42))
        }
        .boxed(),
    );
    start.await.unwrap();
    let stale = control.agent("a", None, async { panic!("closed action ran") }.boxed());
    control.close_now();
    assert!(control.take("v").is_err());
    finish.send(()).unwrap();
    assert_eq!(running.await.unwrap(), json!(42));
    assert!(matches!(stale.await, Err(Error::Control(Reason::Closed))));
    control.close().await;
    control.close().await;
    assert_eq!(control.generation(), 1);
}

#[tokio::test]
async fn overlapping_agent_turns_emit_only_zero_to_one_transitions() {
    let activity = PreviewAgentActivity::default();
    let mut events = activity.subscribe();
    let first = activity.begin("thread".into());
    let second = activity.begin("thread".into());
    assert!(events.recv().await.unwrap().active);
    drop(first);
    assert!(activity.active("thread"));
    assert!(events.try_recv().is_err());
    drop(second);
    assert!(!events.recv().await.unwrap().active);
    assert!(!activity.active("thread"));
    assert!(events.try_recv().is_err());
}

#[tokio::test]
async fn release_waits_for_work_registered_by_the_currently_running_human_action() {
    let control = SessionControl::new(Some("agent".into()));
    control.take("viewer").unwrap().await.unwrap();
    let (start_tx, start) = oneshot::channel();
    let (continue_tx, continued) = oneshot::channel();
    let (finish_tx, finished) = oneshot::channel();
    let inner = control.clone();
    let log = Arc::new(Mutex::new(Vec::new()));
    let action_log = log.clone();
    let running = control.human(
        "viewer",
        async move {
            start_tx.send(()).unwrap();
            continued.await.unwrap();
            inner.track_until_handoff(
                async move {
                    finished.await.unwrap();
                    action_log.lock().unwrap().push("navigation");
                }
                .boxed(),
            );
            Ok(Value::Null)
        }
        .boxed(),
    );
    start.await.unwrap();
    let release_log = log.clone();
    let released = control
        .release(
            "viewer",
            async move {
                release_log.lock().unwrap().push("released");
                Ok(Value::Null)
            }
            .boxed(),
        )
        .unwrap();
    continue_tx.send(()).unwrap();
    running.await.unwrap();
    assert!(log.lock().unwrap().is_empty());
    finish_tx.send(()).unwrap();
    released.await.unwrap();
    assert_eq!(*log.lock().unwrap(), vec!["navigation", "released"]);
}
