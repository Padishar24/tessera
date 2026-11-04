//! The shortcuts window: every action with its shortcuts, also the ones
//! without a shortcut, and editing them in `config.toml`.

use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use eframe::egui::{self, Color32, RichText};
use tessera_core::action::{Action, Group};
use tessera_core::config::Config;
use tessera_core::keys::{KeyCombo, windows_function};
use tessera_core::layout::DEFAULT_LAYOUTS;
use tessera_core::shortcuts::{Binding, Entry, Invalid, Overview, Source};
use tessera_win::hotkeys::key_exists;
use tessera_win::recorder::{Recorded, Recorder};

use crate::config_store;

/// Title of the window, also used to find it when it is opened again.
pub const TITLE: &str = "Tessera – keyboard shortcuts";
/// As long as the recording hook captures the keyboard.
const RECORD_TIME: Duration = Duration::from_secs(10);
/// How often `config.toml` is checked for changes made elsewhere.
const FILE_CHECK: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Filter {
    All,
    Active,
    Unbound,
}

struct Recording {
    recorder: Recorder,
    until: Instant,
}

/// A shortcut being added or changed.
struct Edit {
    action: Action,
    /// The binding being changed; `None` adds one.
    original: Option<Binding>,
    input: String,
    recording: Option<Recording>,
}

struct Status {
    text: String,
    error: bool,
}

pub struct ShortcutsApp {
    config_path: PathBuf,
    layouts_path: PathBuf,
    /// Content of `config.toml` and when it was last changed.
    text: String,
    modified: Option<SystemTime>,
    checked: Instant,
    /// `Err` if `config.toml` cannot be read as a configuration at all.
    overview: Result<Overview, String>,
    search: String,
    filter: Filter,
    /// "Find by keys": only the action of this combination is shown.
    found: Option<KeyCombo>,
    finding: Option<Recording>,
    edit: Option<Edit>,
    status: Option<Status>,
}

/// What a click in the list asks for; done after drawing.
enum Command {
    Edit(Action, Option<Binding>),
    Restore(Action, KeyCombo),
    Remove(Source),
}

impl ShortcutsApp {
    pub fn new(config_path: PathBuf, layouts_path: PathBuf) -> Self {
        let mut app = Self {
            config_path,
            layouts_path,
            text: String::new(),
            modified: None,
            checked: Instant::now(),
            overview: Ok(Overview::default()),
            search: String::new(),
            filter: Filter::All,
            found: None,
            finding: None,
            edit: None,
            status: None,
        };
        app.reload();
        app
    }

    fn reload(&mut self) {
        self.modified = file_modified(&self.config_path);
        match std::fs::read_to_string(&self.config_path) {
            Ok(text) => {
                self.overview = toml::from_str::<Config>(&text)
                    .map(|config| Overview::new(&config))
                    .map_err(|e| format!("config.toml cannot be read: {e}"));
                self.text = text;
            }
            Err(e) => {
                self.overview = Err(format!("{}: {e}", self.config_path.display()));
            }
        }
    }

    /// Reloads `config.toml` if it was changed elsewhere.
    fn check_file(&mut self) {
        if self.checked.elapsed() < FILE_CHECK {
            return;
        }
        self.checked = Instant::now();
        if file_modified(&self.config_path) != self.modified {
            self.reload();
        }
    }

    fn set_status(&mut self, text: impl Into<String>, error: bool) {
        self.status = Some(Status {
            text: text.into(),
            error,
        });
    }

    /// Applies a change to the current file content and writes it.
    fn change(
        &mut self,
        done: &str,
        change: impl FnOnce(&str, &str) -> anyhow::Result<String>,
    ) -> bool {
        // Start from what is on disk, so that changes made elsewhere stay.
        self.reload();
        let layouts = std::fs::read_to_string(&self.layouts_path)
            .unwrap_or_else(|_| DEFAULT_LAYOUTS.to_owned());
        let result = change(&self.text, &layouts).and_then(|text| {
            std::fs::write(&self.config_path, &text)?;
            Ok(())
        });
        match result {
            Ok(()) => {
                self.reload();
                self.set_status(format!("{done} Saved to config.toml."), false);
                true
            }
            Err(e) => {
                self.set_status(format!("{e:#}"), true);
                false
            }
        }
    }

