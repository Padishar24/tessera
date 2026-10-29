//! Autostart via `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.

use std::path::Path;

use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};
use windows::core::{HSTRING, PCWSTR, w};

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const VALUE: PCWSTR = w!("Tessera");
/// Entry under the former project name.
const LEGACY_VALUE: PCWSTR = w!("TilingMgr");

pub fn is_enabled() -> bool {
    exists(VALUE)
}

/// Registers `exe` (quoted) or removes the entry.
pub fn set(enabled: bool, exe: &Path) -> anyhow::Result<()> {
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
        } else if is_enabled() {
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
/// there was an old entry.
pub fn migrate_legacy(exe: &Path) -> anyhow::Result<bool> {
    if !exists(LEGACY_VALUE) {
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
