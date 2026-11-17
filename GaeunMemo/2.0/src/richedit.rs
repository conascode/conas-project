//! The RichEdit body as a v1.1 note body (report §6.4, table 6-2, figure 6-2).
//!
//! * [`load`] / [`save`]: [`rich::Doc`] ↔ RichEdit through TOM — paragraphs end in `\r`,
//!   lines in `\v`, b/i/u/s are character effects, bullets are PFN_BULLET paragraphs, pictures
//!   are PT-D′ images whose alt text is the file name.
//! * Paste and drop go through [`Callback::QueryAcceptData`] with v1.1's rules (note.js):
//!   Ctrl+Shift+V = text only; "HTML Format" → allow-list clean → inserted if it has content;
//!   else pictures (files, PNG, bitmap); else plain text. RTF from other programs is never
//!   read (v1.1 only used text/html), OLE objects are refused.
//! * Copy, cut and drag hand out our own data object: "HTML Format" (what v1.1 would keep),
//!   plain text (Rich.plain) and RTF — the RTF is only accepted back when the drop comes from
//!   a GaeunMemo body, so moving text inside a note stays a native RichEdit move.

use crate::body;
use crate::images;
use crate::rich::{self, Doc, Item, Para, B, I, S, U};
use std::cell::RefCell;
use windows::core::{implement, Interface, BOOL, BSTR, HRESULT};
use windows::Win32::Foundation::{DATA_S_SAMEFORMATETC, DV_E_FORMATETC, E_FAIL, E_NOTIMPL, HGLOBAL, OLE_E_ADVISENOTSUPPORTED, S_OK};
use windows::Win32::System::Com::StructuredStorage::IStorage;
use windows::Win32::System::Com::{IAdviseSink, IDataObject, IDataObject_Impl, IEnumFORMATETC, IEnumSTATDATA, DVASPECT_CONTENT, FORMATETC, STGMEDIUM, STGMEDIUM_0, TYMED_HGLOBAL};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::{IOleInPlaceFrame, IOleInPlaceUIWindow, IOleObject, ReleaseStgMedium, DROPEFFECT, DROPEFFECT_NONE, OLEINPLACEFRAMEINFO};
use windows::Win32::System::SystemServices::{MODIFIERKEYS_FLAGS, RECO_FLAGS};
use windows::Win32::UI::Controls::RichEdit::*;
use windows::Win32::UI::Shell::{DragQueryFileW, SHCreateStdEnumFmtEtc, HDROP};
use windows::Win32::UI::WindowsAndMessaging::HMENU;
use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::SendMessageW;

const OBJ: u16 = 0xFFFC;
const CR: u16 = 0x0D;
const VT: u16 = 0x0B;
const CF_UNICODETEXT: u16 = 13;
const CF_DIB: u16 = 8;
const CF_HDROP: u16 = 15;
const CF_DIBV5: u16 = 17;
const RECO_PASTE: u32 = 0;
const RECO_DROP: u32 = 1;
const RECO_DRAG: u32 = 4;

fn cf(name: &str) -> u16 {
    let w = crate::win::wide(name);
    unsafe { windows_sys::Win32::System::DataExchange::RegisterClipboardFormatW(w.as_ptr()) as u16 }
}

thread_local! {
    /// Ctrl+Shift+V: the next paste takes plain text only
    static PLAIN_PASTE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// the data object of the drag that started in one of our bodies
    static DRAG_OBJ: RefCell<Option<IDataObject>> = const { RefCell::new(None) };
}

pub fn paste_plain(h: HWND) {
    PLAIN_PASTE.with(|p| p.set(true));
    unsafe { SendMessageW(h, windows_sys::Win32::UI::WindowsAndMessaging::WM_PASTE, 0, 0) };
    PLAIN_PASTE.with(|p| p.set(false));
}

/// `tdoc` for other modules (images::warm).
pub fn tdoc_for(h: HWND) -> Option<ITextDocument2> {
    tdoc(h)
}

fn tdoc(h: HWND) -> Option<ITextDocument2> {
    body::tom(h).map(|(_, d)| d)
}

fn story_len(d: &ITextDocument2) -> i32 {
    unsafe { d.Range2(0, 0).and_then(|r| r.GetStoryLength()).unwrap_or(1) }
}

/// Text width in CSS px (the `max-width: 100%` of a picture).
pub fn text_css_width(h: HWND) -> f64 {
    unsafe {
        let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        SendMessageW(h, windows_sys::Win32::UI::Controls::EM_GETRECT, 0, &mut r as *mut _ as isize);
        let dpi = GetDpiForWindow(h).max(96);
        ((r.right - r.left).max(16) as f64) * 96.0 / dpi as f64
    }
}

// ------------------------------------------------------------------ Doc → RichEdit

#[derive(Default)]
struct Layout {
    text: Vec<u16>,
    runs: Vec<(usize, usize, u8)>,
    bullets: Vec<(usize, usize)>,
    images: Vec<(usize, String)>,
}

fn layout(doc: &Doc) -> Layout {
    let mut l = Layout::default();
    for (pi, p) in doc.paras.iter().enumerate() {
        if pi > 0 {
            l.text.push(CR);
        }
        let start = l.text.len();
        for (li, line) in p.lines.iter().enumerate() {
            if li > 0 {
                l.text.push(VT);
            }
            for it in line {
                match it {
                    Item::Text(t, bits) => {
                        let a = l.text.len();
                        l.text.extend(t.encode_utf16());
                        if *bits != 0 {
                            l.runs.push((a, l.text.len(), *bits));
                        }
                    }
                    Item::Img(n) => {
                        l.images.push((l.text.len(), n.clone()));
                        l.text.push(OBJ);
                    }
                }
            }
        }
        if p.bullet {
            l.bullets.push((start, l.text.len()));
        }
    }
    l
}

unsafe fn charformat() -> CHARFORMAT2W {
    let mut cf: CHARFORMAT2W = std::mem::zeroed();
    cf.Base.cbSize = std::mem::size_of::<CHARFORMAT2W>() as u32;
    cf
}

unsafe fn paraformat() -> PARAFORMAT2 {
    let mut pf: PARAFORMAT2 = std::mem::zeroed();
    pf.Base.cbSize = std::mem::size_of::<PARAFORMAT2>() as u32;
    pf
}

unsafe fn select(h: HWND, a: i32, b: i32) {
    let r = CHARRANGE { cpMin: a, cpMax: b };
    SendMessageW(h, EM_EXSETSEL, 0, &r as *const _ as isize);
}

