//! Live environment sessions and local platform state for settings pages.
use crate::{
    ClientError,
    composition::ClientRuntime,
    platform::{Platform, StorageArea, StorageKey, StorageWrite},
    rpc::SessionEndpoint,
    types::EnvironmentId,
};
use std::rc::Rc;

type ResolveSession = dyn Fn(&EnvironmentId) -> Option<(u64, u64, SessionEndpoint)>;

/// Retains the typed RPC endpoint for the selected client's authenticated generation.
#[derive(Clone)]
pub struct SettingsSessions {
    resolve: Rc<ResolveSession>,
    storage: Rc<dyn Platform>,
    relay_configured: bool,
}

impl SettingsSessions {
    pub(crate) fn new<P: Platform + Clone + 'static>(
        client: &ClientRuntime<P>,
        storage: Rc<dyn Platform>,
    ) -> Self {
        let client = client.clone();
        Self {
            relay_configured: client.relay_environments().is_some(),
            storage,
            resolve: Rc::new(move |id| {
                let peer = client.client(id)?;
                Some((peer.serial(), peer.state().generation.0, peer.session()?))
            }),
        }
    }
    pub fn new_device_host_id(&self) -> Result<String, ClientError> {
        Ok(crate::operations::allocate_command_id(self.storage.as_ref(), None)?.0)
    }
    /// Only builds configured with T3 Connect have an account gate for guest links.
    pub async fn guest_sign_in_required(&self) -> bool {
        self.relay_configured && matches!(self.storage.cloud_identity().await, Ok(None))
    }
    pub fn session(&self, id: &EnvironmentId) -> Option<(u64, u64, SessionEndpoint)> {
        (self.resolve)(id)
    }
    pub async fn dismissed(&self) -> Result<Vec<String>, ClientError> {
        Ok(self
            .storage
            .list(
                StorageArea::Ordinary,
                "database-storage-notice-dismissed".into(),
            )
            .await?
            .into_iter()
            .map(|key| key.record)
            .collect())
    }
    pub async fn dismiss(&self, id: String) -> Result<(), ClientError> {
        let key = StorageKey::new("database-storage-notice-dismissed", None, id);
        self.storage
            .write(
                StorageArea::Ordinary,
                vec![StorageWrite {
                    key,
                    value: Some(b"true".to_vec()),
                }],
            )
            .await?;
        Ok(())
    }
}
