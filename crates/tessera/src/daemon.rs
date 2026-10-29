//! The daemon: startup, message loop and dispatching of events to the
//! window manager. Runs on the main thread.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use tessera_core::action::Action;
use tessera_core::config::LoadedConfig;
use tessera_core::ipc::{Request, Response};
use tessera_core::state::SavedState;
use tessera_win::events::{self, Event, EventHooks, WindowEvent};
use tessera_win::explorer;
use tessera_win::hotkeys::Hotkeys;
use tessera_win::msgloop::{self, MessageWindow, Timers, Waker};
use tessera_win::shell::{self, MessageKind};
use tessera_win::vdesk::{self, DesktopListener, DesktopManager, DesktopNotice};
use tessera_win::window;
use tray_icon::TrayIcon;
use tray_icon::menu::CheckMenuItem;
use tray_icon::menu::{MenuEvent, MenuId};

use crate::paths::{self, load_config};
use crate::tray;
use crate::watch::{self, ConfigWatcher};
use crate::wm::Wm;

const INSTANCE_NAME: &str = "Tessera.Daemon";
const TITLE: &str = "Tessera";
/// How long an IPC request waits for the main thread.
const IPC_TIMEOUT: Duration = Duration::from_secs(5);
/// Timer: reload the configuration once no change has come in for a while.
const RELOAD_TIMER: usize = 100;
const RELOAD_DELAY_MS: u32 = 300;
/// Timer: save state periodically (if something has changed).
const SAVE_TIMER: usize = 101;
const SAVE_INTERVAL_MS: u32 = 30_000;
/// Timer: reinstall the keyboard hook as a precaution.
const HOOK_TIMER: usize = 102;
const HOOK_INTERVAL_MS: u32 = 5 * 60_000;

/// Events from other threads or callbacks for the main thread.
enum AppEvent {
    Menu(MenuId),
    Hotkey(Action),
    Ipc(Request, Sender<Response>),
    Desktop(DesktopNotice),
    /// `config.toml` or `layouts.toml` was saved.
    ConfigFileChanged,
}

struct Daemon {
    config_path: PathBuf,
    wm: Wm,
    events: Receiver<AppEvent>,
    hotkeys: Hotkeys,
    timers: Timers,
    /// Last error reported by the automatic reload (don't report it twice).
    last_reload_error: Option<String>,
    _watcher: Option<ConfigWatcher>,
    _hooks: EventHooks,
    _desktop_listener: Option<DesktopListener>,
    /// Last written content of `state.json`.
    saved_state: String,
    autostart: CheckMenuItem,
    _tray: TrayIcon,
}

pub fn run(config_override: Option<PathBuf>) -> anyhow::Result<()> {
    let Some(_instance) = tessera_win::instance::SingleInstance::acquire(INSTANCE_NAME)? else {
        tracing::warn!("Tessera is already running");
        shell::message_box(TITLE, "Tessera is already running.", MessageKind::Info);
        return Ok(());
    };

    // Switch an autostart entry under the former name over to this exe.
    if let Ok(exe) = std::env::current_exe() {
        match tessera_win::autostart::migrate_legacy(&exe) {
            Ok(true) => tracing::info!("Autostart switched from TilingMgr to Tessera"),
            Ok(false) => {}
            Err(e) => tracing::warn!("Could not switch old autostart entry: {e:#}"),
        }
    }

    let config_path = match config_override {
        Some(path) => path,
        None => paths::ensure_config()?,
    };
    let config = load_config(&config_path).unwrap_or_else(|e| {
        report_error(
            &config_path,
            &e,
            "The built-in default configuration is used.",
        );
        LoadedConfig::builtin()
    });

    let desktops = vdesk::init_com()
        .and_then(|()| DesktopManager::new())
        .inspect_err(|e| tracing::warn!("Desktop assignment not available: {e:#}"))
        .ok();
    // The undocumented API can break after Windows updates; tiling and zones
    // still work then, only desktop actions don't.
    let desktop_control = match vdesk::probe() {
        Ok(probe) => {
            tracing::info!(
                desktops = probe.desktops.len(),
                "Virtual desktops controllable"
            );
            true
        }
        Err(e) => {
            tracing::warn!(
                build = vdesk::os_build().unwrap_or_default(),
                "Virtual desktops not controllable: {e}"
            );
            false
        }
    };

    let window = MessageWindow::new()?;
    let waker = window.waker();
    let (tx, app_events) = mpsc::channel();

    let menu_tx = tx.clone();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        let _ = menu_tx.send(AppEvent::Menu(event.id));
        waker.wake();
    }));
    let hotkey_tx = tx.clone();
    let hotkeys = Hotkeys::start(move |action| {
        let _ = hotkey_tx.send(AppEvent::Hotkey(action));
        waker.wake();
    })?;
    let desktop_tx = tx.clone();
    let desktop_listener = desktop_control
        .then(|| {
            vdesk::listen(move |notice| {
                let _ = desktop_tx.send(AppEvent::Desktop(notice));
                waker.wake();
            })
            .inspect_err(|e| tracing::warn!("Desktop events not available: {e}"))
            .ok()
        })
        .flatten();
    let watch_tx = tx.clone();
    let watcher = watch::watch(&config_path, move || {
        let _ = watch_tx.send(AppEvent::ConfigFileChanged);
        waker.wake();
    })
    .inspect_err(|e| tracing::warn!("Automatic reload not available: {e:#}"))
    .ok();
    if let Err(e) = tessera_win::pipe::serve(move |line| handle_ipc_line(line, &tx, waker)) {
        tracing::error!("IPC not available, tesserac cannot reach the daemon: {e:#}");
    }

    let (tray, autostart) = tray::create(tessera_win::autostart::is_enabled())?;
    let desktop_notices = desktop_listener.is_some();
    let mut daemon = Daemon {
        config_path,
        wm: Wm::new(
            config,
            desktops,
            desktop_control,
            desktop_notices,
            window.timers(),
        ),
        events: app_events,
        hotkeys,
        timers: window.timers(),
        last_reload_error: None,
        _watcher: watcher,
        _hooks: EventHooks::install(waker)?,
        _desktop_listener: desktop_listener,
        saved_state: String::new(),
        autostart,
        _tray: tray,
    };
    daemon.apply_bindings();
    let state = daemon.load_state();
    daemon.wm.restore_state(state);
    // Insert existing windows from back to front so that the topmost one
    // comes last. Arranging only happens once all of them are inserted.
    daemon.wm.with_batched_apply(|wm| {
        for existing in window::enumerate().into_iter().rev() {
            wm.on_window_event(WindowEvent::Shown, existing);
        }
    });

    tracing::info!(
        config = %daemon.config_path.display(),
        bindings = daemon.wm.config.bindings.len(),
        rules = daemon.wm.config.rules.len(),
        windows = daemon.wm.window_count(),
        "Tessera started"
    );

    daemon.timers.set(SAVE_TIMER, SAVE_INTERVAL_MS);
    daemon.timers.set(HOOK_TIMER, HOOK_INTERVAL_MS);
    msgloop::run(|| daemon.process_events());

    daemon.save_state();
    daemon.wm.shutdown();
    MenuEvent::set_event_handler(None::<fn(MenuEvent)>);
    drop(daemon);
    drop(window);
    tracing::info!("Tessera stopped");
    Ok(())
}

