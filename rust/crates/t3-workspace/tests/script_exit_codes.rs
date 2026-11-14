//! Setup script exit codes through the real runner and a real terminal, as TypeScript's
//! `ProjectSetupScriptRunner` derives them: the completion wrapper's sentinel carries the code,
//! and a terminal that exits or closes before it gives no code (`None`).
//!
//! PowerShell takes a native program's code from `$LASTEXITCODE` (a later successful cmdlet does
//! not clear it; a later native program replaces it). POSIX shells report the last command's
//! status. Windows needs `PATHEXT` in the shell's environment: without it PowerShell does not
//! wait for a native program and every native exit reads 0, so the environment is part of what
//! these tests pin.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use std::path::Path;
use t3_contracts::{base_schemas::TrimmedNonEmptyString, project::ProjectScript};
use t3_terminal::{ManagerOptions, TerminalCloseInput, TerminalManager};
use t3_workspace::scripts::{RunScriptResult, ScriptInput, ScriptRunner, ScriptTrigger};
use tokio::sync::{mpsc, oneshot};

struct Run {
    manager: TerminalManager,
    thread: String,
    lines: mpsc::UnboundedReceiver<String>,
    completion: oneshot::Receiver<t3_workspace::scripts::ScriptCompletion>,
    _runner: ScriptRunner,
}

impl Run {
    async fn start(shell: &str, root: &Path, thread: &str, command: &str, env: Option<&[&str]>) -> Self {
        let mut options = ManagerOptions::new(root.join(format!("logs-{thread}")));
        options.preferred_shell = Some(shell.into());
        if let Some(keys) = env {
            options.env = keys
                .iter()
                .filter_map(|k| std::env::var(k).ok().map(|v| ((*k).to_owned(), v)))
                .collect();
        }
        let manager = TerminalManager::new(options).unwrap();
        let runner = ScriptRunner::new(manager.clone());
        let worktree = root.join(thread);
        std::fs::create_dir_all(&worktree).unwrap();
        let worktree = worktree.to_string_lossy().into_owned();
        let scripts: Vec<ProjectScript> = vec![serde_json::from_value(serde_json::json!({
            "id": "setup", "name": "Setup", "command": command, "icon": "configure",
            "runOnWorktreeCreate": true, "async": false,
        }))
        .unwrap()];
        let (sender, lines) = mpsc::unbounded_channel();
        let started = runner
            .run_for_thread(ScriptInput {
                thread_id: thread,
                project_root: &worktree,
                worktree_path: &worktree,
                scripts: &scripts,
                trigger: ScriptTrigger::Setup,
                fenced_link: false,
                observe_completion: true,
                on_output_line: Some(std::sync::Arc::new(move |line| {
                    let _ = sender.send(line);
                })),
            })
            .await
            .unwrap();
        let RunScriptResult::Started(started) = started else {
            panic!("the setup script did not start")
        };
        Self {
            manager,
            thread: thread.into(),
            lines,
            completion: started.completion.unwrap(),
            _runner: runner,
        }
    }
    async fn exit_code(self) -> Option<i32> {
        self.completion.await.unwrap().exit_code
    }
    /// Waits for the script's own output, so the next step happens after it ran.
    async fn output(&mut self, marker: &str) {
        while let Some(line) = self.lines.recv().await {
            if line.contains(marker) {
                return;
            }
        }
        panic!("output ended before {marker:?}");
    }
    /// What cancelling a launch does to its setup terminal.
    async fn close(&self) {
        self.manager
            .close(TerminalCloseInput {
                thread_id: TrimmedNonEmptyString(self.thread.clone()),
                terminal_id: Some(Some(TrimmedNonEmptyString("setup-setup".into()))),
                delete_history: Some(Some(true)),
            })
            .await
            .unwrap();
    }
}

#[cfg(windows)]
fn powershell() -> String {
    format!(
        r"{}\System32\WindowsPowerShell\v1.0\powershell.exe",
        std::env::var("SystemRoot").unwrap()
    )
}

