#![allow(clippy::unwrap_used)]
use futures::FutureExt;
use serde_json::json;
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};
use t3_client::{
    thread_views::{
        shelves::{RuntimeState, ShelfInput},
        threads::sort::*,
    },
    types::{EnvironmentId, ThreadId, ThreadKey},
    view_logic::shell::sidebar::{gestures::*, presentation::*, status::*, *},
    workspace::{
        ProjectKey,
        projects::{EnvironmentProject, GroupingSettings},
        sidebar::build_sidebar_snapshots,
    },
};
use t3_contracts::orchestration_v2::*;
fn key(e: &str, t: &str) -> ThreadKey {
    ThreadKey {
        environment: EnvironmentId(e.into()),
        thread: ThreadId(t.into()),
    }
}
fn shell(id: &str) -> OrchestrationV2ThreadShell {
    let mut v: serde_json::Value = serde_json::from_str(
        include_str!("../../../fixtures/samples/OrchestrationV2ThreadShell.jsonl")
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    for (k, value) in [
        ("id", json!(id)),
        ("projectId", json!("p")),
        ("title", json!(id)),
        ("createdAt", json!("1970-01-01T00:00:00.000Z")),
        ("deletedAt", json!(null)),
        ("settledOverride", json!(null)),
        ("latestUserMessageAt", json!(null)),
        ("pendingRuntimeRequest", json!(null)),
        ("hasActionableProposedPlan", json!(false)),
        (
            "lineage",
            json!({"rootThreadId":id,"parentThreadId":null,"relationshipToParent":null}),
        ),
    ] {
        v[k] = value;
    }
    serde_json::from_value(v).unwrap()
}
fn row<'a>(e: &str, s: &'a OrchestrationV2ThreadShell) -> SidebarThread<'a> {
    SidebarThread {
        key: key(e, &s.id.0),
        project: ProjectKey {
            environment: EnvironmentId(e.into()),
            project: s.project_id.clone(),
        },
        shell: s,
    }
}
fn parse(s: &str) -> Option<i64> {
    s.parse().ok()
}
fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|s| (*s).into()).collect()
}
#[test]
fn request_runtime_priority_window_title_and_recession() {
    let base = StatusInput::default();
    for (runtime, expected) in [
        (RuntimeState::Preparing, Status::Preparing),
        (RuntimeState::Queued, Status::Working),
        (RuntimeState::Starting, Status::Working),
        (RuntimeState::Running, Status::Working),
        (RuntimeState::Waiting, Status::Working),
        (RuntimeState::Idle, Status::Waiting),
        (RuntimeState::Failed, Status::Failed),
        (RuntimeState::Other, Status::Ready),
    ] {
        let input = StatusInput {
            runtime: Some(runtime),
            ..base.clone()
        };
        assert_eq!(resolve_status(&input), expected);
        assert_eq!(
            resolve_status(&StatusInput {
                guest_messages: 1,
                ..input.clone()
            }),
            Status::Request
        );
        assert_eq!(
            resolve_status(&StatusInput {
                user_input: true,
                guest_messages: 1,
                ..input.clone()
            }),
            Status::Input
        );
        assert_eq!(
            resolve_status(&StatusInput {
                approvals: true,
                user_input: true,
                ..input.clone()
            }),
            Status::Approval
        );
        assert!(!should_recede(expected, false, false, true, false));
        assert!(!should_recede(expected, false, false, false, true));
    }
    let running = StatusInput {
        runtime: Some(RuntimeState::Running),
        blocked_on_thread_wait: true,
        ..base.clone()
    };
    assert_eq!(resolve_status(&running), Status::Waiting);
    assert_eq!(window_title_status(&running), Some("Working"));
    assert_eq!(
        window_title_status(&StatusInput {
            guest_messages: 1,
            ..running
        }),
        Some("Working")
    );
    assert_eq!(
        resolve_status(&StatusInput {
            runtime: Some(RuntimeState::Failed),
            usage_limit: true,
            ..base.clone()
        }),
        Status::Limited
    );
    for status in [
        Status::Input,
        Status::Request,
        Status::Limited,
        Status::Failed,
    ] {
        assert!(!should_recede(status, false, false, false, false));
    }
    for status in [Status::Preparing, Status::Working, Status::Waiting] {
        assert!(should_recede(status, true, true, false, false));
    }
    for status in [Status::Ready, Status::Approval] {
        assert!(should_recede(status, false, false, false, false));
        assert!(!should_recede(status, true, false, false, false));
        assert!(!should_recede(status, false, true, false, false));
    }
}
#[test]
fn unread_failure_plan_and_background_pill_precedence() {
    let base = StatusInput {
        latest_run_completed_at: Some("20"),
        ..Default::default()
    };
    for (visit, unread) in [
        (None, false),
        (Some(""), false),
        (Some("invalid"), true),
        (Some("19"), true),
        (Some("20"), false),
        (Some("21"), false),
    ] {
        let i = StatusInput {
            last_visited_at: visit,
            ..base.clone()
        };
        assert_eq!(has_unseen_completion(&i, &parse), unread);
    }
    let done = StatusInput {
        last_visited_at: Some("19"),
        ..base
    };
    assert_eq!(resolve_status_pill(&done, &parse), Some(Pill::Completed));
    let failed = StatusInput {
        latest_run_failed: true,
        ..done.clone()
    };
    assert_eq!(resolve_status_pill(&failed, &parse), Some(Pill::Failed));
    assert_eq!(
        resolve_status_pill(
            &StatusInput {
                last_visited_at: None,
                ..failed.clone()
            },
            &parse
        ),
        Some(Pill::Failed)
    );
    assert_eq!(
        resolve_status_pill(
            &StatusInput {
                last_visited_at: Some("21"),
                ..failed
            },
            &parse
        ),
        None
    );
    let plan = StatusInput {
        plan_mode: true,
        latest_run_settled: true,
        actionable_plan: true,
        ..done.clone()
    };
    assert_eq!(resolve_status_pill(&plan, &parse), Some(Pill::PlanReady));
    assert_eq!(
        resolve_status_pill(
            &StatusInput {
                background_holds_completion: true,
                ..plan
            },
            &parse
        ),
        Some(Pill::Waiting)
    );
    for (runtime, blocked, pill) in [
        (RuntimeState::Running, false, Pill::Working),
        (RuntimeState::Running, true, Pill::Waiting),
        (RuntimeState::Waiting, true, Pill::Working),
        (RuntimeState::Preparing, false, Pill::Connecting),
        (RuntimeState::Queued, false, Pill::Connecting),
        (RuntimeState::Starting, false, Pill::Connecting),
    ] {
        let i = StatusInput {
            runtime: Some(runtime),
            blocked_on_thread_wait: blocked,
            ..done.clone()
        };
        assert_eq!(resolve_status_pill(&i, &parse), Some(pill));
        assert_eq!(
            pill.pulse(),
            matches!(pill, Pill::Working | Pill::Connecting)
        );
    }
    assert_eq!(resolve_last_visited_at(None, Some("local")), Some("local"));
    assert_eq!(resolve_last_visited_at(Some(None), Some("local")), None);
    assert_eq!(
        resolve_last_visited_at(Some(Some("server")), Some("local")),
        Some("server")
    );
}
#[test]
fn folder_dots_follow_urgency_and_waiting_stays_quiet() {
    let done = StatusInput {
        latest_run_completed_at: Some("20"),
        last_visited_at: Some("10"),
        ..Default::default()
    };
    let running = StatusInput {
        runtime: Some(RuntimeState::Running),
        ..Default::default()
    };
    let failed = StatusInput {
        runtime: Some(RuntimeState::Failed),
        ..Default::default()
    };
    let attention = StatusInput {
        user_input: true,
        ..Default::default()
    };
    assert_eq!(folder_dot(&[], &parse), None);
    assert_eq!(
        folder_dot(std::slice::from_ref(&running), &parse),
        Some("working")
    );
    assert_eq!(
        folder_dot(&[running.clone(), done.clone()], &parse),
        Some("unread")
    );
    assert_eq!(
        folder_dot(&[running.clone(), done.clone(), failed.clone()], &parse),
        Some("failed")
    );
    assert_eq!(
        folder_dot(&[running, done, failed, attention], &parse),
        Some("attention")
    );
    assert_eq!(
        folder_dot(
            &[StatusInput {
                runtime: Some(RuntimeState::Idle),
                ..Default::default()
            }],
            &parse
        ),
        None
    );
}
#[test]
fn hidden_ancestry_cycles_orphans_and_environment_identity() {
    let root = shell("root");
    let mut child = shell("child");
    child.lineage.parent_thread_id = Some(root.id.clone());
    child.lineage.relationship_to_parent = Some(AppThreadLineageRelationshipToParent::Subagent);
    let mut grand = shell("grand");
    grand.lineage.parent_thread_id = Some(child.id.clone());
    grand.lineage.side_chat = Some(Some(true));
    let mut fork = shell("fork");
    fork.lineage.parent_thread_id = Some(root.id.clone());
    fork.lineage.relationship_to_parent = Some(AppThreadLineageRelationshipToParent::Fork);
    let rows = [
        row("a", &root),
        row("a", &child),
        row("a", &grand),
        row("a", &fork),
        row("b", &child),
    ];
    assert_eq!(
        resolve_row_thread_key(&key("a", "grand"), &rows),
        key("a", "root")
    );
    assert_eq!(
        resolve_row_thread_key(&key("a", "fork"), &rows),
        key("a", "fork")
    );
    assert_eq!(
        resolve_row_thread_key(&key("b", "child"), &rows),
        key("b", "child")
    );
    assert_eq!(visible_threads(&rows, None, &BTreeSet::new()).len(), 2);
    child.lineage.parent_thread_id = Some(grand.id.clone());
    let rows = [row("a", &child), row("a", &grand)];
    assert_eq!(
        resolve_row_thread_key(&key("a", "grand"), &rows),
        key("a", "grand")
    );
}
#[test]
fn selection_range_anchor_removal_and_delete_cleanup() {
    let mut s = ThreadSelection::default();
    s.range_select_to("b", &strings(&["a", "b", "c", "d"]));
    s.range_select_to("d", &strings(&["a", "b", "c", "d"]));
    assert_eq!(s.selected, strings(&["b", "c", "d"]));
    assert_eq!(s.anchor.as_deref(), Some("b"));
    s.toggle("b");
    assert_eq!(s.anchor.as_deref(), Some("b"));
    s.remove(&strings(&["b"]));
    assert_eq!(s.anchor.as_deref(), Some("b"));
    s.range_select_to("missing", &strings(&["a"]));
    assert_eq!(s.anchor.as_deref(), Some("missing"));
    s.remove(&strings(&["missing"]));
    assert_eq!(s.anchor, None);
    assert_eq!(
        keys_to_deselect_after_delete(
            &strings(&["c", "d", "stale"]),
            &BTreeSet::from(["c".into()]),
            |k| k != "stale"
        ),
        strings(&["c", "stale"])
    );
    s.clear();
    assert!(s.selected.is_empty());
}
#[test]
fn deletion_is_sequential_keeps_first_failure_and_stops_on_interrupt() {
    let seen = Rc::new(RefCell::new(vec![]));
    let out = seen.clone();
    let (deleted, failure) = futures::executor::block_on(delete_selected_entries(
        &["a", "b", "c", "d", "e"],
        |s| (*s).into(),
        move |s, deleted| {
            out.borrow_mut().push(((*s).to_string(), deleted.clone()));
            async move {
                match *s {
                    "a" => DeleteResult::Failure("first"),
                    "b" => DeleteResult::Success,
                    "c" => DeleteResult::Failure("later"),
                    "d" => DeleteResult::Interrupted,
                    _ => DeleteResult::Success,
                }
            }
            .boxed_local()
        },
    ));
    assert_eq!(deleted, BTreeSet::from(["b".into()]));
    assert_eq!(failure, Some("first"));
    assert_eq!(seen.borrow().len(), 4);
    assert_eq!(seen.borrow()[3].1, deleted);
}
#[test]
fn route_reveal_survives_missing_params_and_reopened_threads() {
    let mut r = RouteReveal::default();
    let a = key("e", "a");
    let b = key("e", "b");
    assert_eq!(r.pending(Some(&a)), Some(a.clone()));
    assert_eq!(r.pending(None), None);
    assert_eq!(r.pending(Some(&a)), Some(a.clone()));
    r.complete();
    assert_eq!(r.pending(None), None);
    assert_eq!(
        r.resolve(Some(&a), false, true, |_| false, |_| true),
        (None, false)
    );
    r.pending(Some(&b));
    assert_eq!(
        r.resolve(Some(&a), false, true, |_| false, |_| true),
        (Some(a.clone()), true)
    );
    assert_eq!(
        r.resolve(Some(&a), true, true, |_| false, |_| true),
        (Some(a), false)
    );
}
#[test]
fn snooze_clock_skips_elapsed_deadlines_and_park_navigation() {
    assert_eq!(next_snooze_wake(&[50, 10, 30], 20), Some(30));
    assert_eq!(advance_snooze_clock(&[10, 30, 50], 0, 40), 30);
    let s = ShelfInput {
        snoozed_until: Some(40),
        ..Default::default()
    };
    assert!(should_navigate_after_park(
        "a",
        Some("a"),
        false,
        false,
        Some(&s),
        20
    ));
    assert!(!should_navigate_after_park(
        "a",
        Some("b"),
        true,
        true,
        Some(&s),
        20
    ));
    assert!(!should_navigate_after_park(
        "a",
        Some("a"),
        false,
        false,
        Some(&s),
        50
    ));
}
#[test]
fn preferred_aliases_adjacency_and_accessibility() {
    let items = [("a", "old-a"), ("b", "old-b"), ("c", "old-c")];
    assert_eq!(
        order_by_preferences(&items, &["old-c", "a", "c"], |i| vec![i.0, i.1]),
        vec![&items[2], &items[0], &items[1]]
    );
    assert_eq!(adjacent_thread(&[1, 2, 3], None, true), Some(&3));
    assert_eq!(adjacent_thread(&[1, 2, 3], Some(&1), true), None);
    assert_eq!(adjacent_thread(&[1, 2, 3], Some(&2), false), Some(&3));
    assert_eq!(adjacent_thread(&[1, 2, 3], Some(&9), false), None);
    assert_eq!(
        row_accessibility("Title", Some("Working"), Some("Project"), true),
        ("Title, Working, Project".into(), Some("page"))
    );
    for (shift, open, total, expected) in [
        (false, 0, 0, true),
        (false, 0, 2, false),
        (false, 1, 2, true),
        (false, 2, 2, false),
        (true, 2, 2, true),
    ] {
        assert_eq!(
            should_create_new_thread_in_current_project(shift, open, total),
            expected
        );
    }
}
#[test]
fn composed_sidebar_orders_shelves_and_scopes_environment_folders() {
    let mut pinned = shell("pinned");
    pinned.pinned_at = Some(Some(t3_contracts::wire::WireDateTime(
        "1970-01-01T00:00:00.001Z".into(),
    )));
    pinned.pin_order_key = Some(Some("m".into()));
    let active = shell("active");
    let working = shell("working");
    let snoozed = shell("snoozed");
    let mut settled = shell("settled");
    settled.settled_override = Some(AppThreadSettledOverride::Settled);
    let remote = shell("active");
    let mut archived = shell("archived");
    archived.archived_at = Some(t3_contracts::wire::WireDateTime(
        "1970-01-01T00:00:00.001Z".into(),
    ));
    let input = |e: &str, s| ThreadInput {
        thread: row(e, s),
        shelf: ShelfInput::default(),
        status: StatusInput::default(),
    };
    let mut inputs = vec![
        input("a", &settled),
        input("a", &snoozed),
        input("a", &working),
        input("a", &active),
        input("a", &pinned),
        input("b", &remote),
        input("a", &archived),
    ];
    inputs[1].shelf.snoozed_until = Some(100);
    inputs[2].shelf.runtime = Some(RuntimeState::Running);
    inputs[2].status.runtime = Some(RuntimeState::Running);
    inputs[3].status.latest_run_completed_at = Some("20");
    inputs[3].status.last_visited_at = Some("10");
    let project = |e: &str| {
        EnvironmentProject{environment:EnvironmentId(e.into()),project:serde_json::from_value(json!({"id":"p","title":e,"workspaceRoot":format!("/{e}"),"repositoryIdentity":null,"defaultModelSelection":{"instanceId":"codex","model":"m"},"createdAt":"1970-01-01T00:00:00.000Z","updatedAt":"1970-01-01T00:00:00.000Z","scripts":[]})).unwrap(),is_scratch:false}
    };
    let groups = build_sidebar_snapshots(
        &[project("a"), project("b")],
        &GroupingSettings::default(),
        false,
        None,
        |_| None,
        |_| false,
        |_| false,
    );
    let empty = BTreeSet::new();
    let selected = BTreeSet::from([key("a", "active")]);
    let options = Options {
        now: 0,
        scope: None,
        delegated_under_parent: &empty,
        selected: &selected,
        route: Some(&inputs[3].thread.key),
        working_shelf: true,
        active_order: ActiveOrder::Manual,
        inbox: None,
    };
    let view = present_sidebar(&groups, &inputs, &options, &parse, |_| true);
    assert_eq!(
        view.rows.iter().map(|r| r.section).collect::<Vec<_>>(),
        vec![
            Section::Pinned,
            Section::Active,
            Section::Active,
            Section::Working,
            Section::Snoozed,
            Section::Settled
        ]
    );
    assert_eq!(view.next_snooze_wake, Some(100));
    assert_eq!(view.folders.len(), 2);
    let active = view
        .rows
        .iter()
        .find(|r| r.source.thread.key == key("a", "active"))
        .unwrap();
    assert!(active.unread && active.active && active.selected && !active.receded);
    assert_eq!(view.folders[0].dot, Some("unread"));
    assert_eq!(view.folders[1].rows.len(), 1);
    let scope = BTreeSet::from([inputs[5].thread.project.clone()]);
    let options = Options {
        scope: Some(&scope),
        ..options
    };
    let view = present_sidebar(&groups, &inputs, &options, &parse, |_| false);
    assert_eq!(view.rows.len(), 1);
    assert_eq!(view.rows[0].source.thread.key, key("b", "active"));
    assert_eq!(view.folders.len(), 1);
}
#[test]
fn drag_sections_drop_verbs_and_lifecycle() {
    let items = vec![
        ListItem::Marker(Marker::PinnedHeader),
        ListItem::Thread {
            key: "p".into(),
            section: Section::Pinned,
        },
        ListItem::Marker(Marker::PinnedDivider),
        ListItem::Thread {
            key: "a".into(),
            section: Section::Active,
        },
        ListItem::Marker(Marker::WorkingHeader),
        ListItem::Thread {
            key: "w".into(),
            section: Section::Working,
        },
        ListItem::Marker(Marker::SettledHeader),
        ListItem::Thread {
            key: "s".into(),
            section: Section::Settled,
        },
    ];
    assert!(resolve_drop_target(&items, "a", "w").is_none());
    let drop = resolve_drop_target(&items, "p", "a").unwrap();
    assert_eq!(drop.section, Section::Active);
    assert_eq!(drop.active_order, strings(&["a", "p"]));
    assert!(drop.pinned_order.is_empty());
    for (from, to, verb) in [
        (Section::Active, Section::Pinned, "pin"),
        (Section::Pinned, Section::Active, "unpin"),
        (Section::Settled, Section::Active, "unsettle"),
        (Section::Snoozed, Section::Active, "wake"),
        (Section::Active, Section::Settled, "settle"),
    ] {
        assert_eq!(resolve_drop_verb(from, Some(to)), Some(verb));
    }
    assert_eq!(
        resolve_drop_verb(Section::Active, Some(Section::Working)),
        None
    );
    let life = Lifecycle {
        settled_override: Some("settled".into()),
        settled_at: Some("old".into()),
        snoozed_until: Some("later".into()),
        ..Default::default()
    };
    let pinned = apply_thread_drop(life, Section::Pinned, "now", Some("m"));
    assert_eq!(pinned.unsettled_at.as_deref(), Some("now"));
    assert_eq!(pinned.pin_order_key.as_deref(), Some("m"));
    assert_eq!(pinned.snoozed_until, None);
    let settled = apply_thread_drop(pinned, Section::Settled, "then", None);
    assert_eq!(settled.pinned_at, None);
    assert_eq!(settled.active_order_key, None);
    assert_eq!(settled.settled_at.as_deref(), Some("then"));
    assert_eq!(
        apply_thread_drop(settled, Section::Settled, "again", None)
            .settled_at
            .as_deref(),
        Some("then")
    );
}
#[test]
fn offline_bounds_cannot_be_rebalanced_or_crossed() {
    for section in [Section::Active, Section::Pinned] {
        let writable = BTreeSet::from(["online".into(), "neighbor".into()]);
        let keys = BTreeMap::from([
            ("offline".into(), None),
            ("online".into(), None),
            ("neighbor".into(), None),
        ]);
        let make = |order: Vec<String>,
                    keys: &BTreeMap<String, Option<String>>,
                    writable: &BTreeSet<String>| {
            plan_thread_drop(DropInput {
                active_key: "online",
                active_section: section,
                active_pinned: None,
                active_settled: None,
                supports_settlement: true,
                target: &DropTarget {
                    section,
                    pinned_order: order.clone(),
                    active_order: order,
                },
                pinned_order: &strings(&["offline", "online", "neighbor"]),
                pinned_keys: keys,
                reorderable: Some(writable),
                active_order: &strings(&["offline", "online", "neighbor"]),
                active_keys: keys,
                active_reorderable: Some(writable),
                active_time_ordered: false,
            })
        };
        let impossible = if section == Section::Active {
            strings(&["online", "offline"])
        } else {
            strings(&["offline", "online"])
        };
        assert_eq!(make(impossible, &keys, &writable), DropPlan::None);
        let allowed = if section == Section::Active {
            strings(&["offline", "neighbor", "online"])
        } else {
            strings(&["online", "neighbor", "offline"])
        };
        assert_ne!(make(allowed, &keys, &writable), DropPlan::None);
        let keys = BTreeMap::from([
            ("offline".into(), Some("m".into())),
            ("online".into(), Some("t".into())),
        ]);
        let plan = make(strings(&["online", "offline"]), &keys, &writable);
        let assignments = match plan {
            DropPlan::MoveActive { assignments, .. }
            | DropPlan::ReorderPinned { assignments, .. } => assignments,
            _ => panic!("missing move"),
        };
        assert_eq!(assignments.len(), 1);
        assert_eq!(assignments[0].0, "online");
        assert!(assignments[0].1.as_str() < "m");
        assert_eq!(
            make(strings(&["online", "offline"]), &keys, &BTreeSet::new()),
            DropPlan::None
        );
    }
    let threads = [key("online", "t"), key("offline", "t")];
    assert_eq!(
        reorderable_thread_keys(
            &threads,
            &BTreeSet::from([
                EnvironmentId("online".into()),
                EnvironmentId("offline".into())
            ]),
            &BTreeSet::from([EnvironmentId("online".into())])
        ),
        BTreeSet::from([threads[0].clone()])
    );
}
#[test]
fn pointer_threshold_claim_release_cancel_and_click_suppression() {
    assert!(PointerSensor::activate(false, 0, 1, Point::default(), 5.0).is_none());
    assert!(PointerSensor::activate(true, 2, 1, Point::default(), 5.0).is_none());
    let mut p = PointerSensor::activate(true, 0, 1, Point::default(), 5.0).unwrap();
    assert!(
        p.move_pointer(2, Point { x: 10.0, y: 0.0 }, 1, false)
            .is_empty()
    );
    assert!(matches!(
        p.move_pointer(1, Point { x: 3.0, y: 4.0 }, 1, false)[0],
        PointerEffect::Pending { .. }
    ));
    assert_eq!(
        p.move_pointer(1, Point { x: 6.0, y: 0.0 }, 1, true),
        vec![PointerEffect::Start(Point::default())]
    );
    assert!(p.suppress_click);
    assert_eq!(
        p.end(1, Point { x: 6.0, y: 0.0 }, false),
        vec![
            PointerEffect::Drop(Point { x: 6.0, y: 0.0 }),
            PointerEffect::Finish { started: true },
            PointerEffect::End
        ]
    );
    assert!(p.cancel().is_empty());
    p.clear_click_suppression();
    assert!(!p.suppress_click);
    let mut pending = PointerSensor::activate(true, 0, 1, Point::default(), 5.0).unwrap();
    assert_eq!(
        pending.move_pointer(1, Point::default(), 0, false),
        vec![
            PointerEffect::Finish { started: false },
            PointerEffect::Abort,
            PointerEffect::End
        ]
    );
    assert!(!pending.suppress_click);
}
#[test]
fn shift_guard_blocks_surprise_motion_and_honors_own_input() {
    let mut guard = ShiftGuard::new(&[("a".into(), 0.0, 40.0)]);
    guard.update(&[("a".into(), 50.0, 40.0)], 1000);
    assert!(guard.pointer_down(1, Some("a"), 1100));
    assert!(guard.pointer_up(1, true, 1150));
    assert!(guard.follow_up(true, 1, Some(1), true));
    assert!(!guard.follow_up(true, 0, Some(1), true));
    assert!(!guard.pointer_down(2, Some("a"), 1600));
    guard.update(&[("a".into(), 100.0, 40.0)], 1700);
    assert!(!guard.pointer_down(2, Some("a"), 1750));
}
#[test]
fn sweep_geometry_and_fractional_order_contracts() {
    assert_eq!(
        sweep_keys(&strings(&["a", "b", "c"]), "c", "a", |k| k != "b"),
        strings(&["a", "c"])
    );
    assert!(sweep_keys(&[], "a", "b", |_| true).is_empty());
    assert_eq!(
        thread_key_at_y(&[("a", 10.0), ("b", 30.0)], 100.0, Some((0.0, 25.0))),
        Some("a")
    );
    let mut head = None;
    for _ in 0..100 {
        let next = pin_order_key_between(None, head.as_deref()).unwrap();
        assert!(head.as_ref().is_none_or(|h| &next < h));
        head = Some(next);
    }
    for (a, b) in [
        (Some("z"), Some("a")),
        (Some("A!"), None),
        (None, Some("ma")),
        (Some("m"), Some("m")),
    ] {
        assert!(pin_order_key_between(a, b).is_none());
    }
}
fn marker(m: Marker) -> ListItem {
    ListItem::Marker(m)
}
fn item(key: &str, section: Section) -> ListItem {
    ListItem::Thread {
        key: key.into(),
        section,
    }
}
fn layout(items: &[ListItem], scale: f64, card: f64) -> Vec<Option<Rect>> {
    let mut top = 100.0;
    items
        .iter()
        .map(|i| {
            let height = match i {
                ListItem::Thread {
                    section: Section::Pinned | Section::Active | Section::Working,
                    ..
                } => card,
                ListItem::Thread { .. } => 36.0,
                ListItem::Marker(
                    Marker::PinnedHeader
                    | Marker::PinnedDivider
                    | Marker::ActivePlaceholder
                    | Marker::SettledPlaceholder,
                ) => 0.0,
                _ => 32.0,
            } * scale;
            let r = Rect {
                top,
                left: 0.0,
                width: 260.0,
                height,
            };
            top += height + 1.0;
            Some(r)
        })
        .collect()
}
fn project(
    items: &[ListItem],
    active: &str,
    over: &str,
    settled: &[String],
    expanded: bool,
    labels: f64,
) -> Vec<Transform> {
    let rects = layout(items, 1.0, 82.0);
    sorting_transforms(SortingInput {
        items,
        rects: &rects,
        active_index: items.iter().position(|i| i.id() == active).unwrap(),
        over_index: items
            .iter()
            .position(|i| i.id() == over)
            .unwrap_or(usize::MAX),
        enabled: true,
        settled_order: settled,
        active_order: None,
        settled_expanded: expanded,
        settled_visible_count: None,
        route_key: None,
        snoozed_count: 0,
        card_height: None,
        slim_height: None,
        label_height: labels,
    })
}
#[test]
fn drag_projection_preserves_labels_and_pointer_slots() {
    let items = [
        marker(Marker::PinnedHeader),
        item("p1", Section::Pinned),
        item("p2", Section::Pinned),
        marker(Marker::PinnedDivider),
        item("a1", Section::Active),
        marker(Marker::SettledHeader),
        item("s1", Section::Settled),
    ];
    let r = project(&items, "p2", &Marker::PinnedHeader.id(), &[], true, 0.0);
    assert_eq!(r[0], STATIONARY);
    assert_eq!(r[1].y, 83.0);
    assert_eq!(r[3], STATIONARY);
    assert_eq!(r[4], STATIONARY);
    let items = [
        marker(Marker::PinnedHeader),
        item("p", Section::Pinned),
        marker(Marker::PinnedDivider),
        item("a1", Section::Active),
        item("a2", Section::Active),
        marker(Marker::SettledHeader),
        item("s", Section::Settled),
    ];
    for (over, a1, a2) in [
        (Marker::PinnedDivider.id(), 0.0, 0.0),
        ("a1".into(), -83.0, 0.0),
        ("a2".into(), -83.0, -83.0),
    ] {
        let r = project(&items, "p", &over, &[], true, 0.0);
        assert_eq!(r[0], STATIONARY);
        assert_eq!(r[2].y, -83.0);
        assert_eq!(r[3].y, a1);
        assert_eq!(r[4].y, a2);
        assert_eq!(r[5].y, 0.0);
    }
    let r = project(&items, "a2", "a1", &[], true, 16.0);
    assert_eq!(r[1].y, 16.0);
    assert_eq!(r[2].y, 16.0);
    assert_eq!(r[3].y, 115.0);
    assert_eq!(r[4], STATIONARY);
    assert_eq!(r[5].y, 32.0);
    assert_eq!(r[6].y, 32.0);
    for (over, first) in [("a1", 83.0), ("a2", 0.0)] {
        let r = project(&items, "s", over, &[], true, 0.0);
        assert_eq!(r[3].y, first);
        assert_eq!(r[4].y, 83.0);
        assert_eq!(r[5].y, 83.0);
    }
}
#[test]
fn drag_projection_canonical_settled_order_and_collapsed_shelves() {
    let items = [
        marker(Marker::PinnedHeader),
        item("p", Section::Pinned),
        marker(Marker::PinnedDivider),
        item("a", Section::Active),
        marker(Marker::SettledHeader),
        item("s1", Section::Settled),
        item("s2", Section::Settled),
    ];
    let r = project(&items, "a", "s2", &strings(&["s1", "a", "s2"]), true, 0.0);
    assert_eq!(r[4].y, -46.0);
    assert_eq!(r[5].y, -46.0);
    assert_eq!(r[6].y, -9.0);
    let items = [
        marker(Marker::PinnedHeader),
        item("p", Section::Pinned),
        marker(Marker::PinnedDivider),
        item("a1", Section::Active),
        item("a2", Section::Active),
        marker(Marker::SettledHeader),
        marker(Marker::SettledPlaceholder),
    ];
    let r = project(
        &items,
        "a2",
        &Marker::SettledPlaceholder.id(),
        &[],
        false,
        0.0,
    );
    assert_eq!(r[5].y, -83.0);
    assert_eq!(r[6].y, -83.0);
}
#[test]
fn drag_projection_wake_header_and_route_pagination() {
    let items = [
        marker(Marker::PinnedHeader),
        item("p", Section::Pinned),
        marker(Marker::PinnedDivider),
        item("a", Section::Active),
        marker(Marker::SnoozedHeader),
        item("z", Section::Snoozed),
        marker(Marker::SettledHeader),
        item("s", Section::Settled),
    ];
    let r = project(&items, "z", "a", &[], true, 0.0);
    assert_eq!(r[4].scale_y, 0.0);
    assert_eq!(r[6].y, 13.0);
    assert_eq!(r[7].y, 13.0);
    let items = [
        marker(Marker::PinnedHeader),
        item("p", Section::Pinned),
        marker(Marker::PinnedDivider),
        item("a", Section::Active),
        marker(Marker::SettledHeader),
        item("s", Section::Settled),
    ];
    let rects = layout(&items, 1.0, 82.0);
    let order = strings(&["a", "s", "hidden"]);
    for (route, visible) in [(Some("s"), true), (None, false)] {
        let r = sorting_transforms(SortingInput {
            items: &items,
            rects: &rects,
            active_index: 3,
            over_index: 5,
            enabled: true,
            settled_order: &order,
            active_order: None,
            settled_expanded: true,
            settled_visible_count: Some(1),
            route_key: route,
            snoozed_count: 0,
            card_height: None,
            slim_height: None,
            label_height: 0.0,
        });
        assert_eq!(r[5].scale_y, if visible { 1.0 } else { 0.0 });
        if visible {
            assert_eq!(r[5].y, -9.0);
        }
    }
}
#[test]
fn collision_boundary_follows_pointer_direction_and_invalid_target_falls_back() {
    let items = [
        marker(Marker::PinnedHeader),
        item("p", Section::Pinned),
        marker(Marker::PinnedDivider),
        item("a", Section::Active),
        item("source", Section::Active),
        marker(Marker::SettledHeader),
    ];
    let mut c = CollisionState {
        previous_y: Some(600.0),
        boundary_section: None,
    };
    for (y, top, section) in [
        (330.0, 300.0, Section::Active),
        (317.0, 300.0, Section::Active),
        (316.0, 300.0, Section::Pinned),
        (316.0, 400.0, Section::Pinned),
        (399.0, 400.0, Section::Pinned),
        (400.0, 400.0, Section::Active),
        (400.0, 300.0, Section::Active),
        (317.0, 300.0, Section::Active),
        (316.0, 300.0, Section::Pinned),
    ] {
        let r = c.resolve(
            strings(&["a", "p", "source"]),
            "source",
            &items,
            CollisionGeometry {
                pointer: Some(Point { x: 130.0, y }),
                boundary: Some(Rect {
                    top,
                    left: 0.0,
                    width: 260.0,
                    height: 16.0,
                }),
                active_bottom: Some(600.0),
            },
            |_| true,
        );
        assert_eq!(
            resolve_drop_target(&items, "source", &r[0])
                .unwrap()
                .section,
            section
        );
    }
    let mut c = CollisionState::default();
    assert_eq!(
        c.resolve(
            strings(&["a", "source"]),
            "source",
            &items,
            CollisionGeometry::default(),
            |_| false
        ),
        strings(&["source"])
    );
    assert!(
        c.resolve(
            strings(&["a"]),
            "source",
            &items,
            CollisionGeometry::default(),
            |_| { false }
        )
        .is_empty()
    );
}
#[test]
fn lifted_clearance_tracks_scroll_and_leaves_lower_motion_alone() {
    for height in [36.0, 82.0] {
        for y in [150.0, 136.0, 100.0, 0.0] {
            let r = restrict_below_sidebar_label(
                Transform {
                    y: y - 529.0,
                    ..STATIONARY
                },
                Some(Rect {
                    top: 136.0,
                    height: 500.0,
                    ..Default::default()
                }),
                Some(Rect {
                    top: 511.0,
                    height,
                    ..Default::default()
                }),
                32.0,
            );
            assert_eq!(511.0 + r.y, 168.0);
        }
    }
    let r = restrict_below_sidebar_label(
        Transform {
            y: -200.0,
            ..STATIONARY
        },
        Some(Rect {
            top: 136.0,
            ..Default::default()
        }),
        Some(Rect {
            top: 511.0,
            ..Default::default()
        }),
        32.0,
    );
    assert_eq!(r.y, -200.0);
}
#[test]
fn modifier_hold_restarts_on_release_and_reacts_to_focus_without_another_delay() {
    use t3_client::view_logic::shell::keybindings::*;
    let rules = default_keybindings();
    let mods = Modifiers {
        ctrl: true,
        ..Default::default()
    };
    let context = MatchContext::default().with("isDesktop", true);
    let mut hold = ModifierHold::default();
    hold.observe(mods, 0);
    assert!(!hold.visible(199, &rules, "Linux", &context));
    assert!(hold.visible(200, &rules, "Linux", &context));
    assert!(!hold.visible(
        200,
        &rules,
        "Linux",
        &context.clone().with("modelPickerOpen", true)
    ));
    assert!(hold.visible(200, &rules, "Linux", &context));
    hold.observe(Modifiers::default(), 201);
    assert!(!hold.visible(400, &rules, "Linux", &context));
    hold.observe(mods, 410);
    assert!(!hold.visible(609, &rules, "Linux", &context));
    assert!(hold.visible(610, &rules, "Linux", &context));
    hold.reset();
    assert_eq!(hold.deadline(), None);
    assert_eq!(
        without_removed_checkout(Some("main"), Some("/work"), Some(false)),
        (None, None)
    );
    assert_eq!(
        without_removed_checkout(Some("main"), None, None),
        (Some("main"), None)
    );
    assert!(should_navigate_row(0.0, false));
    assert!(!should_navigate_row(2.0, false));
    assert!(!should_navigate_row(1.0, true));
}
#[test]
fn snooze_presets_keep_calendar_semantics_and_refresh_relative_choices() {
    use t3_client::view_logic::shell::sidebar::snooze::*;
    use t3_contracts::settings::SnoozePresetSetting;
    let settings:Vec<SnoozePresetSetting>=serde_json::from_value(json!([{"kind":"after","minutes":60},{"kind":"after","minutes":30},{"kind":"after","minutes":60},{"kind":"at","day":"tomorrow","time":"09:00"}])).unwrap();
    let local = |days: i64, hour: u32, minute: u32| {
        Some(days * 86_400_000 + i64::from(hour) * 3_600_000 + i64::from(minute) * 60_000)
    };
    let same = |a: i64, b: i64| a / 86_400_000 == b / 86_400_000;
    let time = |t: i64| format!("{}:{}", t / 3_600_000 % 24, t / 60_000 % 60);
    let weekday = |_: i64| "Tue".into();
    let iso = |t: i64| t.to_string();
    let calendar = Calendar {
        now_ms: 0,
        weekday: 1,
        local_wake: &local,
        same_day: &same,
        format_time: &time,
        short_weekday: &weekday,
        to_iso: &iso,
    };
    let shown = resolve_presets(&settings, &calendar);
    assert_eq!(shown.len(), 3);
    assert_eq!(shown[0].label, "In 30 minutes");
    assert_eq!(shown[1].label, "In 1 hour");
    assert_eq!(shown[2].label, "Tomorrow");
    let calendar = Calendar {
        now_ms: 4_000_000,
        ..calendar
    };
    let PickDecision::Picked(p) = pick_preset(&shown, "after:30", &settings, &calendar) else {
        panic!("relative choice expired")
    };
    assert_eq!(p.wake_ms, 5_800_000);
    assert_eq!(
        pick_preset(&shown, "missing", &settings, &calendar),
        PickDecision::Expired
    );
    for (wake, now, label) in [
        (Some(60_001), Some(0), "2m"),
        (Some(3_600_000), Some(0), "1h"),
        (Some(86_400_001), Some(0), "2d"),
        (None, Some(0), "now"),
        (Some(1), Some(2), "now"),
    ] {
        assert_eq!(wake_label(wake, now), label);
    }
}
#[test]
fn canonical_shell_adapter_keeps_server_unread_and_request_priority() {
    use t3_client::services::models::{PendingSubagentRequest, present_thread_shell};
    use t3_contracts::wire::WireDateTime;
    let mut source = shell("t");
    source.last_visited_at = Some(None);
    source.latest_run_completed_at = Some(Some(WireDateTime("20".into())));
    source.status = OrchestrationV2ShellThreadStatus::Completed;
    let view = present_thread_shell(
        EnvironmentId("e".into()),
        Rc::new(source),
        Some(PendingSubagentRequest::Approval),
    );
    let input = ThreadInput::from_environment_shell(&view, Some("10"), &parse);
    assert_eq!(input.status.last_visited_at, None);
    assert_eq!(resolve_status(&input.status), Status::Approval);
    assert!(input.shelf.subagent_request);
    assert!(!has_unseen_completion(&input.status, &parse));
}
#[test]
fn sidebar_search_keeps_server_content_rank_deduplicates_titles_and_folds_pr_terms() {
    use t3_client::view_logic::shell::search::{SearchPullRequest, ThreadSubject};
    let threads = [("a", "Fix sidebar"), ("b", "Other"), ("c", "Other too")];
    let matches = search_threads(
        &threads,
        &[
            key("e", "c"),
            key("e", "a"),
            key("e", "b"),
            key("other", "c"),
        ],
        |t| key("e", t.0),
        "sidebar",
        |t| ThreadSubject {
            title: t.1.into(),
            ..Default::default()
        },
        0.0,
    );
    assert_eq!(matches.title_matches, [&threads[0]]);
    assert_eq!(matches.content_matches, [&threads[2], &threads[1]]);
    let empty = search_threads(
        &threads,
        &[key("e", "b")],
        |t| key("e", t.0),
        "  ",
        |_| ThreadSubject::default(),
        0.0,
    );
    assert!(empty.content_matches.is_empty());
    let prs = [SearchPullRequest {
        number: 42,
        title: Some("Réparer café".into()),
        references: vec!["o/repo#42".into()],
    }];
    for query in ["#42 cafe", "REPARER  repo", "café"] {
        assert!(pull_request_matches_query(&prs, query));
    }
    for query in ["", "  ", "#43", "cafee"] {
        assert!(!pull_request_matches_query(&prs, query));
    }
}
#[test]
fn project_activity_is_scoped_and_ignores_archived_and_hidden_rows() {
    use t3_contracts::wire::WireDateTime;
    let project = |e: &str| {
        EnvironmentProject{environment:EnvironmentId(e.into()),project:serde_json::from_value(json!({"id":"p","title":e,"workspaceRoot":format!("/{e}"),"repositoryIdentity":null,"defaultModelSelection":{"instanceId":"codex","model":"m"},"createdAt":"1","updatedAt":"2","scripts":[]})).unwrap(),is_scratch:false}
    };
    let groups = build_sidebar_snapshots(
        &[project("a"), project("b")],
        &GroupingSettings::default(),
        false,
        None,
        |_| None,
        |_| false,
        |_| false,
    );
    let mut a = shell("a");
    a.updated_at = WireDateTime("10".into());
    let mut b = shell("b");
    b.updated_at = WireDateTime("20".into());
    let mut archived = shell("archived");
    archived.archived_at = Some(WireDateTime("100".into()));
    archived.updated_at = WireDateTime("100".into());
    let mut hidden = shell("hidden");
    hidden.lineage.relationship_to_parent = Some(AppThreadLineageRelationshipToParent::Subagent);
    hidden.updated_at = WireDateTime("200".into());
    let rows = [
        row("a", &a),
        row("b", &b),
        row("a", &archived),
        row("a", &hidden),
    ];
    let activity = project_activity(&groups, &rows, true, &BTreeSet::new(), &parse, |t| {
        parse(&t.shell.updated_at.0)
    });
    let sorted = sort_logical_projects(&activity, false, false, |a, b| {
        a.group.key.cmp(&b.group.key)
    });
    assert_eq!(sorted[0].group.representative.environment.0, "b");
    assert_eq!(
        sort_logical_projects(&activity, true, false, |_, _| std::cmp::Ordering::Equal)[0]
            .group
            .key,
        groups[0].group.key
    );
}

