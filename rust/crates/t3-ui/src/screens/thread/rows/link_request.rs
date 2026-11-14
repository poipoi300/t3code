//! Port of apps/web/src/components/chat/LinkRequestCard.tsx LinkRequestCard and
//! PendingLinkRequest. Rules are the client's (`t3_client::thread_views::link_request`); answers go
//! through `peerLinks.answerRequest`, gated by its permission as the web's `permissionAtom`.
//!
//! Not built yet: LinkRequestDialog (machine step, access step, pairing code), so the pending card
//! offers Decline and not "Review link…" / "Choose machine…" (hidden, as a control without its
//! dialog).

use std::rc::Rc;

use futures::future::LocalBoxFuture;
use nsh_core::prelude::*;
use t3_client::thread_views::link_request::{
    LINK_REQUEST_FAILURE, LinkAnswerError, LinkRequestDisplay, access_label,
    link_request_decline_input, link_request_display, link_request_picks_machine,
    link_request_target_name,
};
use t3_client::types::EnvironmentId;
use t3_contracts::orchestration_v2::{ProjectedTurnItemVisibility, TurnItemLinkRequest};
use t3_contracts::peer_link::PeerLinkRequestAnswerInput;

use super::secret_request::answered_row;
use crate::i18n::{Text, Value, t, tr};
use crate::theme::tw;
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};
use crate::ui::icon::Icon;

/// `peerLinkEnvironment.answerRequest` and its permission.
pub trait LinkRequestClient {
    fn can_answer(&self, environment: &EnvironmentId, input: &PeerLinkRequestAnswerInput) -> bool;
    fn answer(
        &self,
        environment: EnvironmentId,
        input: PeerLinkRequestAnswerInput,
    ) -> LocalBoxFuture<'static, Result<(), LinkAnswerError>>;
}

/// Overrides the host client's service (tests); screens bind from `Backend::client()`.
#[derive(Clone)]
pub struct LinkRequestService(pub Rc<dyn LinkRequestClient>);

const ROW_ID: &str = "data-v2-item-type-link_request";

fn target_name(item: &TurnItemLinkRequest) -> String {
    link_request_target_name(item).unwrap_or_else(|| t("a machine"))
}

pub fn link_request_card(
    environment: EnvironmentId,
    item: Rc<TurnItemLinkRequest>,
    visibility: &ProjectedTurnItemVisibility,
    service: Option<LinkRequestService>,
) -> AnyElement {
    match link_request_display(&item, visibility) {
        LinkRequestDisplay::Pending => pending_link_request(environment, item, service),
        LinkRequestDisplay::PendingElsewhere => answered_row(
            ROW_ID,
            Icon::Link,
            t("Waiting for an answer in the original thread"),
        ),
        LinkRequestDisplay::Linked { target, access } => {
            let target = target.unwrap_or_else(|| t("a machine"));
            let linked = tr("Linked {target}", &[("target", Value::Text(target))]);
            let label = match access {
                Some(access) => format!(
                    "{linked} · {}",
                    Text::client_key(access_label(&access)).resolve()
                ),
                None => linked,
            };
            answered_row(ROW_ID, Icon::Check, label)
        }
        LinkRequestDisplay::Declined => answered_row(ROW_ID, Icon::Minus, t("Declined")),
        LinkRequestDisplay::Failed { failure } => answered_row(
            ROW_ID,
            Icon::X,
            tr(
                "Could not link: {failure}",
                &[(
                    "failure",
                    Value::Text(failure.unwrap_or_else(|| t("the other environment refused"))),
                )],
            ),
        ),
        LinkRequestDisplay::Ended => answered_row(ROW_ID, Icon::Minus, t("Request ended")),
    }
}

fn pending_link_request(
    environment: EnvironmentId,
    item: Rc<TurnItemLinkRequest>,
    service: Option<LinkRequestService>,
) -> AnyElement {
    let declining = signal(false);
    let error = signal(None::<String>);
    let input = link_request_decline_input(&item);
    let can_answer = match (&service, &input) {
        (Some(service), Some(input)) => service.0.can_answer(&environment, input),
        _ => false,
    };
    let owner = Owner::current();
    let decline = move || {
        let Some(input) = input.clone() else {
            return;
        };
        declining.set(true);
        error.set(None);
        let answer = service
            .as_ref()
            .map(|service| service.0.answer(environment.clone(), input));
        owner.run(|| {
            spawn(async move {
                let result = match answer {
                    Some(answer) => answer.await,
                    None => Err(LinkAnswerError::Failed(LINK_REQUEST_FAILURE.into())),
                };
                let _ = declining.try_set(false);
                if let Err(LinkAnswerError::Failed(message)) = result {
                    let _ = error.try_set(Some(message));
                }
            });
        });
    };
    let title = if link_request_picks_machine(&item) {
        t("Link a machine to this environment")
    } else {
        tr(
            "Link {target} to this environment",
            &[("target", Value::Text(target_name(&item)))],
        )
    };
    let reason = item.reason.clone();
    view()
        .test_id(ROW_ID)
        .app_class(tw(
            "flex min-w-0 flex-col gap-3 rounded-xl border border-border/60 bg-card p-4",
        ))
        .child(
            view()
                .app_class(tw("flex min-w-0 flex-col gap-1"))
                .child(
                    text(SharedString::from(title))
                        .app_class(tw("text-sm font-medium text-foreground")),
                )
                .child((!t3_contracts::wire::trim_js(&reason).is_empty()).then(|| {
                    text(SharedString::from(reason.clone()))
                        .app_class(tw("text-sm text-muted-foreground"))
                })),
        )
        .child(dynamic(move || {
            error.get().map(|message| {
                text(Text::client_key(message).resolve()).app_class(tw("text-xs text-destructive"))
            })
        }))
        .child(if can_answer {
            view()
                .app_class(tw("flex min-w-0 items-center justify-end gap-2"))
                .child(button(
                    ButtonContent::Label(t("Decline")),
                    ButtonVariant::GhostMuted,
                    ButtonSize::Xs,
                    move || declining.get(),
                    decline,
                ))
                .into_element()
        } else {
            text(t(
                "Only a session that manages access here can answer this.",
            ))
            .app_class(tw("text-xs text-muted-foreground"))
            .into_element()
        })
        .into_element()
}
