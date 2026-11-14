//! `packages/shared/src/managedPolicyFile.test.ts`, case for case. The registry and plutil are
//! fakes; the policy file and the profile are real files in a temporary directory. The real Win32
//! reader is tested in `src/policy_sources/windows.rs`, the real plutil in `plutil.rs` (macOS).

use serde_json::{Value, json};
use std::cell::Cell;
use std::{collections::BTreeMap, path::Path};
use t3_config::{
    Error,
    policy_sources::{
        Converted, PolicyLock, PolicyLockGuard, PolicyRegistry, PolicySources, ProfileConverter,
        RegistryData, RegistryError, RegistryKey, RegistryValue, WINDOWS_POLICY_KEY,
        read_managed_policy,
    },
};

const POLICY: &str = r#"{"schemaVersion":1,"organization":"Example Corp"}"#;

/// Keys below the policy key (`""` for the key itself), or the error every read fails with.
#[derive(Clone)]
enum Registry {
    Keys(BTreeMap<String, RegistryKey>),
    Fails(RegistryError),
}
impl PolicyRegistry for Registry {
    fn source(&self) -> String {
        WINDOWS_POLICY_KEY.into()
    }
    fn read_key(&self, subkey: &str) -> Result<Option<RegistryKey>, RegistryError> {
        match self {
            Self::Keys(keys) => Ok(keys.get(subkey).cloned()),
            Self::Fails(error) => Err(error.clone()),
        }
    }
}
fn no_registry() -> Registry {
    Registry::Keys(BTreeMap::new())
}

/// What plutil prints and exits with; absent is a missing command (127).
struct Plutil(Option<Converted>);
impl ProfileConverter for Plutil {
    fn convert_to_json(&self, _: &Path) -> std::io::Result<Converted> {
        Ok(self.0.clone().unwrap_or(Converted {
            stdout: String::new(),
            exit_code: Some(127),
        }))
    }
}
fn plutil(stdout: &str, exit_code: i32) -> Plutil {
    Plutil(Some(Converted {
        stdout: stdout.into(),
        exit_code: Some(exit_code),
    }))
}
fn plist(contents: Value) -> Plutil {
    plutil(&contents.to_string(), 0)
}

fn text(name: &str, data: &str) -> RegistryValue {
    RegistryValue {
        name: name.into(),
        data: RegistryData::Text(data.into()),
    }
}
fn dword(name: &str, data: u32) -> RegistryValue {
    RegistryValue {
        name: name.into(),
        data: RegistryData::Dword(data),
    }
}
fn other(name: &str) -> RegistryValue {
    RegistryValue {
        name: name.into(),
        data: RegistryData::Other,
    }
}

/// A policy key holding `values`, with checkbox subkeys of `name=0|1` entries.
fn policy_key(values: Vec<RegistryValue>, subkeys: &[(&str, &[(&str, u32)])]) -> Registry {
    let mut keys = BTreeMap::new();
    keys.insert(
        String::new(),
        RegistryKey {
            values,
            subkeys: subkeys.iter().map(|(name, _)| (*name).into()).collect(),
        },
    );
    for (name, boxes) in subkeys {
        keys.insert(
            (*name).into(),
            RegistryKey {
                values: boxes
                    .iter()
                    .map(|(box_, data)| dword(box_, *data))
                    .collect(),
                subkeys: Vec::new(),
            },
        );
    }
    Registry::Keys(keys)
}

/// The machine policy section: counts entries and exits, or cannot be taken.
#[derive(Default)]
struct Lock {
    entered: Cell<u32>,
    left: Cell<u32>,
    fails: bool,
}
impl PolicyLock for Lock {
    fn enter(&self) -> Result<PolicyLockGuard<'_>, String> {
        if self.fails {
            return Err("Windows error 1450".into());
        }
        self.entered.set(self.entered.get() + 1);
        Ok(PolicyLockGuard::new(|| self.left.set(self.left.get() + 1)))
    }
}

