//! P6 measurements of the whole app (report §13.1) — store, session window, notes from the
//! database — in a test data folder, windows off-screen and never focused, no tray icon, no
//! registry. One run per process; `bench/p6/run.py` repeats and takes medians.
//!
//! * `--mode p6seed --data D --notes N`: a store with N open notes (≈1 KB formatted body)
//! * `--mode p6bench --data D --notes N`: start like the app → windows shown and painted
//!   (startup ms) → 8 s → private memory, GDI/USER → idle CPU (no focus, blocking wait) →
//!   typing CPU (≈16.7 chars/s into the first note, saves included) → WM_PAINT times
//! * `--mode p6pic --data D`: one note, ten 4000×3000 JPEG pictures pasted: time to the first
//!   picture shown, longest UI-thread stall, memory at the note size and in 전체화면

use crate::app;
use crate::metrics::{self, Json};
use crate::note;
use crate::store;
use std::time::Instant;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const BODY_HTML: &str = "<b>장보기 목록</b><br>우유 2개, 계란 한 판, 식빵<ul><li>사과 1 kg — 토요일 시장에서</li><li>회의 메모: 다음 주 화요일 오후 3시</li></ul>GaeunMemo 2.0 native rewrite: GDI chrome, RichEdit body, SQLite store. 한글 입력과 <i>English text</i>를 섞어 약 1 KB가 되도록 채운 본문입니다. 한글 입력과 English text를 섞어 약 1 KB가 되도록 채운 본문입니다. 한글 입력과 English text를 섞어 약 1 KB가 되도록 채운 본문입니다.";

pub fn seed(data: &str, n: usize) {
    let dir = std::path::Path::new(data);
    if store::open(dir).is_err() {
        std::process::exit(3);
    }
    let text = crate::rich::Doc::load(BODY_HTML, "").plain();
    for i in 0..n {
        store::send(store::Cmd::Upsert(store::Row {
            id: crate::win::uuid(),
            title: format!("메모 {}", i + 1),
            body_text: text.clone(),
            body_html: BODY_HTML.into(),
            color: i % 6,
            pinned: false,
            folded: false,
            is_open: true,
            locked: false,
            maximized: false,
            rect: None,
            font_size: 14,
            seq: i as i64,
            created_at: 1,
            updated_at: 1,
        }));
    }
    store::send(store::Cmd::Setting("initialized".into(), "1".into()));
    store::close();
}

/// Waits `ms` in a real message loop (GetMessage blocks: no busy waiting that would count as
/// CPU), with `tick` called every `every` ms if given.
fn wait(ms: u64, every: Option<u32>, mut tick: impl FnMut()) {
    unsafe {
        let end_id = 0x6E0;
        let tick_id = 0x6E1;
        let h = std::ptr::null_mut();
        let end = SetTimer(h, end_id, ms as u32, None);
        let t = every.map(|e| SetTimer(h, tick_id, e, None));
        let mut m: MSG = std::mem::zeroed();
        while GetMessageW(&mut m, std::ptr::null_mut(), 0, 0) > 0 {
            if m.message == WM_TIMER && m.hwnd.is_null() {
                if m.wParam == end {
                    break;
                }
                if Some(m.wParam) == t {
                    tick();
                    continue;
                }
            }
            TranslateMessage(&m);
            DispatchMessageW(&m);
        }
        KillTimer(h, end);
        if let Some(t) = t {
            KillTimer(h, t);
        }
    }
}

/// CPU time of the calling (UI) thread, ms.
fn thread_cpu_ms() -> f64 {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::{GetCurrentThread, GetThreadTimes};
    unsafe {
        let z = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let (mut c, mut e, mut k, mut u) = (z, z, z, z);
        GetThreadTimes(GetCurrentThread(), &mut c, &mut e, &mut k, &mut u);
        let f = |t: FILETIME| ((t.dwHighDateTime as u64) << 32 | t.dwLowDateTime as u64) as f64 / 10_000.0;
        f(k) + f(u)
    }
}

