//! Port of apps/web/src/components/guest/GuestPage.tsx (standalone mount and access notices).

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use futures::StreamExt as _;
use nsh_core::AccessibilityState;
use nsh_core::prelude::*;
use t3_client::composition::{CommandOutputLease, Spawner};
use t3_client::guest::session::{
    GuestClient, GuestCredential, GuestEnd, GuestHistoryState, GuestPhase, SubmitRefusal,
    TranscriptRole, TranscriptRow, plain_command_output,
};
use t3_client::guest::transport::{GuestFailure, GuestTransport};
use t3_contracts::guest_share::{
    GUEST_PENDING_MESSAGE_LIMIT, ThreadGuestMessage, ThreadGuestMessageStatus,
};

use crate::i18n::{self, t, tr};
use crate::runtime::tasks::{TaskOwner, watch};
use crate::theme::{self, tw};
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};
use crate::ui::icon::{Icon, icon_with_class};
use crate::ui::input::{InputSize, text_field};
use crate::ui::scroll_area::scroll_area;
pub(crate) use t3_client::guest::GuestAddress;

/// Builds the page's transport. The host installs the browser's same-origin one; tests install a
/// scripted one. Without a factory the page cannot reach any server and says so.
pub type TransportFactory = Arc<dyn Fn() -> Rc<dyn GuestTransport> + Send + Sync>;

#[derive(Default)]
struct Transports {
    default: Option<TransportFactory>,
    by_share: std::collections::HashMap<String, TransportFactory>,
}

static TRANSPORTS: Mutex<Option<Transports>> = Mutex::new(None);

/// The transport for every share, installed once by the host (the browser's same-origin one).
pub fn install_transport(factory: TransportFactory) {
    if let Ok(mut slots) = TRANSPORTS.lock() {
        slots.get_or_insert_with(Transports::default).default = Some(factory);
    }
}

/// A transport for one share only; headless tests use it so parallel tests do not collide.
pub fn install_transport_for(share: &str, factory: TransportFactory) {
    if let Ok(mut slots) = TRANSPORTS.lock() {
        slots
            .get_or_insert_with(Transports::default)
            .by_share
            .insert(share.to_owned(), factory);
    }
}

fn transport(share: &str) -> Option<Rc<dyn GuestTransport>> {
    let slots = TRANSPORTS.lock().ok()?;
    let slots = slots.as_ref()?;
    let factory = slots.by_share.get(share).or(slots.default.as_ref())?;
    Some(factory())
}

pub(crate) fn boot(mut location: Option<GuestAddress>) -> AnyElement {
    // The browser reaches its own origin with its own cookie jar; nothing else can serve a guest.
    #[cfg(target_arch = "wasm32")]
    install_transport(Arc::new(t3_platform::guest::browser_transport));
    i18n::set_language("system", &[locale().to_string()]);
    if let Some(location) = location.as_mut()
        && location.share_id().is_some()
        && let Some(share) = location.share_id().map(str::to_owned)
        && let Some(transport) = transport(&share)
    {
        let credential = location.take_credential().map(GuestCredential::new);
        replace_address(location);
        return theme::with_saved_theme(move || {
            view()
                .app_class(tw("flex h-full flex-col bg-background text-foreground"))
                .child(live_page(transport, share, credential))
                .into_element()
        });
    }
    let has_share = signal(
        location
            .as_ref()
            .is_some_and(|url| url.share_id().is_some()),
    );
    if let Some(location) = location {
        watch(async move {
            has_share.set(replace_address(&location));
            loop {
                let change = next_app_url_change().await;
                if let Some(next) = GuestAddress::parse(&change.url) {
                    let has_next_share = if change.url.contains('#') {
                        replace_address(&next)
                    } else {
                        next.share_id().is_some()
                    };
                    has_share.set(has_next_share);
                }
            }
        });
    }
    theme::with_saved_theme(move || {
        view()
            .app_class(tw("flex h-full flex-col bg-background text-foreground"))
            .child(dynamic(move || {
                if has_share.get() {
                    notice("Guest sharing is unavailable.", None)
                } else {
                    invalid_link()
                }
            }))
            .into_element()
    })
}

