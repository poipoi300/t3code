use crate::{
    Capability, ClientAccess, ClientCaller, InvocationScope,
    transport::{AuthenticatedClient, ClientAuthenticator, bearer},
};
use async_trait::async_trait;
use http::{HeaderMap, Uri};
use std::collections::BTreeSet;
use t3_auth::EnvironmentAuth;
use t3_contracts::{auth::AuthEnvironmentScope as Scope, base_schemas::EnvironmentId};

pub struct OAuthAuthenticator {
    auth: EnvironmentAuth,
    resource: reqwest::Url,
    environment_id: EnvironmentId,
}

impl OAuthAuthenticator {
    pub fn new(auth: EnvironmentAuth, resource: &str) -> crate::Result<Self> {
        let resource = reqwest::Url::parse(resource)
            .map_err(|_| crate::McpError::InvalidInput("Invalid MCP resource URL".into()))?;
        if !matches!(resource.scheme(), "http" | "https")
            || resource.host_str().is_none()
            || resource.path() != "/mcp"
            || resource.query().is_some()
            || resource.fragment().is_some()
            || !resource.username().is_empty()
            || resource.password().is_some()
        {
            return Err(crate::McpError::InvalidInput(
                "MCP resource must be an http(s) /mcp URL without credentials, query or fragment"
                    .into(),
            ));
        }
        let environment_id = EnvironmentId(auth.config().environment_id.clone());
        Ok(Self {
            auth,
            resource,
            environment_id,
        })
    }
}

#[async_trait]
impl ClientAuthenticator for OAuthAuthenticator {
    async fn authenticate(&self, headers: &HeaderMap, uri: &Uri) -> Option<AuthenticatedClient> {
        let token = bearer(headers)?;
        let host = headers.get("host")?.to_str().ok()?;
        let scheme = if headers
            .get("x-forwarded-proto")
            .is_some_and(|v| v == "https")
        {
            "https"
        } else {
            uri.scheme_str().unwrap_or("http")
        };
        let resource = reqwest::Url::parse(&format!("{scheme}://{host}{}", uri.path())).ok()?;
        if resource != self.resource {
            return None;
        }
        let session = self
            .auth
            .sessions
            .verify_mcp(token, self.resource.as_str(), &[])
            .await
            .ok()?;
        let access = if !session.scopes.contains(&Scope::OrchestrationOperate) {
            ClientAccess::ReadOnly
        } else {
            match session.runtime_mode_ceiling {
                Some(t3_contracts::provider_policy::RuntimeMode::FullAccess) => {
                    ClientAccess::FullAccess
                }
                Some(t3_contracts::provider_policy::RuntimeMode::Auto) => ClientAccess::Auto,
                Some(t3_contracts::provider_policy::RuntimeMode::AutoAcceptEdits) => {
                    ClientAccess::AutoAcceptEdits
                }
                _ => ClientAccess::ApprovalRequired,
            }
        };
        Some(AuthenticatedClient {
            scopes: session.scopes,
            scope: InvocationScope {
                environment_id: self.environment_id.clone(),
                request_namespace: format!("client:{}", session.session_id.0),
                issued_at: self.auth.now_ms(),
                capabilities: BTreeSet::from([
                    Capability::Orchestration,
                    Capability::Worktree,
                    Capability::PullRequests,
                ]),
                thread: None,
                client: Some(ClientCaller {
                    session_id: session.session_id.0,
                    label: session
                        .client
                        .label
                        .map(|v| v.0)
                        .unwrap_or_else(|| "MCP client".into()),
                    access,
                    linked: session.peer_link,
                    narrowed_to: None,
                    sender_thread: None,
                }),
                linked_environments: None,
            },
        })
    }
}
