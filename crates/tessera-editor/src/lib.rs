//! Tessera's graphical zone editor. Runs as a separate process
//! (`tessera editor`) so that its event loop does not get in the way of
//! the daemon's.

mod app;
pub mod store;

use std::path::PathBuf;
use std::time::Duration;

use eframe::egui;
use tessera_core::config::LoadedConfig;
use tessera_core::ipc::{Request, Response};
use tessera_core::layout::DEFAULT_LAYOUTS;
use tessera_win::{monitor, position};

pub struct Options {
    pub config: LoadedConfig,
    pub layouts_path: PathBuf,
    /// Monitor ID; if omitted, the monitor under the mouse pointer.
    pub monitor: Option<String>,
    /// Layout to open first.
    pub layout: Option<String>,
}

pub fn run(options: Options) -> anyhow::Result<()> {
    let Some(_instance) = tessera_win::instance::SingleInstance::acquire("Tessera.Editor")? else {
        tracing::info!("zone editor is already open");
        return Ok(());
    };

    let monitors = monitor::enumerate();
    let (x, y) = position::cursor_pos();
    let current = options
        .monitor
        .as_ref()
        .and_then(|id| monitors.iter().position(|m| m.id == *id))
        .or_else(|| monitors.iter().position(|m| m.rect.contains_point(x, y)))
        .unwrap_or(0);
    let monitor = monitors
        .get(current)
        .ok_or_else(|| anyhow::anyhow!("no monitor found"))?;
    let gaps = options.config.config.gaps;
    let targets: Vec<app::Target> = monitors
        .iter()
        .map(|m| {
            let (area, gap) = gaps.usable_area(m.work_area, m.dpi);
            app::Target {
                monitor_id: m.id.clone(),
                monitor_name: m.name.clone(),
                work_area: m.work_area,
                area,
                gap,
            }
        })
        .collect();
    let appearance = &options.config.config.appearance;
    let style = app::Style {
        zone: appearance.zone_color,
        highlight: appearance.zone_highlight_color,
        opacity: appearance.zone_opacity,
    };
    let text = std::fs::read_to_string(&options.layouts_path)
        .unwrap_or_else(|_| DEFAULT_LAYOUTS.to_owned());
    tracing::info!(monitor = monitor.id, "zone editor opened");

    // Rough initial position in logical points; the app sets the exact
    // position in physical pixels in the first frame.
    let scale = monitor.dpi as f32 / 96.0;
    let work = monitor.work_area;
    let viewport = egui::ViewportBuilder::default()
        .with_title("Tessera – zone editor")
        .with_decorations(false)
        .with_transparent(true)
        .with_always_on_top()
        .with_position(egui::pos2(work.x as f32 / scale, work.y as f32 / scale))
        .with_inner_size(egui::vec2(
            work.width as f32 / scale,
            work.height as f32 / scale,
        ));
    let native = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    // Which layouts the monitors are currently using – only known if the daemon is running.
    let usage = match tessera_win::pipe::call(&Request::QueryState, Duration::from_secs(2)) {
        Ok(Response::State(state)) => state
            .monitors
            .into_iter()
            .map(|m| app::MonitorUse {
                monitor_id: m.id,
                monitor_name: m.name,
                layout: m.layout,
            })
            .collect(),
        _ => Vec::new(),
    };
    let setup = app::Setup {
        layouts_path: options.layouts_path,
        text,
        targets,
        current,
        style,
        usage,
        initial: options.layout,
    };
    eframe::run_native(
        "Tessera zone editor",
        native,
        Box::new(move |cc| Ok(Box::new(app::EditorApp::new(cc, setup)))),
    )
    .map_err(|e| anyhow::anyhow!("editor window: {e}"))
}
