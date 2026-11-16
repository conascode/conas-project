//! Note chrome (band, title bar, tool icons, 1px edge) drawn three ways for PT-A/B/B'.
//!
//! * `Gdi`     — memory-DC double buffer, cached fonts/brushes/pens (PT-A)
//! * `D2dHwnd` — one ID2D1HwndRenderTarget per window (PT-B)
//! * `D2dDc`   — one shared ID2D1DCRenderTarget bound to the paint DC (PT-B')
//!
//! d2d1.dll / dwrite.dll are loaded with LoadLibrary only when a D2D mode is used,
//! so the GDI measurement does not pay for them.

use crate::win::{geom, px, rgb_f, theme, wide};
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use windows::core::{Interface, GUID, HRESULT};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows_numerics::Vector2;
use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RenderKind {
    Gdi,
    D2dHwnd,
    D2dDc,
}

impl RenderKind {
    pub fn name(self) -> &'static str {
        match self {
            RenderKind::Gdi => "gdi",
            RenderKind::D2dHwnd => "d2d-hwnd",
            RenderKind::D2dDc => "d2d-dc",
        }
    }
}

/// Tool buttons, left to right (v1.1 order without the menu: pin, new, full/restore, close).
pub const TOOLS: usize = 4;
pub const T_PIN: usize = 0;
pub const T_NEW: usize = 1;
pub const T_FULL: usize = 2;

pub struct Chrome<'a> {
    pub w: i32,
    pub h: i32,
    pub dpi: u32,
    pub accent: u32,
    pub title: &'a [u16],
    pub tools: bool,
    pub hot: Option<usize>,
    pub pinned: bool,
    pub full: bool,
}

pub struct Layout {
    pub band: i32,
    pub title: RECT,
    pub tools: [RECT; TOOLS],
}

pub fn layout(w: i32, dpi: u32, tools_visible: bool) -> Layout {
    let band = px(geom::BAND, dpi);
    let bar_bottom = band + px(geom::BAR, dpi);
    let t = px(geom::TOOL, dpi);
    let gap = px(geom::TOOL_GAP, dpi);
    let top = band + (px(geom::BAR, dpi) - t) / 2;
    let mut tools = [RECT { left: 0, top: 0, right: 0, bottom: 0 }; TOOLS];
    let mut right = w - px(geom::BAR_PAD_R, dpi);
    for i in (0..TOOLS).rev() {
        tools[i] = RECT { left: right - t, top, right, bottom: top + t };
        right -= t + gap;
    }
    let title_right = if tools_visible { tools[0].left - px(4.0, dpi) } else { w - px(geom::BAR_PAD_R, dpi) };
    let title = RECT { left: px(geom::BAR_PAD_L, dpi), top: band, right: title_right, bottom: bar_bottom };
    Layout { band, title, tools }
}

/// Icon strokes in a 14×14 box (from v1.1 note.html SVG paths, straight-line approximations).
fn icon(kind: usize, pinned: bool, full: bool) -> (Vec<Vec<(f32, f32)>>, bool) {
    match kind {
        T_PIN => (
            vec![
                vec![(8.2, 2.2), (11.8, 5.8), (10.0, 6.4), (8.1, 8.3), (8.3, 10.2), (7.4, 11.1), (2.9, 6.6), (3.8, 5.7), (5.7, 5.9), (7.6, 4.0), (8.2, 2.2)],
                vec![(5.6, 8.4), (2.5, 11.5)],
            ],
            pinned,
        ),
        T_NEW => (vec![vec![(7.0, 2.5), (7.0, 11.5)], vec![(2.5, 7.0), (11.5, 7.0)]], false),
        T_FULL if full => (
            vec![
                vec![(2.5, 2.5), (5.7, 5.7)],
                vec![(5.7, 2.8), (5.7, 5.7), (2.8, 5.7)],
                vec![(11.5, 11.5), (8.3, 8.3)],
                vec![(8.3, 11.2), (8.3, 8.3), (11.2, 8.3)],
            ],
            false,
        ),
        T_FULL => (vec![vec![(2.5, 2.5), (11.5, 2.5), (11.5, 11.5), (2.5, 11.5), (2.5, 2.5)]], false),
        _ => (vec![vec![(3.5, 3.5), (10.5, 10.5)], vec![(10.5, 3.5), (3.5, 10.5)]], false),
    }
}



// ---------------------------------------------------------------- Direct2D

