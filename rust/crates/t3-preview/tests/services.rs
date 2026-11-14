#![allow(clippy::unwrap_used)]
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use t3_contracts::{preview::*, preview_automation::*};
use t3_preview::{
    Error, PreviewService, Result,
    browser::{PreviewOptions, ViewerOptions},
    driver::{
        BrowserCommand, BrowserDriver, BrowserEvent, PlaywrightDriver, PlaywrightOptions, TabKey,
    },
    html::*,
    profiles::Profiles,
};
use tokio::sync::{Notify, Semaphore, mpsc};

struct Fake {
    generation: std::sync::atomic::AtomicU64,
    events: Mutex<Vec<mpsc::UnboundedSender<BrowserEvent>>>,
    commands: Mutex<Vec<Value>>,
    pages: Mutex<BTreeMap<TabKey, Value>>,
    started: mpsc::UnboundedSender<String>,
    open_gate: Arc<Semaphore>,
    block_open: AtomicBool,
    fail_open: AtomicBool,
    eval_gate: Arc<Semaphore>,
    block_eval: AtomicBool,
    measuring: Notify,
}
impl Fake {
    fn new() -> (Arc<Self>, mpsc::UnboundedReceiver<String>) {
        let (started, rx) = mpsc::unbounded_channel();
        (
            Arc::new(Self {
                generation: std::sync::atomic::AtomicU64::new(0),
                events: Mutex::default(),
                commands: Mutex::default(),
                pages: Mutex::default(),
                started,
                open_gate: Arc::new(Semaphore::new(0)),
                block_open: AtomicBool::new(false),
                fail_open: AtomicBool::new(false),
                eval_gate: Arc::new(Semaphore::new(0)),
                block_eval: AtomicBool::new(false),
                measuring: Notify::new(),
            }),
            rx,
        )
    }
}
#[async_trait]
impl BrowserDriver for Fake {
    fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }
    async fn call(&self, command: BrowserCommand, _: Duration) -> Result<Value> {
        self.commands
            .lock()
            .unwrap()
            .push(serde_json::to_value(&command).unwrap());
        match command {
            BrowserCommand::DiscardPopup { .. } => {
                let _ = self.started.send("discardPopup".into());
                Ok(Value::Null)
            }
            BrowserCommand::Open { snapshot, .. } => {
                let _ = self.started.send("open".into());
                if self.block_open.load(Ordering::SeqCst) {
                    self.open_gate.acquire().await.unwrap().forget();
                }
                if self.fail_open.swap(false, Ordering::SeqCst) {
                    return Err(Error::operation("LaunchError", "fake launch failure"));
                }
                let value = json!({"_workerGeneration":self.generation.load(Ordering::SeqCst),"url":null,"title":null,"loading":false,"dialog":null,"fileChooser":null,"downloads":[],"viewport":{"width":1280,"height":800}});
                self.pages
                    .lock()
                    .unwrap()
                    .insert(TabKey::of(&snapshot), value.clone());
                Ok(value)
            }
            BrowserCommand::Operate {
                key,
                operation,
                input,
                ..
            } => {
                if operation == "status" {
                    return Ok(self
                        .pages
                        .lock()
                        .unwrap()
                        .get(&key)
                        .cloned()
                        .unwrap_or(json!({})));
                }
                if operation == "evaluate" {
                    let _ = self.started.send("evaluate".into());
                    if self.block_eval.load(Ordering::SeqCst) {
                        self.eval_gate.acquire().await.unwrap().forget();
                    }
                    return Ok(json!(42));
                }
                if operation == "navigate" {
                    let mut pages = self.pages.lock().unwrap();
                    let page = pages.get_mut(&key).ok_or(Error::TabNotFound)?;
                    page["url"] = input["url"].clone();
                    return Ok(page.clone());
                }
                Ok(json!({}))
            }
            BrowserCommand::Close { key, .. } => {
                self.pages.lock().unwrap().remove(&key);
                Ok(Value::Null)
            }
            BrowserCommand::HtmlMeasure { .. } => {
                self.measuring.notify_one();
                std::future::pending().await
            }
            _ => Ok(json!({})),
        }
    }
    fn subscribe(&self) -> mpsc::UnboundedReceiver<BrowserEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.events.lock().unwrap().push(tx);
        rx
    }
}
fn request(
    agent: &str,
    operation: PreviewAutomationOperation,
    tab: Option<&str>,
    input: Value,
) -> PreviewAutomationRequest {
    PreviewAutomationRequest {
        request_id: "request".into(),
        thread_id: "thread".into(),
        tab_id: tab.map(|id| Some(id.into())),
        tab_id_explicit: Some(Some(tab.is_some())),
        agent_session_id: Some(Some(agent.into())),
        operation,
        input,
        timeout_ms: 15000,
    }
}
async fn make() -> (
    tempfile::TempDir,
    PreviewService,
    Arc<Fake>,
    mpsc::UnboundedReceiver<String>,
) {
    let root = tempfile::tempdir().unwrap();
    let (driver, started) = Fake::new();
    let service = PreviewService::new(
        driver.clone(),
        PreviewOptions::new(root.path().to_path_buf()),
    )
    .await;
    (root, service, driver, started)
}
async fn open(service: &PreviewService, agent: &str) -> String {
    service
        .execute(request(
            agent,
            PreviewAutomationOperation::Open,
            None,
            json!({"reuseExistingTab":false,"show":false}),
        ))
        .await
        .unwrap()["tabId"]
        .as_str()
        .unwrap()
        .into()
}