fn replace_address(location: &GuestAddress) -> bool {
    if set_app_url(location.history_path()).is_ok() {
        return location.share_id().is_some();
    }
    // NSH caps address length, and encoding a malformed id may exceed the cap.
    let _ = set_app_url("/guest");
    false
}

pub fn screen() -> AnyElement {
    invalid_link()
}

fn invalid_link() -> AnyElement {
    notice(
        "This link has ended or is not valid",
        Some("Ask the owner for a new link. Each link works in one browser."),
    )
}

fn notice(title: &'static str, detail: Option<&'static str>) -> AnyElement {
    let mut body = view()
        .app_class(tw("m-auto flex max-w-sm flex-col gap-1 p-6 text-center"))
        .child(text(move || t(title)).app_class(tw("text-sm font-medium")));
    if let Some(detail) = detail {
        body = body.child(text(move || t(detail)).app_class(tw("text-xs text-muted-foreground")));
    }
    body.into_element()
}

fn live_page(
    transport: Rc<dyn GuestTransport>,
    share: String,
    credential: Option<GuestCredential>,
) -> AnyElement {
    #[derive(Clone, PartialEq, Eq)]
    enum ScreenKind {
        Checking,
        NeedsName { error: Option<String> },
        Thread,
        Ended(GuestEnd),
    }

    let phase = signal(GuestPhase::Checking);
    let rows = signal(Vec::<TranscriptRow>::new());
    let title = signal(String::new());
    let messages = signal(Vec::<ThreadGuestMessage>::new());
    let history = signal(GuestHistoryState::default());
    let client = GuestClient::new(transport, &share, credential, move |state| {
        phase.set(state.phase.clone());
        rows.set(state.transcript());
        title.set(
            state
                .view
                .as_ref()
                .map(|view| view.title.clone())
                .unwrap_or_default(),
        );
        messages.set(
            state
                .view
                .as_ref()
                .map(|view| view.messages.clone())
                .unwrap_or_default(),
        );
        history.set(state.history());
    });
    let owner = TaskOwner::here();
    let boot_client = client.clone();
    watch(async move {
        boot_client.resume().await;
    });
    let screen = computed(move || match phase.get() {
        GuestPhase::Checking | GuestPhase::Joining | GuestPhase::Connecting => ScreenKind::Checking,
        GuestPhase::NeedsName { error } => ScreenKind::NeedsName { error },
        GuestPhase::Live | GuestPhase::Reconnecting => ScreenKind::Thread,
        GuestPhase::Ended(end) => ScreenKind::Ended(end),
    });
    let reconnecting = computed(move || phase.get() == GuestPhase::Reconnecting);
    dynamic(move || match screen.get() {
        ScreenKind::Checking => notice("Connecting...", None),
        ScreenKind::NeedsName { error } => join_form(client.clone(), owner, error),
        ScreenKind::Thread => thread_view(
            client.clone(),
            owner,
            title,
            rows,
            messages,
            history,
            reconnecting,
        ),
        ScreenKind::Ended(GuestEnd::Invalid) => invalid_link(),
        ScreenKind::Ended(GuestEnd::Used) => notice(
            "This link was already used",
            Some("Ask the owner for a new link. Each link works in one browser."),
        ),
        ScreenKind::Ended(GuestEnd::Inactive) => notice(
            "This link has ended",
            Some("The owner stopped sharing this thread."),
        ),
    })
    .into_element()
}

