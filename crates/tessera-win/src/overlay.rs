//! Zone overlay: translucent and click-through over the whole monitor.

use tessera_core::config::Color;
use tessera_core::geometry::Rect;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::DirectWrite::DWRITE_FONT_WEIGHT_SEMI_BOLD;
use windows::Win32::UI::WindowsAndMessaging::DefWindowProcW;
use windows::core::w;

use crate::layered::{LayeredWindow, Options};

/// Colors and opacity of the overlay.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style {
    pub zone: Color,
    pub highlight: Color,
    /// 0.0 – 1.0
    pub opacity: f32,
}

/// What gets drawn; all rectangles in screen coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct Scene {
    pub monitor: Rect,
    pub dpi: u32,
    pub zones: Vec<Rect>,
    pub highlight: Option<usize>,
    /// Where the dragged window would land.
    pub preview: Option<Rect>,
    pub style: Style,
}

pub struct ZoneOverlay {
    window: LayeredWindow,
    shown: Option<Scene>,
}

impl ZoneOverlay {
    pub fn new() -> anyhow::Result<Self> {
        Ok(Self {
            window: LayeredWindow::new(Options {
                class: w!("Tessera.Overlay"),
                title: w!("Tessera Zones"),
                click_through: true,
                topmost: true,
                wndproc: Some(wndproc),
                user_data: 0,
            })?,
            shown: None,
        })
    }

    pub fn is_visible(&self) -> bool {
        self.shown.is_some()
    }

    /// Draws the scene and shows the overlay (only on changes).
    pub fn show(&mut self, scene: Scene) -> anyhow::Result<()> {
        if self.shown.as_ref() == Some(&scene) {
            return Ok(());
        }
        let scale = scene.dpi as f32 / 96.0;
        let numbers = self
            .window
            .text_format(48.0 * scale, DWRITE_FONT_WEIGHT_SEMI_BOLD, true)?;
        let radius = 8.0 * scale;
        let style = scene.style;
        let white = Color::rgb(255, 255, 255);
        self.window.present(scene.monitor, |p| {
            for (i, zone) in scene.zones.iter().enumerate() {
                let fill = if scene.highlight == Some(i) {
                    style.highlight
                } else {
                    style.zone
                };
                p.fill(*zone, fill, style.opacity, radius)?;
                p.stroke(*zone, style.highlight, 0.8, 2.0 * scale, radius)?;
            }
            if let Some(preview) = scene.preview {
                p.fill(
                    preview,
                    style.highlight,
                    (style.opacity + 0.3).min(0.9),
                    radius,
                )?;
                p.stroke(preview, white, 0.9, 3.0 * scale, radius)?;
            }
            for (i, zone) in scene.zones.iter().enumerate() {
                p.text(&(i + 1).to_string(), *zone, &numbers, white, 0.9)?;
            }
            Ok(())
        })?;
        self.shown = Some(scene);
        Ok(())
    }

    pub fn hide(&mut self) {
        self.shown = None;
        self.window.hide();
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: forwarding to the default procedure.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}