#[tokio::test]
async fn agent_ownership_default_target_and_explicit_ambiguity_are_preserved() {
    let (_root, service, driver, _) = make().await;
    let first = open(&service, "a").await;
    let second = open(&service, "b").await;
    let own = service
        .execute(request(
            "a",
            PreviewAutomationOperation::Status,
            None,
            json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(own["tabId"], first);
    assert_eq!(own["tabs"].as_array().unwrap().len(), 2);
    assert_eq!(own["control"]["ownedByCaller"], true);
    let error = service
        .execute(request(
            "b",
            PreviewAutomationOperation::Navigate,
            Some(&first),
            json!({"url":"http://localhost:5173/foreign"}),
        ))
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        Error::Control(PreviewAutomationControlReason::AgentMismatch)
    ));
    assert_eq!(
        driver.pages.lock().unwrap()[&TabKey::new("thread", &first)]["url"],
        Value::Null
    );
    open(&service, "a").await;
    assert!(matches!(
        service
            .execute(request(
                "a",
                PreviewAutomationOperation::Evaluate,
                None,
                json!({"expression":"1"})
            ))
            .await,
        Err(Error::Control(PreviewAutomationControlReason::TabRequired))
    ));
    assert_eq!(
        service
            .execute(request(
                "a",
                PreviewAutomationOperation::Evaluate,
                Some(&first),
                json!({"expression":"1"})
            ))
            .await
            .unwrap(),
        json!(42)
    );
    assert_ne!(first, second);
}

#[tokio::test]
async fn pending_opens_reserve_all_eight_agent_slots_before_launch() {
    let (_root, service, driver, mut started) = make().await;
    driver.block_open.store(true, Ordering::SeqCst);
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let service = service.clone();
        let notify = driver.started.clone();
        tasks.push(tokio::spawn(async move {
            let result = service
                .execute(request(
                    "a",
                    PreviewAutomationOperation::Open,
                    None,
                    json!({"reuseExistingTab":false,"show":false}),
                ))
                .await;
            if result.is_err() {
                notify.send("rejected".into()).unwrap();
            }
            result
        }));
    }
    let mut opens = 0;
    for _ in 0..8 {
        if started.recv().await.unwrap() == "open" {
            opens += 1;
        }
    }
    driver.open_gate.add_permits(8);
    for task in tasks {
        assert!(task.await.unwrap().is_ok());
    }
    assert_eq!(opens, 8);
    assert!(matches!(
        service
            .execute(request(
                "a",
                PreviewAutomationOperation::Open,
                None,
                json!({"reuseExistingTab":false,"show":false})
            ))
            .await,
        Err(Error::Control(PreviewAutomationControlReason::TabLimit))
    ));
    assert_eq!(
        service
            .manager()
            .list(PreviewListInput {
                thread_id: "thread".into()
            })
            .await
            .sessions
            .len(),
        8
    );
}

#[tokio::test]
async fn failed_launch_and_close_release_budget_and_never_leave_relaunchable_sessions() {
    let (_root, service, driver, _) = make().await;
    for _ in 0..7 {
        open(&service, "a").await;
    }
    driver.fail_open.store(true, Ordering::SeqCst);
    assert!(
        service
            .execute(request(
                "a",
                PreviewAutomationOperation::Open,
                None,
                json!({"reuseExistingTab":false,"show":false})
            ))
            .await
            .is_err()
    );
    assert_eq!(
        service
            .manager()
            .list(PreviewListInput {
                thread_id: "thread".into()
            })
            .await
            .sessions
            .len(),
        7
    );
    let last = open(&service, "a").await;
    service
        .execute(request(
            "a",
            PreviewAutomationOperation::Close,
            Some(&last),
            json!({}),
        ))
        .await
        .unwrap();
    assert!(
        driver
            .pages
            .lock()
            .unwrap()
            .get(&TabKey::new("thread", &last))
            .is_none()
    );
    open(&service, "a").await;
}

