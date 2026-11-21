//! Port of AppearanceSettingsPanel.tsx, including its theme, interface and typography groups.

use nsh_core::LayoutCurve;
use nsh_core::element::LayoutTransition;
use nsh_core::prelude::*;
use serde_json::{Value, json};
use t3_client::services::settings::{
    panels::default_unified_settings, search::is_advanced_typography_target,
};
use t3_client::workspace::git_panel::details::{
    DETAILS_ITEMS, is_default_thread_details_visibility, resolve_thread_details_visibility,
    set_thread_details_item,
};
use t3_theme::fonts::*;
use t3_theme::palette::{AppearanceMode, built_in_themes};

use super::setting_controls::{choice_row, reset, switch_row};
use super::shell::{
    RowReset, SettingsContext, SettingsRowProps, advanced_settings, region,
    settings_page_container, settings_row, settings_section, use_settings,
};
use crate::i18n::{Text, t, tr};
use crate::theme::{ThemeChoice, palette, tw};
use crate::ui::icon::Icon;
use crate::ui::slider::{SliderProps, slider};
use crate::ui::switch::{ControlState, Size, switch};

const THEME_KEY: &str = "t3code:theme";

pub fn screen() -> AnyElement {
    let context = use_settings();
    settings_page_container(
        view()
            .app_class(tw("flex flex-col gap-8"))
            .child(theme_library(context))
            .child(settings_section(
                "appearance-interface",
                Text::from("Interface"),
                view()
                    .child(settings_row(slider_row(context, Range {
                        id: "setting-appearance-contrast",
                        key: "appearanceContrast",
                        label: "Contrast",
                        reset: "contrast",
                        description: "Adjust the contrast of colors and borders across the interface.",
                        bounds: (50, 200, 5),
                        unit: "%",
                    }).first()))
                    .child(settings_row(slider_row(context, Range {
                        id: "setting-glass-opacity",
                        key: "glassOpacity",
                        label: "Glass opacity",
                        reset: "glass opacity",
                        description: "Higher values make menus, dialogs, and the composer more solid.",
                        bounds: (40, 100, 5),
                        unit: "%",
                    })))
                    .child(settings_row(choice_row(
                        "chat-width",
                        "chatWidth",
                        "chat width",
                        "Chat width",
                        "Set how wide messages and the composer can grow on large screens.",
                        &[("comfortable", "Comfortable (default)"), ("wide", "Wide"), ("full", "Full")],
                    )))
                    .child(thread_details_row(context))
                    .child(settings_row(choice_row(
                        "effort-display",
                        "effortDisplay",
                        "effort display",
                        "Effort display",
                        "Show reasoning effort in the composer as its name, such as High, or as bars.",
                        &[("label", "Word (default)"), ("bars", "Bars")],
                    )))
                    .child(settings_row(switch_row(
                        "math-rendering",
                        "mathRenderingEnabled",
                        "math rendering",
                        "Render math",
                        r"Typeset LaTeX math, such as $x^2$ or \[x^2\], in messages and Markdown previews. The composer keeps TeX source editable.",
                    )))
                    .child(advanced_settings(
                        &["panel-animations", "environment-identification", "appearance-interface", "composer-context"],
                        view()
                            .child(settings_row(slider_row(context, Range {
                                id: "panel-animations",
                                key: "panelAnimationDurationMs",
                                label: "Panel animation duration",
                                reset: "panel animations",
                                description: "Set how fast panels open and close.",
                                bounds: (0, 400, 25),
                                unit: " ms",
                            }).first()))
                            .child(settings_row(choice_row(
                                "environment-identification",
                                "environmentIdentificationMode",
                                "environment identification",
                                "Environment identification",
                                "Show Dev or Nightly artwork in the sidebar header.",
                                &[("artwork", "None"), ("nightly-artwork", "Nightly artwork"), ("dev-artwork", "Dev artwork")],
                            )))
                            .child(settings_row(switch_row(
                                "composer-context",
                                "persistComposerContextStrip",
                                "composer context",
                                "Keep composer context visible in active threads",
                                "Keep branch and worktree controls below the composer after a thread starts.",
                            ))),
                    )),
            ))
            .child(typography(context)),
    )
    .into_element()
}

