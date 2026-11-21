//! Port of apps/web/src/components/guest/GuestPage.tsx (standalone mount and access notices).

use std::rc::Rc;
use std::sync::{Arc, Mutex};

use nsh_core::prelude::*;
use t3_client::guest::session::{
    GuestClient, GuestCredential, GuestEnd, GuestPhase, SubmitRefusal, TranscriptRole,
    TranscriptRow,
};
use t3_client::guest::transport::{GuestFailure, GuestTransport};
use t3_contracts::guest_share::{
    GUEST_PENDING_MESSAGE_LIMIT, ThreadGuestMessage, ThreadGuestMessageStatus,
};

use crate::i18n::{self, t};
use crate::runtime::tasks::{TaskOwner, watch};
use crate::theme::{self, tw};
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};
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
    let phase = signal(GuestPhase::Checking);
    let rows = signal(Vec::<TranscriptRow>::new());
    let title = signal(String::new());
    let messages = signal(Vec::<ThreadGuestMessage>::new());
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
    });
    let owner = TaskOwner::here();
    let boot_client = client.clone();
    watch(async move {
        boot_client.start().await;
        if boot_client.snapshot(|state| state.phase == GuestPhase::Connecting) {
            boot_client.run().await;
        }
    });
    dynamic(move || match phase.get() {
        GuestPhase::Checking | GuestPhase::Joining | GuestPhase::Connecting => {
            notice("Connecting...", None)
        }
        GuestPhase::NeedsName { error } => join_form(client.clone(), owner, error),
        GuestPhase::Live | GuestPhase::Reconnecting => thread_view(
            client.clone(),
            owner,
            title,
            rows,
            messages,
            phase.get() == GuestPhase::Reconnecting,
        ),
        GuestPhase::Ended(GuestEnd::Invalid) => invalid_link(),
        GuestPhase::Ended(GuestEnd::Used) => notice(
            "This link was already used",
            Some("Ask the owner for a new link. Each link works in one browser."),
        ),
        GuestPhase::Ended(GuestEnd::Inactive) => notice(
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

fn thread_view(
    client: GuestClient,
    owner: TaskOwner,
    title: Signal<String>,
    rows: Signal<Vec<TranscriptRow>>,
    messages: Signal<Vec<ThreadGuestMessage>>,
    reconnecting: bool,
) -> AnyElement {
    let draft = signal(String::new());
    let problem = signal(None::<String>);
    let send = move || {
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
                    move || reconnecting,
                    || text(t("Reconnecting...")).app_class(tw("text-xs text-muted-foreground")),
                )),
        )
        .child(view().app_class(tw("min-h-0 flex-1")).child(scroll_area(
            false,
            dynamic(move || {
                let mut column = view().app_class(tw("flex flex-col gap-3 p-4"));
                for row in rows.get() {
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
