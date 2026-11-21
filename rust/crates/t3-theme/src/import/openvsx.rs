//! Open VSX theme catalogue: search, detail parsing, and the checked import of a
//! theme extension. A port of `apps/web/src/openVsxThemes.ts` with the network
//! behind [`OpenVsxHttp`].
//!
//! The limits below are the import policy as data; implementations of the HTTP
//! trait are responsible for enforcing [`SEARCH_REQUEST_TIMEOUT_MS`] and for
//! stopping a download once it exceeds the cap it was asked for.

use std::future::Future;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use url::Url;

use super::jsonc::parse_jsonc;
use super::vscode::{
    is_vscode_theme_file, pair_vscode_themes, parse_vscode_theme_file,
    resolve_theme_label_collisions,
};
use super::zip::{MAX_THEME_BYTES, ZipArchive};
use crate::palette::ThemeDefinition;

pub const OPEN_VSX_SEARCH_URL: &str = "https://open-vsx.org/api/-/search";
pub const MAX_VSIX_BYTES: u64 = 20 * 1024 * 1024;
pub const MAX_SEARCH_BYTES: u64 = 512 * 1024;
pub const MAX_DETAIL_BYTES: u64 = 256 * 1024;
pub const MAX_MANIFEST_BYTES: u64 = 256 * 1024;
pub const SEARCH_REQUEST_TIMEOUT_MS: u64 = 10_000;
pub const MAX_THEMES_PER_EXTENSION: usize = 40;
pub const MAX_INCLUDE_DEPTH: usize = 8;
pub const MAX_PACKAGE_PATH_LENGTH: usize = 1_024;
pub const MAX_COLOR_VALUE_LENGTH: usize = 128;
pub const MAX_RESOLVED_THEME_FILES: usize = MAX_THEMES_PER_EXTENSION * MAX_INCLUDE_DEPTH;
/// Search shows at most this many extensions, from at most this many detail lookups.
pub const SEARCH_DETAIL_LOOKUPS: usize = 16;
pub const SEARCH_RESULT_LIMIT: usize = 8;
/// SPDX identifiers whose themes may be imported.
pub const SUPPORTED_LICENSES: [&str; 9] = [
    "0BSD",
    "Apache-2.0",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "CC0-1.0",
    "ISC",
    "MIT",
    "MPL-2.0",
    "Unlicense",
];
/// The only workbench colours the importer reads; everything else is dropped on load.
pub const USED_WORKBENCH_COLORS: [&str; 46] = [
    "activityBar.background",
    "activityBarBadge.background",
    "badge.background",
    "button.background",
    "button.foreground",
    "contrastBorder",
    "descriptionForeground",
    "disabledForeground",
    "dropdown.background",
    "dropdown.border",
    "editor.background",
    "editor.foreground",
    "editor.findMatchBackground",
    "editor.findMatchForeground",
    "editor.findMatchHighlightBackground",
    "editor.findMatchHighlightForeground",
    "editor.selectionBackground",
    "editorCursor.foreground",
    "editorError.foreground",
    "editorGroup.border",
    "editorPane.background",
    "editorWarning.foreground",
    "editorWidget.background",
    "errorForeground",
    "focusBorder",
    "foreground",
    "input.border",
    "input.placeholderForeground",
    "list.activeSelectionBackground",
    "list.hoverBackground",
    "list.inactiveSelectionBackground",
    "menu.background",
    "panel.background",
    "panel.border",
    "progressBar.background",
    "quickInput.background",
    "scrollbarSlider.background",
    "sideBar.background",
    "sideBar.border",
    "sideBar.foreground",
    "terminal.background",
    "terminal.foreground",
    "terminal.selectionBackground",
    "terminalCursor.foreground",
    "textCodeBlock.background",
    "textLink.foreground",
];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OpenVsxError {
    #[error("{0}")]
    Message(String),
    /// The caller cancelled the operation (`AbortError` in the web client).
    #[error("The operation was aborted.")]
    Aborted,
}

fn fail<T>(message: &str) -> Result<T, OpenVsxError> {
    Err(OpenVsxError::Message(message.to_owned()))
}

