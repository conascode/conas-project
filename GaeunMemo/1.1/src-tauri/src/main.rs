#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[macro_use]
mod log;
mod autostart;
mod commands;
mod images;
mod import;
mod model;
mod store;
mod tray;
mod windows;

#[cfg(windows)]
mod anchor;

use model::Data;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;
use store::{LoadOutcome, Saver};
use tauri::{AppHandle, Manager, RunEvent};

pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

pub struct AppState {
    pub data: Arc<Mutex<Data>>,
    pub saver: Saver,
    quitting: AtomicBool,
}

impl AppState {
    pub fn with<R>(&self, f: impl FnOnce(&mut Data) -> R) -> R {
        f(&mut lock(&self.data))
    }
}

static SAVER: OnceLock<Saver> = OnceLock::new();

/// Synchronous save; safe to call from any thread, including the session-end handler.
pub fn flush_now() {
    if let Some(s) = SAVER.get() {
        s.flush();
    }
}

pub fn quit(app: &AppHandle) {
    log!("EXIT: quit requested");
    app.state::<AppState>().quitting.store(true, Ordering::Relaxed);
    flush_now();
    app.exit(0);
}

fn main() {
    log::install_panic_hook();
    let autostarted = std::env::args().any(|a| a == autostart::AUTOSTART_ARG);
    let path = store::data_dir().join(store::FILE_NAME);
    log!("start v{} autostart={autostarted}", env!("CARGO_PKG_VERSION"));

    let (mut data, imported) = if path.exists() {
        let (d, outcome) = store::load(&path, &store::stamp());
        log!("load: {:?}, {} notes ({} open)", outcome, d.notes.len(), d.notes.iter().filter(|n| n.open).count());
        (d, matches!(outcome, LoadOutcome::Broken(_)))
    } else {
        match import::legacy_path().and_then(|p| import::import_from(&p)) {
            Some(d) => {
                log!("load: imported StickyMemo, {} notes", d.notes.len());
                (d, true)
            }
            None => {
                log!("load: no data file, starting empty");
                (Data::default(), false)
            }
        }
    };
    let first_run = !data.settings.initialized;

    // Autostart: the app remembers the user's choice, because reinstalling
    // (the NSIS uninstall step) deletes the Run entry behind its back.
    let mut settings_changed = false;
    match data.settings.autostart {
        None => {
            data.settings.autostart = Some(first_run || autostart::is_enabled());
            settings_changed = true;
        }
        Some(true) if !autostart::is_enabled() => {
            autostart::set_enabled(true);
            log!("autostart: Run entry was missing, restored");
        }
        _ => {}
    }

    // pictures no note refers to any more (older than a day) are removed in the background
    let keep = images::referenced(&data);
    std::thread::spawn(move || {
        let n = images::remove_unreferenced(&images::dir(), &keep, Duration::from_secs(24 * 3600));
        if n > 0 {
            log!("images: removed {n} unused");
        }
    });

    let data = Arc::new(Mutex::new(data));
    let saver = Saver::new(path, data.clone());
    let _ = SAVER.set(saver.clone());
    if imported || settings_changed {
        saver.schedule(Duration::from_millis(10));
    }

    let state = AppState {
        data,
        saver,
        quitting: AtomicBool::new(false),
    };

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            windows::show_all(app, true);
        }))
        .manage(state)
        // note pictures: http://gmimg.localhost/<uuid>.<ext>, only files from the images folder
        .register_uri_scheme_protocol("gmimg", |_ctx, req| {
            let name = req.uri().path().trim_start_matches('/').to_string();
            let resp = tauri::http::Response::builder();
            match images::read_in(&images::dir(), &name) {
                Some(bytes) => resp
                    .header("Content-Type", images::content_type(&name))
                    .header("Cache-Control", "max-age=31536000, immutable")
                    .body(bytes),
                None => resp.status(404).body(Vec::new()),
            }
            .unwrap_or_else(|_| tauri::http::Response::new(Vec::new()))
        })
        .invoke_handler(tauri::generate_handler![
            commands::note_get,
            commands::note_save,
            commands::note_set_color,
            commands::note_set_font,
            commands::note_toggle_pin,
            commands::note_toggle_fold,
            commands::note_new_near,
            commands::note_close,
            commands::note_delete,
            commands::note_add_image,
            commands::note_set_lock,
            commands::note_fullscreen,
            commands::band_set,
            commands::list_get,
            commands::list_open_note,
            commands::list_new,
            commands::list_close,
            commands::open_list,
            commands::theme_set,
        ])
        .setup(move |app| {
            let h = app.handle().clone();
            let st = h.state::<AppState>();
            let focus = !autostarted;

            autostart::remove_legacy();
            if first_run {
                autostart::set_enabled(true);
                let empty = st.with(|d| d.notes.is_empty());
                if empty {
                    windows::new_note(&h, None, focus);
                }
                st.with(|d| d.settings.initialized = true);
                st.saver.schedule(Duration::from_millis(10));
            }

            let (open, list_open) =
                st.with(|d| (d.notes.iter().filter(|n| n.open).map(|n| n.id.clone()).collect::<Vec<_>>(), d.list.open));
            for id in &open {
                windows::open_note(&h, id, focus);
            }
            if list_open || (focus && open.is_empty()) {
                windows::open_list(&h, focus && open.is_empty());
            }

            // A tray failure must not end the app (it used to abort startup).
            if let Err(e) = tray::create(&h) {
                log!("tray: create failed: {e}");
            }
            #[cfg(windows)]
            anchor::create(&h);
            log!("setup done: {} windows", h.webview_windows().len());
            Ok(())
        })
        .build(tauri::generate_context!());

    let app = match app {
        Ok(app) => app,
        Err(e) => {
            log!("EXIT: tauri build failed: {e}");
            return;
        }
    };
    app.run(|app, ev| match ev {
        RunEvent::ExitRequested { api, code, .. } => {
            let quitting = app.state::<AppState>().quitting.load(Ordering::Relaxed);
            if code.is_none() && !quitting {
                api.prevent_exit();
            } else {
                log!("EXIT: exit requested (code {code:?})");
                flush_now();
            }
        }
        RunEvent::Exit => {
            log!("EXIT: event loop ended");
            flush_now();
        }
        _ => {}
    });
}
