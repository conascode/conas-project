//! The SQLite store (report §10, §12; F-21). The UI thread opens and loads the database once
//! at start (before the first window), then hands the connection to one writer thread. From
//! then on the UI only sends owned rows over a channel — no shared state, no locks, no file
//! I/O on the UI thread.
//!
//! * schema v1 (report §12): note, note_image, window_state, setting, meta
//! * WAL + synchronous=FULL: every commit survives a crash or power cut
//! * the writer merges what is waiting (latest row per note) into one transaction; the UI
//!   already throttles body saves like v1 `plan_write` (first change at once, then ≤ 300 ms)
//! * pictures: temp file → flush → rename, always before the note row that refers to them
//!   (same FIFO), so a crash never leaves a reference to a missing file
//! * start: quick_check; a damaged file is moved to broken\ and the newest good backup is
//!   restored; one backup a day (VACUUM INTO), 7 kept; pictures nobody refers to for 24 h
//!   are removed (v1.1 rule)
//!
//! Deleting is immediate like v1 (the soft delete of report §12.1 waits for approval).

use rusqlite::{params, Connection, OptionalExtension};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

pub const COLORS: [&str; 6] = ["#AFA9EC", "#F4C0D1", "#9FE1CB", "#F6C9A0", "#B5D4F4", "#F3E9C6"];

/// One note row, owned (sent to the writer).
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub id: String,
    pub title: String,
    pub body_text: String,
    pub body_html: String,
    pub color: usize,
    pub pinned: bool,
    pub folded: bool,
    pub is_open: bool,
    pub locked: bool,
    pub maximized: bool,
    /// logical px (96 dpi); None = never placed
    pub rect: Option<(f64, f64, f64, f64)>,
    pub font_size: u8,
    pub seq: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

pub enum Cmd {
    Upsert(Row),
    Delete(String),
    Setting(String, String),
    Window { name: String, open: bool, rect: Option<(f64, f64, f64, f64)> },
    Image { name: String, bytes: Arc<Vec<u8>> },
    Flush(Sender<()>),
    Stop,
}

pub struct Loaded {
    pub notes: Vec<Row>,
    pub settings: HashMap<String, String>,
    pub windows: HashMap<String, (bool, Option<(f64, f64, f64, f64)>)>,
    /// what happened at open (restored from a backup, …) for the user / the log
    pub notice: Option<String>,
}

thread_local! {
    static TX: RefCell<Option<Sender<Cmd>>> = const { RefCell::new(None) };
    static DIR: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    static WRITER: RefCell<Option<std::thread::JoinHandle<()>>> = const { RefCell::new(None) };
}

/// Commits done by the writer and the time of the last one (ms since UNIX epoch) — tests.
pub static COMMITS: AtomicU64 = AtomicU64::new(0);
pub static LAST_COMMIT_MS: AtomicU64 = AtomicU64::new(0);
pub static FAILURES: AtomicU64 = AtomicU64::new(0);

pub fn active() -> bool {
    TX.with(|t| t.borrow().is_some())
}

pub fn dir() -> Option<PathBuf> {
    DIR.with(|d| d.borrow().clone())
}

pub fn images_dir() -> Option<PathBuf> {
    dir().map(|d| d.join("images"))
}

pub fn send(c: Cmd) {
    TX.with(|t| {
        if let Some(tx) = t.borrow().as_ref() {
            let _ = tx.send(c);
        }
    });
}

/// Waits until everything sent so far is committed (window close, session end, exit).
pub fn flush() {
    let (tx, rx) = mpsc::channel();
    if !active() {
        return;
    }
    send(Cmd::Flush(tx));
    let _ = rx.recv_timeout(Duration::from_secs(2));
}

/// Flushes and stops the writer (program end).
pub fn close() {
    if !active() {
        return;
    }
    send(Cmd::Stop);
    TX.with(|t| t.borrow_mut().take());
    if let Some(h) = WRITER.with(|w| w.borrow_mut().take()) {
        let _ = h.join();
    }
    DIR.with(|d| d.borrow_mut().take());
}

