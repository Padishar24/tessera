//! The editor UI: a borderless, transparent window over a monitor's work
//! area, showing the layout's zones at actual size.

use std::path::PathBuf;
use std::time::Duration;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, FontId, Key, Pos2, Sense, Stroke, StrokeKind,
    Vec2,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use tessera_core::config::Color;
use tessera_core::geometry::Rect;
use tessera_core::ipc::{Request, Response};
use tessera_core::layout::{Layouts, SplitSpec, Template, template_spec};
use tessera_core::layout_edit::Path;
use tessera_core::tree::Orientation;
use tessera_win::position;

use crate::store;

/// How often the window is readjusted until it sits on the work area
/// (Windows may move it when switching monitors because of a different DPI).
const PLACEMENT_FRAMES: u32 = 30;

pub struct Target {
    pub monitor_id: String,
    pub monitor_name: String,
    /// Monitor work area = window area (physical pixels).
    pub work_area: Rect,
    /// Work area minus outer gap.
    pub area: Rect,
    pub gap: i32,
}

pub struct Style {
    pub zone: Color,
    pub highlight: Color,
    pub opacity: f32,
}

/// Which layout a monitor is currently using (according to the running daemon).
pub struct MonitorUse {
    pub monitor_id: String,
    pub monitor_name: String,
    pub layout: String,
}

struct DividerDrag {
    path: Path,
    index: usize,
    /// Fractions of a pixel not yet applied.
    remainder: f32,
}

struct Status {
    text: String,
    error: bool,
}

pub struct EditorApp {
    layouts_path: PathBuf,
    /// Current content of `layouts.toml`.
    text: String,
    /// All layouts in the file: name and whether it is an (editable) split.
    names: Vec<(String, bool)>,
    /// Name of the loaded layout in the file (`None` = new layout).
    original: Option<String>,
    name: String,
    /// Normalized split tree.
    spec: SplitSpec,
    /// All monitors; `targets[current]` is being edited.
    targets: Vec<Target>,
    current: usize,
    style: Style,
    /// Layouts of all monitors; empty if the daemon is not running.
    usage: Vec<MonitorUse>,
    template_count: usize,
    drag: Option<DividerDrag>,
    status: Option<Status>,
    placement_frames: u32,
}

/// Everything the editor starts with.
pub struct Setup {
    pub layouts_path: PathBuf,
    /// Content of `layouts.toml`.
    pub text: String,
    /// All monitors; must not be empty.
    pub targets: Vec<Target>,
    /// Index of the initial monitor in `targets`.
    pub current: usize,
    pub style: Style,
    pub usage: Vec<MonitorUse>,
    /// Layout of the initial monitor, in case the daemon cannot report it.
    pub initial: Option<String>,
}