#[derive(Clone, Copy)]
struct Range {
    id: &'static str,
    key: &'static str,
    label: &'static str,
    reset: &'static str,
    description: &'static str,
    bounds: (u32, u32, u32),
    unit: &'static str,
}

fn slider_row(context: SettingsContext, range: Range) -> SettingsRowProps {
    let Range {
        id,
        key,
        label,
        reset: name,
        description,
        bounds: (min, max, step),
        unit,
    } = range;
    SettingsRowProps::searchable(id)
        .description(move || Text::from(description))
        .reset(reset(context, key, name))
        .control(move || {
            let value = computed(move || context.setting(key).as_u64().unwrap_or_default() as u32);
            let mut props = SliderProps::new(label, Prop::bind(move || f64::from(value.get())));
            props.min = f64::from(min).into();
            props.max = f64::from(max).into();
            props.step = f64::from(step).into();
            let control = view()
                .app_class(tw("flex w-full flex-row items-center gap-3 "))
                .child(
                    text(move || format!("{}{unit}", crate::i18n::formats().decimal(f64::from(value.get()))))
                        .app_class(tw("min-w-12 rounded-md bg-muted px-2 py-1 text-center font-mono text-xs font-medium tabular-nums text-foreground")),
                )
                .child(slider(
                    props,
                    move |value| {
                        if value.fract() == 0.0 && (f64::from(min)..=f64::from(max)).contains(&value) {
                            context.update(json!({key: value as u32}));
                        }
                    },
                    |_| {},
                ));
            if key == "panelAnimationDurationMs" {
                view()
                    .app_class(tw("grid w-full grid-cols-[5rem_minmax(0,1fr)] items-center gap-3 sm:w-auto sm:grid-cols-[7rem_13rem] sm:gap-4"))
                    .child(panel_animations_preview(value))
                    .child(control)
                    .into_element()
            } else {
                view().app_class(tw("w-full sm:w-52")).child(control).into_element()
            }
        })
}

/// Port of PanelAnimationsPreview.tsx. NSH animates the resulting layout rather than CSS width.
fn panel_animations_preview(duration: Memo<u32>) -> impl IntoElement {
    let open = signal(true);
    let region = element_ref();
    nsh_base::button()
        .accessibility_label(t("Replay panel animation preview"))
        .on_press(move || {
            let duration = duration.get_untracked();
            if !reduce_motion() && duration != 0 {
                region.animate_layout(LayoutTransition::new(LayoutCurve::EaseOut, duration as f32));
            }
            open.update(|open| *open = !*open);
        })
        .app_class(tw("flex h-10 w-full cursor-pointer flex-row overflow-hidden rounded-lg border border-border bg-background p-1 shadow-xs/5 outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-background"))
        .child(view().element_ref(&region).accessible(false).app_class(tw("flex h-full w-full flex-row"))
            .child(view().app_class(tw("h-full shrink-0 overflow-hidden rounded-md bg-sidebar"))
                .w(move || px(if open.get() { 16.0 } else { 0.0 })))
            .child(view().app_class(tw("flex min-w-0 flex-1 flex-col px-1"))
                .child(view().app_class(tw("flex min-h-0 flex-1 flex-col gap-1 pt-1"))
                    .child(view().app_class(tw("h-0.5 w-full rounded-full bg-muted-foreground/25")))
                    .child(view().app_class(tw("h-0.5 w-4/5 rounded-full bg-muted-foreground/20")))
                    .child(view().app_class(tw("h-0.5 w-3/5 rounded-full bg-muted-foreground/15"))))
                .child(view().app_class(move || Some(tw(if open.get() {
                    "flex h-2 shrink-0 items-center overflow-hidden border-t border-border/70 bg-foreground/5 px-2"
                } else {
                    "flex h-0 shrink-0 items-center overflow-hidden bg-foreground/5 px-2"
                }))).child(view().app_class(tw("h-px w-2/3 rounded-full bg-muted-foreground/25")))))
            .child(view().app_class(tw("h-full shrink-0 overflow-hidden rounded-md bg-muted"))
                .w(move || px(if open.get() { 20.0 } else { 0.0 }))))
}

