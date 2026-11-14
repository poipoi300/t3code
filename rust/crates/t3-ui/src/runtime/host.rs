//! The one place that chooses the host: the real client (`client.rs`: t3-client's composition
//! root over t3-platform) unless the launch asks for the demo explicitly.

use std::rc::Rc;

use futures::FutureExt;
use futures::future::LocalBoxFuture;
use t3_client::ClientStore;

use super::backend::Backend;
use super::demo::DemoBackend;

/// On native targets, `T3_UI_BACKEND=demo` starts the demo backend.
pub const DEMO_ENV: &str = "T3_UI_BACKEND";
/// In the launch address (a web page, an app link), `?backend=demo` starts the demo backend.
pub const DEMO_QUERY: &str = "backend=demo";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Client,
    Demo,
}

/// The demo only when asked for by name; no address, no variable or any other value is the
/// real client.
pub fn choose(address: Option<&str>, env: Option<&str>) -> Choice {
    let query = address
        .and_then(|address| address.split_once('?'))
        .map(|(_, rest)| rest.split('#').next().unwrap_or(""));
    let asked = query.is_some_and(|query| query.split('&').any(|pair| pair == DEMO_QUERY));
    if asked || env == Some("demo") {
        Choice::Demo
    } else {
        Choice::Client
    }
}

/// [`choose`] with this process's environment (native) or none (the web).
pub fn launch_choice(address: Option<&str>) -> Choice {
    #[cfg(not(target_arch = "wasm32"))]
    let env = std::env::var(DEMO_ENV).ok();
    #[cfg(target_arch = "wasm32")]
    let env: Option<String> = None;
    choose(address, env.as_deref())
}

/// A started backend, and whether an environment was saved by an earlier run (so the app opens
/// on the workspace while it reconnects instead of the pairing screen).
pub struct Started {
    pub backend: Rc<dyn Backend>,
    pub choice: Choice,
    pub paired: bool,
}

pub fn start(store: ClientStore, choice: Choice) -> LocalBoxFuture<'static, Started> {
    match choice {
        Choice::Demo => futures::future::ready(Started {
            backend: Rc::new(DemoBackend::new(store)),
            choice,
            paired: false,
        })
        .boxed_local(),
        Choice::Client => super::client::start(store),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_real_client_is_the_default_and_the_demo_needs_its_switch() {
        assert_eq!(choose(None, None), Choice::Client);
        assert_eq!(choose(Some("https://app.t3.codes/"), None), Choice::Client);
        assert_eq!(
            choose(
                Some("https://app.t3.codes/t/x?backend=client"),
                Some("real")
            ),
            Choice::Client
        );
        assert_eq!(
            choose(Some("https://app.t3.codes/#backend=demo"), None),
            Choice::Client,
            "a fragment is not the query"
        );
        assert_eq!(
            choose(Some("https://app.t3.codes/?x=1&backend=demo#top"), None),
            Choice::Demo
        );
        assert_eq!(choose(None, Some("demo")), Choice::Demo);
    }
}
