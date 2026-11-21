//! CMP-DRAFTS: the real editor bound to a client-owned draft projection.

use crate::support::{DESKTOP, PHONE};
use futures::stream::LocalBoxStream;
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    rc::Rc,
};
use t3_client::{
    ClientError,
    thread_views::composer::web_models,
    types::{EnvironmentId, ThreadId, ThreadKey},
};
use t3_ui::screens::composer::{
    drafts::{ComposerDraftClient, ComposerDraftView, ComposerDrafts},
    editor::{
        ComposerEditor, ComposerPromptEditorProps, ComposerResolvers, EditorMode,
        composer_prompt_editor_tiptap,
    },
};

#[derive(Default)]
struct Changes(RefCell<Vec<futures::channel::mpsc::UnboundedSender<()>>>);

impl Changes {
    fn subscribe(&self) -> LocalBoxStream<'static, ()> {
        let (sender, receiver) = futures::channel::mpsc::unbounded();
        sender.unbounded_send(()).expect("initial projection");
        self.0.borrow_mut().push(sender);
        Box::pin(receiver)
    }
    fn notify(&self) {
        self.0
            .borrow_mut()
            .retain(|sender| sender.unbounded_send(()).is_ok());
    }
}

struct DraftClient {
    document: RefCell<Value>,
    changes: Changes,
    loaded: Cell<bool>,
    denied: Cell<bool>,
    revision: Cell<u64>,
    views: RefCell<BTreeMap<String, Rc<ComposerDraftView>>>,
}

impl DraftClient {
    fn key(key: &ThreadKey) -> String {
        format!("{}:{}", key.environment.0, key.thread.0)
    }
    fn notify(&self) {
        self.revision.set(self.revision.get() + 1);
        self.changes.notify();
    }
}

impl ComposerDraftClient for DraftClient {
    fn changes(&self, _: &ThreadKey) -> LocalBoxStream<'static, ()> {
        self.changes.subscribe()
    }
    fn view(&self, key: &ThreadKey) -> Rc<ComposerDraftView> {
        let key = Self::key(key);
        let document = self.document.borrow();
        let draft = &document["draftsByThreadKey"][&key];
        let next = ComposerDraftView {
            draft_key: key.clone(),
            revision: self.revision.get(),
            prompt: draft["prompt"].as_str().unwrap_or_default().into(),
            cursor: draft["cursor"].as_u64().unwrap_or_default() as usize,
            loaded: self.loaded.get(),
            writable: !self.denied.get(),
            error: None,
        };
        // The real draft service reuses an unchanged projection. A fresh Rc here
        // would make the initial subscription echo look like an external draft edit.
        let mut views = self.views.borrow_mut();
        if let Some(prior) = views.get(&key).filter(|prior| ***prior == next) {
            return prior.clone();
        }
        let next = Rc::new(next);
        views.insert(key, next.clone());
        next
    }
    fn set_prompt(
        &self,
        key: &ThreadKey,
        prompt: String,
        cursor: usize,
    ) -> Result<(), ClientError> {
        if self.denied.get() {
            return Err(ClientError::InvalidPayload("Draft is read only".into()));
        }
        let key = Self::key(key);
        web_models::set_prompt(&mut self.document.borrow_mut(), &key, &prompt);
        self.document.borrow_mut()["draftsByThreadKey"][&key]["cursor"] = json!(cursor);
        self.notify();
        Ok(())
    }
}

#[derive(Clone, PartialEq, Props)]
struct DraftHarnessProps {
    loading: bool,
}

