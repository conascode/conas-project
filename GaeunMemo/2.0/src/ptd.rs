//! PT-D (report §6.7): pictures inserted with ITextRange2::InsertImage, the file name
//! kept as alt text. Checks the decision criteria:
//!   ① names can be read back (TOM textize, RTF stream-out)
//!   ② memory follows the display size, not the file's pixel size
//!   ③ names survive delete+undo and an in-document formatted copy
//!   ④ width fit: an inserted picture can be re-inserted at another size
//! Pictures are located through U+FFFC (they are not OLE objects).

use crate::body;
use crate::metrics::{self, Json};
use crate::note;
use crate::win::{px, theme};
use windows::core::BSTR;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::UI::Controls::RichEdit::*;
use windows::Win32::UI::Shell::SHCreateMemStream;
use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::Storage::Xps::PrintWindow;
use windows_sys::Win32::UI::Controls::EM_EMPTYUNDOBUFFER;
use windows_sys::Win32::UI::WindowsAndMessaging::{GetClientRect, SendMessageW, SetWindowPos, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER};

const HIMETRIC_PER_PX: f64 = 2540.0 / 96.0;
const OBJ: u16 = 0xFFFC;

fn pixel_size(wic: &IWICImagingFactory, bytes: &[u8]) -> Option<(u32, u32)> {
    unsafe {
        let s = SHCreateMemStream(Some(bytes))?;
        let d = wic.CreateDecoderFromStream(&s, std::ptr::null(), WICDecodeMetadataCacheOnDemand).ok()?;
        let f = d.GetFrame(0).ok()?;
        let (mut w, mut h) = (0u32, 0u32);
        f.GetSize(&mut w, &mut h).ok()?;
        Some((w, h))
    }
}

unsafe extern "system" fn stream_cb(cookie: usize, buf: *mut u8, cb: i32, pcb: *mut i32) -> u32 {
    let out = &mut *(cookie as *mut Vec<u8>);
    out.extend_from_slice(std::slice::from_raw_parts(buf, cb as usize));
    *pcb = cb;
    0
}

fn rtf(body: HWND) -> String {
    let mut out: Vec<u8> = Vec::new();
    let es = EDITSTREAM { dwCookie: &mut out as *mut _ as usize, dwError: 0, pfnCallback: Some(stream_cb) };
    unsafe { SendMessageW(body, EM_STREAMOUT, SF_RTF as usize, &es as *const _ as isize) };
    String::from_utf8_lossy(&out).into_owned()
}

fn count_in(hay: &str, names: &[String]) -> Vec<usize> {
    names.iter().map(|n| hay.matches(n.as_str()).count()).collect()
}

fn story(doc: &ITextDocument2, flags: i32) -> String {
    unsafe {
        doc.Range2(0, 0)
            .and_then(|r| {
                let _ = r.Expand(tomStory.0);
                r.GetText2(flags)
            })
            .map(|b| b.to_string())
            .unwrap_or_default()
    }
}

/// Character positions of the pictures (U+FFFC in the plain story).
fn picture_cps(doc: &ITextDocument2) -> Vec<i32> {
    story(doc, 0).encode_utf16().enumerate().filter(|&(_, c)| c == OBJ).map(|(i, _)| i as i32).collect()
}

fn name_at(doc: &ITextDocument2, cp: i32) -> String {
    unsafe { doc.Range2(cp, cp + 1).and_then(|r| r.GetText2(tomTextize.0)).map(|b| b.to_string()).unwrap_or_default() }
}

fn story_len(doc: &ITextDocument2) -> i32 {
    unsafe { doc.Range2(0, 0).and_then(|r| r.GetStoryLength()).unwrap_or(1) }
}

fn insert(doc: &ITextDocument2, at: i32, bytes: &[u8], w: i32, h: i32, name: &str) -> windows::core::Result<()> {
    unsafe {
        let r = doc.Range2(at, at)?;
        let s = SHCreateMemStream(Some(bytes)).expect("stream");
        let wh = (w as f64 * HIMETRIC_PER_PX).round() as i32;
        let hh = (h as f64 * HIMETRIC_PER_PX).round() as i32;
        r.InsertImage(wh, hh, hh, 24 /* TA_BASELINE */, &BSTR::from(name), &s)
    }
}

