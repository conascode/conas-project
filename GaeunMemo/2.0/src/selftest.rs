//! `--mode selftest`: P1 behaviour checks with real window messages on off-screen,
//! never-activated windows. One JSON line: every check with pass/fail and detail.

use crate::app;
use crate::confirm;
use crate::list;
use crate::menu::Act;
use crate::metrics::{self, Json};
use crate::note::{self, WM_APP_HOVER, WM_APP_MENU};
use crate::theme::{g, Band, Theme};
use crate::win::px;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{VK_ESCAPE, VK_RETURN, VK_TAB};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

struct T {
    out: Vec<String>,
    pass: usize,
    fail: usize,
}

impl T {
    fn check(&mut self, name: &str, ok: bool, detail: String) {
        if ok {
            self.pass += 1;
        } else {
            self.fail += 1;
        }
        self.out.push(Json::default().s("name", name).b("pass", ok).s("detail", &detail).finish());
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

fn wrect(h: HWND) -> RECT {
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    unsafe { GetWindowRect(h, &mut r) };
    r
}

fn hit_at(h: HWND, cx: i32, cy: i32) -> u32 {
    let mut p = POINT { x: cx, y: cy };
    unsafe {
        ClientToScreen(h, &mut p);
        let lp = ((p.y as u16 as u32) << 16 | (p.x as u16 as u32)) as isize;
        SendMessageW(h, WM_NCHITTEST, 0, lp) as u32
    }
}

fn ht_name(v: u32) -> &'static str {
    match v {
        HTCLIENT => "CLIENT",
        HTCAPTION => "CAPTION",
        HTLEFT => "LEFT",
        HTRIGHT => "RIGHT",
        HTTOPLEFT => "TOPLEFT",
        HTTOPRIGHT => "TOPRIGHT",
        HTBOTTOM => "BOTTOM",
        HTBOTTOMLEFT => "BOTTOMLEFT",
        HTBOTTOMRIGHT => "BOTTOMRIGHT",
        _ => "?",
    }
}

pub fn run(out: &str) {
    note::register_class();
    let mut t = T { out: Vec::new(), pass: 0, fail: 0 };
    app::with(|a| {
        a.theme = Theme::Dark;
        a.band = Band::Thin;
    });
    let start = metrics::sample();
    let id = app::add("장보기", "우유 2개\n계란 한 판", Some(0), 60_000);
    app::open_note(id, Some((-15000, -15000)), false);
    pump();
    let h = app::window_of(id).unwrap();
    let dpi = unsafe { GetDpiForWindow(h) };
    let s = unsafe { note::state_of(h).unwrap() };
    let mut rc = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    unsafe { GetClientRect(h, &mut rc) };
    let (w, hh) = (rc.right, rc.bottom);
    t.check("note default size 260×260", (w, hh) == (px(g::NOTE_W, dpi), px(g::NOTE_H, dpi)), format!("{w}×{hh} @ dpi {dpi}"));
    // test windows sit wholly outside the desktop, so showing them exposes nothing;
    // invalidate explicitly to exercise the paint path (on screen, ShowWindow does this)
    unsafe {
        windows_sys::Win32::Graphics::Gdi::InvalidateRect(h, std::ptr::null(), 0);
        windows_sys::Win32::Graphics::Gdi::UpdateWindow(h);
    }
    let t0 = std::time::Instant::now();
    while !s.painted && t0.elapsed().as_millis() < 500 {
        std::thread::sleep(std::time::Duration::from_millis(5));
        pump();
    }
    t.check("first paint", s.painted, format!("{} ms", t0.elapsed().as_millis()));

    // ---- hit-testing (report §6.2, F-06)
    let b = unsafe { GetSystemMetricsForDpi(SM_CXSIZEFRAME, dpi) + GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi) };
    let bar_y = px(4.0 + 17.0, dpi);
    let cases = [
        ("top edge middle drags", w / 2, 1, HTCAPTION),
        ("top-left corner resizes", 1, 1, HTTOPLEFT),
        ("top-right corner resizes", w - 2, 1, HTTOPRIGHT),
        ("left edge", 1, hh / 2, HTLEFT),
        ("right edge", w - 2, hh / 2, HTRIGHT),
        ("bottom edge", w / 2, hh - 2, HTBOTTOM),
        ("bottom-right corner", w - 2, hh - 2, HTBOTTOMRIGHT),
        ("title bar drags", px(40.0, dpi), bar_y, HTCAPTION),
        ("body is client", w / 2, hh / 2, HTCLIENT),
    ];
    for (name, x, y, want) in cases {
        let got = hit_at(h, x, y);
        t.check(&format!("hit: {name}"), got == want, format!("({x},{y}) frame={b} → {}", ht_name(got)));
    }
    let close_x = w - px(5.0 + 12.0, dpi);
    let got = hit_at(h, close_x, bar_y);
    t.check("hit: tool area drags while tools are hidden", got == HTCAPTION, ht_name(got).into());
    unsafe { SendMessageW(h, WM_APP_HOVER, 0, 0) };
    let got = hit_at(h, close_x, bar_y);
    t.check("hit: × is clickable while hovered", got == HTCLIENT, ht_name(got).into());

