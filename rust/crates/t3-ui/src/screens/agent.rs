//! Port of apps/web/src/routes/connect-agent.tsx, routes/approve-agent.tsx and
//! components/auth/ConnectAgentSurface.tsx (ConnectAgentSurface, McpConsent, ConnectAgentHeading,
//! AccessOption): an agent's MCP sign-in, approved on the environment's own page.
//!
//! The rules (access, decision, redirects, ceilings, the request) and the transport are the
//! client's (`t3_client::connection::agent_consent`); this screen shows them.
//!
//! Departures, each forced by NSH:
//! - The agent's redirect opens through the app's link opener (`services::links`), as a new
//!   page on the web; NSH cannot replace the page with another origin (`set_app_url` refuses it),
//!   where the web calls `window.location.replace`.
//! - Approve re-arms only when the page shows; NSH reports no window focus or visibility change
//!   to app code, where the web also re-arms on `focus` and `visibilitychange`.
//! - The description's host is not set in medium weight: one translated sentence, no inline span.
//! - The sign-in code field stays editable while a decision is pending (`ui::input` has no
//!   disabled state); the buttons are disabled as the web's are.

use std::rc::Rc;
use std::time::Duration;

use nsh_core::prelude::*;
use t3_client::connection::agent_consent::{
    ACCESS_LEVELS, ARM_DELAY_MS, AgentConsent, CANNOT_CONTINUE, McpApprovalAnswer, UNREACHABLE,
    UNSAFE_REDIRECT, agent_redirect_url, ceiling_reason, deny, page_origin,
    parse_mcp_approval_code, read_authorization_request, redirects_to_this_computer,
    resolve_approval_decision, resolve_mcp_access,
};
use t3_client::platform::Platform;
use t3_contracts::auth::{
    AuthMcpApprovalDetails, AuthMcpAuthorizationRequest, AuthMcpClientAccess,
};

use crate::i18n::{Text, Value, t, tr};
use crate::runtime::rt;
use crate::runtime::services::links::{LinkOpener, link_opener_here};
use crate::screens::connect::{error_alert, standalone_page, standalone_page_header_with_context};
use crate::state::nav::{Route, navigator};
use crate::theme::{cn, tw};
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};
use crate::ui::icon::{Icon, icon_with_class};
use crate::ui::input::{InputSize, text_field};

use crate::runtime::tasks::watch;

/// Both routes, as the router mounts them (`Route::ConnectAgent | Route::ApproveAgent`).
pub fn screen() -> AnyElement {
    let nav = navigator();
    if matches!(nav.top(), Route::ApproveAgent) {
        // approve-agent.tsx `beforeLoad`: only the hosted app with T3 Connect configured shows
        // it; this client is never the hosted static app, so it redirects to `/`, replacing.
        watch(async move {
            nav.back();
            nav.open(Route::Home);
        });
        return view().into_element();
    }
    connect_agent_surface(rt().backend().client())
}

/// ConnectAgentSurface over a given client: the page's request, sent to the environment that
/// serves the page. A host whose address is not a web page (desktop, mobile) has no such
/// environment, and the page says the sign-in cannot continue.
pub fn connect_agent_surface<P: Platform + Clone + 'static>(
    client: Option<t3_client::composition::ClientRuntime<P>>,
) -> AnyElement {
    // The transport is set once, before `ready`; signals compare values, a handle has none.
    let consent = stored(None::<Rc<dyn ConsentTransport>>);
    let authorization = signal(None::<AuthMcpAuthorizationRequest>);
    let ready = signal(false);
    let unavailable = signal(false);
    watch(async move {
        let address = app_url().await.unwrap_or_default();
        let origin = page_origin(&address);
        match (client, origin) {
            (Some(client), Some(origin)) => {
                let transport: Rc<dyn ConsentTransport> = Rc::new(client.agent_consent(&origin));
                consent.replace(Some(transport));
                authorization.set(Some(read_authorization_request(&address)));
                ready.set(true);
            }
            _ => unavailable.set(true),
        }
    });
    view()
        .w_full()
        .h_full()
        .child(dynamic(move || {
            if unavailable.get() {
                return Some(
                    untracked(|| invalid_page(Text::client_key(UNREACHABLE))).into_element(),
                );
            }
            if !ready.get() {
                return None;
            }
            let transport = consent.get()?;
            let request = authorization.get()?;
            Some(untracked(move || mcp_consent(request, transport)).into_element())
        }))
        .into_element()
}

