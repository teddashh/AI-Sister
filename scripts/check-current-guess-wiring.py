#!/usr/bin/env python3
"""「這一刻」那張卡在桌面那半的接線，還接著沒有。

**這支腳本守的東西在 r25 換了一半，因為縫換了地方。**

r25 之前：判斷順序整個住在 `memory_current_guess`（`apps/desktop/src-tauri/
src/main.rs`）裡，一連串 `if … return CurrentGuess::X`。那個順序是產品邏輯，
而它在一個**永遠不會被 `cargo test` 執行到**的 workspace 裡（根 `Cargo.toml`
的 `members = ["crates/*"]` 碰不到 `apps/`；`.github/workflows/ci.yml` 的
「字母人 — lint and build」那一步對它只跑 `cargo clippy --all-targets -- -D
warnings` 和 `cargo build --release`——**沒有 `cargo test`，也沒有獨立的
`cargo check` 步驟**）。所以這支腳本當時必須連順序一起守。

r25 之後：順序搬進 `crates/sister-core/src/brain.rs` 的 `CurrentGuess::decide`，
由 `crates/sister-core/tests/current_guess_order.rs` 釘住。**順序不再需要這支
腳本守。**

留給這支腳本的是搬不走的那一半，而它有**三塊**，不是一塊：

  (a) **餵進 `decide` 的輸入**——`presence`、`paused`、`consented` 都是在這個
      零覆蓋的 workspace 裡算出來的。順序搬走了，決定順序走哪一條的**輸入**
      沒有。`decide(presence, false, …)` 會讓暫停那張卡永遠不出現，而
      `cargo test --workspace` 全綠：那些測試是拿 `paused` 當**輸入參數**
      在測 `decide` 內部，測不到 `main.rs` 怎麼餵它。
  (b) **去資料庫撈那六個欄位**的那一段（綁著 tauri 的 `State` 和
      `with_db_mut`，搬不進 `crates/`）。
  (c) **`decide` 回來之後的那五行**——把回傳值攤成 `CurrentGuessView`。
      這塊是對抗式稽核逼出來的，而它指出的不是某一刀，是**分布**：這支腳本
      的第一版和它那 22 刀驗收，全部落在 `let presence` 到 `RecordingFacts`
      之間，也就是 `decide` **之前**；`decide` 之後那五行零檢查、零刀，而
      **使用者唯一讀得到的欄位（`message`）就在那裡**。八刀實測，八刀全溜。

於是這裡守 A–D 四塊（**全部只在 `fn memory_current_guess` 的函式本體裡找**——
`let presence = …` 在整個 `main.rs` 有四處，只有這一處是這張卡的）：

  A. 用 segment 當 key 的查詢，引數逐字對得上。兩支現在的形狀**一模一樣**，
     而且它們是在 r28 **合流**的，不是分岔的——r28 之前兩支都是「起點嚴格相等」
     的單引數查詢，r28 把兩支都改成了半開時間範圍（原本這裡寫「兩支的形狀不
     一樣，這一點是 r28 之後才分岔的」，兩個宣稱都是反的，而底下那兩行自己就
     把「一整段的時間範圍（半開）」寫了兩次）：
       - `l2_versions_for_chapter(seg.core_started_at, seg.core_ended_at)` —— 一整段的
         **時間範圍**（半開），先找該挑的那條活著的卡片脈絡，再讀那條脈絡的版本史。
       - `retained_interpreter_attempts_for_segment(seg.core_started_at,
         seg.core_ended_at)` —— 一整段的**時間範圍**（半開）。
     兩支改成範圍的理由是同一個：使用者合併章節之後，右半那些列記著的鍵就對不上
     任何一段了，而一列都沒有被刪。
     兩支都是把 `core_started_at` 換成 `core_ended_at` 一定查到 0 列 → 卡片安靜地
     消失或退回「正在等解釋層處理」。型別一模一樣，clippy 不會叫。
     **這一刀我實跑過，26 條閘門全綠。**
     r28 起這一段還會擋「範圍那支少傳第二個引數」：窗寬塌掉就是把那個少算 bug
     放回來。

  B. 順序沒有偷偷搬回來，**而且餵給它的三個輸入是真的算出來的**：
     不可以出現 `from_presence(` / `while_recording(`；`decide(` 要剛好一次，
     前兩個引數逐字是 `presence` 和 `paused`；`presence` / `paused` /
     `consented` 三個變數各自要從 `heartbeat::presence(` /
     `pause::is_paused(` / `consent::load(…).cloud_permit().is_some()`
     算出來，右手邊不准有 `!`。

     `consented` 是後補的：第一版只掛了前兩個，八個輸入裡的兩個。而沒掛的
     那個代價最高——`is_some()` 翻成 `is_none()`，他簽完第二張同意書之後
     畫面還是說「尚未簽署」，他回去再簽一次，還是那句；而**沒**簽的時候
     那一格反而不講同意書，把真正的關卡藏起來。

  C. 交給 `decide` 的那份 `RecordingFacts`，每個欄位的值**含有它該來的那個
     來源的字**（`FIELD_SOURCES`），**包含 `latest_closed` 裡面那層
     `LatestClosedSegment`**。`previous_attempts: None` 或 `has_card: false`
     這種改法會讓功能整個消失，而 21 個 binary 全綠——`decide` 收到什麼就照著
     算，它分不出這個 `None` 是「真的沒有」還是「有人寫死的」。

     例外是 `latest_closed: None` 那一份：沒有已關閉的段落時，`while_recording`
     的第一道檢查就回 `NoSegment`，其餘五個欄位一個都不會被讀到，所以那裡
     寫死是死資料不是謊話。這個性質由
     `without_a_closed_segment_none_of_the_other_five_arguments_can_change_the_answer`
     釘住（#144 正在討論重排早退，排序一動那個豁免就不成立了，那條測試會紅）。

  D. `decide` 回來之後那五行：`CurrentGuessView` 的三個欄位一樣走
     `FIELD_SOURCES`（`message` 必須是 `status.message()`），而且 `decide(…)`
     的結果後面接的是 `?` 不是 `unwrap_or`。`message` 釘死成
     `CurrentGuess::Queued.message()` 會讓那 15 句話塌成最讓人安心的一句；
     換成 `String::new()` 則因為前端的 `??` 只接 nullish，畫面會是標題底下
     一片空白，連 fallback 那句都不出現。兩種都 21 個 binary 全綠。

**為什麼是 python 而不是 grep。** `cargo fmt` 會把這些呼叫拆成好幾行，
一行一行看的 grep 會因此從紅變綠——一個被例行動作關掉的閘門比沒有更糟。
這裡先把整個檔案的空白壓成單一空格再比對，行怎麼拆都不影響。

**這支腳本守不住的那一半。**

先講第一版這份清單錯在哪，因為那個錯法比清單本身重要：**它列的五條全部是
「已檢查的那幾行裡，精度不夠」，而真正的缺口是「有幾行根本沒被讀到」。**
清單越誠實，越容易讓人以為邊界已經畫完了——五條「擋不住」讀起來像一份完整
的免責聲明，實際上它連 `decide` 回來之後那五行都沒提，而那裡是使用者唯一
讀得到的欄位。對抗式稽核八刀，八刀全溜。所以下面分成兩種，不要混：

  【第一種：讀到了，但認不出來】

  - A 認的是**引數的字面**，所以「`seg` 這個變數綁錯段落」它看不見：少一個
    `segments.pop()`，`seg` 就變成還開著的那一段，而 `seg.core_started_at`
    這幾個字一個都沒變 → 綠。（反過來，把 key 先存進別的變數再傳**是擋得住的**，
    因為那幾個字不再逐字相等。我原本把這一條寫進「擋不住」，實跑才發現寫錯了。）
  - B 認**函式名**，不認引數：`pause::is_paused(別的資料夾)` 是綠的（實測）。
  - C/D 認的是「值裡有沒有那個來源的字」，所以**名字含著正確來源當子字串**的
    另一個欄位是綠的：`daily_budget: config.brain.reviewer_daily_budget`
    照樣通過，因為 `reviewer_daily_budget` 裡面就有 `daily_budget`（實測）。
    「同一個字」和「同一個東西」不是一回事。
  - 它不檢查那個 closure 裡**其餘**的運算：`let worth = !worth_interpreting(…)`
    被翻面、`versions.last()` 改成 `first()`——這兩種它都看不見（實測）。

  【第二種：根本沒讀到那一行】

  - 這一版把讀取範圍延到 `Ok(CurrentGuessView { … })`，第一版停在
    `RecordingFacts`。**在那之前我不知道自己漏了什麼，現在也不知道**——
    上面那份「認不出來」是列得完的，這一種列不完。唯一能做的是每次改這支
    腳本都重問一次「這支函式從第一行到最後一行，哪幾行沒有人碰」。
  - 它證明不了那條路真的會**被走到**——那要靠桌面自己的測試，而那正是沒有
    的東西。**這一條沒有、也不可能有一刀 want=0 去證明**：一支文字掃描腳本
    沒辦法用突變示範「這段碼會不會被執行」。它和上面幾條不同類，別把它算進
    「已量測」那一疊。

  `/home/ted-h/tmp-tests/validate-gate-r25.py` 有 31 刀（26 want=1、5 want=0）
  ＋ 一項「跑一次 `cargo fmt` 不准把閘門弄紅」。**第一種**的每一條至少有一刀
  `want=0` 實跑在證明它，正向的每一條也各有一刀 `want=1`。反向的「抓不到」我
  以前從來沒驗過，而 doc 太保守和太樂觀一樣會讓下一輪做錯決定——第一版就有
  一條（「key 先存進變數」）把**擋得住**的寫成了擋不住。
  那支腳本在 repo 外面（`/home/ted-h/tmp-tests/`），clone 下來的人打不開，
  也沒有任何東西會為它變紅；它是我的驗收紀錄，不是這個 repo 的一部分。

找不到檔案、找不到那個函式、找不到那些呼叫、剖不動，一律**非零退出**。
回報 0 的時候我分不出「真的沒問題」和「我問錯了」，所以每一條都要吵。
"""

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
MAIN = ROOT / "apps/desktop/src-tauri/src/main.rs"
FN = "fn memory_current_guess"