impl From<crate::palette::ThemeError> for OpenVsxError {
    fn from(value: crate::palette::ThemeError) -> Self {
        Self::Message(value.0)
    }
}

impl From<super::zip::PackageError> for OpenVsxError {
    fn from(value: super::zip::PackageError) -> Self {
        Self::Message(value.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OpenVsxThemeSort {
    #[default]
    DownloadCount,
    Rating,
    Timestamp,
    Relevance,
}

impl OpenVsxThemeSort {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DownloadCount => "downloadCount",
            Self::Rating => "rating",
            Self::Timestamp => "timestamp",
            Self::Relevance => "relevance",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenVsxThemeExtension {
    pub id: String,
    pub collection_id: String,
    pub name: String,
    pub publisher: String,
    pub description: String,
    pub download_count: f64,
    pub icon_url: Option<String>,
    pub source_url: Option<String>,
    pub manifest_url: String,
    pub sha256_url: String,
    pub vsix_url: String,
    pub version: String,
    pub license: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Head,
}

#[derive(Clone, Debug, Default)]
pub struct HttpResponse {
    pub status: u16,
    /// The `content-length` header, when the server sent one.
    pub content_length: Option<u64>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// A transport failure (no response at all).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpError(pub String);

/// The network seam. Implementations enforce timeouts and may stop reading a body
/// once it is clearly over the caller's limit (the caller re-checks the length).
pub trait OpenVsxHttp {
    /// One budget spans a candidate's detail request and both of its file probes.
    fn with_detail_timeout<T>(
        &self,
        work: impl Future<Output = Result<T, OpenVsxError>>,
    ) -> impl Future<Output = Result<T, OpenVsxError>> {
        work
    }

    fn request(
        &self,
        method: HttpMethod,
        url: &str,
    ) -> impl Future<Output = Result<HttpResponse, HttpError>>;
}

/// Cooperative cancellation, checked where the web client checks its `AbortSignal`.
pub trait Cancel {
    fn is_cancelled(&self) -> bool;
}

/// Never cancelled.
pub struct NoCancel;

impl Cancel for NoCancel {
    fn is_cancelled(&self) -> bool {
        false
    }
}

fn throw_if_cancelled(cancel: &impl Cancel) -> Result<(), OpenVsxError> {
    if cancel.is_cancelled() {
        Err(OpenVsxError::Aborted)
    } else {
        Ok(())
    }
}

fn short_hash(value: &str) -> String {
    Sha256::digest(value.as_bytes())[..6]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn open_vsx_theme_id(extension_id: &str, source: &str) -> String {
    format!(
        "ovx-theme-{}",
        short_hash(&format!("{extension_id}:{source}"))
    )
}

fn open_vsx_collection_id(extension_id: &str) -> String {
    let normalized = format!("open-vsx:{}", extension_id.to_lowercase());
    let valid = normalized.len() <= 129 && {
        let mut chars = normalized.chars();
        chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
            && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | ':' | '-'))
    };
    if valid {
        normalized
    } else {
        format!("open-vsx:{}", short_hash(extension_id))
    }
}

/// Only the public registry and its file CDN may receive theme requests.
pub fn open_vsx_url(raw: &str) -> Option<Url> {
    let url = Url::parse(raw).ok()?;
    (url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && matches!(
            url.host_str(),
            Some("open-vsx.org" | "openvsx.eclipsecontent.org")
        ))
    .then_some(url)
}

fn trusted_open_vsx_url(value: Option<&Value>) -> Option<String> {
    open_vsx_url(value?.as_str()?).map(Into::into)
}

fn public_source_url(value: Option<&Value>) -> Option<String> {
    let raw = match value? {
        Value::String(s) => s.as_str(),
        Value::Object(o) => o.get("url")?.as_str()?,
        _ => return None,
    };
    let url = Url::parse(raw).ok()?;
    (url.scheme() == "https" && url.username().is_empty() && url.password().is_none())
        .then(|| url.to_string())
}

fn theme_contributions(manifest: &Map<String, Value>) -> Vec<&Map<String, Value>> {
    manifest
        .get("contributes")
        .and_then(Value::as_object)
        .and_then(|c| c.get("themes"))
        .and_then(Value::as_array)
        .map(|themes| themes.iter().filter_map(Value::as_object).collect())
        .unwrap_or_default()
}

fn manifest_license_matches(manifest: &Map<String, Value>, license: &str) -> bool {
    manifest
        .get("license")
        .and_then(Value::as_str)
        .is_some_and(|l| l.trim().to_lowercase() == license.to_lowercase())
}

fn trimmed_string(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_str)
        .map(|s| s.trim().to_owned())
        .unwrap_or_default()
}

/// `extensionFromDetail`: `None` for an unsupported licence, an error for a malformed record.
pub fn extension_from_detail(value: &Value) -> Result<Option<OpenVsxThemeExtension>, OpenVsxError> {
    let malformed = || OpenVsxError::Message("Open VSX returned malformed theme details.".into());
    let object = value.as_object().ok_or_else(malformed)?;
    let files = object
        .get("files")
        .and_then(Value::as_object)
        .ok_or_else(malformed)?;
    let namespace = trimmed_string(object.get("namespace"));
    let extension_name = trimmed_string(object.get("name"));
    let display_name = {
        let display = trimmed_string(object.get("displayName"));
        if display.is_empty() {
            extension_name.clone()
        } else {
            display
        }
    };
    let version = trimmed_string(object.get("version"));
    let license = trimmed_string(object.get("license"));
    let manifest_url = trusted_open_vsx_url(files.get("manifest"));
    let sha256_url = trusted_open_vsx_url(files.get("sha256"));
    let vsix_url = trusted_open_vsx_url(files.get("download"));
    let (Some(manifest_url), Some(sha256_url), Some(vsix_url)) =
        (manifest_url, sha256_url, vsix_url)
    else {
        return Err(malformed());
    };
    if namespace.is_empty() || extension_name.is_empty() || version.is_empty() {
        return Err(malformed());
    }
    if !SUPPORTED_LICENSES.contains(&license.as_str()) {
        return Ok(None);
    }
    let id = format!("{namespace}.{extension_name}");
    Ok(Some(OpenVsxThemeExtension {
        collection_id: open_vsx_collection_id(&id),
        id,
        name: display_name,
        publisher: namespace,
        description: object
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        download_count: object
            .get("downloadCount")
            .and_then(Value::as_f64)
            .filter(|n| n.is_finite())
            .unwrap_or(0.0),
        icon_url: trusted_open_vsx_url(files.get("icon")),
        source_url: public_source_url(object.get("repository"))
            .or_else(|| public_source_url(object.get("homepage")))
            .or_else(|| public_source_url(object.get("url"))),
        manifest_url,
        sha256_url,
        vsix_url,
        version,
        license,
    }))
}

/// The search request for `query`, or `None` when the query is blank.
pub fn search_url(query: &str, sort: OpenVsxThemeSort) -> Option<String> {
    let text = query.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
    if text.is_empty() {
        return None;
    }
    let mut url = Url::parse(OPEN_VSX_SEARCH_URL).ok()?;
    url.query_pairs_mut()
        .append_pair("query", text)
        .append_pair("category", "Themes")
        .append_pair("sortBy", sort.as_str())
        .append_pair("sortOrder", "desc")
        .append_pair("size", &SEARCH_DETAIL_LOOKUPS.to_string());
    Some(url.to_string())
}

fn read_capped(
    response: &HttpResponse,
    limit: u64,
    too_large: &str,
) -> Result<Vec<u8>, OpenVsxError> {
    if response.content_length.is_some_and(|length| length > limit)
        || response.body.len() as u64 > limit
    {
        return fail(too_large);
    }
    Ok(response.body.clone())
}

fn parse_jsonc_object(source: &str, description: &str) -> Result<Map<String, Value>, OpenVsxError> {
    match parse_jsonc(source) {
        Ok(Value::Object(object)) => Ok(object),
        _ => Err(OpenVsxError::Message(format!(
            "{description} is not valid JSON."
        ))),
    }
}

fn sanitize_theme_object(value: &Map<String, Value>) -> Map<String, Value> {
    let mut colors = Map::new();
    if let Some(source) = value.get("colors").and_then(Value::as_object) {
        for (key, color) in source {
            if USED_WORKBENCH_COLORS.contains(&key.as_str())
                && let Some(text) = color.as_str()
                && text.encode_utf16().count() <= MAX_COLOR_VALUE_LENGTH
            {
                colors.insert(key.clone(), color.clone());
            }
        }
    }
    let mut out = Map::new();
    if let Some(include) = value.get("include").and_then(Value::as_str) {
        out.insert("include".into(), json!(include));
    }
    out.insert("colors".into(), Value::Object(colors));
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathError(pub String);

/// `normalizePackagePath`: a safe path inside the package, relative to `relative_to`.
pub fn normalize_package_path(path: &str, relative_to: &str) -> Result<String, PathError> {
    let absolute_drive =
        path.len() >= 2 && path.as_bytes()[0].is_ascii_alphabetic() && path.as_bytes()[1] == b':';
    if path.encode_utf16().count() > MAX_PACKAGE_PATH_LENGTH
        || path.contains('\0')
        || path.starts_with('/')
        || absolute_drive
    {
        return Err(PathError(
            "Theme path is not a safe relative package path.".into(),
        ));
    }
    let normalized = path.replace('\\', "/");
    let mut segments: Vec<&str> = {
        let mut base: Vec<&str> = relative_to.split('/').collect();
        base.pop();
        base
    };
    for segment in normalized.split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." {
            if segments.len() <= 1 {
                return Err(PathError(
                    "Theme path escapes the extension package.".into(),
                ));
            }
            segments.pop();
            continue;
        }
        segments.push(segment);
    }
    if segments.first() != Some(&"extension") {
        segments.insert(0, "extension");
    }
    Ok(segments.join("/"))
}

impl From<PathError> for OpenVsxError {
    fn from(value: PathError) -> Self {
        Self::Message(value.0)
    }
}

fn contribution_type(ui_theme: Option<&Value>) -> Option<&'static str> {
    match ui_theme.and_then(Value::as_str)? {
        "vs" => Some("light"),
        "vs-dark" => Some("dark"),
        "hc-black" => Some("hc-black"),
        "hc-light" => Some("hc-light"),
        _ => None,
    }
}

struct IncludeState {
    cache: Vec<(String, Map<String, Value>)>,
    files: usize,
}

fn load_theme_object(
    zip: &ZipArchive,
    path: &str,
    state: &mut IncludeState,
    ancestors: &[String],
    cancel: &impl Cancel,
) -> Result<Map<String, Value>, OpenVsxError> {
    throw_if_cancelled(cancel)?;
    if ancestors.len() >= MAX_INCLUDE_DEPTH {
        return fail("Theme includes are nested too deeply.");
    }
    if ancestors.iter().any(|a| a == path) {
        return fail("Theme includes contain a cycle.");
    }
    if let Some((_, cached)) = state.cache.iter().find(|(p, _)| p == path) {
        return Ok(cached.clone());
    }
    state.files += 1;
    if state.files > MAX_RESOLVED_THEME_FILES {
        return fail("That extension references too many theme files.");
    }
    let value = sanitize_theme_object(&parse_jsonc_object(&zip.read_text(path, path)?, path)?);
    let Some(include) = value.get("include").and_then(Value::as_str) else {
        state.cache.push((path.to_owned(), value.clone()));
        return Ok(value);
    };
    let include_path = normalize_package_path(include, path)?;
    let mut next_ancestors = ancestors.to_vec();
    next_ancestors.push(path.to_owned());
    let base = load_theme_object(zip, &include_path, state, &next_ancestors, cancel)?;
    let mut colors = base
        .get("colors")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(own) = value.get("colors").and_then(Value::as_object) {
        for (key, color) in own {
            colors.insert(key.clone(), color.clone());
        }
    }
    let mut resolved = base;
    for (key, item) in &value {
        resolved.insert(key.clone(), item.clone());
    }
    resolved.insert("colors".into(), Value::Object(colors));
    state.cache.push((path.to_owned(), resolved.clone()));
    Ok(resolved)
}

/// A client for Open VSX over a caller-supplied transport.
pub struct OpenVsxClient<H> {
    http: H,
}

impl<H: OpenVsxHttp> OpenVsxClient<H> {
    pub fn new(http: H) -> Self {
        Self { http }
    }

