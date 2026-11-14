use crate::{
    Error, Result,
    control::{Action, SessionControl},
    desktop::{DesktopBrowserChannel, DesktopConnection},
    driver::{BrowserCommand, BrowserDriver, BrowserEvent, TabKey, UploadFile},
    lock,
    manager::PreviewManager,
    profiles::{ProfileReporter, Profiles, validate_profile_id},
    stream::{ViewerOutput, ViewerQueue, control_message, parse_driver_output},
    urls::{navigation_url, resolve_viewport},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use futures_util::{
    FutureExt,
    future::{BoxFuture, Shared},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex, Weak},
    time::Duration,
};
use t3_contracts::{base_schemas::ThreadId, preview::*, preview_automation::*};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

const DRIVER_TIMEOUT: Duration = Duration::from_secs(60);
const IDLE: Duration = Duration::from_secs(30 * 60);
type OpenResult = Shared<BoxFuture<'static, Result<()>>>;
struct Execution {
    deadline: Instant,
    read: bool,
    admission: crate::Admission,
}
struct ViewerState {
    can_operate: bool,
    queue: ViewerQueue,
}
struct Tab {
    worker_generation: std::sync::atomic::AtomicU64,
    key: TabKey,
    control: SessionControl,
    created: u64,
    used: Mutex<Instant>,
    snapshot: Mutex<PreviewSessionSnapshot>,
    ready: Mutex<Option<OpenResult>>,
    desktop: Mutex<Option<DesktopConnection>>,
    viewers: Mutex<BTreeMap<String, ViewerState>>,
    cached: Mutex<Value>,
    closed: CancellationToken,
    recovery_url: Option<String>,
    popup: Option<String>,
    opener: Option<String>,
    viewer_attached: tokio::sync::Notify,
}
impl Tab {
    fn driver_generation(&self) -> u64 {
        self.worker_generation
            .load(std::sync::atomic::Ordering::Acquire)
    }
}
struct State {
    tabs: BTreeMap<TabKey, Arc<Tab>>,
    reservations: BTreeMap<String, Option<String>>,
    sequence: u64,
    closed: bool,
}
struct Inner {
    activity: crate::activity::PreviewAgentActivity,
    manager: PreviewManager,
    driver: Arc<dyn BrowserDriver>,
    desktop: DesktopBrowserChannel,
    profiles: Profiles,
    state: Mutex<State>,
    stop: CancellationToken,
}
impl Drop for Inner {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

#[derive(Clone)]
pub struct PreviewService {
    inner: Arc<Inner>,
}
pub struct PreviewOptions {
    pub state_dir: PathBuf,
    pub desktop: DesktopBrowserChannel,
}
impl PreviewOptions {
    pub fn new(state_dir: PathBuf) -> Self {
        Self {
            state_dir,
            desktop: DesktopBrowserChannel::default(),
        }
    }
}

impl PreviewService {
    pub async fn new(driver: Arc<dyn BrowserDriver>, options: PreviewOptions) -> Self {
        let profiles = Profiles::load(&options.state_dir).await;
        let manager = PreviewManager::default();
        let mut events = manager.subscribe_events();
        let mut driver_events = driver.subscribe();
        let mut desktop_events = options.desktop.subscribe();
        let activity = crate::activity::PreviewAgentActivity::default();
        let mut turns = activity.subscribe();
        let inner = Arc::new(Inner {
            activity,
            manager,
            driver,
            desktop: options.desktop,
            profiles,
            state: Mutex::new(State {
                tabs: BTreeMap::new(),
                reservations: BTreeMap::new(),
                sequence: 0,
                closed: false,
            }),
            stop: CancellationToken::new(),
        });
        let desktop = inner.desktop.clone();
        let stop = inner.stop.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {_=stop.cancelled()=>break,change=turns.recv()=>{let Some(change)=change else{break;};desktop.turn(change.thread_id.0.as_str(),change.active).await;}}
            }
        });
        let weak = Arc::downgrade(&inner);
        let stop = inner.stop.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _=stop.cancelled()=>break,
                    event=events.recv()=> {let Some(event)=event else {break;};let Some(inner)=weak.upgrade() else {break;};Self {inner}.mirror(event).await;},
                }
            }
        });
        let weak = Arc::downgrade(&inner);
        let stop = inner.stop.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {_=stop.cancelled()=>break,event=driver_events.recv()=>{let Some(event)=event else {break;};let Some(inner)=weak.upgrade()else{break;};Self {inner}.driver_event(event).await;}}
            }
        });
        let weak = Arc::downgrade(&inner);
        let stop = inner.stop.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {_=stop.cancelled()=>break,event=desktop_events.recv()=>{let Some(event)=event else{break;};let Some(inner)=weak.upgrade()else{break;};let service=Self {inner};if !event.attached {service.drop_tab(&event.key,false).await;}else {service.reconnect(&event.key).await;}}}
            }
        });
        let weak = Arc::downgrade(&inner);
        let stop = inner.stop.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            interval.tick().await;
            loop {
                tokio::select! {_=stop.cancelled()=>break,_=interval.tick()=>{let Some(inner)=weak.upgrade() else {break;};Self {inner}.sweep_idle().await;}}
            }
        });
        Self { inner }
    }
    pub fn manager(&self) -> &PreviewManager {
        &self.inner.manager
    }
    pub fn activity(&self) -> &crate::activity::PreviewAgentActivity {
        &self.inner.activity
    }
    pub fn profile_reporter(&self) -> ProfileReporter {
        self.inner.profiles.reporter()
    }
    pub fn profiles(&self) -> &Profiles {
        &self.inner.profiles
    }
    pub fn driver(&self) -> &Arc<dyn BrowserDriver> {
        &self.inner.driver
    }

    fn get(&self, key: &TabKey) -> Option<Arc<Tab>> {
        lock(&self.inner.state).tabs.get(key).cloned()
    }
    fn visible(&self, tab: &Tab) -> bool {
        !lock(&tab.viewers).is_empty()
            || lock(&tab.desktop).is_some() && self.inner.desktop.is_shown(&tab.key)
    }
    async fn mirror(&self, event: PreviewEvent) {
        match event {
            PreviewEvent::Opened { snapshot, .. }
                if snapshot.runtime == Some(Some(PreviewRuntime::Server)) =>
            {
                if snapshot
                    .automation_owner
                    .as_ref()
                    .and_then(Option::as_ref)
                    .is_none()
                {
                    let _ = self.ensure(snapshot).await;
                }
            }
            PreviewEvent::Closed {
                thread_id, tab_id, ..
            } => {
                self.drop_tab(&TabKey::new(thread_id.0, tab_id.0), true)
                    .await;
            }
            PreviewEvent::Resized {
                snapshot, request, ..
            } => {
                let key = TabKey::of(&snapshot);
                if let Some(tab) = self.get(&key) {
                    *lock(&tab.snapshot) = snapshot.clone();
                    let driver = self.inner.driver.clone();
                    let worker_generation = tab.driver_generation();
                    let control = tab.control.clone();
                    let request = serde_json::to_value(request.flatten()).unwrap_or(Value::Null);
                    let _ = control
                        .system(
                            async move {
                                driver
                                    .call_on(
                                        worker_generation,
                                        BrowserCommand::Adjust {
                                            snapshot: Box::new(snapshot),
                                            request,
                                        },
                                        DRIVER_TIMEOUT,
                                    )
                                    .await
                            }
                            .boxed(),
                        )
                        .await;
                }
            }
            _ => {}
        }
    }
    async fn driver_event(&self, event: BrowserEvent) {
        let (generation, event) = match event {
            BrowserEvent::Worker {
                generation,
                message,
            } => (Some(generation), *message),
            event => (None, event),
        };
        if !matches!(event, BrowserEvent::Disconnected)
            && generation.is_some_and(|generation| generation != self.inner.driver.generation())
        {
            return;
        }
        match event {
            BrowserEvent::Worker { .. } => {}
            BrowserEvent::Disconnected => {
                let tabs: Vec<_> = lock(&self.inner.state)
                    .tabs
                    .values()
                    .filter(|tab| {
                        generation.is_none_or(|generation| {
                            generation
                                == tab
                                    .worker_generation
                                    .load(std::sync::atomic::Ordering::Acquire)
                        })
                    })
                    .cloned()
                    .collect();
                for tab in tabs {
                    let desktop = lock(&tab.desktop).is_some();
                    self.drop_tab(&tab.key, !desktop).await;
                }
            }
            BrowserEvent::Status { input } => {
                let _ = self.inner.manager.report(input, true).await;
            }
            BrowserEvent::Crashed { key, url } => {
                self.recover_crash(key, url).await;
            }
            BrowserEvent::Closed { key } => {
                self.drop_tab(&key, true).await;
            }
            BrowserEvent::Viewer {
                key,
                viewer_id,
                output,
            } => {
                if let Some(tab) = self.get(&key) {
                    if output["type"] == "pointer"
                        && lock(&tab.desktop).is_some()
                        && let Ok(command) = serde_json::from_value(
                            serde_json::json!({"type":"pointer","threadId":key.thread_id,"tabId":key.tab_id,"phase":output["phase"],"x":output["x"],"y":output["y"]}),
                        )
                    {
                        self.inner.desktop.command(command).await;
                    }
                    if output["type"] == "dialogState" {
                        lock(&tab.cached)["dialog"] = output["dialog"].clone();
                        self.broadcast_control(&tab);
                        return;
                    }
                    if let Ok(output) = parse_driver_output(output) {
                        let viewers = lock(&tab.viewers);
                        for (id, viewer) in viewers
                            .iter()
                            .filter(|(id, _)| viewer_id.as_ref().is_none_or(|target| target == *id))
                        {
                            let dropped = viewer.queue.push(output.clone());
                            for frame_id in dropped {
                                self.ack_frame(
                                    key.clone(),
                                    id.clone(),
                                    frame_id,
                                    tab.driver_generation(),
                                );
                            }
                        }
                    }
                }
            }
            BrowserEvent::Popup {
                opener,
                popup_id,
                url,
            } => {
                let service = self.clone();
                tokio::spawn(async move {
                    let _ = service.adopt_popup(opener, popup_id, url).await;
                });
            }
            BrowserEvent::Diagnostic { message } => tracing::warn!(%message,"preview driver"),
        }
    }
    async fn reconnect(&self, key: &TabKey) {
        if self.get(key).is_some() {
            return;
        }
        let listed = self
            .manager()
            .list(PreviewListInput {
                thread_id: key.thread_id.as_str().into(),
            })
            .await;
        if let Some(snapshot) = listed.sessions.into_iter().find(|s| {
            s.tab_id.as_str() == key.tab_id && s.runtime == Some(Some(PreviewRuntime::Server))
        }) {
            let _ = self.ensure(snapshot).await;
        }
    }
    async fn ensure(&self, snapshot: PreviewSessionSnapshot) -> Result<Arc<Tab>> {
        self.ensure_reserved(snapshot, None).await
    }
    async fn ensure_reserved(
        &self,
        snapshot: PreviewSessionSnapshot,
        reservation: Option<&str>,
    ) -> Result<Arc<Tab>> {
        let tab = self.register(snapshot, reservation, None, None)?;
        self.await_ready(tab).await
    }
    fn register(
        &self,
        snapshot: PreviewSessionSnapshot,
        reservation: Option<&str>,
        popup: Option<String>,
        opener: Option<String>,
    ) -> Result<Arc<Tab>> {
        let key = TabKey::of(&snapshot);
        let tab = {
            let mut state = lock(&self.inner.state);
            if state.closed {
                return Err(Error::Disconnected);
            }
            if let Some(reservation) = reservation {
                state.reservations.remove(reservation);
            }
            if let Some(tab) = state.tabs.get(&key) {
                tab.clone()
            } else {
                state.sequence += 1;
                let tab = Self::new_tab(snapshot, state.sequence, popup, opener, None);
                state.tabs.insert(key, tab.clone());
                tab
            }
        };
        Ok(tab)
    }
    fn new_tab(
        snapshot: PreviewSessionSnapshot,
        created: u64,
        popup: Option<String>,
        opener: Option<String>,
        recovery_url: Option<String>,
    ) -> Arc<Tab> {
        let key = TabKey::of(&snapshot);
        Arc::new(Tab {
            worker_generation: std::sync::atomic::AtomicU64::new(0),
            key: key.clone(),
            control: SessionControl::new(snapshot.automation_owner.clone().flatten()),
            created,
            used: Mutex::new(Instant::now()),
            snapshot: Mutex::new(snapshot),
            ready: Mutex::new(None),
            desktop: Mutex::new(None),
            viewers: Mutex::default(),
            cached: Mutex::new(
                json!({"url":null,"title":null,"loading":false,"dialog":null,"fileChooser":null,"downloads":[]}),
            ),
            closed: CancellationToken::new(),
            recovery_url,
            popup,
            opener,
            viewer_attached: tokio::sync::Notify::new(),
        })
    }
    async fn recover_crash(&self, key: TabKey, url: String) {
        let Some(old) = self.get(&key) else {
            return;
        };
        if lock(&old.desktop).is_some() {
            self.drop_tab(&key, true).await;
            return;
        }
        let crash = PreviewAutomationLoadError {
            url: redact_crashed_url(&url).chars().take(2048).collect(),
            code: -2,
            description: "Page crashed".into(),
        };
        let input = PreviewReportStatusInput {
            thread_id: key.thread_id.as_str().into(),
            tab_id: key.tab_id.as_str().into(),
            nav_status: PreviewNavStatus::LoadFailed {
                url: crash.url.clone().into(),
                title: String::new(),
                code: crash.code,
                description: crash.description.clone(),
                download: None,
            },
            can_go_back: false,
            can_go_forward: false,
        };
        if self.manager().report(input, true).await.is_err() {
            return;
        }
        let Some(mut snapshot) = self
            .manager()
            .list(PreviewListInput {
                thread_id: key.thread_id.as_str().into(),
            })
            .await
            .sessions
            .into_iter()
            .find(|snapshot| {
                snapshot.tab_id.as_str() == key.tab_id
                    && snapshot.runtime == Some(Some(PreviewRuntime::Server))
            })
        else {
            return;
        };
        snapshot.nav_status = PreviewNavStatus::LoadFailed {
            url: crash.url.into(),
            title: String::new(),
            code: -2,
            description: crash.description,
            download: None,
        };
        snapshot.can_go_back = false;
        snapshot.can_go_forward = false;
        let replacement = Self::new_tab(snapshot, old.created, None, old.opener.clone(), Some(url));
        replacement.worker_generation.store(
            old.driver_generation(),
            std::sync::atomic::Ordering::Release,
        );
        let service = self.clone();
        let next = replacement.clone();
        let previous = old.clone();
        let reopening = async move {
            let _ = service
                .inner
                .driver
                .call_on(
                    previous.driver_generation(),
                    BrowserCommand::Close {
                        key: previous.key.clone(),
                        desktop: false,
                        unload: false,
                    },
                    Duration::from_secs(6),
                )
                .await;
            let result = service.create_tab(next.clone()).await;
            if result.is_err()
                && service
                    .get(&next.key)
                    .is_some_and(|current| Arc::ptr_eq(&current, &next))
            {
                service.drop_tab(&next.key, true).await;
            }
            result
        }
        .boxed()
        .shared();
        *lock(&replacement.ready) = Some(reopening.clone());
        {
            let mut state = lock(&self.inner.state);
            if !state
                .tabs
                .get(&key)
                .is_some_and(|current| Arc::ptr_eq(current, &old))
            {
                return;
            }
            state.tabs.insert(key, replacement);
        }
        old.closed.cancel();
        old.control.close_now();
        for viewer in lock(&old.viewers).values() {
            viewer.queue.push(ViewerOutput::Reconnect);
            viewer.queue.close();
        }
        tokio::spawn(async move {
            let _ = reopening.await;
        });
    }
    async fn await_ready(&self, tab: Arc<Tab>) -> Result<Arc<Tab>> {
        let opening = {
            let mut ready = lock(&tab.ready);
            ready
                .get_or_insert_with(|| {
                    let service = self.clone();
                    let tab = tab.clone();
                    let join = tokio::spawn(async move {
                        let result = service.create_tab(tab.clone()).await;
                        if result.is_err()
                            && service
                                .get(&tab.key)
                                .is_some_and(|current| Arc::ptr_eq(&current, &tab))
                        {
                            service.drop_tab(&tab.key, true).await;
                        }
                        result
                    });
                    async move { join.await.map_err(|error| Error::Io(error.to_string()))? }
                        .boxed()
                        .shared()
                })
                .clone()
        };
        opening.await?;
        if tab.closed.is_cancelled() {
            return Err(Error::TabNotFound);
        }
        Ok(tab)
    }
    async fn create_tab(&self, tab: Arc<Tab>) -> Result<()> {
        let snapshot = lock(&tab.snapshot).clone();
        let connection = if tab.popup.is_none()
            && self.inner.desktop.available()
            && snapshot.headless != Some(Some(true))
        {
            let wait = self
                .inner
                .desktop
                .await_attached(&tab.key, Duration::from_secs(10));
            let mut announcements = tokio::time::interval(Duration::from_secs(1));
            tokio::pin!(wait);
            let attached = loop {
                tokio::select! {attached=&mut wait=>break attached,_=announcements.tick()=>{let _=self.manager().announce(&tab.key.thread_id,&tab.key.tab_id).await;},_=tab.closed.cancelled()=>return Err(Error::TabNotFound)}
            };
            if attached {
                Some(self.inner.desktop.endpoint(&tab.key).await?)
            } else {
                None
            }
        } else {
            None
        };
        if connection.is_none() {
            self.manager()
                .mark_headless(&tab.key.thread_id, &tab.key.tab_id)
                .await?;
        }
        let isolated = connection.is_none()
            && (snapshot
                .profile_id
                .as_ref()
                .and_then(Option::as_ref)
                .map(String::as_str)
                == Some("incognito")
                || snapshot
                    .automation_owner
                    .as_ref()
                    .and_then(Option::as_ref)
                    .is_some()
                    && snapshot
                        .profile_id
                        .as_ref()
                        .and_then(Option::as_ref)
                        .is_none());
        let endpoint = connection
            .as_ref()
            .map(|connection| connection.endpoint.url.clone());
        *lock(&tab.desktop) = connection;
        let generation = self.inner.driver.prepare().await?;
        tab.worker_generation
            .store(generation, std::sync::atomic::Ordering::Release);
        if tab.closed.is_cancelled() {
            return Err(Error::TabNotFound);
        }
        let agent = snapshot
            .automation_owner
            .as_ref()
            .and_then(Option::as_ref)
            .is_some();
        let load_error = if let PreviewNavStatus::LoadFailed {
            url,
            code,
            description,
            ..
        } = &snapshot.nav_status
        {
            Some(PreviewAutomationLoadError {
                url: url.to_string(),
                code: *code,
                description: description.clone(),
            })
        } else {
            self.inner.desktop.load_failure(&tab.key)
        };
        let mut result = self
            .inner
            .driver
            .call_on(
                generation,
                BrowserCommand::Open {
                    snapshot: Box::new(snapshot),
                    desktop_endpoint: endpoint,
                    isolated,
                    popup_id: tab.popup.clone(),
                    load_error,
                    crashed_url: tab.recovery_url.clone(),
                },
                DRIVER_TIMEOUT,
            )
            .await;
        if agent
            && result
                .as_ref()
                .is_ok_and(|value| value["initialPending"] == true)
        {
            result = self
                .inner
                .driver
                .call_on(
                    generation,
                    BrowserCommand::Operate {
                        key: tab.key.clone(),
                        operation: "awaitInitialNavigation".into(),
                        input: json!({}),
                        timeout_ms: 16000,
                        generation: tab.control.generation(),
                    },
                    Duration::from_secs(17),
                )
                .await;
        }
        if tab.closed.is_cancelled() {
            if self
                .get(&tab.key)
                .is_some_and(|current| !Arc::ptr_eq(&current, &tab))
            {
                return Err(Error::TabNotFound);
            }
            let desktop = lock(&tab.desktop).is_some();
            let _ = self
                .inner
                .driver
                .call_on(
                    tab.driver_generation(),
                    BrowserCommand::Close {
                        key: tab.key.clone(),
                        desktop,
                        unload: false,
                    },
                    DRIVER_TIMEOUT,
                )
                .await;
            return Err(Error::TabNotFound);
        }
        if let Ok(value) = &result {
            *lock(&tab.cached) = value.clone();
        }
        result.map(|_| ())
    }
    async fn drop_tab(&self, key: &TabKey, close_session: bool) {
        let tab = lock(&self.inner.state).tabs.remove(key);
        if let Some(tab) = tab {
            tab.closed.cancel();
            tab.control.close_now();
            let desktop = lock(&tab.desktop).is_some();
            for viewer in lock(&tab.viewers).values() {
                viewer.queue.push(if desktop && !close_session {
                    ViewerOutput::Reconnect
                } else {
                    ViewerOutput::Gone
                });
                viewer.queue.close();
            }
            let _ = self
                .inner
                .driver
                .call_on(
                    tab.driver_generation(),
                    BrowserCommand::Close {
                        key: key.clone(),
                        desktop,
                        unload: close_session,
                    },
                    Duration::from_secs(6),
                )
                .await;
            lock(&tab.desktop).take();
        }
        if close_session {
            let _ = self
                .manager()
                .close(PreviewCloseInput {
                    thread_id: key.thread_id.as_str().into(),
                    tab_id: Some(Some(key.tab_id.as_str().into())),
                })
                .await;
        }
    }
    pub async fn clear_profile(&self, profile_id: &str) -> Result<()> {
        validate_profile_id(profile_id)?;
        if profile_id == "incognito" {
            return Ok(());
        }
        self.inner
            .driver
            .call(
                BrowserCommand::ClearProfile {
                    profile_id: profile_id.into(),
                },
                DRIVER_TIMEOUT,
            )
            .await?;
        Ok(())
    }
    async fn reserve(&self, agent: Option<&str>) -> Result<String> {
        if let Some(agent) = agent {
            self.sweep_idle().await;
            let reclaim = {
                let state = lock(&self.inner.state);
                let owned = state
                    .tabs
                    .values()
                    .filter(|t| t.control.agent_id() == Some(agent))
                    .count()
                    + state
                        .reservations
                        .values()
                        .filter(|id| id.as_deref() == Some(agent))
                        .count();
                if owned < 8 && state.tabs.len() + state.reservations.len() >= 32 {
                    state
                        .tabs
                        .values()
                        .filter(|t| {
                            t.control.agent_id().is_some()
                                && !self.visible(t)
                                && lock(&t.used).elapsed() > IDLE
                        })
                        .min_by_key(|t| *lock(&t.used))
                        .map(|t| t.key.clone())
                } else {
                    None
                }
            };
            if let Some(key) = reclaim {
                self.drop_tab(&key, true).await;
            }
        }
        let mut state = lock(&self.inner.state);
        if state.closed {
            return Err(Error::Disconnected);
        }
        if let Some(agent) = agent {
            let owned = state
                .tabs
                .values()
                .filter(|t| t.control.agent_id() == Some(agent))
                .count()
                + state
                    .reservations
                    .values()
                    .filter(|id| id.as_deref() == Some(agent))
                    .count();
            if owned >= 8 || state.tabs.len() + state.reservations.len() >= 32 {
                return Err(Error::Control(PreviewAutomationControlReason::TabLimit));
            }
        }
        let reservation = uuid::Uuid::new_v4().to_string();
        state
            .reservations
            .insert(reservation.clone(), agent.map(str::to_owned));
        Ok(reservation)
    }
    async fn open_agent(
        &self,
        thread: &str,
        agent: &str,
        input: PreviewAutomationOpenInput,
        admission: crate::Admission,
    ) -> Result<Arc<Tab>> {
        let profile = self.inner.profiles.resolve_open(
            input
                .profile_id
                .as_ref()
                .and_then(Option::as_ref)
                .map(|s| s.as_str()),
        )?;
        let reservation = self.reserve(Some(agent)).await?;
        if let Err(error) = admission().await {
            lock(&self.inner.state).reservations.remove(&reservation);
            return Err(error);
        }
        let result = self
            .manager()
            .open_owned(
                PreviewOpenInput {
                    thread_id: thread.into(),
                    url: input.url.flatten().map(|s| Some(s.into())),
                    viewport: None,
                    color_scheme: None,
                    zoom_factor: None,
                    profile_id: profile.map(Some),
                    runtime: Some(Some(PreviewRuntime::Server)),
                    reveal: Some(Some(false)),
                },
                Some(agent.into()),
            )
            .await;
        let snapshot = match result {
            Ok(snapshot) => snapshot,
            Err(error) => {
                lock(&self.inner.state).reservations.remove(&reservation);
                return Err(error);
            }
        };
        // Holding the reservation until ensure publishes the tab accounts for pending launches.
        let key = TabKey::of(&snapshot);
        let result = self.ensure_reserved(snapshot, Some(&reservation)).await;
        lock(&self.inner.state).reservations.remove(&reservation);
        if result.is_err()
            && self
                .get(&key)
                .is_none_or(|current| current.recovery_url.is_none())
        {
            self.drop_tab(&key, true).await;
        }
        result
    }
    fn latest(&self, thread: &str, agent: Option<&str>) -> Option<Arc<Tab>> {
        let state = lock(&self.inner.state);
        let tabs: Vec<_> = state
            .tabs
            .values()
            .filter(|t| t.key.thread_id == thread)
            .collect();
        tabs.iter()
            .filter(|t| agent.is_some() && t.control.agent_id() == agent)
            .max_by_key(|t| t.created)
            .or_else(|| {
                tabs.iter()
                    .max_by_key(|t| (self.visible(t), *lock(&t.used), t.created))
            })
            .map(|t| (*t).clone())
    }
    async fn find(
        &self,
        thread: &str,
        tab: Option<&str>,
        agent: &str,
        explicit: bool,
    ) -> Result<Arc<Tab>> {
        if !explicit
            && lock(&self.inner.state)
                .tabs
                .values()
                .filter(|t| t.key.thread_id == thread && t.control.agent_id() == Some(agent))
                .count()
                > 1
        {
            return Err(Error::Control(PreviewAutomationControlReason::TabRequired));
        }
        if let Some(id) = tab {
            let key = TabKey::new(thread, id);
            if let Some(tab) = self.get(&key) {
                return self.await_ready(tab).await;
            }
            let listed = self
                .manager()
                .list(PreviewListInput {
                    thread_id: thread.into(),
                })
                .await;
            let snapshot = listed
                .sessions
                .into_iter()
                .find(|s| {
                    s.tab_id.as_str() == id && s.runtime == Some(Some(PreviewRuntime::Server))
                })
                .ok_or(Error::TabNotFound)?;
            return self.ensure(snapshot).await;
        }
        self.latest(thread, Some(agent)).ok_or(Error::TabNotFound)
    }
    async fn status(
        &self,
        tab: Option<Arc<Tab>>,
        agent: Option<&str>,
    ) -> Result<PreviewAutomationStatus> {
        let catalog = self.inner.profiles.catalog();
        let profiles: Vec<_> = catalog
            .profiles
            .iter()
            .map(|p| json!({"id":p.id,"name":p.name}))
            .enumerate()
            .map(|(i, mut p)| {
                if i == 1 {
                    p["incognito"] = true.into();
                }
                p
            })
            .collect();
        let mut status = json!({"available":false,"visible":false,"tabId":null,"url":null,"title":null,"loading":false,"profiles":profiles});
        if self.inner.profiles.has_report() {
            status["defaultProfileId"] = catalog.default_profile_id.into();
        }
        let Some(tab) = tab else {
            return Ok(serde_json::from_value(status)?);
        };
        let tab = self.await_ready(tab).await?;
        let read = self
            .inner
            .driver
            .call_on(
                tab.driver_generation(),
                BrowserCommand::Operate {
                    key: tab.key.clone(),
                    operation: "status".into(),
                    input: json!({}),
                    timeout_ms: 250,
                    generation: tab.control.generation(),
                },
                Duration::from_millis(500),
            )
            .await;
        if let Ok(value) = read {
            *lock(&tab.cached) = value;
        }
        let page = lock(&tab.cached).clone();
        for key in [
            "url",
            "title",
            "loading",
            "dialog",
            "fileChooser",
            "viewport",
            "downloads",
            "loadError",
        ] {
            if let Some(value) = page.get(key) {
                status[key] = value.clone();
            }
        }
        status["available"] = true.into();
        status["visible"] = self.visible(&tab).into();
        status["tabId"] = tab.key.tab_id.clone().into();
        status["viewportSetting"] = serde_json::to_value(
            lock(&tab.snapshot)
                .viewport
                .clone()
                .flatten()
                .unwrap_or(PreviewViewportSetting::Fill {}),
        )?;
        status["control"] = json!({"owner":owner(&tab),"ownedByCaller":agent.is_some() && tab.control.agent_id()==agent,"generation":tab.control.generation()});
        let tabs:Vec<_>=lock(&self.inner.state).tabs.values().filter(|t|t.key.thread_id==tab.key.thread_id).map(|t| {
            let mut value=json!({"tabId":t.key.tab_id,"url":lock(&t.cached)["url"],"owner":owner(t),"ownedByCaller":agent.is_some() && t.control.agent_id()==agent,"visible":self.visible(t)});
            if let Some(profile)=lock(&t.snapshot).profile_id.clone().flatten() {value["profileId"]=profile.into();}
            if let Some(opener)=&t.opener {value["openerTabId"]=opener.clone().into();}value
        }).collect();
        status["tabs"] = tabs.into();
        Ok(serde_json::from_value(status)?)
    }
    pub async fn execute(&self, request: PreviewAutomationRequest) -> Result<Value> {
        self.execute_admitted(request, Arc::new(|| async { Ok(()) }.boxed()))
            .await
    }
    pub async fn execute_admitted(
        &self,
        mut request: PreviewAutomationRequest,
        admission: crate::Admission,
    ) -> Result<Value> {
        request.input = automation_input(request.operation, request.input)?;
        let service = self.clone();
        // An admitted operation owns its cleanup even if its transport future is dropped.
        tokio::spawn(async move {
            let result = service.execute_inner(&request, admission).await;
            let agent = request.agent_session_id.as_ref().and_then(Option::as_ref);
            for tab in lock(&service.inner.state).tabs.values() {
                if tab.key.thread_id == request.thread_id.0.as_str()
                    && (tab.control.agent_id() == agent.map(String::as_str)
                        || request
                            .tab_id
                            .as_ref()
                            .and_then(Option::as_ref)
                            .is_some_and(|id| id.as_str() == tab.key.tab_id))
                {
                    *lock(&tab.used) = Instant::now();
                }
            }
            result
        })
        .await
        .map_err(|e| Error::Io(e.to_string()))?
    }
    async fn execute_inner(
        &self,
        request: &PreviewAutomationRequest,
        admission: crate::Admission,
    ) -> Result<Value> {
        let thread = request.thread_id.0.as_str();
        let target = request
            .tab_id
            .as_ref()
            .and_then(Option::as_ref)
            .map(|s| s.as_str());
        let agent = request
            .agent_session_id
            .as_ref()
            .and_then(Option::as_ref)
            .map(String::as_str);
        let explicit = request.tab_id_explicit == Some(Some(true));
        let timeout = Duration::from_millis(request.timeout_ms.clamp(1, 120_000) as u64);
        let deadline = Instant::now() + timeout;
        if request.operation == PreviewAutomationOperation::Status {
            let tab = target
                .map(|tab| self.get(&TabKey::new(thread, tab)))
                .unwrap_or_else(|| self.latest(thread, agent));
            return Ok(serde_json::to_value(self.status(tab, agent).await?)?);
        }
        let agent = agent
            .filter(|s| !s.is_empty())
            .ok_or(Error::Control(PreviewAutomationControlReason::Interrupted))?;
        if !matches!(
            request.operation,
            PreviewAutomationOperation::Snapshot | PreviewAutomationOperation::WaitFor
        ) {
            admission().await?;
        }
        if request.operation == PreviewAutomationOperation::Open {
            let mut input: PreviewAutomationOpenInput =
                serde_json::from_value(request.input.clone())?;
            if let Some(url) = input.url.as_ref().and_then(Option::as_ref) {
                crate::urls::normalize_preview_url(url)?;
            }
            let reuse = input.reuse_existing_tab != Some(Some(false));
            if reuse
                && !explicit
                && lock(&self.inner.state)
                    .tabs
                    .values()
                    .filter(|t| t.key.thread_id == thread && t.control.agent_id() == Some(agent))
                    .count()
                    > 1
            {
                return Err(Error::Control(PreviewAutomationControlReason::TabRequired));
            }
            let existing = if reuse {
                match target {
                    Some(id) => self.get(&TabKey::new(thread, id)),
                    None => self
                        .latest(thread, Some(agent))
                        .filter(|t| t.control.agent_id() == Some(agent)),
                }
            } else {
                None
            };
            if reuse && explicit && existing.is_none() {
                let listed = self
                    .manager()
                    .list(PreviewListInput {
                        thread_id: thread.into(),
                    })
                    .await;
                if listed.sessions.iter().any(|s| {
                    Some(s.tab_id.as_str()) == target
                        && s.runtime != Some(Some(PreviewRuntime::Server))
                }) {
                    return Err(Error::operation(
                        "PreviewAutomationExecutionError",
                        "This tab belongs to the desktop runtime. This environment's server browser cannot control desktop tabs.",
                    ));
                }
                return Err(Error::TabNotFound);
            }
            let reveal = input.open.flatten().or(input.show.flatten());
            let tab = if let Some(tab) = existing {
                tab.control.assert_agent(agent)?;
                if !lock(&tab.cached)["dialog"].is_null() {
                    return Err(Error::Control(
                        PreviewAutomationControlReason::DialogPending,
                    ));
                }
                if let Some(url) = input.url.take().flatten() {
                    self.operate(tab.clone(),agent,"navigate",json!({"url":crate::urls::normalize_preview_url(&url)?,"timeoutMs":request.timeout_ms.min(15000)}),Execution { deadline, read:false, admission:admission.clone() }).await?;
                }
                tab
            } else {
                self.open_agent(thread, agent, input, admission.clone())
                    .await?
            };
            tab.control.assert_agent(agent)?;
            if let Some(error) = lock(&tab.cached).get("openError") {
                return Err(Error::operation(
                    "PreviewAutomationExecutionError",
                    error["message"].as_str().unwrap_or("Download is starting"),
                ));
            }
            if reveal != Some(false) {
                self.manager()
                    .request_reveal(thread, &tab.key.tab_id, reveal == Some(true))
                    .await?;
                if reveal == Some(true) && lock(&tab.desktop).is_none() {
                    let attached = tab.viewer_attached.notified();
                    tokio::pin!(attached);
                    attached.as_mut().enable();
                    if lock(&tab.viewers).is_empty() {
                        let wait = Duration::from_secs(1)
                            .min(deadline.saturating_duration_since(Instant::now()));
                        let _ = tokio::time::timeout(wait, async {
                            tokio::select! {_=attached=>{},_=tab.closed.cancelled()=>{}}
                        })
                        .await;
                    }
                }
            }
            return Ok(serde_json::to_value(
                self.status(Some(tab), Some(agent)).await?,
            )?);
        }
        let tab = self.find(thread, target, agent, explicit).await?;
        let read = matches!(
            request.operation,
            PreviewAutomationOperation::Snapshot | PreviewAutomationOperation::WaitFor
        );
        if !read && !tab.control.may_act(agent) {
            return Err(Error::Control(
                PreviewAutomationControlReason::AgentMismatch,
            ));
        }
        if request.operation == PreviewAutomationOperation::Close {
            admission().await?;
            if tab.control.agent_id().is_none() {
                return Err(Error::Control(PreviewAutomationControlReason::UnclaimedTab));
            }
            tab.control.assert_agent(agent)?;
            tab.control.close_now();
            self.drop_tab(&tab.key, true).await;
            return Ok(json!({}));
        }
        if request.operation == PreviewAutomationOperation::Dialog {
            admission().await?;
            tab.control.assert_agent(agent)?;
            self.raw(&tab, "dialog", request.input.clone(), timeout)
                .await?;
            return Ok(serde_json::to_value(
                self.status(Some(tab), Some(agent)).await?,
            )?);
        }
        if !lock(&tab.cached)["dialog"].is_null() {
            return Err(Error::Control(
                PreviewAutomationControlReason::DialogPending,
            ));
        }
        let operation = serde_json::to_value(request.operation)?
            .as_str()
            .ok_or_else(|| Error::InvalidInput("Invalid operation".into()))?
            .to_owned();
        let mut input = request.input.clone();
        if request.operation == PreviewAutomationOperation::Navigate {
            let navigation: PreviewAutomationNavigateInput = serde_json::from_value(input.clone())?;
            input["url"] = navigation_url(&navigation)?.into();
        }
        if request.operation == PreviewAutomationOperation::Resize {
            let setting = resolve_viewport(serde_json::from_value(input.clone())?)?;
            input["setting"] = serde_json::to_value(&setting)?;
        }
        input["frameRate"] = self.inner.profiles.frame_rate().into();
        let result = self
            .operate(
                tab.clone(),
                agent,
                &operation,
                input,
                Execution {
                    deadline,
                    read,
                    admission,
                },
            )
            .await?;
        if request.operation == PreviewAutomationOperation::Navigate {
            Ok(serde_json::to_value(
                self.status(Some(tab), Some(agent)).await?,
            )?)
        } else {
            Ok(result)
        }
    }
    async fn raw(
        &self,
        tab: &Tab,
        operation: &str,
        input: Value,
        timeout: Duration,
    ) -> Result<Value> {
        self.inner
            .driver
            .call_on(
                tab.driver_generation(),
                BrowserCommand::Operate {
                    key: tab.key.clone(),
                    operation: operation.into(),
                    input,
                    timeout_ms: timeout.as_millis().min(u64::MAX as u128) as u64,
                    generation: tab.control.generation(),
                },
                timeout + Duration::from_millis(500),
            )
            .await
    }
    async fn operate(
        &self,
        tab: Arc<Tab>,
        agent: &str,
        operation: &str,
        mut input: Value,
        execution: Execution,
    ) -> Result<Value> {
        let Execution {
            deadline,
            read,
            admission,
        } = execution;
        let service = self.clone();
        let owned = tab.clone();
        let operation = operation.to_owned();
        let run = async move {
            if Instant::now() >= deadline {
                return Err(Error::Timeout("expired in control queue".into()));
            }
            if !read {
                admission().await?;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(Error::Timeout("expired during admission".into()));
            }
            let budget = remaining.as_millis().min(u64::MAX as u128) as u64;
            let requested = input["timeoutMs"].as_u64().unwrap_or(budget).max(1);
            input["timeoutMs"] = requested
                .min(budget)
                .min(if operation == "navigate" {
                    15000
                } else {
                    u64::MAX
                })
                .into();
            if operation == "resize" {
                if lock(&owned.desktop).is_some() {
                    let prepared = service
                        .raw(&owned, "prepareResize", json!({}), Duration::from_secs(1))
                        .await?;
                    input["resizeToken"] = prepared["resizeToken"].clone();
                }
                let setting = serde_json::from_value(input["setting"].clone())?;
                service
                    .manager()
                    .resize(PreviewResizeInput {
                        thread_id: owned.key.thread_id.as_str().into(),
                        tab_id: owned.key.tab_id.as_str().into(),
                        viewport: setting,
                    })
                    .await?;
            }
            if operation == "setColorScheme" {
                service
                    .manager()
                    .adjust(PreviewAdjustInput {
                        thread_id: owned.key.thread_id.as_str().into(),
                        tab_id: owned.key.tab_id.as_str().into(),
                        color_scheme: Some(Some(serde_json::from_value(
                            input["colorScheme"].clone(),
                        )?)),
                        zoom_factor: None,
                        hard_reload: None,
                        clear: None,
                    })
                    .await?;
            }
            let result = service
                .raw(
                    &owned,
                    &operation,
                    input,
                    deadline.saturating_duration_since(Instant::now()),
                )
                .await?;
            if let Some(id) = result["pending"].as_str() {
                let input = json!({"id":id});
                let driver = service.inner.driver.clone();
                let key = owned.key.clone();
                let generation = owned.control.generation();
                let worker_generation = owned.driver_generation();
                let waiting = tokio::spawn(async move {
                    let _ = driver
                        .call_on(
                            worker_generation,
                            BrowserCommand::Operate {
                                key,
                                operation: "waitPending".into(),
                                input,
                                timeout_ms: 16000,
                                generation,
                            },
                            Duration::from_secs(17),
                        )
                        .await;
                });
                owned.control.track(
                    async move {
                        let _ = waiting.await;
                    }
                    .boxed(),
                );
            }
            Ok(result)
        }
        .boxed();
        if read && (tab.control.controller().is_some() || !tab.control.may_act(agent)) {
            tab.control.assert_open()?;
            run.await
        } else {
            tab.control.agent(agent, Some(deadline), run).await
        }
    }
    pub async fn sweep_idle(&self) {
        let idle: Vec<_> = lock(&self.inner.state)
            .tabs
            .values()
            .filter(|t| {
                t.control.agent_id().is_some()
                    && !self.visible(t)
                    && lock(&t.desktop).is_none()
                    && lock(&t.used).elapsed() > IDLE
            })
            .map(|t| t.key.clone())
            .collect();
        for key in idle {
            self.drop_tab(&key, true).await;
        }
    }
    pub async fn cleanup_thread(&self, thread: &ThreadId) -> Result<()> {
        self.manager()
            .close(PreviewCloseInput {
                thread_id: thread.clone(),
                tab_id: None,
            })
            .await?;
        let keys: Vec<_> = lock(&self.inner.state)
            .tabs
            .keys()
            .filter(|key| key.thread_id == thread.0.as_str())
            .cloned()
            .collect();
        for key in keys {
            self.drop_tab(&key, true).await;
        }
        Ok(())
    }
    pub async fn shutdown(&self) -> Result<()> {
        let keys = {
            let mut state = lock(&self.inner.state);
            state.closed = true;
            state.tabs.keys().cloned().collect::<Vec<_>>()
        };
        for key in keys {
            self.drop_tab(&key, true).await;
        }
        self.inner.stop.cancel();
        self.inner
            .driver
            .call(BrowserCommand::Shutdown, Duration::from_secs(5))
            .await?;
        Ok(())
    }
    fn broadcast_control(&self, tab: &Tab) {
        let controller = tab.control.controller();
        let dialog = lock(&tab.cached)["dialog"].clone();
        for (id, viewer) in lock(&tab.viewers).iter() {
            let name = if controller.as_ref() == Some(id) {
                "you"
            } else if controller.is_some() {
                "another-viewer"
            } else if tab.control.agent_id().is_some() {
                "agent"
            } else {
                "unclaimed"
            };
            let dropped = viewer.queue.push(control_message(
                viewer.can_operate,
                name,
                tab.control.generation(),
                dialog.clone(),
            ));
            for frame in dropped {
                self.ack_frame(tab.key.clone(), id.clone(), frame, tab.driver_generation());
            }
        }
    }
    fn ack_frame(&self, key: TabKey, viewer_id: String, frame_id: u64, worker_generation: u64) {
        let driver = self.inner.driver.clone();
        tokio::spawn(async move {
            let _ = driver
                .call_on(
                    worker_generation,
                    BrowserCommand::ViewerAck {
                        key,
                        viewer_id,
                        frame_id,
                    },
                    Duration::from_secs(2),
                )
                .await;
        });
    }
    pub async fn attach_viewer(
        &self,
        key: TabKey,
        options: ViewerOptions,
    ) -> Result<PreviewViewer> {
        let listed = self
            .manager()
            .list(PreviewListInput {
                thread_id: key.thread_id.as_str().into(),
            })
            .await;
        let snapshot = listed
            .sessions
            .into_iter()
            .find(|s| {
                s.tab_id.as_str() == key.tab_id && s.runtime == Some(Some(PreviewRuntime::Server))
            })
            .ok_or(Error::TabNotFound)?;
        let tab = self.ensure(snapshot).await?;
        let id = uuid::Uuid::new_v4().to_string();
        let queue = ViewerQueue::default();
        lock(&tab.viewers).insert(
            id.clone(),
            ViewerState {
                can_operate: options.can_operate,
                queue: queue.clone(),
            },
        );
        tab.viewer_attached.notify_waiters();
        let viewer = PreviewViewer {
            service: Arc::downgrade(&self.inner),
            key: key.clone(),
            id: id.clone(),
            can_operate: options.can_operate,
            worker_generation: tab.driver_generation(),
            output: queue,
        };
        if options.can_operate
            && tab.control.agent_id().is_none()
            && tab.control.controller().is_none()
        {
            tab.control.take(&id)?.await?;
        }
        self.broadcast_control(&tab);
        self.inner
            .driver
            .call_on(
                tab.driver_generation(),
                BrowserCommand::ViewerAttach {
                    key,
                    viewer_id: id,
                    max_width: options.max_width.clamp(1, 7680),
                    max_height: options.max_height.clamp(1, 4320),
                    quality: options.quality.clamp(1, 100),
                },
                DRIVER_TIMEOUT,
            )
            .await?;
        Ok(viewer)
    }
    async fn viewer_input(&self, viewer: &PreviewViewer, input: Value) -> Result<()> {
        if !viewer.can_operate {
            return Ok(());
        }
        let tab = self.get(&viewer.key).ok_or(Error::TabNotFound)?;
        if viewer.worker_generation != tab.driver_generation()
            || !lock(&tab.viewers).contains_key(&viewer.id)
        {
            return Err(Error::Disconnected);
        }
        let kind = input["type"].as_str().unwrap_or("").to_owned();
        match kind.as_str() {
            "takeControl" => {
                let taken = tab.control.take(&viewer.id)?;
                self.broadcast_control(&tab);
                self.raw(
                    &tab,
                    "invalidate",
                    json!({"controller":tab.control.controller()}),
                    Duration::from_secs(2),
                )
                .await?;
                taken.await?;
            }
            "releaseControl" => {
                let driver = self.inner.driver.clone();
                let key = viewer.key.clone();
                let viewer_id = viewer.id.clone();
                let worker_generation = viewer.worker_generation;
                let released = tab.control.release(
                    &viewer.id,
                    async move {
                        driver
                            .call_on(
                                worker_generation,
                                BrowserCommand::ViewerInput {
                                    key,
                                    viewer_id,
                                    input: json!({"type":"releaseInput"}),
                                },
                                Duration::from_secs(5),
                            )
                            .await
                    }
                    .boxed(),
                )?;
                self.broadcast_control(&tab);
                self.raw(
                    &tab,
                    "invalidate",
                    json!({"controller":tab.control.controller()}),
                    Duration::from_secs(2),
                )
                .await?;
                released.await?;
            }
            "dialog" => {
                if tab.control.controller().as_deref() != Some(&viewer.id) {
                    return Err(Error::Control(PreviewAutomationControlReason::Interrupted));
                }
                self.raw(&tab, "dialog", input, Duration::from_secs(5))
                    .await?;
            }
            _ => {
                let mut input = input;
                if kind == "navigate" {
                    input["url"] = crate::urls::normalize_preview_url(
                        input["url"]
                            .as_str()
                            .ok_or_else(|| Error::InvalidInput("Missing URL".into()))?,
                    )?
                    .into();
                }
                let key = viewer.key.clone();
                let viewer_id = viewer.id.clone();
                let driver = self.inner.driver.clone();
                let control = tab.control.clone();
                let worker_generation = viewer.worker_generation;
                let manager = self.inner.manager.clone();
                let dispatch: Action = Arc::new(move |mut input| {
                    let driver = driver.clone();
                    let key = key.clone();
                    let viewer_id = viewer_id.clone();
                    let control = control.clone();
                    let manager = manager.clone();
                    async move {
                        if input["type"] == "viewport" {
                            let setting: PreviewViewportSetting =
                                serde_json::from_value(input["setting"].clone())?;
                            let snapshot = manager
                                .resize(PreviewResizeInput {
                                    thread_id: key.thread_id.as_str().into(),
                                    tab_id: key.tab_id.as_str().into(),
                                    viewport: setting,
                                })
                                .await?;
                            input["setting"] = serde_json::to_value(snapshot.viewport.flatten())?;
                        }
                        let result = driver
                            .call_on(
                                worker_generation,
                                BrowserCommand::ViewerInput {
                                    key: key.clone(),
                                    viewer_id,
                                    input,
                                },
                                Duration::from_secs(16),
                            )
                            .await?;
                        if let Some(id) = result["pending"].as_str() {
                            let input = json!({"id":id});
                            let generation = control.generation();
                            let work = tokio::spawn(async move {
                                let _ = driver
                                    .call_on(
                                        worker_generation,
                                        BrowserCommand::Operate {
                                            key,
                                            operation: "waitPending".into(),
                                            input,
                                            timeout_ms: 16000,
                                            generation,
                                        },
                                        Duration::from_secs(17),
                                    )
                                    .await;
                            });
                            control.track_until_handoff(
                                async move {
                                    let _ = work.await;
                                }
                                .boxed(),
                            );
                        }
                        Ok(result)
                    }
                    .boxed()
                });
                let merge_key = if kind == "mouse"
                    && input["action"] == "move"
                    && input["button"] == "none"
                    && input["buttons"] == 0
                {
                    Some(format!("hover:{}", input["modifiers"]))
                } else if kind == "wheel" {
                    Some(format!(
                        "wheel:{}:{}:{}:{}:{}",
                        input["x"],
                        input["y"],
                        input["modifiers"],
                        input["deltaX"].as_f64().unwrap_or(0.0).signum(),
                        input["deltaY"].as_f64().unwrap_or(0.0).signum()
                    ))
                } else {
                    None
                };
                if let Some(key) = merge_key {
                    tab.control
                        .human_latest(&viewer.id, &key, input, dispatch, |old, mut new| {
                            if new["type"] == "wheel" {
                                for axis in ["deltaX", "deltaY"] {
                                    new[axis] = (old[axis].as_f64().unwrap_or(0.0)
                                        + new[axis].as_f64().unwrap_or(0.0))
                                    .into();
                                }
                            }
                            new
                        })
                        .await?;
                } else {
                    tab.control.human(&viewer.id, dispatch(input)).await?;
                }
            }
        }
        self.broadcast_control(&tab);
        Ok(())
    }
    pub async fn answer_file_chooser(
        &self,
        key: TabKey,
        chooser_id: String,
        files: Vec<UploadFile>,
    ) -> Result<bool> {
        let tab = self.get(&key).ok_or(Error::TabNotFound)?;
        if tab.control.controller().is_none() {
            return Ok(false);
        }
        if files.len() > 20 {
            return Err(Error::InvalidInput("Too many uploaded files".into()));
        }
        let mut total = 0usize;
        for file in &files {
            let bytes = STANDARD
                .decode(&file.data)
                .map_err(|e| Error::InvalidInput(e.to_string()))?;
            total = total.saturating_add(bytes.len());
            if total > 100 * 1024 * 1024 {
                return Err(Error::InvalidInput("Upload size limit".into()));
            }
        }
        let result = self
            .inner
            .driver
            .call_on(
                tab.driver_generation(),
                BrowserCommand::AnswerFileChooser {
                    key,
                    chooser_id,
                    files,
                },
                Duration::from_secs(15),
            )
            .await?;
        Ok(result == true)
    }
    pub async fn open_download(&self, key: TabKey, id: &str) -> Result<Option<Value>> {
        let Some(tab) = self.get(&key) else {
            return Ok(None);
        };
        let value = self
            .raw(&tab, "download", json!({"id":id}), Duration::from_secs(1))
            .await?;
        Ok((!value.is_null()).then_some(value))
    }
    async fn adopt_popup(
        &self,
        opener: TabKey,
        popup_id: String,
        url: Option<String>,
    ) -> Result<()> {
        let result = async {
            let tab = self.get(&opener).ok_or(Error::TabNotFound)?;
            let reservation = self.reserve(tab.control.agent_id()).await?;
            let snapshot = lock(&tab.snapshot).clone();
            let opened = self
                .manager()
                .open_registered(
                    PreviewOpenInput {
                        thread_id: opener.thread_id.as_str().into(),
                        url: url
                            .filter(|url| url.starts_with("http:") || url.starts_with("https:"))
                            .map(|url| Some(url.into())),
                        viewport: snapshot.viewport,
                        color_scheme: None,
                        zoom_factor: None,
                        profile_id: snapshot.profile_id,
                        runtime: Some(Some(PreviewRuntime::Server)),
                        reveal: tab.control.agent_id().map(|_| Some(false)),
                    },
                    tab.control.agent_id().map(str::to_owned),
                    |snapshot| {
                        self.register(
                            snapshot.clone(),
                            Some(&reservation),
                            Some(popup_id.clone()),
                            Some(opener.tab_id.clone()),
                        )
                        .map(|_| ())
                    },
                )
                .await;
            lock(&self.inner.state).reservations.remove(&reservation);
            let opened = opened?;
            let adopted = self.ensure(opened).await?;
            if tab.control.agent_id().is_none()
                && let Some(viewer) = tab
                    .control
                    .controller()
                    .and_then(|id| lock(&tab.viewers).get(&id).map(|v| v.queue.clone()))
            {
                viewer.push(ViewerOutput::Message(
                    json!({"type":"popup","tabId":adopted.key.tab_id}),
                ));
            }
            Ok(())
        }
        .await;
        if result.is_err() {
            let _ = self
                .inner
                .driver
                .call(
                    BrowserCommand::DiscardPopup { popup_id },
                    Duration::from_secs(5),
                )
                .await;
        }
        result
    }
}
fn automation_input(operation: PreviewAutomationOperation, input: Value) -> Result<Value> {
    if !input.is_object() {
        return Err(Error::InvalidInput(
            "Expected an automation input object".into(),
        ));
    }
    fn decode<T: serde::de::DeserializeOwned + serde::Serialize>(input: Value) -> Result<Value> {
        Ok(serde_json::to_value(serde_json::from_value::<T>(input)?)?)
    }
    use PreviewAutomationOperation::*;
    match operation {
        Status | Close | RecordingStart | RecordingStop => {
            decode::<PreviewAutomationTabTargetInput>(input)
        }
        Open => decode::<PreviewAutomationOpenInput>(input),
        Navigate => decode::<PreviewAutomationNavigateInput>(input),
        Resize => decode::<PreviewAutomationResizeInput>(input),
        SetColorScheme => decode::<PreviewAutomationSetColorSchemeInput>(input),
        Snapshot => decode::<PreviewAutomationSnapshotInput>(input),
        Click => decode::<PreviewAutomationClickInput>(input),
        Hover => decode::<PreviewAutomationHoverInput>(input),
        Type => decode::<PreviewAutomationTypeInput>(input),
        Press => decode::<PreviewAutomationPressInput>(input),
        Scroll => decode::<PreviewAutomationScrollInput>(input),
        Select => decode::<PreviewAutomationSelectInput>(input),
        Drag => decode::<PreviewAutomationDragInput>(input),
        Upload => decode::<PreviewAutomationUploadInput>(input),
        Dialog => decode::<PreviewAutomationDialogInput>(input),
        Evaluate => decode::<PreviewAutomationEvaluateInput>(input),
        WaitFor => decode::<PreviewAutomationWaitForInput>(input),
    }
}
fn owner(tab: &Tab) -> &str {
    if tab.control.controller().is_some() {
        "human"
    } else if tab.control.agent_id().is_some() {
        "agent"
    } else {
        "unclaimed"
    }
}

