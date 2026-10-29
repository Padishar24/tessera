//! `tesserac.exe` – command line for controlling and diagnosing the daemon.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand, ValueEnum};
use tessera::paths;
use tessera_core::action::Action;
use tessera_core::config::{DEFAULT_CONFIG, LoadedConfig};
use tessera_core::ipc::{Request, Response, StateSnapshot};
use tessera_core::tree::ZoneMode;

#[derive(Parser)]
#[command(version, about = "Diagnostics and control for Tessera")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Send an action to the running daemon, e.g. `msg focus left`.
    Msg {
        /// Action with argument, e.g. `move-to-zone 2`.
        #[arg(required = true, num_args = 1..)]
        action: Vec<String>,
    },
    /// Set the layout on a monitor (current desktop).
    SetLayout {
        /// Name from layouts.toml.
        layout: String,
        /// Monitor ID (default: primary monitor).
        #[arg(long)]
        monitor: Option<String>,
    },
    /// Query the state of the running daemon.
    Query {
        #[arg(value_enum, default_value_t = QueryKind::State)]
        what: QueryKind,
    },
    /// Check a configuration file and list its shortcuts.
    CheckConfig {
        /// File to check (default: %APPDATA%\Tessera\config.toml).
        path: Option<PathBuf>,
    },
    /// Save, apply, list or delete scenes (arrangements of all monitors).
    Scene {
        #[command(subcommand)]
        command: SceneCommand,
    },
    /// List, create or remove virtual desktops.
    Desktop {
        #[command(subcommand)]
        command: DesktopCommand,
    },
    /// Query virtual desktops (compatibility test of the desktop API).
    VdeskProbe,
    /// Show the locations of configuration and logs.
    Paths,
}

#[derive(Subcommand)]
enum SceneCommand {
    /// Save the current arrangement (replaces a scene with the same name).
    Save {
        name: String,
        /// Apply it automatically whenever exactly these monitors are
        /// connected.
        #[arg(long)]
        auto: bool,
        /// When applying it, start programs that have no window yet.
        #[arg(long)]
        launch: bool,
    },
    /// Apply a saved scene on the current desktop.
    Apply { name: String },
    /// Show all saved scenes.
    List,
    /// Delete a saved scene.
    Delete { name: String },
}

