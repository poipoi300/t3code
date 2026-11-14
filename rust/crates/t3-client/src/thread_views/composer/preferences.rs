//! Client settings used by the composer's presentation; server model preferences stay remote.
use super::send::Changes;
use crate::{
    ClientError,
    persistence::{Record, decode_record},
    platform::{Platform, PlatformEvent, StorageArea, StorageKey},
};
use futures::{FutureExt, StreamExt, future::LocalBoxFuture, stream::LocalBoxStream};
use std::{cell::RefCell, rc::Rc};
use t3_contracts::settings::ClientSettingsSchema;

pub struct ComposerPreferences<P: Platform> {
    platform: Rc<P>,
    settings: RefCell<Rc<ClientSettingsSchema>>,
    error: RefCell<Option<Rc<ClientError>>>,
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
            changes: Changes::default(),
        }
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
    pub async fn update(&self, edit: impl FnOnce(&mut ClientSettingsSchema)) -> Result<(), ClientError> {
        use crate::platform::{StorageCondition, StorageTransaction, StorageWrite};
        let prior_bytes = self.platform.get(StorageArea::Ordinary, key()).await?;
        let prior = self.snapshot();
        let mut next = prior.as_ref().clone();
        edit(&mut next);
        let next = Rc::new(next);
        let value = serde_json::to_vec(&Record::new("client-settings", None, key().record, 1, next.as_ref()))?;
        *self.settings.borrow_mut() = next.clone();
        self.changes.notify();
        let written = self.platform.transaction(StorageTransaction {
            conditions: vec![StorageCondition { area: StorageArea::Ordinary, key: key(), expected: prior_bytes }],
            ordinary: vec![StorageWrite { key: key(), value: Some(value) }], secure: vec![],
        }).await;
        match written {
            Ok(true) => Ok(()),
            result => {
                if Rc::ptr_eq(&self.snapshot(), &next) { *self.settings.borrow_mut() = prior; self.changes.notify(); }
                match result { Err(error) => Err(error.into()), _ => Err(ClientError::InvalidPayload("Client settings changed concurrently; try again.".into())) }
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
        self.changes.notify();
        Ok(())
    }
    pub fn run(self: Rc<Self>) -> LocalBoxFuture<'static, ()> {
        async move {
            if let Err(error) = self.load().await {
                *self.error.borrow_mut() = Some(Rc::new(error));
                self.changes.notify();
            }
            let mut events = self.platform.events();
            while let Some(event) = events.next().await {
                if matches!(event, PlatformEvent::StorageChanged(ref changed) if *changed == key())
                    && let Err(error) = self.load().await
                {
                    *self.error.borrow_mut() = Some(Rc::new(error));
                    self.changes.notify();
                }
            }
        }
        .boxed_local()
    }
}
