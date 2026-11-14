//! Port of apps/web/src/components/chat/LinkRequestCard.tsx LinkRequestCard, PendingLinkRequest,
//! LinkRequestDialog and MachinePicker, with settings/LinkAccessPicker.tsx LinkAccessPicker and
//! settings/LinkMachineSelect.tsx LinkMachineBadge and LinkAddressWarning. Rules are the client's
//! (`t3_client::thread_views::link_request`); answers go through `peerLinks.answerRequest`, gated
//! by its permission as the web's `permissionAtom`.
//!
//! Departures: the pairing code is always pasted (the client cannot mint one on a connected
//! machine yet, the web's branch when it cannot mint); T3 Connect machines are not listed; access
//! options show their label (the selected one's description sits below, as the web's); machine
//! names are radio labels, which the radio primitive passes through its translator.

use std::rc::Rc;

use futures::future::LocalBoxFuture;
use nsh_core::prelude::*;
use t3_client::thread_views::link_request::{
    ANOTHER_ADDRESS, LINK_REQUEST_FAILURE, LinkAnswerError, LinkAnswerTarget, LinkRequestDisplay,
    LinkRequestReply, LinkRequestTarget, LinkableMachine, access_label, link_destination,
    link_request_answer_input, link_request_decline_input, link_request_default_access,
    link_request_display, link_request_initial_machine, link_request_picks_machine,
    link_request_target, link_request_target_name, rank_machines_by_hint,
};
use t3_client::types::EnvironmentId;
use t3_contracts::auth::AuthMcpClientAccess;
use t3_contracts::orchestration_v2::{ProjectedTurnItemVisibility, TurnItemLinkRequest};
use t3_contracts::peer_link::PeerLinkRequestAnswerInput;

use super::secret_request::answered_row;
use crate::i18n::{Text, Value, t, tr};
use crate::theme::tw;
use crate::ui::badge;
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};
use crate::ui::dialog::{PopupOptions, dialog};
use crate::ui::icon::Icon;
use crate::ui::input::{InputSize, text_field};
use crate::ui::wizard::{
    WizardSize, WizardStepsOptions, wizard_footer, wizard_header, wizard_panel, wizard_popup,
    wizard_steps,
};

/// `peerLinkEnvironment.answerRequest`, its permission, and the machine list.
pub trait LinkRequestClient {
    fn can_answer(&self, environment: &EnvironmentId, input: &PeerLinkRequestAnswerInput) -> bool;
    fn answer(
        &self,
        environment: EnvironmentId,
        input: PeerLinkRequestAnswerInput,
    ) -> LocalBoxFuture<'static, Result<(), LinkAnswerError>>;
    fn machines(&self, environment: EnvironmentId)
    -> LocalBoxFuture<'static, Vec<LinkableMachine>>;
}

/// Overrides the host client's service (tests); screens bind from `Backend::client()`.
#[derive(Clone)]
pub struct LinkRequestService(pub Rc<dyn LinkRequestClient>);

const ROW_ID: &str = "data-v2-item-type-link_request";

const ACCESSES: [AuthMcpClientAccess; 5] = [
    AuthMcpClientAccess::ReadOnly,
    AuthMcpClientAccess::ApprovalRequired,
    AuthMcpClientAccess::AutoAcceptEdits,
    AuthMcpClientAccess::Auto,
    AuthMcpClientAccess::FullAccess,
];

fn access_value(access: &AuthMcpClientAccess) -> &'static str {
    match access {
        AuthMcpClientAccess::ReadOnly => "read-only",
        AuthMcpClientAccess::ApprovalRequired => "approval-required",
        AuthMcpClientAccess::AutoAcceptEdits => "auto-accept-edits",
        AuthMcpClientAccess::Auto => "auto",
        AuthMcpClientAccess::FullAccess => "full-access",
    }
}

