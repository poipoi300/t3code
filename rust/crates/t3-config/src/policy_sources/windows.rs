//! `layerWindowsRegistry`: the Win32 registry calls TypeScript makes through ffi-rs. The safe
//! wrappers do not fit a policy reader: `windows-registry` ends enumeration silently on an error,
//! which would drop a restriction, and `winreg` caps names at 2,048 characters.
#![allow(unsafe_code)]

use super::{Hive, RegistryData, RegistryError, RegistryKey, RegistryValue};
use std::ptr::{null, null_mut};
use windows_sys::Win32::{
    Foundation::{ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS},
    System::Registry::{
        HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY, REG_DWORD, REG_SZ,
        RegCloseKey, RegEnumKeyExW, RegEnumValueW, RegOpenKeyExW,
    },
};

// Value and key names are at most 16,383 characters.
const MAX_NAME_CHARS: usize = 16_384;

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

fn hive_key(hive: Hive) -> HKEY {
    match hive {
        Hive::LocalMachine => HKEY_LOCAL_MACHINE,
        Hive::CurrentUser => HKEY_CURRENT_USER,
    }
}

struct OpenKey(HKEY);
impl Drop for OpenKey {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful RegOpenKeyExW and is closed once.
        unsafe { RegCloseKey(self.0) };
    }
}

/// One `RegEnumValueW`: the result code, the name's length in characters, the type, and the
/// data's length in bytes (the length it needs when the code is `ERROR_MORE_DATA`).
fn enum_value(
    key: &OpenKey,
    index: u32,
    name: &mut [u16],
    data: &mut [u8],
) -> (u32, u32, u32, u32) {
    let (mut chars, mut kind, mut bytes) = (name.len() as u32, 0u32, data.len() as u32);
    // SAFETY: each buffer pointer is paired with its length in elements (`chars`) or bytes
    // (`bytes`), and all of them outlive the call.
    let code = unsafe {
        RegEnumValueW(
            key.0,
            index,
            name.as_mut_ptr(),
            &mut chars,
            null(),
            &mut kind,
            data.as_mut_ptr(),
            &mut bytes,
        )
    };
    (code, chars, kind, bytes)
}

/// The values and subkey names of `subkey` below `hive` in the 64-bit view; `None` when absent.
pub(super) fn read_key(hive: Hive, subkey: &str) -> Result<Option<RegistryKey>, RegistryError> {
    let path = wide(subkey);
    let mut handle: HKEY = null_mut();
    // SAFETY: `path` is NUL-terminated and outlives the call; `handle` is a valid out pointer.
    let opened = unsafe {
        RegOpenKeyExW(
            hive_key(hive),
            path.as_ptr(),
            0,
            KEY_READ | KEY_WOW64_64KEY,
            &mut handle,
        )
    };
    if opened == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    if opened != 0 {
        return Err(RegistryError::Win32(opened));
    }
    let key = OpenKey(handle);
    let mut name = vec![0u16; MAX_NAME_CHARS];
    let mut values = Vec::new();
    for index in 0u32.. {
        let mut data = vec![0u8; 64 * 1024];
        let (mut code, mut chars, mut kind, mut bytes) =
            enum_value(&key, index, &mut name, &mut data);
        // The value is larger than the buffer; the API reported the size it needs.
        if code == ERROR_MORE_DATA {
            data = vec![0u8; bytes as usize];
            (code, chars, kind, bytes) = enum_value(&key, index, &mut name, &mut data);
        }
        if code == ERROR_NO_MORE_ITEMS {
            break;
        }
        if code != 0 {
            return Err(RegistryError::Win32(code));
        }
        let value_name = String::from_utf16_lossy(&name[..chars as usize]);
        let stored = &data[..bytes as usize];
        values.push(RegistryValue {
            name: value_name,
            data: match (kind, stored.len()) {
                (REG_SZ, _) => {
                    let units: Vec<u16> = stored
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|pair| u16::from_le_bytes(*pair))
                        .collect();
                    RegistryData::Text(
                        String::from_utf16_lossy(&units)
                            .trim_end_matches('\0')
                            .to_owned(),
                    )
                }
                (REG_DWORD, 4) => RegistryData::Dword(u32::from_le_bytes([
                    stored[0], stored[1], stored[2], stored[3],
                ])),
                _ => RegistryData::Other,
            },
        });
    }
    let mut subkeys = Vec::new();
    for index in 0u32.. {
        let mut chars = MAX_NAME_CHARS as u32;
        // SAFETY: `name` holds `chars` UTF-16 units; the optional pointers are null.
        let code = unsafe {
            RegEnumKeyExW(
                key.0,
                index,
                name.as_mut_ptr(),
                &mut chars,
                null(),
                null_mut(),
                null_mut(),
                null_mut(),
            )
        };
        if code == ERROR_NO_MORE_ITEMS {
            break;
        }
        if code != 0 {
            return Err(RegistryError::Win32(code));
        }
        subkeys.push(String::from_utf16_lossy(&name[..chars as usize]));
    }
    Ok(Some(RegistryKey { values, subkeys }))
}

