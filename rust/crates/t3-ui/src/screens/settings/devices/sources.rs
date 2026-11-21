use super::*;
use crate::ui::menu::{ItemOptions, menu_button, menu_item, menu_popup_with};
use t3_client::services::settings::devices::{SourceEdit, host_rows};
use t3_contracts::device::{DeviceHostStatus, DevicePlatform};

#[derive(Clone, PartialEq)]
pub(super) struct Editing {
    pub kind: HostKind,
    pub draft: Value,
    pub original: Option<HostRow>,
}
pub(super) type Editor = Signal<Option<Editing>>;
pub(super) fn section(p: Page) -> AnyElement {
    let editing = signal(None::<Editing>);
    let flags = computed(move || {
        let state = p.primary.and_then(|m| m.state.get());
        (
            state.as_ref().and_then(|s| s.supports_eas_hosts.flatten()) == Some(true),
            state
                .as_ref()
                .and_then(|s| s.supports_simlock_hosts.flatten())
                == Some(true)
                && p.settings()
                    .iter()
                    .any(|(_, s)| s["simlockDeviceHostsEnabled"] == true),
        )
    });
    let add = menu_button(
        t("Add source"),
        move || !p.can_edit() || editing.with(Option::is_some),
        text(t("Add source")),
        Default::default(),
        menu_popup_with(move || {
            let (eas, simlock) = flags.get();
            let mut list = view();
            for (kind, label, show) in [
                (HostKind::Ssh, "Machine over SSH", true),
                (HostKind::Eas, "EAS Simulator", eas),
                (HostKind::Simlock, "Simlock pool (alpha)", simlock),
            ] {
                if show {
                    list = list.child(menu_item(
                        t(label),
                        Default::default(),
                        text(t(label)),
                        move || {
                            if !p.can_edit() {
                                return;
                            }
                            if let Some(id) =
                                p.sessions.get().and_then(|s| s.new_device_host_id().ok())
                            {
                                editing.set(Some(Editing {
                                    kind,
                                    draft: kind.new_host(id),
                                    original: None,
                                }));
                            }
                        },
                    ));
                }
            }
            list
        }),
    );
    let rows = region(
        move || {
            let (eas, simlock) = flags.get();
            host_rows(&p.settings(), eas, simlock)
        },
        move |rows| {
            let mut list = view();
            for row in rows {
                list = list.child(source(p, row, editing));
            }
            list
        },
    );
    let local = settings_row(
        SettingsRowProps::titled("", Text::from("Local simulators"))
            .first()
            .description(move || {
                Text::from(if p.members.with(|m| m.len()) > 1 {
                    "Simulators and emulators installed on each environment's own machine."
                } else {
                    "Simulators and emulators installed on this environment's machine."
                })
            })
            .children(move || availability(p, None)),
    );
    settings_section(
        "device-sources",
        Text::from("Device sources"),
        view().child(add).child(local).child(rows).child(region(
            move || editing.get(),
            move |edit| edit.map(|edit| editor::form(p, editing, edit)),
        )),
    )
    .into_element()
}
fn source(p: Page, row: HostRow, editing: Editor) -> AnyElement {
    let name = row.host["label"].as_str().unwrap_or_default().to_owned();
    let host = &row.host;
    let count = |key: &str| host[key].as_u64().unwrap_or_default();
    let description = match row.kind {
        HostKind::Ssh => host["target"].as_str().unwrap_or_default().into(),
        HostKind::Eas => format!(
            "Up to {} {} at once, each stopped after {} min or {} idle min",
            count("maxConcurrentSessions"),
            if count("maxConcurrentSessions") == 1 {
                "session"
            } else {
                "sessions"
            },
            count("maxSessionMinutes"),
            count("idleMinutes")
        ),
        HostKind::Simlock => format!(
            "{} · Up to {} {} at once, each released after {} min",
            host["gatewayUrl"]
                .as_str()
                .unwrap_or_default()
                .trim_start_matches("https://")
                .trim_start_matches("http://"),
            count("maxLeases"),
            if count("maxLeases") == 1 {
                "device"
            } else {
                "devices"
            },
            count("maxLeaseMinutes")
        ),
    };
    let description = if row.separate {
        p.members
            .with(|members| {
                members.iter().find_map(|m| {
                    let machine = m.member.get();
                    row.holders
                        .contains_key(&machine.id)
                        .then(|| format!("{} · {description}", machine.label))
                })
            })
            .unwrap_or(description)
    } else {
        description
    };
    let row = stored(row);
    settings_row(
        SettingsRowProps::titled("", Text::content(name))
            .description(move || Text::content(description.clone()))
            .children(move || availability(p, Some(row)))
            .control(move || {
                view()
                    .app_class(tw("flex flex-row items-center gap-1"))
                    .child(btn(
                        "Edit",
                        move || !p.can_edit(),
                        move || {
                            if !p.can_edit() {
                                return;
                            }
                            let original = row.get();
                            editing.set(Some(Editing {
                                kind: original.kind,
                                draft: original.host.clone(),
                                original: Some(original),
                            }));
                        },
                    ))
                    .child(menu_button(
                        t("More"),
                        move || !p.can_edit(),
                        text(t("⋯")),
                        Default::default(),
                        menu_popup_with(move || {
                            let mut entries = view();
                            if row.get().kind == HostKind::Ssh {
                                entries = entries.child(menu_item(
                                    t("Test connection"),
                                    Default::default(),
                                    text(t("Test connection")),
                                    move || {
                                        let host = row.get().host;
                                        let peers = p.members.get();
                                        p.tasks.spawn(async move {
                                            let mut failures = vec![];
                                            for m in peers {
                                                if m.session
                                                    .get()
                                                    .check(HostKind::Ssh, &host, None, None)
                                                    .await
                                                    .is_err()
                                                {
                                                    failures.push(m.member.get().label);
                                                }
                                            }
                                            if !failures.is_empty() {
                                                error("Test connection", failures.join(", "));
                                            } else {
                                                toast_manager().add(ToastOptions {
                                                    variant: Variant::Success,
                                                    title: Text::from("Connection checks passed"),
                                                    ..Default::default()
                                                });
                                            }
                                        });
                                    },
                                ));
                            }
                            entries.child(menu_item(
                                t("Remove"),
                                ItemOptions {
                                    variant: crate::ui::menu::Variant::Destructive,
                                    ..Default::default()
                                },
                                text(t("Remove")),
                                move || {
                                    let original = row.get();
                                    save(
                                        p,
                                        editing,
                                        SourceEdit {
                                            kind: original.kind,
                                            draft: original.host.clone(),
                                            original: Some(original),
                                            remove: true,
                                            tokens: Default::default(),
                                        },
                                    );
                                },
                            ))
                        }),
                    ))
            }),
    )
    .into_element()
}
pub(super) fn save(p: Page, editing: Editor, edit: SourceEdit) {
    if !p.can_edit() {
        return;
    }
    let Some(writer) = p.writer.get() else {
        return;
    };
    let ids = p
        .members
        .with(|m| m.iter().map(|m| m.member.get().id).collect());
    let title = match (edit.kind, edit.remove) {
        (HostKind::Eas, true) => "EAS host not removed everywhere",
        (HostKind::Eas, false) => "EAS host not saved everywhere",
        (HostKind::Simlock, true) => "Simlock host not removed everywhere",
        (HostKind::Simlock, false) => "Simlock host not saved everywhere",
        (HostKind::Ssh, true) => "Device host not removed everywhere",
        (HostKind::Ssh, false) => "Device host not saved everywhere",
    };
    p.pending.set(true);
    p.tasks.spawn(async move {
        match writer.save(ids, edit).await {
            Ok(()) => editing.set(None),
            Err(labels) => error(title, labels.join(", ")),
        }
        p.pending.set(false);
    });
}
fn availability(p: Page, row: Option<Stored<HostRow>>) -> AnyElement {
    region(
        move || {
            let mut lines = vec![];
            for m in p.members.get() {
                let member = m.member.get();
                if !p.context.view().is_some_and(|v| {
                    v.selected.connected.iter().any(|e| {
                        e.environment_id == member.id
                            && e.config
                                .as_ref()
                                .is_some_and(|c| c.settings.enable_device_support)
                    })
                }) {
                    continue;
                }
                let Some(state) = m.state.get() else {
                    continue;
                };
                let id = row.map_or_else(
                    || Some("local".to_owned()),
                    |r| r.get().holders.get(&member.id).cloned(),
                );
                let Some(id) = id else {
                    lines.push((member.id, member.label, t("Not added"), None));
                    continue;
                };
                let host = state
                    .hosts
                    .iter()
                    .chain(state.eas_hosts.iter().flatten().flatten())
                    .chain(state.simlock_hosts.iter().flatten().flatten())
                    .find(|h| h.id.0 == id);
                let status = state
                    .host_statuses
                    .iter()
                    .find(|(key, _)| key.0 == id)
                    .map(|(_, s)| s);
                let mut info = vec![];
                if let Some(host) = host {
                    for platform in &host.platforms {
                        let os = match platform.platform {
                            DevicePlatform::Ios => "iOS",
                            DevicePlatform::Android => "Android",
                        };
                        info.push(format!(
                            "{os}: {}",
                            if platform.available {
                                t("Available")
                            } else {
                                platform
                                    .reason
                                    .as_ref()
                                    .and_then(|v| v.as_deref())
                                    .map(t)
                                    .unwrap_or_else(|| t("Not available"))
                            }
                        ));
                    }
                }
                let stage = status
                    .map(|s| &s.status)
                    .or_else(|| (id == "local").then_some(&state.host_status));
                if let Some(detail) =
                    status.and_then(|s| s.detail.clone().flatten()).or_else(|| {
                        (id == "local")
                            .then(|| state.host_status_detail.clone().flatten())
                            .flatten()
                    })
                {
                    info.push(detail);
                } else if let Some(stage) = stage {
                    match stage {
                        DeviceHostStatus::Failed => info.push(t("Can't connect")),
                        DeviceHostStatus::Installing => info.push(t("Installing")),
                        DeviceHostStatus::Starting => info.push(t("Starting")),
                        _ => {}
                    }
                }
                let retry = (stage == Some(&DeviceHostStatus::Failed)
                    && (id == "local" || state.supports_host_retry.flatten() == Some(true)))
                .then_some(id);
                if !info.is_empty() || retry.is_some() {
                    lines.push((member.id, member.label, info.join(" · "), retry));
                }
            }
            lines
        },
        move |lines| {
            let multi = p.members.with(|members| members.len() > 1);
            let mut block = view();
            for (env, label, line, retry) in lines {
                block = block.child(
                    text(if multi {
                        format!("{label}: {line}")
                    } else {
                        line
                    })
                    .app_class(tw("text-xs text-muted-foreground")),
                );
                if let Some(id) = retry {
                    block = block.child(btn(
                        if id == "local" { "Refresh" } else { "Retry" },
                        move || p.pending.get(),
                        move || {
                            let Some(m) = p.members.with(|members| {
                                members.iter().find(|m| m.member.get().id == env).copied()
                            }) else {
                                return;
                            };
                            let session = m.session.get();
                            let input = DeviceListInput {
                                update_tool: None,
                                inspect_only: None,
                                retry_host_id: (id != "local").then(|| {
                                    Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                                        id.clone(),
                                    ))
                                }),
                            };
                            p.tasks.spawn(async move {
                                if let Ok(state) = session.list(input).await {
                                    m.state.set(Some(Rc::new(state)));
                                }
                            });
                        },
                    ));
                }
            }
            block
        },
    )
}
