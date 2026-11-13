use crate::{Error, Result};
use base64::{
    Engine, alphabet,
    engine::{
        DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig, general_purpose::URL_SAFE_NO_PAD,
    },
};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::{
    net::Ipv4Addr,
    path::{Component, Path, PathBuf},
};
use subtle::ConstantTimeEq;
use t3_contracts::{
    auth::{
        AuthClientMetadata, AuthClientMetadataDeviceType, AuthClientPresentationMetadata,
        ServerAuthBootstrapMethod, ServerAuthDescriptor, ServerAuthPolicy, ServerAuthSessionMethod,
        ServerUpdateScope,
    },
    base_schemas::TrimmedNonEmptyString,
};

pub fn b64(bytes: impl AsRef<[u8]>) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}
pub fn unb64(value: &str) -> Option<Vec<u8>> {
    GeneralPurpose::new(
        &alphabet::URL_SAFE,
        GeneralPurposeConfig::new()
            .with_decode_padding_mode(DecodePaddingMode::Indifferent)
            .with_decode_allow_trailing_bits(true),
    )
    .decode(value.replace(['\n', '\r'], "").as_bytes())
    .ok()
}
pub fn sha256(bytes: impl AsRef<[u8]>) -> Vec<u8> {
    Sha256::digest(bytes).to_vec()
}
pub fn random_uuid() -> Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| Error::Random(e.to_string()))?;
    Ok(uuid::Builder::from_random_bytes(bytes)
        .into_uuid()
        .to_string())
}
pub fn hex(bytes: impl AsRef<[u8]>) -> String {
    base16ct::lower::encode_string(bytes.as_ref())
}
pub fn timing_safe_b64(left: &str, right: &str) -> bool {
    match (unb64(left), unb64(right)) {
        (Some(l), Some(r)) => bool::from(l.ct_eq(&r)),
        _ => false,
    }
}

#[derive(Clone)]
pub struct PayloadSigner {
    mac: Hmac<Sha256>,
}
impl PayloadSigner {
    pub fn new(key: Vec<u8>) -> Result<Self> {
        if key.is_empty() {
            return Err(Error::EmptySecret);
        }
        let mac = Hmac::<Sha256>::new_from_slice(&key).map_err(|_| Error::InvalidSigningKey)?;
        Ok(Self { mac })
    }
    pub async fn from_store(store: &crate::ServerSecretStore, name: &str) -> Result<Self> {
        Self::new(store.get_or_create_random(name, 32).await?)
    }
    pub fn sign_bytes(&self, payload: &str) -> Vec<u8> {
        let mut mac = self.mac.clone();
        mac.update(payload.as_bytes());
        mac.finalize().into_bytes().to_vec()
    }
    pub fn sign(&self, payload: &str) -> String {
        b64(self.sign_bytes(payload))
    }
    pub fn verify(&self, payload: &str, signature: &str) -> bool {
        unb64(signature).is_some_and(|s| bool::from(self.sign_bytes(payload).ct_eq(&s)))
    }
}

pub fn normalize_hostname(host: &str) -> String {
    let host = host.to_lowercase();
    let host = host.strip_prefix('[').unwrap_or(&host);
    let host = host.strip_suffix(']').unwrap_or(host);
    host.trim_end_matches('.').to_owned()
}

pub fn loopback_bind_address(host: Option<&str>) -> Option<String> {
    let Some(host) = host else {
        return Some("127.0.0.1".into());
    };
    let host = normalize_hostname(host);
    if host == "localhost" {
        return Some("127.0.0.1".into());
    }
    if host == "::1"
        || host
            .parse::<Ipv4Addr>()
            .is_ok_and(|ip| ip.octets()[0] == 127)
    {
        Some(host)
    } else {
        None
    }
}
pub fn is_loopback_host(host: &str) -> bool {
    let host = normalize_hostname(host);
    if host == "localhost" || host == "::1" {
        return true;
    }
    let parts: Vec<_> = host.split('.').collect();
    let number = |p: &str| {
        p.trim()
            .parse::<f64>()
            .ok()
            .or_else(|| p.trim().is_empty().then_some(0.0))
    };
    parts.len() == 4
        && parts
            .iter()
            .all(|p| number(p).is_some_and(|n| n.fract() == 0.0 && (0.0..=255.0).contains(&n)))
        && number(parts[0]) == Some(127.0)
}

