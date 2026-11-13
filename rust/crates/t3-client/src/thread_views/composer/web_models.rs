//! Model edits retain per-instance traits and identify the exact pick acknowledged by the server.
use super::web_drafts::{coerce_options, hydrate_draft};
use serde_json::{Value, json};
use std::{collections::BTreeMap, rc::Rc};
use t3_contracts::{model::ProviderOptionSelections, model_selection::ModelSelection};

fn model_value(selection: &ModelSelection, prior: &Value, replace_options: bool) -> Value {
    let mut next = json!({"instanceId":selection.instance_id,"model":selection.model});
    if let Some(options) = &selection.options {
        if !options.0.is_empty() {
            next["options"] = json!(options);
        }
    } else if !replace_options && let Some(options) = prior.get("options") {
        next["options"] = options.clone();
    }
    next
}
pub(super) fn draft(state: &Value, key: &str) -> Value {
    state["draftsByThreadKey"]
        .get(key)
        .cloned()
        .unwrap_or_else(|| hydrate_draft(&Value::Null))
}
pub(super) fn save(state: &mut Value, key: &str, draft: Value) {
    if !state["draftsByThreadKey"].is_object() {
        state["draftsByThreadKey"] = json!({});
    }
    let empty = draft["prompt"].as_str().is_none_or(str::is_empty)
        && [
            "images",
            "files",
            "persistedAttachments",
            "terminalContexts",
            "previewAnnotations",
            "reviewComments",
            "threadContexts",
            "environmentContexts",
        ]
        .iter()
        .all(|field| draft[*field].as_array().is_none_or(Vec::is_empty))
        && [
            "replyTo",
            "activeProvider",
            "runtimeMode",
            "interactionMode",
        ]
        .iter()
        .all(|field| draft[*field].is_null())
        && draft["modelSelectionByProvider"]
            .as_object()
            .is_none_or(serde_json::Map::is_empty);
    if empty {
        if let Some(drafts) = state["draftsByThreadKey"].as_object_mut() {
            drafts.remove(key);
        }
    } else {
        state["draftsByThreadKey"][key] = draft;
    }
}

pub fn set_sticky(state: &mut Value, selection: &ModelSelection) {
    if !state["stickyModelSelectionByProvider"].is_object() {
        state["stickyModelSelectionByProvider"] = json!({});
    }
    let id = &selection.instance_id.0;
    state["stickyModelSelectionByProvider"][id] = model_value(
        selection,
        &state["stickyModelSelectionByProvider"][id],
        false,
    );
    state["stickyActiveProvider"] = json!(id);
}
fn apply_sticky(state: &mut Value, key: &str) {
    if key.is_empty() {
        return;
    }
    let mut next = draft(state, key);
    next["modelSelectionByProvider"] = state["stickyModelSelectionByProvider"]
        .as_object()
        .cloned()
        .unwrap_or_default()
        .into();
    next["activeProvider"] = state["stickyActiveProvider"].clone();
    if let Some(next) = next.as_object_mut() {
        next.remove("modelSelectionExplicit");
    }
    save(state, key, next);
}