/// Renders the window through PrintWindow and counts pixels that are neither the
/// paper colour nor the text colour — i.e. picture pixels actually drawn.
fn rendered_picture_pixels(h: HWND) -> (u64, f64) {
    unsafe {
        let mut rc = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        GetClientRect(h, &mut rc);
        let (w, ht) = (rc.right, rc.bottom);
        let screen = GetDC(std::ptr::null_mut());
        let mem = CreateCompatibleDC(screen);
        let mut bi: BITMAPINFO = std::mem::zeroed();
        bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bi.bmiHeader.biWidth = w;
        bi.bmiHeader.biHeight = -ht;
        bi.bmiHeader.biPlanes = 1;
        bi.bmiHeader.biBitCount = 32;
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let dib = CreateDIBSection(mem, &bi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        let old = SelectObject(mem, dib);
        let t0 = std::time::Instant::now();
        PrintWindow(h, mem, 2 /* PW_RENDERFULLCONTENT */);
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        let px = std::slice::from_raw_parts(bits as *const u32, (w * ht) as usize);
        let paper = theme::PAPER;
        let bgr = |c: u32| ((c & 0xFF) << 16) | (c & 0xFF00) | ((c >> 16) & 0xFF);
        let (p, ink) = (bgr(paper), bgr(theme::INK));
        let n = px
            .iter()
            .filter(|&&v| {
                let v = v & 0xFFFFFF;
                let (r, g, b) = ((v >> 16) & 0xFF, (v >> 8) & 0xFF, v & 0xFF);
                v != p && v != ink && !(r == g && g == b) // pictures here are colourful; text AA is grey
            })
            .count() as u64;
        SelectObject(mem, old);
        DeleteObject(dib);
        DeleteDC(mem);
        ReleaseDC(std::ptr::null_mut(), screen);
        (n, ms)
    }
}

pub fn run(images: &[String], out: &str, label: &str) {
    note::register_class();
    let base = metrics::sample();
    let h = note::create(0, -15000, -15000, true);
    let dpi = unsafe { windows_sys::Win32::UI::HiDpi::GetDpiForWindow(h) };
    unsafe { SetWindowPos(h, std::ptr::null_mut(), 0, 0, px(1260.0, dpi), px(1000.0, dpi), SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE) };
    let body_h = note::body_of(h);
    let Some((_ole, doc)) = body::tom(body_h) else {
        metrics::append(out, &Json::default().s("mode", "ptd").s("error", "no TOM").finish());
        return;
    };
    let (blank_px, _) = rendered_picture_pixels(h);
    let after_note = metrics::sample();
    let wic: IWICImagingFactory = unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).expect("WIC") };

    // each file at note width (236 logical px) and at "full screen" width (1200, capped at the file)
    let mut names = Vec::new();
    let mut files: Vec<(Vec<u8>, u32, u32, String)> = Vec::new();
    let mut steps = Vec::new();
    for (k, path) in images.iter().enumerate() {
        let Ok(bytes) = std::fs::read(path) else { continue };
        let Some((pw, ph)) = pixel_size(&wic, &bytes) else {
            steps.push(Json::default().s("file", path).s("error", "WIC cannot decode").finish());
            continue;
        };
        for (j, want) in [236i32, 1200].into_iter().enumerate() {
            let disp_w = want.min(pw as i32);
            let disp_h = (disp_w as f64 * ph as f64 / pw as f64).round() as i32;
            let ext = path.rsplit('.').next().unwrap_or("bin");
            let name = format!("{k:08}-0000-4000-8000-{j:012}.{ext}");
            let before = metrics::sample();
            let t0 = std::time::Instant::now();
            let at = (story_len(&doc) - 1).max(0);
            let ok = insert(&doc, at, &bytes, disp_w, disp_h, &name);
            unsafe {
                let _ = doc.Range2(at + 1, at + 1).and_then(|r| r.SetText(&BSTR::from("\r")));
            }
            let insert_ms = t0.elapsed().as_secs_f64() * 1000.0;
            let after_insert = metrics::sample();
            let (pix, render_ms) = rendered_picture_pixels(h);
            let after_render = metrics::sample();
            steps.push(
                Json::default()
                    .s("file", &path.rsplit(['\\', '/']).next().unwrap_or(path).to_string())
                    .n("file_kb", bytes.len() as f64 / 1024.0)
                    .n("pixels_w", pw as f64)
                    .n("pixels_h", ph as f64)
                    .n("display_w", disp_w as f64)
                    .n("display_h", disp_h as f64)
                    .n("full_decode_mb", pw as f64 * ph as f64 * 4.0 / 1048576.0)
                    .n("display_decode_mb", disp_w as f64 * disp_h as f64 * 4.0 * (dpi as f64 / 96.0).powi(2) / 1048576.0)
                    .b("inserted", ok.is_ok())
                    .n("insert_ms", insert_ms)
                    .n("render_ms", render_ms)
                    .n("picture_pixels_visible", pix as f64)
                    .n("d_private_insert_mb", after_insert.private_mb - before.private_mb)
                    .n("d_private_render_mb", after_render.private_mb - after_insert.private_mb)
                    .finish(),
            );
            names.push(name);
            files.push((bytes.clone(), pw, ph, ext.to_string()));
        }
    }
    let loaded = metrics::sample();

    // ① read-back
    let cps = picture_cps(&doc);
    let read_names: Vec<String> = cps.iter().map(|&cp| name_at(&doc, cp)).collect();
    let names_match = read_names == names;
    let rtf1 = rtf(body_h);
    let rtf_hits = count_in(&rtf1, &names);
    let rtf_marker = ["\\pict", "\\shppict", "wzDescription", "\\*\\picprop", "\\pngblip", "\\jpegblip"]
        .iter()
        .map(|m| format!("\"{}\":{}", m.replace('\\', "\\\\"), rtf1.matches(m).count()))
        .collect::<Vec<_>>()
        .join(",");

    // ③ delete the first picture, then undo
    let undo = unsafe {
        let cp = cps[0];
        let r = doc.Range2(cp, cp + 1).expect("range");
        let _ = r.SetText(&BSTR::new());
        let after_delete = picture_cps(&doc).len();
        let _ = doc.Undo(1);
        let after = picture_cps(&doc);
        let back: Vec<String> = after.iter().map(|&c| name_at(&doc, c)).collect();
        Json::default()
            .n("pictures_before", cps.len() as f64)
            .n("after_delete", after_delete as f64)
            .n("after_undo", after.len() as f64)
            .b("names_equal_after_undo", back == names)
            .finish()
    };
    // ③ copy the first picture to the end of the document (TOM formatted text, no clipboard)
    let copy = unsafe {
        let cp = picture_cps(&doc)[0];
        let src = doc.Range2(cp, cp + 1).expect("range");
        let at = (story_len(&doc) - 1).max(0);
        let dst = doc.Range2(at, at).expect("range");
        let set = src.GetFormattedText().and_then(|f| dst.SetFormattedText(&f));
        let after = picture_cps(&doc);
        let last = after.last().map(|&c| name_at(&doc, c)).unwrap_or_default();
        Json::default()
            .b("copied", set.is_ok())
            .n("pictures", after.len() as f64)
            .s("copy_name", &last)
            .b("copy_keeps_name", last == names[0])
            .finish()
    };
    // ④ width fit by re-insert: replace picture 0 (236 px) with a 118 px one, undo suspended
    let refit = {
        let cp = picture_cps(&doc)[0];
        let (bytes, pw, ph, _) = &files[0];
        let w = 118;
        let hgt = (w as f64 * *ph as f64 / *pw as f64).round() as i32;
        let t0 = std::time::Instant::now();
        let ok = unsafe {
            let _ = doc.Undo(tomSuspend.0);
            let r = doc.Range2(cp, cp + 1).expect("range");
            let _ = r.SetText(&BSTR::new());
            let res = insert(&doc, cp, bytes, w, hgt, &names[0]);
            let _ = doc.Undo(tomResume.0);
            res
        };
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        let now = picture_cps(&doc);
        Json::default()
            .b("reinserted", ok.is_ok())
            .n("ms", ms)
            .n("pictures", now.len() as f64)
            .b("name_kept", name_at(&doc, now[0]) == names[0])
            .finish()
    };
    // release: clear the document and the undo stack; does the memory come back?
    let full = metrics::sample();
    let clear = || unsafe {
        let _ = doc.Range2(0, 0).and_then(|r| {
            let _ = r.Expand(tomStory.0);
            r.SetText(&BSTR::new())
        });
        SendMessageW(body_h, EM_EMPTYUNDOBUFFER, 0, 0);
        let _ = rendered_picture_pixels(h);
    };
    clear();
    let cleared = metrics::sample();

    // PT-D′: insert a display-size re-encoded copy instead of the original file
    let mut dprime = Vec::new();
    for (k, (bytes, pw, ph, ext)) in files.iter().enumerate().step_by(2) {
        for want in [236i32, 1200] {
            let disp_w = want.min(*pw as i32);
            let disp_h = (disp_w as f64 * *ph as f64 / *pw as f64).round() as i32;
            let (tw, th) = (px(disp_w as f32, dpi) as u32, px(disp_h as f32, dpi) as u32);
            let before = metrics::sample();
            let t0 = std::time::Instant::now();
            let enc = reencode(&wic, bytes, tw.min(*pw), th.min(*ph), ext == "jpg");
            let enc_ms = t0.elapsed().as_secs_f64() * 1000.0;
            let Some((small, small_len)) = enc else {
                dprime.push(Json::default().s("ext", ext).s("error", "reencode failed").finish());
                continue;
            };
            let at = (story_len(&doc) - 1).max(0);
            let name = format!("{k:08}-0000-4000-8000-d{want:011}.{ext}");
            let ok = unsafe {
                let r = doc.Range2(at, at).expect("range");
                let wh = (disp_w as f64 * HIMETRIC_PER_PX).round() as i32;
                let hh = (disp_h as f64 * HIMETRIC_PER_PX).round() as i32;
                let res = r.InsertImage(wh, hh, hh, 24, &BSTR::from(name.as_str()), &small);
                let _ = doc.Range2(at + 1, at + 1).and_then(|r| r.SetText(&BSTR::from("\r")));
                res
            };
            let after_insert = metrics::sample();
            let (pix, _) = rendered_picture_pixels(h);
            let after_render = metrics::sample();
            dprime.push(
                Json::default()
                    .s("ext", ext)
                    .n("display_w", disp_w as f64)
                    .n("encoded_px_w", tw as f64)
                    .n("encoded_kb", small_len as f64 / 1024.0)
                    .n("reencode_ms", enc_ms)
                    .b("inserted", ok.is_ok())
                    .n("picture_pixels_visible", pix as f64)
                    .n("d_private_insert_mb", after_insert.private_mb - before.private_mb)
                    .n("d_private_render_mb", after_render.private_mb - after_insert.private_mb)
                    .b("name_ok", picture_cps(&doc).last().map(|&c| name_at(&doc, c)) == Some(name.clone()))
                    .finish(),
            );
        }
    }
    clear();

    // stability: insert every picture (originals) and clear again, `cycles` times
    let cycles = crate::cfg().cycles.min(30);
    let mut trace = Vec::new();
    for _ in 0..cycles {
        for (k, (bytes, pw, ph, ext)) in files.iter().enumerate() {
            let want = if k % 2 == 0 { 236 } else { 1200 };
            let w = want.min(*pw as i32);
            let hh = (w as f64 * *ph as f64 / *pw as f64).round() as i32;
            let at = (story_len(&doc) - 1).max(0);
            let _ = insert(&doc, at, bytes, w, hh, &format!("cycle.{ext}"));
        }
        let _ = rendered_picture_pixels(h);
        clear();
        trace.push((metrics::sample().private_mb * 100.0).round() / 100.0);
    }

    let line = Json::default()
        .s("mode", "ptd")
        .s("label", label)
        .n("dpi", dpi as f64)
        .n("blank_picture_pixels", blank_px as f64)
        .sample("base", base)
        .sample("note", after_note)
        .sample("loaded", loaded)
        .sample("full", full)
        .sample("cleared", cleared)
        .raw("steps", format!("[{}]", steps.join(",")))
        .raw("names", format!("{:?}", names))
        .raw("read_names", format!("{:?}", read_names))
        .b("names_match", names_match)
        .raw("rtf_hits", format!("{:?}", rtf_hits))
        .raw("rtf_markers", format!("{{{rtf_marker}}}"))
        .n("rtf_kb", rtf1.len() as f64 / 1024.0)
        .raw("undo", undo)
        .raw("copy", copy)
        .raw("refit", refit)
        .raw("dprime", format!("[{}]", dprime.join(",")))
        .raw("cycle_private_mb", format!("{:?}", trace))
        .finish();
    metrics::append(out, &line);
}

