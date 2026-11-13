//! Mobile model picks retain reference identity until an authoritative live emission accepts them.
use super::{
    mobile_content,
    web_models::{ModelPicks, Observation},
};
use serde_json::{Value, json};
use std::rc::Rc;
use t3_contracts::{model::ProviderOptionSelections, model_selection::ModelSelection};

#[derive(Default)]
pub struct MobileModelPicks {
    picks: ModelPicks,
}
impl MobileModelPicks {
    pub fn capture(&mut self, document: &Value, key: &str) -> Option<Rc<ModelSelection>> {
        let selection = serde_json::from_value::<ModelSelection>(
            document["drafts"][key]["modelSelection"].clone(),
        )
        .ok()?;
        if let Some(current) = self
            .picks
            .current
            .get(key)
            .filter(|current| current.as_ref() == &selection)
        {
            return Some(current.clone());
        }
        let pick = Rc::new(selection);
        self.picks.current.insert(key.into(), pick.clone());
        Some(pick)
    }
    pub fn captured(&self, key: &str) -> Option<Rc<ModelSelection>> {
        self.picks.captured(key)
    }
    pub fn set(
        &mut self,
        document: &mut Value,
        key: &str,
        selection: ModelSelection,
    ) -> Rc<ModelSelection> {
        let mut draft = mobile_content::normalize(&document["drafts"][key]);
        draft["modelSelection"] = json!(selection);
        document["drafts"] = mobile_content::put(&document["drafts"], key, draft);
        let pick = Rc::new(selection);
        self.picks.current.insert(key.into(), pick.clone());
        pick
    }
    pub fn release(&mut self, document: &mut Value, key: &str, pick: &Rc<ModelSelection>) -> bool {
        if !self
            .picks
            .current
            .get(key)
            .is_some_and(|current| Rc::ptr_eq(current, pick))
            || document["drafts"][key]["modelSelection"] != json!(pick.as_ref())
        {
            return false;
        }
        let mut draft = document["drafts"][key].clone();
        if let Some(fields) = draft.as_object_mut() {
            fields.remove("modelSelection");
        }
        document["drafts"] = mobile_content::put(&document["drafts"], key, draft);
        self.picks.current.remove(key);
        true
    }
    pub fn reconcile(
        &mut self,
        document: &mut Value,
        key: &str,
        pick: &Rc<ModelSelection>,
        observation: Observation<'_>,
    ) -> bool {
        if key.starts_with("new-task:") || key.contains("~queued-edit~") {
            return false;
        }
        observation.matches(pick) && self.release(document, key, pick)
    }
}
pub fn remember_options(document: &mut Value, selection: &ModelSelection) -> bool {
    let Some(options) = selection
        .options
        .as_ref()
        .filter(|options| !options.0.is_empty())
    else {
        return false;
    };
    if !document["modelOptionMemory"].is_object() {
        document["modelOptionMemory"] = json!({});
    }
    let id = &selection.instance_id.0;
    if !document["modelOptionMemory"][id].is_object() {
        document["modelOptionMemory"][id] = json!({});
    }
    document["modelOptionMemory"][id][&selection.model] = json!(options);
    true
}
pub fn with_remembered_options(
    document: &Value,
    selection: Rc<ModelSelection>,
) -> Rc<ModelSelection> {
    let Some(options) = document["modelOptionMemory"][&selection.instance_id.0]
        .get(&selection.model)
        .and_then(|value| serde_json::from_value::<ProviderOptionSelections>(value.clone()).ok())
    else {
        return selection;
    };
    if selection
        .options
        .as_ref()
        .map_or(options.0.is_empty(), |current| current == &options)
    {
        return selection;
    }
    let mut next = selection.as_ref().clone();
    next.options = Some(options);
    Rc::new(next)
}
