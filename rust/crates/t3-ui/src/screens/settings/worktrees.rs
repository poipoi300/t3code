//! Port of WorktreesSettings.tsx, BranchNamingSettings.tsx, ProjectDefaultsSettings.tsx's
//! WorktreeSubmodulesRow, and StorageSettings.tsx's worktree location and cleanup sections.

use nsh_core::prelude::*;
use serde_json::{Value, json};
use t3_client::services::settings::scoped::{ProjectSettingSource, resolve_worktree_cleanup};
use t3_contracts::environment::ExecutionEnvironmentCapabilities;

use super::setting_controls::{choices, reset};
use super::shell::{
    RowReset, SettingsContext, SettingsRowProps, advanced_settings, region,
    settings_page_container, settings_row, settings_section, use_settings,
};
use crate::i18n::{Text, t};
use crate::theme::{palette, tw};
use crate::ui::switch::{ControlState, Size, switch};

const BRANCH_MODES: &[(&str, &str)] = &[
    ("static", "Static prefix"),
    ("semantic", "Semantic prefix"),
    ("custom", "Custom instructions"),
];

pub fn screen() -> AnyElement {
    let context = use_settings();
    settings_page_container(
        view()
            .app_class(tw("flex flex-col gap-8"))
            .child(settings_section(
                "worktree-creation",
                Text::from("Creation"),
                view()
                    .child(region(
                        move || {
                            !project_scope(context) && supports(context, |c| c.worktrees_directory)
                        },
                        move |visible| visible.then(|| worktrees_directory(context)),
                    ))
                    .child(start_from_remote(context))
                    .child(advanced_settings(
                        &["worktree-submodules"],
                        submodules(context),
                    )),
            ))
            .child(settings_section(
                "worktree-branches",
                Text::from("Branches"),
                view().child(branch_mode(context)).child(region(
                    move || selection(context, "branchNamingMode"),
                    move |mode| match mode.as_str() {
                        "static" => Some(branch_prefix(context).into_element()),
                        "custom" => Some(branch_instructions(context).into_element()),
                        _ => None,
                    },
                )),
            ))
            .child(cleanup(context)),
    )
    .into_element()
}

fn start_from_remote(context: SettingsContext) -> impl IntoElement {
    const KEY: &str = "newWorktreesStartFromOrigin";
    let checked = move || context.server_settings().new_worktrees_start_from_origin;
    let mixed = move || context.mixed(&[KEY]);
    let default = t3_contracts::settings::ServerSettings::default().new_worktrees_start_from_origin;
    settings_row(
        SettingsRowProps::searchable("start-from-origin")
            .server(&[KEY])
            .description(|| Text::from("Uses the latest commit of the base branch’s tracked remote branch, even if its name differs. Otherwise uses the same-named branch on origin. Uses your local branch if that remote branch is missing."))
            .reset(RowReset::new(
                "new worktrees start from remote",
                move || checked() != default,
                move || context.update(json!({KEY: default})),
            ))
            .control(move || switch(
                "Start new worktrees from remote by default",
                ControlState {
                    checked: Prop::bind(move || !mixed() && checked()),
                    mixed: Prop::bind(mixed),
                    ..Default::default()
                },
                Size::Default,
                move |checked| context.update(json!({KEY: checked})),
            )),
    )
}

fn project_scope(context: SettingsContext) -> bool {
    context
        .view()
        .is_some_and(|view| view.scope.is_project_scope())
}

fn supports(
    context: SettingsContext,
    cap: fn(&ExecutionEnvironmentCapabilities) -> Option<bool>,
) -> bool {
    context.view().is_none_or(|view| {
        view.selected.connected.iter().all(|environment| {
            environment
                .config
                .as_ref()
                .is_some_and(|config| cap(&config.environment.capabilities) == Some(true))
        })
    })
}

fn selection(context: SettingsContext, key: &'static str) -> String {
    if context.mixed(&[key]) {
        "mixed".into()
    } else {
        context.setting(key).as_str().unwrap_or_default().to_owned()
    }
}

