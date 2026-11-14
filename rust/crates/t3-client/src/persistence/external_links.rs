//! External opener policy; provider-auth decoding applies its own wire limits.
pub fn is_web_url(address: &str) -> bool {
    url::Url::parse(address).is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
}

/// Which links a surface hands to the OS, as each TypeScript client decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalUrlSurface {
    /// `apps/desktop/src/electron/ElectronShell.ts` `parseSafeExternalUrl`: web links, the exact
    /// app-launch link shape, remote editor deep links and local editor file links.
    Desktop,
    /// `apps/web/src/browser/webUrl.ts` `isWebUrl`: web links only.
    Web,
    /// The mobile app opens only web links (each call site checks `isWebUrl`).
    Mobile,
}

/// Remote-capable editors' schemes (`packages/contracts/src/editor.ts` `remoteSchemeOf`).
const REMOTE_EDITOR_SCHEMES: &[&str] = &[
    "cursor",
    "vscode",
    "vscode-insiders",
    "vscodium",
    "zed",
    "jetbrains",
];
/// `goto` editors with a remote scheme also open local files (`LOCAL_FILE_LINK_SCHEMES`).
const LOCAL_FILE_LINK_SCHEMES: &[&str] = &["cursor", "vscode", "vscode-insiders", "vscodium"];

/// The one opener policy: the address to hand to the OS, or `None` to refuse it. The client's
/// `Platform::open_url` and every host apply it for their surface.
pub fn safe_external_url(raw: &str, surface: ExternalUrlSurface) -> Option<String> {
    let url = url::Url::parse(raw).ok()?;
    let allowed = matches!(url.scheme(), "http" | "https")
        || surface == ExternalUrlSurface::Desktop
            && (app_launch(&url) || remote_editor(&url) || local_editor_file(&url));
    allowed.then(|| url.into())
}

/// A path an editor would read as UNC or a device (`//server`, `\\server`), checked after
/// percent-decoding because the editor decodes it. On Windows it is opened over SMB and can
/// send the user's NTLM credentials. Hardening beyond TypeScript, which accepts these.
fn unc_or_device_path(url: &url::Url) -> bool {
    let path = percent_encoding::percent_decode_str(url.path()).decode_utf8_lossy();
    path.contains('\\') || path.get(1..).is_some_and(|rest| rest.starts_with('/'))
}

/// The editor-link opener, apart from [`safe_external_url`] so that accepting editor links on
/// the web never widens what an ordinary link may open. TypeScript's web client hands the remote
/// editor links it builds (`buildRemoteOpenUrl`) to `location.assign`, outside `isWebUrl`; the
/// desktop also opens local editor file links. A UNC or device path is refused first.
pub fn safe_editor_link(raw: &str, surface: ExternalUrlSurface) -> Option<String> {
    let url = url::Url::parse(raw).ok()?;
    if !REMOTE_EDITOR_SCHEMES.contains(&url.scheme()) || unc_or_device_path(&url) {
        return None;
    }
    let allowed = match surface {
        ExternalUrlSurface::Desktop => remote_editor(&url) || local_editor_file(&url),
        ExternalUrlSurface::Web => remote_editor(&url),
        // The mobile app has no editor to hand a link to.
        ExternalUrlSurface::Mobile => false,
    };
    allowed.then(|| url.into())
}

fn no_credentials(url: &url::Url) -> bool {
    url.username().is_empty() && url.password().is_none()
}

/// TypeScript compares `url.host`, which keeps a port: `cursor://file:8080/x` is not `file`.
fn host_is(url: &url::Url, name: &str) -> bool {
    url.port().is_none() && url.host_str() == Some(name)
}

/// "Use T3 Code in another app" launches exactly `grokbot://app/v1/prompt?text=…`.
fn app_launch(url: &url::Url) -> bool {
    url.scheme() == "grokbot"
        && host_is(url, "app")
        && url.path() == "/v1/prompt"
        && no_credentials(url)
        && url.fragment().is_none_or(str::is_empty)
        && url.query_pairs().map(|(key, _)| key).collect::<Vec<_>>() == ["text"]
}

fn remote_editor(url: &url::Url) -> bool {
    if !REMOTE_EDITOR_SCHEMES.contains(&url.scheme()) || !no_credentials(url) {
        return false;
    }
    if url.port().is_some() {
        return false;
    }
    let (host, path) = (url.host_str().unwrap_or_default(), url.path());
    match url.scheme() {
        // Zed embeds [user@]host in the path; no passwords or ports there.
        "zed" => {
            host == "ssh"
                && path.strip_prefix('/').is_some_and(|rest| {
                    rest.split_once('/').is_some_and(|(address, _)| {
                        let machine = address.split_once('@').map_or(address, |(user, machine)| {
                            if user.is_empty() || user.contains([':', '/']) {
                                ""
                            } else {
                                machine
                            }
                        });
                        !machine.is_empty() && !machine.contains(['@', ':', '/'])
                    })
                })
        }
        "jetbrains" => {
            host == "gateway"
                && path == "/ssh/environment"
                && url
                    .query_pairs()
                    .any(|(key, value)| key == "h" && !value.is_empty())
        }
        _ => {
            host == "vscode-remote"
                && path
                    .strip_prefix("/ssh-remote+")
                    .is_some_and(|rest| !rest.is_empty())
        }
    }
}