if not MAIN.exists():
    print(f"✗ 找不到 {MAIN}——路徑改了就要改這支腳本，不可以安靜地過")
    sys.exit(2)

whole = re.sub(r"\s+", " ", MAIN.read_text(encoding="utf-8"))

problems = []

# 每個欄位的值**必須含有**這些片語。
#
# 第一版這裡是一張「什麼算常值」的黑名單正則
# （`^(None|true|false|\d+|"…"|Default::default\(\))$`），而黑名單會輸：
# 對抗式稽核當場示範 `Option::None`、`80u32`、`0_u32`、`u32::MIN`、
# `bool::default()` 五種拼法全部繞過，**而我那 14 刀 want=1 只用了正則剛好
# 認得的那一種**——等於用寫閘門的同一顆腦袋去驗那個閘門。而且根 workspace
# 的 `cargo fmt --check` 碰不到 `apps/desktop`，rustfmt 不會替你把 `0u32`
# 正規化回 `0`。
#
# 白名單問的是另一個問題：「這個值是不是**從它該來的地方**來的」。
# 這一招同時擋掉三族：寫死成任何拼法的常值（值裡沒有那個來源字）、
# 兩個同型別欄位對調（對調之後兩邊都不含對方的來源字）、
# 以及 `is_some()` 被翻成 `is_none()`。
FIELD_SOURCES = {
    "RecordingFacts": {
        "has_command": ("cli(", "is_some()"),
        "consented": ("consented",),
        "used_today": ("used",),
        "daily_budget": ("daily_budget",),
        "previous_attempts": ("previous_attempts",),
    },
    "LatestClosedSegment": {
        "has_card": ("card", "is_some()"),
        "worth_interpreting": ("worth",),
    },
    # 這一份是**使用者唯一讀得到的東西**。`timeline.js` 只讀 `message` 和
    # `card`（`status` 序列化過去沒人讀），而 `message` 就是那 15 句話的出口。
    # 釘死成某一格（`CurrentGuess::Queued.message()`）會讓分類法整個塌成一句
    # 話，而且是最讓人安心的那一句；換成 `String::new()` 則因為 `??` 只接
    # nullish，畫面會是標題底下一段空白，連 fallback 那句都不出現。
    "CurrentGuessView": {
        "message": ("status.message()",),
        "status": ("status",),
        "card": ("card",),
    },
}


