//! Port of ProjectIconPickerDialog.tsx, ProjectFaviconPickerDialog.tsx and the Project icon row.
use futures::StreamExt;
use nsh_core::prelude::*;
use serde_json::{Value, json};
use std::rc::Rc;
use t3_client::{
    services::assets::AssetUrlState,
    thread_views::thread_ops::projects::settings::{
        ProjectSettingsDetail, member_key, normalize_monogram,
    },
    workspace::project_ops::icons::{
        PROJECT_EMOJIS, PROJECT_ICON_COLORS, derive_project_identity, filter_project_icon_names,
        first_emoji,
    },
};
use t3_contracts::project::{ProjectIconColor, ProjectIconOverride};

use super::super::shell::{RowReset, SettingsRowProps, region, settings_row};
use super::ProjectSettings;
use crate::{
    i18n::{Text, t},
    runtime::tasks::watch,
    theme::{cn, tw},
    ui::{
        avatar::{ProjectFaviconProject, lucide_project_icon, project_favicon, project_monogram},
        button::{ButtonContent, ButtonSize, ButtonVariant, button},
        dialog::{
            MaxWidth, PopupOptions, dialog, dialog_close, dialog_description, dialog_footer,
            dialog_header, dialog_panel, dialog_popup, dialog_title,
        },
        input::{InputSize, text_field},
        toggle::{GroupItem, GroupOptions, toggle_group},
    },
};

const SCRATCH_ICON: &str = "message-square-dashed";

fn scratch_icon() -> Value {
    json!({"kind":"lucide","name":SCRATCH_ICON,"color":"gray"})
}

pub(super) fn project_icon_row(
    settings: ProjectSettings,
    detail: Stored<ProjectSettingsDetail>,
) -> impl IntoElement {
    let open_icon = signal(false);
    let open_file = signal(false);
    let (project, enabled, has_custom_icon) = detail.with(|detail| {
        let representative = &detail.representative;
        let current = ProjectFaviconProject::from_shell(
            representative.environment.clone(),
            &representative.project,
        );
        let custom = detail.projects.iter().any(|project| {
            let icon =
                ProjectFaviconProject::from_shell(project.environment.clone(), &project.project);
            icon.favicon_path.is_some()
                || if project.is_scratch {
                    icon.project_icon
                        .as_ref()
                        .is_none_or(|value| json!(value) != scratch_icon())
                } else {
                    icon.project_icon.is_some()
                }
        });
        (current, detail.can_edit, custom)
    });
    let project = stored(project);
    let save = move |favicon: Option<String>, icon: Value| {
        let keys = detail.with(|detail| detail.members.iter().map(member_key).collect());
        settings.write("Failed to update project icon", |services| {
            (services.update)(keys, json!({"faviconPath":favicon,"projectIcon":icon}))
        });
    };
    let mut row = SettingsRowProps::titled("", Text::from("Project icon"))
        .description(move || {
            project.with(|project| {
                let color =
                    |color: &ProjectIconColor| json!(color).as_str().unwrap_or_default().to_owned();
                match &project.project_icon {
                    Some(ProjectIconOverride::Lucide(icon)) => {
                        Text::content(format!("{} · {}", icon.name.0, color(&icon.color)))
                    }
                    Some(ProjectIconOverride::Monogram(icon)) => {
                        Text::content(format!("{} · {}", icon.text.0, color(&icon.color)))
                    }
                    Some(ProjectIconOverride::Emoji(icon)) => Text::content(icon.emoji.0.clone()),
                    None => project
                        .favicon_path
                        .clone()
                        .map(Text::content)
                        .unwrap_or_else(|| Text::from("Automatic")),
                }
            })
        })
        .control(move || {
            view()
                .app_class(tw("flex flex-row items-center gap-2"))
                .child(favicon(settings, project.get(), "size-6"))
                .child(
                    nsh_base::button()
                        .accessibility_label(Some(SharedString::from(t("Choose a project icon"))))
                        .disabled(!enabled)
                        .on_press(move || open_icon.set(true))
                        .app_class(tw(
                            "h-8 rounded-md border border-border px-2.5 text-sm hover:bg-accent",
                        ))
                        .child(text(t("Choose icon"))),
                )
                .child(
                    nsh_base::button()
                        .accessibility_label(Some(SharedString::from(t(
                            "Choose a project icon file",
                        ))))
                        .disabled(!enabled)
                        .on_press(move || open_file.set(true))
                        .app_class(tw(
                            "h-8 rounded-md border border-border px-2.5 text-sm hover:bg-accent",
                        ))
                        .child(text(t("Choose file"))),
                )
        });
    if has_custom_icon && enabled {
        row = row.reset(RowReset::new(
            "project icon",
            || true,
            move || {
                let (scratch, ordinary): (Vec<_>, Vec<_>) = detail.with(|detail| {
                    detail
                        .projects
                        .iter()
                        .map(|project| (project.is_scratch, project.key()))
                        .partition(|(scratch, _)| *scratch)
                });
                settings.write("Failed to update project icon", |services| {
                    use futures::FutureExt;
                    let update = services.update;
                    async move {
                        for (keys, icon) in [(scratch, scratch_icon()), (ordinary, Value::Null)] {
                            if !keys.is_empty() {
                                update(
                                    keys.into_iter().map(|(_, key)| key).collect(),
                                    json!({"faviconPath":null,"projectIcon":icon}),
                                )
                                .await?;
                            }
                        }
                        Ok(())
                    }
                    .boxed_local()
                });
            },
        ));
    }
    let file_target = detail.with(|detail| {
        detail
            .members
            .iter()
            .find(|member| {
                settings.context.view().is_some_and(|view| {
                    view.all_environments.iter().any(|environment| {
                        environment.environment_id == member.environment_id
                            && environment.config.is_some()
                    })
                })
            })
            .or_else(|| detail.members.first())
            .cloned()
    });
    view()
        .child(settings_row(row))
        .child(dialog(open_icon, move |state| {
            dialog_popup(
                state,
                PopupOptions {
                    show_close_button: false,
                    ..Default::default()
                },
                move || {
                    project_icon_picker_body(state, project.get(), move |icon| save(None, icon))
                },
            )
        }))
        .child(file_target.map(|member| {
            dialog(open_file, move |state| {
                dialog_popup(
                    state,
                    PopupOptions {
                        max_width: MaxWidth::Xl,
                        ..Default::default()
                    },
                    move || {
                        project_favicon_picker(settings, state, member.clone(), move |path| {
                            save(Some(path), Value::Null)
                        })
                    },
                )
            })
        }))
}

