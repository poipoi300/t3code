//! Port of filePreviewMode.ts, filePath.ts, the preview flags of FilePreviewPanel.tsx, and the
//! parts of @t3tools/shared (filePreview.ts, mediaPreview.ts, video.ts, delimitedPreview.ts,
//! path.ts) the Files panel calls.
use crate::view_logic::session::links::{is_absolute, is_windows_absolute, workspace_relative_file_path};
use t3_contracts::{
    assets::AssetResource,
    base_schemas::{ThreadId, TrimmedNonEmptyString},
    project::{ProjectFileFailure, ProjectFileOperation, ProjectReadFileError},
};

pub const FILE_TEXT_PREVIEW_MAX_BYTES: u64 = 1024 * 1024;
pub const FILE_EXPLORER_STORAGE_KEY: &str = "t3code.fileExplorerOpen";
pub const RENDER_MARKDOWN_STORAGE_KEY: &str = "t3code.renderMarkdown";
pub const RENDER_BROWSER_FILE_STORAGE_KEY: &str = "t3code.renderBrowserFile";
pub const RENDER_TABLE_STORAGE_KEY: &str = "t3code.renderTable";
pub const RENDER_MERMAID_STORAGE_KEY: &str = "t3code.renderMermaid";
pub const T3_PROJECT_FILE_NAME: &str = "t3.json";

const GENERIC_MIME_TYPES: [&str; 5] = [
    "application/octet-stream",
    "binary/octet-stream",
    "application/unknown",
    "video/mp2t",
    "video/vnd.dlna.mpeg-tts",
];
const IMAGE_EXTENSIONS: [&str; 13] = [
    "avif", "bmp", "gif", "ico", "heic", "heif", "jpeg", "jpg", "png", "svg", "tif", "tiff", "webp",
];
const VIDEO_EXTENSIONS: [&str; 7] = ["avi", "m4v", "mkv", "mov", "mp4", "ogv", "webm"];
const AUDIO_EXTENSIONS: [&str; 11] = [
    "mp3", "wav", "ogg", "oga", "flac", "aac", "m4a", "opus", "aiff", "webm-audio", "weba",
];
const OFFICE_KINDS: [&str; 9] = ["docx", "xlsx", "pptx", "odt", "ods", "odp", "rtf", "epub", "ipynb"];

