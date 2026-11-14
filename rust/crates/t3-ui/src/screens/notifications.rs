//! Port of apps/web/src/components/ThreadNotificationCoordinator.tsx startThreadNotifications,
//! BackgroundThreadAnnouncer.tsx, and the host parts of threadNotifications.ts.
//!
//! `thread_notifications()` is mounted once inside the navigator and the toast provider. Every
//! decision is the client's (`thread_views::threads::{notification_center, notifications}`):
//! which thread changes notify, what they say, toast or system notification. This view feeds it
//! the store's live shells and shows the result through NSH's cross-platform calls
//! (`show_notification`, `set_badge`, `announce_for_accessibility`), which answer
//! "unavailable" where a platform has none, so no platform branch is needed.
//!
//! Not built yet, named in reports/ui-mounts-app-root.md: hidden-thread filtering
//! (`hiddenThreadKeysAtom` has no client producer yet).

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::time::Duration;

use futures::StreamExt;
use nsh_core::notifications::Notification;
use nsh_core::platform::Lifecycle;
use nsh_core::prelude::*;
use t3_client::services::models::PendingSubagentRequest;
use t3_client::thread_views::threads::notification_center::{
    NotificationCenter, NotificationTone, Presentation, ThreadNotification, has_notification_sound,
    presentation,
};
use t3_client::thread_views::threads::notifications::{
    BackgroundMemory, BackgroundThread, NotificationKind, next_background_announcement,
    resolve_background_request,
};
use t3_client::thread_views::threads::shell::{PendingChildRequest, ThreadShellViews};
use t3_client::types::{EnvironmentId, SyncStatus, ThreadId, ThreadKey};
use t3_client::view_logic::session::text::thread_notification_tag;
use t3_contracts::settings::{ClientSettingsSchema, NotificationMode};

use crate::i18n::t;
use crate::runtime::rt;
use crate::runtime::tasks::TaskOwner;
use crate::state::nav::{Route, navigator};
use crate::ui::icon::{Icon, icon_with_class};
use crate::ui::toast::{Action, ThreadToastData, ToastOptions, Variant, toast_manager};

/// The client settings the notifications follow and the client's thread shell views (requests
/// of tasks, threads shown under a parent), provided at the app root
/// (`app::provide_client_services`).
#[derive(Clone)]
pub struct NotificationServices {
    current: Rc<dyn Fn() -> Rc<ClientSettingsSchema>>,
    changes: Rc<dyn Fn() -> futures::stream::LocalBoxStream<'static, ()>>,
    shells: Rc<dyn Fn() -> Rc<ThreadShellViews>>,
}

pub fn notification_services<P: t3_client::platform::Platform + Clone + 'static>(
    client: &t3_client::composition::ClientRuntime<P>,
) -> NotificationServices {
    let current = client.settings();
    let changes = current.clone();
    let threads = client.thread_header();
    NotificationServices {
        current: Rc::new(move || current.client_settings()),
        changes: Rc::new(move || changes.changes()),
        shells: Rc::new(move || threads.shell_views()),
    }
}

/// `pendingSubagentRequestsAtom`: a task's request, as its parent hears of it.
fn parent_request(request: PendingChildRequest) -> PendingSubagentRequest {
    match request {
        PendingChildRequest::Approval => PendingSubagentRequest::Approval,
        PendingChildRequest::UserInput | PendingChildRequest::UserOnlyInput => {
            PendingSubagentRequest::UserInput
        }
    }
}