pub fn session_cookie_name(config: &crate::AuthConfig) -> String {
    if config.desktop {
        return format!("t3_session_{}", config.port);
    }
    let remote = !config.development() && loopback_bind_address(config.host.as_deref()).is_none();
    let key = if remote {
        &config.environment_id
    } else {
        &config.state_dir
    };
    let hash = hex(sha256(key));
    if remote {
        format!("t3_session_{}", &hash[..12])
    } else {
        format!("t3_session_{}_{}", config.port, &hash[..12])
    }
}
pub fn legacy_cookie_name(config: &crate::AuthConfig) -> Option<String> {
    (!config.desktop
        && !config.development()
        && loopback_bind_address(config.host.as_deref()).is_none())
    .then(|| "t3_session".into())
}
pub fn descriptor(config: &crate::AuthConfig) -> ServerAuthDescriptor {
    let remote = loopback_bind_address(config.host.as_deref()).is_none();
    let policy = if remote {
        ServerAuthPolicy::RemoteReachable
    } else if config.desktop {
        ServerAuthPolicy::DesktopManagedLocal
    } else {
        ServerAuthPolicy::LoopbackBrowser
    };
    let bootstrap_methods = if config.desktop && remote {
        vec![
            ServerAuthBootstrapMethod::DesktopBootstrap,
            ServerAuthBootstrapMethod::OneTimeToken,
        ]
    } else if config.desktop {
        vec![ServerAuthBootstrapMethod::DesktopBootstrap]
    } else {
        vec![ServerAuthBootstrapMethod::OneTimeToken]
    };
    ServerAuthDescriptor {
        policy,
        bootstrap_methods,
        session_methods: vec![
            ServerAuthSessionMethod::BrowserSessionCookie,
            ServerAuthSessionMethod::BearerAccessToken,
            ServerAuthSessionMethod::DpopAccessToken,
        ],
        session_cookie_name: TrimmedNonEmptyString(session_cookie_name(config)),
        server_update_scope: Some(ServerUpdateScope::EnvironmentMaintain),
    }
}
pub fn default_metadata() -> AuthClientMetadata {
    AuthClientMetadata {
        label: None,
        ip_address: None,
        user_agent: None,
        device_type: AuthClientMetadataDeviceType::Unknown,
        os: None,
        browser: None,
    }
}
pub fn derive_client_metadata(
    user_agent: Option<&str>,
    remote_address: Option<&str>,
    presented: Option<&AuthClientPresentationMetadata>,
) -> AuthClientMetadata {
    let user_agent = user_agent.map(str::trim).filter(|s| !s.is_empty());
    let agent = user_agent.unwrap_or("").to_lowercase();
    let has = |patterns: &[&str]| patterns.iter().any(|s| agent.contains(s));
    let browser = if has(&["edg/", "edge/", "edga/", "edgios/"]) {
        Some("Edge")
    } else if has(&["opr/", "opios/"]) {
        Some("Opera")
    } else if has(&["firefox/", "fxios/"]) {
        Some("Firefox")
    } else if has(&["electron/"]) {
        Some("Electron")
    } else if has(&["chrome/", "crios/"]) {
        Some("Chrome")
    } else if has(&["safari/"]) {
        Some("Safari")
    } else {
        None
    };
    let os = if has(&["iphone", "ipad", "ipod"]) {
        Some("iOS")
    } else if has(&["android"]) {
        Some("Android")
    } else if has(&["mac os x", "macintosh"]) {
        Some("macOS")
    } else if has(&["windows nt"]) {
        Some("Windows")
    } else if has(&["linux"]) {
        Some("Linux")
    } else {
        None
    };
    let device_type = if agent.is_empty() {
        AuthClientMetadataDeviceType::Unknown
    } else if has(&["bot", "crawler", "spider", "slurp", "curl", "wget"]) {
        AuthClientMetadataDeviceType::Bot
    } else if has(&["ipad", "tablet"]) {
        AuthClientMetadataDeviceType::Tablet
    } else if has(&["iphone", "mobile"]) {
        AuthClientMetadataDeviceType::Mobile
    } else {
        AuthClientMetadataDeviceType::Desktop
    };
    AuthClientMetadata {
        label: presented.and_then(|p| p.label.clone()),
        ip_address: remote_address
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| TrimmedNonEmptyString(s.strip_prefix("::ffff:").unwrap_or(s).into())),
        user_agent: user_agent.map(|s| TrimmedNonEmptyString(s.into())),
        device_type: presented.and_then(|p| p.device_type).unwrap_or(device_type),
        os: presented
            .and_then(|p| p.os.clone())
            .or_else(|| os.map(|s| TrimmedNonEmptyString(s.into()))),
        browser: browser.map(|s| TrimmedNonEmptyString(s.into())),
    }
}

pub fn pairing_url(base: &str, credential: &str) -> Result<String> {
    let mut url = url::Url::parse(base).map_err(|e| Error::InvalidPayload(e.to_string()))?;
    url.set_path("/pair");
    let query: Vec<_> = url
        .query_pairs()
        .filter(|(k, _)| k != "token")
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    url.set_query(None);
    if !query.is_empty() {
        url.query_pairs_mut().extend_pairs(query);
    }
    url.set_fragment(Some(
        &url::form_urlencoded::Serializer::new(String::new())
            .append_pair("token", credential)
            .finish(),
    ));
    Ok(url.into())
}

pub async fn is_server_internal_path(
    file: PathBuf,
    state: PathBuf,
    secrets: PathBuf,
) -> Result<bool> {
    tokio::task::spawn_blocking(move || {
        let resolve = |p: &Path| dunce::canonicalize(p).unwrap_or_else(|_| lexical_absolute(p));
        let file = resolve(&file);
        let state = resolve(&state);
        let secrets = resolve(&secrets);
        Ok(file.starts_with(secrets)
            || (file.parent() == Some(state.as_path())
                && file
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().contains(".sqlite")))
            || file.starts_with(resolve(Path::new("/dev/fd"))))
    })
    .await
    .map_err(|e| Error::InvalidPayload(e.to_string()))?
}
fn lexical_absolute(p: &Path) -> PathBuf {
    let abs = if p.is_absolute() {
        p.to_owned()
    } else {
        std::env::current_dir().unwrap_or_default().join(p)
    };
    let mut result = PathBuf::new();
    for component in abs.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            c => result.push(c.as_os_str()),
        }
    }
    result
}
