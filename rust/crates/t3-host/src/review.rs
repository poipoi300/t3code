use crate::{Error, Result, background::Clock};
use async_trait::async_trait;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use t3_contracts::review::*;

#[async_trait]
pub trait ReviewWorkspaces: Send + Sync {
    /// Check the canonical candidate against current canonical workspace roots.
    async fn contains(&self, path: &Path) -> Result<bool>;
}
pub struct ReviewService {
    root: PathBuf,
    known: Arc<dyn ReviewWorkspaces>,
    clock: Arc<dyn Clock>,
}
impl ReviewService {
    pub fn new(root: PathBuf, known: Arc<dyn ReviewWorkspaces>, clock: Arc<dyn Clock>) -> Self {
        Self { root, known, clock }
    }
    async fn assert_workspace(&self, cwd: &str) -> Result<t3_git::Git> {
        let canonical = |path: &Path| match dunce::canonicalize(path) {
            Ok(path) => Ok(path),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => std::path::absolute(path)
                .map(|path| normalize_missing_path(&path))
                .map_err(|_| Error::Io),
            Err(_) => Err(Error::Io),
        };
        let candidate = canonical(Path::new(cwd))?;
        let root = canonical(&self.root)?;
        if !candidate.starts_with(root) && !self.known.contains(&candidate).await? {
            return Err(Error::Refused(
                "review cwd must stay within a configured workspace",
            ));
        }
        Ok(t3_git::Git::new(cwd))
    }
    pub async fn get_branch_commits(
        &self,
        input: &ReviewBranchCommitsInput,
    ) -> Result<ReviewBranchCommitsResult> {
        let git = self.assert_workspace(&input.cwd.0).await?;
        let registry = t3_git::VcsDriverRegistry::new(git.clone());
        if registry
            .detect(Path::new(&input.cwd.0), t3_git::VcsSelection::Auto)
            .await
            .map_err(|_| Error::Service)?
            .is_none()
        {
            return Ok(ReviewBranchCommitsResult {
                commits: vec![],
                truncated: false,
            });
        }
        git.review_branch_commits(input)
            .await
            .map_err(|_| Error::Service)
    }
    pub async fn get_diff_preview(
        &self,
        input: &ReviewDiffPreviewInput,
    ) -> Result<ReviewDiffPreviewResult> {
        let git = self.assert_workspace(&input.cwd.0).await?;
        let registry = t3_git::VcsDriverRegistry::new(git.clone());
        if registry
            .detect(Path::new(&input.cwd.0), t3_git::VcsSelection::Auto)
            .await
            .map_err(|_| Error::Service)?
            .is_none()
        {
            return Ok(ReviewDiffPreviewResult {
                cwd: input.cwd.clone(),
                generated_at: t3_contracts::wire::WireDateTime(
                    chrono::DateTime::from_timestamp_millis(self.clock.now_ms())
                        .ok_or(Error::Timestamp)?
                        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                ),
                sources: vec![],
                incomplete: None,
            });
        }
        git.review_diff_preview(input)
            .await
            .map_err(|_| Error::Service)
    }
    pub async fn get_diff_file_contents(
        &self,
        input: &ReviewDiffFileContentsInput,
    ) -> Result<ReviewDiffFileContentsResult> {
        let git = self.assert_workspace(&input.cwd.0).await?;
        let registry = t3_git::VcsDriverRegistry::new(git.clone());
        if registry
            .detect(Path::new(&input.cwd.0), t3_git::VcsSelection::Auto)
            .await
            .map_err(|_| Error::Service)?
            .is_none()
        {
            return Err(Error::Refused("unchanged diff expansion requires Git"));
        }
        git.review_diff_file_contents(input)
            .await
            .map_err(|_| Error::Service)
    }
}
fn normalize_missing_path(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match normalized.components().next_back() {
                Some(Component::Normal(_)) => {
                    normalized.pop();
                }
                _ if !normalized.has_root() => normalized.push(component.as_os_str()),
                _ => {}
            },
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_path_fallback_removes_parent_components_before_authorization() {
        assert_eq!(
            normalize_missing_path(Path::new("/allowed/missing/../../outside")),
            Path::new("/outside")
        );
    }
}
