//! Port of apps/web/src/components/settings/SettingsPanels.tsx ThreadsSettingsPanel and
//! AutoSettleDaysInput (route apps/web/src/routes/_chat.settings.threads.tsx). The field logic is
//! `t3_client::services::settings::panels::AutoSettleDaysField`.

use nsh_core::{KeyEvent, prelude::*};
use serde_json::{Value, json};
use t3_client::services::settings::panels::{AutoSettleDaysField, default_unified_settings};

use super::shell::{
    RowReset, SettingsContext, SettingsRowProps, advanced_settings, settings_page_container,
    settings_row, settings_section, settings_select, use_settings,
};
use crate::i18n::{Text, t};
use crate::state::nav::SettingsPage;
use crate::theme::{palette, tw};
use crate::ui::switch::{ControlState, Size as SwitchSize, switch};

pub fn screen() -> AnyElement {
    threads_settings_panel().into_element()
}

fn value(context: SettingsContext, key: &'static str) -> Value {
    context.setting(key)
}

fn reset(context: SettingsContext, key: &'static str, label: &'static str) -> RowReset {
    RowReset::new(
        label,
        move || value(context, key) != default_unified_settings()[key],
        move || context.update(json!({ key: default_unified_settings()[key] })),
    )
}

/// `ScopedSwitch`: mixed across the selected targets shows neither on nor off.
fn scoped_switch(
    context: SettingsContext,
    keys: &'static [&'static str],
    label: &'static str,
    checked: impl Fn() -> bool + 'static,
    on_change: impl Fn(bool) + 'static,
) -> impl IntoElement {
    let mixed = move || context.mixed(keys);
    let checked = std::rc::Rc::new(checked);
    switch(
        label,
        ControlState {
            checked: Prop::bind(move || !mixed() && checked()),
            mixed: Prop::bind(mixed),
            ..Default::default()
        },
        SwitchSize::Default,
        on_change,
    )
}

fn bool_switch(
    context: SettingsContext,
    key: &'static str,
    keys: &'static [&'static str],
    label: &'static str,
) -> impl IntoElement {
    scoped_switch(
        context,
        keys,
        label,
        move || value(context, key).as_bool().unwrap_or(false),
        move |checked| context.update(json!({ key: checked })),
    )
}

/// `AUTO_SETTLE_DEFAULT_DAYS`.
fn auto_settle_default_days() -> i64 {
    default_unified_settings()["sidebarAutoSettleAfterDays"]
        .as_i64()
        .unwrap_or(7)
}

/// `AutoSettleDaysInput`: saves when the edit is finished (blur or Enter), never per keystroke;
/// Escape cancels a dirty edit without closing Settings.
fn auto_settle_days_input(context: SettingsContext) -> impl IntoElement {
    let saved = move || {
        value(context, "sidebarAutoSettleAfterDays")
            .as_i64()
            .unwrap_or_else(auto_settle_default_days)
    };
    let field = signal(AutoSettleDaysField::new(untracked(saved)));
    effect(move || {
        let saved = saved();
        field.update(|current| *current = current.sync(saved));
    });
    let commit = move || {
        let (next, submit) = field.get_untracked().commit();
        let revision = next.revision;
        field.set(next);
        if let Some(days) = submit {
            context.update_with(json!({ "sidebarAutoSettleAfterDays": days }), move || {
                field.update(|current| *current = current.fail(revision));
            });
        }
    };
    nsh_base::input()
        .value(move || field.with(|f| f.draft.clone()))
        .keyboard_type(KeyboardType::Number)
        .on_change(move |draft: &str| field.update(|current| *current = current.edit(draft)))
        .on_blur(commit)
        .on_submit(move |_: &str| commit())
        .submit_behavior(SubmitBehavior::Submit)
        .on_key_down(move |event: &KeyEvent| {
            if event.key.as_ref() == "Escape" && field.with_untracked(|f| f.dirty) {
                field.update(|current| *current = current.cancel());
            }
        })
        .label(t("Days of inactivity before auto-settle"))
        .text_color(move || palette().foreground)
        .app_class(tw("h-8 w-full sm:w-24 rounded-lg border border-input bg-background px-[11px] text-sm shadow-xs/5 dark:bg-input/32"))
}

