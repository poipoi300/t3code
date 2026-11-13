//! Port of `apps/web/src/components/chat/ChatComposer.tsx`'s frame (`ComposerSurface` shell,
//! host, main surface, editor and footer) around a native multiline input: Enter sends,
//! Shift+Enter adds a line. Footer controls (model, permissions, attach) are CMP-*'s.
//!
//! The rich composer (chips, suggestions, attachments, drafts, IME rules) is lane CMP-*; it
//! replaces this module's body. Sending goes through the backend; the reply arrives in the store.

use nsh_core::prelude::*;
use t3_client::types::ThreadKey;

use crate::i18n::t;
use crate::runtime::backend::SendError;
use crate::runtime::rt;
use crate::theme::{palette, tw};
use crate::ui::icon::{Icon, icon};
use crate::ui::text::{TextRole, Tone, label};

pub fn composer(key: ThreadKey) -> impl IntoElement {
    let runtime = rt();
    let draft = signal(String::new());
    let error = signal(None::<String>);
    let sending = signal(false);
    let empty = move || draft.with(|d| d.trim().is_empty());
    let send = {
        let key = key.clone();
        move || {
            let text = draft.get_untracked();
            if text.trim().is_empty() || sending.get_untracked() {
                return;
            }
            sending.set(true);
            error.set(None);
            let backend = runtime.backend();
            let key = key.clone();
            spawn(async move {
                match backend.send_message(&key, text).await {
                    Ok(()) => draft.set(String::new()),
                    Err(SendError::Disconnected) => {
                        error.set(Some(t("Not connected. Your message was not sent.")))
                    }
                    Err(SendError::Rejected(reason)) => error.set(Some(t(&reason))),
                }
                sending.set(false);
            });
        }
    };
    let send_from_button = send.clone();
    let editor = nsh_base::input()
        .value(move || draft.get())
        .on_change(move |text: &str| draft.set(text.to_owned()))
        .on_submit(move |_: &str| send())
        .multiline(true)
        .submit_behavior(SubmitBehavior::Submit)
        .return_key_type(ReturnKeyType::Send)
        .placeholder(t("Ask anything. @ files, $ skills, / commands"))
        .label(SharedString::from(t("Message")))
        .test_id("composer")
        // `ComposerPromptEditorTiptap`'s editor; `min-h-[calc(2lh+--spacing(2))]` hand-ported
        // for 14 px text at `leading-relaxed` (two 22.75 px lines and 8 px).
        .app_class(tw(
            "-m-1 block max-h-52 min-h-[53.5px] p-1 bg-transparent text-sm leading-relaxed text-foreground placeholder:text-placeholder",
        ));
    let send_button = nsh_base::button()
        .accessibility_label(Some(SharedString::from(t("Send message"))))
        .disabled(move || empty() || sending.get())
        .on_press(send_from_button)
        .app_class(tw(
            "relative flex h-9 w-9 items-center justify-center overflow-hidden rounded-full shadow-xs bg-message-action text-message-action-foreground hover:bg-message-action-hover disabled:bg-foreground/12 disabled:text-muted-foreground",
        ))
        .child(icon(Icon::ArrowUp, 16.0, move || {
            if empty() || sending.get() {
                palette().muted_foreground
            } else {
                palette().message_action_foreground
            }
        }));
    // `ComposerSurface.Main` and the surface inside it: the editor, then the footer.
    let surface = view()
        .app_class(tw("group relative z-10 rounded-2xl p-px"))
        .child(
            view()
                .app_class(tw("rounded-2xl"))
                .child(
                    view()
                        .app_class(tw("relative px-4 pb-2 pt-3.5 sm:pt-4"))
                        .child(editor),
                )
                .child(
                    view()
                        .app_class(tw("flex min-w-0 flex-nowrap items-center overflow-visible px-2 pb-2"))
                        .child(
                            view()
                                .app_class(tw(
                                    "flex min-w-0 flex-1 flex-nowrap items-center justify-end gap-2",
                                ))
                                .child(send_button),
                        ),
                ),
        );
    // `ComposerSurface.Shell` and `.Host`, hand-ported per ui-package.md's class table: the
    // `before:` glass is a layer behind the host, the `after:` outline the host's own border
    // (`--chat-composer-outline` per appearance), `shadow-composer` its value, `isolate` dropped.
    let shell = view()
        .app_class(tw("relative mx-auto w-full max-w-(--chat-content-max-width)"))
        .child(view().app_class(tw(
            "pointer-events-none absolute inset-0 z-0 rounded-2xl bg-card/80 dark:bg-surface-raised/80",
        )))
        .child(
            view()
                .app_class(tw(
                    "relative z-10 w-full rounded-2xl shadow-[0_1px_2px_rgb(0_0_0/5%)] dark:shadow-none border border-black/8 dark:border-white/5",
                ))
                .child(surface),
        );
    // `.chat-composer-lane` (index.css, hand-ported) around the form.
    view()
        .app_class(tw("w-full px-3 sm:px-12 pt-1"))
        .pb(move || px(12.0 + safe_area().bottom.0.max(keyboard_height().0)))
        .child(
            view()
                .app_class(tw(
                    "mx-auto w-full min-w-0 max-w-(--chat-content-max-width) gap-1.5",
                ))
                .child(dynamic(move || {
                    error.get().map(|message| {
                        label(TextRole::Caption, Tone::Destructive, move || {
                            message.clone()
                        })
                    })
                }))
                .child(shell),
        )
}
