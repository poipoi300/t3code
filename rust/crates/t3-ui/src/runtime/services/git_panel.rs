//! Mount with `provide_context(git_panel::git_actions(client.git_panel()))`, then the
//! registrations below (`git_actions_registration`, `branch_registration`,
//! `thread_details_runtime`).
use crate::screens::panel::{
    branch::{BranchSelectorBackend, BranchSelectorState, branch_toolbar_branch_selector},
    details::{
        EditorPickerBackend, ProjectScriptsBackend, ThreadDetailsBackend, ThreadDetailsRuntime,
    },
    git_actions::*,
};
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

/// GIT-1 in the details card's version-control slot (PNL-HOST): the control over the thread's
/// checkout, re-derived when the environment's shell changes. Its Changes row opens the Diff
/// surface.
pub fn git_actions_registration<P: Platform + Clone + 'static>(
    service: GitPanel<P>,
) -> crate::screens::panel::GitActionsRegistration {
    crate::screens::panel::GitActionsRegistration(Rc::new(
        move |key: t3_client::types::ThreadKey| {
            let service = service.clone();
            let thread = key.clone();
            let scope =
                crate::runtime::rt()
                    .store
                    .shell_state(key.environment.clone(), move |_| {
                        service
                            .workspace_context(&thread, None)
                            .map(|context| GitActionsScope {
                                thread: thread.clone(),
                                cwd: checkout_dir(
                                    context.worktree_path.as_deref(),
                                    &context.project.workspace_root.0,
                                ),
                                draft_id: context.draft_id.clone(),
                            })
                    });
            let host = crate::screens::panel::workspace_panel_host();
            let mut props =
                GitActionsControlProps::new(nsh_core::prelude::Prop::bind(move || scope.get()));
            // ThreadDetailsPanel's `showActions={visible.gitActions}` and `showChanges`.
            if let Some(details) = nsh_core::prelude::use_context::<ThreadDetailsRuntime>() {
                let visibility = details.visibility;
                props.show_actions = nsh_core::prelude::Prop::bind(move || {
                    visibility.with(|v| v.get("gitActions").copied().unwrap_or(true))
                });
                props.show_changes = nsh_core::prelude::Prop::bind(move || {
                    visibility.with(|v| v.get("changes").copied().unwrap_or(true))
                });
            }
            props.on_open_changes = Some(Rc::new(move || {
                host.apply(&key, crate::screens::panel::Action::Open("diff".into()))
            }));
            nsh_core::prelude::IntoElement::into_element(git_actions_control(props))
        },
    ))
}

/// GIT-2 in the details card's version-control slot: one branch-selector lease per mounted row.
pub fn branch_registration<P: Platform + Clone + 'static>(
    service: GitPanel<P>,
) -> crate::screens::panel::BranchRegistration {
    crate::screens::panel::BranchRegistration(Rc::new(move |key: t3_client::types::ThreadKey| {
        let lease = branch_selector(service.clone(), &key);
        nsh_core::prelude::IntoElement::into_element(branch_toolbar_branch_selector(Rc::new(
            BranchLeaseBinding(lease),
        )))
    }))
}

struct BranchLeaseBinding<P: Platform + Clone + 'static>(
    t3_client::workspace::git_panel::branches::BranchSelectorLease<P>,
);
impl<P: Platform + Clone + 'static> BranchSelectorBackend for BranchLeaseBinding<P> {
    fn watch(&self) -> LocalBoxStream<'static, Rc<BranchSelectorState>> {
        futures::stream::once(futures::future::ready(self.0.snapshot()))
            .chain(self.0.changes())
            .boxed_local()
    }
    fn set_query(&self, query: String) {
        self.0.set_query(query);
    }
    fn refresh(&self) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.0.refresh()
    }
    fn load_next(&self) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.0.load_next()
    }
    fn select_ref(&self, name: String) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.0.select_ref(name)
    }
    fn create_ref(&self, name: String) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.0.create_ref(name)
    }
    fn set_start_from_origin(
        &self,
        value: bool,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.0.set_start_from_origin(value)
    }
}

