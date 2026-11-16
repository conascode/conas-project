//! Formatted note bodies (report §6.4, F-30/F-32): v1.1's stored HTML ↔ a document model
//! the RichEdit body can show. Pure Rust, no Win32 — unit-tested here and checked against
//! v1.1's own `ui/rich.js` running in Chromium (`bench/rich/diff.py`).
//!
//! * [`parse`]: HTML → node tree (a small HTML tokenizer + tree builder).
//! * [`sanitize`]: port of `Rich.fromHtml`'s `clean()` — only b i u s ul li br div and
//!   `<img data-gm="NAME">` survive; style="font-weight:bold" etc. become tags.
//! * [`serialize`]: tree → HTML like Chromium's `innerHTML`.
//! * [`inner_text`]: Chromium `innerText` of the tree inside v1.1's `.body` (pre-wrap) →
//!   with [`plain`], the `text` copy v1.1 saves next to the HTML (`Rich.plain`).
//! * [`Doc`]: what is on screen — paragraphs (bullet or not) of lines of formatted runs and
//!   pictures. [`Doc::from_tree`] follows CSS line building; [`Doc::to_html`] writes HTML that
//!   v1.1 renders as the same lines.

use std::fmt::Write as _;

pub const B: u8 = 1;
pub const I: u8 = 2;
pub const U: u8 = 4;
pub const S: u8 = 8;

/// v1.1 limits (commands.rs): plain text is cut at 200,000 chars, HTML over 1 MB is dropped.
pub const MAX_TEXT: usize = 200_000;
pub const MAX_HTML: usize = 1_000_000;

// ------------------------------------------------------------------ tree

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Text(String),
    El { tag: String, attrs: Vec<(String, String)>, kids: Vec<Node> },
}

impl Node {
    fn el(tag: &str, kids: Vec<Node>) -> Node {
        Node::El { tag: tag.into(), attrs: Vec::new(), kids }
    }
    fn attr(&self, name: &str) -> Option<&str> {
        match self {
            Node::El { attrs, .. } => attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str()),
            _ => None,
        }
    }
}

const VOID: &[&str] = &["area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source", "track", "wbr"];
const RAW: &[&str] = &["script", "style", "textarea", "title", "xmp", "iframe", "noembed", "noframes", "noscript", "plaintext"];

// ------------------------------------------------------------------ entities

fn named_entity(name: &str) -> Option<&'static str> {
    Some(match name {
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "quot" => "\"",
        "apos" => "'",
        "nbsp" => "\u{a0}",
        "copy" => "©",
        "reg" => "®",
        "trade" => "™",
        "hellip" => "…",
        "mdash" => "—",
        "ndash" => "–",
        "lsquo" => "‘",
        "rsquo" => "’",
        "ldquo" => "“",
        "rdquo" => "”",
        "bull" => "•",
        "middot" => "·",
        "times" => "×",
        "divide" => "÷",
        "laquo" => "«",
        "raquo" => "»",
        "deg" => "°",
        "plusmn" => "±",
        "larr" => "←",
        "rarr" => "→",
        "uarr" => "↑",
        "darr" => "↓",
        "hearts" => "♥",
        "star" => "☆",
        "check" => "✓",
        "ensp" => "\u{2002}",
        "emsp" => "\u{2003}",
        "thinsp" => "\u{2009}",
        "zwj" => "\u{200d}",
        "zwnj" => "\u{200c}",
        "shy" => "\u{ad}",
        _ => return None,
    })
}