#[derive(Subcommand)]
enum DesktopCommand {
    /// Show all desktops (like vdesk-probe).
    List,
    /// Create a new desktop at the end.
    New,
    /// Remove a desktop; its windows move to the first (or second) desktop.
    Remove {
        /// Number of the desktop (1-based).
        number: usize,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum QueryKind {
    /// Full state as JSON.
    State,
    /// Managed windows as a table.
    Windows,
    /// Monitors as a table.
    Monitors,
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Msg { action } => msg(&action.join(" ")),
        Command::Query { what } => query(what),
        Command::SetLayout { layout, monitor } => set_layout(layout, monitor),
        Command::CheckConfig { path } => check_config(path),
        Command::Scene { command } => scene(command),
        Command::VdeskProbe
        | Command::Desktop {
            command: DesktopCommand::List,
        } => vdesk_probe(),
        Command::Desktop {
            command: DesktopCommand::New,
        } => report(tessera_win::vdesk::create()),
        Command::Desktop {
            command: DesktopCommand::Remove { number },
        } => {
            if number == 0 {
                eprintln!("Desktops start at 1");
                return ExitCode::FAILURE;
            }
            let fallback = if number == 1 { 1 } else { 0 };
            report(tessera_win::vdesk::remove(number - 1, fallback))
        }
        Command::Paths => {
            println!("Configuration: {}", paths::config_file().display());
            println!("Logs:          {}", paths::log_dir().display());
            ExitCode::SUCCESS
        }
    }
}

/// Sends a request to the daemon; errors have already been printed.
fn send(request: &Request) -> Option<Response> {
    tessera_win::pipe::call(request, Duration::from_secs(10))
        .inspect_err(|e| eprintln!("{e}"))
        .ok()
}

fn msg(action: &str) -> ExitCode {
    // Check early so that typos are caught even without a daemon.
    if let Err(e) = action.parse::<Action>() {
        eprintln!("{e}");
        return ExitCode::FAILURE;
    }
    match send(&Request::Action {
        action: action.to_owned(),
    }) {
        Some(Response::Ok) => ExitCode::SUCCESS,
        Some(Response::Error { message }) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
        Some(Response::State(_) | Response::Scenes { .. }) | None => ExitCode::FAILURE,
    }
}

fn scene(command: SceneCommand) -> ExitCode {
    let request = match command {
        SceneCommand::Save { name, auto, launch } => Request::SaveScene { name, auto, launch },
        SceneCommand::Apply { name } => Request::ApplyScene { name },
        SceneCommand::Delete { name } => Request::DeleteScene { name },
        SceneCommand::List => Request::ListScenes,
    };
    match send(&request) {
        Some(Response::Ok) => ExitCode::SUCCESS,
        Some(Response::Scenes { scenes }) => {
            if scenes.is_empty() {
                println!("No scenes saved yet. Save one with `tesserac scene save <name>`.");
            }
            for s in &scenes {
                let mut flags = Vec::new();
                if s.auto {
                    flags.push("auto");
                }
                if s.launch {
                    flags.push("launch");
                }
                println!(
                    "{} {}{}\n   {} window(s) on {}",
                    if s.current_monitors { "*" } else { " " },
                    s.name,
                    if flags.is_empty() {
                        String::new()
                    } else {
                        format!("  ({})", flags.join(", "))
                    },
                    s.windows,
                    s.monitors.join(" + "),
                );
            }
            if scenes.iter().any(|s| s.current_monitors) {
                println!("\n* = made for the monitors connected now");
            }
            ExitCode::SUCCESS
        }
        Some(Response::Error { message }) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
        Some(Response::State(_)) | None => ExitCode::FAILURE,
    }
}

fn set_layout(layout: String, monitor: Option<String>) -> ExitCode {
    let monitor = match monitor {
        Some(monitor) => monitor,
        None => match send(&Request::QueryState) {
            Some(Response::State(state)) => match state.monitors.into_iter().find(|m| m.primary) {
                Some(m) => m.id,
                None => {
                    eprintln!("no primary monitor");
                    return ExitCode::FAILURE;
                }
            },
            _ => return ExitCode::FAILURE,
        },
    };
    match send(&Request::SetLayout { monitor, layout }) {
        Some(Response::Ok) => ExitCode::SUCCESS,
        Some(Response::Error { message }) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
        _ => ExitCode::FAILURE,
    }
}

fn query(what: QueryKind) -> ExitCode {
    let state = match send(&Request::QueryState) {
        Some(Response::State(state)) => state,
        Some(Response::Error { message }) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
        Some(Response::Ok | Response::Scenes { .. }) | None => return ExitCode::FAILURE,
    };
    match what {
        QueryKind::State => println!(
            "{}",
            serde_json::to_string_pretty(&state).expect("state is serializable")
        ),
        QueryKind::Windows => print_windows(&state),
        QueryKind::Monitors => print_monitors(&state),
    }
    ExitCode::SUCCESS
}

fn print_windows(state: &StateSnapshot) {
    println!(
        "{:<10} {:<22} {:<24} {:>6} {:>6} {:>6} {:>6} {:>4}  {:<9} title",
        "hwnd", "exe", "class", "x", "y", "w", "h", "zone", "flags"
    );
    for w in &state.windows {
        let mut flags = String::new();
        if state.foreground == Some(w.hwnd) {
            flags.push('F');
        }
        if w.tiled {
            flags.push('T');
        }
        if w.on_other_desktop {
            flags.push('D');
        }
        if w.rule_float {
            flags.push('~');
        }
        if w.scratchpad.is_some() {
            flags.push('S');
        }
        if !w.resizable {
            flags.push('#');
        }
        match w.show {
            tessera_core::ipc::ShowState::Minimized => flags.push('m'),
            tessera_core::ipc::ShowState::Maximized => flags.push('M'),
            tessera_core::ipc::ShowState::Normal => {}
        }
        println!(
            "{:<10} {:<22} {:<24} {:>6} {:>6} {:>6} {:>6} {:>4}  {:<9} {}",
            format!("{:#x}", w.hwnd),
            truncate(&w.exe, 22),
            truncate(&w.class, 24),
            w.rect.x,
            w.rect.y,
            w.rect.width,
            w.rect.height,
            w.zone.map_or_else(String::new, |z| z.to_string()),
            flags,
            truncate(&w.title, 60),
        );
    }
    println!(
        "\n{} windows. Flags: F=foreground T=tiled D=other desktop ~=rule float S=scratchpad #=fixed size m=minimized M=maximized",
        state.windows.len()
    );
}

fn print_monitors(state: &StateSnapshot) {
    for m in &state.monitors {
        println!(
            "{}{} ({})  {}x{} @ {},{}  work area {}x{} @ {},{}  {} dpi{}\n   Layout: {} ({} zones{})\n   {}",
            if m.primary { "* " } else { "  " },
            m.name,
            m.device,
            m.rect.width,
            m.rect.height,
            m.rect.x,
            m.rect.y,
            m.work_area.width,
            m.work_area.height,
            m.work_area.x,
            m.work_area.y,
            m.dpi,
            if m.paused { "  (paused)" } else { "" },
            m.layout,
            m.zones.len(),
            zone_modes(&m.zone_modes),
            m.id,
        );
    }
}

/// `: stack, auto` – empty as long as the monitor has no zones yet.
fn zone_modes(modes: &[ZoneMode]) -> String {
    if modes.is_empty() {
        return String::new();
    }
    let names: Vec<&str> = modes
        .iter()
        .map(|m| match m {
            ZoneMode::Auto => "auto",
            ZoneMode::Stack => "stack",
            ZoneMode::Columns => "columns",
            ZoneMode::Rows => "rows",
        })
        .collect();
    format!(": {}", names.join(", "))
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        let mut short: String = text.chars().take(max - 1).collect();
        short.push('…');
        short
    }
}

