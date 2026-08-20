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

**這支腳本守不住的那一半，寫在這裡。** 隔天的對抗式稽核指出，第一版的 Python
還有四個洞，三個已經補掉（見 `mask` 和底下畫面那一段的註解），剩下這一個補不
掉：

    let w = restored.wanted.clone();
    format!("還在用 {w}。")

那個欄位在 `format!` 外面就被抄走了，而這支腳本掃的是 `format!(…)` 括號裡面。
Rust 這邊沒辦法像畫面那邊一樣改用白名單——`.clone()` / `.is_empty()` /
`rejected: Some(view.wanted)` 三種合法用法都要放行，而 `.clone()` 正是這個洞
走的那一條。真正堵得住的只有一個 `Combo` newtype（讓生字串根本 `Display` 不
出來），而那是一次跨 crate 的改動。

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


def mask(source, rel, quotes='"'):
    """把每一段字串和註解的**內容**換成同長度的空白，換行留著。

    **同長度**是重點：換完之後每一個位移都還對得回原檔，所以行號是真的。
    上一版是 `re.sub(r"/\\*.*?\\*/", "", js, flags=re.S)`——它把註解裡的換行
    一起吃掉，於是底下 `enumerate` 出來的行號和 `settings.js` 差了幾十行。
    實測：真的違規在 `:358`，閘門指著 `:289`，而 `:289` 是
    `function pretty(combo) {`——**它指著自己叫你去呼叫的那支函式**。

    字串也要抹，理由是括號配對器不認得字串：一句話裡出現一個半形 `)`（顏文字
    `:)`、或「（見下）」寫成半形），就會被當成呼叫的結尾，那一句後面剩下的
    整段逃掉。實測 `format!("還在用 {}。 :)", restored.wanted)` 在這之前是綠的。

    生字串（`r"…"` / `r#"…"#`）這裡讀不了，遇到就當場算輸，不是猜。一個讀錯
    的掃描器會安靜地放行，而那正是這支腳本在抓的東西。
    """
    if re.search(r'\br#+"', source):
        die(
            f'{rel} 裡有生字串（r#"…"#），這支腳本讀不了',
            "在把它教會之前，別讓它裝作看得懂——它會安靜地放行。",
        )
        return None
    out = list(source)
    i, n = 0, len(source)

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
                    depth, i = 0, i
                    while i < n:
                        if source[i] == "{":
                            depth += 1
                        elif source[i] == "}":
                            depth -= 1
                            if depth == 0:
                                i += 1
                                break
                        i += 1
                    continue
                wipe(i)
                i += 1
            i += 1
        else:
            i += 1
    return "".join(out)


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
    scan = mask(src, rel)
    if scan is None:
        continue
    macros = ["format!", "write!", "writeln!", "push_str"]
    for line, start, call, bare in calls(src, scan, macros):
        if not CJK.search(call):
            continue
        # 日誌巨集自己就長成 `format!` 的樣子，用**它前面那一截**認出來。
        #
        # 這裡以前還問了 `"tracing::" in call`，而那是一個豁免權賣給任何人的洞：
        # 句子本身寫著 `tracing::` 就免死。實測
        # `format!("還在用 {}。（tracing:: 也記了一份）", restored.wanted)` 是綠的。
        # 現在只看前綴，而且看的是抹掉字串之後的 `scan`——句子裡的字買不到豁免。
        head = scan[:start].rsplit("\n", 1)[-1]
        if "tracing::" in head:
            continue
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
    body = mask(js, JS, quotes="\"'`")
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
    for m in re.finditer(r"view\.(wanted|rejected)\b", body):
        before, after = around(m.start(), m.end())
        wrapped = before.endswith("pretty(") or before.endswith("pretty (")
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

    # 白名單認的是 `view.` 這幾個字，所以把它拆開就繞過去了。這兩種都實測綠過：
    #     const { wanted } = view;   →  之後插值 `${wanted}`
    #     const v = view;            →  之後 `v.wanted`
    # 那個欄位只有一個合法的拿法，所以直接把「換一個名字拿」整個禁掉。
    for pat, why in (
        (r"\{[^{}]*\b(wanted|rejected)\b[^{}]*\}\s*=\s*view\b", "解構出來就沒有 `view.` 了"),
        (r"\b(?:const|let|var)\s+[A-Za-z_$][\w$]*\s*=\s*view\s*[;,\n]", "換個名字拿也一樣"),
    ):
        for m in re.finditer(pat, body):
            line = body.count("\n", 0, m.start()) + 1
            die(
                f"settings.js:{line} 把 view 拆開拿，繞過了上面那道白名單",
                js.split("\n")[line - 1].strip()[:160],
                f"{why}——那兩個欄位只有一個合法的拿法：pretty(view.wanted)。",
            )

print()
if failed:
    sys.exit(1)
print("✓ 沒有任何一句給人看的話會叫他去按 KeyP")
