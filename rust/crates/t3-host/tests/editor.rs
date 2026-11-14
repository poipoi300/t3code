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

/// Names a shell, PowerShell, `cmd` or a script would read as code: every PowerShell quote
/// (`'` and U+2018–U+201B), command separators, substitutions and an option-like leading dash.
fn hostile_names() -> Vec<&'static str> {
    let mut names = vec![
        "single'quote.txt",
        "left\u{2018}quote.txt",
        "right\u{2019}quote.txt",
        "low\u{201A}quote.txt",
        "reversed\u{201B}quote.txt",
        "x\u{2019}; Start-Process calc; \u{2019}y.txt",
        "semi;colon.txt",
        "amp&ersand.txt",
        "back`tick.txt",
        "dollar$(whoami).txt",
        "-leading-dash.txt",
        "comma,name.txt",
        "percent%PATH%.txt",
    ];
    // NTFS forbids `|`; newlines are refused below on every platform.
    if cfg!(unix) {
        names.push("pipe|bar.txt");
    }
    names
}

/// Records every launch; nothing is ever started.
struct Recording {
    root: PathBuf,
    editors: Vec<DiscoveredEditor>,
    commands: tokio::sync::Mutex<Vec<EditorCommand>>,
}
#[async_trait]
impl EditorDiscovery for Recording {
    async fn discover(&self) -> Result<DiscoveredEditors> {
        Ok(DiscoveredEditors {
            roots: vec![self.root.clone()],
            editors: self.editors.clone(),
        })
    }
    async fn launch(&self, c: EditorCommand) -> Result<()> {
        self.commands.lock().await.push(c);
        Ok(())
    }
}

/// "Reveal in File Explorer" and "Open" pass a file name as data: no shell, PowerShell or script
/// ever parses it, so a name such as `x’; Start-Process calc; ’y.txt` starts nothing else.
#[tokio::test]
async fn hostile_file_names_reach_the_file_manager_and_editors_as_data() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("workspace");
    std::fs::create_dir(&root).unwrap();
    let file_manager = if cfg!(windows) {
        PathBuf::from("C:\\Windows\\explorer.exe")
    } else if cfg!(target_os = "macos") {
        PathBuf::from("/usr/bin/open")
    } else {
        PathBuf::from("/usr/bin/xdg-open")
    };
    let d = Arc::new(Recording {
        root: root.clone(),
        editors: vec![
            DiscoveredEditor {
                id: EditorId::FileManager,
                command: file_manager.clone(),
                base_args: vec![],
                style: EditorLaunchStyle::DirectPath,
            },
            DiscoveredEditor {
                id: EditorId::Vscode,
                command: "trusted/code".into(),
                base_args: vec![],
                style: EditorLaunchStyle::Goto,
            },
        ],
        commands: tokio::sync::Mutex::new(vec![]),
    });
    let s = ExternalLauncher::new(d.clone());
    for name in hostile_names() {
        let file = root.join(name);
        std::fs::write(&file, "x").unwrap();
        let real = dunce::canonicalize(&file)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let launch = |editor, reveal| LaunchEditorInput {
            cwd: file.to_string_lossy().into_owned().into(),
            editor,
            project_root: None,
            reveal,
        };
        // Linux reveal first asks the desktop's file manager over D-Bus; not run here.
        if !cfg!(target_os = "linux") {
            s.launch_editor(&launch(EditorId::FileManager, Some(Some(true))))
                .await
                .unwrap();
            let reveal = d.commands.lock().await.pop().unwrap();
            assert_eq!(reveal.command, file_manager, "{name}");
            if cfg!(windows) {
                // Explorer's own `/select,"<path>"` syntax, verbatim: NTFS names cannot hold `"`.
                assert_eq!(reveal.args, vec![format!("/select,\"{real}\"")], "{name}");
                assert!(reveal.verbatim, "{name}");
            } else {
                assert_eq!(reveal.args, vec!["-R".to_string(), real.clone()], "{name}");
                assert!(!reveal.verbatim);
            }
        }
        s.launch_editor(&launch(EditorId::FileManager, None))
            .await
            .unwrap();
        let open = d.commands.lock().await.pop().unwrap();
        assert_eq!(open.command, file_manager, "{name}");
        if cfg!(windows) {
            assert_eq!(open.args, vec![format!("\"{real}\"")], "{name}");
            assert!(open.verbatim, "{name}");
        } else {
            assert_eq!(open.args, vec![real.clone()], "{name}");
        }
        s.launch_editor(&launch(EditorId::Vscode, None))
            .await
            .unwrap();
        let editor = d.commands.lock().await.pop().unwrap();
        assert_eq!(editor.command, PathBuf::from("trusted/code"));
        assert_eq!(editor.args, vec![real.clone()], "{name}");
        assert!(!editor.verbatim);
    }
    // A newline never reaches a file manager or an editor.
    for editor in [EditorId::FileManager, EditorId::Vscode] {
        let input = LaunchEditorInput {
            cwd: format!("{}\nStart-Process calc", root.join("a.txt").display()).into(),
            editor,
            project_root: None,
            reveal: Some(Some(true)),
        };
        assert!(s.launch_editor(&input).await.is_err());
    }
    assert!(d.commands.lock().await.is_empty());
}

