//! Port of apps/web/src/components/ThreadNotificationCoordinator.tsx's decisions: per connected
//! environment, the observation of each listed thread, the gate (`notifications.rs`), and what
//! a notification says. The host shows it (sound, in-app toast or system notification), keeps
//! the badge, and calls [`NotificationCenter::tick`] at [`NotificationCenter::next_deadline`].

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use t3_contracts::orchestration_v2::{
    OrchestrationV2PendingBackgroundTask, OrchestrationV2RunStatus, OrchestrationV2ThreadShell,
};
use t3_contracts::settings::NotificationMode;

use super::notifications::{
    NotificationGate, NotificationKind, NotificationObservation, ObserveOptions,
    resolve_background_request,
};
use super::shell::is_shown_under_parent;
use crate::services::models::{PendingSubagentRequest, present_thread_shell};
use crate::types::{EnvironmentId, ThreadId, ThreadKey};
use crate::view_logic::shell::sidebar::presentation::ThreadInput;
use crate::view_logic::shell::sidebar::status::{Status, resolve_status};

/// `hasNotificationSound`.
pub fn has_notification_sound(mode: &NotificationMode) -> bool {
    matches!(
        mode,
        NotificationMode::Sound | NotificationMode::NotificationsAndSound
    )
}

/// `hasDesktopNotifications`.
pub fn has_desktop_notifications(mode: &NotificationMode) -> bool {
    matches!(
        mode,
        NotificationMode::Notifications | NotificationMode::NotificationsAndSound
    )
}

const PULL_REQUEST_WATCH_TASK_ID: &str = "pull-request-watch:";

/// `monitorHoldsWakeCompletion`: a monitor wakes the agent once per line, so a progress wake
/// run's completion waits for the monitor to end; a pull request watch does not hold it.
pub fn monitor_holds_wake_completion(thread: &OrchestrationV2ThreadShell) -> bool {
    thread.latest_run_background_wake == Some(Some(true))
        && thread
            .pending_background_tasks
            .as_ref()
            .and_then(Option::as_ref)
            .is_some_and(|tasks| {
                tasks.iter().any(|task| match task {
                    OrchestrationV2PendingBackgroundTask::Monitor(task) => {
                        !task.task_id.0.starts_with(PULL_REQUEST_WATCH_TASK_ID)
                    }
                    _ => false,
                })
            })
}

/// How a notification looks: the toast type and its leading icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationTone {
    Completed,
    Approval,
    Limited,
    Failed,
    Question,
}

/// One notification to present.
#[derive(Debug, Clone, PartialEq)]
pub struct ThreadNotification {
    pub environment: EnvironmentId,
    pub thread: ThreadId,
    pub kind: NotificationKind,
    /// An English sentence the UI translates ("Thread completed", "Approval needed", ...).
    pub title: &'static str,
    /// The thread's title, shown as it is.
    pub body: String,
    pub tone: NotificationTone,
}

impl ThreadNotification {
    pub fn key(&self) -> ThreadKey {
        ThreadKey {
            environment: self.environment.clone(),
            thread: self.thread.clone(),
        }
    }
}

/// The `present` title for a notification of `kind` from a thread in `status`.
pub fn notification_title(kind: NotificationKind, status: Status) -> &'static str {
    match (kind, status) {
        (NotificationKind::Completion, _) => "Thread completed",
        (_, Status::Approval) => "Approval needed",
        (_, Status::Limited) => "Usage limit reached",
        (_, Status::Failed) => "Thread failed",
        (_, Status::Request) => "Guest request",
        _ => "Input needed",
    }
}

fn tone(kind: NotificationKind, status: Status) -> NotificationTone {
    match (kind, status) {
        (NotificationKind::Completion, _) => NotificationTone::Completed,
        (_, Status::Approval) => NotificationTone::Approval,
        (_, Status::Limited) => NotificationTone::Limited,
        (_, Status::Failed) => NotificationTone::Failed,
        _ => NotificationTone::Question,
    }
}

#[derive(Debug, Clone)]
struct Observed {
    raw: Rc<OrchestrationV2ThreadShell>,
    subagent_request: Option<PendingSubagentRequest>,
    observation: NotificationObservation,
    status: Status,
    title: String,
}