/// ConnectAgentSurface.tsx `accessConfig[access].description` (runtimeModeConfig.ts for modes).
fn access_description(access: &AuthMcpClientAccess) -> String {
    t(match access {
        AuthMcpClientAccess::ReadOnly => {
            "Read projects and threads. Cannot start, message or change anything."
        }
        AuthMcpClientAccess::ApprovalRequired => "Ask before commands and file changes.",
        AuthMcpClientAccess::AutoAcceptEdits => "Auto-approve edits, ask before other actions.",
        AuthMcpClientAccess::Auto => {
            "Supported providers approve routine actions; others still ask."
        }
        AuthMcpClientAccess::FullAccess => "Allow commands and edits without prompts.",
    })
}

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
    let open = signal(false);
    let declining = signal(false);
    let error = signal(None::<String>);
    let input = link_request_decline_input(&item);
    let can_answer = match (&service, &input) {
        (Some(service), Some(input)) => service.0.can_answer(&environment, input),
        _ => false,
    };
    let owner = Owner::current();
    let decline = {
        let service = service.clone();
        let environment = environment.clone();
        move || {
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
        }
    };
    let picks = link_request_picks_machine(&item);
    let title = if picks {
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
                .child(button(
                    ButtonContent::Label(if picks {
                        t("Choose machine…")
                    } else {
                        t("Review link…")
                    }),
                    ButtonVariant::Default,
                    ButtonSize::Xs,
                    move || declining.get(),
                    move || open.set(true),
                ))
                .into_element()
        } else {
            text(t(
                "Only a session that manages access here can answer this.",
            ))
            .app_class(tw("text-xs text-muted-foreground"))
            .into_element()
        })
        .child(service.map(|service| link_request_dialog(environment, item, open, service)))
        .into_element()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LinkStep {
    Machine,
    Access,
}

/// The dialog's derived state, recomputed from its inputs as the web component's render does.
#[derive(Clone, Debug, PartialEq)]
struct Plan {
    machines: Vec<LinkableMachine>,
    selected: Option<String>,
    target: LinkRequestTarget,
    target_name: String,
    needs_address: bool,
    asks_address: bool,
    use_existing: bool,
    steps: Vec<LinkStep>,
    current: LinkStep,
    machine_ready: bool,
    ready: bool,
}

impl Plan {
    fn finishes_here(&self) -> bool {
        self.current == LinkStep::Access || self.use_existing
    }
}

