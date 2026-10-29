//! The daemon: startup, message loop and dispatching of events to the
//! window manager. Runs on the main thread.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use anyhow::Context;
use tessera_core::action::Action;
use tessera_core::config::LoadedConfig;
use tessera_core::ipc::{Request, Response, SceneSummary};
use tessera_core::keys::Modifiers;
use tessera_core::scene::Scenes;
use tessera_core::state::SavedState;
use tessera_win::events::{self, Event, EventHooks};
use tessera_win::explorer;
use tessera_win::hotkeys::{HookEvent, Hotkeys};
use tessera_win::msgloop::{self, MessageWindow, Timers, Waker};
use tessera_win::shell::{self, MessageKind};
use tessera_win::vdesk::{self, DesktopListener, DesktopManager, DesktopNotice};
use tessera_win::window;
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
/// Timer: apply the automatic scene once the monitors have settled after
/// connecting or disconnecting one (docking can report several changes).
const SCENE_TIMER: usize = 103;
const SCENE_DELAY_MS: u32 = 2_000;
/// First lines of `scenes.toml`.
const SCENES_HEADER: &str = "\
# Tessera - saved scenes
#
# Written by Tessera when a scene is saved (tray menu or
# `tesserac scene save <name>`); comments in this file are not kept.
# Per scene you can change:
#   auto   = true   apply it whenever exactly its monitors are connected
#   launch = true   when applying, start programs that have no window yet
#   zone            zone (1-based) of a window on its monitor
";

