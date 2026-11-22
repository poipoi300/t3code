//! Files lane: binds `t3_client::workspace::project_files` to the Files panel's service traits.
//! Pure delegation; the rules are the client's.
use crate::screens::panel::files_tree::{FileTreeHandle, FilesRuntime, FilesService};
use futures::{future::LocalBoxFuture, stream::LocalBoxStream};
use nsh_core::prelude::SharedString;
use std::rc::Rc;
use t3_client::{
    composition::ClientRuntime,
    platform::Platform,
    services::filesystem::ReadAccess,
    types::EnvironmentId,
    workspace::project_files::{
        FileTree, ProjectFiles,
        tree::{FileTreeView, Reveal},
    },
};

struct FilesBinding<P: Platform + Clone + 'static>(ProjectFiles<P>);

/// The Files panel's services over the host client.
pub fn files<P: Platform + Clone + 'static>(client: &ClientRuntime<P>) -> FilesRuntime {
    FilesRuntime(Rc::new(FilesBinding(client.project_files())))
}

impl<P: Platform + Clone + 'static> FilesService for FilesBinding<P> {
    fn tree(
        &self,
        environment: &EnvironmentId,
        cwd: &str,
        host_folder: bool,
    ) -> Rc<dyn FileTreeHandle> {
        Rc::new(TreeBinding(self.0.tree(environment, cwd, host_folder)))
    }
    fn read_access(&self, environment: &EnvironmentId) -> ReadAccess {
        self.0.read_access(environment)
    }
    fn access_changes(&self, environment: &EnvironmentId) -> LocalBoxStream<'static, ()> {
        self.0.changes(environment)
    }
    fn preference(&self, key: &'static str) -> LocalBoxFuture<'static, Option<serde_json::Value>> {
        self.0.preference(key)
    }
    fn set_preference(&self, key: &'static str, value: serde_json::Value) {
        self.0.set_preference(key, value);
    }
}

struct TreeBinding(FileTree);

impl FileTreeHandle for TreeBinding {
    fn view(&self) -> FileTreeView {
        self.0.view()
    }
    fn changes(&self) -> LocalBoxStream<'static, ()> {
        self.0.changes()
    }
    fn set_expanded(&self, ids: &[SharedString]) {
        self.0.set_expanded(ids);
    }
    fn toggle_all(&self) {
        self.0.toggle_all();
    }
    fn refresh(&self) {
        self.0.refresh();
    }
    fn set_query(&self, query: String) {
        self.0.set_query(query);
    }
    fn select(&self, path: &str) -> bool {
        self.0.select(path)
    }
    fn load_ancestors(&self, path: &str) {
        self.0.load_ancestors(path);
    }
    fn reveal(&self, path: Option<&str>, reveal_id: u64) -> Option<Reveal> {
        self.0.reveal(path, reveal_id)
    }
    fn observe_mutation(&self, mutation: Option<&str>) {
        self.0.observe_mutation(mutation);
    }
}

mod viewer {
    use super::*;
    use crate::screens::panel::file_viewer::{
        FileReadHandle, FileViewerRuntime, FileViewerService,
    };
    use t3_client::{
        types::ThreadKey,
        workspace::project_files::{
            FilesWorkspace,
            assets::AssetReadError,
            read::{FileRead, FileReadView},
        },
    };

    /// The file viewer's services over the host client.
    pub fn file_viewer<P: Platform + Clone + 'static>(
        client: &ClientRuntime<P>,
    ) -> FileViewerRuntime {
        FileViewerRuntime(Rc::new(FilesBinding(client.project_files())))
    }

    impl<P: Platform + Clone + 'static> FileViewerService for FilesBinding<P> {
        fn workspace(&self, thread: &ThreadKey) -> Option<FilesWorkspace> {
            self.0.workspace(thread)
        }
        fn workspace_mutation(&self, thread: &ThreadKey) -> Option<String> {
            self.0.workspace_mutation(thread)
        }
        fn can_write(&self, environment: &EnvironmentId) -> bool {
            self.0.can_write(environment)
        }
        fn read(
            &self,
            environment: &EnvironmentId,
            cwd: &str,
            relative_path: &str,
        ) -> Rc<dyn FileReadHandle> {
            Rc::new(ReadBinding(self.0.read_file(
                environment,
                cwd,
                relative_path,
            )))
        }
        fn asset_bytes(
            &self,
            environment: &EnvironmentId,
            resource: t3_contracts::assets::AssetResource,
            limit: usize,
            revision: Option<String>,
        ) -> LocalBoxFuture<'static, Result<Vec<u8>, AssetReadError>> {
            self.0.asset_bytes(environment, resource, limit, revision)
        }
    }

    struct ReadBinding(FileRead);

    impl FileReadHandle for ReadBinding {
        fn view(&self) -> FileReadView {
            self.0.view()
        }
        fn changes(&self) -> LocalBoxStream<'static, ()> {
            self.0.changes()
        }
        fn refresh(&self) {
            self.0.refresh();
        }
        fn observe_mutation(&self, mutation: Option<&str>) {
            self.0.observe_mutation(mutation);
        }
    }
}
pub use viewer::file_viewer;
