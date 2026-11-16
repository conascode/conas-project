//! Reads the RTF a RichEdit body streams out (EM_STREAMOUT, SF_RTF) into a [`rich::Doc`]:
//! one pass, so saving a 200,000 character note stays within the report §5.4 budget (TOM
//! run walking took ~200 ms). Only what a body can hold is read: text, b/i/u/s, bullet
//! paragraphs, line breaks and PT-D′ pictures (name = `wzDescription`). Everything else
//! (fonts, colours, sizes, other destinations) is skipped.

use crate::rich::{self, Doc, Item, Para, B, I, S, U};
use std::collections::HashMap;

#[derive(Clone, Copy)]
struct St {
    bits: u8,
    font: i32,
    uc: i32,
}

#[derive(PartialEq)]
enum Mode {
    Text,
    Skip,
    FontTbl,
    Pict,
}

/// Windows code page of an RTF \fcharset.
fn charset_cp(cs: i32) -> u32 {
    match cs {
        0 | 1 | 2 => 1252,
        77 => 10000,
        128 => 932,
        129 => 949,
        130 => 1361,
        134 => 936,
        136 => 950,
        161 => 1253,
        162 => 1254,
        163 => 1258,
        177 => 1255,
        178 => 1256,
        186 => 1257,
        204 => 1251,
        222 => 874,
        238 => 1250,
        _ => 1252,
    }
}

fn decode(cp: u32, bytes: &[u8]) -> Vec<u16> {
    if bytes.is_empty() {
        return Vec::new();
    }
    unsafe {
        use windows_sys::Win32::Globalization::MultiByteToWideChar;
        let n = MultiByteToWideChar(cp, 0, bytes.as_ptr(), bytes.len() as i32, std::ptr::null_mut(), 0);
        if n <= 0 {
            return bytes.iter().map(|&b| b as u16).collect();
        }
        let mut out = vec![0u16; n as usize];
        MultiByteToWideChar(cp, 0, bytes.as_ptr(), bytes.len() as i32, out.as_mut_ptr(), n);
        out
    }
}

struct Out {
    doc: Doc,
    para: Para,
    run: Vec<u16>,
    run_bits: u8,
}

impl Out {
    fn line(&mut self) -> &mut Vec<Item> {
        self.para.lines.last_mut().unwrap()
    }
    fn flush(&mut self) {
        if !self.run.is_empty() {
            let t = String::from_utf16_lossy(&self.run);
            let bits = self.run_bits;
            self.run.clear();
            self.line().push(Item::Text(t, bits));
        }
    }
    fn chars(&mut self, units: &[u16], bits: u8) {
        if units.is_empty() {
            return;
        }
        if !self.run.is_empty() && bits != self.run_bits {
            self.flush();
        }
        self.run_bits = bits;
        self.run.extend_from_slice(units);
    }
    fn line_break(&mut self) {
        self.flush();
        self.para.lines.push(Vec::new());
    }
    fn par(&mut self, bullet: bool) {
        self.flush();
        self.para.bullet = bullet;
        let p = std::mem::replace(&mut self.para, Para { bullet: false, lines: vec![Vec::new()] });
        self.doc.paras.push(p);
    }
    fn image(&mut self, name: &str) {
        self.flush();
        if rich::is_image_name(name) {
            self.line().push(Item::Img(name.to_string()));
        }
    }
}

