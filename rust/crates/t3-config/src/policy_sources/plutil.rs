//! `plutil -convert json -o - <profile>`, as TypeScript's `readPlatformPolicy` runs it. plutil
//! reads binary and XML property lists alike.

use super::{Converted, process::run_with_deadline};
use std::{
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};

const PLUTIL: &str = "/usr/bin/plutil";
// A hung query must not hang startup; a timeout reads as a failed query.
const TIMEOUT: Duration = Duration::from_secs(10);

pub(super) fn convert(profile: &Path) -> std::io::Result<Converted> {
    run_with_deadline(
        Command::new(PLUTIL)
            .args(["-convert", "json", "-o", "-"])
            .arg(profile)
            .stdout(Stdio::piped()),
        TIMEOUT,
    )
}

#[cfg(test)]
mod tests {
    use super::super::{
        MachinePolicyLock, Plutil, PolicyRegistry, PolicySources, RegistryError, RegistryKey,
        read_managed_policy,
    };

    struct NoRegistry;
    impl PolicyRegistry for NoRegistry {
        fn source(&self) -> String {
            unreachable!("macOS reads no registry")
        }
        fn read_key(&self, _: &str) -> Result<Option<RegistryKey>, RegistryError> {
            unreachable!("macOS reads no registry")
        }
    }

    fn read(profile: std::path::PathBuf) -> crate::Result<crate::policy::ManagedPolicy> {
        let temp = tempfile::tempdir().unwrap();
        read_managed_policy(&PolicySources {
            platform: "darwin",
            file: temp.path().join("policy.json"),
            profile,
            registry: &NoRegistry,
            lock: &MachinePolicyLock::default(),
            converter: &Plutil,
            known_drivers: &[],
        })
    }

    /// The real plutil on the fixtures (`tests/fixtures/policy_sources`, written by Python's
    /// `plistlib`): binary and XML forms of the same policy, the MCX `Forced` wrapper, and a
    /// `schemaVersion` written as `<real>`, which TypeScript accepts as 1.
    #[test]
    fn reads_binary_and_xml_profiles_through_plutil() {
        let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("policy_sources");
        for name in [
            "profile.xml.plist",
            "profile.binary.plist",
            "forced.binary.plist",
            "real.xml.plist",
        ] {
            let limits = read(fixtures.join(name))
                .unwrap()
                .document
                .unwrap()
                .limits
                .unwrap();
            assert_eq!(
                serde_json::to_value(limits.max_runtime_mode).unwrap(),
                "auto-accept-edits",
                "{name}"
            );
        }
        // An MDM `Forced` entry carrying `mcx_data_timestamp` (a date) is refused, as in
        // TypeScript: plutil cannot put a date in JSON, and a string would not be the wrapper.
        assert!(read(fixtures.join("forced-timestamp.xml.plist")).is_err());
        let temp = tempfile::tempdir().unwrap();
        let unreadable = temp.path().join("not.plist");
        std::fs::write(&unreadable, b"\x00\x01 not a property list").unwrap();
        let error = read(unreadable).unwrap_err().to_string();
        assert!(error.contains("the profile cannot be read"), "{error}");
    }
}
