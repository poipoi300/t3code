//! Port of the resolution half of `packages/shared/src/backgroundActivitySettings.ts`: the
//! effective background-activity settings of an environment, including the legacy top-level
//! profile and intervals that apply until a client writes `backgroundActivity`.
use std::time::Duration;
use t3_contracts::settings::{
    BackgroundActivityOverrides, BackgroundActivityProfile as Profile,
    BackgroundActivityProfileSelection as Selection, BackgroundActivitySettings, ServerSettings,
    WireNonFiniteNumber, WireNumber,
};

pub const DEFAULT_AUTOMATIC_GIT_FETCH_INTERVAL: Duration = Duration::from_secs(30);
pub const DEFAULT_PROVIDER_HEALTH_REFRESH_INTERVAL: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedBackgroundActivitySettings {
    pub profile: Profile,
    /// Zero disables background fetching; `Duration::MAX` stands for Effect's infinite duration.
    pub automatic_git_fetch_interval: Duration,
    pub provider_health_refresh_interval: Duration,
    pub host_power_monitor_active_interval: Duration,
    pub host_power_monitor_idle_interval: Duration,
    pub pause_when_host_locked: bool,
    pub pause_when_host_low_power: bool,
    pub pause_when_client_low_power: bool,
    pub pause_when_on_battery: bool,
}

/// Effect's `Duration.millis`: NaN and non-positive values are zero, +Infinity is infinite.
pub fn duration_from_millis(value: &WireNumber) -> Duration {
    match value {
        WireNumber::Finite(number) => {
            let millis = number.value();
            if millis > 0.0 {
                Duration::try_from_secs_f64(millis / 1000.0).unwrap_or(Duration::MAX)
            } else {
                Duration::ZERO
            }
        }
        WireNumber::NonFinite(WireNonFiniteNumber::Infinity) => Duration::MAX,
        WireNumber::NonFinite(_) => Duration::ZERO,
    }
}

pub fn preset(profile: &Profile) -> ResolvedBackgroundActivitySettings {
    let minutes = |m: u64| Duration::from_secs(m * 60);
    match profile {
        Profile::Performance => ResolvedBackgroundActivitySettings {
            profile: Profile::Performance,
            automatic_git_fetch_interval: Duration::from_secs(15),
            provider_health_refresh_interval: minutes(1),
            host_power_monitor_active_interval: Duration::from_secs(30),
            host_power_monitor_idle_interval: minutes(2),
            pause_when_host_locked: true,
            pause_when_host_low_power: false,
            pause_when_client_low_power: false,
            pause_when_on_battery: false,
        },
        Profile::Balanced => ResolvedBackgroundActivitySettings {
            profile: Profile::Balanced,
            automatic_git_fetch_interval: DEFAULT_AUTOMATIC_GIT_FETCH_INTERVAL,
            provider_health_refresh_interval: DEFAULT_PROVIDER_HEALTH_REFRESH_INTERVAL,
            host_power_monitor_active_interval: Duration::from_secs(30),
            host_power_monitor_idle_interval: minutes(5),
            pause_when_host_locked: true,
            pause_when_host_low_power: true,
            pause_when_client_low_power: true,
            pause_when_on_battery: false,
        },
        Profile::BatterySaver => ResolvedBackgroundActivitySettings {
            profile: Profile::BatterySaver,
            automatic_git_fetch_interval: Duration::ZERO,
            provider_health_refresh_interval: minutes(15),
            host_power_monitor_active_interval: minutes(1),
            host_power_monitor_idle_interval: minutes(10),
            pause_when_host_locked: true,
            pause_when_host_low_power: true,
            pause_when_client_low_power: true,
            pause_when_on_battery: true,
        },
    }
}

pub fn base_profile(activity: &BackgroundActivitySettings) -> Profile {
    match activity.profile {
        Selection::Custom => activity.base_profile.clone().unwrap_or(Profile::Balanced),
        Selection::Balanced => Profile::Balanced,
        Selection::Performance => Profile::Performance,
        Selection::BatterySaver => Profile::BatterySaver,
    }
}

pub fn resolve_background_activity_settings(
    activity: &BackgroundActivitySettings,
) -> ResolvedBackgroundActivitySettings {
    let base = base_profile(activity);
    let preset = preset(&base);
    let empty = BackgroundActivityOverrides::default();
    let overrides = if activity.profile == Selection::Custom {
        &activity.overrides
    } else {
        &empty
    };
    let interval = |value: &Option<WireNumber>, fallback: Duration| {
        value.as_ref().map_or(fallback, duration_from_millis)
    };
    ResolvedBackgroundActivitySettings {
        profile: base,
        automatic_git_fetch_interval: interval(
            &overrides.automatic_git_fetch_interval,
            preset.automatic_git_fetch_interval,
        ),
        provider_health_refresh_interval: interval(
            &overrides.provider_health_refresh_interval,
            preset.provider_health_refresh_interval,
        ),
        host_power_monitor_active_interval: interval(
            &overrides.host_power_monitor_active_interval,
            preset.host_power_monitor_active_interval,
        ),
        host_power_monitor_idle_interval: interval(
            &overrides.host_power_monitor_idle_interval,
            preset.host_power_monitor_idle_interval,
        ),
        pause_when_host_locked: overrides
            .pause_when_host_locked
            .unwrap_or(preset.pause_when_host_locked),
        pause_when_host_low_power: overrides
            .pause_when_host_low_power
            .unwrap_or(preset.pause_when_host_low_power),
        pause_when_client_low_power: overrides
            .pause_when_client_low_power
            .unwrap_or(preset.pause_when_client_low_power),
        pause_when_on_battery: overrides
            .pause_when_on_battery
            .unwrap_or(preset.pause_when_on_battery),
    }
}

