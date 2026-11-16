"""Visual regression gallery: v1.1 reference | v2 | diff for every case, with the
numbers from diff.jsonl and the allowed-difference list (report §14, P1 exit criterion).

    python -I bench/visual/gallery.py [out.html]

Self-contained (images inlined), so the page can be opened or sent as one file.
"""
import base64
import html
import json
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
RES = HERE.parent / "results" / "visual"
OUT = Path(sys.argv[1]) if len(sys.argv) > 1 else RES / "gallery.html"

# Allowed differences (A-n) proposed for approval; each case lists the ones it shows.
ALLOWED = [
    ("A1", "글자 래스터화", "v2는 GDI ClearType(서브픽셀, 컬러 테두리), v1.1은 Chromium 그레이스케일 안티앨리어싱. 같은 맑은 고딕·같은 크기라도 글자 가장자리 픽셀과 굵기가 다릅니다."),
    ("A2", "글자 세로 위치 ±1–2 px", "Chromium의 줄 상자(line-height 1.55, 소수 배치)와 GDI DrawText 세로 가운데 정렬의 반올림 차이. 메뉴 항목·목록 행에서 보입니다."),
    ("A3", "아이콘·체크 ±1 px", "SVG 획(Chromium)과 4×4 슈퍼샘플링 비트맵(v2)의 반픽셀 위치 차이."),
    ("A4", "기준 화면의 포인터 부재", "헤드리스 Edge에는 마우스가 없어 :hover 상태가 CSS로 강제됩니다. 확인 카드 아래 도구 버튼(포인터가 창 안에 있을 때 v1.1은 흐리게 보임)처럼 강제하지 않은 상태는 v2와 다를 수 있습니다."),
    ("A5", "테스트 창의 포커스 부재", "v2 캡처는 창을 활성화하지 않으므로(사용자 화면 보호) 제목 수정 시 전체 선택 표시와 캐럿이 캡처에 나오지 않습니다. 실제 사용에서는 선택·캐럿이 보입니다."),
    ("A7", "선택 영역 색", 'v1.1은 선택을 메모 색 65 %로 칠하고(::selection), v2의 본문(RichEdit)·제목(EDIT)은 Windows 강조색(시스템 하이라이트)을 씁니다. 창형 RichEdit/EDIT에는 선택 색 설정이 없어, 맞추려면 windowless RichEdit(ITextHost)로 바꿔야 합니다. 비교: <a href="#note_dark_select">note_dark_select</a>, <a href="#note_light_select">note_light_select</a>.'),
    ("A8", "캐럿 색", "v1.1 캐럿은 글자색(ink), v2는 시스템 캐럿(배경 반전)으로 두 테마 모두에서 보이지만 색이 정확히 같지는 않습니다."),
    ("A6", "스크롤바 모양", "v1.1 WebView2 오버레이 스크롤바 대 v2 시스템(다크 테마) 스크롤바. 이 매트릭스의 기본 크기에서는 스크롤바가 나타나지 않습니다."),
]

NOTES = {
    "note_dark_confirm": ["A1", "A4"],
    "note_light_confirm": ["A1", "A4"],
    "note_dark_rename": ["A1", "A5", "A7"],
    "note_dark_select": ["A7"],
    "note_light_select": ["A7"],
    "note_dark_menu": ["A1", "A2", "A3"],
    "note_light_menu_tall": ["A1", "A2", "A3"],
    "note_dark_menu_locked_tall": ["A1", "A2", "A3"],
    "list_light_ctx": ["A1", "A2"],
}


def uri(p: Path) -> str:
    if not p.exists():
        return ""
    return "data:image/png;base64," + base64.b64encode(p.read_bytes()).decode()


