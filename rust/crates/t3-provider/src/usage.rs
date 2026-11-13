use crate::catalog::date_millis;
use std::sync::Arc;
use t3_contracts::{
    provider_usage_limits::*,
    speech::SpeechNumber,
    wire::{JsNumber, WireDateTime},
};

pub fn clamp_percent(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(0., 100.)
    } else {
        0.
    }
}
fn sort_windows(windows: &mut [ServerProviderUsageWindow]) {
    let rank = |kind: &ServerProviderUsageWindowKind| match kind {
        ServerProviderUsageWindowKind::Session => 0,
        ServerProviderUsageWindowKind::Weekly => 1,
        ServerProviderUsageWindowKind::Monthly => 2,
        ServerProviderUsageWindowKind::Other => 3,
    };
    windows.sort_by(|a, b| {
        rank(&a.kind)
            .cmp(&rank(&b.kind))
            .then_with(|| a.id.0.cmp(&b.id.0))
    });
}

pub fn apply_usage_update(
    previous: Option<&Arc<ServerProviderUsageLimits>>,
    update: &ProviderUsageLimitsUpdate,
    checked_at: &str,
) -> Option<Arc<ServerProviderUsageLimits>> {
    if update.windows.is_empty()
        || previous.is_some_and(|p| {
            p.unavailable
                .as_ref()
                .and_then(Option::as_ref)
                .is_some_and(|u| u.reason == ServerProviderUsageUnavailableReason::Unsupported)
        })
    {
        return previous.cloned();
    }
    let mut windows = previous.map(|p| p.windows.clone()).unwrap_or_default();
    let mut changed = false;
    for window in &update.windows {
        let index = windows.iter().position(|w| w.id == window.id);
        let mut next = window.clone();
        let value = match &window.used_percent {
            SpeechNumber::Finite(n) => n.as_f64().unwrap_or(0.),
            _ => 0.,
        };
        next.used_percent =
            SpeechNumber::Finite(JsNumber::from_f64(clamp_percent(value)).expect("finite percent"));
        if let Some(index) = index {
            if next.resets_at.is_none() {
                next.resets_at = windows[index].resets_at.clone();
            }
            if next.window_duration_mins.is_none() {
                next.window_duration_mins = windows[index].window_duration_mins;
            }
            if !window_equals(&windows[index], &next) {
                changed = true;
                windows[index] = next;
            }
        } else {
            changed = true;
            windows.push(next);
        }
    }
    if !changed && previous.is_some() {
        return previous.cloned();
    }
    sort_windows(&mut windows);
    Some(Arc::new(ServerProviderUsageLimits {
        checked_at: WireDateTime(checked_at.into()),
        windows,
        credential_fingerprint: None,
        reset_credits: previous.and_then(|p| p.reset_credits.clone()),
        external_usage: None,
        unavailable: previous.map(|p| p.unavailable.clone()).unwrap_or(Some(Some(
            ServerProviderUsageUnavailable {
                reason: ServerProviderUsageUnavailableReason::ProbeFailed,
                message: None,
            },
        ))),
    }))
}

fn window_equals(a: &ServerProviderUsageWindow, b: &ServerProviderUsageWindow) -> bool {
    let number = |value: &SpeechNumber| match value {
        SpeechNumber::Finite(n) => n.as_f64(),
        _ => None,
    };
    a.id == b.id
        && a.kind == b.kind
        && a.label == b.label
        && number(&a.used_percent) == number(&b.used_percent)
        && a.model_ids == b.model_ids
        && a.provider_ids == b.provider_ids
        && a.resets_at == b.resets_at
        && a.window_duration_mins == b.window_duration_mins
}

pub fn usage_after_probe(
    published: Option<&Arc<ServerProviderUsageLimits>>,
    probed: Option<&Arc<ServerProviderUsageLimits>>,
) -> Option<Arc<ServerProviderUsageLimits>> {
    let available =
        |p: &ServerProviderUsageLimits| p.unavailable.as_ref().is_none_or(Option::is_none);
    if let (Some(published), Some(probed)) = (published, probed) {
        if available(published)
            && probed
                .unavailable
                .as_ref()
                .and_then(Option::as_ref)
                .is_some_and(|u| u.reason == ServerProviderUsageUnavailableReason::ProbeFailed)
        {
            return Some(published.clone());
        }
        if available(published)
            && available(probed)
            && date_millis(&published.checked_at.0)
                .zip(date_millis(&probed.checked_at.0))
                .is_some_and(|(a, b)| a > b)
        {
            let mut next = probed.as_ref().clone();
            next.checked_at = published.checked_at.clone();
            next.windows = published.windows.clone();
            return Some(Arc::new(next));
        }
    }
    probed.cloned()
}
