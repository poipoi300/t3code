//! Port of apps/web/src/components/ui/wizard.tsx WizardPopup, WizardHeader, WizardFooter,
//! WizardSteps, WizardPanel; standalone-page.tsx StandalonePage, StandalonePageHeader; and
//! discovery-list.tsx DiscoveryList, DiscoveryListRow.
//!
//! A wizard is a `ui::dialog` root: `dialog(open, |state| wizard_popup(state, WizardSize::..,
//! PopupOptions::default(), move || vec![wizard_header(..), wizard_panel(..), wizard_footer(..)]))`.
//! Every slot takes the root's `DialogState`, as the dialog slots do.
//!
//! Departures, each forced by NSH:
//! - WizardPanel's AnimatedHeight: NSH has no height transition (its transition groups are
//!   colour, opacity, transform, shadow and shape), so a step change resizes at once. `hold_height`
//!   keeps the last measured height and clips, as AnimatedHeight does while it holds.
//! - WizardPopup's scrollbar `my-4` inset: the native scrollbar has no margin setting.
//! - WizardSteps' `grid auto-cols-fr grid-flow-col` is equal flex columns (`flex-1 basis-0`).
//! - StandalonePage's `items-center-safe`: the scroll view's content is `min-h-full` and centres
//!   its card, which is safe centring (an oversized card starts at the top). `scrollbar-gutter-both`
//!   has no NSH meaning and is dropped.
//! - DiscoveryList's `divide-y divide-border/60` is a 1px rule between rows.

use std::rc::Rc;

use nsh_core::element::{AnyElement, ImageSrc, IntoChild};
use nsh_core::prelude::*;

use super::dialog::{
    DialogState, FooterJustify, FooterVariant, MaxWidth, PopupOptions, dialog_description,
    dialog_footer_justified, dialog_header, dialog_popup, dialog_title,
};
use super::icon::{Icon, icon_with_class};
use super::scroll_area::scroll_area;
use crate::i18n::{Text, Value, t, tr, ts};
use crate::theme::{cn, tw};

/// The TSX `size` prop of WizardPopup.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WizardSize {
    #[default]
    Default,
    Wide,
}

/// `WizardPopup`: the dialog popup at `max-w-xl` (`max-w-3xl` wide), its content in a scroll area.
pub fn wizard_popup<M: 'static, E: IntoChild<M> + 'static>(
    state: DialogState,
    size: WizardSize,
    options: PopupOptions,
    children: impl Fn() -> E + 'static,
) -> impl IntoElement {
    let max_width = match size {
        WizardSize::Default => MaxWidth::Xl,
        WizardSize::Wide => MaxWidth::ThreeXl,
    };
    dialog_popup(
        state,
        PopupOptions {
            max_width,
            ..options
        },
        move || {
            scroll_area(
                false,
                view()
                    .app_class(tw("flex min-h-0 flex-col"))
                    .child(children()),
            )
        },
    )
}

/// `WizardHeader`. With an `identity` the title is not drawn (`sr-only`) but still names the
/// dialog; the identity element takes its place.
pub fn wizard_header(
    state: DialogState,
    title: impl Fn() -> Text + 'static,
    description: Option<Rc<dyn Fn() -> Text>>,
    identity: Option<AnyElement>,
    children: Option<AnyElement>,
) -> impl IntoElement {
    let title = Rc::new(title);
    let title_source = move || title().resolve().to_string();
    let has_identity = identity.is_some();
    let mut slots = vec![if has_identity {
        // sr-only: no visible element; the registered title still names the dialog.
        view()
            .app_class(tw("hidden"))
            .child(dialog_title(state, title_source))
            .into_element()
    } else {
        dialog_title(state, title_source).into_element()
    }];
    slots.extend(identity);
    slots.extend(description.map(|description| {
        dialog_description(state, move || description().resolve().to_string()).into_element()
    }));
    slots.extend(children);
    dialog_header(state, slots)
}

/// `WizardFooter`. A `leading` element sits at the start and the actions group at the end.
pub fn wizard_footer<M: 'static>(
    state: DialogState,
    leading: Option<AnyElement>,
    children: impl IntoChild<M> + 'static,
) -> impl IntoElement {
    match leading {
        Some(leading) => dialog_footer_justified(
            state,
            FooterVariant::Bare,
            FooterJustify::Between,
            vec![
                leading,
                view()
                    .app_class(tw("flex flex-col-reverse gap-2 sm:flex-row"))
                    .child(children)
                    .into_element(),
            ],
        )
        .into_element(),
        None => dialog_footer_justified(state, FooterVariant::Bare, FooterJustify::End, children)
            .into_element(),
    }
}

