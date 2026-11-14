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
    for index in 0..model.len() {
        let key = model.key_at(index).expect("per-index identity");
        assert_eq!(model.index_for_key(&key), Some(index));
    }
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
    let key = model.key_at(first_code).expect("block key");
    let next = rows(vec![b.clone(), a], Some(&first));
    model.update(&next.rows);
    let moved = cell_with(&model, |kind| matches!(kind, BlockKind::Code(_)));
    assert_ne!(first_code, moved);
    assert_eq!(model.index_for_key(&key), Some(moved));
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

fn thread_options() -> BlockListOptions {
    use t3_client::{
        ClientStore,
        types::{EnvironmentId, ThreadId, ThreadKey},
    };
    use t3_ui::runtime::{Rt, backend::Unavailable, host::Choice};
    use t3_ui::screens::thread::rows::{
        approval::{ApprovalContext, ApprovalRowsState},
        plan::{PlanContext, PlanRowsState},
    };
    use t3_ui::state::nav::{Navigator, Route};
    provide_context(Rt::new(
        ClientStore::default(),
        Rc::new(Unavailable("fixture".into())),
        Choice::Demo,
    ));
    provide_context(Navigator::new(Route::Home));
    let key = ThreadKey {
        environment: EnvironmentId("environment".into()),
        thread: ThreadId("blocks".into()),
    };
    let services = t3_ui::screens::thread::timeline::TimelineServices::new(key.clone());
    BlockListOptions::for_thread(
        &services,
        ApprovalContext::new(key.clone(), ApprovalRowsState::default()),
        PlanContext::new(key, PlanRowsState::default()),
    )
}

