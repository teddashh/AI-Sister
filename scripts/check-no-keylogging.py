#!/usr/bin/env python3
"""PRIVACY.md 最強的那一句，由 CI 守著而不是由記性守著。

    「按鍵內容。不是「過濾掉」，是從來沒讀過——Windows 的低階鍵盤 hook
      會給我們一個指向 `KBDLLHOOKSTRUCT` 的指標，那裡面有 `vkCode`。
      程式碼從頭到尾沒有解參考那個指標。」

這句話的份量在於它**不需要任何人被信任**：不是「我們有記得過濾」，是那些
位元組從來沒有進入過這個程序的記憶體。DATA_INVENTORY 也照抄了一次。

而在這支腳本存在之前，守著它的東西是零。不是「測試比較弱」，是沒有測試、
沒有 CI 檢查、沒有 lint。明天有人為了做「打字速度熱區圖」加一行

    let vk = unsafe { (*(lparam.0 as *const KBDLLHOOKSTRUCT)).vkCode };

整套測試照樣全綠，PRIVACY.md 從那一刻起變成一句謊話，而且沒有人會發現。
這正是 `check-no-network.sh` 存在的同一個理由，只是那一句被守了、這一句沒有。

## 檢查的形狀：正面表列，不是負面表列

黑名單（找 `*lparam`、找 `KBDLLHOOKSTRUCT`）擋不住有心或無意的變形：
`std::ptr::read`、`transmute`、先把 `lparam.0` 存進區域變數再解、或是換一個
型別名稱。所以這裡反過來——**`kb_proc` 的函式體裡，`lparam` 只准出現在
交還給 `CallNextHookEx` 的那一次**。

只要那個指標沒有被傳給任何別的東西、也沒有被算術碰過，它裡面的位元組就
不可能被讀出來。這個條件很嚴格，嚴格到會擋掉一些其實無害的寫法——那是
刻意的。這一句承諾不值得為了寫起來方便而放寬。

對照組是隔壁的 `mouse_proc`：它**確實**解參考 `MSLLHOOKSTRUCT` 去讀座標，
而 PRIVACY.md 也是這樣寫的（「讀座標（位置不是內容）」）。所以這支腳本
只盯 `kb_proc`——如果它連 `mouse_proc` 都能通過，那它就沒有在檢查任何東西。
最下面那個自我檢查就是在確認這件事。
"""

import re
import sys
from pathlib import Path

SRC = Path(__file__).resolve().parent.parent / "crates/sister-capture/src/windows/input.rs"

# `lparam` 唯一被允許出現的地方：原封不動交還給下一個 hook。
ALLOWED = re.compile(r"CallNextHookEx\s*\(\s*None\s*,\s*code\s*,\s*wparam\s*,\s*lparam\s*\)")


def body_of(source: str, fn: str) -> str:
    """抓出 `fn` 的函式體（不含簽章），用大括號配對。"""
    m = re.search(r"\bfn\s+" + re.escape(fn) + r"\s*\(", source)
    if not m:
        fail(f"在 {SRC.name} 裡找不到 `{fn}`。它被改名或刪掉了，"
             "而這支腳本是靠名字找到它的——先確認那句承諾還成立，再改這裡")

    brace = source.index("{", m.end())
    depth = 0
    for i in range(brace, len(source)):
        if source[i] == "{":
            depth += 1
        elif source[i] == "}":
            depth -= 1
            if depth == 0:
                return source[brace + 1 : i]
    fail(f"`{fn}` 的大括號沒有配對成功——這支腳本讀不懂這個檔案了")


def offending_lines(body: str) -> list[tuple[int, str]]:
    """函式體裡每一個「不是交還給 CallNextHookEx」的 lparam。"""
    out = []
    for n, line in enumerate(body.splitlines(), 1):
        code = line.split("//", 1)[0]  # 註解裡提到 lparam 是好事，不是違規
        if "lparam" not in code:
            continue
        if ALLOWED.search(code):
            continue
        out.append((n, line.strip()))
    return out


def fail(msg: str) -> None:
    print(f"::error::{msg}")
    sys.exit(1)


def main() -> None:
    source = SRC.read_text(encoding="utf-8")

    bad = offending_lines(body_of(source, "kb_proc"))
    if bad:
        print("✗ `kb_proc` 動了那個指向 KBDLLHOOKSTRUCT 的指標：")
        for n, line in bad:
            print(f"    第 {n} 行  {line}")
        print()
        fail(
            "PRIVACY.md 寫著「程式碼從頭到尾沒有解參考那個指標」，"
            "而那句話的價值在於它不需要任何人被信任。"
            "要嘛把這行拿掉，要嘛去改 PRIVACY.md 和 DATA_INVENTORY.md"
            "——但不能兩個都不做"
        )

    # 自我檢查：這個條件必須嚴格到連 `mouse_proc` 都會被擋下來。
    # `mouse_proc` 確實會解 `MSLLHOOKSTRUCT` 去讀座標（PRIVACY.md 也是
    # 這樣寫的），所以它**應該**違規。如果它通過了，代表上面那個檢查
    # 根本沒有在檢查任何東西——一個永遠是綠的檢查跟不存在是同一件事。
    if not offending_lines(body_of(source, "mouse_proc")):
        fail(
            "自我檢查失敗：這個規則連 `mouse_proc` 都攔不下來，"
            "而 `mouse_proc` 是真的會解指標的那一個。"
            "所以它對 `kb_proc` 的那個綠燈不代表任何事"
        )

    print("✓ `kb_proc` 只把那個指標交還給 CallNextHookEx，從來沒有讀過它")
    print("  （自我檢查：同一條規則抓得到 `mouse_proc` 的解參考）")


if __name__ == "__main__":
    main()
