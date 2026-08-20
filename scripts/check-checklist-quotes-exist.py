#!/usr/bin/env python3
"""
驗收清單叫他去對照的那幾句話，產品裡真的說得出口嗎。

`docs/WINDOWS-CHECKLIST.md` 是這個專案唯一一份**只有他那台機器執行得了**的
測試，而他是一條一條照著做的。所以一句「那一格要說 X」如果 X 已經不在程式裡
了，後果不是排版難看：他照著找、找不到，然後回報一個沒有壞的東西壞了。那是
第 45 條那個假紅，換到文件上——而假紅比假綠更貴，因為它會吃掉他整輪驗證的
信任。

2026-08-20 一天之內抓到五個：

  1. `%APPDATA%\\sister` ×5（真的位置是 `%APPDATA%\\ted-h\\AI-Sister\\data`）
  2. `check-combo-is-readable.sh` 改名成 `.py` 之後清單還指著 `.sh`
     —— 這兩族由 `check-docs-point-somewhere.py` 守著（那支管路徑）
  3. 「找不到資料目錄，停不了」——程式裡有這個字串，但那條路真機器上走不到
  4. 「她說不動——你還沒簽第一張同意書」——**程式從來沒說過這句話**，
     真正說的是「第一張同意書還沒簽——她不會開始記錄。……」
  5. 「這段時間裡沒有東西」／「還沒有東西，去按開始記錄」——兩句都是我編的，
     真的是 `timeline.js` 的「現在一天都沒有了。」和「一天都沒有。」

這支腳本守的是 4 和 5：**清單裡宣告產品會說的話，要在原始碼裡找得到**。

**這支腳本守不住的那一半，寫在這裡。**

  - 守不住第 3 種（字串在、但那條路走不到）。要驗那個得能判斷可達性，這裡
    做不到。所以「你應該看到 X」還是要自己問一次：X 那條 `if` 在他那台機器
    上進得去嗎？
  - 只認**逐字**出現，而且只在「這一行有宣告動詞」的時候才看。散文裡的引號
    多半是轉述（實測 167 個候選裡 41% 對不上），全掃的話這支腳本會變成一台
    誤報機器，然後被關掉——關掉之後它守的那條線是一格空白。
  - 原始碼裡有沒有那句話 ≠ 那句話出得來。註解裡寫著也算過。這是刻意放寬的：
    要抓的是「改了文案忘了改清單」，不是可達性。
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DOC = Path("docs/WINDOWS-CHECKLIST.md")

# 產品「說得出口的話」住在哪裡。JS 那幾支是字母人自己的文案，Rust 那邊是
# 後端回給它的錯誤訊息和 CLI 印出來的東西。
SOURCES = (
    "crates/**/*.rs",
    "apps/desktop/src-tauri/src/**/*.rs",
    "apps/desktop/ui/*.js",
    "apps/desktop/ui/*.html",
)

# 「這一行在宣告產品會說什麼」。沒有這幾個字的行不看——見上面那段「守不住的
# 那一半」，散文裡的引號多半是轉述。
SAYS = re.compile(r"(要說|要寫|會寫|要出現|寫著|會說|印出)")

# 太短的引號多半是名詞（「電話」「門號」），對不上也沒意義；帶刪節號或
# 佔位符的是模板，不是逐字。
MIN = 8
TEMPLATE = re.compile(r"[…⋯『』*{}]|\bX \b|\bN ")


def die(msg, *extra):
    print(f"✗ {msg}")
    for e in extra:
        print(f"  {e}")
    sys.exit(1)


def haystack():
    """所有產品字串串成一坨，並且把跨行接起來。

    Rust 的 `"abc\\` + 換行 + 縮排 + `def"` 在檔案裡是分兩行的，但它是**一個**
    字串。不接起來的話，清單裡逐字抄對的長句子反而會對不上——那就是一支對著
    正確的文件喊紅的閘門，而那種閘門會被關掉（第 46(a) 條）。JS 的 `\\` 續行
    同理。
    """
    out = []
    for pat in SOURCES:
        for f in sorted(ROOT.glob(pat)):
            out.append(f.read_text(encoding="utf-8", errors="ignore"))
    if not out:
        die("一個原始碼檔都沒掃到", "glob 對不到東西的時候，底下每一圈都是空轉。")
    text = "\n".join(out)
    # 續行：反斜線 + 換行 + 接下來那一行的縮排，整段拿掉。
    return re.sub(r"\\\n\s*", "", text)


def items(lines):
    """只走 `- [ ]` 那些條目（含它們的縮排續行），不走前言散文。

    第 9 行那句「八張都寫著『去 Windows 上測』的紙條」是散文裡的比喻，掃它
    只會誤報。條目的形狀在這份檔案裡很穩定：`- [ ]` 開頭，續行縮排 6 格。
    """
    inside = False
    for i, ln in enumerate(lines, 1):
        if re.match(r"\s*- \[[ x]\]", ln):
            inside = True
        elif inside and not ln.startswith("      ") and ln.strip() != "":
            inside = False
        if inside:
            yield i, ln


def main():
    path = ROOT / DOC
    if not path.exists():
        die(f"{DOC} 不見了", "這支腳本整個沒有意義了——要嘛改路徑，要嘛把它刪掉。")
    lines = path.read_text(encoding="utf-8").split("\n")
    src = haystack()

    checked, bad = 0, []
    for i, ln in items(lines):
        if not SAYS.search(ln):
            continue
        for q in re.findall(r"「([^「」\n]+)」", ln):
            if len(q) < MIN or TEMPLATE.search(q):
                continue
            checked += 1
            if q not in src:
                bad.append((i, q))

    # 活體檢查。哪天條目的排版換了、或那幾個宣告動詞被改掉，這支腳本會安靜地
    # 一句都掃不到然後回報綠——那正是它要抓的那種壞法，發生在它自己身上。
    if checked < 10:
        die(
            f"只挑出 {checked} 句話來對，太少了",
            "這份清單有幾百行，正常會挑出二十句上下。",
            "多半是條目的排版變了（`- [ ]` / 六格縮排），或 SAYS 那幾個動詞對不上了。",
        )

    if bad:
        print(f"✗ 清單叫他去對照 {len(bad)} 句產品說不出口的話：")
        for i, q in bad:
            print(f"  {DOC}:{i}")
            print(f"      「{q}」")
        print()
        print("  他會照著找、找不到，然後回報一個沒有壞的東西壞了。")
        print("  把清單改成程式裡真正那句話（逐字抄），或者把那句話加回程式裡。")
        sys.exit(1)

    print(f"✓ 清單裡那 {checked} 句「產品會說 X」，X 在原始碼裡都找得到")


main()
