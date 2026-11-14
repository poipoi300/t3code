use crate::{Error, Result, lock};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use t3_contracts::{browser_profile::*, preview::PreviewReportProfilesInput};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileCatalog {
    pub profiles: Vec<BrowserProfile>,
    pub default_profile_id: String,
}

fn builtins() -> Vec<BrowserProfile> {
    vec![
        BrowserProfile {
            id: "default".into(),
            name: "Default".into(),
            kind: BrowserProfileKind::Persistent,
        },
        BrowserProfile {
            id: "incognito".into(),
            name: "Incognito".into(),
            kind: BrowserProfileKind::Incognito,
        },
    ]
}
fn resolve(profiles: Vec<BrowserProfile>, default: String) -> ProfileCatalog {
    let mut all = builtins();
    let mut seen = BTreeSet::from(["default".to_owned(), "incognito".to_owned()]);
    for mut profile in profiles {
        if seen.insert(profile.id.clone()) {
            profile.kind = BrowserProfileKind::Persistent;
            all.push(profile);
        }
    }
    let default_profile_id = if seen.contains(&default) {
        default
    } else {
        "default".into()
    };
    ProfileCatalog {
        profiles: all,
        default_profile_id,
    }
}
pub fn validate_profile_id(id: &str) -> Result<()> {
    if id.trim() != id
        || id.is_empty()
        || id.encode_utf16().count() > 64
        || id.chars().any(char::is_control)
    {
        return Err(Error::InvalidInput("Invalid browser profile id".into()));
    }
    Ok(())
}
fn validate_profiles(profiles: &[BrowserProfile]) -> Result<()> {
    for profile in profiles {
        validate_profile_id(&profile.id)?;
        if profile.name.trim() != profile.name
            || profile.name.is_empty()
            || profile.name.encode_utf16().count() > 48
        {
            return Err(Error::InvalidInput("Invalid browser profile name".into()));
        }
    }
    Ok(())
}
struct State {
    reports: Vec<(u64, PreviewReportProfilesInput)>,
    catalog: Option<ProfileCatalog>,
    to_save: Option<ProfileCatalog>,
    rate: u64,
    next: u64,
}
#[derive(Clone)]
pub struct Profiles {
    state: Arc<Mutex<State>>,
    path: PathBuf,
    save: Arc<tokio::sync::Mutex<()>>,
}
impl Profiles {
    pub async fn load(state_dir: &Path) -> Self {
        let path = state_dir.join("server-browser/reported-profiles.json");
        let catalog = match tokio::fs::read(&path).await {
            Ok(bytes) if bytes.is_empty() => None,
            Ok(bytes) => match serde_json::from_slice::<ProfileCatalog>(&bytes) {
                Ok(catalog) if validate_profiles(&catalog.profiles).is_ok() => {
                    Some(resolve(catalog.profiles, catalog.default_profile_id))
                }
                _ => {
                    tracing::warn!("ignoring invalid saved browser profiles");
                    None
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                tracing::warn!(%error,"failed to read saved browser profiles");
                None
            }
        };
        Self {
            state: Arc::new(Mutex::new(State {
                reports: vec![],
                to_save: catalog.clone(),
                catalog,
                rate: 30,
                next: 0,
            })),
            path,
            save: Arc::default(),
        }
    }
    pub fn reporter(&self) -> ProfileReporter {
        let mut state = lock(&self.state);
        let id = state.next;
        state.next += 1;
        ProfileReporter {
            profiles: self.clone(),
            id,
        }
    }
    pub fn catalog(&self) -> ProfileCatalog {
        lock(&self.state).catalog.clone().unwrap_or(ProfileCatalog {
            profiles: builtins(),
            default_profile_id: "default".into(),
        })
    }
    pub fn has_report(&self) -> bool {
        lock(&self.state).catalog.is_some()
    }
    pub fn frame_rate(&self) -> u64 {
        lock(&self.state).rate
    }
    pub fn resolve_open(&self, requested: Option<&str>) -> Result<Option<String>> {
        let state = lock(&self.state);
        if requested.is_none() {
            return Ok(state.catalog.as_ref().map(|c| c.default_profile_id.clone()));
        }
        let catalog = state.catalog.clone().unwrap_or(ProfileCatalog {
            profiles: builtins(),
            default_profile_id: "default".into(),
        });
        let wanted = requested.expect("checked requested profile");
        let found = catalog
            .profiles
            .iter()
            .find(|p| p.id == wanted)
            .or_else(|| {
                catalog
                    .profiles
                    .iter()
                    .find(|p| p.name.to_lowercase() == wanted.to_lowercase())
            });
        found.map(|p| Some(p.id.clone())).ok_or_else(|| {
            Error::operation(
                "PreviewAutomationUnknownProfileError",
                format!(
                    "No browser profile is named \"{wanted}\". Use one of: {}.",
                    catalog
                        .profiles
                        .iter()
                        .map(|p| format!("{} (id {})", p.name, p.id))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )
        })
    }
    async fn save(&self) -> Result<()> {
        let _guard = self.save.lock().await;
        let catalog = lock(&self.state).to_save.clone();
        if let Some(mut catalog) = catalog {
            catalog
                .profiles
                .retain(|p| !matches!(p.id.as_str(), "default" | "incognito"));
            let bytes = serde_json::to_vec(&catalog)?;
            let path = self.path.clone();
            tokio::task::spawn_blocking(move || atomic_write(&path, &bytes))
                .await
                .map_err(|e| Error::Io(e.to_string()))??;
        }
        Ok(())
    }
}
pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let parent = path
        .parent()
        .ok_or_else(|| Error::InvalidInput("Missing parent directory".into()))?;
    std::fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(bytes)?;
    file.flush()?;
    file.persist(path).map_err(|e| Error::Io(e.to_string()))?;
    Ok(())
}
fn rebuild(state: &mut State) {
    if let Some((_, latest)) = state.reports.last() {
        let mut profiles: Vec<BrowserProfile> = Vec::new();
        for (_, report) in &state.reports {
            for profile in &report.profiles {
                if let Some(existing) = profiles.iter_mut().find(|p| p.id == profile.id) {
                    *existing = profile.clone();
                } else {
                    profiles.push(profile.clone());
                }
            }
        }
        state.catalog = Some(resolve(profiles, latest.default_profile_id.clone()));
    }
}
pub struct ProfileReporter {
    profiles: Profiles,
    id: u64,
}
impl ProfileReporter {
    pub async fn report(&self, input: PreviewReportProfilesInput) -> Result<()> {
        if input.profiles.len() > BROWSER_PROFILE_MAX_COUNT {
            return Err(Error::InvalidInput("Too many browser profiles".into()));
        }
        validate_profiles(&input.profiles)?;
        validate_profile_id(&input.default_profile_id)?;
        if input
            .recording_frame_rate
            .flatten()
            .is_some_and(|rate| !matches!(rate, 30 | 60))
        {
            return Err(Error::InvalidInput(
                "Unsupported recording frame rate".into(),
            ));
        }
        {
            let mut state = lock(&self.profiles.state);
            if let Some(rate) = input.recording_frame_rate.flatten() {
                state.rate = rate;
            }
            state.reports.retain(|(id, _)| *id != self.id);
            state.reports.push((self.id, input));
            rebuild(&mut state);
            state.to_save = state.catalog.clone();
        }
        let profiles = self.profiles.clone();
        tokio::spawn(async move { profiles.save().await })
            .await
            .map_err(|e| Error::Io(e.to_string()))?
    }
}
impl Drop for ProfileReporter {
    fn drop(&mut self) {
        let mut state = lock(&self.profiles.state);
        state.reports.retain(|(id, _)| *id != self.id);
        rebuild(&mut state);
    }
}
