use super::Secrets;
use crate::{
    Error, Result,
    host::AgentEndpoint,
    io::{Http, HttpRequest, Secret, request_bytes},
};
use async_trait::async_trait;
use http::Method;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use t3_contracts::{
    device::{DeviceEasApp, DeviceEasCredentialSource, DevicePlatform},
    settings_secret_value::SettingsSecretValue,
};

#[derive(Clone, Debug)]
pub struct Credential {
    pub source: DeviceEasCredentialSource,
    pub header: &'static str,
    pub value: Secret,
}
pub async fn resolve_credential(saved: &str, env: Option<&str>, home: &Path) -> Result<Credential> {
    let (source, header, token) = if !saved.trim().is_empty() {
        (
            DeviceEasCredentialSource::Settings,
            "authorization",
            saved.trim().to_owned(),
        )
    } else if let Some(env) = env.filter(|s| !s.trim().is_empty()) {
        (
            DeviceEasCredentialSource::Env,
            "authorization",
            env.trim().to_owned(),
        )
    } else {
        let value = tokio::fs::read(home.join(".expo/state.json"))
            .await
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok());
        let token=value.as_ref().and_then(|v|v.pointer("/auth/sessionSecret")).and_then(Value::as_str).map(str::trim).filter(|s|!s.is_empty()).ok_or_else(||Error::Unavailable("No Expo credential on the server: save an access token for this host, set EXPO_TOKEN, or run `eas login`.".into()))?;
        (
            DeviceEasCredentialSource::EasLogin,
            "expo-session",
            token.to_owned(),
        )
    };
    if !token.bytes().all(|b| (0x21..=0x7e).contains(&b)) {
        return Err(Error::Unavailable(format!(
            "{} is not a usable Expo credential. Replace it.",
            match source {
                DeviceEasCredentialSource::Settings => "The access token saved for this host",
                DeviceEasCredentialSource::Env => "EXPO_TOKEN",
                DeviceEasCredentialSource::EasLogin => "The eas login session",
            }
        )));
    }
    Ok(Credential {
        source,
        header,
        value: Secret::new(if header == "authorization" {
            format!("Bearer {token}")
        } else {
            token
        }),
    })
}
#[derive(Clone, Debug, thiserror::Error)]
#[error("Expo could not {operation}: {detail}")]
pub struct ExpoError {
    pub operation: String,
    pub detail: String,
    pub rejected: bool,
    pub status: Option<u16>,
}
impl From<ExpoError> for Error {
    fn from(value: ExpoError) -> Self {
        Error::Unavailable(value.to_string())
    }
}
pub type ExpoResult<T> = std::result::Result<T, ExpoError>;
#[derive(Clone)]
pub struct Session {
    pub id: String,
    pub status: String,
    pub agent: Option<AgentEndpoint>,
    pub preview: Option<(String, Secret)>,
}
impl Session {
    pub fn ended(&self) -> bool {
        ["STOPPED", "ERRORED"].contains(&self.status.as_str())
    }
}
#[derive(Clone, Debug)]
pub struct Active {
    pub id: String,
    pub name: Option<String>,
    pub status: String,
}
#[derive(Clone)]
pub struct Create {
    pub app: String,
    pub platform: DevicePlatform,
    pub name: String,
    pub max_minutes: u64,
    pub idle_minutes: u64,
}
#[async_trait]
pub trait ExpoApi: Send + Sync {
    async fn viewer(&self) -> ExpoResult<(String, Vec<DeviceEasApp>)>;
    async fn availability(&self, app: &str) -> ExpoResult<(String, bool)>;
    async fn create(&self, input: Create) -> ExpoResult<String>;
    async fn session(&self, id: &str) -> ExpoResult<Session>;
    async fn active(&self, app: &str) -> ExpoResult<Vec<Active>>;
    async fn stop(&self, id: &str) -> ExpoResult<()>;
}
#[async_trait]
pub trait ExpoProvider: Send + Sync {
    async fn api(&self) -> Result<Arc<dyn ExpoApi>>;
}
pub struct SavedExpo {
    pub http: Arc<dyn Http>,
    pub secrets: Arc<dyn Secrets>,
    pub saved: SettingsSecretValue,
    pub environment: BTreeMap<String, String>,
    pub home: PathBuf,
}
#[async_trait]
impl ExpoProvider for SavedExpo {
    async fn api(&self) -> Result<Arc<dyn ExpoApi>> {
        let saved = self.secrets.resolve(&self.saved).await?;
        let credential = resolve_credential(
            saved.expose(),
            self.environment.get("EXPO_TOKEN").map(String::as_str),
            &self.home,
        )
        .await?;
        Ok(Arc::new(ExpoHttp::new(self.http.clone(), credential)))
    }
}
pub struct ExpoHttp {
    http: Arc<dyn Http>,
    credential: Credential,
}
impl ExpoHttp {
    pub fn new(http: Arc<dyn Http>, credential: Credential) -> Self {
        Self { http, credential }
    }
    async fn call(&self, name: &str, operation: &str, variables: Value) -> ExpoResult<Value> {
        let error = |detail: &str, rejected, status| ExpoError {
            operation: operation.into(),
            detail: detail
                .replace(self.credential.value.expose(), "[redacted]")
                .replace(
                    self.credential
                        .value
                        .expose()
                        .strip_prefix("Bearer ")
                        .unwrap_or(self.credential.value.expose()),
                    "[redacted]",
                )
                .chars()
                .take(300)
                .collect(),
            rejected,
            status,
        };
        let query =
            query(name).ok_or_else(|| error("the request could not be encoded", true, None))?;
        let mut request = HttpRequest::new(
            Method::POST,
            "https://api.expo.dev/graphql",
            Duration::from_secs(30),
        )
        .map_err(|_| error("the request could not be encoded", true, None))?
        .json(&json!({"query":query,"operationName":name,"variables":variables}))
        .map_err(|_| error("the request could not be encoded", true, None))?;
        request
            .headers
            .insert(self.credential.header.into(), self.credential.value.clone());
        let (status, bytes) = request_bytes(self.http.as_ref(), request)
            .await
            .map_err(|e| {
                error(
                    if e.to_string().contains("timed out") {
                        "Expo's API did not answer in time"
                    } else {
                        "Expo's API is unreachable"
                    },
                    false,
                    None,
                )
            })?;
        if status == 401 || status == 403 {
            return Err(error(
                &format!(
                    "the {} was rejected (HTTP {status})",
                    if self.credential.source == DeviceEasCredentialSource::EasLogin {
                        "eas login session"
                    } else {
                        "access token"
                    }
                ),
                true,
                Some(status),
            ));
        }
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|_| error("unexpected response", false, Some(status)))?;
        if let Some(errors) = value["errors"].as_array().filter(|e| !e.is_empty()) {
            let fault = errors.iter().any(|e| {
                e["extensions"]["isTransient"] == true
                    || e["extensions"]
                        .get("errorType")
                        .is_some_and(|v| v != "USER")
                    || e["extensions"]["errorCode"] == "INTERNAL_SERVER_ERROR"
            });
            let users = errors
                .iter()
                .all(|e| e["extensions"]["errorType"] == "USER");
            let uncertain = fault || value.get("data").is_some_and(|v| !(v.is_null() && users));
            let detail = errors
                .iter()
                .filter_map(|e| e["message"].as_str())
                .collect::<Vec<_>>()
                .join("; ");
            return Err(error(&detail, status < 500 && !uncertain, Some(status)));
        }
        if !(200..300).contains(&status) {
            return Err(error(
                &format!("unexpected response (HTTP {status})"),
                status < 500,
                Some(status),
            ));
        }
        value
            .get("data")
            .filter(|v| v.is_object())
            .cloned()
            .ok_or_else(|| error("unexpected response", false, Some(status)))
    }
}
fn decode_error(operation: &str) -> ExpoError {
    ExpoError {
        operation: operation.into(),
        detail: "unexpected response".into(),
        rejected: false,
        status: None,
    }
}
fn session_status(value: &Value) -> Option<&str> {
    value.as_str().filter(|status| {
        [
            "ERRORED",
            "IN_PROGRESS",
            "NEW",
            "QUEUED",
            "STARTING",
            "STOPPED",
        ]
        .contains(status)
    })
}
fn nullable_string(value: Option<&Value>) -> bool {
    value.is_some_and(|value| value.is_null() || value.is_string())
}
#[async_trait]
impl ExpoApi for ExpoHttp {
    async fn viewer(&self) -> ExpoResult<(String, Vec<DeviceEasApp>)> {
        let v = self
            .call("T3ExpoViewer", "read the signed-in account", json!({}))
            .await?;
        let actor = &v["meActor"];
        let typename = actor["__typename"]
            .as_str()
            .ok_or_else(|| decode_error("read the signed-in account"))?;
        let name = match typename {
            "User" | "SSOUser" => actor["username"]
                .as_str()
                .ok_or_else(|| decode_error("read the signed-in account"))?,
            "Robot" => {
                if !nullable_string(actor.get("firstName")) {
                    return Err(decode_error("read the signed-in account"));
                }
                actor["firstName"].as_str().unwrap_or(typename)
            }
            _ => typename,
        };
        let mut apps = Vec::new();
        for account in actor["accounts"]
            .as_array()
            .ok_or_else(|| decode_error("read the signed-in account"))?
        {
            if !account["name"].is_string() {
                return Err(decode_error("read the signed-in account"));
            }
            for edge in account["appsPaginated"]["edges"]
                .as_array()
                .ok_or_else(|| decode_error("read the signed-in account"))?
            {
                apps.push(
                    serde_json::from_value(edge["node"].clone())
                        .map_err(|_| decode_error("read the signed-in account"))?,
                );
            }
        }
        Ok((name.into(), apps))
    }
    async fn availability(&self, app: &str) -> ExpoResult<(String, bool)> {
        let v = self
            .call(
                "T3EasSimulatorAvailability",
                "check EAS Simulator access",
                json!({"appId":app,"filter":["device-run-sessions"]}),
            )
            .await?;
        let account = &v["app"]["byId"]["ownerAccount"];
        Ok((
            account["name"]
                .as_str()
                .ok_or_else(|| decode_error("check EAS Simulator access"))?
                .into(),
            account["accountFeatureGates"]["device-run-sessions"] == true,
        ))
    }
    async fn create(&self, input: Create) -> ExpoResult<String> {
        let v=self.call("T3CreateEasSimulatorSession","start a simulator session",json!({"input":{"appId":input.app,"platform":if input.platform==DevicePlatform::Ios{"IOS"}else{"ANDROID"},"type":"AGENT_DEVICE","name":input.name,"maxRunTimeMinutes":input.max_minutes,"maxIdleTimeMinutes":input.idle_minutes,"corsOrigins":["http://localhost"]}})).await?;
        v.pointer("/deviceRunSession/createDeviceRunSession/id")
            .and_then(Value::as_str)
            .map(Into::into)
            .ok_or_else(|| decode_error("start a simulator session"))
    }
    async fn session(&self, id: &str) -> ExpoResult<Session> {
        let v = self
            .call(
                "T3EasSimulatorSession",
                "read a simulator session",
                json!({"id":id}),
            )
            .await?;
        let s = &v["deviceRunSessions"]["byId"];
        let remote = s
            .get("remoteConfig")
            .ok_or_else(|| decode_error("read a simulator session"))?;
        if !remote.is_null() && !remote["__typename"].is_string() {
            return Err(decode_error("read a simulator session"));
        }
        let mut agent = None;
        let mut preview = None;
        if remote["__typename"] == "AgentDeviceRunSessionRemoteConfig" {
            if !nullable_string(remote.get("previewApiUrl"))
                || !nullable_string(remote.get("webPreviewToken"))
            {
                return Err(decode_error("read a simulator session"));
            }
            let url = remote["agentDeviceRemoteSessionUrl"]
                .as_str()
                .ok_or_else(|| decode_error("read a simulator session"))?;
            crate::io::validated_url(url).map_err(|_| decode_error("read a simulator session"))?;
            agent = Some(AgentEndpoint {
                base_url: url.into(),
                token: Secret::new(
                    remote["agentDeviceRemoteSessionToken"]
                        .as_str()
                        .ok_or_else(|| decode_error("read a simulator session"))?,
                ),
                entry_path: PathBuf::new(),
                agent_device_id: None,
                session: None,
                state_dir: None,
                install_directory: None,
            });
            if let (Some(url), Some(token)) = (
                remote["previewApiUrl"].as_str(),
                remote["webPreviewToken"].as_str(),
            ) {
                crate::io::validated_url(url)
                    .map_err(|_| decode_error("read a simulator session"))?;
                preview = Some((url.into(), Secret::new(token)));
            }
        }
        Ok(Session {
            id: s["id"]
                .as_str()
                .ok_or_else(|| decode_error("read a simulator session"))?
                .into(),
            status: session_status(&s["status"])
                .ok_or_else(|| decode_error("read a simulator session"))?
                .into(),
            agent,
            preview,
        })
    }
    async fn active(&self, app: &str) -> ExpoResult<Vec<Active>> {
        let v=self.call("T3ActiveEasSimulatorSessions","list simulator sessions",json!({"appId":app,"filter":{"statuses":["NEW","QUEUED","STARTING","IN_PROGRESS"]}})).await?;
        v.pointer("/app/byId/deviceRunSessionsPaginated/edges")
            .and_then(Value::as_array)
            .ok_or_else(|| decode_error("list simulator sessions"))?
            .iter()
            .map(|edge| {
                let node = &edge["node"];
                Ok(Active {
                    id: node["id"]
                        .as_str()
                        .ok_or_else(|| decode_error("list simulator sessions"))?
                        .into(),
                    name: if node.get("name").is_some_and(Value::is_null) {
                        None
                    } else {
                        Some(
                            node["name"]
                                .as_str()
                                .ok_or_else(|| decode_error("list simulator sessions"))?
                                .into(),
                        )
                    },
                    status: session_status(&node["status"])
                        .ok_or_else(|| decode_error("list simulator sessions"))?
                        .into(),
                })
            })
            .collect()
    }
    async fn stop(&self, id: &str) -> ExpoResult<()> {
        let v = self
            .call(
                "T3StopEasSimulatorSession",
                "stop a simulator session",
                json!({"id":id}),
            )
            .await?;
        if v.pointer("/deviceRunSession/ensureDeviceRunSessionStopped/status")
            .and_then(session_status)
            .is_none()
        {
            return Err(decode_error("stop a simulator session"));
        }
        Ok(())
    }
}
fn query(name: &str) -> Option<String> {
    let source = include_str!("../expo.graphql");
    let position = source
        .find(&format!("query {name}"))
        .or_else(|| source.find(&format!("mutation {name}")))?;
    let tail = &source[position..];
    let end = tail[1..]
        .find("\nquery ")
        .map(|i| i + 1)
        .into_iter()
        .chain(tail[1..].find("\nmutation ").map(|i| i + 1))
        .min()
        .unwrap_or(tail.len());
    Some(tail[..end].into())
}
