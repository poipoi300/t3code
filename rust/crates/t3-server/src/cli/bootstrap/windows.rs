//! Windows reader for the desktop bootstrap descriptor: the one place in `t3-server` that allows
//! `unsafe` (maintainer-approved for exactly this, see the commit that added it).
//!
//! The desktop (Electron, or the Rust desktop shell) passes the envelope on an inherited **CRT
//! descriptor** number, normally 3. `std::os::windows` knows handles, not CRT descriptors, and
//! `GetStdHandle` only reaches stdin, stdout and stderr, so the CRT's own `_get_osfhandle` is the
//! narrowest way to get from the number to the handle. The handle belongs to the CRT: this module
//! duplicates it into a private non-inheritable handle and never closes the original (the Unix
//! reader leaves its original open too).
//!
//! Invariants every `unsafe` block below relies on, repeated in its `SAFETY` comment:
//! 1. The CRT-owned handle is only borrowed; no block closes it.
//! 2. The thread-local invalid-parameter handler is installed only around `_get_osfhandle` and is
//!    restored on every path.
//! 3. The duplicate is wrapped in an owner exactly once.
//! 4. A read never blocks past the deadline: pipes are polled with `PeekNamedPipe` and read for at
//!    most the byte count it reported, and each read's `OVERLAPPED`, event and buffer live on one
//!    stack frame until the kernel is done with them (cancelled and drained on timeout).
//! 5. Console, socket and unknown handle types are rejected, never treated as pipes.
//! 6. No error, `Debug` output or log carries the payload, a handle value or OS error text.

use std::{
    ffi::c_int,
    io::Read,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{
        DUPLICATE_SAME_ACCESS, DuplicateHandle, ERROR_BROKEN_PIPE, ERROR_HANDLE_EOF,
        ERROR_IO_PENDING, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
    },
    Storage::FileSystem::{FILE_TYPE_DISK, FILE_TYPE_PIPE, GetFileType, ReadFile},
    System::{
        IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED},
        Pipes::PeekNamedPipe,
        Threading::{CreateEventW, GetCurrentProcess, WaitForSingleObject},
    },
};

/// Why no envelope could be read. Carries no payload, handle value or OS text.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Failure {
    /// Console, socket or other handle type with no verified read semantics.
    UnsupportedHandle,
    Open,
    Read,
}

type InvalidParameterHandler = unsafe extern "C" fn(*const u16, *const u16, *const u16, u32, usize);

// The UCRT entry points. `_get_osfhandle` maps a CRT descriptor to its HANDLE and, for a number
// outside the CRT's table, calls the invalid-parameter handler (which aborts by default).
unsafe extern "C" {
    fn _get_osfhandle(fd: c_int) -> isize;
    fn _set_thread_local_invalid_parameter_handler(
        handler: Option<InvalidParameterHandler>,
    ) -> Option<InvalidParameterHandler>;
}

unsafe extern "C" fn ignore_invalid_parameter(
    _: *const u16,
    _: *const u16,
    _: *const u16,
    _: u32,
    _: usize,
) {
}

/// Returns the first line (without CR or LF) of the inherited CRT descriptor `fd`; `Ok(None)` when
/// the descriptor is not open, the stream ends first or `timeout` passes.
pub(super) fn read_first_line(
    fd: i64,
    timeout: Duration,
    max_bytes: usize,
) -> Result<Option<Vec<u8>>, Failure> {
    let Ok(fd) = c_int::try_from(fd) else {
        return Ok(None);
    };
    let deadline = Instant::now() + timeout;
    let Some(owned) = duplicate_inherited(fd)? else {
        return Ok(None);
    };
    // SAFETY: `owned` holds a live handle for the whole call (invariant 3); `GetFileType` only
    // reads it.
    let kind = unsafe { GetFileType(owned.as_raw_handle()) };
    match kind {
        // A file cannot block on a pipe writer, so the safe std reader suffices.
        FILE_TYPE_DISK => read_file(owned, max_bytes),
        FILE_TYPE_PIPE => read_pipe(&owned, deadline, max_bytes),
        _ => Err(Failure::UnsupportedHandle),
    }
}

