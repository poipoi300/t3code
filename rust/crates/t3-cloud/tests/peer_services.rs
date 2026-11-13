#![allow(clippy::unwrap_used)]
mod support;
use async_trait::async_trait;
use serde_json::{Value,json};
use std::{collections::{BTreeMap,VecDeque},sync::{Arc,atomic::Ordering}};
use support::*;
use t3_cloud::{Error,Result,Secret,api::RequestContext,http::Sensitive,link_requests::*,remote::*,peer::{PeerLinks,CallLimits}};
use t3_contracts::{base_schemas::*,orchestration_v2::{InternalCommandLinkRequestRecord as Card,InternalCommandDelegatedTaskRemoteRequest as TaskRequest,InternalCommandDelegatedTaskRemoteComplete as Completion,InternalCommandDelegatedTaskRemoteCompleteStoppedThere as Stopped},provider_policy::{RuntimeMode,ProviderInteractionMode}};
use tokio::sync::{Mutex,broadcast};

async fn peers()->(Arc<PeerLinks>,tempfile::TempDir) {
    let directory=tempfile::tempdir().unwrap();let path=directory.path().join("state.sqlite");let db=tokio::task::spawn_blocking(move||t3_persistence::Database::open(path)).await.unwrap().unwrap();
    db.writer(|c|{c.execute("INSERT INTO peer_environment_links(environment_id,secret_name,label,urls_json,access,linked_at,expires_at,last_reached_at,last_error,fenced) VALUES ('remote','peer-secret','Box','[\"https://peer.test\"]','read-only','2027-01-15T08:00:00.000Z','2027-01-16T08:00:00.000Z',NULL,NULL,1)",[])?;Ok(())}).await.unwrap();
    let peers=Arc::new(PeerLinks::new(db,Arc::new(MemorySecrets::default()),Arc::new(Relay::default()),Arc::new(Time::default()),descriptor("local")));peers.enabled.store(true,Ordering::Relaxed);(peers,directory)
}
fn caller()->LinkCaller {LinkCaller {thread_id:"parent".to_owned().into(),provider_instance_id:"codex".to_owned().into(),namespace:"session-one".into(),linked_origin:false}}
fn context()->RequestContext {RequestContext {request_url:Secret::new("http://127.0.0.1"),subject:"test".into(),scopes:vec![t3_contracts::auth::AuthEnvironmentScope::AccessWrite],forwarded_authority:false}}
fn parent()->RemoteParent {RemoteParent {thread_id:"parent".to_owned().into(),project_id:"project".to_owned().into(),provider_instance_id:"codex".to_owned().into(),title:"Parent".into(),model_selection:serde_json::from_value(json!({"instanceId":"codex","model":"requested"})).unwrap(),limits:CallLimits {runtime_mode:RuntimeMode::ApprovalRequired,interaction_mode:ProviderInteractionMode::Plan},active:Some(ActiveParent {run_id:"parent-run".to_owned().into(),node_id:"parent-node".to_owned().into(),provider_instance_id:"codex".to_owned().into()}),repository_key:Some("repo".into())}}
struct Store {cards:Mutex<BTreeMap<String,Card>>,requests:Mutex<Vec<TaskRequest>>,task:Mutex<Option<RemoteTask>>,completions:Mutex<Vec<Completion>>,events:broadcast::Sender<TaskKey>}
impl Store {fn new()->Arc<Self>{Arc::new(Self {cards:Mutex::new(BTreeMap::new()),requests:Mutex::new(Vec::new()),task:Mutex::new(None),completions:Mutex::new(Vec::new()),events:broadcast::channel(16).0})}}
#[async_trait]
impl LinkRequestStore for Store {
    async fn card(&self,_:&ThreadId,id:&TurnItemId)->Result<Option<Card>>{Ok(self.cards.lock().await.get(&id.0).cloned())}
    async fn active_parent(&self,_:&ThreadId)->Result<Option<ActiveParent>>{Ok(parent().active)}
    async fn record(&self,card:Card)->Result<()> {let mut cards=self.cards.lock().await;if cards.get(&card.turn_item_id.0).is_none_or(|old|old.link_status==t3_contracts::orchestration_v2::OrchestrationV2LinkRequestStatus::Pending){cards.insert(card.turn_item_id.0.clone(),card);}Ok(())}
}
#[async_trait]
impl RemoteTaskStore for Store {
    async fn parent(&self,_:&ThreadId)->Result<RemoteParent>{Ok(parent())}
    async fn task(&self,_:&TaskKey)->Result<Option<RemoteTask>>{Ok(self.task.lock().await.clone())}
    async fn outstanding(&self)->Result<Vec<TaskKey>>{Ok(self.task.lock().await.as_ref().map(|task|vec![task.key.clone()]).unwrap_or_default())}
    fn subscribe(&self)->broadcast::Receiver<TaskKey>{self.events.subscribe()}
    async fn request(&self,request:TaskRequest)->Result<NodeId>{let id:NodeId="task-one".to_owned().into();let mut requests=self.requests.lock().await;if requests.iter().any(|old|old.command_id==request.command_id){return Ok(id);}
        *self.task.lock().await=Some(RemoteTask {key:(request.parent_thread_id.clone(),id.clone()),remote:request.remote_child.clone(),status:"running".into(),result:None,model_selection:request.model_selection.clone(),limits:parent().limits});requests.push(request);Ok(id)}
    async fn complete(&self,command:Completion)->Result<()> {let mut task=self.task.lock().await;let task=task.as_mut().unwrap();if task.result.is_none(){task.result=Some(command.result.clone());task.status=serde_json::to_value(&command.status).unwrap().as_str().unwrap().into();if command.stopped_there==Some(Some(Stopped::RanOut)){task.remote.stopped_there=Some(Some(true));}}
        if command.stopped_there==Some(Some(Stopped::Stopped)){task.remote.stopped_there=Some(Some(true));}self.completions.lock().await.push(command);let _=self.events.send(task.key.clone());Ok(())}
    async fn shared_result_run(&self,_:&TaskKey)->Result<bool>{Ok(false)}
    async fn finished_owner(&self,_:&ThreadId,_:&EnvironmentId,_:&ThreadId)->Result<Option<RemoteTask>>{Ok(self.task.lock().await.clone().filter(|task|task.result.is_some()))}
}
type Step=Box<dyn FnOnce(RemoteCall)->Result<Value>+Send>;
struct Calls(Mutex<VecDeque<Step>>);
impl Calls {fn new()->Arc<Self>{Arc::new(Self(Mutex::new(VecDeque::new())))}async fn add(&self,step:impl FnOnce(RemoteCall)->Result<Value>+Send+'static){self.0.lock().await.push_back(Box::new(step));}}
#[async_trait]
impl RemoteCalls for Calls {async fn call(&self,call:RemoteCall)->Result<Sensitive<Value>>{self.0.lock().await.pop_front().expect("unexpected remote call")(call).map(Sensitive::new)}}
async fn delegate(store:Arc<Store>,calls:Arc<Calls>,peers:Arc<PeerLinks>)->(Arc<RemoteDelegation>,TaskKey) {
    let service=RemoteDelegation::new("local".to_owned().into(),store,calls.clone(),peers);
    calls.add(|call|{assert_eq!(call.tool,"t3_thread_launch");assert_eq!(call.arguments["message"],"Act as the research sub-agent for this task.\n\nInvestigate");assert_eq!(call.arguments["clientRequestId"],"delegate:parent:retry");assert_eq!(call.limits.runtime_mode,RuntimeMode::ApprovalRequired);assert!(call.arguments.get("runtimeMode").is_none());Ok(json!({"threadId":"remote-thread","runId":"remote-run","modelSelection":{"instanceId":"codex","model":"requested"}}))}).await;
    let id=service.delegate(&caller(),serde_json::from_value(json!({"task":"Investigate","role":"research","clientRequestId":"retry","target":{"environmentId":"remote","projectId":"there"}})).unwrap()).await.unwrap();(service,("parent".to_owned().into(),id))
}
#[tokio::test]
async fn link_cards_are_namespaced_idempotent_and_answers_never_record_pairing_codes() {
    let (peers,_directory)=peers().await;let store=Store::new();let service=PeerLinkRequests::new(peers,store.clone(),Arc::new(Time::default()));
    let input=||serde_json::from_value(json!({"clientRequestId":"same","hint":"Box","reason":"Build there"})).unwrap();
    let owner=caller();let (a,b)=tokio::join!(service.request(&owner,input()),service.request(&owner,input()));assert_eq!(a.unwrap(),b.unwrap());assert_eq!(store.cards.lock().await.len(),1);
    let id=card_id("parent","session-one","same");let answer=||Sensitive::new(serde_json::from_value(json!({"threadId":"parent","turnItemId":id,"answer":{"type":"use-existing","environmentId":"remote"}})).unwrap());
    service.answer(&context(),answer()).await.unwrap();assert!(matches!(service.answer(&context(),answer()).await,Err(Error::Superseded)));
    let card=store.cards.lock().await[&id.0].clone();let value=serde_json::to_value(card).unwrap();assert_eq!(value["linkStatus"],"linked");assert_eq!(value["wake"]["notification"]["outcome"],"completed");assert!(!value.to_string().contains("pairingCode"));
    let second=service.request(&caller(),serde_json::from_value(json!({"clientRequestId":"legacy","url":"https://untrusted.test"})).unwrap()).await.unwrap();let id:TurnItemId=serde_json::from_value(serde_json::to_value(second).unwrap()["turnItemId"].clone()).unwrap();store.cards.lock().await.get_mut(&id.0).unwrap().environment_id=Some(Some("remote".to_owned().into()));
    service.answer(&context(),Sensitive::new(serde_json::from_value(json!({"threadId":"parent","turnItemId":id,"answer":{"type":"link","environmentId":"remote","urls":["https://untrusted.test"],"access":"read-only","pairingCode":"marker-one-use-code"}})).unwrap())).await.unwrap();
    let value=serde_json::to_value(&*store.cards.lock().await).unwrap();assert!(!value.to_string().contains("marker-one-use-code"));assert_eq!(value[&id.0]["linkStatus"],"failed");
    assert_ne!(card_id("parent","a","key"),card_id("parent","b","key"));
}
#[tokio::test]
async fn remote_follower_reads_whole_result_for_its_run_and_records_executed_model() {
    let (peers,_directory)=peers().await;let store=Store::new();let calls=Calls::new();let (service,key)=delegate(store.clone(),calls.clone(),peers).await;
    calls.add(|call|{assert_eq!(call.tool,"t3_thread_wait");assert_eq!(call.arguments["runId"],"remote-run");assert_eq!(call.arguments["untilDelegatedWorkSettled"],true);Ok(json!({"threadId":"remote-thread","runId":"remote-run","status":"completed","timedOut":false}))}).await;
    calls.add(|call|{assert_eq!(call.tool,"t3_thread_read");Ok(json!({"items":[{"type":"assistant_message","runId":"other","text":"wrong"},{"type":"assistant_message","runId":"remote-run","itemId":"answer","text":"first ","textTruncated":true,"nextTextOffset":6}],"recentRuns":[{"runId":"remote-run","model":"requested","executedModel":"actual"}],"hasMore":false,"nextPosition":null}))}).await;
    calls.add(|call|{assert_eq!(call.arguments["textOffset"],6);Ok(json!({"items":[{"text":"rest","textTruncated":false}]}))}).await;
    assert!(service.follow_once(&key).await.unwrap());let result=store.completions.lock().await[0].result.clone();assert_eq!(result,"Requested model requested; the provider reported running actual.\n\nfirst rest");assert!(calls.0.lock().await.is_empty());
}
#[tokio::test]
async fn cancelled_remote_tasks_keep_stop_debt_until_the_peer_answers() {
    let (peers,_directory)=peers().await;let store=Store::new();let calls=Calls::new();let (service,key)=delegate(store.clone(),calls.clone(),peers).await;
    calls.add(|call|{assert_eq!(call.tool,"t3_thread_interrupt");Err(t3_cloud::http::unavailable())}).await;
    let cancelled=service.cancel(&caller(),&key,Some("Stop now"),Some("cancel-retry")).await.unwrap();assert_eq!(serde_json::to_value(cancelled).unwrap()["status"],"cancel_requested");assert_ne!(store.task.lock().await.as_ref().unwrap().remote.stopped_there,Some(Some(true)));
    calls.add(|call|{assert_eq!(call.tool,"t3_thread_interrupt");assert_eq!(call.arguments["stop"],true);assert_eq!(call.namespace,"remote-task:parent");Ok(json!({}))}).await;
    assert!(service.follow_once(&key).await.unwrap());let task=store.task.lock().await;let task=task.as_ref().unwrap();assert_eq!(task.status,"cancelled");assert_eq!(task.remote.stopped_there,Some(Some(true)));
}
#[tokio::test]
async fn restart_scans_outstanding_remote_tasks_and_joins_followers_on_shutdown() {
    let (peers,_directory)=peers().await;let store=Store::new();let calls=Calls::new();let (service,key)=delegate(store.clone(),calls.clone(),peers).await;
    calls.add(|_|Err(Error::PeerFailure {code:t3_contracts::orchestrator_mcp::OrchestratorMcpFailureCode::CapabilityDenied})).await;
    let mut events=store.events.subscribe();let runner=service.start();assert_eq!(events.recv().await.unwrap(),key);runner.shutdown().await.unwrap();assert_eq!(store.task.lock().await.as_ref().unwrap().status,"failed");
}
