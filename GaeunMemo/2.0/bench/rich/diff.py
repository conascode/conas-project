"""Differential test of src/rich.rs against v1.1's ui/rich.js in Chromium (report §6.4, §14).

    python -I bench/rich/diff.py [N=600] [SEED=1]

For every HTML case (hand-picked v1.1 / Word / Google Docs / web shapes + N random ones):
  san    Rust sanitize+serialize            == Rich.serialize(Rich.fromHtml(html))
  plain  Rust plain (innerText emulation)   == Rich.plain(rendered)
  vis    Rust Doc lines (markers, b/i/u/s)  == on-screen lines measured in Chromium
  rt     v2's saved HTML read back by v1.1  == the same lines, Rich.plain == Rust text copy,
         and Rich.fromHtml leaves it unchanged (roll-back to v1.1 keeps every note intact)
Writes bench/results/rich_diff.json (summary) and rich_mismatch.jsonl (first mismatches).
"""
import json
import random
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
# automated runs use their own build folder (cargo build --target-dir target/auto):
# the user may be running the exe in target/release
EXE = ROOT / "target" / "auto" / "release" / "gaeunmemo2.exe"
OUT = ROOT / "bench" / "results"
sys.path.insert(0, str(HERE))
from cdp import Edge  # noqa: E402

NAME = "0123abcd-0123-4567-89ab-0123456789ab.png"
NAME2 = "fedcba98-7654-3210-fedc-ba9876543210.jpg"

FIXED = [
    # what v1.1's editor (Chromium contenteditable) writes
    "첫 줄<div>둘째 줄</div><div><br></div><div>넷째</div>",
    "<b>굵게</b> 보통 <i>기울임</i> <u>밑줄</u> <s>취소</s>",
    "<ul><li>하나</li><li>둘</li></ul><div>끝</div>",
    "장보기<ul><li>우유 2개</li><li><b>계란</b> 한 판</li></ul>",
    "a&nbsp;&nbsp;b &nbsp;c&nbsp;",
    "<div><b>굵은 줄</b><br></div><div><br></div>",
    f"사진 앞<img data-gm=\"{NAME}\">사진 뒤",
    f"<div><img data-gm=\"{NAME}\"></div><div><img data-gm=\"{NAME2}\"><br></div>",
    "<b><i>둘 다</i></b><i><b>순서 반대</b></i>",
    "줄1<br>줄2<br><br>줄4",
    "<ul><li>항목<br>둘째 줄</li></ul>",
    "<ul><li>바깥<ul><li>안쪽</li></ul>계속</li></ul>",
    "",
    "<br>",
    "<div></div>",
    "끝에 줄바꿈<br>",
    "끝에 두 번<br><br>",
    # Word / Google Docs / web paste shapes
    "<p class=MsoNormal><span style='font-weight:bold'>워드 굵게</span><o:p></o:p></p><p class=MsoNormal>둘째 문단</p>",
    "<meta charset='utf-8'><b style=\"font-weight:normal;\" id=\"docs-internal-guid-1\"><span style=\"font-weight:700;font-style:italic\">구글 문서</span></b>",
    "<h1>제목</h1><p>본문 <a href=\"https://x\">링크</a></p><script>evil()</script><style>p{}</style>",
    "<table><tr><td>칸1</td><td>칸2</td></tr><tr><td>칸3</td></tr></table>",
    "<blockquote>인용<p>안의 문단</p></blockquote>",
    "<pre>  들여쓰기\n그대로</pre>",
    "<p>가</p>\n<p>나</p>",
    "<span style=\"text-decoration: underline line-through\">둘 다</span>",
    "<span style=\"font-weight:600\">600</span><span style=\"font-weight: 500\">500</span>",
    "<ol><li>번호</li><li>목록</li></ol>",
    "<div>a<div>b</div>c</div>",
    "x<p>단락</p>y",
    "<img src=\"https://example.com/a.png\" onerror=\"x()\"><img data-gm=\"../evil.exe\">",
    "😀 이모지 &#x1F600; &amp; &lt;tag&gt;",
    "<!-- 주석 -->보이는<!-- 숨김 -->글",
    "<B>대문자</B><EM>태그</EM>",
    "줄\r\n바꿈\r끝",
    "<li>목록 밖 항목</li>",
    "<ul>목록 안 글<li>항목</li></ul>",
]

WORDS = ["가나", "다라마", "장보기", "우유", "회의", "hello", "World", "123", "😀", "&amp;", "&lt;", "&nbsp;", "&#44032;", "a b", "  ", " ", "\n", "x"]
INLINE = ["b", "strong", "i", "em", "u", "ins", "s", "strike", "del", "span", "a", "font", "code", "sup", "mark"]
BLOCK = ["div", "p", "h1", "h2", "blockquote", "pre", "section"]
STYLES = ["font-weight:700", "font-weight:bold", "font-weight: normal", "font-weight:600", "font-style:italic",
          "font-style: normal", "text-decoration:underline", "text-decoration: line-through", "color:red",
          "text-decoration: underline line-through", "font-weight:400; font-style:italic"]


