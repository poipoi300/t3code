//! Port of apps/web/src/components/ui/alert.tsx Alert, AlertTitle, AlertDescription, AlertAction.
//!
//! `alert(Props, |Context| Slots)` renders icon, content and action groups in web order.
//! `Variant::{Default, Sidebar, Error, Info, Success, Warning}`, `Surface::{Default, Glass}`
//! and `ControlAlignment::{Center, FirstLine}` have the TSX names and defaults. There is no
//! `Size` prop in the source. Content/action slots accept arbitrary elements; icon slots take
//! Lucide `Icon`s. Text is translated by the caller.
//! `alert_title`, `alert_description` and `alert_action` own their styling and return opaque
//! elements. Pass the supplied `Context` to descriptions so parent variant selectors apply.
//! `Props::scope` selects the sidebar's redeclared tokens when mounted there.
//!
//! [`composer_banner`] ports every slot of `chat/ComposerBanner.tsx`; stacking, prioritisation,
//! details popovers, expansion and dismissal state belong to CMP-BANNERS. Its `root` passes a
//! measured `Context` to its builder; `row` passes a `RowContext` to its slots. These explicit
//! contexts replace CSS descendant selectors without global state or platform branches.

use nsh_core::prelude::*;

use crate::theme::{Scope, cn, palette, tw, tw_with};
use crate::ui::icon::{Icon, icon};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Variant {
    #[default]
    Default,
    Sidebar,
    Error,
    Info,
    Success,
    Warning,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Surface {
    #[default]
    Default,
    Glass,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ControlAlignment {
    #[default]
    Center,
    FirstLine,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Props {
    pub variant: Variant,
    pub surface: Surface,
    pub control_alignment: ControlAlignment,
    pub scope: Scope,
}

/// Explicit groups replace React's child-type/data-slot introspection; order within each is kept.
#[derive(Default)]
pub struct Slots {
    pub icons: Vec<Icon>,
    pub content: Vec<AnyElement>,
    pub actions: Vec<AnyElement>,
}

#[derive(Clone, Copy)]
pub struct Context {
    props: Props,
}

impl Variant {
    fn icon_color(self) -> AdaptiveColor {
        let p = palette();
        match self {
            Self::Default => p.muted_foreground,
            Self::Sidebar => p.card_foreground,
            Self::Error => p.error,
            Self::Info => p.info,
            Self::Success => p.success,
            Self::Warning => p.warning,
        }
    }
}

pub fn alert(props: Props, build: impl FnOnce(Context) -> Slots) -> impl IntoElement {
    let Slots {
        icons,
        content,
        actions,
    } = build(Context { props });
    let first_line = props.control_alignment == ControlAlignment::FirstLine;
    let has_actions = !actions.is_empty();
    let icon_group = (!icons.is_empty()).then(|| {
        view()
            .app_class(tw(&cn([
                Some("flex shrink-0 items-center justify-center"),
                Some(if first_line { "w-4" } else { "size-4" }),
            ])))
            // NSH has no `lh` unit: text-sm is 20px, sidebar's leading-4 is 16px.
            .h(px(if first_line && props.variant != Variant::Sidebar {
                20.0
            } else {
                16.0
            }))
            .children(
                icons
                    .into_iter()
                    .map(move |glyph| icon(glyph, 16.0, move || props.variant.icon_color())),
            )
    });
    let content_group = (!content.is_empty()).then(|| {
        view()
            .app_class(tw("flex min-w-0 flex-1 flex-col gap-0.5"))
            .children(content)
    });
    let action_group = has_actions.then(|| {
        view()
            .app_class(tw(&cn([
                Some("flex shrink-0 items-center"),
                Some(if first_line {
                    "self-start"
                } else {
                    "self-center"
                }),
            ])))
            .h(if first_line {
                Length::from(px(if props.variant == Variant::Sidebar {
                    16.0
                } else {
                    20.0
                }))
            } else {
                Length::Auto
            })
            .children(actions)
    });
    view()
        .role(AccessibilityRole::Alert)
        .app_class(tw_with(props.scope, &cn([
            Some("relative rounded-xl border px-3.5 py-3 text-card-foreground text-sm"),
            // index.css's base `* { @apply border-border }`; NSH's bare border uses currentColor.
            Some("border-border"),
            // Descendant SVG/description selectors are applied to those slots directly.
            Some(match props.variant {
                Variant::Default => "bg-transparent dark:bg-input/32",
                Variant::Sidebar => "rounded-lg border-sidebar-border bg-sidebar-control-surface px-2 py-1.5 text-[11px] leading-4",
                Variant::Error => "border-error/32 bg-error-surface text-error-foreground",
                Variant::Info => "border-info/32 bg-info/4",
                Variant::Success => "border-success/32 bg-success/4",
                Variant::Warning => "border-warning/32 bg-warning-surface text-warning-foreground",
            }),
            // NSH cannot parse alpha variables; theme::pair only registers light numeric values.
            (props.surface == Surface::Glass).then_some("z-0 bg-background/80 dark:bg-background/80 backdrop-blur-(--glass-blur) dark:backdrop-blur-[16px] backdrop-saturate-[1.14] dark:backdrop-saturate-[1.08]"),
        ]), &[]))
        // index.css @utility alert-glass: its tint is a first-child background layer.
        .child((props.surface == Surface::Glass).then(|| glass_tint(props.variant)))
        .child(view().app_class(tw(&cn([
            Some("flex gap-2"),
            Some(if first_line { "items-start" } else { "items-center" }),
            (first_line && has_actions).then_some("min-h-7 pt-1 sm:min-h-6 sm:pt-0.5"),
        ]))).child(icon_group).child(content_group).child(action_group))
}

fn glass_tint(variant: Variant) -> impl IntoElement {
    view()
        .app_class(tw(&cn([
            Some("pointer-events-none absolute inset-0 -z-1"),
            Some(if variant == Variant::Sidebar {
                "rounded-lg"
            } else {
                "rounded-xl"
            }),
        ])))
        .bg(move || {
            let p = palette();
            match variant {
                Variant::Error => p.destructive.opacity(0.04),
                Variant::Info => p.info.opacity(0.04),
                Variant::Success => p.success.opacity(0.04),
                Variant::Warning => p.warning.opacity(0.04),
                _ => AdaptiveColor::TRANSPARENT,
            }
        })
}

pub fn alert_title(content: impl IntoElement) -> impl IntoElement {
    view().app_class(tw("font-medium")).child(content)
}

pub fn alert_description(context: Context, content: impl IntoElement) -> impl IntoElement {
    let props = context.props;
    view()
        .app_class(tw_with(
            props.scope,
            &cn([
                Some("flex flex-col gap-2.5 text-muted-foreground"),
                match props.variant {
                    Variant::Sidebar => Some("block text-sidebar-muted-foreground"),
                    Variant::Error => Some("text-error-foreground/80"),
                    Variant::Warning => Some("text-warning-foreground/80"),
                    _ => None,
                },
            ]),
            &[],
        ))
        .child(content)
}

pub fn alert_action(content: impl IntoElement) -> impl IntoElement {
    view().app_class(tw("flex gap-1")).child(content)
}

/// Port of apps/web/src/components/chat/ComposerBanner.tsx ComposerBanner's named slots.
///
/// `root(Props, |Context| ...)`, `row(Context, Layout, |RowContext| RowSlots)` and
/// `row_button` retain the root/row/slot tree. Every slot returns `impl IntoElement`.
/// `Variant`, `Density`, `Width` and `Layout` preserve the web prop names; there is no Size.
/// `SurfaceProps::joined` replaces the adjacent-attachment selector. Use `AttachmentProps::last`
/// for the Column's last attachment. `dock` takes a reactive attached-surface predicate and
/// `column` takes a reactive empty predicate, replacing `:has`/`:empty`.
/// `children` supplies a Context with 20px rows, replacing its descendant min-height selector.
/// `dismiss` uses Button's ghost/icon-xs primitive; disabled stays reactive and inert.
/// Flex span slots become views because NSH text cannot host their row/grid layout.
pub mod composer_banner {
    use super::*;
    use crate::i18n::t;
    use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub enum Variant {
        #[default]
        Default,
        Error,
        Info,
        Success,
        Warning,
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub enum Density {
        #[default]
        Default,
        Comfortable,
        Spacious,
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub enum Width {
        #[default]
        Fill,
        Content,
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub enum Layout {
        #[default]
        Inline,
        WrapActions,
        Approval,
    }

    #[derive(Clone, Copy, Debug, Default)]
    pub struct SurfaceProps {
        pub variant: Variant,
        pub detached: bool,
        /// A preceding attached surface shares this surface's top edge.
        pub joined: bool,
    }

    #[derive(Clone, Copy, Debug, Default)]
    pub struct Props {
        pub surface: SurfaceProps,
        pub density: Density,
        pub width: Width,
        /// BannerStack supplies Alert for error/warning notices; Root itself has no role.
        pub role: AccessibilityRole,
    }

    #[derive(Clone, Copy, Debug, Default)]
    pub struct AttachmentProps {
        /// `Column` forces immediate attachments to full width.
        pub in_column: bool,
        pub last: bool,
    }

    #[derive(Clone, Copy)]
    pub struct Context {
        width: Signal<f32>,
        compact_rows: bool,
    }

    impl Context {
        /// An existing size-query container's measured content-box width, for Width::Content rows.
        pub fn from_container_width(width: Signal<f32>) -> Self {
            Self {
                width,
                compact_rows: false,
            }
        }
    }

    #[derive(Clone, Copy)]
    pub struct RowContext {
        banner: Context,
        layout: Layout,
        presence: Signal<RowPresence>,
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    struct RowPresence {
        icon: bool,
        actions: bool,
    }

    #[derive(Default)]
    pub struct RowSlots {
        pub icon: Option<AnyElement>,
        pub content: Option<AnyElement>,
        pub actions: Option<AnyElement>,
    }

    fn overlap(detached: bool) -> f32 {
        if detached { 0.0 } else { 17.0 }
    }

    fn icon_column() -> f32 {
        if crate::state::layout::layout_class().is_compact() {
            28.0
        } else {
            24.0
        }
    }

    /// Surface from ComposerBanner.tsx. `content` is built once; theme colours stay reactive.
    pub fn surface(props: SurfaceProps, content: impl IntoElement) -> impl IntoElement {
        surface_frame(props, "").child(content)
    }

    fn surface_frame(props: SurfaceProps, classes: &str) -> View {
        // `before:` is a first-child layer; `isolate` and supports-* have no NSH meaning.
        // NSH has no mask-image: the attached top fade is unsupported; its 17px overlap remains.
        view().app_class(tw(&cn([Some("relative z-0 border-0 bg-transparent shadow-none"), Some(classes)])))
            .child(view()
                .app_class(tw(&cn([
                    // index.css's glass defaults; NSH's alpha parser cannot resolve a variable.
                    Some("pointer-events-none absolute inset-0 -z-1 border bg-card/80 dark:bg-surface-raised/80 backdrop-blur-(--glass-blur) dark:backdrop-blur-[16px] backdrop-saturate-[1.14] dark:backdrop-saturate-[1.08] shadow-[0_1px_2px_rgb(0_0_0/5%)] dark:shadow-[0_14px_32px_-18px_rgb(0_0_0/75%)]"),
                    Some(if props.detached { "rounded-2xl" } else if props.joined { "rounded-none border-t-0" } else { "rounded-t-2xl" }),
                    Some(match props.variant {
                        Variant::Error => "border-error/32",
                        Variant::Warning => "border-warning/28",
                        // Composer-specific custom-theme outlines need the frozen theme bridge.
                        _ => "border-contrast-foreground/8 dark:border-white/5",
                    }),
                ])))
                .child(view().app_class(tw(&cn([
                    Some("absolute inset-0 pointer-events-none"),
                    Some(if props.detached { "rounded-2xl" } else if props.joined { "rounded-none" } else { "rounded-t-2xl" }),
                    match props.variant {
                        Variant::Error => Some("bg-error/8"),
                        Variant::Warning => Some("bg-warning/8"),
                        _ => None,
                    },
                ])))))
    }

    /// Attachment from ComposerBanner.tsx; sibling/column selectors are explicit props.
    pub fn attachment(props: AttachmentProps, content: impl IntoElement) -> impl IntoElement {
        view().app_class(attachment_classes(props)).child(content)
    }

    fn attachment_classes(props: AttachmentProps) -> SharedString {
        tw(&cn([
            Some("mx-auto -mb-[17px] w-full"),
            // NSH cannot mix percentage and absolute widths. This unpainted wrapper's padding
            // gives the same child box; ComposerSurface.Shell defines the inset as 1.375rem.
            (!props.in_column).then_some("px-[1.375rem]"),
            (props.in_column && props.last).then_some("mb-0"),
        ]))
    }

    /// Dock from ComposerBanner.tsx; an empty notice stack must not hide the next attachment.
    pub fn dock(
        props: AttachmentProps,
        has_attached_surface: impl Fn() -> bool + 'static,
        content: impl IntoElement,
    ) -> impl IntoElement {
        view()
            .app_class(move || {
                Some(tw(&cn([
                    Some(attachment_classes(props).as_ref()),
                    Some("flex items-end gap-1"),
                    (!has_attached_surface()).then_some("hidden"),
                ])))
            })
            .child(content)
    }

    /// Column from ComposerBanner.tsx. Empty state is supplied by the stack's own state.
    pub fn column(
        empty: impl Fn() -> bool + 'static,
        content: impl IntoElement,
    ) -> impl IntoElement {
        view()
            .app_class(move || {
                Some(tw(&cn([
                    Some("flex min-w-0 flex-1 flex-col"),
                    empty().then_some("hidden"),
                ])))
            })
            .child(content)
    }

    /// Root from ComposerBanner.tsx; the measured width replaces its @container.
    /// For Width::Content, pass an ancestor Context to rows that query the ancestor's container.
    pub fn root<E: IntoElement>(
        props: Props,
        build: impl FnOnce(Context) -> E,
    ) -> impl IntoElement {
        let width = signal(0.0);
        let content = build(Context::from_container_width(width));
        surface_frame(
            props.surface,
            &cn([
                Some("min-w-0 px-1 text-xs/4"),
                Some(match props.density {
                    Density::Default => "py-1",
                    Density::Comfortable => "py-1.25",
                    Density::Spacious => "px-3 py-3",
                }),
                (props.width == Width::Content).then_some("w-fit max-w-full flex-none"),
            ]),
        )
        .role(props.role)
        .on_layout(move |bounds: &Bounds<Points>| {
            // CSS size-container queries measure the content box, excluding Root's padding.
            let padding = if props.density == Density::Spacious {
                24.0
            } else {
                8.0
            };
            width.set((bounds.size.width.0 - padding).max(0.0));
        })
        .child(content)
        // Root's `after:block after:h-(--chat-composer-attachment-overlap)`.
        .child(view().h(px(overlap(props.surface.detached))))
    }

    fn row_context(context: Context, layout: Layout) -> RowContext {
        RowContext {
            banner: context,
            layout,
            presence: signal(RowPresence::default()),
        }
    }

    fn row_classes(context: RowContext) -> SharedString {
        tw(&cn([
            Some("group/banner-row grid w-full min-w-0 items-center gap-x-1 text-start"),
            Some(if context.presence.get().actions {
                "grid-cols-[28px_minmax(0,1fr)_auto] sm:grid-cols-[24px_minmax(0,1fr)_auto]"
            } else {
                "grid-cols-[28px_minmax(0,1fr)] sm:grid-cols-[24px_minmax(0,1fr)]"
            }),
            Some(if context.banner.compact_rows {
                "min-h-5"
            } else {
                "min-h-7 sm:min-h-6"
            }),
            match context.layout {
                Layout::Inline => None,
                Layout::WrapActions => Some("flex flex-wrap"),
                Layout::Approval => Some("items-start gap-x-2 gap-y-3"),
            },
        ]))
    }

    /// Row from ComposerBanner.tsx. Use row_button for the source's render={<button .../>}.
    pub fn row(
        context: Context,
        layout: Layout,
        build: impl FnOnce(RowContext) -> RowSlots,
    ) -> impl IntoElement {
        let context = row_context(context, layout);
        let slots = build(context);
        context.presence.set(RowPresence {
            icon: slots.icon.is_some(),
            actions: slots.actions.is_some(),
        });
        view()
            .app_class(move || Some(row_classes(context)))
            .child(slots.icon)
            .child(slots.content)
            .child(slots.actions)
    }

    /// Row's useRender button alternative from ComposerBanner.tsx; label is translated at display.
    pub fn row_button(
        context: Context,
        layout: Layout,
        label: impl Fn() -> String + 'static,
        disabled: impl Fn() -> bool + 'static,
        on_press: impl Fn() + 'static,
        build: impl FnOnce(RowContext) -> RowSlots,
    ) -> impl IntoElement {
        let context = row_context(context, layout);
        let slots = build(context);
        context.presence.set(RowPresence {
            icon: slots.icon.is_some(),
            actions: slots.actions.is_some(),
        });
        nsh_base::button()
            .accessibility_label(move || Some(SharedString::from(t(&label()))))
            .disabled(disabled)
            .on_press(on_press)
            // The shared adaptive contract adds a 44pt coarse target without changing row layout.
            .hit_slop(move || {
                let minimum = if context.banner.compact_rows { 20.0 } else { icon_column() };
                let slack = ((crate::theme::metrics().hit_target - minimum) / 2.0).max(0.0);
                EdgeInsets::all(px(slack))
            })
            .app_class(move || Some(tw(&cn([
                Some(row_classes(context).as_ref()),
                Some("cursor-pointer rounded-xl hover:bg-foreground/5 focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring"),
            ]))))
            .child(slots.icon).child(slots.content).child(slots.actions)
    }

    /// Icon from ComposerBanner.tsx; selector sizes and tints are applied at the icon call.
    pub fn icon(context: RowContext, glyph: Icon) -> impl IntoElement {
        view().accessible(false)
            .app_class(tw(&cn([
                Some("col-start-1 row-start-1 flex min-w-0 flex-none items-center justify-center text-muted-foreground"),
                (context.layout == Layout::Approval).then_some("pt-0.5 text-warning"),
            ])))
            .w(move || px(icon_column()))
            .child(crate::ui::icon::icon(glyph, if context.layout == Layout::Approval { 16.0 } else { 12.0 }, move || {
                if context.layout == Layout::Approval { palette().warning } else { palette().muted_foreground }
            }))
    }

    /// Content from ComposerBanner.tsx; the builder receives the same RowContext for Separator.
    pub fn content(context: RowContext, content: impl IntoElement) -> impl IntoElement {
        view()
            .app_class(move || {
                Some(tw(&cn([
                    Some("col-start-2 row-start-1 flex min-w-0 items-center gap-1"),
                    (!context.presence.get().icon).then_some("col-[1/3] ps-2 sm:ps-1.5"),
                    (!context.presence.get().icon && !context.presence.get().actions)
                        .then_some("pe-2 sm:pe-1.5"),
                    (context.layout == Layout::Approval && context.banner.width.get() < 560.0)
                        .then_some("col-end-4"),
                    (context.layout == Layout::WrapActions).then_some("grow"),
                ])))
            })
            // `*:data-[slot=composer-banner-content]:max-w-[calc(...)]` at the selected child.
            .max_w(move || {
                if context.layout == Layout::WrapActions {
                    Length::from(px(
                        (context.banner.width.get() - icon_column() - 4.0).max(0.0)
                    ))
                } else {
                    Length::Auto
                }
            })
            .child(content)
    }

    /// Separator from ComposerBanner.tsx; `in_content` applies Content's direct-child mx-0.
    pub fn separator(in_content: bool) -> impl IntoElement {
        text(crate::i18n::ts("·"))
            .accessible(false)
            .app_class(tw(&cn([
                Some("mx-1 inline-block flex-none text-muted-foreground/40"),
                in_content.then_some("mx-0"),
            ])))
    }

    /// Actions from ComposerBanner.tsx. Root's button-corner selectors require a Button seam.
    pub fn actions(context: RowContext, content: impl IntoElement) -> impl IntoElement {
        view()
            .app_class(move || {
                Some(tw(&cn([
                    Some("col-start-3 row-start-1 flex flex-wrap items-center justify-end gap-1"),
                    (context.layout == Layout::Approval).then_some("self-center gap-1.5"),
                    (context.layout == Layout::Approval && context.banner.width.get() < 560.0)
                        .then_some("col-start-2 col-end-4 row-start-2"),
                    (context.layout == Layout::WrapActions).then_some("ms-auto"),
                ])))
            })
            .child(content)
    }

    /// Children from ComposerBanner.tsx; no wrapper-dependent row discovery is needed.
    pub fn children<E: IntoElement>(
        context: Context,
        build: impl FnOnce(Context) -> E,
    ) -> impl IntoElement {
        view().app_class(tw("grid gap-px")).child(build(Context {
            compact_rows: true,
            ..context
        }))
    }

    /// Scroll from ComposerBanner.tsx; NSH's native scroll owns overflow and the indicator.
    pub fn scroll(content: impl IntoElement) -> impl IntoElement {
        // The ScrollArea's fade and overflow-only pe-2 need UI-SCROLL's primitive seam.
        nsh_core::prelude::scroll()
            .app_class(tw("h-auto"))
            .max_h(move || px(384.0_f32.min(window().height.0 * 0.4)))
            .child(view().child(content))
    }

    /// Count from ComposerBanner.tsx; the caller formats/translates its arbitrary content.
    pub fn count(content: impl IntoElement) -> impl IntoElement {
        view()
            .app_class(tw("inline-flex flex-none justify-center font-medium text-muted-foreground tabular-nums"))
            .min_w(move || px(icon_column()))
            .child(content)
    }

    /// Body from ComposerBanner.tsx.
    pub fn body(content: impl IntoElement) -> impl IntoElement {
        view().app_class(tw("min-w-0 ps-8 sm:ps-7")).child(content)
    }

    /// Dot from ComposerBanner.tsx.
    pub fn dot() -> impl IntoElement {
        view()
            .accessible(false)
            .app_class(tw("size-1.5 flex-none rounded-full bg-current"))
    }

    /// ToggleIcon from ComposerBanner.tsx; this is a decorative span, never a nested button.
    pub fn toggle_icon(expanded: impl Fn() -> bool + 'static) -> impl IntoElement {
        // Button render={<span aria-hidden/>}: copy its ghost/icon-xs classes onto a View.
        view().accessible(false)
            .app_class(tw("pointer-events-none relative inline-flex shrink-0 items-center justify-center gap-2 rounded-(--control-radius) border border-transparent text-foreground size-7 sm:size-6"))
            .child(image(Icon::ChevronDown.asset()).accessible(false)
                .tint_color(move || palette().muted_foreground)
                .app_class(move || Some(tw(&cn([
                    Some("size-3.5"),
                    (!expanded()).then_some("rotate-180"),
                ])))))
    }

    /// Dismiss from ComposerBanner.tsx. The caller owns removal and its accessible label.
    pub fn dismiss(
        label: String,
        disabled: impl Fn() -> bool + 'static,
        on_press: impl Fn() + 'static,
    ) -> impl IntoElement {
        // The frozen Button API cannot request the source's 14px icon or arbitrary children.
        button(
            ButtonContent::Icon {
                icon: Icon::X,
                label: t(&label),
            },
            ButtonVariant::Ghost,
            ButtonSize::IconXs,
            disabled,
            on_press,
        )
    }
}