/// The body document from its RTF (the story's final paragraph mark ends the last paragraph).
pub fn read(rtf: &[u8]) -> Doc {
    let mut out = Out { doc: Doc::default(), para: Para { bullet: false, lines: vec![Vec::new()] }, run: Vec::new(), run_bits: 0 };
    let mut stack: Vec<(St, Mode, usize)> = Vec::new(); // saved state, mode, and the depth it began
    let mut st = St { bits: 0, font: 0, uc: 1 };
    let mut mode = Mode::Text;
    let mut ansi_cp = 1252u32;
    let mut fonts: HashMap<i32, u32> = HashMap::new();
    let mut tbl_font = 0i32;
    let mut bullet = false;
    let mut pending: Vec<u8> = Vec::new(); // \'hh bytes waiting for their code page
    let mut skip_fallback = 0i32;
    // picture metadata
    let mut sn = String::new();
    let mut sv: Option<String> = None;
    let mut pict_name = String::new();
    let mut capture: Option<(usize, u8)> = None; // (depth, 1 = \sn text, 2 = \sv text)
    let mut cap_text = String::new();

    let b = rtf;
    let mut i = 0usize;
    let cp_of = |font: i32, fonts: &HashMap<i32, u32>, ansi: u32| *fonts.get(&font).unwrap_or(&ansi);
    macro_rules! flush_pending {
        () => {
            if !pending.is_empty() {
                let units = decode(cp_of(st.font, &fonts, ansi_cp), &pending);
                pending.clear();
                if mode == Mode::Text {
                    out.chars(&units, st.bits);
                }
            }
        };
    }
    while i < b.len() {
        let c = b[i];
        match c {
            b'{' => {
                flush_pending!();
                stack.push((st, std::mem::replace(&mut mode, Mode::Text), stack.len()));
                // inherit the outer mode (skip / pict / font table continue inside)
                mode = match stack.last().unwrap().1 {
                    Mode::Skip => Mode::Skip,
                    Mode::Pict => Mode::Pict,
                    Mode::FontTbl => Mode::FontTbl,
                    Mode::Text => Mode::Text,
                };
                i += 1;
                // \* destination: skipped unless known
                if b[i..].starts_with(b"\\*") && mode != Mode::Pict {
                    let rest = &b[i + 2..];
                    let word: Vec<u8> = rest.iter().skip(1).take_while(|c| c.is_ascii_alphabetic()).copied().collect();
                    match word.as_slice() {
                        b"shppict" => {}
                        b"pn" => {
                            bullet = true;
                            mode = Mode::Skip;
                        }
                        _ => mode = Mode::Skip,
                    }
                }
            }
            b'}' => {
                flush_pending!();
                if let Some((d, what)) = capture {
                    if stack.len() == d {
                        if what == 1 {
                            sn = cap_text.trim().to_string();
                        } else if sn == "wzDescription" {
                            sv = Some(cap_text.trim().to_string());
                        }
                        capture = None;
                        cap_text.clear();
                    }
                }
                let was_pict = mode == Mode::Pict;
                if let Some((saved, saved_mode, _)) = stack.pop() {
                    st = saved;
                    mode = saved_mode;
                }
                if was_pict && mode != Mode::Pict {
                    // the picture group closed
                    if let Some(n) = sv.take() {
                        pict_name = n;
                    }
                    let name = std::mem::take(&mut pict_name);
                    out.image(&name);
                    sn.clear();
                }
                i += 1;
                if stack.is_empty() {
                    break; // the document group closed (RichEdit adds a NUL after it)
                }
            }
            0 => i += 1,
            b'\\' => {
                i += 1;
                if i >= b.len() {
                    break;
                }
                let s = b[i];
                if s.is_ascii_alphabetic() {
                    let ws = i;
                    while i < b.len() && b[i].is_ascii_alphabetic() {
                        i += 1;
                    }
                    let word = &b[ws..i];
                    let ps = i;
                    if i < b.len() && (b[i] == b'-' || b[i].is_ascii_digit()) {
                        i += 1;
                        while i < b.len() && b[i].is_ascii_digit() {
                            i += 1;
                        }
                    }
                    let param: Option<i32> = std::str::from_utf8(&b[ps..i]).ok().and_then(|p| p.parse().ok());
                    if i < b.len() && b[i] == b' ' {
                        i += 1;
                    }
                    if word != b"u" {
                        flush_pending!();
                    }
                    if skip_fallback > 0 && word != b"u" {
                        // a control word counts as one fallback character only as \'hh; others end it
                        skip_fallback = 0;
                    }
                    let on = param.unwrap_or(1) != 0;
                    match mode {
                        Mode::Skip => {}
                        Mode::FontTbl => match word {
                            b"f" => tbl_font = param.unwrap_or(0),
                            b"fcharset" => {
                                fonts.insert(tbl_font, charset_cp(param.unwrap_or(0)));
                            }
                            _ => {}
                        },
                        Mode::Pict => match word {
                            b"sn" => {
                                capture = Some((stack.len(), 1));
                                cap_text.clear();
                            }
                            b"sv" => {
                                capture = Some((stack.len(), 2));
                                cap_text.clear();
                            }
                            _ => {}
                        },
                        Mode::Text => match word {
                            b"ansicpg" => ansi_cp = param.unwrap_or(1252).max(0) as u32,
                            b"fonttbl" => mode = Mode::FontTbl,
                            b"colortbl" | b"stylesheet" | b"info" | b"pntext" | b"nonshppict" | b"listtable" | b"listoverridetable" | b"rsidtbl" | b"generator" | b"themedata" | b"latentstyles" | b"fldinst" => mode = Mode::Skip,
                            b"pict" => {
                                mode = Mode::Pict;
                                pict_name.clear();
                                sv = None;
                                sn.clear();
                            }
                            b"f" => st.font = param.unwrap_or(0),
                            b"uc" => st.uc = param.unwrap_or(1),
                            b"plain" => st.bits = 0,
                            b"b" => st.bits = if on { st.bits | B } else { st.bits & !B },
                            b"i" => st.bits = if on { st.bits | I } else { st.bits & !I },
                            b"strike" | b"striked" => st.bits = if on { st.bits | S } else { st.bits & !S },
                            b"ulnone" => st.bits &= !U,
                            w if w.starts_with(b"ul") && !w.starts_with(b"ulc") => st.bits = if on { st.bits | U } else { st.bits & !U },
                            b"pard" => bullet = false,
                            b"pnlvlblt" | b"pnlvlbody" => bullet = true,
                            b"par" => out.par(bullet),
                            b"line" => out.line_break(),
                            b"tab" => out.chars(&[0x09], st.bits),
                            b"emdash" => out.chars(&[0x2014], st.bits),
                            b"endash" => out.chars(&[0x2013], st.bits),
                            b"lquote" => out.chars(&[0x2018], st.bits),
                            b"rquote" => out.chars(&[0x2019], st.bits),
                            b"ldblquote" => out.chars(&[0x201C], st.bits),
                            b"rdblquote" => out.chars(&[0x201D], st.bits),
                            b"bullet" => out.chars(&[0x2022], st.bits),
                            b"enspace" => out.chars(&[0x2002], st.bits),
                            b"emspace" => out.chars(&[0x2003], st.bits),
                            b"zwj" => out.chars(&[0x200D], st.bits),
                            b"zwnj" => out.chars(&[0x200C], st.bits),
                            b"u" => {
                                flush_pending!();
                                let v = param.unwrap_or(0);
                                let u = if v < 0 { (v + 65536) as u16 } else { v as u16 };
                                out.chars(&[u], st.bits);
                                skip_fallback = st.uc;
                            }
                            _ => {}
                        },
                    }
                    if mode == Mode::Pict && word == b"u" {
                        skip_fallback = st.uc;
                    }
                } else {
                    // control symbols
                    i += 1;
                    match s {
                        b'\'' => {
                            let nib = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
                            let hex = match (b.get(i).copied().and_then(nib), b.get(i + 1).copied().and_then(nib)) {
                                (Some(h), Some(l)) => Some(h << 4 | l),
                                _ => None,
                            };
                            i += 2;
                            if skip_fallback > 0 {
                                skip_fallback -= 1;
                            } else if let Some(v) = hex {
                                match mode {
                                    Mode::Text => pending.push(v),
                                    Mode::Pict if capture.is_some() => cap_text.push(v as char),
                                    _ => {}
                                }
                            }
                        }
                        b'~' if mode == Mode::Text => {
                            flush_pending!();
                            out.chars(&[0xA0], st.bits);
                        }
                        b'_' if mode == Mode::Text => {
                            flush_pending!();
                            out.chars(&[0x2011], st.bits);
                        }
                        b'\\' | b'{' | b'}' => {
                            flush_pending!();
                            if skip_fallback > 0 {
                                skip_fallback -= 1;
                            } else if mode == Mode::Text {
                                out.chars(&[s as u16], st.bits);
                            } else if capture.is_some() {
                                cap_text.push(s as char);
                            }
                        }
                        b'\r' | b'\n' if mode == Mode::Text => {
                            flush_pending!();
                            out.par(bullet);
                        }
                        _ => {}
                    }
                }
            }
            b'\r' | b'\n' => i += 1,
            _ => {
                // a plain character (ASCII, or a raw code page byte)
                if skip_fallback > 0 {
                    skip_fallback -= 1;
                } else {
                    match mode {
                        Mode::Text => {
                            if c < 0x80 {
                                flush_pending!();
                                out.chars(&[c as u16], st.bits);
                            } else {
                                pending.push(c);
                            }
                        }
                        Mode::FontTbl => {}
                        Mode::Pict if capture.is_some() => cap_text.push(c as char),
                        _ => {}
                    }
                }
                i += 1;
            }
        }
    }
    out.flush();
    // text after the last \par (none for a whole-story stream) still forms a paragraph
    if out.para.lines.iter().any(|l| !l.is_empty()) || out.para.lines.len() > 1 {
        out.par(bullet);
    }
    // the story's own end mark is the last \par: nothing to drop; a body whose only
    // paragraph is empty is an empty document
    if out.doc.paras.len() == 1 && out.doc.paras[0].lines.iter().all(|l| l.is_empty()) && out.doc.paras[0].lines.len() == 1 {
        out.doc.paras.clear();
    }
    out.doc
}