fn join_form(client: GuestClient, owner: TaskOwner, error: Option<String>) -> AnyElement {
    let name = signal(String::new());
    let join = move || {
        let value = name.get_untracked();
        if value.trim().is_empty() {
            return;
        }
        let client = client.clone();
        owner.spawn(async move {
            client.join(&value).await;
            if client.snapshot(|state| state.phase == GuestPhase::Connecting) {
                client.run().await;
            }
        });
    };
    let mut form = view()
        .app_class(tw("m-auto flex w-full max-w-xs flex-col gap-3 p-6"))
        .child(text(t("Join a shared thread")).app_class(tw("text-base font-medium")))
        .child(
            text(t("Your name"))
                .app_class(tw("text-sm"))
                .accessible(false),
        )
        .child(text_field(
            name,
            String::new(),
            t("Your name"),
            InputSize::Default,
            join.clone(),
        ));
    if let Some(error) = error {
        form = form.child(text(move || error.clone()).app_class(tw("text-sm text-destructive")));
    }
    form.child(button(
        ButtonContent::Label(t("Join")),
        ButtonVariant::Default,
        ButtonSize::Default,
        move || name.get().trim().is_empty(),
        join,
    ))
    .child(
        text(t(
            "The owner sees your name and approves each message before the agent gets it.",
        ))
        .app_class(tw("text-xs text-muted-foreground")),
    )
    .into_element()
}

fn status_label(status: &ThreadGuestMessageStatus) -> &'static str {
    match status {
        ThreadGuestMessageStatus::Pending => "Waiting for approval",
        ThreadGuestMessageStatus::Sent => "Sent",
        ThreadGuestMessageStatus::Dismissed => "Not sent",
    }
}

struct ExpandedCommand {
    owner: Owner,
    lease: CommandOutputLease,
    revision: Signal<u64>,
    alive: Rc<std::cell::Cell<bool>>,
}

impl Drop for ExpandedCommand {
    fn drop(&mut self) {
        // Free the per-expand signal and cancel row-owned driver/pump tasks on collapse or teardown.
        self.alive.set(false);
        self.owner.dispose();
    }
}

