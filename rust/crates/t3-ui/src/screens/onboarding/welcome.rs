//! The welcome route and the root's first-run overlay (WelcomePage.tsx and FirstRunGate.tsx).
use std::rc::Rc;

use futures::StreamExt;
use nsh_core::prelude::*;
use t3_client::{
    composition::ClientRuntime, platform::Platform,
    thread_views::composer::preferences::SettingsHydration, types::EnvironmentId,
    view_logic::onboarding::FirstRunDecision,
};

use super::providers;
use crate::{
    i18n::{Value, t, tr},
    runtime::{
        rt,
        services::links::external_link,
        tasks::{TaskOwner, watch},
    },
    state::{
        nav::{Route, navigator},
        url::{UrlRequest, UrlState},
    },
    theme::tw,
    ui::{
        button::{ButtonContent, ButtonSize, ButtonVariant, button},
        toast::{ToastOptions, Variant, toast_manager},
    },
};

/// Binds the same two views to the host platform or to a paired test platform.
#[derive(Clone)]
pub struct Onboarding {
    page: Rc<dyn Fn() -> AnyElement>,
    gate: Rc<dyn Fn() -> AnyElement>,
}

impl Onboarding {
    pub fn new<P: Platform + Clone + 'static>(client: &ClientRuntime<P>) -> Self {
        let page = client.clone();
        let gate = client.clone();
        Self {
            page: Rc::new(move || welcome_page(page.clone())),
            gate: Rc::new(move || gate_view(gate.clone())),
        }
    }
    fn here() -> Option<Self> {
        use_context::<Self>().or_else(|| rt().backend().client().as_ref().map(Self::new))
    }
}

pub fn screen() -> AnyElement {
    Onboarding::here().map_or_else(crate::screens::home::screen, |service| (service.page)())
}

pub fn first_run_gate() -> AnyElement {
    Onboarding::here().map_or_else(|| view().into_element(), |service| (service.gate)())
}

fn gate_view<P: Platform + Clone + 'static>(client: ClientRuntime<P>) -> AnyElement {
    let service = client.first_run();
    let status = signal(service.snapshot());
    let nav = navigator();
    let mut changes = service.watch();
    watch(async move {
        let mut attempted = None;
        while let Some(next) = changes.next().await {
            let persist = next.persist_completion
                && !matches!(untracked(|| nav.top()), Route::Welcome | Route::Pair)
                && next.gate.decision != FirstRunDecision::Wizard
                && next.target != attempted;
            if persist {
                attempted = next.target.clone();
            }
            status.set(next);
            if persist {
                let _ = service.complete().await;
            }
        }
    });
    let urls = use_context::<UrlState>();
    effect(move || {
        let decision = status.with(|s| s.gate.decision);
        if urls.is_none() && decision == FirstRunDecision::Wizard && nav.top() == Route::Home {
            nav.open(Route::Welcome);
        }
    });
    let tasks = TaskOwner::here();
    dynamic(move || {
        let status = status.get();
        let route = nav.top();
        if status.target.is_none() || matches!(route, Route::Pair | Route::Welcome) {
            return None;
        }
        let failed = matches!(status.hydration, SettingsHydration::Failed | SettingsHydration::Retrying);
        if !failed && status.gate.decision != FirstRunDecision::Pending {
            return None;
        }
        let client = client.clone();
        Some(untracked(|| {
            let mut cover = view().test_id("first-run-gate").app_class(tw("absolute inset-0 flex min-h-0 items-center justify-center bg-background px-6 text-foreground"));
            if failed || status.gate.stalled {
                let title = if failed { "Could not read settings" } else { "Still connecting" };
                let reason = if failed { "Your saved settings could not be loaded." } else { "T3 Code could not confirm this workspace." };
                cover = cover.child(view().app_class(tw("flex max-w-sm flex-col items-center text-center"))
                    .child(text(t(title)).role(AccessibilityRole::Header).app_class(tw("text-lg font-semibold")))
                    .child(text(t(reason)).app_class(tw("mt-2 text-sm text-muted-foreground")))
                    .child(button(ButtonContent::Label(t(if failed {"Retry"} else {"Reload"})),ButtonVariant::Outline, ButtonSize::Sm, move || status.hydration == SettingsHydration::Retrying, move || {
                        let client = client.clone();
                        let target = status.target.clone();
                        tasks.spawn(async move {
                            if failed {client.composer_preferences().retry().await;}
                            else if let Some(target) = target {let _ = client.reconnect(&target).await;}
                        });
                    })));
            }
            cover.into_element()
        }))
    }).into_element()
}