pub(super) fn favicon(
    settings: ProjectSettings,
    project: ProjectFaviconProject,
    class_name: &str,
) -> impl IntoElement {
    let url = signal(None::<String>);
    if project.project_icon.is_none()
        && let Some(services) = settings.services.get()
    {
        let mut stream = (services.favicon)(project.resource_key());
        watch(async move {
            while let Some(next) = stream.next().await {
                let _ = url.try_set(match next {
                    AssetUrlState::Success { url, .. } => Some(url),
                    _ => None,
                });
            }
        });
    }
    project_favicon(
        move || project.clone(),
        move |_| url.get(),
        class_name,
        None,
    )
}

fn project_icon_picker_body(
    state: crate::ui::dialog::DialogState,
    project: ProjectFaviconProject,
    on_select: impl Fn(Value) + 'static,
) -> impl IntoElement {
    let automatic = derive_project_identity(&project.title);
    let mut initial_mode = "lucide";
    let mut initial_name = "folder-code".to_owned();
    let mut initial_color = automatic.color;
    let mut initial_letters = automatic.monogram;
    let mut initial_emoji = "💻".to_owned();
    match project.project_icon {
        Some(ProjectIconOverride::Lucide(icon)) => {
            initial_name = icon.name.0;
            initial_color = icon.color;
        }
        Some(ProjectIconOverride::Monogram(icon)) => {
            initial_mode = "monogram";
            initial_letters = icon.text.0;
            initial_color = icon.color;
        }
        Some(ProjectIconOverride::Emoji(icon)) => {
            initial_mode = "emoji";
            initial_emoji = icon.emoji.0;
        }
        None => {}
    }
    let mode = signal(initial_mode.to_owned());
    let name = signal(initial_name);
    let color = signal(initial_color);
    let letters = signal(initial_letters);
    let emoji = signal(initial_emoji);
    let query = signal(String::new());
    let custom_emoji = signal(String::new());
    let draft_icon = move || match mode.get().as_str() {
        "monogram" => normalize_monogram(&letters.get())
            .map(|letters| json!({"kind":"monogram","text":letters,"color":color.get()})),
        "emoji" => Some(json!({"kind":"emoji","emoji":emoji.get()})),
        _ => Some(json!({"kind":"lucide","name":name.get(),"color":color.get()})),
    };
    let modes = vec![
        GroupItem::new("lucide", ButtonContent::Label(t("Icons"))).into(),
        GroupItem::new("emoji", ButtonContent::Label(t("Emoji"))).into(),
        GroupItem::new("monogram", ButtonContent::Label(t("Monogram"))).into(),
    ];
    let body = view().app_class(tw("flex min-h-0 flex-col gap-3"))
        .child(toggle_group(t("Icon type"), modes, GroupOptions::default(),
            move || vec![mode.get()], || false, move |mut next| {
                if let Some(next) = next.pop() { mode.set(next); }
            }))
        .child(when(move || mode.get() != "emoji", move || {
            view().child(text(t("Color")).app_class(tw("mb-2 text-xs font-medium text-muted-foreground")))
                .child(project_icon_colors(color))
        }))
        .child(region(move || mode.get(), move |mode| match mode.as_str() {
            "monogram" => view().app_class(tw("flex flex-row items-center gap-4 py-2"))
                .child(region(move || (normalize_monogram(&letters.get()), color.get()), |(letters, color)| {
                    project_monogram(letters.as_deref().unwrap_or(""), color, "size-12")
                }))
                .child(view().app_class(tw("flex flex-1 flex-col gap-2"))
                    .child(text(t("Letters")).app_class(tw("text-sm font-medium")))
                    .child(text_field(letters, String::new(), t("Letters"), InputSize::Default, || {}))
                    .child(text(t("One or two letters or numbers.")).app_class(tw("text-xs text-muted-foreground"))))
                .into_element(),
            "emoji" => {
                let mut choices = view().app_class(tw("flex flex-row flex-wrap gap-1 p-0.5"));
                for &(glyph, label) in PROJECT_EMOJIS {
                    choices = choices.child(icon_choice(Text::client_key(label).resolve().to_string(),
                        move || emoji.with(|value| value == glyph), move || emoji.set(glyph.to_owned()), text(glyph)));
                }
                view().app_class(tw("flex flex-col gap-3"))
                    .child(scroll().app_class(tw("max-h-64")).child(choices))
                    .child(text(t("Or paste any emoji")).app_class(tw("mb-2 text-xs font-medium text-muted-foreground")))
                    .child(nsh_base::input()
                        .label(t("Custom emoji")).placeholder(t("Paste an emoji"))
                        .value(move || custom_emoji.get())
                        .on_change(move |value: &str| {
                            custom_emoji.set(value.to_owned());
                            if let Some(value) = first_emoji(value) { emoji.set(value.to_owned()); }
                        })
                        .app_class(tw("h-8 w-full rounded-lg border border-input bg-background px-[11px] text-sm")))
                    .into_element()
            }
            _ => view().app_class(tw("flex flex-col gap-3"))
                .child(text_field(query, t("Search all Lucide icons"), t("Search Lucide icons"), InputSize::Default, || {}))
                .child(region(move || filter_project_icon_names(&query.get()), move |icons| {
                    if icons.is_empty() {
                        return text(t("No icons found.")).app_class(tw("py-8 text-center text-sm text-muted-foreground")).into_element();
                    }
                    let mut choices = view().app_class(tw("flex flex-row flex-wrap gap-1 p-0.5"));
                    for icon in icons {
                        let label = icon.split('-').map(|word| {
                            let mut chars = word.chars();
                            chars.next().into_iter().flat_map(char::to_uppercase).chain(chars).collect::<String>()
                        }).collect::<Vec<_>>().join(" ");
                        choices = choices.child(icon_choice(label,
                            move || name.with(|value| value == icon), move || name.set(icon.to_owned()),
                            region(move || color.get(), move |color| lucide_project_icon(icon, &color, "size-5"))));
                    }
                    scroll().app_class(tw("max-h-64")).child(choices).into_element()
                })).into_element(),
        }));
    view()
        .child(dialog_header(
            state,
            view()
                .child(dialog_title(state, || Text::from("Choose project icon")))
                .child(dialog_description(state, || {
                    Text::from("Choose an icon, emoji, or monogram.")
                })),
        ))
        .child(dialog_panel(state, body))
        .child(dialog_footer(
            state,
            crate::ui::dialog::FooterVariant::Bare,
            view()
                .app_class(tw("flex flex-row justify-end gap-2"))
                .child(dialog_close(
                    state,
                    ButtonContent::Label(t("Cancel")),
                    ButtonVariant::Outline,
                    ButtonSize::Default,
                    || false,
                ))
                .child(button(
                    ButtonContent::Label(t("Save icon")),
                    ButtonVariant::Default,
                    ButtonSize::Default,
                    move || draft_icon().is_none(),
                    move || {
                        if let Some(icon) = draft_icon() {
                            on_select(icon);
                            state.close();
                        }
                    },
                )),
        ))
}