    async fn get(&self, url: &str) -> Result<HttpResponse, HttpError> {
        self.http.request(HttpMethod::Get, url).await
    }

    /// `searchOpenVsxThemes`.
    pub async fn search(
        &self,
        query: &str,
        sort: OpenVsxThemeSort,
        cancel: &impl Cancel,
    ) -> Result<Vec<OpenVsxThemeExtension>, OpenVsxError> {
        let Some(url) = search_url(query, sort) else {
            return Ok(Vec::new());
        };
        let response = self
            .get(&url)
            .await
            .map_err(|e| OpenVsxError::Message(e.0))?;
        if !response.ok() {
            return fail("Open VSX search is unavailable right now.");
        }
        let bytes = read_capped(
            &response,
            MAX_SEARCH_BYTES,
            "Open VSX returned an unexpectedly large response.",
        )?;
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| {
            OpenVsxError::Message("Open VSX returned an unreadable response.".into())
        })?;
        let Some(extensions) = value.get("extensions").and_then(Value::as_array) else {
            return fail("Open VSX returned an unreadable search response.");
        };
        let candidates = extensions
            .iter()
            .filter_map(|candidate| {
                let namespace = candidate.get("namespace")?.as_str()?;
                let name = candidate.get("name")?.as_str()?;
                (!namespace.is_empty() && !name.is_empty()).then_some((namespace, name))
            })
            .take(SEARCH_DETAIL_LOOKUPS)
            .map(|(namespace, name)| self.http.with_detail_timeout(self.detail(namespace, name)));
        let settled = futures_util::future::join_all(candidates).await;
        throw_if_cancelled(cancel)?;
        if !settled.is_empty() && settled.iter().all(Result::is_err) {
            return fail("Open VSX theme details are unavailable right now.");
        }
        let mut results: Vec<_> = settled
            .into_iter()
            .filter_map(Result::ok)
            .flatten()
            .collect();
        results.truncate(SEARCH_RESULT_LIMIT);
        Ok(results)
    }

