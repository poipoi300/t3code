#![allow(clippy::unwrap_used)]
mod support;
use std::sync::Arc;
use support::{Fixture, git_environment};
use t3_git::{ExecuteOptions, Git};
use t3_project::{enrichment::*, identity::RepositoryIdentityResolver};

#[tokio::test]
async fn remembered_identity_survives_cache_eviction_by_a_remote_less_folder() {
    let f = Fixture::new();
    let root = f.root("repo");
    let empty = f.root("no-remote");
    let git = Git::new(&root).with_environment(git_environment(f.temp.path()));
    for args in [
        vec!["init"],
        vec!["remote", "add", "origin", "https://github.com/Fixture/Repo.git"],
    ] {
        git.execute(&args, ExecuteOptions::default()).await.unwrap();
    }
    let enrichment = ProjectEnrichmentService::with_options(
        RepositoryIdentityResolver::default().with_environment(git_environment(f.temp.path())),
        Arc::new(t3_workspace::project::FaviconResolver::new(f.temp.path().into())),
        EnrichmentOptions { cache_capacity: 1, ..Default::default() },
    );
    let root = root.to_string_lossy();
    let identity = enrichment.read_repository_identity(&root).await.unwrap();
    assert!(enrichment.read_repository_identity(&empty.to_string_lossy()).await.is_none());
    let available = enrichment.peek(&root).await;
    assert_eq!(available.repository_identity.unwrap().canonical_key, identity.canonical_key);
    assert!(!available.repository_identity_resolved);
    enrichment.invalidate([root.to_string()]).await;
    assert!(enrichment.peek(&root).await.repository_identity.is_none());
    enrichment.shutdown().await;
    f.shutdown().await;
}