fn position(i: usize) -> (i32, i32) {
    (-15000 + (i as i32 % 10) * 300, -15000 + (i as i32 / 10) * 300)
}

/// The app's start-up path in a test folder: store, model, session window (no tray), the
/// open notes shown and painted. Returns the process age in ms at that point.
/// Where start-up time goes (ms): process until main, store open + load, session window,
/// the notes' windows (creation incl. the body), their first paint.
pub static mut PHASES: [f64; 6] = [0.0; 6];

fn start(data: &str) -> f64 {
    let dir = std::path::PathBuf::from(data);
    let t_main = metrics::age_ms();
    let t0 = Instant::now();
    note::register_class();
    // the app's start: store opened on its own thread while RichEdit sets up (A/B: the old
    // serial start with GM_SERIAL_START)
    let opened = if std::env::var_os("GM_SERIAL_START").is_some() { store::open(&dir) } else { store::open_parallel(&dir, |_, _| None, || {
        if store::has_open_notes(&dir) {
            crate::body::warm_richedit();
        }
    }) };
    let Ok(loaded) = opened else { std::process::exit(3) };
    app::load(loaded);
    let t_store = t0.elapsed().as_secs_f64() * 1000.0;
    crate::shell::create_with(&dir, None, false);
    let t_session = t0.elapsed().as_secs_f64() * 1000.0 - t_store;
    let ids: Vec<u64> = app::with(|a| a.notes.iter().filter(|n| n.open).map(|n| n.id).collect());
    let (mut create, mut paint, mut first) = (0.0, 0.0, 0.0);
    for (k, id) in ids.iter().enumerate() {
        let tc = Instant::now();
        app::open_note(*id, Some(position(k)), false);
        let c = tc.elapsed().as_secs_f64() * 1000.0;
        if k == 0 {
            first = c;
        }
        create += c;
        let tp = Instant::now();
        if let Some(h) = app::window_of(*id) {
            unsafe { UpdateWindow(h) };
        }
        paint += tp.elapsed().as_secs_f64() * 1000.0;
    }
    unsafe { PHASES = [t_main, t_store, t_session, create, paint, first] };
    metrics::age_ms()
}

