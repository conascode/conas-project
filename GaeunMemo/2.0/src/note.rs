//! GM_Note: the frameless note window (report §6.1–6.3, §7.1–7.3).
//! P1 reproduces every v1.1 visual state; storage arrives in P3.

use crate::app;
use crate::body::{self, Fmt};
use crate::gfx::{self, rect, ws, FontSpec};
use crate::icons::{self, Icon};
use crate::render::{self, Chrome, HwndTarget, RenderKind};
use crate::theme::{self, g, mix, pal, Band, EDGE, ON_ACCENT, ON_ACCENT_SOFT, PALETTE};
use crate::win::{px, qpc, qpc_us, wide, x_of, y_of};
use crate::{cfg, confirm, list, menu, stats};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::{GetDpiForSystem, GetDpiForWindow, GetSystemMetricsForDpi};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, SetFocus, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{VK_ESCAPE, VK_RETURN};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const CLASS: &str = "GM_Note";
const TIMER_SAVE: usize = 1;
const TIMER_COPIED: usize = 2;
/// a resize settled: pictures follow the new text width
const TIMER_REFIT: usize = 4;
/// once, 2 s after the first note window: picture machinery warmed while idle (P6)
const TIMER_WARM: usize = 5;
const SAVE_DELAY: Duration = Duration::from_millis(300);
const TME_NONCLIENT: u32 = 0x10;
const WM_MOUSELEAVE_: u32 = 0x02A3;
const WM_NCMOUSELEAVE_: u32 = 0x02A2;

pub const WM_APP_HOVER: u32 = WM_APP + 1;
pub const WM_APP_LEAVE: u32 = WM_APP + 2;
pub const WM_APP_COPY: u32 = WM_APP + 3;
pub const WM_APP_FONT: u32 = WM_APP + 4;
/// the body's formatting changed (no EN_CHANGE for that): save
pub const WM_APP_DIRTY: u32 = WM_APP + 6;
/// a picture's display-size copy arrived from the decoding thread (lParam: Box<images::Decoded>)
pub const WM_APP_DECODED: u32 = WM_APP + 7;
/// Decoded pictures freed because their note closed before showing them (tests).
pub static DROPPED_DECODED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
pub const WM_APP_MENU: u32 = WM_APP + 10;
pub const WM_APP_MENUCLOSED: u32 = WM_APP + 11;
pub const WM_APP_CONFIRM: u32 = WM_APP + 12;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Pin,
    New,
    More,
    Restore,
    Close,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Part {
    Tool(Tool),
    Fmt(Fmt),
}

pub struct NoteState {
    pub hwnd: HWND,
    pub id: u64,
    pub body: HWND,
    pub title_edit: HWND,
    pub dpi: u32,
    pub hover: bool,
    pub hot: Option<Part>,
    pub down: Option<Part>,
    pub copied: bool,
    pub menu: HWND,
    pub fmt_on: [bool; 5],
    pub painted: bool,
    pub last_save: Option<Instant>,
    pub save_pending: bool,
    pub saved_chars: usize,
    pub d2d: HwndTarget,
    pub normal_h: i32,
    /// text width (CSS px) the pictures were last fitted to
    pub pic_width: f64,
    /// inside the modal size / move loop (an edge or the title bar is being dragged)
    pub sizing: bool,
}

thread_local! {
    pub static NOTES: std::cell::RefCell<Vec<HWND>> = const { std::cell::RefCell::new(Vec::new()) };
}

thread_local! {
    /// Start-up trace of the first note window (ms): OleInitialize, msftedit load, body
    /// created, body filled, font size, whole WM_CREATE — p6bench reads it.
    pub static TRACE: std::cell::Cell<[f64; 6]> = const { std::cell::Cell::new([f64::NAN; 6]) };
}

thread_local! {
    /// the picture machinery was (or is about to be) warmed
    static WARMED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

thread_local! {
    /// body::create of the first note (ms): CreateWindowEx, callback + text, theme, font size
    pub static TRACE6: std::cell::Cell<[f64; 4]> = const { std::cell::Cell::new([f64::NAN; 4]) };
}

pub fn trace6(i: usize, ms: f64) {
    TRACE6.with(|t| {
        let mut v = t.get();
        if v[i].is_nan() {
            v[i] = ms;
            t.set(v);
        }
    });
}

pub fn trace(i: usize, ms: f64) {
    TRACE.with(|t| {
        let mut v = t.get();
        if v[i].is_nan() {
            v[i] = ms;
            t.set(v);
        }
    });
}

pub fn notes() -> Vec<HWND> {
    NOTES.with(|n| n.borrow().clone())
}

/// Everything typed so far goes to the store now: an IME composition is finished, a
/// rename ends, a waiting throttled save runs (tray 종료, session end).
pub fn flush_all() {
    for h in notes() {
        unsafe {
            let Some(s) = state_of(h) else { continue };
            crate::win::ime_complete(s.body);
            if !s.title_edit.is_null() {
                end_rename(s, false);
            }
            if s.save_pending {
                KillTimer(h, TIMER_SAVE);
                s.save_pending = false;
                serialize(s);
            }
        }
    }
}

pub fn register_class() {
    unsafe {
        let name = wide(CLASS);
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: GetModuleHandleW(std::ptr::null()),
            // the app icon on the taskbar buttons and in Alt+Tab (F-17)
            hIcon: crate::shell::app_icon(GetSystemMetrics(SM_CXICON)),
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: name.as_ptr(),
            hIconSm: crate::shell::app_icon(GetSystemMetrics(SM_CXSMICON)),
        };
        RegisterClassExW(&wc);
    }
}

fn taskbar_title(t: &str) -> String {
    let t = t.trim();
    if t.is_empty() {
        "제목 없음".into()
    } else {
        t.chars().take(80).collect()
    }
}