fn get_sel(h: HWND) -> (i32, i32) {
    let mut r = CHARRANGE { cpMin: 0, cpMax: 0 };
    unsafe { SendMessageW(h, EM_EXGETSEL, 0, &mut r as *mut _ as isize) };
    (r.cpMin.min(r.cpMax), r.cpMin.max(r.cpMax))
}

/// v1.1 `.body ul { padding-left: 1.3em }`: bullet paragraphs.
pub unsafe fn set_bullet(h: HWND, a: i32, b: i32, on: bool) {
    select(h, a, b);
    let mut set = paraformat();
    set.Base.dwMask = PFM_NUMBERING | PFM_OFFSET;
    set.Base.wNumbering = if on { PFN_BULLET } else { PARAFORMAT_NUMBERING(0) };
    set.Base.dxOffset = if on { (body::font_size(h) as f32 * 1.3 * 15.0) as i32 } else { 0 };
    SendMessageW(h, EM_SETPARAFORMAT, 0, &set as *const _ as isize);
}

/// After a text-size change: bullet indents follow the size (1.3 em).
pub fn rescale_bullets(h: HWND) {
    let Some(d) = tdoc(h) else { return };
    let text = story_text(&d);
    let (sa, sb) = get_sel(h);
    let mut start = 0usize;
    unsafe {
        for k in 0..=text.len() {
            if k == text.len() || text[k] == CR {
                let on = d
                    .Range2(start as i32, start as i32)
                    .and_then(|r| r.GetPara())
                    .and_then(|p| p.GetListType())
                    .is_ok_and(|t| t != tomNone.0 && t != tomUndefined.0);
                if on {
                    set_bullet(h, start as i32, k as i32, true);
                }
                start = k + 1;
            }
        }
        select(h, sa, sb);
    }
}

/// The layout as RTF — one EM_STREAMIN instead of a message per run and paragraph (a 200k
/// character note took seconds that way). RTF is only the in-memory transport here; notes are
/// stored as v1.1 HTML.
fn rtf_of(h: HWND, l: &Layout) -> Vec<u8> {
    use std::fmt::Write as _;
    let size = body::font_size(h) as f32;
    let off = (size * 1.3 * 15.0) as i32;
    let sl = (size * 1.6 * 15.0).round() as i32;
    let ink = crate::theme::pal(crate::app::theme()).ink;
    let mut bits = vec![0u8; l.text.len()];
    for &(a, b, x) in &l.runs {
        bits[a..b].fill(x);
    }
    let mut bullet = vec![false; l.text.len() + 1];
    for &(a, _) in &l.bullets {
        bullet[a] = true;
    }
    let mut s = String::with_capacity(l.text.len() * 3 + 256);
    let _ = write!(
        s,
        "{{\\rtf1\\ansi\\deff0\\uc0{{\\fonttbl{{\\f0\\fnil Malgun Gothic;}}{{\\f1\\fnil\\fcharset2 Symbol;}}}}{{\\colortbl ;\\red{}\\green{}\\blue{};}}",
        ink & 0xFF,
        (ink >> 8) & 0xFF,
        (ink >> 16) & 0xFF
    );
    let para = |s: &mut String, at: usize| {
        // exact spacing; paragraphs holding a picture get "at least" in place() (fix_spacing)
        let _ = write!(s, "\\pard\\sl-{sl}\\slmult0");
        if bullet[at] {
            // no {\pntext} fallback group: RichEdit reads \pn, and a merged last paragraph
            // would take the fallback "·\t" as text
            let _ = write!(s, "{{\\*\\pn\\pnlvlblt\\pnf1\\pnindent0{{\\pntxtb\\'B7}}}}\\fi-{off}\\li{off}");
        }
        s.push_str("\\f0\\cf1 ");
    };
    para(&mut s, 0);
    let mut cur = 0u8;
    for (k, &u) in l.text.iter().enumerate() {
        if u != CR && bits[k] != cur {
            for (bit, on, offw) in [(B, "\\b ", "\\b0 "), (I, "\\i ", "\\i0 "), (U, "\\ul ", "\\ulnone "), (S, "\\strike ", "\\strike0 ")] {
                if (bits[k] ^ cur) & bit != 0 {
                    s.push_str(if bits[k] & bit != 0 { on } else { offw });
                }
            }
            cur = bits[k];
        }
        match u {
            CR => {
                s.push_str("\\par\r\n");
                para(&mut s, k + 1);
            }
            VT => s.push_str("\\line "),
            OBJ => s.push_str("\\u-8192 "), // PH = U+E000
            0x09 => s.push_str("\\tab "),
            0x5C => s.push_str("\\\\"),
            0x7B => s.push_str("\\{"),
            0x7D => s.push_str("\\}"),
            0x20..=0x7E => s.push(u as u8 as char),
            _ => {
                let _ = write!(s, "\\u{} ", u as i16);
            }
        }
    }
    s.push('}');
    s.into_bytes()
}

struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

unsafe extern "system" fn rtf_in(cookie: usize, buf: *mut u8, cb: i32, pcb: *mut i32) -> u32 {
    let r = &mut *(cookie as *mut Reader);
    let n = (r.data.len() - r.at).min(cb.max(0) as usize);
    std::ptr::copy_nonoverlapping(r.data.as_ptr().add(r.at), buf, n);
    r.at += n;
    *pcb = n as i32;
    0
}

