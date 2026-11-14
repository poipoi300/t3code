use std::path::{Path, PathBuf};
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invocation {
    pub command: String,
    pub args: Vec<String>,
    pub installer: Option<&'static str>,
}
impl Invocation {
    pub fn direct() -> Self {
        Self {
            command: "az".into(),
            args: vec![],
            installer: None,
        }
    }
    pub fn resolve(
        platform: &str,
        launcher: Option<&Path>,
        exists: impl Fn(&Path) -> bool,
    ) -> Self {
        if platform != "win32" {
            return Self::direct();
        }
        let Some(launcher) = launcher else {
            return Self::direct();
        };
        let Some(ext) = launcher.extension().and_then(|s| s.to_str()) else {
            return Self::direct();
        };
        if !ext.eq_ignore_ascii_case("cmd") && !ext.eq_ignore_ascii_case("bat") {
            return Self::direct();
        }
        let Some(directory) = launcher.parent() else {
            return Self::direct();
        };
        let installer = match directory
            .file_name()
            .and_then(|s| s.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("wbin") => "MSI",
            Some("bin") => "ZIP",
            _ => return Self::direct(),
        };
        let Some(root) = directory.parent() else {
            return Self::direct();
        };
        let python = root.join("python.exe");
        if !exists(
            &root
                .join("Lib")
                .join("site-packages")
                .join("azure")
                .join("cli"),
        ) || !exists(&python)
        {
            return Self::direct();
        }
        Self {
            command: python.to_string_lossy().into(),
            args: vec![
                "-X".into(),
                "utf8".into(),
                "-IBm".into(),
                "azure.cli".into(),
            ],
            installer: Some(installer),
        }
    }
    pub(super) fn detect(environment: &t3_process::Environment) -> Self {
        if !cfg!(windows) {
            return Self::direct();
        }
        let launcher =
            t3_process::resolve_command(std::ffi::OsStr::new("az"), &[], environment, true)
                .ok()
                .map(|v| PathBuf::from(v.program));
        Self::resolve("win32", launcher.as_deref(), Path::exists)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn refuses_unrelated_and_incomplete_installs_before_accepting_bundled_python() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let launcher = root.join("wbin/az.cmd");
        assert_eq!(
            Invocation::resolve("win32", Some(&launcher), |_| false),
            Invocation::direct()
        );
        assert_eq!(
            Invocation::resolve("win32", Some(&root.join("bin/az.exe")), |_| true),
            Invocation::direct()
        );
        assert_eq!(
            Invocation::resolve("win32", Some(&root.join("Scripts/az.cmd")), |_| true),
            Invocation::direct()
        );
        for missing in [
            root.join("python.exe"),
            root.join("Lib/site-packages/azure/cli"),
        ] {
            assert_eq!(
                Invocation::resolve("win32", Some(&launcher), |path| path != missing),
                Invocation::direct()
            );
        }
        assert_eq!(
            Invocation::resolve("linux", Some(&launcher), |_| panic!(
                "off Windows must not probe layout"
            )),
            Invocation::direct()
        );
        for (directory, installer) in [("wbin", "MSI"), ("bin", "ZIP")] {
            let invocation =
                Invocation::resolve("win32", Some(&root.join(directory).join("az.bat")), |_| {
                    true
                });
            assert_eq!(
                invocation.command,
                root.join("python.exe").to_string_lossy()
            );
            assert_eq!(invocation.installer, Some(installer));
            assert_eq!(invocation.args, ["-X", "utf8", "-IBm", "azure.cli"]);
        }
    }
}
