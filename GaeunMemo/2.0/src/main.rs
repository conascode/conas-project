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
mod ptd;
mod render;
mod rich;
mod richedit;
mod rtfread;
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
        quit_when_empty: interactive,
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
        "diff" => visual::diff(&c.refdir, &c.outdir),
        "rich" => rich::check_file(&c.input, &c.out),
        "p2check" => p2check::run(&c.input, &c.out),
        "p3check" => p3check::run(&c.data, &c.out),
        "p3kill" => p3check::kill_child(&c.data, &c.label),
        "gif" => ptd::gif(c.images.first().map(String::as_str).unwrap_or(""), &c.out, &c.label),
        "interactive" => {
            // P3: notes are stored in the data folder (--data, else %APPDATA%\gaeunmemo2-dev)
            let dir = if c.data.is_empty() { store::default_dir() } else { std::path::PathBuf::from(&c.data) };
            if !win::single_instance(&dir) {
                std::process::exit(0); // already running on this folder (P4 brings it to the front)
            }
            note::register_class();
            log::set_dir(&dir);
            match store::open(&dir) {
                Ok(loaded) => {
                    let notice = loaded.notice.clone();
                    let list_open = loaded.windows.get("list").is_some_and(|w| w.0);
                    list::set_saved(loaded.windows.get("list").and_then(|w| w.1));
                    app::load(loaded);
                    if let Some(m) = notice {
                        log::write(&m);
                        win::message_box(&m);
                    }
                    let ids: Vec<u64> = app::with(|a| a.notes.iter().filter(|n| n.open).map(|n| n.id).collect());
                    for (k, id) in ids.iter().enumerate() {
                        app::open_note(*id, None, k + 1 == ids.len());
                    }
                    if app::with(|a| a.notes.is_empty()) {
                        app::new_note(None); // first start: one empty note
                    } else if ids.is_empty() || list_open {
                        list::open(ids.is_empty());
                    }
                }
                Err(e) => {
                    win::message_box(&format!("메모를 불러올 수 없어요.
{e}"));
                    std::process::exit(1);
                }
            }
            message_loop();
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
