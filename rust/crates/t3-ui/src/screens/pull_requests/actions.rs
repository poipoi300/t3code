//! Header controls from PullRequestPrimaryAction, PullRequestStackMenu and PullRequestActionsMenu.
use super::stack::{DetailStackInput, StackMembership, detail_stack_controls};
use crate::{
    i18n::{Text, t, tr},
    runtime::tasks::{TaskOwner, watch},
    theme::tw,
    ui::{
        menu,
        toast::{ToastOptions, Variant, toast_manager},
    },
};
use futures::future::LocalBoxFuture;
use nsh_core::prelude::*;
use std::{rc::Rc, time::Duration};
use t3_client::workspace::pr_views::{
    detail::{
        ChecksState, PrimaryControlInput, allowed_merge_methods, can_perform_action,
        classify_checks, merge_method_label, primary_control, readable_failure,
        resolve_merge_method,
    },
    service::PullRequestReviewSession,
};
use t3_contracts::pull_request::*;

type SaveMethodFn =
    Rc<dyn Fn(PullRequestMergeMethod) -> LocalBoxFuture<'static, Result<(), String>>>;

#[derive(Clone, Copy)]
struct Controls {
    detail: Stored<Rc<PullRequestDetail>>,
    session: Stored<PullRequestReviewSession>,
    term: &'static str,
    pending: Signal<Option<PullRequestAction>>,
    armed: Signal<Option<PullRequestAction>>,
    open: Signal<bool>,
    writable: Signal<bool>,
    method: Memo<PullRequestMergeMethod>,
    selected: Signal<Option<PullRequestMergeMethod>>,
    remembered: Signal<PullRequestMergeMethod>,
    remembered_touched: Signal<bool>,
    save_method: Stored<SaveMethodFn>,
    stack: Signal<StackMembership>,
    stack_actions: Memo<bool>,
    list: Option<super::list::ListRefresh>,
    owner: TaskOwner,
    refresh: Signal<u64>,
}
impl Controls {
    fn single_merge_allowed(self) -> bool {
        !self.stack_actions.get() || self.stack.get() == StackMembership::Single
    }
    fn disabled(self) -> bool {
        !self.writable.get() || self.pending.with(Option::is_some)
    }
    fn label(self, action: &PullRequestAction) -> String {
        match action {
            PullRequestAction::Merge if self.pending.get() == Some(PullRequestAction::Merge) => {
                t("Merging...")
            }
            PullRequestAction::Merge if self.armed.get() == Some(PullRequestAction::Merge) => {
                t("Confirm merge?")
            }
            PullRequestAction::Merge => t(merge_method_label(&self.method.get())),
            PullRequestAction::Close if self.armed.get() == Some(PullRequestAction::Close) => {
                t("Confirm close?")
            }
            PullRequestAction::Close => tr("Close {term}", &[("term", t(self.term).into())]),
            PullRequestAction::Reopen => tr("Reopen {term}", &[("term", t(self.term).into())]),
            PullRequestAction::Draft => t("Convert to draft"),
            PullRequestAction::Ready => t("Ready for review"),
            _ => unreachable!(),
        }
    }
    fn activate(self, action: PullRequestAction) {
        let client = self.session.get();
        if self.disabled()
            || !client.can_write()
            || !self.detail.with(|d| can_perform_action(d, &action))
        {
            return;
        }
        if action == PullRequestAction::Merge && !self.single_merge_allowed() {
            return;
        }
        if matches!(action, PullRequestAction::Merge | PullRequestAction::Close)
            && self.armed.get() != Some(action.clone())
        {
            self.armed.set(Some(action));
            return;
        }
        self.armed.set(None);
        self.open.set(false);
        self.pending.set(Some(action.clone()));
        let method = (action == PullRequestAction::Merge).then(|| self.method.get());
        let toasts = toast_manager();
        self.owner.spawn(async move {
            let result = client.run_action(action.clone(), method).await;
            self.pending.try_set(None);
            match result {
                Ok(_) => {
                    if let Some(list) = self.list {
                        list.revision.update(|n| *n += 1);
                    }
                    if let Some(revision) = self.refresh.try_get() {
                        self.refresh.set(revision + 1);
                    }
                }
                Err(error) => {
                    let (title, hint) = failure_copy(&action, self.term);
                    let reason = match error.into_client_error() {
                        t3_client::ClientError::Rejected {
                            detail, message, ..
                        } => detail.unwrap_or(message),
                        _ => String::new(),
                    };
                    toasts.add(ToastOptions {
                        variant: Variant::Error,
                        title: Text::content(title),
                        description: Some(Text::content(readable_failure(&reason, &t(hint)))),
                        ..Default::default()
                    });
                }
            }
        });
    }
    fn confirmation_target(self, action: PullRequestAction, child: impl IntoElement) -> AnyElement {
        let anchor = element_ref();
        let target = action.clone();
        anchor.on_focus_out(move || {
            if self.armed.get_untracked().as_ref() == Some(&target) {
                self.armed.set(None);
            }
        });
        view()
            .element_ref(&anchor)
            .child(child)
            .child(when(
                move || self.armed.get().as_ref() == Some(&action),
                move || {
                    overlay()
                        .anchor(&anchor)
                        .on_press_outside(move |_: &Point<Points>| {
                            self.armed.set(None);
                        })
                },
            ))
            .into_element()
    }
    fn menu_action(self, action: PullRequestAction) -> AnyElement {
        let confirm = matches!(action, PullRequestAction::Close | PullRequestAction::Merge);
        let a = action.clone();
        let label = move || {
            if a == PullRequestAction::Merge && self.armed.get() != Some(PullRequestAction::Merge) {
                t("Merge now")
            } else {
                self.label(&a)
            }
        };
        // Keep the menu open for the first press. The confirmed handler closes it.
        let target = action.clone();
        let item = dynamic(move || {
            let caption = label();
            let action = action.clone();
            menu::menu_item(
                caption.clone(),
                menu::ItemOptions {
                    close_on_select: !confirm,
                    disabled: Prop::bind(move || self.disabled()),
                    ..Default::default()
                },
                text(caption),
                move || self.activate(action.clone()),
            )
        });
        if confirm {
            self.confirmation_target(target, item)
        } else {
            item.into_element()
        }
    }
}
fn failure_copy(action: &PullRequestAction, term: &str) -> (String, &'static str) {
    let name = [("term", t(term).into())];
    match action {
        PullRequestAction::Merge => (
            tr("Could not merge this {term}", &name),
            "The host refused the merge. Check that you have write access, that the checks it requires have passed, and that the branch is not conflicting.",
        ),
        PullRequestAction::Close => (
            tr("Could not close this {term}", &name),
            "The host refused it. Check that you have write access, or that you opened it.",
        ),
        PullRequestAction::Reopen => (
            tr("Could not reopen this {term}", &name),
            "The host refused it. Check that you have write access, and that the branch still exists.",
        ),
        PullRequestAction::Ready => (
            t("Could not mark this ready for review"),
            "The host refused it. Check that you have write access to this repository.",
        ),
        PullRequestAction::Draft => (
            t("Could not convert this to a draft"),
            "The host refused it. Check that you have write access to this repository.",
        ),
        _ => unreachable!("only lifecycle actions have header controls"),
    }
}
pub(super) struct ControlsInput {
    pub pr: Rc<PullRequestDetail>,
    pub client: PullRequestReviewSession,
    pub writable: Signal<bool>,
    pub refresh: Signal<u64>,
    pub supports_thread_prs: Memo<bool>,
    pub default: Memo<Option<PullRequestMergeMethod>>,
    pub remembered: Signal<PullRequestMergeMethod>,
    pub remembered_touched: Signal<bool>,
    pub save_method: SaveMethodFn,
    pub armed: Signal<Option<PullRequestAction>>,
    pub owner: TaskOwner,
    pub host_label: String,
    pub on_select_layer: Rc<dyn Fn(u64)>,
}

