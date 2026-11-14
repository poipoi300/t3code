//! The client settings (every screen's, the composer's presentation among them); server model
//! preferences stay remote.
use super::send::Changes;
use crate::{
    ClientError,
    persistence::{Record, decode_record},
    platform::{Platform, PlatformEvent, StorageArea, StorageKey},
};
use futures::{FutureExt, StreamExt, future::LocalBoxFuture, stream::LocalBoxStream};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use t3_contracts::settings::ClientSettingsSchema;

/// Whether the stored client settings have been read (TS `ClientSettingsHydrationStatus`).
/// Until `Ready`, [`ComposerPreferences::snapshot`] holds the defaults, not the user's settings,
/// so an unset field there means "not read yet", never "never set" (a first run, say).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsHydration {
    Pending,
    Ready,
    /// The first read failed ([`ComposerPreferences::error`]); [`ComposerPreferences::retry`].
    Failed,
    Retrying,
}

pub struct ComposerPreferences<P: Platform> {
    platform: Rc<P>,
    settings: RefCell<Rc<ClientSettingsSchema>>,
    error: RefCell<Option<Rc<ClientError>>>,
    hydration: Cell<SettingsHydration>,
    changes: Changes,
}
fn key() -> StorageKey {
    StorageKey::new("client-settings", None, "t3code:client-settings:v1")
}
impl<P: Platform + 'static> ComposerPreferences<P> {
    pub fn new(platform: Rc<P>) -> Self {
        Self {
            platform,
            settings: RefCell::new(Rc::new(Default::default())),
            error: RefCell::new(None),
            hydration: Cell::new(SettingsHydration::Pending),
            changes: Changes::default(),
        }
    }
    /// Whether [`Self::snapshot`] is the stored settings yet; [`Self::changes`] fires when it moves.
    pub fn hydration(&self) -> SettingsHydration {
        self.hydration.get()
    }
    /// Reads the stored settings again after the first read failed (the web's Retry); the outcome
    /// is [`Self::hydration`] and [`Self::error`].
    pub async fn retry(&self) {
        if self.hydration.get() == SettingsHydration::Ready {
            return;
        }
        self.set_hydration(SettingsHydration::Retrying);
        if let Err(error) = self.load().await {
            self.failed(error);
        }
    }
    fn set_hydration(&self, hydration: SettingsHydration) {
        if self.hydration.replace(hydration) != hydration {
            self.changes.notify();
        }
    }
    /// A read failed: hydrated settings stay hydrated (a later reload failing does not unread them).
    fn failed(&self, error: ClientError) {
        *self.error.borrow_mut() = Some(Rc::new(error));
        if self.hydration.get() != SettingsHydration::Ready {
            self.hydration.set(SettingsHydration::Failed);
        }
        self.changes.notify();
    }
    pub fn snapshot(&self) -> Rc<ClientSettingsSchema> {
        self.settings.borrow().clone()
    }
    pub fn error(&self) -> Option<Rc<ClientError>> {
        self.error.borrow().clone()
    }
    pub fn changes(&self) -> LocalBoxStream<'static, ()> {
        self.changes.subscribe()
    }
    pub async fn update(
        &self,
        edit: impl FnOnce(&mut ClientSettingsSchema),
    ) -> Result<(), ClientError> {
        use crate::platform::{StorageCondition, StorageTransaction, StorageWrite};
        let prior_bytes = self.platform.get(StorageArea::Ordinary, key()).await?;
        let prior = self.snapshot();
        // Before the first read the snapshot is the defaults: edit the stored record instead, so an
        // early write (completing onboarding, say) never replaces the user's settings with defaults.
        let mut next = match (self.hydration.get(), prior_bytes.as_deref()) {
            (SettingsHydration::Ready, _) | (_, None) => prior.as_ref().clone(),
            (_, Some(bytes)) => decode_record::<ClientSettingsSchema>(bytes, &key(), 1)
                .map_err(|e| ClientError::InvalidPayload(e.to_string()))?
                .payload,
        };
        edit(&mut next);
        let next = Rc::new(next);
        let value = serde_json::to_vec(&Record::new(
            "client-settings",
            None,
            key().record,
            1,
            next.as_ref(),
        ))?;
        *self.settings.borrow_mut() = next.clone();
        self.changes.notify();
        let written = self
            .platform
            .transaction(StorageTransaction {
                conditions: vec![StorageCondition {
                    area: StorageArea::Ordinary,
                    key: key(),
                    expected: prior_bytes,
                }],
                ordinary: vec![StorageWrite {
                    key: key(),
                    value: Some(value),
                }],
                secure: vec![],
            })
            .await;
        match written {
            Ok(true) => Ok(()),
            result => {
                if Rc::ptr_eq(&self.snapshot(), &next) {
                    *self.settings.borrow_mut() = prior;
                    self.changes.notify();
                }
                match result {
                    Err(error) => Err(error.into()),
                    _ => Err(ClientError::InvalidPayload(
                        "Client settings changed concurrently; try again.".into(),
                    )),
                }
            }
        }
    }
    pub async fn load(&self) -> Result<(), ClientError> {
        let settings = match self.platform.get(StorageArea::Ordinary, key()).await? {
            None => ClientSettingsSchema::default(),
            Some(bytes) => {
                let record: Record<ClientSettingsSchema> = decode_record(&bytes, &key(), 1)
                    .map_err(|e| ClientError::InvalidPayload(e.to_string()))?;
                record.payload
            }
        };
        if **self.settings.borrow() != settings {
            *self.settings.borrow_mut() = Rc::new(settings);
        }
        *self.error.borrow_mut() = None;
        self.hydration.set(SettingsHydration::Ready);
        self.changes.notify();
        Ok(())
    }
    pub fn run(self: Rc<Self>) -> LocalBoxFuture<'static, ()> {
        async move {
            if let Err(error) = self.load().await {
                self.failed(error);
            }
            let mut events = self.platform.events();
            while let Some(event) = events.next().await {
                if matches!(event, PlatformEvent::StorageChanged(ref changed) if *changed == key())
                    && let Err(error) = self.load().await
                {
                    self.failed(error);
                }
            }
        }
        .boxed_local()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::platform::{Storage, StorageWrite};
    use crate::testing::FakePlatform;
    use futures::executor::block_on;

    fn stored(platform: &FakePlatform, bytes: Vec<u8>) {
        block_on(platform.write(
            StorageArea::Ordinary,
            vec![StorageWrite {
                key: key(),
                value: Some(bytes),
            }],
        ))
        .unwrap();
    }

    fn completed_on(date: &str) -> Vec<u8> {
        let settings = ClientSettingsSchema {
            onboarding_completed_at: Some(date.into()),
            ..Default::default()
        };
        serde_json::to_vec(&Record::new("client-settings", None, key().record, 1, &settings))
            .unwrap()
    }

    #[test]
    fn settings_read_nothing_until_loaded_and_say_so() {
        let platform = FakePlatform::default();
        stored(&platform, completed_on("2026-11-01T00:00:00.000Z"));
        let preferences = ComposerPreferences::new(Rc::new(platform));
        // Not loaded: the snapshot is the defaults, and the hydration says it is not the user's.
        assert_eq!(preferences.hydration(), SettingsHydration::Pending);
        assert_eq!(preferences.snapshot().onboarding_completed_at, None);
        block_on(preferences.load()).unwrap();
        assert_eq!(preferences.hydration(), SettingsHydration::Ready);
        assert_eq!(
            preferences.snapshot().onboarding_completed_at,
            Some("2026-11-01T00:00:00.000Z".into())
        );
    }

    #[test]
    fn a_failed_read_is_failed_until_a_retry_reads_it() {
        let platform = FakePlatform::default();
        stored(&platform, b"not a record".to_vec());
        let preferences = Rc::new(ComposerPreferences::new(Rc::new(platform.clone())));
        let mut driver = preferences.clone().run();
        let waker = futures::task::noop_waker();
        let _ = driver
            .as_mut()
            .poll(&mut std::task::Context::from_waker(&waker));
        assert_eq!(preferences.hydration(), SettingsHydration::Failed);
        assert!(preferences.error().is_some());
        stored(&platform, completed_on("2026-11-02T00:00:00.000Z"));
        block_on(preferences.retry());
        assert_eq!(preferences.hydration(), SettingsHydration::Ready);
        assert!(preferences.error().is_none());
    }

    #[test]
    fn a_write_before_the_first_read_keeps_the_stored_settings() {
        let platform = FakePlatform::default();
        let saved = ClientSettingsSchema {
            onboarding_completed_at: Some("2026-11-01T00:00:00.000Z".into()),
            load_balancing_enabled: !ClientSettingsSchema::default().load_balancing_enabled,
            ..Default::default()
        };
        stored(
            &platform,
            serde_json::to_vec(&Record::new("client-settings", None, key().record, 1, &saved))
                .unwrap(),
        );
        let preferences = ComposerPreferences::new(Rc::new(platform.clone()));
        block_on(preferences.update(|settings| {
            settings.onboarding_completed_at = Some("2026-11-14T00:00:00.000Z".into());
        }))
        .unwrap();
        let reread = ComposerPreferences::new(Rc::new(platform));
        block_on(reread.load()).unwrap();
        assert_eq!(
            reread.snapshot().load_balancing_enabled,
            saved.load_balancing_enabled,
            "the stored setting the early write did not touch"
        );
        assert_eq!(
            reread.snapshot().onboarding_completed_at,
            Some("2026-11-14T00:00:00.000Z".into())
        );
    }
}
