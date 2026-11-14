use crate::{
    AuthConfig, Error, Result,
    authorization::{STANDARD_SCOPES, scope_name, validate_delegation},
    clock::{Clock, expires_after, iso},
    error::CredentialFailure as F,
    session::{db_date, decode_enum, decode_json},
    utils::{PayloadSigner, hex},
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use t3_contracts::{
    auth::{
        AuthClientMetadata, AuthConnectDeviceRequest, AuthEnvironmentScope, AuthMcpClientAccess,
        AuthPairingCredentialResult, AuthPairingLink, ServerAuthBootstrapMethod,
    },
    base_schemas::TrimmedNonEmptyString,
    wire::WireDateTime,
};
use t3_persistence::{
    Database,
    rusqlite::{self, OptionalExtension, params},
};
use tokio::sync::{Mutex, mpsc};
pub const PAIRING_ALPHABET: &[u8] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ";
pub const PAIRING_LENGTH: usize = 12;
pub const DESKTOP_WINDOW_MS: i64 = 43_200_000;
pub const APPROVAL_SUBJECT_PREFIX: &str = "mcp-approval:";
#[path = "connect.rs"]
mod connect;
pub use connect::connect_device_verification_code;

#[derive(Clone)]
pub struct PairingGrantStore {
    inner: Arc<Inner>,
}
struct Inner {
    db: Database,
    config: AuthConfig,
    clock: Arc<dyn Clock>,
    seeded_expires_ms: i64,
    seed_active: AtomicBool,
    subscribers: Mutex<Vec<mpsc::UnboundedSender<PairingChange>>>,
}
#[derive(Debug, Clone)]
pub enum PairingChange {
    PairingLinkUpserted(Box<AuthPairingLink>),
    PairingLinkRemoved(String),
    ConnectDevicesChanged,
}
#[derive(Default)]
pub struct IssuePairing {
    pub ttl_ms: Option<i64>,
    pub scopes: Vec<AuthEnvironmentScope>,
    pub subject: Option<String>,
    pub label: Option<String>,
    pub proof_key_thumbprint: Option<String>,
    pub startup: bool,
    pub mcp_approval: Option<AuthMcpClientAccess>,
    pub requires_connect_approval: bool,
}
#[derive(Default)]
pub struct ConsumePairing {
    pub proof_key_thumbprint: Option<String>,
    pub requested_scopes: Option<Vec<AuthEnvironmentScope>>,
    pub required_scopes: Option<Vec<AuthEnvironmentScope>>,
    pub mcp_approval: Option<AuthMcpClientAccess>,
}
#[derive(Debug, Clone)]
pub struct BootstrapGrant {
    pub method: ServerAuthBootstrapMethod,
    pub scopes: Vec<AuthEnvironmentScope>,
    pub subject: String,
    pub label: Option<String>,
    pub proof_key_thumbprint: Option<String>,
    pub expires_at: WireDateTime,
}
#[derive(Clone)]
struct PairingRecord {
    id: String,
    grant: BootstrapGrant,
    created_at: WireDateTime,
    consumed_at: Option<WireDateTime>,
    revoked_at: Option<WireDateTime>,
    requires_connect_approval: bool,
    connect_approved: bool,
    requested_client: Option<AuthClientMetadata>,
}

impl PairingGrantStore {
    pub fn new(db: Database, config: AuthConfig, clock: Arc<dyn Clock>) -> Result<Self> {
        config.validate()?;
        let seeded_expires_ms = expires_after(clock.now_ms(), 86_400_000)?;
        let seed_active = AtomicBool::new(
            config.desktop_bootstrap_secret.is_none()
                && config
                    .desktop_bootstrap_token
                    .as_ref()
                    .is_some_and(|s| !s.is_empty()),
        );
        Ok(Self {
            inner: Arc::new(Inner {
                db,
                config,
                clock,
                seeded_expires_ms,
                seed_active,
                subscribers: Mutex::new(Vec::new()),
            }),
        })
    }
    pub async fn subscribe(&self) -> mpsc::UnboundedReceiver<PairingChange> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.inner.subscribers.lock().await.push(tx);
        rx
    }
    async fn emit(&self, event: PairingChange) {
        self.inner
            .subscribers
            .lock()
            .await
            .retain(|s| s.send(event.clone()).is_ok());
    }
    pub async fn issue_one_time_token(
        &self,
        caller_scopes: &[AuthEnvironmentScope],
        input: IssuePairing,
    ) -> Result<AuthPairingCredentialResult> {
        validate_delegation(caller_scopes, &input.scopes)?;
        let store = self.clone();
        crate::finish_mutation(async move { store.issue_inner(input).await }).await
    }
    async fn issue_inner(&self, input: IssuePairing) -> Result<AuthPairingCredentialResult> {
        let id = crate::utils::random_uuid()?;
        let token = generate_pairing_code()?;
        let credential = input
            .mcp_approval
            .map(|a| approval_code(a, &token))
            .unwrap_or(token);
        let now = self.inner.clock.now_ms();
        let ttl = input
            .ttl_ms
            .unwrap_or(if self.inner.config.development() && input.startup {
                86_400_000
            } else {
                300_000
            });
        let expires_at = iso(expires_after(now, ttl)?)?;
        let subject = if let Some(access) = input.mcp_approval {
            format!("{APPROVAL_SUBJECT_PREFIX}{}", access_name(access)?)
        } else {
            input.subject.unwrap_or_else(|| "one-time-token".into())
        };
        let record = PairingRecord {
            id: id.clone(),
            grant: BootstrapGrant {
                method: ServerAuthBootstrapMethod::OneTimeToken,
                scopes: input.scopes,
                subject,
                label: input.label,
                proof_key_thumbprint: input.proof_key_thumbprint,
                expires_at: expires_at.clone(),
            },
            created_at: iso(now)?,
            consumed_at: None,
            revoked_at: None,
            requires_connect_approval: input.requires_connect_approval,
            connect_approved: false,
            requested_client: None,
        };
        let stored = record.clone();
        let code = credential.clone();
        let scopes = serde_json::to_string(&record.grant.scopes)?;
        self.inner
            .db
            .writer(move |db| {
                db.execute(
                    "INSERT INTO auth_pairing_links(
                    id, credential, method, scopes, subject, label, proof_key_thumbprint,
                    created_at, expires_at, consumed_at, revoked_at, requires_connect_approval)
                 VALUES(?1,?2,'one-time-token',?3,?4,?5,?6,?7,?8,NULL,NULL,?9)",
                    params![
                        stored.id,
                        code,
                        scopes,
                        stored.grant.subject,
                        stored.grant.label,
                        stored.grant.proof_key_thumbprint,
                        stored.created_at.0,
                        stored.grant.expires_at.0,
                        stored.requires_connect_approval
                    ],
                )?;
                Ok(())
            })
            .await?;
        if input.mcp_approval.is_none() {
            self.emit(PairingChange::PairingLinkUpserted(Box::new(to_link(
                &record,
            ))))
            .await;
        }
        Ok(AuthPairingCredentialResult {
            id: TrimmedNonEmptyString(id),
            credential: TrimmedNonEmptyString(credential),
            label: record.grant.label.map(TrimmedNonEmptyString),
            expires_at,
            access: input.mcp_approval,
        })
    }
    pub async fn list_active(&self) -> Result<Vec<AuthPairingLink>> {
        let now = iso(self.inner.clock.now_ms())?.0;
        let rows = self
            .inner
            .db
            .reader(move |db| {
                let mut stmt = db.prepare(&format!(
                    "{} WHERE revoked_at IS NULL AND consumed_at IS NULL AND expires_at>?1
                 ORDER BY created_at DESC,id DESC",
                    select(),
                ))?;
                Ok(stmt
                    .query_map([now], decode)?
                    .collect::<rusqlite::Result<Vec<_>>>()?)
            })
            .await?;
        Ok(rows
            .iter()
            .filter(|r| !(r.requires_connect_approval && r.connect_approved))
            .map(to_link)
            .collect())
    }
    pub async fn revoke(&self, id: &str) -> Result<bool> {
        let store = self.clone();
        let id = id.to_owned();
        crate::finish_mutation(async move { store.revoke_inner(&id).await }).await
    }
    async fn revoke_inner(&self, id: &str) -> Result<bool> {
        let id_string = id.to_owned();
        let now = iso(self.inner.clock.now_ms())?.0;
        let revoked = self
            .inner
            .db
            .writer(move |db| {
                Ok(db.execute(
                    "UPDATE auth_pairing_links SET revoked_at=?1, connect_denied=requires_connect_approval
                 WHERE id=?2 AND revoked_at IS NULL AND consumed_at IS NULL",
                    params![now, id_string],
                )? > 0)
            })
            .await?;
        if revoked {
            self.emit(PairingChange::PairingLinkRemoved(id.into()))
                .await;
        }
        Ok(revoked)
    }
    pub async fn consume(&self, credential: &str, input: ConsumePairing) -> Result<BootstrapGrant> {
        let store = self.clone();
        let credential = credential.to_owned();
        crate::finish_mutation(async move { store.consume_inner(&credential, input).await }).await
    }
    async fn consume_inner(
        &self,
        credential: &str,
        input: ConsumePairing,
    ) -> Result<BootstrapGrant> {
        let now = self.inner.clock.now_ms();
        if let Some(secret) = &self.inner.config.desktop_bootstrap_secret {
            if valid_desktop_token(secret, credential, now)? {
                return Ok(BootstrapGrant {
                    method: ServerAuthBootstrapMethod::DesktopBootstrap,
                    scopes: STANDARD_SCOPES.to_vec(),
                    subject: "desktop-bootstrap".into(),
                    label: None,
                    proof_key_thumbprint: None,
                    expires_at: iso(expires_after(now, DESKTOP_WINDOW_MS)?)?,
                });
            }
        } else if self.inner.seed_active.load(Ordering::SeqCst)
            && self.inner.config.desktop_bootstrap_token.as_deref() == Some(credential)
        {
            if now >= self.inner.seeded_expires_ms {
                if !self.inner.seed_active.swap(false, Ordering::SeqCst) {
                    return Err(Error::credential(F::UnknownBootstrapCredentialError, None));
                }
                return Err(Error::credential(F::ExpiredBootstrapCredentialError, None));
            }
            check_scopes(STANDARD_SCOPES, &input)?;
            return Ok(BootstrapGrant {
                method: ServerAuthBootstrapMethod::DesktopBootstrap,
                scopes: STANDARD_SCOPES.to_vec(),
                subject: "desktop-bootstrap".into(),
                label: None,
                proof_key_thumbprint: None,
                expires_at: iso(self.inner.seeded_expires_ms)?,
            });
        }
        let code = normalize_pairing_code(credential);
        let stored_code = code.clone();
        let proof = input.proof_key_thumbprint.clone();
        let requested = input.requested_scopes.clone();
        let required = input.required_scopes.clone();
        let requested_json = serde_json::to_string(&requested.clone().unwrap_or_default())?;
        let required_json = serde_json::to_string(&required.clone().unwrap_or_default())?;
        let approval = input
            .mcp_approval
            .map(|access| {
                access_name(access).map(|name| format!("{APPROVAL_SUBJECT_PREFIX}{name}"))
            })
            .transpose()?;
        let now_iso = iso(now)?.0;
        let consumed = self
            .inner
            .db
            .writer(move |db| {
                Ok(db
                    .query_row(
                        &format!(
                            "UPDATE auth_pairing_links
             SET consumed_at=?1
             WHERE credential=?2
             AND revoked_at IS NULL
             AND consumed_at IS NULL
             AND expires_at>?1
             AND (subject NOT LIKE 'mcp-approval:%' OR subject=?3)
             AND (proof_key_thumbprint IS NULL OR proof_key_thumbprint=?4)
             AND (requires_connect_approval=0 OR proof_key_thumbprint IN (SELECT proof_key_thumbprint FROM auth_connect_devices))
             AND (?5 OR EXISTS (SELECT 1
             FROM json_each(?6) AS requested
             WHERE requested.value IN (SELECT value
             FROM json_each(auth_pairing_links.scopes))))
             AND NOT EXISTS (SELECT 1
             FROM json_each(?7) AS required
             WHERE required.value NOT IN (SELECT value
             FROM json_each(auth_pairing_links.scopes)))
             RETURNING {COLUMNS}"
                        ),
                        params![
                            now_iso,
                            stored_code,
                            approval,
                            proof,
                            requested.is_none(),
                            requested_json,
                            required_json
                        ],
                        decode,
                    )
                    .optional()?)
            })
            .await?;
        if let Some(record) = consumed {
            if !record.grant.subject.starts_with(APPROVAL_SUBJECT_PREFIX) {
                self.emit(PairingChange::PairingLinkRemoved(record.id))
                    .await;
            }
            return Ok(record.grant);
        }
        let matching = self
            .inner
            .db
            .reader(move |db| {
                Ok(db
                    .query_row(&format!("{} WHERE credential=?1", select()), [code], decode)
                    .optional()?)
            })
            .await?;
        let record =
            matching.ok_or_else(|| Error::credential(F::UnknownBootstrapCredentialError, None))?;
        if record.revoked_at.is_some() {
            return Err(Error::credential(
                F::UnavailableBootstrapCredentialError,
                None,
            ));
        }
        if record.consumed_at.is_some() {
            return Err(Error::credential(F::UnknownBootstrapCredentialError, None));
        }
        if crate::clock::epoch(&record.grant.expires_at)? <= now {
            return Err(Error::credential(F::ExpiredBootstrapCredentialError, None));
        }
        if record
            .grant
            .proof_key_thumbprint
            .as_ref()
            .is_some_and(|p| Some(p) != input.proof_key_thumbprint.as_ref())
        {
            return Err(Error::credential(
                F::BootstrapCredentialProofKeyMismatchError,
                None,
            ));
        }
        check_scopes(&record.grant.scopes, &input)?;
        if record.requires_connect_approval && !record.connect_approved {
            return Err(Error::BootstrapConnectApprovalPending {
                pairing_link_id: record.id,
            });
        }
        Err(Error::credential(
            F::UnavailableBootstrapCredentialError,
            None,
        ))
    }
}
fn check_scopes(scopes: &[AuthEnvironmentScope], input: &ConsumePairing) -> Result<()> {
    if input
        .requested_scopes
        .as_ref()
        .is_some_and(|r| !r.iter().any(|s| scopes.contains(s)))
        || input
            .required_scopes
            .as_ref()
            .is_some_and(|r| !r.iter().all(|s| scopes.contains(s)))
    {
        Err(Error::credential(
            F::BootstrapCredentialScopeNotGrantedError,
            None,
        ))
    } else {
        Ok(())
    }
}
pub fn generate_pairing_code() -> Result<String> {
    let mut code = String::new();
    let limit = 256 / PAIRING_ALPHABET.len() * PAIRING_ALPHABET.len();
    while code.len() < PAIRING_LENGTH {
        let mut bytes = [0u8; PAIRING_LENGTH];
        getrandom::fill(&mut bytes).map_err(|e| Error::Random(e.to_string()))?;
        for byte in bytes {
            if (byte as usize) < limit {
                code.push(PAIRING_ALPHABET[byte as usize % PAIRING_ALPHABET.len()] as char);
                if code.len() == PAIRING_LENGTH {
                    break;
                }
            }
        }
    }
    Ok(code)
}
pub fn access_name(access: AuthMcpClientAccess) -> Result<String> {
    serde_json::to_value(access)?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| Error::InvalidPayload("Invalid MCP access name".into()))
}
pub fn approval_code(access: AuthMcpClientAccess, token: &str) -> String {
    let prefix = match access {
        AuthMcpClientAccess::ReadOnly => "READ",
        AuthMcpClientAccess::ApprovalRequired => "SUPERVISED",
        AuthMcpClientAccess::AutoAcceptEdits => "EDITS",
        AuthMcpClientAccess::Auto => "AUTO",
        AuthMcpClientAccess::FullAccess => "FULL",
    };
    format!("{prefix}-{token}")
}
pub fn parse_approval_code(code: &str) -> Option<(AuthMcpClientAccess, String)> {
    let code = code.trim().to_uppercase();
    for access in [
        AuthMcpClientAccess::ReadOnly,
        AuthMcpClientAccess::ApprovalRequired,
        AuthMcpClientAccess::AutoAcceptEdits,
        AuthMcpClientAccess::Auto,
        AuthMcpClientAccess::FullAccess,
    ] {
        let prefix = approval_code(access, "");
        if let Some(token) = code.strip_prefix(&prefix)
            && valid_code(token)
        {
            return Some((access, code));
        }
    }
    None
}
fn valid_code(code: &str) -> bool {
    code.len() == PAIRING_LENGTH
        && code
            .bytes()
            .all(|b| PAIRING_ALPHABET.contains(&b.to_ascii_uppercase()))
}
pub fn normalize_pairing_code(code: &str) -> String {
    if valid_code(code) {
        code.to_ascii_uppercase()
    } else {
        parse_approval_code(code)
            .map(|(_, code)| code)
            .unwrap_or_else(|| code.into())
    }
}
pub fn current_desktop_token(secret: &str, now_ms: i64) -> Result<String> {
    Ok(desktop_token_for_window(
        &PayloadSigner::new(secret.as_bytes().to_vec())?,
        now_ms.div_euclid(DESKTOP_WINDOW_MS),
    ))
}
fn desktop_token_for_window(signer: &PayloadSigner, window: i64) -> String {
    hex(signer.sign_bytes(&format!("t3-desktop-bootstrap:{window}")))
}
pub fn valid_desktop_token(secret: &str, token: &str, now_ms: i64) -> Result<bool> {
    use subtle::ConstantTimeEq;
    let signer = PayloadSigner::new(secret.as_bytes().to_vec())?;
    let window = now_ms.div_euclid(DESKTOP_WINDOW_MS);
    for offset in [-1, 0, 1] {
        if bool::from(
            desktop_token_for_window(&signer, window + offset)
                .as_bytes()
                .ct_eq(token.as_bytes()),
        ) {
            return Ok(true);
        }
    }
    Ok(false)
}
fn to_link(record: &PairingRecord) -> AuthPairingLink {
    AuthPairingLink {
        id: TrimmedNonEmptyString(record.id.clone()),
        scopes: record.grant.scopes.clone(),
        permissions: None,
        subject: TrimmedNonEmptyString(record.grant.subject.clone()),
        label: record
            .grant
            .label
            .clone()
            .filter(|s| !s.is_empty())
            .map(TrimmedNonEmptyString),
        created_at: record.created_at.clone(),
        expires_at: record.grant.expires_at.clone(),
        connect_device_request: record
            .grant
            .proof_key_thumbprint
            .as_ref()
            .filter(|key| record.requires_connect_approval && !key.is_empty())
            .map(|key| AuthConnectDeviceRequest {
                verification_code: TrimmedNonEmptyString(connect_device_verification_code(key)),
                client: record.requested_client.clone(),
            }),
    }
}
const COLUMNS: &str = "id,method,scopes,subject,label,proof_key_thumbprint,created_at,expires_at,consumed_at,revoked_at,requires_connect_approval,EXISTS (SELECT 1 FROM auth_connect_devices WHERE auth_connect_devices.proof_key_thumbprint=auth_pairing_links.proof_key_thumbprint),requested_client_json";
fn select() -> String {
    format!("SELECT {COLUMNS} FROM auth_pairing_links")
}
fn decode(row: &rusqlite::Row<'_>) -> rusqlite::Result<PairingRecord> {
    Ok(PairingRecord {
        id: row.get(0)?,
        grant: BootstrapGrant {
            method: decode_enum(&row.get::<_, String>(1)?)?,
            scopes: decode_json(&row.get::<_, String>(2)?)?,
            subject: row.get(3)?,
            label: row.get(4)?,
            proof_key_thumbprint: row.get(5)?,
            expires_at: db_date(row.get(7)?)?,
        },
        created_at: db_date(row.get(6)?)?,
        consumed_at: row.get::<_, Option<String>>(8)?.map(db_date).transpose()?,
        revoked_at: row.get::<_, Option<String>>(9)?.map(db_date).transpose()?,
        requires_connect_approval: flag(row, 10)?,
        connect_approved: flag(row, 11)?,
        requested_client: row
            .get::<_, Option<String>>(12)?
            .map(|s| decode_json(&s))
            .transpose()?,
    })
}
fn flag(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<bool> {
    match row.get::<_, i64>(index)? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

pub fn oauth_scope(scopes: &[AuthEnvironmentScope]) -> Result<String> {
    let mut names = Vec::new();
    for scope in scopes {
        let name = scope_name(*scope);
        if names.contains(&name) {
            return Err(Error::InvalidScope);
        }
        names.push(name);
    }
    if names.is_empty() {
        return Err(Error::InvalidScope);
    }
    Ok(names.join(" "))
}
