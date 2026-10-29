//! The window manager: managed windows, workspaces per monitor and
//! desktop, zones, actions. Runs exclusively on the main thread.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use tessera_core::action::{Action, Cycle, Direction, Resize};
use tessera_core::config::LoadedConfig;
use tessera_core::geometry::{Rect, scale_px};
use tessera_core::hint::{self, HintMatch};
use tessera_core::ipc::{MonitorState, ShowState, StateSnapshot, WindowState};
use tessera_core::keys::Modifiers;
use tessera_core::nav;
use tessera_core::rules::{RuleEffect, WindowProps};
use tessera_core::scene::{Scene as SavedScene, SceneMonitor, SceneWindow};
use tessera_core::scratchpad::expand_env;
use tessera_core::state::{SavedState, SavedWindow, SavedWorkspace};
use tessera_core::tree::{Node, Orientation, Placement, WindowId, ZoneMode};
use tessera_core::workspace::Zone;
use tessera_core::workspace::{DropSide, DropTarget, Workspace};
use tessera_win::events::WindowEvent;
use tessera_win::hints::{Hint, HintOverlay, HintStyle};
use tessera_win::monitor::{self, Monitor};
use tessera_win::msgloop::Timers;
use tessera_win::overlay::{Scene, Style, ZoneOverlay};
use tessera_win::position;
use tessera_win::tabbar::{TabBar, TabHit, Tabs};
use tessera_win::vdesk::{self, DesktopManager, DesktopNotice};
use tessera_win::window::{Cloak, Window};

/// Step size for `resize grow/shrink`.
const RESIZE_STEP: f32 = 0.05;

/// Timer: poll the mouse position while dragging.
const DRAG_TIMER: usize = 1;
const DRAG_INTERVAL_MS: u32 = 15;
/// Timer: hide the zone numbers again after a layout change.
const FLASH_TIMER: usize = 2;
const FLASH_MS: u32 = 700;
/// Timer: reset tiled windows that have moved themselves.
const DRIFT_TIMER: usize = 3;
const DRIFT_DELAY_MS: u32 = 200;
/// Maximum number of times a window is reset to the same target rect.
/// If an application moves itself again after every correction, there
/// would otherwise be an endless back and forth.
const MAX_DRIFT_CORRECTIONS: u8 = 3;
/// Timer: window hints disappear if nothing is typed for this long.
const HINT_TIMER: usize = 4;
const HINT_MS: u32 = 8_000;
/// A window that appears this long after its scratchpad's program was
/// started is shown right away.
const SCRATCHPAD_LAUNCH_WAIT: Duration = Duration::from_secs(30);
/// The program of a scratchpad is not started again within this time.
const SCRATCHPAD_START_DEBOUNCE: Duration = Duration::from_secs(10);
/// How long windows started by a scene are waited for.
const SCENE_LAUNCH_WAIT: Duration = Duration::from_secs(60);
/// At most this many programs are started per scene.
const SCENE_MAX_LAUNCHES: usize = 20;
const VK_BACK: u16 = 0x08;

/// Workspace = monitor ID × desktop GUID.
type WsKey = (String, String);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Tiled,
    Floating,
    /// Belongs to a scratchpad: floats, minimized while hidden.
    Scratchpad,
}

/// Window hints being shown: the windows, their labels and what has been
/// typed so far.
struct HintSession {
    targets: Vec<Window>,
    labels: Vec<String>,
    hints: Vec<Hint>,
    typed: String,
}

/// State of a scratchpad from `config.toml`, by its position there.
#[derive(Default)]
struct ScratchSlot {
    name: String,
    window: Option<Window>,
    /// When its program was started because it had no window.
    launched: Option<Instant>,
    /// Monitor it is summoned to (the one in use when it was summoned).
    monitor: Option<String>,
}

/// A window started by a scene: lands in its zone when it appears.
struct PendingWindow {
    monitor: String,
    /// 0-based.
    zone: usize,
    exe: String,
    class: String,
    title: String,
    until: Instant,
}

/// State of a window before Tessera tiled it.
#[derive(Debug, Clone, Copy)]
struct Original {
    rect: Rect,
    maximized: bool,
}

/// Corrections of a tiled window that was off its tile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Drift {
    /// Target rect it was last reset to.
    target: Rect,
    /// Corrections to `target` so far.
    corrections: u8,
    /// The window was on `target` after the last correction.
    reached: bool,
}

impl Drift {
    /// May a window with the history `drift` be reset (again) to `target`?
    /// Always for a new target. For the same target only if the last
    /// correction arrived (otherwise e.g. a minimum size keeps the window
    /// from fitting there) and the limit [`MAX_DRIFT_CORRECTIONS`] has not
    /// been reached yet.
    fn allows(drift: Option<Drift>, target: Rect) -> bool {
        match drift {
            Some(d) if d.target == target => d.reached && d.corrections < MAX_DRIFT_CORRECTIONS,
            _ => true,
        }
    }

    /// History after another correction to `target`.
    fn corrected(drift: Option<Drift>, target: Rect) -> Drift {
        let corrections = match drift {
            Some(d) if d.target == target => d.corrections.saturating_add(1),
            _ => 1,
        };
        Drift {
            target,
            corrections,
            reached: false,
        }
    }

    /// The window is on `target` (again).
    fn arrived(drift: &mut Option<Drift>, target: Rect) {
        if let Some(d) = drift
            && d.target == target
        {
            d.reached = true;
        }
    }
}

/// Batch phase of [`Wm::with_batched_apply`].
#[derive(Default)]
struct Batch {
    /// Workspaces that are arranged at the end.
    keys: Vec<WsKey>,
    /// Windows that were maximized when inserted. Their maximization is only
    /// lifted at the end, once their tile is settled – otherwise they would
    /// first be moved to an intermediate position.
    unmaximize: Vec<Window>,
}

struct Tracked {
    exe: String,
    class: String,
    title: String,
    rule: Option<RuleEffect>,
    mode: Mode,
    /// Minimized windows are not in the tree.
    minimized: bool,
    /// Last seen maximized state (to detect restoring).
    maximized: bool,
    original: Option<Original>,
    /// Corrections after foreign moves (see [`Drift`]); cleared on
    /// insertion.
    drift: Option<Drift>,
}

/// A tab bar and the windows of the stack it currently sits above.
struct BarSlot {
    bar: TabBar,
    windows: Vec<WindowId>,
}

/// Ongoing mouse move or resize operation.
struct Drag {
    window: Window,
    /// Rect at the start; if the size changes, it is a resize.
    start: Rect,
    resizing: bool,
}

pub struct Wm {
    pub config: LoadedConfig,
    monitors: Vec<Monitor>,
    windows: HashMap<Window, Tracked>,
    workspaces: HashMap<WsKey, Workspace>,
    foreground: Option<Window>,
    desktops: Option<DesktopManager>,
    /// Desktops can be switched/created (the undocumented API works).
    desktop_control: bool,
    timers: Timers,
    overlay: Option<ZoneOverlay>,
    drag: Option<Drag>,
    /// Reused tab bars; only the first ones are shown.
    bars: Vec<BarSlot>,
    /// Window whose border is currently colored.
    border: Option<Window>,
    /// State from the last run, consumed when windows are inserted.
    saved: SavedState,
    /// GUID of the current desktop, tracked via the desktop events and
    /// queried again on every [`Wm::refresh_monitors`] (also after
    /// standby).
    desktop_cache: String,
    /// Desktop events are arriving (otherwise `desktop_cache` is not
    /// reliable). If the listener ends, the desktop is queried live again.
    desktop_notices: bool,
    /// Tiled windows that are off their tile and are measured again after
    /// [`DRIFT_DELAY_MS`].
    drifting: HashSet<Window>,
    /// During a batch phase: what gets arranged at the end (see
    /// [`Wm::with_batched_apply`]).
    batch: Option<Batch>,
    hints: Option<HintSession>,
    hint_overlay: HintOverlay,
    /// One entry per scratchpad in the configuration.
    scratch: Vec<ScratchSlot>,
    /// Windows that already existed at startup are being taken over.
    adopting: bool,
    /// Windows a scene started and that have not appeared yet.
    pending: Vec<PendingWindow>,
}

impl Wm {
    pub fn new(
        config: LoadedConfig,
        desktops: Option<DesktopManager>,
        desktop_control: bool,
        desktop_notices: bool,
        timers: Timers,
    ) -> Self {
        let overlay = ZoneOverlay::new()
            .inspect_err(|e| tracing::warn!("Zone overlay not available: {e:#}"))
            .ok();
        let mut wm = Self {
            config,
            monitors: Vec::new(),
            windows: HashMap::new(),
            workspaces: HashMap::new(),
            foreground: None,
            desktops,
            desktop_control,
            timers,
            overlay,
            drag: None,
            bars: Vec::new(),
            border: None,
            saved: SavedState::default(),
            desktop_cache: vdesk::current_desktop().unwrap_or_default(),
            desktop_notices,
            drifting: HashSet::new(),
            batch: None,
            hints: None,
            hint_overlay: HintOverlay::new(),
            scratch: Vec::new(),
            adopting: false,
            pending: Vec::new(),
        };
        wm.sync_scratchpads();
        wm.refresh_monitors();
        wm.foreground = position::foreground();
        wm.update_border(wm.foreground);
        wm
    }

    pub fn window_count(&self) -> usize {
        self.windows.len()
    }

    /// Takes over the windows that exist at startup (in z-order, topmost
    /// first). Scratchpad windows among them are hidden rather than shown.
    pub fn adopt_existing(&mut self, windows: Vec<Window>) {
        self.adopting = true;
        // Insert from back to front so that the topmost one comes last.
        // Arranging only happens once all of them are inserted.
        self.with_batched_apply(|wm| {
            for existing in windows.into_iter().rev() {
                wm.on_window_event(WindowEvent::Shown, existing);
            }
        });
        self.adopting = false;
    }

    // ----- Events -------------------------------------------------------------