fn check_config(path: Option<PathBuf>) -> ExitCode {
    let path = path.unwrap_or_else(paths::config_file);
    let result = if path.exists() {
        paths::load_config(&path)
    } else {
        println!(
            "{} does not exist yet – checking the built-in default configuration.",
            path.display()
        );
        LoadedConfig::parse(DEFAULT_CONFIG).map_err(Into::into)
    };

    match result {
        Ok(loaded) => {
            println!("{}: OK", path.display());
            println!("{} layouts:", loaded.layouts.len());
            for layout in loaded.layouts.iter() {
                println!("  {:<28} {} zones", layout.name, layout.zone_count());
            }
            println!(
                "{} rules, {} shortcuts:",
                loaded.rules.len(),
                loaded.bindings.len()
            );
            for (combo, action) in &loaded.bindings {
                println!("  {:<28} {action}", combo.to_string());
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{}: {e}", path.display());
            ExitCode::FAILURE
        }
    }
}

fn report(result: Result<(), String>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn vdesk_probe() -> ExitCode {
    let build = tessera_win::vdesk::os_build().unwrap_or_else(|| "unknown".into());
    println!("Windows build: {build} (winvd requires at least 26100.2605)");
    match tessera_win::vdesk::probe() {
        Ok(result) => {
            println!("{} virtual desktops:", result.desktops.len());
            for d in &result.desktops {
                let marker = if d.index == result.current { "*" } else { " " };
                let name = if d.name.is_empty() {
                    "(unnamed)"
                } else {
                    &d.name
                };
                println!(" {marker} {}  {name:<20} {}", d.index + 1, d.id);
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("Desktop API not usable: {e}");
            ExitCode::FAILURE
        }
    }
}
