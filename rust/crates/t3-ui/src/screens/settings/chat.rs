//! Port of apps/web/src/components/settings/SettingsPanels.tsx ChatSettingsPanel,
//! ToolCallVisibilityRow, ScreenReaderAnnouncementsRow and ScreenReaderWindowTitleRow (route
//! apps/web/src/routes/_chat.settings.chat.tsx).

use nsh_core::prelude::*;
use serde_json::{Value, json};
use t3_client::services::settings::panels::default_unified_settings;

use super::shell::{
    RowReset, SettingsContext, SettingsRowProps, advanced_settings, settings_page_container,
    settings_row, settings_section, settings_select, use_settings,
};
use crate::i18n::{Text, t, tr};
use crate::theme::tw;
use crate::ui::switch::{ControlState, Size as SwitchSize, switch};

pub fn screen() -> AnyElement {
    chat_settings_panel().into_element()
}

fn value(context: SettingsContext, key: &'static str) -> Value {
    context.setting(key)
}

fn text_value(context: SettingsContext, key: &'static str) -> String {
    value(context, key).as_str().unwrap_or_default().to_owned()
}

fn reset(context: SettingsContext, key: &'static str, label: &'static str) -> RowReset {
    RowReset::new(
        label,
        move || value(context, key) != default_unified_settings()[key],
        move || context.update(json!({ key: default_unified_settings()[key] })),
    )
}

fn setting_switch(
    context: SettingsContext,
    key: &'static str,
    label: &'static str,
) -> impl IntoElement {
    switch(
        label,
        ControlState {
            checked: Prop::bind(move || value(context, key).as_bool().unwrap_or(false)),
            ..Default::default()
        },
        SwitchSize::Default,
        move |checked| context.update(json!({ key: checked })),
    )
}

/// The modifier the send shortcuts name (`isMacPlatform(navigator.platform) ? "⌘" : "Ctrl"`).
fn modifier_label() -> &'static str {
    if crate::host::is_mac() { "⌘" } else { "Ctrl" }
}

fn send_shortcut_options() -> Vec<(&'static str, String)> {
    let modifier = t3_i18n::Value::Text(modifier_label().into());
    vec![
        ("enter", t("Enter")),
        (
            "mod-enter-multiline",
            tr(
                "{modifier} + Enter for multiline prompts",
                &[("modifier", modifier.clone())],
            ),
        ),
        (
            "mod-enter",
            tr("{modifier} + Enter always", &[("modifier", modifier)]),
        ),
    ]
}

/// The follow-up row's description, which names the opposite-action shortcut.
fn follow_up_description(send_shortcut: &str) -> String {
    let modifier = t3_i18n::Value::Text(modifier_label().into());
    let lead = t("Queue follow-ups while the agent runs or steer the current run.");
    let detail = if send_shortcut == "mod-enter-multiline" {
        tr(
            "Press {modifier} + Enter for single-line prompts or {modifier} + Shift + Enter for multiline prompts to do the opposite for one message.",
            &[("modifier", modifier)],
        )
    } else if send_shortcut == "mod-enter" {
        tr(
            "Press {modifier} + Shift + Enter to do the opposite for one message.",
            &[("modifier", modifier)],
        )
    } else {
        tr(
            "Press {modifier} + Enter to do the opposite for one message.",
            &[("modifier", modifier)],
        )
    };
    format!("{lead} {detail}")
}

const TOOL_CALL_VISIBILITY_LABELS: [(&str, &str); 4] = [
    ("showAll", "Show all"),
    ("hideSuccessful", "Hide successful"),
    ("hideAll", "Hide all"),
    ("zen", "Zen"),
];

const SCREEN_READER_ANNOUNCEMENTS_LABELS: [(&str, &str); 3] = [
    ("status", "Status only"),
    ("messages", "Status and messages"),
    ("everything", "Everything"),
];

fn labels(table: &[(&'static str, &'static str)]) -> Vec<(&'static str, String)> {
    table.iter().map(|(v, l)| (*v, (*l).to_owned())).collect()
}