/// PNL-DETAILS: item visibility, the editor picker and project scripts. Build it at the app root.
pub fn thread_details_runtime<P: Platform + Clone + 'static>(
    service: GitPanel<P>,
) -> ThreadDetailsRuntime {
    crate::screens::panel::details::thread_details_runtime(Rc::new(DetailsBinding(thread_details(
        service,
    ))))
}

struct DetailsBinding<P: Platform + Clone + 'static>(GitPanel<P>);
impl<P: Platform + Clone + 'static> ThreadDetailsBackend for DetailsBinding<P> {
    fn visibility(&self) -> LocalBoxStream<'static, std::collections::BTreeMap<String, bool>> {
        let service = self.0.clone();
        futures::stream::once(futures::future::ready(self.0.details_visibility()))
            .chain(
                self.0
                    .details_changes()
                    .map(move |()| service.details_visibility()),
            )
            .boxed_local()
    }
    fn set_item(
        &self,
        id: String,
        visible: bool,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.0.set_details_item(id, visible)
    }
    fn reset_items(&self) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.0.reset_details_items()
    }
    fn editor_picker(&self, key: &t3_client::types::ThreadKey) -> Rc<dyn EditorPickerBackend> {
        Rc::new(EditorBinding(editor_picker(self.0.clone(), key)))
    }
    fn project_scripts(&self, key: &t3_client::types::ThreadKey) -> Rc<dyn ProjectScriptsBackend> {
        Rc::new(ScriptsBinding(project_scripts(self.0.clone(), key)))
    }
}

type EditorSnapshot = t3_client::workspace::git_panel::editors::EditorPickerSnapshot;
struct EditorBinding<P: Platform + Clone + 'static>(
    t3_client::workspace::git_panel::editors::EditorPicker<P>,
);
impl<P: Platform + Clone + 'static> EditorPickerBackend for EditorBinding<P> {
    fn watch(&self) -> LocalBoxStream<'static, Result<EditorSnapshot, ClientError>> {
        self.0.changes()
    }
    fn select_editor(
        &self,
        editor: t3_contracts::editor::EditorId,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.0.select_editor(editor)
    }
    fn open_editor(
        &self,
        editor: t3_contracts::editor::EditorId,
    ) -> LocalBoxFuture<'static, Result<bool, ClientError>> {
        self.0.open_editor(editor)
    }
}

type NewScript = t3_client::workspace::git_panel::scripts::NewProjectScriptInput;
type ScriptsSnapshot = t3_client::workspace::git_panel::scripts::ProjectScriptsSnapshot;
struct ScriptsBinding<P: Platform + Clone + 'static>(
    t3_client::workspace::git_panel::scripts::ProjectScripts<P>,
);
impl<P: Platform + Clone + 'static> ProjectScriptsBackend for ScriptsBinding<P> {
    fn watch(&self) -> LocalBoxStream<'static, Result<ScriptsSnapshot, ClientError>> {
        self.0.changes()
    }
    fn add(&self, input: NewScript) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.0.add(input)
    }
    fn update(
        &self,
        id: String,
        input: NewScript,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.0.update(id, input)
    }
    fn delete(&self, id: String) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.0.delete(id)
    }
    fn import(
        &self,
        script: t3_contracts::t3_project_file::T3ProjectFileScript,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.0.import(script)
    }
    fn run(&self, id: String) -> LocalBoxFuture<'static, Result<String, ClientError>> {
        self.0.run(id)
    }
}

/// `gitCwd`: the thread's worktree, else its project's workspace root.
fn checkout_dir(worktree: Option<&str>, workspace_root: &str) -> String {
    worktree.unwrap_or(workspace_root).to_owned()
}

#[cfg(test)]
mod registration_tests {
    use super::checkout_dir;

    #[test]
    fn a_worktree_is_the_checkout_and_the_project_root_is_the_fallback() {
        assert_eq!(checkout_dir(Some("/w/feature"), "/repo"), "/w/feature");
        assert_eq!(checkout_dir(None, "/repo"), "/repo");
    }
}
