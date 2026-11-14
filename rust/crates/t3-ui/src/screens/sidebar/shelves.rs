//! Port of apps/web/src/components/sidebar/SidebarListMarkers.tsx sidebar section markers.

use std::time::Duration;

use nsh_core::prelude::*;
use t3_client::thread_views::threads::older_settled::OLDER_SETTLED_PAGE_SIZE;
pub use t3_client::thread_views::threads::older_settled::PageStatus as OlderSettledState;
use t3_client::view_logic::shell::sidebar::Marker;

use crate::i18n::{Text, Value, formats, t, tr};
use crate::theme::{Scope, cn, tw, tw_with};
use crate::ui::collapsible::{
    CollapsibleHandle, SectionHeaderVariant, Tone, collapsible_section_header,
};
use crate::ui::icon::{Icon, icon_with_class};

fn sortable_sidebar_marker(
    marker: Marker,
    class_name: &str,
    children: impl IntoElement,
) -> AnyElement {
    // NSH uses a view for li (no list-none); SB-DRAG owns sortable transforms and drop geometry.
    view()
        .test_id(&format!("sidebar-{}", marker.name()))
        .app_class(tw_with(Scope::Sidebar, class_name, &[]))
        .child(children)
        .into_element()
}

pub fn sidebar_section_placeholder(
    marker: Marker,
    label: Text,
    show_hint: Prop<bool>,
    is_drop_target: Prop<bool>,
) -> impl IntoElement {
    when(
        matches!(
            marker,
            Marker::ActivePlaceholder | Marker::SettledPlaceholder
        ),
        move || {
            let label = label.clone();
            sortable_sidebar_marker(
                marker,
                "relative mx-0.5 -mb-px h-0",
                when(show_hint, move || {
                    view()
                .app_class(move || {
                    Some(tw_with(
                        Scope::Sidebar,
                        &cn([
                            Some("absolute inset-x-0 top-0 flex h-9 items-center justify-center rounded-md border border-dashed border-sidebar-foreground/25 text-xs text-sidebar-foreground/80"),
                            is_drop_target.get().then_some("border-primary/40 bg-primary/5 text-primary"),
                        ]),
                        &[],
                    ))
                })
                .child(text({ let label = label.clone(); move || label.resolve() }))
                }),
            )
        },
    )
}

pub fn sidebar_drag_boundary(
    marker: Marker,
    label: Text,
    visible: Prop<bool>,
    is_drop_target: Prop<bool>,
) -> impl IntoElement {
    when(
        matches!(marker, Marker::PinnedHeader | Marker::PinnedDivider),
        move || {
            let label = label.clone();
            sortable_sidebar_marker(
                marker,
                "pointer-events-none relative mx-0.5 -mb-px h-0",
                when(visible, move || {
                    let revealed = signal(reduce_motion());
                    if !revealed.get_untracked() {
                        let owner = Owner::current();
                        // index.css's sidebar-drag-label-reveal is a one-shot 200 ms step-end.
                        let timer = set_timeout(Duration::from_millis(200), move || {
                            if owner.is_live() {
                                revealed.set(true);
                            }
                        });
                        cleanup(move || timer.cancel());
                    }
                    view()
                        .app_class(move || {
                            Some(tw(&cn([
                                Some("absolute inset-x-2 top-1 flex h-4 items-center gap-2"),
                                (!revealed.get()).then_some("invisible"),
                            ])))
                        })
                        .child(
                            text({
                                let label = label.clone();
                                move || label.resolve()
                            })
                            .app_class(move || {
                                Some(tw_with(
                                    Scope::Sidebar,
                                    &cn([
                                        Some("shrink-0 text-xs font-medium"),
                                        Some(if is_drop_target.get() {
                                            "text-primary"
                                        } else {
                                            "text-sidebar-foreground/80"
                                        }),
                                    ]),
                                    &[],
                                ))
                            }),
                        )
                        .child(view().accessible(false).app_class(move || {
                            Some(tw_with(
                                Scope::Sidebar,
                                &cn([
                                    Some("h-px flex-1"),
                                    Some(if is_drop_target.get() {
                                        "bg-primary/50"
                                    } else {
                                        "bg-sidebar-foreground/25"
                                    }),
                                ]),
                                &[],
                            ))
                        }))
                }),
            )
        },
    )
}

