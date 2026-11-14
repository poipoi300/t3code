//! Port of `apps/web/src/components/Sidebar.tsx` (`SidebarThreadHeader` and the thread list).
//!
//! Membership is the client's `sidebar_threads`; sections are its port of `Sidebar.logic.ts`
//! (`resolve_thread_section`) and order its port of `threadSort.ts`. Card rows are the
//! pinned, active and working sections, slim rows the rest, as `renderThreadRow` decides.
//! Section markers, the working shelf, folders, search and drag are the SB-* packages'.

pub mod actions;
pub mod chrome;
pub mod drag;
pub mod projects;
pub mod row;
pub mod selection;
pub mod shelves;
pub mod snooze;
pub mod swipe;

use std::collections::BTreeMap;
use std::rc::Rc;

use nsh_core::prelude::*;
use t3_client::services::config::timestamp_ms;
use t3_client::state::selectors::sidebar_threads;
use t3_client::thread_views::threads::sort::{
    OrderedThread, resolve_thread_activity_timestamp, sort_active_threads, sort_pinned_threads,
};
use t3_client::types::{EnvironmentId, ThreadId, ThreadKey};
use t3_client::view_logic::shell::sidebar::{Section, resolve_thread_section};
use t3_contracts::orchestration_v2::{AppThreadSettledOverride, OrchestrationV2ThreadShell};
use t3_contracts::wire::WireDateTime;

use crate::i18n::t;
use crate::runtime::rt;
use crate::state::clock::now_ms;
use crate::state::nav::{Route, navigator};
use crate::theme::{Scope, palette, tw, tw_with};
use crate::ui::icon::{Icon, icon};

/// `renderThreadRow`'s variants the slice draws (`folder` rows come with folders).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowVariant {
    Card,
    Slim,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SidebarItem {
    pub key: ThreadKey,
    pub title: String,
    pub project: String,
    /// `resolveThreadActivityTimestamp`: what the row's relative time counts from.
    pub activity_at: Option<String>,
    pub variant: RowVariant,
}

impl SidebarItem {
    pub fn list_key(&self) -> SharedString {
        format!("{}:{}", self.key.environment.0, self.key.thread.0).into()
    }
}

fn date(date: &Option<Option<WireDateTime>>) -> Option<&str> {
    date.as_ref().and_then(Option::as_ref).map(|d| d.0.as_str())
}

fn parsed(value: &Option<Option<WireDateTime>>) -> Option<i64> {
    date(value).and_then(timestamp_ms)
}

