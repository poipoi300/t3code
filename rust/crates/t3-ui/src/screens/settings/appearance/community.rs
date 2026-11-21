//! ThemeSearchSection.tsx. The theme crate verifies and converts Open VSX packages.
use super::{forms::Submission, *};
use crate::runtime::tasks::TaskOwner;
use crate::theme::{Edit, ThemeState};
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};
use crate::ui::select::{Popup, SelectItem, SelectProps, Size, Value as Selected, select};
use std::time::Duration;
use t3_theme::import::{NoCancel, OpenVsxClient, OpenVsxThemeExtension, OpenVsxThemeSort as Sort};

pub(super) fn section(state: ThemeState, submit: Submission) -> AnyElement {
    let Some(platform) = crate::theme::ThemePlatform::current() else {
        return view().into_element();
    };
    let platform = stored(platform);
    let query = signal(String::new());
    let sort = signal(Sort::DownloadCount);
    let revision = signal(0u64);
    let results = signal(None::<Vec<OpenVsxThemeExtension>>);
    let searching = signal(false);
    let pending = signal(None::<OpenVsxThemeExtension>);
    let installing = signal(false);
    let owner = TaskOwner::here();
    let cancel = stored(None::<futures::future::AbortHandle>);
    let stop_search = move || {
        if let Some(handle) = cancel.with(Option::take) {
            handle.abort();
        }
    };
    let search = move |debounce: bool| {
        stop_search();
        revision.update(|n| *n += 1);
        let serial = revision.get_untracked();
        let q = query.get_untracked();
        if q.trim().is_empty() {
            results.set(None);
            searching.set(false);
            return;
        }
        let order = sort.get_untracked();
        let (handle, registration) = futures::future::AbortHandle::new_pair();
        cancel.with(|slot| *slot = Some(handle));
        owner.spawn(async move {
            if debounce {
                timer(Duration::from_millis(350)).await;
            }
            if revision.try_get() != Some(serial) || installing.get_untracked() {
                return;
            }
            searching.set(true);
            let catalogue = OpenVsxClient::new(platform.with(|p| p.transport(None)));
            let Ok(answer) = futures::future::Abortable::new(
                catalogue.search(&q, order, &NoCancel),
                registration,
            )
            .await
            else {
                return;
            };
            if revision.try_get() != Some(serial) {
                return;
            }
            searching.set(false);
            match answer {
                Ok(items) => results.set(Some(items)),
                Err(e) => {
                    results.set(None);
                    submit.error.set(e.to_string());
                }
            }
        });
    };
    let install = move |extension: OpenVsxThemeExtension, confirmed: bool| {
        if installing.get_untracked() {
            return;
        }
        let expected = state.choice.with_untracked(|c| {
            c.custom
                .iter()
                .filter(|t| {
                    t.collection
                        .as_ref()
                        .is_some_and(|col| col.id == extension.collection_id)
                })
                .cloned()
                .collect::<Vec<_>>()
        });
        if !confirmed && !expected.is_empty() {
            pending.set(Some(extension));
            return;
        }
        stop_search();
        pending.set(None);
        revision.update(|n| *n += 1);
        searching.set(false);
        installing.set(true);
        owner.spawn(async move {
            let answer = OpenVsxClient::new(platform.with(|p| p.transport(Some(&extension))))
                .import(&extension, &NoCancel)
                .await;
            if !installing.try_set(false) {
                return;
            }
            match answer {
                Ok(themes) => {
                    submit.save(Edit::Collection(extension.collection_id, themes, expected))
                }
                Err(e) => submit.error.set(e.to_string()),
            }
        });
    };
    let sorts = [
        (Sort::DownloadCount, "Most downloaded"),
        (Sort::Rating, "Best rated"),
        (Sort::Timestamp, "Newest"),
        (Sort::Relevance, "Most relevant"),
    ];
    view()
        .app_class(tw("flex flex-col gap-3"))
        .child(text(t("Search community themes")).app_class(tw("text-sm font-medium")))
        .child(
            text(t("Find open-source themes from Open VSX."))
                .app_class(tw("text-xs text-muted-foreground")),
        )
        .child(
            nsh_base::input()
                .label(t("Search Open VSX themes"))
                .placeholder(t("Search themes..."))
                .value(move || query.get())
                .on_change(move |v: &str| {
                    query.set(v.into());
                    search(true);
                })
                .on_submit(move |_: &str| search(false))
                .app_class(tw("h-9 w-full rounded-lg border border-input px-3")),
        )
        .child(
            view()
                .app_class(tw("flex flex-row flex-wrap items-center gap-1.5"))
                .child(text(t("Popular")))
                .child(
                    ["Dracula", "Catppuccin", "Nord", "Tokyo Night"]
                        .map(|s| {
                            button(
                                ButtonContent::Label(s.into()),
                                ButtonVariant::Ghost,
                                ButtonSize::Xs,
                                move || installing.get(),
                                move || {
                                    query.set(s.into());
                                    search(false);
                                },
                            )
                            .into_element()
                        })
                        .into_iter()
                        .collect::<Vec<_>>(),
                ),
        )
        .child(region(
            move || results.get(),
            move |items| {
                items.map(|items| {
                    if items.is_empty() {
                        return text(t("No supported open-source themes found")).into_element();
                    }
                    let mut props = SelectProps::new("Sort themes");
                    props.items = sorts
                        .iter()
                        .map(|(key, label)| SelectItem::new(key.as_str(), t(label)))
                        .collect::<Vec<_>>()
                        .into();
                    props.size = Size::Sm;
                    props.popup = Popup {
                        align: Align::End,
                        ..Default::default()
                    };
                    props.value =
                        Prop::bind(move || Selected::Controlled(Some(sort.get().as_str().into())));
                    let rows = items
                        .into_iter()
                        .map(|e| {
                            let exists = state.choice.with_untracked(|c| {
                                c.custom.iter().any(|t| {
                                    t.collection
                                        .as_ref()
                                        .is_some_and(|v| v.id == e.collection_id)
                                })
                            });
                            let label = tr(
                                if exists {
                                    "Update {name}"
                                } else {
                                    "Add {name}"
                                },
                                &[("name", t3_i18n::Value::Text(e.name.clone()))],
                            );
                            view()
                                .app_class(tw("flex flex-row items-center justify-between gap-2"))
                                .child(text(SharedString::from(e.name.clone())))
                                .child(button(
                                    ButtonContent::Label(label),
                                    ButtonVariant::Outline,
                                    ButtonSize::Xs,
                                    move || installing.get() || submit.busy.get(),
                                    move || install(e.clone(), false),
                                ))
                                .into_element()
                        })
                        .collect::<Vec<_>>();
                    view()
                        .child(select(props, move |v| {
                            if let Some((s, _)) =
                                sorts.iter().find(|(s, _)| v.as_deref() == Some(s.as_str()))
                            {
                                sort.set(*s);
                                search(false);
                            }
                        }))
                        .child(scroll().app_class(tw("max-h-60")).child(view().child(rows)))
                        .into_element()
                })
            },
        ))
        .child(region(
            move || searching.get(),
            |active| active.then(|| text(t("Searching themes..."))),
        ))
        .child(region(
            move || pending.get(),
            move |e| {
                e.map(|ext| {
                    view()
                        .child(text(tr(
                            "Update {name}?",
                            &[("name", t3_i18n::Value::Text(ext.name.clone()))],
                        )))
                        .child(button(
                            ButtonContent::Label(t("Cancel")),
                            ButtonVariant::Ghost,
                            ButtonSize::Sm,
                            || false,
                            move || pending.set(None),
                        ))
                        .child(button(
                            ButtonContent::Label(t("Update")),
                            ButtonVariant::Default,
                            ButtonSize::Sm,
                            || false,
                            move || install(ext.clone(), true),
                        ))
                })
            },
        ))
        .into_element()
}
