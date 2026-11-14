#![allow(clippy::unwrap_used)]
mod common;
use serde_json::{Value,json};
use t3_provider::{catalog, opencode_catalog::*};
#[test]
fn connected_v1_models_use_primary_agents_and_variants_and_mark_only_older_releases_of_each_provider_family() {
    let providers=json!({"connected":["anthropic","other"],"all":[
        {"id":"anthropic","name":"Anthropic","models":{
            "old":{"id":"old","name":"Older","family":"opus","release_date":"2026-01-01","variants":{"low":{},"high":{}}},
            "new":{"id":"new","name":"Newest","family":"opus","release_date":"2026-06-01","variants":{"low":{},"high":{}}},
            "deprecated":{"id":"deprecated","name":"Deprecated","family":"opus","release_date":"2027-01-01","status":"deprecated"}
        }},
        {"id":"other","name":"Other","models":{"same":{"id":"same","name":"Other provider","family":"opus","release_date":"2025-01-01","variants":{}}}},
        {"id":"disconnected","models":{"hidden":{"id":"hidden","name":"Hidden"}}}
    ]});
    let agents=vec![json!({"name":"subagent","mode":"subagent"}),json!({"name":"hidden","mode":"primary","hidden":true}),json!({"name":"plan","mode":"primary"}),json!({"name":"build","mode":"all"})];
    let models=v1_models(&providers,&agents,&[json!("custom")]).unwrap().unwrap();
    assert_eq!(models.len(),5);
    let get=|id:&str|models.iter().find(|model|model.slug.0==id).unwrap();
    assert_eq!(get("anthropic/old").is_legacy,Some(Some(true)));
    assert_eq!(get("anthropic/deprecated").is_legacy,Some(Some(true)));
    assert!(get("anthropic/new").is_legacy.is_none());
    assert!(get("other/same").is_legacy.is_none());
    let caps=serde_json::to_value(get("anthropic/new").capabilities.as_ref().unwrap()).unwrap();
    assert_eq!(caps["optionDescriptors"][0]["currentValue"],"high");
    assert_eq!(caps["optionDescriptors"][1]["currentValue"],"build");
    assert_eq!(caps["optionDescriptors"][1]["options"].as_array().unwrap().len(),2);
    assert_eq!(catalog::descriptors(get("custom").capabilities.as_ref().unwrap()).len(),2);
    assert!(v1_models(&json!({"connected":[],"all":[]}),&[],&[]).unwrap().is_none());
}
#[test]
fn v2_subprovider_labels_do_not_repeat_opencode_and_model_options_exclude_v1_agents() {
    let catalog:Vec<OpenCodeModel>=serde_json::from_value(json!([
        {"providerID":"opencode","id":"a","name":"Alpha","providerName":"OpenCode","variants":[{"id":"low"},{"id":"medium"}]},
        {"providerID":"openai","id":"b","name":"Beta","variants":[{"id":"high"}]}
    ])).unwrap();
    let models=v2_models(&catalog,&[]);
    assert!(models[0].sub_provider.is_none());
    assert_eq!(models[1].sub_provider.as_ref().unwrap().as_ref().unwrap().0,"openai");
    let caps=serde_json::to_value(models[0].capabilities.as_ref().unwrap()).unwrap();
    assert_eq!(caps["optionDescriptors"].as_array().unwrap().len(),1);
    assert_eq!(caps["optionDescriptors"][0]["currentValue"],"medium");
}
#[test]
fn inventories_keep_compact_deduplicate_commands_exclude_v1_skill_commands_and_preserve_v2_skill_identity() {
    let native:Vec<Value>=vec![json!({"name":"compact"}),json!({"name":"review","source":"skill","hints":[]}),json!({"name":" review ","description":" Review code ","hints":["file","scope"]}),json!({"name":"review","hints":[]})];
    let legacy=serde_json::to_value(commands(&native,false).unwrap()).unwrap();
    assert_eq!(legacy.as_array().unwrap().len(),2);
    assert_eq!(legacy[1]["name"],"review");
    assert_eq!(legacy[1]["input"]["hint"],"file scope");
    let skills=skills(&[json!({"id":"stable-id","name":"Readable name","path":"/explicit/skill","description":" Description "})],true).unwrap();
    let skill=serde_json::to_value(&skills[0]).unwrap();
    assert_eq!(skill["name"],"stable-id");
    assert_eq!(skill["displayName"],"Readable name");
    assert_eq!(skill["shortDescription"],"Description");
}

