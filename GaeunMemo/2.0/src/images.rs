//! Pictures in note bodies (F-31; report §6.4, §10). P2 keeps the bytes in memory — the
//! images\ folder shared with v1.1 arrives with the store in P3. Rules from v1.1
//! images.rs: type by magic bytes (png / jpg / gif / webp), at most 25 MB, named
//! `<uuid>.<ext>`. A bitmap from the clipboard (screenshot) is stored as PNG, like Chromium
//! hands v1.1 a screenshot as image/png.
//!
//! Shown with PT-D′ (P0 decision): a display-size copy re-encoded through WIC and inserted
//! with `ITextRange2::InsertImage`, alt text = file name (how the name is read back).

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use windows::core::BSTR;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::{CoCreateInstance, IStream, CLSCTX_INPROC_SERVER, STATFLAG_NONAME, STATSTG, STREAM_SEEK_SET};
use windows::Win32::UI::Controls::RichEdit::ITextDocument2;
use windows::Win32::UI::Shell::SHCreateMemStream;

pub const MAX_BYTES: usize = 25 * 1024 * 1024;
const HIMETRIC_PER_PX: f64 = 2540.0 / 96.0;

thread_local! {
    /// bytes of pictures added in this session; bounded (see `remember`)
    static STORE: RefCell<HashMap<String, Arc<Vec<u8>>>> = RefCell::new(HashMap::new());
    static ORDER: RefCell<std::collections::VecDeque<String>> = RefCell::new(std::collections::VecDeque::new());
    /// latest display request per (note window, picture): older results are dropped
    static LATEST: RefCell<HashMap<(isize, String), u64>> = RefCell::new(HashMap::new());
    static WIC: RefCell<Option<IWICImagingFactory>> = const { RefCell::new(None) };
}

pub fn sniff(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some("png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if bytes.starts_with(b"GIF8") {
        Some("gif")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("webp")
    } else {
        None
    }
}

fn wic() -> Option<IWICImagingFactory> {
    WIC.with(|w| {
        if w.borrow().is_none() {
            *w.borrow_mut() = unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok() };
        }
        w.borrow().clone()
    })
}

fn new_name(ext: &str) -> String {
    format!("{}.{ext}", crate::win::uuid())
}

/// Pixel size, or None when WIC cannot decode the bytes.
pub fn pixel_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let wic = wic()?;
    unsafe {
        let s = SHCreateMemStream(Some(bytes))?;
        let d = wic.CreateDecoderFromStream(&s, std::ptr::null(), WICDecodeMetadataCacheOnDemand).ok()?;
        let f = d.GetFrame(0).ok()?;
        let (mut w, mut h) = (0u32, 0u32);
        f.GetSize(&mut w, &mut h).ok()?;
        (w > 0 && h > 0).then_some((w, h))
    }
}

/// Keeps a picture (png / jpg / gif / webp, ≤ 25 MB, decodable) and returns its new name.
pub fn add(bytes: Vec<u8>) -> Option<String> {
    if bytes.is_empty() || bytes.len() > MAX_BYTES {
        return None;
    }
    let ext = sniff(&bytes)?;
    pixel_size(&bytes)?;
    let name = new_name(ext);
    crate::log::write(&format!("picture added: {ext}, {} bytes", bytes.len()));
    let bytes = Arc::new(bytes);
    remember(&name, bytes.clone());
    // the file goes to images\ before the note row that refers to it (same writer FIFO)
    crate::store::send(crate::store::Cmd::Image { name: name.clone(), bytes });
    Some(name)
}

/// A clipboard bitmap (BITMAPINFO + bits, CF_DIB / CF_DIBV5) stored as PNG.
pub fn add_dib(dib: &[u8]) -> Option<String> {
    let png = dib_to_png(dib)?;
    add(png)
}

