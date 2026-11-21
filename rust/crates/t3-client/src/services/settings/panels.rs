//! Port of apps/web/src/components/settings/SettingsPanels.logic.ts (project grouping toggle,
//! auto-settle days field, changed-label helpers) and `useSettingsRestore`'s label list and
//! reset patch from SettingsPanels.tsx.
use super::scoped::SettingsPatch;
use serde_json::Value;
use t3_contracts::settings::{ClientSettingsSchema, ServerSettings};

/// `isProjectGroupingEnabled`.
pub fn is_project_grouping_enabled(mode: &str) -> bool {
    mode != "separate"
}

/// `projectGroupingModeFromToggle`.
pub fn project_grouping_mode_from_toggle(enabled: bool, last_enabled: &str) -> &'static str {
    if !enabled {
        "separate"
    } else if last_enabled == "repository_path" {
        "repository_path"
    } else {
        "repository"
    }
}

/// `LAST_ENABLED_PROJECT_GROUPING_MODE_KEY`, a raw value in ordinary storage.
pub const LAST_ENABLED_PROJECT_GROUPING_MODE_KEY: &str =
    "t3code:last-enabled-project-grouping-mode";

/// `readLastEnabledProjectGroupingMode` over the stored bytes.
pub fn last_enabled_project_grouping_mode(stored: Option<&[u8]>) -> &'static str {
    if stored == Some(b"repository_path".as_slice()) {
        "repository_path"
    } else {
        "repository"
    }
}

pub const MIN_SIDEBAR_AUTO_SETTLE_AFTER_DAYS: i64 = 1;
pub const MAX_SIDEBAR_AUTO_SETTLE_AFTER_DAYS: i64 = 90;

/// `AutoSettleDaysField`: typing stays in `draft` until the edit is finished, because saving a
/// shorter age settles threads at once. `pending` is the submission in flight; `saved` is the
/// only value a draft falls back to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoSettleDaysField {
    pub saved: i64,
    pub draft: String,
    pub dirty: bool,
    pub pending: Option<i64>,
    pub revision: u64,
}

impl AutoSettleDaysField {
    /// `autoSettleDaysField`.
    pub fn new(saved: i64) -> Self {
        Self {
            saved,
            draft: saved.to_string(),
            dirty: false,
            pending: None,
            revision: 0,
        }
    }

    /// `syncAutoSettleDaysField`: an earlier save can be confirmed while a later one is in flight.
    pub fn sync(&self, saved: i64) -> Self {
        let pending = if self.pending == Some(saved) {
            None
        } else {
            self.pending
        };
        if self.saved == saved && pending == self.pending {
            return self.clone();
        }
        Self {
            saved,
            pending,
            draft: if self.dirty {
                self.draft.clone()
            } else {
                pending.unwrap_or(saved).to_string()
            },
            ..self.clone()
        }
    }

    /// `editAutoSettleDaysField`.
    pub fn edit(&self, draft: &str) -> Self {
        Self {
            draft: draft.to_owned(),
            dirty: true,
            ..self.clone()
        }
    }

    /// `cancelAutoSettleDaysField`.
    pub fn cancel(&self) -> Self {
        Self {
            draft: self.pending.unwrap_or(self.saved).to_string(),
            dirty: false,
            ..self.clone()
        }
    }

    /// `commitAutoSettleDaysField`: the next field and the days to submit, if any.
    pub fn commit(&self) -> (Self, Option<i64>) {
        let current = self.pending.unwrap_or(self.saved);
        let days = parse_js_integer(self.draft.trim());
        let Some(days) = days.filter(|days| {
            (MIN_SIDEBAR_AUTO_SETTLE_AFTER_DAYS..=MAX_SIDEBAR_AUTO_SETTLE_AFTER_DAYS).contains(days)
        }) else {
            return (
                Self {
                    draft: current.to_string(),
                    dirty: false,
                    ..self.clone()
                },
                None,
            );
        };
        let settled = Self {
            draft: days.to_string(),
            dirty: false,
            ..self.clone()
        };
        // An edit ending on the saved value is sent again: a save can land on the shown
        // environment and fail on another, and retyping the number is how it is retried.
        if !self.dirty || Some(days) == self.pending {
            return (settled, None);
        }
        (
            Self {
                pending: Some(days),
                revision: self.revision + 1,
                ..settled
            },
            Some(days),
        )
    }

