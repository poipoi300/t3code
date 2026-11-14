use crate::{
    antigravity_callback::{self, Authorization},
    antigravity_profile::AuthConfig,
    antigravity_provider::AntigravityProvider,
    antigravity_runtime::AntigravityRuntimeFactory,
    auth::{AuthBackend, AuthContext, AuthReply},
    sensitive::{Sensitive, ServiceError, ServiceResult},
};
use async_trait::async_trait;
use std::{sync::Arc, time::Duration};
use t3_contracts::provider_setup::{ProviderAuthInteraction, ProviderAuthMethod};
use tokio::sync::{mpsc, watch};
#[async_trait]
pub trait CallbackForwarder: Send + Sync {
    async fn forward(&self, callback: &Sensitive<url::Url>) -> ServiceResult<()>;
}
pub struct LoopbackForwarder;
#[async_trait]
impl CallbackForwarder for LoopbackForwarder {
    async fn forward(&self, callback: &Sensitive<url::Url>) -> ServiceResult<()> {
        antigravity_callback::forward(callback).await
    }
}
pub struct AntigravityAuth {
    pub factory: Arc<dyn AntigravityRuntimeFactory>,
    pub provider: Arc<AntigravityProvider>,
    pub config: AuthConfig,
    pub forwarder: Arc<dyn CallbackForwarder>,
}
#[async_trait]
impl AuthBackend for AntigravityAuth {
    fn default_method_id(&self) -> Option<&str> {
        Some(&self.config.method)
    }
    fn check_start(&self) -> ServiceResult<()> {
        if self.config.issue().is_some() {
            Err(ServiceError::Identity)
        } else {
            Ok(())
        }
    }
    fn is_logout_prompt(&self, text: &str, has_attachments: bool) -> bool {
        !has_attachments && t3_contracts::wire::trim_js(text) == "/logout"
    }
    async fn methods(&self) -> ServiceResult<Vec<ProviderAuthMethod>> {
        Ok(vec![serde_json::from_value(serde_json::json!({"id":self.config.method,"name":self.config.label(),"description":null,"type":"agent"})).map_err(crate::diagnostics::mapped(concat!(module_path!(), "::error"), ServiceError::Response))?])
    }
    async fn authenticate(&self, _method: &str, mut context: AuthContext) -> ServiceResult<()> {
        let _permit = self.provider.setup_guard().await;
        let (urls, mut authorization) = mpsc::channel(8);
        let runtime = self.factory.create(Some(urls)).await?;
        let (phase, mut initialized) = watch::channel(false);
        let work = async {
            runtime.initialize().await?;
            phase.send_replace(true);
            let started = runtime.start().await?;
            let catalog = runtime.catalog(&started).await?;
            Ok::<_, ServiceError>((started, catalog))
        };
        tokio::pin!(work);
        let mut pending: Option<Authorization> = None;
        let mut callback_sent = false;
        let mut hint_sent = false;
        let hint = tokio::time::sleep(Duration::from_secs(90));
        tokio::pin!(hint);
        loop {
            tokio::select! {
                result=&mut work=>{runtime.close().await;let (started,catalog)=result?;context.verifying().await;let provider=self.provider.clone();return context.commit(async move{provider.signed_in().await?;provider.catalog_started(started,catalog.expose(),None,None).await}).await;}
                _=&mut hint,if pending.is_none()&&!hint_sent=>{hint_sent=true;context.message(if *initialized.borrow(){"Antigravity has not reached Google after 30 seconds. Check this machine's network connection. Broken IPv6 is a common cause."}else{"Antigravity has not started after 90 seconds. Its runtime may be stuck starting on this machine."}).await;}
                changed=initialized.changed()=>{if changed.is_ok(){hint_sent=false;hint.as_mut().reset(tokio::time::Instant::now()+Duration::from_secs(30));}}
                Some(url)=authorization.recv()=>{if !self.config.uses_browser(){return Err(ServiceError::Identity);}let request=antigravity_callback::authorization(url.expose())?;if let Some(previous)=&pending{if previous.url!=request.url{return Err(ServiceError::Callback);}continue;}context.interaction(ProviderAuthInteraction::Browser{id:context.flow_id.clone().into(),url:request.url.expose().clone().into(),requires_consent:false,accepts_callback:Some(true)},false,true).await?;pending=Some(request);}
                delivery=context.receive()=>{let delivery=delivery?;let response=match(&pending,&delivery.reply){(Some(pending),AuthReply::Callback(callback))if !callback_sent=>antigravity_callback::validate(pending,callback.expose()),_=>Err(ServiceError::Interaction)};let callback=match response{Ok(callback)=>callback,Err(error)=>{delivery.finish(Err(error));continue;}};callback_sent=true;context.verifying().await;let result=self.forwarder.forward(&callback).await;delivery.finish(result);result?;}
            }
        }
    }
    async fn logout(&self) -> ServiceResult<Option<String>> {
        let _permit = self.provider.setup_guard().await;
        tokio::time::timeout(Duration::from_secs(90), async {
            let runtime = self.factory.create(None).await?;
            let result = runtime.logout().await;
            runtime.close().await;
            result?;
            self.provider.signed_out().await?;
            Ok(Some("Signed out of Google.".into()))
        })
        .await
        .map_err(crate::diagnostics::mapped(
            concat!(module_path!(), "::error"),
            ServiceError::Expired,
        ))?
    }
}
