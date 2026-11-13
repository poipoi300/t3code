//! The installed-theme library: what is saved, and the rules for changing it.
//!
//! Persistence sits behind [`ThemeStorage`]. The library keeps the saved JSON as
//! written (themes it does not understand, and fields it does not know, survive an
//! unrelated install) and caches the first read, so a failed re-read cannot make a
//! write destroy what is stored.

use std::collections::BTreeSet;

use serde_json::Value;

use super::definition::{ThemeDefinition, built_in_themes, is_reserved_theme_id};
use super::files::{
    CUSTOM_THEMES_STORAGE_KEY, ThemeError, canonicalize_theme_definition, parse_stored_theme,
    parse_stored_themes,
};

/// Where the saved library lives (browser local storage, a settings file, ...).
pub trait ThemeStorage {
    fn read(&self) -> Result<Option<String>, StorageFailure>;
    fn write(&mut self, raw: &str) -> Result<(), StorageFailure>;
}

/// The storage backend refused to read or write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageFailure;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryOperation {
    Read,
    Write,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryFailure {
    Malformed,
    StorageUnavailable,
}

/// `ThemeLibraryStorageError`, or a rule violation with its message.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LibraryError {
    #[error("Failed to {} the theme library {} {CUSTOM_THEMES_STORAGE_KEY}.", operation_verb(*.operation), operation_direction(*.operation))]
    Storage {
        operation: LibraryOperation,
        reason: LibraryFailure,
    },
    #[error("{0}")]
    Theme(String),
}

fn operation_verb(operation: LibraryOperation) -> &'static str {
    match operation {
        LibraryOperation::Read => "read",
        LibraryOperation::Write => "write",
    }
}

fn operation_direction(operation: LibraryOperation) -> &'static str {
    match operation {
        LibraryOperation::Read => "from",
        LibraryOperation::Write => "to",
    }
}

impl From<ThemeError> for LibraryError {
    fn from(value: ThemeError) -> Self {
        Self::Theme(value.0)
    }
}

fn error(message: impl Into<String>) -> LibraryError {
    LibraryError::Theme(message.into())
}

#[derive(Debug, Clone, PartialEq)]
enum Snapshot {
    Ready {
        stored: Vec<Value>,
        themes: Vec<ThemeDefinition>,
    },
    Unavailable(LibraryFailure),
}

fn read_snapshot(storage: &impl ThemeStorage) -> Snapshot {
    let raw = match storage.read() {
        Ok(raw) => raw,
        Err(StorageFailure) => return Snapshot::Unavailable(LibraryFailure::StorageUnavailable),
    };
    let Some(raw) = raw.filter(|r| !r.is_empty()) else {
        return Snapshot::Ready {
            stored: Vec::new(),
            themes: Vec::new(),
        };
    };
    match serde_json::from_str::<Value>(&raw) {
        Ok(Value::Array(stored)) => {
            let themes = parse_stored_themes(&stored);
            Snapshot::Ready { stored, themes }
        }
        _ => Snapshot::Unavailable(LibraryFailure::Malformed),
    }
}

fn stored_id(value: &Value) -> Option<&str> {
    value.as_object()?.get("id")?.as_str()
}

fn stored_collection_id(value: &Value) -> Option<&str> {
    value
        .as_object()?
        .get("collection")?
        .as_object()?
        .get("id")?
        .as_str()
}

type Loaded = (Vec<Value>, Vec<ThemeDefinition>);

pub struct ThemeLibrary<S> {
    storage: S,
    snapshot: Option<Snapshot>,
}

impl<S: ThemeStorage> ThemeLibrary<S> {
    pub fn new(storage: S) -> Self {
        Self {
            storage,
            snapshot: None,
        }
    }

    pub fn storage(&self) -> &S {
        &self.storage
    }

    pub fn storage_mut(&mut self) -> &mut S {
        &mut self.storage
    }