#[test]
fn store_sidebar_rows_share_unchanged_rows_and_follow_the_shell() {
    use t3_client::{ClientStore, state::sharing::SharedShellSnapshot};
    let environment = EnvironmentId("a".into());
    let store = ClientStore::default();
    let session = store.begin_session(environment.clone());
    let mut pinned = shell("pinned");
    pinned.pinned_at = Some(Some(t3_contracts::wire::WireDateTime(
        "1970-01-01T00:00:01.000Z".into(),
    )));
    let snapshot = json!({"kind":"snapshot","snapshot":{"schemaVersion":1,"snapshotSequence":1,"threads":[pinned, shell("active")],"archivedThreads":[],"projects":[]}});
    store.apply_shell(
        &environment,
        session,
        vec![serde_json::from_value(snapshot).unwrap()],
    );
    let empty = BTreeSet::new();
    let options = Options {
        now: 0,
        scope: None,
        delegated_under_parent: &empty,
        selected: &empty,
        route: None,
        working_shelf: true,
        active_order: ActiveOrder::Manual,
        inbox: None,
    };
    let rows = |previous: Option<&SidebarRows>| {
        let shared: Rc<SharedShellSnapshot> = store.shared_shell(&environment).unwrap();
        sidebar_rows(&environment, &shared, &[], &options, previous)
    };
    let first = rows(None);
    assert_eq!(
        first
            .rows
            .iter()
            .map(|row| (row.key.thread.0.as_str(), row.section))
            .collect::<Vec<_>>(),
        vec![("pinned", Section::Pinned), ("active", Section::Active)]
    );
    let again = rows(Some(&first));
    assert!(Rc::ptr_eq(&first.rows[0], &again.rows[0]));
    assert!(Rc::ptr_eq(&first.rows[1], &again.rows[1]));

    let mut renamed = shell("active");
    renamed.title = "Renamed".into();
    store.apply_shell(
        &environment,
        session,
        vec![
            serde_json::from_value(
                json!({"kind":"thread.updated","sequence":2,"location":"active","thread":renamed}),
            )
            .unwrap(),
        ],
    );
    let next = rows(Some(&again));
    assert!(
        Rc::ptr_eq(&again.rows[0], &next.rows[0]),
        "untouched row kept"
    );
    assert!(!Rc::ptr_eq(&again.rows[1], &next.rows[1]));
    assert_eq!(next.rows[1].thread.title, "Renamed");
}
