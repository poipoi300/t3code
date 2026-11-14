//! Port of apps/web/src/components/chat/TimelinePendingApprovalRow.tsx TimelinePendingApprovalRow.

use futures::{StreamExt, future::LocalBoxFuture};
use nsh_core::prelude::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};
use t3_client::state::sharing::SharedThreadProjection;
use t3_client::thread_views::messages_timeline::{MessagesTimelineRow, MessagesTimelineRowContent};
use t3_client::thread_views::requests;
use t3_client::thread_views::thread_ops::approvals::{ApprovalMap, PendingApprovals, request_key};
use t3_client::thread_views::threads::shell::ThreadShellViews;
use t3_client::types::ThreadKey;
use t3_contracts::base_schemas::RuntimeRequestId;
use t3_contracts::orchestration_v2::{
    OrchestrationV2RuntimeRequest, OrchestrationV2TurnItem, TurnItemApprovalRequest,
};
use t3_contracts::orchestration_v2_id_table::IdOrIndex;
use t3_contracts::provider_policy::{
    ProviderApprovalDecision as Decision, ProviderApprovalFullAccessOverride,
    ProviderApprovalOption, ProviderRequestKind,
};

use crate::i18n::{Text, Value, t, tr};
use crate::runtime::{Shared, rt};
use crate::theme::{cn, tw};
use crate::ui::icon::{Icon, icon_with_class};
use crate::ui::{menu, popover, tooltip};

#[derive(Clone, Debug, PartialEq)]
pub struct ApprovalResponse {
    pub thread: ThreadKey,
    pub request_id: IdOrIndex<RuntimeRequestId>,
    pub decision: Decision,
}
pub type RespondToApproval =
    Rc<dyn Fn(ApprovalResponse) -> LocalBoxFuture<'static, Result<(), String>>>;

/// Install once at the runtime boundary; permission and execution belong to the destination client.
#[derive(Clone)]
pub struct ApprovalServices {
    pub can_operate: Rc<dyn Fn(&ThreadKey) -> bool>,
    pub respond: RespondToApproval,
}

#[derive(Clone, Copy)]
pub struct ApprovalRowsState {
    entries: Stored<BTreeMap<(String, String, String), Rc<ResponseState>>>,
    owner: Owner,
}
impl Default for ApprovalRowsState {
    fn default() -> Self {
        Self {
            entries: stored(BTreeMap::new()),
            owner: Owner::current(),
        }
    }
}
struct ResponseState {
    owner: Owner,
    responding: Signal<bool>,
    error: Signal<Option<String>>,
}
impl Drop for ResponseState {
    fn drop(&mut self) {
        self.owner.dispose();
    }
}
impl ApprovalRowsState {
    fn get(self, thread: &ThreadKey, id: &IdOrIndex<RuntimeRequestId>) -> Rc<ResponseState> {
        let key = (
            thread.environment.0.clone(),
            thread.thread.0.clone(),
            request_key(id),
        );
        self.owner.run(|| {
            self.entries.with(|entries| {
                entries
                    .entry(key)
                    .or_insert_with(|| {
                        let (owner, (responding, error)) =
                            nsh_core::element::create_root(|| (signal(false), signal(None)));
                        Rc::new(ResponseState {
                            owner,
                            responding,
                            error,
                        })
                    })
                    .clone()
            })
        })
    }
    fn prune(self, thread: &ThreadKey, pending: &ApprovalMap) {
        self.entries.with(|entries| {
            entries.retain(|(environment, id, request), _| {
                environment != &thread.environment.0
                    || id != &thread.thread.0
                    || pending.contains_key(request)
            })
        });
    }
}