    fn save_edit(&mut self) {
        let Some(edit) = &self.edit else {
            return;
        };
        let combo = match edit.input.trim().parse::<KeyCombo>() {
            Ok(combo) => combo,
            Err(e) => {
                self.set_status(format!("{e}"), true);
                return;
            }
        };
        let action = edit.action;
        let replace = edit.original.as_ref().map(|b| b.source.clone());
        let done = format!("{} now runs “{}”.", combo.display_name(), action.label());
        if self.change(&done, |text, layouts| {
            config_store::bind(text, layouts, combo, action, replace.as_ref())
        }) {
            self.edit = None;
        }
    }

    fn run(&mut self, command: Command) {
        match command {
            Command::Edit(action, original) => {
                let input = original
                    .as_ref()
                    .map(|b| b.combo.to_string())
                    .unwrap_or_default();
                self.edit = Some(Edit {
                    action,
                    original,
                    input,
                    recording: None,
                });
                self.status = None;
            }
            Command::Restore(action, combo) => {
                let done = format!("{} restored.", combo.display_name());
                self.change(&done, |text, layouts| {
                    config_store::bind(text, layouts, combo, action, None)
                });
            }
            Command::Remove(source) => {
                self.edit = None;
                self.change("Shortcut removed.", |text, layouts| {
                    config_store::unbind(text, layouts, &source)
                });
            }
        }
    }

    /// Handles key presses of a running recording.
    fn poll_recordings(&mut self, ctx: &egui::Context) {
        if let Some(edit) = &mut self.edit
            && let Some(recording) = &edit.recording
        {
            match recording.recorder.poll() {
                Some(Recorded::Combo(combo)) => {
                    edit.input = combo.to_string();
                    edit.recording = None;
                }
                Some(Recorded::Cancelled) => edit.recording = None,
                None if Instant::now() >= recording.until => edit.recording = None,
                None => ctx.request_repaint_after(Duration::from_millis(200)),
            }
        }
        if let Some(recording) = &self.finding {
            match recording.recorder.poll() {
                Some(Recorded::Combo(combo)) => {
                    self.finding = None;
                    self.found = Some(combo);
                    self.search.clear();
                }
                Some(Recorded::Cancelled) => self.finding = None,
                None if Instant::now() >= recording.until => self.finding = None,
                None => ctx.request_repaint_after(Duration::from_millis(200)),
            }
        }
    }

    /// Ends any running recording first: the keyboard hook is shared by the
    /// whole process, so only one recording can run at a time.
    fn start_recording(&mut self, ctx: &egui::Context) -> Option<Recording> {
        self.finding = None;
        if let Some(edit) = &mut self.edit {
            edit.recording = None;
        }
        let ctx = ctx.clone();
        match Recorder::start(move || ctx.request_repaint()) {
            Ok(recorder) => Some(Recording {
                recorder,
                until: Instant::now() + RECORD_TIME,
            }),
            Err(e) => {
                self.set_status(format!("Keys cannot be recorded: {e:#}"), true);
                None
            }
        }
    }

