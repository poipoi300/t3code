//! Port of apps/web/src/components/desktopUpdate.logic.ts: what the sidebar's update control
//! shows and does for the desktop app's own update state. The strings are English sentences the
//! UI translates; the versions and messages inside them are content.

use t3_contracts::ipc::{
    DesktopRuntimeArchSchema, DesktopUpdateActionResultSchema, DesktopUpdateStateSchema,
    DesktopUpdateStateSchemaErrorContextValue as ErrorContext, DesktopUpdateStatusSchema as Status,
};
use t3_contracts::settings::{WireNonFiniteNumber, WireNumber};

/// `DesktopUpdateButtonAction`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopUpdateButtonAction {
    Download,
    Install,
    None,
}

/// `getDesktopUpdateDownloadedVersion`: the download RPC can beat the updater's event, so fall
/// back to the requested version.
pub fn downloaded_version(state: &DesktopUpdateStateSchema) -> Option<&str> {
    state
        .downloaded_version
        .as_deref()
        .or(state.available_version.as_deref())
}

/// `resolveDesktopUpdateButtonAction`.
pub fn button_action(state: &DesktopUpdateStateSchema) -> DesktopUpdateButtonAction {
    let install_error = matches!(state.error_context, None | Some(ErrorContext::Install));
    if state.downloaded_version.is_some()
        && (state.status == Status::Downloaded || (state.status == Status::Error && install_error))
    {
        return DesktopUpdateButtonAction::Install;
    }
    if state.status == Status::Available {
        return DesktopUpdateButtonAction::Download;
    }
    if state.status == Status::Error
        && state.error_context == Some(ErrorContext::Download)
        && state.available_version.is_some()
    {
        return DesktopUpdateButtonAction::Download;
    }
    DesktopUpdateButtonAction::None
}

/// `shouldShowArm64IntelBuildWarning`.
pub fn shows_arm64_intel_build_warning(state: Option<&DesktopUpdateStateSchema>) -> bool {
    state.is_some_and(|state| {
        state.host_arch == DesktopRuntimeArchSchema::Arm64
            && state.app_arch == DesktopRuntimeArchSchema::X64
    })
}

/// `isDesktopUpdateButtonDisabled`.
pub fn button_disabled(state: Option<&DesktopUpdateStateSchema>) -> bool {
    state.is_some_and(|state| state.status == Status::Downloading)
}

/// `getArm64IntelBuildWarningDescription`: an English key.
pub fn arm64_intel_build_warning(state: &DesktopUpdateStateSchema) -> &'static str {
    if !shows_arm64_intel_build_warning(Some(state)) {
        return "This install is using the correct architecture.";
    }
    match button_action(state) {
        DesktopUpdateButtonAction::Download => {
            "This Mac has Apple Silicon, but T3 Code is still running the Intel build under Rosetta. Download the available update to switch to the native Apple Silicon build."
        }
        DesktopUpdateButtonAction::Install => {
            "This Mac has Apple Silicon, but T3 Code is still running the Intel build under Rosetta. Restart to install the downloaded Apple Silicon build."
        }
        DesktopUpdateButtonAction::None => {
            "This Mac has Apple Silicon, but T3 Code is still running the Intel build under Rosetta. The next app update will replace it with the native Apple Silicon build."
        }
    }
}

/// A sentence with its arguments: the key is English source text, the arguments content.
#[derive(Debug, Clone, PartialEq)]
pub struct Sentence {
    pub key: &'static str,
    pub args: Vec<(&'static str, String)>,
}

impl Sentence {
    fn new(key: &'static str) -> Self {
        Self {
            key,
            args: Vec::new(),
        }
    }

    fn with(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.args.push((name, value.into()));
        self
    }

