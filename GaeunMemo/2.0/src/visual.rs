//! Visual regression (report §14 시각 회귀):
//!   --mode shots --cases bench/visual/cases.tsv --outdir DIR   v2 renders → DIR/<case>.png
//!   --mode diff  --refdir A --outdir B                        compare B/*.png with A/*.png
//! Windows stay off-screen and are never activated; PrintWindow renders them.

use crate::app;
use crate::list;
use crate::menu::Act;
use crate::metrics::{self, Json};
use crate::note::{self, WM_APP_HOVER, WM_APP_MENU};
use crate::theme::{Band, Theme};
use crate::win::{px, wide};
use windows::core::{Interface, GUID, PCWSTR};
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::Storage::Xps::PrintWindow;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub struct Case {
    pub name: String,
    pub theme: Theme,
    pub band: Band,
    pub w: f32,
    pub h: f32,
    pub color: usize,
    pub states: Vec<String>,
    pub title: String,
    pub text: String,
}

pub fn load_cases(path: &str) -> Vec<Case> {
    let s = std::fs::read_to_string(path).unwrap_or_default();
    s.lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            if f.len() < 6 {
                return None;
            }
            let get = |i: usize| f.get(i).copied().unwrap_or("");
            Some(Case {
                name: get(0).into(),
                theme: if get(1) == "light" { Theme::Light } else { Theme::Dark },
                band: match get(2) {
                    "none" => Band::None,
                    "full" => Band::Full,
                    _ => Band::Thin,
                },
                w: get(3).parse().unwrap_or(260.0),
                h: get(4).parse().unwrap_or(260.0),
                color: get(5).parse().unwrap_or(0),
                states: get(6).split(',').filter(|x| !x.is_empty() && *x != "-").map(String::from).collect(),
                title: get(7).into(),
                text: get(8).replace("\\n", "\n"),
            })
        })
        .collect()
}

fn pump_for(ms: u64) {
    unsafe {
        let end = std::time::Instant::now() + std::time::Duration::from_millis(ms);
        loop {
            let mut m: MSG = std::mem::zeroed();
            while PeekMessageW(&mut m, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&m);
                DispatchMessageW(&m);
            }
            if std::time::Instant::now() >= end {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}

/// PrintWindow → top-down BGRX pixels.
pub fn capture(h: HWND) -> (i32, i32, Vec<u32>) {
    capture_with(h, 2)
}

/// flags: 2 = PW_RENDERFULLCONTENT (DWM), 0 = WM_PRINT (our WM_PRINTCLIENT code).
pub fn capture_with(h: HWND, flags: u32) -> (i32, i32, Vec<u32>) {
    unsafe {
        let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        GetWindowRect(h, &mut r);
        let (w, hh) = (r.right - r.left, r.bottom - r.top);
        let screen = GetDC(std::ptr::null_mut());
        let mem = CreateCompatibleDC(screen);
        let mut bi: BITMAPINFO = std::mem::zeroed();
        bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bi.bmiHeader.biWidth = w;
        bi.bmiHeader.biHeight = -hh;
        bi.bmiHeader.biPlanes = 1;
        bi.bmiHeader.biBitCount = 32;
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let dib = CreateDIBSection(mem, &bi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        let old = SelectObject(mem, dib);
        PrintWindow(h, mem, flags);
        GdiFlush();
        let px_ = std::slice::from_raw_parts(bits as *const u32, (w * hh) as usize).to_vec();
        SelectObject(mem, old);
        DeleteObject(dib);
        DeleteDC(mem);
        ReleaseDC(std::ptr::null_mut(), screen);
        (w, hh, px_)
    }
}

/// Pastes a captured popup at its position relative to `base`, clipped to the base window
/// (v1.1 draws its menus inside the window).
fn composite(base: HWND, img: &mut [u32], bw: i32, bh: i32, popup: HWND) {
    if popup.is_null() || unsafe { IsWindow(popup) } == 0 {
        return;
    }
    let (pw, ph, p) = capture_with(popup, 0);
    let mut br = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    let mut pr = br;
    unsafe {
        GetWindowRect(base, &mut br);
        GetWindowRect(popup, &mut pr);
    }
    let (ox, oy) = (pr.left - br.left, pr.top - br.top);
    for y in 0..ph {
        for x in 0..pw {
            let (tx, ty) = (ox + x, oy + y);
            if tx >= 0 && ty >= 0 && tx < bw && ty < bh {
                img[(ty * bw + tx) as usize] = p[(y * pw + x) as usize];
            }
        }
    }
}

fn wic() -> IWICImagingFactory {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).expect("WIC")
    }
}

pub fn save_png(f: &IWICImagingFactory, path: &str, w: i32, h: i32, px_: &[u32]) -> windows::core::Result<()> {
    unsafe {
        let bytes = std::slice::from_raw_parts(px_.as_ptr() as *const u8, px_.len() * 4);
        let bmp = f.CreateBitmapFromMemory(w as u32, h as u32, &GUID_WICPixelFormat32bppBGR, (w * 4) as u32, bytes)?;
        let stream = f.CreateStream()?;
        let wp: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        stream.InitializeFromFilename(PCWSTR(wp.as_ptr()), 0x4000_0000 /* GENERIC_WRITE */)?;
        let enc = f.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())?;
        enc.Initialize(&stream, WICBitmapEncoderNoCache)?;
        let mut fe = None;
        let mut bag = None;
        enc.CreateNewFrame(&mut fe, &mut bag)?;
        let fe = fe.unwrap();
        fe.Initialize(bag.as_ref())?;
        fe.SetSize(w as u32, h as u32)?;
        let mut pf: GUID = GUID_WICPixelFormat24bppBGR;
        fe.SetPixelFormat(&mut pf)?;
        fe.WriteSource(&bmp, std::ptr::null())?;
        fe.Commit()?;
        enc.Commit()
    }
}

