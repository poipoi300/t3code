//! Port of apps/web/src/routes/pair.tsx and apps/web/src/components/auth/PairingRouteSurface.tsx
//! (PairingRouteSurface, HostedPairingRouteSurface), with the remote fields of
//! components/settings/AddEnvironmentDialog.tsx, the pairing-link confirmation of
//! components/desktop/DesktopWebLinkCoordinator.tsx and the saved environments of
//! components/settings/SavedEnvironmentRows.tsx.
//!
//! This client always pairs as a remote client (it never shares the environment's cookie), so the
//! page asks for a pairing link, or a host and a pairing code, where the environment's own page
//! asks only for a token. Reading a link, taking its token out of the address and splitting a
//! pasted link are the client's (`t3_client::connection::pairing_link`); validation and its
//! messages are the client's (`resolve_pairing_target`); this screen collects fields and shows
//! outcomes.
//!
//! Departures, each forced by a frozen seam: the title "Connect to T3 Code", the "Pairing link"
//! field, the "Connect" button and the demo backend's "Fill in the demo server" are what
//! `tests/ui/support.rs::pair` and `tests/ui/slice.rs` (integrator-owned) drive; the web's copy
//! for those is in the hand-off (`reports/ui-conn-1.md`) with the change the integrator makes.

use std::rc::Rc;

use futures::future::LocalBoxFuture;
use nsh_core::element::ImageSrc;
use nsh_core::prelude::*;
use t3_client::connection::pairing_link::{
    LaunchAddress, PairingLaunch, pairing_link_origin, parse_pairing_url_fields,
    parse_remote_pairing_fields, read_pairing_launch,
};
use t3_client::connection::saved_environment::{
    RouteLabel, StatusText, StatusTone, environment_transport_label, saved_environment_status,
};
use t3_client::connection::{
    ConnectionDiagnosis, ConnectionFailure, PairingInput, Phase, present_connection_state,
};
use t3_client::types::EnvironmentId;

use crate::i18n::{Text, Value, t, tr};
use crate::runtime::backend::EnvironmentEntry;
use crate::runtime::demo::DEMO_PAIRING_URL;
use crate::runtime::rt;
use crate::state::nav::{Route, navigator};
use crate::theme::{cn, tw};
use crate::ui::alert::{self, alert, alert_description};
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};
use crate::ui::confirm::{ConfirmDialogOptions, ConfirmDialogVariant, request_confirm_dialog};
use crate::ui::icon::{Icon, icon_with_class};
use crate::ui::input::{InputSize, text_field};
use crate::ui::menu::{ItemOptions, MenuOptions, menu, menu_item, menu_popup, menu_separator, menu_trigger};
use crate::ui::toast::{ToastOptions, Variant as ToastVariant, stacked_thread_toast, toast_manager};

/// The operations the saved-environment rows need, bound to the client in
/// `runtime/services/connection.rs`. Tests provide one through context.
pub trait ConnectionActions {
    fn remove(&self, environment: &EnvironmentId) -> LocalBoxFuture<'static, Result<(), Text>>;
    fn reconnect(&self, environment: &EnvironmentId) -> LocalBoxFuture<'static, Result<(), Text>>;
    /// What failed on the way to `environment` (`useConnectionDiagnosis`), `None` while it is
    /// connected, off, or has not failed.
    fn diagnosis(&self, environment: &EnvironmentId) -> Option<ConnectionDiagnosis>;
}

/// Provided by a test root to stand in for the client.
#[derive(Clone)]
pub struct ConnectionActionsContext(pub Rc<dyn ConnectionActions>);

