//! Port of `apps/web/src/components/chat/ChatComposer.tsx`'s frame (`ComposerSurface` shell,
//! host, main surface, editor and footer) around CMP-EDITOR-1's prompt editor and CMP-EDITOR-2's
//! keys. Sending goes through CMP-SEND's client binding (validated submit; failed and queued
//! messages show in the timeline); the reply arrives in the store. Footer controls (model,
//! attach, primary actions) wait for their client services (`one-change/integration-2`).

use std::rc::Rc;

use futures::future::LocalBoxFuture;
use nsh_core::prelude::*;
use t3_client::operations::ComposerDispatchMode;
use t3_client::thread_views::composer::send::SendOptions;
use t3_client::types::ThreadKey;

use crate::i18n::{Text, t, ts};
use crate::runtime::backend::SendError;
use crate::runtime::rt;
use crate::screens::composer::editor::{
    ComposerEditor, ComposerPromptEditorProps, ComposerResolvers, ComposerSubmitCallback,
    EditorMode, composer_prompt_editor_tiptap,
};
use crate::screens::composer::keys::{ComposerKeyProps, composer_key_plugin};
use crate::screens::composer::send::ComposerSendClient;
use crate::theme::{palette, tw};
use crate::ui::icon::{Icon, icon};
use crate::ui::text::{TextRole, Tone, label};

pub fn composer(key: ThreadKey) -> impl IntoElement {
    let runtime = rt();
    let draft = signal(String::new());
    let error = signal(None::<String>);
    let sending = signal(false);
    let empty = move || draft.with(|d| d.trim().is_empty());
    let deliver: Rc<dyn Fn(String) -> LocalBoxFuture<'static, Result<(), SendError>>> = {
        let key = key.clone();
        let backend = runtime.backend();
        match backend.client() {
            Some(client) => Rc::new(move |text| {
                client.submit(&key, text, ComposerDispatchMode::Auto, SendOptions::default())
            }),
            None => Rc::new(move |text| backend.send_message(&key, text)),
        }
    };
    let prompt = ComposerEditor::new("", 0, EditorMode::Plain, ComposerResolvers::default())
        .expect("an empty plain-text document always builds");
    let resetting = prompt.clone();
    // Starts a send of `text`; false when there is nothing to send or a send is in flight.
    let send = Rc::new(move |text: String| -> bool {
        if text.trim().is_empty() || sending.get_untracked() {
            return false;
        }
        sending.set(true);
        error.set(None);
        let delivery = deliver(text);
        let resetting = resetting.clone();
        spawn(async move {
            match delivery.await {
                Ok(()) => {
                    draft.set(String::new());
                    if let Err(failure) = resetting.reset_after_send() {
                        error.set(Some(failure.to_string()));
                    }
                }
                Err(SendError::Disconnected) => {
                    error.set(Some(t("Not connected. Your message was not sent.")))
                }
                Err(SendError::Rejected(reason)) => {
                    error.set(Some(Text::client_key(reason).resolve().to_string()))
                }
            }
            sending.set(false);
        });
        true
    });
    // CMP-EDITOR-2's Enter handling, ahead of the editor's document adapter.
    let keys = composer_key_plugin(
        &prompt,
        ComposerKeyProps {
            disabled: Prop::bind(move || sending.get()),
            software_keyboard_open: Prop::bind(move || keyboard_height().0 > 0.0),
            ..Default::default()
        },
    );
    if let Err(failure) = prompt.set_plugins(vec![keys]) {
        error.set(Some(failure.to_string()));
    }
    let submit = send.clone();
    let editor = composer_prompt_editor_tiptap(
        prompt,
        ComposerPromptEditorProps {
            disabled: Prop::bind(move || sending.get()),
            placeholder: Prop::new(ts("Ask anything. @ files, $ skills, / commands")),
            aria_label: Prop::new(ts("Message")),
            on_submit: Prop::new(Some(ComposerSubmitCallback::new(move |snapshot| {
                submit(snapshot.value)
            }))),
            ..Default::default()
        },
        move |snapshot| draft.set(snapshot.value),
        |_| {},
    );
    let send_from_button = move || {
        send(draft.get_untracked());
    };
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
