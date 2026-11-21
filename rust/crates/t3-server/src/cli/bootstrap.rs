//! One newline-delimited desktop envelope. Diagnostics never include the input or decoder error.
use t3_contracts::desktop_bootstrap::DesktopBackendBootstrap;

#[derive(Debug, thiserror::Error)]
pub enum BootstrapError {
    #[error("Failed to open bootstrap input stream for file descriptor {0}.")]
    Open(i64),
    #[error("Failed to read bootstrap envelope from file descriptor {0}.")]
    Read(i64),
    #[error("Failed to decode bootstrap envelope from file descriptor {0}.")]
    Decode(i64),
    #[error(
        "Failed to open bootstrap input stream for file descriptor {0}: bootstrap fd not supported on this platform yet."
    )]
    Unsupported(i64),
}

/// The envelope is a few hundred bytes; a longer line is not one.
#[cfg(unix)]
const MAX_ENVELOPE_BYTES: usize = 1 << 20;

#[cfg(not(unix))]
pub(crate) fn read(fd: i64) -> Result<Option<DesktopBackendBootstrap>, BootstrapError> {
    Err(BootstrapError::Unsupported(fd))
}

#[cfg(unix)]
pub(crate) fn read(fd: i64) -> Result<Option<DesktopBackendBootstrap>, BootstrapError> {
    use filedescriptor::{FileDescriptor, POLLIN, poll, pollfd};
    use std::{
        io::{ErrorKind, Read},
        os::fd::{AsRawFd, RawFd},
        time::{Duration, Instant},
    };

    // dup validates the externally supplied number and owns only the duplicate.
    // Reopening /proc/self/fd cannot duplicate an Electron socketpair on Linux.
    struct Inherited(RawFd);
    impl AsRawFd for Inherited {
        fn as_raw_fd(&self) -> RawFd {
            self.0
        }
    }
    let Ok(raw) = RawFd::try_from(fd) else {
        return Ok(None);
    };
    // The parent clears CLOEXEC to deliver this descriptor. Do not pass it on to
    // provider processes, even when the envelope is incomplete or malformed.
    use nix::fcntl::{FcntlArg, FdFlag, fcntl};
    match fcntl(raw, FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC)) {
        Ok(_) => {}
        Err(nix::errno::Errno::EBADF) => return Ok(None),
        Err(_) => return Err(BootstrapError::Open(fd)),
    }
    let mut input = match FileDescriptor::dup(&Inherited(raw)) {
        Ok(file) => file,
        Err(filedescriptor::Error::Dup { source, .. })
            if source.raw_os_error() == Some(rustix::io::Errno::BADF.raw_os_error()) =>
        {
            return Ok(None);
        }
        Err(_) => return Err(BootstrapError::Open(fd)),
    };
    let deadline = Instant::now() + Duration::from_secs(1);
    let mut line = Vec::new();
    let mut chunk = [0; 4096];
    loop {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            return Ok(None);
        };
        let mut interest = [pollfd {
            fd: input.as_raw_fd(),
            events: POLLIN,
            revents: 0,
        }];
        match poll(&mut interest, Some(remaining)) {
            Ok(0) => return Ok(None),
            Err(filedescriptor::Error::Poll(e)) if e.kind() == ErrorKind::Interrupted => continue,
            Err(_) => return Err(BootstrapError::Read(fd)),
            Ok(_) => {}
        }
        let count = match input.read(&mut chunk) {
            Ok(count) => count,
            Err(e) if matches!(e.kind(), ErrorKind::Interrupted | ErrorKind::WouldBlock) => {
                continue;
            }
            Err(_) => return Err(BootstrapError::Read(fd)),
        };
        if count == 0 && line.is_empty() {
            return Ok(None);
        }
        let end = chunk[..count]
            .iter()
            .position(|byte| matches!(byte, b'\r' | b'\n'));
        line.extend_from_slice(&chunk[..end.unwrap_or(count)]);
        if line.len() > MAX_ENVELOPE_BYTES {
            return Err(BootstrapError::Read(fd));
        }
        if count == 0 || end.is_some() {
            break;
        }
    }
    let envelope: DesktopBackendBootstrap =
        serde_json::from_slice(&line).map_err(|_| BootstrapError::Decode(fd))?;
    if envelope.port == 0
        || envelope.tailscale_serve_port == 0
        || [
            envelope.desktop_telemetry_fd,
            envelope.desktop_telemetry_control_fd,
            envelope.desktop_browser_fd,
            envelope.desktop_browser_control_fd,
        ]
        .contains(&Some(0))
    {
        return Err(BootstrapError::Decode(fd));
    }
    Ok(Some(envelope))
}

