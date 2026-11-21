//! Port of apps/desktop/src/app/DesktopStartupRecovery.ts (`resolveStartupRecoveryOffer`) and the
//! restart policy of apps/desktop/src/backend/DesktopBackendManager.ts. The desktop shell
//! supervises the local server; this module decides what the window says and when it says it.
//! Everything here is plain data so the shell, the UI and the tests share one answer.

use t3_contracts::ipc::{
    DesktopUpdateStateSchema as UpdateState, DesktopUpdateStatusSchema as Status,
};
use t3_contracts::settings::WireNumber;

/// First restart delay (`INITIAL_RESTART_DELAY`).
pub const INITIAL_RESTART_DELAY_MS: u64 = 500;
/// Longest restart delay (`MAX_RESTART_DELAY`).
pub const MAX_RESTART_DELAY_MS: u64 = 10_000;
/// Restarts without readiness before the window reports the failure
/// (`STARTUP_FAILURE_RESTART_ATTEMPTS`): about two seconds of backoff, past a blip.
pub const STARTUP_FAILURE_RESTART_ATTEMPTS: u32 = 3;
/// Tail of the last run's output kept for the window (`STARTUP_FAILURE_OUTPUT_CHARS`).
pub const STARTUP_FAILURE_OUTPUT_CHARS: usize = 4_000;
pub use t3_contracts::desktop_bootstrap::SERVER_EXIT_CODE_MANAGED_POLICY;
/// Where the releases are published (`RELEASES_URL`).
pub const RELEASES_URL: &str = "https://github.com/pingdotgg/t3code/releases";

/// `calculateRestartDelay`: 500 ms doubling per attempt, capped at ten seconds.
pub fn restart_delay_ms(attempt: u32) -> u64 {
    INITIAL_RESTART_DELAY_MS
        .checked_shl(attempt)
        .filter(|delay| delay >> attempt == INITIAL_RESTART_DELAY_MS)
        .map_or(MAX_RESTART_DELAY_MS, |delay| {
            delay.min(MAX_RESTART_DELAY_MS)
        })
}

/// What the supervisor does after the server exits without having been asked to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RestartPlan {
    /// This is restart number `attempt` (1-based).
    pub attempt: u32,
    /// Wait this long before spawning again.
    pub delay_ms: u64,
    /// The window must report the failure now, not stay on "starting".
    pub report_failure: bool,
}

/// `scheduleRestart`: `previous_attempts` restarts happened since the server was last ready. A
/// managed-policy refusal cannot be fixed by retrying, so it is reported at once; retries still
/// continue so a fixed policy is picked up.
pub fn plan_restart(previous_attempts: u32, exit_code: Option<i32>) -> RestartPlan {
    let attempt = previous_attempts.saturating_add(1);
    RestartPlan {
        attempt,
        delay_ms: restart_delay_ms(previous_attempts),
        report_failure: exit_code == Some(SERVER_EXIT_CODE_MANAGED_POLICY)
            || attempt >= STARTUP_FAILURE_RESTART_ATTEMPTS,
    }
}

/// `BackendStartupFailure`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupFailure {
    pub reason: String,
    /// The server refused to start under this machine's managed policy.
    pub managed_policy: bool,
    /// Tail of the last run's stdout and stderr.
    pub output: String,
}

impl StartupFailure {
    /// A failure with its finished reason; the output is cut to its last
    /// [`STARTUP_FAILURE_OUTPUT_CHARS`] characters (`reportStartupFailure`).
    pub fn new(reason: impl Into<String>, managed_policy: bool, output: &str) -> Self {
        let skip = output
            .chars()
            .count()
            .saturating_sub(STARTUP_FAILURE_OUTPUT_CHARS);
        Self {
            reason: reason.into(),
            managed_policy,
            output: output.chars().skip(skip).collect(),
        }
    }

    /// The server exited and restarts did not help (`The server could not start (${reason}).`).
    pub fn exited(reason: &str, managed_policy: bool, output: &str) -> Self {
        Self::new(
            format!("The server could not start ({reason})."),
            managed_policy,
            output,
        )
    }

    /// The server runs but never answered its readiness probe.
    pub fn not_ready(output: &str) -> Self {
        Self::new(
            "The server started but did not become ready in time.",
            false,
            output,
        )
    }
}

/// What the desktop shell learns about the primary backend (`primaryBackendStartup`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupEvent {
    Ready,
    Failing(StartupFailure),
}