/// A machine: a temporary directory where the policy file and the profile may exist.
struct Machine {
    dir: tempfile::TempDir,
    lock: Lock,
}
impl Machine {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
            lock: Lock::default(),
        }
    }
    fn file(&self) -> std::path::PathBuf {
        self.dir.path().join("policy.json")
    }
    fn profile(&self) -> std::path::PathBuf {
        self.dir.path().join("com.t3tools.t3code.plist")
    }
    fn with(self, file: Option<&str>, profile: bool) -> Self {
        let _ = std::fs::remove_file(self.file());
        let _ = std::fs::remove_file(self.profile());
        if let Some(contents) = file {
            std::fs::write(self.file(), contents).unwrap();
        }
        if profile {
            std::fs::write(self.profile(), "<binary plist>").unwrap();
        }
        self
    }
    fn read_with(
        &self,
        platform: &str,
        converter: &Plutil,
        registry: &Registry,
        known_drivers: &[&str],
    ) -> Result<t3_config::policy::ManagedPolicy, Error> {
        let before = self.lock.entered.get();
        let read = read_managed_policy(&PolicySources {
            platform,
            file: self.file(),
            profile: self.profile(),
            registry,
            lock: &self.lock,
            converter,
            known_drivers,
        });
        // Review M1: every Windows read, whatever its outcome, holds the machine policy section
        // and leaves it; no other platform takes it.
        let expected = u32::from(platform == "win32" && !self.lock.fails);
        assert_eq!(self.lock.entered.get() - before, expected, "{read:?}");
        assert_eq!(self.lock.left.get(), self.lock.entered.get(), "{read:?}");
        read
    }
    /// The source read, `None` for no policy, or `error: <reason>`.
    fn read(&self, platform: &str, converter: &Plutil, registry: &Registry) -> Option<String> {
        match self.read_with(platform, converter, registry, &[]) {
            Ok(policy) => policy
                .document
                .is_some()
                .then(|| policy.path.display().to_string()),
            Err(Error::ManagedPolicy { reason, .. }) => Some(format!("error: {reason}")),
            Err(Error::Invalid { detail, .. }) => Some(format!("error: {detail}")),
            Err(other) => Some(format!("error: {other}")),
        }
    }
    /// The document read, as JSON, or `error: <reason>`.
    fn document(&self, registry: &Registry) -> Value {
        match self.read_with("win32", &Plutil(None), registry, &["codex", "claudeAgent"]) {
            Ok(policy) => policy.document.map_or(Value::Null, |document| {
                serde_json::to_value(document).unwrap()
            }),
            Err(Error::ManagedPolicy { reason, .. }) => json!(format!("error: {reason}")),
            Err(other) => json!(format!("error: {other}")),
        }
    }
}

fn includes(found: Option<String>, needle: &str) {
    let found = found.unwrap_or_default();
    assert!(found.contains(needle), "{found:?} lacks {needle:?}");
}

#[test]
fn reads_a_macos_configuration_profile_and_refuses_one_beside_a_policy_file() {
    let machine = Machine::new().with(None, true);
    let profile = machine.profile().display().to_string();
    let read =
        |machine: &Machine, converter: &Plutil| machine.read("darwin", converter, &no_registry());

    assert_eq!(read(&machine, &plutil(POLICY, 0)), Some(profile.clone()));
    let settings: Value = serde_json::from_str(POLICY).unwrap();
    // What macOS 27 writes for a manually installed profile.
    let mut manual = settings.clone();
    manual["PayloadUUID"] = json!("e8c7f49b");
    manual["_manualProfile"] = json!(true);
    assert_eq!(read(&machine, &plist(manual)), Some(profile.clone()));
    assert_eq!(
        read(
            &machine,
            &plist(json!({"Forced":[{"mcx_preference_settings":settings}]}))
        ),
        Some(profile.clone())
    );
    // Keys beside the wrapper, or a second entry, are refused rather than dropped or merged.
    for ambiguous in [
        json!({"Forced":[{"mcx_preference_settings":settings}],"limits":{"telemetry":false}}),
        json!({"Forced":[{"mcx_preference_settings":settings},{"mcx_preference_settings":settings}]}),
    ] {
        includes(
            read(&machine, &plist(ambiguous)),
            "does not match the policy schema",
        );
    }
    let machine = machine.with(Some(POLICY), false);
    assert_eq!(
        read(&machine, &Plutil(None)),
        Some(machine.file().display().to_string())
    );
    let mut typo = settings.clone();
    typo["limits"] = json!({"maxRuntimeMod": "auto"});
    let machine = machine.with(Some(&typo.to_string()), false);
    // TypeScript's schema message names `["limits"]["maxRuntimeMod"]`; the Rust decoder's
    // `limits.maxRuntimeMod`.
    includes(read(&machine, &Plutil(None)), "limits.maxRuntimeMod");
    let machine = machine.with(None, false);
    assert_eq!(read(&machine, &Plutil(None)), None);
    let machine = machine.with(Some(POLICY), true);
    includes(
        read(&machine, &plutil(POLICY, 0)),
        "deploy the policy one way",
    );
    // A profile plutil cannot convert fails closed rather than reading as no policy.
    let machine = machine.with(None, true);
    includes(read(&machine, &plutil("", 1)), "cannot be read");
    includes(read(&machine, &Plutil(None)), "cannot be read");
}

