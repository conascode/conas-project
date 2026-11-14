//! Window management: note windows, the list window, show/hide all.
//!
//! Rule: never call a window method while holding the data lock — window
//! calls from worker threads wait on the main thread, which may be waiting
//! on the lock inside an event handler.

use crate::model::*;
use crate::AppState;
use std::time::Duration;
use tauri::window::Color;
use tauri::{AppHandle, Emitter, LogicalSize, Manager, Size, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};

pub const LIST: &str = "list";
const TYPE_DELAY: Duration = Duration::from_millis(300);
const GEOM_DELAY: Duration = Duration::from_secs(1);

/// Same for every window (WebView2 requires identical args per environment).
/// All windows share one origin, so one renderer process can serve them all
/// instead of one renderer per window.
const BROWSER_ARGS: &str = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection \
--process-per-site --renderer-process-limit=1 \
--disable-background-networking --disable-component-update --no-pings \
--disable-gpu --disable-gpu-compositing";

pub fn note_label(id: &str) -> String {
    format!("note-{id}")
}

pub fn bg(theme: &str) -> Color {
    if theme == "light" {
        Color(0xFF, 0xFF, 0xFF, 0xFF)
    } else {
        Color(0x20, 0x20, 0x20, 0xFF)
    }
}

fn state(app: &AppHandle) -> tauri::State<'_, AppState> {
    app.state::<AppState>()
}

// ---------- geometry ----------

type Rect = (f64, f64, f64, f64);

fn logical(m: &tauri::Monitor) -> Rect {
    let s = m.scale_factor().max(0.5);
    let p = m.position();
    let z = m.size();
    (p.x as f64 / s, p.y as f64 / s, z.width as f64 / s, z.height as f64 / s)
}

fn primary(app: &AppHandle) -> Rect {
    app.primary_monitor().ok().flatten().map(|m| logical(&m)).unwrap_or((0.0, 0.0, 1920.0, 1080.0))
}

/// Keeps a window reachable: if its title bar is on no monitor, centre it on the primary one.
pub fn fit_rect(b: Bounds, monitors: &[Rect], primary: Rect) -> Bounds {
    let (gx, gy) = (b.x + b.width.min(80.0) / 2.0, b.y + 12.0);
    let visible = monitors
        .iter()
        .any(|&(x, y, w, h)| gx >= x && gx < x + w - 20.0 && gy >= y && gy < y + h - 20.0);
    if visible {
        return b;
    }
    let (px, py, pw, ph) = primary;
    let width = b.width.min(pw - 40.0).max(NOTE_MIN_W);
    let height = b.height.min(ph - 80.0).max(BAR_H);
    Bounds { x: px + (pw - width) / 2.0, y: py + (ph - height) / 2.0, width, height }
}

fn fit(app: &AppHandle, b: Bounds) -> Bounds {
    let mons: Vec<Rect> = app.available_monitors().unwrap_or_default().iter().map(logical).collect();
    fit_rect(b, &mons, primary(app))
}

fn centered(app: &AppHandle, w: f64, h: f64, step: usize) -> Bounds {
    let (px, py, pw, ph) = primary(app);
    let off = (step % 8) as f64 * 28.0;
    Bounds { x: px + (pw - w) / 2.0 + off, y: py + (ph - h) / 2.0 + off, width: w, height: h }
}

fn logical_pos(w: &WebviewWindow) -> Option<(f64, f64)> {
    let s = w.scale_factor().ok()?;
    let p = w.outer_position().ok()?.to_logical::<f64>(s);
    Some((p.x, p.y))
}

// ---------- list events ----------

pub fn emit_upsert(app: &AppHandle, id: &str) {
    if app.get_webview_window(LIST).is_none() {
        return;
    }
    let s = state(app).with(|d| d.find(id).map(Summary::from));
    if let Some(s) = s {
        let _ = app.emit_to(LIST, "list-upsert", s);
    }
}

pub fn emit_remove(app: &AppHandle, id: &str) {
    if app.get_webview_window(LIST).is_some() {
        let _ = app.emit_to(LIST, "list-remove", id.to_string());
    }
}

// ---------- notes ----------

/// The top edge drags the window (band and title bar) instead of resizing it.
fn free_top_edge(w: &WebviewWindow) {
    #[cfg(windows)]
    if let Ok(h) = w.hwnd() {
        crate::anchor::free_top_edge(h.0 as isize);
    }
    #[cfg(not(windows))]
    let _ = w;
}

/// Note windows have their own taskbar entries (Windows groups them under one
/// icon and shows one preview per note); the preview caption is the note title.
fn taskbar_title(title: &str) -> String {
    let t = title.trim();
    if t.is_empty() { "제목 없음".into() } else { t.chars().take(80).collect() }
}

