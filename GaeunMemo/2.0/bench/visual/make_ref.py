"""v1.1 reference screenshots for the visual regression (report §14).

Builds one page per case from v1.1's own ui/note.html or ui/list.html markup, its CSS
and JS, with a fake Tauri bridge that answers note_get / list_get, applies the case
state, then has headless Edge screenshot it at the v2 capture scale (150 %).

    python -I bench/visual/make_ref.py [filter]
"""
import json
import re
import subprocess
import sys
import tempfile
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent                      # gaeunmemo2/
UI = ROOT.parent / "gaeunmemo" / "ui"          # v1.1 frontend
OUT = HERE.parent / "results" / "visual"
EDGE = r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"
SCALE = 1.5
PALETTE = ["#AFA9EC", "#F4C0D1", "#9FE1CB", "#F6C9A0", "#B5D4F4", "#F3E9C6"]

# same sample notes as app::load_demo (title, text, colour, age ms, open)
DEMO = [
    ("장보기", "우유 2개\n계란 한 판\n사과 1 kg — 토요일 시장에서", 0, 3 * 60_000, True),
    ("회의 메모", "다음 주 화요일 오후 3시\n3층 회의실, 노트북 지참", 4, 2 * 3_600_000, True),
    ("", "제목 없는 메모도 목록에 보여요", 2, 26 * 3_600_000, True),
    # the P2 sample note: its text copy (Rich.plain of the formatted body in app::load_demo)
    ("GaeunMemo 2.0", "P2: 굵게, 기울임, 밑줄, 취소선\n글머리 기호\n붙여넣기는 지원 서식만 남겨요\n사진은 전체화면의 사진 넣기나 붙여넣기로", 3, 30_000, True),
    ("지난주 할 일", "보고서 정리\n사진 백업", 1, 5 * 86_400_000, False),
]


def body_markup(page):
    html = (UI / page).read_text(encoding="utf-8")
    return re.search(r"<body>(.*)</body>", html, re.S).group(1)


def cases(filter_):
    for line in (HERE / "cases.tsv").read_text(encoding="utf-8").splitlines():
        if not line.strip() or line.startswith("#"):
            continue
        f = line.split("\t") + [""] * 9
        name = f[0]
        if filter_ and filter_ not in name:
            continue
        yield {
            "name": name, "theme": f[1], "band": f[2], "w": int(f[3]), "h": int(f[4]),
            "color": int(f[5] or 0), "states": [s for s in f[6].split(",") if s and s != "-"],
            "title": f[7], "text": f[8].replace("\\n", "\n"),
        }


def page(c):
    is_list = c["name"].startswith("list_")
    st = c["states"]
    has = lambda s: any(x == s or x.startswith(s + "=") for x in st)
    uri = lambda n: (UI / n).as_uri()
    note = {
        "title": c["title"], "text": c["text"], "html": "", "color": PALETTE[c["color"]],
        "pinned": has("pinned"), "folded": has("folded"), "theme": c["theme"], "band": c["band"],
        "fontSize": 14, "palette": PALETTE, "locked": has("locked"), "maximized": has("full"),
    }
    demo = [{"id": f"n{i}", "title": t, "text": x, "color": PALETTE[col], "open": o, "locked": False, "age": age}
            for i, (t, x, col, age, o) in enumerate(DEMO)]
    search = next((s.split("=", 1)[1] for s in st if s.startswith("search=")), "")
    shim = f"""
<script>
const NOTE = {json.dumps(note, ensure_ascii=False)};
const DEMO = {json.dumps(demo, ensure_ascii=False)};
const FULL = {json.dumps(has("full"))};
// v1.1 decides 전체화면 from screen.availWidth/Height
Object.defineProperty(screen, 'availWidth', {{ get: () => FULL ? innerWidth : 10000 }});
Object.defineProperty(screen, 'availHeight', {{ get: () => FULL ? innerHeight : 10000 }});
window.__TAURI__ = {{
  core: {{ invoke: async (cmd, args) => {{
    if (cmd === 'note_get') return NOTE;
    if (cmd === 'list_get') return {{ theme: NOTE.theme, notes: DEMO.map(d => ({{ id: d.id, title: d.title, text: d.text, color: d.color, open: d.open, locked: d.locked, updatedAt: Date.now() - d.age }})) }};
    if (cmd === 'note_toggle_fold') {{ NOTE.folded = !NOTE.folded; return NOTE.folded; }}
    if (cmd === 'note_set_lock') return args.locked;
    return null;
  }} }},
  event: {{ listen: async () => () => {{}} }},
  window: {{ getCurrentWindow: () => ({{ label: 'note-ref', startDragging: async () => {{}} }}) }},
}};
</script>"""
    G = "document.getElementById"
    after = []
    # headless Edge has no pointer, so states a mouse user always sees are forced with CSS:
    #  hover - body:hover shows the tools
    #  menu  - opened by clicking ···, so the pointer is inside the window (tools shown) and
    #          the first item's programmatic focus() is not :focus-visible (no ring/fill)
    style = ""
    if has("hover") or has("menu"):
        style += ".tools{display:flex!important}"
    if has("menu"):
        style += ".menu-item:focus-visible{background:none!important;outline:none!important}"
    style = f"<style>{style}</style>" if style else ""
    if has("copied"):
        after.append(f"{G}('bar').classList.add('show-copied'); {G}('copied').hidden = false;")
    if has("menu"):
        after.append(f"document.activeElement && document.activeElement.blur(); {G}('more').click();")
    if has("confirm"):
        after.append(f"{G}('confirm').hidden = false;")
    if has("rename"):
        after.append("document.querySelector('[data-act=\"rename\"]').click();")
    if search:
        after.append(f"const s = {G}('search'); s.value = {json.dumps(search)}; s.dispatchEvent(new Event('input'));")
    if has("ctx"):
        x, y = 120, 160
        after.append(f"const el = document.elementFromPoint({x}, {y}).closest('.row'); el.dispatchEvent(new MouseEvent('contextmenu', {{bubbles: true, cancelable: true, clientX: {x}, clientY: {y}}}));")
    if has("select"):
        # body focused, all of its text selected (::selection = note colour 65 %)
        after.append(f"const b = {G}('body'); b.focus(); const r = document.createRange(); r.selectNodeContents(b); const sel = getSelection(); sel.removeAllRanges(); sel.addRange(r);")
    if not has("rename") and not has("menu") and not search and not has("select"):
        after.append("document.activeElement && document.activeElement.blur && document.activeElement.blur();")
    if is_list and not search and not has("ctx"):
        after.append("document.activeElement && document.activeElement.blur();")
    run = "\n".join(after)
    kind = "list" if is_list else "note"
    css = f'<link rel="stylesheet" href="{uri("common.css")}"><link rel="stylesheet" href="{uri(kind + ".css")}">'
    scripts = "".join(f'<script src="{uri(n)}"></script>' for n in (["ui.js", "list.js"] if is_list else ["ui.js", "rich.js", "note.js"]))
    return f"""<!doctype html><html lang="ko" data-theme="{c['theme']}" data-band="{c['band']}"><head><meta charset="utf-8">{css}{style}{shim}</head>
<body>{body_markup(kind + '.html')}{scripts}
<script>setTimeout(() => {{ {run} }}, 400);</script></body></html>"""


