//! Common controls for the settings pages' wire-keyed rows.

use nsh_core::prelude::*;
use serde_json::{Value, json};
use t3_client::services::settings::panels::default_unified_settings;

use super::shell::{RowReset, SettingsContext, SettingsRowProps, settings_select, use_settings};
use crate::i18n::Text;
use crate::ui::switch::{ControlState, Size, switch};

pub(super) fn reset(context: SettingsContext, key: &'static str, label: &'static str) -> RowReset {
    RowReset::new(
        label,
        move || context.mixed(&[key]) || context.setting(key) != default_unified_settings()[key],
        move || context.update(json!({ key: default_unified_settings()[key] })),
    )
}

pub(super) fn switch_row(
    id: &'static str,
    key: &'static str,
    reset_label: &'static str,
    label: &'static str,
    description: &'static str,
) -> SettingsRowProps {
    let context = use_settings();
    SettingsRowProps::searchable(id)
        .description(move || Text::from(description))
        .reset(reset(context, key, reset_label))
        .control(move || {
            switch(
                label,
                ControlState {
                    checked: Prop::bind(move || context.setting(key).as_bool().unwrap_or_default()),
                    ..Default::default()
                },
                Size::Default,
                move |checked| context.update(json!({ key: checked })),
            )
        })
}

pub(super) fn choices(items: &[(&'static str, &'static str)]) -> Vec<(&'static str, Text)> {
    items
        .iter()
        .map(|(id, title)| (*id, Text::from(*title)))
        .collect()
}

pub(super) fn choice_row(
    id: &'static str,
    key: &'static str,
    reset_label: &'static str,
    label: &'static str,
    description: &'static str,
    options: &'static [(&'static str, &'static str)],
) -> SettingsRowProps {
    let context = use_settings();
    SettingsRowProps::searchable(id)
        .description(move || Text::from(description))
        .reset(reset(context, key, reset_label))
        .control(move || {
            settings_select(
                label,
                "w-full sm:w-40",
                choices(options),
                move || match context.setting(key) {
                    Value::String(value) => value,
                    value => value.to_string(),
                },
                || false,
                move |value| {
                    let value = match value.as_str() {
                        "true" => Value::Bool(true),
                        "false" => Value::Bool(false),
                        _ => Value::String(value),
                    };
                    context.update(json!({ key: value }));
                },
            )
        })
}
