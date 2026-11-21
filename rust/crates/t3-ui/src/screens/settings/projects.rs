//! Port of ProjectsSettings.tsx and ProjectSettingsPanel.tsx.
mod actions;
mod icons;

use std::rc::Rc;

use futures::{future::LocalBoxFuture, stream::LocalBoxStream};
use nsh_core::{KeyEvent, element::Composition, prelude::*};
use serde_json::{Value, json};
use t3_client::{
    ClientError,
    composition::ClientRuntime,
    platform::Platform,
    services::{
        assets::{AssetUrlState, ProjectFaviconKey, project_favicon_resource},
        settings::{
            scope::{ALL_VALUE, ScopeMember, SettingsScopeSearch, select_environment_axis},
            service::{
                SettingsScopeView, SettingsUpdateOutcome,
                project_actions::{ProjectActionEdit, ProjectActionImports},
            },
        },
    },
    thread_views::thread_ops::projects::settings::{
        ProjectSettingsDetail, member_key, regrouped_selection,
    },
    workspace::{ProjectKey, project_ops::entities::ProjectRemovalPlan},
};
use t3_contracts::{project::ProjectEntry, settings::SidebarProjectGroupingMode};

use super::shell::{
    RowReset, SettingsContext, SettingsRowProps, region, settings_group, settings_page_container,
    settings_row, settings_section, use_settings,
};
use crate::{
    i18n::{Text, t, tr},
    runtime::tasks::TaskOwner,
    theme::tw,
    ui::{
        alert::{self, Slots},
        button::{ButtonContent, ButtonSize, ButtonVariant, button},
        confirm::{
            ConfirmDialogInitialFocus, ConfirmDialogOptions, ConfirmDialogVariant,
            request_confirm_dialog,
        },
        icon::{Icon, icon_with_class},
        select::{Popup, SelectItem, SelectProps, Size, Value as SelectValue, select},
        switch::{ControlState, Size as SwitchSize, switch},
        toast::{ToastManager, ToastOptions, Variant, toast_manager},
    },
};

type Write = LocalBoxFuture<'static, Result<(), ClientError>>;
type ReadDetail = Rc<dyn Fn(&SettingsScopeView) -> Option<ProjectSettingsDetail>>;
type Update = Rc<dyn Fn(Vec<ProjectKey>, Value) -> Write>;
type Rename = Rc<dyn Fn(Vec<ScopeMember>, String) -> Write>;
type Favicon = Rc<dyn Fn(ProjectFaviconKey) -> LocalBoxStream<'static, AssetUrlState>>;
type ImageSearch = Rc<
    dyn Fn(ScopeMember, String) -> LocalBoxFuture<'static, Result<Vec<ProjectEntry>, ClientError>>,
>;
type EditAction = Rc<
    dyn Fn(
        SettingsScopeSearch,
        ProjectActionEdit,
    ) -> LocalBoxFuture<'static, Result<SettingsUpdateOutcome, ClientError>>,
>;
type PlanRemoval = Rc<dyn Fn(&ProjectSettingsDetail, &[ScopeMember]) -> Option<ProjectRemovalPlan>>;
type Remove = Rc<dyn Fn(Vec<ScopeMember>, Option<ScopeMember>) -> Write>;
type ActionImports = Rc<dyn Fn(ScopeMember) -> LocalBoxStream<'static, ProjectActionImports>>;
type Grouping = Rc<dyn Fn(ScopeMember, Option<SidebarProjectGroupingMode>) -> Write>;

#[derive(Clone)]
pub(super) struct Services {
    detail: ReadDetail,
    update: Update,
    rename: Rename,
    grouping: Grouping,
    edit_action: EditAction,
    action_imports: ActionImports,
    removal_plan: PlanRemoval,
    remove: Remove,
    favicon: Favicon,
    search_images: ImageSearch,
}