fn link_request_dialog(
    environment: EnvironmentId,
    item: Rc<TurnItemLinkRequest>,
    open: Signal<bool>,
    service: LinkRequestService,
) -> impl IntoElement {
    let pick_machine = link_request_picks_machine(&item);
    let listed = signal(Vec::<LinkableMachine>::new());
    let choice = signal(None::<String>);
    let initial_address = item
        .url
        .clone()
        .flatten()
        .map(|url| url.0)
        .unwrap_or_default();
    let address = signal(initial_address.clone());
    let default_access = link_request_default_access(&item);
    let access = signal(default_access);
    let pairing_code = signal(String::new());
    let step = signal(LinkStep::Machine);
    let busy = signal(false);
    let error = signal(None::<String>);
    let in_flight = signal(false);

    // The machine list is read when the dialog opens, as useLinkableMachines(environment, open).
    {
        let service = service.clone();
        let environment = environment.clone();
        effect(move || {
            if open.get() {
                let machines = service.0.machines(environment.clone());
                spawn(async move {
                    let machines = machines.await;
                    let _ = listed.try_set(machines);
                });
            }
        });
    }

    let plan_item = item.clone();
    let plan = computed(move || {
        let machines =
            rank_machines_by_hint(listed.get(), plan_item.hint.clone().flatten().as_deref());
        let selected = choice
            .get()
            .or_else(|| link_request_initial_machine(&plan_item, &machines));
        let target = link_request_target(&plan_item, &machines, selected.as_deref());
        let target_name = match &target {
            LinkRequestTarget::Machine { label, .. } => label.clone(),
            _ => target_name(&plan_item),
        };
        let needs_address = match &target {
            LinkRequestTarget::Address => true,
            LinkRequestTarget::Machine { urls, .. } => urls.is_empty(),
            LinkRequestTarget::None => false,
        };
        let asks_address = needs_address && plan_item.url.clone().flatten().is_none();
        let use_existing = matches!(target, LinkRequestTarget::Machine { linked: true, .. });
        let steps = if pick_machine || asks_address {
            vec![LinkStep::Machine, LinkStep::Access]
        } else {
            vec![LinkStep::Access]
        };
        let current = if steps.contains(&step.get()) {
            step.get()
        } else {
            LinkStep::Access
        };
        let machine_ready = target != LinkRequestTarget::None
            && (use_existing
                || !needs_address
                || !t3_contracts::wire::trim_js(&address.get()).is_empty());
        // No minting yet: every new link needs a pasted pairing code.
        let ready = machine_ready
            && (use_existing || !t3_contracts::wire::trim_js(&pairing_code.get()).is_empty());
        Plan {
            machines,
            selected,
            target,
            target_name,
            needs_address,
            asks_address,
            use_existing,
            steps,
            current,
            machine_ready,
            ready,
        }
    });

    // Each rebuilt region reads its own memo: a region that read the whole plan would be rebuilt
    // on every keystroke, recreating the field being typed in.
    let current_step = computed(move || plan.with(|p| p.current));
    let use_existing = computed(move || plan.with(|p| p.use_existing));
    let shows_steps = computed(move || plan.with(|p| p.steps.len() > 1 && !p.use_existing));
    let shows_back =
        computed(move || plan.with(|p| p.current == LinkStep::Access && p.steps.len() > 1));
    let target_label = computed(move || plan.with(|p| p.target_name.clone()));
    let primary_label = computed(move || {
        plan.with(|p| {
            if busy.get() {
                "Linking…"
            } else if p.use_existing {
                "Use this link"
            } else if p.finishes_here() {
                "Link"
            } else {
                "Continue"
            }
        })
    });

    // Every close resets, so a reopened dialog never shows the last answer or its pairing code.
    let reset = {
        move || {
            choice.set(None);
            address.set(initial_address.clone());
            access.set(default_access);
            pairing_code.set(String::new());
            step.set(LinkStep::Machine);
            error.set(None);
        }
    };
    let close = Rc::new({
        let reset = reset.clone();
        move || {
            open.set(false);
            reset();
        }
    });

    let owner = Owner::current();
    let submit = Rc::new({
        let item = item.clone();
        let close = close.clone();
        move || {
            let current = plan.get_untracked();
            if in_flight.get_untracked() || current.target == LinkRequestTarget::None {
                return;
            }
            let reply = match &current.target {
                LinkRequestTarget::Machine {
                    environment_id,
                    linked: true,
                    ..
                } => LinkRequestReply::UseExisting(environment_id.clone()),
                target => LinkRequestReply::Link {
                    target: match target {
                        LinkRequestTarget::Machine {
                            environment_id,
                            label,
                            urls,
                            ..
                        } if !urls.is_empty() => LinkAnswerTarget::Machine {
                            environment_id: environment_id.clone(),
                            label: label.clone(),
                            urls: urls.clone(),
                        },
                        _ => LinkAnswerTarget::Address(address.get_untracked()),
                    },
                    access: access.get_untracked(),
                    pairing_code: pairing_code.get_untracked(),
                },
            };
            let Some(input) = link_request_answer_input(&item, reply) else {
                return;
            };
            in_flight.set(true);
            error.set(None);
            let answer = service.0.answer(environment.clone(), input);
            let close = close.clone();
            owner.run(|| {
                spawn(async move {
                    // Set here, not in the press: "Linking…" rebuilds the pressed button.
                    let _ = busy.try_set(true);
                    let result = answer.await;
                    let _ = in_flight.try_set(false);
                    let _ = busy.try_set(false);
                    match result {
                        // The card switches to its answered row once the item updates.
                        Ok(()) => close(),
                        Err(LinkAnswerError::Interrupted) => {}
                        Err(LinkAnswerError::Failed(message)) => {
                            let _ = error.try_set(Some(message));
                        }
                    }
                });
            });
        }
    });
    // The footer's buttons are rebuilt when the step changes; NSH must not dispose a button inside
    // its own press handler, so step changes from them land on the next task.
    let advance = {
        let submit = submit.clone();
        move || {
            let current = plan.get_untracked();
            if current.finishes_here() {
                submit();
            } else if current.machine_ready {
                owner.run(|| {
                    spawn(async move {
                        let _ = step.try_set(LinkStep::Access);
                    })
                });
            }
        }
    };

    let title_item = item.clone();
    dialog(open, move |state| {
        {
            let close = close.clone();
            state.on_open_change(move |next| {
                // An answer that lands after a close would still link, and close a reopened dialog.
                if !next && busy.get_untracked() {
                    return;
                }
                if next {
                    open.set(true);
                } else {
                    close();
                }
            });
        }
        let close = close.clone();
        let advance = advance.clone();
        let title_item = title_item.clone();
        let item = item.clone();
        wizard_popup(
            state,
            WizardSize::Default,
            PopupOptions::default(),
            move || {
                let title_item = title_item.clone();
                let header = wizard_header(
                    state,
                    move || {
                        if pick_machine {
                            Text::from("Link a machine")
                        } else {
                            Text::content(tr(
                                "Link {target}",
                                &[("target", Value::Text(plan.with(|p| p.target_name.clone())))],
                            ))
                        }
                    },
                    Some(Rc::new(|| {
                        Text::from(
                            "Agents here can then launch, message and wait on threads there. Revoke it in that machine's Sharing & access.",
                        )
                    })),
                    None,
                    Some(
                        dynamic(move || {
                            shows_steps.get().then(|| {
                                untracked(|| {
                                    wizard_steps(
                                        || vec![Text::from("Machine"), Text::from("Access")],
                                        move || {
                                            plan.with(|p| {
                                                p.steps
                                                    .iter()
                                                    .position(|s| *s == p.current)
                                                    .unwrap_or(0)
                                            })
                                        },
                                        WizardStepsOptions {
                                            summaries: Some(
                                                (move || {
                                                    plan.with(|p| {
                                                        vec![
                                                            (p.target != LinkRequestTarget::None)
                                                                .then(|| {
                                                                    SharedString::from(
                                                                        p.target_name.clone(),
                                                                    )
                                                                }),
                                                            None,
                                                        ]
                                                    })
                                                })
                                                .into_reactive(),
                                            ),
                                            show_summaries: true,
                                            is_step_disabled: Some(Rc::new(move |index| {
                                                busy.get()
                                                    || (index == 1
                                                        && !plan.with(|p| p.machine_ready))
                                            })),
                                            on_step_change: Some(Rc::new(move |index| {
                                                let steps =
                                                    plan.with_untracked(|p| p.steps.clone());
                                                if let Some(next) = steps.get(index) {
                                                    step.set(*next);
                                                }
                                            })),
                                        },
                                    )
                                })
                            })
                        })
                        .into_element(),
                    ),
                );
                let _ = &title_item;
                let panel_item = item.clone();
                let panel_advance = advance.clone();
                let panel = wizard_panel(
                    state,
                    move || busy.get(),
                    false,
                    view().app_class(tw("flex flex-col gap-4")).child(dynamic(move || {
                        let current = current_step.get();
                        let item = panel_item.clone();
                        let advance = panel_advance.clone();
                        untracked(move || match current {
                            LinkStep::Machine => machine_step(
                                pick_machine,
                                plan,
                                choice,
                                address,
                                busy,
                                error,
                                advance,
                            )
                            .into_element(),
                            LinkStep::Access => access_step(
                                &item,
                                plan,
                                address,
                                access,
                                pairing_code,
                                busy,
                                advance,
                            )
                            .into_element(),
                        })
                    }))
                    .child(dynamic(move || {
                        use_existing.get().then(|| {
                            text(tr(
                                "This environment is already linked to {target}. The agent can use that link as it is; change its access in Settings → Environments.",
                                &[("target", Value::Text(target_label.get()))],
                            ))
                            .app_class(tw("text-xs text-muted-foreground"))
                        })
                    }))
                    .child(dynamic(move || {
                        error.get().map(|message| {
                            text(Text::client_key(message).resolve())
                                .app_class(tw("text-xs text-destructive"))
                        })
                    })),
                );
                let close = close.clone();
                let advance = advance.clone();
                let footer = wizard_footer(
                    state,
                    None,
                    view()
                        .app_class(tw("flex flex-col-reverse gap-2 sm:flex-row"))
                        .child(dynamic(move || {
                            let back = shows_back.get();
                            let close = close.clone();
                            untracked(move || {
                                if back {
                                    button(
                                        ButtonContent::Label(t("Back")),
                                        ButtonVariant::Outline,
                                        ButtonSize::Default,
                                        move || busy.get(),
                                        move || {
                                            owner.run(|| {
                                                spawn(async move {
                                                    let _ = step.try_set(LinkStep::Machine);
                                                    let _ = error.try_set(None);
                                                })
                                            });
                                        },
                                    )
                                    .into_element()
                                } else {
                                    button(
                                        ButtonContent::Label(t("Cancel")),
                                        ButtonVariant::Outline,
                                        ButtonSize::Default,
                                        move || busy.get(),
                                        move || close(),
                                    )
                                    .into_element()
                                }
                            })
                        }))
                        .child(dynamic(move || {
                            let label = t(primary_label.get());
                            let advance = advance.clone();
                            untracked(move || {
                                button(
                                    ButtonContent::Label(label),
                                    ButtonVariant::Default,
                                    ButtonSize::Default,
                                    move || {
                                        busy.get()
                                            || plan.with(|p| {
                                                if p.finishes_here() {
                                                    !p.ready
                                                } else {
                                                    !p.machine_ready
                                                }
                                            })
                                    },
                                    advance,
                                )
                            })
                        })),
                );
                vec![
                    header.into_element(),
                    panel.into_element(),
                    footer.into_element(),
                ]
            },
        )
    })
}

