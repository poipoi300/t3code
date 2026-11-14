use super::super::messages_timeline::{MessagesTimelineRowsInput, derive_messages_timeline_rows};
use super::*;
use crate::view_logic::session::{TimelineContent, TimelineEntry};
use serde_json::json;

fn message(extra: Value) -> ChatMessage {
    let mut value = json!({"id":"message", "role":"user", "text":"hello", "runId":null, "streaming":false,
        "createdAt":"2026-11-13T12:00:00.000Z", "updatedAt":"2026-11-13T12:00:00.000Z"});
    value
        .as_object_mut()
        .expect("object")
        .extend(extra.as_object().expect("object").clone());
    serde_json::from_value(value).expect("chat message")
}
fn file() -> Value {
    json!({"type":"file","id":"report","name":"report.pdf","mimeType":"application/pdf","sizeBytes":1024})
}
fn record() -> Value {
    json!({"version":1,"kind":"file","contextId":"report","label":"report.pdf","attachmentId":"report","name":"report.pdf","mimeType":"application/pdf","sizeBytes":1024})
}
const REFERENCE: &str = "[report.pdf](t3-context://v1/file/report)";

#[test]
fn groups_only_adjacent_own_messages() {
    let own = message(json!({}));
    assert_eq!(
        resolve_user_message_grouping(Some(&own), Some(&own), Some(&own)),
        Grouping {
            grouped_with_previous: true,
            grouped_with_next: true,
            continues_steer: false
        }
    );
    assert!(!resolve_user_message_grouping(Some(&own), None, Some(&own)).grouped_with_previous);
    let boundaries = [
        None,
        Some(message(json!({"role":"assistant"}))),
        Some(message(json!({"createdBy":"agent"}))),
        Some(message(
            json!({"createdBy":"agent","creationSource":"server"}),
        )),
        Some(message(json!({"scheduledTaskId":"task"}))),
        Some(message(
            json!({"context":{"guest":{"guestMessageId":"guest","name":"Sam","text":"hi"}}}),
        )),
    ];
    for boundary in &boundaries {
        assert_eq!(
            resolve_user_message_grouping(Some(&own), boundary.as_ref(), boundary.as_ref()),
            Grouping::default()
        );
        assert_eq!(
            resolve_user_message_grouping(boundary.as_ref(), Some(&own), Some(&own)),
            Grouping::default()
        );
    }
}
#[test]
fn shows_one_label_for_consecutive_steers_including_promoted_queued_messages() {
    let own = message(json!({}));
    for intent in ["steer", "promoted_queued_to_steer"] {
        let steer = message(json!({"inputIntent":intent}));
        for previous in ["steer", "promoted_queued_to_steer"] {
            assert!(
                resolve_user_message_grouping(
                    Some(&steer),
                    Some(&message(json!({"inputIntent":previous}))),
                    None
                )
                .continues_steer
            );
        }
        assert!(!resolve_user_message_grouping(Some(&steer), Some(&own), None).continues_steer);
        assert!(!resolve_user_message_grouping(Some(&own), Some(&steer), None).continues_steer);
    }
}
#[test]
fn shows_file_cards_for_bare_attachments_and_bound_file_references() {
    for text in [
        String::new(),
        " \n".into(),
        REFERENCE.into(),
        format!("{REFERENCE}\n{REFERENCE}"),
    ] {
        let message = message(
            json!({"text":text,"attachments":[file()],"context":{"version":1,"records":[record()]}}),
        );
        assert_eq!(
            resolve_user_message_file_cards(&message)
                .expect("cards")
                .len(),
            1
        );
    }
}
#[test]
fn keeps_words_and_unavailable_or_non_file_references_in_the_bubble() {
    for text in [
        format!("read {REFERENCE}"),
        "[missing.pdf](t3-context://v1/file/missing)".into(),
        "[report.pdf](t3-context://v1/mention/report)".into(),
    ] {
        assert!(resolve_user_message_file_cards(&message(json!({"text":text,"attachments":[file()],"context":{"version":1,"records":[record()]}}))).is_none());
    }
    assert!(
        resolve_user_message_file_cards(&message(json!({"text":REFERENCE,"attachments":[file()]})))
            .is_none()
    );
    let mut other = file();
    other["id"] = json!("other");
    for record in [
        record(),
        json!({"version":1,"kind":"file","contextId":"report","label":"report.pdf","payload":{}}),
    ] {
        assert!(resolve_user_message_file_cards(&message(json!({"text":REFERENCE,"attachments":[other.clone()],"context":{"version":1,"records":[record]}}))).is_none());
    }
    assert!(resolve_user_message_file_cards(&message(json!({"text":REFERENCE,"attachments":[file()],"context":{"version":1,"records":[{"version":1,"kind":"file","contextId":"report","label":"report.pdf","payload":{}}]}}))).is_none());
}
#[test]
fn keeps_media_unknown_attachments_and_empty_messages_out_of_the_file_cards() {
    assert!(
        resolve_user_message_file_cards(&message(json!({"text":"","attachments":[]}))).is_none()
    );
    for (kind, name, mime) in [
        ("image", "photo.png", "image/png"),
        ("file", "photo.png", "application/octet-stream"),
        ("file", "clip.mp4", "video/mp4"),
        ("file", "clip.mov", "application/octet-stream"),
        ("file", "voice.mp3", "audio/mpeg"),
        ("future", "report", "application/pdf"),
    ] {
        let attachment =
            json!({"type":kind,"id":"media","name":name,"mimeType":mime,"sizeBytes":2});
        for attachments in [vec![attachment.clone()], vec![file(), attachment]] {
            assert!(
                resolve_user_message_file_cards(&message(
                    json!({"text":"","attachments":attachments})
                ))
                .is_none()
            );
        }
    }
}
#[test]
fn preserves_user_written_and_assistant_quoted_schedule_headers() {
    let text = "[Triggered by schedule task: Daily audit]\n\nCheck for crashes.\n";
    for extra in [
        json!({"createdBy":"user"}),
        json!({}),
        json!({"role":"assistant","createdBy":"agent"}),
    ] {
        let mut message = message(extra);
        message.text = text.into();
        let p = resolve_user_message_presentation(&message);
        assert_eq!(p.text, text);
        assert_eq!(p.attribution, Attribution::Own);
        assert!(p.scheduled_task_id.is_none());
    }
}
#[test]
fn hides_the_prompt_that_stands_in_for_an_attachment_only_message() {
    for text in [
        ATTACHMENT_ONLY_BOOTSTRAP_PROMPT.into(),
        format!("Ultrathink:\n{ATTACHMENT_ONLY_BOOTSTRAP_PROMPT}"),
    ] {
        let p = resolve_user_message_presentation(&message(json!({"text":text})));
        assert!(p.text.is_empty());
        assert_eq!(p.attribution, Attribution::Own);
    }
}
#[test]
fn shows_a_guests_own_words_and_name_not_the_agents_attributed_text() {
    let p = resolve_user_message_presentation(&message(
        json!({"text":"Message from guest Sam, sent by the owner:\n\nWhy?","createdBy":"user","context":{"guest":{"guestMessageId":"guest-message:one","name":"Sam","text":"Why?"}}}),
    ));
    assert_eq!(p.text, "Why?");
    assert_eq!(p.attribution, Attribution::Guest("Sam".into()));
    assert!(p.scheduled_task_id.is_none());
}
#[test]
fn attributes_older_server_sent_restart_continuations_to_t3_code() {
    for (source, attribution) in [
        ("server", Attribution::T3Code),
        ("mcp", Attribution::Agent),
        ("provider", Attribution::Agent),
    ] {
        assert_eq!(resolve_user_message_presentation(&message(json!({"createdBy":"agent","creationSource":source,"text":"Note: the T3 server restarted."}))).attribution,attribution);
    }
}
#[test]
fn recovers_the_triggering_automation_from_older_scheduler_message_ids() {
    for trigger in ["scheduled", "manual"] {
        let p = resolve_user_message_presentation(&message(
            json!({"id":format!("scheduled-task-message:task:daily-audit:1788661140000:{trigger}"),"createdBy":"user","text":"[Triggered by schedule task: Daily audit]\n\nCheck for crashes.\n"}),
        ));
        assert_eq!(p.text, "Check for crashes.\n");
        assert_eq!(p.attribution, Attribution::Automation);
        assert_eq!(p.scheduled_task_id.as_deref(), Some("task:daily-audit"));
    }
}

