use serde_json::{Value, json};
use t3_orchestration::{
    ports::transport::assistant_streaming::*,
    provider::{ProviderEvent, ProviderEventKind},
};
fn snapshot(id: &str, text: &str, streaming: bool) -> ProviderEvent {
    ProviderEvent {driver:serde_json::from_value(json!("codex")).expect("test value"),kind:ProviderEventKind::Message(Box::new(serde_json::from_value(json!({"id":id,"threadId":"thread","runId":null,"nodeId":null,"role":"assistant","text":text,"attachments":[],"streaming":streaming,"createdBy":"agent","creationSource":"provider","createdAt":"2026-09-08T00:00:00.000Z","updatedAt":"2026-09-08T00:00:00.000Z"})).expect("test value")))}
}
fn shown(event: Option<ProviderEvent>) -> Value {
    match event.map(|e| e.kind) {
        Some(ProviderEventKind::Message(message)) => message.text.into(),
        _ => Value::Null,
    }
}
#[test]
fn source_paragraph_streaming_scenarios() {
    let scenarios: Value = serde_json::from_str(FIXTURES).unwrap();
    for scenario in scenarios.as_array().unwrap() {
        let name = scenario["name"].as_str().unwrap();
        let mut filter = AssistantStreamingFilter::new(scenario["mode"] == "turn", 800);
        for step in scenario["steps"].as_array().unwrap() {
            if step.get("dueAt").is_some() {
                assert_eq!(json!(filter.next_idle_flush_at()), step["dueAt"], "{name}");
            } else if let Some(now) = step["idle"].as_i64() {
                let shown: Vec<_> = filter
                    .take_idle_flushes(now)
                    .into_iter()
                    .map(|e| shown(Some(e)))
                    .collect();
                assert_eq!(json!(shown), step["shows"], "{name}");
            } else {
                let final_text = step["final"].as_str();
                let text = final_text.or_else(|| step["text"].as_str()).unwrap();
                let event = filter.filter(
                    snapshot(
                        step["id"].as_str().unwrap_or("m"),
                        text,
                        final_text.is_none(),
                    ),
                    step["at"].as_i64().unwrap(),
                );
                assert_eq!(
                    shown(event),
                    final_text.map_or_else(|| step["shows"].clone(), |s| json!(s)),
                    "{name}"
                );
            }
        }
    }
}
#[test]
fn markdown_boundaries_keep_fences_and_titles_with_their_content() {
    for (text, ready, rest) in [
        ("Intro\n\n```ts\nx()\n\n", "Intro\n\n", "```ts\nx()\n\n"),
        ("```ts\nx()\n```\nrest", "```ts\nx()\n```\n", "rest"),
        ("Intro\n\n## Results\n\n", "Intro\n\n", "## Results\n\n"),
        (
            "**Results**\n\nBody\n\nNext",
            "**Results**\n\nBody\n\n",
            "Next",
        ),
    ] {
        assert_eq!(split_buffered_assistant_text(text), (ready, rest));
    }
}
#[test]
fn incremental_fence_scanning_and_final_message_delivery() {
    let mut filter = AssistantStreamingFilter::new(false, 800);
    let mut text = "Intro\n\n```ts\n".to_owned();
    for line in 0..400 {
        text.push_str(&format!("const value{line} = {line};\n"));
        filter.filter(snapshot("m", &text, true), line * 400);
    }
    text.push_str("```\n");
    assert_eq!(
        shown(filter.filter(snapshot("m", &text, true), 400 * 400)),
        json!(text)
    );
}

