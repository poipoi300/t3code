//! Where an administrator deploys the managed policy besides the policy file
//! (`packages/shared/src/managedPolicyFile.ts`): the Group Policy registry key on Windows and the
//! configuration profile on macOS. Linux has the file only. Two sources refuse start rather than
//! merge, so what applies is never a mix nobody wrote.

use crate::{
    Error, Result,
    config::Environment,
    policy::{self, ManagedPolicy},
};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

#[cfg(target_os = "macos")]
mod plutil;
// plutil's runner; its deadline tests run on every platform.
#[cfg(any(target_os = "macos", test))]
mod process;
#[cfg(windows)]
mod windows;

/// Where a configuration profile's managed preferences land on macOS.
pub const MACOS_MANAGED_PREFERENCES_PATH: &str =
    "/Library/Managed Preferences/com.t3tools.t3code.plist";
/// The key Group Policy and Intune write to, below HKLM, read in the 64-bit view. It holds either
/// the Group Policy template's values, one per control, or a `Policy` string with the whole policy
/// as JSON.
pub const WINDOWS_POLICY_SUBKEY: &str = r"SOFTWARE\Policies\T3Tools\T3Code";
pub const WINDOWS_POLICY_KEY: &str = r"HKLM\SOFTWARE\Policies\T3Tools\T3Code";
/// Whether this build honours the `T3CODE_DEV_MANAGED_*` overrides at all: only with the
/// `dev-policy-overrides` feature, which tests and a development runner enable and a shipped
/// binary must not (TypeScript's dev channel, `ManagedPolicy.ts:176-185`). A debug build without
/// it reads the machine's policy like a release build.
pub const DEV_OVERRIDES: bool = cfg!(feature = "dev-policy-overrides");
/// With [`DEV_OVERRIDES`], in a debug build: a key below HKCU read in place of the Policies key,
/// so the registry source can be tried and tested without administrator rights.
pub const DEV_REGISTRY_KEY_ENV: &str = "T3CODE_DEV_MANAGED_POLICY_REGISTRY_KEY";
/// With [`DEV_OVERRIDES`], in a debug build: a property list read in place of the managed
/// preferences.
pub const DEV_PREFERENCES_PATH_ENV: &str = "T3CODE_DEV_MANAGED_PREFERENCES_PATH";

#[derive(Clone, Copy)]
enum FieldType {
    String,
    Json,
    Boolean,
}

/// Each value a Group Policy control writes, and where it lands in the document.
const WINDOWS_POLICY_VALUES: &[(&str, FieldType, &[&str])] = &[
    ("Organization", FieldType::String, &["organization"]),
    ("SupportUrl", FieldType::String, &["supportUrl"]),
    ("Settings", FieldType::Json, &["settings"]),
    (
        "MaxRuntimeMode",
        FieldType::String,
        &["limits", "maxRuntimeMode"],
    ),
    ("Connect", FieldType::Boolean, &["limits", "connect"]),
    (
        "WebhookTasks",
        FieldType::Boolean,
        &["limits", "webhookTasks"],
    ),
    ("Telemetry", FieldType::Boolean, &["limits", "telemetry"]),
    (
        "BrowserCookieImport",
        FieldType::Boolean,
        &["limits", "browserCookieImport"],
    ),
    (
        "ProviderInstances",
        FieldType::Json,
        &["limits", "providers", "instances"],
    ),
    (
        "UpdatesMode",
        FieldType::String,
        &["limits", "updates", "mode"],
    ),
    (
        "UpdatesFeedUrl",
        FieldType::String,
        &["limits", "updates", "feedUrl"],
    ),
    (
        "AllowNightly",
        FieldType::Boolean,
        &["limits", "updates", "allowNightly"],
    ),
];
/// Subkeys of checkboxes: every value set to 1 is one entry of the list at the path.
const WINDOWS_POLICY_LISTS: &[(&str, &[&str])] = &[
    ("Clients", &["limits", "clients"]),
    ("AllowedDrivers", &["limits", "providers", "allowedDrivers"]),
];
// A disabled policy still writes its checkboxes' falseValue; its own DWORD disambiguates it.
const WINDOWS_POLICY_GROUPS: &[(&str, &[&str])] = &[
    ("Clients", &["limits", "clients"]),
    ("AllowedDrivers", &["limits", "providers", "allowedDrivers"]),
    ("Updates", &["limits", "updates"]),
];
/// `ManagedPolicyClient.literals`.
const POLICY_CLIENTS: &[&str] = &["desktop", "local-browser", "remote"];