fn thread_view(
    client: GuestClient,
    owner: TaskOwner,
    title: Signal<String>,
    rows: Signal<Vec<TranscriptRow>>,
    messages: Signal<Vec<ThreadGuestMessage>>,
    history: Signal<GuestHistoryState>,
    reconnecting: Memo<bool>,
) -> AnyElement {
    let draft = signal(String::new());
    let problem = signal(None::<String>);
    let expanded_commands = signal(BTreeSet::<String>::new());
    let command_leases: Stored<BTreeMap<String, ExpandedCommand>> = stored(BTreeMap::new());
    let send = {
        let client = client.clone();
        move || {
            let text = draft.get_untracked();
            let client = client.clone();
            owner.spawn(async move {
                match client.submit(&text).await {
                    Ok(()) => {
                        draft.set(String::new());
                        problem.set(None);
                    }
                    Err(refusal) => problem.set(Some(refusal_text(&refusal))),
                }
            });
        }
    };
    let toggle_command = {
        let client = client.clone();
        move |item_id: &str| {
            let already_expanded = expanded_commands.with(|set| set.contains(item_id));
            if already_expanded {
                command_leases.with(|map| {
                    map.remove(item_id);
                });
                expanded_commands.update(|set| {
                    set.remove(item_id);
                });
            } else {
                let (row_owner, (revision, row_tasks)) =
                    nsh_core::element::create_root(|| (signal(0u64), TaskOwner::here()));
                let alive = Rc::new(std::cell::Cell::new(true));
                let pump_alive = alive.clone();
                let row_spawner: Spawner = Rc::new(move |future| row_tasks.spawn(future));
                let lease = client.command_output(item_id, &row_spawner);
                let mut changes = lease.changes();
                row_tasks.spawn(async move {
                    while changes.next().await.is_some() {
                        if !pump_alive.get() {
                            break;
                        }
                        revision.update(|count| *count = count.saturating_add(1));
                    }
                });
                command_leases.with(|map| {
                    map.insert(
                        item_id.to_owned(),
                        ExpandedCommand {
                            owner: row_owner,
                            lease,
                            revision,
                            alive,
                        },
                    );
                });
                expanded_commands.update(|set| {
                    set.insert(item_id.to_owned());
                });
            }
        }
    };
    let at_limit = move || {
        messages.with(|all| {
            all.iter()
                .filter(|message| message.status == ThreadGuestMessageStatus::Pending)
                .count()
                >= GUEST_PENDING_MESSAGE_LIMIT
        })
    };
    view()
        .app_class(tw("flex h-full min-h-0 flex-1 flex-col"))
        .child(
            view()
                .app_class(tw("flex items-center gap-2 border-b px-4 py-2"))
                .child(
                    text(move || title.get())
                        .app_class(tw("min-w-0 flex-1 truncate text-sm font-medium")),
                )
                .child(when(
                    move || reconnecting.get(),
                    || text(t("Reconnecting...")).app_class(tw("text-xs text-muted-foreground")),
                )),
        )
        .child(view().app_class(tw("min-h-0 flex-1")).child(scroll_area(
            false,
            dynamic(move || {
                let mut column = view().app_class(tw("flex flex-col gap-3 p-4"));
                let current_history = history.get();
                if current_history.has_more || current_history.error.is_some() {
                    let history_client = client.clone();
                    let label = if current_history.loading {
                        t("Loading earlier turns…")
                    } else {
                        t("Load earlier turns")
                    };
                    let mut history_box = view()
                        .app_class(tw("flex flex-col items-center gap-1"))
                        .child(button(
                            ButtonContent::Label(label),
                            ButtonVariant::Outline,
                            ButtonSize::Sm,
                            move || history.with(|state| state.loading),
                            move || {
                                let history_client = history_client.clone();
                                owner.spawn(async move {
                                    history_client.load_earlier().await;
                                });
                            },
                        ));
                    if let Some(error) = current_history.error {
                        history_box = history_box
                            .child(text(error).app_class(tw("text-xs text-destructive")));
                    }
                    column = column.child(history_box);
                }
                for row in rows.get() {
                    if row.role == TranscriptRole::Command {
                        column = column.child(command_row_view(
                            &row,
                            expanded_commands,
                            command_leases,
                            toggle_command.clone(),
                        ));
                        continue;
                    }
                    let mine = row.role == TranscriptRole::Person;
                    column = column.child(
                        view()
                            .app_class(tw(if mine {
                                "flex flex-col items-end gap-1"
                            } else {
                                "flex flex-col items-start gap-1"
                            }))
                            .child(text(row.text).app_class(tw("max-w-prose text-sm"))),
                    );
                }
                for message in messages.get() {
                    if message.status == ThreadGuestMessageStatus::Sent {
                        continue;
                    }
                    column = column.child(
                        view()
                            .app_class(tw("flex flex-col items-end gap-1"))
                            .child(text(message.text.clone()).app_class(tw("max-w-prose text-sm")))
                            .child(
                                text(t(status_label(&message.status)))
                                    .app_class(tw("text-2xs text-muted-foreground")),
                            ),
                    );
                }
                column
            }),
        )))
        .child(
            view()
                .app_class(tw("flex flex-col gap-2 border-t p-3"))
                .child(text_field(
                    draft,
                    t("Message"),
                    t("Message"),
                    InputSize::Default,
                    send.clone(),
                ))
                .child(
                    view()
                        .app_class(tw("flex items-center gap-3"))
                        .child(dynamic(move || match problem.get() {
                            Some(message) => text(message)
                                .app_class(tw("min-w-0 flex-1 text-xs text-destructive"))
                                .into_element(),
                            None => text(t("The owner approves each message"))
                                .app_class(tw(
                                    "min-w-0 flex-1 truncate text-xs text-muted-foreground",
                                ))
                                .into_element(),
                        }))
                        .child(button(
                            ButtonContent::Label(t("Send")),
                            ButtonVariant::Default,
                            ButtonSize::Sm,
                            move || draft.get().trim().is_empty() || at_limit(),
                            send,
                        )),
                ),
        )
        .into_element()
}