/// Opens the window for model note `id`. `at` = physical top-left; None = cascade on the primary monitor.
pub fn open(id: u64, at: Option<(i32, i32)>, focus: bool) -> HWND {
    let Some(n) = app::note(id) else { return std::ptr::null_mut() };
    let c = cfg();
    unsafe {
        let dpi = GetDpiForSystem();
        let (mut w, mut h) = (px(g::NOTE_W, dpi), px(g::NOTE_H, dpi));
        // the saved place (logical px), if it is still on a monitor (F-23 / F-27)
        let saved = if at.is_none() { n.rect.and_then(|r| on_screen(r, dpi)) } else { None };
        let (x, y) = match saved {
            Some((sx, sy, sw, sh)) => {
                w = sw.max(px(g::MIN_W, dpi));
                h = sh.max(px(g::MIN_H, dpi));
                (sx, sy)
            }
            None => at.unwrap_or_else(|| {
                let step = app::open_windows().len() as i32 % 8;
                let sw = GetSystemMetrics(SM_CXSCREEN);
                let sh = GetSystemMetrics(SM_CYSCREEN);
                ((sw - w) / 2 + step * px(28.0, dpi), (sh - h) / 2 + step * px(28.0, dpi))
            }),
        };
        let state = Box::new(NoteState {
            hwnd: std::ptr::null_mut(),
            id,
            body: std::ptr::null_mut(),
            title_edit: std::ptr::null_mut(),
            dpi,
            hover: false,
            hot: None,
            down: None,
            copied: false,
            menu: std::ptr::null_mut(),
            fmt_on: [false; 5],
            painted: false,
            last_save: None,
            save_pending: false,
            saved_chars: 0,
            d2d: HwndTarget::default(),
            normal_h: h,
            pic_width: 0.0,
            sizing: false,
        });
        let ex = if c.taskbar { WS_EX_APPWINDOW } else { WS_EX_TOOLWINDOW } | if n.pinned { WS_EX_TOPMOST } else { 0 };
        let style = WS_POPUP | WS_THICKFRAME | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX | WS_CLIPCHILDREN;
        let title = wide(&taskbar_title(&n.title));
        let hwnd = CreateWindowExW(ex, wide(CLASS).as_ptr(), title.as_ptr(), style, x, y, w, h, std::ptr::null_mut(), std::ptr::null_mut(), GetModuleHandleW(std::ptr::null()), Box::into_raw(state) as *const _);
        if hwnd.is_null() {
            crate::log::write(&format!("note window: create failed ({})", windows_sys::Win32::Foundation::GetLastError()));
            return hwnd;
        }
        NOTES.with(|v| v.borrow_mut().push(hwnd));
        app::with(|a| a.windows.insert(id, hwnd));
        if n.folded {
            if let Some(s) = state_of(hwnd) {
                s.normal_h = h;
                SetWindowPos(hwnd, std::ptr::null_mut(), 0, 0, w, folded_h(s.dpi), SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
                relayout(s);
            }
        }
        ShowWindow(hwnd, if focus { SW_SHOW } else { SW_SHOWNOACTIVATE });
        // first paint now, not whenever the queue drains (no blank frame on screen)
        windows_sys::Win32::Graphics::Gdi::UpdateWindow(hwnd);
        if n.maximized && !crate::quiet() {
            if let Some(s) = state_of(hwnd) {
                set_full(s, true); // 전체화면 comes back after a restart (F-29)
            }
        }
        if focus {
            SetForegroundWindow(hwnd);
            if let Some(s) = state_of(hwnd) {
                if !n.folded && !n.locked && !s.body.is_null() {
                    SetFocus(s.body);
                }
            }
        }
        hwnd
    }
}

/// P0 harness entry: a sample note `i` at physical (x, y) (bench / ptd modes).
pub fn create(i: usize, x: i32, y: i32, show: bool) -> HWND {
    let id = app::add(&format!("메모 {}", i + 1), body::SAMPLE, Some(i % 6), 0);
    let h = open(id, Some((x, y)), false);
    if !show {
        unsafe { ShowWindow(h, SW_HIDE) };
    }
    h
}

pub unsafe fn state_of<'a>(hwnd: HWND) -> Option<&'a mut NoteState> {
    (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut NoteState).as_mut()
}

pub fn body_of(hwnd: HWND) -> HWND {
    unsafe { state_of(hwnd).map(|s| s.body).unwrap_or(std::ptr::null_mut()) }
}

fn client(hwnd: HWND) -> (i32, i32) {
    let mut r = rect(0, 0, 0, 0);
    unsafe { GetClientRect(hwnd, &mut r) };
    (r.right, r.bottom)
}

fn is_max(hwnd: HWND) -> bool {
    unsafe { IsZoomed(hwnd) != 0 }
}

thread_local! {
    /// screenshot mode: draw a note as 전체화면 without maximising it
    pub static FORCE_FULL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// "전체화면": maximised, or dragged out to within 16 px of the work area (v1.1 is_full).
pub fn is_full(hwnd: HWND) -> bool {
    if is_max(hwnd) || FORCE_FULL.with(|f| f.get()) {
        return true;
    }
    unsafe {
        let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(mon, &mut mi) == 0 {
            return false;
        }
        let mut r = rect(0, 0, 0, 0);
        GetWindowRect(hwnd, &mut r);
        let slack = px(16.0, GetDpiForWindow(hwnd));
        r.right - r.left + slack >= mi.rcWork.right - mi.rcWork.left && r.bottom - r.top + slack >= mi.rcWork.bottom - mi.rcWork.top
    }
}

fn frame(dpi: u32) -> i32 {
    unsafe { GetSystemMetricsForDpi(SM_CXSIZEFRAME, dpi) + GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi) }
}

pub fn folded_h(dpi: u32) -> i32 {
    px(app::band().strip() + g::BAR, dpi)
}

// ---------------------------------------------------------------- layout

pub struct Layout {
    pub strip: i32,
    pub bar: RECT,
    pub tools: Vec<(Tool, RECT)>,
    pub title_x: i32,
    pub title_right: i32,
    pub fmt_row: Option<RECT>,
    pub fmt: Vec<(Fmt, RECT)>,
    pub seps: Vec<RECT>,
    pub body_top: i32,
}

pub fn layout(w: i32, dpi: u32, n: &app::Note, full: bool, tools_visible: bool) -> Layout {
    let strip = px(app::band().strip(), dpi);
    let bar = rect(0, strip, w, strip + px(g::BAR, dpi));
    let t = px(g::TOOL, dpi);
    let gap = px(g::TOOL_GAP, dpi);
    let top = bar.top + (bar.bottom - bar.top - t) / 2;
    let mut kinds = vec![Tool::Pin, Tool::New, Tool::More];
    if full {
        kinds.push(Tool::Restore);
    }
    kinds.push(Tool::Close);
    let mut tools = Vec::new();
    let mut right = w - px(g::BAR_PAD_R, dpi);
    for k in kinds.iter().rev() {
        tools.push((*k, rect(right - t, top, right, top + t)));
        right -= t + gap;
    }
    tools.reverse();
    let title_right = if tools_visible { tools[0].1.left - px(g::BAR_GAP, dpi) } else { w - px(g::BAR_PAD_R, dpi) };
    let mut title_x = px(g::BAR_PAD_L, dpi);
    if n.locked {
        title_x += px(g::ICON + g::BAR_GAP, dpi);
    }
    let mut fmt = Vec::new();
    let mut seps = Vec::new();
    let mut fmt_row = None;
    let mut body_top = bar.bottom;
    if full && !n.locked && !n.folded {
        let row_h = px(g::FMT_PAD_Y * 2.0 + g::FMT_BTN_H, dpi);
        let row = rect(0, bar.bottom, w, bar.bottom + row_h + px(1.0, dpi).max(1));
        let (bw, bh) = (px(g::FMT_BTN_W, dpi), px(g::FMT_BTN_H, dpi));
        let y = bar.bottom + px(g::FMT_PAD_Y, dpi);
        let mut x = px(g::FMT_PAD_X, dpi);
        for (i, f) in [Fmt::Bold, Fmt::Italic, Fmt::Underline, Fmt::Strike, Fmt::Bullets, Fmt::Image].into_iter().enumerate() {
            if i == 4 || i == 5 {
                let m = px(g::FMT_SEP_M, dpi);
                let sh = px(16.0, dpi);
                seps.push(rect(x + m, y + (bh - sh) / 2, x + m + px(1.0, dpi).max(1), y + (bh - sh) / 2 + sh));
                x += 2 * m + px(1.0, dpi).max(1);
            }
            fmt.push((f, rect(x, y, x + bw, y + bh)));
            x += bw + px(g::TOOL_GAP, dpi);
        }
        body_top = row.bottom;
        fmt_row = Some(row);
    }
    Layout { strip, bar, tools, title_x, title_right, fmt_row, fmt, seps, body_top }
}

fn relayout(s: &mut NoteState) {
    let (w, h) = client(s.hwnd);
    let Some(n) = app::note(s.id) else { return };
    let full = is_full(s.hwnd);
    let l = layout(w, s.dpi, &n, full, s.hover);
    if !s.body.is_null() {
        if n.folded {
            body::set_visible(s.body, false);
        } else {
            // inside the 1 px edge; padding is the RichEdit format rect (body::set_padding)
            let one = px(1.0, s.dpi).max(1);
            body::place(s.body, one, l.body_top, (w - 2 * one).max(1), (h - l.body_top - one).max(1));
            body::set_visible(s.body, true);
        }
    }
    if !s.title_edit.is_null() {
        place_title_edit(s, &l);
    }
    s.d2d.resize(w, h);
}

// ---------------------------------------------------------------- painting

thread_local! {
    // P6 diagnostic: µs in draw: layout, title font, title drawn, to then
    pub static DRAW_TRACE: std::cell::RefCell<Vec<[f64; 4]>> = const { std::cell::RefCell::new(Vec::new()) };
    // P6 diagnostic: µs of each note paint (measurement modes only)
    pub static PAINT_TRACE: std::cell::RefCell<Vec<f64>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn paint(s: &mut NoteState) {
    let t0 = qpc();
    let Some(n) = app::note(s.id) else { return };
    let full = is_full(s.hwnd);
    if cfg().render != RenderKind::Gdi {
        // P0 comparison paths (bench only)
        unsafe {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let hdc = BeginPaint(s.hwnd, &mut ps);
            let (w, h) = client(s.hwnd);
            let title = ws(&n.title);
            let c = Chrome { w, h, dpi: s.dpi, accent: PALETTE[n.color], title: &title, tools: s.hover, hot: None, pinned: n.pinned, full };
            match cfg().render {
                RenderKind::D2dHwnd => render::paint_d2d_hwnd(s.hwnd, &mut s.d2d, &c),
                _ => render::paint_d2d_dc(hdc, &c),
            }
            EndPaint(s.hwnd, &ps);
        }
    } else {
        let hover = s.hover || !s.menu.is_null();
        let renaming = !s.title_edit.is_null();
        gfx::paint_buffered(s.hwnd, |hdc, w, h| {
            draw(hdc, w, h, s.dpi, &n, full, hover, s.hot, s.down, s.copied, &s.fmt_on);
            if renaming {
                let (bx, _) = title_box(s, &layout(w, s.dpi, &n, full, hover));
                gfx::fill(hdc, &bx, title_edit_colors(s.id).0);
            }
        });
    }
    let us = qpc_us(qpc() - t0);
    if crate::metrics::diag() {
        PAINT_TRACE.with(|t| t.borrow_mut().push(us));
    }
    let first = !s.painted;
    s.painted = true;
    stats::with(|st| {
        st.paints += 1;
        st.paint_us_total += us;
        st.paint_us_max = st.paint_us_max.max(us);
        if first {
            st.first_paints += 1;
        }
    });
}

/// Pure drawing of the chrome (also used by the screenshot mode).
#[allow(clippy::too_many_arguments)]
pub fn draw(hdc: HDC, w: i32, h: i32, dpi: u32, n: &app::Note, full: bool, hover: bool, hot: Option<Part>, down: Option<Part>, copied: bool, fmt_on: &[bool; 5]) {
    let tq = std::time::Instant::now();
    let p = pal(app::theme());
    let band = app::band();
    let accent = PALETTE[n.color];
    let l = layout(w, dpi, n, full, hover);
    let lay_us = tq.elapsed().as_secs_f64() * 1e6;
    gfx::fill(hdc, &rect(0, 0, w, h), p.paper);
    if l.strip > 0 {
        gfx::fill(hdc, &rect(0, 0, w, l.strip), accent);
    }
    let on_band = band == Band::Full;
    if on_band {
        gfx::fill(hdc, &l.bar, accent);
    }
    let (tool_c, tool_hot_c, tool_hot_bg) = if on_band { (ON_ACCENT_SOFT, ON_ACCENT, mix(accent, 0, 0.8)) } else { (p.muted, p.ink, p.paper3) };
    let title_c = if on_band { ON_ACCENT } else { p.ink };
    let soft_c = if on_band { ON_ACCENT_SOFT } else { p.muted };

    // title, lock, "✓ 복사됨"
    let x0 = px(g::BAR_PAD_L, dpi);
    let icon = px(g::ICON, dpi);
    let iy = l.bar.top + (l.bar.bottom - l.bar.top - icon) / 2;
    if copied {
        icons::draw(hdc, Icon::Check, x0, iy, g::ICON, dpi, soft_c, false);
        let tx = x0 + icon + px(4.0, dpi);
        gfx::text(hdc, &ws("복사됨"), &rect(tx, l.bar.top, l.title_right, l.bar.bottom), gfx::font(FontSpec::px(130), dpi), soft_c, DT_SINGLELINE | DT_VCENTER);
    } else {
        if n.locked {
            icons::draw(hdc, Icon::Lock, x0, iy, g::ICON, dpi, soft_c, false);
        }
        let empty = n.title.is_empty();
        let txt = if empty { ws("제목") } else { ws(&n.title) };
        let tf = std::time::Instant::now();
        let f = gfx::font(FontSpec::bold(135), dpi);
        let font_us = tf.elapsed().as_secs_f64() * 1e6;
        gfx::text(hdc, &txt, &rect(l.title_x, l.bar.top, l.title_right, l.bar.bottom), f, if empty { soft_c } else { title_c }, DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS);
        if crate::metrics::diag() {
            DRAW_TRACE.with(|t| t.borrow_mut().push([lay_us, font_us, tf.elapsed().as_secs_f64() * 1e6, tq.elapsed().as_secs_f64() * 1e6]));
        }
    }

    // tools (only while hovered)
    if hover {
        for (t, r) in &l.tools {
            let is_hot = hot == Some(Part::Tool(*t));
            let pressed = *t == Tool::Pin && n.pinned;
            if is_hot {
                let bg = if down == Some(Part::Tool(*t)) && !on_band { p.rule } else { tool_hot_bg };
                gfx::fill(hdc, r, bg);
            }
            let c = if is_hot || pressed { tool_hot_c } else { tool_c };
            let ic = match t {
                Tool::Pin => Icon::Pin,
                Tool::New => Icon::New,
                Tool::More => Icon::More,
                Tool::Restore => Icon::Restore,
                Tool::Close => Icon::Close,
            };
            icons::draw_centered(hdc, ic, r, g::ICON, dpi, c, pressed);
        }
    }

    // formatting row (전체화면, not locked)
    if let Some(row) = l.fmt_row {
        let one = px(1.0, dpi).max(1);
        gfx::fill(hdc, &rect(0, row.bottom - one, w, row.bottom), p.rule);
        for s in &l.seps {
            gfx::fill(hdc, s, p.rule);
        }
        for (i, (f, r)) in l.fmt.iter().enumerate() {
            let is_hot = hot == Some(Part::Fmt(*f));
            let on = i < 5 && fmt_on[i];
            if is_hot || on {
                gfx::fill(hdc, r, if is_hot && down == Some(Part::Fmt(*f)) { p.rule } else { p.paper3 });
            }
            let c = if is_hot || on { p.ink } else { p.muted };
            match f {
                Fmt::Bullets => icons::draw_centered(hdc, Icon::Bullets, r, 16.0, dpi, c, false),
                Fmt::Image => icons::draw_centered(hdc, Icon::Image, r, 16.0, dpi, c, false),
                _ => {
                    let mut spec = FontSpec::px(140);
                    let label = match f {
                        Fmt::Bold => {
                            spec.bold = true;
                            "B"
                        }
                        Fmt::Italic => {
                            spec.italic = true;
                            "I"
                        }
                        Fmt::Underline => {
                            spec.underline = true;
                            "U"
                        }
                        _ => {
                            spec.strike = true;
                            "S"
                        }
                    };
                    gfx::text(hdc, &ws(label), r, gfx::font(spec, dpi), c, DT_SINGLELINE | DT_VCENTER | DT_CENTER);
                }
            }
        }
    }
    // 1 px black edge over everything (html::after)
    gfx::frame(hdc, &rect(0, 0, w, h), EDGE);
}

fn invalidate_bar(s: &NoteState) {
    let (w, _) = client(s.hwnd);
    let bottom = px(app::band().strip() + g::BAR, s.dpi) + px(40.0, s.dpi);
    unsafe { InvalidateRect(s.hwnd, &rect(0, 0, w, bottom), 0) };
}

// ---------------------------------------------------------------- behaviour

fn part_at(s: &NoteState, x: i32, y: i32) -> Option<Part> {
    let (w, _) = client(s.hwnd);
    let n = app::note(s.id)?;
    let l = layout(w, s.dpi, &n, is_full(s.hwnd), true);
    let inr = |r: &RECT| x >= r.left && x < r.right && y >= r.top && y < r.bottom;
    if s.hover {
        if let Some((t, _)) = l.tools.iter().find(|(_, r)| inr(r)) {
            return Some(Part::Tool(*t));
        }
    }
    l.fmt.iter().find(|(_, r)| inr(r)).map(|(f, _)| Part::Fmt(*f))
}

pub fn hit(s: &NoteState, sx: i32, sy: i32) -> u32 {
    let mut p = POINT { x: sx, y: sy };
    unsafe { ScreenToClient(s.hwnd, &mut p) };
    let (w, h) = client(s.hwnd);
    let Some(n) = app::note(s.id) else { return HTCLIENT };
    let bar_bottom = px(app::band().strip() + g::BAR, s.dpi);
    if !is_max(s.hwnd) {
        let b = frame(s.dpi);
        let (top, bottom, left, right) = (p.y < b, p.y >= h - b, p.x < b, p.x >= w - b);
        if top {
            // v1 rule: the middle of the top edge drags; corners keep diagonal resize
            return if p.x < 2 * b {
                HTTOPLEFT
            } else if p.x >= w - 2 * b {
                HTTOPRIGHT
            } else {
                HTCAPTION
            };
        }
        if n.folded {
            return if left {
                HTLEFT
            } else if right {
                HTRIGHT
            } else if p.y < bar_bottom && s.hover && part_at(s, p.x, p.y).is_some() {
                HTCLIENT
            } else {
                HTCAPTION
            };
        }
        let _ = (bottom, left, right);
        // left / bottom / right bands (6 CSS px, corners 12) also over the body and its scroll
        // bar (they answer HTTRANSPARENT there); along a scroll bar only its outer 3 px
        let edge = px(6.0, s.dpi);
        let edge_right = if crate::body::bar_visible(s.body) { px(3.0, s.dpi) } else { edge };
        if let Some(code) = crate::win::edge_hit(p.x, p.y, w, h, edge, edge_right, px(12.0, s.dpi)) {
            return code;
        }
    }
    if p.y < bar_bottom {
        if !s.title_edit.is_null() {
            let mut r = rect(0, 0, 0, 0);
            unsafe {
                GetWindowRect(s.title_edit, &mut r);
                if sx >= r.left && sx < r.right && sy >= r.top && sy < r.bottom {
                    return HTCLIENT;
                }
            }
        }
        if part_at(s, p.x, p.y).is_some() {
            return HTCLIENT;
        }
        return HTCAPTION;
    }
    HTCLIENT
}

/// A saved logical rectangle in physical px, or None when no monitor shows any of it
/// (a monitor that was unplugged: the note opens in the default place instead).
fn on_screen(r: (f64, f64, f64, f64), dpi: u32) -> Option<(i32, i32, i32, i32)> {
    let k = dpi as f64 / 96.0;
    let (x, y, w, h) = ((r.0 * k).round() as i32, (r.1 * k).round() as i32, (r.2 * k).round() as i32, (r.3 * k).round() as i32);
    if w <= 0 || h <= 0 {
        return None;
    }
    // the top band (where the note is dragged) must be reachable
    let grab = rect(x, y, x + w, y + px(g::BAR, dpi));
    let mon = unsafe { MonitorFromRect(&grab, MONITOR_DEFAULTTONULL) };
    (!mon.is_null()).then_some((x, y, w, h))
}

/// Saves the normal window rectangle (logical px) after a move or resize (F-27).
fn save_rect(s: &mut NoteState) {
    if is_full(s.hwnd) || unsafe { IsIconic(s.hwnd) } != 0 {
        return;
    }
    let mut r = rect(0, 0, 0, 0);
    unsafe { GetWindowRect(s.hwnd, &mut r) };
    let folded = app::note(s.id).is_some_and(|n| n.folded);
    let h = if folded { s.normal_h } else { r.bottom - r.top };
    let k = 96.0 / s.dpi as f64;
    let l = |v: i32| (v as f64 * k * 10.0).round() / 10.0;
    app::set_rect(s.id, (l(r.left), l(r.top), l(r.right - r.left), l(h)));
}

pub fn toggle_fold(s: &mut NoteState) {
    unsafe {
        let mut r = rect(0, 0, 0, 0);
        GetWindowRect(s.hwnd, &mut r);
        let folded = !app::note(s.id).map(|n| n.folded).unwrap_or(false);
        if folded {
            s.normal_h = r.bottom - r.top;
        }
        app::update(s.id, |n| n.folded = folded);
        let h = if folded { folded_h(s.dpi) } else { s.normal_h.max(px(g::MIN_H, s.dpi)) };
        SetWindowPos(s.hwnd, std::ptr::null_mut(), 0, 0, r.right - r.left, h, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
        relayout(s);
        InvalidateRect(s.hwnd, std::ptr::null(), 0);
    }
}

pub fn set_full(s: &mut NoteState, on: bool) {
    unsafe {
        if on {
            if app::note(s.id).map(|n| n.folded).unwrap_or(false) {
                toggle_fold(s);
            }
            ShowWindow(s.hwnd, SW_MAXIMIZE);
        } else {
            ShowWindow(s.hwnd, SW_RESTORE);
            if is_full(s.hwnd) {
                // dragged out to full size by hand: back to the default size
                SetWindowPos(s.hwnd, std::ptr::null_mut(), 0, 0, px(g::NOTE_W, s.dpi), s.normal_h.max(px(g::NOTE_H, s.dpi)), SWP_NOMOVE | SWP_NOZORDER);
            }
        }
    }
}

fn on_body_change(s: &mut NoteState) {
    if s.save_pending {
        return;
    }
    let due = s.last_save.map(|t| t.elapsed() >= SAVE_DELAY).unwrap_or(true);
    if due {
        serialize(s);
    } else {
        let wait = SAVE_DELAY.saturating_sub(s.last_save.unwrap().elapsed());
        s.save_pending = true;
        unsafe { SetTimer(s.hwnd, TIMER_SAVE, wait.as_millis().max(1) as u32, None) };
    }
}

/// `rich::save` (report §6.4): the body as v1.1 formatted HTML plus its plain-text copy, into
/// the in-memory model (SQLite in P3). The plain EDIT body of the PT-C bench has no HTML.
fn serialize(s: &mut NoteState) {
    let t0 = Instant::now();
    serialize_inner(s);
    let us = t0.elapsed().as_secs_f64() * 1e6;
    stats::with(|st| {
        st.ser_us_total += us;
        st.sers += 1;
        st.ser_us_max = st.ser_us_max.max(us);
    });
}

fn serialize_inner(s: &mut NoteState) {
    let (html, text) = if cfg().body == crate::body::BodyKind::RichEdit {
        crate::rich::stored(&crate::richedit::save(s.body))
    } else {
        (String::new(), String::from_utf16_lossy(&body::plain(s.body)).replace("\r\n", "\n").replace('\r', "\n"))
    };
    s.saved_chars = text.len();
    s.last_save = Some(Instant::now());
    // v1.1 note_save: only a real change is written and moves the note up the list (a
    // v1.0 plain-text note stays plain until its text changes)
    let Some(n) = app::note(s.id) else { return };
    let same_html = html == n.html || (n.html.is_empty() && html == crate::rich::Doc::from_plain(&text).to_html());
    if text == n.text && same_html {
        return;
    }
    app::update(s.id, |n| {
        n.text = text;
        n.html = html;
    });
    app::touch(s.id);
    stats::with(|st| st.saves += 1);
    crate::images::release_written();
}

/// 사진 넣기 (v1.1 file picker: png / jpeg / gif / webp, several at once).
fn pick_pictures(s: &mut NoteState) {
    if crate::quiet() {
        return;
    }
    for path in crate::images::pick_files(s.hwnd) {
        let Ok(bytes) = std::fs::read(&path) else { continue };
        if let Some(name) = crate::images::add(bytes) {
            crate::richedit::insert_picture(s.body, &name);
        }
    }
    on_body_change(s);
    unsafe { SetFocus(s.body) };
}

pub fn click(s: &mut NoteState, p: Part) {
    let id = s.id;
    match p {
        Part::Tool(Tool::Pin) => {
            let pinned = !app::note(id).map(|n| n.pinned).unwrap_or(false);
            app::update(id, |n| n.pinned = pinned);
            unsafe { SetWindowPos(s.hwnd, if pinned { HWND_TOPMOST } else { HWND_NOTOPMOST }, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE) };
            invalidate_bar(s);
        }
        Part::Tool(Tool::New) => {
            app::new_note(Some(s.hwnd));
        }
        Part::Tool(Tool::More) => open_menu(s),
        Part::Tool(Tool::Restore) => set_full(s, false),
        Part::Tool(Tool::Close) => unsafe {
            PostMessageW(s.hwnd, WM_CLOSE, 0, 0);
        },
        Part::Fmt(Fmt::Image) => pick_pictures(s),
        Part::Fmt(f) => {
            body::toggle(s.body, f);
            s.fmt_on = body::format_state(s.body);
            unsafe { SetFocus(s.body) };
            invalidate_bar(s);
            on_body_change(s); // formatting gives no EN_CHANGE
        }
    }
}

pub fn open_menu(s: &mut NoteState) {
    if !s.menu.is_null() {
        unsafe { DestroyWindow(s.menu) };
        return;
    }
    if app::note(s.id).map(|n| n.folded).unwrap_or(false) {
        toggle_fold(s); // v1.1: the menu unfolds the note first
    }
    s.menu = menu::show_note_menu(s.hwnd, s.id, is_full(s.hwnd));
    invalidate_bar(s);
}

fn on_menu(s: &mut NoteState, act: menu::Act) {
    use menu::Act;
    let id = s.id;
    match act {
        Act::Rename => start_rename(s),
        Act::Color(i) => {
            app::update(id, |n| n.color = i);
            unsafe { InvalidateRect(s.hwnd, std::ptr::null(), 0) };
            list::refresh_one(id);
            if !s.menu.is_null() {
                menu::refresh_note_menu(s.menu, id, is_full(s.hwnd));
            }
        }
        Act::Band => {
            app::cycle_band();
            if !s.menu.is_null() {
                menu::refresh_note_menu(s.menu, id, is_full(s.hwnd));
                menu::reposition_note_menu(s.menu, s.hwnd);
            }
        }
        Act::Full => {
            let full = is_full(s.hwnd);
            set_full(s, !full);
        }
        Act::Lock => {
            let locked = !app::note(id).map(|n| n.locked).unwrap_or(false);
            if locked {
                if s.save_pending {
                    unsafe { KillTimer(s.hwnd, TIMER_SAVE) };
                    s.save_pending = false;
                }
                crate::win::ime_complete(s.body); // a Hangul syllable still being composed counts
                serialize(s); // keep the last keystrokes before freezing
                end_rename(s, false);
            }
            app::update(id, |n| n.locked = locked);
            body::set_locked(s.body, locked);
            relayout(s);
            unsafe { InvalidateRect(s.hwnd, std::ptr::null(), 0) };
            list::refresh_one(id);
        }
        Act::Theme => app::set_theme(app::theme().toggled()),
        Act::List => list::open(true),
        Act::Delete => request_delete(s.hwnd, id),
        Act::ListDelete => {}
    }
}

/// "메모 삭제": empty notes go at once; otherwise the 삭제하시겠습니까? card decides.
pub fn request_delete(owner: HWND, id: u64) {
    let Some(n) = app::note(id) else { return };
    if n.locked {
        return;
    }
    if n.is_empty() {
        app::delete_note(id);
    } else {
        confirm::show(owner, id);
    }
}

// ---------------------------------------------------------------- title rename

/// The rename box: `.title:not([readonly])` is 26 px high with the text centred on a
/// 26 px line. A single-line EDIT draws its text at the top, so the EDIT is only one
/// text line high, centred in the box, and the note paints the box around it.
fn title_box(s: &NoteState, l: &Layout) -> (RECT, RECT) {
    let pad = px(6.0, s.dpi);
    let h = px(26.0, s.dpi);
    let y = l.bar.top + (l.bar.bottom - l.bar.top - h) / 2;
    let x = px(g::BAR_PAD_L, s.dpi) - pad;
    let right = x + (l.title_right - x).max(px(40.0, s.dpi));
    let bx = rect(x, y, right, y + h);
    let lh = unsafe {
        let dc = GetDC(s.hwnd);
        let v = gfx::line_h(dc, gfx::font(FontSpec::bold(135), s.dpi));
        ReleaseDC(s.hwnd, dc);
        v
    };
    let ey = y + (h - lh) / 2;
    (bx, rect(x, ey, right, ey + lh))
}

fn title_edit_colors(id: u64) -> (u32, u32) {
    let p = pal(app::theme());
    if app::band() == Band::Full {
        let accent = app::note(id).map(|n| PALETTE[n.color]).unwrap_or(p.paper2);
        (mix(accent, theme::rgb_white(), 0.7), ON_ACCENT)
    } else {
        (p.paper2, p.ink)
    }
}

fn place_title_edit(s: &NoteState, l: &Layout) {
    let pad = px(6.0, s.dpi);
    let (bx, e) = title_box(s, l);
    unsafe {
        SetWindowPos(s.title_edit, std::ptr::null_mut(), e.left, e.top, e.right - e.left, e.bottom - e.top, SWP_NOZORDER | SWP_NOACTIVATE);
        InvalidateRect(s.hwnd, &bx, 0);
    }
    let m = pad as usize;
    unsafe { SendMessageW(s.title_edit, windows_sys::Win32::UI::Controls::EM_SETMARGINS, 3, (m | (m << 16)) as isize) };
}

fn start_rename(s: &mut NoteState) {
    let Some(n) = app::note(s.id) else { return };
    if n.locked || !s.title_edit.is_null() {
        return;
    }
    unsafe {
        let style = WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS | ES_AUTOHSCROLL as u32;
        let e = CreateWindowExW(0, wide("EDIT").as_ptr(), wide(&n.title).as_ptr(), style, 0, 0, 10, 10, s.hwnd, std::ptr::null_mut(), GetModuleHandleW(std::ptr::null()), std::ptr::null());
        SendMessageW(e, WM_SETFONT, gfx::font(FontSpec::bold(135), s.dpi) as usize, 0);
        SendMessageW(e, windows_sys::Win32::UI::Controls::EM_SETLIMITTEXT, 500, 0);
        SetWindowSubclass(e, Some(title_proc), 1, 0);
        s.title_edit = e;
        let (w, _) = client(s.hwnd);
        let l = layout(w, s.dpi, &n, is_full(s.hwnd), s.hover);
        place_title_edit(s, &l);
        if !crate::quiet() {
            SetFocus(e);
        }
        SendMessageW(e, windows_sys::Win32::UI::Controls::EM_SETSEL, 0, -1);
    }
}

fn end_rename(s: &mut NoteState, to_body: bool) {
    if s.title_edit.is_null() {
        return;
    }
    // finish an IME composition first so its text is in the title (report §6.4)
    crate::win::ime_complete(s.title_edit);
    on_title_change(s);
    let e = std::mem::replace(&mut s.title_edit, std::ptr::null_mut());
    unsafe {
        DestroyWindow(e);
        if to_body {
            if app::note(s.id).map(|n| n.folded).unwrap_or(false) {
                toggle_fold(s);
            }
            SetFocus(s.body);
        }
        invalidate_bar(s);
    }
}

fn on_title_change(s: &mut NoteState) {
    let t = String::from_utf16_lossy(&body::plain(s.title_edit)).replace(['\r', '\n'], " ");
    if app::note(s.id).is_some_and(|n| n.title == t) {
        return; // nothing changed: keep the edit time (list order)
    }
    app::update(s.id, |n| n.title = t.clone());
    unsafe { SetWindowTextW(s.hwnd, wide(&taskbar_title(&t)).as_ptr()) };
    app::touch(s.id);
}

unsafe extern "system" fn title_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, _d: usize) -> LRESULT {
    let parent = GetParent(h);
    match msg {
        WM_KEYDOWN if wp == VK_RETURN as usize || wp == VK_ESCAPE as usize => {
            if let Some(s) = state_of(parent) {
                end_rename(s, wp == VK_RETURN as usize);
            }
            return 0;
        }
        WM_CHAR if wp == 0x0D || wp == 0x1B => return 0,
        WM_KILLFOCUS => {
            let r = DefSubclassProc(h, msg, wp, lp);
            PostMessageW(parent, WM_APP + 20, 0, 0); // end rename after focus settles
            return r;
        }
        WM_MOUSEMOVE => {
            SendMessageW(parent, WM_APP_HOVER, 0, 0);
            track(h, false);
        }
        WM_MOUSELEAVE_ => {
            PostMessageW(parent, WM_APP_LEAVE, 0, 0);
        }
        WM_NCDESTROY => {
            RemoveWindowSubclass(h, Some(title_proc), 1);
        }
        _ => {}
    }
    DefSubclassProc(h, msg, wp, lp)
}

// ---------------------------------------------------------------- hover tracking

pub fn track(h: HWND, nc: bool) {
    unsafe {
        let mut t = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE | if nc { TME_NONCLIENT } else { 0 }, hwndTrack: h, dwHoverTime: 0 };
        TrackMouseEvent(&mut t);
    }
}

fn set_hover(s: &mut NoteState, on: bool) {
    if s.hover != on {
        s.hover = on;
        if !on {
            s.hot = None;
        }
        // the title gets the whole bar when the tools hide
        if !s.title_edit.is_null() {
            relayout(s);
        }
        invalidate_bar(s);
    }
}

fn cursor_inside(hwnd: HWND) -> bool {
    unsafe {
        let mut p = POINT { x: 0, y: 0 };
        GetCursorPos(&mut p);
        let w = WindowFromPoint(p);
        !w.is_null() && (w == hwnd || GetAncestor(w, GA_ROOT) == hwnd)
    }
}

// ---------------------------------------------------------------- theme / band

pub fn apply_theme(hwnd: HWND) {
    unsafe {
        if let Some(s) = state_of(hwnd) {
            body::apply_theme(s.body);
            if !s.menu.is_null() {
                InvalidateRect(s.menu, std::ptr::null(), 0);
            }
        }
        RedrawWindow(hwnd, std::ptr::null(), std::ptr::null_mut(), RDW_INVALIDATE | RDW_ALLCHILDREN);
    }
}

pub fn apply_band(hwnd: HWND) {
    unsafe {
        if let Some(s) = state_of(hwnd) {
            if app::note(s.id).map(|n| n.folded).unwrap_or(false) {
                let mut r = rect(0, 0, 0, 0);
                GetWindowRect(hwnd, &mut r);
                SetWindowPos(hwnd, std::ptr::null_mut(), 0, 0, r.right - r.left, folded_h(s.dpi), SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
            }
            relayout(s);
        }
        InvalidateRect(hwnd, std::ptr::null(), 0);
    }
}

fn copy_to_clipboard(hwnd: HWND, text: &str) -> bool {
    use windows_sys::Win32::System::DataExchange::*;
    use windows_sys::Win32::System::Memory::*;
    let w: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        if OpenClipboard(hwnd) == 0 {
            return false;
        }
        EmptyClipboard();
        let mem = GlobalAlloc(GMEM_MOVEABLE, w.len() * 2);
        let ok = if !mem.is_null() {
            let p = GlobalLock(mem) as *mut u16;
            std::ptr::copy_nonoverlapping(w.as_ptr(), p, w.len());
            GlobalUnlock(mem);
            !SetClipboardData(13 /* CF_UNICODETEXT */, mem as _).is_null()
        } else {
            false
        };
        CloseClipboard();
        ok
    }
}

// ---------------------------------------------------------------- WndProc

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let cs = &*(lp as *const CREATESTRUCTW);
        let s = cs.lpCreateParams as *mut NoteState;
        (*s).hwnd = hwnd;
        (*s).dpi = GetDpiForWindow(hwnd);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, s as isize);
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    let Some(s) = state_of(hwnd) else { return DefWindowProcW(hwnd, msg, wp, lp) };
    match msg {
        WM_CREATE => {
            let pref: i32 = 1; // DWMWCP_DONOTROUND: square corners on Windows 11
            DwmSetWindowAttribute(hwnd, 33, &pref as *const i32 as *const _, 4);
            let tc = std::time::Instant::now();
            if let Some(n) = app::note(s.id) {
                let rich = cfg().body == crate::body::BodyKind::RichEdit;
                s.body = body::create(hwnd, cfg().body, s.dpi, if rich { "" } else { &n.text }, n.font_size);
                trace(2, tc.elapsed().as_secs_f64() * 1000.0);
                relayout(s);
                // filling the body is not an edit: no change notifications meanwhile
                let mask = if rich { SendMessageW(s.body, windows::Win32::UI::Controls::RichEdit::EM_SETEVENTMASK, 0, 0) } else { 0 };
                if rich {
                    // after layout: pictures are sized to the text width
                    let tl = std::time::Instant::now();
                    crate::richedit::load(s.body, &crate::rich::Doc::load(&n.html, &n.text));
                    trace(3, tl.elapsed().as_secs_f64() * 1000.0);
                    let tf = std::time::Instant::now();
                    body::set_font_size(s.body, n.font_size);
                    trace(4, tf.elapsed().as_secs_f64() * 1000.0);
                    s.pic_width = crate::richedit::text_css_width(s.body);
                }
                body::set_locked(s.body, n.locked);
                if rich {
                    SendMessageW(s.body, windows::Win32::UI::Controls::RichEdit::EM_SETEVENTMASK, 0, mask);
                }
            }
            relayout(s);
            trace(5, tc.elapsed().as_secs_f64() * 1000.0);
            WARMED.with(|w| {
                if !w.get() {
                    w.set(true);
                    SetTimer(hwnd, TIMER_WARM, 2000, None);
                }
            });
            0
        }
        // client = whole window, for both forms (wp = 0 is sent from CreateWindowEx;
        // leaving it to DefWindowProc would subtract a default frame until the next resize)
        WM_NCCALCSIZE => 0,
        // No non-client area (WM_NCCALCSIZE = 0), but on activation changes DefWindowProc
        // still paints the classic WS_THICKFRAME border over the client edges
        // (gray bevel on the left, right and bottom). lParam -1 = update state, don't draw.
        WM_NCACTIVATE => DefWindowProcW(hwnd, WM_NCACTIVATE, wp, -1),
        WM_NCPAINT => 0,
        WM_NCHITTEST => hit(s, x_of(lp), y_of(lp)) as LRESULT,
        WM_NCLBUTTONDBLCLK if wp == HTCAPTION as usize => {
            if is_full(hwnd) {
                set_full(s, false);
            } else {
                toggle_fold(s);
            }
            0
        }
        WM_GETMINMAXINFO => {
            let mmi = &mut *(lp as *mut MINMAXINFO);
            let folded = app::note(s.id).map(|n| n.folded).unwrap_or(false);
            mmi.ptMinTrackSize.x = px(g::MIN_W, s.dpi);
            mmi.ptMinTrackSize.y = if folded { folded_h(s.dpi) } else { px(g::MIN_H, s.dpi) };
            if folded {
                mmi.ptMaxTrackSize.y = folded_h(s.dpi);
            }
            let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
            let mut mi: MONITORINFO = std::mem::zeroed();
            mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
            if GetMonitorInfoW(mon, &mut mi) != 0 {
                mmi.ptMaxPosition = POINT { x: mi.rcWork.left - mi.rcMonitor.left, y: mi.rcWork.top - mi.rcMonitor.top };
                mmi.ptMaxSize = POINT { x: mi.rcWork.right - mi.rcWork.left, y: mi.rcWork.bottom - mi.rcWork.top };
            }
            0
        }
        WM_SIZE => {
            // the body is held while it takes the new size and its pictures follow, so it
            // shows only the finished frame (no step with the old picture size)
            let frozen = if s.body.is_null() { None } else { crate::richedit::freeze(s.body) };
            relayout(s);
            if let Some(d) = frozen {
                crate::richedit::live_refit(s.body);
                crate::richedit::unfreeze(&d);
            }
            InvalidateRect(hwnd, std::ptr::null(), 0);
            // the finished frame now, before the size loop goes on and DWM shows the window
            // (otherwise the new size could show half-painted)
            RedrawWindow(hwnd, std::ptr::null(), std::ptr::null_mut(), RDW_UPDATENOW | RDW_ALLCHILDREN);
            if !s.menu.is_null() {
                menu::reposition_note_menu(s.menu, hwnd);
            }
            crate::confirm::resize(hwnd);
            // the sharp picture copies once the size has settled: after the drag ends
            // (WM_EXITSIZEMOVE) — a pause in the middle of a drag swapped soft and sharp
            // copies back and forth, which flashed (user report 2026-11-16) — or 150 ms after
            // a size change without a drag (maximise, 전체화면, Win+arrows)
            if s.pic_width > 0.0 && !s.sizing {
                SetTimer(hwnd, TIMER_REFIT, 150, None);
            }
            // 전체화면 by the menu, the icon, Win+↑ or by dragging out: remembered (F-29)
            let full = is_full(hwnd) && wp as u32 != SIZE_MINIMIZED;
            if wp as u32 != SIZE_MINIMIZED && app::note(s.id).is_some_and(|n| n.maximized != full) {
                app::update(s.id, |n| n.maximized = full);
            }
            0
        }
        WM_ENTERSIZEMOVE => {
            s.sizing = true;
            KillTimer(hwnd, TIMER_REFIT);
            0
        }
        WM_EXITSIZEMOVE => {
            s.sizing = false;
            save_rect(s);
            if s.pic_width > 0.0 {
                SetTimer(hwnd, TIMER_REFIT, 1, None); // sharp copies now that the drag is over
            }
            0
        }
        WM_QUERYENDSESSION => {
            // log off / shut down: what is typed so far is committed first (report §10.3)
            crate::win::ime_complete(s.body);
            if s.save_pending {
                KillTimer(hwnd, TIMER_SAVE);
                s.save_pending = false;
                serialize(s);
            }
            crate::store::flush();
            1
        }
        WM_MOVE => {
            if !s.menu.is_null() {
                menu::reposition_note_menu(s.menu, hwnd);
            }
            0
        }
        WM_DPICHANGED => {
            s.dpi = (wp & 0xFFFF) as u32;
            let r = &*(lp as *const RECT);
            SetWindowPos(hwnd, std::ptr::null_mut(), r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
            body::set_dpi(s.body, s.dpi);
            relayout(s);
            0
        }
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            paint(s);
            0
        }
        WM_PRINTCLIENT => {
            gfx::print_into(wp as HDC, || paint(s));
            0
        }
        WM_MOUSEMOVE => {
            track(hwnd, false);
            set_hover(s, true);
            let hot = part_at(s, x_of(lp), y_of(lp));
            if hot != s.hot {
                s.hot = hot;
                invalidate_bar(s);
            }
            0
        }
        WM_NCMOUSEMOVE => {
            track(hwnd, true);
            set_hover(s, true);
            if s.hot.is_some() {
                s.hot = None;
                invalidate_bar(s);
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_APP_HOVER => {
            set_hover(s, true);
            if s.hot.is_some() {
                s.hot = None;
                invalidate_bar(s);
            }
            0
        }
        WM_MOUSELEAVE_ | WM_NCMOUSELEAVE_ | WM_APP_LEAVE => {
            if !cursor_inside(hwnd) && s.menu.is_null() {
                set_hover(s, false);
            } else if s.hot.is_some() && msg != WM_APP_LEAVE {
                s.hot = None;
                invalidate_bar(s);
            }
            0
        }
        WM_LBUTTONDOWN => {
            if let Some(p) = part_at(s, x_of(lp), y_of(lp)) {
                s.down = Some(p);
                SetCapture(hwnd);
                invalidate_bar(s);
            }
            0
        }
        WM_LBUTTONUP => {
            if let Some(p) = s.down.take() {
                ReleaseCapture();
                invalidate_bar(s);
                if part_at(s, x_of(lp), y_of(lp)) == Some(p) {
                    click(s, p);
                }
            }
            0
        }
        WM_COMMAND => {
            let code = ((wp >> 16) & 0xFFFF) as u32;
            if lp as HWND == s.body && code == EN_CHANGE {
                let c0 = crate::metrics::prof_start();
                on_body_change(s);
                crate::metrics::prof_add(4, c0);
            } else if lp as HWND == s.title_edit && code == EN_CHANGE {
                on_title_change(s);
            }
            0
        }
        WM_NOTIFY => {
            let hdr = &*(lp as *const windows_sys::Win32::UI::Controls::NMHDR);
            let c0 = crate::metrics::prof_start();
            let _guard = ProfGuard(5, c0);
            if hdr.hwndFrom == s.body && hdr.code == 0x0702 /* EN_SELCHANGE */ && is_full(hwnd) {
                let st = body::format_state(s.body);
                if st != s.fmt_on {
                    s.fmt_on = st;
                    invalidate_bar(s);
                }
            }
            0
        }
        WM_CTLCOLOREDIT => {
            let hdc = wp as HDC;
            let p = pal(app::theme());
            if lp as HWND == s.title_edit {
                let (bg, fg) = title_edit_colors(s.id);
                SetTextColor(hdc, fg);
                SetBkColor(hdc, bg);
                return gfx::brush(bg) as LRESULT;
            }
            SetTextColor(hdc, p.ink);
            SetBkColor(hdc, p.paper);
            gfx::brush(p.paper) as LRESULT
        }
        WM_TIMER if wp == TIMER_SAVE => {
            KillTimer(hwnd, TIMER_SAVE);
            s.save_pending = false;
            serialize(s);
            0
        }
        WM_TIMER if wp == TIMER_WARM => {
            KillTimer(hwnd, TIMER_WARM);
            crate::images::warm(s.dpi);
            0
        }
        WM_TIMER if wp == TIMER_REFIT => {
            KillTimer(hwnd, TIMER_REFIT);
            s.pic_width = crate::richedit::refit_pictures(s.body, s.pic_width);
            0
        }
        WM_TIMER if wp == TIMER_COPIED => {
            KillTimer(hwnd, TIMER_COPIED);
            s.copied = false;
            invalidate_bar(s);
            0
        }
        WM_APP_COPY => {
            let text = app::note(s.id).map(|n| n.text).unwrap_or_default();
            if copy_to_clipboard(hwnd, &text.replace('\n', "\r\n")) {
                s.copied = true;
                invalidate_bar(s);
                SetTimer(hwnd, TIMER_COPIED, 1000, None);
            }
            0
        }
        WM_APP_FONT => {
            let delta = wp as isize as i32;
            let mut size = 0;
            app::update(s.id, |n| {
                n.font_size = (n.font_size as i32 + delta).clamp(g::FONT_MIN as i32, g::FONT_MAX as i32) as u8;
                size = n.font_size;
            });
            body::set_font_size(s.body, size);
            crate::richedit::rescale_bullets(s.body); // ul padding is 1.3 em
            0
        }
        WM_APP_DIRTY => {
            on_body_change(s);
            0
        }
        WM_APP_DECODED => {
            let d = Box::from_raw(lp as *mut crate::images::Decoded);
            crate::richedit::show_picture(s.body, &d);
            0
        }
        WM_APP_MENU => {
            on_menu(s, menu::Act::from_code(wp));
            0
        }
        WM_APP_MENUCLOSED => {
            s.menu = std::ptr::null_mut();
            if !cursor_inside(hwnd) {
                set_hover(s, false);
            }
            invalidate_bar(s);
            0
        }
        WM_APP_CONFIRM => {
            if wp == 1 {
                app::delete_note(s.id);
            } else if !s.body.is_null() {
                SetFocus(s.body);
            }
            0
        }
        m if m == WM_APP + 20 => {
            if !s.title_edit.is_null() && windows_sys::Win32::UI::Input::KeyboardAndMouse::GetFocus() != s.title_edit {
                end_rename(s, false);
            }
            0
        }
        WM_SETFOCUS => {
            if !s.body.is_null() && !app::note(s.id).map(|n| n.folded || n.locked).unwrap_or(true) {
                SetFocus(s.body);
            }
            0
        }
        WM_CLOSE => {
            crate::win::ime_complete(s.body);
            if !s.title_edit.is_null() {
                end_rename(s, false);
            }
            if s.save_pending {
                KillTimer(hwnd, TIMER_SAVE);
                s.save_pending = false;
                serialize(s);
            }
            app::close_note(s.id);
            0
        }
        WM_DESTROY => {
            KillTimer(hwnd, TIMER_REFIT);
            crate::images::forget_note(hwnd);
            if !s.menu.is_null() {
                DestroyWindow(s.menu);
            }
            NOTES.with(|n| n.borrow_mut().retain(|&h| h != hwnd));
            let id = s.id;
            app::with(|a| {
                if a.windows.get(&id) == Some(&hwnd) {
                    a.windows.remove(&id);
                }
            });
            if cfg().quit_when_empty && app::open_windows().is_empty() && !list::is_open() {
                PostQuitMessage(0);
            }
            0
        }
        WM_NCDESTROY => {
            let p = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) as *mut NoteState;
            if !p.is_null() {
                drop(Box::from_raw(p));
            }
            // pictures decoded for this note but not shown yet (closed while decoding): the
            // posted copies would be dropped with the window's queue and never freed
            let mut m: MSG = std::mem::zeroed();
            while PeekMessageW(&mut m, hwnd, WM_APP_DECODED, WM_APP_DECODED, PM_REMOVE) != 0 {
                if m.lParam != 0 {
                    drop(Box::from_raw(m.lParam as *mut crate::images::Decoded));
                    DROPPED_DECODED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

struct ProfGuard(usize, u64);
impl Drop for ProfGuard {
    fn drop(&mut self) {
        crate::metrics::prof_add(self.0, self.1);
    }
}

/// Note id of a note window (for accelerators).
pub fn id_of(hwnd: HWND) -> Option<u64> {
    unsafe { state_of(hwnd).map(|s| s.id) }
}