/// `DraftInput`: edits a draft and commits it on blur or Enter.
fn draft_input(
    label: &'static str,
    value: impl Fn() -> String + 'static,
    placeholder: impl Fn() -> String + 'static,
    on_commit: impl Fn(String) + 'static,
) -> impl IntoElement {
    let committed = computed(value);
    let draft = signal(untracked(|| committed.get()));
    effect(move || draft.set(committed.get()));
    let on_commit = std::rc::Rc::new(on_commit);
    let commit = {
        let on_commit = on_commit.clone();
        move || {
            let next = draft.get_untracked();
            if next != committed.get_untracked() {
                on_commit(next);
            }
        }
    };
    let submit = commit.clone();
    nsh_base::input()
        .value(move || draft.get())
        .on_change(move |text: &str| draft.set(text.to_owned()))
        .on_blur(commit)
        .on_submit(move |_: &str| submit())
        .submit_behavior(SubmitBehavior::Submit)
        .placeholder(placeholder)
        .label(t(label))
        .text_color(move || palette().foreground)
        .app_class(tw("h-8 w-full sm:w-72 rounded-lg border border-input bg-background px-[11px] text-sm shadow-xs/5 dark:bg-input/32"))
}

const RESPONSE_STREAMING_MODE_LABELS: [(&str, &str); 2] = [
    ("turn", "Wait for the full response"),
    ("paragraph", "Show finished paragraphs"),
];

fn response_streaming_description(mode: &str) -> &'static str {
    if mode == "turn" {
        "Text appears once the agent finishes its turn."
    } else {
        "Each paragraph or code block appears as soon as it is complete."
    }
}

