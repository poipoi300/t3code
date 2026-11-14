//! Port of apps/web/src/components/chat/MarkdownCodeBlock.tsx SuspenseShikiCodeBlock and UncachedShikiCodeBlock.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;
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

static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();

// Match NSH's loader boundary: only native binaries contain two-face's syntax dump.
#[cfg(not(target_arch = "wasm32"))]
pub async fn load_code_syntaxes() -> bool {
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
    Some(if is("comment") || is("punctuation.definition.comment") {
        "comment"
    } else if is("punctuation.definition.string") { "string"
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
    } else if is("entity.name.function") || is("support.function") {
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
        self.pending.insert(at);
        true
    }

    /// Parses one queued chunk and publishes its exit checkpoint to the next chunk.
    pub fn step(&mut self, syntax: &SyntaxTheme) -> bool {
        let Some(at) = self.pending.pop_first() else {
            return false;
        };
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
        let key = CacheKey { language: language.clone(), text_hash: text_hash(&record.chunk.text),
            theme: syntax.name.clone(), context: record.context.clone() };
        let cached = (!record.streaming || !record.chunk.last).then(|| self.cache.borrow_mut().get_parsed(&key, &record.chunk.text)).flatten();
        let (value, exit) = if let Some(cached) = cached { cached } else {
            let tokenizer = record.tokenizer.get_or_insert_with(|| ChunkTokenizer::new(entry));
            let (spans, exit) = match tokenizer.update(&record.chunk.text, set, &mut self.work) {
                Ok(result) => result,
                Err(error) => { record.failure = Some(error); record.tokenizer = None; return true; }
            };
            let provider: HighlightCode = Rc::new(move |_, _, _| Ok(spans.clone()));
            let value = self.cache.borrow_mut().highlight(&record.chunk, syntax, &record.context, record.streaming, Some(&provider));
            self.cache.borrow_mut().remember_exit(&key, &record.chunk.text, exit.clone());
            (value, exit)
        };
        let changed_exit = record.exit.as_ref() != Some(&exit);
        record.exit = Some(exit);
        record.failure = None;
        record.result =
            (record.streaming && record.chunk.last).then(|| (syntax.name.clone(), value));
        if !record.streaming || !record.chunk.last {
            record.tokenizer = None;
        }
        if changed_exit && let Some((&next, _)) = self.chunks.range((at + 1)..).next() {
            self.pending.insert(next);
        }
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
            return Rc::new(highlight_code_to_lines(
                &chunk.text,
                &[],
                HighlightStatus::Plain,
            ));
        }
        if let Some(record) = self.chunks.get(&chunk.value_bytes.start) {
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
            if let Some(error) = &record.failure {
                return Rc::new(highlight_code_to_lines(
                    &chunk.text,
                    &[],
                    HighlightStatus::Failed(error.clone()),
                ));
            }
        }
        self.pending.insert(chunk.value_bytes.start);
        Rc::new(highlight_code_to_lines(
            &chunk.text,
            &[],
            HighlightStatus::Unavailable,
        ))
    }
}

/// One per fence, created above virtual cells. Feed every changed code chunk, even offscreen ones.
#[derive(Clone)]
pub struct CodeHighlightState {
    document: Rc<RefCell<CodeHighlightDocument>>,
    revision: Signal<u64>,
    wake: mpsc::UnboundedSender<()>,
}

impl Default for CodeHighlightState {
    fn default() -> Self {
        Self::new()
    }
}

impl CodeHighlightState {
    pub fn new() -> Self {
        let document = Rc::new(RefCell::new(CodeHighlightDocument::default()));
        let revision = signal(0);
        let (wake, mut messages) = mpsc::unbounded();
        let processing = document.clone();
        spawn(async move {
            if !load_code_syntaxes().await {
                return;
            }
            while messages.next().await.is_some() {
                while messages.try_recv().is_ok() {}
                loop {
                    let progressed = processing.borrow_mut().step(code_syntax_theme());
                    if !progressed {
                        break;
                    }
                    revision.update(|value| *value += 1);
                    next_frame().await;
                }
            }
        });
        Self {
            document,
            revision,
            wake,
        }
    }

    pub fn upsert(&self, chunk: &CodeChunk, streaming: bool) {
        if self.document.borrow_mut().upsert(chunk, streaming) {
            let _ = self.wake.unbounded_send(());
        }
    }

    pub fn clear(&self) {
        self.document.borrow_mut().clear();
        self.revision.update(|revision| *revision += 1);
    }

    pub fn highlighted(&self, chunk: &CodeChunk) -> Shared<HighlightedCode> {
        self.revision.get();
        let result = self
            .document
            .borrow_mut()
            .highlighted(chunk, code_syntax_theme());
        if result.status == HighlightStatus::Unavailable {
            let _ = self.wake.unbounded_send(());
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

    fn get_parsed(&mut self, key: &CacheKey, text: &str) -> Option<(Rc<HighlightedCode>, Checkpoint)> {
        let value = self.get(key, text)?;
        Some((value, self.entries.back()?.exit.clone()?))
    }

    fn remember_exit(&mut self, key: &CacheKey, text: &str, exit: Checkpoint) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.key == *key && e.value.text.as_ref() == text) {
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
        self.entries.push_back(CacheEntry { key, value, bytes, exit: None });
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
fn highlight_code_to_lines(
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
        .text_color(move || {
            if crate::theme::has_theme_id() {
                return palette().code_foreground;
            }
            highlighted
                .with(|code| {
                    code.as_ref()
                        .and_then(|code| code.foreground.as_deref().and_then(hex_color))
                })
                .unwrap_or_else(|| palette().foreground.opacity(0.9))
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
        let blocks = MessageFlattener::new("message").update(&source);
        let mut cache = HighlightCache::default();
        let provider = provider(Rc::new(Cell::new(0)));
        let syntax = syntax_theme_for(Appearance::Dark);
        let mut count = 0;
        for block in blocks.blocks.iter() {
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
