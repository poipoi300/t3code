use crate::{
    Error, Result,
    driver::{BrowserCommand, BrowserDriver},
    lock,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};
use t3_contracts::base_schemas::ThreadId;

pub const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_PAGE_BYTES: usize = 25 * 1024 * 1024;
pub const MEASURE_WIDTHS: &[u32] = &[320, 375, 430, 520, 640, 728, 860, 1000, 1144];

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HtmlRenderReference {
    pub attachment_id: String,
    pub title: String,
    pub height: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heights: Option<Vec<(u32, u32)>>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", untagged)]
pub enum HtmlSource {
    Html { html: String },
    Path { path: PathBuf },
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PublishedSource {
    pub source: HtmlSource,
    pub title: String,
    pub height: f64,
}

/// Provider-session ownership lives outside this crate; replacement/clear must drop its records.
pub trait HtmlRenderRecovery: Send + Sync {
    fn record(
        &self,
        thread: &ThreadId,
        source: &PublishedSource,
        reference: &HtmlRenderReference,
    ) -> Result<()>;
    fn forget(&self, thread: &ThreadId, reference: &HtmlRenderReference);
}
type Recoveries = BTreeMap<String, Vec<(PublishedSource, HtmlRenderReference)>>;
#[derive(Default)]
pub struct MemoryHtmlRecovery {
    records: Mutex<Recoveries>,
}
impl MemoryHtmlRecovery {
    pub fn take(&self, thread: &ThreadId, source: &PublishedSource) -> Option<HtmlRenderReference> {
        let mut records = lock(&self.records);
        let entries = records.get_mut(thread.0.as_str())?;
        let index = entries.iter().position(|(s, _)| s == source)?;
        Some(entries.remove(index).1)
    }
    pub fn clear(&self, thread: &ThreadId) {
        lock(&self.records).remove(thread.0.as_str());
    }
}
impl HtmlRenderRecovery for MemoryHtmlRecovery {
    fn record(
        &self,
        thread: &ThreadId,
        source: &PublishedSource,
        reference: &HtmlRenderReference,
    ) -> Result<()> {
        lock(&self.records)
            .entry(thread.0.clone())
            .or_default()
            .push((source.clone(), reference.clone()));
        Ok(())
    }
    fn forget(&self, thread: &ThreadId, reference: &HtmlRenderReference) {
        if let Some(entries) = lock(&self.records).get_mut(thread.0.as_str()) {
            entries.retain(|(_, r)| r.attachment_id != reference.attachment_id);
        }
    }
}
pub struct HtmlOptions {
    pub attachments_dir: PathBuf,
    pub state_dir: PathBuf,
    pub secrets_dir: PathBuf,
    pub recovery: Arc<dyn HtmlRenderRecovery>,
}
#[derive(Clone)]
pub struct HtmlService {
    #[cfg(test)]
    work_threads: Arc<Mutex<Vec<std::thread::ThreadId>>>,
    #[cfg(test)]
    write_gate: Option<Arc<WriteGate>>,
    options: Arc<HtmlOptions>,
    driver: Option<Arc<dyn BrowserDriver>>,
    browsers: Arc<tokio::sync::Semaphore>,
}
#[cfg(test)]
#[derive(Default)]
struct WriteGate {
    entered: tokio::sync::Notify,
    released: Mutex<bool>,
    changed: std::sync::Condvar,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HtmlPreview {
    pub png: String,
    pub width: u32,
    pub content_height: u32,
    pub captured_height: u32,
    pub console_messages: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing_images: Vec<String>,
}

impl HtmlService {
    pub fn new(options: HtmlOptions, driver: Option<Arc<dyn BrowserDriver>>) -> Self {
        Self {
            #[cfg(test)]
            work_threads: Arc::default(),
            #[cfg(test)]
            write_gate: None,
            options: Arc::new(options),
            driver,
            browsers: Arc::new(tokio::sync::Semaphore::new(2)),
        }
    }
    pub async fn prepare(&self, html: &str) -> Result<String> {
        let inlined = self.inline_document(html.to_owned()).await?;
        if !inlined.1.is_empty() {
            return Err(Error::Operation {
                tag: "HtmlRenderImagesNotFoundError".into(),
                message: format!(
                    "These local images could not be read: {}. Use absolute paths to existing image files, or remove them.",
                    inlined.1.join(", ")
                ),
                detail: Some(serde_json::json!({"paths":inlined.1})),
            });
        }
        Ok(inlined.0)
    }
    async fn inline_document(&self, source: String) -> Result<(String, Vec<String>)> {
        #[cfg(test)]
        let work_threads = self.work_threads.clone();
        tokio::task::spawn_blocking(move || {
            #[cfg(test)]
            lock(&work_threads).push(std::thread::current().id());
            reject_viewport_height(&source)?;
            inline_images(inject_bootstrap(&source))
        })
        .await
        .map_err(|error| Error::Io(error.to_string()))?
    }
    pub async fn read_source(&self, source: &HtmlSource) -> Result<String> {
        match source {
            HtmlSource::Html { html } => Ok(html.clone()),
            HtmlSource::Path { path } => {
                let options = self.options.clone();
                let path = path.clone();
                tokio::task::spawn_blocking(move || {
                    read_page_file(&path, &options.state_dir, &options.secrets_dir)
                })
                .await
                .map_err(|e| Error::Io(e.to_string()))?
            }
        }
    }
    pub async fn publish(
        &self,
        thread: ThreadId,
        input: PublishedSource,
    ) -> Result<HtmlRenderReference> {
        self.publish_admitted(thread, input, Arc::new(|| Box::pin(async { Ok(()) })))
            .await
    }
    pub async fn publish_admitted(
        &self,
        thread: ThreadId,
        input: PublishedSource,
        admission: crate::Admission,
    ) -> Result<HtmlRenderReference> {
        if !input.height.is_finite() {
            return Err(Error::InvalidInput("Height must be finite".into()));
        }
        let source = self.read_source(&input.source).await?;
        let html = self.prepare(&source).await?;
        let segment = safe_thread_segment(thread.0.as_str()).ok_or_else(|| {
            Error::operation(
                "HtmlRenderStoreError",
                "The HTML render could not be saved: invalid thread id.",
            )
        })?;
        let id = format!("{segment}-{}-html", uuid::Uuid::new_v4());
        let path = self.options.attachments_dir.join(format!("{id}.html"));
        let mut reference = HtmlRenderReference {
            attachment_id: id,
            title: truncate_utf16(input.title.trim(), 200),
            height: input.height.round().clamp(80.0, 2000.0) as u32,
            heights: None,
        };
        if reference.title.is_empty() {
            reference.title = "HTML".into();
        }
        tokio::fs::create_dir_all(&self.options.attachments_dir).await?;
        admission().await?;
        let rollback = PublishedFile {
            path: path.clone(),
            thread: thread.clone(),
            reference: reference.clone(),
            recovery: self.options.recovery.clone(),
            committed: false,
        };
        #[cfg(test)]
        let work_threads = self.work_threads.clone();
        #[cfg(test)]
        let gate = self.write_gate.clone();
        // The job owns rollback until its write finishes, even when its async waiter is cancelled.
        let (mut rollback, html) = tokio::task::spawn_blocking(move || {
            #[cfg(test)]
            lock(&work_threads).push(std::thread::current().id());
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map_err(|error| Error::operation("HtmlRenderStoreError", error.to_string()))?;
            #[cfg(test)]
            if let Some(gate) = gate {
                gate.entered.notify_one();
                let mut released = lock(&gate.released);
                while !*released {
                    released = gate.changed.wait(released).expect("write gate");
                }
            }
            use std::io::Write;
            file.write_all(html.as_bytes())
                .map_err(|error| Error::operation("HtmlRenderStoreError", error.to_string()))?;
            drop(file);
            Ok::<_, Error>((rollback, html))
        })
        .await
        .map_err(|error| Error::Io(error.to_string()))??;
        if let Some(driver) = &self.driver {
            let measure = async {
                let _permit = self
                    .browsers
                    .acquire()
                    .await
                    .map_err(|_| Error::Disconnected)?;
                driver
                    .call(
                        BrowserCommand::HtmlMeasure {
                            html,
                            widths: MEASURE_WIDTHS.to_vec(),
                            fragment: include_str!("../assets/dark-fragment.txt").into(),
                        },
                        Duration::from_secs(6),
                    )
                    .await
            };
            if let Ok(Ok(value)) = tokio::time::timeout(Duration::from_secs(6), measure).await
                && let Ok(mut heights) = serde_json::from_value::<Vec<(u32, u32)>>(value)
            {
                heights.sort_by_key(|(width, _)| *width);
                reference.heights = Some(heights);
            }
        }
        self.options.recovery.record(&thread, &input, &reference)?;
        rollback.committed = true;
        Ok(reference)
    }
    pub async fn preview(
        &self,
        source: HtmlSource,
        width: Option<f64>,
        appearance: Option<&str>,
    ) -> Result<HtmlPreview> {
        let width = width.unwrap_or(728.0);
        if !width.is_finite() {
            return Err(Error::InvalidInput("Width must be finite".into()));
        }
        let width = width.round().clamp(240.0, 1600.0) as u32;
        let appearance = appearance.unwrap_or("dark");
        if !matches!(appearance, "light" | "dark") {
            return Err(Error::InvalidInput("Invalid appearance".into()));
        }
        let source = self.read_source(&source).await?;
        let (html, missing) = self.inline_document(source).await?;
        let driver = self.driver.as_ref().ok_or_else(|| {
            Error::operation(
                "HtmlRenderBrowserError",
                "No headless browser driver is configured.",
            )
        })?;
        let _permit = self
            .browsers
            .acquire()
            .await
            .map_err(|_| Error::Disconnected)?;
        let value = driver
            .call(
                BrowserCommand::HtmlCapture {
                    html,
                    width,
                    appearance: appearance.into(),
                    fragment: if appearance == "light" {
                        include_str!("../assets/light-fragment.txt")
                    } else {
                        include_str!("../assets/dark-fragment.txt")
                    }
                    .into(),
                },
                Duration::from_secs(20),
            )
            .await?;
        Ok(HtmlPreview {
            png: value["png"]
                .as_str()
                .ok_or_else(|| Error::InvalidInput("Missing HTML screenshot".into()))?
                .into(),
            width,
            content_height: required_u32(&value, "contentHeight")?,
            captured_height: required_u32(&value, "capturedHeight")?,
            console_messages: serde_json::from_value(value["consoleMessages"].clone())?,
            missing_images: missing,
        })
    }
}
fn required_u32(value: &Value, key: &str) -> Result<u32> {
    value[key]
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| Error::InvalidInput(format!("Invalid {key}")))
}
struct PublishedFile {
    path: PathBuf,
    thread: ThreadId,
    reference: HtmlRenderReference,
    recovery: Arc<dyn HtmlRenderRecovery>,
    committed: bool,
}
impl Drop for PublishedFile {
    fn drop(&mut self) {
        if !self.committed {
            let _ = std::fs::remove_file(&self.path);
            self.recovery.forget(&self.thread, &self.reference);
        }
    }
}

pub fn safe_thread_segment(thread: &str) -> Option<String> {
    let mut segment = String::new();
    let mut hyphen = false;
    for ch in thread.trim().to_ascii_lowercase().chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            segment.push(ch);
            hyphen = false;
        } else if !hyphen {
            segment.push('-');
            hyphen = true;
        }
    }
    let segment = segment
        .trim_matches(['-', '_'])
        .chars()
        .take(80)
        .collect::<String>();
    let segment = segment.trim_end_matches(['-', '_']);
    if segment.is_empty() {
        None
    } else {
        Some(if segment == "pending" {
            "_pending".into()
        } else {
            segment.into()
        })
    }
}
pub fn truncate_utf16(text: &str, limit: usize) -> String {
    let mut length = 0;
    text.chars()
        .take_while(|ch| {
            length += ch.len_utf16();
            length <= limit
        })
        .collect()
}
fn rx(pattern: &'static str) -> Regex {
    static COMPILED: OnceLock<Mutex<BTreeMap<&'static str, Regex>>> = OnceLock::new();
    lock(COMPILED.get_or_init(Mutex::default))
        .entry(pattern)
        .or_insert_with(|| Regex::new(pattern).expect("literal regex"))
        .clone()
}
fn raw_text_closing(tag: &str) -> &'static Regex {
    static CLOSING: OnceLock<Vec<(&str, Regex)>> = OnceLock::new();
    &CLOSING
        .get_or_init(|| {
            [
                "script", "style", "textarea", "title", "xmp", "iframe", "noembed", "noframes",
                "noscript",
            ]
            .into_iter()
            .map(|tag| {
                (
                    tag,
                    Regex::new(&format!(r"(?i)</{tag}\s*>")).expect("known raw text tag"),
                )
            })
            .collect()
        })
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(tag))
        .expect("known raw text tag")
        .1
}
pub fn reject_viewport_height(html: &str) -> Result<()> {
    static SOURCE: OnceLock<Regex> = OnceLock::new();
    static UNIT: OnceLock<Regex> = OnceLock::new();
    let sources=SOURCE.get_or_init(||rx(r#"(?is)<!--[\s\S]*?(?:-->|$)|<style\b[^>]*(?:>|$)([\s\S]*?)(?:</style\s*>|$)|\sstyle\s*=\s*(?:"([^"]*)(?:"|$)|'([^']*)(?:'|$)|([^\s>]+))"#));
    let units = UNIT.get_or_init(|| rx(r"(?i)(?:\d+(?:\.\d+)?|\.\d+)[sld]?vh"));
    for cap in sources.captures_iter(html) {
        let css = (1..=4)
            .find_map(|i| cap.get(i))
            .map(|m| m.as_str())
            .unwrap_or("");
        for found in units.find_iter(css) {
            let before = css[..found.start()].chars().next_back();
            let after = css[found.end()..].chars().next();
            if before.is_none_or(|ch| !ch.is_alphanumeric() && !"_.+/".contains(ch))
                && after.is_none_or(|ch| !ch.is_alphanumeric() && !"_-".contains(ch))
            {
                return Err(Error::operation(
                    "HtmlRenderViewportHeightError",
                    format!(
                        "The page uses a viewport height unit ({}). Let content set the height, or use fixed pixel heights.",
                        found.as_str()
                    ),
                ));
            }
        }
    }
    Ok(())
}

pub fn inject_bootstrap(html: &str) -> String {
    static INERT: OnceLock<Regex> = OnceLock::new();
    static TEMPLATE: OnceLock<Regex> = OnceLock::new();
    let inert=INERT.get_or_init(||rx(r"(?i)<!--|<(script|style|textarea|title|xmp|iframe|noembed|noframes|noscript|plaintext)\b"));
    let mut scan = String::with_capacity(html.len());
    let mut cursor = 0;
    while let Some(cap) = inert.captures(&html[cursor..]) {
        let found = cap.get(0).expect("inert opener");
        let start = cursor + found.start();
        let after = cursor + found.end();
        scan.push_str(&html[cursor..start]);
        let end = if let Some(tag) = cap.get(1) {
            if tag.as_str().eq_ignore_ascii_case("plaintext") {
                html.len()
            } else {
                raw_text_closing(tag.as_str())
                    .find(&html[after..])
                    .map_or(html.len(), |close| after + close.end())
            }
        } else {
            html[after..]
                .find("-->")
                .map_or(html.len(), |at| after + at + 3)
        };
        scan.push_str(&" ".repeat(end - start));
        cursor = end;
    }
    scan.push_str(&html[cursor..]);
    let template = TEMPLATE.get_or_init(|| rx(r"(?i)<(/?)template(?:\s[^>]*)?/?>"));
    let mut depth = 0;
    let mut start = 0;
    let mut spans = Vec::new();
    for cap in template.captures_iter(&scan) {
        let span = cap.get(0).expect("template match");
        if cap[1].is_empty() {
            if depth == 0 {
                start = span.start();
            }
            depth += 1;
        } else if depth > 0 {
            depth -= 1;
            if depth == 0 {
                spans.push((start, span.end()));
            }
        }
    }
    if depth > 0 {
        spans.push((start, scan.len()));
    }
    for (start, end) in spans {
        scan.replace_range(start..end, &" ".repeat(end - start));
    }
    let mut markup = String::new();
    if !rx(r"(?i)<meta\s[^>]*charset").is_match(&scan[..truncate_utf16(html, 4096).len()]) {
        markup.push_str("<meta charset=\"utf-8\">");
    }
    if !rx(r#"(?i)<meta\s[^>]*name\s*=\s*["']?viewport"#).is_match(&scan) {
        markup.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">");
    }
    markup.push_str(include_str!("../assets/bootstrap.html"));
    for (pattern, wrap) in [
        (r#"(?i)<head(?:\s(?:[^"'>]|"[^"]*"|'[^']*')*)?>"#, false),
        (r#"(?i)<html(?:\s(?:[^"'>]|"[^"]*"|'[^']*')*)?>"#, true),
    ] {
        if let Some(found) = rx(pattern).find(&scan) {
            let at = found.end();
            let insert = if wrap {
                format!("<head>{markup}</head>")
            } else {
                markup
            };
            return format!("{}{}{}", &html[..at], insert, &html[at..]);
        }
    }
    if let Some(doctype) = rx(r"(?i)^\s*<!doctype[^>]*>").find(html) {
        let at = doctype.end();
        return format!("{}<head>{markup}</head>{}", &html[..at], &html[at..]);
    }
    format!("<!doctype html><head>{markup}</head>{html}")
}

struct ImageRef {
    start: usize,
    end: usize,
    path: String,
}
fn image_mime(path: &str) -> Option<&'static str> {
    match path.rsplit('.').next()?.to_ascii_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "avif" => Some("image/avif"),
        "svg" => Some("image/svg+xml"),
        "bmp" => Some("image/bmp"),
        "ico" => Some("image/x-icon"),
        _ => None,
    }
}
fn absolute_image(path: &str) -> bool {
    let b = path.as_bytes();
    path.starts_with('/') && !path.starts_with("//")
        || b.len() >= 3
            && b[0].is_ascii_alphabetic()
            && b[1] == b':'
            && matches!(b[2], b'/' | b'\\')
}
fn image_refs(html: &str) -> Vec<ImageRef> {
    let bytes = html.as_bytes();
    let mut out = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        if matches!(bytes[at], b'\'' | b'"' | b'`') {
            let quote = bytes[at];
            let start = at + 1;
            if let Some(offset) = bytes[start..]
                .iter()
                .take(2060)
                .position(|b| *b == quote || *b == b'\r' || *b == b'\n')
            {
                let end = start + offset;
                if bytes[end] == quote
                    && let Some(path) = html.get(start..end)
                    && path.chars().count() <= 2055
                    && absolute_image(path)
                    && image_mime(path).is_some()
                {
                    out.push(ImageRef {
                        start,
                        end,
                        path: path.into(),
                    });
                    at = end;
                }
            }
        }
        at += 1;
    }
    static URL: OnceLock<Regex> = OnceLock::new();
    for cap in URL.get_or_init(||rx(r#"(?i)url\(\s*((?:/|[a-z]:[\\/])[^\s"'`()]{0,2048}?\.(?:png|jpg|jpeg|gif|webp|avif|svg|bmp|ico))\s*\)"#)).captures_iter(html) {
        let p=cap.get(1).expect("image path match");if absolute_image(p.as_str()) {out.push(ImageRef {start:p.start(),end:p.end(),path:p.as_str().into()});}
    }
    out.sort_by_key(|r| r.start);
    out.dedup_by_key(|r| r.start);
    out
}
fn inline_images(html: String) -> Result<(String, Vec<String>)> {
    let refs = image_refs(&html);
    let mut files = Vec::new();
    for reference in &refs {
        if !files.contains(&reference.path) {
            files.push(reference.path.clone());
        }
    }
    let mut images = BTreeMap::new();
    let mut missing = Vec::new();
    let mut projected = html.len();
    let mut read_bytes = 0usize;
    for path in &files {
        let file = if path.as_bytes().get(1) == Some(&b':') {
            path.replace("\\\\", "\\")
        } else {
            path.clone()
        };
        let size = std::fs::metadata(&file)
            .ok()
            .filter(|m| m.is_file())
            .map(|m| m.len() as usize);
        if let Some(size) = size {
            if size > MAX_IMAGE_BYTES {
                return Err(Error::operation(
                    "HtmlRenderImageTooLargeError",
                    format!("{path} exceeds the 10 MiB local image limit."),
                ));
            }
            let prefix = format!(
                "data:{};base64,",
                image_mime(path).expect("known image extension")
            );
            for reference in refs.iter().filter(|r| r.path == *path) {
                projected = projected
                    .saturating_add(prefix.len() + size.div_ceil(3) * 4)
                    .saturating_sub(reference.path.len());
            }
            if projected > MAX_PAGE_BYTES {
                return Err(Error::operation(
                    "HtmlRenderPageTooLargeError",
                    "The page with its inlined images exceeds 25 MiB.",
                ));
            }
            let path_owned = file.clone();
            let bytes = (|| {
                let file = open_regular_file(Path::new(&path_owned), false)?;
                let mut bytes = Vec::new();
                file.take((MAX_IMAGE_BYTES + 1) as u64)
                    .read_to_end(&mut bytes)?;
                Ok::<_, std::io::Error>(bytes)
            })();
            if let Ok(bytes) = bytes
                && is_image_bytes(&bytes)
            {
                if bytes.len() > MAX_IMAGE_BYTES {
                    return Err(Error::operation(
                        "HtmlRenderImageTooLargeError",
                        "Local image grew beyond 10 MiB.",
                    ));
                }
                read_bytes += bytes.len().div_ceil(3) * 4;
                if read_bytes > MAX_PAGE_BYTES {
                    return Err(Error::operation(
                        "HtmlRenderPageTooLargeError",
                        "Inlined images exceed 25 MiB.",
                    ));
                }
                images.insert(path.clone(), prefix + &STANDARD.encode(bytes));
                continue;
            }
        }
        missing.push(path.clone());
    }
    let mut out = String::new();
    let mut cursor = 0;
    for reference in refs {
        if let Some(data) = images.get(&reference.path) {
            if reference.start < cursor {
                continue;
            }
            out.push_str(&html[cursor..reference.start]);
            out.push_str(data);
            cursor = reference.end;
        }
    }
    out.push_str(&html[cursor..]);
    if out.len() > MAX_PAGE_BYTES {
        return Err(Error::operation(
            "HtmlRenderPageTooLargeError",
            "Page exceeds 25 MiB.",
        ));
    }
    Ok((out, missing))
}
pub fn is_image_bytes(bytes: &[u8]) -> bool {
    if bytes.starts_with(b"\x89PNG")
        || bytes.starts_with(b"\xff\xd8\xff")
        || bytes.starts_with(b"GIF8")
        || bytes.starts_with(b"\0\0\x01\0")
        || bytes.starts_with(b"BM") && bytes.get(6..10) == Some(&[0, 0, 0, 0])
        || bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP")
        || matches!(
            bytes.get(4..12),
            Some(b"ftypavif" | b"ftypavis" | b"ftypmif1")
        )
    {
        return true;
    }
    has_svg_root(&String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]))
}
pub fn has_svg_root(text: &str) -> bool {
    let mut at = 0;
    loop {
        while text
            .get(at..)
            .and_then(|s| s.chars().next())
            .is_some_and(char::is_whitespace)
        {
            at += text[at..]
                .chars()
                .next()
                .expect("character exists")
                .len_utf8();
        }
        let rest = &text[at..];
        let token = if rest.starts_with("<?") {
            Some(("?>", 2))
        } else if rest.starts_with("<!--") {
            Some(("-->", 4))
        } else {
            None
        };
        if let Some((end, start)) = token {
            if let Some(offset) = rest[start..].find(end) {
                at += start + offset + end.len();
                continue;
            }
            return false;
        }
        if rest
            .get(..9)
            .is_some_and(|s| s.eq_ignore_ascii_case("<!doctype"))
        {
            let bytes = text.as_bytes();
            let mut subset = false;
            let mut cursor = at + 9;
            let mut closed = false;
            while cursor < bytes.len() {
                if matches!(bytes[cursor], b'\'' | b'"') {
                    let quote = bytes[cursor];
                    cursor += 1;
                    while cursor < bytes.len() && bytes[cursor] != quote {
                        cursor += 1;
                    }
                } else if subset && bytes[cursor..].starts_with(b"<!--") {
                    if let Some(end) = text[cursor + 4..].find("-->") {
                        cursor += 4 + end + 2;
                    } else {
                        return false;
                    }
                } else if subset && bytes[cursor..].starts_with(b"<?") {
                    if let Some(end) = text[cursor + 2..].find("?>") {
                        cursor += 2 + end + 1;
                    } else {
                        return false;
                    }
                } else if bytes[cursor] == b'>' && !subset {
                    at = cursor + 1;
                    closed = true;
                    break;
                } else if bytes[cursor] == b'[' {
                    subset = true;
                } else if bytes[cursor] == b']' {
                    subset = false;
                }
                cursor += 1;
            }
            if !closed {
                return false;
            }
            continue;
        }
        return rest.starts_with("<svg")
            && rest
                .as_bytes()
                .get(4)
                .is_some_and(|b| b" \t\r\n/>".contains(b));
    }
}

