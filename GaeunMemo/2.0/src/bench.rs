//! Self-measuring scenarios (report §6.7 / §13). Each run is one process that
//! creates its windows, measures itself, appends one JSON line and exits.
//! Windows are placed far off-screen and never activated, so the harness does
//! not disturb the desktop; DWM still redirects and paints them.

use crate::metrics::{self, Json, Sample};
use crate::note;
use crate::win::wide;
use crate::{cfg, stats};
use std::time::Instant;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::{RedrawWindow, UpdateWindow, RDW_INVALIDATE, RDW_UPDATENOW};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const T_SETTLE: usize = 1;
const T_IDLE: usize = 2;
const T_TYPE: usize = 3;
const T_TYPE_END: usize = 4;

struct Run {
    pre: Sample,
    created: Sample,
    startup_ms: f64,
    /// window-creation loop only (startup_ms also includes process launch and DLL loading)
    create_ms: f64,
    settled: Sample,
    idle_t0: Instant,
    idle_cpu0: f64,
    idle_pct: f64,
    idle_s: f64,
    idled: Sample,
    type_t0: Instant,
    type_cpu0: f64,
    chars: u32,
}

static mut RUN: Option<Run> = None;

#[allow(static_mut_refs)]
fn run_state() -> &'static mut Run {
    unsafe { RUN.as_mut().unwrap() }
}

pub fn position(i: usize) -> (i32, i32) {
    if cfg().onscreen {
        (60 + (i % 10) as i32 * 32 + (i / 10) as i32 * 300, 60 + (i % 10) as i32 * 32)
    } else {
        (-15000 + (i % 10) as i32 * 400, -15000 + (i / 10) as i32 * 400)
    }
}

fn pump() {
    unsafe {
        let mut m: MSG = std::mem::zeroed();
        while PeekMessageW(&mut m, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
            TranslateMessage(&m);
            DispatchMessageW(&m);
        }
    }
}

fn host_window() -> HWND {
    unsafe {
        let name = wide("GM2_Bench");
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: 0,
            lpfnWndProc: Some(host_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: GetModuleHandleW(std::ptr::null()),
            hIcon: std::ptr::null_mut(),
            hCursor: std::ptr::null_mut(),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: name.as_ptr(),
            hIconSm: std::ptr::null_mut(),
        };
        RegisterClassExW(&wc);
        CreateWindowExW(0, name.as_ptr(), name.as_ptr(), 0, 0, 0, 0, 0, HWND_MESSAGE, std::ptr::null_mut(), GetModuleHandleW(std::ptr::null()), std::ptr::null())
    }
}

/// Mode `run`: startup → settle → idle → typing → repaint → JSON.
pub fn start_run() {
    let c = cfg();
    let pre = metrics::sample();
    note::register_class();
    let host = host_window();
    let t_create = Instant::now();
    for i in 0..c.notes {
        let (x, y) = position(i);
        let h = note::create(i, x, y, true);
        unsafe { UpdateWindow(h) };
    }
    let startup_ms = metrics::age_ms();
    let create_ms = t_create.elapsed().as_secs_f64() * 1000.0;
    let created = metrics::sample();
    unsafe {
        RUN = Some(Run {
            pre,
            created,
            startup_ms,
            create_ms,
            settled: Sample::default(),
            idle_t0: Instant::now(),
            idle_cpu0: 0.0,
            idle_pct: 0.0,
            idle_s: 0.0,
            idled: Sample::default(),
            type_t0: Instant::now(),
            type_cpu0: 0.0,
            chars: 0,
        });
        SetTimer(host, T_SETTLE, (c.settle_s * 1000.0) as u32, None);
    }
}

const TYPED: &[u16] = &[0xAC00, 0xB098, 0xB2E4, 0xB77C, 0x20, 0x61, 0x62, 0x63, 0x20, 0xD55C, 0xAE00, 0x20];

