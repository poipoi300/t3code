use crate::{
    Result,
    environment::{Environment, process_options},
    invalid,
};
use regex::Regex;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, io::Write, sync::LazyLock, time::Duration};
use t3_contracts::{
    secret_reference::{SecretProviderId, SecretReference},
    settings::SecretProviderConfig,
};
use tokio::sync::{Mutex, RwLock};

static ACCOUNT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._-]*$").expect("account regex"));
static REFERENCE: LazyLock<Regex> = LazyLock::new(|| {
    let name = r"[A-Za-z0-9._-](?:[A-Za-z0-9 ._-]*[A-Za-z0-9._-])?";
    Regex::new(&format!(
        r"^op://{name}(?:/{name}){{2,3}}(?:\?[a-z-]+=[A-Za-z0-9_-]+)?$"
    ))
    .expect("reference regex")
});

pub fn valid_one_password_account(account: &str) -> bool {
    account.len() <= 253 && ACCOUNT.is_match(account)
}
pub fn valid_one_password_reference(reference: &str) -> bool {
    reference.len() <= 1024 && REFERENCE.is_match(reference)
}

pub struct OnePassword {
    executable: String,
    environment: Environment,
}

impl OnePassword {
    pub fn new(environment: Environment) -> Self {
        Self {
            executable: "op".into(),
            environment,
        }
    }

    /// Explicit executable selection supports isolated toolchains and process fixtures.
    pub fn with_executable(executable: String, environment: Environment) -> Self {
        Self {
            executable,
            environment,
        }
    }

    async fn run(
        &self,
        args: &[String],
    ) -> std::result::Result<t3_process::ProcessOutput, t3_process::ProcessError> {
        let args = std::iter::once("--cache=false".to_owned())
            .chain(args.iter().cloned())
            .collect::<Vec<_>>();
        let mut options = process_options(&self.executable, &args, &self.environment);
        options.timeout = Some(Duration::from_secs(45));
        options.check_exit = false;
        t3_process::run(options).await
    }

    pub async fn read(&self, account: &str, references: &[String]) -> Result<Vec<Option<String>>> {
        if !valid_one_password_account(account)
            || references.iter().any(|r| !valid_one_password_reference(r))
        {
            return Err(invalid("1Password read", "invalid account or reference"));
        }
        if references.len() > 1 {
            let separator = format!("__t3-secret-{}__", uuid::Uuid::new_v4());
            let mut template = tempfile::Builder::new()
                .prefix("t3code-op-inject-")
                .tempfile()?;
            for reference in references {
                write!(template, "{{{{ {reference} }}}}{separator}")?;
            }
            template.flush()?;
            let result = self
                .run(&[
                    "inject".into(),
                    format!("--account={account}"),
                    "-i".into(),
                    template.path().to_string_lossy().into_owned(),
                ])
                .await;
            match result {
                Err(t3_process::ProcessError::Timeout { .. }) => {
                    return Ok(vec![None; references.len()]);
                }
                Ok(result) if result.code == Some(0) => {
                    if let Some(values) =
                        parse_injected_secrets(&result.stdout_text(), &separator, references.len())
                    {
                        return Ok(values);
                    }
                }
                _ => {}
            }
        }
        let mut values = Vec::with_capacity(references.len());
        for reference in references {
            let output = self
                .run(&[
                    "read".into(),
                    format!("--account={account}"),
                    "--no-newline".into(),
                    reference.clone(),
                ])
                .await;
            values.push(
                output
                    .ok()
                    .filter(|o| o.code == Some(0))
                    .map(|o| o.stdout_text())
                    .filter(|s| !s.trim().is_empty()),
            );
        }
        Ok(values)
    }
}

pub fn parse_injected_secrets(
    output: &str,
    separator: &str,
    count: usize,
) -> Option<Vec<Option<String>>> {
    let mut values: Vec<_> = output.split(separator).collect();
    if !values.pop()?.trim().is_empty() || values.len() != count {
        return None;
    }
    Some(
        values
            .into_iter()
            .map(|s| {
                if s.trim().is_empty() {
                    None
                } else {
                    Some(s.to_owned())
                }
            })
            .collect(),
    )
}

struct CachedSecret {
    config: String,
    value: Option<String>,
}

/// Holds both successful and failed reads until invalidated or the vault configuration changes.
pub struct SecretResolver {
    one_password: OnePassword,
    cache: RwLock<BTreeMap<(String, String), CachedSecret>>,
    gate: Mutex<()>,
}

impl SecretResolver {
    pub fn new(one_password: OnePassword) -> Self {
        Self {
            one_password,
            cache: RwLock::new(BTreeMap::new()),
            gate: Mutex::new(()),
        }
    }