impl Services {
    pub(super) fn new<P: Platform + Clone + 'static>(client: &ClientRuntime<P>) -> Self {
        let projects = client.project_actions();
        let assets = client.asset_urls();
        let settings = client.settings();
        Self {
            detail: {
                let projects = projects.clone();
                Rc::new(move |scope| projects.project_settings_detail(scope))
            },
            update: {
                let projects = projects.clone();
                Rc::new(move |members, patch| projects.update_project_group(members, patch))
            },
            rename: {
                let projects = projects.clone();
                Rc::new(move |members, title| projects.rename_settings_project(members, title))
            },
            favicon: Rc::new(move |key| {
                assets.watch(
                    key.environment,
                    project_favicon_resource(&key.cwd, key.path.as_deref()),
                )
            }),
            search_images: {
                let projects = projects.clone();
                Rc::new(move |member, query| projects.search_project_images(member, query))
            },
            action_imports: {
                let settings = settings.clone();
                Rc::new(move |member| settings.watch_project_action_imports(member))
            },
            edit_action: Rc::new(move |search, edit| settings.edit_project_actions(search, edit)),
            removal_plan: {
                let projects = projects.clone();
                Rc::new(move |detail, members| {
                    projects.plan_settings_project_removal(detail, members)
                })
            },
            remove: {
                let projects = projects.clone();
                Rc::new(move |members, remaining| {
                    projects.remove_settings_project(members, remaining)
                })
            },
            grouping: Rc::new(move |member, mode| projects.set_checkout_grouping(member, mode)),
        }
    }
}

#[derive(Clone, Copy)]
struct ProjectSettings {
    context: SettingsContext,
    services: Stored<Option<Services>>,
    tasks: TaskOwner,
    toasts: Stored<ToastManager>,
}

impl ProjectSettings {
    fn write(self, title: &'static str, command: impl FnOnce(Services) -> Write) {
        let Some(services) = self.services.get() else {
            return;
        };
        let toasts = self.toasts.get();
        let task = command(services);
        self.tasks.spawn(async move {
            if let Err(error) = task.await {
                toasts.add(ToastOptions {
                    variant: Variant::Error,
                    title: Text::from(title),
                    description: Some(Text::content(error.to_string())),
                    ..Default::default()
                });
            }
        });
    }
}

pub fn screen() -> AnyElement {
    let settings = ProjectSettings {
        context: use_settings(),
        services: stored(use_context::<Services>()),
        tasks: TaskOwner::here(),
        toasts: stored(toast_manager()),
    };
    let previous = signal(None::<(SettingsScopeSearch, Vec<String>)>);
    effect(move || {
        let Some(view) = settings.context.view() else {
            return;
        };
        if view.scope.is_project_scope() {
            previous.set(Some((
                view.search.clone(),
                view.scope
                    .members
                    .iter()
                    .map(|member| member.physical_project_key.clone())
                    .collect(),
            )));
        } else if let Some((search, members)) = previous.get_untracked()
            && let Some(search) = regrouped_selection(&view, &search, &members)
        {
            settings.context.select_scope(search);
        }
    });

    settings_page_container(region(
        move || {
            let view = settings.context.view();
            let detail = view.as_ref().and_then(|view| {
                settings
                    .services
                    .with(|services| (services.as_ref()?.detail)(view))
            });
            let unavailable = view.as_ref().and_then(|view| {
                view.scope
                    .unavailable_message()
                    .map(|message| (message, view.search.clone()))
            });
            let groups = view.map(|view| view.groups.clone()).unwrap_or_default();
            (detail, unavailable, groups)
        },
        move |(detail, unavailable, groups)| {
            if let Some(detail) = detail {
                project_detail(settings, detail).into_element()
            } else if let Some((message, search)) = unavailable {
                let show_all = search.machine.is_some() || search.checkout.is_some();
                view()
                    .app_class(tw("flex flex-col items-start gap-3"))
                    .child(
                        text(Text::client_key(message).resolve())
                            .app_class(tw("text-sm text-muted-foreground")),
                    )
                    .child(show_all.then(|| {
                        button(
                            ButtonContent::IconLabel {
                                icon: Icon::Layers,
                                label: t("Show all environments"),
                            },
                            ButtonVariant::Outline,
                            ButtonSize::Xs,
                            || false,
                            move || {
                                settings
                                    .context
                                    .select_scope(select_environment_axis(&search, ALL_VALUE))
                            },
                        )
                    }))
                    .into_element()
            } else {
                project_choices(settings, groups).into_element()
            }
        },
    ))
    .into_element()
}

