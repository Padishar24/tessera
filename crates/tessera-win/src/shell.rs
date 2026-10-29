//! Helpers for Explorer/shell and simple message boxes.

use std::path::Path;

use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    MB_ICONERROR, MB_ICONINFORMATION, MB_ICONWARNING, MB_OK, MB_SETFOREGROUND, MessageBoxW,
    SW_SHOWNORMAL,
};
use windows::core::{HSTRING, w};

/// Opens a file or folder with the associated application. Files without
/// an association (e.g. `.toml`) are opened in Notepad.
pub fn open_path(path: &Path) -> anyhow::Result<()> {
    let target = HSTRING::from(path.as_os_str());
    // SAFETY: valid, null-terminated strings.
    let result = unsafe { ShellExecuteW(None, w!("open"), &target, None, None, SW_SHOWNORMAL) };
    // Per the docs, values <= 32 indicate an error.
    if result.0 as usize > 32 {
        return Ok(());
    }
    std::process::Command::new("notepad.exe")
        .arg(path)
        .spawn()?;
    Ok(())
}

#[derive(Debug, Clone, Copy)]
pub enum MessageKind {
    Info,
    Warning,
    Error,
}

/// Modal message box without a parent window.
pub fn message_box(title: &str, text: &str, kind: MessageKind) {
    let icon = match kind {
        MessageKind::Info => MB_ICONINFORMATION,
        MessageKind::Warning => MB_ICONWARNING,
        MessageKind::Error => MB_ICONERROR,
    };
    // SAFETY: valid, null-terminated strings.
    unsafe {
        MessageBoxW(
            None,
            &HSTRING::from(text),
            &HSTRING::from(title),
            MB_OK | MB_SETFOREGROUND | icon,
        );
    }
}

/// Like [`message_box`], but does not block the calling thread.
pub fn message_box_async(title: &str, text: &str, kind: MessageKind) {
    let (title, text) = (title.to_owned(), text.to_owned());
    std::thread::spawn(move || message_box(&title, &text, kind));
}
