//! One-time import from the older StickyMemo app (`%APPDATA%\StickyMemo\notes.json`).

use crate::model::{Bounds, Data, Note};
use serde::Deserialize;
use std::path::{Path, PathBuf};

pub fn legacy_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("StickyMemo").join("notes.json"))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct LegacyNote {
    id: String,
    text: String,
    color: String,
    pinned: bool,
    bounds: Option<Bounds>,
}

/// Format 1: `{ notes: [...], list: {...} }` (same as ours).
/// Format 2: `[{ id, text, color, pinned, bounds }]`, first line of `text` is the title.
pub fn convert(s: &str) -> Option<Data> {
    let s = s.trim_start_matches('\u{feff}');
    let v: serde_json::Value = serde_json::from_str(s).ok()?;
    let mut data = if v.is_array() {
        let items: Vec<serde_json::Value> = serde_json::from_value(v).ok()?;
        let notes = items
            .into_iter()
            .filter_map(|it| serde_json::from_value::<LegacyNote>(it).ok())
            .map(|l| {
                let (title, text) = match l.text.split_once('\n') {
                    Some((t, rest)) => (t.trim_end_matches('\r').to_string(), rest.to_string()),
                    None => (l.text.clone(), String::new()),
                };
                Note {
                    id: l.id,
                    title,
                    text,
                    color: l.color,
                    pinned: l.pinned,
                    bounds: l.bounds,
                    ..Default::default()
                }
            })
            .collect();
        Data { notes, ..Default::default() }
    } else if v.is_object() {
        serde_json::from_value::<Data>(v).ok()?
    } else {
        return None;
    };
    data.sanitize();
    data.settings.next_color = data.notes.len() % crate::model::PALETTE.len();
    Some(data)
}

/// Never deletes or modifies the source file.
pub fn import_from(path: &Path) -> Option<Data> {
    let raw = std::fs::read(path).ok()?;
    convert(&String::from_utf8_lossy(&raw))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::PALETTE;

    #[test]
    fn format2_first_line_becomes_title() {
        let d = convert(
            r##"[{"id":"a","text":"장보기\r\n우유\n계란","color":"#F4C0D1","pinned":true,
                  "bounds":{"x":1,"y":2,"width":250,"height":240}},
                 {"id":"b","text":"한 줄"},
                 {"text":""}]"##,
        )
        .unwrap();
        assert_eq!(d.notes.len(), 3);
        assert_eq!(d.notes[0].title, "장보기");
        assert_eq!(d.notes[0].text, "우유\n계란");
        assert_eq!(d.notes[0].color, "#F4C0D1");
        assert!(d.notes[0].pinned && d.notes[0].open);
        assert_eq!(d.notes[0].bounds.unwrap().width, 250.0);
        assert_eq!((d.notes[1].title.as_str(), d.notes[1].text.as_str()), ("한 줄", ""));
        assert_eq!(d.notes[1].color, PALETTE[0]);
        assert!(!d.notes[2].id.is_empty());
    }

    #[test]
    fn format1_is_read_as_is() {
        let d = convert(r#"{"notes":[{"id":"x","title":"T","text":"B","open":false}],"list":{"open":true}}"#).unwrap();
        assert_eq!(d.notes[0].title, "T");
        assert!(!d.notes[0].open);
        assert!(d.list.open);
    }

    #[test]
    fn garbage_is_ignored() {
        assert!(convert("nope").is_none());
        assert!(convert("42").is_none());
    }

    #[test]
    fn source_file_is_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("notes.json");
        std::fs::write(&p, r#"[{"id":"a","text":"x"}]"#).unwrap();
        assert!(import_from(&p).is_some());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), r#"[{"id":"a","text":"x"}]"#);
    }
}