fn project_choices(
    settings: ProjectSettings,
    groups: Vec<t3_client::services::settings::scope::ScopeGroup>,
) -> impl IntoElement {
    let mut choices = view();
    for (index, group) in groups.into_iter().enumerate() {
        let search = SettingsScopeSearch {
            project: Some(group.project_key),
            ..Default::default()
        };
        choices = choices.child(nsh_base::button()
            .accessibility_label(Some(SharedString::from(group.display_name.clone())))
            .on_press(move || settings.context.select_scope(search.clone()))
            .app_class(tw(if index == 0 {
                "flex w-full min-w-0 flex-row items-center gap-3 px-3 py-3 text-left outline-none hover:bg-accent/50 focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring sm:px-4"
            } else {
                "flex w-full min-w-0 flex-row items-center gap-3 border-t border-border/50 px-3 py-3 text-left outline-none hover:bg-accent/50 focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring sm:px-4"
            }))
            .child(icon_with_class(Icon::FolderCode, "size-4 shrink-0"))
            .child(view().app_class(tw("min-w-0 flex-1"))
                .child(text(group.display_name).number_of_lines(1u32).app_class(tw("block truncate text-sm font-medium")))
                .child(group.members.first().map(|member| {
                    text(member.workspace_root.clone()).number_of_lines(1u32)
                        .app_class(tw("block truncate text-xs text-muted-foreground"))
                })))
            .child(icon_with_class(Icon::ChevronRight, "size-4 shrink-0 text-muted-foreground")));
    }
    view()
        .app_class(tw("flex flex-col gap-3"))
        .child(
            text(t(
                "Choose a project to manage its name, icon, checkouts and actions.",
            ))
            .app_class(tw("px-3 text-sm text-muted-foreground sm:px-4")),
        )
        .child(settings_group(choices))
}

fn project_detail(settings: ProjectSettings, detail: ProjectSettingsDetail) -> impl IntoElement {
    let detail = stored(detail);
    let can_edit = detail.with(|detail| detail.can_edit);
    let can_organize = detail.with(|detail| detail.can_organize);
    view()
        .app_class(tw("flex flex-col gap-6"))
        .child(alert::alert(alert::Props { variant: alert::Variant::Info, ..Default::default() }, |context| {
            Slots {
                icons: vec![alert::alert_icon(context, Icon::Info).into_element()],
                content: vec![alert::alert_description(context, text(t(
                    "Can't find a setting? Keep this project picked above and hop to any other settings page.",
                ))).into_element()],
                ..Default::default()
            }
        }))
        .child(settings_section(
            "project-overview",
            Text::from("Project"),
            view()
                .child((!can_edit).then(|| {
                    text(if detail.with(|detail| detail.members.len() > 1) {
                        t("Shared settings require permission to change every checkout in this group.")
                    } else {
                        t("This connection cannot change this project.")
                    }).app_class(tw("px-3 py-2 text-sm text-muted-foreground sm:px-4"))
                }))
                .child(project_name(settings, detail, can_edit))
                .child(icons::project_icon_row(settings, detail))
                .child(can_organize.then(|| project_organization(settings, detail))),
        ))
        .child(super::threads::project_defaults_settings(settings.context))
        .child(actions::project_actions_settings(settings))
        .child(checkout_choices(settings, detail))
        .child(project_danger(settings, detail))
}