/// Display-size copy of a picture through WIC (Fant scaler → JPEG or PNG), as an IStream at 0.
fn reencode(wic: &IWICImagingFactory, bytes: &[u8], w: u32, h: u32, jpeg: bool) -> Option<(windows::Win32::System::Com::IStream, u64)> {
    use windows::Win32::System::Com::{STATFLAG_NONAME, STATSTG, STREAM_SEEK_SET};
    unsafe {
        let src = SHCreateMemStream(Some(bytes))?;
        let dec = wic.CreateDecoderFromStream(&src, std::ptr::null(), WICDecodeMetadataCacheOnDemand).ok()?;
        let frame = dec.GetFrame(0).ok()?;
        let scaler = wic.CreateBitmapScaler().ok()?;
        scaler.Initialize(&frame, w, h, WICBitmapInterpolationModeFant).ok()?;
        let conv = wic.CreateFormatConverter().ok()?;
        let fmt = if jpeg { GUID_WICPixelFormat24bppBGR } else { GUID_WICPixelFormat32bppBGRA };
        conv.Initialize(&scaler, &fmt, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom).ok()?;
        let out = SHCreateMemStream(None)?;
        let container = if jpeg { GUID_ContainerFormatJpeg } else { GUID_ContainerFormatPng };
        let enc = wic.CreateEncoder(&container, std::ptr::null()).ok()?;
        enc.Initialize(&out, WICBitmapEncoderNoCache).ok()?;
        let mut fe = None;
        let mut bag = None;
        enc.CreateNewFrame(&mut fe, &mut bag).ok()?;
        let fe = fe?;
        fe.Initialize(bag.as_ref()).ok()?;
        fe.SetSize(w, h).ok()?;
        let mut pf = fmt;
        fe.SetPixelFormat(&mut pf).ok()?;
        fe.WriteSource(&conv, std::ptr::null()).ok()?;
        fe.Commit().ok()?;
        enc.Commit().ok()?;
        let mut st: STATSTG = std::mem::zeroed();
        out.Stat(&mut st, STATFLAG_NONAME).ok()?;
        out.Seek(0, STREAM_SEEK_SET, None).ok()?;
        Some((out, st.cbSize))
    }
}

