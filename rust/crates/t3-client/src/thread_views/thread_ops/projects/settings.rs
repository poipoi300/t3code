//! Settings projections and commands for a logical project and its selected checkouts.
use super::super::ThreadOperations;
use crate::{
    ClientError,
    platform::Platform,
    services::settings::{
        scope::{ScopeGroup, ScopeMember, SettingsScopeSearch},
        service::SettingsScopeView,
    },
    workspace::{ProjectKey, projects::EnvironmentProject},
};
use futures::{FutureExt, future::LocalBoxFuture};
use serde_json::json;
use std::{collections::BTreeSet, rc::Rc};
use t3_contracts::{
    base_schemas::TrimmedNonEmptyString, settings::SidebarProjectGroupingMode, wire::trim_js,
};

#[derive(Clone, PartialEq)]
pub struct ProjectSettingsDetail {
    pub group: ScopeGroup,
    pub members: Vec<ScopeMember>,
    pub representative: Rc<EnvironmentProject>,
    pub projects: Vec<Rc<EnvironmentProject>>,
    pub can_edit: bool,
    pub editable: BTreeSet<ProjectKey>,
    pub can_organize: bool,
    pub pinned: bool,
    pub archived: bool,
}

impl ProjectSettingsDetail {
    pub fn is_partial(&self) -> bool {
        self.members.len() < self.group.members.len()
    }
}

pub fn member_key(member: &ScopeMember) -> ProjectKey {
    ProjectKey {
        environment: member.environment_id.clone(),
        project: member.id.clone(),
    }
}

/// ProjectSettingsPanel keeps the same checkouts selected after their logical group changes.
pub fn regrouped_selection(
    view: &SettingsScopeView,
    previous: &SettingsScopeSearch,
    member_keys: &[String],
) -> Option<SettingsScopeSearch> {
    if &view.search != previous || !view.scope.members.is_empty() {
        return None;
    }
    view.groups.iter().find_map(|group| {
        group
            .members
            .iter()
            .any(|member| member_keys.contains(&member.physical_project_key))
            .then(|| SettingsScopeSearch {
                project: Some(group.project_key.clone()),
                ..previous.clone()
            })
    })
}

