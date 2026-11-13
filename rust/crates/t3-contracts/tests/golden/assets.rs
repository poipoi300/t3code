use super::super::{Registry, register};
use t3_contracts::assets::*;
pub fn register_module(registry: &mut Registry) {
    register::<AssetAccessError>(registry, "AssetAccessError");
    register::<AssetAttachmentNotFoundError>(registry, "AssetAttachmentNotFoundError");
    register::<AssetCreateUrlInput>(registry, "AssetCreateUrlInput");
    register::<AssetCreateUrlResult>(registry, "AssetCreateUrlResult");
    register::<AssetImageDimensions>(registry, "AssetImageDimensions");
    register::<AssetPreviewTypeValidationError>(registry, "AssetPreviewTypeValidationError");
    register::<AssetProjectFaviconInspectionError>(registry, "AssetProjectFaviconInspectionError");
    register::<AssetProjectFaviconNotFoundError>(registry, "AssetProjectFaviconNotFoundError");
    register::<AssetProjectFaviconResolutionError>(registry, "AssetProjectFaviconResolutionError");
    register::<AssetResource>(registry, "AssetResource");
    register::<AssetSigningKeyLoadError>(registry, "AssetSigningKeyLoadError");
    register::<AssetSourceControlMediaUrlValidationError>(
        registry,
        "AssetSourceControlMediaUrlValidationError",
    );
    register::<AssetWorkspaceAssetInspectionError>(registry, "AssetWorkspaceAssetInspectionError");
    register::<AssetWorkspaceAssetNotFoundError>(registry, "AssetWorkspaceAssetNotFoundError");
    register::<AssetWorkspaceContextNotFoundError>(registry, "AssetWorkspaceContextNotFoundError");
    register::<AssetWorkspaceContextResolutionError>(
        registry,
        "AssetWorkspaceContextResolutionError",
    );
    register::<AssetWorkspacePathValidationError>(registry, "AssetWorkspacePathValidationError");
    register::<AssetWorkspaceResolutionError>(registry, "AssetWorkspaceResolutionError");
    register::<AssetWorkspaceRootNormalizationError>(
        registry,
        "AssetWorkspaceRootNormalizationError",
    );
    register::<AttachmentCreateUploadUrlInput>(registry, "AttachmentCreateUploadUrlInput");
    register::<AttachmentCreateUploadUrlResult>(registry, "AttachmentCreateUploadUrlResult");
    register::<AttachmentDeleteInput>(registry, "AttachmentDeleteInput");
    register::<AttachmentUploadSigningKeyError>(registry, "AttachmentUploadSigningKeyError");
    register::<NativeAppIconReference>(registry, "NativeAppIconReference");
}
#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("assets", register_module)
}
