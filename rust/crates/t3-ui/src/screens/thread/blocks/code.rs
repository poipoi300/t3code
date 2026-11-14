//! Port of apps/web/src/components/chat/MarkdownCodeBlock.tsx SuspenseShikiCodeBlock and UncachedShikiCodeBlock.

use std::cell::{Cell, RefCell};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;
use std::rc::{Rc, Weak};
use std::sync::OnceLock;

use futures::{StreamExt, channel::mpsc};
use nsh_core::element::StyleRefinement;
use nsh_core::prelude::*;
use syntect::parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet};
use t3_theme::palette::Appearance as ThemeAppearance;
use t3_theme::syntax::{ScopeStyle, SyntaxTheme, syntax_theme_for};

use super::flatten::CodeChunk;
use crate::runtime::Shared;
use crate::theme::{cn, palette, tw};

pub const MAX_HIGHLIGHT_CACHE_ENTRIES: usize = 500;
pub const MAX_HIGHLIGHT_CACHE_MEMORY_BYTES: usize = 50 * 1024 * 1024;
pub const MAX_HIGHLIGHT_CHUNKS_PER_FRAME: usize = 1;

static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();

// Match NSH's loader boundary: only native binaries contain two-face's syntax dump.
#[cfg(not(target_arch = "wasm32"))]
pub async fn load_code_syntaxes() -> bool {
    if SYNTAXES.get().is_some() {
        return true;
    }
    blocking(|| {
        SYNTAXES.get_or_init(two_face::syntax::extra_newlines);
    })
    .await
    .is_ok()
}

// Match nsh-base/src/syntax.rs::load_async; the web page supplies nsh-syntaxes.bin.
#[cfg(target_arch = "wasm32")]
pub async fn load_code_syntaxes() -> bool {
    next_frame().await;
    if SYNTAXES.get().is_some() {
        return true;
    }
    let Some(bytes) = nsh_core::runtime_data::load("syntaxes").await else {
        return false;
    };
    match syntect::dumps::from_uncompressed_data::<SyntaxSet>(&bytes) {
        Ok(set) => {
            let _ = SYNTAXES.set(set);
            SYNTAXES.get().is_some()
        }
        Err(error) => {
            log::warn!("Code syntaxes could not be loaded: {error}");
            false
        }
    }
}

/// ChatMarkdown.tsx / resolveDiffThemeName: custom themes keep the appearance's syntax palette.
pub fn code_syntax_theme() -> &'static SyntaxTheme {
    syntax_theme_for(if dark() {
        ThemeAppearance::Dark
    } else {
        ThemeAppearance::Light
    })
}

fn fence_language(info: Option<&str>) -> &str {
    let language = info
        .unwrap_or("text")
        .split_whitespace()
        .next()
        .unwrap_or("text");
    super::super::markdown::code_block::highlighter_language_of(language)
}

/// syntaxHighlighting.ts aliases, plus the bundled grammar names for Highlights language IDs.
fn find_syntax<'a>(set: &'a SyntaxSet, language: &str) -> Option<&'a SyntaxReference> {
    if plain(language) {
        return None;
    }
    let lower = language.to_ascii_lowercase();
    let alias = match lower.as_str() {
        "luau" => "lua",
        "xsl" | "xslt" => "xml",
        "assembly" | "asm" | "s" | "nasm" | "mips" | "mipsasm" | "riscv" => "asm",
        "shell" | "shellscript" | "console" | "zsh" => "sh",
        "golang" => "go",
        "jsx" => "tsx",
        "mjs" | "cjs" => "js",
        "c++" => "cpp",
        "c#" | "csharp" => "cs",
        "f#" | "fsharp" => "fs",
        "jsonc" | "json5" => "json",
        "objective-c" | "objc" => "m",
        "docker" => "Dockerfile",
        "make" => "Makefile",
        "vue" | "svelte" => "html",
        _ => lower.as_str(),
    };
    set.find_syntax_by_token(alias)
        .filter(|syntax| syntax.name != "Plain Text")
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Checkpoint {
    parser: ParseState,
    scopes: ScopeStack,
}