/// CRT descriptor to a private, non-inheritable duplicate of its handle. The CRT keeps the
/// original; it is never closed here.
fn duplicate_inherited(fd: c_int) -> Result<Option<OwnedHandle>, Failure> {
    // SAFETY: installing a thread-local handler that does nothing is sound. It makes
    // `_get_osfhandle` return -1 (errno EBADF) for a number outside the CRT table instead of
    // aborting the process (invariant 2). The previous handler is restored below, and nothing
    // between the two calls can return early or unwind.
    let previous =
        unsafe { _set_thread_local_invalid_parameter_handler(Some(ignore_invalid_parameter)) };
    // SAFETY: any `c_int` is a valid argument; the call only reads CRT tables under the CRT lock.
    let raw = unsafe { _get_osfhandle(fd) };
    // SAFETY: restores exactly the handler the first call returned (invariant 2).
    unsafe { _set_thread_local_invalid_parameter_handler(previous) };
    // -1 is INVALID_HANDLE_VALUE (EBADF); -2 means the descriptor exists with no OS handle.
    if raw == -1 || raw == -2 || raw == 0 {
        return Ok(None);
    }
    let source = raw as HANDLE;
    let mut duplicate: HANDLE = INVALID_HANDLE_VALUE;
    // SAFETY: `source` came from the CRT and is only borrowed: `DuplicateHandle` does not close it
    // (invariant 1). Both process arguments are this process's pseudo-handle, `duplicate` is a
    // valid out pointer, and bInheritHandle is 0, so the copy never reaches child processes.
    let duplicated = unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            source,
            GetCurrentProcess(),
            &mut duplicate,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        )
    };
    if duplicated == 0 || duplicate == INVALID_HANDLE_VALUE || duplicate.is_null() {
        return Err(Failure::Open);
    }
    // SAFETY: `DuplicateHandle` just returned `duplicate` and nothing else owns it; this is the
    // only place an owner is built from it, so it is closed exactly once (invariant 3).
    Ok(Some(unsafe { OwnedHandle::from_raw_handle(duplicate) }))
}

fn read_file(file: OwnedHandle, max_bytes: usize) -> Result<Option<Vec<u8>>, Failure> {
    let mut file = std::fs::File::from(file).take(
        u64::try_from(max_bytes)
            .unwrap_or(u64::MAX)
            .saturating_add(1),
    );
    let mut data = Vec::new();
    file.read_to_end(&mut data).map_err(|_| Failure::Read)?;
    first_line(&data, max_bytes)
}

/// The text before the first CR or LF, or all of it at the end of the data. No data means no
/// envelope; more than `max_bytes` before the terminator is a read failure.
fn first_line(data: &[u8], max_bytes: usize) -> Result<Option<Vec<u8>>, Failure> {
    if data.is_empty() {
        return Ok(None);
    }
    let end = data.iter().position(|byte| matches!(byte, b'\r' | b'\n'));
    let line = &data[..end.unwrap_or(data.len())];
    if line.len() > max_bytes {
        return Err(Failure::Read);
    }
    Ok(Some(line.to_vec()))
}

/// What a pipe reports about waiting data.
enum Pending {
    Bytes(usize),
    /// The writer closed and nothing is left: a read observes the end of the stream.
    Closed,
}

