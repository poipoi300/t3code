use crate::{
    Error, Rejection, Result, Secret, SharedClock,
    cloud::{self, CloudLink, CredentialIssuer, LinkRequestContext, Registration},
    http::{Http, Sensitive},
    managed::{EndpointStatus, ManagedEndpoint},
    reachability::ConnectReachability,
    store::{SecretStore, read_text},
};
use serde::Serialize;
use std::{collections::BTreeMap, sync::Arc};
use t3_contracts::{auth::AuthEnvironmentScope as Scope, environment_http::*, relay::*};
use tokio::sync::{Mutex, watch};

pub struct RequestContext {
    pub request_url: Secret,
    pub subject: String,
    pub scopes: Vec<Scope>,
    pub forwarded_authority: bool,
}
impl std::fmt::Debug for RequestContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RequestContext([REDACTED])")
    }
}
impl RequestContext {
    pub fn require(&self, scope: Scope) -> Result<()> {
        if self.scopes.contains(&scope) {
            Ok(())
        } else {
            Err(Error::Remote {
                rejection: Rejection::Forbidden,
                retry_after_ms: None,
            })
        }
    }
    pub fn linkable_here(&self) -> bool {
        self.subject == "desktop-bootstrap"
            && !self.forwarded_authority
            && url::Url::parse(self.request_url.expose())
                .is_ok_and(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some())
    }
}

