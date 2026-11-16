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
import ctypes
import json
import os
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


def wait_for(path, p, limit=30):
    t0 = time.time()
    while not path.exists() and time.time() - t0 < limit and p.poll() is None:
        time.sleep(0.002)
    return path.exists()


def run_commit():
    """Killed inside a commit (after its statements, before COMMIT; store::test_hold):
    the database must open and hold exactly the text committed before."""
    d = Path(tempfile.mkdtemp(prefix="gm-p3kill-"))
    env = dict(os.environ, GM_TEST_COMMIT_HOLD_MS="5000")
    p = subprocess.Popen([str(EXE), "--mode", "p3kill", "--data", str(d), "--label", "commit"], env=env)
    started = wait_for(d / "in_commit", p)
    p.kill()
    p.wait()
    res = {"label": "commit", "started": started}
    try:
        first = (d / "first.txt").read_text(encoding="utf-8")
        c = sqlite3.connect(str(d / "gaeunmemo.db"))
        res["quick_check"] = c.execute("PRAGMA quick_check").fetchone()[0]
        row = c.execute("SELECT body_text FROM note WHERE title='강제 종료'").fetchone()
        res["kept"] = row[0] if row else None
        res["full"] = res["kept"] == first  # the earlier commit, whole; nothing of the killed one
        res["prefix"] = res["full"]
        c.close()
    except Exception as e:  # noqa: BLE001
        res["error"] = repr(e)
    shutil.rmtree(d, ignore_errors=True)
    return res


def run_endsession():
    """Log off: WM_QUERYENDSESSION + WM_ENDSESSION right after the last key, then killed
    at once (Windows ends the process after WM_ENDSESSION): the whole text must be there."""
    d = Path(tempfile.mkdtemp(prefix="gm-p3kill-"))
    p = subprocess.Popen([str(EXE), "--mode", "p3kill", "--data", str(d), "--label", "endsession"])
    started = wait_for(d / "typed.txt", p)
    res = {"label": "endsession", "started": started}
    if started:
        hwnd = int((d / "hwnd.txt").read_text())
        u = ctypes.windll.user32
        u.SendMessageTimeoutW.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_size_t, ctypes.c_ssize_t, ctypes.c_uint, ctypes.c_uint, ctypes.POINTER(ctypes.c_size_t)]
        out = ctypes.c_size_t(0)
        ENDSESSION_LOGOFF = 0x80000000
        u.SendMessageTimeoutW(hwnd, 0x0011, 0, ENDSESSION_LOGOFF, 0x0002, 5000, ctypes.byref(out))  # WM_QUERYENDSESSION
        res["query_answer"] = out.value
        u.SendMessageTimeoutW(hwnd, 0x0016, 1, ENDSESSION_LOGOFF, 0x0002, 5000, ctypes.byref(out))  # WM_ENDSESSION
    p.kill()
    p.wait()
    try:
        c = sqlite3.connect(str(d / "gaeunmemo.db"))
        res["quick_check"] = c.execute("PRAGMA quick_check").fetchone()[0]
        row = c.execute("SELECT body_text FROM note WHERE title='강제 종료'").fetchone()
        text = row[0] if row else ""
        res["kept"] = text
        res["full"] = text == TEXT
        res["prefix"] = TEXT.startswith(text)
        c.close()
    except Exception as e:  # noqa: BLE001
        res["error"] = repr(e)
    shutil.rmtree(d, ignore_errors=True)
    return res


def main():
    runs = int(sys.argv[1]) if len(sys.argv) > 1 else 50
    only = sys.argv[2] if len(sys.argv) > 2 else ""
    results = []
    if not only:
        for delay in (0, 100, 350):
            for _ in range(runs):
                results.append(run_once(delay))
        # pictures: killed 0, 5, 10 … 100 ms after the paste, so some runs die before the row is
        # committed and some after — no run may leave a reference without its file
        for k in range(max(21, runs // 2)):
            results.append(run_once((k * 5) % 105, "picture"))
    if not only or only == "commit":
        for _ in range(max(10, runs // 2)):
            results.append(run_commit())
    if not only or only == "endsession":
        for _ in range(max(10, runs // 2)):
            results.append(run_endsession())
    summary = {}
    groups = {
        "0": lambda r: r["label"] == "" and r["delay_ms"] == 0,
        "100": lambda r: r["label"] == "" and r["delay_ms"] == 100,
        "350": lambda r: r["label"] == "" and r["delay_ms"] == 350,
        "picture": lambda r: r["label"] == "picture",
        "commit": lambda r: r["label"] == "commit",
        "endsession": lambda r: r["label"] == "endsession",
    }
    for key, pick in groups.items():
        rs = [r for r in results if pick(r)]
        if not rs:
            continue
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
        if key == "endsession":
            summary[key]["query_answered_true"] = sum(1 for r in rs if r.get("query_answer") == 1)
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / "p3_kill.json").write_text(json.dumps(summary, ensure_ascii=False, indent=1), encoding="utf-8")
    print(json.dumps(summary, ensure_ascii=False))


if __name__ == "__main__":
    main()
