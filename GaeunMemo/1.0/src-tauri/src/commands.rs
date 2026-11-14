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
        })
    }))
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
pub async fn note_save(app: AppHandle, id: String, title: String, text: String) -> R<()> {
    let (title, text) = (clip(title.replace(['\r', '\n'], " "), MAX_TITLE), clip(text, MAX_TEXT));
    let new_title = title.clone();
    let (changed, title_changed) = st(&app).with(|d| match d.find_mut(&id) {
        Some(n) if n.title != title || n.text != text => {
            let tc = n.title != title;
            n.title = title;
            n.text = text;
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
    let hit = st(&app).with(|d| d.find_mut(&id).map(|n| n.color = color).is_some());
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
        let i = d.notes.iter().position(|n| n.id == id)?;
        Some(d.notes.remove(i))
    });
    if removed.is_none() {
        return Ok(());
    }
    if let Some(w) = app.get_webview_window(&note_label(&id)) {
        let _ = w.destroy();
    }
    win::emit_remove(&app, &id);
    s.saver.schedule(Duration::from_millis(300));
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