/// A registry value; REG_EXPAND_SZ and every type but REG_SZ and a 4-byte REG_DWORD read as
/// `Other`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryValue {
    pub name: String,
    pub data: RegistryData,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryData {
    Text(String),
    Dword(u32),
    Other,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RegistryKey {
    pub values: Vec<RegistryValue>,
    pub subkeys: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegistryError {
    /// No registry on this host.
    #[error("the registry API cannot load")]
    Unavailable,
    /// The Win32 error, such as 5 for access denied.
    #[error("Windows error {0}")]
    Win32(u32),
}

/// Reads the policy key and its subkeys. A missing key and a failed read come back apart.
pub trait PolicyRegistry {
    /// The key as start errors name it.
    fn source(&self) -> String;
    /// The values and subkey names of the policy key (`""`) or of its subkey `subkey`; `None`
    /// when absent.
    fn read_key(&self, subkey: &str) -> std::result::Result<Option<RegistryKey>, RegistryError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hive {
    LocalMachine,
    CurrentUser,
}

/// The registry through the Win32 API in this process, so blocked registry tools such as reg.exe
/// do not matter.
#[derive(Debug, Clone)]
pub struct WindowsRegistry {
    hive: Hive,
    key: String,
}
impl WindowsRegistry {
    /// `HKLM\SOFTWARE\Policies\T3Tools\T3Code`, where Group Policy writes.
    pub fn machine_policy() -> Self {
        Self {
            hive: Hive::LocalMachine,
            key: WINDOWS_POLICY_SUBKEY.into(),
        }
    }
    /// A key below HKCU, for development builds and tests.
    pub fn current_user(key: impl Into<String>) -> Self {
        Self {
            hive: Hive::CurrentUser,
            key: key.into(),
        }
    }
}
impl PolicyRegistry for WindowsRegistry {
    fn source(&self) -> String {
        match self.hive {
            Hive::LocalMachine => format!(r"HKLM\{}", self.key),
            Hive::CurrentUser => format!(r"HKCU\{}", self.key),
        }
    }
    fn read_key(&self, subkey: &str) -> std::result::Result<Option<RegistryKey>, RegistryError> {
        let path = if subkey.is_empty() {
            self.key.clone()
        } else {
            format!(r"{}\{subkey}", self.key)
        };
        #[cfg(windows)]
        return windows::read_key(self.hive, &path);
        #[cfg(not(windows))]
        {
            let _ = path;
            Err(RegistryError::Unavailable)
        }
    }
}

/// Holds Group Policy's machine policy section while the registry is read, so a policy refresh
/// cannot change the key part-way through the read.
pub trait PolicyLock {
    /// Takes the section; the guard leaves it when dropped. The error says why it cannot be taken.
    fn enter(&self) -> std::result::Result<PolicyLockGuard<'_>, String>;
}
/// Leaves the policy section when dropped, on every path.
pub struct PolicyLockGuard<'a>(Option<Box<dyn FnOnce() + 'a>>);
impl<'a> PolicyLockGuard<'a> {
    pub fn new(leave: impl FnOnce() + 'a) -> Self {
        Self(Some(Box::new(leave)))
    }
}
impl Drop for PolicyLockGuard<'_> {
    fn drop(&mut self) {
        if let Some(leave) = self.0.take() {
            leave();
        }
    }
}

/// `EnterCriticalPolicySection(TRUE)` on a helper thread, waited for at most `timeout`: the call
/// can block for minutes while Group Policy applies.
#[derive(Debug, Clone, Copy)]
pub struct MachinePolicyLock {
    pub timeout: std::time::Duration,
}
/// Chromium's loader waits 15 seconds, which its field data says covers 99.9% of machines.
pub const MACHINE_POLICY_LOCK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
impl Default for MachinePolicyLock {
    fn default() -> Self {
        Self {
            timeout: MACHINE_POLICY_LOCK_TIMEOUT,
        }
    }
}
impl PolicyLock for MachinePolicyLock {
    fn enter(&self) -> std::result::Result<PolicyLockGuard<'_>, String> {
        #[cfg(windows)]
        return windows::enter_machine_policy_section(self.timeout);
        #[cfg(not(windows))]
        Err("Group Policy exists only on Windows".into())
    }
}

