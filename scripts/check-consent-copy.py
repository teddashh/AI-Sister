#!/usr/bin/env python3
"""檢查三張同意書逐字一致，且五份撤回文案符合程式的重讀週期。"""

import json
import re
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
RUST = ROOT / "crates/sister-core/src/consent.rs"
UI = ROOT / "apps/desktop/ui/onboarding.js"
OPS = ROOT / "crates/sister-cli/src/ops.rs"


def fail(message: str) -> None:
    print(f"::error::{message}")
    raise SystemExit(1)


def rust_strings(method: str) -> list[str]:
    source = RUST.read_text(encoding="utf-8")
    start = source.find(f"pub fn {method}(")
    if start < 0:
        fail(f"consent.rs 找不到 {method}()")
    end = source.find("\n    }\n", start)
    if end < 0:
        fail(f"consent.rs 無法切出 {method}()")
    body = source[start:end]
    values = re.findall(r'=>\s*(?:\{\s*)?"((?:[^"\\]|\\.)*)"', body, re.DOTALL)
    if len(values) != 3:
        fail(f"consent.rs 的 {method}() 應有三張文案，實際抓到 {len(values)} 張")
    return [json.loads(f'"{value}"') for value in values]


def js_strings(field: str) -> list[str]:
    source = UI.read_text(encoding="utf-8")
    match = re.search(rf"\b{field}:\s*(\[.*?\])\s*,", source, re.DOTALL)
    if not match:
        fail(f"onboarding.js 的 DEMO 找不到 {field} 陣列")
    try:
        values = json.loads(re.sub(r",\s*]$", "]", match.group(1)))
    except json.JSONDecodeError as error:
        fail(f"onboarding.js 的 {field} 陣列不是可比對的字串陣列：{error}")
    if len(values) != 3 or not all(isinstance(value, str) for value in values):
        fail(f"onboarding.js 的 {field} 應有三張字串文案")
    return values


def consent_seconds() -> int:
    source = OPS.read_text(encoding="utf-8")
    matches = re.findall(
        r"^\s*const\s+CONSENT_EVERY\s*:\s*Duration\s*=\s*Duration::from_secs\(([0-9]+)\);",
        source,
        re.MULTILINE,
    )
    if len(matches) != 1:
        fail(
            "ops.rs 應恰有一個 `CONSENT_EVERY: Duration = "
            f"Duration::from_secs(N);`，實際抓到 {len(matches)} 個"
        )
    seconds = int(matches[0])
    if seconds <= 0:
        fail(f"ops.rs 的 CONSENT_EVERY 必須大於 0 秒，實際是 {seconds}")
    return seconds


for rust_method, js_field in (("wording", "wording"), ("without", "without")):
    expected = rust_strings(rust_method)
    actual = js_strings(js_field)
    if actual != expected:
        for index, (left, right) in enumerate(zip(expected, actual), start=1):
            if left != right:
                fail(
                    f"第 {index} 張同意書的 {js_field} 文案分岔："
                    f"consent.rs={left!r}，onboarding.js={right!r}"
                )
        fail(f"同意書的 {js_field} 文案數量分岔")

seconds = consent_seconds()
needles = {
    RUST: (
        f"每 {seconds} 秒重讀同意書，最多再錄 {seconds} 秒加一拍",
        f"capture.min_interval_ms 超過 {seconds} 秒時，主要會等那一拍",
    ),
    UI: (
        f"每 {seconds} 秒重讀同意書，最多再錄 {seconds} 秒加一拍",
        f"capture.min_interval_ms 超過 {seconds} 秒時，主要會等那一拍",
    ),
    ROOT / "README.md": (
        f"每 {seconds} 秒重讀同意書，最多再錄 {seconds} 秒加一拍",
        f"`capture.min_interval_ms` 超過 {seconds} 秒時，主要會等那一拍",
    ),
    ROOT / "docs/WINDOWS-CHECKLIST.md": (
        f"每 {seconds} 秒重讀同意書，所以撤回後最多再錄 {seconds} 秒加一拍",
        f"`capture.min_interval_ms` 超過 {seconds} 秒時，主要會等那一拍",
    ),
    ROOT / "docs/PRIVACY.md": (
        f"每 {seconds} 秒\n  重讀一次同意書；撤回第一張後最多再錄 {seconds} 秒加一拍",
        f"`capture.min_interval_ms`\n  超過 {seconds} 秒時，主要會等那一拍",
    ),
}
for path, path_needles in needles.items():
    source = path.read_text(encoding="utf-8")
    for needle in path_needles:
        if needle not in source:
            fail(
                f"程式的 CONSENT_EVERY 是 {seconds} 秒，但 {path.relative_to(ROOT)} "
                f"沒有承諾「{needle.replace(chr(10), ' ')}」"
            )

print(
    "三張同意書的條文與未簽後果：consent.rs / onboarding.js 逐字一致；"
    f"撤回週期的五份文案都與 ops.rs 的 CONSENT_EVERY={seconds} 秒一致"
)
