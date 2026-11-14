//! Port of packages/shared/src/threadLinks.ts and apps/web/src/components/chat/MarkdownFileLink.logic.ts.

use super::links::{MarkdownFileLinkMeta, should_open_markdown_file_link_in_browser_by_default};
use crate::types::{EnvironmentId, ThreadId, ThreadKey};
use crate::view_logic::JsTrim;

const HOSTED_PREFIX: &str = "https://app.t3.codes/threads/";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreadLink {
    pub environment: Option<EnvironmentId>,
    pub thread: ThreadId,
}

fn id(value: &str) -> Option<String> {
    let value = value.js_trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn decode(value: &str) -> Option<String> {
    let decoded = super::links::safe_decode_uri_component(value);
    // decodeURIComponent rejects malformed escapes, rather than preserving them.
    let bytes = value.as_bytes();
    if bytes.iter().enumerate().any(|(at, byte)| {
        *byte == b'%'
            && !bytes
                .get(at + 1..at + 3)
                .is_some_and(|hex| hex.iter().all(u8::is_ascii_hexdigit))
    }) || (decoded == value && value.contains('%'))
    {
        return None;
    }
    Some(decoded)
}

pub fn parse_thread_link_href(href: &str) -> Option<ThreadLink> {
    if let Some(path) = href.strip_prefix(HOSTED_PREFIX) {
        let (environment, thread) = path.split_once('/')?;
        let segment = |s: &str| {
            !s.is_empty()
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'%' | b'-'))
        };
        if !segment(environment) || !segment(thread) {
            return None;
        }
        return Some(ThreadLink {
            environment: Some(EnvironmentId(id(&decode(environment)?)?)),
            thread: ThreadId(id(&decode(thread)?)?),
        });
    }
    if let Some(thread) = href.strip_prefix("t3-thread://v1/") {
        return Some(ThreadLink {
            environment: None,
            thread: ThreadId(id(thread)?),
        });
    }
    let (environment, thread) = href.strip_prefix("t3-thread://v2/")?.split_once('/')?;
    Some(ThreadLink {
        environment: Some(EnvironmentId(id(environment)?)),
        thread: ThreadId(id(thread)?),
    })
}

pub fn thread_link_candidates(
    link: &ThreadLink,
    message_environment: &EnvironmentId,
) -> Vec<ThreadKey> {
    let environment = link.environment.as_ref().unwrap_or(message_environment);
    let mut candidates = vec![ThreadKey {
        environment: environment.clone(),
        thread: link.thread.clone(),
    }];
    if let Some(decoded) = decode(&link.thread.0).and_then(|value| id(&value))
        && decoded != link.thread.0
    {
        candidates.push(ThreadKey {
            environment: environment.clone(),
            thread: ThreadId(decoded),
        });
    }
    candidates
}

pub fn guess_thread_link_ref(link: &ThreadLink, refs: &[ThreadKey]) -> Option<ThreadKey> {
    if link.environment.is_some() {
        return None;
    }
    for candidate in thread_link_candidates(link, &EnvironmentId(String::new())) {
        let mut matches = refs.iter().filter(|key| key.thread == candidate.thread);
        if let Some(first) = matches.next() {
            return matches.next().is_none().then(|| first.clone());
        }
    }
    None
}

pub fn unknown_thread_link_label(
    link: &ThreadLink,
    message_environment: &EnvironmentId,
) -> &'static str {
    if link
        .environment
        .as_ref()
        .is_none_or(|id| id == message_environment)
    {
        "Thread no longer available"
    } else {
        "Thread on another machine"
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreadLinkResolution<T> {
    pub key: ThreadKey,
    pub metadata: Option<T>,
    pub missing: bool,
}

/// MarkdownThreadLink's selection policy; a failed query does not prove a thread missing.
pub fn resolve_thread_link<T>(
    link: &ThreadLink,
    message_environment: &EnvironmentId,
    reachable: bool,
    settled: bool,
    refs: &[ThreadKey],
    shell: impl Fn(&ThreadKey) -> Option<T>,
    metadata: impl Fn(&ThreadKey) -> Option<T>,
) -> ThreadLinkResolution<T> {
    let candidates = thread_link_candidates(link, message_environment);
    let written = &candidates[0];
    let target = shell(written)
        .map(|value| (written.clone(), value))
        .or_else(|| {
            candidates
                .iter()
                .find_map(|key| metadata(key).map(|value| (key.clone(), value)))
        });
    let missing = target.is_none() && (settled || !reachable);
    let remote = if missing && link.environment.is_none() {
        let remote_refs = refs
            .iter()
            .filter(|key| key.environment != written.environment)
            .cloned()
            .collect::<Vec<_>>();
        guess_thread_link_ref(link, &remote_refs)
            .and_then(|key| shell(&key).map(|value| (key, value)))
    } else {
        None
    };
    match target.or(remote) {
        Some((key, metadata)) => ThreadLinkResolution {
            key,
            metadata: Some(metadata),
            missing: false,
        },
        None => ThreadLinkResolution {
            key: written.clone(),
            metadata: None,
            missing,
        },
    }
}

fn hosted_segment(value: &str) -> String {
    let mut result = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-') {
            result.push(byte as char);
        } else {
            use std::fmt::Write;
            let _ = write!(result, "%{byte:02X}");
        }
    }
    result
}