/// `ThreadsSettingsPanel`. Not yet here: `ProjectDefaultsSettings`, `SnoozePresetsRow`,
/// `ThreadTitleInstructionsSettings` and the text generation model and traits pickers (see the
/// SET-THREADS hand-off).
pub fn threads_settings_panel() -> impl IntoElement {
    let context = use_settings();
    let has_server_targets = move || {
        context
            .view()
            .is_some_and(|v| !v.selected.connected.is_empty())
    };
    let supports_auto_settlement_scope = move || {
        context.view().is_some_and(|v| {
            v.selected.connected.iter().all(|e| {
                e.config.as_ref().is_some_and(|c| {
                    c.environment.capabilities.thread_auto_settlement_scope == Some(true)
                })
            })
        })
    };
    let auto_settle_on = move || !value(context, "sidebarAutoSettleAfterDays").is_null();
    settings_page_container(
        view()
            .app_class(tw("flex flex-col gap-8"))
            .child(settings_section(
                "new-projects",
                Text::from("New projects"),
                view()
                    .child(settings_row(
                        SettingsRowProps::searchable("add-project-starts-in")
                            .first()
                            .server(&["addProjectBaseDirectory"])
                            .description(|| Text::from("Leave empty to use \"~/\" when the Add Project browser opens."))
                            .reset(reset(context, "addProjectBaseDirectory", "add project base directory"))
                            .control(move || {
                                let mixed = move || context.mixed(&["addProjectBaseDirectory"]);
                                draft_input(
                                    "Add project base directory",
                                    move || if mixed() { String::new() } else { value(context, "addProjectBaseDirectory").as_str().unwrap_or_default().to_owned() },
                                    move || if mixed() { t("Mixed") } else { "~/".to_owned() },
                                    move |next| context.update(json!({ "addProjectBaseDirectory": next })),
                                )
                            }),
                    ))
                    .child(settings_row(
                        SettingsRowProps::searchable("ghq-clone-layout")
                            .server(&["ghqCloneLayout"])
                            .description(|| Text::from("Clones go to github.com/owner/repo under the folder above, like ghq."))
                            .reset(reset(context, "ghqCloneLayout", "clone into host folders"))
                            .control(move || bool_switch(context, "ghqCloneLayout", &["ghqCloneLayout"], "Clone into host folders")),
                    )),
            ))
            .child(project_defaults_settings(context))
            .child(settings_section(
                "usage-limits",
                Text::from("Usage limits"),
                view()
                    .child(settings_row(
                        SettingsRowProps::searchable("auto-resume-limited-threads")
                            .first()
                            .server(&["autoResumeLimitedThreads"])
                            .description(|| Text::from("Automatically resume threads when their usage limits reset. You can cancel auto-resume in each thread."))
                            .control(move || bool_switch(context, "autoResumeLimitedThreads", &["autoResumeLimitedThreads"], "Resume after usage limits")),
                    ))
                    .child(settings_row(
                        SettingsRowProps::searchable("snooze-limited-threads")
                            .server(&["snoozeLimitedThreads"])
                            .description(|| Text::from("Snooze threads until their usage limits reset. Enable auto-resume to continue the agent after the reset."))
                            .control(move || bool_switch(context, "snoozeLimitedThreads", &["snoozeLimitedThreads"], "Snooze until limits reset")),
                    )),
            ))
            .child(when(has_server_targets, move || {
                settings_section(
                    "auto-settle",
                    Text::from("Auto-settle"),
                    view()
                        .child(settings_row(
                            SettingsRowProps::searchable("auto-settle-merged-threads")
                                .first()
                                .server(&["sidebarAutoSettleOnMerge"])
                                .description(|| Text::from("Settle a thread when its pull request merges. Closed pull requests still settle automatically."))
                                .reset(reset(context, "sidebarAutoSettleOnMerge", "auto-settle on merge"))
                                .control(move || bool_switch(context, "sidebarAutoSettleOnMerge", &["sidebarAutoSettleOnMerge"], "Auto-settle merged threads")),
                        ))
                        .child(settings_row(
                            SettingsRowProps::searchable("auto-settle-inactive-threads")
                                .server(&["sidebarAutoSettleAfterDays"])
                                .description(|| Text::from("Sidebar threads with no activity for this long settle automatically."))
                                .reset(reset(context, "sidebarAutoSettleAfterDays", "auto-settle"))
                                .control(move || {
                                    scoped_switch(
                                        context,
                                        &["sidebarAutoSettleAfterDays"],
                                        "Auto-settle inactive threads",
                                        auto_settle_on,
                                        move |checked| {
                                            context.update(json!({
                                                "sidebarAutoSettleAfterDays": if checked { json!(auto_settle_default_days()) } else { Value::Null }
                                            }))
                                        },
                                    )
                                }),
                        ))
                        .child(when(auto_settle_on, move || {
                            let mut row = SettingsRowProps::searchable("days-before-auto-settle")
                                .server(&["sidebarAutoSettleAfterDays"])
                                .description(|| Text::from("Any new activity un-settles a thread automatically."))
                                .control(move || auto_settle_days_input(context));
                            // The web passes only the registry title here, not the anchor.
                            row.id = "";
                            settings_row(row)
                        }))
                        .child(when(move || auto_settle_on() && supports_auto_settlement_scope(), move || {
                            let mut row = SettingsRowProps::searchable("keep-threads-with-pull-requests")
                                .server(&["sidebarAutoSettleKeepPullRequests"])
                                .description(|| Text::from("Inactivity does not settle threads with a pull request. Merge and close still do."))
                                .control(move || bool_switch(context, "sidebarAutoSettleKeepPullRequests", &["sidebarAutoSettleKeepPullRequests"], "Keep threads with a PR"));
                            row.title = Text::from("Keep threads with a PR");
                            row.id = "";
                            settings_row(row)
                        })),
                )
            }))
            .child(settings_section(
                "text-generation",
                Text::from("Text generation"),
                view()
                    .child(settings_row(
                        SettingsRowProps::searchable("text-generation-model")
                            .first()
                            .server(&["textGenerationModelSelection"])
                            .description(|| Text::from("For thread titles and other generated text. Source control has its own setting. Text generators from Providers also appear here."))
                            .reset(RowReset::new(
                                "text generation model",
                                move || has_server_targets() && value(context, "textGenerationModelSelection") != default_unified_settings()["textGenerationModelSelection"],
                                move || context.update(json!({ "textGenerationModelSelection": default_unified_settings()["textGenerationModelSelection"] })),
                            ))
                            .control(move || text_generation_control(context, has_server_targets)),
                    ))
                    .child(advanced_settings(
                        &["thread-title-instructions", "response-streaming"],
                        view().child(thread_title_instructions_settings(context)).child(settings_row(
                            SettingsRowProps::searchable("response-streaming")
                                .server(&["responseStreamingMode"])
                                .description(move || {
                                    if context.mixed(&["responseStreamingMode"]) {
                                        Text::from("The selected targets use different streaming modes.")
                                    } else {
                                        Text::from(response_streaming_description(value(context, "responseStreamingMode").as_str().unwrap_or("paragraph")))
                                    }
                                })
                                .reset(reset(context, "responseStreamingMode", "response streaming"))
                                .control(move || {
                                    settings_select(
                                        "Response streaming",
                                        "w-full sm:w-56",
                                        RESPONSE_STREAMING_MODE_LABELS.iter().map(|(v, l)| (*v, Text::from(*l))).collect(),
                                        move || {
                                            if context.mixed(&["responseStreamingMode"]) {
                                                String::new()
                                            } else {
                                                value(context, "responseStreamingMode").as_str().unwrap_or_default().to_owned()
                                            }
                                        },
                                        || false,
                                        move |mode| {
                                            if mode == "turn" || mode == "paragraph" {
                                                context.update(json!({ "responseStreamingMode": mode }));
                                            }
                                        },
                                    )
                                }),
                        )),
                    )),
            ))
    )
}

