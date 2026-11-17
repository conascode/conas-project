//! `--mode p5check --data DIR --out FILE [--v1 COPY]`: P5 checks (report §16, 17장 P5 exit:
//! damage injection, round trip on a copy of real data). Everything happens in DIR, a new
//! test folder; COPY, when given, is a copy of a real v1.1 data folder and is only read.

use crate::metrics::{self, Json};
use crate::migrate::{self, Bounds, Data, ListState, Note, Settings};
use crate::store;
use std::path::{Path, PathBuf};
use std::time::Instant;

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

const PNG_A: &str = "0123abcd-0123-4567-89ab-0123456789ab.png";
const PNG_B: &str = "0123abcd-0123-4567-89ab-0123456789ac.png";
const PNG_GONE: &str = "0123abcd-0123-4567-89ab-0123456789ad.png";

/// A v1.1 data folder with `n` ordinary notes plus every case that is easy to get wrong.
fn fixture(dir: &Path, n: usize) -> Data {
    let _ = std::fs::create_dir_all(dir.join("images"));
    let png = crate::images::sample_png(40, 30);
    let _ = std::fs::write(dir.join("images").join(PNG_A), &png);
    let _ = std::fs::write(dir.join("images").join(PNG_B), &png);
    let mut notes = Vec::new();
    for i in 0..n {
        notes.push(Note {
            id: format!("00000000-0000-4000-8000-{i:012}"),
            title: if i % 3 == 0 { String::new() } else { format!("메모 {i}") },
            text: format!("줄 {i}\n둘째 줄 😀\n셋째"),
            color: migrate::PALETTE[i % 6].to_string(),
            pinned: i % 7 == 0,
            folded: i % 5 == 0,
            open: i % 4 == 0,
            bounds: if i % 2 == 0 { Some(Bounds { x: 100.0 + i as f64, y: 50.5, width: 260.0, height: 300.0 }) } else { None },
            updated_at: 1_790_000_000_000 + i as u64,
            font_size: 10 + (i % 23) as u8,
            ..Default::default()
        });
    }
    // CRLF text (v1.0 era), emoji and a lone-surrogate-free supplementary character
    notes.push(Note { id: "crlf".into(), title: "윈도 줄바꿈".into(), text: "하나\r\n둘\r\n셋 𝄞".into(), ..Default::default() });
    // the largest body v1.1 accepts
    notes.push(Note { id: "big".into(), title: "큰 메모".into(), text: "가".repeat(200_000), ..Default::default() });
    // v1.1 formatting with pictures (one picture file missing)
    notes.push(Note {
        id: "rich".into(),
        title: "서식".into(),
        text: "굵게 기울임\n항목".into(),
        html: format!(r#"<b>굵게</b> <i>기울임</i><ul><li>항목</li></ul><img data-gm="{PNG_A}"><img data-gm="{PNG_B}"><img data-gm="{PNG_GONE}">"#),
        locked: true,
        maximized: true,
        ..Default::default()
    });
    let data = Data {
        notes,
        list: ListState { open: true, bounds: Some(Bounds { x: 1400.0, y: 72.0, width: 280.0, height: 380.0 }) },
        settings: Settings { theme: "light".into(), next_color: 4, initialized: true, band: "full".into(), autostart: Some(true) },
    };
    // a v1.0 note (no html / locked / maximized fields) and the numeric band of old files
    let mut v = serde_json::to_value(&data).unwrap();
    v["notes"].as_array_mut().unwrap().push(serde_json::json!({"id":"v10","title":"옛 메모","text":"v1.0","color":"#B5D4F4","open":true,"bounds":null,"updatedAt":5,"fontSize":14}));
    v["settings"]["band"] = serde_json::json!(0);
    let mut bytes = vec![0xEF, 0xBB, 0xBF]; // BOM: v1 tolerates one
    bytes.extend(serde_json::to_vec(&v).unwrap());
    let _ = std::fs::write(dir.join("notes.json"), &bytes);
    migrate::parse_v1(&String::from_utf8_lossy(&bytes)).unwrap()
}

/// Field-by-field: what the store gave back (`got`, as exported) against what v1 had
/// (`want`, after v1.1's own sanitize), with the documented normalisations (LF line ends,
/// the allow-list once). Returns the ids that differ.
fn compare(want: &Data, got: &Data) -> Vec<String> {
    let mut bad = Vec::new();
    for w in &want.notes {
        let p = migrate::plan_note(w);
        let Some(g) = got.notes.iter().find(|g| g.id == w.id) else {
            bad.push(format!("{} missing", w.id));
            continue;
        };
        let same = g.title == p.title
            && g.text == p.text
            && g.html == p.html
            && g.color == w.color
            && g.pinned == w.pinned
            && g.folded == w.folded
            && g.open == w.open
            && g.locked == w.locked
            && g.maximized == w.maximized
            && g.bounds == w.bounds
            && g.font_size == w.font_size
            && g.updated_at == w.updated_at;
        if !same {
            bad.push(w.id.clone());
        }
    }
    bad
}

fn open_import(dev: &Path, v1: &Path, force: bool) -> (Option<String>, usize) {
    let sticky = dev.join("no-sticky.json");
    match store::open_with(dev, |c, d| migrate::on_open(c, d, v1, &sticky, force)) {
        Ok(l) => {
            let n = l.notes.len();
            store::close();
            (l.step, n)
        }
        Err(e) => (Some(format!("open failed: {e}")), 0),
    }
}

fn meta(dev: &Path, key: &str) -> Option<String> {
    let c = rusqlite::Connection::open(dev.join("gaeunmemo.db")).ok()?;
    c.query_row("SELECT value FROM meta WHERE key=?1", [key], |r| r.get(0)).ok()
}

pub fn run(data: &str, out: &str, real: &str) {
    let base = PathBuf::from(data);
    let mut t = T { out: Vec::new(), pass: 0, fail: 0 };
    if data.is_empty() || base.exists() {
        t.check("test folder is new (--data)", false, data.to_string());
        metrics::append(out, &Json::default().s("mode", "p5check").raw("checks", format!("[{}]", t.out.join(","))).finish());
        return;
    }
    let v1 = base.join("v1");
    let dev = base.join("dev");
    let n = 1000;
    let src = fixture(&v1, n);
    let src_bytes = std::fs::read(v1.join("notes.json")).unwrap_or_default();

    // ---- ② – ⑤ import into a new store
    let t0 = Instant::now();
    let (step, loaded) = open_import(&dev, &v1, false);
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    let total = src.notes.len();
    t.check("import: every note in one go (1,000 + edge cases)", loaded == total, format!("{loaded} / {total} notes, {ms:.0} ms; {}", step.clone().unwrap_or_default()));
    t.check("import: pictures copied before the rows, a missing one counted", dev.join("images").join(PNG_A).exists() && dev.join("images").join(PNG_B).exists() && meta(&dev, "migrated_images_missing").as_deref() == Some("1"), String::new());
    t.check("import: meta records source and SHA-256", meta(&dev, "v1_sha256").as_deref() == Some(migrate::sha256(&src_bytes).as_str()) && meta(&dev, "migrated_from").is_some_and(|m| m.starts_with("v1 ")), String::new());
    t.check("import: the source file is not changed", std::fs::read(v1.join("notes.json")).ok().as_deref() == Some(src_bytes.as_slice()), String::new());

    // ---- round trip v1 → v2 → v1
    let exported = migrate::export(&dev).unwrap_or_default();
    let bad = compare(&src, &exported);
    t.check("round trip: every note field comes back (v1 → v2 → v1)", bad.is_empty() && exported.notes.len() == total, format!("{} differ: {}", bad.len(), bad.iter().take(5).cloned().collect::<Vec<_>>().join(", ")));
    t.check("round trip: list window and settings", exported.list == src.list && exported.settings.theme == "light" && exported.settings.band == "none" && exported.settings.next_color == 4 && exported.settings.initialized && exported.settings.autostart == Some(true), format!("{:?} / {:?}", exported.list, exported.settings));
    let order: Vec<&str> = exported.notes.iter().map(|n| n.id.as_str()).collect();
    let want_order: Vec<&str> = src.notes.iter().map(|n| n.id.as_str()).collect();
    t.check("round trip: order kept (seq = v1 array position)", order == want_order, String::new());
    let file = base.join("exported.json");
    let file_ok = migrate::export_file(&dev, &file).is_ok() && std::fs::read(&file).ok().and_then(|b| migrate::parse_v1(&String::from_utf8_lossy(&b)).ok()).is_some_and(|d| compare(&src, &d).is_empty());
    t.check("export-v1 writes a notes.json that v1.1's reader loads unchanged", file_ok, String::new());

    // ---- once only; merge; re-import adds only what is new
    let (step2, again) = open_import(&dev, &v1, false);
    t.check("a second start does not import again", step2.is_none() && again == total, format!("{again} notes"));
    let mut more = src.clone();
    more.notes.push(Note { id: "new-in-v1".into(), title: "나중에 쓴 메모".into(), ..Default::default() });
    let _ = std::fs::write(v1.join("notes.json"), serde_json::to_vec(&more).unwrap());
    let (step3, after) = open_import(&dev, &v1, true);
    t.check("--reimport-v1 adds only notes not there yet", after == total + 1 && step3.as_deref().is_some_and(|s| s.contains("→ 1 notes")), step3.unwrap_or_default());
    let merged = base.join("merged");
    {
        let _ = store::open(&merged);
        store::send(store::Cmd::Upsert(store::Row {
            id: "rich".into(), // same id as a v1 note: the v2 note wins
            title: "v2에서 쓴 메모".into(),
            body_text: String::new(),
            body_html: String::new(),
            color: 0,
            pinned: false,
            folded: false,
            is_open: true,
            locked: false,
            maximized: false,
            rect: None,
            font_size: 14,
            seq: 0,
            created_at: 1,
            updated_at: 1,
        }));
        store::close();
    }
    let _ = std::fs::write(v1.join("notes.json"), &src_bytes);
    let (step4, m) = open_import(&merged, &v1, false);
    let kept = store::read_row(&merged.join("gaeunmemo.db"), "rich").map(|r| r.title).unwrap_or_default();
    t.check("merge: v2 notes stay, a note with the same id is skipped", m == total && kept == "v2에서 쓴 메모" && meta(&merged, "migrated_skipped").as_deref() == Some("1"), step4.unwrap_or_default());

    // ---- damage injection: nothing imported, nothing written next to the source
    for (label, body) in [
        ("cut off", src_bytes[..src_bytes.len() / 2].to_vec()),
        ("wrong types", br#"{"notes":[{"id":"a","fontSize":"big"}]}"#.to_vec()),
        ("not json", b"\x00\x01binary".to_vec()),
    ] {
        let bad_v1 = base.join(format!("bad-{}", label.replace(' ', "-")));
        let bad_dev = base.join(format!("dev-{}", label.replace(' ', "-")));
        let _ = std::fs::create_dir_all(&bad_v1);
        let _ = std::fs::write(bad_v1.join("notes.json"), &body);
        let (step, count) = open_import(&bad_dev, &bad_v1, false);
        let untouched = std::fs::read(bad_v1.join("notes.json")).ok().as_deref() == Some(body.as_slice()) && std::fs::read_dir(&bad_v1).map(|r| r.count()).unwrap_or(0) == 1;
        t.check(&format!("damaged source ({label}): nothing imported, source and its folder untouched"), count == 0 && meta(&bad_dev, "migrated_from").is_none() && untouched && step.as_deref().is_some_and(|s| s.contains("does not parse")), step.unwrap_or_default());
    }

    // ---- StickyMemo (format 2), when there is no v1.1 file
    {
        let sticky = base.join("sticky.json");
        let _ = std::fs::write(&sticky, r##"[{"id":"s1","text":"장보기\r\n우유","color":"#F4C0D1","pinned":true},{"id":"s2","text":"한 줄"}]"##);
        let sdev = base.join("dev-sticky");
        let r = store::open_with(&sdev, |c, d| migrate::on_open(c, d, &base.join("no-v1"), &sticky, false));
        let titles: Vec<String> = r.map(|l| l.notes.iter().map(|n| n.title.clone()).collect()).unwrap_or_default();
        store::close();
        t.check("StickyMemo format 2: first line becomes the title", titles == ["장보기", "한 줄"] && meta(&sdev, "migrated_from").is_some_and(|m| m.starts_with("stickymemo ")), format!("{titles:?}"));
    }

    // ---- a copy of real v1.1 data (read only), the same round trip
    if !real.is_empty() {
        let rv1 = PathBuf::from(real);
        let rdev = base.join("dev-real");
        let raw = std::fs::read(rv1.join("notes.json")).unwrap_or_default();
        match migrate::parse_v1(&String::from_utf8_lossy(&raw)) {
            Ok(rsrc) => {
                let (step, count) = open_import(&rdev, &rv1, false);
                let back = migrate::export(&rdev).unwrap_or_default();
                let bad = compare(&rsrc, &back);
                let normalized = rsrc.notes.iter().filter(|n| migrate::plan_note(n).html_normalized).count();
                t.check("real data copy: imported and back unchanged (v1 → v2 → v1)", count == rsrc.notes.len() && bad.is_empty() && back.settings.theme == rsrc.settings.theme && back.settings.band == rsrc.settings.band, format!("{count} notes, {} differ, html normalised {normalized}; {}", bad.len(), step.unwrap_or_default()));
                t.check("real data copy: the copy is not changed", std::fs::read(rv1.join("notes.json")).ok() == Some(raw), String::new());
            }
            Err(e) => t.check("real data copy parses", false, e),
        }
    }

    let line = Json::default().s("mode", "p5check").n("pass", t.pass as f64).n("fail", t.fail as f64).n("import_ms_1000", ms).raw("checks", format!("[{}]", t.out.join(","))).finish();
    metrics::append(out, &line);
    let _ = std::fs::remove_dir_all(&base);
}
