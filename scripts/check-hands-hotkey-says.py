#!/usr/bin/env python3
"""拔手那顆鍵的接線層，說的話要是真的。

拔手熱鍵是**安全鍵**：按下去把她的手拔掉（寫 `data_dir/hands.stop`）。它失效
的後果比任何顯示錯誤都嚴重——他以為她停了，而她還在動。

而這顆鍵的接線全部落在 `apps/desktop/src-tauri/src/main.rs` 和
`apps/desktop/ui/settings.js`。**那是另一個 workspace**，根目錄的
`cargo test --workspace` 一行都編不到它；CI 對 `apps/desktop` 只跑
`cargo clippy` 和 `cargo build --release`（`ci.yml` 的「字母人 — lint and
build」），**從來沒有 `cargo test` 過**。所以這幾格的行為只可能由原始碼形狀的
針來守，這支腳本就是那些針。

為什麼知道需要它：`/home/ted-h/tmp-tests/mut-r33.py` 那十二刀裡有四刀打在呼叫
端，第一輪跑出來**四刀全綠**——

- 系統匣兩顆標籤對調（他按下去做的是相反的事，畫面上一個字看不出來）
- 問不出資料目錄時退回寫死的「（現在沒拔）」
- 系統匣失敗句退回作業系統的英文原文
- 設定檔壞掉時那句話不再講「現在真的在用哪一組」

四刀都不會讓任何測試紅，因為沒有任何測試編得到那個檔案。

## 這幾道針**擋不住**什麼（照著寫，不要讀成「這裡安全了」）

- 它們比的是**原始碼的形狀**，不是跑起來的行為。函式名字對、參數位置對，不代表
  那支函式回傳的字是對的——crate 那半（`kill_switch.rs` 的 `mod tests`）才守值。
- ⑥ 只問「那一格是不是可按的」和「句子有沒有叫他換一組」。句子改成另一句**同樣
  做不到**的指示，它照樣綠。
- ③ 只問那句話裡有沒有「現在」這兩個字（字面或它插進去的變數裡都算）。把「現在」
  接到一個算錯的變數上，它照樣綠。
- 這裡**每一格**都不會在 Windows 上跑出任何行為證據。這一顆鍵在真 Tauri 視窗上
  一次都沒被按過。

## 錨點對不到就要吵

底下每一圈都先確認自己抓到了東西。抓不到就 `die`，不是安靜跳過——一支印 ✓ 而
什麼都沒檢查的閘門，和沒有這支腳本長得一模一樣，但它會讓人以為有人在看。
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MAIN = ROOT / "apps/desktop/src-tauri/src/main.rs"
SJS = ROOT / "apps/desktop/ui/settings.js"
SHTML = ROOT / "apps/desktop/ui/settings.html"

CJK = r"[一-鿿]"
BAD = []
CHECKED = []


def die(what: str, why: str) -> None:
    print(f"✗ {what}\n  {why}")
    sys.exit(2)


def want(tag: str, ok: bool, why: str) -> None:
    CHECKED.append(tag)
    print(f"  {'✓' if ok else '✗'} {tag}")
    if not ok:
        print(f"      {why}")
        BAD.append(tag)


def rust_stmt_at(scope: str, at: int) -> str:
    """從 `at` 起算的一整句 Rust 敘述（右手邊帶大括號也吃得下）。"""
    depth = 0
    for j in range(at, len(scope)):
        if scope[j] == "{":
            depth += 1
        elif scope[j] == "}":
            depth -= 1
        elif scope[j] == ";" and depth == 0:
            return scope[at : j + 1]
    die("敘述沒有結尾的分號", "檔案被截斷，或錨點指到字串裡去了。")
    return ""


def rust_binding(scope: str, name: str) -> str:
    """`let <name> = …;` 整句（右手邊帶大括號也吃得下）。抓不到就當場停。

    抓不到回空字串的話，底下每一個 `not in` 都會成立，於是這一格印 ✓ 而什麼
    都沒看——所以是 `die`。
    """
    at = scope.rfind(f"let {name} = ")
    if at < 0:
        die(f"找不到 `let {name} = `", "錨點對不到的時候，底下那一格是空轉。")
    depth = 0
    for j in range(at, len(scope)):
        if scope[j] == "{":
            depth += 1
        elif scope[j] == "}":
            depth -= 1
        elif scope[j] == ";" and depth == 0:
            return scope[at : j + 1]
    die(f"`let {name} = ` 沒有結尾的分號", "檔案被截斷，或錨點指到字串裡去了。")
    return ""


def brace_body(text: str, start: int, what: str) -> str:
    """從 start 之後第一個 `{` 起做括號配對。抓不到就當場停。"""
    if start < 0:
        die(f"找不到 {what}", "錨點對不到的時候，底下那一圈是空轉，而它會印 ✓。")
    i = text.find("{", start)
    depth, j = 0, i
    while j < len(text):
        if text[j] == "{":
            depth += 1
        elif text[j] == "}":
            depth -= 1
            if depth == 0:
                return text[i : j + 1]
        j += 1
    die(f"{what} 的大括號沒有配對成功", "檔案被截斷，或者那個錨點指到字串裡去了。")
    return ""


main = MAIN.read_text("utf-8")
sjs = SJS.read_text("utf-8")
shtml = SHTML.read_text("utf-8")

print("▶ 設定頁送出去的還是 e.code")
combo_of = brace_body(sjs, sjs.find("function comboOf"), "settings.js 的 comboOf")
want(
    "① comboOf 推的是 e.code",
    "parts.push(e.code)" in combo_of,
    # `normalize_hotkey` 那支手抄別名表的函式在 alpha.96 被整支刪掉了（改用
    # `global_hotkey::hotkey::HotKey::from_str`），所以這句話不能再指著它。
    "送給後端的組合鍵形狀（`Ctrl+Alt+KeyH`）是 `hotkeys_collide` 那一族測試的\n"
    "      前提。這裡一改，`crates/sister-core/tests/exam_r33.rs`、`exam_r34.rs`\n"
    "      和 `kill_switch.rs` 裡**手寫**的那些字串就不再是產品生得出來的形狀，\n"
    "      而它們會繼續綠著——綠得沒有意義。那一天要回去重推配對，不是把它改綠。",
)

print("▶ 設定檔讀不出來的時候，換熱鍵那條路不早退")
hotkey_set = brace_body(main, main.find("fn hotkey_set("), "main.rs 的 hotkey_set")
load_at = hotkey_set.find("Config::load")
stmt = hotkey_set[load_at : hotkey_set.find(";", load_at)] if load_at >= 0 else ""
want(
    "② 讀設定檔失敗不准裸 `?` 出去",
    "?" not in stmt,
    "他按的是熱鍵。裸 `?` 會讓 `apply_hotkey` 一次都沒跑，而他讀到的是一句設定\n"
    "      檔解析錯誤——沒有一個字講他剛剛試的那組生不生效。這個 bug 在 alpha.95\n"
    "      修好過一次，r30 又把它加回它自己的正上方。",
)

says = list(re.finditer(r'return Err\(format!\(\s*"((?:[^"\\]|\\.)*)"', hotkey_set))
if not says:
    die("hotkey_set 裡一句 `return Err(format!` 都沒抓到", "正規表示式對不到，底下那一圈是空轉。")
dumb = []
for m in says:
    literal = m.group(1)
    resolved = literal
    for name in set(re.findall(r"\{(\w+)", literal)):
        # **往回找，不要 `find`。** 第一版用 `hotkey_set.find(f"let {name} = ")`，
        # 而 `hotkey_set` 裡有兩個 `let still = `（實測位移 533 和 2676）——
        # `find` 永遠回第一個，於是第二句話被第一句的變數赦免了。閘門印
        # 「看了 2 句」，實際是同一句看了兩次。每一句要用**它自己那一段**裡的
        # 定義去解。
        at = hotkey_set.rfind(f"let {name} = ", 0, m.start())
        if at >= 0:
            resolved += hotkey_set[at : m.start()]
    if "現在" not in resolved:
        dumb.append(literal)
want(
    f"③ 它吐的每一句話都講得出現在在用哪一組（看了 {len(says)} 句）",
    not dumb,
    f"沒講的：{dumb}\n"
    "      只說「出錯了」而不說現在生效的是哪一組，他讀完不知道下一步做什麼。\n"
    "      那幾個字寫在字面裡、或塞在一個變數裡，都算。",
)
unreadable_says = [m.group(1) for m in says if "設定檔讀不出來" in m.group(1)]
if len(unreadable_says) != 1:
    die("設定檔讀不出來的回話不是一格", f"抓到 {len(unreadable_says)} 格，無法確認兩顆鍵都交代。")
unreadable_at = hotkey_set.find(unreadable_says[0])
unreadable_context = hotkey_set[max(0, unreadable_at - 1800) : unreadable_at + len(unreadable_says[0])]
want(
    "③ᵇ 設定檔壞掉時同時交代暫停鍵與拔手鍵",
    "暫停鍵" in unreadable_context
    and "拔手鍵" in unreadable_context
    and "{hands_still}" in unreadable_says[0],
    "這條路會先拆掉再重裝兩顆鍵；只交代暫停鍵，安全鍵換人或搶不到都沒有一句話說。",
)

# ③ᵇ 只問「那 1800 字裡有沒有這幾個詞」，沒有任何一格把 `hands_still` 綁到
# `hands_*` 欄位上。實測把 `pretty_combo(&restored.hands_wanted)` 換成
# `pretty_combo(&restored.wanted)`，13 格全綠、Rust 全綠——而他讀到的是
# 「拔手鍵現在還在用 Ctrl + Alt + P」，按下去她的手還接著。
# 兩臂各自只准讀自己那一組欄位。
# 問的是「**印出來的組合**是不是自己那一顆」，不是「有沒有碰到對方任何欄位」：
# `still` 讀 `hands_collided` 是正當的——那是暫停鍵為什麼沒註冊的**原因**。
# 不正當的是把對方的**組合**印進自己這一句。
# `needs_collision` 只給暫停那一句：撞號的時候讓位的是它，所以它才需要一臂把
# 「讓給拔手了」和「被別的程式搶走了」分開。拔手那一句相反——撞號時它是活下來
# 的那顆，照常註冊，`hands_collided` 對它不是一個分辨條件。
for name, mine, theirs, needs_collision in [
    ("still", "wanted", ["hands_wanted", "hands_registered"], True),
    ("hands_still", "hands_wanted", ["wanted", "registered"], False),
]:
    # **每一個**同名的 `let` 都要看，不是最後那個。`hotkey_set` 裡有兩個
    # `let still = `（設定檔讀不出來一個、存不進去一個），而 `rust_binding` 用
    # `rfind`——第一版的這一格只檢查了後面那個，前面那個從來沒人看。
    # 這是我在 ③ 上犯過的同一個錯（`find` 拿第一個去替後面每一句背書），
    # 換成 `rfind` 之後又從另一頭犯了一次。
    bindings = [
        rust_stmt_at(hotkey_set, m.start())
        for m in re.finditer(rf"let {name} = ", hotkey_set)
    ]
    if not bindings:
        die(f"找不到 `let {name} = `", "錨點對不到的時候，這一格是空轉的。")
    for i, arm in enumerate(bindings, 1):
        fields = set(re.findall(r"restored\.(\w+)", arm))
        if not fields:
            die(f"第 {i} 個 `let {name} = ` 沒有讀任何 restored 欄位", "這一格是空轉的。")
        stolen = [f for f in theirs if f in fields]
        want(
            f"③ᶜ 第 {i} 個 `{name}` 印的是自己那一顆鍵的組合"
            + ("，而且分得出撞號" if needs_collision else ""),
            mine in fields
            and not stolen
            and (not needs_collision or "hands_collided" in fields),
            f"讀到了 {sorted(fields)}；不該碰的：{stolen or '（無）'}。\n"
            f"      兩臂吐的是同一種形狀的完整中文句子，拿錯欄位的話流程不垮、畫面\n"
            f"      也不怪——他就是讀到隔壁那顆鍵的組合。實測把 `hands_still` 裡的\n"
            f"      `restored.hands_wanted` 換成 `restored.wanted`，其餘 13 格全綠，\n"
            f"      而他讀到「拔手鍵現在還在用 Ctrl + Alt + P」：出事時按 P，手還接著。\n"
            f"      `hands_collided` 要有自己一臂：`wanted` 現在撞號時也有值，少了\n"
            f"      那一臂會把「讓給拔手了」講成「被別的程式搶走了」——歸因是假的，\n"
            f"      而他會去找一個不存在的程式。",
        )

print("▶ 問不出資料目錄的時候，系統匣不宣稱她拔了沒")
want(
    "④ 那一格不寫死狀態宣稱",
    "現在沒拔" not in main,
    "`data_dir == None` ＝這個行程問不出資料目錄在哪 ＝她拔沒拔我們根本不知道。\n"
    "      括號裡寫「現在沒拔」是倒向危險那一邊；同一個 None 在 `hands_execute`、\n"
    "      `kill_switch::is_pulled` 和這支函式自己上面兩行的註解裡全部倒向安全那邊。",
)
copies = len(re.findall(r'"拔掉她的手"', main))
want(
    "⑤ 那兩顆的字只有一個來源",
    copies <= 1,
    f"`main.rs` 裡有 {copies} 份寫死的「拔掉她的手」。兩份各寫一次的東西會分岔——\n"
    "      這一族抓到的第一個 bug 就是其中一份被偷偷加上了狀態宣稱。",
)

print("▶ 那兩顆按鈕各拿自己那一格的字")
pairs_ok = True
for item, var in [("HandsStopItem", "stop"), ("HandsResumeItem", "resume")]:
    at = main.find(item)
    if at < 0:
        die(f"找不到 {item}", "系統匣那兩顆的 state 名字改了，這一圈要跟著改。")
    pairs_ok = pairs_ok and f"set_text({var})" in main[at : at + 200]
menu = dict(re.findall(r'MenuItem::with_id\(\s*app,\s*"(hands-\w+)",\s*(\S+?),', main))
if not menu:
    die("抓不到系統匣那兩顆的建構", "`MenuItem::with_id` 的寫法改了，正規表示式要跟著改。")
want(
    "⑥ 拔掉／接回沒有對調",
    pairs_ok and menu == {"hands-stop": "hands_labels.0", "hands-resume": "hands_labels.1"},
    f"選單配對＝{menu}。兩顆對調的話他按下去做的是相反的事，而畫面上一個字\n"
    "      都看不出來——他以為自己把手拔了。",
)

print("▶ 拔手那一格：要嘛真的按得動，要嘛別叫他去按")
span = re.search(r"<(\w+)([^>]*data-hands-combo[^>]*)>", shtml)
if not span:
    die("settings.html 裡找不到 data-hands-combo", "那一格改名了，這一圈要跟著改。")
at = sjs.find("data-hands-combo")
head = sjs.rfind("\nfunction ", 0, at)
hands_fn = brace_body(sjs, head, "settings.js 裡碰 data-hands-combo 的那支函式")
route_a = span.group(1) == "button" and "addEventListener" in hands_fn
route_b = "換一組" not in hands_fn and 'class="combo"' not in span.group(0)
want(
    "⑦ 唯讀的那一格不准叫他「換一組」",
    route_a or route_b,
    f"現況：tag=<{span.group(1)}>、句子裡{'還有' if '換一組' in hands_fn else '沒有'}「換一組」。\n"
    "      `.combo` 是暫停那顆**可按的**鍵盤格子用的樣式。套著它、零 listener、\n"
    "      底下那行字又在叫他換一組——唯一改得動的路是手改 `config.toml`。",
)

print("▶ 系統匣那條路說中文")
arm = brace_body(main, main.find('"hands-stop" | "hands-resume" =>'), "系統匣拔手 handler")
raw = "map_err(|e| e.to_string())" in arm
# 第一版問的是「`format!` 的第一個字面量裡有沒有中文」。那條針太細：
#   format!("{}{}", tray_hands_failure_message(why), os_error.unwrap_or_default())
# 三項全過，而 OS 原文照樣長在中文句子後面。這一臂裡本來就不該有任何
# `format!`——要說的話都在 `kill_switch::` 那一族裡拼好了，log 用
# `tracing::error!` 自己的格式化（原文本來就該留在 log）。所以整個禁掉。
spliced = "format!" in arm
emitted = re.search(r'emit\(\s*"recorder-failed"\s*,\s*([^,\n]+)', arm)
speaks = bool(emitted) and "kill_switch::" in emitted.group(1)
want(
    "⑧ 作業系統的原文不進中文句子",
    not raw and not spliced and speaks,
    "`io::Error::to_string()` 是英文的，實測長成「拔手開關失敗：File exists\n"
    "      (os error 17)」。`WhyNotWritten` 的 doc 明寫這幾個字會出現在中文介面\n"
    "      上，而 `os_error` 這個欄位存在的理由就是「原文留給 log、不進句子」。\n"
    "      送進 `emit` 的字只能來自 `kill_switch::` 那一族；`tracing::error!` 不\n"
    "      在此限——原文本來就該留在 log。",
)

print("▶ 設定檔讀不出來的那一輪，安全鍵不可以在他沒看見的時候換人")
# 這四格是 alpha.96 補的。它們守的是那一版的**招牌修法**，而那些修法全落在
# `apps/desktop`——`mut-r34.py` 的 M4／M5／M6／M7 四刀，在補這幾格之前**一刀都
# 沒有人紅**（只有一次性的收貨考題咬得住，而考題不會進 CI）。
#
# 寫法上刻意問「那個值**從哪裡來**」而不是「有沒有出現某幾個字」：第一版的
# 收貨考題問的是 `"unwrap_or_default" in …`，而 M4 用
# `Config::default().shell.hands_stop_shortcut` 換同一個 bug 的另一種拼法，
# 它就安靜地綠了。針只跟拼法一樣強。
fallback = rust_binding(hotkey_set, "hands_wanted")
want(
    "⑨ 讀不出設定檔時，拔手鍵的退路是現在那一組拔手鍵",
    # 第一版只問 `"current." in fallback`，而它自己的失敗訊息就招了「擋不住取錯
    # 欄位」。招了沒人去證＝那句自白是一個沒人守的前提。實測 `current.wanted`
    # （拿暫停那一組當拔手）CI 一格都不紅，所以這裡把欄位名整個釘死。
    "current.hands_wanted" in fallback
    and "Config::default" not in fallback
    and "unwrap_or_default" not in fallback,
    "`apply_hotkey` 的第一件事是 `unregister_all()`。這一格退回出廠值的話，他在\n"
    "      `config.toml` 裡設的那顆會被拆掉、出廠的 `Ctrl+Alt+H` 上位，而設定頁\n"
    "      繼續寫著舊的那一組——**他出事時按下去什麼都不會發生**。\n"
    "      擋不住：換了來源但取錯欄位（拿到的是暫停鍵那一組）。",
)

unreadable_flag = re.search(r"config_unreadable:\s*([^,\n]+)", hotkey_set)
if not unreadable_flag:
    die("hotkey_set 裡找不到 config_unreadable", "這一格是空轉的，而它會印 ✓。")
want(
    "⑩ 重裝之後沿用開機那一次的 config_unreadable，不自己生一個",
    unreadable_flag.group(1).startswith("current."),
    "這個欄位的意思是「現在用的是**內建預設值**」，設定頁的 `paintHandsHotkey`\n"
    "      就是照這個意思寫句子的。兩種寫錯的方向都會出人命：\n"
    "      填 `None` ＝ 把開機那句警告抹掉，他再也不知道設定檔壞了；\n"
    "      填 `Some(error)` ＝ 在一條**保住他自己那一組**的路上宣稱那是預設值，\n"
    "      拔手那一格會紅著說他的安全鍵不是他的，而它正上方就印著他那顆。\n"
    "      他會改去按出廠的組合，那顆沒註冊。所以只能沿用 `current.` 那一份。\n"
    "      擋不住：沿用了，但沿用的是別的欄位。",
)

apply_body = brace_body(main, main.find("fn apply_hotkey("), "main.rs 的 apply_hotkey")
at_hands = apply_body.find("on_shortcut(hands_wanted")
at_pause = apply_body.find("on_shortcut(wanted_to_register")
if at_hands < 0 or at_pause < 0:
    die("apply_hotkey 裡找不到兩次 on_shortcut", "改名或重構過，這一格的比較沒有意義了。")
at_clear = apply_body.find("unregister_all")
if at_clear < 0:
    die("apply_hotkey 裡找不到 unregister_all", "這一格在看一個不存在的東西。")
want(
    "⑪ 先拆乾淨，再註冊拔手，最後才註冊暫停",
    # `unregister_all()` 的位置本來零覆蓋——全 repo 唯一提到它的地方是 ⑨ 的
    # **失敗訊息**裡那句「`apply_hotkey` 的第一件事是 `unregister_all()`」，
    # 也就是一句被當成事實在用、卻沒人守的前提。實測把它挪到拔手註冊之後，
    # CI 一格都不紅，而後果是：剛搶到的安全鍵當場被自己拆掉，`hands_reason`
    # 仍然是 `None`（那一刻真的成功了），於是設定頁寫「搶到了，按 X 就會拔手」
    # ——按下去什麼都不會發生。**這個洞是改註冊順序的這一版自己開的**：
    # 改序之前這一刀殺的是暫停鍵，改序之後殺的是拔手鍵。
    at_clear < at_hands < at_pause,
    "global-hotkey 對第二次註冊同一顆回 `AlreadyRegistered`。正規化再怎麼補都有\n"
    "      補不到的形狀，所以**順序**是最後一道保險：先搶的那顆活下來。暫停排前面\n"
    "      的話，每一個認不出來的撞號死的都是拔手，而暫停是那顆可以從系統匣代替的。",
)

view = brace_body(apply_body, apply_body.find("HotkeyView {"), "apply_hotkey 的 HotkeyView")
shown = re.search(r"(?<!\w)wanted:\s*([\w.]+)", view)
if not shown:
    die("HotkeyView 裡找不到 wanted 欄位", "欄位改名了，這一格在看一個不存在的東西。")
feeds = rust_binding(apply_body, shown.group(1))
want(
    "⑫ 畫面上那一格是設定檔裡的那一組，不是拿去註冊的那一組",
    # 第一版寫成 `not ("plan.pause" in feeds and "unwrap_or_default" in feeds)`
    # ——那是一個 **and**，擋的其實是「來自 plan **而且**寫成 `unwrap_or_default`」，
    # 也就是舊碼那一行的逐字拼法。實測 `plan.pause.clone().unwrap_or_else(String::new)`
    # 從它旁邊走過去，CI 全綠。改成問產地：這個值只能從參數 `wanted` 來，
    # 不准碰 `plan`。
    "plan" not in feeds and "wanted" in feeds,
    "撞號的時候 `plan.pause` 是 `None`，收成空字串之後設定頁那一格印「沒有設」\n"
    "      ——而他的 `config.toml` 裡明明寫著一組。那一格的註解自己說它是**唯一**\n"
    "      寫著暫停鍵是哪一組的地方，卡在一句假話上等於他連「按哪一顆會暫停」都\n"
    "      問不到。要顯示的是他設的那一組，要註冊的是計畫算出來的那一組。",
)

boot_log = main[main.find("match &view.reason {") :][:600]
if "暫停熱鍵" not in boot_log:
    die("找不到開機那段暫停熱鍵的 log", "錨點對不到，這一格是空轉的。")
want(
    "⑬ 開機那行 log 不拿「空不空」當「有沒有搶到」",
    "registered" in boot_log or "hands_collided" in boot_log,
    "`wanted` 現在裝的是**設定檔裡那一組**，撞號的時候也有值（設定頁要靠它才講\n"
    "      得出他設的是哪一顆）。拿 `wanted.is_empty()` 問「有沒有搶到」的話，撞號\n"
    "      那一輪會掉進最後一臂印「暫停熱鍵 Ctrl+Alt+H 搶到了」——那顆這一輪一次\n"
    "      都沒送去註冊，按下去做的是拔手。這幾行存在的理由（它自己的註解）就是\n"
    "      要分辨「搶到了」和「這段程式根本沒跑到」，對第三種情況印肯定句正好\n"
    "      毀掉那件事，而他就是照著 `grep 搶到了` 來查「為什麼我的暫停鍵沒反應」的。",
)

print()
if BAD:
    print(f"✗ 拔手那顆鍵的接線層有 {len(BAD)} 格在說謊：{'、'.join(BAD)}")
    sys.exit(1)
# 數出來的，不是寫死的：codex 補 ③ᵇ 之後這一行還在說「八格」，而那本身就是
# 一句閘門自己印出來的假話。
print(f"✓ 拔手那顆鍵的接線層，{len(CHECKED)} 格都對得上")
