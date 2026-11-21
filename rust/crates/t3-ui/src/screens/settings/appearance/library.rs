//! ThemeSettings.tsx cards for whole-theme and per-appearance selection.
use super::*;
use crate::theme::{Edit, ThemeState, mode_key};
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};
use t3_theme::palette::{Appearance as Half, ThemeDefinition};

#[derive(Clone, PartialEq)]
pub(super) enum Form {
    Import,
    Editor {
        seed: Option<ThemeDefinition>,
        editing: bool,
        name: String,
    },
    Remove(ThemeDefinition, Vec<ThemeDefinition>),
}

pub(super) fn theme_library(context: SettingsContext) -> AnyElement {
    let state = use_context::<ThemeState>();
    let choice =
        computed(move || state.map_or_else(ThemeChoice::default, |state| state.choice.get()));
    let host = use_context::<super::ThemeForms>();
    let form = host.map_or_else(|| signal(None), |host| host.form);
    let modes = [
        ("system", "System", "Follow the system appearance"),
        ("light", "Light", "Use light mode"),
        ("dark", "Dark", "Use dark mode"),
    ].into_iter().map(|(key,label,accessible)| {
        let active = move || choice.with(|choice| mode_key(choice.mode) == key);
        nsh_base::button()
            .accessibility_label(t(accessible))
            .selected(active)
            .on_press(move || { if let Some(state) = state { state.choose(Edit::Mode(key.into())); } })
            .app_class(move || Some(tw(&crate::theme::cn([Some("flex cursor-pointer flex-col items-stretch gap-1.5 rounded-xl border p-2 outline-none transition-colors focus-visible:outline-2 focus-visible:outline-solid focus-visible:-outline-offset-7 focus-visible:outline-foreground/60"), Some(if active() {"border-transparent bg-accent/30 ring-2 ring-inset ring-ring"} else {"border-border/70 bg-card/60 hover:bg-accent/10"})]))))
            .child(view().app_class(tw("flex h-[8.75rem] flex-row overflow-hidden rounded-lg"))
                .child((key != "dark").then(|| theme_wireframe(choice, false, key == "system")))
                .child((key != "light").then(|| theme_wireframe(choice, true, key == "system"))))
            .child(text(t(label)).app_class(tw("text-center text-xs font-medium")))
            .into_element()
    }).collect::<Vec<_>>();
    let heading = view()
        .app_class(tw(
            "flex min-h-8 flex-row flex-wrap items-center justify-between gap-3 px-3 pt-5 sm:px-4",
        ))
        .child(text(t("Themes")).app_class(tw("text-base font-semibold text-foreground")))
        .child(
            view()
                .app_class(tw("flex flex-row flex-wrap items-center justify-end gap-2"))
                .child(action("Create theme", Icon::Paintbrush, move || {
                    let selected = choice.get_untracked();
                    let half = if dark() { Half::Dark } else { Half::Light };
                    let seed = selected.catalog().get(selected.owner(half)).cloned();
                    form.set(Some(Form::Editor {
                        seed,
                        editing: false,
                        name: String::new(),
                    }));
                }))
                .child(action("Add theme", Icon::Plus, move || {
                    form.set(Some(Form::Import))
                })),
        );
    view()
        .test_id("appearance")
        .app_class(tw("flex flex-col gap-3"))
        .child(super::heading("Color scheme"))
        .child(
            view()
                .test_id("color-scheme")
                .app_class(tw("grid w-full grid-cols-3 gap-3"))
                .child(modes),
        )
        .child(heading)
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
                .child(region(
                    move || choice.with(|s| (s.custom.clone(), s.environment.clone())),
                    move |(custom, environment)| {
                        let mut cards = std::iter::once(None)
                            .chain(built_in_themes().iter().map(Some))
                            .chain(
                                environment
                                    .iter()
                                    .filter(|t| !custom.iter().any(|saved| saved.id == t.id))
                                    .map(Some),
                            )
                            .map(|theme| card(choice, state, form, theme, false, None))
                            .collect::<Vec<_>>();
                        let mut groups: Vec<Vec<ThemeDefinition>> = Vec::new();
                        for theme in custom.iter() {
                            if let Some(collection) = &theme.collection
                                && let Some(group) = groups.iter_mut().find(|g| {
                                    g[0].collection
                                        .as_ref()
                                        .is_some_and(|c| c.id == collection.id)
                                })
                            {
                                group.push(theme.clone());
                            } else {
                                groups.push(vec![theme.clone()]);
                            }
                        }
                        for group in groups {
                            if group.len() == 1 {
                                cards.push(card(choice, state, form, group.first(), true, None));
                            } else {
                                let selected = choice.with_untracked(|c| {
                                    group
                                        .iter()
                                        .position(|t| Half::ALL.iter().any(|h| c.owner(*h) == t.id))
                                        .unwrap_or(0)
                                });
                                let variants = Variants {
                                    group: stored(group),
                                    selected: signal(selected),
                                };
                                cards.push(
                                    region(
                                        move || variants.selected.get(),
                                        move |i| {
                                            let selected =
                                                variants.group.with(|group| group.get(i).cloned());
                                            card(
                                                choice,
                                                state,
                                                form,
                                                selected.as_ref(),
                                                true,
                                                Some(variants),
                                            )
                                        },
                                    )
                                    .into_element(),
                                );
                            }
                        }
                        cards
                    },
                )),
        )
        .child(host.is_none().then(|| {
            region(
                move || form.get(),
                move |request| {
                    request
                        .zip(state)
                        .map(|(request, state)| super::forms::show(form, state, request))
                },
            )
        }))
        .into_element()
}

