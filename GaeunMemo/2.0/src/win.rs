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
