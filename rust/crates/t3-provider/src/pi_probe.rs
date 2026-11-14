use crate::{environment::process_options, pi_launch::RpcLaunch, sensitive::Sensitive};
use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
#[derive(Debug, Clone, Copy, thiserror::Error)]
pub enum PiProbeError {
    #[error("Pi discovery exited before replying.")]
    Exit,
    #[error("Pi discovery refused the request.")]
    Refused,
    #[error("Pi discovery could not read a response.")]
    Response,
}
pub struct PiInventory {
    pub state: Value,
    pub models: Value,
    pub commands: Value,
}
impl std::fmt::Debug for PiInventory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PiInventory([REDACTED])")
    }
}
#[derive(Debug, Clone, Copy)]
pub enum ProbeKind {
    Inventory,
    Models,
    Commands,
}
#[async_trait]
pub trait PiProbe: Send + Sync {
    async fn read(
        &self,
        launch: RpcLaunch,
        kind: ProbeKind,
    ) -> Result<Sensitive<PiInventory>, PiProbeError>;
}
pub struct ProcessPiProbe;
struct AbortTask(tokio::task::JoinHandle<()>);
impl Drop for AbortTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}
#[async_trait]
impl PiProbe for ProcessPiProbe {
    async fn read(
        &self,
        launch: RpcLaunch,
        kind: ProbeKind,
    ) -> Result<Sensitive<PiInventory>, PiProbeError> {
        let mut options = process_options(
            &launch.binary,
            launch.args.expose(),
            launch.environment.expose(),
        );
        options.cwd = Some(launch.cwd);
        options.timeout = None;
        let mut child = t3_process::spawn_non_blocking(&options)
            .await
            .map_err(|_| PiProbeError::Exit)?;
        let mut stdin = child.stdin.take().ok_or(PiProbeError::Response)?;
        let stdout = child.stdout.take().ok_or(PiProbeError::Response)?;
        let _stderr = child.stderr.take().map(|mut stderr| {
            AbortTask(tokio::spawn(async move {
                let _ = tokio::io::copy(&mut stderr, &mut tokio::io::sink()).await;
            }))
        });
        let mut reader = BufReader::new(stdout);
        let mut result = PiInventory {
            state: Value::Null,
            models: Value::Null,
            commands: Value::Null,
        };
        let commands: &[&str] = match kind {
            ProbeKind::Inventory => &["get_state", "get_available_models", "get_commands"],
            ProbeKind::Models => &["get_available_models"],
            ProbeKind::Commands => &["get_commands"],
        };
        for (index, command) in commands.iter().enumerate() {
            let id = format!("t3-discovery-{index}");
            let mut request = serde_json::to_vec(&json!({"id":id,"type":command}))
                .map_err(|_| PiProbeError::Response)?;
            request.push(b'\n');
            stdin
                .write_all(&request)
                .await
                .map_err(|_| PiProbeError::Exit)?;
            stdin.flush().await.map_err(|_| PiProbeError::Exit)?;
            let value = loop {
                let mut line = Vec::new();
                let count = (&mut reader)
                    .take(8 * 1024 * 1024 + 1)
                    .read_until(b'\n', &mut line)
                    .await
                    .map_err(|_| PiProbeError::Response)?;
                if count == 0 {
                    return Err(PiProbeError::Exit);
                }
                if count > 8 * 1024 * 1024 {
                    return Err(PiProbeError::Response);
                }
                let Ok(value) = serde_json::from_slice::<Value>(&line) else {
                    continue;
                };
                if value["type"] != "response" || value["id"] != id {
                    continue;
                }
                if value["success"] != true {
                    if *command == "get_commands" && matches!(kind, ProbeKind::Inventory) {
                        break Value::Null;
                    }
                    return Err(PiProbeError::Refused);
                }
                break value.get("data").cloned().unwrap_or(Value::Null);
            };
            match *command {
                "get_state" => result.state = value,
                "get_available_models" => result.models = value,
                _ => result.commands = value,
            }
        }
        let _ = child.kill().await;
        Ok(Sensitive::new(result))
    }
}