    /// `invalidateCustomThemes`: forget the cached read.
    pub fn invalidate(&mut self) {
        self.snapshot = None;
    }

    fn snapshot(&mut self) -> &Snapshot {
        let storage = &self.storage;
        self.snapshot.get_or_insert_with(|| read_snapshot(storage))
    }

    fn writable(&mut self) -> Result<Loaded, LibraryError> {
        match self.snapshot() {
            Snapshot::Ready { stored, themes } => Ok((stored.clone(), themes.clone())),
            Snapshot::Unavailable(reason) => Err(LibraryError::Storage {
                operation: LibraryOperation::Read,
                reason: *reason,
            }),
        }
    }

    /// A fresh read that bypasses the cache.
    fn read_writable(&self) -> Result<Loaded, LibraryError> {
        match read_snapshot(&self.storage) {
            Snapshot::Ready { stored, themes } => Ok((stored, themes)),
            Snapshot::Unavailable(reason) => Err(LibraryError::Storage {
                operation: LibraryOperation::Read,
                reason,
            }),
        }
    }

    /// `getCustomThemes`: empty while the library cannot be read.
    pub fn custom_themes(&mut self) -> &[ThemeDefinition] {
        match self.snapshot() {
            Snapshot::Ready { themes, .. } => themes,
            Snapshot::Unavailable(_) => &[],
        }
    }

    /// `getStoredCustomThemeCollection`.
    pub fn stored_collection(
        &self,
        collection_id: &str,
    ) -> Result<Vec<ThemeDefinition>, LibraryError> {
        let (_, themes) = self.read_writable()?;
        Ok(themes
            .into_iter()
            .filter(|theme| {
                theme
                    .collection
                    .as_ref()
                    .is_some_and(|c| c.id == collection_id)
            })
            .collect())
    }

    fn save(
        &mut self,
        stored: Vec<Value>,
        themes: Vec<ThemeDefinition>,
    ) -> Result<(), LibraryError> {
        let raw = serde_json::to_string(&stored).map_err(|e| error(e.to_string()))?;
        self.storage
            .write(&raw)
            .map_err(|StorageFailure| LibraryError::Storage {
                operation: LibraryOperation::Write,
                reason: LibraryFailure::StorageUnavailable,
            })?;
        self.snapshot = Some(Snapshot::Ready { stored, themes });
        Ok(())
    }

    /// `installCustomTheme`.
    pub fn install(&mut self, theme: &ThemeDefinition) -> Result<ThemeDefinition, LibraryError> {
        if is_reserved_theme_id(&theme.id) {
            return Err(error(format!("The theme id \"{}\" is reserved.", theme.id)));
        }
        let (mut stored, mut themes) = self.writable()?;
        if built_in_themes()
            .iter()
            .any(|existing| existing.id == theme.id)
            || stored
                .iter()
                .any(|value| stored_id(value) == Some(theme.id.as_str()))
        {
            return Err(error(format!(
                "A theme named \"{}\" is already installed.",
                theme.label
            )));
        }
        let canonical = canonicalize_theme_definition(theme)?;
        themes.push(canonical.clone());
        stored.push(to_value(&canonical)?);
        self.save(stored, themes)?;
        Ok(canonical)
    }

    /// `updateCustomTheme`.
    pub fn update(&mut self, theme: &ThemeDefinition) -> Result<ThemeDefinition, LibraryError> {
        if is_reserved_theme_id(&theme.id) {
            return Err(error(format!("The theme id \"{}\" is reserved.", theme.id)));
        }
        let (stored, mut themes) = self.writable()?;
        let Some(index) = themes.iter().position(|existing| existing.id == theme.id) else {
            return Err(error(format!(
                "The theme \"{}\" is not installed.",
                theme.label
            )));
        };
        let canonical = canonicalize_theme_definition(theme)?;
        themes[index] = canonical.clone();
        let mut next = Vec::with_capacity(stored.len());
        let mut replaced = false;
        for value in stored {
            if stored_id(&value) != Some(theme.id.as_str()) {
                next.push(value);
            } else if !replaced {
                next.push(to_value(&canonical)?);
                replaced = true;
            }
        }
        self.save(next, themes)?;
        Ok(canonical)
    }

