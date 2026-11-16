//! GaeunMemo 2.0 — P0: Win32 skeleton + measurement prototypes (report GM2-TDR-001 §6.7, §17).
//!
//! gaeunmemo2.exe                         interactive: three notes to try (drag, resize, fold,
//!                                        pin, new, full screen, close)
//! gaeunmemo2.exe --mode run  [opts]      PT-A/B/B'/C measurement run → one JSON line
//! gaeunmemo2.exe --mode leak [opts]      create/destroy cycles → resource drift
//! gaeunmemo2.exe --mode ptd --images a,b PT-D picture test
//!
//! opts: --render gdi|d2d-hwnd|d2d-dc  --body richedit|edit|none  --notes N
//!       --settle S --idle S --type S  --batch N --cycles N  --out FILE  --label L
//!       --taskbar (WS_EX_APPWINDOW)  --onscreen (default: off-screen, never activated)
//!
//! Touches no files except --out, no registry, no AppData.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod autostart;
mod bench;
mod body;
mod confirm;
mod gfx;
mod icons;
mod images;
mod list;
mod log;
mod menu;
mod metrics;
mod note;
mod p2check;
mod p3check;
mod p4check;
mod ptd;
mod render;
mod rich;
mod richedit;
mod rtfread;
mod sbar;
mod shell;
mod selftest;
mod store;
mod theme;
mod visual;
mod win;

use body::BodyKind;
use render::RenderKind;
use std::sync::OnceLock;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub struct Config {
    pub mode: String,
    pub render: RenderKind,
    pub body: BodyKind,
    pub notes: usize,
    pub settle_s: f64,
    pub idle_s: f64,
    pub type_s: f64,
    pub batch: usize,
    pub cycles: usize,
    pub out: String,
    pub label: String,
    pub images: Vec<String>,
    pub cases: String,
    pub outdir: String,
    pub refdir: String,
    pub input: String,
    pub data: String,
    pub taskbar: bool,
    pub onscreen: bool,
    pub quit_when_empty: bool,
}

static CFG: OnceLock<Config> = OnceLock::new();

pub fn cfg() -> &'static Config {
    CFG.get().expect("config")
}

/// Test / measurement modes: never activate, focus or bring windows to the front.
pub fn quiet() -> bool {
    cfg().mode != "interactive"
}

pub mod stats {
    use std::cell::RefCell;
    #[derive(Default, Clone, Copy)]
    pub struct Stats {
        pub paints: u64,
        pub first_paints: u64,
        pub paint_us_total: f64,
        pub paint_us_max: f64,
        pub saves: u64,
    }
    thread_local! {
        static S: RefCell<Stats> = RefCell::new(Stats::default());
    }
    pub fn with(f: impl FnOnce(&mut Stats)) {
        S.with(|s| f(&mut s.borrow_mut()));
    }
    pub fn get() -> Stats {
        S.with(|s| *s.borrow())
    }
}

fn parse() -> Config {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let get = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let has = |k: &str| args.iter().any(|a| a == k);
    let num = |k: &str, d: f64| get(k).and_then(|v| v.parse().ok()).unwrap_or(d);
    let mode = get("--mode").unwrap_or_else(|| "interactive".into());
    let interactive = mode == "interactive";
    Config {
        render: match get("--render").as_deref() {
            Some("d2d-hwnd") | Some("d2d") => RenderKind::D2dHwnd,
            Some("d2d-dc") => RenderKind::D2dDc,
            _ => RenderKind::Gdi,
        },
        body: match get("--body").as_deref() {
            Some("edit") => BodyKind::Edit,
            Some("none") => BodyKind::None,
            _ => BodyKind::RichEdit,
        },
        notes: num("--notes", if interactive { 3.0 } else { 1.0 }) as usize,
        settle_s: num("--settle", 3.0),
        idle_s: num("--idle", 30.0),
        type_s: num("--type", 10.0),
        batch: num("--batch", 50.0) as usize,
        cycles: num("--cycles", 100.0) as usize,
        out: get("--out").unwrap_or_default(),
        label: get("--label").unwrap_or_default(),
        cases: get("--cases").unwrap_or_else(|| r"bench\visual\cases.tsv".into()),
        outdir: get("--outdir").unwrap_or_default(),
        refdir: get("--refdir").unwrap_or_default(),
        input: get("--in").unwrap_or_default(),
        data: get("--data").unwrap_or_default(),
        images: get("--images").map(|v| v.split(',').map(String::from).collect()).unwrap_or_default(),
        taskbar: interactive || has("--taskbar"),
        onscreen: interactive || has("--onscreen"),
        // P4: the app stays in the tray when every window is closed (v1.1); 종료 is in the tray menu
        quit_when_empty: false,
        mode,
    }
}

