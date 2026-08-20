#!/usr/bin/env bash
#
# PRIVACY.md 的第一句承諾是：
#
#     「資料不離開這台機器。程式裡沒有任何對外連線的程式碼路徑。
#       你可以自己驗證：整個 repo 搜不到 HTTP client。」
#
# 這支腳本讓那句話**由 CI 保證，而不是由記性保證**。
#
# 為什麼需要它：這個缺口不會由人親手打開，會由一個相依套件默默帶進來。
# 實際差一點發生過——OCR 本來要用 `oar-ocr`，而它的 `auto-download` feature
# 會拉進 `ureq` 去執行期下載模型。那個 feature 預設是關的，所以沒有人會在
# review 的時候看到它；等到哪天有人為了別的功能打開，PRIVACY.md 就從那一刻
# 起變成一句謊話，而且不會有任何測試變紅。
#
# 只看 `--edges normal`：build script 與測試用的相依不會被連進出貨的執行檔，
# 所以它們有 HTTP client 是可以接受的（下載模型的建置腳本就是這種）。

set -euo pipefail
cd "$(dirname "$0")/.."

# 常見的 Rust HTTP/網路 client。名單寧可長一點——多列一個頂多是誤報，
# 少列一個就是一句沒有人守著的承諾。
#
# 後半段是推論引擎。PRIVACY.md 與 README 都寫著「目前整份程式碼零次模型
# 呼叫」，而那句話跟「沒有 HTTP client」是兩件不同的事：一個本機的 ONNX
# runtime 一條網路連線都不會開，照樣讓那句話變成假的。這裡守的是 L0/L1
# 「抄寫歸程式」那條線本身，不是隱私。
FORBIDDEN='^(reqwest|ureq|hyper|curl|isahc|attohttpc|surf|minreq|ehttp|awc|http-req|tokio-tungstenite|tungstenite|async-tungstenite|websocket|ort|onnxruntime|onnxruntime-sys|tract-onnx|tract-core|candle-core|candle-nn|candle-transformers|llama-cpp-2|llm|tch|burn|rten|openai-api-rs|async-openai)$'

# 出貨的是**兩個**執行檔，而它們在兩個不同的 workspace 裡：`sister.exe` 在
# repo 根目錄那個，`sister-desktop.exe` 在 `apps/desktop/src-tauri`（Tauri 要
# 自己一份 Cargo.lock）。
#
# 這一行原本只有 `--workspace`，於是字母人那半邊——外殼、視窗、所有 Tauri
# plugin——**從來沒有被這支腳本看過**。它是靠「這裡沒有人會加相依」守著的，
# 而那正是檔案開頭那段話說不該靠的東西。發現的時機是加一個全域熱鍵 plugin
# 進去、腳本照樣印綠勾的時候。
MANIFESTS=("Cargo.toml" "apps/desktop/src-tauri/Cargo.toml")

fail=0
skipped=""
for manifest in "${MANIFESTS[@]}"; do
    who=$([ "$manifest" = "Cargo.toml" ] && echo "sister.exe" || echo "sister-desktop.exe")
    for target in "" "x86_64-pc-windows-msvc"; do
        args=(tree --manifest-path "$manifest" --edges normal --prefix none --no-dedupe --format '{p}')
        # 根目錄那個是真的 workspace（三個 crate），子目錄那個只有一個 package。
        [ "$manifest" = "Cargo.toml" ] && args+=(--workspace)
        label="host"
        if [ -n "$target" ]; then
            args+=(--target "$target")
            label="$target"
            # 沒裝這個 target 就跳過，不要讓開發機因此卡住
            rustup target list --installed | grep -qx "$target" || {
                echo "▶ $who / $label：未安裝 target，略過"
                # 略過要記下來。最後那一行以前不管跳過幾個都照印
                # 「✓ 出貨的相依樹裡沒有 HTTP client」——一句關於一棵沒有被
                # 看過的樹的話，而這個產品只出 Windows 執行檔。CI 有裝
                # target，所以它只騙得到開發機上的人；但那個人正是會照著這個
                # 勾勾決定「可以推了」的人。
                skipped="$skipped $who/$label"
                continue
            }
        fi

        echo "▶ 檢查出貨相依樹（$who / $label）"
        hits=$(cargo "${args[@]}" 2>/dev/null | awk '{print $1}' | sort -u | grep -E "$FORBIDDEN" || true)
        if [ -n "$hits" ]; then
            echo "✗ 出貨的相依樹裡出現了 HTTP client（$who / $label）："
            echo "$hits" | sed 's/^/    /'
            echo
            echo "  PRIVACY.md 說「程式裡沒有任何對外連線的程式碼路徑」。"
            echo "  要嘛拿掉這個相依，要嘛去改 PRIVACY.md——但不能兩個都不做。"
            fail=1
        fi
    done