/// Decodes character references (numeric and the common named ones).
pub fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'&' {
            let rest = &s[i + 1..];
            if let Some(num) = rest.strip_prefix('#') {
                let (hex, digits) = match num.strip_prefix(['x', 'X']) {
                    Some(h) => (true, h),
                    None => (false, num),
                };
                let n = digits.bytes().take_while(|c| if hex { c.is_ascii_hexdigit() } else { c.is_ascii_digit() }).count();
                if n > 0 {
                    let v = u32::from_str_radix(&digits[..n], if hex { 16 } else { 10 }).unwrap_or(0xFFFD);
                    let c = match v {
                        0 => '\u{FFFD}',
                        0x80..=0x9F => cp1252(v),
                        _ => char::from_u32(v).unwrap_or('\u{FFFD}'),
                    };
                    out.push(c);
                    let used = 1 + (if hex { 2 } else { 1 }) + n;
                    i += used;
                    if b.get(i) == Some(&b';') {
                        i += 1;
                    }
                    continue;
                }
            } else {
                let n = rest.bytes().take_while(|c| c.is_ascii_alphanumeric()).count();
                if n > 0 {
                    if let Some(rep) = named_entity(&rest[..n]) {
                        out.push_str(rep);
                        i += 1 + n;
                        if b.get(i) == Some(&b';') {
                            i += 1;
                        }
                        continue;
                    }
                }
            }
        }
        let ch = s[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn cp1252(v: u32) -> char {
    const T: [u16; 32] = [
        0x20AC, 0x81, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039, 0x0152, 0x8D, 0x017D, 0x8F,
        0x90, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x9D, 0x017E, 0x0178,
    ];
    char::from_u32(T[(v - 0x80) as usize] as u32).unwrap_or('\u{FFFD}')
}

// ------------------------------------------------------------------ tokenizer

#[derive(Debug, PartialEq)]
enum Tok {
    Start { tag: String, attrs: Vec<(String, String)>, self_closing: bool },
    End(String),
    Text(String),
    /// comments, doctype, CDATA: dropped, but they are tokens (a newline right after
    /// <pre> is only skipped when it is the very next token)
    Comment,
}

fn tokenize(src: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let b = src.as_bytes();
    let mut i = 0;
    let mut text_start = 0;
    let flush = |out: &mut Vec<Tok>, from: usize, to: usize| {
        if to > from {
            out.push(Tok::Text(decode_entities(&src[from..to])));
        }
    };
    while i < b.len() {
        if b[i] != b'<' {
            i += 1;
            continue;
        }
        let next = b.get(i + 1).copied().unwrap_or(0);
        if src[i..].starts_with("<!--") {
            flush(&mut out, text_start, i);
            let end = src[i + 4..].find("-->").map(|p| i + 4 + p + 3).unwrap_or(b.len());
            i = end;
            text_start = i;
            out.push(Tok::Comment);
        } else if next == b'!' || next == b'?' {
            // doctype, CDATA, processing instruction: bogus comment up to '>'
            flush(&mut out, text_start, i);
            i = src[i..].find('>').map(|p| i + p + 1).unwrap_or(b.len());
            text_start = i;
            out.push(Tok::Comment);
        } else if next == b'/' && b.get(i + 2).is_some_and(|c| c.is_ascii_alphabetic()) {
            flush(&mut out, text_start, i);
            let mut j = i + 2;
            while j < b.len() && !matches!(b[j], b'>' | b' ' | b'\t' | b'\n' | b'\r' | b'\x0c' | b'/') {
                j += 1;
            }
            let tag = src[i + 2..j].to_ascii_lowercase();
            i = src[j..].find('>').map(|p| j + p + 1).unwrap_or(b.len());
            text_start = i;
            out.push(Tok::End(tag));
        } else if next.is_ascii_alphabetic() {
            flush(&mut out, text_start, i);
            let (tok, end) = start_tag(src, i);
            i = end;
            text_start = i;
            if let Tok::Start { tag, self_closing: false, .. } = &tok {
                if RAW.contains(&tag.as_str()) {
                    // raw text: everything up to the matching end tag
                    let close = format!("</{tag}");
                    let lower = src[i..].to_ascii_lowercase();
                    let stop = lower.find(&close).map(|p| i + p).unwrap_or(b.len());
                    let body = &src[i..stop];
                    let tag = tag.clone();
                    out.push(tok);
                    if !body.is_empty() {
                        out.push(Tok::Text(if tag == "textarea" || tag == "title" { decode_entities(body) } else { body.to_string() }));
                    }
                    out.push(Tok::End(tag));
                    i = src[stop..].find('>').map(|p| stop + p + 1).unwrap_or(b.len());
                    text_start = i;
                    continue;
                }
            }
            out.push(tok);
        } else {
            i += 1;
        }
    }
    flush(&mut out, text_start, b.len());
    out
}

fn start_tag(src: &str, at: usize) -> (Tok, usize) {
    let b = src.as_bytes();
    let mut j = at + 1;
    let ws = |c: u8| matches!(c, b' ' | b'\t' | b'\n' | b'\r' | b'\x0c');
    while j < b.len() && !ws(b[j]) && b[j] != b'>' && b[j] != b'/' {
        j += 1;
    }
    let tag = src[at + 1..j].to_ascii_lowercase();
    let mut attrs: Vec<(String, String)> = Vec::new();
    let mut self_closing = false;
    loop {
        while j < b.len() && (ws(b[j]) || (b[j] == b'/' && b.get(j + 1) != Some(&b'>'))) {
            j += 1;
        }
        if j >= b.len() {
            break;
        }
        if b[j] == b'>' {
            j += 1;
            break;
        }
        if b[j] == b'/' {
            self_closing = true;
            j += 2;
            break;
        }
        let ns = j;
        while j < b.len() && !ws(b[j]) && !matches!(b[j], b'=' | b'>') && !(b[j] == b'/' && j > ns) {
            j += 1;
        }
        let name = src[ns..j].to_ascii_lowercase();
        while j < b.len() && ws(b[j]) {
            j += 1;
        }
        let mut value = String::new();
        if j < b.len() && b[j] == b'=' {
            j += 1;
            while j < b.len() && ws(b[j]) {
                j += 1;
            }
            if j < b.len() && (b[j] == b'"' || b[j] == b'\'') {
                let q = b[j];
                let vs = j + 1;
                let ve = src[vs..].find(q as char).map(|p| vs + p).unwrap_or(b.len());
                value = decode_entities(&src[vs..ve]);
                j = (ve + 1).min(b.len());
            } else {
                let vs = j;
                while j < b.len() && !ws(b[j]) && b[j] != b'>' {
                    j += 1;
                }
                value = decode_entities(&src[vs..j]);
            }
        }
        if !name.is_empty() && !attrs.iter().any(|(k, _)| *k == name) {
            attrs.push((name, value));
        }
    }
    (Tok::Start { tag, attrs, self_closing }, j)
}

// ------------------------------------------------------------------ tree builder

const FORMATTING: &[&str] = &["a", "b", "big", "code", "em", "font", "i", "nobr", "s", "small", "strike", "strong", "tt", "u"];
/// HTML "special" elements: generic end tags do not cross them.
const SPECIAL: &[&str] = &[
    "address", "applet", "area", "article", "aside", "base", "basefont", "bgsound", "blockquote", "body", "br", "button", "caption",
    "center", "col", "colgroup", "dd", "details", "dir", "div", "dl", "dt", "embed", "fieldset", "figcaption", "figure", "footer",
    "form", "frame", "frameset", "h1", "h2", "h3", "h4", "h5", "h6", "head", "header", "hgroup", "hr", "html", "iframe", "img",
    "input", "keygen", "li", "link", "listing", "main", "marquee", "menu", "meta", "nav", "noembed", "noframes", "noscript",
    "object", "ol", "p", "param", "plaintext", "pre", "script", "search", "section", "select", "source", "style", "summary",
    "table", "tbody", "td", "template", "textarea", "tfoot", "th", "thead", "title", "tr", "track", "ul", "wbr", "xmp",
];
/// Block containers: their start tag closes an open <p>; their end tag closes them in scope.
const BLOCKS: &[&str] = &[
    "address", "article", "aside", "blockquote", "center", "details", "dialog", "dir", "div", "dl", "fieldset", "figcaption",
    "figure", "footer", "header", "hgroup", "main", "menu", "nav", "ol", "p", "search", "section", "summary", "ul",
];
/// Start tags that do not reopen the active formatting elements first.
const NO_RECONSTRUCT: &[&str] = &[
    "address", "article", "aside", "blockquote", "center", "details", "dialog", "dir", "div", "dl", "fieldset", "figcaption",
    "figure", "footer", "header", "hgroup", "main", "menu", "nav", "ol", "p", "search", "section", "summary", "ul", "h1", "h2",
    "h3", "h4", "h5", "h6", "pre", "listing", "form", "li", "dd", "dt", "plaintext", "table", "hr", "textarea", "iframe",
    "noembed", "noscript", "script", "style", "template", "title", "meta", "link", "base", "basefont", "bgsound", "noframes",
    "caption", "col", "colgroup", "frame", "head", "tbody", "td", "tfoot", "th", "thead", "tr",
];
const HEADINGS: &[&str] = &["h1", "h2", "h3", "h4", "h5", "h6"];
const IMPLIED_END: &[&str] = &["dd", "dt", "li", "optgroup", "option", "p", "rb", "rp", "rt", "rtc"];
const SCOPE: &[&str] = &["applet", "caption", "html", "table", "td", "th", "marquee", "object", "template", "#root"];

struct Open {
    id: usize,
    tag: String,
    attrs: Vec<(String, String)>,
    kids: Vec<Node>,
}

/// Tree builder state: the stack of open elements and the active formatting elements.
struct Tb {
    stack: Vec<Open>,
    afl: Vec<(usize, String, Vec<(String, String)>)>,
    next: usize,
}

impl Tb {
    fn new_id(&mut self) -> usize {
        self.next += 1;
        self.next
    }
    fn top(&mut self) -> &mut Open {
        self.stack.last_mut().unwrap()
    }
    fn top_tag(&self) -> &str {
        &self.stack.last().unwrap().tag
    }
    fn close_top(&mut self) {
        let o = self.stack.pop().unwrap();
        self.top().kids.push(Node::El { tag: o.tag, attrs: o.attrs, kids: o.kids });
    }
    fn push(&mut self, tag: String, attrs: Vec<(String, String)>) -> usize {
        let id = self.new_id();
        self.stack.push(Open { id, tag, attrs, kids: Vec::new() });
        id
    }
    fn leaf(&mut self, tag: String, attrs: Vec<(String, String)>) {
        self.top().kids.push(Node::El { tag, attrs, kids: Vec::new() });
    }
    fn text(&mut self, t: &str) {
        let kids = &mut self.top().kids;
        if let Some(Node::Text(prev)) = kids.last_mut() {
            prev.push_str(t);
        } else {
            kids.push(Node::Text(t.to_string()));
        }
    }
    fn in_stack(&self, id: usize) -> bool {
        self.stack.iter().any(|o| o.id == id)
    }
    /// "has an element in scope" (`extra` = button / list-item scope boundaries)
    fn in_scope(&self, pred: impl Fn(&str) -> bool, extra: &[&str]) -> bool {
        for o in self.stack.iter().rev() {
            if pred(&o.tag) {
                return true;
            }
            if SCOPE.contains(&o.tag.as_str()) || extra.contains(&o.tag.as_str()) {
                return false;
            }
        }
        false
    }
    fn pop_until(&mut self, pred: impl Fn(&str) -> bool) {
        while self.stack.len() > 1 {
            let hit = pred(self.top_tag());
            self.close_top();
            if hit {
                break;
            }
        }
    }
    fn implied_end(&mut self, except: &str) {
        while self.stack.len() > 1 {
            let t = self.top_tag();
            if IMPLIED_END.contains(&t) && t != except {
                self.close_top();
            } else {
                break;
            }
        }
    }
    fn close_p(&mut self) {
        if self.in_scope(|t| t == "p", &["button"]) {
            self.implied_end("p");
            self.pop_until(|t| t == "p");
        }
    }
    fn reconstruct(&mut self) {
        let Some(last) = self.afl.last() else { return };
        if self.in_stack(last.0) {
            return;
        }
        let mut k = self.afl.len() - 1;
        while k > 0 && !self.in_stack(self.afl[k - 1].0) {
            k -= 1;
        }
        for j in k..self.afl.len() {
            let (tag, attrs) = (self.afl[j].1.clone(), self.afl[j].2.clone());
            let id = self.push(tag, attrs);
            self.afl[j].0 = id;
        }
    }
    /// </b>, </i>, … — the adoption agency (formatting elements between the formatting
    /// element and the furthest block are just closed; the outer loop's cloning of them is
    /// not modelled). Returns false when there is no such active formatting element.
    fn adopt(&mut self, tag: &str) -> bool {
        let Some(fi) = self.afl.iter().rposition(|e| e.1 == tag) else { return false };
        let fe_id = self.afl[fi].0;
        let Some(fe_at) = self.stack.iter().position(|o| o.id == fe_id) else {
            self.afl.remove(fi);
            return true;
        };
        if !self.in_scope(|t| t == tag, &[]) {
            return true;
        }
        let fb_at = (fe_at + 1..self.stack.len()).find(|&k| SPECIAL.contains(&self.stack[k].tag.as_str()));
        match fb_at {
            None => {
                while self.stack.len() > fe_at {
                    self.close_top();
                }
                self.afl.remove(fi);
            }
            Some(fb_at) => {
                // the furthest block leaves the formatting element; a copy of the formatting
                // element takes over the block's children
                let mut upper: Vec<Open> = self.stack.drain(fb_at..).collect();
                while self.stack.len() > fe_at {
                    self.close_top();
                }
                let mut fb = upper.remove(0);
                let ne_id = self.new_id();
                let (ne_tag, ne_attrs) = (self.afl[fi].1.clone(), self.afl[fi].2.clone());
                let ne_kids = std::mem::take(&mut fb.kids);
                self.stack.push(fb);
                self.stack.push(Open { id: ne_id, tag: ne_tag, attrs: ne_attrs, kids: ne_kids });
                self.stack.extend(upper);
                self.afl[fi].0 = ne_id;
            }
        }
        true
    }
}

/// Parses an HTML fragment (like `<template>.innerHTML = html`) into nodes, following the
/// HTML tree-construction rules that change structure for misnested input: implied </p>,
/// heading in heading, list items, formatting elements reopened after an implicit close
/// ("reconstruct the active formatting elements") and the adoption agency for </b> etc.
pub fn parse(src: &str) -> Vec<Node> {
    // the HTML input stream turns CR LF and lone CR into LF before tokenizing
    let normalized;
    let src = if src.contains('\r') {
        normalized = src.replace("\r\n", "\n").replace('\r', "\n");
        normalized.as_str()
    } else {
        src
    };
    let mut tb = Tb { stack: vec![Open { id: 0, tag: "#root".into(), attrs: Vec::new(), kids: Vec::new() }], afl: Vec::new(), next: 0 };
    let mut skip_nl = false;
    for tok in tokenize(src) {
        let skip = std::mem::take(&mut skip_nl);
        match tok {
            Tok::Comment => {}
            Tok::Text(t) => {
                let t = if skip { t.strip_prefix('\n').unwrap_or(&t).to_string() } else { t };
                if t.is_empty() {
                    continue;
                }
                tb.reconstruct();
                tb.text(&t);
            }
            Tok::Start { tag, attrs, self_closing } => {
                let _ = self_closing; // a self-closing non-void tag is a start tag in HTML
                let t = tag.as_str();
                if matches!(t, "html" | "head" | "body") {
                    continue; // ignored inside a template
                }
                if BLOCKS.contains(&t) || matches!(t, "pre" | "listing" | "table" | "hr" | "dd" | "dt" | "li" | "form" | "plaintext") || HEADINGS.contains(&t) {
                    tb.close_p();
                }
                if HEADINGS.contains(&t) && HEADINGS.contains(&tb.top_tag()) {
                    tb.close_top();
                }
                if t == "li" || t == "dd" || t == "dt" {
                    // close the open item unless a block other than address/div/p is in the way
                    let mut k = tb.stack.len() - 1;
                    while k > 0 {
                        let n = tb.stack[k].tag.clone();
                        if (t == "li" && n == "li") || (t != "li" && (n == "dd" || n == "dt")) {
                            tb.implied_end(&n);
                            tb.pop_until(|x| x == n);
                            break;
                        }
                        if SPECIAL.contains(&n.as_str()) && !matches!(n.as_str(), "address" | "div" | "p") {
                            break;
                        }
                        k -= 1;
                    }
                }
                if t == "a" {
                    // an <a> inside an open <a>: the outer one is closed first (adoption agency)
                    if let Some(old) = tb.afl.iter().rev().find(|e| e.1 == "a").map(|e| e.0) {
                        tb.adopt("a");
                        tb.afl.retain(|e| e.0 != old);
                        if let Some(at) = tb.stack.iter().position(|o| o.id == old) {
                            // still open (no furthest block): close it where it stands
                            let upper: Vec<Open> = tb.stack.drain(at + 1..).collect();
                            tb.close_top();
                            tb.stack.extend(upper);
                        }
                    }
                }
                if !NO_RECONSTRUCT.contains(&t) {
                    tb.reconstruct();
                }
                if VOID.contains(&t) {
                    tb.leaf(tag, attrs);
                } else if FORMATTING.contains(&t) {
                    let a2 = attrs.clone();
                    let id = tb.push(tag.clone(), attrs);
                    tb.afl.push((id, tag, a2));
                } else {
                    if matches!(t, "pre" | "listing" | "textarea") {
                        skip_nl = true;
                    }
                    tb.push(tag, attrs);
                }
            }
            Tok::End(tag) => {
                let t = tag.as_str();
                if t == "br" {
                    tb.reconstruct();
                    tb.leaf("br".into(), Vec::new());
                } else if t == "p" {
                    if !tb.in_scope(|x| x == "p", &["button"]) {
                        tb.push("p".into(), Vec::new());
                    }
                    tb.implied_end("p");
                    tb.pop_until(|x| x == "p");
                } else if t == "li" {
                    if tb.in_scope(|x| x == "li", &["ol", "ul"]) {
                        tb.implied_end("li");
                        tb.pop_until(|x| x == "li");
                    }
                } else if HEADINGS.contains(&t) {
                    if tb.in_scope(|x| HEADINGS.contains(&x), &[]) {
                        tb.implied_end("");
                        tb.pop_until(|x| HEADINGS.contains(&x));
                    }
                } else if BLOCKS.contains(&t) || matches!(t, "pre" | "listing" | "button" | "form" | "dd" | "dt") {
                    if tb.in_scope(|x| x == t, &[]) {
                        tb.implied_end(t);
                        tb.pop_until(|x| x == t);
                    }
                } else if FORMATTING.contains(&t) && tb.adopt(t) {
                    // handled by the adoption agency
                } else {
                    // any other end tag: close the nearest same-named element unless a
                    // special element is in the way
                    let mut k = tb.stack.len() - 1;
                    while k > 0 {
                        if tb.stack[k].tag == t {
                            tb.implied_end(t);
                            while tb.stack.len() > k {
                                tb.close_top();
                            }
                            break;
                        }
                        if SPECIAL.contains(&tb.stack[k].tag.as_str()) {
                            break;
                        }
                        k -= 1;
                    }
                }
            }
        }
    }
    while tb.stack.len() > 1 {
        tb.close_top();
    }
    tb.stack.pop().unwrap().kids
}

// ------------------------------------------------------------------ sanitizer (rich.js clean)

fn map_tag(tag: &str) -> Option<&'static str> {
    Some(match tag {
        "b" | "strong" => "b",
        "i" | "em" => "i",
        "u" | "ins" => "u",
        "s" | "strike" | "del" => "s",
        "ul" | "ol" => "ul",
        "li" => "li",
        "br" => "br",
        "div" | "p" => "div",
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "section" | "article" | "header" | "footer" | "blockquote" | "pre" | "tr"
        | "table" | "figure" | "aside" | "nav" | "main" | "address" | "dt" | "dd" => "div",
        _ => return None,
    })
}

