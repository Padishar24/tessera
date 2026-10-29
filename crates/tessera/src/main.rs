//! `tessera.exe` – the daemon (without a console window); with `editor`, the
//! zone editor.

#![windows_subsystem = "windows"]

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use tessera::{daemon, logging, paths};
use tessera_core::config::LoadedConfig;
use tessera_win::shell::{self, MessageKind};

/// Tessera – zones and auto-tiling for Windows.
#[derive(Parser)]
#[command(version)]
struct Args {
    /// Alternative configuration file instead of %APPDATA%\Tessera\config.toml.
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    /// Log level (error, warn, info, debug, trace); TESSERA_LOG takes precedence.
    #[arg(long, default_value = "info", global = true)]
    log_level: String,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Open the graphical zone editor.
    Editor {
        /// Monitor ID (default: monitor under the mouse pointer).
        #[arg(long)]
        monitor: Option<String>,
        /// Layout to open first.
        #[arg(long)]
        layout: Option<String>,
    },
}

fn main() -> ExitCode {
    let args = match Args::try_parse() {
        Ok(args) => args,
        Err(e) => {
            // Without a console, help and error texts end up in a message box.
            let kind = if e.use_stderr() {
                MessageKind::Error
            } else {
                MessageKind::Info
            };
            shell::message_box("Tessera", &e.to_string(), kind);
            return if e.use_stderr() {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            };
        }
    };

    let prefix = match args.command {
        None => "tessera",
        Some(Command::Editor { .. }) => "editor",
    };
    let _log_guard = match logging::init(&args.log_level, prefix) {
        Ok(guard) => Some(guard),
        Err(e) => {
            shell::message_box(
                "Tessera",
                &format!("Logging could not be started:\n{e:#}"),
                MessageKind::Warning,
            );
            None
        }
    };

    let result = match args.command {
        None => daemon::run(args.config),
        Some(Command::Editor { monitor, layout }) => run_editor(args.config, monitor, layout),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("Aborted: {e:#}");
            shell::message_box("Tessera", &format!("Aborted:\n{e:#}"), MessageKind::Error);
            ExitCode::FAILURE
        }
    }
}

fn run_editor(
    config: Option<PathBuf>,
    monitor: Option<String>,
    layout: Option<String>,
) -> anyhow::Result<()> {
    let config_path = match config {
        Some(path) => path,
        None => paths::ensure_config()?,
    };
    // A faulty configuration must not block the editor – not least because
    // it is used to repair a broken layout.
    let config = paths::load_config(&config_path).unwrap_or_else(|e| {
        tracing::warn!("Configuration faulty, editor uses defaults: {e:#}");
        LoadedConfig::builtin()
    });
    tessera_editor::run(tessera_editor::Options {
        config,
        layouts_path: paths::layouts_file_for(&config_path),
        monitor,
        layout,
    })
}
