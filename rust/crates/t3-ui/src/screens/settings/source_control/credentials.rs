//! Port of SourceControlTokenSettings, SourceControlAccountSettings and SourceControlAppSignInSettings.
use nsh_core::prelude::*;
use t3_client::{
    services::settings::service::source_control::{
        ForgeCredentialEdit, HostCredentials, SavedToken, SignInAction, credentials,
        group_host_accounts, normalize_forge_host, sign_in_target, token_management_url,
    },
    types::EnvironmentId,
};
use t3_contracts::{
    secret_reference::{SecretProviderId, SecretReference},
    settings_secret_value::SettingsSecretValue,
    source_control::{
        SourceControlAppAccessStatus, SourceControlProviderDiscoveryItem,
        SourceControlProviderKind, SourceControlSignInPrompt,
    },
};

use super::{Page, option_value, region};
use crate::{
    i18n::{Text, t, tr},
    runtime::services::links::link_opener_here,
    theme::tw,
    ui::{
        button::{ButtonContent, ButtonSize, ButtonVariant, button},
        icon::Icon,
        select::{SelectItem, SelectProps, Size as SelectSize, Value as SelectValue, select},
        switch::{ControlState, Size, switch},
    },
};

#[derive(Clone, Copy)]
pub(super) struct Definition {
    public_host: &'static str,
    cli: &'static str,
    cli_label: &'static str,
    login: &'static str,
    variable: &'static str,
    access: &'static str,
    token_url: &'static str,
}

pub(super) fn definition(provider: &str) -> Option<Definition> {
    match provider {
        "github" => Some(Definition {
            public_host: "github.com",
            cli: "gh",
            cli_label: "GitHub CLI",
            login: "gh auth login",
            variable: "GH_TOKEN",
            access: "read and write access to pull requests and contents",
            token_url: "/settings/personal-access-tokens/new",
        }),
        _ => None,
    }
}

#[derive(Clone, Copy)]
struct HostPanel {
    page: Page,
    id: Stored<EnvironmentId>,
    provider: Stored<SourceControlProviderKind>,
    label: Stored<String>,
    definition: Definition,
    saved: Memo<HostCredentials>,
    saving: Signal<bool>,
    failure: Signal<bool>,
}

impl HostPanel {
    fn commit(self, edit: ForgeCredentialEdit, complete: impl FnOnce() + 'static) {
        if self.saving.get_untracked() || !self.page.writable() {
            return;
        }
        self.saving.set(true);
        self.failure.set(false);
        let pending = self.page.edit(self.id.get(), self.provider.get(), edit);
        self.page.tasks.spawn(async move {
            let result = pending.await;
            let _ = self.saving.try_set(false);
            if result.is_ok() {
                complete();
                self.page.rescan();
            } else {
                let _ = self.failure.try_set(true);
            }
        });
    }
    fn disabled(self) -> bool {
        self.saving.get() || !self.page.writable()
    }
}

pub(super) fn host_settings(
    page: Page,
    id: EnvironmentId,
    item: SourceControlProviderDiscoveryItem,
) -> AnyElement {
    let Some(definition) = definition(&item.kind.0) else {
        return view().into_element();
    };
    let provider = stored(item.kind.clone());
    let saved = computed(move || {
        page.context
            .view()
            .map_or_else(HostCredentials::default, |view| {
                provider.with(|provider| credentials(&view, &provider.0, definition.public_host))
            })
    });
    let panel = HostPanel {
        page,
        id: stored(id),
        provider,
        label: stored(item.label.0.clone()),
        definition,
        saved,
        saving: signal(false),
        failure: signal(false),
    };
    let app_sign_in = item.kind.0 == "github" && item.app_sign_in == Some(true);
    view()
        .app_class(tw("flex flex-col gap-4"))
        .child(app_sign_in.then(|| {
            app_sign_in_settings(
                panel,
                option_value(&item.auth.account).map(|name| name.0.clone()),
            )
        }))
        .child(app_sign_in.then(|| {
            text(t("Or use your own credentials")).app_class(tw("text-xs text-muted-foreground"))
        }))
        .child(app_sign_in.then(|| app_access_notice(panel)))
        .child(token_settings(panel))
        .child(account_settings(
            panel,
            item.auth.accounts.unwrap_or_default(),
        ))
        .child(when(
            move || panel.failure.get(),
            || {
                text(t("Could not save source control settings."))
                    .app_class(tw("text-xs text-destructive"))
            },
        ))
        .into_element()
}