fn heading(title: &'static str) -> impl IntoElement {
    text(Text::from(title).resolve())
        .role(AccessibilityRole::Header)
        .app_class(tw(
            "flex min-h-7 items-center px-3 text-base font-semibold text-foreground sm:px-4",
        ))
}

fn stored_preference(
    context: SettingsContext,
    key: &'static str,
    fallback: Value,
) -> Signal<Value> {
    let preference = signal(fallback);
    if let Some(load) = context.read_device_preference(key) {
        crate::runtime::tasks::watch(async move {
            if let Some(value) = load.await {
                let _ = preference.try_set(value);
            }
        });
    }
    preference
}

fn choose(context: SettingsContext, state: Signal<Value>, key: &'static str, value: Value) {
    if state.get_untracked() != value && context.available() {
        state.set(value.clone());
        context.write_device_preference(key, value);
    }
}

fn theme_library(context: SettingsContext) -> impl IntoElement {
    use t3_theme::palette::THEME_APPEARANCE_MODE_STORAGE_KEY as MODE_KEY;
    let mode = stored_preference(context, MODE_KEY, json!("system"));
    let theme = stored_preference(context, THEME_KEY, json!("system"));
    let choice = computed(move || ThemeChoice {
        theme: theme.with(|value| value.as_str().unwrap_or_default().to_owned()),
        mode: mode.with(|value| {
            value
                .as_str()
                .and_then(AppearanceMode::parse)
                .unwrap_or(AppearanceMode::System)
        }),
        contrast: context.client_settings().appearance_contrast as u32,
        ..ThemeChoice::default()
    });
    effect(move || {
        let selection = choice.get();
        untracked(|| crate::theme::install(&selection));
    });

    let modes = [
        ("system", "System", "Follow the system appearance"),
        ("light", "Light", "Use light mode"),
        ("dark", "Dark", "Use dark mode"),
    ].into_iter().map(|(key, label, accessible)| {
        nsh_base::button()
            .accessibility_label(Text::from(accessible).resolve())
            .selected(move || mode.with(|value| value.as_str() == Some(key)))
            .on_press(move || choose(context, mode, MODE_KEY, json!(key)))
            .app_class(move || Some(tw(if mode.with(|value| value.as_str() == Some(key)) {
                "flex flex-col items-stretch gap-1.5 rounded-xl border border-transparent bg-accent/30 p-2 ring-2 ring-inset ring-ring"
            } else {
                "flex flex-col items-stretch gap-1.5 rounded-xl border border-border/70 bg-card/60 p-2 hover:bg-accent/10"
            })))
            .child(view().app_class(tw("flex h-[8.75rem] flex-row overflow-hidden rounded-lg"))
                .child((key != "dark").then(|| theme_wireframe(choice, false)))
                .child((key != "light").then(|| theme_wireframe(choice, true))))
            .child(text(Text::from(label).resolve()).app_class(tw("text-center text-xs font-medium")))
            .into_element()
    }).collect::<Vec<_>>();
    let cards = std::iter::once(("system", "T3 Code", false))
        .chain(built_in_themes().iter().map(|theme| (theme.id.as_str(), theme.label.as_str(), true)))
        .map(|(id, name, literal)| {
            let name = if literal { Text::content(name) } else { Text::from("T3 Code") }.resolve();
            let picked = move || theme.with(|value| value.as_str() == Some(id));
            let tokens = crate::theme::tokens_for(&ThemeChoice { theme: id.to_owned(), ..ThemeChoice::default() });
            let swatches = crate::theme::Palette::of(&tokens);
            nsh_base::button()
                .accessibility_label(name.clone())
                .on_press(move || choose(context, theme, THEME_KEY, json!(id)))
                .app_class(move || Some(tw(if picked() {
                    "flex flex-col gap-2 rounded-xl border border-ring bg-accent/30 p-3"
                } else {
                    "flex flex-col gap-2 rounded-xl border border-border/70 bg-card/60 p-3 hover:bg-accent/10"
                })))
                .child(view().app_class(tw("flex flex-row justify-center gap-8 py-2"))
                    .child(theme_swatch(swatches.clone(), false))
                    .child(theme_swatch(swatches, true)))
                .child(text(name).app_class(tw("text-sm font-medium text-foreground")))
                .into_element()
        })
        .collect::<Vec<_>>();

    view()
        .test_id("appearance")
        .app_class(tw("flex flex-col gap-3"))
        .child(heading("Color scheme"))
        .child(
            view()
                .test_id("color-scheme")
                .app_class(tw("grid w-full grid-cols-3 gap-3"))
                .child(modes),
        )
        .child(view().app_class(tw("pt-5")).child(heading("Themes")))
        .child(
            view()
                .test_id("theme")
                .app_class(move || {
                    Some(tw(match context.content_width.get() {
                        width if width >= 784.0 => "grid w-full grid-cols-3 gap-2",
                        width if width >= 520.0 => "grid w-full grid-cols-2 gap-2",
                        _ => "grid w-full grid-cols-1 gap-2",
                    }))
                })
                .child(cards),
        )
}

