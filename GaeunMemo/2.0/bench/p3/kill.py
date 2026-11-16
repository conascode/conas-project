"""Forced-termination test of the P3 store (report §14.2).

    python -I bench/p3/kill.py [RUNS=50]

For each delay (0 / 100 / 350 ms after the last keystroke) the child
(`gaeunmemo2.exe --mode p3kill`) types a known text into a stored note in a fresh test
folder, then is killed (TerminateProcess). The database must then open and pass
quick_check every time; after 350 ms (> the 300 ms save throttle) the whole text must be
there, after 0 / 100 ms a prefix of it. Picture runs paste a picture and are killed 0–100 ms later:
no stored note may refer to a picture file that does not exist.
Writes bench/results/p3_kill.json. Test folders live in the temp folder and are removed.
"""
import json
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
EXE = ROOT / "target" / "auto" / "release" / "gaeunmemo2.exe"
OUT = ROOT / "bench" / "results"
TEXT = "강제 종료 시험 0123456789 마지막"


def run_once(delay_ms, label=""):
    d = Path(tempfile.mkdtemp(prefix="gm-p3kill-"))
    args = [str(EXE), "--mode", "p3kill", "--data", str(d)] + (["--label", label] if label else [])
    p = subprocess.Popen(args)
    marker = d / "typed.txt"
    t0 = time.time()
    while not marker.exists() and time.time() - t0 < 30 and p.poll() is None:
        time.sleep(0.002)
    ok_start = marker.exists()
    time.sleep(delay_ms / 1000)
    p.kill()
    p.wait()
    res = {"delay_ms": delay_ms, "label": label, "started": ok_start}
    try:
        c = sqlite3.connect(str(d / "gaeunmemo.db"))
        res["quick_check"] = c.execute("PRAGMA quick_check").fetchone()[0]
        row = c.execute("SELECT body_text, body_html FROM note WHERE title='강제 종료'").fetchone()
        text, html = (row or ("", ""))
        res["kept"] = text
        res["full"] = text == TEXT
        res["prefix"] = TEXT.startswith(text)
        # every stored picture reference has its file
        names = [r[0] for r in c.execute("SELECT name FROM note_image")]
        res["refs"] = len(names)
        res["broken_refs"] = sum(1 for n in names if not (d / "images" / n).exists())
        res["files"] = len([f for f in (d / "images").iterdir() if not f.name.endswith(".tmp")])
        c.close()
    except Exception as e:  # noqa: BLE001 — a failure to open is a test result
        res["error"] = repr(e)
    shutil.rmtree(d, ignore_errors=True)
    return res


def main():
    runs = int(sys.argv[1]) if len(sys.argv) > 1 else 50
    results = []
    for delay in (0, 100, 350):
        for _ in range(runs):
            results.append(run_once(delay))
    # pictures: killed 0, 5, 10 … 100 ms after the paste, so some runs die before the row is
    # committed and some after — no run may leave a reference without its file
    for k in range(max(21, runs // 2)):
        results.append(run_once((k * 5) % 105, "picture"))
    summary = {}
    for key in ("0", "100", "350", "picture"):
        rs = [r for r in results if (r["label"] == "picture") == (key == "picture") and (key == "picture" or str(r["delay_ms"]) == key)]
        summary[key] = {
            "runs": len(rs),
            "opened_ok": sum(1 for r in rs if r.get("quick_check") == "ok"),
            "full_text": sum(1 for r in rs if r.get("full")),
            "prefix_ok": sum(1 for r in rs if r.get("prefix")),
            "broken_refs": sum(r.get("broken_refs", 0) for r in rs),
            "pictures_referenced": sum(r.get("refs", 0) for r in rs),
            "errors": [r["error"] for r in rs if "error" in r][:3],
            "not_started": sum(1 for r in rs if not r["started"]),
        }
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / "p3_kill.json").write_text(json.dumps(summary, ensure_ascii=False, indent=1), encoding="utf-8")
    print(json.dumps(summary, ensure_ascii=False))


if __name__ == "__main__":
    main()
