//! Binding of the client's signed asset URLs (`t3_client::services::asset_urls`, the port of
//! `useAssetUrlState`) for the rows that draw host media. Pure delegation.
use futures::stream::LocalBoxStream;
use std::rc::Rc;
use t3_client::{
    composition::ClientRuntime, platform::Platform, services::assets::AssetUrlState,
    types::EnvironmentId,
};
use t3_contracts::assets::AssetResource;

pub trait AssetUrlClient {
    fn watch(
        &self,
        environment: EnvironmentId,
        resource: AssetResource,
    ) -> LocalBoxStream<'static, AssetUrlState>;
}

impl<P: Platform + Clone + 'static> AssetUrlClient
    for t3_client::services::asset_urls::AssetUrls<P>
{
    fn watch(
        &self,
        environment: EnvironmentId,
        resource: AssetResource,
    ) -> LocalBoxStream<'static, AssetUrlState> {
        t3_client::services::asset_urls::AssetUrls::watch(self, environment, resource)
    }
}

#[derive(Clone)]
pub struct AssetUrlBinding(pub Rc<dyn AssetUrlClient>);

pub fn asset_urls<P: Platform + Clone + 'static>(client: &ClientRuntime<P>) -> AssetUrlBinding {
    AssetUrlBinding(Rc::new(client.asset_urls()))
}

/// The host's asset URLs, `None` on the demo backend. Read in a component body.
pub fn host_asset_urls() -> Option<AssetUrlBinding> {
    crate::runtime::rt()
        .backend()
        .client()
        .map(|client| asset_urls(&client))
}
