//! Port of SettingsPanels.tsx's DiffsSettingsPanel and DiffColorsSettingsRow, and
//! IntegrationsSettings.tsx's PullRequestLinksOpenInAppSetting.

use nsh_core::prelude::*;
use serde_json::json;

use super::setting_controls::{choice_row, switch_row};
use super::shell::{settings_page_container, settings_row, settings_section};
use crate::i18n::Text;

pub fn screen() -> AnyElement {
    settings_page_container(
        view()
            .app_class(crate::theme::tw("flex flex-col gap-8"))
            .child(settings_section(
                "diffs",
                Text::from("Diffs"),
                view()
                    .child(settings_row(choice_row(
                        "diff-layout",
                        "diffLayout",
                        "diff layout",
                        "Diff layout",
                        "Show diffs stacked or side by side. The toggle in the diff toolbar changes this too.",
                        &[("stacked", "Stacked"), ("split", "Split")],
                    ).first()))
                    .child(settings_row(switch_row(
                        "hide-whitespace-changes",
                        "diffIgnoreWhitespace",
                        "diff whitespace changes",
                        "Hide whitespace changes by default",
                        "Set whether the diff panel ignores whitespace-only edits by default.",
                    )))
                    .child(settings_row(choice_row(
                        "default-diff-file-state",
                        "diffFilesCollapsed",
                        "default diff file state",
                        "Default diff file state",
                        "Start with files expanded or collapsed when opening diffs or a pull request's Code tab.",
                        &[("false", "Expanded"), ("true", "Collapsed")],
                    )))
                    .child(diff_colors()),
            ))
            .child(settings_section(
                "pull-requests",
                Text::from("Pull requests"),
                view()
                    .child(settings_row(switch_row(
                        "proactive-panels",
                        "proactivePanelsEnabled",
                        "proactive panels",
                        "Open pull requests and changes panels proactively",
                        "Open linked pull requests first. Otherwise, open Changes for edits to at least 3 files or 50 lines.",
                    ).first()))
                    .child(pull_request_links()),
            )),
    )
    .into_element()
}

fn diff_colors() -> impl IntoElement {
    use super::setting_controls::reset;
    use super::shell::{SettingsRowProps, use_settings};
    use crate::theme::tw;
    use crate::ui::select::{
        Popup, SelectItem, SelectProps, Size, Value, Variant, select, select_trigger,
    };
    use std::rc::Rc;
    use t3_theme::{
        palette::Appearance,
        resolved::{ThemeInput, resolve_theme},
    };
    const KEY: &str = "diffColorScheme";
    let context = use_settings();
    let alternate = computed(move || context.setting(KEY).as_str() == Some("blue-orange"));
    settings_row(
        SettingsRowProps::searchable("diff-color-scheme")
            .description(|| {
                Text::from("Choose colors for additions and deletions, including change counts.")
            })
            .reset(reset(context, KEY, "diff colors"))
            .control(move || {
                let open = signal(false);
                let mut props = SelectProps::new("Diff colors");
                props.items = vec![
                    SelectItem::new("red-green", Text::from("Red & green (default)").resolve()),
                    SelectItem::new("blue-orange", Text::from("Blue & orange").resolve()),
                ]
                .into();
                props.value = Prop::bind(move || {
                    Value::Controlled(Some(
                        if alternate.get() {
                            "blue-orange"
                        } else {
                            "red-green"
                        }
                        .into(),
                    ))
                });
                props.open = Prop::bind(move || Some(open.get()));
                props.on_open_change = Some(Rc::new(move |next, _| open.set(next)));
                props.size = Size::Sm;
                props.popup = Popup {
                    align: Align::End,
                    ..Popup::default()
                };
                let swatches = |read_color| {
                    let resolve = |scheme, mode| {
                        let mut input = ThemeInput::new(None, mode);
                        input.diff_color_scheme = Some(scheme);
                        resolve_theme(&input)
                    };
                    let get = |scheme| {
                        let color = |mode| {
                            resolve(scheme, mode)
                                .color(read_color)
                                .map_or(AdaptiveColor::TRANSPARENT.light, |color| {
                                    crate::theme::hsla_of(&color)
                                })
                        };
                        AdaptiveColor::new(color(Appearance::Light), color(Appearance::Dark))
                    };
                    (get("red-green"), get("blue-orange"))
                };
                let additions = swatches("--color-diff-addition");
                let deletions = swatches("--color-diff-deletion");
                props.trigger = Some(Rc::new(move || {
                    select_trigger(
                        view()
                            .app_class(tw("flex flex-1 min-w-0 flex-row items-center gap-1.5"))
                            .child(
                                view()
                                    .accessible(false)
                                    .app_class(move || {
                                        Some(tw(if alternate.get() {
                                            "flex shrink-0 flex-row-reverse gap-1"
                                        } else {
                                            "flex shrink-0 flex-row gap-1"
                                        }))
                                    })
                                    .child(view().app_class(tw("size-2 rounded-full")).bg(
                                        move || {
                                            if alternate.get() {
                                                deletions.1
                                            } else {
                                                deletions.0
                                            }
                                        },
                                    ))
                                    .child(view().app_class(tw("size-2 rounded-full")).bg(
                                        move || {
                                            if alternate.get() {
                                                additions.1
                                            } else {
                                                additions.0
                                            }
                                        },
                                    )),
                            )
                            .child(
                                text(move || {
                                    if alternate.get() {
                                        crate::i18n::t("Blue & orange")
                                    } else {
                                        crate::i18n::t("Red & green")
                                    }
                                })
                                .app_class(tw("truncate")),
                            ),
                        Variant::Default,
                        Size::Sm,
                        false.into(),
                        false.into(),
                        Prop::bind(move || open.get()),
                        None,
                    )
                    .into_element()
                }));
                view()
                    .app_class(tw("w-full sm:w-40"))
                    .child(select(props, move |next| {
                        if let Some(next) = next {
                            context.update(json!({KEY: next.as_ref()}));
                        }
                    }))
            }),
    )
}

fn pull_request_links() -> impl IntoElement {
    use super::shell::{RowReset, SettingsRowProps, use_settings};
    use crate::ui::switch::{ControlState, Size, switch};
    let context = use_settings();
    let checked = move || context.client_settings().pull_request_links_open_in_app;
    settings_row(
        SettingsRowProps::searchable("pull-request-links-open-in-app")
            .description(|| Text::from("Clicking a pull request link opens it in T3 Code. Turn off to open pull request links like any other link, on your git host."))
            .reset(RowReset::new(
                "open pull requests in T3 Code",
                move || !checked(),
                move || context.update(json!({"pullRequestLinksOpenInApp": true})),
            ))
            .control(move || switch(
                "Open pull requests in T3 Code",
                ControlState { checked: Prop::bind(checked), ..Default::default() },
                Size::Default,
                move |checked| context.update(json!({"pullRequestLinksOpenInApp": checked})),
            )),
    )
}
