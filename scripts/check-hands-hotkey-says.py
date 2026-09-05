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
- 全部八條都不會在 Windows 上跑出任何行為證據。這一顆鍵在真 Tauri 視窗上
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


def die(what: str, why: str) -> None:
    print(f"✗ {what}\n  {why}")
    sys.exit(2)


def want(tag: str, ok: bool, why: str) -> None:
    print(f"  {'✓' if ok else '✗'} {tag}")
    if not ok:
        print(f"      {why}")
        BAD.append(tag)


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
    "送給後端的組合鍵形狀（`Ctrl+Alt+KeyH`）是 `kill_switch::normalize_hotkey` 那\n"
    "      一族測試的前提。這裡一改，`crates/sister-core/tests/exam_r33.rs` 裡\n"
    "      手寫的那一對字串就不再是產品生得出來的形狀，而它會繼續綠著。",
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

print()
if BAD:
    print(f"✗ 拔手那顆鍵的接線層有 {len(BAD)} 格在說謊：{'、'.join(BAD)}")
    sys.exit(1)
print("✓ 拔手那顆鍵的接線層，八格都對得上")
