//! Port of `apps/web/src/components/chat/ChatHeader.tsx` `ChatHeader` (the breadcrumb) inside
//! `ChatView.tsx`'s `<header>`. Departure: on compact layouts the stack's Back button leads, as
//! the adaptive tree pushes threads (the web shows its sidebar trigger there). Rename, the
//! actions and panel controls are TL-HEADER's.

use nsh_core::prelude::*;

use crate::i18n::t;
use crate::state::layout::layout_class;
use crate::state::nav::navigator;
use crate::theme::{palette, tw};
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};
use crate::ui::icon::{Icon, icon};

pub fn header(titles: Memo<Option<(String, Option<String>)>>) -> impl IntoElement {
    let nav = navigator();
    let title = move || {
        titles
            .get()
            .map_or_else(|| t("Loading thread…"), |(title, _)| title)
    };
    let project = move || titles.get().and_then(|(_, project)| project);
    let crumbs = view()
        .app_class(tw("min-w-0"))
        .child(
            view()
                .app_class(tw("m-0 flex min-w-0 items-center gap-2 p-0 text-sm sm:gap-3"))
                .child(dynamic(move || {
                    project().map(|name| {
                        view()
                            .app_class(tw(
                                "flex min-w-0 items-center font-medium shrink-0 text-muted-foreground shrink",
                            ))
                            .child(
                                view()
                                    .app_class(tw(
                                        "inline-flex min-w-0 max-w-full items-center gap-1.5 rounded-sm text-muted-foreground",
                                    ))
                                    // `ProjectFavicon`'s fallback until favicons load.
                                    .child(icon(Icon::Folder, 14.0, move || {
                                        palette().muted_foreground
                                    }))
                                    .child(
                                        text(SharedString::from(name))
                                            .app_class(tw("block min-w-0 truncate max-w-40")),
                                    ),
                            )
                            .into_element()
                    })
                }))
                .child(dynamic(move || {
                    project().is_some().then(|| {
                        view()
                            .app_class(tw("flex shrink-0 items-center text-icon-muted"))
                            .child(text(t("/")))
                    })
                }))
                .child(
                    view()
                        .app_class(tw(
                            "flex min-w-0 items-center font-medium text-foreground min-w-10 flex-1",
                        ))
                        .child(
                            text(move || SharedString::from(title()))
                                .app_class(tw("block min-w-0 truncate")),
                        ),
                ),
        );
    view()
        .app_class(tw(
            "relative bg-background flex h-[var(--workspace-topbar-height)] min-h-[var(--workspace-topbar-height)] shrink-0 items-center px-3 sm:px-5",
        ))
        .child(when(
            move || layout_class().is_compact(),
            move || {
                button(
                    ButtonContent::Icon {
                        icon: Icon::ArrowLeft,
                        label: t("Back"),
                    },
                    ButtonVariant::Ghost,
                    ButtonSize::Icon,
                    || false,
                    move || nav.back(),
                )
            },
        ))
        .child(
            view()
                .app_class(tw("flex min-w-0 flex-1 items-center gap-2 sm:gap-3"))
                .child(view().app_class(tw("flex-1")).child(crumbs)),
        )
}
