//! GIT-2: the details card's branch selector driven through its client service binding.

use std::cell::RefCell;
use std::rc::Rc;

use futures::channel::mpsc;
use futures::{FutureExt, StreamExt, future::LocalBoxFuture, stream::LocalBoxStream};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use serde_json::json;
use t3_client::ClientError;
use t3_client::workspace::branch::EnvMode;
use t3_ui::screens::panel::branch::{
    BranchItem, BranchSelectorBackend, BranchSelectorState, branch_toolbar_branch_selector,
};

use crate::support::{DESKTOP, PHONE};

fn reference(name: &str, extra: serde_json::Value) -> t3_contracts::git::VcsRef {
    let mut value = json!({"name":name,"current":false,"isDefault":false,"worktreePath":null});
    value
        .as_object_mut()
        .expect("object")
        .extend(extra.as_object().expect("extra").clone());
    serde_json::from_value(value).expect("canonical ref")
}

fn item(name: &str, extra: serde_json::Value, disabled: Option<&str>) -> BranchItem {
    BranchItem {
        reference: reference(name, extra),
        disabled_reason: disabled.map(str::to_owned),
    }
}

fn repository() -> BranchSelectorState {
    let refs = vec![
        reference("main", json!({"current":true,"isDefault":true})),
        reference("feature", json!({})),
        reference("origin/release", json!({"isRemote":true})),
    ];
    BranchSelectorState {
        branch: Some("main".into()),
        filtered: refs
            .iter()
            .cloned()
            .map(|reference| BranchItem {
                reference,
                disabled_reason: None,
            })
            .collect(),
        refs,
        total_count: 3,
        can_write: true,
        can_operate: true,
        can_change: true,
        checkout_root: Some("/repo".into()),
        is_repo: Some(true),
        offers_git: true,
        ..Default::default()
    }
}

#[derive(Clone)]
struct FakeBranches {
    state: Rc<RefCell<Rc<BranchSelectorState>>>,
    senders: Rc<RefCell<Vec<mpsc::UnboundedSender<Rc<BranchSelectorState>>>>>,
    recorded: Signal<String>,
    refuse: Signal<bool>,
}

impl FakeBranches {
    fn change(&self, f: impl FnOnce(&mut BranchSelectorState)) {
        let mut next = (**self.state.borrow()).clone();
        f(&mut next);
        let next = Rc::new(next);
        *self.state.borrow_mut() = next.clone();
        self.senders
            .borrow_mut()
            .retain(|s| s.unbounded_send(next.clone()).is_ok());
    }
    fn answer(&self, call: String) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.recorded.set(call);
        let refused = self.refuse.get_untracked();
        async move {
            if refused {
                Err(ClientError::PermissionDenied {
                    method: "vcs.switchRef".into(),
                })
            } else {
                Ok(())
            }
        }
        .boxed_local()
    }
}

impl BranchSelectorBackend for FakeBranches {
    fn watch(&self) -> LocalBoxStream<'static, Rc<BranchSelectorState>> {
        let (sender, receiver) = mpsc::unbounded();
        let _ = sender.unbounded_send(self.state.borrow().clone());
        self.senders.borrow_mut().push(sender);
        receiver.boxed_local()
    }
    fn set_query(&self, query: String) {
        self.recorded.set(format!("query:{query}"));
        self.change(|s| s.query = query);
    }
    fn refresh(&self) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.recorded.set("refreshed".into());
        async { Ok(()) }.boxed_local()
    }
    fn load_next(&self) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.answer("next page".into())
    }
    fn select_ref(&self, name: String) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.answer(format!("selected:{name}"))
    }
    fn create_ref(&self, name: String) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.answer(format!("created:{name}"))
    }
    fn set_start_from_origin(
        &self,
        value: bool,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.change(|s| s.start_from_origin = value);
        self.answer(format!("from remote:{value}"))
    }
}

fn control(label: &'static str, action: impl Fn() + 'static) -> impl IntoElement {
    nsh_base::button()
        .accessibility_label(label)
        .on_press(action)
        .child(text(label))
}

