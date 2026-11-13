use crate::Result;
use std::{collections::BTreeMap, path::Path, sync::Arc, time::Duration};
use t3_contracts::environment::RepositoryIdentity;
use t3_git::{ExecuteOptions, Git};
use tokio::sync::Mutex;
use tokio::time::Instant;

type IdentityCache = BTreeMap<(String, bool), (Option<RepositoryIdentity>, Instant)>;

#[derive(Clone, Default)]
pub struct RepositoryIdentityResolver {
    cache: Arc<Mutex<IdentityCache>>,
    environment: t3_process::Environment,
}

pub fn is_local_remote(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("file:") {
        return true;
    }
    if url.contains("://") {
        return false;
    }
    if url.as_bytes().get(1) == Some(&b':') && matches!(url.as_bytes().get(2), Some(b'/' | b'\\')) {
        return true;
    }
    match (url.find(':'), url.find(['/', '\\'])) {
        (None, _) => true,
        (Some(colon), Some(slash)) => slash < colon,
        _ => false,
    }
}

pub fn parse_fetch_urls(stdout: &str) -> BTreeMap<String, String> {
    let mut remotes = BTreeMap::new();
    for line in stdout.lines() {
        let Some((name, rest)) = line.split_once('\t') else {
            continue;
        };
        let Some((url, suffix)) = rest.rsplit_once(" (fetch)") else {
            continue;
        };
        if name.is_empty()
            || name.contains(char::is_whitespace)
            || url.is_empty()
            || is_local_remote(url)
            || !(suffix.is_empty()
                || suffix.trim().starts_with('[') && suffix.trim().ends_with(']'))
        {
            continue;
        }
        remotes.insert(name.to_owned(), url.to_owned());
    }
    remotes
}

pub fn pick_primary_remote(
    remotes: &BTreeMap<String, String>,
    marks: &str,
) -> Option<(String, String)> {
    let marks: Vec<_> = marks
        .lines()
        .filter_map(|line| {
            let (key, value) = line.trim().split_once(char::is_whitespace)?;
            let name = key.strip_prefix("remote.")?.strip_suffix(".gh-resolved")?;
            remotes.get(name).map(|url| (name, url, value.trim()))
        })
        .collect();
    if let [(name, url, value)] = marks.as_slice() {
        if *value == "base" {
            return Some(((*name).into(), (*url).clone()));
        }
        let key = t3_git::remote_repository_key(url);
        if let Some((host, _)) = key.split_once('/') {
            let wanted = format!("{host}/{}", value.to_lowercase());
            if let Some((name, url)) = remotes
                .iter()
                .find(|(_, url)| t3_git::remote_repository_key(url) == wanted)
            {
                return Some((name.clone(), url.clone()));
            }
        }
    }
    ["upstream", "origin"]
        .into_iter()
        .find_map(|name| remotes.get(name).map(|url| (name.into(), url.clone())))
        .or_else(|| {
            remotes
                .first_key_value()
                .map(|(n, u)| (n.clone(), u.clone()))
        })
}

impl RepositoryIdentityResolver {
    pub fn with_environment(mut self, environment: t3_process::Environment) -> Self {
        self.environment = environment;
        self.cache = Arc::new(Mutex::new(BTreeMap::new()));
        self
    }
    pub async fn resolve(&self, cwd: &str, refresh: bool) -> Option<RepositoryIdentity> {
        self.try_resolve(cwd, refresh).await.ok().flatten()
    }

    async fn try_resolve(&self, cwd: &str, refresh: bool) -> Result<Option<RepositoryIdentity>> {
        let git = Git::new(cwd).with_environment(self.environment.clone());
        let options = || ExecuteOptions {
            allow_nonzero: true,
            ..Default::default()
        };
        let top = git
            .execute(&["rev-parse", "--show-toplevel"], options())
            .await?;
        let (root, bare) = if top.code == Some(0) {
            (
                top.stdout_text().trim_end_matches(['\r', '\n']).to_owned(),
                false,
            )
        } else {
            let mut found = None;
            for git_dir in ["--git-dir=.", "--git-dir=.git"] {
                let out = git
                    .execute(
                        &[
                            git_dir,
                            "rev-parse",
                            "--is-bare-repository",
                            "--absolute-git-dir",
                        ],
                        options(),
                    )
                    .await?;
                let text = out.stdout_text();
                let mut lines = text.lines();
                if out.code == Some(0) && lines.next() == Some("true") {
                    found = lines
                        .next()
                        .filter(|s| !s.is_empty())
                        .map(|s| (s.into(), true));
                    break;
                }
            }
            let Some(found) = found else { return Ok(None) };
            found
        };
        if root.is_empty() {
            return Ok(None);
        }
        let key = (root.clone(), bare);
        let mut cache = self.cache.lock().await;
        if refresh {
            cache.remove(&key);
        }
        if let Some((value, until)) = cache.get(&key)
            && *until > Instant::now()
        {
            return Ok(value.clone());
        }
        drop(cache);
        let git = Git::new(Path::new(&root)).with_environment(self.environment.clone());
        let mut remote_args = vec!["remote", "-v"];
        let mut marks_args = vec!["config", "--get-regexp", "^remote\\..*\\.gh-resolved$"];
        if bare {
            remote_args.insert(0, "--git-dir=.");
            marks_args.insert(0, "--git-dir=.");
        }
        let (remote, marks) = tokio::try_join!(
            git.execute(&remote_args, options()),
            git.execute(&marks_args, options())
        )?;
        let remotes = parse_fetch_urls(&remote.stdout_text());
        let picked = if remote.code == Some(0) {
            pick_primary_remote(&remotes, &marks.stdout_text())
        } else {
            None
        };
        let identity = picked.map(|(name,url)| -> Result<RepositoryIdentity> {
            let key = t3_git::remote_repository_key(&url);
            let remote=t3_forge::remote::parse_remote(&url);
            let display=remote.as_ref().and_then(|r|r.repository.as_deref());
            let canonical_path=key.split_once('/').map(|(_,p)|p).unwrap_or("");
            let repository_path=display.filter(|p|p.to_lowercase()==canonical_path).unwrap_or(canonical_path);
            let origin = remotes.get("origin").filter(|url| t3_git::remote_repository_key(url) != key).map(|url| {
                let key = t3_git::remote_repository_key(url);
                let display = key.split_once('/').map(|(_,p)|p.to_owned());
                serde_json::json!({"canonicalKey":key,"remoteUrl":url,"displayName":display})
            });
            let mut value = serde_json::json!({"canonicalKey":key,"locator":{"source":"git-remote","remoteName":name,"remoteUrl":url},"rootPath":root});
            if let Some(provider)=t3_forge::remote::recognize_remote(&url) {value["provider"]=serde_json::to_value(provider.kind)?;}
            if !repository_path.is_empty() {
                value["displayName"] = repository_path.into();
                value["owner"] = repository_path.split('/').next().unwrap_or_default().into();
                value["name"] = repository_path.rsplit('/').next().unwrap_or_default().into();
            }
            if let Some(origin) = origin { value["origin"] = origin; }
            Ok(serde_json::from_value(value)?)
        }).transpose()?;
        let ttl = if identity.is_some() { 900 } else { 60 };
        let mut cache = self.cache.lock().await;
        if cache.len() >= 512
            && let Some(old) = cache
                .iter()
                .min_by_key(|(_, (_, until))| *until)
                .map(|(k, _)| k.clone())
        {
            cache.remove(&old);
        }
        cache.insert(
            key,
            (identity.clone(), Instant::now() + Duration::from_secs(ttl)),
        );
        Ok(identity)
    }
}