#[tokio::test]
async fn explicit_desktop_targets_fail_without_mutating_or_opening_server_state() {
    let (_root, service, driver, _) = make().await;
    let desktop = service
        .manager()
        .open(
            serde_json::from_value(
                json!({"threadId":"thread","runtime":"desktop","url":"https://example.com"}),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let before = service
        .manager()
        .list(PreviewListInput {
            thread_id: "thread".into(),
        })
        .await;
    assert!(
        service
            .execute(request(
                "a",
                PreviewAutomationOperation::Open,
                Some(desktop.tab_id.as_str()),
                json!({"url":"https://attacker.test"})
            ))
            .await
            .unwrap_err()
            .to_string()
            .contains("desktop runtime")
    );
    assert_eq!(
        service
            .manager()
            .list(PreviewListInput {
                thread_id: "thread".into()
            })
            .await,
        before
    );
    assert!(driver.pages.lock().unwrap().is_empty());
}

#[tokio::test]
async fn read_only_viewers_cannot_take_control_type_resize_or_navigate() {
    let (_root, service, driver, _) = make().await;
    let id = open(&service, "a").await;
    let viewer = service
        .attach_viewer(
            TabKey::new("thread", &id),
            ViewerOptions {
                can_operate: false,
                max_width: 1280,
                max_height: 800,
                quality: 70,
            },
        )
        .await
        .unwrap();
    for input in [
        json!({"type":"takeControl"}),
        json!({"type":"text","text":"secret"}),
        json!({"type":"resize","width":390,"height":844}),
        json!({"type":"navigate","url":"http://attacker.test"}),
        json!({"type":"stopLoading"}),
    ] {
        viewer.input(input).await.unwrap();
    }
    let status = service
        .execute(request(
            "a",
            PreviewAutomationOperation::Status,
            Some(&id),
            json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(status["control"]["owner"], "agent");
    assert_eq!(
        driver.pages.lock().unwrap()[&TabKey::new("thread", &id)]["url"],
        Value::Null
    );
    assert!(
        !driver
            .commands
            .lock()
            .unwrap()
            .iter()
            .any(|command| command["command"] == "viewerInput")
    );
}

#[tokio::test]
async fn takeover_reserves_immediately_but_waits_for_the_running_agent() {
    let (_root, service, driver, mut started) = make().await;
    let id = open(&service, "a").await;
    assert_eq!(started.recv().await.unwrap(), "open");
    let viewer = Arc::new(
        service
            .attach_viewer(
                TabKey::new("thread", &id),
                ViewerOptions {
                    can_operate: true,
                    max_width: 1280,
                    max_height: 800,
                    quality: 70,
                },
            )
            .await
            .unwrap(),
    );
    driver.block_eval.store(true, Ordering::SeqCst);
    let service_owned = service.clone();
    let id_owned = id.clone();
    let evaluating = tokio::spawn(async move {
        service_owned
            .execute(request(
                "a",
                PreviewAutomationOperation::Evaluate,
                Some(&id_owned),
                json!({"expression":"pending()"}),
            ))
            .await
    });
    assert_eq!(started.recv().await.unwrap(), "evaluate");
    let viewer_owned = viewer.clone();
    let taking =
        tokio::spawn(async move { viewer_owned.input(json!({"type":"takeControl"})).await });
    loop {
        let Some(t3_preview::stream::ViewerOutput::Message(value)) = viewer.output.recv().await
        else {
            continue;
        };
        if value["controller"] == "you" {
            break;
        }
    }
    let blocked = service
        .execute(request(
            "a",
            PreviewAutomationOperation::Evaluate,
            Some(&id),
            json!({"expression":"racing()"}),
        ))
        .await
        .unwrap_err();
    assert!(matches!(
        blocked,
        Error::Control(PreviewAutomationControlReason::HumanControl)
    ));
    driver.eval_gate.add_permits(1);
    assert_eq!(evaluating.await.unwrap().unwrap(), json!(42));
    taking.await.unwrap().unwrap();
    viewer
        .input(json!({"type":"releaseControl"}))
        .await
        .unwrap();
    driver.block_eval.store(false, Ordering::SeqCst);
    assert_eq!(
        service
            .execute(request(
                "a",
                PreviewAutomationOperation::Evaluate,
                Some(&id),
                json!({"expression":"resumed()"})
            ))
            .await
            .unwrap(),
        json!(42)
    );
}

#[tokio::test(start_paused = true)]
async fn idle_agent_tabs_close_but_viewed_tabs_keep_their_slots() {
    let (_root, service, _driver, _) = make().await;
    let first = open(&service, "a").await;
    let viewed = open(&service, "a").await;
    let _viewer = service
        .attach_viewer(
            TabKey::new("thread", &viewed),
            ViewerOptions {
                can_operate: false,
                max_width: 1280,
                max_height: 800,
                quality: 70,
            },
        )
        .await
        .unwrap();
    tokio::time::advance(Duration::from_secs(31 * 60)).await;
    service.sweep_idle().await;
    let listed = service
        .manager()
        .list(PreviewListInput {
            thread_id: "thread".into(),
        })
        .await;
    assert!(!listed.sessions.iter().any(|s| s.tab_id.as_str() == first));
    assert!(listed.sessions.iter().any(|s| s.tab_id.as_str() == viewed));
}

#[tokio::test]
async fn profile_union_is_uncapped_on_disk_and_disconnect_does_not_shrink_saved_catalog() {
    let root = tempfile::tempdir().unwrap();
    let profiles = Profiles::load(root.path()).await;
    let a = profiles.reporter();
    let b = profiles.reporter();
    let rows = |prefix: &str| {
        (0..13).map(|i|serde_json::from_value(json!({"id":format!("{prefix}-{i}"),"name":format!("{prefix}{i}"),"kind":"persistent"})).unwrap()).collect::<Vec<_>>()
    };
    let saved = root.path().join("server-browser/reported-profiles.json");
    std::fs::create_dir_all(&saved).unwrap();
    let report = PreviewReportProfilesInput {
        profiles: rows("a"),
        default_profile_id: "a-0".into(),
        recording_frame_rate: Some(Some(60)),
    };
    a.report(report.clone()).await.unwrap();
    assert_eq!(profiles.catalog().default_profile_id, "a-0");
    assert!(saved.is_dir());
    std::fs::remove_dir(&saved).unwrap();
    a.report(report).await.unwrap();
    b.report(PreviewReportProfilesInput {
        profiles: rows("b"),
        default_profile_id: "b-0".into(),
        recording_frame_rate: None,
    })
    .await
    .unwrap();
    assert_eq!(profiles.catalog().profiles.len(), 28);
    assert_eq!(profiles.frame_rate(), 60);
    drop(a);
    assert_eq!(profiles.catalog().profiles.len(), 15);
    assert!(profiles.resolve_open(Some("a-0")).is_err());
    drop(b);
    let restarted = Profiles::load(root.path()).await;
    assert_eq!(restarted.catalog().profiles.len(), 28);
    assert_eq!(
        restarted.resolve_open(Some("A0")).unwrap(),
        Some("a-0".into())
    );
    assert_eq!(restarted.resolve_open(None).unwrap(), Some("b-0".into()));
}

#[tokio::test]
async fn builtins_cannot_be_overridden_or_custom_profiles_made_ephemeral() {
    let root = tempfile::tempdir().unwrap();
    let profiles = Profiles::load(root.path()).await;
    let reporter = profiles.reporter();
    reporter.report(serde_json::from_value(json!({"profiles":[{"id":"default","name":"Fake","kind":"incognito"},{"id":"custom","name":"Custom","kind":"incognito"}],"defaultProfileId":"missing"})).unwrap()).await.unwrap();
    assert_eq!(profiles.catalog().profiles[0].name, "Default");
    assert_eq!(
        profiles.catalog().profiles[2].kind,
        t3_contracts::browser_profile::BrowserProfileKind::Persistent
    );
    assert_eq!(profiles.catalog().default_profile_id, "default");
    assert!(reporter.report(serde_json::from_value(json!({"profiles":[{"id":"..\u{0000}","name":"Bad","kind":"persistent"}],"defaultProfileId":"default"})).unwrap()).await.is_err());
}

#[tokio::test]
async fn interrupted_publication_removes_attachment_and_recovery() {
    let root = tempfile::tempdir().unwrap();
    let (driver, _) = Fake::new();
    let recovery = Arc::new(MemoryHtmlRecovery::default());
    let attachments = root.path().join("attachments");
    let service = t3_preview::HtmlService::new(
        HtmlOptions {
            attachments_dir: attachments.clone(),
            state_dir: root.path().join("state"),
            secrets_dir: root.path().join("secrets"),
            recovery: recovery.clone(),
        },
        Some(driver.clone()),
    );
    let input = PublishedSource {
        source: HtmlSource::Html {
            html: "<p>cancel</p>".into(),
        },
        title: "Cancel".into(),
        height: 200.0,
    };
    let original = input.clone();
    let publishing = tokio::spawn(async move { service.publish("thread".into(), input).await });
    driver.measuring.notified().await;
    assert_eq!(std::fs::read_dir(&attachments).unwrap().count(), 1);
    publishing.abort();
    assert!(publishing.await.unwrap_err().is_cancelled());
    assert_eq!(std::fs::read_dir(&attachments).unwrap().count(), 0);
    assert!(recovery.take(&"thread".into(), &original).is_none());
}

#[tokio::test]
async fn owned_node_driver_reports_missing_tabs_and_exits_without_launching_a_browser() {
    let root = tempfile::tempdir().unwrap();
    let driver = PlaywrightDriver::start(PlaywrightOptions {
        node: PathBuf::from("node"),
        cwd: PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        executable: root.path().join("browser-must-never-start"),
        profiles_dir: root.path().join("profiles"),
        downloads_dir: root.path().join("downloads"),
        attachments_dir: root.path().join("attachments"),
        playwright_module: None,
        sandbox: true,
    })
    .await
    .unwrap();
    let error = driver
        .call(
            BrowserCommand::Operate {
                key: TabKey::new("thread", "missing"),
                operation: "evaluate".into(),
                input: json!({"expression":"1"}),
                timeout_ms: 1000,
                generation: 0,
            },
            Duration::from_secs(5),
        )
        .await
        .unwrap_err();
    assert_eq!(error.tag(), "PreviewAutomationTabNotFoundError");
    driver
        .call(BrowserCommand::Shutdown, Duration::from_secs(5))
        .await
        .unwrap();
    assert!(!root.path().join("profiles").exists());
}

#[tokio::test]
async fn popup_adoption_preserves_ownership_opener_and_one_browser_page() {
    let (_root, service, driver, _) = make().await;
    let opener = open(&service, "agent").await;
    let mut events = service.manager().subscribe_events();
    for tx in driver.events.lock().unwrap().iter() {
        tx.send(BrowserEvent::Popup {
            opener: TabKey::new("thread", &opener),
            popup_id: "popup-1".into(),
            url: Some("https://example.com/login".into()),
        })
        .unwrap();
    }
    let popup = loop {
        if let PreviewEvent::Opened { snapshot, .. } = events.recv().await.unwrap() {
            break snapshot;
        }
    };
    assert_eq!(popup.automation_owner, Some(Some("agent".into())));
    let _viewer = service
        .attach_viewer(
            TabKey::of(&popup),
            ViewerOptions {
                can_operate: false,
                max_width: 1280,
                max_height: 800,
                quality: 70,
            },
        )
        .await
        .unwrap();
    let status = service
        .execute(request(
            "agent",
            PreviewAutomationOperation::Status,
            Some(popup.tab_id.as_str()),
            json!({}),
        ))
        .await
        .unwrap();
    let entry = status["tabs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["tabId"] == popup.tab_id.as_str())
        .unwrap();
    assert_eq!(entry["openerTabId"], opener);
    let calls = driver.commands.lock().unwrap();
    let opens: Vec<_> = calls.iter().filter(|v| v["command"] == "open").collect();
    assert_eq!(opens.len(), 2);
    assert_eq!(opens[1]["popupId"], "popup-1");
}

#[tokio::test]
async fn over_limit_popup_is_closed_without_publishing_a_session() {
    let (_root, service, driver, mut started) = make().await;
    let opener = open(&service, "agent").await;
    for _ in 1..8 {
        open(&service, "agent").await;
    }
    for _ in 0..8 {
        assert_eq!(started.recv().await.unwrap(), "open");
    }
    let mut events = service.manager().subscribe_events();
    for tx in driver.events.lock().unwrap().iter() {
        tx.send(BrowserEvent::Popup {
            opener: TabKey::new("thread", &opener),
            popup_id: "refused".into(),
            url: None,
        })
        .unwrap();
    }
    assert_eq!(started.recv().await.unwrap(), "discardPopup");
    assert!(events.try_recv().is_err());
    assert_eq!(driver.pages.lock().unwrap().len(), 8);
}

#[tokio::test]
async fn stale_admission_refuses_a_queued_resize_before_changing_session_state() {
    use futures_util::FutureExt;
    let (_root, service, driver, mut started) = make().await;
    let id = open(&service, "agent").await;
    assert_eq!(started.recv().await.unwrap(), "open");
    driver.block_eval.store(true, Ordering::SeqCst);
    let owned = service.clone();
    let tab = id.clone();
    let running = tokio::spawn(async move {
        owned
            .execute(request(
                "agent",
                PreviewAutomationOperation::Evaluate,
                Some(&tab),
                json!({"expression":"hold"}),
            ))
            .await
    });
    assert_eq!(started.recv().await.unwrap(), "evaluate");
    let permitted = Arc::new(AtomicBool::new(true));
    let checked = Arc::new(Notify::new());
    let admission: t3_preview::Admission = {
        let permitted = permitted.clone();
        let checked = checked.clone();
        Arc::new(move || {
            let permitted = permitted.clone();
            let checked = checked.clone();
            async move {
                checked.notify_one();
                if permitted.load(Ordering::SeqCst) {
                    Ok(())
                } else {
                    Err(Error::InvalidInput("revoked".into()))
                }
            }
            .boxed()
        })
    };
    let before = service
        .manager()
        .list(PreviewListInput {
            thread_id: "thread".into(),
        })
        .await;
    let owned = service.clone();
    let tab = id.clone();
    let queued = tokio::spawn(async move {
        owned
            .execute_admitted(
                request(
                    "agent",
                    PreviewAutomationOperation::Resize,
                    Some(&tab),
                    json!({"mode":"freeform","width":390,"height":844}),
                ),
                admission,
            )
            .await
    });
    checked.notified().await;
    permitted.store(false, Ordering::SeqCst);
    driver.eval_gate.add_permits(1);
    running.await.unwrap().unwrap();
    assert!(
        queued
            .await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("revoked")
    );
    assert_eq!(
        service
            .manager()
            .list(PreviewListInput {
                thread_id: "thread".into()
            })
            .await,
        before
    );
    assert!(
        !driver
            .commands
            .lock()
            .unwrap()
            .iter()
            .any(|v| v["operation"] == "resize")
    );
}

#[tokio::test]
async fn html_admission_is_checked_after_preparation_before_any_attachment_write() {
    let root = tempfile::tempdir().unwrap();
    let attachments = root.path().join("attachments");
    let service = t3_preview::HtmlService::new(
        HtmlOptions {
            attachments_dir: attachments.clone(),
            state_dir: root.path().join("state"),
            secrets_dir: root.path().join("secrets"),
            recovery: Arc::new(MemoryHtmlRecovery::default()),
        },
        None,
    );
    let error = service
        .publish_admitted(
            "thread".into(),
            PublishedSource {
                source: HtmlSource::Html {
                    html: "<p>hello</p>".into(),
                },
                title: "hello".into(),
                height: 200.0,
            },
            Arc::new(|| Box::pin(async { Err(Error::InvalidInput("revoked".into())) })),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("revoked"));
    assert_eq!(std::fs::read_dir(attachments).unwrap().count(), 0);
}

#[tokio::test]
async fn viewer_viewport_refuses_malformed_oversized_and_excess_area_before_the_driver() {
    let (_root, service, driver, _) = make().await;
    let id = open(&service, "agent").await;
    let viewer = service
        .attach_viewer(
            TabKey::new("thread", &id),
            ViewerOptions {
                can_operate: true,
                max_width: 1280,
                max_height: 800,
                quality: 70,
            },
        )
        .await
        .unwrap();
    viewer.input(json!({"type":"takeControl"})).await.unwrap();
    let before = service
        .manager()
        .list(PreviewListInput {
            thread_id: "thread".into(),
        })
        .await;
    for input in [
        json!({"type":"viewport"}),
        json!({"type":"viewport","setting":null}),
        json!({"type":"viewport","setting":{"_tag":"unknown"}}),
        json!({"type":"viewport","setting":{"_tag":"freeform","width":100000,"height":100000}}),
        json!({"type":"viewport","setting":{"_tag":"freeform","width":3840,"height":3840}}),
    ] {
        assert!(
            viewer.input(input.clone()).await.is_err(),
            "accepted {input}"
        );
    }
    assert_eq!(
        service
            .manager()
            .list(PreviewListInput {
                thread_id: "thread".into()
            })
            .await,
        before
    );
    assert!(
        !driver
            .commands
            .lock()
            .unwrap()
            .iter()
            .any(|command| command["command"] == "viewerInput"
                && command["input"]["type"] == "viewport")
    );
}

#[tokio::test]
async fn viewer_viewport_updates_the_manager_snapshot_and_resized_event() {
    let (_root, service, _driver, _) = make().await;
    let id = open(&service, "agent").await;
    let viewer = service
        .attach_viewer(
            TabKey::new("thread", &id),
            ViewerOptions {
                can_operate: true,
                max_width: 1280,
                max_height: 800,
                quality: 70,
            },
        )
        .await
        .unwrap();
    viewer.input(json!({"type":"takeControl"})).await.unwrap();
    let mut events = service.manager().subscribe_events();
    let setting = json!({"_tag":"freeform","width":390,"height":844});
    viewer
        .input(json!({"type":"viewport","setting":setting}))
        .await
        .unwrap();
    let listed = service
        .manager()
        .list(PreviewListInput {
            thread_id: "thread".into(),
        })
        .await;
    assert_eq!(
        serde_json::to_value(&listed.sessions[0].viewport).unwrap(),
        setting
    );
    let PreviewEvent::Resized { snapshot, .. } = events.try_recv().unwrap() else {
        panic!("missing resize event")
    };
    assert_eq!(serde_json::to_value(snapshot.viewport).unwrap(), setting);
}

#[tokio::test]
async fn late_disconnect_closes_only_tabs_from_the_dead_worker() {
    let (_root, service, driver, _) = make().await;
    driver.generation.store(1, Ordering::SeqCst);
    let old = open(&service, "agent").await;
    driver.generation.store(2, Ordering::SeqCst);
    let fresh = open(&service, "agent").await;
    let viewer = service
        .attach_viewer(
            TabKey::new("thread", &fresh),
            ViewerOptions {
                can_operate: false,
                max_width: 1280,
                max_height: 800,
                quality: 70,
            },
        )
        .await
        .unwrap();
    let marker =
        json!({"type":"alert","message":"processed after stale disconnect","defaultValue":""});
    for sender in driver.events.lock().unwrap().iter() {
        sender
            .send(BrowserEvent::Worker {
                generation: 1,
                message: Box::new(BrowserEvent::Disconnected),
            })
            .unwrap();
        sender
            .send(BrowserEvent::Viewer {
                key: TabKey::new("thread", &fresh),
                viewer_id: None,
                output: json!({"type":"dialogState","dialog":marker}),
            })
            .unwrap();
    }
    loop {
        match viewer.output.recv().await {
            Some(t3_preview::stream::ViewerOutput::Message(message))
                if message["dialog"] == marker =>
            {
                break;
            }
            Some(t3_preview::stream::ViewerOutput::Gone) | None => {
                panic!("stale worker closed the replacement tab")
            }
            _ => {}
        }
    }
    let listed = service
        .manager()
        .list(PreviewListInput {
            thread_id: "thread".into(),
        })
        .await;
    assert!(!listed.sessions.iter().any(|tab| tab.tab_id.as_str() == old));
    assert!(
        listed
            .sessions
            .iter()
            .any(|tab| tab.tab_id.as_str() == fresh)
    );
}

#[tokio::test]
async fn malformed_automation_inputs_are_rejected_before_any_driver_action_or_task_panic() {
    use PreviewAutomationOperation::*;
    let (_root, service, driver, _) = make().await;
    let id = open(&service, "agent").await;
    for operation in [
        Snapshot,
        Evaluate,
        WaitFor,
        Click,
        Hover,
        Type,
        Press,
        Scroll,
        Select,
        Drag,
        Upload,
        RecordingStart,
        RecordingStop,
        Dialog,
        SetColorScheme,
        Resize,
        Navigate,
        Open,
        Status,
        Close,
    ] {
        for input in [Value::Null, json!([]), json!("not an object"), json!(1)] {
            let error = service
                .execute(request("agent", operation, Some(&id), input.clone()))
                .await
                .unwrap_err();
            assert!(
                matches!(error, Error::InvalidInput(_)),
                "{operation:?} {input}: {error:?}"
            );
        }
    }
    for (operation, input) in [
        (
            Scroll,
            json!({"deltaX":"1);globalThis.injected=true;//","deltaY":0}),
        ),
        (Type, json!({"text":7})),
        (Press, json!({"key":"Enter","modifiers":["invalid"]})),
        (Upload, json!({"paths":[["bad"]]})),
        (Select, json!({"values":{}})),
        (Drag, json!({"source":"#from","target":7})),
        (Dialog, json!({"accept":"yes"})),
        (Evaluate, json!({"expression":5})),
    ] {
        assert!(matches!(
            service
                .execute(request("agent", operation, Some(&id), input))
                .await,
            Err(Error::InvalidInput(_))
        ));
    }
    assert!(
        !driver
            .commands
            .lock()
            .unwrap()
            .iter()
            .any(|command| command["command"] == "operate" && command["operation"] != "status")
    );
}

#[tokio::test]
async fn worker_disconnect_during_open_cannot_leave_a_successful_dead_tab() {
    let (_root, service, driver, mut started) = make().await;
    let marker = service
        .manager()
        .open(serde_json::from_value(json!({"threadId":"marker","runtime":"desktop"})).unwrap())
        .await
        .unwrap();
    let mut events = service.manager().subscribe_events();
    driver.generation.store(1, Ordering::SeqCst);
    driver.block_open.store(true, Ordering::SeqCst);
    let opening = {
        let service = service.clone();
        tokio::spawn(async move {
            service
                .execute(request(
                    "agent",
                    PreviewAutomationOperation::Open,
                    None,
                    json!({"reuseExistingTab":false,"show":false}),
                ))
                .await
        })
    };
    assert_eq!(started.recv().await.unwrap(), "open");
    for sender in driver.events.lock().unwrap().iter() {
        sender
            .send(BrowserEvent::Worker {
                generation: 1,
                message: Box::new(BrowserEvent::Disconnected),
            })
            .unwrap();
        sender
            .send(BrowserEvent::Status {
                input: PreviewReportStatusInput {
                    thread_id: "marker".into(),
                    tab_id: marker.tab_id.clone(),
                    nav_status: PreviewNavStatus::Success {
                        url: "https://example.com/processed".into(),
                        title: "marker".into(),
                    },
                    can_go_back: false,
                    can_go_forward: false,
                },
            })
            .unwrap();
    }
    loop {
        if let PreviewEvent::Navigated { thread_id, .. } = events.recv().await.unwrap()
            && thread_id.as_str() == "marker"
        {
            break;
        }
    }
    let before = service
        .manager()
        .list(PreviewListInput {
            thread_id: "thread".into(),
        })
        .await;
    driver.block_open.store(false, Ordering::SeqCst);
    driver.open_gate.add_permits(1);
    let result = opening.await.unwrap();
    assert!(
        before.sessions.is_empty(),
        "dead worker left a pending session"
    );
    assert!(result.is_err(), "open returned a dead page");
    assert!(driver.pages.lock().unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn renderer_crash_reserves_a_blank_replacement_preserves_identity_and_reconnects_viewers() {
    let (_root, service, driver, mut started) = make().await;
    let id = open(&service, "a").await;
    assert_eq!(started.recv().await.unwrap(), "open");
    let peer = open(&service, "b").await;
    assert_eq!(started.recv().await.unwrap(), "open");
    let viewer = service
        .attach_viewer(
            TabKey::new("thread", &id),
            ViewerOptions {
                can_operate: true,
                max_width: 1280,
                max_height: 800,
                quality: 80,
            },
        )
        .await
        .unwrap();
    let key = TabKey::new("thread", &id);
    driver.block_open.store(true, Ordering::SeqCst);
    let event: BrowserEvent = serde_json::from_value(
        json!({"event":"crashed","key":key,"url":"http://localhost:3000/original"}),
    )
    .unwrap();
    for sender in driver.events.lock().unwrap().iter() {
        sender.send(event.clone()).unwrap();
    }
    assert_eq!(started.recv().await.unwrap(), "open");
    let mut unrelated = service.manager().subscribe_events();
    for sender in driver.events.lock().unwrap().iter() {
        sender.send(BrowserEvent::Status {input:serde_json::from_value(json!({"threadId":"thread","tabId":peer,"navStatus":{"_tag":"Success","url":"http://localhost:3000/peer","title":"Peer"},"canGoBack":false,"canGoForward":false})).unwrap()}).unwrap();
    }
    let event = tokio::time::timeout(Duration::from_secs(1), unrelated.recv())
        .await
        .expect("a pending replacement must not block other tabs' events")
        .unwrap();
    assert_eq!(
        serde_json::to_value(event).unwrap()["snapshot"]["tabId"],
        peer
    );
    let listed = service
        .manager()
        .list(serde_json::from_value(json!({"threadId":"thread"})).unwrap())
        .await;
    assert_eq!(listed.sessions.len(), 2);
    let crashed = listed
        .sessions
        .iter()
        .find(|snapshot| snapshot.tab_id.as_str() == id)
        .unwrap();
    assert_eq!(crashed.tab_id.as_str(), id);
    assert_eq!(
        serde_json::to_value(&crashed.nav_status).unwrap()["description"],
        "Page crashed"
    );
    let mut reconnected = false;
    while let Some(output) = viewer.output.recv().await {
        reconnected |= matches!(output, t3_preview::stream::ViewerOutput::Reconnect);
    }
    assert!(reconnected);
    let replacement = driver
        .commands
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find(|command| command["command"] == "open")
        .unwrap()
        .clone();
    assert_eq!(replacement["snapshot"]["navStatus"]["_tag"], "LoadFailed");
    assert_eq!(replacement["crashedUrl"], "http://localhost:3000/original");
    driver.open_gate.add_permits(1);
    let state = service
        .execute(request(
            "a",
            PreviewAutomationOperation::Status,
            Some(&id),
            json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(state["tabId"], id);
    assert_eq!(state["available"], true);
}

#[tokio::test(start_paused = true)]
async fn queued_click_receives_only_its_remaining_request_budget() {
    use futures_util::FutureExt;
    let (_root, service, driver, mut started) = make().await;
    let id = open(&service, "a").await;
    assert_eq!(started.recv().await.unwrap(), "open");
    driver.block_eval.store(true, Ordering::SeqCst);
    let owned = service.clone();
    let tab = id.clone();
    let running = tokio::spawn(async move {
        owned
            .execute(request(
                "a",
                PreviewAutomationOperation::Evaluate,
                Some(&tab),
                json!({"expression":"hold"}),
            ))
            .await
    });
    assert_eq!(started.recv().await.unwrap(), "evaluate");
    let entered = Arc::new(Notify::new());
    let admission: t3_preview::Admission = {
        let entered = entered.clone();
        Arc::new(move || {
            let entered = entered.clone();
            async move {
                entered.notify_one();
                Ok(())
            }
            .boxed()
        })
    };
    let mut click = request(
        "a",
        PreviewAutomationOperation::Click,
        Some(&id),
        json!({"selector":"#save","timeoutMs":100}),
    );
    click.timeout_ms = 100;
    let queued = tokio::spawn(async move { service.execute_admitted(click, admission).await });
    entered.notified().await;
    tokio::time::advance(Duration::from_millis(60)).await;
    driver.eval_gate.add_permits(1);
    running.await.unwrap().unwrap();
    queued.await.unwrap().unwrap();
    let commands = driver.commands.lock().unwrap();
    let click = commands
        .iter()
        .find(|command| command["operation"] == "click")
        .unwrap();
    assert_eq!(click["input"]["timeoutMs"], 40);
    assert_eq!(click["timeoutMs"], 40);
}