/// The data folder of this build: `--data DIR`, else %APPDATA%\gaeunmemo2-dev (development
/// builds never use v1.1's %APPDATA%\gaeunmemo — user decision 2026-11-14).
pub fn default_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("gaeunmemo2-dev")
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS note (
  id TEXT PRIMARY KEY,
  title TEXT NOT NULL DEFAULT '',
  body_text TEXT NOT NULL DEFAULT '',
  body_html TEXT NOT NULL DEFAULT '',
  color TEXT NOT NULL CHECK (color IN ('#AFA9EC','#F4C0D1','#9FE1CB','#F6C9A0','#B5D4F4','#F3E9C6')),
  pinned INTEGER NOT NULL DEFAULT 0 CHECK (pinned IN (0,1)),
  folded INTEGER NOT NULL DEFAULT 0 CHECK (folded IN (0,1)),
  is_open INTEGER NOT NULL DEFAULT 1 CHECK (is_open IN (0,1)),
  locked INTEGER NOT NULL DEFAULT 0 CHECK (locked IN (0,1)),
  maximized INTEGER NOT NULL DEFAULT 0 CHECK (maximized IN (0,1)),
  x REAL, y REAL, w REAL, h REAL,
  font_size INTEGER NOT NULL DEFAULT 14 CHECK (font_size BETWEEN 10 AND 32),
  seq INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL DEFAULT 0,
  deleted_at INTEGER
);
CREATE INDEX IF NOT EXISTS note_live_updated ON note(updated_at DESC) WHERE deleted_at IS NULL;
CREATE TABLE IF NOT EXISTS note_image (
  note_id TEXT NOT NULL REFERENCES note(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  w INTEGER, h INTEGER,
  PRIMARY KEY (note_id, name)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS note_image_name ON note_image(name);
CREATE TABLE IF NOT EXISTS window_state (
  name TEXT PRIMARY KEY,
  is_open INTEGER NOT NULL DEFAULT 0,
  x REAL, y REAL, w REAL, h REAL
);
CREATE TABLE IF NOT EXISTS setting (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
";

fn configure(c: &Connection) -> rusqlite::Result<()> {
    c.pragma_update(None, "journal_mode", "WAL")?;
    c.pragma_update(None, "synchronous", "FULL")?;
    c.pragma_update(None, "foreign_keys", "ON")?;
    c.pragma_update(None, "cache_size", -512)?;
    c.busy_timeout(Duration::from_millis(2000))?;
    Ok(())
}

fn quick_check(path: &Path) -> bool {
    match Connection::open(path) {
        Ok(c) => c.query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0)).map(|s| s == "ok").unwrap_or(false),
        Err(_) => false,
    }
}

fn day_stamp() -> String {
    // UTC date is enough to give one backup per day and sort by name
    let days = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs() / 86_400).unwrap_or(0) as i64;
    let (y, m, d) = civil(days);
    format!("{y:04}{m:02}{d:02}")
}

