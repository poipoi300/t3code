//! DSK-BACKEND: the local server's startup state and the "could not start" recovery window.
//!
//! Port of apps/desktop/src/app/DesktopStartupRecovery.ts's window. The desktop shell owns the
//! server process and publishes a [`LocalBackendStatus`] through a [`LocalBackendHandle`] it
//! provides as a context; the policy (restart delay, when to report, which offer) is
//! `t3_client::view_logic::shell::startup_recovery`. The overlay is absent without a handle
//! (web, phone and tests that do not model a local server).

use std::rc::Rc;

use nsh_core::prelude::*;
use t3_client::view_logic::shell::startup_recovery::{RecoveryOffer, StartupFailure};

use crate::i18n::{t, tr};
use crate::theme::{metrics, palette};
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};
use crate::ui::scroll_area::scroll_area;
use crate::ui::text::{TextRole, Tone, label};

/// What the window offers to do; the shell performs it (`runAction`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryAction {
    Update,
    Reinstall,
    Releases,
    Copy,
    Quit,
}

/// The failure the window reports and everything it prints around it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailedStartup {
    pub failure: StartupFailure,
    pub offer: RecoveryOffer,
    pub app_version: String,
    pub log_dir: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalBackendStatus {
    /// Running and answering, or not supervised: nothing is shown.
    Ready,
    /// First launch of the server, before it answers.
    Starting,
    /// The server exited; restart `attempt` is scheduled and the failure is not yet reported.
    Restarting { attempt: u32 },
    /// Restarts did not help (`reportStartupFailure`).
    Failed(Box<FailedStartup>),
}

/// Provided by the desktop shell above `App`.
#[derive(Clone)]
pub struct LocalBackendHandle {
    pub status: Signal<LocalBackendStatus>,
    pub act: Rc<dyn Fn(RecoveryAction)>,
}

/// What the desktop shell holds to report the server's state from its own thread
/// ([`install_local_backend`]).
#[derive(Clone)]
pub struct LocalBackendPublisher(futures::channel::mpsc::UnboundedSender<LocalBackendStatus>);

impl LocalBackendPublisher {
    /// The window shows `status` from now on.
    pub fn publish(&self, status: LocalBackendStatus) {
        let _ = self.0.unbounded_send(status);
    }
}

type Act = std::sync::Arc<dyn Fn(RecoveryAction) + Send + Sync>;
type Pending = (
    futures::channel::mpsc::UnboundedReceiver<LocalBackendStatus>,
    Act,
);

thread_local! {
    static INSTALLED: std::cell::RefCell<Option<Pending>> = const { std::cell::RefCell::new(None) };
}

/// The shell calls this once on the thread that runs the UI, before the app starts, when it
/// supervises a local server: `act` performs what the recovery window asks (`runAction`) and the
/// returned publisher feeds the window. Without a call the overlay stays inert.
pub fn install_local_backend(
    act: impl Fn(RecoveryAction) + Send + Sync + 'static,
) -> LocalBackendPublisher {
    let (sender, receiver) = futures::channel::mpsc::unbounded();
    INSTALLED.with(|installed| {
        *installed.borrow_mut() = Some((receiver, std::sync::Arc::new(act)));
    });
    LocalBackendPublisher(sender)
}

thread_local! {
    static MOUNTED: std::cell::RefCell<Option<LocalBackendHandle>> = const { std::cell::RefCell::new(None) };
}

/// The handle of the installed server link on the UI thread: created from [`install_local_backend`]
/// the first time it is asked for, then shared.
fn installed_handle() -> Option<LocalBackendHandle> {
    use futures::StreamExt;
    if let Some(handle) = MOUNTED.with(|mounted| mounted.borrow().clone()) {
        return Some(handle);
    }
    let (mut receiver, act) = INSTALLED.with(|installed| installed.borrow_mut().take())?;
    let status = signal(LocalBackendStatus::Starting);
    crate::runtime::tasks::watch(async move {
        while let Some(next) = receiver.next().await {
            status.set(next);
        }
    });
    let handle = LocalBackendHandle {
        status,
        act: Rc::new(move |action| act(action)),
    };
    MOUNTED.with(|mounted| *mounted.borrow_mut() = Some(handle.clone()));
    Some(handle)
}