def balanced_span(text, start, opener, closer):
    depth = 0
    for i in range(start, len(text)):
        if text[i] == opener:
            depth += 1
        elif text[i] == closer:
            depth -= 1
            if depth == 0:
                return i
    return None


# ── 先把範圍縮到那支函式的本體 ────────────────────────────────────────────
at = whole.find(FN)
if at == -1:
    print(f"✗ 在 {MAIN} 裡找不到 `{FN}`——函式改名了就要改這支腳本，不可以安靜地過")
    sys.exit(2)
body_open = whole.find("{", at)
body_end = balanced_span(whole, body_open, "{", "}") if body_open != -1 else None
if body_end is None:
    print(f"✗ 剖不出 `{FN}` 的函式本體（大括號沒配對），當作紅的")
    sys.exit(2)
flat = whole[body_open : body_end + 1]


def split_top_level(text):
    out, depth, cur = [], 0, ""
    for ch in text:
        if ch in "([{":
            depth += 1
        elif ch in ")]}":
            depth -= 1
        if ch == "," and depth == 0:
            out.append(cur.strip())
            cur = ""
        else:
            cur += ch
    if cur.strip():
        out.append(cur.strip())
    return out


def leading_args(text, open_paren, want):
    """只剖前 `want` 個引數就停。

    第三個引數是一整個 closure（幾十行、帶巢狀大括號與 `format!("{e:#}")`），
    剖它沒必要也不穩。前兩個是單純的識別字，掃到第 `want` 個逗號就收工。
    """
    args, depth, cur = [], 0, ""
    for i in range(open_paren, len(text)):
        ch = text[i]
        if ch == "(":
            depth += 1
            if depth == 1:
                continue
        elif ch == ")":
            depth -= 1
            if depth == 0:
                args.append(cur.strip())
                return args
        if ch == "," and depth == 1:
            args.append(cur.strip())
            cur = ""
            if len(args) == want:
                return args
            continue
        cur += ch
    return None