struct Clock;
impl TimestampHost for Clock {
    fn parse_epoch_ms(&self, iso: &str) -> Option<i64> {
        crate::time::epoch_ms(iso)
    }
    fn calendar(
        &self,
        epoch_ms: i64,
        _: Option<&str>,
    ) -> crate::view_logic::session::timestamps::CalendarTimestamp {
        crate::view_logic::session::timestamps::CalendarTimestamp {
            epoch_ms,
            day_index: 0,
            year: 2026,
            month: 11,
            day: 13,
        }
    }
    fn short_time(&self, _: i64, _: &TimestampFormat, _: Option<&str>) -> String {
        "12:00".into()
    }
    fn numeric_date(&self, _: i64, _: bool, _: Option<&str>) -> String {
        "11/13".into()
    }
    fn full_timestamp(&self, iso: &str, _: &TimestampFormat) -> String {
        iso.into()
    }
    fn locale_hour12(&self) -> bool {
        false
    }
}
fn row(message: ChatMessage) -> UserRow {
    let entries = vec![Rc::new(TimelineEntry {
        id: message_id(&message),
        created_at: message.created_at.clone(),
        attempt: None,
        content: TimelineContent::Message {
            message: Rc::new(message),
            projected_item: None,
        },
    })];
    let rows = derive_messages_timeline_rows(&MessagesTimelineRowsInput {
        timeline_entries: entries,
        ..Default::default()
    });
    rows.iter()
        .find_map(|r| present_user_message_row(r, Clock, &TimestampFormat::Locale, 0))
        .expect("user row")
}
#[test]
fn derives_media_only_media_above_text_file_cards_and_audio_from_projected_messages() {
    for (text, media_only, above) in [("", true, false), ("Look at this", false, true)] {
        let p = row(message(
            json!({"text":text,"attachments":[{"type":"image","id":"photo","name":"photo.png","mimeType":"image/png","sizeBytes":2}]}),
        ));
        assert_eq!(p.media_only, media_only);
        assert_eq!(p.media_above_text, above);
        assert_eq!(p.images.len(), 1);
    }
    assert!(row(message(json!({"text":REFERENCE,"attachments":[file()],"context":{"version":1,"records":[record()]}}))).file_cards_only);
    assert!(row(message(json!({"text":"","attachments":[{"type":"file","id":"voice","name":"voice.mp3","mimeType":"audio/mpeg","sizeBytes":2}]}))).audio_only);
}
