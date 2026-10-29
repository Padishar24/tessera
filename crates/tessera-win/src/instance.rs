//! Ensures that only one daemon runs per user session.

use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
use windows::Win32::System::Threading::CreateMutexW;
use windows::core::HSTRING;

/// Named mutex that is released on drop.
pub struct SingleInstance(HANDLE);

impl SingleInstance {
    /// `None` if an instance with this name is already running.
    pub fn acquire(name: &str) -> anyhow::Result<Option<Self>> {
        let name = HSTRING::from(format!("Local\\{name}"));
        // SAFETY: valid, null-terminated name; handle is closed in Drop.
        unsafe {
            let handle = CreateMutexW(None, true, &name)?;
            if GetLastError() == ERROR_ALREADY_EXISTS {
                let _ = CloseHandle(handle);
                return Ok(None);
            }
            Ok(Some(Self(handle)))
        }
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        // SAFETY: handle comes from CreateMutexW and is only closed here.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
