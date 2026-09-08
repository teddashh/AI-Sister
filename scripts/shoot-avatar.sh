#!/usr/bin/env bash
#
# 把桌面姊妹的四個主要狀態各截一張圖。
#
# 為什麼要有這支腳本：桌面姊妹整個是 HTML/CSS，而這台開發機是 Linux、沒有
# webkit2gtk 也沒有 sudo，所以 Tauri 視窗**在這裡開不起來**。但角色長什麼樣子
# 不需要那個視窗——用瀏覽器渲染同一份檔案就看得到。
#
# 這比「等下次上 Windows 再看」強的地方在於它是現在就能看的。第一版的
# `thinking` 和 `idle` 在靜止畫面裡一模一樣（差別全在動畫，而動畫在
# prefers-reduced-motion 之下會被關掉），就是並排看這幾張圖才發現的。
#
# 它證明不了的事：視窗透明度、置頂、系統匣、拖曳——那些是 Tauri 的行為，
# 只有真的 Windows 上跑得出來。
#
# 用法：scripts/shoot-avatar.sh [輸出目錄]

set -euo pipefail

cd "$(dirname "$0")/.."
UI_DIR="apps/desktop/ui"
OUT_DIR="${1:-/tmp/sister-avatar}"

mkdir -p "$OUT_DIR"

# 這四張是一組證據。若第二態失敗就讓 set -e 停下來，尚未輪到的第三、四態不會
# 進到 shot.mjs 裡自行清舊檔；因此啟動前先只刪掉這組 exact targets，避免產出
# 「前兩張是這輪、後兩張是上一輪」的混合目錄。
rm -f \
    "$OUT_DIR/idle.png" \
    "$OUT_DIR/thinking.png" \
    "$OUT_DIR/paused.png" \
    "$OUT_DIR/asleep.png"

# 為什麼要起一個 server 而不是直接開 file://：index.html 的 CSP 寫著
# `script-src 'self'`，而 file:// 文件的 'self' 不涵蓋同目錄的 module，
# 於是 app.js 根本不會執行——截出來的幾張圖會一模一樣（踩過）。
port_file=$(mktemp)
server_log=$(mktemp)
python3 - "$UI_DIR" "$port_file" >"$server_log" 2>&1 <<'PY' &
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import sys


class QuietHandler(SimpleHTTPRequestHandler):
    def log_message(self, _format, *_args):
        pass


root = Path(sys.argv[1]).resolve(strict=True)
port_file = Path(sys.argv[2])
server = ThreadingHTTPServer(
    ("127.0.0.1", 0),
    partial(QuietHandler, directory=root),
)
# Port 0 由 OS 在 socket 已經獨占之後分配；寫出來的數字只屬於這個 PID，不會
# 因固定 port 上剛好有另一份 stale worktree server 而把錯頁截成成功。
port_file.write_text(str(server.server_address[1]), encoding="ascii")
server.serve_forever()
PY
server=$!
trap 'kill "$server" 2>/dev/null || true; rm -f "$port_file" "$server_log"' EXIT

PORT=""
ready=false
for _ in $(seq 100); do
    if ! kill -0 "$server" 2>/dev/null; then
        echo "本機截圖 server 啟動失敗：" >&2
        sed -n '1,20p' "$server_log" >&2
        exit 1
    fi
    if [[ -s "$port_file" ]]; then
        PORT=$(<"$port_file")
        if [[ "$PORT" =~ ^[0-9]+$ ]] &&
            curl -fs -o /dev/null "http://127.0.0.1:$PORT/index.html"; then
            ready=true
            break
        fi
    fi
    sleep 0.05
done
[[ "$ready" == true ]] || { echo "本機截圖 server 5 秒內沒有 ready" >&2; exit 1; }

for state in idle thinking paused asleep; do
    node scripts/shot.mjs \
        "http://127.0.0.1:$PORT/index.html?state=$state" \
        "$OUT_DIR/$state.png" \
        340 560 \
        --expect-js "document.querySelector('[data-avatar]')?.dataset.state === '$state'" \
        >/dev/null
    echo "  $state → $OUT_DIR/$state.png"
done

echo "✓ 四張都在 $OUT_DIR（每張 340×560 且已到指定狀態；Tauri 視窗透明度仍需 Windows 實測）"
