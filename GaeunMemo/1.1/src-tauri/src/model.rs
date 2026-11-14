//! Data model persisted to `%APPDATA%\gaeunmemo\notes.json`.
//! Every field has a default so partial or older files still load.

use serde::{Deserialize, Serialize};

pub const PALETTE: [&str; 6] = ["#AFA9EC", "#F4C0D1", "#9FE1CB", "#F6C9A0", "#B5D4F4", "#F3E9C6"];

pub const NOTE_W: f64 = 260.0;
pub const NOTE_H: f64 = 260.0;
pub const NOTE_MIN_W: f64 = 180.0;
pub const NOTE_MIN_H: f64 = 130.0;
pub const BAR_H: f64 = 34.0;
pub const LIST_W: f64 = 280.0;
/// Colour band modes: no band, a 4px band, or the whole title bar in the note colour.
pub const BANDS: [&str; 3] = ["none", "thin", "full"];
pub const BAND_DEFAULT: &str = "thin";
pub const THIN_BAND: f64 = 4.0;
pub const FONT_MIN: u8 = 10;
pub const FONT_MAX: u8 = 32;
pub const FONT_DEFAULT: u8 = 14;

/// Height of a folded note: the title bar, plus the band in "thin" mode.
pub fn folded_h(band: &str) -> f64 {
    if band == "thin" {
        BAR_H + THIN_BAND
    } else {
        BAR_H
    }
}

