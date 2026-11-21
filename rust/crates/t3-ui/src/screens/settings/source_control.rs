//! Port of SourceControlSettings.tsx and its environment discovery and credential panels.
mod credentials;
mod routing;
mod writing;

use std::rc::Rc;

use futures::future::LocalBoxFuture;
use nsh_core::prelude::*;
use t3_client::{
    composition::ClientRuntime,
    persistence::RoutingAccess,
    platform::Platform,
    services::settings::service::{
        SettingsService,
        source_control::{ForgeCredentialEdit, ForgeResult, RoutingChoice, SignInAction},
    },
    types::EnvironmentId,
};
use t3_contracts::source_control::{
    SourceControlDiscoveryResult, SourceControlDiscoveryStatus, SourceControlProviderAuthStatus,
    SourceControlProviderDiscoveryItem, SourceControlProviderKind, SourceControlSignInInput,
    SourceControlSignInPrompt, WireOption,
};

use super::shell::{
    SettingsContext, SettingsRowProps, region, settings_page_container, settings_row,
    settings_section, use_settings,
};
use crate::{
    i18n::{Text, t, tr},
    runtime::tasks::{TaskOwner, watch},
    theme::tw,
    ui::{
        button::{ButtonContent, ButtonSize, ButtonVariant, button},
        icon::Icon,
        switch::{ControlState, Size, switch},
    },
};

trait ForgeClient {
    fn discover(
        &self,
        environment: EnvironmentId,
    ) -> LocalBoxFuture<'static, ForgeResult<SourceControlDiscoveryResult>>;
    fn edit(
        &self,
        environment: EnvironmentId,
        provider: SourceControlProviderKind,
        edit: ForgeCredentialEdit,
    ) -> LocalBoxFuture<'static, ForgeResult<()>>;
    fn can_write(&self, environment: &EnvironmentId) -> bool;
    fn set_offered(
        &self,
        environment: EnvironmentId,
        provider: SourceControlProviderKind,
        offered: bool,
    ) -> LocalBoxFuture<'static, ForgeResult<()>>;
    fn can_sign_in(&self, environment: &EnvironmentId, input: &SourceControlSignInInput) -> bool;
    fn start(
        &self,
        environment: EnvironmentId,
        input: SourceControlSignInInput,
    ) -> LocalBoxFuture<'static, ForgeResult<SourceControlSignInPrompt>>;
    fn finish(
        &self,
        environment: EnvironmentId,
        input: SourceControlSignInInput,
        action: SignInAction,
    ) -> LocalBoxFuture<'static, ForgeResult<()>>;
    fn routing(&self) -> LocalBoxFuture<'static, Vec<RoutingChoice>>;
    fn set_routing(
        &self,
        environment: EnvironmentId,
        access: RoutingAccess,
    ) -> LocalBoxFuture<'static, ForgeResult<()>>;
}

struct Client<P: Platform + Clone + 'static> {
    settings: SettingsService<P>,
}

impl<P: Platform + Clone + 'static> ForgeClient for Client<P> {
    fn discover(
        &self,
        environment: EnvironmentId,
    ) -> LocalBoxFuture<'static, ForgeResult<SourceControlDiscoveryResult>> {
        self.settings.discover_forges(environment)
    }
    fn edit(
        &self,
        environment: EnvironmentId,
        provider: SourceControlProviderKind,
        edit: ForgeCredentialEdit,
    ) -> LocalBoxFuture<'static, ForgeResult<()>> {
        self.settings
            .edit_forge_credentials(environment, provider, edit)
    }
    fn can_write(&self, environment: &EnvironmentId) -> bool {
        self.settings.forge_settings_writable(environment)
    }
    fn set_offered(
        &self,
        environment: EnvironmentId,
        provider: SourceControlProviderKind,
        offered: bool,
    ) -> LocalBoxFuture<'static, ForgeResult<()>> {
        self.settings
            .set_forge_offered(environment, provider, offered)
    }
    fn can_sign_in(&self, environment: &EnvironmentId, input: &SourceControlSignInInput) -> bool {
        self.settings.can_sign_in_to_forge(environment, input)
    }
    fn start(
        &self,
        environment: EnvironmentId,
        input: SourceControlSignInInput,
    ) -> LocalBoxFuture<'static, ForgeResult<SourceControlSignInPrompt>> {
        self.settings.start_forge_sign_in(environment, input)
    }
    fn finish(
        &self,
        environment: EnvironmentId,
        input: SourceControlSignInInput,
        action: SignInAction,
    ) -> LocalBoxFuture<'static, ForgeResult<()>> {
        self.settings
            .finish_forge_sign_in(environment, input, action)
    }
    fn routing(&self) -> LocalBoxFuture<'static, Vec<RoutingChoice>> {
        self.settings.routing_choices()
    }
    fn set_routing(
        &self,
        environment: EnvironmentId,
        access: RoutingAccess,
    ) -> LocalBoxFuture<'static, ForgeResult<()>> {
        self.settings.set_routing_choice(environment, access)
    }
}