#[cfg(test)]
mod tests {
    use super::super::{
        PolicyRegistry, PolicySources, ProfileConverter, WindowsRegistry, read_managed_policy,
    };
    use super::*;
    use windows_sys::Win32::System::Registry::{
        REG_BINARY, REG_EXPAND_SZ, RegCreateKeyW, RegDeleteKeyW, RegDeleteTreeW, RegSetKeyValueW,
    };

    /// A key below `HKCU\Software` that exists for one test, with everything under it deleted
    /// when it drops, panicking or not. Nothing here writes outside it.
    struct TemporaryKey(String);
    impl TemporaryKey {
        fn new() -> Self {
            let key = Self(format!(
                r"Software\T3CodePolicyTest-{}",
                uuid::Uuid::new_v4()
            ));
            key.create("");
            key
        }
        fn path(&self, subkey: &str) -> Vec<u16> {
            if subkey.is_empty() {
                wide(&self.0)
            } else {
                wide(&format!(r"{}\{subkey}", self.0))
            }
        }
        fn create(&self, subkey: &str) {
            let path = self.path(subkey);
            let mut handle: HKEY = null_mut();
            // SAFETY: NUL-terminated path, valid out pointer; the handle is closed by `OpenKey`.
            let code = unsafe { RegCreateKeyW(HKEY_CURRENT_USER, path.as_ptr(), &mut handle) };
            assert_eq!(code, 0, "create {}", self.0);
            drop(OpenKey(handle));
        }
        fn set(&self, subkey: &str, name: &str, kind: u32, data: &[u8]) {
            let (path, name) = (self.path(subkey), wide(name));
            // SAFETY: NUL-terminated strings and a byte slice with its length.
            let code = unsafe {
                RegSetKeyValueW(
                    HKEY_CURRENT_USER,
                    path.as_ptr(),
                    name.as_ptr(),
                    kind,
                    data.as_ptr().cast(),
                    data.len() as u32,
                )
            };
            assert_eq!(code, 0, "set {name:?}");
        }
        fn text(&self, subkey: &str, name: &str, text: &str) {
            let bytes: Vec<u8> = wide(text).into_iter().flat_map(u16::to_le_bytes).collect();
            self.set(subkey, name, REG_SZ, &bytes);
        }
        fn dword(&self, subkey: &str, name: &str, value: u32) {
            self.set(subkey, name, REG_DWORD, &value.to_le_bytes());
        }
        fn registry(&self) -> WindowsRegistry {
            WindowsRegistry::current_user(self.0.clone())
        }
    }
    impl Drop for TemporaryKey {
        fn drop(&mut self) {
            let path = self.path("");
            // SAFETY: NUL-terminated path below HKCU\Software that this test created.
            unsafe {
                RegDeleteTreeW(HKEY_CURRENT_USER, path.as_ptr());
                RegDeleteKeyW(HKEY_CURRENT_USER, path.as_ptr());
            }
        }
    }

    struct NoConverter;
    impl ProfileConverter for NoConverter {
        fn convert_to_json(&self, _: &std::path::Path) -> std::io::Result<super::super::Converted> {
            unreachable!("Windows reads no profile")
        }
    }

    /// TypeScript's "lists registry values and subkeys through the Win32 API", read-only on HKLM.
    #[test]
    fn lists_registry_values_and_subkeys_through_the_win32_api() {
        let version = read_key(
            Hive::LocalMachine,
            r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
        )
        .unwrap()
        .unwrap();
        let value = |key: &RegistryKey, name: &str| {
            key.values
                .iter()
                .find(|value| value.name == name)
                .map(|value| value.data.clone())
        };
        assert!(
            matches!(value(&version, "ProductName"), Some(RegistryData::Text(text)) if text.contains("Windows"))
        );
        assert!(matches!(
            value(&version, "CurrentMajorVersionNumber"),
            Some(RegistryData::Dword(_))
        ));
        assert_eq!(
            read_key(Hive::LocalMachine, r"SOFTWARE\T3CodeNoSuchKey").unwrap(),
            None
        );
        let windows = read_key(
            Hive::LocalMachine,
            r"SOFTWARE\Microsoft\Windows\CurrentVersion",
        )
        .unwrap()
        .unwrap();
        // REG_EXPAND_SZ, which would otherwise expand the user's variables.
        assert_eq!(
            value(&windows, "ProgramFilesPath"),
            Some(RegistryData::Other)
        );
        let parent = read_key(Hive::LocalMachine, r"SOFTWARE\Microsoft\Windows NT")
            .unwrap()
            .unwrap();
        assert!(parent.subkeys.iter().any(|name| name == "CurrentVersion"));
    }