/// Streams `l` in (over the selection when `selection`), then puts the exact text size,
/// colour and line spacing on it and the pictures in place. Returns the start position.
unsafe fn place(h: HWND, d: &ITextDocument2, l: &Layout, selection: bool) -> i32 {
    let (base, sel_end) = if selection { get_sel(h) } else { (0, story_len(d) - 1) };
    let expected = story_len(d) - (sel_end - base) + l.text.len() as i32;
    let data = rtf_of(h, l);
    let mut reader = Reader { data: &data, at: 0 };
    let es = EDITSTREAM { dwCookie: &mut reader as *mut _ as usize, dwError: 0, pfnCallback: Some(rtf_in) };
    let flags = if selection { SF_RTF | SFF_SELECTION } else { SF_RTF };
    SendMessageW(h, EM_STREAMIN, flags as usize, &es as *const _ as isize);
    // a \par or \line at the very end of the stream is absorbed by the story's end mark
    let last = l.text.last().copied();
    if story_len(d) == expected - 1 && matches!(last, Some(CR) | Some(VT)) {
        let at = base + l.text.len() as i32 - 1;
        if let Ok(r) = d.Range2(at, at) {
            let _ = r.SetText(&BSTR::from_wide(&[last.unwrap()]));
        }
    }
    // exact size / face / colour (RTF \fs is in half points) and line height on the new text
    let end = base + l.text.len() as i32;
    let size = body::font_size(h);
    select(h, base, end);
    let mut cf = charformat();
    cf.Base.dwMask = CFM_FACE | CFM_SIZE | CFM_COLOR;
    cf.Base.yHeight = size as i32 * 15;
    cf.Base.crTextColor = windows::Win32::Foundation::COLORREF(crate::theme::pal(crate::app::theme()).ink);
    for (i, c) in "Malgun Gothic".encode_utf16().enumerate() {
        cf.Base.szFaceName[i] = c;
    }
    SendMessageW(h, EM_SETCHARFORMAT, SCF_SELECTION as usize, &cf as *const _ as isize);
    // the last streamed paragraph merges into the paragraph already there and keeps that
    // paragraph's format: set its bullet explicitly
    let last_start = l.text.iter().rposition(|&u| u == CR).map(|k| k + 1).unwrap_or(0);
    let last_bullet = l.bullets.iter().any(|&(a, _)| a == last_start);
    set_bullet(h, base + last_start as i32, end, last_bullet);
    let width = text_css_width(h);
    let dpi = GetDpiForWindow(h).max(96);
    for (cp, name) in l.images.iter().rev() {
        let at = base + *cp as i32;
        if let Ok(r) = d.Range2(at, at + 1) {
            let _ = r.SetText(&BSTR::new());
        }
        images::insert(d, at, name, width, dpi, windows_sys::Win32::UI::WindowsAndMessaging::GetParent(h));
    }
    fix_spacing(h, base, end);
    base
}

/// v1.1's line-height 1.6 is exact spacing here, except on paragraphs holding a picture:
/// exact spacing clamps a picture's line to text height and squashes the picture, so those
/// get "at least" spacing. ("At least" puts all the extra space above the text — about 2 px
/// lower than v1.1 — so text-only paragraphs keep exact spacing.) Covers the paragraphs
/// touching `a..b`; the selection is kept.
pub unsafe fn fix_spacing(h: HWND, a: i32, b: i32) {
    let Some(d) = tdoc(h) else { return };
    let text = story_text(&d);
    let n = text.len();
    let (mut s, mut e) = (a.clamp(0, n as i32) as usize, b.clamp(0, n as i32) as usize);
    while s > 0 && text[s - 1] != CR {
        s -= 1;
    }
    while e < n && text[e] != CR {
        e += 1;
    }
    let sl = (body::font_size(h) as f32 * 1.6 * 15.0).round() as i32;
    // each paragraph gets its rule once, and only when it has another one: every
    // EM_SETPARAFORMAT re-lays the paragraph out (a picture line measures its picture) —
    // the old "exact for all, then 'at least' for pictures" cost 7–15 ms per pasted picture
    // (P6 paste stall). Display frozen meanwhile: one repaint at the end.
    let set = |a: usize, b: usize, rule: u8| {
        select(h, a as i32, b as i32);
        let mut cur = paraformat();
        SendMessageW(h, EM_GETPARAFORMAT, 0, &mut cur as *mut _ as isize);
        // a range of several paragraphs with mixed rules reports PFM_LINESPACING unset
        if (cur.Base.dwMask & PFM_LINESPACING).0 != 0 && cur.bLineSpacingRule == rule && cur.dyLineSpacing == sl {
            return;
        }
        let mut pf = paraformat();
        pf.Base.dwMask = PFM_LINESPACING;
        pf.bLineSpacingRule = rule;
        pf.dyLineSpacing = sl;
        SendMessageW(h, EM_SETPARAFORMAT, 0, &pf as *const _ as isize);
    };
    let frozen = freeze(h);
    let (sa, sb) = get_sel(h);
    // runs of paragraphs with the same rule, set together
    let mut run: Option<(usize, usize, u8)> = None;
    let mut p = s;
    for k in s..=e {
        if k == e || text[k] == CR {
            let rule = if text[p..k].contains(&OBJ) { 3 } else { 4 };
            run = match run {
                Some((ra, _, rr)) if rr == rule => Some((ra, k, rule)),
                Some((ra, rb, rr)) => {
                    set(ra, rb, rr);
                    Some((p, k, rule))
                }
                None => Some((p, k, rule)),
            };
            p = k + 1;
        }
    }
    if let Some((ra, rb, rr)) = run {
        set(ra, rb, rr);
    }
    select(h, sa, sb);
    if let Some(d) = frozen {
        unfreeze(&d);
    }
}

/// Replaces the whole body with `doc` (no undo history, no change notifications).
pub fn load(h: HWND, doc: &Doc) {
    let Some(d) = tdoc(h) else { return };
    let l = layout(doc);
    unsafe {
        let mask = SendMessageW(h, EM_SETEVENTMASK, 0, 0);
        let _ = d.Undo(tomSuspend.0);
        place(h, &d, &l, false);
        let _ = d.Undo(tomResume.0);
        SendMessageW(h, windows_sys::Win32::UI::Controls::EM_EMPTYUNDOBUFFER, 0, 0);
        select(h, 0, 0);
        SendMessageW(h, EM_SETEVENTMASK, 0, mask);
    }
}

/// Inserts `doc` over the selection as one undo step; the caret ends after it.
pub fn insert_doc(h: HWND, doc: &Doc) {
    let Some(d) = tdoc(h) else { return };
    let l = layout(doc);
    unsafe {
        let _ = d.BeginEditCollection();
        let base = place(h, &d, &l, true);
        let _ = d.EndEditCollection();
        let end = base + l.text.len() as i32;
        select(h, end, end);
    }
}

