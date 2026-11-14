//! Port of apps/web/src/components/ComposerPromptEditorTiptap.tsx ComposerPromptEditorTiptap.

use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use futures::StreamExt;
use nsh_base::rich_editor::{
    commands::helpers::is_active,
    model::{Fragment, Node, ResolvedPos, Schema, SchemaError, SchemaSpec, Slice},
    state::{
        EditorState, EditorStateConfig, KeyEvent, Plugin, PluginSpec, Selection, StateError,
        TextSelection, Transaction, TransactionOrigin,
    },
    transform::{Mapping, ReplaceStep},
    view::{EditorSession, NodeViews, RichEditorClasses, RichEditorPastedFiles, rich_editor},
};
use nsh_core::prelude::*;
use serde_json::{Value, json};
use t3_client::thread_views::{
    composer::{
        block_shortcuts::ListNormalizer,
        code_pairs::CodePairs,
        editor::{self as document, BuildOptions, RichDocMap},
        input_rules,
        markdown_source::{self, EditorSelection, SourceEditor},
        policy::pasted_path_query_length,
    },
    document::{self as source, Selection as SourceSelection, utf16_len},
    undo,
};

use crate::state::layout::{Pointer, layout_class, pointer};
use crate::theme::{self, tw};

pub type SkillResolver = Arc<dyn Fn(&str) -> Option<(String, Option<String>)> + Send + Sync>;
pub type ModelResolver = Arc<dyn Fn(&str, &str) -> bool + Send + Sync>;
#[derive(Clone)]
pub struct ComposerSubmitCallback(Rc<dyn Fn(ComposerSnapshot) -> bool>);

