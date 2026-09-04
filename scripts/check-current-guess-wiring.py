#!/usr/bin/env python3
"""「這一刻」那張卡的次數與結局，接線接對了沒有。

`memory_current_guess`（`apps/desktop/src-tauri/src/main.rs`）要做兩件事，
才能讓卡片說得出「她試著問過 N 次，最近一次是⋯」：

    let previous_attempts = db
        .retained_interpreter_attempts_for_segment(seg.core_started_at)   # ← 1
        .map_err(|e| format!("{e:#}"))?;
    let status = CurrentGuess::while_recording(…, previous_attempts);     # ← 2

**為什麼要一支腳本守，而不是一條測試。**

`apps/desktop` 是一個獨立的 workspace（根 `Cargo.toml` 的
`members = ["crates/*"]` 碰不到它）。CI 對它只跑 `cargo check`、
`cargo clippy --all-targets` 和 `cargo build --release`，**從來沒有
`cargo test` 過**。所以那兩行改壞了，`cargo test --workspace` 的 20 個 binary
不會有任何一個變紅。這不是推測——底下兩種改法我都實跑過，26 條閘門全綠：

    - `.retained_interpreter_attempts_for_segment(seg.core_ended_at)`
      `brain_outbound.segment_core_start` 存的是 `core_started_at`，
      用 `core_ended_at` 查一定是 0 列 → `None` → 卡片永遠退回舊那句
      「最新一段值得理解，正在等解釋層處理。」。型別一樣，clippy 不會叫。
    - `let previous_attempts = None;`
      同樣安靜，同樣全綠。

同一個形狀在這個 repo 已經出現第六次了（見 `check-combo-is-readable.py`
的開頭，那一支是為了完全一樣的理由存在的）。結構解是把接線搬進 `crates/`，
在那之前這支腳本是唯一會紅的東西。

**為什麼是 python 而不是 grep。** `cargo fmt` 會把這個呼叫拆成好幾行，
一行一行看的 grep 會因此從紅變綠——一個被例行動作關掉的閘門比沒有更糟。
這裡先把整個檔案的空白壓成單一空格再比對，行怎麼拆都不影響。

**這支腳本守不住的那一半，寫在這裡：**

  - 它只認得 `seg.core_started_at` 這個**字面**。有人把 `seg` 換成別的變數、
    或先 `let key = seg.core_ended_at;` 再傳 `key`，它看不出來。
  - 它不檢查 `while_recording` 前面五個引數對不對。
  - 它證明不了那條路真的會被走到——那要靠桌面自己的測試，而那正是沒有的東西。

找不到檔案、找不到那個呼叫、比對不出來，一律**非零退出**。
回報 0 的時候我分不出「真的沒問題」和「我問錯了」，所以每一條都要吵。
"""

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
MAIN = ROOT / "apps/desktop/src-tauri/src/main.rs"

if not MAIN.exists():
    print(f"✗ 找不到 {MAIN}——路徑改了就要改這支腳本，不可以安靜地過")
    sys.exit(2)

src = MAIN.read_text(encoding="utf-8")
flat = re.sub(r"\s+", " ", src)

problems = []

# 1. 查詢的 key 必須是 core_started_at
QUERY = "retained_interpreter_attempts_for_segment"
calls = re.findall(re.escape(QUERY) + r" *\( *([^)]*)\)", flat)
if not calls:
    problems.append(
        f"完全找不到 {QUERY}( 的呼叫——「這一刻」那張卡不可能說得出次數與結局。"
    )
elif len(calls) != 1:
    problems.append(f"{QUERY}( 出現 {len(calls)} 次，這支腳本只認得剛好一次。")
else:
    # rustfmt 把呼叫拆行時會留一個尾逗號（`(\n    seg.core_started_at,\n)`），
    # 壓平之後就是 `seg.core_started_at,`。第一版在這裡用完全相等去比，於是
    # 跑一次 `cargo fmt` 就會把閘門弄紅——方向雖然是 fail-closed，但一個會被
    # 例行動作弄紅的閘門一樣會被人關掉。實測補上的。
    arg = calls[0].strip().rstrip(",").strip()
    if arg != "seg.core_started_at":
        problems.append(
            f"{QUERY}( 的引數是 `{arg}`，不是 `seg.core_started_at`。\n"
            f"      `brain_outbound.segment_core_start` 存的是 core_started_at；\n"
            f"      查錯欄位會安靜地回 0 列，卡片就永遠退回「正在等解釋層處理」。"
        )

# 2. while_recording 的最後一個引數必須是算出來的 previous_attempts
CALL = "CurrentGuess::while_recording"
idx = flat.find(CALL + " (")
if idx == -1:
    idx = flat.find(CALL + "(")
if idx == -1:
    problems.append(f"找不到 {CALL} 的呼叫——組裝端不見了？")
else:
    start = flat.index("(", idx)
    depth, end = 0, None
    for i in range(start, len(flat)):
        if flat[i] == "(":
            depth += 1
        elif flat[i] == ")":
            depth -= 1
            if depth == 0:
                end = i
                break
    if end is None:
        problems.append(f"{CALL} 的括號沒有收——這支腳本剖不動，當作紅的。")
    else:
        inner = flat[start + 1 : end]
        # 只在最外層切逗號
        args, depth, cur = [], 0, ""
        for ch in inner:
            if ch in "([{":
                depth += 1
            elif ch in ")]}":
                depth -= 1
            if ch == "," and depth == 0:
                args.append(cur.strip())
                cur = ""
            else:
                cur += ch
        if cur.strip():
            args.append(cur.strip())
        if len(args) != 6:
            problems.append(
                f"{CALL} 收到 {len(args)} 個引數，這支腳本認得的是 6 個：{args}"
            )
        # 要的是「不可以是寫死的常數」，不是「必須逐字長這樣」。
        # `previous_attempts.clone()` 之類的改法是正當的，不該把閘門弄紅——
        # 一個會對正當改動叫的閘門，遲早會被人關掉。
        elif "previous_attempts" not in args[5]:
            problems.append(
                f"{CALL} 的第 6 個引數是 `{args[5]}`，裡面沒有 `previous_attempts`。\n"
                f"      寫死成 None 的話這一版的功能整個消失，而所有測試都是綠的。"
            )

if problems:
    print("✗ 「這一刻」那張卡的接線斷了：")
    for p in problems:
        print(f"    - {p}")
    sys.exit(1)

print("✓ 「這一刻」的次數與結局接線完整（查 core_started_at、傳 previous_attempts）")