    /// `failAutoSettleDaysField`.
    pub fn fail(&self, revision: u64) -> Self {
        if self.revision != revision {
            return self.clone();
        }
        Self {
            pending: None,
            draft: if self.dirty {
                self.draft.clone()
            } else {
                self.saved.to_string()
            },
            ..self.clone()
        }
    }
}

/// `Number(text)` restricted to `Number.isInteger`: "7", "07", "7.0", "1e1" are integers.
fn parse_js_integer(text: &str) -> Option<i64> {
    if text.is_empty() {
        return None;
    }
    let value: f64 = text.parse().ok()?;
    (value.is_finite() && value.fract() == 0.0 && value.abs() < 9.0e15).then_some(value as i64)
}

/// `DEFAULT_UNIFIED_SETTINGS` as wire JSON.
pub fn default_unified_settings() -> Value {
    let mut value = serde_json::to_value(ClientSettingsSchema::default()).unwrap_or_default();
    if let (Some(fields), Some(server)) = (
        value.as_object_mut(),
        serde_json::to_value(ServerSettings::default())
            .ok()
            .and_then(|v| v.as_object().cloned()),
    ) {
        fields.extend(server);
    }
    value
}

/// `mergeEnvironmentSettings` as wire JSON: server values win over client keys.
pub fn unified_settings(server: &ServerSettings, client: &ClientSettingsSchema) -> Value {
    let mut value = serde_json::to_value(client).unwrap_or_default();
    if let (Some(fields), Some(server)) = (
        value.as_object_mut(),
        serde_json::to_value(server)
            .ok()
            .and_then(|v| v.as_object().cloned()),
    ) {
        fields.extend(server);
    }
    value
}

/// `useSettingsRestore`'s settings keys in order, with the label each contributes. Theme,
/// follow-system and theme mix come from the theme store, which is not a client service yet.
const RESTORE_LABELS: &[(&str, &str)] = &[
    ("appearanceContrast", "Contrast"),
    ("glassOpacity", "Glass opacity"),
    ("diffColorScheme", "Diff colors"),
    ("chatWidth", "Chat width"),
    ("effortDisplay", "Effort display"),
    ("mathRenderingEnabled", "Render math"),
    ("panelAnimationDurationMs", "Panel animations"),
    (
        "environmentIdentificationMode",
        "Environment identification",
    ),
    ("language", "Language"),
    ("timestampFormat", "Time format"),
    ("notificationMode", "Thread notifications"),
    ("inAppNotificationsEnabled", "In-app notifications"),
    ("sidebarProjectGroupingMode", "Project Grouping"),
    ("sidebarProjectSortOrder", "Project order"),
    ("sidebarWorkingShelfEnabled", "Working section"),
    ("sidebarAutoSettleAfterDays", "Auto-settle inactive threads"),
    (
        "sidebarAutoSettleKeepPullRequests",
        "Keep threads with a PR",
    ),
    ("sidebarAutoSettleOnMerge", "Auto-settle merged threads"),
    ("autoResumeLimitedThreads", "Resume after usage limits"),
    ("snoozeLimitedThreads", "Snooze until limits reset"),
    ("snoozePresets", "Snooze options"),
    ("wordWrap", "Word wrap"),
    ("persistComposerContextStrip", "Composer context"),
];
const RESTORE_LABELS_AFTER_TYPOGRAPHY: &[(&str, &str)] = &[
    ("diffFilesCollapsed", "Default diff file state"),
    ("diffIgnoreWhitespace", "Diff whitespace changes"),
    ("diffLayout", "Diff layout"),
    (
        "proactivePanelsEnabled",
        "Open pull requests and changes panels proactively",
    ),
    ("showSkillsInSlashMenu", "Show skills in slash menu"),
    ("composerRichTextEnabled", "Rich text composer"),
    ("sendShortcut", "Send shortcut"),
    ("followUpBehavior", "Follow-up behavior"),
    ("toolCallVisibility", "Tool calls in chat"),
    ("screenReaderAnnouncements", "Screen reader announcements"),
    ("screenReaderWindowTitle", "Screen reader window title"),
    ("responseStreamingMode", "Response streaming"),
    ("enableProviderUpdateChecks", "Provider update checks"),
    (
        "continueThreadsAfterServerUpdate",
        "Continue threads after restarts",
    ),
];
const RESTORE_LABELS_AFTER_BACKGROUND: &[(&str, &str)] = &[
    ("keepAwakeWhileAgentsWork", "Keep awake while agents work"),
    ("defaultThreadEnvMode", "New thread mode"),
    (
        "newWorktreesStartFromOrigin",
        "New worktrees start from remote",
    ),
    ("addProjectBaseDirectory", "Add project base directory"),
    ("ghqCloneLayout", "Clone into host folders"),
    ("confirmThreadUnpin", "Unpin confirmation"),
    ("confirmThreadArchive", "Archive confirmation"),
    ("confirmThreadDelete", "Delete confirmation"),
    ("confirmQuit", "Quit shortcut"),
    ("textGenerationModelSelection", "Text generation model"),
    ("threadTitleInstructions", "Thread title instructions"),
];

