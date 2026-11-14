//! The one place the UI opens an address outside the app, and the only module that may hand a
//! runtime value to NSH's opener or to a link's `href` (`links::tests` guards the crate).
//!
//! TypeScript sends every link through `useOpenLink` → the desktop's `parseSafeExternalUrl` or the
//! web's `isWebUrl`. Here the client owns that policy: `ClientRuntime::external_link` (the address
//! a link may carry) and `ClientRuntime::open_external_url` (`Platform::open_url`, which applies
//! `external_links::safe_external_url` for the host's surface). NSH's own opener refuses only a few
//! schemes and would launch any registered protocol handler (`search-ms:`, `ms-msdt:`,
//! `vscode://file/<UNC>`), so no screen reaches it. Editor links go through the client's editor
//! picker (`Platform::open_editor_link`), never through here.

use std::rc::Rc;

use futures::future::LocalBoxFuture;
use nsh_core::prelude::*;
use t3_client::composition::ClientRuntime;
use t3_client::persistence::external_links::{ExternalUrlSurface, safe_external_url};
use t3_client::platform::{Platform, PlatformError};

use crate::runtime::tasks::TaskOwner;

type Check = dyn Fn(&str) -> Option<String>;
type Open = dyn Fn(String) -> LocalBoxFuture<'static, Result<(), PlatformError>>;

/// The client's link policy and opener, provided at the app root
/// (`app::provide_client_services`).
#[derive(Clone)]
pub struct LinkOpener {
    check: Rc<Check>,
    open: Rc<Open>,
    /// Where opens run: the opener's maker, so an open outlives the toast or menu that asked.
    tasks: TaskOwner,
}

pub fn link_opener<P: Platform + Clone + 'static>(client: &ClientRuntime<P>) -> LinkOpener {
    let checking = client.clone();
    let opening = client.clone();
    LinkOpener {
        check: Rc::new(move |raw| checking.external_link(raw)),
        open: Rc::new(move |url| opening.open_external_url(url)),
        tasks: TaskOwner::here(),
    }
}

/// Without a client (the demo, an unavailable host): web links only, as the web client's
/// `isWebUrl`, opened by NSH after that check.
fn web_only() -> LinkOpener {
    LinkOpener {
        check: Rc::new(|raw| safe_external_url(raw, ExternalUrlSurface::Web)),
        open: Rc::new(|url| {
            Box::pin(async move {
                if safe_external_url(&url, ExternalUrlSurface::Web).is_none() {
                    return Err(PlatformError::InvalidData("Unable to open link.".into()));
                }
                if open_url(url).await {
                    Ok(())
                } else {
                    Err(PlatformError::Unsupported("Unable to open link.".into()))
                }
            })
        }),
        tasks: TaskOwner::here(),
    }
}

/// The opener for this component. Read it in a component body and capture it: contexts are not
/// available in event handlers.
pub fn link_opener_here() -> LinkOpener {
    use_context::<LinkOpener>().unwrap_or_else(|| {
        debug_assert!(
            use_context::<crate::runtime::Rt>().is_none_or(|rt| rt.backend().client().is_none()),
            "LinkOpener is not provided: app::provide_client_services provides it with the client"
        );
        web_only()
    })
}

impl LinkOpener {
    /// The address a link may carry for `raw` (as authored: what the link reports and its
    /// hint shows), or `None` when the policy refuses it.
    pub fn href(&self, raw: &str) -> Option<SharedString> {
        (self.check)(raw).map(|_| SharedString::from(raw.to_owned()))
    }

    /// Opens `raw` outside the app if the policy allows it; a refused address opens nothing
    /// (the platform logs it), as `useOpenLink` shows nothing for a refused link.
    pub fn open(&self, raw: impl Into<String>) {
        let raw = raw.into();
        // The string the policy validated (parsed and normalized) is the string opened; the
        // authored text is only the link's `href`, what is shown and copied.
        let Some(url) = (self.check)(&raw) else {
            log::warn!("Refused to open a link outside the app's policy");
            return;
        };
        let open = (self.open)(url);
        self.tasks.spawn(async move {
            if let Err(error) = open.await {
                log::warn!("Could not open the link: {error}");
            }
        });
    }
}

/// A link to an address outside the app (NSH's `Link`, `openWith="app"`): its `href` is the
/// address the policy accepts (none when refused, so the platform cannot open it either: the
/// web's `<a href>`, a terminal's hyperlink), and a press opens it through the policy. `on_open`
/// hears the press as the link's own `onOpen` would.
pub fn external_link(
    raw: impl Fn() -> Option<String> + 'static,
    on_open: impl Fn(&str) + 'static,
) -> nsh_base::LinkElement {
    let opener = link_opener_here();
    let checking = opener.clone();
    nsh_base::link()
        .href(move || raw().and_then(|raw| checking.href(&raw)))
        .open_with(nsh_base::LinkOpenWith::App)
        .on_open(move |href: &SharedString| {
            on_open(href);
            opener.open(href.to_string());
        })
}

/// A link to a place inside the app (a route path, a workspace file the app opens in its own
/// panel): `openWith="app"`, so nothing is opened outside the app; the caller's `on_open` is
/// how it opens.
pub fn in_app_link(href: impl Fn() -> Option<SharedString> + 'static) -> nsh_base::LinkElement {
    nsh_base::link()
        .href(href)
        .open_with(nsh_base::LinkOpenWith::App)
}

#[cfg(test)]
mod tests {
    /// Every opener and every runtime `href` lives in this module: NSH's `open_url`, a `Link`
    /// that opens with the system, a TextView that opens its links, or `.href(` anywhere else
    /// in `src` fails this test. Reads each file up to its trailing `#[cfg(test)]` module.
    #[test]
    fn only_this_module_opens_addresses_or_sets_link_hrefs() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("source directory") {
                let path = entry.expect("entry").path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().is_none_or(|ext| ext != "rs") {
                    continue;
                }
                let relative = path.strip_prefix(&root).expect("under src");
                if relative == std::path::Path::new("runtime/services/links.rs") {
                    continue;
                }
                let source = std::fs::read_to_string(&path).expect("source");
                let source = source.split("#[cfg(test)]").next().unwrap_or_default();
                for (number, line) in source.lines().enumerate() {
                    let code = line.split("//").next().unwrap_or_default();
                    // `.href(` with an argument (a getter such as `location.href()` is fine).
                    let sets_href = code
                        .match_indices(".href(")
                        .any(|(at, _)| !code[at + ".href(".len()..].starts_with(')'));
                    // NSH's opener, not a longer name ending in it (`open_external_url(`).
                    let opens = code.match_indices("open_url(").any(|(at, _)| {
                        code[..at]
                            .chars()
                            .next_back()
                            .is_none_or(|c| !(c.is_alphanumeric() || c == '_'))
                    });
                    let forbidden = sets_href
                        || opens
                        || code.contains("LinkOpenWith::System")
                        || code.contains("TextViewLinks::Open");
                    if forbidden {
                        offenders.push(format!(
                            "{}:{}: {}",
                            relative.display(),
                            number + 1,
                            line.trim()
                        ));
                    }
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "open addresses and set link hrefs only through runtime/services/links.rs:\n{}",
            offenders.join("\n")
        );
    }
}