/// What `plutil -convert json` printed and how it exited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Converted {
    pub stdout: String,
    pub exit_code: Option<i32>,
}
/// Converts a property list, binary or XML, to JSON.
pub trait ProfileConverter {
    fn convert_to_json(&self, profile: &Path) -> std::io::Result<Converted>;
}
/// `/usr/bin/plutil`, by absolute path: an earlier PATH entry could otherwise answer that there is
/// no policy. A run longer than 10 seconds reads as a failed one.
#[derive(Debug, Clone, Copy, Default)]
pub struct Plutil;
impl ProfileConverter for Plutil {
    fn convert_to_json(&self, profile: &Path) -> std::io::Result<Converted> {
        #[cfg(target_os = "macos")]
        return plutil::convert(profile);
        #[cfg(not(target_os = "macos"))]
        {
            let _ = profile;
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "plutil exists only on macOS",
            ))
        }
    }
}

/// Every place a policy can come from on `platform`.
pub struct PolicySources<'a> {
    /// `darwin`, `win32` (or `windows`), or anything else for the file only.
    pub platform: &'a str,
    pub file: PathBuf,
    pub profile: PathBuf,
    pub registry: &'a dyn PolicyRegistry,
    /// Taken around the whole registry read.
    pub lock: &'a dyn PolicyLock,
    pub converter: &'a dyn ProfileConverter,
    /// The drivers this build knows; a checkbox naming one in other case takes its spelling.
    pub known_drivers: &'a [&'a str],
}

fn refuse(path: impl Into<String>, reason: impl Into<String>) -> Error {
    Error::ManagedPolicy {
        path: path.into(),
        reason: reason.into(),
    }
}

/// `readManagedPolicy`: the policy from wherever an administrator put it. The returned policy
/// has no document when there is none; its path is the file's then.
pub fn read_managed_policy(sources: &PolicySources<'_>) -> Result<ManagedPolicy> {
    let platform = read_platform_policy(sources)?;
    let file = ManagedPolicy::load_file(&sources.file)?;
    match platform {
        Some(found) if file.document.is_some() => Err(refuse(
            found.path.display().to_string(),
            format!(
                "a policy file at {} also exists; deploy the policy one way",
                sources.file.display()
            ),
        )),
        Some(found) => Ok(found),
        None => Ok(file),
    }
}

/// The machine's real sources on `platform`.
pub fn read_system_managed_policy(platform: &str, known_drivers: &[&str]) -> Result<ManagedPolicy> {
    read_managed_policy(&PolicySources {
        platform,
        file: policy::managed_policy_path(platform),
        profile: MACOS_MANAGED_PREFERENCES_PATH.into(),
        registry: &WindowsRegistry::machine_policy(),
        lock: &MachinePolicyLock::default(),
        converter: &Plutil,
        known_drivers,
    })
}

/// The server's choice (`ManagedPolicy.ts` `make`): the machine's sources. Only a debug build
/// (`dev_build`) of a build with [`DEV_OVERRIDES`] honours the overrides: given
/// `T3CODE_DEV_MANAGED_POLICY_PATH` alone it reads that file only, as TypeScript's dev channel
/// does; given a registry key or preferences override as well, it reads that platform source
/// beside the file (the dev file when set), with the same both-exist refusal.
pub fn read_selected_managed_policy(
    platform: &str,
    dev_build: bool,
    env: &Environment,
    known_drivers: &[&str],
) -> Result<ManagedPolicy> {
    let dev = |name: &str| {
        (DEV_OVERRIDES && dev_build)
            .then(|| env.get(name))
            .flatten()
    };
    let (registry_key, preferences) = (dev(DEV_REGISTRY_KEY_ENV), dev(DEV_PREFERENCES_PATH_ENV));
    if registry_key.is_none() && preferences.is_none() {
        if let Some(path) = dev("T3CODE_DEV_MANAGED_POLICY_PATH") {
            return ManagedPolicy::load_file(path);
        }
        return read_system_managed_policy(platform, known_drivers);
    }
    let registry = registry_key.map_or_else(WindowsRegistry::machine_policy, |key| {
        WindowsRegistry::current_user(key.as_str())
    });
    read_managed_policy(&PolicySources {
        platform,
        file: policy::selected_policy_path(platform, dev_build, env),
        profile: preferences.map_or_else(|| MACOS_MANAGED_PREFERENCES_PATH.into(), PathBuf::from),
        registry: &registry,
        lock: &MachinePolicyLock::default(),
        converter: &Plutil,
        known_drivers,
    })
}