fn submodules(context: SettingsContext) -> impl IntoElement {
    settings_row(
        SettingsRowProps::searchable("worktree-submodules")
            .first()
            .server(&["worktreeSubmodules"])
            .description(move || Text::from(if project_scope(context) {
                "How new worktrees in this project populate git submodules."
            } else {
                "How new worktrees populate git submodules. Projects and their t3.json can override it."
            }))
            .reset(RowReset::new(
                "worktree submodules",
                move || !project_scope(context) && !context.setting("worktreeSubmodules").is_null(),
                move || context.update(json!({"worktreeSubmodules": null})),
            ))
            .control(move || scoped_choice(
                "Worktree submodules",
                &[("recursive", "Recursive"), ("top-level", "Top level only"), ("none", "Skip")],
                move || match selection(context, "worktreeSubmodules").as_str() {
                    "" if context.view().is_some_and(|v| !v.targets.is_empty()) => "recursive".into(),
                    value => value.to_owned(),
                },
                move |next| context.update(json!({"worktreeSubmodules": next})),
            )),
    )
}

fn branch_mode(context: SettingsContext) -> impl IntoElement {
    settings_row(
        SettingsRowProps::searchable("worktree-branch-naming")
            .first()
            .server(&["branchNamingMode"])
            .description(|| Text::from("Choose how new worktree branches are named from your first message."))
            .status(move || (selection(context, "branchNamingMode") == "semantic").then(|| Text::from(
                "The model chooses a prefix that describes the work, such as feat/add-search, fix/login-timeout, or refactor/auth.",
            )))
            .reset(reset(context, "branchNamingMode", "branch naming"))
            .control(move || scoped_choice(
                "Worktree branch naming",
                BRANCH_MODES,
                move || selection(context, "branchNamingMode"),
                move |next| context.update(json!({"branchNamingMode": next})),
            )),
    )
}

fn scoped_choice(
    label: &'static str,
    options: &'static [(&'static str, &'static str)],
    value: impl Fn() -> String + 'static,
    on_change: impl Fn(String) + 'static,
) -> impl IntoElement {
    use crate::ui::select::{Popup, SelectItem, SelectProps, Size, Value, select};
    let mut props = SelectProps::new(label);
    props.items = choices(options)
        .into_iter()
        .map(|(id, title)| SelectItem::new(id, title.resolve()))
        .collect::<Vec<_>>()
        .into();
    let value = computed(value);
    props.value = Prop::bind(move || {
        let value = value.get();
        Value::Controlled(
            options
                .iter()
                .any(|(key, _)| *key == value)
                .then(|| value.into()),
        )
    });
    props.placeholder = Prop::bind(move || {
        Text::from(if value.get().is_empty() {
            "Unavailable"
        } else {
            "Mixed"
        })
    });
    props.size = Size::Sm;
    props.popup = Popup {
        align: Align::End,
        ..Popup::default()
    };
    select(props, move |next| {
        if let Some(next) = next {
            on_change(next.to_string());
        }
    })
}

fn worktrees_directory(context: SettingsContext) -> impl IntoElement {
    settings_row(
        SettingsRowProps::searchable("storage-worktrees-location")
            .first()
            .server(&["worktreesDirectory"])
            .description(|| Text::from(
                r"Folder where new worktrees are created, on any drive, such as D:\worktrees or ~/worktrees. Existing worktrees stay where they are. Leave empty to use the T3 home folder.",
            ))
            .reset(reset(context, "worktreesDirectory", "worktree location"))
            .control(move || scoped_input(context, "worktreesDirectory", "Worktree location", "Default", false)),
    )
}

fn branch_prefix(context: SettingsContext) -> impl IntoElement {
    settings_row(
        SettingsRowProps::titled("", Text::from("Branch prefix"))
            .server(&["branchNamePrefix"])
            .description(|| {
                Text::from(
                    "For example, t3 or t3/ produces t3/add-search. Leave empty for no prefix.",
                )
            })
            .reset(reset(context, "branchNamePrefix", "branch prefix"))
            .control(move || {
                scoped_input(
                    context,
                    "branchNamePrefix",
                    "Branch prefix",
                    "No prefix",
                    false,
                )
            }),
    )
}

