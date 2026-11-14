use super::{COLUMNS, PAIRING_ALPHABET, PairingChange, PairingGrantStore, decode, select, to_link};
use crate::{
    Result,
    clock::iso,
    session::{db_date, decode_json},
};
use t3_contracts::auth::AuthConnectDevice;
use t3_contracts::{
    auth::{AuthClientMetadata, AuthPairingCredentialResult},
    base_schemas::TrimmedNonEmptyString,
};
use t3_persistence::rusqlite::{self, OptionalExtension, params};

pub fn connect_device_verification_code(thumbprint: &str) -> String {
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let characters: Vec<_> = char::decode_utf16(thumbprint.encode_utf16().take(8))
        .map(|c| {
            let index = alphabet
                .iter()
                .position(|b| Some(char::from(*b)) == c.as_ref().ok().copied())
                .unwrap_or(0);
            char::from(PAIRING_ALPHABET[index % PAIRING_ALPHABET.len()])
        })
        .collect();
    let split = characters.len().min(4);
    format!(
        "{}-{}",
        characters[..split].iter().collect::<String>(),
        characters[split..].iter().collect::<String>()
    )
}

fn trust(
    db: &rusqlite::Connection,
    key: &str,
    client: Option<&str>,
    now: &str,
) -> rusqlite::Result<()> {
    db.execute(
        "INSERT INTO auth_connect_devices(proof_key_thumbprint,client_json,approved_at)
         VALUES(?1,?2,?3)
         ON CONFLICT(proof_key_thumbprint) DO UPDATE
         SET client_json=COALESCE(excluded.client_json,auth_connect_devices.client_json)",
        params![key, client, now],
    )?;
    Ok(())
}