/// The optional props of `WizardSteps`.
#[derive(Clone, Default)]
pub struct WizardStepsOptions {
    /// Content shown after a completed step's name, such as the chosen machine.
    pub summaries: Option<Reactive<Vec<Option<SharedString>>>>,
    pub show_summaries: bool,
    pub is_step_disabled: Option<Rc<dyn Fn(usize) -> bool>>,
    /// With it each step is a button; without it the steps are plain items.
    pub on_step_change: Option<Rc<dyn Fn(usize)>>,
}

/// `WizardSteps`: the progress list. Steps are names (keys or content).
pub fn wizard_steps(
    steps: impl IntoReactive<Vec<Text>>,
    current_step: impl IntoReactive<usize>,
    options: WizardStepsOptions,
) -> impl IntoElement {
    let steps = steps.into_reactive();
    let current_step = current_step.into_reactive();
    let current = computed(move || current_step.get());
    let summaries = options.summaries.clone();
    let summary = Rc::new(move |index: usize| -> Option<SharedString> {
        summaries
            .as_ref()
            .and_then(|summaries| summaries.get().get(index).cloned().flatten())
    });
    view()
        .role(AccessibilityRole::List)
        .label(ts("Setup progress"))
        .app_class(tw("flex gap-1 rounded-xl bg-background p-1 ring-1 ring-black/5 dark:bg-white/4 dark:ring-white/5"))
        .child(dynamic(move || {
            let steps = steps.get();
            let options = options.clone();
            let summary = summary.clone();
            untracked(move || {
                steps
                    .into_iter()
                    .enumerate()
                    .map(|(index, step)| {
                        // aria-current sits on the item: NSH's Button takes no accessibility state.
                        view()
                            .test_id(&format!("wizard-step-{index}"))
                            .accessibility_state(move || AccessibilityState {
                                current: (current.get() == index)
                                    .then_some(nsh_core::AccessibilityCurrent::Step),
                                ..Default::default()
                            })
                            .app_class(tw("min-w-0 flex-1 basis-0"))
                            .child(wizard_step(index, step, current, summary.clone(), &options))
                            .into_element()
                    })
                    .collect::<Vec<_>>()
            })
        }))
}

fn wizard_step(
    index: usize,
    step: Text,
    current: Memo<usize>,
    summary: Rc<dyn Fn(usize) -> Option<SharedString>>,
    options: &WizardStepsOptions,
) -> AnyElement {
    let interactive = options.on_step_change.is_some();
    let show_summaries = options.show_summaries;
    let step_name = step.clone();
    let label_summary = summary.clone();
    let label = move || {
        let number = Value::Int(index as i64 + 1);
        let name = Value::Text(step_name.resolve().to_string());
        match label_summary(index).filter(|_| index < current.get()) {
            Some(summary) => tr(
                "{step}, step {number}, {summary}",
                &[
                    ("step", name),
                    ("number", number),
                    ("summary", Value::Text(summary.to_string())),
                ],
            ),
            None => tr(
                "{step}, step {number}",
                &[("step", name), ("number", number)],
            ),
        }
    };
    let class = move || {
        let state = current.get().cmp(&index);
        tw(&cn([
            Some("flex w-full min-w-0 items-center gap-2 rounded-lg px-2.5 py-2 text-left outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring max-sm:justify-center max-sm:px-2"),
            interactive.then_some("cursor-pointer hover:bg-card disabled:cursor-default disabled:hover:bg-transparent"),
            (state == std::cmp::Ordering::Equal).then_some("bg-card text-foreground shadow-xs ring-1 ring-inset ring-black/5 hover:bg-card dark:shadow-none dark:ring-white/5"),
        ]))
    };
    let badge = view()
        .accessible(false)
        .app_class(move || {
            let at = current.get();
            tw(&cn([
                Some("flex size-5 shrink-0 items-center justify-center rounded-full text-sm font-medium ring-1"),
                Some(if index < at {
                    "bg-primary text-primary-foreground ring-primary"
                } else if index == at {
                    "bg-primary/10 text-primary ring-primary/30"
                } else {
                    "bg-card text-muted-foreground ring-black/10 dark:bg-white/5 dark:ring-white/10"
                }),
            ]))
        })
        .child(dynamic(move || {
            if index < current.get() {
                icon_with_class(Icon::Check, "size-4 shrink-0").into_element()
            } else {
                text((index + 1).to_string()).into_element()
            }
        }));
    let name = text(move || {
        let name = step.resolve();
        match summary(index).filter(|_| show_summaries && index < current.get()) {
            Some(summary) => SharedString::from(tr(
                "{step}: {summary}",
                &[
                    ("step", Value::Text(name.to_string())),
                    ("summary", Value::Text(summary.to_string())),
                ],
            )),
            None => name,
        }
    })
    .accessible(false)
    .app_class(move || {
        tw(&cn([
            Some("min-w-0 truncate text-sm font-medium max-sm:hidden"),
            Some(if index == current.get() {
                "text-foreground"
            } else {
                "text-muted-foreground"
            }),
        ]))
    });
    match options.on_step_change.clone() {
        Some(on_step_change) => {
            let disabled = options.is_step_disabled.clone();
            let is_disabled = move || disabled.as_ref().is_some_and(|disabled| disabled(index));
            let guard = is_disabled.clone();
            nsh_base::button()
                .accessibility_label(move || Some(SharedString::from(label())))
                .selected(move || current.get() == index)
                .disabled(is_disabled)
                .on_press(move || {
                    if !guard() {
                        on_step_change(index);
                    }
                })
                .app_class(class)
                .child(badge)
                .child(name)
                .into_element()
        }
        None => view()
            .label(label)
            .app_class(class)
            .child(badge)
            .child(name)
            .into_element(),
    }
}