fn credential_field(
    value: Signal<String>,
    label: String,
    placeholder: impl Fn() -> String + 'static,
    masked: bool,
    disabled: impl Fn() -> bool + 'static,
    change: impl Fn(String) + 'static,
    submit: impl Fn() + 'static,
) -> impl IntoElement {
    nsh_base::input()
        .label(label)
        .placeholder(placeholder)
        .masked(masked)
        .disabled(disabled)
        .value(move || value.get())
        .on_change(move |value: &str| change(value.to_owned()))
        .submit_behavior(SubmitBehavior::Submit)
        .on_submit(move |_: &str| submit())
        .app_class(tw(
            "h-8 w-full rounded-lg border border-input bg-background px-[11px] text-sm",
        ))
}

fn token_settings(panel: HostPanel) -> impl IntoElement {
    let opener = link_opener_here();
    let host = signal(panel.definition.public_host.to_owned());
    let token = signal(String::new());
    let reference = signal(String::new());
    let provider_source = signal(None::<String>);
    let selected_host = computed(move || normalize_forge_host(&host.get()));
    let current = computed(move || {
        panel
            .saved
            .with(|saved| saved.tokens.get(&selected_host.get()).cloned())
    });
    effect(move || {
        let _ = selected_host.get();
        token.set(String::new());
        match current.get() {
            Some(SavedToken::Reference(saved)) => {
                provider_source.set(Some(saved.provider.0));
                reference.set(saved.reference);
            }
            _ => {
                provider_source.set(None);
                reference.set(String::new());
            }
        }
    });
    let ready = move || {
        if panel.disabled() || selected_host.with(String::is_empty) {
            return false;
        }
        if provider_source.with(Option::is_some) {
            reference.with(|reference| !reference.trim().is_empty())
        } else {
            token.with(|token| !token.trim().is_empty())
        }
    };
    let submit = move || {
        if !ready() {
            return;
        }
        let value = provider_source.get_untracked().map_or_else(
            || SettingsSecretValue::Literal(token.with_untracked(|value| value.trim().to_owned())),
            |provider| {
                SettingsSecretValue::Reference(SecretReference {
                    provider: SecretProviderId(provider),
                    reference: reference.with_untracked(|value| value.trim().to_owned()),
                })
            },
        );
        panel.commit(
            ForgeCredentialEdit::Token {
                host: selected_host.get_untracked(),
                value,
            },
            move || {
                let _ = token.try_set(String::new());
            },
        );
    };
    let token_label = tr(
        "{provider} token",
        &[("provider", t3_i18n::Value::Text(panel.label.get()))],
    );
    let field_label = token_label.clone();
    view().app_class(tw("flex flex-col gap-3"))
        .child(text(tr(
            "A token saved here is used before {variable} and the {cli} login, so {provider} works without the {cliLabel}. Give it {access}.",
            &[
                ("variable", t3_i18n::Value::Text(panel.definition.variable.into())),
                ("cli", t3_i18n::Value::Text(panel.definition.cli.into())),
                ("provider", t3_i18n::Value::Text(panel.label.get())),
                ("cliLabel", t3_i18n::Value::Text(panel.definition.cli_label.into())),
                ("access", t3_i18n::Value::Text(panel.definition.access.into())),
            ],
        )).app_class(tw("max-w-2xl text-xs text-muted-foreground")))
        .child(region(
            move || token_management_url(&selected_host.get(), panel.definition.token_url),
            move |url| url.map(|url| {
                let opener = opener.clone();
                button(ButtonContent::IconLabel {
                    icon: Icon::ExternalLink, label: t("Create a token"),
                }, ButtonVariant::Link, ButtonSize::Xs, move || panel.disabled(), move || opener.open(url.clone()))
            }),
        ))
        .child(view().app_class(tw("flex flex-col gap-3 sm:flex-row"))
            .child(view().app_class(tw("flex flex-col gap-1.5 sm:w-48"))
                .child(text(t("Host")).app_class(tw("text-sm font-medium")))
                .child(credential_field(host, t("Host"), move || panel.definition.public_host.to_owned(),
                    false, move || panel.disabled(), move |value| host.set(value), submit)))
            .child(view().app_class(tw("flex min-w-0 flex-1 flex-col gap-1.5"))
                .child(text(t("Token")).app_class(tw("text-sm font-medium")))
                .child(view().app_class(tw("flex flex-row items-center gap-2"))
                    .child(region(
                        move || (panel.saved.with(|saved| saved.sources.clone()), provider_source.get()),
                        move |(sources, selected)| {
                            if sources.is_empty() && selected.is_none() { return None; }
                            let mut items = vec![SelectItem::new("", t("Value"))];
                            items.extend(sources.iter().map(|(id, label)| SelectItem::new(id.clone(), label.clone())));
                            if let Some(selected) = selected.filter(|id| !sources.iter().any(|(key, _)| key == id)) {
                                items.push(SelectItem::new(selected.clone(), selected));
                            }
                            let mut props = SelectProps::new(Text::content(tr("{label} source", &[
                                ("label", t3_i18n::Value::Text(token_label.clone()))
                            ])));
                            props.items = items.into();
                            props.size = SelectSize::Sm;
                            props.disabled = Prop::bind(move || panel.disabled());
                            props.value = Prop::bind(move || SelectValue::Controlled(Some(provider_source.get().unwrap_or_default().into())));
                            Some(select(props, move |value| {
                                token.set(String::new());
                                reference.set(String::new());
                                provider_source.set(value.filter(|value| !value.is_empty()).map(|value| value.to_string()));
                            }))
                        },
                    ))
                    .child(region(
                        move || provider_source.with(Option::is_some),
                        move |use_reference| {
                            let label = field_label.clone();
                            if use_reference {
                                credential_field(reference, label, || t("Secret reference"),
                                    false, move || panel.disabled(), move |value| reference.set(value), submit).into_element()
                            } else {
                                credential_field(token, label, move || {
                                    if current.get() == Some(SavedToken::Stored) {
                                        t("Stored secret, enter a new value to replace")
                                    } else { t("Not set") }
                                }, true, move || panel.disabled(), move |value| token.set(value), submit).into_element()
                            }
                        },
                    )))))
        .child(view().app_class(tw("flex flex-row flex-wrap items-center justify-between gap-3"))
            .child(text(move || {
                let saved = panel.saved.with(|saved| saved.tokens.keys().cloned().collect::<Vec<_>>());
                if saved.is_empty() {
                    tr("No token saved; the server uses {variable} or the {cli} login.", &[
                        ("variable", t3_i18n::Value::Text(panel.definition.variable.into())),
                        ("cli", t3_i18n::Value::Text(panel.definition.cli.into())),
                    ])
                } else {
                    tr("Saved for {hosts}.", &[("hosts", t3_i18n::Value::Text(saved.join(", ")))])
                }
            }).app_class(tw("text-xs text-muted-foreground")))
            .child(view().app_class(tw("flex shrink-0 flex-row gap-2"))
                .child(when(move || current.get().is_some(), move || button(
                    ButtonContent::Label(t("Remove")), ButtonVariant::Outline, ButtonSize::Xs,
                    move || panel.disabled(), move || panel.commit(ForgeCredentialEdit::Token {
                        host: selected_host.get_untracked(), value: SettingsSecretValue::Literal(String::new()),
                    }, move || { let _ = token.try_set(String::new()); }),
                )))
                .child(button(ButtonContent::Label(t("Save")), ButtonVariant::Default, ButtonSize::Xs,
                    move || !ready(), submit))))
}

