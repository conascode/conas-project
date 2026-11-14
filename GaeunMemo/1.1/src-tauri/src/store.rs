//! Loading, parsing, backups and atomic writes, plus the background saver.

use crate::model::Data;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const FILE_NAME: &str = "notes.json";
pub const BACKUP_KEEP: usize = 7;

#[derive(Debug, PartialEq)]
pub enum LoadOutcome {
    Loaded,
    Missing,
    Broken(PathBuf),
}

pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    base.join("gaeunmemo")
}

pub fn parse(s: &str) -> Result<Data, serde_json::Error> {
    let s = s.trim_start_matches('\u{feff}');
    let mut d: Data = serde_json::from_str(s)?;
    d.sanitize();
    Ok(d)
}

/// Reads `path`. A file that does not parse is copied to
/// `notes-broken-<stamp>.json` next to it and an empty state is returned.
pub fn load(path: &Path, stamp: &str) -> (Data, LoadOutcome) {
    let raw = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return (Data::default(), LoadOutcome::Missing),
        Err(_) => Vec::new(),
    };
    let text = String::from_utf8_lossy(&raw);
    match parse(&text) {
        Ok(d) => (d, LoadOutcome::Loaded),
        Err(_) => {
            let dir = path.parent().unwrap_or_else(|| Path::new("."));
            let backup = unique(dir, &format!("notes-broken-{stamp}"));
            let _ = fs::write(&backup, &raw);
            let mut d = Data::default();
            // Not a fresh install: don't re-run first-run actions.
            d.settings.initialized = true;
            (d, LoadOutcome::Broken(backup))
        }
    }
}

fn unique(dir: &Path, stem: &str) -> PathBuf {
    let mut p = dir.join(format!("{stem}.json"));
    let mut i = 1;
    while p.exists() {
        p = dir.join(format!("{stem}-{i}.json"));
        i += 1;
    }
    p
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_atomic_with(path, bytes, |a, b| fs::rename(a, b))
}

/// Temp file + rename; if the rename fails, overwrite in place.
pub fn write_atomic_with(
    path: &Path,
    bytes: &[u8],
    rename: impl Fn(&Path, &Path) -> io::Result<()>,
) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    let staged = (|| {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()
    })();
    if staged.is_ok() && rename(&tmp, path).is_ok() {
        return Ok(());
    }
    let _ = fs::remove_file(&tmp);
    fs::write(path, bytes)
}

/// Copies `src` to `notes-backup-<date>.json` once per day and keeps the newest `keep`.
pub fn daily_backup(src: &Path, date: &str, keep: usize) -> io::Result<()> {
    let dir = src.parent().unwrap_or_else(|| Path::new("."));
    let target = dir.join(format!("notes-backup-{date}.json"));
    if src.exists() && !target.exists() {
        fs::copy(src, &target)?;
    }
    let mut backups: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("notes-backup-") && n.ends_with(".json"))
                .unwrap_or(false)
        })
        .collect();
    backups.sort();
    backups.reverse();
    for old in backups.into_iter().skip(keep) {
        let _ = fs::remove_file(old);
    }
    Ok(())
}

// ---------- local clock (no chrono) ----------

/// (year, month, day, hour, minute, second) in local time.
pub fn local_now() -> (u16, u16, u16, u16, u16, u16) {
    #[cfg(windows)]
    unsafe {
        let mut t: windows_sys::Win32::Foundation::SYSTEMTIME = std::mem::zeroed();
        windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut t);
        (t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
    }
    #[cfg(not(windows))]
    {
        (1970, 1, 1, 0, 0, 0)
    }
}

pub fn today() -> String {
    let (y, m, d, ..) = local_now();
    format!("{y:04}{m:02}{d:02}")
}

pub fn stamp() -> String {
    let (y, m, d, h, mi, s) = local_now();
    format!("{y:04}{m:02}{d:02}-{h:02}{mi:02}{s:02}")
}

// ---------- background saver ----------

enum Msg {
    Save(Duration),
}

/// What the saver thread does with one request. Throttle with a leading edge:
/// the first change is written at once, and while changes keep coming, at least
/// once per `delay` — so a crash or forced shutdown loses at most `delay` of typing.
fn plan_write(deadline: Option<Instant>, last_write: Option<Instant>, delay: Duration, now: Instant) -> Option<Instant> {
    if deadline.is_some() {
        return deadline; // a write is already due soon; don't push it back
    }
    match last_write {
        Some(t) if now.saturating_duration_since(t) < delay => Some(t + delay),
        _ => Some(now), // write right away
    }
}

