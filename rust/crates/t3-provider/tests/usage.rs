#![allow(clippy::unwrap_used)]
use serde_json::json;
use std::sync::Arc;
use t3_contracts::provider_usage_limits::*;
use t3_provider::usage::*;

#[test]
fn percent_only_updates_keep_reset_metadata_and_noops_keep_published_identity() {
    let previous=Arc::new(serde_json::from_value::<ServerProviderUsageLimits>(json!({"checkedAt":"2026-09-03T12:00:00Z","windows":[{"id":"session","kind":"session","label":"Session","usedPercent":40,"resetsAt":"2026-09-03T14:00:00Z","windowDurationMins":300}]})).unwrap());
    let update = serde_json::from_value(
        json!({"windows":[{"id":"session","kind":"session","label":"Session","usedPercent":40}]}),
    )
    .unwrap();
    let same = apply_usage_update(Some(&previous), &update, "2026-09-03T12:00:05Z").unwrap();
    assert!(Arc::ptr_eq(&same, &previous));
    let update = serde_json::from_value(
        json!({"windows":[{"id":"session","kind":"session","label":"Session","usedPercent":150}]}),
    )
    .unwrap();
    let next = apply_usage_update(Some(&previous), &update, "2026-09-03T12:00:05Z").unwrap();
    assert_eq!(next.windows[0].resets_at, previous.windows[0].resets_at);
    assert_eq!(
        serde_json::to_value(&next.windows[0]).unwrap()["usedPercent"].as_f64(),
        Some(100.)
    );
    for (used, expected) in [
        (json!(-5), 0.),
        (json!(40.25), 40.25),
        (json!("NaN"), 0.),
        (json!("Infinity"), 0.),
        (json!("-Infinity"), 0.),
    ] {
        let update = serde_json::from_value(json!({"windows":[{
            "id":"session","kind":"session","label":"Session","usedPercent":used
        }]}))
        .unwrap();
        let next = apply_usage_update(Some(&previous), &update, "2026-09-03T12:00:05Z").unwrap();
        assert_eq!(
            serde_json::to_value(&next.windows[0]).unwrap()["usedPercent"].as_f64(),
            Some(expected)
        );
        assert_eq!(next.windows[0].resets_at, previous.windows[0].resets_at);
        assert_eq!(
            next.windows[0].window_duration_mins,
            previous.windows[0].window_duration_mins
        );
    }
    let empty = ProviderUsageLimitsUpdate { windows: vec![] };
    assert!(Arc::ptr_eq(
        &apply_usage_update(Some(&previous), &empty, "2026-09-03T12:00:05Z").unwrap(),
        &previous
    ));
    assert!(apply_usage_update(None, &empty, "2026-09-03T12:00:05Z").is_none());
}

