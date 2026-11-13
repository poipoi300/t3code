use crate::{Git, GitError, Repository, core::Cache};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use t3_contracts::{base_schemas::TrimmedNonEmptyString as Text, vcs::*};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum VcsSelection {
    #[default]
    Auto,
    Kind(VcsDriverKind),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectConfigWarning {
    pub operation: &'static str,
    pub cwd: PathBuf,
    pub config_path: PathBuf,
}

pub async fn resolve_vcs_kind(
    cwd: &Path,
    requested: VcsSelection,
) -> (VcsSelection, Vec<ProjectConfigWarning>) {
    if requested != VcsSelection::Auto {
        return (requested, vec![]);
    }
    let mut warnings = Vec::new();
    for directory in cwd.ancestors() {
        let config_path = directory.join(".t3code/vcs.json");
        match config_path.try_exists() {
            Err(_) => {
                warnings.push(ProjectConfigWarning {
                    operation: "inspect",
                    cwd: cwd.into(),
                    config_path,
                });
                continue;
            }
            Ok(false) => continue,
            Ok(true) => {}
        }
        let raw = match tokio::fs::read_to_string(&config_path).await {
            Ok(raw) => raw,
            Err(_) => {
                warnings.push(ProjectConfigWarning {
                    operation: "read",
                    cwd: cwd.into(),
                    config_path,
                });
                return (VcsSelection::Auto, warnings);
            }
        };
        #[derive(serde::Deserialize)]
        struct Nested {
            kind: Option<VcsDriverKind>,
        }
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Config {
            vcs: Option<Nested>,
            vcs_kind: Option<VcsDriverKind>,
        }
        let config: Result<Config, _> = serde_json::from_str(&lenient_json(&raw));
        match config {
            Ok(config) => {
                return (
                    config
                        .vcs
                        .and_then(|vcs| vcs.kind)
                        .or(config.vcs_kind)
                        .map_or(VcsSelection::Auto, VcsSelection::Kind),
                    warnings,
                );
            }
            Err(_) => {
                warnings.push(ProjectConfigWarning {
                    operation: "decode",
                    cwd: cwd.into(),
                    config_path,
                });
                return (VcsSelection::Auto, warnings);
            }
        }
    }
    (VcsSelection::Auto, warnings)
}
fn lenient_json(input: &str) -> String {
    let Ok(comments) = regex::Regex::new(r#"("(?:[^"\\]|\\.)*")|//[^\r\n]*|/\*[\s\S]*?\*/"#) else {
        return input.into();
    };
    let stripped = comments.replace_all(input, |captures: &regex::Captures<'_>| {
        captures
            .get(1)
            .map_or(" ", |literal| literal.as_str())
            .to_owned()
    });
    let Ok(commas) = regex::Regex::new(r#"("(?:[^"\\]|\\.)*")|,(\s*[}\]])"#) else {
        return stripped.into_owned();
    };
    commas
        .replace_all(&stripped, |captures: &regex::Captures<'_>| {
            captures
                .get(1)
                .or_else(|| captures.get(2))
                .map_or("", |literal| literal.as_str())
                .to_owned()
        })
        .into_owned()
}
#[derive(Clone)]
struct Detection {
    repository: Repository,
    git_entries: Vec<bool>,
}
pub struct VcsDriverRegistry {
    git: Git,
    cache: Cache<Option<Detection>>,
}
impl VcsDriverRegistry {
    pub fn new(git: Git) -> Self {
        Self {
            git,
            cache: Cache::new(2048),
        }
    }
    pub fn get(&self, kind: VcsDriverKind) -> Result<&Git, GitError> {
        if kind == VcsDriverKind::Git {
            return Ok(&self.git);
        }
        let label = if kind == VcsDriverKind::Jj {
            "jj"
        } else {
            "unknown"
        };
        Err(crate::VcsFailure::unsupported(
            "VcsDriverRegistry.get",
            kind,
            &format!("No {label} VCS driver is registered."),
        )
        .into())
    }
    pub async fn detect(
        &self,
        cwd: &Path,
        requested: VcsSelection,
    ) -> Result<Option<VcsRepositoryIdentity>, GitError> {
        let (selection, _) = resolve_vcs_kind(cwd, requested).await;
        if let VcsSelection::Kind(kind) = &selection
            && *kind != VcsDriverKind::Unknown
        {
            self.get(kind.clone())?;
        }
        let key = format!("{selection:?}\0{}", cwd.display());
        let git = self.git.at(cwd);
        let read = || async {
            let repository = match git.detect_repository().await {
                Ok(repository) => repository,
                Err(GitError::Exit { .. } | GitError::Vcs(_)) => None,
                Err(error) => return Err(error),
            };
            Ok(repository.map(|repository| Detection {
                git_entries: git_entries(cwd, &repository.root),
                repository,
            }))
        };
        let mut detected = self
            .cache
            .get(&key, Duration::from_secs(300), Duration::ZERO, read)
            .await?;
        if let Some(cached) = &detected {
            if !cwd.exists()
                || !cached.repository.common_dir.exists()
                || git_entries(cwd, &cached.repository.root) != cached.git_entries
            {
                self.cache.invalidate(&key).await;
                detected = self
                    .cache
                    .get(&key, Duration::from_secs(300), Duration::ZERO, read)
                    .await?;
            }
        } else {
            self.cache.invalidate(&key).await;
        }
        Ok(detected.map(|detected| VcsRepositoryIdentity {
            kind: VcsDriverKind::Git,
            root_path: Text(detected.repository.root.to_string_lossy().into_owned()),
            metadata_path: Some(Text(
                detected
                    .repository
                    .common_dir
                    .to_string_lossy()
                    .into_owned(),
            )),
            freshness: VcsFreshness {
                source: VcsFreshnessSource::LiveLocal,
                observed_at: t3_contracts::wire::WireDateTime(
                    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                ),
                expires_at: WireOption::None,
            },
        }))
    }
}
fn git_entries(cwd: &Path, root: &Path) -> Vec<bool> {
    if !cwd.starts_with(root) {
        return vec![cwd.join(".git").exists()];
    }
    cwd.ancestors()
        .take_while(|path| *path != root)
        .map(|path| path.join(".git").exists())
        .collect()
}