pub fn format_hosted_thread_link(key: &ThreadKey, label: &str, origin: &str) -> Option<String> {
    let url = url::Url::parse(origin)
        .ok()?
        .join(&format!(
            "/threads/{}/{}",
            hosted_segment(&key.environment.0),
            hosted_segment(&key.thread.0)
        ))
        .ok()?;
    let cleaned = label
        .replace(['[', ']', '\\', '\r', '\n'], " ")
        .split(super::super::is_js_whitespace)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    Some(if cleaned.is_empty() {
        url.into()
    } else {
        format!("[{cleaned}]({url})")
    })
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FileActions {
    pub editor: bool,
    pub browser: bool,
    pub panel: bool,
    pub media: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilePrimaryAction {
    Editor,
    Browser,
    Panel,
    Media,
    Options,
}

pub fn can_use_markdown_file_shell_actions(
    environment: Option<&EnvironmentId>,
    remote_open_mode: &str,
    resolved: bool,
) -> bool {
    environment.is_some() && resolved && remote_open_mode == "local-exec"
}

pub fn markdown_file_primary_action(
    path: &str,
    actions: FileActions,
    editor_modifier: bool,
) -> FilePrimaryAction {
    if actions.editor && editor_modifier {
        return FilePrimaryAction::Editor;
    }
    if actions.browser
        && (should_open_markdown_file_link_in_browser_by_default(path)
            || (!actions.editor && !actions.panel))
    {
        return FilePrimaryAction::Browser;
    }
    if actions.panel {
        FilePrimaryAction::Panel
    } else if actions.media {
        FilePrimaryAction::Media
    } else if actions.editor {
        FilePrimaryAction::Editor
    } else {
        FilePrimaryAction::Options
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileCopyItem {
    pub relative: bool,
    pub label: &'static str,
    pub value: String,
}

pub fn markdown_file_copy_menu_items(meta: &MarkdownFileLinkMeta) -> Vec<FileCopyItem> {
    let mut items = Vec::with_capacity(2);
    if let Some(relative) = &meta.workspace_relative_path {
        let path = super::links::split_file_path_position(&meta.target_path, "").path;
        items.push(FileCopyItem {
            relative: true,
            label: "Copy relative path",
            value: format!("{relative}{}", &meta.target_path[path.len()..]),
        });
    }
    items.push(FileCopyItem {
        relative: false,
        label: "Copy full path",
        value: meta.target_path.clone(),
    });
    items
}

/// Port of shared/fileLinks.ts fileLinkLabel, with a precomputed sibling-path suffix.
pub fn file_link_label(meta: &MarkdownFileLinkMeta, parent_suffix: Option<&str>) -> String {
    let mut parts = vec![meta.basename.clone()];
    if let Some(suffix) = parent_suffix.filter(|value| !value.is_empty()) {
        parts.push(suffix.into());
    }
    if let Some(line) = meta.line {
        parts.push(match (meta.end_line, meta.column) {
            (Some(end), _) => format!("L{line}–{end}"),
            (_, Some(column)) => format!("L{line}:C{column}"),
            _ => format!("L{line}"),
        });
    }
    parts.join(" · ")
}

/// Port of shared/markdownLinks.ts isMarkdownFileLinkLabel.
pub fn is_markdown_file_link_label(label: &str, href: &str) -> bool {
    let Some(destination) = super::links::parse_markdown_file_link(href) else {
        return false;
    };
    let position = super::links::split_file_path_position(label.js_trim(), "");
    if (position.line.is_some() && position.line != destination.line)
        || (position.column.is_some() && position.column != destination.column)
        || (position.end_line.is_some() && position.end_line != destination.end_line)
    {
        return false;
    }
    let normalize = |path: &str| {
        let path = path.replace('\\', "/");
        path.strip_prefix("./")
            .unwrap_or(&path)
            .trim_end_matches('/')
            .to_owned()
    };
    let mut label = normalize(&position.path);
    let mut target = normalize(&destination.path);
    if label.is_empty() {
        return true;
    }
    if super::links::is_windows_absolute(&destination.path) {
        label = label.to_lowercase();
        target = target.to_lowercase();
    }
    target == label || target.ends_with(&format!("/{label}"))
}

/// Port of MarkdownExternalLink.tsx; offsets are UTF-8 boundaries rather than UTF-16.
pub fn leading_external_link_text_length(text: &str) -> usize {
    for prefix in ["https://", "http://"] {
        if text
            .get(..prefix.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
        {
            return prefix.len();
        }
    }
    text.chars().next().map_or(0, char::len_utf8)
}

pub fn external_link_host(href: &str) -> Option<String> {
    let url = url::Url::parse(if href.starts_with("//") {
        return external_link_host(&format!("https:{href}"));
    } else {
        href
    })
    .ok()?;
    matches!(url.scheme(), "http" | "https")
        .then(|| url.host_str().map(str::to_owned))
        .flatten()
}

/// Port of shared/favicon.ts faviconUrlForOrigin and its hostClassification.ts guard.
pub fn favicon_url_for_host(host: &str) -> Option<String> {
    if host.ends_with("..") {
        return None;
    }
    let normalized = host
        .trim_matches(['[', ']'])
        .trim_end_matches('.')
        .to_lowercase();
    if ["localhost", "home.arpa"].contains(&normalized.as_str())
        || [
            ".localhost",
            ".local",
            ".home.arpa",
            ".ts.net",
            ".alt",
            ".corp",
            ".example",
            ".home",
            ".internal",
            ".intranet",
            ".invalid",
            ".lan",
            ".onion",
            ".private",
            ".test",
        ]
        .iter()
        .any(|suffix| normalized == &suffix[1..] || normalized.ends_with(suffix))
        || normalized
            .split('.')
            .any(|part| ["corp", "internal", "intranet"].contains(&part))
    {
        return None;
    }
    let public_v4 = |[a, b, c, _]: [u8; 4]| {
        !(matches!(a, 0 | 10 | 127 | 224..=255)
            || (a == 100 && (64..=127).contains(&b))
            || (a == 172 && (16..=31).contains(&b))
            || (a == 192 && b == 168)
            || (a == 169 && b == 254)
            || (a == 198 && (18..=19).contains(&b))
            || matches!(
                (a, b, c),
                (192, 0, 0) | (192, 0, 2) | (192, 88, 99) | (198, 51, 100) | (203, 0, 113)
            ))
    };
    let parsed = url::Host::parse(if normalized.contains(':') {
        return favicon_ipv6(host, &normalized, public_v4);
    } else {
        &normalized
    })
    .ok()?;
    let public = match parsed {
        url::Host::Ipv4(ip) => public_v4(ip.octets()),
        url::Host::Domain(_) => normalized.contains('.'),
        _ => false,
    };
    public.then(|| favicon_url(host))
}

fn favicon_url(host: &str) -> String {
    let encoded: String = url::form_urlencoded::byte_serialize(host.as_bytes()).collect();
    format!("https://www.google.com/s2/favicons?domain={encoded}&sz=32")
}

fn favicon_ipv6(
    host: &str,
    normalized: &str,
    public_v4: impl Fn([u8; 4]) -> bool,
) -> Option<String> {
    let url::Host::Ipv6(ip) = url::Host::parse(&format!("[{normalized}]")).ok()? else {
        return None;
    };
    let s = ip.segments();
    let public = if let Some(ip) = ip.to_ipv4_mapped() {
        public_v4(ip.octets())
    } else if s[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
        let b = ip.octets();
        public_v4([b[12], b[13], b[14], b[15]])
    } else if s[0] & 0xe000 != 0x2000 {
        false
    } else if s[0] == 0x2001 && s[1] & 0xfe00 == 0 {
        (s[1] == 1 && s[2..7] == [0; 5] && (1..=3).contains(&s[7]))
            || s[1] == 3
            || (s[1] == 4 && s[2] == 0x112)
            || s[1] & 0xfff0 == 0x20
            || s[1] & 0xfff0 == 0x30
    } else {
        !(s[..2] == [0x2001, 0xdb8] || s[0] == 0x2002 || (s[0] == 0x3fff && s[1] & 0xf000 == 0))
    };
    public.then(|| favicon_url(host))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(environment: &str, thread: &str) -> ThreadKey {
        ThreadKey {
            environment: EnvironmentId(environment.into()),
            thread: ThreadId(thread.into()),
        }
    }
    fn link(value: &str) -> ThreadLink {
        parse_thread_link_href(value).expect("thread link")
    }

    #[test]
    fn thread_links_keep_written_ids_try_decoded_second_and_never_guess_v2() {
        let legacy = link("t3-thread://v1/provider%3A1");
        assert_eq!(
            thread_link_candidates(&legacy, &EnvironmentId("local".into())),
            [key("local", "provider%3A1"), key("local", "provider:1")]
        );
        assert_eq!(
            guess_thread_link_ref(&legacy, &[key("remote", "provider:1")]),
            Some(key("remote", "provider:1"))
        );
        assert_eq!(
            guess_thread_link_ref(
                &legacy,
                &[
                    key("a", "provider%3A1"),
                    key("b", "provider%3A1"),
                    key("c", "provider:1")
                ]
            ),
            None
        );
        let message_environment = EnvironmentId("local".into());
        let pending = resolve_thread_link::<&str>(
            &legacy,
            &message_environment,
            true,
            false,
            &[key("remote", "provider:1")],
            |key| (key.environment.0 == "remote").then_some("Remote thread"),
            |_| None,
        );
        assert!(
            !pending.missing,
            "pending/offline query never proves absence"
        );
        assert_eq!(pending.key, key("local", "provider%3A1"));
        assert_eq!(
            pending.metadata, None,
            "a real remote match cannot steal an unsettled link"
        );
        let target = resolve_thread_link(
            &legacy,
            &message_environment,
            true,
            true,
            &[key("remote", "provider:1")],
            |key| (key.environment.0 == "remote").then_some("Remote thread"),
            |_| None,
        );
        assert_eq!(target.key, key("remote", "provider:1"));
        assert_eq!(target.metadata, Some("Remote thread"));
        let explicit = link("t3-thread://v2/local/provider%3A1");
        let missing = resolve_thread_link::<&str>(
            &explicit,
            &message_environment,
            true,
            true,
            &[key("remote", "provider:1")],
            |key| (key.environment.0 == "remote").then_some("Remote thread"),
            |_| None,
        );
        assert!(missing.missing);
        let decoded = resolve_thread_link(
            &explicit,
            &message_environment,
            true,
            true,
            &[],
            |_| None,
            |key| (key.thread.0 == "provider:1").then_some("Decoded"),
        );
        assert_eq!(decoded.key, key("local", "provider:1"));
        let literal = resolve_thread_link(
            &explicit,
            &message_environment,
            true,
            true,
            &[],
            |_| None,
            |key| Some(key.thread.0.clone()),
        );
        assert_eq!(literal.key, key("local", "provider%3A1"));
        assert_eq!(
            guess_thread_link_ref(
                &link("t3-thread://v2/there/provider%3A1"),
                &[key("remote", "provider:1")]
            ),
            None
        );
        assert_eq!(
            link("t3-thread://v2/env-1/thread:a/b%3A1").thread.0,
            "thread:a/b%3A1"
        );
        for href in [
            "t3-thread://v1/",
            "t3-thread://v1/ ",
            "t3-thread://v2//t",
            "t3-thread://v2/e/",
            "t3-thread://v2/e",
            "https://evil.test/threads/e/t",
        ] {
            assert!(parse_thread_link_href(href).is_none(), "{href}");
        }
    }

    #[test]
    fn hosted_links_round_trip_escape_punctuation_and_clean_labels() {
        let target = key("env.1", "thread:delegated/mcp%3A1.");
        let href = "https://app.t3.codes/threads/env%2E1/thread%3Adelegated%2Fmcp%253A1%2E";
        assert_eq!(
            format_hosted_thread_link(&target, " ", "https://app.t3.codes").as_deref(),
            Some(href)
        );
        assert_eq!(
            link(href),
            ThreadLink {
                environment: Some(target.environment),
                thread: target.thread
            }
        );
        assert_eq!(
            format_hosted_thread_link(
                &key("e", "t1"),
                "Fix [ci] \\ build",
                "https://preview.t3.codes/"
            )
            .as_deref(),
            Some("[Fix ci build](https://preview.t3.codes/threads/e/t1)")
        );
        for href in [
            "https://app.t3.codes/threads/e/t?prompt=go",
            "https://app.t3.codes/threads/e/t/extra",
            "https://app.t3.codes/threads/e/%ZZ",
            "https://app.t3.codes/threads/e/%C0%AF",
        ] {
            assert!(parse_thread_link_href(href).is_none(), "{href}");
        }
    }

    #[test]
    fn file_primary_action_preserves_modifiers_pdf_panel_media_and_options_priority() {
        use FilePrimaryAction::*;
        assert!(can_use_markdown_file_shell_actions(
            Some(&EnvironmentId("host".into())),
            "local-exec",
            true
        ));
        assert!(!can_use_markdown_file_shell_actions(
            None,
            "local-exec",
            true
        ));
        assert!(!can_use_markdown_file_shell_actions(
            Some(&EnvironmentId("host".into())),
            "local-exec",
            false
        ));
        assert!(!can_use_markdown_file_shell_actions(
            Some(&EnvironmentId("host".into())),
            "unknown",
            true
        ));
        for (path, actions, modifier, expected) in [
            (
                "x.pdf",
                FileActions {
                    editor: true,
                    browser: true,
                    panel: true,
                    media: true,
                },
                false,
                Browser,
            ),
            (
                "x.pdf",
                FileActions {
                    editor: true,
                    browser: true,
                    panel: true,
                    media: true,
                },
                true,
                Editor,
            ),
            (
                "x.rs",
                FileActions {
                    editor: true,
                    browser: true,
                    panel: true,
                    media: true,
                },
                false,
                Panel,
            ),
            (
                "x.png",
                FileActions {
                    media: true,
                    ..Default::default()
                },
                false,
                Media,
            ),
            ("x.rs", FileActions::default(), true, Options),
            (
                "x.html",
                FileActions {
                    browser: true,
                    ..Default::default()
                },
                false,
                Browser,
            ),
        ] {
            assert_eq!(
                markdown_file_primary_action(path, actions, modifier),
                expected
            );
        }
        let meta = super::super::links::resolve_markdown_file_link_meta(
            "src/main.rs:12:3",
            Some("/repo"),
            Some("/repo"),
        )
        .expect("meta");
        let items = markdown_file_copy_menu_items(&meta);
        assert_eq!(
            items
                .iter()
                .map(|item| item.value.as_str())
                .collect::<Vec<_>>(),
            ["src/main.rs:12:3", "/repo/src/main.rs:12:3"]
        );
        assert_eq!(file_link_label(&meta, None), "main.rs · L12:C3");
        assert!(is_markdown_file_link_label(
            "main.rs:12:3",
            "src/main.rs:12:3"
        ));
        assert!(!is_markdown_file_link_label(
            "See source",
            "src/main.rs:12:3"
        ));
        assert!(!is_markdown_file_link_label(
            "main.rs:11",
            "src/main.rs:12:3"
        ));
    }

    #[test]
    fn external_prefix_does_not_split_a_code_point_or_protocol_and_favicons_stay_public() {
        for (value, prefix) in [
            ("https://example.com", "https://"),
            ("HTTP://x", "HTTP://"),
            ("🦀rust", "🦀"),
            ("éclair", "é"),
            ("", ""),
        ] {
            assert_eq!(&value[..leading_external_link_text_length(value)], prefix);
        }
        for host in [
            "localhost",
            "127.0.0.1",
            "10.0.0.1",
            "100.64.0.1",
            "192.168.1.1",
            "server",
            "box.ts.net",
            "foo.test",
            "corp.example.com",
            "[::1]",
            "[fd7a:115c:a1e0::1]",
            "[::ffff:192.168.1.1]",
            "[2001:db8::1]",
            "[64:ff9b::c0a8:101]",
        ] {
            assert!(favicon_url_for_host(host).is_none(), "{host}");
        }
        assert_eq!(
            favicon_url_for_host("example.com").as_deref(),
            Some("https://www.google.com/s2/favicons?domain=example.com&sz=32")
        );
        assert!(favicon_url_for_host("8.8.8.8").is_some());
        assert!(favicon_url_for_host("[2606:4700:4700::1111]").is_some());
        assert!(external_link_host("javascript:alert(1)").is_none());
    }
}