fn account_settings(
    panel: HostPanel,
    entries: Vec<t3_contracts::source_control::SourceControlProviderAuthAccounts>,
) -> impl IntoElement {
    let groups = group_host_accounts(&entries);
    let revealed = signal(false);
    let mut rows = view().app_class(tw("flex flex-col gap-4"));
    if groups.is_empty() {
        return rows.child(
            text(tr(
                "Sign in with {command} on the server host, then rescan to choose accounts here.",
                &[(
                    "command",
                    t3_i18n::Value::Text(panel.definition.login.into()),
                )],
            ))
            .app_class(tw("text-xs text-muted-foreground")),
        );
    }
    rows = rows.child(
        view()
            .app_class(tw("flex flex-row items-center justify-between gap-3"))
            .child(
                text(tr(
                    "Choose which {cli} login each {provider} host uses, or turn a host off.",
                    &[
                        ("cli", t3_i18n::Value::Text(panel.definition.cli.into())),
                        ("provider", t3_i18n::Value::Text(panel.label.get())),
                    ],
                ))
                .app_class(tw("max-w-2xl text-xs text-muted-foreground")),
            )
            .child(region(
                move || revealed.get(),
                move |shown| {
                    button(
                        ButtonContent::Icon {
                            icon: if shown { Icon::EyeOff } else { Icon::Eye },
                            label: tr(
                                if shown {
                                    "Hide {provider} accounts"
                                } else {
                                    "Reveal {provider} accounts"
                                },
                                &[("provider", t3_i18n::Value::Text(panel.label.get()))],
                            ),
                        },
                        ButtonVariant::GhostMuted,
                        ButtonSize::IconXs,
                        || false,
                        move || revealed.update(|shown| *shown = !*shown),
                    )
                },
            )),
    );
    for group in groups {
        let group = stored(group);
        let host = group.with(|group| group.host.clone());
        let read_host = host.clone();
        let chosen = computed(move || {
            panel
                .saved
                .with(|saved| saved.hosts.get(&read_host).cloned())
        });
        let switch_host = host.clone();
        let enabled = move || chosen.get().is_none_or(|choice| choice.enabled);
        let switch_label = tr(
            "Use {provider} on {host}",
            &[
                ("provider", t3_i18n::Value::Text(panel.label.get())),
                ("host", t3_i18n::Value::Text(host.clone())),
            ],
        );
        let mut row = view().app_class(tw("flex flex-col gap-2")).child(
            view()
                .app_class(tw("flex flex-row items-center justify-between gap-3"))
                .child(text(host.clone()).app_class(tw("text-xs font-medium")))
                .child(switch(
                    switch_label,
                    ControlState {
                        checked: Prop::bind(enabled),
                        disabled: Prop::bind(move || panel.disabled()),
                        ..Default::default()
                    },
                    Size::Default,
                    move |enabled| {
                        panel.commit(
                            ForgeCredentialEdit::Account {
                                host: switch_host.clone(),
                                enabled: Some(enabled),
                                account: None,
                            },
                            || {},
                        )
                    },
                )),
        );
        if let Some(account) =
            group.with(|group| (group.selectable.len() == 1).then(|| group.selectable[0].clone()))
        {
            row = row.child(
                text(move || {
                    tr(
                        "Signed in as {account}",
                        &[(
                            "account",
                            t3_i18n::Value::Text(if revealed.get() {
                                account.clone()
                            } else {
                                t("Hidden account")
                            }),
                        )],
                    )
                })
                .app_class(tw("text-xs text-muted-foreground")),
            );
        }
        if group.with(|group| group.selectable.len() > 1) {
            let key = Text::content(tr(
                "{provider} account for {host}",
                &[
                    ("provider", t3_i18n::Value::Text(panel.label.get())),
                    ("host", t3_i18n::Value::Text(host.clone())),
                ],
            ));
            let mut props = SelectProps::new(key);
            props.items = Prop::bind(move || {
                group.with(|group| {
                    let active = tr(
                        "Active {cli} account",
                        &[("cli", t3_i18n::Value::Text(panel.definition.cli.into()))],
                    );
                    std::iter::once(SelectItem::new("", active))
                        .chain(group.selectable.iter().enumerate().map(|(index, name)| {
                            SelectItem::new(
                                index.to_string(),
                                if revealed.get() {
                                    name.clone()
                                } else {
                                    tr(
                                        "Account {number}",
                                        &[("number", t3_i18n::Value::Int(index as i64 + 1))],
                                    )
                                },
                            )
                        }))
                        .collect()
                })
            });
            props.value = Prop::bind(move || {
                let index = chosen
                    .get()
                    .and_then(|choice| choice.account)
                    .and_then(|account| {
                        group
                            .with(|group| group.selectable.iter().position(|name| name == &account))
                    });
                SelectValue::Controlled(Some(
                    index
                        .map(|index| index.to_string())
                        .unwrap_or_default()
                        .into(),
                ))
            });
            props.disabled = Prop::bind(move || panel.disabled() || !enabled());
            props.size = SelectSize::Sm;
            let select_host = host.clone();
            row = row.child(
                view()
                    .app_class(tw("flex flex-row items-center justify-between gap-3"))
                    .child(text(t("Account")).app_class(tw("text-xs text-muted-foreground")))
                    .child(select(props, move |next| {
                        let account =
                            next.and_then(|index| index.parse::<usize>().ok())
                                .and_then(|index| {
                                    group.with(|group| group.selectable.get(index).cloned())
                                });
                        panel.commit(
                            ForgeCredentialEdit::Account {
                                host: select_host.clone(),
                                enabled: None,
                                account: Some(account),
                            },
                            || {},
                        );
                    })),
            );
        }
        let stale = move || {
            chosen
                .get()
                .and_then(|choice| choice.account)
                .is_some_and(|account| group.with(|group| !group.selectable.contains(&account)))
        };
        row = row.child(when(stale, move || {
            let host = host.clone();
            view().app_class(tw("flex flex-row items-center justify-between gap-3"))
                .child(text(tr("The chosen login is no longer signed in, so the active {cli} login is used.",
                    &[("cli", t3_i18n::Value::Text(panel.definition.cli.into()))]))
                    .app_class(tw("text-xs text-warning")))
                .child(button(ButtonContent::Label(t("Use active login")), ButtonVariant::Outline,
                    ButtonSize::Xs, move || panel.disabled(), move || panel.commit(
                        ForgeCredentialEdit::Account { host:host.clone(), enabled:None, account:Some(None) }, || {}
                    )))
        }));
        rows = rows.child(row);
    }
    rows
}