fn field_label(label: String) -> impl IntoElement {
    text(SharedString::from(label)).app_class(tw("block text-xs font-medium text-foreground"))
}

/// The machine step: MachinePicker when the agent left the machine open, then the address when
/// one must be typed.
fn machine_step(
    pick_machine: bool,
    plan: Memo<Plan>,
    choice: Signal<Option<String>>,
    address: Signal<String>,
    busy: Signal<bool>,
    error: Signal<Option<String>>,
    advance: impl Fn() + Clone + 'static,
) -> impl IntoElement {
    let mut step = view().app_class(tw("flex flex-col gap-4"));
    if pick_machine {
        step = step.child(machine_picker(plan, choice, busy, error));
    }
    let advance_address = advance.clone();
    let asks_address = computed(move || plan.with(|p| p.asks_address));
    step.child(dynamic(move || {
        asks_address.get().then(|| {
            let advance = advance_address.clone();
            untracked(move || {
                let mut field = view()
                    .app_class(tw("flex flex-col gap-1.5"))
                    .child(field_label(t("Address")))
                    .child(text_field(
                        address,
                        "https://machine.tailnet.ts.net".into(),
                        t("Address"),
                        InputSize::Default,
                        advance,
                    ));
                if let LinkRequestTarget::Machine { label, .. } = plan.with_untracked(|p| p.target.clone()) {
                    field = field.child(
                        text(tr(
                            "This app has no address for {machine} that another machine can reach. Enter its https or Tailscale address.",
                            &[("machine", Value::Text(label))],
                        ))
                        .app_class(tw("block text-xs text-muted-foreground")),
                    );
                }
                field
            })
        })
    }))
}