/// Options for a list outside a thread, built by the production constructor.
fn standalone_options() -> BlockListOptions {
    use t3_client::{
        ClientStore,
        types::{EnvironmentId, ThreadId, ThreadKey},
    };
    use t3_ui::runtime::{Rt, backend::Unavailable, host::Choice};
    use t3_ui::state::nav::{Navigator, Route};
    provide_context(Rt::new(
        ClientStore::default(),
        Rc::new(Unavailable("fixture".into())),
        Choice::Demo,
    ));
    provide_context(Navigator::new(Route::Home));
    BlockListOptions::new(&t3_ui::screens::thread::timeline::TimelineServices::new(
        ThreadKey {
            environment: EnvironmentId("environment".into()),
            thread: ThreadId("standalone".into()),
        },
    ))
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
    let options = thread_options();
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
        app.wait_for("Send a message to start this thread.")
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
    let options = standalone_options();
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

fn mixed_entry(value: serde_json::Value) -> Rc<TimelineEntry> {
    Rc::new(serde_json::from_value(value).expect("canonical mixed timeline entry"))
}

fn mixed_message(id: &str, role: &str, run: &str) -> Rc<TimelineEntry> {
    mixed_entry(json!({"id":id,"kind":"message","createdAt":AT,
        "message":{"id":id,"role":role,"text":id,"runId":run,
        "inputIntent":"turn_start","streaming":false,"createdAt":AT,"updatedAt":AT}}))
}

fn mixed_work(id: &str, run: &str, extra: serde_json::Value) -> Rc<TimelineEntry> {
    let mut entry = json!({"id":id,"runId":run,"createdAt":AT,"label":id,
        "tone":"tool","itemType":"command_execution","toolLifecycleStatus":"completed"});
    entry
        .as_object_mut()
        .expect("work fields")
        .extend(extra.as_object().expect("extra fields").clone());
    mixed_entry(json!({"id":id,"kind":"work","createdAt":AT,"entry":entry}))
}

fn mixed_approval_projection(live: bool) -> t3_client::state::sharing::SharedThreadProjection {
    use t3_contracts::orchestration_v2::OrchestrationV2ThreadProjection;
    let mut source: serde_json::Value = serde_json::from_str(
        include_str!("../../../../fixtures/samples/OrchestrationV2ThreadProjection.jsonl")
            .lines()
            .next()
            .expect("projection sample"),
    )
    .expect("canonical projection");
    source["thread"]["id"] = json!("mixed");
    source["runtimeRequests"] = json!(["matched", "unmatched"].map(|id| json!({
        "id":id,"nodeId":"node","providerTurnId":null,"nativeRequestRef":null,
        "kind":"command","status":"pending","responseCapability":{"type":"live","providerSessionId":"session"},
        "createdAt":AT,"resolvedAt":null
    })));
    let item = json!({"id":"approval-item","threadId":"mixed","runId":null,"nodeId":null,
        "providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,
        "ordinal":0,"status":"waiting","title":null,"startedAt":AT,"completedAt":null,"updatedAt":AT,
        "type":"approval_request","requestId":"matched","requestKind":"command","prompt":"Run mixed command","reason":"Mixed permission"});
    source["turnItems"] = json!([item]);
    source["visibleTurnItems"] = json!([{"position":0,"visibility":"local","sourceThreadId":"mixed","sourceItemId":"approval-item","item":item}]);
    if live {
        source["runtimeRequests"] = json!([]);
        source["turnItems"] = json!([]);
        source["visibleTurnItems"] = json!([]);
    }
    t3_client::state::sharing::SharedThreadProjection::update(
        Rc::new(
            serde_json::from_value::<OrchestrationV2ThreadProjection>(source)
                .expect("mixed requests"),
        ),
        None,
    )
}

fn mixed_input(
    mode: &str,
    projection: &t3_client::state::sharing::SharedThreadProjection,
) -> MessagesTimelineRowsInput {
    use t3_client::thread_views::messages_timeline::TimelineLatestRun;
    use t3_client::view_logic::session::{
        TimelineEntriesInput, derive_timeline_entries_from_visible_turn_items_with_state,
    };
    let mut entries = vec![mixed_message("Mixed user", "user", "run")];
    match mode {
        "mixed" => {
            entries.extend([
                mixed_message("Mixed assistant", "assistant", "run"),
                mixed_work(
                    "Mixed read",
                    "run",
                    json!({"toolTitle":"read","viewedImagePath":"/unrendered-viewed-image.png"}),
                ),
                mixed_work("Mixed command", "run", json!({"command":"vp test run"})),
                mixed_work(
                    "Mixed work notice",
                    "notice-run",
                    json!({"tone":"error","detail":"Mixed work detail"}),
                ),
            ]);
            let approvals = derive_timeline_entries_from_visible_turn_items_with_state(
                TimelineEntriesInput {
                    visible_turn_items: projection.visible_turn_items.to_vec(),
                    ..Default::default()
                },
                None,
            );
            entries.extend(approvals.entries.iter().cloned());
            let item = json!({"id":"event","type":"run_interrupt_result","threadId":"mixed","runId":null,"nodeId":null,
                "providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,
                "status":"completed","title":null,"startedAt":AT,"completedAt":AT,"updatedAt":AT,"message":"Mixed interruption"});
            entries.push(mixed_entry(json!({"id":"event","kind":"event","createdAt":AT,
                "projectedItem":{"position":1,"visibility":"local","sourceThreadId":"mixed","sourceItemId":"event","item":item}})));
            entries.push(mixed_entry(json!({"id":"plan","kind":"proposed-plan","createdAt":AT,
                "proposedPlan":{"id":"plan","runId":null,"planMarkdown":"# Mixed plan\n\nPlan body","status":"completed","createdAt":AT,"updatedAt":AT}})));
            entries.push(mixed_entry(json!({"id":"sent","kind":"sent-message","createdAt":AT,"runId":null,"text":"Mixed sent **message**"})));
            entries.push(mixed_entry(json!({"id":"html","kind":"html-render","createdAt":AT,"runId":null,"htmlRender":{"html":"Unrendered HTML sentinel"}})));
            entries.push(mixed_entry(json!({"id":"mcp","kind":"mcp-app","createdAt":AT,"runId":null,"sourceThreadId":"mixed","itemId":"mcp","revision":"one","mcpApp":{"title":"Unrendered MCP sentinel"}})));
            entries.push(mixed_work(
                "Unrendered compaction sentinel",
                "compact-run",
                json!({"sourceActivityKind":"context-compaction"}),
            ));
        }
        "live" => entries.push(mixed_work(
            "Live command",
            "run",
            json!({"command":"vp test run","toolLifecycleStatus":"inProgress"}),
        )),
        "attempt" => {
            let mut old = serde_json::to_value(mixed_work("Old attempt tool", "run", json!({})))
                .expect("entry");
            old["attempt"] = json!({"id":"old-attempt","runId":"run","attemptOrdinal":1,"rootNodeId":"root",
                "providerInstanceId":"codex","providerThreadId":"provider-thread","providerTurnId":null,
                "reason":"initial","status":"superseded","startedAt":AT,"completedAt":AT});
            entries.push(mixed_entry(old));
            entries.push(mixed_work(
                "Current attempt tool",
                "run",
                json!({"command":"vp test run","toolLifecycleStatus":"inProgress"}),
            ));
        }
        "thinking" => {}
        _ => panic!("unknown data scenario"),
    }
    let live = mode != "mixed";
    MessagesTimelineRowsInput {
        timeline_entries: entries,
        is_working: live,
        running_run_id: live.then(|| "run".into()),
        active_turn_started_at: live.then(|| AT.into()),
        latest_run: Some(TimelineLatestRun {
            run_id: "run".into(),
            status: if live { "running" } else { "completed" }.into(),
            started_at: Some(AT.into()),
            completed_at: (!live).then(|| AT.into()),
            turn_ended: None,
        }),
        worktree_setup: (!live).then(|| json!({"phase":"done","startedAt":AT})),
        tool_call_visibility: "showAll".into(),
        ..Default::default()
    }
}

#[component]
fn MixedRowsFixture() -> impl IntoElement {
    use t3_client::thread_views::composer::send::{PendingMessage, SendState};
    use t3_client::types::{EnvironmentId, ThreadId, ThreadKey};
    use t3_client::{ClientStore, composition::ClientRuntime, testing::FakePlatform};
    use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;
    use t3_ui::runtime::{Rt, backend::Unavailable, host::Choice};
    use t3_ui::screens::composer::send::PendingMessages;
    use t3_ui::screens::thread::rows::approval::{ApprovalContext, ApprovalRowsState};
    use t3_ui::screens::thread::rows::plan::{PlanContext, PlanRowsState};
    use t3_ui::state::nav::{Navigator, Route};
    t3_ui::theme::install(&Default::default());
    provide_context(Rt::new(
        ClientStore::default(),
        Rc::new(Unavailable("fixture".into())),
        Choice::Demo,
    ));
    provide_context(Navigator::new(Route::Home));
    let key = ThreadKey {
        environment: EnvironmentId("environment".into()),
        thread: ThreadId("mixed".into()),
    };
    let mode = signal("mixed");
    let services = t3_ui::screens::thread::timeline::TimelineServices::new(key.clone());
    let groups = services.groups;
    groups
        .expanded_run_ids
        .set(["run".into(), "notice-run".into()].into_iter().collect());
    let projection = computed(move || {
        Some(Shared(Rc::new(mixed_approval_projection(
            mode.get() != "mixed",
        ))))
    });
    let approvals =
        ApprovalContext::from_projection(key.clone(), projection, ApprovalRowsState::default());
    let pending_approvals = approvals.pending;
    let rows = computed(move || {
        projection.with(|projection| {
            let mut input = mixed_input(mode.get(), projection.as_ref().expect("projection"));
            groups.apply_to_input(&mut input);
            input.pending_approval_request_ids = pending_approvals.with(|pending| {
                pending
                    .values()
                    .filter_map(|pending| match &pending.request.id {
                        t3_contracts::orchestration_v2_id_table::IdOrIndex::Id(id) => {
                            Some(id.0.clone())
                        }
                        _ => None,
                    })
                    .collect()
            });
            Shared(derive_messages_timeline_rows_with_state(&input, None).rows)
        })
    });
    let inventory = computed(move || {
        rows.with(|rows| {
            let kinds = rows
                .iter()
                .map(|row| {
                    let value = serde_json::to_value(row.as_ref()).expect("row");
                    if value["kind"] == "message" {
                        format!(
                            "message:{}",
                            value["message"]["role"].as_str().expect("role")
                        )
                    } else {
                        value["kind"].as_str().expect("kind").to_owned()
                    }
                })
                .collect::<std::collections::BTreeSet<_>>();
            format!(
                "mixed inventory {}",
                serde_json::to_string(&kinds).expect("inventory")
            )
        })
    });
    let mut options = BlockListOptions::for_thread(
        &services,
        approvals,
        PlanContext::new(key.clone(), PlanRowsState::default()),
    );
    options.user.working = computed(move || mode.get() != "mixed");
    let client = Rc::new(ClientRuntime::new(
        FakePlatform::default(),
        ClientStore::default(),
        OrchestrationV2CreationSource::Web,
        Rc::new(|_| {}),
    ));
    let pending = PendingMessage {message:Rc::new(serde_json::from_value(json!({
        "environmentId":"environment","threadId":"mixed","messageId":"pending","commandId":"pending-command",
        "text":"Mixed pending message","attachments":[],"createdAt":AT})).expect("canonical queued message")), state:SendState::Queued};
    let pending = PendingMessages {
        messages: signal(Rc::new(vec![pending])),
        restored_draft: signal(None),
        delivery_problem: signal(None),
    };
    options.thread.as_mut().expect("thread mount").pending = Some((pending, client));
    view()
        .w_full()
        .h_full()
        .child(control("Mixed", move || mode.set("mixed")))
        .child(control("Live", move || mode.set("live")))
        .child(control("Thinking", move || mode.set("thinking")))
        .child(control("Attempt", move || mode.set("attempt")))
        .child(control("Fold", move || {
            groups.expanded_run_ids.set(Default::default())
        }))
        .child(text(move || inventory.get()))
        .child(messages_timeline(rows, options))
}

#[test]
fn every_client_row_kind_uses_its_owner_or_an_explicit_unrendered_arm() {
    let mut app = Headless::start(EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(1280.0), px(6000.0)),
        ..EngineConfig::new(Root::of::<MixedRowsFixture>())
    })
    .expect("mixed production block timeline");
    app.wait_for("Mixed pending message")
        .expect("pending footer");
    let mut observed = std::collections::BTreeSet::<String>::new();
    for (control, expected) in [
        (
            "Mixed",
            vec![
                "Mixed user",
                "Mixed assistant",
                "Mixed work detail",
                "Working...",
                "Mixed interruption",
                "Mixed plan",
                "Mixed sent message",
                "Run mixed command",
            ],
        ),
        ("Fold", vec!["id:turn-fold:run"]),
        ("Live", vec!["Running vp"]),
        ("Thinking", vec!["Thinking"]),
        ("Attempt", vec!["Cut off by a steer"]),
    ] {
        app.press(&format!("role:button[{control}]"))
            .expect("data scenario");
        for text in expected {
            app.wait_for(text)
                .unwrap_or_else(|error| panic!("{control}: {text}: {error:?}"));
        }
        if control == "Mixed" {
            let screen = app.screen();
            let shown = screen.texts();
            let approval = shown
                .iter()
                .position(|&text| text == "Run mixed command")
                .expect("matched approval");
            let event = shown
                .iter()
                .position(|&text| text == "Mixed interruption")
                .expect("following event");
            assert!(
                approval < event,
                "matched approval must render in its work row, before the event and footer"
            );
            assert_eq!(
                app.screen()
                    .texts()
                    .iter()
                    .filter(|text| **text == "Command approval")
                    .count(),
                1,
                "unmatched footer mounts its generic owner card; the matched row shows its prompt"
            );
            assert_eq!(
                app.screen()
                    .count("role:button[Copy message]")
                    .expect("owner copy actions"),
                2,
                "user and standalone assistant meta each expose their owner action"
            );
            app.press("role:button[Copy message] >> nth=1")
                .expect("standalone assistant meta action");
            assert_eq!(app.screen().clipboard(), Some("Mixed assistant"));
            app.press("id:work-toggle:Mixed read")
                .expect("owner group disclosure");
            app.wait_for("vp test run").expect("owner work detail");
        }
        let screen = app.screen();
        let encoded = screen
            .texts()
            .into_iter()
            .find_map(|text| text.strip_prefix("mixed inventory "))
            .expect("raw client kind inventory");
        observed.extend(serde_json::from_str::<Vec<String>>(encoded).expect("kinds"));
        for absent in [
            "Unrendered HTML sentinel",
            "Unrendered MCP sentinel",
            "Unrendered compaction sentinel",
            "/unrendered-viewed-image.png",
        ] {
            assert!(
                !screen.shows(absent),
                "unavailable row content was invented: {absent}"
            );
        }
    }
    let mounted = [
        "message:user",
        "message:assistant",
        "assistant-meta",
        "sent-message",
        "event",
        "proposed-plan",
        "work",
        "work-live",
        "working",
        "thinking",
        "work-toggle",
        "turn-fold",
        "attempt-fold",
    ];
    let unrendered = [
        "worktree-setup",
        "viewed-images",
        "context-compaction",
        "html-render",
        "mcp-app",
    ];
    for kind in mounted.into_iter().chain(unrendered) {
        assert!(
            observed.contains(kind),
            "client fixture did not exercise {kind}: {observed:?}"
        );
    }
    assert_eq!(
        app.screen()
            .layout_nodes()
            .iter()
            .filter(|node| node.scroll.is_some())
            .count(),
        1,
        "viewport/footer share the block list's single scroller"
    );
    eprintln!("TL-BLOCKS-3 mounted kinds: {mounted:?}; explicitly unrendered: {unrendered:?}");
}

