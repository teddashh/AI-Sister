#!/usr/bin/env python3
"""文件裡寫的 bytes，必須是 repo 自己釘住的那個數字。

alpha.137 把 952 段語音全部重做，三包的總量都變了。同一組數字散在 README、
AGENTS、SPEC、PRIVACY 六個地方——我是靠 `grep` 一個一個找出來的，而 `grep` 只找
得到我想得到要找的那個數字。這一條把它變成機械的：**living doc 裡每一個
「…… bytes」都要對得上 repo 裡某一份 manifest／fixture 釘住的值**，對不上就紅。

刻意不掃的兩份：

- `docs/RELEASE-NOTES.md`：版本區段是歷史。「alpha.119 出貨 8,918,728 bytes」
  在那裡是真的，改成今天的數字才會變成假話。
- `docs/PHASES.md`：同理，`✅ alpha.NNN …` 那些行寫的是那一版做了什麼。

擋得住的：數字過期。實測把三包的總量換回 alpha.136 出貨的值，七處全被點名；
改一個字（`8,977,603` → `8,977,608`）也紅。

擋不住的（打過一刀 `want=綠` 確認過，不是猜的）：**張冠李戴**。把語音包的
`8,977,603` 換成 persona-reels 裡的 `1,431,301`，這一條照樣綠——它只問「這個數字
在 repo 裡存不存在」，不問「它屬不屬於這一包」。要守後者得知道每一句話講的是哪
一包，而那是散文，這裡讀不出來。另外整句話被刪掉也擋不住（不會留下數字可以查）。
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
    for name in DOCS:
        path = ROOT / name
        for number, line in enumerate(path.read_text(encoding="utf-8").split("\n"), 1):
            for match in FIGURE.finditer(line):
                checked += 1
                value = int(match.group(1).replace(",", ""))
                if value not in pins:
                    problems.append(
                        f"{name}:{number} 寫的 {match.group(1)} bytes 不是 repo 裡任何一份"
                        f" manifest 釘住的值——它多半是上一版的數字")

    print(f"對了 {checked} 個 bytes 數字，來源是 {len(PIN_SOURCES)} 份 manifest／fixture "
          f"裡的 {len(pins)} 個值。")
    if problems:
        print(f"\n✗ {len(problems)} 個對不上：")
        for line in problems:
            print(f"    {line}")
        return 1
    print("✔ 文件上的每一個 bytes 都對得上 repo 自己釘住的數字。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