/// The policy file beside a profile names both, as TypeScript's message does.
#[test]
fn the_both_sources_refusal_names_the_platform_source_and_the_file() {
    let machine = Machine::new().with(Some(POLICY), true);
    let error = machine
        .read_with("darwin", &plutil(POLICY, 0), &no_registry(), &[])
        .unwrap_err()
        .to_string();
    assert_eq!(
        error,
        format!(
            "T3 Code cannot use the managed policy at {}: a policy file at {} also exists; deploy the policy one way. It will not start until an administrator fixes this.",
            machine.profile().display(),
            machine.file().display()
        )
    );
    let registry = policy_key(vec![text("Policy", POLICY)], &[]);
    let error = machine
        .read_with("win32", &Plutil(None), &registry, &[])
        .unwrap_err()
        .to_string();
    assert!(
        error.starts_with(&format!(
            "T3 Code cannot use the managed policy at {WINDOWS_POLICY_KEY}: a policy file at {} also exists",
            machine.file().display()
        )),
        "{error}"
    );
    // Linux reads the file only: a profile or registry there is never consulted.
    assert_eq!(
        machine.read("linux", &plutil(POLICY, 0), &registry),
        Some(machine.file().display().to_string())
    );
}

#[test]
fn reads_a_policy_value_of_json_from_the_registry_and_treats_only_an_absent_key_as_none() {
    let none = Machine::new().with(None, false);
    let file = Machine::new().with(Some(POLICY), false);
    let win32 =
        |machine: &Machine, registry: Registry| machine.read("win32", &Plutil(None), &registry);
    let json = || policy_key(vec![text("Policy", POLICY)], &[]);

    assert_eq!(win32(&none, json()), Some(WINDOWS_POLICY_KEY.into()));
    assert_eq!(win32(&none, no_registry()), None);
    assert_eq!(
        win32(&file, no_registry()),
        Some(file.file().display().to_string())
    );
    includes(win32(&file, json()), "deploy the policy one way");
    // A read that fails, here access denied, is not read as no policy, file or not.
    for machine in [&none, &file] {
        includes(
            win32(machine, Registry::Fails(RegistryError::Win32(5))),
            "the registry cannot be read (Windows error 5)",
        );
    }
    includes(
        win32(&none, Registry::Fails(RegistryError::Unavailable)),
        "cannot load its registry reader; reinstall T3 Code",
    );
    includes(
        win32(&none, policy_key(vec![other("Policy")], &[])),
        "its Policy value is not a string (REG_SZ)",
    );
    includes(
        win32(&none, policy_key(vec![text("Policy", "{ not json")], &[])),
        "does not match the policy schema",
    );
    for both in [
        policy_key(
            vec![text("Policy", POLICY), text("Organization", "Example")],
            &[],
        ),
        policy_key(
            vec![text("Policy", POLICY)],
            &[("Clients", &[("desktop", 1)])],
        ),
    ] {
        includes(
            win32(&none, both),
            "both a Policy value and Group Policy settings",
        );
    }
    // Not Configured clears a list policy's values but can leave its empty subkey behind.
    assert_eq!(
        win32(
            &none,
            policy_key(vec![text("Policy", POLICY)], &[("Clients", &[])])
        ),
        Some(WINDOWS_POLICY_KEY.into())
    );
}

