//! Host comments and review conversations from PullRequestTimelineTab.
use crate::{
    i18n::t,
    runtime::tasks::{TaskOwner, watch},
    theme::tw,
};
use nsh_core::prelude::*;
use std::{collections::BTreeMap, rc::Rc};
use t3_client::workspace::pr_views::service::PullRequestReviewSession;
use t3_contracts::{base_schemas::TrimmedNonEmptyString, pull_request::*};

#[derive(Clone, Copy)]
pub(super) struct Conversation {
    pub(super) writable: Signal<bool>,
    activity: Signal<Option<Rc<PullRequestActivity>>>,
    drafts: Signal<BTreeMap<Option<String>, String>>,
    reply_to: Signal<Option<String>>,
    busy: Signal<bool>,
    refresh: Signal<u64>,
    owner: TaskOwner,
}

impl Conversation {
    pub fn new(session: PullRequestReviewSession) -> Self {
        use futures::StreamExt;
        let writable = signal(session.can_write());
        let mut changes = session.changes();
        watch(async move {
            while changes.next().await.is_some() {
                writable.set(session.can_write());
            }
        });
        Self {
            writable,
            activity: signal(None),
            drafts: signal(BTreeMap::new()),
            reply_to: signal(None),
            busy: signal(false),
            refresh: signal(0),
            owner: TaskOwner::here(),
        }
    }
}
pub(super) fn feedback_error(title: &'static str) {
    use crate::ui::toast::{ToastOptions, Variant, toast_manager};
    toast_manager().add(ToastOptions {
        variant: Variant::Error,
        title: crate::i18n::Text::from(title),
        ..Default::default()
    });
}
pub(super) fn panel_composer(
    session: PullRequestReviewSession,
    state: Conversation,
    pr: &PullRequestDetail,
) -> AnyElement {
    let permitted = pr.capabilities.comment && pr.viewer_permissions.comment;
    dynamic(move || {
        (permitted && state.writable.get()).then(|| composer(session.clone(), state, None))
    })
    .into_element()
}
pub(super) fn timeline(
    session: PullRequestReviewSession,
    pr: &PullRequestDetail,
    state: Conversation,
    markdown: super::markdown::PrMarkdown,
) -> AnyElement {
    let term = if pr.provider.0 == "gitlab" {
        "merge request"
    } else {
        "pull request"
    };
    let activity = state.activity;
    let read_error = signal(None::<String>);
    let read = session.clone();
    effect(move || {
        state.refresh.get();
        let read = read.clone();
        watch(async move {
            match read.activity().await {
                Ok(value) => {
                    activity.set(Some(Rc::new(value)));
                    read_error.set(None);
                }
                Err(_) => read_error.set(Some(crate::i18n::tr(
                    "Could not load {term} activity",
                    &[("term", crate::i18n::Value::from(t(term)))],
                ))),
            }
        });
    });
    let can_reply = pr.capabilities.review.reply && pr.viewer_permissions.comment;
    let replies = session.clone();
    let body = view()
        .app_class(tw("flex min-w-0 flex-col gap-3"))
        .child(dynamic(move || {
            read_error.get().map(|error| {
                view()
                    .child(text(error))
                    .child(super::detail::button(t("Retry"), move || {
                        state.refresh.update(|v| *v += 1)
                    }))
            })
        }))
        .child(dynamic(move || {
            let Some(data) = activity.get() else {
                return view().into_element();
            };
            let mut rows = view()
                .test_id("pr-conversations")
                .app_class(tw("flex min-w-0 flex-col gap-3"));
            use t3_client::workspace::pr_views::detail::{ConversationThread, conversation_items};
            let threads = data
                .review_threads
                .iter()
                .map(ConversationThread::Review)
                .chain(
                    data.overview_threads
                        .iter()
                        .flatten()
                        .flatten()
                        .map(ConversationThread::Overview),
                )
                .collect();
            let items = conversation_items(&data.comments, threads);
            if items.is_empty() {
                rows = rows.child(text(t("No comments yet.")));
            }
            for item in items {
                let mut conversation = view().app_class(tw("flex min-w-0 flex-col gap-1"));
                if let Some(thread) = &item.thread {
                    if let Some(path) = thread.path() {
                        conversation = conversation.child(text(
                            thread
                                .line()
                                .map_or_else(|| path.to_owned(), |line| format!("{path}:{line}")),
                        ));
                    }
                    if thread.is_resolved() {
                        conversation = conversation.child(text(t("Resolved")));
                    }
                    if match thread {
                        ConversationThread::Review(t) => t.is_outdated,
                        ConversationThread::Overview(t) => t.is_outdated,
                    } {
                        conversation = conversation.child(text(t("Outdated")));
                    }
                }
                for comment in &item.comments {
                    conversation =
                        conversation.child(note(comment.author.as_ref(), &comment.body, &markdown));
                }
                if let Some(thread) = item.thread {
                    let id = thread.id().to_owned();
                    conversation = conversation.test_id(&format!("pr-thread-{id}"));
                    let cursor = match &thread {
                        ConversationThread::Review(t) => &t.next_comments_cursor,
                        ConversationThread::Overview(t) => &t.next_comments_cursor,
                    };
                    if let Some(Some(cursor)) = cursor {
                        conversation = conversation.child(load_more(
                            replies.clone(),
                            state,
                            activity,
                            id.clone(),
                            cursor.clone(),
                        ));
                    } else if item.more_replies > 0 {
                        conversation = conversation.child(text(t("Showing partial conversation")));
                    }
                    if can_reply && state.writable.get() {
                        let session = replies.clone();
                        conversation = conversation.child(dynamic(move || {
                            if state.reply_to.get().as_ref() == Some(&id) {
                                composer(session.clone(), state, Some(id.clone()))
                            } else {
                                let id = id.clone();
                                super::detail::button(t("Reply"), move || {
                                    state.reply_to.set(Some(id.clone()))
                                })
                            }
                        }));
                    }
                }
                rows = rows.child(conversation);
            }
            if data.comments_truncated || data.review_threads_truncated == Some(Some(true)) {
                rows = rows.child(text(t("Showing partial activity")));
            }
            rows.into_element()
        }));
    body.into_element()
}
fn load_more(
    session: PullRequestReviewSession,
    state: Conversation,
    activity: Signal<Option<Rc<PullRequestActivity>>>,
    id: String,
    cursor: TrimmedNonEmptyString,
) -> AnyElement {
    let label = t("Load more replies");
    nsh_base::button()
        .accessibility_label(label.clone())
        .child(text(label))
        .disabled(move || state.busy.get())
        .on_press(move || {
            if state.busy.get() {
                return;
            }
            state.busy.set(true);
            let client = session.clone();
            let id = id.clone();
            let cursor = cursor.clone();
            state.owner.spawn(async move {
                match client
                    .thread_comments(TrimmedNonEmptyString(id.clone()), cursor.clone())
                    .await
                {
                    Ok(page) => activity.update(|result| {
                        let Some(data) = result.as_mut().map(Rc::make_mut) else {
                            return;
                        };
                        let target = data
                            .review_threads
                            .iter_mut()
                            .find(|t| t.id.0 == id)
                            .map(|t| (&mut t.comments, &mut t.next_comments_cursor))
                            .or_else(|| {
                                data.overview_threads
                                    .iter_mut()
                                    .flatten()
                                    .flatten()
                                    .find(|t| t.id.0 == id)
                                    .map(|t| (&mut t.comments, &mut t.next_comments_cursor))
                            });
                        if let Some((comments, next)) = target
                            && next.as_ref().and_then(Option::as_ref) == Some(&cursor)
                        {
                            *comments =
                                t3_client::workspace::pr_views::detail::merge_thread_comments(
                                    comments,
                                    &page.comments,
                                );
                            *next = page.next_cursor.filter(|c| c != &cursor).map(Some);
                        }
                    }),
                    Err(_) => feedback_error("More comments could not be loaded"),
                }
                state.busy.set(false);
            });
        })
        .into_element()
}
fn note(
    author: Option<&PullRequestActor>,
    body: &str,
    markdown: &super::markdown::PrMarkdown,
) -> AnyElement {
    view()
        .app_class(tw("flex min-w-0 flex-col gap-1"))
        .child(
            text(author.map_or_else(|| t("ghost"), |a| a.login.0.clone()))
                .app_class(tw("text-sm font-semibold")),
        )
        .child(markdown.render(body))
        .into_element()
}