fn project_icon_colors(color: Signal<ProjectIconColor>) -> impl IntoElement {
    let mut colors = view().app_class(tw("flex flex-row flex-wrap gap-1.5"));
    for option in PROJECT_ICON_COLORS {
        colors = colors.child(nsh_base::button()
            .accessibility_label(Some(Text::client_key(option.label).resolve()))
            .selected(move || color.get() == option.value)
            .on_press(move || color.set(option.value.clone()))
            .app_class(move || Some(tw(&cn([
                Some("flex size-6 items-center justify-center rounded-full border outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"),
                (color.get() == option.value).then_some("border-foreground/64"),
                (color.get() != option.value).then_some("border-transparent"),
            ]))))
            .child(view().app_class(tw(&cn([Some("size-4 rounded-full"), Some(option.swatch_class_name)])))));
    }
    colors
}

fn icon_choice(
    label: String,
    selected: impl Fn() -> bool + 'static,
    pick: impl Fn() + 'static,
    content: impl IntoElement,
) -> impl IntoElement {
    let selected = Rc::new(selected);
    let pressed = selected.clone();
    nsh_base::button()
        .accessibility_label(Some(SharedString::from(label)))
        .selected(move || pressed())
        .on_press(pick)
        .app_class(move || Some(tw(&cn([
            Some("flex size-8 items-center justify-center rounded-md border text-xl outline-none hover:bg-accent focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"),
            selected().then_some("border-border bg-accent"),
            (!selected()).then_some("border-transparent"),
        ]))))
        .child(content)
}