thread_local! {
    // P6 diagnostic: ms of `show_picture` steps (find, replace, select, unfreeze, KB)
    pub static SHOW_TRACE: std::cell::RefCell<Vec<[f64; 6]>> = const { std::cell::RefCell::new(Vec::new()) };
    // P6 diagnostic: ms per step of each `insert_picture` (text, breaks, insert, spacing,
    // end edit, caret).
    pub static INS_TRACE: std::cell::RefCell<Vec<[f64; 6]>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// A new picture on a line of its own (v1.1 shows pictures as blocks), as one undo step.
pub fn insert_picture(h: HWND, name: &str) {
    let Some(d) = tdoc(h) else { return };
    let (a, b) = get_sel(h);
    let mut tr = [0.0f64; 6];
    let mut tk = std::time::Instant::now();
    let mut lap = |i: usize| {
        tr[i] = tk.elapsed().as_secs_f64() * 1000.0;
        tk = std::time::Instant::now();
    };
    unsafe {
        let _ = d.BeginEditCollection();
        if b > a {
            if let Ok(r) = d.Range2(a, b) {
                let _ = r.SetText(&BSTR::new());
            }
        }
        let text = story_text(&d);
        lap(0);
        let mut cp = a;
        let at_start = cp == 0 || text.get(cp as usize - 1) == Some(&CR);
        let at_end = text.get(cp as usize).is_none_or(|&c| c == CR);
        if !at_start {
            if let Ok(r) = d.Range2(cp, cp) {
                let _ = r.SetText(&BSTR::from("\r"));
            }
            cp += 1;
        }
        if !at_end {
            if let Ok(r) = d.Range2(cp, cp) {
                let _ = r.SetText(&BSTR::from("\r"));
            }
        }
        lap(1);
        images::insert(&d, cp, name, text_css_width(h), GetDpiForWindow(h).max(96), windows_sys::Win32::UI::WindowsAndMessaging::GetParent(h));
        lap(2);
        fix_spacing(h, cp, cp + 1);
        lap(3);
        let _ = d.EndEditCollection();
        lap(4);
        let caret = if at_end { cp + 1 } else { cp + 2 };
        select(h, caret, caret);
        lap(5);
    }
    if crate::metrics::diag() {
        INS_TRACE.with(|t| t.borrow_mut().push(tr));
    }
}

// ------------------------------------------------------------------ RichEdit → Doc

fn story_text(d: &ITextDocument2) -> Vec<u16> {
    unsafe {
        d.Range2(0, story_len(d))
            .and_then(|r| r.GetText2(0))
            .map(|b| b.to_vec())
            .unwrap_or_default()
    }
}

/// The body (or `from..to` of it) as a document.
pub fn extract(h: HWND, from: i32, to: i32) -> Doc {
    let Some(d) = tdoc(h) else { return Doc::default() };
    let all = story_text(&d);
    // the story always ends with a paragraph mark that is not part of the note
    let len = (all.len() as i32 - 1).max(0);
    let (from, to) = (from.clamp(0, len), to.clamp(0, len));
    if to <= from {
        return Doc::default();
    }
    let text = &all[from as usize..to as usize];
    let mut bits = vec![0u8; text.len()];
    unsafe {
        let mut cp = from;
        while cp < to {
            let Ok(r) = d.Range2(cp, cp) else { break };
            let _ = r.MoveEnd(tomCharFormat.0, 1);
            let end = r.GetEnd().unwrap_or(to).min(to).max(cp + 1);
            let mut b = 0u8;
            if let Ok(f) = r.GetFont() {
                if f.GetBold().unwrap_or(0) == tomTrue.0 {
                    b |= B;
                }
                if f.GetItalic().unwrap_or(0) == tomTrue.0 {
                    b |= I;
                }
                let u = f.GetUnderline().unwrap_or(0);
                if u != tomNone.0 && u != tomUndefined.0 {
                    b |= U;
                }
                if f.GetStrikeThrough().unwrap_or(0) == tomTrue.0 {
                    b |= S;
                }
            }
            for k in (cp - from) as usize..(end - from) as usize {
                bits[k] = b;
            }
            cp = end;
        }
    }
    let bullet_at = |cp: i32| -> bool {
        unsafe {
            d.Range2(cp, cp)
                .and_then(|r| r.GetPara())
                .and_then(|p| p.GetListType())
                .map(|t| t != tomNone.0 && t != tomUndefined.0)
                .unwrap_or(false)
        }
    };
    let name_at = |cp: i32| -> String {
        unsafe { d.Range2(cp, cp + 1).and_then(|r| r.GetText2(tomTextize.0)).map(|b| b.to_string()).unwrap_or_default() }
    };
    let mut doc = Doc::default();
    let mut para = Para { bullet: bullet_at(from), lines: vec![Vec::new()] };
    let mut run: Vec<u16> = Vec::new();
    let mut run_bits = 0u8;
    fn flush(run: &mut Vec<u16>, bits: u8, line: &mut Vec<Item>) {
        if !run.is_empty() {
            line.push(Item::Text(String::from_utf16_lossy(run), bits));
            run.clear();
        }
    }
    for (k, &c) in text.iter().enumerate() {
        let cp = from + k as i32;
        match c {
            CR => {
                flush(&mut run, run_bits, para.lines.last_mut().unwrap());
                doc.paras.push(std::mem::take(&mut para));
                para = Para { bullet: bullet_at(cp + 1), lines: vec![Vec::new()] };
            }
            VT => {
                flush(&mut run, run_bits, para.lines.last_mut().unwrap());
                para.lines.push(Vec::new());
            }
            OBJ => {
                flush(&mut run, run_bits, para.lines.last_mut().unwrap());
                let n = name_at(cp);
                if rich::is_image_name(&n) {
                    para.lines.last_mut().unwrap().push(Item::Img(n));
                }
            }
            0x0A => {
                // a stray LF (RichEdit keeps CR) counts as a line break
                flush(&mut run, run_bits, para.lines.last_mut().unwrap());
                para.lines.push(Vec::new());
            }
            _ => {
                if !run.is_empty() && bits[k] != run_bits {
                    flush(&mut run, run_bits, para.lines.last_mut().unwrap());
                }
                run_bits = bits[k];
                run.push(c);
            }
        }
    }
    flush(&mut run, run_bits, para.lines.last_mut().unwrap());
    doc.paras.push(para);
    doc
}

/// A decoded picture arrived: its placeholders in the body become the picture — not an
/// edit (no undo step, no change notification, the selection stays).
pub fn show_picture(h: HWND, d: &images::Decoded) {
    if !images::is_latest(unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetParent(h) }, d) {
        return; // a newer request (another width) is on its way
    }
    let Some(doc) = tdoc(h) else { return };
    let t0 = std::time::Instant::now();
    let text = story_text(&doc);
    let positions: Vec<i32> = text
        .iter()
        .enumerate()
        .filter(|&(_, &u)| u == OBJ)
        .map(|(k, _)| k as i32)
        .filter(|&cp| unsafe { doc.Range2(cp, cp + 1).and_then(|r| r.GetText2(tomTextize.0)).map(|b| b.to_string() == d.name).unwrap_or(false) })
        .collect();
    if positions.is_empty() {
        return;
    }
    unsafe {
        // frozen: the body shows the result once, not each delete / insert step
        let t1 = t0.elapsed().as_secs_f64() * 1000.0;
        let frozen = freeze(h);
        let (sa, sb) = get_sel(h);
        let mask = SendMessageW(h, EM_SETEVENTMASK, 0, 0);
        let _ = doc.Undo(tomSuspend.0);
        images::show_decoded(windows_sys::Win32::UI::WindowsAndMessaging::GetParent(h), &doc, &positions, d);
        let t2 = t0.elapsed().as_secs_f64() * 1000.0;
        let _ = doc.Undo(tomResume.0);
        select(h, sa, sb);
        SendMessageW(h, EM_SETEVENTMASK, 0, mask);
        let t3 = t0.elapsed().as_secs_f64() * 1000.0;
        if let Some(f) = frozen {
            unfreeze(&f);
        }
        let t4 = t0.elapsed().as_secs_f64() * 1000.0;
        if crate::metrics::diag() {
            SHOW_TRACE.with(|t| t.borrow_mut().push([t1, t2 - t1, t3 - t2, t4 - t3, d.bytes.len() as f64 / 1024.0, 0.0]));
        }
    }
    crate::p6bench::SHOWN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    images::release_written();
}