#[component]
fn BranchFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let fake = FakeBranches {
        state: Rc::new(RefCell::new(Rc::new(repository()))),
        senders: Rc::default(),
        recorded: signal("no call".into()),
        refuse: signal(false),
    };
    let not_repo = fake.clone();
    let scratch = fake.clone();
    let creatable = fake.clone();
    let blocked = fake.clone();
    let worktree = fake.clone();
    let paged = fake.clone();
    let refuse = fake.clone();
    let renamed = fake.clone();
    t3_ui::ui::toast::toast_provider(
        view()
            .w_full()
            .h_full()
            .child(control("Not a repository", move || {
                not_repo.change(|s| s.is_repo = Some(false))
            }))
            .child(control("Scratch folder", move || {
                scratch.change(|s| s.offers_git = false)
            }))
            .child(control("Offer create", move || {
                creatable.change(|s| {
                    s.filtered.clear();
                    s.create_name = Some("new-thing".into());
                })
            }))
            .child(control("Block worktrees", move || {
                blocked.change(|s| {
                    s.filtered = vec![item(
                        "other",
                        json!({"worktreePath":"/w/other"}),
                        Some("Stop the current turn to switch worktrees."),
                    )];
                })
            }))
            .child(control("New worktree", move || {
                worktree.change(|s| {
                    s.env_mode = EnvMode::Worktree;
                    s.selecting_worktree_base = true;
                    s.branch = Some("main".into());
                })
            }))
            .child(control("More pages", move || {
                paged.change(|s| {
                    s.total_count = 120;
                    s.next_cursor = Some(100);
                })
            }))
            .child(control("Refuse", move || refuse.refuse.set(true)))
            .child(control("Checked out elsewhere", move || {
                renamed.change(|s| s.branch = Some("feature".into()))
            }))
            .child(text(move || fake.recorded.get()))
            .child(
                view()
                    .w(px(280.0))
                    .child(branch_toolbar_branch_selector(Rc::new(fake.clone()))),
            ),
        t3_ui::ui::toast::Position::TopRight,
    )
}

fn start(window: (f32, f32)) -> Headless {
    let mut app = Headless::start(EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<BranchFixture>())
    })
    .expect("branch fixture starts");
    app.wait_for("role:combobox[Branch: main]")
        .expect("current branch");
    app
}

#[test]
fn choosing_a_listed_ref_switches_through_the_client_at_both_widths() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window);
        app.press("role:combobox[Branch: main]").expect("open");
        app.wait_for("text:refreshed")
            .expect("refs refreshed on open");
        app.wait_for("role:option[feature]").expect("listed ref");
        app.wait_for("remote").expect("remote badge");
        app.type_text("id:branch-search", "feat").expect("query");
        app.wait_for("text:query:feat")
            .expect("query reaches the client");
        app.press("role:option[feature]").expect("select");
        app.wait_for("text:selected:feature")
            .expect("client switch");
        assert!(
            !app.screen().shows("Search refs..."),
            "the picker closes on selection"
        );
        app.press("role:button[Checked out elsewhere]")
            .expect("client state");
        app.wait_for("role:combobox[Branch: feature]")
            .expect("label follows the client");
    }
}

#[test]
fn closing_the_picker_clears_its_query() {
    let mut app = start(DESKTOP);
    app.press("role:combobox[Branch: main]").expect("open");
    app.type_text("id:branch-search", "rel").expect("query");
    app.press_key("escape").expect("dismiss");
    app.wait_for("text:query:").expect("query reset on close");
}

#[test]
fn a_new_ref_is_created_from_the_query_and_refusal_reports_the_web_toast() {
    let mut app = start(DESKTOP);
    app.press("role:button[Refuse]").expect("refuse");
    app.press("role:button[Offer create]").expect("create item");
    app.press("role:combobox[Branch: main]").expect("open");
    app.press("role:option[Create new ref \"new-thing\"]")
        .expect("create");
    app.wait_for("text:created:new-thing")
        .expect("client create");
    app.wait_for("Action unavailable")
        .expect("permission toast");
    app.wait_for("This connection cannot change the thread's branch.")
        .expect("reason");
}

#[test]
fn a_ref_blocked_by_a_running_turn_is_disabled_with_the_notice() {
    let mut app = start(DESKTOP);
    app.press("role:button[Block worktrees]").expect("blocked");
    app.press("role:combobox[Branch: main]").expect("open");
    app.wait_for("Stop the current turn to switch worktrees.")
        .expect("notice");
    assert!(!app.screen().enabled("role:option[other]").expect("listed"));
    let _ = app.press("role:option[other]");
    assert!(!app.screen().shows("selected:other"));
}

#[test]
fn a_new_worktree_offers_start_from_remote_and_labels_its_base() {
    let mut app = start(DESKTOP);
    app.press("role:button[New worktree]")
        .expect("worktree draft");
    app.wait_for("role:combobox[Branch: From main]")
        .expect("base label");
    app.press("role:combobox[Branch: From main]").expect("open");
    app.press("role:switch[Start worktree from remote]")
        .expect("toggle");
    app.wait_for("text:from remote:true")
        .expect("client setting");
}

#[test]
fn paging_status_and_non_repositories_follow_the_web() {
    let mut app = start(DESKTOP);
    app.press("role:button[More pages]").expect("paged");
    app.press("role:combobox[Branch: main]").expect("open");
    app.wait_for("Showing 3 of 120 refs").expect("status text");
    app.press_key("escape").expect("close");
    app.press("role:button[Not a repository]")
        .expect("not a repo");
    assert!(!app.screen().shows("Branch: main"));
    let mut scratch = start(PHONE);
    scratch
        .press("role:button[Scratch folder]")
        .expect("scratch");
    assert!(
        !scratch.screen().shows("Branch: main"),
        "the scratch folder never offers git"
    );
}
