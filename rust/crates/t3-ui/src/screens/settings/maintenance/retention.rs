use super::*;
use crate::i18n::{Value, tr};

pub(super) fn section(
    context: SettingsContext,
    supported: bool,
    machines: Vec<Machine>,
) -> AnyElement {
    let children = if supported {
        view()
            .child(rule(
                context,
                "browserArtifactsAfterDays",
                &["storageCleanup.browserArtifactsAfterDays"],
                "Delete old browser artifacts",
                "Delete saved browser captures after this many days. Older capture links will no longer open.",
            ))
            .child(rule(context, "logsAfterDays", &["storageCleanup.logsAfterDays"], "Delete old rotated logs", "Delete inactive rotated log files after this many days. Current logs are kept."))
    } else {
        let mut notice = view().child(text(t("Update the selected environments to use storage cleanup, or choose a machine that supports it.")).app_class(tw("px-3 text-sm text-muted-foreground sm:px-4")));

        for m in machines.into_iter().filter(|m| m.cleanup) {
            notice = notice.child(button(
                ButtonContent::Label(m.scope_label),
                ButtonVariant::Ghost,
                ButtonSize::Sm,
                || false,
                move || {
                    context.select_scope(SettingsScopeSearch {
                        machine: Some(m.id.0.clone()),
                        ..Default::default()
                    })
                },
            ));
        }
        notice
    };
    settings_section(
        "storage-artifacts",
        Text::from("Artifacts and logs"),
        children,
    )
    .into_element()
}
fn rule(
    context: SettingsContext,
    key: &'static str,
    paths: &'static [&'static str],
    label: &'static str,
    description: &'static str,
) -> AnyElement {
    let saved = computed(move || context.setting("storageCleanup")[key].as_i64());
    settings_row(
        SettingsRowProps::titled("", Text::from(label))
            .server(&[])
            .managed_paths(paths)
            .description(move || Text::from(description))
            .status(move || {
                context
                    .view()
                    .is_some_and(|v| {
                        v.targets.iter().any(|target| {
                            target.json["storageCleanup"][key].as_i64() != saved.get()
                        })
                    })
                    .then(|| Text::from("Mixed across selected machines"))
            })
            .control(move || control(context, saved, key, label)),
    )
    .into_element()
}

fn control(
    context: SettingsContext,
    saved: Memo<Option<i64>>,
    key: &'static str,
    label: &'static str,
) -> AnyElement {
    let draft = signal(String::new());
    let enabled = signal(false);
    let dirty = signal(false);
    let pending = signal(None::<i64>);
    let sequence = signal(0u64);
    effect(move || {
        let value = saved.get();
        enabled.set(value.is_some());
        draft.set(value.map(|n| n.to_string()).unwrap_or_default());
        dirty.set(false);
        pending.set(None);
    });
    let cancel = move || {
        let value = pending.get_untracked().or(saved.get_untracked());
        draft.set(value.map(|n| n.to_string()).unwrap_or_default());
        enabled.set(value.is_some());
        dirty.set(false);
    };
    let write = move |value: Option<i64>| {
        sequence.update(|v| *v += 1);
        let revision = sequence.get_untracked();
        pending.set(value);
        context.update_with(json!({"storageCleanup":{key:value}}), move || {
            if sequence.try_get() == Some(revision) {
                pending.set(None);
                cancel();
            }
        });
    };
    let commit = move |explicit: bool| {
        if !enabled.get_untracked() {
            return;
        }
        if !explicit && !dirty.get_untracked() {
            cancel();
            return;
        }
        let Some(days) = draft.with_untracked(|s| {
            s.trim()
                .parse::<f64>()
                .ok()
                .filter(|n| n.is_finite())
                .map(|n| n.round().clamp(1., 3650.) as i64)
        }) else {
            cancel();
            return;
        };
        draft.set(days.to_string());
        dirty.set(false);
        if pending.get_untracked().or(saved.get_untracked()) != Some(days) {
            write(Some(days));
        }
    };
    let row = element_ref();
    row.on_focus_out(move || commit(false));
    view()
        .element_ref(&row)
        .app_class(tw("flex flex-row items-center gap-3"))
        .child(region(
            move || enabled.get(),
            move |on| {
                if !on {
                    return text(t("Off")).app_class(tw("text-xs text-muted-foreground")).into_element();
                }
                let step = move |increase: bool| {
                    let label = tr(if increase { "Increase {label}" } else { "Decrease {label}" }, &[("label", Value::Text(t(label)))]);

                    nsh_base::button()
                        .accessibility_label(Some(SharedString::from(label)))
                        .on_press(move || {
                            let old = draft.with_untracked(|s| s.parse::<i64>().unwrap_or(8));
                            draft.set((old.saturating_add(if increase { 1 } else { -1 }).clamp(1, 3650)).to_string());
                            dirty.set(true);
                            if saved.get_untracked().is_some() {
                                commit(true);
                            }
                        })
                        .child(text(if increase { "+" } else { "−" }))
                };
                view()
                    .app_class(tw("flex flex-row items-center gap-1"))
                    .child(step(false))
                    .child(
                        nsh_base::input()
                            .label(tr("{label} in days", &[("label", Value::Text(t(label)))]))
                            .auto_focus(saved.get_untracked().is_none())
                            .keyboard_type(KeyboardType::Number)
                            .value(move || draft.get())
                            .on_change(move |s: &str| {
                                draft.set(s.to_string());
                                dirty.set(true);
                            })
                            .submit_behavior(SubmitBehavior::Submit)
                            .on_submit(move |_: &str| commit(true))
                            .on_key_down(move |e: &KeyEvent| {
                                if e.key.as_ref() == "Escape" {
                                    cancel();
                                }
                            })
                            .app_class(tw("h-8 w-16 rounded-lg border border-input bg-background text-right text-sm")),
                    )
                    .child(text(t("days")).app_class(tw("pr-2 text-xs")))
                    .child(step(true))
                    .into_element()
            },
        ))
        .child(switch(label, ControlState { checked: Prop::bind(move || enabled.get()), ..Default::default() }, Size::Default, move |on| {
            dirty.set(false);
            enabled.set(on);
            if on {
                draft.set("8".into());
            } else if saved.get_untracked().is_some() || pending.get_untracked().is_some() {
                write(None);
            }
        }))
        .into_element()
}