    pub async fn read(
        &self,
        providers: &BTreeMap<SecretProviderId, SecretProviderConfig>,
        references: &[SecretReference],
        fresh: bool,
    ) -> Vec<Option<String>> {
        let plans: Vec<_> = references
            .iter()
            .map(|r| {
                let provider = providers.get(&r.provider)?;
                if provider.kind != "one_password" || !valid_one_password_reference(&r.reference) {
                    return None;
                }
                let account = provider.config.as_ref()?.get("account")?.as_str()?;
                if !valid_one_password_account(account) {
                    return None;
                }
                Some(account.to_owned())
            })
            .collect();
        let key = |r: &SecretReference| (r.provider.0.clone(), r.reference.clone());
        {
            let cache = self.cache.read().await;
            if !fresh
                && references.iter().zip(&plans).all(|(r, p)| {
                    p.as_ref()
                        .is_none_or(|p| cache.get(&key(r)).is_some_and(|c| c.config == *p))
                })
            {
                return references
                    .iter()
                    .zip(&plans)
                    .map(|(r, p)| {
                        p.as_ref().and_then(|p| {
                            cache
                                .get(&key(r))
                                .filter(|c| c.config == *p)
                                .and_then(|c| c.value.clone())
                        })
                    })
                    .collect();
            }
        }
        let _gate = self.gate.lock().await;
        let mut groups: Vec<(String, String, Vec<String>)> = Vec::new();
        {
            let cache = self.cache.read().await;
            for (reference, plan) in references.iter().zip(&plans) {
                let Some(account) = plan else {
                    continue;
                };
                if !fresh
                    && cache
                        .get(&key(reference))
                        .is_some_and(|c| c.config == *account)
                {
                    continue;
                }
                if let Some(group) = groups.iter_mut().find(|g| g.0 == reference.provider.0) {
                    if !group.2.contains(&reference.reference) {
                        group.2.push(reference.reference.clone());
                    }
                } else {
                    groups.push((
                        reference.provider.0.clone(),
                        account.clone(),
                        vec![reference.reference.clone()],
                    ));
                }
            }
        }
        let mut answered = BTreeMap::new();
        for (provider, account, wanted) in groups {
            let values = self
                .one_password
                .read(&account, &wanted)
                .await
                .unwrap_or_else(|_| vec![None; wanted.len()]);
            let mut cache = self.cache.write().await;
            for (reference, value) in wanted.into_iter().zip(values) {
                let cache_key = (provider.clone(), reference);
                let readable = value.as_ref().is_some_and(|v| !v.trim().is_empty());
                if !fresh || readable || cache.get(&cache_key).is_none_or(|c| c.config != account) {
                    cache.insert(
                        cache_key.clone(),
                        CachedSecret {
                            config: account.clone(),
                            value: value.clone(),
                        },
                    );
                }
                answered.insert(cache_key, value);
            }
        }
        let cache = self.cache.read().await;
        references
            .iter()
            .zip(&plans)
            .map(|(r, p)| {
                p.as_ref().and_then(|p| {
                    if fresh {
                        answered.get(&key(r)).cloned().flatten()
                    } else {
                        cache
                            .get(&key(r))
                            .filter(|c| c.config == *p)
                            .and_then(|c| c.value.clone())
                    }
                })
            })
            .collect()
    }

    pub async fn invalidate(&self, references: Option<&[SecretReference]>) {
        let _gate = self.gate.lock().await;
        let mut cache = self.cache.write().await;
        if let Some(references) = references {
            for r in references {
                cache.remove(&(r.provider.0.clone(), r.reference.clone()));
            }
        } else {
            cache.clear();
        }
    }

    pub fn label(
        providers: &BTreeMap<SecretProviderId, SecretProviderConfig>,
        provider: &SecretProviderId,
    ) -> String {
        providers
            .get(provider)
            .map(|p| p.label.0.trim())
            .filter(|s| !s.is_empty())
            .unwrap_or(&provider.0)
            .to_owned()
    }
}

pub fn credential_binding_key(driver: &str, binding: &str) -> String {
    let tuple = format!("{}:{driver}{binding}", driver.encode_utf16().count());
    format!("provider-auth-{:x}", Sha256::digest(tuple.as_bytes()))
}

pub struct CredentialStore {
    store: t3_config::secrets::SecretStore,
    key: String,
}
impl CredentialStore {
    pub fn new(store: t3_config::secrets::SecretStore, driver: &str, binding: &str) -> Self {
        Self {
            store,
            key: credential_binding_key(driver, binding),
        }
    }
    pub fn key(&self) -> &str {
        &self.key
    }
    pub fn get(&self) -> t3_config::Result<Option<Vec<u8>>> {
        self.store.get(&self.key)
    }
    pub async fn set(&self, value: &[u8]) -> t3_config::Result<()> {
        self.store.set(&self.key, value).await
    }
    pub fn remove(&self) -> t3_config::Result<()> {
        self.store.remove(&self.key)
    }
}