fn welcome_page<P: Platform + Clone + 'static>(client: ClientRuntime<P>) -> AnyElement {
    let runtime = rt();
    let urls = use_context::<UrlState>();
    let target = computed({
        let client = client.clone();
        move || {
            let environments = runtime.environments();
            let resume = urls.and_then(|urls| {
                urls.location
                    .with(|loc| loc.hash.strip_prefix("#agents:").map(str::to_owned))
            });
            environments
                .iter()
                .find(|entry| Some(&entry.id.0) == resume.as_ref())
                .map(|entry| entry.id.clone())
                .or_else(|| client.first_run().target())
        }
    });
    dynamic(move || {
        let id = target.get();
        untracked(|| match id {
            Some(id) => page_for(client.clone(), id),
            None => crate::screens::home::screen(),
        })
    })
    .into_element()
}

fn page_for<P: Platform + Clone + 'static>(
    client: ClientRuntime<P>,
    id: EnvironmentId,
) -> AnyElement {
    let runtime = rt();
    let label = computed({
        let id = id.clone();
        move || {
            runtime
                .environments()
                .iter()
                .find(|entry| entry.id == id)
                .map_or_else(|| t("this computer"), |entry| entry.label.clone())
        }
    });
    let config = computed({
        let id = id.clone();
        move || {
            runtime.store.track_environments();
            runtime
                .store
                .store()
                .snapshot()
                .environments
                .get(&id)
                .and_then(|state| state.config.clone())
                .map(crate::runtime::Shared)
        }
    });
    let ready = computed(move || {
        config.with(|config| providers::groups(config.as_deref()).ready_count > 0)
    });
    let busy = signal(false);
    let error = signal(None);
    let tasks = TaskOwner::here();
    let nav = navigator();
    let urls = use_context::<UrlState>();
    let finish = {
        let first_run = client.first_run();
        move || {
            if busy.get_untracked() {
                return;
            }
            let manager = toast_manager();
            if let Some(id) = error.get_untracked() {
                manager.close(id);
            }
            busy.set(true);
            let first_run = first_run.clone();
            tasks.spawn(async move {
                let result = first_run.complete().await;
                busy.set(false);
                if result.is_ok() {
                    if let Some(urls) = urls {
                        urls.request(UrlRequest::Replace("/".into()));
                    } else {
                        nav.open(Route::Home);
                    }
                } else {
                    error.set(Some(manager.add(ToastOptions {
                        variant: Variant::Error,
                        title: "Could not finish setup".into(),
                        description: Some("Your settings could not be saved. Try again.".into()),
                        ..Default::default()
                    })));
                }
            });
        }
    };
    let without = finish.clone();
    let section = providers::section(client, id, config, label);
    let main = view().app_class(tw("mx-auto flex min-h-full w-full max-w-3xl flex-col px-6 pt-12 pb-8 sm:pt-20"))
        .child(text(t("Welcome to T3 Code")).role(AccessibilityRole::Header).app_class(tw("text-2xl font-semibold tracking-tight text-foreground")))
        .child(text(move || SharedString::from(tr("Setting up {label}.",&[("label",Value::Text(label.get()))]))).app_class(tw("mt-1.5 text-sm text-muted-foreground")))
        .child(view().app_class(tw("mt-10 border-t border-border/60")).child(section))
        .child(view().app_class(tw("mt-6 flex flex-row flex-wrap items-center justify-between gap-3"))
            .child(dynamic(move || (!ready.get()).then(|| untracked(|| button(ButtonContent::Label(t("Start without an agent")),ButtonVariant::GhostMuted,ButtonSize::Default,move || busy.get(),without.clone()).into_element()))))
            .child(view().app_class(tw("flex flex-row items-center gap-3")).child(dynamic(move || (!ready.get()).then(|| text(t("Set up one agent to start.")).app_class(tw("text-xs text-muted-foreground")))))
                .child(button(ButtonContent::Custom {label:t("Start"),child:text(move || SharedString::from(t(if busy.get() {"Starting…"} else {"Start"}))).into_element()},ButtonVariant::Default,ButtonSize::Default,move || !ready.get() || busy.get(),finish))))
        .child(view().app_class(tw("mt-auto flex flex-row flex-wrap pt-12 text-2xs leading-relaxed text-muted-foreground/70"))
            .child(text(move || SharedString::from(config.with(|cfg| cfg.as_ref().and_then(|c| c.managed_policy.as_ref()).filter(|p| p.limits.as_ref().and_then(|l|l.telemetry)==Some(false)).map_or_else(||t("T3 Code collects anonymous usage data to improve the app. To opt out, see the"), |p| tr("{organization} turned off anonymous usage data. See the",&[("organization",Value::Text(p.organization.0.clone()))]))))))
            .child(external_link(||Some("https://t3.codes/privacy-policy".into()),|_|{}).child(text(t("privacy policy")).app_class(tw("underline decoration-current/40 underline-offset-2 hover:text-muted-foreground")))).child(text(t("."))));
    view()
        .app_class(tw("flex min-h-0 min-w-0 flex-1 flex-col bg-background"))
        .child(scroll().app_class(tw("min-h-0 flex-1")).child(main))
        .into_element()
}
