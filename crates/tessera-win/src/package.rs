//! Package identity: whether Tessera runs as an MSIX package (Microsoft Store)
//! or as a plain exe (zip release, `cargo run`).

use std::sync::OnceLock;

use windows::Win32::Foundation::{APPMODEL_ERROR_NO_PACKAGE, ERROR_SUCCESS};
use windows::Win32::Storage::Packaging::Appx::{
    GetCurrentPackageFamilyName, GetCurrentPackageFullName,
};
use windows::core::PWSTR;

/// `true` if the process has package identity (installed from an MSIX).
pub fn is_packaged() -> bool {
    static PACKAGED: OnceLock<bool> = OnceLock::new();
    *PACKAGED.get_or_init(|| {
        let mut len = 0u32;
        // SAFETY: with a null buffer the call only reports the required length.
        let status = unsafe { GetCurrentPackageFullName(&mut len, None) };
        status != APPMODEL_ERROR_NO_PACKAGE
    })
}

/// Package family name (`<name>_<publisher hash>`), which names the
/// package's data folder under `%LOCALAPPDATA%\Packages`; `None` without
/// package identity.
pub fn family_name() -> Option<String> {
    if !is_packaged() {
        return None;
    }
    let mut len = 0u32;
    // SAFETY: with a null buffer the call only reports the required length.
    let _ = unsafe { GetCurrentPackageFamilyName(&mut len, None) };
    let mut buf = vec![0u16; len as usize];
    // SAFETY: the buffer holds `len` characters, as the call expects.
    let status = unsafe { GetCurrentPackageFamilyName(&mut len, Some(PWSTR(buf.as_mut_ptr()))) };
    if status != ERROR_SUCCESS {
        return None;
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..end]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_process_has_no_package() {
        assert!(!is_packaged());
        assert_eq!(family_name(), None);
    }
}
