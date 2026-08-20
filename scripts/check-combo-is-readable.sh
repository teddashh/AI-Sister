#!/usr/bin/env bash
#
# 鍵盤上沒有一顆鍵叫 KeyP。
#
# 熱鍵在程式裡的形狀是 `Ctrl+Alt+KeyP`——那是 Tauri 的 accelerator 語法，
# 而 `HotkeyView.wanted` / `.rejected` 一路帶著它。給人看的時候要先過
# `sister_shell::pretty_combo`（Rust）或 settings.js 裡那個 `pretty`（畫面），
# 兩個都是把 `KeyP` / `Digit1` 前面那截拔掉，變成 `Ctrl + Alt + P`。
#
# 為什麼要一支腳本守：這條規則的每一個違反都長得**完全正常**。
#
#     format!("還在用 {}。", restored.wanted)
#
# 這行編得過、clippy 綠、review 讀起來也對——它確實在講「還在用哪一組」。
# 只有真的把那句話印出來，才看得到它叫使用者去按一顆不存在的鍵。而這句話
# 出現的時機（設定檔存不進去、退回舊的那一組）在測試機上幾乎踩不到。
#
# 而且它現在**沒有任何測試守著**：那句話在 `apps/desktop/src-tauri`，那是
# 一個獨立的 workspace，只跑 `cargo check` 和 `clippy`，從來沒有 `cargo test`
# 過。把 `:1442` 改回 `restored.wanted` 是紅不了任何東西的。
#
# 這裡不去驗那句話長什麼樣（那要一個跑得起來的 Tauri），只釘一件事：
# **凡是在組給人看的句子，就不准直接碰 `wanted` / `rejected`。**

set -euo pipefail
cd "$(dirname "$0")/.."

fail=0

# ── Rust ──────────────────────────────────────────────────────────────
#
# 只看在組字串的那幾行：`format!` / `push_str` / `to_string` / `write!`。
# `tracing::info!("暫停熱鍵 {} 搶到了", view.wanted)` 是刻意放過的——log 給的是
# 開發者，而開發者要的正是原始的 accelerator 字串（照著它去查 Tauri 的文件）。
echo "▶ Rust：給人看的句子裡有沒有生的 accelerator"
rc=0
raw_rs=$(grep -rnE '(format!|write!|push_str|\.to_string\(\)).*\.(wanted|rejected)' \
    crates/ apps/desktop/src-tauri/src --include='*.rs' \
    | grep -v 'pretty_combo') || rc=$?
if [ "$rc" -gt 1 ]; then
    echo "✗ 掃 Rust 的那一步自己失敗了（grep 退出碼 $rc）——這不是「沒找到」。"
    fail=1
fi
if [ -n "$raw_rs" ]; then
    echo "✗ 有一句給人看的話直接印了 accelerator："
    echo "$raw_rs" | sed 's/^/    /'
    echo
    echo "  包一層 sister_shell::pretty_combo(…)。生的那個字串裡的 KeyP / Digit1"
    echo "  在鍵盤上是不存在的鍵，而這幾句話正是要他照著去按的。"
    fail=1
fi

# 反過來也要守：`pretty_combo` 自己被刪掉、或被改成 identity，上面那條 grep
# 會全綠——它只看得到「有沒有呼叫」，看不到「呼叫了有沒有用」。
echo "▶ pretty_combo 自己還在不在，而且真的拔得掉前綴"
if ! grep -q 'pub fn pretty_combo' crates/sister-shell/src/lib.rs; then
    echo "✗ crates/sister-shell/src/lib.rs 裡沒有 pretty_combo 了。"
    echo "  上面那條 grep 從此永遠是綠的——它問的是「有沒有呼叫它」。"
    fail=1
elif ! grep -q 'assert_eq!(pretty_combo(' crates/sister-shell/src/lib.rs; then
    echo "✗ pretty_combo 還在，但沒有 assert 釘著它拔前綴的行為。"
    echo "  把它的內容換成 combo.to_string() 不會紅任何東西，而上面那條 grep 照樣綠。"
    fail=1
fi

# ── 畫面 ──────────────────────────────────────────────────────────────
#
# 同一條規則的另一半。判斷「這一行在組給人看的句子」用的是「這一行有中文」
# ——這一頁上所有給人看的字都是中文的，而 `view.wanted === ""` 這種純比較沒有。
echo "▶ 畫面：給人看的句子裡有沒有生的 accelerator"
rc=0
raw_js=$(grep -nE 'view\.(wanted|rejected)' apps/desktop/ui/settings.js \
    | grep -P '[\x{4e00}-\x{9fff}]' \
    | grep -v 'pretty(') || rc=$?
if [ "$rc" -gt 1 ]; then
    echo "✗ 掃畫面的那一步自己失敗了（grep 退出碼 $rc）——這不是「沒找到」。"
    fail=1
fi
if [ -n "$raw_js" ]; then
    echo "✗ settings.js 有一句給人看的話直接印了 accelerator："
    echo "$raw_js" | sed 's/^/    /'
    echo
    echo "  包一層 pretty(…)。"
    fail=1
fi

if ! grep -q 'function pretty(' apps/desktop/ui/settings.js; then
    echo "✗ settings.js 裡沒有 pretty() 了——上面那條 grep 從此永遠是綠的。"
    fail=1
fi

[ "$fail" -ne 0 ] && exit 1
echo "✓ 沒有任何一句給人看的話會叫他去按 KeyP"
