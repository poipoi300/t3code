use std::{
    future::Future,
    process::Stdio,
    task::{Context, Waker},
    time::{Duration, Instant},
};
use t3_process::{ProcessOptions, resolve_command, run, spawn};

fn percentiles(values: &mut [Duration]) -> (Duration, Duration) {
    values.sort_unstable();
    (
        values[values.len() / 2],
        values[(values.len() * 95).div_ceil(100) - 1],
    )
}

#[cfg(windows)]
fn legacy_spawn(
    options: &ProcessOptions,
) -> Result<Box<dyn process_wrap::tokio::ChildWrapper>, t3_process::ProcessError> {
    use process_wrap::tokio::{CommandWrap, CreationFlags, JobObject};
    let resolved = resolve_command(
        &options.program,
        &options.args,
        &options.env,
        options.inherit_env,
    )?;
    let mut command = CommandWrap::with_new(&resolved.program, |command| {
        command
            .args(&resolved.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
    });
    command.wrap(CreationFlags(
        windows::Win32::System::Threading::CREATE_NO_WINDOW,
    ));
    command.wrap(JobObject);
    command
        .spawn()
        .map_err(|source| t3_process::ProcessError::Spawn {
            program: options.program.clone(),
            source,
        })
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let program = args.next().ok_or("usage: spawn-latency EXE [ARGS...]")?;
    let options = ProcessOptions::new(program).args(args);
    let resolved = resolve_command(
        &options.program,
        &options.args,
        &options.env,
        options.inherit_env,
    )?;
    println!(
        "executable: {:?}, args: {:?}",
        resolved.program, resolved.args
    );
    let mut plain = Vec::new();
    let mut owned = Vec::new();
    let mut first_poll = Vec::new();
    let mut resolution = Vec::new();
    #[cfg(windows)]
    let (mut assignment, mut window_bound) = (Vec::new(), Vec::new());
    #[cfg(windows)]
    let mut legacy = Vec::new();
    for index in 0..205 {
        let start = Instant::now();
        resolve_command(
            &options.program,
            &options.args,
            &options.env,
            options.inherit_env,
        )?;
        if index >= 5 {
            resolution.push(start.elapsed());
        }
        let mut command = tokio::process::Command::new(&resolved.program);
        command
            .args(&resolved.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        command.creation_flags(0x0800_0000);
        let start = Instant::now();
        let mut child = command.spawn()?;
        let latency = start.elapsed();
        child.wait().await?;
        if index >= 5 {
            plain.push(latency);
        }
        let start = Instant::now();
        let mut child = spawn(&options)?;
        let latency = start.elapsed();
        child.wait().await?;
        if index >= 5 {
            owned.push(latency);
        }
        #[cfg(windows)]
        {
            if std::env::var_os("T3_PROCESS_PROBE_LEGACY").is_some() {
                let start = Instant::now();
                let mut child = legacy_spawn(&options)?;
                let latency = start.elapsed();
                let native = child.inner_mut() as &mut dyn std::any::Any;
                native
                    .downcast_mut::<tokio::process::Child>()
                    .ok_or("missing native child")?
                    .wait()
                    .await?;
                if index >= 5 {
                    legacy.push(latency);
                }
            }
            let job = win32job::Job::create()?;
            let start = Instant::now();
            let mut child = command.spawn()?;
            let returned = Instant::now();
            job.assign_process(child.raw_handle().ok_or("missing child handle")? as isize)?;
            let assigned = Instant::now();
            if index >= 5 {
                assignment.push(assigned - returned);
                window_bound.push(assigned - start);
            }
            child.wait().await?;
        }
        let mut execution = Box::pin(run(options.clone()));
        let start = Instant::now();
        let result = execution
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()));
        let latency = start.elapsed();
        match result {
            std::task::Poll::Ready(output) => {
                output?;
            }
            std::task::Poll::Pending => {
                execution.await?;
            }
        }
        if index >= 5 {
            first_poll.push(latency);
        }
    }
    let (plain, plain_p95) = percentiles(&mut plain);
    let (owned, owned_p95) = percentiles(&mut owned);
    let (first_poll, first_poll_p95) = percentiles(&mut first_poll);
    println!(
        "200 spawns after 5 warmups, spawn only: tokio median={plain:?} p95={plain_p95:?}, t3-process median={owned:?} p95={owned_p95:?}, median overhead={:.3}ms, ratio={:.2}",
        (owned.as_secs_f64() - plain.as_secs_f64()) * 1000.0,
        owned.as_secs_f64() / plain.as_secs_f64()
    );
    println!("run first poll (reactor occupied): median={first_poll:?} p95={first_poll_p95:?}");
    let (median, p95) = percentiles(&mut resolution);
    println!("command resolution: median={median:?} p95={p95:?}");
    #[cfg(windows)]
    {
        if !legacy.is_empty() {
            let (median, p95) = percentiles(&mut legacy);
            println!("legacy process-wrap suspended spawn: median={median:?} p95={p95:?}");
        }
        let (median, p95) = percentiles(&mut assignment);
        let maximum = assignment.last().ok_or("no assignment samples")?;
        println!(
            "Tokio spawn return -> job assigned: median={median:?} p95={p95:?} max={maximum:?}"
        );
        let (median, p95) = percentiles(&mut window_bound);
        let maximum = window_bound.last().ok_or("no window samples")?;
        println!(
            "spawn entry -> job assigned (upper bound on running unassigned): median={median:?} p95={p95:?} max={maximum:?}"
        );
    }
    if cfg!(windows) && first_poll > plain {
        return Err("run blocked the reactor longer than a plain Tokio spawn".into());
    }
    Ok(())
}
