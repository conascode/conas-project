//! Note body control: RichEdit 5.0 (v2) or plain EDIT (PT-C comparison only).
//! The subclass forwards hover / Ctrl+wheel / locked-click to the note window,
//! draws the "메모" placeholder and blocks RichEdit shortcuts outside the
//! v1.1 allow-list (report §6.3, R16).

use crate::app;
use crate::gfx::{self, ws, FontSpec};
use crate::note::{self, WM_APP_COPY, WM_APP_FONT, WM_APP_HOVER, WM_APP_LEAVE};
use crate::theme::pal;
use crate::win::{px, wide};
use std::cell::Cell;
use std::ffi::c_void;
use windows::core::Interface;
use windows::Win32::Foundation::COLORREF;
use windows::Win32::UI::Controls::RichEdit as re;
use windows::Win32::UI::Controls::RichEdit::{IRichEditOle, ITextDocument2};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, LoadLibraryW};
use windows_sys::Win32::UI::Controls::{EM_EMPTYUNDOBUFFER, EM_SETLIMITTEXT, EM_SETREADONLY, EM_SETSEL};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetKeyState;
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{VK_CONTROL, VK_SHIFT};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BodyKind {
    RichEdit,
    Edit,
    None,
}

impl BodyKind {
    pub fn name(self) -> &'static str {
        match self {
            BodyKind::RichEdit => "richedit",
            BodyKind::Edit => "edit",
            BodyKind::None => "none",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fmt {
    Bold,
    Italic,
    Underline,
    Strike,
    Bullets,
    Image,
}

pub const SAMPLE: &str = "장보기 목록\n우유 2개, 계란 한 판, 식빵\n사과 1 kg — 토요일 시장에서\n\
회의 메모: 다음 주 화요일 오후 3시, 3층 회의실\n\
GaeunMemo 2.0 native rewrite: GDI chrome, RichEdit body, SQLite store.\n\
한글 입력과 English text를 섞어 약 1 KB가 되도록 채운 본문입니다. \
가나다라마바사아자차카타파하 abcdefghijklmnopqrstuvwxyz 0123456789.\n\
끝.";

thread_local! {
    static MSFTEDIT: Cell<bool> = const { Cell::new(false) };
}

/// RichEdit's one-time set-up (the first RICHEDIT50W costs 25–40 ms more than later ones —
/// P6 start-up profile), paid in a hidden throwaway control while the store opens on its own
/// thread. Never shown, no parent.
pub fn warm_richedit() {
    crate::gfx::warm_fonts(unsafe { windows_sys::Win32::UI::HiDpi::GetDpiForSystem() });
    ensure_richedit();
    unsafe {
        let h = CreateWindowExW(0, wide("RICHEDIT50W").as_ptr(), std::ptr::null(), WS_POPUP, 0, 0, 10, 10, std::ptr::null_mut(), std::ptr::null_mut(), GetModuleHandleW(std::ptr::null()), std::ptr::null());
        if !h.is_null() {
            DestroyWindow(h);
        }
    }
}

/// Loads msftedit.dll and initialises OLE the first time a RichEdit body is needed.
pub fn ensure_richedit() {
    MSFTEDIT.with(|m| {
        if !m.get() {
            unsafe {
                let t = std::time::Instant::now();
                let _ = windows::Win32::System::Ole::OleInitialize(None);
                let ole = t.elapsed().as_secs_f64() * 1000.0;
                LoadLibraryW(wide("msftedit.dll").as_ptr());
                crate::note::trace(0, ole);
                crate::note::trace(1, t.elapsed().as_secs_f64() * 1000.0 - ole);
            }
            m.set(true);
        }
    });
}

const PROP_LOCKED: &str = "gm_locked";
const PROP_FONT: &str = "gm_font";
const PROP_KIND: &str = "gm_kind";
/// wheel distance not scrolled yet (a touchpad's small steps), in 1/1024 px
const PROP_WHEEL: &str = "gm_wheel";
/// the v1.1 scroll bar window (crate::sbar) that sits over the body's right edge
const PROP_BAR: &str = "gm_bar";
/// width of the body that shows (the RichEdit window itself is wider, see `place`)
const PROP_VISW: &str = "gm_visw";
/// right edge of the window region last set (to skip setting the same one again)
const PROP_CLIP: &str = "gm_clip";

/// Clips the body so that nothing of its system scroll bar shows: while the bar is there the
/// region ends where it starts (the v1.1 bar window covers the rest of the edge); without it
/// the region is the visible width.
fn clip(h: HWND) {
    unsafe {
        let w = prop(h, PROP_VISW) as i32;
        if w <= 0 {
            return;
        }
        let mut cr = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        let mut wr = cr;
        GetClientRect(h, &mut cr);
        GetWindowRect(h, &mut wr);
        let right = if native_bar_shown(h) { cr.right.min(w) } else { w }.max(1);
        let hh = (wr.bottom - wr.top).max(1);
        let key = (right as isize) << 20 | hh as isize;
        if prop(h, PROP_CLIP) != key {
            set_prop(h, PROP_CLIP, key);
            SetWindowRgn(h, CreateRectRgn(0, 0, right, hh), 1);
        }
    }
}

/// True while the v1.1 scroll bar shows over the body's right edge.
pub fn bar_visible(h: HWND) -> bool {
    let b = bar_of(h);
    !h.is_null() && !b.is_null() && unsafe { IsWindowVisible(b) } != 0
}

pub fn bar_for_test(h: HWND) -> HWND {
    bar_of(h)
}

fn bar_of(h: HWND) -> HWND {
    prop(h, PROP_BAR) as HWND
}

fn native_bar_w(h: HWND) -> i32 {
    unsafe { windows_sys::Win32::UI::HiDpi::GetSystemMetricsForDpi(SM_CXVSCROLL, GetDpiForWindow(h).max(96)) }
}

/// Puts the body in the note: the RichEdit is made wider by the system scroll bar's width
/// minus the 8 px of v1.1's bar and clipped to the visible part, so its own (system) scroll
/// bar lies outside the clip — it only supplies the range — and `sbar` draws the v1.1 bar
/// over the right 8 px. With no bar the text area is the whole width (set_padding).
pub fn place(h: HWND, x: i32, y: i32, w: i32, hh: i32) {
    if h.is_null() {
        return;
    }
    unsafe {
        if !is_rich(h) {
            SetWindowPos(h, std::ptr::null_mut(), x, y, w, hh, SWP_NOZORDER | SWP_NOACTIVATE);
            return;
        }
        let dpi = GetDpiForWindow(h).max(96);
        let sb = px(crate::sbar::WIDTH, dpi);
        let ww = (w - sb + native_bar_w(h)).max(1);
        SetWindowPos(h, std::ptr::null_mut(), x, y, ww, hh, SWP_NOZORDER | SWP_NOACTIVATE);
        set_prop(h, PROP_VISW, w as isize);
        clip(h);
        let bar = bar_of(h);
        if !bar.is_null() {
            // v1.1's bar runs to the window's right edge with its last pixel under the 1 px
            // border: it starts `sb` from that edge and shows `sb - 1` (the body itself is
            // inset by `x` on both sides, so the edge is at x + w + x)
            let edge = x + w + x;
            SetWindowPos(bar, std::ptr::null_mut(), edge - sb, y, sb - 1, hh, SWP_NOZORDER | SWP_NOACTIVATE);
        }
        set_padding(h);
        sync_bar(h);
    }
}

/// Shows or hides the body together with its scroll bar.
pub fn set_visible(h: HWND, on: bool) {
    if h.is_null() {
        return;
    }
    unsafe {
        ShowWindow(h, if on { SW_SHOWNA } else { SW_HIDE });
        if on {
            sync_bar(h);
        } else {
            crate::sbar::hide(bar_of(h));
        }
    }
}

/// True while the system scroll bar (outside the clip) is there: the client is narrower
/// than the window by its width.
fn native_bar_shown(h: HWND) -> bool {
    unsafe {
        let mut wr = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        let mut cr = wr;
        GetWindowRect(h, &mut wr);
        GetClientRect(h, &mut cr);
        wr.right - wr.left - cr.right > 0
    }
}

/// The v1.1 bar follows the body's scroll range (called after painting and layout).
pub fn sync_bar(h: HWND) {
    let bar = bar_of(h);
    if bar.is_null() || unsafe { IsWindowVisible(h) } == 0 {
        return;
    }
    unsafe {
        let mut si: SCROLLINFO = std::mem::zeroed();
        si.cbSize = std::mem::size_of::<SCROLLINFO>() as u32;
        si.fMask = SIF_RANGE | SIF_PAGE | SIF_POS;
        if native_bar_shown(h) && GetScrollInfo(h, SB_VERT as i32, &mut si) != 0 {
            crate::sbar::set(bar, si.nMax - si.nMin + 1, si.nPage as i32, si.nPos);
        } else {
            crate::sbar::set(bar, 0, 0, 0);
        }
    }
}

/// The whole body into `dc`: background, RichEdit's own drawing (WM_PRINTCLIENT), the
/// "메모" placeholder. The background is filled first because RichEdit leaves the padding
/// under a picture that runs into it undrawn.
pub fn paint_to(h: HWND, dc: HDC, locked: bool) {
    unsafe {
        let mut rc = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        GetClientRect(h, &mut rc);
        let br = CreateSolidBrush(pal(app::theme()).paper);
        FillRect(dc, &rc, br);
        DeleteObject(br);
        DefSubclassProc(h, WM_PRINTCLIENT, dc as usize, PRF_CLIENT as isize);
        placeholder(h, dc, locked);
    }
}

/// No text and no picture. One line of length 0 — O(1), unlike `GetWindowTextLength`, which
/// RichEdit answers by walking the whole text (it ran on every paint: P6 typing profile).
fn is_empty(h: HWND) -> bool {
    unsafe { SendMessageW(h, windows_sys::Win32::UI::Controls::EM_GETLINECOUNT, 0, 0) <= 1 && SendMessageW(h, windows_sys::Win32::UI::Controls::EM_LINELENGTH, 0, 0) == 0 }
}

/// "메모" placeholder of an empty body (.body.is-empty::before).
fn placeholder(h: HWND, dc: HDC, locked: bool) {
    unsafe {
        if locked || !is_empty(h) {
            return;
        }
        let dpi = GetDpiForWindow(h);
        let mut fr = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        SendMessageW(h, windows_sys::Win32::UI::Controls::EM_GETRECT, 0, &mut fr as *mut _ as isize);
        let size = prop(h, PROP_FONT).max(10) as u32;
        let f = gfx::font(FontSpec::px(size * 10), dpi);
        let lh = (size as f32 * 1.6 * dpi as f32 / 96.0).round() as i32;
        let fh = gfx::line_h(dc, f);
        fr.top += (lh - fh) / 2;
        gfx::text(dc, &ws("메모"), &fr, f, pal(app::theme()).muted, DT_SINGLELINE | DT_TOP | DT_LEFT);
    }
}

/// WM_PAINT. A small repaint (a typed character: a line or two) is RichEdit's own — it
/// draws only that part. A large one (resize, a picture replaced, a scroll) is drawn
/// off-screen and copied in one BitBlt, because RichEdit draws a picture line by clearing
/// it and then drawing the picture, which flashed on every resize step. RichEdit's
/// off-screen drawing (WM_PRINTCLIENT) lays out every visible line, so it is kept to the
/// repaints that redraw most of the body anyway.
/// How the body was painted (tests: a resize step must be one off-screen frame, no erase).
pub static ERASES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
pub static NATIVE_PAINTS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
pub static BUFFERED_PAINTS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
pub static FROZEN_SKIPS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn paint(h: HWND, locked: bool) {
    if crate::richedit::is_frozen() {
        FROZEN_SKIPS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        return; // left invalid: painted when the freeze ends (no bare-paper frame)
    }
    unsafe {
        let mut ur = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        GetUpdateRect(h, &mut ur, 0);
        let dpi = GetDpiForWindow(h).max(96) as i32;
        let line = (prop(h, PROP_FONT).max(10) as i32 * 16 * dpi) / (10 * 96);
        let c0 = crate::metrics::prof_start();
        // three lines plus the paragraph spacing a wrapped edit adds (typing measured 102 px
        // = 3 × 33 + 3): such a repaint went the off-screen way, which lays out every line
        if ur.bottom - ur.top <= 3 * line + line / 2 {
            NATIVE_PAINTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            DefSubclassProc(h, WM_PAINT, 0, 0);
            crate::metrics::prof_add(0, c0);
            let c1 = crate::metrics::prof_start();
            if !locked && is_empty(h) {
                let dc = GetDC(h);
                placeholder(h, dc, locked);
                ReleaseDC(h, dc);
            }
            crate::metrics::prof_add(1, c1);
        } else {
            BUFFERED_PAINTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            paint_buffered(h, locked);
            crate::metrics::prof_add(2, c0);
        }
        let c2 = crate::metrics::prof_start();
        sync_bar(h);
        crate::metrics::prof_add(3, c2);
    }
}

/// WM_PAINT drawn off-screen and copied in one BitBlt: RichEdit draws a picture line by
/// clearing it and then drawing the picture, which showed as a flash on every resize step.
/// (BeginPaint hides the caret meanwhile.)
fn paint_buffered(h: HWND, locked: bool) {
    unsafe {
        let mut ps: PAINTSTRUCT = std::mem::zeroed();
        let dc = BeginPaint(h, &mut ps);
        let mut rc = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        GetClientRect(h, &mut rc);
        let (w, hh) = (rc.right.max(1), rc.bottom.max(1));
        let mem = CreateCompatibleDC(dc);
        let bmp = CreateCompatibleBitmap(dc, w, hh);
        if mem.is_null() || bmp.is_null() {
            // out of GDI resources: draw directly rather than not at all
            if !bmp.is_null() {
                DeleteObject(bmp);
            }
            if !mem.is_null() {
                DeleteDC(mem);
            }
            paint_to(h, dc, locked);
        } else {
            let old = SelectObject(mem, bmp);
            let r = ps.rcPaint;
            paint_to(h, mem, locked);
            BitBlt(dc, r.left, r.top, r.right - r.left, r.bottom - r.top, mem, r.left, r.top, SRCCOPY);
            SelectObject(mem, old);
            DeleteObject(bmp);
            DeleteDC(mem);
        }
        EndPaint(h, &ps);
    }
}

/// Touchpad momentum filter. After the fingers lift, the touchpad driver keeps sending
/// wheel values that shrink step by step (momentum); the body should only move by what the
/// fingers did. A mouse wheel sends whole notches (multiples of 120) and is never touched.
/// Touchpad values (not multiples of 120) that come close together and fall at least 5 %
/// each, four times in a row, are momentum: they and the rest of that run are dropped
/// until a pause (> 120 ms), a sign change, or a value that grows again (new gesture).
/// A heuristic — the OS does not mark momentum values — so a finger that slows down very
/// smoothly loses its last few small steps.
pub struct Momentum {
    last: Option<(i32, std::time::Instant)>,
    falling: u32,
    dropping: bool,
}

thread_local! {
    static MOMENTUM: std::cell::RefCell<Momentum> = const { std::cell::RefCell::new(Momentum { last: None, falling: 0, dropping: false }) };
}

pub fn is_inertia(delta: i32, now: std::time::Instant) -> bool {
    MOMENTUM.with(|m| m.borrow_mut().feed(delta, now))
}

impl Momentum {
    pub fn feed(&mut self, delta: i32, now: std::time::Instant) -> bool {
        if delta % 120 == 0 {
            *self = Momentum { last: None, falling: 0, dropping: false };
            return false; // a mouse wheel notch
        }
        let mag = delta.abs();
        let continues = self.last.is_some_and(|(p, t)| {
            now.duration_since(t) <= std::time::Duration::from_millis(120) && p.signum() == delta.signum()
        });
        let prev = self.last.map_or(0, |(p, _)| p.abs());
        self.last = Some((delta, now));
        if !continues {
            self.falling = 0;
            self.dropping = false;
            return false;
        }
        if self.dropping {
            if mag * 2 > prev * 3 {
                self.dropping = false; // grows again: the fingers are back
                self.falling = 0;
                return false;
            }
            return true;
        }
        if mag * 100 <= prev * 95 {
            self.falling += 1;
        } else {
            self.falling = 0;
        }
        if self.falling >= 4 {
            self.dropping = true;
            return true;
        }
        false
    }
}

/// Mouse wheel: the body moves exactly the distance turned, at once — v1.1 (Chromium)
/// distance, 100 CSS px per three lines of the system wheel setting (one notch at the default
/// 3 lines), or a page. RichEdit's own wheel handling scrolls whole lines (a picture is one
/// tall line, so one notch skipped all of it) and animates on after the wheel stops.
pub fn wheel(h: HWND, delta: i32) {
    if is_inertia(delta, std::time::Instant::now()) {
        return;
    }
    unsafe {
        let dpi = GetDpiForWindow(h).max(96) as f64;
        let mut lines: u32 = 3;
        SystemParametersInfoW(SPI_GETWHEELSCROLLLINES, 0, &mut lines as *mut u32 as *mut c_void, 0);
        let mut rc = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        GetClientRect(h, &mut rc);
        let page = (rc.bottom as f64 - 16.0 * dpi / 96.0).max(1.0); // less the 4 + 12 px padding
        let step = if lines == u32::MAX { page } else { lines as f64 * 100.0 / 3.0 * dpi / 96.0 };
        let mut carry = prop(h, PROP_WHEEL) as f64 / 1024.0;
        if carry != 0.0 && carry.signum() != (delta as f64).signum() {
            carry = 0.0; // turned the other way
        }
        let want = delta as f64 / 120.0 * step + carry;
        let dist = want.trunc();
        let mut p = POINT { x: 0, y: 0 };
        SendMessageW(h, re::EM_GETSCROLLPOS, 0, &mut p as *mut _ as isize);
        let mut si: SCROLLINFO = std::mem::zeroed();
        si.cbSize = std::mem::size_of::<SCROLLINFO>() as u32;
        si.fMask = SIF_RANGE | SIF_PAGE;
        let max = if GetScrollInfo(h, SB_VERT as i32, &mut si) != 0 { (si.nMax - si.nPage as i32 + 1).max(si.nMin) } else { 0 };
        let y = (p.y - dist as i32).clamp(0, max.max(0));
        // at either end the rest is dropped, as in a browser
        let left = if y == 0 || y == max { 0.0 } else { want - dist };
        set_prop(h, PROP_WHEEL, (left * 1024.0) as isize);
        if y != p.y {
            let np = POINT { x: p.x, y };
            SendMessageW(h, re::EM_SETSCROLLPOS, 0, &np as *const _ as isize);
            sync_bar(h);
        }
    }
}

fn prop(h: HWND, k: &str) -> isize {
    unsafe { GetPropW(h, wide(k).as_ptr()) as isize }
}
fn set_prop(h: HWND, k: &str, v: isize) {
    unsafe { SetPropW(h, wide(k).as_ptr(), v as *mut c_void) };
}

fn is_rich(h: HWND) -> bool {
    prop(h, PROP_KIND) == 1
}

pub fn is_locked(h: HWND) -> bool {
    prop(h, PROP_LOCKED) != 0
}

/// Body text size in px (10–32).
pub fn font_size(h: HWND) -> u8 {
    (prop(h, PROP_FONT) as u8).clamp(10, 32)
}

/// Formatting changed without a text change (no EN_CHANGE for that): the note saves it.
pub fn changed(h: HWND) {
    unsafe { PostMessageW(GetParent(h), note::WM_APP_DIRTY, 0, 0) };
}

pub fn create(parent: HWND, kind: BodyKind, dpi: u32, text: &str, font_size: u8) -> HWND {
    unsafe {
        let hinst = GetModuleHandleW(std::ptr::null());
        let style = WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS | WS_VSCROLL | ES_MULTILINE as u32 | ES_AUTOVSCROLL as u32 | ES_WANTRETURN as u32 | ES_NOHIDESEL as u32;
        let h = match kind {
            BodyKind::None => return std::ptr::null_mut(),
            BodyKind::RichEdit => {
                ensure_richedit();
                let tw = std::time::Instant::now();
                let h = CreateWindowExW(0, wide("RICHEDIT50W").as_ptr(), std::ptr::null(), style | re::ES_SAVESEL, 0, 0, 10, 10, parent, std::ptr::null_mut(), hinst, std::ptr::null());
                if h.is_null() {
                    return h;
                }
                crate::note::trace6(0, tw.elapsed().as_secs_f64() * 1000.0);
                let ta = std::time::Instant::now();
                set_prop(h, PROP_KIND, 1);
                set_prop(h, PROP_BAR, crate::sbar::create(parent, crate::sbar::Target::Rich(h)) as isize);
                SendMessageW(h, re::EM_SETTEXTMODE, (re::TM_RICHTEXT.0 | re::TM_MULTILEVELUNDO.0) as usize, 0);
                SendMessageW(h, re::EM_EXLIMITTEXT, 0, 2_000_000);
                // paste / drop / copy follow v1.1's rules (crate::richedit)
                crate::richedit::Callback::attach(h);
                set_text(h, &text.replace('\n', "\r"));
                crate::note::trace6(1, ta.elapsed().as_secs_f64() * 1000.0);
                h
            }
            BodyKind::Edit => {
                let h = CreateWindowExW(0, wide("EDIT").as_ptr(), std::ptr::null(), style, 0, 0, 10, 10, parent, std::ptr::null_mut(), hinst, std::ptr::null());
                SendMessageW(h, EM_SETLIMITTEXT, 2_000_000, 0);
                set_text(h, &text.replace('\n', "\r\n"));
                h
            }
        };
        set_prop(h, PROP_FONT, font_size as isize);
        SetWindowSubclass(h, Some(body_proc), 7, 0);
        let tt = std::time::Instant::now();
        apply_theme(h);
        crate::note::trace6(2, tt.elapsed().as_secs_f64() * 1000.0);
        let tf = std::time::Instant::now();
        set_font_size(h, font_size);
        crate::note::trace6(3, tf.elapsed().as_secs_f64() * 1000.0);
        if kind == BodyKind::RichEdit {
            SendMessageW(h, EM_SETSEL, 0, 0);
            SendMessageW(h, EM_EMPTYUNDOBUFFER, 0, 0);
            SendMessageW(h, re::EM_SETEVENTMASK, 0, (re::ENM_CHANGE | re::ENM_SELCHANGE) as isize);
        }
        let _ = dpi;
        h
    }
}

fn set_text(h: HWND, s: &str) {
    let w = wide(s);
    unsafe { SendMessageW(h, WM_SETTEXT, 0, w.as_ptr() as isize) };
}

unsafe fn charformat() -> re::CHARFORMAT2W {
    let mut cf: re::CHARFORMAT2W = std::mem::zeroed();
    cf.Base.cbSize = std::mem::size_of::<re::CHARFORMAT2W>() as u32;
    cf
}

unsafe fn paraformat() -> re::PARAFORMAT2 {
    let mut pf: re::PARAFORMAT2 = std::mem::zeroed();
    pf.Base.cbSize = std::mem::size_of::<re::PARAFORMAT2>() as u32;
    pf
}

/// Theme colours: background + text colour of all text and of new text.
pub fn apply_theme(h: HWND) {
    if h.is_null() {
        return;
    }
    let p = pal(app::theme());
    unsafe {
        if is_rich(h) {
            SendMessageW(h, re::EM_SETBKGNDCOLOR, 0, p.paper as isize);
            let mut cf = charformat();
            cf.Base.dwMask = re::CFM_COLOR;
            cf.Base.crTextColor = COLORREF(p.ink);
            SendMessageW(h, re::EM_SETCHARFORMAT, re::SCF_ALL as usize, &cf as *const _ as isize);
            SendMessageW(h, re::EM_SETCHARFORMAT, 0 /* SCF_DEFAULT */, &cf as *const _ as isize);
            SetWindowTheme_dark(h);
        }
        InvalidateRect(h, std::ptr::null(), 1);
        if !bar_of(h).is_null() {
            InvalidateRect(bar_of(h), std::ptr::null(), 0);
        }
    }
}

#[allow(non_snake_case)]
fn SetWindowTheme_dark(h: HWND) {
    // dark scrollbars (undocumented theme name; harmless if missing — report §6.5)
    let name = if app::theme() == crate::theme::Theme::Dark { "DarkMode_Explorer" } else { "Explorer" };
    unsafe {
        let _ = windows::Win32::UI::Controls::SetWindowTheme(windows::Win32::Foundation::HWND(h), &windows::core::HSTRING::from(name), windows::core::PCWSTR::null());
    }
}

/// Body text size (v1.1 fontSize, 10–32 px) and line height 1.6.
pub fn set_font_size(h: HWND, size: u8) {
    if h.is_null() {
        return;
    }
    set_prop(h, PROP_FONT, size as isize);
    unsafe {
        if is_rich(h) {
            let mut cf = charformat();
            cf.Base.dwMask = re::CFM_FACE | re::CFM_SIZE;
            cf.Base.yHeight = size as i32 * 15; // px → twips at 96 dpi (RichEdit scales by DPI)
            for (i, c) in "Malgun Gothic".encode_utf16().enumerate() {
                cf.Base.szFaceName[i] = c;
            }
            SendMessageW(h, re::EM_SETCHARFORMAT, re::SCF_ALL as usize, &cf as *const _ as isize);
            SendMessageW(h, re::EM_SETCHARFORMAT, 0, &cf as *const _ as isize);
            // line-height: 1.6 → exact spacing in twips on every paragraph, "at least" on
            // paragraphs holding a picture (richedit::fix_spacing)
            crate::richedit::fix_spacing(h, 0, i32::MAX);
        } else {
            let dpi = GetDpiForWindow(h);
            SendMessageW(h, WM_SETFONT, gfx::font(FontSpec::px(size as u32 * 10), dpi) as usize, 1);
        }
    }
    set_padding(h);
}

pub fn set_dpi(h: HWND, dpi: u32) {
    if h.is_null() {
        return;
    }
    if !is_rich(h) {
        unsafe { SendMessageW(h, WM_SETFONT, gfx::font(FontSpec::px(prop(h, PROP_FONT) as u32 * 10), dpi) as usize, 1) };
    }
    set_padding(h);
}

/// v1.1 `.body { padding: 4px 12px 12px }`, inside the control so text scrolls under it.
pub fn set_padding(h: HWND) {
    if h.is_null() || !is_rich(h) {
        return;
    }
    unsafe {
        let dpi = GetDpiForWindow(h);
        let mut rc = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        GetClientRect(h, &mut rc);
        // without the (hidden) system bar the client is wider than what shows: keep the 12 px
        // right padding at the visible edge
        let hidden = if native_bar_shown(h) { 0 } else { (native_bar_w(h) - px(crate::sbar::WIDTH, dpi)).max(0) };
        let r = RECT { left: px(12.0, dpi) - px(1.0, dpi).max(1), top: px(4.0, dpi), right: rc.right - px(12.0, dpi) - hidden, bottom: rc.bottom - px(12.0, dpi) };
        SendMessageW(h, windows_sys::Win32::UI::Controls::EM_SETRECT, 0, &r as *const _ as isize);
        // the system bar may have come or gone: keep it out of sight
        clip(h);
    }
}

pub fn set_locked(h: HWND, on: bool) {
    if h.is_null() {
        return;
    }
    set_prop(h, PROP_LOCKED, on as isize);
    unsafe {
        SendMessageW(h, EM_SETREADONLY, on as usize, 0);
        if on {
            SendMessageW(h, EM_SETSEL, 0, 0);
        }
        if is_rich(h) {
            // readonly RichEdit would grey its background; keep the paper colour
            SendMessageW(h, re::EM_SETBKGNDCOLOR, 0, pal(app::theme()).paper as isize);
        }
    }
}

pub fn toggle(h: HWND, f: Fmt) {
    if h.is_null() || !is_rich(h) || prop(h, PROP_LOCKED) != 0 {
        return;
    }
    unsafe {
        if f == Fmt::Bullets {
            let mut pf = paraformat();
            SendMessageW(h, re::EM_GETPARAFORMAT, 0, &mut pf as *mut _ as isize);
            let on = pf.Base.wNumbering == re::PFN_BULLET;
            let mut set = paraformat();
            set.Base.dwMask = re::PFM_NUMBERING | re::PFM_OFFSET;
            set.Base.wNumbering = if on { re::PARAFORMAT_NUMBERING(0) } else { re::PFN_BULLET };
            set.Base.dxOffset = if on { 0 } else { (prop(h, PROP_FONT) as f32 * 1.3 * 15.0) as i32 };
            SendMessageW(h, re::EM_SETPARAFORMAT, 0, &set as *const _ as isize);
            return;
        }
        let bit = match f {
            Fmt::Bold => 1u32,
            Fmt::Italic => 2,
            Fmt::Underline => 4,
            _ => 8,
        };
        let mut cf = charformat();
        SendMessageW(h, re::EM_GETCHARFORMAT, re::SCF_SELECTION as usize, &mut cf as *mut _ as isize);
        let on = cf.Base.dwMask.0 & bit != 0 && cf.Base.dwEffects.0 & bit != 0;
        let mut set = charformat();
        set.Base.dwMask = re::CFM_MASK(bit);
        set.Base.dwEffects = re::CFE_EFFECTS(if on { 0 } else { bit });
        SendMessageW(h, re::EM_SETCHARFORMAT, re::SCF_SELECTION as usize, &set as *const _ as isize);
    }
}

/// Pressed state of B / I / U / S / bullets for the selection (formatting row).
pub fn format_state(h: HWND) -> [bool; 5] {
    if h.is_null() || !is_rich(h) {
        return [false; 5];
    }
    unsafe {
        let mut cf = charformat();
        SendMessageW(h, re::EM_GETCHARFORMAT, re::SCF_SELECTION as usize, &mut cf as *mut _ as isize);
        let on = |b: u32| cf.Base.dwMask.0 & b != 0 && cf.Base.dwEffects.0 & b != 0;
        let mut pf = paraformat();
        SendMessageW(h, re::EM_GETPARAFORMAT, 0, &mut pf as *mut _ as isize);
        [on(1), on(2), on(4), on(8), pf.Base.wNumbering == re::PFN_BULLET]
    }
}

/// The plain-text copy the app hands to the saver (v1.1 `text`).
pub fn plain(h: HWND) -> Vec<u16> {
    unsafe {
        let n = GetWindowTextLengthW(h);
        let mut buf = vec![0u16; n as usize + 1];
        let got = GetWindowTextW(h, buf.as_mut_ptr(), buf.len() as i32);
        buf.truncate(got as usize);
        buf
    }
}

/// TOM document of a RichEdit body.
pub fn tom(h: HWND) -> Option<(IRichEditOle, ITextDocument2)> {
    unsafe {
        let mut p: *mut c_void = std::ptr::null_mut();
        SendMessageW(h, re::EM_GETOLEINTERFACE, 0, &mut p as *mut _ as isize);
        if p.is_null() {
            return None;
        }
        let ole = IRichEditOle::from_raw(p);
        let doc = ole.cast::<ITextDocument2>().ok()?;
        Some((ole, doc))
    }
}

fn ctrl() -> bool {
    unsafe { GetKeyState(VK_CONTROL as i32) < 0 }
}
fn shift() -> bool {
    unsafe { GetKeyState(VK_SHIFT as i32) < 0 }
}

/// RichEdit's own formatting shortcuts that v1.1 does not have (R16).
fn blocked_shortcut(vk: usize) -> bool {
    if !ctrl() {
        return false;
    }
    let k = vk as u8;
    match k {
        b'L' | b'E' | b'R' | b'J' | b'1' | b'2' | b'5' => !shift() || k == b'L',
        b'A' => shift(),
        0xBB /* VK_OEM_PLUS: Ctrl+= / Ctrl+Shift+= sub/superscript */ => true,
        _ => false,
    }
}

unsafe extern "system" fn body_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, _d: usize) -> LRESULT {
    let parent = GetParent(h);
    let locked = prop(h, PROP_LOCKED) != 0;
    match msg {
        WM_MOUSEMOVE => {
            SendMessageW(parent, WM_APP_HOVER, 0, 0);
            note::track(h, false);
            if locked {
                return 0;
            }
        }
        0x02A3 /* WM_MOUSELEAVE */ => {
            PostMessageW(parent, WM_APP_LEAVE, 0, 0);
        }
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK if locked => return 0, // no selection when locked
        WM_LBUTTONDOWN => {
            // a drag started here runs inside this call; its data object is done after it
            let r = DefSubclassProc(h, msg, wp, lp);
            crate::richedit::drag_done();
            return r;
        }
        WM_LBUTTONUP if locked => {
            PostMessageW(parent, WM_APP_COPY, 0, 0);
            return 0;
        }
        WM_SETCURSOR if locked => {
            SetCursor(LoadCursorW(std::ptr::null_mut(), IDC_HAND));
            return 1;
        }
        WM_MOUSEWHEEL if (wp & 0x0008) != 0 /* MK_CONTROL */ => {
            let delta = ((wp >> 16) & 0xFFFF) as u16 as i16;
            SendMessageW(parent, WM_APP_FONT, if delta > 0 { 1usize } else { (-1isize) as usize }, 0);
            return 0; // RichEdit's own zoom stays off
        }
        // the note's resize bands run over the body: let the note have the mouse there
        WM_NCHITTEST if crate::win::on_parent_edge(h, lp) => return HTTRANSPARENT as LRESULT,
        WM_ERASEBKGND if is_rich(h) => {
            ERASES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        WM_MOUSEWHEEL if is_rich(h) => {
            wheel(h, ((wp >> 16) & 0xFFFF) as u16 as i16 as i32);
            return 0;
        }
        WM_KEYDOWN if blocked_shortcut(wp) => return 0,
        WM_KEYDOWN if wp == b'V' as usize && ctrl() && shift() => {
            // Ctrl+Shift+V: paste as plain text (v1.1)
            if !locked {
                crate::richedit::paste_plain(h);
            }
            return 0;
        }
        WM_KEYDOWN if ctrl() && matches!(wp as u8, b'B' | b'I' | b'U') => {
            if locked {
                return 0; // v1.1 blocks Ctrl+B / I / U on a locked note
            }
            let r = DefSubclassProc(h, msg, wp, lp);
            changed(h);
            return r;
        }
        WM_SIZE => {
            let r = DefSubclassProc(h, msg, wp, lp);
            set_padding(h);
            if is_rich(h) {
                // also when only the scroll bar came or went (the text width changed)
                crate::richedit::live_refit(h);
                sync_bar(h);
            }
            return r;
        }
        WM_PAINT if is_rich(h) => {
            paint(h, locked);
            return 0;
        }
        WM_PRINTCLIENT if is_rich(h) => {
            paint_to(h, wp as HDC, locked);
            return 0;
        }
        WM_NCDESTROY => {
            RemoveWindowSubclass(h, Some(body_proc), 7);
            for k in [PROP_LOCKED, PROP_FONT, PROP_KIND, PROP_WHEEL, PROP_BAR, PROP_VISW, PROP_CLIP] {
                RemovePropW(h, wide(k).as_ptr());
            }
        }
        _ => {}
    }
    DefSubclassProc(h, msg, wp, lp)
}

#[cfg(test)]
mod tests {
    use super::Momentum;
    use std::time::{Duration, Instant};

    fn run(deltas: &[i32], gap_ms: u64) -> Vec<bool> {
        let mut m = Momentum { last: None, falling: 0, dropping: false };
        let t0 = Instant::now();
        deltas.iter().enumerate().map(|(k, &d)| m.feed(d, t0 + Duration::from_millis(k as u64 * gap_ms))).collect()
    }

    #[test]
    fn mouse_notches_are_never_momentum() {
        assert!(run(&[-120, -120, -240, -120, -120, -120, -120], 10).iter().all(|d| !d));
    }

    #[test]
    fn fingers_speeding_up_and_slowing_slightly_keep_moving() {
        // rises, noisy plateau: no four falling steps in a row
        let v = [-10, -18, -30, -44, -52, -50, -53, -49, -52, -47, -51];
        assert!(run(&v, 8).iter().all(|d| !d));
    }

    #[test]
    fn decaying_tail_is_dropped_after_four_falling_steps() {
        let v = [-40, -80, -120 + 1, -110, -100, -90, -80, -72, -64, -56, -48, -40, -30, -20, -10];
        let d = run(&v, 8);
        let kept: i32 = v.iter().zip(&d).filter(|(_, &x)| !x).map(|(&a, _)| a).sum();
        assert!(d[..6].iter().all(|x| !x), "{d:?}"); // the fingers' part and the first fall
        assert!(d[7..].iter().all(|x| *x), "{d:?}");
        assert!(kept > -700, "{kept}");
    }

    #[test]
    fn a_new_gesture_after_a_pause_moves_again() {
        let mut m = Momentum { last: None, falling: 0, dropping: false };
        let t0 = Instant::now();
        for (k, d) in [-100, -90, -80, -70, -60, -50, -40].iter().enumerate() {
            m.feed(*d, t0 + Duration::from_millis(k as u64 * 8));
        }
        assert!(m.dropping);
        assert!(!m.feed(-30, t0 + Duration::from_millis(500)));
        assert!(!m.feed(-45, t0 + Duration::from_millis(508)));
    }

    #[test]
    fn growing_again_ends_the_drop() {
        let mut m = Momentum { last: None, falling: 0, dropping: false };
        let t0 = Instant::now();
        for (k, d) in [-100, -90, -80, -70, -60, -50].iter().enumerate() {
            m.feed(*d, t0 + Duration::from_millis(k as u64 * 8));
        }
        assert!(m.dropping);
        assert!(!m.feed(-90, t0 + Duration::from_millis(60)));
    }
}
