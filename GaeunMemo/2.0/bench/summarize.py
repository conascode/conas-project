"""Summarises bench/results/*.jsonl into median tables (markdown) for the report.

    python -I bench/summarize.py  >  bench/results/summary.md
"""
import json
import statistics as st
import sys
from collections import defaultdict
from pathlib import Path

RES = Path(__file__).resolve().parent / "results"


def load(name):
    p = RES / name
    if not p.exists():
        return []
    return [json.loads(l) for l in p.read_text(encoding="utf-8").splitlines() if l.strip()]


def med(xs):
    xs = [x for x in xs if x is not None]
    return st.median(xs) if xs else float("nan")


def rng(xs):
    xs = [x for x in xs if x is not None]
    return (min(xs), max(xs)) if xs else (float("nan"), float("nan"))


def f(x, d=2):
    return "—" if x != x else f"{x:.{d}f}"


def main():
    runs = load("matrix.jsonl")
    groups = defaultdict(list)
    for r in runs:
        groups[(r["label"], int(r["notes"]))].append(r)
    order = ["tray", "PT-A", "PT-B", "PT-B'", "PT-C"]
    keys = sorted(groups, key=lambda k: (order.index(k[0]) if k[0] in order else 9, k[1]))

    print("## Run matrix (median of reps; private MB = PrivateUsage)\n")
    print("| config | notes | reps | private MB settled | (min–max) | private MB after typing | WS MB | GDI | USER | handles | startup ms | idle CPU % | typing CPU % | paint avg µs | paint max µs | first paints |")
    print("|---|---:|---:|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|")
    for k in keys:
        g = groups[k]
        lo, hi = rng([r["settled_private_mb"] for r in g])
        print(
            f"| {k[0]} | {k[1]} | {len(g)} | {f(med([r['settled_private_mb'] for r in g]))} | {f(lo)}–{f(hi)} "
            f"| {f(med([r['typed_private_mb'] for r in g]))} | {f(med([r['settled_ws_mb'] for r in g]),1)} "
            f"| {f(med([r['settled_gdi'] for r in g]),0)} | {f(med([r['settled_user'] for r in g]),0)} | {f(med([r['settled_handles'] for r in g]),0)} "
            f"| {f(med([r['startup_ms'] for r in g]),0)} | {f(med([r['idle_cpu_pct'] for r in g]),3)} "
            f"| {f(med([r['typing_cpu_pct'] for r in g]),2)} | {f(med([r['paint_avg_us'] for r in g]),0)} "
            f"| {f(med([r['paint_max_us'] for r in g]),0)} | {f(med([r['first_paints'] for r in g]),0)} |"
        )

    # per-note increments (10→50) and one-time cost (1-note minus tray)
    print("\n## Derived (from medians)\n")
    print("| config | one-time MB (1 note − tray) | per note MB ((50−10)/40) |")
    print("|---|---:|---:|")
    tray = med([r["settled_private_mb"] for r in groups.get(("tray", 0), [])])
    for lab in order[1:]:
        m1 = med([r["settled_private_mb"] for r in groups.get((lab, 1), [])])
        m10 = med([r["settled_private_mb"] for r in groups.get((lab, 10), [])])
        m50 = med([r["settled_private_mb"] for r in groups.get((lab, 50), [])])
        print(f"| {lab} | {f(m1 - tray)} | {f((m50 - m10) / 40, 3)} |")

    leaks = load("leak.jsonl")
    if leaks:
        print("\n## Leak (create/destroy batches)\n")
        print("| config | windows | seconds | Δ private MB | Δ GDI | Δ USER | Δ handles | private trace MB |")
        print("|---|---:|---:|---:|---:|---:|---:|---|")
        for r in leaks:
            print(f"| {r['label']} | {int(r['windows_created'])} | {f(r['seconds'],1)} | {f(r['d_private_mb'])} | {int(r['d_gdi'])} | {int(r['d_user'])} | {int(r['d_handles'])} | {r['private_mb_trace']} |")

    ptd = load("ptd.jsonl")
    if ptd:
        print("\n## PT-D (InsertImage, original file stream), rep 1\n")
        print("| file | display px | Δ private insert MB | Δ private render MB | display-decode est. MB | full-decode MB | insert ms | render ms |")
        print("|---|---:|---:|---:|---:|---:|---:|---:|")
        for s in ptd[0]["steps"]:
            print(f"| {s['file']} | {int(s['display_w'])} | {f(s['d_private_insert_mb'])} | {f(s['d_private_render_mb'])} | {f(s['display_decode_mb'])} | {f(s['full_decode_mb'],1)} | {f(s['insert_ms'],1)} | {f(s['render_ms'],1)} |")
        print("\n### PT-D′ (display-size re-encode), rep 1\n")
        print("| ext | display px | encoded px | encoded KB | re-encode ms | Δ insert MB | Δ render MB | name ok |")
        print("|---|---:|---:|---:|---:|---:|---:|---|")
        for s in ptd[0].get("dprime", []):
            print(f"| {s['ext']} | {int(s['display_w'])} | {int(s['encoded_px_w'])} | {f(s['encoded_kb'],1)} | {f(s['reencode_ms'],1)} | {f(s['d_private_insert_mb'])} | {f(s['d_private_render_mb'])} | {s['name_ok']} |")
        print("\n### Criteria per rep\n")
        print("| rep | names read back | undo keeps names | copy keeps name | refit keeps name | refit ms | cleared MB | 10-cycle private MB |")
        print("|---|---|---|---|---|---:|---:|---|")
        for r in ptd:
            print(f"| {r['label']} | {r['names_match']} | {r['undo']['names_equal_after_undo']} | {r['copy']['copy_keeps_name']} | {r['refit']['name_kept']} | {f(r['refit']['ms'],2)} | {f(r['cleared_private_mb'])} | {r.get('cycle_private_mb')} |")

    gif = load("gif.jsonl")
    if gif:
        print("\n## Animated GIF in RichEdit\n")
        for r in gif:
            print(f"- {r['label']}: animates={r['animates']} colour changes={int(r['colour_changes'])} idle CPU with GIF={f(r['idle_cpu_pct_with_gif'],3)} % samples={r['red_blue_samples']}")


if __name__ == "__main__":
    sys.stdout.reconfigure(encoding="utf-8")
    main()
