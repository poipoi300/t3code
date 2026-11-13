use crate::types::EnvironmentId;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WslUncPath {
    pub distro: String,
    pub linux_path: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WslEnvironmentCandidate {
    pub environment: EnvironmentId,
    pub backend_id: String,
    pub running_distro: Option<String>,
}
#[derive(Debug, Clone)]
pub struct WslEnvironmentConfiguration {
    pub enabled: bool,
    pub wsl_only: bool,
    pub distro: Option<String>,
    pub distros: Vec<(String, bool)>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WslProjectSelection {
    pub environment: EnvironmentId,
    pub path: WslUncPath,
}
pub fn parse_wsl_unc_path(input: &str) -> Option<WslUncPath> {
    let normalized = input.trim().replace('/', "\\");
    let lower = normalized.to_ascii_lowercase();
    let prefix = ["\\\\wsl.localhost\\", "\\\\wsl$\\"]
        .into_iter()
        .find(|p| lower.starts_with(p))?;
    let mut segments = normalized[prefix.len()..]
        .split('\\')
        .filter(|s| !s.is_empty());
    let distro = segments.next()?;
    if !distro
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return None;
    }
    Some(WslUncPath {
        distro: distro.into(),
        linux_path: format!("/{}", segments.collect::<Vec<_>>().join("/")),
    })
}
pub fn resolve_wsl_project_selection(
    input: &str,
    candidates: &[WslEnvironmentCandidate],
) -> Option<WslProjectSelection> {
    let path = parse_wsl_unc_path(input)?;
    let candidate = candidates.iter().find(|candidate| {
        let Some(backend) = candidate.backend_id.strip_prefix("wsl:") else {
            return false;
        };
        candidate
            .running_distro
            .as_deref()
            .or_else(|| {
                (!backend.is_empty() && !backend.eq_ignore_ascii_case("default")).then_some(backend)
            })
            .is_some_and(|d| d.eq_ignore_ascii_case(&path.distro))
    })?;
    Some(WslProjectSelection {
        environment: candidate.environment.clone(),
        path,
    })
}
fn configured_backend(config: &WslEnvironmentConfiguration) -> Option<(String, Option<String>)> {
    let Some(distro) = &config.distro else {
        return Some(("wsl:default".into(), None));
    };
    let distro = distro.trim();
    if distro.is_empty() {
        return None;
    }
    let resolved = config
        .distros
        .iter()
        .find(|(d, _)| d.eq_ignore_ascii_case(distro))
        .map_or(distro, |(d, _)| d);
    Some((format!("wsl:{resolved}"), Some(resolved.into())))
}
pub fn resolve_project_picker_target(
    browse: Option<&EnvironmentId>,
    primary: Option<&EnvironmentId>,
    desktop_instance: Option<&str>,
    config: Option<&WslEnvironmentConfiguration>,
) -> Option<String> {
    if let Some(instance) = desktop_instance {
        return Some(instance.into());
    }
    let config = config?;
    if browse.is_none() || browse != primary || !config.enabled || !config.wsl_only {
        return None;
    }
    configured_backend(config).map(|(backend, _)| backend)
}
pub fn apply_wsl_environment_configuration(
    candidates: &[WslEnvironmentCandidate],
    primary: Option<&EnvironmentId>,
    config: Option<&WslEnvironmentConfiguration>,
    primary_running_distro: Option<&str>,
) -> Vec<WslEnvironmentCandidate> {
    let mut result = candidates.to_vec();
    if let (Some(config), Some(primary)) = (config, primary)
        && config.enabled
        && config.wsl_only
        && !candidates.iter().any(|c| &c.environment == primary)
        && let Some((backend_id, running)) = configured_backend(config)
    {
        result.push(WslEnvironmentCandidate {
            environment: primary.clone(),
            backend_id,
            running_distro: primary_running_distro.map(str::to_string).or(running),
        });
    }
    result
}