fn composer(
    session: PullRequestReviewSession,
    state: Conversation,
    thread: Option<String>,
) -> AnyElement {
    let reply = thread.is_some();
    let label = t(if reply {
        "Write a reply"
    } else {
        "Leave a comment"
    });
    let submit = t(if reply { "Send reply" } else { "Comment" });
    let read = thread.clone();
    let edit = thread.clone();
    let key = thread.clone();
    view()
        .app_class(tw("flex min-w-0 flex-col gap-1"))
        .child(
            nsh_base::input()
                .label(label.clone())
                .placeholder(label)
                .multiline(true)
                .value(move || state.drafts.get().get(&read).cloned().unwrap_or_default())
                .on_change(move |v: &str| {
                    state.drafts.update(|drafts| {
                        drafts.insert(edit.clone(), v.to_owned());
                    })
                }),
        )
        .child(
            nsh_base::button()
                .accessibility_label(submit.clone())
                .child(text(submit))
                .disabled(move || {
                    state.busy.get()
                        || state
                            .drafts
                            .get()
                            .get(&key)
                            .is_none_or(|s| s.trim().is_empty())
                })
                .on_press(move || {
                    if state.busy.get() || !session.can_write() {
                        return;
                    }
                    let Some(draft) = state.drafts.get().get(&thread).cloned() else {
                        return;
                    };
                    let body = draft.trim().to_owned();
                    if body.is_empty() {
                        return;
                    }
                    state.busy.set(true);
                    let thread = thread.clone();
                    let session = session.clone();
                    state.owner.spawn(async move {
                        match session
                            .comment(thread.clone().map(TrimmedNonEmptyString), body)
                            .await
                        {
                            Ok(()) => {
                                if state.drafts.get().get(&thread) == Some(&draft) {
                                    state.drafts.update(|drafts| {
                                        drafts.remove(&thread);
                                    });
                                    if reply && state.reply_to.get() == thread {
                                        state.reply_to.set(None);
                                    }
                                }
                                state.refresh.update(|v| *v += 1);
                            }
                            Err(_) => feedback_error(if reply {
                                "Reply could not be posted"
                            } else {
                                "Could not post the comment"
                            }),
                        }
                        state.busy.set(false);
                    });
                }),
        )
        .into_element()
}