/// Days since 1970-01-01 → (year, month, day) (H. Hinnant's civil_from_days).
fn civil(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Opens (or creates) the store in `dir`, checks it, loads everything and starts the writer.
pub fn open(dir: &Path) -> Result<Loaded, String> {
    std::fs::create_dir_all(dir.join("images")).map_err(|e| format!("데이터 폴더를 만들 수 없어요: {e}"))?;
    let path = dir.join("gaeunmemo.db");
    let mut notice = None;
    if path.exists() && !quick_check(&path) {
        // damaged: keep the file, restore the newest backup that passes the check
        let broken = dir.join("broken");
        let _ = std::fs::create_dir_all(&broken);
        let stamp = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        for suffix in ["", "-wal", "-shm"] {
            let f = dir.join(format!("gaeunmemo.db{suffix}"));
            if f.exists() {
                let _ = std::fs::rename(&f, broken.join(format!("gaeunmemo-{stamp}.db{suffix}")));
            }
        }
        let mut backups: Vec<PathBuf> = std::fs::read_dir(dir.join("backup"))
            .map(|r| r.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|x| x == "db")).collect())
            .unwrap_or_default();
        backups.sort();
        notice = Some("저장 파일이 손상되어 원본을 broken 폴더로 옮겼어요.".to_string());
        for b in backups.iter().rev() {
            if quick_check(b) && std::fs::copy(b, &path).is_ok() {
                notice = Some(format!("저장 파일이 손상되어 {} 백업으로 복원했어요. 원본은 broken 폴더에 있어요.", b.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
                break;
            }
        }
    }
    let conn = Connection::open(&path).map_err(|e| format!("저장 파일을 열 수 없어요: {e}"))?;
    configure(&conn).map_err(|e| e.to_string())?;
    conn.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
    conn.pragma_update(None, "user_version", 1).map_err(|e| e.to_string())?;

    let mut notes = Vec::new();
    {
        let mut st = conn
            .prepare("SELECT id,title,body_text,body_html,color,pinned,folded,is_open,locked,maximized,x,y,w,h,font_size,seq,created_at,updated_at FROM note WHERE deleted_at IS NULL ORDER BY seq")
            .map_err(|e| e.to_string())?;
        let rows = st
            .query_map([], |r| {
                let color: String = r.get(4)?;
                let rect = match (r.get::<_, Option<f64>>(10)?, r.get::<_, Option<f64>>(11)?, r.get::<_, Option<f64>>(12)?, r.get::<_, Option<f64>>(13)?) {
                    (Some(x), Some(y), Some(w), Some(h)) => Some((x, y, w, h)),
                    _ => None,
                };
                Ok(Row {
                    id: r.get(0)?,
                    title: r.get(1)?,
                    body_text: r.get(2)?,
                    body_html: r.get(3)?,
                    color: COLORS.iter().position(|c| c.eq_ignore_ascii_case(&color)).unwrap_or(0),
                    pinned: r.get::<_, i64>(5)? != 0,
                    folded: r.get::<_, i64>(6)? != 0,
                    is_open: r.get::<_, i64>(7)? != 0,
                    locked: r.get::<_, i64>(8)? != 0,
                    maximized: r.get::<_, i64>(9)? != 0,
                    rect,
                    font_size: r.get::<_, i64>(14)?.clamp(10, 32) as u8,
                    seq: r.get(15)?,
                    created_at: r.get(16)?,
                    updated_at: r.get(17)?,
                })
            })
            .map_err(|e| e.to_string())?;
        for row in rows {
            notes.push(row.map_err(|e| e.to_string())?);
        }
    }
    let settings: HashMap<String, String> = {
        let mut st = conn.prepare("SELECT key, value FROM setting").map_err(|e| e.to_string())?;
        let it = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map_err(|e| e.to_string())?;
        it.filter_map(Result::ok).collect()
    };
    let windows = {
        let mut st = conn.prepare("SELECT name, is_open, x, y, w, h FROM window_state").map_err(|e| e.to_string())?;
        let it = st
            .query_map([], |r| {
                let rect = match (r.get::<_, Option<f64>>(2)?, r.get::<_, Option<f64>>(3)?, r.get::<_, Option<f64>>(4)?, r.get::<_, Option<f64>>(5)?) {
                    (Some(x), Some(y), Some(w), Some(h)) => Some((x, y, w, h)),
                    _ => None,
                };
                Ok((r.get::<_, String>(0)?, (r.get::<_, i64>(1)? != 0, rect)))
            })
            .map_err(|e| e.to_string())?;
        it.filter_map(Result::ok).collect()
    };

    let (tx, rx) = mpsc::channel();
    let d = dir.to_path_buf();
    let handle = std::thread::Builder::new().name("gm-store".into()).spawn(move || writer(conn, d, rx)).map_err(|e| e.to_string())?;
    TX.with(|t| *t.borrow_mut() = Some(tx));
    DIR.with(|x| *x.borrow_mut() = Some(dir.to_path_buf()));
    WRITER.with(|w| *w.borrow_mut() = Some(handle));
    Ok(Loaded { notes, settings, windows, notice })
}

// ------------------------------------------------------------------ writer thread

#[derive(Default)]
struct Pending {
    upserts: HashMap<String, Row>,
    order: Vec<String>,
    deletes: HashSet<String>,
    settings: HashMap<String, String>,
    windows: HashMap<String, (bool, Option<(f64, f64, f64, f64)>)>,
}

impl Pending {
    fn is_empty(&self) -> bool {
        self.upserts.is_empty() && self.deletes.is_empty() && self.settings.is_empty() && self.windows.is_empty()
    }
    fn take(&mut self, c: Cmd, flushes: &mut Vec<Sender<()>>, dir: &Path) -> bool {
        match c {
            Cmd::Upsert(r) => {
                self.deletes.remove(&r.id);
                if !self.upserts.contains_key(&r.id) {
                    self.order.push(r.id.clone());
                }
                self.upserts.insert(r.id.clone(), r);
            }
            Cmd::Delete(id) => {
                self.upserts.remove(&id);
                self.order.retain(|x| *x != id);
                self.deletes.insert(id);
            }
            Cmd::Setting(k, v) => {
                self.settings.insert(k, v);
            }
            Cmd::Window { name, open, rect } => {
                self.windows.insert(name, (open, rect));
            }
            Cmd::Image { name, bytes } => {
                // before any later note row that refers to it (same channel order)
                if let Err(e) = write_image(&dir.join("images"), &name, &bytes) {
                    FAILURES.fetch_add(1, Ordering::Relaxed);
                    crate::log::write(&format!("사진 저장 실패 {name}: {e}"));
                }
            }
            Cmd::Flush(s) => flushes.push(s),
            Cmd::Stop => return false,
        }
        true
    }
}

/// `images\<name>`: temp file, flushed to disk, then renamed (v1.1 write_atomic).
pub fn write_image(dir: &Path, name: &str, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    if !crate::rich::is_image_name(name) {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "bad picture name"));
    }
    let target = dir.join(name);
    if target.exists() {
        return Ok(());
    }
    let tmp = dir.join(format!("{name}.tmp"));
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, &target)
}

