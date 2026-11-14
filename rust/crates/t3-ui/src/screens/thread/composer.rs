//! Port of apps/web/src/components/chat/ChatComposer.tsx ChatComposer on a thread route, inside
//! ChatView.tsx's `ComposerSurface.Shell` and `.Host`. Every part is the CMP package that ports it,
//! bound to the client through `runtime/services/composer.rs`; the arrangement is the web's.

use std::cell::RefCell;
use std::rc::Rc;

use futures::StreamExt;
use nsh_core::prelude::*;
use t3_client::composition::{ClientRuntime, SendError};
use t3_client::operations::ComposerDispatchMode;
use t3_client::platform::Platform;
use t3_client::thread_views::composer::banners::BannerOutcome;
use t3_client::thread_views::composer::send::SendOptions;
use t3_client::thread_views::composer::target::ComposerTarget;
use t3_client::thread_views::composer::{context_records, reply, resolvers};
use t3_client::types::ThreadKey;
use t3_contracts::orchestration_v2_id_table::IdOrIndex;

use crate::i18n::{Text, Value, tr, ts};
use crate::keys::bindings::{Keybindings, Scope};
use crate::runtime::backend::SendError as BackendSendError;
use crate::runtime::services::composer as bindings;
use crate::runtime::{Shared, rt};
use crate::screens::composer::attachments::{
    ComposerAttachments, composer_attach_button, composer_attachment_strip,
};
use crate::screens::composer::banners::{
    ComposerBannerGroup, ComposerBannerStackItem, NoticeContent, StackOptions,
    composer_banner_stack,
};
use crate::screens::composer::chips::{ChipContext, ChipData, composer_prompt_editor_with_chips};
use crate::screens::composer::drafts::ComposerDrafts;
use crate::screens::composer::editor::{
    ComposerCommandKeyCallback, ComposerEditor, ComposerPromptEditorProps, ComposerResolvers,
    ComposerSnapshot, ComposerSubmitCallback, EditorMode, composer_prompt_editor_tiptap,
};
use crate::screens::composer::keys::{ComposerKeyProps, composer_key_plugin};
use crate::screens::composer::model_picker::{model_picker_create_handle, provider_model_picker};
use crate::screens::composer::questions::{ComposerQuestions, composer_pending_user_input_panel};
use crate::screens::composer::send::{
    ComposerSendClient, PrimaryActionState, PrimaryActionsProps, composer_primary_actions,
};
use crate::screens::composer::stash::{
    PromptStash, StashMenuProps, composer_stash_badge, composer_stash_menu,
};
use crate::screens::composer::suggestions::{ComposerSuggestions, composer_command_menu_layer};
use crate::screens::composer::voice::{ComposerSpeechButtonProps, composer_speech_button};
use crate::screens::thread::rows::assistant::{ReplyHandler, ReplyRequest};
use crate::state::layout::layout_class;
use crate::state::nav::{Route, navigator};
use crate::theme::{cn, tw};
use crate::ui::alert::composer_banner as banner;
use crate::ui::icon::Icon;
use crate::ui::toast::{self, ToastOptions};

/// The thread composer: the client's composer services, or the demo backend's plain send.
pub fn composer(key: ThreadKey) -> AnyElement {
    match rt().backend().client() {
        Some(client) => thread_composer(client, key),
        None => backend_composer(key),
    }
}

/// The production constructor over any client runtime (tests pass a `FakePlatform` runtime).
pub fn thread_composer<P: Platform + Clone + 'static>(
    client: ClientRuntime<P>,
    key: ThreadKey,
) -> AnyElement {
    let sender: Rc<dyn ComposerSendClient> = Rc::new(client.clone());
    thread_composer_with_sender(client, sender, key)
}

