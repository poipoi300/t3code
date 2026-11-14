use crate::{
    discovery::{OpenCodeVersion, classify_opencode_response},
    environment::Environment,
    opencode_catalog::OpenCodeModel,
    opencode_loader::OpenCodeModels,
    sensitive::{SecretString, Sensitive, ServiceError, ServiceResult},
    transport::{HttpRequest, HttpTransport},
};
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc, time::Duration};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum OpenCodeError {
    #[error("Could not reach the OpenCode server.")]
    Unavailable,
    #[error("The OpenCode server rejected the server password.")]
    Authentication,
    #[error("The server did not identify itself as OpenCode.")]
    Identity,
    #[error("Timed out waiting for the OpenCode server.")]
    Timeout,
    #[error("Could not start the OpenCode server.")]
    Process,
}
impl From<OpenCodeError> for ServiceError {
    fn from(error: OpenCodeError) -> Self {
        match error {
            OpenCodeError::Unavailable | OpenCodeError::Timeout => ServiceError::Network,
            _ => ServiceError::Response,
        }
    }
}
pub struct OpenCodeHttp {
    transport: Arc<dyn HttpTransport>,
    base: url::Url,
    password: Option<SecretString>,
}
impl std::fmt::Debug for OpenCodeHttp {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("OpenCodeHttp([REDACTED])")
    }
}
pub struct LegacyInventory {
    pub providers: Value,
    pub agents: Vec<Value>,
    pub skills: Vec<Value>,
    pub commands: Vec<Value>,
}
impl OpenCodeHttp {
    pub fn event_request(&self) -> HttpRequest {
        let mut request = self.request("/api/event", None, 2);
        request
            .headers
            .insert("accept".into(), Sensitive::new("text/event-stream".into()));
        request
    }
    pub fn new(
        transport: Arc<dyn HttpTransport>,
        base_url: &str,
        password: Option<SecretString>,
    ) -> Result<Self, OpenCodeError> {
        let base = url::Url::parse(base_url.trim()).map_err(|_| OpenCodeError::Identity)?;
        if !matches!(base.scheme(), "http" | "https") || base.host_str().is_none() {
            return Err(OpenCodeError::Identity);
        }
        Ok(Self {
            transport,
            base,
            password,
        })
    }
    fn request(&self, path: &str, directory: Option<&str>, generation: u8) -> HttpRequest {
        let mut url = self.base.clone();
        url.set_path(&format!("{}{path}", self.base.path().trim_end_matches('/')));
        url.set_query(None);
        url.set_fragment(None);
        if let Some(directory) = directory {
            url.query_pairs_mut().append_pair(
                if generation == 2 {
                    "location[directory]"
                } else {
                    "directory"
                },
                directory,
            );
        }
        let mut headers =
            BTreeMap::from([("accept".into(), Sensitive::new("application/json".into()))]);
        if let Some(password) = self.password.as_ref() {
            headers.insert(
                "authorization".into(),
                Sensitive::new(format!(
                    "Basic {}",
                    STANDARD.encode(format!("opencode:{}", password.expose()))
                )),
            );
        }
        HttpRequest {
            method: "GET",
            url: url.into(),
            headers,
            body: None,
        }
    }
    pub async fn get(
        &self,
        path: &str,
        directory: Option<&str>,
        generation: u8,
        timeout: Duration,
    ) -> Result<Value, OpenCodeError> {
        let response = self.raw(path, directory, generation, timeout).await?;
        if matches!(response.status, 401 | 403) {
            return Err(OpenCodeError::Authentication);
        }
        if !(200..300).contains(&response.status) {
            return Err(OpenCodeError::Unavailable);
        }
        let json = response
            .content_type
            .as_deref()
            .and_then(|value| value.split(';').next())
            .is_some_and(|value| {
                value.trim() == "application/json" || value.trim().ends_with("+json")
            });
        if !json {
            return Err(OpenCodeError::Identity);
        }
        serde_json::from_slice(response.body.expose()).map_err(|_| OpenCodeError::Identity)
    }
    pub async fn raw(
        &self,
        path: &str,
        directory: Option<&str>,
        generation: u8,
        timeout: Duration,
    ) -> Result<crate::transport::RawHttpResponse, OpenCodeError> {
        tokio::time::timeout(
            timeout,
            self.transport
                .raw(self.request(path, directory, generation)),
        )
        .await
        .map_err(|_| OpenCodeError::Timeout)?
        .map_err(|_| OpenCodeError::Unavailable)
    }
    pub async fn probe(&self, environment: &Environment) -> Result<OpenCodeVersion, OpenCodeError> {
        for path in ["/api/info", "/global/health"] {
            let response = tokio::time::timeout(
                Duration::from_millis(crate::versions::probe_timeout_ms(
                    "opencode",
                    5_000,
                    environment,
                )),
                self.transport.raw(self.request(path, None, 0)),
            )
            .await
            .map_err(|_| OpenCodeError::Timeout)?
            .map_err(|_| OpenCodeError::Unavailable)?;
            if matches!(response.status, 401 | 403) {
                return Err(OpenCodeError::Authentication);
            }
            if let Some(version) = classify_opencode_response(
                path,
                response.status,
                response.content_type.as_deref(),
                &String::from_utf8_lossy(response.body.expose()),
            )
            .map_err(|_| OpenCodeError::Identity)?
            {
                return Ok(version);
            }
        }
        Err(OpenCodeError::Identity)
    }
    pub async fn inventory_v1(
        &self,
        directory: &str,
        timeout: Duration,
    ) -> Result<LegacyInventory, OpenCodeError> {
        let (providers, agents, skills, commands) = tokio::join!(
            self.get("/provider", Some(directory), 1, timeout),
            self.get("/agent", Some(directory), 1, timeout),
            self.get("/skill", Some(directory), 1, timeout),
            self.get("/command", Some(directory), 1, timeout)
        );
        let array = |result: Result<Value, OpenCodeError>| {
            result
                .ok()
                .and_then(|value| value.as_array().cloned())
                .unwrap_or_default()
        };
        Ok(LegacyInventory {
            providers: providers?,
            agents: array(agents),
            skills: array(skills),
            commands: array(commands),
        })
    }
    pub async fn workspace_v2(
        &self,
        directory: &str,
        timeout: Duration,
    ) -> Result<(Vec<Value>, Vec<Value>), OpenCodeError> {
        let (skills, commands) = tokio::try_join!(
            self.get("/api/skill", Some(directory), 2, timeout),
            self.get("/api/command", Some(directory), 2, timeout)
        )?;
        Ok((
            skills["data"]
                .as_array()
                .ok_or(OpenCodeError::Identity)?
                .clone(),
            commands["data"]
                .as_array()
                .ok_or(OpenCodeError::Identity)?
                .clone(),
        ))
    }
}
pub struct HttpModels {
    pub connection: Arc<OpenCodeHttp>,
    pub directory: String,
    pub strict_names: bool,
}
#[async_trait]
impl OpenCodeModels for HttpModels {
    async fn list(&self) -> ServiceResult<Vec<OpenCodeModel>> {
        let (models, providers) = tokio::join!(
            self.connection.get(
                "/api/model",
                Some(&self.directory),
                2,
                Duration::from_secs(5)
            ),
            self.connection.get(
                "/api/provider",
                Some(&self.directory),
                2,
                Duration::from_secs(3)
            )
        );
        let models = match models {
            Ok(models) => models,
            Err(OpenCodeError::Timeout) => return Ok(vec![]),
            Err(error) => return Err(error.into()),
        };
        let providers = match providers {
            Ok(providers) => match providers["data"].as_array().filter(|providers| {
                providers
                    .iter()
                    .all(|provider| provider["id"].is_string() && provider["name"].is_string())
            }) {
                Some(providers) => providers.clone(),
                None if self.strict_names => return Err(ServiceError::Response),
                None => vec![],
            },
            Err(error) if self.strict_names => return Err(error.into()),
            Err(_) => vec![],
        };
        let names: BTreeMap<_, _> = providers
            .iter()
            .filter_map(|provider| Some((provider["id"].as_str()?, provider["name"].as_str()?)))
            .collect();
        models["data"].as_array().ok_or(ServiceError::Response)?.iter().map(|model|{
            let mut normalized=json!({"providerID":model["providerID"],"id":model["id"],"name":model["name"],"variants":model["variants"],"released":model["time"]["released"],"deprecated":model["status"]=="deprecated"});
            if let Some(family)=model.get("family") { normalized["family"]=family.clone(); }
            if let Some(name)=model["providerID"].as_str().and_then(|id|names.get(id)).filter(|name|!name.is_empty()) { normalized["providerName"]=json!(name); }
            serde_json::from_value(normalized).map_err(|_|ServiceError::Response)
        }).collect()
    }
}
