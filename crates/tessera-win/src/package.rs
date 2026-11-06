//! Package identity: whether Tessera runs as an MSIX package (Microsoft Store)
//! or as a plain exe (zip release, `cargo run`).

use std::sync::OnceLock;

use windows::Win32::Foundation::APPMODEL_ERROR_NO_PACKAGE;
use windows::Win32::Storage::Packaging::Appx::GetCurrentPackageFullName;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_process_has_no_package() {
        assert!(!is_packaged());
    }
}