fn theme_wireframe(choice: Memo<ThemeChoice>, dark: bool) -> impl IntoElement {
    let colors =
        computed(move || crate::theme::Palette::of(&crate::theme::tokens_for(&choice.get())));
    let half = move |color: AdaptiveColor| if dark { color.dark } else { color.light };
    view()
        .accessible(false)
        .app_class(tw("flex flex-1 flex-row"))
        .bg(move || half(colors.get().background))
        .child(
            view()
                .app_class(tw("w-1/3 border-r p-2"))
                .bg(move || half(colors.get().sidebar))
                .child(
                    view()
                        .app_class(tw("mt-3 h-1.5 w-full rounded-full"))
                        .bg(move || half(colors.get().muted)),
                ),
        )
        .child(
            view()
                .app_class(tw("flex flex-1 flex-col justify-end gap-2 p-2"))
                .child(
                    view()
                        .app_class(tw("h-2 w-3/4 rounded-full"))
                        .bg(move || half(colors.get().muted)),
                )
                .child(
                    view()
                        .app_class(tw("h-2 w-1/2 rounded-full"))
                        .bg(move || half(colors.get().primary)),
                )
                .child(
                    view()
                        .app_class(tw("h-5 w-full rounded-md"))
                        .bg(move || half(colors.get().muted)),
                ),
        )
}

fn theme_swatch(colors: crate::theme::Palette, dark: bool) -> impl IntoElement {
    let half = |color: AdaptiveColor| if dark { color.dark } else { color.light };
    view()
        .accessible(false)
        .app_class(tw(
            "flex size-14 items-center justify-center rounded-full border",
        ))
        .bg(half(colors.background))
        .child(
            view()
                .app_class(tw("size-5 rounded-full"))
                .bg(half(colors.primary)),
        )
}

