//! `--mode p3check --data DIR --out FILE`: P3 exit checks on a real store in DIR (a fresh
//! test folder, never the user's data). Windows are off-screen and never focused.
//!
//! * F-09 / F-10 / F-11: new note stored, × keeps a note with content (closed) and removes
//!   an empty one, delete removes the row, a locked note refuses
//! * F-21: save latency — first change committed at once, a later one within the 300 ms
//!   throttle (+ commit)
//! * settings and note fields (colour, pin, fold, size, lock, theme, band) survive a restart
//! * F-31: a pasted picture is a file in images\ before any row refers to it; note_image
//!   index; the picture comes back after a restart
//! * damaged database: original kept in broken\, newest good backup restored
//! * picture clean-up: only files nobody refers to and older than 24 h are removed

use crate::app;
use crate::images;
use crate::metrics::{self, Json};
use crate::note;
use crate::rich::{self, Doc};
use crate::richedit;
use crate::store;
use std::path::Path;
use std::sync::atomic::Ordering;
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

/// Opens the store and loads it into the app (no windows).
fn start(dir: &Path) -> Option<String> {
    match store::open(dir) {
        Ok(l) => {
            let notice = l.notice.clone();
            app::load(l);
            notice
        }
        Err(e) => Some(format!("open failed: {e}")),
    }
}

fn restart(dir: &Path) -> Option<String> {
    for h in note::notes() {
        unsafe { DestroyWindow(h) };
    }
    pump_for(20);
    store::close();
    app::with(|a| {
        a.notes.clear();
        a.windows.clear();
    });
    start(dir)
}

fn by_title(t: &str) -> Option<app::Note> {
    app::with(|a| a.notes.iter().find(|n| n.title == t).cloned())
}

fn open_off(id: u64) -> windows_sys::Win32::Foundation::HWND {
    app::open_note(id, Some((-15000, -15000)), false);
    pump_for(10);
    app::window_of(id).unwrap_or(std::ptr::null_mut())
}

/// Waits for the writer's next commit after `before` commits; returns ms waited.
fn wait_commit(before: u64, limit_ms: u64) -> Option<f64> {
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_millis(limit_ms) {
        pump_for(1);
        if store::COMMITS.load(Ordering::Relaxed) > before {
            return Some(t0.elapsed().as_secs_f64() * 1000.0);
        }
    }
    None
}

fn type_into(h: windows_sys::Win32::Foundation::HWND, s: &str) {
    let body = unsafe { note::state_of(h).map(|s| s.body).unwrap_or(std::ptr::null_mut()) };
    for u in s.encode_utf16() {
        unsafe { SendMessageW(body, WM_CHAR, u as usize, 1) };
    }
}

