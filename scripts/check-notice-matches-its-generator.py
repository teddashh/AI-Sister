#!/usr/bin/env python3
"""三份語音 NOTICE 必須逐字等於產生它的那支 promote 腳本印得出來的東西。

出貨的 NOTICE 是一份權利與隱私聲明：它說這些檔案怎麼來的、做過什麼處理、哪些
東西**沒有**進 repo。讀它的人沒有辦法自己驗證那些句子，只能相信它和產地是同一
份文字。

那條繫繩本來沒有人守。app icon 與 reel 那兩包各自 pin 了 NOTICE 的 sha256
（check-app-icons.py:15、check-persona-reel-assets.py:40），語音這三包卻只檢查
「manifest 的 notice 欄位寫著 NOTICE.md」——檔案裡寫什麼都可以。2026-09-12 我
手改過這三份 NOTICE 的一句話（把一句照字面讀是假的全稱句換掉），那次是連同
腳本一起改的，但沒有任何東西擋得住只改一邊。

這支不 pin 常數，改問等式：把 promote 腳本裡那段 NOTICE 常數求值，和出貨的
.md 逐字比。兩邊一起改是正當的（那就是改文案），只改一邊就會紅。

順帶守第二件事：**repo 裡不准有沒人守的 NOTICE**。數過一次（2026-09-12），
git 追蹤的出貨 NOTICE 有六份——這三份走上面的等式，另外三份走 sha256 pin
（icons → check-app-icons.py、reels → check-persona-reel-assets.py、personas →
check-persona.mjs）。website 那份不在 repo 裡，是 build-website.py 從 personas
那份 copy2 出去的，跟著 personas 的 pin 走。這份清單寫成註解只會逼人看一眼，
所以下面直接算：新加一份 NOTICE 而沒有人守，這條就紅。

**它擋不住什麼**（寫在這裡，因為不寫下來我下次會以為它守得比實際多）：
  - NOTICE 裡的句子是不是真的。它只保證「出貨的字＝產地的字」，不保證那些字
    對應到做過的事——那要靠各自的量測（loudness/edges/QC 收據）。
  - 腳本和 .md 一起被改成假話。
  - 「有人守」只問得出「有沒有一支 checker 拿它的 sha256 當常數」，問不出那支
    checker 有沒有真的比對**這個路徑**。實際做過一刀：把 reels 那份 NOTICE
    原封不動複製到一個新資料夾，這條照樣印綠（7 份都有人守），而沒有任何
    checker 認得那個新路徑。
"""
import ast
import hashlib
import json
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
PAIRS = (
    ("scripts/promote-persona-voice-assets.py", "apps/desktop/ui/persona-voices/v1"),
    ("scripts/promote-persona-consent-voice-assets.py", "apps/desktop/ui/persona-consent-voices/v1"),
    ("scripts/promote-persona-banter-voice-assets.py", "apps/desktop/ui/persona-banter-voices/v1"),
)
HEADER = "# Bundled persona"


def literal(node, names):
    """只吃字串常數與 f-string，f-string 裡只准放 names 裡的名字。

    不用 eval：那會讓 repo 裡的任何一句運算式在閘門裡跑起來。這支閘門看的是
    一段固定文案，它本來就不該需要執行任何東西。
    """
    if isinstance(node, ast.Constant) and isinstance(node.value, str):
        return node.value
    if isinstance(node, ast.JoinedStr):
        out = []
        for part in node.values:
            if isinstance(part, ast.Constant) and isinstance(part.value, str):
                out.append(part.value)
            elif (
                isinstance(part, ast.FormattedValue)
                and isinstance(part.value, ast.Name)
                and part.value.id in names
                and part.format_spec is None
                and part.conversion == -1
            ):
                out.append(str(names[part.value.id]))
            else:
                raise ValueError(f"NOTICE 文案裡有這支閘門不認得的內插：{ast.dump(part)[:80]}")
        return "".join(out)
    raise ValueError(f"NOTICE 文案不是單純的字串：{type(node).__name__}")