impl<P: Platform + Clone + 'static> ThreadOperations<P> {
    pub fn project_settings_detail(
        &self,
        view: &SettingsScopeView,
    ) -> Option<ProjectSettingsDetail> {
        if !view.scope.is_project_scope() {
            return None;
        }
        let group = view
            .groups
            .iter()
            .find(|group| Some(&group.project_key) == view.search.project.as_ref())?;
        let entities = self.projects();
        let projects_for = |members: &[ScopeMember]| {
            members
                .iter()
                .filter_map(|member| {
                    let key = member_key(member);
                    entities
                        .projects
                        .iter()
                        .find(|project| project.key() == key)
                        .cloned()
                })
                .collect::<Vec<_>>()
        };
        let projects = projects_for(&view.scope.members);
        let all_projects = projects_for(&group.members);
        let preferred = all_projects
            .iter()
            .find(|project| Some(&project.environment) == view.primary.as_ref())
            .or_else(|| all_projects.first())?;
        let representative = projects
            .iter()
            .find(|project| project.key() == preferred.key())
            .or_else(|| projects.first())?
            .clone();
        let editable: BTreeSet<_> = view
            .scope
            .members
            .iter()
            .filter(|member| self.can_edit_project_settings(member))
            .map(member_key)
            .collect();
        let can_edit = projects.len() == view.scope.members.len()
            && view
                .scope
                .members
                .iter()
                .all(|member| editable.contains(&member_key(member)));
        Some(ProjectSettingsDetail {
            group: group.clone(),
            members: view.scope.members.clone(),
            representative,
            projects,
            can_edit,
            editable,
            can_organize: self
                .can_organize_projects(&group.members.iter().map(member_key).collect::<Vec<_>>()),
            pinned: all_projects.iter().any(|project| {
                project
                    .project
                    .pinned_at
                    .as_ref()
                    .is_some_and(Option::is_some)
            }),
            archived: all_projects.iter().all(|project| {
                project
                    .project
                    .archived_at
                    .as_ref()
                    .is_some_and(Option::is_some)
            }),
        })
    }

    pub fn can_edit_project_settings(&self, member: &ScopeMember) -> bool {
        self.check_project(&member_key(member), false).is_ok()
    }

    /// A group label may be derived from its repository. Only an edited field sets custom titles.
    pub fn rename_settings_project(
        &self,
        members: Vec<ScopeMember>,
        value: String,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let title = trim_js(&value);
        if title.is_empty() {
            return futures::future::ready(Err(Self::refused("Project title cannot be empty")))
                .boxed_local();
        }
        let keys: Vec<_> = members.iter().map(member_key).collect();
        let projects = self.projects();
        let needs_update = keys.iter().any(|key| {
            projects
                .projects
                .iter()
                .find(|project| project.key() == *key)
                .is_none_or(|project| {
                    project.project.title.0 != title
                        || project.project.has_custom_title != Some(Some(true))
                })
        });
        if needs_update {
            self.update_project_group(keys, json!({ "title": title }))
        } else {
            futures::future::ready(Ok(())).boxed_local()
        }
    }

    pub fn set_checkout_grouping(
        &self,
        member: ScopeMember,
        selection: Option<SidebarProjectGroupingMode>,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let preferences = self.runtime.composer_preferences();
        async move {
            preferences.load().await?;
            let key = TrimmedNonEmptyString(member.physical_project_key);
            preferences
                .update(move |settings| {
                    if let Some(mode) = selection {
                        settings
                            .sidebar_project_grouping_overrides
                            .insert(key, mode);
                    } else {
                        settings.sidebar_project_grouping_overrides.remove(&key);
                    }
                })
                .await
        }
        .boxed_local()
    }
}
/// The icon editor normalizes the draft before checking the TypeScript wire pattern.
pub fn normalize_monogram(value: &str) -> Option<String> {
    use unicode_general_category::{GeneralCategory, get_general_category};
    use unicode_normalization::UnicodeNormalization;

    fn letter_or_number(c: char) -> bool {
        matches!(
            get_general_category(c),
            GeneralCategory::UppercaseLetter
                | GeneralCategory::LowercaseLetter
                | GeneralCategory::TitlecaseLetter
                | GeneralCategory::ModifierLetter
                | GeneralCategory::OtherLetter
                | GeneralCategory::DecimalNumber
                | GeneralCategory::LetterNumber
                | GeneralCategory::OtherNumber
        )
    }
    let normalized: String = value.nfkc().collect();
    let normalized = trim_js(&normalized).to_uppercase();
    let mut chars = normalized.chars();
    let valid = normalized.encode_utf16().count() <= 32
        && chars.next().is_some_and(letter_or_number)
        && chars.all(|c| {
            letter_or_number(c)
                || matches!(
                    get_general_category(c),
                    GeneralCategory::NonspacingMark
                        | GeneralCategory::SpacingMark
                        | GeneralCategory::EnclosingMark
                )
                || matches!(c, '\u{200c}' | '\u{200d}')
        });
    valid.then_some(normalized)
}

impl<P: Platform + Clone + 'static> ThreadOperations<P> {
    pub fn search_project_images(
        &self,
        member: ScopeMember,
        query: String,
    ) -> LocalBoxFuture<'static, Result<Vec<t3_contracts::project::ProjectEntry>, ClientError>>
    {
        use crate::rpc::RpcEndpoint;
        use t3_contracts::{
            base_schemas::TrimmedString,
            project::{ProjectEntryKind, ProjectSearchEntriesInput},
            rpc::WsProjectsSearchEntriesRpc,
        };
        let endpoint = self.endpoint(&member.environment_id);
        async move {
            let entries = endpoint?
                .call::<WsProjectsSearchEntriesRpc>(ProjectSearchEntriesInput {
                    cwd: TrimmedNonEmptyString(member.workspace_root),
                    query: TrimmedString(trim_js(&query).to_owned()),
                    limit: 200,
                    kind: Some(Some(ProjectEntryKind::File)),
                    image_only: Some(Some(true)),
                })
                .await?
                .entries;
            Ok(entries
                .into_iter()
                .filter(|entry| entry.kind == ProjectEntryKind::File)
                .collect())
        }
        .boxed_local()
    }
}

