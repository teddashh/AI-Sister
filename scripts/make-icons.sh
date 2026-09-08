#!/usr/bin/env bash
#
# 從 bundled ChatGPT 角色圖與 apps/desktop/icon/icon.html 產生應用程式圖示。
#
# 角色原檔的來源、digest 與授權邊界固定在 ui/personas/manifest.json 和 NOTICE.md；
# icon.html 只負責把同一張 shipped asset 裁成 Windows 圖示，不再另外畫一個字母。
#
# 產出的檔案是 build 產物，進 .gitignore，不進 repo。CI 在打包前跑這支。

set -euo pipefail

cd "$(dirname "$0")/.."
SRC="apps/desktop/icon/icon.html"
OUT="apps/desktop/src-tauri/icons"
PORT=8732

CHROME=""
for candidate in \
    "$HOME/.cache/ms-playwright/chromium-"*/chrome-linux64/chrome \
    "$(command -v chromium || true)" \
    "$(command -v google-chrome || true)"; do
    if [[ -x "$candidate" ]]; then CHROME="$candidate"; break; fi
done
[[ -n "$CHROME" ]] || { echo "找不到 chromium" >&2; exit 1; }

mkdir -p "$OUT"

python3 -m http.server "$PORT" --bind 127.0.0.1 \
    --directory "apps/desktop" >/dev/null 2>&1 &
server=$!
trap 'kill "$server" 2>/dev/null || true' EXIT
for _ in $(seq 20); do
    curl -fs -o /dev/null "http://127.0.0.1:$PORT/icon/icon.html" && break
    sleep 0.1
done

# 背景色全 0 = 連 alpha 都是 0，截出來是透明的 PNG。
"$CHROME" --headless --disable-gpu --no-sandbox --hide-scrollbars \
    --force-color-profile=srgb \
    --default-background-color=00000000 \
    --window-size=512,512 \
    --virtual-time-budget=1200 \
    --screenshot="$OUT/icon.png" \
    "http://127.0.0.1:$PORT/icon/icon.html" >/dev/null 2>&1

python3 - "$OUT" <<'PY'
import sys
from PIL import Image

out = sys.argv[1]
master = Image.open(f"{out}/icon.png").convert("RGBA")

for size, name in ((32, "32x32.png"), (128, "128x128.png"), (256, "128x128@2x.png")):
    master.resize((size, size), Image.LANCZOS).save(f"{out}/{name}")

# Windows 的執行檔資源要 .ico，而且要多解析度——工作列、Alt-Tab、檔案總管
# 各自挑不同的一張。只塞 256 的話小尺寸會是縮出來的糊圖。
master.save(
    f"{out}/icon.ico",
    sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)],
)
print("  " + ", ".join(["icon.png", "32x32.png", "128x128.png", "128x128@2x.png", "icon.ico"]))
PY

echo "✓ 圖示產在 $OUT"