/// The two calls the consent page makes, so the page holds one handle whatever the platform.
pub trait ConsentTransport {
    fn load(
        &self,
        authorization: &AuthMcpAuthorizationRequest,
    ) -> futures::future::LocalBoxFuture<'static, McpApprovalAnswer<AuthMcpApprovalDetails>>;
    fn decide(
        &self,
        authorization: &AuthMcpAuthorizationRequest,
        decision: t3_contracts::auth::AuthMcpApprovalDecision,
    ) -> futures::future::LocalBoxFuture<'static, McpApprovalAnswer<serde_json::Value>>;
}

impl<P: Platform + Clone + 'static> ConsentTransport for AgentConsent<P> {
    fn load(
        &self,
        authorization: &AuthMcpAuthorizationRequest,
    ) -> futures::future::LocalBoxFuture<'static, McpApprovalAnswer<AuthMcpApprovalDetails>> {
        let consent = self.clone();
        let authorization = authorization.clone();
        Box::pin(async move { consent.load(&authorization).await })
    }

    fn decide(
        &self,
        authorization: &AuthMcpAuthorizationRequest,
        decision: t3_contracts::auth::AuthMcpApprovalDecision,
    ) -> futures::future::LocalBoxFuture<'static, McpApprovalAnswer<serde_json::Value>> {
        let consent = self.clone();
        let authorization = authorization.clone();
        Box::pin(async move { consent.decide(&authorization, decision).await })
    }
}

#[derive(Clone, PartialEq)]
enum Loaded {
    Loading,
    Invalid(Text),
    Ready(AuthMcpApprovalDetails),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pending {
    Approve,
    Deny,
}

/// `followRedirect`: opens a safe redirect and says whether it did.
fn follow_redirect(links: &LinkOpener, url: &str) -> bool {
    match agent_redirect_url(url) {
        Some(safe) => {
            links.open(safe);
            true
        }
        None => false,
    }
}

/// The web's `accessConfig` (runtime modes from `runtimeModeConfig`).
fn access_label(access: AuthMcpClientAccess) -> &'static str {
    match access {
        AuthMcpClientAccess::ReadOnly => "Read only",
        AuthMcpClientAccess::ApprovalRequired => "Supervised",
        AuthMcpClientAccess::AutoAcceptEdits => "Auto-accept edits",
        AuthMcpClientAccess::Auto => "Auto",
        AuthMcpClientAccess::FullAccess => "Full access",
    }
}

fn access_description(access: AuthMcpClientAccess) -> &'static str {
    match access {
        AuthMcpClientAccess::ReadOnly => {
            "Read projects and threads. Cannot start, message or change anything."
        }
        AuthMcpClientAccess::ApprovalRequired => "Ask before commands and file changes.",
        AuthMcpClientAccess::AutoAcceptEdits => "Auto-approve edits, ask before other actions.",
        AuthMcpClientAccess::Auto => {
            "Supported providers approve routine actions; others still ask."
        }
        AuthMcpClientAccess::FullAccess => "Allow commands and edits without prompts.",
    }
}

fn access_icon(access: AuthMcpClientAccess) -> Icon {
    match access {
        AuthMcpClientAccess::ReadOnly => Icon::Eye,
        AuthMcpClientAccess::ApprovalRequired => Icon::Lock,
        AuthMcpClientAccess::AutoAcceptEdits => Icon::PenLine,
        AuthMcpClientAccess::Auto => Icon::ShieldCheck,
        AuthMcpClientAccess::FullAccess => Icon::ShieldOff,
    }
}