fn commit(conn: &mut Connection, p: &Pending) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    {
        let mut up = tx.prepare_cached(
            "INSERT INTO note (id,title,body_text,body_html,color,pinned,folded,is_open,locked,maximized,x,y,w,h,font_size,seq,created_at,updated_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18)
             ON CONFLICT(id) DO UPDATE SET title=excluded.title, body_text=excluded.body_text, body_html=excluded.body_html,
               color=excluded.color, pinned=excluded.pinned, folded=excluded.folded, is_open=excluded.is_open, locked=excluded.locked,
               maximized=excluded.maximized, x=excluded.x, y=excluded.y, w=excluded.w, h=excluded.h, font_size=excluded.font_size,
               seq=excluded.seq, updated_at=excluded.updated_at, deleted_at=NULL",
        )?;
        let mut clear = tx.prepare_cached("DELETE FROM note_image WHERE note_id=?1")?;
        let mut img = tx.prepare_cached("INSERT OR IGNORE INTO note_image (note_id, name) VALUES (?1, ?2)")?;
        for id in &p.order {
            let Some(r) = p.upserts.get(id) else { continue };
            let (x, y, w, h) = match r.rect {
                Some((x, y, w, h)) => (Some(x), Some(y), Some(w), Some(h)),
                None => (None, None, None, None),
            };
            up.execute(params![
                r.id,
                r.title,
                r.body_text,
                r.body_html,
                COLORS[r.color.min(5)],
                r.pinned as i64,
                r.folded as i64,
                r.is_open as i64,
                r.locked as i64,
                r.maximized as i64,
                x,
                y,
                w,
                h,
                r.font_size.clamp(10, 32) as i64,
                r.seq,
                r.created_at,
                r.updated_at
            ])?;
            // derived index of the pictures this note refers to (v1 image_refs)
            clear.execute(params![r.id])?;
            for name in image_refs(&r.body_html) {
                img.execute(params![r.id, name])?;
            }
        }
        let mut del = tx.prepare_cached("DELETE FROM note WHERE id=?1")?;
        for id in &p.deletes {
            del.execute(params![id])?;
        }
        let mut set = tx.prepare_cached("INSERT INTO setting (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value")?;
        for (k, v) in &p.settings {
            set.execute(params![k, v])?;
        }
        let mut win = tx.prepare_cached(
            "INSERT INTO window_state (name,is_open,x,y,w,h) VALUES (?1,?2,?3,?4,?5,?6)
             ON CONFLICT(name) DO UPDATE SET is_open=excluded.is_open, x=excluded.x, y=excluded.y, w=excluded.w, h=excluded.h",
        )?;
        for (name, (open, rect)) in &p.windows {
            let (x, y, w, h) = match rect {
                Some((x, y, w, h)) => (Some(*x), Some(*y), Some(*w), Some(*h)),
                None => (None, None, None, None),
            };
            win.execute(params![name, *open as i64, x, y, w, h])?;
        }
    }
    tx.commit()
}

/// v1 `image_refs`: valid picture names in `data-gm="…"`, in order, no duplicates.
pub fn image_refs(html: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for part in html.split("data-gm=\"").skip(1) {
        if let Some(name) = part.split('"').next() {
            if crate::rich::is_image_name(name) && !out.iter().any(|n| n == name) {
                out.push(name.to_string());
            }
        }
    }
    out
}

fn backup_if_due(conn: &Connection, dir: &Path) {
    let bdir = dir.join("backup");
    let _ = std::fs::create_dir_all(&bdir);
    let target = bdir.join(format!("gaeunmemo-{}.db", day_stamp()));
    if target.exists() {
        return;
    }
    let path = target.to_string_lossy().replace('\'', "''");
    if conn.execute_batch(&format!("VACUUM INTO '{path}'")).is_err() {
        crate::log::write("백업 실패 (다음 쓰기 때 다시 시도)");
        return;
    }
    let mut all: Vec<PathBuf> = std::fs::read_dir(&bdir)
        .map(|r| r.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("gaeunmemo-"))).collect())
        .unwrap_or_default();
    all.sort();
    while all.len() > 7 {
        let _ = std::fs::remove_file(all.remove(0));
    }
}