    /// `replaceCustomThemeCollection`: swaps a whole imported collection in one write.
    pub fn replace_collection(
        &mut self,
        collection_id: &str,
        replacement: &[ThemeDefinition],
        expected: Option<&[ThemeDefinition]>,
    ) -> Result<Vec<ThemeDefinition>, LibraryError> {
        if replacement.is_empty() {
            return Err(error("A theme collection cannot be empty."));
        }
        let validated: Vec<Option<ThemeDefinition>> = replacement
            .iter()
            .map(|theme| {
                to_value(theme)
                    .ok()
                    .and_then(|value| parse_stored_theme(&value))
            })
            .collect();
        let mut ids = BTreeSet::new();
        let invalid = validated.iter().any(|theme| match theme {
            None => true,
            Some(t) => {
                t.collection.as_ref().map(|c| c.id.as_str()) != Some(collection_id)
                    || !ids.insert(t.id.clone())
            }
        });
        if invalid {
            return Err(error("That theme collection is invalid."));
        }
        let replacement: Vec<ThemeDefinition> = validated.into_iter().flatten().collect();
        let (stored, current) = self.read_writable()?;
        let current_collection: Vec<&ThemeDefinition> = current
            .iter()
            .filter(|theme| {
                theme
                    .collection
                    .as_ref()
                    .is_some_and(|c| c.id == collection_id)
            })
            .collect();
        if let Some(expected) = expected
            && !(current_collection.len() == expected.len()
                && current_collection
                    .iter()
                    .zip(expected)
                    .all(|(a, b)| *a == b))
        {
            return Err(error(
                "Your installed themes changed while this package was downloading. Try again.",
            ));
        }
        let mut occupied: BTreeSet<String> = built_in_themes()
            .iter()
            .map(|theme| theme.id.clone())
            .collect();
        for value in &stored {
            if stored_collection_id(value) != Some(collection_id)
                && let Some(id) = stored_id(value)
            {
                occupied.insert(id.to_owned());
            }
        }
        if let Some(conflict) = replacement
            .iter()
            .find(|theme| is_reserved_theme_id(&theme.id) || occupied.contains(&theme.id))
        {
            return Err(error(format!(
                "A theme named \"{}\" is already installed.",
                conflict.label
            )));
        }
        let mut next: Vec<Value> = Vec::new();
        let mut inserted = false;
        for value in stored {
            if stored_collection_id(&value) != Some(collection_id) {
                next.push(value);
            } else if !inserted {
                for theme in &replacement {
                    next.push(to_value(theme)?);
                }
                inserted = true;
            }
        }
        if !inserted {
            for theme in &replacement {
                next.push(to_value(theme)?);
            }
        }
        let themes = parse_stored_themes(&next);
        self.save(next, themes)?;
        Ok(replacement)
    }

    /// `removeCustomTheme` / `removeCustomThemes`.
    pub fn remove(&mut self, theme_ids: &[&str]) -> Result<(), LibraryError> {
        if theme_ids.is_empty() {
            return Ok(());
        }
        let (stored, themes) = self.writable()?;
        let remaining: Vec<ThemeDefinition> = themes
            .iter()
            .filter(|t| !theme_ids.contains(&t.id.as_str()))
            .cloned()
            .collect();
        if remaining.len() == themes.len() {
            return Ok(());
        }
        let stored = stored
            .into_iter()
            .filter(|value| stored_id(value).is_none_or(|id| !theme_ids.contains(&id)))
            .collect();
        self.save(stored, remaining)
    }
}

fn to_value(theme: &ThemeDefinition) -> Result<Value, LibraryError> {
    serde_json::to_value(theme).map_err(|e| error(e.to_string()))
}
