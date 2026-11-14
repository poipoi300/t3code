use crate::{
    Error, Result, lock, now,
    urls::{normalize_preview_url, validate_viewport},
};
use std::sync::{Arc, Mutex};
use t3_contracts::{base_schemas::TrimmedNonEmptyString, preview::*};
use tokio::sync::mpsc;

pub struct Events<T>(Arc<Mutex<Vec<mpsc::UnboundedSender<T>>>>);
impl<T> Clone for Events<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<T> Default for Events<T> {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(Vec::new())))
    }
}
impl<T: Clone> Events<T> {
    pub fn subscribe(&self) -> mpsc::UnboundedReceiver<T> {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut subscribers = lock(&self.0);
        subscribers.retain(|subscriber| !subscriber.is_closed());
        subscribers.push(tx);
        rx
    }
    pub(crate) fn publish(&self, event: T) {
        lock(&self.0).retain(|tx| tx.send(event.clone()).is_ok());
    }
}

#[derive(Default)]
struct State {
    sessions: Vec<PreviewSessionSnapshot>,
    revision: u64,
    sequence: u64,
}

#[derive(Clone)]
pub struct PreviewManager {
    state: Arc<Mutex<State>>,
    epoch: TrimmedNonEmptyString,
    events: Events<PreviewEvent>,
}

impl Default for PreviewManager {
    fn default() -> Self {
        Self {
            state: Arc::default(),
            epoch: uuid::Uuid::new_v4().to_string().into(),
            events: Events::default(),
        }
    }
}

impl PreviewManager {
    pub fn tab_thread(&self, tab_id: &str) -> Option<String> {
        lock(&self.state)
            .sessions
            .iter()
            .find(|session| session.tab_id.as_str() == tab_id)
            .map(|session| session.thread_id.0.clone())
    }
    pub fn subscribe_events(&self) -> mpsc::UnboundedReceiver<PreviewEvent> {
        self.events.subscribe()
    }

    pub async fn open(&self, input: PreviewOpenInput) -> Result<PreviewSessionSnapshot> {
        self.open_owned(input, None).await
    }

    pub(crate) async fn open_owned(
        &self,
        input: PreviewOpenInput,
        owner: Option<String>,
    ) -> Result<PreviewSessionSnapshot> {
        self.open_registered(input, owner, |_| Ok(())).await
    }

    pub(crate) async fn open_registered(
        &self,
        input: PreviewOpenInput,
        owner: Option<String>,
        before_publish: impl FnOnce(&PreviewSessionSnapshot) -> Result<()>,
    ) -> Result<PreviewSessionSnapshot> {
        let viewport = input
            .viewport
            .flatten()
            .unwrap_or(PreviewViewportSetting::Fill {});
        validate_viewport(&viewport)?;
        let nav_status = match input.url.flatten() {
            Some(url) => PreviewNavStatus::Loading {
                url: normalize_preview_url(url.as_str())?.into(),
                title: String::new(),
            },
            None => PreviewNavStatus::Idle {},
        };
        let runtime = input.runtime.flatten();
        let mut state = lock(&self.state);
        state.sequence += 1;
        let mut tab_id = format!("tab_{}", base36(state.sequence));
        if runtime == Some(PreviewRuntime::Server) {
            tab_id.push('_');
            tab_id.push_str(self.epoch.as_str());
        }
        let snapshot = PreviewSessionSnapshot {
            thread_id: input.thread_id.0.as_str().into(),
            tab_id: tab_id.into(),
            nav_status,
            can_go_back: false,
            can_go_forward: false,
            viewport: Some(Some(viewport)),
            color_scheme: input.color_scheme,
            zoom_factor: input.zoom_factor,
            profile_id: input.profile_id,
            runtime: runtime.map(Some),
            headless: None,
            automation_owner: if runtime == Some(PreviewRuntime::Server) {
                owner.map(Some)
            } else {
                None
            },
            reveal: input.reveal,
            reveal_request: None,
            updated_at: now(),
        };
        before_publish(&snapshot)?;
        state.sessions.push(snapshot.clone());
        state.revision += 1;
        self.events.publish(PreviewEvent::Opened {
            thread_id: snapshot.thread_id.clone(),
            tab_id: snapshot.tab_id.clone(),
            created_at: snapshot.updated_at.clone(),
            server_epoch: self.epoch.clone(),
            revision: state.revision,
            snapshot: snapshot.clone(),
        });
        Ok(snapshot)
    }

    fn mutate(
        &self,
        thread: &str,
        tab: &str,
        kind: &str,
        request: Option<PreviewReloadRequest>,
        f: impl FnOnce(&mut PreviewSessionSnapshot) -> Result<bool>,
    ) -> Result<PreviewSessionSnapshot> {
        let mut state = lock(&self.state);
        let snapshot = state
            .sessions
            .iter_mut()
            .find(|s| s.thread_id.as_str() == thread && s.tab_id.as_str() == tab)
            .ok_or(Error::TabNotFound)?;
        let changed = f(snapshot)?;
        let snapshot = snapshot.clone();
        if !changed {
            return Ok(snapshot);
        }
        state.revision += 1;
        let event = if kind == "resized" {
            PreviewEvent::Resized {
                thread_id: snapshot.thread_id.clone(),
                tab_id: snapshot.tab_id.clone(),
                created_at: now(),
                server_epoch: self.epoch.clone(),
                revision: state.revision,
                snapshot: snapshot.clone(),
                request: request.map(Some),
            }
        } else if kind == "status"
            && let PreviewNavStatus::LoadFailed {
                url,
                title,
                code,
                description,
                download,
            } = &snapshot.nav_status
        {
            PreviewEvent::Failed {
                thread_id: snapshot.thread_id.clone(),
                tab_id: snapshot.tab_id.clone(),
                created_at: snapshot.updated_at.clone(),
                server_epoch: self.epoch.clone(),
                revision: state.revision,
                url: url.clone(),
                title: title.clone(),
                code: *code,
                description: description.clone(),
                download: download.clone(),
            }
        } else {
            PreviewEvent::Navigated {
                thread_id: snapshot.thread_id.clone(),
                tab_id: snapshot.tab_id.clone(),
                created_at: snapshot.updated_at.clone(),
                server_epoch: self.epoch.clone(),
                revision: state.revision,
                snapshot: snapshot.clone(),
            }
        };
        self.events.publish(event);
        Ok(snapshot)
    }

