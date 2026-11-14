//! TL-BLOCKS-3: the production block list fed by the client's timeline projection.

use std::rc::Rc;

use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use serde_json::json;
use t3_client::thread_views::messages_timeline::{
    MessagesTimelineRowsInput, MessagesTimelineRowsProjection,
    derive_messages_timeline_rows_with_state,
};
use t3_client::view_logic::session::TimelineEntry;
use t3_ui::runtime::Shared;
use t3_ui::screens::thread::blocks::code::{MAX_HIGHLIGHT_CHUNKS_PER_FRAME, load_code_syntaxes};
use t3_ui::screens::thread::blocks::flatten::BlockKind;
use t3_ui::screens::thread::blocks::list::{
    BlockListModel, BlockListOptions, CellId, messages_timeline,
};

use crate::support::{DESKTOP, PHONE};

const AT: &str = "2026-11-14T12:00:00Z";

fn message(id: &str, source: &str, streaming: bool) -> Rc<TimelineEntry> {
    Rc::new(
        serde_json::from_value(json!({
            "id":id, "kind":"message", "createdAt":AT,
            "message":{"id":id,"role":"assistant","text":source,"streaming":streaming,
                "runId":null,"createdAt":AT,"updatedAt":AT}
        }))
        .expect("canonical timeline message"),
    )
}

fn rows(
    entries: Vec<Rc<TimelineEntry>>,
    old: Option<&MessagesTimelineRowsProjection>,
) -> MessagesTimelineRowsProjection {
    derive_messages_timeline_rows_with_state(
        &MessagesTimelineRowsInput {
            timeline_entries: entries,
            ..Default::default()
        },
        old,
    )
}

fn code(id: &str, line_count: usize, tail: &str, streaming: bool) -> Rc<TimelineEntry> {
    message(
        id,
        &format!(
            "Before\n\n```typescript title=example.ts\n{}{tail}",
            (0..line_count)
                .map(|n| format!("const value{n} = {n};\n"))
                .collect::<String>()
        ),
        streaming,
    )
}

fn cell_with(model: &BlockListModel, predicate: impl Fn(&BlockKind) -> bool) -> usize {
    (0..model.len())
        .find(|&at| {
            model
                .cell(at)
                .and_then(|c| c.leaf)
                .is_some_and(|leaf| predicate(&leaf.block.kind))
        })
        .expect("matching block")
}

#[test]
fn five_hundred_lines_are_thirteen_list_items_and_copy_uses_unmounted_chunks() {
    let projection = rows(vec![code("answer", 500, "```\n\nAfter", false)], None);
    let mut model = BlockListModel::default();
    model.update(&projection.rows);
    let chunks: Vec<_> = (0..model.len())
        .filter_map(|index| model.cell(index)?.leaf)
        .filter(|leaf| matches!(leaf.block.kind, BlockKind::Code(_)))
        .collect();
    assert_eq!(chunks.len(), 13);
    let first = &chunks[0].block.id;
    let value = model.code_text(first).expect("whole fence");
    assert!(value.starts_with("const value0 = 0;\n"));
    assert!(value.ends_with("const value499 = 499;"));
    assert_eq!(
        value,
        chunks
            .iter()
            .filter_map(|leaf| match &leaf.block.kind {
                BlockKind::Code(chunk) => Some(chunk.text.as_str()),
                _ => None,
            })
            .collect::<String>()
    );
    assert_eq!(
        chunks
            .iter()
            .map(|leaf| &leaf.block.id)
            .collect::<std::collections::HashSet<_>>()
            .len(),
        13
    );
    assert!(model.cell(model.len()).is_none());
}

#[test]
fn appends_and_completion_retain_completed_blocks_and_finish_keeps_the_final_splice() {
    let before = rows(vec![code("answer", 82, "let tail", true)], None);
    let mut model = BlockListModel::default();
    model.update(&before.rows);
    let at = cell_with(
        &model,
        |kind| matches!(kind, BlockKind::Code(chunk) if chunk.first),
    );
    let stable = model.cell(at).expect("first chunk").leaf.expect("leaf");
    let next = rows(
        vec![code("answer", 82, "let tail = 42;\n```\n\nDone", false)],
        Some(&before),
    );
    model.update(&next.rows);
    assert_eq!(
        model.cell(at).expect("same chunk").leaf.as_ref(),
        Some(&stable)
    );
    assert!(
        model
            .code_text(&stable.block.id)
            .expect("whole code")
            .ends_with("let tail = 42;")
    );
    assert!(
        (0..model.len())
            .filter_map(|index| model.cell(index)?.leaf)
            .any(|leaf| leaf
                .block
                .text
                .as_deref()
                .is_some_and(|text| text.contains("Done")))
    );
    let document = model
        .document(&stable.block.id.message_id)
        .expect("document");
    assert!(
        document
            .source_range(0..document.source_len())
            .expect("source")
            .ends_with("Done")
    );
}