def main():
    rows = {}
    for line in (RES / "v2" / "diff.jsonl").read_text(encoding="utf-8").splitlines():
        if line.strip():
            r = json.loads(line)
            rows[r["case"]] = r
    order = [l.split("\t")[0] for l in (HERE / "cases.tsv").read_text(encoding="utf-8").splitlines()
             if l.strip() and not l.startswith("#")]
    worst = max(rows.values(), key=lambda r: r["beyond_1px_pct"])
    cards = []
    for name in order:
        r = rows.get(name)
        if not r:
            continue
        notes = NOTES.get(name, ["A1"])
        badge = "ok" if r["beyond_1px_pct"] < 1.0 else "warn" if r["beyond_1px_pct"] < 3.0 else "bad"
        cards.append(f"""
<section class="case" id="{name}">
  <header><h2>{html.escape(name)}</h2>
    <span class="m {badge}">1 px 밖 {r['beyond_1px_pct']:.2f}%</span>
    <span class="m">변경 {r['changed_pct']:.2f}%</span><span class="m">평균 |Δ| {r['mean_abs']:.2f}</span>
    <span class="tags">{' '.join(f'<a href="#allowed">{n}</a>' for n in notes)}</span></header>
  <div class="trio">
    <figure><img src="{uri(RES / 'ref' / (name + '.png'))}" alt="v1.1 {name}"><figcaption>v1.1 (기준)</figcaption></figure>
    <figure><img src="{uri(RES / 'v2' / (name + '.png'))}" alt="v2 {name}"><figcaption>v2 (Win32)</figcaption></figure>
    <figure><img src="{uri(RES / 'v2' / (name + '.diff.png'))}" alt="diff {name}"><figcaption>차이</figcaption></figure>
  </div>
</section>""")
    allowed = "".join(f"<li id=\"{a}\"><b>{a}</b> {html.escape(t)} — {d if '<a ' in d else html.escape(d)}</li>" for a, t, d in sorted(ALLOWED))
    page = f"""<!doctype html><html lang="ko"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>GaeunMemo 2.0 시각 회귀</title>
<style>
:root {{ --bg:#f6f6f3; --fg:#1d1d1b; --mut:#6b6b66; --card:#fff; --rule:#deded8; --ok:#2f7d4f; --warn:#a36a00; --bad:#b3261e; }}
@media (prefers-color-scheme: dark) {{ :root {{ --bg:#161615; --fg:#ececea; --mut:#a3a39d; --card:#1f1f1e; --rule:#33332f; --ok:#6cc391; --warn:#e0a84a; --bad:#ef7a72; }} }}
* {{ box-sizing: border-box; }}
body {{ margin:0; padding:24px 16px 64px; background:var(--bg); color:var(--fg); font:14px/1.55 "Malgun Gothic", system-ui, sans-serif; }}
main {{ max-width:1320px; margin:0 auto; }}
h1 {{ font-size:22px; margin:0 0 4px; }}
.sub {{ color:var(--mut); margin:0 0 20px; }}
.summary, #allowed {{ background:var(--card); border:1px solid var(--rule); padding:14px 18px; margin-bottom:18px; }}
#allowed li {{ margin:4px 0; }}
.case {{ background:var(--card); border:1px solid var(--rule); padding:12px 14px; margin:0 0 14px; }}
.case header {{ display:flex; flex-wrap:wrap; align-items:baseline; gap:8px 12px; margin-bottom:8px; }}
.case h2 {{ font-size:15px; margin:0; font-family:Consolas, monospace; }}
.m {{ font-size:12px; color:var(--mut); font-variant-numeric:tabular-nums; }}
.m.ok {{ color:var(--ok); font-weight:700; }} .m.warn {{ color:var(--warn); font-weight:700; }} .m.bad {{ color:var(--bad); font-weight:700; }}
.tags a {{ font-size:12px; color:var(--fg); border:1px solid var(--rule); padding:0 6px; text-decoration:none; margin-right:4px; }}
.trio {{ display:flex; flex-wrap:wrap; gap:12px; }}
figure {{ margin:0; max-width:100%; }}
figure img {{ display:block; max-width:100%; height:auto; image-rendering:pixelated; border:1px solid var(--rule); }}
figcaption {{ font-size:12px; color:var(--mut); margin-top:4px; }}
</style></head><body><main>
<h1>GaeunMemo 2.0 — P1 시각 회귀 (v1.1 대비)</h1>
<p class="sub">31개 케이스 · 150 % 배율 · 기준: v1.1 ui/*.html·css·js를 헤드리스 Edge로 렌더 · v2: 같은 상태를 화면 밖 창에서 WM_PRINT로 캡처.
차이 이미지: 회색 = 같음, 색 = 다른 픽셀. "1 px 밖" = 주변 1 px 안에서도 맞는 픽셀을 찾지 못한 비율.</p>
<div class="summary">가장 큰 "1 px 밖" 차이: <b>{worst['case']}</b> {worst['beyond_1px_pct']:.2f}% ·
모든 케이스 크기 일치: {'예' if all(r['same_size'] for r in rows.values()) else '아니오'} ·
케이스 수 {len(rows)}</div>
<section id="allowed"><h2 style="font-size:16px;margin:0 0 6px">허용 차이 목록 — 2026-11-14 사용자 승인</h2><ul>{allowed}</ul></section>
{''.join(cards)}
</main></body></html>"""
    OUT.write_text(page, encoding="utf-8")
    print(OUT, round(OUT.stat().st_size / 1024), "KB")


if __name__ == "__main__":
    main()