fn local_editor_file(url: &url::Url) -> bool {
    LOCAL_FILE_LINK_SCHEMES.contains(&url.scheme())
        && host_is(url, "file")
        && no_credentials(url)
        && url.path().len() > 1
        && !unc_or_device_path(url)
}

#[cfg(test)]
mod tests {
    use super::{ExternalUrlSurface::*, safe_external_url};

    /// The cases the platform's own test replays through each host, so the two cannot drift.
    pub const CASES: &[(&str, bool, bool)] = &[
        // (address, desktop allows, web and mobile allow)
        ("https://example.com/a?b#c", true, true),
        ("HTTP://Example.com", true, true),
        ("mailto:someone@example.com", false, false),
        ("file:///etc/passwd", false, false),
        ("javascript:alert(1)", false, false),
        ("grokbot://app/v1/prompt?text=hi", true, false),
        ("grokbot://app/v1/prompt?text=hi&x=1", false, false),
        ("grokbot://app/v1/other?text=hi", false, false),
        ("vscode://vscode-remote/ssh-remote+box/home/me", true, false),
        ("vscode://vscode-remote/ssh-remote+", false, false),
        ("zed://ssh/me@box/home/me", true, false),
        ("zed://ssh/me:pw@box/home", false, false),
        ("jetbrains://gateway/ssh/environment?h=box", true, false),
        ("jetbrains://gateway/ssh/environment?h=", false, false),
        ("cursor://file/home/me/a.rs:3:4", true, false),
        ("zed://file/home/me/a.rs", false, false),
        ("vscode://user:pw@file/home/me/a.rs", false, false),
        ("cursor://file:8080/home/me/a.rs", false, false),
        ("grokbot://app:1/v1/prompt?text=hi", false, false),
        (
            "vscode://vscode-remote:22/ssh-remote+box/home/me",
            false,
            false,
        ),
        ("jetbrains://gateway:1/ssh/environment?h=box", false, false),
        ("vscode://file//server/share/a.rs", false, false),
        ("vscode://file/%5C%5Cserver/share/a.rs", false, false),
        ("vscode://file/\\\\server\\share\\a.rs", false, false),
        ("vscode://file/C:/Users/me/a.rs:3", true, false),
        ("not a url", false, false),
    ];

    #[test]
    fn editor_links_open_only_editor_links_and_never_a_unc_path() {
        use super::safe_editor_link;
        for (address, desktop, web) in [
            ("vscode://vscode-remote/ssh-remote+box/home/me", true, true),
            ("zed://ssh/me@box/home/me", true, true),
            ("jetbrains://gateway/ssh/environment?h=box", true, true),
            ("cursor://file/home/me/a.rs:3", true, false),
            // Not editor links: the ordinary opener's business, never this one's.
            ("https://example.com/", false, false),
            ("javascript:alert(1)", false, false),
            ("file:///etc/passwd", false, false),
            ("grokbot://app/v1/prompt?text=hi", false, false),
            // UNC and device paths, plain or percent-encoded, local or remote.
            ("vscode://file//server/share/a.rs", false, false),
            ("vscode://file/%5C%5Cserver/share", false, false),
            ("vscode://file/%2F%2Fserver/share", false, false),
            ("vscode://vscode-remote//server/share", false, false),
            ("vscode://vscode-remote/ssh-remote+box/a\\b", false, false),
            ("vscode://user:pw@vscode-remote/ssh-remote+box/home", false, false),
            ("vscode://vscode-remote:22/ssh-remote+box/home", false, false),
        ] {
            assert_eq!(safe_editor_link(address, Desktop).is_some(), desktop, "{address}");
            assert_eq!(safe_editor_link(address, Web).is_some(), web, "{address}");
            assert!(safe_editor_link(address, Mobile).is_none(), "{address}");
        }
    }

    #[test]
    fn each_surface_opens_only_what_its_typescript_client_opens() {
        for (address, desktop, web) in CASES {
            assert_eq!(
                safe_external_url(address, Desktop).is_some(),
                *desktop,
                "{address}"
            );
            assert_eq!(safe_external_url(address, Web).is_some(), *web, "{address}");
            assert_eq!(
                safe_external_url(address, Mobile).is_some(),
                *web,
                "{address}"
            );
        }
    }
}