#[test]
fn reorder_edit_remove_and_reinsert_do_not_reuse_another_messages_content() {
    let a = code("a", 85, "```", false);
    let b = message("b", "Other answer", false);
    let first = rows(vec![a.clone(), b.clone()], None);
    let mut model = BlockListModel::default();
    model.update(&first.rows);
    let first_code = cell_with(&model, |kind| matches!(kind, BlockKind::Code(_)));
    let leaf = model.cell(first_code).expect("code").leaf.expect("leaf");
    let next = rows(vec![b.clone(), a], Some(&first));
    model.update(&next.rows);
    let moved = cell_with(&model, |kind| matches!(kind, BlockKind::Code(_)));
    assert_ne!(first_code, moved);
    assert_eq!(model.cell(moved).expect("moved").leaf.as_ref(), Some(&leaf));
    let edited = rows(vec![code("a", 1, "```", false)], None);
    model.update(&edited.rows);
    let at = cell_with(&model, |kind| matches!(kind, BlockKind::Code(_)));
    let id = model
        .cell(at)
        .expect("edited")
        .leaf
        .expect("leaf")
        .block
        .id
        .clone();
    assert_eq!(model.code_text(&id).as_deref(), Some("const value0 = 0;"));
    model.update(&[]);
    assert!(model.is_empty());
    assert!(model.document(&id.message_id).is_none());
    assert!(model.code_text(&id).is_none());
    model.update(
        &rows(
            vec![message("a", "Old prefix\n\n```typescript\nold()", true)],
            None,
        )
        .rows,
    );
    let replacement = message(
        "a",
        &format!("```typescript\n{}```", "value();\n".repeat(85)),
        false,
    );
    let retired = model.update(&rows(vec![replacement.clone()], None).rows);
    assert!(
        retired
            .iter()
            .any(|id| id.message_id == "a" && id.index == 2),
        "a former fence header reused as a continuation retires its controller"
    );
    let document = model.document("a").expect("replacement document");
    let t3_client::view_logic::session::TimelineContent::Message { message, .. } =
        &replacement.content
    else {
        panic!("message entry");
    };
    assert_eq!(
        document.source_range(0..document.source_len()).as_deref(),
        Some(message.text.as_str())
    );
    model.update(&rows(vec![b], None).rows);
    assert_eq!(model.cell(0).expect("new document").row.id, "b");
}

#[test]
fn sent_message_markdown_and_late_definitions_keep_their_original_source() {
    let source = "[Sent **exactly**][ref]\n\n```text\n😀\n```\n\n[ref]: https://example.test";
    let sent: Rc<TimelineEntry> = Rc::new(
        serde_json::from_value(json!({
            "id":"sent", "kind":"sent-message", "createdAt":AT, "runId":null, "text":source
        }))
        .expect("canonical sent message"),
    );
    let projection = rows(vec![sent], None);
    let mut model = BlockListModel::default();
    model.update(&projection.rows);
    let document = model
        .document("sent")
        .expect("sent message is block-rendered");
    assert_eq!(
        document.source_range(0..document.source_len()).as_deref(),
        Some(source)
    );
    assert!(document.definition("ref", false).is_some());
    assert!(document.context_version() > 0);
    assert!(matches!(
        model.cell(model.len() - 1).expect("footer").id,
        CellId::Footer(_)
    ));
}