/// What the shell does after an event.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StartupEffects {
    /// Publish `LocalBackendStatus::Failed` (open or refresh the recovery window).
    pub show_window: bool,
    /// Close the recovery window: the backend is ready and the main window is up.
    pub close_window: bool,
    /// Ask the updater for a newer build, once per run (`updates.check("startup-recovery")`).
    pub check_for_update: bool,
    /// Write `desktop-last-started-version` (the shell skips it when unchanged).
    pub record_started_version: bool,
}

/// The state `DesktopStartupRecovery.listen` keeps: whether the backend ever started, the first
/// failure of the current run of failures (later ones never replace it) and the one-time update
/// check. A window is never shown over a main window that is already visible after a start, so a
/// server that dies mid-session does not cover the live session.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StartupRecovery {
    backend_has_started: bool,
    failure: Option<StartupFailure>,
    checked: bool,
}

impl StartupRecovery {
    pub fn on_event(&mut self, event: StartupEvent, main_visible: bool) -> StartupEffects {
        match event {
            StartupEvent::Ready => {
                self.backend_has_started = true;
                self.failure = None;
                StartupEffects {
                    close_window: main_visible,
                    record_started_version: true,
                    ..Default::default()
                }
            }
            StartupEvent::Failing(failure) => {
                if self.failure.is_some() {
                    return StartupEffects::default();
                }
                self.failure = Some(failure);
                StartupEffects {
                    show_window: self.may_show(main_visible),
                    check_for_update: !std::mem::replace(&mut self.checked, true),
                    ..Default::default()
                }
            }
        }
    }

    /// macOS `activate`: the Dock icon brings the error back while a failure is recorded.
    pub fn on_activate(&self, main_visible: bool) -> bool {
        self.failure.is_some() && self.may_show(main_visible)
    }

    /// The failure the window reports, if any.
    pub fn failure(&self) -> Option<&StartupFailure> {
        self.failure.as_ref()
    }

    fn may_show(&self, main_visible: bool) -> bool {
        !(self.backend_has_started && main_visible)
    }
}

/// The page of one release (`${RELEASES_URL}/tag/v${version}`), for "Reinstall".
pub fn release_tag_url(version: &str) -> String {
    format!("{RELEASES_URL}/tag/v{version}")
}

/// `StartupRecoveryOffer`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryOffer {
    Checking,
    Update {
        version: String,
    },
    Downloading {
        version: String,
        percent: Option<u32>,
    },
    Reinstall {
        version: String,
    },
    Releases,
    ManagedPolicy,
}

/// `resolveStartupRecoveryOffer`: prefer a newer build, then the last build that started on this
/// machine, then the releases page.
pub fn resolve_offer(
    update: &UpdateState,
    last_started_version: Option<&str>,
    managed_policy: bool,
) -> RecoveryOffer {
    if update.status == Status::Downloading
        && let Some(version) = &update.available_version
    {
        return RecoveryOffer::Downloading {
            version: version.clone(),
            percent: update.download_percent.as_ref().and_then(percent),
        };
    }
    let update_version = update.downloaded_version.as_ref().or_else(|| {
        (update.status == Status::Available)
            .then_some(update.available_version.as_ref())
            .flatten()
    });
    // A version that already failed to apply here would loop through install and relaunch.
    if let Some(version) =
        update_version.filter(|v| Some(*v) != update.not_applied_version.as_ref())
    {
        return RecoveryOffer::Update {
            version: version.clone(),
        };
    }
    // A newer release can know a key this one refuses; an older one or the releases page cannot help.
    if managed_policy {
        return RecoveryOffer::ManagedPolicy;
    }
    if update.enabled && matches!(update.status, Status::Idle | Status::Checking) {
        return RecoveryOffer::Checking;
    }
    if let Some(version) = last_started_version.filter(|v| *v != update.current_version) {
        return RecoveryOffer::Reinstall {
            version: version.to_owned(),
        };
    }
    RecoveryOffer::Releases
}

fn percent(value: &WireNumber) -> Option<u32> {
    match value {
        WireNumber::Finite(n) => n.as_f64().filter(|p| *p >= 0.0).map(|p| p as u32),
        _ => None,
    }
}