use async_trait::async_trait;
use std::{collections::VecDeque, sync::Arc};
use t3_provider::{environment::Environment, opencode_loader::*, sensitive::{ServiceError, ServiceResult}};
use tokio::sync::Mutex;
struct Source(Mutex<VecDeque<ServiceResult<Vec<OpenCodeModel>>>>);
#[async_trait]
impl OpenCodeModels for Source {
    async fn list(&self) -> ServiceResult<Vec<OpenCodeModel>> {
        match self.0.lock().await.pop_front() {
            Some(value) => value,
            None => std::future::pending().await,
        }
    }
}
fn catalog(slugs: &[&str]) -> Vec<OpenCodeModel> {
    slugs.iter().map(|slug| serde_json::from_value(json!({"providerID":"openai","id":slug,"name":slug,"variants":[]})).unwrap()).collect()
}
#[tokio::test(start_paused=true)]
async fn cold_v2_catalogs_wait_for_two_agreeing_reads_and_warm_catalogs_accept_new_progress() {
    let partial=catalog(&["one"]); let full=catalog(&["one","two"]);
    let source=Arc::new(Source(Mutex::new(vec![Ok(partial),Ok(full.clone()),Ok(full.clone())].into())));
    let loader=OpenCodeModelLoader::new(source.clone(),&Environment::new());
    assert_eq!(loader.load().await.unwrap(),full);
    let newest=catalog(&["one","two","three"]);
    source.0.lock().await.extend([Ok(newest.clone())]);
    assert_eq!(loader.load().await.unwrap(),newest);
    source.0.lock().await.extend([Ok(vec![])]);
    assert_eq!(loader.load().await.unwrap(),newest);
    source.0.lock().await.extend([Err(ServiceError::Network)]);
    assert_eq!(loader.load().await.unwrap_err(),ServiceError::Network);
}
#[tokio::test(start_paused=true)]
async fn strict_directory_catalogs_never_publish_a_partial_inventory_as_settled() {
    let source=Arc::new(Source(Mutex::new(vec![Ok(catalog(&["one"])),Err(ServiceError::Network)].into())));
    let loader=OpenCodeModelLoader::new(source.clone(),&Environment::new());
    assert!(loader.read_settled().await.is_none());
    source.0.lock().await.extend([Ok(catalog(&["one"])),Ok(catalog(&["one"]))]);
    assert_eq!(loader.read_settled().await.unwrap().len(),1);
}