#[test]
fn metadata_skips_text_work_and_editing_a_fence_invalidates_continuation_context() {
    let before = rows(vec![code("answer", 85, "```", false)], None);
    let mut model = BlockListModel::default();
    model.update(&before.rows);
    let at = cell_with(
        &model,
        |kind| matches!(kind, BlockKind::Code(chunk) if !chunk.first),
    );
    let original = model.cell(at).expect("continuation").leaf.expect("leaf");
    let mut metadata = before.rows.as_ref().clone();
    let changed = Rc::make_mut(&mut metadata[0]);
    changed.created_at = Some(String::from("2026-11-14T13:00:00Z"));
    let work = model.work();
    model.update(&metadata);
    assert_eq!(model.work().flatten, work.flatten);
    assert_eq!(model.work().inspected_bytes, work.inspected_bytes);
    assert_eq!(
        model.cell(at).expect("same continuation").leaf.as_ref(),
        Some(&original)
    );
    let source = before
        .rows
        .iter()
        .find_map(|row| match &row.content {
            t3_client::thread_views::messages_timeline::MessagesTimelineRowContent::Message {
                message,
                ..
            } => Some(message.text.replacen("value0", "other0", 1)),
            _ => None,
        })
        .expect("message source");
    model.update(&rows(vec![message("answer", &source, false)], None).rows);
    let updated = model
        .cell(at)
        .expect("edited continuation")
        .leaf
        .expect("leaf");
    assert_eq!(updated.block.text, original.block.text);
    assert_ne!(
        updated.lexer_context, original.lexer_context,
        "equal chunk text must not reuse tokens from a different multiline parser entry state"
    );
}

fn measured_appends(prefix_fences: usize) -> (usize, usize, usize) {
    let mut source = "```text\nstable\n```\n\n".repeat(prefix_fences);
    source.push_str("tail");
    let mut projection = rows(vec![message("answer", &source, true)], None);
    let mut model = BlockListModel::default();
    model.update(&projection.rows);
    let before = model.work();
    for _ in 0..128 {
        source.push('x');
        projection = rows(vec![message("answer", &source, true)], Some(&projection));
        model.update(&projection.rows);
    }
    let after = model.work();
    (
        after.flatten.byte_work() - before.flatten.byte_work(),
        after.changed_blocks - before.changed_blocks,
        after.inspected_bytes - before.inspected_bytes,
    )
}

#[test]
fn streamed_delta_work_is_independent_of_the_settled_reply_length() {
    let short = measured_appends(4);
    let long = measured_appends(512);
    eprintln!(
        "TL-BLOCKS-3 counted work (bytes, block updates, derived bytes): short={short:?}, long={long:?}"
    );
    assert_eq!(short, long, "settled prefixes add no per-delta work");
}

fn control(label: &'static str, action: impl Fn() + 'static) -> impl IntoElement {
    nsh_base::button()
        .accessibility_label(Some(SharedString::from(label)))
        .on_press(action)
        .child(text(SharedString::from(label)))
}

#[component]
fn BlockListFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    provide_context(t3_ui::theme::text_view_parts());
    let source = signal(String::from(
        "Stable prose\n\n```typescript\nconst first = 1;\nlet pending",
    ));
    let streaming = signal(true);
    let visible = signal(true);
    let previous: Stored<Option<MessagesTimelineRowsProjection>> = stored(None);
    let rows = computed(move || {
        let input = if visible.get() {
            vec![message("answer", &source.get(), streaming.get())]
        } else {
            Vec::new()
        };
        previous.with(|previous| {
            let next = rows(input, previous.as_ref());
            let rows = Shared(next.rows.clone());
            *previous = Some(next);
            rows
        })
    });
    let options = BlockListOptions::default();
    view()
        .w_full()
        .h_full()
        .child(
            view()
                .flex_row()
                .child(control("Append", move || {
                    source.update(|source| source.push_str(" = 2;"))
                }))
                .child(control("Finish", move || {
                    batch(|| {
                        source.update(|source| source.push_str("\n```\n\nCompleted reply"));
                        streaming.set(false);
                    })
                }))
                .child(control("Long", move || {
                    batch(|| {
                        source.set(format!(
                            "```typescript\n{}const end = 500;\n```",
                            (0..500)
                                .map(|n| format!("const value{n} = {n};\n"))
                                .collect::<String>()
                        ));
                        streaming.set(false);
                    })
                }))
                .child(control("Empty", move || visible.set(false)))
                .child(control("Restore", move || visible.set(true)))
                .child(control("Spacing", move || batch(|| {
                    source.set(String::from("# Initial heading\n\nFirst paragraph\n\n## Next heading\n\nLast paragraph"));
                    streaming.set(false);
                }))),
        )
        .child(messages_timeline(rows, options))
}