fn branch_instructions(context: SettingsContext) -> impl IntoElement {
    settings_row(
        SettingsRowProps::titled("", Text::from("Branch naming instructions"))
            .server(&["branchNameInstructions"])
            .description(|| Text::from("Appended to the naming prompt. The model returns the complete branch name; no prefix or suffix is added."))
            .reset(reset(context, "branchNameInstructions", "branch naming instructions"))
            .children(move || view().app_class(tw("mt-3 max-w-2xl pb-3.5")).child(
                scoped_input(context, "branchNameInstructions", "Branch naming instructions", "Use julius/ followed by the issue ID and a short description.", true)
            )),
    )
}

fn scoped_input(
    context: SettingsContext,
    key: &'static str,
    label: &'static str,
    placeholder: &'static str,
    multiline: bool,
) -> AnyElement {
    region(
        move || {
            let scope = context.view().map(|v| v.search.clone());
            let mixed = context.mixed(&[key]);
            (
                scope,
                mixed,
                context.setting(key).as_str().unwrap_or_default().to_owned(),
            )
        },
        move |(_, mixed, saved)| {
            let draft = signal(if mixed { String::new() } else { saved.clone() });
            let edited = signal(false);
            nsh_base::input()
                .multiline(multiline)
                .value(move || draft.get())
                .on_change(move |value: &str| {
                    edited.set(true);
                    if key == "worktreesDirectory" {
                        draft.update(|previous| {
                            *previous = t3_client::services::settings::panels::resolve_pasted_path(previous, value);
                        });
                    } else {
                        draft.set(value.to_owned());
                    }
                })
                .on_blur(move || {
                    if edited.get_untracked() {
                        edited.set(false);
                        let next = draft.with_untracked(|value| value.trim().to_owned());
                        if mixed || next != saved {
                            context.update(json!({key: next}));
                        }
                    }
                })
                .label(Text::from(label).resolve())
                .placeholder(if mixed {
                    if multiline { t("Mixed. Enter instructions to apply to all selected targets.") } else { t("Mixed") }
                } else {
                    Text::from(placeholder).resolve().to_string()
                })
                .text_color(move || palette().foreground)
                .app_class(tw(if multiline {
                    "min-h-[96px] w-full rounded-lg border border-input bg-background px-[11px] py-2 text-sm shadow-xs/5 dark:bg-input/32"
                } else {
                    "h-9 w-full rounded-lg border border-input bg-background px-[11px] text-base shadow-xs/5 sm:text-sm dark:bg-input/32"
                }))
        },
    )
}

fn target_cleanup_mode(
    target: &t3_client::services::settings::scoped::ScopedSettingsTarget,
) -> &'static str {
    if target.sources.get("worktreeCleanup") == Some(&ProjectSettingSource::Project) {
        match target.json["worktreeCleanup"]["mode"].as_str() {
            Some("off") => "off",
            Some("custom") => "custom",
            _ => "inherit",
        }
    } else {
        "inherit"
    }
}

fn cleanup_mode(context: SettingsContext) -> &'static str {
    let view = context.view();
    let mode = view
        .as_ref()
        .and_then(|v| v.target())
        .map_or("inherit", target_cleanup_mode);
    if view.is_some_and(|v| {
        v.targets
            .iter()
            .any(|entry| target_cleanup_mode(entry) != mode)
    }) {
        "mixed"
    } else {
        mode
    }
}