impl EditorApp {
    pub fn new(cc: &eframe::CreationContext<'_>, setup: Setup) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let mut app = Self {
            layouts_path: setup.layouts_path,
            text: setup.text,
            names: Vec::new(),
            original: None,
            name: String::new(),
            spec: SplitSpec::default(),
            targets: setup.targets,
            current: setup.current,
            style: setup.style,
            usage: setup.usage,
            template_count: 3,
            drag: None,
            status: None,
            placement_frames: 0,
        };
        app.refresh_names();
        app.open_monitor(setup.current, setup.initial);
        app
    }

    fn target(&self) -> &Target {
        &self.targets[self.current]
    }

    /// Switches to monitor `index`: moves the window there and loads its
    /// current layout. Unsaved changes are lost.
    fn open_monitor(&mut self, index: usize, fallback: Option<String>) {
        self.current = index.min(self.targets.len() - 1);
        self.placement_frames = 0;
        self.drag = None;
        let id = self.target().monitor_id.clone();
        let editable = |name: &String| self.names.iter().any(|(n, e)| n == name && *e);
        let start = self
            .usage
            .iter()
            .find(|u| u.monitor_id == id)
            .map(|u| u.layout.clone())
            .or(fallback)
            .filter(editable)
            .or_else(|| {
                self.names
                    .iter()
                    .find(|(_, editable)| *editable)
                    .map(|(n, _)| n.clone())
            });
        let Some(name) = start else {
            self.new_layout();
            return;
        };
        self.load(&name);
        // Layouts are shared definitions: if other monitors use the same
        // layout, saving would change them too. So start right away with a
        // copy for this monitor only.
        let others = self.shared_with(&name);
        if !others.is_empty() {
            self.fork_for_monitor();
            self.set_status(
                format!(
                    "`{name}` is also used on {}. You are editing a copy for this \
                     monitor only – \"Save & apply\" puts it in place here.",
                    others.join(", ")
                ),
                false,
            );
        }
    }

    /// Names of the other monitors currently using `layout`.
    fn shared_with(&self, layout: &str) -> Vec<String> {
        self.usage
            .iter()
            .filter(|u| u.monitor_id != self.target().monitor_id && u.layout == layout)
            .map(|u| u.monitor_name.clone())
            .collect()
    }

    /// Turns the loaded layout into a new one with its own name for this
    /// monitor (e.g. `single-dell-u2412m`); the zones are kept.
    fn fork_for_monitor(&mut self) {
        let base = self.original.clone().unwrap_or_else(|| self.name.clone());
        self.original = None;
        self.name = self.unique_name(&format!("{base}-{}", slug(&self.target().monitor_name)));
    }

    /// `base`, with a number appended if needed, so that the name is not taken yet.
    fn unique_name(&self, base: &str) -> String {
        let taken = |name: &str| self.names.iter().any(|(existing, _)| existing == name);
        let mut name = base.to_owned();
        let mut n = 2;
        while taken(&name) {
            name = format!("{base}-{n}");
            n += 1;
        }
        name
    }

    fn refresh_names(&mut self) {
        self.names = Layouts::parse(&self.text)
            .map(|layouts| {
                layouts
                    .iter()
                    .map(|l| (l.name.clone(), l.split().is_some()))
                    .collect()
            })
            .unwrap_or_default();
    }

    fn load(&mut self, name: &str) {
        let Ok(layouts) = Layouts::parse(&self.text) else {
            return;
        };
        if let Some(spec) = layouts.get(name).and_then(|l| l.split()) {
            self.spec = spec.normalized();
            self.original = Some(name.to_owned());
            self.name = name.to_owned();
            self.status = None;
        }
    }

    fn new_layout(&mut self) {
        self.original = None;
        self.name = self.unique_name("new-layout");
        self.spec = SplitSpec::default();
        self.status = None;
    }

    fn set_status(&mut self, text: impl Into<String>, error: bool) {
        self.status = Some(Status {
            text: text.into(),
            error,
        });
    }

    fn save(&mut self, apply: bool) {
        let result =
            store::upsert_layout(&self.text, self.original.as_deref(), &self.name, &self.spec)
                .and_then(|text| {
                    std::fs::write(&self.layouts_path, &text)?;
                    Ok(text)
                });
        match result {
            Ok(text) => {
                self.text = text;
                self.name = self.name.trim().to_owned();
                self.original = Some(self.name.clone());
                self.refresh_names();
                if apply {
                    match self.apply_in_daemon() {
                        Ok(()) => {
                            let (id, name) = (self.target().monitor_id.clone(), self.name.clone());
                            if let Some(own) = self.usage.iter_mut().find(|u| u.monitor_id == id) {
                                own.layout = name;
                            }
                            self.set_status("Saved and applied to the monitor.", false)
                        }
                        Err(e) => self.set_status(format!("Saved, but not applied: {e}"), true),
                    }
                } else {
                    self.set_status("Saved.", false);
                }
            }
            Err(e) => self.set_status(format!("{e:#}"), true),
        }
    }

    fn delete(&mut self) {
        let Some(original) = self.original.clone() else {
            return;
        };
        let result = store::remove_layout(&self.text, &original).and_then(|text| {
            std::fs::write(&self.layouts_path, &text)?;
            Ok(text)
        });
        match result {
            Ok(text) => {
                self.text = text;
                self.refresh_names();
                match self.names.iter().find(|(_, editable)| *editable).cloned() {
                    Some((name, _)) => self.load(&name),
                    None => self.new_layout(),
                }
                self.set_status(format!("Layout `{original}` deleted."), false);
            }
            Err(e) => self.set_status(format!("{e:#}"), true),
        }
    }

    /// The daemon reloads the layouts and sets the saved one on the monitor.
    fn apply_in_daemon(&self) -> Result<(), String> {
        send(&Request::Action {
            action: "reload-config".into(),
        })?;
        send(&Request::SetLayout {
            monitor: self.target().monitor_id.clone(),
            layout: self.name.clone(),
        })
    }

    fn template(&mut self, template: Template) {
        self.spec = template_spec(template, self.template_count).normalized();
    }

    /// Places the window exactly on the work area (physical pixels).
    fn place_window(&mut self, frame: &eframe::Frame) {
        if self.placement_frames >= PLACEMENT_FRAMES {
            return;
        }
        self.placement_frames += 1;
        let Ok(handle) = frame.window_handle() else {
            return;
        };
        if let RawWindowHandle::Win32(win32) = handle.as_raw() {
            let hwnd = win32.hwnd.get();
            if position::raw_window_rect(hwnd) != self.target().work_area {
                position::set_raw_window_rect(hwnd, self.target().work_area);
            }
        }
    }
}

