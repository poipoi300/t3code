use async_trait::async_trait;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use t3_contracts::editor::*;
use t3_host::{Result, editor::*};
struct Discovery {
    root: PathBuf,
    command: PathBuf,
    calls: AtomicUsize,
}
#[async_trait]
impl EditorDiscovery for Discovery {
    async fn discover(&self) -> Result<DiscoveredEditors> {
        Ok(DiscoveredEditors {
            roots: vec![self.root.clone()],
            editors: vec![DiscoveredEditor {
                id: EditorId::Vscode,
                command: self.command.clone(),
                base_args: vec![],
                style: EditorLaunchStyle::Goto,
            }],
        })
    }
    async fn launch(&self, _: EditorCommand) -> Result<()> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
#[tokio::test]
async fn refuses_undiscovered_editor_paths_traversal_and_shim_injection_before_launch() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    std::fs::create_dir(&root).unwrap();
    let outside = temp.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    let d = Arc::new(Discovery {
        root: root.clone(),
        command: PathBuf::from("C:/trusted/code.cmd"),
        calls: AtomicUsize::new(0),
    });
    let service = ExternalLauncher::new(d.clone());
    for (editor, path) in [
        (EditorId::Cursor, root.to_string_lossy().into_owned()),
        (EditorId::Vscode, outside.to_string_lossy().into_owned()),
        (EditorId::Vscode, "--help".into()),
        (EditorId::Vscode, format!("{}\n& echo bad", root.display())),
        (EditorId::Vscode, format!("{}\"&echo bad", root.display())),
        (EditorId::Vscode, format!("{}/../outside", root.display())),
    ] {
        let input = LaunchEditorInput {
            cwd: path.into(),
            editor,
            project_root: None,
            reveal: None,
        };
        assert!(
            service.launch_editor(&input).await.is_err(),
            "{}",
            input.cwd.0
        );
    }
    let input = LaunchEditorInput {
        cwd: root.to_string_lossy().into_owned().into(),
        editor: EditorId::Vscode,
        project_root: Some(Some(outside.to_string_lossy().into_owned().into())),
        reveal: None,
    };
    assert!(service.launch_editor(&input).await.is_err());
    assert_eq!(d.calls.load(Ordering::SeqCst), 0);
}
#[test]
fn arguments_preserve_positions_spaces_and_jetbrains_zero_based_columns() {
    let root = PathBuf::from("C:/project with spaces");
    let editor = DiscoveredEditor {
        id: EditorId::Idea,
        command: PathBuf::from("idea"),
        base_args: vec!["--from-discovery".into()],
        style: EditorLaunchStyle::LineColumn,
    };
    assert_eq!(
        editor_arguments(&editor, "C:/file name.rs:12:3", false, Some(&root)),
        vec![
            "--from-discovery",
            "C:/project with spaces",
            "--line",
            "12",
            "--column",
            "2",
            "C:/file name.rs"
        ]
    );
    let editor = DiscoveredEditor {
        style: EditorLaunchStyle::Goto,
        ..editor
    };
    assert_eq!(
        editor_arguments(&editor, "C:/folder", true, None),
        vec!["--from-discovery", "--new-window", "C:/folder"]
    );
    assert_eq!(
        editor_arguments(&editor, "C:/file:1", false, None),
        vec!["--from-discovery", "--goto", "C:/file:1"]
    );
}
#[tokio::test]
async fn junction_outside_discovered_root_is_refused() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    std::fs::create_dir(&root).unwrap();
    let outside = temp.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    let link = root.join("escape");
    #[cfg(windows)]
    junction::create(&outside, &link).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    let d = Arc::new(Discovery {
        root,
        command: "code".into(),
        calls: AtomicUsize::new(0),
    });
    let s = ExternalLauncher::new(d.clone());
    assert!(
        s.launch_editor(&LaunchEditorInput {
            cwd: link.to_string_lossy().into_owned().into(),
            editor: EditorId::Vscode,
            project_root: None,
            reveal: None
        })
        .await
        .is_err()
    );
    assert_eq!(d.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn mac_bundle_candidates_do_not_confuse_stable_insiders_or_antigravity_hub() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("Applications");
    for name in [
        "Visual Studio Code.app",
        "Visual Studio Code - Insiders.app",
        "IntelliJ IDEA 2026.3.app",
        "IntelliJ IDEA Ultimate.app",
        "Antigravity.app",
        "Antigravity IDE.app",
    ] {
        std::fs::create_dir_all(root.join(name)).unwrap();
    }
    let paths = editor_install_candidates(
        "darwin",
        &[("HOME".into(), temp.path().to_string_lossy().into_owned())].into(),
        EditorId::Vscode,
        "code",
        EditorLaunchStyle::Goto,
    );
    assert!(
        paths
            .iter()
            .any(|p| p.starts_with(root.join("Visual Studio Code.app")))
    );
    assert!(
        !paths
            .iter()
            .any(|p| p.starts_with(root.join("Visual Studio Code - Insiders.app")))
    );
    let paths = editor_install_candidates(
        "darwin",
        &[("HOME".into(), temp.path().to_string_lossy().into_owned())].into(),
        EditorId::Idea,
        "idea",
        EditorLaunchStyle::LineColumn,
    );
    assert!(
        paths
            .iter()
            .any(|p| p.starts_with(root.join("IntelliJ IDEA 2026.3.app")))
    );
    let paths = editor_install_candidates(
        "darwin",
        &[("HOME".into(), temp.path().to_string_lossy().into_owned())].into(),
        EditorId::Antigravity,
        "antigravity-ide",
        EditorLaunchStyle::Goto,
    );
    assert!(
        paths
            .iter()
            .any(|p| p.starts_with(root.join("Antigravity IDE.app")))
    );
    assert!(
        !paths
            .iter()
            .any(|p| p.starts_with(root.join("Antigravity.app")))
    );
}
#[tokio::test]
async fn discovered_existing_file_launches_with_position_as_discrete_arguments() {
    use tokio::sync::Mutex;
    struct Capture {
        root: PathBuf,
        commands: Mutex<Vec<EditorCommand>>,
    }
    #[async_trait]
    impl EditorDiscovery for Capture {
        async fn discover(&self) -> Result<DiscoveredEditors> {
            Ok(DiscoveredEditors {
                roots: vec![self.root.clone()],
                editors: vec![DiscoveredEditor {
                    id: EditorId::Cursor,
                    command: "trusted/cursor".into(),
                    base_args: vec!["--classic".into()],
                    style: EditorLaunchStyle::Goto,
                }],
            })
        }
        async fn launch(&self, c: EditorCommand) -> Result<()> {
            self.commands.lock().await.push(c);
            Ok(())
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("file & spaces.rs");
    std::fs::write(&file, "content").unwrap();
    let d = Arc::new(Capture {
        root: temp.path().into(),
        commands: Mutex::new(vec![]),
    });
    let s = ExternalLauncher::new(d.clone());
    s.launch_editor(&LaunchEditorInput {
        cwd: format!("{}:12:3", file.display()).into(),
        editor: EditorId::Cursor,
        project_root: None,
        reveal: None,
    })
    .await
    .unwrap();
    let commands = d.commands.lock().await;
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].command, PathBuf::from("trusted/cursor"));
    assert_eq!(
        commands[0].args,
        vec![
            "--classic".to_string(),
            "--goto".into(),
            format!("{}:12:3", dunce::canonicalize(file).unwrap().display())
        ]
    );
}

#[test]
fn windows_reveal_encodes_literal_paths_instead_of_shell_arguments() {
    use base64::Engine;
    let target = "C:\\project\\O'Connor & $file.txt";
    let command = windows_reveal_command("powershell.exe".into(), target, "C:\\project");
    assert_eq!(
        &command.args[..5],
        [
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-EncodedCommand"
        ]
    );
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&command.args[5])
        .unwrap();
    let units = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| u16::from_le_bytes([p[0], p[1]]))
        .collect::<Vec<_>>();
    let source = String::from_utf16(&units).unwrap();
    assert!(source.contains("O''Connor & $file.txt"));
    assert!(source.contains("Invoke-Item -LiteralPath 'C:\\project'"));
    assert!(source.contains("Registry::HKEY_CLASSES_ROOT\\Directory\\shell"));
}