pub(super) fn controls(input: ControlsInput) -> AnyElement {
    let ControlsInput {
        pr,
        client,
        writable,
        refresh,
        supports_thread_prs,
        default,
        remembered,
        remembered_touched,
        save_method,
        armed,
        owner,
        host_label,
        on_select_layer,
    } = input;
    let has_stacks = pr.capabilities.stacks == Some(Some(true));
    let cap_stack_actions = pr.capabilities.stack_actions == Some(Some(true));
    let stack_actions =
        computed(move || supports_thread_prs.get() && has_stacks && cap_stack_actions);
    let stack = signal(if has_stacks && supports_thread_prs.get_untracked() {
        StackMembership::Pending
    } else {
        StackMembership::Single
    });
    let stack_revision = signal(0u64);
    let session = client.clone();
    effect(move || {
        if !has_stacks || !supports_thread_prs.get() {
            stack.set(StackMembership::Single);
            return;
        }
        stack_revision.get();
        stack.set(StackMembership::Pending);
        let session = session.clone();
        watch(async move {
            stack.set(match session.stack().await {
                Ok(Some(s)) => StackMembership::Stack(Rc::new(s)),
                Ok(None) => StackMembership::Single,
                Err(_) => StackMembership::Failed,
            });
        });
    });
    let methods = stored(allowed_merge_methods(&pr));
    let selected = signal(pr.auto_merge_method.clone().flatten());
    let method = computed(move || {
        resolve_merge_method(
            &methods.get(),
            selected.get().as_ref(),
            default.get().as_ref(),
            &remembered.get(),
        )
    });
    let (term_singular, term_plural) = if pr.provider.0 == "gitlab" {
        ("merge request", "merge requests")
    } else {
        ("pull request", "pull requests")
    };
    let list = use_context::<super::list::ListRefresh>();
    let controls_owner = list.map_or(owner, |list| list.owner);
    let c = Controls {
        detail: stored(pr.clone()),
        session: stored(client.clone()),
        term: term_singular,
        pending: signal(None),
        armed,
        open: signal(false),
        writable,
        method,
        selected,
        remembered,
        remembered_touched,
        save_method: stored(save_method),
        stack,
        stack_actions,
        list,
        owner: controls_owner,
        refresh,
    };
    effect(move || {
        if c.armed.get().is_some() {
            on_key("escape", move |_| c.armed.set(None));
            set_timeout(Duration::from_secs(3), move || c.armed.set(None));
        }
    });
    effect(move || {
        if nsh_core::element::desktop::window_state().is_some_and(|state| !state.focused) {
            c.armed.set(None);
        }
    });
    effect(move || {
        let _ = c.open.get();
        c.armed.set(None);
    });
    let can = |a| can_perform_action(&pr, &a);
    let open = pr.state == PullRequestState::Open;
    let close = open && can(PullRequestAction::Close);
    let reopen = pr.state == PullRequestState::Closed && can(PullRequestAction::Reopen);
    let draft = open && !pr.is_draft && can(PullRequestAction::Draft);
    let methods_visible = open
        && !pr.is_draft
        && pr.mergeability != PullRequestMergeability::Conflicting
        && can(PullRequestAction::Merge)
        && methods.with(|m| m.len() > 1);
    let more = tr("More {term} actions", &[("term", t(term_singular).into())]);
    let methods_available = methods.with(|m| !m.is_empty());
    let stack_pr = pr.clone();
    let stack_session = client.clone();
    let primary_pr = pr.clone();
    let primary = computed(move || {
        let checks = match classify_checks(&primary_pr.checks) {
            ChecksState::None => None,
            ChecksState::Passing => Some(PullRequestChecksState::Passing),
            ChecksState::Pending => Some(PullRequestChecksState::Pending),
            ChecksState::Failing => Some(PullRequestChecksState::Failing),
        };
        let single = !stack_actions.get() || !matches!(stack.get(), StackMembership::Stack(_));
        primary_control(PrimaryControlInput {
            state: &primary_pr.state,
            is_draft: primary_pr.is_draft,
            mergeability: &primary_pr.mergeability,
            checks: checks.as_ref(),
            auto_merge_enabled: primary_pr.auto_merge_enabled.flatten(),
            has_merge_method: methods.with(|m| !m.is_empty()),
            can_merge: single && can_perform_action(&primary_pr, &PullRequestAction::Merge),
            can_mark_ready: can_perform_action(&primary_pr, &PullRequestAction::Ready),
            can_enable_auto_merge: single
                && can_perform_action(&primary_pr, &PullRequestAction::EnableAutoMerge),
        })
    });
    let merge_now = computed(move || {
        c.single_merge_allowed()
            && matches!(
                primary.get(),
                Some("enable-auto-merge" | "auto-merge-armed")
            )
            && c.detail.with(|p| {
                !p.is_draft
                    && p.mergeability != PullRequestMergeability::Conflicting
                    && can_perform_action(p, &PullRequestAction::Merge)
            })
            && methods.with(|m| !m.is_empty())
    });
    let ready_menu = computed(move || {
        primary.get() != Some("ready")
            && c.detail.with(|p| {
                p.state == PullRequestState::Open
                    && p.is_draft
                    && can_perform_action(p, &PullRequestAction::Ready)
            })
    });
    let stack_view = dynamic(move || match stack.get() {
        StackMembership::Stack(_) => detail_stack_controls(DetailStackInput {
            detail: stack_pr.clone(),
            session: stack_session.clone(),
            stack,
            stack_revision,
            writable,
            method,
            methods_available,
            stack_actions: stack_actions.get(),
            host_label: host_label.clone(),
            term_singular,
            term_plural,
            list,
            refresh,
            owner: controls_owner,
            on_select: on_select_layer.clone(),
        }),
        StackMembership::Failed if stack_actions.get() => {
            let label = t("Retry stack lookup");
            nsh_base::button()
                .accessibility_label(label.clone())
                .on_press(move || stack_revision.update(|n| *n += 1))
                .child(text(label))
                .into_element()
        }
        _ => view().into_element(),
    });
    view()
        .app_class(tw("flex flex-row flex-wrap items-center gap-1"))
        .child(stack_view)
        .child(dynamic(move || {
            let a = match primary.get() {
                Some("merge") => PullRequestAction::Merge,
                Some("ready") => PullRequestAction::Ready,
                _ => return view().into_element(),
            };
            let click = a.clone();
            let caption = a.clone();
            let target = a.clone();
            let control = nsh_base::button()
                .accessibility_label(move || SharedString::from(c.label(&a)))
                .child(text(move || c.label(&caption)))
                .disabled(move || {
                    c.disabled() || (primary.get() == Some("merge") && !c.single_merge_allowed())
                })
                .on_press(move || c.activate(click.clone()));
            c.confirmation_target(target, control)
        }))
        .child(dynamic(move || match (primary.get(), stack.get()) {
            (Some("merge"), StackMembership::Pending) if stack_actions.get() => {
                text(t("Checking stack membership…")).into_element()
            }
            (Some("merge"), StackMembership::Failed) if stack_actions.get() => {
                text(t("Couldn't check stack membership")).into_element()
            }
            _ => view().into_element(),
        }))
        .child(menu::menu_button(
            more.clone(),
            || false,
            text(more),
            menu::MenuOptions {
                open: Some(c.open),
                ..Default::default()
            },
            menu::menu_popup_with(move || {
                let mut items = view();
                if draft {
                    items = items.child(c.menu_action(PullRequestAction::Draft));
                }
                if ready_menu.get() {
                    items = items.child(c.menu_action(PullRequestAction::Ready));
                }
                if merge_now.get() {
                    items = items.child(c.menu_action(PullRequestAction::Merge));
                }
                if methods_visible {
                    let choices = view().child(methods.get().into_iter().map(move |m| {
                        let label = t(merge_method_label(&m));
                        menu::menu_radio_item(
                            label.clone(),
                            merge_method_label(&m).into(),
                            menu::ItemOptions {
                                disabled: Prop::bind(move || c.disabled()),
                                ..Default::default()
                            },
                            text(label),
                            move || {
                                c.selected.set(Some(m.clone()));
                                c.remembered_touched.set(true);
                                c.remembered.set(m.clone());
                                let chosen = m.clone();
                                let save = c.save_method.get();
                                c.owner.spawn(async move {
                                    if let Err(err) = (save)(chosen).await {
                                        toast_manager().add(ToastOptions {
                                            variant: Variant::Error,
                                            title: Text::content(t("Could not save merge method")),
                                            description: (!err.is_empty())
                                                .then(|| Text::content(err)),
                                            ..Default::default()
                                        });
                                    }
                                });
                            },
                        )
                    }));
                    items = items.child(menu::menu_radio_group(
                        t("Merge method"),
                        move || merge_method_label(&method.get()).into(),
                        choices,
                        |_| {},
                    ));
                }
                if close {
                    items = items.child(c.menu_action(PullRequestAction::Close));
                }
                if reopen {
                    items = items.child(c.menu_action(PullRequestAction::Reopen));
                }
                items
            }),
        ))
        .into_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::FutureExt;
    use nsh_engine::{EngineConfig, Root};
    use serde_json::Value;
    use t3_client::{
        ClientStore,
        composition::{ClientRuntime, Spawner},
        testing::{CallKind, environment::FakeEnvironment},
        types::{EnvironmentId, ProjectId},
    };
    use t3_contracts::{
        base_schemas::TrimmedNonEmptyString, orchestration_v2::OrchestrationV2CreationSource,
    };

    #[component]
    fn SupportsThreadPrsFalseFixture() -> impl IntoElement {
        let root_owner = Owner::current();
        let spawner: Spawner = Rc::new(move |future| {
            root_owner.run(|| {
                spawn(future);
            });
        });
        let server = FakeEnvironment::new("forge", spawner.clone());
        server.set_permissions(&["orchestration:read", "source-control:write"]);
        for tag in ["pullRequests.stack", "pullRequests.runAction"] {
            server.register(tag, CallKind::Unary);
        }
        server.reply("pullRequests.runAction", Ok(Value::Null));

        let store = ClientStore::default();
        let client = ClientRuntime::new(
            server.platform(),
            store,
            OrchestrationV2CreationSource::Web,
            spawner,
        );
        let pair = client.pair(server.pairing());
        let paired = signal(false);
        spawn(
            async move {
                pair.await.expect("pair");
                paired.set(true);
            }
            .boxed_local(),
        );

        let pr: Rc<PullRequestDetail> = Rc::new(
            serde_json::from_str(
                r#"{
                "provider": "github",
                "host": "github.com",
                "projectId": "project",
                "projectTitle": "Project",
                "repository": "team/widget",
                "number": 14,
                "title": "Stack cap with config missing",
                "url": "https://github.com/team/widget/pull/14",
                "author": {"login": "alice", "name": null, "avatarUrl": null},
                "headBranch": "feature",
                "baseBranch": "main",
                "state": "open",
                "isDraft": false,
                "mergeability": "mergeable",
                "additions": 5,
                "deletions": 0,
                "createdAt": "2026-01-01T00:00:00Z",
                "updatedAt": "2026-01-02T00:00:00Z",
                "viewerReviewRequested": false,
                "labels": [],
                "workspaceRoot": "/project",
                "body": "Body",
                "changedFiles": 1,
                "mergedAt": null,
                "closedAt": null,
                "reviewers": [],
                "checks": [],
                "mergeCapabilities": {"merge": true, "squash": false, "rebase": false},
                "capabilities": {
                    "actions": ["merge"],
                    "mergeMethods": ["merge"],
                    "diff": true,
                    "comment": false,
                    "search": true,
                    "deleteBranch": false,
                    "stacks": true,
                    "stackActions": true,
                    "review": {"inlineComment": false, "reply": false, "resolve": false, "verdicts": []},
                    "reviewers": {"request": false, "listCandidates": false}
                },
                "viewerPermissions": {
                    "actions": ["merge"],
                    "comment": false,
                    "resolve": false,
                    "verdicts": [],
                    "requestReviewers": false,
                    "stackRebase": true
                }
            }"#,
            )
            .expect("detail json"),
        );

        let verified = signal(false);
        let check_server = server.clone();
        on_key("f9", move |_| {
            assert_eq!(
                check_server.calls("pullRequests.stack").len(),
                0,
                "pullRequests.stack must not fire when supports_thread_prs is false"
            );
            assert_eq!(check_server.calls("pullRequests.runAction").len(), 1);
            verified.set(true);
        });

        let task_owner = TaskOwner::here();
        let pr_ref = PullRequestRef {
            project_id: ProjectId("project".into()),
            host: Some(Some(TrimmedNonEmptyString("github.com".into()))),
            repository: TrimmedNonEmptyString("team/widget".into()),
            number: 14,
            expected_account_id: None,
            allow_stale: None,
        };
        let session = client.pull_request_review_session(&EnvironmentId("forge".into()), pr_ref);
        let writable = signal(true);
        let refresh = signal(0u64);
        let supports_thread_prs = computed(|| false);
        let default = computed(|| None);
        let remembered = signal(PullRequestMergeMethod::Merge);
        let remembered_touched = signal(false);
        let save_method: SaveMethodFn = Rc::new(|_| futures::future::ready(Ok(())).boxed_local());
        let armed = signal(None);

        view()
            .child(when(paired, move || {
                controls(ControlsInput {
                    pr: pr.clone(),
                    client: session.clone(),
                    writable,
                    refresh,
                    supports_thread_prs,
                    default,
                    remembered,
                    remembered_touched,
                    save_method: save_method.clone(),
                    armed,
                    owner: task_owner,
                    host_label: "GitHub".into(),
                    on_select_layer: Rc::new(|_| {}),
                })
            }))
            .child(text(move || {
                if verified.get() {
                    "Supports thread PRs gate verified"
                } else {
                    ""
                }
            }))
    }

    #[test]
    fn supports_thread_prs_false_gates_stack_lookup_and_allows_single_merge() {
        let mut sets = crate::registries();
        let mut fixtures = Registry::new();
        fixtures.register::<SupportsThreadPrsFalseFixture>();
        sets.push(fixtures);
        let mut app = nsh_testing::Headless::start(EngineConfig {
            ui_sets: sets,
            viewport: size(px(800.0), px(600.0)),
            ..EngineConfig::new(Root::of::<SupportsThreadPrsFalseFixture>())
        })
        .expect("start headless");
        app.wait_for("role:button[Merge]")
            .expect("single Merge allowed when supports_thread_prs is false");
        assert!(
            !app.screen().shows("Checking stack membership…") && !app.screen().shows("Merge stack"),
            "stack check banner and stack actions must stay suppressed when supports_thread_prs is false"
        );
        app.press("role:button[Merge]").expect("arm merge");
        app.press("role:button[Confirm merge?]")
            .expect("confirm single merge");
        app.press_key("f9").expect("assert zero stack calls");
        app.wait_for("Supports thread PRs gate verified")
            .expect("verified");
    }
}
