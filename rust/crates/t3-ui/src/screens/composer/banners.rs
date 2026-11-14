//! Port of apps/web/src/components/chat/ComposerBannerStack.tsx ComposerBannerStack.

use std::rc::Rc;
use std::time::Duration;

use nsh_core::element::text_measurer;
use nsh_core::prelude::*;

use crate::i18n::{Text as UiText, Value, tr};
use crate::runtime::Shared;
use crate::theme::{cn, tw};
use crate::ui::alert::composer_banner as banner;
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};
use crate::ui::icon::Icon;
use crate::ui::popover::{PopoverOptions, popover, popover_create_handle, popover_title};

pub type Node = Rc<dyn Fn() -> AnyElement>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ComposerBannerGroup {
    Environment,
    #[default]
    Attention,
}

/// ReactNode slots retain rich children; plain text needs no caller renderer.
#[derive(Clone)]
pub struct NoticeContent {
    pub text: UiText,
    pub rich: Option<Node>,
}

impl From<UiText> for NoticeContent {
    fn from(text: UiText) -> Self {
        Self { text, rich: None }
    }
}

impl NoticeContent {
    fn element(&self) -> AnyElement {
        if let Some(rich) = &self.rich {
            rich()
        } else {
            let value = self.text.clone();
            text(move || value.resolve()).into_element()
        }
    }
}

