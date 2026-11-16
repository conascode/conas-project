//! Diagnostic log `gaeunmemo.log` in the data folder (F-36, v1.1 log.rs): start, load
//! result, window and tray failures, pictures added (size only), session end, why the
//! process ends, panics. Never note contents. Rotates to `gaeunmemo.log.old` at 256 KiB.
//! Any thread may write; nothing is logged before the data folder is known (test and
//! measurement modes log only into their own test folders).

use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

const MAX: u64 = 256 * 1024;
static FILE: Mutex<Option<PathBuf>> = Mutex::new(None);

pub fn set_dir(dir: &std::path::Path) {
    if let Ok(mut f) = FILE.lock() {
        *f = Some(dir.join("gaeunmemo.log"));
    }
}

/// Local time `YYYY-MM-DD HH:MM:SS` (v1.1 log lines look the same).
fn stamp() -> String {
    let mut t: windows_sys::Win32::Foundation::SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut t) };
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

pub fn write(msg: &str) {
    let Ok(guard) = FILE.lock() else { return };
    let Some(path) = guard.as_ref() else { return };
    if std::fs::metadata(path).map(|m| m.len() > MAX).unwrap_or(false) {
        let _ = std::fs::rename(path, path.with_extension("log.old"));
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{} [{}] {msg}", stamp(), std::process::id());
    }
}

/// A panic ends a release build at once: where and why goes to the log first.
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        write(&format!("PANIC: {info}"));
    }));
}
