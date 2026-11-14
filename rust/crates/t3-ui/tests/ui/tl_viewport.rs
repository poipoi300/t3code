//! TL-VIEWPORT: the production viewport over thousands of mixed-height client-derived rows.

use std::rc::Rc;

use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::{Headless, Screen};
use serde_json::json;
use t3_client::thread_views::messages_timeline::{
    MessagesTimelineRowContent, MessagesTimelineRowsInput, MessagesTimelineRowsProjection,
    derive_messages_timeline_rows_with_state,
};
use t3_client::types::{EnvironmentId, ThreadId, ThreadKey};
use t3_client::view_logic::session::{TimelineContent, TimelineEntry};
use t3_ui::runtime::Shared;
use t3_ui::screens::thread::viewport::{TimelineFollow, TimelineViewport, timeline_row_keys};

use crate::support::{DESKTOP, PHONE};

const ROWS: i32 = 3_000;

#[derive(Clone, Copy)]
struct FixtureViewport {
    viewport: TimelineViewport,
    keys: Memo<Shared<Vec<SharedString>>>,
}

fn entry(id: i32, length: usize) -> Rc<TimelineEntry> {
    Rc::new(
        serde_json::from_value(json!({
            "id":format!("m-{id}"), "kind":"message", "createdAt":"2026-11-14T12:00:00Z",
            "message": {"id":format!("m-{id}"), "role":if id % 2 == 0 {"user"} else {"assistant"},
                "text":"x".repeat(length), "streaming":id == ROWS - 1, "runId":null,
                "createdAt":"2026-11-14T12:00:00Z", "updatedAt":"2026-11-14T12:00:00Z"}
        }))
        .expect("canonical client timeline entry"),
    )
}

fn grow(entries: Signal<Rc<Vec<Rc<TimelineEntry>>>>, id: i32) {
    resize_entry(entries, id, 37);
}

fn resize_entry(entries: Signal<Rc<Vec<Rc<TimelineEntry>>>>, id: i32, delta: isize) {
    entries.update(|entries| {
        let entries = Rc::make_mut(entries);
        let index = entries
            .iter()
            .position(|e| e.id == format!("m-{id}"))
            .expect("entry");
        let length = match &entries[index].content {
            TimelineContent::Message { message, .. } => message.text.len(),
            _ => unreachable!(),
        };
        entries[index] = entry(id, length.checked_add_signed(delta).expect("fixture size"));
    });
}

fn control(id: &'static str, action: impl Fn() + 'static) -> impl IntoElement {
    nsh_base::button()
        .test_id(id)
        .accessibility_label(Some(SharedString::from(id)))
        .on_press(action)
        .child(text(SharedString::from("•")))
}

