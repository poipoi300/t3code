//! CMP-STASH: real menu navigation over the canonical client stash record/store.
use crate::support::{DESKTOP, PHONE};
use futures::{
    FutureExt, StreamExt, channel::mpsc, future::LocalBoxFuture, stream::LocalBoxStream,
};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use std::{cell::RefCell, rc::Rc};
use t3_client::view_logic::shell::stash::{Entry, Stash, WriteOutcome};
use t3_ui::runtime::Shared;
use t3_ui::screens::composer::stash::{
    self, MenuRequest, PromptStash, PromptStashClient, StashCompletion, StashMenuProps,
    StashResult, StashSnapshot,
};

struct FixtureClient {
    store: Rc<RefCell<Stash>>,
    listeners: Rc<RefCell<Vec<mpsc::UnboundedSender<()>>>>,
    restored: Signal<String>,
}
impl FixtureClient {
    fn notify(listeners: &RefCell<Vec<mpsc::UnboundedSender<()>>>) {
        listeners
            .borrow_mut()
            .retain_mut(|sender| sender.unbounded_send(()).is_ok());
    }
}
impl PromptStashClient for FixtureClient {
    fn snapshot(&self) -> StashSnapshot {
        StashSnapshot {
            entries: self
                .store
                .borrow()
                .entries
                .iter()
                .cloned()
                .map(|e| Shared(Rc::new(e)))
                .collect(),
            can_stash: true,
            can_restore: true,
            can_delete: true,
            ..Default::default()
        }
    }
    fn changes(&self) -> LocalBoxStream<'static, ()> {
        let (send, recv) = mpsc::unbounded();
        self.listeners.borrow_mut().push(send);
        recv.boxed_local()
    }
    fn stash_current(&self) -> LocalBoxFuture<'static, StashResult> {
        panic!("saving drafts is outside this menu fixture")
    }
    fn restore(
        &self,
        id: String,
    ) -> Result<LocalBoxFuture<'static, StashResult>, stash::StashNotice> {
        let store = self.store.clone();
        let listeners = self.listeners.clone();
        let restored = self.restored;
        Ok(async move {
            let mut local = store.borrow().clone();
            let (taken, _) = local
                .take(&id, None, async |_| WriteOutcome {
                    written: true,
                    durable: true,
                })
                .await;
            *store.borrow_mut() = local;
            if let Some(entry) = taken {
                restored.set(entry.prompt);
            }
            Self::notify(&listeners);
            Ok(StashCompletion {
                notices: vec![],
                menu: MenuRequest::Close,
            })
        }
        .boxed_local())
    }
    fn delete(&self, id: String) -> LocalBoxFuture<'static, StashResult> {
        let store = self.store.clone();
        let listeners = self.listeners.clone();
        async move {
            let mut local = store.borrow().clone();
            local
                .take(&id, None, async |_| WriteOutcome {
                    written: true,
                    durable: true,
                })
                .await;
            let empty = local.entries.is_empty();
            *store.borrow_mut() = local;
            Self::notify(&listeners);
            Ok(StashCompletion {
                notices: vec![],
                menu: if empty {
                    MenuRequest::Close
                } else {
                    MenuRequest::Keep
                },
            })
        }
        .boxed_local()
    }
}
fn entry(id: &str, prompt: &str) -> Entry {
    serde_json::from_value(serde_json::json!({"id":id,"prompt":prompt,"createdAt":"2026-11-14T00:00:00Z","attachments":[],"droppedImageNames":[]})).expect("entry")
}