fn command_row_view(
    row: &TranscriptRow,
    expanded_commands: Signal<BTreeSet<String>>,
    command_leases: Stored<BTreeMap<String, ExpandedCommand>>,
    toggle: impl Fn(&str) + Clone + 'static,
) -> AnyElement {
    let item_id = row.id.clone();
    let is_expanded = expanded_commands.with(|set| set.contains(&item_id));
    let command_label = row.text.clone();
    let toggle_id = item_id.clone();

    let mut header = view()
        .role(AccessibilityRole::Button)
        .label(SharedString::from(command_label.clone()))
        .accessibility_state(AccessibilityState {
            expanded: Some(is_expanded),
            ..Default::default()
        })
        .app_class(tw(
            "flex w-full cursor-pointer items-center gap-2 py-1 text-left text-xs text-muted-foreground hover:text-foreground",
        ))
        .on_press(move || toggle(&toggle_id))
        .child(icon_with_class(Icon::Terminal, "size-3.5 shrink-0"))
        .child(
            text(command_label).app_class(tw("min-w-0 flex-1 truncate font-mono text-xs")),
        );
    if let Some(code) = row.exit_code.filter(|code| *code != 0) {
        header = header.child(
            text(tr("exit {code}", &[("code", i18n::Value::Int(code))]))
                .app_class(tw("shrink-0 text-2xs text-destructive")),
        );
    }
    header = header.child(icon_with_class(
        if is_expanded {
            Icon::ChevronDown
        } else {
            Icon::ChevronRight
        },
        "size-3.5 shrink-0",
    ));

    let mut container = view()
        .app_class(tw("flex w-full flex-col gap-1"))
        .child(header);

    if is_expanded
        && let Some(revision) =
            command_leases.with(|map| map.get(&item_id).map(|entry| entry.revision))
    {
        let body_id = item_id.clone();
        container = container.child(dynamic(move || {
            let _ = revision.get();
            command_leases.with(|map| {
                let Some(entry) = map.get(&body_id) else {
                    return view().into_element();
                };
                command_output_panel(&entry.lease)
            })
        }));
    }

    container.into_element()
}

fn command_output_panel(lease: &CommandOutputLease) -> AnyElement {
    let delta = lease.read(None);
    let output_text = lease.text();
    let error = lease.error();
    let truncated = delta.as_ref().is_some_and(|frame| frame.truncated);
    let running = delta.as_ref().is_some_and(|frame| frame.running);

    let mut panel = view().app_class(tw(
        "ml-5 flex flex-col gap-1 border-l border-border/60 py-1 pl-3",
    ));
    if let Some(error) = error {
        let message = if error == "Output is no longer available." {
            t("Output is no longer available.")
        } else {
            tr(
                "Couldn't load output: {error}",
                &[("error", i18n::Value::Text(error))],
            )
        };
        return panel
            .child(text(message).app_class(tw("text-xs text-destructive")))
            .into_element();
    }
    if delta.is_none() {
        return panel
            .child(text(t("Loading output…")).app_class(tw("text-xs text-muted-foreground")))
            .into_element();
    }
    if truncated {
        panel = panel.child(
            text(t("Earlier output not shown")).app_class(tw("text-2xs text-muted-foreground")),
        );
    }
    let rendered = output_text
        .map(|raw| plain_command_output(&raw))
        .unwrap_or_default();
    if rendered.is_empty() {
        let label = if running {
            t("Loading output…")
        } else {
            t("No output.")
        };
        panel = panel.child(text(label).app_class(tw("text-xs text-muted-foreground")));
    } else {
        panel = panel.child(text(rendered).select_text().app_class(tw(
            "cursor-text font-mono text-2xs leading-relaxed text-muted-foreground select-text",
        )));
    }
    panel.into_element()
}

fn refusal_text(refusal: &SubmitRefusal) -> String {
    match refusal {
        SubmitRefusal::Empty => t("Write a message or attach a file."),
        SubmitRefusal::TooLong => t("Messages can be up to 4,000 characters."),
        SubmitRefusal::TooManyPending => t("Wait for the owner to answer your earlier messages."),
        SubmitRefusal::NotLive => t("Connecting..."),
        SubmitRefusal::Network => t("Could not reach the server."),
        SubmitRefusal::Server(GuestFailure::Busy, _) => {
            t("Wait for the owner to answer your earlier messages.")
        }
        SubmitRefusal::Server(_, message) => message.clone(),
    }
}
