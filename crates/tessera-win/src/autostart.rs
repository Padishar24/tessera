//! Autostart. The plain exe uses
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`; the MSIX package
//! (Microsoft Store) uses the `windows.startupTask` declared in its manifest,
//! because a Run entry cannot start an exe inside `WindowsApps`.

use std::path::Path;

use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};
use windows::core::{HSTRING, PCWSTR, w};

use crate::package::is_packaged;

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const VALUE: PCWSTR = w!("Tessera");
/// Entry under the former project name.
const LEGACY_VALUE: PCWSTR = w!("TilingMgr");

pub fn is_enabled() -> bool {
    if is_packaged() {
        return startup_task::is_enabled().unwrap_or_else(|e| {
            tracing::warn!("Startup task: {e:#}");
            false
        });
    }
    exists(VALUE)
}

/// Turns autostart on or off. The plain exe registers `exe` (quoted).
pub fn set(enabled: bool, exe: &Path) -> anyhow::Result<()> {
    if is_packaged() {
        return startup_task::set(enabled);
    }
    // SAFETY: strings live for the duration of the calls; size in bytes incl. null terminator.
    unsafe {
        let status = if enabled {
            let command = HSTRING::from(format!("\"{}\"", exe.display()));
            let bytes = (command.len() + 1) * size_of::<u16>();
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                RUN_KEY,
                VALUE,
                REG_SZ.0,
                Some(command.as_ptr().cast()),
                bytes as u32,
            )
        } else if exists(VALUE) {
            RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, VALUE)
        } else {
            ERROR_SUCCESS
        };
        status.ok()?;
    }
    Ok(())
}

/// Replaces an autostart entry under the former name "TilingMgr"
/// (points to the old `tilingmgr.exe`) with one for `exe`. `true` if
/// there was an old entry. Does nothing in the MSIX package.
pub fn migrate_legacy(exe: &Path) -> anyhow::Result<bool> {
    if is_packaged() || !exists(LEGACY_VALUE) {
        return Ok(false);
    }
    set(true, exe)?;
    // SAFETY: name is a valid, null-terminated string.
    unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, LEGACY_VALUE).ok()? };
    Ok(true)
}

fn exists(value: PCWSTR) -> bool {
    // SAFETY: only queries existence and size.
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            value,
            RRF_RT_REG_SZ,
            None,
            None,
            None,
        ) == ERROR_SUCCESS
    }
}

/// The `windows.startupTask` of the MSIX package.
mod startup_task {
    use anyhow::{Context, bail};
    use windows::ApplicationModel::{StartupTask, StartupTaskState};
    use windows::core::HSTRING;

    /// Must match `TaskId` in `packaging/msix/AppxManifest.xml`.
    const TASK_ID: &str = "TesseraStartup";

    pub fn is_enabled() -> anyhow::Result<bool> {
        let state = on_worker(|| task()?.State().map_err(Into::into))?;
        Ok(matches!(
            state,
            StartupTaskState::Enabled | StartupTaskState::EnabledByPolicy
        ))
    }

    pub fn set(enabled: bool) -> anyhow::Result<()> {
        let state = on_worker(move || {
            let task = task()?;
            if enabled {
                Ok(task.RequestEnableAsync()?.join()?)
            } else {
                task.Disable()?;
                Ok(task.State()?)
            }
        })?;
        match state {
            StartupTaskState::Enabled | StartupTaskState::Disabled => Ok(()),
            StartupTaskState::EnabledByPolicy if enabled => Ok(()),
            StartupTaskState::DisabledByUser => bail!(
                "Autostart was turned off in Windows. Turn it on under \
                 Settings → Apps → Startup (or in Task Manager → Startup apps)."
            ),
            StartupTaskState::DisabledByPolicy | StartupTaskState::EnabledByPolicy => {
                bail!("Autostart is controlled by a group policy.")
            }
            other => bail!("Unexpected startup task state {}", other.0),
        }
    }

    fn task() -> anyhow::Result<StartupTask> {
        StartupTask::GetAsync(&HSTRING::from(TASK_ID))?
            .join()
            .with_context(|| format!("startup task {TASK_ID} not found in the package manifest"))
    }

    /// Runs the blocking WinRT calls on a short-lived thread (implicit MTA),
    /// so they never wait inside the caller's message loop or STA.
    fn on_worker<T: Send>(f: impl FnOnce() -> anyhow::Result<T> + Send) -> anyhow::Result<T> {
        std::thread::scope(|s| {
            s.spawn(f)
                .join()
                .unwrap_or_else(|_| bail!("startup task thread panicked"))
        })
    }
}
