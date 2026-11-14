use std::ffi::{OsStr, OsString};
use std::path::Path;
use t3_process::{env_trampoline, supports_env_chdir};

#[test]
fn env_trampoline_preserves_argv_and_refuses_env_operands() {
    let args: Vec<OsString> = [
        "-n",
        "--",
        "a b",
        "it's \"quoted\"",
        "$HOME",
        "; exit 3",
        "",
        "A=b",
    ]
    .into_iter()
    .map(Into::into)
    .collect();
    for cwd in [None, Some(Path::new("/target/a b"))] {
        let wrapped = env_trampoline(OsStr::new("tool"), &args, cwd).expect("trampoline");
        assert_eq!(wrapped.program, "/usr/bin/env");
        let mut expected: Vec<OsString> =
            cwd.map_or_else(Vec::new, |cwd| vec!["-C".into(), cwd.as_os_str().into()]);
        expected.extend(["--".into(), "tool".into()]);
        expected.extend(args.clone());
        assert_eq!(wrapped.args, expected);
        assert!(wrapped.shell_line.is_none());
    }
    for command in ["", "-", "launch=stable", "./launch=stable"] {
        assert!(env_trampoline(OsStr::new(command), &[], None).is_none());
    }
    assert!(env_trampoline(OsStr::new("--tool"), &[], None).is_some());
}

#[tokio::test]
async fn env_capability_is_false_off_linux() {
    if !cfg!(target_os = "linux") {
        assert!(!supports_env_chdir().await);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn native_and_nonblocking_spawns_keep_cwd_argv_path_and_error_behavior()
-> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::fs::PermissionsExt;
    use t3_process::{ProcessOptions, spawn, spawn_non_blocking};
    use tokio::io::AsyncReadExt;
    let root = tempfile::tempdir()?;
    for (name, mode) in [
        ("stale/tool", 0o644),
        ("fresh/tool", 0o755),
        ("target/bin/tool", 0o755),
        ("fresh/-", 0o755),
        ("fresh/launch=stable", 0o755),
    ] {
        let path = root.path().join(name);
        std::fs::create_dir_all(path.parent().expect("parent"))?;
        std::fs::write(
            &path,
            format!("#!/bin/sh\nprintf '%s\\n' '{name}' \"$@\"\n"),
        )?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))?;
    }
    std::fs::create_dir_all(root.path().join("target/subdir"))?;
    std::os::unix::fs::symlink(root.path().join("target/subdir"), root.path().join("link"))?;
    let mut cases = vec![
        ProcessOptions::new("/bin/pwd").args(["-P"]),
        ProcessOptions::new("/usr/bin/printf")
            .args(["%s\n", "-n", "--", "a b", "$HOME", "; exit 3", "", "A=b"]),
        ProcessOptions::new("/bin/sh").args(["-c", "pwd -P; printf '%s\\n' \"$0\" 'a  b'"]),
        ProcessOptions::new("t3code-missing-command"),
        ProcessOptions::new("./missing"),
    ];
    for options in &mut cases {
        options.cwd = Some(root.path().into());
    }
    let mut environment = ProcessOptions::new("/usr/bin/env");
    environment.inherit_env = false;
    environment.cwd = Some(root.path().into());
    environment
        .env
        .insert("PATH".into(), Some("/usr/bin:/bin".into()));
    environment.env.insert("A".into(), Some("1".into()));
    environment
        .env
        .insert("BASH_FUNC_x%%".into(), Some("() { echo exported\n}".into()));
    cases.push(environment);
    for (program, path) in [
        ("tool", "stale:fresh".to_owned()),
        ("tool", "fresh".into()),
        (
            "tool",
            root.path()
                .join("link/../bin")
                .to_string_lossy()
                .into_owned(),
        ),
        ("-", "fresh".into()),
        ("launch=stable", "fresh".into()),
        ("tool", "stale".into()),
    ] {
        let mut options = ProcessOptions::new(program);
        options.cwd = Some(root.path().into());
        options.env.insert("PATH".into(), Some(path.into()));
        cases.push(options);
    }
    for cwd in [root.path().join("missing"), root.path().join("missing/..")] {
        let mut options = ProcessOptions::new("/bin/true");
        options.cwd = Some(cwd);
        cases.push(options);
    }
    async fn collect(
        mut child: t3_process::SpawnedProcess,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let mut output = Vec::new();
        if let Some(mut stdout) = child.stdout.take() {
            stdout.read_to_end(&mut output).await?;
        }
        assert!(child.wait().await?.success());
        Ok(output)
    }
    for options in cases {
        let native = spawn(&options);
        let wrapped = spawn_non_blocking(&options).await;
        match (native, wrapped) {
            (Ok(a), Ok(b)) => assert_eq!(
                collect(a).await?,
                collect(b).await?,
                "{:?}",
                options.program
            ),
            (Err(a), Err(b)) => assert_eq!(a.to_string(), b.to_string(), "{:?}", options.program),
            _ => panic!("different outcomes for {:?}", options.program),
        }
    }
    Ok(())
}
