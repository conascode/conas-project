//! `--mode p2check [--in CASES] --out FILE`: P2 exit checks on a real RichEdit body
//! (off-screen, never focused; the clipboard is never touched — pastes are fed to the
//! routing as in-process data objects).
//!
//! * round trip: every differential case (bench/rich/diff.py) → Doc → RichEdit → Doc,
//!   normal forms equal (report §6.4 "정규형 왕복")
//! * save cost: `rich::save` + HTML + text for 1k … 200k characters
//! * v1.1 tests/note_ui_test.html checks 1–8 (paste, picture, drop, lock) and the rest of
//!   F-30 / F-32 / F-33 that a test can drive without a keyboard

use crate::app;
use crate::body::{self, Fmt};
use crate::images;
use crate::metrics::{self, Json};
use crate::note;
use crate::rich::{self, Doc, Item, Para, B};
use crate::richedit;
use std::time::Instant;
use windows_sys::Win32::Foundation::{HWND, POINT};
use windows_sys::Win32::UI::Controls::{EM_SETSEL, EM_UNDO};
const EM_REDO: u32 = 0x0454; // WM_USER + 84 (RichEdit)
use windows_sys::Win32::UI::WindowsAndMessaging::*;

struct T {
    out: Vec<String>,
    pass: u32,
    fail: u32,
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

fn caret_end(h: HWND) {
    unsafe {
        let n = GetWindowTextLengthW(h) as usize;
        SendMessageW(h, EM_SETSEL, n, n as isize);
    }
}

fn stored(h: HWND) -> (String, String) {
    rich::stored(&richedit::save(h))
}

fn sample_doc(chars: usize) -> Doc {
    let words = ["가나다", "회의", "메모", "hello", "World", "123", "장보기", "우유"];
    let mut doc = Doc::default();
    let mut n = 0;
    let mut k = 0;
    while n < chars {
        let mut line = Vec::new();
        for j in 0..10 {
            let w = words[(k + j) % words.len()];
            let bits = if (k + j) % 7 == 0 { B } else { 0 };
            line.push(Item::Text(format!("{w} "), bits));
            n += w.chars().count() + 1;
        }
        doc.paras.push(Para { bullet: k % 20 == 5, lines: vec![line] });
        k += 1;
    }
    doc
}

pub fn run(input: &str, out: &str) {
    note::register_class();
    let mut t = T { out: Vec::new(), pass: 0, fail: 0 };
    let id = app::add("P2", "", Some(0), 0);
    app::open_note(id, Some((-15000, -15000)), false);
    pump();
    let hw = app::window_of(id).unwrap();
    let h = unsafe { note::state_of(hw).unwrap().body };
    unsafe { SetWindowPos(hw, std::ptr::null_mut(), 0, 0, 600, 700, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE) };
    pump();

    // ---- round trip through RichEdit
    let mut rt = (0u32, 0u32);
    let mut bad = Vec::new();
    let (mut load_ms, mut save_ms) = (0.0f64, 0.0f64);
    if !input.is_empty() {
        let src = std::fs::read_to_string(input).unwrap_or_default();
        for (k, html) in src.split('\u{1e}').enumerate() {
            let doc = Doc::from_tree(&rich::sanitize(html));
            let t0 = Instant::now();
            richedit::load(h, &doc);
            let t1 = Instant::now();
            let back = richedit::save(h);
            load_ms += (t1 - t0).as_secs_f64() * 1000.0;
            save_ms += t1.elapsed().as_secs_f64() * 1000.0;
            // the one-pass save writes exactly what the tree path writes
            let tree = back.to_tree();
            let (html_fast, text_fast) = rich::stored(&back);
            let same_save = html_fast == rich::serialize(&tree) && text_fast == rich::plain(&tree);
            if back.normal() == doc.normal() && same_save {
                rt.0 += 1;
            } else {
                rt.1 += 1;
                if bad.len() < 20 {
                    bad.push(Json::default().n("case", k as f64).s("html", html).s("want", &doc.to_html()).s("got", &back.to_html()).finish());
                }
            }
        }
        t.check("RichEdit round trip keeps the normal form", rt.1 == 0, format!("{} / {}", rt.0, rt.0 + rt.1));
    }

    // ---- save cost (report §9.3: serialization off the typing path, ≤ throttle)
    let mut cost = Vec::new();
    for chars in [1_000usize, 10_000, 100_000, 200_000] {
        let doc = sample_doc(chars);
        let t0 = Instant::now();
        richedit::load(h, &doc);
        let lm = t0.elapsed().as_secs_f64() * 1000.0;
        let mut runs = Vec::new();
        for _ in 0..3 {
            let t1 = Instant::now();
            let (html, _) = stored(h);
            runs.push((t1.elapsed().as_secs_f64() * 1000.0, html.len()));
        }
        runs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        // where the time goes: stream-out, RTF read, HTML + text
        let t2 = Instant::now();
        let rtf = richedit::stream_rtf(h);
        let t3 = Instant::now();
        let d2 = crate::rtfread::read(&rtf);
        let t4 = Instant::now();
        let _ = rich::stored(&d2);
        let t5 = Instant::now();
        let ms = |a: Instant, b: Instant| (b - a).as_secs_f64() * 1000.0;
        cost.push(format!("{{\"chars\":{chars},\"stream_ms\":{:.1},\"read_ms\":{:.1},\"html_text_ms\":{:.1},\"rtf_kb\":{:.0}}}", ms(t2, t3), ms(t3, t4), ms(t4, t5), rtf.len() as f64 / 1024.0));
        cost.push(format!("{{\"chars\":{chars},\"load_ms\":{lm:.1},\"save_ms\":{:.1},\"html_kb\":{:.0}}}", runs[1].0, runs[1].1 as f64 / 1024.0));
    }

    // ---- input latency (report §13.1: p95 ≤ 4 ms): WM_CHAR → RichEdit → EN_CHANGE → note,
    // plus the throttled save when its timer comes due; 15 ms between keys. Off-screen, so
    // painting is not included.
    let mut latency = Vec::new();
    for (label, start) in [("empty", Doc::default()), ("10k", sample_doc(10_000))] {
        richedit::load(h, &start);
        caret_end(h);
        let typed: Vec<u16> = "가나다라 마바사 abc 한글 입력 test 123 ".encode_utf16().collect();
        let mut times: Vec<f64> = Vec::with_capacity(400);
        for k in 0..400 {
            let ch = if k % 40 == 39 { 0x0D } else { typed[k % typed.len()] };
            std::thread::sleep(std::time::Duration::from_millis(15));
            let t0 = Instant::now();
            unsafe {
                SendMessageW(h, WM_CHAR, ch as usize, 1);
                // painting is measured on its own below: an off-screen window cannot scroll
                // its pixels, so every auto-scroll would repaint the whole body here
                windows_sys::Win32::Graphics::Gdi::ValidateRect(h, std::ptr::null());
            }
            pump(); // due save timers run here, as they would between keystrokes
            times.push(t0.elapsed().as_secs_f64() * 1000.0);
        }
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let q = |p: f64| times[((times.len() as f64 * p) as usize).min(times.len() - 1)];
        latency.push(format!("{{\"doc\":\"{label}\",\"p50_ms\":{:.3},\"p95_ms\":{:.3},\"p99_ms\":{:.3},\"max_ms\":{:.3}}}", q(0.50), q(0.95), q(0.99), times[times.len() - 1]));
        t.check(&format!("input latency p95 ≤ 4 ms ({label})"), q(0.95) <= 4.0, format!("p95 {:.2} ms", q(0.95)));
    }
    // the typed text reached the saved note
    // full repaint off-screen (resize, scroll, picture replaced: body::paint_buffered); a
    // small repaint such as a typed character stays RichEdit's own
    let full_paint = |h: HWND| -> (f64, f64) {
        unsafe {
            use windows_sys::Win32::Graphics::Gdi::*;
            let mut rc = windows_sys::Win32::Foundation::RECT { left: 0, top: 0, right: 0, bottom: 0 };
            GetClientRect(h, &mut rc);
            let screen = GetDC(std::ptr::null_mut());
            let mem = CreateCompatibleDC(screen);
            let bmp = CreateCompatibleBitmap(screen, rc.right.max(1), rc.bottom.max(1));
            let old = SelectObject(mem, bmp);
            let mut v = Vec::new();
            for _ in 0..21 {
                let t0 = Instant::now();
                body::paint_to(h, mem, false);
                GdiFlush();
                v.push(t0.elapsed().as_secs_f64() * 1000.0);
            }
            SelectObject(mem, old);
            DeleteObject(bmp);
            DeleteDC(mem);
            ReleaseDC(std::ptr::null_mut(), screen);
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            (v[10], v[20])
        }
    };
    let mut paints = Vec::new();
    for (w, hh) in [(600, 700), (1600, 1000)] {
        unsafe { SetWindowPos(hw, std::ptr::null_mut(), 0, 0, w, hh, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE) };
        pump();
        let (p50, max) = full_paint(h);
        paints.push(format!("{{\"doc\":\"10k\",\"window\":[{w},{hh}],\"p50_ms\":{p50:.3},\"max_ms\":{max:.3}}}"));
        t.check(&format!("full repaint ≤ 8 ms (10k, {w}×{hh})"), p50 <= 8.0, format!("p50 {p50:.2} ms, max {max:.2} ms"));
    }
    unsafe { SetWindowPos(hw, std::ptr::null_mut(), 0, 0, 600, 700, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE) };
    pump();
    let saved = app::note(id).map(|n| n.text).unwrap_or_default();
    t.check("typed text is saved (throttled save)", saved.contains("가나다라 마바사"), format!("{} chars", saved.chars().count()));

    // ---- v1.1 note_ui_test.html 1: paste from a web page / Word keeps supported formatting
    richedit::load(h, &Doc::default());
    let web = r#"<p><span style="font-weight:700">굵게</span> <a href="https://x">링크</a><script>evil()</script> <span style="text-decoration:line-through">취소</span> <em>기울임</em></p><ol><li>하나</li></ol><img src="https://example.com/x.png" onerror="evil()">"#;
    let obj = richedit::test_data(vec![("HTML Format", richedit::html_bytes(web)), ("text", richedit::text_bytes("굵게 링크 취소 기울임 하나"))]);
    caret_end(h);
    richedit::paste_for_test(h, &obj, false);
    let (html, text) = stored(h);
    t.check("1 web paste keeps bold", html.contains("<b>굵게</b>"), html.clone());
    t.check("1 web paste keeps strike / italic / list", html.contains("<s>취소</s>") && html.contains("<i>기울임</i>") && html.contains("<ul><li>하나</li></ul>"), html.clone());
    t.check("1 web paste drops link / script / foreign picture", !html.contains("<a") && !html.contains("script") && !html.contains("example.com") && !html.contains("onerror") && html.contains("링크"), html.clone());
    t.check("1 plain-text copy saved", text.contains("굵게") && !text.contains('<'), text.clone());
    // one undo step takes the whole paste back
    unsafe { SendMessageW(h, EM_UNDO, 0, 0) };
    t.check("paste is one undo step", stored(h).1.is_empty(), stored(h).1);
    unsafe { SendMessageW(h, EM_REDO, 0, 0) };

    // ---- 2: Ctrl+Shift+V pastes text without formatting
    let before = stored(h).0.matches("<b>").count();
    caret_end(h);
    let obj = richedit::test_data(vec![("HTML Format", richedit::html_bytes("<b>평문만</b>")), ("text", richedit::text_bytes("평문만"))]);
    richedit::paste_for_test(h, &obj, true);
    let (html, _) = stored(h);
    t.check("2 Ctrl+Shift+V pastes text without formatting", html.contains("평문만") && html.matches("<b>").count() == before, html.clone());

    // ---- 3: paste a picture (screenshot / image file)
    let png = images::sample_png(64, 48);
    t.check("test picture encodes and decodes (WIC)", !png.is_empty() && images::pixel_size(&png) == Some((64, 48)), format!("{} bytes, size {:?}, sniff {:?}", png.len(), images::pixel_size(&png), images::sniff(&png)));
    let n0 = stored(h).0.matches("data-gm=").count();
    caret_end(h);
    richedit::paste_for_test(h, &richedit::test_data(vec![("PNG", png.clone())]), false);
    let (html, _) = stored(h);
    let names: Vec<String> = html.split("data-gm=\"").skip(1).filter_map(|s| s.split('"').next()).map(String::from).collect();
    let last = names.last().cloned().unwrap_or_default();
    t.check("3 picture paste stores the bytes", images::get(&last).is_some_and(|b| *b == png), format!("{} pictures", names.len()));
    t.check("3 saved html keeps the picture name, not a URL", html.matches("data-gm=").count() == n0 + 1 && rich::is_image_name(&last) && !html.contains("gmimg"), html.clone());
    let lines = richedit::save(h).normal();
    t.check("3 picture on a line of its own (v1.1 block)", lines.iter().any(|(_, l)| matches!(l.as_slice(), [Item::Img(n)] if *n == last)), String::new());

    // ---- 4: a picture copied from a browser (bitmap + <img src=web>)
    caret_end(h);
    let obj = richedit::test_data(vec![("PNG", png.clone()), ("HTML Format", richedit::html_bytes(r#"<img src="https://example.com/cat.png">"#))]);
    richedit::paste_for_test(h, &obj, false);
    t.check("4 browser-copied picture is added", stored(h).0.matches("data-gm=").count() == n0 + 2, String::new());

    // ---- 5: drop a picture file / 7: a non-picture file is ignored
    let dir = std::env::temp_dir().join(format!("gm-p2check-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let pic = dir.join("a.png");
    let exe = dir.join("a.exe");
    let _ = std::fs::write(&pic, &png);
    let _ = std::fs::write(&exe, b"MZ\x90\0not a picture");
    let hdrop = |p: &std::path::Path| -> Vec<u8> {
        // DROPFILES { pFiles: 20, pt, fNC, fWide: 1 } + path\0\0 (UTF-16)
        let mut v = Vec::new();
        v.extend_from_slice(&20u32.to_le_bytes());
        v.extend_from_slice(&[0u8; 12]);
        v.extend_from_slice(&1u32.to_le_bytes());
        for u in p.to_string_lossy().encode_utf16().chain([0, 0]) {
            v.extend_from_slice(&u.to_le_bytes());
        }
        v
    };
    caret_end(h);
    let mut f: u16 = 0;
    let _ = richedit::route(h, &richedit::test_data(vec![("hdrop", hdrop(&pic))]), &mut f, 1 /* RECO_DROP */, true, false);
    t.check("5 dropped picture is added", stored(h).0.matches("data-gm=").count() == n0 + 3, String::new());
    let snapshot = stored(h);
    let _ = richedit::route(h, &richedit::test_data(vec![("hdrop", hdrop(&exe))]), &mut f, 1, true, false);
    t.check("7 non-picture file ignored", stored(h) == snapshot, String::new());
    let _ = std::fs::remove_dir_all(&dir);

    // pictures keep their height: exact line spacing once squashed them to a text line
    // (user report 2026-11-14). The test picture is 64 × 48 CSS px once decoded.
    for _ in 0..80 {
        pump();
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let need = (48.0 * unsafe { windows_sys::Win32::UI::HiDpi::GetDpiForWindow(h) } as f64 / 96.0).floor() as i32;
    let heights = richedit::picture_line_heights(h);
    t.check("picture lines are as tall as the picture", !heights.is_empty() && heights.iter().all(|&y| y >= need), format!("{heights:?} px, picture {need} px"));

    // ---- 8: lock — paste does nothing, the body is read-only
    unsafe { SendMessageW(hw, note::WM_APP_MENU, crate::menu::Act::Lock.code(), 0) };
    pump();
    let snapshot = stored(h);
    caret_end(h);
    let obj = richedit::test_data(vec![("text", richedit::text_bytes("잠금 중 붙여넣기")), ("PNG", png.clone())]);
    richedit::paste_for_test(h, &obj, false);
    t.check("8 locked: paste does nothing", stored(h) == snapshot, String::new());
    t.check("8 locked: body is read-only", unsafe { GetWindowLongW(h, GWL_STYLE) } as u32 & ES_READONLY as u32 != 0, String::new());
    unsafe { SendMessageW(hw, note::WM_APP_MENU, crate::menu::Act::Lock.code(), 0) };
    pump();

    // ---- copy hands out v1.1-clean HTML and Rich.plain text
    richedit::load(h, &Doc::from_tree(&rich::sanitize("<b>굵게</b> 보통<ul><li>항목</li></ul>끝")));
    let (chtml, ctext) = richedit::copy_for_test(h, 0, 100);
    t.check("copy: HTML keeps b and bullets", chtml.contains("<b>굵게</b>") && chtml.contains("<ul><li>항목</li></ul>"), chtml.clone());
    t.check("copy: text is the plain copy", ctext == "굵게 보통\n항목\n끝", format!("{ctext:?}"));

    // ---- formatting row (F-30): B on a selection, bullets on a paragraph
    richedit::load(h, &Doc::load("", "가나다\n라마바"));
    unsafe { SendMessageW(h, EM_SETSEL, 0, 3) };
    body::toggle(h, Fmt::Bold);
    unsafe { SendMessageW(h, EM_SETSEL, 5, 5) };
    body::toggle(h, Fmt::Bullets);
    let (html, text) = stored(h);
    t.check("formatting row: bold and bullets are saved", html == "<b>가나다</b><ul><li>라마바</li></ul>", html.clone());
    t.check("formatting row: text copy unchanged", text == "가나다\n라마바", format!("{text:?}"));

    // ---- v1.0 notes (plain text only) load line by line and keep their text
    richedit::load(h, &Doc::load("", "하나\n\n셋\n"));
    t.check("v1.0 plain-text note round trip", stored(h).1 == "하나\n\n셋", format!("{:?}", stored(h).1));

    // ---- an unknown picture keeps its name (placeholder until P3 brings the files)
    let ghost = "0123abcd-0123-4567-89ab-0123456789ab.png";
    richedit::load(h, &Doc::load(&format!("앞<img data-gm=\"{ghost}\">뒤"), ""));
    t.check("missing picture keeps its name", stored(h).0.contains(ghost), stored(h).0);

    // ---- mouse wheel (user report 2026-11-14): exactly 100 CSS px a notch (system default
    // 3 lines), at once, nothing after the wheel stops, tall pictures not skipped
    let dpi = unsafe { windows_sys::Win32::UI::HiDpi::GetDpiForWindow(h) } as f64;
    let mut lines: u32 = 3;
    unsafe { SystemParametersInfoW(SPI_GETWHEELSCROLLLINES, 0, &mut lines as *mut u32 as *mut core::ffi::c_void, 0) };
    let notch = if lines == u32::MAX { 0 } else { (lines as f64 * 100.0 / 3.0 * dpi / 96.0) as i32 };
    let scroll_y = || unsafe {
        let mut p = POINT { x: 0, y: 0 };
        SendMessageW(h, 0x04DD /* EM_GETSCROLLPOS */, 0, &mut p as *mut _ as isize);
        p.y
    };
    let set_y = |y: i32| unsafe {
        let p = POINT { x: 0, y };
        SendMessageW(h, 0x04DE /* EM_SETSCROLLPOS */, 0, &p as *const _ as isize);
    };
    let turn = |delta: i32| unsafe { SendMessageW(h, WM_MOUSEWHEEL, ((delta as u32 as usize) & 0xFFFF) << 16, 0) };
    let settle = || {
        for _ in 0..16 {
            pump();
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
    };
    richedit::load(h, &sample_doc(200_000));
    set_y(0);
    turn(-120);
    let y1 = scroll_y();
    settle();
    let y1b = scroll_y();
    t.check("wheel: one notch = 100 CSS px × lines/3", notch == 0 || y1 == notch, format!("{y1} px, expected {notch}"));
    t.check("wheel: nothing moves after the wheel stops", y1b == y1, format!("{y1} → {y1b} px after 400 ms"));
    turn(120);
    turn(120);
    t.check("wheel: stops at the top", scroll_y() == 0, format!("{} px", scroll_y()));
    for _ in 0..12 {
        turn(-10); // a touchpad's small steps add up to one notch
    }
    t.check("wheel: small steps add up exactly", (scroll_y() - notch).abs() <= 1, format!("{} px, expected {notch}", scroll_y()));
    set_y(100_000);
    let deep = scroll_y();
    turn(-120);
    t.check("wheel: positions past 65,535 px (200,000-char note)", deep == 100_000 && scroll_y() == deep + notch, format!("{deep} → {} px", scroll_y()));
    let tall = images::sample_png(64, 600);
    let name = images::add(tall).unwrap_or_default();
    richedit::load(h, &Doc::load(&format!("위<br><img data-gm=\"{name}\"><br>아래"), ""));
    for _ in 0..80 {
        pump();
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    set_y(0);
    turn(-120);
    t.check("wheel: a tall picture is scrolled through, not skipped", notch == 0 || scroll_y() == notch, format!("{} px, expected {notch}", scroll_y()));

    // ---- the v1.1 scroll bar (8 px, square thumb in the rule colour, own window over the body)
    let bar = body::bar_for_test(h);
    let bar_w = |b: HWND| unsafe {
        let mut r = windows_sys::Win32::Foundation::RECT { left: 0, top: 0, right: 0, bottom: 0 };
        GetClientRect(b, &mut r);
        (r.right, r.bottom)
    };
    richedit::load(h, &Doc::default());
    pump();
    let short_w = richedit::text_css_width(h);
    t.check("scroll bar: hidden when the note fits", unsafe { IsWindowVisible(bar) } == 0, String::new());
    richedit::load(h, &sample_doc(10_000));
    set_y(0);
    pump();
    let long_w = richedit::text_css_width(h);
    let (bw, bh) = bar_w(bar);
    // 8 CSS px from the window edge; the last pixel lies under the 1 px border (as in v1.1)
    let edge_ok = unsafe {
        let (mut nr, mut br) = (std::mem::zeroed::<windows_sys::Win32::Foundation::RECT>(), std::mem::zeroed::<windows_sys::Win32::Foundation::RECT>());
        GetWindowRect(hw, &mut nr);
        GetWindowRect(bar, &mut br);
        br.right == nr.right - 1
    };
    t.check("scroll bar: shown when it scrolls, 8 CSS px to the window edge", unsafe { IsWindowVisible(bar) } != 0 && bw == (8.0 * dpi / 96.0).round() as i32 - 1 && edge_ok, format!("{bw}×{bh} px, ends at the border: {edge_ok}"));
    // nothing of the system scroll bar may show: it must lie wholly outside the body's
    // window region (user report 2026-11-16: a 2 px sliver of it showed)
    let (sys_left, rgn_right) = unsafe {
        let mut sbi: SCROLLBARINFO = std::mem::zeroed();
        sbi.cbSize = std::mem::size_of::<SCROLLBARINFO>() as u32;
        GetScrollBarInfo(h, OBJID_VSCROLL, &mut sbi);
        let mut wr = std::mem::zeroed::<windows_sys::Win32::Foundation::RECT>();
        GetWindowRect(h, &mut wr);
        let mut rb = std::mem::zeroed::<windows_sys::Win32::Foundation::RECT>();
        windows_sys::Win32::Graphics::Gdi::GetWindowRgnBox(h, &mut rb);
        (sbi.rcScrollBar.left - wr.left, rb.right)
    };
    t.check("scroll bar: the system bar is wholly clipped away", sys_left >= rgn_right, format!("system bar from {sys_left} px, body region to {rgn_right} px"));
    t.check("scroll bar: takes 8 CSS px from the text width (v1.1)", (short_w - long_w - 8.0).abs() < 0.6, format!("{short_w:.1} → {long_w:.1} CSS px"));
    let lp = |x: i32, y: i32| (((y as u32) << 16) | (x as u32 & 0xFFFF)) as isize;
    let max_y = {
        let mut si: SCROLLINFO = unsafe { std::mem::zeroed() };
        si.cbSize = std::mem::size_of::<SCROLLINFO>() as u32;
        si.fMask = SIF_RANGE | SIF_PAGE;
        unsafe { GetScrollInfo(h, SB_VERT as i32, &mut si) };
        si.nMax - si.nPage as i32 + 1
    };
    // thumb at the top: grab it near its top edge, pull to the bottom of the track
    unsafe {
        SendMessageW(bar, WM_LBUTTONDOWN, 1, lp(bw / 2, 3));
        SendMessageW(bar, WM_MOUSEMOVE, 1, lp(bw / 2, bh + 50));
        SendMessageW(bar, WM_LBUTTONUP, 0, lp(bw / 2, bh + 50));
    }
    t.check("scroll bar: dragging the thumb to the end scrolls to the end", scroll_y() == max_y, format!("{} / {max_y} px", scroll_y()));
    unsafe {
        SendMessageW(bar, WM_LBUTTONDOWN, 1, lp(bw / 2, bh - 3));
        SendMessageW(bar, WM_MOUSEMOVE, 1, lp(bw / 2, -50));
        SendMessageW(bar, WM_LBUTTONUP, 0, lp(bw / 2, -50));
    }
    t.check("scroll bar: dragging back to the top", scroll_y() == 0, format!("{} px", scroll_y()));
    // resize bands over the body and the scroll bar (user report 2026-11-16: only 1–2 px at
    // the left / bottom / right could be grabbed): the children step aside, the note resizes
    {
        let mut nr = unsafe { std::mem::zeroed::<windows_sys::Win32::Foundation::RECT>() };
        unsafe { GetWindowRect(hw, &mut nr) };
        let sp = |x: i32, y: i32| ((y as u32 as isize) << 16) | (x as u32 & 0xFFFF) as isize;
        let ask = |w: HWND, x: i32, y: i32| unsafe { SendMessageW(w, WM_NCHITTEST, 0, sp(x, y)) };
        let d = |v: f32| (v * dpi as f32 / 96.0).round() as i32;
        let midy = (nr.top + nr.bottom) / 2;
        let midx = (nr.left + nr.right) / 2;
        let cases: [(&str, HWND, i32, i32, isize, isize); 7] = [
            ("left band over the body", h, nr.left + d(4.0), midy, -1, HTLEFT as isize),
            ("bottom band over the body", h, midx, nr.bottom - 1 - d(4.0), -1, HTBOTTOM as isize),
            ("right edge over the scroll bar", bar, nr.right - 2, midy, -1, HTRIGHT as isize),
            ("bottom-right corner", bar, nr.right - 2, nr.bottom - 1 - d(10.0), -1, HTBOTTOMRIGHT as isize),
            ("bottom-left corner up the side", h, nr.left + d(2.0), nr.bottom - 1 - d(10.0), -1, HTBOTTOMLEFT as isize),
            ("scroll bar inside the 3 px", bar, nr.right - 1 - d(6.0), midy, HTCLIENT as isize, HTCLIENT as isize),
            ("text area inside the band", h, nr.left + d(8.0), midy, HTCLIENT as isize, HTCLIENT as isize),
        ];
        let mut bad = Vec::new();
        for (name, child, x, y, want_child, want_note) in cases {
            let (c, n) = (ask(child, x, y), ask(hw, x, y));
            if c != want_child || n != want_note {
                bad.push(format!("{name}: child {c} note {n}"));
            }
        }
        t.check("resize bands: 6 CSS px over the body, 3 along the scroll bar, 12 px corners", bad.is_empty(), bad.join("; "));
    }
    // dragging the thumb: every mouse move shows its frame at once (user report 2026-11-16:
    // the text moved in jumps), within one 60 Hz frame
    let mut step_ms = Vec::new();
    let mut left_over = 0;
    pump();
    unsafe {
        SendMessageW(bar, WM_LBUTTONDOWN, 1, lp(bw / 2, 3));
        for k in 1..=40 {
            let t0 = Instant::now();
            SendMessageW(bar, WM_MOUSEMOVE, 1, lp(bw / 2, 3 + k * 6));
            step_ms.push(t0.elapsed().as_secs_f64() * 1000.0);
            let mut r = std::mem::zeroed::<windows_sys::Win32::Foundation::RECT>();
            left_over += (windows_sys::Win32::Graphics::Gdi::GetUpdateRect(h, &mut r, 0) != 0) as i32;
            left_over += (windows_sys::Win32::Graphics::Gdi::GetUpdateRect(bar, &mut r, 0) != 0) as i32;
        }
        SendMessageW(bar, WM_LBUTTONUP, 0, lp(bw / 2, 3 + 40 * 6));
    }
    step_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    t.check("scroll bar: each drag step is painted at once", left_over == 0 && scroll_y() > 0, format!("{left_over} moves left something unpainted, scrolled to {} px", scroll_y()));
    t.check("scroll bar: a drag step takes ≤ 16 ms (with painting)", step_ms[20] <= 16.0, format!("p50 {:.2} ms, max {:.2} ms", step_ms[20], step_ms[39]));
    set_y(0);
    let page = {
        let mut si: SCROLLINFO = unsafe { std::mem::zeroed() };
        si.cbSize = std::mem::size_of::<SCROLLINFO>() as u32;
        si.fMask = SIF_PAGE;
        unsafe { GetScrollInfo(h, SB_VERT as i32, &mut si) };
        si.nPage as i32
    };
    unsafe { SendMessageW(bar, WM_LBUTTONDOWN, 1, lp(bw / 2, bh - 3)) };
    unsafe { SendMessageW(bar, WM_LBUTTONUP, 0, lp(bw / 2, bh - 3)) };
    t.check("scroll bar: a click below the thumb scrolls one page", (scroll_y() - page).abs() <= 2, format!("{} px, page {page}", scroll_y()));
    set_y(0);

    // ---- resizing with pictures (user report 2026-11-14): pictures follow the new width in
    // the same WM_SIZE (v1.1 max-width: 100%), not after a pause; the sharp copy follows
    let big = images::add(images::sample_png(900, 300)).unwrap_or_default();
    let small = images::add(images::sample_png(300, 200)).unwrap_or_default();
    richedit::load(h, &Doc::load(&format!("<img data-gm=\"{big}\"><br>글<br><img data-gm=\"{small}\">"), ""));
    for _ in 0..80 {
        pump();
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let before = stored(h);
    let scale = dpi / 96.0;
    let expect = |natural: f64| (natural.min(richedit::text_css_width(h)) * scale).round() as i32;
    let near = |got: &[i32], want: [i32; 2]| got.len() == 2 && (got[0] - want[0]).abs() <= 2 && (got[1] - want[1]).abs() <= 2;
    let w0 = richedit::picture_widths(h);
    t.check("resize: pictures shown at min(own width, text width)", near(&w0, [expect(900.0), expect(300.0)]), format!("{w0:?} px, expected {:?}", [expect(900.0), expect(300.0)]));
    let mut steps = Vec::new();
    let mut follow = true;
    for width in [450, 520, 900, 700, 380] {
        unsafe { SetWindowPos(hw, std::ptr::null_mut(), 0, 0, width, 700, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE) };
        // no message pumped: what the next paint would show
        let got = richedit::picture_widths(h);
        let want = [expect(900.0), expect(300.0)];
        follow &= near(&got, want);
        steps.push(format!("{width}: {got:?}/{want:?}"));
    }
    t.check("resize: pictures follow each size step at once", follow, steps.join(", "));
    t.check("resize: the body is not left frozen", richedit::freeze_balanced(h), String::new());
    t.check("resize: following pictures is not an edit", stored(h) == before && unsafe { SendMessageW(h, windows_sys::Win32::UI::Controls::EM_CANUNDO, 0, 0) } == 0, String::new());
    let note_text_w = richedit::text_css_width(h);
    for _ in 0..80 {
        pump();
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let settled = richedit::picture_widths(h);
    t.check("resize: sharp copies replace the quick ones once the size settles", near(&settled, [expect(900.0), expect(300.0)]) && !images::needs_sharp(hw, &big, note_text_w) && !images::needs_sharp(hw, &small, note_text_w), format!("{settled:?} px"));
    t.check("resize: still the same note after settling", stored(h) == before, String::new());
    // a pause in the middle of a drag must not swap in the sharp copies (they went soft →
    // sharp → soft while dragging slowly, which flashed); the drag's end does
    unsafe {
        SendMessageW(hw, WM_ENTERSIZEMOVE, 0, 0);
        SetWindowPos(hw, std::ptr::null_mut(), 0, 0, 640, 700, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
    }
    for _ in 0..24 {
        pump();
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let wpause = richedit::text_css_width(h);
    let soft_during = images::needs_sharp(hw, &big, wpause);
    unsafe { SendMessageW(hw, WM_EXITSIZEMOVE, 0, 0) };
    for _ in 0..60 {
        pump();
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let sharp_after = !images::needs_sharp(hw, &big, richedit::text_css_width(h));
    t.check("resize: no sharp swap during a 600 ms pause in a drag, sharp after it ends", soft_during && sharp_after, format!("soft while dragging: {soft_during}, sharp after: {sharp_after}"));
    // what one resize step paints (user report 2026-11-16: pictures flash while resizing):
    // the body must show one finished frame per step — no erase, no partial native paint
    {
        use std::sync::atomic::Ordering::Relaxed;
        let (mut er, mut na, mut bu) = (0, 0, 0);
        for k in 0..10 {
            let w = if k % 2 == 0 { 620 } else { 680 };
            body::ERASES.store(0, Relaxed);
            body::NATIVE_PAINTS.store(0, Relaxed);
            body::BUFFERED_PAINTS.store(0, Relaxed);
            unsafe {
                SetWindowPos(hw, std::ptr::null_mut(), 0, 0, w, 700, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
                windows_sys::Win32::Graphics::Gdi::UpdateWindow(hw);
                windows_sys::Win32::Graphics::Gdi::UpdateWindow(h);
            }
            er += body::ERASES.load(Relaxed);
            na += body::NATIVE_PAINTS.load(Relaxed);
            bu += body::BUFFERED_PAINTS.load(Relaxed);
        }
        t.check("resize: each step is one off-screen frame, nothing erased first", er == 0 && na == 0 && bu >= 10 && bu <= 10, format!("10 steps: {er} erases, {na} partial paints, {bu} frames"));
    }
    // one resize step with pictures: WM_SIZE (pictures follow) + the full repaint
    let mut step_ms = Vec::new();
    for k in 0..20 {
        let w = if k % 2 == 0 { 640 } else { 660 };
        let t0 = Instant::now();
        unsafe { SetWindowPos(hw, std::ptr::null_mut(), 0, 0, w, 700, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE) };
        let size_ms = t0.elapsed().as_secs_f64() * 1000.0;
        step_ms.push(size_ms + full_paint(h).0);
    }
    step_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    paints.push(format!("{{\"doc\":\"resize step, 2 pictures\",\"window\":[650,700],\"p50_ms\":{:.3},\"max_ms\":{:.3}}}", step_ms[10], step_ms[19]));
    t.check("resize step with pictures ≤ 16 ms (one frame)", step_ms[10] <= 16.0, format!("p50 {:.2} ms, max {:.2} ms", step_ms[10], step_ms[19]));
    for _ in 0..80 {
        pump();
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    unsafe { SetWindowPos(hw, std::ptr::null_mut(), 0, 0, 600, 700, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE) };
    pump();

    let line = Json::default()
        .s("mode", "p2check")
        .n("pass", t.pass as f64)
        .n("fail", t.fail as f64)
        .n("roundtrip_cases", (rt.0 + rt.1) as f64)
        .n("roundtrip_ok", rt.0 as f64)
        .n("roundtrip_load_ms_total", load_ms)
        .n("roundtrip_save_ms_total", save_ms)
        .raw("save_cost", format!("[{}]", cost.join(",")))
        .raw("input_latency", format!("[{}]", latency.join(",")))
        .raw("full_paint", format!("[{}]", paints.join(",")))
        .raw("roundtrip_mismatch", format!("[{}]", bad.join(",")))
        .raw("checks", format!("[{}]", t.out.join(",")))
        .finish();
    metrics::append(out, &line);
    unsafe { DestroyWindow(hw) };
}
