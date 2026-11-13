use super::super::pr_list::ListSort;
use crate::{
    ClientError,
    platform::{Storage, StorageArea, StorageKey, StorageWrite},
};
use serde_json::{Value, json};
use t3_contracts::pull_request::{
    PullRequestInvolvement, PullRequestListFilters, PullRequestListState,
};

pub fn default_preferences() -> Value {
    json!({"involvement": "all", "state": "open"})
}
pub fn normalize_preferences(value: &Value) -> Option<Value> {
    serde_json::from_value::<PullRequestInvolvement>(value.get("involvement")?.clone()).ok()?;
    serde_json::from_value::<PullRequestListState>(value.get("state")?.clone()).ok()?;
    let mut result = json!({"involvement": value["involvement"], "state": value["state"]});
    for key in ["environmentId", "projectId", "host", "q", "author"] {
        if let Some(raw) = value.get(key) {
            let text = raw.as_str()?;
            if matches!(key, "environmentId" | "projectId") {
                if text.trim().is_empty() {
                    return None;
                }
            } else if text.encode_utf16().count() > 200 {
                return None;
            }
            if !text.is_empty() {
                result[key] = raw.clone();
            }
        }
    }
    for key in ["draft", "review", "checks"] {
        if let Some(raw) = value.get(key) {
            raw.as_str()?;
            let filter =
                serde_json::from_value::<PullRequestListFilters>(json!({ key: raw })).ok()?;
            serde_json::to_value(filter).ok()?.get(key)?;
            result[key] = raw.clone();
        }
    }
    if let Some(labels) = value.get("labels") {
        let labels = labels.as_array()?;
        if labels.len() > 10
            || labels.iter().any(|label| {
                label
                    .as_str()
                    .is_none_or(|s| s.encode_utf16().count() > 200)
            })
        {
            return None;
        }
        if !labels.is_empty() {
            result["labels"] = json!(labels);
        }
    }
    if let Some(sort) = value.get("sort") {
        let decoded = serde_json::from_value::<ListSort>(sort.clone()).ok()?;
        if decoded != ListSort::Ready {
            result["sort"] = sort.clone();
        }
    }
    Some(result)
}
fn key() -> StorageKey {
    StorageKey::new("local-preferences", None, "t3.pullRequests.preferences")
}
pub async fn read_preferences(storage: &impl Storage) -> Value {
    match storage.get(StorageArea::Ordinary, key()).await {
        Ok(Some(bytes)) => serde_json::from_slice(&bytes)
            .ok()
            .and_then(|value| normalize_preferences(&value))
            .unwrap_or_else(default_preferences),
        _ => default_preferences(),
    }
}
pub async fn write_preferences(
    storage: &impl Storage,
    preferences: &Value,
) -> Result<(), ClientError> {
    storage
        .write(
            StorageArea::Ordinary,
            vec![StorageWrite {
                key: key(),
                value: Some(serde_json::to_vec(preferences)?),
            }],
        )
        .await?;
    Ok(())
}
