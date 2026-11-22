//! Pull-request link input parsing and resolution (`packages/client-runtime/src/pullRequestLinkInput.ts`).
use crate::{
    state::pull_requests::parsed_key,
    view_logic::{JsTrim, is_js_whitespace},
};
use t3_contracts::{
    base_schemas::TrimmedNonEmptyString,
    environment::RepositoryIdentity,
    thread_pull_request::{ThreadPullRequestKey, ThreadPullRequestLinkSource},
};
use url::Url;

const MAX_REQUEST_NUMBER: u64 = 9_007_199_254_740_991;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullRequestLinkProject {
    pub host: String,
    pub repository: String,
    pub terminology_singular: &'static str,
    pub provider_kind: String,
    pub remote_url: String,
    pub web_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedPullRequestLink {
    pub host: String,
    pub repository: String,
    pub number: u64,
    pub url: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PullRequestInputResolution {
    Empty,
    Invalid,
    Error(String),
    Resolved(ResolvedPullRequestLink),
}

pub fn pull_request_link_project(
    identity: Option<&RepositoryIdentity>,
) -> Option<PullRequestLinkProject> {
    let identity = identity?;
    let repository = identity
        .display_name
        .as_ref()
        .map(|s| s.0.clone())
        .or_else(|| match (&identity.owner, &identity.name) {
            (Some(owner), Some(name)) => Some(format!("{}/{}", owner.0, name.0)),
            _ => None,
        })?;
    let provider_kind = identity
        .provider
        .as_ref()
        .map(|p| p.0.js_trim().to_ascii_lowercase())
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| "unknown".into());
    let terminology_singular = match provider_kind.as_str() {
        "gitlab" => "merge request",
        _ => "pull request",
    };
    let host = change_request_host(identity, &provider_kind);
    Some(PullRequestLinkProject {
        host,
        repository,
        terminology_singular,
        provider_kind,
        remote_url: identity.locator.remote_url.0.clone(),
        web_url: identity.web_url.as_ref().map(|u| u.0.clone()),
    })
}

fn host_with_port(url: &Url) -> Option<String> {
    let host = url.host_str()?.to_ascii_lowercase();
    Some(match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host,
    })
}

fn change_request_host(identity: &RepositoryIdentity, provider_kind: &str) -> String {
    if provider_kind == "forgejo"
        && let Ok(remote) = Url::parse(identity.locator.remote_url.0.js_trim())
        && matches!(remote.scheme(), "http" | "https")
        && let Some(host) = host_with_port(&remote)
    {
        return host;
    }
    identity
        .canonical_key
        .0
        .split('/')
        .next()
        .map(JsTrim::js_trim)
        .filter(|h| !h.is_empty())
        .map(str::to_ascii_lowercase)
        .unwrap_or_else(|| provider_kind.to_owned())
}

fn remote_web_origin(host: &str, remote_url: Option<&str>) -> String {
    if let Some(raw) = remote_url
        && let Ok(remote) = Url::parse(raw.js_trim())
        && matches!(remote.scheme(), "http" | "https")
    {
        let hostname_match = remote
            .host_str()
            .is_some_and(|h| h.eq_ignore_ascii_case(host));
        let authority_match = host_with_port(&remote).is_some_and(|h| h.eq_ignore_ascii_case(host));
        if (hostname_match || authority_match)
            && let Some(origin_host) = host_with_port(&remote)
        {
            return format!("{}://{origin_host}", remote.scheme());
        }
    }
    format!("https://{host}")
}

fn change_request_url_below(base: &str, route: &str) -> String {
    format!("{}/{route}", base.trim_end_matches('/'))
}

fn canonical_azure_repository_key(key: &str) -> Option<String> {
    let lower = key.to_ascii_lowercase();
    let parts: Vec<&str> = lower.split('/').collect();
    let canonical = if matches!(
        parts.first().copied(),
        Some("ssh.dev.azure.com" | "vs-ssh.visualstudio.com")
    ) && parts.len() == 5
        && parts[1] == "v3"
    {
        format!("dev.azure.com/{}/{}/_git/{}", parts[2], parts[3], parts[4])
    } else if let Some(host) = parts.first().copied()
        && let Some(organization) = host
            .strip_suffix(".visualstudio.com")
            .filter(|org| !org.is_empty() && !org.contains('.'))
    {
        let rest = &parts[1..];
        let rest = if rest.first().copied() == Some("defaultcollection") {
            &rest[1..]
        } else {
            rest
        };
        if rest.len() == 3 && rest[1] == "_git" {
            format!("dev.azure.com/{organization}/{}", rest.join("/"))
        } else {
            lower
        }
    } else {
        lower
    };
    canonical.starts_with("dev.azure.com/").then_some(canonical)
}

