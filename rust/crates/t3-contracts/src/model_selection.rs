use crate::{model::ProviderOptionSelections, provider_instance::ProviderInstanceId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(try_from = "ModelSelectionSource")]
pub struct ModelSelection {
    pub instance_id: ProviderInstanceId,
    pub model: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub options: Option<ProviderOptionSelections>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelSelectionSource {
    #[serde(default, with = "crate::wire::optional")]
    instance_id: Option<serde_json::Value>,
    #[serde(default)]
    provider: Option<serde_json::Value>,
    model: String,
    #[serde(default, with = "crate::wire::optional")]
    options: Option<ProviderOptionSelections>,
}

impl TryFrom<ModelSelectionSource> for ModelSelection {
    type Error = serde_json::Error;

    fn try_from(raw: ModelSelectionSource) -> Result<Self, Self::Error> {
        let source = raw
            .instance_id
            .or_else(|| raw.provider.filter(serde_json::Value::is_string))
            .unwrap_or(serde_json::Value::Null);
        Ok(Self {
            instance_id: serde_json::from_value(source)?,
            model: raw.model,
            options: raw.options,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn legacy_provider_normalizes_and_explicit_instance_takes_precedence()
    -> Result<(), serde_json::Error> {
        for (raw, expected) in [
            (json!({"provider": "codex", "model": "m"}), "codex"),
            (
                json!({"provider": "codex", "instanceId": "custom", "model": "m"}),
                "custom",
            ),
        ] {
            let selection: ModelSelection = serde_json::from_value(raw)?;
            assert_eq!(
                serde_json::to_value(selection)?,
                json!({"instanceId": expected, "model": "m"})
            );
        }
        assert!(
            serde_json::from_value::<ModelSelection>(
                json!({"provider": "codex", "instanceId": null, "model": "m"})
            )
            .is_err()
        );
        Ok(())
    }
}