/// `startThreadNotifications` and `startBackgroundThreadAnnouncements` for the life of the app.
pub fn thread_notifications() -> impl IntoElement {
    let runtime = rt();
    let nav = navigator();
    let Some(settings_service) = use_context::<NotificationServices>() else {
        // The demo backend has no settings or live environments to watch.
        debug_assert!(
            runtime.backend().client().is_none(),
            "NotificationServices is not provided: app::provide_client_services provides it with the client"
        );
        return view().app_class(crate::theme::tw("hidden"));
    };
    let settings = signal_with((settings_service.current)(), SignalEquals::Never);
    let watch = settings_service.clone();
    spawn(async move {
        let mut changes = (watch.changes)();
        while changes.next().await.is_some() {
            let _ = settings.try_set((watch.current)());
        }
    });

    let center = Rc::new(RefCell::new(NotificationCenter::default()));
    let deadline = signal(None::<u64>);
    // System notifications showing, by tag, for the badge and for presses.
    let pending: Rc<RefCell<BTreeMap<String, ThreadKey>>> = Rc::default();
    let background: Rc<RefCell<BackgroundMemory>> = Rc::default();

    let open_thread = move |key: ThreadKey| nav.open(Route::Thread(key));
    let open_key = move || match nav.top() {
        Route::Thread(key) | Route::ThreadPanel(key, _) => Some(key),
        _ => None,
    };

    let sounds = Rc::new(Sounds::new(TaskOwner::here()));
    let show: Rc<dyn Fn(Vec<ThreadNotification>)> = {
        let pending = pending.clone();
        Rc::new(move |shown: Vec<ThreadNotification>| {
            let current = settings.get_untracked();
            for notification in shown {
                if has_notification_sound(&current.notification_mode) {
                    sounds.play(notification.kind, move || {
                        has_notification_sound(&settings.get_untracked().notification_mode)
                    });
                }
                present(&notification, &current, open_key(), &pending, open_thread);
            }
        })
    };

    // Feed every environment's live shell to the center; a non-live shell pauses its grace.
    let observe_center = center.clone();
    let observe_show = show.clone();
    let observe_background = background.clone();
    let shells = settings_service.shells.clone();
    effect(move || {
        let current = settings.get();
        let enabled = current.notification_mode != NotificationMode::Off
            || current.in_app_notifications_enabled;
        runtime.store.track_environments();
        let snapshot = runtime.store.store().snapshot();
        let environments: BTreeSet<EnvironmentId> = if enabled {
            snapshot.environments.keys().cloned().collect()
        } else {
            BTreeSet::new()
        };
        let mut center = observe_center.borrow_mut();
        center.retain_environments(&environments);
        let now = nsh_core::time::unix_ms() as u64;
        let views = shells();
        let mut shown = Vec::new();
        let mut listed = Vec::new();
        for environment in snapshot.environments.keys() {
            let shell = runtime.store.shell(environment.clone()).get();
            let live = t3_client::state::selectors::shell(&snapshot, environment)
                .is_some_and(|state| state.status == SyncStatus::Live);
            match (live, shell) {
                (true, Some(shell)) => {
                    let requests: BTreeMap<ThreadId, PendingSubagentRequest> = views
                        .pending_child_requests
                        .iter()
                        .filter(|(key, _)| &key.environment == environment)
                        .map(|(key, request)| (key.thread.clone(), parent_request(*request)))
                        .collect();
                    if environments.contains(environment) {
                        shown.extend(center.observe(
                            environment,
                            &shell.0.threads,
                            &requests,
                            &views.delegated_under_parent,
                            now,
                        ));
                    }
                    listed.extend(
                        shell
                            .0
                            .threads
                            .iter()
                            // Tasks and side chats have no row; their requests reach the parent.
                            .filter(|thread| {
                                thread.archived_at.is_none()
                                    && !t3_client::view_logic::shell::sidebar::is_hidden_thread(
                                        &thread.lineage,
                                    )
                            })
                            .map(|thread| {
                                let view = t3_client::services::models::present_thread_shell(
                                    environment.clone(),
                                    thread.clone(),
                                    requests.get(&thread.id).copied(),
                                );
                                let shelf = t3_client::view_logic::shell::sidebar::presentation::ThreadInput::from_environment_shell(
                                    &view,
                                    None,
                                    &t3_client::time::epoch_ms,
                                )
                                .shelf;
                                let snoozed = t3_client::thread_views::shelves::effective_snoozed(
                                    &shelf,
                                    now as i64,
                                );
                                let (request, request_id) = resolve_background_request(
                                    view.has_pending_approvals,
                                    view.has_pending_user_input,
                                    view.pending_subagent_request,
                                    thread
                                        .pending_runtime_request
                                        .as_ref()
                                        .map(|request| request.id.0.as_str()),
                                );
                                BackgroundThread {
                                    key: format!("{}:{}", environment.0, thread.id.0.as_str()),
                                    title: thread.title.clone(),
                                    request,
                                    request_id,
                                    run_id: view.latest_run.as_ref().map(|run| {
                                        t3_contracts::orchestration_v2_id_table::IdOrIndex::Id(
                                            run.run_id.clone(),
                                        )
                                    }),
                                    run_status: view
                                        .latest_run
                                        .as_ref()
                                        .map(|run| run.status.clone()),
                                    active_run_id: view
                                        .runtime
                                        .as_ref()
                                        .and_then(|runtime| runtime.active_run_id.clone())
                                        .map(
                                            t3_contracts::orchestration_v2_id_table::IdOrIndex::Id,
                                        ),
                                    snoozed,
                                }
                            }),
                    );
                }
                _ => center.disconnected(environment),
            }
        }
        deadline.set(center.next_deadline());
        drop(center);
        // BackgroundThreadAnnouncer: status changes of threads other than the open one.
        let open = untracked(open_key)
            .map(|key| format!("{}:{}", key.environment.0, key.thread.0.as_str()));
        let (memory, announcement) =
            next_background_announcement(&observe_background.borrow(), &listed, open.as_deref());
        *observe_background.borrow_mut() = memory;
        if let Some(announcement) = announcement {
            announce_for_accessibility(announcement);
        }
        observe_show(shown);
    });

    // Wake completions waiting out their grace.
    let tick_center = center;
    let tick_show = show;
    effect(move || {
        let Some(due) = deadline.get() else {
            return;
        };
        let wait = due.saturating_sub(nsh_core::time::unix_ms() as u64);
        let center = tick_center.clone();
        let show = tick_show.clone();
        set_timeout(Duration::from_millis(wait), move || {
            let shown = center.borrow_mut().tick(nsh_core::time::unix_ms() as u64);
            let _ = deadline.try_set(center.borrow().next_deadline());
            show(shown);
        });
    });

    // A pressed system notification opens its thread; the app coming to the front clears them.
    let press_pending = pending.clone();
    spawn(async move {
        loop {
            let tag = next_notification_press().await;
            let key = press_pending.borrow_mut().remove(&tag);
            set_badge(press_pending.borrow().len() as u32);
            if let Some(key) = key {
                open_thread(key);
            }
        }
    });
    let front_pending = pending;
    effect(move || {
        if lifecycle() == Lifecycle::Active {
            let tags: Vec<String> = front_pending.borrow_mut().keys().cloned().collect();
            for tag in &tags {
                close_notification(tag.clone());
            }
            front_pending.borrow_mut().clear();
            set_badge(0);
        }
    });

    view().app_class(crate::theme::tw("hidden"))
}