/// The overlay `app::themed_tree` mounts: nothing while ready, a status line while starting or
/// restarting, the recovery window once the failure is reported.
pub fn overlay() -> AnyElement {
    let Some(handle) = use_context::<LocalBackendHandle>().or_else(installed_handle) else {
        return view().into_element();
    };
    let status = handle.status;
    let act = handle.act;
    dynamic(move || {
        let current = status.get();
        let act = act.clone();
        untracked(|| match current {
            LocalBackendStatus::Ready => view().into_element(),
            LocalBackendStatus::Starting => status_line(t("Starting T3 Code…")),
            LocalBackendStatus::Restarting { attempt } => status_line(tr(
                "The server stopped. Restarting (attempt {attempt})…",
                &[("attempt", attempt.into())],
            )),
            LocalBackendStatus::Failed(failed) => recovery_window(*failed, act),
        })
    })
    .into_element()
}

fn status_line(text: String) -> AnyElement {
    view()
        .test_id("local-backend-status")
        .app_class(crate::theme::tw("absolute inset-0"))
        .bg(move || palette().background)
        .items_center()
        .justify_center()
        .child(label(TextRole::Body, Tone::Muted, move || text.clone()))
        .into_element()
}

/// `buildStartupRecoveryHtml`: title, reason, the offer and its button, copy and quit, the last
/// output of the server and a footer with the version and log directory.
pub fn recovery_window(failed: FailedStartup, act: Rc<dyn Fn(RecoveryAction)>) -> AnyElement {
    let FailedStartup {
        failure,
        offer,
        app_version,
        log_dir,
    } = failed;
    let output = failure.output.trim().to_owned();
    let output = if output.is_empty() {
        t("The server wrote no output.")
    } else {
        output
    };
    let offer_button = offer_action(&offer).map(|(action, text)| {
        let act = act.clone();
        button(
            ButtonContent::Label(text),
            ButtonVariant::Default,
            ButtonSize::Default,
            || false,
            move || act(action),
        )
    });
    let reason = failure.reason.clone();
    let offer_sentence = localized_offer_text(&offer);
    let footer = tr(
        "Version {version}. T3 Code tries again in the background. Logs: {logs}",
        &[("version", app_version.into()), ("logs", log_dir.into())],
    );
    let (copy, quit) = (act.clone(), act);
    view()
        .test_id("startup-recovery")
        .app_class(crate::theme::tw("absolute inset-0"))
        .bg(move || palette().background)
        .p(px(28.0))
        .gap(px(14.0))
        .child(label(TextRole::Title, Tone::Default, || {
            t("T3 Code could not start")
        }))
        .child(label(TextRole::Body, Tone::Default, move || reason.clone()))
        .child(label(TextRole::Body, Tone::Default, move || {
            offer_sentence.clone()
        }))
        .child(
            view()
                .flex_row()
                .gap(px(8.0))
                .child(offer_button)
                .child(button(
                    ButtonContent::Label(t("Copy details")),
                    ButtonVariant::Outline,
                    ButtonSize::Default,
                    || false,
                    move || copy(RecoveryAction::Copy),
                ))
                .child(button(
                    ButtonContent::Label(t("Quit")),
                    ButtonVariant::Outline,
                    ButtonSize::Default,
                    || false,
                    move || quit(RecoveryAction::Quit),
                )),
        )
        .child(
            view()
                .flex_1()
                .min_h(px(60.0))
                .w_full()
                .rounded(move || px(metrics().radius_lg))
                .border_1()
                .border_color(move || palette().border)
                .bg(move || palette().muted)
                .p(px(10.0))
                .child(scroll_area(
                    false,
                    label(TextRole::Code, Tone::Default, move || output.clone()),
                )),
        )
        .child(label(TextRole::Caption, Tone::Muted, move || {
            footer.clone()
        }))
        .into_element()
}