/// Does RichEdit animate a GIF inserted with InsertImage? Samples the picture colour over time.
pub fn gif(path: &str, out: &str, label: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    note::register_class();
    let h = note::create(0, -15000, -15000, true);
    let body_h = note::body_of(h);
    let Some((_ole, doc)) = body::tom(body_h) else { return };
    let Ok(bytes) = std::fs::read(path) else { return };
    unsafe {
        let _ = doc.Range2(0, 0).and_then(|r| {
            let _ = r.Expand(tomStory.0);
            r.SetText(&BSTR::new())
        });
    }
    let _ = insert(&doc, 0, &bytes, 120, 80, "anim.gif");
    let colour = |hwnd: HWND| -> (u64, u64) {
        unsafe {
            let mut rc = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            GetClientRect(hwnd, &mut rc);
            let (w, ht) = (rc.right, rc.bottom);
            let screen = GetDC(std::ptr::null_mut());
            let mem = CreateCompatibleDC(screen);
            let mut bi: BITMAPINFO = std::mem::zeroed();
            bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
            bi.bmiHeader.biWidth = w;
            bi.bmiHeader.biHeight = -ht;
            bi.bmiHeader.biPlanes = 1;
            bi.bmiHeader.biBitCount = 32;
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            let dib = CreateDIBSection(mem, &bi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
            let old = SelectObject(mem, dib);
            PrintWindow(hwnd, mem, 2);
            let px = std::slice::from_raw_parts(bits as *const u32, (w * ht) as usize);
            let red = px.iter().filter(|&&v| ((v >> 16) & 0xFF) > 180 && ((v >> 8) & 0xFF) < 110 && (v & 0xFF) < 110).count() as u64;
            let blue = px.iter().filter(|&&v| (v & 0xFF) > 180 && ((v >> 16) & 0xFF) < 110).count() as u64;
            SelectObject(mem, old);
            DeleteObject(dib);
            DeleteDC(mem);
            ReleaseDC(std::ptr::null_mut(), screen);
            (red, blue)
        }
    };
    // pump messages for `ms` so RichEdit's own timers (if any) can run
    let pump_for = |ms: u64| unsafe {
        let end = std::time::Instant::now() + std::time::Duration::from_millis(ms);
        loop {
            let mut m: MSG = std::mem::zeroed();
            while PeekMessageW(&mut m, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&m);
                DispatchMessageW(&m);
            }
            let now = std::time::Instant::now();
            if now >= end {
                break;
            }
            MsgWaitForMultipleObjects(0, std::ptr::null(), 0, (end - now).as_millis() as u32, QS_ALLINPUT);
        }
    };
    let mut samples = Vec::new();
    for _ in 0..8 {
        samples.push(colour(h));
        pump_for(130);
    }
    let cpu0 = metrics::cpu_ms();
    let t0 = std::time::Instant::now();
    pump_for(5000);
    let cpu_pct = (metrics::cpu_ms() - cpu0) / (t0.elapsed().as_secs_f64() * 1000.0) * 100.0;
    let changes = samples.windows(2).filter(|w| (w[0].0 > w[0].1) != (w[1].0 > w[1].1)).count();
    let line = Json::default()
        .s("mode", "gif")
        .s("label", label)
        .raw("red_blue_samples", format!("[{}]", samples.iter().map(|(r, b)| format!("[{r},{b}]")).collect::<Vec<_>>().join(",")))
        .n("colour_changes", changes as f64)
        .b("animates", changes > 0)
        .n("idle_cpu_pct_with_gif", cpu_pct)
        .finish();
    metrics::append(out, &line);
}
