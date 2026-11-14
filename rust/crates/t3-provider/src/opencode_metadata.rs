use crate::opencode_http::OpenCodeHttp;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};
use tokio::sync::Mutex;

#[derive(Clone, Debug, PartialEq)]
pub struct CatalogModel {
    pub variants: BTreeSet<String>,
    pub context_window: Option<f64>,
}

pub struct OpenCodeModelMetadata {
    http: Arc<OpenCodeHttp>,
    directory: String,
    entries: Mutex<BTreeMap<String, Option<CatalogModel>>>,
}
impl OpenCodeModelMetadata {
    pub fn new(http: Arc<OpenCodeHttp>, directory: String) -> Self {
        Self {
            http,
            directory,
            entries: Mutex::new(BTreeMap::new()),
        }
    }
    pub async fn read(&self, provider: &str, model: &str) -> Option<CatalogModel> {
        let key = format!("{provider}/{model}");
        let mut entries = self.entries.lock().await;
        if let Some(entry) = entries.get(&key) {
            return entry.clone();
        }
        let response = self
            .http
            .get(
                "/provider",
                Some(&self.directory),
                1,
                Duration::from_secs(10),
            )
            .await
            .ok()?;
        let providers = response["all"].as_array()?;
        let entry = providers
            .iter()
            .find(|item| item["id"] == provider)
            .and_then(|item| item["models"].get(model))
            .filter(|item| item.is_object())
            .map(|item| CatalogModel {
                variants: item["variants"]
                    .as_object()
                    .map(|variants| variants.keys().cloned().collect())
                    .unwrap_or_default(),
                context_window: item["limit"]
                    .get("input")
                    .filter(|input| !input.is_null())
                    .unwrap_or(&item["limit"]["context"])
                    .as_f64()
                    .filter(|value| value.is_finite()),
            });
        entries.insert(key, entry.clone());
        entry
    }
    pub async fn resolve_variant(
        &self,
        provider: &str,
        model: &str,
        variant: Option<&str>,
    ) -> Option<String> {
        let variant = variant?;
        if variant.is_empty() {
            return Some(variant.into());
        }
        let entry = self.read(provider, model).await;
        if entry.is_some_and(|entry| !entry.variants.contains(variant)) {
            None
        } else {
            Some(variant.into())
        }
    }
    pub async fn invalidate(&self) {
        self.entries.lock().await.clear();
    }
}
