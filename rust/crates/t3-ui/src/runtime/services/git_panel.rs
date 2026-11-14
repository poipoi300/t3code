//! Mount with `provide_context(git_panel::git_actions(client.git_panel()))`.
use crate::screens::panel::git_actions::*;
use futures::{StreamExt, future::LocalBoxFuture, stream::LocalBoxStream};
use std::rc::Rc;
use t3_client::{
    ClientError,
    platform::Platform,
    workspace::git_panel::{GitPanel, actions as client},
};
use t3_contracts::{git::*, source_control::*};

pub struct GitPanelBinding<P: Platform + Clone + 'static>(GitPanel<P>);
pub fn thread_details<P: Platform + Clone + 'static>(service: GitPanel<P>) -> GitPanel<P> {
    service
}
pub fn git_actions<P: Platform + Clone + 'static>(service: GitPanel<P>) -> GitActionsRuntime {
    GitActionsRuntime(Rc::new(GitPanelBinding(service)))
}
pub fn branch_selector<P: Platform + Clone + 'static>(
    service: GitPanel<P>,
    key: &t3_client::types::ThreadKey,
) -> t3_client::workspace::git_panel::branches::BranchSelectorLease<P> {
    service.branch_selector(key)
}
pub fn editor_picker<P: Platform + Clone + 'static>(
    service: GitPanel<P>,
    key: &t3_client::types::ThreadKey,
) -> t3_client::workspace::git_panel::editors::EditorPicker<P> {
    service.editor_picker(key)
}
pub fn project_scripts<P: Platform + Clone + 'static>(
    service: GitPanel<P>,
    key: &t3_client::types::ThreadKey,
) -> t3_client::workspace::git_panel::scripts::ProjectScripts<P> {
    service.project_scripts(key)
}

impl crate::runtime::StoreSignals {
    pub fn environment_config(
        &self,
        environment: t3_client::types::EnvironmentId,
    ) -> nsh_core::prelude::Memo<Option<crate::runtime::Shared<t3_contracts::server::ServerConfig>>>
    {
        let signals = *self;
        nsh_core::prelude::computed(move || {
            signals.track_environments();
            t3_client::workspace::git_panel::details::environment_config(
                &signals.store(),
                &environment,
            )
            .map(crate::runtime::Shared)
        })
    }
}
fn scope(value: &GitActionsScope) -> client::GitActionsScope {
    client::GitActionsScope {
        thread: value.thread.clone(),
        cwd: value.cwd.clone(),
        draft_id: value.draft_id.clone(),
    }
}
impl<P: Platform + Clone + 'static> GitActionsBackend for GitPanelBinding<P> {
    fn watch(
        &self,
        value: &GitActionsScope,
        watch: GitActionsWatch,
    ) -> LocalBoxStream<'static, GitActionsSnapshot> {
        self.0
            .watch_actions(
                &scope(value),
                client::GitActionsWatch {
                    show_actions: watch.show_actions,
                    show_changes: watch.show_changes,
                },
            )
            .map(|s| GitActionsSnapshot {
                status: s.status,
                status_error: s.status_error,
                branch_preview: s.branch_preview,
                action: s.action,
                can_write: s.can_write,
                can_operate: s.can_operate,
                is_local_draft: s.is_local_draft,
                has_server_thread: s.has_server_thread,
                latest_run_is_null: s.latest_run_is_null,
                selecting_worktree_base: s.selecting_worktree_base,
                project_cloning: s.project_cloning,
                thread_branch: s.thread_branch,
                discovery: s.discovery,
                offered_providers: s.offered_providers,
                publish_providers: s
                    .publish_providers
                    .into_iter()
                    .map(|p| PublishProvider {
                        kind: p.kind,
                        label: p.label,
                        host: p.host,
                        path_placeholder: p.path_placeholder,
                        icon_svg: p.icon_svg,
                    })
                    .collect(),
                presentation: s.presentation.map(|p| SourceControlPresentation {
                    singular: p.singular,
                    short_label: p.short_label,
                    icon_svg: p.icon_svg,
                }),
                announce_status_changes: s.announce_status_changes,
            })
            .boxed_local()
    }
    fn refresh(&self, s: &GitActionsScope) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.0.refresh_actions(&scope(s))
    }
    fn run_action(
        &self,
        s: &GitActionsScope,
        r: GitActionRequest,
    ) -> LocalBoxFuture<'static, Result<GitRunStackedActionResult, ClientError>> {
        self.0.run_action(
            &scope(s),
            client::GitActionRequest {
                action: r.action,
                commit_message: r.commit_message,
                feature_branch: r.feature_branch,
                file_paths: r.file_paths,
            },
        )
    }
    fn pull(
        &self,
        s: &GitActionsScope,
    ) -> LocalBoxFuture<'static, Result<VcsPullResult, ClientError>> {
        self.0.pull(&scope(s))
    }
    fn initialize(&self, s: &GitActionsScope) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.0.initialize(&scope(s))
    }
    fn publish(
        &self,
        s: &GitActionsScope,
        input: SourceControlPublishRepositoryInput,
    ) -> LocalBoxFuture<'static, Result<SourceControlPublishRepositoryResult, ClientError>> {
        self.0.publish(&scope(s), input)
    }
    fn sync_branch(
        &self,
        s: &GitActionsScope,
        branch: Option<String>,
        expected: Option<String>,
        manual: bool,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.0.sync_branch(&scope(s), branch, expected, manual)
    }
    fn open_file(
        &self,
        s: &GitActionsScope,
        root: String,
        path: String,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.0.open_file(&scope(s), root, path)
    }
}
