//! DevicesSettingsPanel and DeviceSourcesSettings.
mod editor;
mod sources;
use super::shell::{
    SettingsContext, SettingsRowProps, region, settings_page_container, settings_row,
    settings_section, use_settings,
};
use crate::{
    i18n::{Text, t},
    runtime::tasks::{TaskOwner, watch},
    theme::tw,
    ui::{
        button::{ButtonContent, ButtonSize, ButtonVariant, button},
        switch::{ControlState, Size, switch},
        toast::{ToastOptions, Variant, toast_manager},
    },
};
use futures::StreamExt;
use nsh_core::prelude::*;
use serde_json::{Value, json};
use std::rc::Rc;
use t3_client::{
    services::settings::{
        devices::{DeviceSession, HostKind, HostRow, SourceWriter, busy},
        scope::settings_scope_environment_label,
        sessions::SettingsSessions,
    },
    types::EnvironmentId,
};
use t3_contracts::device::{DeviceHostKind, DeviceListInput, DeviceServiceState, DeviceUpdateTool};
#[derive(Clone, PartialEq)]
struct Member {
    id: EnvironmentId,
    label: String,
    serial: u64,
    generation: u64,
    writable: bool,
}
#[derive(Clone, Copy)]
struct Live {
    member: Stored<Member>,
    session: Stored<DeviceSession>,
    state: Signal<Option<Rc<DeviceServiceState>>>,
}
#[derive(Clone, Copy)]
struct Page {
    context: SettingsContext,
    sessions: Stored<Option<SettingsSessions>>,
    writer: Stored<Option<SourceWriter>>,
    members: Stored<Vec<Live>>,
    primary: Option<Live>,
    pending: Signal<bool>,
    tasks: TaskOwner,
}
impl Page {
    fn project(self) -> bool {
        self.context
            .view()
            .is_some_and(|v| v.scope.is_project_scope())
    }
    fn settings(self) -> Vec<(EnvironmentId, Value)> {
        self.context
            .view()
            .map(|v| {
                v.selected
                    .connected
                    .iter()
                    .filter_map(|e| {
                        let s = &e.config.as_ref()?.settings;
                        Some((
                            e.environment_id.clone(),
                            json!({
                                "enableDeviceSupport":s.enable_device_support,
                                "simlockDeviceHostsEnabled":s.simlock_device_hosts_enabled,
                                "deviceHosts":s.device_hosts,
                                "easDeviceHosts":s.eas_device_hosts,
                                "simlockDeviceHosts":s.simlock_device_hosts,
                            }),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
    fn can_edit(self) -> bool {
        !self.project()
            && !self.pending.get()
            && !self.members.with(|m| m.is_empty())
            && self
                .members
                .with(|m| m.iter().any(|m| m.session.get().writable()))
    }
}
pub fn screen() -> AnyElement {
    let c = use_settings();
    let sessions = stored(use_context::<SettingsSessions>());
    let writer = stored(use_context::<SourceWriter>());
    settings_page_container(region(
        move || {
            c.revision();
            let scope = c.view();
            let primary = scope.as_ref().and_then(|s| {
                s.selected
                    .representative
                    .as_ref()
                    .map(|e| e.environment_id.clone())
            });
            let members = scope
                .iter()
                .flat_map(|s| {
                    s.selected.connected.iter().filter_map(|e| {
                        let (serial, generation, ep) =
                            sessions.get()?.session(&e.environment_id)?;
                        Some(Member {
                            id: e.environment_id.clone(),
                            label: settings_scope_environment_label(e, &s.selected.environments),
                            serial,
                            generation,
                            writable: DeviceSession(ep).writable(),
                        })
                    })
                })
                .collect::<Vec<_>>();
            (scope.map(|s| s.search.clone()), primary, members)
        },
        move |(_, primary, members)| {
            let members = members
                .into_iter()
                .filter_map(|m| {
                    let (_, _, ep) = sessions.get()?.session(&m.id)?;
                    let session = DeviceSession(ep);
                    let state = signal(None);
                    let mut updates = session.state();
                    watch(async move {
                        while let Some(next) = updates.next().await {
                            state.set(next.ok().map(Rc::new));
                        }
                    });
                    Some(Live {
                        member: stored(m),
                        session: stored(session),
                        state,
                    })
                })
                .collect::<Vec<_>>();
            let primary = members
                .iter()
                .find(|m| m.member.with(|m| Some(&m.id) == primary.as_ref()))
                .copied();
            let p = Page {
                context: c,
                sessions,
                writer,
                primary,
                members: stored(members),
                pending: signal(false),
                tasks: TaskOwner::here(),
            };
            view()
                .app_class(tw("flex flex-col gap-8"))
                .child(settings_section(
                    "devices",
                    Text::from("Devices"),
                    view().child(control(p, true)).child(control(p, false)),
                ))
                .child(sources::section(p))
        },
    ))
    .into_element()
}
fn control(p: Page, hub: bool) -> AnyElement {
    let (id, key, label, keys): (_, _, _, &'static [&'static str]) = if hub {
        (
            "device-hub",
            "enableDeviceSupport",
            "Device hub",
            &["enableDeviceSupport"],
        )
    } else {
        (
            "agent-device-access",
            "enableAgentDeviceAccess",
            "Agent device access",
            &["enableAgentDeviceAccess"],
        )
    };
    let disabled = move || {
        let Some(primary) = p.primary else {
            return true;
        };
        if p.pending.get() || !primary.session.get().writable() {
            return true;
        }
        if p.project() {
            return hub;
        }
        if primary.state.with(|s| s.as_ref().is_none_or(|s| busy(s))) {
            return true;
        }
        !hub && !p
            .settings()
            .iter()
            .any(|(_, s)| s["enableDeviceSupport"] == true)
    };
    settings_row(
        SettingsRowProps::searchable(id)
            .server(keys)
            .description(move || {
                let version = p
                    .primary
                    .and_then(|m| m.state.get())
                    .and_then(|s| {
                        s.hosts
                            .iter()
                            .find(|h| h.kind == DeviceHostKind::Local)
                            .and_then(|h| h.tools.as_ref()?.as_ref())
                            .map(|tools| {
                                let v = if hub { &tools.hub } else { &tools.agent };
                                v.running_version
                                    .clone()
                                    .or_else(|| {
                                        v.installed_versions
                                            .iter()
                                            .find(|n| *n == &v.required_version)
                                            .cloned()
                                    })
                                    .or_else(|| v.installed_versions.last().cloned())
                                    .map(|s| format!("v{s}"))
                                    .unwrap_or_else(|| t("Not installed"))
                            })
                    })
                    .unwrap_or_else(|| t("Version unknown"));
                let prefix = if hub {
                    "Open simulators and emulators in the Device panel. Hub: {version}"
                } else {
                    "New agent sessions can open and drive these devices. agent-device: {version}"
                };
                Text::content(crate::i18n::tr(
                    prefix,
                    &[("version", crate::i18n::Value::Text(version))],
                ))
            })
            .control(move || {
                view()
                    .app_class(tw("flex flex-row items-center gap-2"))
                    .child(tool_actions(p, hub))
                    .child(switch(
                        label,
                        ControlState {
                            checked: Prop::bind(move || {
                                !p.context.mixed(keys) && p.context.setting(key) == true
                            }),
                            mixed: Prop::bind(move || p.context.mixed(keys)),
                            disabled: Prop::bind(disabled),
                            ..Default::default()
                        },
                        Size::Default,
                        move |checked| {
                            if disabled() {
                                return;
                            }
                            if !hub && p.project() {
                                p.context.update(json!({key:checked}));
                                return;
                            }
                            let Some(scope) = p.context.view() else {
                                return;
                            };
                            let members = p.members.get();
                            p.pending.set(true);
                            let targets = scope
                                .selected
                                .environments
                                .iter()
                                .map(|e| {
                                    (
                                        e.label.clone(),
                                        members
                                            .iter()
                                            .find(|m| m.member.get().id == e.environment_id)
                                            .map(|m| m.session.get()),
                                    )
                                })
                                .collect::<Vec<_>>();
                            p.tasks.spawn(async move {
                                let failed = futures::future::join_all(targets.into_iter().map(
                                    |(label, session)| async move {
                                        let Some(session) = session else {
                                            return Some(label);
                                        };
                                        let agent = if hub {
                                            (!checked).then_some(false)
                                        } else {
                                            Some(checked)
                                        };
                                        session
                                            .configure(hub.then_some(checked), agent)
                                            .await
                                            .err()
                                            .map(|_| label)
                                    },
                                ))
                                .await
                                .into_iter()
                                .flatten()
                                .collect::<Vec<_>>();
                                p.pending.set(false);
                                if !failed.is_empty() {
                                    error(
                                        "Device settings not saved on all environments",
                                        format!("Could not update {}.", failed.join(", ")),
                                    );
                                }
                            });
                        },
                    ))
            }),
    )
    .into_element()
}
fn tool_actions(p: Page, hub: bool) -> AnyElement {
    let Some(primary) = p.primary else {
        return view().into_element();
    };
    region(
        move || {
            primary.state.get().map(|s| {
                let tools = s
                    .hosts
                    .iter()
                    .find(|h| h.kind == DeviceHostKind::Local)
                    .and_then(|h| h.tools.as_ref()?.as_ref())
                    .map(|t| if hub { &t.hub } else { &t.agent });
                (
                    s.supports_tool_inspection.flatten() == Some(true),
                    s.supports_tool_update.flatten() == Some(true)
                        && tools
                            .is_some_and(|t| !t.installed_versions.contains(&t.required_version)),
                    busy(&s),
                )
            })
        },
        move |state| {
            state.map(|(inspect, update, busy)| {
                let run = move |check: bool| {
                    if p.pending.get_untracked() || busy {
                        return;
                    }
                    p.pending.set(true);
                    let session = primary.session.get();
                    let input = DeviceListInput {
                        inspect_only: check.then_some(Some(true)),
                        update_tool: (!check).then_some(Some(if hub {
                            DeviceUpdateTool::Hub
                        } else {
                            DeviceUpdateTool::Agent
                        })),
                        retry_host_id: None,
                    };
                    p.tasks.spawn(async move {
                        match session.list(input).await {
                            Ok(state) => primary.state.set(Some(Rc::new(state))),
                            Err(err) if !check => error("Update", err.to_string()),
                            _ => {}
                        }
                        p.pending.set(false);
                    });
                };
                view()
                    .app_class(tw("flex flex-wrap gap-2"))
                    .child(update.then(|| {
                        btn(
                            "Update",
                            move || busy || p.pending.get(),
                            move || run(false),
                        )
                    }))
                    .child(inspect.then(|| {
                        btn(
                            "Check versions",
                            move || busy || p.pending.get(),
                            move || run(true),
                        )
                    }))
            })
        },
    )
}
fn btn(
    label: &'static str,
    disabled: impl Fn() -> bool + 'static,
    run: impl Fn() + 'static,
) -> AnyElement {
    button(
        ButtonContent::Label(t(label)),
        ButtonVariant::Outline,
        ButtonSize::Sm,
        disabled,
        run,
    )
    .into_element()
}
fn error(title: &'static str, description: String) {
    toast_manager().add(ToastOptions {
        variant: Variant::Error,
        title: Text::from(title),
        description: Some(Text::content(description)),
        ..Default::default()
    });
}
