//! Logging to daily files under `%LOCALAPPDATA%\Tessera\logs`.

use std::backtrace::Backtrace;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{Builder, Rotation};
use tracing_subscriber::EnvFilter;

use crate::paths;

/// The returned guard must live until the program exits, otherwise
/// buffered log lines are lost. `TESSERA_LOG` overrides `level`.
/// `prefix` distinguishes the log files of daemon and editor.
pub fn init(level: &str, prefix: &str) -> anyhow::Result<WorkerGuard> {
    let dir = paths::log_dir();
    std::fs::create_dir_all(&dir)?;
    let appender = Builder::new()
        .rotation(Rotation::DAILY)
        .filename_prefix(prefix)
        .filename_suffix("log")
        .max_log_files(7)
        .build(&dir)?;
    let (writer, guard) = tracing_appender::non_blocking(appender);

    let filter = EnvFilter::try_from_env("TESSERA_LOG").unwrap_or_else(|_| EnvFilter::new(level));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer)
        .with_ansi(false)
        .init();

    // The daemon program has no console – panics must go to the log.
    std::panic::set_hook(Box::new(|info| {
        tracing::error!("Panic: {info}\n{}", Backtrace::force_capture());
    }));
    Ok(guard)
}