pub fn load_png(f: &IWICImagingFactory, path: &str) -> Option<(i32, i32, Vec<u32>)> {
    unsafe {
        let wp: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        let dec = f.CreateDecoderFromFilename(PCWSTR(wp.as_ptr()), None, windows::Win32::Foundation::GENERIC_ACCESS_RIGHTS(0x8000_0000), WICDecodeMetadataCacheOnDemand).ok()?;
        let fr = dec.GetFrame(0).ok()?;
        let conv = f.CreateFormatConverter().ok()?;
        conv.Initialize(&fr, &GUID_WICPixelFormat32bppBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom).ok()?;
        let (mut w, mut h) = (0u32, 0u32);
        conv.GetSize(&mut w, &mut h).ok()?;
        let mut buf = vec![0u32; (w * h) as usize];
        let bytes = std::slice::from_raw_parts_mut(buf.as_mut_ptr() as *mut u8, buf.len() * 4);
        conv.CopyPixels(std::ptr::null(), w * 4, bytes).ok()?;
        Some((w as i32, h as i32, buf))
    }
}

fn reset() {
    for h in app::open_windows() {
        unsafe { DestroyWindow(h) };
    }
    list::close();
    app::with(|a| a.notes.clear());
    note::FORCE_FULL.with(|f| f.set(false));
}

