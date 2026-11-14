use crate::adapters::config::ConfigInputs;
use crate::adapters::{
    auth::AuthService,
    persistence::{Database, service_error},
};
use crate::cli::CliError;
use std::collections::BTreeMap;
use t3_config::{
    environment::resolve_worktree_home,
    home::{HomeLayout, resolve_dev_home},
    runtime::read_live_runtime_state,
};
use t3_contracts::{
    auth::{AuthEnvironmentScope, AuthPairingCredentialResult},
    environment::ExecutionEnvironmentDescriptor,
};

pub(crate) fn select_home(inputs: &mut ConfigInputs, home_dir: Option<&str>) {
    let worktree = resolve_worktree_home(&inputs.process_cwd);
    let explicit = home_dir
        .filter(|value| !value.trim().is_empty())
        .or(inputs.flags.base_dir.as_deref());
    let (home, selected) = resolve_dev_home(
        explicit,
        worktree.as_deref(),
        inputs.env.get("T3CODE_HOME").map(String::as_str),
        &inputs.home,
        &inputs.process_cwd,
    );
    if selected {
        inputs.flags.base_dir = Some(home.to_string_lossy().into_owned());
    }
}
pub(crate) fn load_observability(inputs: &mut ConfigInputs) -> Result<(), CliError> {
    let config = crate::adapters::config::resolve(inputs)?;
    let raw = match std::fs::read_to_string(config.paths.settings()) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    inputs.persisted_observability = t3_config::config::persisted_observability(&raw);
    Ok(())
}
pub(crate) fn standard_scopes() -> Vec<AuthEnvironmentScope> {
    t3_auth::authorization::STANDARD_SCOPES.to_vec()
}
pub(crate) fn parse_bool(raw: &str) -> Option<bool> {
    t3_config::config::parse_bool(raw)
}
async fn mint(
    config: crate::adapters::config::ResolvedConfig,
    scopes: Vec<AuthEnvironmentScope>,
    ttl: Option<i64>,
    label: String,
    policy: Option<t3_contracts::managed_policy::ManagedPolicyDocument>,
) -> Result<AuthPairingCredentialResult, CliError> {
    let path = config.paths.database();
    if !path.is_file() {
        return Err(CliError::Usage("Running server database is missing".into()));
    }
    let owner_directory = config.paths.state_dir.clone();
    let db = tokio::task::spawn_blocking(move || {
        Database::open_with_options(
            path,
            t3_persistence::OpenOptions {
                migration_owner_directory: Some(owner_directory),
                ..Default::default()
            },
        )
    })
    .await
    .map_err(crate::Error::from)?
    .map_err(service_error)?;
    let result = async {
        let id = std::fs::read_to_string(config.paths.environment_id())?;
        let auth = AuthService::new(&config, id.trim().to_owned(), db.clone(), policy).await?;
        auth.inner()
            .create_pairing_link(
                &standard_scopes(),
                t3_auth::pairing::IssuePairing {
                    scopes,
                    ttl_ms: ttl,
                    label: Some(label),
                    ..Default::default()
                },
            )
            .await
            .map_err(service_error)
            .map_err(CliError::from)
    }
    .await;
    let closed = db.close().await.map_err(service_error);
    let issued = result?;
    closed?;
    Ok(issued)
}
pub(crate) async fn pair(
    inputs: ConfigInputs,
    scopes: Vec<AuthEnvironmentScope>,
    ttl: Option<i64>,
    label: String,
) -> Result<PairOutput, CliError> {
    let mut config = crate::adapters::config::resolve(&inputs)?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_millis(2500))
        .build()
        .map_err(service_error)?;
    let mut checked = Vec::new();
    for variant in ["userdata", "dev"] {
        let layout = HomeLayout {
            base_dir: config.paths.base_dir.clone(),
            state_dir: config.paths.base_dir.join(variant),
        };
        checked.push(layout.runtime_state());
        let Some(state) = read_live_runtime_state(&layout.runtime_state()) else {
            continue;
        };
        let Ok(url) = url::Url::parse(&state.origin) else {
            continue;
        };
        if url.scheme() != "http" {
            continue;
        }
        let Ok(response) = client
            .get(format!(
                "{}/.well-known/t3/environment",
                state.origin.trim_end_matches('/')
            ))
            .send()
            .await
        else {
            continue;
        };
        if !response.status().is_success() {
            continue;
        }
        let Ok(descriptor) = response.json::<ExecutionEnvironmentDescriptor>().await else {
            continue;
        };
        if descriptor.orchestration_protocol_version != Some(2) {
            return Err(CliError::Usage(format!(
                "The running T3 Code server ({}) speaks orchestration protocol {}, but this CLI speaks protocol 2.",
                descriptor.server_version.0,
                descriptor.orchestration_protocol_version.unwrap_or(1)
            )));
        }
        let persisted_id = std::fs::read_to_string(layout.environment_id())?;
        if persisted_id.trim() != descriptor.environment_id.0 {
            continue;
        }
        config.paths = layout;
        config.port = u16::try_from(state.port).map_err(|e| CliError::Usage(e.to_string()))?;
        config.host = state.host.clone();
        config.dev_url = state
            .dev_url
            .as_deref()
            .map(url::Url::parse)
            .transpose()
            .map_err(|e| CliError::Usage(e.to_string()))?;
        let port = config.port;
        let policy = crate::adapters::config::managed_policy(&inputs.env)?;
        let issued = mint(config, scopes, ttl, label, policy).await?;
        return Ok(PairOutput {
            label: descriptor.label.0,
            origin: state.origin,
            host: state.host,
            port,
            dev_url: state.dev_url,
            credential: issued.credential.0,
            expires_at: issued.expires_at.0,
        });
    }
    Err(CliError::Usage(format!(
        "No running T3 Code server found.\n{}\nStart one with `t3 serve`, or connect this machine with T3 Connect: `t3 connect`.",
        checked
            .iter()
            .map(|p| format!("  checked {}", p.display()))
            .collect::<Vec<_>>()
            .join("\n")
    )))
}
pub(crate) fn status(inputs: ConfigInputs) -> Result<StatusOutput, CliError> {
    let config = crate::adapters::config::resolve(&inputs)?;
    let raw = match std::fs::read_to_string(config.paths.service_env()) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.into()),
    };
    let entries = service_env(&raw);
    let lan = entries
        .get("T3CODE_LAN")
        .map(|value| {
            t3_config::config::parse_bool(value)
                .ok_or_else(|| CliError::Usage("Invalid T3CODE_LAN in service.env".into()))
        })
        .transpose()?
        .unwrap_or(false);
    let host = entries.get("T3CODE_HOST").cloned();
    Ok(StatusOutput {
        service: ServiceStatus {
            supported: false,
            installed: false,
            installed_version: None,
            other_base_dir: None,
        },
        connect: None,
        lan: LanStatus {
            enabled: lan,
            inherited_default: false,
            host_override: host,
        },
    })
}