/// `readPlatformPolicy`: the policy a configuration profile or Group Policy set.
pub fn read_platform_policy(sources: &PolicySources<'_>) -> Result<Option<ManagedPolicy>> {
    match sources.platform {
        "darwin" => read_profile(sources),
        "win32" | "windows" => read_registry(sources),
        _ => Ok(None),
    }
}

fn read_profile(sources: &PolicySources<'_>) -> Result<Option<ManagedPolicy>> {
    let path = sources.profile.display().to_string();
    let exists = sources
        .profile
        .try_exists()
        .map_err(|_| refuse(&path, "the profile cannot be checked"))?;
    if !exists {
        return Ok(None);
    }
    let converted = sources
        .converter
        .convert_to_json(&sources.profile)
        .map_err(|_| refuse(&path, "the profile cannot be read"))?;
    if converted.exit_code != Some(0) {
        return Err(refuse(&path, "the profile cannot be read"));
    }
    decode_document(&path, unwrap_forced(&converted.stdout)).map(Some)
}

fn read_registry(sources: &PolicySources<'_>) -> Result<Option<ManagedPolicy>> {
    let path = sources.registry.source();
    // Not in TypeScript (`managedPolicyFile.ts:520-577` reads unlocked): a Group Policy refresh
    // rewrites the key, and a read in the middle could see a weaker policy or none.
    let _section = sources.lock.enter().map_err(|reason| {
        refuse(
            &path,
            format!(
                "the machine policy lock cannot be taken ({reason}); start again once Group \
                 Policy has finished applying"
            ),
        )
    })?;
    let read_key = |subkey: &str| {
        sources.registry.read_key(subkey).map_err(|error| {
            refuse(
                &path,
                match error {
                    RegistryError::Unavailable => {
                        "this T3 Code install cannot load its registry reader; reinstall T3 Code"
                            .to_owned()
                    }
                    RegistryError::Win32(_) => format!("the registry cannot be read ({error})"),
                },
            )
        })
    };
    let Some(root) = read_key("")? else {
        return Ok(None);
    };
    if let Some((first, second)) = same_name(root.values.iter().map(|value| value.name.as_str())) {
        return Err(refuse(
            &path,
            format!("the values {first} and {second} name the same setting; keep one"),
        ));
    }
    if let Some((first, second)) = same_name(root.subkeys.iter().map(String::as_str)) {
        return Err(refuse(
            &path,
            format!("the subkeys {first} and {second} name the same setting; keep one"),
        ));
    }
    if let Some(json) = root
        .values
        .iter()
        .find(|value| value.name.to_lowercase() == "policy")
    {
        // Not Configured clears a list policy's values but can leave its empty subkey behind.
        let mut template_settings = root.values.len() > 1;
        for subkey in &root.subkeys {
            if template_settings {
                break;
            }
            template_settings = read_key(subkey)?
                .is_some_and(|child| !child.values.is_empty() || !child.subkeys.is_empty());
        }
        if template_settings {
            return Err(refuse(
                &path,
                "it holds both a Policy value and Group Policy settings; deploy the policy one way",
            ));
        }
        let RegistryData::Text(text) = &json.data else {
            return Err(refuse(&path, "its Policy value is not a string (REG_SZ)"));
        };
        if let Some(key) = repeated_key(text) {
            return Err(refuse(
                &path,
                format!("its Policy value has the key {key:?} twice"),
            ));
        }
        return decode_document(&path, Raw::Text(text.clone())).map(Some);
    }
    let mut values: Vec<(&str, RegistryValue)> = root
        .values
        .iter()
        .map(|value| ("", value.clone()))
        .collect();
    for subkey in &root.subkeys {
        // Registry names ignore case.
        let Some((list, _)) = WINDOWS_POLICY_LISTS
            .iter()
            .find(|(name, _)| name.to_lowercase() == subkey.to_lowercase())
        else {
            return Err(refuse(
                &path,
                format!("{subkey} is not a T3 Code policy key"),
            ));
        };
        let Some(checkboxes) = read_key(subkey)? else {
            continue;
        };
        if let Some((first, second)) =
            same_name(checkboxes.values.iter().map(|value| value.name.as_str()))
        {
            return Err(refuse(
                &path,
                format!(r"{subkey}\{first} and {subkey}\{second} name the same checkbox; keep one"),
            ));
        }
        if let Some(nested) = checkboxes.subkeys.first() {
            return Err(refuse(
                &path,
                format!(r"{subkey}\{nested} is not a T3 Code policy key"),
            ));
        }
        values.extend(checkboxes.values.into_iter().map(|value| (*list, value)));
    }
    match windows_policy_document(values, sources.known_drivers) {
        Err(reason) => Err(refuse(&path, reason)),
        // An empty key, or one holding only policies set to Disabled, sets nothing.
        Ok(None) => Ok(None),
        Ok(Some(document)) => decode_document(&path, Raw::Value(document)).map(Some),
    }
}