done

# THREAT_MODEL.md 對遠端攻擊者寫的是「結構性免疫：**沒有監聽埠**、沒有輸出
# 連線」。上面那個相依樹檢查只擋得住 client——一個 `TcpListener::bind` 只用
# std，一個相依都不會多，而那句話從那一刻起就是假的。
#
# 這裡看的是原始碼而不是相依樹，因為 std 本來就在相依樹裡。
# `apps/` 和上面同一個理由：字母人的外殼也是我們自己寫的 Rust。TokenMonster
# （這個殼抄來的那份配方）當初就是靠一個本機 loopback HTTP gateway 讓畫面跟
# 後端說話的——這裡刻意沒有走那條路，而這一行是那個決定的看門人。
echo "▶ 檢查原始碼裡有沒有 socket"
# `|| true` 原本吃掉了 grep 的所有非零退出，包括「那個目錄不存在」。目錄改個
# 名字，這一步就從此永遠是綠的——而它守的是 THREAT_MODEL 對遠端攻擊者宣稱的
# 免疫。grep 的 1 是「沒找到」（要的結果），2 以上才是它自己出事。
rc=0
# `tokio::net` 那幾個名字不在上面那組裡：`TcpSocket`、`UnixStream`、
# `UnixListener`，還有兩支「還沒連但正在查去哪連」的——`lookup_host` 和
# `to_socket_addrs`。它們一個 crate 都不用多（tokio 已經在樹上了），而擋的
# 是同一句承諾。
sockets=$(grep -rnE '\b(TcpListener|TcpStream|TcpSocket|UdpSocket|UnixStream|UnixListener|lookup_host|to_socket_addrs|std::net::)' crates/ apps/ --include='*.rs') || rc=$?
if [ "$rc" -gt 1 ]; then
    echo "✗ 掃 socket 的那一步自己失敗了（grep 退出碼 $rc）——這不是「沒找到」。"
    echo "  在修好之前，「沒有監聽埠」這句話沒有任何東西守著。"
    fail=1
fi
if [ -n "$sockets" ]; then
    echo "✗ 原始碼裡出現了 socket："
    echo "$sockets" | sed 's/^/    /'
    echo
    echo "  THREAT_MODEL.md 說「沒有監聽埠、沒有輸出連線」，而那是它對"
    echo "  遠端攻擊者宣稱的**結構性**免疫——不是設定、不是預設值。"
    fail=1
fi

# 上面兩段只看 Rust。`--include='*.rs'` 這幾個字讓字母人的**畫面那一半**整個
# 在這支腳本的視線之外：`fetch()`、`new WebSocket()`、`new Image().src = "https://…"`、
# 一個 `<img src="https://…">`——每一個都能把螢幕上的字送出去，一個 crate 都不用多。
#
# 執行期擋住它們的是 CSP，而 CSP 在這個 repo 裡是**手抄六份**的（`tauri.conf.json`
# 一份，五個 HTML 各一份 `<meta>`）。從其中一份刪掉 `connect-src`、或把
# `default-src` 放寬，CI 全綠。照這個 repo 自己那條「一條規則寫在 N 個地方就會
# 有一邊忘了改」的規矩，那是這半邊最大的一塊沒人守的地方。
#
# `--include` 那份清單自己也會漏。`apps/desktop/ui` 底下是 5 個 js、5 個 html、
# **5 個 css**，而 css 那五個一直在這支腳本的視線之外——正是上面那段話在講的
# 同一種漏法，只是換一個副檔名。CSS 送得出去東西：`background: url(https://…)`、
# `@import url(https://…)`、`@font-face { src: url(https://…) }`。而且就算把
# `*.css` 加進去，上面那幾個 `src=` / `href=` 的樣式一個都對不上 CSS 的寫法，
# 所以樣式也要跟著加——**副檔名和樣式要一起補，只補一半等於沒補**。
echo "▶ 檢查畫面那一半有沒有連外"
rc=0
web=$(grep -rnE '\b(fetch|XMLHttpRequest|WebSocket|EventSource|navigator\.sendBeacon|importScripts)\s*\(|\bsrc\s*=\s*.https?://|\bhref\s*=\s*.https?://|url\(\s*.?\s*(https?:)?//|@import\s+.?\s*(https?:)?//' \
    apps/desktop/ui --include='*.js' --include='*.html' --include='*.css') || rc=$?
