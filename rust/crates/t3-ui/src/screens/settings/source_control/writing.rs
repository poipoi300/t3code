//! Port of ProjectDefaultsSettings.tsx (source-control) and SourceControlWritingSettings.tsx.
use nsh_core::prelude::*;
use serde_json::{Value, json};
use t3_client::services::settings::panels::default_unified_settings;

use super::super::shell::{
    RowReset, SettingsContext, SettingsRowProps, advanced_settings, region, settings_row,
    settings_section, settings_select,
};
use crate::{
    i18n::{Text, t},
    theme::tw,
    ui::{
        button::{ButtonContent, ButtonSize, ButtonVariant, button},
        switch::{ControlState, Size, switch},
    },
};

const STYLE: &str = "sourceControlWritingStyle";
const WRITING_OPTIONS: [(&str, &str); 3] = [
    ("repo_conventions", "Repository conventions"),
    ("conventional_commits", "Conventional Commits"),
    ("custom", "Custom instructions"),
];

fn bool_row(
    context: SettingsContext,
    id: &'static str,
    keys: &'static [&'static str],
    label: &'static str,
    description: impl Fn() -> Text + 'static,
) -> impl IntoElement {
    let key = keys[0];
    settings_row(
        SettingsRowProps::searchable(id)
            .server(keys)
            .description(description)
            .reset(RowReset::new(
                label,
                move || context.setting(key) != default_unified_settings()[key],
                move || context.update(json!({key:default_unified_settings()[key]})),
            ))
            .control(move || {
                let mixed = move || context.mixed(keys);
                switch(
                    t(label),
                    ControlState {
                        checked: Prop::bind(move || {
                            !mixed() && context.setting(key).as_bool().unwrap_or(false)
                        }),
                        mixed: Prop::bind(mixed),
                        ..Default::default()
                    },
                    Size::Default,
                    move |checked| context.update(json!({key:checked})),
                )
            }),
    )
}

pub(super) fn repositories(context: SettingsContext) -> impl IntoElement {
    settings_section("source-control-defaults", Text::from("Repositories"), view()
        .child(bool_row(context, "automatic-pull", &["defaultAutoPull"], "Default automatic pull", move || {
            if context.view().is_some_and(|view| view.scope.is_project_scope()) {
                Text::from("Keeps this project's default branch current when the checkout has no local changes or commits.")
            } else {
                Text::from("Keeps the default branch current when the checkout has no local changes or commits. Projects can override it.")
            }
        }))
        .child(bool_row(context, "remove-agent-credits-on-merge", &["removeAgentCreditsOnMerge"], "Remove agent credits when merging", || {
            Text::from("Remove recognized agent credit lines from merge and squash messages on hosts that support it, keeping human co-authors. Includes auto-merge. Excludes merge queues, stack merges, and existing commits.")
        }))
        .child(settings_row(SettingsRowProps::searchable("pull-request-merge-method")
            .server(&["pullRequestMergeMethod"])
            .description(move || if context.view().is_some_and(|view| view.scope.is_project_scope()) {
                Text::from("Pull requests in this project start with this method.")
            } else {
                Text::from("Pull requests start with this method. Last selected reuses whatever you chose most recently on this device.")
            })
            .reset(RowReset::new("default merge method", move || !context.setting("pullRequestMergeMethod").is_null(),
                move || context.update(json!({"pullRequestMergeMethod":null}))))
            .control(move || settings_select("Default pull request merge method", "w-full sm:w-56",
                vec![("last", Text::from("Last selected")), ("merge", Text::from("Merge commit")),
                    ("squash", Text::from("Squash and merge")), ("rebase", Text::from("Rebase and merge"))],
                move || {
                    if context.mixed(&["pullRequestMergeMethod"]) {
                        String::new()
                    } else {
                        context.setting("pullRequestMergeMethod").as_str().unwrap_or("last").to_owned()
                    }
                }, || false, move |mode| {
                    if mode == "last" {
                        context.update(json!({"pullRequestMergeMethod":null}));
                    } else if matches!(mode.as_str(), "merge" | "squash" | "rebase") {
                        context.update(json!({"pullRequestMergeMethod":mode}));
                    }
                }
            )))))
}

fn style_field_mixed(context: SettingsContext, name: &str) -> bool {
    context.view().is_some_and(|view| {
        view.targets.first().is_some_and(|first| {
            view.targets
                .iter()
                .any(|target| target.json[STYLE][name] != first.json[STYLE][name])
        })
    })
}

