"""P6 measurements of the whole app (report §13.1), medians of interleaved runs.

    python -I bench/p6/run.py LABEL [RUNS=3] [IDLE_S=120]

Each run is its own process in a new test folder (seeded with N open notes): tray-only (0),
1, 10, 50 notes, then the ten-picture run. The order is interleaved (0,1,10,50,pic, 0,1,…)
so that this PC's two speed states fall on every case alike. Nothing else should run
meanwhile (no builds). Writes bench/results/p6_<LABEL>.jsonl (every run) and
bench/results/p6_<LABEL>_summary.json (medians, min, max).
"""
import json
import shutil
import statistics
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
EXE = ROOT / "target" / "auto" / "release" / "gaeunmemo2.exe"
OUT = ROOT / "bench" / "results"
PHOTO = ROOT / "bench" / "img" / "photo_4000x3000.jpg"


def run(args):
    subprocess.run([str(EXE)] + args, check=False, timeout=900)


def main():
    label = sys.argv[1] if len(sys.argv) > 1 else "p6"
    runs = int(sys.argv[2]) if len(sys.argv) > 2 else 3
    idle = sys.argv[3] if len(sys.argv) > 3 else "120"
    out = OUT / f"p6_{label}.jsonl"
    if out.exists():
        out.unlink()
    for r in range(runs):
        for n in (0, 1, 10, 50):
            d = Path(tempfile.mkdtemp(prefix="gm-p6-"))
            shutil.rmtree(d)
            run(["--mode", "p6seed", "--data", str(d), "--notes", str(n)])
            run(["--mode", "p6bench", "--data", str(d), "--notes", str(n), "--idle", idle, "--type", "10" if n else "0", "--label", f"{label}-{n}", "--out", str(out)])
            shutil.rmtree(d, ignore_errors=True)
        d = Path(tempfile.mkdtemp(prefix="gm-p6pic-"))
        shutil.rmtree(d)
        run(["--mode", "p6seed", "--data", str(d), "--notes", "1"])
        run(["--mode", "p6pic", "--data", str(d), "--images", str(PHOTO), "--label", f"{label}-pic", "--out", str(out)])
        shutil.rmtree(d, ignore_errors=True)
    rows = [json.loads(l) for l in out.read_text(encoding="utf-8").splitlines() if l.strip()]
    summary = {}
    for key in sorted({r["label"] for r in rows}):
        rs = [r for r in rows if r["label"] == key]
        fields = {}
        for f in rs[0]:
            vals = [r[f] for r in rs if isinstance(r.get(f), (int, float)) and r.get(f) is not None]
            if vals and f not in ("notes", "pictures", "idle_s"):
                fields[f] = {"median": round(statistics.median(vals), 4), "min": round(min(vals), 4), "max": round(max(vals), 4)}
        summary[key] = {"runs": len(rs), **fields}
    (OUT / f"p6_{label}_summary.json").write_text(json.dumps(summary, ensure_ascii=False, indent=1), encoding="utf-8")
    keys = ["startup_ms", "settled_private_mb", "settled_gdi", "settled_user", "idle_cpu_pct", "typing_cpu_pct", "typing_cpu_cyc_pct", "paint_avg_us", "paint_max_us",
            "first_shown_ms", "ui_stall_max_ms", "before_private_mb", "shown_private_mb", "full_private_mb", "budget_note_mb", "budget_full_mb"]
    for k, v in summary.items():
        print(k, {f: v[f]["median"] for f in keys if f in v})


if __name__ == "__main__":
    main()
