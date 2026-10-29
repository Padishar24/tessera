//! Window hints: a badge in the middle of each window with the letters that
//! select it. One small topmost, click-through window per badge.

use tessera_core::config::Color;
use tessera_core::geometry::Rect;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::DirectWrite::{DWRITE_FONT_WEIGHT_BOLD, DWRITE_FONT_WEIGHT_NORMAL};
use windows::Win32::UI::WindowsAndMessaging::DefWindowProcW;
use windows::core::w;

use crate::layered::{LayeredWindow, Options};

/// A window to label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hint {
    /// Lowercase letters, see `tessera_core::hint::labels`.
    pub label: String,
    /// Shown below the letters to tell windows of the same program apart.
    pub title: String,
    /// Visible area of the window in screen coordinates.
    pub window: Rect,
    pub dpi: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HintStyle {
    pub background: Color,
    pub highlight: Color,
}

#[derive(Default)]
pub struct HintOverlay {
    /// Reused badge windows; only the first ones are shown.
    badges: Vec<LayeredWindow>,
}

impl HintOverlay {
    pub fn new() -> Self {
        Self::default()
    }

    /// Shows a badge for every hint whose label starts with `typed`, with
    /// the letters still to be typed.
    pub fn show(&mut self, hints: &[Hint], typed: &str, style: HintStyle) -> anyhow::Result<()> {
        let visible: Vec<&Hint> = hints
            .iter()
            .filter(|h| h.label.starts_with(typed))
            .collect();
        while self.badges.len() < visible.len() {
            self.badges.push(LayeredWindow::new(Options {
                class: w!("Tessera.Hint"),
                title: w!("Tessera Hint"),
                click_through: true,
                topmost: true,
                wndproc: Some(wndproc),
                user_data: 0,
            })?);
        }
        for (badge, hint) in self.badges.iter_mut().zip(&visible) {
            draw(badge, hint, &hint.label[typed.len()..], style)?;
        }
        for badge in self.badges.iter_mut().skip(visible.len()) {
            badge.hide();
        }
        Ok(())
    }

    pub fn hide(&mut self) {
        for badge in &mut self.badges {
            badge.hide();
        }
    }
}

fn draw(
    badge: &mut LayeredWindow,
    hint: &Hint,
    letters: &str,
    style: HintStyle,
) -> anyhow::Result<()> {
    let scale = hint.dpi as f32 / 96.0;
    let px = |v: f32| (v * scale).round() as i32;
    let big = badge.text_format(36.0 * scale, DWRITE_FONT_WEIGHT_BOLD, true)?;
    let small = badge.text_format(12.0 * scale, DWRITE_FONT_WEIGHT_NORMAL, false)?;

    let letters_height = px(52.0);
    let title_height = px(22.0);
    let padding = px(10.0);
    let width = px(220.0).min(hint.window.width.max(px(80.0)));
    let height = letters_height + title_height + padding;
    let (cx, cy) = hint.window.center();
    let rect = Rect::new(cx - width / 2, cy - height / 2, width, height);
    let radius = 10.0 * scale;
    let white = Color::rgb(255, 255, 255);
    let upper = letters.to_uppercase();

    badge.present(rect, |p| {
        p.fill(rect, style.background, 0.94, radius)?;
        p.stroke(rect, style.highlight, 1.0, 2.0 * scale, radius)?;
        let letters_rect = Rect::new(rect.x, rect.y + padding / 2, rect.width, letters_height);
        p.text(&upper, letters_rect, &big, white, 1.0)?;
        let title_rect = Rect::new(
            rect.x + padding,
            letters_rect.bottom(),
            rect.width - 2 * padding,
            title_height,
        );
        p.text(&hint.title, title_rect, &small, white, 0.8)
    })
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: forwarding to the default procedure.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}