fn cleanup(context: SettingsContext) -> AnyElement {
    region(
        move || {
            let project = project_scope(context);
            let error = if project && !supports(context, |c| c.project_worktree_cleanup) {
                Some("Update the selected machines to configure project worktree cleanup.")
            } else if !supports(context, |c| c.storage_cleanup) {
                Some(
                    "Update the selected environments to use storage cleanup, or choose a machine that supports it.",
                )
            } else {
                None
            };
            (project, error)
        },
        move |(project, error)| {
            settings_section(
                "storage-worktrees",
                Text::from("Cleanup"),
                match error {
                    Some(error) => text(Text::from(error).resolve())
                        .app_class(tw("px-3 py-3 text-sm text-muted-foreground sm:px-4"))
                        .into_element(),
                    None => view()
                        .child(project.then(|| cleanup_mode_row(context)))
                        .child(region(
                            move || !project || cleanup_mode(context) == "custom",
                            move |visible| visible.then(|| cleanup_rules(context, project)),
                        ))
                        .into_element(),
                },
            )
        },
    )
}

fn cleanup_mode_row(context: SettingsContext) -> impl IntoElement {
    settings_row(
        SettingsRowProps::titled("", Text::from("Automatic worktree cleanup"))
            .first()
            .server(&["worktreeCleanup"])
            .description(move || {
                Text::from(match cleanup_mode(context) {
                    "off" => "Keep this project's worktrees until you delete them manually.",
                    "custom" => "Use these rules for this project.",
                    _ => "Use each machine's worktree cleanup settings.",
                })
            })
            .control(move || {
                scoped_choice(
                    "Automatic worktree cleanup",
                    &[("inherit", "Inherit"), ("off", "Off"), ("custom", "Custom")],
                    move || cleanup_mode(context).into(),
                    move |mode| match mode.as_str() {
                        "inherit" => context.clear(vec!["worktreeCleanup"]),
                        "custom" => context
                            .update(json!({"worktreeCleanup": {"mode": "custom", "rules": {}}})),
                        "off" => context.update(json!({"worktreeCleanup": {"mode": "off"}})),
                        _ => {}
                    },
                )
            }),
    )
}

fn cleanup_value(context: SettingsContext, key: &str) -> Value {
    let view = context.view();
    if let Some(target) = view.as_ref().and_then(|v| v.target()) {
        resolve_worktree_cleanup(&target.json, &target.json)[key].clone()
    } else {
        t3_client::services::settings::panels::default_unified_settings()["storageCleanup"][key]
            .clone()
    }
}

fn cleanup_mixed(context: SettingsContext, key: &str) -> bool {
    let value = cleanup_value(context, key);
    context.view().is_some_and(|view| {
        view.targets
            .iter()
            .any(|target| resolve_worktree_cleanup(&target.json, &target.json)[key] != value)
    })
}

fn cleanup_patch(project: bool, key: &str, value: Value) -> Value {
    if project {
        json!({"worktreeCleanup": {"mode": "custom", "rules": {key: value}}})
    } else {
        json!({"storageCleanup": {key: value}})
    }
}

fn cleanup_row(
    context: SettingsContext,
    project: bool,
    key: &'static str,
    path: &'static [&'static str],
    mut row: SettingsRowProps,
) -> SettingsRowProps {
    row = row
        .managed_paths(if project { &["worktreeCleanup"] } else { path })
        .status(move || {
            let mixed = cleanup_mixed(context, key);
            let offline = key == "worktreeSettledAfterDays"
                && context.view().is_some_and(|view| {
                    view.selected.environments.len() > view.selected.connected.len()
                });
            match (mixed, offline) {
                (true, true) => Some(Text::content(format!(
                    "{}. {}",
                    t("Mixed across selected machines"),
                    t("Offline machines will not get this setting"),
                ))),
                (true, false) => Some(Text::from("Mixed across selected machines")),
                (false, true) => Some(Text::from("Offline machines will not get this setting")),
                (false, false) => None,
            }
        });
    if !project {
        row = row.server(&[]);
    }
    row
}

fn cleanup_switch(
    context: SettingsContext,
    project: bool,
    key: &'static str,
    path: &'static [&'static str],
    label: &'static str,
    description: &'static str,
) -> SettingsRowProps {
    cleanup_row(
        context,
        project,
        key,
        path,
        SettingsRowProps::titled("", Text::from(label)),
    )
    .description(move || Text::from(description))
    .control(move || {
        switch(
            label,
            ControlState {
                checked: Prop::bind(move || {
                    cleanup_value(context, key).as_bool().unwrap_or_default()
                }),
                ..Default::default()
            },
            Size::Default,
            move |checked| context.update(cleanup_patch(project, key, json!(checked))),
        )
    })
}