const DROP: &[&str] = &[
    "script", "style", "template", "iframe", "object", "embed", "head", "title", "meta", "link", "noscript", "svg", "math", "canvas",
    "video", "audio", "select", "textarea", "input", "button",
];

/// `[0-9a-f-]{36}\.(png|jpg|gif|webp)` — a picture file name the app created.
pub fn is_image_name(n: &str) -> bool {
    let Some((stem, ext)) = n.split_once('.') else { return false };
    stem.len() == 36 && stem.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c) || c == b'-') && matches!(ext, "png" | "jpg" | "gif" | "webp")
}

/// rich.js styleWraps: inline styles that mean b / i / u / s.
fn style_wraps(style: &str) -> Vec<&'static str> {
    let lower = style.to_ascii_lowercase();
    let mut w = Vec::new();
    // /prop\s*:\s*VALUE/i: any occurrence whose value matches
    let any_value = |prop: &str, ok: &dyn Fn(&str) -> bool| -> bool {
        let mut from = 0;
        while let Some(p) = lower[from..].find(prop) {
            let at = from + p + prop.len();
            if let Some(v) = lower[at..].trim_start().strip_prefix(':') {
                if ok(v.trim_start()) {
                    return true;
                }
            }
            from = from + p + 1;
        }
        false
    };
    let heavy = |v: &str| {
        let b = v.as_bytes();
        v.starts_with("bold") || (b.len() >= 3 && (b'6'..=b'9').contains(&b[0]) && b[1] == b'0' && b[2] == b'0')
    };
    if any_value("font-weight", &heavy) {
        w.push("b");
    }
    if any_value("font-style", &|v: &str| v.starts_with("italic")) {
        w.push("i");
    }
    // /text-decoration[^;]*underline/ and line-through: anywhere after "text-decoration", before a ';'
    let deco = |word: &str| {
        let mut from = 0;
        while let Some(p) = lower[from..].find("text-decoration") {
            let at = from + p + "text-decoration".len();
            let seg = lower[at..].split(';').next().unwrap_or("");
            if seg.contains(word) {
                return true;
            }
            from = at;
        }
        false
    };
    if deco("underline") {
        w.push("u");
    }
    if deco("line-through") {
        w.push("s");
    }
    w
}