/// The pictures in the body as (position, name).
fn body_pictures(doc: &ITextDocument2) -> Vec<(i32, String)> {
    let text = story_text(doc);
    let mut out = Vec::new();
    for (k, &u) in text.iter().enumerate() {
        if u == OBJ {
            let n = unsafe { doc.Range2(k as i32, k as i32 + 1).and_then(|r| r.GetText2(tomTextize.0)).map(|b| b.to_string()).unwrap_or_default() };
            if rich::is_image_name(&n) {
                out.push((k as i32, n));
            }
        }
    }
    out
}

/// The note's text width settled after a change (resize, 전체화면): pictures not shown sharp
/// at the new `max-width: 100%` are asked for again (v1.1 re-flows them). Returns the width.
pub fn refit_pictures(h: HWND, _last_width: f64) -> f64 {
    let width = text_css_width(h);
    let Some(doc) = tdoc(h) else { return width };
    let note = unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetParent(h) };
    let dpi = unsafe { GetDpiForWindow(h).max(96) };
    let mut names: Vec<String> = Vec::new();
    for (_, n) in body_pictures(&doc) {
        if !names.contains(&n) && images::needs_sharp(note, &n, width) {
            names.push(n);
        }
    }
    for n in names {
        images::request(note, &n, width, dpi);
    }
    images::end_live(note);
    width
}

thread_local! {
    /// TOM freezes held now (all bodies live on the UI thread)
    static FROZEN: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Holds the body's display while the window is being resized (see `live_refit`).
pub fn freeze(h: HWND) -> Option<ITextDocument2> {
    let d = tdoc(h)?;
    unsafe { d.Freeze().ok()? };
    FROZEN.with(|f| f.set(f.get() + 1));
    Some(d)
}

pub fn unfreeze(d: &ITextDocument2) {
    FROZEN.with(|f| f.set(f.get().saturating_sub(1)));
    unsafe {
        let _ = d.Unfreeze();
    }
}

/// True while a body is frozen: it must not be painted then — a frozen RichEdit draws
/// nothing, so the paint showed bare paper, a white flash on every resize step (user report
/// 2026-11-16). The update region stays and is painted once the freeze ends.
pub fn is_frozen() -> bool {
    FROZEN.with(|f| f.get() > 0)
}

/// During a resize (each WM_SIZE): pictures capped by the text width follow it at once —
/// v1.1's `max-width: 100%` re-flows them in the same frame. Not an edit: no undo step,
/// no change notification, the selection stays.
pub fn live_refit(h: HWND) {
    thread_local! {
        static BUSY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    // replacing pictures can show or hide the scroll bar, which resizes the body and calls
    // this again from its WM_SIZE: that pass is the loop below
    if BUSY.with(|b| b.replace(true)) {
        return;
    }
    let frozen = freeze(h);
    live_passes(h);
    if let Some(d) = frozen {
        unfreeze(&d);
    }
    // unfreezing can still show or hide the scroll bar (one more width)
    live_passes(h);
    BUSY.with(|b| b.set(false));
}

fn live_passes(h: HWND) {
    let note = unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetParent(h) };
    // smaller pictures can make the scroll bar go (or come), which changes the text width
    // again: refit until it holds, like a browser's layout pass (three passes at most —
    // a body that keeps flipping keeps the last one)
    for _ in 0..3 {
        let width = text_css_width(h);
        if !images::needs_live(note, width) {
            return;
        }
        let Some(doc) = tdoc(h) else { return };
        let found = body_pictures(&doc);
        if found.is_empty() {
            return;
        }
        unsafe {
            let dpi = GetDpiForWindow(h).max(96);
            let (sa, sb) = get_sel(h);
            let mask = SendMessageW(h, EM_SETEVENTMASK, 0, 0);
            let _ = doc.Undo(tomSuspend.0);
            images::live(note, &doc, &found, width, dpi, crate::theme::pal(crate::app::theme()).paper);
            let _ = doc.Undo(tomResume.0);
            select(h, sa, sb);
            SendMessageW(h, EM_SETEVENTMASK, 0, mask);
        }
    }
}

/// The whole body as RTF (EM_STREAMOUT).
/// P6 experiment: the stream with any EM_STREAMOUT flags.
pub fn stream_with(h: HWND, flags: u32) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let es = EDITSTREAM { dwCookie: &mut out as *mut _ as usize, dwError: 0, pfnCallback: Some(rtf_out) };
    unsafe { SendMessageW(h, EM_STREAMOUT, flags as usize, &es as *const _ as isize) };
    out
}

pub fn stream_rtf(h: HWND) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let es = EDITSTREAM { dwCookie: &mut out as *mut _ as usize, dwError: 0, pfnCallback: Some(rtf_out) };
    unsafe { SendMessageW(h, EM_STREAMOUT, SF_RTF as usize, &es as *const _ as isize) };
    out
}

