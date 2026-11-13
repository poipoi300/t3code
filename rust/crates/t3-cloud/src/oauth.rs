use crate::{
    Error, Result, Secret, SharedClock,
    http::{Http, Request, check},
    store::{SecretStore, read_text},
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tokio::sync::Mutex;
use url::Url;

const NAME: &str = "cloud-cli-oauth-token";
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedToken {
    pub access_token: Secret,
    pub refresh_token: Secret,
    pub expires_at_epoch_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
}
#[derive(Deserialize)]
struct TokenResponse {
    access_token: Secret,
    refresh_token: Option<Secret>,
    expires_in: i64,
    token_type: String,
}
pub struct TokenManager {
    secrets: SecretStore,
    http: Http,
    clock: SharedClock,
    endpoint: Url,
    client_id: String,
    lock: Arc<Mutex<()>>,
}
impl TokenManager {
    pub fn new(
        secrets: SecretStore,
        http: Http,
        clock: SharedClock,
        endpoint: &str,
        client_id: String,
    ) -> Result<Self> {
        let endpoint = Url::parse(endpoint).map_err(|_| Error::Configuration)?;
        if endpoint.scheme() != "https"
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.fragment().is_some()
            || client_id.is_empty()
        {
            return Err(Error::Configuration);
        }
        Ok(Self {
            secrets,
            http,
            clock,
            endpoint,
            client_id,
            lock: Arc::new(Mutex::new(())),
        })
    }
    pub async fn store(&self, token: &PersistedToken) -> Result<()> {
        let _lock = self.lock.lock().await;
        Self::persist(self.secrets.as_ref(), token).await
    }
    async fn persist(store: &dyn crate::store::Secrets, token: &PersistedToken) -> Result<()> {
        let encoded =
            zeroize::Zeroizing::new(serde_json::to_vec(token).map_err(|_| Error::Storage)?);
        store.set(NAME, &encoded).await
    }
    pub async fn clear(&self) -> Result<()> {
        let _lock = self.lock.lock().await;
        self.secrets.remove(NAME).await
    }
    pub async fn has_credential(&self) -> Result<bool> {
        let _lock = self.lock.lock().await;
        Ok(read_text(self.secrets.as_ref(), NAME).await?.is_some())
    }
    /// Complete exchange and persistence in an owned task even if the requester disconnects.
    pub async fn get_existing(&self) -> Result<Option<PersistedToken>> {
        let lock = self.lock.clone().lock_owned().await;
        let secrets = self.secrets.clone();
        let http = self.http.clone();
        let clock = self.clock.clone();
        let endpoint = self.endpoint.clone();
        let client_id = self.client_id.clone();
        tokio::spawn(async move {
            let _lock = lock;
            let Some(raw) = read_text(secrets.as_ref(), NAME).await? else {
                return Ok(None);
            };
            let token: PersistedToken = serde_json::from_str(&raw).map_err(|_| Error::Storage)?;
            let now = clock.now_ms();
            if token.expires_at_epoch_ms.saturating_sub(300_000) > now {
                return Ok(Some(token));
            }
            let body = url::form_urlencoded::Serializer::new(String::new())
                .extend_pairs([
                    ("grant_type", "refresh_token"),
                    ("refresh_token", token.refresh_token.expose()),
                    ("client_id", client_id.as_str()),
                ])
                .finish()
                .into_bytes();
            let response = http
                .send(Request {
                    method: reqwest::Method::POST,
                    url: endpoint,
                    headers: BTreeMap::from([(
                        "content-type".to_owned(),
                        Secret::new("application/x-www-form-urlencoded"),
                    )]),
                    body,
                    timeout: Duration::from_secs(10),
                })
                .await?;
            if (400..500).contains(&response.status) && !matches!(response.status, 408 | 429) {
                return Err(Error::LoginRequired);
            }
            check(&response, clock.now_ms())?;
            let body: TokenResponse =
                serde_json::from_slice(&response.body).map_err(|_| Error::Protocol)?;
            if body.expires_in <= 0
                || body.access_token.expose().is_empty()
                || !body.token_type.eq_ignore_ascii_case("bearer")
            {
                return Err(Error::Protocol);
            }
            let refreshed = PersistedToken {
                access_token: body.access_token,
                refresh_token: body.refresh_token.unwrap_or(token.refresh_token),
                expires_at_epoch_ms: clock
                    .now_ms()
                    .checked_add(body.expires_in.checked_mul(1000).ok_or(Error::Protocol)?)
                    .ok_or(Error::Protocol)?,
                identity: token.identity,
            };
            Self::persist(secrets.as_ref(), &refreshed).await?;
            Ok(Some(refreshed))
        })
        .await
        .map_err(|_| Error::Storage)?
    }
}