#[test]
fn builds_the_policy_from_the_values_the_group_policy_template_writes() {
    let machine = Machine::new();
    let organization = || text("Organization", "Société 日本 ✓");

    assert_eq!(
        machine.document(&policy_key(
            vec![
                organization(),
                text("SupportUrl", ""),
                // Registry names ignore case.
                text("maxRuntimeMode", "auto-accept-edits"),
                dword("Telemetry", 0),
                text(
                    "Settings",
                    r#"{"enableAgentBrowserAccess": {"value": false, "locked": true}}"#
                ),
                text("UpdatesMode", "off"),
                dword("AllowNightly", 1),
                dword("Updates", 1),
                dword("Clients", 1),
                dword("AllowedDrivers", 1),
            ],
            // Registry names ignore case, so a subkey written in other case still counts.
            &[
                (
                    "clients",
                    &[("Desktop", 1), ("LOCAL-BROWSER", 1), ("REMOTE", 1)]
                ),
                (
                    "AllowedDrivers",
                    &[("Codex", 1), ("CLAUDEAGENT", 1), ("UnknownDriver", 1)]
                ),
            ],
        )),
        json!({
            "schemaVersion": 1,
            "organization": "Société 日本 ✓",
            "settings": {"enableAgentBrowserAccess": {"value": false, "locked": true}},
            "limits": {
                "maxRuntimeMode": "auto-accept-edits",
                "telemetry": false,
                "updates": {"mode": "off", "allowNightly": true},
                "clients": ["desktop", "local-browser", "remote"],
                "providers": {"allowedDrivers": ["codex", "claudeAgent", "UnknownDriver"]},
            },
        })
    );

    // Disabling a policy leaves each checkbox's falseValue behind; it restricts nothing.
    let disabled_clients: &[(&str, u32)] = &[("desktop", 0), ("local-browser", 0), ("remote", 0)];
    let disabled_drivers: &[(&str, u32)] = &[("codex", 0), ("claudeAgent", 0)];
    assert_eq!(
        machine.document(&policy_key(
            vec![
                organization(),
                dword("Clients", 0),
                dword("AllowedDrivers", 0),
                dword("Updates", 0),
                dword("AllowNightly", 0),
                dword("Telemetry", 0),
            ],
            &[
                ("Clients", disabled_clients),
                ("AllowedDrivers", disabled_drivers)
            ],
        )),
        json!({"schemaVersion": 1, "organization": "Société 日本 ✓", "limits": {"telemetry": false}})
    );
    // With nothing else configured, that is no policy rather than one missing Organization.
    assert_eq!(
        machine.document(&policy_key(
            vec![dword("AllowedDrivers", 0)],
            &[("AllowedDrivers", disabled_drivers)]
        )),
        Value::Null
    );
    assert_eq!(machine.document(&policy_key(vec![], &[])), Value::Null);

    // Anything the template does not write fails closed rather than being ignored.
    let mut nested = policy_key(
        vec![organization(), dword("Clients", 1)],
        &[("Clients", &[("desktop", 1)])],
    );
    if let Registry::Keys(keys) = &mut nested {
        keys.get_mut("Clients").unwrap().subkeys = vec!["Nested".into()];
    }
    for (registry, reason) in [
        (
            policy_key(vec![organization(), dword("Telemetry", 2)], &[]),
            "DWORD of 0 or 1",
        ),
        (
            policy_key(vec![organization(), text("MaxMode", "auto")], &[]),
            "not a T3 Code policy value",
        ),
        (
            policy_key(vec![organization()], &[("Other", &[("x", 1)])]),
            "Other is not a T3 Code policy key",
        ),
        (nested, r"Clients\Nested is not a T3 Code policy key"),
        (
            policy_key(vec![organization(), text("Settings", "{")], &[]),
            "not valid JSON",
        ),
        (
            policy_key(vec![organization(), dword("Clients", 2)], &[]),
            "DWORD of 0 or 1",
        ),
        (
            policy_key(
                vec![organization(), dword("Clients", 1)],
                &[("Clients", &[("UnknownClient", 1)])],
            ),
            "does not match the policy schema",
        ),
        (
            policy_key(
                vec![organization(), dword("Clients", 1)],
                &[("Clients", &[("desktop", 0)])],
            ),
            "does not match the policy schema",
        ),
        (
            policy_key(vec![dword("Organization", 1)], &[]),
            "Organization must be a string",
        ),
        // REG_EXPAND_SZ would fill in the reading user's variables.
        (
            policy_key(vec![other("Organization")], &[]),
            "Organization must be a string",
        ),
        (
            policy_key(vec![text("MaxRuntimeMode", "auto-accept-edits")], &[]),
            "set the Organization policy whenever another T3 Code policy is set",
        ),
    ] {
        let found = machine.document(&registry);
        assert!(
            found.as_str().is_some_and(|found| found.contains(reason)),
            "{found} lacks {reason:?}"
        );
    }
}

/// Review M1: when the machine policy section cannot be taken, start is refused rather than the
/// registry read without it.
#[test]
fn refuses_when_the_machine_policy_section_cannot_be_taken() {
    let mut machine = Machine::new();
    machine.lock.fails = true;
    let registry = policy_key(vec![text("Policy", POLICY)], &[]);
    includes(
        machine.read("win32", &Plutil(None), &registry),
        "the machine policy lock cannot be taken (Windows error 1450)",
    );
    assert_eq!(machine.read("linux", &Plutil(None), &registry), None);
}