    pub fn on_window_event(&mut self, kind: WindowEvent, window: Window) {
        match kind {
            WindowEvent::Shown => self.manage(window),
            WindowEvent::Uncloaked => {
                self.manage(window);
                self.follow_desktop(window);
                self.refresh_bars();
            }
            WindowEvent::Foreground => {
                self.manage(window);
                self.on_foreground(window);
            }
            WindowEvent::MinimizeStart => self.on_minimize(window, true),
            WindowEvent::MinimizeEnd => {
                self.manage(window);
                self.on_minimize(window, false);
            }
            WindowEvent::Hidden | WindowEvent::Destroyed => self.release(window, kind),
            // Keep windows hidden by a desktop switch, release everything else.
            WindowEvent::Cloaked if !self.is_on_other_desktop(window) => self.release(window, kind),
            WindowEvent::Cloaked => self.refresh_bars(),
            WindowEvent::TitleChanged => {
                if let Some(tracked) = self.windows.get_mut(&window) {
                    tracked.title = window.title();
                    if self.bars.iter().any(|s| s.windows.contains(&window.id())) {
                        self.refresh_bars();
                    }
                    self.claim_on_title_change(window);
                }
            }
            WindowEvent::MoveSizeStart => self.on_drag_start(window),
            WindowEvent::LocationChanged => self.on_location_changed(window),
            WindowEvent::MoveSizeEnd => self.on_drag_end(window),
        }
    }

    pub fn on_desktop_notice(&mut self, notice: DesktopNotice) {
        tracing::debug!(?notice, "Desktop event");
        if !matches!(notice, DesktopNotice::WindowMoved(_)) {
            self.desktop_cache = vdesk::current_desktop().unwrap_or_default();
        }
        match notice {
            DesktopNotice::Switched => {
                self.update_border(position::foreground());
                self.refresh_bars();
            }
            DesktopNotice::Created => {}
            DesktopNotice::Removed { removed, fallback } => {
                // Dissolve the workspaces of the removed desktop; the windows
                // are now on `fallback` and get inserted there again.
                let orphans: Vec<WsKey> = self
                    .workspaces
                    .keys()
                    .filter(|(_, desktop)| *desktop == removed)
                    .cloned()
                    .collect();
                self.with_batched_apply(|wm| {
                    for key in orphans {
                        let ws = wm.workspaces.remove(&key).expect("exists");
                        let target = (key.0.clone(), fallback.clone());
                        for id in ws.windows() {
                            wm.tile(Window::from_id(id), Some((target.clone(), None)));
                        }
                    }
                });
                self.refresh_bars();
            }
            DesktopNotice::WindowMoved(id) => {
                self.follow_desktop(Window::from_id(id));
                self.refresh_bars();
            }
            DesktopNotice::Stopped => {
                tracing::warn!("Desktop events no longer arrive, current desktop is queried live");
                self.desktop_notices = false;
                self.refresh_bars();
            }
        }
    }

    /// Take over the state from the last run (before the windows are inserted).
    pub fn restore_state(&mut self, state: SavedState) {
        self.saved = state;
    }

    /// Current state for saving. Workspaces that were not created in this
    /// run (e.g. disconnected monitor) are kept.
    pub fn saved_state(&self) -> SavedState {
        let mut workspaces: Vec<SavedWorkspace> = self
            .workspaces
            .iter()
            .map(|((monitor, desktop), ws)| SavedWorkspace {
                monitor: monitor.clone(),
                desktop: desktop.clone(),
                layout: ws.layout.clone(),
                paused: ws.paused,
                zones: ws.zones().iter().map(|z| z.mode).collect(),
                windows: ws
                    .windows()
                    .into_iter()
                    .filter_map(|id| {
                        let tracked = self.windows.get(&Window::from_id(id))?;
                        Some(SavedWindow {
                            exe: tracked.exe.clone(),
                            class: tracked.class.clone(),
                            title: tracked.title.clone(),
                            zone: ws.zone_of(id)?,
                        })
                    })
                    .collect(),
            })
            .collect();
        let untouched = self.saved.workspaces.iter().filter(|s| {
            !self
                .workspaces
                .contains_key(&(s.monitor.clone(), s.desktop.clone()))
        });
        workspaces.extend(untouched.cloned());
        workspaces.sort_by(|a, b| (&a.monitor, &a.desktop).cmp(&(&b.monitor, &b.desktop)));
        SavedState { workspaces }
    }

    /// Window has moved or changed its state.
    ///
    /// Switch between maximized and normal: after restoring, the window
    /// belongs back on its (possibly changed in the meantime) tile.
    ///
    /// Otherwise checks whether a tiled window is off its tile, for instance
    /// because the application restores its saved size after starting. Only
    /// this window is reset, and only once no deviation has been reported
    /// for [`DRIFT_DELAY_MS`] and the deviation persists when measured again:
    /// that way the intermediate states of our own asynchronous positioning
    /// don't count as foreign moves. This also applies to keyboard snaps
    /// (Win+arrow) – a tiled window belongs on its tile; to place it freely,
    /// make it floating.
    ///
    /// [`Drift`] prevents endless loops: if the window is still not on its
    /// tile after the reset (e.g. because of a minimum size), it is not
    /// corrected again for this target, and an application that moves itself
    /// again after every correction is reset at most
    /// [`MAX_DRIFT_CORRECTIONS`] times.
    fn on_location_changed(&mut self, window: Window) {
        let Some(tracked) = self.windows.get_mut(&window) else {
            return;
        };
        let maximized = window.is_maximized();
        if tracked.maximized != maximized {
            tracked.maximized = maximized;
            match self.workspace_of(window) {
                Some(key) if !maximized => self.apply(&key),
                _ => self.refresh_bars(),
            }
            return;
        }
        let Some(target) = self.drift_target_of(window) else {
            return;
        };
        let tracked = self.windows.get_mut(&window).expect("managed");
        if window.rect() == target {
            Drift::arrived(&mut tracked.drift, target);
        } else if Drift::allows(tracked.drift, target) {
            // Debounce: every further deviation restarts the timer.
            self.drifting.insert(window);
            self.timers.set(DRIFT_TIMER, DRIFT_DELAY_MS);
        }
    }

    /// Target rect of a tiled window, provided it should currently be there:
    /// not minimized or maximized, not being dragged with the mouse and in a
    /// workspace that is not paused.
    fn drift_target_of(&self, window: Window) -> Option<Rect> {
        let tracked = self.windows.get(&window)?;
        if tracked.mode != Mode::Tiled
            || tracked.minimized
            || window.is_minimized()
            || window.is_maximized()
            || self.drag.as_ref().is_some_and(|d| d.window == window)
        {
            return None;
        }
        let key = self.workspace_of(window)?;
        if self.workspaces[&key].paused {
            return None;
        }
        self.target_rect(&key, window.id())
    }

    /// After the debounce delay: reset windows that are still off their
    /// tile, as far as [`Drift::allows`] permits.
    fn correct_drift(&mut self) {
        self.timers.kill(DRIFT_TIMER);
        for window in std::mem::take(&mut self.drifting) {
            let Some(target) = self.drift_target_of(window) else {
                continue;
            };
            let tracked = self.windows.get_mut(&window).expect("managed");
            let rect = window.rect();
            if rect == target {
                Drift::arrived(&mut tracked.drift, target);
                continue;
            }
            if !Drift::allows(tracked.drift, target) {
                continue;
            }
            let drift = Drift::corrected(tracked.drift, target);
            tracked.drift = Some(drift);
            tracing::debug!(
                hwnd = window.id(),
                ?rect,
                ?target,
                corrections = drift.corrections,
                "Window is off its tile, resetting it"
            );
            if drift.corrections >= MAX_DRIFT_CORRECTIONS {
                tracing::info!(
                    hwnd = window.id(),
                    exe = tracked.exe,
                    "Window keeps moving itself, \
                     no longer resetting it for this tile"
                );
            }
            position::set_rect(window, target);
        }
    }

    /// Click on a tab: bring the window in the stack to the front and focus
    /// it. A click on its close button or with the middle button closes the
    /// window instead.
    pub fn on_tab_clicked(&mut self, bar: usize, x: i32, middle: bool) {
        let Some(slot) = self.bars.get(bar) else {
            return;
        };
        let (index, close) = match slot.bar.hit_at(x) {
            Some(TabHit::Tab(i)) => (i, middle),
            Some(TabHit::Close(i)) => (i, true),
            None => return,
        };
        let Some(&id) = slot.windows.get(index) else {
            return;
        };
        let window = Window::from_id(id);
        if close {
            tracing::debug!(hwnd = id, "Closing window from its tab");
            position::close(window);
            return;
        }
        if let Some(key) = self.workspace_of(window) {
            self.workspaces
                .get_mut(&key)
                .expect("exists")
                .with_tree(id, |tree, _, _| tree.activate(id));
            self.apply(&key);
        }
        position::focus(window);
    }

    pub fn on_timer(&mut self, id: usize) {
        match id {
            DRAG_TIMER => self.drag_tick(),
            DRIFT_TIMER => self.correct_drift(),
            HINT_TIMER => self.end_hints(),
            FLASH_TIMER => {
                self.timers.kill(FLASH_TIMER);
                if self.drag.is_none() {
                    self.hide_zones();
                }
            }
            _ => {}
        }
    }

    /// Re-read the monitors (connected, disconnected, taskbar moved, scaling
    /// changed) and rearrange everything. Returns whether a monitor was
    /// connected or disconnected (not at the first call).
    pub fn refresh_monitors(&mut self) -> bool {
        let monitors = monitor::enumerate();
        let mut connections_changed = false;
        if monitors != self.monitors {
            let ids = |list: &[Monitor]| {
                let mut ids: Vec<String> = list.iter().map(|m| m.id.clone()).collect();
                ids.sort_unstable();
                ids
            };
            connections_changed =
                !self.monitors.is_empty() && ids(&monitors) != ids(&self.monitors);
            for m in &monitors {
                tracing::info!(
                    id = m.id,
                    name = m.name,
                    rect = ?m.rect,
                    work_area = ?m.work_area,
                    dpi = m.dpi,
                    primary = m.primary,
                    "Monitor"
                );
            }
            self.monitors = monitors;
        }
        // Opportunity to sync the cached desktop: a desktop switch whose
        // event got lost (standby, Explorer restart) would otherwise go
        // unnoticed until the next switch.
        self.desktop_cache = vdesk::current_desktop().unwrap_or_default();
        self.relayout_workspaces();
        connections_changed
    }

    /// After reloading the configuration: apply rules, gaps and layouts.
    pub fn config_changed(&mut self) {
        self.end_hints();
        let ignored: Vec<Window> = self
            .windows
            .iter_mut()
            .filter_map(|(window, tracked)| {
                tracked.rule = self.config.rules.find(props(tracked));
                (tracked.mode != Mode::Scratchpad && tracked.rule.is_some_and(|r| r.ignore))
                    .then_some(*window)
            })
            .collect();
        self.with_batched_apply(|wm| {
            for window in ignored {
                wm.release(window, WindowEvent::Hidden);
            }
            wm.sync_scratchpads();
            // The border's color or on/off state may have changed.
            if let Some(old) = wm.border.take() {
                position::set_border_color(old, None);
            }
            wm.update_border(wm.focused());
            wm.relayout_workspaces();
        });
    }

