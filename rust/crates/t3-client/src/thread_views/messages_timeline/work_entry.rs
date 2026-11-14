//! Port of TimelineWorkEntryRow.tsx plain-output policy and shared/toolActivity.ts Skill arguments.

use crate::view_logic::{JsTrim, session::WorkLogEntry};
use t3_contracts::orchestration_v2::OrchestrationV2TurnItem;

pub fn action(entry: &WorkLogEntry) -> &'static str {
    super::helpers::tool_action(entry)
}

pub fn trim(value: &str) -> &str {
    value.js_trim()
}

/// None selects the inspector; Some(None) is a read/skill with no plain body.
pub fn plain_output(entry: &WorkLogEntry, workspace_root: Option<&str>) -> Option<Option<String>> {
    if action(entry) == "read" {
        return Some(super::work_entry_read_output(entry, workspace_root));
    }
    let OrchestrationV2TurnItem::DynamicTool(tool) = entry.structured_payload.as_deref()? else {
        return None;
    };
    skill_args(
        tool.tool_name.as_ref().map(|name| name.0.as_str()),
        &tool.input,
    )
}

pub fn has_detail(entry: &WorkLogEntry) -> bool {
    let Some(projected) = &entry.projected_item else {
        return false;
    };
    // Streaming command output must not be serialized just to check its nonempty input.
    if let OrchestrationV2TurnItem::CommandExecution(command) = &projected.item
        && !command.input.js_trim().is_empty()
    {
        return true;
    }
    crate::work_log::turn_item_has_detail(&projected.item)
}

pub fn is_javascript_repl_tool(name: Option<&str>) -> bool {
    matches!(name, Some("js" | "cua_repl.js" | "node_repl.js"))
}

fn skill_args(name: Option<&str>, input: &serde_json::Value) -> Option<Option<String>> {
    if name != Some("Skill")
        || input
            .get("skill")
            .and_then(serde_json::Value::as_str)
            .is_none_or(|name| name.js_trim().is_empty())
    {
        return None;
    }
    Some(
        input
            .get("args")
            .and_then(serde_json::Value::as_str)
            .map(str::js_trim)
            .filter(|args| !args.is_empty())
            .map(str::to_owned),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn skill_distinguishes_missing_invocation_from_invocation_without_args() {
        assert_eq!(skill_args(None, &json!({"skill":"demo"})), None);
        assert_eq!(skill_args(Some("skill"), &json!({"skill":"demo"})), None);
        assert_eq!(
            skill_args(Some("Skill"), &json!({"skill":"\u{feff}"})),
            None
        );
        assert_eq!(
            skill_args(Some("Skill"), &json!({"skill":"demo"})),
            Some(None)
        );
        assert_eq!(
            skill_args(
                Some("Skill"),
                &json!({"skill":"demo", "args":"  hello \u{feff}"})
            ),
            Some(Some("hello".into()))
        );
    }

    #[test]
    fn current_c3_read_classification_selects_the_canonical_read_output() {
        let entry = WorkLogEntry {
            tool_data: Some(json!({"kind":"read","path":"./package.json"})),
            ..Default::default()
        };
        assert_eq!(action(&entry), "read");
        assert_eq!(
            plain_output(&entry, Some("C:/repo")),
            Some(Some("C:/repo/package.json".into()))
        );
        assert_eq!(plain_output(&WorkLogEntry::default(), None), None);
    }

    #[test]
    fn trimming_matches_javascript_bom_and_next_line_behavior() {
        assert_eq!(trim("\u{feff}value\u{feff}"), "value");
        assert_eq!(trim("\u{85}value\u{85}"), "\u{85}value\u{85}");
    }
}