fn extension(path: &str) -> Option<String> {
    let index = path.rfind('.')?;
    let extension = &path[index + 1..];
    (!extension.is_empty()
        && extension
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-'))
    .then(|| extension.to_ascii_lowercase())
}

/// path.ts `isHostPath`: absolute, or under the home folder.
pub fn is_host_path(value: &str) -> bool {
    is_absolute(value) || value.starts_with("~/")
}

pub fn is_workspace_image_preview_path(path: &str) -> bool {
    extension(path).is_some_and(|e| IMAGE_EXTENSIONS.contains(&e.as_str()))
}
pub fn is_workspace_video_preview_path(path: &str) -> bool {
    extension(path).is_some_and(|e| VIDEO_EXTENSIONS.contains(&e.as_str()))
}
pub fn is_workspace_audio_preview_path(path: &str) -> bool {
    extension(path).is_some_and(|e| AUDIO_EXTENSIONS.contains(&e.as_str()))
}
pub fn is_workspace_font_preview_path(path: &str) -> bool {
    extension(path).is_some_and(|e| matches!(e.as_str(), "ttf" | "otf" | "woff" | "woff2"))
}
/// BrowserDocumentFrame.tsx / openFileInPreview.ts: the path before any `?` or `#`.
fn literal(path: &str) -> &str {
    path.split(['?', '#']).next().unwrap_or(path)
}
pub fn is_pdf_preview_file(path: &str) -> bool {
    literal(path).to_ascii_lowercase().ends_with(".pdf")
}
pub fn is_browser_preview_file(path: &str) -> bool {
    let path = literal(path).to_ascii_lowercase();
    path.ends_with(".html") || path.ends_with(".htm") || path.ends_with(".pdf")
}
pub fn is_markdown_preview_file(path: &str) -> bool {
    extension(path).is_some_and(|e| matches!(e.as_str(), "md" | "mdx" | "markdown" | "mdown" | "mkd"))
}
pub fn is_mermaid_preview_file(path: &str) -> bool {
    extension(path).is_some_and(|e| matches!(e.as_str(), "mmd" | "mermaid"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryKind {
    Archive,
    Sqlite,
}
/// filePreview.ts `binaryPreviewKind` for a workspace path (no MIME type).
pub fn binary_preview_kind(path: &str) -> Option<BinaryKind> {
    match extension(path)?.as_str() {
        "sqlite" | "sqlite3" | "db" => Some(BinaryKind::Sqlite),
        "zip" | "jar" | "tar" | "gz" | "tgz" => Some(BinaryKind::Archive),
        _ => None,
    }
}
/// filePreview.ts `officePreviewKind` for a workspace path: the kind's extension.
pub fn office_preview_kind(path: &str) -> Option<&'static str> {
    let lower = path.to_lowercase();
    OFFICE_KINDS
        .into_iter()
        .find(|kind| lower.ends_with(&format!(".{kind}")))
}
/// delimitedPreview.ts `filePreviewDelimiter` for a workspace path.
pub fn file_preview_delimiter(path: &str) -> Option<char> {
    match extension(path)?.as_str() {
        "csv" => Some(','),
        "tsv" => Some('\t'),
        _ => None,
    }
}
pub fn is_generic_mime(mime: &str) -> bool {
    mime.is_empty() || GENERIC_MIME_TYPES.contains(&mime) || mime == "text/plain"
}

/// filePreviewMode.ts `resolveFilePreviewPath`: the workspace root itself shows the tree alone.
pub fn resolve_file_preview_path(path: Option<&str>, cwd: &str) -> Option<String> {
    let path = path?;
    (path != "." && workspace_relative_file_path(path, Some(cwd)).as_deref() != Some("."))
        .then(|| path.to_owned())
}

/// filePreviewMode.ts `filePreviewReadErrorMessage`: client English for known failures (keys),
/// else the server's own message (content).
pub fn file_preview_read_error_message(error: &ProjectReadFileError) -> (String, bool) {
    let owned = |text: &str| (text.to_owned(), true);
    match error.failure.clone().flatten() {
        Some(ProjectFileFailure::PathNotFile) => {
            owned("The path is a directory or special file, not a regular file.")
        }
        Some(ProjectFileFailure::BinaryFile) => {
            owned("The file is binary and cannot be displayed as text.")
        }
        Some(ProjectFileFailure::WorkspacePathOutsideRoot) => {
            owned("The requested path is outside the workspace.")
        }
        Some(ProjectFileFailure::ResolvedPathOutsideRoot) => {
            owned("The path resolves to a location outside the workspace.")
        }
        Some(ProjectFileFailure::OperationFailed) => {
            if error.operation.clone().flatten() == Some(ProjectFileOperation::RealpathWorkspaceRoot) {
                owned("The workspace folder could not be accessed.")
            } else {
                owned("The file could not be accessed or read. It may be missing or inaccessible.")
            }
        }
        None => (error.message.0.clone(), false),
    }
}

/// filePreviewMode.ts `shouldShowFileExplorer`.
pub fn should_show_file_explorer(relative_path: Option<&str>, explorer_open: bool, attachment_open: bool) -> bool {
    if attachment_open || relative_path.is_some_and(is_host_path) {
        return false;
    }
    explorer_open || relative_path.is_none()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetKind {
    WorkspaceFile,
    MediaFile,
}
/// filePreviewMode.ts `workspaceAssetResource`: the server signs `workspace-file` assets only
/// below the workspace root; anything else is the host `media-file` resource.
pub fn workspace_asset_resource(
    kind: AssetKind,
    thread: &ThreadId,
    draft: bool,
    workspace_root: &str,
    absolute_path: &str,
) -> AssetResource {
    let path = TrimmedNonEmptyString(absolute_path.to_owned());
    if draft {
        return AssetResource::DraftWorkspaceFile {
            cwd: TrimmedNonEmptyString(workspace_root.to_owned()),
            path,
        };
    }
    let inside = workspace_relative_file_path(absolute_path, Some(workspace_root))
        .is_some_and(|relative| relative != ".");
    if inside && kind == AssetKind::WorkspaceFile {
        AssetResource::WorkspaceFile {
            thread_id: thread.clone(),
            path,
        }
    } else {
        AssetResource::MediaFile {
            thread_id: thread.clone(),
            path,
            disposition: None,
            host_only: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrumbKind {
    Project,
    Directory,
    File,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileBreadcrumb {
    pub label: String,
    pub path: String,
    pub kind: CrumbKind,
}

/// filePath.ts `fileBreadcrumbs`.
pub fn file_breadcrumbs(project_name: &str, relative_path: &str) -> Vec<FileBreadcrumb> {
    let host = is_host_path(relative_path);
    let windows = is_windows_absolute(relative_path);
    let separator = if windows { "\\" } else { "/" };
    let parts: Vec<&str> = if windows {
        relative_path.split(['\\', '/']).filter(|p| !p.is_empty()).collect()
    } else {
        relative_path.split('/').filter(|p| !p.is_empty()).collect()
    };
    let root = if relative_path.starts_with("\\\\") {
        "\\\\"
    } else if relative_path.starts_with('/') {
        "/"
    } else {
        ""
    };
    let mut crumbs = Vec::new();
    if !host {
        crumbs.push(FileBreadcrumb {
            label: project_name.to_owned(),
            path: String::new(),
            kind: CrumbKind::Project,
        });
    }
    for (index, part) in parts.iter().enumerate() {
        let path = format!("{root}{}", parts[..=index].join(separator));
        let bare_drive = path.len() == 2 && path.as_bytes()[0].is_ascii_alphabetic() && path.ends_with(':');
        let path = if bare_drive {
            format!("{path}\\")
        } else if host && path == "~" {
            "~/".to_owned()
        } else {
            path
        };
        crumbs.push(FileBreadcrumb {
            label: (*part).to_owned(),
            path,
            kind: if index == parts.len() - 1 {
                CrumbKind::File
            } else {
                CrumbKind::Directory
            },
        });
    }
    crumbs
}

/// filePath.ts `breadcrumbPathOf`.
pub fn breadcrumb_path_of(relative_path: &str) -> String {
    file_breadcrumbs("", relative_path)
        .pop()
        .map_or_else(|| relative_path.to_owned(), |crumb| crumb.path)
}

fn trail_crumb_index(trail: &str, path: &str) -> Option<usize> {
    let crumb = breadcrumb_path_of(path);
    file_breadcrumbs("", trail).iter().position(|c| c.path == crumb)
}

/// filePath.ts `retainedBreadcrumbTrail`: navigating up keeps the descendant crumbs.
pub fn retained_breadcrumb_trail(trail: Option<&str>, path: &str) -> String {
    match trail {
        Some(trail) if trail_crumb_index(trail, path).is_some() => trail.to_owned(),
        _ => path.to_owned(),
    }
}

/// filePath.ts `trailChildOf`.
pub fn trail_child_of(folder: &str, trail: &str) -> Option<String> {
    let index = trail_crumb_index(trail, folder)?;
    file_breadcrumbs("", trail).get(index + 1).map(|c| c.label.clone())
}

/// filePath.ts `joinHostPath`.
pub fn join_host_path(folder: &str, relative_path: &str) -> String {
    let separator = if is_windows_absolute(folder) { "\\" } else { "/" };
    let base = if folder.ends_with(separator) {
        folder.to_owned()
    } else {
        format!("{folder}{separator}")
    };
    base + &relative_path.split('/').collect::<Vec<_>>().join(separator)
}

/// FilePreviewPanel's `clampFileLine`.
pub fn clamp_file_line(contents: &str, requested: i64) -> i64 {
    let lines = contents.split('\n').count().max(1) as i64;
    requested.clamp(1, lines)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderedMode {
    Mermaid,
    Markdown,
    Table,
    Html,
}
impl RenderedMode {
    /// FilePreviewPanel's `renderedToggleLabel` (translation keys).
    pub fn toggle_label(self, rendered: bool) -> &'static str {
        match (self, rendered) {
            (Self::Mermaid, true) => "Show Mermaid source",
            (Self::Mermaid, false) => "Show rendered diagram",
            (Self::Markdown, true) => "Show markdown source",
            (Self::Markdown, false) => "Show rendered markdown",
            (Self::Table, true) => "Show source",
            (Self::Table, false) => "Show table",
            (Self::Html, true) => "Show HTML source",
            (Self::Html, false) => "Show rendered page",
        }
    }
}

/// What a path is, as FilePreviewPanel classifies it before reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PathKinds {
    pub video: bool,
    pub audio: bool,
    pub image: bool,
    pub pdf: bool,
    pub html: bool,
    pub document: bool,
    pub markdown: bool,
    pub mermaid: bool,
    pub delimiter: Option<char>,
}
impl PathKinds {
    pub fn of(path: &str, attachment: bool) -> Self {
        let video = is_workspace_video_preview_path(path);
        let pdf = is_pdf_preview_file(path);
        Self {
            video,
            audio: !video && is_workspace_audio_preview_path(path),
            image: !video && is_workspace_image_preview_path(path),
            pdf,
            html: !pdf && is_browser_preview_file(path),
            document: binary_preview_kind(path).is_some()
                || office_preview_kind(path).is_some()
                || is_workspace_font_preview_path(path),
            markdown: is_markdown_preview_file(path),
            mermaid: is_mermaid_preview_file(path),
            delimiter: (!attachment).then(|| file_preview_delimiter(path)).flatten(),
        }
    }
    pub fn media(self) -> bool {
        self.image || self.video || self.audio
    }
    /// The rendered/source toggle's mode, in FilePreviewPanel's precedence.
    pub fn rendered_mode(self) -> Option<RenderedMode> {
        if self.mermaid {
            Some(RenderedMode::Mermaid)
        } else if self.markdown {
            Some(RenderedMode::Markdown)
        } else if self.delimiter.is_some() {
            Some(RenderedMode::Table)
        } else if self.html {
            Some(RenderedMode::Html)
        } else {
            None
        }
    }
}

/// markdownFrontmatter.ts `maskMarkdownFrontmatter`: a leading YAML block becomes spaces, line
/// breaks kept, so rendered previews skip it and source offsets stay valid.
pub fn mask_markdown_frontmatter(markdown: &str) -> std::borrow::Cow<'_, str> {
    let body = markdown.strip_prefix('\u{feff}').unwrap_or(markdown);
    let offset = markdown.len() - body.len();
    let line_end = |text: &str, from: usize| -> Option<(usize, usize)> {
        let rest = &text[from..];
        let index = rest.find(['\r', '\n'])?;
        let width = if rest[index..].starts_with("\r\n") { 2 } else { 1 };
        Some((from + index, from + index + width))
    };
    let Some((first_end, mut cursor)) = line_end(body, 0) else {
        return markdown.into();
    };
    let marker = |line: &str, open: bool| {
        (line.starts_with("---") || (!open && line.starts_with("...")))
            && line[3..].chars().all(|c| c == ' ' || c == '\t')
    };
    if !marker(&body[..first_end], true) {
        return markdown.into();
    }
    loop {
        let (end, next) = line_end(body, cursor).unwrap_or((body.len(), body.len()));
        if marker(&body[cursor..end], false) {
            let block_end = offset + end;
            let masked: String = markdown[..block_end]
                .chars()
                .map(|c| if c == '\r' || c == '\n' { c } else { ' ' })
                .collect();
            return (masked + &markdown[block_end..]).into();
        }
        if next >= body.len() {
            return markdown.into();
        }
        cursor = next;
    }
}

/// The highlighter language of a file: its extension, or the grammar its well-known name has.
pub fn file_highlight_language(path: &str) -> String {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let lower = name.to_ascii_lowercase();
    if lower == "dockerfile" || lower.starts_with("dockerfile.") {
        return "Dockerfile".into();
    }
    if matches!(lower.as_str(), "makefile" | "gnumakefile") {
        return "Makefile".into();
    }
    extension(name).unwrap_or_else(|| "text".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontmatter_is_masked_with_its_line_breaks() {
        assert_eq!(mask_markdown_frontmatter("---\na: 1\n---\n# T"), "   \n    \n   \n# T");
        assert_eq!(mask_markdown_frontmatter("---\r\n...\r\nx"), "   \r\n   \r\nx");
        assert_eq!(mask_markdown_frontmatter("# no\n---\n"), "# no\n---\n");
        assert_eq!(mask_markdown_frontmatter("---\nunclosed"), "---\nunclosed");
        assert_eq!(file_highlight_language("src/main.RS"), "rs");
        assert_eq!(file_highlight_language("docker/Dockerfile"), "Dockerfile");
        assert_eq!(file_highlight_language("LICENSE"), "text");
    }

    fn read_error(failure: Option<ProjectFileFailure>, operation: Option<ProjectFileOperation>) -> ProjectReadFileError {
        serde_json::from_value(serde_json::json!({
            "_tag": "ProjectReadFileError",
            "message": "Server says no",
            "failure": failure,
            "operation": operation,
        }))
        .unwrap()
    }

    #[test]
    fn breadcrumbs_follow_file_path_ts() {
        let crumbs = file_breadcrumbs("Proj", "src/lib/a.ts");
        assert_eq!(
            crumbs.iter().map(|c| (c.label.as_str(), c.path.as_str(), c.kind)).collect::<Vec<_>>(),
            vec![
                ("Proj", "", CrumbKind::Project),
                ("src", "src", CrumbKind::Directory),
                ("lib", "src/lib", CrumbKind::Directory),
                ("a.ts", "src/lib/a.ts", CrumbKind::File),
            ]
        );
        let host = file_breadcrumbs("Proj", "C:\\Users\\a.txt");
        assert_eq!(host[0].path, "C:\\", "a bare drive means its root");
        assert_eq!(host.last().unwrap().path, "C:\\Users\\a.txt");
        assert_eq!(file_breadcrumbs("P", "~/notes")[0].path, "~/");
        assert_eq!(file_breadcrumbs("P", "/etc/hosts")[0].path, "/etc");
        assert_eq!(breadcrumb_path_of("src//lib/"), "src/lib");
    }

    #[test]
    fn trails_are_retained_only_when_navigating_up() {
        assert_eq!(retained_breadcrumb_trail(Some("a/b/c.ts"), "a/b"), "a/b/c.ts");
        assert_eq!(retained_breadcrumb_trail(Some("a/b/c.ts"), "x"), "x");
        assert_eq!(retained_breadcrumb_trail(None, "a"), "a");
        assert_eq!(trail_child_of("a", "a/b/c.ts").as_deref(), Some("b"));
        assert_eq!(trail_child_of("z", "a/b"), None);
        assert_eq!(join_host_path("C:\\x", "a/b"), "C:\\x\\a\\b");
        assert_eq!(join_host_path("/x/", "a/b"), "/x/a/b");
    }

    #[test]
    fn read_errors_say_what_failed() {
        assert_eq!(
            file_preview_read_error_message(&read_error(Some(ProjectFileFailure::BinaryFile), None)),
            ("The file is binary and cannot be displayed as text.".into(), true)
        );
        assert_eq!(
            file_preview_read_error_message(&read_error(
                Some(ProjectFileFailure::OperationFailed),
                Some(ProjectFileOperation::RealpathWorkspaceRoot)
            ))
            .0,
            "The workspace folder could not be accessed."
        );
        assert_eq!(
            file_preview_read_error_message(&read_error(None, None)),
            ("Server says no".into(), false),
            "an unknown failure is the server's own words, as content"
        );
    }

    #[test]
    fn preview_paths_and_explorer_follow_file_preview_mode_ts() {
        assert_eq!(resolve_file_preview_path(Some("."), "/repo"), None);
        assert_eq!(resolve_file_preview_path(Some("/repo"), "/repo"), None);
        assert_eq!(resolve_file_preview_path(Some("a.ts"), "/repo").as_deref(), Some("a.ts"));
        assert!(should_show_file_explorer(None, false, false));
        assert!(!should_show_file_explorer(Some("/etc/hosts"), true, false));
        assert!(!should_show_file_explorer(Some("a.ts"), true, true));
        assert!(!should_show_file_explorer(Some("a.ts"), false, false));
    }

    #[test]
    fn asset_resources_stay_inside_the_workspace() {
        let thread = ThreadId("t".into());
        assert!(matches!(
            workspace_asset_resource(AssetKind::WorkspaceFile, &thread, false, "/repo", "/repo/a.png"),
            AssetResource::WorkspaceFile { .. }
        ));
        assert!(matches!(
            workspace_asset_resource(AssetKind::WorkspaceFile, &thread, false, "/repo", "/elsewhere/a.png"),
            AssetResource::MediaFile { .. }
        ));
        assert!(matches!(
            workspace_asset_resource(AssetKind::WorkspaceFile, &thread, true, "/repo", "/repo/a.png"),
            AssetResource::DraftWorkspaceFile { .. }
        ));
    }

    #[test]
    fn path_kinds_follow_the_panel_precedence() {
        let kinds = PathKinds::of("clip.webm", false);
        assert!(kinds.video && !kinds.audio && !kinds.image);
        assert_eq!(PathKinds::of("a.md", false).rendered_mode(), Some(RenderedMode::Markdown));
        assert_eq!(PathKinds::of("a.csv", false).rendered_mode(), Some(RenderedMode::Table));
        assert_eq!(PathKinds::of("a.csv", true).rendered_mode(), None);
        assert_eq!(PathKinds::of("a.mmd", false).rendered_mode(), Some(RenderedMode::Mermaid));
        assert_eq!(PathKinds::of("x.pdf?x=1", false).rendered_mode(), None);
        assert!(PathKinds::of("x.pdf", false).pdf);
        assert!(PathKinds::of("book.xlsx", false).document);
        assert!(PathKinds::of("db.sqlite", false).document);
        assert_eq!(binary_preview_kind("a.tgz"), Some(BinaryKind::Archive));
        assert_eq!(office_preview_kind("Notes.IPYNB"), Some("ipynb"));
        assert_eq!(clamp_file_line("a\nb\nc", 10), 3);
        assert_eq!(clamp_file_line("a", -2), 1);
    }
}