thread_local! {
    /// Mounted composers by thread, so the timeline's Reply can focus the one it wrote to.
    static MOUNTED: RefCell<Vec<(u64, ThreadKey, ComposerEditor)>> = const { RefCell::new(Vec::new()) };
    static NEXT_MOUNT: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

fn register_mounted(key: ThreadKey, editor: ComposerEditor) {
    let id = NEXT_MOUNT.with(|next| {
        next.set(next.get() + 1);
        next.get()
    });
    MOUNTED.with(|mounted| mounted.borrow_mut().push((id, key, editor)));
    cleanup(move || MOUNTED.with(|mounted| mounted.borrow_mut().retain(|entry| entry.0 != id)));
}

fn focus_mounted(key: &ThreadKey) {
    let editor = MOUNTED.with(|mounted| {
        mounted
            .borrow()
            .iter()
            .rev()
            .find(|entry| &entry.1 == key)
            .map(|entry| entry.2.clone())
    });
    if let Some(editor) = editor {
        editor.focus();
    }
}

/// ChatView's `replyToResponseStable`. Call it in a component body (it reads `rt()`); the handler
/// is what `AssistantRowContext.on_reply_to_response` holds. None without a client: the row then
/// hides Reply, as the web does with no handler.
pub fn reply_to_response_handler(key: &ThreadKey) -> Option<ReplyHandler> {
    let client = rt().backend().client()?;
    let key = key.clone();
    Some(Rc::new(move |request: ReplyRequest| {
        reply_to_response(&client, &key, request)
    }))
}

/// `ChatComposer.replyToResponse` behind ChatView's refusal toast.
pub fn reply_to_response<P: Platform + Clone + 'static>(
    client: &ClientRuntime<P>,
    key: &ThreadKey,
    request: ReplyRequest,
) -> Result<(), String> {
    // Rows read decoded projections; an unresolved table index cannot name the response.
    let ids = match (&request.projected_item.source_item_id, &request.message.id) {
        (IdOrIndex::Id(item), IdOrIndex::Id(message)) => Some((item.clone(), message.clone())),
        _ => None,
    };
    let accepted = match ids {
        Some((item, message)) => reply::reply_to_response(
            client,
            &request.thread_ref,
            &message,
            &request.message.text,
            item,
        )
        .map_err(|error| error.to_string())?,
        None => false,
    };
    if !accepted {
        toast::toast_manager().add(ToastOptions {
            variant: toast::Variant::Warning,
            title: "The composer is not ready".into(),
            description: Some(
                "Try replying after the connection or pending input is resolved.".into(),
            ),
            ..Default::default()
        });
        return Err("the composer is not ready".into());
    }
    focus_mounted(key);
    Ok(())
}

/// The web's `activeEnvironmentUnavailable` branch of `useComposerSend.onSend`.
fn not_connected_toast() {
    toast::toast_manager().add(toast::stacked_thread_toast(ToastOptions {
        variant: toast::Variant::Warning,
        title: "Not connected: message not sent".into(),
        description: Some(
            "Reconnecting to the environment. Try again once it is connected.".into(),
        ),
        ..Default::default()
    }));
}

fn still_sending_toast() {
    toast::toast_manager().add(toast::stacked_thread_toast(ToastOptions {
        variant: toast::Variant::Info,
        title: "Still sending your previous message.".into(),
        description: Some("Send again once it goes through or fails.".into()),
        ..Default::default()
    }));
}

fn send_shortcut(
    shortcut: &t3_contracts::settings::ClientSettingsSchemaSendShortcut,
) -> t3_client::thread_views::composer::policy::SendShortcut {
    use t3_client::thread_views::composer::policy::SendShortcut;
    use t3_contracts::settings::ClientSettingsSchemaSendShortcut as Setting;
    match shortcut {
        Setting::Enter => SendShortcut::Enter,
        Setting::ModEnter => SendShortcut::ModEnter,
        Setting::ModEnterMultiline => SendShortcut::ModEnterMultiline,
    }
}

/// A client session's view as a signal, subscribed before the first read. The client keeps
/// unchanged views' `Rc`s, so readers compare by identity.
fn watch<T: 'static>(
    mut changes: futures::stream::LocalBoxStream<'static, ()>,
    read: impl Fn() -> Rc<T> + 'static,
) -> Signal<Shared<T>> {
    let value = signal(Shared(read()));
    spawn(async move {
        while changes.next().await.is_some() {
            let next = read();
            if !value.with_untracked(|current| Rc::ptr_eq(&current.0, &next)) {
                value.set(Shared(next));
            }
        }
    });
    value
}