/// The keys `useSettingsRestore` resets, in its patch order.
const RESTORE_PATCH_KEYS: &[&str] = &[
    "appearanceContrast",
    "diffColorScheme",
    "chatWidth",
    "effortDisplay",
    "mathRenderingEnabled",
    "language",
    "timestampFormat",
    "notificationMode",
    "inAppNotificationsEnabled",
    "wordWrap",
    "persistComposerContextStrip",
    "diffFilesCollapsed",
    "diffIgnoreWhitespace",
    "diffLayout",
    "proactivePanelsEnabled",
    "showSkillsInSlashMenu",
    "composerRichTextEnabled",
    "sendShortcut",
    "followUpBehavior",
    "toolCallVisibility",
    "screenReaderAnnouncements",
    "screenReaderWindowTitle",
    "environmentIdentificationMode",
    "glassOpacity",
    "panelAnimationDurationMs",
    "sidebarProjectGroupingMode",
    "sidebarProjectSortOrder",
    "sidebarWorkingShelfEnabled",
    "sidebarAutoSettleAfterDays",
    "sidebarAutoSettleKeepPullRequests",
    "sidebarAutoSettleOnMerge",
    "autoResumeLimitedThreads",
    "snoozeLimitedThreads",
    "snoozePresets",
    "responseStreamingMode",
    "enableProviderUpdateChecks",
    "continueThreadsAfterServerUpdate",
    "backgroundActivity",
    "keepAwakeWhileAgentsWork",
    "backgroundActivityProfile",
    "automaticGitFetchInterval",
    "providerHealthRefreshInterval",
    "defaultThreadEnvMode",
    "newWorktreesStartFromOrigin",
    "addProjectBaseDirectory",
    "ghqCloneLayout",
    "confirmThreadArchive",
    "confirmThreadDelete",
    "confirmThreadUnpin",
    "confirmQuit",
    "textGenerationModelSelection",
    "threadTitleInstructions",
    "fontFamilySans",
    "fontFamilyComposer",
    "fontFamilyCode",
    "fontFamilyTerminal",
    "fontSizeInterface",
    "fontSizePrompt",
    "fontSizeCode",
    "fontSizeTerminal",
    "browserDefaultViewport",
    "browserDefaultZoomFactor",
    "browserDefaultAppearance",
    "browserRecordingFrameRate",
    "browserRecordingShowKeyPresses",
    "browserRecordingShowMousePresses",
    "browserLinkTarget",
    "browserSearchEngine",
    "browserCustomSearchUrl",
    "browserAutoShowFloatingPreview",
    "pullRequestLinksOpenInApp",
    "enableAgentBrowserAccess",
];

