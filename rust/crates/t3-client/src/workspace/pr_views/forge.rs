use super::super::{pr_list::EnvironmentPullRequestEntry, projects::EnvironmentProject};
use super::{
    detail::{TimelineEvent, displayed_detail},
    markdown::{MarkdownNode, markdown_excerpt},
};
use crate::types::{EnvironmentId, ProjectId};
use std::collections::BTreeMap;
use t3_contracts::{
    environment::RepositoryIdentity,
    pull_request::*,
    thread_pull_request::{ThreadLinkedPullRequest, ThreadPullRequestLink},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeRequestLink {
    pub provider: String,
    pub host: String,
    pub authority: Option<String>,
    pub repository_url: String,
}
pub trait ForgeViews {
    fn public_host_provider(&self, host: &str) -> Option<String>;
    fn parse_change_request_url(&self, url: &str) -> Option<ChangeRequestLink>;
    fn change_request_host(&self, identity: &RepositoryIdentity, provider: &str) -> Option<String>;
    fn checkout_host(&self, identity: &RepositoryIdentity, provider: &str) -> Option<String>;
    fn checkout_command(
        &self,
        provider: &str,
        number: u64,
        head_branch: &str,
        head_repository: Option<&str>,
        repository_url: Option<&str>,
    ) -> Option<String>;
    fn repository_selector(&self, identity: &RepositoryIdentity) -> Option<String>;
    fn unreported_actions(&self, provider: &str) -> Vec<PullRequestAction>;
    fn lists_entire_host(&self, provider: &str) -> bool;
}
pub fn loading_checkout_command(
    reference: &PullRequestRef,
    identity: Option<&RepositoryIdentity>,
    forge: &impl ForgeViews,
) -> Option<String> {
    let host = reference
        .host
        .as_ref()
        .and_then(Option::as_ref)
        .map(|host| host.0.trim().to_lowercase());
    let provider = identity
        .and_then(|identity| identity.provider.as_ref())
        .map(|provider| provider.0.clone())
        .or_else(|| {
            host.as_deref()
                .and_then(|host| forge.public_host_provider(host))
        })?;
    let command = forge.checkout_command(&provider, reference.number, "", None, None)?;
    if let (Some(identity), Some(host)) = (
        identity.filter(|identity| identity.provider.is_some()),
        host,
    ) && forge.change_request_host(identity, &provider).as_deref() != Some(&host)
    {
        return None;
    }
    Some(command)
}
pub fn resolve_reference_host(
    reference: &PullRequestRef,
    identity: Option<&RepositoryIdentity>,
    forge: &impl ForgeViews,
) -> PullRequestRef {
    if reference.host.is_some() {
        return reference.clone();
    }
    let mut result = reference.clone();
    if let Some(identity) = identity
        && let Some(provider) = identity.provider.as_ref()
        && let Some(host) = forge.checkout_host(identity, &provider.0)
    {
        result.host = Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
            host,
        )));
    }
    result
}
pub fn displayed_forge_detail<'a>(
    live: Option<&'a PullRequestDetail>,
    cached: Option<&'a PullRequestDetail>,
    reference: &PullRequestRef,
    forge: &impl ForgeViews,
) -> Option<&'a PullRequestDetail> {
    if live.is_some() {
        return live;
    }
    let detail = cached?;
    let mut hostless = reference.clone();
    hostless.host = None;
    displayed_detail(None, Some(detail), &hostless)?;
    if let Some(host) = reference.host.as_ref().and_then(Option::as_ref) {
        let link = forge.parse_change_request_url(&detail.url.0)?;
        if link.authority.as_ref().unwrap_or(&link.host) != &host.0.to_lowercase() {
            return None;
        }
    }
    Some(detail)
}
pub fn host_actions(
    entry: &EnvironmentPullRequestEntry,
    providers: &[PullRequestProviderSummary],
    forge: &impl ForgeViews,
) -> Vec<PullRequestAction> {
    providers
        .iter()
        .rev()
        .find(|provider| {
            provider.host.0.eq_ignore_ascii_case(&entry.entry.host.0) && provider.actions.is_some()
        })
        .and_then(|provider| provider.actions.clone().flatten())
        .unwrap_or_else(|| forge.unreported_actions(&entry.entry.provider.0))
}
pub fn largest_read_count(
    entries: &[EnvironmentPullRequestEntry],
    forge: &impl ForgeViews,
) -> usize {
    let mut counts = BTreeMap::new();
    for entry in entries {
        let repository = if forge.lists_entire_host(&entry.entry.provider.0) {
            String::new()
        } else {
            entry.entry.repository.0.to_lowercase()
        };
        *counts
            .entry((
                &entry.environment,
                entry.entry.host.0.to_lowercase(),
                repository,
            ))
            .or_insert(0) += 1;
    }
    counts.into_values().max().unwrap_or(0)
}
pub struct RepositoryScope<'a> {
    pub repository: &'a str,
    pub host: Option<&'a str>,
    pub selected_environment: Option<&'a EnvironmentId>,
    pub scoped_project: Option<&'a ProjectId>,
    pub scoped_environment: Option<&'a EnvironmentId>,
}
pub fn find_project_for_repository<'a>(
    projects: &'a [EnvironmentProject],
    scope: RepositoryScope<'_>,
    forge: &impl ForgeViews,
) -> Option<&'a EnvironmentProject> {
    let candidates: Vec<_> = projects
        .iter()
        .filter(|project| {
            scope
                .selected_environment
                .is_none_or(|environment| &project.environment == environment)
        })
        .filter(|project| {
            project
                .project
                .repository_identity
                .as_ref()
                .and_then(Option::as_ref)
                .is_some_and(|identity| {
                    forge
                        .repository_selector(identity)
                        .is_some_and(|repository| repository.eq_ignore_ascii_case(scope.repository))
                        && scope.host.is_none_or(|host| {
                            forge
                                .change_request_host(
                                    identity,
                                    identity
                                        .provider
                                        .as_ref()
                                        .map_or("github", |provider| provider.0.as_str()),
                                )
                                .is_some_and(|value| value == host.to_lowercase())
                        })
                })
        })
        .collect();
    let scoped = scope.scoped_project.and_then(|id| {
        let mut matching = candidates.iter().copied().filter(|project| {
            &project.project.id == id
                && scope
                    .scoped_environment
                    .is_none_or(|environment| &project.environment == environment)
        });
        let first = matching.next()?;
        (scope.scoped_environment.is_some() || matching.next().is_none()).then_some(first)
    });
    scoped.or_else(|| candidates.first().copied())
}
pub trait ThreadLinkViews {
    fn visible<'a>(&self, links: &'a [ThreadPullRequestLink]) -> Vec<&'a ThreadPullRequestLink>;
    fn current<'a>(&self, links: &'a [ThreadPullRequestLink]) -> Option<&'a ThreadPullRequestLink>;
    fn matches_legacy(
        &self,
        link: &ThreadPullRequestLink,
        legacy: &ThreadLinkedPullRequest,
    ) -> bool;
}
pub struct ThreadPanel<'a> {
    pub project: Option<&'a ProjectId>,
    pub links: &'a [ThreadPullRequestLink],
    pub linked: Option<&'a ThreadLinkedPullRequest>,
    pub branch: Option<&'a ThreadLinkedPullRequest>,
}
pub fn panel_context(
    thread: &ThreadPanel<'_>,
    surface: &PullRequestRef,
    links: &impl ThreadLinkViews,
) -> &'static str {
    if thread.project != Some(&surface.project_id) {
        return "page";
    }
    let visible = links.visible(thread.links);
    let matches = if visible.is_empty() {
        thread.linked.or(thread.branch).is_some_and(|legacy| {
            legacy.repository == surface.repository && legacy.number == surface.number
        })
    } else {
        visible.iter().any(|link| {
            link.number == surface.number
                && link
                    .repository
                    .0
                    .eq_ignore_ascii_case(&surface.repository.0)
                && surface
                    .host
                    .as_ref()
                    .and_then(Option::as_ref)
                    .is_none_or(|host| link.host.0.eq_ignore_ascii_case(&host.0))
        })
    };
    if matches { "thread" } else { "page" }
}
pub enum PanelTarget<'a> {
    Legacy(&'a ThreadLinkedPullRequest),
    Current {
        project: &'a ProjectId,
        link: &'a ThreadPullRequestLink,
    },
}
pub fn panel_target<'a>(
    thread: &ThreadPanel<'a>,
    links: &impl ThreadLinkViews,
) -> Option<PanelTarget<'a>> {
    if let Some(current) = links.current(thread.links) {
        if let Some(legacy) = thread.linked
            && links.matches_legacy(current, legacy)
        {
            return Some(PanelTarget::Legacy(legacy));
        }
        return Some(PanelTarget::Current {
            project: thread.project?,
            link: current,
        });
    }
    thread.linked.or(thread.branch).map(PanelTarget::Legacy)
}
pub trait PullRequestAnchors {
    fn commit(&self, url: &str) -> Option<String>;
    fn timeline_anchor(&self, url: &str) -> Option<String>;
    fn resolve_review_anchor(
        &self,
        anchor: &str,
        comments: &[PullRequestComment],
        threads: &[PullRequestReviewThread],
    ) -> String;
    fn matches_url(&self, anchor: &str, url: Option<&str>) -> bool;
}
pub enum LinkSubject<'a> {
    Commit(&'a PullRequestCommit),
    Comment {
        author: Option<&'a PullRequestActor>,
        created_at: &'a str,
        review_state: Option<&'a str>,
        excerpt: Option<String>,
    },
}
pub fn link_subject<'a>(
    url: &str,
    activity: &'a PullRequestActivity,
    anchors: &impl PullRequestAnchors,
    parse_markdown: impl Fn(&str) -> MarkdownNode,
) -> Option<LinkSubject<'a>> {
    if let Some(oid) = anchors.commit(url) {
        return activity
            .commits
            .iter()
            .find(|commit| commit.oid.0.eq_ignore_ascii_case(&oid))
            .map(LinkSubject::Commit);
    }
    let anchor = anchors.timeline_anchor(url)?;
    let anchor =
        anchors.resolve_review_anchor(&anchor, &activity.comments, &activity.review_threads);
    if let Some(comment) = activity
        .comments
        .iter()
        .find(|comment| anchors.matches_url(&anchor, comment.url.as_deref()))
    {
        return Some(LinkSubject::Comment {
            author: comment.author.as_ref(),
            created_at: &comment.created_at.0,
            review_state: comment.review_state.as_deref(),
            excerpt: markdown_excerpt(&parse_markdown(&comment.body)),
        });
    }
    let comment = activity
        .review_threads
        .iter()
        .flat_map(|thread| &thread.comments)
        .find(|comment| anchors.matches_url(&anchor, comment.url.as_deref()))?;
    Some(LinkSubject::Comment {
        author: comment.author.as_ref(),
        created_at: &comment.created_at.0,
        review_state: None,
        excerpt: markdown_excerpt(&parse_markdown(&comment.body)),
    })
}
pub fn timeline_event_matches(
    event: &TimelineEvent,
    anchor: &str,
    anchors: &impl PullRequestAnchors,
) -> bool {
    anchors.matches_url(anchor, event.url.as_deref())
        || event.thread.as_ref().is_some_and(|thread| {
            thread
                .replies
                .iter()
                .any(|reply| anchors.matches_url(anchor, reply.url.as_deref()))
        })
}
