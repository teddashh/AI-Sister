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

echo "✓ README 的 quickstart 跑得起來，而且答得出 ★ +886800080123"