fn differs(settings: &Value, defaults: &Value, key: &str) -> bool {
    settings.get(key).unwrap_or(&Value::Null) != defaults.get(key).unwrap_or(&Value::Null)
}

/// `getChangedTypographySettingLabels`.
pub fn changed_typography_setting_labels(settings: &Value, defaults: &Value) -> Vec<&'static str> {
    [
        ("fontFamilySans", "fontSizeInterface", "Interface font"),
        ("fontFamilyComposer", "fontSizePrompt", "Prompt font"),
        ("fontFamilyCode", "fontSizeCode", "Code font"),
        ("fontFamilyTerminal", "fontSizeTerminal", "Terminal font"),
    ]
    .into_iter()
    .filter(|(family, size, _)| {
        differs(settings, defaults, family) || differs(settings, defaults, size)
    })
    .map(|(_, _, label)| label)
    .collect()
}

/// `isSamePreviewViewport`.
fn same_preview_viewport(left: &Value, right: &Value) -> bool {
    let tag = |v: &Value| v["_tag"].as_str().map(str::to_owned);
    if tag(left) != tag(right) {
        return false;
    }
    if tag(left).as_deref() == Some("fill") {
        return true;
    }
    if left["width"] != right["width"] || left["height"] != right["height"] {
        return false;
    }
    if tag(left).as_deref() == Some("preset") {
        left["presetId"] == right["presetId"]
    } else {
        true
    }
}

/// `getChangedBrowserSettingLabels`.
pub fn changed_browser_setting_labels(settings: &Value, defaults: &Value) -> Vec<&'static str> {
    let mut labels = Vec::new();
    if !same_preview_viewport(
        &settings["browserDefaultViewport"],
        &defaults["browserDefaultViewport"],
    ) {
        labels.push("Browser viewport");
    }
    for (key, label) in [
        ("browserDefaultZoomFactor", "Browser zoom"),
        ("browserDefaultAppearance", "Browser appearance"),
        ("browserRecordingFrameRate", "Recording frame rate"),
    ] {
        if differs(settings, defaults, key) {
            labels.push(label);
        }
    }
    if settings["browserRecordingShowKeyPresses"] == Value::Bool(true) {
        labels.push("Recording key presses");
    }
    if settings["browserRecordingShowMousePresses"] == Value::Bool(true) {
        labels.push("Recording mouse presses");
    }
    if differs(settings, defaults, "browserLinkTarget") {
        labels.push("Open links in");
    }
    if differs(settings, defaults, "browserSearchEngine")
        || differs(settings, defaults, "browserCustomSearchUrl")
    {
        labels.push("Search engine");
    }
    if differs(settings, defaults, "browserAutoShowFloatingPreview") {
        labels.push("Floating preview");
    }
    if differs(settings, defaults, "pullRequestLinksOpenInApp") {
        labels.push("Open pull requests in T3 Code");
    }
    labels
}

/// `hasChangedBackgroundActivitySettings`.
pub fn has_changed_background_activity_settings(settings: &Value, defaults: &Value) -> bool {
    [
        "backgroundActivity",
        "backgroundActivityProfile",
        "automaticGitFetchInterval",
        "providerHealthRefreshInterval",
    ]
    .iter()
    .any(|key| differs(settings, defaults, key))
}