# ── A. 用 segment 當 key 的查詢，key 必須是 core 的時間欄位 ─────────────
for query, expected in (
    (
        "retained_interpreter_attempts_for_segment",
        "seg.core_started_at, seg.core_ended_at",
    ),
    (
        "l2_versions_for_chapter",
        "seg.core_started_at, seg.core_ended_at",
    ),
):
    calls = re.findall(re.escape(query) + r" *\( *([^)]*)\)", flat)
    if not calls:
        problems.append(f"完全找不到 {query}( 的呼叫——那張卡少了一塊資料。")
    elif len(calls) != 1:
        problems.append(f"{query}( 出現 {len(calls)} 次，這支腳本只認得剛好一次。")
    else:
        # rustfmt 把呼叫拆行時會留一個尾逗號，壓平之後就是 `<那支的引數列>,`
        # （範圍那支是 `seg.core_started_at, seg.core_ended_at,`）。
        # 第一版在這裡用完全相等去比，於是跑一次 `cargo fmt` 就會把閘門弄紅。
        arg = calls[0].strip().rstrip(",").strip()
        if arg != expected:
            problems.append(
                f"{query}( 的引數是 `{arg}`，不是 `{expected}`。\n"
                f"      查錯 core 時間欄位會安靜地回 0 列或算錯範圍。"
            )