    /// Runs `f` as a batch phase: within it, `apply` only records the affected
    /// workspaces, and at the end each one is arranged exactly once. Without
    /// this, e.g. inserting n windows at startup would reposition all previous
    /// ones every time (n² moves, visible shuffling).
    /// Nested calls belong to the outer batch phase.
    pub fn with_batched_apply<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        if self.batch.is_some() {
            return f(self);
        }
        self.batch = Some(Batch::default());
        let result = f(self);
        let batch = self.batch.take().unwrap_or_default();
        for key in &batch.keys {
            self.apply(key);
        }
        for window in batch.unmaximize {
            self.unmaximize_into_tile(window);
        }
        result
    }

    /// On exit: reset tiled windows to their original state.
    pub fn shutdown(&mut self) {
        self.end_hints();
        self.hide_zones();
        for slot in &mut self.bars {
            slot.bar.hide();
        }
        if let Some(old) = self.border.take() {
            position::set_border_color(old, None);
        }
        if !self.config.config.general.restore_on_exit {
            return;
        }
        for (window, tracked) in &self.windows {
            if tracked.mode == Mode::Tiled
                && !tracked.minimized
                && let Some(original) = tracked.original
            {
                restore(*window, self.on_screen(original));
            }
        }
    }

    // ----- Management ---------------------------------------------------------

    /// Takes a window under management if it is suitable, and tiles it if
    /// applicable.
    fn manage(&mut self, window: Window) {
        if self.windows.contains_key(&window) {
            return;
        }
        if let Err(reason) = window.manageable() {
            tracing::trace!(hwnd = window.id(), "not managed: {reason}");
            return;
        }
        if window.cloak() == Cloak::OtherDesktop && !self.is_on_other_desktop(window) {
            tracing::trace!(hwnd = window.id(), "not managed: hidden by the shell");
            return;
        }
        let mut tracked = Tracked {
            exe: window.exe(),
            class: window.class(),
            title: window.title(),
            rule: None,
            mode: Mode::Tiled,
            minimized: window.is_minimized(),
            maximized: window.is_maximized(),
            original: None,
            drift: None,
        };
        tracked.rule = self.config.rules.find(props(&tracked));
        if let Some(index) = self.config.scratchpads.find(props(&tracked))
            && self.scratchpad_window(index).is_none()
        {
            tracked.mode = Mode::Scratchpad;
            self.windows.insert(window, tracked);
            let show = !self.adopting;
            self.claim_scratchpad(index, window, show);
            return;
        }
        if tracked.rule.is_some_and(|r| r.ignore) {
            tracing::debug!(hwnd = window.id(), exe = tracked.exe, "ignored by rule");
            return;
        }
        let general = &self.config.config.general;
        let floats = tracked.rule.is_some_and(|r| r.float)
            || (general.float_fixed_size && !window.is_resizable())
            || !general.auto_tile;
        if !window.can_be_moved() {
            tracing::info!(
                hwnd = window.id(),
                exe = tracked.exe,
                "Window runs with administrator rights and cannot be moved \
                 (start Tessera as administrator for that)"
            );
            tracked.mode = Mode::Floating;
        }
        if floats {
            tracked.mode = Mode::Floating;
        }
        // Titles can contain private data (mail subjects, document names), so they
        // only appear in the log at debug level.
        tracing::info!(
            hwnd = window.id(),
            exe = tracked.exe,
            class = tracked.class,
            mode = ?tracked.mode,
            "window managed"
        );
        tracing::debug!(hwnd = window.id(), title = tracked.title, "window title");
        let tile = tracked.mode == Mode::Tiled && !tracked.minimized;
        self.windows.insert(window, tracked);
        if tile {
            self.tile(window, None);
        }
    }

    fn release(&mut self, window: Window, cause: WindowEvent) {
        if self.foreground == Some(window) {
            self.foreground = None;
        }
        if self.border == Some(window) {
            self.border = None;
            position::set_border_color(window, None);
        }
        self.drifting.remove(&window);
        for slot in &mut self.scratch {
            if slot.window == Some(window) {
                slot.window = None;
            }
        }
        let Some(tracked) = self.windows.remove(&window) else {
            return;
        };
        tracing::info!(
            hwnd = window.id(),
            exe = tracked.exe,
            ?cause,
            "Window released"
        );
        self.untile(window);
    }

    /// Inserts a window into its workspace (or into `target`).
    fn tile(&mut self, window: Window, target: Option<(WsKey, Option<WindowId>)>) {
        let Some(tracked) = self.windows.get_mut(&window) else {
            return;
        };
        tracked.drift = None;
        let first_time = tracked.original.is_none();
        if first_time {
            // Rect in the restored state: on `MinimizeEnd` the window is still
            // minimized and would otherwise be at -32000/-32000.
            tracked.original = Some(Original {
                rect: window.normal_rect(),
                maximized: window.is_maximized(),
            });
        }
        let rule_zone = tracked
            .rule
            .and_then(|r| r.zone)
            .map(|z| usize::from(z) - 1);
        // Started by a scene: into the scene's monitor and zone. Only for
        // windows tiled for the first time, not e.g. restored ones.
        let pending = match (&target, rule_zone) {
            (None, None) if first_time => self.take_pending(window),
            _ => None,
        };
        let (key, next_to) = match (target, &pending) {
            (Some(target), _) => target,
            (None, Some((monitor, _))) => ((monitor.clone(), self.key_for_window(window).1), None),
            (None, None) => (self.key_for_window(window), None),
        };
        // Without a rule: the zone from the last run, if the window was there.
        let zone = rule_zone.or(pending.map(|(_, zone)| zone)).or_else(|| {
            let tracked = &self.windows[&window];
            self.saved.take_zone(&key.0, &key.1, props(tracked))
        });
        let maximized = window.is_maximized();
        self.workspace_mut(&key).insert(window.id(), zone, next_to);
        self.apply(&key);
        // `apply` leaves maximized windows alone; on insertion, however, the
        // maximization is lifted (in a batch phase only at its end).
        if maximized {
            match &mut self.batch {
                Some(batch) => {
                    if !batch.unmaximize.contains(&window) {
                        batch.unmaximize.push(window);
                    }
                }
                None => self.unmaximize_into_tile(window),
            }
        }
    }

    /// Lifts the maximization of a tiled window and puts it on its
    /// tile.
    fn unmaximize_into_tile(&mut self, window: Window) {
        if let Some(key) = self.workspace_of(window)
            && let Some(rect) = self.target_rect(&key, window.id())
        {
            position::set_rect(window, rect);
        }
    }

    fn untile(&mut self, window: Window) {
        if let Some(key) = self.workspace_of(window) {
            self.workspaces
                .get_mut(&key)
                .expect("exists")
                .remove(window.id());
            self.apply(&key);
        }
    }

    /// Turns a floating window into a tiled one (without inserting it).
    /// `before` is the rect it returns to when it is untiled.
    fn mark_tiled(&mut self, window: Window, before: Rect) {
        if let Some(tracked) = self.windows.get_mut(&window) {
            tracked.mode = Mode::Tiled;
            tracked.original.get_or_insert(Original {
                rect: before,
                maximized: false,
            });
        }
    }

    fn on_minimize(&mut self, window: Window, minimized: bool) {
        let Some(tracked) = self.windows.get_mut(&window) else {
            return;
        };
        if tracked.minimized == minimized {
            return;
        }
        tracked.minimized = minimized;
        if tracked.mode == Mode::Scratchpad {
            self.on_scratchpad_minimize(window, minimized);
            return;
        }
        if tracked.mode != Mode::Tiled {
            return;
        }
        if minimized {
            self.untile(window);
        } else {
            self.tile(window, None);
        }
    }

    fn on_foreground(&mut self, window: Window) {
        self.hide_scratchpads_on_focus_loss(window);
        self.foreground = Some(window);
        self.update_border(Some(window));
        let Some(key) = self.workspace_of(window) else {
            self.refresh_bars();
            return;
        };
        let ws = self.workspaces.get_mut(&key).expect("exists");
        ws.last_focus = Some(window.id());
        // Bring a hidden stack window to the front (e.g. via the taskbar).
        if ws
            .with_tree(window.id(), |tree, _, _| tree.activate(window.id()))
            .unwrap_or(false)
        {
            self.apply(&key);
        } else {
            // Update the z-order of the tab bars.
            self.refresh_bars();
        }
    }

    /// Colors the border of the focused managed window and resets that of
    /// the previous one.
    fn update_border(&mut self, focused: Option<Window>) {
        let appearance = &self.config.config.appearance;
        let color = appearance
            .active_border
            .then_some(appearance.active_border_color);
        let target = focused
            .filter(|w| self.windows.contains_key(w))
            .filter(|_| color.is_some());
        if self.border == target {
            return;
        }
        if let Some(old) = self.border.take() {
            position::set_border_color(old, None);
        }
        if let (Some(window), Some(color)) = (target, color) {
            position::set_border_color(window, Some(color));
            self.border = Some(window);
        }
    }

    /// Window was moved to another desktop.
    fn follow_desktop(&mut self, window: Window) {
        let Some(current) = self.workspace_of(window) else {
            return;
        };
        let wanted = self.key_for_window(window);
        if wanted != current {
            self.untile(window);
            self.tile(window, Some((wanted, None)));
        }
    }

    // ----- Dragging with the mouse ----------------------------------------------

    fn on_drag_start(&mut self, window: Window) {
        // Scratchpads are moved freely and never dropped into a zone.
        if self
            .windows
            .get(&window)
            .is_none_or(|t| t.mode == Mode::Scratchpad)
        {
            return;
        }
        self.drag = Some(Drag {
            window,
            start: window.rect(),
            resizing: false,
        });
        self.timers.set(DRAG_TIMER, DRAG_INTERVAL_MS);
    }

    /// While dragging: show the zones as long as the key is held.
    fn drag_tick(&mut self) {
        let Some(drag) = &mut self.drag else {
            self.timers.kill(DRAG_TIMER);
            return;
        };
        let window = drag.window;
        if !drag.resizing {
            let now = window.rect();
            drag.resizing = (now.width, now.height) != (drag.start.width, drag.start.height);
        }
        let target = if drag.resizing {
            None
        } else {
            self.drop_target_at_cursor(window)
        };
        match target {
            Some((key, target)) => self.show_zones(&key, Some((window, target))),
            None => self.hide_zones(),
        }
    }

    fn on_drag_end(&mut self, window: Window) {
        self.timers.kill(DRAG_TIMER);
        self.hide_zones();
        let Some(drag) = self.drag.take().filter(|d| d.window == window) else {
            return;
        };
        if drag.resizing {
            // Resize by mouse: carry the moved edges over into the ratios in the tree.
            if let Some(key) = self.workspace_of(window) {
                if let Some(before) = self.target_rect(&key, window.id()) {
                    let after = window.rect();
                    self.workspaces.get_mut(&key).expect("exists").resize_edges(
                        window.id(),
                        before,
                        after,
                    );
                }
                self.apply(&key);
            }
            return;
        }
        if let Some((key, target)) = self.drop_target_at_cursor(window) {
            self.drop_into(window, drag.start, &key, target);
        } else {
            self.drop_without_zones(window);
        }
    }

    /// Target under the mouse pointer if the zone key is held.
    fn drop_target_at_cursor(&mut self, window: Window) -> Option<(WsKey, DropTarget)> {
        if !position::drag_modifier_held(self.config.config.drag.modifier) {
            return None;
        }
        let (x, y) = position::cursor_pos();
        let monitor = self.monitors.iter().find(|m| m.rect.contains_point(x, y))?;
        let key = (monitor.id.clone(), self.key_for_window(window).1);
        let target = self.workspace_mut(&key).drop_target(x, y, window.id())?;
        Some((key, target))
    }

    /// Released with the zone key held: insert into a zone or next to/onto a
    /// window. Floating windows get tiled in the process.
    fn drop_into(&mut self, window: Window, before: Rect, key: &WsKey, target: DropTarget) {
        self.mark_tiled(window, before);
        if let Some(old) = self.workspace_of(window)
            && old != *key
        {
            self.untile(window);
        }
        self.workspace_mut(key).insert_at(window.id(), target);
        self.apply(key);
    }

    /// Released without the zone key: tiled windows swap with the window
    /// under the pointer, change monitors or snap back.
    fn drop_without_zones(&mut self, window: Window) {
        let Some(key) = self.workspace_of(window) else {
            return;
        };
        let (x, y) = position::cursor_pos();
        let target_key = self
            .monitors
            .iter()
            .find(|m| m.rect.contains_point(x, y))
            .map(|m| (m.id.clone(), key.1.clone()))
            .unwrap_or_else(|| key.clone());
        let under_cursor = self.workspaces.get(&target_key).and_then(|ws| {
            ws.placements()
                .into_iter()
                .find(|p| p.visible && p.id != window.id() && p.rect.contains_point(x, y))
                .map(|p| p.id)
        });

        if target_key != key {
            self.untile(window);
            self.tile(window, Some((target_key, under_cursor)));
        } else {
            if let Some(other) = under_cursor {
                self.workspace_mut(&key).swap(window.id(), other);
            }
            self.apply(&key);
        }
    }

    // ----- Overlay ------------------------------------------------------------

    /// Shows the zones of a workspace; with `drop` also the target zone and
    /// a preview for the dragged window.
    fn show_zones(&mut self, key: &WsKey, drop: Option<(Window, DropTarget)>) {
        let (Some(ws), Some(monitor)) = (
            self.workspaces.get(key),
            self.monitors.iter().find(|m| m.id == key.0),
        ) else {
            return;
        };
        let appearance = &self.config.config.appearance;
        let scene = Scene {
            monitor: monitor.rect,
            dpi: monitor.dpi,
            zones: ws.zones().iter().map(|z| z.rect).collect(),
            highlight: drop.map(|(_, t)| t.zone),
            preview: drop.and_then(|(window, t)| ws.preview(window.id(), t)),
            style: Style {
                zone: appearance.zone_color,
                highlight: appearance.zone_highlight_color,
                opacity: appearance.zone_opacity,
            },
        };
        if let Some(overlay) = &mut self.overlay
            && let Err(e) = overlay.show(scene)
        {
            tracing::warn!("Overlay could not be drawn: {e:#}");
        }
    }

    fn hide_zones(&mut self) {
        if let Some(overlay) = &mut self.overlay {
            overlay.hide();
        }
    }

    // ----- Actions ------------------------------------------------------------

    pub fn run_action(&mut self, action: Action) -> Result<(), String> {
        tracing::debug!("Action: {action}");
        match action {
            Action::Focus(dir) => self.focus_dir(dir),
            Action::Move(dir) => self.move_dir(dir),
            Action::Swap(dir) => {
                let (window, key) = self.focused_tiled()?;
                if self.workspace_mut(&key).swap_dir(window.id(), dir) {
                    self.apply(&key);
                }
                Ok(())
            }
            Action::MoveToZone(zone) => self.move_to_zone(usize::from(zone) - 1),
            Action::CycleLayout(cycle) => self.cycle_layout(cycle),
            Action::ToggleFloat => self.toggle_float(),
            // Stack and orientation apply to the whole zone and are remembered.
            Action::ToggleStack => self.change_zone_mode(|mode, _| match mode {
                ZoneMode::Stack => ZoneMode::Auto,
                _ => ZoneMode::Stack,
            }),
            Action::CycleStack(cycle) => {
                self.with_focused_tree(|tree, id, _, _| tree.cycle_stack(id, cycle == Cycle::Next))
            }
            Action::ToggleOrientation => self.change_zone_mode(|mode, zone| match mode {
                ZoneMode::Columns => ZoneMode::Rows,
                ZoneMode::Rows => ZoneMode::Columns,
                // From auto or stack: rotate the visible split, otherwise take
                // the direction that matches the zone's shape.
                ZoneMode::Auto | ZoneMode::Stack => match zone.tree.root() {
                    Some(Node::Split {
                        orientation: Orientation::Horizontal,
                        ..
                    }) => ZoneMode::Rows,
                    Some(Node::Split {
                        orientation: Orientation::Vertical,
                        ..
                    }) => ZoneMode::Columns,
                    _ if Orientation::for_rect(zone.rect) == Orientation::Horizontal => {
                        ZoneMode::Columns
                    }
                    _ => ZoneMode::Rows,
                },
            }),
            Action::Resize(resize) => self.with_focused_tree(|tree, id, _, _| {
                let delta = match resize {
                    Resize::Grow => RESIZE_STEP,
                    Resize::Shrink => -RESIZE_STEP,
                };
                tree.resize(id, delta);
                None
            }),
            Action::ToggleMonocle => self.with_focused_tree(|tree, id, _, _| {
                tree.toggle_monocle(id);
                None
            }),
            Action::TogglePause => self.toggle_pause(),
            Action::SwitchDesktop(n) => {
                self.require_desktop_control()?;
                vdesk::switch_to(usize::from(n) - 1)
            }
            Action::MoveToDesktop(n) => self.move_to_desktop(usize::from(n) - 1),
            Action::Hint => self.start_hints(),
            Action::ToggleScratchpad(n) => self.toggle_scratchpad(usize::from(n) - 1),
            Action::OpenEditor | Action::OpenExplorer | Action::ReloadConfig | Action::Quit => {
                unreachable!("handled by the daemon")
            }
        }
    }

    fn focus_dir(&mut self, dir: Direction) -> Result<(), String> {
        let hidden: HashSet<WindowId> = self
            .workspaces
            .values()
            .flat_map(|ws| ws.placements())
            .filter(|p| !p.visible)
            .map(|p| p.id)
            .collect();
        let from = match self.focused() {
            Some(window) => window.rect(),
            None => {
                let (x, y) = position::cursor_pos();
                Rect::new(x, y, 1, 1)
            }
        };
        let current = self.focused();
        let candidates: Vec<(Window, Rect)> = self
            .windows
            .iter()
            .filter(|(w, t)| {
                Some(**w) != current
                    && !t.minimized
                    && !hidden.contains(&w.id())
                    && !self.is_on_other_desktop(**w)
            })
            .map(|(w, _)| (*w, w.rect()))
            .collect();
        let target = nav::neighbor(from, dir, candidates)
            .ok_or_else(|| format!("no window in direction {dir}"))?;
        if position::focus(target) {
            Ok(())
        } else {
            Err("Windows refused the focus change".into())
        }
    }

    /// Swap with the neighbor within the zone, otherwise move to the
    /// neighboring zone; at the edge, switch to the neighboring monitor.
    fn move_dir(&mut self, dir: Direction) -> Result<(), String> {
        let (window, key) = self.focused_tiled()?;
        if self.workspace_mut(&key).move_dir(window.id(), dir) {
            self.apply(&key);
            return Ok(());
        }
        let current = self
            .monitors
            .iter()
            .find(|m| m.id == key.0)
            .ok_or("monitor not found")?;
        let target = nav::neighbor(
            current.rect,
            dir,
            self.monitors.iter().map(|m| (m.id.as_str(), m.rect)),
        )
        .ok_or_else(|| format!("no room in direction {dir}"))?;
        let target_key = (target.to_owned(), key.1.clone());
        self.untile(window);
        self.tile(window, Some((target_key, None)));
        Ok(())
    }

    /// Focused window into zone `zone` (0-based); floating ones get tiled.
    fn move_to_zone(&mut self, zone: usize) -> Result<(), String> {
        let window = self
            .focused()
            .ok_or("no managed window in the foreground")?;
        self.require_not_scratchpad(window)?;
        let key = self
            .workspace_of(window)
            .unwrap_or_else(|| self.key_for_window(window));
        let ws = self.workspace_mut(&key);
        let count = ws.zones().len();
        if zone >= count {
            return Err(format!("layout `{}` only has {count} zone(s)", ws.layout));
        }
        self.mark_tiled(window, window.rect());
        if self.workspace_mut(&key).move_to_zone(window.id(), zone) {
            self.apply(&key);
        }
        Ok(())
    }

    /// Next/previous layout for the current monitor.
    fn cycle_layout(&mut self, cycle: Cycle) -> Result<(), String> {
        let key = self.current_key()?;
        let current = self.workspace_mut(&key).layout.clone();
        let next = self.config.layouts.cycle(&current, cycle).name.clone();
        self.set_layout(&key, &next);
        tracing::info!(monitor = key.0, layout = next, "Layout switched");
        self.show_zones(&key, None);
        self.timers.set(FLASH_TIMER, FLASH_MS);
        Ok(())
    }

    fn require_desktop_control(&self) -> Result<(), String> {
        if self.desktop_control {
            Ok(())
        } else {
            Err("virtual desktops cannot be controlled on this Windows build".into())
        }
    }

    /// Move the focused window to desktop `index` (0-based). It keeps its
    /// monitor and tiling mode; the focus goes to a window that stays on the
    /// current desktop.
    fn move_to_desktop(&mut self, index: usize) -> Result<(), String> {
        self.require_desktop_control()?;
        let window = self
            .focused()
            .ok_or("no managed window in the foreground")?;
        let old_key = self
            .workspace_of(window)
            .unwrap_or_else(|| self.key_for_window(window));
        let desktop = vdesk::move_window(window, index)?;
        if desktop == old_key.1 {
            return Ok(());
        }
        if self.workspace_of(window).is_some() {
            self.untile(window);
            self.tile(window, Some(((old_key.0.clone(), desktop), None)));
        }
        let next = self.workspaces.get(&old_key).and_then(|ws| {
            ws.last_focus
                .filter(|id| *id != window.id())
                .or_else(|| ws.placements().iter().find(|p| p.visible).map(|p| p.id))
        });
        if let Some(next) = next {
            position::focus(Window::from_id(next));
        }
        Ok(())
    }

    fn toggle_float(&mut self) -> Result<(), String> {
        let window = self
            .focused()
            .ok_or("no managed window in the foreground")?;
        self.require_not_scratchpad(window)?;
        let tracked = self.windows.get_mut(&window).expect("managed");
        match tracked.mode {
            Mode::Tiled => {
                tracked.mode = Mode::Floating;
                let original = tracked.original.take();
                self.untile(window);
                if let Some(original) = original {
                    restore(window, self.on_screen(original));
                }
            }
            Mode::Floating => {
                tracked.mode = Mode::Tiled;
                tracked.original = None;
                self.tile(window, None);
            }
            Mode::Scratchpad => unreachable!("checked above"),
        }
        Ok(())
    }

    fn require_not_scratchpad(&self, window: Window) -> Result<(), String> {
        match self.windows.get(&window) {
            Some(t) if t.mode == Mode::Scratchpad => {
                Err("the window is a scratchpad and always floats".into())
            }
            _ => Ok(()),
        }
    }

    // ----- Window hints ---------------------------------------------------------

    /// The hints capture the keyboard (see [`Wm::on_hint_key`]).
    pub fn hints_active(&self) -> bool {
        self.hints.is_some()
    }

    /// Labels every visible window on the current desktop with letters.
    fn start_hints(&mut self) -> Result<(), String> {
        self.end_hints();
        let hidden: HashSet<WindowId> = self
            .workspaces
            .values()
            .flat_map(|ws| ws.placements())
            .filter(|p| !p.visible)
            .map(|p| p.id)
            .collect();
        let mut candidates: Vec<(Window, Rect, &Monitor)> = self
            .windows
            .iter()
            .filter(|(w, t)| {
                !t.minimized
                    && !w.is_minimized()
                    && w.is_visible()
                    && !hidden.contains(&w.id())
                    && !self.is_on_other_desktop(**w)
            })
            .filter_map(|(w, _)| {
                let rect = w.rect();
                let (cx, cy) = rect.center();
                let monitor = self
                    .monitors
                    .iter()
                    .find(|m| m.rect.contains_point(cx, cy))?;
                Some((*w, rect, monitor))
            })
            .collect();
        if candidates.is_empty() {
            return Err("no window to jump to".into());
        }
        // Reading order: monitors from left to right, then top to bottom.
        candidates
            .sort_by_key(|(_, rect, monitor)| (monitor.rect.x, monitor.rect.y, rect.y, rect.x));
        let labels = hint::labels(candidates.len());
        candidates.truncate(labels.len());
        let hints = candidates
            .iter()
            .zip(&labels)
            .map(|((window, rect, monitor), label)| {
                let tracked = &self.windows[window];
                Hint {
                    label: label.clone(),
                    title: if tracked.title.is_empty() {
                        tracked.exe.clone()
                    } else {
                        tracked.title.clone()
                    },
                    window: *rect,
                    dpi: monitor.dpi,
                }
            })
            .collect();
        let targets = candidates.iter().map(|(w, _, _)| *w).collect();
        self.hints = Some(HintSession {
            targets,
            labels,
            hints,
            typed: String::new(),
        });
        self.draw_hints();
        Ok(())
    }

    fn draw_hints(&mut self) {
        let Some(session) = &self.hints else {
            return;
        };
        let appearance = &self.config.config.appearance;
        let style = HintStyle {
            background: appearance.zone_color,
            highlight: appearance.zone_highlight_color,
        };
        if let Err(e) = self
            .hint_overlay
            .show(&session.hints, &session.typed, style)
        {
            tracing::warn!("Window hints could not be drawn: {e:#}");
        }
        // Every key press restarts the countdown.
        self.timers.set(HINT_TIMER, HINT_MS);
    }

    pub fn end_hints(&mut self) {
        if self.hints.take().is_some() {
            self.hint_overlay.hide();
            self.timers.kill(HINT_TIMER);
        }
    }

    /// Key pressed while the hints capture the keyboard. Letters select a
    /// window: alone they focus it, with Shift it swaps places with the
    /// focused window, with Ctrl it joins the focused window's stack.
    /// Backspace takes back a letter; any other key cancels.
    pub fn on_hint_key(&mut self, vk: u16, mods: Modifiers) {
        let Some(session) = &mut self.hints else {
            return;
        };
        match vk {
            0x41..=0x5A => session.typed.push((vk as u8 as char).to_ascii_lowercase()),
            VK_BACK => {
                session.typed.pop();
                self.draw_hints();
                return;
            }
            _ => {
                self.end_hints();
                return;
            }
        }
        match hint::resolve(&session.labels, &session.typed) {
            HintMatch::Exact(index) => {
                let target = session.targets[index];
                self.end_hints();
                if let Err(message) = self.jump(target, mods) {
                    tracing::info!("hint: {message}");
                }
            }
            HintMatch::Prefix => self.draw_hints(),
            HintMatch::None => self.end_hints(),
        }
    }

    fn jump(&mut self, target: Window, mods: Modifiers) -> Result<(), String> {
        if !self.windows.contains_key(&target) {
            return Err("the window no longer exists".into());
        }
        if mods.contains(Modifiers::CTRL) {
            self.pull_into_stack(target)
        } else if mods.contains(Modifiers::SHIFT) {
            self.swap_with(target)
        } else if position::focus(target) {
            Ok(())
        } else {
            Err("Windows refused the focus change".into())
        }
    }

    /// The focused window and `target` swap places, also across zones and
    /// monitors. Both must be tiled.
    fn swap_with(&mut self, target: Window) -> Result<(), String> {
        let (window, key) = self.focused_tiled()?;
        if window == target {
            return Ok(());
        }
        let target_key = self
            .workspace_of(target)
            .ok_or("the selected window is not tiled")?;
        let (a, b) = (window.id(), target.id());
        if key == target_key {
            self.workspace_mut(&key).swap(a, b);
        } else {
            self.workspace_mut(&key).replace(a, b);
            self.workspace_mut(&target_key).replace(b, a);
            self.apply(&target_key);
        }
        self.apply(&key);
        Ok(())
    }

    /// `target` joins the stack of the focused window (which must be tiled)
    /// and comes to the front; floating windows get tiled in the process.
    fn pull_into_stack(&mut self, target: Window) -> Result<(), String> {
        let (window, key) = self.focused_tiled()?;
        if window == target {
            return Ok(());
        }
        self.require_not_scratchpad(target)?;
        let zone = self.workspaces[&key]
            .zone_of(window.id())
            .ok_or("window is not in any zone")?;
        if let Some(old) = self.workspace_of(target)
            && old != key
        {
            self.untile(target);
        }
        self.mark_tiled(target, target.rect());
        self.workspace_mut(&key).insert_at(
            target.id(),
            DropTarget {
                zone,
                beside: Some((window.id(), DropSide::Stack)),
            },
        );
        self.apply(&key);
        position::focus(target);
        Ok(())
    }

    // ----- Scratchpads ------------------------------------------------------------

    /// Matches the scratchpad slots to the (possibly reloaded)
    /// configuration by name. Windows of removed scratchpads become normal
    /// floating windows; scratchpads without a window take over a matching
    /// managed window (hidden).
    fn sync_scratchpads(&mut self) {
        let mut old = std::mem::take(&mut self.scratch);
        self.scratch = self
            .config
            .scratchpads
            .iter()
            .map(|pad| {
                let index = old.iter().position(|s| s.name == pad.name());
                index.map_or_else(
                    || ScratchSlot {
                        name: pad.name().to_owned(),
                        ..Default::default()
                    },
                    |i| old.swap_remove(i),
                )
            })
            .collect();
        for slot in old {
            if let Some(window) = slot.window
                && let Some(tracked) = self.windows.get_mut(&window)
            {
                tracked.mode = Mode::Floating;
                tracing::info!(hwnd = window.id(), name = slot.name, "Scratchpad removed");
            }
        }
        for index in 0..self.scratch.len() {
            if self.scratchpad_window(index).is_some() {
                continue;
            }
            let pad = self.config.scratchpads.get(index).expect("same length");
            let found = self
                .windows
                .iter()
                .find(|(_, t)| t.mode != Mode::Scratchpad && pad.matches(props(t)))
                .map(|(w, _)| *w);
            if let Some(window) = found {
                self.untile(window);
                let tracked = self.windows.get_mut(&window).expect("managed");
                tracked.mode = Mode::Scratchpad;
                self.claim_scratchpad(index, window, false);
            }
        }
    }

    /// A managed window whose title now matches a scratchpad without a
    /// window becomes that scratchpad and is shown. Some programs set the
    /// title only after the window appears (Notepad shows the file name
    /// once it has opened the file).
    fn claim_on_title_change(&mut self, window: Window) {
        let Some(tracked) = self.windows.get(&window) else {
            return;
        };
        if tracked.mode == Mode::Scratchpad {
            return;
        }
        let Some(index) = self.config.scratchpads.find(props(tracked)) else {
            return;
        };
        if self.scratchpad_window(index).is_some() {
            return;
        }
        self.untile(window);
        let tracked = self.windows.get_mut(&window).expect("managed");
        tracked.mode = Mode::Scratchpad;
        self.claim_scratchpad(index, window, true);
    }

    /// The live window of scratchpad `index`.
    fn scratchpad_window(&self, index: usize) -> Option<Window> {
        self.scratch
            .get(index)?
            .window
            .filter(|w| self.windows.contains_key(w) && w.exists())
    }

    fn scratchpad_of(&self, window: Window) -> Option<usize> {
        self.scratch.iter().position(|s| s.window == Some(window))
    }

    /// `window` (already in `Mode::Scratchpad`) becomes the window of
    /// scratchpad `index`. It is shown if it was started for that or `show`
    /// is set (a program the user just started), otherwise hidden.
    fn claim_scratchpad(&mut self, index: usize, window: Window, show: bool) {
        let slot = &mut self.scratch[index];
        let launched = slot
            .launched
            .take()
            .is_some_and(|t| t.elapsed() < SCRATCHPAD_LAUNCH_WAIT);
        slot.window = Some(window);
        tracing::info!(
            hwnd = window.id(),
            name = slot.name,
            "Window is a scratchpad"
        );
        if window.is_minimized() {
            return;
        }
        if show || launched {
            self.summon(index);
        } else {
            position::minimize(window);
        }
    }

    /// Hides the scratchpad if it is in front, otherwise brings it to the
    /// current monitor; without a window its program is started.
    fn toggle_scratchpad(&mut self, index: usize) -> Result<(), String> {
        let pad = self
            .config
            .scratchpads
            .get(index)
            .ok_or_else(|| format!("there is no scratchpad {}", index + 1))?
            .config
            .clone();
        match self.scratchpad_window(index) {
            Some(window) if !window.is_minimized() && position::foreground() == Some(window) => {
                position::minimize(window);
                Ok(())
            }
            Some(_) => {
                self.summon(index);
                Ok(())
            }
            None => {
                let command = pad.command.as_deref().ok_or_else(|| {
                    format!(
                        "scratchpad `{}`: no window matches and no command is set",
                        pad.name
                    )
                })?;
                if self.scratch[index]
                    .launched
                    .is_some_and(|t| t.elapsed() < SCRATCHPAD_START_DEBOUNCE)
                {
                    return Err(format!(
                        "scratchpad `{}`: its program is still starting",
                        pad.name
                    ));
                }
                let expand = |text: &str| expand_env(text, |name| std::env::var(name).ok());
                let args: Vec<String> = pad.args.iter().map(|a| expand(a)).collect();
                spawn(&expand(command), &args)?;
                let monitor = self.summon_monitor();
                let slot = &mut self.scratch[index];
                slot.launched = Some(Instant::now());
                slot.monitor = monitor;
                tracing::info!(name = pad.name, command, "Scratchpad program started");
                Ok(())
            }
        }
    }

    /// Brings the scratchpad to the current desktop and monitor and focuses
    /// it. A minimized one is placed once it is restored
    /// ([`Wm::on_scratchpad_minimize`]).
    fn summon(&mut self, index: usize) {
        let Some(window) = self.scratchpad_window(index) else {
            return;
        };
        self.scratch[index].monitor = self.summon_monitor();
        if self.desktop_control && self.is_on_other_desktop(window) {
            let current = self.current_desktop();
            if let Some(i) = vdesk::desktop_ids()
                .ok()
                .and_then(|ids| ids.iter().position(|id| *id == current))
                && let Err(e) = vdesk::move_window(window, i)
            {
                tracing::warn!("Scratchpad could not be moved to this desktop: {e}");
            }
        }
        if !window.is_minimized() {
            self.place_scratchpad(index);
        }
        position::focus(window);
    }

    fn place_scratchpad(&self, index: usize) {
        let (Some(window), Some(pad)) = (
            self.scratchpad_window(index),
            self.config.scratchpads.get(index),
        ) else {
            return;
        };
        let monitor = self.scratch[index]
            .monitor
            .as_ref()
            .and_then(|id| self.monitors.iter().find(|m| m.id == *id))
            .or_else(|| self.monitor_at_cursor());
        if let Some(monitor) = monitor {
            position::set_rect(window, pad.rect(monitor.work_area));
        }
    }

    /// Monitor of the focused window (unless it is a scratchpad itself),
    /// otherwise the one under the mouse pointer.
    fn summon_monitor(&self) -> Option<String> {
        let handle = self
            .focused()
            .filter(|w| self.windows[w].mode != Mode::Scratchpad)
            .map(monitor::handle_for_window);
        handle
            .and_then(|h| self.monitors.iter().find(|m| m.handle == h))
            .or_else(|| self.monitor_at_cursor())
            .map(|m| m.id.clone())
    }

    fn monitor_at_cursor(&self) -> Option<&Monitor> {
        let (x, y) = position::cursor_pos();
        self.monitors
            .iter()
            .find(|m| m.rect.contains_point(x, y))
            .or(self.monitors.first())
    }

    /// A scratchpad was restored (by [`Wm::summon`] or from the taskbar):
    /// put it in its place. When it is minimized, it next appears on the
    /// monitor in use then.
    fn on_scratchpad_minimize(&mut self, window: Window, minimized: bool) {
        let Some(index) = self.scratchpad_of(window) else {
            return;
        };
        if minimized {
            self.scratch[index].monitor = None;
        } else {
            if self.scratch[index].monitor.is_none() {
                self.scratch[index].monitor = self.monitor_at_cursor().map(|m| m.id.clone());
            }
            self.place_scratchpad(index);
        }
    }

    /// Hides scratchpads with `hide_on_focus_loss` when another window
    /// (that is not one of their dialogs) comes to the front.
    fn hide_scratchpads_on_focus_loss(&self, foreground: Window) {
        for (index, pad) in self.config.scratchpads.iter().enumerate() {
            if !pad.config.hide_on_focus_loss {
                continue;
            }
            if let Some(window) = self.scratchpad_window(index)
                && window != foreground
                && foreground.owner() != Some(window)
                && !window.is_minimized()
            {
                position::minimize(window);
            }
        }
    }

    // ----- Scenes -------------------------------------------------------------------

    /// Identifiers of the connected monitors.
    pub fn monitor_ids(&self) -> Vec<&str> {
        self.monitors.iter().map(|m| m.id.as_str()).collect()
    }

    /// Display names of the connected monitors, e.g. for naming a scene.
    pub fn monitor_names(&self) -> Vec<&str> {
        self.monitors.iter().map(|m| m.name.as_str()).collect()
    }

    /// Display name of a monitor, if it is connected.
    pub fn monitor_name(&self, id: &str) -> Option<&str> {
        self.monitors
            .iter()
            .find(|m| m.id == id)
            .map(|m| m.name.as_str())
    }

    /// The arrangement of all monitors on the current desktop as a scene.
    pub fn capture_scene(&self, name: &str, auto: bool, launch: bool) -> SavedScene {
        let desktop = self.current_desktop();
        let monitors = self
            .monitors
            .iter()
            .map(|m| {
                let ws = self.workspaces.get(&(m.id.clone(), desktop.clone()));
                SceneMonitor {
                    id: m.id.clone(),
                    name: m.name.clone(),
                    layout: ws.map_or_else(
                        || self.config.config.general.default_layout.clone(),
                        |ws| ws.layout.clone(),
                    ),
                    zones: ws
                        .map_or_else(Vec::new, |ws| ws.zones().iter().map(|z| z.mode).collect()),
                    windows: ws.map_or_else(Vec::new, |ws| {
                        ws.windows()
                            .into_iter()
                            .filter_map(|id| {
                                let window = Window::from_id(id);
                                let tracked = self.windows.get(&window)?;
                                Some(SceneWindow {
                                    exe: tracked.exe.clone(),
                                    class: tracked.class.clone(),
                                    title: tracked.title.clone(),
                                    zone: ws.zone_of(id)? + 1,
                                    path: window.exe_path().unwrap_or_default(),
                                })
                            })
                            .collect()
                    }),
                }
            })
            .collect();
        SavedScene {
            name: name.to_owned(),
            auto,
            launch,
            monitors,
        }
    }

    /// Applies a scene on the current desktop: layouts and zone modes of its
    /// connected monitors, then tiled windows move to the zones the scene
    /// has for their program. With `launch`, programs without a window are
    /// started and land in their zone when their window appears. Returns a
    /// short summary.
    pub fn apply_scene(&mut self, scene: &SavedScene) -> Result<String, String> {
        let desktop = self.current_desktop();
        let present: Vec<usize> = (0..scene.monitors.len())
            .filter(|&i| self.monitors.iter().any(|m| m.id == scene.monitors[i].id))
            .collect();
        if present.is_empty() {
            return Err(format!(
                "none of the monitors of scene `{}` is connected",
                scene.name
            ));
        }
        self.end_hints();
        let mut live: Vec<Window> = self
            .windows
            .iter()
            .filter(|(w, t)| {
                t.mode == Mode::Tiled
                    && !t.minimized
                    && self.workspace_of(**w).is_some_and(|k| k.1 == desktop)
            })
            .map(|(w, _)| *w)
            .collect();
        // Stable order for several windows of the same program.
        live.sort_by_key(|w| {
            let r = w.rect();
            (r.x, r.y, w.id())
        });
        let assignments = {
            let live_props: Vec<WindowProps<'_>> =
                live.iter().map(|w| props(&self.windows[w])).collect();
            scene.assign(&live_props)
        };

        let (mut moved, mut launched) = (0, 0);
        self.with_batched_apply(|wm| {
            for &i in &present {
                let monitor = &scene.monitors[i];
                let key = (monitor.id.clone(), desktop.clone());
                wm.set_layout(&key, &monitor.layout);
                wm.workspace_mut(&key).restore_modes(&monitor.zones);
                wm.apply(&key);
            }
            for a in assignments.iter().filter(|a| present.contains(&a.monitor)) {
                let monitor = &scene.monitors[a.monitor];
                let zone = monitor.windows[a.entry].zone.saturating_sub(1);
                let window = live[a.window];
                let id = window.id();
                let key = (monitor.id.clone(), desktop.clone());
                match wm.workspace_of(window) {
                    Some(current) if current == key => {
                        if wm.workspaces[&key].zone_of(id) != Some(zone)
                            && wm.workspace_mut(&key).move_to_zone(id, zone)
                        {
                            moved += 1;
                        }
                    }
                    Some(_) => {
                        wm.untile(window);
                        wm.workspace_mut(&key).insert(id, Some(zone), None);
                        moved += 1;
                    }
                    None => continue,
                }
                wm.apply(&key);
            }
            if scene.launch {
                launched = wm.launch_missing(scene, &present, &assignments);
            }
        });
        tracing::info!(scene = scene.name, moved, launched, "Scene applied");
        Ok(format!(
            "scene `{}`: {moved} window(s) moved, {launched} program(s) started",
            scene.name
        ))
    }

    /// Starts the programs of scene entries that got no window; their
    /// windows are expected for [`SCENE_LAUNCH_WAIT`].
    fn launch_missing(
        &mut self,
        scene: &SavedScene,
        present: &[usize],
        assignments: &[tessera_core::scene::Assignment],
    ) -> usize {
        let until = Instant::now() + SCENE_LAUNCH_WAIT;
        let mut launched = 0;
        for &m in present {
            let monitor = &scene.monitors[m];
            for (e, entry) in monitor.windows.iter().enumerate() {
                let assigned = assignments.iter().any(|a| a.monitor == m && a.entry == e);
                if assigned || !can_launch(&entry.path) {
                    continue;
                }
                if launched == SCENE_MAX_LAUNCHES {
                    tracing::warn!(scene = scene.name, "Too many programs to start, stopping");
                    return launched;
                }
                if let Err(e) = spawn(&entry.path, &[]) {
                    tracing::warn!("{e}");
                    continue;
                }
                launched += 1;
                self.pending.push(PendingWindow {
                    monitor: monitor.id.clone(),
                    zone: entry.zone.saturating_sub(1),
                    exe: entry.exe.clone(),
                    class: entry.class.clone(),
                    title: entry.title.clone(),
                    until,
                });
            }
        }
        launched
    }

    /// Monitor and zone a scene started this window for, if any (same
    /// program, the same title preferred). The entry is consumed.
    fn take_pending(&mut self, window: Window) -> Option<(String, usize)> {
        let now = Instant::now();
        self.pending.retain(|p| p.until > now);
        let tracked = self.windows.get(&window)?;
        let same_program = |p: &PendingWindow| {
            p.exe.eq_ignore_ascii_case(&tracked.exe) && p.class == tracked.class
        };
        let index = self
            .pending
            .iter()
            .position(|p| same_program(p) && p.title == tracked.title)
            .or_else(|| self.pending.iter().position(same_program))?;
        let pending = self.pending.remove(index);
        self.monitors
            .iter()
            .any(|m| m.id == pending.monitor)
            .then_some((pending.monitor, pending.zone))
    }

    fn toggle_pause(&mut self) -> Result<(), String> {
        let key = self.current_key()?;
        let ws = self.workspace_mut(&key);
        ws.paused = !ws.paused;
        tracing::info!(monitor = key.0, paused = ws.paused, "Tiling paused/resumed");
        self.apply(&key);
        Ok(())
    }

    /// Sets the mode of the focused window's zone to `f(old, zone)` and
    /// rearranges it; the focused window stays visible.
    fn change_zone_mode(
        &mut self,
        f: impl FnOnce(ZoneMode, &Zone) -> ZoneMode,
    ) -> Result<(), String> {
        let (window, key) = self.focused_tiled()?;
        let ws = self.workspace_mut(&key);
        let index = ws.zone_of(window.id()).ok_or("window is not in any zone")?;
        let zone = &ws.zones()[index];
        let mode = f(zone.mode, zone);
        ws.set_zone_mode(index, mode, Some(window.id()));
        tracing::info!(
            monitor = key.0,
            zone = index + 1,
            ?mode,
            "Zone mode changed"
        );
        self.apply(&key);
        Ok(())
    }

    /// Runs `f` on the tree of the focused window. If `f` returns a window,
    /// that window gets the focus afterwards.
    fn with_focused_tree(
        &mut self,
        f: impl FnOnce(&mut tessera_core::tree::Tree, WindowId, Rect, i32) -> Option<WindowId>,
    ) -> Result<(), String> {
        let (window, key) = self.focused_tiled()?;
        let id = window.id();
        let focus = self
            .workspace_mut(&key)
            .with_tree(id, |tree, area, gap| f(tree, id, area, gap))
            .flatten();
        self.apply(&key);
        if let Some(next) = focus {
            position::focus(Window::from_id(next));
        }
        Ok(())
    }

    // ----- Workspaces and positions --------------------------------------------

    /// Rearranges all windows of a workspace (in a batch phase only at its
    /// end).
    fn apply(&mut self, key: &WsKey) {
        if let Some(batch) = &mut self.batch {
            if !batch.keys.contains(key) {
                batch.keys.push(key.clone());
            }
            return;
        }
        let Some(ws) = self.workspaces.get(key) else {
            return;
        };
        if ws.paused {
            return;
        }
        let placements = ws.placements();
        let offset = self.stack_offset(&key.0);
        for p in &placements {
            let window = Window::from_id(p.id);
            // Windows the user maximized stay maximized until the user restores
            // them – then they land in their place in the tree.
            if window.is_maximized() {
                continue;
            }
            let rect = with_stack_offset(p, offset);
            if window.rect() != rect {
                position::set_rect(window, rect);
            }
        }
        // Put hidden stack/monocle windows behind the visible one.
        for hidden in placements.iter().filter(|p| !p.visible) {
            if let Some(top) = placements
                .iter()
                .find(|p| p.visible && p.rect == hidden.rect)
            {
                position::place_below(Window::from_id(hidden.id), Window::from_id(top.id));
            }
        }
        self.refresh_bars();
    }

    /// Target rect of a window (for stacks below the tab bar).
    fn target_rect(&self, key: &WsKey, id: WindowId) -> Option<Rect> {
        let placement = self
            .workspaces
            .get(key)?
            .placements()
            .into_iter()
            .find(|p| p.id == id)?;
        Some(with_stack_offset(&placement, self.stack_offset(&key.0)))
    }

    /// Space above stack windows for the tab bar, DPI-scaled:
    /// (height of the bar, gap to the window).
    fn stack_offset(&self, monitor_id: &str) -> (i32, i32) {
        let dpi = self
            .monitors
            .iter()
            .find(|m| m.id == monitor_id)
            .map_or(96, |m| m.dpi);
        (
            scale_px(self.config.config.appearance.stack_bar_height, dpi),
            scale_px(4, dpi),
        )
    }

    /// Shows a tab bar above every stack on the current desktop.
    fn refresh_bars(&mut self) {
        let current = self.current_desktop();
        let appearance = &self.config.config.appearance;
        let mut wanted: Vec<(Tabs, Window, Vec<WindowId>)> = Vec::new();

        for ((monitor_id, desktop), ws) in &self.workspaces {
            if *desktop != current || ws.paused {
                continue;
            }
            let Some(monitor) = self.monitors.iter().find(|m| m.id == *monitor_id) else {
                continue;
            };
            let (bar_height, _) = self.stack_offset(monitor_id);
            let placements = ws.placements();
            // Stack windows follow each other directly in the layout.
            for group in placements.chunk_by(|a, b| a.stacked && b.stacked && a.rect == b.rect) {
                let Some(first) = group.first().filter(|p| p.stacked) else {
                    continue;
                };
                let active = group.iter().position(|p| p.visible).unwrap_or(0);
                let top = Window::from_id(group[active].id);
                if top.is_maximized() {
                    continue;
                }
                let ids: Vec<WindowId> = group.iter().map(|p| p.id).collect();
                let titles = ids
                    .iter()
                    .map(|id| {
                        self.windows
                            .get(&Window::from_id(*id))
                            .map_or_else(String::new, |t| t.title.clone())
                    })
                    .collect();
                let tabs = Tabs {
                    rect: Rect::new(first.rect.x, first.rect.y, first.rect.width, bar_height),
                    dpi: monitor.dpi,
                    titles,
                    active,
                    background: appearance.zone_color,
                    highlight: appearance.zone_highlight_color,
                    close_buttons: appearance.stack_close_button,
                };
                wanted.push((tabs, top, ids));
            }
        }

        let count = wanted.len();
        for (i, (tabs, top, ids)) in wanted.into_iter().enumerate() {
            if i == self.bars.len() {
                match TabBar::new(i) {
                    Ok(bar) => self.bars.push(BarSlot {
                        bar,
                        windows: Vec::new(),
                    }),
                    Err(e) => {
                        tracing::warn!("Tab bar could not be created: {e:#}");
                        break;
                    }
                }
            }
            let slot = &mut self.bars[i];
            slot.windows = ids;
            if let Err(e) = slot.bar.show(tabs, top) {
                tracing::warn!("Tab bar could not be drawn: {e:#}");
            }
        }
        for slot in self.bars.iter_mut().skip(count) {
            slot.bar.hide();
            slot.windows.clear();
        }
    }

    /// Monitor and layout the zone editor should open with.
    pub fn editor_target(&mut self) -> Option<(String, String)> {
        let key = self.current_key().ok()?;
        let layout = self.workspace_mut(&key).layout.clone();
        Some((key.0, layout))
    }

    /// Sets a layout on a monitor (current desktop), e.g. from the editor.
    pub fn set_layout_on(&mut self, monitor: &str, layout: &str) -> Result<(), String> {
        if !self.monitors.iter().any(|m| m.id == monitor) {
            return Err(format!("monitor `{monitor}` does not exist"));
        }
        if self.config.layouts.get(layout).is_none() {
            return Err(format!("layout `{layout}` does not exist"));
        }
        let key = (monitor.to_owned(), self.current_desktop());
        self.set_layout(&key, layout);
        tracing::info!(monitor, layout, "Layout set");
        self.show_zones(&key, None);
        self.timers.set(FLASH_TIMER, FLASH_MS);
        Ok(())
    }

    /// Switches the layout of a workspace and redistributes the windows.
    fn set_layout(&mut self, key: &WsKey, layout: &str) {
        let Some((name, zones, gap)) = self.layout_zones(&key.0, layout) else {
            return;
        };
        let ws = self.workspace_mut(key);
        ws.layout = name;
        ws.set_zones(&zones, gap);
        self.apply(key);
    }

    /// Recompute the rects of all workspaces; workspaces of vanished monitors
    /// are moved to the primary monitor.
    fn relayout_workspaces(&mut self) {
        self.with_batched_apply(|wm| {
            let keys: Vec<WsKey> = wm.workspaces.keys().cloned().collect();
            for key in keys {
                if wm.area_for(&key.0).is_some() {
                    let layout = wm.workspaces[&key].layout.clone();
                    wm.set_layout(&key, &layout);
                    continue;
                }
                let orphan = wm.workspaces.remove(&key).expect("exists");
                let Some(primary) = wm.monitors.first() else {
                    continue;
                };
                let target = (primary.id.clone(), key.1.clone());
                tracing::info!(monitor = key.0, "Monitor removed, moving its windows");
                for id in orphan.windows() {
                    wm.tile(Window::from_id(id), Some((target.clone(), None)));
                }
            }
        });
    }

    /// Zones of the layout `layout` (or of the default layout if it no
    /// longer exists) on the monitor: (actual name, zones, gap).
    fn layout_zones(&self, monitor_id: &str, layout: &str) -> Option<(String, Vec<Rect>, i32)> {
        let (area, gap) = self.area_for(monitor_id)?;
        let layouts = &self.config.layouts;
        let layout = layouts
            .get(layout)
            .or_else(|| layouts.get(&self.config.config.general.default_layout))
            .or_else(|| layouts.iter().next())?;
        Some((layout.name.clone(), layout.zones(area, gap), gap))
    }

    /// Usable rect (work area minus outer gap) and inner gap of a monitor,
    /// DPI-scaled.
    fn area_for(&self, monitor_id: &str) -> Option<(Rect, i32)> {
        let m = self.monitors.iter().find(|m| m.id == monitor_id)?;
        Some(self.config.config.gaps.usable_area(m.work_area, m.dpi))
    }

    fn workspace_mut(&mut self, key: &WsKey) -> &mut Workspace {
        if !self.workspaces.contains_key(key) {
            // Layout, pause and zone modes from the last run, otherwise the default layout.
            let saved = self.saved.workspace(&key.0, &key.1);
            let layout = saved.map_or_else(
                || self.config.config.general.default_layout.clone(),
                |s| s.layout.clone(),
            );
            let paused = saved.is_some_and(|s| s.paused);
            let modes = saved.map(|s| s.zones.clone()).unwrap_or_default();
            let mut workspace = match self.layout_zones(&key.0, &layout) {
                Some((name, zones, gap)) => Workspace::new(name, &zones, gap),
                None => Workspace::single(Rect::new(0, 0, 1920, 1080), 0),
            };
            workspace.paused = paused;
            workspace.restore_modes(&modes);
            self.workspaces.insert(key.clone(), workspace);
        }
        self.workspaces.get_mut(key).expect("just created")
    }

    fn workspace_of(&self, window: Window) -> Option<WsKey> {
        self.workspaces
            .iter()
            .find(|(_, ws)| ws.contains(window.id()))
            .map(|(key, _)| key.clone())
    }

    /// Workspace a window belongs to based on its position.
    fn key_for_window(&self, window: Window) -> WsKey {
        let handle = monitor::handle_for_window(window);
        let monitor = self
            .monitors
            .iter()
            .find(|m| m.handle == handle)
            .or(self.monitors.first())
            .map(|m| m.id.clone())
            .unwrap_or_default();
        let desktop = self
            .desktops
            .as_ref()
            .and_then(|d| d.window_desktop(window))
            .unwrap_or_else(|| self.current_desktop());
        (monitor, desktop)
    }

    /// GUID of the current desktop. Querying it via `winvd` is a COM call;
    /// as long as desktop events arrive, the cache they keep up to date is
    /// therefore enough. Without them a desktop switch is not reported, so
    /// it is queried live every time.
    fn current_desktop(&self) -> String {
        if self.desktop_notices {
            self.desktop_cache.clone()
        } else {
            vdesk::current_desktop().unwrap_or_default()
        }
    }

    /// Workspace of the focused window, otherwise of the monitor under the
    /// mouse pointer on the current desktop.
    fn current_key(&self) -> Result<WsKey, String> {
        if let Some(window) = self.focused() {
            return Ok(self
                .workspace_of(window)
                .unwrap_or_else(|| self.key_for_window(window)));
        }
        let (x, y) = position::cursor_pos();
        let monitor = self
            .monitors
            .iter()
            .find(|m| m.rect.contains_point(x, y))
            .or(self.monitors.first())
            .ok_or("no monitor")?;
        Ok((monitor.id.clone(), self.current_desktop()))
    }

    /// The managed window in the foreground.
    fn focused(&self) -> Option<Window> {
        position::foreground()
            .filter(|w| self.windows.contains_key(w))
            .or(self.foreground.filter(|w| self.windows.contains_key(w)))
    }

    fn focused_tiled(&self) -> Result<(Window, WsKey), String> {
        let window = self
            .focused()
            .ok_or("no managed window in the foreground")?;
        let key = self
            .workspace_of(window)
            .ok_or("the foreground window is not tiled")?;
        Ok((window, key))
    }

    /// Hidden by the shell because it is on another virtual desktop (and is
    /// not, say, a suspended UWP app). Without the desktop API, any hiding by
    /// the shell counts as another desktop.
    fn is_on_other_desktop(&self, window: Window) -> bool {
        window.cloak() == Cloak::OtherDesktop
            && self
                .desktops
                .as_ref()
                .and_then(|d| d.is_on_current_desktop(window))
                != Some(true)
    }

    /// If the old position is no longer on a monitor (disconnected), the
    /// window is centered on the primary monitor. Obviously invalid rects
    /// (empty, or that of a minimized window at -32000/-32000) are not usable
    /// as a size either; the window then gets two thirds of the work
    /// area.
    fn on_screen(&self, original: Original) -> Original {
        let r = original.rect;
        let invalid = r.is_empty() || r.x <= -32000 || r.y <= -32000;
        let visible = !invalid
            && self
                .monitors
                .iter()
                .any(|m| m.work_area.intersection(&r).area() > 0);
        match self.monitors.first() {
            Some(primary) if !visible => {
                let w = primary.work_area;
                let (width, height) = if invalid {
                    (w.width * 2 / 3, w.height * 2 / 3)
                } else {
                    (r.width.min(w.width), r.height.min(w.height))
                };
                Original {
                    rect: Rect::new(
                        w.x + (w.width - width) / 2,
                        w.y + (w.height - height) / 2,
                        width,
                        height,
                    ),
                    ..original
                }
            }
            _ => original,
        }
    }

    // ----- Query --------------------------------------------------------------

    pub fn snapshot(&self) -> StateSnapshot {
        // The same state that tab bars and actions work from.
        let current_desktop = Some(self.current_desktop()).filter(|d| !d.is_empty());

        let monitors = self
            .monitors
            .iter()
            .map(|m| {
                let ws = self
                    .workspaces
                    .get(&(m.id.clone(), current_desktop.clone().unwrap_or_default()));
                MonitorState {
                    id: m.id.clone(),
                    name: m.name.clone(),
                    device: m.device.clone(),
                    rect: m.rect,
                    work_area: m.work_area,
                    dpi: m.dpi,
                    primary: m.primary,
                    paused: ws.is_some_and(|ws| ws.paused),
                    layout: ws.map_or_else(
                        || self.config.config.general.default_layout.clone(),
                        |ws| ws.layout.clone(),
                    ),
                    zones: ws
                        .map_or_else(Vec::new, |ws| ws.zones().iter().map(|z| z.rect).collect()),
                    zone_modes: ws
                        .map_or_else(Vec::new, |ws| ws.zones().iter().map(|z| z.mode).collect()),
                }
            })
            .collect();

        let mut windows: Vec<WindowState> = self
            .windows
            .iter()
            .filter(|(window, _)| window.exists())
            .map(|(&window, tracked)| {
                let handle = monitor::handle_for_window(window);
                let workspace = self.workspace_of(window);
                WindowState {
                    hwnd: window.id(),
                    exe: tracked.exe.clone(),
                    class: tracked.class.clone(),
                    title: tracked.title.clone(),
                    rect: window.rect(),
                    show: if window.is_minimized() {
                        ShowState::Minimized
                    } else if window.is_maximized() {
                        ShowState::Maximized
                    } else {
                        ShowState::Normal
                    },
                    resizable: window.is_resizable(),
                    tiled: workspace.is_some(),
                    zone: workspace
                        .and_then(|key| self.workspaces[&key].zone_of(window.id()))
                        .map(|z| z as u8 + 1),
                    monitor: self
                        .monitors
                        .iter()
                        .find(|m| m.handle == handle)
                        .map(|m| m.id.clone()),
                    desktop: self
                        .desktops
                        .as_ref()
                        .and_then(|d| d.window_desktop(window)),
                    on_other_desktop: self.is_on_other_desktop(window),
                    rule_float: tracked.rule.is_some_and(|r| r.float),
                    rule_zone: tracked.rule.and_then(|r| r.zone),
                    scratchpad: self
                        .scratchpad_of(window)
                        .map(|i| self.scratch[i].name.clone()),
                }
            })
            .collect();
        windows.sort_by(|a, b| {
            (&a.monitor, a.rect.x, a.rect.y, a.hwnd).cmp(&(&b.monitor, b.rect.x, b.rect.y, b.hwnd))
        });

        StateSnapshot {
            monitors,
            windows,
            foreground: self.focused().map(Window::id),
            current_desktop,
            desktops: if self.desktop_control {
                vdesk::desktop_ids().unwrap_or_default()
            } else {
                Vec::new()
            },
        }
    }
}