fn pending(pipe: &OwnedHandle) -> Result<Pending, Failure> {
    let mut available = 0u32;
    // SAFETY: `pipe` is a live pipe handle (invariant 3); every pointer argument is null except
    // `available`, a valid out pointer for the call.
    let ok = unsafe {
        PeekNamedPipe(
            pipe.as_raw_handle(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            &mut available,
            std::ptr::null_mut(),
        )
    };
    if ok != 0 {
        return Ok(Pending::Bytes(available as usize));
    }
    match std::io::Error::last_os_error().raw_os_error() {
        Some(code) if code as u32 == ERROR_BROKEN_PIPE => Ok(Pending::Closed),
        _ => Err(Failure::Read),
    }
}

/// Polls until bytes (or the end of the stream) are reported or `deadline`, then reads only what
/// was reported, so a synchronous handle never blocks past the deadline.
fn read_pipe(
    pipe: &OwnedHandle,
    deadline: Instant,
    max_bytes: usize,
) -> Result<Option<Vec<u8>>, Failure> {
    let mut line = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if Instant::now() >= deadline {
            return Ok(None);
        }
        let wanted = match pending(pipe)? {
            Pending::Bytes(0) => {
                std::thread::sleep(Duration::from_millis(5));
                continue;
            }
            Pending::Bytes(count) => count.min(chunk.len()),
            Pending::Closed => 0,
        };
        let count = if wanted == 0 {
            Some(0)
        } else {
            read_available(pipe, &mut chunk[..wanted], deadline)?
        };
        let Some(count) = count else {
            return Ok(None);
        };
        if count == 0 && line.is_empty() {
            return Ok(None);
        }
        let end = chunk[..count]
            .iter()
            .position(|byte| matches!(byte, b'\r' | b'\n'));
        line.extend_from_slice(&chunk[..end.unwrap_or(count)]);
        if line.len() > max_bytes {
            return Err(Failure::Read);
        }
        if count == 0 || end.is_some() {
            return Ok(Some(line));
        }
    }
}