impl PullRequestLinkProject {
    pub fn web_url(&self, number: u64) -> Option<String> {
        let host = self.host.trim();
        let repository = self.repository.trim().trim_matches('/');
        if repository.is_empty() {
            return None;
        }
        let web_url = self
            .web_url
            .as_deref()
            .map(str::trim)
            .filter(|base| !base.is_empty());
        match self.provider_kind.as_str() {
            "cursor-origin" => Some(format!(
                "https://cursor.com/codebase/{repository}/pull/{number}"
            )),
            "azure-devops" => {
                if let Some(base) = web_url {
                    return Some(change_request_url_below(
                        base,
                        &format!("pullrequest/{number}"),
                    ));
                }
                if host.is_empty() {
                    return None;
                }
                let raw_key = format!(
                    "{}/{}",
                    host.to_ascii_lowercase(),
                    repository.to_ascii_lowercase()
                );
                let canonical = canonical_azure_repository_key(&raw_key).unwrap_or(raw_key);
                let (canonical_host, path) = canonical.split_once('/')?;
                Some(format!(
                    "{}/{path}/pullrequest/{number}",
                    remote_web_origin(canonical_host, Some(&self.remote_url))
                ))
            }
            "github" | "gitlab" | "bitbucket" | "gitcafe" | "forgejo" | "gitea" => {
                let route = match self.provider_kind.as_str() {
                    "github" => format!("pull/{number}"),
                    "gitlab" => format!("-/merge_requests/{number}"),
                    "bitbucket" => format!("pull-requests/{number}"),
                    _ => format!("pulls/{number}"),
                };
                if let Some(base) = web_url {
                    return Some(change_request_url_below(base, &route));
                }
                if host.is_empty() {
                    return None;
                }
                Some(format!(
                    "{}/{repository}/{route}",
                    remote_web_origin(host, Some(&self.remote_url))
                ))
            }
            _ => None,
        }
    }
}

fn split_ci_words<'a>(input: &'a str, prefixes: &[&[&str]]) -> Option<&'a str> {
    let mut rest = input.js_trim();
    for alternatives in prefixes {
        let len = rest
            .char_indices()
            .find(|(_, ch)| is_js_whitespace(*ch))
            .map_or(rest.len(), |(idx, _)| idx);
        let (word, after) = rest.split_at(len);
        if !alternatives
            .iter()
            .any(|expected| word.eq_ignore_ascii_case(expected))
        {
            return None;
        }
        let next_start = after
            .char_indices()
            .find(|(_, ch)| !is_js_whitespace(*ch))
            .map(|(idx, _)| idx)?;
        rest = &after[next_start..];
    }
    let trimmed = rest.js_trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn checkout_command_id(args: &str) -> Option<&str> {
    let parts: Vec<&str> = args
        .js_trim()
        .split(is_js_whitespace)
        .filter(|part| !part.is_empty())
        .collect();
    for (index, part) in parts.iter().enumerate() {
        if *part == "--id" || *part == "-i" {
            return parts.get(index + 1).copied();
        }
        if let Some(id) = part.strip_prefix("--id=") {
            return (!id.is_empty()).then_some(id);
        }
    }
    parts.into_iter().find(|part| !part.starts_with('-'))
}

fn checkout_command_argument(input: &str) -> Option<&str> {
    let trimmed = input.js_trim();
    if let Some(rest) = split_ci_words(trimmed, &[&["gh"], &["pr"], &["checkout"]])
        .or_else(|| split_ci_words(trimmed, &[&["glab"], &["mr"], &["checkout"]]))
        .or_else(|| split_ci_words(trimmed, &[&["origin"], &["pr"], &["checkout"]]))
        .or_else(|| {
            split_ci_words(
                trimmed,
                &[&["tea"], &["pr", "pull", "pulls"], &["checkout", "co"]],
            )
        })
    {
        return Some(rest);
    }
    split_ci_words(trimmed, &[&["az"], &["repos"], &["pr"], &["checkout"]])
        .and_then(checkout_command_id)
}

fn parse_number(raw: &str) -> Option<u64> {
    let digits = raw.strip_prefix('#').unwrap_or(raw);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits
        .parse::<u64>()
        .ok()
        .filter(|n| *n >= 1 && *n <= MAX_REQUEST_NUMBER)
}

