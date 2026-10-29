//! Code shared by `tessera.exe` (daemon) and `tesserac.exe` (CLI).

pub mod daemon;
pub mod logging;
pub mod paths;
mod tray;
mod watch;
mod wm;