/// ConnectAgentHeading.
fn connect_agent_heading(
    title: impl Fn() -> String + 'static,
    description: impl Fn() -> String + 'static,
) -> impl IntoElement {
    standalone_page_header_with_context(|| t("Agent sign-in"), title, description)
}

fn invalid_page(message: Text) -> impl IntoElement {
    let description = message.resolve().to_string();
    standalone_page(
        view()
            .child(connect_agent_heading(
                || t("This sign-in cannot continue"),
                move || description.clone(),
            ))
            .child(
                text(t(
                    "Close this page and start the sign-in again from your agent.",
                ))
                .app_class(tw("mt-4 text-sm text-muted-foreground")),
            ),
    )
}

/// McpConsent with `acceptsCode` (the environment's own page) and no level required up front.
fn mcp_consent(
    authorization: AuthMcpAuthorizationRequest,
    transport: Rc<dyn ConsentTransport>,
) -> impl IntoElement {
    // Every task below belongs to the page, never to the control that started it (rule 7).
    let owner = stored(Owner::current());
    let links = stored(link_opener_here());
    let loaded = signal(Loaded::Loading);
    let requested = signal(None::<AuthMcpClientAccess>);
    let code = signal(String::new());
    let armed = signal(false);
    let error = signal(None::<Text>);
    let pending = signal(None::<Pending>);
    let transport = stored(transport);
    let authorization = stored(authorization);

    watch(async move {
        let answer = transport.get().load(&authorization.get()).await;
        let next = match answer {
            McpApprovalAnswer::Redirect(url) => {
                if links.with(|links| follow_redirect(links, &url)) {
                    return;
                }
                Loaded::Invalid(Text::client_key(UNSAFE_REDIRECT))
            }
            McpApprovalAnswer::Error(message) => Loaded::Invalid(Text::content(message)),
            McpApprovalAnswer::Ok(details) => Loaded::Ready(details),
        };
        let ready = matches!(next, Loaded::Ready(_));
        loaded.try_set(next);
        if ready {
            timer(Duration::from_millis(ARM_DELAY_MS)).await;
            armed.try_set(true);
        }
    });

    let access = move || {
        loaded.with(|loaded| {
            let details = match loaded {
                Loaded::Ready(details) => Some(details),
                _ => None,
            };
            resolve_mcp_access(
                requested.get(),
                details,
                !code.with(|c| c.trim().is_empty()),
            )
        })
    };

    let decide = move |choice: Pending| {
        let Loaded::Ready(details) = loaded.get_untracked() else {
            return;
        };
        let decision = match choice {
            Pending::Deny => deny(),
            Pending::Approve => {
                match resolve_approval_decision(&code.get_untracked(), untracked(access), &details)
                {
                    Some(decision) => decision,
                    None => return,
                }
            }
        };
        pending.set(Some(choice));
        error.set(None);
        let call = transport.get().decide(&authorization.get(), decision);
        owner.get().run(|| {
            watch(async move {
                let answer = call.await;
                if let McpApprovalAnswer::Redirect(url) = &answer
                    && links.with(|links| follow_redirect(links, url))
                {
                    return;
                }
                pending.try_set(None);
                error.try_set(Some(match answer {
                    McpApprovalAnswer::Error(message) => Text::content(message),
                    McpApprovalAnswer::Redirect(_) => Text::client_key(UNSAFE_REDIRECT),
                    McpApprovalAnswer::Ok(_) => Text::client_key(CANNOT_CONTINUE),
                }));
            })
        });
    };

    let can_approve = move || {
        armed.get()
            && pending.get().is_none()
            && loaded.with(|loaded| match loaded {
                Loaded::Ready(details) => {
                    resolve_approval_decision(&code.get(), access(), details).is_some()
                }
                _ => false,
            })
    };

    dynamic(move || {
        let state = loaded.get();
        untracked(move || match state {
            Loaded::Loading => standalone_page(
                view()
                    .child(connect_agent_heading(
                        || t("Checking the sign-in request"),
                        || t("One moment while this environment verifies the agent's request."),
                    ))
                    // Spinner: a static mark, never a repainting animation (NSH rule).
                    .child(view().app_class(tw("mt-6")).child(icon_with_class(
                        Icon::LoaderCircle,
                        "size-6 text-muted-foreground",
                    ))),
            )
            .into_element(),
            Loaded::Invalid(message) => invalid_page(message).into_element(),
            Loaded::Ready(details) => ready_page(
                details,
                code,
                requested,
                error,
                pending,
                access,
                can_approve,
                decide,
            )
            .into_element(),
        })
    })
    .into_element()
}

