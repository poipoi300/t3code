use crate::{
    Result,
    project::{self, CompletionShell, ScriptOutput},
};
use std::sync::{Arc, Mutex};
use t3_contracts::{base_schemas::TrimmedNonEmptyString, project::ProjectScript};
use t3_terminal::{
    TerminalCloseInput, TerminalEvent, TerminalManager, TerminalObserveInput, TerminalOpenInput,
    TerminalWriteInput,
};
use tokio::{
    sync::oneshot,
    task::AbortHandle,
    time::{Duration, Instant},
};

#[derive(Clone, Copy)]
pub enum ScriptTrigger {
    Setup,
    Settle,
}
pub struct ScriptInput<'a> {
    pub thread_id: &'a str,
    pub project_root: &'a str,
    pub worktree_path: &'a str,
    pub scripts: &'a [ProjectScript],
    pub trigger: ScriptTrigger,
    /// Supplied from the authoritative thread projection, before any terminal opens.
    pub fenced_link: bool,
    pub observe_completion: bool,
    pub on_output_line: Option<Arc<dyn Fn(String) + Send + Sync>>,
}
#[derive(Debug, Clone)]
pub struct ScriptCompletion {
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
}
pub struct StartedScript {
    pub script: ProjectScript,
    pub terminal_id: String,
    pub cwd: String,
    pub asynchronous: bool,
    pub completion: Option<oneshot::Receiver<ScriptCompletion>>,
}
pub enum RunScriptResult {
    SkippedForLink,
    NoScript,
    Started(Box<StartedScript>),
}

