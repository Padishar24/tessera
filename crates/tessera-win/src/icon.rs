//! Application icons of other windows, converted to premultiplied BGRA pixels
//! for Direct2D.

use std::ffi::c_void;
use std::mem::size_of;

use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC,
    DeleteObject, GetDIBits, GetObjectW, HBITMAP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GCLP_HICON, GCLP_HICONSM, GET_CLASS_LONG_INDEX, GetClassLongPtrW, GetIconInfo, HICON, ICON_BIG,
    ICON_SMALL, ICON_SMALL2, ICONINFO, SMTO_ABORTIFHUNG, SMTO_NORMAL, SendMessageTimeoutW,
    WM_GETICON,
};

use crate::window::Window;

/// How long a window may take to answer `WM_GETICON`.
const ICON_TIMEOUT_MS: u32 = 100;

/// Icon pixels: premultiplied BGRA, top-down, 4 bytes per pixel.
#[derive(Debug, Clone)]
pub struct Icon {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// Small icon of `window` for drawing at `size` px on a monitor with `dpi`.
///
/// If the small icon is smaller than `size` (e.g. 16 px at 125 %), the large
/// one is taken instead: scaled down it looks sharper than scaled up.
pub fn load(window: Window, size: u32, dpi: u32) -> Option<Icon> {
    let mut source = Source {
        window,
        dpi,
        answering: true,
    };
    let small = source
        .message(ICON_SMALL2)
        .or_else(|| source.message(ICON_SMALL))
        .or_else(|| source.class(GCLP_HICONSM))
        .and_then(convert);
    if small.as_ref().is_some_and(|icon| icon.width >= size) {
        return small;
    }
    let big = source
        .message(ICON_BIG)
        .or_else(|| source.class(GCLP_HICON))
        .and_then(convert);
    match (small, big) {
        (_, Some(big)) if big.width >= size => Some(big),
        (Some(small), _) => Some(small),
        (None, big) => big,
    }
}

/// Where the icon handles come from. They belong to the other application or
/// its window class and are never destroyed here.
struct Source {
    window: Window,
    dpi: u32,
    /// Cleared after a `WM_GETICON` timed out: a busy application costs at
    /// most one timeout.
    answering: bool,
}

impl Source {
    fn message(&mut self, kind: u32) -> Option<HICON> {
        if !self.answering {
            return None;
        }
        let mut result = 0;
        // SAFETY: message with timeout; `result` lives for the duration of
        // the call. lParam is the DPI the icon is wanted for.
        let sent = unsafe {
            SendMessageTimeoutW(
                self.window.0,
                WM_GETICON,
                WPARAM(kind as usize),
                LPARAM(self.dpi as isize),
                SMTO_NORMAL | SMTO_ABORTIFHUNG,
                ICON_TIMEOUT_MS,
                Some(&mut result),
            )
        };
        if sent.0 == 0 {
            self.answering = false;
            return None;
        }
        (result != 0).then_some(HICON(result as *mut c_void))
    }

    fn class(&self, index: GET_CLASS_LONG_INDEX) -> Option<HICON> {
        // SAFETY: pure query.
        let icon = unsafe { GetClassLongPtrW(self.window.0, index) };
        (icon != 0).then_some(HICON(icon as *mut c_void))
    }
}

fn convert(icon: HICON) -> Option<Icon> {
    let mut info = ICONINFO::default();
    // SAFETY: valid output structure; the bitmaps are copies that we release.
    unsafe {
        GetIconInfo(icon, &mut info).ok()?;
        let result = pixels(info.hbmColor, info.hbmMask);
        if !info.hbmColor.is_invalid() {
            let _ = DeleteObject(info.hbmColor.into());
        }
        let _ = DeleteObject(info.hbmMask.into());
        result
    }
}

fn pixels(color: HBITMAP, mask: HBITMAP) -> Option<Icon> {
    // Monochrome icons have no color bitmap; hardly any application uses them.
    if color.is_invalid() {
        return None;
    }
    let mut bitmap = BITMAP::default();
    // SAFETY: the buffer has the size passed in.
    let read = unsafe {
        GetObjectW(
            color.into(),
            size_of::<BITMAP>() as i32,
            Some((&raw mut bitmap).cast()),
        )
    };
    let (width, height) = (bitmap.bmWidth, bitmap.bmHeight);
    if read == 0 || width <= 0 || height <= 0 {
        return None;
    }
    let mut bgra = dib_bits(color, width, height)?;
    // Without an alpha channel the transparency is in the AND mask.
    let mask = if bgra.as_chunks::<4>().0.iter().all(|p| p[3] == 0) {
        Some(dib_bits(mask, width, height)?)
    } else {
        None
    };
    premultiply(&mut bgra, mask.as_deref());
    Some(Icon {
        width: width as u32,
        height: height as u32,
        pixels: bgra,
    })
}

/// Contents of `bitmap` as 32-bit top-down pixels.
fn dib_bits(bitmap: HBITMAP, width: i32, height: i32) -> Option<Vec<u8>> {
    let mut info = BITMAPINFO {
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
    let mut bits = vec![0u8; width as usize * height as usize * 4];
    // SAFETY: the buffer holds `height` lines of 32-bit pixels; the DC is
    // released right away.
    let lines = unsafe {
        let dc = CreateCompatibleDC(None);
        let lines = GetDIBits(
            dc,
            bitmap,
            0,
            height as u32,
            Some(bits.as_mut_ptr().cast()),
            &mut info,
            DIB_RGB_COLORS,
        );
        let _ = DeleteDC(dc);
        lines
    };
    (lines == height).then_some(bits)
}

/// Straight to premultiplied alpha; with `mask`, the alpha comes from there
/// (black = opaque).
fn premultiply(bgra: &mut [u8], mask: Option<&[u8]>) {
    for (i, pixel) in bgra.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        if let Some(mask) = mask {
            pixel[3] = if mask[i * 4] == 0 { 255 } else { 0 };
        }
        let alpha = u16::from(pixel[3]);
        for c in &mut pixel[..3] {
            *c = ((u16::from(*c) * alpha + 127) / 255) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn premultiplies_the_alpha_channel() {
        let mut bgra = [200, 100, 50, 128, 10, 20, 30, 0];
        premultiply(&mut bgra, None);
        assert_eq!(bgra, [100, 50, 25, 128, 0, 0, 0, 0]);
    }

    #[test]
    fn takes_the_alpha_from_the_mask_without_alpha_channel() {
        let mut bgra = [200, 100, 50, 0, 10, 20, 30, 0];
        let mask = [0, 0, 0, 0, 255, 255, 255, 0];
        premultiply(&mut bgra, Some(&mask));
        assert_eq!(bgra, [200, 100, 50, 255, 0, 0, 0, 0]);
    }
}
