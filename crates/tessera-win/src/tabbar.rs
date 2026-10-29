//! Tab bar above a stack: shows the titles of the stacked windows and a
//! close button on each tab; a click on a tab reaches the daemon as
//! [`Event::TabClicked`].

use tessera_core::config::Color;
use tessera_core::geometry::Rect;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, GW_HWNDPREV, GWL_EXSTYLE, GWLP_USERDATA, GetWindow, GetWindowLongPtrW,
    HWND_TOP, MA_NOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetWindowPos, WM_LBUTTONUP,
    WM_MBUTTONUP, WM_MOUSEACTIVATE, WS_EX_TOPMOST,
};
use windows::core::w;

use crate::events::{self, Event};
use crate::layered::{LayeredWindow, Options};
use crate::window::Window;

/// Contents of a tab bar.
#[derive(Debug, Clone, PartialEq)]
pub struct Tabs {
    /// Area of the bar in screen coordinates.
    pub rect: Rect,
    pub dpi: u32,
    pub titles: Vec<String>,
    pub active: usize,
    pub background: Color,
    pub highlight: Color,
    /// Show a close button at the right end of each tab.
    pub close_buttons: bool,
}

/// What a click on the bar hits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabHit {
    /// The tab itself: bring its window to the front.
    Tab(usize),
    /// The close button of the tab.
    Close(usize),
}

impl Tabs {
    fn tab_rect(&self, index: usize) -> Rect {
        let n = self.titles.len().max(1) as i32;
        let i = index as i32;
        let left = self.rect.x + self.rect.width * i / n;
        let right = self.rect.x + self.rect.width * (i + 1) / n;
        Rect::new(left, self.rect.y, right - left, self.rect.height)
    }

    /// Square close button at the right end of a tab. Tabs too narrow to keep
    /// some room for the title next to it get none (middle-click still
    /// closes them).
    fn close_rect(&self, index: usize) -> Option<Rect> {
        let tab = self.tab_rect(index);
        let size = tab.height;
        (self.close_buttons && tab.width >= 3 * size)
            .then(|| Rect::new(tab.right() - size, tab.y, size, size))
    }

    /// Hit test at `x` relative to the bar.
    fn hit(&self, x: i32) -> Option<TabHit> {
        let n = self.titles.len();
        if n == 0 || self.rect.width <= 0 {
            return None;
        }
        let index = ((x.max(0) as usize) * n / self.rect.width as usize).min(n - 1);
        let on_close = self
            .close_rect(index)
            .is_some_and(|r| (r.x - self.rect.x..r.right() - self.rect.x).contains(&x));
        Some(if on_close {
            TabHit::Close(index)
        } else {
            TabHit::Tab(index)
        })
    }
}

pub struct TabBar {
    window: LayeredWindow,
    shown: Option<Tabs>,
}

impl TabBar {
    /// `id` comes back in [`Event::TabClicked`].
    pub fn new(id: usize) -> anyhow::Result<Self> {
        Ok(Self {
            window: LayeredWindow::new(Options {
                class: w!("Tessera.TabBar"),
                title: w!("Tessera Tabs"),
                click_through: false,
                topmost: false,
                wndproc: Some(wndproc),
                user_data: id as isize,
            })?,
            shown: None,
        })
    }

    /// Draws (only on changes) and places the bar directly above `above`.
    pub fn show(&mut self, tabs: Tabs, above: Window) -> anyhow::Result<()> {
        if self.shown.as_ref() != Some(&tabs) {
            self.draw(&tabs)?;
            self.shown = Some(tabs);
        }
        self.raise_above(above);
        Ok(())
    }

    pub fn hide(&mut self) {
        self.shown = None;
        self.window.hide();
    }

    /// Tab or close button at the x position relative to the bar.
    pub fn hit_at(&self, x: i32) -> Option<TabHit> {
        self.shown.as_ref()?.hit(x)
    }