if [ "$rc" -gt 1 ]; then
    echo "✗ 掃畫面的那一步自己失敗了（grep 退出碼 $rc）——這不是「沒找到」。"
    fail=1
fi
if [ -n "$web" ]; then
    echo "✗ 畫面那一半出現了連外的東西："
    echo "$web" | sed 's/^/    /'
    echo
    echo "  這一頁和後端說話只走 Tauri 的 ipc:。真的需要別的路，先去改 PRIVACY.md。"
    fail=1
fi

# CSP 是上面那條的執行期後盾，所以它自己也要有人守。只釘兩件事：來源預設全關、
# 出去的路只有 ipc——其餘（`img-src` 要不要 `data:` / `asset:`）各頁本來就該不一樣，
# 釘死了只會逼人去改這支腳本。
echo "▶ 檢查 CSP 還在不在"
for f in apps/desktop/ui/*.html apps/desktop/src-tauri/tauri.conf.json; do
    for rule in "default-src 'none'" "connect-src ipc:"; do
        grep -qF "$rule" "$f" || {
            echo "✗ $f 少了 CSP 的「$rule」"
            echo "  少了它，上面那一段 grep 就是這條路上唯一的東西——而它只讀得懂"
            echo "  寫死在原始碼裡的網址。"
            fail=1
        }
    done

    # 上面那一圈找的是「`default-src 'none'` 這幾個字還在不在」，而 `default-src`
    # **只在一個 directive 缺席的時候**才輪得到它。有人在 `img-src` 後面加一個
    # `https://fonts.gstatic.com`，那幾個字一個都沒少，上面兩條照樣過——而這一頁
    # 已經有一條出去的路了。字在、話是假的，跟這個 repo 一路在修的是同一件事。
    #
    # 所以這裡不去釘每個 directive 該長什麼樣（那才會逼人每加一頁就來改這支
    # 腳本，也就是上面那段註解在避開的事），只釘一件事：**整條 CSP 裡不准出現
    # 遠端來源**。`'self'` / `data:` / `asset:` / `ipc:` 想怎麼排都行。
    csp=$(grep -hE "default-src" "$f") || csp=''
    remote=$(printf '%s' "$csp" | grep -oE "(https?:)?//[^ ;\"']*" | grep -vFx 'http://ipc.localhost') || remote=''
    if [ -n "$remote" ]; then
        echo "✗ $f 的 CSP 裡放進了遠端來源：$(printf '%s' "$remote" | tr '\n' ' ')"
        echo "  default-src 'none' 還在，所以上面兩條是綠的——但這一頁現在連得出去了。"
        echo "  真的需要它，先去改 PRIVACY.md，再回來改這裡。"
        fail=1
    fi
done

if [ "$fail" -ne 0 ]; then
    exit 1
fi

if [ -n "$skipped" ]; then
    echo "⚠ 有東西沒檢查到（未安裝 target）：$skipped"
    echo "  底下這句話只涵蓋真的跑過的那幾棵樹。出貨的是 Windows 執行檔。"
fi
echo "✓ 出貨的相依樹裡沒有 HTTP client 也沒有推論引擎，原始碼和畫面裡沒有連外的路"