fn start(size: (f32, f32)) -> Headless {
    let mut app = Headless::start(EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: nsh_core::geometry::size(px(size.0), px(size.1)),
        ..EngineConfig::new(Root::of::<BlockListFixture>())
    })
    .expect("production block list");
    app.wait_for("let pending").expect("code chunk paints");
    app
}

fn frame_bounds(app: &Headless, chunk: usize) -> Bounds<Points> {
    let nodes = app.screen().layout_nodes();
    let marker = format!("timeline-code-chunk-{chunk}");
    let root = nodes
        .iter()
        .find(|node| node.test_id.as_deref() == Some(marker.as_str()))
        .expect("mounted code chunk")
        .id;
    let mut children = vec![root];
    while let Some(id) = children.pop() {
        let node = nodes
            .iter()
            .find(|node| node.id == id)
            .expect("layout node");
        if id != root && node.clips {
            return node.bounds;
        }
        children.extend(
            nodes
                .iter()
                .rev()
                .filter(|node| node.parent == id)
                .map(|node| node.id),
        );
    }
    panic!("chunk surface clips its content");
}

#[test]
fn streaming_updates_the_tail_without_remounting_completed_prose_at_both_sizes() {
    for size in [PHONE, DESKTOP] {
        let mut app = start(size);
        let stable = app.screen().id("text:Stable prose").expect("prose node");
        app.press("role:button[Append]").expect("append");
        app.wait_for("let pending = 2;").expect("updated tail");
        assert_eq!(
            app.screen()
                .id("text:Stable prose")
                .expect("retained prose"),
            stable
        );
        app.press("role:button[Finish]").expect("finish");
        app.wait_for("Completed reply")
            .expect("final delta rendered");
        assert_eq!(
            app.screen()
                .id("text:Stable prose")
                .expect("completed prose"),
            stable
        );
        app.press("role:button[Spacing]")
            .expect("markdown block boundaries");
        app.wait_for("Last paragraph").expect("all prose blocks");
        let bounds = |label: &str| {
            let id = app
                .screen()
                .id(&format!("text:{label}"))
                .expect("text block");
            app.screen()
                .layout_nodes()
                .into_iter()
                .find(|node| node.id == id)
                .expect("text layout")
                .bounds
        };
        let paragraph = bounds("First paragraph");
        let heading = bounds("Next heading");
        let last = bounds("Last paragraph");
        let rem = nsh_core::geometry::REM_SIZE.0;
        assert!(
            (heading.origin.y.0 - paragraph.origin.y.0 - paragraph.size.height.0 - 1.25 * rem)
                .abs()
                < 1.0,
            "heading has the shared block gap plus heading margin: {paragraph:?}, {heading:?}"
        );
        assert!(
            (last.origin.y.0 - heading.origin.y.0 - heading.size.height.0 - 0.65 * rem).abs() < 1.0,
            "paragraph has the shared block gap: {heading:?}, {last:?}"
        );
    }
}

#[test]
fn one_long_message_is_recycled_by_chunk_and_empty_has_a_reverse_state() {
    for size in [PHONE, DESKTOP] {
        let mut app = start(size);
        app.press("role:button[Long]").expect("long reply");
        app.wait_for("const end = 500;").expect("last code chunk");
        assert!(
            !app.screen().shows("const value0 = 0;"),
            "an entire message was mounted"
        );
        let scrolls = app
            .screen()
            .layout_nodes()
            .iter()
            .filter(|node| node.scroll.is_some())
            .count();
        assert_eq!(scrolls, 1, "code rows introduced a nested scroll container");
        app.scroll("id:timeline", px(-100_000.0))
            .expect("scroll to start");
        app.wait_for("const value0 = 0;")
            .expect("first chunk remounts");
        assert!(app.settle());
        let first = frame_bounds(&app, 1);
        let next = frame_bounds(&app, 2);
        assert!(
            (first.origin.y.0 + first.size.height.0 - next.origin.y.0).abs() < 1.0,
            "a split fence has no interior margin: {first:?}, {next:?}"
        );
        app.press("role:button[Disable line wrap]")
            .expect("disable wrap");
        app.wait_for("Wrap lines").expect("wrap changes");
        app.press("role:button[Copy code]")
            .expect("copy full fence");
        app.wait_for("Copied").expect("copy feedback");
        let expected = format!(
            "{}const end = 500;\n",
            (0..500)
                .map(|n| format!("const value{n} = {n};\n"))
                .collect::<String>()
        );
        assert_eq!(app.screen().clipboard(), Some(expected.as_str()));
        app.scroll("id:timeline", px(100_000.0))
            .expect("recycle header");
        app.wait_for("const end = 500;").expect("tail");
        app.scroll("id:timeline", px(-100_000.0))
            .expect("remount header");
        app.wait_for("Wrap lines")
            .expect("wrap override survives recycling");
        app.press("role:button[Empty]").expect("clear");
        app.wait_for("Send a message to start the conversation.")
            .expect("empty state");
        app.press("role:button[Restore]").expect("restore");
        app.wait_for("const end = 500;")
            .expect("content returns at the live end");
    }
}