#[derive(Debug, Default)]
struct Watched {
    previous: BTreeMap<ThreadId, Observed>,
    reconnecting: bool,
    gate: NotificationGate<ThreadId>,
}

/// The coordinator's per-environment state.
#[derive(Debug, Default)]
pub struct NotificationCenter {
    watched: BTreeMap<EnvironmentId, Watched>,
}

fn observe_thread(
    environment: &EnvironmentId,
    raw: &Rc<OrchestrationV2ThreadShell>,
    subagent_request: Option<PendingSubagentRequest>,
) -> Observed {
    let parse = |value: &str| crate::time::epoch_ms(value);
    let thread = present_thread_shell(environment.clone(), raw.clone(), subagent_request);
    let input = ThreadInput::from_environment_shell(&thread, None, &parse);
    let mut status = resolve_status(&input.status);
    let latest_status = thread.latest_run.as_ref().map(|run| &run.status);
    if status == Status::Ready && latest_status == Some(&OrchestrationV2RunStatus::Failed) {
        status = Status::Failed;
    }
    let needs_attention = matches!(
        status,
        Status::Input | Status::Request | Status::Approval | Status::Failed | Status::Limited
    );
    let status_name = match status {
        Status::Input => "input",
        Status::Request => "request",
        Status::Approval => "approval",
        Status::Failed => "failed",
        Status::Limited => "limited",
        _ => "",
    };
    let request_id = needs_attention
        .then(|| {
            resolve_background_request(
                thread.has_pending_approvals,
                thread.has_pending_user_input,
                thread.pending_subagent_request,
                raw.pending_runtime_request
                    .as_ref()
                    .map(|request| request.id.0.as_str()),
            )
            .1
        })
        .flatten();
    // Queued messages become the latest run while a request stays pending, so a request with an
    // id is keyed by that id alone and announces once.
    let attention = needs_attention.then(|| match &request_id {
        Some(id) => format!("{status_name}:{id}"),
        None => format!(
            "{}:{status_name}",
            thread
                .latest_run
                .as_ref()
                .map_or("", |run| run.run_id.0.as_str())
        ),
    });
    let completed =
        status == Status::Ready && latest_status == Some(&OrchestrationV2RunStatus::Completed);
    let settled = completed && !monitor_holds_wake_completion(raw);
    let completed_at = thread
        .latest_run
        .as_ref()
        .and_then(|run| run.completed_at.as_ref())
        .and_then(|at| parse(&at.0));
    Observed {
        raw: raw.clone(),
        subagent_request,
        observation: NotificationObservation {
            attention,
            request_id,
            settled,
            completed_at: if settled { completed_at } else { None },
            background_wake: raw.latest_run_background_wake == Some(Some(true)),
        },
        status,
        title: thread.title.clone(),
    }
}

impl NotificationCenter {
    /// `observeThreads`: a live snapshot of `environment`'s threads arrived. Returns what to
    /// present now. `subagent_requests` and `delegated_under_parent` are the shell's atoms.
    pub fn observe(
        &mut self,
        environment: &EnvironmentId,
        threads: &[Rc<OrchestrationV2ThreadShell>],
        subagent_requests: &BTreeMap<ThreadId, PendingSubagentRequest>,
        delegated_under_parent: &BTreeSet<ThreadKey>,
        now: u64,
    ) -> Vec<ThreadNotification> {
        let entry = self.watched.entry(environment.clone()).or_default();
        let mut next = BTreeMap::new();
        let mut changed = Vec::new();
        for raw in threads {
            let key = ThreadKey {
                environment: environment.clone(),
                thread: raw.id.clone(),
            };
            if is_shown_under_parent(&key, &raw.lineage, delegated_under_parent) {
                continue;
            }
            let prior = entry.previous.get(&raw.id);
            let subagent_request = subagent_requests.get(&raw.id).copied();
            if let Some(prior) = prior
                && !entry.reconnecting
                && Rc::ptr_eq(&prior.raw, raw)
                && prior.subagent_request == subagent_request
            {
                next.insert(raw.id.clone(), prior.clone());
                continue;
            }
            let state = observe_thread(environment, raw, subagent_request);
            changed.push((prior.map(|prior| prior.observation.clone()), raw.id.clone()));
            next.insert(raw.id.clone(), state);
        }
        entry.reconnecting = false;
        entry.previous = next;
        entry
            .gate
            .retain(&entry.previous.keys().cloned().collect::<BTreeSet<_>>());
        let mut shown = Vec::new();
        for (prior, thread) in changed {
            let Some(state) = entry.previous.get(&thread) else {
                continue;
            };
            let options = ObserveOptions {
                archived: state.raw.archived_at.is_some(),
                quiet: state.raw.latest_run_stop_requested == Some(Some(true)),
            };
            if let Some(kind) =
                entry
                    .gate
                    .observe(&thread, prior.as_ref(), &state.observation, options, now)
            {
                shown.push(present(environment, &thread, state, kind));
            }
        }
        shown
    }

