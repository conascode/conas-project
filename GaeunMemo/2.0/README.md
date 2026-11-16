# GaeunMemo 2.0 — P3 (storage: SQLite, pictures on disk)

Native rewrite planned in `../gaeunmemo/docs/GaeunMemo2_Technical_Report.pdf` (GM2-TDR-001).
No Tauri, no WebView2: Rust + Win32, one process.

- **P0** (done): Win32 skeleton and measurement prototypes → GDI chrome + RichEdit 5.0 body;
  GIFs shown as a static first frame.
- **P1** (done, allowed differences A1–A8 approved 2026-11-14): the v1.1 note / list / menu /
  confirm UI rebuilt in Win32 and checked against v1.1 pixel by pixel.
- **P2** (done): the body holds v1.1's formatted HTML — bold, italic, underline, strike,
  bullets, pictures — with v1.1's paste / drop / copy / lock rules.
- **P3** (this build): notes, settings, window places and pictures are **saved** — SQLite
  (WAL, synchronous=FULL) written by one background thread, pictures as files. Development
  builds use their own folder **`%APPDATA%\gaeunmemo2-dev\`** (user decision 2026-11-14) and
  never touch v1.1's `%APPDATA%\gaeunmemo\`; importing v1 data is P5.

## Try it

Run `dist\p3\gaeunmemo2.exe` (or `target\release\gaeunmemo2.exe` after a build) with no
arguments. The first start opens one empty note; what you write, colours, sizes, places,
전체화면 and pictures are there again after closing and restarting. `--data DIR` uses
another folder. Things to try:

- Ctrl+B / I / U, and in 전체화면 the formatting row (B I U S, bullets, 사진 넣기)
- paste from a web page or Word (only supported formatting stays), Ctrl+Shift+V (text only)
- paste a screenshot, drop a picture file, copy text between notes (formatting kept)
- lock a note (⋯ → 잠금): nothing can be typed or pasted, a click copies the text
- close everything and start again: notes, the list window and its place come back

## What is where

| Path | |
| --- | --- |
| `src/store.rs` | SQLite store: schema v1 (report §12), writer thread (merges waiting changes per note, one transaction), pictures written temp → flush → rename before the rows that refer to them, quick_check + backup restore at start, daily backup (7 kept), 24 h orphan picture clean-up |
| `src/log.rs` | `gaeunmemo.log` in the data folder (store failures, restores) |
| `src/rich.rs` | v1.1 `rich.js` in Rust: HTML tokenizer + tree builder, allow-list sanitizer, `innerHTML`, Chromium `innerText` (the `text` copy), on-screen document model, one-pass save |
| `src/richedit.rs`, `src/rtfread.rs` | The body ↔ document (RTF as in-memory transport), paste / drop routing (`IRichEditOleCallback`), copy / drag data object, picture refit after a resize |
| `src/images.rs` | Pictures: sniffing (png / jpg / gif / webp, ≤ 25 MB), decoding thread (display-size copies, generation numbers, ends after 60 s idle), 64 MB in-memory limit, placeholder for missing files, 사진 넣기 picker |
| `src/sbar.rs` | v1.1's scroll bar (8 px, square thumb in the rule colour on the paper), drawn over the note body and the list; the body's system bar stays outside its window region and only supplies the range |
| `src/note.rs`, `src/body.rs`, `src/app.rs` | Note window, RichEdit body, model; every change goes to the store; saved places restored (off-screen places fall back). The body scrolls by the wheel itself (v1.1 distance, at once, no animation) and draws large repaints off-screen (no flash); pictures follow a resize in the same `WM_SIZE` (`images::live`), the sharp copy comes once the size settles |
| `src/p3check.rs`, `bench/p3/kill.py` | `--mode p3check` (store checks on a new test folder) and the forced-termination test (`--mode p3kill` child, killed 0 / 100 / 350 ms after typing) |
| `src/p2check.rs`, `bench/rich/*` | P2 checks and the differential test against v1.1's real `ui/rich.js` |
| `src/selftest.rs`, `src/visual.rs`, `bench/visual/*` | P1 behaviour checks and the visual regression against v1.1 |
| `src/render.rs`, `src/bench.rs`, `src/metrics.rs`, `src/ptd.rs` | P0 prototypes and the measurement harness |

## Checks

Automated runs build into `target\auto` so a copy you are running from `target\release`
never blocks them.

```
cargo test --offline --release --target-dir target\auto
cargo build --offline --release --target-dir target\auto
target\auto\release\gaeunmemo2.exe --mode selftest --out bench\results\selftest.jsonl
python -I bench\rich\diff.py 600 1
target\auto\release\gaeunmemo2.exe --mode p2check --in CASES --out bench\results\p2check.jsonl
target\auto\release\gaeunmemo2.exe --mode p3check --data NEW_TEST_FOLDER --out bench\results\p3check.jsonl
python -I bench\p3\kill.py 50
target\auto\release\gaeunmemo2.exe --mode shots --outdir bench\results\visual\v2
target\auto\release\gaeunmemo2.exe --mode diff --refdir bench\results\visual\ref --outdir bench\results\visual\v2
```

`CASES` is the case file `diff.py` writes (HTML snippets separated by U+001E).

To see how stored notes render (e.g. a picture a user reports), copy a data folder and run
`target\auto\release\gaeunmemo2.exe --mode notesnap --data COPY --outdir DIR` — it opens the
copy's open notes off-screen at their saved sizes and saves one PNG per note. Never point
it at a live data folder.

Safety: tests use new folders of their own (`--data`), never `%APPDATA%`; test and
measurement windows are off-screen and never focused; pastes in tests are in-process data
objects (the clipboard is never touched); an unknown `--mode` exits instead of opening
windows; one process per data folder; the registry is never touched and the installed
GaeunMemo 1.x is never started.
