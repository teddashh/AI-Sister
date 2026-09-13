#!/usr/bin/env python3
r"""文件裡寫的 bytes，必須是 repo 自己釘住的那個數字。

alpha.137 把 952 段語音全部重做，三包的總量都變了。同一組數字散在 README、
AGENTS、SPEC、PRIVACY 六個地方——我是靠 `grep` 一個一個找出來的，而 `grep` 只找
得到我想得到要找的那個數字。這一條把它變成機械的：**living doc 裡每一個
「…… bytes」都要對得上 repo 裡某一份 manifest／fixture 釘住的值**，對不上就紅。

刻意不掃的兩份：

- `docs/RELEASE-NOTES.md`：版本區段是歷史。「alpha.119 出貨 8,918,728 bytes」
  在那裡是真的，改成今天的數字才會變成假話。
- `docs/PHASES.md`：同理，`✅ alpha.NNN …` 那些行寫的是那一版做了什麼。

擋得住的：數字過期。實測把三包的總量換回上一版出貨的值，七處全被點名；
改一個字（`8,895,060` → `8,895,068`）也紅。（alpha.138 和 alpha.139 各換過一次
語音包，兩次都重打了同樣的三刀，數字換成當版的，結果一樣。）

**2026-09-12：張冠李戴那個洞補上了。** 舊版檔頭寫著它擋不住——把語音包的
`8,895,060` 換成 persona-reels 裡的 `1,431,301` 照樣綠，因為它只問「這個數字在
repo 裡存不存在」。當時的理由是「要知道每一句話講的是哪一包，而那是散文」。
散文其實讀得出來：每個數字旁邊都寫著它是哪一包（「同意書」「閒話」「日常」
「PNG」「WebP」「app icon」「ZIP」）。所以第二層改問**這個數字有沒有被那一包
自己的 manifest 釘住**。

配對法是「離數字最近的那個包名」，前後都找，但兩邊的邊界不一樣：
往前找到 `；。|：` 為止（同一句話裡的前一段還算數），往後只找到 `，；。|：`
為止（後面的包名只有在緊貼著數字時才算，例如「68 段（4,542,053 bytes）同意書
朗讀」）。距離量的是**包名結尾到數字的空隙**，不是包名開頭——量開頭會讓
「consent Ogg（4,542,053」輸給後面十個字外的「banter」。實測 23 個數字全部
配對成功、零錯配。

配不到包名的數字直接紅，不退回舊的寬鬆比對：退回去等於這一層可以被一句沒寫
包名的話繞過，而繞過的時候沒有人會知道。新的一類資產要在 `PACKS` 加一行。

還是擋不住的，三條，每一條都打過一刀 `want=綠` 證明過（正向的「會紅」我一直
在驗，反向的「抓不到」不驗的話，太樂觀或太保守都會害下一輪做錯決定）：

1. **整句話被刪掉。** 把 README 裡「340 段閒話短句、2,249,872 bytes」整段拿掉，
   這一條照樣綠，只是掃到的數字從 23 變成 22——而 22 沒有人在對。
2. **同一包內部張冠李戴。** 把同意書整包的 `4,542,053` 換成同一份 manifest 裡
   `sakana/azure-tts` 那一支的 `144,164`，照樣綠：包名配對只問「哪一份
   manifest」，問不出「哪一個欄位」。
3. **七個字元以下的數字根本不會被掃到。** `[\d,]{7,}` 的門檻是「100,000」，
   所以「34,919 bytes」寫錯了這裡不會知道。今天量過，四份文件裡這種數字有
   **0 個**，所以現在不花成本；有一天寫了單支 clip 的大小就會變成真的洞。

第 2 條我第一次的刀是壞的：拿了一支 `8,381` 的 clip，它連 `[\d,]{7,}` 都沒
越過，那個綠證明的是第 3 條不是第 2 條。**刀量級不夠的時候，綠是假的。**
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# 「這個數字是真的」的來源：釘住位元組數的 manifest 與 fixture。
PIN_SOURCES = (
    *sorted((ROOT / "apps" / "desktop").rglob("manifest.json")),
    *sorted((ROOT / "crates" / "sister-assets" / "tests" / "fixtures").glob("*.json")),
)

DOCS = ("README.md", "AGENTS.md", "docs/SPEC.md", "docs/PRIVACY.md")

FIGURE = re.compile(r"([\d,]{7,})\s*bytes")
BYTES_KEY = re.compile(r"bytes$", re.IGNORECASE)

# 每個數字旁邊都寫著它是哪一包。左邊是散文裡會出現的說法，右邊是有資格釘住
# 這個數字的來源路徑前綴。
PACKS = (
    ("同意書朗讀", ("同意書", "consent"), "apps/desktop/ui/persona-consent-voices/"),
    ("閒話短句", ("閒話", "banter"), "apps/desktop/ui/persona-banter-voices/"),
    ("日常台詞", ("日常", "基本包", "基本語音"), "apps/desktop/ui/persona-voices/"),
    ("角色 reel", ("PNG", "rig", "reel", "Reel"), "apps/desktop/ui/persona-reels/"),
    ("WebP 退路", ("WebP",), "apps/desktop/ui/personas/"),
    ("app icon", ("app icon", "icons"), "apps/desktop/src-tauri/icons/"),
    ("下載包 ZIP", ("ZIP", "cdn.ted-h.com", "entries"), "crates/sister-assets/tests/fixtures/"),
)
BACK_DELIM = "；。|："
FORWARD_DELIM = "，；。|："
CLAUSE_MAX = 160


def whose_number(flat: str, start: int, end: int) -> tuple[str, str] | None:
    """離這個數字最近的那個包名。回 (包名, 來源路徑前綴)，找不到回 None。"""
    lo = start
    while lo > 0 and start - lo < CLAUSE_MAX and flat[lo - 1] not in BACK_DELIM:
        lo -= 1
    hi = end
    while hi < len(flat) and hi - end < CLAUSE_MAX and flat[hi] not in FORWARD_DELIM:
        hi += 1
    back, forward = flat[lo:start], flat[end:hi]

    best: tuple[int, str, str] | None = None
    for label, words, prefix in PACKS:
        for word in words:
            at = back.rfind(word)
            # 量的是包名**結尾**到數字的空隙。量開頭會讓「consent Ogg（4,542,053」
            # 輸給十個字以外的「banter」，那正好是 SPEC 那一列的形狀。
            if at >= 0:
                gap = len(back) - (at + len(word))
                if best is None or gap < best[0]:
                    best = (gap, label, prefix)
            at = forward.find(word)
            if at >= 0 and (best is None or at < best[0]):
                best = (at, label, prefix)
    return None if best is None else (best[1], best[2])


def pinned_values() -> dict[int, list[str]]:
    found: dict[int, list[str]] = {}

    def walk(node: object, source: str, key: str | None) -> None:
        if isinstance(node, dict):
            for name, value in node.items():
                walk(value, source, name)
        elif isinstance(node, list):
            for value in node:
                walk(value, source, key)
        elif isinstance(node, int) and not isinstance(node, bool):
            if key and BYTES_KEY.search(key):
                found.setdefault(node, []).append(f"{source}:{key}")

    for path in PIN_SOURCES:
        try:
            data = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as error:
            raise SystemExit(f"讀不動 {path.relative_to(ROOT)}：{error}")
        walk(data, str(path.relative_to(ROOT)), None)
    return found


def main() -> int:
    pins = pinned_values()
    if not pins:
        print("✗ 一個釘住的 bytes 都找不到——這條檢查問錯地方了，不可以印成綠燈")
        return 1

    problems: list[str] = []
    checked = 0
    per_pack: dict[str, int] = {}
    for name in DOCS:
        text = (ROOT / name).read_text(encoding="utf-8")
        # 文件會折行，同一句話常常跨兩行；配對包名要看整句，所以攤平之後再找，
        # 行號另外由前面有幾個換行算回來。
        flat = text.replace("\n", " ")
        for match in FIGURE.finditer(flat):
            checked += 1
            number = text.count("\n", 0, match.start()) + 1
            value = int(match.group(1).replace(",", ""))
            if value not in pins:
                problems.append(
                    f"{name}:{number} 寫的 {match.group(1)} bytes 不是 repo 裡任何一份"
                    f" manifest 釘住的值——它多半是上一版的數字")
                continue
            owner = whose_number(flat, match.start(), match.end())
            if owner is None:
                problems.append(
                    f"{name}:{number} 的 {match.group(1)} bytes 附近沒有寫是哪一包——"
                    f"這一條就沒辦法確認它沒有張冠李戴。句子裡寫上包名，或在 PACKS 加一行")
                continue
            label, prefix = owner
            per_pack[label] = per_pack.get(label, 0) + 1
            if not any(src.startswith(prefix) for src in pins[value]):
                problems.append(
                    f"{name}:{number} 的 {match.group(1)} bytes 講的是「{label}」，"
                    f"但釘住這個數字的是 {sorted(pins[value])}——張冠李戴")

    print(f"對了 {checked} 個 bytes 數字，來源是 {len(PIN_SOURCES)} 份 manifest／fixture "
          f"裡的 {len(pins)} 個值。")
    if per_pack:
        print("  各包：" + "、".join(f"{k} {v}" for k, v in sorted(per_pack.items())))
    if problems:
        print(f"\n✗ {len(problems)} 個對不上：")
        for line in problems:
            print(f"    {line}")
        return 1
    print("✔ 文件上的每一個 bytes 都對得上 repo 自己釘住的數字。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
