#![allow(clippy::unwrap_used)]
use serde_json::json;
use t3_client::{services::config::timestamp_ms, thread_views::shelves::*};
use t3_contracts::settings::SnoozePresetSetting;
const DAY: i64 = 86_400_000;
fn at(value: &str) -> i64 {
    timestamp_ms(value).unwrap()
}
fn input() -> ShelfInput {
    ShelfInput {
        snoozed_at: Some(at("2026-04-10T09:00:00Z")),
        snoozed_until: Some(at("2026-04-11T09:00:00Z")),
        ..Default::default()
    }
}
#[test]
fn snoozed_questions_wait_for_the_timer_and_fresh_questions_approvals_and_failures_wake() {
    let now = at("2026-04-10T12:00:00Z");
    let mut shell = input();
    shell.snoozed_until = None;
    assert!(!effective_snoozed(&shell, now));
    shell = input();
    shell.questions = true;
    shell.question_at = shell.snoozed_at;
    assert!(can_snooze(&shell, now));
    assert!(effective_snoozed(&shell, now));
    assert_eq!(woke_at(&shell, now), None);
    assert!(!effective_snoozed(&shell, shell.snoozed_until.unwrap()));
    assert_eq!(
        woke_at(&shell, shell.snoozed_until.unwrap()),
        shell.snoozed_until
    );
    shell.question_at = Some(at("2026-04-10T10:00:00Z"));
    assert!(!effective_snoozed(&shell, now));
    assert_eq!(woke_at(&shell, now), shell.question_at);
    shell.question_at = None;
    assert!(!effective_snoozed(&shell, now));
    assert!(raised_hand_while_snoozed(&shell));
    shell = input();
    shell.approvals = true;
    assert!(!can_snooze(&shell, now));
    assert!(!effective_snoozed(&shell, now));
    shell = input();
    shell.subagent_request = true;
    assert!(!can_snooze(&shell, now));
    shell = input();
    shell.runtime = Some(RuntimeState::Running);
    assert!(effective_snoozed(&shell, now));
    shell.latest_run = Some(Run {
        state: Some(RunState::Completed),
        requested_at: Some(at("2026-04-10T09:00:00.001Z")),
        started_at: Some(at("2026-04-10T09:15:00Z")),
        completed_at: Some(at("2026-04-10T10:30:00Z")),
        ..Default::default()
    });
    assert!(effective_snoozed(&shell, now));
    assert_eq!(woke_at(&shell, now), None);
}
#[test]
fn wake_timestamps_use_the_actual_event_and_queued_turns_clear_on_adoption_or_failure() {
    let now = at("2026-04-10T12:00:00Z");
    let metadata = at("2026-04-10T11:00:00Z");
    let stale = at("2026-04-10T08:00:00Z");
    let fresh = at("2026-04-10T10:00:00Z");
    for (completed, approvals, questions, expected) in [
        (Some(stale), false, false, None),
        (None, false, false, None),
        (None, true, false, Some(metadata)),
        (Some(fresh), false, false, Some(fresh)),
        (Some(stale), true, false, Some(metadata)),
        (Some(stale), false, true, Some(metadata)),
    ] {
        let mut shell = input();
        shell.runtime = Some(RuntimeState::Failed);
        shell.runtime_updated_at = Some(metadata);
        shell.approvals = approvals;
        shell.questions = questions;
        shell.latest_run = Some(Run {
            state: Some(RunState::Failed),
            completed_at: completed,
            ..Default::default()
        });
        assert_eq!(effective_snoozed(&shell, now), expected.is_none());
        assert_eq!(woke_at(&shell, now), expected);
    }
    for (runtime, question, expected) in [
        (RuntimeState::Error, fresh, metadata),
        (RuntimeState::Failed, fresh, metadata),
        (
            RuntimeState::Failed,
            at("2026-04-10T11:30:00Z"),
            at("2026-04-10T11:30:00Z"),
        ),
        (RuntimeState::Running, fresh, fresh),
    ] {
        let mut shell = input();
        shell.runtime = Some(runtime);
        shell.runtime_updated_at = Some(metadata);
        shell.questions = true;
        shell.question_at = Some(question);
        assert_eq!(woke_at(&shell, now), Some(expected));
        assert_eq!(
            expected > at("2026-04-10T10:30:00Z"),
            runtime != RuntimeState::Running
        );
    }
    for subagent in [false, true] {
        let mut shell = input();
        shell.questions = true;
        shell.question_at = shell.snoozed_at;
        shell.runtime_updated_at = Some(metadata);
        shell.runtime = Some(if subagent {
            RuntimeState::Running
        } else {
            RuntimeState::Error
        });
        shell.subagent_request = subagent;
        assert_eq!(woke_at(&shell, now), Some(metadata));
    }
    let mut shell = input();
    shell.snoozed_until = Some(fresh);
    assert_eq!(woke_at(&shell, now), Some(fresh));
    shell.latest_run = Some(Run {
        completed_at: Some(at("2026-04-10T09:30:00Z")),
        ..Default::default()
    });
    assert_eq!(woke_at(&shell, now), Some(fresh));
    shell.runtime = Some(RuntimeState::Failed);
    shell.latest_run.as_mut().unwrap().state = Some(RunState::Failed);
    assert_eq!(woke_at(&shell, now), Some(at("2026-04-10T09:30:00Z")));
    let mut shell = ShelfInput {
        latest_user_message_at: Some(now - 30_000),
        ..Default::default()
    };
    assert!(!can_snooze(&shell, now));
    shell.latest_user_message_at = Some(now - 3_600_000);
    assert!(can_snooze(&shell, now));
    shell.latest_user_message_at = Some(now - 60_000);
    shell.latest_run = Some(Run {
        requested_at: shell.latest_user_message_at,
        ..Default::default()
    });
    assert!(!has_queued_turn_start(&shell, now));
    shell.latest_run = None;
    shell.legacy_session_error = true;
    assert!(!has_queued_turn_start(&shell, now));
    assert!(is_woke_unseen(Some(fresh), false, None, None));
    assert!(!is_woke_unseen(Some(fresh), false, Some(fresh), None));
    assert!(is_woke_unseen(Some(now), false, Some(fresh), None));
}
fn settings(value: serde_json::Value) -> Vec<SnoozePresetSetting> {
    serde_json::from_value(value).unwrap()
}
fn calendar<T>(now: i64, weekday: u8, f: impl FnOnce(&SnoozeCalendar<'_>) -> T) -> T {
    let local = |days, hours, minutes| {
        Some(
            (now.div_euclid(DAY) + days) * DAY
                + i64::from(hours) * 3_600_000
                + i64::from(minutes) * 60_000,
        )
    };
    let same = |a: i64, b: i64| a.div_euclid(DAY) == b.div_euclid(DAY);
    let time = |value: i64| {
        format!(
            "{}:{:02}",
            value.rem_euclid(DAY) / 3_600_000,
            value.rem_euclid(3_600_000) / 60_000
        )
    };
    let short = |value: i64| {
        ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"][(i64::from(weekday)
            + value.div_euclid(DAY)
            - now.div_euclid(DAY))
        .rem_euclid(7) as usize]
            .to_string()
    };
    let iso = |value| format!("epoch:{value}");
    f(&SnoozeCalendar {
        now_ms: now,
        weekday,
        local_wake: &local,
        same_day: &same,
        format_time: &time,
        short_weekday: &short,
        to_iso: &iso,
    })
}
fn menu(now: &str, weekday: u8, presets: &[SnoozePresetSetting]) -> Vec<String> {
    calendar(at(now), weekday, |c| {
        resolve_snooze_presets(presets, c)
            .into_iter()
            .map(|p| format!("{} {}", p.label, p.when_label))
            .collect()
    })
}
#[test]
fn snooze_presets_sort_by_real_day_gap_drop_expired_times_and_deduplicate_minutes() {
    let presets = settings(
        json!([{"kind":"at","day":"monday","time":"18:00"},{"kind":"at","day":"tomorrow","time":"21:00"},{"kind":"after","minutes":360},{"kind":"after","minutes":180}]),
    );
    assert_eq!(
        menu("2026-04-08T20:00:00Z", 3, &presets),
        [
            "In 3 hours 23:00",
            "In 6 hours Thu 2:00",
            "Tomorrow 21:00",
            "Next week Mon 18:00"
        ]
    );
    let presets = settings(
        json!([{"kind":"at","day":"today","time":"18:00"},{"kind":"at","day":"monday","time":"09:00"}]),
    );
    calendar(at("2026-04-13T18:30:00Z"), 1, |c| {
        let rows = resolve_snooze_presets(&presets, c);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "at:monday:09:00");
        assert_eq!(rows[0].wake_ms, at("2026-04-20T09:00:00Z"));
    });
    let presets = settings(
        json!([{"kind":"at","day":"tomorrow","time":"21:00"},{"kind":"at","day":"monday","time":"18:00"},{"kind":"at","day":"friday","time":"17:00"}]),
    );
    assert_eq!(
        menu("2026-08-30T14:00:00Z", 0, &presets),
        ["Tomorrow 18:00", "Tomorrow 21:00", "Friday 17:00"]
    );
    assert_eq!(
        menu("2026-09-04T10:00:00Z", 5, &presets),
        ["Tomorrow 21:00", "Next week Mon 18:00", "Next Friday 17:00"]
    );
    let defaults = t3_contracts::settings::ServerSettings::default().snooze_presets;
    assert_eq!(
        menu("2026-08-30T07:01:00Z", 0, &defaults),
        [
            "In 1 hour 8:01",
            "In 3 hours 10:01",
            "This evening 18:00",
            "Tomorrow 9:00"
        ]
    );
    let presets = settings(
        json!([{"kind":"after","minutes":180},{"kind":"at","day":"today","time":"18:00"}]),
    );
    assert_eq!(
        menu("2026-04-08T15:00:25Z", 3, &presets),
        ["In 3 hours 18:00"]
    );
}
#[test]
fn picking_a_duration_restarts_it_but_picking_a_set_time_keeps_the_shown_day() {
    let presets = settings(
        json!([{"kind":"at","day":"today","time":"18:30"},{"kind":"after","minutes":180}]),
    );
    let shown = calendar(at("2026-04-08T15:00:00Z"), 3, |c| {
        resolve_snooze_presets(&presets, c)
    });
    calendar(at("2026-04-08T15:30:00Z"), 3, |c| {
        let PickDecision::Picked(picked) = pick_snooze_preset(&shown, "after:180", &presets, c)
        else {
            panic!("duration expired")
        };
        assert_eq!(picked.wake_ms, at("2026-04-08T18:30:00Z"));
    });
    let presets = t3_contracts::settings::ServerSettings::default().snooze_presets;
    let shown = calendar(at("2026-08-30T23:59:00Z"), 0, |c| {
        resolve_snooze_presets(&presets, c)
    });
    calendar(at("2026-08-31T00:01:00Z"), 1, |c| {
        let PickDecision::Picked(picked) =
            pick_snooze_preset(&shown, "at:tomorrow:09:00", &presets, c)
        else {
            panic!("shown time expired")
        };
        assert_eq!(picked.wake_ms, at("2026-08-31T09:00:00Z"));
    });
}