fn action(label: &'static str, icon: Icon, press: impl Fn() + 'static) -> impl IntoElement {
    button(
        ButtonContent::IconLabel {
            icon,
            label: t(label),
        },
        ButtonVariant::Outline,
        ButtonSize::Xs,
        || false,
        press,
    )
}

#[derive(Clone, Copy)]
struct Variants {
    group: Stored<Vec<ThemeDefinition>>,
    selected: Signal<usize>,
}

impl Variants {
    fn label(self) -> String {
        self.group.with(|group| {
            group[0]
                .collection
                .as_ref()
                .map_or_else(|| group[0].label.clone(), |c| c.label.clone())
        })
    }

    fn defaults(self, state: ThemeState) {
        self.group.with(|group| {
            for half in Half::ALL {
                if let Some(t) = group.iter().find(|t| t.colors_for(half).is_some()) {
                    state.choose(Edit::Half(half, Some(t.id.clone())));
                }
            }
        });
        self.selected.set(0);
    }

    fn controls(self, state: Option<ThemeState>, choice: Memo<ThemeChoice>) -> Vec<AnyElement> {
        let open = signal(None);
        Half::ALL
            .into_iter()
            .filter_map(|half| self.half_control(half, open, state, choice))
            .collect()
    }

    fn half_control(
        self,
        half: Half,
        open: Signal<Option<Half>>,
        state: Option<ThemeState>,
        choice: Memo<ThemeChoice>,
    ) -> Option<AnyElement> {
        let options = self.group.with(|group| {
            group
                .iter()
                .enumerate()
                .filter(|(_, theme)| theme.colors_for(half).is_some())
                .map(|(i, theme)| (i, theme.clone()))
                .collect::<Vec<_>>()
        });
        let first = options.first()?.clone();
        let count = options.len();
        let options = stored(options);
        let selected = computed(move || {
            choice.with(|c| {
                options.with(|items| {
                    items
                        .iter()
                        .find(|(_, theme)| theme.id == c.owner(half))
                        .cloned()
                        .unwrap_or_else(|| first.clone())
                })
            })
        });
        let pick = move |index, id| {
            if let Some(state) = state {
                state.choose(Edit::Half(half, Some(id)));
            }
            open.set(None);
            self.selected.set(index);
        };
        let trigger = nsh_base::button()
            .accessibility_label(move || {
                let label = selected.with(|(_, theme)| theme.label.clone());
                let mode = half.as_str();
                Some(
                    if count > 1 {
                        format!("Choose {mode} variant, {count} options, currently {label}")
                    } else {
                        format!("Use {mode} variant, currently {label}")
                    }
                    .into(),
                )
            })
            .on_press(move || {
                if count > 1 {
                    open.set(Some(half));
                } else {
                    let (index, theme) = selected.get_untracked();
                    pick(index, theme.id);
                }
            })
            .child(dynamic(move || {
                selected.with(|(_, t)| theme_swatch(Some(t), half))
            }));
        let options_list = region(
            move || open.get() == Some(half),
            move |expanded| {
                if !expanded {
                    return Vec::new();
                }
                options.with(|items| {
                    items
                        .iter()
                        .map(|(i, theme)| {
                            let index = *i;
                            let id = theme.id.clone();
                            nsh_base::button()
                                .accessibility_label(format!(
                                    "Use {} for {} mode",
                                    theme.label,
                                    half.as_str()
                                ))
                                .on_press(move || pick(index, id.clone()))
                                .child(text(theme.label.clone()))
                                .into_element()
                        })
                        .collect::<Vec<_>>()
                })
            },
        );
        Some(
            view()
                .app_class(tw("relative flex flex-col items-center"))
                .child(trigger)
                .child(text(move || SharedString::from(selected.get().1.label)))
                .child(options_list)
                .into_element(),
        )
    }
}