# ── B. 順序住在 crates/，而且餵進去的兩個引數是真的算出來的 ──────────────
for leaked in ("from_presence", "while_recording"):
    n = len(re.findall(r"\b" + leaked + r" *\(", flat))
    if n:
        problems.append(
            f"{FN} 裡有 {n} 處 {leaked}( 的呼叫。判斷順序在 r25 已經搬進\n"
            f"      `CurrentGuess::decide`，由 crates/ 那邊的測試釘住。在這裡自己\n"
            f"      再叫一次，等於把那一段邏輯搬回**不會被 cargo test 執行**的地方。"
        )

decide_calls = list(re.finditer(r"CurrentGuess *:: *decide *\(", flat))
if len(decide_calls) != 1:
    problems.append(
        f"CurrentGuess::decide( 在 {FN} 出現 {len(decide_calls)} 次，應該剛好 1 次。\n"
        f"      0 次＝有人把判斷順序搬回這個零覆蓋的 workspace 了。"
    )
else:
    args = leading_args(flat, flat.index("(", decide_calls[0].start()), 2)
    if not args or len(args) < 2:
        problems.append("剖不出 CurrentGuess::decide( 的前兩個引數，當作紅的。")
    else:
        for i, want in enumerate(("presence", "paused")):
            if args[i].strip() != want:
                problems.append(
                    f"CurrentGuess::decide( 的第 {i + 1} 個引數是 `{args[i].strip()}`，"
                    f"不是 `{want}`。\n"
                    f"      順序搬進 crates/ 了，但**餵給順序的輸入**還在這個零覆蓋的\n"
                    f"      workspace 裡。寫死成 `false` 的話，暫停那張卡永遠不會出現，\n"
                    f"      而 21 個 binary 全綠。"
                )

# 三個在這個零覆蓋的 workspace 裡算出來、然後餵給判斷順序的值。
#
# `consented` 是後來補的：第一版只掛了 `presence` 和 `paused`，八個輸入裡的
# 兩個。對抗式稽核指出這個不對稱「不像取捨，像是寫到一半停了」——而沒掛的
# 那個代價最高：`cloud_permit().is_some()` 翻成 `.is_none()`，他簽完第二張
# 同意書之後畫面還是說「尚未簽署」，他回去再簽一次，還是那句；而**沒**簽的
# 時候那一格反而不講同意書，把真正的關卡藏起來。
for var, needles, human in (
    ("presence", ("heartbeat::presence(",), "heartbeat::presence("),
    ("paused", ("pause::is_paused(",), "pause::is_paused("),
    (
        "consented",
        ("consent::load(", "cloud_permit()", "is_some()"),
        "consent::load(…).cloud_permit().is_some()",
    ),
):
    m = list(re.finditer(r"let " + var + r" = ([^;]*);", flat))
    if len(m) != 1:
        problems.append(f"`let {var} = …;` 在 {FN} 出現 {len(m)} 次，應該剛好 1 次。")
        continue
    rhs = m[0].group(1).strip()
    # 壓掉路徑上的空白，`sister_core :: consent :: load (` 也要認得
    tight = re.sub(r"\s*::\s*", "::", rhs).replace("( ", "(").replace(" )", ")")
    missing = [n for n in needles if n not in tight]
    if missing:
        problems.append(
            f"`let {var} = {rhs};` 的右手邊少了 {'、'.join(f'`{n}`' for n in missing)}，"
            f"不是從 {human} 算出來的。"
        )
    if rhs.startswith("!"):
        problems.append(
            f"`let {var} = {rhs};` 被翻面了。這一個 `!` 會讓那張卡對\n"
            f"      「她現在有沒有在看你」說反話，而沒有任何測試看得到。"
        )


