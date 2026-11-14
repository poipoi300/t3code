//! The only contact surface with `t3-cloud` (built with its `auth` feature); compiled with the
//! `cloud` feature.
//!
//! One `CloudService` per environment (`cloud/CloudLink.ts`): its Connect mint always goes
//! through the concrete `AuthCredentialIssuer` over the shared `EnvironmentAuth` and the live
//! settings, so the device-approval setting decides every mint. The relay client
//! (cloudflared) is resolved and installed from host configuration, never from RPC input.
use std::{path::PathBuf, sync::Arc};
use t3_cloud::{
    SharedClock, SystemClock,
    api::{CloudService, CloudServices, RequestContext},
    cloud::{AuthCredentialIssuer, CloudLink},
    http::{Http, ReqwestTransport},
    managed::NativeManagedEndpoint,
    reachability::{ConnectReachability, NativeReachabilityProbe},
    relay_client::{NativeDownload, NativeProbe, RelayClient, RelayClientOptions, pinned_asset},
    store::SecretStore,
};

pub use t3_cloud::Error as CloudError;

pub struct CloudAdapter {
    pub service: Arc<CloudService>,
    pub relay_client: Arc<RelayClient>,
    origin: String,
}

impl CloudAdapter {
    #[cfg(feature = "local-services")]
    pub async fn start(context: &crate::services::DomainContext<'_>) -> crate::Result<Arc<Self>> {
        let service = |e: &dyn std::fmt::Display| crate::Error::Service(e.to_string());
        let local = context.local;
        let config = &local.config;
        let secrets: SecretStore = Arc::new(
            t3_auth::ServerSecretStore::open(config.paths.secrets())
                .await
                .map_err(|e| service(&e))?,
        );
        let http: Http = Arc::new(ReqwestTransport::new().map_err(|e| service(&e))?);
        let clock: SharedClock = Arc::new(SystemClock);
        let link = Arc::new(
            CloudLink::new(
                local.environment.clone(),
                secrets.clone(),
                http.clone(),
                clock.clone(),
            )
            .await
            .map_err(|e| service(&e))?,
        );
        // The organisation's policy is fixed for the process; Connect and the managed relay
        // client install follow it.
        let connect = t3_host::policy::ManagedPolicy::from_document(local.managed_policy.clone())
            .map_err(|e| service(&e))?
            .allows_connect();
        let (_, allowed) = tokio::sync::watch::channel(connect);
        let (platform, arch) = platform();
        let relay_client = RelayClient::new(
            RelayClientOptions {
                base_dir: config.paths.base_dir.clone(),
                platform: platform.into(),
                arch: arch.into(),
                executable_override: context
                    .environment
                    .get("T3CODE_CLOUDFLARED_PATH")
                    .filter(|path| !path.is_empty())
                    .map(PathBuf::from),
                search_path: context
                    .environment
                    .iter()
                    .find(|(key, _)| key.eq_ignore_ascii_case("PATH"))
                    .map(|(_, path)| std::env::split_paths(path).collect())
                    .unwrap_or_default(),
                release: pinned_asset(platform, arch),
                install_allowed: allowed.clone(),
            },
            Arc::new(NativeDownload::new().map_err(|e| service(&e))?),
            Arc::new(NativeProbe),
        );
        let runtime = NativeManagedEndpoint::with_relay_client(
            relay_client.clone(),
            http.clone(),
            secrets.clone(),
            clock.clone(),
        );
        let reachability = Arc::new(ConnectReachability::new(
            Arc::new(NativeReachabilityProbe { http }),
            clock,
        ));
        let host = config
            .host
            .clone()
            .and_then(|host| t3_auth::utils::loopback_bind_address(Some(&host)).or(Some(host)))
            .filter(|host| !matches!(host.as_str(), "" | "0.0.0.0" | "::" | "[::]"))
            .unwrap_or_else(|| "127.0.0.1".into());
        let origin = t3_contracts::relay::RelayManagedEndpointOrigin {
            local_http_host: t3_contracts::base_schemas::TrimmedNonEmptyString(host.clone()),
            local_http_port: config.port,
        };
        let service = CloudService::new(CloudServices {
            link,
            secrets,
            runtime,
            reachability,
            issuer: Arc::new(AuthCredentialIssuer::new(
                local.auth.inner().clone(),
                local.settings.clone(),
            )),
            origin,
            connect_allowed: allowed,
        })
        .map_err(|e| service(&e))?;
        Ok(Arc::new(Self {
            service,
            relay_client,
            origin: format!("http://{host}:{}", config.port),
        }))
    }

    /// The verified session as the cloud services see it: scopes and subject come from the
    /// session, never from a payload.
    pub fn request_context(
        &self,
        session: &t3_contracts::base_schemas::AuthSessionId,
        scopes: &[t3_contracts::auth::AuthEnvironmentScope],
    ) -> RequestContext {
        RequestContext {
            request_url: t3_cloud::Secret::new(self.origin.clone()),
            subject: session.0.clone(),
            scopes: scopes.to_vec(),
            forwarded_authority: false,
        }
    }
}