    fn draw(&mut self, tabs: &Tabs) -> anyhow::Result<()> {
        let scale = tabs.dpi as f32 / 96.0;
        let normal = self
            .window
            .text_format(12.5 * scale, DWRITE_FONT_WEIGHT_NORMAL, false)?;
        let bold = self
            .window
            .text_format(12.5 * scale, DWRITE_FONT_WEIGHT_SEMI_BOLD, false)?;
        let padding = (10.0 * scale) as i32;
        let inset = (2.0 * scale).max(1.0) as i32;
        let radius = 6.0 * scale;
        let white = Color::rgb(255, 255, 255);
        self.window.present(tabs.rect, |p| {
            // Opaque background: fully transparent pixels would be click-through.
            p.fill(tabs.rect, tabs.background, 0.92, radius)?;
            for (i, title) in tabs.titles.iter().enumerate() {
                let tab = tabs.tab_rect(i);
                let active = i == tabs.active;
                if active {
                    let r = Rect::new(
                        tab.x + inset,
                        tab.y + inset,
                        tab.width - 2 * inset,
                        tab.height - 2 * inset,
                    );
                    p.fill(r, tabs.highlight, 0.95, radius)?;
                }
                let close = tabs.close_rect(i);
                let text_right = close.map_or(tab.right() - padding, |c| c.x);
                let text = Rect::new(
                    tab.x + padding,
                    tab.y,
                    text_right - tab.x - padding,
                    tab.height,
                );
                let (format, alpha) = if active {
                    (&bold, 1.0)
                } else {
                    (&normal, 0.75)
                };
                p.text(title, text, format, white, alpha)?;
                if let Some(c) = close {
                    // A small "x" in the middle of the button.
                    let (cx, cy) = c.center();
                    let (cx, cy) = (cx as f32, cy as f32);
                    let arm = 4.0 * scale;
                    let width = (1.5 * scale).max(1.0);
                    let alpha = if active { 0.9 } else { 0.6 };
                    p.line(
                        (cx - arm, cy - arm),
                        (cx + arm, cy + arm),
                        white,
                        alpha,
                        width,
                    )?;
                    p.line(
                        (cx - arm, cy + arm),
                        (cx + arm, cy - arm),
                        white,
                        alpha,
                        width,
                    )?;
                }
            }
            Ok(())
        })
    }

    /// Z-order: directly above the active stack window, so the bar is
    /// covered and reappears together with the stack.
    fn raise_above(&self, above: Window) {
        let bar = self.window.hwnd();
        // SAFETY: pure queries; z-order of our own window.
        unsafe {
            let previous = GetWindow(above.0, GW_HWNDPREV).ok();
            if previous == Some(bar) {
                return;
            }
            // Inserted behind a topmost window, the bar would itself become topmost.
            let insert_after = previous
                .filter(|p| GetWindowLongPtrW(*p, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST.0 == 0)
                .unwrap_or(HWND_TOP);
            let _ = SetWindowPos(
                bar,
                Some(insert_after),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        // Clicks should not move the focus to the bar.
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_LBUTTONUP | WM_MBUTTONUP => {
            // SAFETY: own window; USERDATA was set on creation.
            let bar = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as usize;
            let x = i32::from(lparam.0 as u16 as i16);
            events::push(Event::TabClicked {
                bar,
                x,
                middle: msg == WM_MBUTTONUP,
            });
            LRESULT(0)
        }
        // SAFETY: forwarding to the default procedure.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tabs(width: i32, count: usize, close_buttons: bool) -> Tabs {
        Tabs {
            rect: Rect::new(100, 50, width, 28),
            dpi: 96,
            titles: vec![String::new(); count],
            active: 0,
            background: Color::rgb(0, 0, 0),
            highlight: Color::rgb(0, 0, 0),
            close_buttons,
        }
    }

    #[test]
    fn close_button_sits_at_the_right_end_of_each_tab() {
        let t = tabs(600, 2, true);
        assert_eq!(t.hit(10), Some(TabHit::Tab(0)));
        assert_eq!(t.hit(299), Some(TabHit::Close(0)));
        assert_eq!(t.hit(300), Some(TabHit::Tab(1)));
        assert_eq!(t.hit(590), Some(TabHit::Close(1)));
        // Past the right edge still counts as the last tab, not its button.
        assert_eq!(t.hit(700), Some(TabHit::Tab(1)));
    }

    #[test]
    fn no_close_button_when_disabled_or_too_narrow() {
        assert_eq!(tabs(600, 2, false).hit(299), Some(TabHit::Tab(0)));
        // 8 tabs of 75 px: less than three button widths each.
        assert_eq!(tabs(600, 8, true).hit(74), Some(TabHit::Tab(0)));
        assert_eq!(tabs(600, 0, true).hit(10), None);
    }
}
