//! Port of `apps/web/src/components/sidebar/SidebarThreadRow.tsx` `SidebarThreadRow`: the card
//! variant (status slot showing the relative time; no status, draft, pin, controls, meta line or
//! provider stack yet: SB-ROW), and the slim variant's single line.

use nsh_core::prelude::*;
use t3_client::services::config::timestamp_ms;
use t3_client::view_logic::session::timestamps::{
    format_relative_elapsed, relative_time_elapsed_ms,
};

use super::{RowVariant, SidebarItem};
use crate::i18n::t;
use crate::state::clock::now_ms;
use crate::state::nav::{Navigator, Route};
use crate::theme::{Scope, cn, palette, tw_with};
use crate::ui::icon::{Icon, icon};

/// `compactSidebarTimeLabel(formatRelativeTimeLabel(..))`: `now`, `5m`, `2h`.
pub fn relative_label(iso: &str, now_ms: i64) -> String {
    let Some(date) = timestamp_ms(iso) else {
        return String::new();
    };
    let label = t(&format_relative_elapsed(relative_time_elapsed_ms(date, now_ms)).value);
    compact_sidebar_time_label(&label, &t("just now"), &t("now"))
}

/// `compactSidebarTimeLabel`, on the translated labels.
fn compact_sidebar_time_label(label: &str, just_now: &str, now: &str) -> String {
    if label == just_now {
        return now.to_owned();
    }
    label.strip_suffix(" ago").unwrap_or(label).to_owned()
}

pub fn thread_row(item: Memo<Option<SidebarItem>>, nav: Navigator) -> impl IntoElement {
    let field = move |read: fn(&SidebarItem) -> String| {
        move || item.with(|i| i.as_ref().map(read).unwrap_or_default())
    };
    let title = field(|i| i.title.clone());
    let project = field(|i| i.project.clone());
    let key = move || item.with(|i| i.as_ref().map(|i| i.key.clone()));
    let card = move || item.with(|i| i.as_ref().is_some_and(|i| i.variant == RowVariant::Card));
    let time = move || {
        item.with(|i| {
            i.as_ref()
                .and_then(|i| i.activity_at.as_deref())
                .map(|at| relative_label(at, now_ms()))
                .unwrap_or_default()
        })
    };
    let active = move || {
        let current = nav
            .stack
            .with(|stack| stack.last().and_then(|r| r.thread().cloned()));
        current.is_some() && current == key()
    };
    let surface = move || {
        let classes = cn([
            Some(
                "group/sidebar-row relative w-full cursor-pointer overflow-hidden rounded-md text-left outline-none select-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring",
            ),
            Some(if active() {
                "bg-sidebar-row-active text-sidebar-foreground light:ring-1 light:ring-inset light:ring-sidebar-border/70"
            } else {
                "bg-transparent text-sidebar-foreground hover:bg-sidebar-row-hover"
            }),
        ]);
        Some(tw_with(Scope::Sidebar, &classes, &[]))
    };
    let class = |classes: &'static str| Some(tw_with(Scope::Sidebar, classes, &[]));
    let title_text = move || SharedString::from(title());

    let card_body = move || {
        view()
            // `h-auto`: `metaLineEmpty`, while the meta line (branch, PR, diff) is SB-ROW's.
            .app_class(class(
                "relative z-10 h-[4.875rem] px-(--sidebar-row-content-inset) py-(--sidebar-content-inset) h-auto",
            ))
            .child(
                view()
                    .app_class(class("flex h-5 min-w-0 items-center gap-1.5"))
                    // `ProjectFavicon`'s fallback until favicons load (SB-ROW).
                    .child(icon(Icon::Folder, 16.0, move || {
                        palette().sidebar_muted_foreground
                    }))
                    .child(
                        text(move || SharedString::from(project()))
                            .app_class(class(
                                "min-w-0 flex-1 truncate font-normal text-secondary-label text-xs",
                            )),
                    )
                    .child(
                        view()
                            .app_class(class(
                                "relative ml-auto flex h-5 min-w-8 shrink-0 items-stretch justify-end text-xs",
                            ))
                            .child(
                                text(move || SharedString::from(time())).app_class(class(
                                    "flex items-center self-center tabular-nums text-secondary-label text-xs",
                                )),
                            ),
                    ),
            )
            .child(
                view()
                    .app_class(class("mt-1 flex min-w-0 items-center gap-1.5"))
                    .child(text(title_text).app_class(class(
                        "min-w-0 flex-1 text-start text-sm font-medium truncate text-foreground",
                    ))),
            )
    };
    let slim_body = move || {
        view()
            .app_class(class("flex h-9 items-center gap-2.5 px-2.5"))
            .child(text(title_text).app_class(class(
                "min-w-0 flex-1 text-start text-sm font-medium truncate text-foreground",
            )))
            .child(text(move || SharedString::from(time())).app_class(class(
                "inline-flex justify-end tabular-nums text-secondary-label text-xs",
            )))
    };

    // `<li className="list-none py-0.5">`: `list-none` has no NSH meaning (no list markers).
    view().app_class(class("py-0.5")).child(
        nsh_base::button()
            .accessibility_label(move || Some(title_text()))
            .selected(active)
            .on_press(move || {
                if let Some(key) = key() {
                    nav.open(Route::Thread(key));
                }
            })
            .app_class(surface)
            .child(when(card, card_body).otherwise(slim_body)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_labels_come_from_the_clients_rules_compacted_as_the_sidebar_does() {
        let now = timestamp_ms("2026-11-13T12:10:00.000Z").expect("valid");
        assert_eq!(relative_label("2026-11-13T12:05:00.000Z", now), "5m");
        assert_eq!(relative_label("2026-11-13T10:00:00.000Z", now), "2h");
        assert_eq!(relative_label("2026-11-13T12:09:59.000Z", now), "now");
        assert_eq!(relative_label("not a date", now), "");
        assert_eq!(
            compact_sidebar_time_label("3d ago", "just now", "now"),
            "3d"
        );
    }
}