fn project_name(
    settings: ProjectSettings,
    detail: Stored<ProjectSettingsDetail>,
    enabled: bool,
) -> impl IntoElement {
    let mut row = SettingsRowProps::titled("", Text::from("Name"))
        .first()
        .description(|| Text::from("The shared name for this project group in the sidebar and thread lists."))
        .control(move || {
            let title = signal(detail.with(|detail| detail.group.display_name.clone()));
            let edited = signal(false);
            let input = element_ref();
            let composing = signal(false);
            let save = move || {
                if !enabled || !edited.get_untracked() {
                    return;
                }
                edited.set(false);
                let title = title.get_untracked();
                if t3_contracts::wire::trim_js(&title).is_empty() {
                    settings.toasts.get().add(ToastOptions {
                        variant: Variant::Warning,
                        title: Text::from("Project title cannot be empty"),
                        ..Default::default()
                    });
                    return;
                }
                let members = detail.with(|detail| detail.members.clone());
                settings.write("Failed to rename project", |services| (services.rename)(members, title));
            };
            input.on_focus_out(save);
            nsh_base::input()
                .element_ref(&input)
                .label(t("Project name"))
                .disabled(!enabled)
                .value(move || title.get())
                .on_change(move |next: &str| {
                    edited.set(true);
                    title.set(next.to_owned());
                })
                .submit_behavior(SubmitBehavior::Submit)
                .on_composition_change(move |event: &Composition| composing.set(event.composing))
                .on_key_down(move |event: &KeyEvent| {
                    if event.key.as_ref() == "Enter" && !composing.get_untracked() {
                        save();
                        input.blur();
                    }
                })
                .on_submit(move |_: &str| {
                    if !composing.get_untracked() {
                        save();
                        input.blur();
                    }
                })
                .app_class(tw("h-8 w-full rounded-lg border border-input bg-background px-[11px] text-sm sm:w-64"))
        });
    let reset = detail.with(|detail| {
        detail.projects.iter().all(|project| project.is_scratch)
            && detail
                .projects
                .iter()
                .any(|project| project.project.title.0 != "No project")
    });
    if reset && enabled {
        row = row.reset(RowReset::new(
            "project name",
            || true,
            move || {
                let members = detail.with(|detail| detail.members.iter().map(member_key).collect());
                settings.write("Failed to rename project", |services| {
                    (services.update)(members, json!({"title":"No project"}))
                });
            },
        ));
    }
    settings_row(row)
}

fn project_organization(
    settings: ProjectSettings,
    detail: Stored<ProjectSettingsDetail>,
) -> impl IntoElement {
    let (pinned, archived) = detail.with(|detail| (detail.pinned, detail.archived));
    let pending = signal(false);
    let update_organization = move |failure, patch| {
        if pending.get_untracked() {
            return;
        }
        let members = detail.with(|detail| detail.group.members.iter().map(member_key).collect());
        pending.set(true);
        settings.write(failure, |services| {
            let save = (services.update)(members, patch);
            Box::pin(async move {
                let result = save.await;
                let _ = pending.try_set(false);
                result
            })
        });
    };
    view()
        .child(settings_row(
            SettingsRowProps::titled("", Text::from("Pin"))
                .description(|| Text::from("Pinned projects come first in project pickers."))
                .control(move || {
                    switch(
                        "Pin project",
                        ControlState {
                            checked: pinned.into(),
                            disabled: Prop::bind(move || pending.get()),
                            ..Default::default()
                        },
                        SwitchSize::Default,
                        move |checked| {
                            update_organization(
                                if checked {
                                    "Failed to pin project"
                                } else {
                                    "Failed to unpin project"
                                },
                                json!({"pinned":checked}),
                            );
                        },
                    )
                }),
        ))
        .child(settings_row(
            SettingsRowProps::titled("", Text::from("Archive"))
                .description(move || {
                    Text::from(if archived {
                        "Hidden from project pickers. Threads and files are kept."
                    } else {
                        "Hides the project from project pickers. Threads and files are kept."
                    })
                })
                .control(move || {
                    button(
                        ButtonContent::Label(if archived {
                            t("Unarchive")
                        } else {
                            t("Archive")
                        }),
                        ButtonVariant::Outline,
                        ButtonSize::Sm,
                        move || pending.get(),
                        move || {
                            update_organization(
                                if archived {
                                    "Failed to unarchive project"
                                } else {
                                    "Failed to archive project"
                                },
                                json!({"archived":!archived}),
                            );
                        },
                    )
                }),
        ))
}