    /// The environment's snapshot is not live: its grace timers stop until the next one.
    pub fn disconnected(&mut self, environment: &EnvironmentId) {
        if let Some(entry) = self.watched.get_mut(environment) {
            entry.reconnecting = true;
            entry.gate.pause();
        }
    }

    /// `syncEnvironments`: stop watching environments that left (or every one, when
    /// notifications are off).
    pub fn retain_environments(&mut self, environments: &BTreeSet<EnvironmentId>) {
        self.watched
            .retain(|environment, _| environments.contains(environment));
    }

    /// Wake completions whose grace ended by `now`.
    pub fn tick(&mut self, now: u64) -> Vec<ThreadNotification> {
        let mut shown = Vec::new();
        for (environment, entry) in &mut self.watched {
            for thread in entry.gate.tick(now) {
                if let Some(state) = entry.previous.get(&thread) {
                    shown.push(present(
                        environment,
                        &thread,
                        state,
                        NotificationKind::Completion,
                    ));
                }
            }
        }
        shown
    }

    pub fn next_deadline(&self) -> Option<u64> {
        self.watched
            .values()
            .filter_map(|entry| entry.gate.next_deadline())
            .min()
    }
}

fn present(
    environment: &EnvironmentId,
    thread: &ThreadId,
    state: &Observed,
    kind: NotificationKind,
) -> ThreadNotification {
    ThreadNotification {
        environment: environment.clone(),
        thread: thread.clone(),
        kind,
        title: notification_title(kind, state.status),
        body: state.title.clone(),
        tone: tone(kind, state.status),
    }
}

/// When to show what (the end of `present`): a toast in the app when it is in front and the
/// thread is not the open one, else a system notification when the mode allows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presentation {
    Toast,
    System,
    None,
}

