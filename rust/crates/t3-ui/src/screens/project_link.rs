//! Port of apps/web/src/routes/projects.$projectKey.tsx (its `beforeLoad`): `/projects/<key>` is
//! not a page. Signed in (here: an environment is saved), it replaces itself with
//! `/settings/projects?project=<key>`; otherwise with `/pair`.
//!
//! The rule and the encoding of `<key>` are the client's (`navigation::Location::resolve`). The
//! replacement address carries `?project=`, so the settings page (SET-PROJECTS) can read it from
//! the app's address; the route stack carries no search.

use nsh_core::prelude::*;
use t3_client::navigation::{Location, RoutePolicy, path_for};

use crate::runtime::rt;
use crate::state::nav::{Route, navigator};

pub fn screen(key: &str) -> AnyElement {
    let nav = navigator();
    let signed_in = !rt().environments_signal().get_untracked().is_empty();
    let path = path_for(&Route::ProjectLink(key.to_owned()));
    // A route change during the router's own render would rebuild what is rendering; move after.
    spawn(async move {
        let Some(path) = path else { return };
        let location = Location::resolve(&path, redirect_policy(signed_in));
        replace(nav, location);
    });
    view().into_element()
}

/// The web's `beforeLoad` reads `authGateState`; this client is signed in exactly when it has a
/// saved environment (the hosted app's static mode does not exist here).
fn redirect_policy(signed_in: bool) -> RoutePolicy {
    RoutePolicy {
        authenticated_or_hosted: signed_in,
        ..RoutePolicy::default()
    }
}

/// `redirect({ replace: true })`: the link's entry is replaced, in the address and in the stack, so
/// Back does not return to a page that only redirects.
fn replace(nav: crate::state::nav::Navigator, location: Location) {
    let _ = set_app_url(location.href());
    if matches!(nav.top(), Route::ProjectLink(_)) {
        nav.back();
    }
    nav.open(location.route);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_environment_sends_the_link_to_the_project_settings_else_to_pairing() {
        let path = path_for(&Route::ProjectLink("env:repo".into())).expect("an address");
        let signed_in = Location::resolve(&path, redirect_policy(true));
        assert_eq!(
            signed_in.route,
            Route::Settings(crate::state::nav::SettingsPage::Projects)
        );
        assert_eq!(signed_in.href(), "/settings/projects?project=env%3Arepo");
        let signed_out = Location::resolve(&path, redirect_policy(false));
        assert_eq!(signed_out.route, Route::Pair);
    }
}