pub fn parse_pull_request_reference(input: &str) -> Option<String> {
    let trimmed = input.js_trim();
    if trimmed.is_empty() {
        return None;
    }
    let normalized = checkout_command_argument(trimmed).unwrap_or(trimmed);
    if normalized.is_empty() {
        return None;
    }
    if parsed_key(normalized).is_some() {
        return Some(normalized.into());
    }
    parse_number(normalized).map(|n| n.to_string())
}

fn canonicalize_url(raw: &str, key: &ThreadPullRequestKey) -> String {
    if matches!(key.host.0.as_str(), "git.cafe" | "staging.git.cafe")
        && let Ok(mut parsed) = Url::parse(raw)
        && parsed
            .path()
            .trim_matches('/')
            .split('/')
            .nth(2)
            .is_some_and(|seg| matches!(seg, "pull" | "pulls"))
    {
        parsed.set_path(&format!("/{}/pulls/{}", key.repository.0, key.number));
        parsed.set_query(None);
        parsed.set_fragment(None);
        return parsed.into();
    }
    raw.to_owned()
}

pub fn resolve_link_pull_request_input(
    reference: &str,
    project: Option<&PullRequestLinkProject>,
    can_link: impl Fn(&str) -> bool,
) -> PullRequestInputResolution {
    let trimmed = reference.js_trim();
    if trimmed.is_empty() {
        return PullRequestInputResolution::Empty;
    }
    let parsed = if parsed_key(trimmed).is_some() {
        Some(trimmed.to_owned())
    } else {
        parse_pull_request_reference(trimmed)
    };
    let Some(parsed) = parsed else {
        return PullRequestInputResolution::Invalid;
    };
    if let Some((key, _)) = parsed_key(&parsed) {
        if !can_link(&parsed) {
            return PullRequestInputResolution::Error(format!(
                "No project in this environment can read {}/{}.",
                key.host.0, key.repository.0
            ));
        }
        let url = canonicalize_url(&parsed, &key);
        return PullRequestInputResolution::Resolved(ResolvedPullRequestLink {
            host: key.host.0,
            repository: key.repository.0,
            number: key.number,
            url,
        });
    }
    let Some(number) = parse_number(&parsed) else {
        return PullRequestInputResolution::Invalid;
    };
    let Some(project) = project else {
        return PullRequestInputResolution::Error(
            "Paste the full pull request URL; this thread has no repository to resolve the number against."
                .into(),
        );
    };
    let Some(web_url) = project.web_url(number) else {
        return PullRequestInputResolution::Error(format!(
            "Paste a full URL; this project's host has no known {} URL.",
            project.terminology_singular
        ));
    };
    let Some((key, _)) = parsed_key(&web_url) else {
        return PullRequestInputResolution::Error(format!(
            "Paste a full URL; this project's host has no known {} URL.",
            project.terminology_singular
        ));
    };
    PullRequestInputResolution::Resolved(ResolvedPullRequestLink {
        host: key.host.0,
        repository: key.repository.0,
        number: key.number,
        url: web_url,
    })
}