/// The web's alert sounds, kept byte-identical by `tests::sounds_match_the_web`.
const COMPLETION_SOUND: &[u8] = include_bytes!("../../assets/sounds/notification-completion.mp3");
const INPUT_SOUND: &[u8] = include_bytes!("../../assets/sounds/notification-input.mp3");

/// `playNotificationSound`: each alert decoded once, then played. Another of the same alert while
/// one still plays is coalesced; completion and input keep separate slots.
struct Sounds {
    tasks: TaskOwner,
    slots: [RefCell<SoundSlot>; 2],
}

#[derive(Default)]
struct SoundSlot {
    sound: Option<Sound>,
    playing_until: u64,
}

impl Sounds {
    fn new(tasks: TaskOwner) -> Self {
        Self {
            tasks,
            slots: Default::default(),
        }
    }

    fn play(self: &Rc<Self>, kind: NotificationKind, should_play: impl Fn() -> bool + 'static) {
        let (index, bytes) = match kind {
            NotificationKind::Completion => (0, COMPLETION_SOUND),
            NotificationKind::Input => (1, INPUT_SOUND),
        };
        let sounds = self.clone();
        self.tasks.spawn(async move {
            let cached = sounds.slots[index].borrow().sound.clone();
            let sound = match cached {
                Some(sound) => sound,
                // A failed load (no audio here, a refused capability) is retried on the next alert.
                None => match load_sound(bytes).await {
                    Ok(sound) => {
                        sounds.slots[index].borrow_mut().sound = Some(sound.clone());
                        sound
                    }
                    Err(_) => return,
                },
            };
            let now = nsh_core::time::unix_ms() as u64;
            let mut slot = sounds.slots[index].borrow_mut();
            if !should_play() || now < slot.playing_until {
                return;
            }
            slot.playing_until = now + sound.duration().as_millis() as u64;
            drop(slot);
            sound.play(1.0, 1.0);
        });
    }
}

