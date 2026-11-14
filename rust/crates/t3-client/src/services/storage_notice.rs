//! What restoring found it could not keep, kept on disk until the UI has shown it: a crash
//! between restore and the notice must not lose it, since the next launch's report is clean.
use crate::{
    platform::{Platform, StorageArea, StorageKey, StorageReport, StorageWrite},
    types::EnvironmentId,
};
use serde::{Deserialize, Serialize};

/// What the first restore found it could not keep, for the UI to tell the user once.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageNotice {
    pub storage: StorageReport,
    /// The saved environment list was unreadable; it was set aside and started empty.
    pub reset_catalog: bool,
    /// Saved environments with a record that could not be read.
    pub damaged: Vec<EnvironmentId>,
}

impl StorageNotice {
    /// Both, as one notice: counts add up, and an environment is listed once.
    fn merge(mut self, other: Self) -> Self {
        let (mine, theirs) = (&mut self.storage, other.storage);
        mine.discarded_secure_records += theirs.discarded_secure_records;
        mine.retained_secure_records += theirs.retained_secure_records;
        mine.recovered_secure_records += theirs.recovered_secure_records;
        mine.replaced_unreadable_key |= theirs.replaced_unreadable_key;
        mine.recreated_database |= theirs.recreated_database;
        self.reset_catalog |= other.reset_catalog;
        for environment in other.damaged {
            if !self.damaged.contains(&environment) {
                self.damaged.push(environment);
            }
        }
        self
    }
}

fn key() -> StorageKey {
    StorageKey::new("storage-notice", None, "unacknowledged")
}

/// Adds `notice` to one an earlier launch saved and nobody saw, saves the result until it is
/// acknowledged, and returns it. A storage failure keeps the in-memory notice: it is still shown.
pub(crate) async fn remember(platform: &(impl Platform + ?Sized), notice: StorageNotice) -> StorageNotice {
    let saved = crate::persistence::load_record::<StorageNotice>(platform, StorageArea::Ordinary, &key(), 1)
        .await
        .ok()
        .flatten()
        .map(|record| record.payload)
        .unwrap_or_default();
    let notice = saved.clone().merge(notice);
    if notice != saved
        && let Ok(value) = serde_json::to_value(&notice)
        && let Ok(encoded) =
            crate::persistence::Record::new("storage-notice", None, "unacknowledged", 1, value).encode()
    {
        let _ = platform
            .write(
                StorageArea::Ordinary,
                vec![StorageWrite {
                    key: key(),
                    value: Some(encoded),
                }],
            )
            .await;
    }
    notice
}

/// The UI has the notice: forget the saved copy.
pub(crate) async fn acknowledge(platform: &(impl Platform + ?Sized)) {
    let _ = platform
        .write(StorageArea::Ordinary, vec![StorageWrite { key: key(), value: None }])
        .await;
}
