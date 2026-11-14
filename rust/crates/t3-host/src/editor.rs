use crate::{Error, Result};
use async_trait::async_trait;
use std::{
    path::{Component, Path, PathBuf},
    sync::Arc,
};
use t3_contracts::editor::*;
#[derive(Debug, Clone)]
pub struct DiscoveredEditor {
    pub id: EditorId,
    pub command: PathBuf,
    pub base_args: Vec<String>,
    pub style: EditorLaunchStyle,
}
pub struct DiscoveredEditors {
    pub roots: Vec<PathBuf>,
    pub editors: Vec<DiscoveredEditor>,
}
#[derive(Debug)]
pub struct EditorCommand {
    pub command: PathBuf,
    pub args: Vec<String>,
}
#[async_trait]
pub trait EditorDiscovery: Send + Sync {
    /// The same discovery used for server.config: trusted executables and server-owned workspace roots.
    /// Unavailable workspace/discovery reads must fail, never return a client-provided fallback.
    async fn discover(&self) -> Result<DiscoveredEditors>;
    fn home_directory(&self) -> Option<PathBuf> {
        std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
    }
    async fn launch(&self, command: EditorCommand) -> Result<()> {
        launch_native(command).await
    }
}
#[async_trait]
pub trait EditorRoots: Send + Sync {
    async fn roots(&self) -> Result<Vec<PathBuf>>;
}
pub struct SystemEditorDiscovery {
    roots: Arc<dyn EditorRoots>,
    cache: tokio::sync::Mutex<Option<(tokio::time::Instant, Vec<DiscoveredEditor>)>>,
}
impl SystemEditorDiscovery {
    pub fn new(roots: Arc<dyn EditorRoots>) -> Self {
        Self {
            roots,
            cache: tokio::sync::Mutex::new(None),
        }
    }
}
#[async_trait]
impl EditorDiscovery for SystemEditorDiscovery {
    async fn discover(&self) -> Result<DiscoveredEditors> {
        let roots = self.roots.roots().await?;
        let mut cache = self.cache.lock().await;
        if let Some((expires, editors)) = &*cache
            && *expires > tokio::time::Instant::now()
        {
            return Ok(DiscoveredEditors {
                roots,
                editors: editors.clone(),
            });
        }
        let mut editors = Vec::new();
        use EditorId::*;
        let definitions = [
            (
                Cursor,
                vec!["cursor"],
                EditorLaunchStyle::Goto,
                vec!["--classic"],
            ),
            (Trae, vec!["trae"], EditorLaunchStyle::Goto, vec![]),
            (Kiro, vec!["kiro"], EditorLaunchStyle::Goto, vec!["ide"]),
            (Vscode, vec!["code"], EditorLaunchStyle::Goto, vec![]),
            (
                VscodeInsiders,
                vec!["code-insiders"],
                EditorLaunchStyle::Goto,
                vec![],
            ),
            (Vscodium, vec!["codium"], EditorLaunchStyle::Goto, vec![]),
            (
                Zed,
                vec!["zed", "zeditor"],
                EditorLaunchStyle::DirectPath,
                vec![],
            ),
            (
                Antigravity,
                vec!["antigravity-ide", "agy-ide"],
                EditorLaunchStyle::Goto,
                vec![],
            ),
            (Idea, vec!["idea"], EditorLaunchStyle::LineColumn, vec![]),
            (Aqua, vec!["aqua"], EditorLaunchStyle::LineColumn, vec![]),
            (Clion, vec!["clion"], EditorLaunchStyle::LineColumn, vec![]),
            (
                Datagrip,
                vec!["datagrip"],
                EditorLaunchStyle::LineColumn,
                vec![],
            ),
            (
                Dataspell,
                vec!["dataspell"],
                EditorLaunchStyle::LineColumn,
                vec![],
            ),
            (
                Goland,
                vec!["goland"],
                EditorLaunchStyle::LineColumn,
                vec![],
            ),
            (
                Phpstorm,
                vec!["phpstorm"],
                EditorLaunchStyle::LineColumn,
                vec![],
            ),
            (
                Pycharm,
                vec!["pycharm"],
                EditorLaunchStyle::LineColumn,
                vec![],
            ),
            (Rider, vec!["rider"], EditorLaunchStyle::LineColumn, vec![]),
            (
                Rubymine,
                vec!["rubymine"],
                EditorLaunchStyle::LineColumn,
                vec![],
            ),
            (
                Rustrover,
                vec!["rustrover"],
                EditorLaunchStyle::LineColumn,
                vec![],
            ),
            (
                Webstorm,
                vec!["webstorm"],
                EditorLaunchStyle::LineColumn,
                vec![],
            ),
        ];
        for (id, commands, style, base_args) in definitions {
            let platform = if cfg!(windows) {
                "win32"
            } else if cfg!(target_os = "macos") {
                "darwin"
            } else {
                "linux"
            };
            let env = std::env::vars_os()
                .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
                .collect();
            let prefer_bundle = platform == "darwin" && id == Zed;
            let prefer_zeditor = platform == "linux" && id == Zed;
            let direct = if prefer_bundle || prefer_zeditor {
                None
            } else {
                commands.iter().find_map(|name| find_command(name))
            };
            let mut candidates = editor_install_candidates(platform, &env, id, commands[0], style);
            if prefer_bundle {
                candidates.extend(commands.iter().map(PathBuf::from));
            }
            let mut fallback = None;
            if direct.is_none() {
                for candidate in candidates {
                    let Some(command) = find_command(&candidate.to_string_lossy()) else {
                        continue;
                    };
                    if prefer_zeditor && candidate.file_name().is_some_and(|n| n == "zed") {
                        let mut options = t3_process::ProcessOptions::new(command.as_os_str())
                            .args(["--version"]);
                        options.timeout = Some(std::time::Duration::from_secs(2));
                        options.max_output_bytes = 64 * 1024;
                        if !t3_process::run(options)
                            .await
                            .is_ok_and(|r| r.code == Some(0) && r.stdout.starts_with(b"Zed"))
                        {
                            continue;
                        }
                    }
                    fallback = Some(command);
                    break;
                }
            }
            let from_bundle = direct.is_none();
            if let Some(command) = direct.or(fallback) {
                editors.push(DiscoveredEditor {
                    id,
                    command,
                    style,
                    base_args: if from_bundle && id == Kiro && platform != "linux" {
                        vec![]
                    } else {
                        base_args.into_iter().map(str::to_owned).collect()
                    },
                });
            }
        }
        let graphical = std::env::var_os("DISPLAY").is_some()
            || std::env::var_os("WAYLAND_DISPLAY").is_some()
            || !graphical_session_env().await.is_empty();
        let mut manager = if cfg!(windows) {
            find_command("explorer.exe")
        } else if cfg!(target_os = "macos") {
            find_command("open")
        } else {
            wsl_distribution().and_then(|_| find_command("explorer.exe"))
        };
        if manager.is_none()
            && cfg!(target_os = "linux")
            && graphical
            && let (Some(open), Some(mime)) = (find_command("xdg-open"), find_command("xdg-mime"))
        {
            let mut options = t3_process::ProcessOptions::new(mime.as_os_str()).args([
                "query",
                "default",
                "inode/directory",
            ]);
            options.timeout = Some(std::time::Duration::from_secs(2));
            options.max_output_bytes = 64 * 1024;
            if t3_process::run(options)
                .await
                .is_ok_and(|r| r.code == Some(0) && !r.stdout_text().trim().is_empty())
            {
                manager = Some(open);
            }
        }
        if let Some(command) = manager {
            editors.push(DiscoveredEditor {
                id: FileManager,
                command,
                style: EditorLaunchStyle::DirectPath,
                base_args: vec![],
            });
        }
        *cache = Some((
            tokio::time::Instant::now() + std::time::Duration::from_secs(60),
            editors.clone(),
        ));
        Ok(DiscoveredEditors { roots, editors })
    }
}
fn find_command(name: &str) -> Option<PathBuf> {
    if Path::new(name).is_absolute() {
        return Path::new(name).is_file().then(|| PathBuf::from(name));
    }
    #[cfg(windows)]
    return t3_process::resolve_command(std::ffi::OsStr::new(name), &[], &Default::default(), true)
        .ok()
        .map(|r| r.program.into());
    #[cfg(not(windows))]
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|p| p.join(name))
            .find(|p| p.is_file())
    })
}
pub struct ExternalLauncher {
    discovery: Arc<dyn EditorDiscovery>,
}
impl ExternalLauncher {
    pub fn new(discovery: Arc<dyn EditorDiscovery>) -> Self {
        Self { discovery }
    }
    pub async fn available_editors(&self) -> Result<Vec<EditorId>> {
        Ok(self
            .discovery
            .discover()
            .await?
            .editors
            .into_iter()
            .map(|e| e.id)
            .collect())
    }
    pub async fn launch_editor(
        &self,
        input: &LaunchEditorInput,
    ) -> std::result::Result<(), Box<ExternalLauncherError>> {
        let unsupported = || {
            Box::new(ExternalLauncherError::UnsupportedTarget(
                ExternalLauncherUnsupportedTargetError::ExternalLauncherUnsupportedTargetError {
                    editor: input.editor,
                },
            ))
        };
        let discovered = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            self.discovery.discover(),
        )
        .await
        .map_err(|_| unsupported())?
        .map_err(|_| unsupported())?;
        let editor=discovered.editors.iter().find(|e|e.id==input.editor).ok_or_else(||Box::new(ExternalLauncherError::UnsupportedEditor(ExternalLauncherUnsupportedEditorError::ExternalLauncherUnsupportedEditorError{editor:input.editor})))?;
        let expanded = if input.editor == EditorId::FileManager
            && (input.cwd.0.starts_with("~/") || input.cwd.0.starts_with("~\\"))
        {
            self.discovery
                .home_directory()
                .map(|home| home.join(&input.cwd.0[2..]).to_string_lossy().into_owned())
                .ok_or_else(unsupported)?
        } else {
            input.cwd.0.clone()
        };
        let (raw_path, position) = if input.editor == EditorId::FileManager {
            (expanded.as_str(), None)
        } else {
            split_position(&expanded)
        };
        let path = checked_path(raw_path, &discovered.roots).map_err(|_| unsupported())?;
        let project_root = input
            .project_root
            .as_ref()
            .and_then(|p| p.as_ref())
            .map(|p| checked_path(&p.0, &discovered.roots))
            .transpose()
            .map_err(|_| unsupported())?;
        if project_root
            .as_ref()
            .is_some_and(|root| !contains(root, &path))
        {
            return Err(unsupported());
        }
        let mut target = path.to_string_lossy().into_owned();
        if let Some((line, column)) = position {
            target.push(':');
            target.push_str(line);
            if let Some(c) = column {
                target.push(':');
                target.push_str(c);
            }
        }
        let mut command = editor.command.clone();
        let mut args = editor_arguments(editor, &target, path.is_dir(), project_root.as_deref());
        if input.editor == EditorId::FileManager {
            let distro = if cfg!(target_os = "linux")
                && editor
                    .command
                    .file_name()
                    .is_some_and(|n| n == "explorer.exe")
            {
                wsl_distribution()
            } else {
                None
            };
            if let Some(distro) = &distro {
                target = format!(
                    "\\\\wsl.localhost\\{distro}{}",
                    path.to_string_lossy().replace('/', "\\")
                );
                args = vec![target.clone()];
            }
            if input.reveal == Some(Some(true)) {
                if cfg!(target_os = "macos") {
                    args = vec!["-R".into(), target.clone()];
                } else if cfg!(windows) || distro.is_some() {
                    let parent = if let Some(distro) = &distro {
                        format!(
                            "\\\\wsl.localhost\\{distro}{}",
                            path.parent()
                                .ok_or_else(unsupported)?
                                .to_string_lossy()
                                .replace('/', "\\")
                        )
                    } else {
                        path.parent()
                            .ok_or_else(unsupported)?
                            .to_string_lossy()
                            .into_owned()
                    };
                    let powershell = if cfg!(windows) {
                        let system_root = std::env::var_os("SYSTEMROOT")
                            .or_else(|| std::env::var_os("windir"))
                            .unwrap_or_else(|| "C:\\Windows".into());
                        Some(
                            PathBuf::from(system_root)
                                .join("System32/WindowsPowerShell/v1.0/powershell.exe"),
                        )
                    } else {
                        find_command("powershell.exe")
                    };
                    if let Some(powershell) = powershell {
                        let reveal = windows_reveal_command(powershell, &target, &parent);
                        command = reveal.command;
                        args = reveal.args;
                    } else {
                        args = vec![parent];
                    }
                } else {
                    if try_reveal_linux(&path).await {
                        return Ok(());
                    }
                    args = vec![
                        path.parent()
                            .ok_or_else(unsupported)?
                            .to_string_lossy()
                            .into_owned(),
                    ];
                }
            }
        }
        let shim = editor.command.extension().is_some_and(|e| {
            matches!(
                e.to_string_lossy().to_ascii_lowercase().as_str(),
                "cmd" | "bat"
            )
        });
        if args.iter().any(|a| {
            a.chars()
                .any(|c| c == '\0' || shim && (c.is_control() || c == '"'))
        }) {
            return Err(unsupported());
        }
        self.discovery
            .launch(EditorCommand {
                command: command.clone(),
                args: args.clone(),
            })
            .await
            .map_err(|_| {
                Box::new(ExternalLauncherError::EditorSpawn(
                    ExternalLauncherEditorSpawnError::ExternalLauncherEditorSpawnError {
                        command: command.to_string_lossy().into_owned(),
                        args,
                        cause: serde_json::Value::Null,
                        editor: input.editor,
                        target,
                    },
                ))
            })
    }
}
fn contains(root: &Path, path: &Path) -> bool {
    #[cfg(windows)]
    {
        let root = PathBuf::from(root.to_string_lossy().to_lowercase());
        let path = PathBuf::from(path.to_string_lossy().to_lowercase());
        path.starts_with(root)
    }
    #[cfg(not(windows))]
    path.starts_with(root)
}
fn checked_path(raw: &str, roots: &[PathBuf]) -> Result<PathBuf> {
    let path = Path::new(raw);
    if raw.chars().any(|c| c.is_control())
        || !path.is_absolute()
        || path.components().any(|p| matches!(p, Component::ParentDir))
        || (raw.starts_with("//") || raw.starts_with("\\\\"))
    {
        return Err(Error::Invalid);
    }
    let real = dunce::canonicalize(path).map_err(|_| Error::Io)?;
    if !roots
        .iter()
        .filter_map(|r| dunce::canonicalize(r).ok())
        .any(|r| contains(&r, &real))
    {
        return Err(Error::Refused("target was not discovered"));
    }
    Ok(real)
}
fn split_position(target: &str) -> (&str, Option<(&str, Option<&str>)>) {
    let Some((head, tail)) = target.rsplit_once(':') else {
        return (target, None);
    };
    if tail.is_empty() || !tail.bytes().all(|b| b.is_ascii_digit()) {
        return (target, None);
    }
    if let Some((path, line)) = head.rsplit_once(':')
        && !line.is_empty()
        && line.bytes().all(|b| b.is_ascii_digit())
    {
        return (path, Some((line, Some(tail))));
    }
    (head, Some((tail, None)))
}
pub fn editor_arguments(
    editor: &DiscoveredEditor,
    target: &str,
    is_directory: bool,
    project_root: Option<&Path>,
) -> Vec<String> {
    let mut args = editor.base_args.clone();
    let (path, position) = split_position(target);
    match editor.style {
        EditorLaunchStyle::DirectPath => args.push(target.into()),
        EditorLaunchStyle::Goto => {
            if position.is_some() {
                args.push("--goto".into());
            } else if is_directory {
                args.push("--new-window".into());
            }
            args.push(target.into());
        }
        EditorLaunchStyle::LineColumn => {
            if let Some((line, column)) = position {
                if let Some(root) = project_root {
                    args.push(root.to_string_lossy().into_owned());
                }
                args.extend(["--line".into(), line.into()]);
                if let Some(column) = column {
                    args.extend([
                        "--column".into(),
                        column
                            .parse::<u64>()
                            .unwrap_or(u64::MAX)
                            .saturating_sub(1)
                            .to_string(),
                    ]);
                }
                args.push(path.into());
            } else {
                args.push(target.into());
            }
        }
    }
    args
}
pub async fn launch_native(command: EditorCommand) -> Result<()> {
    let mut options =
        t3_process::ProcessOptions::new(command.command.as_os_str()).args(command.args);
    options.timeout = None;
    for (key, value) in graphical_session_env().await {
        options.env.insert(key.into(), Some(value.into()));
    }
    options.env.insert("ELECTRON_RUN_AS_NODE".into(), None);
    let mut child = t3_process::spawn(&options).map_err(|_| Error::Service)?;
    child.stdin.take();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    tokio::spawn(async move {
        let out = async move {
            if let Some(mut stream) = stdout {
                let _ = tokio::io::copy(&mut stream, &mut tokio::io::sink()).await;
            }
        };
        let err = async move {
            if let Some(mut stream) = stderr {
                let _ = tokio::io::copy(&mut stream, &mut tokio::io::sink()).await;
            }
        };
        let _ = tokio::join!(child.wait(), out, err);
    });
    Ok(())
}

