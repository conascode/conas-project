"""Builds docs/GaeunMemo2_Technical_Report.pdf from report.html with headless Edge
on Windows, or Chromium through Node + Playwright elsewhere (headless Chromium's
--print-to-pdf can hang in sandboxed Linux containers).

Pass 1 prints the PDF, then reads the named destinations Chromium writes for
each TOC link target to learn which page every section starts on. Pass 2 writes
those page numbers into the TOC and prints again. Standard library only.

    python -I docs/build_pdf.py
"""
import re
import subprocess
import sys
import zlib
from pathlib import Path

DOCS = Path(__file__).resolve().parent
SRC = DOCS / "report.html"
TMP = DOCS / "_report_build.html"
OUT = DOCS / "GaeunMemo2_Technical_Report.pdf"
EDGE = r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"

PLAYWRIGHT_JS = r"""
const {chromium} = require('playwright');
(async () => {
  const b = await chromium.launch();
  const p = await b.newPage();
  await p.goto(process.argv[1], {waitUntil: 'load'});
  await p.evaluate(() => document.fonts.ready);
  await p.pdf({path: process.argv[2], preferCSSPageSize: true, printBackground: true,
               displayHeaderFooter: false, tagged: true});
  await b.close();
})().catch(e => { console.error(e); process.exit(1); });
"""


def print_pdf(html: Path, pdf: Path) -> None:
    if pdf.exists():
        pdf.unlink()
    if sys.platform != "win32":
        r = subprocess.run(["node", "-e", PLAYWRIGHT_JS, html.as_uri(), str(pdf)],
                           capture_output=True, timeout=180)
        if r.returncode or not pdf.exists():
            sys.exit(f"Playwright did not produce a PDF\n{r.stderr.decode(errors='replace')[-2000:]}")
        return
    profile = Path(__import__("tempfile").gettempdir()) / "gm-report-edge-profile"
    r = subprocess.run(
        [EDGE, "--headless=new", f"--user-data-dir={profile}", "--no-first-run",
         "--disable-extensions", "--no-pdf-header-footer",
         "--run-all-compositor-stages-before-draw", "--virtual-time-budget=4000",
         f"--print-to-pdf={pdf}", html.as_uri()],
        capture_output=True, timeout=180,
    )
    # Edge may finish writing after the launcher process exits: wait for a stable file
    import time
    last = -1
    for _ in range(120):
        size = pdf.stat().st_size if pdf.exists() else -1
        if size > 0 and size == last:
            break
        last = size
        time.sleep(0.5)
    if not pdf.exists():
        sys.exit(f"Edge did not produce a PDF (exit {r.returncode})\n{r.stderr.decode(errors='replace')[-2000:]}")


def objects(data: bytes) -> dict:
    """obj number -> raw body (streams inflated when possible)."""
    out = {}
    for m in re.finditer(rb"(\d+) 0 obj(.*?)endobj", data, re.S):
        num, body = int(m.group(1)), m.group(2)
        s = re.search(rb"stream\r?\n(.*?)\r?\nendstream", body, re.S)
        if s and b"/FlateDecode" in body[: s.start()]:
            try:
                body = body[: s.start()] + zlib.decompress(s.group(1))
            except zlib.error:
                pass
        out[num] = body
    # object streams (PDF 1.5) may hold the dictionaries we need
    for num, body in list(out.items()):
        if b"/Type /ObjStm" in body or b"/Type/ObjStm" in body:
            n = int(re.search(rb"/N (\d+)", body).group(1))
            first = int(re.search(rb"/First (\d+)", body).group(1))
            s = re.search(rb">>\s*stream\r?\n", body)
            raw = body[s.end():] if s else body
            try:
                raw = zlib.decompress(raw)
            except zlib.error:
                pass
            head = raw[:first].split()
            for i in range(n):
                onum, off = int(head[2 * i]), int(head[2 * i + 1])
                end = int(head[2 * i + 3]) if i + 1 < n else len(raw) - first
                out.setdefault(onum, raw[first + off: first + end])
    return out


def page_order(objs: dict) -> list:
    root = None
    for num, body in objs.items():
        if re.search(rb"/Type\s*/Pages", body) and b"/Parent" not in body:
            root = num
            break
    pages = []

    def walk(n):
        body = objs[n]
        kids = re.search(rb"/Kids\s*\[(.*?)\]", body, re.S)
        if kids:
            for k in re.findall(rb"(\d+) 0 R", kids.group(1)):
                walk(int(k))
        else:
            pages.append(n)

    walk(root)
    return pages


def dest_pages(pdf: Path) -> dict:
    data = pdf.read_bytes()
    objs = objects(data)
    order = {obj: i + 1 for i, obj in enumerate(page_order(objs))}
    found = {}
    blob = b"\n".join(objs.values())
    # named destinations: /name [N 0 R /XYZ ...]  or (name) [N 0 R ...]
    for m in re.finditer(rb"[/(]([A-Za-z0-9_\-]+)\)?\s*\[\s*(\d+) 0 R\s*/XYZ", blob):
        name, pg = m.group(1).decode(), int(m.group(2))
        if pg in order:
            found[name] = order[pg]
    # indirect dests: (name) 12 0 R  ->  12 0 obj [N 0 R /XYZ ...]
    for m in re.finditer(rb"[/(]([A-Za-z0-9_\-]+)\)?\s+(\d+) 0 R", blob):
        name, ref = m.group(1).decode(), int(m.group(2))
        body = objs.get(ref, b"")
        d = re.match(rb"\s*\[\s*(\d+) 0 R\s*/XYZ", body)
        if d and int(d.group(1)) in order:
            found.setdefault(name, order[int(d.group(1))])
    return found, len(order)


def main():
    html = SRC.read_text(encoding="utf-8")
    TMP.write_text(html, encoding="utf-8")
    print_pdf(TMP, OUT)
    pages, total = dest_pages(OUT)
    wanted = re.findall(r'data-for="([^"]+)"', html)
    missing = [w for w in wanted if w not in pages]
    print(f"pass 1: {total} pages, {len(pages)} destinations, missing: {missing}")
    filled = re.sub(
        r'(<span class="pn" data-for="([^"]+)">)(</span>)',
        lambda m: m.group(1) + str(pages.get(m.group(2), "?")) + m.group(3),
        html,
    )
    TMP.write_text(filled, encoding="utf-8")
    print_pdf(TMP, OUT)
    pages2, total2 = dest_pages(OUT)
    drift = {k: (pages[k], pages2.get(k)) for k in pages if pages2.get(k) != pages[k]}
    print(f"pass 2: {total2} pages, drift: {drift}")
    for w in wanted:
        print(f"  {w:6} -> p.{pages2.get(w, '?')}")
    TMP.unlink()
    print("written", OUT, OUT.stat().st_size, "bytes")


if __name__ == "__main__":
    main()
