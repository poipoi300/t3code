use super::sources::{Editing, Editor, save};
use super::*;
use crate::ui::{
    dialog::{
        FooterVariant, dialog, dialog_close, dialog_description, dialog_footer, dialog_header,
        dialog_panel, dialog_popup, dialog_title,
    },
    select::{SelectItem, SelectProps, Size as SelectSize, Value as Selection, select},
};
use std::collections::BTreeMap;
use t3_client::services::settings::devices::{HostCheck, SourceEdit, valid_device_token};
#[derive(Clone, Copy)]
struct Form {
    page: Page,
    editor: Editor,
    original: Stored<Editing>,
    draft: Signal<Value>,
    tokens: Signal<BTreeMap<EnvironmentId, Value>>,
    sequence: Signal<u64>,
    gateway_edited: Signal<bool>,
    checks: Signal<BTreeMap<EnvironmentId, CheckState>>,
    tasks: TaskOwner,
}
#[derive(Clone, PartialEq)]
enum CheckState {
    Busy(u64),
    Ready(HostCheck),
    Failed,
}
impl Form {
    fn kind(self) -> HostKind {
        self.original.with(|v| v.kind)
    }
    fn valid(self) -> bool {
        self.kind().valid(&self.draft.get())
            && self
                .tokens
                .with(|tokens| tokens.values().all(valid_device_token))
    }
    fn commit(self) {
        if !self.valid() {
            return;
        }
        let mut tokens = self.tokens.get_untracked();
        if self.kind() == HostKind::Eas
            && let Some(value) = tokens.values().next().cloned()
        {
            tokens = self.page.members.with(|members| {
                members
                    .iter()
                    .map(|m| (m.member.get().id, value.clone()))
                    .collect()
            });
        }
        save(
            self.page,
            self.editor,
            SourceEdit {
                kind: self.kind(),
                original: self.original.get().original,
                draft: self.draft.get_untracked(),
                remove: false,
                tokens,
            },
        );
    }
    fn check(self, m: Live) {
        let session = m.session.get();
        let env = m.member.get().id;
        let kind = self.kind();
        let original = self.original.get().original;
        let host_id = original.as_ref().and_then(|r| r.holders.get(&env)).cloned();
        let token = self.tokens.with_untracked(|t| t.get(&env).cloned());
        let draft = self.draft.get_untracked();
        if token.as_ref().is_some_and(|v| !valid_device_token(v))
            || (kind != HostKind::Eas && !kind.valid(&draft))
        {
            return;
        }
        let attempt = self.sequence.get_untracked() + 1;
        self.sequence.set(attempt);
        self.checks.update(|c| {
            // Changes to connection inputs erase this entry, including a switch back to earlier inputs.
            c.insert(env.clone(), CheckState::Busy(attempt));
        });
        self.tasks.spawn(async move {
            let answer = session
                .check(kind, &draft, token.as_ref(), host_id.as_deref())
                .await;
            self.checks.update(|c| {
                if c.get(&env) == Some(&CheckState::Busy(attempt)) {
                    c.insert(
                        env,
                        answer.map(CheckState::Ready).unwrap_or(CheckState::Failed),
                    );
                }
            });
        });
    }
    fn set_field(self, key: &'static str, raw: &str, numeric: bool, optional: bool) {
        if key == "gatewayUrl" {
            self.gateway_edited.set(true);
        }
        self.draft.update(|d| {
            if optional && raw.trim().is_empty() {
                if let Some(m) = d.as_object_mut() {
                    m.remove(key);
                }
            } else if numeric {
                d[key] = raw.trim().parse::<u64>().map_or(Value::Null, |n| json!(n));
            } else {
                d[key] = json!(raw.trim());
            }
        });
        if matches!(key, "target" | "port" | "identityFile" | "gatewayUrl") {
            self.checks.set(BTreeMap::new());
        }
    }
}
pub(super) fn form(page: Page, editor: Editor, edit: Editing) -> AnyElement {
    let open = signal(true);
    let is_new = edit.original.is_none();
    let kind = edit.kind;
    let f = Form {
        page,
        editor,
        draft: signal(edit.draft.clone()),
        original: stored(edit),
        tokens: signal(Default::default()),
        checks: signal(Default::default()),
        sequence: signal(0),
        gateway_edited: signal(false),
        tasks: TaskOwner::here(),
    };
    let eas_peer = page
        .primary
        .filter(|m| {
            f.original.with(|e| {
                e.original
                    .as_ref()
                    .is_none_or(|r| r.holders.contains_key(&m.member.get().id))
            })
        })
        .or_else(|| {
            page.members.with(|members| {
                members
                    .iter()
                    .find(|m| {
                        f.original.with(|e| {
                            e.original
                                .as_ref()
                                .is_none_or(|r| r.holders.contains_key(&m.member.get().id))
                        })
                    })
                    .copied()
            })
        });
    if kind == HostKind::Eas
        && let Some(m) = eas_peer
    {
        f.check(m);
    }
    if kind == HostKind::Simlock {
        if is_new {
            let sessions = page
                .members
                .with(|members| members.iter().map(|m| m.session.get()).collect::<Vec<_>>());
            f.tasks.spawn(async move {
                let gateways =
                    futures::future::join_all(sessions.iter().map(DeviceSession::machine_gateway))
                        .await;
                if !f.gateway_edited.get_untracked()
                    && let Some(gateway) = gateways
                        .into_iter()
                        .filter_map(Result::ok)
                        .find(|url| !url.is_empty())
                {
                    f.draft.update(|draft| draft["gatewayUrl"] = json!(gateway));
                }
            });
        }
        for member in page.members.get() {
            let id = member.member.get().id;
            let inputs = computed(move || {
                (
                    f.draft.with(|draft| draft["gatewayUrl"].clone()),
                    f.tokens.with(|tokens| tokens.get(&id).cloned()),
                )
            });

            effect(move || {
                inputs.get();
                f.check(member);
            });
        }
    }
    let title = match (kind, is_new) {
        (HostKind::Ssh, true) => "Add device host",
        (HostKind::Ssh, false) => "Edit device host",
        (HostKind::Eas, true) => "Add EAS host",
        (HostKind::Eas, false) => "Edit EAS host",
        (HostKind::Simlock, true) => "Add Simlock host",
        (HostKind::Simlock, false) => "Edit Simlock host",
    };
    let detail = match kind {
        HostKind::Ssh => format!(
            "Connect from {}. Hosts on the same machine are skipped.",
            page.members.with(|m| m
                .iter()
                .map(|m| m.member.get().label)
                .collect::<Vec<_>>()
                .join(", "))
        ),
        HostKind::Eas => t(
            "The environment's server signs in to Expo and starts the sessions. Without a token here it uses EXPO_TOKEN, then its eas login.",
        ),
        HostKind::Simlock => t(if page.members.with(|m| m.len() > 1) {
            "Each selected environment's server leases devices from your Simlock gateway with its own agent token, and releases them when they close."
        } else {
            "The environment's server leases devices from your Simlock gateway and releases them when they close."
        }),
    };
    dialog(open, move |state| {
        state.on_open_change(move |next| {
            if !next && !page.pending.get_untracked() {
                editor.set(None);
            }
        });
        dialog_popup(state, Default::default(), move || {
            let header = dialog_header(
                state,
                view().child(dialog_title(state, move || Text::from(title))).child(dialog_description(state, {
                    let detail = detail.clone();
                    move || Text::content(detail.clone())
                })),
            );

            let mut fields = view().app_class(tw("flex min-w-0 flex-col gap-3")).child(field(f, "label", "Name", false, false));

            match kind {
                HostKind::Ssh => {
                    fields = fields.child(field(f, "target", "SSH target", false, false)).child(field(f, "port", "Port", true, true)).child(field(f, "identityFile", "Identity file", false, true));

                    let members = page.members.get();
                    fields = fields.child(btn(
                        "Test connection",
                        move || !f.valid() || page.pending.get(),
                        move || {
                            for &m in &members {
                                f.check(m);
                            }
                        },
                    ));

                    for m in page.members.get() {
                        fields = fields.child(check_status(f, m));
                    }
                }
                HostKind::Eas => {
                    if let Some(m) = eas_peer {
                        fields = fields.child(token(f, m, "Expo access token")).child(check_status(f, m)).child(btn("Check account", move || page.pending.get(), move || f.check(m))).child(project_picker(f, m));
                    }
                    for (key, label) in [("maxConcurrentSessions", "Sessions at once"), ("maxSessionMinutes", "Session minutes"), ("idleMinutes", "Idle minutes")] {
                        fields = fields.child(field(f, key, label, true, false));
                    }
                    fields = fields.child(text(t("Expo allows sessions of up to 40 minutes on free accounts and 115 on paid ones, and ends one after the idle minutes without activity, so idle minutes must be smaller than session minutes.")));

                    if !is_new {
                        fields = fields.child(text(t("Saving restarts this host, which stops its running sessions.")));
                    }
                }
                HostKind::Simlock => {
                    fields = fields.child(field(f, "gatewayUrl", "Gateway URL", false, false));
                    for m in page.members.get() {
                        fields = fields.child(token(f, m, "Agent token")).child(check_status(f, m)).child(btn("Check identity", move || !f.valid() || page.pending.get(), move || f.check(m)));
                    }
                    fields = fields.child(field(f, "maxLeases", "Devices at once", true, false)).child(field(f, "maxLeaseMinutes", "Lease minutes", true, false)).child(text(t("A lease ends after the lease minutes even while in use.")));

                    if !is_new {
                        fields = fields.child(text(t("Saving restarts this host, which releases its devices.")));
                    }
                }
            }
            let footer = dialog_footer(
                state,
                FooterVariant::Default,
                view()
                    .app_class(tw("flex flex-row gap-2"))
                    .child(dialog_close(state, ButtonContent::Label(t("Cancel")), ButtonVariant::Ghost, ButtonSize::Default, move || page.pending.get()))
                    .child(button(ButtonContent::Label(t("Save host")), ButtonVariant::Default, ButtonSize::Default, move || !f.valid() || !page.can_edit(), move || f.commit())),
            );

            view().child(header).child(dialog_panel(state, fields)).child(footer)
        })
    })
    .into_element()
}
fn field(
    f: Form,
    key: &'static str,
    label: &'static str,
    numeric: bool,
    optional: bool,
) -> AnyElement {
    let initial = f.draft.with_untracked(|d| match &d[key] {
        Value::String(v) => v.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    });
    let shown = signal(initial);
    if key == "gatewayUrl" {
        effect(move || {
            if !f.gateway_edited.get() {
                shown.set(
                    f.draft
                        .with(|draft| draft[key].as_str().unwrap_or_default().to_owned()),
                );
            }
        });
    }
    view()
        .app_class(tw("flex flex-col gap-1.5 text-sm"))
        .child(text(t(label)))
        .child(
            nsh_base::input()
                .label(t(label))
                .value(move || shown.get())
                .disabled(move || f.page.pending.get())
                .keyboard_type(if numeric {
                    KeyboardType::Number
                } else {
                    KeyboardType::Default
                })
                .on_change(move |raw: &str| {
                    shown.set(raw.into());
                    f.set_field(key, raw, numeric, optional);
                })
                .app_class(tw(
                    "w-full rounded-lg border border-input px-3 py-1.5 text-sm",
                )),
        )
        .into_element()
}
fn token(f: Form, m: Live, label: &'static str) -> AnyElement {
    let id = m.member.get().id;
    let key = f.kind().token().unwrap_or_default();
    let saved = f
        .page
        .settings()
        .into_iter()
        .find(|(env, _)| env == &id)
        .and_then(|(_, s)| {
            s[f.kind().key()]
                .as_array()?
                .iter()
                .find(|h| h["id"] == f.draft.get_untracked()["id"])
                .map(|h| h[key].clone())
        })
        .unwrap_or(json!(""));
    let name = if f.kind() == HostKind::Simlock && f.page.members.with(|v| v.len() > 1) {
        format!("{} · {}", t(label), m.member.get().label)
    } else {
        t(label)
    };
    let display = stored(name);
    let id = stored(id);
    let secret = signal(String::new());
    let configured = saved.is_object() || saved.as_str().is_some_and(|s| !s.is_empty());
    let reference = signal(saved.is_object());
    let provider = signal(saved["provider"].as_str().unwrap_or_default().to_owned());
    let path = signal(saved["reference"].as_str().unwrap_or_default().to_owned());
    let saved_reference = saved.is_object();
    let restore_on_blank = configured && (f.kind() == HostKind::Eas || saved.is_string());
    let change = move |value: Option<Value>| {
        f.tokens.update(|v| {
            if let Some(value) = value {
                v.insert(id.get(), value);
            } else {
                v.remove(&id.get());
            }
        });
        f.checks.update(|s| {
            s.remove(&id.get());
        });
    };
    view()
        .app_class(tw("flex flex-col gap-1.5 text-sm"))
        .child(text(display.get()))
        .child(region(
            move || reference.get(),
            move |is_ref| {
                if is_ref {
                    view()
                        .child(
                            nsh_base::input()
                                .label(t("Secret provider"))
                                .value(move || provider.get())
                                .on_change(move |s: &str| {
                                    provider.set(s.into());
                                    change(Some(json!({
"provider":s,
"reference":path.get_untracked()})));
                                }),
                        )
                        .child(
                            nsh_base::input()
                                .label(t("Secret reference"))
                                .value(move || path.get())
                                .on_change(move |s: &str| {
                                    path.set(s.into());
                                    change(Some(json!({
"provider":provider.get_untracked(),
"reference":s})));
                                }),
                        )
                        .into_element()
                } else {
                    nsh_base::input()
                        .masked(true)
                        .label(display.get())
                        .placeholder(if configured {
                            t("Configured")
                        } else {
                            t("Optional")
                        })
                        .value(move || secret.get())
                        .on_change(move |s: &str| {
                            secret.set(s.into());
                            change(
                                (!s.trim().is_empty() || !restore_on_blank)
                                    .then(|| json!(s.trim())),
                            );
                        })
                        .app_class(tw(
                            "w-full rounded-lg border border-input px-3 py-1.5 text-sm",
                        ))
                        .into_element()
                }
            },
        ))
        .child(
            view()
                .app_class(tw("flex flex-row gap-2"))
                .child(btn(
                    "Secret reference",
                    move || f.page.pending.get(),
                    move || {
                        let next = !reference.get_untracked();
                        reference.set(next);
                        change(if next {
                            Some(json!({
"provider":provider.get_untracked(),
"reference":path.get_untracked()}))
                        } else {
                            (!secret.get_untracked().trim().is_empty()
                                || !restore_on_blank
                                || saved_reference)
                                .then(|| json!(secret.get_untracked().trim()))
                        });
                    },
                ))
                .child(btn(
                    if f.kind() == HostKind::Eas {
                        "Clear access token"
                    } else {
                        "Clear agent token"
                    },
                    move || f.page.pending.get(),
                    move || {
                        secret.set(String::new());
                        reference.set(false);
                        change(Some(json!("")));
                    },
                )),
        )
        .into_element()
}
fn check_status(f: Form, m: Live) -> AnyElement {
    region(
        move || f.checks.with(|v| v.get(&m.member.get().id).cloned()),
        move |check| {
            check.map(|status| {
                let label = match status {
                    CheckState::Busy(_) => t("Checking…"),
                    CheckState::Failed => t("Check failed"),
                    CheckState::Ready(HostCheck::Ssh(_)) => t("Connected"),
                    CheckState::Ready(HostCheck::Eas(a)) => {
                        format!("{} {}", t("Signed in as"), a.signed_in_as)
                    }
                    CheckState::Ready(HostCheck::Simlock(a)) => {
                        format!("{} {}", t("Signed in as"), a.signed_in_as)
                    }
                };
                text(if f.page.members.with(|v| v.len() > 1) {
                    format!("{}: {label}", m.member.get().label)
                } else {
                    label
                })
                .app_class(tw("text-xs text-muted-foreground"))
            })
        },
    )
}
fn project_picker(f: Form, m: Live) -> AnyElement {
    region(
        move || {
            f.checks.with(|c| match c.get(&m.member.get().id) {
                Some(CheckState::Ready(HostCheck::Eas(a))) => a.apps.clone(),
                _ => vec![],
            })
        },
        move |apps| {
            let mut props = SelectProps::new("EAS project");
            let mut options = vec![SelectItem::new("", t("From the thread's project"))];
            if let Some(id) = f
                .draft
                .with_untracked(|d| d["appId"].as_str().map(str::to_owned))
                && !apps.iter().any(|a| a.id == id)
            {
                options.push(SelectItem::new(id.clone(), id));
            }
            options.extend(apps.into_iter().map(|a| SelectItem::new(a.id, a.full_name)));
            props.items = options.into();
            props.size = SelectSize::Sm;
            props.value = Prop::bind(move || {
                Selection::Controlled(Some(
                    f.draft
                        .with(|d| d["appId"].as_str().unwrap_or_default().to_owned().into()),
                ))
            });
            select(props, move |value| {
                if let Some(v) = value {
                    f.set_field("appId", &v, false, true);
                }
            })
        },
    )
}