#[derive(Default)]
pub struct ModelPicks {
    pub(super) current: BTreeMap<String, Rc<ModelSelection>>,
}
impl ModelPicks {
    pub fn capture(&mut self, state: &Value, key: &str) -> Option<Rc<ModelSelection>> {
        let draft = &state["draftsByThreadKey"][key];
        let id = draft["activeProvider"].as_str()?;
        let selection =
            serde_json::from_value::<ModelSelection>(draft["modelSelectionByProvider"][id].clone())
                .ok()?;
        if let Some(current) = self
            .current
            .get(key)
            .filter(|current| current.as_ref() == &selection)
        {
            return Some(current.clone());
        }
        let pick = Rc::new(selection);
        self.current.insert(key.into(), pick.clone());
        Some(pick)
    }
    pub fn captured(&self, key: &str) -> Option<Rc<ModelSelection>> {
        self.current.get(key).cloned()
    }
    pub fn reconcile(
        &mut self,
        state: &mut Value,
        key: &str,
        pick: &Rc<ModelSelection>,
        observation: Observation<'_>,
    ) -> bool {
        observation.matches(pick) && self.release(state, key, pick)
    }
    pub fn apply_sticky(&mut self, state: &mut Value, key: &str) {
        let before = state["draftsByThreadKey"][key].clone();
        apply_sticky(state, key);
        if state["draftsByThreadKey"][key] != before {
            self.current.remove(key);
        }
    }
    pub fn set_provider_options(
        &mut self,
        state: &mut Value,
        key: &str,
        input: ProviderOptions<'_>,
    ) {
        set_provider_options(state, key, input);
        self.current.remove(key);
    }
    pub fn set(
        &mut self,
        state: &mut Value,
        key: &str,
        selection: Option<&ModelSelection>,
        replace_options: bool,
        explicit: bool,
    ) -> Option<Rc<ModelSelection>> {
        if key.is_empty() {
            return None;
        }
        let mut next = draft(state, key);
        if !next["modelSelectionByProvider"].is_object() {
            next["modelSelectionByProvider"] = json!({});
        }
        if let Some(selection) = selection {
            let id = &selection.instance_id.0;
            next["modelSelectionByProvider"][id] = model_value(
                selection,
                &next["modelSelectionByProvider"][id],
                replace_options,
            );
            next["activeProvider"] = json!(id);
        }
        if !explicit && next == draft(state, key) && next["modelSelectionExplicit"] != true {
            return self.current.get(key).cloned();
        }
        if let Some(next) = next.as_object_mut() {
            next.remove("modelSelectionExplicit");
        }
        if explicit {
            next["modelSelectionExplicit"] = json!(true);
        }
        let pick = selection
            .and_then(|selection| {
                serde_json::from_value::<ModelSelection>(
                    next["modelSelectionByProvider"][&selection.instance_id.0].clone(),
                )
                .ok()
            })
            .map(Rc::new);
        self.current.remove(key);
        if let Some(pick) = &pick {
            self.current.insert(key.into(), pick.clone());
        }
        save(state, key, next);
        pick
    }
    pub fn release(&mut self, state: &mut Value, key: &str, pick: &Rc<ModelSelection>) -> bool {
        if !self
            .current
            .get(key)
            .is_some_and(|current| Rc::ptr_eq(current, pick))
        {
            return false;
        }
        let current = &state["draftsByThreadKey"][key];
        let id = &pick.instance_id.0;
        if current["activeProvider"] != *id
            || current["modelSelectionByProvider"][id] != json!(pick.as_ref())
        {
            return false;
        }
        let mut next = current.clone();
        next["activeProvider"] = Value::Null;
        next["modelSelectionByProvider"] = json!({});
        if let Some(next) = next.as_object_mut() {
            next.remove("modelSelectionExplicit");
        }
        save(state, key, next);
        self.current.remove(key);
        true
    }
}

pub struct Observation<'a> {
    pub live: bool,
    pub pending: bool,
    pub visible: Option<&'a ModelSelection>,
    pub authoritative: Option<&'a ModelSelection>,
}
impl Observation<'_> {
    pub(super) fn matches(&self, pick: &ModelSelection) -> bool {
        self.live
            && !self.pending
            && self
                .visible
                .is_some_and(|selection| pick_matches_thread(pick, selection))
            && self
                .authoritative
                .is_some_and(|selection| pick_matches_thread(pick, selection))
    }
}
pub fn pick_matches_thread(pick: &ModelSelection, thread: &ModelSelection) -> bool {
    use t3_contracts::model::ProviderOptionSelectionValue;
    if pick.instance_id != thread.instance_id || pick.model != thread.model {
        return false;
    }
    let canonical = |selection: &ModelSelection| {
        let mut options = selection
            .options
            .as_ref()
            .map(|options| options.0.clone())
            .unwrap_or_default();
        options.retain(|option| {
            option.id != "fastMode" || option.value != ProviderOptionSelectionValue::Boolean(false)
        });
        let value = |value: &ProviderOptionSelectionValue| match value {
            ProviderOptionSelectionValue::Boolean(value) => value.to_string(),
            ProviderOptionSelectionValue::String(value) => value.clone(),
        };
        options.sort_by(|left, right| {
            left.id
                .cmp(&right.id)
                .then_with(|| value(&left.value).cmp(&value(&right.value)))
        });
        options
    };
    canonical(pick) == canonical(thread)
}

