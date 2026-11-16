//! App state (UI thread only) and the commands every window calls.
//! P3: every change is also sent to the SQLite store (crate::store) when one is open; test and
//! measurement modes run without a store (in memory, like P1/P2).
//! Rule: never hold the RefCell borrow across a Win32 call that can re-enter a WndProc.

use crate::store::{self, Cmd, Row};
use crate::theme::{Band, Theme, PALETTE};
use crate::{list, note};
use std::cell::RefCell;
use std::collections::HashMap;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

#[derive(Clone, Debug)]
pub struct Note {
    /// runtime id (window maps); `uuid` is the stored id (v1 keeps UUIDs)
    pub id: u64,
    pub uuid: String,
    pub title: String,
    pub text: String,
    /// v1.1 formatted body (allow-listed HTML); empty for plain-text (v1.0) notes
    pub html: String,
    pub color: usize,
    pub pinned: bool,
    pub folded: bool,
    pub open: bool,
    pub locked: bool,
    /// 전체화면 (maximized); `rect` keeps the normal size
    pub maximized: bool,
    /// logical px (96 dpi): x, y, w, h of the normal (unfolded, unmaximized) window
    pub rect: Option<(f64, f64, f64, f64)>,
    pub font_size: u8,
    pub created_at: u64,
    pub updated_at: u64,
    /// creation order (ties in the list sort: newest created first)
    pub seq: u64,
}

impl Note {
    pub fn is_empty(&self) -> bool {
        // v1.1 model.rs: no title, no text and no picture
        self.title.trim().is_empty() && self.text.trim().is_empty() && !self.html.contains("data-gm=")
    }

    fn row(&self) -> Row {
        Row {
            id: self.uuid.clone(),
            title: self.title.clone(),
            body_text: self.text.clone(),
            body_html: self.html.clone(),
            color: self.color,
            pinned: self.pinned,
            folded: self.folded,
            is_open: self.open,
            locked: self.locked,
            maximized: self.maximized,
            rect: self.rect,
            font_size: self.font_size,
            seq: self.seq as i64,
            created_at: self.created_at as i64,
            updated_at: self.updated_at as i64,
        }
    }
}

pub struct App {
    pub notes: Vec<Note>,
    pub theme: Theme,
    pub band: Band,
    pub next_color: usize,
    next_id: u64,
    next_seq: u64,
    pub windows: HashMap<u64, HWND>,
}

thread_local! {
    static APP: RefCell<App> = RefCell::new(App {
        notes: Vec::new(),
        theme: Theme::Dark,
        band: Band::Thin,
        next_color: 0,
        next_id: 1,
        next_seq: 1,
        windows: HashMap::new(),
    });
}

