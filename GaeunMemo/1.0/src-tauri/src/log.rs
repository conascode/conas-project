//! Tiny append-only diagnostic log: %APPDATA%\gaeunmemo\gaeunmemo.log
//! Records startup steps, failures and the reason the process ends, so a
//! launch that dies silently (e.g. right after logon) leaves a trace.
//! Never logs note contents. Rotates to gaeunmemo.log.old at 256 KiB.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

const MAX: u64 = 256 * 1024;
static LOCK: Mutex<()> = Mutex::new(());

fn path() -> PathBuf {
    crate::store::data_dir().join("gaeunmemo.log")
}

pub fn line(msg: impl AsRef<str>) {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let p = path();
    if let Some(dir) = p.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if fs::metadata(&p).map(|m| m.len() > MAX).unwrap_or(false) {
        let _ = fs::rename(&p, p.with_extension("log.old"));
    }
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&p) {
        let _ = writeln!(f, "{} [{}] {}", crate::store::stamp(), std::process::id(), msg.as_ref());
    }
}

/// Panics abort the process in release builds; write where and why first.
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        line(format!("PANIC: {info}"));
    }));
}

#[macro_export]
macro_rules! log {
    ($($t:tt)*) => { $crate::log::line(format!($($t)*)) };
}
