//! PTY ownership stays here until t3-process supports adopting a portable-pty child.
use crate::{Environment, ShellCandidate, TerminalError, lock};
use portable_pty::{CommandBuilder, MasterPty, PtySize};
use std::{
    io::{Read, Write},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc as channel,
    },
    thread,
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, oneshot, watch};

type Master = Arc<Mutex<Option<Box<dyn MasterPty + Send>>>>;
const DRAIN: Duration = Duration::from_millis(200);
const TICK: Duration = Duration::from_millis(10);
const STARTUP: Duration = Duration::from_secs(2);
const INPUT_CAPACITY: usize = 2048;
pub(crate) enum ProcessEvent {
    Output {
        data: String,
        epoch: u64,
    },
    Exit {
        code: Option<i64>,
        signal: Option<i32>,
    },
    Error(String),
}
struct Input {
    data: Vec<u8>,
    reply: oneshot::Sender<Result<(), TerminalError>>,
}
#[derive(Clone)]
pub(crate) struct Writer {
    send: channel::SyncSender<Input>,
    pid: u32,
}
impl Writer {
    pub fn enqueue(
        &self,
        data: &str,
    ) -> Result<oneshot::Receiver<Result<(), TerminalError>>, TerminalError> {
        let (reply, receive) = oneshot::channel();
        self.send
            .try_send(Input {
                data: data.as_bytes().to_vec(),
                reply,
            })
            .map_err(|e| pty_error("write", e).context(self.pid, None))?;
        Ok(receive)
    }
}
pub(crate) struct Process {
    pub pid: u32,
    pub shell: String,
    master: Master,
    writer: Writer,
    stop: channel::Sender<()>,
    pub done: watch::Receiver<bool>,
    output_epoch: Arc<AtomicU64>,
}
impl Process {
    pub fn spawn(
        candidate: &ShellCandidate,
        cwd: &Path,
        env: &Environment,
        cols: u16,
        rows: u16,
        kill_descendants: bool,
    ) -> Result<(Self, mpsc::Receiver<ProcessEvent>), TerminalError> {
        let pty = portable_pty::native_pty_system()
            .openpty(size(cols, rows))
            .map_err(|e| pty_error("open", e))?;
        let patch = env
            .iter()
            .map(|(k, v)| (k.into(), Some(v.into())))
            .collect();
        let args = candidate.args.iter().map(Into::into).collect::<Vec<_>>();
        let resolved = t3_process::resolve_command(candidate.shell.as_ref(), &args, &patch, false)
            .map_err(|error| {
                let missing = matches!(error, t3_process::ProcessError::NotFound { .. });
                let mut result = pty_error("spawn", error);
                if let TerminalError::Pty { missing: flag, .. } = &mut result {
                    *flag = missing;
                }
                result
            })?;
        let mut command = CommandBuilder::new(&resolved.program);
        command.args(&resolved.args);
        command.cwd(cwd);
        command.env_clear();
        for (key, value) in env {
            command.env(key, value);
        }
        let mut child = pty.slave.spawn_command(command).map_err(|e| {
            let not_found = e.chain().any(|cause| {
                cause
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound)
            }) || [
                "not found",
                "does not exist",
                "doesn't exist",
                "(os error 2)",
                "(os error 3)",
            ]
            .iter()
            .any(|text| e.to_string().contains(text));
            let mut error = pty_error("spawn", e);
            if let TerminalError::Pty { missing, .. } = &mut error {
                *missing = not_found;
            }
            error
        })?;
        drop(pty.slave);
        let Some(pid) = child.process_id().filter(|pid| *pid > 0) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(pty_error("spawn", "missing PID"));
        };
        let setup = (|| {
            #[cfg(windows)]
            let tree = if kill_descendants {
                let mut limits = win32job::ExtendedLimitInfo::new();
                limits.limit_kill_on_job_close();
                let job = win32job::Job::create_with_limit_info(&limits)
                    .map_err(|e| pty_error("job", e))?;
                let handle = child
                    .as_raw_handle()
                    .ok_or_else(|| pty_error("job", "missing process handle"))?;
                job.assign_process(handle as isize)
                    .map_err(|e| pty_error("job", e))?;
                Some(job)
            } else {
                None
            };
            #[cfg(unix)]
            {
                use nix::fcntl::{FcntlArg, OFlag, fcntl};
                let fd = pty
                    .master
                    .as_raw_fd()
                    .ok_or_else(|| pty_error("open", "missing PTY fd"))?;
                let flags = fcntl(fd, FcntlArg::F_GETFL).map_err(|e| pty_error("open", e))?;
                fcntl(
                    fd,
                    FcntlArg::F_SETFL(OFlag::from_bits_truncate(flags) | OFlag::O_NONBLOCK),
                )
                .map_err(|e| pty_error("open", e))?;
            }
            let reader = pty
                .master
                .try_clone_reader()
                .map_err(|e| pty_error("reader", e))?;
            let writer = pty
                .master
                .take_writer()
                .map_err(|e| pty_error("writer", e))?;
            #[cfg(windows)]
            return Ok::<_, TerminalError>((reader, writer, tree));
            #[cfg(unix)]
            Ok::<_, TerminalError>((reader, writer, ()))
        })();
        let (mut reader, mut raw_writer, _tree) = match setup {
            Ok(value) => value,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error.context(pid, Some((cols, rows))));
            }
        };
        let master = Arc::new(Mutex::new(Some(pty.master)));
        let (send, receive) = mpsc::channel(64);
        let (input_send, input_receive) = channel::sync_channel::<Input>(INPUT_CAPACITY);
        let writer = Writer {
            send: input_send,
            pid,
        };
        let startup_writer = writer.clone();
        let writer_wake = writer.send.clone();
        let (stop, stop_receive) = channel::channel();
        let (reader_done_send, reader_done) = channel::sync_channel(1);
        let (startup_send, startup_receive) = channel::sync_channel(1);
        let (done_send, done) = watch::channel(false);
        let cancelled = Arc::new(AtomicBool::new(false));
        let writer_cancelled = cancelled.clone();
        thread::spawn(move || {
            while !writer_cancelled.load(Ordering::Acquire) {
                let Ok(input) = input_receive.recv() else {
                    break;
                };
                let mut data = input.data.as_slice();
                let result = loop {
                    if writer_cancelled.load(Ordering::Acquire) {
                        break Err(pty_error("write", "closed").context(pid, None));
                    }
                    match raw_writer.write(data) {
                        Ok(0) => {
                            break Err(pty_error("write", "zero-length write").context(pid, None));
                        }
                        Ok(n) => {
                            data = &data[n..];
                            if data.is_empty() {
                                break raw_writer
                                    .flush()
                                    .map_err(|e| pty_error("write", e).context(pid, None));
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::park_timeout(TICK)
                        }
                        Err(e) => break Err(pty_error("write", e).context(pid, None)),
                    }
                };
                let _ = input.reply.send(result);
            }
        });
        let output_epoch = Arc::new(AtomicU64::new(0));
        let reader_epoch = output_epoch.clone();
        let reader_cancelled = cancelled.clone();
        let output_send = send.clone();
        let output_lock = Arc::new(Mutex::new(false));
        let reader_lock = output_lock.clone();
        thread::spawn(move || {
            let mut buffer = [0; 16 * 1024];
            let mut decoder = Utf8Decoder::default();
            let mut startup = StartupFilter::new(cfg!(windows));
            let mut startup_send = Some(startup_send);
            loop {
                if reader_cancelled.load(Ordering::Acquire) {
                    break;
                }
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => {
                        let text = decoder.push(&buffer[..n], false);
                        let (text, answered) = startup.push(&text);
                        if answered {
                            let _ = startup_writer.enqueue("\x1b[1;1R");
                            if let Some(ready) = startup_send.take() {
                                let _ = ready.send(());
                            }
                        }
                        if !text.is_empty()
                            && !send_output(
                                &output_send,
                                &reader_lock,
                                &reader_cancelled,
                                ProcessEvent::Output {
                                    data: text,
                                    epoch: reader_epoch.load(Ordering::SeqCst),
                                },
                            )
                        {
                            break;
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::park_timeout(TICK)
                    }
                    Err(e) if cfg!(unix) && e.raw_os_error() == Some(5) => break,
                    Err(e) => {
                        let _ = send_output(
                            &output_send,
                            &reader_lock,
                            &reader_cancelled,
                            ProcessEvent::Error(e.to_string()),
                        );
                        break;
                    }
                }
            }
            let (mut tail, _) = startup.push(&decoder.push(&[], true));
            tail.push_str(&startup.finish());
            if !tail.is_empty() {
                let _ = send_output(
                    &output_send,
                    &reader_lock,
                    &reader_cancelled,
                    ProcessEvent::Output {
                        data: tail,
                        epoch: reader_epoch.load(Ordering::SeqCst),
                    },
                );
            }
            let _ = reader_done_send.send(());
        });
        let wait_master = master.clone();
        thread::spawn(move || {
            // This thread alone signals and reaps: no numeric PID survives a successful wait.
            #[cfg(unix)]
            let mut stopping = None;
            #[cfg(unix)]
            let mut requested = false;
            let status = loop {
                match child.try_wait() {
                    Ok(Some(status)) => break Ok(status),
                    Err(error) => break Err(error),
                    Ok(None) => {}
                }
                #[cfg(unix)]
                if stopping.is_some_and(|deadline| Instant::now() >= deadline) {
                    let _ = nix::sys::signal::killpg(
                        nix::unistd::Pid::from_raw(pid as i32),
                        nix::sys::signal::Signal::SIGKILL,
                    );
                    stopping = None;
                }
                match stop_receive.recv_timeout(TICK) {
                    Ok(()) | Err(channel::RecvTimeoutError::Disconnected) => {
                        #[cfg(unix)]
                        if !requested {
                            requested = true;
                            let _ = kill_descendants;
                            let _ = nix::sys::signal::killpg(
                                nix::unistd::Pid::from_raw(pid as i32),
                                nix::sys::signal::Signal::SIGTERM,
                            );
                            stopping = Some(Instant::now() + Duration::from_secs(1));
                        }
                        #[cfg(windows)]
                        let _ = child.kill();
                    }
                    Err(channel::RecvTimeoutError::Timeout) => {}
                }
            };
            #[cfg(windows)]
            drop(_tree);
            let (master_closed, master_done) = channel::sync_channel(1);
            thread::spawn(move || {
                let value = lock(&wait_master).take();
                drop(value);
                let _ = master_closed.send(());
            });
            // A background job may retain the slave forever; exit does not depend on EOF.
            let _ = reader_done.recv_timeout(DRAIN);
            cancelled.store(true, Ordering::Release);
            let (reply, _) = oneshot::channel();
            let _ = writer_wake.try_send(Input {
                data: Vec::new(),
                reply,
            });
            *lock(&output_lock) = true;
            match status {
                Ok(status) => {
                    let signal = status.signal().and_then(signal_number);
                    let _ = send.blocking_send(ProcessEvent::Exit {
                        code: Some(if cfg!(unix) && status.signal().is_some() {
                            0
                        } else {
                            i64::from(status.exit_code())
                        }),
                        signal: if cfg!(unix) && status.signal().is_none() {
                            Some(0)
                        } else {
                            signal
                        },
                    });
                }
                Err(error) => {
                    let _ = send.blocking_send(ProcessEvent::Error(error.to_string()));
                }
            }
            let _ = master_done.recv_timeout(DRAIN);
            done_send.send_replace(true);
        });
        let process = Self {
            pid,
            shell: candidate.shell.clone(),
            master,
            writer,
            stop,
            done,
            output_epoch,
        };
        if cfg!(windows) && startup_receive.recv_timeout(STARTUP).is_err() {
            return Err(
                pty_error("startup", "ConPTY cursor negotiation did not complete")
                    .context(pid, Some((cols, rows))),
            );
        }
        Ok((process, receive))
    }
    pub fn writer(&self) -> Writer {
        self.writer.clone()
    }
    pub fn clear_output(&self) {
        self.output_epoch.fetch_add(1, Ordering::SeqCst);
    }
    pub fn output_epoch(&self) -> u64 {
        self.output_epoch.load(Ordering::SeqCst)
    }
    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), TerminalError> {
        if let Some(master) = lock(&self.master).as_ref() {
            master
                .resize(size(cols, rows))
                .map_err(|e| pty_error("resize", e).context(self.pid, Some((cols, rows))))?;
        }
        Ok(())
    }
    pub fn stop(&mut self) {
        let _ = self.stop.send(());
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.stop.send(());
    }
}
fn size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        cols,
        rows,
        pixel_width: 0,
        pixel_height: 0,
    }
}
fn pty_error(operation: &'static str, error: impl std::fmt::Display) -> TerminalError {
    TerminalError::Pty {
        operation,
        message: error.to_string(),
        pid: None,
        cols: None,
        rows: None,
        missing: false,
    }
}
fn signal_number(name: &str) -> Option<i32> {
    let name = name.to_ascii_lowercase();
    [
        ("hangup", 1),
        ("interrupt", 2),
        ("quit", 3),
        ("illegal", 4),
        ("trap", 5),
        ("abort", 6),
        ("bus", 7),
        ("floating", 8),
        ("killed", 9),
        ("user defined signal 1", 10),
        ("segmentation", 11),
        ("user defined signal 2", 12),
        ("broken pipe", 13),
        ("alarm", 14),
        ("terminated", 15),
    ]
    .iter()
    .find_map(|(s, n)| name.contains(s).then_some(*n))
}
fn send_output(
    send: &mpsc::Sender<ProcessEvent>,
    ended: &Mutex<bool>,
    cancelled: &AtomicBool,
    mut event: ProcessEvent,
) -> bool {
    loop {
        {
            let ended = lock(ended);
            if *ended || cancelled.load(Ordering::Acquire) {
                return false;
            }
            match send.try_send(event) {
                Ok(()) => return true,
                Err(mpsc::error::TrySendError::Closed(_)) => return false,
                Err(mpsc::error::TrySendError::Full(value)) => event = value,
            }
        }
        thread::park_timeout(TICK);
    }
}
struct StartupFilter {
    active: bool,
    pending: String,
    deadline: Instant,
    windows: bool,
    modes: ModeFilter,
}
impl StartupFilter {
    fn new(active: bool) -> Self {
        Self {
            active,
            pending: String::new(),
            deadline: Instant::now() + STARTUP,
            windows: active,
            modes: ModeFilter::default(),
        }
    }
    fn push(&mut self, data: &str) -> (String, bool) {
        if !self.active {
            return (
                if self.windows {
                    self.modes.push(data)
                } else {
                    data.to_owned()
                },
                false,
            );
        }
        self.pending.push_str(data);
        if let Some(index) = self.pending.find("\x1b[6n") {
            self.pending.replace_range(index..index + 4, "");
            self.active = false;
            return (self.modes.push(&std::mem::take(&mut self.pending)), true);
        }
        if Instant::now() >= self.deadline || self.pending.len() > 64 * 1024 {
            self.active = false;
            return (self.modes.push(&std::mem::take(&mut self.pending)), false);
        }
        (String::new(), false)
    }
}
#[derive(Default)]
struct ModeFilter {
    pending: String,
}
impl ModeFilter {
    fn push(&mut self, data: &str) -> String {
        if self.pending.is_empty() && !data.contains('\x1b') {
            return data.to_owned();
        }
        let mut out = String::with_capacity(data.len());
        for ch in data.chars() {
            self.pending.push(ch);
            loop {
                if ["\x1b[?9001h", "\x1b[?1004h"].contains(&self.pending.as_str()) {
                    self.pending.clear();
                    break;
                }
                if ["\x1b[?9001h", "\x1b[?1004h"]
                    .iter()
                    .any(|mode| mode.starts_with(&self.pending))
                {
                    break;
                }
                out.push(self.pending.remove(0));
                if self.pending.is_empty() {
                    break;
                }
            }
        }
        out
    }
}
impl StartupFilter {
    fn finish(&mut self) -> String {
        let mut output = self.modes.push(&std::mem::take(&mut self.pending));
        output.push_str(&std::mem::take(&mut self.modes.pending));
        output
    }
}

