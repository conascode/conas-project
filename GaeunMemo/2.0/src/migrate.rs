//! P5: v1 data → the v2 store, and back (report §16, F-24).
//!
//! * the v1 model is v1.1's `model.rs` (serde, every field defaulted, `sanitize`)
//! * source: v1.1's `%APPDATA%\gaeunmemo\notes.json`, else the old StickyMemo
//!   `%APPDATA%\StickyMemo\notes.json` (format 1 = same document, format 2 = array whose
//!   first text line is the title — v1.1 import.rs). Sources are only ever read.
//! * pictures the imported notes refer to are copied from the source's `images\` into the
//!   v2 folder before the rows (development builds keep their own folder — user decision
//!   2026-11-17; a shared folder comes with P7)
//! * one transaction: rows inserted, read back and compared field by field; any difference
//!   rolls everything back. Existing v2 notes stay; a note whose id is already there is
//!   skipped (user decision 2026-11-17: merge). `meta` records source, SHA-256, counts.
//! * `--mode export-v1`: the v2 store as a v1.1 `notes.json` (rollback, §16.3)

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const PALETTE: [&str; 6] = crate::store::COLORS;
const NOTE_MIN_W: f64 = 180.0;
const NOTE_MIN_H: f64 = 130.0;
const BANDS: [&str; 3] = ["none", "thin", "full"];