pub fn bench(data: &str, out: &str, label: &str, notes: usize, idle_s: f64, type_s: f64) {
    metrics::set_diag(true);
    let pre = metrics::sample();
    let startup_ms = start(data);
    let created = metrics::sample();
    wait(8000, None, || {});
    let settled = metrics::sample();
    // idle: nothing focused, the thread blocked in GetMessage
    let c0 = metrics::cpu_ms();
    let t0 = Instant::now();
    wait((idle_s * 1000.0) as u64, None, || {});
    let idle_pct = (metrics::cpu_ms() - c0) / (t0.elapsed().as_secs_f64() * 1000.0) * 100.0;
    // typing: ≈16.7 chars/s posted to the first note's body (v1 SendKeys run: 200 chars / 20 ms
    // steps measured over the run); throttled saves and commits included
    let body = note::notes().first().map(|&h| note::body_of(h)).unwrap_or(std::ptr::null_mut());
    if std::env::var_os("GM_TYPE_HIDDEN").is_some() {
        // A/B: the same typing with the windows hidden (no painting at all)
        for &h in &note::notes() {
            unsafe { ShowWindow(h, SW_HIDE) };
        }
    }
    let typed: Vec<u16> = "가나다라 abc 한글 입력 ".encode_utf16().collect();
    let mut k = 0usize;
    let (c1, t1) = (metrics::cpu_ms(), Instant::now());
    let (pc1, uc1) = (metrics::process_cycles(), metrics::thread_cycles());
    let u1 = thread_cpu_ms();
    let saves0 = crate::stats::get().saves;
    let (ser0, sers0) = { let st = crate::stats::get(); (st.ser_us_total, st.sers) };
    let mut profile: Vec<(String, u64, u32)> = Vec::new();
    for a in &crate::metrics::PROF {
        a.store(0, std::sync::atomic::Ordering::Relaxed);
    }
    if !body.is_null() && type_s > 0.0 {
        if std::env::var_os("GM_TYPE_PROFILE").is_some() {
            profile = wait_profiled((type_s * 1000.0) as u64, 60, body, || {
                let ch = if k % 24 == 23 { 0x0D } else { typed[k % typed.len()] };
                unsafe { PostMessageW(body, WM_CHAR, ch as usize, 1) };
                k += 1;
            });
        } else {
            wait((type_s * 1000.0) as u64, Some(60), || {
                let ch = if k % 24 == 23 { 0x0D } else { typed[k % typed.len()] };
                unsafe { PostMessageW(body, WM_CHAR, ch as usize, 1) };
                k += 1;
            });
        }
    }
    let (pc2, uc2, t2) = (metrics::process_cycles(), metrics::thread_cycles(), t1.elapsed().as_secs_f64() * 1000.0);
    let prof_typing: Vec<u64> = crate::metrics::PROF.iter().map(|a| a.load(std::sync::atomic::Ordering::Relaxed)).collect();
    let type_pct = if k > 0 { (metrics::cpu_ms() - c1) / (t1.elapsed().as_secs_f64() * 1000.0) * 100.0 } else { f64::NAN };
    let type_ui_pct = if k > 0 { (thread_cpu_ms() - u1) / (t1.elapsed().as_secs_f64() * 1000.0) * 100.0 } else { f64::NAN };
    let saves = crate::stats::get().saves - saves0;
    let (ser_ms, sers) = { let st = crate::stats::get(); ((st.ser_us_total - ser0) / 1000.0, st.sers - sers0) };
    // WM_PAINT: 20 full repaints of every note (theme switch / band change cost)
    crate::stats::with(|s| {
        s.paints = 0;
        s.paint_us_total = 0.0;
        s.paint_us_max = 0.0;
    });
    note::PAINT_TRACE.with(|t| t.borrow_mut().clear());
    crate::gfx::GFX_TRACE.with(|t| t.borrow_mut().clear());
    note::DRAW_TRACE.with(|t| t.borrow_mut().clear());
    for _ in 0..20 {
        for &h in &note::notes() {
            unsafe { RedrawWindow(h, std::ptr::null(), std::ptr::null_mut(), RDW_INVALIDATE | RDW_UPDATENOW) };
        }
    }
    let paint_trace: Vec<f64> = note::PAINT_TRACE.with(|t| t.borrow().iter().take(40).map(|x| x.round()).collect());
    let st = crate::stats::get();
    let cpm = metrics::cycles_per_ms();
    let (type_cyc_pct, type_ui_cyc_pct) = if k > 0 { ((pc2 - pc1) as f64 / cpm / t2 * 100.0, (uc2 - uc1) as f64 / cpm / t2 * 100.0) } else { (f64::NAN, f64::NAN) };
    let line = Json::default()
        .s("mode", "p6bench")
        .n("typing_cpu_cyc_pct", type_cyc_pct)
        .n("typing_ui_cyc_pct", type_ui_cyc_pct)
        .n("cycles_per_ms", cpm)
        .s("label", label)
        .n("notes", notes as f64)
        .n("startup_ms", startup_ms)
        .raw("body_create_ms", { let t = note::TRACE6.with(|t| t.get()); format!("{{\"createwindow\":{:.1},\"callback_text\":{:.1},\"theme\":{:.1},\"font\":{:.1}}}", t[0], t[1], t[2], t[3]) })
        .raw("first_note_ms", { let t = note::TRACE.with(|t| t.get()); format!("{{\"ole\":{:.1},\"msftedit\":{:.1},\"body\":{:.1},\"fill\":{:.1},\"font\":{:.1},\"create\":{:.1}}}", t[0], t[1], t[2], t[3], t[4], t[5]) })
        .raw("phases_ms", { let p = unsafe { PHASES }; format!("{{\"to_main\":{:.1},\"store\":{:.1},\"session\":{:.1},\"windows\":{:.1},\"first_paint\":{:.1},\"first_window\":{:.1}}}", p[0], p[1], p[2], p[3], p[4], p[5]) })
        .sample("pre", pre)
        .sample("created", created)
        .sample("settled", settled)
        .n("idle_s", idle_s)
        .n("idle_cpu_pct", idle_pct)
        .n("typing_cpu_pct", type_pct)
        .n("typing_ui_thread_cpu_pct", type_ui_pct)
        .n("typed_chars", k as f64)
        .n("saves", saves as f64)
        .n("serialize_ms_total", ser_ms)
        .n("serializes", sers as f64)
        .n("paint_avg_us", if st.paints > 0 { st.paint_us_total / st.paints as f64 } else { f64::NAN })
        .n("paint_max_us", st.paint_us_max)
        .n("paints", st.paints as f64)
        .raw("paint_trace_us", format!("{paint_trace:?}"))
        .raw("draw_trace_us", format!("{:?}", note::DRAW_TRACE.with(|t| t.borrow().iter().take(3).map(|a| a.map(|x| x.round())).collect::<Vec<_>>())))
        .raw("gfx_trace_us", format!("{:?}", crate::gfx::GFX_TRACE.with(|t| t.borrow().iter().take(3).map(|a| a.map(|x| x.round())).collect::<Vec<_>>())))
        .raw("prof_slots", format!("{prof_typing:?}"))
        .raw("type_profile_cycles", format!("{{{}}}", profile.iter().map(|(k, c, n)| format!("\"{k}\":[{c},{n}]")).collect::<Vec<_>>().join(",")))
        .finish();
    metrics::append(out, &line);
    store::close();
}

