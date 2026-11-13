#![allow(clippy::unwrap_used)]
mod support;
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use support::*;
use t3_cloud::{Result, Secret, Error, api::*, cloud::{CredentialIssuer, IssuedCredential}, http::Sensitive, managed::*, reachability::*, tunnel::ConnectorFacts, store::Secrets};
use t3_contracts::{auth::AuthEnvironmentScope as Scope, relay::*, environment_http::*};
use tokio::sync::{Mutex,broadcast,watch};

struct Runtime { changes:Mutex<Vec<bool>>, notices:broadcast::Sender<RuntimeNotice> }
#[async_trait]
impl ManagedEndpoint for Runtime {
    async fn apply(&self,config:Option<Sensitive<RelayManagedEndpointRuntimeConfig>>)->Result<EndpointStatus> {
        self.changes.lock().await.push(config.is_some());
        Ok(if config.is_some(){EndpointStatus::Running {provider_kind:RelayManagedEndpointProviderKind::CloudflareTunnel,pid:42,tunnel_id:Some("tunnel-1".into()),tunnel_name:None}}else{EndpointStatus::Disabled})
    }
    async fn inspect(&self)->Option<ConnectorFacts> {Some(ConnectorFacts {pid:Some(42),ready_connections:Some(2),hostname:Some("env.test".into()),http2:true,..Default::default()})}
    fn subscribe(&self)->broadcast::Receiver<RuntimeNotice>{self.notices.subscribe()}
}
struct Probe;
#[async_trait]
impl ReachabilityProbe for Probe {
    async fn endpoint(&self,host:&str)->EndpointProbe {assert_eq!(host,"env.test");EndpointProbe::Status {status:200,cloudflare_error:None}}
    async fn edge_reachable(&self)->bool{panic!("healthy endpoint needs no edge probe")}
    async fn exit_node(&self)->Option<t3_cloud::direct::ExitNode>{panic!("healthy endpoint needs no exit-node probe")}
}
struct Issuer;
#[async_trait]
impl CredentialIssuer for Issuer {async fn issue_cloud_pairing(&self,_:&str)->Result<IssuedCredential>{Ok(IssuedCredential {credential:Secret::new("marker-pairing"),expires_at_ms:NOW+120_000})}}
fn context(scopes:Vec<Scope>)->RequestContext {RequestContext {request_url:Secret::new("http://127.0.0.1:3773/api/connect/link-state"),subject:"desktop-bootstrap".into(),scopes,forwarded_authority:false}}
async fn service()->(Arc<CloudService>,Arc<Runtime>,Arc<MemorySecrets>,Arc<Relay>) {
    let (link,secrets,relay,time,_)=cloud().await;
    let runtime=Arc::new(Runtime {changes:Mutex::new(Vec::new()),notices:broadcast::channel(10).0});
    let service=CloudService::new(CloudServices {link,secrets:secrets.clone(),runtime:runtime.clone(),reachability:Arc::new(ConnectReachability::new(Arc::new(Probe),time)),issuer:Arc::new(Issuer),origin:serde_json::from_value(json!({"localHttpHost":"127.0.0.1","localHttpPort":3773})).unwrap(),connect_allowed:watch::channel(true).1}).unwrap();
    (service,runtime,secrets,relay)
}
#[tokio::test]
async fn cloud_routes_authorize_activate_report_and_unlink_without_disclosing_secrets() {
    let (service,runtime,secrets,relay)=service().await;
    assert!(service.link_state(&context(vec![])).await.is_err());
    let state=service.link_state(&context(vec![Scope::RelayRead])).await.unwrap();
    assert!(state.linked);assert_eq!(state.linkable_here,Some(Some(true)));assert_eq!(state.tunnel_hostname,Some(Some("env.test".into())));
    let input=serde_json::from_value(json!({"relayUrl":"https://relay.test","relayIssuer":"https://issuer.test","cloudUserId":"user-one","environmentCredential":"marker-rotated","cloudMintPublicKey":t3_cloud::crypto::EnvironmentKey::generate().unwrap().public_key,"endpointRuntime":support::runtime()})).unwrap();
    relay.add(|request|{assert_eq!(request.url.path(),"/v1/environments/env-one/tunnel/recovery");assert_eq!(request.headers["authorization"].expose(),"Bearer marker-rotated");Ok(response(200,json!({"status":"ready"})))}).await;
    let result=service.relay_config(&context(vec![Scope::RelayWrite]),Sensitive::new(input)).await.unwrap();
    assert_eq!(result.endpoint_runtime_status["pid"],42);
    assert_eq!(*runtime.changes.lock().await,vec![false,true]);
    assert!(secrets.get("cloud-endpoint-confirmed-origin").await.unwrap().is_some());
    assert!(matches!(service.check_reachability(&context(vec![Scope::RelayRead])).await.unwrap(),EnvironmentConnectReachability::Reachable {..}));
    secrets.set(t3_cloud::cloud::HOLD,b"true").await.unwrap();
    service.unlink(&context(vec![Scope::RelayWrite])).await.unwrap();
    assert!(secrets.get(t3_cloud::cloud::HOLD).await.unwrap().is_none());
    assert!(!service.state().await.unwrap().linked);
    let failure=HttpFailure::from(Error::Remote {rejection:t3_cloud::Rejection::Forbidden,retry_after_ms:None});assert_eq!(failure.status,403);
    let reply=reply(Ok(Secret::new("marker-private"))).unwrap();assert!(!format!("{reply:?}").contains("marker-private"));assert_eq!(reply.headers["cache-control"],"no-store");
    relay.done().await;
}
#[tokio::test]
async fn hosted_approval_requires_matching_active_host_and_keeps_peer_links_local() {
    let (service,_,secrets,_)=service().await;
    let redirect=service.approval_redirect("https://env.test","?state=marker-state&environment=attacker&client_id=marker-client",false,"https://app.test").await.unwrap();
    let url=url::Url::parse(redirect.expose()).unwrap();assert_eq!(url.path(),"/approve-agent");let query=url.query_pairs().collect::<Vec<_>>();assert_eq!(query.iter().filter(|(k,_)|k=="environment").count(),1);assert!(query.iter().any(|(k,v)|k=="environment" && v=="env-one"));assert!(!format!("{redirect:?}").contains("marker-state"));
    for (issuer,peer) in [("https://other.test",false),("https://env.test",true)] {assert_eq!(service.approval_redirect(issuer,"?state=x",peer,"https://app.test").await.unwrap().expose(),"/connect-agent?state=x");}
    secrets.set(t3_cloud::cloud::REVOKED,b"true").await.unwrap();assert_eq!(service.approval_redirect("https://env.test","",false,"https://app.test").await.unwrap().expose(),"/connect-agent");
}
#[tokio::test]
async fn publish_only_preferences_do_not_require_or_contact_a_relay() {
    let (service,_,secrets,relay)=service().await;
    service.link.unlink().await.unwrap();
    let state=service.preferences(&context(vec![Scope::RelayWrite]),EnvironmentCloudPreferencesRequest {publish_agent_activity:true,hold_webhooks_while_offline:None}).await.unwrap();
    assert!(state.publish_agent_activity);assert!(secrets.get(t3_cloud::cloud::HOLD).await.unwrap().is_none());relay.done().await;
}
#[test]
fn reachability_classification_prioritizes_origin_and_dns_over_a_failed_tcp_probe() {
    let mut facts=ReachabilityFacts {configured:true,connector:Some(ConnectorFacts {ready_connections:Some(0),..Default::default()}),endpoint:Some(EndpointProbe::Status {status:502,cloudflare_error:None}),edge_reachable:Some(false),exit_node:None};
    assert!(matches!(classify(&facts,"date".into()),EnvironmentConnectReachability::Unreachable {cause:EnvironmentConnectReachabilityCause::OriginUnreachable,..}));
    facts.connector.as_mut().unwrap().ready_connections=Some(1);facts.endpoint=Some(EndpointProbe::DnsFailed {exists_publicly:Some(true)});
    assert!(matches!(classify(&facts,"date".into()),EnvironmentConnectReachability::Unreachable {cause:EnvironmentConnectReachabilityCause::DnsFailed,..}));
    facts.endpoint=Some(EndpointProbe::Status {status:530,cloudflare_error:Some("1016".into())});
    assert!(matches!(classify(&facts,"date".into()),EnvironmentConnectReachability::Unreachable {cause:EnvironmentConnectReachabilityCause::UnexpectedResponse,..}));
}