fn thread_composer_with_sender<P: Platform + Clone + 'static>(
    client: ClientRuntime<P>,
    sender: Rc<dyn ComposerSendClient>,
    key: ThreadKey,
) -> AnyElement {
    let keys = use_context::<Keybindings>();
    let target = ComposerTarget::Thread(key.clone());
    let drafts = ComposerDrafts::attach(bindings::composer_drafts(&client), key.clone());
    let questions = bindings::composer_questions(&client, key.clone());
    let question_session = client.pending_user_input(&key);
    let preferences = client.composer_preferences();
    let settings = {
        let source = preferences.clone();
        watch(preferences.changes(), move || source.snapshot())
    };
    let models = client.composer_models(&key);
    let model_view = {
        let source = models.clone();
        watch(models.changes(), move || source.snapshot())
    };
    let actions_session = client.composer_primary_actions(&key);
    let actions = {
        let source = actions_session.clone();
        watch(actions_session.changes(), move || source.snapshot())
    };
    let voice_session = client.composer_voice();
    let voice = {
        let source = voice_session.clone();
        watch(voice_session.changes(), move || source.snapshot())
    };
    let busy = signal(false);
    let validation = signal(None::<Text>);

    // `activePendingProgress`: the question owning the composer, from the client's session.
    let pending = computed(move || {
        questions.view.with(|view| {
            view.as_ref().map(|view| {
                let question = view.questions.get(view.question_index);
                (
                    question.is_some_and(|q| q.allow_custom_answer == Some(Some(false))),
                    question.is_some_and(|q| q.options.is_empty()),
                    view.is_responding,
                    question.is_some_and(|q| q.initial_answer.is_some()),
                )
            })
        })
    });
    let choice_only = computed(move || pending.get().is_some_and(|p| p.0));
    let mode = computed(move || {
        if pending.get().is_some_and(|p| p.3) {
            EditorMode::Literal
        } else if settings.with(|s| s.composer_rich_text_enabled) {
            EditorMode::Rich
        } else {
            EditorMode::Plain
        }
    });

    let initial = drafts.view.get_untracked();
    let resolvers = move || {
        let view = model_view.get_untracked();
        let skills = resolvers::selected_provider_skills(&view);
        let known = resolvers::model_mention_keys(&view);
        ComposerResolvers {
            skill: std::sync::Arc::new(move |name| resolvers::skill_meta(&skills, name)),
            model: std::sync::Arc::new(move |instance, model| {
                known.contains(&(instance.to_owned(), model.to_owned()))
            }),
        }
    };
    let editor = match ComposerEditor::new(
        &initial.prompt,
        initial.cursor,
        mode.get_untracked(),
        resolvers(),
    ) {
        Ok(editor) => editor,
        Err(failure) => {
            return text(Text::content(failure.to_string()).resolve())
                .role(AccessibilityRole::Alert)
                .into_element();
        }
    };
    drafts.bind_editor(editor.clone());
    register_mounted(key.clone(), editor.clone());
    {
        let editor = editor.clone();
        effect(move || {
            let next = mode.get();
            if let Err(error) = untracked(|| editor.set_mode(next)) {
                drafts.error.set(Some(Text::content(error.to_string())));
            }
        });
    }
    {
        // A skills or models refresh re-recognises chips without remounting the editor.
        let editor = editor.clone();
        effect(move || {
            model_view.get();
            if let Err(error) = untracked(|| editor.set_resolvers(resolvers())) {
                drafts.error.set(Some(Text::content(error.to_string())));
            }
        });
    }

    // CMP-SUGGEST over the client's suggestion session.
    let cwd = client
        .git_panel()
        .workspace_context(&key, None)
        .map(|context| {
            context
                .worktree_path
                .clone()
                .unwrap_or(context.project.workspace_root.0.clone())
        });
    let suggestion_session = client.composer_suggestions(&key, cwd);
    let suggestions_cell: Stored<Option<ComposerSuggestions>> = stored(None);
    let picker_handle = model_picker_create_handle();
    let query = {
        let session = suggestion_session.clone();
        move |request: Option<crate::screens::composer::suggestions::ComposerSuggestionRequest>| {
            let operation = session.query(request.as_ref().map(bindings::suggestion_request));
            spawn(async move {
                let _ = operation.await;
            });
        }
    };
    let select = {
        let session = suggestion_session.clone();
        let editor = editor.clone();
        move |id: &str,
              _trigger: &t3_client::thread_views::composer::policy::Trigger,
              snapshot: &ComposerSnapshot| {
            let Some(request) = suggestions_cell
                .get()
                .and_then(|state| state.request.get_untracked())
            else {
                return false;
            };
            // The client compares the draft with the editor's source; write the latest keystroke.
            drafts.on_change(snapshot.clone());
            match session.select(
                &bindings::suggestion_request(&request),
                id,
                &snapshot.value,
                snapshot.cursor,
            ) {
                Ok(outcome) => {
                    use t3_client::thread_views::composer::suggestions::SuggestionOutcome;
                    let (prompt, cursor) = match &outcome {
                        SuggestionOutcome::Edited { prompt, cursor }
                        | SuggestionOutcome::OpenModelPicker { prompt, cursor }
                        | SuggestionOutcome::ArmFreshSession { prompt, cursor } => {
                            (prompt.clone(), *cursor)
                        }
                    };
                    if let Err(error) = editor.sync(&prompt, cursor) {
                        drafts.error.set(Some(Text::content(error.to_string())));
                    }
                    if matches!(outcome, SuggestionOutcome::OpenModelPicker { .. }) {
                        picker_handle.set_open(true);
                    }
                    true
                }
                Err(_) => false,
            }
        }
    };
    let suggestions = ComposerSuggestions::new(editor.clone(), query, select);
    let _ = suggestions_cell.replace(Some(suggestions));
    {
        let session = suggestion_session.clone();
        let mut changes = session.changes();
        spawn(async move {
            while changes.next().await.is_some() {
                if let Some(view) = session.snapshot() {
                    suggestions.publish(bindings::suggestion_results(&view));
                }
            }
        });
    }
    effect(move || {
        let disabled = choice_only.get();
        suggestions.enabled.set(!disabled);
    });

    // CMP-CHIPS over the client's chip session.
    let chip_session = client.composer_chips(target.clone());
    let chip_view = {
        let source = chip_session.clone();
        watch(chip_session.changes(), move || {
            Rc::new(
                source
                    .snapshot()
                    .map(|view| bindings::chip_data(&view))
                    .unwrap_or_default(),
            )
        })
    };
    let chip_data = computed(move || chip_view.with(|data| ChipData::clone(data)));
    let chips = ChipContext::for_thread(chip_data, key.clone());

    // CMP-ATTACH: one client session for this draft.
    let attachments_disabled = signal(false);
    effect(move || attachments_disabled.set(drafts.disabled() || busy.get()));
    let attachments = bindings::composer_attachments(&client, &target)
        .ok()
        .map(|session| ComposerAttachments::new(session, attachments_disabled));

    // CMP-STASH.
    let stash = PromptStash::attach(bindings::prompt_stash(&client, target.clone()), {
        let editor = editor.clone();
        move || editor.focus()
    });

    // CMP-BANNERS, plus the reply banner ChatComposer adds to the client's notices.
    let banner_session = client.composer_banners(target.clone());
    let notices = {
        let source = banner_session.clone();
        watch(banner_session.changes(), move || Rc::new(source.snapshot()))
    };
    let nav = navigator();
    let invoke: Rc<dyn Fn(String, String)> = {
        let session = banner_session.clone();
        Rc::new(move |notice: String, action: String| {
            let operation = session.invoke(&notice, &action);
            spawn(async move {
                match operation.await {
                    Ok(BannerOutcome::NavigateHome) => nav.open(Route::Home),
                    Ok(BannerOutcome::Unchanged) => {}
                    Err(error) => validation.set(Some(Text::content(error.to_string()))),
                }
            });
        })
    };
    let notice_binding = Rc::new(bindings::ComposerBannerBinding::new(invoke));
    let reply_to = {
        let client = client.clone();
        let key = key.clone();
        computed(move || {
            drafts.view.get();
            reply::reply_to(&client, &key)
        })
    };
    let reply_item = {
        let client = client.clone();
        let key = key.clone();
        computed(move || {
            reply_to.get().map(|record| {
                let client = client.clone();
                let key = key.clone();
                let label = t3_client::thread_views::composer::citations::label(&record.citation);
                Shared(Rc::new(ComposerBannerStackItem {
                    id: "reply-to".into(),
                    variant: banner::Variant::Default,
                    group: ComposerBannerGroup::Attention,
                    icon: Icon::Reply,
                    custom_icon: None,
                    title: NoticeContent::from(Text::from("Replying to")),
                    // The web's description is a button that navigates to the cited response;
                    // TL-ROWS-ASSISTANT has no citation navigation yet, so it is the label alone.
                    description: Some(NoticeContent::from(Text::content(label))),
                    children: None,
                    actions: None,
                    dismiss_label: Some(Text::from("Cancel reply")),
                    on_dismiss: Some(Rc::new(move || {
                        let _ = reply::set_reply_to(&client, &key, None);
                    })),
                }))
            })
        })
    };
    let shelf_items = {
        let binding = notice_binding.clone();
        computed(move || {
            let notices = notices.get();
            let mut items = binding
                .snapshot(&notices)
                .into_iter()
                .map(Shared)
                .collect::<Vec<_>>();
            items.extend(reply_item.get());
            // An open question owns the space above the composer; only machine problems stay.
            if pending.get().is_some() {
                items.retain(|item| {
                    item.group == ComposerBannerGroup::Environment
                        && matches!(
                            item.variant,
                            banner::Variant::Error | banner::Variant::Warning
                        )
                });
            }
            items
        })
    };
    let question_shown = computed(move || pending.get().is_some());
    let banner_attached = computed(move || {
        !shelf_items.with(Vec::is_empty)
            || question_shown.get()
            || stash.state.with(|state| !state.entries.is_empty())
    });

    // CMP-SEND: the client's dispatch with the captured draft's context and attachments.
    let submit: Rc<dyn Fn(ComposerSnapshot, ComposerDispatchMode) -> bool> = {
        let client = client.clone();
        let key = key.clone();
        let editor = editor.clone();
        let sender = sender.clone();
        let question_session = question_session.clone();
        Rc::new(
            move |snapshot: ComposerSnapshot, dispatch: ComposerDispatchMode| {
                if let Some(view) = question_session.view() {
                    // Enter answers the active question, as `onAdvanceActivePendingUserInput`.
                    drafts.on_change(snapshot);
                    let Some(question) = view.questions.get(view.question_index) else {
                        return false;
                    };
                    if let Err(error) = question_session.advance(&view.request_id, &question.id.0) {
                        validation.set(Some(Text::content(error.to_string())));
                    }
                    return true;
                }
                if busy.get_untracked() {
                    still_sending_toast();
                    return true;
                }
                if actions.with_untracked(|state| state.is_environment_unavailable) {
                    not_connected_toast();
                    return true;
                }
                if snapshot.value.trim().is_empty()
                    && attachments.is_none_or(|attachments| attachments.items().is_empty())
                {
                    return false;
                }
                drafts.on_change(snapshot.clone());
                validation.set(None);
                let mut options = SendOptions::default();
                if let Ok(session) = bindings::composer_attachments(&client, &target) {
                    match session.draft_attachments() {
                        Ok(staged) => options.attachments = staged,
                        Err(error) => {
                            validation.set(Some(Text::content(error.to_string())));
                            return true;
                        }
                    }
                }
                let draft = {
                    let service = client.composer_drafts();
                    let id = service.editor_draft_key(&key);
                    let document = service.document();
                    let document = document.borrow();
                    document["draftsByThreadKey"][&id].clone()
                };
                match context_records::outgoing_message_context(&draft, &options.attachments) {
                    Ok(context) => options.context = context,
                    Err(error) => {
                        validation.set(Some(Text::content(error.to_string())));
                        return true;
                    }
                }
                busy.set(true);
                let delivery = sender.submit(&key, snapshot.value, dispatch, options);
                let editor = editor.clone();
                spawn(async move {
                    let result = delivery.await;
                    busy.set(false);
                    match result {
                        Ok(()) => {
                            if let Err(error) = editor.reset_after_send() {
                                validation.set(Some(Text::content(error.to_string())));
                            }
                        }
                        Err(SendError::Disconnected) => not_connected_toast(),
                        // Delivery refusals become the timeline's failed-message row (CMP-SEND);
                        // what is left here is the submission's own validation, as the web shows it.
                        Err(SendError::Rejected(reason)) => {
                            validation.set(Some(Text::content(reason)))
                        }
                    }
                });
                true
            },
        )
    };

    // CMP-EDITOR-2's keys, with CMP-SUGGEST's menu keys first, as composerCommandKey.ts orders them.
    let command_key =
        ComposerCommandKeyCallback::new(move |event, _in_list| suggestions.handle_key(event, None));
    let key_plugin = composer_key_plugin(
        &editor,
        ComposerKeyProps {
            send_shortcut: Prop::bind(move || {
                settings.with(|settings| send_shortcut(&settings.send_shortcut))
            }),
            software_keyboard_open: Prop::bind(move || keyboard_height().0 > 0.0),
            disabled: Prop::bind(move || drafts.disabled()),
            on_command_key_down: Prop::new(Some(command_key)),
        },
    );
    if let Err(failure) = editor.set_plugins(vec![key_plugin]) {
        drafts.error.set(Some(Text::content(failure.to_string())));
    }

    let title = {
        let key = key.clone();
        rt().store
            .shell_state(key.environment.clone(), move |shell| {
                shell
                    .and_then(|shell| shell.snapshot.as_ref())
                    .and_then(|snapshot| snapshot.threads.iter().find(|row| row.id == key.thread))
                    .map(|row| row.title.clone())
            })
    };
    let placeholder = computed(move || match pending.get() {
        Some((true, ..)) => ts("Choose an option above"),
        Some((false, true, ..)) => ts("Type your answer"),
        Some(_) => ts("Type your own answer, or leave this blank to use the selected option"),
        None => ts("Ask anything. @ files, $ skills, / commands"),
    });
    let editor_disabled = computed(move || {
        choice_only.get() || pending.get().is_some_and(|p| p.2) || drafts.disabled()
    });
    let on_submit = {
        let submit = submit.clone();
        ComposerSubmitCallback::new(move |snapshot| submit(snapshot, ComposerDispatchMode::Auto))
    };
    let props = ComposerPromptEditorProps {
        disabled: Prop::bind(move || editor_disabled.get()),
        placeholder: Prop::bind(move || placeholder.get()),
        // Switching threads lands focus here, so the label names the thread.
        aria_label: Prop::bind(move || match title.get() {
            Some(title) => {
                SharedString::from(tr("Message {title}", &[("title", Value::from(title))]))
            }
            None => ts("Message"),
        }),
        context_count: Prop::bind(move || chip_data.with(|data| data.records.len())),
        on_submit: Prop::new(Some(on_submit)),
        ..Default::default()
    };
    let on_change = move |snapshot: ComposerSnapshot| {
        drafts.on_change(snapshot);
        suggestions.observe();
    };
    let prompt =
        composer_prompt_editor_with_chips(editor.clone(), props, chips, on_change, move |paste| {
            if let Some(attachments) = attachments {
                attachments.paste_files(paste);
            }
        });

    // SH-KEYS focus facts: digits and other shortcuts never take keys typed into the editor.
    let editor_boundary = element_ref();
    if let Some(keys) = keys {
        keys.context.focus_within(editor_boundary, "editableFocus");
        editor_boundary.on_focus_in(move || {
            keys.context.set(
                "composerRichTextFocus",
                mode.get_untracked() == EditorMode::Rich,
            )
        });
        editor_boundary.on_focus_out(move || keys.context.set("composerRichTextFocus", false));
    }
    // `composerMenuAnchor`: the body's element and measured width anchor the menus.
    let anchor = element_ref();
    let anchor_width = signal(0.0_f32);
    let anchor_width_memo = computed(move || anchor_width.get());
    let list_id = format!(
        "composer-suggestions-{}-{}",
        key.environment.0, key.thread.0
    );
    let composer_focused = signal(false);
    let prompt_revision = signal(0_u64);
    effect(move || {
        drafts.view.get();
        prompt_revision.update(|revision| *revision += 1);
    });
    let menu_open = signal(false);
    effect(move || menu_open.set(suggestions.trigger.get().is_some()));
    let stash_shortcut = signal(None::<SharedString>);
    if let Some(keys) = keys {
        effect(move || {
            keys.rules.get();
            stash_shortcut.set(
                keys.shortcut_label("composer.stash")
                    .map(SharedString::from),
            );
        });
    }

    let body = view()
        .element_ref(&anchor)
        .on_layout(move |bounds: &Bounds<Points>| anchor_width.set(bounds.size.width.0))
        .app_class(tw("relative px-4 pb-2 pt-3.5 sm:pt-4"))
        .child(composer_stash_menu(
            stash,
            StashMenuProps {
                anchor,
                composer_focused,
                composer_menu_open: menu_open,
                prompt_revision,
                shortcut_label: stash_shortcut,
                composer_width: anchor_width,
            },
        ))
        .child(composer_command_menu_layer(
            suggestions,
            anchor,
            anchor_width_memo,
            list_id,
        ))
        .child(attachments.map(composer_attachment_strip))
        .child(
            view()
                .element_ref(&editor_boundary)
                .app_class(tw("relative"))
                .child(prompt),
        );

    // `ComposerPromptLengthValidation`, with the draft binding's failure as the web's store errors.
    let validation_line = dynamic(move || {
        validation
            .get()
            .or_else(|| drafts.failure())
            .or_else(|| questions.error.get())
            .or_else(|| attachments.and_then(ComposerAttachments::error))
            .map(|message| {
                text(move || message.resolve())
                    .role(AccessibilityRole::Alert)
                    .app_class(tw("px-3 pb-2 text-xs text-destructive sm:px-4"))
            })
    });

    // `composerControls` (ComposerControlsBar): the model and traits picker.
    let picker = provider_model_picker(bindings::thread_model_picker(&client, &key, picker_handle));

    // CMP-SEND's action group, with CMP-VOICE in its children slot when the client offers it.
    let action_state = computed(move || {
        let mut state: PrimaryActionState = bindings::primary_action_state(&actions.get());
        state.voice_input_active =
            voice.with(|voice| voice.state.phase != t3_client::voice::VoicePhase::Idle);
        state.is_send_busy = state.is_send_busy || busy.get();
        state.has_sendable_content = state.has_sendable_content || state.voice_input_active;
        state
    });
    let submit_from_action = {
        let submit = submit.clone();
        let editor = editor.clone();
        move |dispatch: ComposerDispatchMode| {
            submit(editor.read_snapshot(), dispatch);
        }
    };
    let mut primary =
        PrimaryActionsProps::new(Prop::bind(move || action_state.get()), submit_from_action);
    primary.on_interrupt = Some({
        let client = client.clone();
        let key = key.clone();
        Rc::new(move || {
            let operation = client.interrupt_thread(&key);
            spawn(async move {
                if let Err(error) = operation.await {
                    validation.set(Some(Text::content(error.to_string())));
                }
            });
        })
    });
    primary.on_previous_pending_question = Some({
        let question_session = question_session.clone();
        Rc::new(move || {
            if let Some(view) = question_session.view()
                && let Some(question) = view.questions.get(view.question_index)
                && let Err(error) = question_session.previous(&view.request_id, &question.id.0)
            {
                validation.set(Some(Text::content(error.to_string())));
            }
        })
    });
    primary.composer_surface_width = Prop::bind(move || anchor_width.get());
    primary.preserve_composer_focus_on_pointer_down = layout_class().is_compact();
    if let Some(keys) = keys {
        primary.alternate_shortcut_label = Prop::bind(move || {
            keys.rules.get();
            keys.shortcut_label("composer.sendAlternate")
        });
    }
    primary.children = Some(Rc::new(move || {
        let session = voice_session.clone();
        dynamic(move || {
            let view = voice.get();
            view.available.then(|| {
                let start = session.clone();
                untracked(|| {
                    composer_speech_button(ComposerSpeechButtonProps {
                        state: Prop::bind(move || voice.get().state.clone()),
                        disabled: Prop::bind(move || drafts.disabled()),
                        compact: false,
                        available: Prop::new(true),
                        unavailable_reason: Prop::new(Text::from(
                            "Voice transcription is not available.",
                        )),
                        shortcut: Prop::new(None),
                        on_start: Rc::new(move || {
                            let operation = start.start();
                            spawn(async move {
                                let _ = operation.await;
                            });
                        }),
                    })
                })
            })
        })
        .into_element()
    }));

    let attach = attachments.map(|attachments| {
        let editor = editor.clone();
        view()
            .app_class(tw("me-2"))
            .child(composer_attach_button(attachments, move || {
                let _ = editor.focus_at_end();
            }))
    });
    let footer = view()
        .app_class(move || {
            tw(&cn([
                Some("flex min-w-0 flex-nowrap items-center overflow-visible px-2 pb-2"),
                question_shown.get().then_some("pt-2"),
            ]))
        })
        .child(attach)
        .child(
            view()
                .app_class(tw(
                    "flex min-w-0 flex-1 flex-nowrap items-center justify-end gap-2",
                ))
                .child(
                    view()
                        .app_class(tw(
                            "relative flex min-w-0 flex-1 items-center justify-end gap-0.5",
                        ))
                        .child(picker),
                )
                .child(composer_primary_actions(primary)),
        );

    // ComposerBanner.Dock: notices, then the top drawer holding the open question; the stash badge.
    let dock = banner::dock(
        banner::AttachmentProps::default(),
        move || banner_attached.get(),
        view()
            .app_class(tw("flex w-full items-end gap-1"))
            .child(banner::column(
                move || shelf_items.with(Vec::is_empty) && !question_shown.get(),
                view()
                    .app_class(tw("flex min-w-0 flex-col"))
                    .child(composer_banner_stack(
                        move || shelf_items.get(),
                        StackOptions {
                            detached: false,
                            in_column: true,
                            last_attachment: true,
                        },
                    ))
                    .child(dynamic(move || {
                        question_shown.get().then(|| {
                            untracked(|| {
                                banner::attachment(
                                    banner::AttachmentProps {
                                        in_column: true,
                                        last: true,
                                    },
                                    banner::root(
                                        banner::Props {
                                            surface: banner::SurfaceProps {
                                                variant: banner::Variant::Info,
                                                ..Default::default()
                                            },
                                            ..Default::default()
                                        },
                                        move |_| composer_pending_user_input_panel(questions),
                                    ),
                                )
                            })
                        })
                    })),
            ))
            .child(composer_stash_badge(stash)),
    );

    // `ComposerSurface.Main` and the surface inside it. With an attached banner the Shell's glass
    // and Host's outline are hidden and Main paints its own (`group-has-data-[composer-banner-surface=attached]`).
    let main = view()
        .app_class(move || {
            tw(&cn([
                Some("group relative z-10 rounded-2xl p-px"),
                banner_attached
                    .get()
                    .then_some("bg-card/80 dark:bg-surface-raised/80 shadow-[0_1px_2px_rgb(0_0_0/5%)] dark:shadow-none"),
            ]))
        })
        .child(
            view()
                .app_class(tw("rounded-2xl transition-[background-color] duration-200"))
                .child(body)
                .child(validation_line)
                .child(footer),
        )
        .child(dynamic(move || {
            banner_attached.get().then(|| {
                view().app_class(tw(
                    "pointer-events-none absolute inset-0 z-20 rounded-2xl border border-black/8 dark:border-white/5",
                ))
            })
        }));
    let form = view()
        .app_class(tw(
            "mx-auto w-full min-w-0 max-w-(--chat-content-max-width)",
        ))
        .child(dock)
        .child(view().app_class(tw("relative")).child(main));
    if let Some(keys) = keys {
        let form_ref = element_ref();
        keys.context.focus_within(form_ref, "composerFocus");
        form_ref.on_focus_in(move || composer_focused.set(true));
        form_ref.on_focus_out(move || composer_focused.set(false));
        install_composer_keys(keys, questions, stash);
        return composer_frame(form.element_ref(&form_ref), banner_attached);
    }
    composer_frame(form, banner_attached)
}

