use super::*;

#[test]
fn theme_metadata_lifecycle_and_summaries() {
    for (name, action, detail, summary) in [
        (
            "t3_theme_read",
            "theme-read",
            "themes",
            "Checked themes 2 times",
        ),
        (
            "t3_theme_set",
            "theme-set",
            "the theme",
            "Set the theme 2 times",
        ),
    ] {
        let entry: WorkLogEntry = serde_json::from_value(json!({"id":"theme", "createdAt":"2026-01-01T00:00:00Z", "runId":null, "tone":"tool", "label":name,"toolLifecycleStatus":"completed"})).unwrap();
        let presentation =
            resolve_work_entry_tool_presentation(&entry, None).expect("theme metadata");
        assert_eq!(summary_action(&entry), Some(action));
        assert_eq!(presentation.action, None);
        assert_eq!(
            presentation.display_name,
            if action == "theme-read" {
                format!("Read {detail}")
            } else {
                format!("Set {detail}")
            }
        );
        let calls = vec![
            T3ToolSummaryCall {
                input: Value::Null,
                output: Value::Null,
                outcome: "completed".into()
            };
            2
        ];
        assert_eq!(summarize_t3_tool_calls(action, &calls).label, summary);
    }
    assert_eq!(action_priority("theme-set"), 0);
}
