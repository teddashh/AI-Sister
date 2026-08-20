#!/usr/bin/env python3
"""鍵盤上沒有一顆鍵叫 KeyP。

熱鍵在程式裡的形狀是 `Ctrl+Alt+KeyP`——那是 Tauri 的 accelerator 語法，而
`HotkeyView.wanted` / `.rejected` 一路帶著它。給人看的時候要先過
`sister_shell::pretty_combo`（Rust）或 settings.js 裡那個 `pretty`（畫面），
兩個都是把 `KeyP` / `Digit1` 前面那截拔掉，變成 `Ctrl + Alt + P`。

為什麼要一支腳本守：這條規則的每一個違反都長得**完全正常**。

    format!("還在用 {}。", restored.wanted)

這行編得過、clippy 綠、review 讀起來也對——它確實在講「還在用哪一組」。只有
真的把那句話印出來，才看得到它叫使用者去按一顆不存在的鍵。而這句話出現的時機
（設定檔存不進去、退回舊的那一組）在測試機上幾乎踩不到。

而且它**沒有任何測試守著**：那句話在 `apps/desktop/src-tauri`，那是一個獨立的
workspace，只跑 `cargo check` 和 `clippy`，從來沒有 `cargo test` 過。把
`main.rs` 那一行改回 `restored.wanted` 是紅不了任何東西的。

這支腳本上一版是 shell，而它有三個洞，三個都是對抗式稽核當場示範出來的：

1. **`cargo fmt` 把紅的變成綠的。** 那一版的 grep 要求 `format!` 和 `.wanted`
   在**同一行**。那句話是中文，本來就貼著 rustfmt 的行寬，所以寫出違規 → 閘門
   紅 → 跑一次 `cargo fmt`（CI 本來就要求）→ 它被拆成三行 → 閘門綠，而那句
   叫人去按 `KeyP` 的話原封不動。**一個會被例行動作關掉的閘門，比沒有更糟。**
2. **「這一步自己失敗了嗎」那道保險是死的。** `set -o pipefail` 取的是**最右邊**
   那個非零退出碼：`grep -r` 因為路徑不存在而回 2，後面那個 `grep -v` 拿到空
   輸入回 1，於是 `rc` 是 1，而檢查寫的是 `[ "$rc" -gt 1 ]`。路徑打錯一個字，
   閘門從此永遠是綠的，唯一的痕跡是 stderr 上一行 `No such file or directory`。
3. **`grep -v` 排掉的是整行。** 一句話裡把 `rejected` 包好、`wanted` 沒包
   （`main.rs` 那一行正是兩個混在一起的形狀），它會被自己正確的那一半赦免。

Python 讓前兩個不存在（讀整個檔案、找不到路徑就當場出錯），第三個靠「先把包好
的那幾個拔掉，再看還剩什麼」。
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# 那個 accelerator 欄位的兩個名字。`wanted` 是現在設成哪一組，`rejected` 是剛剛
# 試了但沒搶到的那一組——兩個都是生的。
FIELDS = re.compile(r"\.(wanted|rejected)\b")
CJK = re.compile(r"[一-鿿]")

failed = []


def die(msg, *rest):
    failed.append(msg)
    print(f"✗ {msg}")
    for line in rest:
        print(f"  {line}")


def read(rel):
    """讀一份檔案。找不到就當場算輸——這是那個 shell 版活不下來的地方。"""
    path = ROOT / rel
    if not path.is_file():
        die(
            f"要掃的檔案不在：{rel}",
            "這不是「沒找到違規」。在修好之前，這支腳本守的那條線是一格空白。",
        )
        return None
    return path.read_text(encoding="utf-8")


def calls(source, names):
    """把 `format!(…)` 這種呼叫整個抓出來，**跨行**，括號配對。

    回傳 `(行號, 整段文字)`。跨行是重點：`cargo fmt` 會把一句中文的
    `format!` 拆成三行，而上一版的 grep 是一行一行看的。
    """
    out = []
    pattern = re.compile(r"\b(" + "|".join(names) + r")\s*\(")
    for m in pattern.finditer(source):
        depth, i = 0, m.end() - 1
        while i < len(source):
            c = source[i]
            if c == "(":
                depth += 1
            elif c == ")":
                depth -= 1
                if depth == 0:
                    break
            i += 1
        out.append((source.count("\n", 0, m.start()) + 1, source[m.start() : i + 1]))
    return out


# ── Rust ──────────────────────────────────────────────────────────────
#
# 只看在組字串的那幾個巨集，而且只看**話是給人看的**那幾句（含中文）。
# `tracing::info!("暫停熱鍵 {} 搶到了", view.wanted)` 是刻意放過的：log 給的是
# 開發者，而開發者要的正是原始的 accelerator 字串（照著它去查 Tauri 的文件）。
print("▶ Rust：給人看的句子裡有沒有生的 accelerator")
for rel in ["apps/desktop/src-tauri/src/main.rs", "crates/sister-shell/src/lib.rs"]:
    src = read(rel)
    if src is None:
        continue
    for line, call in calls(src, ["format!", "write!", "writeln!", "push_str"]):
        if not CJK.search(call):
            continue
        # 日誌巨集自己就長成 `format!` 的樣子，用前綴認出來。
        head = src[: src.find(call)].rsplit("\n", 1)[-1]
        if "tracing::" in head or "tracing::" in call:
            continue
        # **先把包好的那幾個拔掉**，剩下的才是漏網的。整行 `grep -v` 會被一句話
        # 裡正確的那一半赦免掉另一半。
        stripped = re.sub(r"pretty_combo\s*\([^)]*\)", "", call)
        if FIELDS.search(stripped):
            die(
                f"{rel}:{line} 有一句給人看的話直接印了 accelerator",
                " ".join(call.split())[:160],
                "包一層 sister_shell::pretty_combo(…)。生的那個字串裡的 KeyP /",
                "Digit1 在鍵盤上是不存在的鍵，而這幾句話正是要他照著去按的。",
            )

# 反過來也要守：`pretty_combo` 被刪掉、或被改成 identity，上面那一圈會全綠——
# 它只看得到「有沒有呼叫」，看不到「呼叫了有沒有用」。
print("▶ pretty_combo 自己還在不在，而且真的拔得掉前綴")
shell = read("crates/sister-shell/src/lib.rs")
if shell is not None:
    if "pub fn pretty_combo" not in shell:
        die(
            "crates/sister-shell/src/lib.rs 裡沒有 pretty_combo 了",
            "上面那一圈從此永遠是綠的——它問的是「有沒有呼叫它」。",
        )
    elif "assert_eq!(pretty_combo(" not in shell:
        die(
            "pretty_combo 還在，但沒有 assert 釘著它拔前綴的行為",
            "把它的內容換成 combo.to_string() 不會紅任何東西，而上面那一圈照樣綠。",
        )

# ── 畫面 ──────────────────────────────────────────────────────────────
#
# 這一邊用**白名單**而不是黑名單，因為 JS 這邊只有一個檔案在碰這兩個欄位，而
# 白名單擋得住黑名單擋不住的那一種：`const raw = view.wanted;` 之後再去插值。
# 那一行本身沒有中文、也沒有 `pretty(`，黑名單版整支腳本外加另外五支閘門都放它
# 過（實測）。
#
# 允許的只有兩種：拿去**比較**（`view.wanted === ""`），或**包在 `pretty(…)`
# 裡面**。要多一種用途就來改這裡——那正是應該被看見的時候。
print("▶ 畫面：那兩個欄位只准拿去比較，或包在 pretty() 裡")
js = read("apps/desktop/ui/settings.js")
if js is not None:
    if "function pretty(" not in js:
        die("settings.js 裡沒有 pretty() 了", "下面那一圈從此沒有東西可以認。")
    # 註解不算數。上一版會把一行「// view.wanted 是生的 accelerator…」抓成違規
    # ——一個因為**解釋了這條規則**而被判違規的註解。
    body = re.sub(r"/\*.*?\*/", "", js, flags=re.S)
    body = re.sub(r"^\s*(//|\*).*$", "", body, flags=re.M)
    # 比較涵蓋 `=== ""`、`!== ""`、`!= null` 這三種都在用的寫法，兩邊順序都認。
    ok = re.compile(
        r"pretty\s*\(\s*view\.(wanted|rejected)\s*\)"
        r"|view\.(wanted|rejected)\s*[!=]=+"
        r"|[!=]=+\s*view\.(wanted|rejected)"
    )
    for i, line in enumerate(body.split("\n"), 1):
        rest = ok.sub("", line)
        if re.search(r"view\.(wanted|rejected)\b", rest):
            die(
                f"settings.js:{i} 那個欄位既不是拿去比較、也沒有包在 pretty() 裡",
                line.strip()[:160],
                "包一層 pretty(…)。要是真的需要生的那一份，先想清楚它會不會走到畫面上。",
            )

print()
if failed:
    sys.exit(1)
print("✓ 沒有任何一句給人看的話會叫他去按 KeyP")