fn checkout_choices(
    settings: ProjectSettings,
    detail: Stored<ProjectSettingsDetail>,
) -> impl IntoElement {
    let mut rows = view().child(text(t(
        "Grouping rule decides how each checkout joins project groups in the sidebar. Changing it can move a checkout to a different project.",
    )).app_class(tw("px-3 py-2.5 text-xs text-muted-foreground sm:px-4")));
    let multiple = detail.with(|detail| detail.members.len() > 1);
    for member in detail.with(|detail| detail.members.clone()) {
        let title = member
            .environment_label
            .clone()
            .map(Text::content)
            .unwrap_or_else(|| Text::from("Environment"));
        let path = member.workspace_root.clone();
        let editable = detail.with(|detail| detail.editable.contains(&member_key(&member)));
        rows = rows.child(settings_row(
            SettingsRowProps::titled("", title)
                .description(move || Text::content(path.clone()))
                .control(move || {
                    let label = tr(
                        "Remove checkout {path}",
                        &[("path", t3_i18n::Value::Text(member.workspace_root.clone()))],
                    );
                    let member_to_remove = member.clone();
                    view()
                        .app_class(tw(
                            "flex w-full min-w-0 flex-row items-center gap-2 sm:w-auto",
                        ))
                        .child(checkout_grouping_select(settings, member.clone()))
                        .child(multiple.then(|| {
                            button(
                                ButtonContent::Custom {
                                    label,
                                    child: text(t("Remove")).into_element(),
                                },
                                ButtonVariant::Outline,
                                ButtonSize::Sm,
                                move || !editable,
                                move || {
                                    remove_members(
                                        settings,
                                        detail.get(),
                                        vec![member_to_remove.clone()],
                                    )
                                },
                            )
                        }))
                }),
        ));
    }
    settings_section("", Text::from("Checkouts"), rows)
}

fn grouping_label(mode: &SidebarProjectGroupingMode) -> &'static str {
    match mode {
        SidebarProjectGroupingMode::Repository => "Group by repository",
        SidebarProjectGroupingMode::RepositoryPath => "Group by repository path",
        SidebarProjectGroupingMode::Separate => "Keep separate",
    }
}

fn checkout_grouping_select(settings: ProjectSettings, member: ScopeMember) -> impl IntoElement {
    let machine = match &member.environment_label {
        Some(label) => format!("{label} ({})", member.environment_id.0),
        None => member.environment_id.0.clone(),
    };
    let label = Text::content(tr(
        "Grouping rule for {path} on {environment}",
        &[
            ("path", t3_i18n::Value::Text(member.workspace_root.clone())),
            ("environment", t3_i18n::Value::Text(machine)),
        ],
    ));
    let key =
        t3_contracts::base_schemas::TrimmedNonEmptyString(member.physical_project_key.clone());
    let mut props = SelectProps::new(label);
    props.size = Size::Sm;
    props.popup = Popup {
        align: Align::End,
        ..Default::default()
    };
    props.items = vec![
        SelectItem::new("inherit", t("Use global default")),
        SelectItem::new("repository", t("Group by repository")),
        SelectItem::new("repository_path", t("Group by repository path")),
        SelectItem::new("separate", t("Keep separate")),
    ]
    .into();
    props.value = Prop::bind(move || {
        let client = settings.context.client_settings();
        let value = client
            .sidebar_project_grouping_overrides
            .get(&key)
            .map(|mode| {
                match mode {
                    SidebarProjectGroupingMode::Repository => "repository",
                    SidebarProjectGroupingMode::RepositoryPath => "repository_path",
                    SidebarProjectGroupingMode::Separate => "separate",
                }
                .into()
            });
        SelectValue::Controlled(value)
    });
    props.placeholder = Prop::bind(move || {
        Text::content(tr(
            "Default ({mode})",
            &[(
                "mode",
                t3_i18n::Value::Text(
                    Text::client_key(grouping_label(
                        &settings
                            .context
                            .client_settings()
                            .sidebar_project_grouping_mode,
                    ))
                    .resolve()
                    .to_string(),
                ),
            )],
        ))
    });
    view()
        .app_class(tw("w-full sm:w-72"))
        .child(select(props, move |next| {
            let mode = match next.as_deref() {
                Some("repository") => Some(SidebarProjectGroupingMode::Repository),
                Some("repository_path") => Some(SidebarProjectGroupingMode::RepositoryPath),
                Some("separate") => Some(SidebarProjectGroupingMode::Separate),
                Some("inherit") => None,
                _ => return,
            };
            settings.write("Could not save grouping rule", |services| {
                (services.grouping)(member.clone(), mode)
            });
        }))
}