#[allow(clippy::too_many_arguments)]
fn ready_page(
    details: AuthMcpApprovalDetails,
    code: Signal<String>,
    requested: Signal<Option<AuthMcpClientAccess>>,
    error: Signal<Option<Text>>,
    pending: Signal<Option<Pending>>,
    access: impl Fn() -> AuthMcpClientAccess + Copy + 'static,
    can_approve: impl Fn() -> bool + Copy + 'static,
    decide: impl Fn(Pending) + Copy + 'static,
) -> impl IntoElement {
    let client_name = details.client_name.clone();
    let host = details.environment_host.clone();
    let redirect_host = details.redirect_host.clone();
    let here = redirects_to_this_computer(&redirect_host);
    let details = stored(details);
    // A code from "Use T3 Code in another app" names its level, so the page does not ask again.
    let code_access = move || parse_mcp_approval_code(&code.get()).map(|(level, _)| level);
    let approve_level = move || code_access().unwrap_or_else(access);

    let field = view()
        .app_class(tw("flex flex-col gap-2"))
        .child(text(t("Sign-in code")).app_class(tw("text-sm font-medium")).accessible(false))
        .child(text_field(code, t("Paste a code"), t("Sign-in code"), InputSize::Default, move || {
            if can_approve() {
                decide(Pending::Approve);
            }
        }))
        .child(
            text(t(
                "Get one from Use T3 Code in another app (MCP), Settings → Sharing & access, or t3 auth pairing create on this machine.",
            ))
            .app_class(tw("text-xs text-muted-foreground")),
        );

    let level = dynamic(move || {
        if let Some(level) = code_access() {
            return untracked(move || {
                view()
                    .app_class(tw("flex flex-row gap-1 text-sm"))
                    .child(text(t("Access")).app_class(tw("text-muted-foreground")))
                    .child(text(t(access_label(level))).app_class(tw("font-medium")))
                    .into_element()
            });
        }
        untracked(move || {
            let mut group = nsh_base::radio_group()
                .accessibility_label(Some(SharedString::from(t("What it may do"))))
                .app_class(tw("flex flex-col gap-2"));
            for option in ACCESS_LEVELS {
                let reason = details.with(|details| ceiling_reason(details, option));
                group = group.child(access_option(
                    option,
                    move || access() == option,
                    reason,
                    move || {
                        requested.set(Some(option));
                    },
                ));
            }
            view()
                .app_class(tw("flex flex-col gap-2"))
                .child(text(t("What it may do")).app_class(tw("text-sm font-medium")).accessible(false))
                .child(group)
                .child(
                    text(t(
                        "Beyond read only, it can start, message and stop threads, and none of them can run with more than the mode you pick.",
                    ))
                    .app_class(tw("text-xs text-muted-foreground")),
                )
                .into_element()
        })
    });

    let actions = view()
        .app_class(tw("flex flex-row flex-wrap gap-2"))
        .child(dynamic(move || {
            let label = if pending.get() == Some(Pending::Approve) {
                t("Approving…")
            } else {
                tr(
                    "Approve {level}",
                    &[("level", Value::Text(t(access_label(approve_level()))))],
                )
            };
            untracked(move || {
                button(
                    ButtonContent::Label(label),
                    ButtonVariant::Default,
                    ButtonSize::Default,
                    move || !can_approve(),
                    move || decide(Pending::Approve),
                )
            })
        }))
        .child(dynamic(move || {
            let label = if pending.get() == Some(Pending::Deny) {
                t("Denying…")
            } else {
                t("Deny")
            };
            untracked(move || {
                button(
                    ButtonContent::Label(label),
                    ButtonVariant::Outline,
                    ButtonSize::Default,
                    move || pending.get().is_some(),
                    move || decide(Pending::Deny),
                )
            })
        }));

    standalone_page(
        view()
            .child(connect_agent_heading(
                {
                    let client_name = client_name.clone();
                    move || {
                        tr(
                            "Connect {name}",
                            &[("name", Value::Text(client_name.clone()))],
                        )
                    }
                },
                {
                    let host = host.clone();
                    move || {
                        tr(
                            "This agent wants to use the threads in every project on {host}.",
                            &[("host", Value::Text(host.clone()))],
                        )
                    }
                },
            ))
            .child(
                view()
                    .app_class(tw("mt-4 flex flex-col gap-1 text-sm"))
                    .child(
                        view()
                            .app_class(tw("flex flex-row flex-wrap gap-x-4"))
                            .child(text(t("App")).app_class(tw("text-muted-foreground")))
                            .child(
                                text(tr(
                                    "{name} · name set by the app",
                                    &[("name", Value::Text(client_name.clone()))],
                                ))
                                .app_class(tw("min-w-0")),
                            ),
                    )
                    .child(
                        view()
                            .app_class(tw("flex flex-row flex-wrap gap-x-4"))
                            .child(text(t("Access goes to")).app_class(tw("text-muted-foreground")))
                            .child(text(redirect_host).app_class(tw("min-w-0 font-medium")))
                            .child(
                                text(if here {
                                    t("on the computer that opened this page")
                                } else {
                                    t("· whoever runs it gets this access")
                                })
                                .app_class(tw("font-normal text-muted-foreground")),
                            ),
                    ),
            )
            .child(
                text(t("Only approve a sign-in you just started."))
                    .app_class(tw("mt-2 text-xs text-muted-foreground")),
            )
            .child(
                view()
                    .app_class(tw("mt-6 flex flex-col gap-5"))
                    .child(field)
                    .child(level)
                    .child(dynamic(move || {
                        error.get().map(|message| {
                            let message = message.resolve().to_string();
                            error_alert(move || message.clone())
                        })
                    }))
                    .child(actions),
            ),
    )
}

