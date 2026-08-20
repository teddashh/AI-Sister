#!/usr/bin/env bash
#
# README 的 quickstart 要真的跑得起來。
#
# Phase 1 的退場條件寫的是「clone → 跑起來 < 10 分鐘（**含 README quickstart
# 實測**）」。實測一次不難，難的是三個月後它還是對的：那幾行指令裡的旗標
# （`--data-dir`、子命令名字、scenario 檔名）全都是會被改的東西，而 README
# 不會跟著編譯失敗。一份跑不起來的 quickstart 比沒有 quickstart 糟——後者
# 至少不會讓人以為是自己裝錯了。
#
# 所以這支不是「另外寫一份和 README 一樣的測試」——那樣兩份還是會分岔。
# 它是**把 README 裡那個 code block 挖出來執行**。改了 README 就是改了這個
# 測試，改壞了 CI 就會紅。
#
# 只跑 `sister` 開頭的那幾行：`git clone` 和 `cd` 在 CI 上已經發生過了，
# `cargo build` 由呼叫端負責（它要決定 debug 還是 release）。
set -euo pipefail

cd "$(dirname "$0")/.."

SISTER="${SISTER:-./target/release/sister}"
[ -x "$SISTER" ] || { echo "::error::找不到 $SISTER——先 cargo build --release -p sister-cli"; exit 1; }

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# 挖出「從原始碼」那個標題後面**第一個** fenced block 的內容。
BLOCK="$(awk '
  /^\*\*從原始碼\*\*/ { seen = 1; next }
  seen && /^```/      { fence++; next }
  seen && fence == 1  { print }
  fence > 1           { exit }
' README.md)"
[ -n "$BLOCK" ] || { echo "::error::README 裡找不到「從原始碼」那段 quickstart——標題改了就要跟著改這裡"; exit 1; }

# README 寫的是 `./target/release/sister`；換成呼叫端給的那顆，其餘一字不動。
CMDS="$(printf '%s\n' "$BLOCK" | grep '^\./target/release/sister ' || true)"
[ -n "$CMDS" ] || { echo "::error::那段 quickstart 裡一行 sister 指令都沒有"; exit 1; }

echo "README 的 quickstart，逐行跑一次："
OUT="$WORK/out.txt"
: > "$OUT"
while IFS= read -r line; do
  cmd="${line/.\/target\/release\/sister/$SISTER}"
  # `--data-dir ./data` 要落在暫存目錄裡，不能真的在 repo 根目錄長出 ./data
  cmd="${cmd/--data-dir .\/data/--data-dir $WORK/data}"
  echo "  \$ $line"
  eval "$cmd" >> "$OUT" 2>&1 || { echo "::error::這一行掛了：$line"; tail -20 "$OUT"; exit 1; }
done <<< "$CMDS"

# 跑得完不等於答得出來。README 承諾的是「最後那行會給你這個」，而那個東西
# 是 ★ 開頭的答案——不是一句「找不到」。
grep -q '★ +886800080123' "$OUT" || {
  echo "::error::quickstart 跑完了，但 README 承諾的那個 ★ 答案沒出現"
  tail -30 "$OUT"
  exit 1
}

# 答得出來還不等於**長得像 README 畫的那樣**。上面那一條只認得一行，而
# README 底下貼的是一整塊六行的輸出：標題那行的兩個計數、「我最後看到的
# 是：」、兩筆 ★、兩行 `↳` 的種類／來源／視窗標題／frame 編號。那一塊是給
# **還沒下載的人**看的——public repo 上第一個畫面。它一漂走，每一個新來的人
# 都會拿自己機器上的輸出去對一份不存在的樣本，然後以為是自己裝錯了。
#
# 這一族今天在 `docs/WINDOWS-CHECKLIST.md` 上抓到五個（見
# `check-checklist-quotes-exist.py`）。同一種壞法，而 README 的觀眾更多。
#
# 會變的東西要遮掉，不然這一步會變成一台每天紅一次的機器（而那種閘門會被
# 關掉）：時間戳、`(剛剛)` / `(1 分鐘前)` 那個相對時間、還有毫秒數。
# 剩下要對的是**形狀和數字**，那才是承諾。
mask() { sed -E -e 's/[0-9]{4}-[0-9]{2}-[0-9]{2} [0-9]{2}:[0-9]{2}:[0-9]{2}/TS/g' \
                -e 's/\([^)]*\)/(REL)/g' \
                -e 's/[0-9]+(\.[0-9]+)? ms/MS/g'; }

SAMPLE="$(awk '
  /^最後那行會給你這個/ { seen = 1; next }
  seen && /^```/         { fence++; next }
  seen && fence == 1     { print }
  fence > 1              { exit }
' README.md)"
[ -n "$SAMPLE" ] || { echo "::error::README 裡找不到「最後那行會給你這個」底下那塊範例輸出——改了標題就要跟著改這裡"; exit 1; }

# 從最後一個 🔍 那行切到結尾，不要用 `tail -n 6` 去數。$OUT 裡前面還躺著
# `replay` 的輸出，而「查詢那一段剛好是最後六行」是一個沒有人保證的巧合——
# debug build 多印一行警告，這一步就會變成一台看起來很有道理的紅燈。
ACTUAL="$(awk '/^🔍/ { buf = ""; } { buf = buf $0 "\n" } END { printf "%s", buf }' "$OUT")"
[ -n "$ACTUAL" ] || { echo "::error::跑完了，但輸出裡連一行 🔍 都沒有"; tail -30 "$OUT"; exit 1; }

# 兩邊都要 `printf '%s\n'`。`$(...)` 會把結尾的換行**全部**吃掉，所以這裡用
# `printf '%s'` 的那一版是差一個換行的——diff 印出 `\ No newline at end of
# file`，而閘門就在**完全正確的** README 上翻紅了。一支會在對的東西上喊紅的
# 閘門會被關掉，關掉之後它守的那條線是一格空白。
diff <(printf '%s\n' "$SAMPLE" | mask) <(printf '%s\n' "$ACTUAL" | mask) > "$WORK/diff.txt" || {
  echo "::error::README 底下那塊範例輸出，和她現在真的印出來的不一樣了"
  echo "（時間戳、(剛剛) 那種相對時間、毫秒數都已經遮掉，所以差的是形狀或數字。）"
  echo "--- README 說的 / 她真的說的 ---"
  cat "$WORK/diff.txt"
  exit 1
}

echo "✓ README 的 quickstart 跑得起來，答得出 ★ +886800080123，而且那塊範例輸出逐行對得上"