/// `WizardPanel`. Registers as the popup's `dialog-panel` so the header takes its `pb-3`.
pub fn wizard_panel<M: 'static>(
    state: DialogState,
    hold_height: impl IntoReactive<bool>,
    min_h_72: impl IntoReactive<bool>,
    children: impl IntoChild<M> + 'static,
) -> impl IntoElement {
    state.register_panel();
    let hold_height = hold_height.into_reactive();
    let min_h_72 = min_h_72.into_reactive();
    let holding = computed(move || hold_height.get());
    let measured = signal(None::<f32>);
    view()
        .test_id("wizard-panel")
        .app_class(move || {
            tw(&cn([
                Some("min-w-0 space-y-4 bg-background/80 px-6 py-5 ring-1 ring-black/5 dark:bg-white/2 dark:ring-white/5"),
                min_h_72.get().then_some("min-h-72"),
            ]))
        })
        .child(animated_height(holding, measured, children))
}

/// Port of components/AnimatedHeight.tsx AnimatedHeight, without the height tween (NSH has none).
fn animated_height<M: 'static>(
    holding: Memo<bool>,
    measured: Signal<Option<f32>>,
    children: impl IntoChild<M> + 'static,
) -> impl IntoElement {
    let held = computed(move || holding.get().then(|| measured.get_untracked()).flatten());
    view()
        .test_id("animated-height")
        .app_class(move || {
            tw(if held.get().is_some() {
                "overflow-hidden"
            } else {
                ""
            })
        })
        .h(move || held.get().map_or_else(auto, |height| px(height).into()))
        .child(
            view()
                .on_layout(move |bounds: &Bounds<Points>| {
                    if !holding.get_untracked() {
                        measured.set(Some(bounds.size.height.0.ceil()));
                    }
                })
                .app_class(move || tw(if held.get().is_some() { "h-full" } else { "" }))
                .child(children),
        )
}

/// `StandalonePage`: a full-height scrolling page holding one glass card.
pub fn standalone_page<M: 'static>(children: impl IntoChild<M> + 'static) -> impl IntoElement {
    scroll()
        .app_class(tw("h-full bg-background text-foreground"))
        .child(
            view()
                .app_class(tw("flex min-h-full items-center justify-center px-4 py-10 sm:px-6"))
                .child(
                    // index.css @utility dialog-glass, as ui::dialog's popup expands it.
                    view()
                        .app_class(tw("w-full max-w-xl rounded-2xl border p-6 bg-background/80 backdrop-blur-[12px] dark:backdrop-blur-[16px] backdrop-saturate-[1.14] dark:backdrop-saturate-[1.08] border-contrast-foreground/10 dark:border-white/8 shadow-[0_24px_64px_-24px_rgb(0_0_0/65%)] dark:shadow-[inset_0_1px_rgb(255_255_255/4%),0_24px_72px_-20px_rgb(0_0_0/90%)]"))
                        .child(children),
                ),
        )
}

