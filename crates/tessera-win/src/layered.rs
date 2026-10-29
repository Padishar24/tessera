//! Common base for our own displays (zone overlay, tab bars):
//! a borderless layered window that Direct2D draws into and that is shown
//! with an alpha channel via `UpdateLayeredWindow`.

use std::mem::size_of;

use tessera_core::config::Color;
use tessera_core::geometry::Rect;
use windows::Win32::Foundation::{COLORREF, HWND, POINT, RECT, SIZE};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FEATURE_LEVEL_DEFAULT,
    D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_USAGE_NONE,
    D2D1_ROUNDED_RECT, D2D1CreateFactory, ID2D1DCRenderTarget, ID2D1Factory, ID2D1RenderTarget,
    ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT, DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
    DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TRIMMING,
    DWRITE_TRIMMING_GRANULARITY_CHARACTER, DWRITE_WORD_WRAPPING_NO_WRAP, DWriteCreateFactory,
    IDWriteFactory, IDWriteTextFormat,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
    CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, HBITMAP, HDC,
    HGDIOBJ, SelectObject,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, GWLP_USERDATA, HWND_TOPMOST, IDC_ARROW, LoadCursorW,
    RegisterClassW, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SetWindowLongPtrW, SetWindowPos, ShowWindow, ULW_ALPHA, UpdateLayeredWindow, WINDOW_EX_STYLE,
    WNDCLASSW, WNDPROC, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::{PCWSTR, w};

pub struct Options {
    pub class: PCWSTR,
    pub title: PCWSTR,
    /// Mouse clicks pass through the window.
    pub click_through: bool,
    pub topmost: bool,
    pub wndproc: WNDPROC,
    /// Ends up in `GWLP_USERDATA`, e.g. for lookup in the window procedure.
    pub user_data: isize,
}

/// 32-bit DIB that Direct2D draws into.
struct Surface {
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    width: i32,
    height: i32,
}

impl Surface {
    fn new(width: i32, height: i32) -> anyhow::Result<Self> {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                // Negative = top-down.
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        // SAFETY: DC and bitmap are released in Drop.
        unsafe {
            let dc = CreateCompatibleDC(None);
            let mut bits = std::ptr::null_mut();
            let bitmap = match CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)
            {
                Ok(bitmap) => bitmap,
                Err(e) => {
                    let _ = DeleteDC(dc);
                    return Err(e.into());
                }
            };
            let previous = SelectObject(dc, bitmap.into());
            Ok(Self {
                dc,
                bitmap,
                previous,
                width,
                height,
            })
        }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        // SAFETY: objects come from `new` and are only released here.
        unsafe {
            SelectObject(self.dc, self.previous);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

pub struct LayeredWindow {
    hwnd: HWND,
    topmost: bool,
    factory: ID2D1Factory,
    dwrite: IDWriteFactory,
    target: Option<ID2D1DCRenderTarget>,
    surface: Option<Surface>,
    visible: bool,
}

impl LayeredWindow {
    pub fn new(options: Options) -> anyhow::Result<Self> {
        let mut ex_style = WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
        if options.click_through {
            ex_style |= WS_EX_TRANSPARENT;
        }
        if options.topmost {
            ex_style |= WS_EX_TOPMOST;
        }
        // SAFETY: standard window creation; window lives until Drop.
        unsafe {
            let instance = GetModuleHandleW(None)?;
            // Fails if the class is already registered - harmless.
            RegisterClassW(&WNDCLASSW {
                lpfnWndProc: options.wndproc,
                hInstance: instance.into(),
                hCursor: LoadCursorW(None, IDC_ARROW)?,
                lpszClassName: options.class,
                ..Default::default()
            });
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(ex_style.0),
                options.class,
                options.title,
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(instance.into()),
                None,
            )?;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, options.user_data);
            Ok(Self {
                hwnd,
                topmost: options.topmost,
                factory: D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?,
                dwrite: DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?,
                target: None,
                surface: None,
                visible: false,
            })
        }
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// Draws with `draw` and shows the window at `rect` (screen coordinates).
    pub fn present(
        &mut self,
        rect: Rect,
        draw: impl FnOnce(&Painter) -> windows::core::Result<()>,
    ) -> anyhow::Result<()> {
        let (width, height) = (rect.width, rect.height);
        if width <= 0 || height <= 0 {
            self.hide();
            return Ok(());
        }
        if self
            .surface
            .as_ref()
            .is_none_or(|s| s.width != width || s.height != height)
        {
            self.surface = None;
            self.surface = Some(Surface::new(width, height)?);
        }
        let dc = self.surface.as_ref().expect("just created").dc;
        if let Err(e) = self.render(rect, dc, draw) {
            // Device lost or similar: recreate next time.
            self.target = None;
            return Err(e);
        }

        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        // SAFETY: valid window and DC; structures live for the duration of the call.
        unsafe {
            UpdateLayeredWindow(
                self.hwnd,
                None,
                Some(&POINT {
                    x: rect.x,
                    y: rect.y,
                }),
                Some(&SIZE {
                    cx: width,
                    cy: height,
                }),
                Some(dc),
                Some(&POINT { x: 0, y: 0 }),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            )?;
            if !self.visible {
                let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
                if self.topmost {
                    let _ = SetWindowPos(
                        self.hwnd,
                        Some(HWND_TOPMOST),
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                    );
                }
            }
        }
        self.visible = true;
        Ok(())
    }

    pub fn hide(&mut self) {
        if self.visible {
            self.visible = false;
            // SAFETY: own window.
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
        }
    }

    /// Font for [`Painter::text`]; centered or left-aligned with "..." at the end.
    pub fn text_format(
        &self,
        size: f32,
        weight: DWRITE_FONT_WEIGHT,
        centered: bool,
    ) -> anyhow::Result<IDWriteTextFormat> {
        // SAFETY: valid, null-terminated strings.
        unsafe {
            let format = self.dwrite.CreateTextFormat(
                w!("Segoe UI"),
                None,
                weight,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size,
                w!("en-us"),
            )?;
            format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            if centered {
                format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
            } else {
                format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING)?;
                format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
                let ellipsis = self.dwrite.CreateEllipsisTrimmingSign(&format)?;
                let trimming = DWRITE_TRIMMING {
                    granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                    delimiter: 0,
                    delimiterCount: 0,
                };
                format.SetTrimming(&trimming, &ellipsis)?;
            }
            Ok(format)
        }
    }