pub struct SidebarSectionHeaderProps {
    pub marker: Marker,
    pub label: Text,
    pub count: Prop<usize>,
    pub class_name: Option<String>,
    pub dragging: Prop<bool>,
    pub is_drop_target: Prop<bool>,
    pub toggle: CollapsibleHandle,
}

pub fn sidebar_section_header(props: SidebarSectionHeaderProps) -> impl IntoElement {
    let tone = computed(move || {
        if props.is_drop_target.get() {
            Tone::Accent
        } else if props.dragging.get() {
            Tone::Emphasized
        } else {
            Tone::Muted
        }
    });
    when(props.marker.shelf(), move || {
        sortable_sidebar_marker(
            props.marker,
            &cn([Some("relative mx-0.5 h-8"), props.class_name.as_deref()]),
            dynamic({
                let label = props.label.clone();
                move || {
                    let tone = tone.get();
                    let content = label.clone();
                    untracked(|| {
                        collapsible_section_header(
                            props.toggle,
                            label.clone(),
                            text(move || content.resolve()),
                            tone,
                            SectionHeaderVariant::Default,
                            Some(
                                text(move || formats().decimal(props.count.get() as f64))
                                    .app_class(tw("font-normal tabular-nums"))
                                    .into_element(),
                            ),
                        )
                    })
                }
            }),
        )
    })
}

/// Port of apps/web/src/components/Sidebar.tsx's trailing settled:more list item.
pub fn sidebar_settled_more(
    expanded: Prop<bool>,
    hidden_count: Prop<usize>,
    state: Prop<OlderSettledState>,
    on_more: impl Fn() + 'static,
) -> impl IntoElement {
    let on_more = stored(on_more);
    when(
        move || expanded.get() && hidden_count.get() > 0,
        move || {
            let label = computed(move || match state.get() {
                OlderSettledState::Pending => t("Loading"),
                OlderSettledState::Failed => t("Could not load older threads. Retry"),
                OlderSettledState::Ready => tr(
                    "Show {count} more",
                    &[(
                        "count",
                        Value::Int(hidden_count.get().min(OLDER_SETTLED_PAGE_SIZE) as i64),
                    )],
                ),
            });
            // NSH couples disabled accessibility to focus exclusion; busy plus the press guard
            // preserves the web's aria-disabled focus behavior. ButtonProps has no busy slot.
            view().child(
                view()
                    .test_id("sidebar-settled-more")
                    .role(AccessibilityRole::Button)
                    .label(move || label.get())
                    .accessibility_state(move || AccessibilityState {
                        busy: state.get() == OlderSettledState::Pending,
                        ..Default::default()
                    })
                    .on_press(move || {
                        if expanded.get_untracked()
                            && hidden_count.get_untracked() > 0
                            && state.get_untracked() != OlderSettledState::Pending
                        {
                            on_more.with(|on_more| on_more());
                        }
                    })
                    .app_class(move || {
                        Some(tw_with(
                            Scope::Sidebar,
                            "flex h-9 w-full cursor-pointer items-center gap-2.5 rounded-md px-2.5 text-left text-sm text-sidebar-muted-foreground hover:bg-sidebar-row-hover hover:text-sidebar-foreground aria-disabled:cursor-default",
                            &[("aria-disabled", if state.get() == OlderSettledState::Pending { "true" } else { "false" })],
                        ))
                    })
                    .child(icon_with_class(Icon::ChevronDown, "size-4 shrink-0"))
                    .child(text(move || label.get())),
            )
        },
    )
}
