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

fn no_credentials(url: &url::Url) -> bool {
    url.username().is_empty() && url.password().is_none()
}

/// "Use T3 Code in another app" launches exactly `grokbot://app/v1/prompt?text=…`.
fn app_launch(url: &url::Url) -> bool {
    url.scheme() == "grokbot"
        && url.host_str() == Some("app")
        && url.path() == "/v1/prompt"
        && no_credentials(url)
        && url.fragment().is_none_or(str::is_empty)
        && url.query_pairs().map(|(key, _)| key).collect::<Vec<_>>() == ["text"]
}

fn remote_editor(url: &url::Url) -> bool {
    if !REMOTE_EDITOR_SCHEMES.contains(&url.scheme()) || !no_credentials(url) {
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
        && url.host_str() == Some("file")
        && no_credentials(url)
        && url.path().len() > 1
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
        ("not a url", false, false),
    ];

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