fn cleanup_rules(context: SettingsContext, project: bool) -> impl IntoElement {
    view()
        .child(settings_row(cleanup_switch(
            context, project, "worktreeOnDelete", &["worktreeCleanup", "storageCleanup.worktreeOnDelete"],
            "Delete worktrees with deleted threads",
            "Remove unused worktrees when active or archived threads are deleted. Worktrees with local changes are kept.",
        ).first()))
        .child(settings_row(cleanup_switch(
            context, project, "worktreeOnMerge", &["worktreeCleanup", "storageCleanup.worktreeOnMerge"],
            "Delete merged worktrees",
            "Remove worktrees whose pull request is merged and whose commits are included in the default branch.",
        )))
        .child(retention_row(
            context, project, "worktreeAfterDays", &["worktreeCleanup", "storageCleanup.worktreeAfterDays"],
            "Delete inactive worktrees",
            "Remove worktrees after their threads have been inactive for this many days. Branches and thread history are kept.",
            1,
        ))
        .child(advanced_settings(
            &["storage-worktrees", "storage-worktree-keep-when"],
            view()
                .child(region(
                    move || context.view().is_some_and(|view| !view.selected.connected.is_empty())
                        && supports(context, |c| c.settled_worktree_cleanup),
                    move |visible| visible.then(|| retention_row(
                        context, project, "worktreeSettledAfterDays",
                        &["worktreeCleanup", "storageCleanup.worktreeSettledAfterDays"],
                        "Delete settled worktrees",
                        "Remove worktrees this many days after their thread settles. Set 0 days to remove them at the next cleanup. Branches and thread history are kept.",
                        0,
                    )),
                ))
                .child(settings_row(cleanup_switch(
                    context, project, "worktreeUnchanged", &["worktreeCleanup", "storageCleanup.worktreeUnchanged"],
                    "Delete unchanged worktrees",
                    "Remove worktrees with no commits beyond the default branch.",
                )))
                .child(region(
                    move || supports(context, |c| c.storage_cleanup_run),
                    move |visible| visible.then(|| keep_when(context, project)),
                )),
        ))
}

fn retention_row(
    context: SettingsContext,
    project: bool,
    key: &'static str,
    path: &'static [&'static str],
    label: &'static str,
    description: &'static str,
    minimum: i64,
) -> impl IntoElement {
    settings_row(
        cleanup_row(
            context,
            project,
            key,
            path,
            SettingsRowProps::titled("", Text::from(label)),
        )
        .description(move || Text::from(description))
        .control(move || retention_control(context, project, key, label, minimum)),
    )
}