fn message_loop() {
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            if accelerator(&msg) {
                continue;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Ctrl+N (new note nearby) / Ctrl+W (close) while a note is active; ignored under
/// the 삭제하시겠습니까? card, like v1.1.
fn accelerator(msg: &MSG) -> bool {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetKeyState;
    if msg.message != WM_KEYDOWN || !matches!(msg.wParam as u8, b'N' | b'W') {
        return false;
    }
    unsafe {
        if GetKeyState(0x11) >= 0 || GetKeyState(0x10) < 0 || GetKeyState(0x12) < 0 {
            return false;
        }
        let root = GetAncestor(msg.hwnd, GA_ROOT);
        let Some(id) = note::id_of(root) else { return false };
        if confirm::is_showing(root) {
            return false;
        }
        if msg.wParam as u8 == b'N' {
            app::new_note(Some(root));
        } else {
            PostMessageW(root, WM_CLOSE, 0, 0);
            let _ = id;
        }
    }
    true
}

fn main() {
    let _ = CFG.set(parse());
    let c = cfg();
    match c.mode.as_str() {
        "run" => {
            bench::start_run();
            message_loop();
        }
        "leak" => bench::leak(),
        "ptd" => ptd::run(&c.images, &c.out, &c.label),
        "selftest" => selftest::run(&c.out),
        "shots" => visual::shots(&c.cases, &c.outdir, &c.label),
        "notesnap" => visual::notesnap(&c.data, &c.outdir),
        "flashprobe" => visual::flashprobe(&c.data, &c.outdir),
        "diff" => visual::diff(&c.refdir, &c.outdir),
        "rich" => rich::check_file(&c.input, &c.out),
        "p2check" => p2check::run(&c.input, &c.out),
        "p3check" => p3check::run(&c.data, &c.out),
        "p3kill" => p3check::kill_child(&c.data, &c.label),
        "p4check" => p4check::run(&c.data, &c.out),
        "p4child" => p4check::child(&c.data),
        "gif" => ptd::gif(c.images.first().map(String::as_str).unwrap_or(""), &c.out, &c.label),
        "interactive" => {
            // P4: one app per data folder, resident in the tray (report §6.5, F-16 – F-20, F-35)
            let autostarted = std::env::args().any(|a| a == autostart::ARG);
            let focus = !autostarted; // started by Windows at logon: take no focus (F-19)
            // taskbar: every note window is a button of this app (F-17); its own id while v1.1
            // is still installed, so the two apps' windows are not grouped together
            unsafe {
                let _ = windows_sys::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID(win::wide("GaeunMemo2.Dev").as_ptr());
            }
            let custom = !c.data.is_empty();
            let dir = if custom { std::path::PathBuf::from(&c.data) } else { store::default_dir() };
            if !win::single_instance(&dir) {
                // already running on this folder: it shows its notes (F-18)
                shell::signal_running(&dir);
                std::process::exit(0);
            }
            log::set_dir(&dir);
            log::install_panic_hook();
            log::write(&format!("start v{} autostart={autostarted}", env!("CARGO_PKG_VERSION")));
            note::register_class();
            match store::open(&dir) {
                Ok(loaded) => {
                    let notice = loaded.notice.clone();
                    let list_open = loaded.windows.get("list").is_some_and(|w| w.0);
                    let first_run = loaded.settings.get("initialized").map(String::as_str) != Some("1");
                    let saved_auto = loaded.settings.get("autostart").cloned();
                    list::set_saved(loaded.windows.get("list").and_then(|w| w.1));
                    app::load(loaded);
                    let (total, open) = app::with(|a| (a.notes.len(), a.notes.iter().filter(|n| n.open).count()));
                    log::write(&format!("load: {total} notes ({open} open){}", if first_run { ", first run" } else { "" }));
                    if let Some(m) = notice {
                        log::write(&m);
                        win::message_box(&m);
                    }
                    // autostart (F-19 / F-35): the stored choice wins; a value removed behind
                    // the app's back comes back, a moved development exe is followed
                    let data = custom.then(|| dir.clone());
                    let wanted = autostart::command(data.as_deref());
                    let (choice, action) = autostart::plan(saved_auto.as_deref(), first_run, autostart::current().as_deref(), &wanted);
                    if let Some(on) = choice {
                        store::send(store::Cmd::Setting("autostart".into(), if on { "1" } else { "0" }.into()));
                    }
                    match action {
                        autostart::Action::Register | autostart::Action::Update => {
                            let ok = autostart::set_enabled(true, data.as_deref());
                            log::write(&format!("autostart: {} ({})", if action == autostart::Action::Register { "registered" } else { "path updated" }, if ok { "ok" } else { "failed" }));
                        }
                        autostart::Action::None => {}
                    }
                    // first run (F-20): one empty note in the middle of the screen, once
                    if first_run {
                        if total == 0 {
                            app::new_note(None);
                        }
                        store::send(store::Cmd::Setting("initialized".into(), "1".into()));
                    }
                    let ids: Vec<u64> = app::with(|a| a.notes.iter().filter(|n| n.open).map(|n| n.id).collect());
                    for (k, id) in ids.iter().enumerate() {
                        app::open_note(*id, None, focus && k + 1 == ids.len());
                    }
                    // the list as it was, or — started by the user with nothing open — to
                    // show something (at logon with nothing open: the tray only)
                    if list_open || (focus && ids.is_empty() && !(first_run && total == 0)) {
                        list::open(focus && ids.is_empty());
                    }
                }
                Err(e) => {
                    log::write(&format!("EXIT: store open failed: {e}"));
                    win::message_box(&format!("메모를 불러올 수 없어요.
{e}"));
                    std::process::exit(1);
                }
            }
            shell::create(&dir, custom.then(|| dir.clone()));
            message_loop();
            shell::remove_tray();
            log::write("EXIT: message loop ended");
            store::close();
        }
        // an unknown --mode (e.g. an older build than the caller expects) must never fall
        // back to opening windows on the user's screen
        _ => std::process::exit(2),
    }
    // destroy any windows left (bench modes) so per-window D2D targets are released first
    for h in note::notes() {
        unsafe { DestroyWindow(h) };
    }
    render::shutdown();
    std::process::exit(0);
}
