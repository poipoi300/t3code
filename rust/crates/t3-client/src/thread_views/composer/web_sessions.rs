//! Draft sessions keep logical-project mappings separate from their reserved server thread ids.
use super::web_drafts::{finalize_promotion, mark_promoting, promotion_ready};
use crate::{thread_views::drafts::web_has_user_content, view_logic::JsTrim};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use t3_contracts::{
    environment::{ScopedProjectRef, ScopedThreadRef},
    provider_policy::DEFAULT_RUNTIME_MODE,
};

fn string<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or_default()
}
fn thread_key(reference: &ScopedThreadRef) -> String {
    format!("{}:{}", reference.environment_id.0, reference.thread_id.0)
}
pub enum Target<'a> {
    Draft(&'a str),
    Thread(&'a ScopedThreadRef),
}
pub fn resolve(state: &Value, target: Target<'_>) -> Option<String> {
    match target {
        Target::Draft(id) => (!id.js_trim().is_empty()).then(|| id.js_trim().into()),
        Target::Thread(reference) => {
            let key = thread_key(reference);
            if state["draftsByThreadKey"].get(&key).is_some() {
                return Some(key);
            }
            state["draftThreadsByThreadKey"]
                .as_object()
                .and_then(|sessions| {
                    sessions
                        .iter()
                        .find(|(_, session)| {
                            session["environmentId"] == reference.environment_id.0
                                && session["threadId"] == reference.thread_id.0
                        })
                        .map(|(id, _)| id.clone())
                })
                .or(Some(key))
        }
    }
}
pub fn by_logical_project<'a>(state: &'a Value, logical: &str) -> Option<(&'a str, &'a Value)> {
    let id =
        state["logicalProjectDraftThreadKeyByLogicalProjectKey"][logical.js_trim()].as_str()?;
    let session = state["draftThreadsByThreadKey"].get(id)?;
    session["promotedTo"].is_null().then_some((id, session))
}
pub fn by_project<'a>(
    state: &'a Value,
    project: &ScopedProjectRef,
) -> Option<(&'a str, &'a Value)> {
    let sessions = state["draftThreadsByThreadKey"].as_object()?;
    let matches = |session: &Value| {
        session["projectId"] == project.project_id.0
            && session["environmentId"] == project.environment_id.0
            && session["promotedTo"].is_null()
    };
    for id in state["logicalProjectDraftThreadKeyByLogicalProjectKey"]
        .as_object()
        .into_iter()
        .flat_map(|map| map.values())
        .filter_map(Value::as_str)
    {
        if let Some(session) = sessions.get(id).filter(|session| matches(session)) {
            return Some((id, session));
        }
    }
    sessions
        .iter()
        .find(|(_, session)| matches(session))
        .map(|(id, session)| (id.as_str(), session))
}
fn make_session(
    project: &ScopedProjectRef,
    id: &str,
    logical: &str,
    previous: &Value,
    options: &Value,
    now: &str,
    context: bool,
) -> Value {
    let changed = previous.is_object()
        && (previous["environmentId"] != project.environment_id.0
            || previous["projectId"] != project.project_id.0);
    let mut next = previous.as_object().cloned().unwrap_or_default();
    let value = |field: &str, default: Value| {
        options
            .get(field)
            .filter(|value| !value.is_null())
            .or_else(|| previous.get(field).filter(|value| !value.is_null()))
            .cloned()
            .unwrap_or(default)
    };
    next.insert(
        "threadId".into(),
        if context {
            previous["threadId"].clone()
        } else {
            value("threadId", json!(id))
        },
    );
    next.insert("environmentId".into(), json!(project.environment_id));
    next.insert("projectId".into(), json!(project.project_id));
    next.insert("logicalProjectKey".into(), json!(logical));
    for field in ["branch", "worktreePath"] {
        next.insert(
            field.into(),
            options.get(field).cloned().unwrap_or_else(|| {
                if changed {
                    Value::Null
                } else {
                    previous[field].clone()
                }
            }),
        );
    }
    next.insert(
        "createdAt".into(),
        if context && options["createdAt"] == "" {
            previous["createdAt"].clone()
        } else {
            value("createdAt", json!(now))
        },
    );
    next.insert(
        "runtimeMode".into(),
        value("runtimeMode", json!(DEFAULT_RUNTIME_MODE)),
    );
    next.insert(
        "interactionMode".into(),
        value("interactionMode", json!("default")),
    );
    next.insert(
        "startFromOrigin".into(),
        value("startFromOrigin", json!(false)),
    );
    let env_mode = if next["worktreePath"]
        .as_str()
        .is_some_and(|path| !path.is_empty())
    {
        json!("worktree")
    } else {
        previous.get("envMode").cloned().unwrap_or(json!("local"))
    };
    next.insert(
        "envMode".into(),
        options.get("envMode").cloned().unwrap_or(env_mode),
    );
    let environment_selection = options
        .get("environmentSelection")
        .filter(|value| !value.is_null())
        .cloned()
        .or_else(|| {
            if context
                && ["branch", "worktreePath"]
                    .iter()
                    .any(|field| options.get(*field).is_some_and(|value| !value.is_null()))
            {
                Some(json!("manual"))
            } else {
                previous.get("environmentSelection").cloned()
            }
        });
    if let Some(selection) = environment_selection {
        next.insert("environmentSelection".into(), selection);
    }
    if let Some(balance) = options
        .get("loadBalancedEnvironmentId")
        .cloned()
        .or_else(|| {
            if context || previous.get("loadBalancedEnvironmentId").is_some() {
                Some(if changed {
                    Value::Null
                } else {
                    previous["loadBalancedEnvironmentId"].clone()
                })
            } else {
                None
            }
        })
    {
        next.insert("loadBalancedEnvironmentId".into(), balance);
    }
    let checkout = options
        .get("checkoutPullRequest")
        .cloned()
        .or_else(|| (!changed).then(|| previous["checkoutPullRequest"].clone()));
    next.remove("checkoutPullRequest");
    if let Some(checkout) = checkout.filter(|value| !value.is_null()) {
        next.insert("checkoutPullRequest".into(), checkout);
    }
    next.insert(
        "promotedTo".into(),
        if context {
            previous["promotedTo"].clone()
        } else {
            Value::Null
        },
    );
    next.into()
}
pub fn clear(state: &mut Value, id: &str) -> Vec<String> {
    let previews = state["draftsByThreadKey"][id]["images"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|image| image["previewUrl"].as_str())
        .filter(|preview| preview.starts_with("blob:"))
        .map(str::to_owned)
        .collect();
    for field in ["draftsByThreadKey", "draftThreadsByThreadKey"] {
        if let Some(map) = state[field].as_object_mut() {
            map.remove(id);
        }
    }
    if let Some(map) = state["logicalProjectDraftThreadKeyByLogicalProjectKey"].as_object_mut() {
        map.retain(|_, value| value != id);
    }
    previews
}
pub fn set_logical_project(
    state: &mut Value,
    logical: &str,
    project: &ScopedProjectRef,
    id: &str,
    options: &Value,
    now: &str,
    open_previews: &BTreeSet<String>,
) -> Vec<String> {
    let logical = logical.js_trim();
    if logical.is_empty() || id.is_empty() {
        return Vec::new();
    }
    let prior = state["draftThreadsByThreadKey"][id].clone();
    let old = state["logicalProjectDraftThreadKeyByLogicalProjectKey"][logical]
        .as_str()
        .map(str::to_owned);
    let next = make_session(project, id, logical, &prior, options, now, false);
    for field in [
        "draftThreadsByThreadKey",
        "logicalProjectDraftThreadKeyByLogicalProjectKey",
    ] {
        if !state[field].is_object() {
            state[field] = json!({});
        }
    }
    state["draftThreadsByThreadKey"][id] = next;
    if let Some(map) = state["logicalProjectDraftThreadKeyByLogicalProjectKey"].as_object_mut() {
        map.retain(|key, value| value != id || key == logical);
    }
    state["logicalProjectDraftThreadKeyByLogicalProjectKey"][logical] = json!(id);
    if prior.is_object()
        && prior["environmentId"] != project.environment_id.0
        && let Some(files) = state["draftsByThreadKey"][id]["files"].as_array_mut()
    {
        for file in files {
            if file["uploadEnvironmentId"] != project.environment_id.0
                && let Some(fields) = file.as_object_mut()
            {
                fields.remove("uploadedAttachmentId");
                fields.remove("uploadEnvironmentId");
            }
        }
    }
    if let Some(old) = old.filter(|old| old != id) {
        let session = &state["draftThreadsByThreadKey"][&old];
        let preview_key = format!(
            "{}:{}",
            string(session, "environmentId"),
            string(session, "threadId")
        );
        let used = state["logicalProjectDraftThreadKeyByLogicalProjectKey"]
            .as_object()
            .is_some_and(|map| map.values().any(|value| value == &old));
        if !used
            && session["promotedTo"].is_null()
            && !web_has_user_content(&state["draftsByThreadKey"][&old])
            && !open_previews.contains(&preview_key)
        {
            return clear(state, &old);
        }
    }
    Vec::new()
}
pub fn update_context(
    state: &mut Value,
    target: Target<'_>,
    project: Option<&ScopedProjectRef>,
    options: &Value,
    now: &str,
) -> bool {
    let Some(id) = resolve(state, target) else {
        return false;
    };
    let Some(prior) = state["draftThreadsByThreadKey"].get(&id).cloned() else {
        return false;
    };
    let existing = serde_json::from_value::<ScopedProjectRef>(prior.clone()).ok();
    let Some(project) = project.or(existing.as_ref()) else {
        return false;
    };
    if project.project_id.0.is_empty() || project.environment_id.0.is_empty() {
        return false;
    }
    let next = make_session(
        project,
        &id,
        string(&prior, "logicalProjectKey"),
        &prior,
        options,
        now,
        true,
    );
    if next == prior {
        return false;
    }
    state["draftThreadsByThreadKey"][&id] = next;
    true
}
pub fn clear_project(state: &mut Value, project: &ScopedProjectRef) -> Vec<String> {
    let ids = state["draftThreadsByThreadKey"]
        .as_object()
        .into_iter()
        .flat_map(|map| map.iter())
        .filter(|(_, session)| {
            session["projectId"] == project.project_id.0
                && session["environmentId"] == project.environment_id.0
        })
        .map(|(id, _)| id.clone())
        .collect::<Vec<_>>();
    ids.iter().flat_map(|id| clear(state, id)).collect()
}
pub fn checkout_pull_request_url(draft: &Value) -> Option<&str> {
    let checkout = draft.get("checkoutPullRequest")?;
    (draft["branch"] == checkout["branch"])
        .then(|| checkout["url"].as_str())
        .flatten()
}
pub fn clear_environment(state: &mut Value, environment: &str) -> Vec<String> {
    let scoped = |key: &str| {
        key.split_once(':')
            .is_some_and(|(env, id)| env == environment && !env.is_empty() && !id.is_empty())
    };
    let mut removed = BTreeSet::new();
    for (key, session) in state["draftThreadsByThreadKey"]
        .as_object()
        .into_iter()
        .flatten()
    {
        if session["environmentId"] == environment {
            removed.insert(key.clone());
        }
    }
    for key in state["draftsByThreadKey"]
        .as_object()
        .into_iter()
        .flat_map(|map| map.keys())
    {
        if scoped(key) {
            removed.insert(key.clone());
        }
    }
    for (project, key) in state["logicalProjectDraftThreadKeyByLogicalProjectKey"]
        .as_object()
        .into_iter()
        .flatten()
    {
        if scoped(project)
            && let Some(key) = key.as_str()
        {
            removed.insert(key.into());
        }
    }
    let previews = removed.iter().flat_map(|id| clear(state, id)).collect();
    for field in ["backgroundSubmissionThreadKeys", "rewindingThreadKeys"] {
        if let Some(map) = state[field].as_object_mut() {
            map.retain(|key, _| !scoped(key));
        }
        if let Some(keys) = state[field].as_array_mut() {
            keys.retain(|key| !key.as_str().is_some_and(scoped));
        }
    }
    if let Some(map) = state["promotedDraftRedirects"].as_object_mut() {
        map.retain(|_, target| target["environmentId"] != environment);
    }
    previews
}

