//! Port of apps/web/src/components/pullRequest/LinkPullRequestDialog.tsx.

use std::{cell::RefCell, rc::Rc};

use nsh_core::prelude::*;
use t3_client::{
    navigation::pull_request_links::{PullRequestInputResolution, ResolvedPullRequestLink},
    types::ThreadKey,
};

use crate::i18n::{Text, t};
use crate::runtime::tasks::TaskOwner;
use crate::runtime::{Rt, services::navigation::PaletteClient};
use crate::theme::tw;
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};
use crate::ui::dialog::{
    FooterVariant, PopupOptions, dialog, dialog_description, dialog_footer, dialog_header,
    dialog_panel, dialog_popup, dialog_title,
};

type Opener = Rc<dyn Fn(ThreadKey)>;

thread_local! {
    static OPENER: RefCell<Option<Opener>> = const { RefCell::new(None) };
}

/// Ask the root dialog host to open for `key`. False when no host is mounted.
pub fn open_link_pull_request_dialog(key: ThreadKey) -> bool {
    OPENER.with(|slot| match slot.borrow().as_ref() {
        Some(open) => {
            open(key);
            true
        }
        None => false,
    })
}

pub fn link_pull_request_dialog_host() -> AnyElement {
    let target = signal(None::<ThreadKey>);
    OPENER.with(|slot| *slot.borrow_mut() = Some(Rc::new(move |key| target.set(Some(key)))));
    dynamic(move || {
        target.get().and_then(|key| {
            let service = use_context::<Rc<dyn PaletteClient>>()
                .or_else(|| use_context::<Rt>().and_then(|rt| rt.backend().palette()))?;
            if !service.pull_request_linking_supported(&key.environment) {
                return None;
            }
            Some(untracked(move || {
                link_dialog(key, service, move || target.set(None))
            }))
        })
    })
    .into_element()
}

fn resolved_link(resolution: &PullRequestInputResolution) -> Option<&ResolvedPullRequestLink> {
    match resolution {
        PullRequestInputResolution::Resolved(link) => Some(link),
        _ => None,
    }
}

fn link_dialog(
    key: ThreadKey,
    service: Rc<dyn PaletteClient>,
    close: impl Fn() + Clone + 'static,
) -> AnyElement {
    let reference = signal(String::new());
    let dirty = signal(false);
    let pending = signal(false);
    let submit_error = signal(None::<String>);
    let open = signal(true);
    let input_ref = element_ref();
    let owner = TaskOwner::here();

    let key_resolve = key.clone();
    let service_resolve = service.clone();
    let resolution = computed(move || {
        let input = reference.get();
        service_resolve.resolve_link_pull_request(&key_resolve, &input)
    });

    let validation = computed(move || -> Option<Text> {
        if !dirty.get() {
            return None;
        }
        match resolution.get() {
            PullRequestInputResolution::Empty => Some(Text::client_key(
                "Paste a pull request URL or enter 123 / #123.",
            )),
            PullRequestInputResolution::Invalid => {
                Some(Text::client_key("Use a pull request URL, 123, or #123."))
            }
            PullRequestInputResolution::Error(message) => Some(Text::content(message)),
            PullRequestInputResolution::Resolved(_) => None,
        }
    });

    let submit = {
        let key = key.clone();
        let service = service.clone();
        let close = close.clone();
        move || {
            dirty.set(true);
            let res = resolution.get_untracked();
            let Some(link) = resolved_link(&res).cloned() else {
                return;
            };
            if pending.get_untracked() {
                return;
            }
            submit_error.set(None);
            pending.set(true);
            let req = service.link_pull_request(key.clone(), link);
            let close = close.clone();
            owner.spawn(async move {
                let result = req.await;
                let _ = pending.try_set(false);
                match result {
                    Ok(()) => {
                        let _ = open.try_set(false);
                        close();
                    }
                    Err(error) => {
                        let _ = submit_error.try_set(Some(error.to_string()));
                    }
                }
            });
        }
    };

    let leave = close.clone();
    dialog(open, move |dialog_state| {
        let leave = leave.clone();
        dialog_state.on_open_change(move |next| {
            if !next && !pending.get_untracked() {
                open.set(false);
                leave();
            }
        });
        let close = close.clone();
        let submit = submit.clone();
        dialog_popup(
            dialog_state,
            PopupOptions {
                initial_focus: Some(input_ref),
                ..Default::default()
            },
            move || {
                let close = close.clone();
                let submit = submit.clone();
                let on_enter = submit.clone();
                view()
                    .app_class(tw("flex min-h-0 w-full flex-col"))
                    .child(dialog_header(
                        dialog_state,
                        view()
                            .child(dialog_title(dialog_state, || {
                                Text::client_key("Link pull request")
                            }))
                            .child(dialog_description(dialog_state, || {
                                Text::client_key(
                                    "Attach a pull request to this thread by its URL, or by its number for this thread's repository.",
                                )
                            })),
                    ))
                    .child(dialog_panel(
                        dialog_state,
                        view()
                            .app_class(tw("flex flex-col gap-2"))
                            .child(
                                nsh_base::input()
                                    .element_ref(&input_ref)
                                    .label(t("Pull request reference"))
                                    .placeholder(t("Pull request URL or #42"))
                                    .value(move || reference.get())
                                    .on_change(move |value: &str| {
                                        dirty.set(true);
                                        reference.set(value.to_owned());
                                    })
                                    .on_submit(move |_: &str| on_enter())
                                    .app_class(tw("text-sm")),
                            )
                        .child(dynamic(move || {
                            resolution.with(|res| {
                                resolved_link(res).map(|link| {
                                    text(format!(
                                        "{}/{} #{}",
                                        link.host, link.repository, link.number
                                    ))
                                    .app_class(tw("truncate text-muted-foreground text-xs"))
                                    .into_element()
                                })
                            })
                        }))
                        .child(dynamic(move || {
                            let message =
                                validation.get().or_else(|| submit_error.get().map(Text::content));
                            message.map(|message| {
                                text(move || message.resolve())
                                    .role(AccessibilityRole::Alert)
                                    .app_class(tw("text-xs text-destructive"))
                                    .into_element()
                            })
                        })),
                ))
                .child(dialog_footer(
                    dialog_state,
                    FooterVariant::Default,
                    view()
                        .app_class(tw("flex items-center justify-end gap-2"))
                        .child(button(
                            ButtonContent::Label(t("Cancel")),
                            ButtonVariant::Outline,
                            ButtonSize::Sm,
                            move || pending.get(),
                            move || {
                                open.set(false);
                                close();
                            },
                        ))
                        .child(dynamic(move || {
                            let submit = submit.clone();
                            let label = if pending.get() {
                                t("Linking...")
                            } else {
                                t("Link")
                            };
                            button(
                                ButtonContent::Label(label),
                                ButtonVariant::Default,
                                ButtonSize::Sm,
                                move || {
                                    pending.get() || resolution.with(|r| resolved_link(r).is_none())
                                },
                                submit,
                            )
                            .into_element()
                        }))
                        .into_element(),
                ))
        })
    })
    .into_element()
}
