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
                t("New projects"),
                view()
                    .child(settings_row(
                        SettingsRowProps::searchable("add-project-starts-in")
                            .first()
                            .server(&["addProjectBaseDirectory"])
                            .description(|| t("Leave empty to use \"~/\" when the Add Project browser opens."))
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
                            .description(|| t("Clones go to github.com/owner/repo under the folder above, like ghq."))
                            .reset(reset(context, "ghqCloneLayout", "clone into host folders"))
                            .control(move || bool_switch(context, "ghqCloneLayout", &["ghqCloneLayout"], "Clone into host folders")),
                    )),
            ))
            .child(settings_section(
                "usage-limits",
                t("Usage limits"),
                view()
                    .child(settings_row(
                        SettingsRowProps::searchable("auto-resume-limited-threads")
                            .first()
                            .server(&["autoResumeLimitedThreads"])
                            .description(|| t("Automatically resume threads when their usage limits reset. You can cancel auto-resume in each thread."))
                            .control(move || bool_switch(context, "autoResumeLimitedThreads", &["autoResumeLimitedThreads"], "Resume after usage limits")),
                    ))
                    .child(settings_row(
                        SettingsRowProps::searchable("snooze-limited-threads")
                            .server(&["snoozeLimitedThreads"])
                            .description(|| t("Snooze threads until their usage limits reset. Enable auto-resume to continue the agent after the reset."))
                            .control(move || bool_switch(context, "snoozeLimitedThreads", &["snoozeLimitedThreads"], "Snooze until limits reset")),
                    )),
            ))
            .child(when(has_server_targets, move || {
                settings_section(
                    "auto-settle",
                    t("Auto-settle"),
                    view()
                        .child(settings_row(
                            SettingsRowProps::searchable("auto-settle-merged-threads")
                                .first()
                                .server(&["sidebarAutoSettleOnMerge"])
                                .description(|| t("Settle a thread when its pull request merges. Closed pull requests still settle automatically."))
                                .reset(reset(context, "sidebarAutoSettleOnMerge", "auto-settle on merge"))
                                .control(move || bool_switch(context, "sidebarAutoSettleOnMerge", &["sidebarAutoSettleOnMerge"], "Auto-settle merged threads")),
                        ))
                        .child(settings_row(
                            SettingsRowProps::searchable("auto-settle-inactive-threads")
                                .server(&["sidebarAutoSettleAfterDays"])
                                .description(|| t("Sidebar threads with no activity for this long settle automatically."))
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
                                .description(|| t("Any new activity un-settles a thread automatically."))
                                .control(move || auto_settle_days_input(context));
                            // The web passes only the registry title here, not the anchor.
                            row.id = "";
                            settings_row(row)
                        }))
                        .child(when(move || auto_settle_on() && supports_auto_settlement_scope(), move || {
                            let mut row = SettingsRowProps::searchable("keep-threads-with-pull-requests")
                                .server(&["sidebarAutoSettleKeepPullRequests"])
                                .description(|| t("Inactivity does not settle threads with a pull request. Merge and close still do."))
                                .control(move || bool_switch(context, "sidebarAutoSettleKeepPullRequests", &["sidebarAutoSettleKeepPullRequests"], "Keep threads with a PR"));
                            row.title = Text::from("Keep threads with a PR");
                            row.id = "";
                            settings_row(row)
                        })),
                )
            }))
            .child(settings_section(
                "text-generation",
                t("Text generation"),
                view()
                    .child(settings_row(
                        SettingsRowProps::searchable("text-generation-model")
                            .first()
                            .server(&["textGenerationModelSelection"])
                            .description(|| t("For thread titles and other generated text. Source control has its own setting. Text generators from Providers also appear here."))
                            .reset(RowReset::new(
                                "text generation model",
                                move || has_server_targets() && value(context, "textGenerationModelSelection") != default_unified_settings()["textGenerationModelSelection"],
                                move || context.update(json!({ "textGenerationModelSelection": default_unified_settings()["textGenerationModelSelection"] })),
                            ))
                            .control(move || {
                                // The model and traits pickers are not ported yet; with no target the
                                // web shows this sentence instead of them.
                                dynamic(move || {
                                    (!has_server_targets()).then(|| {
                                        text(t("Connect an environment to choose its text generation model."))
                                            .app_class(tw("text-sm text-muted-foreground"))
                                    })
                                })
                            }),
                    ))
                    .child(advanced_settings(
                        &["thread-title-instructions", "response-streaming"],
                        settings_row(
                            SettingsRowProps::searchable("response-streaming")
                                .first()
                                .server(&["responseStreamingMode"])
                                .description(move || {
                                    if context.mixed(&["responseStreamingMode"]) {
                                        t("The selected targets use different streaming modes.")
                                    } else {
                                        t(response_streaming_description(value(context, "responseStreamingMode").as_str().unwrap_or("paragraph")))
                                    }
                                })
                                .reset(reset(context, "responseStreamingMode", "response streaming"))
                                .control(move || {
                                    settings_select(
                                        "Response streaming",
                                        "w-full sm:w-56",
                                        RESPONSE_STREAMING_MODE_LABELS.iter().map(|(v, l)| (*v, (*l).to_owned())).collect(),
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
                        ),
                    )),
            ))
    )
}
