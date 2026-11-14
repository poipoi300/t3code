//! Port of apps/web/src/components/ui/collapsible.tsx Collapsible, CollapsibleTrigger and CollapsiblePanel.

use std::rc::Rc;
use std::time::Duration;

use nsh_core::LayoutCurve;
use nsh_core::element::LayoutTransition;
use nsh_core::prelude::*;

use crate::i18n::Text;
use crate::theme::{Scope as ThemeScope, cn, tw, tw_with};
use crate::ui::icon::{Icon, icon_with_class};

#[derive(Clone, Copy)]
pub struct CollapsibleHandle {
    open: Memo<bool>,
    disabled: Memo<bool>,
    change: Stored<Rc<dyn Fn(bool)>>,
    panel: ElementRef,
}

impl CollapsibleHandle {
    pub fn controlled(
        open: impl Fn() -> bool + 'static,
        disabled: impl Fn() -> bool + 'static,
        on_open_change: impl Fn(bool) + 'static,
    ) -> Self {
        Self {
            open: computed(open),
            disabled: computed(disabled),
            change: stored(Rc::new(on_open_change)),
            panel: element_ref(),
        }
    }

    pub fn new(default_open: bool) -> Self {
        let open = signal(default_open);
        Self::controlled(move || open.get(), || false, move |next| open.set(next))
    }

    pub fn is_open(self) -> bool {
        self.open.get()
    }

    pub fn toggle(self) {
        if !self.disabled.get_untracked() {
            self.change
                .with(|change| change(!self.open.get_untracked()));
        }
    }
}

pub fn collapsible(children: impl IntoElement) -> impl IntoElement {
    view().child(children)
}

pub fn collapsible_trigger(
    handle: CollapsibleHandle,
    label: Text,
    children: impl IntoElement,
    class_name: &str,
) -> impl IntoElement {
    // ButtonProps has no expanded/controls props; this is Button's host behavior with those ARIA bindings.
    view()
        .role(AccessibilityRole::Button)
        .label(move || label.resolve())
        .disabled(move || handle.disabled.get())
        .accessibility_state(move || AccessibilityState {
            expanded: Some(handle.is_open()),
            ..Default::default()
        })
        .accessibility_controls(Some(handle.panel))
        .on_press(move || handle.toggle())
        .app_class(tw(&cn([Some("cursor-pointer"), Some(class_name)])))
        .child(children)
}

pub fn collapsible_panel(
    handle: CollapsibleHandle,
    animate: bool,
    children: impl IntoElement,
) -> impl IntoElement {
    retained_panel()
        .open(move || handle.is_open())
        .animate(animate)
        .content(children)
        .element_ref(&handle.panel)
}

pub use collapsible_panel as collapsible_content;

#[derive(Clone, PartialEq, Props)]
struct RetainedPanelProps {
    open: Prop<bool>,
    animate: bool,
    content: Prop<Option<Slot>>,
}

#[component]
fn RetainedPanel(props: RetainedPanelProps) -> impl IntoElement {
    let open = props.open;
    let initial = open.get_untracked();
    let mounted = signal(initial);
    let previous = stored(initial);
    let generation = stored(0_u64);
    let region = element_ref();
    effect(move || {
        let next = open.get();
        untracked(|| {
            if previous.replace(next) == next {
                return;
            }
            let current = generation.get().wrapping_add(1);
            generation.replace(current);
            let animate = props.animate && !reduce_motion();
            if animate {
                // NSH cannot transition height or use a Bezier LayoutCurve; retain the web's 200 ms clipped reveal.
                region.animate_layout(LayoutTransition::new(LayoutCurve::EaseInEaseOut, 200.0));
            }
            if next || !animate {
                mounted.set(next);
            } else {
                set_timeout(Duration::from_millis(200), move || {
                    if generation.get() == current {
                        mounted.set(false);
                    }
                });
            }
        });
    });
    view()
        .element_ref(&region)
        .app_class(tw("overflow-hidden"))
        .style_with(move |style| if open.get() { style } else { style.h(px(0.0)) })
        // A retained component slot preserves child instances and native input state on detach.
        .child(when(mounted, move || {
            props.content.place(|content| content)
        }))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tone {
    #[default]
    Muted,
    Emphasized,
    Accent,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SectionHeaderVariant {
    #[default]
    Default,
    Panel,
}

/// Port of apps/web/src/components/ui/collapsible-section-header.tsx CollapsibleSectionHeader.
pub fn collapsible_section_header(
    handle: CollapsibleHandle,
    label: Text,
    children: impl IntoElement,
    tone: Tone,
    variant: SectionHeaderVariant,
    accessory: Option<AnyElement>,
) -> impl IntoElement {
    let has_accessory = accessory.is_some();
    let (label_class, line_class) = match tone {
        Tone::Muted => ("text-sidebar-muted-foreground", "bg-sidebar-border/50"),
        Tone::Emphasized => ("text-sidebar-foreground/80", "bg-sidebar-foreground/25"),
        Tone::Accent => ("text-primary", "bg-primary/50"),
    };
    let classes = cn([
        Some(
            "flex h-8 w-full cursor-pointer items-center gap-2 rounded-md text-left text-xs font-medium focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring/70",
        ),
        Some(match variant {
            SectionHeaderVariant::Default => "px-2",
            SectionHeaderVariant::Panel => "ps-2 pe-2.5",
        }),
        Some(label_class),
    ]);
    // The section-header source uses sidebar tokens, including inside the details panel.
    view()
        .role(AccessibilityRole::Button)
        .label(move || label.resolve())
        .disabled(move || handle.disabled.get())
        .accessibility_state(move || AccessibilityState {
            expanded: Some(handle.is_open()),
            ..Default::default()
        })
        .accessibility_controls(Some(handle.panel))
        .on_press(move || handle.toggle())
        .app_class(tw_with(ThemeScope::Sidebar, &classes, &[]))
        .child(view().app_class(tw("min-w-0 truncate")).child(children))
        .child(view().accessible(false).app_class(tw_with(
            ThemeScope::Sidebar,
            &cn([
                Some("h-px min-w-2 flex-1"),
                Some(line_class),
                has_accessory.then_some("invisible"),
            ]),
            &[],
        )))
        .child(accessory)
        .child(
            view()
                .accessible(false)
                .app_class(move || {
                    Some(tw(&cn([
                        Some("size-3 shrink-0 transition-transform"),
                        (!handle.is_open()).then_some("-rotate-90"),
                        reduce_motion().then_some("transition-none"),
                    ])))
                })
                .child(icon_with_class(Icon::ChevronDown, "size-3")),
        )
}

/// Port of apps/web/src/components/ui/collapsible-section-header.tsx SectionHeaderStatus.
pub fn section_header_status(children: impl IntoElement) -> impl IntoElement {
    view()
        .app_class(tw(
            "shrink-0 text-3xs leading-none text-destructive-foreground",
        ))
        .child(children)
}