#[derive(Clone)]
pub struct Saver {
    tx: Sender<Msg>,
    inner: Arc<SaverInner>,
}

struct SaverInner {
    path: PathBuf,
    data: Arc<Mutex<Data>>,
    write_lock: Mutex<String>, // holds the date of the last daily backup
}

impl SaverInner {
    fn write_now(&self) {
        // compact JSON: written up to ~3x a second while typing
        let json = match self.data.lock() {
            Ok(d) => serde_json::to_vec(&*d),
            Err(p) => serde_json::to_vec(&*p.into_inner()),
        };
        let Ok(json) = json else { return };
        let mut last = match self.write_lock.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        let day = today();
        if *last != day && daily_backup(&self.path, &day, BACKUP_KEEP).is_ok() {
            *last = day;
        }
        let _ = write_atomic(&self.path, &json);
    }
}

impl Saver {
    pub fn new(path: PathBuf, data: Arc<Mutex<Data>>) -> Saver {
        let (tx, rx) = mpsc::channel::<Msg>();
        let inner = Arc::new(SaverInner { path, data, write_lock: Mutex::new(String::new()) });
        let worker = inner.clone();
        std::thread::Builder::new()
            .name("gaeunmemo-saver".into())
            .spawn(move || {
                let mut deadline: Option<Instant> = None;
                let mut last_write: Option<Instant> = None;
                loop {
                    let msg = match deadline {
                        Some(d) => rx.recv_timeout(d.saturating_duration_since(Instant::now())),
                        None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
                    };
                    match msg {
                        Ok(Msg::Save(delay)) => {
                            deadline = plan_write(deadline, last_write, delay, Instant::now());
                        }
                        Err(RecvTimeoutError::Timeout) => {
                            deadline = None;
                            worker.write_now();
                            last_write = Some(Instant::now());
                        }
                        Err(RecvTimeoutError::Disconnected) => {
                            if deadline.is_some() {
                                worker.write_now();
                            }
                            break;
                        }
                    }
                }
            })
            .ok();
        Saver { tx, inner }
    }

    /// Throttled save, written on the saver thread (never blocks the caller):
    /// at once, or within `delay` of the previous write (300 ms typing, 1 s geometry).
    pub fn schedule(&self, delay: Duration) {
        let _ = self.tx.send(Msg::Save(delay));
    }