#[component]
fn ViewportFixture() -> impl IntoElement {
    let entries = signal(Rc::new(
        (0..ROWS).map(|id| entry(id, 0)).collect::<Vec<_>>(),
    ));
    let other = signal(false);
    let empty = signal(false);
    let current: Stored<Option<FixtureViewport>> = stored(None);
    let height_reads = stored(0usize);
    let sampled_reads = signal(0usize);
    let key_updates = stored(0usize);
    let sampled_keys = signal(0usize);
    view()
        .w_full()
        .h_full()
        .child(
            view()
                .flex()
                .h(px(30.0))
                .child(control("stream", move || grow(entries, ROWS - 1)))
                .child(control("grow-above", move || grow(entries, 1499)))
                .child(control("grow-near-tail", move || grow(entries, ROWS - 3)))
                .child(control("balance-heights", move || {
                    batch(|| {
                        resize_entry(entries, ROWS - 3, -37);
                        grow(entries, ROWS - 1);
                    })
                }))
                .child(control("prepend", move || {
                    entries.update(|entries| {
                        *entries = Rc::new(
                            (-200..0)
                                .map(|id| entry(id, 0))
                                .chain(entries.iter().cloned())
                                .collect(),
                        );
                    })
                }))
                .child(control("history", move || {
                    current.with(|v| {
                        if let Some(current) = v {
                            let index = current
                                .keys
                                .with(|keys| keys.iter().position(|id| id.as_str() == "m-1500"))
                                .expect("history row");
                            current.viewport.scroll_to_index(index, px(-37.0));
                        }
                    })
                }))
                .child(control("end", move || {
                    current.with(|v| {
                        if let Some(current) = v {
                            current.viewport.scroll_to_end();
                        }
                    })
                }))
                .child(control("switch", move || {
                    other.update(|other| *other = !*other)
                }))
                .child(control("empty", move || {
                    empty.update(|empty| *empty = !*empty)
                }))
                .child(control("sample", move || {
                    sampled_reads.set(height_reads.get());
                    sampled_keys.set(key_updates.get());
                })),
        )
        .child(text(move || {
            format!("height reads {}", sampled_reads.get())
        }))
        .child(text(move || format!("key updates {}", sampled_keys.get())))
        .child(dynamic(move || {
            let other = other.get();
            Some(untracked(move || {
                let previous: Stored<Option<MessagesTimelineRowsProjection>> = stored(None);
                let rows = computed(move || {
                    let input = MessagesTimelineRowsInput {
                        timeline_entries: if empty.get() {
                            Vec::new()
                        } else {
                            entries.get().as_ref().clone()
                        },
                        ..Default::default()
                    };
                    previous.with(|previous| {
                        let next =
                            derive_messages_timeline_rows_with_state(&input, previous.as_ref());
                        let rows = Shared(next.rows.clone());
                        *previous = Some(next);
                        rows
                    })
                });
                let keys = timeline_row_keys(rows);
                effect(move || {
                    keys.get();
                    key_updates.with(|updates| *updates += 1);
                });
                let count = computed(move || rows.with(|r| r.len()));
                let viewport = TimelineViewport::new(
                    ThreadKey {
                        environment: EnvironmentId("host".into()),
                        thread: ThreadId(if other { "other" } else { "thread" }.into()),
                    },
                    keys,
                );
                current.with(|v| *v = Some(FixtureViewport { viewport, keys }));
                let list = virtual_list(count, move |at| {
                    let row =
                        computed(move || rows.with(|r| r.get(at.index()).cloned().map(Shared)));
                    view()
                        .test_id("viewport-row")
                        .h(move || {
                            height_reads.with(|reads| *reads += 1);
                            row.with(|r| {
                                px(match r.as_ref().map(|r| &r.content) {
                                    Some(MessagesTimelineRowContent::Message {
                                        message, ..
                                    }) => {
                                        let number = r
                                            .as_ref()
                                            .expect("message row")
                                            .id
                                            .trim_start_matches("m-")
                                            .parse::<i32>()
                                            .unwrap_or(0);
                                        80.0 + number.rem_euclid(7) as f32 * 27.0
                                            + message.text.len() as f32
                                    }
                                    _ => 24.0,
                                })
                            })
                        })
                        .child(text(move || {
                            if at.index() + 1 == count.get() {
                                return SharedString::from("tail-row");
                            }
                            row.with(|r| {
                                r.as_ref()
                                    .map_or_else(SharedString::default, |r| r.id.clone().into())
                            })
                        }))
                })
                .estimated_item_size(px(120.0))
                .content_padding_y(px(16.0))
                .flex_1()
                .test_id("viewport-list");
                view()
                    .flex_1()
                    .min_h(px(0.0))
                    .child(text(move || {
                        SharedString::from(if viewport.follow.get() == TimelineFollow::Following {
                            "following"
                        } else {
                            "released"
                        })
                    }))
                    .child(viewport.attach(list))
            }))
        }))
}

fn start(window: (f32, f32)) -> Headless {
    let mut app = Headless::start(EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<ViewportFixture>())
    })
    .expect("headless viewport");
    app.first_render().expect("initial layout");
    assert!(app.settle(), "finite initial measurement");
    app
}

