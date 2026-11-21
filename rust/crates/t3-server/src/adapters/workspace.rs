//! Filesystem and signed-asset boundary. Authorities are supplied by orchestration.
#[cfg(all(feature = "workspace", feature = "orchestration"))]
#[path = "workspace/attachment_membership.rs"]
mod attachment_membership;
#[cfg(feature = "workspace")]
mod enabled {
    use futures::future::BoxFuture;
    use std::{path::PathBuf, sync::Arc};
    use t3_contracts::{assets::*, chat_attachment::*, filesystem::*, project::*};
    pub use t3_workspace::{
        Error, Result,
        assets::{
            AssetAccess, AssetSigner, Checkout, ExternalAssets, ResolvedAsset, SourceControlMedia,
        },
        attachments::{AttachmentStore, DocumentText, UploadClaims},
        entries::WorkspaceEntries,
        filesystem::WorkspaceFileSystem,
        paths::{InternalPaths, KnownWorkspaces, WorkspaceRoots},
        project::FaviconResolver,
        search::SearchIndexes,
    };

    #[derive(Default)]
    pub struct AssetWorkspaceContext {
        pub workspace_root: Option<PathBuf>,
        pub project_favicon_path: Option<String>,
        pub checkout: Checkout,
    }
    /// Resolves active project/clone context or a thread's authoritative workspace.
    pub trait AssetAuthority: Send + Sync {
        fn resolve<'a>(
            &'a self,
            resource: &'a AssetResource,
        ) -> BoxFuture<'a, Result<Option<AssetWorkspaceContext>>>;
    }
    /// Upload issuance and deletion require an authenticated attachment adapter.
    /// ```compile_fail
    /// let _ = t3_server::adapters::workspace::WorkspaceAdapter::create_upload_url;
    /// ```
    /// ```compile_fail
    /// let _ = t3_server::adapters::workspace::WorkspaceAdapter::delete_attachment;
    /// ```
    pub struct WorkspaceAdapter {
        pub filesystem: WorkspaceFileSystem,
        pub entries: WorkspaceEntries,
        pub assets: AssetAccess,
        pub attachments: AttachmentStore,
        pub authority: Arc<dyn AssetAuthority>,
    }
    impl WorkspaceAdapter {
        pub async fn list_entries(
            &self,
            input: ProjectListEntriesInput,
        ) -> Result<ProjectListEntriesResult> {
            self.entries.list(input).await
        }
        pub async fn list_ignored_files(
            &self,
            input: ProjectListIgnoredFilesInput,
        ) -> Result<ProjectListIgnoredFilesResult> {
            self.entries.list_ignored_files(input).await
        }
        pub async fn read_file(
            &self,
            input: ProjectReadFileInput,
        ) -> Result<ProjectReadFileResult> {
            self.filesystem.read_file(input).await
        }
        pub async fn search_contents(
            &self,
            input: ProjectSearchContentsInput,
        ) -> Result<ProjectSearchContentsResult> {
            self.entries.search_contents(input).await
        }
        pub async fn search_entries(
            &self,
            input: ProjectSearchEntriesInput,
        ) -> Result<ProjectSearchEntriesResult> {
            self.entries.search(input).await
        }
        pub async fn write_file(
            &self,
            input: ProjectWriteFileInput,
        ) -> Result<ProjectWriteFileResult> {
            self.filesystem.write_file(input).await
        }
        pub async fn browse(&self, input: FilesystemBrowseInput) -> Result<FilesystemBrowseResult> {
            self.entries
                .browse_with_storage(input, Some(self.attachments.base.clone()))
                .await
        }
        pub async fn metadata(
            &self,
            input: FilesystemGetMetadataInput,
        ) -> Result<FilesystemGetMetadataResult> {
            self.filesystem.get_metadata(input).await
        }
        pub async fn create_url(
            &self,
            input: AssetCreateUrlInput,
            now_ms: u64,
        ) -> std::result::Result<AssetCreateUrlResult, Box<AssetAccessError>> {
            self.create_url_for(None, input, now_ms).await
        }
        pub(crate) async fn create_url_for(
            &self,
            caller: Option<&crate::registry::Context>,
            input: AssetCreateUrlInput,
            now_ms: u64,
        ) -> std::result::Result<AssetCreateUrlResult, Box<AssetAccessError>> {
            let mut resource = input.resource;
            let mut attachment_identity = None;
            if let AssetResource::Attachment { attachment_id, .. } = &resource {
                let denied = || Box::new(asset_error(resource.clone(), Error::AssetDenied));
                let caller = caller
                    .filter(|ctx| {
                        ctx.scopes
                            .contains(&t3_contracts::auth::AuthEnvironmentScope::OrchestrationRead)
                    })
                    .ok_or_else(denied)?;
                let id = &attachment_id.0;
                let parsed = t3_workspace::attachments::parse_id(id).ok_or_else(denied)?;
                if parsed.thread_segment == "pending" {
                    let issuer = crate::adapters::attachments::Principal::Session(
                        caller.session_id.0.clone(),
                    );
                    attachment_identity = Some(
                        self.attachments
                            .owner(id)
                            .await
                            .and_then(|r| r.preview_identity(&issuer.record()))
                            .ok_or_else(denied)?,
                    );
                } else if t3_workspace::attachments::resolve_by_id(&self.attachments.base, id)
                    .is_none()
                    || !thread_has_attachment(caller, id, parsed.thread_segment).await
                {
                    return Err(denied());
                }
            }
            let context = match &resource {
                AssetResource::DraftWorkspaceFile { cwd, .. } => {
                    let root = PathBuf::from(&cwd.0);
                    self.filesystem
                        .known
                        .load()
                        .await
                        .and_then(|known| known.assert_contains(&root))
                        .map_err(|error| Box::new(context_error(&resource, error)))?;
                    AssetWorkspaceContext {
                        workspace_root: Some(root),
                        ..Default::default()
                    }
                }
                AssetResource::WorkspaceFile { .. } | AssetResource::ProjectFavicon { .. } => {
                    self.authoritative_context(&resource).await?
                }
                AssetResource::MediaFile { path, .. }
                    if !std::path::Path::new(&path.0).is_absolute()
                        || t3_workspace::paths::is_network_path(&path.0) =>
                {
                    self.authoritative_context(&resource).await?
                }
                _ => AssetWorkspaceContext::default(),
            };
            if matches!(resource, AssetResource::ProjectFavicon { .. }) {
                let root = context.workspace_root.as_ref().ok_or_else(|| {
                    Box::new(AssetAccessError::WorkspaceContextNotFoundError(
                        AssetWorkspaceContextNotFoundError {
                            tag: AssetWorkspaceContextNotFoundErrorTag::Error,
                            resource: resource.clone(),
                        },
                    ))
                })?;
                if let AssetResource::ProjectFavicon { cwd, .. } = &mut resource {
                    cwd.0 = root.to_string_lossy().into_owned();
                }
            }
            self.assets
                .issue_url(
                    resource.clone(),
                    t3_workspace::assets::AssetContext {
                        workspace_root: context.workspace_root.as_deref(),
                        project_favicon_path: context.project_favicon_path.as_deref(),
                        checkout: context.checkout,
                        attachment_identity,
                    },
                    now_ms,
                )
                .await
                .map_err(|error| Box::new(asset_error(resource, error)))
        }
        async fn authoritative_context(
            &self,
            resource: &AssetResource,
        ) -> std::result::Result<AssetWorkspaceContext, Box<AssetAccessError>> {
            self.authority
                .resolve(resource)
                .await
                .map_err(|error| Box::new(context_error(resource, error)))?
                .ok_or_else(|| {
                    Box::new(AssetAccessError::WorkspaceContextNotFoundError(
                        AssetWorkspaceContextNotFoundError {
                            tag: AssetWorkspaceContextNotFoundErrorTag::Error,
                            resource: resource.clone(),
                        },
                    ))
                })
        }
        pub async fn persist_attachments(
            &self,
            input: PersistChatAttachmentsInput,
        ) -> Result<PersistChatAttachmentsResult> {
            self.attachments.persist_chat_attachments(input).await
        }
        pub async fn resolve_asset(
            &self,
            token: &str,
            suffix: &str,
            now_ms: u64,
        ) -> Result<Option<ResolvedAsset>> {
            self.assets.resolve(token, suffix, now_ms).await
        }
        pub fn validate_upload(&self, token: &str, now_ms: u64) -> Option<UploadClaims> {
            self.attachments.validate_upload(token, now_ms)
        }
        pub async fn upload(
            &self,
            token: &str,
            now_ms: u64,
            body: impl tokio::io::AsyncRead + Unpin,
        ) -> Result<()> {
            self.attachments
                .upload(token, now_ms, body)
                .await
                .map(|_| ())
        }
    }
    #[cfg(feature = "orchestration")]
    async fn thread_has_attachment(
        caller: &crate::registry::Context,
        id: &str,
        segment: String,
    ) -> bool {
        // The RPC gate supplies the environment's orchestration:read grant. Membership is
        // evaluated against live threads (including archives) in a single read transaction.
        let Some(orchestration) = caller
            .services
            .domain::<crate::adapters::orchestration::OrchestrationAdapter>()
        else {
            return false;
        };
        let Some(runtime) = orchestration.composed() else {
            return false;
        };
        let id = id.to_owned();
        runtime
            .projections
            .database
            .transaction(move |db| super::attachment_membership::readable(db, &id, &segment))
            .await
            .unwrap_or(false)
    }
    #[cfg(not(feature = "orchestration"))]
    async fn thread_has_attachment(_: &crate::registry::Context, _: &str, _: String) -> bool {
        false
    }
    fn context_error(resource: &AssetResource, error: Error) -> AssetAccessError {
        AssetAccessError::WorkspaceContextResolutionError(AssetWorkspaceContextResolutionError {
            tag: AssetWorkspaceContextResolutionErrorTag::Error,
            resource: resource.clone(),
            cause: serde_json::Value::String(error.to_string()),
        })
    }
    fn asset_error(resource: AssetResource, error: Error) -> AssetAccessError {
        let cause = serde_json::Value::String(error.to_string());
        match error {
            Error::AssetDenied => match &resource {
                AssetResource::Attachment { .. } => {
                    AssetAccessError::AttachmentNotFoundError(AssetAttachmentNotFoundError {
                        tag: AssetAttachmentNotFoundErrorTag::Error,
                        resource,
                    })
                }
                _ => AssetAccessError::WorkspaceAssetNotFoundError(
                    AssetWorkspaceAssetNotFoundError {
                        tag: AssetWorkspaceAssetNotFoundErrorTag::Error,
                        resource,
                        checked_path: None,
                    },
                ),
            },
            Error::OutsideRoot(_) | Error::PathEscape(_) | Error::UnknownWorkspace(_) => {
                AssetAccessError::WorkspacePathValidationError(AssetWorkspacePathValidationError {
                    tag: AssetWorkspacePathValidationErrorTag::Error,
                    resource,
                    cause,
                })
            }
            _ => AssetAccessError::WorkspaceResolutionError(AssetWorkspaceResolutionError {
                tag: AssetWorkspaceResolutionErrorTag::Error,
                resource,
                cause,
            }),
        }
    }
}
#[cfg(feature = "workspace")]
pub use enabled::*;