#[derive(Clone)]
pub struct ScriptRunner {
    terminal: TerminalManager,
    tasks: Arc<RunnerTasks>,
}
#[derive(Default)]
struct RunnerTasks(Mutex<Vec<AbortHandle>>);
impl Drop for RunnerTasks {
    fn drop(&mut self) {
        for task in self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .drain(..)
        {
            task.abort();
        }
    }
}
impl ScriptRunner {
    pub fn new(terminal: TerminalManager) -> Self {
        Self {
            terminal,
            tasks: Default::default(),
        }
    }
    pub async fn run_for_thread(&self, input: ScriptInput<'_>) -> Result<RunScriptResult> {
        if input.fenced_link {
            return Ok(RunScriptResult::SkippedForLink);
        }
        let script = match input.trigger {
            ScriptTrigger::Setup => project::setup_script(input.scripts),
            ScriptTrigger::Settle => project::settle_script(input.scripts),
        };
        let Some(script) = script.cloned() else {
            return Ok(RunScriptResult::NoScript);
        };
        let terminal_id = match input.trigger {
            ScriptTrigger::Setup => format!("setup-{}", script.id.0),
            ScriptTrigger::Settle => format!(
                "settle-{}-{}",
                script.id.0,
                &uuid::Uuid::new_v4().to_string()[..8]
            ),
        };
        let mut env: t3_terminal::Environment =
            project::script_runtime_env(input.project_root, Some(input.worktree_path))
                .into_iter()
                .collect();
        for (key, value) in [("COLORTERM", ""), ("NO_COLOR", "1"), ("FORCE_COLOR", "0")] {
            env.insert(key.into(), value.into());
        }
        let mut events = self.terminal.subscribe();
        self.terminal
            .open(TerminalOpenInput {
                thread_id: TrimmedNonEmptyString(input.thread_id.into()),
                terminal_id: TrimmedNonEmptyString(terminal_id.clone()),
                cwd: TrimmedNonEmptyString(input.worktree_path.into()),
                worktree_path: Some(Some(TrimmedNonEmptyString(input.worktree_path.into()))),
                env: Some(Some(env)),
                stop_on_settle: script.stop_on_settle,
                cols: None,
                rows: None,
                provider_instance_id: None,
            })
            .await?;
        let id = TerminalObserveInput {
            thread_id: TrimmedNonEmptyString(input.thread_id.into()),
            terminal_id: TrimmedNonEmptyString(terminal_id.clone()),
        };
        let shell = match self.terminal.get_shell(id.clone()).await {
            Ok(shell) => shell,
            Err(error) => {
                let _ = self
                    .terminal
                    .close(TerminalCloseInput {
                        thread_id: id.thread_id.clone(),
                        terminal_id: Some(Some(id.terminal_id.clone())),
                        delete_history: None,
                    })
                    .await;
                return Err(error.into());
            }
        };
        let shell_kind = project::completion_shell(&shell);
        if input.observe_completion && shell_kind.is_err() {
            self.terminal
                .close_idle(TerminalCloseInput {
                    thread_id: id.thread_id.clone(),
                    terminal_id: Some(Some(id.terminal_id.clone())),
                    delete_history: None,
                })
                .await?;
            return shell_kind.map(|_| RunScriptResult::NoScript);
        }
        let token = uuid::Uuid::new_v4().simple().to_string();
        let command = if input.observe_completion {
            project::wrap_script(
                &script.command.0,
                shell_kind
                    .as_ref()
                    .copied()
                    .map_err(|_| crate::Error::Invalid("unsupported shell".into()))?,
                &token,
            )?
        } else {
            script.command.0.clone()
        };
        let mut output = ScriptOutput::new(&token, &command)?;
        let manager = self.terminal.clone();
        let callback = input.on_output_line;
        let observe = input.observe_completion;
        let immediate =
            shell_kind.is_err() || matches!(shell_kind, Ok(CompletionShell::PowerShell));
        let (done, receiver) = oneshot::channel();
        let task = tokio::spawn(async move {
            let started = Instant::now();
            let mut sent = false;
            let mut busy = false;
            let mut reading = false;
            let mut carry = String::new();
            let mut quiet = Instant::now() + Duration::from_secs(3);
            let send_command = || {
                manager.write(TerminalWriteInput {
                    thread_id: id.thread_id.clone(),
                    terminal_id: id.terminal_id.clone(),
                    data: project::script_terminal_input(&command),
                })
            };
            if immediate {
                sent = send_command().await.is_ok();
                if !sent {
                    output.closed();
                }
            }
            loop {
                if (sent && !observe) || output.settled {
                    break;
                }
                tokio::select! {
                    event = events.recv() => {
                        let Some(event) = event else { output.closed(); break; };
                        let (thread_id, terminal_id) = event_identity(&event);
                        if thread_id != id.thread_id.0 || terminal_id != id.terminal_id.0 { continue; }
                        quiet = Instant::now()+Duration::from_secs(3);
                        match event {
                            TerminalEvent::TerminalStartedEvent(_)|TerminalEvent::TerminalRestartedEvent(_) => { busy=false; reading=false; carry.clear(); },
                            TerminalEvent::TerminalActivityEvent(event) => busy=event.has_running_subprocess,
                            TerminalEvent::TerminalOutputEvent(event) => {
                                let data = event.data;
                                let text=format!("{carry}{data}");
                                let on=text.rfind("\x1b[?2004h"); let off=text.rfind("\x1b[?2004l");
                                if on!=off { reading=on>off; }
                                carry=text.chars().rev().take(7).collect::<String>().chars().rev().collect();
                                for line in output.feed(&data) { if let Some(callback)=&callback { callback(line); } }
                            },
                            TerminalEvent::TerminalExitedEvent(_)|TerminalEvent::TerminalClosedEvent(_) => { output.closed(); break; },
                            _ => {},
                        }
                        if !sent && reading { sent=send_command().await.is_ok(); if !sent {output.closed();} }
                    },
                    _ = tokio::time::sleep_until(quiet), if !sent => {
                        if !busy { sent=send_command().await.is_ok(); if !sent {output.closed();} }
                        quiet=Instant::now()+Duration::from_secs(3);
                    }
                }
            }
            if observe {
                if output.exit_code == Some(0) {
                    // The shell can redraw after the sentinel; let the terminal's own activity check decide idle close.
                    let _ = manager
                        .close_idle(TerminalCloseInput {
                            thread_id: id.thread_id,
                            terminal_id: Some(Some(id.terminal_id)),
                            delete_history: None,
                        })
                        .await;
                }
                let _ = done.send(ScriptCompletion {
                    exit_code: output.exit_code,
                    duration_ms: started.elapsed().as_millis() as u64,
                });
            }
        });
        let mut tasks = self
            .tasks
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        tasks.retain(|t| !t.is_finished());
        tasks.push(task.abort_handle());
        Ok(RunScriptResult::Started(Box::new(StartedScript {
            asynchronous: script.r#async != Some(Some(false)),
            script,
            terminal_id,
            cwd: input.worktree_path.into(),
            completion: observe.then_some(receiver),
        })))
    }
}

fn event_identity(event: &TerminalEvent) -> (&str, &str) {
    match event {
        TerminalEvent::TerminalStartedEvent(e) => (&e.thread_id, &e.terminal_id),
        TerminalEvent::TerminalRestartedEvent(e) => (&e.thread_id, &e.terminal_id),
        TerminalEvent::TerminalActivityEvent(e) => (&e.thread_id, &e.terminal_id),
        TerminalEvent::TerminalOutputEvent(e) => (&e.thread_id, &e.terminal_id),
        TerminalEvent::TerminalExitedEvent(e) => (&e.thread_id, &e.terminal_id),
        TerminalEvent::TerminalClosedEvent(e) => (&e.thread_id, &e.terminal_id),
        TerminalEvent::TerminalErrorEvent(e) => (&e.thread_id, &e.terminal_id),
        TerminalEvent::TerminalClearedEvent(e) => (&e.thread_id, &e.terminal_id),
    }
}
