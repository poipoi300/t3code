//! Port of apps/web/src/components/chat/MessagesTimeline.tsx find integration,
//! ThreadFindBar.tsx ThreadFindBar and threadFind.ts.

use std::collections::BTreeMap;
use std::ops::Range;
use std::rc::Rc;

use nsh_core::prelude::*;
use t3_client::thread_views::threads::find::{
    find_thread_search_occurrences, format_thread_find_count, markdown_segments, rendered_segments,
    step_thread_find_index,
};
use t3_contracts::orchestration_v2::SearchThreadInputSkillsItem;

use super::blocks::flatten::{Block, BlockDelta, BlockId, BlockKind, SourceRange, StreamBlocks};
use super::viewport::{TimelineFollow, TimelineNavigation, TimelineViewport};
use crate::i18n::{t, ts};
use crate::runtime::tasks::TaskOwner;
use crate::theme::{cn, tw};
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};
use crate::ui::icon::Icon;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FindKind {
    User,
    Assistant,
    Plan,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreadFindMatch {
    pub order: usize,
    pub block: BlockId,
    pub occurrence: usize,
    pub segment: usize,
    /// UTF-16 positions in the rendered segment, never Markdown source offsets.
    pub rendered: Range<usize>,
    pub source: SourceRange,
}

struct SearchBlock {
    block: Rc<Block>,
    segments: Vec<String>,
    hits: Vec<(usize, Range<usize>)>,
}

struct Document {
    kind: FindKind,
    source: StreamBlocks,
    blocks: BTreeMap<usize, SearchBlock>,
    definitions: BTreeMap<(bool, String), Rc<str>>,
    dirty: bool,
}

/// All source blocks remain searchable after their native cells are recycled.
#[derive(Default)]
pub struct ThreadFindIndex {
    documents: BTreeMap<(usize, String), Document>,
    query: String,
    count: usize,
    normalized_bytes: usize,
    unsettled: usize,
}

impl ThreadFindIndex {
    pub fn count(&self) -> usize {
        self.count
    }
    pub fn normalized_bytes(&self) -> usize {
        self.normalized_bytes
    }
    pub fn counting(&self) -> bool {
        self.unsettled > 0
    }

    /// Feed the flattener's explicit replacement/append delta, including unmounted blocks.
    pub fn apply(
        &mut self,
        order: usize,
        id: &str,
        kind: FindKind,
        delta: &BlockDelta,
        cwd: Option<&str>,
        skills: &[SearchThreadInputSkillsItem],
    ) {
        let document = self
            .documents
            .entry((order, id.into()))
            .or_insert_with(|| Document {
                kind,
                source: delta.document.clone(),
                blocks: BTreeMap::new(),
                definitions: BTreeMap::new(),
                dirty: false,
            });
        if document.source != delta.document || document.kind != kind {
            self.unsettled -= usize::from(document.dirty);
            self.count -= document
                .blocks
                .values()
                .map(|b| b.hits.len())
                .sum::<usize>();
            document.blocks.clear();
            document.definitions.clear();
            document.dirty = false;
        }
        document.kind = kind;
        document.source = delta.document.clone();
        for block in document.blocks.split_off(&delta.changed_from).into_values() {
            self.count -= block.hits.len();
        }
        for invalidation in &delta.invalidations {
            let key = (invalidation.footnote, invalidation.identifier.clone());
            if let Some(definition) = document.source.definition(&key.1, key.0) {
                document.definitions.insert(key, definition.source.clone());
            } else {
                document.definitions.remove(&key);
            }
            // Reconcile references on a user action, never scan settled source per URL delta.
            if !document.dirty {
                self.unsettled += 1;
                document.dirty = true;
            }
        }
        for block in &delta.rows {
            let segments = normalize(block, kind, cwd, skills, &[]);
            self.normalized_bytes += block.text.as_ref().map_or(0, |text| text.len());
            let hits = hits(&segments, &self.query);
            self.count += hits.len();
            document.blocks.insert(
                block.id.index,
                SearchBlock {
                    block: block.clone(),
                    segments,
                    hits,
                },
            );
        }
    }

    /// Query changes are explicit user actions; completed blocks retain their parsed text.
    pub fn search(
        &mut self,
        query: &str,
        cwd: Option<&str>,
        skills: &[SearchThreadInputSkillsItem],
    ) {
        self.query = t3_contracts::wire::trim_js(query).into();
        self.count = 0;
        for document in self.documents.values_mut() {
            let definitions: Vec<_> = document.definitions.values().cloned().collect();
            for block in document.blocks.values_mut() {
                if document.dirty {
                    block.segments =
                        normalize(&block.block, document.kind, cwd, skills, &definitions);
                    self.normalized_bytes += block.block.text.as_ref().map_or(0, |text| text.len());
                }
                block.hits = hits(&block.segments, &self.query);
                self.count += block.hits.len();
            }
            document.dirty = false;
        }
        self.unsettled = 0;
    }

    /// A stream settles once; late definitions are reconciled then, never on each delta.
    pub fn finish(&mut self, order: usize, id: &str) {
        let Some(document) = self.documents.get_mut(&(order, id.into())) else {
            return;
        };
        if !document.dirty {
            return;
        }
        let definitions: Vec<_> = document.definitions.values().cloned().collect();
        for block in document.blocks.values_mut() {
            self.count -= block.hits.len();
            block.segments = normalize(&block.block, document.kind, None, &[], &definitions);
            self.normalized_bytes += block.block.text.as_ref().map_or(0, |text| text.len());
            block.hits = hits(&block.segments, &self.query);
            self.count += block.hits.len();
        }
        document.dirty = false;
        self.unsettled -= 1;
    }

    pub fn retain_order(&mut self, ids: &[String]) {
        let mut previous: BTreeMap<_, _> = std::mem::take(&mut self.documents)
            .into_iter()
            .map(|((_, id), document)| (id, document))
            .collect();
        self.documents = ids
            .iter()
            .enumerate()
            .filter_map(|(order, id)| {
                previous
                    .remove(id)
                    .map(|document| ((order, id.clone()), document))
            })
            .collect();
        self.count = self
            .documents
            .values()
            .flat_map(|document| document.blocks.values())
            .map(|block| block.hits.len())
            .sum();
        self.unsettled = self
            .documents
            .values()
            .filter(|document| document.dirty)
            .count();
    }

    pub fn remove(&mut self, order: usize, id: &str) {
        if let Some(document) = self.documents.remove(&(order, id.into())) {
            self.count -= document
                .blocks
                .values()
                .map(|b| b.hits.len())
                .sum::<usize>();
            self.unsettled -= usize::from(document.dirty);
        }
    }

    pub fn match_at(&self, mut index: usize) -> Option<ThreadFindMatch> {
        for ((order, _), document) in &self.documents {
            let mut occurrence = 0;
            for block in document.blocks.values() {
                if index < block.hits.len() {
                    let (segment, rendered) = &block.hits[index];
                    return Some(ThreadFindMatch {
                        order: *order,
                        block: block.block.id.clone(),
                        occurrence: occurrence + index,
                        segment: *segment,
                        rendered: rendered.clone(),
                        source: block.block.source.clone(),
                    });
                }
                index -= block.hits.len();
                occurrence += block.hits.len();
            }
        }
        None
    }

    pub fn index_of(&self, target: &ThreadFindMatch) -> Option<usize> {
        let mut index = 0;
        for document in self.documents.values() {
            for block in document.blocks.values() {
                if block.block.id == target.block {
                    return block
                        .hits
                        .iter()
                        .position(|(segment, range)| {
                            *segment == target.segment && *range == target.rendered
                        })
                        .map(|at| index + at);
                }
                index += block.hits.len();
            }
        }
        None
    }

    fn tail_anchor(
        &self,
        target: &ThreadFindMatch,
        changed_from: usize,
    ) -> Option<(usize, SourceRange)> {
        let document = self
            .documents
            .get(&(target.order, target.block.message_id.clone()))?;
        let block = document.blocks.get(&target.block.index)?;
        let local = block
            .hits
            .iter()
            .position(|(segment, range)| *segment == target.segment && *range == target.rendered)?;
        let before: usize = document
            .blocks
            .range(changed_from..target.block.index)
            .map(|(_, block)| block.hits.len())
            .sum();
        Some((before + local, block.block.source.clone()))
    }

    fn first_in_tail(&self, order: usize, id: &str, from: usize) -> Option<ThreadFindMatch> {
        let document = self.documents.get(&(order, id.to_owned()))?;
        document.blocks.range(from..).find_map(|(_, block)| {
            block
                .hits
                .first()
                .map(|(segment, rendered)| ThreadFindMatch {
                    order,
                    block: block.block.id.clone(),
                    occurrence: 0,
                    segment: *segment,
                    rendered: rendered.clone(),
                    source: block.block.source.clone(),
                })
        })
    }

    fn first_at_or_after(&self, row: usize, resolve: &dyn Fn(&BlockId) -> Option<usize>) -> usize {
        let mut at = 0;
        for document in self.documents.values() {
            for block in document.blocks.values() {
                if !block.hits.is_empty()
                    && resolve(&block.block.id).is_some_and(|index| index >= row)
                {
                    return at;
                }
                at += block.hits.len();
            }
        }
        0
    }
}

fn normalize(
    block: &Block,
    kind: FindKind,
    cwd: Option<&str>,
    skills: &[SearchThreadInputSkillsItem],
    definitions: &[Rc<str>],
) -> Vec<String> {
    match &block.kind {
        BlockKind::Header | BlockKind::Definition { .. } | BlockKind::Output { .. } => Vec::new(),
        BlockKind::Code(code) => vec![code.text.clone()],
        _ => {
            let mut source = block.text.as_deref().unwrap_or_default().to_owned();
            for definition in definitions {
                source.push_str("\n\n");
                source.push_str(definition);
            }
            if kind == FindKind::Plan && block.id.index > 1 {
                return markdown_segments(&source, false, cwd, &[]);
            }
            let item = match kind {
                FindKind::User => serde_json::json!({"type":"user_message", "text":source}),
                FindKind::Assistant => {
                    serde_json::json!({"type":"assistant_message", "text":source, "streaming":true})
                }
                FindKind::Plan => serde_json::json!({"type":"proposed_plan", "markdown":source}),
            };
            rendered_segments(&item, cwd, skills)
        }
    }
}

fn hits(segments: &[String], query: &str) -> Vec<(usize, Range<usize>)> {
    let length = query.encode_utf16().count();
    segments
        .iter()
        .enumerate()
        .flat_map(|(segment, text)| {
            find_thread_search_occurrences(text, query)
                .into_iter()
                .map(move |start| (segment, start..start + length))
        })
        .collect()
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FindState {
    pub open: bool,
    pub query: String,
    pub count: usize,
    pub active_index: usize,
    pub active: Option<ThreadFindMatch>,
    pub focus_request: u64,
    pub counting: bool,
}

type BlockRowIndex = Rc<dyn Fn(&BlockId) -> Option<usize>>;

/// Construct above the list; the only resolver is its existing block-to-list-index map.
#[derive(Clone, Copy)]
pub struct ThreadFind {
    pub state: Signal<FindState>,
    index: Stored<ThreadFindIndex>,
    viewport: TimelineViewport,
    row_index: Stored<BlockRowIndex>,
    input: ElementRef,
    request: Signal<u64>,
    tasks: TaskOwner,
}

impl ThreadFind {
    pub fn new(
        viewport: TimelineViewport,
        row_index: impl Fn(&BlockId) -> Option<usize> + 'static,
    ) -> Self {
        Self::with_state(signal(FindState::default()), viewport, row_index)
    }

    /// Over a state the caller created first, so rows built before the list can read it.
    pub fn with_state(
        state: Signal<FindState>,
        viewport: TimelineViewport,
        row_index: impl Fn(&BlockId) -> Option<usize> + 'static,
    ) -> Self {
        let this = Self {
            state,
            index: stored(ThreadFindIndex::default()),
            viewport,
            row_index: stored(Rc::new(row_index)),
            input: element_ref(),
            request: signal(0),
            tasks: TaskOwner::here(),
        };
        let open = computed(move || this.state.with(|state| state.open));
        // Find suspends the external viewport's live follow, including end-band re-arming.
        effect(move || {
            if open.get() && viewport.follow.get() == TimelineFollow::Following {
                viewport.navigate(TimelineNavigation::Command {
                    releases_follow: true,
                });
            }
        });
        this
    }
    pub fn apply(self, order: usize, id: &str, kind: FindKind, delta: &BlockDelta) {
        let before = self.state.get_untracked();
        let found_first = self.index.with(|index| {
            let old_count = index.count();
            let affected = before
                .active
                .as_ref()
                .filter(|active| active.order == order && active.block.index >= delta.changed_from);
            let old_anchor =
                affected.and_then(|active| index.tail_anchor(active, delta.changed_from));
            index.apply(order, id, kind, delta, None, &[]);
            let first = (old_count == 0 && index.count() > 0)
                .then(|| index.first_in_tail(order, id, delta.changed_from))
                .flatten();
            self.state.update(|state| {
                state.count = index.count();
                state.counting = index.counting();
                if let Some(active) = state.active.as_mut() {
                    if active.order > order {
                        state.active_index = state
                            .active_index
                            .saturating_add(index.count())
                            .saturating_sub(old_count);
                    } else if let Some((old, _)) = old_anchor {
                        if let Some((new, source)) = index.tail_anchor(active, delta.changed_from) {
                            state.active_index =
                                state.active_index.saturating_add(new).saturating_sub(old);
                            active.occurrence =
                                active.occurrence.saturating_add(new).saturating_sub(old);
                            active.source = source;
                        } else {
                            state.active = None;
                        }
                    }
                }
                state.active_index = state.active_index.min(state.count.saturating_sub(1));
                if let Some(first) = first {
                    state.active = Some(first);
                    state.active_index = 0;
                }
            });
            old_count == 0 && index.count() > 0
        });
        if found_first {
            self.reveal_active();
        }
    }
    pub fn finish(self, order: usize, id: &str) {
        self.index.with(|index| index.finish(order, id));
        self.refresh_selection(false);
    }
    pub fn retain_order(self, ids: &[String]) {
        self.index.with(|index| index.retain_order(ids));
        self.refresh_selection(false);
    }
    pub fn open(self) {
        if !self.state.with_untracked(|s| s.open) {
            self.viewport.navigate(TimelineNavigation::Command {
                releases_follow: true,
            });
        }
        self.state.update(|state| {
            state.open = true;
            state.focus_request += 1;
        });
        if self
            .state
            .with_untracked(|state| !state.query.trim().is_empty())
        {
            self.restart_from_viewport();
        }
    }
    pub fn close(self) {
        self.request.update(|request| *request += 1);
        self.state.set(FindState::default());
        self.index.with(|index| index.search("", None, &[]));
        self.viewport.element.focus();
    }
    pub fn set_query(self, query: &str) {
        let mut units = 0;
        let query: String = query
            .chars()
            .take_while(|ch| {
                units += ch.len_utf16();
                units <= 200
            })
            .collect();
        self.index.with(|index| index.search(&query, None, &[]));
        self.state.update(|state| {
            state.query = query;
            state.active_index = 0;
            state.active = None;
        });
        self.refresh_selection(false);
        self.restart_from_viewport();
    }
    pub fn step(self, delta: isize) {
        if !self
            .state
            .with_untracked(|s| s.open && s.count > 0 && !s.counting)
        {
            return;
        }
        self.request.update(|request| *request += 1);
        self.state.update(|state| {
            if state.active.is_some() {
                state.active_index = step_thread_find_index(state.active_index, state.count, delta);
            }
            state.active = None;
        });
        self.refresh_selection(true);
    }
    pub fn find_active(self) -> Memo<bool> {
        computed(move || {
            self.state
                .with(|s| s.open && !t3_contracts::wire::trim_js(&s.query).is_empty())
        })
    }
    /// Retry after the block list materializes a previously folded or newly loaded target.
    pub fn reveal_active(self) {
        if self.state.with_untracked(|state| state.open)
            && let Some(active) = self.state.with_untracked(|state| state.active.clone())
            && let Some(index) = self.row_index.with(|resolve| resolve(&active.block))
        {
            self.viewport.scroll_to_index(index, px(-96.0));
        }
    }
    fn restart_from_viewport(self) {
        self.request.update(|request| *request += 1);
        let request = self.request.get_untracked();
        self.tasks.spawn(async move {
            let position = self.viewport.element.measure_scroll_position().await;
            if self.request.get_untracked() != request
                || !self.state.with_untracked(|state| state.open)
            {
                return;
            }
            let at = position.map_or(0, |position| {
                self.row_index.with(|resolve| {
                    self.index.with(|index| {
                        index.first_at_or_after(position.index as usize, resolve.as_ref())
                    })
                })
            });
            self.state.update(|state| {
                state.active_index = at;
                state.active = None;
            });
            self.refresh_selection(true);
        });
    }
    fn refresh_selection(self, navigate: bool) {
        let before = self.state.get_untracked();
        let (count, counting, active_index, active) = self.index.with(|index| {
            let active_index = before
                .active
                .as_ref()
                .and_then(|m| index.index_of(m))
                .unwrap_or(before.active_index)
                .min(index.count().saturating_sub(1));
            (
                index.count(),
                index.counting(),
                active_index,
                index.match_at(active_index),
            )
        });
        self.state.update(|state| {
            state.count = count;
            state.counting = counting;
            state.active_index = active_index;
            state.active = active.clone();
        });
        if navigate
            && before.open
            && let Some(active) = active
            && let Some(index) = self.row_index.with(|resolve| resolve(&active.block))
        {
            self.viewport.scroll_to_index(index, px(-96.0));
        }
    }
}

/// Port of ThreadFindBar.tsx ThreadFindBar.
pub fn thread_find_bar(find: ThreadFind) -> impl IntoElement {
    let available_width = signal(0.0_f32);
    let shift = signal(false);
    let composing = signal(false);
    let focus = computed(move || find.state.with(|state| (state.open, state.focus_request)));
    // Synchronizes native input focus with the web's open/refocus request.
    effect(move || {
        let (open, _) = focus.get();
        if open {
            after_answer(move || {
                find.input.focus();
                find.input.set_selection(
                    0,
                    find.state
                        .with_untracked(|s| s.query.encode_utf16().count() as u32),
                );
            });
        }
    });
    when(
        move || find.state.with(|state| state.open),
        move || {
            // No InputGroup primitive exists yet: these are its popover/lg and pill classes.
            // NSH has no CSS starting: mount transition; the bar mounts at full opacity.
            // NSH cannot parse min(token, calc(100%-1.5rem)); measure the inset native wrapper.
            view().app_class(tw("absolute top-3 right-3 left-3 z-40 transition-opacity duration-150 ease-out motion-reduce:transition-none"))
            .pointer_events(PointerEvents::BoxNone)
            .on_layout(move |bounds: &Bounds<Points>| available_width.set(bounds.size.width.0))
            .test_id("thread-find-bar")
            .child(view().app_class(tw("relative inline-flex w-full min-w-0 items-center rounded-2xl border h-9 text-base text-foreground sm:text-sm backdrop-blur-(--glass-blur) dark:backdrop-blur-[16px] backdrop-saturate-[1.14] dark:backdrop-saturate-[1.08]"))
                .self_end()
                .w(move || px(tokens().number("thread-details-panel-width").unwrap_or(280.0).min(available_width.get())))
                .bg(crate::ui::popover::dropdown_glass_background)
                // index.css @utility dropdown-glass supplies the contrast border as well as blur.
                .border_color(move || tokens().color("contrast-foreground").unwrap_or_default().opacity(0.1))
                .child(nsh_base::input().element_ref(&find.input)
                    .value(move || find.state.with(|state| state.query.clone()))
                    .on_change(move |query: &str| find.set_query(query))
                    .placeholder(t("Find in thread")).label(ts("Find in thread"))
                    .test_id("thread-find-input")
                    .app_class(tw("min-w-0 flex-1 bg-transparent px-2.5 pe-1.5 text-sm"))
                    // Native submit excludes an IME composition Enter; raw keys do not.
                    .on_composition_change(move |event: &nsh_core::element::Composition| composing.set(event.composing))
                    .on_submit(move |_: &str| { if !composing.get_untracked() { find.step(if shift.get_untracked() { -1 } else { 1 }); } })
                    .on_key_up(move |event: &KeyEvent| shift.set(event.modifiers.shift))
                    .on_key_down(move |event: &KeyEvent| {
                        shift.set(event.modifiers.shift);
                        if event.key.as_str() == "Escape" { find.close(); }
                    }))
                // This is already the last sibling; NSH has no CSS order property.
                .child(view().app_class(tw("flex h-auto items-center justify-center gap-0.5 pe-1 leading-none"))
                    .child(text(move || find.state.with(|s| if s.query.trim().is_empty() { String::new() } else if s.counting { t("1/…") } else { format_thread_find_count(s.active_index, s.count) }))
                        .app_class(move || Some(tw(&cn([Some("min-w-10 text-center text-xs tabular-nums"), Some(if find.state.with(|s| !s.query.trim().is_empty() && s.count == 0) { "text-destructive" } else { "text-muted-foreground" })])))))
                    .child(button(ButtonContent::Icon { icon: Icon::ChevronUp, label: t("Previous match") }, ButtonVariant::Ghost, ButtonSize::IconXs, move || find.state.with(|s| s.count == 0 || s.counting), move || find.step(-1)))
                    .child(button(ButtonContent::Icon { icon: Icon::ChevronDown, label: t("Next match") }, ButtonVariant::Ghost, ButtonSize::IconXs, move || find.state.with(|s| s.count == 0 || s.counting), move || find.step(1)))
                    .child(button(ButtonContent::Icon { icon: Icon::X, label: t("Close find") }, ButtonVariant::Ghost, ButtonSize::IconXs, || false, move || find.close()))))
        },
    )
}

#[cfg(test)]
mod tests {
    use super::super::blocks::flatten::MessageFlattener;
    use super::*;

    #[test]
    fn finds_rendered_text_in_unmounted_blocks_and_keeps_utf16_ranges() {
        let source = format!(
            "😀 **Café** and CAFÉ\n\n```rs\n{}needle\n```\n\n[shown](https://hidden.test)",
            "line\n".repeat(500)
        );
        let mut flattener = MessageFlattener::new("reply");
        let delta = flattener.replace("reply", &source);
        let mut index = ThreadFindIndex::default();
        index.apply(0, "reply", FindKind::Assistant, &delta, None, &[]);
        index.search("café", None, &[]);
        assert_eq!(index.count(), 2);
        assert_eq!(index.match_at(0).expect("first").rendered, 3..7);
        index.search("needle", None, &[]);
        let found = index.match_at(0).expect("unmounted code chunk");
        assert!(found.block.index > 10);
        assert!(
            delta
                .document
                .source_range(found.source.bytes)
                .expect("source")
                .contains("needle")
        );
        index.search("hidden.test", None, &[]);
        assert_eq!(index.count(), 0, "link targets are not displayed text");
        index.search("shown", None, &[]);
        assert_eq!(index.count(), 1);
    }

    #[test]
    fn matches_are_non_overlapping_and_navigation_wraps() {
        let mut flattener = MessageFlattener::new("m");
        let mut index = ThreadFindIndex::default();
        index.apply(
            0,
            "m",
            FindKind::User,
            &flattener.replace("m", "aaaaa\n\nsecond"),
            None,
            &[],
        );
        index.search("aa", None, &[]);
        assert_eq!(index.count(), 2);
        assert_eq!(index.match_at(1).expect("second").rendered, 2..4);
        assert_eq!(step_thread_find_index(0, 2, -1), 1);
        assert_eq!(step_thread_find_index(1, 2, 1), 0);
        assert_eq!(step_thread_find_index(9, 0, -1), 0);
        assert_eq!(format_thread_find_count(9, 2), "2/2");
        index.remove(0, "m");
        assert_eq!(index.count(), 0);
        assert!(index.match_at(0).is_none());
    }

    #[test]
    fn appended_search_work_does_not_read_completed_message_prefixes() {
        fn run(prefix: usize) -> usize {
            let mut flattener = MessageFlattener::new("m");
            let mut index = ThreadFindIndex::default();
            let delta = flattener.replace(
                "m",
                &format!("{}tail", "settled\n\n```rs\ncode\n```\n\n".repeat(prefix)),
            );
            index.apply(0, "m", FindKind::Assistant, &delta, None, &[]);
            index.search("needle", None, &[]);
            let before = index.normalized_bytes();
            for ch in " needle needle".chars() {
                index.apply(
                    0,
                    "m",
                    FindKind::Assistant,
                    &flattener.append(&ch.to_string()),
                    None,
                    &[],
                );
            }
            assert_eq!(index.count(), 2);
            index.normalized_bytes() - before
        }
        assert_eq!(run(4), run(512));
    }

    #[test]
    fn replacement_clears_old_matches_and_preserves_document_order() {
        let mut first = MessageFlattener::new("a");
        let mut last = MessageFlattener::new("b");
        let mut index = ThreadFindIndex::default();
        index.apply(
            1,
            "b",
            FindKind::Assistant,
            &last.replace("b", "needle"),
            None,
            &[],
        );
        index.apply(
            0,
            "a",
            FindKind::User,
            &first.replace("a", "needle needle"),
            None,
            &[],
        );
        index.search("needle", None, &[]);
        assert_eq!(index.count(), 3);
        assert_eq!(index.match_at(0).expect("first").block.message_id, "a");
        assert_eq!(index.match_at(2).expect("last").block.message_id, "b");
        index.apply(
            0,
            "a",
            FindKind::User,
            &first.replace("a", "edited"),
            None,
            &[],
        );
        assert_eq!(index.count(), 1);
        assert_eq!(index.match_at(0).expect("remaining").block.message_id, "b");
    }

    #[test]
    fn late_definitions_mark_counts_incomplete_then_reconcile_once_when_finished() {
        let mut flattener = MessageFlattener::new("m");
        let mut index = ThreadFindIndex::default();
        index.apply(
            0,
            "m",
            FindKind::Assistant,
            &flattener.replace("m", "[needle][ref]\n\nBefore\n\n```rs\nx\n```\n\nTail"),
            None,
            &[],
        );
        index.search("ref", None, &[]);
        assert_eq!(index.count(), 1);
        let before = index.normalized_bytes();
        for ch in "\n\n[ref]: /target".chars() {
            index.apply(
                0,
                "m",
                FindKind::Assistant,
                &flattener.append(&ch.to_string()),
                None,
                &[],
            );
        }
        assert!(index.counting());
        assert!(index.normalized_bytes() > before);
        index.finish(0, "m");
        assert!(!index.counting());
        assert_eq!(
            index.count(),
            0,
            "resolved reference syntax is not visible text"
        );
        let after = index.normalized_bytes();
        index.finish(0, "m");
        assert_eq!(index.normalized_bytes(), after, "completion is idempotent");
        index.search("needle", None, &[]);
        assert_eq!(index.count(), 1);
    }

    #[test]
    fn a_first_live_hit_can_be_selected_from_only_the_changed_tail() {
        let mut flattener = MessageFlattener::new("live");
        let mut index = ThreadFindIndex::default();
        index.apply(
            0,
            "live",
            FindKind::Assistant,
            &flattener.replace("live", "Before\n\n```rs\nx\n```\n\nTail"),
            None,
            &[],
        );
        index.search("needle", None, &[]);
        assert_eq!(index.count(), 0);
        let delta = flattener.append(" needle");
        index.apply(0, "live", FindKind::Assistant, &delta, None, &[]);
        let first = index
            .first_in_tail(0, "live", delta.changed_from)
            .expect("new live match");
        assert_eq!(first.occurrence, 0);
        assert_eq!(index.match_at(0), Some(first));
    }
}
