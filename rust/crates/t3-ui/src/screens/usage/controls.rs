use super::*;
use crate::ui::{
    menu::*,
    select::{self, SelectItem, SelectProps},
    toggle::*,
};

pub(super) fn choice(
    label: &'static str,
    items: Vec<(&'static str, String)>,
    current: &'static str,
    set: impl Fn(&str) + 'static,
) -> AnyElement {
    if crate::state::layout::layout_class().is_compact() {
        let mut props = SelectProps::new(label);
        props.variant = select::Variant::Ghost;
        props.size = select::Size::Compact;
        props.items = items
            .into_iter()
            .map(|(v, l)| SelectItem::new(v, l))
            .collect::<Vec<_>>()
            .into();
        props.value = select::Value::Controlled(Some(current.into())).into();
        select::select(props, move |v| {
            if let Some(v) = v {
                set(&v)
            }
        })
        .into_element()
    } else {
        toggle_group(
            t(label),
            items
                .into_iter()
                .map(|(v, l)| GroupItem::new(v, Content::Label(l)).into())
                .collect(),
            Default::default(),
            move || vec![current.into()],
            || false,
            move |v| {
                if let Some(v) = v.first() {
                    set(v)
                }
            },
        )
        .into_element()
    }
}
pub(super) fn filters(
    targets: &[(EnvironmentId, String)],
    state: Page,
    prices: Signal<bool>,
) -> AnyElement {
    let all = targets
        .iter()
        .map(|(id, _)| id.0.clone())
        .collect::<BTreeSet<_>>();
    let selected = state.selected.get_untracked();
    let env_label = match selected.as_ref() {
        None => t("All environments"),
        Some(ids) if ids.len() == 1 => targets
            .iter()
            .find(|(id, _)| ids.contains(&id.0))
            .map(|(_, label)| label.clone())
            .unwrap_or_else(|| t("No environments connected.")),
        Some(ids) => tr(
            "{count} environments",
            &[("count", Value::Int(ids.len() as i64))],
        ),
    };
    let mut env = view()
        .child(check(
            t("All environments"),
            move || state.selected.get().is_none(),
            move |yes| {
                state
                    .selected
                    .set(if yes { None } else { Some(BTreeSet::new()) })
            },
        ))
        .child(menu_separator());
    for (id, label) in targets {
        let id = id.0.clone();
        let toggle = id.clone();
        let all = all.clone();
        env = env.child(check(
            label.clone(),
            move || {
                state
                    .selected
                    .with(|s| s.as_ref().is_none_or(|s| s.contains(&id)))
            },
            move |yes| {
                let mut selected = state
                    .selected
                    .get_untracked()
                    .unwrap_or_else(|| all.clone());
                if yes {
                    selected.insert(toggle.clone());
                } else {
                    selected.remove(&toggle);
                }
                state.selected.set((selected != all).then_some(selected));
            },
        ));
    }
    env = env.child(menu_separator()).child(menu_item(
        t("Model prices"),
        Default::default(),
        text(t("Model prices")),
        move || prices.set(true),
    ));
    let hidden = state.hidden.get_untracked();
    let provider_label = match PROVIDERS.len() - hidden.len() {
        0 => t("No providers"),
        1 => PROVIDERS
            .iter()
            .find(|p| !hidden.contains(**p))
            .copied()
            .unwrap_or_default()
            .into(),
        8 => t("All providers"),
        n => tr("{count} providers", &[("count", Value::Int(n as i64))]),
    };
    let mut providers = view()
        .child(check(
            t("All providers"),
            move || state.hidden.with(BTreeSet::is_empty),
            move |yes| {
                state.hidden.set(if yes {
                    BTreeSet::new()
                } else {
                    PROVIDERS.iter().map(|s| s.to_string()).collect()
                })
            },
        ))
        .child(menu_separator());
    for p in PROVIDERS {
        providers = providers.child(check(
            p.into(),
            move || !state.hidden.with(|s| s.contains(p)),
            move |yes| {
                state.hidden.update(|s| {
                    if yes {
                        s.remove(p);
                    } else {
                        s.insert(p.to_string());
                    }
                })
            },
        ))
    }
    let trigger = |label: String| {
        let style = if layout_class().is_compact() {
            "max-w-12 truncate text-xs"
        } else {
            "truncate text-sm"
        };
        text(label).app_class(tw(style))
    };
    view()
        .app_class(tw("flex min-w-0 flex-row items-center gap-1"))
        .child(menu_button(
            env_label.clone(),
            || false,
            trigger(env_label),
            Default::default(),
            menu_popup(env),
        ))
        .child(separator())
        .child(menu_button(
            provider_label.clone(),
            || false,
            trigger(provider_label),
            Default::default(),
            menu_popup(providers),
        ))
        .into_element()
}
fn check(
    label: String,
    checked: impl Fn() -> bool + 'static,
    change: impl Fn(bool) + 'static,
) -> AnyElement {
    menu_checkbox_item(
        label.clone(),
        checked,
        CheckboxVariant::Default,
        ItemOptions {
            close_on_select: false,
            ..Default::default()
        },
        text(label),
        change,
    )
    .into_element()
}

#[derive(Clone)]
pub(super) struct LimitsButton {
    pub(super) disabled: Memo<bool>,
    pub(super) run: Rc<dyn Fn(bool)>,
}
pub(super) fn limits_refresh<P: Platform + Clone + 'static>(
    client: &ClientRuntime<P>,
    selected: Memo<Vec<UsageTarget>>,
    visible: Memo<bool>,
) -> LimitsButton {
    let targets = computed(move || {
        selected.with(|targets| {
            targets
                .iter()
                .filter(|target| target.connected && target.config.is_some())
                .map(|target| target.id.clone())
                .collect::<Vec<_>>()
        })
    });
    let pending = signal(0usize);
    let owner = TaskOwner::here();
    let client = client.clone();
    let coordinator = t3_client::services::usage::UsageLimitsRefresh::default();
    let run: Rc<dyn Fn(bool)> = Rc::new(move |automatic| {
        if !automatic && pending.get_untracked() != 0 {
            return;
        }
        let reads = targets
            .get_untracked()
            .into_iter()
            .map(|id| client.refresh_usage_limits(&coordinator, id, automatic))
            .collect::<Vec<_>>();
        if reads.is_empty() {
            return;
        }
        pending.update(|n| *n += 1);
        owner.spawn(async move {
            for result in futures::future::join_all(reads).await {
                if let Err(error) = result {
                    log::warn!("Could not refresh usage limits: {error}");
                }
            }
            pending.update(|n| *n -= 1);
        });
    });
    let auto = run.clone();
    effect(move || {
        if visible.get() && !targets.get().is_empty() {
            auto(true);
        }
    });
    LimitsButton {
        disabled: computed(move || targets.with(Vec::is_empty) || pending.get() != 0),
        run,
    }
}

pub(super) fn separator() -> impl IntoElement {
    crate::ui::icon::icon(Icon::Slash, 10.0, || {
        crate::theme::palette().muted_foreground
    })
}