impl eframe::App for EditorApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.place_window(frame);
        let ctx = ui.ctx().clone();
        if self.placement_frames < PLACEMENT_FRAMES {
            ctx.request_repaint_after(Duration::from_millis(30));
        }
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        let ppp = ctx.pixels_per_point();
        let origin = (self.target().work_area.x, self.target().work_area.y);
        let to_rect = |r: Rect| {
            egui::Rect::from_min_size(
                Pos2::new((r.x - origin.0) as f32 / ppp, (r.y - origin.1) as f32 / ppp),
                Vec2::new(r.width as f32 / ppp, r.height as f32 / ppp),
            )
        };
        let to_physical = |p: Pos2| ((p.x * ppp) as i32 + origin.0, (p.y * ppp) as i32 + origin.1);

        let full = ui.max_rect();
        ui.painter()
            .rect_filled(full, CornerRadius::ZERO, Color32::from_black_alpha(110));
        let background = ui.interact(full, ui.id().with("zones"), Sense::click());

        // Determine dividers first: they lie on top of the zones.
        let area = self.target().area;
        let gap = self.target().gap;
        let dividers = self.spec.dividers(area, gap);
        let mut over_divider = self.drag.is_some();
        let mut moved = None;
        for (i, divider) in dividers.iter().enumerate() {
            let mut rect = to_rect(divider.rect);
            // Grabbable at least 12 points wide, even without a gap.
            let grow = match divider.orientation {
                Orientation::Horizontal => Vec2::new(((12.0 - rect.width()) / 2.0).max(0.0), 0.0),
                Orientation::Vertical => Vec2::new(0.0, ((12.0 - rect.height()) / 2.0).max(0.0)),
            };
            rect = rect.expand2(grow);
            let response = ui.interact(rect, ui.id().with(("divider", i)), Sense::drag());
            if response.hovered() || response.dragged() {
                over_divider = true;
                ctx.set_cursor_icon(match divider.orientation {
                    Orientation::Horizontal => CursorIcon::ResizeHorizontal,
                    Orientation::Vertical => CursorIcon::ResizeVertical,
                });
                ui.painter().rect_filled(
                    rect,
                    CornerRadius::same(3),
                    color(self.style.highlight, 0.9),
                );
            }
            if response.drag_started() {
                self.drag = Some(DividerDrag {
                    path: divider.path.clone(),
                    index: divider.index,
                    remainder: 0.0,
                });
            }
            if response.dragged()
                && let Some(drag) = &mut self.drag
            {
                let delta = response.drag_delta();
                let along = match divider.orientation {
                    Orientation::Horizontal => delta.x,
                    Orientation::Vertical => delta.y,
                } * ppp
                    + drag.remainder;
                let pixels = along.round() as i32;
                drag.remainder = along - pixels as f32;
                if pixels != 0 {
                    moved = Some((drag.path.clone(), drag.index, pixels));
                }
            }
            if response.drag_stopped() {
                self.drag = None;
            }
        }
        if let Some((path, index, pixels)) = moved {
            self.spec.move_divider(&path, index, pixels, area, gap);
        }

        let zones = self.spec.zones_with_paths(area, gap);
        let hover = background.hover_pos().filter(|_| !over_divider);
        let shift = ctx.input(|i| i.modifiers.shift);
        let numbers = FontId::proportional(48.0);
        let small = FontId::proportional(14.0);
        let mut hovered_zone: Option<(Path, Rect)> = None;
        for (n, (path, zone)) in zones.iter().enumerate() {
            let rect = to_rect(*zone);
            let hovered = hover.is_some_and(|p| rect.contains(p));
            let fill = if hovered {
                color(self.style.highlight, self.style.opacity)
            } else {
                color(self.style.zone, self.style.opacity.max(0.55))
            };
            let painter = ui.painter();
            painter.rect_filled(rect, CornerRadius::same(8), fill);
            painter.rect_stroke(
                rect,
                CornerRadius::same(8),
                Stroke::new(2.0, color(self.style.highlight, 0.9)),
                StrokeKind::Inside,
            );
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                (n + 1).to_string(),
                numbers.clone(),
                Color32::WHITE,
            );
            painter.text(
                rect.center() + Vec2::new(0.0, 40.0),
                Align2::CENTER_CENTER,
                format!("{} × {}", zone.width, zone.height),
                small.clone(),
                Color32::from_white_alpha(180),
            );
            if hovered {
                hovered_zone = Some((path.clone(), *zone));
                // Preview of the split line.
                if let Some(p) = hover {
                    let line = Stroke::new(2.0, Color32::WHITE);
                    if shift {
                        painter.hline(rect.x_range(), p.y, line);
                    } else {
                        painter.vline(p.x, rect.y_range(), line);
                    }
                }
            }
        }

        if let Some((path, zone)) = hovered_zone {
            if background.clicked()
                && let Some(p) = background.interact_pointer_pos()
            {
                let (x, y) = to_physical(p);
                let (orientation, fraction) = if shift {
                    (
                        Orientation::Vertical,
                        (y - zone.y) as f32 / zone.height as f32,
                    )
                } else {
                    (
                        Orientation::Horizontal,
                        (x - zone.x) as f32 / zone.width as f32,
                    )
                };
                self.spec.split_zone(&path, orientation, fraction);
            }
            if background.secondary_clicked() && !self.spec.remove_zone(&path) {
                self.set_status("The last zone cannot be removed.", true);
            }
        }

        self.panel(&ctx);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0; 4]
    }
}

