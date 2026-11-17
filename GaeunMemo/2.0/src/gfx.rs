//! GDI helpers: cached fonts/brushes, text, double-buffered painting.

use crate::win::px;
use std::cell::RefCell;
use std::collections::HashMap;
use windows_sys::Win32::Foundation::{HWND, RECT, SIZE};
use windows_sys::Win32::Graphics::Gdi::*;

pub fn ws(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct FontSpec {
    /// CSS px × 10 (13.5 px → 135)
    pub size_x10: u32,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
}

impl FontSpec {
    pub const fn px(size_x10: u32) -> FontSpec {
        FontSpec { size_x10, bold: false, italic: false, underline: false, strike: false }
    }
    pub const fn bold(size_x10: u32) -> FontSpec {
        FontSpec { size_x10, bold: true, italic: false, underline: false, strike: false }
    }
}

thread_local! {
    static FONTS: RefCell<HashMap<(FontSpec, u32), HFONT>> = RefCell::new(HashMap::new());
    static BRUSHES: RefCell<HashMap<u32, HBRUSH>> = RefCell::new(HashMap::new());
}

pub fn font(f: FontSpec, dpi: u32) -> HFONT {
    FONTS.with(|m| {
        *m.borrow_mut().entry((f, dpi)).or_insert_with(|| unsafe {
            let face = crate::win::wide("Malgun Gothic");
            CreateFontW(
                -px(f.size_x10 as f32 / 10.0, dpi),
                0,
                0,
                0,
                if f.bold { 700 } else { 400 },
                f.italic as u32,
                f.underline as u32,
                f.strike as u32,
                DEFAULT_CHARSET as u32,
                OUT_DEFAULT_PRECIS as u32,
                CLIP_DEFAULT_PRECIS as u32,
                CLEARTYPE_QUALITY as u32,
                0,
                face.as_ptr(),
            )
        })
    })
}

/// The note chrome's fonts made and drawn once into a memory DC: the first title drawn with
/// Malgun Gothic Bold cost 4–6 ms (font file and Hangul glyphs — P6 paint profile). Run at
/// start-up while the store opens on its own thread.
pub fn warm_fonts(dpi: u32) {
    unsafe {
        let dc = CreateCompatibleDC(std::ptr::null_mut());
        if dc.is_null() {
            return;
        }
        let bmp = CreateCompatibleBitmap(dc, 8, 8);
        let old = SelectObject(dc, bmp);
        let r = rect(0, 0, 8, 8);
        let sample = crate::win::wide("메모 제목 가나다라 abc 123 ✓ 복사됨");
        for f in [FontSpec::bold(135), FontSpec::px(130), FontSpec::px(140)] {
            text(dc, &sample[..sample.len() - 1], &r, font(f, dpi), 0, DT_SINGLELINE);
        }
        SelectObject(dc, old);
        DeleteObject(bmp);
        DeleteDC(dc);
    }
}

pub fn brush(c: u32) -> HBRUSH {
    BRUSHES.with(|b| *b.borrow_mut().entry(c).or_insert_with(|| unsafe { CreateSolidBrush(c) }))
}

pub fn fill(hdc: HDC, r: &RECT, c: u32) {
    unsafe { FillRect(hdc, r, brush(c)) };
}

pub fn frame(hdc: HDC, r: &RECT, c: u32) {
    unsafe { FrameRect(hdc, r, brush(c)) };
}

pub fn rect(l: i32, t: i32, r: i32, b: i32) -> RECT {
    RECT { left: l, top: t, right: r, bottom: b }
}

/// Draws `s` in `r`; returns nothing. Flags are DT_* (DT_NOPREFIX always added).
pub fn text(hdc: HDC, s: &[u16], r: &RECT, f: HFONT, color: u32, flags: u32) {
    unsafe {
        let old = SelectObject(hdc, f);
        SetBkMode(hdc, TRANSPARENT as i32);
        SetTextColor(hdc, color);
        let mut rr = *r;
        DrawTextW(hdc, s.as_ptr(), s.len() as i32, &mut rr, flags | DT_NOPREFIX);
        SelectObject(hdc, old);
    }
}

pub fn text_w(hdc: HDC, s: &[u16], f: HFONT) -> i32 {
    unsafe {
        let old = SelectObject(hdc, f);
        let mut sz = SIZE { cx: 0, cy: 0 };
        GetTextExtentPoint32W(hdc, s.as_ptr(), s.len() as i32, &mut sz);
        SelectObject(hdc, old);
        sz.cx
    }
}

/// Line height of a font (ascent + descent).
pub fn line_h(hdc: HDC, f: HFONT) -> i32 {
    unsafe {
        let old = SelectObject(hdc, f);
        let mut tm: TEXTMETRICW = std::mem::zeroed();
        GetTextMetricsW(hdc, &mut tm);
        SelectObject(hdc, old);
        tm.tmHeight
    }
}

thread_local! {
    static PRINT_DC: std::cell::Cell<HDC> = const { std::cell::Cell::new(std::ptr::null_mut()) };
}

/// WM_PRINTCLIENT: run the window's normal paint code into `dc` instead of the screen.
pub fn print_into(dc: HDC, paint: impl FnOnce()) {
    PRINT_DC.with(|p| p.set(dc));
    paint();
    PRINT_DC.with(|p| p.set(std::ptr::null_mut()));
}

thread_local! {
    // P6 diagnostic: µs into each buffered paint at BeginPaint, bitmap made, drawn, done
    pub static GFX_TRACE: std::cell::RefCell<Vec<[f64; 4]>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// BeginPaint → draw the whole client into a memory DC → BitBlt → EndPaint
/// (or straight into the WM_PRINTCLIENT DC while `print_into` is active).

pub fn paint_buffered(hwnd: HWND, f: impl FnOnce(HDC, i32, i32)) {
    let pdc = PRINT_DC.with(|p| p.get());
    if !pdc.is_null() {
        let mut rc = rect(0, 0, 0, 0);
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut rc) };
        f(pdc, rc.right.max(1), rc.bottom.max(1));
        return;
    }
    unsafe {
        let t = std::time::Instant::now();
        let mut lap = [0.0f64; 4];
        let mut ps: PAINTSTRUCT = std::mem::zeroed();
        let hdc = BeginPaint(hwnd, &mut ps);
        lap[0] = t.elapsed().as_secs_f64() * 1e6;
        let mut rc = rect(0, 0, 0, 0);
        windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut rc);
        let (w, h) = (rc.right.max(1), rc.bottom.max(1));
        let mem = CreateCompatibleDC(hdc);
        let bmp = CreateCompatibleBitmap(hdc, w, h);
        let old = SelectObject(mem, bmp);
        lap[1] = t.elapsed().as_secs_f64() * 1e6;
        f(mem, w, h);
        lap[2] = t.elapsed().as_secs_f64() * 1e6;
        BitBlt(hdc, 0, 0, w, h, mem, 0, 0, SRCCOPY);
        SelectObject(mem, old);
        DeleteObject(bmp);
        DeleteDC(mem);
        EndPaint(hwnd, &ps);
        lap[3] = t.elapsed().as_secs_f64() * 1e6;
        if crate::metrics::diag() {
            GFX_TRACE.with(|g| g.borrow_mut().push(lap));
        }
    }
}
