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

fail=0
for target in "" "x86_64-pc-windows-msvc"; do
    args=(tree --workspace --edges normal --prefix none --no-dedupe --format '{p}')
    label="host"
    if [ -n "$target" ]; then
        args+=(--target "$target")
        label="$target"
        # 沒裝這個 target 就跳過，不要讓開發機因此卡住
        rustup target list --installed | grep -qx "$target" || {
            echo "▶ $label：未安裝 target，略過"
            continue
        }
    fi

    echo "▶ 檢查出貨相依樹（$label）"
    hits=$(cargo "${args[@]}" 2>/dev/null | awk '{print $1}' | sort -u | grep -E "$FORBIDDEN" || true)
    if [ -n "$hits" ]; then
        echo "✗ 出貨的相依樹裡出現了 HTTP client（$label）："
        echo "$hits" | sed 's/^/    /'
        echo
        echo "  PRIVACY.md 說「程式裡沒有任何對外連線的程式碼路徑」。"
        echo "  要嘛拿掉這個相依，要嘛去改 PRIVACY.md——但不能兩個都不做。"
        fail=1
    fi
done

# THREAT_MODEL.md 對遠端攻擊者寫的是「結構性免疫：**沒有監聽埠**、沒有輸出
# 連線」。上面那個相依樹檢查只擋得住 client——一個 `TcpListener::bind` 只用
# std，一個相依都不會多，而那句話從那一刻起就是假的。
#
# 這裡看的是原始碼而不是相依樹，因為 std 本來就在相依樹裡。
echo "▶ 檢查原始碼裡有沒有 socket"
sockets=$(grep -rnE '\b(TcpListener|UdpSocket|TcpStream|std::net::)' crates/ --include='*.rs' || true)
if [ -n "$sockets" ]; then
    echo "✗ 原始碼裡出現了 socket："
    echo "$sockets" | sed 's/^/    /'
    echo
    echo "  THREAT_MODEL.md 說「沒有監聽埠、沒有輸出連線」，而那是它對"
    echo "  遠端攻擊者宣稱的**結構性**免疫——不是設定、不是預設值。"
    fail=1
fi

if [ "$fail" -ne 0 ]; then
    exit 1
fi

echo "✓ 出貨的相依樹裡沒有 HTTP client 也沒有推論引擎，原始碼裡沒有 socket"