/// The text generation row's control: the composer's `ProviderModelPicker` with its effort
/// column (the web's `ProviderModelPicker` plus `TraitsPicker`), or the web's sentences.
fn text_generation_control(
    context: SettingsContext,
    has_server_targets: impl Fn() -> bool + Copy + 'static,
) -> impl IntoElement {
    use crate::screens::composer::model_picker::{
        ModelKey, PickerData, PickerProps, TraitsView, provider_model_picker,
    };
    use std::rc::Rc;
    use t3_client::services::settings::models::{
        selection_traits, text_generation_selection, with_option,
    };
    use t3_contracts::model_selection::ModelSelection;
    let state = computed(move || {
        let targets = context.text_generation_targets();
        let settings = context.server_settings();
        let project = context
            .view()
            .and_then(|v| v.target().and_then(|t| t.project_id.clone()));
        let has_provider = targets.has_provider(&settings, project.as_ref());
        let selection = text_generation_selection(&settings, &targets.providers);
        (Rc::new(targets), settings, project, has_provider, selection)
    });
    let save = move |selection: ModelSelection| {
        let (targets, settings, _, _, _) = state.get_untracked();
        if let Some(reason) =
            context.model_disabled_reason(&selection.instance_id, &selection.model, true)
        {
            crate::ui::toast::toast_manager().add(crate::ui::toast::ToastOptions {
                variant: crate::ui::toast::Variant::Error,
                title: Text::from("Text generation model not saved"),
                description: Some(Text::content(super::shell::settings_text(&reason))),
                ..Default::default()
            });
            return;
        }
        let mut next = settings.as_ref().clone();
        next.text_generation_model_selection = selection;
        let resolved = text_generation_selection(&next, &targets.providers);
        context.update(json!({ "textGenerationModelSelection": resolved }));
    };
    dynamic(move || {
        if !has_server_targets() {
            return text(t(
                "Connect an environment to choose its text generation model.",
            ))
            .app_class(tw("text-sm text-muted-foreground"))
            .into_element();
        }
        if !state.with(|(_, _, _, has_provider, _)| *has_provider) {
            return text(t("No text generation providers available."))
                .app_class(tw("text-sm text-muted-foreground"))
                .into_element();
        }
        untracked(|| {
            let data = computed(move || {
                let (targets, settings, project, _, selection) = state.get();
                Rc::new(PickerData::from_catalog(
                    &targets.providers,
                    &settings,
                    project.as_ref(),
                    &selection,
                ))
            });
            let traits = computed(move || {
                let (targets, _, _, _, selection) = state.get();
                let view = selection_traits(&targets.providers, &selection);
                TraitsView {
                    model: Some(ModelKey::from(&selection)),
                    reasoning: view.reasoning,
                    others: view.others,
                    speed: view.speed,
                    ultracode: view.ultracode,
                    summary: Some(view.summary),
                    disabled_reason: None,
                    unavailable: view.unavailable,
                }
            });
            let mut props =
                PickerProps::new(Prop::bind(move || data.get()), move |key: ModelKey| {
                    save(ModelSelection {
                        instance_id: key.instance_id,
                        model: key.model,
                        options: None,
                    });
                });
            props.traits = Prop::bind(move || traits.get());
            props.docked = false;
            props.trigger_label = Prop::bind(move || {
                context
                    .mixed(&["textGenerationModelSelection"])
                    .then(|| t("Mixed"))
            });
            // The web also passes the instance (`instanceId`); the Providers page reads its own.
            props.on_open_provider_setup = Some(Rc::new(move |_instance| {
                let machine = state
                    .get_untracked()
                    .0
                    .environment
                    .as_ref()
                    .map(|e| e.0.clone());
                context.open_page(
                    SettingsPage::Providers,
                    t3_client::services::settings::scope::SettingsScopeSearch {
                        machine,
                        ..Default::default()
                    },
                );
            }));
            props.on_option_pick = Some(Rc::new(move |key: ModelKey, option| {
                let (_, _, _, _, selection) = state.get_untracked();
                if selection.instance_id == key.instance_id && selection.model == key.model {
                    save(with_option(&selection, option));
                }
            }));
            view()
                .app_class(tw(
                    "flex flex-row flex-wrap items-center justify-end gap-1.5",
                ))
                .child(provider_model_picker(props))
                .into_element()
        })
    })
}