/// The whole body (one RTF stream-out, read in one pass — report §5.4 budget).
///
/// Pictures: RichEdit writes each picture's data into RTF as hex (ten pictures ≈ 470 KB,
/// 10 ms of the UI thread every save — P6 paste stall). The stream is made without object
/// data (`SF_RTFNOOBJS`: a `\objattph` mark per picture) and the names come from TOM, in story
/// order. If the counts disagree, the full stream is read as before.
pub fn save(h: HWND) -> Doc {
    let t0 = std::time::Instant::now();
    let names: Vec<String> = tdoc(h).map(|d| body_pictures(&d).into_iter().map(|(_, n)| n).collect()).unwrap_or_default();
    let (d, kb) = if names.is_empty() {
        let rtf = stream_rtf(h);
        (crate::rtfread::read(&rtf), rtf.len())
    } else {
        let rtf = stream_with(h, (SF_RTFNOOBJS) as u32);
        match crate::rtfread::read_named(&rtf, &names) {
            (d, used) if used == names.len() => (d, rtf.len()),
            _ => {
                crate::log::write("save: picture marks and names differ, full stream used");
                let rtf = stream_rtf(h);
                (crate::rtfread::read(&rtf), rtf.len())
            }
        }
    };
    let t1 = t0.elapsed().as_secs_f64() * 1000.0;
    if crate::metrics::diag() {
        SAVE_TRACE.with(|t| t.borrow_mut().push([t1, 0.0, kb as f64 / 1024.0]));
    }
    d
}

/// `save` the old way, the pictures' data streamed and read (tests compare the two).
pub fn save_full(h: HWND) -> Doc {
    crate::rtfread::read(&stream_rtf(h))
}

thread_local! {
    // P6 diagnostic: ms of `save` steps (stream out, read) and RTF KB
    pub static SAVE_TRACE: std::cell::RefCell<Vec<[f64; 3]>> = const { std::cell::RefCell::new(Vec::new()) };
}

// ------------------------------------------------------------------ clipboard / drag data

fn cf_html(fragment: &str) -> Vec<u8> {
    let pre = "<html><body>\r\n<!--StartFragment-->";
    let post = "<!--EndFragment-->\r\n</body></html>";
    let head_len = "Version:0.9\r\nStartHTML:0000000000\r\nEndHTML:0000000000\r\nStartFragment:0000000000\r\nEndFragment:0000000000\r\n".len();
    let start_html = head_len;
    let start_frag = start_html + pre.len();
    let end_frag = start_frag + fragment.len();
    let end_html = end_frag + post.len();
    format!(
        "Version:0.9\r\nStartHTML:{start_html:010}\r\nEndHTML:{end_html:010}\r\nStartFragment:{start_frag:010}\r\nEndFragment:{end_frag:010}\r\n{pre}{fragment}{post}"
    )
    .into_bytes()
}

unsafe extern "system" fn rtf_out(cookie: usize, buf: *mut u8, cb: i32, pcb: *mut i32) -> u32 {
    let out = &mut *(cookie as *mut Vec<u8>);
    out.extend_from_slice(std::slice::from_raw_parts(buf, cb as usize));
    *pcb = cb;
    0
}

fn selection_rtf(h: HWND) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let es = EDITSTREAM { dwCookie: &mut out as *mut _ as usize, dwError: 0, pfnCallback: Some(rtf_out) };
    unsafe { SendMessageW(h, EM_STREAMOUT, (SF_RTF | SFF_SELECTION) as usize, &es as *const _ as isize) };
    out.push(0);
    out
}

/// What a copy / cut / drag of a body selection puts on offer.
#[implement(IDataObject)]
struct Clip {
    items: Vec<(u16, Vec<u8>)>,
}

impl IDataObject_Impl for Clip_Impl {
    fn GetData(&self, f: *const FORMATETC) -> windows::core::Result<STGMEDIUM> {
        let f = unsafe { &*f };
        let Some((_, data)) = self.items.iter().find(|(c, _)| *c == f.cfFormat) else { return Err(DV_E_FORMATETC.into()) };
        if f.tymed & TYMED_HGLOBAL.0 as u32 == 0 {
            return Err(DV_E_FORMATETC.into());
        }
        unsafe {
            let g = GlobalAlloc(GMEM_MOVEABLE, data.len().max(1))?;
            let p = GlobalLock(g) as *mut u8;
            std::ptr::copy_nonoverlapping(data.as_ptr(), p, data.len());
            let _ = GlobalUnlock(g);
            Ok(STGMEDIUM { tymed: TYMED_HGLOBAL.0 as u32, u: STGMEDIUM_0 { hGlobal: g }, pUnkForRelease: std::mem::ManuallyDrop::new(None) })
        }
    }
    fn GetDataHere(&self, _f: *const FORMATETC, _m: *mut STGMEDIUM) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn QueryGetData(&self, f: *const FORMATETC) -> HRESULT {
        let f = unsafe { &*f };
        if self.items.iter().any(|(c, _)| *c == f.cfFormat) && f.tymed & TYMED_HGLOBAL.0 as u32 != 0 {
            S_OK
        } else {
            DV_E_FORMATETC
        }
    }
    fn GetCanonicalFormatEtc(&self, _i: *const FORMATETC, o: *mut FORMATETC) -> HRESULT {
        if !o.is_null() {
            unsafe { (*o).ptd = std::ptr::null_mut() };
        }
        DATA_S_SAMEFORMATETC
    }
    fn SetData(&self, _f: *const FORMATETC, _m: *const STGMEDIUM, _r: BOOL) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn EnumFormatEtc(&self, dir: u32) -> windows::core::Result<IEnumFORMATETC> {
        if dir != 1 {
            return Err(E_NOTIMPL.into());
        }
        let fmts: Vec<FORMATETC> = self
            .items
            .iter()
            .map(|(c, _)| FORMATETC { cfFormat: *c, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 })
            .collect();
        unsafe { SHCreateStdEnumFmtEtc(&fmts) }
    }
    fn DAdvise(&self, _f: *const FORMATETC, _a: u32, _s: windows::core::Ref<IAdviseSink>) -> windows::core::Result<u32> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }
    fn DUnadvise(&self, _c: u32) -> windows::core::Result<()> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }
    fn EnumDAdvise(&self) -> windows::core::Result<IEnumSTATDATA> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }
}

fn clip_for(h: HWND, a: i32, b: i32) -> IDataObject {
    let doc = extract(h, a, b);
    let mut text: Vec<u16> = doc.plain().encode_utf16().collect();
    text.push(0);
    let text_bytes: Vec<u8> = text.iter().flat_map(|u| u.to_le_bytes()).collect();
    let items = vec![(cf("HTML Format"), cf_html(&doc.to_html())), (CF_UNICODETEXT, text_bytes), (cf("Rich Text Format"), selection_rtf(h))];
    Clip { items }.into()
}

// ------------------------------------------------------------------ reading a data object

fn query(obj: &IDataObject, c: u16) -> bool {
    let f = FORMATETC { cfFormat: c, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 };
    unsafe { obj.QueryGetData(&f) == S_OK }
}

