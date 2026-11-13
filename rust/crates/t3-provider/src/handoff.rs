use crate::{
    auth::{AuthController, AuthSubscription, CredentialBinding, SessionStopper},
    chatgpt::{
        CallbackFactory, ChatGptAuth, Entropy, IdentityVerifier, ProfileStore, SavedProfiles,
    },
    sensitive::{Sensitive, ServiceResult},
    transport::{HttpTransport, ProviderClock},
};
use async_trait::async_trait;
use std::{sync::Arc, time::Duration};
use t3_contracts::{provider_instance::ProviderInstanceId, provider_setup::*};
use tokio::sync::Mutex;
struct MemoryProfiles(Mutex<SavedProfiles>);
#[async_trait]
impl ProfileStore for MemoryProfiles {
    async fn load(&self) -> ServiceResult<SavedProfiles> {
        Ok(self.0.lock().await.clone())
    }
    async fn save(&self, profiles: &SavedProfiles) -> ServiceResult<()> {
        *self.0.lock().await = profiles.clone();
        Ok(())
    }
}
struct NoSessions;
#[async_trait]
impl SessionStopper for NoSessions {
    async fn stop(&self, _: &ProviderInstanceId, _: &CredentialBinding) -> ServiceResult<()> {
        Ok(())
    }
}
pub struct HandoffService {
    pub http: Arc<dyn HttpTransport>,
    pub verifier: Arc<dyn IdentityVerifier>,
    pub entropy: Arc<dyn Entropy>,
    pub clock: Arc<dyn ProviderClock>,
    pub callbacks: Arc<dyn CallbackFactory>,
}
impl HandoffService {
    pub async fn subscribe(
        &self,
        input: ChatGptHandoffInput,
        owner: &str,
    ) -> ServiceResult<HandoffSubscription> {
        let mut saved = SavedProfiles::default();
        if let Some(profile) = input.profile {
            saved.registrations.last_client_id = Some(profile.client_id.clone());
            saved.registrations.profiles.push(profile);
        }
        let auth = ChatGptAuth::new(
            Arc::new(MemoryProfiles(Mutex::new(saved))),
            self.http.clone(),
            self.verifier.clone(),
            self.entropy.clone(),
            self.clock.clone(),
            self.callbacks.clone(),
            input.environment_id.0,
        );
        let controller = AuthController::new(
            input.instance_id,
            CredentialBinding {
                key: uuid::Uuid::new_v4().to_string(),
                owner: ProviderCredentialOwner::T3,
            },
            auth.clone(),
            Arc::new(NoSessions),
            Duration::from_secs(300),
        )
        .await;
        let states = controller.subscribe(owner.to_owned());
        controller
            .start(
                owner,
                Some("chatgpt"),
                Some(input.return_url),
                Some(ProviderAuthCallbackMode::Server),
                self.clock.now_ms(),
            )
            .await?;
        Ok(HandoffSubscription {
            controller,
            auth,
            states,
            finished: false,
        })
    }
}
pub struct HandoffSubscription {
    controller: Arc<AuthController>,
    auth: Arc<ChatGptAuth>,
    states: AuthSubscription,
    finished: bool,
}
impl HandoffSubscription {
    pub async fn next(&mut self) -> ServiceResult<Option<Sensitive<ChatGptHandoffState>>> {
        if self.finished {
            return Ok(None);
        }
        let Some(state) = self.states.next().await else {
            return Ok(None);
        };
        self.finished = matches!(
            state.expose().phase,
            ProviderAuthPhase::Succeeded | ProviderAuthPhase::Failed | ProviderAuthPhase::Cancelled
        );
        if state.expose().phase == ProviderAuthPhase::Succeeded {
            Ok(Some(Sensitive::new(ChatGptHandoffState::Finished {
                profile: self.auth.export_profile().await?.into_inner(),
            })))
        } else {
            Ok(Some(Sensitive::new(ChatGptHandoffState::Auth {
                state: state.into_inner(),
            })))
        }
    }
}
impl Drop for HandoffSubscription {
    fn drop(&mut self) {
        self.controller.close();
    }
}