/// Port of ThreadTitleInstructionsSettings.tsx: saved on blur when edited.
fn thread_title_instructions_settings(context: SettingsContext) -> impl IntoElement {
    let current = move || {
        value(context, "threadTitleInstructions")
            .as_str()
            .unwrap_or_default()
            .to_owned()
    };
    let mixed = move || context.mixed(&["threadTitleInstructions"]);
    settings_row(
        SettingsRowProps::searchable("thread-title-instructions")
            .first()
            .server(&["threadTitleInstructions"])
            .description(|| Text::from("Appended to the prompt for new and regenerated thread titles."))
            .reset(RowReset::new(
                "thread title instructions",
                move || mixed() || !current().is_empty(),
                move || context.update(json!({ "threadTitleInstructions": "" })),
            ))
            .children(move || {
                // `key={scope:mixed:value}`: a new scope or saved value starts a fresh field.
                dynamic(move || {
                    let mixed = mixed();
                    let saved = current();
                    let _ = context.view().map(|v| v.search.clone());
                    untracked(|| {
                        let draft = signal(if mixed { String::new() } else { saved.clone() });
                        let edited = signal(false);
                        view()
                            .app_class(tw("mt-3 max-w-2xl pb-3.5"))
                            .child(
                                nsh_base::input()
                                    .multiline(true)
                                    .value(move || draft.get())
                                    .on_change(move |text: &str| {
                                        edited.set(true);
                                        draft.set(text.to_owned());
                                    })
                                    .on_blur(move || {
                                        let value = draft.get_untracked().trim().to_owned();
                                        if edited.get_untracked() && (mixed || value != current()) {
                                            context.update(json!({ "threadTitleInstructions": value }));
                                        }
                                        edited.set(false);
                                    })
                                    .label(t("Thread title instructions"))
                                    .placeholder(if mixed {
                                        t("Mixed. Enter instructions to apply to all selected targets.")
                                    } else {
                                        t("Keep titles to a few words. Lead with the main subject.")
                                    })
                                    .text_color(move || palette().foreground)
                                    .app_class(tw("min-h-[96px] w-full rounded-lg border border-input bg-background px-[11px] py-2 text-sm shadow-xs/5 dark:bg-input/32")),
                            )
                            .into_element()
                    })
                })
            }),
    )
}

/// The representative environment's providers, for the default-model pickers.
fn scope_providers(context: SettingsContext) -> Vec<t3_contracts::server::ServerProvider> {
    context
        .view()
        .and_then(|view| {
            view.selected
                .representative
                .as_ref()
                .and_then(|e| e.config.as_ref())
                .map(|c| c.providers.0.clone())
        })
        .unwrap_or_default()
}