    /// The English text, as the web renders it (for tests and logs).
    pub fn english(&self) -> String {
        let mut text = self.key.to_owned();
        for (name, value) in &self.args {
            text = text.replace(&format!("{{{name}}}"), value);
        }
        text
    }
}

/// `getDesktopUpdateButtonTooltip`. A server message (`state.message`) is content and comes
/// back as the `{message}` of "{message}".
pub fn button_tooltip(state: &DesktopUpdateStateSchema) -> Sentence {
    if let Some(not_applied) = &state.not_applied_version
        && matches!(state.status, Status::Available | Status::Downloaded)
        && downloaded_version(state) == Some(not_applied.as_str())
    {
        return Sentence::new("Update {version} did not apply. Click to retry.")
            .with("version", not_applied.clone());
    }
    match state.status {
        Status::Available => match &state.available_version {
            Some(version) => {
                Sentence::new("Update {version} ready to download").with("version", version.clone())
            }
            None => Sentence::new("Update available ready to download"),
        },
        Status::Downloading => match &state.download_percent {
            Some(WireNumber::Finite(percent)) => Sentence::new("Downloading update ({percent}%)")
                .with("percent", (percent.value().floor() as i64).to_string()),
            // `typeof NaN === "number"`: the web prints JavaScript's spelling.
            Some(WireNumber::NonFinite(value)) => Sentence::new("Downloading update ({percent}%)")
                .with(
                    "percent",
                    match value {
                        WireNonFiniteNumber::Infinity => "Infinity",
                        WireNonFiniteNumber::NegativeInfinity => "-Infinity",
                        WireNonFiniteNumber::NaN => "NaN",
                    },
                ),
            None => Sentence::new("Downloading update"),
        },
        Status::Downloaded => {
            match state
                .downloaded_version
                .as_ref()
                .or(state.available_version.as_ref())
            {
                Some(version) => {
                    Sentence::new("Update {version} downloaded. Click to restart and install.")
                        .with("version", version.clone())
                }
                None => Sentence::new("Update ready downloaded. Click to restart and install."),
            }
        }
        Status::Error => {
            if state.error_context == Some(ErrorContext::Download)
                && let Some(version) = &state.available_version
            {
                return Sentence::new("Download failed for {version}. Click to retry.")
                    .with("version", version.clone());
            }
            if state.error_context == Some(ErrorContext::Install)
                && let Some(version) = &state.downloaded_version
            {
                return Sentence::new("Install failed for {version}. Click to retry.")
                    .with("version", version.clone());
            }
            if let Some(version) = &state.downloaded_version {
                return Sentence::new("Update {version} downloaded. Click to restart and install.")
                    .with("version", version.clone());
            }
            match &state.message {
                Some(message) => Sentence::new("{message}").with("message", message.clone()),
                None => Sentence::new("Update failed"),
            }
        }
        _ => Sentence::new("Up to date"),
    }
}

/// `getDesktopUpdateInstallConfirmationMessage`.
pub fn install_confirmation(state: &DesktopUpdateStateSchema) -> Sentence {
    match state
        .downloaded_version
        .as_ref()
        .or(state.available_version.as_ref())
    {
        Some(version) => Sentence::new(
            "Install update {version} and restart T3 Code?\n\nAny running tasks will be interrupted. Make sure you're ready before continuing.",
        )
        .with("version", version.clone()),
        None => Sentence::new(
            "Install update and restart T3 Code?\n\nAny running tasks will be interrupted. Make sure you're ready before continuing.",
        ),
    }
}

/// `getDesktopUpdateActionError`: the updater's message when an accepted action did not
/// complete.
pub fn action_error(result: &DesktopUpdateActionResultSchema) -> Option<String> {
    if !result.accepted || result.completed {
        return None;
    }
    let message = result.state.message.as_deref()?.trim();
    (!message.is_empty()).then(|| message.to_owned())
}

/// `canCheckForUpdate`.
pub fn can_check(state: Option<&DesktopUpdateStateSchema>) -> bool {
    state.is_some_and(|state| {
        state.enabled
            && !matches!(
                state.status,
                Status::Checking | Status::Downloading | Status::Disabled
            )
    })
}

/// `getDesktopUpdateDisabledReason`. `managed_denial` (this machine's policy refusing updates)
/// wins. The returned text is a server or policy message (content) or the English fallback key.
pub fn disabled_reason(
    state: Option<&DesktopUpdateStateSchema>,
    managed_denial: Option<&str>,
) -> Option<DisabledReason> {
    if let Some(denial) = managed_denial {
        return Some(DisabledReason::Content(denial.to_owned()));
    }
    let state = state?;
    if state.enabled && state.status != Status::Disabled {
        return None;
    }
    Some(
        match state
            .message
            .as_deref()
            .map(str::trim)
            .filter(|message| !message.is_empty())
        {
            Some(message) => DisabledReason::Content(message.to_owned()),
            None => DisabledReason::Key("Automatic updates are not available in this build."),
        },
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisabledReason {
    Key(&'static str),
    Content(String),
}

/// `getDesktopUpdateIdleTooltip`: its title key and optional detail.
pub fn idle_tooltip(
    state: Option<&DesktopUpdateStateSchema>,
    managed_denial: Option<&str>,
) -> (&'static str, Option<DisabledReason>) {
    match disabled_reason(state, managed_denial) {
        Some(reason) => ("Updates unavailable", Some(reason)),
        None => ("Check for updates", None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use t3_contracts::ipc::DesktopUpdateChannelSchema;
    use t3_contracts::wire::JsNumber;

    fn base() -> DesktopUpdateStateSchema {
        DesktopUpdateStateSchema {
            enabled: true,
            status: Status::Idle,
            channel: DesktopUpdateChannelSchema::Latest,
            current_version: "1.0.0".into(),
            host_arch: DesktopRuntimeArchSchema::X64,
            app_arch: DesktopRuntimeArchSchema::X64,
            running_under_arm64_translation: false,
            available_version: None,
            downloaded_version: None,
            release_notes: Vec::new(),
            omitted_release_count: WireNumber::Finite(JsNumber::from_f64(0.0).expect("finite")),
            download_percent: None,
            checked_at: None,
            message: None,
            error_context: None,
            can_retry: false,
            not_applied_version: None,
        }
    }

    #[test]
    fn keeps_retry_action_available_after_a_download_error() {
        let state = DesktopUpdateStateSchema {
            status: Status::Error,
            available_version: Some("1.1.0".into()),
            message: Some("network timeout".into()),
            error_context: Some(ErrorContext::Download),
            can_retry: true,
            ..base()
        };
        assert_eq!(button_action(&state), DesktopUpdateButtonAction::Download);
        assert!(button_tooltip(&state).english().contains("Click to retry"));
    }

    #[test]
    fn prefers_a_newly_available_release_over_a_stale_downloaded_version() {
        let state = DesktopUpdateStateSchema {
            status: Status::Available,
            available_version: Some("1.2.0".into()),
            downloaded_version: Some("1.1.0".into()),
            ..base()
        };
        assert_eq!(button_action(&state), DesktopUpdateButtonAction::Download);
    }

    #[test]
    fn hides_the_install_action_while_checking_for_a_newer_release() {
        let state = DesktopUpdateStateSchema {
            status: Status::Checking,
            available_version: Some("1.1.0".into()),
            downloaded_version: Some("1.1.0".into()),
            download_percent: Some(WireNumber::Finite(
                JsNumber::from_f64(100.0).expect("finite"),
            )),
            ..base()
        };
        assert_eq!(button_action(&state), DesktopUpdateButtonAction::None);
    }

    #[test]
    fn can_check_once_an_update_is_downloaded_and_never_without_state() {
        let state = DesktopUpdateStateSchema {
            status: Status::Downloaded,
            available_version: Some("1.1.0".into()),
            downloaded_version: Some("1.1.0".into()),
            ..base()
        };
        assert!(can_check(Some(&state)));
        assert!(!can_check(None));
        assert_eq!(idle_tooltip(None, None), ("Check for updates", None));
        assert_eq!(
            idle_tooltip(
                Some(&DesktopUpdateStateSchema {
                    enabled: false,
                    ..base()
                }),
                None
            )
            .1,
            Some(DisabledReason::Key(
                "Automatic updates are not available in this build."
            ))
        );
        assert_eq!(
            idle_tooltip(None, Some("Your organization manages updates.")).1,
            Some(DisabledReason::Content(
                "Your organization manages updates.".into()
            ))
        );
    }

    #[test]
    fn tooltips_carry_versions_and_progress_as_arguments() {
        let downloading = DesktopUpdateStateSchema {
            status: Status::Downloading,
            download_percent: Some(WireNumber::Finite(
                JsNumber::from_f64(42.9).expect("finite"),
            )),
            ..base()
        };
        assert_eq!(
            button_tooltip(&downloading).english(),
            "Downloading update (42%)"
        );
        let not_applied = DesktopUpdateStateSchema {
            status: Status::Downloaded,
            downloaded_version: Some("1.3.0".into()),
            not_applied_version: Some("1.3.0".into()),
            ..base()
        };
        assert_eq!(
            button_tooltip(&not_applied).english(),
            "Update 1.3.0 did not apply. Click to retry."
        );
        assert_eq!(button_tooltip(&base()).english(), "Up to date");
    }
}