pub struct D2d {
    factory: ID2D1Factory,
    dwrite: IDWriteFactory,
    formats: HashMap<u32, IDWriteTextFormat>,
    dc: Option<(ID2D1DCRenderTarget, ID2D1SolidColorBrush)>,
}

thread_local! {
    static D2D: RefCell<Option<D2d>> = const { RefCell::new(None) };
}

type CreateFactoryFn = unsafe extern "system" fn(u32, *const GUID, *const D2D1_FACTORY_OPTIONS, *mut *mut c_void) -> HRESULT;
type DWriteCreateFn = unsafe extern "system" fn(u32, *const GUID, *mut *mut c_void) -> HRESULT;

fn d2d_init() -> Option<D2d> {
    unsafe {
        let d2d1 = LoadLibraryW(wide("d2d1.dll").as_ptr());
        let dw = LoadLibraryW(wide("dwrite.dll").as_ptr());
        let f1: CreateFactoryFn = std::mem::transmute(GetProcAddress(d2d1, c"D2D1CreateFactory".as_ptr() as *const u8)?);
        let f2: DWriteCreateFn = std::mem::transmute(GetProcAddress(dw, c"DWriteCreateFactory".as_ptr() as *const u8)?);
        let mut p = std::ptr::null_mut();
        f1(D2D1_FACTORY_TYPE_SINGLE_THREADED.0 as u32, &ID2D1Factory::IID, &D2D1_FACTORY_OPTIONS::default(), &mut p).ok().ok()?;
        let factory = ID2D1Factory::from_raw(p);
        let mut q = std::ptr::null_mut();
        f2(DWRITE_FACTORY_TYPE_SHARED.0 as u32, &IDWriteFactory::IID, &mut q).ok().ok()?;
        let dwrite = IDWriteFactory::from_raw(q);
        Some(D2d { factory, dwrite, formats: HashMap::new(), dc: None })
    }
}

fn with_d2d<R>(f: impl FnOnce(&mut D2d) -> R) -> Option<R> {
    D2D.with(|d| {
        let mut d = d.borrow_mut();
        if d.is_none() {
            *d = d2d_init();
        }
        d.as_mut().map(f)
    })
}

/// Releases the shared D2D objects while d2d1.dll is still loaded
/// (thread-local destructors would otherwise run during process teardown).
pub fn shutdown() {
    D2D.with(|d| d.borrow_mut().take());
}

fn color(c: u32) -> D2D1_COLOR_F {
    let (r, g, b) = rgb_f(c);
    D2D1_COLOR_F { r, g, b, a: 1.0 }
}

fn text_format(d: &mut D2d, dpi: u32) -> Option<IDWriteTextFormat> {
    if let Some(f) = d.formats.get(&dpi) {
        return Some(f.clone());
    }
    unsafe {
        let f = d
            .dwrite
            .CreateTextFormat(
                windows::core::w!("Malgun Gothic"),
                None,
                DWRITE_FONT_WEIGHT_BOLD,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                geom::TITLE_PX * dpi as f32 / 96.0,
                windows::core::w!("ko-kr"),
            )
            .ok()?;
        let _ = f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
        let _ = f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
        d.formats.insert(dpi, f.clone());
        Some(f)
    }
}

/// Per-window D2D resources for PT-B.
#[derive(Default)]
pub struct HwndTarget {
    rt: Option<ID2D1HwndRenderTarget>,
    brush: Option<ID2D1SolidColorBrush>,
}

impl HwndTarget {
    pub fn resize(&mut self, w: i32, h: i32) {
        if let Some(rt) = &self.rt {
            unsafe {
                let _ = rt.Resize(&D2D_SIZE_U { width: w.max(1) as u32, height: h.max(1) as u32 });
            }
        }
    }
}

fn rt_props(format: DXGI_FORMAT, alpha: D2D1_ALPHA_MODE) -> D2D1_RENDER_TARGET_PROPERTIES {
    D2D1_RENDER_TARGET_PROPERTIES {
        r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
        pixelFormat: D2D1_PIXEL_FORMAT { format, alphaMode: alpha },
        dpiX: 96.0,
        dpiY: 96.0,
        usage: D2D1_RENDER_TARGET_USAGE_NONE,
        minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
    }
}

const RECREATE: HRESULT = HRESULT(0x8899000Cu32 as i32);