fn connection_actions() -> Option<Rc<dyn ConnectionActions>> {
    if let Some(ConnectionActionsContext(actions)) = use_context::<ConnectionActionsContext>() {
        return Some(actions);
    }
    rt().backend()
        .client()
        .map(|client| crate::runtime::services::connection::actions(&client))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Method {
    Link,
    Code,
}

#[derive(Clone, Debug, PartialEq)]
enum Attempt {
    Idle,
    Connecting,
    Failed(Text),
}

/// An environment the form pairs again (`AddEnvironmentDialog`'s `routeTarget`).
#[derive(Clone, Debug, PartialEq)]
struct RouteTarget {
    environment: EnvironmentId,
    label: String,
}

/// What the page shows once it has read the address it was opened at.
#[derive(Clone, PartialEq)]
enum Page {
    Reading,
    Form,
    Launch(HostedLaunch),
}

/// A pairing a launch link asked for, with the host shown before and while it is tried.
#[derive(Clone)]
struct HostedLaunch {
    host: String,
    label: String,
    input: PairingInput,
}

// A launch is read once per page, so its host and label identify it; the credential is not compared.
impl PartialEq for HostedLaunch {
    fn eq(&self, other: &Self) -> bool {
        self.host == other.host && self.label == other.label
    }
}

pub fn screen() -> AnyElement {
    let page = signal(Page::Reading);
    let nav = navigator();
    let runtime = rt();
    spawn(async move {
        let address = app_url().await;
        let LaunchAddress { launch, stripped } = address
            .as_deref()
            .map(read_pairing_launch)
            .unwrap_or(LaunchAddress {
                launch: PairingLaunch::None,
                stripped: None,
            });
        // stripPairingTokenFromUrl before anything is submitted: the credential leaves the
        // visible address (and, natively, what `app_url` answers) first.
        if let Some(stripped) = stripped {
            let _ = set_app_url(stripped);
        }
        match launch {
            PairingLaunch::None => page.set(Page::Form),
            PairingLaunch::Hosted(request) => page.set(Page::Launch(HostedLaunch {
                input: request.pairing_input(),
                host: request.host,
                label: request.label,
            })),
            PairingLaunch::Environment { host, token } => page.set(Page::Launch(HostedLaunch {
                input: PairingInput {
                    host: Some(host.clone()),
                    pairing_code: Some(token.into_inner()),
                    ..PairingInput::default()
                },
                host,
                label: String::new(),
            })),
            PairingLaunch::AppLink { pairing_url } => {
                page.set(Page::Form);
                pair_from_link(pairing_url.into_inner(), runtime.backend(), nav).await;
            }
        }
    });
    view()
        .w_full()
        .h_full()
        .child(dynamic(move || {
            let current = page.get();
            untracked(move || match current {
                Page::Reading => standalone_page(view()).into_element(),
                Page::Form => pairing_route_surface().into_element(),
                Page::Launch(launch) => hosted_pairing_route_surface(launch).into_element(),
            })
        }))
        .into_element()
}

/// `DesktopWebLinkCoordinator.pairFromLink`: any page can open a pairing link, so the user
/// confirms the backend's origin (with its scheme, so plain HTTP is not mistaken for HTTPS)
/// before its one-time token is spent.
async fn pair_from_link(
    pairing_url: String,
    backend: Rc<dyn crate::runtime::backend::Backend>,
    nav: crate::state::nav::Navigator,
) {
    let report = |detail: &str| {
        toast_manager().add(stacked_thread_toast(ToastOptions {
            variant: ToastVariant::Error,
            title: t("Could not add backend"),
            description: Some(t(detail)), // client English key
            ..ToastOptions::default()
        }));
    };
    let origin = match pairing_link_origin(&pairing_url) {
        Ok(origin) => origin,
        Err(failure) => return report(&failure.detail),
    };
    let message = tr(
        "Add the backend at {origin}?",
        &[("origin", Value::Text(origin.into()))],
    );
    // No mounted confirm host is no confirmation: the token is not spent.
    let confirmed = match request_confirm_dialog(message, None) {
        Some(answer) => answer.await,
        None => false,
    };
    if !confirmed {
        return;
    }
    let input = PairingInput {
        pairing_url: Some(pairing_url),
        ..PairingInput::default()
    };
    match backend.pair(input).await {
        Ok(_) => {
            toast_manager().add(ToastOptions {
                variant: ToastVariant::Success,
                title: t("Backend added"),
                description: Some(t(
                    "The environment is saved and will reconnect on app startup.",
                )),
                ..ToastOptions::default()
            });
            nav.open(Route::Home);
        }
        Err(failure) => report(&failure.detail),
    }
}

/// Port of apps/web/src/components/ui/standalone-page.tsx StandalonePage, until UI-WIZARD's
/// `ui/wizard.rs` provides it. `items-center-safe` centres while the card fits and top-aligns it
/// when it does not; the page scroll gives the same. `scrollbar-gutter-both` has no NSH meaning.
fn standalone_page(children: impl IntoElement) -> impl IntoElement {
    scroll().w_full().h_full().app_class(tw("bg-background text-foreground")).child(
        view()
            .app_class(tw("flex min-h-full items-center justify-center px-4 py-10 sm:px-6"))
            .child(
                view()
                    // index.css @utility dialog-glass, as ui/dialog.rs resolves it.
                    .app_class(tw("w-full max-w-xl rounded-2xl border p-6 bg-background/80 backdrop-blur-[12px] dark:backdrop-blur-[16px] backdrop-saturate-[1.14] dark:backdrop-saturate-[1.08] border-contrast-foreground/10 dark:border-white/8 shadow-[0_24px_64px_-24px_rgb(0_0_0/65%)] dark:shadow-[inset_0_1px_rgb(255_255_255/4%),0_24px_72px_-20px_rgb(0_0_0/90%)]"))
                    .child(children),
            ),
    )
}

/// Port of standalone-page.tsx StandalonePageHeader.
fn standalone_page_header(title: impl Fn() -> String + 'static, description: impl Fn() -> String + 'static) -> impl IntoElement {
    view()
        .child(
            view()
                .app_class(tw("flex flex-row items-baseline justify-between gap-3 text-muted-foreground"))
                .child(
                    // NSH gives a plain view no accessible name (the web's `role="img"
                    // aria-label="T3 Code"`): the mark is hidden and "Code" is read.
                    view()
                        .app_class(tw("flex flex-row items-baseline gap-1.5"))
                        .child(svg(t3_wordmark()).app_class(tw("h-3 w-[20px] shrink-0 text-foreground")).accessible(false))
                        .child(text(t("Code")).app_class(tw("text-lg font-medium tracking-tight"))),
                ),
        )
        .child(
            text(title)
                .role(AccessibilityRole::Header)
                .app_class(tw("mt-6 text-xl leading-tight font-semibold")),
        )
        .child(text(description).app_class(tw("mt-2 text-sm text-muted-foreground")))
}

/// Port of apps/web/src/components/T3Wordmark.tsx T3Wordmark (also private in thread/rows/work.rs).
fn t3_wordmark() -> ImageSrc {
    ImageSrc::svg_markup(
        r##"<svg viewBox="15.5309 37 94.3941 56.96" xmlns="http://www.w3.org/2000/svg"><path d="M33.4509 93V47.56H15.5309V37H64.3309V47.56H46.4109V93H33.4509ZM86.7253 93.96C82.832 93.96 78.9653 93.4533 75.1253 92.44C71.2853 91.3733 68.032 89.88 65.3653 87.96L70.4053 78.04C72.5386 79.5867 75.0186 80.8133 77.8453 81.72C80.672 82.6267 83.5253 83.08 86.4053 83.08C89.6586 83.08 92.2186 82.44 94.0853 81.16C95.952 79.88 96.8853 78.12 96.8853 75.88C96.8853 73.7467 96.0586 72.0667 94.4053 70.84C92.752 69.6133 90.0853 69 86.4053 69H80.4853V60.44L96.0853 42.76L97.5253 47.4H68.1653V37H107.365V45.4L91.8453 63.08L85.2853 59.32H89.0453C95.9253 59.32 101.125 60.8667 104.645 63.96C108.165 67.0533 109.925 71.0267 109.925 75.88C109.925 79.0267 109.099 81.9867 107.445 84.76C105.792 87.48 103.259 89.6933 99.8453 91.4C96.432 93.1067 92.0586 93.96 86.7253 93.96Z" fill="currentColor"/></svg>"##,
    )
}

fn error_alert(message: impl Fn() -> String + 'static) -> impl IntoElement {
    alert(
        alert::Props {
            variant: alert::Variant::Error,
            ..alert::Props::default()
        },
        move |cx| alert::Slots {
            content: vec![alert_description(cx, text(message)).into_element()],
            ..alert::Slots::default()
        },
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HostedStatus {
    Pairing,
    Paired,
    Error,
}

/// Port of PairingRouteSurface.tsx HostedPairingRouteSurface, also used for the environment's
/// own `/pair#token=…` link: the token was read from the address and never enters a field.
fn hosted_pairing_route_surface(launch: HostedLaunch) -> impl IntoElement {
    // Pressing Try again rebuilds its button; the attempt belongs to the page, not the button.
    let owner = stored(Owner::current());
    let status = signal(HostedStatus::Pairing);
    let message = signal(Text::from("Connecting to this backend."));
    let can_retry = signal(false);
    let token_submitted = signal(false);
    let nav = navigator();
    let backend = stored(rt().backend());
    let input = stored(launch.input.clone());
    let label = launch.label.clone();
    let submit = move || {
        if token_submitted.get_untracked() {
            status.set(HostedStatus::Error);
            message.set(Text::from(
                "This one-time pairing token was already submitted. Request a new pairing link.",
            ));
            can_retry.set(false);
            return;
        }
        status.set(HostedStatus::Pairing);
        message.set(Text::from("Connecting to this backend."));
        can_retry.set(false);
        token_submitted.set(true);
        let pairing = backend.get().pair(input.get());
        let label = label.clone();
        owner.get().run(|| spawn(async move {
            match pairing.await {
                Ok(_) => {
                    status.try_set(HostedStatus::Paired);
                    message.try_set(Text::content(if label.is_empty() {
                        t("The environment is saved in this browser.")
                    } else {
                        tr("{label} is saved in this browser.", &[("label", Value::Text(label.into()))])
                    }));
                }
                Err(failure) => {
                    token_submitted.try_set(false);
                    status.try_set(HostedStatus::Error);
                    can_retry.try_set(true);
                    message.try_set(Text::content(tr(
                        "{error} If the backend accepted this one-time token, request a new pairing link before retrying.",
                        &[("error", Value::Text(failure_message(&failure).into()))],
                    )));
                }
            }
        }));
    };
    submit();
    let host = launch.host;
    standalone_page(
        view()
            .child(standalone_page_header(
                move || match status.get() {
                    HostedStatus::Paired => t("Backend paired"),
                    HostedStatus::Error => t("Pairing failed"),
                    HostedStatus::Pairing => t("Pairing backend"),
                },
                move || message.with(|m| m.resolve().to_string()),
            ))
            .child(
                view()
                    .app_class(tw("mt-5 flex flex-row flex-wrap gap-1 rounded-lg border border-border/70 bg-background/55 px-3 py-3 text-xs leading-relaxed text-muted-foreground"))
                    .child(text(t("Host:")).app_class(tw("text-xs text-muted-foreground")))
                    .child(text(host).app_class(tw("font-mono text-xs text-foreground/80"))),
            )
            .child(dynamic(move || {
                (status.get() == HostedStatus::Error).then(|| {
                    view().app_class(tw("mt-5")).child(error_alert(|| {
                        t("Verify the backend is reachable from this browser, supports CORS for hosted clients, and is served over HTTPS when opening this page from HTTPS.")
                    }))
                })
            }))
            .child(
                view()
                    .app_class(tw("mt-6 flex flex-row flex-wrap gap-2"))
                    .child(dynamic(move || match (status.get(), can_retry.get()) {
                        (HostedStatus::Pairing, _) => Some(
                            button(ButtonContent::Label(t("Pairing...")), ButtonVariant::Default, ButtonSize::Sm, || true, || {})
                                .into_element(),
                        ),
                        (_, true) => Some(
                            button(ButtonContent::Label(t("Try again")), ButtonVariant::Default, ButtonSize::Sm, || false, submit.clone())
                                .into_element(),
                        ),
                        _ => None,
                    }))
                    .child(dynamic(move || {
                        (status.get() == HostedStatus::Paired).then(|| {
                            button(ButtonContent::Label(t("Open app")), ButtonVariant::Outline, ButtonSize::Sm, || false, move || nav.open(Route::Home))
                        })
                    })),
            ),
    )
}

/// The client's English reason, translated (a client sentence is a catalogue key).
fn failure_message(failure: &ConnectionFailure) -> String {
    t(&failure.detail) // client English key
}

fn pairing_input(method: Method, link: &str, host: &str, code: &str) -> Result<PairingInput, ConnectionFailure> {
    match method {
        Method::Link => Ok(PairingInput {
            pairing_url: Some(link.trim().to_owned()),
            ..PairingInput::default()
        }),
        Method::Code => parse_remote_pairing_fields(host, code, None).map(|fields| fields.pairing_input()),
    }
}

/// The form: PairingRouteSurface's page and buttons, with AddEnvironmentDialog's remote fields.
fn pairing_route_surface() -> impl IntoElement {
    let method = signal(Method::Link);
    let link = signal(String::new());
    let host = signal(String::new());
    let code = signal(String::new());
    let attempt = signal(Attempt::Idle);
    let route_target = signal(None::<RouteTarget>);
    let connecting = move || attempt.with(|a| *a == Attempt::Connecting);
    let nav = navigator();
    let runtime = rt();
    // Connect is rebuilt when the attempt starts (its label changes); the pairing must outlive it.
    let owner = stored(Owner::current());

    let submit = move || {
        if connecting() {
            return;
        }
        let parsed = pairing_input(
            method.get_untracked(),
            &link.get_untracked(),
            &host.get_untracked(),
            &code.get_untracked(),
        );
        let mut input = match parsed {
            Ok(input) => input,
            Err(failure) => {
                attempt.set(Attempt::Failed(Text::client_key(failure.detail)));
                return;
            }
        };
        let target = route_target.get_untracked();
        input.expected_environment = target.as_ref().map(|target| target.environment.clone());
        attempt.set(Attempt::Connecting);
        let pairing = runtime.backend().pair(input);
        owner.get().run(|| spawn(async move {
            let outcome = pairing.await;
            // A pairing link or code is spent on any answer: neither stays in a field.
            link.try_set(String::new());
            code.try_set(String::new());
            match outcome {
                Ok(_) => {
                    attempt.try_set(Attempt::Idle);
                    host.try_set(String::new());
                    toast_manager().add(match &target {
                        Some(target) => ToastOptions {
                            variant: ToastVariant::Success,
                            title: t("Route added"),
                            description: Some(tr(
                                "{label} now has another way to connect.",
                                &[("label", Value::Text(target.label.clone().into()))],
                            )),
                            ..ToastOptions::default()
                        },
                        None => ToastOptions {
                            variant: ToastVariant::Success,
                            title: t("Backend added"),
                            description: Some(t(
                                "The environment is saved and will reconnect on app startup.",
                            )),
                            ..ToastOptions::default()
                        },
                    });
                    route_target.try_set(None);
                    nav.open(Route::Home);
                }
                Err(failure) => {
                    attempt.try_set(Attempt::Failed(Text::client_key(failure.detail)));
                }
            }
        }));
    };

    // AddEnvironmentDialog handleSavedBackendHostChange: a full pairing link pasted into the host
    // field fills both fields. `text_field` owns the change handler, so this follows its value.
    effect(move || {
        let value = host.get();
        if let Some(fields) = parse_pairing_url_fields(&value, None) {
            untracked(move || {
                host.set(fields.host);
                code.set(fields.pairing_code.into_inner());
            });
        }
    });

    let fields = view()
        .app_class(tw("flex flex-col gap-3"))
        .child(when(
            move || method.get() == Method::Link,
            move || {
                view().app_class(tw("flex flex-col gap-2"))
                    .child(text(t("Pairing link")).app_class(tw("text-sm font-medium")).accessible(false))
                    .child(text_field(link, t("https://host/pair#token=…"), t("Pairing link"), InputSize::Lg, submit))
            },
        ))
        .child(when(
            move || method.get() == Method::Code,
            move || {
                view()
                    .app_class(tw("flex flex-col gap-3"))
                    .child(
                        view()
                            .app_class(tw("flex flex-col gap-3 sm:flex-row"))
                            .child(
                                view().app_class(tw("flex min-w-0 flex-1 flex-col"))
                                    .child(text(t("Host")).app_class(tw("mb-1.5 text-xs font-medium text-foreground")).accessible(false))
                                    .child(text_field(host, t("backend.example.com"), t("Host"), InputSize::Default, submit)),
                            )
                            .child(
                                view().app_class(tw("flex flex-col sm:w-40"))
                                    .child(text(t("Pairing code")).app_class(tw("mb-1.5 text-xs font-medium text-foreground")).accessible(false))
                                    .child(text_field(code, t("PAIRCODE"), t("Pairing code"), InputSize::Default, submit)),
                            ),
                    )
                    .child(text(t("Paste a full pairing URL here to fill both fields automatically.")).app_class(tw("mt-1 text-2xs text-muted-foreground")))
            },
        ));

    let error = dynamic(move || {
        attempt.with(|a| match a {
            Attempt::Failed(detail) => {
                let detail = detail.resolve().to_string();
                Some(error_alert(move || detail.clone()))
            }
            _ => None,
        })
    });

    let actions = view()
        .app_class(tw("flex flex-row flex-wrap items-center gap-2"))
        .child(dynamic(move || {
            let label = if connecting() { t("Connecting…") } else { t("Connect") };
            untracked(move || button(ButtonContent::Label(label), ButtonVariant::Default, ButtonSize::Sm, connecting, submit))
        }))
        .child(button(
            ButtonContent::Label(t("Use a pairing code")),
            ButtonVariant::Outline,
            ButtonSize::Sm,
            connecting,
            move || {
                attempt.set(Attempt::Idle);
                method.update(|m| {
                    *m = match m {
                        Method::Link => Method::Code,
                        Method::Code => Method::Link,
                    }
                })
            },
        ))
        .child(dynamic(move || {
            route_target.get().map(|_| {
                button(ButtonContent::Label(t("Cancel")), ButtonVariant::Ghost, ButtonSize::Sm, connecting, move || {
                    route_target.set(None);
                    attempt.set(Attempt::Idle);
                })
            })
        }));
    // Only the demo backend knows the demo server.
    let actions = if runtime.is_demo() {
        actions.child(button(
            ButtonContent::Label(t("Fill in the demo server")),
            ButtonVariant::GhostMuted,
            ButtonSize::Sm,
            || false,
            move || {
                method.set(Method::Link);
                link.set(DEMO_PAIRING_URL.to_owned());
            },
        ))
    } else {
        actions
    };

    let entries = runtime.environments_signal();
    standalone_page(
        view()
            .child(standalone_page_header(
                move || match route_target.get() {
                    Some(target) => tr("Add a route to {label}", &[("label", Value::Text(target.label.into()))]),
                    None => t("Connect to T3 Code"),
                },
                move || match route_target.get() {
                    Some(_) => t("Pair this machine again over another address, such as its Tailscale name. It joins the existing routes instead of adding a second machine."),
                    None => t("Paste the pairing link your server printed, or enter its address and a pairing code."),
                },
            ))
            .child(
                view()
                    .app_class(tw("mt-6 flex flex-col gap-4"))
                    .child(fields)
                    .child(error)
                    .child(actions),
            )
            .child(
                view()
                    .app_class(tw("mt-6 rounded-lg border border-border/70 bg-background/55 px-3 py-3 text-xs leading-relaxed text-muted-foreground"))
                    .child(text(t("This environment accepts one-time pairing tokens. Pairing links can open this page directly, or you can paste the token here.")).app_class(tw("text-xs leading-relaxed text-muted-foreground"))),
            )
            .child(saved_environments(entries, route_target, method, attempt)),
    )
}

/// The saved environments under the form, so a broken one is listed with its reason and can be
/// paired again or removed. Port of the web's Settings > Connections "Environments" list
/// (`ConnectionsSettings.tsx`, `SavedBackendListRow`), reduced to what pairing needs.
fn saved_environments(
    entries: Signal<Vec<EnvironmentEntry>>,
    route_target: Signal<Option<RouteTarget>>,
    method: Signal<Method>,
    attempt: Signal<Attempt>,
) -> impl IntoElement {
    let actions = stored(connection_actions());
    let ids = computed(move || entries.with(|list| list.iter().map(|entry| entry.id.clone()).collect::<Vec<_>>()));
    dynamic(move || {
        let list = ids.get();
        untracked(move || {
            (!list.is_empty()).then(|| {
                let mut rows = view().app_class(tw("flex flex-col rounded-lg border"));
                for (index, id) in list.into_iter().enumerate() {
                    let entry = computed({
                        let id = id.clone();
                        move || entries.with(|list| list.iter().find(|entry| entry.id == id).cloned())
                    });
                    rows = rows
                        .child((index > 0).then(|| view().app_class(tw("h-px w-full bg-border"))))
                        .child(saved_backend_list_row(entry, actions, route_target, method, attempt));
                }
                view()
                    .app_class(tw("mt-6 flex flex-col gap-2"))
                    .child(text(t("Environments")).role(AccessibilityRole::Header).app_class(tw("text-sm font-medium text-foreground")))
                    .child(rows)
            })
        })
    })
}

fn status_text(text: &StatusText) -> String {
    match text {
        StatusText::Key(key) => t(key),
        StatusText::WithReason { key, reason } => tr(key, &[("reason", Value::Text(t(reason).into()))]), // reason: client English key
        StatusText::Content(content) => content.clone(),
    }
}

fn route_label(label: &RouteLabel) -> String {
    match label {
        RouteLabel::Key(key) => t(key),
        RouteLabel::Address(address) => address.clone(),
    }
}

/// Port of SavedEnvironmentRows.tsx SavedBackendListRow (identity, status, diagnosis headline,
/// More actions) inside EnvironmentRow.tsx EnvironmentRow's grid. Not here: the switch (the client
/// does not expose an environment's saved switch yet), Routes and Permissions disclosures,
/// server updates, MCP URL and machine icons (SET-CONNECTIONS).
fn saved_backend_list_row(
    entry: Memo<Option<EnvironmentEntry>>,
    actions: Stored<Option<Rc<dyn ConnectionActions>>>,
    route_target: Signal<Option<RouteTarget>>,
    method: Signal<Method>,
    attempt: Signal<Attempt>,
) -> impl IntoElement {
    // Menu items unmount when the menu closes; the removal and retry belong to the row.
    let owner = stored(Owner::current());
    let removing = signal(false);
    let label = move || entry.with(|e| e.as_ref().map(|e| e.label.clone()).unwrap_or_default());
    // Once per change of this environment's entry, not per read.
    let headline = computed(move || {
        entry.with(|e| {
            let e = e.as_ref()?;
            actions.with(|a| a.as_ref()?.diagnosis(&e.id)).map(|d| d.headline)
        })
    });
    let subtitle = move || {
        entry.with(|e| {
            let Some(e) = e else { return (String::new(), false) };
            let presentation = present_connection_state(&e.state);
            // The entry carries no saved switch: a switched-off environment is listed and not
            // wanted (`desired`), and one that never started is blocked, not off.
            let enabled = e.state.desired || e.state.phase != Phase::Available;
            let status = saved_environment_status(enabled, &presentation);
            let transport = route_label(&environment_transport_label(&e.routes));
            let text = if headline.get().is_some() {
                // The headline below names the failure in full.
                transport
            } else {
                format!("{transport} · {}", status_text(&status.text))
            };
            (text, status.tone == StatusTone::Error && headline.get().is_none())
        })
    };
    let damaged = move || entry.with(|e| e.as_ref().is_some_and(|e| e.damaged));
    let remove = move || {
        let Some(e) = entry.get_untracked() else { return };
        let Some(actions) = actions.get() else { return };
        let message = tr(
            "Remove {label} from this device?\nThis forgets its pairing, credentials, and cached threads here. Switch it off instead to keep it saved.",
            &[("label", Value::Text(e.label.clone().into()))],
        );
        let Some(confirmation) = request_confirm_dialog(
            message,
            Some(ConfirmDialogOptions {
                variant: ConfirmDialogVariant::Destructive,
                ..ConfirmDialogOptions::default()
            }),
        ) else {
            return;
        };
        owner.get().run(|| spawn(async move {
            if !confirmation.await {
                return;
            }
            removing.try_set(true);
            let result = actions.remove(&e.id).await;
            removing.try_set(false);
            if let Err(message) = result {
                toast_manager().add(stacked_thread_toast(ToastOptions {
                    variant: ToastVariant::Error,
                    title: t("Could not remove backend"),
                    description: Some(message.resolve().to_string()),
                    ..ToastOptions::default()
                }));
            }
        }));
    };
    let add_route = move || {
        let Some(e) = entry.get_untracked() else { return };
        route_target.set(Some(RouteTarget {
            environment: e.id,
            label: e.label,
        }));
        method.set(Method::Code);
        attempt.set(Attempt::Idle);
    };
    let retry = move || {
        let Some(e) = entry.get_untracked() else { return };
        let Some(actions) = actions.get() else { return };
        owner.get().run(|| spawn(async move {
            if let Err(message) = actions.reconnect(&e.id).await {
                toast_manager().add(stacked_thread_toast(ToastOptions {
                    variant: ToastVariant::Error,
                    title: t("Could not reconnect"),
                    description: Some(message.resolve().to_string()),
                    ..ToastOptions::default()
                }));
            }
        }));
    };
    let has_actions = actions.with(|a| a.is_some());
    let more_label = move || tr("More actions for {label}", &[("label", Value::Text(label().into()))]);
    view()
        .app_class(tw("flex flex-row items-center gap-x-3 px-3 py-2.5 sm:px-4"))
        .child(icon_with_class(Icon::Server, "size-4 text-muted-foreground"))
        .child(
            view()
                .app_class(tw("flex min-w-0 flex-1 flex-col"))
                .child(text(label).app_class(tw("truncate text-sm font-medium text-foreground")))
                .child(text(move || subtitle().0).app_class(move || {
                    Some(tw(&cn([Some("truncate text-xs text-muted-foreground"), subtitle().1.then_some("text-destructive")])))
                }))
                .child(dynamic(move || headline.get().map(|h| text(h).app_class(tw("text-xs text-destructive")))))
                .child(dynamic(move || {
                    damaged().then(|| {
                        text(t("Saved connection details for this environment could not be read. Pair it again or remove it."))
                            .app_class(tw("text-xs text-destructive"))
                    })
                }))
                .child(dynamic(move || {
                    (has_actions && headline.get().is_some()).then(|| {
                        view().app_class(tw("mt-1 flex flex-row")).child(button(
                            ButtonContent::Label(t("Retry now")),
                            ButtonVariant::Outline,
                            ButtonSize::Xs,
                            move || removing.get(),
                            retry,
                        ))
                    })
                })),
        )
        .child(dynamic(move || {
            has_actions.then(|| {
                let trigger_label = more_label();
                untracked(move || {
                    menu(
                        menu_trigger(
                            trigger_label.clone(),
                            move || removing.get(),
                            view()
                                .app_class(tw("flex size-7 items-center justify-center rounded-md text-muted-foreground"))
                                .child(icon_with_class(Icon::Ellipsis, "size-3.5")),
                        ),
                        MenuOptions {
                            align: Align::End,
                            ..MenuOptions::default()
                        },
                        menu_popup(
                            view()
                                .child(menu_item(t("Add route"), ItemOptions::default(), text(t("Add route")), add_route))
                                .child(menu_separator())
                                .child(menu_item(
                                    t("Remove from this device…"),
                                    ItemOptions {
                                        variant: crate::ui::menu::Variant::Destructive,
                                        ..ItemOptions::default()
                                    },
                                    text(move || if removing.get() { t("Removing…") } else { t("Remove from this device…") }),
                                    remove,
                                )),
                        ),
                    )
                })
            })
        }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use t3_client::connection::resolve_pairing_target;

    #[test]
    fn the_link_method_sends_only_the_trimmed_link() {
        let input = pairing_input(Method::Link, "  https://h.test/pair#token=abc ", "ignored", "ignored")
            .expect("a link is passed through");
        assert_eq!(input.pairing_url.as_deref(), Some("https://h.test/pair#token=abc"));
        assert!(input.host.is_none() && input.pairing_code.is_none());
        let target = resolve_pairing_target(&input).map_err(|f| f.detail).expect("valid");
        assert_eq!(target.http_base, "https://h.test/");
    }

    #[test]
    fn the_code_method_says_what_is_missing_before_the_client_is_asked() {
        let input = pairing_input(Method::Code, "ignored", " h.test:3773 ", " 1234 ").expect("both fields");
        assert_eq!(input.pairing_url, None);
        assert_eq!(input.host.as_deref(), Some("h.test:3773"));
        assert_eq!(input.pairing_code.as_deref(), Some("1234"));
        let missing = |host, code| pairing_input(Method::Code, "", host, code).err().map(|f| f.detail);
        assert_eq!(missing("h.test", "  ").as_deref(), Some("Enter a pairing code."));
        assert_eq!(missing(" ", "1234").as_deref(), Some("Enter a backend host."));
        let pasted = pairing_input(Method::Code, "", "https://h.test/pair#token=xyz", "").expect("a link in the host field");
        assert_eq!((pasted.host.as_deref(), pasted.pairing_code.as_deref()), (Some("https://h.test"), Some("xyz")));
    }
}
