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

**這支腳本守不住的那一半，寫在這裡。**

上一版這一段寫的是「第一版的 Python 還有四個洞，三個已經補掉，剩下這一個補不
掉」，然後只列了 `.clone()` 那一個。**那個數字是假的**：隔天的第二輪稽核在同
一支腳本上又示範了五個，其中三個是那一輪修法自己造出來的。所以這一段改成一份
清單，而不是一個數字——數字會過期，而過期的數字讀起來像保證。

alpha.42 補掉的（每一條都有實測的重現，見 `mask()` 和底下兩段的註解）：

  · 生字串那道保險只認 `r#+"`，沒有 `#` 的 `r"…\\"` 整支漏掉 → 引號奇偶性翻轉
    → 一個一定會紅的違規**一個字都沒印**
  · Rust 的 `'"'`（字元字面量）同樣開一個假字串
  · JS 的 `/["]/`（正規表示式字面量）同樣開一個假字串
  · `view["wanted"]`、`function raw(v){return v.wanted}` + `raw(view)`
    ——白名單認的是 `view.` 那幾個字，這兩種都不長那樣
  · `notpretty(view.wanted)`：`before.endswith("pretty(")` 對任何名字結尾是
    `pretty` 的函式都放行
  · `tracing::` 那道豁免保護不了任何東西，但賣得出免死金牌（同一行前面塞一句
    `tracing::debug!("x")` 就好）
  · CJK 那道過濾漏掉「把句子拆成兩次 `push_str`」

而且 `mask()` 現在會**驗自己的輸出**（見那段 canary）：抹過的字串裡面只該剩
空白。這一條的用處不在於它認得哪幾種語法，而在於它不必認得——下一種沒想到的
寫法會撞牆，不會安靜地放行。

**還開著的：**

    let w = restored.wanted.clone();
    format!("還在用 {w}。")

那個欄位在 `format!` 外面就被抄走了，而這支腳本掃的是 `format!(…)` 括號裡面。
Rust 這邊沒辦法像畫面那邊一樣改用白名單——`.clone()` / `.is_empty()` /
`rejected: Some(view.wanted)` 三種合法用法都要放行，而 `.clone()` 正是這個洞
走的那一條。真正堵得住的只有一個 `Combo` newtype（讓生字串根本 `Display` 不
出來），而那是一次跨 crate 的改動。

還有：只掃那兩份 Rust 檔和 `settings.js`。第四個地方開始碰那兩個欄位的時候，
這支腳本不會知道。

**寫在這裡是因為「看起來被守住了」比「知道沒被守住」危險。** 這一批修改自己
就犯過同一件事：把一整類問題標成修好了，而它比較大的那一半還開著。
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# 那個 accelerator 欄位的兩個名字。`wanted` 是現在設成哪一組，`rejected` 是剛剛
# 試了但沒搶到的那一組——兩個都是生的。
FIELDS = re.compile(r"\.(wanted|rejected)\b")

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


# Rust 的生字串：`r"…"`、`r#"…"#`、`br"…"`。裡面沒有跳脫字元，收尾是引號後面
# 接同樣多的 `#`。
RAW = re.compile(r'(?:b|c|rb|br)?r(#*)"')
# Rust 的字元字面量。`'a'`、`'\n'`、`'"'`。生命週期（`'a` 後面沒有收尾的單引號）
# 配不上這個式子，所以照原樣走過去。
CHAR = re.compile(r"'(?:\\.|[^'\\\n])'")
# JS 的正規表示式字面量。`[...]` 裡面的 `/` 不是收尾。
JS_RE = re.compile(r"/(?![*/])(?:\\.|\[(?:\\.|[^\]\\\n])*\]|[^/\\\n\[])+/[dgimsuvy]*")
# `/` 前面是這些的時候，它是正規表示式的開頭，不是除號。
JS_RE_AFTER = set("(,=:[!&|?{};+-*%^~<>") | {""}
JS_RE_WORDS = {"return", "typeof", "case", "in", "of", "do", "else", "void", "yield", "await", "new"}