def frame_page(case_html: Path, w, h):
    """headless Edge has a minimum window width (~500 px), so the case page runs in an
    iframe of exactly w x h CSS px at the top-left of a larger window; the shot is then
    cropped to w*SCALE x h*SCALE."""
    return (f'<!doctype html><html><head><meta charset="utf-8"><style>html,body{{margin:0;background:#808080}}'
            f'iframe{{display:block;border:0;width:{w}px;height:{h}px}}</style></head>'
            f'<body><iframe src="{case_html.as_uri()}"></iframe></body></html>')


def shoot(html: Path, png: Path, w, h):
    if png.exists():
        png.unlink()
    frame = html.with_suffix(".frame.html")
    frame.write_text(frame_page(html, w, h), encoding="utf-8")
    raw = png.with_suffix(".raw.png")
    if raw.exists():
        raw.unlink()
    profile = Path(tempfile.gettempdir()) / "gm-ref-edge-profile"
    subprocess.run([EDGE, "--headless=new", f"--user-data-dir={profile}", "--no-first-run", "--disable-extensions",
                    "--hide-scrollbars", f"--force-device-scale-factor={SCALE}", f"--window-size={max(w, 900)},{max(h, 700)}",
                    "--virtual-time-budget=3000", "--allow-file-access-from-files", f"--screenshot={raw}", frame.as_uri()],
                   capture_output=True, timeout=120)
    for _ in range(40):
        if raw.exists() and raw.stat().st_size > 0:
            break
        time.sleep(0.25)
    cw, ch = round(w * SCALE), round(h * SCALE)
    ps = ("Add-Type -AssemblyName System.Drawing; "
          f"$b=[System.Drawing.Bitmap]::FromFile('{raw}'); "
          f"$c=$b.Clone((New-Object System.Drawing.Rectangle 0,0,{cw},{ch}), [System.Drawing.Imaging.PixelFormat]::Format24bppRgb); "
          f"$c.Save('{png}', [System.Drawing.Imaging.ImageFormat]::Png); $c.Dispose(); $b.Dispose()")
    subprocess.run(["powershell", "-NoProfile", "-NonInteractive", "-Command", ps], capture_output=True, timeout=60)
    raw.unlink(missing_ok=True)


def main():
    filter_ = sys.argv[1] if len(sys.argv) > 1 else ""
    pages = OUT / "ref_html"
    ref = OUT / "ref"
    pages.mkdir(parents=True, exist_ok=True)
    ref.mkdir(parents=True, exist_ok=True)
    for c in cases(filter_):
        p = pages / f"{c['name']}.html"
        p.write_text(page(c), encoding="utf-8")
        shoot(p, ref / f"{c['name']}.png", c["w"], c["h"])
        print(c["name"], (ref / f"{c['name']}.png").exists())


if __name__ == "__main__":
    main()