/// One `ReadFile` of at most `buffer.len()` bytes that the pipe reported as available.
/// `Ok(None)` when the deadline passed with the read still pending.
///
/// Every read passes an `OVERLAPPED` with its own event, so a handle the launcher opened for
/// overlapped I/O is also correct. The `OVERLAPPED`, the event and `buffer` live on this frame
/// until `GetOverlappedResult` has returned, which is what makes a pending read safe to abandon.
fn read_available(
    pipe: &OwnedHandle,
    buffer: &mut [u8],
    deadline: Instant,
) -> Result<Option<usize>, Failure> {
    let handle = pipe.as_raw_handle();
    // SAFETY: a manual-reset, initially non-signalled, unnamed event with default security.
    let event = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
    if event.is_null() {
        return Err(Failure::Read);
    }
    // SAFETY: `CreateEventW` just returned `event` and nothing else owns it; this is the only
    // owner, so it is closed exactly once when this frame ends (invariant 3).
    let event = unsafe { OwnedHandle::from_raw_handle(event) };
    // SAFETY: an all-zero `OVERLAPPED` is a valid initial value (offsets zero, no event yet).
    let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
    overlapped.hEvent = event.as_raw_handle();
    let mut read = 0u32;
    let length = u32::try_from(buffer.len()).unwrap_or(u32::MAX);
    // SAFETY: `handle` is live (the caller's owner outlives this call); `buffer` is valid for
    // `length` bytes; `read` and `overlapped` are valid for the call and, if it goes pending, until
    // the `GetOverlappedResult` below completes (invariant 4).
    let started = unsafe {
        ReadFile(
            handle,
            buffer.as_mut_ptr().cast(),
            length,
            &mut read,
            &mut overlapped,
        )
    };
    if started == 0 {
        match std::io::Error::last_os_error()
            .raw_os_error()
            .map(|code| code as u32)
        {
            Some(ERROR_IO_PENDING) => {
                let left = deadline.saturating_duration_since(Instant::now());
                let millis = u32::try_from(left.as_millis()).unwrap_or(u32::MAX);
                // SAFETY: `event` is a live event handle owned by this frame.
                let waited = unsafe { WaitForSingleObject(event.as_raw_handle(), millis) };
                if waited != WAIT_OBJECT_0 {
                    // SAFETY: cancels only this operation on `handle`; the wait that follows
                    // returns once the kernel has finished with `buffer` and `overlapped`, so
                    // neither is released while still in use (invariant 4).
                    unsafe {
                        CancelIoEx(handle, &overlapped);
                        GetOverlappedResult(handle, &overlapped, &mut read, 1);
                    }
                    return Ok(None);
                }
                // SAFETY: the event is signalled, so the operation completed; `read` is a valid
                // out pointer and `overlapped` is the structure the read used.
                if unsafe { GetOverlappedResult(handle, &overlapped, &mut read, 0) } == 0 {
                    return Err(Failure::Read);
                }
            }
            Some(ERROR_BROKEN_PIPE | ERROR_HANDLE_EOF) => read = 0,
            _ => return Err(Failure::Read),
        }
    }
    Ok(Some(read as usize))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use std::io::Write;

    const MAX: usize = 1 << 20;
    const LONG: Duration = Duration::from_secs(5);

    unsafe extern "C" {
        fn _open_osfhandle(handle: isize, flags: c_int) -> c_int;
        fn _close(fd: c_int) -> c_int;
    }

    /// A CRT descriptor for the read end of an anonymous pipe, closed with the guard. The writer
    /// is returned too, so a test controls when the stream ends.
    struct Descriptor(c_int);
    impl Drop for Descriptor {
        fn drop(&mut self) {
            // SAFETY: closes the descriptor `pipe` opened, once; the CRT also closes its handle.
            unsafe { _close(self.0) };
        }
    }

    fn pipe() -> (std::io::PipeWriter, Descriptor) {
        let (reader, writer) = std::io::pipe().unwrap();
        let handle = OwnedHandle::from(reader);
        // SAFETY: `_open_osfhandle` takes over `handle`'s value; `into_raw_handle` releases it from
        // the owner first, so the CRT is its only closer.
        let fd = unsafe {
            use std::os::windows::io::IntoRawHandle;
            _open_osfhandle(handle.into_raw_handle() as isize, 0)
        };
        assert!(fd >= 0);
        (writer, Descriptor(fd))
    }

    fn read(fd: &Descriptor, timeout: Duration, max: usize) -> Result<Option<Vec<u8>>, Failure> {
        read_first_line(i64::from(fd.0), timeout, max)
    }

    #[test]
    fn a_complete_line_is_returned_while_the_writer_stays_open() {
        let (mut writer, fd) = pipe();
        writer.write_all(b"{\"a\":1}\r\nrest").unwrap();
        assert_eq!(read(&fd, LONG, MAX).unwrap().unwrap(), b"{\"a\":1}");
    }

    #[test]
    fn an_unterminated_line_ends_at_eof_and_no_data_is_no_envelope() {
        let (mut writer, fd) = pipe();
        writer.write_all(b"partial").unwrap();
        drop(writer);
        assert_eq!(read(&fd, LONG, MAX).unwrap().unwrap(), b"partial");
        let (writer, fd) = pipe();
        drop(writer);
        assert_eq!(read(&fd, LONG, MAX).unwrap(), None);
    }

    #[test]
    fn a_silent_or_partial_open_writer_times_out_without_an_error() {
        let (mut writer, fd) = pipe();
        assert_eq!(read(&fd, Duration::from_millis(50), MAX).unwrap(), None);
        writer.write_all(b"partial").unwrap();
        assert_eq!(read(&fd, Duration::from_millis(50), MAX).unwrap(), None);
    }

    #[test]
    fn an_oversized_line_is_a_read_failure() {
        let (mut writer, fd) = pipe();
        writer.write_all(&[b'x'; 64]).unwrap();
        assert_eq!(read(&fd, LONG, 16), Err(Failure::Read));
    }

    #[test]
    fn closed_and_out_of_range_descriptors_are_no_envelope() {
        for fd in [-1, 999_999, i64::from(i32::MAX), i64::MAX] {
            assert_eq!(read_first_line(fd, LONG, MAX).unwrap(), None);
        }
    }

    #[test]
    fn the_first_line_rules() {
        assert_eq!(first_line(b"", 8).unwrap(), None);
        assert_eq!(first_line(b"a\nb", 8).unwrap().unwrap(), b"a");
        assert_eq!(first_line(b"a\rb", 8).unwrap().unwrap(), b"a");
        assert_eq!(first_line(b"abc", 2), Err(Failure::Read));
    }
}