fn props(tracked: &Tracked) -> WindowProps<'_> {
    WindowProps {
        exe: &tracked.exe,
        class: &tracked.class,
        title: &tracked.title,
    }
}

/// Starts a program detached from Tessera.
fn spawn(program: &str, args: &[String]) -> Result<(), String> {
    std::process::Command::new(program)
        .args(args)
        .spawn()
        .map(drop)
        .map_err(|e| format!("could not start `{program}`: {e}"))
}

/// Can a scene start the program at `path` directly? Store apps can only be
/// started through the shell, and some windows belong to a host process.
fn can_launch(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    !lower.is_empty()
        && !lower.contains("\\windowsapps\\")
        && !lower.ends_with("\\applicationframehost.exe")
        && !lower.ends_with("\\explorer.exe")
}

/// Stack windows move down below their tab bar.
fn with_stack_offset(placement: &Placement, (bar, spacing): (i32, i32)) -> Rect {
    let r = placement.rect;
    if placement.stacked {
        Rect::new(r.x, r.y + bar + spacing, r.width, r.height - bar - spacing)
    } else {
        r
    }
}

fn restore(window: Window, original: Original) {
    position::set_rect(window, original.rect);
    if original.maximized {
        position::maximize(window);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TILE: Rect = Rect::new(0, 0, 800, 600);
    const OTHER: Rect = Rect::new(800, 0, 800, 600);

    /// Sequence of an application that moves itself again after every
    /// correction: it used to be reset endlessly every 200 ms.
    #[test]
    fn drift_corrections_are_limited_per_target() {
        let mut drift = None;
        for _ in 0..MAX_DRIFT_CORRECTIONS {
            assert!(Drift::allows(drift, TILE));
            drift = Some(Drift::corrected(drift, TILE));
            Drift::arrived(&mut drift, TILE);
        }
        assert!(!Drift::allows(drift, TILE));
        // A new target (e.g. after a layout change) starts a new count.
        assert!(Drift::allows(drift, OTHER));
        assert_eq!(Drift::corrected(drift, OTHER).corrections, 1);
    }

    #[test]
    fn drift_not_repeated_when_target_unreachable() {
        // Minimum size or similar: the window does not arrive after the correction.
        let drift = Some(Drift::corrected(None, TILE));
        assert!(!Drift::allows(drift, TILE));
        // Arrival at a different target does not count.
        let mut drift = drift;
        Drift::arrived(&mut drift, OTHER);
        assert!(!Drift::allows(drift, TILE));
        Drift::arrived(&mut drift, TILE);
        assert!(Drift::allows(drift, TILE));
    }
}
