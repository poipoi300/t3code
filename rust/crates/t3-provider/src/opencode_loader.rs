use crate::{
    environment::Environment,
    opencode_catalog::OpenCodeModel,
    sensitive::{ServiceError, ServiceResult},
};
use async_trait::async_trait;
use std::{sync::Arc, time::Duration};
use tokio::sync::Mutex;
#[async_trait]
pub trait OpenCodeModels: Send + Sync {
    async fn list(&self) -> ServiceResult<Vec<OpenCodeModel>>;
}
fn fingerprint(models: &[OpenCodeModel]) -> ServiceResult<Vec<String>> {
    let mut rows = models
        .iter()
        .map(|model| {
            let mut variants: Vec<_> = model
                .variants
                .iter()
                .map(|variant| variant.id.as_str())
                .collect();
            variants.sort();
            serde_json::to_string(&(
                &model.provider_id,
                &model.id,
                &model.name,
                &model.provider_name,
                variants,
            ))
            .map_err(|_| ServiceError::Response)
        })
        .collect::<ServiceResult<Vec<_>>>()?;
    rows.sort();
    Ok(rows)
}
async fn settle(
    source: &dyn OpenCodeModels,
    first: Option<&[OpenCodeModel]>,
    latest: &mut Vec<OpenCodeModel>,
) -> ServiceResult<Vec<OpenCodeModel>> {
    let mut previous = first
        .filter(|models| !models.is_empty())
        .map(fingerprint)
        .transpose()?;
    if first.is_some() {
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    loop {
        let models = source.list().await?;
        let next = (!models.is_empty())
            .then(|| fingerprint(&models))
            .transpose()?;
        if !models.is_empty() {
            *latest = models.clone();
        }
        if next.is_some() && previous == next {
            return Ok(models);
        }
        previous = next;
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}
pub struct OpenCodeModelLoader {
    source: Arc<dyn OpenCodeModels>,
    timeout: Duration,
    cached: Mutex<Vec<OpenCodeModel>>,
}
impl OpenCodeModelLoader {
    pub fn new(source: Arc<dyn OpenCodeModels>, environment: &Environment) -> Self {
        Self {
            source,
            timeout: Duration::from_millis(crate::versions::probe_timeout_ms(
                "opencode",
                5_000,
                environment,
            )),
            cached: Mutex::new(vec![]),
        }
    }
    pub async fn load(&self) -> ServiceResult<Vec<OpenCodeModel>> {
        let mut cached = self.cached.lock().await;
        let initial = async {
            loop {
                match self.source.list().await {
                    Err(ServiceError::ActivatingPlugins) => {
                        tokio::time::sleep(Duration::from_secs(1)).await
                    }
                    result => return result,
                }
            }
        };
        let first = match tokio::time::timeout(Duration::from_secs(120), initial).await {
            Ok(result) => result?,
            Err(_) => return Ok(cached.clone()),
        };
        let mut latest = first.clone();
        let settled = tokio::time::timeout(
            self.timeout,
            settle(&*self.source, Some(&first), &mut latest),
        )
        .await;
        match settled {
            Ok(Ok(models)) => *cached = models,
            Ok(Err(error)) if latest.is_empty() => return Err(error),
            _ if !latest.is_empty() => *cached = latest,
            _ => {}
        }
        Ok(cached.clone())
    }
    pub async fn read_settled(&self) -> Option<Vec<OpenCodeModel>> {
        tokio::time::timeout(self.timeout, settle(&*self.source, None, &mut vec![]))
            .await
            .ok()?
            .ok()
    }
    pub async fn invalidate(&self) {
        self.cached.lock().await.clear();
    }
}