#[cfg(windows)]
#[tokio::test]
async fn powershell_scripts_report_native_exit_codes_as_typescript_does() {
    let temp = tempfile::tempdir().unwrap();
    let shell = powershell();
    for (index, (command, expected)) in [
        // A native program's non-zero exit as the last statement.
        ("cmd.exe /c exit 3", Some(3)),
        // A later successful cmdlet does not clear it; a later native program replaces it.
        ("cmd.exe /c exit 3; Write-Output ok", Some(3)),
        ("cmd.exe /c exit 3; cmd.exe /c exit 0", Some(0)),
        ("Write-Output ok", Some(0)),
        // `exit` ends the shell before the sentinel: no code.
        ("exit 4", None),
    ]
    .into_iter()
    .enumerate()
    {
        let run = Run::start(&shell, temp.path(), &format!("ps{index}"), command, None).await;
        assert_eq!(run.exit_code().await, expected, "{command}");
    }
}

/// The environment a server hands its terminals must carry `PATHEXT`: with it, a reduced
/// environment still reports the native code.
#[cfg(windows)]
#[tokio::test]
async fn powershell_reports_native_exit_codes_in_a_reduced_environment_with_pathext() {
    let temp = tempfile::tempdir().unwrap();
    let run = Run::start(
        &powershell(),
        temp.path(),
        "reduced",
        "& $env:ComSpec /c exit 3",
        Some(&["SystemRoot", "ComSpec", "WINDIR", "PATHEXT"]),
    )
    .await;
    assert_eq!(run.exit_code().await, Some(3));
}

/// A terminating error stops the wrapper before its sentinel, as in TypeScript: the setup does
/// not settle as a success, and cancelling it (closing its terminal) settles it with no code.
/// A script still running when cancelled settles the same way.
#[cfg(windows)]
#[tokio::test]
async fn powershell_terminating_errors_and_cancelled_scripts_have_no_exit_code() {
    let temp = tempfile::tempdir().unwrap();
    let shell = powershell();
    let mut thrown = Run::start(&shell, temp.path(), "thrown", "throw 't3-setup-boom'", None).await;
    thrown.output("t3-setup-boom").await;
    thrown.close().await;
    assert_eq!(thrown.exit_code().await, None);

    let mut running = Run::start(
        &shell,
        temp.path(),
        "running",
        "Write-Output t3-setup-started; Start-Sleep -Seconds 600",
        None,
    )
    .await;
    running.output("t3-setup-started").await;
    running.close().await;
    assert_eq!(running.exit_code().await, None);
}

/// POSIX shells: the last command's status, `exit N` inside the wrapper's subshell, and a
/// cancelled script. On Windows this uses Git's bash, as `tests/project.rs` does.
#[tokio::test]
async fn posix_scripts_report_the_last_status_and_cancel_has_no_exit_code() {
    #[cfg(windows)]
    let shell = "C:/Program Files/Git/bin/bash.exe";
    #[cfg(unix)]
    let shell = "/bin/bash";
    let temp = tempfile::tempdir().unwrap();
    for (index, (command, expected)) in [
        ("false", Some(1)),
        ("exit 7", Some(7)),
        ("sh -c 'exit 3'", Some(3)),
        // A failure followed by a success reports the success, as the shell does.
        ("false; true", Some(0)),
        ("echo ok", Some(0)),
    ]
    .into_iter()
    .enumerate()
    {
        let run = Run::start(shell, temp.path(), &format!("sh{index}"), command, None).await;
        assert_eq!(run.exit_code().await, expected, "{command}");
    }
    let mut running = Run::start(
        shell,
        temp.path(),
        "sh-running",
        "echo t3-setup-started; sleep 600",
        None,
    )
    .await;
    running.output("t3-setup-started").await;
    running.close().await;
    assert_eq!(running.exit_code().await, None);
}
