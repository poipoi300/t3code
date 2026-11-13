use super::super::{Registry, register};
use t3_contracts::filesystem::*;
pub fn register_module(registry: &mut Registry) {
    register::<FilesystemBrowseEntry>(registry, "FilesystemBrowseEntry");
    register::<FilesystemBrowseError>(registry, "FilesystemBrowseError");
    register::<FilesystemBrowseFailure>(registry, "FilesystemBrowseFailure");
    register::<FilesystemBrowseInput>(registry, "FilesystemBrowseInput");
    register::<FilesystemBrowseResult>(registry, "FilesystemBrowseResult");
    register::<FilesystemEntryMetadata>(registry, "FilesystemEntryMetadata");
    register::<FilesystemGetMetadataInput>(registry, "FilesystemGetMetadataInput");
    register::<FilesystemGetMetadataResult>(registry, "FilesystemGetMetadataResult");
}
#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("filesystem", register_module)
}