    fn visible(&self, entry: &Entry) -> bool {
        if let Some(found) = self.found {
            return entry.bindings.iter().any(|b| b.combo == found);
        }
        let shown = match self.filter {
            Filter::All => true,
            Filter::Active => !entry.bindings.is_empty(),
            Filter::Unbound => entry.bindings.is_empty(),
        };
        let search = self.search.trim().to_lowercase();
        shown
            && (search.is_empty()
                || entry.label.to_lowercase().contains(&search)
                || entry.action.to_string().contains(&search)
                || entry.bindings.iter().any(|b| {
                    b.combo.to_string().contains(&search)
                        || b.combo.display_name().to_lowercase().contains(&search)
                }))
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text("Search actions or keys, e.g. zone or win+alt+k")
                    .desired_width(300.0),
            );
            if !self.search.is_empty() {
                self.found = None;
            }
            for (filter, text) in [
                (Filter::All, "All"),
                (Filter::Active, "Active"),
                (Filter::Unbound, "Not bound"),
            ] {
                if ui.selectable_label(self.filter == filter, text).clicked() {
                    self.filter = filter;
                }
            }
            ui.separator();
            if self.finding.is_some() {
                ui.label(RichText::new("Press the keys… (Esc cancels)").color(warning(ui)));
            } else if ui
                .button("Find by keys")
                .on_hover_text("Press a combination to see what it does")
                .clicked()
            {
                self.finding = self.start_recording(ui.ctx());
            }
        });
        if let Some(found) = self.found {
            ui.horizontal(|ui| {
                ui.label(format!("Showing {}", found.display_name()));
                if ui.small_button("Show all").clicked() {
                    self.found = None;
                }
            });
            if let Ok(overview) = &self.overview
                && overview.find(found).is_none()
            {
                let mut text = format!("{} is not bound in Tessera.", found.display_name());
                if let Some(function) = windows_function(found) {
                    text.push_str(&format!(" Windows uses it for: {function}."));
                }
                ui.label(text);
            }
        }
    }

    fn invalid_list(ui: &mut egui::Ui, invalid: &[Invalid], commands: &mut Vec<Command>) {
        if invalid.is_empty() {
            return;
        }
        ui.label(RichText::new("These shortcuts in config.toml cannot be used:").color(error(ui)));
        for item in invalid {
            ui.horizontal(|ui| {
                ui.label(format!("{}: {}", item.place, item.error));
                if ui.small_button("Remove").clicked() {
                    commands.push(Command::Remove(item.source.clone()));
                }
            });
        }
        ui.separator();
    }

    fn list(&mut self, ui: &mut egui::Ui) -> Vec<Command> {
        let mut commands = Vec::new();
        let overview = match &self.overview {
            Ok(overview) => overview.clone(),
            Err(e) => {
                ui.colored_label(error(ui), e);
                return commands;
            }
        };
        Self::invalid_list(ui, &overview.invalid, &mut commands);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for group in Group::ALL {
                    let entries: Vec<&Entry> = overview
                        .entries
                        .iter()
                        .filter(|e| e.action.group() == group && self.visible(e))
                        .collect();
                    if entries.is_empty() {
                        continue;
                    }
                    ui.add_space(8.0);
                    ui.heading(group.title());
                    egui::Grid::new(group.title())
                        .num_columns(2)
                        .striped(true)
                        .min_col_width(240.0)
                        .spacing([16.0, 6.0])
                        .show(ui, |ui| {
                            for entry in entries {
                                self.row(ui, &overview, entry, &mut commands);
                                ui.end_row();
                            }
                        });
                }
            });
        commands
    }

    fn row(
        &mut self,
        ui: &mut egui::Ui,
        overview: &Overview,
        entry: &Entry,
        commands: &mut Vec<Command>,
    ) {
        ui.label(&entry.label)
            .on_hover_text(entry.action.to_string());
        let editing = self
            .edit
            .as_ref()
            .is_some_and(|edit| edit.action == entry.action);
        ui.vertical(|ui| {
            ui.horizontal_wrapped(|ui| {
                if entry.bindings.is_empty() {
                    ui.weak("Not bound");
                }
                for binding in &entry.bindings {
                    let mut chip =
                        ui.button(RichText::new(binding.combo.display_name()).monospace());
                    let mut hover = "Click to change".to_owned();
                    if let Some(function) = windows_function(binding.combo) {
                        hover.push_str(&format!("\nOverrides the Windows shortcut: {function}"));
                    }
                    chip = chip.on_hover_text(hover);
                    if chip.clicked() {
                        commands.push(Command::Edit(entry.action, Some(binding.clone())));
                    }
                    if windows_function(binding.combo).is_some() {
                        ui.colored_label(warning(ui), "⚠");
                    }
                    match &binding.source {
                        Source::ExplorerTabs => {
                            ui.weak("(from explorer_tabs)");
                        }
                        Source::Scratchpad(_) => {
                            ui.weak("(scratchpad key)");
                        }
                        Source::Keybindings(_) => {}
                    }
                    if !key_exists(binding.combo.key) {
                        ui.colored_label(warning(ui), "key not on this keyboard layout");
                    }
                }
                if !editing
                    && ui
                        .small_button("+")
                        .on_hover_text("Add a shortcut")
                        .clicked()
                {
                    commands.push(Command::Edit(entry.action, None));
                }
            });
            for combo in &entry.removed_defaults {
                ui.horizontal(|ui| {
                    ui.weak(format!("Default {} removed", combo.display_name()));
                    if ui.small_button("Restore").clicked() {
                        commands.push(Command::Restore(entry.action, *combo));
                    }
                });
            }
            if editing {
                self.edit_row(ui, overview, commands);
            }
        });
    }

    fn edit_row(&mut self, ui: &mut egui::Ui, overview: &Overview, commands: &mut Vec<Command>) {
        let ctx = ui.ctx().clone();
        let mut start_recording = false;
        let mut save = false;
        let mut cancel = false;
        let Some(edit) = &mut self.edit else {
            return;
        };
        ui.horizontal(|ui| {
            match &edit.recording {
                Some(recording) => {
                    let left = recording.until.saturating_duration_since(Instant::now());
                    ui.label(
                        RichText::new(format!("Press the keys… {} s", left.as_secs() + 1))
                            .color(warning(ui)),
                    );
                }
                None => {
                    start_recording = ui
                        .button("Press the keys")
                        .on_hover_text("Records the next combination, also ones Windows uses")
                        .clicked();
                }
            }
            ui.label("or type");
            let response = ui.add(
                egui::TextEdit::singleline(&mut edit.input)
                    .hint_text("win+alt+k")
                    .font(egui::TextStyle::Monospace)
                    .desired_width(180.0),
            );
            if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                save = true;
            }
            save |= ui.button("Save").clicked();
            if let Some(original) = &edit.original
                && ui.button("Remove").clicked()
            {
                commands.push(Command::Remove(original.source.clone()));
            }
            cancel = ui.button("Cancel").clicked();
        });
        // What saving would do.
        if !edit.input.trim().is_empty() {
            match edit.input.trim().parse::<KeyCombo>() {
                Err(e) => {
                    ui.colored_label(error(ui), e.to_string());
                }
                Ok(combo) => {
                    let original = edit.original.as_ref().map(|b| &b.source);
                    if let Some((other, binding)) = overview.find(combo)
                        && Some(&binding.source) != original
                    {
                        let text = if other.action == edit.action {
                            format!("{} already runs this.", combo.display_name())
                        } else {
                            format!(
                                "{} already runs “{}”. Saving moves it here.",
                                combo.display_name(),
                                other.label
                            )
                        };
                        ui.colored_label(warning(ui), text);
                    }
                    if let Some(function) = windows_function(combo) {
                        ui.colored_label(
                            warning(ui),
                            format!("⚠ Overrides the Windows shortcut: {function}"),
                        );
                    }
                    if !key_exists(combo.key) {
                        ui.colored_label(
                            warning(ui),
                            "This key is not on the current keyboard layout.",
                        );
                    }
                }
            }
        }
        if start_recording {
            edit.recording = None;
            let recording = self.start_recording(&ctx);
            if let Some(edit) = &mut self.edit {
                edit.recording = recording;
            }
        }
        if cancel {
            self.edit = None;
        } else if save {
            self.save_edit();
        }
    }
}