pub fn shots(cases_path: &str, outdir: &str, only: &str) {
    note::register_class();
    let f = wic();
    let _ = std::fs::create_dir_all(outdir);
    let mut log = Vec::new();
    for c in load_cases(cases_path) {
        if !only.is_empty() && !c.name.contains(only) {
            continue;
        }
        reset();
        app::with(|a| {
            a.theme = c.theme;
            a.band = c.band;
        });
        let has = |s: &str| c.states.iter().any(|x| x == s || x.starts_with(&format!("{s}=")));
        note::FORCE_FULL.with(|ff| ff.set(has("full")));
        let (target, popup) = if c.name.starts_with("list_") {
            app::load_demo();
            list::open(false);
            let h = list::hwnd();
            let dpi = unsafe { GetDpiForWindow(h) };
            unsafe { SetWindowPos(h, std::ptr::null_mut(), -15000, -15000, px(c.w, dpi), px(c.h, dpi), SWP_NOZORDER | SWP_NOACTIVATE) };
            pump_for(50);
            if let Some(q) = c.states.iter().find_map(|s| s.strip_prefix("search=")) {
                list::set_query(q);
            }
            let popup = if has("ctx") { list::context_at(px(120.0, dpi), px(160.0, dpi)) } else { std::ptr::null_mut() };
            (h, popup)
        } else {
            let id = app::add(&c.title, &c.text, Some(c.color), 0);
            app::update(id, |n| {
                n.folded = has("folded");
                n.locked = has("locked");
                n.pinned = has("pinned");
            });
            app::open_note(id, Some((-15000, -15000)), false);
            let h = app::window_of(id).unwrap();
            let dpi = unsafe { GetDpiForWindow(h) };
            unsafe {
                SetWindowPos(h, std::ptr::null_mut(), 0, 0, px(c.w, dpi), px(c.h, dpi), SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
                if has("pinned") {
                    // pinned windows are topmost; keep the screenshot run from covering anything
                    SetWindowPos(h, HWND_NOTOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
                }
            }
            let s = unsafe { note::state_of(h).unwrap() };
            if has("hover") {
                unsafe { SendMessageW(h, WM_APP_HOVER, 0, 0) };
            }
            if has("copied") {
                s.copied = true;
            }
            if has("menu") {
                note::open_menu(s);
            }
            if has("confirm") {
                note::request_delete(h, id);
            }
            if has("rename") {
                unsafe { SendMessageW(h, WM_APP_MENU, Act::Rename.code(), 0) };
            }
            if has("select") {
                // body text selected (allowed difference A7: selection colour). The window is
                // never focused here, so EM_HIDESELECTION(FALSE) keeps the selection drawn.
                unsafe {
                    SendMessageW(s.body, windows_sys::Win32::UI::Controls::EM_SETSEL, 0, -1);
                    SendMessageW(s.body, 0x043F /* EM_HIDESELECTION */, 0, 0);
                }
            }
            (h, s.menu)
        };
        pump_for(120);
        unsafe {
            RedrawWindow(target, std::ptr::null(), std::ptr::null_mut(), RDW_INVALIDATE | RDW_UPDATENOW | RDW_ALLCHILDREN);
            if !popup.is_null() {
                RedrawWindow(popup, std::ptr::null(), std::ptr::null_mut(), RDW_INVALIDATE | RDW_UPDATENOW);
            }
        }
        pump_for(80);
        if std::env::var_os("GM_DUMP_POPUP").is_some() {
            unsafe {
                let mut cls = [0u16; 64];
                let n = GetClassNameW(target, cls.as_mut_ptr(), 64);
                let mut wr = RECT { left: 0, top: 0, right: 0, bottom: 0 };
                let mut cr = wr;
                GetWindowRect(target, &mut wr);
                GetClientRect(target, &mut cr);
                log.push(
                    Json::default()
                        .s("diag", &c.name)
                        .s("class", &String::from_utf16_lossy(&cls[..n as usize]))
                        .n("userdata", GetWindowLongPtrW(target, GWLP_USERDATA) as f64)
                        .raw("window", format!("[{},{}]", wr.right - wr.left, wr.bottom - wr.top))
                        .raw("client", format!("[{},{}]", cr.right, cr.bottom))
                        .n("visible", IsWindowVisible(target) as f64)
                        .n("list_paints", list::PAINTS.load(std::sync::atomic::Ordering::Relaxed) as f64)
                        .n("menu_paints", crate::menu::PAINTS.load(std::sync::atomic::Ordering::Relaxed) as f64)
                        .finish(),
                );
            }
        }
        let (w, h, mut img) = capture_with(target, 0);
        if std::env::var_os("GM_DUMP_POPUP").is_some() && !popup.is_null() {
            let (pw, ph, p) = capture(popup);
            let vis = unsafe { IsWindowVisible(popup) };
            let mut ur = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            let upd = unsafe { GetUpdateRect(popup, &mut ur, 0) };
            let _ = save_png(&f, &format!("{outdir}\\{}.popup.png", c.name), pw, ph, &p);
            log.push(Json::default().s("popup", &c.name).n("vis", vis as f64).n("pending_update", upd as f64).n("w", pw as f64).n("h", ph as f64).n("px0", p.first().copied().unwrap_or(0) as f64).finish());
        }
        composite(target, &mut img, w, h, popup);
        let path = format!("{outdir}\\{}.png", c.name);
        let ok = save_png(&f, &path, w, h, &img);
        log.push(Json::default().s("case", &c.name).n("w", w as f64).n("h", h as f64).b("saved", ok.is_ok()).finish());
        if !popup.is_null() {
            unsafe { DestroyWindow(popup) };
        }
    }
    reset();
    metrics::append(&format!("{outdir}\\shots.jsonl"), &log.join("\n"));
}

/// Pixel comparison. "changed" = any channel differs by more than 40; "shifted" is the
/// subset that has no matching pixel within 1 px in the reference (tolerates 1 px moves).
pub fn diff(refdir: &str, curdir: &str) {
    let f = wic();
    let mut lines = Vec::new();
    let Ok(rd) = std::fs::read_dir(curdir) else { return };
    let mut names: Vec<String> = rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.ends_with(".png") && !n.ends_with(".diff.png")).collect();
    names.sort();
    for n in names {
        let (Some((cw, ch, cur)), Some((rw, rh, rf))) = (load_png(&f, &format!("{curdir}\\{n}")), load_png(&f, &format!("{refdir}\\{n}"))) else {
            lines.push(Json::default().s("case", &n).s("error", "missing reference").finish());
            continue;
        };
        let (w, h) = (cw.min(rw), ch.min(rh));
        let close = |a: u32, b: u32| {
            let d = |s: u32| ((a >> s) & 0xFF).abs_diff((b >> s) & 0xFF);
            d(0).max(d(8)).max(d(16)) <= 40
        };
        let mut changed = 0u64;
        let mut shifted = 0u64;
        let mut sum = 0u64;
        let mut out = vec![0u32; (cw * ch) as usize];
        for y in 0..ch {
            for x in 0..cw {
                let c = cur[(y * cw + x) as usize];
                let g = ((c & 0xFF) + ((c >> 8) & 0xFF) + ((c >> 16) & 0xFF)) / 3 / 3 + 170;
                let mut o = g | (g << 8) | (g << 16);
                if x < w && y < h {
                    let r = rf[(y * rw + x) as usize];
                    let d = |s: u32| ((c >> s) & 0xFF).abs_diff((r >> s) & 0xFF) as u64;
                    sum += d(0) + d(8) + d(16);
                    if !close(c, r) {
                        changed += 1;
                        let mut near = false;
                        for dy in -1..=1 {
                            for dx in -1..=1 {
                                let (xx, yy) = (x + dx, y + dy);
                                if xx >= 0 && yy >= 0 && xx < w && yy < h && close(c, rf[(yy * rw + xx) as usize]) {
                                    near = true;
                                }
                            }
                        }
                        if near {
                            o = 0x00F0A060; // light orange: within 1 px
                        } else {
                            shifted += 1;
                            o = 0x000000E0; // red in BGRX
                        }
                    }
                } else {
                    o = 0x00808080;
                }
                out[(y * cw + x) as usize] = o;
            }
        }
        let total = (w as u64 * h as u64).max(1);
        let _ = save_png(&f, &format!("{curdir}\\{}", n.replace(".png", ".diff.png")), cw, ch, &out);
        lines.push(
            Json::default()
                .s("case", &n.replace(".png", ""))
                .raw("cur", format!("[{cw},{ch}]"))
                .raw("ref", format!("[{rw},{rh}]"))
                .b("same_size", cw == rw && ch == rh)
                .n("mean_abs", sum as f64 / (total * 3) as f64)
                .n("changed_pct", changed as f64 * 100.0 / total as f64)
                .n("beyond_1px_pct", shifted as f64 * 100.0 / total as f64)
                .finish(),
        );
    }
    let _ = std::fs::write(format!("{curdir}\\diff.jsonl"), lines.join("\n"));
    let _ = Interface::as_raw(&f);
}

#[allow(dead_code)]
fn unused() {
    let _ = wide("");
}

/// `--mode notesnap --data COPY --outdir DIR`: opens the notes stored in COPY (a copy of a
/// data folder, never the live one) off-screen at their saved sizes, waits for the
/// picture decoder, and saves one PNG per open note.
pub fn notesnap(data: &str, outdir: &str) {
    note::register_class();
    let f = wic();
    let _ = std::fs::create_dir_all(outdir);
    let Ok(loaded) = crate::store::open(std::path::Path::new(data)) else { std::process::exit(3) };
    app::load(loaded);
    let notes: Vec<(u64, Option<(f64, f64, f64, f64)>)> = app::with(|a| a.notes.iter().filter(|n| n.open).map(|n| (n.id, n.rect)).collect());
    let mut log = vec![Json::default().n("loaded", app::with(|a| a.notes.len()) as f64).n("open", notes.len() as f64).finish()];
    metrics::append(&format!("{outdir}/notesnap.jsonl"), &log[0]);
    for (k, (id, rect)) in notes.iter().enumerate() {
        app::with(|a| a.notes.iter_mut().filter(|n| n.id == *id).for_each(|n| n.folded = false));
        app::open_note(*id, Some((-15000, -15000 + 1200 * k as i32)), false);
        let h = app::window_of(*id).unwrap();
        let dpi = unsafe { GetDpiForWindow(h) };
        if let Some((_, _, w, hh)) = rect {
            unsafe { SetWindowPos(h, std::ptr::null_mut(), 0, 0, px(*w as f32, dpi), px(*hh as f32, dpi), SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE) };
        }
    }
    pump_for(2500);
    for (id, _) in &notes {
        let h = app::window_of(*id).unwrap();
        unsafe { RedrawWindow(h, std::ptr::null(), std::ptr::null_mut(), RDW_INVALIDATE | RDW_UPDATENOW | RDW_ALLCHILDREN) };
        pump_for(100);
        let (w, hh, img) = capture_with(h, 0);
        let path = format!("{outdir}/note{id}.png");
        let ok = save_png(&f, &path, w, hh, &img);
        // mouse wheel: scroll position (px) right after each notch down and 400 ms later
        // (a difference = the scroll went on after the wheel stopped)
        let body = unsafe { note::state_of(h).unwrap().body };
        let pos = || unsafe {
            let mut p = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
            SendMessageW(body, 0x04DD /* EM_GETSCROLLPOS */, 0, &mut p as *mut _ as isize);
            p.y
        };
        let mut ys = vec![format!("[{},{}]", pos(), pos())];
        for _ in 0..6 {
            unsafe { SendMessageW(body, WM_MOUSEWHEEL, ((-120i32 as u32 as usize) << 16) & 0xFFFF_0000, 0) };
            let now = pos();
            pump_for(400);
            ys.push(format!("[{now},{}]", pos()));
        }
        log.push(Json::default().n("note", *id as f64).n("w", w as f64).n("h", hh as f64).b("saved", ok.is_ok()).raw("wheel_y", format!("[{}]", ys.join(","))).finish());
        // narrowed to 60 %: captured right after WM_SIZE (what the next frame shows), then
        // after the sharp copies came
        unsafe {
            let p = POINT { x: 0, y: 0 };
            SendMessageW(body, 0x04DE /* EM_SETSCROLLPOS */, 0, &p as *const _ as isize);
            SetWindowPos(h, std::ptr::null_mut(), 0, 0, w * 6 / 10, hh, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
        }
        let (nw, nh, img) = capture_with(h, 0);
        let _ = save_png(&f, &format!("{outdir}/note{id}_narrow_now.png"), nw, nh, &img);
        pump_for(2000);
        let (nw, nh, img) = capture_with(h, 0);
        let _ = save_png(&f, &format!("{outdir}/note{id}_narrow_settled.png"), nw, nh, &img);
    }
    crate::store::close();
    metrics::append(&format!("{outdir}/notesnap.jsonl"), &log[1..].join("\n"));
}
