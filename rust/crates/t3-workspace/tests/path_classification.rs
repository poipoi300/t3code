//! `paths::is_network_path` and `paths::canonicalize` on Windows path spellings: a verbatim disk
//! path (`\\?\C:\…`, what the OS hands back for a path longer than `MAX_PATH`) is local; a
//! verbatim UNC, volume GUID or device path is not. Client input in any verbatim or device
//! spelling stays refused by `validate_host_path`.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use t3_workspace::paths;

#[test]
fn network_and_device_spellings_are_classified() {
    for (path, network) in [
        (r"\\server\share\x", true),
        ("//server/share/x", true),
        (r"\\?\UNC\server\share\x", true),
        ("//?/UNC/server/share/x", true),
        (r"\\?\Volume{6a4f2e10-0000-0000-0000-100000000000}\x", true),
        (r"\\.\C:\x", true),
        (r"\\.\pipe\name", true),
        (r"\\?\GLOBALROOT\Device\HarddiskVolume1\x", true),
        (r"\\?\C:\Users\me\x.js", false),
        ("//?/C:/Users/me/x.js", false),
        (r"\\?\c:\", false),
        (r"C:\Users\me\x.js", false),
        ("/home/me/x.js", false),
        ("relative/x.js", false),
    ] {
        assert_eq!(paths::is_network_path(path), network, "{path}");
    }
}

#[cfg(windows)]
#[test]
fn verbatim_and_device_input_stays_refused() {
    for path in [
        r"\\?\C:\Windows\win.ini",
        r"\\?\UNC\server\share\x",
        r"\\.\C:\Windows\win.ini",
        r"\\server\share\x",
    ] {
        assert!(
            paths::validate_host_path(std::path::Path::new(path)).is_err(),
            "{path}"
        );
    }
}

/// A file deeper than `MAX_PATH` canonicalizes to a plain drive path inside its canonical root,
/// so containment checks against a short root still hold.
#[cfg(windows)]
#[test]
fn a_local_path_longer_than_max_path_canonicalizes_inside_its_root() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("projects");
    let mut deep = root.clone();
    while deep.to_string_lossy().len() < 330 {
        deep.push("C--Users-someone-a-fairly-long-project-folder");
    }
    std::fs::create_dir_all(&deep).unwrap();
    let file = deep.join("workflow.js");
    std::fs::write(&file, "x").unwrap();
    let real = paths::canonicalize(&file).expect("a long local path is not a network path");
    assert!(
        !real.to_string_lossy().starts_with(r"\\"),
        "{}",
        real.display()
    );
    let root = paths::canonicalize(&root).unwrap();
    assert!(paths::within(&real, &root), "{}", real.display());
}
