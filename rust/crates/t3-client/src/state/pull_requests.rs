use t3_contracts::{
    base_schemas::TrimmedNonEmptyString,
    orchestration_v2::OrchestrationV2AppThread,
    thread_pull_request::{
        ThreadLinkedPullRequest, ThreadPullRequestKey, ThreadPullRequestLink,
        ThreadPullRequestLinkSource,
    },
};
use url::Url;

fn canonical(host: &str, repository: &str, number: u64) -> ThreadPullRequestKey {
    let mut host = host.trim().to_lowercase();
    let mut repository = repository.trim().to_lowercase();
    let parts = repository.split('/').collect::<Vec<_>>();
    if matches!(
        host.as_str(),
        "ssh.dev.azure.com" | "vs-ssh.visualstudio.com"
    ) && parts.len() == 4
        && parts[0] == "v3"
    {
        repository = format!("{}/{}/_git/{}", parts[1], parts[2], parts[3]);
        host = "dev.azure.com".into();
    } else if let Some(organization) = host
        .strip_suffix(".visualstudio.com")
        .filter(|org| !org.contains('.'))
    {
        let path = repository
            .strip_prefix("defaultcollection/")
            .unwrap_or(&repository);
        let parts = path.split('/').collect::<Vec<_>>();
        if parts.len() == 3 && parts[1] == "_git" {
            repository = format!("{organization}/{path}");
            host = "dev.azure.com".into();
        }
    }
    ThreadPullRequestKey {
        host: TrimmedNonEmptyString(host),
        repository: TrimmedNonEmptyString(repository),
        number,
    }
}

fn parsed_key(value: &str) -> Option<(ThreadPullRequestKey, bool)> {
    let url = Url::parse(value).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let host = url.host_str()?.to_lowercase();
    let parts = url.path().strip_prefix('/')?.split('/').collect::<Vec<_>>();
    let (repository, number, authority) =
        if matches!(host.as_str(), "cursor.com" | "www.cursor.com")
            && parts.len() >= 5
            && parts[0] == "codebase"
            && parts[3] == "pull"
        {
            return Some((
                canonical(
                    "origin.cursor.com",
                    &parts[1..3].join("/"),
                    request_number(parts[4])?,
                ),
                false,
            ));
        } else if let Some(index) = parts
            .windows(2)
            .position(|parts| parts == ["-", "merge_requests"])
        {
            (parts[..index].join("/"), *parts.get(index + 2)?, false)
        } else if let Some(index) = parts.iter().position(|part| *part == "pullrequest") {
            if index < 2 || parts[index - 2] != "_git" {
                return None;
            }
            (
                parts[..index].join("/"),
                *parts.get(index + 1)?,
                url.port().is_some(),
            )
        } else if let Some(index) = parts
            .iter()
            .rposition(|part| *part == "pulls")
            .filter(|index| *index >= 2)
        {
            (
                parts[..index].join("/"),
                *parts.get(index + 1)?,
                index != 2 || !matches!(host.as_str(), "git.cafe" | "staging.git.cafe"),
            )
        } else if parts.len() >= 4 && matches!(parts[2], "pull" | "pull-requests") {
            (parts[..2].join("/"), parts[3], false)
        } else {
            return None;
        };
    let number = request_number(number)?;
    let host = if authority {
        url.authority().rsplit('@').next()?.to_lowercase()
    } else {
        host
    };
    Some((canonical(&host, &repository, number), authority))
}

fn request_number(value: &str) -> Option<u64> {
    value
        .parse::<u64>()
        .ok()
        .filter(|n| *n > 0 && *n <= 9_007_199_254_740_991)
}

pub fn normalize_key(link: &ThreadPullRequestLink) -> ThreadPullRequestKey {
    let parsed = parsed_key(&link.url.0);
    let host = parsed
        .as_ref()
        .filter(|(key, authority)| {
            *authority
                && key.repository.0 == link.repository.0.trim().to_lowercase()
                && key.number == link.number
        })
        .map_or(link.host.0.as_str(), |(key, _)| key.host.0.as_str());
    canonical(host, &link.repository.0, link.number)
}

pub fn legacy_key(link: &ThreadLinkedPullRequest) -> ThreadPullRequestKey {
    if let Some((key, authority)) = parsed_key(&link.url.0)
        && key.number == link.number
        && (authority || key.host.0 == "dev.azure.com")
    {
        return key;
    }
    let host = Url::parse(&link.url.0)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_else(|| "unknown".into());
    canonical(&host, &link.repository.0, link.number)
}

pub fn links(thread: &OrchestrationV2AppThread) -> Vec<ThreadPullRequestLink> {
    if let Some(Some(links)) = &thread.pull_requests {
        return links.clone();
    }
    if thread.pull_requests.is_some() {
        return Vec::new();
    }
    let Some(linked) = thread.linked_pull_request.as_ref().and_then(Option::as_ref) else {
        return Vec::new();
    };
    let key = legacy_key(linked);
    vec![ThreadPullRequestLink {
        host: key.host,
        repository: key.repository,
        number: key.number,
        url: linked.url.clone(),
        source: ThreadPullRequestLinkSource::Manual,
        linked_at: t3_contracts::wire::WireDateTime("1970-01-01T00:00:00.000Z".into()),
        snapshot: None,
        stack: None,
        watch: None,
        ended_watch: None,
        repository_web_url: None,
    }]
}

pub fn apply_update(thread: &mut OrchestrationV2AppThread, updated: ThreadPullRequestLink) {
    let key = normalize_key(&updated);
    let mut links = links(thread);
    for link in &mut links {
        if normalize_key(link) == key {
            *link = updated.clone();
        }
    }
    thread.pull_requests = Some(Some(links));
}