# ── C. 每個欄位的值都必須從它該來的地方來 ────────────────────────────────
def check_struct(name, some_key=None):
    """`name { … }` 的每個欄位，值裡必須含有 FIELD_SOURCES 指定的片語。"""
    expected = FIELD_SOURCES[name]
    found, wrong, saw_some = 0, [], False
    idx = flat.find(name + " {")
    while idx != -1:
        brace = flat.index("{", idx)
        end = balanced_span(flat, brace, "{", "}")
        if end is None:
            problems.append(f"{name} {{ 的大括號沒有收——這支腳本剖不動，當作紅的。")
            return found, wrong, saw_some
        found += 1
        fields = {}
        for piece in split_top_level(flat[brace + 1 : end]):
            if ":" in piece:
                k, v = piece.split(":", 1)
                fields[k.strip()] = v.strip()
            else:
                fields[piece.strip()] = piece.strip()  # 簡寫 `previous_attempts,`

        skip_rest = False
        if some_key:
            latest = fields.get(some_key, "")
            if latest.startswith("Some"):
                saw_some = True
            elif latest == "None":
                skip_rest = True  # 死資料，豁免（理由見上面 C）
            else:
                problems.append(
                    f"{name} 的 {some_key} 是 `{latest}`，這支腳本只認得\n"
                    f"      `Some(...)` 或 `None` 兩種——剖不動就當作紅的。"
                )
        if not skip_rest:
            for k, needles in expected.items():
                if k not in fields:
                    wrong.append(f"{name}.{k} 整個不見了")
                    continue
                v = fields[k]
                tight = v.replace("( ", "(").replace(" )", ")").replace(" .", ".")
                missing = [n for n in needles if n not in tight]
                if missing:
                    wrong.append(
                        f"{name}.{k}: {v}"
                        f"（少了 {'、'.join(chr(96) + n + chr(96) for n in missing)}）"
                    )
        idx = flat.find(name + " {", end)
    return found, wrong, saw_some


n_facts, wrong_facts, saw_some = check_struct("RecordingFacts", "latest_closed")
n_seg, wrong_seg, _ = check_struct("LatestClosedSegment")

if n_facts == 0:
    problems.append("找不到 RecordingFacts { 的建構——交給 decide 的那份資料不見了？")
if n_seg == 0:
    problems.append(
        "找不到 LatestClosedSegment { 的建構——`has_card` / `worth_interpreting`\n"
        "      是那張卡最要命的兩個輸入，不可以沒有人組。"
    )
if n_facts and not saw_some:
    problems.append(
        "每一份 RecordingFacts 的 latest_closed 都是 None——那張卡永遠只會說\n"
        "      「正在錄，但這一刻還沒有任何段落。」，其餘每一句話都到不了。"
    )

# ── D. decide 回來之後那五行：使用者唯一讀得到的東西 ──────────────────────
#
# 這一段是對抗式稽核加的，而它指出的**不是某一刀，是分布**：第一版的檢查和
# 我那 22 刀驗收全部落在 `let presence` 到 `RecordingFacts` 之間，而
# `decide` 回來之後把結果攤成 `CurrentGuessView` 的那五行——**使用者唯一讀
# 得到的欄位就在那裡**——零檢查、零刀。實測八刀全部溜過去。
n_view, wrong_view, _ = check_struct("CurrentGuessView")
if n_view != 1:
    problems.append(
        f"CurrentGuessView {{ 在 {FN} 出現 {n_view} 次，應該剛好 1 次。"
    )

if wrong_facts or wrong_seg or wrong_view:
    problems.append(
        "底下這些欄位的值**不是從它該來的地方來的**：\n"
        + "".join(f"        {x}\n" for x in wrong_facts + wrong_seg + wrong_view)
        + "      decide 收到什麼就照著算，它分不出寫死的值和真的撈出來的值；\n"
        + "      而 message 那一格是那 15 句話唯一的出口。這幾種改法會讓功能\n"
        + "      整個消失或整個塌成一句話，而 21 個 binary 全綠。"
    )