    #[test]
    fn reads_each_value_type_the_policy_key_can_hold_and_leaves_nothing_behind() {
        let key = TemporaryKey::new();
        let registry = key.registry();
        assert_eq!(registry.read_key("").unwrap(), Some(RegistryKey::default()));
        key.text("", "Organization", "Société 日本 ✓");
        key.dword("", "Telemetry", 0);
        key.set("", "Expand", REG_EXPAND_SZ, &[b'%', 0, 0, 0]);
        key.set("", "Binary", REG_BINARY, &[1, 2, 3]);
        // REG_DWORD is 4 bytes; anything else is not a DWORD the template wrote.
        key.set("", "WideDword", REG_DWORD, &[1, 0, 0, 0, 0, 0, 0, 0]);
        // Larger than the first 64 KiB buffer: the second read takes the size the API reports.
        let long = "x".repeat(40_000);
        key.text("", "Long", &long);
        key.create("Clients");
        key.dword("Clients", "desktop", 1);
        let root = registry.read_key("").unwrap().unwrap();
        let data = |name: &str| {
            root.values
                .iter()
                .find(|value| value.name == name)
                .map(|value| value.data.clone())
        };
        assert_eq!(
            data("Organization"),
            Some(RegistryData::Text("Société 日本 ✓".into()))
        );
        assert_eq!(data("Telemetry"), Some(RegistryData::Dword(0)));
        assert_eq!(data("Expand"), Some(RegistryData::Other));
        assert_eq!(data("Binary"), Some(RegistryData::Other));
        assert_eq!(data("WideDword"), Some(RegistryData::Other));
        assert_eq!(data("Long"), Some(RegistryData::Text(long)));
        assert_eq!(root.subkeys, ["Clients"]);
        assert_eq!(
            registry.read_key("Clients").unwrap().unwrap().values,
            [RegistryValue {
                name: "desktop".into(),
                data: RegistryData::Dword(1)
            }]
        );
        assert_eq!(registry.read_key("Missing").unwrap(), None);
        let path = key.0.clone();
        drop(key);
        assert_eq!(read_key(Hive::CurrentUser, &path).unwrap(), None);
    }

    /// The whole Windows path on a real key: Group Policy values become a document, the file
    /// beside them is refused, and the JSON `Policy` value reads the same way.
    #[test]
    fn reads_a_policy_from_a_real_registry_key() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("policy.json");
        let key = TemporaryKey::new();
        let registry = key.registry();
        let read = |registry: &WindowsRegistry| {
            read_managed_policy(&PolicySources {
                platform: "win32",
                file: file.clone(),
                profile: temp.path().join("unused.plist"),
                registry,
                converter: &NoConverter,
                known_drivers: &["codex", "claudeAgent"],
            })
        };
        key.text("", "Organization", "Acme");
        key.text("", "MaxRuntimeMode", "auto-accept-edits");
        key.dword("", "AllowedDrivers", 1);
        key.create("AllowedDrivers");
        key.dword("AllowedDrivers", "CODEX", 1);
        let policy = read(&registry).unwrap();
        assert_eq!(policy.path.display().to_string(), registry.source());
        assert_eq!(
            serde_json::to_value(policy.document.unwrap()).unwrap(),
            serde_json::json!({"schemaVersion":1,"organization":"Acme","limits":{"maxRuntimeMode":"auto-accept-edits","providers":{"allowedDrivers":["codex"]}}})
        );
        std::fs::write(&file, r#"{"schemaVersion":1,"organization":"Acme"}"#).unwrap();
        let both = read(&registry).unwrap_err().to_string();
        assert!(both.contains("deploy the policy one way"), "{both}");
        std::fs::remove_file(&file).unwrap();

        let json = TemporaryKey::new();
        json.text(
            "",
            "Policy",
            r#"{"schemaVersion":1,"organization":"Acme","limits":{"telemetry":false}}"#,
        );
        let policy = read(&json.registry()).unwrap();
        assert_eq!(
            policy
                .document
                .unwrap()
                .limits
                .and_then(|limits| limits.telemetry),
            Some(false)
        );
    }
}