def rand_html(rng, depth=0):
    parts = []
    for _ in range(rng.randint(1, 5 if depth < 2 else 3)):
        r = rng.random()
        if r < 0.38 or depth >= 4:
            parts.append(rng.choice(WORDS))
        elif r < 0.62:
            tag = rng.choice(INLINE)
            attr = f' style="{rng.choice(STYLES)}"' if rng.random() < 0.4 else (' href="https://x"' if tag == "a" else "")
            parts.append(f"<{tag}{attr}>{rand_html(rng, depth + 1)}</{tag}>")
        elif r < 0.77:
            tag = rng.choice(BLOCK)
            attr = f' style="{rng.choice(STYLES)}"' if rng.random() < 0.25 else ""
            parts.append(f"<{tag}{attr}>{rand_html(rng, depth + 1)}</{tag}>")
        elif r < 0.85:
            lt = rng.choice(["ul", "ol"])
            items = "".join(f"<li>{rand_html(rng, depth + 1)}</li>" for _ in range(rng.randint(1, 3)))
            parts.append(f"<{lt}>{items}</{lt}>")
        elif r < 0.93:
            parts.append("<br>")
        elif r < 0.96:
            parts.append(rng.choice([f'<img data-gm="{NAME}">', f'<img data-gm="{NAME2}">', '<img src="https://e/x.png">']))
        else:
            parts.append(rng.choice(["<script>x()</script>", "<!-- c -->", "<button>버튼</button>", "<div></div>"]))
    return "".join(parts)


def lines(vis):
    """Normal form as in Doc::normal: one empty line is an empty body (v1.1 shows the
    placeholder for both "<br>" and "")."""
    return [] if vis == [[False, []]] else vis


def main():
    n = int(sys.argv[1]) if len(sys.argv) > 1 else 600
    seed = int(sys.argv[2]) if len(sys.argv) > 2 else 1
    rng = random.Random(seed)
    cases = FIXED + [rand_html(rng) for _ in range(n)]
    tmp = Path(tempfile.mkdtemp(prefix="gm-rich-"))
    with open(tmp / "cases.txt", "w", encoding="utf-8", newline="") as f:  # no CRLF translation
        f.write("\x1e".join(cases))
    subprocess.run([str(EXE), "--mode", "rich", "--in", str(tmp / "cases.txt"), "--out", str(tmp / "rust.jsonl")], check=True, timeout=120)
    rust = [json.loads(l) for l in (tmp / "rust.jsonl").read_text(encoding="utf-8").splitlines() if l.strip()]
    assert len(rust) == len(cases), (len(rust), len(cases))

    js = []
    with Edge() as e:
        e.open((HERE / "harness.html").as_uri())
        assert e.eval("window.GM_ready === true && !!window.Rich"), "harness / rich.js did not load"
        for k in range(0, len(cases), 60):
            batch = [{"html": c, "saved": r["saved"]} for c, r in zip(cases[k:k + 60], rust[k:k + 60])]
            js += e.eval(f"GM_check({json.dumps(batch, ensure_ascii=False)})")

    checks = {"san": 0, "plain": 0, "vis": 0, "rt_vis": 0, "rt_plain": 0, "rt_stable": 0}
    bad = []
    for i, (c, r, j) in enumerate(zip(cases, rust, js)):
        res = {
            "san": r["san"] == j["san"],
            "plain": r["plain"] == j["plain"],
            "vis": r["vis"] == lines(j["vis"]),
            "rt_vis": r["vis"] == lines(j["savedVis"]),
            "rt_plain": r["saved_plain"] == j["savedPlain"],
            "rt_stable": j["savedSan"] == r["saved"],
        }
        for k, ok in res.items():
            checks[k] += ok
        if not all(res.values()):
            bad.append({"i": i, "fixed": i < len(FIXED), "html": c, "fail": [k for k, ok in res.items() if not ok],
                        "rust": r, "js": j})
    summary = {"cases": len(cases), "fixed": len(FIXED), "random": n, "seed": seed, "pass": checks,
               "all_pass": len(cases) - len(bad)}
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / "rich_diff.json").write_text(json.dumps(summary, ensure_ascii=False, indent=1), encoding="utf-8")
    with open(OUT / "rich_mismatch.jsonl", "w", encoding="utf-8") as f:
        for b in bad[:200]:
            f.write(json.dumps(b, ensure_ascii=False) + "\n")
    print(json.dumps(summary, ensure_ascii=False))


if __name__ == "__main__":
    main()