    /// Synchronous write — used on quit and session end.
    pub fn flush(&self) {
        self.inner.write_now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::PALETTE;

    #[test]
    fn parse_full_document() {
        let d = parse(
            r##"{"notes":[{"id":"1","title":"t","text":"b","color":"#9FE1CB","pinned":true,
                 "folded":true,"open":false,"bounds":{"x":10,"y":20,"width":300,"height":200}}],
                 "list":{"open":true,"bounds":{"x":5,"y":6,"width":280,"height":380}},
                 "settings":{"theme":"light"}}"##,
        )
        .unwrap();
        let n = &d.notes[0];
        assert_eq!((n.id.as_str(), n.title.as_str(), n.text.as_str()), ("1", "t", "b"));
        assert!(n.pinned && n.folded && !n.open);
        assert_eq!(n.bounds.unwrap().width, 300.0);
        assert!(d.list.open);
        assert_eq!(d.settings.theme, "light");
    }

    #[test]
    fn missing_fields_get_defaults() {
        let d = parse(r#"{"notes":[{"text":"hello"}]}"#).unwrap();
        let n = &d.notes[0];
        assert_eq!(uuid::Uuid::parse_str(&n.id).map(|_| ()), Ok(()));
        assert_eq!(n.title, "");
        assert_eq!(n.color, PALETTE[0]);
        assert!(n.open && !n.pinned && !n.folded);
        assert!(n.bounds.is_none());
        assert!(!d.list.open);
        assert_eq!(d.settings.theme, "dark");
        assert!(!d.settings.initialized);

        let empty = parse("{}").unwrap();
        assert!(empty.notes.is_empty());
        let bom = parse("\u{feff}{}").unwrap();
        assert!(bom.notes.is_empty());
    }

    #[test]
    fn broken_file_is_backed_up() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        fs::write(&path, b"{ this is not json").unwrap();
        let (d, outcome) = load(&path, "20261113-101010");
        assert!(d.notes.is_empty());
        assert!(d.settings.initialized);
        let LoadOutcome::Broken(backup) = outcome else { panic!("expected Broken") };
        assert_eq!(backup.file_name().unwrap(), "notes-broken-20261113-101010.json");
        assert_eq!(fs::read(&backup).unwrap(), b"{ this is not json");
        // second failure in the same second does not clobber the first backup
        let (_, again) = load(&path, "20261113-101010");
        assert_eq!(again, LoadOutcome::Broken(dir.path().join("notes-broken-20261113-101010-1.json")));
    }

    #[test]
    fn missing_file_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let (_, outcome) = load(&dir.path().join(FILE_NAME), "x");
        assert_eq!(outcome, LoadOutcome::Missing);
    }

    #[test]
    fn atomic_write_replaces_and_cleans_up() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join(FILE_NAME);
        write_atomic(&path, b"one").unwrap();
        write_atomic(&path, b"two").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"two");
        assert!(!path.with_extension("json.tmp").exists());
    }

    #[test]
    fn atomic_write_falls_back_when_rename_fails() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        fs::write(&path, b"old").unwrap();
        write_atomic_with(&path, b"new", |_, _| Err(io::Error::new(io::ErrorKind::PermissionDenied, "locked")))
            .unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new");
        assert!(!path.with_extension("json.tmp").exists());
    }

    #[test]
    fn daily_backup_once_per_day_keeps_seven() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join(FILE_NAME);
        fs::write(&src, b"today").unwrap();
        for d in 1..=8 {
            fs::write(dir.path().join(format!("notes-backup-202611{d:02}.json")), b"old").unwrap();
        }
        daily_backup(&src, "20261113", 7).unwrap();
        let today = dir.path().join("notes-backup-20261113.json");
        assert_eq!(fs::read(&today).unwrap(), b"today");
        // same day again: not overwritten
        fs::write(&src, b"later").unwrap();
        daily_backup(&src, "20261113", 7).unwrap();
        assert_eq!(fs::read(&today).unwrap(), b"today");
        let mut left: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .filter(|n| n.starts_with("notes-backup-"))
            .collect();
        left.sort();
        assert_eq!(left.len(), 7);
        assert_eq!(left[0], "notes-backup-20261103.json");
        assert_eq!(left[6], "notes-backup-20261113.json");
    }

    #[test]
    fn saver_flush_writes_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let data = Arc::new(Mutex::new(Data::default()));
        data.lock().unwrap().notes.push(crate::model::Note { title: "x".into(), ..Default::default() });
        let saver = Saver::new(path.clone(), data);
        saver.flush();
        let back = parse(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(back.notes[0].title, "x");
    }

    #[test]
    fn first_change_is_written_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let saver = Saver::new(path.clone(), Arc::new(Mutex::new(Data::default())));
        saver.schedule(Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(200));
        assert!(path.exists(), "a long delay must not hold back the first write");
    }

    #[test]
    fn steady_typing_is_written_every_delay() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let data = Arc::new(Mutex::new(Data::default()));
        data.lock().unwrap().notes.push(crate::model::Note::default());
        let saver = Saver::new(path.clone(), data.clone());
        let on_disk = || -> usize {
            let d = parse(&fs::read_to_string(&path).unwrap_or_default()).unwrap_or_default();
            d.notes.first().and_then(|n| n.text.parse().ok()).unwrap_or(0)
        };
        // a keystroke every 50 ms for ~1.2 s; the text is the keystroke number
        for i in 1..=24usize {
            data.lock().unwrap().notes[0].text = i.to_string();
            saver.schedule(Duration::from_millis(300));
            std::thread::sleep(Duration::from_millis(50));
            if i == 20 {
                // still typing: the file must be at most ~300 ms (6 keystrokes) + slack behind
                assert!(on_disk() + 9 >= i, "file lags too far behind: {} vs {i}", on_disk());
            }
        }
        std::thread::sleep(Duration::from_millis(450));
        assert_eq!(on_disk(), 24, "last keystroke reaches the file");
    }

    #[test]
    fn plan_write_throttles_with_leading_edge() {
        let now = Instant::now();
        let d = Duration::from_millis(300);
        // nothing written yet → now
        assert_eq!(plan_write(None, None, d, now), Some(now));
        // last write long ago → now
        assert_eq!(plan_write(None, Some(now - Duration::from_secs(2)), d, now), Some(now));
        // wrote 100 ms ago → 300 ms after that write, not later
        let t = now - Duration::from_millis(100);
        assert_eq!(plan_write(None, Some(t), d, now), Some(t + d));
        // already due → keep the earlier deadline (keystrokes never postpone it)
        let due = now + Duration::from_millis(50);
        assert_eq!(plan_write(Some(due), Some(t), d, now), Some(due));
    }
}