fn scope_project(context: SettingsContext) -> Option<t3_contracts::base_schemas::ProjectId> {
    context
        .view()
        .and_then(|v| v.target().and_then(|t| t.project_id.clone()))
}

/// A settings model picker: the composer's `ProviderModelPicker` with its effort column (the
/// web's `ProviderModelPicker` beside a `TraitsPicker`). `None` shows `empty`.
fn settings_model_picker(
    context: SettingsContext,
    selection: impl Fn() -> Option<t3_contracts::model_selection::ModelSelection> + 'static,
    trigger_label: impl Fn() -> Option<String> + 'static,
    show_traits: impl Fn() -> bool + 'static,
    empty: impl Fn() -> String + 'static,
    save: impl Fn(t3_contracts::model_selection::ModelSelection) + 'static,
) -> impl IntoElement {
    use crate::screens::composer::model_picker::{
        ModelKey, PickerData, PickerProps, TraitsView, provider_model_picker,
    };
    use std::rc::Rc;
    use t3_client::services::settings::models::{selection_traits, with_option};
    use t3_contracts::model_selection::ModelSelection;
    let selection = computed(selection);
    let save = Rc::new(save);
    let trigger_label = Rc::new(trigger_label);
    let data = computed(move || {
        let selection = selection.get()?;
        let settings = context.server_settings();
        Some(Rc::new(PickerData::from_catalog(
            &scope_providers(context),
            &settings,
            scope_project(context).as_ref(),
            &selection,
        )))
    });
    let traits = computed(move || match selection.get() {
        Some(selection) if show_traits() => {
            let view = selection_traits(&scope_providers(context), &selection);
            TraitsView {
                model: Some(ModelKey::from(&selection)),
                reasoning: view.reasoning,
                others: view.others,
                speed: view.speed,
                ultracode: view.ultracode,
                summary: Some(view.summary),
                disabled_reason: None,
                unavailable: view.unavailable,
            }
        }
        _ => TraitsView::default(),
    });
    let present = computed(move || data.with(Option::is_some));
    dynamic(move || {
        if !present.get() {
            return text(empty())
                .app_class(tw("text-sm text-muted-foreground"))
                .into_element();
        }
        let save = save.clone();
        let pick = save.clone();
        let trigger_label = trigger_label.clone();
        untracked(|| {
            let initial = data.get().expect("present");
            let mut props = PickerProps::new(
                Prop::bind(move || data.get().unwrap_or_else(|| initial.clone())),
                move |key: ModelKey| {
                    pick(ModelSelection {
                        instance_id: key.instance_id,
                        model: key.model,
                        options: None,
                    })
                },
            );
            props.traits = Prop::bind(move || traits.get());
            props.docked = false;
            props.trigger_label = Prop::bind(move || trigger_label());
            props.on_option_pick = Some(Rc::new(move |key: ModelKey, option| {
                if let Some(current) = selection.get_untracked()
                    && current.instance_id == key.instance_id
                    && current.model == key.model
                {
                    save(with_option(&current, option));
                }
            }));
            view()
                .app_class(tw(
                    "flex flex-row flex-wrap items-center justify-end gap-1.5",
                ))
                .child(provider_model_picker(props))
                .into_element()
        })
    })
}

fn model_not_saved(
    title: &'static str,
    reason: &t3_client::services::settings::scoped::SettingsText,
) {
    crate::ui::toast::toast_manager().add(crate::ui::toast::ToastOptions {
        variant: crate::ui::toast::Variant::Error,
        title: Text::from(title),
        description: Some(Text::content(super::shell::settings_text(reason))),
        ..Default::default()
    });
}