impl ComposerSubmitCallback {
    pub fn new(callback: impl Fn(ComposerSnapshot) -> bool + 'static) -> Self {
        Self(Rc::new(callback))
    }
    pub fn call(&self, snapshot: ComposerSnapshot) -> bool {
        (self.0)(snapshot)
    }
}
impl PartialEq for ComposerSubmitCallback {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

#[derive(Clone)]
pub struct ComposerCommandKeyCallback(Rc<CommandKeyFn>);
type CommandKeyFn = dyn Fn(&KeyEvent, bool) -> bool;

impl ComposerCommandKeyCallback {
    pub fn new(callback: impl Fn(&KeyEvent, bool) -> bool + 'static) -> Self {
        Self(Rc::new(callback))
    }
    pub fn call(&self, event: &KeyEvent, in_list: bool) -> bool {
        (self.0)(event, in_list)
    }
}
impl PartialEq for ComposerCommandKeyCallback {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}
type SubmitHandler = Arc<dyn Fn(ComposerSnapshot) -> bool + Send + Sync>;
type CommandKeyHandler = Arc<dyn Fn(&KeyEvent, bool) -> bool + Send + Sync>;

/// Provider metadata is resolved by the owner; unknown skills and models remain editable text.
#[derive(Clone)]
pub struct ComposerResolvers {
    pub skill: SkillResolver,
    pub model: ModelResolver,
}

impl Default for ComposerResolvers {
    fn default() -> Self {
        Self {
            skill: Arc::new(|_| None),
            model: Arc::new(|_, _| false),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EditorMode {
    Rich,
    #[default]
    Plain,
    Literal,
}

impl EditorMode {
    fn options(self) -> BuildOptions {
        BuildOptions {
            styling: self == Self::Rich,
            literal: self == Self::Literal,
            ..Default::default()
        }
    }
}

/// `createComposerExtensions`: only the serializable T3 nodes and marks, in schema order.
pub fn composer_schema(mode: EditorMode) -> Result<Schema, SchemaError> {
    let mut nodes = vec![
        json!(["doc", {"content":"block+"}]),
        json!(["paragraph", {"content":"inline*","group":"block","parseDOM":[{"tag":"p"}],"toDOM":["p",0]}]),
        json!(["text", {"group":"inline"}]),
        json!(["hardBreak", {"inline":true,"group":"inline","selectable":false,"linebreakReplacement":true,"leafText":"\n","toDOM":["br"]}]),
    ];
    if mode != EditorMode::Literal {
        for (name, attrs, render_text) in [
            (
                "composer-mention",
                json!({"path":{"default":""},"source":{"default":""}}),
                "{source}",
            ),
            (
                "composer-skill",
                json!({"skillName":{"default":""},"skillLabel":{"default":""},"skillDescription":{"default":null}}),
                "${skillName}",
            ),
            (
                "composer-model",
                json!({"instanceId":{"default":""},"model":{"default":""},"source":{"default":""}}),
                "{source}",
            ),
            (
                "composer-citation",
                json!({"citation":{"default":null},"source":{"default":""},"citeKey":{"default":""}}),
                "{source}",
            ),
            (
                "composer-context-reference",
                json!({"kind":{"default":""},"contextId":{"default":""},"label":{"default":""},"source":{"default":""}}),
                "{source}",
            ),
        ] {
            // CMP-CHIPS supplies NodeViews; source remains visible until those are installed.
            nodes.push(json!([name, {"group":"inline","inline":true,"atom":true,"selectable":true,"attrs":attrs,"renderText":render_text}]));
        }
    }
    let mut marks = Vec::new();
    if mode == EditorMode::Rich {
        nodes.extend([
            json!(["bulletList", {"content":"listItem+","group":"block list","listMarker":"bullet","toDOM":["ul",0]}]),
            json!(["orderedList", {"content":"listItem+","group":"block list","listMarker":"ordered","attrs":{"start":{"default":1}},"toDOM":["ol",0]}]),
            json!(["listItem", {"content":"paragraph list*","defining":true,"attrs":{"indent":{"default":""},"marker":{"default":"-"},"space":{"default":" "}},"toDOM":["li",0]}]),
            json!(["taskList", {"content":"taskItem+","group":"block list","toDOM":["ul",{"data-type":"taskList"},0]}]),
            json!(["taskItem", {"content":"paragraph list*","defining":true,"render":"task","attrs":{"checked":{"default":false},"indent":{"default":""},"markerSpace":{"default":" "},"contentSpace":{"default":null}},"toDOM":["li",0]}]),
            json!(["blockquote", {"content":"paragraph+","group":"block","defining":true,"attrs":{"prefix":{"default":"> "}},"toDOM":["blockquote",0]}]),
            json!(["heading", {"content":"inline*","group":"block","defining":true,"attrs":{"level":{"default":1},"space":{"default":" "}},"toDOM":["h1",0]}]),
            json!(["horizontalRule", {"group":"block","attrs":{"source":{"default":"---"}},"toDOM":["hr"]}]),
            json!(["codeBlock", {"content":"text*","group":"block","marks":"","code":true,"defining":true,"attrs":{"language":{"default":""},"fence":{"default":"```"},"close":{"default":"\n```"}},"toDOM":["pre",["code",0]]}]),
        ]);
        marks.extend([
            json!(["strike", {"toDOM":["s",0]}]),
            json!(["bold", {"toDOM":["strong",0]}]),
            json!(["italic", {"toDOM":["em",0]}]),
            json!(["code", {"excludes":"code","exitable":false,"code":true,"toDOM":["code",{"dir":"ltr"},0]}]),
            json!(["markdownSource", {"inclusive":false,"excludes":"","toDOM":["span",0]}]),
        ]);
    }
    SchemaSpec::from_json(&json!({"nodes":nodes,"marks":marks}))
}

fn invalid(error: impl std::fmt::Display) -> StateError {
    StateError::Invalid(error.to_string())
}

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn tree(schema: &Schema, value: &Value) -> Result<Node, StateError> {
    let node = schema.node_from_json(value).map_err(invalid)?;
    node.check().map_err(invalid)?;
    Ok(node)
}

#[cfg(test)]
thread_local! {
    static JSON_EXPORTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn export_document(node: &Node) -> Value {
    #[cfg(test)]
    JSON_EXPORTS.with(|count| count.set(count.get() + 1));
    node.to_json()
}

fn selection(state: &EditorState) -> SourceSelection {
    SourceSelection {
        anchor: state.selection().anchor(),
        head: state.selection().head(),
    }
}

/// `composer-rich-text-doc.ts` caretTakesMarksBefore, excluding the editable source marker.
fn caret_takes_marks_before(state: &EditorState) -> bool {
    let at = state.selection().resolved_from();
    if !state.selection().empty() || !at.parent().is_textblock() {
        return false;
    }
    let styled = |marks: Vec<nsh_base::rich_editor::model::Mark>| {
        marks
            .into_iter()
            .filter(|mark| mark.ty().name() != markdown_source::SOURCE_MARK)
            .collect::<Vec<_>>()
    };
    let before = styled(
        at.node_before()
            .map_or_else(Vec::new, |node| node.marks().to_vec()),
    );
    let after = styled(
        at.node_after()
            .map_or_else(Vec::new, |node| node.marks().to_vec()),
    );
    let current = styled(
        state
            .stored_marks()
            .map_or_else(|| at.marks(), |marks| marks.to_vec()),
    );
    before != after && current == before
}

fn replace_document(
    state: &EditorState,
    value: &Value,
    range: SourceSelection,
) -> Result<Transaction, StateError> {
    let doc = tree(state.schema(), value)?;
    replace_native_document(state, &doc, range)
}

fn diff_end(left: &Fragment, right: &Fragment, mut a: usize, mut b: usize) -> (usize, usize) {
    let mut old = left.iter().rev();
    let mut new = right.iter().rev();
    while let (Some(left), Some(right)) = (old.next(), new.next()) {
        if left.eq(right) {
            a -= left.node_size();
            b -= right.node_size();
            continue;
        }
        if !left.same_markup(right) {
            return (a, b);
        }
        if let (Some(left), Some(right)) = (left.text(), right.text()) {
            let shared = left
                .chars()
                .rev()
                .zip(right.chars().rev())
                .take_while(|(a, b)| a == b)
                .map(|(ch, _)| ch.len_utf16())
                .sum::<usize>();
            return (a - shared, b - shared);
        }
        if left.content().size() > 0 || right.content().size() > 0 {
            return diff_end(left.content(), right.content(), a - 1, b - 1);
        }
        return (a, b);
    }
    (a, b)
}

fn replace_native_document(
    state: &EditorState,
    doc: &Node,
    range: SourceSelection,
) -> Result<Transaction, StateError> {
    if range.anchor > doc.content().size() || range.head > doc.content().size() {
        return Err(invalid("Composer selection is outside the document."));
    }
    let mut tr = state.tr();
    if let Some(start) = state.doc().content().find_diff_start(doc.content(), 0) {
        let (old_end, new_end) = diff_end(
            state.doc().content(),
            doc.content(),
            state.doc().content().size(),
            doc.content().size(),
        );
        let overlap = start.saturating_sub(old_end.min(new_end));
        tr.replace(
            start,
            old_end + overlap,
            doc.slice(start, new_end + overlap, false)
                .map_err(invalid)?,
        )
        .map_err(invalid)?;
    }
    tr.set_selection(TextSelection::create(
        tr.doc(),
        range.anchor,
        Some(range.head),
    ));
    Ok(tr)
}

struct DocumentProjection {
    native: Node,
    json: Arc<Value>,
    map: Option<Arc<RichDocMap>>,
}

fn projection<'a>(data: &'a mut EditorData, node: &Node) -> &'a mut DocumentProjection {
    if data
        .projection
        .as_ref()
        .is_some_and(|cached| !cached.native.ptr_eq(node))
    {
        data.projection = None;
    }
    data.projection.get_or_insert_with(|| DocumentProjection {
        native: node.clone(),
        json: Arc::new(export_document(node)),
        map: None,
    })
}

fn remember_document(data: &mut EditorData, node: &Node, json: Value) {
    data.projection = Some(DocumentProjection {
        native: node.clone(),
        json: Arc::new(json),
        map: None,
    });
}

struct EditorData {
    source: SourceEditor,
    lists: ListNormalizer,
    mode: EditorMode,
    resolvers: ComposerResolvers,
    pairs: Arc<Mutex<CodePairs>>,
    path_query_active: bool,
    disabled: bool,
    error: Option<String>,
    projection: Option<DocumentProjection>,
    draft_key: Option<String>,
    controlled_snapshot: Option<ComposerSnapshot>,
    submit: Option<SubmitHandler>,
    command_key: Option<CommandKeyHandler>,
}

/// A key plugin keeps this weak handle rather than owning the session that owns the plugin.
#[derive(Clone)]
pub struct ComposerSubmit(Weak<Mutex<EditorData>>);

impl ComposerSubmit {
    /// Native key dispatch already defers keys while its input method composes.
    pub fn submit(&self, state: &EditorState) -> bool {
        let Some(shared) = self.0.upgrade() else {
            return false;
        };
        let callback = {
            let data = lock(&shared);
            if data.disabled || data.error.is_some() {
                return false;
            }
            data.submit.clone()
        };
        callback.is_some_and(|callback| callback(snapshot_for(&shared, state)))
    }
}

fn map_for(shared: &Mutex<EditorData>, state: &EditorState) -> Arc<RichDocMap> {
    let mut data = lock(shared);
    let cached = projection(&mut data, state.doc());
    cached
        .map
        .get_or_insert_with(|| Arc::new(document::serialize_document(&cached.json)))
        .clone()
}

fn snapshot_for(shared: &Mutex<EditorData>, state: &EditorState) -> ComposerSnapshot {
    let map = map_for(shared, state);
    let expanded_cursor = map
        .flat_to_markdown(map.pm_to_flat(state.selection().from()))
        .min(utf16_len(&map.value));
    let literal = lock(shared).mode == EditorMode::Literal;
    let at = state.selection().resolved_from();
    let adjacent = [at.node_before(), at.node_after()]
        .into_iter()
        .flatten()
        .any(|node| {
            node.is_inline() && node.is_atom() && !node.is_text() && node.ty().name() != "hardBreak"
        });
    ComposerSnapshot {
        cursor: source::collapse_cursor(&map.value, expanded_cursor, literal),
        expanded_cursor,
        context_ids: if literal {
            vec![]
        } else {
            map.context_ids.clone()
        },
        cursor_adjacent_to_mention: literal || at.parent().ty().spec().code || adjacent,
        value: map.value.clone(),
    }
}

/// The shared seam for CMP-EDITOR-2, CMP-CHIPS, CMP-SUGGEST and the composer host.
#[derive(Clone)]
pub struct ComposerEditor {
    session: EditorSession,
    data: Arc<Mutex<EditorData>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComposerSnapshot {
    pub value: String,
    pub cursor: usize,
    pub expanded_cursor: usize,
    pub context_ids: Vec<String>,
    pub cursor_adjacent_to_mention: bool,
}

impl ComposerSnapshot {
    fn matches_change(&self, next: &Self) -> bool {
        self.value == next.value
            && self.cursor == next.cursor
            && self.expanded_cursor == next.expanded_cursor
            && self.context_ids == next.context_ids
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaretEdge {
    Start,
    End,
}

impl ComposerEditor {
    pub fn new(
        value: &str,
        cursor: usize,
        mode: EditorMode,
        resolvers: ComposerResolvers,
    ) -> Result<Self, StateError> {
        let schema = composer_schema(mode).map_err(invalid)?;
        let value = document::build_document(
            value,
            mode.options(),
            |name| (resolvers.skill)(name),
            |instance, model| (resolvers.model)(instance, model),
        );
        let doc = tree(&schema, &value)?;
        let map = document::serialize_document(&value);
        let expanded = source::expand_cursor(&map.value, cursor, mode == EditorMode::Literal);
        let pos = map.flat_to_pm(map.markdown_to_flat(expanded));
        let data = Arc::new(Mutex::new(EditorData {
            source: SourceEditor::new(value, SourceSelection::caret(pos)),
            lists: ListNormalizer::default(),
            mode,
            resolvers,
            pairs: Arc::new(Mutex::new(CodePairs::default())),
            path_query_active: false,
            disabled: false,
            error: None,
            projection: None,
            draft_key: None,
            controlled_snapshot: None,
            submit: None,
            command_key: None,
        }));
        let initial = lock(&data).source.view().clone();
        let doc = if mode == EditorMode::Rich {
            tree(&schema, &initial.document)?
        } else {
            doc
        };
        let plugin: Plugin = Arc::new(ComposerDocumentPlugin(data.clone()));
        let state = EditorState::create(
            EditorStateConfig::new(schema)
                .doc(doc.clone())
                .selection(TextSelection::create(
                    &doc,
                    initial.selection.anchor,
                    Some(initial.selection.head),
                ))
                .plugins(vec![plugin]),
        )?;
        let session = EditorSession::new(state, NodeViews::new());
        let shared = data.clone();
        // SourceEditor owns canonical history; source-only rewrites never enter NSH's history.
        session.set_dispatch_transaction(Some(Arc::new(move |session, tr| {
            if let Err(error) = apply_transaction(session, &shared, tr) {
                lock(&shared).error = Some(error.to_string());
                session.set_editable(false);
                session.update_state(session.state());
            }
        })));
        Ok(Self { session, data })
    }

    pub fn session(&self) -> EditorSession {
        self.session.clone()
    }

    pub fn submitter(&self) -> ComposerSubmit {
        ComposerSubmit(Arc::downgrade(&self.data))
    }

    pub fn submit(&self) -> bool {
        !self.session.composing()
            && self.session.is_editable()
            && self.submitter().submit(&self.session.state())
    }

    /// `useComposerSend`: clear same-draft content and cursor without remounting its history.
    pub fn reset_after_send(&self) -> Result<(), StateError> {
        self.sync("", 0)?;
        let mut data = lock(&self.data);
        data.path_query_active = false;
        *lock(&data.pairs) = CodePairs::default();
        Ok(())
    }

    /// Shared with Editor-2's caret and composition handlers; input rules use this same tracker.
    pub fn code_pairs(&self) -> Arc<Mutex<CodePairs>> {
        lock(&self.data).pairs.clone()
    }

    pub fn mode(&self) -> EditorMode {
        lock(&self.data).mode
    }

    pub fn error(&self) -> Option<String> {
        lock(&self.data).error.clone()
    }

    pub fn set_disabled(&self, disabled: bool) {
        lock(&self.data).disabled = disabled;
        self.session
            .set_editable(!disabled && self.error().is_none());
    }

    pub fn set_path_query_active(&self, active: bool) {
        lock(&self.data).path_query_active = active;
    }

    pub fn read_map(&self) -> Arc<RichDocMap> {
        self.map_for(&self.session.state())
    }

    fn map_for(&self, state: &EditorState) -> Arc<RichDocMap> {
        map_for(&self.data, state)
    }

    fn document_json(&self, state: &EditorState) -> Arc<Value> {
        projection(&mut lock(&self.data), state.doc()).json.clone()
    }

    pub fn read_snapshot(&self) -> ComposerSnapshot {
        let state = self.session.state();
        self.snapshot_for(&state)
    }

    fn snapshot_for(&self, state: &EditorState) -> ComposerSnapshot {
        snapshot_for(&self.data, state)
    }

    fn is_controlled_snapshot(&self, snapshot: &ComposerSnapshot) -> bool {
        lock(&self.data)
            .controlled_snapshot
            .take()
            .is_some_and(|controlled| controlled.matches_change(snapshot))
    }

    pub fn read_selection_range(&self) -> SourceSelection {
        let state = self.session.state();
        let map = self.read_map();
        SourceSelection {
            anchor: map.flat_to_markdown(map.pm_to_flat(state.selection().from())),
            head: map.flat_to_markdown(map.pm_to_flat(state.selection().to())),
        }
    }

    pub fn focus(&self) {
        self.session.focus();
    }

    pub fn focus_at(&self, cursor: usize) -> Result<(), StateError> {
        let snapshot = self.read_snapshot();
        self.select_markdown_offset(source::expand_cursor(
            &snapshot.value,
            cursor,
            self.mode() == EditorMode::Literal,
        ))?;
        self.focus();
        Ok(())
    }

    pub fn focus_at_end(&self) -> Result<(), StateError> {
        self.select_markdown_offset(utf16_len(&self.read_map().value))?;
        self.focus();
        Ok(())
    }

    pub fn is_caret_on_visual_edge(&self, edge: CaretEdge) -> bool {
        let snapshot = self.read_snapshot();
        if snapshot.value.is_empty() {
            return true;
        }
        let state = self.session.state();
        if !state.selection().empty() {
            return false;
        }
        let Some(byte) = source::utf16_byte_offset(&snapshot.value, snapshot.expanded_cursor)
        else {
            return false;
        };
        let (before, after) = snapshot.value.split_at(byte);
        if (if edge == CaretEdge::Start {
            before
        } else {
            after
        })
        .contains('\n')
        {
            return false;
        }
        let block = match edge {
            CaretEdge::Start => 0,
            CaretEdge::End => state
                .doc()
                .content()
                .size()
                .saturating_sub(state.doc().last_child().map_or(0, Node::node_size)),
        };
        let Some(caret) = self.session.selection_rect() else {
            return false;
        };
        let Some(bounds) = self.session.node_rect(block) else {
            return false;
        };
        let threshold = caret.size.height.0 / 2.0;
        match edge {
            CaretEdge::Start => caret.origin.y.0 - bounds.origin.y.0 < threshold,
            CaretEdge::End => {
                bounds.origin.y.0 + bounds.size.height.0 - caret.origin.y.0 - caret.size.height.0
                    < threshold
            }
        }
    }

    pub fn select_markdown_offset(&self, offset: usize) -> Result<(), StateError> {
        // Revealing source moves the caret stops; the web places the same source offset twice.
        for pass in 0..2 {
            let map = self.read_map();
            let pos = map.flat_to_pm(map.markdown_to_flat(offset));
            let state = self.session.state();
            if pass > 0 && state.selection().empty() && state.selection().from() == pos {
                break;
            }
            let mut tr = state.tr();
            tr.set_selection(TextSelection::create(state.doc(), pos, None))
                .scroll_into_view();
            self.dispatch(tr)?;
        }
        Ok(())
    }

    /// Same-draft `setContent(..., { emitUpdate: false })`; only a new draft/mode remounts history.
    pub fn sync(&self, value: &str, cursor: usize) -> Result<bool, StateError> {
        let snapshot = self.read_snapshot();
        let literal = self.mode() == EditorMode::Literal;
        let expanded = source::expand_cursor(value, cursor, literal);
        let cursor = source::collapse_cursor(value, expanded, literal);
        if snapshot.value == value && snapshot.cursor == cursor {
            return Ok(false);
        }
        if snapshot.value == value {
            if self.session.is_focused() {
                self.select_markdown_offset(expanded)?;
                let snapshot = self.read_snapshot();
                lock(&self.data).controlled_snapshot = Some(snapshot);
            }
            return Ok(false);
        }
        let state = self.session.state();
        let data = lock(&self.data);
        let doc = document::build_document(
            value,
            data.mode.options(),
            |name| (data.resolvers.skill)(name),
            |instance, model| (data.resolvers.model)(instance, model),
        );
        drop(data);
        let map = document::serialize_document(&doc);
        let pos = map.flat_to_pm(map.markdown_to_flat(expanded));
        let mut tr = replace_document(&state, &doc, SourceSelection::caret(pos))?;
        // Web setContent replaces the document, classified as Other even when the new doc is empty.
        tr.set_meta(
            "composerInput",
            json!({"deleted":state.doc().content().size(), "inserted":tr.doc().content().size()}),
        )
        .set_meta("composerControlled", json!(true))
        .set_meta("composerMetadata", json!(true));
        self.dispatch(tr)?;
        Ok(true)
    }

    pub fn sync_draft(
        &self,
        draft_key: &str,
        value: &str,
        cursor: usize,
    ) -> Result<bool, StateError> {
        if lock(&self.data).draft_key.as_deref() == Some(draft_key) {
            return self.sync(value, cursor);
        }
        let mode = self.mode();
        self.reset(
            value,
            source::expand_cursor(value, cursor, mode == EditorMode::Literal),
            mode,
        )?;
        lock(&self.data).draft_key = Some(draft_key.into());
        Ok(true)
    }

    pub fn set_mode(&self, mode: EditorMode) -> Result<bool, StateError> {
        if self.mode() == mode {
            return Ok(false);
        }
        let snapshot = self.read_snapshot();
        self.reset(&snapshot.value, snapshot.expanded_cursor, mode)?;
        Ok(true)
    }

    fn reset(&self, value: &str, expanded: usize, mode: EditorMode) -> Result<(), StateError> {
        let schema = composer_schema(mode).map_err(invalid)?;
        let resolvers = lock(&self.data).resolvers.clone();
        let document = document::build_document(
            value,
            mode.options(),
            |name| (resolvers.skill)(name),
            |instance, model| (resolvers.model)(instance, model),
        );
        let map = document::serialize_document(&document);
        let pos = map.flat_to_pm(map.markdown_to_flat(expanded));
        let source = SourceEditor::new(document, SourceSelection::caret(pos));
        let rewritten = source.view();
        let doc = tree(&schema, &rewritten.document)?;
        let state = EditorState::create(
            EditorStateConfig::new(schema)
                .doc(doc.clone())
                .selection(TextSelection::create(
                    &doc,
                    rewritten.selection.anchor,
                    Some(rewritten.selection.head),
                ))
                .plugins(self.session.state().plugins().to_vec()),
        )?;
        let mut data = lock(&self.data);
        data.source = source;
        data.lists = ListNormalizer::default();
        data.mode = mode;
        *lock(&data.pairs) = CodePairs::default();
        data.error = None;
        let disabled = data.disabled;
        drop(data);
        let controlled = self.snapshot_for(&state);
        lock(&self.data).controlled_snapshot = Some(controlled);
        self.session.update_state(state);
        self.session.set_editable(!disabled);
        Ok(())
    }

    pub fn set_resolvers(&self, resolvers: ComposerResolvers) -> Result<(), StateError> {
        let state = self.session.state();
        let map = self.read_map();
        let range = self.read_selection_range();
        let current = self.document_json(&state);
        let mut data = lock(&self.data);
        if data.mode == EditorMode::Literal {
            data.resolvers = resolvers;
            return Ok(());
        }
        let doc = document::reconcile_chips(
            &current,
            |name| (resolvers.skill)(name),
            |instance, model| (resolvers.model)(instance, model),
        );
        let next_map = document::serialize_document(&doc);
        let next_range = SourceSelection {
            anchor: next_map.flat_to_pm(next_map.markdown_to_flat(range.anchor)),
            head: next_map.flat_to_pm(next_map.markdown_to_flat(range.head)),
        };
        let mut tr = replace_document(&state, &doc, next_range)?;
        tr.set_add_to_history(false)
            .set_meta("composerMetadata", json!(true));
        data.resolvers = resolvers;
        drop(data);
        if *map != next_map || tr.doc_changed() {
            self.dispatch(tr)?;
        }
        Ok(())
    }

    pub fn copy_selection(&self) -> Result<String, StateError> {
        let state = self.session.state();
        document::serialize_selection(
            &self.document_json(&state),
            state.selection().from(),
            state.selection().to(),
        )
        .ok_or_else(|| invalid("Cannot serialize this composer selection."))
    }

    pub fn undo(&self) -> Result<bool, StateError> {
        self.history(false)
    }

    /// Installs the key and suggestion plugins before the document adapter, preserving their state.
    pub fn set_plugins(&self, mut plugins: Vec<Plugin>) -> Result<(), StateError> {
        plugins.push(Arc::new(ComposerDocumentPlugin(self.data.clone())));
        let state = self.session.state().reconfigure(plugins)?;
        self.session.update_state(state);
        Ok(())
    }
    pub fn redo(&self) -> Result<bool, StateError> {
        self.history(true)
    }

    fn history(&self, redo: bool) -> Result<bool, StateError> {
        if !self.session.is_editable() {
            return Ok(false);
        }
        let state = self.session.state();
        let Some(tr) = history_transaction(&state, &self.data, redo)? else {
            return Ok(false);
        };
        self.dispatch(tr)?;
        Ok(true)
    }

    /// Keys and suggestions dispatch through here to retain source normalization and undo grouping.
    pub fn dispatch(&self, tr: Transaction) -> Result<(), StateError> {
        self.session.dispatch(tr)?;
        self.error().map_or(Ok(()), |error| Err(invalid(error)))
    }
}

fn history_transaction(
    state: &EditorState,
    shared: &Mutex<EditorData>,
    redo: bool,
) -> Result<Option<Transaction>, StateError> {
    let mut data = lock(shared);
    if data.disabled || data.error.is_some() {
        return Ok(None);
    }
    if !(if redo {
        data.source.redo()
    } else {
        data.source.undo()
    }) {
        return Ok(None);
    }
    let rewrite = data.source.view();
    let mut tr = replace_document(state, &rewrite.document, rewrite.selection)?;
    tr.set_origin(TransactionOrigin::History).scroll_into_view();
    Ok(Some(tr))
}

fn apply_transaction(
    session: &EditorSession,
    shared: &Mutex<EditorData>,
    mut tr: Transaction,
) -> Result<(), StateError> {
    let old = session.state();
    let composing = session.composing() || tr.get_meta("composition").is_some();
    // NSH holds preedit outside the document and inserts its commit after composing ends.
    let composition_commit =
        !session.composing() && matches!(tr.origin(), Some(TransactionOrigin::Input));
    let controlled = tr.get_meta("composerControlled") == Some(&Value::Bool(true));
    if let Some(error) = tr.get_meta("composerError").and_then(Value::as_str) {
        return Err(invalid(error));
    }
    if tr.doc_changed()
        && !session.is_editable()
        && tr.get_meta("composerMetadata") != Some(&Value::Bool(true))
    {
        return Ok(());
    }
    if let Some(prepared) = prepare_text_input(&old, &mut lock(shared), &mut tr, composing)? {
        tr = prepared;
    }
    let history = matches!(tr.origin(), Some(TransactionOrigin::History));
    let clipboard = matches!(
        tr.origin(),
        Some(TransactionOrigin::Paste | TransactionOrigin::Drop)
    ) || tr.get_meta("paste").is_some()
        || tr.get_meta("cut").is_some();
    let now = tr.time;
    let changed = tr.doc_changed() && tr.add_to_history();
    let steps = if let Some(input) = tr.get_meta("composerInput") {
        vec![Some(undo::Replace {
            deleted: input["deleted"].as_u64().unwrap_or(0) as usize,
            inserted: input["inserted"].as_u64().unwrap_or(0) as usize,
        })]
    } else {
        tr.steps
            .iter()
            .map(|step| {
                let step = step.as_any().downcast_ref::<ReplaceStep>()?;
                Some(undo::Replace {
                    deleted: step.to - step.from,
                    inserted: step.slice.size(),
                })
            })
            .collect::<Vec<_>>()
    };
    let (mut state, mut applied) = old.apply_transaction(tr)?;
    if applied.is_empty() {
        return Ok(());
    }
    let mut data = lock(shared);
    if tr_pairs_need_mapping(&applied) {
        lock(&data.pairs).map(|position, bias| {
            let mut mapped = position;
            for tr in &applied {
                let next = tr.mapping.map_result(mapped, bias);
                if next.deleted {
                    return None;
                }
                mapped = next.pos;
            }
            Some(mapped)
        });
    }
    let cached = projection(&mut data, state.doc()).json.clone();
    let mut doc = (*cached).clone();
    if history && data.mode != EditorMode::Literal {
        doc = document::reconcile_chips(
            &doc,
            |name| (data.resolvers.skill)(name),
            |instance, model| (data.resolvers.model)(instance, model),
        );
    }
    let mut range = selection(&state);
    if data.mode == EditorMode::Rich {
        let mut mapping = Mapping::new();
        for tr in &applied {
            mapping.append_mapping(&tr.mapping);
        }
        data.lists.map_pending(|position| {
            let mapped = mapping.map_result(position, 1);
            (!mapped.deleted).then_some(mapped.pos)
        });
        let mut ranges = Vec::new();
        for map in &mapping.maps {
            for (from, to) in &mut ranges {
                *from = map.map(*from, -1);
                *to = map.map(*to, 1);
            }
            map.for_each(|_, _, from, to| ranges.push((from, to)));
        }
        let normalized = data.lists.apply(
            &doc,
            range,
            &ranges,
            composing && !composition_commit,
            history || controlled,
        );
        let converted = normalized.document != doc;
        doc = normalized.document;
        range = normalized.selection;
        if converted {
            let normalized = tree(state.schema(), &doc)?;
            let mut tr = replace_native_document(&state, &normalized, range)?;
            if state.selection().json_id() != "text" {
                tr.set_selection(state.selection().map(tr.doc(), &tr.mapping));
            }
            tr.set_add_to_history(false)
                .set_stored_marks(state.stored_marks().map(<[_]>::to_vec));
            drop(data);
            let (next, transactions) = state.apply_transaction(tr)?;
            data = lock(shared);
            for tr in &transactions {
                lock(&data.pairs).map(|position, bias| {
                    let mapped = tr.mapping.map_result(position, bias);
                    (!mapped.deleted).then_some(mapped.pos)
                });
            }
            state = next;
            applied.extend(transactions);
        }
    }
    let takes_before = caret_takes_marks_before(&state);
    let rewrite = if history {
        // Reconcile stale chips restored by undo before drawing them.
        data.source.apply(
            doc.clone(),
            range,
            undo::Transaction {
                history: true,
                ..Default::default()
            },
            now,
            takes_before,
        );
        data.source.view().clone()
    } else {
        data.source.apply(
            doc.clone(),
            range,
            undo::Transaction {
                changed,
                clipboard_or_drop: clipboard,
                composition: composing,
                history: false,
                steps: &steps,
            },
            now,
            takes_before,
        );
        if composing && !session.composing() {
            data.source.finish_composition(takes_before);
        }
        data.source.view().clone()
    };
    let mode = data.mode;
    data.error = None;
    drop(data);
    let (rewrite, normalized_json) =
        if state.selection().json_id() != "text" && mode == EditorMode::Rich {
            let kind = if state.selection().json_id() == "node" {
                EditorSelection::Node(state.selection().from())
            } else {
                EditorSelection::All
            };
            let selected = markdown_source::normalize_selection(&doc, kind, changed, takes_before);
            let mut tr = replace_document(
                &state,
                &selected.document,
                selected.selection.range(&selected.document),
            )?;
            let json = match selected.selection {
                EditorSelection::Node(anchor) => json!({"type":"node","anchor":anchor}),
                EditorSelection::All => json!({"type":"all"}),
                EditorSelection::Text(range) => {
                    json!({"type":"text","anchor":range.anchor,"head":range.head})
                }
            };
            tr.set_selection(Selection::from_json(tr.doc(), &json)?);
            (Some(tr), selected.document)
        } else {
            let normalized = tree(state.schema(), &rewrite.document)?;
            let tr = if state.doc().eq(&normalized) && selection(&state) == rewrite.selection {
                None
            } else {
                Some(replace_native_document(
                    &state,
                    &normalized,
                    rewrite.selection,
                )?)
            };
            (tr, rewrite.document)
        };
    if let Some(mut tr) = rewrite {
        tr.set_add_to_history(false);
        if let Some(marks) = state.stored_marks() {
            tr.set_stored_marks(Some(marks.to_vec()));
        }
        let (next, transactions) = state.apply_transaction(tr)?;
        let mut data = lock(shared);
        for tr in &transactions {
            data.lists.map_pending(|position| {
                let mapped = tr.mapping.map_result(position, 1);
                (!mapped.deleted).then_some(mapped.pos)
            });
        }
        drop(data);
        state = next;
        applied.extend(transactions);
    }
    let mut data = lock(shared);
    if data
        .projection
        .as_ref()
        .is_none_or(|cached| !cached.native.ptr_eq(state.doc()))
    {
        remember_document(&mut data, state.doc(), normalized_json);
    }
    drop(data);
    if controlled {
        let snapshot = snapshot_for(shared, &state);
        lock(shared).controlled_snapshot = Some(snapshot);
    }
    session.commit(state, &applied);
    Ok(())
}

fn tr_pairs_need_mapping(applied: &[Transaction]) -> bool {
    applied.iter().any(|tr| tr.doc_changed())
        && !applied
            .iter()
            .any(|tr| tr.get_meta("composerPairsMapped").is_some())
}

// NSH calls input hooks before attaching composition metadata; inspect the native transaction instead.
fn prepare_text_input(
    state: &EditorState,
    data: &mut EditorData,
    tr: &mut Transaction,
    composing: bool,
) -> Result<Option<Transaction>, StateError> {
    if data.mode == EditorMode::Literal
        || composing
        || !matches!(tr.origin(), Some(TransactionOrigin::Input))
        || tr.steps.len() != 1
    {
        return Ok(None);
    }
    let Some(step) = tr.steps[0].as_any().downcast_ref::<ReplaceStep>() else {
        return Ok(None);
    };
    if step.slice.content.child_count() != 1 {
        return Ok(None);
    }
    let Some(text) = step
        .slice
        .content
        .first_child()
        .and_then(Node::text)
        .map(str::to_owned)
    else {
        return Ok(None);
    };
    if utf16_len(&text) != 1 {
        return Ok(None);
    }
    let (from, to) = (step.from, step.to);
    if from != to {
        let doc = projection(data, state.doc()).json.clone();
        if let Some(mut surrounded) = surround_selection(state, &doc, from, to, &text)? {
            surrounded
                .set_origin(TransactionOrigin::Input)
                .set_time(tr.time)
                .scroll_into_view();
            return Ok(Some(surrounded));
        }
    }
    if data.mode != EditorMode::Rich
        || !matches!(text.as_str(), "*" | "_" | "~" | "`" | " ")
        || state.selection().resolved_from().parent().ty().spec().code
    {
        return Ok(None);
    }
    let doc = projection(data, state.doc()).json.clone();
    let marks = state
        .stored_marks()
        .map(<[_]>::to_vec)
        .unwrap_or_else(|| state.selection().resolved_from().marks());
    let marks = marks.iter().map(|mark| mark.to_json()).collect::<Vec<_>>();
    let Some(rewrite) = input_rules::type_text(
        &mut lock(&data.pairs),
        &doc,
        SourceSelection {
            anchor: from,
            head: to,
        },
        &text,
        &marks,
        false,
    ) else {
        return Ok(None);
    };
    let native = tree(state.schema(), &rewrite.document)?;
    let marks = rewrite
        .stored_marks
        .iter()
        .map(|mark| state.schema().mark_from_json(mark).map_err(invalid))
        .collect::<Result<Vec<_>, _>>()?;
    if native.eq(tr.doc())
        && rewrite.selection.anchor == tr.selection().anchor()
        && rewrite.selection.head == tr.selection().head()
    {
        tr.set_stored_marks(Some(marks))
            .set_meta("composerPairsMapped", json!(true));
        remember_document(data, tr.doc(), rewrite.document);
        return Ok(None);
    }
    let mut prepared = replace_native_document(state, &native, rewrite.selection)?;
    prepared
        .set_stored_marks(Some(marks))
        .set_origin(TransactionOrigin::Input)
        .set_time(tr.time)
        .scroll_into_view()
        .set_meta("composerPairsMapped", json!(true))
        .set_meta(
            "composerInput",
            json!({"deleted":to-from,"inserted":utf16_len(&text)}),
        );
    remember_document(data, prepared.doc(), rewrite.document);
    Ok(Some(prepared))
}

struct ComposerDocumentPlugin(Arc<Mutex<EditorData>>);

fn surround_selection(
    state: &EditorState,
    doc: &Value,
    from: usize,
    to: usize,
    text: &str,
) -> Result<Option<Transaction>, StateError> {
    if state.selection().json_id() != "text"
        || from == to
        || state.selection().from() != from
        || state.selection().to() != to
    {
        return Ok(None);
    }
    let closer = match text {
        "(" => ")",
        "[" => "]",
        "{" => "}",
        "'" => "'",
        "\"" => "\"",
        "“" => "”",
        "`" => "`",
        "<" => ">",
        "«" => "»",
        "*" => "*",
        "_" => "_",
        _ => return Ok(None),
    };
    let mut special = false;
    state.doc().nodes_between(from, to, &mut |node, _, _, _| {
        special |= node.is_atom() && node.is_inline() && !node.is_text()
            || node.is_text()
                && node
                    .marks()
                    .iter()
                    .any(|mark| mark.ty().name() != markdown_source::SOURCE_MARK);
        !special
    });
    let map = document::serialize_document(doc);
    if special
        || source::selection_touches_mention_boundary(
            &map.value,
            SourceSelection {
                anchor: map.flat_to_markdown(map.pm_to_flat(from)),
                head: map.flat_to_markdown(map.pm_to_flat(to)),
            },
        )
    {
        return Ok(None);
    }
    let mark = match text {
        "`" => Some(t3_client::thread_views::rich_text::Mark::Code),
        "*" | "_" => Some(t3_client::thread_views::rich_text::Mark::Italic),
        _ => None,
    };
    if let Some(mark) = mark
        && state
            .schema()
            .mark_type(if text == "`" { "code" } else { "italic" })
            .is_some()
        && let Some(document) = document::surround_selection(doc, from, to, mark)
    {
        return replace_document(
            state,
            &document,
            SourceSelection {
                anchor: from,
                head: to,
            },
        )
        .map(Some);
    }
    let mut tr = state.tr();
    tr.insert_text(closer, Some(to), Some(to))
        .insert_text(text, Some(from), Some(from));
    tr.set_selection(TextSelection::create(
        tr.doc(),
        from + utf16_len(text),
        Some(to + utf16_len(text)),
    ));
    Ok(Some(tr))
}

fn error_transaction(state: &EditorState, error: impl std::fmt::Display) -> Transaction {
    let mut tr = state.tr();
    tr.set_meta("composerError", json!(error.to_string()));
    tr
}

impl PluginSpec for ComposerDocumentPlugin {
    fn handle_key_down(
        &self,
        state: &EditorState,
        event: &KeyEvent,
        dispatch: &mut dyn FnMut(Transaction),
    ) -> bool {
        let command = lock(&self.0).command_key.clone();
        let in_list = if event.key == "Enter" {
            let attrs = Default::default();
            is_active(state, Some("taskItem"), &attrs) || is_active(state, Some("listItem"), &attrs)
        } else {
            false
        };
        if command.is_some_and(|command| command(event, in_list)) {
            return true;
        }
        if !event.mod_key || event.alt {
            return false;
        }
        let redo = event.key.eq_ignore_ascii_case("y")
            || (event.key.eq_ignore_ascii_case("z") && event.shift);
        if !redo && !event.key.eq_ignore_ascii_case("z") {
            return false;
        }
        match history_transaction(state, &self.0, redo) {
            Ok(Some(tr)) => dispatch(tr),
            Ok(None) => {}
            Err(error) => dispatch(error_transaction(state, error)),
        }
        true
    }

    fn transform_pasted_html(&self, _state: &EditorState, _html: String) -> String {
        // The web composer deliberately imports the plain-text payload, never foreign HTML styles.
        String::new()
    }

    fn clipboard_text_parser(
        &self,
        state: &EditorState,
        text: &str,
        context: &ResolvedPos,
        _plain: bool,
    ) -> Option<Slice> {
        let mut data = lock(&self.0);
        let mut text = text.replace("\r\n", "\n").replace('\r', "\n");
        if context.parent().ty().spec().code {
            return Some(if text.is_empty() {
                Slice::empty()
            } else {
                Slice::new(
                    Fragment::from(vec![state.schema().text(&text, vec![])]),
                    0,
                    0,
                )
            });
        }
        let tokens = if data.mode == EditorMode::Literal {
            vec![]
        } else {
            source::collect_atoms(&format!("{text}\n"))
        };
        let source_marks = context.marks();
        if !text.contains('\n')
            && tokens.is_empty()
            && source_marks
                .iter()
                .any(|mark| mark.ty().name() == markdown_source::SOURCE_MARK)
        {
            return Some(Slice::new(
                Fragment::from(vec![state.schema().text(&text, source_marks)]),
                0,
                0,
            ));
        }
        let chip = |token: &source::Atom| match token.kind {
            source::AtomKind::Mention => true,
            source::AtomKind::Skill => (data.resolvers.skill)(&token.value).is_some(),
            source::AtomKind::Model => token
                .value
                .split_once('/')
                .is_some_and(|(instance, model)| (data.resolvers.model)(instance, model)),
            _ => false,
        };
        let pasted_length = utf16_len(&text);
        if tokens
            .last()
            .is_some_and(|token| chip(token) && token.range.end == pasted_length)
        {
            text.push(' ');
        }
        if tokens
            .first()
            .is_some_and(|token| chip(token) && token.range.start == 0)
        {
            let cached = projection(&mut data, state.doc()).json.clone();
            let map = document::serialize_document(&cached);
            let offset = map.flat_to_markdown(map.pm_to_flat(state.selection().from()));
            if source::utf16_byte_offset(&map.value, offset)
                .and_then(|byte| map.value[..byte].chars().next_back())
                .is_some_and(|ch| !ch.is_whitespace() && ch != '\u{feff}')
            {
                text.insert(0, ' ');
            }
        }
        let nested = (1..=context.depth).any(|depth| {
            matches!(
                context.node(depth).ty().name(),
                "listItem" | "taskItem" | "blockquote"
            )
        });
        let line_before =
            context
                .parent()
                .text_between(0, context.parent_offset, None, Some("\u{fffc}"));
        let mut options = data.mode.options();
        options.blocks = !nested;
        options.literal_length = if options.literal {
            0
        } else {
            pasted_path_query_length(&line_before, &text, data.path_query_active)
        };
        let value = document::build_document(
            &text,
            options,
            |name| (data.resolvers.skill)(name),
            |instance, model| (data.resolvers.model)(instance, model),
        );
        match tree(state.schema(), &value) {
            Ok(doc) => Some(Slice::max_open(doc.content().clone(), true)),
            Err(error) => {
                drop(data);
                lock(&self.0).error = Some(error.to_string());
                Some(Slice::empty())
            }
        }
    }

    fn handle_paste(
        &self,
        state: &EditorState,
        slice: &Slice,
        dispatch: &mut dyn FnMut(Transaction),
    ) -> bool {
        let error = lock(&self.0).error.clone();
        if let Some(error) = error {
            dispatch(error_transaction(state, error));
            return true;
        }
        if state.selection().resolved_from().parent().ty().spec().code {
            let text = slice
                .content
                .text_between(0, slice.content.size(), None, Some("\n"));
            let mut tr = state.tr();
            tr.insert_text(
                &text,
                Some(state.selection().from()),
                Some(state.selection().to()),
            )
            .set_origin(TransactionOrigin::Paste)
            .scroll_into_view();
            dispatch(tr);
            return true;
        }
        let source_text = slice.content.iter().all(|node| {
            node.is_text()
                && node
                    .marks()
                    .iter()
                    .any(|mark| mark.ty().name() == markdown_source::SOURCE_MARK)
        });
        if source_text && slice.open_start == 0 && slice.open_end == 0 {
            let mut tr = state.tr();
            tr.replace_selection(slice.clone())
                .set_origin(TransactionOrigin::Paste)
                .scroll_into_view();
            dispatch(tr);
            return true;
        }
        let cached = projection(&mut lock(&self.0), state.doc()).json.clone();
        let collapsed = markdown_source::collapse(&cached, selection(state), false);
        let result =
            replace_document(state, &collapsed.document, collapsed.selection).map(|mut tr| {
                tr.replace_selection(slice.clone())
                    .set_origin(TransactionOrigin::Paste)
                    .scroll_into_view();
                tr
            });
        match result {
            Ok(tr) => dispatch(tr),
            Err(error) => dispatch(error_transaction(state, error)),
        }
        true
    }

    fn clipboard_text_serializer(&self, state: &EditorState, _slice: &Slice) -> Option<String> {
        document::serialize_selection(
            &projection(&mut lock(&self.0), state.doc()).json,
            state.selection().from(),
            state.selection().to(),
        )
    }
}

#[derive(Clone, Copy, Default)]
pub struct ComposerPromptEditorProps {
    pub disabled: Prop<bool>,
    /// Final display text: caller-translated keys or literal content, never translated here.
    pub placeholder: Prop<SharedString>,
    /// NSH's placeholder part accepts text; rich suffix content needs a native placeholder slot.
    pub placeholder_suffix: Prop<SharedString>,
    pub class_name: Prop<SharedString>,
    pub placeholder_class_name: Prop<SharedString>,
    pub container_class_name: Prop<SharedString>,
    pub aria_label: Prop<SharedString>,
    pub context_count: Prop<usize>,
    pub on_submit: Prop<Option<ComposerSubmitCallback>>,
    pub on_command_key_down: Prop<Option<ComposerCommandKeyCallback>>,
}

pub fn composer_prompt_editor_tiptap(
    editor: ComposerEditor,
    props: ComposerPromptEditorProps,
    on_change: impl Fn(ComposerSnapshot) + 'static,
    on_paste_files: impl Fn(&RichEditorPastedFiles) + 'static,
) -> impl IntoElement {
    composer_prompt_editor_tiptap_inner(editor, props, on_change, on_paste_files)
}

fn composer_prompt_editor_tiptap_inner(
    editor: ComposerEditor,
    props: ComposerPromptEditorProps,
    on_change: impl Fn(ComposerSnapshot) + 'static,
    on_paste_files: impl Fn(&RichEditorPastedFiles) + 'static,
) -> impl IntoElement {
    let snapshot = signal(editor.read_snapshot());
    let handle = editor.clone();
    let session = handle.session();
    {
        let mut data = lock(&handle.data);
        data.submit = Some(Arc::new(move |snapshot| {
            props
                .on_submit
                .get()
                .is_some_and(|callback| callback.call(snapshot))
        }));
        data.command_key = Some(Arc::new(move |event, in_list| {
            props
                .on_command_key_down
                .get()
                .is_some_and(|callback| callback.call(event, in_list))
        }));
    }
    let callbacks = Arc::downgrade(&handle.data);
    cleanup(move || {
        if let Some(shared) = callbacks.upgrade() {
            let mut data = lock(&shared);
            data.submit = None;
            data.command_key = None;
        }
    });
    // Synchronizes the controlled read-only state with the imperative editor session.
    effect(move || handle.set_disabled(props.disabled.get()));
    let (sender, mut changes) = futures::channel::mpsc::unbounded();
    let subscription = session.subscribe(Arc::new(move |_| {
        let _ = sender.unbounded_send(());
    }));
    let subscribed = session.clone();
    cleanup(move || subscribed.unsubscribe(subscription));
    spawn(async move {
        while changes.next().await.is_some() {
            let next = editor.read_snapshot();
            if !snapshot.get_untracked().matches_change(&next) {
                snapshot.set(next.clone());
                if !editor.is_controlled_snapshot(&next) {
                    on_change(next);
                }
            }
        }
    });
    let content = rich_editor()
        .editor(session)
        .editable(move || !props.disabled.get())
        .accessibility_label(move || {
            let label = props.aria_label.get();
            (!label.is_empty()).then_some(label)
        })
        .test_id("composer-editor")
        // `2lh + spacing(2)` follows prompt sizing; native scroll replaces overflow-y-auto.
        .app_class(move || {
            let minimum = format!("min-h-[{}px]", prompt_size() * 3.25 + 8.0);
            tw(&theme::cn([
                Some("relative -m-1 block max-h-52 p-1 bg-transparent leading-relaxed text-foreground focus:outline-none"),
                Some(minimum.as_str()),
                Some(props.class_name.get().as_str()),
            ]))
        })
        .classes(move || composer_editor_classes(props.placeholder_class_name.get().as_str()))
        // Native preedit is outside the document; its placeholder hides itself during IME.
        .placeholder(move || {
            let source = props.placeholder.get();
            (snapshot.with(|value| value.value.is_empty()) && props.context_count.get() == 0 && !source.is_empty())
                .then(|| SharedString::from(format!("{}{}", source, props.placeholder_suffix.get())))
        })
        .show_only_when_editable(false)
        .on_paste_files(on_paste_files);
    // NSH owns scrolling/wrapping; flow-root has no native meaning. Font fallbacks use tokens.
    view()
        .app_class(move || {
            tw(&theme::cn([
                Some("relative"),
                Some(props.container_class_name.get().as_str()),
            ]))
        })
        .text_size(move || px(prompt_size()))
        .font_family(move || {
            let tokens = theme::current_tokens();
            SharedString::from(
                tokens
                    .font("font-composer")
                    .or_else(|| tokens.font(nsh_core::element::FONT_SANS))
                    .map_or(theme::INTER.family.to_owned(), |font| font.to_string()),
            )
        })
        .child(content)
}

fn prompt_size() -> f32 {
    let size = theme::current_tokens()
        .number("font-size-prompt")
        .unwrap_or(14.0);
    if layout_class().is_compact() && pointer() == Pointer::Coarse {
        // index.css --font-size-prompt-touch: max(prompt, 16px) prevents iOS focus zoom.
        size.max(16.0)
    } else {
        size
    }
}

fn composer_editor_classes(placeholder_class_name: &str) -> RichEditorClasses {
    // index.css uses em units relative to the inline code's own 0.92em font.
    let code_size = prompt_size() * 0.92;
    let code_class = format!(
        "font-mono bg-muted-foreground/12 rounded-[{:.4}px] px-[{:.4}px] py-[{:.4}px] text-[{:.4}px]",
        code_size * 0.3,
        code_size * 0.25,
        code_size * 0.05,
        code_size,
    );
    let mut classes = RichEditorClasses::default();
    classes.caret = Some(tw("bg-foreground"));
    classes.selection = Some(tw("bg-primary/20"));
    // NSH places this text part in the empty block; it owns hit testing and preedit visibility.
    classes.placeholder = Some(tw(&theme::cn([
        Some("leading-relaxed text-placeholder"),
        Some(placeholder_class_name),
    ])));
    classes.selected_node = Some(tw("rounded-md ring-2 ring-inset ring-ring"));
    // `index.css`'s `.composer-tiptap` node and mark selectors, applied to their parts.
    classes.nodes = Some(BTreeMap::from([
        ("paragraph".into(), tw("m-0").to_string()),
        ("bulletList".into(), tw("my-[10.4px] pl-5").to_string()),
        ("orderedList".into(), tw("my-[10.4px] pl-5").to_string()),
        ("taskList".into(), tw("m-0 p-0").to_string()),
        ("taskItem".into(), tw("flex items-start gap-2 m-0 p-0").to_string()),
        ("blockquote".into(), tw("my-[10.4px] border-l-2 border-foreground/35 pl-[12.8px]").to_string()),
        ("horizontalRule".into(), tw("my-[10.4px] border-t border-border dark:border-contrast-muted-foreground").to_string()),
        ("codeBlock".into(), tw("rounded-lg border border-border bg-code-background p-3 font-mono text-code-foreground").to_string()),
        ("heading".into(), tw("mt-5 mb-2 font-semibold leading-[1.3] text-contrast-foreground").to_string()),
        ("heading[level=1]".into(), tw("text-xl").to_string()),
        ("heading[level=2]".into(), tw("text-lg").to_string()),
        ("heading[level=3]".into(), tw("text-base").to_string()),
        ("heading[level=4]".into(), tw("text-sm").to_string()),
        ("heading[level=5]".into(), tw("text-sm").to_string()),
        ("heading[level=6]".into(), tw("text-sm text-contrast-muted-foreground").to_string()),
    ]));
    classes.marks = Some(BTreeMap::from([
        ("bold".into(), tw("font-bold").to_string()),
        ("italic".into(), tw("italic").to_string()),
        ("strike".into(), tw("line-through").to_string()),
        ("code".into(), tw(&code_class).to_string()),
    ]));
    classes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolvers() -> ComposerResolvers {
        ComposerResolvers {
            skill: Arc::new(|name| Some((name.into(), None))),
            model: Arc::new(|_, _| true),
        }
    }

    fn editor(value: &str) -> ComposerEditor {
        ComposerEditor::new(value, 0, EditorMode::Rich, resolvers()).expect("editor")
    }

    fn parse(value: &str, mode: EditorMode) -> Node {
        let schema = composer_schema(mode).expect("T3 schema");
        let value = document::build_document(
            value,
            mode.options(),
            |name| Some((name.into(), None)),
            |_, _| true,
        );
        tree(&schema, &value).expect("client document is valid against the native schema")
    }

    fn type_text(editor: &ComposerEditor, text: &str) {
        for ch in text.chars() {
            let state = editor.session.state();
            let handled = state.plugins().iter().any(|plugin| {
                plugin.handle_text_input(
                    &state,
                    state.selection().from(),
                    state.selection().to(),
                    &ch.to_string(),
                    &mut |tr| editor.dispatch(tr).expect("type"),
                )
            });
            if !handled {
                let mut tr = state.tr();
                tr.insert_text(&ch.to_string(), None, None)
                    .set_origin(TransactionOrigin::Input);
                editor.dispatch(tr).expect("insert text");
            }
        }
    }

    fn paste(editor: &ComposerEditor, text: &str) {
        let state = editor.session.state();
        let slice = state
            .plugins()
            .iter()
            .find_map(|plugin| {
                plugin.clipboard_text_parser(&state, text, state.selection().resolved_from(), false)
            })
            .expect("composer paste parser");
        assert!(state.plugins().iter().any(|plugin| plugin.handle_paste(
            &state,
            &slice,
            &mut |tr| editor.dispatch(tr).expect("paste")
        )));
    }

    #[test]
    fn ts_document_round_trips_validate_every_node_against_the_nsh_schema() {
        // composer-rich-text-doc.test.ts: round trips, lists, quotes, headings, rules and fences.
        for source in [
            "",
            "\n\n",
            "plain text",
            "hello **bold** world",
            "a *italic* word and `code` here",
            "struck ~~out~~ now",
            "**`x`**",
            "*`x`*",
            "~~`x`~~",
            "**a `code` c**",
            "***bold italic*** keeps nesting",
            "line one\nline two",
            "trailing newline\n",
            "1. foo\n2. asdf\n",
            "- [ ] buy milk",
            "-   [ ]  buy milk",
            "\t-\t[x]\t\titem",
            "- [ ]  ",
            "- [ ]\n  - [ ] child",
            "  - [ ] first\n - [ ] second\n  - [ ] child",
            "**before @README.md after**",
            "*a **b** c*",
            "*a**b***",
            "**a*b***",
            "**a *b* c**",
            "literal \u{fffc} **before @README.md after**",
            "- [x] done\n- [ ] next",
            "- [ ] parent\n  - [ ] child\n  - [ ] sibling\n- [ ] uncle",
            "- [ ] empty task follows\n- [ ]",
            "- [ ] **bold** task with @README.md",
            "para\n- [ ] task\npara",
            "- [ ]No space stays literal",
            "-[ ] also literal",
            "@README.md explain this",
            "@\"docs/My File.md\" and $my-skill please",
            "snake_case stays literal",
            "unmatched ** stays literal",
            "**bold** then @README.md then *italic*",
            "- one\n- two",
            "* star\n* star two",
            "+ plus",
            "* star\n+ plus",
            "1. first\n2. second",
            "1) paren\n2) paren",
            "3. starts at three\n4. four",
            "01. zero padded\n02. two",
            "- parent\n  - child\n  - sibling\n- uncle",
            "1. ordered\n   - bullet child\n2. next",
            "- outer\n  1. inner ordered\n  2. more\n- outer again",
            "-   wide space item",
            "-",
            "1.",
            "- \n- second",
            "- **bold** item with @README.md",
            "- [ ] task\n- bullet after",
            "- bullet\n- [x] task after",
            "para\n- item\npara",
            "\t- tab indented\n\t- again",
            "  - leading indent only",
            "- item\n\n- after a blank",
            "- item one\n```ts\ncode\n```\n- item two",
            "-no space stays literal",
            "1.no space stays literal",
            "10. ten\n11. eleven",
            "> quoted",
            "> line one\n> line two",
            ">no space",
            ">  two spaces",
            "> a\n>b",
            ">",
            "> **bold** and @README.md inside",
            "> - looks like a list but stays quote text",
            "> > nested stays literal inside the quote",
            "before\n> quoted",
            "> quote\n\n> another",
            "- item\n> quote after list",
            "> quoted\n\ncomment",
            "> line one\n> line two\n\ncomment",
            "---",
            "***",
            "___",
            "- - -",
            "* * *",
            "-----",
            "---   ",
            "text\n---\nmore",
            "- item\n---\n- item two",
            "```\n---\n```",
            "--",
            "-- -",
            "---text",
            "# Heading",
            "## Two",
            "###### Six",
            "####### seven hashes stays a paragraph",
            "#  two spaces",
            "#\tTab",
            "# Trailing hashes stay literal #",
            "#1234",
            "#1234 is a pull request, not a heading",
            "# Heading with **bold** and @README.md",
            "#",
            "# ",
            "text\n# Heading\ntext",
            "# Heading\n- item\n> quote\n---",
            "```\ncode\n```",
            "```ts\nconst a = 1;\n```",
            "```ts\nconst a = 1;\n```\n",
            "before\n```ts\nconst a = 1;\n```\nafter",
            "```\n```",
            "```ts\nline one\nline two\nline three\n```",
            "```ts\n  indented\n    deeper\n```",
            "```js title=example\ncode\n```",
            "~~~py\ncode\n~~~",
            "````\n```\n````",
            "```ts\ncode without a closing fence",
            "```",
            "```ts\n**not bold** and @README.md stay literal\n```",
            "```ts\ncode\n````",
            "- [ ] task\n```ts\ncode\n```\n- [ ] after",
            "  ```ts\nindented fence stays a paragraph\n  ```",
            "# Title\r\n> quote\r\n---\r\n```ts\r\nconst a = 1;\r\n```\r\nafter",
            "before $x + @README.md$ after",
            "before \\(x + @README.md\\) after",
            "\\[x + @README.md\\] after",
            "Area $x_i^2$ after",
            "before \\(\\frac{1}{2}\\) after",
            "before $$x^2$$ after",
            "\\[\nx = 1\ny = 2\n\\]",
            "- **use** $x_i$ then @README.md",
        ] {
            let doc = parse(source, EditorMode::Rich);
            assert_eq!(
                document::serialize_document(&doc.to_json()).value,
                source,
                "{source:?}"
            );
        }
    }

    #[test]
    fn plain_and_literal_modes_keep_markers_and_literal_answers_keep_crlf_and_context_text() {
        for mode in [EditorMode::Plain, EditorMode::Literal] {
            for value in [
                "",
                "\n\n",
                "hello **bold** stays literal",
                "a *italic* stays literal",
                "some `code` stays literal",
                "- [ ] stays a paragraph",
                "line one\nline two",
                "@README.md explain this",
            ] {
                let doc = parse(value, mode);
                assert_eq!(document::serialize_document(&doc.to_json()).value, value);
                assert!(
                    doc.content()
                        .iter()
                        .all(|node| node.ty().name() == "paragraph")
                );
            }
        }
        for value in [
            "  - [X] done\n\n**keep** €my-skill @README.md  \n",
            "line one\r\nline two\r\n",
            "[file](t3-context://v1/file/ctx_1)",
        ] {
            let doc = parse(value, EditorMode::Literal);
            let map = document::serialize_document(&doc.to_json());
            assert_eq!(map.value, value);
            let editor = ComposerEditor::new(value, 0, EditorMode::Literal, resolvers())
                .expect("literal editor");
            assert!(editor.read_snapshot().context_ids.is_empty());
            assert_eq!(
                document::serialize_selection(&doc.to_json(), 1, doc.content().size() - 1)
                    .as_deref(),
                Some(value)
            );
        }
    }

    #[test]
    fn flat_markdown_and_native_positions_round_trip_utf16_and_atoms() {
        for value in [
            "",
            "\n",
            "text\n\n",
            "😀 **café** @README.md after",
            "- [ ] parent\n  - [ ] child\n- [ ]",
            "**before @README.md after**",
            "```ts\na😀b\n```",
            "before\n```\n```",
        ] {
            let doc = parse(value, EditorMode::Rich);
            let map = document::serialize_document(&doc.to_json());
            for flat in 0..=map.doc_length {
                let pos = map.flat_to_pm(flat);
                assert!(
                    doc.resolve(pos)
                        .expect("native position")
                        .parent()
                        .is_textblock(),
                    "{value:?} flat={flat}"
                );
                assert_eq!(map.pm_to_flat(pos), flat, "{value:?}");
                assert_eq!(
                    map.markdown_to_flat(map.flat_to_markdown(flat)),
                    flat,
                    "{value:?}"
                );
            }
        }
        let doc = parse("- item", EditorMode::Rich);
        let map = document::serialize_document(&doc.to_json());
        for offset in 0..=2 {
            assert_eq!(map.markdown_to_flat(offset), 0);
        }
        assert_eq!(map.flat_to_markdown(0), 2);
    }

    #[test]
    fn pasted_code_is_parsed_once_and_crlf_preserves_blank_lines() {
        for text in ["see `**x**` now", "plain **bold** after"] {
            let editor = editor("");
            paste(&editor, text);
            assert_eq!(editor.read_snapshot().value, text);
            assert!(editor.undo().expect("undo paste"));
            assert_eq!(editor.read_snapshot().value, "");
            assert!(editor.redo().expect("redo paste"));
            assert_eq!(editor.read_snapshot().value, text);
        }
        for text in ["x\r\n\r\ny", "x\r\ry"] {
            let editor = editor("");
            paste(&editor, text);
            assert_eq!(editor.read_snapshot().value, "x\n\ny");
            assert_eq!(editor.session.state().doc().child_count(), 3);
        }
    }

    #[test]
    fn pasted_urls_and_paths_keep_their_underscores() {
        let editor = editor("");
        let source = "Read https://example.com/_handle_/status/1 and open src/__test__.tsx";
        paste(&editor, source);
        assert_eq!(editor.read_snapshot().value, source);
    }

    #[test]
    fn source_reveal_edits_markers_and_undo_does_not_record_selection_rewrites() {
        let editor = editor("");
        paste(&editor, "**bold** after");
        editor.select_markdown_offset(4).expect("reveal bold");
        assert_eq!(
            editor.session.state().doc().text_content(),
            "**bold** after"
        );
        editor.select_markdown_offset(0).expect("opening marker");
        let state = editor.session.state();
        let mut tr = state.tr();
        tr.delete(state.selection().from(), state.selection().from() + 2)
            .expect("delete markers");
        editor.dispatch(tr).expect("marker edit");
        editor.focus_at_end().expect("leave source");
        assert_eq!(editor.read_snapshot().value, "bold** after");
        assert!(editor.undo().expect("undo marker edit"));
        assert_eq!(editor.read_snapshot().value, "**bold** after");
        assert!(editor.undo().expect("undo paste"));
        assert_eq!(editor.read_snapshot().value, "");
        assert!(editor.redo().expect("redo paste"));
        assert!(editor.redo().expect("redo marker edit"));
        assert_eq!(editor.read_snapshot().value, "bold** after");
    }

    #[test]
    fn typing_groups_by_the_actual_edit_and_read_only_history_cannot_consume_it() {
        let editor = editor("");
        type_text(&editor, "hello");
        assert_eq!(editor.read_snapshot().value, "hello");
        editor.set_disabled(true);
        assert!(!editor.undo().expect("read only undo"));
        let state = editor.session.state();
        let mut tr = state.tr();
        let attempted_edit = "discarded";
        tr.insert_text(attempted_edit, None, None);
        editor.dispatch(tr).expect("read only transaction");
        assert_eq!(editor.read_snapshot().value, "hello");
        editor.set_disabled(false);
        assert!(editor.undo().expect("undo typing"));
        assert_eq!(editor.read_snapshot().value, "");
        assert!(editor.redo().expect("redo typing"));
        assert_eq!(editor.read_snapshot().value, "hello");
    }

    #[test]
    fn unknown_dollar_names_keep_the_exact_caret_through_mode_switches() {
        let editor = ComposerEditor::new(
            "echo $HOME now",
            0,
            EditorMode::Rich,
            ComposerResolvers::default(),
        )
        .expect("unknown skill remains text");
        editor
            .select_markdown_offset(8)
            .expect("caret inside unknown dollar text");
        let initial = editor.read_snapshot();
        assert_eq!(initial.expanded_cursor, 8);
        for mode in [
            EditorMode::Plain,
            EditorMode::Rich,
            EditorMode::Literal,
            EditorMode::Rich,
        ] {
            editor.set_mode(mode).expect("mode switch");
            assert_eq!(editor.read_snapshot().expanded_cursor, 8);
        }
        type_text(&editor, "X");
        assert_eq!(editor.read_snapshot().value, "echo $HOXME now");
        editor
            .sync_draft("draft-b", "Draft B", 7)
            .expect("switch drafts");
        assert!(!editor.undo().expect("old draft cannot be restored"));
        assert_eq!(editor.read_snapshot().value, "Draft B");
    }

    #[test]
    fn schema_rejects_unsupported_nested_blocks_instead_of_silently_dropping_them() {
        let schema = composer_schema(EditorMode::Rich).expect("schema");
        for value in [
            json!({"type":"doc","content":[{"type":"image","attrs":{"src":"secret"}}]}),
            json!({"type":"doc","content":[{"type":"blockquote","content":[{"type":"heading","content":[{"type":"text","text":"keep me"}]}]}]}),
            json!({"type":"doc","content":[{"type":"bulletList","content":[{"type":"listItem","content":[{"type":"paragraph"},{"type":"paragraph"}]}]}]}),
        ] {
            assert!(tree(&schema, &value).is_err(), "{value}");
        }
    }

    #[test]
    fn pasting_a_chip_adds_boundary_spaces_and_pasting_inside_shown_source_keeps_its_marks() {
        let first = editor("hello");
        first.focus_at_end().expect("end");
        paste(&first, "@README.md");
        assert_eq!(first.read_snapshot().value, "hello @README.md ");
        let second = editor("**bold** after");
        second.select_markdown_offset(4).expect("inside source");
        paste(&second, "X");
        second.focus_at_end().expect("leave source");
        assert_eq!(second.read_snapshot().value, "**boXld** after");
        let third = editor("before\n");
        third.focus_at_end().expect("empty second line");
        paste(&third, "---");
        assert_eq!(third.read_snapshot().value, "before\n---");
        assert!(
            third
                .session
                .state()
                .selection()
                .resolved_from()
                .parent()
                .is_textblock()
        );
    }

    #[test]
    fn provider_refresh_keeps_source_caret_and_undo_never_restores_stale_chips() {
        let editor = ComposerEditor::new(
            "$browser after",
            0,
            EditorMode::Rich,
            ComposerResolvers::default(),
        )
        .expect("unknown skill");
        editor.focus_at_end().expect("caret");
        editor.set_disabled(true);
        editor.set_resolvers(resolvers()).expect("skills load");
        assert!(
            editor.session.state().doc().to_json()["content"][0]["content"]
                .as_array()
                .expect("inline content")
                .iter()
                .any(|node| node["type"] == "composer-skill")
        );
        editor.set_disabled(false);
        type_text(&editor, "!");
        editor
            .set_resolvers(ComposerResolvers::default())
            .expect("provider removed skill");
        assert_eq!(editor.read_snapshot().value, "$browser after!");
        assert!(editor.undo().expect("undo typed edit"));
        assert_eq!(editor.read_snapshot().value, "$browser after");
        assert!(
            editor.session.state().doc().to_json()["content"][0]["content"]
                .as_array()
                .expect("inline content")
                .iter()
                .all(|node| node["type"] != "composer-skill")
        );
    }

    #[test]
    fn selected_styled_atoms_remain_node_selections_when_source_is_revealed() {
        let editor = editor("$browser after");
        let state = editor.session.state();
        let mut tr = state.tr();
        tr.add_mark(
            1,
            2,
            state
                .schema()
                .mark_type("bold")
                .expect("bold type")
                .create(None)
                .expect("bold mark"),
        )
        .expect("style chip");
        editor.dispatch(tr).expect("apply bold");
        let state = editor.session.state();
        let doc = state.doc().to_json();
        let pos = document::serialize_document(&doc)
            .runs
            .iter()
            .find(|run| run.node_name.as_deref() == Some("composer-skill"))
            .expect("skill run")
            .pm_pos;
        let mut tr = state.tr();
        tr.set_selection(nsh_base::rich_editor::state::NodeSelection::create(
            state.doc(),
            pos,
        ));
        editor.dispatch(tr).expect("select chip");
        assert_eq!(editor.session.state().selection().json_id(), "node");
        let state = editor.session.state();
        assert_eq!(
            state
                .doc()
                .node_at(state.selection().from())
                .expect("selected atom")
                .ty()
                .name(),
            "composer-skill"
        );
        assert_eq!(editor.read_snapshot().value, "**$browser** after");
    }

    #[test]
    fn typing_after_closed_inline_marks_uses_the_plain_caret_stop() {
        for value in ["`Test`", "**Test**"] {
            let editor = editor("");
            type_text(&editor, value);
            type_text(&editor, "f");
            editor.focus_at_end().expect("leave source");
            assert_eq!(editor.read_snapshot().value, format!("{value}f"));
        }
    }

    #[test]
    fn a_native_split_after_closed_code_or_bold_starts_a_plain_line() {
        for value in ["`Test`", "**Test**"] {
            let editor = editor("");
            type_text(&editor, value);
            assert!(
                editor
                    .session
                    .run(&nsh_base::rich_editor::commands::command(
                        nsh_base::rich_editor::commands::split_block_keep_marks
                    ))
            );
            type_text(&editor, "f");
            editor.focus_at_end().expect("leave source");
            assert_eq!(editor.read_snapshot().value, format!("{value}\nf"));
            let state = editor.session.state();
            let last = state.doc().last_child().expect("second line");
            assert!(last.content().iter().all(|node| node.marks().is_empty()));
        }
    }

    #[test]
    fn typing_delimiters_surrounds_plain_selections_without_replacing_their_text() {
        for mode in [EditorMode::Rich, EditorMode::Plain] {
            for (typed, expected) in [
                ("(", "(word)"),
                ("\"", "\"word\""),
                ("*", "*word*"),
                ("`", "`word`"),
            ] {
                let editor = ComposerEditor::new("word", 0, mode, ComposerResolvers::default())
                    .expect("editor");
                let state = editor.session.state();
                let mut tr = state.tr();
                tr.set_selection(TextSelection::create(state.doc(), 1, Some(5)));
                editor.dispatch(tr).expect("select word");
                type_text(&editor, typed);
                editor.focus_at_end().expect("leave source");
                assert_eq!(editor.read_snapshot().value, expected, "{mode:?} {typed}");
            }
        }
    }

    #[test]
    fn switching_draft_keys_discards_history_even_when_the_text_is_identical() {
        let editor = editor("");
        editor.sync_draft("a", "", 0).expect("draft a");
        type_text(&editor, "hello");
        editor.sync_draft("b", "hello", 5).expect("draft b");
        assert!(!editor.undo().expect("draft a is not in draft b's history"));
        assert_eq!(editor.read_snapshot().value, "hello");
        type_text(&editor, "!");
        assert!(editor.undo().expect("draft b has its own history"));
        assert_eq!(editor.read_snapshot().value, "hello");
    }

    #[test]
    fn clearing_a_sent_same_draft_preserves_undo_but_switching_drafts_resets_it() {
        let editor = editor("");
        editor
            .sync_draft("thread-a", "", 0)
            .expect("draft identity");
        type_text(&editor, "hello");
        editor.set_path_query_active(true);
        editor.set_disabled(true);
        editor
            .reset_after_send()
            .expect("controlled clear while sending");
        let empty = editor.read_snapshot();
        assert_eq!(
            (empty.value.as_str(), empty.cursor, empty.expanded_cursor),
            ("", 0, 0)
        );
        assert!(editor.is_controlled_snapshot(&empty));
        assert_eq!(lock(&editor.data).draft_key.as_deref(), Some("thread-a"));
        assert!(!lock(&editor.data).path_query_active);
        editor.set_disabled(false);
        assert!(editor.undo().expect("undo controlled setContent"));
        assert_eq!(editor.read_snapshot().value, "hello");
        assert!(editor.redo().expect("redo clear"));
        assert_eq!(editor.read_snapshot().value, "");
        type_text(&editor, "next");
        assert!(editor.undo().expect("new typing has its own group"));
        assert_eq!(editor.read_snapshot().value, "");
        editor.sync_draft("thread-b", "", 0).expect("new draft");
        assert!(!editor.undo().expect("new identity has no old history"));
    }

    #[test]
    fn submit_reads_current_source_respects_disabled_state_and_does_not_own_the_editor() {
        let editor = editor("");
        assert!(!editor.submit());
        let submitted = Arc::new(Mutex::new(Vec::new()));
        let recorded = submitted.clone();
        lock(&editor.data).submit = Some(Arc::new(move |snapshot| {
            lock(&recorded).push(snapshot);
            true
        }));
        type_text(&editor, "latest");
        editor.set_disabled(true);
        assert!(!editor.submit());
        assert!(lock(&submitted).is_empty());
        editor.set_disabled(false);
        assert!(editor.submit());
        assert_eq!(lock(&submitted)[0].value, "latest");
        let weak = editor.submitter();
        drop(editor);
        let other = self::editor("");
        assert!(!weak.submit(&other.session.state()));
    }

    #[test]
    fn ordinary_keystrokes_export_the_native_document_once_and_reuse_it_for_snapshots() {
        let mut measurements = Vec::new();
        for length in [8_192, 65_536] {
            let editor = editor(&"a".repeat(length));
            editor.focus_at_end().expect("end");
            JSON_EXPORTS.with(|count| count.set(0));
            for _ in 0..100 {
                type_text(&editor, "a");
                editor.read_snapshot();
                editor.read_snapshot();
            }
            assert_eq!(editor.read_snapshot().value, "a".repeat(length + 100));
            let exports = JSON_EXPORTS.with(std::cell::Cell::get);
            measurements.push((length, exports));
        }
        eprintln!(
            "100 keys + two snapshot reads/key, (initial bytes, native JSON exports): {measurements:?}"
        );
        assert!(
            measurements.iter().all(|(_, exports)| *exports <= 100),
            "reuse the same immutable document projection"
        );
    }
}