use t3_provider::{opencode_http::*, transport::*, sensitive::Sensitive};
struct Http {
    replies: Mutex<std::collections::BTreeMap<String,RawHttpResponse>>,
    calls: Mutex<Vec<HttpRequest>>,
    slow_names: bool,
}
#[async_trait]
impl HttpTransport for Http {
    async fn send(&self,_:HttpRequest)->ServiceResult<HttpResponse> { panic!("OpenCode requires response identity metadata") }
    async fn raw(&self,request:HttpRequest)->ServiceResult<RawHttpResponse> {
        assert_eq!(request.method,"GET");
        let path=url::Url::parse(&request.url).unwrap().path().to_owned();
        self.calls.lock().await.push(request);
        if self.slow_names && path.ends_with("/api/provider") { return std::future::pending().await; }
        self.replies.lock().await.get(&path).cloned().ok_or(ServiceError::Network)
    }
}
fn raw(status:u16,body:Value)->RawHttpResponse { RawHttpResponse {status,content_type:Some("application/json; charset=utf-8".into()),body:Sensitive::new(serde_json::to_vec(&body).unwrap())} }
#[tokio::test]
async fn opencode_http_requires_version_identity_and_preserves_basic_passwords_and_directory_queries() {
    use base64::{Engine,engine::general_purpose::STANDARD};
    let http=Arc::new(Http {replies:Mutex::new(std::collections::BTreeMap::from([
        ("/nested/api/info".into(),raw(404,json!({}))),
        ("/nested/global/health".into(),raw(200,json!({"healthy":true,"version":"1.14.19"}))),
        ("/nested/provider".into(),raw(200,json!({"all":[],"connected":[]}))),
    ])),calls:Mutex::new(vec![]),slow_names:false});
    let client=OpenCodeHttp::new(http.clone(),"https://server.invalid/nested",Some(Sensitive::new("sëcret-päss".into()))).unwrap();
    assert_eq!(client.probe(&Environment::new()).await.unwrap().generation,1);
    let inventory=client.inventory_v1("C:/workspace é",std::time::Duration::from_secs(5)).await.unwrap();
    assert!(inventory.agents.is_empty());
    assert!(inventory.skills.is_empty());
    let calls=http.calls.lock().await;
    assert_eq!(calls[0].headers["authorization"].expose(),&format!("Basic {}",STANDARD.encode("opencode:sëcret-päss")));
    let provider=calls.iter().find(|request|request.url.contains("/provider?")).unwrap();
    assert_eq!(url::Url::parse(&provider.url).unwrap().query_pairs().find(|(key,_)|key=="directory").unwrap().1,"C:/workspace é");
    assert!(!format!("{client:?} {calls:?}").contains("sëcret-päss"));
    drop(calls);
    http.replies.lock().await.insert("/nested/api/info".into(),RawHttpResponse {status:401,content_type:None,body:Sensitive::new(b"never include this secret".to_vec())});
    let error=client.probe(&Environment::new()).await.unwrap_err();
    assert_eq!(error,OpenCodeError::Authentication);
    assert!(!format!("{error:?} {error}").contains("never include"));
}
#[tokio::test(start_paused=true)]
async fn optional_v2_provider_labels_cannot_stall_models_but_directory_catalogs_require_them() {
    let http=Arc::new(Http {replies:Mutex::new(std::collections::BTreeMap::from([
        ("/api/model".into(),raw(200,json!({"data":[{"providerID":"openai","id":"model","name":"Model","variants":[],"time":{"released":1000},"status":"active"}]}))),
    ])),calls:Mutex::new(vec![]),slow_names:true});
    let connection=Arc::new(OpenCodeHttp::new(http.clone(),"https://server.invalid",Some(Sensitive::new("secret".into()))).unwrap());
    let mut source=HttpModels {connection,directory:"/workspace".into(),strict_names:false};
    let models=source.list().await.unwrap();
    assert_eq!(models.len(),1);
    assert!(models[0].provider_name.is_none());
    let calls=http.calls.lock().await;
    assert!(calls.iter().all(|request|url::Url::parse(&request.url).unwrap().query_pairs().any(|(key,value)|key=="location[directory]"&&value=="/workspace")));
    drop(calls);
    source.strict_names=true;
    assert!(source.list().await.is_err());
}

#[tokio::test]
async fn owned_opencode_servers_use_private_environment_share_borrowers_and_stop_only_their_captured_child() {
    use common::Fixture;
    use t3_provider::opencode_runtime::*;
    let fixture=Fixture::new(json!({"protocol":"opencode","generation":2,"version":"2.0.28"}));
    let environment_log=fixture.directory.path().join("environment.json");
    fixture.write(json!({"protocol":"opencode","generation":2,"version":"2.0.28","environmentLog":environment_log}));
    let mut environment=fixture.environment.clone();
    environment.insert("OPENCODE_SERVER_PASSWORD".into(),"never-log-inherited-password".into());
    let launch=OpenCodeLaunch {binary:fixture.binary(),directory:fixture.directory.path().into(),environment,server_url:String::new(),server_password:None};
    assert!(!format!("{launch:?}").contains("never-log"));
    let runtime=OpenCodeRuntime::new(launch,Arc::new(ReqwestTransport::new().unwrap()));
    let first=runtime.acquire().await.unwrap();
    let second=runtime.acquire().await.unwrap();
    assert!(!first.external);
    assert!(Arc::ptr_eq(&first.http,&second.http));
    assert_eq!(runtime.forget().await.unwrap_err(),ServiceError::Busy);
    let exit=first.process_exit.clone().unwrap();
    let environment:Value=serde_json::from_slice(&std::fs::read(environment_log).unwrap()).unwrap();
    assert_eq!(environment,json!({"hasV2Password":true,"hasV1Password":false,"emptyConfig":true}));
    let controls:Vec<_>=fixture.calls().into_iter().filter(|call|call.first().is_some_and(|argument|argument=="serve")).collect();
    assert_eq!(controls.len(),1);
    assert!(controls[0].iter().all(|argument|!argument.contains("password")));
    drop(first);drop(second);
    runtime.forget().await.unwrap();
    assert!(exit.is_cancelled());
    let restarted=runtime.acquire().await.unwrap();
    assert!(!restarted.process_exit.as_ref().unwrap().is_cancelled());
    runtime.close().await;
    assert!(restarted.process_exit.as_ref().unwrap().is_cancelled());
    assert!(!fixture.calls().iter().flatten().any(|argument|argument.starts_with("session/")||argument=="prompt"||argument=="run"));
}