/// The question panel's digit keys and the stash command, under this composer's owner.
fn install_composer_keys(keys: Keybindings, questions: ComposerQuestions, stash: PromptStash) {
    keys.register(Scope {
        when: Rc::new(move |event| {
            !event.key.editable_focus && questions.view.with(Option::is_some)
        }),
        raw_keys: (1..=9)
            .flat_map(|digit| {
                t3_client::view_logic::shell::keybindings::parse_shortcut(&digit.to_string())
            })
            .collect(),
        on_keydown: Some(Rc::new(move |event| {
            questions.handle_key(event, false, false)
        })),
        ..Default::default()
    });
    keys.register(Scope::default().command("composer.stash", move |event| {
        event.claim();
        stash.stash_current();
        true
    }));
}

/// `ComposerSurface.Shell` and `.Host`, hand-ported per ui-package.md's class table: the `before:`
/// glass is a layer behind the host, the `after:` outline the host's own border
/// (`--chat-composer-outline` per appearance), `shadow-composer` its value, `isolate` dropped.
/// Both are hidden while a banner is attached; Main then paints them.
fn composer_frame(form: impl IntoElement, banner_attached: Memo<bool>) -> AnyElement {
    let shell = view()
        .app_class(tw("relative mx-auto w-full max-w-(--chat-content-max-width)"))
        .child(dynamic(move || {
            (!banner_attached.get()).then(|| {
                view().app_class(tw(
                    "pointer-events-none absolute inset-0 z-0 rounded-2xl bg-card/80 dark:bg-surface-raised/80",
                ))
            })
        }))
        .child(
            view()
                .app_class(move || {
                    tw(if banner_attached.get() {
                        "relative z-10 w-full rounded-2xl"
                    } else {
                        "relative z-10 w-full rounded-2xl shadow-[0_1px_2px_rgb(0_0_0/5%)] dark:shadow-none border border-black/8 dark:border-white/5"
                    })
                })
                .child(view().app_class(tw("relative z-10")).child(form)),
        );
    // `.chat-composer-lane` (index.css, hand-ported) around the shell.
    view()
        .app_class(tw("w-full px-3 sm:px-12 pt-1"))
        .pb(move || px(12.0 + safe_area().bottom.0.max(keyboard_height().0)))
        .child(shell)
        .into_element()
}

