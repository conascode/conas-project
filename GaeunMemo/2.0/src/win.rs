//! Small Win32 helpers shared by every module (UI thread only).

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Logical (96-DPI) px → physical px.
pub fn px(v: f32, dpi: u32) -> i32 {
    (v * dpi as f32 / 96.0).round() as i32
}

pub const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    r as u32 | (g as u32) << 8 | (b as u32) << 16
}

pub fn rgb_f(c: u32) -> (f32, f32, f32) {
    ((c & 0xFF) as f32 / 255.0, ((c >> 8) & 0xFF) as f32 / 255.0, ((c >> 16) & 0xFF) as f32 / 255.0)
}

pub fn x_of(l: LPARAM) -> i32 {
    (l & 0xFFFF) as u16 as i16 as i32
}
pub fn y_of(l: LPARAM) -> i32 {
    ((l >> 16) & 0xFFFF) as u16 as i16 as i32
}

pub fn qpc() -> i64 {
    let mut v = 0i64;
    unsafe { QueryPerformanceCounter(&mut v) };
    v
}

pub fn qpc_us(delta: i64) -> f64 {
    static mut FREQ: i64 = 0;
    unsafe {
        if FREQ == 0 {
            QueryPerformanceFrequency(&raw mut FREQ);
        }
        delta as f64 * 1_000_000.0 / FREQ as f64
    }
}

/// v1.1 dark theme tokens (common.css) — the prototype draws only the dark theme.
pub mod theme {
    use super::rgb;
    pub const PAPER: u32 = rgb(0x20, 0x20, 0x20);
    pub const PAPER3: u32 = rgb(0x35, 0x35, 0x35);
    pub const INK: u32 = rgb(0xF0, 0xF0, 0xF0);
    pub const MUTED: u32 = rgb(0xA6, 0xA6, 0xA6);
    pub const EDGE: u32 = rgb(0, 0, 0);
}

/// v1.1 note geometry (model.rs / note.css), logical px.
pub mod geom {
    pub const BAND: f32 = 4.0;
    pub const BAR: f32 = 34.0;
    pub const TOOL: f32 = 24.0;
    pub const TOOL_GAP: f32 = 1.0;
    pub const BAR_PAD_L: f32 = 12.0;
    pub const BAR_PAD_R: f32 = 5.0;
    pub const ICON: f32 = 14.0;
    pub const TITLE_PX: f32 = 13.5;
}

/// Finishes an IME composition in `h` (a Hangul syllable still being composed becomes text),
/// before the note is locked, closed or a rename ends (report §6.4, F-26).
pub fn ime_complete(h: windows_sys::Win32::Foundation::HWND) {
    use windows_sys::Win32::UI::Input::Ime::{ImmGetContext, ImmNotifyIME, ImmReleaseContext, CPS_COMPLETE, NI_COMPOSITIONSTR};
    if h.is_null() {
        return;
    }
    unsafe {
        let c = ImmGetContext(h);
        if !c.is_null() {
            ImmNotifyIME(c, NI_COMPOSITIONSTR, CPS_COMPLETE, 0);
            ImmReleaseContext(h, c);
        }
    }
}

/// A random (v4) UUID in lowercase, like v1's note ids and picture names.
pub fn uuid() -> String {
    let g = unsafe { windows::Win32::System::Com::CoCreateGuid().unwrap_or_default() };
    format!(
        "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        g.data1, g.data2, g.data3, g.data4[0], g.data4[1], g.data4[2], g.data4[3], g.data4[4], g.data4[5], g.data4[6], g.data4[7]
    )
}

/// One process per data folder: a named mutex from the folder path (P4 adds bringing the
/// running one to the front).
pub fn single_instance(dir: &std::path::Path) -> bool {
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::CreateMutexW;
    let key: String = dir.to_string_lossy().to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    let name = wide(&format!(r"Local\GaeunMemo2-{}", &key[key.len().saturating_sub(120)..]));
    unsafe {
        let h = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        // the handle stays open for the life of the process
        !h.is_null() && GetLastError() != ERROR_ALREADY_EXISTS
    }
}

pub fn message_box(text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONINFORMATION, MB_OK};
    let t = wide(text);
    let c = wide("GaeunMemo");
    unsafe { MessageBoxW(std::ptr::null_mut(), t.as_ptr(), c.as_ptr(), MB_OK | MB_ICONINFORMATION) };
}

// ------------------------------------------------------------------ resize edges

/// The resize bands of a note or the list window (client coordinates; the top edge is the
/// caller's): `edge` px along the left and bottom, `edge_right` along the right (narrower
/// while a scroll bar sits there), corners `corner` px long so a diagonal is easy to catch.
pub fn edge_hit(x: i32, y: i32, w: i32, h: i32, edge: i32, edge_right: i32, corner: i32) -> Option<u32> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{HTBOTTOM, HTBOTTOMLEFT, HTBOTTOMRIGHT, HTLEFT, HTRIGHT};
    let left = x < edge;
    let right = x >= w - edge_right;
    let bottom = y >= h - edge;
    let low = y >= h - corner;
    if (bottom && x < corner) || (left && low) {
        Some(HTBOTTOMLEFT)
    } else if (bottom && x >= w - corner) || (right && low) {
        Some(HTBOTTOMRIGHT)
    } else if bottom {
        Some(HTBOTTOM)
    } else if left {
        Some(HTLEFT)
    } else if right {
        Some(HTRIGHT)
    } else {
        None
    }
}

/// For a child window lying over its parent's edges (the note body, the scroll bar, the list
/// rows): true where the parent resizes, so the child answers HTTRANSPARENT and the parent
/// gets the mouse. Without this only the 1–2 px the child leaves free could be grabbed.
pub fn on_parent_edge(child: HWND, lp: LPARAM) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetParent, SendMessageW, WM_NCHITTEST};
    let parent = unsafe { GetParent(child) };
    if parent.is_null() {
        return false;
    }
    let r = unsafe { SendMessageW(parent, WM_NCHITTEST, 0, lp) };
    (10..=17).contains(&r) // HTLEFT … HTBOTTOMRIGHT
}

#[cfg(test)]
mod tests {
    use super::edge_hit;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    #[test]
    fn edges_and_corners() {
        // 300 × 200, edge 9, right edge 4 (scroll bar), corner 18
        let h = |x, y| edge_hit(x, y, 300, 200, 9, 4, 18);
        assert_eq!(h(0, 100), Some(HTLEFT));
        assert_eq!(h(8, 100), Some(HTLEFT));
        assert_eq!(h(9, 100), None);
        assert_eq!(h(299, 100), Some(HTRIGHT));
        assert_eq!(h(296, 100), Some(HTRIGHT));
        assert_eq!(h(295, 100), None); // the scroll bar keeps the rest
        assert_eq!(h(150, 199), Some(HTBOTTOM));
        assert_eq!(h(150, 191), Some(HTBOTTOM));
        assert_eq!(h(150, 190), None);
        assert_eq!(h(10, 195), Some(HTBOTTOMLEFT)); // corner along the bottom
        assert_eq!(h(2, 185), Some(HTBOTTOMLEFT)); // corner up the left side
        assert_eq!(h(285, 195), Some(HTBOTTOMRIGHT));
        assert_eq!(h(298, 185), Some(HTBOTTOMRIGHT));
        assert_eq!(h(150, 100), None);
    }
}
