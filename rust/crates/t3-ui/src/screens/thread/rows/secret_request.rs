//! Port of apps/web/src/components/chat/SecretRequestCard.tsx SecretRequestCard and
//! PendingSecretRequestForm. Display, link, answer and failure rules are the client's
//! (`t3_client::thread_views::secret_request`); the answer goes through `secrets.answerRequest`.
//!
//! Not ported: SecretSourceSelect and reference answers (behind the web's off-by-default
//! `T3CODE_FEATURE_SECRETS` flag), and the form's click-to-focus (NSH gives no event target).

use std::rc::Rc;

use futures::future::LocalBoxFuture;
use nsh_core::prelude::*;
use t3_client::thread_views::secret_request::{
    SECRET_REQUEST_DEFAULT_PLACEHOLDER, SecretAnswerError, SecretRequestDisplay,
    SecretRequestOutcome, SecretRequestReply, secret_request_answer_input, secret_request_display,
    secret_request_link, secret_request_privacy_note,
};
use t3_client::types::EnvironmentId;
use t3_contracts::orchestration_v2::{ProjectedTurnItemVisibility, TurnItemSecretRequest};
use t3_contracts::secret_request::SecretRequestAnswerInput;

use crate::i18n::{Text, Value, t, tr};
use crate::theme::{palette, tw};
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};
use crate::ui::icon::{Icon, icon};
use crate::ui::input::masked_text_field;

/// `serverEnvironment.answerSecretRequest`.
pub trait SecretRequestClient {
    fn answer(
        &self,
        environment: EnvironmentId,
        input: SecretRequestAnswerInput,
    ) -> LocalBoxFuture<'static, Result<(), SecretAnswerError>>;
}

/// Overrides the host client's service (tests); screens bind from `Backend::client()`.
#[derive(Clone)]
pub struct SecretRequestService(pub Rc<dyn SecretRequestClient>);

pub fn secret_request_card(
    environment: EnvironmentId,
    item: Rc<TurnItemSecretRequest>,
    visibility: &ProjectedTurnItemVisibility,
    service: Option<SecretRequestService>,
) -> AnyElement {
    match secret_request_display(&item.secret_status, visibility) {
        SecretRequestDisplay::Pending => {
            pending_secret_request_form(environment, item, service).into_element()
        }
        SecretRequestDisplay::PendingElsewhere { label } => answered_row(
            "data-v2-item-type-secret_request",
            Icon::Lock,
            format!("{} · {}", item.label.0, Text::client_key(label).resolve()),
        ),
        SecretRequestDisplay::Answered { outcome, label } => answered_row(
            "data-v2-item-type-secret_request",
            match outcome {
                SecretRequestOutcome::Saved => Icon::Check,
                _ => Icon::Minus,
            },
            format!("{} · {}", item.label.0, Text::client_key(label).resolve()),
        ),
    }
}

/// `<WorkLogRow icon label>` outside a work-log list, as WorkLog.tsx draws it.
pub(super) fn answered_row(id: &'static str, glyph: Icon, label: String) -> AnyElement {
    view()
        .test_id(id)
        .app_class(tw("group/timeline-row relative w-full min-w-0 rounded-md px-0.5 text-left transition-colors py-0.5"))
        .child(
            view()
                .app_class(tw("flex min-h-6 min-w-0 gap-1.5 text-sm leading-relaxed select-none items-center"))
                .child(
                    view()
                        .app_class(tw("flex min-w-0 flex-1 items-baseline gap-1.5"))
                        // NSH has no cap-relative baseline; the 24px icon slot as TL-ROWS-WORK-2 keeps it.
                        .child(view().app_class(tw("relative w-6 shrink-0 leading-0")).child(
                            view().app_class(tw("size-6 overflow-hidden")).child(
                                view()
                                    .app_class(tw("flex size-full items-center justify-center"))
                                    .child(icon(glyph, 14.0, || palette().muted_foreground)),
                            ),
                        ))
                        .child(
                            view()
                                .app_class(tw("min-w-0 flex-1 text-secondary-label truncate"))
                                .child(text(SharedString::from(label))),
                        ),
                ),
        )
        .into_element()
}