/// The demo backend has no client services: the editor and send, through `Backend::send_message`.
fn backend_composer(key: ThreadKey) -> AnyElement {
    let backend = rt().backend();
    let sending = signal(false);
    let failure = signal(None::<Text>);
    let editor = match ComposerEditor::new("", 0, EditorMode::Plain, ComposerResolvers::default()) {
        Ok(editor) => editor,
        Err(error) => {
            return text(Text::content(error.to_string()).resolve()).into_element();
        }
    };
    let draft = signal(String::new());
    let submit: Rc<dyn Fn(String) -> bool> = {
        let editor = editor.clone();
        Rc::new(move |value: String| {
            if value.trim().is_empty() || sending.get_untracked() {
                return false;
            }
            sending.set(true);
            failure.set(None);
            let delivery = backend.send_message(&key, value);
            let editor = editor.clone();
            spawn(async move {
                match delivery.await {
                    Ok(()) => {
                        draft.set(String::new());
                        if let Err(error) = editor.reset_after_send() {
                            failure.set(Some(Text::content(error.to_string())));
                        }
                    }
                    Err(BackendSendError::Disconnected) => not_connected_toast(),
                    Err(BackendSendError::Rejected(reason)) => {
                        failure.set(Some(Text::content(reason)))
                    }
                }
                sending.set(false);
            });
            true
        })
    };
    let keys = composer_key_plugin(
        &editor,
        ComposerKeyProps {
            disabled: Prop::bind(move || sending.get()),
            software_keyboard_open: Prop::bind(move || keyboard_height().0 > 0.0),
            ..Default::default()
        },
    );
    if let Err(error) = editor.set_plugins(vec![keys]) {
        failure.set(Some(Text::content(error.to_string())));
    }
    let on_submit = {
        let submit = submit.clone();
        ComposerSubmitCallback::new(move |snapshot| submit(snapshot.value))
    };
    let prompt = composer_prompt_editor_tiptap(
        editor,
        ComposerPromptEditorProps {
            disabled: Prop::bind(move || sending.get()),
            placeholder: Prop::new(ts("Ask anything. @ files, $ skills, / commands")),
            aria_label: Prop::new(ts("Message")),
            on_submit: Prop::new(Some(on_submit)),
            ..Default::default()
        },
        move |snapshot| draft.set(snapshot.value),
        |_| {},
    );
    let state = computed(move || PrimaryActionState {
        can_operate_thread: true,
        is_send_busy: sending.get(),
        has_sendable_content: draft.with(|draft| !draft.trim().is_empty()),
        ..Default::default()
    });
    let actions = composer_primary_actions(PrimaryActionsProps::new(
        Prop::bind(move || state.get()),
        move |_| {
            submit(draft.get_untracked());
        },
    ));
    let form = view()
        .app_class(tw("mx-auto w-full min-w-0 max-w-(--chat-content-max-width)"))
        .child(
            view().app_class(tw("relative")).child(
                view()
                    .app_class(tw("group relative z-10 rounded-2xl p-px"))
                    .child(
                        view()
                            .app_class(tw("rounded-2xl"))
                            .child(
                                view()
                                    .app_class(tw("relative px-4 pb-2 pt-3.5 sm:pt-4"))
                                    .child(prompt),
                            )
                            .child(dynamic(move || {
                                failure.get().map(|message| {
                                    text(move || message.resolve())
                                        .role(AccessibilityRole::Alert)
                                        .app_class(tw("px-3 pb-2 text-xs text-destructive sm:px-4"))
                                })
                            }))
                            .child(
                                view()
                                    .app_class(tw(
                                        "flex min-w-0 flex-nowrap items-center overflow-visible px-2 pb-2",
                                    ))
                                    .child(
                                        view()
                                            .app_class(tw(
                                                "flex min-w-0 flex-1 flex-nowrap items-center justify-end gap-2",
                                            ))
                                            .child(actions),
                                    ),
                            ),
                    ),
            ),
        );
    composer_frame(form, computed(|| false))
}