/// A registry name as start errors print it: characters outside ASCII escaped, so two names that
/// look alike, such as `K` and KELVIN SIGN, read differently.
fn shown(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii() {
                c.to_string()
            } else {
                format!("\\u{{{:x}}}", u32::from(c))
            }
        })
        .collect()
}

/// The first two names the reader would take for one (it folds case as TypeScript's
/// `toLowerCase` does). Not in TypeScript, where the later value silently wins.
fn same_name<'a>(names: impl Iterator<Item = &'a str>) -> Option<(String, String)> {
    let mut seen: Vec<(String, &str)> = Vec::new();
    for name in names {
        let folded = name.to_lowercase();
        if let Some((_, first)) = seen.iter().find(|(other, _)| *other == folded) {
            return Some((shown(first), shown(name)));
        }
        seen.push((folded, name));
    }
    None
}

/// Walks a JSON document and records the first object key that appears twice in one object.
#[derive(Clone, Copy)]
struct UniqueKeys<'a>(&'a std::cell::RefCell<Option<String>>);
impl<'de> serde::de::DeserializeSeed<'de> for UniqueKeys<'_> {
    type Value = ();
    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> std::result::Result<(), D::Error> {
        deserializer.deserialize_any(self)
    }
}
impl<'de> serde::de::Visitor<'de> for UniqueKeys<'_> {
    type Value = ();
    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("JSON")
    }
    fn visit_bool<E>(self, _: bool) -> std::result::Result<(), E> {
        Ok(())
    }
    fn visit_i64<E>(self, _: i64) -> std::result::Result<(), E> {
        Ok(())
    }
    fn visit_u64<E>(self, _: u64) -> std::result::Result<(), E> {
        Ok(())
    }
    fn visit_f64<E>(self, _: f64) -> std::result::Result<(), E> {
        Ok(())
    }
    fn visit_str<E>(self, _: &str) -> std::result::Result<(), E> {
        Ok(())
    }
    fn visit_unit<E>(self) -> std::result::Result<(), E> {
        Ok(())
    }
    fn visit_seq<A: serde::de::SeqAccess<'de>>(
        self,
        mut seq: A,
    ) -> std::result::Result<(), A::Error> {
        while seq.next_element_seed(self)?.is_some() {}
        Ok(())
    }
    fn visit_map<A: serde::de::MapAccess<'de>>(
        self,
        mut map: A,
    ) -> std::result::Result<(), A::Error> {
        let mut seen = std::collections::HashSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if !seen.insert(key.clone()) {
                *self.0.borrow_mut() = Some(key);
                return Err(serde::de::Error::custom("a key appears twice"));
            }
            map.next_value_seed(self)?;
        }
        Ok(())
    }
}