pub fn editor_install_candidates(
    platform: &str,
    env: &std::collections::BTreeMap<String, String>,
    id: EditorId,
    command: &str,
    style: EditorLaunchStyle,
) -> Vec<PathBuf> {
    use EditorId::*;
    let label = match id {
        Cursor => "Cursor",
        Trae => "Trae",
        Kiro => "Kiro",
        Vscode => "VS Code",
        VscodeInsiders => "VS Code Insiders",
        Vscodium => "VSCodium",
        Zed => "Zed",
        Antigravity => "Antigravity",
        Idea => "IntelliJ IDEA",
        Aqua => "Aqua",
        Clion => "CLion",
        Datagrip => "DataGrip",
        Dataspell => "DataSpell",
        Goland => "GoLand",
        Phpstorm => "PhpStorm",
        Pycharm => "PyCharm",
        Rider => "Rider",
        Rubymine => "RubyMine",
        Rustrover => "RustRover",
        Webstorm => "WebStorm",
        FileManager => "File Manager",
    };
    let names = match id {
        Vscode => vec!["Visual Studio Code"],
        VscodeInsiders => vec!["Visual Studio Code - Insiders"],
        Antigravity => vec!["Antigravity IDE"],
        Idea => vec![
            "IntelliJ IDEA",
            "IntelliJ IDEA CE",
            "IntelliJ IDEA Ultimate",
        ],
        Pycharm => vec!["PyCharm", "PyCharm CE"],
        Rider => vec!["Rider", "JetBrains Rider"],
        _ => vec![label],
    };
    let home = env.get("HOME").map(PathBuf::from);
    let jetbrains = style == EditorLaunchStyle::LineColumn;
    let mut candidates = Vec::new();
    if platform == "darwin" {
        let mut roots = home
            .as_ref()
            .map(|p| vec![p.join("Applications")])
            .unwrap_or_default();
        roots.push("/Applications".into());
        for root in roots {
            let mut bundles = names.iter().map(|n| format!("{n}.app")).collect::<Vec<_>>();
            if let Ok(entries) = std::fs::read_dir(&root) {
                let mut entries = entries.flatten().collect::<Vec<_>>();
                entries.sort_by_key(|entry| entry.file_name());
                for entry in entries {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if names.iter().any(|n| {
                        name.strip_prefix(&format!("{n} "))
                            .and_then(|s| s.strip_suffix(".app"))
                            .is_some_and(|s| {
                                s.starts_with(|c: char| c.is_ascii_digit())
                                    && s.bytes().all(|b| b.is_ascii_digit() || b == b'.')
                            })
                    }) && !bundles.contains(&name)
                    {
                        bundles.push(name);
                    }
                }
            }
            for bundle in bundles {
                let contents = root.join(bundle).join("Contents");
                if jetbrains || id == Zed {
                    candidates.push(contents.join("MacOS").join(if id == Zed {
                        "cli"
                    } else {
                        command
                    }));
                } else {
                    candidates.extend([
                        contents.join("Resources/app/bin").join(command),
                        contents.join("Resources/app/bin/code"),
                    ]);
                }
            }
        }
        if jetbrains && let Some(home) = home {
            candidates.push(
                home.join("Library/Application Support/JetBrains/Toolbox/scripts")
                    .join(command),
            );
        }
    } else if platform == "win32" {
        let mut roots = env
            .get("LOCALAPPDATA")
            .map(|p| vec![PathBuf::from(p).join("Programs")])
            .unwrap_or_default();
        roots.extend(
            ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"]
                .iter()
                .filter_map(|k| env.get(*k))
                .map(PathBuf::from),
        );
        if jetbrains {
            if let Some(local) = env.get("LOCALAPPDATA") {
                candidates.push(
                    PathBuf::from(local)
                        .join("JetBrains/Toolbox/scripts")
                        .join(format!("{command}.cmd")),
                );
            }
            for root in roots
                .into_iter()
                .flat_map(|r| [r.clone(), r.join("JetBrains")])
            {
                if let Ok(entries) = std::fs::read_dir(&root) {
                    for entry in entries.flatten() {
                        let name = entry.file_name().to_string_lossy().into_owned();
                        if names
                            .iter()
                            .any(|n| name == *n || name.starts_with(&format!("{n} ")))
                        {
                            candidates.extend([
                                entry.path().join("bin").join(format!("{command}64.exe")),
                                entry.path().join("bin").join(format!("{command}.exe")),
                            ]);
                        }
                    }
                }
            }
        } else {
            let name = match id {
                Vscode => "Microsoft VS Code",
                VscodeInsiders => "Microsoft VS Code Insiders",
                _ => label,
            };
            for root in roots {
                let root = root.join(name);
                candidates.extend([
                    root.join("resources/app/bin")
                        .join(format!("{command}.cmd")),
                    root.join("resources/app/bin/code.cmd"),
                    root.join("bin").join(format!("{command}.cmd")),
                    root.join("bin/code.cmd"),
                ]);
                if id == Zed {
                    candidates.extend([root.join("bin/zed.exe"), root.join("zed.exe")]);
                }
            }
        }
    } else {
        let mut dirs = home
            .as_ref()
            .map(|p| vec![p.join(".local/bin")])
            .unwrap_or_default();
        dirs.extend(["/usr/local/bin", "/usr/bin", "/snap/bin"].map(PathBuf::from));
        if jetbrains
            && let Some(data) = env
                .get("XDG_DATA_HOME")
                .map(PathBuf::from)
                .or_else(|| home.map(|p| p.join(".local/share")))
        {
            dirs.push(data.join("JetBrains/Toolbox/scripts"));
        }
        if id == Zed {
            for name in ["zeditor", "zed"] {
                candidates.push(name.into());
                candidates.extend(dirs.iter().map(|d| d.join(name)));
            }
        } else {
            candidates.extend(dirs.iter().map(|d| d.join(command)));
            if id == Antigravity {
                candidates.extend(dirs.iter().map(|d| d.join("agy-ide")));
            }
        }
    }
    candidates
}

