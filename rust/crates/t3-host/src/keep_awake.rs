use crate::Result;
use std::{ffi::OsString, time::Duration};
use tokio::sync::watch;

pub fn inhibitor_command(platform: &str, pid: u32) -> Option<t3_process::ProcessOptions> {
    let options=match platform {
        "darwin"=>t3_process::ProcessOptions::new("caffeinate").args(["-i","-w",&pid.to_string()]),
        "linux"=>t3_process::ProcessOptions::new("systemd-inhibit").args(["--what=idle:sleep","--who=T3 Code","--why=Agents are working","--mode=block","tail",&format!("--pid={pid}"),"-f","/dev/null"]),
        "win32"|"windows"=>t3_process::ProcessOptions::new("powershell.exe").args(["-NoProfile","-NonInteractive","-Command",&format!("$t = Add-Type -Name Power -Namespace T3 -PassThru -MemberDefinition '[DllImport(\"kernel32.dll\")] public static extern uint SetThreadExecutionState(uint f);'; [void]$t::SetThreadExecutionState([uint32]2147483649); Wait-Process -Id {pid}")]),
        _=>return None,
    };
    Some(options)
}
pub async fn hold_while_wanted(
    wanted: watch::Receiver<bool>,
    mut options: t3_process::ProcessOptions,
) -> Result<()> {
    options.timeout = None;
    options.max_output_bytes = 1024;
    options.output_mode = t3_process::OutputMode::Truncate;
    hold_with_grace(
        wanted,
        || async {
            let mut retry = Duration::from_secs(5);
            loop {
                let _ = t3_process::run(options.clone()).await;
                tokio::time::sleep(retry).await;
                retry = (retry * 2).min(Duration::from_secs(600));
            }
        },
        Duration::from_secs(60),
    )
    .await;
    Ok(())
}
pub async fn hold_with_grace<F, H>(mut wanted: watch::Receiver<bool>, hold: F, grace: Duration)
where
    F: Fn() -> H,
    H: std::future::Future<Output = ()>,
{
    let mut active = None;
    let mut release = None;
    loop {
        if *wanted.borrow_and_update() {
            release = None;
            if active.is_none() {
                active = Some(Box::pin(hold()));
            }
        } else if active.is_some() && release.is_none() {
            release = Some(tokio::time::Instant::now() + grace);
        }
        tokio::select! {
            changed = wanted.changed() => { if changed.is_err() { return; } },
            _ = async {
                if let Some(hold) = active.as_mut() { hold.await; }
                else { std::future::pending::<()>().await; }
            } => { active = None; },
            _ = async {
                if let Some(at) = release { tokio::time::sleep_until(at).await; }
                else { std::future::pending::<()>().await; }
            } => { active = None; release = None; }
        }
    }
}
pub fn command_arguments(options: &t3_process::ProcessOptions) -> (&OsString, &[OsString]) {
    (&options.program, &options.args)
}