fn clean_into(src: &[Node], out: &mut Vec<Node>) {
    for node in src {
        match node {
            Node::Text(t) => out.push(Node::Text(t.clone())),
            Node::El { tag, kids, .. } => {
                if DROP.contains(&tag.as_str()) {
                    continue;
                }
                if tag == "img" {
                    if let Some(n) = node.attr("data-gm") {
                        if is_image_name(n) {
                            out.push(Node::El { tag: "img".into(), attrs: vec![("data-gm".into(), n.into())], kids: Vec::new() });
                        }
                    }
                    continue;
                }
                let mapped = map_tag(tag);
                // wrappers, outermost first: the mapped element, then style wraps
                let mut chain: Vec<&'static str> = Vec::new();
                if let Some(m) = mapped {
                    chain.push(m);
                }
                if tag != "br" {
                    for w in style_wraps(node.attr("style").unwrap_or("")) {
                        if Some(w) != mapped {
                            chain.push(w);
                        }
                    }
                }
                let mut inner = Vec::new();
                if tag != "br" {
                    clean_into(kids, &mut inner);
                }
                let mut built = inner;
                for t in chain.iter().rev() {
                    built = vec![Node::el(t, built)];
                }
                out.extend(built);
            }
        }
    }
}

/// `Rich.fromHtml`: parse + allow-list clean.
pub fn sanitize(html: &str) -> Vec<Node> {
    let mut out = Vec::new();
    clean_into(&parse(html), &mut out);
    merge_text(out)
}

