//! `plutil -convert json -o - <profile>`, as TypeScript's `readPlatformPolicy` runs it. plutil
//! reads binary and XML property lists alike.

use super::Converted;
use std::{
    io::Read,
    path::Path,
    process::{Command, Stdio},
    sync::mpsc,
    time::Duration,
};

const PLUTIL: &str = "/usr/bin/plutil";
// A hung query must not hang startup; a timeout reads as a failed query.
const TIMEOUT: Duration = Duration::from_secs(10);

pub(super) fn convert(profile: &Path) -> std::io::Result<Converted> {
    let mut child = Command::new(PLUTIL)
        .args(["-convert", "json", "-o", "-"])
        .arg(profile)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut stdout = child.stdout.take().expect("stdout is piped");
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = sender.send(stdout.read_to_end(&mut bytes).map(|_| bytes));
    });
    let bytes = match receiver.recv_timeout(TIMEOUT) {
        Ok(read) => read,
        Err(_) => {
            // Our own child, not yet reaped: its PID cannot have been reused.
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "plutil did not finish",
            ));
        }
    };
    let bytes = match bytes {
        Ok(bytes) => bytes,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };
    // Output has ended, so plutil is exiting; the timeout covered the run itself.
    let status = child.wait()?;
    Ok(Converted {
        stdout: String::from_utf8_lossy(&bytes).into_owned(),
        exit_code: status.code(),
    })
}

#[cfg(test)]
mod tests {
    use super::super::{
        Plutil, PolicyRegistry, PolicySources, RegistryError, RegistryKey, read_managed_policy,
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

    /// The real plutil on both property list forms of the same policy; the fixtures were written
    /// by Python's `plistlib` (`tests/fixtures/policy_sources`).
    #[test]
    fn reads_binary_and_xml_profiles_through_plutil() {
        let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("policy_sources");
        let temp = tempfile::tempdir().unwrap();
        for name in [
            "profile.xml.plist",
            "profile.binary.plist",
            "forced.binary.plist",
        ] {
            let policy = read_managed_policy(&PolicySources {
                platform: "darwin",
                file: temp.path().join("policy.json"),
                profile: fixtures.join(name),
                registry: &NoRegistry,
                converter: &Plutil,
                known_drivers: &[],
            })
            .unwrap();
            let limits = policy.document.unwrap().limits.unwrap();
            assert_eq!(
                serde_json::to_value(limits.max_runtime_mode).unwrap(),
                "auto-accept-edits",
                "{name}"
            );
        }
        let unreadable = temp.path().join("not.plist");
        std::fs::write(&unreadable, b"\x00\x01 not a property list").unwrap();
        let error = read_managed_policy(&PolicySources {
            platform: "darwin",
            file: temp.path().join("policy.json"),
            profile: unreadable,
            registry: &NoRegistry,
            converter: &Plutil,
            known_drivers: &[],
        })
        .unwrap_err()
        .to_string();
        assert!(error.contains("the profile cannot be read"), "{error}");
    }
}
