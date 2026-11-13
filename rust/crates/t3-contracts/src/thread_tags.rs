pub type ThreadTag = String;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(transparent)]
pub struct StoredThreadTags(pub Vec<String>);

impl<'de> serde::Deserialize<'de> for StoredThreadTags {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = <serde_json::Value as serde::Deserialize>::deserialize(deserializer)?;
        Ok(Self(serde_json::from_value(raw).unwrap_or_default()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn malformed_stored_tag_lists_fall_back_to_empty() -> Result<(), serde_json::Error> {
        for raw in [json!(null), json!(["valid", false]), json!({})] {
            let tags: StoredThreadTags = serde_json::from_value(raw)?;
            assert!(tags.0.is_empty());
        }
        Ok(())
    }
}