def mask(source, rel, lang="rust"):
    """把每一段字串和註解的**內容**換成同長度的空白，換行留著。

    **同長度**是重點：換完之後每一個位移都還對得回原檔，所以行號是真的。
    上一版是 `re.sub(r"/\\*.*?\\*/", "", js, flags=re.S)`——它把註解裡的換行
    一起吃掉，於是底下 `enumerate` 出來的行號和 `settings.js` 差了幾十行。
    實測：真的違規在 `:358`，閘門指著 `:289`，而 `:289` 是
    `function pretty(combo) {`——**它指著自己叫你去呼叫的那支函式**。

    字串也要抹，理由是括號配對器不認得字串：一句話裡出現一個半形 `)`（顏文字
    `:)`、或「（見下）」寫成半形），就會被當成呼叫的結尾，那一句後面剩下的
    整段逃掉。實測 `format!("還在用 {}。 :)", restored.wanted)` 在這之前是綠的。

    **上一版對生字串是「遇到就當場算輸」，而那道保險只認 `r#+"`。** `#` 至少
    要一個，所以沒有 `#` 的那種——`r"C:\\Users\\ted\\AppData\\"`，Windows 路徑
    最自然的寫法——整支漏掉：結尾那個 `\\"` 被當成跳脫字元，字串沒收掉，整個檔案
    的引號奇偶性從那裡翻轉，後面每一句真的字串變成「程式碼」、每一段程式碼變成
    「字串」。實測一個一定會紅的違規變成**綠的，一個字都沒印**。而 docstring
    上一行自己寫著「一個讀錯的掃描器會安靜地放行，而那正是這支腳本在抓的東西」
    ——只實作了一半，漏掉的那一半就是「安靜地放行」。

    這一輪的判斷是：**不能靠死**。這顆 repo 裡 `r"…"` 有 43 處、`'"'` 有 3 處，
    死在上面就是一支對著正確的程式碼喊紅的閘門，而那種閘門會被關掉。所以三種
    都真的讀懂：生字串（含 `b`/`c` 前綴、任意個 `#`）、Rust 的字元字面量（和
    生命週期分得開）、JS 的正規表示式字面量（`/["]/` 那個 `"` 以前會開一個假
    字串）。

    讀懂之後還要**自己驗一次**，見底下那段 canary：抹完的字串裡面只該剩空白，
    剩下別的就代表剛剛吞掉了一段程式碼。這是「假的那一半要從真的那一半推出來」
    ——不然下一種沒想到的寫法還是會安靜地放行。
    """
    quotes = '"' if lang == "rust" else "\"'`"
    out = list(source)
    i, n = 0, len(source)
    holes = []  # `${…}` 那幾段是**刻意**不抹的，canary 要跳過它們

    def wipe(j):
        if out[j] != "\n":
            out[j] = " "

    while i < n:
        c = source[i]
        two = source[i : i + 2]
        if two == "//":
            i += 2
            while i < n and source[i] != "\n":
                wipe(i)
                i += 1
        elif two == "/*":
            i += 2
            while i < n and source[i : i + 2] != "*/":
                wipe(i)
                i += 1
            i += 2
        elif (
            lang == "rust"
            and c in "brc"
            and (i == 0 or not (source[i - 1].isalnum() or source[i - 1] == "_"))
            and RAW.match(source, i)
        ):
            m = RAW.match(source, i)
            close = '"' + m.group(1)
            end = source.find(close, m.end())
            if end == -1:
                die(f"{rel} 裡有一個生字串收不掉（{close!r} 找不到）", "讀不完就別裝作讀懂了。")
                return None
            for j in range(m.end(), end):
                wipe(j)
            i = end + len(close)
        elif lang == "rust" and c == "'" and CHAR.match(source, i):
            m = CHAR.match(source, i)
            for j in range(i + 1, m.end() - 1):
                wipe(j)
            i = m.end()
        elif lang == "js" and c == "/" and two not in ("//", "/*") and JS_RE.match(source, i):
            # 除號還是正規表示式，看**前一個有意義的字元**。註解已經在上面被抹成
            # 空白了，所以往回跳空白就是往回跳註解。
            k = i - 1
            while k >= 0 and out[k] in " \t\n":
                k -= 1
            prev = out[k] if k >= 0 else ""
            word = ""
            if prev.isalnum() or prev in "_$":
                e = k + 1
                while k >= 0 and (out[k].isalnum() or out[k] in "_$"):
                    k -= 1
                word = "".join(out[k + 1 : e])
            if prev in JS_RE_AFTER or word in JS_RE_WORDS:
                m = JS_RE.match(source, i)
                for j in range(i + 1, m.end() - 1):
                    wipe(j)
                i = m.end()
            else:
                i += 1
        elif c in quotes:
            close, i = c, i + 1
            while i < n:
                if source[i] == "\\":
                    wipe(i)
                    if i + 1 < n:
                        wipe(i + 1)
                    i += 2
                    continue
                if source[i] == close:
                    break
                # `${…}` 裡面是**程式碼**，不是字串內容。整段留著。
                #
                # 這一段是我自己在修行號的時候當場造出來的：把樣板字串整個抹掉
                # 之後，`` `…按 ${view.wanted} 都會…` `` 裡那個欄位跟著不見了，
                # 於是一個真的違規從紅變綠——**修法自己造出下一個 bug**，而且是
                # 「閘門翻不成紅」這一種。Rust 那半截沒有這個問題：`format!` 的
                # 參數本來就在引號外面。
                if close == "`" and source[i : i + 2] == "${":
                    depth, hole = 0, i
                    while i < n:
                        if source[i] == "{":
                            depth += 1
                        elif source[i] == "}":
                            depth -= 1
                            if depth == 0:
                                i += 1
                                break
                        i += 1
                    holes.append((hole, i))
                    continue
                wipe(i)
                i += 1
            i += 1
        else:
            i += 1
    masked = "".join(out)

    # ── canary ────────────────────────────────────────────────────────
    #
    # 抹完之後，每一對引號中間**只該剩空白**——內容都被 wipe 掉了。中間還留著
    # 字，就代表剛才某個地方沒收好，把一整段**程式碼**當成字串吞了進去。那正是
    # 生字串那個洞的形狀，而它的後果是「一個一定會紅的違規，一個字都沒印」。
    #
    # 這一條的意義不在於它認得哪幾種寫法，而在於**它不必認得**：下一種沒想到的
    # 語法一樣會在這裡撞牆，而不是安靜地放行。
    #
    # `${…}` 那幾段是刻意留著的（裡面是程式碼，欄位就藏在那裡），所以跳過。
    for m in re.finditer(r'"[^"]*"', masked):
        if any(a <= m.start() < b for a, b in holes):
            continue
        if m.group(0)[1:-1].strip():
            line = masked.count("\n", 0, m.start()) + 1
            die(
                f"{rel}:{line} 這支腳本把一段程式碼當成字串吞掉了",
                " ".join(m.group(0).split())[:120],
                "抹過的字串裡面只該剩空白。剩下別的就是剛剛某個字面量沒收好，",
                "而它的後果是真的違規安靜地變綠——先把 mask() 教會再說。",
            )
            return None
    return masked