/// Accepts the current string form and the older numeric form (0/2/4/8 px).
fn de_band<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(match v {
        serde_json::Value::String(s) if BANDS.contains(&s.as_str()) => s,
        serde_json::Value::Number(n) if n.as_u64() == Some(0) => "none".into(),
        _ => BAND_DEFAULT.into(),
    })
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
pub const LIST_H: f64 = 380.0;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(default)]
pub struct Bounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Default for Bounds {
    fn default() -> Self {
        Bounds { x: 0.0, y: 0.0, width: NOTE_W, height: NOTE_H }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Note {
    pub id: String,
    pub title: String,
    pub text: String,
    pub color: String,
    pub pinned: bool,
    pub folded: bool,
    pub open: bool,
    pub bounds: Option<Bounds>,
    /// Last content edit, ms since the Unix epoch (0 = unknown).
    #[serde(rename = "updatedAt")]
    pub updated_at: u64,
    #[serde(rename = "fontSize")]
    pub font_size: u8,
    /// Formatted body (sanitised HTML subset). `text` stays the plain-text
    /// mirror used for search, previews, copying and older versions.
    pub html: String,
    /// Read-only: clicking the body copies it; edits are refused.
    pub locked: bool,
    /// Shown maximised ("전체화면"); `bounds` keep the normal size.
    pub maximized: bool,
}

impl Default for Note {
    fn default() -> Self {
        Note {
            id: new_id(),
            title: String::new(),
            text: String::new(),
            color: PALETTE[0].to_string(),
            pinned: false,
            folded: false,
            open: true,
            bounds: None,
            updated_at: 0,
            font_size: FONT_DEFAULT,
            html: String::new(),
            locked: false,
            maximized: false,
        }
    }
}

impl Note {
    pub fn is_empty(&self) -> bool {
        self.title.trim().is_empty() && self.text.trim().is_empty() && image_refs(&self.html).is_empty()
    }
}

/// Image file names a note's HTML refers to (`<img data-gm="NAME">`).
pub fn image_refs(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(i) = rest.find("data-gm=\"") {
        rest = &rest[i + 9..];
        if let Some(end) = rest.find('"') {
            let name = &rest[..end];
            if valid_image_name(name) {
                out.push(name.to_string());
            }
            rest = &rest[end..];
        } else {
            break;
        }
    }
    out
}

/// `<uuid>.<png|jpg|gif|webp>` — the only names the image store creates or serves.
pub fn valid_image_name(name: &str) -> bool {
    let Some((stem, ext)) = name.rsplit_once('.') else { return false };
    stem.len() == 36
        && stem.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
        && matches!(ext, "png" | "jpg" | "gif" | "webp")
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct ListState {
    pub open: bool,
    pub bounds: Option<Bounds>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub theme: String,
    pub next_color: usize,
    /// Set after the very first run (autostart enabled, welcome note shown).
    pub initialized: bool,
    /// Colour band mode: one of BANDS.
    #[serde(deserialize_with = "de_band")]
    pub band: String,
    /// The user's autostart choice. None = not recorded yet (older files).
    pub autostart: Option<bool>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { theme: "dark".into(), next_color: 0, initialized: false, band: BAND_DEFAULT.into(), autostart: None }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct Data {
    pub notes: Vec<Note>,
    pub list: ListState,
    pub settings: Settings,
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub fn valid_color(c: &str) -> bool {
    PALETTE.iter().any(|p| p.eq_ignore_ascii_case(c))
}

fn fix_bounds(b: &mut Bounds, min_w: f64, min_h: f64) {
    let ok = |v: f64| v.is_finite();
    if !ok(b.x) || b.x.abs() > 100_000.0 {
        b.x = 0.0;
    }
    if !ok(b.y) || b.y.abs() > 100_000.0 {
        b.y = 0.0;
    }
    if !ok(b.width) || b.width < min_w {
        b.width = min_w.max(if ok(b.width) { b.width } else { 0.0 });
    }
    if !ok(b.height) || b.height < min_h {
        b.height = min_h.max(if ok(b.height) { b.height } else { 0.0 });
    }
    b.width = b.width.min(4000.0);
    b.height = b.height.min(4000.0);
}

impl Data {
    /// Repairs anything a hand-edited or imported file could get wrong.
    pub fn sanitize(&mut self) {
        let mut seen = std::collections::HashSet::new();
        for n in &mut self.notes {
            if n.id.trim().is_empty() || !seen.insert(n.id.clone()) {
                n.id = new_id();
                seen.insert(n.id.clone());
            }
            match PALETTE.iter().find(|p| p.eq_ignore_ascii_case(&n.color)) {
                Some(p) => n.color = p.to_string(),
                None => n.color = PALETTE[0].to_string(),
            }
            if let Some(b) = n.bounds.as_mut() {
                fix_bounds(b, NOTE_MIN_W, NOTE_MIN_H);
            }
            n.font_size = n.font_size.clamp(FONT_MIN, FONT_MAX);
        }
        if !BANDS.contains(&self.settings.band.as_str()) {
            self.settings.band = BAND_DEFAULT.into();
        }
        if let Some(b) = self.list.bounds.as_mut() {
            fix_bounds(b, 200.0, 200.0);
        }
        if self.settings.theme != "light" {
            self.settings.theme = "dark".into();
        }
        self.settings.next_color %= PALETTE.len();
    }

    pub fn find(&self, id: &str) -> Option<&Note> {
        self.notes.iter().find(|n| n.id == id)
    }

    pub fn find_mut(&mut self, id: &str) -> Option<&mut Note> {
        self.notes.iter_mut().find(|n| n.id == id)
    }

    pub fn take_color(&mut self) -> String {
        let c = PALETTE[self.settings.next_color % PALETTE.len()].to_string();
        self.settings.next_color = (self.settings.next_color + 1) % PALETTE.len();
        c
    }
}

/// What the list window needs for one row (text is included for search).
#[derive(Serialize, Clone, Debug)]
pub struct Summary {
    pub id: String,
    pub title: String,
    pub text: String,
    pub color: String,
    pub open: bool,
    #[serde(rename = "updatedAt")]
    pub updated_at: u64,
    pub locked: bool,
}

impl From<&Note> for Summary {
    fn from(n: &Note) -> Self {
        Summary {
            id: n.id.clone(),
            title: n.title.clone(),
            text: n.text.clone(),
            color: n.color.clone(),
            open: n.open,
            updated_at: n.updated_at,
            locked: n.locked,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_repairs_bad_values() {
        let mut d: Data = serde_json::from_str(
            r##"{"notes":[{"id":"a","color":"red","bounds":{"x":1,"y":2,"width":10,"height":5}},
                          {"id":"a","color":"#afa9ec","fontSize":99}],
                "settings":{"theme":"purple","nextColor":99,"band":"wide"}}"##,
        )
        .unwrap();
        d.sanitize();
        assert_eq!(d.notes[0].color, PALETTE[0]);
        assert_eq!(d.notes[1].color, "#AFA9EC");
        assert_ne!(d.notes[0].id, d.notes[1].id);
        let b = d.notes[0].bounds.unwrap();
        assert_eq!((b.width, b.height), (NOTE_MIN_W, NOTE_MIN_H));
        assert_eq!(d.settings.theme, "dark");
        assert!(d.settings.next_color < PALETTE.len());
        assert_eq!(d.notes[1].font_size, FONT_MAX);
        assert_eq!(d.notes[0].font_size, FONT_DEFAULT);
        assert_eq!(d.settings.band, "thin");
    }

    #[test]
    fn image_refs_only_valid_names() {
        let ok = "0123abcd-0123-4567-89ab-0123456789ab.png";
        let html = format!(r#"a<img data-gm="{ok}">b<img data-gm="../evil.exe"><img data-gm="x.png">"#);
        assert_eq!(image_refs(&html), vec![ok.to_string()]);
        assert!(!valid_image_name("0123abcd-0123-4567-89ab-0123456789ab.svg"));
        let n = Note { html, ..Default::default() };
        assert!(!n.is_empty(), "a note holding only a picture is not empty");
    }

    #[test]
    fn older_files_get_new_fields_defaulted() {
        let d: Data = serde_json::from_str(r#"{"notes":[{"id":"a","text":"t"}]}"#).unwrap();
        assert_eq!((d.notes[0].html.as_str(), d.notes[0].locked, d.notes[0].maximized), ("", false, false));
    }

    #[test]
    fn folded_height_follows_band() {
        assert_eq!(folded_h("none"), 34.0);
        assert_eq!(folded_h("thin"), 38.0);
        assert_eq!(folded_h("full"), 34.0);
    }

    #[test]
    fn older_numeric_band_still_loads() {
        let band = |s: &str| serde_json::from_str::<Data>(s).unwrap().settings.band;
        assert_eq!(band(r#"{"settings":{"band":0}}"#), "none");
        assert_eq!(band(r#"{"settings":{"band":8}}"#), "thin");
        assert_eq!(band(r#"{"settings":{"band":"full"}}"#), "full");
        assert_eq!(band(r#"{"settings":{}}"#), "thin");
    }

    #[test]
    fn take_color_rotates() {
        let mut d = Data::default();
        let got: Vec<String> = (0..7).map(|_| d.take_color()).collect();
        assert_eq!(got[0], PALETTE[0]);
        assert_eq!(got[5], PALETTE[5]);
        assert_eq!(got[6], PALETTE[0]);
    }
}
