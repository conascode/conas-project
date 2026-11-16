//! `--mode p4check --data DIR --out FILE`: P4 checks in new test folders. Never touches the
//! real Run key (a test key is used and removed), never adds a tray icon (the session window
//! is made without one), windows stay off-screen and unfocused.
//!
//! * F-18 a second launch on the same data folder: the first process holds the folder, the
//!   second finds it and asks it to show its notes (a child process plays the first)
//! * session end through the hidden session window: what was just typed is in the database
//! * F-19 autostart value: command line, write / read / remove (test key)
//! * the app icon resource and the note window class icon (F-17)
//! * F-36 log: rotation at 256 KiB, local time and process id on each line

use crate::app;
use crate::autostart;
use crate::metrics::{self, Json};
use crate::note;
use crate::shell;
use crate::store;
use crate::win;
use std::path::Path;
use std::time::{Duration, Instant};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

struct T {
    out: Vec<String>,
    pass: u32,
    fail: u32,
}

impl T {
    fn check(&mut self, name: &str, ok: bool, detail: String) {
        if ok {
            self.pass += 1;
        } else {
            self.fail += 1;
        }
        self.out.push(Json::default().s("name", name).b("pass", ok).s("detail", &detail).finish());
    }
}

fn pump_for(ms: u64) {
    let end = Instant::now() + Duration::from_millis(ms);
    loop {
        unsafe {
            let mut m: MSG = std::mem::zeroed();
            while PeekMessageW(&mut m, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&m);
                DispatchMessageW(&m);
            }
        }
        if Instant::now() >= end {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn wait_file(p: &Path, ms: u64) -> bool {
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_millis(ms) {
        if p.exists() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    p.exists()
}

const TEST_KEY: &str = r"Software\GaeunMemo2-test\Run";

pub fn run(data: &str, out: &str) {
    let base = Path::new(data).to_path_buf();
    let mut t = T { out: Vec::new(), pass: 0, fail: 0 };
    if data.is_empty() || base.exists() {
        t.check("test folder is new (--data)", false, data.to_string());
        metrics::append(out, &Json::default().s("mode", "p4check").raw("checks", format!("[{}]", t.out.join(","))).finish());
        return;
    }
    let first = base.join("first");
    let mine = base.join("mine");
    let _ = std::fs::create_dir_all(&first);
    let _ = std::fs::create_dir_all(&mine);

    // ---- F-18: a second launch shows the running app's notes
    let exe = std::env::current_exe().unwrap_or_default();
    let child = std::process::Command::new(&exe).args(["--mode", "p4child", "--data"]).arg(&first).spawn();
    let ready = child.is_ok() && wait_file(&first.join("ready.txt"), 15_000);
    t.check("a running app holds its data folder", ready && !win::single_instance(&first), String::new());
    let asked = shell::signal_running(&first);
    let shown = wait_file(&first.join("shown.txt"), 5_000);
    t.check("a second launch makes the running app show its notes (F-18)", asked && shown, format!("asked {asked}, shown {shown}"));
    t.check("another data folder is another app", win::single_instance(&mine) && !shell::signal_running(&mine), String::new());
    if let Ok(mut c) = child {
        let _ = c.kill();
        let _ = c.wait();
    }

    // ---- session end through the session window (no note needs to see it)
    note::register_class();
    crate::log::set_dir(&mine);
    let opened = store::open(&mine).map(app::load).is_ok();
    let id = app::add("세션 종료", "", Some(0), 0);
    app::open_note(id, Some((-15000, -15000)), false);
    pump_for(50);
    let h = app::window_of(id).unwrap_or(std::ptr::null_mut());
    let session = shell::create_with(&mine, None, false);
    let body = unsafe { note::state_of(h).map(|s| s.body).unwrap_or(std::ptr::null_mut()) };
    // a first key commits at once; the rest waits for the 300 ms throttle
    for u in "로그오프 직전 입력".encode_utf16() {
        unsafe { SendMessageW(body, WM_CHAR, u as usize, 1) };
    }
    let answer = unsafe { SendMessageW(session, WM_QUERYENDSESSION, 0, 0x8000_0000u32 as isize) };
    let uuid = app::note(id).map(|n| n.uuid).unwrap_or_default();
    let kept = store::read_row(&mine.join("gaeunmemo.db"), &uuid).map(|r| r.body_text).unwrap_or_default();
    t.check("session end through the session window saves what was just typed", opened && !session.is_null() && answer == 1 && kept == "로그오프 직전 입력", format!("answer {answer}, stored {kept:?}"));

    // ---- F-17: the app icon
    let icon = shell::app_icon(16);
    let class_icon = unsafe { GetClassLongPtrW(h, GCLP_HICON) };
    t.check("app icon resource loads; note windows carry it", !icon.is_null() && class_icon != 0, String::new());

    // ---- F-19: the autostart value (a test key, never the real Run key)
    let cmd = autostart::command(Some(&mine));
    t.check("autostart command: this exe, --autostart, --data", cmd.contains(&exe.display().to_string()) && cmd.contains(" --autostart --data ") && cmd.ends_with(&format!("\"{}\"", mine.display())), cmd.clone());
    let wrote = autostart::set_enabled_in(TEST_KEY, true, Some(&mine));
    let read = autostart::current_in(TEST_KEY);
    let removed = autostart::set_enabled_in(TEST_KEY, false, Some(&mine)) && autostart::current_in(TEST_KEY).is_none();
    unsafe {
        let k = win::wide(r"Software\GaeunMemo2-test");
        windows_sys::Win32::System::Registry::RegDeleteTreeW(windows_sys::Win32::System::Registry::HKEY_CURRENT_USER, k.as_ptr());
    }
    t.check("autostart value: write, read back, remove (test key)", wrote && read.as_deref() == Some(cmd.as_str()) && removed, format!("{read:?}"));

    // ---- F-36: the log
    let log = mine.join("gaeunmemo.log");
    let _ = std::fs::write(&log, vec![b'x'; 300 * 1024]);
    crate::log::write("rotation check");
    let rotated = mine.join("gaeunmemo.log.old").exists() && std::fs::metadata(&log).map(|m| m.len() < 1024).unwrap_or(false);
    let line = std::fs::read_to_string(&log).unwrap_or_default();
    let pid = format!("[{}] rotation check", std::process::id());
    let dated = line.len() > 20 && line.as_bytes()[4] == b'-' && line.as_bytes()[13] == b':' && line.contains(&pid);
    t.check("log rotates at 256 KiB; lines carry local time and process id", rotated && dated, line.trim().to_string());

    for w in note::notes() {
        unsafe { DestroyWindow(w) };
    }
    store::close();
    let line = Json::default().s("mode", "p4check").n("pass", t.pass as f64).n("fail", t.fail as f64).raw("checks", format!("[{}]", t.out.join(","))).finish();
    metrics::append(out, &line);
    let _ = std::fs::remove_dir_all(&base);
}

/// `--mode p4child --data DIR`: plays the running app for `run` — holds the folder, has a
/// note off-screen and a session window (no tray), writes `shown.txt` when asked to show.
pub fn child(data: &str) {
    let dir = Path::new(data).to_path_buf();
    if !win::single_instance(&dir) {
        std::process::exit(5);
    }
    note::register_class();
    if store::open(&dir).map(app::load).is_err() {
        std::process::exit(6);
    }
    let id = app::add("열린 메모", "내용", Some(0), 0);
    app::open_note(id, Some((-15000, -15000)), false);
    shell::create_with(&dir, None, false);
    let _ = std::fs::write(dir.join("ready.txt"), b"1");
    loop {
        pump_for(20);
        if shell::SHOW_REQUESTS.load(std::sync::atomic::Ordering::Relaxed) > 0 && !dir.join("shown.txt").exists() {
            let _ = std::fs::write(dir.join("shown.txt"), b"1");
        }
    }
}