pub fn read_page_file(path: &Path, state_dir: &Path, secrets_dir: &Path) -> Result<String> {
    let error = || {
        Error::operation(
            "HtmlRenderFileError",
            format!(
                "{} is not a readable HTML document. Pass an absolute path to a regular .html file that starts with <!doctype html> or <html>, or pass html instead.",
                path.display()
            ),
        )
    };
    let requested = path.to_string_lossy();
    if !path.is_absolute() || requested.starts_with("\\\\") || requested.starts_with("//") {
        return Err(error());
    }
    let real = std::fs::canonicalize(path).map_err(|_| error())?;
    let ext = real
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !matches!(ext.as_str(), "html" | "htm") {
        return Err(error());
    }
    let secret = std::fs::canonicalize(secrets_dir).unwrap_or_else(|_| secrets_dir.to_path_buf());
    let state = std::fs::canonicalize(state_dir).unwrap_or_else(|_| state_dir.to_path_buf());
    if real.starts_with(secret)
        || real.parent() == Some(state.as_path())
            && real
                .file_name()
                .is_some_and(|n| n.to_string_lossy().contains(".sqlite"))
        || real.starts_with("/dev/fd")
    {
        return Err(error());
    }
    let file = open_regular_file(&real, true).map_err(|_| error())?;
    let metadata = file.metadata().map_err(|_| error())?;
    if std::fs::canonicalize(path).map_err(|_| error())? != real {
        return Err(error());
    }
    if metadata.len() > MAX_PAGE_BYTES as u64 {
        return Err(Error::operation(
            "HtmlRenderFileTooLargeError",
            format!("{} exceeds 25 MiB.", path.display()),
        ));
    }
    let mut bytes = Vec::new();
    file.take((MAX_PAGE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| error())?;
    if bytes.len() > MAX_PAGE_BYTES {
        return Err(Error::operation(
            "HtmlRenderFileTooLargeError",
            "HTML file grew beyond 25 MiB.",
        ));
    }
    let html = String::from_utf8_lossy(&bytes).into_owned();
    if !rx(r"(?i)^\s*<(?:!doctype\s+html|html)[\s>]").is_match(&html) {
        return Err(error());
    }
    Ok(html)
}

fn open_regular_file(path: &Path, require_single_link: bool) -> std::io::Result<std::fs::File> {
    let invalid = || std::io::Error::other("File changed or is not a regular file");
    let real = std::fs::canonicalize(path)?;
    let before = std::fs::symlink_metadata(&real)?;
    if !before.is_file() {
        return Err(invalid());
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x00200000);
    }
    let file = options.open(&real)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || require_single_link && !singly_linked(&file, &metadata) {
        return Err(invalid());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if before.dev() != metadata.dev() || before.ino() != metadata.ino() {
            return Err(invalid());
        }
    }
    if before.len() != metadata.len() || before.modified()? != metadata.modified()? {
        return Err(invalid());
    }
    let handle = same_file::Handle::from_file(file.try_clone()?)?;
    // Reopen with the same no-follow/nonblocking flags: from_path could block on a swapped FIFO.
    let current_file = options.open(&real)?;
    if !current_file.metadata()?.is_file() {
        return Err(invalid());
    }
    let current = same_file::Handle::from_file(current_file)?;
    if handle != current
        || std::fs::canonicalize(path)? != real
        || !std::fs::symlink_metadata(&real)?.is_file()
    {
        return Err(invalid());
    }
    Ok(file)
}
#[cfg(unix)]
fn singly_linked(_file: &std::fs::File, metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    metadata.nlink() == 1
}
#[cfg(windows)]
fn singly_linked(file: &std::fs::File, _metadata: &std::fs::Metadata) -> bool {
    winapi_util::file::information(file).is_ok_and(|info| info.number_of_links() == 1)
}
#[cfg(not(any(unix, windows)))]
fn singly_linked(_: &std::fs::File, _: &std::fs::Metadata) -> bool {
    false
}