fn wsl_distribution() -> Option<String> {
    if std::env::var_os("SSH_CONNECTION").is_some()
        || std::env::var_os("SSH_TTY").is_some()
        || std::env::var_os("container").is_some()
    {
        return None;
    }
    std::env::var("WSL_DISTRO_NAME")
        .ok()
        .filter(|s| !s.trim().is_empty())
}
pub fn windows_reveal_command(powershell: PathBuf, target: &str, parent: &str) -> EditorCommand {
    use base64::Engine;
    let literal = |value: &str| format!("'{}'", value.replace("'", "''"));
    let source = format!(
        "$ProgressPreference = 'SilentlyContinue'; $folderVerb = (Get-ItemProperty -LiteralPath 'Registry::HKEY_CLASSES_ROOT\\Directory\\shell' -ErrorAction SilentlyContinue).'(default)'; if ($folderVerb -and $folderVerb -ne 'none') {{ Invoke-Item -LiteralPath {} }} else {{ Start-Process 'explorer.exe' -ArgumentList ('/select,\"' + {} + '\"') }}",
        literal(parent),
        literal(target)
    );
    let bytes = source
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    EditorCommand {
        command: powershell,
        args: vec![
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-ExecutionPolicy".into(),
            "Bypass".into(),
            "-EncodedCommand".into(),
            base64::engine::general_purpose::STANDARD.encode(bytes),
        ],
    }
}
async fn graphical_session_env() -> std::collections::BTreeMap<String, String> {
    let mut env = std::collections::BTreeMap::new();
    if !cfg!(target_os = "linux")
        || std::env::var_os("DISPLAY").is_some()
        || std::env::var_os("WAYLAND_DISPLAY").is_some()
    {
        return env;
    }
    let mut options = t3_process::ProcessOptions::new("busctl").args([
        "--user",
        "--json=short",
        "get-property",
        "org.freedesktop.systemd1",
        "/org/freedesktop/systemd1",
        "org.freedesktop.systemd1.Manager",
        "Environment",
    ]);
    options.timeout = Some(std::time::Duration::from_secs(2));
    options.max_output_bytes = 256 * 1024;
    if let Ok(output) = t3_process::run(options).await
        && let Ok(value) = serde_json::from_slice::<serde_json::Value>(&output.stdout)
        && let Some(data) = value["data"].as_array()
    {
        for entry in data {
            if let Some((key, value)) = entry.as_str().and_then(|s| s.split_once('='))
                && ["DISPLAY", "WAYLAND_DISPLAY", "XAUTHORITY"].contains(&key)
            {
                env.insert(key.into(), value.into());
            }
        }
    }
    env
}
async fn try_reveal_linux(path: &Path) -> bool {
    let Some(command) = find_command("gdbus") else {
        return false;
    };
    let Ok(uri) = reqwest::Url::from_file_path(path) else {
        return false;
    };
    let mut options = t3_process::ProcessOptions::new(command.as_os_str()).args([
        "call",
        "--session",
        "--dest",
        "org.freedesktop.FileManager1",
        "--object-path",
        "/org/freedesktop/FileManager1",
        "--method",
        "org.freedesktop.FileManager1.ShowItems",
        &format!("[\"{uri}\"]"),
        "",
    ]);
    options.timeout = Some(std::time::Duration::from_secs(2));
    options.max_output_bytes = 64 * 1024;
    t3_process::run(options)
        .await
        .is_ok_and(|r| r.code == Some(0))
}