pub struct CloudServices {
    pub link: Arc<CloudLink>,
    pub secrets: SecretStore,
    pub runtime: Arc<dyn ManagedEndpoint>,
    pub reachability: Arc<ConnectReachability>,
    pub issuer: Arc<dyn CredentialIssuer>,
    pub origin: RelayManagedEndpointOrigin,
    pub connect_allowed: watch::Receiver<bool>,
}
pub struct CloudService {
    pub link: Arc<CloudLink>,
    secrets: SecretStore,
    runtime: Arc<dyn ManagedEndpoint>,
    reachability: Arc<ConnectReachability>,
    issuer: Arc<dyn CredentialIssuer>,
    origin: RelayManagedEndpointOrigin,
    allowed: watch::Receiver<bool>,
    mutation: Arc<Mutex<()>>,
}
impl CloudService {
    pub fn new(services: CloudServices) -> Result<Arc<Self>> {
        crate::direct::validate_tunnel_origin(&services.origin)?;
        Ok(Arc::new(Self {
            link: services.link,
            secrets: services.secrets,
            runtime: services.runtime,
            reachability: services.reachability,
            issuer: services.issuer,
            origin: services.origin,
            allowed: services.connect_allowed,
            mutation: Arc::new(Mutex::new(())),
        }))
    }
    fn allowed(&self) -> Result<()> {
        if *self.allowed.borrow() {
            Ok(())
        } else {
            Err(Error::Disabled)
        }
    }
    pub async fn state(&self) -> Result<EnvironmentCloudLinkStateResult> {
        let _lock = self.mutation.lock().await;
        self.state_unlocked().await
    }
    async fn state_unlocked(&self) -> Result<EnvironmentCloudLinkStateResult> {
        let user = read_text(self.secrets.as_ref(), cloud::USER).await?;
        let configured = self.secrets.get(cloud::RUNTIME).await?.is_some();
        let facts = if configured {
            self.runtime.inspect().await
        } else {
            None
        };
        let reachability = self.reachability.latest().await.filter(|result| {
            !configured
                || !matches!(
                    result,
                    EnvironmentConnectReachability::Unreachable {
                        cause: EnvironmentConnectReachabilityCause::NotLinked,
                        ..
                    }
                )
        });
        Ok(EnvironmentCloudLinkStateResult {
            linked: user.is_some(),
            cloud_user_id: user.clone(),
            relay_url: read_text(self.secrets.as_ref(), cloud::RELAY_URL).await?,
            relay_issuer: read_text(self.secrets.as_ref(), cloud::RELAY_ISSUER).await?,
            managed_tunnel_active: Some(Some(configured)),
            publish_agent_activity: read_text(self.secrets.as_ref(), cloud::PUBLISH)
                .await?
                .as_deref()
                == Some("true"),
            hold_webhooks_while_offline: Some(Some(
                read_text(self.secrets.as_ref(), cloud::HOLD)
                    .await?
                    .as_deref()
                    == Some("true"),
            )),
            link_revoked: Some(Some(
                user.is_some() && self.secrets.get(cloud::REVOKED).await?.is_some(),
            )),
            reachability: reachability.map(Some),
            tunnel_hostname: facts.and_then(|f| f.hostname).map(Some),
            linkable_here: None,
        })
    }
    pub async fn link_state(
        &self,
        context: &RequestContext,
    ) -> Result<EnvironmentCloudLinkStateResult> {
        context.require(Scope::RelayRead)?;
        let mut state = self.state().await?;
        state.linkable_here = Some(Some(context.linkable_here()));
        Ok(state)
    }
    pub fn link_proof(
        &self,
        context: &RequestContext,
        request: &Sensitive<RelayLinkProofRequest>,
    ) -> Result<Secret> {
        context.require(Scope::RelayWrite)?;
        self.allowed()?;
        self.link.link_proof_for_client(
            request,
            LinkRequestContext {
                request_url: context.request_url.expose(),
                desktop_bootstrap: context.subject == "desktop-bootstrap",
                forwarded_authority: context.forwarded_authority,
            },
        )
    }
    pub async fn relay_config(
        self: &Arc<Self>,
        context: &RequestContext,
        request: Sensitive<RelayEnvironmentConfigRequest>,
    ) -> Result<EnvironmentCloudRelayConfigResult> {
        context.require(Scope::RelayWrite)?;
        self.allowed()?;
        let service = self.clone();
        tokio::spawn(async move {
            let _lock = service.mutation.lock().await;
            let runtime = request.expose().endpoint_runtime.clone();
            if let Some(config) = &runtime {
                crate::tunnel::validate_runtime(config)?;
            }
            // Install validates account and trust before a running connector is stopped.
            service.link.install(request).await?;
            service.runtime.apply(None).await?;
            let Some(runtime) = runtime else {
                return Ok(EnvironmentCloudRelayConfigResult {
                    ok: true,
                    endpoint_runtime_status: crate::encode(&EndpointStatus::Disabled)?,
                });
            };
            match service.link.register(service.origin.clone()).await? {
                Registration::Ready => service.activate(runtime).await,
                Registration::RecoveryRequired => {
                    let response = service
                        .link
                        .recover_serialized(service.origin.clone(), Sensitive::new(runtime))
                        .await?;
                    service
                        .activate(response.expose().endpoint_runtime.clone())
                        .await
                }
                Registration::Superseded => Err(Error::Superseded),
                _ => Err(Error::EndpointUnavailable {
                    status: EndpointStatus::Disabled,
                }),
            }
        })
        .await
        .map_err(|_| Error::Process)?
    }
    async fn activate(
        &self,
        config: RelayManagedEndpointRuntimeConfig,
    ) -> Result<EnvironmentCloudRelayConfigResult> {
        self.allowed()?;
        let status = self
            .runtime
            .apply(Some(Sensitive::new(config.clone())))
            .await?;
        if !matches!(status, EndpointStatus::Running { .. }) {
            return Err(Error::EndpointUnavailable { status });
        }
        self.link
            .confirm_origin(&Sensitive::new(config), self.origin.clone())
            .await?;
        Ok(EnvironmentCloudRelayConfigResult {
            ok: true,
            endpoint_runtime_status: crate::encode(&status)?,
        })
    }
    pub async fn unlink(
        self: &Arc<Self>,
        context: &RequestContext,
    ) -> Result<EnvironmentCloudRelayConfigResult> {
        context.require(Scope::RelayWrite)?;
        let service = self.clone();
        tokio::spawn(async move {
            let _lock = service.mutation.lock().await;
            let status = service.runtime.apply(None).await?;
            service.link.unlink().await?;
            service.secrets.remove(cloud::HOLD).await?;
            Ok(EnvironmentCloudRelayConfigResult {
                ok: true,
                endpoint_runtime_status: crate::encode(&status)?,
            })
        })
        .await
        .map_err(|_| Error::Process)?
    }
    pub async fn preferences(
        &self,
        context: &RequestContext,
        request: EnvironmentCloudPreferencesRequest,
    ) -> Result<EnvironmentCloudLinkStateResult> {
        context.require(Scope::RelayWrite)?;
        self.allowed()?;
        let _lock = self.mutation.lock().await;
        self.link.update_preferences(request).await?;
        self.state_unlocked().await
    }
    pub async fn check_reachability(
        &self,
        context: &RequestContext,
    ) -> Result<EnvironmentConnectReachability> {
        context.require(Scope::RelayRead)?;
        self.reachability
            .check(
                self.secrets.get(cloud::RUNTIME).await?.is_some(),
                self.runtime.inspect().await,
            )
            .await
    }
    pub async fn health(
        &self,
        request: Sensitive<RelayCloudEnvironmentHealthRequest>,
    ) -> Result<Sensitive<RelayEnvironmentHealthResponse>> {
        self.allowed()?;
        self.link
            .health(Secret::new(&request.expose().proof.0))
            .await
    }
    pub async fn mint_credential(
        &self,
        request: Sensitive<RelayCloudMintCredentialRequest>,
    ) -> Result<Sensitive<RelayEnvironmentMintResponse>> {
        self.allowed()?;
        self.link
            .mint(Secret::new(&request.expose().proof.0), self.issuer.as_ref())
            .await
    }
    /// Call after MCP validates the client and redirect; peer links always approve locally.
    pub async fn approval_redirect(
        &self,
        issuer: &str,
        search: &str,
        peer_link: bool,
        hosted_app: &str,
    ) -> Result<Secret> {
        let fallback = || {
            Secret::new(format!(
                "/connect-agent{}",
                if search.is_empty() {
                    String::new()
                } else {
                    format!("?{}", search.trim_start_matches('?'))
                }
            ))
        };
        if peer_link {
            return Ok(fallback());
        }
        let state = match self.state().await {
            Ok(state) => state,
            Err(_) => return Ok(fallback()),
        };
        let origin = crate::http::secure_origin(issuer)?;
        if !state.linked
            || state.link_revoked.flatten() == Some(true)
            || state.tunnel_hostname.flatten().as_deref() != origin.host_str()
        {
            return Ok(fallback());
        }
        let mut url = crate::http::secure_origin(hosted_app)?;
        url.set_path("/approve-agent");
        url.set_query(Some(search.trim_start_matches('?')));
        let existing = url
            .query_pairs()
            .filter(|(key, _)| key != "environment")
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect::<Vec<_>>();
        url.set_query(None);
        url.query_pairs_mut()
            .extend_pairs(existing)
            .append_pair("environment", &self.link.descriptor.environment_id.0);
        Ok(Secret::new(url.to_string()))
    }
    pub async fn shutdown(&self) -> Result<()> {
        self.runtime.apply(None).await.map(|_| ())
    }
}

