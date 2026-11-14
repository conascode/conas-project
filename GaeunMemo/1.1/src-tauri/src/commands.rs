//! IPC commands. All are async (window creation from a sync command
//! deadlocks on Windows) and all quietly ignore unknown ids or closed windows.

use crate::model::{now_ms, valid_color, Summary, FONT_MAX, FONT_MIN, PALETTE};
use crate::windows::{self as win, note_label};
use crate::AppState;
use serde::Serialize;
use std::time::Duration;
use tauri::{AppHandle, Manager};

type R<T> = Result<T, String>;

const MAX_TITLE: usize = 500;
const MAX_TEXT: usize = 200_000;
const MAX_HTML: usize = 1_000_000;

/// Edits to a locked note are refused here as well as in the page.
fn is_locked(app: &AppHandle, id: &str) -> bool {
    st(app).with(|d| d.find(id).map(|n| n.locked).unwrap_or(false))
}

fn st(app: &AppHandle) -> tauri::State<'_, AppState> {
    app.state::<AppState>()
}

fn clip(s: String, max: usize) -> String {
    if s.chars().count() <= max {
        s
    } else {
        s.chars().take(max).collect()
    }
}

#[derive(Serialize)]
pub struct NoteView {
    title: String,
    text: String,
    color: String,
    pinned: bool,
    folded: bool,
    theme: String,
    band: String,
    #[serde(rename = "fontSize")]
    font_size: u8,
    palette: [&'static str; 6],
    html: String,
    locked: bool,
    maximized: bool,
}

#[tauri::command]
pub async fn note_get(app: AppHandle, id: String) -> R<Option<NoteView>> {
    Ok(st(&app).with(|d| {
        let theme = d.settings.theme.clone();
        let band = d.settings.band.clone();
        d.find(&id).map(|n| NoteView {
            title: n.title.clone(),
            text: n.text.clone(),
            color: n.color.clone(),
            pinned: n.pinned,
            folded: n.folded,
            theme,
            band,
            font_size: n.font_size,
            palette: PALETTE,
            html: n.html.clone(),
            locked: n.locked,
            maximized: n.maximized,
        })
    }))
}

/// Stores a pasted/inserted picture (raw bytes in the IPC body) and returns its file name.
#[tauri::command]
pub async fn note_add_image(app: AppHandle, request: tauri::ipc::Request<'_>) -> R<String> {
    let id = request.headers().get("x-gm-id").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    if is_locked(&app, &id) {
        return Err("잠긴 메모예요".into());
    }
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
        return Err("사진 데이터가 없어요".into());
    };
    let name = crate::images::save_in(&crate::images::dir(), bytes)?;
    crate::log!("image added ({} bytes)", bytes.len());
    Ok(name)
}

#[tauri::command]
pub async fn note_set_lock(app: AppHandle, id: String, locked: bool) -> R<bool> {
    let hit = st(&app).with(|d| d.find_mut(&id).map(|n| n.locked = locked).is_some());
    if hit {
        st(&app).saver.schedule(Duration::from_millis(10));
        win::emit_upsert(&app, &id);
    }
    Ok(locked && hit)
}

/// "전체화면" (maximise) on / off. Returns the new state.
#[tauri::command]
pub async fn note_fullscreen(app: AppHandle, id: String, on: bool) -> R<bool> {
    Ok(win::set_fullscreen(&app, &id, on))
}

#[tauri::command]
pub async fn note_set_font(app: AppHandle, id: String, size: u8) -> R<()> {
    let size = size.clamp(FONT_MIN, FONT_MAX);
    let hit = st(&app).with(|d| d.find_mut(&id).map(|n| n.font_size = size).is_some());
    if hit {
        st(&app).saver.schedule(Duration::from_millis(300));
    }
    Ok(())
}