/// `useSettingsRestore().changedSettingLabels` without the theme store's three labels.
pub fn changed_setting_labels(settings: &Value) -> Vec<&'static str> {
    let defaults = default_unified_settings();
    let pick = |table: &[(&str, &'static str)]| {
        table
            .iter()
            .filter(|(key, _)| differs(settings, &defaults, key))
            .map(|(_, label)| *label)
            .collect::<Vec<_>>()
    };
    let mut labels = pick(RESTORE_LABELS);
    labels.extend(changed_typography_setting_labels(settings, &defaults));
    labels.extend(pick(RESTORE_LABELS_AFTER_TYPOGRAPHY));
    if has_changed_background_activity_settings(settings, &defaults) {
        labels.push("Background activity");
    }
    labels.extend(pick(RESTORE_LABELS_AFTER_BACKGROUND));
    labels.extend(changed_browser_setting_labels(settings, &defaults));
    if differs(settings, &defaults, "enableAgentBrowserAccess") {
        labels.push("Agent browser access");
    }
    labels
}

/// The patch `restoreDefaults` sends.
pub fn restore_defaults_patch() -> SettingsPatch {
    let defaults = default_unified_settings();
    RESTORE_PATCH_KEYS
        .iter()
        .filter_map(|key| {
            defaults
                .get(*key)
                .map(|value| ((*key).to_owned(), value.clone()))
        })
        .collect()
}

/// `reconcileSnoozeDraft`: reads a server value against the editor's writes still in flight.
/// Matching one acknowledges it and every earlier write (`Some(remaining)`); anything else came
/// `MAX_SNOOZE_PRESETS` and `MAX_SNOOZE_PRESET_MINUTES`.
pub const MAX_SNOOZE_PRESETS: usize = 8;
pub const MAX_SNOOZE_PRESET_MINUTES: i64 = 30 * 24 * 60;

/// `UNIT_MINUTES`.
pub fn snooze_unit_minutes(unit: &str) -> i64 {
    match unit {
        "days" => 24 * 60,
        "hours" => 60,
        _ => 1,
    }
}

/// `splitMinutes`: the largest whole unit.
pub fn split_snooze_minutes(minutes: i64) -> (i64, &'static str) {
    if minutes % (24 * 60) == 0 {
        (minutes / (24 * 60), "days")
    } else if minutes % 60 == 0 {
        (minutes / 60, "hours")
    } else {
        (minutes, "minutes")
    }
}

fn snooze_setting(value: Value) -> Option<t3_contracts::settings::SnoozePresetSetting> {
    serde_json::from_value(value).ok()
}

/// `nextCandidate`: the first of `ADD_CANDIDATES` not already listed, else one hour.
pub fn next_snooze_candidate(
    presets: &[t3_contracts::settings::SnoozePresetSetting],
) -> t3_contracts::settings::SnoozePresetSetting {
    use crate::thread_views::shelves::snooze_preset_id;
    let used: Vec<String> = presets.iter().map(snooze_preset_id).collect();
    let mut candidates = ServerSettings::default().snooze_presets;
    candidates.extend(
        [
            serde_json::json!({"kind": "after", "minutes": 30}),
            serde_json::json!({"kind": "at", "day": "saturday", "time": "09:00"}),
            serde_json::json!({"kind": "after", "minutes": 24 * 60}),
        ]
        .into_iter()
        .filter_map(snooze_setting),
    );
    candidates
        .into_iter()
        .find(|candidate| !used.contains(&snooze_preset_id(candidate)))
        .or_else(|| snooze_setting(serde_json::json!({"kind": "after", "minutes": 60})))
        .expect("a valid snooze preset")
}

/// `sameSnoozePresets`: row order is display order only.
pub fn same_snooze_presets(
    left: &[t3_contracts::settings::SnoozePresetSetting],
    right: &[t3_contracts::settings::SnoozePresetSetting],
) -> bool {
    use crate::thread_views::shelves::snooze_preset_id;
    let ids = |presets: &[t3_contracts::settings::SnoozePresetSetting]| {
        let mut ids: Vec<String> = presets.iter().map(snooze_preset_id).collect();
        ids.sort();
        ids
    };
    ids(left) == ids(right)
}

