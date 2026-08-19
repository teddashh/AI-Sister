#!/usr/bin/env bash
#
# 把字母人的三個狀態各截一張圖。
#
# 為什麼要有這支腳本：字母人整個是 HTML/CSS，而這台開發機是 Linux、沒有
# webkit2gtk 也沒有 sudo，所以 Tauri 視窗**在這裡開不起來**。但角色長什麼樣子
# 不需要那個視窗——用瀏覽器渲染同一份檔案就看得到。
#
# 這比「等下次上 Windows 再看」強的地方在於它是現在就能看的。第一版的
# `thinking` 和 `idle` 在靜止畫面裡一模一樣（差別全在動畫，而動畫在
# prefers-reduced-motion 之下會被關掉），就是並排看這三張圖才發現的。
#
# 它證明不了的事：視窗透明度、置頂、系統匣、拖曳——那些是 Tauri 的行為，
# 只有真的 Windows 上跑得出來。
#
# 用法：scripts/shoot-avatar.sh [輸出目錄]

set -euo pipefail

cd "$(dirname "$0")/.."
UI_DIR="apps/desktop/ui"
OUT_DIR="${1:-/tmp/sister-avatar}"
PORT=8731

CHROME=""
for candidate in \
    "$HOME/.cache/ms-playwright/chromium-"*/chrome-linux64/chrome \
    "$(command -v chromium || true)" \
    "$(command -v google-chrome || true)"; do
    if [[ -x "$candidate" ]]; then CHROME="$candidate"; break; fi
done

if [[ -z "$CHROME" ]]; then
    echo "找不到 chromium。裝一個，或自己開瀏覽器看 $UI_DIR/index.html" >&2
    exit 1
fi

mkdir -p "$OUT_DIR"

# 為什麼要起一個 server 而不是直接開 file://：index.html 的 CSP 寫著
# `script-src 'self'`，而 file:// 文件的 'self' 不涵蓋同目錄的 module，
# 於是 app.js 根本不會執行——截出來的三張圖會一模一樣（踩過）。
python3 -m http.server "$PORT" --bind 127.0.0.1 --directory "$UI_DIR" >/dev/null 2>&1 &
server=$!
trap 'kill "$server" 2>/dev/null || true' EXIT

for _ in $(seq 20); do
    curl -fs -o /dev/null "http://127.0.0.1:$PORT/index.html" && break
    sleep 0.1
done

for state in idle thinking paused; do
    "$CHROME" --headless --disable-gpu --no-sandbox --hide-scrollbars \
        --force-color-profile=srgb \
        --default-background-color=1E1E2EFF \
        --window-size=340,560 \
        --virtual-time-budget=1500 \
        --screenshot="$OUT_DIR/$state.png" \
        "http://127.0.0.1:$PORT/index.html?state=$state" >/dev/null 2>&1
    echo "  $state → $OUT_DIR/$state.png"
done

echo "✓ 三張都在 $OUT_DIR（背景色是截圖加的，視窗本身是透明的）"