/// The thread screen's `timeline()` mounts this list: a long reply in the store is many virtual
/// cells (code chunks with their own cell markers), not one row.
#[component]
fn ProductionTimelineFixture() -> impl IntoElement {
    use t3_client::ClientStore;
    use t3_client::types::{EnvironmentId, ThreadId, ThreadKey};
    use t3_contracts::orchestration_v2::OrchestrationV2ThreadStreamItem;
    use t3_ui::runtime::{Rt, backend::Unavailable, host::Choice};
    use t3_ui::state::nav::{Navigator, Route};
    t3_ui::theme::install(&Default::default());
    let store = ClientStore::default();
    let environment = EnvironmentId("environment".into());
    let session = store.begin_session(environment.clone());
    let key = ThreadKey {
        environment,
        thread: ThreadId("long".into()),
    };
    let code = (0..500)
        .map(|line| format!("const production_{line} = {line};\n"))
        .collect::<String>();
    let text = format!("Here it is:\n\n```ts\n{code}```\n");
    let item = json!({"id":"answer","threadId":"long","type":"assistant_message","messageId":"answer",
        "text":text,"streaming":false,"runId":"run","nodeId":null,"providerThreadId":null,
        "providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,
        "status":"completed","title":null,"startedAt":AT,"completedAt":AT,"updatedAt":AT});
    let thread = json!({"id":"long","projectId":"project","title":"Long","providerInstanceId":"codex",
        "modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access",
        "interactionMode":"default","branch":null,"worktreePath":null,"activeProviderThreadId":null,
        "lineage":{"rootThreadId":"long","parentThreadId":null,"relationshipToParent":null},
        "forkedFrom":null,"createdBy":"user","creationSource":"web","createdAt":AT,"updatedAt":AT,
        "archivedAt":null,"settledOverride":null,"settledAt":null,"lastVisitedAt":null,"deletedAt":null});
    let snapshot: OrchestrationV2ThreadStreamItem = serde_json::from_value(json!({
        "kind":"snapshot","snapshotSequence":1,
        "projection":{"thread":thread,"runs":[],"attempts":[],"nodes":[],"subagents":[],
            "providerSessions":[],"providerThreads":[],"providerTurns":[],"runtimeRequests":[],
            "messages":[],"plans":[],"turnItems":[item],"checkpointScopes":[],"checkpoints":[],
            "contextHandoffs":[],"contextTransfers":[],
            "visibleTurnItems":[{"position":0,"visibility":"local","sourceThreadId":"long","sourceItemId":"answer","item":item}],
            "updatedAt":AT}
    }))
    .expect("canonical thread snapshot");
    store
        .apply_thread(key.clone(), session, vec![snapshot])
        .expect("seeded thread");
    provide_context(Rt::new(
        store,
        Rc::new(Unavailable("fixture".into())),
        Choice::Demo,
    ));
    provide_context(Navigator::new(Route::Thread(key.clone())));
    view()
        .w_full()
        .h_full()
        .child(t3_ui::screens::thread::timeline::timeline(key))
}

#[test]
fn the_thread_timeline_mounts_a_long_reply_as_block_cells() {
    let mut sets = t3_ui::registries();
    let mut fixtures = Registry::new();
    fixtures.register::<ProductionTimelineFixture>();
    sets.push(fixtures);
    let mut app = Headless::start(EngineConfig {
        ui_sets: sets,
        viewport: size(px(DESKTOP.0), px(DESKTOP.1)),
        ..EngineConfig::new(Root::of::<ProductionTimelineFixture>())
    })
    .expect("fixture");
    app.wait_for("text:/const production_/")
        .expect("reply mounted");
    let chunks = app
        .screen()
        .layout_nodes()
        .into_iter()
        .filter(|node| {
            node.test_id
                .as_deref()
                .is_some_and(|id| id.starts_with("timeline-code-chunk-"))
        })
        .count();
    assert!(
        chunks >= 2,
        "a 500-line fence is split into chunk cells, saw {chunks}"
    );
}