#[tauri::command]
pub async fn note_save(app: AppHandle, id: String, title: String, text: String, html: String) -> R<()> {
    let (title, text) = (clip(title.replace(['\r', '\n'], " "), MAX_TITLE), clip(text, MAX_TEXT));
    // an over-long formatted body is dropped (the plain text is still saved) rather than cut mid-tag
    let html = if html.len() > MAX_HTML { String::new() } else { html };
    let new_title = title.clone();
    let (changed, title_changed) = st(&app).with(|d| match d.find_mut(&id) {
        Some(n) if !n.locked && (n.title != title || n.text != text || n.html != html) => {
            let tc = n.title != title;
            n.title = title;
            n.text = text;
            n.html = html;
            n.updated_at = now_ms();
            (true, tc)
        }
        _ => (false, false),
    });
    if title_changed {
        win::set_note_title(&app, &id, &new_title);
    }
    if changed {
        st(&app).saver.schedule(Duration::from_millis(300));
        win::emit_upsert(&app, &id);
    }
    Ok(())
}

#[tauri::command]
pub async fn note_set_color(app: AppHandle, id: String, color: String) -> R<()> {
    if !valid_color(&color) {
        return Ok(());
    }
    let color = color.to_ascii_uppercase();
    let hit = st(&app).with(|d| d.find_mut(&id).filter(|n| !n.locked).map(|n| n.color = color).is_some());
    if hit {
        st(&app).saver.schedule(Duration::from_millis(300));
        win::emit_upsert(&app, &id);
    }
    Ok(())
}

#[tauri::command]
pub async fn note_toggle_pin(app: AppHandle, id: String) -> R<bool> {
    Ok(win::set_pin(&app, &id))
}

#[tauri::command]
pub async fn note_toggle_fold(app: AppHandle, id: String) -> R<bool> {
    Ok(win::set_fold(&app, &id))
}

#[tauri::command]
pub async fn note_new_near(app: AppHandle, id: String) -> R<()> {
    win::new_note_near(&app, &id);
    Ok(())
}

#[tauri::command]
pub async fn note_close(app: AppHandle, id: String) -> R<()> {
    win::close_note(&app, &id);
    Ok(())
}

/// Deletes a note (the page asked "삭제하시겠습니까?" first) and closes its window.
#[tauri::command]
pub async fn note_delete(app: AppHandle, id: String) -> R<()> {
    let s = st(&app);
    let removed = s.with(|d| {
        let i = d.notes.iter().position(|n| n.id == id && !n.locked)?;
        let note = d.notes.remove(i);
        // its pictures go too, unless another note shows the same file
        let still_used = crate::images::referenced(d);
        let orphans: Vec<String> =
            crate::model::image_refs(&note.html).into_iter().filter(|f| !still_used.contains(f)).collect();
        Some(orphans)
    });
    let Some(orphans) = removed else { return Ok(()) };
    if let Some(w) = app.get_webview_window(&note_label(&id)) {
        let _ = w.destroy();
    }
    win::emit_remove(&app, &id);
    s.saver.schedule(Duration::from_millis(300));
    let dir = crate::images::dir();
    for f in orphans {
        let _ = std::fs::remove_file(dir.join(f));
    }
    Ok(())
}

/// Band mode for every window: "none", "thin" or "full".
#[tauri::command]
pub async fn band_set(app: AppHandle, band: String) -> R<()> {
    win::apply_band(&app, &band);
    Ok(())
}

#[derive(Serialize)]
pub struct ListView {
    notes: Vec<Summary>,
    theme: String,
}

#[tauri::command]
pub async fn list_get(app: AppHandle) -> R<ListView> {
    Ok(st(&app).with(|d| ListView {
        notes: d.notes.iter().map(Summary::from).collect(),
        theme: d.settings.theme.clone(),
    }))
}

#[tauri::command]
pub async fn list_open_note(app: AppHandle, id: String) -> R<()> {
    win::open_note(&app, &id, true);
    Ok(())
}

#[tauri::command]
pub async fn list_new(app: AppHandle) -> R<()> {
    win::new_note(&app, None, true);
    Ok(())
}

#[tauri::command]
pub async fn list_close(app: AppHandle) -> R<()> {
    win::close_list(&app);
    Ok(())
}

#[tauri::command]
pub async fn open_list(app: AppHandle) -> R<()> {
    win::open_list(&app, true);
    Ok(())
}

#[tauri::command]
pub async fn theme_set(app: AppHandle, theme: String) -> R<()> {
    win::apply_theme(&app, &theme);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::clip;

    #[test]
    fn clip_counts_chars_not_bytes() {
        assert_eq!(clip("가나다라".into(), 2), "가나");
        assert_eq!(clip("ab".into(), 5), "ab");
    }
}