def notice_source(script):
    tree = ast.parse((ROOT / script).read_text(encoding="utf-8"))
    found = [
        call.args[0]
        for call in ast.walk(tree)
        if isinstance(call, ast.Call)
        and isinstance(call.func, ast.Attribute)
        and call.func.attr == "write_text"
        and call.args
        and HEADER in ast.unparse(call.args[0])
    ]
    if len(found) != 1:
        raise ValueError(f"{script}：找到 {len(found)} 段 NOTICE 文案，期望剛好 1 段")
    return found[0]


def unguarded_notices():
    """git 追蹤的每一份 NOTICE.md，要嘛在 PAIRS 裡，要嘛有人拿它的 sha256 當常數。"""
    listed = subprocess.run(
        ["git", "ls-files", "-z", "*NOTICE.md"],
        cwd=ROOT, check=True, capture_output=True, text=True,
    ).stdout.split("\0")
    mine = {f"{base}/NOTICE.md" for _, base in PAIRS}
    checkers = "\n".join(
        p.read_text(encoding="utf-8", errors="replace")
        for p in sorted((ROOT / "scripts").iterdir())
        if p.suffix in (".py", ".mjs", ".js") and p.name != pathlib.Path(__file__).name
    )
    out = []
    for rel in listed:
        if not rel or rel in mine:
            continue
        digest = hashlib.sha256((ROOT / rel).read_bytes()).hexdigest()
        if digest not in checkers:
            out.append(rel)
    return len(listed) - 1, out


def main():
    problems = []
    for script, base in PAIRS:
        pack = ROOT / base
        try:
            manifest = json.loads((pack / "manifest.json").read_text(encoding="utf-8"))
            want = literal(notice_source(script), {"total": len(manifest["clips"])})
        except (ValueError, KeyError, OSError) as exc:
            problems.append(f"{script}：讀不出產地文案——{exc}")
            continue
        got = (pack / "NOTICE.md").read_text(encoding="utf-8")
        if want == got:
            print(f"  ✓ {base}/NOTICE.md（{len(got.encode())} bytes）逐字等於 {script} 印得出來的")
            continue
        problems.append(f"{base}/NOTICE.md 和 {script} 不一致")
        for i, (a, b) in enumerate(zip(want.split("\n"), got.split("\n"))):
            if a != b:
                cut = next((n for n, (x, y) in enumerate(zip(a, b)) if x != y), min(len(a), len(b)))
                problems.append(f"    第 {i + 1} 行第 {cut + 1} 個字元起：")
                problems.append(f"      產地：…{a[max(0, cut - 40):cut + 60]}…")
                problems.append(f"      出貨：…{b[max(0, cut - 40):cut + 60]}…")
                break
        else:
            problems.append(f"    行數不同：產地 {want.count(chr(10))} 行、出貨 {got.count(chr(10))} 行")

    try:
        total, orphans = unguarded_notices()
    except (subprocess.CalledProcessError, OSError) as exc:
        problems.append(f"數不出 repo 裡有幾份 NOTICE：{exc}")
    else:
        if orphans:
            problems.append("這幾份 NOTICE 沒有人守——要嘛接上這支閘門，要嘛在某支 checker 裡 pin 它的 sha256：")
            problems += [f"    {o}" for o in orphans]
        else:
            print(f"  ✓ git 追蹤的 {total} 份 NOTICE 都有人守（這三份比產地，其餘 sha256 pin）")

    if problems:
        print("\n出貨的 NOTICE 和它自己的產地對不上：", file=sys.stderr)
        for p in problems:
            print(p, file=sys.stderr)
        print(
            "\n改文案要改 promote 腳本裡那段字串，再把同一份文字寫進出貨的 NOTICE.md；"
            "只改一邊就會看到這條。",
            file=sys.stderr,
        )
        return 1
    print("三份語音 NOTICE 都和它的產地一字不差。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