impl Checkpoint {
    fn new(syntax: &SyntaxReference) -> Self {
        Self {
            parser: ParseState::new(syntax),
            scopes: ScopeStack::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HighlightWork {
    pub parsed_bytes: usize,
    pub parsed_lines: usize,
    pub visited_chunks: usize,
}

#[derive(Clone)]
struct ChunkTokenizer {
    committed: Checkpoint,
    completed_bytes: usize,
    stable: Vec<ScopeSpan>,
}

impl ChunkTokenizer {
    fn new(entry: Checkpoint) -> Self {
        Self {
            committed: entry,
            completed_bytes: 0,
            stable: Vec::new(),
        }
    }

    fn update(
        &mut self,
        source: &str,
        set: &SyntaxSet,
        work: &mut HighlightWork,
    ) -> Result<(Vec<ScopeSpan>, Checkpoint), String> {
        let mut current = self.committed.clone();
        let mut spans = self.stable.clone();
        let mut offset = self.completed_bytes;
        for line in source[self.completed_bytes..].split_inclusive('\n') {
            work.parsed_bytes += line.len();
            work.parsed_lines += 1;
            let operations = current
                .parser
                .parse_line(line, set)
                .map_err(|error| error.to_string())?;
            let mut at = 0;
            for (end, operation) in operations {
                push_scope(
                    &mut spans,
                    offset + at..offset + end,
                    source,
                    &current.scopes,
                );
                current
                    .scopes
                    .apply(&operation)
                    .map_err(|error| error.to_string())?;
                at = end;
            }
            push_scope(
                &mut spans,
                offset + at..offset + line.len(),
                source,
                &current.scopes,
            );
            offset += line.len();
            if line.ends_with('\n') {
                self.completed_bytes = offset;
                self.committed = current.clone();
                self.stable = spans.clone();
            }
        }
        Ok((spans, current))
    }
}

fn push_scope(spans: &mut Vec<ScopeSpan>, range: Range<usize>, source: &str, scopes: &ScopeStack) {
    if range.is_empty() {
        return;
    }
    let content = &source[range.clone()];
    let Some(scope) = scopes
        .as_slice()
        .iter()
        .rev()
        .find_map(|scope| canonical_scope(&scope.build_string(), content))
    else {
        return;
    };
    if let Some(last) = spans.last_mut()
        && last.scope == scope
        && last.range.end == range.start
    {
        last.range.end = range.end;
    } else {
        spans.push(ScopeSpan {
            range,
            scope: scope.into(),
        });
    }
}

fn canonical_scope(scope: &str, content: &str) -> Option<&'static str> {
    let is = |prefix: &str| {
        scope
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
    };
    if is("punctuation.definition.string") || is("punctuation.definition.comment") {
        return None;
    }
    Some(if is("comment") {
        "comment"
    } else if is("constant.character.escape") {
        "string.escape"
    } else if is("string.regexp") || is("string.regex") {
        "string.regex"
    } else if is("string") {
        "string"
    } else if is("constant.numeric") {
        "number"
    } else if is("constant.language.boolean") || matches!(content, "true" | "false") {
        "boolean"
    } else if is("constant") || is("support.constant") {
        "constant"
    } else if is("keyword.operator") {
        if content.chars().all(|c| c.is_ascii_alphabetic()) {
            "keyword"
        } else {
            "operator"
        }
    } else if is("storage.type") {
        if matches!(
            content,
            "let"
                | "const"
                | "var"
                | "static"
                | "mut"
                | "fn"
                | "func"
                | "function"
                | "def"
                | "class"
                | "struct"
                | "enum"
                | "union"
                | "trait"
                | "impl"
                | "type"
                | "interface"
                | "module"
                | "mod"
                | "namespace"
                | "typedef"
                | "val"
                | "extern"
                | "async"
        ) {
            "keyword"
        } else {
            "type.builtin"
        }
    } else if is("keyword") || is("storage") {
        "keyword"
    } else if is("entity.name.function") || is("support.function") || is("variable.function") {
        "function"
    } else if is("entity.name.tag") {
        "tag"
    } else if is("entity.name.type")
        || is("entity.name.class")
        || is("support.type")
        || is("support.class")
    {
        "type"
    } else if is("entity.other.attribute-name") {
        "attribute"
    } else if is("variable.parameter") {
        "variable.parameter"
    } else if is("variable.language") {
        "variable.builtin"
    } else if is("variable") {
        "variable"
    } else if is("meta.object-literal.key")
        || is("meta.mapping.key")
        || is("support.type.property-name")
    {
        "property"
    } else if is("punctuation.section") || is("punctuation.definition") {
        "punctuation.bracket"
    } else if is("punctuation") {
        "punctuation"
    } else if is("markup.heading") {
        "markup.heading"
    } else if is("markup.bold") {
        "markup.bold"
    } else if is("markup.italic") {
        "markup.italic"
    } else if is("markup.inserted") {
        "diff.plus"
    } else if is("markup.deleted") {
        "diff.minus"
    } else {
        return None;
    })
}

struct ChunkRecord {
    chunk: CodeChunk,
    streaming: bool,
    entry: Option<Checkpoint>,
    exit: Option<Checkpoint>,
    tokenizer: Option<ChunkTokenizer>,
    context: String,
    failure: Option<String>,
    unknown: bool,
    result: Option<(String, Rc<HighlightedCode>)>,
}

thread_local! {
    static CODE_CACHE: Rc<RefCell<HighlightCache>> = Rc::new(RefCell::new(HighlightCache::default()));
    static NEXT_DOCUMENT: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// One fence's source/checkpoints, fed changed chunks above recycled cells.
/// Each step parses at most one 40-line chunk; completed token data lives in the bounded cache.
pub struct CodeHighlightDocument {
    chunks: BTreeMap<usize, ChunkRecord>,
    pending: BTreeSet<usize>,
    cache: Rc<RefCell<HighlightCache>>,
    id: u64,
    generation: u64,
    work: HighlightWork,
    version: u64,
}

impl Default for CodeHighlightDocument {
    fn default() -> Self {
        Self {
            chunks: BTreeMap::new(),
            pending: BTreeSet::new(),
            cache: CODE_CACHE.with(Clone::clone),
            id: NEXT_DOCUMENT.with(|next| {
                let id = next.get();
                next.set(id + 1);
                id
            }),
            generation: 0,
            work: HighlightWork::default(),
            version: 0,
        }
    }
}

impl CodeHighlightDocument {
    pub fn work(&self) -> HighlightWork {
        self.work
    }

    pub fn clear(&mut self) {
        self.chunks.clear();
        self.pending.clear();
        self.generation += 1;
    }

    pub fn upsert(&mut self, chunk: &CodeChunk, streaming: bool) -> bool {
        let at = chunk.value_bytes.start;
        if let Some(old) = self.chunks.get_mut(&at) {
            if old.chunk == *chunk && old.streaming == streaming {
                return false;
            }
            let completed = old
                .tokenizer
                .as_ref()
                .map_or(0, |parser| parser.completed_bytes);
            let append = old.streaming
                && (streaming || old.chunk.text == chunk.text)
                && old.chunk.language == chunk.language
                && chunk.text.len() >= old.chunk.text.len()
                && chunk.text.get(completed..old.chunk.text.len())
                    == old.chunk.text.get(completed..);
            if !append {
                old.tokenizer = None;
            }
            if old.chunk.language != chunk.language {
                old.entry = None;
                old.exit = None;
                old.result = None;
                old.unknown = false;
            }
            old.chunk = chunk.clone();
            old.streaming = streaming;
            old.failure = None;
        } else {
            self.generation += 1;
            self.chunks.insert(
                at,
                ChunkRecord {
                    chunk: chunk.clone(),
                    streaming,
                    entry: None,
                    exit: None,
                    tokenizer: None,
                    context: format!("syntect:{}:{}", self.id, self.generation),
                    failure: None,
                    unknown: false,
                    result: None,
                },
            );
        }
        if plain(fence_language(chunk.language.as_deref())) {
            self.pending.remove(&at);
            false
        } else {
            self.pending.insert(at);
            true
        }
    }

    /// Parses one queued chunk and publishes its exit checkpoint to the next chunk.
    pub fn step(&mut self, syntax: &SyntaxTheme) -> bool {
        let Some(at) = self.pending.pop_first() else {
            return false;
        };
        self.work.visited_chunks += 1;
        let Some(set) = SYNTAXES.get() else {
            return false;
        };
        let Some(chunk) = self.chunks.get(&at) else {
            return true;
        };
        let language = fence_language(chunk.chunk.language.as_deref()).to_owned();
        let Some(grammar) = find_syntax(set, &language) else {
            self.chunks
                .get_mut(&at)
                .expect("queued chunk exists")
                .unknown = true;
            self.version += 1;
            return true;
        };
        let entry = if at == 0 {
            Some(Checkpoint::new(grammar))
        } else {
            self.chunks.range(..at).next_back().and_then(|(_, prior)| {
                (prior.chunk.value_bytes.end == at)
                    .then(|| prior.exit.clone())
                    .flatten()
            })
        };
        let Some(entry) = entry else {
            if let Some((&prior, record)) = self.chunks.range(..at).next_back()
                && record.chunk.value_bytes.end == at
                && record.exit.is_none()
                && record.failure.is_none()
                && !record.unknown
            {
                self.pending.insert(prior);
                self.pending.insert(at);
            }
            return true;
        };
        let record = self.chunks.get_mut(&at).expect("queued chunk exists");
        record.unknown = false;
        if record.entry.as_ref() != Some(&entry) {
            record.tokenizer = None;
            self.generation += 1;
            record.context = if at == 0 {
                format!("syntect:v1:{language}:start")
            } else {
                format!("syntect:{}:{}", self.id, self.generation)
            };
        }
        record.entry = Some(entry.clone());
        let key = CacheKey {
            language: language.clone(),
            text_hash: text_hash(&record.chunk.text),
            theme: syntax.name.clone(),
            context: record.context.clone(),
        };
        let cached = (!record.streaming || !record.chunk.last)
            .then(|| self.cache.borrow_mut().get_parsed(&key, &record.chunk.text))
            .flatten();
        let (mut value, exit) = if let Some(cached) = cached {
            cached
        } else {
            let tokenizer = record
                .tokenizer
                .get_or_insert_with(|| ChunkTokenizer::new(entry));
            let (spans, exit) = match tokenizer.update(&record.chunk.text, set, &mut self.work) {
                Ok(result) => result,
                Err(error) => {
                    record.failure = Some(error);
                    record.tokenizer = None;
                    self.version += 1;
                    return true;
                }
            };
            let provider: HighlightCode = Rc::new(move |_, _, _| Ok(spans.clone()));
            let value = self.cache.borrow_mut().highlight(
                &record.chunk,
                syntax,
                &record.context,
                record.streaming,
                Some(&provider),
            );
            self.cache
                .borrow_mut()
                .remember_exit(&key, &record.chunk.text, exit.clone());
            (value, exit)
        };
        let changed_exit = record.exit.as_ref() != Some(&exit);
        record.exit = Some(exit.clone());
        record.failure = None;
        let mut rendered = IncrementalHighlightedDocument {
            previous: record
                .result
                .as_ref()
                .filter(|(theme, _)| theme == &syntax.name)
                .map(|(_, value)| value.clone()),
        };
        value = rendered.update(value);
        if !record.streaming || !record.chunk.last {
            self.cache.borrow_mut().set(key.clone(), value.clone());
            self.cache
                .borrow_mut()
                .remember_exit(&key, &record.chunk.text, exit);
        }
        record.result =
            (record.streaming && record.chunk.last).then(|| (syntax.name.clone(), value));
        if !record.streaming || !record.chunk.last {
            record.tokenizer = None;
        }
        if changed_exit && let Some((&next, _)) = self.chunks.range((at + 1)..).next() {
            self.pending.insert(next);
        }
        self.version += 1;
        true
    }

    pub fn highlighted(&mut self, chunk: &CodeChunk, syntax: &SyntaxTheme) -> Rc<HighlightedCode> {
        let language = fence_language(chunk.language.as_deref());
        if plain(language)
            || self
                .chunks
                .get(&chunk.value_bytes.start)
                .is_some_and(|r| r.unknown)
        {
            let mut value = highlight_code_to_lines(&chunk.text, &[], HighlightStatus::Plain);
            if !plain(language) {
                value.foreground = Some(syntax.style.text.clone());
            }
            return Rc::new(value);
        }
        if let Some(record) = self.chunks.get(&chunk.value_bytes.start) {
            if let Some(error) = &record.failure {
                let mut value = highlight_code_to_lines(
                    &chunk.text,
                    &[],
                    HighlightStatus::Failed(error.clone()),
                );
                value.foreground = Some(syntax.style.text.clone());
                return Rc::new(value);
            }
            if let Some((theme, value)) = &record.result
                && theme == &syntax.name
                && value.text.as_ref() == chunk.text
            {
                return value.clone();
            }
            let key = CacheKey {
                language: language.into(),
                text_hash: text_hash(&chunk.text),
                theme: syntax.name.clone(),
                context: record.context.clone(),
            };
            if let Some(value) = self.cache.borrow_mut().get(&key, &chunk.text) {
                return value;
            }
        }
        self.pending.insert(chunk.value_bytes.start);
        let record = self.chunks.get_mut(&chunk.value_bytes.start);
        let stable = record
            .as_ref()
            .and_then(|record| record.tokenizer.as_ref())
            .map_or_else(Vec::new, |parser| {
                parser
                    .stable
                    .iter()
                    .filter_map(|span| {
                        syntax.scope(&span.scope).map(|style| StyledSpan {
                            range: span.range.clone(),
                            style: style.clone(),
                        })
                    })
                    .collect()
            });
        let mut value = highlight_code_to_lines(&chunk.text, &stable, HighlightStatus::Unavailable);
        value.foreground = Some(syntax.style.text.clone());
        let mut rendered = IncrementalHighlightedDocument {
            previous: record
                .as_ref()
                .and_then(|r| r.result.as_ref())
                .filter(|(theme, _)| theme == &syntax.name)
                .map(|(_, value)| value.clone()),
        };
        let value = rendered.update(Rc::new(value));
        if let Some(record) = record
            && record.streaming
            && record.chunk.last
        {
            record.result = Some((syntax.name.clone(), value.clone()));
        }
        value
    }
}

struct HighlightJob {
    document: RefCell<CodeHighlightDocument>,
    revision: Option<Signal<u64>>,
    visible_until: Cell<Option<usize>>,
}

impl HighlightJob {
    fn publish(&self) {
        if let Some(revision) = self.revision
            && let Some(value) = revision.try_get()
        {
            revision.try_set(value + 1);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum HighlightQueueKey {
    Visible(Reverse<u64>, u64),
    Background(u64),
}

#[derive(Default)]
struct HighlightFrame {
    jobs: usize,
    work: HighlightWork,
    parsed_chunks: usize,
    changed: Vec<Rc<HighlightJob>>,
}

#[doc(hidden)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HighlightQueueFrameWork {
    pub dequeued_jobs: usize,
    pub visited_chunks: usize,
    pub parsed_chunks: usize,
    pub parsed_lines: usize,
    pub parsed_bytes: usize,
}

#[doc(hidden)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HighlightQueueWork {
    pub frames: usize,
    pub total: HighlightQueueFrameWork,
    pub last_frame: HighlightQueueFrameWork,
}

/// A frame consumes a fixed number of queue entries, including cancelled ones; no pruning scan.
#[derive(Default)]
struct HighlightQueue {
    pending: BTreeMap<HighlightQueueKey, Weak<HighlightJob>>,
    keys: BTreeMap<u64, HighlightQueueKey>,
    priority: u64,
    work: HighlightQueueWork,
}

impl HighlightQueue {
    fn enqueue(&mut self, job: &Rc<HighlightJob>, visible: Option<usize>) {
        let id = job.document.borrow().id;
        let key = if let Some(at) = visible {
            job.visible_until.set(Some(at));
            self.priority += 1;
            HighlightQueueKey::Visible(Reverse(self.priority), id)
        } else {
            self.keys
                .get(&id)
                .copied()
                .unwrap_or(HighlightQueueKey::Background(id))
        };
        self.insert(id, key, job);
    }

    fn insert(&mut self, id: u64, key: HighlightQueueKey, job: &Rc<HighlightJob>) {
        if let Some(prior) = self.keys.insert(id, key) {
            self.pending.remove(&prior);
        }
        self.pending.insert(key, Rc::downgrade(job));
    }

    fn remove(&mut self, id: u64) {
        if let Some(key) = self.keys.remove(&id) {
            self.pending.remove(&key);
        }
    }

    fn frame(&mut self, syntax: &SyntaxTheme) -> HighlightFrame {
        let mut frame = HighlightFrame::default();
        for _ in 0..MAX_HIGHLIGHT_CHUNKS_PER_FRAME {
            let Some((key, job)) = self.pending.pop_first() else {
                break;
            };
            let id = match key {
                HighlightQueueKey::Visible(_, id) | HighlightQueueKey::Background(id) => id,
            };
            self.keys.remove(&id);
            frame.jobs += 1;
            let Some(job) = job.upgrade() else {
                continue;
            };
            if job
                .revision
                .is_some_and(|revision| revision.try_get().is_none())
            {
                continue;
            }
            let pending = {
                let mut document = job.document.borrow_mut();
                let before = document.work;
                let version = document.version;
                document.step(syntax);
                frame.parsed_chunks +=
                    usize::from(document.work.parsed_lines != before.parsed_lines);
                frame.work.parsed_bytes += document.work.parsed_bytes - before.parsed_bytes;
                frame.work.parsed_lines += document.work.parsed_lines - before.parsed_lines;
                frame.work.visited_chunks += document.work.visited_chunks - before.visited_chunks;
                if version != document.version {
                    frame.changed.push(job.clone());
                }
                if let Some(at) = job.visible_until.get()
                    && document.pending.range(..=at).next().is_none()
                {
                    job.visible_until.set(None);
                }
                !document.pending.is_empty()
            };
            if pending {
                self.insert(
                    id,
                    if job.visible_until.get().is_some() {
                        key
                    } else {
                        HighlightQueueKey::Background(id)
                    },
                    &job,
                );
            }
        }
        self.work.frames += 1;
        self.work.last_frame = HighlightQueueFrameWork {
            dequeued_jobs: frame.jobs,
            visited_chunks: frame.work.visited_chunks,
            parsed_chunks: frame.parsed_chunks,
            parsed_lines: frame.work.parsed_lines,
            parsed_bytes: frame.work.parsed_bytes,
        };
        self.work.total.dequeued_jobs += frame.jobs;
        self.work.total.visited_chunks += frame.work.visited_chunks;
        self.work.total.parsed_chunks += frame.parsed_chunks;
        self.work.total.parsed_lines += frame.work.parsed_lines;
        self.work.total.parsed_bytes += frame.work.parsed_bytes;
        frame
    }
}

thread_local! {
    static HIGHLIGHT_QUEUES: RefCell<HashMap<Owner, Weak<CodeHighlightQueue>>> = RefCell::default();
}

struct CodeHighlightQueue {
    queue: Rc<RefCell<HighlightQueue>>,
    wake: mpsc::UnboundedSender<()>,
    notified: Rc<Cell<bool>>,
    load_failed: Signal<bool>,
}

impl CodeHighlightQueue {
    fn shared() -> Rc<Self> {
        let owner = Owner::current();
        if let Some(queue) =
            HIGHLIGHT_QUEUES.with_borrow(|queues| queues.get(&owner).and_then(Weak::upgrade))
        {
            return queue;
        }
        let queue = Rc::new(RefCell::new(HighlightQueue::default()));
        let processing = queue.clone();
        let load_failed = signal(false);
        let (wake, mut messages) = mpsc::unbounded();
        let notified = Rc::new(Cell::new(false));
        let received = notified.clone();
        // NSH polls every ready task: only this owner-wide worker may spend the frame budget.
        spawn(async move {
            while messages.next().await.is_some() {
                received.set(false);
                if !load_code_syntaxes().await {
                    load_failed.try_set(true);
                    return;
                }
                loop {
                    // Yield before every budget, even if another wake was already queued.
                    next_frame().await;
                    let frame = processing.borrow_mut().frame(code_syntax_theme());
                    debug_assert!(frame.jobs <= MAX_HIGHLIGHT_CHUNKS_PER_FRAME);
                    debug_assert!(frame.work.visited_chunks <= MAX_HIGHLIGHT_CHUNKS_PER_FRAME);
                    for job in frame.changed {
                        job.publish();
                    }
                    if processing.borrow().pending.is_empty() {
                        break;
                    }
                }
            }
        });
        let shared = Rc::new(Self {
            queue,
            wake,
            notified,
            load_failed,
        });
        HIGHLIGHT_QUEUES.with_borrow_mut(|queues| {
            queues.insert(owner, Rc::downgrade(&shared));
        });
        cleanup(move || {
            HIGHLIGHT_QUEUES.with_borrow_mut(|queues| {
                queues.remove(&owner);
            });
        });
        shared
    }

    fn enqueue(&self, job: &Rc<HighlightJob>, visible: Option<usize>) {
        self.queue.borrow_mut().enqueue(job, visible);
        if !self.notified.replace(true) && self.wake.unbounded_send(()).is_err() {
            self.notified.set(false);
        }
    }
}

/// One per fence, all created under the timeline owner, with one shared tokenizer budget.
#[derive(Clone)]
pub struct CodeHighlightState {
    job: Rc<HighlightJob>,
    queue: Rc<CodeHighlightQueue>,
}

impl Default for CodeHighlightState {
    fn default() -> Self {
        Self::new()
    }
}

impl CodeHighlightState {
    pub fn new() -> Self {
        Self {
            job: Rc::new(HighlightJob {
                document: RefCell::new(CodeHighlightDocument::default()),
                revision: Some(signal(0)),
                visible_until: Cell::new(None),
            }),
            queue: CodeHighlightQueue::shared(),
        }
    }

    pub fn upsert(&self, chunk: &CodeChunk, streaming: bool) {
        if self.job.document.borrow_mut().upsert(chunk, streaming) {
            self.queue.enqueue(&self.job, None);
        }
    }

    /// Test/benchmark observation only; no subscription, queue traversal or mutation.
    #[doc(hidden)]
    pub fn queue_work(&self) -> HighlightQueueWork {
        self.queue.queue.borrow().work
    }

    pub fn clear(&self) {
        let mut document = self.job.document.borrow_mut();
        document.clear();
        self.queue.queue.borrow_mut().remove(document.id);
        drop(document);
        self.job.visible_until.set(None);
        self.job.publish();
    }

    pub fn highlighted(&self, chunk: &CodeChunk) -> Shared<HighlightedCode> {
        if self.queue.load_failed.get() {
            let mut value = highlight_code_to_lines(&chunk.text, &[], HighlightStatus::Plain);
            if !plain(fence_language(chunk.language.as_deref())) {
                value.foreground = Some(code_syntax_theme().style.text.clone());
            }
            return Shared(Rc::new(value));
        }
        if let Some(revision) = self.job.revision {
            revision.get();
        }
        let result = self
            .job
            .document
            .borrow_mut()
            .highlighted(chunk, code_syntax_theme());
        if result.status == HighlightStatus::Unavailable {
            self.queue.enqueue(&self.job, Some(chunk.value_bytes.start));
        }
        Shared(result)
    }
}

/// Bind a mounted chunk to the fence's retained tokenizer and the active syntax palette.
pub fn highlighted_chunk(
    chunk: Memo<Option<CodeChunk>>,
    streaming: Memo<bool>,
    state: CodeHighlightState,
) -> Memo<Option<Shared<HighlightedCode>>> {
    computed(move || {
        chunk.with(|chunk| {
            chunk.as_ref().map(|chunk| {
                state.upsert(chunk, streaming.get());
                state.highlighted(chunk)
            })
        })
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScopeSpan {
    /// UTF-8 byte range relative to this chunk, not the whole fence.
    pub range: Range<usize>,
    pub scope: String,
}

/// Resume the fence's parser state, including for an out-of-order mount.
/// `context` identifies that state AND provider revision: equal text alone is insufficient
/// for a chunk that starts inside a multiline string or comment.
pub type HighlightCode = Rc<dyn Fn(&str, &CodeChunk, &str) -> Result<Vec<ScopeSpan>, String>>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HighlightStatus {
    Highlighted,
    Plain,
    Unavailable,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StyledSpan {
    pub range: Range<usize>,
    pub style: ScopeStyle,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HighlightedLine {
    pub text: String,
    pub spans: Vec<StyledSpan>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HighlightedCode {
    /// Exact parsed value; rendering never becomes the source for copy or selection.
    pub text: Rc<str>,
    pub lines: Vec<Rc<HighlightedLine>>,
    pub status: HighlightStatus,
    pub foreground: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CacheKey {
    language: String,
    text_hash: u64,
    theme: String,
    context: String,
}

struct CacheEntry {
    key: CacheKey,
    value: Rc<HighlightedCode>,
    bytes: usize,
    exit: Option<Checkpoint>,
}

/// Port of lib/lruCache.ts, with Rust allocation sizes instead of UTF-16 HTML sizes.
/// Share one per app, never inside a recycled cell.
pub struct HighlightCache {
    entries: VecDeque<CacheEntry>,
    bytes: usize,
    max_entries: usize,
    max_bytes: usize,
}

impl Default for HighlightCache {
    fn default() -> Self {
        Self::new(
            MAX_HIGHLIGHT_CACHE_ENTRIES,
            MAX_HIGHLIGHT_CACHE_MEMORY_BYTES,
        )
    }
}

impl HighlightCache {
    pub fn new(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            bytes: 0,
            max_entries,
            max_bytes,
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn memory_bytes(&self) -> usize {
        self.bytes
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }

    pub fn highlight(
        &mut self,
        chunk: &CodeChunk,
        syntax: &SyntaxTheme,
        context: &str,
        is_streaming: bool,
        provider: Option<&HighlightCode>,
    ) -> Rc<HighlightedCode> {
        let language = fence_language(chunk.language.as_deref());
        if plain(language) {
            return Rc::new(highlight_code_to_lines(
                &chunk.text,
                &[],
                HighlightStatus::Plain,
            ));
        }
        // NSH's tokenizer and CodeHighlighter callback are private at the pinned revision.
        // Do not cache absent-provider output: connecting one must immediately recolor it.
        let Some(provider) = provider else {
            return Rc::new(highlight_code_to_lines(
                &chunk.text,
                &[],
                HighlightStatus::Unavailable,
            ));
        };
        let key = CacheKey {
            language: language.into(),
            text_hash: text_hash(&chunk.text),
            theme: syntax.name.clone(),
            context: context.into(),
        };
        let cacheable = !is_streaming || !chunk.last;
        if cacheable && let Some(value) = self.get(&key, &chunk.text) {
            return value;
        }
        let result = provider(language, chunk, context).and_then(|spans| {
            validate_spans(&chunk.text, &spans)?;
            Ok(spans
                .into_iter()
                .filter_map(|span| {
                    syntax.scope(&span.scope).map(|style| StyledSpan {
                        range: span.range,
                        style: style.clone(),
                    })
                })
                .collect::<Vec<_>>())
        });
        let value = Rc::new(match result {
            Ok(spans) => {
                let mut highlighted =
                    highlight_code_to_lines(&chunk.text, &spans, HighlightStatus::Highlighted);
                highlighted.foreground = Some(syntax.style.text.clone());
                highlighted
            }
            Err(error) => {
                log::warn!("Code highlighting failed for language {language:?}: {error}");
                highlight_code_to_lines(&chunk.text, &[], HighlightStatus::Failed(error))
            }
        });
        // A failing/unknown lexer must not poison a later successful mount.
        if cacheable && value.status == HighlightStatus::Highlighted {
            self.set(key, value.clone());
        }
        value
    }

    fn get(&mut self, key: &CacheKey, text: &str) -> Option<Rc<HighlightedCode>> {
        // A hash collision must never display another chunk.
        let at = self
            .entries
            .iter()
            .position(|entry| entry.key == *key && entry.value.text.as_ref() == text)?;
        let entry = self.entries.remove(at)?;
        let value = entry.value.clone();
        self.entries.push_back(entry);
        Some(value)
    }

    fn get_parsed(
        &mut self,
        key: &CacheKey,
        text: &str,
    ) -> Option<(Rc<HighlightedCode>, Checkpoint)> {
        let value = self.get(key, text)?;
        Some((value, self.entries.back()?.exit.clone()?))
    }

    fn remember_exit(&mut self, key: &CacheKey, text: &str, exit: Checkpoint) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|e| e.key == *key && e.value.text.as_ref() == text)
        {
            entry.exit = Some(exit);
        }
    }

    fn set(&mut self, key: CacheKey, value: Rc<HighlightedCode>) {
        let bytes = estimate_highlighted_size(&key, &value);
        if bytes > self.max_bytes || self.max_entries == 0 {
            return;
        }
        if let Some(at) = self
            .entries
            .iter()
            .position(|entry| entry.key == key && entry.value.text == value.text)
            && let Some(entry) = self.entries.remove(at)
        {
            self.bytes -= entry.bytes;
        }
        while self.entries.len() >= self.max_entries || self.bytes + bytes > self.max_bytes {
            let Some(entry) = self.entries.pop_front() else {
                break;
            };
            self.bytes -= entry.bytes;
        }
        self.entries.push_back(CacheEntry {
            key,
            value,
            bytes,
            exit: None,
        });
        self.bytes += bytes;
    }
}

fn text_hash(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

fn plain(language: &str) -> bool {
    ["", "text", "txt", "plain", "plaintext"]
        .iter()
        .any(|plain| language.eq_ignore_ascii_case(plain))
}

fn validate_spans(text: &str, spans: &[ScopeSpan]) -> Result<(), String> {
    let mut end = 0;
    for span in spans {
        if span.range.start < end || span.range.is_empty() || text.get(span.range.clone()).is_none()
        {
            return Err("Highlighter returned invalid UTF-8 token ranges".into());
        }
        end = span.range.end;
    }
    Ok(())
}

fn estimate_highlighted_size(key: &CacheKey, value: &HighlightedCode) -> usize {
    std::mem::size_of::<CacheEntry>()
        + key.language.capacity()
        + key.theme.capacity()
        + key.context.capacity()
        + value.text.len()
        + std::mem::size_of::<HighlightedCode>()
        + value.lines.capacity() * std::mem::size_of::<Rc<HighlightedLine>>()
        + value
            .lines
            .iter()
            .map(|line| {
                std::mem::size_of::<HighlightedLine>()
                    + line.text.capacity()
                    + line.spans.capacity() * std::mem::size_of::<StyledSpan>()
                    + line
                        .spans
                        .iter()
                        .map(|span| {
                            span.style.color.as_ref().map_or(0, String::capacity)
                                + span.style.font_style.as_ref().map_or(0, String::capacity)
                        })
                        .sum::<usize>()
            })
            .sum::<usize>()
}

/// Port of lib/incrementalHighlighting.ts highlightedDocument, retaining exact line endings.
pub(crate) fn highlight_code_to_lines(
    text: &str,
    spans: &[StyledSpan],
    status: HighlightStatus,
) -> HighlightedCode {
    let mut offset = 0;
    let mut next_span = 0;
    let mut lines: Vec<_> = text
        .split_inclusive('\n')
        .map(|text| {
            let end = offset + text.len();
            while next_span < spans.len() && spans[next_span].range.end <= offset {
                next_span += 1;
            }
            let line_spans = spans[next_span..]
                .iter()
                .take_while(|span| span.range.start < end)
                .map(|span| StyledSpan {
                    range: span.range.start.max(offset) - offset..span.range.end.min(end) - offset,
                    style: span.style.clone(),
                })
                .collect();
            offset = end;
            Rc::new(HighlightedLine {
                text: text.into(),
                spans: line_spans,
            })
        })
        .collect();
    if lines.is_empty() {
        lines.push(Rc::new(HighlightedLine {
            text: String::new(),
            spans: Vec::new(),
        }));
    }
    HighlightedCode {
        text: text.into(),
        lines,
        status,
        foreground: None,
    }
}

/// Port of createIncrementalHighlightedDocument's completed-line identity.
/// Keep one per BlockId outside the virtual cell; edits replace only changed lines.
#[derive(Default)]
pub struct IncrementalHighlightedDocument {
    previous: Option<Rc<HighlightedCode>>,
}

impl IncrementalHighlightedDocument {
    pub fn update(&mut self, highlighted: Rc<HighlightedCode>) -> Rc<HighlightedCode> {
        if let Some(previous) = &self.previous {
            if Rc::ptr_eq(previous, &highlighted) {
                return previous.clone();
            }
            let mut next = highlighted.as_ref().clone();
            for (line, old) in next.lines.iter_mut().zip(&previous.lines) {
                if line == old {
                    *line = old.clone();
                }
            }
            let next = Rc::new(next);
            self.previous = Some(next.clone());
            return next;
        }
        self.previous = Some(highlighted.clone());
        highlighted
    }

    pub fn clear(&mut self) {
        self.previous = None;
    }
}

/// One body row beneath TL-MARKDOWN-1's chrome; the list owns the fence's wrap state.
/// The web's nested horizontal pre scroller is clipped here: only the timeline scrolls.
pub fn code_chunk_row(
    chunk: Memo<Option<CodeChunk>>,
    highlighted: Memo<Option<Shared<HighlightedCode>>>,
    wrapped: Memo<bool>,
) -> impl IntoElement {
    view()
        .app_class(move || {
            Some(tw(&cn([
                Some("min-w-0 max-w-full overflow-hidden px-[0.9rem]"),
                // index.css .chat-markdown pre: padding only at the ends of a split fence.
                chunk
                    .with(|chunk| chunk.as_ref().is_some_and(|c| c.first))
                    .then_some("pt-[0.8rem]"),
                chunk
                    .with(|chunk| chunk.as_ref().is_some_and(|c| c.last))
                    .then_some("pb-[0.8rem]"),
            ])))
        })
        .child(suspense_shiki_code_block(highlighted, wrapped))
}

/// NSH needs no Suspense/HTML swap: cached and streaming chunks use the same keyed line tree.
pub fn suspense_shiki_code_block(
    highlighted: Memo<Option<Shared<HighlightedCode>>>,
    wrapped: Memo<bool>,
) -> impl IntoElement {
    uncached_shiki_code_block(highlighted, wrapped)
}

/// Cached and uncached spans share the web's line renderer, without an HTML replacement.
pub fn uncached_shiki_code_block(
    highlighted: Memo<Option<Shared<HighlightedCode>>>,
    wrapped: Memo<bool>,
) -> impl IntoElement {
    // chat-markdown-shiki is a selector hook, and its pre background is transparent.
    view()
        .app_class(tw("min-w-0 max-w-full bg-transparent"))
        .child(highlighted_code_lines(highlighted, wrapped))
}

/// Port of apps/web/src/components/chat/HighlightedCodeLines.tsx HighlightedCodeLines.
pub fn highlighted_code_lines(
    highlighted: Memo<Option<Shared<HighlightedCode>>>,
    wrapped: Memo<bool>,
) -> impl IntoElement {
    view()
        .style_with(move |style| {
            if crate::theme::has_theme_id() {
                return style.text_color(palette().code_foreground);
            }
            if let Some(color) = highlighted.with(|code| {
                code.as_ref()
                    .and_then(|code| code.foreground.as_deref().and_then(hex_color))
            }) {
                style.text_color(color)
            } else {
                style
            }
        })
        .child(each(
            move || {
                highlighted.with(|code| {
                    code.as_ref().map_or_else(Vec::new, |code| {
                        code.lines
                            .iter()
                            .enumerate()
                            .map(|(index, line)| (index, Shared(line.clone())))
                            .collect::<Vec<_>>()
                    })
                })
            },
            |(index, _)| *index,
            move |line, _| {
                let value = computed(move || line.with(|(_, line)| line.clone()));
                // A dynamic text child is invisible on native NSH; keep the region outside text.
                view().child(dynamic(move || {
                    let line = value.get();
                    untracked(|| highlighted_line(&line, wrapped))
                }))
            },
        ))
}

/// Port of HighlightedCodeLines.tsx HighlightedLine.
fn highlighted_line(line: &HighlightedLine, wrapped: Memo<bool>) -> AnyElement {
    let source = line.text.strip_suffix('\n').unwrap_or(&line.text);
    let source = source.strip_suffix('\r').unwrap_or(source);
    let mut children = Vec::new();
    let mut at = 0;
    for span in &line.spans {
        let end = span.range.end.min(source.len());
        if span.range.start >= end {
            continue;
        }
        if at < span.range.start {
            children.push(text(source[at..span.range.start].to_owned()).into_element());
        }
        let mut token = text(source[span.range.start..end].to_owned());
        *token.style() = syntax_style(&span.style);
        children.push(token.into_element());
        at = end;
    }
    if at < source.len() {
        children.push(text(source[at..].to_owned()).into_element());
    }
    // index.css pre code; copy and selection use the source, never these visual line nodes.
    text(String::new())
        .children(children)
        .select_text()
        // --font-size-code is optional in the frozen theme bridge; use the TS preference default.
        .text_size(move || px(code_font_size()))
        .min_h(move || px(code_font_size() * 1.375))
        // NSH preserves spaces by default; only its normal/nowrap classes control wrapping.
        .app_class(move || {
            Some(tw(if wrapped.get() {
                "font-mono leading-snug whitespace-normal"
            } else {
                "font-mono leading-snug whitespace-nowrap"
            }))
        })
        .into_element()
}

fn code_font_size() -> f32 {
    tokens()
        .number("font-size-code")
        .unwrap_or(t3_theme::fonts::DEFAULT_CODE_FONT_SIZE as f32)
}

fn syntax_style(style: &ScopeStyle) -> StyleRefinement {
    let mut result = StyleRefinement::default();
    if let Some(color) = style.color.as_deref().and_then(hex_color) {
        result = result.text_color(color);
    }
    if style.font_style.as_deref() == Some("italic") {
        result = result.italic();
    }
    if let Some(weight) = style.font_weight {
        result = result.font_weight(FontWeight(weight.clamp(100, 900) as u16));
    }
    result
}

fn hex_color(color: &str) -> Option<AdaptiveColor> {
    let hex = color.strip_prefix('#')?;
    if !matches!(hex.len(), 6 | 8) || !hex.bytes().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let value = u32::from_str_radix(hex, 16).ok()?;
    Some(AdaptiveColor::from(rgba(if hex.len() == 6 {
        value << 8 | 0xff
    } else {
        value
    })))
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use t3_theme::palette::Appearance;
    use t3_theme::syntax::syntax_theme_for;

    use super::super::flatten::{BlockKind, CODE_CHUNK_LINES, MessageFlattener};
    use super::*;

    fn chunk(text: &str) -> CodeChunk {
        CodeChunk {
            language: Some("rust".into()),
            meta: None,
            text: text.into(),
            start_line: 1,
            first: true,
            last: true,
            value_bytes: 0..text.len(),
        }
    }

    fn key(text: &str) -> CacheKey {
        CacheKey {
            language: "rust".into(),
            text_hash: text_hash(text),
            theme: "pierre-dark".into(),
            context: "lexer-v1:start".into(),
        }
    }

    fn value(text: &str) -> Rc<HighlightedCode> {
        Rc::new(highlight_code_to_lines(
            text,
            &[],
            HighlightStatus::Highlighted,
        ))
    }

    fn syntax_set() -> &'static SyntaxSet {
        SYNTAXES.get_or_init(two_face::syntax::extra_newlines)
    }

    fn drain(document: &mut CodeHighlightDocument, syntax: &SyntaxTheme) {
        syntax_set();
        let mut remaining = document.chunks.len() * 3 + 3;
        while document.step(syntax) {
            assert!(remaining > 0, "tokenizer must make progress");
            remaining -= 1;
        }
        assert!(document.pending.is_empty());
    }

    fn full_tokens(language: &str, source: &str) -> Vec<ScopeSpan> {
        let set = syntax_set();
        let syntax = find_syntax(set, language).expect("test grammar");
        ChunkTokenizer::new(Checkpoint::new(syntax))
            .update(source, set, &mut HighlightWork::default())
            .expect("tokens")
            .0
    }

    fn style_at(code: &HighlightedCode, needle: &str) -> Option<ScopeStyle> {
        code.lines.iter().find_map(|line| {
            let at = line.text.find(needle)?;
            line.spans
                .iter()
                .find(|span| span.range.contains(&at))
                .map(|span| span.style.clone())
        })
    }

    #[component]
    fn SharedQueueFixture() -> impl IntoElement {
        let owner = Owner::current();
        let states: Vec<_> = (0..512)
            .map(|_| owner.run(CodeHighlightState::new))
            .collect();
        let first = &states[0];
        for state in &states {
            assert!(Rc::ptr_eq(&first.queue, &state.queue));
            assert_eq!(first.queue_work(), state.queue_work());
        }
        let states = stored(states);
        view().child(text(move || {
            states.with(|states| format!("{} fences share one worker", states.len()))
        }))
    }

    #[test]
    fn fences_created_by_the_timeline_owner_share_one_worker() {
        let mut app = nsh_testing::Headless::start(nsh_engine::EngineConfig {
            ui_sets: crate::registries(),
            ..nsh_engine::EngineConfig::new(nsh_engine::Root::of::<SharedQueueFixture>())
        })
        .expect("shared queue fixture");
        app.wait_for("512 fences share one worker")
            .expect("shared worker");
    }

    #[test]
    fn shared_queue_bounds_all_512_fences_per_frame_and_promotes_visible_work() {
        syntax_set();
        let syntax = syntax_theme_for(ThemeAppearance::Dark);
        let mut queue = HighlightQueue::default();
        let cache = Rc::new(RefCell::new(HighlightCache::default()));
        let jobs: Vec<_> = (0..512)
            .map(|index| {
                let mut document = CodeHighlightDocument::default();
                document.cache = cache.clone();
                document.upsert(
                    &chunk(&format!("let fence_{index} = {index};\n").repeat(CODE_CHUNK_LINES)),
                    false,
                );
                let job = Rc::new(HighlightJob {
                    document: RefCell::new(document),
                    revision: None,
                    visible_until: Cell::new(None),
                });
                queue.enqueue(&job, None);
                job
            })
            .collect();
        let run_frame = |queue: &mut HighlightQueue| {
            let frame = queue.frame(syntax);
            for job in &frame.changed {
                let mut document = job.document.borrow_mut();
                let code = document.chunks[&0].chunk.clone();
                assert_eq!(
                    document.highlighted(&code, syntax).status,
                    HighlightStatus::Highlighted
                );
            }
            frame
        };
        queue.enqueue(&jobs[511], Some(0));
        let first = run_frame(&mut queue);
        assert_eq!(first.work.visited_chunks, 1);
        assert_eq!(
            first.changed[0].document.borrow().id,
            jobs[511].document.borrow().id
        );
        // A newly mounted fence outranks background work already fed by the list.
        queue.enqueue(&jobs[256], Some(0));
        let mut frames = vec![first];
        let mut counters = vec![queue.work];
        frames.push(run_frame(&mut queue));
        counters.push(queue.work);
        assert_eq!(
            frames[1].changed[0].document.borrow().id,
            jobs[256].document.borrow().id
        );
        while !queue.pending.is_empty() {
            frames.push(run_frame(&mut queue));
            counters.push(queue.work);
            assert!(
                frames.len() <= 512,
                "every frame must retire one queued fence"
            );
        }
        let mut highlighted = BTreeSet::new();
        for frame in &frames {
            assert!(frame.jobs <= MAX_HIGHLIGHT_CHUNKS_PER_FRAME);
            assert!(frame.work.visited_chunks <= MAX_HIGHLIGHT_CHUNKS_PER_FRAME);
            assert!(frame.work.parsed_lines <= CODE_CHUNK_LINES * MAX_HIGHLIGHT_CHUNKS_PER_FRAME);
            for job in &frame.changed {
                highlighted.insert(job.document.borrow().id);
            }
        }
        assert_eq!(frames.len(), 512);
        assert_eq!(highlighted.len(), 512);
        for (index, counter) in counters.iter().enumerate() {
            assert_eq!(counter.frames, index + 1);
            assert_eq!(counter.last_frame.parsed_chunks, 1);
            assert_eq!(counter.last_frame.parsed_lines, CODE_CHUNK_LINES);
            assert_eq!(
                counter.last_frame.dequeued_jobs,
                MAX_HIGHLIGHT_CHUNKS_PER_FRAME
            );
            assert_eq!(counter.total.parsed_chunks, index + 1);
            assert_eq!(counter.total.parsed_lines, (index + 1) * CODE_CHUNK_LINES);
        }
        for job in &jobs {
            let document = job.document.borrow();
            assert!(document.pending.is_empty());
            assert_eq!(document.work.parsed_lines, CODE_CHUNK_LINES);
            assert_eq!(document.work.visited_chunks, 1);
            assert!(document.chunks[&0].exit.is_some());
        }
        let background: Vec<_> = frames[2..]
            .iter()
            .map(|frame| frame.changed[0].document.borrow().id)
            .collect();
        let expected: Vec<_> = jobs
            .iter()
            .enumerate()
            .filter(|(index, _)| ![256, 511].contains(index))
            .map(|(_, job)| job.document.borrow().id)
            .collect();
        assert_eq!(background, expected, "offscreen fences retain source order");

        for job in &jobs {
            queue.enqueue(job, None);
        }
        drop(frames);
        drop(jobs);
        let cancelled = queue.frame(syntax);
        assert_eq!(cancelled.jobs, MAX_HIGHLIGHT_CHUNKS_PER_FRAME);
        assert_eq!(cancelled.work, HighlightWork::default());
        assert_eq!(queue.work.last_frame.parsed_chunks, 0);
        assert_eq!(queue.work.last_frame.parsed_lines, 0);
        assert_eq!(
            queue.work.last_frame.dequeued_jobs,
            MAX_HIGHLIGHT_CHUNKS_PER_FRAME
        );
        assert_eq!(queue.pending.len(), 512 - MAX_HIGHLIGHT_CHUNKS_PER_FRAME);
    }

    #[test]
    fn real_tokenizer_uses_fence_info_aliases_and_web_palettes() {
        let mut document = CodeHighlightDocument::default();
        let dark = syntax_theme_for(ThemeAppearance::Dark);
        let mut code = chunk("fn main() { let value = 42; // answer\n}\n");
        code.language = Some("rust title=main.rs".into());
        document.upsert(&code, false);
        drain(&mut document, dark);
        let highlighted = document.highlighted(&code, dark);
        assert_eq!(highlighted.status, HighlightStatus::Highlighted);
        for (word, scope) in [
            ("fn", "keyword"),
            ("main", "function"),
            ("42", "number"),
            ("answer", "comment"),
        ] {
            assert_eq!(
                style_at(&highlighted, word),
                dark.scope(scope).cloned(),
                "{word}"
            );
        }
        for (language, base) in [
            ("gitignore", "ini"),
            ("luau", "lua"),
            ("xsl", "xml"),
            ("xslt", "xml"),
            ("assembly", "asm"),
            ("s", "asm"),
            ("nasm", "asm"),
        ] {
            assert_eq!(
                find_syntax(syntax_set(), fence_language(Some(language))).map(|s| &s.name),
                find_syntax(syntax_set(), base).map(|s| &s.name),
                "{language}"
            );
            assert!(find_syntax(syntax_set(), base).is_some(), "{base}");
        }
        document.clear();
        let mut showcase = chunk("clamp(12, 0, 10); // 10\nclamp(-3, 0, 10); // 0");
        showcase.language = Some("typescript".into());
        document.upsert(&showcase, false);
        drain(&mut document, dark);
        assert_eq!(
            style_at(&document.highlighted(&showcase, dark), "clamp"),
            dark.scope("function").cloned()
        );
        let mut unknown = chunk("<b>unchanged 😀</b>");
        unknown.language = Some("unknown-language-23912".into());
        document.clear();
        document.upsert(&unknown, false);
        drain(&mut document, dark);
        let result = document.highlighted(&unknown, dark);
        assert_eq!(result.status, HighlightStatus::Plain);
        assert_eq!(result.text.as_ref(), unknown.text);
        assert!(result.lines.iter().all(|line| line.spans.is_empty()));
    }

    #[test]
    fn real_incremental_tokens_match_full_tokens_at_every_streaming_prefix() {
        // incrementalHighlighting.test.ts: every prefix, multiline constructs and embedded languages.
        for (language, source) in [
            (
                "typescript",
                "/* multi\nline comment */\nconst x = `template\n${1 + 2}`;\nconst re = /abc/;\n",
            ),
            (
                "python",
                "#!/usr/bin/python\nx = \"\"\"multi\nline\"\"\"\nprint(x)\n",
            ),
            ("bash", "#!/bin/bash\ncat <<EOF\nhello\nEOF\necho hi\n"),
            (
                "html",
                "<script>\nconst x = 1;\n</script>\n<style>\np {color:red;}\n</style>\n",
            ),
            (
                "markdown",
                "# heading\n\n```ts\nconst a = 1;\n```\n\ntext\n",
            ),
            ("rust", "fn main() {\n let x = r#\"multi\nline\"#;\n}\n"),
            (
                "tsx",
                "const element = <div\n className=\"x\">\n{value}\n</div>;\n",
            ),
            ("json", "{\n \"value\": [1,\n 2, 3]\n}\n"),
            ("yaml", "key: |\n  multiline\n  value\nnext: true\n"),
            (
                "css",
                "/* comment\n continued */\np::before {\n content: \"text\";\n}\n",
            ),
            ("sql", "SELECT 'multi\nline'\nFROM table_name;\n"),
        ] {
            let grammar = find_syntax(syntax_set(), language).expect("grammar");
            let mut incremental = ChunkTokenizer::new(Checkpoint::new(grammar));
            for end in source
                .char_indices()
                .map(|(at, _)| at)
                .chain([source.len()])
            {
                let code = &source[..end];
                let actual = incremental
                    .update(code, syntax_set(), &mut HighlightWork::default())
                    .expect("incremental")
                    .0;
                assert_eq!(
                    actual,
                    full_tokens(language, code),
                    "{language}, prefix {end}"
                );
            }
        }
    }

    #[test]
    fn real_tokenizer_preserves_multiline_state_across_recycled_chunk_boundaries() {
        let first = format!("/* begin\n{}", "still a comment\n".repeat(39));
        let second = "comment end */\nlet value = 42;";
        let mut a = chunk(&first);
        a.last = false;
        let mut b = chunk(second);
        b.first = false;
        b.start_line = 41;
        b.value_bytes = first.len()..first.len() + second.len();
        let syntax = syntax_theme_for(ThemeAppearance::Light);
        let mut document = CodeHighlightDocument::default();
        document.upsert(&b, false);
        document.upsert(&a, false);
        drain(&mut document, syntax);
        let result = document.highlighted(&b, syntax);
        assert_eq!(
            style_at(&result, "comment"),
            syntax.scope("comment").cloned()
        );
        assert_eq!(style_at(&result, "let"), syntax.scope("keyword").cloned());
        let before = document.work();
        let again = document.highlighted(&b, syntax);
        assert!(Rc::ptr_eq(&result, &again));
        drain(&mut document, syntax);
        assert_eq!(document.work(), before, "recycled mount does no parsing");
    }

    #[test]
    fn missing_prefix_waits_for_ingestion_without_publishing_empty_progress() {
        syntax_set();
        let syntax = syntax_theme_for(ThemeAppearance::Dark);
        let mut a = chunk("/* start\n");
        a.last = false;
        let mut b = chunk("still comment */");
        b.first = false;
        b.value_bytes = a.text.len()..a.text.len() + b.text.len();
        b.last = false;
        let mut c = chunk("\nlet after = 1;");
        c.first = false;
        c.value_bytes = b.value_bytes.end..b.value_bytes.end + c.text.len();
        let mut document = CodeHighlightDocument::default();
        document.upsert(&b, false);
        document.upsert(&c, false);
        let before = document.version;
        assert!(document.step(syntax));
        assert!(document.step(syntax));
        assert!(!document.step(syntax));
        assert_eq!(
            document.version, before,
            "missing source must not wake the same row forever"
        );
        document.upsert(&a, false);
        drain(&mut document, syntax);
        assert_eq!(
            style_at(&document.highlighted(&b, syntax), "still"),
            syntax.scope("comment").cloned()
        );
        assert_eq!(
            style_at(&document.highlighted(&c, syntax), "let"),
            syntax.scope("keyword").cloned()
        );
    }

    #[test]
    fn a_new_fence_reuses_cached_tokens_and_exit_checkpoint() {
        syntax_set();
        let syntax = syntax_theme_for(ThemeAppearance::Light);
        let source = chunk("let cached_unique_1234 = 2468;");
        let mut first = CodeHighlightDocument::default();
        first.upsert(&source, false);
        drain(&mut first, syntax);
        let original = first.highlighted(&source, syntax);
        let mut second = CodeHighlightDocument::default();
        second.upsert(&source, false);
        drain(&mut second, syntax);
        assert_eq!(second.work().parsed_bytes, 0);
        assert!(Rc::ptr_eq(&original, &second.highlighted(&source, syntax)));
        assert!(second.chunks[&0].exit.is_some());
    }

    #[test]
    fn growing_tail_work_is_independent_of_four_or_five_hundred_twelve_completed_chunks() {
        let syntax = syntax_theme_for(ThemeAppearance::Dark);
        let measure = |completed| {
            let mut document = CodeHighlightDocument::default();
            let mut offset = 0;
            for index in 0..completed {
                let mut code = chunk(&"let value = 1;\n".repeat(40));
                code.first = index == 0;
                code.last = false;
                code.start_line = index * 40 + 1;
                code.value_bytes = offset..offset + code.text.len();
                offset += code.text.len();
                document.upsert(&code, true);
            }
            let stable = "// completed\n".repeat(39);
            let mut tail = chunk(&format!("{stable}let"));
            tail.first = completed == 0;
            tail.start_line = completed * 40 + 1;
            tail.value_bytes = offset..offset + tail.text.len();
            document.upsert(&tail, true);
            drain(&mut document, syntax);
            let before = document.work();
            tail.text.push_str(" value = 2;\n");
            tail.value_bytes.end = offset + tail.text.len();
            document.upsert(&tail, true);
            drain(&mut document, syntax);
            let after = document.work();
            HighlightWork {
                parsed_bytes: after.parsed_bytes - before.parsed_bytes,
                parsed_lines: after.parsed_lines - before.parsed_lines,
                visited_chunks: after.visited_chunks - before.visited_chunks,
            }
        };
        let short = measure(4);
        assert_eq!(short, measure(512));
        assert_eq!(
            short,
            HighlightWork {
                parsed_bytes: "let value = 2;\n".len(),
                parsed_lines: 1,
                visited_chunks: 1
            }
        );
    }

    #[test]
    fn edits_truncation_and_crlf_inputs_reset_without_losing_source() {
        let syntax = syntax_theme_for(ThemeAppearance::Dark);
        let mut document = CodeHighlightDocument::default();
        for source in [
            "/* open\ncomment\n",
            "/* open\ncomment\n*/\nconst x = 1;",
            "const edited = 2;\nconst x = 1;",
            "const edited = 2;\n",
            "",
            "/* multi\r\nline */\r\nconst x = 1;\r\n",
        ] {
            let code = chunk(source);
            document.upsert(&code, false);
            drain(&mut document, syntax);
            let result = document.highlighted(&code, syntax);
            let spans: Vec<_> = full_tokens("rust", source)
                .into_iter()
                .filter_map(|span| {
                    syntax.scope(&span.scope).map(|style| StyledSpan {
                        range: span.range,
                        style: style.clone(),
                    })
                })
                .collect();
            let expected = highlight_code_to_lines(source, &spans, HighlightStatus::Highlighted);
            assert_eq!(result.lines, expected.lines, "{source}");
            assert_eq!(result.text.as_ref(), source);
        }
    }

    fn provider(calls: Rc<Cell<usize>>) -> HighlightCode {
        Rc::new(move |_, chunk, _| {
            calls.set(calls.get() + 1);
            Ok(if chunk.text.is_empty() {
                Vec::new()
            } else {
                vec![ScopeSpan {
                    range: 0..chunk.text.len(),
                    scope: "keyword".into(),
                }]
            })
        })
    }

    #[test]
    fn promotes_on_get_and_evicts_least_recently_used() {
        let mut cache = HighlightCache::new(2, usize::MAX);
        let a = value("a");
        cache.set(key("a"), a.clone());
        cache.set(key("b"), value("b"));
        assert!(Rc::ptr_eq(&cache.get(&key("a"), "a").expect("a"), &a));
        cache.set(key("c"), value("c"));
        assert!(cache.get(&key("b"), "b").is_none());
        assert!(cache.get(&key("a"), "a").is_some());
        assert!(cache.get(&key("c"), "c").is_some());
    }

    #[test]
    fn evicts_by_memory_and_preserves_an_entry_when_oversized_replacement_is_rejected() {
        let bytes = estimate_highlighted_size(&key("a"), &value("a"));
        let mut cache = HighlightCache::new(10, bytes * 2);
        for text in ["a", "b", "c"] {
            cache.set(key(text), value(text));
        }
        assert!(cache.get(&key("a"), "a").is_none());
        assert!(cache.get(&key("b"), "b").is_some());
        let c = cache.get(&key("c"), "c").expect("c");
        cache.set(key("c"), value(&"x".repeat(bytes * 3)));
        assert!(Rc::ptr_eq(&cache.get(&key("c"), "c").expect("kept"), &c));
        assert!(cache.memory_bytes() <= bytes * 2);
        cache.clear();
        assert_eq!(cache.memory_bytes(), 0);
        assert!(cache.is_empty());
    }

    #[test]
    fn hash_collisions_cannot_return_other_source_and_zero_capacity_keeps_nothing() {
        let mut cache = HighlightCache::default();
        let key = key("a");
        cache.set(key.clone(), value("a"));
        cache.set(key.clone(), value("b"));
        assert_eq!(cache.get(&key, "a").expect("a").text.as_ref(), "a");
        assert_eq!(cache.get(&key, "b").expect("b").text.as_ref(), "b");
        assert!(cache.get(&key, "c").is_none());
        let mut disabled = HighlightCache::new(0, usize::MAX);
        disabled.set(key, value("a"));
        assert!(disabled.is_empty());
    }

    #[test]
    fn recycling_reuses_highlights_but_language_context_and_theme_are_independent() {
        let calls = Rc::new(Cell::new(0));
        let provider = provider(calls.clone());
        let dark = syntax_theme_for(Appearance::Dark);
        let light = syntax_theme_for(Appearance::Light);
        let mut cache = HighlightCache::default();
        let mut c = chunk("let");
        let first = cache.highlight(&c, dark, "start:v1", false, Some(&provider));
        let cached = cache.highlight(&c, dark, "start:v1", false, Some(&provider));
        assert!(Rc::ptr_eq(&first, &cached));
        let other_context = cache.highlight(&c, dark, "comment:v1", false, Some(&provider));
        assert!(!Rc::ptr_eq(&first, &other_context));
        let other_theme = cache.highlight(&c, light, "start:v1", false, Some(&provider));
        assert_ne!(
            first.lines[0].spans[0].style,
            other_theme.lines[0].spans[0].style
        );
        assert_eq!(
            first.lines[0].spans[0].style,
            *dark.scope("keyword").expect("palette keyword")
        );
        c.language = Some("typescript".into());
        cache.highlight(&c, dark, "start:v1", false, Some(&provider));
        assert_eq!(calls.get(), 4);
    }

    #[test]
    fn streaming_tail_is_not_cached_but_completed_chunks_and_finished_tail_are() {
        let calls = Rc::new(Cell::new(0));
        let provider = provider(calls.clone());
        let syntax = syntax_theme_for(Appearance::Dark);
        let mut cache = HighlightCache::default();
        let mut c = chunk("let");
        for _ in 0..2 {
            cache.highlight(&c, syntax, "v1", true, Some(&provider));
        }
        assert!(cache.is_empty());
        assert_eq!(calls.get(), 2);
        c.last = false;
        let stable = cache.highlight(&c, syntax, "v1", true, Some(&provider));
        assert!(Rc::ptr_eq(
            &stable,
            &cache.highlight(&c, syntax, "v1", true, Some(&provider))
        ));
        c.last = true;
        assert!(Rc::ptr_eq(
            &stable,
            &cache.highlight(&c, syntax, "v1", false, Some(&provider))
        ));
        assert_eq!(calls.get(), 3);
    }

    #[test]
    fn absent_failed_and_malformed_highlighters_leave_exact_source_and_do_not_poison_cache() {
        let syntax = syntax_theme_for(Appearance::Dark);
        let mut cache = HighlightCache::default();
        let c = chunk("😀 let\n");
        assert_eq!(
            cache.highlight(&c, syntax, "v1", false, None).status,
            HighlightStatus::Unavailable
        );
        for ranges in [
            vec![(1, 4)],
            vec![(0, 4), (3, 6)],
            vec![(0, 100)],
            vec![(4, 4)],
        ] {
            let provider: HighlightCode = Rc::new(move |_, _, _| {
                Ok(ranges
                    .iter()
                    .map(|(start, end)| ScopeSpan {
                        range: *start..*end,
                        scope: "keyword".into(),
                    })
                    .collect())
            });
            let result = cache.highlight(&c, syntax, "v1", false, Some(&provider));
            assert!(matches!(result.status, HighlightStatus::Failed(_)));
            assert_eq!(result.text.as_ref(), c.text);
            assert!(result.lines.iter().all(|line| line.spans.is_empty()));
        }
        let failure: HighlightCode = Rc::new(|_, _, _| Err("unknown language".into()));
        assert_eq!(
            cache
                .highlight(&c, syntax, "v1", false, Some(&failure))
                .status,
            HighlightStatus::Failed("unknown language".into())
        );
        assert!(cache.is_empty());
        let provider = provider(Rc::new(Cell::new(0)));
        assert_eq!(
            cache
                .highlight(&c, syntax, "v1", false, Some(&provider))
                .status,
            HighlightStatus::Highlighted
        );
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn plain_aliases_skip_lexer_and_gitignore_uses_the_existing_ini_alias() {
        let syntax = syntax_theme_for(Appearance::Dark);
        let mut cache = HighlightCache::default();
        let provider: HighlightCode = Rc::new(|language, _, _| {
            assert_eq!(language, "ini");
            Ok(Vec::new())
        });
        for language in ["text", "TXT", "plain", "plaintext", ""] {
            let mut c = chunk("let");
            c.language = Some(language.into());
            assert_eq!(
                cache
                    .highlight(&c, syntax, "v1", false, Some(&provider))
                    .status,
                HighlightStatus::Plain
            );
        }
        let mut c = chunk("target/");
        c.language = Some("gitignore".into());
        assert_eq!(
            cache
                .highlight(&c, syntax, "v1", false, Some(&provider))
                .status,
            HighlightStatus::Highlighted
        );
    }

    #[test]
    fn append_finish_edits_and_truncation_retain_only_unchanged_line_identity() {
        let mut document = IncrementalHighlightedDocument::default();
        let first = document.update(value("let a = 1;\nlet b"));
        let next = document.update(value("let a = 1;\nlet b = 2;"));
        assert!(Rc::ptr_eq(&first.lines[0], &next.lines[0]));
        assert!(!Rc::ptr_eq(&first.lines[1], &next.lines[1]));
        let finished = document.update(value("let a = 1;\nlet b = 2;"));
        for (a, b) in next.lines.iter().zip(&finished.lines) {
            assert!(Rc::ptr_eq(a, b));
        }
        let edited = document.update(value("let a = 3;\nlet b = 2;"));
        assert!(!Rc::ptr_eq(&finished.lines[0], &edited.lines[0]));
        assert!(Rc::ptr_eq(&finished.lines[1], &edited.lines[1]));
        assert_eq!(document.update(value("")).text.as_ref(), "");
        document.clear();
        let reset = document.update(value("let a = 1;\nlet b = 2;"));
        assert!(!Rc::ptr_eq(&reset.lines[0], &finished.lines[0]));
    }

    #[test]
    fn line_splitting_is_lossless_at_every_unicode_crlf_and_cr_streaming_prefix() {
        let source = "😀 café\r\n\r\nlet x = 1;\rnext\n";
        for end in source
            .char_indices()
            .map(|(at, _)| at)
            .chain([source.len()])
        {
            let prefix = &source[..end];
            let highlighted = value(prefix);
            assert_eq!(highlighted.text.as_ref(), prefix);
            assert_eq!(
                highlighted
                    .lines
                    .iter()
                    .map(|line| line.text.as_str())
                    .collect::<String>(),
                prefix
            );
        }
        let syntax = syntax_theme_for(Appearance::Dark);
        let provider: HighlightCode = Rc::new(|_, c, _| {
            Ok(vec![ScopeSpan {
                range: 0..c.text.len(),
                scope: "comment".into(),
            }])
        });
        let code = HighlightCache::default().highlight(
            &chunk(source),
            syntax,
            "comment",
            false,
            Some(&provider),
        );
        for line in &code.lines {
            assert_eq!(line.spans[0].range, 0..line.text.len());
            assert_eq!(
                line.spans[0].style,
                *syntax.scope("comment").expect("comment style")
            );
        }
    }

    #[test]
    fn five_hundred_code_lines_use_thirteen_bounded_cached_chunks() {
        let source = format!(
            "```rs\n{}\n```",
            (0..500)
                .map(|n| format!("let v{n} = {n};"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        let blocks = MessageFlattener::new("message").replace("message", &source);
        let mut cache = HighlightCache::default();
        let provider = provider(Rc::new(Cell::new(0)));
        let syntax = syntax_theme_for(Appearance::Dark);
        let mut count = 0;
        for block in blocks.rows.iter() {
            if let BlockKind::Code(chunk) = &block.kind {
                let code =
                    cache.highlight(chunk, syntax, "test-lexer:start", false, Some(&provider));
                assert!(code.lines.len() <= CODE_CHUNK_LINES);
                assert_eq!(code.text.as_ref(), chunk.text);
                count += 1;
            }
        }
        assert_eq!(count, 13);
        assert_eq!(cache.len(), 13);
    }
}