/// Runs on the IPC thread: pass the request to the main thread and wait for
/// the response.
fn handle_ipc_line(line: &str, tx: &Sender<AppEvent>, waker: Waker) -> String {
    let response = match serde_json::from_str::<Request>(line) {
        Err(e) => Response::Error {
            message: format!("invalid request: {e}"),
        },
        Ok(request) => {
            let (reply_tx, reply_rx) = mpsc::channel();
            if tx.send(AppEvent::Ipc(request, reply_tx)).is_err() {
                Response::Error {
                    message: "daemon is shutting down".into(),
                }
            } else {
                waker.wake();
                reply_rx
                    .recv_timeout(IPC_TIMEOUT)
                    .unwrap_or_else(|_| Response::Error {
                        message: "daemon not responding (open dialog?)".into(),
                    })
            }
        }
    };
    serde_json::to_string(&response).expect("response is serializable")
}

impl Daemon {
    fn process_events(&mut self) {
        while let Ok(event) = self.events.try_recv() {
            match event {
                AppEvent::Menu(id) => self.on_menu(id.as_ref()),
                AppEvent::Hotkey(action) => {
                    if let Err(message) = self.run_action(action) {
                        tracing::info!("{action}: {message}");
                    }
                }
                AppEvent::Ipc(request, reply) => {
                    let _ = reply.send(self.on_ipc(request));
                }
                AppEvent::Desktop(notice) => self.wm.on_desktop_notice(notice),
                // Debounce: editors often write several times in quick succession.
                AppEvent::ConfigFileChanged => self.timers.set(RELOAD_TIMER, RELOAD_DELAY_MS),
            }
        }
        for event in events::drain() {
            match event {
                Event::Window(kind, window) => self.wm.on_window_event(kind, window),
                Event::DisplayChanged | Event::WorkAreaChanged => self.wm.refresh_monitors(),
                Event::Timer(RELOAD_TIMER) => self.on_config_file_changed(),
                Event::Timer(SAVE_TIMER) => self.save_state(),
                Event::Timer(HOOK_TIMER) => self.hotkeys.reinstall(),
                Event::Resumed => {
                    tracing::info!("Resumed from standby");
                    self.hotkeys.reinstall();
                    self.wm.refresh_monitors();
                }
                Event::Timer(id) => self.wm.on_timer(id),
                Event::TabClicked { bar, x } => self.wm.on_tab_clicked(bar, x),
            }
        }
    }

    fn on_menu(&mut self, id: &str) {
        match id {
            tray::OPEN_CONFIG => open(&self.config_path),
            tray::RELOAD_CONFIG => {
                if let Err(e) = self.reload_config() {
                    report_error(
                        &self.config_path,
                        &e,
                        "The previous configuration remains active.",
                    );
                }
            }
            tray::OPEN_EDITOR => self.open_editor(),
            tray::OPEN_LOGS => open(&paths::log_dir()),
            tray::AUTOSTART => self.toggle_autostart(),
            tray::QUIT => msgloop::quit(0),
            other => tracing::warn!("unknown menu item `{other}`"),
        }
    }