const FIXTURES: &str = r##"[{"name":"an unfinished code fence","steps":[{"at":0,"text":"A\n\n```ts\nx()","shows":"A\n\n"},{"dueAt":800},{"idle":799,"shows":[]},{"idle":800,"shows":["A\n\n```ts\nx()"]},{"dueAt":null},{"at":1300,"text":"A\n\n```ts\nx()\n```\n\nB","shows":"A\n\n```ts\nx()\n```\n\n"}]},{"name":"a list's partial last item","steps":[{"at":0,"text":"Steps:\n\n- one\n- tw","shows":"Steps:\n\n- one\n"},{"idle":800,"shows":["Steps:\n\n- one\n- tw"]},{"at":1300,"text":"Steps:\n\n- one\n- two\n- th","shows":"Steps:\n\n- one\n- two\n"}]},{"name":"an incomplete table row","steps":[{"at":0,"text":"Intro.\n\n| a | b |\n|---|---|\n| 1 |","shows":"Intro.\n\n"},{"idle":800,"shows":["Intro.\n\n| a | b |\n|---|---|\n| 1 |"]},{"at":1300,"text":"Intro.\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\nAfter","shows":"Intro.\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n"}]},{"name":"more of the same paragraph after an idle delivery","steps":[{"at":0,"text":"A\n\nB","shows":"A\n\n"},{"idle":800,"shows":["A\n\nB"]},{"at":900,"text":"A\n\nB more","shows":null},{"idle":1699,"shows":[]},{"idle":1700,"shows":["A\n\nB more"]},{"idle":5000,"shows":[]},{"at":5100,"text":"A\n\nB more and","shows":null},{"at":5200,"text":"A\n\nB more and.\n\nC","shows":"A\n\nB more and.\n\n"},{"at":5300,"final":"A\n\nB more and.\n\nC."},{"idle":10000,"shows":[]}]},{"name":"completion at the idle deadline","steps":[{"at":0,"text":"A\n\nB","shows":"A\n\n"},{"at":800,"final":"A\n\nB."},{"idle":800,"shows":[]}]},{"name":"completion right after an idle delivery at the same instant","steps":[{"at":0,"text":"A\n\nB","shows":"A\n\n"},{"idle":800,"shows":["A\n\nB"]},{"at":800,"final":"A\n\nB."},{"idle":5000,"shows":[]}]},{"name":"several fast paragraphs inside the throttle","steps":[{"at":0,"text":"A\n\nB","shows":"A\n\n"},{"at":100,"text":"A\n\nB\n\nC","shows":null},{"at":200,"text":"A\n\nB\n\nC\n\nD","shows":null},{"at":300,"text":"A\n\nB\n\nC\n\nD\n\nE","shows":null},{"idle":1099,"shows":[]},{"idle":1100,"shows":["A\n\nB\n\nC\n\nD\n\nE"]}]},{"name":"two messages due at different times","steps":[{"id":"a","at":0,"text":"A\n\nx","shows":"A\n\n"},{"id":"b","at":300,"text":"B\n\ny","shows":"B\n\n"},{"dueAt":800},{"idle":800,"shows":["A\n\nx"]},{"dueAt":1100},{"idle":1100,"shows":["B\n\ny"]}]},{"name":"empty text","steps":[{"at":0,"text":"","shows":null},{"dueAt":null}]},{"name":"whitespace-only text","steps":[{"at":0,"text":" \n","shows":null},{"idle":800,"shows":[" \n"]},{"idle":5000,"shows":[]}]},{"name":"provider rewrites and truncation","steps":[{"at":0,"text":"Original long paragraph.\n\n","shows":"Original long paragraph.\n\n"},{"at":1000,"text":"New.\n\n","shows":"New.\n\n"},{"at":1100,"text":"Old.\n\n","shows":null},{"idle":1900,"shows":["Old.\n\n"]},{"at":2500,"text":"Old","shows":null},{"idle":3300,"shows":["Old"]},{"at":4000,"text":"","shows":null},{"idle":4800,"shows":[""]}]},{"name":"a rewrite that drops an open code fence","steps":[{"at":0,"text":"Intro\n\n```ts\nx()\n","shows":"Intro\n\n"},{"at":400,"text":"Retry\n\n- a\n- b","shows":"Retry\n\n- a\n"}]},{"name":"turn mode","mode":"turn","steps":[{"at":0,"text":"A\n\nB","shows":null},{"dueAt":null}]}]"##;