unsafe fn draw_d2d(rt: &ID2D1RenderTarget, b: &ID2D1SolidColorBrush, fmt: &IDWriteTextFormat, c: &Chrome) {
    let l = layout(c.w, c.dpi, c.tools);
    let f = |r: &RECT| D2D_RECT_F { left: r.left as f32, top: r.top as f32, right: r.right as f32, bottom: r.bottom as f32 };
    rt.Clear(Some(&color(theme::PAPER)));
    b.SetColor(&color(c.accent));
    rt.FillRectangle(&D2D_RECT_F { left: 0.0, top: 0.0, right: c.w as f32, bottom: l.band as f32 }, b);
    b.SetColor(&color(theme::INK));
    rt.DrawText(c.title, fmt, &f(&l.title), b, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
    if c.tools {
        let s = c.dpi as f32 / 96.0;
        let w = 1.5 * s;
        for (i, r) in l.tools.iter().enumerate() {
            if c.hot == Some(i) {
                b.SetColor(&color(theme::PAPER3));
                rt.FillRectangle(&f(r), b);
            }
            b.SetColor(&color(if c.hot == Some(i) { theme::INK } else { theme::MUTED }));
            let ox = (r.left + r.right) as f32 / 2.0 - geom::ICON * s / 2.0;
            let oy = (r.top + r.bottom) as f32 / 2.0 - geom::ICON * s / 2.0;
            let (strokes, _fill) = icon(i, c.pinned, c.full);
            for poly in &strokes {
                for seg in poly.windows(2) {
                    let p0 = Vector2 { X: ox + seg[0].0 * s, Y: oy + seg[0].1 * s };
                    let p1 = Vector2 { X: ox + seg[1].0 * s, Y: oy + seg[1].1 * s };
                    rt.DrawLine(p0, p1, b, w, None);
                }
            }
        }
    }
    b.SetColor(&color(theme::EDGE));
    rt.DrawRectangle(&D2D_RECT_F { left: 0.5, top: 0.5, right: c.w as f32 - 0.5, bottom: c.h as f32 - 0.5 }, b, 1.0, None);
}

/// PT-B. Caller has already called BeginPaint (validation) and calls EndPaint after.
pub fn paint_d2d_hwnd(hwnd: HWND, t: &mut HwndTarget, c: &Chrome) {
    with_d2d(|d| unsafe {
        let Some(fmt) = text_format(d, c.dpi) else { return };
        if t.rt.is_none() {
            let hp = D2D1_HWND_RENDER_TARGET_PROPERTIES {
                hwnd: windows::Win32::Foundation::HWND(hwnd),
                pixelSize: D2D_SIZE_U { width: c.w.max(1) as u32, height: c.h.max(1) as u32 },
                presentOptions: D2D1_PRESENT_OPTIONS_NONE,
            };
            let Ok(rt) = d.factory.CreateHwndRenderTarget(&rt_props(DXGI_FORMAT_UNKNOWN, D2D1_ALPHA_MODE_UNKNOWN), &hp) else { return };
            t.brush = rt.CreateSolidColorBrush(&color(theme::INK), None).ok();
            t.rt = Some(rt);
        }
        let (Some(rt), Some(b)) = (t.rt.clone(), t.brush.clone()) else { return };
        rt.BeginDraw();
        draw_d2d(&rt, &b, &fmt, c);
        if let Err(e) = rt.EndDraw(None, None) {
            if e.code() == RECREATE {
                t.rt = None;
                t.brush = None;
            }
        }
    });
}

/// PT-B'. Draws into the BeginPaint DC through one shared DC render target.
pub fn paint_d2d_dc(hdc: HDC, c: &Chrome) {
    with_d2d(|d| unsafe {
        let Some(fmt) = text_format(d, c.dpi) else { return };
        if d.dc.is_none() {
            let Ok(rt) = d.factory.CreateDCRenderTarget(&rt_props(DXGI_FORMAT_B8G8R8A8_UNORM, D2D1_ALPHA_MODE_IGNORE)) else { return };
            let Ok(b) = rt.CreateSolidColorBrush(&color(theme::INK), None) else { return };
            d.dc = Some((rt, b));
        }
        let Some((rt, b)) = d.dc.clone() else { return };
        let rc = windows::Win32::Foundation::RECT { left: 0, top: 0, right: c.w, bottom: c.h };
        if rt.BindDC(windows::Win32::Graphics::Gdi::HDC(hdc), &rc).is_err() {
            return;
        }
        rt.BeginDraw();
        draw_d2d(&rt, &b, &fmt, c);
        if let Err(e) = rt.EndDraw(None, None) {
            if e.code() == RECREATE {
                d.dc = None;
            }
        }
    });
}