fn project_danger(
    settings: ProjectSettings,
    detail: Stored<ProjectSettingsDetail>,
) -> impl IntoElement {
    let (disabled, partial, count) =
        detail.with(|detail| (!detail.can_edit, detail.is_partial(), detail.members.len()));
    let (title, label, description) = if partial {
        (
            Text::from("Remove checkout"),
            t("Remove checkout"),
            Text::from(
                "Deletes the selected machine's checkout entries and their threads. Other machines and files on disk are not touched.",
            ),
        )
    } else if count > 1 {
        (
            Text::from("Remove this project everywhere"),
            t("Remove all entries"),
            Text::content(tr(
                "Deletes all {count} checkout entries and their threads on every machine. Files on disk are not touched.",
                &[("count", t3_i18n::Value::Int(count as i64))],
            )),
        )
    } else {
        (
            Text::from("Remove project"),
            t("Remove project"),
            Text::from("Deletes the project entry and its threads. Files on disk are not touched."),
        )
    };
    settings_section(
        "",
        Text::from("Danger"),
        settings_row(
            SettingsRowProps::titled("", title)
                .first()
                .description(move || description.clone())
                .control(move || {
                    button(
                        ButtonContent::IconLabel {
                            icon: Icon::Trash2,
                            label: label.clone(),
                        },
                        ButtonVariant::DestructiveOutline,
                        ButtonSize::Sm,
                        move || disabled,
                        move || {
                            remove_members(
                                settings,
                                detail.get(),
                                detail.with(|detail| detail.members.clone()),
                            )
                        },
                    )
                }),
        ),
    )
}

fn remove_members(
    settings: ProjectSettings,
    detail: ProjectSettingsDetail,
    members: Vec<ScopeMember>,
) {
    let Some(services) = settings.services.get() else {
        return;
    };
    let Some(plan) = (services.removal_plan)(&detail, &members) else {
        return;
    };
    let Some(search) = settings.context.view().map(|view| view.search.clone()) else {
        return;
    };
    let whole = plan.is_whole_group && !detail.is_partial();
    let remaining = detail.group.members.into_iter().find(|member| {
        !members
            .iter()
            .any(|selected| member_key(selected) == member_key(member))
    });
    let Some(confirmed) = request_confirm_dialog(
        plan.confirmation_message,
        Some(ConfirmDialogOptions {
            variant: ConfirmDialogVariant::Destructive,
            initial_focus: Some(ConfirmDialogInitialFocus::Cancel),
        }),
    ) else {
        return;
    };
    let toasts = settings.toasts.get();
    settings.tasks.spawn(async move {
        if !confirmed.await {
            return;
        }
        match (services.remove)(members, remaining).await {
            Ok(())
                if whole
                    && settings
                        .context
                        .view()
                        .is_some_and(|view| view.search == search) =>
            {
                settings.context.select_scope(SettingsScopeSearch {
                    machine: search.machine,
                    ..Default::default()
                });
            }
            Ok(()) => {}
            Err(error) => {
                toasts.add(ToastOptions {
                    variant: Variant::Error,
                    title: Text::from("Failed to remove project"),
                    description: Some(Text::content(error.to_string())),
                    ..Default::default()
                });
            }
        }
    });
}