fn thread_details_row(context: SettingsContext) -> impl IntoElement {
    use crate::ui::menu::{self, CheckboxVariant, ItemOptions, MenuOptions};
    let visibility =
        computed(move || resolve_thread_details_visibility(&context.client_settings()));
    let count = computed(move || visibility.with(|items| items.values().filter(|on| **on).count()));
    settings_row(
        SettingsRowProps::searchable("thread-details-items")
            .description(|| Text::from("Choose what the thread details card shows. You can also right-click the card."))
            .reset(RowReset::new(
                "thread details",
                move || visibility.with(|items| !is_default_thread_details_visibility(items)),
                move || context.update(json!({"threadDetailsItems": {}, "threadDetailsUsageEnabled": false})),
            ))
            .control(move || {
                let count_args = move || [
                    ("enabled", t3_i18n::Value::Float(count.get() as f64)),
                    ("total", t3_i18n::Value::Float(DETAILS_ITEMS.len() as f64)),
                ];
                menu::menu(
                    nsh_base::button()
                        .accessibility_label(move || Some(tr("Thread details: {enabled} of {total} on", &count_args()).into()))
                        .app_class(tw("flex h-8 w-full flex-row items-center justify-between rounded-lg border border-input bg-background px-3 text-sm sm:w-40"))
                        .child(text(move || tr("{enabled} of {total} on", &count_args())))
                        .child(svg(Icon::ChevronDown.asset()).app_class(tw("size-4"))),
                    MenuOptions { align: Align::End, ..Default::default() },
                    menu::menu_popup_with(move || menu::menu_group(
                        t("Show in thread details"),
                        view().child(menu::menu_group_label(t("Show in thread details"), false)).child(
                            DETAILS_ITEMS.iter().map(|&(id, label, _)| {
                                let label = Text::from(label).resolve();
                                menu::menu_checkbox_item(
                                    label.to_string(),
                                    move || visibility.with(|items| items.get(id).copied().unwrap_or(false)),
                                    CheckboxVariant::Default,
                                    ItemOptions { close_on_select: false, ..Default::default() },
                                    text(label),
                                    move |checked| {
                                        let items = set_thread_details_item(&context.client_settings(), id, checked);
                                        context.update(json!({"threadDetailsItems": items}));
                                    },
                                ).into_element()
                            }).collect::<Vec<_>>(),
                        ),
                    )),
                )
            }),
    )
}

fn typography(context: SettingsContext) -> impl IntoElement {
    let preference = stored_preference(context, TYPOGRAPHY_ADVANCED_STORAGE_KEY, json!(false));
    let separate = move || preference.with(|v| v.as_bool() == Some(true));
    let show_advanced =
        computed(move || separate() || is_advanced_typography_target(&context.target_id()));
    effect(move || {
        if is_advanced_typography_target(&context.target_id()) && !separate() {
            preference.set(json!(true));
            context.write_device_preference(TYPOGRAPHY_ADVANCED_STORAGE_KEY, json!(true));
        }
    });
    settings_section(
        "typography",
        Text::from("Typography"),
        view()
            .child(region(move || show_advanced.get(), move |advanced| {
                view()
                    .child(font_row(context, INTERFACE, true, !advanced))
                    .child(advanced.then(|| font_row(context, PROMPT, false, true)))
                    .child(font_row(context, if advanced { CODE } else { MONO }, false, true))
                    .child(advanced.then(|| font_row(context, TERMINAL, false, true)))
            }))
            .child(settings_row(switch_row(
                "word-wrap",
                "wordWrap",
                "word wrapping",
                "Wrap code, diffs, and file previews by default",
                "Wrap long lines in code blocks, diffs, and file previews by default.",
            )))
            .child(advanced_settings(
                &["separate-fonts", "font-smoothing"],
                view()
                    .child(settings_row(SettingsRowProps::searchable("separate-fonts")
                        .first()
                        .description(|| Text::from("Set the prompt, code, and terminal fonts separately instead of one monospace font."))
                        .control(move || switch(
                            "Separate fonts",
                            ControlState { checked: Prop::bind(move || show_advanced.get()), ..Default::default() },
                            Size::Default,
                            move |next| choose(context, preference, TYPOGRAPHY_ADVANCED_STORAGE_KEY, json!(next)),
                        ))))
                    .child(crate::host::is_mac().then(|| settings_row(switch_row(
                        "font-smoothing",
                        "fontSmoothing",
                        "font smoothing",
                        "Font smoothing",
                        "Use thinner grayscale text smoothing instead of the macOS default.",
                    )))),
            )),
    )
}

