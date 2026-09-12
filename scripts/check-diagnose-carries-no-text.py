#!/usr/bin/env python3
"""診斷報告那條線以上的每一格，型別上就放不下螢幕上的字。

報告自己是這樣寫的：

    這條線以上 N 個字。裡面沒有一個字是螢幕上的內容，理由不是有人記得要遮，
    是上面每一格能放的東西都在型別裡寫死了：時間、次數、結局代號、毫秒、版本。

那句話是一個關於**型別**的承諾，所以守它的也該是一個看型別的檢查。`Note`
是畫面那半唯一送得進來的東西；只要它每個欄位都是數字、布林，或是清單裡點名
過的那幾個，那句話就是真的。誰在上面加一個 `String`，這裡當場紅。

為什麼不是靠既有的兩道：

* Rust 那邊的單元測試守的是**現有欄位的行為**（`Word` 收不收得下、報告印不
  印得出來）。它們一條都不會因為多了一個新欄位而變紅。
* `check-persona.mjs` ⑨ 那條機械掃描守的是**執行期真的送出去的東西**，而它只
  看得到那一輪夾具真的產生過的種類。實測：八種 note 裡它看得到七種，`bubble`
  那一種一則都到不了它手上（那一則排在 `requestAnimationFrame` 上，而假 DOM
  沒有那個函式）。所以它對「以後有人在 `Bubble` 上加一個字串欄位」是瞎的。

這一支不看夾具，所以不會有那種盲區。三道各守一段，缺一段就有洞。

**例外的鑰匙是（變體, 欄位），不是欄位。** 第一版只用欄位名，於是我隔一天
加了一個 `AskFailed { why: String }`，它安靜地被 `GiggleSkipped` 那條例外收
下了——而那條例外寫的理由是「擋下那一聲笑的代號」，和新欄位一點關係都沒有。
一條例外只該保護它自己指名的那一格。
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

SOURCE = Path(__file__).resolve().parent.parent / "crates" / "sister-core" / "src" / "diagnose.rs"

# 放得下數字、放不下字。
PLAIN = {"Millis", "u32", "u64", "i64", "usize", "f64", "bool"}

# 例外，以及它為什麼可以是例外。
#
# 每一條都要說得出「那段字最後去了哪裡」。前三條是代號，指名之前過 `Word`
# （`[a-z0-9_-]{1,32}`），螢幕上的字進不來；後兩條是她自己的答案，報告把它們
# 整段放在那條線的**下面**，而④ 會告訴他那一段是什麼、可以整段刪掉。
ALLOWED = {
    ("Poke", "clip"): "語音檔的 lineId，`clips()` 指名之前過 `Word`",
    ("Giggle", "clip"): "語音檔的 lineId，`clips()` 指名之前過 `Word`",
    ("GiggleSkipped", "why"): "擋下那一聲笑的代號，指名之前過 `Word`",
    ("AskFailed", "why"): "一題沒答成的代號，指名之前過 `Word`",
    ("Answered", "sentences"): "她自己的答案，只出現在那條線的下面（⑤）",
    ("Answered", "sources"): "她自己的答案，只出現在那條線的下面（⑤）",
}


def note_enum(source: str) -> str:
    """`pub enum Note { … }` 的內容。數大括號，不用正規表示式數。"""
    start = source.index("pub enum Note {")
    depth = 0
    for i in range(start, len(source)):
        if source[i] == "{":
            depth += 1
        elif source[i] == "}":
            depth -= 1
            if depth == 0:
                return source[source.index("{", start) + 1 : i]
    raise SystemExit("找不到 `pub enum Note` 的結尾——這支檢查看錯檔案了")


def strip_comments(block: str) -> str:
    return "\n".join(
        line for line in block.splitlines() if not line.lstrip().startswith("//")
    )


FIELD = re.compile(r"([a-z_][a-z0-9_]*)\s*:\s*([A-Za-z0-9_]+(?:<[^>]*>)?)")
VARIANT = re.compile(r"([A-Z][A-Za-z0-9_]*)\s*\{")
UNIT_VARIANT = re.compile(r"^[A-Z][A-Za-z0-9_]*$")


def fields_of(block: str) -> list[tuple[str, str, str]]:
    """`Note` 上每一個欄位，連它住在哪個變體裡。

    這支剖析器必須把整塊吃乾淨。第一版只認縮排八格的行，於是單行寫法的
    `Started { at: Millis },` 整個被跳過——而它不吵，看起來就像那個變體沒有
    欄位。真正的問題不是漏掉 `at`（那是個數字），是**下一個人如果單行寫一個
    `Whatever { text: String }`，它一樣安靜地放過**。

    所以這裡不只是抓，還要證明抓完了，而且要證明兩層：每個變體的大括號裡沒
    有看不懂的東西，以及大括號**外面**除了變體本身沒有別的東西。剩下任何東西
    ＝有一段我沒看懂，那就不准印綠燈。
    """
    found: list[tuple[str, str, str]] = []
    spans: list[tuple[int, int]] = []
    for match in VARIANT.finditer(block):
        if any(a <= match.start() < b for a, b in spans):
            continue
        name = match.group(1)
        open_at = block.index("{", match.start())
        depth = 0
        end = None
        for i in range(open_at, len(block)):
            if block[i] == "{":
                depth += 1
            elif block[i] == "}":
                depth -= 1
                if depth == 0:
                    end = i + 1
                    break
        if end is None:
            raise SystemExit(f"✗ 變體 `{name}` 的大括號沒有結尾——剖析壞了。")
        spans.append((match.start(), end))
        body = block[open_at + 1 : end - 1]
        for field, ty in FIELD.findall(body):
            found.append((name, field, ty))
        leftover = re.sub(r"[{},\s]", "", FIELD.sub("", body))
        if leftover:
            raise SystemExit(
                f"✗ 變體 `{name}` 裡有一段這支檢查看不懂：{leftover!r}\n"
                "  看不懂就不能說「線以上放不下字」。先把剖析改對。"
            )

    outside = []
    last = 0
    for a, b in sorted(spans):
        outside.append(block[last:a])
        last = b
    outside.append(block[last:])
    for chunk in re.split(r"[,\s]+", "".join(outside)):
        # 沒有欄位的變體（`Foo,`）放得下的東西是零個，所以它不必被看懂。
        # 其他任何殘渣——tuple 變體、屬性、巨集——都要當場停下來。
        if chunk and not UNIT_VARIANT.match(chunk):
            raise SystemExit(
                f"✗ `Note` 的變體外面有一段這支檢查看不懂：{chunk!r}\n"
                "  看不懂就不能說「線以上放不下字」。先把剖析改對。"
            )
    return found


def main() -> int:
    source = SOURCE.read_text(encoding="utf-8")
    block = strip_comments(note_enum(source))

    fields = fields_of(block)
    variants = len(VARIANT.findall(block))
    if not fields or not variants:
        print("✗ 一個欄位都沒解析到——寫法變了，這支檢查等於沒在跑。")
        return 1

    problems = []
    used = set()
    for variant, name, ty in fields:
        ty = ty.strip()
        if ty in PLAIN:
            continue
        if (variant, name) in ALLOWED:
            used.add((variant, name))
            continue
        problems.append(f"    `{variant}.{name}: {ty}`")

    # 清單枯掉也是一種洞：欄位被改名或刪掉，這裡的例外就永遠不會被用到，而
    # 下一個人會以為那個名字仍然被守著。
    stale = sorted(set(ALLOWED) - used)

    print(f"掃了 `Note` 的 {variants} 個變體、{len(fields)} 個欄位。")
    for variant, name in sorted(used):
        print(f"  例外 `{variant}.{name}`：{ALLOWED[(variant, name)]}")

    if problems:
        print("\n✗ 這幾個欄位放得下螢幕上的字，而報告說線以上放不下：")
        print("\n".join(problems))
        print(
            "\n  要嘛換成數字／布林，要嘛加進這支腳本的 ALLOWED 並寫清楚"
            "「那段字最後去了哪裡」。"
        )
        return 1

    if stale:
        listed = "、".join(f"{variant}.{name}" for variant, name in stale)
        print(f"\n✗ ALLOWED 裡這幾格在 `Note` 上已經不存在了：{listed}")
        print("  留著會讓下一個人以為它還被守著。刪掉它。")
        return 1

    print("\n✔ 線以上每一格，型別上就放不下螢幕上的字。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