#[derive(serde::Serialize)]
pub(crate) struct StatusOutput {
    pub(crate) service: ServiceStatus,
    pub(crate) connect: Option<serde_json::Value>,
    pub(crate) lan: LanStatus,
}
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ServiceStatus {
    pub(crate) supported: bool,
    pub(crate) installed: bool,
    pub(crate) installed_version: Option<String>,
    pub(crate) other_base_dir: Option<String>,
}
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LanStatus {
    pub(crate) enabled: bool,
    pub(crate) inherited_default: bool,
    pub(crate) host_override: Option<String>,
}

pub(crate) struct PairOutput {
    pub(crate) label: String,
    pub(crate) origin: String,
    pub(crate) host: Option<String>,
    pub(crate) port: u16,
    pub(crate) dev_url: Option<String>,
    pub(crate) credential: String,
    pub(crate) expires_at: String,
}

fn service_env(contents: &str) -> BTreeMap<String, String> {
    let mut result = BTreeMap::new();
    let mut rest = contents.trim_start_matches('\u{feff}');
    while !rest.is_empty() {
        let (line, next) = rest.split_once('\n').unwrap_or((rest, ""));
        let line = line.trim_start();
        if line.starts_with('#') {
            rest = next;
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line).trim_start();
        let Some((key, _)) = line.split_once('=') else {
            rest = next;
            continue;
        };
        let key = key.trim();
        if key.is_empty()
            || !key
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.'))
        {
            rest = next;
            continue;
        }
        let Some(equals) = rest.find('=') else {
            rest = next;
            continue;
        };
        let value = rest[equals + 1..].trim_start_matches([' ', '\t', '\r']);
        if let Some(quote @ ('\'' | '"' | '`')) = value.chars().next()
            && let Some(end) = value[1..].find(quote)
        {
            let raw = &value[1..end + 1];
            result.insert(
                key.to_owned(),
                if quote == '"' {
                    raw.replace("\\n", "\n")
                } else {
                    raw.to_owned()
                },
            );
            rest = value[end + 2..]
                .split_once('\n')
                .map_or("", |(_, tail)| tail);
        } else {
            let raw = line.split_once('=').map_or("", |(_, value)| value);
            result.insert(
                key.to_owned(),
                raw.split('#').next().unwrap_or("").trim().to_owned(),
            );
            rest = next;
        }
    }
    result
}