fn card(
    choice: Memo<ThemeChoice>,
    state: Option<ThemeState>,
    form: Signal<Option<Form>>,
    definition: Option<&ThemeDefinition>,
    editable: bool,
    variants: Option<Variants>,
) -> AnyElement {
    let id = stored(definition.map(|t| t.id.clone()));
    let name = definition.map_or_else(|| t("T3 Code"), |t| t.label.clone());
    let seed = stored(definition.cloned());
    let halves = if let Some(variants) = variants {
        variants.controls(state, choice)
    } else {
        Half::ALL.into_iter().filter(|&half| definition.is_none_or(|t|t.colors_for(half).is_some())).map(|half| {
        let active = move || choice.with(|c| c.catalog().get(c.owner(half)).map(|t| &t.id) == id.get().as_ref());
        nsh_base::button().accessibility_label(tr("Use {name} {mode} mode", &[("name",t3_i18n::Value::Text(name.clone())),("mode",t3_i18n::Value::Text(half.as_str().into()))]))
            .selected(active)
            .on_press(move || { if let Some(state) = state { state.choose(Edit::Half(half,id.get())); } })
            .app_class(move || Some(tw(if active() { "relative flex size-[68px] shrink-0 cursor-pointer items-center justify-center rounded-full p-1.5 ring-2 ring-inset ring-ring" } else { "relative flex size-[68px] shrink-0 cursor-pointer items-center justify-center rounded-full p-1.5" })))
            .child(theme_swatch(definition,half)).into_element()
    }).collect::<Vec<_>>()
    };
    let whole = move || {
        if let Some(state) = state {
            if let Some(variants) = variants {
                variants.defaults(state);
                return;
            }
            if let Some(half) =
                seed.with(|t| t.as_ref().and_then(ThemeDefinition::single_appearance))
            {
                state.choose(Edit::Half(half, id.get()));
            } else {
                state
                    .choose(Edit::Whole(id.get().unwrap_or_else(|| {
                        mode_key(choice.get_untracked().mode).into()
                    })));
            }
        }
    };
    let icon = |label: String, icon: Icon, press: Box<dyn Fn()>| {
        button(
            ButtonContent::Icon { icon, label },
            ButtonVariant::Ghost,
            ButtonSize::IconXs,
            || false,
            press,
        )
        .into_element()
    };
    let label = |template| tr(template, &[("name", t3_i18n::Value::Text(name.clone()))]);
    let copy_name = label("{name} copy");
    let duplicate = icon(
        label("Duplicate {name}"),
        Icon::Copy,
        Box::new(move || {
            form.set(Some(Form::Editor {
                seed: seed.get(),
                editing: false,
                name: copy_name.clone(),
            }))
        }),
    );
    let edit = editable.then(|| {
        icon(
            label("Edit {name}"),
            Icon::PenLine,
            Box::new(move || {
                if let Some(t) = seed.get() {
                    form.set(Some(Form::Editor {
                        name: t.label.clone(),
                        seed: Some(t),
                        editing: true,
                    }));
                }
            }),
        )
    });
    let export = state.filter(|s| editable && s.can_export()).map(|state| {
        icon(
            label("Export {name}"),
            Icon::Upload,
            Box::new(move || {
                seed.with(|s| {
                    if let Some(t) = s {
                        state.export(t);
                    }
                })
            }),
        )
    });
    let remove = editable.then(|| {
        icon(
            label("Remove {name}"),
            Icon::Trash2,
            Box::new(move || {
                if let Some(t) = seed.get() {
                    form.set(Some(Form::Remove(
                        t,
                        variants.map_or_else(Vec::new, |v| v.group.get()),
                    )));
                }
            }),
        )
    });
    let caption = variants.map_or_else(|| name.clone(), Variants::label);
    let use_label = variants.map_or_else(
        || label("Use {name} theme"),
        |v| format!("Use {}, {name} variant", v.label()),
    );
    view().app_class(tw("cursor-pointer overflow-hidden rounded-xl border border-border/70 bg-card/60 transition-colors hover:bg-accent/10"))
        .child(view().app_class(tw("flex min-h-16 flex-row items-center justify-center gap-2.5 px-3 pt-3")).child(halves))
        .child(view().app_class(tw("flex flex-row items-center gap-2 px-3 pb-3 pt-2"))
            .child(nsh_base::button().accessibility_label(use_label).on_press(whole)
                   .app_class(tw("min-w-0 flex-1 cursor-pointer truncate rounded-sm text-left text-sm font-medium text-foreground")).child(text(SharedString::from(caption))))
            .child(duplicate).child(edit).child(export).child(remove)).into_element()
}