/// The fallback model row (`fallbackRow`), rebuilt when the scope changes.
fn fallback_row(
    context: SettingsContext,
    presentation: Memo<t3_client::services::settings::models::DefaultModelPresentation>,
) -> SettingsRowProps {
    SettingsRowProps::searchable("fallback-model")
        .server(&["fallbackModelSelection"])
        .description(|| {
            Text::from("Used for new threads when the default model's provider is out of usage.")
        })
        .reset(RowReset::new(
            "fallback model",
            move || {
                context.mixed(&["fallbackModelSelection"])
                    || !value(context, "fallbackModelSelection").is_null()
            },
            move || context.update(json!({ "fallbackModelSelection": null })),
        ))
        .control(move || {
            settings_model_picker(
                context,
                move || {
                    context
                        .server_settings()
                        .fallback_model_selection
                        .clone()
                        .or_else(|| presentation.get().selection)
                },
                move || {
                    if context.mixed(&["fallbackModelSelection"]) {
                        Some(t("Mixed"))
                    } else if context.server_settings().fallback_model_selection.is_none() {
                        Some(t("None"))
                    } else {
                        None
                    }
                },
                move || {
                    context.server_settings().fallback_model_selection.is_some()
                        && !context.mixed(&["fallbackModelSelection"])
                },
                || t("No providers available"),
                move |selection| {
                    if let Some(reason) = context.model_disabled_reason(
                        &selection.instance_id,
                        &selection.model,
                        false,
                    ) {
                        model_not_saved("Fallback model not saved", &reason);
                        return;
                    }
                    context.update(json!({ "fallbackModelSelection": selection }));
                },
            )
        })
}

