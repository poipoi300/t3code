use crate::{
    forge::{CheckoutContext, RefSelector},
    model::*,
};
use serde::{Deserialize, Serialize};
use url::Url;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Remote {
    pub url: String,
    pub host: String,
    pub hostname: String,
    pub repository: Option<String>,
    pub ssh: bool,
}
pub fn parse_remote(value: &str) -> Option<Remote> {
    let value = value.trim();
    if value.is_empty() || value.contains('\\') {
        return None;
    }
    if let Some(url) = Url::parse(value)
        .ok()
        .filter(|u| matches!(u.scheme(), "https" | "http" | "ssh" | "git"))
    {
        let hostname = url.host_str()?.to_lowercase();
        let ssh = url.scheme() == "ssh";
        let host = if ssh {
            hostname.clone()
        } else {
            url.authority().rsplit('@').next()?.to_lowercase()
        };
        return Some(Remote {
            url: value.into(),
            host,
            hostname,
            repository: repository_path(url.path()),
            ssh,
        });
    }
    if value.contains("://") {
        return None;
    }
    let (prefix, path) = value.split_once(':')?;
    let host = prefix.rsplit('@').next()?;
    if host.len() < 2
        || host.contains('/')
        || host.contains(char::is_whitespace)
        || path.starts_with(':')
    {
        return None;
    }
    Some(Remote {
        url: value.into(),
        host: host.to_lowercase(),
        hostname: host.to_lowercase(),
        repository: repository_path(path),
        ssh: true,
    })
}
fn repository_path(path: &str) -> Option<String> {
    let path = path.trim_matches('/');
    let path = if path.to_lowercase().ends_with(".git") {
        &path[..path.len() - 4]
    } else {
        path
    };
    (path.split('/').count() >= 2 && !path.contains(char::is_whitespace) && !path.is_empty())
        .then(|| path.into())
}
pub fn recognize_remote(value: &str) -> Option<SourceControlProviderInfo> {
    let remote = parse_remote(value)?;
    let h = remote.hostname.as_str();
    let labels: Vec<_> = h.split('.').collect();
    let (kind, name) = if matches!(h, "git.cafe" | "staging.git.cafe") {
        ("gitcafe", "GitCafe")
    } else if h == "origin.cursor.com" {
        ("cursor-origin", "Cursor Origin")
    } else if h == "codeberg.org" || labels.contains(&"forgejo") || labels.contains(&"gitea") {
        ("forgejo", "Forgejo")
    } else if h == "github.com" || h.ends_with(".ghe.com") || labels.contains(&"github") {
        (
            "github",
            if h == "github.com" {
                "GitHub"
            } else {
                "GitHub Self-Hosted"
            },
        )
    } else if h == "gitlab.com" || labels.contains(&"gitlab") {
        (
            "gitlab",
            if h == "gitlab.com" {
                "GitLab"
            } else {
                "GitLab Self-Hosted"
            },
        )
    } else if h == "dev.azure.com"
        || h.ends_with(".dev.azure.com")
        || h.ends_with(".visualstudio.com")
    {
        ("azure-devops", "Azure DevOps")
    } else if h == "bitbucket.org" || labels.contains(&"bitbucket") {
        (
            "bitbucket",
            if h == "bitbucket.org" {
                "Bitbucket"
            } else {
                "Bitbucket Self-Hosted"
            },
        )
    } else {
        ("unknown", h)
    };
    let base_url = if kind == "cursor-origin" {
        "https://cursor.com".into()
    } else if kind == "forgejo" {
        Url::parse(value)
            .ok()
            .filter(|u| matches!(u.scheme(), "http" | "https"))
            .map(|u| u.origin().ascii_serialization())
            .unwrap_or_else(|| format!("https://{}", remote.host))
    } else {
        format!(
            "https://{}",
            if kind == "gitcafe" { h } else { &remote.host }
        )
    };
    serde_json::from_value(serde_json::json!({"kind":kind,"name":name,"baseUrl":base_url})).ok()
}
pub fn select_context(remotes: &[(String, String)]) -> Option<CheckoutContext> {
    let candidates: Vec<_> = remotes
        .iter()
        .filter_map(|(name, url)| {
            Some(CheckoutContext {
                provider: recognize_remote(url)?,
                remote_name: name.clone(),
                remote_url: url.clone(),
                requested_host: None,
            })
        })
        .collect();
    candidates
        .iter()
        .find(|c| c.remote_name == "origin")
        .or_else(|| candidates.iter().find(|c| c.provider.kind.0 != "unknown"))
        .or(candidates.first())
        .cloned()
}
pub fn parse_owner_ref(value: &str) -> Option<RefSelector> {
    let (owner, reference) = value.trim().split_once(':')?;
    if owner.is_empty()
        || owner.contains([':', '/'])
        || owner.contains(char::is_whitespace)
        || reference.trim().is_empty()
    {
        return None;
    }
    Some(RefSelector {
        owner: Some(owner.into()),
        ref_name: reference.trim().into(),
        repository: None,
    })
}
pub fn source_branch(head: &str, source: Option<&RefSelector>) -> String {
    source
        .map(|s| s.ref_name.clone())
        .or_else(|| parse_owner_ref(head).map(|s| s.ref_name))
        .unwrap_or_else(|| head.trim().into())
}
pub fn transport_safe_error_value(value: &str) -> String {
    static URLS: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#"(?i)(?:https?|ssh|git)://[^\s<>"']+"#).expect("diagnostic URL regex")
    });
    let value = URLS.replace_all(value, |captures: &regex::Captures<'_>| {
        let Ok(mut url) = Url::parse(&captures[0]) else {
            return "[redacted URL]".to_string();
        };
        let _ = url.set_username("");
        let _ = url.set_password(None);
        url.set_query(None);
        url.set_fragment(None);
        url.to_string()
    });
    let printable: String = value
        .chars()
        .map(|c| {
            if (c as u32) < 32 || c == '\u{7f}' {
                ' '
            } else {
                c
            }
        })
        .collect();
    let normalized = printable.split_whitespace().collect::<Vec<_>>().join(" ");
    let safe = if let Ok(mut url) = Url::parse(&normalized) {
        let _ = url.set_username("");
        let _ = url.set_password(None);
        url.set_query(None);
        url.set_fragment(None);
        url.to_string()
    } else {
        normalized
    };
    safe.chars().take(256).collect()
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequestLink {
    pub provider: String,
    pub host: String,
    pub authority: String,
    pub repository: String,
    pub number: u64,
    pub repository_url: String,
}
pub fn parse_pull_request_url(value: &str) -> Option<PullRequestLink> {
    let url = Url::parse(value).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    let hostname = url.host_str()?.to_lowercase();
    let authority = url.authority().to_lowercase();
    let path: Vec<_> = url.path_segments()?.collect();
    let cursor = matches!(hostname.as_str(), "cursor.com" | "www.cursor.com");
    let cafe = matches!(hostname.as_str(), "git.cafe" | "staging.git.cafe");
    let (kind, repository, n) =
        if cursor && path.first() == Some(&"codebase") && path.get(3) == Some(&"pull") {
            ("cursor-origin", path.get(1..3)?.join("/"), *path.get(4)?)
        } else if cafe && matches!(path.get(2), Some(&"pull" | &"pulls")) {
            ("gitcafe", path.get(..2)?.join("/"), *path.get(3)?)
        } else if let Some(i) = path
            .iter()
            .position(|s| *s == "pullrequest")
            .filter(|i| *i >= 2 && path[i - 2] == "_git")
        {
            ("azure-devops", path[..i].join("/"), *path.get(i + 1)?)
        } else if let Some(i) = path
            .iter()
            .position(|s| *s == "merge_requests")
            .filter(|i| *i >= 3 && path[i - 1] == "-")
        {
            ("gitlab", path[..i - 1].join("/"), *path.get(i + 1)?)
        } else if path.get(2) == Some(&"pull") {
            ("github", path[..2].join("/"), *path.get(3)?)
        } else if let Some(i) = path.iter().position(|s| *s == "pulls").filter(|i| *i >= 2) {
            ("forgejo", path[..i].join("/"), *path.get(i + 1)?)
        } else if path.get(2) == Some(&"pull-requests") {
            ("bitbucket", path[..2].join("/"), *path.get(3)?)
        } else {
            return None;
        };
    let number: u64 = n.parse().ok()?;
    if number == 0 || number > 9_007_199_254_740_991 || repository.split('/').any(str::is_empty) {
        return None;
    }
    let repository_url = format!(
        "{}/{}{}",
        url.origin().ascii_serialization(),
        if cursor { "codebase/" } else { "" },
        repository
    );
    Some(PullRequestLink {
        provider: kind.into(),
        host: if cursor {
            "origin.cursor.com".into()
        } else {
            hostname
        },
        authority,
        repository: repository.to_lowercase(),
        number,
        repository_url,
    })
}