/// `resolveServerBackgroundActivitySettings`: the legacy top-level profile and intervals apply
/// while `backgroundActivity` is still the untouched default.
pub fn resolve_server_background_activity_settings(
    settings: &ServerSettings,
) -> ResolvedBackgroundActivitySettings {
    let activity = &settings.background_activity;
    let activity_is_default = activity.profile == Selection::Balanced
        && activity.base_profile.is_none()
        && activity.overrides == BackgroundActivityOverrides::default();
    let legacy = &settings.background_activity_profile;
    let fetch = duration_from_millis(&settings.automatic_git_fetch_interval);
    let health = duration_from_millis(&settings.provider_health_refresh_interval);
    let has_legacy_overrides = *legacy != Profile::Balanced
        || fetch != DEFAULT_AUTOMATIC_GIT_FETCH_INTERVAL
        || health != DEFAULT_PROVIDER_HEALTH_REFRESH_INTERVAL;
    if !(activity_is_default && has_legacy_overrides) {
        return resolve_background_activity_settings(activity);
    }
    let legacy_preset = preset(legacy);
    let fetch_differs = fetch != legacy_preset.automatic_git_fetch_interval;
    let health_differs = health != legacy_preset.provider_health_refresh_interval;
    resolve_background_activity_settings(&BackgroundActivitySettings {
        profile: if fetch_differs || health_differs {
            Selection::Custom
        } else {
            match legacy {
                Profile::Balanced => Selection::Balanced,
                Profile::Performance => Selection::Performance,
                Profile::BatterySaver => Selection::BatterySaver,
            }
        },
        base_profile: Some(legacy.clone()),
        overrides: BackgroundActivityOverrides {
            automatic_git_fetch_interval: fetch_differs
                .then(|| settings.automatic_git_fetch_interval.clone()),
            provider_health_refresh_interval: health_differs
                .then(|| settings.provider_health_refresh_interval.clone()),
            ..Default::default()
        },
        ..activity.clone()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn settings(value: serde_json::Value) -> ServerSettings {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn defaults_resolve_to_the_balanced_preset() {
        let resolved = resolve_server_background_activity_settings(&settings(json!({})));
        assert_eq!(resolved, preset(&Profile::Balanced));
        assert_eq!(
            resolved.automatic_git_fetch_interval,
            DEFAULT_AUTOMATIC_GIT_FETCH_INTERVAL
        );
    }

    // serverSettings.test.ts: "turns legacy interval patches into custom background activity
    // overrides" (the stored shape after the patch).
    #[test]
    fn custom_overrides_replace_the_base_profile_interval() {
        let resolved = resolve_server_background_activity_settings(&settings(json!({
            "backgroundActivity": {
                "schemaVersion": 1,
                "profile": "custom",
                "baseProfile": "balanced",
                "overrides": {"automaticGitFetchInterval": 15000}
            }
        })));
        assert_eq!(resolved.profile, Profile::Balanced);
        assert_eq!(resolved.automatic_git_fetch_interval, Duration::from_secs(15));
    }

    #[test]
    fn overrides_apply_only_to_the_custom_profile() {
        let resolved = resolve_server_background_activity_settings(&settings(json!({
            "backgroundActivity": {
                "schemaVersion": 1,
                "profile": "battery-saver",
                "overrides": {"automaticGitFetchInterval": 15000}
            }
        })));
        assert_eq!(resolved.automatic_git_fetch_interval, Duration::ZERO);
        assert!(resolved.pause_when_on_battery);
    }

    // serverSettings.test.ts: "preserves legacy background activity settings when applying an
    // unrelated patch" (the legacy fields before any `backgroundActivity` was written).
    #[test]
    fn legacy_fields_apply_while_background_activity_is_untouched() {
        let resolved = resolve_server_background_activity_settings(&settings(json!({
            "backgroundActivityProfile": "performance",
            "automaticGitFetchInterval": 7000,
            "providerHealthRefreshInterval": 240000
        })));
        assert_eq!(resolved.profile, Profile::Performance);
        assert_eq!(resolved.automatic_git_fetch_interval, Duration::from_secs(7));
        assert_eq!(
            resolved.provider_health_refresh_interval,
            Duration::from_secs(240)
        );
        assert!(!resolved.pause_when_host_low_power);

        let preset_legacy = resolve_server_background_activity_settings(&settings(json!({
            "backgroundActivityProfile": "battery-saver",
            "automaticGitFetchInterval": 0,
            "providerHealthRefreshInterval": 900000
        })));
        assert_eq!(preset_legacy, preset(&Profile::BatterySaver));

        // A written `backgroundActivity` wins over the legacy fields.
        let written = resolve_server_background_activity_settings(&settings(json!({
            "backgroundActivity": {"schemaVersion": 1, "profile": "performance", "overrides": {}},
            "automaticGitFetchInterval": 7000
        })));
        assert_eq!(written.automatic_git_fetch_interval, Duration::from_secs(15));
    }

    #[test]
    fn durations_follow_effect_millis() {
        let finite = |n: f64| WireNumber::Finite(t3_contracts::wire::JsNumber::from_f64(n).unwrap());
        assert_eq!(duration_from_millis(&finite(-5.0)), Duration::ZERO);
        assert_eq!(duration_from_millis(&finite(1500.0)), Duration::from_millis(1500));
        assert_eq!(
            duration_from_millis(&WireNumber::NonFinite(WireNonFiniteNumber::Infinity)),
            Duration::MAX
        );
        assert_eq!(
            duration_from_millis(&WireNumber::NonFinite(WireNonFiniteNumber::NaN)),
            Duration::ZERO
        );
    }
}