/// `withWhen`: a preset moved to another "When", keeping the other kind's last value.
pub fn snooze_with_when(
    preset: &t3_contracts::settings::SnoozePresetSetting,
    when: &str,
    memory_minutes: i64,
    memory_time: &str,
) -> Option<t3_contracts::settings::SnoozePresetSetting> {
    let current = serde_json::to_value(preset).ok()?;
    if when == "after" {
        return if current["kind"] == "after" {
            Some(preset.clone())
        } else {
            snooze_setting(serde_json::json!({"kind": "after", "minutes": memory_minutes}))
        };
    }
    let time = if current["kind"] == "at" {
        current["time"].as_str().unwrap_or(memory_time).to_owned()
    } else {
        memory_time.to_owned()
    };
    snooze_setting(serde_json::json!({"kind": "at", "day": when, "time": time}))
}

/// from elsewhere and replaces the draft (`None`).
pub fn reconcile_snooze_draft<T: PartialEq + Clone>(pending: &[T], saved: &T) -> Option<Vec<T>> {
    pending
        .iter()
        .position(|snapshot| snapshot == saved)
        .map(|acknowledged| pending[acknowledged + 1..].to_vec())
}

/// `sortSnoozeRows`: soonest wake first, options with no wake right now (a passed "Today")
/// last; ties keep their order.
pub fn sort_snooze_rows<K: Clone>(
    presets: &[t3_contracts::settings::SnoozePresetSetting],
    keys: &[K],
    calendar: &crate::thread_views::shelves::SnoozeCalendar<'_>,
) -> (Vec<t3_contracts::settings::SnoozePresetSetting>, Vec<K>) {
    use crate::thread_views::shelves::resolve_snooze_preset;
    let mut order: Vec<(usize, Option<String>)> = presets
        .iter()
        .enumerate()
        .map(|(index, preset)| (index, resolve_snooze_preset(preset, calendar).map(|p| p.snoozed_until)))
        .collect();
    order.sort_by(|a, b| match (&a.1, &b.1) {
        (None, None) => std::cmp::Ordering::Equal,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (Some(_), None) => std::cmp::Ordering::Less,
        (Some(a), Some(b)) => a.cmp(b),
    });
    (
        order.iter().map(|(index, _)| presets[*index].clone()).collect(),
        order.iter().map(|(index, _)| keys[*index].clone()).collect(),
    )
}