/// AccessOption: one level as a card in the radio group.
fn access_option(
    access: AuthMcpClientAccess,
    selected: impl Fn() -> bool + Copy + 'static,
    disabled_reason: Option<String>,
    choose: impl Fn() + 'static,
) -> impl IntoElement {
    let disabled = disabled_reason.is_some();
    let detail = disabled_reason.unwrap_or_else(|| t(access_description(access)));
    nsh_base::radio()
        .checked(selected)
        .disabled(disabled)
        .accessibility_label(Some(SharedString::from(t(access_label(access)))))
        .on_change(move |value: &bool| {
            if *value && !disabled {
                choose();
            }
        })
        .app_class(move || {
            Some(tw(&cn([
                Some("flex flex-row cursor-pointer items-start gap-3 rounded-lg border px-3 py-2.5 text-left outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"),
                Some(if selected() {
                    "border-primary bg-background ring-2 ring-inset ring-primary/25 dark:border-transparent dark:bg-primary/10 dark:ring-1 dark:ring-primary/30"
                } else {
                    "border-border bg-background hover:bg-muted/50 dark:border-transparent dark:bg-white/[0.035] dark:hover:bg-accent"
                }),
                disabled.then_some("cursor-not-allowed opacity-64"),
            ])))
        })
        .child(dynamic(move || {
            let class = if selected() { "mt-0.5 size-4 shrink-0 text-primary" } else { "mt-0.5 size-4 shrink-0 text-muted-foreground" };
            untracked(move || icon_with_class(access_icon(access), class))
        }))
        .child(
            view()
                .app_class(tw("flex min-w-0 flex-col"))
                .child(text(t(access_label(access))).app_class(tw("text-sm font-medium text-foreground")))
                .child(text(detail).app_class(tw("text-xs text-muted-foreground"))),
        )
}