    fn on_ipc(&mut self, request: Request) -> Response {
        let result = match request {
            Request::QueryState => return Response::State(self.wm.snapshot()),
            Request::SetLayout { monitor, layout } => self.wm.set_layout_on(&monitor, &layout),
            Request::Action { action } => action
                .parse::<Action>()
                .map_err(|e| e.to_string())
                .and_then(|action| self.run_action(action)),
        };
        match result {
            Ok(()) => Response::Ok,
            Err(message) => Response::Error { message },
        }
    }

    fn run_action(&mut self, action: Action) -> Result<(), String> {
        match action {
            Action::Quit => {
                msgloop::quit(0);
                Ok(())
            }
            Action::ReloadConfig => self.reload_config().map_err(|e| format!("{e:#}")),
            Action::OpenEditor => {
                self.open_editor();
                Ok(())
            }
            Action::OpenExplorer => {
                explorer::open_tab_or_window().map_err(|e| format!("File Explorer: {e:#}"))
            }
            other => self.wm.run_action(other),
        }
    }

    fn reload_config(&mut self) -> anyhow::Result<()> {
        self.wm.config = load_config(&self.config_path)?;
        self.apply_bindings();
        self.wm.config_changed();
        tracing::info!(
            bindings = self.wm.config.bindings.len(),
            rules = self.wm.config.rules.len(),
            "Configuration reloaded"
        );
        Ok(())
    }

    /// Automatic reload after saving. Errors are reported without a blocking
    /// dialog so that tiling keeps running.
    fn on_config_file_changed(&mut self) {
        self.timers.kill(RELOAD_TIMER);
        match self.reload_config() {
            Ok(()) => self.last_reload_error = None,
            Err(e) => {
                let message = format!("{e:#}");
                tracing::error!("Configuration faulty: {message}");
                if self.last_reload_error.as_ref() != Some(&message) {
                    shell::message_box_async(
                        TITLE,
                        &format!(
                            "{message}

The previous configuration remains active."
                        ),
                        MessageKind::Warning,
                    );
                    self.last_reload_error = Some(message);
                }
            }
        }
    }

    fn load_state(&mut self) -> SavedState {
        let path = paths::state_file_for(&self.config_path);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return SavedState::default();
        };
        self.saved_state = text.clone();
        serde_json::from_str(&text).unwrap_or_else(|e| {
            tracing::warn!("{} unreadable, will be recreated: {e}", path.display());
            SavedState::default()
        })
    }

    /// Writes `state.json` if something has changed since the last time.
    fn save_state(&mut self) {
        let text =
            serde_json::to_string_pretty(&self.wm.saved_state()).expect("state is serializable");
        if text == self.saved_state {
            return;
        }
        let path = paths::state_file_for(&self.config_path);
        match std::fs::write(&path, &text) {
            Ok(()) => self.saved_state = text,
            Err(e) => tracing::warn!("{} not writable: {e}", path.display()),
        }
    }

    fn toggle_autostart(&mut self) {
        let enabled = !tessera_win::autostart::is_enabled();
        let result = std::env::current_exe()
            .map_err(anyhow::Error::from)
            .and_then(|exe| tessera_win::autostart::set(enabled, &exe));
        match result {
            Ok(()) => tracing::info!(enabled, "Autostart changed"),
            Err(e) => tracing::error!("Autostart: {e:#}"),
        }
        self.autostart
            .set_checked(tessera_win::autostart::is_enabled());
    }

    /// Starts the zone editor as a separate process for the current monitor.
    fn open_editor(&mut self) {
        let exe = match std::env::current_exe() {
            Ok(exe) => exe,
            Err(e) => {
                tracing::error!("Program path unknown: {e}");
                return;
            }
        };
        let mut command = std::process::Command::new(exe);
        command.arg("--config").arg(&self.config_path).arg("editor");
        if let Some((monitor, layout)) = self.wm.editor_target() {
            command.args(["--monitor", &monitor, "--layout", &layout]);
        }
        if let Err(e) = command.spawn() {
            tracing::error!("Zone editor could not be started: {e}");
        }
    }

    fn apply_bindings(&mut self) {
        for error in self.hotkeys.set_bindings(&self.wm.config.bindings) {
            tracing::warn!("Shortcut {error}");
        }
    }
}

fn report_error(path: &Path, error: &anyhow::Error, consequence: &str) {
    tracing::error!("Configuration {} faulty: {error:#}", path.display());
    shell::message_box(
        TITLE,
        &format!("Error in {}:\n\n{error:#}\n\n{consequence}", path.display()),
        MessageKind::Warning,
    );
}

fn open(path: &Path) {
    if let Err(e) = shell::open_path(path) {
        tracing::error!("Could not open {}: {e:#}", path.display());
    }
}