#[component]
fn StashFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let restored = signal(String::new());
    let client = Rc::new(FixtureClient {
        store: Rc::new(RefCell::new(Stash {
            entries: vec![entry("a", "First prompt"), entry("b", "Second prompt")],
        })),
        listeners: Rc::default(),
        restored,
    });
    let stash = PromptStash::attach(client, || {});
    let anchor = element_ref();
    let width = signal(0.0);
    let revision = signal(0_u64);
    on_key("mod+e", move |_| revision.update(|r| *r += 1));
    let allowed = signal(true);
    on_key("mod+d", move |_| {
        allowed.update(|a| *a = !*a);
        stash.state.update(|s| {
            s.can_restore = allowed.get_untracked();
            s.can_delete = allowed.get_untracked();
        });
    });
    view()
        .w_full()
        .h_full()
        .justify_end()
        .child(text(move || SharedString::from(restored.get())).test_id("restored-prompt"))
        .child(view().w_full().child(stash::composer_stash_badge(stash)))
        .child(
            view()
                .element_ref(&anchor)
                .w_full()
                .h(px(120.0))
                .on_layout(move |b: &Bounds<Points>| width.set(b.size.width.0))
                .child(text(SharedString::from("Composer"))),
        )
        .child(stash::composer_stash_menu(
            stash,
            StashMenuProps {
                anchor,
                composer_focused: signal(true),
                composer_menu_open: signal(false),
                prompt_revision: revision,
                shortcut_label: signal(Some("Ctrl+S".into())),
                composer_width: width,
            },
        ))
}
fn start(window: (f32, f32)) -> Headless {
    let mut sets = t3_ui::registries();
    let mut reg = Registry::new();
    reg.register::<StashFixture>();
    sets.push(reg);
    let mut config = EngineConfig {
        ui_sets: sets,
        clock: nsh_engine::Clock::manual(std::time::UNIX_EPOCH),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<StashFixture>())
    };
    config.environment.os = if window == PHONE {
        nsh_core::platform::Os::Android
    } else {
        nsh_core::platform::Os::Linux
    };
    config.environment.idiom = if window == PHONE {
        nsh_core::platform::Idiom::Phone
    } else {
        nsh_core::platform::Idiom::Desktop
    };
    let mut app = Headless::start(config).expect("start");
    app.wait_for("role:button[Stashed prompts: 2. Open stash.]")
        .expect("badge mounted");
    app
}
fn open(app: &mut Headless, count: u32) {
    app.press(&format!(
        "role:button[Stashed prompts: {count}. Open stash.]"
    ))
    .expect("open stash");
    app.wait_for("Close stash").expect("drawer");
}

#[test]
fn arrows_wrap_and_enter_restores_the_highlighted_entry() {
    for window in [PHONE, DESKTOP] {
        let mut app = start(window);
        open(&mut app, 2);
        app.press_key("up").expect("wrap to last");
        app.press_key("enter").expect("restore highlighted");
        app.wait_for("role:button[Stashed prompts: 1. Open stash.]")
            .expect("take reflected");
        let restored = app
            .screen()
            .layout_nodes()
            .into_iter()
            .find(|n| n.test_id.as_deref() == Some("restored-prompt"))
            .expect("restored");
        assert_eq!(
            restored.text.expect("text").text.visible_text(),
            "Second prompt"
        );
        open(&mut app, 1);
        assert_eq!(
            app.screen()
                .count("role:button[Restore stashed prompt: Second prompt]"),
            Ok(0)
        );
        assert!(app.screen().shows("First prompt"));
    }
}

#[test]
fn delete_targets_its_own_row_and_keyboard_delete_tracks_the_remaining_selection() {
    for window in [PHONE, DESKTOP] {
        let mut app = start(window);
        open(&mut app, 2);
        app.press_key("down").expect("highlight second");
        app.press("id:stash-entry-a >> role:button[Delete stashed prompt]")
            .expect("delete first without restoring it");
        app.wait_for("role:button[Stashed prompts: 1. Open stash.]")
            .expect("deleted");
        assert_eq!(
            app.screen()
                .count("role:button[Restore stashed prompt: First prompt]"),
            Ok(0)
        );
        app.press_key("mod+backspace")
            .expect("delete remaining selection");
        app.wait_for("Composer").expect("composer remains");
        assert_eq!(
            app.screen()
                .count("role:button[Stashed prompts: 1. Open stash.]"),
            Ok(0)
        );
        assert!(
            !app.screen()
                .matches("role:button[Close stash]")
                .expect("drawer state")
        );
        assert_eq!(
            app.screen().count("role:button[Delete stashed prompt]"),
            Ok(0)
        );
    }
}

#[test]
fn denied_actions_are_inert_and_escape_and_prompt_edits_close_the_menu() {
    let mut app = start(DESKTOP);
    open(&mut app, 2);
    app.press_key("mod+d").expect("revoke availability");
    assert_eq!(
        app.screen()
            .enabled("role:button[Restore stashed prompt: First prompt]"),
        Ok(false)
    );
    app.press_key("enter").expect("inert restore");
    assert!(
        app.screen()
            .matches("role:button[Close stash]")
            .expect("still open after Enter")
    );
    app.press_key("mod+backspace").expect("inert deletion");
    assert!(
        app.screen()
            .matches("role:button[Close stash]")
            .expect("drawer state")
    );
    assert!(app.screen().shows("First prompt"));
    assert!(app.screen().shows("Second prompt"));
    app.press_key("escape").expect("close");
    assert!(
        !app.screen()
            .matches("role:button[Close stash]")
            .expect("drawer state")
    );
    open(&mut app, 2);
    app.press_key("mod+e").expect("external editor edit");
    assert!(
        !app.screen()
            .matches("role:button[Close stash]")
            .expect("drawer state")
    );
}