/// `describeOffer`: the sentence the window shows beside the offer's button. Downloads show the
/// percentage rounded down to ten.
pub fn offer_text(offer: &RecoveryOffer) -> String {
    match offer {
        RecoveryOffer::Checking => "Checking for a newer version…".into(),
        RecoveryOffer::Update { version } => {
            format!("Version {version} is available. It can contain a fix.")
        }
        RecoveryOffer::Downloading { version, percent } => format!(
            "Downloading {version}{}. T3 Code restarts when the download is done.",
            percent.map_or(String::new(), |p| format!(" ({}%)", p / 10 * 10))
        ),
        RecoveryOffer::Reinstall { version } => format!(
            "Version {version} started correctly on this computer. Download it and install it to go back."
        ),
        RecoveryOffer::Releases => "Get a different version from the releases page.".into(),
        RecoveryOffer::ManagedPolicy => "Your organization's managed policy stopped T3 Code from starting. Send the details below to your IT administrator.".into(),
    }
}

/// The text "Copy details" puts on the clipboard.
pub fn copy_text(
    app_version: &str,
    platform: &str,
    arch: &str,
    failure: &StartupFailure,
) -> String {
    format!(
        "T3 Code {app_version} ({platform} {arch}) could not start.\n{}\n\n{}",
        failure.reason, failure.output
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update(status: Status) -> UpdateState {
        serde_json::from_value(serde_json::json!({
            "enabled": true, "status": serde_json::to_value(status).unwrap(),
            "channel": "latest", "currentVersion": "1.2.0", "hostArch": "x64", "appArch": "x64",
            "runningUnderArm64Translation": false, "availableVersion": null,
            "downloadedVersion": null, "releaseNotes": [], "omittedReleaseCount": 0,
            "downloadPercent": null, "checkedAt": null, "message": null, "errorContext": null,
            "canRetry": false
        }))
        .unwrap()
    }

    #[test]
    fn the_managed_policy_exit_code_is_the_contracts_value() {
        // packages/contracts/src/desktopBootstrap.ts
        assert_eq!(SERVER_EXIT_CODE_MANAGED_POLICY, 77);
    }

    #[test]
    fn restart_delay_doubles_from_half_a_second_to_the_cap() {
        let delays: Vec<u64> = (0..7).map(restart_delay_ms).collect();
        assert_eq!(delays, [500, 1000, 2000, 4000, 8000, 10_000, 10_000]);
        assert_eq!(restart_delay_ms(200), MAX_RESTART_DELAY_MS);
    }

    #[test]
    fn the_third_restart_reports_and_a_policy_refusal_reports_at_once() {
        assert!(!plan_restart(0, Some(1)).report_failure);
        assert!(!plan_restart(1, None).report_failure);
        let third = plan_restart(2, Some(1));
        assert!(third.report_failure && third.attempt == 3 && third.delay_ms == 2000);
        assert!(plan_restart(0, Some(SERVER_EXIT_CODE_MANAGED_POLICY)).report_failure);
    }

    #[test]
    fn a_newer_build_wins_then_the_last_started_build_then_the_releases_page() {
        let mut state = update(Status::Available);
        state.available_version = Some("1.3.0".into());
        assert_eq!(
            resolve_offer(&state, Some("1.1.0"), false),
            RecoveryOffer::Update {
                version: "1.3.0".into()
            }
        );
        // A version that already failed to apply is not offered again.
        state.not_applied_version = Some("1.3.0".into());
        state.enabled = false;
        assert_eq!(
            resolve_offer(&state, Some("1.1.0"), false),
            RecoveryOffer::Reinstall {
                version: "1.1.0".into()
            }
        );
        assert_eq!(
            resolve_offer(&state, Some("1.2.0"), false),
            RecoveryOffer::Releases
        );
        assert_eq!(resolve_offer(&state, None, false), RecoveryOffer::Releases);
    }

    #[test]
    fn checking_downloading_and_managed_policy_offers() {
        assert_eq!(
            resolve_offer(&update(Status::Idle), None, false),
            RecoveryOffer::Checking
        );
        assert_eq!(
            resolve_offer(&update(Status::Checking), Some("1.0.0"), false),
            RecoveryOffer::Checking
        );
        let mut state = update(Status::Downloading);
        state.available_version = Some("1.3.0".into());
        state.download_percent = Some(WireNumber::Finite(47.into()));
        let offer = resolve_offer(&state, None, false);
        assert_eq!(
            offer,
            RecoveryOffer::Downloading {
                version: "1.3.0".into(),
                percent: Some(47)
            }
        );
        assert!(offer_text(&offer).starts_with("Downloading 1.3.0 (40%)."));
        // Policy refusal skips the checking and reinstall offers but not a ready update.
        assert_eq!(
            resolve_offer(&update(Status::Idle), Some("1.0.0"), true),
            RecoveryOffer::ManagedPolicy
        );
        state = update(Status::Downloaded);
        state.downloaded_version = Some("1.4.0".into());
        assert_eq!(
            resolve_offer(&state, None, true),
            RecoveryOffer::Update {
                version: "1.4.0".into()
            }
        );
    }

    #[test]
    fn failure_keeps_the_last_four_thousand_characters_and_copy_text_matches_typescript() {
        let long = format!(
            "{}{}",
            "a".repeat(10),
            "é".repeat(STARTUP_FAILURE_OUTPUT_CHARS)
        );
        let failure = StartupFailure::exited("exit code 1", false, &long);
        assert_eq!(failure.output.chars().count(), STARTUP_FAILURE_OUTPUT_CHARS);
        assert!(failure.output.chars().all(|c| c == 'é'));
        assert_eq!(failure.reason, "The server could not start (exit code 1).");
        assert_eq!(
            copy_text(
                "1.2.0",
                "linux",
                "x64",
                &StartupFailure::exited("boom", true, "out")
            ),
            "T3 Code 1.2.0 (linux x64) could not start.\nThe server could not start (boom).\n\nout"
        );
    }

    #[test]
    fn the_readiness_timeout_reason_is_not_wrapped() {
        let failure = StartupFailure::not_ready("log");
        assert_eq!(
            failure.reason,
            "The server started but did not become ready in time."
        );
        assert!(!failure.managed_policy);
    }

    #[test]
    fn the_delay_overflow_guard_holds_at_the_shift_width() {
        assert_eq!(restart_delay_ms(63), MAX_RESTART_DELAY_MS);
        assert_eq!(restart_delay_ms(64), MAX_RESTART_DELAY_MS);
        assert_eq!(restart_delay_ms(u32::MAX), MAX_RESTART_DELAY_MS);
    }

    #[test]
    fn downloading_without_a_version_falls_through_and_a_missing_percent_has_no_suffix() {
        let mut state = update(Status::Downloading);
        state.enabled = false;
        assert_eq!(resolve_offer(&state, None, false), RecoveryOffer::Releases);
        state.available_version = Some("1.3.0".into());
        let offer = resolve_offer(&state, None, false);
        assert_eq!(
            offer,
            RecoveryOffer::Downloading {
                version: "1.3.0".into(),
                percent: None
            }
        );
        assert_eq!(
            offer_text(&offer),
            "Downloading 1.3.0. T3 Code restarts when the download is done."
        );
        state.download_percent = Some(WireNumber::Finite(9.into()));
        assert!(offer_text(&resolve_offer(&state, None, false)).contains("(0%)"));
    }

    fn failing(reason: &str) -> StartupEvent {
        StartupEvent::Failing(StartupFailure::exited(reason, false, ""))
    }

    #[test]
    fn the_first_failure_shows_the_window_checks_for_an_update_once_and_is_never_replaced() {
        let mut recovery = StartupRecovery::default();
        let first = recovery.on_event(failing("first"), false);
        assert!(first.show_window && first.check_for_update && !first.close_window);
        let later = recovery.on_event(failing("second"), false);
        assert_eq!(later, StartupEffects::default());
        assert_eq!(
            recovery.failure().unwrap().reason,
            "The server could not start (first)."
        );
        // Ready clears the failure and closes the window once the main window is visible.
        let ready = recovery.on_event(StartupEvent::Ready, true);
        assert!(ready.close_window && ready.record_started_version && recovery.failure().is_none());
        // A later run of failures shows again but does not ask the updater a second time.
        let again = recovery.on_event(failing("third"), false);
        assert!(again.show_window && !again.check_for_update);
    }

    #[test]
    fn a_server_that_dies_after_starting_never_covers_a_visible_main_window() {
        let mut recovery = StartupRecovery::default();
        recovery.on_event(StartupEvent::Ready, false);
        let effects = recovery.on_event(failing("late"), true);
        assert!(
            !effects.show_window,
            "main window is visible after a successful start"
        );
        assert!(
            effects.check_for_update,
            "the update check still runs, as in TypeScript"
        );
        assert!(!recovery.on_activate(true));
        assert!(recovery.on_activate(false));
    }

    #[test]
    fn activate_without_a_failure_shows_nothing_and_the_tag_url_matches_typescript() {
        assert!(!StartupRecovery::default().on_activate(false));
        assert_eq!(
            release_tag_url("1.1.0"),
            "https://github.com/pingdotgg/t3code/releases/tag/v1.1.0"
        );
    }
}
