//! Port of apps/web/src/components/guest/GuestPage.tsx (standalone mount and access notices).

use nsh_core::prelude::*;

use crate::i18n::{self, t};
use crate::runtime::tasks::watch;
use crate::theme::{self, tw};
pub(crate) use t3_client::guest::GuestAddress;

pub(crate) fn boot(location: Option<GuestAddress>) -> AnyElement {
    i18n::set_language("system", &[locale().to_string()]);
    let has_share = signal(
        location
            .as_ref()
            .is_some_and(|url| url.share_id().is_some()),
    );
    if let Some(location) = location {
        watch(async move {
            has_share.set(replace_address(&location));
            loop {
                let change = next_app_url_change().await;
                if let Some(next) = GuestAddress::parse(&change.url) {
                    let has_next_share = if change.url.contains('#') {
                        replace_address(&next)
                    } else {
                        next.share_id().is_some()
                    };
                    has_share.set(has_next_share);
                }
            }
        });
    }
    theme::with_saved_theme(move || {
        view()
            .app_class(tw("flex h-full flex-col bg-background text-foreground"))
            .child(dynamic(move || {
                if has_share.get() {
                    notice("Guest sharing is unavailable.", None)
                } else {
                    invalid_link()
                }
            }))
            .into_element()
    })
}

fn replace_address(location: &GuestAddress) -> bool {
    if set_app_url(location.history_path()).is_ok() {
        return location.share_id().is_some();
    }
    // NSH caps address length, and encoding a malformed id may exceed the cap.
    let _ = set_app_url("/guest");
    false
}

pub fn screen() -> AnyElement {
    invalid_link()
}

fn invalid_link() -> AnyElement {
    notice(
        "This link has ended or is not valid",
        Some("Ask the owner for a new link. Each link works in one browser."),
    )
}

fn notice(title: &'static str, detail: Option<&'static str>) -> AnyElement {
    let mut body = view()
        .app_class(tw("m-auto flex max-w-sm flex-col gap-1 p-6 text-center"))
        .child(text(move || t(title)).app_class(tw("text-sm font-medium")));
    if let Some(detail) = detail {
        body = body.child(text(move || t(detail)).app_class(tw("text-xs text-muted-foreground")));
    }
    body.into_element()
}
