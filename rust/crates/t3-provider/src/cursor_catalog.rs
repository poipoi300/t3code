use crate::{
    sensitive::{SecretString, Sensitive},
    transport::{HttpRequest, HttpTransport, ProviderClock},
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::Arc,
};
use tokio::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("Cursor catalog request failed.")]
pub struct CatalogError {
    pub authentication_failure: bool,
    pub plan_required: bool,
    pub status: Option<u16>,
}
impl CatalogError {
    fn response() -> Self {
        Self {
            authentication_failure: false,
            plan_required: false,
            status: None,
        }
    }
    fn account_blocked(self) -> bool {
        self.status == Some(403) && !self.plan_required
    }
}
pub struct CatalogSnapshot {
    pub user: Option<Value>,
    pub models: Vec<Value>,
}
struct Cached {
    key: [u8; 32],
    expires: i64,
    models: Vec<Value>,
}
pub struct CursorCatalog {
    http: Arc<dyn HttpTransport>,
    clock: Arc<dyn ProviderClock>,
    base_url: String,
    models: Mutex<VecDeque<Cached>>,
}
impl CursorCatalog {
    pub fn new(
        http: Arc<dyn HttpTransport>,
        clock: Arc<dyn ProviderClock>,
        base_url: String,
    ) -> Self {
        Self {
            http,
            clock,
            base_url: base_url.trim_end_matches('/').into(),
            models: Mutex::new(VecDeque::new()),
        }
    }
    async fn request(&self, key: &SecretString, path: &str) -> Result<Value, CatalogError> {
        let response = self
            .http
            .raw(HttpRequest {
                method: "GET",
                url: format!("{}{path}", self.base_url),
                headers: BTreeMap::from([(
                    "authorization".into(),
                    Sensitive::new(format!("Bearer {}", key.expose())),
                )]),
                body: None,
            })
            .await
            .map_err(|_| CatalogError::response())?;
        if !(200..300).contains(&response.status) {
            let value: Value = serde_json::from_slice(response.body.expose()).unwrap_or(Value::Null);
            let error = value
                .get("error")
                .filter(|error| error.is_object())
                .unwrap_or(&value);
            return Err(CatalogError {
                authentication_failure: response.status == 401,
                plan_required: error["code"] == "plan_required",
                status: Some(response.status),
            });
        }
        serde_json::from_slice(response.body.expose()).map_err(|_| CatalogError::response())
    }
    async fn user(&self, key: &SecretString) -> Result<Option<Value>, CatalogError> {
        match self.request(key, "/v1/me").await {
            Err(error) if error.account_blocked() => Ok(None),
            Err(error) => Err(error),
            Ok(user) if valid_user(&user) => Ok(Some(user)),
            Ok(_) => Err(CatalogError::response()),
        }
    }
    async fn models(&self, key: &SecretString) -> Result<Vec<Value>, CatalogError> {
        let digest: [u8; 32] = Sha256::digest(key.expose().as_bytes()).into();
        let mut cache = self.models.lock().await;
        if let Some(index) = cache.iter().position(|entry| entry.key == digest) {
            let entry = cache.remove(index).ok_or_else(CatalogError::response)?;
            if entry.expires > self.clock.now_ms() {
                let models = entry.models.clone();
                cache.push_back(entry);
                return Ok(models);
            }
        }
        let response = self.request(key, "/v1/models").await?;
        let models = response["items"]
            .as_array()
            .filter(|models| models.iter().all(valid_model))
            .ok_or_else(CatalogError::response)?
            .clone();
        if !models.is_empty() {
            cache.push_back(Cached {
                key: digest,
                expires: self.clock.now_ms().saturating_add(30 * 60 * 1000),
                models: models.clone(),
            });
            if cache.len() > 32 {
                cache.pop_front();
            }
        }
        Ok(models)
    }
    pub async fn read(&self, key: &SecretString) -> Result<CatalogSnapshot, CatalogError> {
        let (user, models) = tokio::try_join!(self.user(key), self.models(key))?;
        Ok(CatalogSnapshot { user, models })
    }
    pub async fn invalidate(&self) {
        self.models.lock().await.clear();
    }
}
fn optional(value: &Value, key: &str, valid: impl FnOnce(&Value) -> bool) -> bool {
    value.get(key).is_none_or(valid)
}
fn array(value: &Value, valid: impl Fn(&Value) -> bool) -> bool {
    value
        .as_array()
        .is_some_and(|values| values.iter().all(valid))
}
fn valid_user(value: &Value) -> bool {
    value["apiKeyName"].is_string()
        && value["createdAt"].is_string()
        && optional(value, "userId", Value::is_number)
        && ["userEmail", "userFirstName", "userLastName"]
            .iter()
            .all(|key| optional(value, key, Value::is_string))
}
fn valid_model(value: &Value) -> bool {
    value["id"].is_string()
        && value["displayName"].is_string()
        && optional(value, "description", Value::is_string)
        && optional(value, "aliases", |values| array(values, Value::is_string))
        && optional(value, "parameters", |values| {
            array(values, |parameter| {
                parameter["id"].is_string()
                    && optional(parameter, "displayName", Value::is_string)
                    && array(&parameter["values"], |entry| {
                        entry["value"].is_string()
                            && optional(entry, "displayName", Value::is_string)
                    })
            })
        })
        && optional(value, "variants", |values| {
            array(values, |variant| {
                variant["displayName"].is_string()
                    && optional(variant, "description", Value::is_string)
                    && optional(variant, "isDefault", Value::is_boolean)
                    && optional(variant, "isMaxMode", Value::is_boolean)
                    && array(&variant["params"], |parameter| {
                        parameter["id"].is_string() && parameter["value"].is_string()
                    })
            })
        })
}
