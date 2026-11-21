use super::*;
use crate::keys::bindings::{Keybindings, Scope};
use t3_client::{
    composition::usage::{UsageMetric, UsagePreferences},
    services::usage::overview::provider_label,
};
use t3_contracts::usage::UsageProviderKind;

pub(super) fn bind<P: Platform + Clone + 'static>(client: &ClientRuntime<P>, page: Page) {
    let prefs = computed(move || {
        use UsageProviderKind::*;
        let hidden = page.hidden.get();
        let hidden_providers = [Codex, Claude, Grok, Cursor, Opencode, Antigravity, Pi, Muse]
            .into_iter()
            .filter(|p| hidden.contains(provider_label(p)))
            .collect();
        UsagePreferences {
            metric: match page.metric.get() {
                "tokens" => UsageMetric::Tokens,
                "cost" => UsageMetric::Cost,
                _ => UsageMetric::Limits,
            },
            window_days: page.days.get(),
            hidden_providers,
        }
    });
    let loaded = signal(false);
    let loader = client.clone();
    let saved = stored(None);
    watch(async move {
        let value = loader.usage_preferences().await;
        saved.replace(Some(value.clone()));
        batch(|| {
            if prefs.get_untracked() == UsagePreferences::default() {
                page.metric.set(match value.metric {
                    UsageMetric::Limits => "limits",
                    UsageMetric::Cost => "cost",
                    UsageMetric::Tokens => "tokens",
                });
                page.days.set(value.window_days);
                page.hidden.set(
                    value
                        .hidden_providers
                        .iter()
                        .map(|p| provider_label(p).to_owned())
                        .collect(),
                );
            }
            loaded.set(true);
        });
    });
    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    effect(move || {
        if loaded.get() {
            let next = prefs.get();
            if saved.get().as_ref() != Some(&next) {
                saved.replace(Some(next.clone()));
                let _ = tx.unbounded_send(next);
            }
        }
    });
    let writer = client.clone();
    watch(async move {
        while let Some(value) = rx.next().await {
            if let Err(e) = writer.save_usage_preferences(&value).await {
                log::warn!("Could not save Usage page preferences: {e}");
            }
        }
    });
}

pub(super) fn shortcuts(page: Page, prices: Signal<bool>) {
    let Some(keys) = use_context::<Keybindings>() else {
        return;
    };
    let mut scope = Scope {
        when: Rc::new(move |e| {
            !e.key.editable_focus
                && !e.inside_confirmation
                && !prices.get_untracked()
                && page.model.with_untracked(Option::is_none)
                && !keys.context.read().get("commandPaletteOpen")
                && !keys.context.read().get("modelPickerOpen")
        }),
        context: Rc::new(move || keys.context.read().with("usagePageOpen", true)),
        ..Default::default()
    };
    for (command, metric) in [
        ("usage.limits", "limits"),
        ("usage.cost", "cost"),
        ("usage.tokens", "tokens"),
    ] {
        scope = scope.command(command, move |e| {
            page.metric.set(metric);
            e.claim();
            true
        });
    }
    for (command, days) in [
        ("usage.period.day", 1),
        ("usage.period.week", 7),
        ("usage.period.month", 30),
        ("usage.period.quarter", 90),
    ] {
        scope = scope.command(command, move |e| {
            if page.metric.get_untracked() == "limits" {
                return false;
            }
            page.days.set(days);
            e.claim();
            true
        });
    }
    keys.register(scope);
}
