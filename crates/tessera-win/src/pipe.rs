//! Named pipe for control via `tesserac` and the editor. Line-based:
//! one request line and one response line per connection. The server passes
//! the lines through unchanged; the client [`call`] speaks the JSON protocol
//! from [`tessera_core::ipc`].

use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, HANDLE};
use windows::Win32::Storage::FileSystem::{
    FILE_FLAG_FIRST_PIPE_INSTANCE, FlushFileBuffers, PIPE_ACCESS_DUPLEX,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT, WaitNamedPipeW,
};
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::core::HSTRING;

use tessera_core::ipc::{Request, Response};

/// Upper limit for a request line.
const MAX_REQUEST: u64 = 64 * 1024;
const BUFFER_SIZE: u32 = 64 * 1024;

/// Pipe name per user session, so parallel RDP sessions don't interfere.
pub fn pipe_name() -> String {
    let mut session = 0;
    // SAFETY: output parameter points to a local variable.
    let _ = unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session) };
    format!(r"\\.\pipe\tessera-{session}")
}

/// Starts the server thread. `handler` receives the request line and returns
/// the response line; requests are handled one after another.
///
/// Fails if the pipe already exists (another process holds the name).
pub fn serve(handler: impl Fn(&str) -> String + Send + 'static) -> anyhow::Result<()> {
    let name = HSTRING::from(pipe_name());
    // SAFETY: ownership of the handle passes to the `File`, which closes it.
    let pipe = unsafe {
        let handle = CreateNamedPipeW(
            &name,
            PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            BUFFER_SIZE,
            BUFFER_SIZE,
            0,
            None,
        );
        if handle.is_invalid() {
            return Err(io::Error::last_os_error().into());
        }
        File::from_raw_handle(handle.0)
    };

    std::thread::Builder::new()
        .name("ipc".into())
        .spawn(move || {
            loop {
                if let Err(e) = serve_one(&pipe, &handler) {
                    tracing::warn!("IPC connection failed: {e}");
                }
            }
        })?;
    Ok(())
}

fn serve_one(pipe: &File, handler: &impl Fn(&str) -> String) -> io::Result<()> {
    let handle = HANDLE(pipe.as_raw_handle());
    // SAFETY: valid pipe handle, synchronous call.
    if let Err(e) = unsafe { ConnectNamedPipe(handle, None) } {
        // Client connected between Create and Connect - that is fine.
        if e.code() != ERROR_PIPE_CONNECTED.to_hresult() {
            return Err(e.into());
        }
    }

    let result = (|| {
        let mut line = String::new();
        BufReader::new(pipe.take(MAX_REQUEST)).read_line(&mut line)?;
        let mut response = handler(line.trim_end());
        response.push('\n');
        let mut writer = pipe;
        writer.write_all(response.as_bytes())?;
        // SAFETY: waits until the client has read everything.
        unsafe { FlushFileBuffers(handle)? };
        Ok(())
    })();

    // SAFETY: disconnects the client; the pipe instance stays for the next one.
    unsafe { DisconnectNamedPipe(handle)? };
    result
}

/// Error while talking to the daemon. The texts are meant for users.
#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    /// Pipe does not exist.
    #[error("Tessera is not running.")]
    NotRunning,
    /// Pipe stayed busy until the timeout expired.
    #[error("Daemon not reachable: daemon is busy")]
    Busy,
    #[error("Daemon not reachable: {0}")]
    Io(#[source] io::Error),
    /// Response line is not valid protocol JSON.
    #[error("Unintelligible response from daemon: {0}")]
    BadResponse(#[source] serde_json::Error),
}

impl From<io::Error> for IpcError {
    fn from(e: io::Error) -> Self {
        match e.kind() {
            io::ErrorKind::NotFound => Self::NotRunning,
            io::ErrorKind::TimedOut => Self::Busy,
            _ => Self::Io(e),
        }
    }
}

/// Sends a request to the daemon and returns its response.
/// `timeout` limits the wait for a free pipe instance.
pub fn call(request: &Request, timeout: Duration) -> Result<Response, IpcError> {
    let line = serde_json::to_string(request).expect("request is serializable");
    let answer = exchange(&line, timeout)?;
    serde_json::from_str(&answer).map_err(IpcError::BadResponse)
}

/// Sends a request line to the daemon and returns the response line.
/// `ErrorKind::NotFound` if no daemon is running, `ErrorKind::TimedOut`
/// if the pipe stays busy until `timeout`.
fn exchange(line: &str, timeout: Duration) -> io::Result<String> {
    let name = pipe_name();
    let deadline = Instant::now() + timeout;
    let mut pipe = loop {
        match OpenOptions::new().read(true).write(true).open(&name) {
            Ok(pipe) => break pipe,
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY.0 as i32) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(io::Error::new(io::ErrorKind::TimedOut, "daemon is busy"));
                }
                // SAFETY: valid, null-terminated name.
                let _ =
                    unsafe { WaitNamedPipeW(&HSTRING::from(&name), remaining.as_millis() as u32) };
            }
            Err(e) => return Err(e),
        }
    };

    pipe.write_all(line.as_bytes())?;
    pipe.write_all(b"\n")?;
    let mut response = String::new();
    BufReader::new(pipe).read_line(&mut response)?;
    Ok(response.trim_end().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_errors_are_classified() {
        let not_found = io::Error::new(io::ErrorKind::NotFound, "gone");
        assert!(matches!(IpcError::from(not_found), IpcError::NotRunning));
        let timed_out = io::Error::new(io::ErrorKind::TimedOut, "busy");
        assert!(matches!(IpcError::from(timed_out), IpcError::Busy));
        let other = io::Error::new(io::ErrorKind::BrokenPipe, "aborted");
        assert!(matches!(IpcError::from(other), IpcError::Io(_)));
    }

    #[test]
    fn messages_unchanged() {
        assert_eq!(IpcError::NotRunning.to_string(), "Tessera is not running.");
        assert_eq!(
            IpcError::Busy.to_string(),
            "Daemon not reachable: daemon is busy"
        );
        let io = io::Error::new(io::ErrorKind::BrokenPipe, "aborted");
        assert_eq!(
            IpcError::Io(io).to_string(),
            "Daemon not reachable: aborted"
        );
    }
}
