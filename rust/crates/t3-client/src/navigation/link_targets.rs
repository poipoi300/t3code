//! packages/client-runtime/src/promptLinks.ts target selection.
use super::prompt_links::{PullRequest, Repository};
use crate::{
    state::pull_requests,
    types::{EnvironmentId, ThreadKey},
    view_logic::shell::models,
    workspace::projects::EnvironmentProject,
};
use t3_contracts::{
    model_selection::ModelSelection,
    orchestration_v2::OrchestrationV2ThreadShell,
    pull_request::PullRequestLinkedThreadsResult,
    server::{ServerProvider, ServerProviderAvailability, ServerProviderState},
};

pub fn repository_matches(project: &EnvironmentProject, repository: &Repository) -> bool {
    let Some(identity) = project
        .project
        .repository_identity
        .as_ref()
        .and_then(Option::as_ref)
    else {
        return false;
    };
    let key = identity
        .origin
        .as_ref()
        .map_or(&identity.canonical_key.0, |o| &o.canonical_key.0);
    ["github.com", "git.cafe", "staging.git.cafe"]
        .iter()
        .any(|host| {
            key.eq_ignore_ascii_case(&format!("{host}/{}/{}", repository.owner, repository.name))
        })
}
pub fn find_projects<'a>(
    projects: &'a [EnvironmentProject],
    repository: &Repository,
) -> Vec<&'a EnvironmentProject> {
    projects
        .iter()
        .filter(|p| repository_matches(p, repository))
        .collect()
}

pub fn find_pull_request_thread<'a>(
    threads: impl IntoIterator<Item = (&'a EnvironmentId, &'a OrchestrationV2ThreadShell)>,
    pr: &PullRequest,
) -> Option<ThreadKey> {
    let mut latest: Option<(&EnvironmentId, &OrchestrationV2ThreadShell)> = None;
    for (environment, thread) in threads {
        if latest.is_some_and(|(_, prior)| thread.updated_at.0 <= prior.updated_at.0)
            || thread.lineage.side_chat == Some(Some(true))
        {
            continue;
        }
        let linked=thread.pull_requests.as_ref().and_then(Option::as_ref).map(|links|links.iter().filter(|link|link.source!=t3_contracts::thread_pull_request::ThreadPullRequestLinkSource::StackDismissed).map(pull_requests::normalize_key).collect::<Vec<_>>())
            .unwrap_or_else(||thread.linked_pull_request.as_ref().and_then(Option::as_ref).map(pull_requests::legacy_key).into_iter().collect());
        if linked.iter().any(|key| {
            key.host.0 == "github.com"
                && key.repository.0.eq_ignore_ascii_case(&format!(
                    "{}/{}",
                    pr.repository.owner, pr.repository.name
                ))
                && key.number == pr.number
        }) {
            latest = Some((environment, thread));
        }
    }
    latest.map(|(environment, thread)| ThreadKey {
        environment: environment.clone(),
        thread: thread.id.clone(),
    })
}

pub enum Lookup<'a> {
    Pending,
    Failed,
    Ready(&'a PullRequestLinkedThreadsResult),
}
pub enum LookupChoice {
    Pending,
    None,
    Thread(ThreadKey),
}
pub fn pick_pull_request_thread<'a>(
    lookups: impl IntoIterator<Item = (EnvironmentId, Lookup<'a>)>,
) -> LookupChoice {
    for (environment, result) in lookups {
        match result {
            Lookup::Pending => return LookupChoice::Pending,
            Lookup::Failed => {}
            Lookup::Ready(result) => {
                if let Some(thread) = result.threads.iter().find(|t| t.archived_at.is_none()) {
                    return LookupChoice::Thread(ThreadKey {
                        environment,
                        thread: thread.id.clone(),
                    });
                }
            }
        }
    }
    LookupChoice::None
}

pub fn model_selection(
    providers: &[ServerProvider],
    provider: &str,
    model: Option<&str>,
) -> Option<(ModelSelection, bool)> {
    let driver = if provider == "claude" {
        "claudeagent"
    } else {
        provider
    };
    let usable = |p: &&ServerProvider| {
        p.driver.0.to_lowercase() == driver
            && p.enabled
            && p.availability != Some(Some(ServerProviderAvailability::Unavailable))
    };
    let instance = providers
        .iter()
        .filter(usable)
        .find(|p| p.status == ServerProviderState::Ready)
        .or_else(|| {
            providers
                .iter()
                .filter(usable)
                .find(|p| p.status == ServerProviderState::Warning)
        })?;
    let options = instance
        .models
        .iter()
        .map(|m| models::ModelOption {
            slug: m.slug.0.clone(),
            name: m.name.0.clone(),
            short_name: m.short_name.clone().flatten().map(|s| s.0),
            sub_provider: m.sub_provider.clone().flatten().map(|s| s.0),
            aliases: m
                .aliases
                .as_ref()
                .and_then(Option::as_ref)
                .map_or_else(Vec::new, |a| a.iter().map(|s| s.0.clone()).collect()),
            badge_new: false,
            is_custom: m.is_custom,
            is_default: m.is_default == Some(Some(true)),
            is_legacy: m.is_legacy == Some(Some(true)),
            is_unavailable: false,
        })
        .collect::<Vec<_>>();
    let requested = models::resolve_model(&instance.driver.0, model, &options);
    let exact = model.is_none() || requested.is_some();
    let model = requested.or_else(|| {
        options
            .iter()
            .find(|m| m.is_default)
            .or_else(|| options.first())
            .map(|m| m.slug.as_str())
    })?;
    Some((
        ModelSelection {
            instance_id: instance.instance_id.clone(),
            model: model.into(),
            options: None,
        },
        exact,
    ))
}
