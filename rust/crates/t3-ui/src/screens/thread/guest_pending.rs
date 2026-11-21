//! Port of apps/web/src/components/chat/GuestShareControls.tsx GuestPendingMessages. A guest's
//! messages wait at the end of the timeline until the owner sends or dismisses each one; a sent
//! one then shows in the timeline as a normal user message with the guest's name.
//!
//! Differences from the web component, declared: guest attachments are not drawn
//! (`GuestMessageAttachments`) because the Rust server does not accept guest files yet, so a
//! pending message never carries any. The text drops `whitespace-pre-wrap` and
//! `[overflow-wrap:anywhere]`: native text keeps newlines and wraps at any width.

use std::{collections::BTreeSet, rc::Rc};

use futures::future::LocalBoxFuture;
use nsh_core::prelude::*;
use t3_client::thread_views::thread_ops::guest_share::GuestMessageDecision;
use t3_client::types::ThreadKey;
use t3_contracts::base_schemas::GuestMessageId;
use t3_contracts::guest_share::ThreadGuestMessageStatus;

use crate::i18n::{Text, Value, t, tr};
use crate::runtime::rt;
use crate::runtime::tasks::TaskOwner;
use crate::theme::tw;
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};
use crate::ui::toast::{ToastOptions, Variant, toast_manager};

/// Sends or dismisses one guest message; bound to the client in `app::provide_client_services`.
#[derive(Clone)]
pub struct GuestMessageResolver(
    #[allow(clippy::type_complexity)]
    pub  Rc<
        dyn Fn(
            ThreadKey,
            GuestMessageId,
            GuestMessageDecision,
        ) -> LocalBoxFuture<'static, Result<(), String>>,
    >,
);

fn failed(error: String) {
    toast_manager().add(ToastOptions {
        variant: Variant::Error,
        title: Text::from("Failed to resolve guest message"),
        description: Some(Text::content(error)),
        ..ToastOptions::default()
    });
}

/// The pending messages of `key`'s guest share. `can_operate` hides Send and Dismiss, as the
/// web's `canOperate` does, so a viewer without the operate scope only reads.
pub fn guest_pending_messages(key: ThreadKey, can_operate: Memo<bool>) -> AnyElement {
    let projection = rt().store.thread(key.clone());
    let resolver = use_context::<GuestMessageResolver>();
    let busy = signal(BTreeSet::<String>::new());
    let owner = TaskOwner::here();
    dynamic(move || {
        let (name, pending) = projection.with(|projection| {
            let share = projection
                .as_ref()
                .and_then(|projection| projection.thread.guest_share.as_ref())
                .and_then(Option::as_ref);
            (
                share
                    .and_then(|share| share.guest_name.as_ref())
                    .map_or_else(|| "Guest".to_owned(), |name| name.0.clone()),
                share
                    .map(|share| {
                        share
                            .messages
                            .iter()
                            .filter(|message| message.status == ThreadGuestMessageStatus::Pending)
                            .map(|message| (message.id.clone(), message.text.clone()))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default(),
            )
        });
        if pending.is_empty() {
            return None;
        }
        let operate = can_operate.get() && resolver.is_some();
        let busy_now = busy.get();
        let key = key.clone();
        let resolver = resolver.clone();
        Some(untracked(move || {
            let count = pending.len();
            let mut list = view()
                .app_class(tw("flex flex-col gap-4 pb-4"))
                .role(AccessibilityRole::List)
                .label(SharedString::from(if count == 1 {
                    t("1 guest message waiting")
                } else {
                    tr(
                        "{count} guest messages waiting",
                        &[("count", Value::Int(count as i64))],
                    )
                }));
            for (id, body) in pending {
                let disabled = busy_now.contains(&id.0);
                let name_text = || Value::Text(name.clone());
                let resolve = |decision: GuestMessageDecision| {
                    let (key, id, resolver) = (key.clone(), id.clone(), resolver.clone());
                    move || {
                        let Some(resolver) = resolver.clone() else {
                            return;
                        };
                        // A second press before the list redraws must not send a second command.
                        if busy.with_untracked(|ids| ids.contains(&id.0)) {
                            return;
                        }
                        busy.update(|ids| {
                            ids.insert(id.0.clone());
                        });
                        let request = (resolver.0)(key.clone(), id.clone(), decision);
                        let id = id.clone();
                        owner.spawn(async move {
                            let result = request.await;
                            busy.update(|ids| {
                                ids.remove(&id.0);
                            });
                            if let Err(error) = result {
                                failed(error);
                            }
                        });
                    }
                };
                let mut item = view()
                    .app_class(tw("flex flex-col items-end gap-1"))
                    .child(
                        text(name.clone()).app_class(tw("me-1 text-2xs text-guest-foreground")),
                    )
                    .child(
                        view()
                            .app_class(tw(
                                "flex min-w-0 max-w-[80%] flex-col gap-2 rounded-2xl border border-dashed border-guest/50 bg-transparent p-3 text-foreground",
                            ))
                            .child(text(body).app_class(tw(
                                "text-sm leading-relaxed",
                            ))),
                    );
                if operate {
                    item = item.child(
                        view()
                            .app_class(tw("flex items-center gap-1 pe-1"))
                            .child(button(
                                ButtonContent::Custom {
                                    label: tr(
                                        "Dismiss message from {name}",
                                        &[("name", name_text())],
                                    ),
                                    child: text(t("Dismiss")).into_element(),
                                },
                                ButtonVariant::GhostMuted,
                                ButtonSize::Xs,
                                move || disabled,
                                resolve(GuestMessageDecision::Dismiss),
                            ))
                            .child(button(
                                ButtonContent::Custom {
                                    label: tr(
                                        "Send message from {name}",
                                        &[("name", name_text())],
                                    ),
                                    child: text(t("Send")).into_element(),
                                },
                                ButtonVariant::Default,
                                ButtonSize::Xs,
                                move || disabled,
                                resolve(GuestMessageDecision::Send),
                            )),
                    );
                }
                list = list.child(item);
            }
            list.into_element()
        }))
    })
    .into_element()
}