#[derive(Clone, Copy)]
struct FontRow {
    anchor: &'static str,
    title: &'static str,
    description: &'static str,
    family: &'static str,
    size: &'static str,
    size_label: &'static str,
    range: (u32, u32),
    reset: &'static str,
}
const INTERFACE: FontRow = FontRow {
    anchor: "interface-font",
    title: "Interface font",
    description: "Everything outside code blocks and the terminal.",
    family: "fontFamilySans",
    size: "fontSizeInterface",
    size_label: "Interface font size",
    range: (MIN_INTERFACE_FONT_SIZE, MAX_INTERFACE_FONT_SIZE),
    reset: "interface font",
};
const PROMPT: FontRow = FontRow {
    anchor: "prompt-font",
    title: "Prompt font",
    description: "Only the box you write prompts in. Mono works well here.",
    family: "fontFamilyComposer",
    size: "fontSizePrompt",
    size_label: "Prompt font size",
    range: (MIN_PROMPT_FONT_SIZE, MAX_PROMPT_FONT_SIZE),
    reset: "prompt font",
};
const CODE: FontRow = FontRow {
    anchor: "code-font",
    title: "Code font",
    description: "Code blocks, diffs, and file previews.",
    family: "fontFamilyCode",
    size: "fontSizeCode",
    size_label: "Code font size",
    range: (MIN_CODE_FONT_SIZE, MAX_CODE_FONT_SIZE),
    reset: "code font",
};
const MONO: FontRow = FontRow {
    title: "Monospace font",
    description: "Code blocks, diffs, file previews, and the terminal.",
    reset: "monospace font",
    ..CODE
};
const TERMINAL: FontRow = FontRow {
    anchor: "terminal-font",
    title: "Terminal font",
    description: "Terminal output, independent from code blocks and diffs.",
    family: "fontFamilyTerminal",
    size: "fontSizeTerminal",
    size_label: "Terminal font size",
    range: (MIN_TERMINAL_FONT_SIZE, MAX_TERMINAL_FONT_SIZE),
    reset: "terminal font",
};

fn font_row(
    context: SettingsContext,
    row: FontRow,
    first: bool,
    preview: bool,
) -> impl IntoElement {
    let FontRow {
        anchor,
        title,
        description,
        family,
        size,
        size_label,
        range: (min, max),
        reset,
    } = row;
    let saved = computed(move || {
        context
            .setting(family)
            .as_str()
            .unwrap_or_default()
            .to_owned()
    });
    let defaults = default_unified_settings();
    let reset_patch = json!({family: defaults[family], size: defaults[size]});
    let mut props = SettingsRowProps::titled(anchor, Text::from(title))
        .description(move || Text::from(description))
        .reset(RowReset::new(
            reset,
            move || context.setting(family) != defaults[family] || context.setting(size) != defaults[size],
            move || context.update(reset_patch.clone()),
        ))
        .control(move || {
            use crate::ui::select::{Popup, SelectItem, SelectProps, Size, Value, select};
            let draft = signal(saved.get_untracked());
            effect(move || draft.set(saved.get()));
            let commit = move || {
                let value = draft.with_untracked(|value| value.trim().to_owned());
                let monospace = matches!(family, "fontFamilyCode" | "fontFamilyTerminal");
                if value != saved.get_untracked()
                    && value.chars().count() <= 200
                    && (value.is_empty() || (is_font_family_available(&NshFontProbe, &value)
                        && (!monospace || is_monospace_family(&NshFontProbe, &value))))
                {
                    context.update(json!({family: value}));
                }
            };
            let mut props = SelectProps::new(size_label);
            props.items = (min..=max)
                .map(|size| SelectItem::new(size.to_string(), format!("{size} px")))
                .collect::<Vec<_>>().into();
            props.value = Prop::bind(move || Value::Controlled(Some(context.setting(size).to_string().into())));
            props.size = Size::Sm;
            props.popup = Popup { align: Align::End, ..Popup::default() };
            view().app_class(tw("flex w-full flex-row flex-wrap items-center gap-2 sm:w-auto"))
                .child(view().app_class(tw("min-w-36 flex-1 sm:w-44 sm:flex-none"))
                    .child(nsh_base::input()
                        .label(tr("{title} family", &[("title", t3_i18n::Value::Text(Text::from(title).resolve().to_string()))]))
                        .value(move || draft.get())
                        .placeholder(if family == "fontFamilySans" || family == "fontFamilyComposer" { t("System default") } else { t("System monospace") })
                        .on_change(move |next: &str| draft.set(next.to_owned()))
                        .on_blur(commit)
                        .on_submit(move |_: &str| commit())
                        .submit_behavior(SubmitBehavior::Submit)
                        .on_key_down(move |event: &KeyEvent| {
                            if event.key.as_ref() == "Escape" { draft.set(saved.get_untracked()); }
                        })
                        .text_color(move || palette().foreground)
                        .app_class(tw("h-8 w-full min-w-0 rounded-lg border border-input bg-background px-[11px] text-sm"))))
                .child(view().app_class(tw("w-22 shrink-0")).child(select(props, move |next| {
                    if let Some(size_value) = next.and_then(|value| value.parse::<u32>().ok())
                        && (min..=max).contains(&size_value)
                    {
                        context.update(json!({size: size_value}));
                    }
                })))
        });
    if first {
        props = props.first();
    }
    if preview {
        props = props.children(move || font_previews(context, row));
    }
    settings_row(props)
}