pub fn run(data: &str, out: &str) {
    let dir = Path::new(data).to_path_buf();
    let mut t = T { out: Vec::new(), pass: 0, fail: 0 };
    if data.is_empty() || dir.join("gaeunmemo.db").exists() {
        t.check("test folder is new (--data)", false, data.to_string());
        metrics::append(out, &Json::default().s("mode", "p3check").raw("checks", format!("[{}]", t.out.join(","))).finish());
        return;
    }
    note::register_class();
    crate::log::set_dir(&dir);
    let mem0 = metrics::sample();
    t.check("store opens in a new folder", start(&dir).is_none(), String::new());
    let mem_store = metrics::sample();

    // ---- F-09: a new note is stored
    let c0 = store::COMMITS.load(Ordering::Relaxed);
    let id = app::add("저장 시험", "", Some(2), 0);
    let h = open_off(id);
    t.check("new note committed", wait_commit(c0, 1000).is_some(), String::new());

    // ---- F-21: save latency (first change at once, a later one within the throttle)
    let saves0 = crate::stats::get().saves;
    pump_for(500); // a user starts typing a moment after the note opens
    let saves1 = crate::stats::get().saves;
    let c1 = store::COMMITS.load(Ordering::Relaxed);
    let t0 = Instant::now();
    type_into(h, "가");
    let saves2 = crate::stats::get().saves;
    let first = wait_commit(c1, 1000).map(|_| t0.elapsed().as_secs_f64() * 1000.0);
    t.check("opening a note writes nothing; the first key saves", saves0 == 0 && saves1 == 0 && saves2 == 1, format!("open {saves0} → idle {saves1} → typed {saves2}"));
    let c2 = store::COMMITS.load(Ordering::Relaxed);
    let t1 = Instant::now();
    type_into(h, "나다");
    let later = wait_commit(c2, 1500).map(|_| t1.elapsed().as_secs_f64() * 1000.0);
    t.check("first change committed at once (≤ 50 ms)", first.is_some_and(|ms| ms <= 50.0), format!("{first:?} ms"));
    t.check("later change committed within 300 ms throttle (≤ 350 ms)", later.is_some_and(|ms| ms <= 350.0), format!("{later:?} ms"));

    // ---- note fields and settings
    app::update(id, |n| {
        n.color = 4;
        n.pinned = true;
        n.font_size = 18;
    });
    unsafe { SendMessageW(h, note::WM_APP_MENU, crate::menu::Act::Lock.code(), 0) };
    app::set_theme(crate::theme::Theme::Light);
    app::cycle_band(); // thin → full
    app::set_rect(id, (100.0, 120.0, 300.0, 280.0));

    // ---- F-31: a pasted picture (file before the row, index, back after restart)
    let other = app::add("사진 메모", "", Some(1), 0);
    let ho = open_off(other);
    let body = unsafe { note::state_of(ho).unwrap().body };
    let png = images::sample_png(80, 60);
    richedit::paste_for_test(body, &richedit::test_data(vec![("PNG", png.clone())]), false);
    pump_for(400);
    store::flush();
    let html = app::note(other).map(|n| n.html).unwrap_or_default();
    let pic = store::image_refs(&html).into_iter().next().unwrap_or_default();
    let file = dir.join("images").join(&pic);
    t.check("pasted picture saved as a file in images\\", !pic.is_empty() && std::fs::read(&file).ok().as_deref() == Some(png.as_slice()), pic.clone());
    let indexed = rusqlite::Connection::open(dir.join("gaeunmemo.db"))
        .and_then(|c| c.query_row("SELECT COUNT(*) FROM note_image WHERE name=?1", [&pic], |r| r.get::<_, i64>(0)))
        .unwrap_or(0);
    t.check("note_image index has the picture", indexed == 1, format!("{indexed}"));

    // ---- F-10 / F-11: close and delete
    let keep = app::add("닫아도 남는 메모", "내용", Some(0), 0);
    open_off(keep);
    app::close_note(keep);
    let empty = app::add("", "", Some(0), 0);
    open_off(empty);
    let empty_uuid = app::note(empty).map(|n| n.uuid).unwrap_or_default();
    app::close_note(empty);
    let gone = app::add("지울 메모", "내용", Some(0), 0);
    let gone_uuid = app::note(gone).map(|n| n.uuid).unwrap_or_default();
    open_off(gone);
    let deleted = app::delete_note(gone);
    let refused = !app::delete_note(id); // locked
    store::flush();
    let db = dir.join("gaeunmemo.db");
    t.check("× keeps a note with content (stored as closed)", store::read_row(&db, &app::note(keep).map(|n| n.uuid).unwrap_or_default()).is_some_and(|r| !r.is_open), String::new());
    t.check("× removes an empty note from the store", store::read_row(&db, &empty_uuid).is_none(), String::new());
    t.check("delete removes the row", deleted && store::read_row(&db, &gone_uuid).is_none(), String::new());
    t.check("a locked note refuses delete", refused && store::read_row(&db, &app::note(id).map(|n| n.uuid).unwrap_or_default()).is_some(), String::new());

    // ---- restart: everything comes back
    let before: Vec<(String, String, String, usize, bool, bool, u8)> =
        app::with(|a| a.notes.iter().map(|n| (n.uuid.clone(), n.title.clone(), n.html.clone(), n.color, n.pinned, n.locked, n.font_size)).collect());
    let notice = restart(&dir);
    let after: Vec<(String, String, String, usize, bool, bool, u8)> =
        app::with(|a| a.notes.iter().map(|n| (n.uuid.clone(), n.title.clone(), n.html.clone(), n.color, n.pinned, n.locked, n.font_size)).collect());
    t.check("restart: notes come back unchanged", notice.is_none() && before == after, format!("{} notes", after.len()));
    let n = by_title("저장 시험");
    t.check("restart: typed text kept", n.as_ref().is_some_and(|n| n.text == "가나다"), format!("{:?}", n.as_ref().map(|n| n.text.clone())));
    t.check("restart: rect kept (logical px)", n.as_ref().is_some_and(|n| n.rect == Some((100.0, 120.0, 300.0, 280.0))), format!("{:?}", n.as_ref().map(|n| n.rect)));
    t.check("restart: theme light, band 제목 줄", app::theme() == crate::theme::Theme::Light && app::band() == crate::theme::Band::Full, String::new());
    let pn = by_title("사진 메모");
    t.check("restart: picture reference kept", pn.as_ref().is_some_and(|n| n.html.contains(&pic)), String::new());
    // the picture shows again: its file is read by the decoding thread
    if let Some(pn) = pn {
        let hp = open_off(pn.id);
        pump_for(600);
        let body = unsafe { note::state_of(hp).unwrap().body };
        let d = richedit::save(body);
        t.check("restart: picture loads from images\\", d.normal().iter().any(|(_, l)| l.iter().any(|i| matches!(i, rich::Item::Img(x) if *x == pic))), String::new());
    }

    // ---- picture clean-up: only old orphans go
    let orphan_old = dir.join("images").join(format!("{}.png", crate::win::uuid()));
    let orphan_new = dir.join("images").join(format!("{}.png", crate::win::uuid()));
    let _ = std::fs::write(&orphan_old, &png);
    let _ = std::fs::write(&orphan_new, &png);
    let two_days = std::time::SystemTime::now() - Duration::from_secs(48 * 3600);
    let _ = std::fs::File::options().write(true).open(&orphan_old).and_then(|f| f.set_modified(two_days));
    let _ = std::fs::File::options().write(true).open(&file).and_then(|f| f.set_modified(two_days));
    restart(&dir);
    pump_for(300);
    t.check("old orphan picture removed", !orphan_old.exists(), String::new());
    t.check("young orphan kept (24 h grace)", orphan_new.exists(), String::new());
    t.check("old referenced picture kept", file.exists(), String::new());

    // ---- disk full (report §14.2): SQLite's own "database or disk is full", made by capping
    // the file at its current size. Nothing typed is dropped: the commit is retried (1, 2, 4 …
    // 60 s) and goes through once there is room again.
    let db = dir.join("gaeunmemo.db");
    let full = app::add("가득 참", "가득 전", Some(0), 0);
    store::flush();
    let full_uuid = app::note(full).map(|n| n.uuid).unwrap_or_default();
    let pages = rusqlite::Connection::open(&db).and_then(|c| c.query_row("PRAGMA page_count", [], |r| r.get::<_, i64>(0))).unwrap_or(0);
    store::send(store::Cmd::TestSql(format!("PRAGMA max_page_count = {pages}")));
    let fails0 = store::FAILURES.load(Ordering::Relaxed);
    let big: String = "가나다라마바사 ".repeat(25_000);
    app::update(full, |n| {
        n.text = big.clone();
        n.html = big.clone();
    });
    pump_for(800);
    let fails1 = store::FAILURES.load(Ordering::Relaxed);
    let kept = store::read_row(&db, &full_uuid).map(|r| r.body_text).unwrap_or_default();
    let logged = std::fs::read_to_string(dir.join("gaeunmemo.log")).unwrap_or_default();
    t.check("disk full: the commit fails, is logged, and the stored note is the last good one", fails1 > fails0 && kept == "가득 전" && logged.contains("저장 실패"), format!("failures {fails0} → {fails1}, stored {} chars", kept.chars().count()));
    t.check("disk full: the database still opens (quick_check)", rusqlite::Connection::open(&db).and_then(|c| c.query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0))).is_ok_and(|s| s == "ok"), String::new());
    store::send(store::Cmd::TestSql("PRAGMA max_page_count = 1073741823".into()));
    let t_room = Instant::now();
    let mut back = false;
    while t_room.elapsed() < Duration::from_secs(10) {
        pump_for(100);
        if store::read_row(&db, &full_uuid).is_some_and(|r| r.body_text == big) {
            back = true;
            break;
        }
    }
    t.check("disk full: once there is room the waiting text is committed (retry)", back, format!("{:.1} s", t_room.elapsed().as_secs_f64()));

    // ---- a picture file that cannot be written (full disk, no permission): the note row
    // that refers to it waits with it — never a reference to a missing file — and both go
    // in once the file can be written
    let images = dir.join("images");
    let parked = dir.join("images.parked");
    let _ = std::fs::rename(&images, &parked);
    let _ = std::fs::write(&images, b"not a folder");
    let pnote = app::add("사진 실패", "", Some(0), 0);
    let hp = open_off(pnote);
    let bodyp = unsafe { note::state_of(hp).unwrap().body };
    let png2 = images::sample_png(50, 40);
    let fails2 = store::FAILURES.load(Ordering::Relaxed);
    richedit::paste_for_test(bodyp, &richedit::test_data(vec![("PNG", png2.clone())]), false);
    pump_for(800);
    let puuid = app::note(pnote).map(|n| n.uuid).unwrap_or_default();
    let pname = app::note(pnote).map(|n| store::image_refs(&n.html)).unwrap_or_default().into_iter().next().unwrap_or_default();
    let row_html = store::read_row(&db, &puuid).map(|r| r.body_html).unwrap_or_default();
    t.check("picture write fails: the row referring to it is not committed", !pname.is_empty() && !row_html.contains(&pname) && store::FAILURES.load(Ordering::Relaxed) > fails2, format!("{pname}, stored html {} chars", row_html.len()));
    let _ = std::fs::remove_file(&images);
    let _ = std::fs::rename(&parked, &images);
    let t_pic = Instant::now();
    let mut pic_back = false;
    while t_pic.elapsed() < Duration::from_secs(10) {
        pump_for(100);
        if store::read_row(&db, &puuid).is_some_and(|r| r.body_html.contains(&pname)) {
            pic_back = true;
            break;
        }
    }
    let on_disk = std::fs::read(images.join(&pname)).ok().as_deref() == Some(png2.as_slice());
    t.check("picture write fails: file and row both go in once it can be written", pic_back && on_disk, format!("{:.1} s, file {on_disk}", t_pic.elapsed().as_secs_f64()));

    // ---- damaged database: kept in broken\, newest good backup restored
    for h in note::notes() {
        unsafe { DestroyWindow(h) };
    }
    store::close();
    let backups = std::fs::read_dir(dir.join("backup")).map(|r| r.count()).unwrap_or(0);
    {
        use std::io::{Seek, SeekFrom, Write};
        if let Ok(mut f) = std::fs::OpenOptions::new().write(true).open(&db) {
            let _ = f.seek(SeekFrom::Start(0));
            let _ = f.write_all(b"this is not a database at all, the header is gone.........");
        }
    }
    app::with(|a| {
        a.notes.clear();
        a.windows.clear();
    });
    let notice = start(&dir);
    let broken = std::fs::read_dir(dir.join("broken")).map(|r| r.count()).unwrap_or(0);
    t.check("damaged database: original moved to broken\\", broken >= 1, format!("{broken} file(s)"));
    t.check("damaged database: a backup is restored and opens", notice.as_deref().is_some_and(|m| m.contains("백업")) && backups >= 1, format!("{notice:?}, {backups} backup(s)"));
    store::close();

    // ---- what a note row looks like on disk (v1 fields, report §12)
    let schema_ok = rusqlite::Connection::open(&db)
        .and_then(|c| c.query_row("SELECT COUNT(*) FROM pragma_table_info('note') WHERE name IN ('id','title','body_text','body_html','color','seq','created_at','updated_at')", [], |r| r.get::<_, i64>(0)))
        .unwrap_or(0);
    t.check("schema v1 note columns", schema_ok == 8, format!("{schema_ok}"));

    let _ = Doc::default();
    let line = Json::default()
        .s("mode", "p3check")
        .n("pass", t.pass as f64)
        .n("fail", t.fail as f64)
        .n("first_commit_ms", first.unwrap_or(-1.0))
        .n("later_commit_ms", later.unwrap_or(-1.0))
        .n("private_mb_before_store", mem0.private_mb)
        .n("private_mb_store_open", mem_store.private_mb)
        .raw("checks", format!("[{}]", t.out.join(",")))
        .finish();
    metrics::append(out, &line);
}

