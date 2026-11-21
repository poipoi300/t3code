//! Port of apps/web/src/components/chat/GuestShareControls.tsx (GuestShareDialog) and
//! guestShareDialogState.ts. The thread menu and the command palette both close as their command
//! runs, so the app root renders the dialog once. Stopping a share is the header indicator's
//! "Stop sharing", which calls the close action at once.

use std::{cell::RefCell, rc::Rc};

use futures::future::LocalBoxFuture;
use nsh_core::prelude::*;
use t3_client::types::ThreadKey;

use crate::i18n::{Text, t};
use crate::theme::tw;
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};
use crate::ui::dialog::{
    FooterVariant, PopupOptions, dialog, dialog_description, dialog_footer, dialog_header,
    dialog_panel, dialog_popup, dialog_title,
};

/// Creates a link for a thread; bound to the client in `app::provide_client_services`.
#[derive(Clone)]
pub struct GuestShareCreator(
    pub Rc<dyn Fn(ThreadKey) -> LocalBoxFuture<'static, Result<String, String>>>,
);

type Opener = Rc<dyn Fn(ThreadKey)>;

thread_local! {
    static OPENER: RefCell<Option<Opener>> = const { RefCell::new(None) };
}

/// Ask the root host to show the dialog for `key`. False when no host is mounted.
pub fn open_guest_share_dialog(key: ThreadKey) -> bool {
    OPENER.with(|slot| match slot.borrow().as_ref() {
        Some(open) => {
            open(key);
            true
        }
        None => false,
    })
}

#[derive(Clone, PartialEq)]
enum Create {
    Idle,
    Creating,
    Created(String),
    Failed(String),
}

pub fn guest_share_dialog_host() -> AnyElement {
    let target = signal(None::<ThreadKey>);
    OPENER.with(|slot| *slot.borrow_mut() = Some(Rc::new(move |key| target.set(Some(key)))));
    dynamic(move || {
        target
            .get()
            .map(|key| untracked(move || share_dialog(key, move || target.set(None))))
    })
    .into_element()
}

fn share_dialog(key: ThreadKey, close: impl Fn() + Clone + 'static) -> AnyElement {
    let creator = use_context::<GuestShareCreator>();
    let state = signal(Create::Idle);
    let open = signal(true);
    let owner = crate::runtime::tasks::TaskOwner::here();
    let replacing = use_context::<ReplacesShare>().is_some_and(|replaces| (replaces.0)(&key));
    let create = {
        let key = key.clone();
        move || {
            if state.get_untracked() == Create::Creating {
                return;
            }
            let Some(creator) = creator.clone() else {
                state.set(Create::Failed("Could not create a guest link.".into()));
                return;
            };
            state.set(Create::Creating);
            let request = (creator.0)(key.clone());
            owner.spawn(async move {
                state.set(match request.await {
                    Ok(url) => Create::Created(url),
                    Err(message) => Create::Failed(message),
                });
            });
        }
    };
    let copied = signal(false);
    let leave = close.clone();
    dialog(open, move |dialog_state| {
        let leave = leave.clone();
        dialog_state.on_open_change(move |next| {
            if !next && state.get_untracked() != Create::Creating {
                open.set(false);
                leave();
            }
        });
        let close = close.clone();
        let create = create.clone();
        dialog_popup(dialog_state, PopupOptions::default(), move || {
            let close = close.clone();
            let create = create.clone();
            view()
                .app_class(tw("flex min-h-0 w-full flex-col"))
                .child(dialog_header(
                    dialog_state,
                    view()
                        .child(dialog_title(dialog_state, || Text::from("Share with guest")))
                        .child(dialog_description(dialog_state, || {
                            Text::from(
                                "The guest sees the thread as you do, including command output and the agent's reasoning. Each message they send waits for you.",
                            )
                        })),
                ))
                .child(dialog_panel(
                    dialog_state,
                    view()
                        .app_class(tw("flex flex-col gap-2"))
                        .child(dynamic(move || match state.get() {
                            Create::Created(url) => {
                                let shown = signal(url.clone());
                                view()
                                    .app_class(tw("flex items-center gap-2"))
                                    .child(
                                        nsh_base::input()
                                            .label(t("Guest link"))
                                            .value(move || shown.get())
                                            .on_change(|_: &str| {})
                                            .app_class(tw("min-w-0 flex-1 text-sm")),
                                    )
                                    .child(dynamic(move || {
                                        let url = url.clone();
                                        let label = if copied.get() { t("Copied") } else { t("Copy") };
                                        button(
                                            ButtonContent::Label(label),
                                            ButtonVariant::Outline,
                                            ButtonSize::Sm,
                                            || false,
                                            move || {
                                                set_clipboard(url.clone());
                                                copied.set(true);
                                            },
                                        )
                                        .into_element()
                                    }))
                                    .into_element()
                            }
                            _ => text(if replacing {
                                t("A new link ends the current one.")
                            } else {
                                t("The link works once and expires in 24 hours.")
                            })
                            .app_class(tw("text-sm text-muted-foreground"))
                            .into_element(),
                        }))
                        .child(dynamic(move || match state.get() {
                            Create::Failed(message) => text(message)
                                .role(AccessibilityRole::Alert)
                                .app_class(tw("text-xs text-destructive"))
                                .into_element(),
                            _ => view().into_element(),
                        })),
                ))
                .child(dialog_footer(
                    dialog_state,
                    FooterVariant::Default,
                    dynamic(move || {
                        let close = close.clone();
                        let create = create.clone();
                        if matches!(state.get(), Create::Created(_)) {
                            return button(
                                ButtonContent::Label(t("Done")),
                                ButtonVariant::Default,
                                ButtonSize::Sm,
                                || false,
                                move || {
                                    open.set(false);
                                    close();
                                },
                            )
                            .into_element();
                        }
                        let cancel = close.clone();
                        view()
                            .app_class(tw("flex items-center justify-end gap-2"))
                            .child(button(
                                ButtonContent::Label(t("Cancel")),
                                ButtonVariant::Outline,
                                ButtonSize::Sm,
                                move || state.get() == Create::Creating,
                                move || {
                                    open.set(false);
                                    cancel();
                                },
                            ))
                            .child(button(
                                ButtonContent::Label(if state.get() == Create::Creating {
                                    t("Creating...")
                                } else {
                                    t("Create link")
                                }),
                                ButtonVariant::Default,
                                ButtonSize::Sm,
                                move || state.get() == Create::Creating,
                                create,
                            ))
                            .into_element()
                    }),
                ))
        })
    })
    .into_element()
}

/// Whether a thread already has a share, so the dialog warns that a new link replaces it.
#[derive(Clone)]
pub struct ReplacesShare(pub Rc<dyn Fn(&ThreadKey) -> bool>);