    // ---- min / max (WM_GETMINMAXINFO, F-01, F-29)
    unsafe {
        let mut mmi: MINMAXINFO = std::mem::zeroed();
        SendMessageW(h, WM_GETMINMAXINFO, 0, &mut mmi as *mut _ as isize);
        let mon = MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST);
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        GetMonitorInfoW(mon, &mut mi);
        t.check("min size 180×130", (mmi.ptMinTrackSize.x, mmi.ptMinTrackSize.y) == (px(180.0, dpi), px(130.0, dpi)), format!("{}×{}", mmi.ptMinTrackSize.x, mmi.ptMinTrackSize.y));
        let work = (mi.rcWork.right - mi.rcWork.left, mi.rcWork.bottom - mi.rcWork.top);
        t.check("maximise = monitor work area (taskbar stays visible)", (mmi.ptMaxSize.x, mmi.ptMaxSize.y) == work, format!("max {}×{} at ({},{}) work {}×{}", mmi.ptMaxSize.x, mmi.ptMaxSize.y, mmi.ptMaxPosition.x, mmi.ptMaxPosition.y, work.0, work.1));
    }

    // ---- fold per band (F-07, F-02)
    unsafe { SendMessageW(h, WM_NCLBUTTONDBLCLK, HTCAPTION as usize, 0) };
    let fh = |h| wrect(h).bottom - wrect(h).top;
    t.check("fold (thin band) = 4 + 34", fh(h) == px(38.0, dpi), format!("{}", fh(h)));
    app::cycle_band(); // thin → full
    pump();
    t.check("fold (제목 줄 band) = 34", fh(h) == px(34.0, dpi), format!("{}", fh(h)));
    app::cycle_band(); // full → none
    pump();
    t.check("fold (no band) = 34", fh(h) == px(34.0, dpi), format!("{}", fh(h)));
    app::cycle_band(); // none → thin
    unsafe { SendMessageW(h, WM_NCLBUTTONDBLCLK, HTCAPTION as usize, 0) };
    t.check("unfold restores the height", fh(h) == px(260.0, dpi), format!("{}", fh(h)));

    // ---- ⋯ menu placement and compact rule (F-25)
    note::open_menu(s);
    pump();
    let m = s.menu;
    t.check("⋯ opens the menu", !m.is_null(), String::new());
    if !m.is_null() {
        let mr = wrect(m);
        let nr = wrect(h);
        let one = 1; // the menu outline is one device pixel (v1.1 renders 1 CSS px as 1 device px)
        t.check("menu right edge = note right − 5 (+1 outline)", mr.right == nr.right - px(5.0, dpi) + one, format!("menu {} note {}", mr.right, nr.right));
        let compact_h = px(4.0 + 20.0 * 7.0 + 20.0 + 4.0, dpi) + 2 * one;
        t.check("260 px note → compact menu (top 2, rows 20)", mr.bottom - mr.top == compact_h && mr.top == nr.top + px(2.0, dpi) - one, format!("h {} (want {compact_h}) top {}", mr.bottom - mr.top, mr.top - nr.top));
        unsafe { SetWindowPos(h, std::ptr::null_mut(), 0, 0, px(260.0, dpi), px(320.0, dpi), SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE) };
        let mr = wrect(m);
        let full_h = px(4.0 + 28.0 * 7.0 + 28.0 + 4.0, dpi) + 2 * one;
        t.check("tall note → regular menu below the bar", mr.bottom - mr.top == full_h && mr.top == wrect(h).top + px(38.0, dpi) - one, format!("h {} (want {full_h}) top {}", mr.bottom - mr.top, mr.top - wrect(h).top));
        unsafe { SendMessageW(h, WM_APP_MENU, Act::Color(3).code(), 0) };
        t.check("colour swatch recolours and keeps the menu open", app::note(id).map(|n| n.color) == Some(3) && unsafe { IsWindow(m) } != 0, String::new());
        let before = app::band();
        unsafe { SendMessageW(h, WM_APP_MENU, Act::Band.code(), 0) };
        t.check("띠 모양 cycles and keeps the menu open", app::band() == before.next() && unsafe { IsWindow(m) } != 0, format!("{:?} → {:?}", before, app::band()));
        while app::band() != Band::Thin {
            app::cycle_band();
        }
        unsafe { DestroyWindow(m) };
        pump();
        t.check("menu close clears the note's menu handle", s.menu.is_null(), String::new());
    }

    // ---- lock (F-33 visuals) and full screen (F-29)
    unsafe { SendMessageW(h, WM_APP_MENU, Act::Lock.code(), 0) };
    let ro = unsafe { GetWindowLongW(s.body, GWL_STYLE) } as u32 & ES_READONLY as u32 != 0;
    t.check("잠금: model locked + body read-only", app::note(id).map(|n| n.locked) == Some(true) && ro, String::new());
    unsafe { SendMessageW(h, WM_APP_MENU, Act::Lock.code(), 0) };
    note::FORCE_FULL.with(|f| f.set(true));
    unsafe { SendMessageW(h, WM_SIZE, 0, 0) };
    let br = wrect(s.body);
    let want_top = wrect(h).top + px(38.0, dpi) + px(32.0, dpi) + px(1.0, dpi).max(1);
    t.check("전체화면: formatting row pushes the body down", br.top == want_top, format!("body top {} want {}", br.top - wrect(h).top, want_top - wrect(h).top));
    let n = app::note(id).unwrap();
    let l = note::layout(px(260.0, dpi), dpi, &n, true, true);
    t.check("전체화면: tools = pin, new, ⋯, restore, ×", l.tools.len() == 5 && l.tools[3].0 == note::Tool::Restore, format!("{:?}", l.tools.iter().map(|t| t.0).collect::<Vec<_>>()));
    note::FORCE_FULL.with(|f| f.set(false));
    unsafe { SendMessageW(h, WM_SIZE, 0, 0) };

    // ---- 항상 위 (F-12): the pin tool makes the window topmost and back
    let topmost = |h: HWND| unsafe { GetWindowLongW(h, GWL_EXSTYLE) } as u32 & WS_EX_TOPMOST != 0;
    note::click(s, note::Part::Tool(note::Tool::Pin));
    let pinned = app::note(id).map(|n| n.pinned) == Some(true) && topmost(h);
    note::click(s, note::Part::Tool(note::Tool::Pin));
    t.check("pin → topmost, again → normal", pinned && app::note(id).map(|n| n.pinned) == Some(false) && !topmost(h), String::new());

    // ---- 글자 크기 (F-13): Ctrl+wheel steps clamp to 10–32
    let fs = |id| app::note(id).map(|n| n.font_size).unwrap_or(0);
    unsafe { SendMessageW(h, note::WM_APP_FONT, 100, 0) };
    let hi = fs(id);
    unsafe { SendMessageW(h, note::WM_APP_FONT, (-100isize) as usize, 0) };
    let lo = fs(id);
    unsafe { SendMessageW(h, note::WM_APP_FONT, (g::FONT_DEFAULT as isize - lo as isize) as usize, 0) };
    t.check("font size clamps to 10–32", hi == g::FONT_MAX && lo == g::FONT_MIN && fs(id) == g::FONT_DEFAULT, format!("max {hi} min {lo}"));

    // ---- 제목 수정 (F-03 UI): typing renames live, Esc ends the edit
    unsafe { SendMessageW(h, WM_APP_MENU, Act::Rename.code(), 0) };
    let e = s.title_edit;
    let started = !e.is_null();
    if started {
        unsafe {
            SetWindowTextW(e, crate::win::wide("새 제목").as_ptr()); // EN_CHANGE → model
            SendMessageW(e, WM_KEYDOWN, VK_ESCAPE as usize, 0);
        }
        pump();
    }
    t.check("rename: edit opens, text saved live, Esc closes", started && app::note(id).map(|n| n.title.clone()).as_deref() == Some("새 제목") && s.title_edit.is_null(), String::new());

    // ---- 색상 (F-08): new notes take the palette in turn
    let c1 = app::add("색 1", "", None, 0);
    let c2 = app::add("색 2", "", None, 0);
    let (k1, k2) = (app::note(c1).map(|n| n.color).unwrap_or(9), app::note(c2).map(|n| n.color).unwrap_or(9));
    app::delete_note(c1);
    app::delete_note(c2);
    t.check("new notes rotate through the 6 colours", k2 == (k1 + 1) % 6, format!("{k1} → {k2}"));

    // ---- delete flow (F-11)
    note::request_delete(h, id);
    pump();
    t.check("삭제 on a note with content shows the card", confirm::is_showing(h), String::new());
    let card = unsafe { FindWindowExW(h, std::ptr::null_mut(), crate::win::wide("GM_Confirm").as_ptr(), std::ptr::null()) };
    unsafe { SendMessageW(card, WM_KEYDOWN, VK_ESCAPE as usize, 0) };
    pump();
    t.check("Esc = 유지 (note kept, card gone)", app::note(id).is_some() && !confirm::is_showing(h), String::new());
    note::request_delete(h, id);
    let card = unsafe { FindWindowExW(h, std::ptr::null_mut(), crate::win::wide("GM_Confirm").as_ptr(), std::ptr::null()) };
    unsafe {
        SendMessageW(card, WM_KEYDOWN, VK_TAB as usize, 0);
        SendMessageW(card, WM_KEYDOWN, VK_RETURN as usize, 0);
    }
    pump();
    t.check("Tab → 삭제, Enter deletes the note", app::note(id).is_none() && unsafe { IsWindow(h) } == 0, String::new());
    let empty = app::add("", "", None, 0);
    app::open_note(empty, Some((-15000, -14000)), false);
    let eh = app::window_of(empty).unwrap();
    note::request_delete(eh, empty);
    pump();
    t.check("empty note is deleted without the card", app::note(empty).is_none(), String::new());
    let keep = app::add("남는 메모", "내용", None, 0);
    app::open_note(keep, Some((-15000, -14000)), false);
    app::close_note(keep);
    t.check("× keeps a note with content (window closed, still listed)", app::note(keep).map(|n| !n.open) == Some(true) && app::window_of(keep).is_none(), String::new());
    let gone = app::add("", "", None, 0);
    app::open_note(gone, Some((-15000, -14000)), false);
    app::close_note(gone);
    t.check("× removes an empty note", app::note(gone).is_none(), String::new());

    // ---- list (F-15)
    app::add("회의 메모", "화요일 3시", Some(4), 7_200_000);
    list::open(false);
    pump();
    let (items, total) = list::items("");
    t.check("list shows every note, newest edit first", items.len() == total && items.windows(2).all(|p| p[0].updated_at >= p[1].updated_at), format!("{} rows", items.len()));
    t.check("search filters title + text, case-insensitive", list::items("회의").0.len() == 1 && list::items("화요일").0.len() == 1, String::new());
    t.check("no match → empty result, total kept", list::items("zzzz").0.is_empty() && list::items("zzzz").1 == total, String::new());
    t.check("ago(): 방금 / n분 전 / n시간 전 / 어제", list::ago(app::now_ms()) == "방금" && list::ago(app::now_ms() - 180_000) == "3분 전" && list::ago(app::now_ms() - 7_200_000) == "2시간 전" && list::ago(app::now_ms() - 100_000_000) == "어제", String::new());

    // ---- theme switch repaints every window without error
    app::set_theme(Theme::Light);
    pump();
    app::set_theme(Theme::Dark);
    pump();
    t.check("theme switch round trip", app::theme() == Theme::Dark, String::new());

    // ---- leak: 20 rounds × 15 notes with menu + card opened and closed
    let mut base = metrics::sample();
    let mut trace: Vec<(u32, u32, u32)> = Vec::new();
    for round in 0..22 {
        let mut ids = Vec::new();
        for k in 0..15 {
            let id = app::add(&format!("leak {k}"), "x", None, 0);
            app::open_note(id, Some((-15000 + k * 20, -13000)), false);
            ids.push(id);
        }
        pump();
        for (k, &id) in ids.iter().enumerate() {
            let h = app::window_of(id).unwrap();
            let s = unsafe { note::state_of(h).unwrap() };
            note::open_menu(s);
            pump();
            if !s.menu.is_null() {
                unsafe { DestroyWindow(s.menu) };
            }
            if k % 3 == 0 {
                note::request_delete(h, id);
                let c = unsafe { FindWindowExW(h, std::ptr::null_mut(), crate::win::wide("GM_Confirm").as_ptr(), std::ptr::null()) };
                unsafe { SendMessageW(c, WM_KEYDOWN, VK_ESCAPE as usize, 0) };
            }
            pump();
        }
        for id in ids {
            app::delete_note(id);
        }
        pump();
        let m = metrics::sample();
        trace.push((m.gdi, m.user, m.handles));
        if round == 1 {
            base = m; // two warm-up rounds, then 20 measured rounds
        }
    }
    let end = metrics::sample();
    t.check(
        "no GDI / USER / handle growth over 20 rounds × 15 notes with menus and cards",
        (end.gdi as i64 - base.gdi as i64).abs() <= 2 && (end.user as i64 - base.user as i64).abs() <= 2 && (end.handles as i64 - base.handles as i64).abs() <= 4,
        format!("ΔGDI {} ΔUSER {} Δhandles {} Δprivate {:.2} MB; per round (gdi,user,handles) {:?}", end.gdi as i64 - base.gdi as i64, end.user as i64 - base.user as i64, end.handles as i64 - base.handles as i64, end.private_mb - base.private_mb, trace),
    );
    list::close();
    pump();
    let line = Json::default()
        .s("mode", "selftest")
        .n("dpi", dpi as f64)
        .n("pass", t.pass as f64)
        .n("fail", t.fail as f64)
        .sample("start", start)
        .sample("end", metrics::sample())
        .raw("checks", format!("[{}]", t.out.join(",")))
        .finish();
    metrics::append(out, &line);
    let _ = FillRect;
}