pub fn set_note_title(app: &AppHandle, id: &str, title: &str) {
    if let Some(w) = app.get_webview_window(&note_label(id)) {
        let _ = w.set_title(&taskbar_title(title));
    }
}

fn reveal(w: &WebviewWindow, focus: bool) {
    let _ = w.show();
    let _ = w.unminimize();
    if focus {
        let _ = w.set_focus();
    }
}

pub fn open_note(app: &AppHandle, id: &str, focus: bool) {
    let st = state(app);
    if let Some(w) = app.get_webview_window(&note_label(id)) {
        reveal(&w, focus);
        return;
    }
    let step = app.webview_windows().len();
    let snap = st.with(|d| {
        let theme = d.settings.theme.clone();
        let band = d.settings.band.clone();
        d.find_mut(id).map(|n| {
            n.open = true;
            (n.clone(), theme, band)
        })
    });
    let Some((note, theme, band)) = snap else { return };
    let b = fit(app, note.bounds.unwrap_or_else(|| centered(app, NOTE_W, NOTE_H, step)));
    st.with(|d| {
        if let Some(n) = d.find_mut(id) {
            n.bounds = Some(b);
        }
    });

    let url = WebviewUrl::App(format!("note.html?t={theme}&b={band}").into());
    let mut builder = WebviewWindowBuilder::new(app, note_label(id), url)
        .title(taskbar_title(&note.title))
        .decorations(false)
        .shadow(false)
        .skip_taskbar(false)
        .resizable(true)
        .maximizable(false)
        .minimizable(true)
        .always_on_top(note.pinned)
        .focused(focus)
        .background_color(bg(&theme))
        .additional_browser_args(BROWSER_ARGS)
        .position(b.x, b.y);
    builder = if note.folded {
        builder.inner_size(b.width, folded_h(&band)).max_inner_size(4000.0, folded_h(&band))
    } else {
        builder.inner_size(b.width, b.height).min_inner_size(NOTE_MIN_W, NOTE_MIN_H)
    };
    match builder.build() {
        Ok(w) => {
            free_top_edge(&w);
            if note.folded {
                // re-apply now that Windows' minimum height is lowered (34px strip)
                size_to_strip(&w, b.width, folded_h(&band));
            }
            let app2 = app.clone();
            let id2 = id.to_string();
            w.on_window_event(move |e| on_note_event(&app2, &id2, e));
        }
        Err(e) => crate::log!("note window failed ({id}): {e}"),
    }
    st.saver.schedule(TYPE_DELAY);
    emit_upsert(app, id);
}

fn on_note_event(app: &AppHandle, id: &str, e: &WindowEvent) {
    let st = state(app);
    match e {
        WindowEvent::Moved(p) => {
            if p.x <= -30000 || p.y <= -30000 {
                return;
            }
            let Some(w) = app.get_webview_window(&note_label(id)) else { return };
            let s = w.scale_factor().unwrap_or(1.0);
            let lp = p.to_logical::<f64>(s);
            let hit = st.with(|d| {
                d.find_mut(id).map(|n| {
                    let b = n.bounds.get_or_insert_with(Bounds::default);
                    b.x = lp.x;
                    b.y = lp.y;
                })
            });
            if hit.is_some() {
                st.saver.schedule(GEOM_DELAY);
            }
        }
        WindowEvent::Resized(sz) => {
            if sz.width == 0 || sz.height == 0 {
                return;
            }
            let Some(w) = app.get_webview_window(&note_label(id)) else { return };
            let s = w.scale_factor().unwrap_or(1.0);
            let ls = sz.to_logical::<f64>(s);
            let hit = st.with(|d| {
                d.find_mut(id).map(|n| {
                    let folded = n.folded;
                    let b = n.bounds.get_or_insert_with(Bounds::default);
                    b.width = ls.width.max(NOTE_MIN_W);
                    if !folded && ls.height >= NOTE_MIN_H - 1.0 {
                        b.height = ls.height;
                    }
                })
            });
            if hit.is_some() {
                st.saver.schedule(GEOM_DELAY);
            }
        }
        WindowEvent::CloseRequested { api, .. } => {
            api.prevent_close();
            close_note(app, id);
        }
        _ => {}
    }
}

pub fn new_note(app: &AppHandle, pos: Option<(f64, f64)>, focus: bool) -> String {
    let step = state(app).with(|d| d.notes.iter().filter(|n| n.open).count());
    let b = match pos {
        Some((x, y)) => Bounds { x, y, width: NOTE_W, height: NOTE_H },
        None => centered(app, NOTE_W, NOTE_H, step),
    };
    let id = state(app).with(|d| {
        let color = d.take_color();
        let n = Note { color, bounds: Some(b), updated_at: now_ms(), ..Default::default() };
        let id = n.id.clone();
        d.notes.push(n);
        id
    });
    open_note(app, &id, focus);
    id
}

