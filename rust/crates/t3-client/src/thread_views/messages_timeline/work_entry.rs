//! Port of TimelineWorkEntryRow.tsx plain-output policy and shared/toolActivity.ts Skill arguments.

use crate::view_logic::{JsTrim, session::WorkLogEntry};
use t3_contracts::orchestration_v2::OrchestrationV2TurnItem;

pub fn action(entry: &WorkLogEntry) -> &'static str {
    super::helpers::tool_action(entry)
}

pub fn thread_read_target_id(entry: &WorkLogEntry) -> Option<String> {
    let OrchestrationV2TurnItem::DynamicTool(tool) = entry.structured_payload.as_deref()? else {
        return None;
    };
    if super::presentation::tool_id(tool.tool_name.as_ref()?.0.as_str()) != Some("t3_thread_read") {
        return None;
    }
    tool.input
        .get("threadId")?
        .as_str()
        .map(str::js_trim)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
}

/// threadExecution.ts: the wire background_task/subagent shape decodes to semantic subagent.
pub fn notification_child_thread_id(
    source: &t3_contracts::orchestration_v2::OrchestrationV2NotificationSource,
) -> Option<&t3_contracts::base_schemas::ThreadId> {
    if let t3_contracts::orchestration_v2::OrchestrationV2NotificationSource::Member2(source) =
        source
    {
        return source.child_thread_id.as_ref().and_then(Option::as_ref);
    }
    crate::thread_views::threads::execution_presentation::notification_child_thread_id(source)
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

/// Port of shared/favicon.ts toolActivityFaviconUrl and TimelineToolActivityIcon's themed-logo selection.
pub fn tool_icon_url(
    icon: &t3_contracts::provider_runtime::ToolActivityIcon,
    dark: bool,
) -> Option<String> {
    use t3_contracts::provider_runtime::ToolActivityIcon;
    match icon {
        ToolActivityIcon::ThemedLogo(icon) => Some(
            if dark {
                icon.logo_url_dark
                    .as_ref()
                    .and_then(Option::as_ref)
                    .unwrap_or(&icon.logo_url)
            } else {
                &icon.logo_url
            }
            .clone(),
        ),
        ToolActivityIcon::NativeApp(_) => None,
        ToolActivityIcon::Website(icon) => {
            let explicit = |raw: Option<&str>| {
                raw.filter(|raw| raw.encode_utf16().take(4097).count() <= 4096)
                    .and_then(|raw| url::Url::parse(raw).ok())
                    .filter(|url| matches!(url.scheme(), "http" | "https" | "data"))
                    .map(|url| url.to_string())
            };
            let page = (icon.page_url.encode_utf16().take(4097).count() <= 4096)
                .then(|| url::Url::parse(&icon.page_url).ok())
                .flatten()
                .filter(|url| matches!(url.scheme(), "http" | "https"));
            let themed = || {
                page.as_ref()
                    .filter(|url| url.host_str() == Some("github.com"))
                    .map(|_| {
                        if dark {
                            "https://github.githubassets.com/favicons/favicon-dark.svg"
                        } else {
                            "https://github.githubassets.com/favicons/favicon.svg"
                        }
                        .to_owned()
                    })
            };
            let light = || explicit(icon.favicon_url.as_ref().and_then(Option::as_deref));
            let origin = || {
                page.as_ref()
                    .and_then(|url| url::Url::parse(&url.origin().ascii_serialization()).ok())
                    .and_then(|url| url.join("/favicon.ico").ok())
                    .map(|url| url.to_string())
            };
            if dark {
                explicit(icon.favicon_url_dark.as_ref().and_then(Option::as_deref))
                    .or_else(themed)
                    .or_else(light)
                    .or_else(origin)
            } else {
                light().or_else(themed).or_else(origin)
            }
        }
    }
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
    fn website_icons_follow_the_webs_theme_precedence_and_origin_fallback() {
        for (icon, dark, expected) in [
            (
                json!({"_tag":"website","pageUrl":"https://github.com/t3","faviconUrl":"https://custom/icon"}),
                true,
                Some("https://github.githubassets.com/favicons/favicon-dark.svg"),
            ),
            (
                json!({"_tag":"website","pageUrl":"https://github.com/t3","faviconUrl":"https://custom/icon"}),
                false,
                Some("https://custom/icon"),
            ),
            (
                json!({"_tag":"website","pageUrl":"https://example.com/path","faviconUrl":"file:///bad"}),
                false,
                Some("https://example.com/favicon.ico"),
            ),
            (
                json!({"_tag":"website","pageUrl":"https://user:password@example.com/path?query"}),
                false,
                Some("https://example.com/favicon.ico"),
            ),
            (
                json!({"_tag":"website","pageUrl":"file:///bad"}),
                false,
                None,
            ),
            (
                json!({"_tag":"themed-logo","logoUrl":"light","logoUrlDark":"dark"}),
                true,
                Some("dark"),
            ),
        ] {
            let icon = serde_json::from_value(icon).expect("tool icon");
            assert_eq!(tool_icon_url(&icon, dark).as_deref(), expected);
        }
    }

    #[test]
    fn notification_links_follow_the_decoded_semantic_source() {
        for (source, expected) in [
            (
                json!({"kind":"subagent","childThreadId":"child"}),
                Some("child"),
            ),
            (
                json!({"kind":"background_task","work":"subagent","childThreadId":"child"}),
                Some("child"),
            ),
            (
                json!({"kind":"delegated_task","taskIds":[],"childThreadId":"child"}),
                Some("child"),
            ),
            (
                json!({"kind":"background_task","childThreadId":"child"}),
                None,
            ),
            (json!({"kind":"system","childThreadId":"child"}), None),
        ] {
            let source = serde_json::from_value(source).expect("notification source");
            assert_eq!(
                notification_child_thread_id(&source).map(|id| id.0.as_str()),
                expected
            );
        }
    }

    #[test]
    fn trimming_matches_javascript_bom_and_next_line_behavior() {
        assert_eq!(trim("\u{feff}value\u{feff}"), "value");
        assert_eq!(trim("\u{85}value\u{85}"), "\u{85}value\u{85}");
    }
}