/// Client-selected notices only. Send delivery failures belong to CMP-SEND's pending rows.
pub struct ComposerBannerStackItem {
    pub id: String,
    pub variant: banner::Variant,
    pub group: ComposerBannerGroup,
    pub icon: Icon,
    pub custom_icon: Option<Node>,
    pub title: NoticeContent,
    pub description: Option<NoticeContent>,
    pub children: Option<Node>,
    pub actions: Option<Node>,
    pub dismiss_label: Option<UiText>,
    pub on_dismiss: Option<Rc<dyn Fn()>>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct StackOptions {
    pub detached: bool,
    pub in_column: bool,
    pub last_attachment: bool,
}

pub fn composer_banner_stack(
    items: impl Fn() -> Vec<Shared<ComposerBannerStackItem>> + 'static,
    options: StackOptions,
) -> impl IntoElement {
    let ordered = computed(move || {
        let items = items();
        [
            ComposerBannerGroup::Environment,
            ComposerBannerGroup::Attention,
        ]
        .into_iter()
        .flat_map(|group| {
            items
                .iter()
                .filter(move |item| item.group == group)
                .cloned()
        })
        .collect::<Vec<_>>()
    });
    let count = computed(move || ordered.with(Vec::len));
    each(
        move || ordered.get(),
        |item| item.id.clone(),
        move |item, index| {
            dynamic(move || {
                let item = item.get();
                let index = index.get();
                let last = index + 1 == count.get();
                untracked(move || {
                    let card = notice_card(item, options.detached, index > 0);
                    // starting: has no NSH meaning; no continuous animation is introduced.
                    let card = view().app_class(tw("transition-opacity duration-150 ease-out motion-reduce:transition-none")).child(card);
                    if options.detached {
                        card.into_element()
                    } else {
                        banner::attachment(
                            banner::AttachmentProps {
                                in_column: options.in_column,
                                last: options.last_attachment && last,
                            },
                            card,
                        )
                        .into_element()
                    }
                })
            })
        },
    )
}

fn notice_card(
    item: Shared<ComposerBannerStackItem>,
    detached: bool,
    joined: bool,
) -> impl IntoElement {
    let width = signal(0.0_f32);
    let item_id = SharedString::from(item.id.clone());
    view().test_id(&item_id)
        .on_layout(move |bounds: &Bounds<Points>| width.set((bounds.size.width.0 - 8.0).max(0.0)))
        .child(banner::root(banner::Props {
            surface: banner::SurfaceProps { variant: item.variant, detached, joined: joined && !detached },
            density: banner::Density::Comfortable,
            role: if matches!(item.variant, banner::Variant::Error | banner::Variant::Warning) { AccessibilityRole::Alert } else { AccessibilityRole::None },
            ..Default::default()
        }, move |context| {
            let row_item = item.clone();
            view().child(banner::row(context, banner::Layout::WrapActions, move |row| {
                let icon = if let Some(custom) = &row_item.custom_icon {
                    banner::icon(row, custom()).into_element()
                } else { banner::lucide_icon(row, row_item.icon).into_element() };
                let title = row_item.title.clone();
                let description = row_item.description.clone();
                let title_label = details_label(&title, true);
                let description_label = details_label(&title, false);
                let content = view().app_class(tw("flex min-w-0 grow items-center gap-1 whitespace-nowrap"))
                    .child(notice_text(title, NoticeTextStyle::Title, width, title_label))
                    .child(description.map(|description| notice_text(description, NoticeTextStyle::Description, width, description_label)));
                let has_actions = row_item.actions.is_some() || row_item.on_dismiss.is_some();
                let actions = has_actions.then(|| {
                    let dismissal = row_item.on_dismiss.clone().map(|dismiss| banner::dismiss(
                        row_item.dismiss_label.clone().unwrap_or_else(|| "Dismiss warning".into()), || false, move || dismiss(),
                    ));
                    banner::actions(row, view().app_class(tw("flex min-w-14 items-center justify-end gap-1 sm:min-w-12"))
                        .child(row_item.actions.as_ref().map(|actions| actions()))
                        .child(dismissal)).into_element()
                });
                banner::RowSlots {
                    icon: Some(view().app_class(tw("self-start h-7 sm:h-6")).child(icon).into_element()),
                    content: Some(banner::content(row, content).into_element()),
                    actions,
                }
            })).child(item.children.as_ref().map(|children| banner::children(context, |_| children())))
        }))
}

fn details_label(title: &NoticeContent, full_title: bool) -> UiText {
    if title.rich.is_some() {
        return if full_title {
            "Show title details".into()
        } else {
            "Show notice details".into()
        };
    }
    let title = title.text.resolve();
    UiText::content(if full_title {
        tr(
            "Show full title: {title}",
            &[("title", Value::from(title.to_string()))],
        )
    } else {
        tr(
            "Show details: {title}",
            &[("title", Value::from(title.to_string()))],
        )
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeTextStyle {
    Title,
    Description,
}

#[derive(Default)]
struct Overflow {
    content: Option<SharedString>,
    shown_at: Option<f32>,
}

fn details_at_width(
    shown_at: Option<f32>,
    overflowing: bool,
    width: f32,
    in_banner: bool,
) -> Option<f32> {
    if overflowing {
        Some(shown_at.unwrap_or(width))
    } else if in_banner && shown_at.is_some_and(|shown| width <= shown) {
        shown_at
    } else {
        None
    }
}

fn collapse_whitespace(value: &str) -> String {
    let mut out = String::new();
    let mut space = false;
    for ch in value.chars() {
        if matches!(ch, ' ' | '\t' | '\n' | '\r' | '\u{c}') {
            space = !out.is_empty();
        } else {
            if space {
                out.push(' ');
            }
            out.push(ch);
            space = false;
        }
    }
    out
}

pub fn notice_text(
    content: NoticeContent,
    style: NoticeTextStyle,
    banner_width: Signal<f32>,
    details_label: UiText,
) -> impl IntoElement {
    let width = signal(0.0_f32);
    let details_width = signal(0.0_f32);
    let value = content.text.clone();
    let resolved = computed(move || value.resolve());
    let single_line = computed(move || collapse_whitespace(&resolved.get()).into());
    let measure = text_measurer();
    let natural_width = computed(move || {
        let current_tokens = tokens();
        let family = current_tokens
            .font(nsh_core::element::FONT_SANS)
            .map_or(crate::theme::INTER.family, |value| value.as_ref());
        let font = format!(
            "{} 12px {}",
            if style == NoticeTextStyle::Title {
                500
            } else {
                400
            },
            family
        );
        single_line.with(|value: &SharedString| measure(&font, value).0)
    });
    let overflow = stored(Overflow::default());
    let show_details = computed(move || {
        let content = resolved.get();
        let container = banner_width.get();
        let hidden = style == NoticeTextStyle::Description && container > 0.0 && container < 400.0;
        let recovering = overflow.with(|state| state.shown_at.is_some());
        let recovered = if recovering && details_width.get() > 0.0 {
            details_width.get() + 4.0
        } else {
            0.0
        };
        let overflowing = hidden || natural_width.get() > width.get() + recovered + 0.001;
        overflow.with(|state| {
            if state.content.as_ref() != Some(&content) {
                state.content = Some(content);
                state.shown_at = None;
            }
            state.shown_at = details_at_width(
                state.shown_at,
                overflowing,
                if container > 0.0 {
                    container
                } else {
                    width.get()
                },
                container > 0.0,
            );
            state.shown_at.is_some()
        })
    });
    // NSH has no DOM Range/descendant traversal. Plain text uses its native font measurer;
    // rich slots measure their supplied textual equivalent, not positioned touch-hit layers.
    let line = if content.rich.is_some() {
        view().child(content.element()).into_element()
    } else {
        text(move || single_line.get())
            .number_of_lines(1_u32)
            .into_element()
    };
    // A native flex view replaces the source contents fragment so the details button shares layout.
    view()
        // Native flex defaults to shrink=0; the source spans inherit CSS shrink=1.
        .flex_shrink(1.0)
        .app_class(move || {
            Some(tw(
                if style == NoticeTextStyle::Title
                    || (banner_width.get() > 0.0 && banner_width.get() < 400.0)
                {
                    "flex min-w-0 items-center gap-1"
                } else {
                    "flex w-0 min-w-0 grow items-center gap-1"
                },
            ))
        })
        .child(
            view()
                .flex_shrink(1.0)
                .app_class(move || {
                    Some(tw(&cn([
                        Some(if style == NoticeTextStyle::Title {
                            "min-w-0 truncate font-medium leading-7 sm:leading-6"
                        } else {
                            "min-w-0 grow truncate text-muted-foreground"
                        }),
                        (style == NoticeTextStyle::Description
                            && banner_width.get() > 0.0
                            && banner_width.get() < 400.0)
                            .then_some("hidden"),
                    ])))
                })
                .on_layout(move |bounds: &Bounds<Points>| width.set(bounds.size.width.0))
                .child(line),
        )
        .child(dynamic(move || {
            show_details.get().then(|| {
                untracked(|| {
                    let handle = popover_create_handle();
                    effect(move || println!("DETAIL OPEN {}", handle.is_open()));
                    let pending = stored(None::<Spawned<()>>);
                    let stay = move || {
                        if let Some(task) = pending.replace(None) {
                            task.cancel();
                        }
                    };
                    let leave = move || {
                        stay();
                        pending.replace(Some(set_timeout(Duration::from_millis(300), move || {
                            handle.set_open(false)
                        })));
                    };
                    let popup_content = content.clone();
                    let trigger = button(
                        ButtonContent::Icon {
                            icon: Icon::Info,
                            label: details_label.resolve().to_string(),
                        },
                        ButtonVariant::GhostMuted,
                        ButtonSize::IconXs,
                        || false,
                        move || { println!("DETAIL PRESS before {}",handle.is_open()); handle.set_open(!handle.is_open()); println!("DETAIL PRESS after {}",handle.is_open()); },
                    );
                    // The details region owns and cancels its timers when removed.
                    view()
                        .app_class(tw("flex-none"))
                        .on_layout(move |bounds: &Bounds<Points>| {
                            details_width.set(bounds.size.width.0)
                        })
                        .on_mouse_enter(move |_: &MouseEvent| {
                            println!("DETAIL ENTER"); stay();
                            handle.set_open(true);
                        })
                        .on_mouse_leave(move |_: &MouseEvent| leave())
                        .child(popover(
                            handle,
                            PopoverOptions {
                                tooltip_style: true,
                                side: Placement::Top,
                                ..Default::default()
                            },
                            trigger,
                            move || {
                                view()
                                    .test_id("notice-details-content")
                                    // The primitive registers its popup name through the title slot.
                                    .child(
                                        view()
                                            .app_class(tw("hidden"))
                                            .child(popover_title("Notice details")),
                                    )
                                    .max_w(move || {
                                        px(480.0_f32.min((window().width.0 - 32.0).max(0.0)))
                                    })
                                    .on_mouse_enter(move |_: &MouseEvent| stay())
                                    .on_mouse_leave(move |_: &MouseEvent| leave())
                                    .child(banner::scroll(popup_content.element()))
                                    .into_element()
                            },
                        ))
                        .into_element()
                })
            })
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn details_remain_until_the_banner_grows_but_content_changes_reset_them() {
        assert_eq!(details_at_width(None, true, 300.0, true), Some(300.0));
        assert_eq!(
            details_at_width(Some(300.0), false, 300.0, true),
            Some(300.0)
        );
        assert_eq!(
            details_at_width(Some(300.0), false, 299.0, true),
            Some(300.0)
        );
        assert_eq!(details_at_width(Some(300.0), false, 301.0, true), None);
        assert_eq!(details_at_width(Some(300.0), false, 300.0, false), None);
        assert_eq!(details_at_width(None, false, 300.0, true), None);
    }
}
