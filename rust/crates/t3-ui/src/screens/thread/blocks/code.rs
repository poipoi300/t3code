//! Port of apps/web/src/components/chat/MarkdownCodeBlock.tsx SuspenseShikiCodeBlock and UncachedShikiCodeBlock.

use std::collections::VecDeque;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;

use nsh_core::element::StyleRefinement;
use nsh_core::prelude::*;
use t3_theme::syntax::{ScopeStyle, SyntaxTheme};

use super::flatten::CodeChunk;
use crate::runtime::Shared;
use crate::theme::{cn, palette, tw};

pub const MAX_HIGHLIGHT_CACHE_ENTRIES: usize = 500;
pub const MAX_HIGHLIGHT_CACHE_MEMORY_BYTES: usize = 50 * 1024 * 1024;

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
        let language = super::super::markdown::code_block::highlighter_language_of(
            chunk.language.as_deref().unwrap_or("text"),
        );
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
            Ok(spans) => highlight_code_to_lines(&chunk.text, &spans, HighlightStatus::Highlighted),
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
        self.entries.push_back(CacheEntry { key, value, bytes });
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
    view().child(each(
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
        .text_color(move || palette().code_foreground)
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