    pub async fn report_status(&self, input: PreviewReportStatusInput) -> Result<()> {
        self.report(input, false).await
    }
    pub(crate) async fn report(&self, input: PreviewReportStatusInput, server: bool) -> Result<()> {
        self.mutate(
            input.thread_id.0.as_str(),
            input.tab_id.as_str(),
            "status",
            None,
            |snapshot| {
                if snapshot.runtime == Some(Some(PreviewRuntime::Server)) && !server {
                    return Err(Error::ControlRequired);
                }
                snapshot.nav_status = input.nav_status;
                snapshot.can_go_back = input.can_go_back;
                snapshot.can_go_forward = input.can_go_forward;
                snapshot.updated_at = now();
                Ok(true)
            },
        )?;
        Ok(())
    }
    pub async fn resize(&self, input: PreviewResizeInput) -> Result<PreviewSessionSnapshot> {
        validate_viewport(&input.viewport)?;
        self.mutate(
            input.thread_id.0.as_str(),
            input.tab_id.as_str(),
            "resized",
            None,
            |s| {
                s.viewport = Some(Some(input.viewport));
                s.updated_at = now();
                Ok(true)
            },
        )
    }
    pub async fn adjust(&self, input: PreviewAdjustInput) -> Result<PreviewSessionSnapshot> {
        if let Some(zoom) = input.zoom_factor.as_ref().and_then(Option::as_ref)
            && !PREVIEW_ZOOM_LEVELS.contains(&zoom.value())
        {
            return Err(Error::InvalidInput("Unsupported zoom factor".into()));
        }
        let request = if input.hard_reload == Some(Some(true))
            || input.clear.as_ref().and_then(Option::as_ref).is_some()
        {
            Some(PreviewReloadRequest {
                hard_reload: if input.hard_reload == Some(Some(true)) {
                    input.hard_reload
                } else {
                    None
                },
                clear: input.clear,
            })
        } else {
            None
        };
        self.mutate(
            input.thread_id.0.as_str(),
            input.tab_id.as_str(),
            "resized",
            request,
            |s| {
                if input.color_scheme.is_some() {
                    s.color_scheme = input.color_scheme;
                }
                if input.zoom_factor.is_some() {
                    s.zoom_factor = input.zoom_factor;
                }
                s.updated_at = now();
                Ok(true)
            },
        )
    }
    pub async fn mark_headless(&self, thread: &str, tab: &str) -> Result<()> {
        self.mutate(thread, tab, "resized", None, |s| {
            if s.headless == Some(Some(true)) {
                return Ok(false);
            }
            s.headless = Some(Some(true));
            s.updated_at = now();
            Ok(true)
        })?;
        Ok(())
    }
    pub async fn announce(&self, thread: &str, tab: &str) -> Result<()> {
        self.mutate(thread, tab, "resized", None, |_| Ok(true))?;
        Ok(())
    }
    pub async fn request_reveal(&self, thread: &str, tab: &str, force: bool) -> Result<()> {
        self.mutate(thread, tab, "navigated", None, |s| {
            s.reveal = Some(Some(true));
            s.reveal_request = Some(Some(PreviewRevealRequest {
                id: uuid::Uuid::new_v4().to_string(),
                force,
            }));
            s.updated_at = now();
            Ok(true)
        })?;
        Ok(())
    }
    pub async fn close(&self, input: PreviewCloseInput) -> Result<()> {
        let mut state = lock(&self.state);
        let mut removed = Vec::new();
        state.sessions.retain(|s| {
            let remove = s.thread_id.as_str() == input.thread_id.0.as_str()
                && input
                    .tab_id
                    .as_ref()
                    .and_then(Option::as_ref)
                    .is_none_or(|id| *id == s.tab_id);
            if remove {
                removed.push(s.clone());
            }
            !remove
        });
        let created_at = now();
        for s in removed {
            state.revision += 1;
            self.events.publish(PreviewEvent::Closed {
                thread_id: s.thread_id,
                tab_id: s.tab_id,
                created_at: created_at.clone(),
                server_epoch: self.epoch.clone(),
                revision: state.revision,
            });
        }
        Ok(())
    }
    pub async fn list(&self, input: PreviewListInput) -> PreviewListResult {
        let state = lock(&self.state);
        let mut sessions: Vec<_> = state
            .sessions
            .iter()
            .filter(|s| s.thread_id.as_str() == input.thread_id.0.as_str())
            .cloned()
            .collect();
        sessions.sort_by(|a, b| a.updated_at.cmp(&b.updated_at));
        PreviewListResult {
            sessions,
            server_epoch: self.epoch.clone(),
            revision: state.revision,
        }
    }
}
fn base36(mut n: u64) -> String {
    let mut out = Vec::new();
    loop {
        out.push(char::from_digit((n % 36) as u32, 36).expect("base 36 digit"));
        n /= 36;
        if n == 0 {
            break;
        }
    }
    out.into_iter().rev().collect()
}