fn retention_control(
    context: SettingsContext,
    project: bool,
    key: &'static str,
    label: &'static str,
    minimum: i64,
) -> impl IntoElement {
    let saved = computed(move || cleanup_value(context, key).as_i64());
    let draft = signal(String::new());
    let editing = signal(false);
    let edited = signal(false);
    let switched_off = signal(false);
    effect(move || {
        draft.set(saved.get().map(|days| days.to_string()).unwrap_or_default());
        editing.set(false);
        edited.set(false);
        switched_off.set(false);
    });
    let on = move || (saved.get().is_some() && !switched_off.get()) || editing.get();
    let row = element_ref();
    let cancel = move || {
        draft.set(
            saved
                .get_untracked()
                .map(|days| days.to_string())
                .unwrap_or_default(),
        );
        editing.set(false);
        edited.set(false);
    };
    let commit = move |explicit: bool| {
        if !on() || (!explicit && !edited.get_untracked()) {
            if !explicit && editing.get_untracked() {
                cancel();
            }
            return;
        }
        let Ok(days) = draft.with_untracked(|text| text.trim().parse::<i64>()) else {
            cancel();
            return;
        };
        let days = days.clamp(minimum, 3650);
        edited.set(false);
        draft.set(days.to_string());
        if saved.get_untracked() != Some(days) {
            context.update_with(cleanup_patch(project, key, json!(days)), move || {
                if editing.try_get() == Some(true) {
                    cancel();
                }
            });
        }
    };
    row.on_focus_out(move || commit(false));
    view()
        .element_ref(&row)
        .app_class(tw("flex flex-row items-center gap-3"))
        .child(region(on, move |on| {
            if !on {
                return text(t("Off")).app_class(tw("text-xs text-muted-foreground")).into_element();
            }
            view().app_class(tw("flex flex-row items-center gap-2"))
                .child(nsh_base::input()
                    .label(crate::i18n::tr("{label} in days", &[("label", t3_i18n::Value::Text(Text::from(label).resolve().to_string()))]))
                    .value(move || draft.get())
                    .keyboard_type(KeyboardType::Number)
                    .on_change(move |next: &str| {
                        edited.set(true);
                        draft.set(next.to_owned());
                    })
                    .submit_behavior(SubmitBehavior::Submit)
                    .on_submit(move |_: &str| commit(true))
                    .on_key_down(move |event: &KeyEvent| {
                        if event.key.as_ref() == "Escape" { cancel(); }
                    })
                    .app_class(tw("h-8 w-20 rounded-lg border border-input bg-background px-[11px] text-sm")))
                .child(text(t("days")).app_class(tw("self-center pr-2 text-xs")))
                .into_element()
        }))
        .child(switch(
            label,
            ControlState { checked: Prop::bind(on), ..Default::default() },
            Size::Default,
            move |checked| {
                if checked {
                    editing.set(true);
                    draft.set("8".into());
                } else {
                    cancel();
                    if saved.get_untracked().is_some() {
                        switched_off.set(true);
                        context.update_with(cleanup_patch(project, key, Value::Null), move || {
                            let _ = switched_off.try_set(false);
                        });
                    }
                }
            },
        ))
}

fn keep_when(context: SettingsContext, project: bool) -> impl IntoElement {
    use crate::ui::select::{
        Popup, SelectItem, SelectProps, Size as SelectSize, Value as SelectValue, select,
    };

    const KEY: &str = "worktreeKeepWhen";
    settings_row(
        cleanup_row(
            context,
            project,
            KEY,
            &["worktreeCleanup", "storageCleanup.worktreeKeepWhen"],
            SettingsRowProps::searchable("storage-worktree-keep-when"),
        )
        .description(move || {
            Text::from(match cleanup_value(context, KEY).as_str() {
                Some("any-local-files") => "Includes ignored files, except node_modules.",
                Some("tracked-changes") => "Deletes untracked and ignored files, including .env.",
                _ => "Keeps edits and untracked files. Deletes ignored files, including .env.",
            })
        })
        .control(move || {
            let mut props = SelectProps::new("Keep worktrees with local changes");
            props.items = choices(&[
                ("any-local-files", "Any local files"),
                ("uncommitted-changes", "Uncommitted changes"),
                ("tracked-changes", "Edited tracked files"),
            ])
            .into_iter()
            .map(|(id, label)| SelectItem::new(id, label.resolve()))
            .collect::<Vec<_>>()
            .into();
            props.value = Prop::bind(move || {
                let selection = if cleanup_mixed(context, KEY) {
                    None
                } else {
                    cleanup_value(context, KEY)
                        .as_str()
                        .map(|value| SharedString::from(value.to_owned()))
                };
                SelectValue::Controlled(selection)
            });
            props.placeholder = Prop::bind(move || {
                if cleanup_mixed(context, KEY) {
                    Text::from("Mixed")
                } else {
                    Text::content("")
                }
            });
            props.size = SelectSize::Sm;
            props.popup = Popup {
                align: Align::End,
                ..Popup::default()
            };
            view()
                .app_class(tw("w-auto min-w-0"))
                .child(select(props, move |next| {
                    if let Some(next) = next {
                        context.update(cleanup_patch(project, KEY, json!(next.to_string())));
                    }
                }))
        }),
    )
}