/// Port of LinkRequestCard.tsx MachinePicker with LinkMachineBadge.
fn machine_picker(
    plan: Memo<Plan>,
    choice: Signal<Option<String>>,
    busy: Signal<bool>,
    error: Signal<Option<String>>,
) -> impl IntoElement {
    use crate::ui::switch::{ControlState, radio, radio_group};
    let options = computed(move || plan.with(|p| (p.machines.clone(), p.selected.clone())));
    view()
        .app_class(tw("flex flex-col gap-2"))
        .child(field_label(t("Machine")))
        .child(dynamic(move || {
            let (machines, selected) = options.get();
            untracked(move || {
                let mut rows: Vec<AnyElement> = machines
                    .into_iter()
                    .map(|machine| {
                        let id = machine.environment_id.0.clone();
                        let checked = selected.as_deref() == Some(id.as_str());
                        let badge = if machine.linked {
                            Some((badge::Variant::Success, t("Linked")))
                        } else if machine.connected {
                            None
                        } else {
                            Some((badge::Variant::Outline, t("Not connected")))
                        };
                        view()
                            .app_class(tw(
                                "flex items-center gap-2.5 rounded-lg border px-3 py-2 text-sm",
                            ))
                            .child(radio(
                                machine.label.clone(),
                                ControlState {
                                    checked: checked.into(),
                                    disabled: Prop::bind(move || busy.get()),
                                    ..Default::default()
                                },
                                move |on| {
                                    if on {
                                        choice.set(Some(id.clone()));
                                        error.set(None);
                                    }
                                },
                            ))
                            .child(
                                text(SharedString::from(machine.label.clone()))
                                    .app_class(tw("min-w-0 flex-1 truncate font-medium")),
                            )
                            .child(badge.map(|(variant, label)| {
                                badge::badge(
                                    badge::Props {
                                        variant,
                                        size: badge::Size::Sm,
                                        ..Default::default()
                                    },
                                    vec![badge::Child::Element(
                                        text(SharedString::from(label)).into_element(),
                                    )],
                                )
                            }))
                            .into_element()
                    })
                    .collect();
                let another = selected.as_deref() == Some(ANOTHER_ADDRESS);
                rows.push(
                    view()
                        .app_class(tw(
                            "flex items-center gap-2.5 rounded-lg border px-3 py-2 text-sm",
                        ))
                        .child(radio(
                            "Another address…",
                            ControlState {
                                checked: another.into(),
                                disabled: Prop::bind(move || busy.get()),
                                ..Default::default()
                            },
                            move |on| {
                                if on {
                                    choice.set(Some(ANOTHER_ADDRESS.to_owned()));
                                    error.set(None);
                                }
                            },
                        ))
                        .child(
                            text(t("Another address…")).app_class(tw("min-w-0 flex-1 font-medium")),
                        )
                        .into_element(),
                );
                radio_group("Machine", rows)
            })
        }))
}