fn global_bytes(obj: &IDataObject, c: u16) -> Option<Vec<u8>> {
    let f = FORMATETC { cfFormat: c, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 };
    unsafe {
        let mut m = obj.GetData(&f).ok()?;
        let g: HGLOBAL = m.u.hGlobal;
        let n = GlobalSize(g);
        let p = GlobalLock(g) as *const u8;
        let v = if p.is_null() { None } else { Some(std::slice::from_raw_parts(p, n).to_vec()) };
        let _ = GlobalUnlock(g);
        ReleaseStgMedium(&mut m);
        v
    }
}

/// Picture files from Explorer (CF_HDROP): pictures by their bytes, at most 25 MB.
fn dropped_files(obj: &IDataObject) -> Vec<Vec<u8>> {
    let f = FORMATETC { cfFormat: CF_HDROP, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 };
    let mut out = Vec::new();
    unsafe {
        let Ok(mut m) = obj.GetData(&f) else { return out };
        let drop = HDROP(m.u.hGlobal.0);
        let n = DragQueryFileW(drop, u32::MAX, None);
        for k in 0..n {
            let len = DragQueryFileW(drop, k, None) as usize;
            let mut buf = vec![0u16; len + 1];
            DragQueryFileW(drop, k, Some(&mut buf));
            buf.truncate(len);
            let path = String::from_utf16_lossy(&buf);
            let ok_size = std::fs::metadata(&path).map(|m| m.is_file() && (m.len() as usize) <= images::MAX_BYTES).unwrap_or(false);
            if ok_size {
                if let Ok(bytes) = std::fs::read(&path) {
                    if images::sniff(&bytes).is_some() {
                        out.push(bytes);
                    }
                }
            }
        }
        ReleaseStgMedium(&mut m);
    }
    out
}

/// Pictures on offer, as stored names (v1.1: image files; a screenshot arrives as PNG).
fn pictures(obj: &IDataObject) -> Vec<String> {
    let mut names: Vec<String> = dropped_files(obj).into_iter().filter_map(images::add).collect();
    if names.is_empty() {
        if let Some(png) = global_bytes(obj, cf("PNG")) {
            names.extend(images::add(png));
        }
    }
    if names.is_empty() {
        for c in [CF_DIBV5, CF_DIB] {
            if let Some(dib) = global_bytes(obj, c) {
                if let Some(n) = images::add_dib(&dib) {
                    names.push(n);
                    break;
                }
            }
        }
    }
    names
}

fn has_pictures(obj: &IDataObject) -> bool {
    query(obj, CF_HDROP) || query(obj, cf("PNG")) || query(obj, CF_DIB) || query(obj, CF_DIBV5)
}

/// v1.1 `markupHasContent`: visible text or a picture after cleaning.
fn has_content(doc: &Doc) -> bool {
    doc.paras.iter().flat_map(|p| p.lines.iter()).flatten().any(|it| match it {
        Item::Text(t, _) => t.chars().any(|c| !c.is_whitespace()),
        Item::Img(_) => true,
    })
}

// ------------------------------------------------------------------ paste / drop routing

/// v1.1 note.js paste rules for data `obj` arriving in body `h`. Ok = RichEdit inserts the
/// format left in `cf_out` itself; Err = refused, or already inserted here.
pub fn route(h: HWND, obj: &IDataObject, cf_out: *mut u16, reco: u32, really: bool, plain_only: bool) -> windows::core::Result<()> {
    if body::is_locked(h) {
        return Err(E_FAIL.into()); // v1.1: a locked note takes nothing
    }
    let ours = DRAG_OBJ.with(|d| d.borrow().as_ref().is_some_and(|o| o.as_raw() == obj.as_raw()));
    let set_cf = |c: u16| {
        if !cf_out.is_null() {
            unsafe { *cf_out = c };
        }
    };
    if ours && reco == RECO_DROP && !plain_only {
        set_cf(cf("Rich Text Format")); // a move / copy between our own bodies
        return Ok(());
    }
    let html = if plain_only { None } else { global_bytes(obj, cf("HTML Format")) };
    if !really {
        // drag feedback only
        return if html.is_some() || query(obj, CF_UNICODETEXT) || (!plain_only && has_pictures(obj)) { Ok(()) } else { Err(E_FAIL.into()) };
    }
    let doc = html.map(|raw| Doc::from_tree(&rich::sanitize(rich::cf_html_fragment(&String::from_utf8_lossy(&raw))))).unwrap_or_default();
    let content = has_content(&doc);
    if !plain_only && !content && has_pictures(obj) {
        let names = pictures(obj);
        if !names.is_empty() {
            for n in names {
                insert_picture(h, &n);
            }
            body::changed(h);
            return Err(E_FAIL.into()); // handled here
        }
    }
    if content {
        insert_doc(h, &doc);
        body::changed(h);
        return Err(E_FAIL.into());
    }
    if query(obj, CF_UNICODETEXT) {
        set_cf(CF_UNICODETEXT); // RichEdit inserts the text with the caret's formatting
        return Ok(());
    }
    Err(E_FAIL.into())
}

/// Test data object (`--mode p2check`): formats by registered name or number.
pub fn test_data(items: Vec<(&str, Vec<u8>)>) -> IDataObject {
    let items = items
        .into_iter()
        .map(|(f, v)| {
            let c = match f {
                "text" => CF_UNICODETEXT,
                "dib" => CF_DIB,
                "hdrop" => CF_HDROP,
                name => cf(name),
            };
            (c, v)
        })
        .collect();
    Clip { items }.into()
}

pub fn text_bytes(s: &str) -> Vec<u8> {
    s.encode_utf16().chain([0]).flat_map(|u| u.to_le_bytes()).collect()
}

pub fn html_bytes(fragment: &str) -> Vec<u8> {
    cf_html(fragment)
}

/// A paste of `obj` as RichEdit does it (`--mode p2check`): our routing, then the text
/// insertion RichEdit would do itself when the routing leaves CF_UNICODETEXT.
pub fn paste_for_test(h: HWND, obj: &IDataObject, plain_only: bool) -> bool {
    let mut f: u16 = 0;
    match route(h, obj, &mut f, RECO_PASTE, true, plain_only) {
        Ok(()) if f == CF_UNICODETEXT => {
            let Some(b) = global_bytes(obj, CF_UNICODETEXT) else { return false };
            let w: Vec<u16> = b.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&u| u != 0).collect();
            let mut z = w.clone();
            z.push(0);
            unsafe { SendMessageW(h, windows_sys::Win32::UI::Controls::EM_REPLACESEL, 1, z.as_ptr() as isize) };
            true
        }
        Ok(()) => true,
        Err(_) => true, // inserted here, or refused
    }
}

