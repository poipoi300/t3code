//! Source-control links in a thread's first message as title context (`ThreadTitleLinks.ts`,
//! `SourceControlProviderRegistry.resolveLink`): only `https` GitHub and GitLab issue, pull and
//! merge-request links without credentials are looked up, through the environment's one forge
//! registry, with the forge's own configured credentials. The registry starts after the
//! orchestration runtime, so it is bound once it has started.
use async_trait::async_trait;
use std::{
    path::Path,
    sync::{Arc, OnceLock},
};
use t3_forge::{forge::LinkInput, registry::ForgeRegistry};
use t3_textgen::title_context::{LinkedSubject, TitleLinkResolver};

#[derive(Default)]
pub struct TitleLinks {
    registry: OnceLock<Arc<ForgeRegistry>>,
}

impl TitleLinks {
    pub fn bind(&self, registry: Arc<ForgeRegistry>) {
        let _ = self.registry.set(registry);
    }
}

/// The forge that resolves `url`, mirroring the GitHub and GitLab providers' synchronous guards
/// (anything else is not a lookup at all, as TypeScript's `undefined`).
pub fn link_forge(url: &reqwest::Url) -> Option<&'static str> {
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return None;
    }
    let positive = |value: &str| value.parse::<u64>().is_ok_and(|number| number > 0);
    match url.host_str()? {
        "github.com" => {
            let segments: Vec<_> = url.path_segments()?.collect();
            let name = |part: &str| {
                !part.is_empty()
                    && part != "."
                    && part != ".."
                    && part
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
            };
            (segments.len() >= 4
                && name(segments[0])
                && name(segments[1])
                && matches!(segments[2], "issues" | "pull")
                && positive(segments[3]))
            .then_some("github")
        }
        "gitlab.com" => {
            let (_, tail) = url.path().trim_matches('/').split_once("/-/")?;
            let mut parts = tail.split('/');
            let kind = parts.next().unwrap_or_default();
            let number = parts.next().unwrap_or_default();
            (matches!(kind, "merge_requests" | "issues") && positive(number)).then_some("gitlab")
        }
        _ => None,
    }
}

#[async_trait]
impl TitleLinkResolver for TitleLinks {
    fn supports(&self, _: &Path, url: &reqwest::Url) -> bool {
        self.registry.get().is_some() && link_forge(url).is_some()
    }
    async fn resolve(&self, cwd: &Path, url: &reqwest::Url) -> Result<LinkedSubject, ()> {
        let kind = link_forge(url).ok_or(())?;
        let forge = self.registry.get().ok_or(())?.get(kind).map_err(|_| ())?;
        match forge
            .resolve_link(LinkInput {
                cwd: cwd.to_path_buf(),
                url: url.clone(),
            })
            .await
        {
            Ok(Some(subject)) => Ok(LinkedSubject {
                title: subject.title,
                body: subject.body,
            }),
            _ => Err(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_issue_and_change_request_links_on_the_two_hosts_are_looked_up() {
        let kind = |url: &str| link_forge(&reqwest::Url::parse(url).expect("url"));
        assert_eq!(kind("https://github.com/acme/web/pull/7"), Some("github"));
        assert_eq!(
            kind("https://github.com/acme/web/issues/3#top"),
            Some("github")
        );
        assert_eq!(kind("https://github.com/acme/web/tree/main"), None);
        assert_eq!(kind("https://github.com/acme/web/pull/0"), None);
        assert_eq!(kind("http://github.com/acme/web/pull/7"), None);
        assert_eq!(kind("https://user@github.com/acme/web/pull/7"), None);
        assert_eq!(kind("https://github.com:8443/acme/web/pull/7"), None);
        assert_eq!(
            kind("https://gitlab.com/group/sub/web/-/merge_requests/9"),
            Some("gitlab")
        );
        assert_eq!(kind("https://gitlab.com/group/web/-/tree/main"), None);
        assert_eq!(kind("https://ghe.example.com/acme/web/pull/7"), None);
    }
}
