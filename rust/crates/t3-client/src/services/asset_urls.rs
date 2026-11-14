//! Port of apps/web/src/assets/assetUrls.ts `useAssetUrlState`: a signed URL for one asset of
//! one environment, minted through `assets.createUrl` and shared by every reader of the same
//! resource (`ClientRuntime::query`, with the web's asset policy).
//!
//! A file-backed resource on a connection that cannot read host files never mints: it answers
//! the client's `blocked_asset_url` state, as the web does before creating its atom.
use super::assets::{ASSET_URL_POLICY, AssetUrlState, asset_url_state, blocked_asset_url};
use crate::{
    composition::{ClientRuntime, QueryLease},
    platform::Platform,
    types::EnvironmentId,
};
use futures::{StreamExt, stream::LocalBoxStream};
use t3_contracts::{
    assets::{AssetCreateUrlInput, AssetCreateUrlResult, AssetResource},
    rpc::WsAssetsCreateUrlRpc,
};

#[derive(Clone)]
pub struct AssetUrls<P: Platform + Clone + 'static> {
    runtime: ClientRuntime<P>,
    platform: P,
}

impl<P: Platform + Clone + 'static> AssetUrls<P> {
    pub fn new(runtime: ClientRuntime<P>, platform: P) -> Self {
        Self { runtime, platform }
    }

    /// The asset's URL state now, then on every change of the minted URL, the connection or
    /// its grants. The mint runs while the stream is held.
    pub fn watch(
        &self,
        environment: EnvironmentId,
        resource: AssetResource,
    ) -> LocalBoxStream<'static, AssetUrlState> {
        struct Watch<P: Platform + Clone + 'static> {
            service: AssetUrls<P>,
            environment: EnvironmentId,
            resource: AssetResource,
            lease: Option<QueryLease<AssetCreateUrlResult>>,
            changes: LocalBoxStream<'static, ()>,
            last: Option<AssetUrlState>,
        }
        let changes = self.runtime.project_files().changes(&environment);
        let watch = Watch {
            service: self.clone(),
            environment,
            resource,
            lease: None,
            changes,
            last: None,
        };
        futures::stream::unfold(watch, |mut watch| async move {
            loop {
                let state = watch.service.read(
                    &watch.environment,
                    &watch.resource,
                    &mut watch.lease,
                    &mut watch.changes,
                );
                if watch.last.as_ref() != Some(&state) {
                    watch.last = Some(state.clone());
                    return Some((state, watch));
                }
                watch.changes.next().await?;
            }
        })
        .boxed_local()
    }

    /// Reads the current state, starting the mint (and merging its changes into `changes`) the
    /// first time the resource is readable.
    fn read(
        &self,
        environment: &EnvironmentId,
        resource: &AssetResource,
        lease: &mut Option<QueryLease<AssetCreateUrlResult>>,
        changes: &mut LocalBoxStream<'static, ()>,
    ) -> AssetUrlState {
        let access = self.runtime.project_files().read_access(environment);
        if let Some(blocked) = blocked_asset_url(Some(resource), &access) {
            return blocked;
        }
        let lease = lease.get_or_insert_with(|| {
            let lease = self.runtime.query::<WsAssetsCreateUrlRpc>(
                environment,
                AssetCreateUrlInput {
                    resource: resource.clone(),
                },
                ASSET_URL_POLICY,
            );
            let minted = lease
                .resource()
                .observe(self.platform.monotonic_ms())
                .map(|_| ());
            let previous = std::mem::replace(changes, futures::stream::pending().boxed_local());
            *changes = futures::stream::select(previous, minted).boxed_local();
            lease
        });
        let base = self
            .runtime
            .environments()
            .into_iter()
            .find(|entry| &entry.id == environment)
            .and_then(|entry| entry.routes.first().map(|route| route.http_base.clone()))
            .filter(|base| !base.is_empty());
        let state = lease.resource().snapshot();
        asset_url_state(&state, base.as_deref())
    }
}