/// `StandalonePageHeader`: the T3 Code mark with an optional context, the title and description.
pub fn standalone_page_header(
    context: Option<Text>,
    title: impl Fn() -> Text + 'static,
    description: impl Fn() -> Text + 'static,
) -> Vec<AnyElement> {
    vec![
        view()
            .app_class(tw(
                "flex items-baseline justify-between gap-3 text-muted-foreground",
            ))
            .child(
                view()
                    .role(AccessibilityRole::Image)
                    .label(t("T3 Code"))
                    .app_class(tw("flex items-baseline gap-1.5"))
                    // T3Wordmark at h-3 w-auto: 12px tall at its viewBox's aspect ratio.
                    .child(
                        svg(ImageSrc::svg_markup(T3_WORDMARK))
                            .accessible(false)
                            .app_class(tw("shrink-0 text-foreground"))
                            .h(px(12.0))
                            .w(px(12.0 * 94.3941 / 56.96)),
                    )
                    .child(
                        text(t("Code"))
                            .accessible(false)
                            .app_class(tw("text-lg font-medium tracking-tight")),
                    ),
            )
            .child(context.map(|context| text(context.resolve()).app_class(tw("text-sm"))))
            .into_element(),
        text(move || title().resolve())
            .role(AccessibilityRole::Header)
            .accessibility_level(1u32)
            .app_class(tw("mt-6 text-xl leading-tight font-semibold"))
            .into_element(),
        text(move || description().resolve())
            .app_class(tw("mt-2 text-sm text-muted-foreground"))
            .into_element(),
    ]
}

/// `DiscoveryList`: a bordered list of rows with a rule between them.
pub fn discovery_list(rows: Vec<AnyElement>) -> impl IntoElement {
    let mut children = Vec::with_capacity(rows.len() * 2);
    for (index, row) in rows.into_iter().enumerate() {
        if index > 0 {
            children.push(
                view()
                    .app_class(tw("h-px shrink-0 bg-border/60"))
                    .into_element(),
            );
        }
        children.push(row);
    }
    view()
        .app_class(tw(
            "flex flex-col overflow-hidden rounded-xl border border-border/70 bg-background",
        ))
        .child(children)
}

/// `DiscoveryListRow`: a full-width button with an icon, a title, a description and an action.
pub fn discovery_list_row(
    icon: AnyElement,
    title: Text,
    description: Text,
    action: Option<AnyElement>,
    disabled: impl IntoReactive<bool>,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    let disabled = disabled.into_reactive();
    let blocked = computed(move || disabled.get());
    nsh_base::button()
        .disabled(move || blocked.get())
        .on_press(move || {
            if !blocked.get_untracked() {
                on_press();
            }
        })
        .app_class(tw("group flex w-full items-center gap-3 px-3 py-3 text-left hover:bg-accent/40 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-64"))
        .child(icon)
        .child(
            view()
                .app_class(tw("flex min-w-0 flex-1 flex-col"))
                .child(text(title.resolve()).app_class(tw("truncate text-sm font-medium text-foreground")))
                .child(text(description.resolve()).app_class(tw("truncate text-xs text-muted-foreground"))),
        )
        .child(action)
}

/// components/T3Wordmark.tsx (the same glyph as the sidebar's brand mark).
const T3_WORDMARK: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="15.5309 37 94.3941 56.96"><path d="M33.4509 93V47.56H15.5309V37H64.3309V47.56H46.4109V93H33.4509ZM86.7253 93.96C82.832 93.96 78.9653 93.4533 75.1253 92.44C71.2853 91.3733 68.032 89.88 65.3653 87.96L70.4053 78.04C72.5386 79.5867 75.0186 80.8133 77.8453 81.72C80.672 82.6267 83.5253 83.08 86.4053 83.08C89.6586 83.08 92.2186 82.44 94.0853 81.16C95.952 79.88 96.8853 78.12 96.8853 75.88C96.8853 73.7467 96.0586 72.0667 94.4053 70.84C92.752 69.6133 90.0853 69 86.4053 69H80.4853V60.44L96.0853 42.76L97.5253 47.4H68.1653V37H107.365V45.4L91.8453 63.08L85.2853 59.32H89.0453C95.9253 59.32 101.125 60.8667 104.645 63.96C108.165 67.0533 109.925 71.0267 109.925 75.88C109.925 79.0267 109.099 81.9867 107.445 84.76C105.792 87.48 103.259 89.6933 99.8453 91.4C96.432 93.1067 92.0586 93.96 86.7253 93.96Z" fill="currentColor"/></svg>"##;