pub(super) fn source_control_writing_settings_section(
    context: SettingsContext,
) -> impl IntoElement {
    let edit_all = signal(false);
    let custom = move || context.setting(STYLE)["mode"] == "custom";
    let custom_instructions = move || {
        context.setting(STYLE)["customInstructions"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    };
    let custom_mixed = move || style_field_mixed(context, "customInstructions");
    let guides_enabled = move || {
        context
            .setting("reviewGuidesEnabled")
            .as_bool()
            .unwrap_or_default()
    };

    settings_section("source-control-text-generation", Text::from("Commits & pull requests"), view()
        .child(settings_row(SettingsRowProps::searchable("source-control-writing-style")
            .first()
            .server(&[STYLE])
            .description(move || match context.setting(STYLE)["mode"].as_str() {
                Some("conventional_commits") => Text::from("Conventional Commit prefixes, with short pull request titles and descriptions."),
                Some("custom") => Text::from("Your instructions shape commit messages and pull request titles and descriptions."),
                _ => Text::from("Commit messages and pull request titles and descriptions match each project's history."),
            })
            .reset(RowReset::new("commit and pull request writing", move || {
                let current = context.setting(STYLE);
                let defaults = &default_unified_settings()[STYLE];
                custom_mixed() || style_field_mixed(context, "mode")
                    || current["mode"] != defaults["mode"] || current["customInstructions"] != defaults["customInstructions"]
            }, move || {
                let defaults = &default_unified_settings()[STYLE];
                context.update(json!({STYLE:{"mode":defaults["mode"],"customInstructions":defaults["customInstructions"]}}));
            }))
            .control(move || settings_select("Commit and pull request writing", "w-full sm:w-56",
                WRITING_OPTIONS.iter().map(|(key, label)| (*key, Text::from(*label))).collect(),
                move || {
                    if style_field_mixed(context, "mode") {
                        String::new()
                    } else {
                        context.setting(STYLE)["mode"].as_str().unwrap_or_default().to_owned()
                    }
                }, || false, move |mode| {
                    if WRITING_OPTIONS.iter().any(|(key, _)| *key == mode) {
                        context.update(json!({STYLE:{"mode":mode}}));
                    }
                }))
            .children(move || region(move || (custom(), custom_mixed(), edit_all.get(), custom_instructions()),
                move |(show, mixed, editing, initial)| {
                    show.then(|| {
                        if mixed && !editing {
                            return view().app_class(tw("flex flex-col items-start gap-2"))
                                .child(text(t("Different instructions across the selected environments.")).app_class(tw("text-xs text-muted-foreground")))
                                .child(button(ButtonContent::Label(t("Edit all")), ButtonVariant::Outline, ButtonSize::Sm,
                                    || false, move || edit_all.set(true))).into_element();
                        }
                        let input = if mixed { String::new() } else { initial };
                        let save = move |value| {
                            context.update(json!({STYLE:{"customInstructions":value}}));
                            edit_all.set(false);
                        };
                        writing_instructions("Custom writing instructions", input, mixed, save).into_element()
                    })
                }
            ))))
        .child(bool_row(context, "review-guides", &["reviewGuidesEnabled"], "Review guides",
            || Text::from("Generate a review guide for pull requests.")))
        .child(region(guides_enabled, move |enabled| {
            enabled.then(|| settings_row(SettingsRowProps::searchable("review-guide-instructions")
                .server(&["reviewGuideInstructions"])
                .description(|| Text::from("Give the review guide agent additional instructions."))
                .reset(RowReset::new("review guide instructions", move || context.mixed(&["reviewGuideInstructions"])
                    || context.setting("reviewGuideInstructions") != "",
                    move || context.update(json!({"reviewGuideInstructions":""}))))
                .control(move || region(move || (context.mixed(&["reviewGuideInstructions"]), context.setting("reviewGuideInstructions")),
                    move |(mixed, value)| writing_instructions("Review guide instructions",
                        if mixed { String::new() } else { value.as_str().unwrap_or_default().to_owned() },
                        false, move |value| context.update(json!({"reviewGuideInstructions":value}))),
                ))))
        }))
        .child(advanced_settings(&["follow-change-request-templates"], settings_row(
            SettingsRowProps::searchable("follow-change-request-templates")
                .server(&[STYLE])
                .description(|| Text::from("Use the repository's pull request template when it has one."))
                .reset(RowReset::new("pull request templates", move || {
                    style_field_mixed(context, "followChangeRequestTemplates")
                    || context.setting(STYLE)["followChangeRequestTemplates"] != default_unified_settings()[STYLE]["followChangeRequestTemplates"]
                }, move || context.update(json!({STYLE:{"followChangeRequestTemplates":default_unified_settings()[STYLE]["followChangeRequestTemplates"]}}))))
                .control(move || {
                    let mixed = move || style_field_mixed(context, "followChangeRequestTemplates");
                    switch(t("Follow pull request templates"), ControlState {
                        checked: Prop::bind(move || !mixed() && context.setting(STYLE)["followChangeRequestTemplates"].as_bool().unwrap_or(false)),
                        mixed: Prop::bind(mixed),
                        ..Default::default()
                    }, Size::Default, move |checked| context.update(json!({STYLE:{"followChangeRequestTemplates":checked}})))
                }),
        ))))
}

fn writing_instructions(
    label: &'static str,
    initial: String,
    explicit: bool,
    save: impl Fn(Value) + 'static,
) -> impl IntoElement {
    let input = element_ref();
    let value = signal(initial);
    let edited = signal(false);
    let commit = move || {
        if edited.get_untracked() {
            edited.set(false);
            save(json!(value.with_untracked(|value| value.trim().to_owned())));
        }
    };
    let commit = std::rc::Rc::new(commit);
    if !explicit {
        let commit = commit.clone();
        input.on_focus_out(move || commit());
    }
    view().app_class(tw("flex flex-col gap-2"))
        .child(nsh_base::input()
            .element_ref(&input)
            .label(Text::client_key(label).resolve())
            .multiline(true)
            .value(move || value.get())
            .on_change(move |next: &str| {
                value.set(next.to_owned());
                edited.set(true);
            })
            .app_class(tw("min-h-20 w-full rounded-md border border-input bg-background px-3 py-2 text-sm")))
        .child(explicit.then(|| button(ButtonContent::Label(t("Save")), ButtonVariant::Default, ButtonSize::Sm,
            move || !edited.get(), move || commit())))
}