/// v1.1: picture files no note refers to, untouched for 24 h, are removed.
fn remove_orphans(conn: &Connection, dir: &Path) {
    let used: HashSet<String> = conn
        .prepare("SELECT DISTINCT name FROM note_image")
        .and_then(|mut s| s.query_map([], |r| r.get::<_, String>(0)).map(|it| it.filter_map(Result::ok).collect()))
        .unwrap_or_default();
    let Ok(rd) = std::fs::read_dir(dir.join("images")) else { return };
    let day = Duration::from_secs(24 * 3600);
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|a| a > day);
        let stale_tmp = name.ends_with(".tmp");
        if old && (stale_tmp || (crate::rich::is_image_name(&name) && !used.contains(&name))) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

fn writer(mut conn: Connection, dir: PathBuf, rx: Receiver<Cmd>) {
    backup_if_due(&conn, &dir);
    remove_orphans(&conn, &dir);
    let mut pending = Pending::default();
    let mut flushes: Vec<Sender<()>> = Vec::new();
    let mut backoff = Duration::from_secs(1);
    let mut retry_at: Option<Instant> = None;
    let mut running = true;
    while running {
        // wait for work (or for the retry time of a failed commit)
        let first = match retry_at {
            Some(t) => match rx.recv_timeout(t.saturating_duration_since(Instant::now())) {
                Ok(c) => Some(c),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => {
                    running = false;
                    None
                }
            },
            None => match rx.recv() {
                Ok(c) => Some(c),
                Err(_) => {
                    running = false;
                    None
                }
            },
        };
        if let Some(c) = first {
            running &= pending.take(c, &mut flushes, &dir);
        }
        while let Ok(c) = rx.try_recv() {
            running &= pending.take(c, &mut flushes, &dir);
        }
        if retry_at.is_some_and(|t| Instant::now() < t) && running && flushes.is_empty() {
            continue;
        }
        if !pending.is_empty() {
            match commit(&mut conn, &pending) {
                Ok(()) => {
                    pending = Pending::default();
                    retry_at = None;
                    backoff = Duration::from_secs(1);
                    COMMITS.fetch_add(1, Ordering::Relaxed);
                    LAST_COMMIT_MS.store(crate::app::now_ms(), Ordering::Relaxed);
                }
                Err(e) => {
                    // disk full / no permission: keep everything and retry 1, 2, 4 … 60 s
                    FAILURES.fetch_add(1, Ordering::Relaxed);
                    crate::log::write(&format!("저장 실패, {}초 뒤 다시 시도: {e}", backoff.as_secs()));
                    retry_at = Some(Instant::now() + backoff);
                    backoff = (backoff * 2).min(Duration::from_secs(60));
                }
            }
        }
        if pending.is_empty() {
            for f in flushes.drain(..) {
                let _ = f.send(());
            }
        }
    }
    if !pending.is_empty() {
        let _ = commit(&mut conn, &pending);
    }
    for f in flushes.drain(..) {
        let _ = f.send(());
    }
    let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)");
}

/// The note row a test or tool reads back (`--mode p3check`).
pub fn read_row(path: &Path, id: &str) -> Option<Row> {
    let c = Connection::open(path).ok()?;
    c.query_row(
        "SELECT id,title,body_text,body_html,color,pinned,folded,is_open,locked,maximized,font_size,seq,created_at,updated_at FROM note WHERE id=?1",
        params![id],
        |r| {
            let color: String = r.get(4)?;
            Ok(Row {
                id: r.get(0)?,
                title: r.get(1)?,
                body_text: r.get(2)?,
                body_html: r.get(3)?,
                color: COLORS.iter().position(|c| *c == color).unwrap_or(0),
                pinned: r.get::<_, i64>(5)? != 0,
                folded: r.get::<_, i64>(6)? != 0,
                is_open: r.get::<_, i64>(7)? != 0,
                locked: r.get::<_, i64>(8)? != 0,
                maximized: r.get::<_, i64>(9)? != 0,
                rect: None,
                font_size: r.get::<_, i64>(10)? as u8,
                seq: r.get(11)?,
                created_at: r.get(12)?,
                updated_at: r.get(13)?,
            })
        },
    )
    .optional()
    .ok()
    .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(20_771), (2026, 11, 14));
    }

    #[test]
    fn image_refs_like_v1() {
        let ok = "0123abcd-0123-4567-89ab-0123456789ab.png";
        let html = format!(r#"a<img data-gm="{ok}">b<img data-gm="../evil.exe"><img data-gm="x.png"><img data-gm="{ok}">"#);
        assert_eq!(image_refs(&html), vec![ok.to_string()]);
    }
}