/// Keeps new pictures in memory until they are files; above 64 MB the oldest ones whose
/// file is already written are let go (report A.3 "LRU 64 MB").
fn remember(name: &str, bytes: Arc<Vec<u8>>) {
    const LIMIT: usize = 64 * 1024 * 1024;
    STORE.with(|s| s.borrow_mut().insert(name.to_string(), bytes));
    ORDER.with(|o| o.borrow_mut().push_back(name.to_string()));
    let Some(dir) = crate::store::images_dir() else { return }; // no store: memory only
    let total = || STORE.with(|s| s.borrow().values().map(|b| b.len()).sum::<usize>());
    let mut checked = 0;
    while total() > LIMIT && checked < ORDER.with(|o| o.borrow().len()) {
        let Some(old) = ORDER.with(|o| o.borrow_mut().pop_front()) else { break };
        if dir.join(&old).exists() {
            STORE.with(|s| s.borrow_mut().remove(&old));
        } else {
            ORDER.with(|o| o.borrow_mut().push_back(old)); // not on disk yet: keep it
            checked += 1;
        }
    }
}

pub fn get(name: &str) -> Option<Arc<Vec<u8>>> {
    STORE.with(|s| s.borrow().get(name).cloned())
}

fn encode(src: &IWICBitmapSource, w: u32, h: u32, jpeg: bool) -> Option<(IStream, u64)> {
    let wic = wic()?;
    unsafe {
        let conv = wic.CreateFormatConverter().ok()?;
        let fmt = if jpeg { GUID_WICPixelFormat24bppBGR } else { GUID_WICPixelFormat32bppBGRA };
        conv.Initialize(src, &fmt, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom).ok()?;
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

fn stream_bytes(s: &IStream, len: u64) -> Option<Vec<u8>> {
    let mut v = vec![0u8; len as usize];
    let mut got = 0u32;
    unsafe { s.Read(v.as_mut_ptr() as *mut _, len as u32, Some(&mut got)).ok().ok()? };
    v.truncate(got as usize);
    Some(v)
}

fn dib_to_png(dib: &[u8]) -> Option<Vec<u8>> {
    // a BMP file is BITMAPFILEHEADER + the DIB; WIC's BMP decoder reads it
    if dib.len() < 40 {
        return None;
    }
    let header_size = u32::from_le_bytes(dib[0..4].try_into().ok()?) as usize;
    let bit_count = u16::from_le_bytes(dib[14..16].try_into().ok()?) as usize;
    let compression = u32::from_le_bytes(dib[16..20].try_into().ok()?);
    let clr_used = u32::from_le_bytes(dib[32..36].try_into().ok()?) as usize;
    let masks = if compression == 3 && header_size == 40 { 12 } else { 0 }; // BI_BITFIELDS after a v1 header
    let palette = if bit_count <= 8 { (if clr_used > 0 { clr_used } else { 1 << bit_count }) * 4 } else { clr_used * 4 };
    let off = 14 + header_size + masks + palette;
    let mut bmp = Vec::with_capacity(dib.len() + 14);
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&((dib.len() + 14) as u32).to_le_bytes());
    bmp.extend_from_slice(&[0, 0, 0, 0]);
    bmp.extend_from_slice(&(off as u32).to_le_bytes());
    bmp.extend_from_slice(dib);
    let wic = wic()?;
    unsafe {
        let s = SHCreateMemStream(Some(&bmp))?;
        let d = wic.CreateDecoderFromStream(&s, std::ptr::null(), WICDecodeMetadataCacheOnDemand).ok()?;
        let f = d.GetFrame(0).ok()?;
        let (mut w, mut h) = (0u32, 0u32);
        f.GetSize(&mut w, &mut h).ok()?;
        let (out, len) = encode(&f.cast().ok()?, w, h, false)?;
        stream_bytes(&out, len)
    }
}

use windows::core::Interface;

/// Display-size stream of a picture (PT-D′): v1.1 shows it at its pixel size in CSS px,
/// capped at the note's text width (`max-width: 100%`).
fn display_stream(bytes: &[u8], max_css_w: f64, dpi: u32) -> Option<(IStream, f64, f64, f64)> {
    let wic = wic()?;
    unsafe {
        let src = SHCreateMemStream(Some(bytes))?;
        let dec = wic.CreateDecoderFromStream(&src, std::ptr::null(), WICDecodeMetadataCacheOnDemand).ok()?;
        let frame = dec.GetFrame(0).ok()?; // GIFs: first frame (user decision 2026-11-14)
        let (mut pw, mut ph) = (0u32, 0u32);
        frame.GetSize(&mut pw, &mut ph).ok()?;
        let css_w = (pw as f64).min(max_css_w.max(16.0));
        let css_h = css_w * ph as f64 / pw as f64;
        let scale = dpi as f64 / 96.0;
        let (tw, th) = (((css_w * scale).round() as u32).clamp(1, pw), ((css_h * scale).round() as u32).clamp(1, ph));
        let scaler = wic.CreateBitmapScaler().ok()?;
        scaler.Initialize(&frame, tw, th, WICBitmapInterpolationModeFant).ok()?;
        let jpeg = sniff(bytes) == Some("jpg");
        let (s, _) = encode(&scaler.cast().ok()?, tw, th, jpeg)?;
        Some((s, css_w, css_h, pw as f64))
    }
}

/// A small PNG for tests: a two-colour gradient `w`×`h`.
pub fn sample_png(w: u32, h: u32) -> Vec<u8> {
    let Some(wic) = wic() else { return Vec::new() };
    let mut px = vec![0u8; (w * h * 4) as usize];
    for (k, p) in px.chunks_mut(4).enumerate() {
        let (x, y) = (k as u32 % w, k as u32 / w);
        p.copy_from_slice(&[(x * 255 / w.max(1)) as u8, (y * 255 / h.max(1)) as u8, 0xB0, 0xFF]);
    }
    unsafe {
        let Ok(bmp) = wic.CreateBitmapFromMemory(w, h, &GUID_WICPixelFormat32bppBGRA, w * 4, &px) else { return Vec::new() };
        let Some(src) = bmp.cast::<IWICBitmapSource>().ok() else { return Vec::new() };
        encode(&src, w, h, false).and_then(|(s, n)| stream_bytes(&s, n)).unwrap_or_default()
    }
}

/// A grey box for a picture whose bytes are not available (keeps its name in the note).
fn placeholder(dpi: u32) -> Option<(IStream, f64, f64)> {
    let wic = wic()?;
    let (cw, ch) = (120.0, 80.0);
    let scale = dpi as f64 / 96.0;
    let (w, h) = ((cw * scale) as u32, (ch * scale) as u32);
    let mut px = vec![0u8; (w * h * 4) as usize];
    for (k, p) in px.chunks_mut(4).enumerate() {
        let (x, y) = (k as u32 % w, k as u32 / w);
        let edge = x == 0 || y == 0 || x == w - 1 || y == h - 1;
        let v = if edge { 0x99 } else { 0xD9 };
        p.copy_from_slice(&[v, v, v, 0xFF]);
    }
    unsafe {
        let bmp = wic.CreateBitmapFromMemory(w, h, &GUID_WICPixelFormat32bppBGRA, w * 4, &px).ok()?;
        let (s, _) = encode(&bmp.cast().ok()?, w, h, false)?;
        Some((s, cw, ch))
    }
}

/// The 사진 넣기 file picker (v1.1 `accept="image/png,image/jpeg,image/gif,image/webp" multiple`).
pub fn pick_files(owner: windows_sys::Win32::Foundation::HWND) -> Vec<std::path::PathBuf> {
    use windows_sys::Win32::UI::Controls::Dialogs::*;
    let filter: Vec<u16> = "사진 (PNG, JPEG, GIF, WebP)\0*.png;*.jpg;*.jpeg;*.gif;*.webp\0\0".encode_utf16().collect();
    let mut buf = vec![0u16; 32 * 1024];
    let title = crate::win::wide("사진 넣기");
    let mut ofn: OPENFILENAMEW = unsafe { std::mem::zeroed() };
    ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
    ofn.hwndOwner = owner;
    ofn.lpstrFilter = filter.as_ptr();
    ofn.lpstrFile = buf.as_mut_ptr();
    ofn.nMaxFile = buf.len() as u32;
    ofn.lpstrTitle = title.as_ptr();
    ofn.Flags = OFN_EXPLORER | OFN_ALLOWMULTISELECT | OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR | OFN_HIDEREADONLY;
    if unsafe { GetOpenFileNameW(&mut ofn) } == 0 {
        return Vec::new();
    }
    // one file: the full path; several: the folder, then the names; each NUL-ended, then NUL
    let parts: Vec<String> = buf.split(|&c| c == 0).take_while(|p| !p.is_empty()).map(String::from_utf16_lossy).collect();
    match parts.as_slice() {
        [] => Vec::new(),
        [one] => vec![one.into()],
        [dir, names @ ..] => names.iter().map(|n| std::path::Path::new(dir).join(n)).collect(),
    }
}

// ------------------------------------------------------------------ showing pictures

thread_local! {
    /// the grey placeholder per dpi (encoded once)
    static PLACEHOLDER: RefCell<HashMap<u32, Vec<u8>>> = RefCell::new(HashMap::new());
}

fn insert_stream(doc: &ITextDocument2, cp: i32, name: &str, stream: &IStream, cw: f64, ch: f64) -> bool {
    unsafe {
        let Ok(r) = doc.Range2(cp, cp) else { return false };
        let wh = (cw * HIMETRIC_PER_PX).round() as i32;
        let hh = (ch * HIMETRIC_PER_PX).round() as i32;
        r.InsertImage(wh, hh, hh, 24 /* TA_BASELINE */, &BSTR::from(name), stream).is_ok()
    }
}

/// Inserts picture `name` at `cp` (an empty range there): a grey placeholder at once, the
/// picture itself when the decoding thread has it (report §11.1: no decoding on the UI
/// thread). A picture without bytes or file keeps the placeholder — and its name.
pub fn insert(doc: &ITextDocument2, cp: i32, name: &str, max_css_w: f64, dpi: u32, note: windows_sys::Win32::Foundation::HWND) -> bool {
    let bytes = PLACEHOLDER.with(|p| {
        p.borrow_mut()
            .entry(dpi)
            .or_insert_with(|| placeholder(dpi).and_then(|(s, _, _)| unsafe {
                let mut st: STATSTG = std::mem::zeroed();
                s.Stat(&mut st, STATFLAG_NONAME).ok()?;
                stream_bytes(&s, st.cbSize)
            }).unwrap_or_default())
            .clone()
    });
    let Some(stream) = (unsafe { SHCreateMemStream(Some(&bytes)) }) else { return false };
    let ok = insert_stream(doc, cp, name, &stream, 120.0, 80.0);
    if ok {
        request(note, name, max_css_w, dpi);
    }
    ok
}

/// A finished display-size copy, posted to the note window (`note::WM_APP_DECODED`).
pub struct Decoded {
    pub name: String,
    pub bytes: Arc<Vec<u8>>,
    pub css: (f64, f64),
    /// the picture's own width in CSS px (v1.1 shows 1 px = 1 CSS px, capped at the text width)
    pub natural_w: f64,
    /// request generation: a result older than the latest request for the picture is dropped
    pub gen: u64,
}

/// True when `d` answers the latest request of `note` for that picture.
pub fn is_latest(note: windows_sys::Win32::Foundation::HWND, d: &Decoded) -> bool {
    LATEST.with(|l| l.borrow().get(&(note as isize, d.name.clone())).is_none_or(|&g| g == d.gen))
}

struct Req {
    gen: u64,
    note: isize,
    name: String,
    bytes: Option<Arc<Vec<u8>>>,
    file: Option<std::path::PathBuf>,
    max_css_w: f64,
    dpi: u32,
}

static DECODER: std::sync::Mutex<Option<std::sync::mpsc::Sender<Req>>> = std::sync::Mutex::new(None);

static GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Asks the decoding thread for a display-size copy of `name` (also after a resize).
pub fn request(note: windows_sys::Win32::Foundation::HWND, name: &str, max_css_w: f64, dpi: u32) {
    let bytes = get(name);
    let file = crate::store::images_dir().map(|d| d.join(name)).filter(|p| bytes.is_none() && p.exists());
    if bytes.is_none() && file.is_none() {
        return; // nothing to show: the placeholder stays (name kept)
    }
    let gen = GEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    LATEST.with(|l| l.borrow_mut().insert((note as isize, name.to_string()), gen));
    let mut req = Some(Req { gen, note: note as isize, name: name.to_string(), bytes, file, max_css_w, dpi });
    let Ok(mut slot) = DECODER.lock() else { return };
    if let Some(tx) = slot.as_ref() {
        match tx.send(req.take().unwrap()) {
            Ok(()) => return,
            Err(e) => req = Some(e.0), // the thread ended after 60 s idle: start a new one
        }
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let _ = tx.send(req.take().unwrap());
    if std::thread::Builder::new().name("gm-decode".into()).spawn(move || decoder(rx)).is_ok() {
        *slot = Some(tx);
    }
}

/// The decoding thread (MTA, WIC only): reads the picture, scales it to the display size
/// and hands the encoded copy to the UI. Ends after 60 s without work.
fn decoder(rx: std::sync::mpsc::Receiver<Req>) {
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
    }
    while let Ok(req) = rx.recv_timeout(std::time::Duration::from_secs(60)) {
        let bytes = match (&req.bytes, &req.file) {
            (Some(b), _) => Some(b.clone()),
            (None, Some(p)) => std::fs::metadata(p)
                .ok()
                .filter(|m| m.len() as usize <= MAX_BYTES)
                .and_then(|_| std::fs::read(p).ok())
                .filter(|b| sniff(b).is_some())
                .map(Arc::new),
            _ => None,
        };
        let Some(bytes) = bytes else { continue };
        let Some((s, cw, ch, natural_w)) = display_stream(&bytes, req.max_css_w, req.dpi) else { continue };
        let encoded = unsafe {
            let mut st: STATSTG = std::mem::zeroed();
            if s.Stat(&mut st, STATFLAG_NONAME).is_err() {
                continue;
            }
            stream_bytes(&s, st.cbSize)
        };
        let Some(encoded) = encoded else { continue };
        let msg = Box::into_raw(Box::new(Decoded { name: req.name, bytes: Arc::new(encoded), css: (cw, ch), natural_w, gen: req.gen }));
        let posted = unsafe { windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(req.note as _, crate::note::WM_APP_DECODED, 0, msg as isize) };
        if posted == 0 {
            drop(unsafe { Box::from_raw(msg) }); // the note closed meanwhile
        }
    }
    // idle: let the next request start a new thread
    if let Ok(mut slot) = DECODER.lock() {
        *slot = None;
    }
}

/// UI side of `Decoded`: every placeholder (or earlier copy) of that picture in `doc`
/// becomes the picture.
pub fn show_decoded(note: windows_sys::Win32::Foundation::HWND, doc: &ITextDocument2, positions: &[i32], d: &Decoded) {
    replace(doc, positions, &d.name, &d.bytes, d.css);
    SHOWN.with(|m| {
        m.borrow_mut().insert((note as isize, d.name.clone()), Shown { bytes: d.bytes.clone(), natural_w: d.natural_w, css: d.css, sharp: true, source: None });
    });
}

fn replace(doc: &ITextDocument2, positions: &[i32], name: &str, bytes: &[u8], css: (f64, f64)) {
    for &cp in positions.iter().rev() {
        unsafe {
            if let Ok(r) = doc.Range2(cp, cp + 1) {
                let _ = r.SetText(&BSTR::new());
            }
            if let Some(s) = SHCreateMemStream(Some(bytes)) {
                insert_stream(doc, cp, name, &s, css.0, css.1);
            }
        }
    }
}

// ------------------------------------------------------------------ following a resize

/// What a note shows for each of its pictures (UI thread only).
struct Shown {
    /// the display copy on screen (from the decoding thread)
    bytes: Arc<Vec<u8>>,
    natural_w: f64,
    /// CSS size shown now
    css: (f64, f64),
    /// false while a quick rescale is shown (the sharp copy is asked for once the size settles)
    sharp: bool,
    /// the display copy decoded, kept only while the window is being resized
    source: Option<IWICBitmap>,
}

thread_local! {
    static SHOWN: RefCell<HashMap<(isize, String), Shown>> = RefCell::new(HashMap::new());
}

/// v1.1 `max-width: 100%`: the CSS width a picture takes in a text width of `width`
/// (the same rule as `display_stream`).
fn fit_width(natural_w: f64, width: f64) -> f64 {
    natural_w.min(width.max(16.0))
}

/// True when some picture of `note` would change size at text width `width`.
pub fn needs_live(note: windows_sys::Win32::Foundation::HWND, width: f64) -> bool {
    SHOWN.with(|m| m.borrow().iter().any(|((n, _), s)| *n == note as isize && (fit_width(s.natural_w, width) - s.css.0).abs() >= 0.5))
}

/// True when picture `name` of `note` needs a new sharp copy from the decoding thread at
/// text width `width` (never shown yet, a quick rescale on screen, or another size).
pub fn needs_sharp(note: windows_sys::Win32::Foundation::HWND, name: &str, width: f64) -> bool {
    SHOWN.with(|m| m.borrow().get(&(note as isize, name.to_string())).is_none_or(|s| !s.sharp || (fit_width(s.natural_w, width) - s.css.0).abs() >= 0.5))
}

/// While the window is being resized: each picture whose width changes is rescaled at once
/// from the copy on screen (no decoding-thread round trip, no wait) and replaced. The quick
/// copy is an uncompressed BMP with transparency laid on `paper`; the sharp copy follows
/// once the size settles (`needs_sharp`). Pictures are given as (position, name).
pub fn live(note: windows_sys::Win32::Foundation::HWND, doc: &ITextDocument2, found: &[(i32, String)], width: f64, dpi: u32, paper: u32) {
    let Some(wic) = wic() else { return };
    let scale = dpi as f64 / 96.0;
    let mut names: Vec<&str> = Vec::new();
    for (_, n) in found {
        if !names.contains(&n.as_str()) {
            names.push(n);
        }
    }
    for name in names {
        let key = (note as isize, name.to_string());
        let Some((bytes, natural_w, css, source)) = SHOWN.with(|m| m.borrow().get(&key).map(|s| (s.bytes.clone(), s.natural_w, s.css, s.source.clone()))) else { continue };
        let w = fit_width(natural_w, width);
        if (w - css.0).abs() < 0.5 || css.0 <= 0.0 {
            continue;
        }
        let h = w * css.1 / css.0;
        let source = match source {
            Some(s) => s,
            None => {
                let Some(s) = (unsafe { decode_bitmap(&wic, &bytes) }) else { continue };
                SHOWN.with(|m| {
                    if let Some(e) = m.borrow_mut().get_mut(&key) {
                        e.source = Some(s.clone());
                    }
                });
                s
            }
        };
        let (tw, th) = (((w * scale).round() as u32).max(1), ((h * scale).round() as u32).max(1));
        let Some(bmp) = (unsafe { quick_bmp(&wic, &source, tw, th, paper) }) else { continue };
        let positions: Vec<i32> = found.iter().filter(|(_, n)| n == name).map(|(cp, _)| *cp).collect();
        replace(doc, &positions, name, &bmp, (w, h));
        // a sharp copy decoded for an earlier size must not come back over this one
        let gen = GEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        LATEST.with(|l| l.borrow_mut().insert(key.clone(), gen));
        SHOWN.with(|m| {
            if let Some(e) = m.borrow_mut().get_mut(&key) {
                e.css = (w, h);
                e.sharp = false;
            }
        });
    }
}

/// The resize is over: the decoded copies kept for it are let go.
pub fn end_live(note: windows_sys::Win32::Foundation::HWND) {
    SHOWN.with(|m| m.borrow_mut().iter_mut().filter(|((n, _), _)| *n == note as isize).for_each(|(_, s)| s.source = None));
}

/// The note window is gone.
pub fn forget_note(note: windows_sys::Win32::Foundation::HWND) {
    SHOWN.with(|m| m.borrow_mut().retain(|(n, _), _| *n != note as isize));
    LATEST.with(|l| l.borrow_mut().retain(|(n, _), _| *n != note as isize));
}

unsafe fn decode_bitmap(wic: &IWICImagingFactory, bytes: &[u8]) -> Option<IWICBitmap> {
    let s = SHCreateMemStream(Some(bytes))?;
    let dec = wic.CreateDecoderFromStream(&s, std::ptr::null(), WICDecodeMetadataCacheOnDemand).ok()?;
    let frame = dec.GetFrame(0).ok()?;
    let conv = wic.CreateFormatConverter().ok()?;
    conv.Initialize(&frame, &GUID_WICPixelFormat32bppBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom).ok()?;
    wic.CreateBitmapFromSource(&conv, WICBitmapCacheOnLoad).ok()
}

/// `src` scaled to `w`×`h` (bilinear) as a 32-bit BMP file, transparency laid on `paper`
/// (a COLORREF) — what RichEdit shows over the note's paper anyway.
unsafe fn quick_bmp(wic: &IWICImagingFactory, src: &IWICBitmap, w: u32, h: u32, paper: u32) -> Option<Vec<u8>> {
    let scaler = wic.CreateBitmapScaler().ok()?;
    scaler.Initialize(src, w, h, WICBitmapInterpolationModeLinear).ok()?;
    let stride = (w as usize).checked_mul(4)?;
    let size = stride.checked_mul(h as usize)?;
    if size > u32::MAX as usize - 54 {
        return None;
    }
    let mut bmp = vec![0u8; 54 + size];
    scaler.CopyPixels(std::ptr::null(), stride as u32, &mut bmp[54..]).ok()?;
    let (pr, pg, pb) = (paper & 0xFF, (paper >> 8) & 0xFF, (paper >> 16) & 0xFF);
    for p in bmp[54..].chunks_exact_mut(4) {
        let a = p[3] as u32;
        if a != 255 {
            p[0] = ((p[0] as u32 * a + pb * (255 - a)) / 255) as u8;
            p[1] = ((p[1] as u32 * a + pg * (255 - a)) / 255) as u8;
            p[2] = ((p[2] as u32 * a + pr * (255 - a)) / 255) as u8;
            p[3] = 255;
        }
    }
    // BITMAPFILEHEADER + BITMAPINFOHEADER: top-down, 32 bpp, BI_RGB
    bmp[0..2].copy_from_slice(b"BM");
    bmp[2..6].copy_from_slice(&((54 + size) as u32).to_le_bytes());
    bmp[10..14].copy_from_slice(&54u32.to_le_bytes());
    bmp[14..18].copy_from_slice(&40u32.to_le_bytes());
    bmp[18..22].copy_from_slice(&(w as i32).to_le_bytes());
    bmp[22..26].copy_from_slice(&(-(h as i32)).to_le_bytes());
    bmp[26..28].copy_from_slice(&1u16.to_le_bytes());
    bmp[28..30].copy_from_slice(&32u16.to_le_bytes());
    bmp[34..38].copy_from_slice(&(size as u32).to_le_bytes());
    Some(bmp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rich::is_image_name;

    #[test]
    fn sniffs_like_v11() {
        assert_eq!(sniff(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A]), Some("png"));
        assert_eq!(sniff(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("jpg"));
        assert_eq!(sniff(b"GIF89a"), Some("gif"));
        assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 "), Some("webp"));
        assert_eq!(sniff(b"MZ\x90\0"), None);
        assert_eq!(sniff(b"BM"), None);
    }

    #[test]
    fn names_follow_the_v11_rule() {
        assert!(is_image_name(&new_name("png")));
        assert_ne!(new_name("png"), new_name("png"));
    }
}