/// One environment's rows: pinned, active, snoozed, settled, each in the web's order.
pub fn items_for(
    environment: &EnvironmentId,
    shell: Option<&t3_client::state::shell::ShellState>,
    now: i64,
) -> Vec<SidebarItem> {
    let Some(shell) = shell else {
        return Vec::new();
    };
    let Some(snapshot) = shell.snapshot.as_ref() else {
        return Vec::new();
    };
    let keyed: Vec<(ThreadKey, &OrchestrationV2ThreadShell)> = sidebar_threads(shell, None)
        .into_iter()
        .map(|thread| {
            let key = ThreadKey {
                environment: environment.clone(),
                thread: ThreadId(thread.id.0.clone()),
            };
            (key, thread)
        })
        .collect();
    let mut sections: [Vec<OrderedThread<'_>>; 5] = Default::default();
    let mut by_key = BTreeMap::new();
    for (key, thread) in &keyed {
        let section = resolve_thread_section(
            parsed(&thread.snoozed_until).is_some_and(|wake| wake > now),
            thread.settled_override == Some(AppThreadSettledOverride::Settled),
            thread.pinned_at.as_ref().is_some_and(Option::is_some),
        );
        let order_key = match section {
            Section::Pinned => &thread.pin_order_key,
            _ => &thread.active_order_key,
        };
        sections[section_index(section)].push(OrderedThread {
            key,
            created_at: timestamp_ms(&thread.created_at.0),
            unsettled_at: parsed(&thread.unsettled_at),
            order_key: order_key
                .as_ref()
                .and_then(Option::as_ref)
                .map(|k| k.0.as_str()),
        });
        by_key.insert(key, (*thread, section));
    }
    sort_pinned_threads(&mut sections[0]);
    sort_active_threads(&mut sections[1]);
    sections
        .iter()
        .flatten()
        .map(|ordered| {
            let (thread, section) = by_key[ordered.key];
            let runs = [
                date(&thread.latest_run_requested_at),
                date(&thread.latest_run_started_at),
                date(&thread.latest_run_completed_at),
            ];
            let activity_at = resolve_thread_activity_timestamp(
                thread.latest_user_message_at.as_ref().map(|d| d.0.as_str()),
                &runs,
                &thread.updated_at.0,
                &timestamp_ms,
            )
            .map(str::to_owned);
            SidebarItem {
                key: ordered.key.clone(),
                title: thread.title.clone(),
                project: snapshot
                    .projects
                    .iter()
                    .find(|p| p.id == thread.project_id)
                    .map(|p| p.title.0.clone())
                    .unwrap_or_default(),
                activity_at,
                variant: match section {
                    Section::Pinned | Section::Active | Section::Working => RowVariant::Card,
                    Section::Snoozed | Section::Settled => RowVariant::Slim,
                },
            }
        })
        .collect()
}

fn section_index(section: Section) -> usize {
    match section {
        Section::Pinned => 0,
        Section::Active => 1,
        Section::Working => 2,
        Section::Snoozed => 3,
        Section::Settled => 4,
    }
}

pub fn sidebar() -> impl IntoElement {
    let runtime = rt();
    let nav = navigator();
    let items: Memo<Rc<Vec<SidebarItem>>> = {
        let environments = runtime.environments_signal();
        let per_environment = computed(move || {
            environments.with(|list| list.iter().map(|e| e.id.clone()).collect::<Vec<_>>())
        });
        let shells = computed(move || {
            per_environment
                .get()
                .into_iter()
                .map(|environment| {
                    let memo_env = environment.clone();
                    // Untracked: a snooze wake re-sorts through SB-SNOOZE's timer, not the clock.
                    runtime.store.shell_state(environment, move |shell| {
                        items_for(&memo_env, shell, untracked(now_ms))
                    })
                })
                .collect::<Vec<_>>()
        });
        computed(move || Rc::new(shells.get().iter().flat_map(|memo| memo.get()).collect()))
    };
    let count = move || items.with(|i| i.len());
    let keys = move || items.with(|i| i.iter().map(SidebarItem::list_key).collect::<Vec<_>>());

    let list = virtual_list(count, move |at| {
        let item = computed(move || items.with(|i| i.get(at.index()).cloned()));
        row::thread_row(item, nav)
    })
    .keys(keys)
    .estimated_item_size(px(82.0))
    .content_padding_x(px(8.0))
    .content_padding_y(px(4.0))
    .flex_1()
    .test_id("sidebar-threads");

    view()
        .w_full()
        .h_full()
        .app_class(tw_with(Scope::Sidebar, "bg-sidebar text-foreground", &[]))
        .child(thread_header(move || nav.open(Route::NewThread)))
        .child(list)
}

/// `sidebar/SidebarThreadHeader.tsx` `SidebarThreadHeader`, without search results (SB-SEARCH)
/// and the project scope menu (SB-PROJECTS).
fn thread_header(on_new_thread: impl Fn() + 'static) -> impl IntoElement {
    let query = signal(String::new());
    let header = view()
        .app_class(tw("flex items-center gap-1"))
        .child(
            view()
                .app_class(tw_with(
                    Scope::Sidebar,
                    "flex h-8 min-w-0 flex-1 items-center gap-2 rounded-md px-2 py-1.5 text-sm font-medium text-sidebar-muted-foreground hover:bg-sidebar-row-hover hover:text-sidebar-foreground focus-within:bg-sidebar-row-hover focus-within:text-sidebar-foreground focus-within:ring-1 focus-within:ring-inset focus-within:ring-ring/70",
                    &[],
                ))
                .child(icon(Icon::Search, 16.0, move || palette().sidebar_icon))
                .child(
                    nsh_base::input()
                        .value(move || query.get())
                        .on_change(move |text: &str| query.set(text.to_owned()))
                        .placeholder(t("Search"))
                        .label(SharedString::from(t("Search threads")))
                        .app_class(tw_with(
                            Scope::Sidebar,
                            "min-w-0 flex-1 bg-transparent text-sm text-sidebar-foreground placeholder:text-sidebar-muted-foreground",
                            &[],
                        )),
                ),
        )
        .child(
            view()
                .app_class(tw("flex shrink-0 items-center"))
                .child(header_icon_button(Icon::FolderPlus, t("Add project"), || {}))
                .child(header_icon_button(
                    Icon::SquarePen,
                    t("New thread"),
                    on_new_thread,
                )),
        );
    // The `SidebarGroup` the web wraps it in.
    view()
        .app_class(tw(
            "relative flex w-full min-w-0 flex-col p-[var(--sidebar-content-inset)]",
        ))
        .child(header)
}

/// `SidebarHeaderIconButton`: a `Button size="icon-sm" variant="ghost-muted"` with its label.
fn header_icon_button(
    which: Icon,
    label: String,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    nsh_base::button()
        .accessibility_label(Some(SharedString::from(label)))
        .on_press(on_press)
        .app_class(tw_with(
            Scope::Sidebar,
            "flex items-center justify-center size-8 rounded-md text-sidebar-muted-foreground hover:bg-sidebar-row-hover hover:text-sidebar-foreground focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring",
            &[],
        ))
        .child(icon(which, 16.0, move || palette().sidebar_icon))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_are_the_active_section_newest_first_with_their_project() {
        use crate::runtime::backend::Backend;
        use crate::runtime::demo::{DEMO_PAIRING_URL, DemoBackend, FIRST_THREAD};
        use futures::executor::block_on;
        use std::time::Duration;
        use t3_client::ClientStore;
        use t3_client::connection::PairingInput;

        let store = ClientStore::default();
        let backend = DemoBackend::with_chunk_delay(store.clone(), Duration::ZERO);
        let environment = block_on(backend.pair(PairingInput {
            pairing_url: Some(DEMO_PAIRING_URL.into()),
            ..PairingInput::default()
        }))
        .expect("pairs");
        let snapshot = store.snapshot();
        let shell = t3_client::state::selectors::shell(&snapshot, &environment);
        let items = items_for(&environment, shell, 0);
        assert_eq!(items.len(), 9);
        assert!(items.iter().all(|i| i.variant == RowVariant::Card));
        assert_eq!(items[0].key.thread.0, FIRST_THREAD);
        assert_eq!(items[0].project, "t3code");
        assert_eq!(items[8].project, "nsh-spikes");
        assert!(items.iter().all(|i| i.activity_at.is_some()));
        let keys: std::collections::BTreeSet<String> =
            items.iter().map(|i| i.list_key().to_string()).collect();
        assert_eq!(keys.len(), items.len(), "keys are unique");
    }
}
