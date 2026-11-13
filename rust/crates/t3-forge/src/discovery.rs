use crate::{FailureReason, ForgeError, Result, registry::ForgeRegistry};
use futures_util::{StreamExt, stream};
use serde_json::json;
use std::{path::PathBuf, sync::Arc, time::Duration};
use t3_contracts::source_control::{SourceControlDiscoveryResult, VcsDiscoveryItem};

pub struct SourceControlDiscovery {
    registry: Arc<ForgeRegistry>,
    cwd: PathBuf,
    environment: t3_process::Environment,
}
impl SourceControlDiscovery {
    pub fn new(registry: Arc<ForgeRegistry>, cwd: PathBuf) -> Self {
        Self {
            registry,
            cwd,
            environment: Default::default(),
        }
    }
    pub fn with_environment(mut self, environment: t3_process::Environment) -> Self {
        self.environment = environment;
        self
    }
    async fn probe(
        &self,
        kind: &str,
        label: &str,
        implemented: bool,
        hint: &str,
    ) -> Result<VcsDiscoveryItem> {
        let mut options = t3_process::ProcessOptions::new(kind).args(["--version"]);
        options.cwd = Some(self.cwd.clone());
        options.env = self.environment.clone();
        options.timeout = Some(Duration::from_secs(5));
        options.max_output_bytes = 8_000;
        let output = t3_process::run(options).await;
        let available = output.as_ref().is_ok_and(|o| o.code == Some(0));
        let version = output.as_ref().ok().filter(|_| available).and_then(|o| {
            o.stdout_text()
                .lines()
                .chain(o.stderr_text().lines())
                .map(str::trim)
                .find(|s| !s.is_empty())
                .map(crate::remote::transport_safe_error_value)
        });
        serde_json::from_value(json!({
            "kind":kind,"label":label,"executable":kind,"implemented":implemented,
            "status":if available {"available"} else {"missing"},
            "version": version.map(|value|json!({"_tag":"Some","value":value})).unwrap_or(json!({"_tag":"None"})),
            "installHint":hint,"detail":if available {json!({"_tag":"None"})} else {json!({"_tag":"Some","value":"The version-control tool could not be started successfully."})}
        })).map_err(|_|ForgeError::provider("unknown", "discovery", FailureReason::Failed, "Invalid version-control discovery response."))
    }
    pub async fn discover(&self) -> Result<SourceControlDiscoveryResult> {
        let git = self.probe(
            "git",
            "Git",
            true,
            "Install Git from https://git-scm.com/downloads or with your package manager.",
        );
        let jj = self.probe(
            "jj",
            "Jujutsu",
            false,
            "Install Jujutsu with `brew install jj` or from https://github.com/jj-vcs/jj.",
        );
        let providers = stream::iter(self.registry.registered())
            .map(|forge| forge.discover(self.cwd.clone()))
            .buffered(4)
            .collect::<Vec<_>>();
        let (git, jj, providers) = tokio::join!(git, jj, providers);
        Ok(SourceControlDiscoveryResult {
            version_control_systems: vec![git?, jj?],
            source_control_providers: providers.into_iter().collect::<Result<Vec<_>>>()?,
        })
    }
}