impl eframe::App for ShortcutsApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.check_file();
        ctx.request_repaint_after(FILE_CHECK);
        self.poll_recordings(&ctx);
        if self.edit.as_ref().is_some_and(|e| e.recording.is_none())
            && self.finding.is_none()
            && ctx.input(|i| i.key_pressed(egui::Key::Escape))
        {
            self.edit = None;
        }

        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.add_space(6.0);
            self.toolbar(ui);
            ui.add_space(6.0);
        });
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.add_space(4.0);
            match &self.status {
                Some(status) => {
                    ui.colored_label(if status.error { error(ui) } else { ok(ui) }, &status.text);
                }
                None => {
                    if let Ok(overview) = &self.overview {
                        ui.weak(format!(
                            "{} shortcuts · {} actions not bound · Changes are saved to \
                             config.toml and work right away while Tessera runs.",
                            overview.binding_count(),
                            overview.unbound_count()
                        ));
                    }
                }
            }
            ui.add_space(4.0);
        });
        let commands = egui::CentralPanel::default()
            .show(ui, |ui| self.list(ui))
            .inner;
        for command in commands {
            self.run(command);
        }
    }
}

// Colors that work with the light and the dark theme (the window follows
// the Windows setting).
fn warning(ui: &egui::Ui) -> Color32 {
    ui.visuals().warn_fg_color
}

fn error(ui: &egui::Ui) -> Color32 {
    ui.visuals().error_fg_color
}

fn ok(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(140, 220, 140)
    } else {
        Color32::from_rgb(30, 120, 40)
    }
}

fn file_modified(path: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}