#[link(name = "kernel32")]
extern "system" {
    fn QueryThreadCycleTime(thread: windows_sys::Win32::Foundation::HANDLE, cycles: *mut u64) -> i32;
}

fn cycles() -> u64 {
    let mut c = 0u64;
    unsafe { QueryThreadCycleTime(windows_sys::Win32::System::Threading::GetCurrentThread(), &mut c) };
    c
}

/// `wait` that charges the UI thread's cycles to each dispatched message (by window: body,
/// note, other; and message), the tick included (`tick`), and what is left (`loop`).
fn wait_profiled(ms: u64, every: u32, body: windows_sys::Win32::Foundation::HWND, mut tick: impl FnMut()) -> Vec<(String, u64, u32)> {
    let mut acc: std::collections::BTreeMap<String, (u64, u32)> = std::collections::BTreeMap::new();
    let note = unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetParent(body) };
    let all0 = cycles();
    unsafe {
        let h = std::ptr::null_mut();
        let end = SetTimer(h, 0x6E0, ms as u32, None);
        let t = SetTimer(h, 0x6E1, every, None);
        let mut m: MSG = std::mem::zeroed();
        while GetMessageW(&mut m, std::ptr::null_mut(), 0, 0) > 0 {
            let c0 = cycles();
            let key;
            if m.message == WM_TIMER && m.hwnd.is_null() {
                if m.wParam == end {
                    break;
                }
                if m.wParam == t {
                    tick();
                    let e = acc.entry("tick".into()).or_default();
                    e.0 += cycles() - c0;
                    e.1 += 1;
                    continue;
                }
            }
            let who = if m.hwnd == body { "body" } else if m.hwnd == note { "note" } else { "other" };
            key = if m.message == WM_TIMER { format!("{who}:timer{}", m.wParam) } else { format!("{who}:{:#06x}", m.message) };
            TranslateMessage(&m);
            DispatchMessageW(&m);
            let e = acc.entry(key).or_default();
            e.0 += cycles() - c0;
            e.1 += 1;
        }
        KillTimer(h, end);
        KillTimer(h, t);
    }
    let total = cycles() - all0;
    let counted: u64 = acc.values().map(|v| v.0).sum();
    acc.insert("loop".into(), (total.saturating_sub(counted), 0));
    acc.insert("total".into(), (total, 0));
    acc.into_iter().map(|(k, (c, n))| (k, c, n)).collect()
}

