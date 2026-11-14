//! modelTraits.ts and shared/model.ts descriptor presentation and trait-change planning.
use t3_contracts::{model::*, model_selection::ModelSelection};

pub struct ModelTraitsInput<'a> {
    pub descriptors: &'a [ProviderOptionDescriptor],
    pub selection: Option<&'a ModelSelection>,
    pub reported: Option<&'a ModelSelection>,
    pub prompt: &'a str,
    pub allow_prompt_injected_effort: bool,
    pub unavailable: bool,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelTraitsView {
    pub descriptors: Vec<ProviderOptionDescriptor>,
    pub reasoning: Option<ProviderOptionDescriptor>,
    pub reasoning_value: Option<String>,
    pub others: Vec<ProviderOptionDescriptor>,
    pub speed: Option<ProviderOptionDescriptor>,
    pub ultracode: Option<ProviderOptionDescriptor>,
    pub summary: String,
    pub speed_label: Option<String>,
    pub ultrathink_prompt_controlled: bool,
    pub ultrathink_in_body: bool,
    pub unavailable: bool,
}
#[derive(Clone, Debug, PartialEq)]
pub struct TraitChange {
    pub prompt: String,
    pub options: Option<ProviderOptionSelections>,
}
pub fn descriptor_id(descriptor: &ProviderOptionDescriptor) -> &str {
    match descriptor {
        ProviderOptionDescriptor::Select(d) => &d.id,
        ProviderOptionDescriptor::Boolean(d) => &d.id,
    }
}
pub fn is_reasoning_control(descriptor: &ProviderOptionDescriptor) -> bool {
    matches!(descriptor, ProviderOptionDescriptor::Select(d) if d.role == Some(Some(ProviderOptionRole::Reasoning)))
        || crate::thread_views::threads::execution_presentation::REASONING_EFFORT_OPTION_IDS
            .contains(&descriptor_id(descriptor))
        || descriptor_id(descriptor) == "thinking"
}
fn stated(selection: Option<&ModelSelection>, id: &str) -> Option<ProviderOptionSelectionValue> {
    selection?
        .options
        .as_ref()?
        .0
        .iter()
        .find(|o| o.id == id)
        .map(|o| o.value.clone())
}
fn reported(input: &ModelTraitsInput<'_>, id: &str) -> Option<ProviderOptionSelectionValue> {
    let selected = input.selection?;
    let reported = input.reported?;
    if selected.instance_id != reported.instance_id
        || selected.model != reported.model
        || stated(Some(selected), id).is_some()
    {
        return None;
    }
    stated(Some(reported), id)
}
fn descriptor_value(descriptor: &ProviderOptionDescriptor) -> Option<ProviderOptionSelectionValue> {
    match descriptor {
        ProviderOptionDescriptor::Boolean(d) => d
            .current_value
            .flatten()
            .map(ProviderOptionSelectionValue::Boolean),
        ProviderOptionDescriptor::Select(d) => d
            .current_value
            .as_ref()
            .and_then(Option::as_ref)
            .filter(|s| !s.is_empty())
            .cloned()
            .or_else(|| {
                d.options
                    .iter()
                    .find(|o| o.is_default == Some(Some(true)))
                    .map(|o| o.id.clone())
            })
            .map(ProviderOptionSelectionValue::String),
    }
}
fn current_value(
    input: &ModelTraitsInput<'_>,
    descriptor: &ProviderOptionDescriptor,
) -> Option<ProviderOptionSelectionValue> {
    let id = descriptor_id(descriptor);
    if let Some(value) = reported(input, id) {
        return Some(value);
    }
    if id == "variant" && input.selection.is_some() && stated(input.selection, id).is_none() {
        return None;
    }
    descriptor_value(descriptor)
}
fn set_value(descriptor: &mut ProviderOptionDescriptor, value: &ProviderOptionSelectionValue) {
    match (descriptor, value) {
        (ProviderOptionDescriptor::Boolean(d), ProviderOptionSelectionValue::Boolean(value)) => {
            d.current_value = Some(Some(*value))
        }
        (ProviderOptionDescriptor::Select(d), ProviderOptionSelectionValue::String(value)) => {
            let resolved = if d.options.is_empty() || d.options.iter().any(|o| &o.id == value) {
                Some(value.clone())
            } else {
                d.current_value
                    .as_ref()
                    .and_then(Option::as_ref)
                    .cloned()
                    .or_else(|| {
                        d.options
                            .iter()
                            .find(|o| o.is_default == Some(Some(true)))
                            .map(|o| o.id.clone())
                    })
            };
            d.current_value = resolved.map(Some);
        }
        _ => {}
    }
}
fn prefixless(prompt: &str) -> &str {
    if prompt
        .get(..11)
        .is_some_and(|s| s.eq_ignore_ascii_case("Ultrathink:"))
    {
        prompt[11..].trim_start_matches(crate::view_logic::is_js_whitespace)
    } else {
        prompt
    }
}
fn ultrathink(prompt: &str) -> bool {
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let prompt = prompt.to_lowercase();
    prompt.match_indices("ultrathink").any(|(at, _)| {
        prompt[..at].chars().next_back().is_none_or(|c| !word(c))
            && prompt[at + 10..].chars().next().is_none_or(|c| !word(c))
    })
}
fn options(descriptors: &[ProviderOptionDescriptor]) -> Option<ProviderOptionSelections> {
    let values = descriptors
        .iter()
        .filter_map(|d| {
            descriptor_value(d).map(|value| ProviderOptionSelection {
                id: descriptor_id(d).into(),
                value,
            })
        })
        .collect::<Vec<_>>();
    (!values.is_empty()).then_some(ProviderOptionSelections(values))
}
pub fn resolve_model_traits(input: &ModelTraitsInput<'_>) -> ModelTraitsView {
    let mut descriptors = input.descriptors.to_vec();
    let legacy = stated(input.selection, "effort")
        == Some(ProviderOptionSelectionValue::String("ultracode".into()))
        && descriptors
            .iter()
            .any(|d| matches!(d,ProviderOptionDescriptor::Boolean(d) if d.id=="ultracode"));
    for descriptor in &mut descriptors {
        let id = descriptor_id(descriptor).to_owned();
        let value = if legacy && id == "effort" {
            Some(ProviderOptionSelectionValue::String("xhigh".into()))
        } else if legacy && id == "ultracode" && stated(input.selection, &id).is_none() {
            Some(ProviderOptionSelectionValue::Boolean(true))
        } else {
            stated(input.selection, &id)
        };
        if let Some(value) = value {
            set_value(descriptor, &value);
        }
    }
    let primary = descriptors.iter().find_map(|d| {
        if let ProviderOptionDescriptor::Select(d) = d {
            Some(d)
        } else {
            None
        }
    });
    let primary_id = primary.map(|d| d.id.clone());
    let controlled = input.allow_prompt_injected_effort
        && primary.is_some_and(|d| {
            d.prompt_injected_values
                .as_ref()
                .and_then(Option::as_ref)
                .is_some_and(|v| !v.is_empty())
        })
        && ultrathink(input.prompt);
    let body = controlled && ultrathink(prefixless(input.prompt));
    let mut labels = Vec::<String>::new();
    let (mut reasoning_index, mut speed_label, mut fallback) = (None, None::<String>, None);
    for descriptor in &descriptors {
        let id = descriptor_id(descriptor);
        if let ProviderOptionDescriptor::Boolean(d) = descriptor {
            if id == "fastMode" {
                speed_label = (d.current_value == Some(Some(true))).then(|| "Fast".into());
                fallback = Some(speed_label.clone().unwrap_or_else(|| "Normal".into()));
                continue;
            }
            if id == "ultracode" {
                if d.current_value == Some(Some(true)) {
                    labels.push(d.label.clone());
                }
                continue;
            }
        }
        let value = current_value(input, descriptor);
        if let ProviderOptionDescriptor::Select(d) = descriptor
            && id == "serviceTier"
        {
            let value = descriptor_value(descriptor);
            let fast = d.options.iter().find(|o| o.label == "Fast");
            let ultra = d.options.iter().find(|o| o.label == "Ultrafast");
            if let Some(ProviderOptionSelectionValue::String(value)) = &value
                && ((value == "default" && (fast.is_some() || ultra.is_some()))
                    || fast.is_some_and(|o| &o.id == value)
                    || ultra.is_some_and(|o| &o.id == value))
            {
                speed_label = ultra
                    .filter(|o| &o.id == value)
                    .map(|_| "Ultrafast".into())
                    .or_else(|| fast.filter(|o| &o.id == value).map(|_| "Fast".into()));
                fallback = Some(
                    d.options
                        .iter()
                        .find(|o| &o.id == value)
                        .map_or("Normal", |o| o.label.as_str())
                        .into(),
                );
                continue;
            }
        }
        let label = if controlled && Some(id) == primary_id.as_deref() {
            Some("Ultrathink".into())
        } else {
            match descriptor {
                ProviderOptionDescriptor::Boolean(d) => Some(format!(
                    "{} {}",
                    d.label,
                    if d.current_value == Some(Some(true)) {
                        "On"
                    } else {
                        "Off"
                    }
                )),
                ProviderOptionDescriptor::Select(d) => value
                    .as_ref()
                    .and_then(|value| {
                        if let ProviderOptionSelectionValue::String(value) = value {
                            d.options
                                .iter()
                                .find(|o| &o.id == value)
                                .map(|o| o.label.clone())
                        } else {
                            None
                        }
                    })
                    .or_else(|| {
                        (reported(input, id)
                            == Some(ProviderOptionSelectionValue::String("default".into())))
                        .then(|| "Default".into())
                    })
                    .or_else(|| (id == "variant").then(|| "Unknown".into()))
                    .or_else(|| (id == "thinking").then(|| d.label.clone())),
            }
        };
        if let Some(label) = label.filter(|s| !s.is_empty()) {
            if reasoning_index.is_none()
                && matches!(descriptor, ProviderOptionDescriptor::Select(_))
                && is_reasoning_control(descriptor)
            {
                reasoning_index = Some(labels.len());
            }
            labels.push(label);
        }
    }
    let summary = if labels.is_empty() && fallback.is_some() {
        fallback.unwrap_or_default()
    } else {
        if let Some(speed) = &speed_label {
            if let Some(index) = reasoning_index {
                labels[index] = format!("{} {speed}", labels[index]);
            } else {
                labels.push(speed.clone());
            }
        }
        labels.join(" · ")
    };
    let reasoning = descriptors
        .iter()
        .filter(|d| matches!(d, ProviderOptionDescriptor::Select(_)))
        .chain(
            descriptors
                .iter()
                .filter(|d| matches!(d, ProviderOptionDescriptor::Boolean(_))),
        )
        .find(|d| is_reasoning_control(d))
        .cloned();
    let reasoning_value = reasoning
        .as_ref()
        .and_then(|d| current_value(input, d))
        .and_then(|v| {
            if let ProviderOptionSelectionValue::String(v) = v {
                Some(v)
            } else {
                None
            }
        });
    ModelTraitsView {
        reasoning_value,
        others: descriptors
            .iter()
            .filter(|d| {
                reasoning
                    .as_ref()
                    .is_none_or(|reasoning| descriptor_id(reasoning) != descriptor_id(d))
                    && !matches!(descriptor_id(d), "fastMode" | "serviceTier" | "ultracode")
            })
            .cloned()
            .collect(),
        reasoning,
        speed: descriptors
            .iter()
            .find(|d| matches!(descriptor_id(d), "fastMode" | "serviceTier"))
            .cloned(),
        ultracode: descriptors
            .iter()
            .find(|d| descriptor_id(d) == "ultracode")
            .cloned(),
        descriptors,
        summary,
        speed_label,
        ultrathink_prompt_controlled: controlled,
        ultrathink_in_body: body,
        unavailable: input.unavailable,
    }
}
pub fn plan_model_trait_change(
    input: &ModelTraitsInput<'_>,
    change: ProviderOptionSelection,
) -> Option<TraitChange> {
    if input.unavailable {
        return None;
    }
    let view = resolve_model_traits(input);
    let descriptor = view
        .descriptors
        .iter()
        .find(|d| descriptor_id(d) == change.id)?;
    let primary = view
        .descriptors
        .iter()
        .find(|d| matches!(d, ProviderOptionDescriptor::Select(_)))
        .map(descriptor_id);
    let is_primary = primary == Some(change.id.as_str());
    if let ProviderOptionDescriptor::Select(d) = descriptor {
        let ProviderOptionSelectionValue::String(value) = &change.value else {
            return None;
        };
        if value.is_empty()
            || d.prompt_injected_values
                .as_ref()
                .and_then(Option::as_ref)
                .is_some_and(|values| values.contains(value))
            || (!d.options.is_empty() && !d.options.iter().any(|o| &o.id == value))
            || is_primary && view.ultrathink_in_body
        {
            return None;
        }
    } else if !matches!(change.value, ProviderOptionSelectionValue::Boolean(_)) {
        return None;
    }
    let mut descriptors = view.descriptors;
    set_value(
        descriptors
            .iter_mut()
            .find(|d| descriptor_id(d) == change.id)?,
        &change.value,
    );
    Some(TraitChange {
        prompt: if is_primary && view.ultrathink_prompt_controlled {
            prefixless(input.prompt).into()
        } else {
            input.prompt.into()
        },
        options: options(&descriptors),
    })
}