    async fn detail(
        &self,
        namespace: &str,
        name: &str,
    ) -> Result<Option<OpenVsxThemeExtension>, OpenVsxError> {
        let encode = |part: &str| {
            url::form_urlencoded::byte_serialize(part.as_bytes())
                .collect::<String>()
                .replace('+', "%20")
        };
        let detail_url = format!(
            "https://open-vsx.org/api/{}/{}",
            encode(namespace),
            encode(name)
        );
        let response = self
            .get(&detail_url)
            .await
            .map_err(|e| OpenVsxError::Message(e.0))?;
        if !response.ok() {
            return fail("Open VSX theme details are unavailable.");
        }
        let bytes = read_capped(
            &response,
            MAX_DETAIL_BYTES,
            "Open VSX returned an unexpectedly large detail response.",
        )?;
        let unreadable =
            || OpenVsxError::Message("Open VSX returned unreadable theme details.".into());
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| unreadable())?;
        let Some(extension) = extension_from_detail(&value).map_err(|_| unreadable())? else {
            return Ok(None);
        };
        let (manifest_response, package_response) = futures_util::try_join!(
            self.get(&extension.manifest_url),
            self.http.request(HttpMethod::Head, &extension.vsix_url),
        )
        .map_err(|_| unreadable())?;
        if !manifest_response.ok() {
            return Err(unreadable());
        }
        if !package_response.ok() {
            return Ok(None);
        }
        if package_response
            .content_length
            .is_some_and(|length| length > MAX_VSIX_BYTES)
        {
            return Ok(None);
        }
        let manifest_bytes = read_capped(
            &manifest_response,
            MAX_MANIFEST_BYTES,
            "Open VSX returned an unexpectedly large manifest.",
        )
        .map_err(|_| unreadable())?;
        let manifest = parse_jsonc_object(
            &String::from_utf8_lossy(&manifest_bytes),
            "Extension manifest",
        )
        .map_err(|_| unreadable())?;
        Ok((!theme_contributions(&manifest).is_empty()
            && manifest_license_matches(&manifest, &extension.license))
        .then_some(extension))
    }

    /// `importOpenVsxThemeExtension`: downloads, verifies and converts every theme in the package.
    pub async fn import(
        &self,
        extension: &OpenVsxThemeExtension,
        cancel: &impl Cancel,
    ) -> Result<Vec<ThemeDefinition>, OpenVsxError> {
        let manifest_response = self
            .get(&extension.manifest_url)
            .await
            .map_err(|e| OpenVsxError::Message(e.0))?;
        if !manifest_response.ok() {
            return fail("That Open VSX extension has no readable manifest.");
        }
        let manifest_bytes = read_capped(
            &manifest_response,
            MAX_MANIFEST_BYTES,
            "That Open VSX extension manifest is too large.",
        )?;
        let manifest = parse_jsonc_object(
            &String::from_utf8_lossy(&manifest_bytes),
            "Extension manifest",
        )?;
        let advertised = theme_contributions(&manifest).len();
        if advertised == 0 {
            return fail("That extension does not contain color themes.");
        }
        if advertised > MAX_THEMES_PER_EXTENSION {
            return fail("That extension contains too many color themes to import safely.");
        }

        let package_response = self
            .get(&extension.vsix_url)
            .await
            .map_err(|e| OpenVsxError::Message(e.0))?;
        if !package_response.ok() {
            return fail("That Open VSX theme could not be downloaded.");
        }
        let package_bytes = read_capped(
            &package_response,
            MAX_VSIX_BYTES,
            "That theme extension is too large to import safely.",
        )?;
        throw_if_cancelled(cancel)?;
        let checksum_response = self
            .get(&extension.sha256_url)
            .await
            .map_err(|e| OpenVsxError::Message(e.0))?;
        if !checksum_response.ok() {
            return fail("That Open VSX theme has no readable checksum.");
        }
        let checksum_bytes = read_capped(
            &checksum_response,
            256,
            "That Open VSX checksum response is invalid.",
        )?;
        let checksum_text = String::from_utf8_lossy(&checksum_bytes);
        let expected = checksum_text
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_owned();
        if expected.len() != 64 || !expected.bytes().all(|b| b.is_ascii_hexdigit()) {
            return fail("That Open VSX theme has an invalid checksum.");
        }
        throw_if_cancelled(cancel)?;
        if hex(&Sha256::digest(&package_bytes)).to_lowercase() != expected.to_lowercase() {
            return fail("That Open VSX theme failed its integrity check.");
        }
        throw_if_cancelled(cancel)?;
        let zip = match ZipArchive::open(&package_bytes) {
            Ok(zip) => zip,
            Err(error)
                if error.0.starts_with("That extension package")
                    || error.0.starts_with("Theme path") =>
            {
                return Err(error.into());
            }
            Err(_) => return fail("That Open VSX extension package could not be opened."),
        };
        throw_if_cancelled(cancel)?;

        let packaged = parse_jsonc_object(
            &zip.read_text("extension/package.json", "Extension manifest")?,
            "Extension manifest",
        )?;
        let publisher = packaged.get("publisher").and_then(Value::as_str);
        let name = packaged.get("name").and_then(Value::as_str);
        let matches_extension = publisher
            .is_some_and(|p| p.to_lowercase() == extension.publisher.to_lowercase())
            && match (publisher, name) {
                (Some(p), Some(n)) => {
                    format!("{p}.{n}").to_lowercase() == extension.id.to_lowercase()
                }
                _ => false,
            }
            && packaged.get("version").and_then(Value::as_str) == Some(extension.version.as_str());
        if !matches_extension {
            return fail("That extension package does not match the selected Open VSX theme.");
        }
        if !manifest_license_matches(&packaged, &extension.license) {
            return fail("That extension package does not match its advertised license.");
        }
        let contributions = theme_contributions(&packaged);
        if contributions.is_empty() {
            return fail("That extension does not contain color themes.");
        }
        if contributions.len() > MAX_THEMES_PER_EXTENSION {
            return fail("That extension contains too many color themes to import safely.");
        }

        struct Parsed {
            theme: ThemeDefinition,
            source_name: String,
            source_path: String,
        }
        let mut parsed: Vec<Parsed> = Vec::new();
        let mut failures = 0usize;
        let mut state = IncludeState {
            cache: Vec::new(),
            files: 0,
        };
        for contribution in contributions {
            throw_if_cancelled(cancel)?;
            let Some(path) = contribution.get("path").and_then(Value::as_str) else {
                failures += 1;
                continue;
            };
            let attempt = (|| -> Result<Parsed, OpenVsxError> {
                let path = normalize_package_path(path, "extension/")?;
                let mut theme_value = load_theme_object(&zip, &path, &mut state, &[], cancel)?;
                let kind = contribution_type(contribution.get("uiTheme"));
                let label = match contribution
                    .get("label")
                    .and_then(Value::as_str)
                    .map(str::trim)
                {
                    Some(label) if !label.is_empty() => label.to_owned(),
                    _ => extension.name.clone(),
                };
                theme_value.insert("displayName".into(), json!(label));
                if let Some(kind) = kind {
                    theme_value.insert("type".into(), json!(kind));
                }
                let decorated = Value::Object(theme_value);
                if !is_vscode_theme_file(&decorated) {
                    return fail("not a VS Code color theme");
                }
                Ok(Parsed {
                    theme: parse_vscode_theme_file(&decorated)?,
                    source_name: path.rsplit('/').next().unwrap_or_default().to_owned(),
                    source_path: path,
                })
            })();
            match attempt {
                Ok(item) => parsed.push(item),
                Err(OpenVsxError::Aborted) => return Err(OpenVsxError::Aborted),
                Err(_) => failures += 1,
            }
        }
        if failures > 0 {
            return fail(
                "One or more color themes in that extension could not be imported safely.",
            );
        }
        if parsed.is_empty() {
            return fail("That extension has no compatible color themes.");
        }
        let extension_id = extension.id.to_lowercase();
        let mut path_counts: Vec<(&str, usize)> = Vec::new();
        for item in &parsed {
            match path_counts.iter_mut().find(|(p, _)| *p == item.source_path) {
                Some(entry) => entry.1 += 1,
                None => path_counts.push((&item.source_path, 1)),
            }
        }
        let mut occurrences: Vec<(&str, usize)> = Vec::new();
        let identities: Vec<String> = parsed
            .iter()
            .map(|item| {
                let total = path_counts
                    .iter()
                    .find(|(p, _)| *p == item.source_path)
                    .map_or(1, |(_, n)| *n);
                if total == 1 {
                    return item.source_path.clone();
                }
                let occurrence = match occurrences.iter_mut().find(|(p, _)| *p == item.source_path)
                {
                    Some(entry) => {
                        let current = entry.1;
                        entry.1 += 1;
                        current
                    }
                    None => {
                        occurrences.push((&item.source_path, 1));
                        0
                    }
                };
                if occurrence == 0 {
                    item.source_path.clone()
                } else {
                    format!("{}\0{occurrence}", item.source_path)
                }
            })
            .collect();
        let entries: Vec<(ThemeDefinition, Option<&str>)> = parsed
            .iter()
            .map(|item| (item.theme.clone(), Some(item.source_name.as_str())))
            .collect();
        let resolved: Vec<ThemeDefinition> = resolve_theme_label_collisions(&entries)
            .into_iter()
            .zip(&identities)
            .map(|(theme, identity)| ThemeDefinition {
                id: open_vsx_theme_id(&extension_id, identity),
                ..theme
            })
            .collect();
        let make_id = |light: &ThemeDefinition, dark: &ThemeDefinition| {
            let mut ids = [light.id.clone(), dark.id.clone()];
            ids.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            open_vsx_theme_id(&extension_id, &ids.join(":"))
        };
        let paired = pair_vscode_themes(&resolved, Some(&make_id));
        let entries: Vec<(ThemeDefinition, Option<&str>)> =
            paired.into_iter().map(|theme| (theme, None)).collect();
        let themes = resolve_theme_label_collisions(&entries);
        let collection = crate::palette::ThemeCollection {
            id: extension.collection_id.clone(),
            label: extension.name.chars().take(48).collect(),
        };
        Ok(themes
            .into_iter()
            .map(|theme| ThemeDefinition {
                collection: Some(collection.clone()),
                ..theme
            })
            .collect())
    }
}

/// The largest manifest or theme file the importer reads, for transport implementations.
pub const fn max_theme_file_bytes() -> usize {
    MAX_THEME_BYTES
}