#[cfg(test)]
mod runtime_tests {
    use super::*;
    #[tokio::test]
    async fn agent_html_processing_runs_off_the_async_runtime_thread() {
        let root = tempfile::tempdir().expect("tempdir");
        let service = HtmlService::new(
            HtmlOptions {
                attachments_dir: root.path().join("attachments"),
                state_dir: root.path().join("state"),
                secrets_dir: root.path().join("secrets"),
                recovery: Arc::new(MemoryHtmlRecovery::default()),
            },
            None,
        );
        let runtime_thread = std::thread::current().id();
        let html = format!(
            "<html><head>{}</head><p>content</p></html>",
            "<script>const inert='</style><head>';</script>".repeat(100)
        );
        let prepared = service.prepare(&html).await.expect("prepare");
        assert!(prepared.contains("<p>content</p>"));
        let threads = lock(&service.work_threads);
        assert!(!threads.is_empty());
        assert!(
            threads.iter().all(|thread| *thread != runtime_thread),
            "HTML parsing/inlining ran on the async runtime thread"
        );
    }
    #[tokio::test]
    async fn publication_file_writes_run_off_the_async_runtime_thread() {
        let root = tempfile::tempdir().expect("tempdir");
        let service = HtmlService::new(
            HtmlOptions {
                attachments_dir: root.path().join("attachments"),
                state_dir: root.path().join("state"),
                secrets_dir: root.path().join("secrets"),
                recovery: Arc::new(MemoryHtmlRecovery::default()),
            },
            None,
        );
        let runtime_thread = std::thread::current().id();
        service
            .publish(
                "thread".into(),
                PublishedSource {
                    source: HtmlSource::Html {
                        html: "<p>publication</p>".into(),
                    },
                    title: "Page".into(),
                    height: 200.0,
                },
            )
            .await
            .expect("publication");
        let threads = lock(&service.work_threads);
        assert_eq!(threads.len(), 2);
        assert!(
            threads.iter().all(|thread| *thread != runtime_thread),
            "publication wrote the file on the async runtime thread"
        );
    }
    #[tokio::test]
    async fn cancelled_writer_finishes_before_removing_its_file_and_never_records_recovery() {
        struct Recovery {
            forgotten: tokio::sync::Notify,
            records: std::sync::atomic::AtomicUsize,
        }
        impl HtmlRenderRecovery for Recovery {
            fn record(
                &self,
                _: &ThreadId,
                _: &PublishedSource,
                _: &HtmlRenderReference,
            ) -> Result<()> {
                self.records
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            }
            fn forget(&self, _: &ThreadId, _: &HtmlRenderReference) {
                self.forgotten.notify_one();
            }
        }
        let root = tempfile::tempdir().expect("tempdir");
        let attachments = root.path().join("attachments");
        let recovery = Arc::new(Recovery {
            forgotten: tokio::sync::Notify::new(),
            records: std::sync::atomic::AtomicUsize::new(0),
        });
        let mut service = HtmlService::new(
            HtmlOptions {
                attachments_dir: attachments.clone(),
                state_dir: root.path().join("state"),
                secrets_dir: root.path().join("secrets"),
                recovery: recovery.clone(),
            },
            None,
        );
        let gate = Arc::new(WriteGate::default());
        service.write_gate = Some(gate.clone());
        let publishing = tokio::spawn(async move {
            service
                .publish(
                    "thread".into(),
                    PublishedSource {
                        source: HtmlSource::Html {
                            html: "<p>cancel during write</p>".into(),
                        },
                        title: "Page".into(),
                        height: 200.0,
                    },
                )
                .await
        });
        gate.entered.notified().await;
        assert_eq!(
            std::fs::read_dir(&attachments)
                .expect("attachments")
                .count(),
            1
        );
        publishing.abort();
        assert!(publishing.await.expect_err("cancelled").is_cancelled());
        *lock(&gate.released) = true;
        gate.changed.notify_one();
        recovery.forgotten.notified().await;
        assert_eq!(
            std::fs::read_dir(&attachments)
                .expect("attachments")
                .count(),
            0
        );
        assert_eq!(
            recovery.records.load(std::sync::atomic::Ordering::SeqCst),
            0
        );
    }
}
