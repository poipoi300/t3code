//! Attachment cleanup runs only after every durable owner has loaded and flushed.
use super::{
    attachment_files::{AttachmentRetention, reference_key, resolve_owned_uri},
    mobile_drafts::AttachmentReferences,
};
use crate::{
    ClientError,
    platform::{HostFuture, PlatformError},
    rpc::RpcEndpoint,
    types::EnvironmentId,
};
use futures::{FutureExt, future::LocalBoxFuture, lock::Mutex};
use serde_json::Value;
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

pub trait CleanupSources {
    /// False means at least one owner could not be read; its resources remain owned.
    fn hydrate_and_flush(&self) -> LocalBoxFuture<'_, Result<bool, ClientError>>;
    /// Includes current drafts, outbox, signed-out archives, and in-flight share imports.
    fn references(&self) -> Option<AttachmentReferences>;
    fn incoming_shares(&self) -> LocalBoxFuture<'_, Result<Vec<Value>, ClientError>>;
}
pub struct StoredCleanupSources<P: crate::platform::Platform, E: RpcEndpoint> {
    pub drafts: Rc<super::mobile_store::MobileDraftStore<P>>,
    pub sends: super::send::SendService<P, E>,
    active_imports: RefCell<Vec<Rc<Value>>>,
    ready: std::cell::Cell<bool>,
}
impl<P: crate::platform::Platform + 'static, E: RpcEndpoint + 'static> StoredCleanupSources<P, E> {
    pub fn new(
        drafts: Rc<super::mobile_store::MobileDraftStore<P>>,
        sends: super::send::SendService<P, E>,
    ) -> Result<Self, ClientError> {
        if !Rc::ptr_eq(&drafts.platform, &sends.inner.platform) {
            return Err(ClientError::InvalidPayload(
                "Cleanup owners must use the same host.".into(),
            ));
        }
        Ok(Self {
            drafts,
            sends,
            active_imports: RefCell::new(vec![]),
            ready: std::cell::Cell::new(false),
        })
    }
    pub fn retain_import(&self, owner: Value) -> Rc<Value> {
        let owner = Rc::new(owner);
        self.active_imports.borrow_mut().push(owner.clone());
        owner
    }
}
impl<P: crate::platform::Platform + 'static, E: RpcEndpoint + 'static> CleanupSources
    for StoredCleanupSources<P, E>
{
    fn hydrate_and_flush(&self) -> LocalBoxFuture<'_, Result<bool, ClientError>> {
        async move {
            self.ready.set(false);
            self.drafts.flush_all().await?;
            let ready = self.sends.flush_outbox().await?;
            self.ready.set(ready);
            Ok(ready)
        }
        .boxed_local()
    }
    fn references(&self) -> Option<AttachmentReferences> {
        if !self.ready.get() || !self.drafts.loaded() {
            return None;
        }
        let mut imports = self.active_imports.borrow_mut();
        imports.retain(|owner| Rc::strong_count(owner) > 1);
        Some(AttachmentReferences::from_document(
            &self.drafts.snapshot(),
            &self.sends.attachment_owners(),
            &imports
                .iter()
                .map(|owner| owner.as_ref().clone())
                .collect::<Vec<_>>(),
        ))
    }
    fn incoming_shares(&self) -> LocalBoxFuture<'_, Result<Vec<Value>, ClientError>> {
        async move {
            use crate::platform::StorageArea;
            let mut shares = vec![];
            for key in self
                .drafts
                .platform
                .list(StorageArea::Ordinary, "incoming-share".into())
                .await?
            {
                let bytes = self
                    .drafts
                    .platform
                    .get(StorageArea::Ordinary, key.clone())
                    .await?
                    .ok_or_else(|| {
                        ClientError::InvalidPayload(
                            "An incoming share disappeared during ownership loading.".into(),
                        )
                    })?;
                let record: crate::persistence::Record<Value> = serde_json::from_slice(&bytes)?;
                if record.format_version != crate::persistence::FORMAT_VERSION
                    || record.kind != key.namespace
                    || record.environment_id != key.environment
                    || record.record_id != key.record
                    || record.payload_version != 1
                {
                    return Err(ClientError::InvalidPayload(
                        "Unreadable incoming share ownership record.".into(),
                    ));
                }
                let share = record.payload;
                if share["schemaVersion"] != 1
                    || ["id", "createdAt", "text"]
                        .iter()
                        .any(|field| !share[*field].is_string())
                    || share["attachments"].as_array().is_none_or(|attachments| {
                        attachments.iter().any(|attachment| {
                            crate::thread_views::outbox::DraftAttachment::decode(attachment.clone())
                                .is_err()
                        })
                    })
                    || share["warnings"]
                        .as_array()
                        .is_none_or(|warnings| warnings.iter().any(|warning| !warning.is_string()))
                {
                    return Err(ClientError::InvalidPayload(
                        "Unreadable incoming share retained for recovery.".into(),
                    ));
                }
                shares.push(share);
            }
            Ok(shares)
        }
        .boxed_local()
    }
}
pub trait ResourceRemoval {
    fn remove_resource(&self, handle: String) -> HostFuture<'_, ()>;
}
pub struct HostResourceRemoval<P>(pub Rc<P>);
impl<P: crate::platform::Platform> ResourceRemoval for HostResourceRemoval<P> {
    fn remove_resource(&self, handle: String) -> HostFuture<'_, ()> {
        self.0.remove_resource(handle)
    }
}
/// Hosts without registered-resource removal retain candidates for a later retry.
pub struct PendingResourceRemoval;
impl ResourceRemoval for PendingResourceRemoval {
    fn remove_resource(&self, _handle: String) -> HostFuture<'_, ()> {
        async {
            Err(PlatformError::Unsupported(
                "HostAction::RemoveResource is not implemented by this host.".into(),
            ))
        }
        .boxed_local()
    }
}
pub trait CleanupRemoval {
    fn file(&self, uri: String) -> LocalBoxFuture<'_, Result<(), ClientError>>;
    fn upload(
        &self,
        environment: EnvironmentId,
        id: String,
    ) -> LocalBoxFuture<'_, Result<(), ClientError>>;
}
pub struct HostCleanup<E, R> {
    pub resources: R,
    pub endpoints: BTreeMap<EnvironmentId, Rc<E>>,
}
impl<E: RpcEndpoint, R: ResourceRemoval> CleanupRemoval for HostCleanup<E, R> {
    fn file(&self, uri: String) -> LocalBoxFuture<'_, Result<(), ClientError>> {
        async move {
            self.resources
                .remove_resource(uri)
                .await
                .map_err(ClientError::from)
        }
        .boxed_local()
    }
    fn upload(
        &self,
        environment: EnvironmentId,
        id: String,
    ) -> LocalBoxFuture<'_, Result<(), ClientError>> {
        async move {
            let endpoint = self
                .endpoints
                .get(&environment)
                .ok_or(ClientError::Disconnected)?;
            if endpoint.destination() != Some(environment) {
                return Err(ClientError::EnvironmentMismatch);
            }
            endpoint
                .call::<t3_contracts::rpc::WsAttachmentsDeleteRpc>(
                    t3_contracts::assets::AttachmentDeleteInput {
                        attachment_id: serde_json::from_value(Value::String(id))?,
                    },
                )
                .await?;
            Ok(())
        }
        .boxed_local()
    }
}
#[derive(Debug, Default)]
pub struct CleanupReport {
    pub files_removed: Vec<String>,
    pub uploads_removed: Vec<(EnvironmentId, String)>,
    pub failures: Vec<String>,
    pub deferred: bool,
}
#[derive(Default)]
pub struct AttachmentCleanup {
    files: RefCell<BTreeMap<String, String>>,
    uploads: RefCell<BTreeMap<(EnvironmentId, String), ()>>,
    running: Mutex<()>,
}
impl AttachmentCleanup {
    pub fn schedule(&self, attachments: &[Value]) {
        for attachment in attachments {
            if let Some(uri) = attachment["fileUri"].as_str() {
                self.files
                    .borrow_mut()
                    .insert(reference_key(uri), uri.into());
            }
            if let (Some(environment), Some(id)) = (
                attachment["uploadEnvironmentId"].as_str(),
                attachment["uploadedAttachmentId"].as_str(),
            ) {
                self.uploads
                    .borrow_mut()
                    .insert((EnvironmentId(environment.into()), id.into()), ());
            }
        }
    }
    pub fn pending(&self) -> usize {
        self.files.borrow().len() + self.uploads.borrow().len()
    }
    pub async fn run_once(
        &self,
        sources: &impl CleanupSources,
        removal: &impl CleanupRemoval,
        retention: &AttachmentRetention,
        document_directory: &str,
    ) -> Result<CleanupReport, ClientError> {
        let _guard = self.running.lock().await;
        let mut report = CleanupReport::default();
        if self.pending() == 0 {
            return Ok(report);
        }
        if !sources.hydrate_and_flush().await? || sources.references().is_none() {
            report.deferred = true;
            return Ok(report);
        }
        let incoming = match sources.incoming_shares().await {
            Ok(shares) => shares,
            Err(error) => {
                report.deferred = true;
                report.failures.push(error.to_string());
                return Ok(report);
            }
        };
        let mut shares = AttachmentReferences::default();
        for owner in &incoming {
            shares.retain_owner(owner);
        }
        let files = self.files.borrow().clone();
        for (key, uri) in files {
            let Some(references) = sources.references() else {
                report.deferred = true;
                return Ok(report);
            };
            if references.files.contains(&key)
                || shares.files.contains(&key)
                || retention.is_retained(&uri)
            {
                continue;
            }
            let Some(resolved) = resolve_owned_uri(&uri, document_directory) else {
                self.files.borrow_mut().remove(&key);
                continue;
            };
            match removal.file(resolved).await {
                Ok(()) => {
                    self.files.borrow_mut().remove(&key);
                    report.files_removed.push(uri);
                }
                Err(error) => report.failures.push(error.to_string()),
            }
        }
        let uploads = self.uploads.borrow().keys().cloned().collect::<Vec<_>>();
        for (environment, id) in uploads {
            let Some(references) = sources.references() else {
                report.deferred = true;
                return Ok(report);
            };
            let key = (environment.0.clone(), id.clone());
            if references.uploads.contains(&key) || shares.uploads.contains(&key) {
                continue;
            }
            match removal.upload(environment.clone(), id.clone()).await {
                Ok(()) => {
                    self.uploads
                        .borrow_mut()
                        .remove(&(environment.clone(), id.clone()));
                    report.uploads_removed.push((environment, id));
                }
                Err(error) => report.failures.push(error.to_string()),
            }
        }
        Ok(report)
    }
}