unsafe extern "system" fn host_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg != WM_TIMER {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    let c = cfg();
    let r = run_state();
    match wp {
        T_SETTLE => {
            KillTimer(hwnd, T_SETTLE);
            r.settled = metrics::sample();
            r.idle_cpu0 = metrics::cpu_ms();
            r.idle_t0 = Instant::now();
            SetTimer(hwnd, T_IDLE, (c.idle_s * 1000.0) as u32, None);
        }
        T_IDLE => {
            KillTimer(hwnd, T_IDLE);
            r.idle_s = r.idle_t0.elapsed().as_secs_f64();
            r.idle_pct = (metrics::cpu_ms() - r.idle_cpu0) / (r.idle_s * 1000.0) * 100.0;
            r.idled = metrics::sample();
            let target = note::notes().first().map(|&h| note::body_of(h)).unwrap_or(std::ptr::null_mut());
            if c.type_s > 0.0 && !target.is_null() {
                r.type_cpu0 = metrics::cpu_ms();
                r.type_t0 = Instant::now();
                SetTimer(hwnd, T_TYPE, 60, None); // ≈16.7 chars/s, like the v1 SendKeys run
                SetTimer(hwnd, T_TYPE_END, (c.type_s * 1000.0) as u32, None);
            } else {
                finish(hwnd, f64::NAN);
            }
        }
        T_TYPE => {
            let target = note::notes().first().map(|&h| note::body_of(h)).unwrap_or(std::ptr::null_mut());
            let ch = if r.chars % 24 == 23 { 0x0D } else { TYPED[r.chars as usize % TYPED.len()] };
            PostMessageW(target, WM_CHAR, ch as usize, 1);
            r.chars += 1;
        }
        T_TYPE_END => {
            KillTimer(hwnd, T_TYPE);
            KillTimer(hwnd, T_TYPE_END);
            pump();
            let secs = r.type_t0.elapsed().as_secs_f64();
            let pct = (metrics::cpu_ms() - r.type_cpu0) / (secs * 1000.0) * 100.0;
            finish(hwnd, pct);
        }
        _ => {}
    }
    0
}

fn finish(hwnd: HWND, typing_pct: f64) {
    let c = cfg();
    let r = run_state();
    let typed = metrics::sample();
    // repaint phase: 20 full repaints of every note (theme switch / band cycle cost)
    stats::with(|s| {
        s.paints = 0;
        s.paint_us_total = 0.0;
        s.paint_us_max = 0.0;
    });
    let notes = note::notes();
    let t0 = Instant::now();
    for _ in 0..20 {
        for &h in &notes {
            unsafe { RedrawWindow(h, std::ptr::null(), std::ptr::null_mut(), RDW_INVALIDATE | RDW_UPDATENOW) };
        }
    }
    let repaint_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let st = stats::get();
    let line = Json::default()
        .s("mode", "run")
        .s("label", &c.label)
        .s("render", c.render.name())
        .s("body", c.body.name())
        .n("notes", c.notes as f64)
        .b("taskbar", c.taskbar)
        .b("onscreen", c.onscreen)
        .n("startup_ms", r.startup_ms)
        .n("create_ms", r.create_ms)
        .n("first_paints", st.first_paints as f64)
        .sample("pre", r.pre)
        .sample("created", r.created)
        .sample("settled", r.settled)
        .sample("idle", r.idled)
        .sample("typed", typed)
        .n("idle_s", r.idle_s)
        .n("idle_cpu_pct", r.idle_pct)
        .n("typing_cpu_pct", typing_pct)
        .n("typed_chars", r.chars as f64)
        .n("saves", st.saves as f64)
        .n("repaint_ms", repaint_ms)
        .n("paint_avg_us", if st.paints > 0 { st.paint_us_total / st.paints as f64 } else { f64::NAN })
        .n("paint_max_us", st.paint_us_max)
        .n("repaints", st.paints as f64)
        .finish();
    metrics::append(&c.out, &line);
    let _ = hwnd;
    unsafe { PostQuitMessage(0) };
}

/// Mode `leak`: create/destroy `batch` notes `cycles` times; report resource drift.
pub fn leak() {
    let c = cfg();
    note::register_class();
    let cycle = || {
        let hs: Vec<HWND> = (0..c.batch)
            .map(|i| {
                let (x, y) = position(i);
                let h = note::create(i, x, y, true);
                unsafe { UpdateWindow(h) };
                h
            })
            .collect();
        pump();
        for h in hs {
            unsafe { DestroyWindow(h) };
        }
        pump();
    };
    for _ in 0..2 {
        cycle();
    }
    let s0 = metrics::sample();
    let t0 = Instant::now();
    let mut mid = Vec::new();
    for k in 0..c.cycles {
        cycle();
        if (k + 1) % (c.cycles / 4).max(1) == 0 {
            mid.push(metrics::sample().private_mb);
        }
    }
    let s1 = metrics::sample();
    let line = Json::default()
        .s("mode", "leak")
        .s("label", &c.label)
        .s("render", c.render.name())
        .s("body", c.body.name())
        .n("batch", c.batch as f64)
        .n("cycles", c.cycles as f64)
        .n("windows_created", (c.batch * (c.cycles + 2)) as f64)
        .n("seconds", t0.elapsed().as_secs_f64())
        .sample("start", s0)
        .sample("end", s1)
        .raw("private_mb_trace", format!("{:?}", mid.iter().map(|v| (v * 100.0).round() / 100.0).collect::<Vec<_>>()))
        .n("d_private_mb", s1.private_mb - s0.private_mb)
        .n("d_gdi", s1.gdi as f64 - s0.gdi as f64)
        .n("d_user", s1.user as f64 - s0.user as f64)
        .n("d_handles", s1.handles as f64 - s0.handles as f64)
        .finish();
    metrics::append(&c.out, &line);
}
