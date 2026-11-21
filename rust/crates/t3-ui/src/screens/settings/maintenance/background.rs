use super::super::{setting_controls::choices, shell::settings_select};
use super::*;
use crate::ui::{
    dialog::{
        FooterVariant, dialog, dialog_close, dialog_description, dialog_footer, dialog_header,
        dialog_panel, dialog_popup, dialog_title,
    },
    select::{SelectItem, SelectProps, Size as SelectSize, Value as Selection, select},
};
use t3_client::services::settings::{maintenance::BackgroundPolicy, service::SettingsWriteAccess};

const PROFILES: &[(&str, &str)] = &[
    ("balanced", "Balanced"),
    ("performance", "Performance"),
    ("battery-saver", "Battery saver"),
];
fn policy(c: SettingsContext) -> BackgroundPolicy {
    BackgroundPolicy::read(&c.server_settings())
}
pub(super) fn row(c: SettingsContext) -> AnyElement {
    let open = signal(false);
    let single = move || single_id(c).is_some();
    let writable = move || {
        matches!(c.write_access(), SettingsWriteAccess::Writable)
            && single()
            && c.view().is_some_and(|v| !v.scope.is_project_scope())
    };
    effect(move || {
        if !writable() {
            open.set(false);
        }
    });
    settings_row(
        SettingsRowProps::searchable("background-activity")
            .first()
            .server(&["backgroundActivity"])
            .description(move || {
                let p = policy(c);
                if p.advanced {
                    let name = PROFILES.iter().find(|v| v.0 == p.profile).map_or("Balanced", |v| v.1);
                    Text::content(crate::i18n::tr("Uses custom intervals. Shared policy: {profile}.", &[("profile", crate::i18n::Value::Text(t(name)))]))
                } else {
                    Text::from(match p.profile {
                        "performance" => "Allows scoped background probes while any subscribed client remains connected.",
                        "battery-saver" => "Also pauses background probes when the host or client is on battery.",
                        _ => "Pauses probes for idle clients, locked hosts, or low power mode.",
                    })
                }
            })
            .reset(reset(c, "backgroundActivity", "background activity"))
            .control(move || {
                let picker = region(single, move |single| {
                    let mut items: Vec<_> = PROFILES.iter().map(|(value, name)| SelectItem::new(*value, t(name))).collect();
                    items.push(SelectItem::new("advanced", t(if single { "Advanced" } else { "Advanced (one environment)" })).disabled(!single));

                    let mut props = SelectProps::new("Background activity profile");
                    props.items = items.into();
                    props.size = SelectSize::Sm;
                    props.placeholder = Text::from("Mixed").into();
                    props.value = Prop::bind(move || {
                        let p = policy(c);
                        Selection::Controlled((!c.mixed(&["backgroundActivity"])).then(|| SharedString::from(if p.advanced { "advanced" } else { p.profile })))
                    });

                    select(props, move |next| match next.as_deref() {
                        Some("advanced") if writable() => open.set(true),
                        Some(v) if PROFILES.iter().any(|(id, _)| *id == v) => c.update(json!({
                        "backgroundActivity":{
                        "schemaVersion":1,
                        "profile":v,
                        "overrides":{
                        }}})),
                        _ => {}
                    })
                });
                view()
                    .app_class(tw("flex flex-row items-center gap-2"))
                    .child(picker)
                    .child(region(move || policy(c).advanced && writable(), move |visible| visible.then(|| action("Configure advanced background activity", ButtonSize::Sm, || false, move || open.set(true)))))
                    .child(dialog(open, move |state| {
                        dialog_popup(state, Default::default(), move || {
                            let header = dialog_header(state, view().child(dialog_title(state, || Text::from("Background Activity"))).child(dialog_description(state, || Text::from("Tune the shared power policy and the background intervals that feed it."))));

                            let panel = dialog_panel(state, advanced(c));
                            let footer = dialog_footer(
                                state,
                                FooterVariant::Default,
                                view()
                                    .app_class(tw("flex flex-row gap-2"))
                                    .child(action(
                                        "Reset all",
                                        ButtonSize::Default,
                                        move || !writable(),
                                        move || {
                                            c.update(json!({
"backgroundActivity":t3_contracts::settings::ServerSettings::default().background_activity}))
                                        },
                                    ))
                                    .child(dialog_close(state, ButtonContent::Label(t("Done")), ButtonVariant::Default, ButtonSize::Default, || false)),
                            );

                            view().child(header).child(panel).child(footer)
                        })
                    }))
            }),
    )
    .into_element()
}
fn advanced(c: SettingsContext) -> AnyElement {
    let mut panel = view()
        .app_class(tw("flex min-w-0 flex-col rounded-xl border bg-card text-card-foreground"))
        .child(settings_row(SettingsRowProps::titled("", Text::from("Shared policy")).description(|| Text::from("Controls whether background work may run after a subscribed interval fires.")).control(move || {
            settings_select(
                "Shared background policy",
                "w-full sm:w-40",
                choices(PROFILES),
                move || policy(c).profile.into(),
                || false,
                move |v| {
                    c.update(json!({
"backgroundActivity":{
"schemaVersion":1,
"profile":"custom",
"baseProfile":v,
"overrides":policy(c).overrides}}))
                },
            )
        })));

    for (key, title, label, desc, min, step) in [
        (
            "automaticGitFetchInterval",
            "Git fetch interval",
            "Git fetch interval in seconds",
            "Refresh remote branch status in the background.",
            0,
            5,
        ),
        (
            "providerHealthRefreshInterval",
            "Provider health interval",
            "Provider health interval in seconds",
            "Refresh provider availability, versions, and auth state. Model catalog updates follow Provider update checks.",
            0,
            30,
        ),
        (
            "hostPowerMonitorActiveInterval",
            "Host power monitor",
            "Active host power interval in seconds",
            "Poll host power state while clients are active.",
            5,
            5,
        ),
        (
            "hostPowerMonitorIdleInterval",
            "Idle host monitor",
            "Idle host power interval in seconds",
            "Poll host power state when no foreground client is active.",
            5,
            30,
        ),
    ] {
        panel = panel.child(settings_row(
            SettingsRowProps::titled("", Text::from(title))
                .description(move || Text::from(desc))
                .control(move || interval(c, key, label, min, step)),
        ));
    }
    for (key, label) in [
        ("pauseWhenHostLocked", "Pause when host is locked"),
        ("pauseWhenHostLowPower", "Pause on host low power"),
        ("pauseWhenClientLowPower", "Pause on client low power"),
        ("pauseWhenOnBattery", "Pause on battery"),
    ] {
        panel = panel.child(settings_row(
            SettingsRowProps::titled("", Text::from(label)).control(move || {
                switch(
                    label,
                    ControlState {
                        checked: Prop::bind(move || policy(c).values[key] == true),
                        ..Default::default()
                    },
                    Size::Default,
                    move |on| write_override(c, key, json!(on), || {}),
                )
            }),
        ));
    }
    panel.into_element()
}
fn write_override(
    c: SettingsContext,
    key: &str,
    value: serde_json::Value,
    failed: impl Fn() + 'static,
) {
    let mut p = policy(c);
    p.values[key] = value;
    c.update_with(
        json!({
"backgroundActivity":{
"schemaVersion":1,
"profile":"custom",
"baseProfile":p.profile,
"overrides":p.values}}),
        failed,
    );
}
fn interval(
    c: SettingsContext,
    key: &'static str,
    label: &'static str,
    min: i64,
    step: i64,
) -> AnyElement {
    let saved = computed(move || {
        (policy(c).values[key].as_f64().unwrap_or_default() / 1000.).round() as i64
    });
    let draft = signal(String::new());
    let submitted = signal(None::<i64>);
    effect(move || {
        draft.set(saved.get().to_string());
        submitted.set(None);
    });
    let commit = move || {
        let Some(n) = draft.with_untracked(|v| v.parse::<f64>().ok().filter(|n| n.is_finite()))
        else {
            draft.set(saved.get_untracked().to_string());
            return;
        };
        let n = (n.round() as i64).max(min);
        draft.set(n.to_string());
        if submitted.get_untracked().unwrap_or(saved.get_untracked()) == n {
            return;
        }
        submitted.set(Some(n));
        write_override(c, key, json!(n.saturating_mul(1000)), move || {
            if submitted.try_get() == Some(Some(n)) {
                submitted.set(None);
                draft.set(saved.get_untracked().to_string());
            }
        });
    };
    let input = element_ref();
    input.on_focus_out(commit);
    let adjust = move |up: bool| {
        let n = draft.with_untracked(|v| v.parse::<i64>().unwrap_or(saved.get_untracked()));
        draft.set(
            n.saturating_add(if up { step } else { -step })
                .max(min)
                .to_string(),
        );
        commit();
    };
    view()
        .app_class(tw("flex flex-row items-center gap-2"))
        .child(button(
            ButtonContent::Label(t("−")),
            ButtonVariant::Ghost,
            ButtonSize::Xs,
            || false,
            move || adjust(false),
        ))
        .child(
            nsh_base::input()
                .element_ref(&input)
                .label(t(label))
                .keyboard_type(KeyboardType::Number)
                .value(move || draft.get())
                .on_change(move |v: &str| draft.set(v.to_string()))
                .on_submit(move |_: &str| commit())
                .app_class(tw("w-16 text-right")),
        )
        .child(button(
            ButtonContent::Label(t("+")),
            ButtonVariant::Ghost,
            ButtonSize::Xs,
            || false,
            move || adjust(true),
        ))
        .child(text(t("seconds")).app_class(tw("text-xs text-muted-foreground")))
        .into_element()
}