/// The access step: the agent's address, the address warning, the pairing code and the access.
fn access_step(
    item: &TurnItemLinkRequest,
    plan: Memo<Plan>,
    address: Signal<String>,
    access: Signal<AuthMcpClientAccess>,
    pairing_code: Signal<String>,
    busy: Signal<bool>,
    advance: impl Fn() + Clone + 'static,
) -> impl IntoElement {
    let current = plan.get_untracked();
    let mut step = view().app_class(tw("flex flex-col gap-4"));
    if let (Some(url), LinkRequestTarget::Machine { .. }) =
        (item.url.clone().flatten(), &current.target)
    {
        step = step.child(
            text(SharedString::from(url.0)).app_class(tw("truncate text-xs text-muted-foreground")),
        );
    }
    if current.use_existing {
        return step;
    }
    if current.needs_address {
        step = step.child(link_address_warning(address));
    }
    step = step.child(
        view()
            .app_class(tw("flex flex-col gap-1.5"))
            .child(field_label(t("Pairing code")))
            .child(text_field(
                pairing_code,
                t("Paste the code"),
                t("Pairing code"),
                InputSize::Default,
                advance,
            ))
            .child(
                text(tr(
                    "Create a link on {target}: Settings → Sharing & access → Authorized clients → Create link, then paste its code here.",
                    &[("target", Value::Text(current.target_name.clone()))],
                ))
                .app_class(tw("block text-xs text-muted-foreground")),
            ),
    );
    step.child(link_access_picker(access, busy))
}

/// Port of LinkMachineSelect.tsx LinkAddressWarning.
fn link_address_warning(address: Signal<String>) -> impl IntoElement {
    use crate::ui::alert::{
        Props, Slots, Variant, alert, alert_description, alert_icon, alert_title,
    };
    alert(
        Props {
            variant: Variant::Warning,
            ..Default::default()
        },
        move |context| {
            Slots {
            icons: vec![alert_icon(context, Icon::TriangleAlert).into_element()],
            content: vec![
                alert_title(text(t("Check this address"))).into_element(),
                alert_description(
                    context,
                    view()
                        .child(text(t(
                            "This app cannot confirm it is one of your machines. The pairing code you paste is sent there.",
                        )))
                        .child(text(move || SharedString::from(link_destination(&address.get())))),
                )
                .into_element(),
            ],
            actions: Vec::new(),
        }
        },
    )
}

/// Port of settings/LinkAccessPicker.tsx LinkAccessPicker.
fn link_access_picker(access: Signal<AuthMcpClientAccess>, busy: Signal<bool>) -> impl IntoElement {
    use crate::ui::select::{SelectItem, SelectProps, Size, Value as SelectValue, select};
    let mut props = SelectProps::new(Text::from("What agents here may do there"));
    props.items = ACCESSES
        .iter()
        .map(|option| {
            SelectItem::new(
                access_value(option),
                Text::client_key(access_label(option)).resolve(),
            )
        })
        .collect::<Vec<_>>()
        .into();
    props.value = Prop::bind(move || {
        SelectValue::Controlled(Some(SharedString::from(access_value(&access.get()))))
    });
    props.disabled = Prop::bind(move || busy.get());
    props.size = Size::Default;
    view()
        .app_class(tw("flex flex-col gap-1.5"))
        .child(field_label(t("What agents here may do there")))
        .child(select(props, move |next| {
            if let Some(option) = next
                .as_deref()
                .and_then(|next| ACCESSES.iter().find(|a| access_value(a) == next))
            {
                access.set(*option);
            }
        }))
        .child(
            text(move || {
                SharedString::from(format!(
                    "{} {}",
                    access_description(&access.get()),
                    t("An agent never gets more access there than its own mode here. Below full access, it can change only threads this link started, not that machine's own threads, projects or settings.")
                ))
            })
            .app_class(tw("text-xs text-muted-foreground")),
        )
}