/// `--mode p3kill --data DIR [--label picture]`: the child of bench/p3/kill.py. Types a known
/// text into a stored note (20 ms between keys) — or pastes a picture — then writes
/// DIR\typed.txt and keeps running until the parent kills it.
pub fn kill_child(data: &str, label: &str) {
    let dir = Path::new(data).to_path_buf();
    note::register_class();
    crate::log::set_dir(&dir);
    if start(&dir).is_some() {
        std::process::exit(3);
    }
    let id = app::add("강제 종료", "", Some(0), 0);
    let h = open_off(id);
    pump_for(50);
    let type_text = |text: &str| {
        for ch in text.chars() {
            let mut b = [0u16; 2];
            type_into(h, ch.encode_utf16(&mut b).iter().map(|&u| char::from_u32(u as u32).unwrap_or(' ')).collect::<String>().as_str());
            pump_for(20);
        }
    };
    if label == "commit" {
        // a first text, committed; then more typing whose commit the test kills inside the
        // transaction (store::test_hold): the database must still hold exactly the first text
        type_text("첫 저장 완료");
        pump_for(400); // the throttled save goes out
        crate::store::flush();
        let _ = std::fs::write(dir.join("first.txt"), "첫 저장 완료");
        let _ = std::fs::write(dir.join("hold"), b"1");
        type_text(" 추가 입력");
        loop {
            pump_for(1000);
        }
    }
    if label == "endsession" {
        // log off: the test sends WM_QUERYENDSESSION / WM_ENDSESSION right after the last
        // key and then kills the process, as Windows does after WM_ENDSESSION
        type_text("강제 종료 시험 0123456789 마지막");
        let _ = std::fs::write(dir.join("hwnd.txt"), format!("{}", h as isize));
        let _ = std::fs::write(dir.join("typed.txt"), "강제 종료 시험 0123456789 마지막");
        loop {
            pump_for(1000);
        }
    }
    let typed = if label == "picture" {
        let body = unsafe { note::state_of(h).unwrap().body };
        let png = images::sample_png(120, 90);
        richedit::paste_for_test(body, &richedit::test_data(vec![("PNG", png)]), false);
        String::new()
    } else {
        let text = "강제 종료 시험 0123456789 마지막";
        for ch in text.chars() {
            let mut b = [0u16; 2];
            type_into(h, ch.encode_utf16(&mut b).iter().map(|&u| char::from_u32(u as u32).unwrap_or(' ')).collect::<String>().as_str());
            pump_for(20);
        }
        text.to_string()
    };
    let _ = std::fs::write(dir.join("typed.txt"), &typed);
    loop {
        pump_for(1000);
    }
}