#[component]
fn DraftHarness(props: DraftHarnessProps) -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let client = Rc::new(DraftClient {
        document: RefCell::new(json!({"draftsByThreadKey":{
            "one:thread":{"prompt":"saved 😀", "cursor":8, "threadContexts":[{"contextId":"keep"}]},
            "two:thread":{"prompt":"other draft", "cursor":11}
        }})),
        changes: Default::default(),
        loaded: Cell::new(!props.loading),
        denied: Cell::new(false),
        revision: Cell::new(0),
        views: RefCell::default(),
    });
    let environment = signal(String::from("one"));
    let mounted = signal(true);
    let view_client = client.clone();
    let denied = client.clone();
    let loaded = client.clone();
    let state = client.clone();
    let revision = signal(0usize);
    let mut changes = client.changes.subscribe();
    spawn(async move {
        while futures::StreamExt::next(&mut changes).await.is_some() {
            revision.update(|revision| *revision += 1);
        }
    });
    view()
        .w_full()
        .p(px(16.0))
        .child(dynamic(move || {
            let environment = environment.get();
            if !mounted.get() {
                return None;
            }
            let client = view_client.clone();
            Some(untracked(|| {
                let binding = ComposerDrafts::attach(
                    client,
                    ThreadKey {
                        environment: EnvironmentId(environment),
                        thread: ThreadId("thread".into()),
                    },
                );
                let editor =
                    ComposerEditor::new("", 0, EditorMode::Plain, ComposerResolvers::default())
                        .expect("editor");
                binding.bind_editor(editor.clone());
                let inspected = editor.clone();
                let source = signal(String::from("uninspected"));
                view()
                    .child(composer_prompt_editor_tiptap(
                        editor,
                        ComposerPromptEditorProps {
                            disabled: Prop::bind(move || binding.disabled()),
                            aria_label: Prop::new("Message".into()),
                            ..Default::default()
                        },
                        move |snapshot| binding.on_change(snapshot),
                        |_| {},
                    ))
                    .child(
                        nsh_base::button()
                            .accessibility_label(Some("Inspect editor".into()))
                            .child(text(String::from("Inspect editor")))
                            .on_press(move || source.set(inspected.read_snapshot().value)),
                    )
                    .child(text(move || format!("Editor text {:?}", source.get())))
                    .child(text(move || {
                        binding
                            .failure()
                            .map(|failure| failure.resolve().to_string())
                            .unwrap_or_default()
                    }))
            }))
        }))
        .child(
            nsh_base::button()
                .accessibility_label(Some("Other environment".into()))
                .child(text(String::from("Other environment")))
                .on_press(move || environment.set("two".into())),
        )
        .child(
            nsh_base::button()
                .accessibility_label(Some("First environment".into()))
                .child(text(String::from("First environment")))
                .on_press(move || environment.set("one".into())),
        )
        .child(
            nsh_base::button()
                .accessibility_label(Some("Mount".into()))
                .child(text(String::from("Mount")))
                .on_press(move || mounted.update(|mounted| *mounted = !*mounted)),
        )
        .child(
            nsh_base::button()
                .accessibility_label(Some("Load".into()))
                .child(text(String::from("Load")))
                .on_press(move || {
                    loaded.loaded.set(true);
                    loaded.notify();
                }),
        )
        .child(
            nsh_base::button()
                .accessibility_label(Some("Deny".into()))
                .child(text(String::from("Deny")))
                .on_press(move || {
                    denied.denied.set(true);
                    denied.notify();
                }),
        )
        .child(text(move || {
            revision.get();
            format!(
                "Client draft {}",
                state.document.borrow()["draftsByThreadKey"]["one:thread"]["prompt"]
                    .as_str()
                    .unwrap_or_default()
            )
        }))
}

fn start(window: (f32, f32), loading: bool) -> Headless {
    let mut registries = t3_ui::registries();
    let mut fixture = Registry::new();
    fixture.register::<DraftHarness>();
    registries.push(fixture);
    let mut app = Headless::start(EngineConfig {
        ui_sets: registries,
        clock: nsh_core::time::Clock::manual(std::time::SystemTime::UNIX_EPOCH),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::new::<DraftHarness>(move || DraftHarnessProps {
            loading,
        }))
    })
    .expect("draft harness");
    app.wait_for("id:composer-editor").expect("editor mounted");
    app.wait_for("Client draft saved 😀")
        .expect("client projection mounted");
    app
}

#[test]
fn editor_changes_survive_remount_and_do_not_cross_environment_scopes() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window, false);
        app.wait_for("saved 😀")
            .expect("client hydration reaches the editor");
        app.type_text("id:composer-editor", " typed")
            .expect("edit saved draft");
        app.wait_for("Client draft saved 😀 typed")
            .expect("editor writes the client");
        app.press("role:button[Other environment]")
            .expect("change environment");
        app.wait_for("other draft")
            .expect("separate environment draft");
        app.press("role:button[First environment]").expect("return");
        app.wait_for("saved 😀 typed")
            .expect("restored through binding");
        app.press("role:button[Mount]").expect("unmount");
        app.wait_for_gone("id:composer-editor", nsh_testing::WAIT)
            .expect("the editor unmounted");
        app.press("role:button[Mount]").expect("mount");
        app.wait_for("saved 😀 typed")
            .expect("client retains draft across mount");
        app.type_text("id:composer-editor", " again")
            .expect("edit the restored editor");
        app.wait_for("Client draft saved 😀 typed again")
            .expect("remount hydrated the editor itself");
    }
}

#[test]
fn hydration_and_permission_changes_gate_edits_without_clearing_saved_text() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window, true);
        let _ = app.type_text("id:composer-editor", "discard");
        app.press("role:button[Inspect editor]")
            .expect("inspect native document");
        app.wait_for("Editor text \"\"")
            .expect("loading editor ignored native sink input");
        assert!(app.screen().shows("Client draft saved 😀"));
        app.press("role:button[Load]").expect("finish loading");
        app.wait_for("saved 😀").expect("loaded value");
        app.type_text("id:composer-editor", " loaded")
            .expect("edit loaded content");
        app.wait_for("Client draft saved 😀 loaded")
            .expect("hydration populated the editor");
        app.press("role:button[Deny]").expect("permission lost");
        let _ = app.type_text("id:composer-editor", "discard");
        app.press("role:button[Inspect editor]")
            .expect("inspect native document");
        app.wait_for("Editor text \"saved 😀 loaded\"")
            .expect("permission loss did not edit the document");
        assert!(app.screen().shows("Client draft saved 😀 loaded"));
        assert!(!app.screen().shows("discard"));
    }
}