#[tokio::test]
async fn file_manager_expands_home_and_requires_its_literal_discovered_target() {
    use tokio::sync::Mutex;
    struct Home {
        root: PathBuf,
        args: Mutex<Vec<String>>,
    }
    #[async_trait]
    impl EditorDiscovery for Home {
        async fn discover(&self) -> Result<DiscoveredEditors> {
            Ok(DiscoveredEditors {
                roots: vec![self.root.clone()],
                editors: vec![DiscoveredEditor {
                    id: EditorId::FileManager,
                    command: "trusted/file-manager".into(),
                    base_args: vec![],
                    style: EditorLaunchStyle::DirectPath,
                }],
            })
        }
        fn home_directory(&self) -> Option<PathBuf> {
            Some(self.root.clone())
        }
        async fn launch(&self, c: EditorCommand) -> Result<()> {
            *self.args.lock().await = c.args;
            Ok(())
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("notes");
    std::fs::write(&target, "notes").unwrap();
    let d = Arc::new(Home {
        root: temp.path().into(),
        args: Mutex::new(vec![]),
    });
    let s = ExternalLauncher::new(d.clone());
    s.launch_editor(&LaunchEditorInput {
        cwd: "~/notes".into(),
        editor: EditorId::FileManager,
        project_root: None,
        reveal: None,
    })
    .await
    .unwrap();
    assert_eq!(
        *d.args.lock().await,
        vec![
            dunce::canonicalize(&target)
                .unwrap()
                .to_string_lossy()
                .into_owned()
        ]
    );
    assert!(
        s.launch_editor(&LaunchEditorInput {
            cwd: "~/notes:12".into(),
            editor: EditorId::FileManager,
            project_root: None,
            reveal: None
        })
        .await
        .is_err(),
        "file manager must not interpret a filename as an editor position"
    );
}

/// Under WSL, Explorer receives one `\\wsl.localhost` location rebuilt by WSL interop, which quotes
/// only arguments with whitespace. Explorer splits at commas outside quotes, so a Linux name
/// holding `,` (or `"`, which toggles quoting, or `\`, which a Linux name cannot carry over
/// `\\wsl.localhost` faithfully) would become a second location: refused for Open and Reveal.
#[test]
fn wsl_paths_explorer_could_split_or_misread_are_refused() {
    for path in [
        "/home/me/docs,\\\\attacker.example\\x/readme.md",
        "/home/me/docs,x/readme.md",
        "/home/me/q\"uote/readme.md",
        "/home/me/back\\slash/readme.md",
        "/home/me/new\nline/readme.md",
    ] {
        for reveal in [false, true] {
            assert_eq!(
                wsl_explorer_arguments("Ubuntu", path, reveal),
                None,
                "{path:?} reveal={reveal}"
            );
        }
    }
    // Everything else Explorer reads as one location: the exact argument vector.
    let name = "/home/me/my project/a\u{2019}b;c&d$(x)`y%PATH%.md";
    let target = "\\\\wsl.localhost\\Ubuntu\\home\\me\\my project\\a\u{2019}b;c&d$(x)`y%PATH%.md";
    assert_eq!(
        wsl_explorer_arguments("Ubuntu", name, false),
        Some((target.to_owned(), vec![target.to_owned()]))
    );
    assert_eq!(
        wsl_explorer_arguments("Ubuntu", name, true),
        Some((
            target.to_owned(),
            vec!["\\\\wsl.localhost\\Ubuntu\\home\\me\\my project".to_owned()]
        ))
    );
}

/// The D-Bus `ShowItems` argument is a GVariant text literal: no backslash or quote inside it.
#[test]
fn the_dbus_show_items_argument_holds_no_escape() {
    let argument = show_items_argument("file:///home/me/a%22b/c\\d\\%22.txt");
    assert_eq!(argument, "[\"file:///home/me/a%22b/c%5Cd%5C%22.txt\"]");
    assert!(!argument[2..argument.len() - 2].contains(['\\', '"']));
}
