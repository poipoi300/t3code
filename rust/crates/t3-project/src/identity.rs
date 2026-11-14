use crate::Result;
use std::{collections::BTreeMap, path::Path, sync::Arc, time::Duration};
use t3_contracts::environment::RepositoryIdentity;
use t3_git::{ExecuteOptions, Git};
use tokio::sync::Mutex;
use tokio::time::Instant;

use t3_orchestration::ports::decisions::thread_command_executor::KeyedLanes;

struct Cached<V> {
    value: V,
    until: Instant,
    touched: u64,
}
struct TimedCache<K, V> {
    entries: BTreeMap<K, Cached<V>>,
    serial: u64,
    capacity: usize,
}
impl<K, V> Default for TimedCache<K, V> {
    fn default() -> Self {
        Self {
            entries: BTreeMap::new(),
            serial: 0,
            capacity: 512,
        }
    }
}
impl<K: Ord + Clone, V: Clone> TimedCache<K, V> {
    fn get(&mut self, key: &K) -> Option<V> {
        self.serial += 1;
        let entry = self.entries.get_mut(key)?;
        if entry.until <= Instant::now() {
            return None;
        }
        entry.touched = self.serial;
        Some(entry.value.clone())
    }
    fn put(&mut self, key: K, value: V, ttl: u64) {
        if !self.entries.contains_key(&key)
            && self.entries.len() >= self.capacity
            && let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.touched)
                .map(|(k, _)| k.clone())
        {
            self.entries.remove(&oldest);
        }
        self.serial += 1;
        self.entries.insert(
            key,
            Cached {
                value,
                until: Instant::now() + Duration::from_secs(ttl),
                touched: self.serial,
            },
        );
    }
}
type IdentityCache = TimedCache<(String, bool), Option<RepositoryIdentity>>;
type RootCache = TimedCache<String, Option<(String, bool)>>;

#[async_trait::async_trait]
pub trait IdentityRefiner: Send + Sync {
    async fn refine(&self, identity: &RepositoryIdentity) -> Result<RepositoryIdentity>;
}

#[derive(Clone)]
pub struct RepositoryIdentityResolver {
    cache: Arc<Mutex<IdentityCache>>,
    roots: Arc<Mutex<RootCache>>,
    lanes: Arc<KeyedLanes>,
    refiner: Option<Arc<dyn IdentityRefiner>>,
    environment: t3_process::Environment,
    git: Option<Git>,
    capacity: usize,
}
impl Default for RepositoryIdentityResolver {
    fn default() -> Self {
        Self {
            cache: Default::default(),
            roots: Default::default(),
            lanes: Default::default(),
            refiner: None,
            environment: Default::default(),
            git: None,
            capacity: 512,
        }
    }
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
    pub fn with_git(mut self, git: Git) -> Self {
        self.git = Some(git);
        let capacity = self.capacity;
        self.with_cache_capacity(capacity)
    }
    fn git_at(&self, cwd: &str) -> Git {
        self.git
            .as_ref()
            .map_or_else(|| Git::new(cwd), |git| git.at(cwd))
            .with_environment(self.environment.clone())
    }
    pub fn with_environment(mut self, environment: t3_process::Environment) -> Self {
        self.environment = environment;
        let capacity = self.capacity;
        self.with_cache_capacity(capacity)
    }
    pub fn with_cache_capacity(mut self, capacity: usize) -> Self {
        self.capacity = capacity.max(1);
        self.cache = Arc::new(Mutex::new(TimedCache {
            capacity: capacity.max(1),
            ..Default::default()
        }));
        self.roots = Arc::new(Mutex::new(TimedCache {
            capacity: capacity.max(1),
            ..Default::default()
        }));
        self
    }
    pub fn with_refiner(mut self, refiner: Arc<dyn IdentityRefiner>) -> Self {
        self.refiner = Some(refiner);
        self.cache = Arc::new(Mutex::new(TimedCache {
            capacity: self.capacity,
            ..Default::default()
        }));
        self
    }
    pub async fn resolve(&self, cwd: &str, refresh: bool) -> Option<RepositoryIdentity> {
        self.try_resolve(cwd, refresh).await.ok().flatten()
    }

    async fn probe_root(&self, cwd: &str) -> Result<Option<(String, bool)>> {
        if t3_workspace::paths::normalize_root(cwd, Path::new(cwd), false)
            .await
            .is_err()
        {
            return Ok(None);
        }
        let git = self.git_at(cwd);
        let options = || ExecuteOptions {
            allow_nonzero: true,
            ..Default::default()
        };
        let top = git
            .execute(&["rev-parse", "--show-toplevel"], options())
            .await?;
        let found = if top.code == Some(0) {
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
        Ok((!found.0.is_empty()).then_some(found))
    }
    async fn try_resolve(&self, cwd: &str, refresh: bool) -> Result<Option<RepositoryIdentity>> {
        let _cwd_guard = self.lanes.acquire([format!("cwd:{cwd}")]).await;
        let cwd_key = cwd.to_owned();
        if refresh {
            self.roots.lock().await.entries.remove(&cwd_key);
        }
        let cached = self.roots.lock().await.get(&cwd_key);
        let found = match cached {
            Some(value) => value,
            None => {
                let value = self.probe_root(cwd).await?;
                self.roots.lock().await.put(
                    cwd_key,
                    value.clone(),
                    if value.is_some() { 900 } else { 60 },
                );
                value
            }
        };
        let Some((root, bare)) = found else {
            return Ok(None);
        };
        let _root_guard = self
            .lanes
            .acquire([format!("repository:{root}:{bare}")])
            .await;
        let key = (root.clone(), bare);
        let mut cache = self.cache.lock().await;
        if refresh {
            cache.entries.remove(&key);
        }
        if let Some(value) = cache.get(&key) {
            return Ok(value);
        }
        drop(cache);
        let git = self.git_at(&root);
        let options = || ExecuteOptions {
            allow_nonzero: true,
            ..Default::default()
        };
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
            let safe_url = crate::redaction::remote_url(&url);
            let key = t3_git::remote_repository_key(&safe_url);
            let remote=t3_forge::remote::parse_remote(&safe_url);
            let display=remote.as_ref().and_then(|r|r.repository.as_deref());
            let canonical_path=key.split_once('/').map(|(_,p)|p).unwrap_or("");
            let repository_path=display.filter(|p|p.to_lowercase()==canonical_path).unwrap_or(canonical_path);
            let origin = remotes.get("origin").filter(|url| t3_git::remote_repository_key(&crate::redaction::remote_url(url)) != key).map(|url| {
                let safe_url = crate::redaction::remote_url(url);
                let key = t3_git::remote_repository_key(&safe_url);
                let canonical = key.split_once('/').map(|(_,p)|p).unwrap_or("");
                let display = t3_forge::remote::parse_remote(&safe_url).and_then(|r|r.repository)
                    .filter(|path|path.to_lowercase()==canonical).unwrap_or_else(||canonical.into());
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
        let identity =
            if let (Some(identity), Some(refiner)) = (identity.as_ref(), self.refiner.as_ref()) {
                Some(
                    refiner
                        .refine(identity)
                        .await
                        .unwrap_or_else(|_| identity.clone()),
                )
            } else {
                identity
            };
        let identity = identity.map(crate::redaction::identity);
        let ttl = if identity.is_some() { 900 } else { 60 };
        self.cache.lock().await.put(key, identity.clone(), ttl);
        Ok(identity)
    }
}