#[test]
fn sparse_first_update_stays_unavailable_unsupported_stays_unsupported_and_latest_live_limits_win()
{
    let update = serde_json::from_value(
        json!({"windows":[{"id":"weekly","kind":"weekly","label":"Weekly","usedPercent":20}]}),
    )
    .unwrap();
    let first = apply_usage_update(None, &update, "2026-09-03T12:00:05Z").unwrap();
    assert_eq!(
        first.unavailable.as_ref().unwrap().as_ref().unwrap().reason,
        ServerProviderUsageUnavailableReason::ProbeFailed
    );
    let mut unsupported = (*first).clone();
    unsupported
        .unavailable
        .as_mut()
        .unwrap()
        .as_mut()
        .unwrap()
        .reason = ServerProviderUsageUnavailableReason::Unsupported;
    let unsupported = Arc::new(unsupported);
    assert!(Arc::ptr_eq(
        &apply_usage_update(Some(&unsupported), &update, "2026-09-03T12:00:10Z").unwrap(),
        &unsupported
    ));
    let published = Arc::new(
        serde_json::from_value::<ServerProviderUsageLimits>(
            json!({"checkedAt":"2026-09-03T12:00:10Z","windows":[]}),
        )
        .unwrap(),
    );
    assert!(Arc::ptr_eq(
        &usage_after_probe(Some(&published), Some(&first)).unwrap(),
        &published
    ));
    assert!(Arc::ptr_eq(
        &usage_after_probe(Some(&published), Some(&unsupported)).unwrap(),
        &unsupported
    ));
    let live = Arc::new(
        serde_json::from_value::<ServerProviderUsageLimits>(json!({
            "checkedAt":"2026-09-03T12:00:10Z",
            "windows":[{"id":"session","kind":"session","label":"Session","usedPercent":95}],
            "resetCredits":{"availableCount":2},
            "externalUsage":{"label":"Old dashboard","url":"https://example.com/old"}
        }))
        .unwrap(),
    );
    let probed = Arc::new(
        serde_json::from_value::<ServerProviderUsageLimits>(json!({
            "checkedAt":"2026-09-03T12:00:00Z",
            "windows":[{"id":"session","kind":"session","label":"Session","usedPercent":0}],
            "resetCredits":{"availableCount":1},
            "externalUsage":{"label":"Usage dashboard","url":"https://example.com/usage"}
        }))
        .unwrap(),
    );
    let next = usage_after_probe(Some(&live), Some(&probed)).unwrap();
    assert_eq!(next.checked_at, live.checked_at);
    assert_eq!(next.windows, live.windows);
    assert_eq!(next.reset_credits, probed.reset_credits);
    assert_eq!(next.external_usage, probed.external_usage);
    let mut same_time = (*probed).clone();
    same_time.checked_at = live.checked_at.clone();
    let same_time = Arc::new(same_time);
    assert!(Arc::ptr_eq(
        &usage_after_probe(Some(&live), Some(&same_time)).unwrap(),
        &same_time
    ));
}

#[test]
fn streamed_updates_sort_windows_track_scope_changes_and_preserve_reset_credits() {
    let previous = Arc::new(serde_json::from_value::<ServerProviderUsageLimits>(json!({
        "checkedAt":"2026-09-03T12:00:00Z",
        "windows":[{"id":"weekly","kind":"weekly","label":"Weekly","usedPercent":20,"modelIds":["opencode/a"],"providerIds":["opencode"]}],
        "resetCredits":{"availableCount":2}
    })).unwrap());
    let update = serde_json::from_value(json!({"windows":[{
        "id":"weekly","kind":"weekly","label":"Weekly","usedPercent":20.0,
        "modelIds":["opencode/a"],"providerIds":["opencode"]
    }]}))
    .unwrap();
    assert!(Arc::ptr_eq(
        &apply_usage_update(Some(&previous), &update, "2026-09-03T12:00:05Z").unwrap(),
        &previous
    ));
    for (field, values) in [
        ("modelIds", json!(["opencode/b"])),
        ("providerIds", json!(["console-custom"])),
    ] {
        let mut window = serde_json::to_value(&previous.windows[0]).unwrap();
        window[field] = values.clone();
        let update = serde_json::from_value(json!({"windows":[window]})).unwrap();
        let next = apply_usage_update(Some(&previous), &update, "2026-09-03T12:00:05Z").unwrap();
        assert_eq!(
            serde_json::to_value(&next.windows[0]).unwrap()[field],
            values
        );
        assert_eq!(next.reset_credits, previous.reset_credits);
    }
    let update = serde_json::from_value(json!({"windows":[
        {"id":"z","kind":"other","label":"Z","usedPercent":0},
        {"id":"monthly","kind":"monthly","label":"Monthly","usedPercent":0},
        {"id":"a","kind":"other","label":"A","usedPercent":0},
        {"id":"session","kind":"session","label":"Session","usedPercent":55}
    ]}))
    .unwrap();
    let next = apply_usage_update(Some(&previous), &update, "2026-09-03T12:00:05Z").unwrap();
    assert_eq!(
        next.windows
            .iter()
            .map(|w| w.id.0.as_str())
            .collect::<Vec<_>>(),
        ["session", "weekly", "monthly", "a", "z"]
    );
    assert_eq!(next.reset_credits, previous.reset_credits);
}