pub struct HttpReply<T> {
    pub status: u16,
    pub headers: BTreeMap<&'static str, &'static str>,
    pub body: Sensitive<T>,
}
impl<T> std::fmt::Debug for HttpReply<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HttpReply([REDACTED])")
    }
}
impl<T> HttpReply<T> {
    pub fn ok(value: T) -> Self {
        Self {
            status: 200,
            headers: BTreeMap::from([("cache-control", "no-store"), ("pragma", "no-cache")]),
            body: Sensitive::new(value),
        }
    }
}
#[derive(Debug)]
pub struct HttpFailure {
    pub status: u16,
    pub body: serde_json::Value,
}
impl From<Error> for HttpFailure {
    fn from(error: Error) -> Self {
        if let Error::EndpointUnavailable { status } = &error {
            return Self {
                status: 503,
                body: serde_json::json!({"_tag":"EnvironmentCloudEndpointUnavailableError","message":"The managed endpoint is unavailable.","endpointRuntimeStatus":status}),
            };
        }
        let (status, tag) = match &error {
            Error::Proof | Error::Expired | Error::LoginRequired => {
                (401, "EnvironmentHttpUnauthorizedError")
            }
            Error::Replay | Error::Identity | Error::Superseded => {
                (409, "EnvironmentHttpConflictError")
            }
            Error::Configuration | Error::NotLinked | Error::Protocol => {
                (400, "EnvironmentHttpBadRequestError")
            }
            Error::Disabled => (403, "EnvironmentHttpForbiddenError"),
            Error::Remote { rejection, .. } => match rejection {
                Rejection::Unauthorized => (401, "EnvironmentHttpUnauthorizedError"),
                Rejection::Forbidden => (403, "EnvironmentHttpForbiddenError"),
                Rejection::Rejected => (400, "EnvironmentHttpBadRequestError"),
                Rejection::Unavailable => (500, "EnvironmentHttpInternalServerError"),
            },
            _ => (500, "EnvironmentHttpInternalServerError"),
        };
        Self {
            status,
            body: serde_json::json!({"_tag":tag,"message":error.to_string()}),
        }
    }
}
pub fn reply<T: Serialize>(result: Result<T>) -> std::result::Result<HttpReply<T>, HttpFailure> {
    result.map(HttpReply::ok).map_err(Into::into)
}

pub async fn open_link(
    descriptor: t3_contracts::environment::ExecutionEnvironmentDescriptor,
    secrets: SecretStore,
    http: Http,
    clock: SharedClock,
) -> Result<Arc<CloudLink>> {
    CloudLink::new(descriptor, secrets, http, clock)
        .await
        .map(Arc::new)
}
