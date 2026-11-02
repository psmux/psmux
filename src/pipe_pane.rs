use std::io::{self, Read, Write};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::TrySendError;

pub(crate) type PipeWriter = Arc<Mutex<Box<dyn Write + Send>>>;

/// Snapshot ownership before writing: only the source pane reader may stall.
pub(crate) fn tee_output(
    registry: &Mutex<Vec<(usize, PipeWriter)>>,
    count: &AtomicUsize,
    pane_id: usize,
    bytes: &[u8],
) {
    let sinks: Vec<PipeWriter> = match registry.lock() {
        Ok(writers) => writers.iter().filter(|(id, _)| *id == pane_id)
            .map(|(_, writer)| writer.clone()).collect(),
        Err(_) => return,
    };
    for sink in sinks {
        let failed = match sink.lock() {
            Ok(mut writer) => writer.write_all(bytes).is_err() || writer.flush().is_err(),
            Err(_) => true,
        };
        if failed {
            remove_writer(registry, count, &sink);
        }
    }
}

pub(crate) fn remove_writer(
    registry: &Mutex<Vec<(usize, PipeWriter)>>,
    count: &AtomicUsize,
    sink: &PipeWriter,
) {
    if let Ok(mut writers) = registry.lock() {
        let before = writers.len();
        writers.retain(|(_, writer)| !Arc::ptr_eq(writer, sink));
        count.fetch_sub(before - writers.len(), Ordering::Relaxed);
    }
}

/// Only leading options belong to pipe-pane. The command's flags are opaque.
pub(crate) fn options(args: &[&str]) -> (bool, bool, bool, String) {
    let (mut input, mut output, mut toggle) = (false, false, false);
    let mut start = 0;
    while let Some(arg) = args.get(start) {
        if *arg == "--" { start += 1; break; }
        if !arg.starts_with('-') || arg.len() == 1
            || !arg[1..].chars().all(|c| matches!(c, 'I' | 'O' | 'o')) {
            break;
        }
        for c in arg[1..].chars() {
            match c {
                'I' => input = true,
                'O' => output = true,
                'o' => toggle = true,
                _ => unreachable!(),
            }
        }
        start += 1;
    }
    (input, output || !input, toggle, args[start..].join(" "))
}

pub(crate) const INPUT_CHUNK: usize = 8192;

/// Retry pending input before reading again. With no pending input, the retry
/// detects a failed pane writer while stdout is idle.
pub(crate) fn pump_input<R: Read>(
    reader: &mut R,
    pending: &mut Option<Vec<u8>>,
    writer: &mut dyn crate::pane::PaneInputSink,
    ready: impl FnOnce(&R) -> io::Result<bool>,
) -> io::Result<bool> {
    match writer.try_queue_pipe(pending.take().unwrap_or_default()) {
        Ok(()) => {}
        Err(TrySendError::Full(bytes)) => { *pending = Some(bytes); return Ok(true); }
        Err(TrySendError::Disconnected(_)) => return Err(io::ErrorKind::BrokenPipe.into()),
    }
    if !ready(reader)? { return Ok(true); }
    let mut buffer = [0; INPUT_CHUNK];
    match reader.read(&mut buffer) {
        Ok(0) => Ok(false),
        Ok(n) => {
            let bytes = buffer[..n].to_vec();
            match writer.try_queue_pipe(bytes) {
                Ok(()) => {}
                Err(TrySendError::Full(bytes)) => *pending = Some(bytes),
                Err(TrySendError::Disconnected(_)) => return Err(io::ErrorKind::BrokenPipe.into()),
            }
            Ok(true)
        }
        Err(e) if matches!(e.kind(), io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock) => Ok(true),
        Err(e) => Err(e),
    }
}

#[cfg(windows)]
pub(crate) fn ready(stdout: &std::process::ChildStdout) -> io::Result<bool> {
    use std::os::windows::io::AsRawHandle;
    #[link(name = "kernel32")]
    extern "system" {
        fn PeekNamedPipe(handle: *mut std::ffi::c_void, buffer: *mut u8, size: u32,
            read: *mut u32, available: *mut u32, left: *mut u32) -> i32;
    }
    let mut available = 0;
    // ChildStdout owns this pipe and the server is its only reader.
    let ok = unsafe { PeekNamedPipe(stdout.as_raw_handle(), std::ptr::null_mut(), 0,
        std::ptr::null_mut(), &mut available, std::ptr::null_mut()) };
    if ok == 0 {
        let e = io::Error::last_os_error();
        // A broken pipe is readable EOF; buffered bytes are drained first.
        if e.raw_os_error() == Some(109) { return Ok(true); }
        return Err(e);
    }
    Ok(available > 0)
}

#[cfg(unix)]
pub(crate) fn ready(stdout: &std::process::ChildStdout) -> io::Result<bool> {
    use std::os::fd::AsRawFd;
    #[repr(C)]
    struct PollFd { fd: i32, events: i16, revents: i16 }
    extern "C" { fn poll(fds: *mut PollFd, count: usize, timeout: i32) -> i32; }
    let mut fd = PollFd { fd: stdout.as_raw_fd(), events: 1, revents: 0 };
    let result = unsafe { poll(&mut fd, 1, 0) };
    if result < 0 { return Err(io::Error::last_os_error()); }
    Ok(result > 0)
}

#[cfg(test)]
#[path = "../tests-rs/test_pipe_pane_input.rs"]
mod tests;