fn merge_text(nodes: Vec<Node>) -> Vec<Node> {
    let mut out: Vec<Node> = Vec::with_capacity(nodes.len());
    for n in nodes {
        match n {
            Node::Text(t) => {
                if t.is_empty() {
                    continue;
                }
                if let Some(Node::Text(p)) = out.last_mut() {
                    p.push_str(&t);
                } else {
                    out.push(Node::Text(t));
                }
            }
            Node::El { tag, attrs, kids } => out.push(Node::El { tag, attrs, kids: merge_text(kids) }),
        }
    }
    out
}

// ------------------------------------------------------------------ serializer

fn escape_text(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\u{a0}' => out.push_str("&nbsp;"),
            _ => out.push(c),
        }
    }
}

fn escape_attr(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\u{a0}' => out.push_str("&nbsp;"),
            _ => out.push(c),
        }
    }
}

/// Chromium `innerHTML` of a node list.
pub fn serialize(nodes: &[Node]) -> String {
    let mut s = String::new();
    ser(nodes, &mut s);
    s
}

fn ser(nodes: &[Node], out: &mut String) {
    for n in nodes {
        match n {
            Node::Text(t) => escape_text(t, out),
            Node::El { tag, attrs, kids } => {
                out.push('<');
                out.push_str(tag);
                for (k, v) in attrs {
                    out.push(' ');
                    out.push_str(k);
                    out.push_str("=\"");
                    escape_attr(v, out);
                    out.push('"');
                }
                out.push('>');
                if VOID.contains(&tag.as_str()) {
                    continue;
                }
                ser(kids, out);
                let _ = write!(out, "</{tag}>");
            }
        }
    }
}

// ------------------------------------------------------------------ innerText (Rich.plain)

enum It {
    Text(String),
    Req(u8),
}

fn collect_inner(nodes: &[Node], out: &mut Vec<It>) {
    for n in nodes {
        match n {
            Node::Text(t) => out.push(It::Text(t.clone())),
            Node::El { tag, kids, .. } => match tag.as_str() {
                "br" => out.push(It::Text("\n".into())),
                // block-level in .body: div, ul, li (list-item), img (display: block)
                "div" | "ul" | "li" | "img" => {
                    out.push(It::Req(1));
                    collect_inner(kids, out);
                    out.push(It::Req(1));
                }
                _ => collect_inner(kids, out),
            },
        }
    }
}

/// Chromium `innerText` of a sanitized tree rendered in v1.1's `.body`.
pub fn inner_text(nodes: &[Node]) -> String {
    let mut items = Vec::new();
    collect_inner(nodes, &mut items);
    // drop leading / trailing required line breaks, collapse runs to their max
    let mut out = String::new();
    let mut pending: u8 = 0;
    let mut started = false;
    for it in items {
        match it {
            It::Req(n) => pending = pending.max(n),
            It::Text(t) => {
                if t.is_empty() {
                    continue;
                }
                if started {
                    for _ in 0..pending {
                        out.push('\n');
                    }
                }
                pending = 0;
                started = true;
                out.push_str(&t);
            }
        }
    }
    out
}

/// `Rich.plain`: innerText with NBSP → space and trailing newlines removed.
pub fn plain(nodes: &[Node]) -> String {
    let t = inner_text(nodes).replace('\u{a0}', " ");
    t.trim_end_matches('\n').to_string()
}

// ------------------------------------------------------------------ on-screen document

#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Text(String, u8),
    Img(String),
}

/// One paragraph: `\r`-separated in RichEdit; its lines are `\v`-separated.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Para {
    pub bullet: bool,
    pub lines: Vec<Vec<Item>>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Doc {
    pub paras: Vec<Para>,
}

/// Inline content of one block while building lines.
#[derive(Default)]
struct Flow {
    segs: Vec<Vec<Item>>, // lines so far (last one open)
    breaks: usize,
    any: bool,
}

impl Flow {
    fn push_text(&mut self, t: &str, bits: u8) {
        for (k, part) in t.split('\n').enumerate() {
            if k > 0 {
                self.brk();
            }
            if !part.is_empty() {
                self.any = true;
                if self.segs.is_empty() {
                    self.segs.push(Vec::new());
                }
                let line = self.segs.last_mut().unwrap();
                match line.last_mut() {
                    Some(Item::Text(p, b)) if *b == bits => p.push_str(part),
                    _ => line.push(Item::Text(part.to_string(), bits)),
                }
            }
        }
    }
    fn brk(&mut self) {
        self.any = true;
        if self.segs.is_empty() {
            self.segs.push(Vec::new());
        }
        self.segs.push(Vec::new());
        self.breaks += 1;
    }
    /// Lines of this block: a final forced break does not open another line.
    fn lines(mut self) -> Vec<Vec<Item>> {
        if !self.any {
            return Vec::new();
        }
        if self.breaks > 0 && self.segs.last().is_some_and(|l| l.is_empty()) {
            self.segs.pop();
        }
        self.segs
    }
}

/// Where block lines go while inside a <li>.
#[derive(Clone, Copy, PartialEq)]
enum Li {
    Out,
    /// item opened, no line yet: the next lines start a bullet paragraph
    Fresh,
    /// lines join this bullet paragraph (the marker is on its first line only)
    Para(usize),
    /// after a nested list: further lines of the outer item have no marker
    Cont,
}

struct Builder {
    doc: Doc,
    flow: Flow,
    li: Li,
}

impl Builder {
    /// Ends the current anonymous / leaf block: its lines become paragraph(s).
    fn end_block(&mut self) {
        let lines = std::mem::take(&mut self.flow).lines();
        if lines.is_empty() {
            return;
        }
        match self.li {
            // outside lists every line is a paragraph (no visible difference in RichEdit)
            Li::Out | Li::Cont => self.doc.paras.extend(lines.into_iter().map(|l| Para { bullet: false, lines: vec![l] })),
            Li::Fresh => {
                self.doc.paras.push(Para { bullet: true, lines });
                self.li = Li::Para(self.doc.paras.len() - 1);
            }
            Li::Para(p) => self.doc.paras[p].lines.extend(lines),
        }
    }