pub fn new_note_near(app: &AppHandle, id: &str) {
    let pos = app
        .get_webview_window(&note_label(id))
        .and_then(|w| logical_pos(&w))
        .or_else(|| state(app).with(|d| d.find(id).and_then(|n| n.bounds).map(|b| (b.x, b.y))));
    new_note(app, pos.map(|(x, y)| (x + 28.0, y + 28.0)), true);
}

/// Closes the window; the note stays in the list unless it is empty.
pub fn close_note(app: &AppHandle, id: &str) {
    let st = state(app);
    let removed = st.with(|d| {
        let i = d.notes.iter().position(|n| n.id == id)?;
        if d.notes[i].is_empty() {
            d.notes.remove(i);
            Some(true)
        } else {
            d.notes[i].open = false;
            Some(false)
        }
    });
    if let Some(w) = app.get_webview_window(&note_label(id)) {
        let _ = w.destroy();
    }
    match removed {
        Some(true) => emit_remove(app, id),
        Some(false) => emit_upsert(app, id),
        None => return,
    }
    st.saver.schedule(TYPE_DELAY);
}

fn logical_width(w: &WebviewWindow, fallback: f64) -> f64 {
    let s = w.scale_factor().unwrap_or(1.0);
    w.inner_size().map(|z| z.to_logical::<f64>(s).width).unwrap_or(fallback)
}

/// Sizes a window to a single strip of `h` (folded note, or the deleted-note strip).
fn size_to_strip(w: &WebviewWindow, width: f64, h: f64) {
    let _ = w.set_min_size(None::<Size>);
    let _ = w.set_max_size(Some(LogicalSize::new(4000.0, h)));
    let _ = w.set_size(LogicalSize::new(width, h));
}

fn size_to_full(w: &WebviewWindow, width: f64, h: f64) {
    let _ = w.set_max_size(None::<Size>);
    let _ = w.set_size(LogicalSize::new(width, h.max(NOTE_MIN_H)));
    let _ = w.set_min_size(Some(LogicalSize::new(NOTE_MIN_W, NOTE_MIN_H)));
}

pub fn set_fold(app: &AppHandle, id: &str) -> bool {
    let st = state(app);
    let snap = st.with(|d| {
        let band = d.settings.band.clone();
        d.find_mut(id).map(|n| {
            n.folded = !n.folded;
            (n.folded, n.bounds.unwrap_or_default(), band)
        })
    });
    let Some((folded, b, band)) = snap else { return false };
    if let Some(w) = app.get_webview_window(&note_label(id)) {
        let width = logical_width(&w, b.width);
        if folded {
            size_to_strip(&w, width, folded_h(&band));
        } else {
            size_to_full(&w, width, b.height);
        }
    }
    st.saver.schedule(TYPE_DELAY);
    folded
}

/// Band mode (⋯ menu): applies to every window at once.
pub fn apply_band(app: &AppHandle, band: &str) {
    if !BANDS.contains(&band) {
        return;
    }
    let st = state(app);
    let folded: Vec<String> = st.with(|d| {
        d.settings.band = band.to_string();
        d.notes.iter().filter(|n| n.folded).map(|n| n.id.clone()).collect()
    });
    st.saver.schedule(TYPE_DELAY);
    for id in folded {
        if let Some(w) = app.get_webview_window(&note_label(&id)) {
            let width = logical_width(&w, NOTE_W);
            size_to_strip(&w, width, folded_h(band));
        }
    }
    let _ = app.emit("band", band);
}

pub fn set_pin(app: &AppHandle, id: &str) -> bool {
    let st = state(app);
    let Some(pinned) = st.with(|d| {
        d.find_mut(id).map(|n| {
            n.pinned = !n.pinned;
            n.pinned
        })
    }) else {
        return false;
    };
    if let Some(w) = app.get_webview_window(&note_label(id)) {
        let _ = w.set_always_on_top(pinned);
    }
    st.saver.schedule(TYPE_DELAY);
    pinned
}

// ---------- list window ----------