#[derive(Default)]
struct Utf8Decoder {
    pending: Vec<u8>,
}
impl Utf8Decoder {
    fn push(&mut self, data: &[u8], eof: bool) -> String {
        self.pending.extend_from_slice(data);
        let mut out = String::new();
        let mut i = 0;
        while i < self.pending.len() {
            match std::str::from_utf8(&self.pending[i..]) {
                Ok(s) => {
                    out.push_str(s);
                    i = self.pending.len();
                }
                Err(e) => {
                    let valid = i + e.valid_up_to();
                    out.push_str(&String::from_utf8_lossy(&self.pending[i..valid]));
                    i = valid;
                    if let Some(n) = e.error_len() {
                        out.push('\u{fffd}');
                        i += n;
                    } else {
                        break;
                    }
                }
            }
        }
        self.pending.drain(..i);
        if eof && !self.pending.is_empty() {
            out.push_str(&String::from_utf8_lossy(&self.pending));
            self.pending.clear();
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cursor_query_can_follow_other_controls_and_split_at_any_boundary() {
        let burst = "\x1b[?9001h\x1b[?1004h\x1b[6nready";
        for split in 0..=burst.len() {
            let mut filter = StartupFilter::new(true);
            let (mut text, first) = filter.push(&burst[..split]);
            let (tail, second) = filter.push(&burst[split..]);
            text.push_str(&tail);
            assert_eq!(text, "ready");
            assert_ne!(first, second);
            assert_eq!(filter.push("\x1b[6n\x1b[?1004h"), ("\x1b[6n".into(), false));
        }
    }
    #[test]
    fn conhost_controls_are_removed_even_after_cursor_negotiation() {
        let mut filter = StartupFilter::new(true);
        assert_eq!(filter.push("\x1b[6n"), (String::new(), true));
        let burst = "hello\x1b[?9001h\x1b[?1004hworld\x1b[?1049h";
        for split in 0..=burst.len() {
            let mut filter = StartupFilter::new(true);
            filter.push("\x1b[6n");
            let (mut text, _) = filter.push(&burst[..split]);
            text.push_str(&filter.push(&burst[split..]).0);
            assert_eq!(text, "helloworld\x1b[?1049h");
        }
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn stop_after_reaping_has_no_signal_receiver() {
        let temp = tempfile::tempdir().expect("temp");
        let env = std::env::vars().collect();
        let (mut process, mut events) = Process::spawn(
            &ShellCandidate {
                shell: "/bin/sh".into(),
                args: vec!["-c".into(), "exit 0".into()],
            },
            temp.path(),
            &env,
            80,
            24,
            false,
        )
        .expect("PTY");
        loop {
            if matches!(
                events.recv().await,
                Some(ProcessEvent::Exit {
                    code: Some(0),
                    signal: Some(0)
                })
            ) {
                break;
            }
        }
        while !*process.done.borrow_and_update() {
            process.done.changed().await.expect("wait completed");
        }
        assert!(
            process.stop.send(()).is_err(),
            "a reaped child cannot accept signal requests"
        );
        process.stop();
    }
    #[test]
    fn input_queue_is_bounded_and_enqueue_never_waits_for_writer() {
        let (send, receive) = channel::sync_channel(2);
        let writer = Writer { send, pid: 1 };
        let _first = writer.enqueue("one").expect("first");
        let _second = writer.enqueue("two").expect("second");
        assert!(writer.enqueue("three").is_err());
        assert_eq!(receive.recv().expect("input").data, b"one");
        assert_eq!(receive.recv().expect("input").data, b"two");
    }
}
