//! Host file links use the destination environment's paths, never this machine's filesystem.
use crate::view_logic::JsTrim;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilePathPosition {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u64>,
}
pub fn is_windows_absolute(path: &str) -> bool {
    let b = path.as_bytes();
    path.starts_with("\\\\")
        || (b.len() >= 3
            && b[0].is_ascii_alphabetic()
            && b[1] == b':'
            && matches!(b[2], b'/' | b'\\'))
}
pub fn is_absolute(path: &str) -> bool {
    path.starts_with('/') || is_windows_absolute(path)
}
fn strip_drive(path: &str) -> &str {
    if path.starts_with('/') && is_windows_absolute(&path[1..]) {
        &path[1..]
    } else {
        path
    }
}
fn basename(path: &str) -> &str {
    let p = path.trim_end_matches(['/', '\\']);
    p.rsplit(['/', '\\'])
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(path)
}
pub fn split_file_path_position(path: &str, hash: &str) -> FilePathPosition {
    fn digits(s: &str) -> Option<u64> {
        (!s.is_empty() && s.bytes().all(|c| c.is_ascii_digit()))
            .then(|| s.parse().ok())
            .flatten()
    }
    fn position(s: &str) -> Option<(u64, Option<u64>, Option<u64>)> {
        if let Some((line, column)) = s.split_once(':') {
            return Some((digits(line)?, Some(digits(column)?), None));
        }
        if let Some((line, end)) = s.split_once('-') {
            return Some((
                digits(line)?,
                None,
                Some(digits(end.trim_start_matches(['L', 'l']))?),
            ));
        }
        Some((digits(s)?, None, None))
    }
    let mut suffix = None;
    if let Some((head, tail)) = path.rsplit_once(':') {
        if let Some((prefix, previous)) = head.rsplit_once(':')
            && let (Some(line), Some(column)) = (digits(previous), digits(tail))
        {
            suffix = Some((prefix, line, Some(column), None));
        }
        if suffix.is_none()
            && let Some((line, col, end)) = position(tail)
        {
            suffix = Some((head, line, col, end));
        }
    }
    let result = suffix.or_else(|| {
        hash.strip_prefix('#')
            .and_then(|h| h.strip_prefix(['L', 'l']))
            .and_then(|h| {
                let h = h.replace(['C', 'c'], ":");
                position(&h).map(|(l, c, e)| (path, l, c, e))
            })
    });
    result.map_or_else(
        || FilePathPosition {
            path: path.into(),
            ..Default::default()
        },
        |(path, line, col, end)| FilePathPosition {
            path: path.into(),
            line: (line > 0).then_some(line),
            column: col.filter(|c| *c > 0),
            end_line: end.filter(|e| line > 0 && *e > line),
        },
    )
}
pub fn format_file_path_position(p: &FilePathPosition) -> String {
    p.line.map_or_else(
        || p.path.clone(),
        |line| {
            format!(
                "{}:{line}{}",
                p.path,
                p.column.map_or_else(String::new, |c| format!(":{c}"))
            )
        },
    )
}
pub fn safe_decode_uri_component(s: &str) -> String {
    let mut bytes = Vec::with_capacity(s.len());
    let mut i = 0;
    let raw = s.as_bytes();
    while i < raw.len() {
        if raw[i] == b'%' {
            if i + 2 >= raw.len() {
                return s.into();
            }
            let hex = |b: u8| (b as char).to_digit(16).map(|v| v as u8);
            let (Some(a), Some(b)) = (hex(raw[i + 1]), hex(raw[i + 2])) else {
                return s.into();
            };
            bytes.push(a * 16 + b);
            i += 3;
        } else {
            bytes.push(raw[i]);
            i += 1;
        }
    }
    String::from_utf8(bytes).unwrap_or_else(|_| s.into())
}
pub fn parse_file_url_href(href: &str) -> Option<(String, String)> {
    let url = url::Url::parse(href).ok()?;
    if url.scheme() != "file" {
        return None;
    }
    let host = url
        .host_str()
        .filter(|h| !h.eq_ignore_ascii_case("localhost"))
        .unwrap_or_default();
    let path = if host.is_empty() {
        strip_drive(url.path()).into()
    } else {
        format!("\\\\{host}{}", url.path().replace('/', "\\"))
    };
    Some((
        path,
        url.fragment().map_or_else(String::new, |h| format!("#{h}")),
    ))
}
pub fn resolve_path_link_target(raw: &str, cwd: &str) -> String {
    let mut p = split_file_path_position(raw, "");
    let windows = is_windows_absolute(cwd);
    let join = |base: &str, next: &str| {
        if windows {
            format!(
                "{}\\{}",
                base.trim_end_matches(['/', '\\']),
                next.replace('/', "\\")
            )
        } else {
            format!(
                "{}/{}",
                base.trim_end_matches(['/', '\\']),
                next.trim_start_matches('/')
            )
        }
    };
    if p.path.starts_with("~/") || (windows && p.path.starts_with("~\\")) {
        let normalized = cwd.replace('\\', "/");
        let parts: Vec<_> = normalized.split('/').collect();
        let home = if normalized.starts_with("/Users/") || normalized.starts_with("/home/") {
            Some(parts[..parts.len().min(3)].join("/"))
        } else if windows
            && parts
                .get(1)
                .is_some_and(|p| p.eq_ignore_ascii_case("Users"))
        {
            Some(parts[..parts.len().min(3)].join("\\"))
        } else {
            None
        };
        if let Some(home) = home {
            p.path = join(&home, &p.path[2..]);
        }
    } else if !is_absolute(&p.path) {
        p.path = join(cwd, &p.path);
    }
    format_file_path_position(&p)
}
pub fn workspace_relative_file_path(path: &str, cwd: Option<&str>) -> Option<String> {
    let cwd = cwd.filter(|s| !s.is_empty())?;
    let insensitive = is_windows_absolute(strip_drive(cwd))
        || (cwd.starts_with("//") && path.starts_with("\\\\"));
    let (path, cwd) = if insensitive {
        (
            strip_drive(&path.replace('\\', "/")).to_string(),
            strip_drive(&cwd.replace('\\', "/")).to_string(),
        )
    } else {
        (path.into(), cwd.into())
    };
    let root = cwd.trim_end_matches('/');
    let p = if insensitive {
        path.to_lowercase()
    } else {
        path.clone()
    };
    let r = if insensitive {
        root.to_lowercase()
    } else {
        root.into()
    };
    if p.trim_end_matches('/') == r {
        return Some(".".into());
    }
    if !p.starts_with(&format!("{r}/")) {
        return None;
    }
    let relative = path.get(root.len() + 1..)?;
    let mut depth = 0;
    for s in relative.split('/') {
        if s == ".." {
            depth -= 1;
        } else if !s.is_empty() && s != "." {
            depth += 1;
        }
        if depth < 0 {
            return None;
        }
    }
    Some(relative.into())
}
const EXTENSIONLESS: &[&str] = &[
    "Makefile",
    "makefile",
    "GNUmakefile",
    "Dockerfile",
    "Containerfile",
    "Justfile",
    "justfile",
    "Rakefile",
    "Gemfile",
    "Procfile",
    "Brewfile",
    "Caddyfile",
    "Vagrantfile",
    "Jenkinsfile",
    "Podfile",
    "Fastfile",
    "BUILD",
    "WORKSPACE",
    "LICENSE",
    "LICENCE",
    "COPYING",
    "NOTICE",
    "AUTHORS",
    "CONTRIBUTORS",
    "CHANGELOG",
    "README",
    "CODEOWNERS",
];
fn file_extension(path: &str) -> bool {
    basename(path).rsplit_once('.').is_some_and(|(_, ext)| {
        !ext.is_empty()
            && ext
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    })
}
fn elided(path: &str) -> bool {
    path.split(['/', '\\'])
        .any(|s| s == "…" || (s.len() >= 3 && s.bytes().all(|b| b == b'.')))
}
fn hostname(segment: &str, position: bool) -> bool {
    let s = segment.to_lowercase();
    if s.starts_with('.') {
        return false;
    }
    if s == "localhost" {
        return true;
    }
    if s.contains('.')
        && s.split('.')
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
    {
        return true;
    }
    let Some((_, tld)) = s.rsplit_once('.') else {
        return false;
    };
    [
        "com", "net", "org", "io", "dev", "app", "ai", "co", "edu", "gov", "mil", "info", "biz",
        "xyz", "me", "tv", "gg", "chat", "cloud", "site", "online", "tech", "store", "link",
    ]
    .contains(&tld)
        || (!position
            && [
                "uk", "de", "fr", "nl", "se", "no", "fi", "dk", "pl", "ch", "at", "be", "es", "it",
                "pt", "eu", "us", "ca", "au", "nz", "jp", "kr", "cn", "br", "ru", "mx", "ie", "cz",
                "tr", "sg", "hk", "cc",
            ]
            .contains(&tld))
}
fn explicit(path: &str) -> bool {
    path.starts_with("~/")
        || path.starts_with("./")
        || path.starts_with("../")
        || path.starts_with('/')
        || is_windows_absolute(path)
}
pub fn inline_code_file_path_candidate(code: &str) -> Option<String> {
    let code = code.js_trim();
    if code.is_empty()
        || code
            .chars()
            .any(|c| super::super::is_js_whitespace(c) || c == '`')
    {
        return None;
    }
    let candidate = if is_windows_absolute(code) {
        code.into()
    } else {
        code.replace('\\', "/")
    };
    let p = split_file_path_position(&candidate, "");
    let has_position = p.path != candidate;
    if (!has_position && !candidate.contains(['/', '\\'])) || elided(&p.path) {
        return None;
    }
    if !explicit(&candidate) {
        if p.path.ends_with('/')
            || hostname(p.path.split('/').next().unwrap_or_default(), has_position)
        {
            return None;
        }
        let base = basename(&p.path);
        let bytes = base.as_bytes();
        if bytes
            .windows(3)
            .any(|b| b[0].is_ascii_digit() && b[1] == b'.' && b[2].is_ascii_digit())
            && base.rsplit_once('.').is_some_and(|(_, e)| !e.contains('.'))
        {
            return None;
        }
        if !file_extension(base)
            && (!has_position
                || (p.path.contains('/') && !base.contains('.') && !EXTENSIONLESS.contains(&base)))
        {
            return None;
        }
    }
    Some(candidate)
}
pub fn parse_markdown_file_link(href: &str) -> Option<FilePathPosition> {
    let href = href.js_trim();
    let href = href
        .strip_prefix('<')
        .and_then(|h| h.strip_suffix('>'))
        .unwrap_or(href);
    if href.is_empty() || href.starts_with('#') || href.starts_with("//") {
        return None;
    }
    let (raw, hash) = if href
        .get(..5)
        .is_some_and(|p| p.eq_ignore_ascii_case("file:"))
    {
        parse_file_url_href(href)?
    } else {
        let (p, h) = href.split_once('#').map_or((href, ""), |(p, h)| {
            (p, &href[p.len()..p.len() + 1 + h.len()])
        });
        (p.split('?').next().unwrap_or_default().into(), h.into())
    };
    let raw = safe_decode_uri_component(raw.js_trim());
    let path = strip_drive(&raw);
    let hash = safe_decode_uri_component(hash.js_trim());
    if path.is_empty() {
        return None;
    }
    if !is_windows_absolute(path)
        && let Some((scheme, rest)) = path.split_once(':')
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'.' | b'-'))
        && (rest.starts_with("//") || split_file_path_position(path, "").path == path)
    {
        return None;
    }
    let valid = |path: &str| {
        let p = split_file_path_position(path, &hash);
        if elided(&p.path) {
            return None;
        }
        let shape = explicit(&p.path)
            || EXTENSIONLESS.contains(&p.path.as_str())
            || file_extension(&p.path)
            || p.path.contains('/');
        let allowed = p.path.chars().all(|c| {
            c.is_alphanumeric()
                || c.is_alphabetic()
                || matches!(
                    c,
                    '/' | '\\'
                        | '.'
                        | '_'
                        | '$'
                        | '+'
                        | '@'
                        | '('
                        | ')'
                        | '['
                        | ']'
                        | '-'
                        | ' '
                        | ':'
                        | '~'
                )
        });
        (shape && allowed && (!p.path.contains('@') || p.path.contains('/'))).then_some(p)
    };
    valid(path).or_else(|| {
        if !path.starts_with(['/', '\\']) && !is_windows_absolute(path) && path.contains('\\') {
            valid(&path.replace('\\', "/"))
        } else {
            None
        }
    })
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarkdownFileLinkMeta {
    pub file_path: String,
    pub workspace_lookup_path: Option<String>,
    pub target_path: String,
    pub display_path: String,
    pub workspace_relative_path: Option<String>,
    pub basename: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u64>,
}
pub fn resolve_markdown_file_link_meta(
    href: &str,
    cwd: Option<&str>,
    base_dir: Option<&str>,
) -> Option<MarkdownFileLinkMeta> {
    let source = parse_markdown_file_link(href)?;
    let relative = !is_absolute(&source.path);
    let mut position = source.clone();
    position.end_line = None;
    let target = if relative {
        resolve_path_link_target(&format_file_path_position(&source), base_dir?)
    } else {
        if source.path.starts_with("\\\\")
            && workspace_relative_file_path(&source.path, cwd).is_none()
        {
            return None;
        }
        format_file_path_position(&position)
    };
    let p = split_file_path_position(&target, "");
    let display = format_workspace_relative_path(&target, cwd);
    Some(MarkdownFileLinkMeta {
        file_path: p.path.clone(),
        workspace_lookup_path: if relative
            && base_dir
                .is_some_and(|d| workspace_relative_file_path(d, cwd).as_deref() == Some("."))
        {
            Some(source.path)
        } else {
            None
        },
        target_path: target,
        display_path: display,
        workspace_relative_path: workspace_relative_file_path(&p.path, cwd),
        basename: basename(&p.path).into(),
        line: p.line,
        column: p.column,
        end_line: source.end_line,
    })
}
pub fn resolve_inline_code_file_link_meta(
    code: &str,
    cwd: Option<&str>,
    base_dir: Option<&str>,
) -> Option<MarkdownFileLinkMeta> {
    let candidate = inline_code_file_path_candidate(code)?;
    let root = base_dir.is_some_and(|b| workspace_relative_file_path(b, cwd).is_some())
        && !candidate.starts_with(['/', '~'])
        && !candidate.starts_with("./")
        && !candidate.starts_with("../")
        && split_file_path_position(&candidate, "").path.contains('/')
        && !is_windows_absolute(&candidate);
    resolve_markdown_file_link_meta(&candidate, cwd, if root { cwd } else { base_dir })
}
pub use crate::workspace::project_ops::paths::format_workspace_relative_path;
pub fn rewrite_markdown_file_uri_href(href: &str) -> Option<String> {
    let h = href.js_trim();
    let h = h
        .strip_prefix('<')
        .and_then(|s| s.strip_suffix('>'))
        .unwrap_or(h);
    parse_file_url_href(h).map(|(p, h)| format!("{p}{h}"))
}
pub fn is_local_editor_file_href(href: &str) -> bool {
    url::Url::parse(href).ok().is_some_and(|u| {
        matches!(u.scheme(), "cursor" | "vscode" | "vscode-insiders")
            && u.host_str() == Some("file")
            && u.username().is_empty()
            && u.password().is_none()
            && u.port().is_none()
            && u.path().starts_with('/')
            && u.path().len() > 1
    })
}
pub fn should_open_markdown_file_link_in_browser_by_default(path: &str) -> bool {
    path.split(['?', '#'])
        .next()
        .unwrap_or_default()
        .to_lowercase()
        .ends_with(".pdf")
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkdownFileHostPath {
    pub absolute_path: String,
    pub matched: bool,
}
pub async fn resolve_markdown_file_host_path<
    E,
    F: std::future::Future<Output = Result<Option<String>, E>>,
>(
    meta: &MarkdownFileLinkMeta,
    cwd: Option<&str>,
    find_workspace_match: impl FnOnce(String) -> F,
) -> Result<MarkdownFileHostPath, E> {
    if let (Some(lookup), Some(cwd)) = (&meta.workspace_lookup_path, cwd)
        && let Some(found) = find_workspace_match(lookup.clone())
            .await?
            .filter(|s| !s.is_empty())
    {
        return Ok(MarkdownFileHostPath {
            absolute_path: resolve_path_link_target(&found, cwd),
            matched: true,
        });
    }
    Ok(MarkdownFileHostPath {
        absolute_path: meta.file_path.clone(),
        matched: false,
    })
}
pub fn markdown_file_editor_target(
    meta: &MarkdownFileLinkMeta,
    host: &MarkdownFileHostPath,
) -> String {
    if host.matched {
        format_file_path_position(&FilePathPosition {
            path: host.absolute_path.clone(),
            line: meta.line,
            column: meta.column,
            end_line: meta.end_line,
        })
    } else {
        meta.target_path.clone()
    }
}
pub fn markdown_file_copy_paths(
    meta: &MarkdownFileLinkMeta,
    host: &MarkdownFileHostPath,
    cwd: Option<&str>,
) -> (String, String) {
    (
        markdown_file_editor_target(meta, host),
        format_file_path_position(&FilePathPosition {
            path: workspace_relative_file_path(&host.absolute_path, cwd)
                .unwrap_or_else(|| meta.file_path.clone()),
            line: meta.line,
            column: meta.column,
            end_line: meta.end_line,
        }),
    )
}