pub fn open_list(app: &AppHandle, focus: bool) {
    let st = state(app);
    if let Some(w) = app.get_webview_window(LIST) {
        reveal(&w, focus);
        return;
    }
    let (saved, theme, band) = st.with(|d| {
        d.list.open = true;
        (d.list.bounds, d.settings.theme.clone(), d.settings.band.clone())
    });
    let b = fit(
        app,
        saved.unwrap_or_else(|| {
            let (px, py, pw, _) = primary(app);
            Bounds { x: px + pw - LIST_W - 24.0, y: py + 72.0, width: LIST_W, height: LIST_H }
        }),
    );
    st.with(|d| d.list.bounds = Some(b));
    let url = WebviewUrl::App(format!("list.html?t={theme}&b={band}").into());
    let built = WebviewWindowBuilder::new(app, LIST, url)
        .title("GaeunMemo")
        .decorations(false)
        .shadow(false)
        .skip_taskbar(true)
        .maximizable(false)
        .minimizable(false)
        .focused(focus)
        .background_color(bg(&theme))
        .additional_browser_args(BROWSER_ARGS)
        .inner_size(b.width, b.height)
        .min_inner_size(220.0, 240.0)
        .position(b.x, b.y)
        .build();
    match built {
        Ok(w) => {
            free_top_edge(&w);
            let app2 = app.clone();
            w.on_window_event(move |e| on_list_event(&app2, e));
        }
        Err(e) => crate::log!("list window failed: {e}"),
    }
    st.saver.schedule(TYPE_DELAY);
}

fn on_list_event(app: &AppHandle, e: &WindowEvent) {
    let st = state(app);
    let scale = || app.get_webview_window(LIST).and_then(|w| w.scale_factor().ok()).unwrap_or(1.0);
    match e {
        WindowEvent::Moved(p) if p.x > -30000 && p.y > -30000 => {
            let lp = p.to_logical::<f64>(scale());
            st.with(|d| {
                let b = d.list.bounds.get_or_insert(Bounds { x: 0.0, y: 0.0, width: LIST_W, height: LIST_H });
                b.x = lp.x;
                b.y = lp.y;
            });
            st.saver.schedule(GEOM_DELAY);
        }
        WindowEvent::Resized(sz) if sz.width > 0 && sz.height > 0 => {
            let ls = sz.to_logical::<f64>(scale());
            st.with(|d| {
                let b = d.list.bounds.get_or_insert(Bounds { x: 0.0, y: 0.0, width: LIST_W, height: LIST_H });
                b.width = ls.width;
                b.height = ls.height;
            });
            st.saver.schedule(GEOM_DELAY);
        }
        WindowEvent::CloseRequested { api, .. } => {
            api.prevent_close();
            close_list(app);
        }
        _ => {}
    }
}

pub fn close_list(app: &AppHandle) {
    let st = state(app);
    st.with(|d| d.list.open = false);
    if let Some(w) = app.get_webview_window(LIST) {
        let _ = w.destroy();
    }
    st.saver.schedule(TYPE_DELAY);
}

// ---------- all windows ----------

/// Brings every open note forward (the list window if none are open).
pub fn show_all(app: &AppHandle, focus: bool) {
    let st = state(app);
    let ids: Vec<String> = st.with(|d| d.notes.iter().filter(|n| n.open).map(|n| n.id.clone()).collect());
    if let Some(w) = app.get_webview_window(LIST) {
        reveal(&w, focus && ids.is_empty());
    }
    if ids.is_empty() {
        open_list(app, focus);
        return;
    }
    for id in ids {
        open_note(app, &id, focus);
    }
}

pub fn apply_theme(app: &AppHandle, theme: &str) {
    let theme = if theme == "light" { "light" } else { "dark" };
    let st = state(app);
    st.with(|d| d.settings.theme = theme.to_string());
    st.saver.schedule(TYPE_DELAY);
    for w in app.webview_windows().values() {
        let _ = w.set_background_color(Some(bg(theme)));
    }
    let _ = app.emit("theme", theme);
}

#[cfg(test)]
mod tests {
    use super::*;

    const MON: Rect = (0.0, 0.0, 1920.0, 1080.0);

    #[test]
    fn on_screen_window_is_kept() {
        let b = Bounds { x: 100.0, y: 100.0, width: 260.0, height: 260.0 };
        assert_eq!(fit_rect(b, &[MON], MON), b);
    }

    #[test]
    fn off_screen_window_moves_to_primary_center() {
        let b = Bounds { x: 5000.0, y: -900.0, width: 260.0, height: 260.0 };
        let f = fit_rect(b, &[MON], MON);
        assert_eq!((f.x, f.y), ((1920.0 - 260.0) / 2.0, (1080.0 - 260.0) / 2.0));
    }

    #[test]
    fn second_monitor_is_respected() {
        let right: Rect = (1920.0, 0.0, 1280.0, 1024.0);
        let b = Bounds { x: 2000.0, y: 50.0, width: 260.0, height: 260.0 };
        assert_eq!(fit_rect(b, &[MON, right], MON), b);
        assert_ne!(fit_rect(b, &[MON], MON), b);
    }
}
