use super::{
    ResourceKey,
    filesystem::{ReadAccess, windows_path},
    resources::{LocalLimiter, ResourcePolicy},
};
use crate::{
    ClientError,
    platform::{Credentials, HttpRequest, Platform},
    rpc::{RpcCallError, RpcEndpoint},
    types::{EnvironmentId, QueryState},
};
use std::{collections::BTreeMap, rc::Rc};
use t3_contracts::{
    assets::*,
    chat_attachment::PROVIDER_SEND_TURN_MAX_FILE_BYTES,
    rpc::{
        WsAssetsCreateUrlRpc as AssetsCreateUrl,
        WsAttachmentsCreateUploadUrlRpc as AttachmentsCreateUploadUrl,
        WsAttachmentsDeleteRpc as AttachmentsDelete,
    },
    speech::SpeechNumber,
};
use url::Url;

pub const ASSET_URL_MAX_IN_FLIGHT: usize = 6;
pub const ASSET_URL_POLICY: ResourcePolicy = ResourcePolicy {
    stale_ms: 1_800_000,
    idle_ttl_ms: 3_600_000,
    refresh_ms: Some(1_800_000),
    completed_ttl_ms: None,
};
pub const MISSING_FILE_ACCESS_MESSAGE: &str =
    "This connection cannot read host files. Pair again with a new link to view it.";