// ------------------------------------------------------------------ v1 model (v1.1 model.rs)

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(default)]
pub struct Bounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Default for Bounds {
    fn default() -> Self {
        Bounds { x: 0.0, y: 0.0, width: 260.0, height: 260.0 }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Note {
    pub id: String,
    pub title: String,
    pub text: String,
    pub color: String,
    pub pinned: bool,
    pub folded: bool,
    pub open: bool,
    pub bounds: Option<Bounds>,
    #[serde(rename = "updatedAt")]
    pub updated_at: u64,
    #[serde(rename = "fontSize")]
    pub font_size: u8,
    pub html: String,
    pub locked: bool,
    pub maximized: bool,
}

impl Default for Note {
    fn default() -> Self {
        Note {
            id: crate::win::uuid(),
            title: String::new(),
            text: String::new(),
            color: PALETTE[0].to_string(),
            pinned: false,
            folded: false,
            open: true,
            bounds: None,
            updated_at: 0,
            font_size: 14,
            html: String::new(),
            locked: false,
            maximized: false,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct ListState {
    pub open: bool,
    pub bounds: Option<Bounds>,
}

/// Accepts the current string form and the older numeric form (0/2/4/8 px) — v1.1 `de_band`.
fn de_band<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(match v {
        serde_json::Value::String(s) if BANDS.contains(&s.as_str()) => s,
        serde_json::Value::Number(n) if n.as_u64() == Some(0) => "none".into(),
        _ => "thin".into(),
    })
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub theme: String,
    pub next_color: usize,
    pub initialized: bool,
    #[serde(deserialize_with = "de_band")]
    pub band: String,
    pub autostart: Option<bool>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { theme: "dark".into(), next_color: 0, initialized: false, band: "thin".into(), autostart: None }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct Data {
    pub notes: Vec<Note>,
    pub list: ListState,
    pub settings: Settings,
}

fn fix_bounds(b: &mut Bounds, min_w: f64, min_h: f64) {
    let ok = |v: f64| v.is_finite();
    if !ok(b.x) || b.x.abs() > 100_000.0 {
        b.x = 0.0;
    }
    if !ok(b.y) || b.y.abs() > 100_000.0 {
        b.y = 0.0;
    }
    if !ok(b.width) || b.width < min_w {
        b.width = min_w.max(if ok(b.width) { b.width } else { 0.0 });
    }
    if !ok(b.height) || b.height < min_h {
        b.height = min_h.max(if ok(b.height) { b.height } else { 0.0 });
    }
    b.width = b.width.min(4000.0);
    b.height = b.height.min(4000.0);
}

impl Data {
    /// v1.1 `Data::sanitize`: repairs what a hand-edited or imported file could get wrong.
    pub fn sanitize(&mut self) {
        let mut seen = std::collections::HashSet::new();
        for n in &mut self.notes {
            if n.id.trim().is_empty() || !seen.insert(n.id.clone()) {
                n.id = crate::win::uuid();
                seen.insert(n.id.clone());
            }
            n.color = PALETTE.iter().find(|p| p.eq_ignore_ascii_case(&n.color)).unwrap_or(&PALETTE[0]).to_string();
            if let Some(b) = n.bounds.as_mut() {
                fix_bounds(b, NOTE_MIN_W, NOTE_MIN_H);
            }
            n.font_size = n.font_size.clamp(10, 32);
        }
        if !BANDS.contains(&self.settings.band.as_str()) {
            self.settings.band = "thin".into();
        }
        if let Some(b) = self.list.bounds.as_mut() {
            fix_bounds(b, 200.0, 200.0);
        }
        if self.settings.theme != "light" {
            self.settings.theme = "dark".into();
        }
        self.settings.next_color %= PALETTE.len();
    }
}

/// v1.1 `store::parse`: a notes.json document (a leading BOM is allowed).
pub fn parse_v1(s: &str) -> Result<Data, String> {
    let s = s.trim_start_matches('\u{feff}');
    let mut d: Data = serde_json::from_str(s).map_err(|e| e.to_string())?;
    d.sanitize();
    Ok(d)
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct LegacyNote {
    id: String,
    text: String,
    color: String,
    pinned: bool,
    bounds: Option<Bounds>,
}

/// v1.1 `import::convert` (StickyMemo): format 1 = our document, format 2 = an array whose
/// first text line is the title.
pub fn parse_sticky(s: &str) -> Option<Data> {
    let s = s.trim_start_matches('\u{feff}');
    let v: serde_json::Value = serde_json::from_str(s).ok()?;
    let mut data = if v.is_array() {
        let items: Vec<serde_json::Value> = serde_json::from_value(v).ok()?;
        let notes = items
            .into_iter()
            .filter_map(|it| serde_json::from_value::<LegacyNote>(it).ok())
            .map(|l| {
                let (title, text) = match l.text.split_once('\n') {
                    Some((t, rest)) => (t.trim_end_matches('\r').to_string(), rest.to_string()),
                    None => (l.text.clone(), String::new()),
                };
                Note { id: l.id, title, text, color: l.color, pinned: l.pinned, bounds: l.bounds, ..Default::default() }
            })
            .collect();
        Data { notes, ..Default::default() }
    } else if v.is_object() {
        serde_json::from_value::<Data>(v).ok()?
    } else {
        return None;
    };
    data.sanitize();
    data.settings.next_color = data.notes.len() % PALETTE.len();
    Some(data)
}

// ------------------------------------------------------------------ sources

/// v1.1's data folder (`%APPDATA%\gaeunmemo`) — read only.
pub fn v1_dir() -> PathBuf {
    std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("gaeunmemo")
}

pub fn sticky_path() -> PathBuf {
    std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("StickyMemo").join("notes.json")
}

pub struct Source {
    /// "v1" or "stickymemo"
    pub kind: &'static str,
    pub path: PathBuf,
    pub sha256: String,
    pub data: Data,
    /// where the pictures the notes refer to are
    pub images: PathBuf,
}

/// SHA-256 (hex) with Windows CNG — recorded so the source can be matched later.
pub fn sha256(bytes: &[u8]) -> String {
    use windows_sys::Win32::Security::Cryptography::{BCryptHash, BCRYPT_SHA256_ALG_HANDLE};
    let mut out = [0u8; 32];
    let r = unsafe { BCryptHash(BCRYPT_SHA256_ALG_HANDLE, std::ptr::null(), 0, bytes.as_ptr(), bytes.len() as u32, out.as_mut_ptr(), 32) };
    if r != 0 {
        return String::new();
    }
    out.iter().map(|b| format!("{b:02x}")).collect()
}

/// The data to import: v1.1's notes.json, else StickyMemo's. `Err` = a file is there but
/// does not parse (nothing is imported, nothing is written next to it).
pub fn find_source(v1: &Path, sticky: &Path) -> Result<Option<Source>, String> {
    let main = v1.join("notes.json");
    if let Ok(raw) = std::fs::read(&main) {
        let data = parse_v1(&String::from_utf8_lossy(&raw)).map_err(|e| format!("{}: {e}", main.display()))?;
        return Ok(Some(Source { kind: "v1", path: main, sha256: sha256(&raw), data, images: v1.join("images") }));
    }
    if let Ok(raw) = std::fs::read(sticky) {
        let data = parse_sticky(&String::from_utf8_lossy(&raw)).ok_or_else(|| format!("{}: not a notes file", sticky.display()))?;
        let images = sticky.parent().map(|p| p.join("images")).unwrap_or_default();
        return Ok(Some(Source { kind: "stickymemo", path: sticky.to_path_buf(), sha256: sha256(&raw), data, images }));
    }
    Ok(None)
}

// ------------------------------------------------------------------ v1 note → v2 row

/// What a v1 note becomes in the store: text with LF line ends, HTML through the v1.1
/// allow-list once (a hand-edited file), v1.1's limits (500 / 200,000 chars, 1 MB HTML).
pub struct Planned {
    pub id: String,
    pub title: String,
    pub text: String,
    pub html: String,
    pub color: usize,
    pub note: Note,
    /// the HTML changed through the allow-list
    pub html_normalized: bool,
}

pub fn plan_note(n: &Note) -> Planned {
    let title: String = n.title.replace(['\r', '\n'], " ").chars().take(500).collect();
    let text: String = n.text.replace("\r\n", "\n").replace('\r', "\n").chars().take(200_000).collect();
    let clean = if n.html.is_empty() { String::new() } else { crate::rich::serialize(&crate::rich::sanitize(&n.html)) };
    let html = if clean.len() > 1_000_000 { String::new() } else { clean };
    let html_normalized = html != n.html;
    Planned { id: n.id.clone(), title, text, html, color: PALETTE.iter().position(|c| *c == n.color).unwrap_or(0), note: n.clone(), html_normalized }
}

// ------------------------------------------------------------------ import

#[derive(Debug, Default, PartialEq)]
pub struct Report {
    pub imported: usize,
    pub skipped_existing: usize,
    pub images_copied: usize,
    pub images_missing: usize,
    pub html_normalized: usize,
}

/// True once something was imported into this store (meta `migrated_from`).
pub fn migrated(conn: &Connection) -> bool {
    conn.query_row("SELECT value FROM meta WHERE key='migrated_from'", [], |r| r.get::<_, String>(0)).optional().ok().flatten().is_some()
}

/// Imports `src` into the store `conn` of data folder `dir` (see the module comment).
pub fn import(conn: &mut Connection, dir: &Path, src: &Source) -> Result<Report, String> {
    let mut rep = Report::default();
    let existing: std::collections::HashSet<String> = {
        let mut st = conn.prepare("SELECT id FROM note").map_err(|e| e.to_string())?;
        let it = st.query_map([], |r| r.get::<_, String>(0)).map_err(|e| e.to_string())?;
        it.filter_map(Result::ok).collect()
    };
    let todo: Vec<Planned> = src.data.notes.iter().filter(|n| !existing.contains(&n.id)).map(plan_note).collect();
    rep.skipped_existing = src.data.notes.len() - todo.len();
    rep.html_normalized = todo.iter().filter(|p| p.html_normalized).count();

    // pictures first: files are on disk before any row refers to them
    let to_dir = dir.join("images");
    let _ = std::fs::create_dir_all(&to_dir);
    let mut names: Vec<String> = Vec::new();
    for p in &todo {
        for n in crate::store::image_refs(&p.html) {
            if !names.contains(&n) {
                names.push(n);
            }
        }
    }
    for n in &names {
        if to_dir.join(n).exists() {
            continue;
        }
        match std::fs::read(src.images.join(n)) {
            Ok(bytes) => {
                crate::store::write_image(&to_dir, n, &bytes).map_err(|e| format!("사진 {n}: {e}"))?;
                rep.images_copied += 1;
            }
            Err(_) => rep.images_missing += 1, // kept as a reference: shown as a grey box, like v1.1
        }
    }

    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let base: i64 = tx.query_row("SELECT COALESCE(MAX(seq), -1) FROM note", [], |r| r.get(0)).map_err(|e| e.to_string())?;
    let now = crate::app::now_ms() as i64;
    let total = todo.len() as i64;
    {
        let mut ins = tx
            .prepare("INSERT INTO note (id,title,body_text,body_html,color,pinned,folded,is_open,locked,maximized,x,y,w,h,font_size,seq,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18)")
            .map_err(|e| e.to_string())?;
        let mut img = tx.prepare("INSERT OR IGNORE INTO note_image (note_id, name) VALUES (?1, ?2)").map_err(|e| e.to_string())?;
        for (i, p) in todo.iter().enumerate() {
            let n = &p.note;
            let b = n.bounds;
            ins.execute(params![
                p.id,
                p.title,
                p.text,
                p.html,
                PALETTE[p.color],
                n.pinned as i64,
                n.folded as i64,
                n.open as i64,
                n.locked as i64,
                n.maximized as i64,
                b.map(|b| b.x),
                b.map(|b| b.y),
                b.map(|b| b.width),
                b.map(|b| b.height),
                n.font_size as i64,
                base + 1 + i as i64,
                // v1 keeps no creation time: the array order, oldest first, ending now
                now - (total - i as i64),
                n.updated_at as i64
            ])
            .map_err(|e| format!("{}: {e}", p.id))?;
            for name in crate::store::image_refs(&p.html) {
                img.execute(params![p.id, name]).map_err(|e| e.to_string())?;
            }
        }
        // the list window and settings: only what this store does not have yet (merge)
        if let Some(b) = src.data.list.bounds {
            tx.execute("INSERT OR IGNORE INTO window_state (name,is_open,x,y,w,h) VALUES ('list',?1,?2,?3,?4,?5)", params![src.data.list.open as i64, b.x, b.y, b.width, b.height]).map_err(|e| e.to_string())?;
        } else {
            tx.execute("INSERT OR IGNORE INTO window_state (name,is_open) VALUES ('list',?1)", params![src.data.list.open as i64]).map_err(|e| e.to_string())?;
        }
        let s = &src.data.settings;
        let mut set = tx.prepare("INSERT OR IGNORE INTO setting (key, value) VALUES (?1, ?2)").map_err(|e| e.to_string())?;
        set.execute(params!["theme", s.theme]).map_err(|e| e.to_string())?;
        set.execute(params!["band", s.band]).map_err(|e| e.to_string())?;
        set.execute(params!["next_color", s.next_color.to_string()]).map_err(|e| e.to_string())?;
        if s.initialized {
            set.execute(params!["initialized", "1"]).map_err(|e| e.to_string())?;
        }
        if let Some(a) = s.autostart {
            set.execute(params!["autostart", if a { "1" } else { "0" }]).map_err(|e| e.to_string())?;
        }
    }
    // ④ read back and compare every imported field before committing
    let mut bad = Vec::new();
    {
        let mut st = tx
            .prepare("SELECT title,body_text,body_html,color,pinned,folded,is_open,locked,maximized,x,y,w,h,font_size,updated_at FROM note WHERE id=?1")
            .map_err(|e| e.to_string())?;
        for p in &todo {
            let n = &p.note;
            let ok = st
                .query_row([&p.id], |r| {
                    let rect = match (r.get::<_, Option<f64>>(9)?, r.get::<_, Option<f64>>(10)?, r.get::<_, Option<f64>>(11)?, r.get::<_, Option<f64>>(12)?) {
                        (Some(x), Some(y), Some(w), Some(h)) => Some(Bounds { x, y, width: w, height: h }),
                        _ => None,
                    };
                    Ok(r.get::<_, String>(0)? == p.title
                        && r.get::<_, String>(1)? == p.text
                        && r.get::<_, String>(2)? == p.html
                        && r.get::<_, String>(3)? == PALETTE[p.color]
                        && (r.get::<_, i64>(4)? != 0) == n.pinned
                        && (r.get::<_, i64>(5)? != 0) == n.folded
                        && (r.get::<_, i64>(6)? != 0) == n.open
                        && (r.get::<_, i64>(7)? != 0) == n.locked
                        && (r.get::<_, i64>(8)? != 0) == n.maximized
                        && rect == n.bounds
                        && r.get::<_, i64>(13)? == n.font_size as i64
                        && r.get::<_, i64>(14)? == n.updated_at as i64)
                })
                .unwrap_or(false);
            if !ok {
                bad.push(p.id.clone());
            }
        }
    }
    if !bad.is_empty() {
        // dropping `tx` rolls back: nothing of this import stays
        return Err(format!("가져온 메모 {}개가 원본과 달라 가져오기를 취소했어요 ({})", bad.len(), bad.iter().take(3).cloned().collect::<Vec<_>>().join(", ")));
    }
    rep.imported = todo.len();
    {
        let mut meta = tx.prepare("INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)").map_err(|e| e.to_string())?;
        meta.execute(params!["migrated_from", format!("{} {}", src.kind, src.path.display())]).map_err(|e| e.to_string())?;
        meta.execute(params!["v1_sha256", src.sha256]).map_err(|e| e.to_string())?;
        meta.execute(params!["migrated_at", now.to_string()]).map_err(|e| e.to_string())?;
        meta.execute(params!["migrated_notes", rep.imported.to_string()]).map_err(|e| e.to_string())?;
        meta.execute(params!["migrated_skipped", rep.skipped_existing.to_string()]).map_err(|e| e.to_string())?;
        meta.execute(params!["migrated_images_missing", rep.images_missing.to_string()]).map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    let ok: String = conn.query_row("PRAGMA integrity_check", [], |r| r.get(0)).unwrap_or_default();
    if ok != "ok" {
        return Err(format!("가져온 뒤 무결성 검사 실패: {ok}"));
    }
    Ok(rep)
}

/// The start-up step (store::open_with): imports once — or again with `force`
/// (`--reimport-v1`: notes added in v1.1 since). Returns a line for the log.
pub fn on_open(conn: &mut Connection, dir: &Path, v1: &Path, sticky: &Path, force: bool) -> Option<String> {
    if !force && migrated(conn) {
        return None;
    }
    match find_source(v1, sticky) {
        Ok(None) => None,
        Err(e) => Some(format!("import: source does not parse, nothing imported ({e})")),
        Ok(Some(src)) => match import(conn, dir, &src) {
            Ok(r) => Some(format!(
                "import: {} {} → {} notes (skipped {} already here), pictures copied {}, missing {}, html normalised {}, sha256 {}",
                src.kind,
                src.path.display(),
                r.imported,
                r.skipped_existing,
                r.images_copied,
                r.images_missing,
                r.html_normalized,
                src.sha256
            )),
            Err(e) => Some(format!("import failed, nothing changed: {e}")),
        },
    }
}

// ------------------------------------------------------------------ export (rollback)

/// The store in `dir` as a v1.1 document: what `--mode export-v1` writes.
pub fn export(dir: &Path) -> Result<Data, String> {
    let conn = Connection::open_with_flags(dir.join("gaeunmemo.db"), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|e| e.to_string())?;
    let mut notes = Vec::new();
    {
        let mut st = conn
            .prepare("SELECT id,title,body_text,body_html,color,pinned,folded,is_open,locked,maximized,x,y,w,h,font_size,updated_at FROM note WHERE deleted_at IS NULL ORDER BY seq")
            .map_err(|e| e.to_string())?;
        let it = st
            .query_map([], |r| {
                let bounds = match (r.get::<_, Option<f64>>(10)?, r.get::<_, Option<f64>>(11)?, r.get::<_, Option<f64>>(12)?, r.get::<_, Option<f64>>(13)?) {
                    (Some(x), Some(y), Some(w), Some(h)) => Some(Bounds { x, y, width: w, height: h }),
                    _ => None,
                };
                Ok(Note {
                    id: r.get(0)?,
                    title: r.get(1)?,
                    text: r.get(2)?,
                    html: r.get(3)?,
                    color: r.get(4)?,
                    pinned: r.get::<_, i64>(5)? != 0,
                    folded: r.get::<_, i64>(6)? != 0,
                    open: r.get::<_, i64>(7)? != 0,
                    locked: r.get::<_, i64>(8)? != 0,
                    maximized: r.get::<_, i64>(9)? != 0,
                    bounds,
                    font_size: r.get::<_, i64>(14)?.clamp(10, 32) as u8,
                    updated_at: r.get::<_, i64>(15)?.max(0) as u64,
                })
            })
            .map_err(|e| e.to_string())?;
        for n in it {
            notes.push(n.map_err(|e| e.to_string())?);
        }
    }
    let get = |k: &str| conn.query_row("SELECT value FROM setting WHERE key=?1", [k], |r| r.get::<_, String>(0)).optional().ok().flatten();
    let settings = Settings {
        theme: get("theme").filter(|t| t == "light").unwrap_or_else(|| "dark".into()),
        next_color: get("next_color").and_then(|v| v.parse().ok()).unwrap_or(0) % PALETTE.len(),
        initialized: get("initialized").as_deref() == Some("1"),
        band: get("band").filter(|b| BANDS.contains(&b.as_str())).unwrap_or_else(|| "thin".into()),
        autostart: get("autostart").map(|v| v == "1"),
    };
    let list = conn
        .query_row("SELECT is_open,x,y,w,h FROM window_state WHERE name='list'", [], |r| {
            let bounds = match (r.get::<_, Option<f64>>(1)?, r.get::<_, Option<f64>>(2)?, r.get::<_, Option<f64>>(3)?, r.get::<_, Option<f64>>(4)?) {
                (Some(x), Some(y), Some(w), Some(h)) => Some(Bounds { x, y, width: w, height: h }),
                _ => None,
            };
            Ok(ListState { open: r.get::<_, i64>(0)? != 0, bounds })
        })
        .optional()
        .map_err(|e| e.to_string())?
        .unwrap_or_default();
    Ok(Data { notes, list, settings })
}

/// `--mode export-v1 --data DIR --out FILE`: writes FILE (never v1.1's own notes.json unless
/// that very path is given) the way v1.1 writes it: compact JSON, temp file then rename.
pub fn export_file(dir: &Path, out: &Path) -> Result<usize, String> {
    let d = export(dir)?;
    let bytes = serde_json::to_vec(&d).map_err(|e| e.to_string())?;
    let tmp = out.with_extension("json.tmp");
    std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, out).map_err(|e| e.to_string())?;
    Ok(d.notes.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v11_documents_load_with_defaults_and_repairs() {
        let d = parse_v1(r##"{"notes":[{"id":"a","color":"red","bounds":{"x":1,"y":2,"width":10,"height":5}},{"id":"a","color":"#afa9ec","fontSize":99},{"id":"b","text":"t"}],"settings":{"theme":"purple","nextColor":99,"band":"wide"}}"##).unwrap();
        assert_eq!(d.notes[0].color, PALETTE[0]);
        assert_eq!(d.notes[1].color, "#AFA9EC");
        assert_ne!(d.notes[0].id, d.notes[1].id);
        assert_eq!(d.notes[0].bounds.unwrap().width, NOTE_MIN_W);
        assert_eq!(d.notes[1].font_size, 32);
        assert_eq!((d.notes[2].html.as_str(), d.notes[2].locked, d.notes[2].maximized), ("", false, false));
        assert_eq!(d.settings.theme, "dark");
        assert_eq!(d.settings.band, "thin");
        assert!(d.settings.next_color < 6);
    }

    #[test]
    fn numeric_band_and_bom() {
        let band = |s: &str| parse_v1(s).unwrap().settings.band;
        assert_eq!(band("\u{feff}{\"settings\":{\"band\":0}}"), "none");
        assert_eq!(band(r#"{"settings":{"band":8}}"#), "thin");
        assert_eq!(band(r#"{"settings":{"band":"full"}}"#), "full");
    }

    #[test]
    fn sticky_format2_first_line_is_the_title() {
        let d = parse_sticky(r##"[{"id":"a","text":"장보기\r\n우유\n계란","color":"#F4C0D1","pinned":true},{"id":"b","text":"한 줄"},{"text":""}]"##).unwrap();
        assert_eq!((d.notes[0].title.as_str(), d.notes[0].text.as_str()), ("장보기", "우유\n계란"));
        assert!(d.notes[0].pinned && d.notes[0].open);
        assert_eq!((d.notes[1].title.as_str(), d.notes[1].text.as_str()), ("한 줄", ""));
        assert!(!d.notes[2].id.is_empty());
        assert!(parse_sticky("nope").is_none() && parse_sticky("42").is_none());
    }

    #[test]
    fn planned_rows_follow_v11_limits() {
        let n = Note { title: "a\r\nb".into(), text: "x\r\ny\rz".into(), html: "<b>굵게</b><script>x()</script>".into(), ..Default::default() };
        let p = plan_note(&n);
        assert_eq!(p.title, "a  b");
        assert_eq!(p.text, "x\ny\nz");
        assert_eq!(p.html, "<b>굵게</b>");
        assert!(p.html_normalized);
        let same = plan_note(&Note { html: "<b>굵게</b>".into(), ..Default::default() });
        assert!(!same.html_normalized);
    }

    #[test]
    fn sha256_of_abc() {
        assert_eq!(sha256(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
}