/// The first key `json` repeats inside one object. `JSON.parse` and serde both keep the last
/// value, so a repeated restriction would be decided by its order; refused instead.
fn repeated_key(json: &str) -> Option<String> {
    use serde::de::DeserializeSeed;
    let found = std::cell::RefCell::new(None);
    let _ = UniqueKeys(&found).deserialize(&mut serde_json::Deserializer::from_str(json));
    found.into_inner()
}

fn lookup<'a>(table: &'a [(&'a str, &'a [&'a str])], name: &str) -> Option<&'a [&'a str]> {
    table
        .iter()
        .find(|(key, _)| *key == name)
        .map(|(_, path)| *path)
}

fn dword_boolean(value: &RegistryValue) -> Option<bool> {
    match value.data {
        RegistryData::Dword(0) => Some(false),
        RegistryData::Dword(1) => Some(true),
        _ => None,
    }
}

fn set_path(target: &mut Map<String, Value>, path: &[&str], value: Value) {
    let (last, parents) = path.split_last().expect("policy paths are nonempty");
    let mut node = target;
    for segment in parents {
        node = node
            .entry(*segment)
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
            .expect("no policy path is a prefix of a leaf");
    }
    node.insert((*last).into(), value);
}

/// `windowsPolicyDocument`: the document the Group Policy template's values describe, `None` when
/// they describe none, or why they do not describe one. Each value carries the list subkey it
/// sits in, or `""` for the policy key itself.
fn windows_policy_document(
    values: Vec<(&str, RegistryValue)>,
    known_drivers: &[&str],
) -> std::result::Result<Option<Value>, String> {
    // Registry names ignore case; a hand-written `organization` is still Organization.
    let values: Vec<(&str, RegistryValue)> = values
        .into_iter()
        .map(|(key, mut value)| {
            if key.is_empty()
                && let Some(name) = WINDOWS_POLICY_VALUES
                    .iter()
                    .map(|(name, ..)| *name)
                    .chain(WINDOWS_POLICY_GROUPS.iter().map(|(name, _)| *name))
                    .find(|name| name.to_lowercase() == value.name.to_lowercase())
            {
                value.name = name.into();
            }
            (key, value)
        })
        .collect();
    let mut document = Map::new();
    document.insert("schemaVersion".into(), Value::from(1));
    // Insertion-ordered, as a JavaScript Map.
    let mut lists: Vec<(&str, Vec<String>)> = Vec::new();
    let mut disabled: Vec<&[&str]> = Vec::new();
    for (key, value) in &values {
        if !key.is_empty() {
            continue;
        }
        let Some(group) = lookup(WINDOWS_POLICY_GROUPS, &value.name) else {
            continue;
        };
        match dword_boolean(value) {
            None => return Err(format!("{} must be a DWORD of 0 or 1", value.name)),
            Some(false) => disabled.push(group),
            Some(true) => {
                if let Some((name, _)) = WINDOWS_POLICY_LISTS
                    .iter()
                    .find(|(name, _)| *name == value.name)
                {
                    match lists.iter_mut().find(|(list, _)| list == name) {
                        Some((_, entries)) => entries.clear(),
                        None => lists.push((name, Vec::new())),
                    }
                }
            }
        }
    }
    let put = |document: &mut Map<String, Value>, path: &[&str], value: Value| {
        if !disabled.iter().any(|prefix| {
            prefix
                .iter()
                .enumerate()
                .all(|(i, s)| path.get(i) == Some(s))
        }) {
            set_path(document, path, value);
        }
    };
    for (key, value) in &values {
        let place = if key.is_empty() {
            value.name.clone()
        } else {
            format!(r"{key}\{}", value.name)
        };
        if !key.is_empty() {
            let Some(checked) = dword_boolean(value) else {
                return Err(format!("{place} must be a DWORD of 0 or 1"));
            };
            let index = match lists.iter().position(|(list, _)| list == key) {
                Some(index) => index,
                None => {
                    lists.push((key, Vec::new()));
                    lists.len() - 1
                }
            };
            if checked {
                let names = if *key == "Clients" {
                    POLICY_CLIENTS
                } else {
                    known_drivers
                };
                let lowered = value.name.to_lowercase();
                lists[index].1.push(
                    names
                        .iter()
                        .find(|name| name.to_lowercase() == lowered)
                        .map_or_else(|| value.name.clone(), |name| (*name).to_owned()),
                );
            }
            continue;
        }
        if lookup(WINDOWS_POLICY_GROUPS, &value.name).is_some() {
            continue;
        }
        let Some((_, field, path)) = WINDOWS_POLICY_VALUES
            .iter()
            .find(|(name, ..)| *name == value.name)
        else {
            return Err(format!("{} is not a T3 Code policy value", value.name));
        };
        if let FieldType::Boolean = field {
            let Some(flag) = dword_boolean(value) else {
                return Err(format!("{place} must be a DWORD of 0 or 1"));
            };
            put(&mut document, path, Value::Bool(flag));
            continue;
        }
        let RegistryData::Text(text) = &value.data else {
            return Err(format!("{place} must be a string (REG_SZ)"));
        };
        // Group Policy writes an empty string for a box left blank.
        if text.is_empty() {
            continue;
        }
        match field {
            FieldType::String => put(&mut document, path, Value::String(text.clone())),
            _ => {
                let Ok(parsed) = serde_json::from_str::<Value>(text) else {
                    return Err(format!("{place} is not valid JSON"));
                };
                if let Some(key) = repeated_key(text) {
                    return Err(format!("{place} has the key {key:?} twice"));
                }
                put(&mut document, path, parsed);
            }
        }
    }
    for (key, list) in lists {
        let path = lookup(WINDOWS_POLICY_LISTS, key).expect("lists are list subkeys");
        put(
            &mut document,
            path,
            Value::Array(list.into_iter().map(Value::String).collect()),
        );
    }
    // Policies set to Disabled leave values behind that restrict nothing.
    if document.len() == 1 {
        return Ok(None);
    }
    // The schema would name `organization`, which an admin cannot map to a Group Policy setting.
    if !document.contains_key("organization") {
        return Err("set the Organization policy whenever another T3 Code policy is set".into());
    }
    Ok(Some(Value::Object(document)))
}

/// A policy as JSON text, or as a value a profile already parsed.
enum Raw {
    Text(String),
    Value(Value),
}

// macOS writes `PayloadUUID` and `_manualProfile` beside the profile's own keys; no policy key
// starts with `Payload` or `_`, so dropping them cannot hide a restriction.
fn without_profile_keys(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .filter(|(key, _)| !key.starts_with("Payload") && !key.starts_with('_'))
                .collect(),
        ),
        other => other,
    }
}

/// `{ Forced: [{ mcx_preference_settings }] }` exactly: keys beside the wrapper, or more than one
/// entry, are not the wrapper and decode as a plain document, which fails.
fn forced_settings(preferences: &Value) -> Option<&Value> {
    let map = preferences.as_object().filter(|map| map.len() == 1)?;
    let [entry] = map.get("Forced")?.as_array()?.as_slice() else {
        return None;
    };
    let entry = entry.as_object().filter(|entry| entry.len() == 1)?;
    entry.get("mcx_preference_settings")
}

/// `unwrapForced`: installed managed preferences hold the profile's keys directly, but MCX-style
/// profiles keep them under `Forced[0].mcx_preference_settings`; read either.
fn unwrap_forced(json: &str) -> Raw {
    let Ok(parsed) = serde_json::from_str::<Value>(json) else {
        return Raw::Text(json.to_owned());
    };
    let preferences = without_profile_keys(parsed);
    match forced_settings(&preferences) {
        Some(settings) => Raw::Value(without_profile_keys(settings.clone())),
        None => Raw::Value(preferences),
    }
}

/// `decodeDocument`: the same strict decode the policy file gets.
fn decode_document(path: &str, raw: Raw) -> Result<ManagedPolicy> {
    let text = match raw {
        Raw::Text(text) | Raw::Value(Value::String(text)) => text,
        Raw::Value(value) => value.to_string(),
    };
    let schema_error = |error: Error| {
        let detail = match error {
            Error::Invalid { detail, .. } => detail,
            other => other.to_string(),
        };
        refuse(
            path,
            format!("it is not valid JSON or does not match the policy schema ({detail})"),
        )
    };
    let document = policy::decode(Path::new(path), &text).map_err(schema_error)?;
    let source_json = serde_json::from_str(&text).map_err(|error| schema_error(error.into()))?;
    Ok(ManagedPolicy::from_source(path, document, source_json))
}