impl EditorApp {
    fn panel(&mut self, ctx: &egui::Context) {
        egui::Window::new("Zone editor")
            .anchor(Align2::CENTER_TOP, Vec2::new(0.0, 16.0))
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Monitor:");
                    let mut switch = None;
                    egui::ComboBox::from_id_salt("monitor")
                        .selected_text(monitor_label(self.target()))
                        .show_ui(ui, |ui| {
                            for (i, target) in self.targets.iter().enumerate() {
                                if ui
                                    .add(egui::Button::selectable(
                                        i == self.current,
                                        monitor_label(target),
                                    ))
                                    .clicked()
                                    && i != self.current
                                {
                                    switch = Some(i);
                                }
                            }
                        });
                    if let Some(index) = switch {
                        self.open_monitor(index, None);
                    }
                    ui.label(format!(
                        "·  {} zones",
                        self.spec
                            .zones_with_paths(self.target().area, self.target().gap)
                            .len()
                    ));
                });
                ui.horizontal(|ui| {
                    ui.label("Layout:");
                    let selected = self.original.clone().unwrap_or_else(|| "(new)".into());
                    let mut load = None;
                    let mut create = false;
                    egui::ComboBox::from_id_salt("layout")
                        .selected_text(selected)
                        .show_ui(ui, |ui| {
                            for (name, editable) in &self.names {
                                let current = self.original.as_deref() == Some(name.as_str());
                                let response = ui
                                    .add_enabled(*editable, egui::Button::selectable(current, name))
                                    .on_disabled_hover_text(
                                        "Canvas layouts can only be edited in layouts.toml",
                                    );
                                if response.clicked() {
                                    load = Some(name.clone());
                                }
                            }
                            ui.separator();
                            create = ui.button("New layout").clicked();
                        });
                    if let Some(name) = load {
                        self.load(&name);
                    }
                    if create {
                        self.new_layout();
                    }
                    ui.label("Name:");
                    ui.text_edit_singleline(&mut self.name);
                });
                // A shared layout was chosen deliberately: warn clearly.
                if let Some(original) = self.original.clone() {
                    let others = self.shared_with(&original);
                    if !others.is_empty() {
                        ui.horizontal(|ui| {
                            ui.colored_label(
                                Color32::from_rgb(255, 190, 90),
                                format!(
                                    "Warning: `{original}` is also used on {} – \
                                     saving changes it there too.",
                                    others.join(", ")
                                ),
                            );
                            if ui.button("Copy for this monitor").clicked() {
                                self.fork_for_monitor();
                            }
                        });
                    }
                }
                ui.horizontal(|ui| {
                    ui.label("Template:");
                    if ui.button("Single zone").clicked() {
                        self.template(Template::Single);
                    }
                    if ui.button("Columns").clicked() {
                        self.template(Template::Columns);
                    }
                    if ui.button("Rows").clicked() {
                        self.template(Template::Rows);
                    }
                    if ui.button("Grid").clicked() {
                        self.template(Template::Grid);
                    }
                    if ui.button("Focus center").clicked() {
                        self.template(Template::PriorityGrid);
                    }
                    ui.add(egui::DragValue::new(&mut self.template_count).range(2..=9))
                        .on_hover_text("Number of zones for the template");
                });
                ui.small(
                    "Click: split vertically · Shift+click: split horizontally · \
                     Right-click: remove zone · Drag divider: resize · Esc: close",
                );
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() {
                        self.save(false);
                    }
                    if ui
                        .button("Save & apply")
                        .on_hover_text("Saves and sets the layout on this monitor")
                        .clicked()
                    {
                        self.save(true);
                    }
                    if self.original.is_some() && ui.button("Delete").clicked() {
                        self.delete();
                    }
                    if ui.button("Close").clicked() {
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
                if let Some(status) = &self.status {
                    let color = if status.error {
                        Color32::from_rgb(255, 120, 110)
                    } else {
                        Color32::from_rgb(140, 220, 140)
                    };
                    ui.colored_label(color, &status.text);
                }
            });
    }
}