pub fn presentation(
    mode: &NotificationMode,
    in_app_notifications: bool,
    app_in_front: bool,
    thread_open: bool,
    hidden: bool,
) -> Presentation {
    if hidden {
        Presentation::None
    } else if in_app_notifications && app_in_front && !thread_open {
        Presentation::Toast
    } else if has_desktop_notifications(mode) && !app_in_front {
        Presentation::System
    } else {
        Presentation::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: &str = "2026-11-14T10:00:00.000Z";

    fn shell(
        status: &str,
        completed_at: Option<&str>,
        archived: bool,
    ) -> Rc<OrchestrationV2ThreadShell> {
        let value = serde_json::json!({
            "id": "thread-1", "projectId": "project", "title": "Fix the build",
            "providerInstanceId": "codex", "modelSelection": {"instanceId": "codex", "model": "gpt-5.4"},
            "runtimeMode": "full-access", "interactionMode": "default", "branch": null,
            "worktreePath": null, "activeProviderThreadId": null,
            "lineage": {"rootThreadId": "thread-1", "parentThreadId": null, "relationshipToParent": null},
            "forkedFrom": null, "createdBy": "user", "creationSource": "web", "createdAt": NOW,
            "updatedAt": NOW, "archivedAt": if archived { serde_json::json!(NOW) } else { serde_json::Value::Null },
            "settledOverride": null, "settledAt": null, "lastVisitedAt": null, "deletedAt": null,
            "latestRunId": "run-1", "activeRunId": null, "status": status,
            "latestRunCompletedAt": completed_at, "pendingRuntimeRequest": null,
            "latestVisibleMessage": null, "latestUserMessageAt": null,
            "hasActionableProposedPlan": false, "itemCount": 1, "visibleItemCount": 1
        });
        Rc::new(serde_json::from_value(value).expect("a thread shell"))
    }

    fn observe(
        center: &mut NotificationCenter,
        thread: &Rc<OrchestrationV2ThreadShell>,
    ) -> Vec<ThreadNotification> {
        center.observe(
            &EnvironmentId("env".into()),
            std::slice::from_ref(thread),
            &BTreeMap::new(),
            &BTreeSet::new(),
            0,
        )
    }

    #[test]
    fn a_completion_notifies_once_after_the_first_sighting() {
        let mut center = NotificationCenter::default();
        assert!(
            observe(&mut center, &shell("running", None, false)).is_empty(),
            "first sighting"
        );
        let done = shell("completed", Some("2026-11-14T10:05:00.000Z"), false);
        let shown = observe(&mut center, &done);
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].title, "Thread completed");
        assert_eq!(shown[0].body, "Fix the build");
        assert_eq!(shown[0].tone, NotificationTone::Completed);
        assert!(
            observe(&mut center, &done).is_empty(),
            "the same shell is not re-observed"
        );
        let echo = shell("completed", Some("2026-11-14T10:05:00.000Z"), false);
        assert!(
            observe(&mut center, &echo).is_empty(),
            "a handled completion stays quiet"
        );
    }

    #[test]
    fn archived_threads_and_disconnected_environments_stay_quiet() {
        let mut center = NotificationCenter::default();
        observe(&mut center, &shell("running", None, true));
        assert!(
            observe(
                &mut center,
                &shell("completed", Some("2026-11-14T10:05:00.000Z"), true)
            )
            .is_empty()
        );
        let mut other = NotificationCenter::default();
        observe(&mut other, &shell("running", None, false));
        other.retain_environments(&BTreeSet::new());
        assert!(
            observe(
                &mut other,
                &shell("completed", Some("2026-11-14T10:05:00.000Z"), false)
            )
            .is_empty(),
            "an environment watched again starts from a silent first sighting"
        );
    }

    #[test]
    fn modes_decide_sound_and_system_notifications() {
        assert!(has_notification_sound(&NotificationMode::Sound));
        assert!(has_notification_sound(
            &NotificationMode::NotificationsAndSound
        ));
        assert!(!has_notification_sound(&NotificationMode::Notifications));
        assert!(has_desktop_notifications(&NotificationMode::Notifications));
        assert!(!has_desktop_notifications(&NotificationMode::Off));
    }

    #[test]
    fn titles_follow_the_kind_then_the_status() {
        use NotificationKind::*;
        assert_eq!(
            notification_title(Completion, Status::Failed),
            "Thread completed"
        );
        assert_eq!(
            notification_title(Input, Status::Approval),
            "Approval needed"
        );
        assert_eq!(
            notification_title(Input, Status::Limited),
            "Usage limit reached"
        );
        assert_eq!(notification_title(Input, Status::Failed), "Thread failed");
        assert_eq!(notification_title(Input, Status::Request), "Guest request");
        assert_eq!(notification_title(Input, Status::Input), "Input needed");
    }

    #[test]
    fn a_toast_shows_in_front_unless_the_thread_is_open_and_system_notifications_behind() {
        use NotificationMode::*;
        assert_eq!(
            presentation(&Off, true, true, false, false),
            Presentation::Toast
        );
        assert_eq!(
            presentation(&Off, true, true, true, false),
            Presentation::None
        );
        assert_eq!(
            presentation(&Notifications, true, false, false, false),
            Presentation::System
        );
        assert_eq!(
            presentation(&Notifications, false, true, false, false),
            Presentation::None
        );
        assert_eq!(
            presentation(&Sound, true, false, false, false),
            Presentation::None
        );
        assert_eq!(
            presentation(&NotificationsAndSound, true, true, false, true),
            Presentation::None
        );
    }
}