#[cfg(all(test, unix))]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::{
        io::Write,
        os::{fd::AsRawFd, unix::net::UnixStream},
        time::{Duration, Instant},
    };

    const FIXTURE: &str = r#"{"mode":"desktop","noBrowser":true,"port":3773,"host":"127.0.0.1","desktopBootstrapToken":"private-fixture","tailscaleServeEnabled":false,"tailscaleServePort":443}"#;

    /// `read` takes a descriptor number; keep the reading end alive for the whole test.
    fn pair() -> (UnixStream, UnixStream) {
        UnixStream::pair().unwrap()
    }

    #[test]
    fn a_complete_line_decodes_without_waiting_for_eof() {
        let (mut writer, reader) = pair();
        writeln!(writer, "{FIXTURE}").unwrap();
        let started = Instant::now();
        let envelope = read(i64::from(reader.as_raw_fd())).unwrap().unwrap();
        assert_eq!(envelope.port, 3773);
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn crlf_and_an_unterminated_final_line_at_eof_both_decode() {
        let (mut writer, reader) = pair();
        write!(writer, "{FIXTURE}\r\n").unwrap();
        assert!(read(i64::from(reader.as_raw_fd())).unwrap().is_some());
        let (mut writer, reader) = pair();
        write!(writer, "{FIXTURE}").unwrap();
        drop(writer);
        assert!(read(i64::from(reader.as_raw_fd())).unwrap().is_some());
    }

    #[test]
    fn eof_without_data_and_an_open_silent_writer_yield_no_envelope() {
        let (writer, reader) = pair();
        drop(writer);
        assert!(read(i64::from(reader.as_raw_fd())).unwrap().is_none());
        let (_writer, reader) = pair();
        let started = Instant::now();
        assert!(read(i64::from(reader.as_raw_fd())).unwrap().is_none());
        assert!(started.elapsed() >= Duration::from_millis(900));
    }

    #[test]
    fn a_partial_line_with_the_writer_still_open_times_out_without_an_error() {
        let (mut writer, reader) = pair();
        write!(writer, "{{\"mode\":\"desk").unwrap();
        assert!(read(i64::from(reader.as_raw_fd())).unwrap().is_none());
    }

    #[test]
    fn a_lone_carriage_return_ends_the_line() {
        let (mut writer, reader) = pair();
        write!(writer, "{FIXTURE}\r").unwrap();
        assert!(read(i64::from(reader.as_raw_fd())).unwrap().is_some());
    }

    #[test]
    fn an_oversized_line_is_a_read_error_without_the_payload() {
        let (mut writer, reader) = pair();
        let big = "x".repeat(MAX_ENVELOPE_BYTES + 8192);
        let feeder = std::thread::spawn(move || {
            let _ = writer.write_all(big.as_bytes());
        });
        let error = read(i64::from(reader.as_raw_fd())).unwrap_err();
        assert!(matches!(error, BootstrapError::Read(_)));
        drop(reader);
        feeder.join().unwrap();
    }

    #[test]
    fn zero_telemetry_and_browser_descriptors_are_rejected() {
        for key in [
            "desktopTelemetryFd",
            "desktopTelemetryControlFd",
            "desktopBrowserFd",
            "desktopBrowserControlFd",
        ] {
            let line = FIXTURE.replace('}', &format!(",\"{key}\":0}}"));
            let (mut writer, reader) = pair();
            writeln!(writer, "{line}").unwrap();
            assert!(matches!(
                read(i64::from(reader.as_raw_fd())),
                Err(BootstrapError::Decode(_))
            ));
        }
    }

    #[test]
    fn the_inherited_descriptor_is_not_passed_on_to_children() {
        let (mut writer, reader) = pair();
        writeln!(writer, "{FIXTURE}").unwrap();
        let fd = reader.as_raw_fd();
        read(i64::from(fd)).unwrap().unwrap();
        let flags = nix::fcntl::fcntl(fd, nix::fcntl::FcntlArg::F_GETFD).unwrap();
        assert!(flags & nix::fcntl::FdFlag::FD_CLOEXEC.bits() != 0);
    }

    #[test]
    fn closed_and_out_of_range_descriptors_are_no_envelope() {
        assert!(read(999_999).unwrap().is_none());
        assert!(read(-1).unwrap().is_none());
        assert!(read(i64::MAX).unwrap().is_none());
    }

    #[test]
    fn malformed_and_zero_port_envelopes_fail_without_echoing_the_payload() {
        for line in [
            "not json".to_owned(),
            String::new(),
            FIXTURE.replace("3773", "0"),
            FIXTURE.replace("443", "0"),
        ] {
            let (mut writer, reader) = pair();
            writeln!(writer, "{line}").unwrap();
            let error = read(i64::from(reader.as_raw_fd())).unwrap_err();
            let text = format!("{error} {error:?}");
            assert!(matches!(error, BootstrapError::Decode(_)));
            assert!(!text.contains("private-fixture") && !text.contains("not json"));
        }
    }
}