/// `ChatSettingsPanel`.
pub fn chat_settings_panel() -> impl IntoElement {
    let context = use_settings();
    settings_page_container(
        view()
            .app_class(tw("flex flex-col gap-8"))
            .child(settings_section(
                "composer",
                Text::from("Composer"),
                view()
                    .child(settings_row(
                        SettingsRowProps::searchable("composer-rich-text")
                            .first()
                            .description(|| Text::from("Show formatted Markdown as you type."))
                            .reset(reset(context, "composerRichTextEnabled", "rich text composer"))
                            .control(move || setting_switch(context, "composerRichTextEnabled", "Rich text composer")),
                    ))
                    .child(settings_row(
                        SettingsRowProps::searchable("send-shortcut")
                            .description(|| Text::from("Choose when Enter sends a prompt or inserts a new line"))
                            .reset(reset(context, "sendShortcut", "send shortcut"))
                            .control(move || {
                                settings_select(
                                    "Send shortcut",
                                    "w-auto min-w-0 max-w-full",
                                    send_shortcut_options(),
                                    move || text_value(context, "sendShortcut"),
                                    || false,
                                    move |shortcut| {
                                        if send_shortcut_options().iter().any(|(v, _)| *v == shortcut) {
                                            context.update(json!({ "sendShortcut": shortcut }));
                                        }
                                    },
                                )
                            }),
                    ))
                    .child(advanced_settings(
                        &["skills-in-slash-menu"],
                        settings_row(
                            SettingsRowProps::searchable("skills-in-slash-menu")
                                .first()
                                .description(|| Text::from("Also include skills in the / command menu. Skills always appear when you type $."))
                                .reset(reset(context, "showSkillsInSlashMenu", "skills in slash menu"))
                                .control(move || setting_switch(context, "showSkillsInSlashMenu", "Show skills in slash menu")),
                        ),
                    )),
            ))
            .child(settings_section(
                "follow-ups",
                Text::from("Follow-ups"),
                settings_row(
                    SettingsRowProps::searchable("follow-up-behavior")
                        .first()
                        .description(move || Text::content(follow_up_description(&text_value(context, "sendShortcut"))))
                        .reset(reset(context, "followUpBehavior", "follow-up behavior"))
                        .control(move || {
                            settings_select(
                                "Follow-up behavior",
                                "w-auto min-w-0",
                                vec![("queue", "Queue".to_owned()), ("steer", "Steer".to_owned())],
                                move || text_value(context, "followUpBehavior"),
                                || false,
                                move |behavior| context.update(json!({ "followUpBehavior": behavior })),
                            )
                        }),
                ),
            ))
            .child(settings_section(
                "activity",
                Text::from("Activity"),
                settings_row(
                    SettingsRowProps::searchable("tool-call-visibility")
                        .first()
                        .description(|| Text::from("Hide the commands, edits, and searches agents run. Hide successful keeps failures. Zen shows only final answers. Approvals, questions, and errors always show."))
                        .reset(reset(context, "toolCallVisibility", "tool calls in chat"))
                        .control(move || {
                            settings_select(
                                "Tool calls in chat",
                                "w-full sm:w-40",
                                labels(&TOOL_CALL_VISIBILITY_LABELS),
                                move || text_value(context, "toolCallVisibility"),
                                || false,
                                move |visibility| context.update(json!({ "toolCallVisibility": visibility })),
                            )
                        }),
                ),
            ))
            .child(settings_section(
                "screen-reader",
                Text::from("Screen reader"),
                view()
                    .child(settings_row(
                        SettingsRowProps::searchable("screen-reader-announcements")
                            .first()
                            .description(|| Text::from("What a screen reader hears from the open thread: a turn starting, needing you, and ending. Status and messages adds what the agent writes before its last message. Everything adds each step."))
                            .reset(reset(context, "screenReaderAnnouncements", "screen reader announcements"))
                            .control(move || {
                                settings_select(
                                    "Screen reader announcements",
                                    "w-full sm:w-48",
                                    labels(&SCREEN_READER_ANNOUNCEMENTS_LABELS),
                                    move || text_value(context, "screenReaderAnnouncements"),
                                    || false,
                                    move |mode| context.update(json!({ "screenReaderAnnouncements": mode })),
                                )
                            }),
                    ))
                    .child(settings_row(
                        SettingsRowProps::searchable("screen-reader-window-title")
                            .description(|| Text::from("Start the window title with the thread's status, like Needs approval, and its name, so screen readers read them first when you switch apps. Taskbars, shared screens, and time trackers see it too."))
                            .reset(reset(context, "screenReaderWindowTitle", "screen reader window title"))
                            .control(move || setting_switch(context, "screenReaderWindowTitle", "Screen reader window title")),
                    )),
            )),
    )
}
