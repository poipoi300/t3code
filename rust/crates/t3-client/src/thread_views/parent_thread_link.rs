//! Port of apps/web/src/components/ChatView.tsx `parentThreadLink`: what MessagesTimeline's list
//! header names ("Context from" a side chat's parent, "Subagent of" a subagent's parent, or the
//! thread that delegated this one from a linked environment), and whether it can be opened.

use t3_contracts::orchestration_v2::{
    AppThreadLineageRelationshipToParent, OrchestrationV2ThreadShell,
};

use crate::types::{EnvironmentId, ThreadKey};

/// `{ title, sideChat?, environmentLabel?, open? }`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParentThreadLink {
    /// The parent's title; `None` is the client sentence "Parent thread".
    pub title: Option<String>,
    pub side_chat: bool,
    /// Set for a delegated thread: the delegating environment's label (possibly empty, as the
    /// web's `?? ""`); the detail then reads "{title} on {environment}".
    pub environment_label: Option<String>,
    /// Where "Open parent thread" goes; `None` hides the action.
    pub open: Option<ThreadKey>,
}

/// `parentThreadLink` for `thread` in `environment`. `title_of` reads a thread's title where this
/// client sees it; `environment_label` is the label of an environment this client knows
/// (`useEnvironment`), `None` when it does not. `embedded`: the side chat is shown inside its
/// parent, so the parent is not opened from it.
pub fn resolve_parent_thread_link(
    environment: &EnvironmentId,
    thread: &OrchestrationV2ThreadShell,
    embedded: bool,
    title_of: impl Fn(&ThreadKey) -> Option<String>,
    environment_label: impl Fn(&EnvironmentId) -> Option<String>,
) -> Option<ParentThreadLink> {
    let parent = thread
        .lineage
        .parent_thread_id
        .as_ref()
        .map(|id| ThreadKey {
            environment: environment.clone(),
            thread: id.clone(),
        });
    if thread.lineage.side_chat == Some(Some(true))
        && let Some(parent) = parent.clone()
    {
        return Some(ParentThreadLink {
            title: title_of(&parent),
            side_chat: true,
            environment_label: None,
            open: (!embedded).then_some(parent),
        });
    }
    if thread.lineage.relationship_to_parent == Some(AppThreadLineageRelationshipToParent::Subagent)
        && let Some(parent) = parent
    {
        return Some(ParentThreadLink {
            title: title_of(&parent),
            side_chat: false,
            environment_label: None,
            open: Some(parent),
        });
    }
    let delegated = thread.delegated_from.clone().flatten()?;
    let parent = ThreadKey {
        environment: delegated.environment_id.clone(),
        thread: delegated.thread_id.clone(),
    };
    let known = environment_label(&delegated.environment_id);
    let label = known.clone().unwrap_or_else(|| {
        thread
            .link_origin
            .clone()
            .flatten()
            .map(|origin| origin.label)
            .unwrap_or_default()
    });
    Some(ParentThreadLink {
        title: Some(title_of(&parent).unwrap_or(delegated.title)),
        side_chat: false,
        environment_label: Some(label),
        // Only an environment this client is connected to can open the parent.
        open: known.is_some().then_some(parent),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn shell(lineage: Value, extra: Value) -> OrchestrationV2ThreadShell {
        let mut thread = json!({"id":"child","projectId":"project","title":"Child","providerInstanceId":"codex",
            "modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access",
            "interactionMode":"default","branch":null,"worktreePath":null,"activeProviderThreadId":null,
            "lineage":lineage,"forkedFrom":null,"createdBy":"user","creationSource":"web",
            "createdAt":"2026-11-14T00:00:00.000Z","updatedAt":"2026-11-14T00:00:00.000Z",
            "archivedAt":null,"settledOverride":null,"settledAt":null,"lastVisitedAt":null,"deletedAt":null,
            "latestRunId":null,"activeRunId":null,"status":"idle","pendingRuntimeRequest":null,
            "latestVisibleMessage":null,"latestUserMessageAt":null,"hasActionableProposedPlan":false,
            "itemCount":0,"visibleItemCount":0});
        thread
            .as_object_mut()
            .expect("thread")
            .extend(extra.as_object().expect("extra").clone());
        serde_json::from_value(thread).expect("thread shell")
    }

    fn env(id: &str) -> EnvironmentId {
        EnvironmentId(id.into())
    }

    fn titles(key: &ThreadKey) -> Option<String> {
        (key.thread.0 == "parent").then(|| "Parser rewrite".to_owned())
    }

    #[test]
    fn a_side_chat_names_its_parent_and_opens_it_unless_embedded() {
        let side = shell(
            json!({"rootThreadId":"parent","parentThreadId":"parent","relationshipToParent":null,"sideChat":true}),
            json!({}),
        );
        let link = resolve_parent_thread_link(&env("e"), &side, false, titles, |_| None)
            .expect("side chat");
        assert!(link.side_chat);
        assert_eq!(link.title.as_deref(), Some("Parser rewrite"));
        assert_eq!(link.open.map(|key| key.thread.0), Some("parent".into()));
        let embedded = resolve_parent_thread_link(&env("e"), &side, true, |_| None, |_| None)
            .expect("embedded side chat");
        assert_eq!(embedded.title, None, "the client's \"Parent thread\"");
        assert_eq!(embedded.open, None);
    }

    #[test]
    fn a_subagent_names_its_parent_and_a_plain_thread_has_none() {
        let subagent = shell(
            json!({"rootThreadId":"parent","parentThreadId":"parent","relationshipToParent":"subagent"}),
            json!({}),
        );
        let link = resolve_parent_thread_link(&env("e"), &subagent, false, titles, |_| None)
            .expect("subagent");
        assert!(!link.side_chat);
        assert_eq!(link.open.map(|key| key.environment.0), Some("e".into()));
        let plain = shell(
            json!({"rootThreadId":"child","parentThreadId":null,"relationshipToParent":null}),
            json!({}),
        );
        assert_eq!(
            resolve_parent_thread_link(&env("e"), &plain, false, titles, |_| None),
            None
        );
    }

    #[test]
    fn a_delegated_thread_opens_only_from_a_known_environment() {
        let delegated = shell(
            json!({"rootThreadId":"child","parentThreadId":null,"relationshipToParent":null}),
            json!({"delegatedFrom":{"environmentId":"other","threadId":"remote","title":"Release prep"},
                   "linkOrigin":{"sessionId":"session","label":"Studio"}}),
        );
        let unknown = resolve_parent_thread_link(&env("e"), &delegated, false, titles, |_| None)
            .expect("delegated");
        assert_eq!(unknown.title.as_deref(), Some("Release prep"));
        assert_eq!(unknown.environment_label.as_deref(), Some("Studio"));
        assert_eq!(unknown.open, None);
        let known = resolve_parent_thread_link(&env("e"), &delegated, false, titles, |_| {
            Some("Workstation".into())
        })
        .expect("delegated");
        assert_eq!(known.environment_label.as_deref(), Some("Workstation"));
        assert_eq!(
            known.open,
            Some(ThreadKey {
                environment: env("other"),
                thread: crate::types::ThreadId("remote".into())
            })
        );
    }
}