impl<P: Platform + Clone + 'static> ThreadOperations<P> {
    pub fn plan_settings_project_removal(
        &self,
        detail: &ProjectSettingsDetail,
        members: &[ScopeMember],
    ) -> Option<crate::workspace::project_ops::entities::ProjectRemovalPlan> {
        use crate::{
            thread_views::threads::older_settled::{
                OlderSettled, OlderSettledScope, unloaded_older_settled_count,
            },
            types::ThreadKey,
            workspace::project_ops::entities::{ProjectRemovalMember, plan_project_removal},
        };
        use std::collections::BTreeMap;

        if members.is_empty()
            || !members
                .iter()
                .all(|member| self.can_edit_project_settings(member))
        {
            return None;
        }
        let store = self.runtime.store();
        let environments: BTreeSet<_> = members
            .iter()
            .map(|member| &member.environment_id)
            .collect();
        let mut threads = Vec::new();
        let mut older = BTreeMap::new();
        for environment in environments {
            if let Some(shell) = store.shared_shell(environment) {
                threads.extend(
                    shell
                        .threads
                        .iter()
                        .map(|thread| (environment.clone(), thread.as_ref().clone())),
                );
                if let Some(counts) = shell.older_settled.as_ref()
                    && let Some(before_ms) = crate::time::epoch_ms(&counts.before.0)
                {
                    older.insert(
                        environment.clone(),
                        OlderSettled {
                            before: t3_contracts::wire::WireDateTime(counts.before.0.clone()),
                            before_ms,
                            counts: counts
                                .counts
                                .iter()
                                .map(|row| (row.project_id.clone(), row.count))
                                .collect(),
                        },
                    );
                }
            }
        }
        let keys: Vec<_> = threads
            .iter()
            .map(|(environment, thread)| ThreadKey {
                environment: environment.clone(),
                thread: thread.id.clone(),
            })
            .collect();
        let scoped_threads: Vec<_> = keys
            .iter()
            .zip(threads.iter().map(|(_, thread)| thread))
            .collect();
        let scopes = members
            .iter()
            .map(|member| OlderSettledScope {
                environment: member.environment_id.clone(),
                project_ids: Some(vec![member.id.clone()]),
            })
            .collect::<Vec<_>>();
        let unloaded = unloaded_older_settled_count(&scopes, &older, &scoped_threads, &|date| {
            crate::time::epoch_ms(&date.0)
        });
        let removals = members
            .iter()
            .map(|member| ProjectRemovalMember {
                key: member_key(member),
                title: detail
                    .projects
                    .iter()
                    .find(|project| project.key() == member_key(member))
                    .map(|project| project.project.title.0.clone())
                    .unwrap_or_else(|| detail.group.display_name.clone()),
                workspace_root: member.workspace_root.clone(),
                environment_label: member.environment_label.clone(),
            })
            .collect::<Vec<_>>();
        Some(plan_project_removal(
            &removals,
            detail.members.len(),
            detail.is_partial(),
            &detail.group.display_name,
            &threads,
            unloaded.try_into().unwrap_or(usize::MAX - threads.len()),
        ))
    }

    pub fn remove_settings_project(
        &self,
        members: Vec<ScopeMember>,
        remaining: Option<ScopeMember>,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        use crate::thread_views::composer::web_sessions;
        use futures::StreamExt;
        use t3_contracts::environment::ScopedProjectRef;

        let service = self.clone();
        async move {
            for member in &members {
                service.check_project(&member_key(member), false)?;
            }
            for member in members {
                let store = service.runtime.store();
                let mut updates = store.subscribe_environment(&member.environment_id);
                service.remove_project(member_key(&member), true).await?;
                let disappeared = async {
                    while store.shared_shell(&member.environment_id).is_some_and(|shell| {
                        shell.projects.iter().any(|project| project.id == member.id)
                    }) {
                        if updates.next().await.is_none() {
                            return;
                        }
                    }
                };
                let timeout = service.platform.sleep(5_000);
                futures::pin_mut!(disappeared, timeout);
                let _ = futures::future::select(disappeared, timeout).await;

                let project = ScopedProjectRef {
                    environment_id: member.environment_id.clone(), project_id: member.id.clone(),
                };
                let drafts = service.runtime.composer_drafts();
                if drafts.writable() {
                    let now = crate::time::iso_from_epoch_ms(service.platform.now_epoch_ms());
                    drafts.update(|doc| {
                        let draft = web_sessions::by_project(doc, &project).map(|(id, _)| id.to_owned());
                        if let Some(id) = draft {
                            if let Some(remaining) = &remaining {
                                web_sessions::update_context(doc, web_sessions::Target::Draft(&id),
                                    Some(&ScopedProjectRef {
                                        environment_id: remaining.environment_id.clone(), project_id: remaining.id.clone(),
                                    }),
                                    &json!({"environmentSelection":"manual","loadBalancedEnvironmentId":null}), &now);
                            } else {
                                web_sessions::clear(doc, &id);
                            }
                        }
                        Ok(())
                    })?;
                }
            }
            Ok(())
        }.boxed_local()
    }
}