#[derive(Clone)]
pub(super) struct Services(Rc<dyn ForgeClient>);

impl Services {
    pub(super) fn new<P: Platform + Clone + 'static>(runtime: &ClientRuntime<P>) -> Self {
        Self(Rc::new(Client {
            settings: runtime.settings(),
        }))
    }
}

#[derive(Clone, Copy)]
struct Page {
    context: SettingsContext,
    services: Stored<Option<Services>>,
    tasks: TaskOwner,
    refresh: Signal<u64>,
}

impl Page {
    fn rescan(self) {
        if let Some(revision) = self.refresh.try_get() {
            let _ = self.refresh.try_set(revision + 1);
        }
    }
    fn environment_writable(self, environment: &EnvironmentId) -> bool {
        let _ = self.context.view();
        self.services.with(|services| {
            services
                .as_ref()
                .is_some_and(|services| services.0.can_write(environment))
        })
    }
    fn edit(
        self,
        environment: EnvironmentId,
        provider: SourceControlProviderKind,
        edit: ForgeCredentialEdit,
    ) -> LocalBoxFuture<'static, ForgeResult<()>> {
        self.services.get().map_or_else(
            || Box::pin(async { Err(()) }) as LocalBoxFuture<'static, ForgeResult<()>>,
            |service| service.0.edit(environment, provider, edit),
        )
    }
}

pub fn screen() -> AnyElement {
    let page = Page {
        context: use_settings(),
        services: stored(use_context::<Services>()),
        tasks: TaskOwner::here(),
        refresh: signal(0),
    };
    settings_page_container(
        view().app_class(tw("flex flex-col gap-6"))
            .child(writing::repositories(page.context))
            .child(region(
                move || page.context.view().and_then(|view| {
                    view.selected.representative.as_ref()
                        .filter(|environment| environment.phase == t3_client::connection::Phase::Connected)
                        .map(|environment| environment.environment_id.clone())
                }),
                move |environment| match environment {
                    Some(environment) => source_control_discovery(page, environment).into_element(),
                    None => settings_section(
                        "source-control",
                        Text::from("Server environment"),
                        text(t("Connect an environment to inspect its version control tools and hosting integrations."))
                            .app_class(tw("px-4 py-3 text-sm text-muted-foreground")),
                    ).into_element(),
                },
            ))
            .child(writing::source_control_writing_settings_section(page.context))
            .child(routing::pull_request_routing_settings(page)),
    ).into_element()
}

fn source_control_discovery(page: Page, environment: EnvironmentId) -> impl IntoElement {
    let pending = signal(true);
    let result = signal(None::<ForgeResult<SourceControlDiscoveryResult>>);
    let services = page.services.get();
    let scan_environment = environment.clone();
    effect(move || {
        let revision = page.refresh.get();
        let Some(services) = services.clone() else {
            return;
        };
        pending.set(true);
        let request = services.0.discover(scan_environment.clone());
        watch(async move {
            let next = request.await;
            if page.refresh.try_get() == Some(revision) {
                let _ = pending.try_set(false);
                let _ = result.try_set(Some(next));
            }
        });
    });
    view()
        .app_class(tw("flex flex-col gap-6"))
        .child(
            view()
                .app_class(tw("flex flex-row justify-end"))
                .child(button(
                    ButtonContent::Icon {
                        icon: Icon::RefreshCw,
                        label: t("Rescan server environment"),
                    },
                    ButtonVariant::GhostMuted,
                    ButtonSize::IconXs,
                    move || pending.get(),
                    move || page.rescan(),
                )),
        )
        .child(region(
            move || result.get(),
            move |discovery| match discovery {
                Some(Ok(discovery)) => {
                    let mut vcs = view();
                    let mut providers = view();
                    let has_vcs = !discovery.version_control_systems.is_empty();
                    for item in discovery.version_control_systems {
                        let description = if !item.implemented {
                            Text::content(tr(
                                "Support for {name} is coming soon.",
                                &[("name", t3_i18n::Value::Text(item.label.0.clone()))],
                            ))
                        } else if item.status == SourceControlDiscoveryStatus::Available {
                            Text::from("Available")
                        } else {
                            Text::content(item.install_hint.0)
                        };
                        vcs = vcs.child(settings_row(
                            SettingsRowProps::titled("", Text::content(item.label.0))
                                .description(move || description.clone()),
                        ));
                    }
                    let has_providers = !discovery.source_control_providers.is_empty();
                    for item in discovery.source_control_providers {
                        providers =
                            providers.child(discovery_item_row(page, environment.clone(), item));
                    }
                    view()
                        .app_class(tw("flex flex-col gap-6"))
                        .child(has_vcs.then(|| {
                            settings_section("source-control", Text::from("Version Control"), vcs)
                        }))
                        .child(has_providers.then(|| {
                            settings_section(
                                if has_vcs { "" } else { "source-control" },
                                Text::from("Source control providers"),
                                providers,
                            )
                        }))
                        .child((!has_vcs && !has_providers).then(|| {
                            text(t("No version control tools discovered."))
                                .app_class(tw("px-4 py-3 text-sm text-muted-foreground"))
                        }))
                        .into_element()
                }
                Some(Err(())) => text(t("Could not scan this server environment."))
                    .app_class(tw("px-4 py-3 text-sm text-muted-foreground"))
                    .into_element(),
                None => settings_section(
                    "source-control",
                    Text::from("Version Control"),
                    text(t("Scanning server environment…"))
                        .app_class(tw("px-4 py-3 text-sm text-muted-foreground")),
                )
                .into_element(),
            },
        ))
}