/// The primary button of an offer, when it has one (`describeOffer().action`).
fn offer_action(offer: &RecoveryOffer) -> Option<(RecoveryAction, String)> {
    match offer {
        RecoveryOffer::Update { version } => Some((
            RecoveryAction::Update,
            tr(
                "Update to {version}",
                &[("version", version.clone().into())],
            ),
        )),
        RecoveryOffer::Reinstall { version } => Some((
            RecoveryAction::Reinstall,
            tr("Download {version}", &[("version", version.clone().into())]),
        )),
        RecoveryOffer::Releases => Some((RecoveryAction::Releases, t("Open releases"))),
        RecoveryOffer::Checking
        | RecoveryOffer::Downloading { .. }
        | RecoveryOffer::ManagedPolicy => None,
    }
}

/// The sentence of `offer_text`, translated: each variant has its own catalogue key with the
/// version and percentage as arguments, so an interpolated string is never looked up.
fn localized_offer_text(offer: &RecoveryOffer) -> String {
    match offer {
        RecoveryOffer::Checking => t("Checking for a newer version…"),
        RecoveryOffer::Update { version } => tr(
            "Version {version} is available. It can contain a fix.",
            &[("version", version.clone().into())],
        ),
        RecoveryOffer::Downloading { version, percent } => {
            let progress = percent.map_or_else(String::new, |p| format!(" ({}%)", p / 10 * 10));
            tr(
                "Downloading {version}{progress}. T3 Code restarts when the download is done.",
                &[
                    ("version", version.clone().into()),
                    ("progress", progress.into()),
                ],
            )
        }
        RecoveryOffer::Reinstall { version } => tr(
            "Version {version} started correctly on this computer. Download it and install it to go back.",
            &[("version", version.clone().into())],
        ),
        RecoveryOffer::Releases => t("Get a different version from the releases page."),
        RecoveryOffer::ManagedPolicy => t(
            "Your organization's managed policy stopped T3 Code from starting. Send the details below to your IT administrator.",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use t3_client::view_logic::shell::startup_recovery::offer_text;

    #[test]
    fn only_offers_with_something_to_do_have_a_button() {
        assert_eq!(
            offer_action(&RecoveryOffer::Releases).map(|(a, _)| a),
            Some(RecoveryAction::Releases)
        );
        assert!(offer_action(&RecoveryOffer::Checking).is_none());
        assert!(offer_action(&RecoveryOffer::ManagedPolicy).is_none());
        assert!(
            offer_action(&RecoveryOffer::Downloading {
                version: "1".into(),
                percent: None
            })
            .is_none()
        );
    }

    #[test]
    fn update_and_reinstall_offers_name_their_version() {
        assert_eq!(
            offer_action(&RecoveryOffer::Update {
                version: "1.3.0".into()
            }),
            Some((RecoveryAction::Update, "Update to 1.3.0".into()))
        );
        assert_eq!(
            offer_action(&RecoveryOffer::Reinstall {
                version: "1.1.0".into()
            }),
            Some((RecoveryAction::Reinstall, "Download 1.1.0".into()))
        );
    }

    /// The translated sentences equal the policy module's English one for every offer, so the
    /// two cannot drift (the policy module's text is the TypeScript string).
    #[test]
    fn every_localized_sentence_equals_the_policy_sentence_in_english() {
        for offer in [
            RecoveryOffer::Checking,
            RecoveryOffer::Update {
                version: "1.3.0".into(),
            },
            RecoveryOffer::Downloading {
                version: "1.3.0".into(),
                percent: Some(47),
            },
            RecoveryOffer::Downloading {
                version: "1.3.0".into(),
                percent: None,
            },
            RecoveryOffer::Reinstall {
                version: "1.1.0".into(),
            },
            RecoveryOffer::Releases,
            RecoveryOffer::ManagedPolicy,
        ] {
            assert_eq!(
                localized_offer_text(&offer),
                offer_text(&offer),
                "{offer:?}"
            );
        }
    }
}
