//! Diagnostic log `gaeunmemo.log` in the data folder (v1.1 F-36 keeps the same idea; the
//! full port with rotation is P4). Any thread may write; nothing is logged before the store
//! folder is known (test and measurement modes log nothing).

use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

static FILE: Mutex<Option<PathBuf>> = Mutex::new(None);

pub fn set_dir(dir: &std::path::Path) {
    if let Ok(mut f) = FILE.lock() {
        *f = Some(dir.join("gaeunmemo.log"));
    }
}

pub fn write(msg: &str) {
    let Ok(guard) = FILE.lock() else { return };
    let Some(path) = guard.as_ref() else { return };
    // keep the file small: start over above 1 MB (v1.1 rotates at the same size)
    if std::fs::metadata(path).map(|m| m.len() > 1_000_000).unwrap_or(false) {
        let _ = std::fs::rename(path, path.with_extension("log.old"));
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{} {msg}", crate::app::now_ms());
    }
}
