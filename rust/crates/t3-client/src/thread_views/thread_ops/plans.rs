use super::ThreadOperations;
use crate::{ClientError, platform::Platform, rpc::RpcCallError, types::ThreadKey};
use futures::{FutureExt, StreamExt, future::LocalBoxFuture, stream::LocalBoxStream};
use t3_contracts::{
    base_schemas::TrimmedNonEmptyString, project::ProjectWriteFileInput,
    rpc::WsProjectsWriteFileRpc,
};

impl<P: Platform + Clone + 'static> ThreadOperations<P> {
    pub fn plan_workspace_root(&self, key: &ThreadKey) -> Option<String> {
        let row = self.thread_shell(key)?;
        row.worktree_path
            .as_ref()
            .map(|path| path.0.clone())
            .or_else(|| {
                self.project(&crate::workspace::ProjectKey {
                    environment: key.environment.clone(),
                    project: row.project_id.clone(),
                })
                .map(|project| project.project.workspace_root.0.clone())
            })
    }
    pub fn watch_plan_workspace(&self, key: ThreadKey) -> LocalBoxStream<'static, Option<String>> {
        let changes = self.changes(&key);
        let service = self.clone();
        changes
            .map(move |_| service.plan_workspace_root(&key))
            .boxed_local()
    }
    pub fn can_write(&self, key: &ThreadKey) -> bool {
        if self.unsupported(&key.environment, "projects.writeFile") {
            return false;
        }
        self.endpoint(&key.environment).is_ok_and(|e| {
            e.available::<WsProjectsWriteFileRpc>(&ProjectWriteFileInput {
                cwd: TrimmedNonEmptyString("/".into()),
                relative_path: String::new(),
                contents: String::new(),
            })
        })
    }
    pub fn write_workspace_file(
        &self,
        key: ThreadKey,
        cwd: String,
        relative_path: String,
        contents: String,
    ) -> LocalBoxFuture<'static, Result<Option<String>, ClientError>> {
        let service = self.clone();
        async move {
            let endpoint = service.endpoint(&key.environment)?;
            let generation = service.session_generation(&key.environment);
            if let Some(generation) = generation {
                service.state.files.begin_session(
                    key.environment.clone(),
                    crate::types::Generation(generation),
                );
            }
            let result = service
                .state
                .files
                .write_file(
                    key.environment.clone(),
                    &endpoint,
                    &service.platform,
                    ProjectWriteFileInput {
                        cwd: TrimmedNonEmptyString(cwd),
                        relative_path,
                        contents,
                    },
                )
                .await;
            match result {
                Ok(result) => Ok(Some(result.relative_path)),
                Err(RpcCallError::Interrupted) => Ok(None),
                Err(error) => {
                    let error = error.into_client_error();
                    service.remember_unsupported(
                        &key.environment,
                        "projects.writeFile",
                        generation,
                        &error,
                    );
                    Err(error)
                }
            }
        }
        .boxed_local()
    }
    pub fn can_download(&self) -> bool {
        self.platform.text_download_supported()
    }
    pub fn download_text(
        &self,
        filename: String,
        contents: String,
        media_type: String,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let platform = self.platform.clone();
        async move {
            platform
                .download_text(filename, contents, media_type)
                .await
                .map_err(Into::into)
        }
        .boxed_local()
    }
}