    fn render(
        &mut self,
        rect: Rect,
        dc: HDC,
        draw: impl FnOnce(&Painter) -> windows::core::Result<()>,
    ) -> anyhow::Result<()> {
        let target = match &self.target {
            Some(target) => target.clone(),
            None => {
                let props = D2D1_RENDER_TARGET_PROPERTIES {
                    r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                    pixelFormat: D2D1_PIXEL_FORMAT {
                        format: DXGI_FORMAT_B8G8R8A8_UNORM,
                        alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                    },
                    // Drawing happens in physical pixels.
                    dpiX: 96.0,
                    dpiY: 96.0,
                    usage: D2D1_RENDER_TARGET_USAGE_NONE,
                    minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
                };
                // SAFETY: valid properties.
                let target = unsafe { self.factory.CreateDCRenderTarget(&props)? };
                self.target = Some(target.clone());
                target
            }
        };
        let bounds = RECT {
            left: 0,
            top: 0,
            right: rect.width,
            bottom: rect.height,
        };
        // SAFETY: drawing between BeginDraw and EndDraw on the bound DC.
        unsafe {
            target.BindDC(dc, &bounds)?;
            target.BeginDraw();
            target.Clear(Some(&D2D1_COLOR_F::default()));
            let result = draw(&Painter {
                target: &target,
                origin: (rect.x, rect.y),
            });
            target.EndDraw(None, None)?;
            result?;
        }
        Ok(())
    }
}

impl Drop for LayeredWindow {
    fn drop(&mut self) {
        self.surface = None;
        // SAFETY: own window, only destroyed here.
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

/// Drawing helpers; rectangles in screen coordinates.
pub struct Painter<'a> {
    target: &'a ID2D1RenderTarget,
    origin: (i32, i32),
}

impl Painter<'_> {
    fn rect(&self, r: Rect, inset: f32) -> D2D_RECT_F {
        let (ox, oy) = self.origin;
        D2D_RECT_F {
            left: (r.x - ox) as f32 + inset,
            top: (r.y - oy) as f32 + inset,
            right: (r.right() - ox) as f32 - inset,
            bottom: (r.bottom() - oy) as f32 - inset,
        }
    }

    fn brush(&self, color: Color, alpha: f32) -> windows::core::Result<ID2D1SolidColorBrush> {
        let c = D2D1_COLOR_F {
            r: f32::from(color.r) / 255.0,
            g: f32::from(color.g) / 255.0,
            b: f32::from(color.b) / 255.0,
            a: alpha,
        };
        // SAFETY: valid color.
        unsafe { self.target.CreateSolidColorBrush(&c, None) }
    }

    pub fn fill(
        &self,
        r: Rect,
        color: Color,
        alpha: f32,
        radius: f32,
    ) -> windows::core::Result<()> {
        let brush = self.brush(color, alpha)?;
        let rounded = D2D1_ROUNDED_RECT {
            rect: self.rect(r, 0.0),
            radiusX: radius,
            radiusY: radius,
        };
        // SAFETY: inside BeginDraw/EndDraw.
        unsafe { self.target.FillRoundedRectangle(&rounded, &brush) };
        Ok(())
    }

    pub fn stroke(
        &self,
        r: Rect,
        color: Color,
        alpha: f32,
        width: f32,
        radius: f32,
    ) -> windows::core::Result<()> {
        let brush = self.brush(color, alpha)?;
        let rounded = D2D1_ROUNDED_RECT {
            rect: self.rect(r, width / 2.0),
            radiusX: radius,
            radiusY: radius,
        };
        // SAFETY: inside BeginDraw/EndDraw.
        unsafe {
            self.target
                .DrawRoundedRectangle(&rounded, &brush, width, None)
        };
        Ok(())
    }

    /// Text in `r`, clipped to the rectangle.
    pub fn text(
        &self,
        text: &str,
        r: Rect,
        format: &IDWriteTextFormat,
        color: Color,
        alpha: f32,
    ) -> windows::core::Result<()> {
        let brush = self.brush(color, alpha)?;
        let wide: Vec<u16> = text.encode_utf16().collect();
        // SAFETY: inside BeginDraw/EndDraw.
        unsafe {
            self.target.DrawText(
                &wide,
                format,
                &self.rect(r, 0.0),
                &brush,
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
        Ok(())
    }
}