#[component]
fn ManyFenceBudgetFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let source = (0..512)
        .map(|fence| {
            format!(
                "```typescript\n{}```\n\n",
                (0..40)
                    .map(|line| format!("const budget_{fence}_{line} = {line};\n"))
                    .collect::<String>()
            )
        })
        .collect::<String>();
    let projection = rows(vec![message("budget", &source, false)], None);
    let visible = signal(false);
    let empty = Shared(Rc::new(Vec::new()));
    let rows = computed(move || {
        if visible.get() {
            Shared(projection.rows.clone())
        } else {
            empty.clone()
        }
    });
    let options = BlockListOptions::default();
    let state = options.state;
    let samples = signal(String::new());
    spawn(async move {
        assert!(load_code_syntaxes().await, "production grammars load");
        visible.set(true);
        let mut observed = Vec::new();
        for _ in 0..4 {
            next_frame().await;
            let work = state.work();
            let queue = work.highlight_queue.expect("mounted timeline queue");
            observed.push([
                work.highlight_fences,
                queue.frames,
                queue.total.parsed_chunks,
                queue.total.parsed_lines,
                queue.last_frame.dequeued_jobs,
                queue.last_frame.visited_chunks,
                queue.last_frame.parsed_chunks,
                queue.last_frame.parsed_lines,
            ]);
        }
        samples.set(format!(
            "budget samples {}",
            serde_json::to_string(&observed).expect("counters")
        ));
    });
    view()
        .w_full()
        .h_full()
        .child(text(move || {
            if samples.with(String::is_empty) {
                "Sampling budget"
            } else {
                "Budget sampled"
            }
        }))
        .child(text(move || samples.get()))
        .child(messages_timeline(rows, options))
}

#[test]
fn five_hundred_twelve_fences_share_one_bounded_frame_budget() {
    let mut app = Headless::start(EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: nsh_core::geometry::size(px(DESKTOP.0), px(DESKTOP.1)),
        ..EngineConfig::new(Root::of::<ManyFenceBudgetFixture>())
    })
    .expect("production timeline with 512 fences");
    app.wait_for("Budget sampled").expect("four frame samples");
    let screen = app.screen();
    let encoded = screen
        .texts()
        .into_iter()
        .find_map(|text| text.strip_prefix("budget samples "))
        .expect("counter samples");
    let samples: Vec<[usize; 8]> = serde_json::from_str(encoded).expect("raw work counters");
    assert_eq!(samples.len(), 4);
    let budget = MAX_HIGHLIGHT_CHUNKS_PER_FRAME;
    for (frame, sample) in samples.iter().enumerate() {
        let [
            fences,
            budgets,
            chunks,
            lines,
            dequeued,
            visited,
            last_chunks,
            last_lines,
        ] = *sample;
        assert_eq!(fences, 512, "offscreen fences were fed above recycling");
        assert!(
            budgets <= frame + 1,
            "multiple budgets spent in one observed frame: {samples:?}"
        );
        assert!(
            chunks <= (frame + 1) * budget,
            "work scaled with fence count: {samples:?}"
        );
        assert!(
            lines <= (frame + 1) * budget * 40,
            "line work exceeded the frame budget"
        );
        assert!(dequeued <= budget && visited <= budget && last_chunks <= budget);
        assert!(last_lines <= budget * 40);
    }
    assert!(
        samples.last().expect("samples")[2] >= 2,
        "the aggregate counter must progress across distinct one-chunk fences: {samples:?}"
    );
    eprintln!("TL-BLOCKS-3 512-fence frame samples: {samples:?}");
}