fn app_access_notice(panel: HostPanel) -> impl IntoElement {
    let target = panel.page.context.view().and_then(|view| {
        let target = view.target()?;
        (target.environment_id == panel.id.get())
            .then(|| target.project_id.clone())
            .flatten()
    });
    let status = signal(None::<SourceControlAppAccessStatus>);
    if let Some((services, target)) = panel.page.services.get().zip(target) {
        let request = services.0.app_access(panel.id.get(), target);
        crate::runtime::tasks::watch(async move {
            let _ = status.try_set(request.await);
        });
    }
    region(
        move || status.get(),
        move |status| {
            status.map(|status| {
                let label = match status {
                    SourceControlAppAccessStatus::Covered => {
                        t("The GitHub app has access to this repository.")
                    }
                    SourceControlAppAccessStatus::NotCovered => {
                        t("The GitHub app does not have access to this repository.")
                    }
                    SourceControlAppAccessStatus::SignedOut => {
                        t("Sign in to check the GitHub app's repository access.")
                    }
                };
                text(label).app_class(tw("text-xs text-muted-foreground"))
            })
        },
    )
}

#[derive(Clone, PartialEq)]
enum SignInState {
    Idle,
    Starting,
    Waiting(SourceControlSignInPrompt),
    Failed,
}

