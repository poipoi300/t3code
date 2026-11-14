use super::ThreadOperations;
use crate::{ClientError, platform::Platform, rpc::RpcCallError, types::ThreadKey};
use futures::{FutureExt, future::LocalBoxFuture};
use t3_contracts::{base_schemas::TrimmedNonEmptyString, project::ProjectWriteFileInput, rpc::WsProjectsWriteFileRpc};

impl<P: Platform + Clone + 'static> ThreadOperations<P> {
    pub fn can_write(&self, key: &ThreadKey) -> bool {
        self.endpoint(&key.environment).is_ok_and(|e| e.available::<WsProjectsWriteFileRpc>(&ProjectWriteFileInput {
            cwd: TrimmedNonEmptyString("/".into()), relative_path: String::new(), contents: String::new(),
        }))
    }
    pub fn write_workspace_file(&self, key: ThreadKey, cwd: String, relative_path: String, contents: String) -> LocalBoxFuture<'static, Result<Option<String>, ClientError>> {
        let service = self.clone();
        async move {
            let endpoint = service.endpoint(&key.environment)?;
            let result = service.state.files.write_file(key.environment, &endpoint, &service.platform, ProjectWriteFileInput {
                cwd: TrimmedNonEmptyString(cwd), relative_path, contents,
            }).await;
            match result {
                Ok(result) => Ok(Some(result.relative_path)),
                Err(RpcCallError::Interrupted) => Ok(None),
                Err(error) => Err(error.into_client_error()),
            }
        }.boxed_local()
    }
    pub fn can_download(&self) -> bool { self.platform.text_download_supported() }
    pub fn download_text(&self, filename: String, contents: String, media_type: String) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let platform = self.platform.clone();
        async move { platform.download_text(filename, contents, media_type).await.map_err(Into::into) }.boxed_local()
    }
}