fn bounds(screen: &Screen, id: &str) -> Bounds<Points> {
    let nodes = screen.layout_nodes();
    if let Some(node) = nodes.iter().find(|n| n.test_id.as_deref() == Some(id)) {
        return node.bounds;
    }
    let label = nodes
        .iter()
        .find(|n| n.text.as_ref().is_some_and(|text| text.text.text == id))
        .unwrap_or_else(|| panic!("mounted {id}"));
    nodes
        .iter()
        .find(|n| n.id == label.parent)
        .expect("row frame")
        .bounds
}

fn assert_at_end(app: &Headless) {
    let list = bounds(app.screen(), "viewport-list");
    let tail = bounds(app.screen(), "tail-row");
    assert!(
        (list.origin.y.0 + list.size.height.0 - tail.origin.y.0 - tail.size.height.0 - 16.0).abs()
            <= 1.0,
        "last reply is at the measured end: list {list:?}, tail {tail:?}"
    );
}

fn assert_anchored(app: &Headless, expected: f32) {
    let top = bounds(app.screen(), "m-1500").origin.y.0;
    assert!(
        (top - expected).abs() <= 1.0,
        "reading position moved: {expected} -> {top}"
    );
}

#[test]
fn thousands_of_mixed_height_rows_follow_a_stream_and_release_on_user_scroll() {
    for size in [DESKTOP, PHONE] {
        let mut app = start(size);
        assert_at_end(&app);
        app.press("id:sample")
            .expect("sample mounted height bindings");
        let mut previous_reads = read_count(app.screen());
        let initial_keys = counter(app.screen(), "key updates ");
        for _ in 0..24 {
            app.press("id:stream")
                .expect("append simulated stream delta");
            assert!(app.settle(), "stream does not keep repainting");
            assert_at_end(&app);
            app.press("id:sample")
                .expect("sample streamed height bindings");
            let next_reads = read_count(app.screen());
            assert_eq!(
                counter(app.screen(), "key updates "),
                initial_keys,
                "streaming does not re-hash or diff the key list"
            );
            assert!(
                next_reads - previous_reads < 80,
                "height work is bounded by mounted cells, not 3,000 rows"
            );
            previous_reads = next_reads;
        }
        let mounted = app
            .screen()
            .layout_nodes()
            .into_iter()
            .filter(|n| n.test_id.as_deref() == Some("viewport-row"))
            .count();
        assert!(
            mounted < 80,
            "only viewport and overscan rows mount: {mounted}"
        );
        app.press("id:grow-near-tail")
            .expect("prepare a measured row above the viewport");
        assert!(app.settle());
        assert_at_end(&app);
        let above = bounds(app.screen(), "m-2997");
        assert!(
            above.origin.y + above.size.height <= bounds(app.screen(), "viewport-list").origin.y
        );
        app.press("id:balance-heights")
            .expect("a row above shrinks as the reply grows by the same amount");
        assert!(app.settle());
        assert_at_end(&app);
        app.scroll("id:viewport-list", px(-240.0))
            .expect("user reads above the end");
        app.wait_for("released")
            .expect("follow released by actual input");
        let tail_before = bounds(app.screen(), "tail-row").origin.y;
        app.press("id:stream").expect("stream while reading");
        assert!(app.settle());
        assert_eq!(
            bounds(app.screen(), "tail-row").origin.y,
            tail_before,
            "growing row does not jump"
        );
        app.press("id:end").expect("resume follow");
        app.wait_for("following").expect("follow rearmed");
        assert_at_end(&app);
    }
}

fn read_count(screen: &Screen) -> usize {
    counter(screen, "height reads ")
}

fn counter(screen: &Screen, prefix: &str) -> usize {
    screen
        .texts()
        .into_iter()
        .find_map(|text| text.strip_prefix(prefix).and_then(|n| n.parse().ok()))
        .expect("height read counter")
}