/// Events from other threads or callbacks for the main thread.
enum AppEvent {
    Menu(MenuId),
    Hotkey(Action),
    /// Key pressed while window hints capture the keyboard.
    HintKey(u16, Modifiers),
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
    /// The keyboard hook captures all keys for the window hints.
    capturing: bool,
    tray: tray::Tray,
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
    let hotkeys = Hotkeys::start(move |event| {
        let _ = hotkey_tx.send(match event {
            HookEvent::Action(action) => AppEvent::Hotkey(action),
            HookEvent::Key { vk, mods } => AppEvent::HintKey(vk, mods),
        });
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

    let tray = tray::create(tessera_win::autostart::is_enabled())?;
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
        capturing: false,
        tray,
    };
    daemon.apply_bindings();
    let state = daemon.load_state();
    daemon.wm.restore_state(state);
    daemon.wm.adopt_existing(window::enumerate());
    daemon.refresh_scene_menu();

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
                    if action == Action::Hint {
                        // The hook started capturing on its own; end it right
                        // away if no hints are shown.
                        self.capturing = self.wm.hints_active();
                        self.hotkeys.set_capture(self.capturing);
                    }
                }
                AppEvent::HintKey(vk, mods) => {
                    self.wm.on_hint_key(vk, mods);
                    // Still typing: restart the hook's own time limit too.
                    if self.wm.hints_active() {
                        self.hotkeys.set_capture(true);
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
                Event::DisplayChanged | Event::WorkAreaChanged => self.on_monitors_changed(),
                Event::Timer(RELOAD_TIMER) => self.on_config_file_changed(),
                Event::Timer(SAVE_TIMER) => self.save_state(),
                Event::Timer(HOOK_TIMER) => self.hotkeys.reinstall(),
                Event::Timer(SCENE_TIMER) => self.apply_auto_scene(),
                Event::Resumed => {
                    tracing::info!("Resumed from standby");
                    self.hotkeys.reinstall();
                    self.on_monitors_changed();
                }
                Event::Timer(id) => self.wm.on_timer(id),
                Event::TabClicked { bar, x, middle } => self.wm.on_tab_clicked(bar, x, middle),
            }
        }
        // Hints end on their own (timeout, a window was chosen): release the
        // keyboard then.
        let capture = self.wm.hints_active();
        if capture != self.capturing {
            self.capturing = capture;
            self.hotkeys.set_capture(capture);
        }
    }

    fn on_monitors_changed(&mut self) {
        if self.wm.refresh_monitors() {
            self.timers.set(SCENE_TIMER, SCENE_DELAY_MS);
        }
    }

    /// Applies the automatic scene for the connected monitors, if there is
    /// one.
    fn apply_auto_scene(&mut self) {
        self.timers.kill(SCENE_TIMER);
        let scenes = match self.load_scenes() {
            Ok(scenes) => scenes,
            Err(e) => {
                tracing::warn!("{e:#}");
                return;
            }
        };
        let Some(scene) = scenes.auto_for(&self.wm.monitor_ids()) else {
            return;
        };
        tracing::info!(scene = scene.name, "Monitor setup has an automatic scene");
        if let Err(message) = self.wm.apply_scene(scene) {
            tracing::warn!("{message}");
        }
    }

    fn load_scenes(&self) -> anyhow::Result<Scenes> {
        let path = paths::scenes_file_for(&self.config_path);
        match std::fs::read_to_string(&path) {
            Ok(text) => Scenes::parse(&text).with_context(|| path.display().to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Scenes::default()),
            Err(e) => Err(e).with_context(|| path.display().to_string()),
        }
    }

    fn write_scenes(&mut self, scenes: &Scenes) -> anyhow::Result<()> {
        let path = paths::scenes_file_for(&self.config_path);
        let text = format!("{SCENES_HEADER}\n{}", scenes.to_toml());
        std::fs::write(&path, text).with_context(|| path.display().to_string())?;
        self.refresh_scene_menu();
        Ok(())
    }

    fn refresh_scene_menu(&self) {
        let entries = match self.load_scenes() {
            Ok(scenes) => scenes
                .scenes
                .iter()
                .map(|s| (s.name.clone(), s.auto))
                .collect(),
            Err(e) => {
                tracing::warn!("{e:#}");
                Vec::new()
            }
        };
        self.tray.set_scenes(&entries);
    }

    fn save_scene(&mut self, name: &str, auto: bool, launch: bool) -> Result<String, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("a scene needs a name".into());
        }
        let mut scenes = self.load_scenes().map_err(|e| format!("{e:#}"))?;
        let scene = self.wm.capture_scene(name, auto, launch);
        let summary = format!(
            "scene `{name}` saved: {} monitor(s), {} window(s)",
            scene.monitors.len(),
            scene.window_count()
        );
        scenes.upsert(scene);
        self.write_scenes(&scenes).map_err(|e| format!("{e:#}"))?;
        tracing::info!(scene = name, auto, launch, "Scene saved");
        Ok(summary)
    }

    fn apply_scene(&mut self, name: &str) -> Result<String, String> {
        let scenes = self.load_scenes().map_err(|e| format!("{e:#}"))?;
        let scene = scenes
            .get(name)
            .ok_or_else(|| format!("there is no scene `{name}`"))?;
        self.wm.apply_scene(scene)
    }

    fn delete_scene(&mut self, name: &str) -> Result<String, String> {
        let mut scenes = self.load_scenes().map_err(|e| format!("{e:#}"))?;
        if !scenes.remove(name) {
            return Err(format!("there is no scene `{name}`"));
        }
        self.write_scenes(&scenes).map_err(|e| format!("{e:#}"))?;
        Ok(format!("scene `{name}` deleted"))
    }

    fn scene_summaries(&self) -> Result<Vec<SceneSummary>, String> {
        let scenes = self.load_scenes().map_err(|e| format!("{e:#}"))?;
        let mut current = self.wm.monitor_ids();
        current.sort_unstable();
        current.dedup();
        Ok(scenes
            .scenes
            .iter()
            .map(|s| SceneSummary {
                name: s.name.clone(),
                auto: s.auto,
                launch: s.launch,
                monitors: s
                    .monitors
                    .iter()
                    .map(|m| {
                        self.wm
                            .monitor_name(&m.id)
                            .map_or_else(|| m.name.clone(), ToOwned::to_owned)
                    })
                    .collect(),
                windows: s.window_count(),
                current_monitors: s.monitor_ids() == current,
            })
            .collect())
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
            tray::SAVE_SCENE => {
                // Named after the monitors, applied automatically with them.
                // Without a display name, a monitor is called `\\.\DISPLAY1`.
                let names: Vec<&str> = self
                    .wm
                    .monitor_names()
                    .into_iter()
                    .map(|n| n.trim_start_matches(r"\\.\"))
                    .collect();
                let name = names.join(" + ");
                match self.save_scene(&name, true, false) {
                    Ok(summary) => tracing::info!("{summary}"),
                    Err(message) => shell::message_box_async(
                        TITLE,
                        &format!("The scene could not be saved:\n\n{message}"),
                        MessageKind::Warning,
                    ),
                }
            }
            tray::QUIT => msgloop::quit(0),
            other if other.starts_with(tray::SCENE_PREFIX) => {
                let name = &other[tray::SCENE_PREFIX.len()..];
                if let Err(message) = self.apply_scene(name) {
                    tracing::warn!("{message}");
                    shell::message_box_async(TITLE, &message, MessageKind::Warning);
                }
            }
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
            Request::SaveScene { name, auto, launch } => {
                self.save_scene(&name, auto, launch).map(drop)
            }
            Request::ApplyScene { name } => self.apply_scene(&name).map(drop),
            Request::DeleteScene { name } => self.delete_scene(&name).map(drop),
            Request::ListScenes => {
                return match self.scene_summaries() {
                    Ok(scenes) => Response::Scenes { scenes },
                    Err(message) => Response::Error { message },
                };
            }
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
        self.tray
            .autostart
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