#[derive(Clone)]
pub struct ApprovalContext {
    pub thread: ThreadKey,
    pub projection: Memo<Option<Shared<SharedThreadProjection>>>,
    pub pending: Memo<Shared<ApprovalMap>>,
    pub state: ApprovalRowsState,
    pub services: Option<ApprovalServices>,
    #[cfg(test)]
    derivations: Stored<usize>,
}
impl ApprovalContext {
    pub fn new(thread: ThreadKey, state: ApprovalRowsState) -> Self {
        let runtime = rt();
        let mut context =
            Self::from_projection(thread.clone(), runtime.store.thread(thread.clone()), state);
        if context.services.is_none()
            && let Some(client) = runtime.backend().client()
        {
            let mut services = crate::runtime::services::thread_ops::approval_services(&client);
            let mut changes = client.approval_services().changes(&thread);
            let revision = signal(0usize);
            spawn(async move {
                while changes.next().await.is_some() {
                    revision.update(|revision| *revision = revision.wrapping_add(1));
                }
            });
            let can_operate = services.can_operate;
            services.can_operate = Rc::new(move |key| {
                revision.get();
                can_operate(key)
            });
            context.services = Some(services);
        }
        context
    }
    pub fn from_projection(
        thread: ThreadKey,
        projection: Memo<Option<Shared<SharedThreadProjection>>>,
        state: ApprovalRowsState,
    ) -> Self {
        let selector = stored(PendingApprovals::default());
        let destination = thread.clone();
        let closing = thread.clone();
        cleanup(move || {
            let _ = state.entries.try_with(|entries| {
                entries.retain(|(environment, thread, _), _| {
                    environment != &closing.environment.0 || thread != &closing.thread.0
                });
            });
        });
        #[cfg(test)]
        let derivations = stored(0usize);
        let pending = computed(move || {
            #[cfg(test)]
            derivations.with(|count| *count += 1);
            let pending = projection.with(|projection| {
                Shared(projection.as_ref().map_or_else(Rc::default, |projection| {
                    selector.with(|selector| selector.project(projection))
                }))
            });
            state.prune(&destination, &pending);
            pending
        });
        Self {
            thread,
            projection,
            pending,
            state,
            services: {
                let services = use_context::<ApprovalServices>();
                debug_assert!(
                    services.is_some() || use_context::<crate::runtime::Rt>().is_none_or(|rt| rt.backend().client().is_none()),
                    "ApprovalServices is not provided: app::provide_client_services provides it with the client"
                );
                services
            },
            #[cfg(test)]
            derivations,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ApprovalRow {
    pub request: Rc<OrchestrationV2RuntimeRequest>,
    pub item: Option<Rc<OrchestrationV2TurnItem>>,
    pub request_kind: ProviderRequestKind,
    pub live: bool,
}
impl PartialEq for ApprovalRow {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.request, &other.request)
            && self.item.as_ref().map(Rc::as_ptr) == other.item.as_ref().map(Rc::as_ptr)
    }
}
impl ApprovalRow {
    pub fn from_row(row: &MessagesTimelineRow, pending: &ApprovalMap) -> Option<Self> {
        Self::all_from_row(row, pending).into_iter().next()
    }
    pub fn all_from_row(row: &MessagesTimelineRow, pending: &ApprovalMap) -> Vec<Self> {
        Self::all_from_map(row, pending)
    }
    pub fn all_from_map(row: &MessagesTimelineRow, pending: &ApprovalMap) -> Vec<Self> {
        let entries = match &row.content {
            MessagesTimelineRowContent::Work {
                grouped_entries, ..
            }
            | MessagesTimelineRowContent::WorkLive {
                grouped_entries, ..
            } => grouped_entries,
            _ => return Vec::new(),
        };
        entries
            .iter()
            .filter_map(|entry| {
                let OrchestrationV2TurnItem::ApprovalRequest(item) =
                    &entry.projected_item.as_ref()?.item
                else {
                    return None;
                };
                pending
                    .get(&request_key(&item.request_id))
                    .map(|row| Self::from_client(row))
            })
            .collect()
    }
    pub fn from_client(
        row: &t3_client::thread_views::thread_ops::approvals::PendingApproval,
    ) -> Self {
        Self {
            request: row.request.clone(),
            item: row.item.clone(),
            request_kind: row.request_kind,
            live: row.live,
        }
    }
    fn detail(&self) -> Option<&TurnItemApprovalRequest> {
        match self.item.as_deref() {
            Some(OrchestrationV2TurnItem::ApprovalRequest(item)) => Some(item),
            _ => None,
        }
    }
    fn options(&self) -> Vec<ProviderApprovalOption> {
        if self.detail().is_some_and(|item| item.options == Some(None)) {
            return Vec::new();
        }
        self.detail()
            .and_then(|item| item.options.as_ref().and_then(Option::as_ref))
            .cloned()
            .unwrap_or_else(|| {
                [
                    (Decision::Cancel, "Cancel"),
                    (Decision::Decline, "Decline"),
                    (Decision::AcceptForSession, "Always allow this session"),
                    (Decision::Accept, "Approve"),
                ]
                .into_iter()
                .map(|(decision, label)| ProviderApprovalOption {
                    decision,
                    label: label.into(),
                    warning: None,
                })
                .collect()
            })
    }
}

fn presentation(kind: ProviderRequestKind) -> (Icon, &'static str, &'static str) {
    match kind {
        ProviderRequestKind::Command => (Icon::Terminal, "Command approval", "Command"),
        ProviderRequestKind::FileChange => (Icon::SquarePen, "File change approval", "File change"),
        ProviderRequestKind::FileRead => (Icon::Eye, "File read approval", "File to read"),
        ProviderRequestKind::Permission => (
            Icon::Wrench,
            "App permission approval",
            "Permission request",
        ),
        ProviderRequestKind::McpElicitation => {
            (Icon::Hammer, "App access approval", "App access request")
        }
    }
}

pub fn approval_timeline_row(
    row: Memo<Option<Shared<MessagesTimelineRow>>>,
    context: ApprovalContext,
) -> impl IntoElement {
    let pending = context.pending;
    let approvals = computed(move || {
        pending.with(|pending| {
            row.with(|row| match row {
                Some(row) => ApprovalRow::all_from_map(row, pending),
                None => Vec::new(),
            })
        })
    });
    view().child(each(
        move || approvals.get(),
        |approval| serde_json::to_string(&approval.request.id).expect("request identity"),
        move |approval, _| {
            timeline_pending_approval_row(computed(move || Some(approval.get())), context.clone())
        },
    ))
}

/// Requests can arrive before their turn item. Keep those actionable without duplicating
/// requests already mounted by Work/WorkLive rows from the client projection.
pub fn unmatched_pending_approval_rows(
    rows: Memo<Shared<Vec<Rc<MessagesTimelineRow>>>>,
    context: ApprovalContext,
) -> impl IntoElement {
    let pending = context.pending;
    let mounted = computed(move || {
        rows.with(|rows| {
            rows.iter()
                .flat_map(|row| {
                    match &row.content {
                        MessagesTimelineRowContent::Work {
                            grouped_entries, ..
                        }
                        | MessagesTimelineRowContent::WorkLive {
                            grouped_entries, ..
                        } => grouped_entries.as_slice(),
                        _ => &[],
                    }
                    .iter()
                    .filter_map(|entry| {
                        match &entry.projected_item.as_ref()?.item {
                            OrchestrationV2TurnItem::ApprovalRequest(item) => {
                                Some(request_key(&item.request_id))
                            }
                            _ => None,
                        }
                    })
                })
                .collect::<BTreeSet<_>>()
        })
    });
    let approvals = computed(move || {
        pending.with(|pending| {
            if pending.is_empty() {
                return Vec::new();
            }
            t3_client::thread_views::thread_ops::approvals::ordered(pending)
                .iter()
                .filter(|pending| {
                    mounted.with(|mounted| !mounted.contains(&request_key(&pending.request.id)))
                })
                .map(|pending| ApprovalRow::from_client(pending))
                .collect::<Vec<_>>()
        })
    });
    view().child(each(
        move || approvals.get(),
        |approval| serde_json::to_string(&approval.request.id).expect("request identity"),
        move |approval, _| {
            timeline_pending_approval_row(computed(move || Some(approval.get())), context.clone())
        },
    ))
}

/// Also mounts unmatched client requests, which have no timeline entry yet.
pub fn timeline_pending_approval_row(
    row: Memo<Option<ApprovalRow>>,
    context: ApprovalContext,
) -> impl IntoElement {
    dynamic(move || {
        let waiting = t("Waiting for approval");
        row.get().map(|approval| untracked(|| {
        let (glyph, label, detail_label) = presentation(approval.request_kind);
        let item = approval.detail();
        let detail = item.and_then(|item| item.prompt.as_ref().and_then(Option::as_ref)).filter(|detail| !detail.is_empty()).cloned();
        let app = item.and_then(|item| item.app_name.clone().flatten()).filter(|name| !name.is_empty());
        let reason = item.and_then(|item| item.reason.clone().flatten()).filter(|reason| !reason.is_empty());
        let explanation = item.and_then(|item| item.full_access_override.flatten());
        let response = context.state.get(&context.thread, &approval.request.id);
        let grant_hint = unavailable_hint(&context);
        // WorkLogLine's cap-relative icon baseline has no NSH equivalent. Detail remains
        // selectable and complete; the timeline contract excludes the web's inner scroller.
        view().role(AccessibilityRole::Group).label(Text::from(label).resolve())
            .app_class(tw("group/timeline-row relative w-full min-w-0 rounded-md px-0.5 py-0.5 text-left transition-colors"))
            .child(view().app_class(tw("flex min-h-6 min-w-0 gap-1.5 text-sm leading-relaxed items-start"))
                .child(view().app_class(tw("flex min-w-0 flex-1 items-baseline gap-1.5"))
                    .child(view().app_class(tw("relative w-6 shrink-0 leading-0")).child(view().app_class(tw("inline-block size-6 overflow-hidden")).child(view().app_class(tw("flex size-full items-center justify-center")).child(icon_with_class(glyph, "block size-4 shrink-0 stroke-2 text-icon-muted")))))
                    .child(view().app_class(tw("min-w-0 flex-1 text-secondary-label")).child(text(detail.map(SharedString::from).unwrap_or_else(|| Text::from(label).resolve())).label(Text::from(detail_label).resolve()).select_text().focusable(true).app_class(tw("block min-w-0 font-mono text-xs leading-5 text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring/70"))))))
            .child(view().app_class(tw("ms-7.5 flex min-w-0 flex-col gap-1 pb-1 text-xs"))
                .child(app.map(|app| text(SharedString::from(app)).app_class(tw("text-muted-foreground"))))
                .child(reason.map(|reason| text(SharedString::from(reason)).select_text().app_class(tw("text-muted-foreground"))))
                .child(view().app_class(tw("flex flex-wrap items-center gap-x-2 gap-y-1"))
                    .tooltip(tooltip::tooltip(grant_hint, Default::default(), text(t("This connection cannot respond to approvals."))))
                    .children({
                        let mut children = Vec::new();
                        if approval.live {
                            children.push(view().app_class(tw("inline-flex items-center gap-1 text-warning")).child(icon_with_class(Icon::Shield, "size-3 shrink-0")).child(text(waiting.clone())).into_element());
                            if context.services.is_some() {
                                children.push(text(SharedString::from("·")).accessible(false).app_class(tw("text-muted-foreground")).into_element());
                                children.push(composer_pending_approval_actions(row, context.clone()).into_element());
                            }
                        } else {
                            children.push(text(t("Provider process is gone. Interrupt or restart the run to respond.")).app_class(tw("text-warning")).into_element());
                        }
                        if let Some(explanation) = explanation { children.push(override_explanation(explanation).into_element()); }
                        children
                    }))
                .child(dynamic(move || response.error.get().map(|error| text(SharedString::from(error)).role(AccessibilityRole::Alert).app_class(tw("mt-2 text-destructive"))))))
    }))
    })
}

fn disabled(row: Memo<Option<ApprovalRow>>, context: &ApprovalContext) -> bool {
    let Some(approval) = row.get() else {
        return true;
    };
    let state = context.state.get(&context.thread, &approval.request.id);
    !approval.live
        || state.responding.get()
        || context
            .services
            .as_ref()
            .is_none_or(|services| !(services.can_operate)(&context.thread))
}

fn unavailable_hint(context: &ApprovalContext) -> tooltip::TooltipHandle {
    let hint = tooltip::tooltip_create_handle();
    let context = context.clone();
    effect(move || {
        hint.set_disabled(
            context
                .services
                .as_ref()
                .is_none_or(|services| (services.can_operate)(&context.thread)),
        );
    });
    hint
}
fn respond(row: Memo<Option<ApprovalRow>>, context: &ApprovalContext, decision: Decision) {
    if disabled(row, context) {
        return;
    }
    let Some(approval) = row.get_untracked() else {
        return;
    };
    if !approval
        .options()
        .iter()
        .any(|option| option.decision == decision)
    {
        return;
    }
    let Some(services) = &context.services else {
        return;
    };
    let state = context.state.get(&context.thread, &approval.request.id);
    state.responding.set(true);
    state.error.set(None);
    let request = ApprovalResponse {
        thread: context.thread.clone(),
        request_id: approval.request.id.clone(),
        decision,
    };
    let respond = services.respond.clone();
    context.state.owner.run(|| {
        spawn(async move {
            let result = respond(request).await;
            state.responding.set(false);
            if let Err(error) = result {
                state.error.set(Some(error));
            }
        })
    });
}

/// Port of ComposerPendingApprovalActions.tsx ComposerPendingApprovalActions (inline variant).
// InlineButton's --inline-button-text-align defaults to center; native text handles wrapping.
pub fn composer_pending_approval_actions(
    row: Memo<Option<ApprovalRow>>,
    context: ApprovalContext,
) -> impl IntoElement {
    dynamic(move || {
        let more_label = t("More");
        let Some(approval) = row.get() else {
            return Vec::new();
        };
        if context.services.is_none() {
            return Vec::new();
        }
        untracked(|| {
            let provider_options = approval.detail().is_some_and(|item| item.options.is_some());
            let options = approval.options();
            let more = options
                .iter()
                .filter(|option| !matches!(option.decision, Decision::Accept | Decision::Decline))
                .cloned()
                .collect::<Vec<_>>();
            let mut children = Vec::new();
            for option in options
                .into_iter()
                .filter(|option| matches!(option.decision, Decision::Accept | Decision::Decline))
            {
                let can = context.clone();
                let action = context.clone();
                let decision = option.decision;
                let label = option_text(option.label, provider_options).resolve();
                let warning = option
                    .warning
                    .flatten()
                    .filter(|warning| !warning.is_empty());
                let content = view()
                    .app_class(tw("inline-flex items-center gap-0.5"))
                    .child(
                        warning
                            .as_ref()
                            .map(|_| icon_with_class(Icon::TriangleAlert, "size-3 shrink-0")),
                    )
                    .child(text(label.clone()));
                let control = nsh_base::button().accessibility_label(label).disabled(move || disabled(row, &can))
                    .app_class(tw(&cn([Some("inline-flex shrink-0 cursor-pointer items-center gap-0.5 font-medium text-center underline-offset-2 hover:underline focus-visible:outline-2 focus-visible:outline-ring disabled:pointer-events-none disabled:opacity-64"), Some(if decision == Decision::Accept { "text-foreground" } else { "text-muted-foreground hover:text-foreground" })])))
                    .on_press(move || respond(row, &action, decision)).child(content);
                let wrapper = view().app_class(tw("w-fit")).child(control);
                children.push(if let Some(warning) = warning {
                    wrapper
                        .tooltip(tooltip::tooltip(
                            tooltip::tooltip_create_handle(),
                            Default::default(),
                            text(Text::content(warning).resolve()),
                        ))
                        .into_element()
                } else {
                    wrapper.into_element()
                });
            }
            if !more.is_empty() {
                let can = context.clone();
                let actions = context.clone();
                let trigger = nsh_base::button().accessibility_label(t("More approval options")).disabled(move || disabled(row, &can))
                    .app_class(tw("inline-flex shrink-0 cursor-pointer items-center gap-0.5 font-medium text-center underline-offset-2 hover:underline focus-visible:outline-2 focus-visible:outline-ring disabled:pointer-events-none disabled:opacity-64 text-muted-foreground hover:text-foreground"))
                    .child(text(more_label));
                children.push(
                    menu::menu(
                        trigger,
                        menu::MenuOptions {
                            side: Placement::Top,
                            align: Align::End,
                            width: menu::Width::Content,
                            ..Default::default()
                        },
                        menu::menu_popup_with(move || {
                            let mut popup = view();
                            for option in &more {
                                let can = actions.clone();
                                let action = actions.clone();
                                let decision = option.decision;
                                let label = option_text(option.label.clone(), provider_options)
                                    .resolve()
                                    .to_string();
                                let warning = option
                                    .warning
                                    .clone()
                                    .flatten()
                                    .filter(|warning| !warning.is_empty());
                                let item = menu::menu_item(
                                    label.clone(),
                                    menu::ItemOptions {
                                        disabled: Prop::bind(move || disabled(row, &can)),
                                        ..Default::default()
                                    },
                                    view()
                                        .app_class(tw("flex min-w-0 items-center gap-2"))
                                        .child(warning.as_ref().map(|_| {
                                            icon_with_class(
                                                Icon::TriangleAlert,
                                                "size-3 text-warning",
                                            )
                                        }))
                                        .child(
                                            text(SharedString::from(label))
                                                .app_class(tw("min-w-0")),
                                        ),
                                    move || respond(row, &action, decision),
                                );
                                popup = popup.child(if let Some(warning) = warning {
                                    view()
                                        .tooltip(tooltip::tooltip(
                                            tooltip::tooltip_create_handle(),
                                            Default::default(),
                                            text(Text::content(warning).resolve()),
                                        ))
                                        .child(item)
                                        .into_element()
                                } else {
                                    item.into_element()
                                });
                            }
                            popup
                        }),
                    )
                    .into_element(),
                );
            }
            children
        })
    })
}

fn option_text(label: String, provider_supplied: bool) -> Text {
    if provider_supplied {
        Text::content(label)
    } else {
        Text::client_key(label)
    }
}

fn override_explanation(value: ProviderApprovalFullAccessOverride) -> impl IntoElement {
    let handle = popover::popover_create_handle();
    let explanation = requests::full_access_override_explanation(
        value == ProviderApprovalFullAccessOverride::AskRule,
    );
    popover::popover(
        handle,
        popover::PopoverOptions {
            tooltip_style: true,
            side: Placement::Top,
            ..Default::default()
        },
        nsh_base::button()
            .accessibility_label(t("Why am I being asked this?"))
            .on_layout(move |bounds: &Bounds<Points>| handle.measure_anchor(*bounds))
            .app_class(tw("inline-flex shrink-0 cursor-pointer items-center gap-0.5 font-medium text-center underline-offset-2 hover:underline focus-visible:outline-2 focus-visible:outline-ring disabled:pointer-events-none disabled:opacity-64 text-muted-foreground hover:text-foreground"))
            .child(text(t("Why am I being asked this?"))),
        move || text(Text::client_key(explanation).resolve()).into_element(),
    )
}

/// Port of ChildThreadApprovals.tsx ChildApproval. Entry/title come from the client selector.
pub fn child_approval(
    title: Prop<Text>,
    row: Memo<Option<ApprovalRow>>,
    context: ApprovalContext,
) -> impl IntoElement {
    dynamic(move || {
        let title = title.get().resolve().to_string();
        let heading = tr(
            "Approval needed from {title}",
            &[("title", Value::Text(title.clone()))],
        );
        let accessible = tr("Approval from {title}", &[("title", Value::Text(title))]);
        row.get().map(|approval| untracked(|| {
        let item = approval.detail();
        let detail = item.and_then(|item| item.prompt.clone().flatten()).filter(|prompt| !prompt.is_empty())
            .unwrap_or_else(|| serde_json::to_value(approval.request_kind).ok().and_then(|kind| kind.as_str().map(str::to_owned)).unwrap_or_default());
        let response = context.state.get(&context.thread, &approval.request.id);
        let grant_hint = unavailable_hint(&context);
        view().role(AccessibilityRole::Region).label(SharedString::from(accessible.clone()))
            .app_class(tw("rounded-lg border border-border bg-background p-3 text-xs"))
            .child(text(SharedString::from(heading.clone())).app_class(tw("mb-2 font-medium")))
            .child(text(SharedString::from(detail)).select_text().app_class(tw("font-mono")))
            .child(item.and_then(|item| item.app_name.clone().flatten()).map(|app| text(SharedString::from(app)).app_class(tw("mt-2 text-muted-foreground"))))
            .child((!approval.live).then(|| text(t("Provider process is gone. Interrupt or restart the child thread to respond.")).app_class(tw("mt-2 text-warning"))))
            .child(item.and_then(|item| item.reason.clone().flatten()).map(|reason| text(SharedString::from(reason)).app_class(tw("mt-2 text-muted-foreground"))))
            .child(item.and_then(|item| item.full_access_override.flatten()).map(|value| text(Text::client_key(requests::full_access_override_explanation(value == ProviderApprovalFullAccessOverride::AskRule)).resolve()).app_class(tw("mt-2 text-muted-foreground"))))
            .child(view().app_class(tw("mt-2 flex flex-wrap gap-2"))
                .tooltip(tooltip::tooltip(grant_hint, Default::default(), text(t("This connection cannot respond to approvals."))))
                .child(composer_pending_approval_actions(row, context.clone())))
            .child(dynamic(move || response.error.get().map(|error| text(SharedString::from(error)).role(AccessibilityRole::Alert).app_class(tw("mt-2 text-destructive")))))
    }))
    })
}

/// Port of ChildThreadApprovals.tsx ChildThreadApprovals. Shell relationships and pending
/// request selection are the client's; each mounted child owns a balanced client demand.
pub fn child_thread_approvals(
    parent: ThreadKey,
    shells: Memo<Shared<ThreadShellViews>>,
    state: ApprovalRowsState,
) -> impl IntoElement {
    let runtime = rt();
    let children = computed(move || {
        shells.with(|shells| {
            shells
                .child_inputs(&parent)
                .into_iter()
                .filter(|child| child.has_pending_approvals())
                .map(|child| (child.key.clone(), child.source.title.clone()))
                .collect::<Vec<_>>()
        })
    });
    // The outer timeline owns scrolling; the web's max-h-80 overflow-auto is omitted here.
    view().app_class(tw("mb-2 flex flex-col gap-2")).child(each(
        move || children.get(),
        |(key, _)| (key.environment.0.clone(), key.thread.0.clone()),
        move |child, _| {
            let key = child.get_untracked().0;
            let backend = runtime.backend();
            backend.open_thread(&key);
            let closing = key.clone();
            cleanup(move || backend.close_thread(&closing));
            let context = ApprovalContext::new(key, state);
            let pending = context.pending;
            let approvals = computed(move || {
                pending.with(|pending| {
                    t3_client::thread_views::thread_ops::approvals::ordered(pending)
                        .iter()
                        .map(|row| ApprovalRow::from_client(row))
                        .collect::<Vec<_>>()
                })
            });
            view().child(each(
                move || approvals.get(),
                |approval| serde_json::to_string(&approval.request.id).unwrap_or_default(),
                move |approval, _| {
                    let title = Prop::bind(move || Text::content(child.get().1));
                    child_approval(
                        title,
                        computed(move || Some(approval.get())),
                        context.clone(),
                    )
                },
            ))
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nsh_engine::{EngineConfig, Root};
    use nsh_testing::Headless;
    use std::cell::Cell;
    use t3_client::types::{EnvironmentId, ThreadId};
    use t3_contracts::orchestration_v2::{OrchestrationV2ThreadProjection, RuntimeRequestStatus};

    #[component]
    fn ProjectionFixture() -> impl IntoElement {
        for consumers in [1, 128] {
            let mut source: OrchestrationV2ThreadProjection = serde_json::from_str(
                include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../../fixtures/samples/OrchestrationV2ThreadProjection.jsonl"
                ))
                .lines()
                .next()
                .expect("canonical projection"),
            )
            .expect("projection");
            source.runtime_requests = vec![serde_json::from_value(serde_json::json!({
                "id":"approval","nodeId":"node","providerTurnId":null,"nativeRequestRef":null,
                "kind":"command","status":"pending","responseCapability":{"type":"live","providerSessionId":"session"},
                "createdAt":"2026-11-14T12:00:00Z","resolvedAt":null
            })).expect("canonical request")];
            source.turn_items.clear();
            let destination = ThreadKey {
                environment: EnvironmentId("environment".into()),
                thread: ThreadId("thread".into()),
            };
            let input = signal(Some(Shared(Rc::new(SharedThreadProjection::update(
                Rc::new(source.clone()),
                None,
            )))));
            let projection = computed(move || input.get());
            let state = ApprovalRowsState::default();
            let context = ApprovalContext::from_projection(destination.clone(), projection, state);
            let pending = context.pending;
            let lookups = Rc::new(Cell::new(0));
            let readers = (0..consumers)
                .map(|_| {
                    let lookups = lookups.clone();
                    computed(move || {
                        lookups.set(lookups.get() + 1);
                        pending.with(|rows| rows.get("id:approval").cloned().map(Shared))
                    })
                })
                .collect::<Vec<_>>();
            for reader in &readers {
                assert!(reader.get().is_some());
            }
            assert_eq!(
                context.derivations.get(),
                1,
                "one projection pass for {consumers} rows"
            );
            assert_eq!(lookups.get(), consumers);

            source.thread.title.0.push_str(" updated");
            let previous = input.get_untracked().expect("previous projection");
            input.set(Some(Shared(Rc::new(SharedThreadProjection::update(
                Rc::new(source.clone()),
                Some(&previous),
            )))));
            for reader in &readers {
                assert!(reader.get().is_some());
            }
            assert_eq!(
                context.derivations.get(),
                2,
                "one additional pass, independent of row count"
            );
            assert_eq!(
                lookups.get(),
                consumers,
                "unchanged approval map must not wake row lookups"
            );

            let response = state.get(&destination, &source.runtime_requests[0].id);
            let response_owner = response.owner;
            let foreign = ThreadKey {
                environment: EnvironmentId("other environment".into()),
                ..destination.clone()
            };
            let other_response = state.get(&foreign, &source.runtime_requests[0].id);
            assert_eq!(state.entries.with(|entries| entries.len()), 2);
            source.runtime_requests[0].status = RuntimeRequestStatus::Resolved;
            let previous = input.get_untracked().expect("previous projection");
            input.set(Some(Shared(Rc::new(SharedThreadProjection::update(
                Rc::new(source),
                Some(&previous),
            )))));
            assert!(pending.get().is_empty());
            assert_eq!(
                state.entries.with(|entries| entries.len()),
                1,
                "prune only this destination"
            );
            assert!(
                response_owner.is_live(),
                "an outstanding view/task can finish safely"
            );
            drop(response);
            assert!(
                !response_owner.is_live(),
                "last consumer releases request signals"
            );
            state.prune(&foreign, &ApprovalMap::new());
            drop(other_response);
            assert!(state.entries.with(|entries| entries.is_empty()));
        }
        view().child(text("projection assertions passed"))
    }

    #[test]
    fn pending_projection_work_is_independent_of_row_count_and_retired_state_is_released() {
        let mut app = Headless::start(EngineConfig {
            ui_sets: crate::registries(),
            ..EngineConfig::new(Root::of::<ProjectionFixture>())
        })
        .expect("start projection regression");
        app.first_render()
            .expect("projection and lifetime assertions");
        assert!(app.screen().shows("projection assertions passed"));
    }
}