/// Pictures shown so far (richedit::show_picture), for `pic`.
pub static SHOWN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

pub fn pic(data: &str, out: &str, label: &str, photo: &str) {
    metrics::set_diag(true);
    let _ = start(data);
    let Some(&h) = note::notes().first() else { std::process::exit(4) };
    unsafe { SetWindowPos(h, std::ptr::null_mut(), 0, 0, crate::win::px(260.0, 144), crate::win::px(260.0, 144), SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE) };
    let body = note::body_of(h);
    let bytes = std::fs::read(photo).unwrap_or_default();
    wait(3000, None, || {});
    let before = metrics::sample();
    // ten pastes of the same photograph, each its own stored picture (as ten screenshots)
    let t0 = Instant::now();
    let mut first_ms = f64::NAN;
    let mut stall_ms: f64 = 0.0;
    let (mut add_ms, mut insert_ms, mut msg_ms, mut stall_msg) = (0.0f64, 0.0f64, 0.0f64, 0u32);
    let mut stall_wp = 0usize;
    let mut slow: Vec<String> = Vec::new();
    let mut per: Vec<String> = Vec::new();
    for _ in 0..10 {
        let ta = Instant::now();
        let name = crate::images::add(bytes.clone()).unwrap_or_default();
        add_ms = add_ms.max(ta.elapsed().as_secs_f64() * 1000.0);
        let t = Instant::now();
        crate::richedit::insert_picture(body, &name);
        insert_ms = insert_ms.max(t.elapsed().as_secs_f64() * 1000.0);
        per.push(format!("[{:.1},{:.1}]", t.duration_since(ta).as_secs_f64() * 1000.0, t.elapsed().as_secs_f64() * 1000.0));
        stall_ms = stall_ms.max(ta.elapsed().as_secs_f64() * 1000.0);
    }
    // until all ten are decoded and shown, timing each message the UI thread handles
    let deadline = Instant::now() + std::time::Duration::from_secs(30);
    unsafe {
        let mut m: MSG = std::mem::zeroed();
        while SHOWN.load(std::sync::atomic::Ordering::Relaxed) < 10 && Instant::now() < deadline {
            if PeekMessageW(&mut m, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                let t = Instant::now();
                TranslateMessage(&m);
                DispatchMessageW(&m);
                let e = t.elapsed().as_secs_f64() * 1000.0;
                if e > msg_ms {
                    msg_ms = e;
                    stall_msg = m.message;
                    stall_wp = m.wParam;
                }
                if e > 8.0 {
                    slow.push(format!("[{},{},{:.1}]", m.message, m.wParam, e));
                }
                stall_ms = stall_ms.max(e);
                if first_ms.is_nan() && SHOWN.load(std::sync::atomic::Ordering::Relaxed) > 0 {
                    first_ms = t0.elapsed().as_secs_f64() * 1000.0;
                }
            } else {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
    }
    let all_ms = t0.elapsed().as_secs_f64() * 1000.0;
    wait(2000, None, || {});
    let shown = metrics::sample();
    let w_note = crate::richedit::picture_widths(body);
    // 전체화면: the sharp copies at the wider text width
    if let Some(s) = unsafe { note::state_of(h) } {
        note::set_full(s, true);
    }
    wait(4000, None, || {});
    let full = metrics::sample();
    let w_full = crate::richedit::picture_widths(body);
    // what the budget allows: display pixels × 4 B × 1.2 (4:3 photographs)
    let budget = |w: &Vec<i32>| w.iter().map(|&x| (x as f64) * (x as f64 * 0.75) * 4.0 * 1.2).sum::<f64>() / 1048576.0;
    let line = Json::default()
        .s("mode", "p6pic")
        .s("label", label)
        .n("pictures", 10.0)
        .n("first_shown_ms", first_ms)
        .n("all_shown_ms", all_ms)
        .n("ui_stall_max_ms", stall_ms)
        .n("stall_add_ms", add_ms)
        .n("stall_insert_ms", insert_ms)
        .n("stall_msg_ms", msg_ms)
        .n("stall_msg", stall_msg as f64)
        .raw("per_paste_add_insert_ms", format!("[{}]", per.join(",")))
        .n("stall_wparam", stall_wp as f64)
        .raw("slow_msgs", format!("[{}]", slow.join(",")))
        .n("serialize_max_ms", crate::stats::get().ser_us_max / 1000.0)
        .raw("show_steps_ms", format!("{:?}", crate::richedit::SHOW_TRACE.with(|t| t.borrow().iter().map(|a| a.map(|x| (x * 10.0).round() / 10.0)).collect::<Vec<_>>())))
        .raw("save_steps_ms", format!("{:?}", crate::richedit::SAVE_TRACE.with(|t| t.borrow().iter().map(|a| a.map(|x| (x * 10.0).round() / 10.0)).collect::<Vec<_>>())))
        .raw("insert_steps_ms", format!("{:?}", crate::richedit::INS_TRACE.with(|t| t.borrow().iter().map(|a| a.map(|x| (x * 10.0).round() / 10.0)).collect::<Vec<_>>())))
        .sample("before", before)
        .sample("shown", shown)
        .sample("full", full)
        .n("budget_note_mb", budget(&w_note))
        .n("budget_full_mb", budget(&w_full))
        .raw("widths_note", format!("{w_note:?}"))
        .raw("widths_full", format!("{w_full:?}"))
        .finish();
    metrics::append(out, &line);
    store::close();
}

/// Bytes in use and committed in all heaps of the process (MB): tells memory still held by
/// the program from freed space a heap keeps for later.
fn heap_mb() -> (f64, f64) {
    use windows_sys::Win32::System::Memory::{GetProcessHeaps, HeapSummary, HEAP_SUMMARY};
    unsafe {
        let mut hs = vec![std::ptr::null_mut(); 64];
        let n = GetProcessHeaps(hs.len() as u32, hs.as_mut_ptr()) as usize;
        let (mut used, mut committed) = (0usize, 0usize);
        for &h in &hs[..n.min(hs.len())] {
            let mut s: HEAP_SUMMARY = std::mem::zeroed();
            s.cb = std::mem::size_of::<HEAP_SUMMARY>() as u32;
            if HeapSummary(h, 0, &mut s) != 0 {
                used += s.cbAllocated;
                committed += s.cbCommitted;
            }
        }
        (used as f64 / 1048576.0, committed as f64 / 1048576.0)
    }
}

/// `--mode p6picleak --data D` (after `p6pic` on D): the ten-picture note opened (all
/// pictures decoded and shown) and closed 200 times; private memory after 10 warm-up cycles
/// against the end (report §8.3: ≤ +1 MB).
pub fn picleak(data: &str, out: &str, label: &str, cycles: usize) {
    let dir = std::path::PathBuf::from(data);
    note::register_class();
    let Ok(loaded) = store::open(&dir) else { std::process::exit(3) };
    app::load(loaded);
    crate::shell::create_with(&dir, None, false);
    let Some(id) = app::with(|a| a.notes.iter().find(|n| n.html.contains("data-gm=")).map(|n| n.id)) else { std::process::exit(4) };
    let pictures = app::note(id).map(|n| crate::store::image_refs(&n.html).len()).unwrap_or(0) as u32;
    let once = || {
        let before = SHOWN.load(std::sync::atomic::Ordering::Relaxed);
        app::open_note(id, Some((-15000, -15000)), false);
        let deadline = Instant::now() + std::time::Duration::from_secs(10);
        while SHOWN.load(std::sync::atomic::Ordering::Relaxed) < before + pictures && Instant::now() < deadline {
            wait(20, None, || {});
        }
        if let Some(h) = app::window_of(id) {
            unsafe { DestroyWindow(h) };
        }
        wait(20, None, || {});
    };
    for _ in 0..10 {
        once();
    }
    let s0 = metrics::sample();
    let h0 = heap_mb();
    let t0 = Instant::now();
    let mut trace = Vec::new();
    let mut htrace = Vec::new();
    for k in 0..cycles {
        once();
        if (k + 1) % (cycles / 4).max(1) == 0 {
            trace.push(format!("{:.2}", metrics::sample().private_mb));
            htrace.push(format!("{:.2}", heap_mb().0));
        }
    }
    let s1 = metrics::sample();
    let h1 = heap_mb();
    let line = Json::default()
        .s("mode", "p6picleak")
        .n("d_heap_used_mb", h1.0 - h0.0)
        .n("d_heap_committed_mb", h1.1 - h0.1)
        .raw("heap_used_mb_trace", format!("[{}]", htrace.join(",")))
        .s("label", label)
        .n("pictures", pictures as f64)
        .n("cycles", cycles as f64)
        .n("seconds", t0.elapsed().as_secs_f64())
        .sample("start", s0)
        .sample("end", s1)
        .raw("private_mb_trace", format!("[{}]", trace.join(",")))
        .n("d_private_mb", s1.private_mb - s0.private_mb)
        .n("d_gdi", s1.gdi as f64 - s0.gdi as f64)
        .n("d_user", s1.user as f64 - s0.user as f64)
        .n("d_handles", s1.handles as f64 - s0.handles as f64)
        .n("shown", SHOWN.load(std::sync::atomic::Ordering::Relaxed) as f64)
        .n("dropped_decoded", note::DROPPED_DECODED.load(std::sync::atomic::Ordering::Relaxed) as f64)
        .finish();
    metrics::append(out, &line);
    store::close();
}

/// `--mode p6list --data D` (a store with many notes): the list window off-screen, the
/// search filter timed (report §9.2: 1,000 notes ≤ 1 ms).
pub fn list_filter(data: &str, out: &str, label: &str) {
    let dir = std::path::PathBuf::from(data);
    note::register_class();
    let Ok(loaded) = store::open(&dir) else { std::process::exit(3) };
    app::load(loaded);
    crate::list::open(false);
    wait(200, None, || {});
    let n = app::with(|a| a.notes.len());
    let mut times = Vec::new();
    for q in ["메", "메모", "메모 9", "장보기", "없는말", "", "a", "1"] {
        let t = Instant::now();
        crate::list::filter_for_bench(q);
        times.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    // the first key fills the lower-case cache of the list (once per list window)
    let first = times.remove(0);
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let line = Json::default().s("mode", "p6list").s("label", label).n("notes", n as f64).n("filter_ms_first", first).n("filter_ms_median", times[times.len() / 2]).n("filter_ms_max", times[times.len() - 1]).finish();
    metrics::append(out, &line);
    store::close();
}

/// `--mode p6reimg --label VARIANT`: where picture memory goes, without the app — a hidden
/// RICHEDIT50W made and destroyed `cycles` times. VARIANT: `none` (no pictures), `stream`
/// (ten memory streams, no control), `ph` (ten placeholders inserted), `img` (ten 260×195
/// PNGs inserted), `imgdel` (inserted, then the text deleted before destroying).
pub fn reimg(out: &str, label: &str, cycles: usize) {
    let dpi = 144;
    crate::body::ensure_richedit();
    let img = crate::images::sample_png(260, 195);
    let ph = crate::images::placeholder_bytes(dpi);
    let bytes = if label == "ph" { ph } else { img };
    let once = || unsafe {
        if label == "decode" || label == "decodeui" {
            // the decoding thread's work only: file read + scaled JPEG copy, ten times
            let work = move || {
                for _ in 0..10 {
                    if let Ok(b) = std::fs::read("bench/img/photo_4000x3000.jpg") {
                        let _ = crate::images::display_stream(&b, 248.0, dpi);
                    }
                }
            };
            if label == "decode" {
                let _ = std::thread::spawn(move || {
                    let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
                    work();
                })
                .join();
            } else {
                work();
            }
            return;
        }
        if label == "rtf" {
            let h = CreateWindowExW(0, crate::win::wide("RICHEDIT50W").as_ptr(), std::ptr::null(), WS_POPUP, -15000, -15000, 300, 400, std::ptr::null_mut(), std::ptr::null_mut(), windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()), std::ptr::null());
            SendMessageW(h, 0x0459 /* EM_SETTEXTMODE */, 0x2 | 0x8, 0);
            if let Some(doc) = crate::richedit::tdoc_for(h) {
                if let Ok(r) = doc.Range2(0, 0) {
                    let _ = r.SetText(&windows::core::BSTR::from("ab \r\rcd"));
                }
                if let Some(st) = windows::Win32::UI::Shell::SHCreateMemStream(Some(&bytes)) {
                    crate::images::insert_stream(&doc, 3, "pic1.png", &st, 26.0, 19.0);
                }
            }
            for f in [2u32, 3] {
                let r = crate::richedit::stream_with(h, f);
                let t = String::from_utf8_lossy(&r);
                let t: String = t.chars().filter(|c| *c != '\r' && *c != '\n').collect();
                let cut: String = t.chars().take(1400).collect();
                eprintln!("flags {f}: {} bytes\n{}\n", r.len(), cut.split("\\pard").skip(1).collect::<Vec<_>>().join(" | \\pard"));
            }
            DestroyWindow(h);
            std::process::exit(0);
        }
        if label == "stream" {
            for _ in 0..10 {
                let s = windows::Win32::UI::Shell::SHCreateMemStream(Some(&bytes));
                drop(s);
            }
            return;
        }
        let h = CreateWindowExW(0, crate::win::wide("RICHEDIT50W").as_ptr(), std::ptr::null(), WS_POPUP, -15000, -15000, 300, 400, std::ptr::null_mut(), std::ptr::null_mut(), windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()), std::ptr::null());
        if h.is_null() {
            std::process::exit(5); // no RichEdit: the run would measure nothing
        }
        if label != "none" {
            if let Some(doc) = crate::richedit::tdoc_for(h) {
                for k in 0..10 {
                    if let Some(s) = windows::Win32::UI::Shell::SHCreateMemStream(Some(&bytes)) {
                        crate::images::insert_stream(&doc, k, "x.png", &s, 260.0, 195.0);
                    }
                }
                if label == "imgdel" {
                    if let Ok(r) = doc.Range2(0, 100) {
                        let _ = r.SetText(&windows::core::BSTR::new());
                    }
                }
            }
        }
        DestroyWindow(h);
        wait(5, None, || {});
    };
    for _ in 0..10 {
        once();
    }
    let s0 = metrics::sample();
    let mut trace = Vec::new();
    for k in 0..cycles {
        once();
        if (k + 1) % (cycles / 4).max(1) == 0 {
            trace.push(format!("{:.2}", metrics::sample().private_mb));
        }
    }
    let s1 = metrics::sample();
    let line = Json::default()
        .s("mode", "p6reimg")
        .s("label", label)
        .n("cycles", cycles as f64)
        .raw("private_mb_trace", format!("[{}]", trace.join(",")))
        .n("d_private_mb", s1.private_mb - s0.private_mb)
        .n("d_gdi", s1.gdi as f64 - s0.gdi as f64)
        .n("d_user", s1.user as f64 - s0.user as f64)
        .n("d_handles", s1.handles as f64 - s0.handles as f64)
        .finish();
    metrics::append(out, &line);
}
