use crate::{
    FailureReason, Forge, ForgeError, Result, forge::CheckoutContext, remote::select_context,
};
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
type DetectionKey = (PathBuf, Vec<(String, u64)>);
type DetectionCache = Mutex<HashMap<DetectionKey, (Instant, Option<CheckoutContext>)>>;
#[derive(Default)]
pub struct ForgeRegistry {
    forges: BTreeMap<String, Arc<dyn Forge>>,
    git_environment: t3_process::Environment,
    cache: DetectionCache,
}
pub struct ForgeHandle {
    pub forge: Arc<dyn Forge>,
    pub context: Option<CheckoutContext>,
}
impl ForgeRegistry {
    pub async fn shutdown(&self) {
        futures_util::future::join_all(self.forges.values().map(|forge| forge.shutdown())).await;
    }
    pub fn registered(&self) -> impl Iterator<Item = &Arc<dyn Forge>> {
        self.forges.values()
    }
    pub fn invalidate_detection(&self) {
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
    pub fn with_git_environment(mut self, environment: t3_process::Environment) -> Self {
        self.git_environment = environment;
        self
    }
    pub fn new(forges: impl IntoIterator<Item = Arc<dyn Forge>>) -> Self {
        Self {
            forges: forges.into_iter().map(|f| (f.kind().0, f)).collect(),
            git_environment: Default::default(),
            cache: Default::default(),
        }
    }
    pub fn get(&self, kind: &str) -> Result<Arc<dyn Forge>> {
        if let Some(forge) = self.forges.get(kind) {
            return Ok(forge.clone());
        }
        Ok(Arc::new(UnavailableForge::new(kind)?))
    }

    pub fn resolve_remotes(
        &self,
        remotes: &[(String, String)],
        context: Option<CheckoutContext>,
    ) -> Result<ForgeHandle> {
        let context = context.or_else(|| select_context(remotes));
        let kind = context
            .as_ref()
            .map_or("unknown", |c| c.provider.kind.0.as_str());
        Ok(ForgeHandle {
            forge: self.get(kind)?,
            context,
        })
    }
    pub async fn resolve(
        &self,
        cwd: &Path,
        context: Option<CheckoutContext>,
    ) -> Result<ForgeHandle> {
        if context.is_some() {
            return self.resolve_remotes(&[], context);
        }
        let cache_key = (
            cwd.to_path_buf(),
            self.forges
                .iter()
                .map(|(kind, forge)| (kind.clone(), forge.settings_revision()))
                .collect(),
        );
        if let Some((_, context)) = self
            .cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&cache_key)
            .filter(|(created, _)| created.elapsed() < Duration::from_secs(5))
        {
            return self.resolve_remotes(&[], context.clone());
        }
        let git = t3_git::Git::new(cwd).with_environment(self.git_environment.clone());
        let context = self.detect(&git).await?;
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if cache.len() >= 2048
            && let Some(oldest) = cache
                .iter()
                .min_by_key(|(_, (at, _))| *at)
                .map(|(key, _)| key.clone())
        {
            cache.remove(&oldest);
        }
        cache.insert(cache_key, (Instant::now(), context.clone()));
        self.resolve_remotes(&[], context)
    }
    pub async fn resolve_fresh(&self, git: &t3_git::Git) -> Result<ForgeHandle> {
        self.resolve_remotes(&[], self.detect(git).await?)
    }

    async fn detect(&self, git: &t3_git::Git) -> Result<Option<CheckoutContext>> {
        let cwd = git.cwd();
        let output = git
            .execute(&["remote", "-v"], t3_git::ExecuteOptions::default())
            .await
            .map_err(|_| {
                ForgeError::provider(
                    "unknown",
                    "detectProvider",
                    FailureReason::Failed,
                    "Failed to detect source control provider.",
                )
            })?;
        let mut context = select_context(&parse_fetch_remotes(&output.stdout_text()));
        if let Some(unknown) = context.as_ref().filter(|c| c.provider.kind.0 == "unknown") {
            for forge in self.forges.values() {
                if let Some(provider) = forge
                    .refine_unknown_remote(crate::forge::CheckoutInput {
                        cwd: cwd.into(),
                        context: Some(unknown.clone()),
                    })
                    .await?
                {
                    context = Some(CheckoutContext {
                        provider,
                        ..unknown.clone()
                    });
                    break;
                }
            }
        }
        Ok(context)
    }
}
pub fn parse_fetch_remotes(value: &str) -> Vec<(String, String)> {
    value
        .lines()
        .filter_map(|line| {
            let parts: Vec<_> = line.split_whitespace().collect();
            if parts.len() < 3 || parts[2] != "(fetch)" {
                return None;
            }
            Some((parts[0].into(), parts[1].into()))
        })
        .collect()
}

struct UnavailableForge {
    kind: crate::SourceControlProviderKind,
    capabilities: crate::PullRequestCapabilities,
}
impl UnavailableForge {
    fn new(kind: &str) -> Result<Self> {
        let capabilities = serde_json::from_value(serde_json::json!({"diff":false,"comment":false,"actions":[],"mergeMethods":[],"search":false,"review":{"inlineComment":false,"reply":false,"resolve":false,"verdicts":[]},"reviewers":{"request":false,"listCandidates":false}})).map_err(|_|ForgeError::provider(kind,"resolve",FailureReason::Failed,"Invalid fallback capabilities."))?;
        Ok(Self {
            kind: crate::SourceControlProviderKind(kind.into()),
            capabilities,
        })
    }
}
#[async_trait::async_trait]
impl Forge for UnavailableForge {
    fn kind(&self) -> crate::SourceControlProviderKind {
        self.kind.clone()
    }
    fn capabilities(&self) -> &crate::PullRequestCapabilities {
        &self.capabilities
    }
    fn supports(&self, operation: &str) -> bool {
        self.kind.0 == "unknown" && operation == "checkout_list_change_requests"
    }
    async fn checkout_list_change_requests(
        &self,
        _input: crate::forge::CheckoutListInput,
    ) -> Result<Vec<crate::ChangeRequest>> {
        if self.kind.0 == "unknown" {
            Ok(vec![])
        } else {
            Err(ForgeError::unsupported(
                &self.kind,
                "checkout_list_change_requests",
            ))
        }
    }
}