pub fn resolve_asset_url(base: &str, relative: &str) -> Option<String> {
    Url::parse(base).ok()?.join(relative).ok().map(Into::into)
}
#[derive(Debug, Clone, PartialEq)]
pub enum AssetUrlState {
    Loading,
    Failure {
        missing_file_access: bool,
    },
    Success {
        url: String,
        asset: Rc<AssetCreateUrlResult>,
    },
}
pub fn blocked_asset_url(
    resource: Option<&AssetResource>,
    access: &ReadAccess,
) -> Option<AssetUrlState> {
    let needs_files = matches!(
        resource,
        Some(
            AssetResource::WorkspaceFile { .. }
                | AssetResource::MediaFile { .. }
                | AssetResource::DraftWorkspaceFile { .. }
                | AssetResource::NativeAppIcon {
                    app: NativeAppIconReference::Path(_)
                }
        )
    );
    if access.allowed || !needs_files {
        return None;
    }
    Some(if access.pending {
        AssetUrlState::Loading
    } else {
        AssetUrlState::Failure {
            missing_file_access: access.error.is_none(),
        }
    })
}
pub fn asset_url_state(
    state: &QueryState<Rc<AssetCreateUrlResult>>,
    base: Option<&str>,
) -> AssetUrlState {
    if state.error.is_some() {
        return AssetUrlState::Failure {
            missing_file_access: false,
        };
    }
    let (Some(asset), Some(base)) = (&state.value, base) else {
        return AssetUrlState::Loading;
    };
    resolve_asset_url(base, asset.relative_url.0.as_str()).map_or(
        AssetUrlState::Failure {
            missing_file_access: false,
        },
        |url| AssetUrlState::Success {
            url,
            asset: asset.clone(),
        },
    )
}
fn expires_after(asset: &AssetCreateUrlResult, now: u64, minimum: u64) -> bool {
    match &asset.expires_at {
        SpeechNumber::Finite(value) => value
            .as_f64()
            .is_some_and(|value| value - now as f64 > minimum as f64),
        SpeechNumber::NonFinite(_) => false,
    }
}
pub fn keep_unchanged_file(
    shown: &Rc<AssetCreateUrlResult>,
    next: AssetCreateUrlResult,
    now: u64,
) -> Rc<AssetCreateUrlResult> {
    if next
        .file_revision
        .as_ref()
        .and_then(Option::as_ref)
        .is_some()
        && next.file_revision == shown.file_revision
        && expires_after(shown, now, 300_000)
    {
        shown.clone()
    } else {
        Rc::new(next)
    }
}
pub fn can_fallback_to_local_media(resource: &AssetResource, error: &AssetAccessError) -> bool {
    let AssetResource::MediaFile {
        path,
        host_only: None,
        ..
    } = resource
    else {
        return false;
    };
    let extension = path
        .0
        .as_str()
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    (path.0.as_str().starts_with('/') || windows_path(path.0.as_str()))
        && matches!(
            extension.as_str(),
            "png"
                | "jpg"
                | "jpeg"
                | "gif"
                | "webp"
                | "avif"
                | "svg"
                | "bmp"
                | "ico"
                | "heic"
                | "heif"
                | "tif"
                | "tiff"
                | "avi"
                | "mkv"
                | "mp4"
                | "webm"
                | "mov"
                | "m4v"
                | "ogv"
        )
        && matches!(
            error,
            AssetAccessError::WorkspaceAssetNotFoundError(_)
                | AssetAccessError::WorkspaceAssetInspectionError(_)
                | AssetAccessError::WorkspaceContextNotFoundError(_)
        )
}
pub fn asset_resource_key(
    environment: EnvironmentId,
    resource: &AssetResource,
) -> Result<ResourceKey, ClientError> {
    Ok(ResourceKey {
        environment,
        resource: "assets.createUrl".into(),
        arguments: serde_json::to_string(resource)?,
    })
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetRevisionAction {
    Share,
    Mint,
    MintAfterCurrent,
}
#[derive(Default)]
struct RevisionedAsset {
    minted: Option<Option<String>>,
    pending: Option<Option<String>>,
    shown: Option<Rc<AssetCreateUrlResult>>,
}
#[derive(Default)]
pub struct RevisionedAssets {
    entries: BTreeMap<ResourceKey, RevisionedAsset>,
}
impl RevisionedAssets {
    pub fn read(
        &mut self,
        key: ResourceKey,
        revision: Option<String>,
        in_flight: bool,
    ) -> AssetRevisionAction {
        let entry = self.entries.entry(key).or_default();
        if entry.minted.as_ref() == Some(&revision) {
            return AssetRevisionAction::Share;
        }
        if entry.minted.is_none() || !in_flight {
            entry.minted = Some(revision);
            return if in_flight {
                AssetRevisionAction::Share
            } else {
                AssetRevisionAction::Mint
            };
        }
        entry.pending = Some(revision);
        AssetRevisionAction::MintAfterCurrent
    }
    pub fn settled(
        &mut self,
        key: &ResourceKey,
        value: AssetCreateUrlResult,
        now: u64,
    ) -> (Rc<AssetCreateUrlResult>, bool) {
        let entry = self.entries.entry(key.clone()).or_default();
        let shown = entry.shown.as_ref().map_or_else(
            || Rc::new(value.clone()),
            |shown| keep_unchanged_file(shown, value.clone(), now),
        );
        entry.shown = Some(shown.clone());
        let mint_again = if let Some(revision) = entry.pending.take() {
            entry.minted = Some(revision);
            true
        } else {
            false
        };
        (shown, mint_again)
    }
    pub fn remove_environment(&mut self, environment: &EnvironmentId) {
        self.entries
            .retain(|key, _| &key.environment != environment);
    }
}
#[derive(Clone, Default)]
pub struct AssetService {
    limits: Rc<std::cell::RefCell<BTreeMap<EnvironmentId, LocalLimiter>>>,
}
impl AssetService {
    pub async fn read<R: RpcEndpoint>(
        &self,
        query: &super::resources::Resource<AssetCreateUrlResult>,
        platform: &(impl Platform + ?Sized),
        environment: EnvironmentId,
        endpoint: &R,
        local: Option<(&EnvironmentId, &R, &str)>,
        resource: AssetResource,
    ) -> Result<(), ClientError> {
        query
            .run(platform, |_| {
                self.mint_with_fallback(environment, endpoint, local, resource)
            })
            .await
    }
    pub async fn mint(
        &self,
        environment: EnvironmentId,
        endpoint: &impl RpcEndpoint,
        resource: AssetResource,
    ) -> Result<AssetCreateUrlResult, ClientError> {
        let limit = self
            .limits
            .borrow_mut()
            .entry(environment)
            .or_insert_with(|| LocalLimiter::new(ASSET_URL_MAX_IN_FLIGHT))
            .clone();
        let _permit = limit.acquire().await;
        endpoint
            .call::<AssetsCreateUrl>(AssetCreateUrlInput { resource })
            .await
    }
    pub fn remove_environment(&self, environment: &EnvironmentId) {
        self.limits.borrow_mut().remove(environment);
    }
    pub async fn mint_with_fallback<R: RpcEndpoint>(
        &self,
        environment: EnvironmentId,
        endpoint: &R,
        local: Option<(&EnvironmentId, &R, &str)>,
        resource: AssetResource,
    ) -> Result<AssetCreateUrlResult, ClientError> {
        let limit = self
            .limits
            .borrow_mut()
            .entry(environment.clone())
            .or_insert_with(|| LocalLimiter::new(ASSET_URL_MAX_IN_FLIGHT))
            .clone();
        let _permit = limit.acquire().await;
        let failure = match endpoint
            .command::<AssetsCreateUrl>(AssetCreateUrlInput {
                resource: resource.clone(),
            })
            .await
        {
            Ok(value) => return Ok(value),
            Err(error) => error,
        };
        let allowed = match &failure {
            RpcCallError::Rejected(errors) => errors
                .iter()
                .filter_map(|error| serde_json::from_value::<AssetAccessError>(error.clone()).ok())
                .any(|error| can_fallback_to_local_media(&resource, &error)),
            _ => false,
        };
        drop(_permit);
        if let Some((local_environment, local_endpoint, base)) = local
            && local_environment != &environment
            && allowed
            && let Ok(mut asset) = self
                .mint(local_environment.clone(), local_endpoint, resource)
                .await
            && let Some(url) = resolve_asset_url(base, &asset.relative_url.0)
        {
            asset.relative_url.0 = url;
            return Ok(asset);
        }
        Err(failure.into_client_error())
    }
}
#[derive(Debug)]
pub enum AttachmentVerification {
    Verified,
    Missing,
    Failed(ClientError),
}
pub async fn verify_persisted_attachment(
    endpoint: &impl RpcEndpoint,
    attachment_id: String,
) -> AttachmentVerification {
    let resource = AssetResource::Attachment {
        attachment_id: t3_contracts::base_schemas::TrimmedNonEmptyString(attachment_id),
        file_name: None,
        mime_type: None,
        disposition: None,
        render_intent: None,
    };
    match endpoint
        .command::<AssetsCreateUrl>(AssetCreateUrlInput { resource })
        .await
    {
        Ok(_) => AttachmentVerification::Verified,
        Err(RpcCallError::Rejected(errors)) if errors.iter().any(is_attachment_not_found) => {
            AttachmentVerification::Missing
        }
        Err(error) => AttachmentVerification::Failed(error.into_client_error()),
    }
}
pub fn parse_asset_collection_key(
    key: &str,
) -> Result<(EnvironmentId, Vec<AssetResource>), ClientError> {
    Ok(serde_json::from_str(key)?)
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProjectFaviconKey {
    pub environment: EnvironmentId,
    pub cwd: String,
    pub path: Option<String>,
}
#[derive(Default)]
pub struct ProjectFaviconUrls {
    urls: BTreeMap<ProjectFaviconKey, String>,
    clone_phases:
        BTreeMap<ProjectFaviconKey, Option<t3_contracts::project_clone::ProjectClonePhase>>,
}
impl ProjectFaviconUrls {
    pub fn resolve(
        &mut self,
        mut key: ProjectFaviconKey,
        state: &QueryState<Rc<AssetCreateUrlResult>>,
        base: Option<&str>,
    ) -> Option<&str> {
        if key.path.as_deref() == Some("") {
            key.path = None;
        }
        if let AssetUrlState::Success { url, .. } = asset_url_state(state, base) {
            self.urls.insert(key.clone(), url);
        }
        self.urls.get(&key).map(String::as_str)
    }
    pub fn clone_phase_changed(
        &mut self,
        key: ProjectFaviconKey,
        phase: Option<t3_contracts::project_clone::ProjectClonePhase>,
        query: &super::resources::Resource<AssetCreateUrlResult>,
    ) {
        let changed = self.clone_phases.insert(key, phase.clone()).flatten() != phase;
        if changed {
            query.invalidate();
        }
    }
    pub fn remove_environment(&mut self, environment: &EnvironmentId) {
        self.urls.retain(|key, _| &key.environment != environment);
        self.clone_phases
            .retain(|key, _| &key.environment != environment);
    }
}
pub fn project_favicon_resource(cwd: &str, path: Option<&str>) -> AssetResource {
    AssetResource::ProjectFavicon {
        cwd: t3_contracts::base_schemas::TrimmedNonEmptyString(cwd.into()),
        path: path.filter(|path| !path.is_empty()).map(|path| {
            Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                path.into(),
            ))
        }),
    }
}
pub fn is_project_favicon_fallback_url(url: &str) -> bool {
    Url::parse("https://t3.invalid")
        .ok()
        .and_then(|base| base.join(url).ok())
        .is_some_and(|url| url.path().rsplit('/').next() == Some("project-favicon-missing"))
}
pub fn is_attachment_not_found(error: &serde_json::Value) -> bool {
    error.get("_tag").and_then(serde_json::Value::as_str) == Some("AssetAttachmentNotFoundError")
}
pub fn clamp_file_attachment_upload_bytes(advertised: u64) -> u64 {
    advertised.min(PROVIDER_SEND_TURN_MAX_FILE_BYTES)
}
pub fn format_attachment_size(bytes: u64) -> String {
    let kb = bytes.div_ceil(1024).max(1);
    if kb >= 1024 {
        let tenths = (u128::from(bytes) * 10 + 524_288) / 1_048_576;
        format!("{}.{:01} MB", tenths / 10, tenths % 10)
    } else {
        format!("{kb} KB")
    }
}
pub fn file_attachment_too_large_message(name: &str, limit: u64) -> String {
    let size = if limit >= 1_048_576 && limit.is_multiple_of(1_048_576) {
        format!("{} MB", limit / 1_048_576)
    } else if limit >= 1024 && limit.is_multiple_of(1024) {
        format!("{} KB", limit / 1024)
    } else {
        format!("{limit} {}", if limit == 1 { "byte" } else { "bytes" })
    };
    format!("'{name}' exceeds the {size} attachment limit.")
}
#[derive(Debug)]
pub enum AttachmentUploadResult {
    Uploaded {
        attachment_id: String,
    },
    Cancelled {
        attachment_id: String,
    },
    Failed {
        step: AttachmentUploadStep,
        attachment_id: Option<String>,
        error: ClientError,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentUploadStep {
    Mint,
    ResolveUrl,
    Transfer,
}
pub async fn attachment_upload_cycle(
    endpoint: &impl RpcEndpoint,
    platform: &(impl Platform + ?Sized),
    base: &str,
    upload: AttachmentCreateUploadUrlInput,
    body: Vec<u8>,
    mime: &str,
    on_minted: impl FnOnce(&str) -> bool,
) -> AttachmentUploadResult {
    let minted = match endpoint.call::<AttachmentsCreateUploadUrl>(upload).await {
        Ok(minted) => minted,
        Err(error) => {
            return AttachmentUploadResult::Failed {
                step: AttachmentUploadStep::Mint,
                attachment_id: None,
                error,
            };
        }
    };
    let id = minted.attachment_id.0.as_str().to_string();
    if !on_minted(&id) {
        let input = AttachmentDeleteInput {
            attachment_id: minted.attachment_id,
        };
        let _result = endpoint.call::<AttachmentsDelete>(input).await;
        return AttachmentUploadResult::Cancelled { attachment_id: id };
    }
    let Some(url) = resolve_asset_url(base, minted.relative_url.0.as_str()) else {
        return AttachmentUploadResult::Failed {
            step: AttachmentUploadStep::ResolveUrl,
            attachment_id: Some(id),
            error: ClientError::Disconnected,
        };
    };
    let result = platform
        .http(HttpRequest {
            method: "POST".into(),
            url,
            headers: BTreeMap::from([("content-type".into(), mime.into())]),
            body,
            credentials: Credentials::Omit,
            max_response_bytes: 64 * 1024,
        })
        .await;
    match result {
        Ok(response) if (200..300).contains(&response.status) => {
            AttachmentUploadResult::Uploaded { attachment_id: id }
        }
        Ok(_) => AttachmentUploadResult::Failed {
            step: AttachmentUploadStep::Transfer,
            attachment_id: Some(id),
            error: ClientError::InvalidPayload("attachment transfer rejected".into()),
        },
        Err(error) => AttachmentUploadResult::Failed {
            step: AttachmentUploadStep::Transfer,
            attachment_id: Some(id),
            error: error.into(),
        },
    }
}