/// What a copy of `a..b` hands out (`--mode p2check`): (HTML fragment, text).
pub fn copy_for_test(h: HWND, a: i32, b: i32) -> (String, String) {
    let obj = clip_for(h, a, b);
    let html = global_bytes(&obj, cf("HTML Format")).map(|v| rich::cf_html_fragment(&String::from_utf8_lossy(&v)).to_string()).unwrap_or_default();
    let text = global_bytes(&obj, CF_UNICODETEXT)
        .map(|v| String::from_utf16_lossy(&v.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&u| u != 0).collect::<Vec<_>>()))
        .unwrap_or_default();
    (html, text)
}

// ------------------------------------------------------------------ the callback

#[implement(IRichEditOleCallback)]
pub struct Callback {
    body: HWND,
}

impl Callback {
    pub fn attach(body: HWND) {
        let cb: IRichEditOleCallback = Callback { body }.into();
        unsafe { SendMessageW(body, EM_SETOLECALLBACK, 0, cb.as_raw() as isize) };
    }
}

impl IRichEditOleCallback_Impl for Callback_Impl {
    fn GetNewStorage(&self) -> windows::core::Result<IStorage> {
        Err(E_NOTIMPL.into()) // no embedded OLE objects
    }
    fn GetInPlaceContext(&self, _f: windows::core::OutRef<IOleInPlaceFrame>, _d: windows::core::OutRef<IOleInPlaceUIWindow>, _i: *mut OLEINPLACEFRAMEINFO) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn ShowContainerUI(&self, _s: BOOL) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn QueryInsertObject(&self, _c: *mut windows::core::GUID, _s: windows::core::Ref<IStorage>, _cp: i32) -> windows::core::Result<()> {
        // pictures from InsertImage pass here too; embedded OLE objects still cannot be
        // created because GetNewStorage gives them no storage
        Ok(())
    }
    fn DeleteObject(&self, _o: windows::core::Ref<IOleObject>) -> windows::core::Result<()> {
        Ok(())
    }
    fn QueryAcceptData(&self, obj: windows::core::Ref<IDataObject>, cf_out: *mut u16, reco: RECO_FLAGS, really: BOOL, _m: HGLOBAL) -> windows::core::Result<()> {
        let Some(obj) = obj.as_ref() else { return Err(E_FAIL.into()) };
        let plain_only = reco.0 == RECO_PASTE && PLAIN_PASTE.with(|p| p.get());
        route(self.body, obj, cf_out, reco.0, really.as_bool(), plain_only)
    }
    fn ContextSensitiveHelp(&self, _e: BOOL) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn GetClipboardData(&self, chrg: *mut CHARRANGE, reco: u32, out: windows::core::OutRef<IDataObject>) -> windows::core::Result<()> {
        let h = self.body;
        let (a, b) = if chrg.is_null() { get_sel(h) } else { unsafe { ((*chrg).cpMin.min((*chrg).cpMax), (*chrg).cpMin.max((*chrg).cpMax)) } };
        let obj = clip_for(h, a, b);
        if reco == RECO_DRAG {
            DRAG_OBJ.with(|d| *d.borrow_mut() = Some(obj.clone()));
        }
        out.write(Some(obj))
    }
    fn GetDragDropEffect(&self, _drag: BOOL, _keys: MODIFIERKEYS_FLAGS, effect: *mut DROPEFFECT) -> windows::core::Result<()> {
        if body::is_locked(self.body) && !effect.is_null() {
            unsafe { *effect = DROPEFFECT_NONE };
            return Ok(());
        }
        Err(E_NOTIMPL.into()) // RichEdit's own choice (move, Ctrl = copy)
    }
    fn GetContextMenu(&self, _t: RICH_EDIT_GET_CONTEXT_MENU_SEL_TYPE, _o: windows::core::Ref<IOleObject>, _c: *mut CHARRANGE, _m: *mut HMENU) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into()) // v1.1: no menu on the body (F-28)
    }
}

/// Drag finished (the source's DoDragDrop returned): forget its data object.
pub fn drag_done() {
    DRAG_OBJ.with(|d| *d.borrow_mut() = None);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cf_html_offsets_point_at_the_fragment() {
        let raw = String::from_utf8(cf_html("<b>굵게</b>")).unwrap();
        assert_eq!(rich::cf_html_fragment(&raw), "<b>굵게</b>");
        let start: usize = raw.split("StartHTML:").nth(1).unwrap()[..10].parse().unwrap();
        assert!(raw[start..].starts_with("<html>"));
    }
}

/// Test helper: for each picture whose paragraph is followed by another one, the distance
/// in pixels from the picture's line to the next paragraph (the picture line's height).
pub fn picture_line_heights(h: HWND) -> Vec<i32> {
    let Some(d) = tdoc(h) else { return Vec::new() };
    let text = story_text(&d);
    let y = |cp: usize| unsafe {
        let mut p = windows_sys::Win32::Foundation::POINTL { x: 0, y: 0 };
        SendMessageW(h, 0x00D6 /* EM_POSFROMCHAR */, &mut p as *mut _ as usize, cp as isize);
        p.y
    };
    let mut out = Vec::new();
    for (k, &u) in text.iter().enumerate() {
        if u == OBJ {
            if let Some(e) = text[k..].iter().position(|&c| c == CR).map(|e| k + e + 1) {
                if e < text.len() {
                    out.push(y(e) - y(k));
                }
            }
        }
    }
    out
}

/// Test helper: the on-screen width in pixels of each picture that ends its line.
pub fn picture_widths(h: HWND) -> Vec<i32> {
    let Some(d) = tdoc(h) else { return Vec::new() };
    let text = story_text(&d);
    let x = |cp: usize| unsafe {
        let mut p = windows_sys::Win32::Foundation::POINTL { x: 0, y: 0 };
        SendMessageW(h, 0x00D6 /* EM_POSFROMCHAR */, &mut p as *mut _ as usize, cp as isize);
        p.x
    };
    let mut out = Vec::new();
    for (k, &u) in text.iter().enumerate() {
        if u == OBJ && matches!(text.get(k + 1), Some(&CR) | Some(&VT)) {
            out.push(x(k + 1) - x(k));
        }
    }
    out
}

/// Test helper: TOM freeze count is back to zero (a Freeze now returns 1).
pub fn freeze_balanced(h: HWND) -> bool {
    let Some(d) = tdoc(h) else { return false };
    unsafe {
        let n = d.Freeze().unwrap_or(-1);
        let _ = d.Unfreeze();
        n == 1
    }
}