#[test]
fn a_small_upward_wheel_releases_follow_until_the_reader_leaves_and_returns() {
    let mut app = start(DESKTOP);
    // Headless defaults to iOS, whose input driver intentionally has no wheel events.
    app.set_environment("desktop wheel input", |environment| {
        environment.os = nsh_core::platform::Os::Windows
    })
    .expect("desktop input");
    app.wheel("id:viewport-list", point(px(0.0), px(-5.0)))
        .expect("upward input inside the end band");
    app.wait_for("released")
        .expect("input releases even inside the end band");
    let before = bounds(app.screen(), "tail-row").origin.y;
    app.press("id:stream").expect("stream after upward input");
    assert!(app.settle());
    assert_eq!(bounds(app.screen(), "tail-row").origin.y, before);
    app.scroll("id:viewport-list", px(200.0))
        .expect("return to the end band");
    app.wait_for("following")
        .expect("leaving and returning rearms follow");
    assert_at_end(&app);
}

#[test]
fn height_changes_and_prepended_history_preserve_the_visible_row_and_offset() {
    let mut app = start(DESKTOP);
    app.press("id:history").expect("navigate into history");
    app.wait_for("released").expect("history releases follow");
    let before = bounds(app.screen(), "m-1500").origin.y.0;
    app.press("id:grow-above").expect("row above grows");
    assert!(app.settle());
    assert_anchored(&app, before);
    app.press("id:prepend").expect("older page arrives");
    assert!(app.settle());
    assert_anchored(&app, before);
    for _ in 0..12 {
        app.press("id:stream").expect("offscreen stream");
    }
    assert!(app.settle());
    assert_anchored(&app, before);
}

#[test]
fn returning_to_a_thread_restores_the_saved_row_after_history_changes() {
    for size in [DESKTOP, PHONE] {
        let mut app = start(size);
        app.press("id:history").expect("read history");
        let before = bounds(app.screen(), "m-1500").origin.y.0;
        app.press("id:switch").expect("open a fresh thread");
        assert!(app.settle());
        assert_at_end(&app);
        app.press("id:prepend").expect("history loads while away");
        app.press("id:stream").expect("reply grows while away");
        app.press("id:switch").expect("return to remembered thread");
        assert!(app.settle());
        app.wait_for("released")
            .expect("returning to history leaves follow off");
        assert_anchored(&app, before);
    }
}

#[test]
fn a_loading_mount_opens_at_the_last_reply_when_rows_arrive() {
    let mut app = start(PHONE);
    app.press("id:empty").expect("empty projection");
    app.press("id:switch").expect("mount a loading thread");
    app.press("id:empty").expect("hydrated projection");
    assert!(app.settle());
    assert_at_end(&app);
}

#[test]
fn the_indicator_stays_inside_the_safe_area_through_streaming_and_resize() {
    let mut app = start(PHONE);
    app.set_environment("Duo vertical bar", |environment| {
        environment.safe_area.right = px(28.0)
    })
    .expect("system inset");
    for _ in 0..6 {
        app.press("id:stream").expect("stream with system inset");
        assert!(app.settle());
        let frame = bounds(app.screen(), "viewport-list");
        assert_eq!(frame.origin.x + frame.size.width, px(PHONE.0 - 28.0));
        assert_at_end(&app);
    }
    app.set_environment("bar moves", |environment| {
        environment.safe_area.right = px(44.0)
    })
    .expect("updated inset");
    let frame = bounds(app.screen(), "viewport-list");
    assert_eq!(frame.origin.x + frame.size.width, px(PHONE.0 - 44.0));
    app.set_environment("bar leaves", |environment| {
        environment.safe_area.right = px(0.0)
    })
    .expect("remove inset");
    assert_eq!(
        bounds(app.screen(), "viewport-list").size.width,
        px(PHONE.0)
    );
}