impl crate::services::DomainRuntime for CloudAdapter {
    fn name(&self) -> &'static str {
        "cloud"
    }
    /// Stops the connector this server owns.
    fn shutdown(&self) -> futures::future::BoxFuture<'_, crate::Result<()>> {
        Box::pin(async move {
            self.service
                .shutdown()
                .await
                .map_err(|e| crate::Error::Service(e.to_string()))
        })
    }
}

/// `POST /api/t3-connect/mint-credential` (`cloud/http.ts`): a signed relay proof, no session.
/// A mint for an untrusted key while device approval is on yields a pending request whose
/// credential cannot bootstrap a session until the device is approved.
pub fn routes(
    router: axum::Router<Arc<crate::services::ServiceRegistry>>,
) -> axum::Router<Arc<crate::services::ServiceRegistry>> {
    router.route(
        "/api/t3-connect/mint-credential",
        axum::routing::post(mint_credential),
    )
}

async fn mint_credential(
    axum::extract::State(services): axum::extract::State<Arc<crate::services::ServiceRegistry>>,
    body: axum::body::Bytes,
) -> axum::response::Response {
    use axum::{http::StatusCode, response::IntoResponse};
    let Some(cloud) = services.domain::<CloudAdapter>() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    // Effect's HttpApi answers a payload that does not decode with an empty 400.
    let Ok(request) =
        serde_json::from_slice::<t3_contracts::relay::RelayCloudMintCredentialRequest>(&body)
    else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let mut response = match cloud
        .service
        .mint_credential(t3_cloud::http::Sensitive::new(request))
        .await
    {
        Ok(minted) => axum::Json(minted.expose()).into_response(),
        Err(error) => http_error(error),
    };
    let headers = response.headers_mut();
    headers.insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    headers.insert(
        axum::http::header::PRAGMA,
        axum::http::HeaderValue::from_static("no-cache"),
    );
    response
}

/// `connectErrorCases` in `cloud/http.ts`: the relay's own rejections keep their meaning;
/// storage and process failures are logged and answered as internal errors.
fn http_error(error: CloudError) -> axum::response::Response {
    use axum::{http::StatusCode, response::IntoResponse};
    use t3_cloud::Rejection;
    use t3_contracts::environment_http as h;
    let message = error.to_string();
    let (status, body) = match &error {
        CloudError::NotLinked | CloudError::Configuration => (
            StatusCode::BAD_REQUEST,
            serde_json::to_value(h::EnvironmentHttpBadRequestError {
                tag: h::EnvironmentHttpBadRequestErrorTag::EnvironmentHttpBadRequestError,
                message,
            }),
        ),
        CloudError::Proof
        | CloudError::Remote {
            rejection: Rejection::Unauthorized,
            ..
        } => (
            StatusCode::UNAUTHORIZED,
            serde_json::to_value(h::EnvironmentHttpUnauthorizedError {
                tag: h::EnvironmentHttpUnauthorizedErrorTag::EnvironmentHttpUnauthorizedError,
                message,
            }),
        ),
        CloudError::Replay | CloudError::Superseded | CloudError::Identity => (
            StatusCode::CONFLICT,
            serde_json::to_value(h::EnvironmentHttpConflictError {
                tag: h::EnvironmentHttpConflictErrorTag::EnvironmentHttpConflictError,
                message,
            }),
        ),
        CloudError::Disabled
        | CloudError::Remote {
            rejection: Rejection::Forbidden,
            ..
        } => (
            StatusCode::FORBIDDEN,
            serde_json::to_value(h::EnvironmentHttpForbiddenError {
                tag: h::EnvironmentHttpForbiddenErrorTag::EnvironmentHttpForbiddenError,
                message,
            }),
        ),
        CloudError::Remote {
            rejection: Rejection::Rejected,
            ..
        } => (
            StatusCode::BAD_REQUEST,
            serde_json::to_value(h::EnvironmentHttpBadRequestError {
                tag: h::EnvironmentHttpBadRequestErrorTag::EnvironmentHttpBadRequestError,
                message,
            }),
        ),
        _ => {
            tracing::error!(%error, "connect request failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                serde_json::to_value(h::EnvironmentHttpInternalServerError {
                    tag:
                        h::EnvironmentHttpInternalServerErrorTag::EnvironmentHttpInternalServerError,
                    message,
                }),
            )
        }
    };
    (status, axum::Json(body.unwrap_or_default())).into_response()
}

/// Node's `process.platform` and `process.arch` names, which the pinned releases use.
fn platform() -> (&'static str, &'static str) {
    let platform = match std::env::consts::OS {
        "windows" => "win32",
        "macos" => "darwin",
        _ => "linux",
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        _ => "x64",
    };
    (platform, arch)
}