# 錯誤不准在最後一步被吞掉。`decide` 內部把 `fetch()?` 原樣往上帶，那件事由
# `current_guess_order.rs::a_failed_read_is_not_swallowed_into_a_normal_looking_sentence`
# 釘住——但它守的是 `decide` **裡面**，而吞掉的動作可以發生在它回來之後的
# 那一個字元，也就是這個沒有 cargo test 的 workspace 裡。把 `?` 換成
# `.unwrap_or(NoSegment)`，資料庫壞掉會長成「正在錄，但這一刻還沒有任何
# 段落。」——一句完全正常的話。
#
# 只看 `)` 之後到 `CurrentGuessView` 之前那一小段，不看整個 closure 本體：
# closure 裡本來就有 `?` 和各種 `unwrap_*`，拿整段去比會兩邊都失真。
if decide_calls:
    open_paren = flat.index("(", decide_calls[0].start())
    close_paren = balanced_span(flat, open_paren, "(", ")")
    view_at = flat.find("CurrentGuessView {")
    if close_paren is None or view_at == -1 or view_at < close_paren:
        problems.append(
            "剖不出 `CurrentGuess::decide(…)` 和 `CurrentGuessView {` 的相對位置，"
            "當作紅的。"
        )
    else:
        after = flat[close_paren + 1 : view_at]
        head = after.split(";", 1)[0]  # 只看那一個運算式，別跨到下一句
        for swallow in ("unwrap_or", "unwrap_or_else", "unwrap_or_default", ".ok()"):
            if swallow in head:
                problems.append(
                    f"`CurrentGuess::decide(…)` 後面接了 `{swallow}`。\n"
                    f"      查詢失敗要往上傳，不准被吞成某一格 CurrentGuess——吞掉的話，\n"
                    f"      資料庫壞掉會長得像一句完全正常的話。"
                )
        if "?" not in head:
            problems.append(
                f"`CurrentGuess::decide(…)` 的結果沒有用 `?` 往上傳"
                f"（後面接的是 `{head.strip()[:60]}`）。\n"
                f"      這支腳本只認得 `?`；用 match 或 if let 把 Err 攤開來處理的話，\n"
                f"      它看不出你是往上傳還是吞掉——那種寫法要自己另外找人守。"
            )

# ── E. 時間軸把整章的半開範圍交給 crates/ 選卡 ────────────────────────────
#
# 這種原始碼閘門只釘得住**引數**，釘不住**結果**：即使這個呼叫原封不動，呼叫端
# 隨後又把結果濾回 `segment_core_start == ch.core_start_ts`，這條仍會是綠的。
# 行為本身要由 crates/ 的執行測試守；這裡只守 apps/ 這個零執行覆蓋 workspace 的接線。
timeline_call = re.findall(
    r"sister_core::brain::chapter_l2_views *\( *([^)]*)\)", whole
)
if len(timeline_call) != 1:
    problems.append(
        "sister_core::brain::chapter_l2_views( 在 main.rs 應該剛好出現 1 次，"
        f"實際是 {len(timeline_call)} 次。"
    )
else:
    args = timeline_call[0].strip().rstrip(",").strip()
    if args != "cards, ch.core_start_ts, ch.core_end_ts":
        problems.append(
            "chapter_l2_views( 的引數是 `"
            + args
            + "`，不是 `cards, ch.core_start_ts, ch.core_end_ts`。"
        )