/// Review L2: values or subkeys whose names fold to one name, and JSON that names a key twice,
/// refuse start instead of letting enumeration or parse order decide. TypeScript keeps the last.
#[test]
fn refuses_names_that_fold_together_and_repeated_json_keys() {
    let machine = Machine::new();
    let organization = || text("Organization", "Acme");
    // KELVIN SIGN folds to `k`; the registry keeps the two names apart.
    let kelvin = "Webhoo\u{212A}Tasks";
    for values in [
        vec![organization(), dword("WebhookTasks", 0), dword(kelvin, 1)],
        vec![organization(), dword(kelvin, 1), dword("WebhookTasks", 0)],
    ] {
        let found = machine.document(&policy_key(values, &[]));
        let found = found.as_str().unwrap_or_default();
        assert!(
            found.contains("WebhookTasks") && found.contains(r"Webhoo\u{212a}Tasks"),
            "{found}"
        );
    }
    let mut subkeys = policy_key(vec![organization(), dword("Clients", 1)], &[]);
    if let Registry::Keys(keys) = &mut subkeys {
        keys.get_mut("").unwrap().subkeys = vec!["Clients".into(), "clients".into()];
    }
    let drivers = policy_key(
        vec![organization(), dword("AllowedDrivers", 1)],
        &[("AllowedDrivers", &[("kiro", 1), ("\u{212A}iro", 0)])],
    );
    for (registry, names) in [
        (subkeys, ["Clients", "clients"]),
        (drivers, ["kiro", r"\u{212a}iro"]),
    ] {
        let found = machine.document(&registry);
        let found = found.as_str().unwrap_or_default();
        assert!(
            found.contains(names[0]) && found.contains(names[1]),
            "{found}"
        );
    }
    let twice = r#"{"schemaVersion":1,"organization":"Acme","limits":{"maxRuntimeMode":"approval-required","maxRuntimeMode":"full-access"}}"#;
    for (registry, place) in [
        (policy_key(vec![text("Policy", twice)], &[]), "Policy"),
        (
            policy_key(
                vec![
                    organization(),
                    text(
                        "Settings",
                        r#"{"enableAgentBrowserAccess":{"value":false,"locked":true},"enableAgentBrowserAccess":{"value":true,"locked":true}}"#,
                    ),
                ],
                &[],
            ),
            "Settings",
        ),
        (
            policy_key(
                vec![
                    organization(),
                    text("ProviderInstances", r#"{"codex":{},"codex":{}}"#),
                ],
                &[],
            ),
            "ProviderInstances",
        ),
    ] {
        let found = machine.document(&registry);
        let found = found.as_str().unwrap_or_default();
        assert!(found.contains(place) && found.contains("twice"), "{found}");
    }
}

/// Shapes real managed Macs write, as TypeScript reads them (checked by running its reader on
/// the same plutil output): a `<real>` number is a JSON number like any other, so
/// `schemaVersion` 1.0 is 1; a `Forced` entry carrying `mcx_data_timestamp` is not the wrapper
/// and is refused.
#[test]
fn reads_macos_numbers_written_as_reals_and_pins_the_timestamped_wrapper() {
    let machine = Machine::new().with(None, true);
    let real = r#"{"schemaVersion":1.0,"organization":"Acme","limits":{"maxRuntimeMode":"auto-accept-edits"}}"#;
    let policy = machine
        .read_with("darwin", &plutil(real, 0), &no_registry(), &[])
        .unwrap();
    assert_eq!(
        serde_json::to_value(policy.document.unwrap()).unwrap(),
        json!({"schemaVersion":1,"organization":"Acme","limits":{"maxRuntimeMode":"auto-accept-edits"}})
    );
    includes(
        machine.read(
            "darwin",
            &plutil(r#"{"schemaVersion":1.5,"organization":"Acme"}"#, 0),
            &no_registry(),
        ),
        "does not match the policy schema",
    );
    // JSON.parse reads a policy file's 1.0 as 1 too.
    let machine = machine.with(
        Some(r#"{"schemaVersion":1.0,"organization":"Acme"}"#),
        false,
    );
    assert_eq!(
        machine.read("linux", &Plutil(None), &no_registry()),
        Some(machine.file().display().to_string())
    );
    let machine = machine.with(None, true);
    let stamped = json!({"Forced":[{"mcx_data_timestamp":"2026-01-01T00:00:00Z","mcx_preference_settings":{"schemaVersion":1,"organization":"Acme"}}]});
    includes(
        machine.read("darwin", &plist(stamped), &no_registry()),
        "does not match the policy schema",
    );
}