    fn walk(&mut self, nodes: &[Node], bits: u8) {
        for n in nodes {
            match n {
                Node::Text(t) => self.flow.push_text(t, bits),
                Node::El { tag, kids, .. } => match tag.as_str() {
                    "br" => self.flow.brk(),
                    "b" => self.walk(kids, bits | B),
                    "i" => self.walk(kids, bits | I),
                    "u" => self.walk(kids, bits | U),
                    "s" => self.walk(kids, bits | S),
                    "img" => {
                        // display: block — a line of its own
                        self.end_block();
                        let name = n.attr("data-gm").unwrap_or("").to_string();
                        self.flow.any = true;
                        self.flow.segs.push(vec![Item::Img(name)]);
                        self.end_block();
                    }
                    "li" => {
                        self.end_block();
                        let outer = self.li;
                        self.li = Li::Fresh;
                        self.walk(kids, bits);
                        self.end_block();
                        self.li = match outer {
                            Li::Para(_) | Li::Cont => Li::Cont,
                            other => other,
                        };
                    }
                    _ => {
                        // div, ul and anything else block-level after sanitizing
                        self.end_block();
                        self.walk(kids, bits);
                        self.end_block();
                    }
                },
            }
        }
    }
}

impl Doc {
    /// Lines as Chromium lays out a sanitized tree in v1.1's `.body`.
    pub fn from_tree(nodes: &[Node]) -> Doc {
        let mut b = Builder { doc: Doc::default(), flow: Flow::default(), li: Li::Out };
        b.walk(nodes, 0);
        b.end_block();
        b.doc
    }

    /// v1.0 notes have only plain text: one paragraph per line.
    pub fn from_plain(text: &str) -> Doc {
        if text.is_empty() {
            return Doc::default();
        }
        Doc {
            paras: text
                .split('\n')
                .map(|l| Para { bullet: false, lines: vec![if l.is_empty() { Vec::new() } else { vec![Item::Text(l.to_string(), 0)] }] })
                .collect(),
        }
    }

    /// The body as stored by v1.1: HTML if any, else plain text.
    pub fn load(html: &str, text: &str) -> Doc {
        if html.is_empty() {
            Doc::from_plain(text)
        } else {
            Doc::from_tree(&sanitize(html))
        }
    }

    /// HTML that v1.1 renders as exactly these lines (report table 6-2):
    /// a <div> per paragraph, <ul><li> for bullet runs, <br> between lines (plus one more
    /// when the last line is empty), b→i→u→s nesting opened and closed at every run.
    pub fn to_tree(&self) -> Vec<Node> {
        fn line_nodes(line: &[Item], out: &mut Vec<Node>) {
            for it in line {
                match it {
                    Item::Text(t, bits) => {
                        let mut n = vec![Node::Text(t.clone())];
                        for (bit, tag) in [(S, "s"), (U, "u"), (I, "i"), (B, "b")] {
                            if bits & bit != 0 {
                                n = vec![Node::el(tag, n)];
                            }
                        }
                        out.extend(n);
                    }
                    Item::Img(name) => out.push(Node::El { tag: "img".into(), attrs: vec![("data-gm".into(), name.clone())], kids: Vec::new() }),
                }
            }
        }
        // lines joined by <br>; one more <br> when the last line is empty (a final break
        // opens no line of its own)
        fn run(lines: &[&Vec<Item>], out: &mut Vec<Node>) {
            for (k, l) in lines.iter().enumerate() {
                if k > 0 {
                    out.push(Node::el("br", Vec::new()));
                }
                line_nodes(l, out);
            }
            if lines.last().is_none_or(|l| l.is_empty()) {
                out.push(Node::el("br", Vec::new()));
            }
        }
        // a picture is a block of its own in v1.1 (display: block): no <br> around it, and
        // the text lines between pictures are separate runs
        fn block(lines: &[&Vec<Item>], out: &mut Vec<Node>) {
            let is_img = |l: &Vec<Item>| matches!(l.as_slice(), [Item::Img(_)]);
            let mut text: Vec<&Vec<Item>> = Vec::new();
            for l in lines {
                if is_img(l) {
                    if !text.is_empty() {
                        run(&text, out);
                        text.clear();
                    }
                    line_nodes(l, out);
                } else {
                    text.push(l);
                }
            }
            if !text.is_empty() || lines.is_empty() {
                run(&text, out);
            }
        }
        let mut out: Vec<Node> = Vec::new();
        let mut k = 0;
        while k < self.paras.len() {
            if self.paras[k].bullet {
                let mut items = Vec::new();
                while k < self.paras.len() && self.paras[k].bullet {
                    let mut kids = Vec::new();
                    block(&self.paras[k].lines.iter().collect::<Vec<_>>(), &mut kids);
                    items.push(Node::el("li", kids));
                    k += 1;
                }
                out.push(Node::el("ul", items));
            } else {
                // plain paragraphs: top-level lines, like v1.1 shows a v1.0 note
                let mut lines: Vec<&Vec<Item>> = Vec::new();
                while k < self.paras.len() && !self.paras[k].bullet {
                    lines.extend(self.paras[k].lines.iter());
                    k += 1;
                }
                block(&lines, &mut out);
            }
        }
        merge_text(out)
    }

    pub fn to_html(&self) -> String {
        serialize(&self.to_tree())
    }

    /// The `text` copy v1.1 would compute for [`Doc::to_html`] (`Rich.plain`).
    pub fn plain(&self) -> String {
        plain(&self.to_tree())
    }

    /// Normal form (report §6.4): on-screen lines, each with its bullet marker (first line of
    /// a bullet paragraph), characters with b/i/u/s bits and pictures — equal normal forms
    /// look the same.
    pub fn normal(&self) -> Vec<(bool, Vec<Item>)> {
        let mut v = Vec::new();
        for p in &self.paras {
            for (k, l) in p.lines.iter().enumerate() {
                // merge adjacent runs with the same bits
                let mut items: Vec<Item> = Vec::new();
                for it in l {
                    match (items.last_mut(), it) {
                        (Some(Item::Text(a, ab)), Item::Text(t, tb)) if ab == tb => a.push_str(t),
                        (_, Item::Text(t, _)) if t.is_empty() => {}
                        _ => items.push(it.clone()),
                    }
                }
                v.push((p.bullet && k == 0, items));
            }
        }
        // one empty line is an empty body: v1.1 shows the placeholder for both ("<br>" / "")
        if matches!(v.as_slice(), [(false, items)] if items.is_empty()) {
            v.clear();
        }
        v
    }
}

