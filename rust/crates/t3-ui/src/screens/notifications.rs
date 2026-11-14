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
//! Not built yet, each named in reports/ui-sh-notify.md: notification sounds (the web's two mp3
//! assets and an audio context; NSH's audio API is not wired), hidden-thread filtering
//! (`hiddenThreadKeysAtom` has no client selector yet), subagent requests and delegated-under-
//! parent sets (no client selectors yet: treated as empty, so linked-task requests do not notify
//! the parent).

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::time::Duration;

use futures::StreamExt;
use nsh_core::notifications::Notification;
use nsh_core::platform::Lifecycle;
use nsh_core::prelude::*;
use t3_client::thread_views::threads::notification_center::{
    NotificationCenter, NotificationTone, Presentation, ThreadNotification, presentation,
};
use t3_client::thread_views::threads::notifications::{
    BackgroundMemory, BackgroundThread, next_background_announcement, resolve_background_request,
};
use t3_client::types::{EnvironmentId, SyncStatus, ThreadKey};
use t3_client::view_logic::session::text::thread_notification_tag;
use t3_contracts::settings::{ClientSettingsSchema, NotificationMode};

use crate::i18n::t;
use crate::runtime::rt;
use crate::state::nav::{Route, navigator};
use crate::ui::icon::{Icon, icon_with_class};
use crate::ui::toast::{Action, ThreadToastData, ToastOptions, Variant, toast_manager};

/// `startThreadNotifications` and `startBackgroundThreadAnnouncements` for the life of the app.
pub fn thread_notifications() -> impl IntoElement {
    let runtime = rt();
    let nav = navigator();
    let Some(client) = runtime.backend().client() else {
        // The demo backend has no settings or live environments to watch.
        return view().app_class(crate::theme::tw("hidden"));
    };
    let settings_service = client.settings();
    let settings = signal_with(settings_service.client_settings(), SignalEquals::Never);
    let watch = settings_service.clone();
    spawn(async move {
        let mut changes = watch.changes();
        while changes.next().await.is_some() {
            let _ = settings.try_set(watch.client_settings());
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

    let show: Rc<dyn Fn(Vec<ThreadNotification>)> = {
        let pending = pending.clone();
        Rc::new(move |shown: Vec<ThreadNotification>| {
            let current = settings.get_untracked();
            for notification in shown {
                present(&notification, &current, open_key(), &pending, open_thread);
            }
        })
    };

    // Feed every environment's live shell to the center; a non-live shell pauses its grace.
    let observe_center = center.clone();
    let observe_show = show.clone();
    let observe_background = background.clone();
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
        let mut shown = Vec::new();
        let mut listed = Vec::new();
        for environment in snapshot.environments.keys() {
            let shell = runtime.store.shell(environment.clone()).get();
            let live = t3_client::state::selectors::shell(&snapshot, environment)
                .is_some_and(|state| state.status == SyncStatus::Live);
            match (live, shell) {
                (true, Some(shell)) => {
                    if environments.contains(environment) {
                        shown.extend(center.observe(
                            environment,
                            &shell.0.threads,
                            &BTreeMap::new(),
                            &BTreeSet::new(),
                            now,
                        ));
                    }
                    listed.extend(
                        shell
                            .0
                            .threads
                            .iter()
                            .filter(|thread| thread.archived_at.is_none())
                            .map(|thread| {
                                let view = t3_client::services::models::present_thread_shell(
                                    environment.clone(),
                                    thread.clone(),
                                    None,
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
                                    snoozed: false,
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
                title: String::from(notification.title),
                description: Some(body.clone()),
                // The thread's title is content: shown as it is, never translated.
                description_content: Some(Rc::new(move || text(body.clone()).into_element())),
                action: Some(Action {
                    label: String::from("Open thread"),
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