struct NshFontProbe;

impl FontProbe for NshFontProbe {
    fn canvas_text_width(&self, font: &str, text: &str) -> Option<f64> {
        Some(f64::from(measure_text(font, text).0))
    }

    fn dom_text_width(&self, _: &str) -> Option<f64> {
        None
    }

    fn is_apple_platform(&self) -> bool {
        crate::host::is_mac()
    }
}

/// Samples from SettingsFontPreviews.tsx. The terminal is an in-process NSH terminal.
fn font_previews(context: SettingsContext, row: FontRow) -> AnyElement {
    let family = move || {
        context
            .setting(row.family)
            .as_str()
            .unwrap_or_default()
            .to_owned()
    };
    let size = move || context.setting(row.size).as_f64().unwrap_or_default() as f32;
    match row.family {
        "fontFamilyCode" => view()
            .child(
                view()
                    .app_class(tw(
                        "mt-1 mb-2 overflow-hidden rounded-lg border border-border bg-code",
                    ))
                    .child(diff_font_preview_line(
                        "src/formatUser.ts",
                        family,
                        size,
                        "px-3 py-2 bg-muted",
                    ))
                    .child(diff_font_preview_line(
                        " export function formatUser(user: User) {",
                        family,
                        size,
                        "px-3",
                    ))
                    .child(diff_font_preview_line(
                        "-  return user.name.toUpperCase();",
                        family,
                        size,
                        "px-3 bg-diff-deletion/30",
                    ))
                    .child(diff_font_preview_line(
                        "+  return `${user.name} <${user.email}>`; // 0O 1lI",
                        family,
                        size,
                        "px-3 bg-diff-addition/30",
                    ))
                    .child(diff_font_preview_line(" }", family, size, "px-3")),
            )
            .child((row.title == "Monospace font").then(|| terminal_font_preview(family, size)))
            .into_element(),
        "fontFamilyTerminal" => terminal_font_preview(family, size).into_element(),
        _ => {
            let prompt = signal("Use $frontend-design to fix the flaky test in [surface.test.ts](apps/web/src/terminal/ghostty/surface.test.ts) and align the header with [SettingsPanels.tsx](apps/web/src/components/settings/SettingsPanels.tsx) before shipping.".to_owned());
            view()
                .app_class(tw(
                    "mt-1 mb-2 rounded-lg border border-border bg-background px-3 py-2",
                ))
                .child(
                    nsh_base::input()
                        .multiline(true)
                        .value(move || prompt.get())
                        .on_change(move |next: &str| prompt.set(next.to_owned()))
                        .label(t("Prompt font preview"))
                        .font_family(move || {
                            SharedString::from(appearance_font_stack(
                                &family(),
                                DEFAULT_SANS_FONT_STACK,
                            ))
                        })
                        .text_size(move || px(size()))
                        .app_class(tw("min-h-24 w-full max-h-42 text-foreground")),
                )
                .into_element()
        }
    }
}