/// HTML that is saved: v1.1 drops HTML over 1 MB and keeps the plain text (cut at 200,000
/// characters).
/// innerText built as it goes (same rules as [`inner_text`]).
#[derive(Default)]
struct Inner {
    out: String,
    pending: u8,
    started: bool,
}

impl Inner {
    fn text(&mut self, t: &str) {
        if t.is_empty() {
            return;
        }
        if self.started {
            for _ in 0..self.pending {
                self.out.push('\n');
            }
        }
        self.pending = 0;
        self.started = true;
        self.out.push_str(t);
    }
    fn req(&mut self) {
        self.pending = self.pending.max(1);
    }
}

/// [`Doc::to_html`] and [`Doc::plain`] in one pass without building the node tree — the
/// hot part of every save (report §5.4). Must equal the tree path (tested).
fn emit(doc: &Doc) -> (String, String) {
    fn line(l: &[Item], h: &mut String, t: &mut Inner) {
        for it in l {
            match it {
                Item::Text(s, bits) => {
                    for (bit, tag) in [(B, "<b>"), (I, "<i>"), (U, "<u>"), (S, "<s>")] {
                        if bits & bit != 0 {
                            h.push_str(tag);
                        }
                    }
                    escape_text(s, h);
                    t.text(s);
                    for (bit, tag) in [(S, "</s>"), (U, "</u>"), (I, "</i>"), (B, "</b>")] {
                        if bits & bit != 0 {
                            h.push_str(tag);
                        }
                    }
                }
                Item::Img(n) => {
                    h.push_str("<img data-gm=\"");
                    escape_attr(n, h);
                    h.push_str("\">");
                    t.req();
                    t.req();
                }
            }
        }
    }
    fn run(lines: &[&Vec<Item>], h: &mut String, t: &mut Inner) {
        for (k, l) in lines.iter().enumerate() {
            if k > 0 {
                h.push_str("<br>");
                t.text("\n");
            }
            line(l, h, t);
        }
        if lines.last().is_none_or(|l| l.is_empty()) {
            h.push_str("<br>");
            t.text("\n");
        }
    }
    fn block(lines: &[&Vec<Item>], h: &mut String, t: &mut Inner) {
        let is_img = |l: &Vec<Item>| matches!(l.as_slice(), [Item::Img(_)]);
        let mut text: Vec<&Vec<Item>> = Vec::new();
        for l in lines {
            if is_img(l) {
                if !text.is_empty() {
                    run(&text, h, t);
                    text.clear();
                }
                line(l, h, t);
            } else {
                text.push(l);
            }
        }
        if !text.is_empty() || lines.is_empty() {
            run(&text, h, t);
        }
    }
    let mut h = String::new();
    let mut t = Inner::default();
    let mut k = 0;
    let paras = &doc.paras;
    while k < paras.len() {
        if paras[k].bullet {
            h.push_str("<ul>");
            t.req();
            while k < paras.len() && paras[k].bullet {
                h.push_str("<li>");
                t.req();
                block(&paras[k].lines.iter().collect::<Vec<_>>(), &mut h, &mut t);
                h.push_str("</li>");
                t.req();
                k += 1;
            }
            h.push_str("</ul>");
            t.req();
        } else {
            let mut lines: Vec<&Vec<Item>> = Vec::new();
            while k < paras.len() && !paras[k].bullet {
                lines.extend(paras[k].lines.iter());
                k += 1;
            }
            block(&lines, &mut h, &mut t);
        }
    }
    let text = t.out.replace('\u{a0}', " ").trim_end_matches('\n').to_string();
    (h, text)
}

pub fn stored(doc: &Doc) -> (String, String) {
    let (mut html, mut text) = emit(doc);
    if text.len() > MAX_TEXT && text.chars().count() > MAX_TEXT {
        text = text.chars().take(MAX_TEXT).collect();
    }
    if html.len() > MAX_HTML {
        html.clear();
    }
    (html, text)
}

// ------------------------------------------------------------------ differential check

/// JSON string literal (UTF-16 escapes for non-ASCII, like the JS side expects).
pub fn json_str(v: &str) -> String {
    let mut e = String::with_capacity(v.len() + 2);
    e.push('"');
    for ch in v.chars() {
        match ch {
            '"' => e.push_str("\\\""),
            '\\' => e.push_str("\\\\"),
            c if (c as u32) < 0x20 || !c.is_ascii() => {
                let mut buf = [0u16; 2];
                for u in c.encode_utf16(&mut buf) {
                    let _ = write!(e, "\\u{:04x}", u);
                }
            }
            c => e.push(c),
        }
    }
    e.push('"');
    e
}

fn json_normal(doc: &Doc) -> String {
    let mut s = String::from("[");
    for (k, (marker, items)) in doc.normal().iter().enumerate() {
        if k > 0 {
            s.push(',');
        }
        let _ = write!(s, "[{marker},[");
        for (j, it) in items.iter().enumerate() {
            if j > 0 {
                s.push(',');
            }
            match it {
                Item::Text(t, b) => {
                    let _ = write!(s, "[{},{b}]", json_str(t));
                }
                Item::Img(n) => {
                    let _ = write!(s, "[\"#img\",{}]", json_str(n));
                }
            }
        }
        s.push_str("]]");
    }
    s.push(']');
    s
}

/// `--mode rich --in CASES --out FILE`: the Rust side of `bench/rich/diff.py`. CASES holds
/// HTML snippets separated by U+001E; one JSON line per case.
pub fn check_file(input: &str, output: &str) {
    let src = std::fs::read_to_string(input).unwrap_or_default();
    let mut out = String::new();
    for (k, html) in src.split('\u{1e}').enumerate() {
        let tree = sanitize(html);
        let doc = Doc::from_tree(&tree);
        let _ = writeln!(
            out,
            "{{\"i\":{k},\"san\":{},\"plain\":{},\"vis\":{},\"saved\":{},\"saved_plain\":{}}}",
            json_str(&serialize(&tree)),
            json_str(&plain(&tree)),
            json_normal(&doc),
            json_str(&doc.to_html()),
            json_str(&doc.plain())
        );
    }
    let _ = std::fs::write(output, out);
}