pub struct ProviderOptions<'a> {
    pub instance_id: &'a str,
    pub fallback_model: &'a str,
    pub remembered_model: Option<&'a str>,
    pub options: Option<&'a ProviderOptionSelections>,
    pub persist_sticky: bool,
    pub activate_instance: bool,
}
fn set_provider_options(state: &mut Value, key: &str, input: ProviderOptions<'_>) {
    if key.is_empty() {
        return;
    }
    let id = input.instance_id;
    let mut next = draft(state, key);
    if !next["modelSelectionByProvider"].is_object() {
        next["modelSelectionByProvider"] = json!({});
    }
    let prior = next["modelSelectionByProvider"][id].clone();
    let options = input
        .options
        .map_or_else(Vec::new, |options| coerce_options(&json!(options)));
    let selection = |model: &str| {
        let mut pick = json!({"instanceId":id,"model":model});
        if !options.is_empty() {
            pick["options"] = json!(options);
        }
        pick
    };
    next["modelSelectionByProvider"][id] =
        selection(prior["model"].as_str().unwrap_or(input.fallback_model));
    if input.persist_sticky {
        if !state["stickyModelSelectionByProvider"].is_object() {
            state["stickyModelSelectionByProvider"] = json!({});
        }
        let sticky = state["stickyModelSelectionByProvider"]
            .get(id)
            .cloned()
            .unwrap_or(prior);
        let sticky_model = sticky["model"].as_str().unwrap_or(input.fallback_model);
        let remembered = input.remembered_model.unwrap_or(sticky_model);
        if !options.is_empty()
            || sticky["options"]
                .as_array()
                .is_some_and(|options| !options.is_empty())
        {
            state["stickyModelSelectionByProvider"][id] = selection(sticky_model);
            if !state["stickyOptionsByModelByProvider"].is_object() {
                state["stickyOptionsByModelByProvider"] = json!({});
            }
            if !state["stickyOptionsByModelByProvider"][id].is_object() {
                state["stickyOptionsByModelByProvider"][id] = json!({});
            }
            if options.is_empty() {
                if let Some(models) = state["stickyOptionsByModelByProvider"][id].as_object_mut() {
                    models.remove(remembered);
                }
            } else {
                state["stickyOptionsByModelByProvider"][id][remembered] = json!(options);
            }
        }
        state["stickyActiveProvider"] = if input.activate_instance {
            json!(id)
        } else {
            next["activeProvider"]
                .as_str()
                .map_or_else(|| json!(id), |id| json!(id))
        };
    }
    if input.activate_instance {
        next["activeProvider"] = json!(id);
    }
    next["modelSelectionExplicit"] = json!(true);
    save(state, key, next);
}

pub fn set_prompt(state: &mut Value, key: &str, prompt: &str) {
    if key.is_empty() {
        return;
    }
    let mut next = draft(state, key);
    next["prompt"] = json!(prompt);
    save(state, key, next);
}
pub fn clear_content(state: &mut Value, key: &str) {
    if !state["draftsByThreadKey"][key].is_object() {
        return;
    }
    let mut next = draft(state, key);
    next["prompt"] = json!("");
    next["replyTo"] = Value::Null;
    for field in [
        "images",
        "files",
        "nonPersistedImageIds",
        "persistedAttachments",
        "terminalContexts",
        "previewAnnotations",
        "reviewComments",
        "threadContexts",
        "environmentContexts",
    ] {
        next[field] = json!([]);
    }
    save(state, key, next);
}
pub fn set_runtime_mode(
    state: &mut Value,
    key: &str,
    mode: Option<t3_contracts::provider_policy::RuntimeMode>,
) {
    if key.is_empty() {
        return;
    }
    let mut next = draft(state, key);
    next["runtimeMode"] = json!(mode);
    save(state, key, next);
}
pub fn set_interaction_mode(
    state: &mut Value,
    key: &str,
    mode: Option<t3_contracts::provider_policy::ProviderInteractionMode>,
) {
    if key.is_empty() {
        return;
    }
    let mut next = draft(state, key);
    next["interactionMode"] = json!(mode);
    save(state, key, next);
}