# ── F. 「前一版是誰」這個決定只准住在 crates/ ────────────────────────────
#
# 這一條是量出來才加的。r27 修好之後，我把 `memory_current_guess` 那一行原封
# 不動改回舊的 `versions.get(versions.len().saturating_sub(2))`，然後跑：
#
#     cargo test --workspace          → 23 個 test binary 全綠
#     check-current-guess-wiring.py   → ✓
#
# 一個都沒紅。原因是 `apps/desktop` 是另一個 workspace（根 Cargo.toml 的
# `members = ["crates/*"]` 不含它），`cargo test --workspace` 一行都執行不到
# 它；而 crates/ 那邊的測試釘的是 `latest_with_previous` 這支函式**本身**，
# 呼叫端有沒有用它，那些測試看不見。
#
# 所以這一格只能由原始碼形狀來守——它是唯一搆得到那半的東西。
#
# ⚠ 而「唯一搆得到」不等於「守得住」。下面這四刀我全部真的打過，閘門一聲都
#   沒響（對照組「把 helper 拆回 saturating_sub(2)」是紅的，所以它不是壞的，
#   是**淺的**）：
#
#     1. view_from_row_with_previous(row, previous.or_else(|| versions.first()))
#        —— helper 有呼叫、計數是 3，但它算出來的答案在下一格被蓋掉。
#           r27 修掉的那個 bug 一字不差地回來，而且讀起來像「防禦性補值」。
#     2. saturating_sub( 2 ) —— 括號內側加空白。下面壓空白只做了
#        re.sub(r"\s+", " ")，沒做 B/C 兩塊都有做的 .replace("( ", "(")。
#     3. versions.len().max(2) - 2 —— 語意等同舊 bug，三根針逐字比對都不中。
#     4. attach_l2 的 sort_by_key 改成 Reverse(...) —— A-D 只掃
#        `fn memory_current_guess` 的函式本體，attach_l2 整段在掃描範圍外。
#
#   結論：字串比對守得住「這個決定被整個抄走」，守不住「答案被算出來又丟掉」。
#   不要因為這一段印 ✓ 就相信呼叫端是對的。
PREV_HELPER = "sister_core::brain::latest_with_previous"
WANT_HELPER_CALLS = 2  # memory_guesses、memory_current_guess；attach_l2 已搬進 crates/
n_helper = whole.count(PREV_HELPER)
if n_helper != WANT_HELPER_CALLS:
    problems.append(
        f"`{PREV_HELPER}` 在 {MAIN} 裡出現 {n_helper} 次，預期 {WANT_HELPER_CALLS} 次。\n"
        f"      預期的兩處是 memory_guesses、memory_current_guess。\n"
        f"      注意這裡數的是**整份檔案裡這串字出現幾次**，不是「那三支函式各有一次」\n"
        f"      ——註解或字串裡寫到它一樣會被數進來。多一處少一處都請先改這支腳本的\n"
        f"      WANT_HELPER_CALLS，順便想一下新的那一處是不是又抄了一份「前一版是誰」。"
    )

# 手算下標的三種寫法一種都不准回來。這正是 r27 之前的 bug：只有一版的時候
# `1usize.saturating_sub(2)` 是 0，`get(0)` 拿到的就是最新那一版自己，畫面上
# 就變成「原版：<和它正上方一模一樣的那句話>」。
#
# main.rs 裡另外兩處 `saturating_sub` 是時間相減（:199、:235），不帶 `(2)`，
# 所以這三根針不會誤傷它們。
for shape in ("saturating_sub(2)", "len() - 2", "len()-2"):
    if shape in whole:
        problems.append(
            f"{MAIN} 裡出現 `{shape}`——「前一版是誰」又被手算了一次。\n"
            f"      只有一版的時候這個算式會指回最新那一版自己。改用\n"
            f"      `{PREV_HELPER}`，那支函式在 crates/ 裡有測試守著。"
        )

if problems:
    print("✗ 「這一刻」那張卡在桌面那半的接線斷了：")
    for p in problems:
        print(f"    - {p}")
    sys.exit(1)

print(
    "✓ 「這一刻」的桌面接線完整（A 卡片與次數都查 "
    "[core_started_at, core_ended_at)／B 順序只在 decide 裡、"
    "三個輸入是算出來的／C 欄位有來源／D 回來之後 message 是 status.message()、"
    "錯誤用 ? 往上傳／E 前一版走 latest_with_previous，沒有人手算下標）"
)