def calls(source, scan, names):
    """把 `format!(…)` 這種呼叫整個抓出來，**跨行**，括號配對。

    `scan` 是 `source` 的等長副本，裡面的字串字面量已經被抹成空白——括號
    配對看 `scan`，切出來的文字取自 `source`。跨行是重點：`cargo fmt` 會把
    一句中文的 `format!` 拆成三行，而上一版的 grep 是一行一行看的。

    回傳 `(行號, 起點, 原文, 抹過的文)`。起點帶出去是因為呼叫端要看**這一次**
    呼叫前面那一截（判 `tracing::`）——上一版用 `src.find(call)` 去回推位置，
    那會在兩處寫法一模一樣的時候指到第一處。
    """
    out = []
    pattern = re.compile(r"\b(" + "|".join(names) + r")\s*\(")
    for m in pattern.finditer(scan):
        depth, i = 0, m.end() - 1
        while i < len(scan):
            c = scan[i]
            if c == "(":
                depth += 1
            elif c == ")":
                depth -= 1
                if depth == 0:
                    break
            i += 1
        out.append(
            (
                scan.count("\n", 0, m.start()) + 1,
                m.start(),
                source[m.start() : i + 1],
                scan[m.start() : i + 1],
            )
        )
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
    scan = mask(src, rel, "rust")
    if scan is None:
        continue
    macros = ["format!", "write!", "writeln!", "push_str"]
    for line, start, call, bare in calls(src, scan, macros):
        # **這裡以前有兩道豁免，兩道都拿掉了。** 兩道都量過：拿掉之後這顆 repo
        # 上一條誤報都沒有，也就是說它們**當時保護的東西是零**，而它們各自賣得
        # 出一張免死金牌。一道保護不了任何東西、又擋得住閘門的例外，只會往一個
        # 方向動。
        #
        # 一、`if "tracing::" in head`。本意是「日誌是給開發者看的，開發者要的
        #     正是生的 accelerator」，但 `tracing::warn!` 這種巨集根本配不上
        #     `macros` 那張表，所以它從來沒有真的赦免過任何一行。而它擋得住的
        #     是：同一行前面隨手寫一句 `tracing::debug!("x");`，後面那個
        #     `format!("…{}", restored.wanted)` 當場變綠（實測）。上一版的註解
        #     還寫著「句子裡的字買不到豁免」——買不到的只有**句子裡**那一種，
        #     行首那一種照樣買得到。
        # 二、`if not CJK.search(call)`。本意是「只看給人看的話」，而它漏掉的是
        #     把句子拆成兩半：
        #         s.push_str("暫停熱鍵現在是 ");
        #         s.push_str(&view.wanted);      ← 這一句沒有中文，直接跳過
        #     湊起來還是那句叫他去按 KeyP 的話。中文在不在**這一次呼叫**裡，
        #     和那個字串最後會不會走到畫面上，是兩件事。
        #
        # 開發者真的要生的那一份的話，用 `tracing::` 的巨集——它們不在上面那張
        # 表裡，本來就不會走到這裡。
        #
        # **先把包好的那幾個拔掉**，剩下的才是漏網的。整行 `grep -v` 會被一句話
        # 裡正確的那一半赦免掉另一半。看 `bare`（字串已抹）而不是 `call`：句子
        # 裡寫著 `pretty_combo(` 幾個字不可以當成真的呼叫過。
        stripped = re.sub(r"pretty_combo\s*\([^)]*\)", "", bare)
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
JS = "apps/desktop/ui/settings.js"
js = read(JS)
if js is not None:
    if "function pretty(" not in js:
        die("settings.js 裡沒有 pretty() 了", "下面那一圈從此沒有東西可以認。")
    # 註解和字串一起抹掉，長度和換行都留著——**行號因此是真的**。上一版把註解
    # 整段刪掉再 `enumerate`，於是每一則違規報出來的行號都偏了幾十行。
    body = mask(js, JS, "js")
    if body is None:
        body = ""

    # 一次看**前後各 60 個字**，而不是一行。上一版是一行一行看的，於是一個被
    # formatter 拆成兩行的**正確**寫法
    #     `${pretty(\n  view.wanted,\n)}`
    # 會被判違規——那正是 Rust 那半截重寫掉的同一個坑，留在隔壁沒改。
    # （現在 CI 沒有 JS formatter，所以這一條是預防，不是活的 bug。）
    def around(i, j):
        before = " ".join(body[max(0, i - 60) : i].split())
        after = " ".join(body[j : j + 60].split())
        return before, after

    CMP = re.compile(r"[!=]=+$")
    # `before` 是一個 60 字的窗，而 `endswith("pretty(")` 對 `notpretty(` /
    # `xpretty(` 一律放行——任何名字結尾是 `pretty` 的函式都買得到豁免（實測
    # `${notpretty(view.wanted)}` 是綠的）。改成從**整份檔案**回頭看，而且要求
    # `pretty` 前面不是識別字元：窗切在半路的時候，`notpretty(` 會被截成
    # `pretty(`，那是同一個洞換一個入口。
    WRAP = re.compile(r"(?:^|[^\w$.])pretty\s*\(\s*$")
    for m in re.finditer(r"view\.(wanted|rejected)\b", body):
        before, after = around(m.start(), m.end())
        wrapped = WRAP.search(body[: m.start()]) is not None
        # 收尾那個逗號要放過：formatter 拆多行的時候會自己加一個
        # （`pretty(\n  view.wanted,\n)`）。不放過的話，這道閘門會在
        # **正確**的寫法上翻紅——一支在對的程式碼上紅的閘門會被關掉。
        wrapped = wrapped and re.match(r",?\s*\)", after) is not None
        compared = bool(CMP.search(before.replace(" ", ""))) or re.match(
            r"\s*[!=]=+", after
        )
        if wrapped or compared:
            continue
        line = body.count("\n", 0, m.start()) + 1
        die(
            f"settings.js:{line} 那個欄位既不是拿去比較、也沒有包在 pretty() 裡",
            js.split("\n")[line - 1].strip()[:160],
            "包一層 pretty(…)。要是真的需要生的那一份，先想清楚它會不會走到畫面上。",
        )

    # 白名單認的是 `view.` 這幾個字，所以把它拆開就繞過去了。
    #
    # 上一版禁的是**兩種寫法**（解構、`const x = view`），而要禁的是**一件事**：
    # 把那個物件從 `view.` 這個形狀裡帶走。稽核當場示範了兩條漏的：
    #
    #     view["wanted"]                     ← 中括號；而且 `"wanted"` 已經被抹成空白
    #     function raw(v){return v.wanted;}  ← 換一個**參數**名，不是換變數名
    #     ${raw(view)}
    #
    # 所以改成一條，而且是正面表列：**`view` 後面永遠要接一個 `.`**。唯一的
    # 例外是它自己的參數宣告。這一顆物件在 settings.js 裡只活在 `paintHotkey`
    # 裡面，每一個用法都是 `view.<欄位>`，所以這條規則沒有第二種合法情形——
    # 真的需要別的拿法，就是應該被看見的時候。
    DECL = "function paintHotkey("
    for m in re.finditer(r"\bview\b", body):
        if body[: m.start()].endswith(DECL):
            continue
        tail = body[m.end() :].lstrip()
        if tail.startswith("."):
            continue
        line = body.count("\n", 0, m.start()) + 1
        die(
            f"settings.js:{line} 把 view 從 `view.` 這個形狀裡帶走了",
            js.split("\n")[line - 1].strip()[:160],
            "解構、中括號、傳給另一個函式——三種都讓上面那道白名單認不出來。",
            "那兩個欄位只有一個合法的拿法：pretty(view.wanted)。",
        )

print()
if failed:
    sys.exit(1)
print("✓ 沒有任何一句給人看的話會叫他去按 KeyP")