fn pending_secret_request_form(
    environment: EnvironmentId,
    item: Rc<TurnItemSecretRequest>,
    service: Option<SecretRequestService>,
) -> impl IntoElement {
    let secret = signal(String::new());
    let submitting = signal(false);
    let error = signal(None::<String>);
    let owner = Owner::current();
    let send = Rc::new({
        let item = item.clone();
        move |reply: SecretRequestReply| {
            // Enter and a click can both submit before a render.
            if submitting.get_untracked() {
                return;
            }
            let Some(input) = secret_request_answer_input(&item, reply) else {
                return;
            };
            submitting.set(true);
            error.set(None);
            let answer = service
                .as_ref()
                .map(|service| service.0.answer(environment.clone(), input));
            owner.run(|| {
                spawn(async move {
                    let result = match answer {
                        Some(answer) => answer.await,
                        None => Err(SecretAnswerError::Failed(
                            t3_client::thread_views::secret_request::SECRET_REQUEST_FAILURE.into(),
                        )),
                    };
                    let _ = submitting.try_set(false);
                    match result {
                        Ok(()) => {
                            let _ = secret.try_set(String::new());
                        }
                        Err(SecretAnswerError::Interrupted) => {}
                        Err(SecretAnswerError::Failed(message)) => {
                            let _ = error.try_set(Some(message));
                        }
                    }
                });
            });
        }
    });
    let save = {
        let send = send.clone();
        move || {
            send(SecretRequestReply::Save {
                secret: secret.get_untracked(),
            })
        }
    };
    let opener = crate::runtime::services::links::link_opener_here();
    let link = secret_request_link(item.url.clone().flatten().as_deref(), &item.reason);
    let placeholder = item
        .placeholder
        .clone()
        .flatten()
        .unwrap_or_else(|| t(SECRET_REQUEST_DEFAULT_PLACEHOLDER));
    let privacy = Text::client_key(secret_request_privacy_note(&item)).resolve();
    let reason = item.reason.clone();
    let url = item.url.clone().flatten();
    let decline = send.clone();
    view()
        .test_id("data-v2-item-type-secret_request")
        .app_class(tw(
            "flex min-w-0 flex-col gap-3 rounded-xl border border-border/60 bg-card p-4",
        ))
        .child(
            view()
                .app_class(tw("flex min-w-0 flex-col gap-1"))
                .child(
                    text(SharedString::from(item.label.0.clone()))
                        .app_class(tw("text-sm font-medium text-foreground")),
                )
                .child((!t3_contracts::wire::trim_js(&reason).is_empty()).then(|| {
                    text(SharedString::from(reason.clone()))
                        .app_class(tw("text-sm text-muted-foreground"))
                })),
        )
        .child(link.map(|link| {
            let href = link.href.clone();
            view()
                .app_class(tw("flex min-w-0 flex-col items-start gap-1.5"))
                .child(button(
                    ButtonContent::Custom {
                        label: tr("Open {host}", &[("host", Value::Text(link.host.clone()))]),
                        child: view()
                            .app_class(tw("flex items-center gap-2"))
                            .child(text(SharedString::from(tr(
                                "Open {host}",
                                &[("host", Value::Text(link.host.clone()))],
                            ))))
                            .child(icon(Icon::ExternalLink, 16.0, || palette().foreground))
                            .into_element(),
                    },
                    ButtonVariant::Outline,
                    ButtonSize::Sm,
                    || false,
                    move || opener.open(href.clone()),
                ))
                // A link found in the reason is already shown there.
                .child(url.clone().map(|url| {
                    text(SharedString::from(url))
                        .app_class(tw("font-mono text-xs text-muted-foreground"))
                }))
        }))
        .child(
            view()
                .app_class(tw("flex min-w-0 items-center gap-2"))
                .child(
                    view()
                        .app_class(tw("min-w-0 flex-1"))
                        .child(masked_text_field(
                            secret,
                            placeholder,
                            item.label.0.clone(),
                            move || submitting.get(),
                            save.clone(),
                        )),
                )
                .child(button(
                    ButtonContent::Label(t("Save securely")),
                    ButtonVariant::Default,
                    ButtonSize::Default,
                    move || {
                        submitting.get()
                            || secret.with(|s| t3_contracts::wire::trim_js(s).is_empty())
                    },
                    save,
                )),
        )
        .child(dynamic(move || {
            error.get().map(|message| {
                // A client sentence, or the server's own message for a known error.
                text(Text::client_key(message).resolve()).app_class(tw("text-xs text-destructive"))
            })
        }))
        .child(
            view()
                .app_class(tw("flex min-w-0 items-center justify-between gap-2"))
                .child(
                    view()
                        .app_class(tw(
                            "flex min-w-0 items-center gap-1.5 text-xs text-muted-foreground",
                        ))
                        .child(icon(Icon::ShieldCheck, 14.0, || palette().muted_foreground))
                        .child(text(privacy).app_class(tw("text-xs text-muted-foreground"))),
                )
                .child(button(
                    ButtonContent::Label(t("Decline")),
                    ButtonVariant::GhostMuted,
                    ButtonSize::Xs,
                    move || submitting.get(),
                    move || decline(SecretRequestReply::Decline),
                )),
        )
}