/// Display name in the monitor picker, e.g. `DELL U2412M (1920×1200)`.
fn monitor_label(target: &Target) -> String {
    format!(
        "{} ({}×{})",
        target.monitor_name, target.work_area.width, target.work_area.height
    )
}

/// Monitor name as part of a layout name: `DELL U2412M` → `dell-u2412m`,
/// `\\.\DISPLAY1` → `display1`.
fn slug(text: &str) -> String {
    let lower = text.to_lowercase();
    let parts: Vec<&str> = lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|p| !p.is_empty())
        .collect();
    if parts.is_empty() {
        "monitor".to_owned()
    } else {
        parts.join("-")
    }
}

fn color(c: Color, alpha: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r, c.g, c.b, (alpha.clamp(0.0, 1.0) * 255.0) as u8)
}

/// A request to the running daemon.
fn send(request: &Request) -> Result<(), String> {
    match tessera_win::pipe::call(request, Duration::from_secs(5)) {
        Ok(Response::Ok) => Ok(()),
        Ok(Response::Error { message }) => Err(message),
        Ok(Response::State(_) | Response::Scenes { .. }) => {
            Err("unexpected response from the daemon".into())
        }
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::slug;

    #[test]
    fn slug_makes_monitor_names_usable_in_layout_names() {
        assert_eq!(slug("DELL U2412M"), "dell-u2412m");
        assert_eq!(slug(r"\.\DISPLAY1"), "display1");
        assert_eq!(slug("LG  ULTRAWIDE (2)"), "lg-ultrawide-2");
        assert_eq!(slug("ÄÖÜ"), "monitor");
    }
}