#[derive(Clone, Copy)]
pub struct ViewerOptions {
    pub can_operate: bool,
    pub max_width: u32,
    pub max_height: u32,
    pub quality: u8,
}
pub struct PreviewViewer {
    worker_generation: u64,
    service: Weak<Inner>,
    key: TabKey,
    id: String,
    can_operate: bool,
    pub output: ViewerQueue,
}
impl PreviewViewer {
    pub async fn input(&self, input: Value) -> Result<()> {
        let inner = self.service.upgrade().ok_or(Error::Disconnected)?;
        PreviewService { inner }.viewer_input(self, input).await
    }
    pub async fn acknowledge(&self, frame_id: u64) -> Result<()> {
        let inner = self.service.upgrade().ok_or(Error::Disconnected)?;
        inner
            .driver
            .call_on(
                self.worker_generation,
                BrowserCommand::ViewerAck {
                    key: self.key.clone(),
                    viewer_id: self.id.clone(),
                    frame_id,
                },
                Duration::from_secs(2),
            )
            .await?;
        Ok(())
    }
}
impl Drop for PreviewViewer {
    fn drop(&mut self) {
        if let Some(inner) = self.service.upgrade()
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            let key = self.key.clone();
            let id = self.id.clone();
            let worker_generation = self.worker_generation;
            self.output.close();
            runtime.spawn(async move {
                let service = PreviewService { inner };
                if let Some(tab) = service
                    .get(&key)
                    .filter(|tab| tab.driver_generation() == worker_generation)
                {
                    let _ = tab
                        .control
                        .disconnect(&id, futures_util::future::ready(Ok(Value::Null)).boxed())
                        .await;
                    lock(&tab.viewers).remove(&id);
                    service.broadcast_control(&tab);
                }
                let _ = service
                    .inner
                    .driver
                    .call_on(
                        worker_generation,
                        BrowserCommand::ViewerDetach { key, viewer_id: id },
                        Duration::from_secs(5),
                    )
                    .await;
            });
        }
    }
}

fn redact_crashed_url(url: &str) -> String {
    if !url
        .get(..5)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("data:"))
    {
        return url.into();
    }
    let media = url[5..].split([';', ',']).next().unwrap_or("").trim();
    let valid = media.split_once('/').is_some_and(|(a, b)| {
        [a, b].iter().all(|part| {
            !part.is_empty()
                && part.len() <= 64
                && part
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_.+-".contains(&c))
        })
    });
    format!("data:{},…", if valid { media } else { "" })
}