impl PairingGrantStore {
    pub async fn find_pending_connect(
        &self,
        key: &str,
    ) -> Result<Option<AuthPairingCredentialResult>> {
        let key = key.to_owned();
        let now = iso(self.inner.clock.now_ms())?.0;
        Ok(self
            .inner
            .db
            .reader(move |db| {
                Ok(db
                    .query_row(
                        &format!(
                            "SELECT {COLUMNS},credential FROM auth_pairing_links
                    WHERE proof_key_thumbprint=?1 AND requires_connect_approval=1
                    AND (revoked_at IS NULL OR connect_denied=1)
                    AND consumed_at IS NULL AND expires_at>?2
                    ORDER BY created_at DESC,id DESC LIMIT 1"
                        ),
                        params![key, now],
                        |r| {
                            let record = decode(r)?;
                            Ok(AuthPairingCredentialResult {
                                id: TrimmedNonEmptyString(record.id),
                                credential: TrimmedNonEmptyString(r.get(13)?),
                                label: record
                                    .grant
                                    .label
                                    .filter(|s| !s.is_empty())
                                    .map(TrimmedNonEmptyString),
                                expires_at: record.grant.expires_at,
                                access: None,
                            })
                        },
                    )
                    .optional()?)
            })
            .await?)
    }
    pub async fn record_connect_request_client(
        &self,
        id: &str,
        client: AuthClientMetadata,
    ) -> Result<()> {
        let store = self.clone();
        let id = id.to_owned();
        let client = serde_json::to_string(&client)?;
        crate::finish_mutation(async move {
            let now = iso(store.inner.clock.now_ms())?.0;
            let row = store
                .inner
                .db
                .writer(move |db| {
                    let tx = db.transaction()?;
                    let recorded = tx.execute(
                        "UPDATE auth_pairing_links SET requested_client_json=?1
                     WHERE id=?2 AND requires_connect_approval=1 AND requested_client_json IS NULL",
                        params![client, id],
                    )?;
                    let row = if recorded > 0 {
                        tx.query_row(
                            &format!(
                                "{} WHERE id=?1 AND revoked_at IS NULL
                            AND consumed_at IS NULL AND expires_at>?2",
                                select()
                            ),
                            params![id, now],
                            decode,
                        )
                        .optional()?
                    } else {
                        None
                    };
                    tx.commit()?;
                    Ok(row)
                })
                .await?;
            if let Some(row) = row {
                store
                    .emit(PairingChange::PairingLinkUpserted(Box::new(to_link(&row))))
                    .await;
            }
            Ok(())
        })
        .await
    }
    pub async fn is_connect_device_trusted(&self, key: &str) -> Result<bool> {
        let key = key.to_owned();
        Ok(self
            .inner
            .db
            .reader(move |db| {
                Ok(db.query_row(
                "SELECT EXISTS(SELECT 1 FROM auth_connect_devices WHERE proof_key_thumbprint=?1)",
                [key], |r| r.get(0),
            )?)
            })
            .await?)
    }
    pub async fn trust_connect_device(&self, key: &str, client: AuthClientMetadata) -> Result<()> {
        let store = self.clone();
        let key = key.to_owned();
        let client = serde_json::to_string(&client)?;
        crate::finish_mutation(async move {
            let now = iso(store.inner.clock.now_ms())?.0;
            store
                .inner
                .db
                .writer(move |db| {
                    trust(db, &key, Some(&client), &now)?;
                    Ok(())
                })
                .await?;
            store.emit(PairingChange::ConnectDevicesChanged).await;
            Ok(())
        })
        .await
    }
    pub async fn approve_connect_request(&self, id: &str) -> Result<bool> {
        let store = self.clone();
        let id = id.to_owned();
        crate::finish_mutation(async move {
            let now = iso(store.inner.clock.now_ms())?.0;
            let selected = id.clone();
            let approved = store
                .inner
                .db
                .writer(move |db| {
                    let tx = db.transaction()?;
                    let row = tx
                        .query_row(
                            &format!(
                                "{} WHERE id=?1 AND revoked_at IS NULL
                        AND consumed_at IS NULL AND expires_at>?2",
                                select()
                            ),
                            params![selected, now],
                            decode,
                        )
                        .optional()?;
                    let Some(row) = row.filter(|r| {
                        r.requires_connect_approval && r.grant.proof_key_thumbprint.is_some()
                    }) else {
                        return Ok(false);
                    };
                    let client = row
                        .requested_client
                        .as_ref()
                        .map(serde_json::to_string)
                        .transpose()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?;
                    if let Some(key) = row.grant.proof_key_thumbprint {
                        trust(&tx, &key, client.as_deref(), &now)?;
                    }
                    tx.commit()?;
                    Ok(true)
                })
                .await?;
            if approved {
                store.emit(PairingChange::PairingLinkRemoved(id)).await;
                store.emit(PairingChange::ConnectDevicesChanged).await;
            }
            Ok(approved)
        })
        .await
    }
    pub async fn list_connect_devices(&self) -> Result<Vec<AuthConnectDevice>> {
        Ok(self
            .inner
            .db
            .reader(|db| {
                let mut stmt = db.prepare(
                    "SELECT proof_key_thumbprint,client_json,approved_at FROM auth_connect_devices
                 ORDER BY approved_at DESC,proof_key_thumbprint",
                )?;
                Ok(stmt
                    .query_map([], |r| {
                        let key: String = r.get(0)?;
                        Ok(AuthConnectDevice {
                            verification_code: TrimmedNonEmptyString(
                                connect_device_verification_code(&key),
                            ),
                            proof_key_thumbprint: TrimmedNonEmptyString(key),
                            client: r
                                .get::<_, Option<String>>(1)?
                                .map(|s| decode_json(&s))
                                .transpose()?,
                            approved_at: db_date(r.get(2)?)?,
                        })
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?)
            })
            .await?)
    }
    pub async fn revoke_connect_device(&self, key: &str) -> Result<bool> {
        Ok(self.revoke_connect_keys(Some(key.to_owned()), None).await? > 0)
    }
    pub async fn revoke_connect_devices_except(&self, keep: Option<&str>) -> Result<usize> {
        self.revoke_connect_keys(None, keep.map(str::to_owned))
            .await
    }
    async fn revoke_connect_keys(
        &self,
        key: Option<String>,
        keep: Option<String>,
    ) -> Result<usize> {
        let store = self.clone();
        crate::finish_mutation(async move {
            let now = iso(store.inner.clock.now_ms())?.0;
            let (ids, removed) = store
                .inner
                .db
                .writer(move |db| {
                    let tx = db.transaction()?;
                    let ids = {
                        let mut stmt = tx.prepare(
                            "UPDATE auth_pairing_links SET revoked_at=?1
                         WHERE proof_key_thumbprint IS NOT NULL
                         AND revoked_at IS NULL AND consumed_at IS NULL
                         AND (?2 IS NULL OR proof_key_thumbprint=?2)
                         AND proof_key_thumbprint IS NOT ?3 RETURNING id",
                        )?;
                        stmt.query_map(params![now, key, keep], |r| r.get::<_, String>(0))?
                            .collect::<rusqlite::Result<Vec<_>>>()?
                    };
                    let removed = tx.execute(
                        "DELETE FROM auth_connect_devices
                     WHERE (?1 IS NULL OR proof_key_thumbprint=?1)
                     AND proof_key_thumbprint IS NOT ?2",
                        params![key, keep],
                    )?;
                    tx.commit()?;
                    Ok((ids, removed))
                })
                .await?;
            for id in ids {
                store.emit(PairingChange::PairingLinkRemoved(id)).await;
            }
            if removed > 0 {
                store.emit(PairingChange::ConnectDevicesChanged).await;
            }
            Ok(removed)
        })
        .await
    }
}