fn app_sign_in_settings(panel: HostPanel, account: Option<String>) -> impl IntoElement {
    let opener = link_opener_here();
    let current = signal(SignInState::Idle);
    let attempt = signal(0u64);
    let target = stored(sign_in_target(
        panel.provider.get(),
        panel.definition.public_host,
    ));
    let can_sign_in = move || {
        panel
            .page
            .services
            .get()
            .is_some_and(|services| services.0.can_sign_in(&panel.id.get(), &target.get()))
    };
    let sign_in = move || {
        let Some(services) = panel.page.services.get() else {
            return;
        };
        let next = attempt.get_untracked() + 1;
        attempt.set(next);
        current.set(SignInState::Starting);
        let id = panel.id.get();
        let input = target.get();
        panel.page.tasks.spawn(async move {
            let prompt = services.0.start(id.clone(), input.clone()).await;
            if attempt.try_get() != Some(next) {
                return;
            }
            match prompt {
                Ok(prompt) => {
                    let _ = current.try_set(SignInState::Waiting(prompt));
                    let result = services.0.finish(id, input, SignInAction::Wait).await;
                    if attempt.try_get() != Some(next) {
                        return;
                    }
                    let _ = current.try_set(if result.is_ok() {
                        SignInState::Idle
                    } else {
                        SignInState::Failed
                    });
                    if result.is_ok() {
                        panel.page.rescan();
                    }
                }
                Err(()) => {
                    let _ = current.try_set(SignInState::Failed);
                }
            }
        });
    };
    let cancel = move || {
        attempt.update(|value| *value += 1);
        current.set(SignInState::Idle);
        if let Some(services) = panel.page.services.get() {
            let request = services
                .0
                .finish(panel.id.get(), target.get(), SignInAction::Cancel);
            panel.page.tasks.spawn(async move {
                let _ = request.await;
            });
        }
    };
    let revealed = signal(false);
    view().app_class(tw("flex flex-col overflow-hidden rounded-xl border border-border bg-card"))
        .child(region(move || (current.get(), panel.saved.with(|saved| saved.signed_in), revealed.get()), move |(status, signed_in, revealed_value)| {
            let (subtitle, label) = if signed_in {
                if panel.saved.with(|saved| saved.tokens.contains_key(panel.definition.public_host)) {
                    (t("Signed in, but the token saved below is used while it can be read."), t("Sign out"))
                } else if let Some(account) = &account {
                    (tr("Signed in as {account}", &[("account", t3_i18n::Value::Text(
                        if revealed_value { account.clone() } else { t("Hidden account") }
                    ))]), t("Sign out"))
                } else {
                    (t("Signed in"), t("Sign out"))
                }
            } else { match status {
                SignInState::Waiting(_) => (t("Approve the app on GitHub to finish signing in."), t("Cancel")),
                SignInState::Failed => (t("Could not finish signing in."), t("Try again")),
                _ => (t("A far higher GitHub API rate limit than a personal token or gh login."), t("Sign in with GitHub")),
            }};
            let waiting = matches!(status, SignInState::Waiting(_));
            let starting = matches!(status, SignInState::Starting);
            view().app_class(tw("flex flex-row items-center gap-3 p-3"))
                .child(view().app_class(tw("flex min-w-0 flex-1 flex-col gap-0.5"))
                    .child(text(t("T3 Code for GitHub")).app_class(tw("text-sm font-medium")))
                    .child(text(subtitle).app_class(tw("text-xs text-muted-foreground"))))
                .child(button(ButtonContent::Label(label),
                    if signed_in { ButtonVariant::Outline } else if waiting { ButtonVariant::Ghost } else { ButtonVariant::Default },
                    ButtonSize::Xs, move || !can_sign_in() || starting || panel.saving.get(), move || {
                        if signed_in {
                            panel.commit(ForgeCredentialEdit::SignOut { host: panel.definition.public_host.into() }, || {});
                        } else if waiting { cancel(); } else { sign_in(); }
                    }))
        }))
        .child(region(move || current.get(), move |status| {
            match status {
                SignInState::Waiting(prompt) => {
                    let code = prompt.user_code.0;
                    let uri = prompt.verification_uri.0;
                    let opener = opener.clone();
                    view().app_class(tw("flex flex-col gap-3 border-t border-border bg-muted/40 px-3 py-4 sm:flex-row"))
                        .child(text(code).app_class(tw("font-mono text-lg font-medium")))
                        .child(button(ButtonContent::IconLabel { icon: Icon::ExternalLink, label: t("Open GitHub") },
                            ButtonVariant::Link, ButtonSize::Xs, || false, move || opener.open(uri.clone()))).into_element()
                }
                _ => view().into_element(),
            }
        }))
}