/// Port of shared/path.ts `resolvePastedPath`: accept a pasted host path even after
/// a prefilled directory, while leaving ordinary typing and relative text alone.
pub fn resolve_pasted_path(previous: &str, next: &str) -> String {
    use crate::view_logic::session::links::{is_absolute, is_windows_absolute};

    fn host_path(value: &str) -> bool {
        is_absolute(value) || value.starts_with("~/")
    }

    if next
        .strip_prefix(previous)
        .is_some_and(|suffix| suffix.encode_utf16().count() == 1)
        || previous.starts_with(next)
    {
        return next.to_owned();
    }

    let trimmed = next.trim_end();
    if let Some(quote @ ('\'' | '"')) = trimmed.chars().last() {
        let quoted = &trimmed[..trimmed.len() - 1];
        for (index, _) in quoted.match_indices(quote) {
            let inner = &quoted[index + 1..];
            if inner.is_empty() || !previous.starts_with(quoted[..index].trim_end()) {
                continue;
            }
            let candidate = if quote == '\'' {
                if inner.split("'\\''").any(|part| part.contains('\'')) {
                    continue;
                }
                inner.replace("'\\''", "'")
            } else {
                if inner.contains('"') {
                    continue;
                }
                inner.to_owned()
            };
            if host_path(&candidate) {
                return candidate;
            }
        }
    }

    let path = if previous.ends_with(['/', '\\']) {
        next.strip_prefix(previous).unwrap_or(next)
    } else {
        next
    };
    if !host_path(path) {
        return next.to_owned();
    }
    if is_windows_absolute(path) {
        return path.to_owned();
    }

    let mut chars = path.chars().peekable();
    let mut decoded = String::with_capacity(path.len());
    while let Some(ch) = chars.next() {
        if ch == '\\'
            && chars
                .peek()
                .is_some_and(|ch| ch.is_whitespace() || r#"'"\()[]{}<>|&;$`!*?#~^="#.contains(*ch))
            && let Some(escaped) = chars.next()
        {
            decoded.push(escaped);
        } else {
            decoded.push(ch);
        }
    }
    decoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snooze_editor_helpers_follow_the_web() {
        assert_eq!(split_snooze_minutes(2 * 1440), (2, "days"));
        assert_eq!(split_snooze_minutes(90), (90, "minutes"));
        assert_eq!(split_snooze_minutes(120), (2, "hours"));
        let defaults = ServerSettings::default().snooze_presets;
        let next = next_snooze_candidate(&defaults);
        assert!(!defaults.contains(&next), "the first unused candidate");
        let mut reversed = defaults.clone();
        reversed.reverse();
        assert!(same_snooze_presets(&defaults, &reversed));
        let at: t3_contracts::settings::SnoozePresetSetting =
            serde_json::from_value(serde_json::json!({"kind": "at", "day": "today", "time": "18:00"})).expect("preset");
        let after = snooze_with_when(&at, "after", 45, "09:00").expect("after");
        assert_eq!(serde_json::to_value(&after).unwrap_or_default(), serde_json::json!({"kind": "after", "minutes": 45}));
        let friday = snooze_with_when(&at, "friday", 45, "09:00").expect("friday");
        assert_eq!(serde_json::to_value(&friday).unwrap_or_default()["time"], "18:00");
        assert!(snooze_with_when(&at, "someday", 45, "09:00").is_none());
    }

    #[test]
    fn snooze_drafts_keep_our_echoes_and_adopt_values_from_elsewhere() {
        assert_eq!(reconcile_snooze_draft(&["a", "b"], &"a"), Some(vec!["b"]));
        assert_eq!(reconcile_snooze_draft(&["b"], &"b"), Some(vec![]));
        assert_eq!(reconcile_snooze_draft(&["b"], &"c"), None);
        // "a" was written and acknowledged before; seeing it again means someone restored it.
        assert_eq!(reconcile_snooze_draft::<&str>(&[], &"a"), None);
        // Every earlier write is acknowledged when a later one echoes.
        assert_eq!(reconcile_snooze_draft(&["a", "b", "a"], &"b"), Some(vec!["a"]));
    }

    #[test]
    fn snooze_rows_sort_by_wake_and_put_a_passed_today_option_last() {
        use crate::thread_views::shelves::SnoozeCalendar as Calendar;
        use t3_contracts::settings::SnoozePresetSetting;
        // 16:00 on a Thursday in a fixed UTC zone: 20 hours wakes tomorrow and 09:00 has passed.
        let now = 1_793_232_000_000_i64 + 16 * 3_600_000;
        let day = 86_400_000_i64;
        let local_wake = |days: i64, hour: u32, minute: u32| {
            Some(now.div_euclid(day) * day + days * day + i64::from(hour) * 3_600_000 + i64::from(minute) * 60_000)
        };
        let same_day = |a: i64, b: i64| a.div_euclid(day) == b.div_euclid(day);
        let format = |ms: i64| ms.to_string();
        let iso = |ms: i64| crate::time::iso_from_epoch_ms(ms as u64);
        let calendar = Calendar {
            now_ms: now,
            weekday: 4,
            local_wake: &local_wake,
            same_day: &same_day,
            format_time: &format,
            short_weekday: &format,
            to_iso: &iso,
        };
        let presets: Vec<SnoozePresetSetting> = serde_json::from_value(serde_json::json!([
            {"kind": "after", "minutes": 1200},
            {"kind": "at", "day": "today", "time": "09:00"},
            {"kind": "at", "day": "today", "time": "18:00"},
            {"kind": "after", "minutes": 60}
        ]))
        .expect("presets");
        let (_, keys) = sort_snooze_rows(&presets, &["20h", "passed", "evening", "1h"], &calendar);
        assert_eq!(keys, vec!["1h", "evening", "20h", "passed"]);
    }

    #[test]
    fn grouping_toggle_restores_the_last_enabled_mode() {
        assert!(!is_project_grouping_enabled("separate"));
        assert_eq!(
            project_grouping_mode_from_toggle(false, "repository_path"),
            "separate"
        );
        assert_eq!(
            project_grouping_mode_from_toggle(true, "repository_path"),
            "repository_path"
        );
        assert_eq!(
            project_grouping_mode_from_toggle(true, "separate"),
            "repository"
        );
        assert_eq!(
            last_enabled_project_grouping_mode(Some(b"repository_path")),
            "repository_path"
        );
        assert_eq!(
            last_enabled_project_grouping_mode(Some(b"garbage")),
            "repository"
        );
        assert_eq!(last_enabled_project_grouping_mode(None), "repository");
    }

    #[test]
    fn auto_settle_days_save_once_the_edit_finishes() {
        let field = AutoSettleDaysField::new(7);
        let typing = field.edit("3").edit("30");
        assert_eq!(typing.saved, 7);
        let (committed, submit) = typing.commit();
        assert_eq!(submit, Some(30));
        assert_eq!(committed.pending, Some(30));
        assert_eq!(committed.revision, 1);
        // Confirmation clears the pending submission.
        let confirmed = committed.sync(30);
        assert_eq!(
            (confirmed.saved, confirmed.pending, confirmed.draft.as_str()),
            (30, None, "30")
        );
    }

    #[test]
    fn invalid_or_unchanged_days_are_not_submitted() {
        let field = AutoSettleDaysField::new(7);
        for draft in ["", "0", "91", "2.5", "abc"] {
            let (next, submit) = field.edit(draft).commit();
            assert_eq!(submit, None, "{draft}");
            assert_eq!(next.draft, "7");
        }
        assert_eq!(field.commit().1, None);
        // Retyping the saved value retries it.
        assert_eq!(field.edit("7").commit().1, Some(7));
        assert_eq!(field.edit("1e1").commit().1, Some(10));
    }

    #[test]
    fn escape_cancels_and_a_failed_save_restores_the_saved_value_unless_superseded() {
        let field = AutoSettleDaysField::new(7).edit("12");
        assert_eq!(field.cancel().draft, "7");
        let (pending, _) = field.commit();
        let failed = pending.fail(pending.revision);
        assert_eq!((failed.pending, failed.draft.as_str()), (None, "7"));
        let (newer, _) = pending.edit("14").commit();
        assert_eq!(newer.fail(pending.revision), newer);
        let typing = pending.edit("20");
        assert_eq!(typing.fail(pending.revision).draft, "20");
        // A newer save can be in flight while an earlier one confirms.
        assert_eq!(newer.sync(12).pending, Some(14));
    }

    #[test]
    fn restore_lists_changed_settings_and_resets_them_to_defaults() {
        let mut settings = default_unified_settings();
        assert!(changed_setting_labels(&settings).is_empty());
        settings["confirmThreadDelete"] =
            Value::Bool(!settings["confirmThreadDelete"].as_bool().unwrap_or(true));
        settings["language"] = Value::String("fr".into());
        settings["browserRecordingShowKeyPresses"] = Value::Bool(true);
        assert_eq!(
            changed_setting_labels(&settings),
            vec!["Language", "Delete confirmation", "Recording key presses"]
        );
        let patch = restore_defaults_patch();
        assert_eq!(
            patch["confirmThreadDelete"],
            default_unified_settings()["confirmThreadDelete"]
        );
        assert!(patch.contains_key("sidebarAutoSettleAfterDays"));
    }
}