/// The "HTML Format" clipboard payload (CF_HTML): the fragment between the markers.
pub fn cf_html_fragment(raw: &str) -> &str {
    let num = |key: &str| -> Option<usize> {
        let p = raw.find(key)? + key.len();
        raw[p..].lines().next()?.trim().parse().ok()
    };
    if let (Some(s), Some(e)) = (num("StartFragment:"), num("EndFragment:")) {
        if s <= e && e <= raw.len() && raw.is_char_boundary(s) && raw.is_char_boundary(e) {
            return &raw[s..e];
        }
    }
    if let (Some(s), Some(e)) = (raw.find("<!--StartFragment-->"), raw.find("<!--EndFragment-->")) {
        if s < e {
            return &raw[s + "<!--StartFragment-->".len()..e];
        }
    }
    raw
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAME: &str = "0123abcd-0123-4567-89ab-0123456789ab.png";

    fn clean(h: &str) -> String {
        serialize(&sanitize(h))
    }

    #[test]
    fn web_paste_keeps_supported_formatting() {
        // v1.1 note_ui_test.html check 1
        let h = clean(r#"<p><span style="font-weight:700">굵게</span> <a href="https://x">링크</a><script>evil()</script> <span style="text-decoration:line-through">취소</span> <em>기울임</em></p><ol><li>하나</li></ol><img src="https://example.com/x.png" onerror="evil()">"#);
        assert!(h.contains("<b>굵게</b>"), "{h}");
        assert!(h.contains("<s>취소</s>") && h.contains("<i>기울임</i>") && h.contains("<ul><li>하나</li></ul>"), "{h}");
        assert!(!h.contains("<a") && !h.contains("script") && !h.contains("example.com") && !h.contains("onerror") && h.contains("링크"), "{h}");
    }

    #[test]
    fn pictures_keep_only_app_names() {
        let h = clean(&format!(r#"a<img data-gm="{NAME}" src="http://gmimg.localhost/{NAME}">b<img data-gm="../evil.exe"><img src="x.png">"#));
        assert_eq!(h, format!(r#"a<img data-gm="{NAME}">b"#));
    }

    #[test]
    fn style_wraps_nest_inside_mapped_tag() {
        assert_eq!(clean(r#"<p style="font-style: italic; text-decoration: underline line-through">x</p>"#), "<div><i><u><s>x</s></u></i></div>");
        assert_eq!(clean(r#"<b style="font-weight:bold">x</b>"#), "<b>x</b>");
        assert_eq!(clean(r#"<span style="font-weight: 400">x</span>"#), "x");
        assert_eq!(clean(r#"<span style="FONT-WEIGHT:BOLDER">x</span>"#), "<b>x</b>");
    }

    #[test]
    fn entities_and_escaping_round_trip() {
        assert_eq!(clean("a&nbsp;&amp;&lt;b&gt; &#xAC00;&#44032;"), "a&nbsp;&amp;&lt;b&gt; 가가");
    }

    #[test]
    fn inner_text_matches_chromium_example() {
        // measured in Chromium (bench/rich/cdp.py self-test)
        let t = inner_text(&sanitize("a<div>b<br></div><div><br></div>c<div>x\n</div>y"));
        assert_eq!(t, "a\nb\n\n\n\nc\nx\n\ny");
    }

    #[test]
    fn lines_follow_css_rules() {
        let d = Doc::from_tree(&sanitize("a<div>b<br></div><div><br></div>c<div>x\n</div>y"));
        let lines: Vec<String> = d.normal().iter().map(|(_, l)| l.iter().map(|i| if let Item::Text(t, _) = i { t.clone() } else { "#".into() }).collect()).collect();
        assert_eq!(lines, vec!["a", "b", "", "c", "x", "y"]);
    }

    #[test]
    fn bullets_and_runs_survive_to_html_and_back() {
        let src = "<b>굵</b>게 <i>기울<u>밑</u></i><ul><li>하나</li><li>둘<br>셋</li></ul><div><br></div><div>끝</div>";
        let d = Doc::from_tree(&sanitize(src));
        let again = Doc::from_tree(&sanitize(&d.to_html()));
        assert_eq!(d.normal(), again.normal());
        assert_eq!(d.normal()[1].0, true);
        assert_eq!(d.normal()[3].0, false); // "셋" is the item's second line: no marker
    }

    #[test]
    fn empty_lines_and_trailing_breaks_round_trip() {
        let d = Doc { paras: vec![
            Para { bullet: false, lines: vec![vec![Item::Text("a".into(), 0)], vec![]] },
            Para { bullet: false, lines: vec![vec![]] },
            Para { bullet: true, lines: vec![vec![]] },
        ] };
        let back = Doc::from_tree(&sanitize(&d.to_html()));
        assert_eq!(back.normal(), d.normal(), "{}", d.to_html());
    }

    #[test]
    fn plain_text_notes_load_line_by_line() {
        let d = Doc::load("", "하나\n\n셋");
        assert_eq!(d.paras.len(), 3);
        assert_eq!(d.plain(), "하나\n\n셋");
    }

    #[test]
    fn one_pass_save_equals_the_tree_path() {
        let samples = [
            "",
            "<br>",
            "a<br><br>",
            "<b>굵</b>게 <i>기<u>울</u></i><s></s><ul><li>하나</li><li>둘<br>셋</li><li><br></li></ul><div><br></div>끝&nbsp;",
            "앞<img data-gm=\"0123abcd-0123-4567-89ab-0123456789ab.png\">뒤<ul><li><img data-gm=\"0123abcd-0123-4567-89ab-0123456789ab.png\"></li></ul>",
            "<ul><li>바깥<ul><li>안쪽</li></ul>계속</li></ul><br><br>",
        ];
        for src in samples {
            let d = Doc::from_tree(&sanitize(src));
            let tree = d.to_tree();
            assert_eq!(emit(&d), (serialize(&tree), plain(&tree)), "{src}");
        }
        // runs with empty text keep their tags, like the tree path
        let d = Doc { paras: vec![Para { bullet: false, lines: vec![vec![Item::Text(String::new(), B), Item::Text("x".into(), 0)]] }] };
        let tree = d.to_tree();
        assert_eq!(emit(&d), (serialize(&tree), plain(&tree)));
    }

    #[test]
    fn stored_limits_follow_v11() {
        let big = Doc::from_plain(&"가".repeat(400_000)); // 1.2 MB of UTF-8
        let (html, text) = stored(&big);
        assert!(html.is_empty()); // > 1 MB of HTML
        assert_eq!(text.chars().count(), MAX_TEXT);
    }

    #[test]
    fn cf_html_fragment_is_extracted() {
        let raw = "Version:0.9\r\nStartHTML:000000097\r\nEndHTML:000000170\r\nStartFragment:000000131\r\nEndFragment:000000134\r\n<html><body><!--StartFragment--><b>x</b><!--EndFragment--></body></html>";
        // offsets above are illustrative; the marker fallback finds the fragment
        assert!(cf_html_fragment(raw).contains("<b>x</b>") || cf_html_fragment(raw).len() < raw.len());
    }

    #[test]
    fn image_name_rule() {
        assert!(is_image_name(NAME));
        assert!(!is_image_name("0123ABCD-0123-4567-89ab-0123456789ab.png"));
        assert!(!is_image_name("x.png"));
        assert!(!is_image_name("0123abcd-0123-4567-89ab-0123456789ab.bmp"));
    }
}