fn project_favicon_picker(
    settings: ProjectSettings,
    state: crate::ui::dialog::DialogState,
    member: t3_client::services::settings::scope::ScopeMember,
    select_path: impl Fn(String) + 'static,
) -> impl IntoElement {
    let query = signal(String::new());
    let result = signal(None::<Result<Vec<t3_contracts::project::ProjectEntry>, String>>);
    let search = settings
        .services
        .get()
        .map(|services| services.search_images);
    let target = member.clone();
    effect(move || {
        let input = query.get();
        result.set(None);
        if let Some(search) = &search {
            let search = search(target.clone(), input.clone());
            watch(async move {
                let response = search.await.map_err(|error| error.to_string());
                if query.try_get().as_ref() == Some(&input) {
                    let _ = result.try_set(Some(response));
                }
            });
        }
    });
    let select_path = Rc::new(select_path);
    view()
        .child(dialog_header(state, dialog_title(state, || Text::from("Choose project icon"))))
        .child(dialog_panel(state, view().app_class(tw("flex min-h-0 flex-col gap-2"))
            .child(text_field(query, t("Search image files…"), t("Search image files"), InputSize::Default, || {}))
            .child(region(move || (result.get(), query.with(|query| query.trim().is_empty())),
                move |(result, empty_query)| match result {
                    Some(Ok(entries)) if !entries.is_empty() => {
                        let mut rows = view();
                        for entry in entries {
                            let path = entry.path.0;
                            let label = t3_client::services::settings::scope::file_basename(&path).to_owned();
                            let select_path = select_path.clone();
                            let target = ProjectFaviconProject {
                                environment_id: member.environment_id.clone(),
                                workspace_root: member.workspace_root.clone(),
                                title: label.clone(),
                                favicon_path: Some(path.clone()),
                                project_icon: None,
                            };
                            rows = rows.child(nsh_base::button()
                                .accessibility_label(Some(SharedString::from(label.clone())))
                                .app_class(tw("flex w-full flex-row items-center gap-2 rounded-md px-2 py-1.5 text-left hover:bg-accent"))
                                .on_press({
                                    let path = path.clone();
                                    move || { select_path(path.clone()); state.close(); }
                                })
                                .child(favicon(settings, target, "size-5 shrink-0"))
                                .child(view().app_class(tw("flex min-w-0 flex-col"))
                                    .child(text(label).app_class(tw("truncate text-sm")))
                                    .child(text(path).app_class(tw("truncate text-xs text-muted-foreground")))));
                        }
                        scroll().app_class(tw("max-h-80")).child(rows).into_element()
                    }
                    state => {
                        let message = match state {
                            Some(Ok(_)) if empty_query => Text::from("No image files found."),
                            Some(Ok(_)) => Text::from("No matching image files."),
                            Some(Err(error)) => Text::content(error),
                            None if empty_query => Text::from("Indexing project files…"),
                            None => Text::from("Searching project files…"),
                        };
                        text(message.resolve()).app_class(tw("py-8 text-center text-sm text-muted-foreground")).into_element()
                    }
                }))))
}