pub fn with<R>(f: impl FnOnce(&mut App) -> R) -> R {
    APP.with(|a| f(&mut a.borrow_mut()))
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

pub fn theme() -> Theme {
    with(|a| a.theme)
}
pub fn band() -> Band {
    with(|a| a.band)
}

pub fn note(id: u64) -> Option<Note> {
    with(|a| a.notes.iter().find(|n| n.id == id).cloned())
}

/// Sends the note's row to the store (no-op without a store).
fn persist(id: u64) {
    if store::active() {
        if let Some(n) = note(id) {
            store::send(Cmd::Upsert(n.row()));
        }
    }
}

fn persist_settings() {
    if store::active() {
        let (t, b, c) = with(|a| (a.theme, a.band, a.next_color));
        store::send(Cmd::Setting("theme".into(), if t == Theme::Light { "light" } else { "dark" }.into()));
        store::send(Cmd::Setting("band".into(), match b {
            Band::None => "none",
            Band::Thin => "thin",
            Band::Full => "full",
        }
        .into()));
        store::send(Cmd::Setting("next_color".into(), c.to_string()));
    }
}

/// Changes a note in place (and stores it); returns false if it no longer exists.
pub fn update(id: u64, f: impl FnOnce(&mut Note)) -> bool {
    let ok = with(|a| a.notes.iter_mut().find(|n| n.id == id).map(f).is_some());
    if ok {
        persist(id);
    }
    ok
}

pub fn add(title: &str, text: &str, color: Option<usize>, age_ms: u64) -> u64 {
    let picked = color.is_none();
    let id = with(|a| {
        let id = a.next_id;
        a.next_id += 1;
        let seq = a.next_seq;
        a.next_seq += 1;
        let color = color.unwrap_or_else(|| {
            let c = a.next_color;
            a.next_color = (a.next_color + 1) % PALETTE.len();
            c
        });
        let now = now_ms();
        a.notes.push(Note {
            id,
            uuid: crate::win::uuid(),
            title: title.into(),
            text: text.into(),
            html: String::new(),
            color,
            pinned: false,
            folded: false,
            open: true,
            locked: false,
            maximized: false,
            rect: None,
            font_size: crate::theme::g::FONT_DEFAULT,
            created_at: now,
            updated_at: now.saturating_sub(age_ms),
            seq,
        });
        id
    });
    persist(id);
    if picked {
        persist_settings();
    }
    id
}

/// The notes, settings and window states the store loaded (start).
pub fn load(l: store::Loaded) {
    with(|a| {
        a.notes.clear();
        for r in l.notes {
            let id = a.next_id;
            a.next_id += 1;
            a.next_seq = a.next_seq.max(r.seq.max(0) as u64 + 1);
            a.notes.push(Note {
                id,
                uuid: r.id,
                title: r.title,
                text: r.body_text,
                html: r.body_html,
                color: r.color.min(PALETTE.len() - 1),
                pinned: r.pinned,
                folded: r.folded,
                open: r.is_open,
                locked: r.locked,
                maximized: r.maximized,
                rect: r.rect,
                font_size: r.font_size,
                created_at: r.created_at.max(0) as u64,
                updated_at: r.updated_at.max(0) as u64,
                seq: r.seq.max(0) as u64,
            });
        }
        a.theme = if l.settings.get("theme").map(String::as_str) == Some("light") { Theme::Light } else { Theme::Dark };
        a.band = match l.settings.get("band").map(String::as_str) {
            Some("none") => Band::None,
            Some("full") => Band::Full,
            _ => Band::Thin,
        };
        a.next_color = l.settings.get("next_color").and_then(|v| v.parse().ok()).unwrap_or(0) % PALETTE.len();
    });
}

pub fn window_of(id: u64) -> Option<HWND> {
    with(|a| a.windows.get(&id).copied())
}

pub fn open_windows() -> Vec<HWND> {
    with(|a| a.windows.values().copied().collect())
}

/// Shows a note (creating its window if needed) and optionally focuses it.
pub fn open_note(id: u64, at: Option<(i32, i32)>, focus: bool) {
    let focus = focus && !crate::quiet();
    if let Some(h) = window_of(id) {
        unsafe {
            ShowWindow(h, if IsIconic(h) != 0 { SW_RESTORE } else { SW_SHOWNA });
            if focus {
                SetForegroundWindow(h);
            }
        }
        return;
    }
    if !note(id).is_some_and(|n| n.open) {
        update(id, |n| n.open = true);
    }
    note::open(id, at, focus);
    list::refresh();
}

/// "+" / Ctrl+N: a new note 28 px right-down of `near`, or cascaded on the primary monitor.
pub fn new_note(near: Option<HWND>) -> u64 {
    let id = add("", "", None, 0);
    let at = near.and_then(|h| unsafe {
        let mut r = windows_sys::Win32::Foundation::RECT { left: 0, top: 0, right: 0, bottom: 0 };
        (GetWindowRect(h, &mut r) != 0).then(|| {
            let off = crate::win::px(28.0, windows_sys::Win32::UI::HiDpi::GetDpiForWindow(h));
            (r.left + off, r.top + off)
        })
    });
    open_note(id, at, true);
    id
}

/// × / Ctrl+W: closes the window; an empty note is removed (F-10).
pub fn close_note(id: u64) {
    let h = with(|a| a.windows.get(&id).copied());
    let Some(n) = note(id) else { return };
    if n.is_empty() {
        with(|a| a.notes.retain(|x| x.id != id));
        if store::active() {
            store::send(Cmd::Delete(n.uuid));
        }
    } else {
        update(id, |n| n.open = false);
    }
    if let Some(h) = h {
        unsafe { DestroyWindow(h) };
    }
    list::refresh();
}

/// Deletes after the caller has confirmed (or the note was empty). Locked notes refuse.
pub fn delete_note(id: u64) -> bool {
    let Some(n) = note(id) else { return false };
    if n.locked {
        return false;
    }
    let h = with(|a| {
        a.notes.retain(|x| x.id != id);
        a.windows.get(&id).copied()
    });
    if store::active() {
        store::send(Cmd::Delete(n.uuid));
    }
    if let Some(h) = h {
        unsafe { DestroyWindow(h) };
    }
    list::refresh();
    true
}

/// The normal window rectangle of a note, in logical px (moves and resizes, F-27).
pub fn set_rect(id: u64, rect: (f64, f64, f64, f64)) {
    if note(id).is_some_and(|n| n.rect != Some(rect)) {
        update(id, |n| n.rect = Some(rect));
    }
}

pub fn set_theme(t: Theme) {
    with(|a| a.theme = t);
    persist_settings();
    for h in open_windows() {
        note::apply_theme(h);
    }
    list::apply_theme();
}

pub fn cycle_band() -> Band {
    let b = with(|a| {
        a.band = a.band.next();
        a.band
    });
    persist_settings();
    for h in open_windows() {
        note::apply_band(h);
    }
    list::apply_theme();
    b
}

pub fn touch(id: u64) {
    update(id, |n| n.updated_at = now_ms());
    list::refresh_one(id);
}

/// Sample notes for runs without a store (tests, measurement).
pub fn load_demo() {
    let min = 60_000;
    add("장보기", "우유 2개\n계란 한 판\n사과 1 kg — 토요일 시장에서", Some(0), 3 * min);
    add("회의 메모", "다음 주 화요일 오후 3시\n3층 회의실, 노트북 지참", Some(4), 2 * 3600_000);
    add("", "제목 없는 메모도 목록에 보여요", Some(2), 26 * 3600_000);
    let p2 = add("GaeunMemo 2.0", "", Some(3), 30_000);
    let html = "P2: <b>굵게</b>, <i>기울임</i>, <u>밑줄</u>, <s>취소선</s><ul><li>글머리 기호</li><li>붙여넣기는 지원 서식만 남겨요</li></ul>사진은 전체화면의 사진 넣기나 붙여넣기로";
    let doc = crate::rich::Doc::load(html, "");
    update(p2, |n| {
        n.html = doc.to_html();
        n.text = doc.plain();
    });
    let closed = add("지난주 할 일", "보고서 정리\n사진 백업", Some(1), 5 * 86_400_000);
    update(closed, |n| n.open = false);
    with(|a| a.next_color = 5);
}