pub struct PromotionTicket {
    draft: String,
    target: Value,
}
impl PromotionTicket {
    pub fn draft_id(&self) -> &str {
        &self.draft
    }
    pub fn target(&self) -> &Value {
        &self.target
    }
}
#[derive(Default)]
pub struct PromotionTracker {
    pending: BTreeSet<String>,
    disposed: bool,
}
impl PromotionTracker {
    pub fn observe(
        &mut self,
        state: &mut Value,
        shells: &BTreeMap<String, Value>,
    ) -> Vec<PromotionTicket> {
        if self.disposed {
            return Vec::new();
        }
        let sessions = state["draftThreadsByThreadKey"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        let mut tickets = Vec::new();
        for (id, session) in sessions {
            let target = if session["promotedTo"].is_null() {
                json!({"environmentId":session["environmentId"],"threadId":session["threadId"]})
            } else {
                session["promotedTo"].clone()
            };
            let key = format!(
                "{}:{}",
                string(&target, "environmentId"),
                string(&target, "threadId")
            );
            let Some(shell) = shells.get(&key) else {
                continue;
            };
            if session["promotedTo"].is_null() {
                mark_promoting(state, &id, Some(&target));
                continue;
            }
            if promotion_ready(
                Some(shell),
                state["backgroundSubmissionThreadKeys"][&key] == true,
            ) && self.pending.insert(id.clone())
            {
                tickets.push(PromotionTicket { draft: id, target });
            }
        }
        tickets
    }
    pub fn complete(&mut self, state: &mut Value, ticket: PromotionTicket) -> bool {
        self.pending.remove(&ticket.draft);
        if self.disposed
            || state["draftThreadsByThreadKey"][&ticket.draft]["promotedTo"] != ticket.target
        {
            return false;
        }
        let ids = state["draftThreadsByThreadKey"]
            .as_object()
            .into_iter()
            .flat_map(|map| map.iter())
            .filter(|(_, session)| {
                session["promotedTo"] == ticket.target
                    || (session["promotedTo"].is_null()
                        && session["environmentId"] == ticket.target["environmentId"]
                        && session["threadId"] == ticket.target["threadId"])
            })
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        let mut changed = false;
        for id in ids {
            mark_promoting(state, &id, Some(&ticket.target));
            changed |= finalize_promotion(state, &id);
        }
        let key = format!(
            "{}:{}",
            string(&ticket.target, "environmentId"),
            string(&ticket.target, "threadId")
        );
        if let Some(pending) = state["backgroundSubmissionThreadKeys"].as_object_mut() {
            pending.remove(&key);
        }
        changed
    }
    pub fn dispose(&mut self) {
        self.disposed = true;
        self.pending.clear();
    }
}