fn discovery_item_row(
    page: Page,
    environment: EnvironmentId,
    item: SourceControlProviderDiscoveryItem,
) -> impl IntoElement {
    let environment = stored(environment);
    let item = stored(item);
    let open = signal(false);
    let expand = item.with(|item| {
        tr(
            "Toggle {provider} details",
            &[("provider", t3_i18n::Value::Text(item.label.0.clone()))],
        )
    });
    let offered = item.with(|item| {
        tr(
            "Offer {provider} when adding or publishing a project",
            &[("provider", t3_i18n::Value::Text(item.label.0.clone()))],
        )
    });
    let supported = item.with(|item| credentials::definition(&item.kind.0).is_some());
    settings_row(
        SettingsRowProps::titled("", Text::content(item.with(|item| item.label.0.clone())))
            .description(move || {
                item.with(|item| match item.auth.status {
                    SourceControlProviderAuthStatus::Authenticated => Text::from("Authenticated"),
                    SourceControlProviderAuthStatus::Unauthenticated => {
                        Text::from("Not authenticated")
                    }
                    SourceControlProviderAuthStatus::Unknown => Text::from("Status unknown"),
                })
            })
            .control(move || {
                view()
                    .app_class(tw("flex flex-row items-center gap-2"))
                    .child(supported.then(|| {
                        button(
                            ButtonContent::Icon {
                                icon: Icon::ChevronDown,
                                label: expand.clone(),
                            },
                            ButtonVariant::GhostMuted,
                            ButtonSize::IconXs,
                            || false,
                            move || open.update(|open| *open = !*open),
                        )
                    }))
                    .child(switch(
                        offered.clone(),
                        ControlState {
                            disabled: Prop::bind(move || {
                                !environment.with(|id| page.environment_writable(id))
                            }),
                            checked: Prop::bind(move || {
                                item.with(|item| {
                                    page.context
                                        .view()
                                        .and_then(|view| {
                                            environment.with(|id| {
                                                view.all_environments
                                                    .iter()
                                                    .find(|entry| &entry.environment_id == id)?
                                                    .config
                                                    .as_ref()?
                                                    .settings
                                                    .offered_source_control_providers
                                                    .get(&item.kind.0)
                                                    .copied()
                                            })
                                        })
                                        .unwrap_or(true)
                                })
                            }),
                            ..Default::default()
                        },
                        Size::Default,
                        move |checked| {
                            if let Some(services) = page.services.get() {
                                let provider = item.with(|item| item.kind.clone());
                                let save =
                                    services.0.set_offered(environment.get(), provider, checked);
                                page.tasks.spawn(async move {
                                    let _ = save.await;
                                });
                            }
                        },
                    ))
            })
            .children(move || {
                region(
                    move || open.get(),
                    move |open| {
                        open.then(|| {
                            credentials::host_settings(page, environment.get(), item.get())
                        })
                    },
                )
            }),
    )
}

fn option_value<T>(option: &WireOption<T>) -> Option<&T> {
    match option {
        WireOption::None => None,
        WireOption::Some { value } => Some(value),
    }
}