/// Port of ProjectDefaultsSettings.tsx for `category="general"` (the Threads page): default
/// model, fallback model, permissions and workspace for new threads. The project and
/// source-control categories belong to their pages' owners and reuse these rows.
pub fn project_defaults_settings(context: SettingsContext) -> impl IntoElement {
    use t3_client::services::settings::models::{
        default_model_presentation, default_models_mixed, managed_runtime_mode_denial,
        runtime_mode_label,
    };
    use t3_contracts::provider_policy::RuntimeMode;
    let project_scope = move || context.view().is_some_and(|v| v.scope.is_project_scope());
    let unavailable = move || {
        context
            .view()
            .is_none_or(|v| v.selected.connected.is_empty())
    };
    let presentation = computed(move || {
        let settings = context.server_settings();
        default_model_presentation(
            &scope_providers(context),
            &settings,
            scope_project(context).as_ref(),
        )
    });
    let mixed_model = computed(move || context.view().is_some_and(|v| default_models_mixed(&v)));
    let automatic = move || {
        context.view().is_some_and(|v| {
            !v.targets.is_empty()
                && v.targets
                    .iter()
                    .all(|t| t.settings.default_model_selection.is_none())
        })
    };
    let effort_reason = move || {
        presentation.get().selection.and_then(|selection| {
            context.model_disabled_reason(&selection.instance_id, &selection.model, false)
        })
    };
    let set_model = move |selection: Option<t3_contracts::model_selection::ModelSelection>| {
        if let Some(selection) = &selection
            && let Some(reason) =
                context.model_disabled_reason(&selection.instance_id, &selection.model, false)
        {
            model_not_saved("Default model not saved", &reason);
            return;
        }
        context.update(json!({ "defaultModelSelection": selection }));
    };
    let model_row = SettingsRowProps::titled("default-model", Text::from("Model"))
        .first()
        .server(&["defaultModelSelection"])
        .description(move || {
            Text::from(if project_scope() {
                "Model for new threads in this project."
            } else if !unavailable()
                && context.view().is_none_or(|v| {
                    t3_client::services::settings::scoped::scoped_settings_source(&v.targets, &["defaultModelSelection"])
                        != t3_client::services::settings::scoped::ScopedSettingSource::Project
                })
                && automatic()
            {
                "Default model for new threads, selected automatically. Projects can override it."
            } else {
                "Default model for new threads. Projects can override it."
            })
        })
        .status(move || {
            if mixed_model.get() {
                return Some(Text::from(if automatic() {
                    "Automatic defaults differ. Select one environment or choose a shared model to edit effort."
                } else {
                    "Defaults differ. Select one environment or choose a shared model to edit effort."
                }));
            }
            presentation
                .get()
                .provider_note
                .or_else(effort_reason)
                .map(|note| Text::content(super::shell::settings_text(&note)))
        })
        .reset(RowReset::new(
            "default model",
            move || !value(context, "defaultModelSelection").is_null(),
            move || set_model(None),
        ))
        .control(move || {
            settings_model_picker(
                context,
                move || presentation.get().selection,
                move || mixed_model.get().then(|| if automatic() { t("Automatic (mixed)") } else { t("Mixed") }),
                move || !mixed_model.get() && presentation.get().provider_note.is_none() && effort_reason().is_none(),
                move || if mixed_model.get() { t("Mixed") } else { t("No providers available") },
                move |selection| set_model(Some(selection)),
            )
        });
    let modes = [
        RuntimeMode::ApprovalRequired,
        RuntimeMode::AutoAcceptEdits,
        RuntimeMode::Auto,
        RuntimeMode::FullAccess,
    ];
    let wire = |mode: &RuntimeMode| {
        serde_json::to_value(mode)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default()
    };
    let permissions_row = SettingsRowProps::searchable("default-permissions")
        .server(&["defaultRuntimeMode"])
        .description(move || {
            Text::from(if project_scope() {
                "Permissions for new threads in this project."
            } else {
                "Default permissions for new threads. Projects can override them."
            })
        })
        .reset(RowReset::new(
            "default permissions",
            move || value(context, "defaultRuntimeMode") != t3_client::services::settings::panels::default_unified_settings()["defaultRuntimeMode"],
            move || context.update(json!({ "defaultRuntimeMode": t3_client::services::settings::panels::default_unified_settings()["defaultRuntimeMode"] })),
        ))
        .control(move || {
            use crate::ui::select::{Popup, SelectItem, SelectProps, Size, Value as SelectValue, select};
            let mut props = SelectProps::new("Default permissions");
            props.items = Prop::bind(move || {
                modes
                    .iter()
                    .map(|mode| {
                        // The web shows the reason under the label; NSH's items carry one title.
                        let denied = context.view().and_then(|v| managed_runtime_mode_denial(&v, mode.clone()));
                        SelectItem::new(wire(mode), Text::from(runtime_mode_label(mode)).resolve())
                            .disabled(denied.is_some())
                    })
                    .collect()
            });
            props.value = Prop::bind(move || {
                SelectValue::Controlled(if context.mixed(&["defaultRuntimeMode"]) {
                    None
                } else {
                    let mode = value(context, "defaultRuntimeMode");
                    mode.as_str().map(|mode| SharedString::from(mode.to_owned()))
                })
            });
            props.placeholder = Text::from("Mixed").into();
            props.size = Size::Sm;
            props.popup = Popup { align: Align::End, ..Popup::default() };
            select(props, move |next| {
                if let Some(next) = next {
                    context.update(json!({ "defaultRuntimeMode": next.to_string() }));
                }
            })
        });
    let workspace_row = {
        let mut row = SettingsRowProps::searchable("new-threads")
            .server(&["defaultThreadEnvMode"])
            .description(move || {
                Text::from(if project_scope() {
                    "Where new threads in this project start."
                } else {
                    "Where new threads start. Projects and their t3.json can override it."
                })
            })
            .reset(RowReset::new(
                "default workspace",
                move || !project_scope() && !value(context, "defaultThreadEnvMode").is_null(),
                move || context.update(json!({ "defaultThreadEnvMode": null })),
            ))
            .control(move || {
                settings_select(
                    "Default workspace",
                    "",
                    vec![
                        ("local", Text::from("Current checkout")),
                        ("worktree", Text::from("New worktree")),
                    ],
                    move || {
                        if context.mixed(&["defaultThreadEnvMode"]) {
                            String::new()
                        } else {
                            // The effective value: an unset environment default is the built-in one.
                            value(context, "defaultThreadEnvMode")
                                .as_str()
                                .unwrap_or("local")
                                .to_owned()
                        }
                    },
                    || false,
                    move |mode| {
                        if mode == "local" || mode == "worktree" {
                            context.update(json!({ "defaultThreadEnvMode": mode }));
                        }
                    },
                )
            });
        row.title = Text::from("Workspace");
        row
    };
    settings_section(
        "project-defaults",
        Text::from("New threads"),
        view()
            .child(settings_row(model_row))
            .child(when(
                move || !project_scope(),
                move || settings_row(fallback_row(context, presentation)),
            ))
            .child(settings_row(permissions_row))
            .child(settings_row(workspace_row)),
    )
}