fn diff_font_preview_line(
    source: &'static str,
    family: impl Fn() -> String + 'static,
    size: impl Fn() -> f32 + 'static,
    class: &'static str,
) -> impl IntoElement {
    view().app_class(tw(class)).child(
        text(SharedString::from(source))
            .text_size(move || px(size()))
            .font_family(move || {
                SharedString::from(appearance_font_stack(&family(), DEFAULT_CODE_FONT_STACK))
            })
            .app_class(tw("text-code-foreground")),
    )
}

fn terminal_font_preview(
    family: impl Fn() -> String + 'static,
    size: impl Fn() -> f32 + 'static,
) -> impl IntoElement {
    const PROMPT: &str = "\x1b[1;32m→\x1b[0m \x1b[1;36mt3code\x1b[0m \x1b[1;34mgit:(\x1b[1;31mmain\x1b[1;34m)\x1b[0m \x1b[1;33m✗\x1b[0m ";
    const OUTPUT: &str = "\r\n\r\n  \x1b[1;32mVITE\x1b[0m \x1b[32mv7.1.1\x1b[0m  \x1b[2mready in\x1b[0m \x1b[1m1.24s\x1b[0m\r\n\r\n  \x1b[32m→\x1b[0m  \x1b[2mLocal:\x1b[0m    \x1b[4;36mhttp://127.0.0.1:5173/\x1b[0m\r\n  \x1b[32m→\x1b[0m  \x1b[2mNetwork:\x1b[0m  \x1b[4;36mhttp://192.168.1.24:5173/\x1b[0m\r\n\r\n  \x1b[32m✓ 85 passed\x1b[0m   \x1b[33m△ 2 warnings\x1b[0m   \x1b[31m✗ 0 failed\x1b[0m\r\n\r\n  \x1b[42;30m READY \x1b[0m \x1b[2mwatching for changes, press\x1b[0m \x1b[1mq\x1b[0m \x1b[2mto quit\x1b[0m\r\n\r\n";
    let terminal = component_ref::<nsh_base::Terminal>();
    let shown = signal(false);
    let columns = signal(0_usize);
    let size = stored(size);
    let line_height = move || size.with(|size| size() * 1.4);
    view()
        .app_class(tw(
            "relative mt-1 mb-2 overflow-hidden rounded-lg border border-border",
        ))
        .h(move || px(line_height() * 12.0 + 2.0))
        .child(
            nsh_base::terminal()
                .component_ref(&terminal)
                .accessibility_label(t("Terminal font preview"))
                .font_family(move || {
                    let family = family();
                    (!family.is_empty()).then(|| family.into())
                })
                .font_size(move || px(size.with(|size| size())))
                .line_height(move || px(line_height()))
                .on_resize(move |_: &nsh_base::TerminalEvent| {
                    if !shown.get_untracked()
                        && let Some(api) = terminal.api::<nsh_base::TerminalApi>()
                    {
                        shown.set(true);
                        api.write(format!("{PROMPT}vpr dev{OUTPUT}{PROMPT}"));
                    }
                })
                .on_data(move |data: &SharedString| {
                    let Some(api) = terminal.api::<nsh_base::TerminalApi>() else {
                        return;
                    };
                    match data.as_ref() {
                        "\r" => {
                            api.write(format!("\r\n{PROMPT}"));
                            columns.set(0);
                        }
                        "\x7f" | "\x08" if columns.get_untracked() > 0 => {
                            columns.update(|count| *count -= 1);
                            api.write("\x08 \x08".into());
                        }
                        data if !data.starts_with('\x1b') => {
                            let printable: String =
                                data.chars().filter(|ch| !ch.is_control()).collect();
                            columns.update(|count| *count += printable.chars().count());
                            api.write(printable);
                        }
                        _ => {}
                    }
                })
                .size_full(),
        )
}