/// `present`: a toast in front of the app, else a system notification, as the client decides.
fn present(
    notification: &ThreadNotification,
    settings: &ClientSettingsSchema,
    open: Option<ThreadKey>,
    pending: &Rc<RefCell<BTreeMap<String, ThreadKey>>>,
    open_thread: impl Fn(ThreadKey) + Copy + 'static,
) {
    let key = notification.key();
    let in_front = lifecycle() == Lifecycle::Active;
    match presentation(
        &settings.notification_mode,
        settings.in_app_notifications_enabled,
        in_front,
        open.as_ref() == Some(&key),
        false,
    ) {
        Presentation::Toast => {
            let (variant, glyph, class) = match notification.tone {
                NotificationTone::Completed => (
                    Variant::Success,
                    Icon::CircleCheck,
                    "size-4 text-success-foreground",
                ),
                NotificationTone::Approval => (
                    Variant::Warning,
                    Icon::ShieldQuestionMark,
                    "size-4 text-warning-foreground",
                ),
                NotificationTone::Limited => (
                    Variant::Warning,
                    Icon::CircleAlert,
                    "size-4 text-warning-foreground",
                ),
                NotificationTone::Failed => (
                    Variant::Error,
                    Icon::CircleAlert,
                    "size-4 text-destructive-foreground",
                ),
                NotificationTone::Question => (
                    Variant::Warning,
                    Icon::MessageCircleQuestionMark,
                    "size-4 text-info-foreground",
                ),
            };
            let manager = toast_manager();
            let body = notification.body.clone();
            let id_cell: Rc<RefCell<Option<crate::ui::toast::ToastId>>> = Rc::default();
            let close_id = id_cell.clone();
            let target = key.clone();
            let id = manager.add(ToastOptions {
                variant,
                // A client English key, translated where the toast shows it.
                title: crate::i18n::Text::client_key(notification.title),
                description: Some(crate::i18n::Text::content(body.clone())),
                // The thread's title is content: shown as it is, never translated.
                description_content: Some(Rc::new(move || text(body.clone()).into_element())),
                action: Some(Action {
                    label: "Open thread".into(),
                    disabled: false,
                    on_press: Rc::new(move || {
                        if let Some(id) = *close_id.borrow() {
                            toast_manager().close(id);
                        }
                        open_thread(target.clone());
                    }),
                }),
                data: ThreadToastData {
                    hide_copy_button: true,
                    leading_icon: Some(Rc::new(move || {
                        icon_with_class(glyph, class).into_element()
                    })),
                    ..ThreadToastData::default()
                },
                ..ToastOptions::default()
            });
            *id_cell.borrow_mut() = Some(id);
        }
        Presentation::System => {
            let tag = thread_notification_tag(&key.environment.0, key.thread.0.as_str());
            let shown = show_notification(
                Notification::new(
                    tag.clone(),
                    t(notification.title),
                    notification.body.clone(),
                )
                .silent(true),
            );
            let pending = pending.clone();
            spawn(async move {
                if shown.await {
                    pending.borrow_mut().insert(tag, key);
                    set_badge(pending.borrow().len() as u32);
                }
            });
        }
        Presentation::None => {}
    }
}

#[cfg(test)]
mod tests {
    /// The alerts are the web's files, byte for byte, while the web app is beside this crate; a
    /// change there must be copied here. Skipped where there is no `apps/web`.
    #[test]
    fn sounds_match_the_web() {
        let web =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../apps/web/src/assets");
        if !web.is_dir() {
            return;
        }
        for (name, ours) in [
            ("notification-completion.mp3", super::COMPLETION_SOUND),
            ("notification-input.mp3", super::INPUT_SOUND),
        ] {
            let theirs =
                std::fs::read(web.join(name)).unwrap_or_else(|error| panic!("{name}: {error}"));
            assert!(
                theirs == ours,
                "assets/sounds/{name} differs from apps/web/src/assets/{name}: copy it again"
            );
        }
    }
}