pub(crate) fn build_link_command(
    command_id: t3_contracts::base_schemas::CommandId,
    thread_id: crate::types::ThreadId,
    link: ResolvedPullRequestLink,
) -> t3_contracts::orchestration_v2::OrchestrationV2Command {
    use t3_contracts::orchestration_v2::{CommandThreadPullRequestLink, OrchestrationV2Command};
    OrchestrationV2Command::ThreadPullRequestLink(Box::new(CommandThreadPullRequestLink {
        command_id,
        thread_id,
        host: TrimmedNonEmptyString(link.host),
        repository: TrimmedNonEmptyString(link.repository),
        number: link.number,
        url: TrimmedNonEmptyString(link.url),
        source: ThreadPullRequestLinkSource::Manual,
        repository_web_url: None,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use t3_contracts::{
        environment::{RepositoryIdentityLocator, RepositoryIdentityLocatorSource},
        source_control::SourceControlProviderKind,
    };

    fn github_project() -> PullRequestLinkProject {
        PullRequestLinkProject {
            host: "github.com".into(),
            repository: "acme/web".into(),
            terminology_singular: "pull request",
            provider_kind: "github".into(),
            remote_url: "https://github.com/acme/web.git".into(),
            web_url: Some("https://github.com/acme/web".into()),
        }
    }

    fn identity(
        canonical_key: &str,
        provider: Option<&str>,
        display_name: &str,
        remote_url: &str,
        web_url: Option<&str>,
    ) -> RepositoryIdentity {
        RepositoryIdentity {
            canonical_key: TrimmedNonEmptyString(canonical_key.into()),
            locator: RepositoryIdentityLocator {
                source: RepositoryIdentityLocatorSource::GitRemote,
                remote_name: TrimmedNonEmptyString("origin".into()),
                remote_url: TrimmedNonEmptyString(remote_url.into()),
            },
            web_url: web_url.map(|u| TrimmedNonEmptyString(u.into())),
            root_path: None,
            display_name: Some(TrimmedNonEmptyString(display_name.into())),
            provider: provider.map(|p| SourceControlProviderKind(p.into())),
            owner: None,
            name: None,
            origin: None,
        }
    }

    #[test]
    fn canonicalizes_gitcafe_pull_and_pulls_routes() {
        for host in ["git.cafe", "staging.git.cafe"] {
            for repository in ["web", "pull", "pulls"] {
                for route in ["pull", "pulls"] {
                    let url = format!("https://{host}/acme/{repository}/{route}/42");
                    assert_eq!(parse_pull_request_reference(&url), Some(url.clone()));
                    assert_eq!(
                        resolve_link_pull_request_input(&url, None, |_| true),
                        PullRequestInputResolution::Resolved(ResolvedPullRequestLink {
                            host: host.into(),
                            repository: format!("acme/{repository}"),
                            number: 42,
                            url: format!("https://{host}/acme/{repository}/pulls/42"),
                        })
                    );
                }
            }
        }
    }

    #[test]
    fn links_supported_hosts_and_validates_azure_full_repository() {
        for (url, host) in [
            (
                "https://bitbucket.org/acme/web/pull-requests/42",
                "bitbucket.org",
            ),
            (
                "https://github.acme.test/acme/web/pull/42",
                "github.acme.test",
            ),
            (
                "https://git.acme.test/acme/web/-/merge_requests/42",
                "git.acme.test",
            ),
        ] {
            assert_eq!(
                resolve_link_pull_request_input(&format!(" {url} "), None, |candidate| {
                    parsed_key(candidate).is_some_and(|(k, _)| k.host.0 == host)
                }),
                PullRequestInputResolution::Resolved(ResolvedPullRequestLink {
                    host: host.into(),
                    repository: "acme/web".into(),
                    number: 42,
                    url: url.into(),
                })
            );
        }

        let has_azure = |candidate: &str| {
            parsed_key(candidate).is_some_and(|(k, _)| {
                k.host.0 == "dev.azure.com" && k.repository.0 == "org-a/project/_git/web"
            })
        };
        assert!(matches!(
            resolve_link_pull_request_input(
                "https://dev.azure.com/org-a/project/_git/web/pullrequest/42",
                None,
                has_azure
            ),
            PullRequestInputResolution::Resolved(ResolvedPullRequestLink {
                number: 42,
                repository,
                ..
            }) if repository == "org-a/project/_git/web"
        ));
        assert!(matches!(
            resolve_link_pull_request_input(
                "https://dev.azure.com/org-b/project/_git/web/pullrequest/42",
                None,
                has_azure
            ),
            PullRequestInputResolution::Error(msg) if msg.contains("org-b/project/_git/web")
        ));
    }

    #[test]
    fn resolves_bare_azure_numbers_ssh_and_legacy_urls() {
        let ssh_identity = identity(
            "ssh.dev.azure.com/v3/org/project/web",
            Some("azure-devops"),
            "v3/org/project/web",
            "git@ssh.dev.azure.com:v3/org/project/web",
            None,
        );
        let project = pull_request_link_project(Some(&ssh_identity)).expect("azure project");
        assert_eq!(project.provider_kind, "azure-devops");
        assert_eq!(
            resolve_link_pull_request_input("#42", Some(&project), |_| true),
            PullRequestInputResolution::Resolved(ResolvedPullRequestLink {
                host: "dev.azure.com".into(),
                repository: "org/project/_git/web".into(),
                number: 42,
                url: "https://dev.azure.com/org/project/_git/web/pullrequest/42".into(),
            })
        );

        let vs_identity = identity(
            "acme.visualstudio.com/DefaultCollection/project/_git/web",
            Some("azure-devops"),
            "DefaultCollection/project/_git/web",
            "https://acme.visualstudio.com/DefaultCollection/project/_git/web",
            None,
        );
        let vs_project = pull_request_link_project(Some(&vs_identity)).expect("vs project");
        assert_eq!(
            vs_project.web_url(42).as_deref(),
            Some("https://dev.azure.com/acme/project/_git/web/pullrequest/42")
        );

        for legacy in [
            "https://acme.visualstudio.com/project/_git/t3code/pullrequest/42",
            "https://acme.visualstudio.com/DefaultCollection/project/_git/t3code/pullrequest/42",
        ] {
            assert_eq!(parse_pull_request_reference(legacy), Some(legacy.into()));
        }
    }

    #[test]
    fn handles_forgejo_ports_unknown_providers_and_terminology_errors() {
        let forgejo_id = identity(
            "forge.example/team/repo",
            Some("forgejo"),
            "team/repo",
            "https://forge.example:3000/team/repo.git",
            None,
        );
        let forgejo = pull_request_link_project(Some(&forgejo_id)).expect("forgejo project");
        assert_eq!(forgejo.host, "forge.example:3000");
        assert_eq!(
            forgejo.web_url(12).as_deref(),
            Some("https://forge.example:3000/team/repo/pulls/12")
        );

        let cursor_id = identity(
            "origin.cursor.com/acme/web",
            Some("cursor-origin"),
            "acme/web",
            "https://origin.cursor.com/acme/web.git",
            None,
        );
        let cursor = pull_request_link_project(Some(&cursor_id)).expect("cursor project");
        assert_eq!(
            cursor.web_url(5).as_deref(),
            Some("https://cursor.com/codebase/acme/web/pull/5")
        );

        let unknown_id = identity(
            "git.internal/acme/web",
            None,
            "acme/web",
            "https://git.internal/acme/web.git",
            Some("https://git.internal/acme/web"),
        );
        let unknown = pull_request_link_project(Some(&unknown_id)).expect("unknown project");
        assert_eq!(unknown.host, "git.internal");
        assert_eq!(unknown.web_url(42), None);
        assert!(matches!(
            resolve_link_pull_request_input("#42", Some(&unknown), |_| true),
            PullRequestInputResolution::Error(msg)
                if msg == "Paste a full URL; this project's host has no known pull request URL."
        ));

        let gitlab = PullRequestLinkProject {
            terminology_singular: "merge request",
            provider_kind: "unknown".into(),
            ..github_project()
        };
        assert_eq!(
            resolve_link_pull_request_input("#42", Some(&gitlab), |_| true),
            PullRequestInputResolution::Error(
                "Paste a full URL; this project's host has no known merge request URL.".into()
            )
        );
    }

    #[test]
    fn parses_checkout_commands_and_rejects_trailing_flags_where_ts_does() {
        let project = github_project();
        assert_eq!(
            parse_pull_request_reference("gh pr checkout 42"),
            Some("42".into())
        );
        assert_eq!(
            parse_pull_request_reference("GH  PR   CHECKOUT   42"),
            Some("42".into())
        );
        assert_eq!(
            parse_pull_request_reference("glab mr checkout 42"),
            Some("42".into())
        );
        assert_eq!(
            parse_pull_request_reference("origin pr checkout 42"),
            Some("42".into())
        );
        assert_eq!(
            parse_pull_request_reference("tea pr checkout 42"),
            Some("42".into())
        );
        assert_eq!(
            parse_pull_request_reference("tea pull co 42"),
            Some("42".into())
        );
        assert_eq!(
            parse_pull_request_reference("tea pulls co 42"),
            Some("42".into())
        );
        assert_eq!(
            parse_pull_request_reference("az repos pr checkout --id=42"),
            Some("42".into())
        );
        assert_eq!(
            parse_pull_request_reference("az repos pr checkout --id 42 --remote-name origin"),
            Some("42".into())
        );
        assert_eq!(
            parse_pull_request_reference("az repos pr checkout -i 42 --detect"),
            Some("42".into())
        );
        // Matches TS CHECKOUT_COMMAND: extra flags after `gh pr checkout 99` fail `^#?(\d+)$`.
        assert_eq!(
            parse_pull_request_reference("gh pr checkout 99 --force"),
            None
        );
        assert_eq!(parse_pull_request_reference("feature/my-branch"), None);
        assert!(matches!(
            resolve_link_pull_request_input("#42", None, |_| true),
            PullRequestInputResolution::Error(msg) if msg.contains("no repository")
        ));
        assert_eq!(
            resolve_link_pull_request_input("   ", Some(&project), |_| true),
            PullRequestInputResolution::Empty
        );
    }
}
