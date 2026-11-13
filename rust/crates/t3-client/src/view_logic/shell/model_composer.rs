use super::*;

pub struct ComposerModelInput<'a> {
    pub draft_selections: Option<&'a BTreeMap<ProviderInstanceId, ModelSelection>>,
    pub providers: &'a [ServerProvider],
    pub driver: &'a str,
    pub selected_instance: Option<&'a ProviderInstanceId>,
    pub requested_instance: Option<&'a ProviderInstanceId>,
    pub thread_selection: Option<&'a ModelSelection>,
    pub project_selection: Option<&'a ModelSelection>,
    pub settings: &'a Settings<'a>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ComposerModelState {
    pub selected_model: String,
    pub options: BTreeMap<ProviderInstanceId, ProviderOptionSelections>,
}
/// Draft picks win, followed by the thread and project. An optionless thread deliberately
/// blocks project defaults. Same-driver fallback carries options only with their model identity.
pub fn composer_model_state(input: ComposerModelInput<'_>) -> ComposerModelState {
    let base = input.thread_selection.or(input.project_selection);
    let resolve = |id: &ProviderInstanceId, candidate: Option<&str>, preserve| {
        resolve_for_instance(input.settings, input.providers, id, candidate, preserve)
            .or_else(|| (input.driver == "antigravity").then(String::new))
    };
    let base_model = input
        .selected_instance
        .and_then(|id| {
            resolve(
                id,
                base.map(|s| s.model.as_str()),
                input.thread_selection.is_some_and(|s| &s.instance_id == id),
            )
        })
        .unwrap_or_else(|| {
            resolve_app_selection(
                input.driver,
                input.settings,
                input.providers,
                base.map(|s| s.model.as_str()),
            )
        });
    let default_id = default_instance_id_for_driver(
        &t3_contracts::provider_instance::ProviderDriverKind(input.driver.into()),
    );
    let active = input.draft_selections.and_then(|selections| {
        input
            .selected_instance
            .and_then(|id| selections.get(id))
            .map(|s| (input.selected_instance.unwrap_or(&default_id), s))
            .or_else(|| {
                input
                    .selected_instance
                    .is_none_or(|id| id == &default_id)
                    .then(|| {
                        selections
                            .get(&ProviderInstanceId(input.driver.into()))
                            .map(|s| (&default_id, s))
                    })
                    .flatten()
            })
    });
    let selected_model =
        active
            .filter(|(_, s)| !s.model.is_empty())
            .map_or(base_model, |(id, selection)| {
                resolve(id, Some(&selection.model), true).unwrap_or_else(|| {
                    resolve_app_selection(
                        input.driver,
                        input.settings,
                        input.providers,
                        Some(&selection.model),
                    )
                })
            });
    let has_options = |s: &ModelSelection| s.options.as_ref().is_some_and(|o| !o.0.is_empty());
    let selections = input
        .draft_selections
        .filter(|s| s.values().any(has_options));
    let mut options = selections.map_or_else(
        || {
            base.and_then(|s| {
                s.options
                    .as_ref()
                    .filter(|o| !o.0.is_empty())
                    .map(|o| (s.instance_id.clone(), o.clone()))
            })
            .into_iter()
            .collect()
        },
        |selections| {
            selections
                .iter()
                .filter_map(|(id, s)| {
                    s.options
                        .as_ref()
                        .filter(|o| !o.0.is_empty())
                        .map(|o| (id.clone(), o.clone()))
                })
                .collect::<BTreeMap<_, _>>()
        },
    );
    if let (Some(selected), Some(requested)) = (input.selected_instance, input.requested_instance)
        && selected != requested
        && !options.contains_key(selected)
        && let Some(requested_options) = options.get(requested).cloned()
        && input
            .providers
            .iter()
            .any(|p| &p.instance_id == requested && p.driver.0 == input.driver)
    {
        let requested_model = selections
            .and_then(|s| s.get(requested))
            .or_else(|| base.filter(|s| &s.instance_id == requested))
            .map(|s| s.model.as_str());
        let identity = |id: &ProviderInstanceId, model: &str| {
            let custom = input
                .providers
                .iter()
                .find(|p| &p.instance_id == id)
                .is_some_and(|p| p.models.iter().any(|m| m.is_custom && m.slug.0 == model))
                || read_custom_models(input.settings.server, id, input.driver)
                    .iter()
                    .any(|m| m.slug == model);
            if custom {
                return model.to_owned();
            }
            let model = model.js_trim();
            MODEL_SLUG_ALIASES_BY_PROVIDER
                .iter()
                .find(|(p, _)| *p == input.driver)
                .and_then(|(_, aliases)| {
                    aliases
                        .iter()
                        .find_map(|(alias, slug)| (*alias == model).then_some(*slug))
                })
                .unwrap_or(model)
                .into()
        };
        if requested_model.is_some_and(|model| {
            model == selected_model
                || identity(requested, model) == identity(selected, &selected_model)
        }) {
            options.insert(selected.clone(), requested_options);
        }
    }
    ComposerModelState {
        selected_model,
        options,
    }
}
