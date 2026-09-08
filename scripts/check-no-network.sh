#!/usr/bin/env bash
#
# PRIVACY.md 現在守的是一條**能力邊界**：
#
#     畫面不離機；root 預設與 recorder/core/brain/hands 沒有 HTTP client。desktop
#     只有兩個窄例外：`sister-assets[download]` 的 fixed Persona GET，以及
#     `sister-tts[azure]` 在現行第四張同意、enabled 設定下只替最新新答案或
#     trusted 手動重播走 fixed Azure POST。
#
# 這支腳本讓那條界線**由 CI 保證，而不是由記性保證**。
# 它不替使用者設定的外部 CLI 背書：簽了 cloud-reading 後，OCR 原文會交給
# 那支本機行程；它是否連到 provider，是那支 CLI 自己的邊界。
#
# 為什麼需要它：這個缺口不會由人親手打開，會由一個相依套件默默帶進來。
# 實際差一點發生過——OCR 本來要用 `oar-ocr`，而它的 `auto-download` feature
# 會拉進 `ureq` 去執行期下載模型。Persona 與 Azure TTS 現在刻意使用同名
# client，但各自只准在隔離 crate 的非預設 feature；因此不能再拿「看到 ureq
# 就全紅」代替邊界。
#
# 只看 `--edges normal`：build script 與測試用的相依不會被連進出貨的執行檔，
# 所以它們有 HTTP client 是可以接受的（下載模型的建置腳本就是這種）。

set -euo pipefail
cd "$(dirname "$0")/.."

# 常見的 Rust HTTP/網路 client。名單寧可長一點——多列一個頂多是誤報，
# 少列一個就是一句沒有人守著的承諾。
#
# 後半段是**內建**推論引擎。產品現在可以用 `std::process::Command` 呼叫
# 使用者設定的 CLI；這份名單守的是「AI-Sister 自己沒有模型 runtime」，
# 不是「整個產品零次模型呼叫」，也不檢查外部 CLI 之後是否連網。
NETWORK_CLIENTS='^(reqwest|ureq|hyper|curl|isahc|attohttpc|surf|minreq|ehttp|awc|http-req|tokio-tungstenite|tungstenite|async-tungstenite|websocket|openai-api-rs|async-openai)$'
INFERENCE='^(ort|onnxruntime|onnxruntime-sys|tract-onnx|tract-core|candle-core|candle-nn|candle-transformers|llama-cpp-2|llm|tch|burn|rten)$'

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
        packages=$(cargo "${args[@]}" 2>/dev/null | awk '{print $1}' | sort -u)
        network=$(printf '%s\n' "$packages" | grep -E "$NETWORK_CLIENTS" || true)
        inference=$(printf '%s\n' "$packages" | grep -E "$INFERENCE" || true)
        if [ -n "$inference" ]; then
            echo "✗ 出貨的相依樹裡出現了內建推論引擎（$who / $label）："
            echo "$inference" | sed 's/^/    /'
            echo
            echo "  模型只能由使用者設定的外部 CLI 接手；Persona 下載不是這一格的例外。"
            fail=1
        fi
        if [ "$manifest" = "Cargo.toml" ]; then
            if [ -n "$network" ]; then
                echo "✗ sister.exe／root workspace 的相依樹出現了 HTTP client（$label）："
                echo "$network" | sed 's/^/    /'
                echo "  Persona／Azure transport 只能存在 desktop 的兩個窄 feature 路徑。"
                fail=1
            fi
        else
            unexpected=$(printf '%s\n' "$network" | grep -vE '^ureq$' || true)
            if [ -n "$unexpected" ] || ! printf '%s\n' "$network" | grep -qx 'ureq'; then
                echo "✗ sister-desktop 的 client 集合不是唯一允許的 ureq（$label）："
                printf '%s\n' "${network:-（沒有找到 ureq）}" | sed 's/^/    /'
                fail=1
            fi
        fi
    done
done

# 允許 client 不等於讓它散進整個 desktop。manifest 與 source 都要證明兩條反向
# 路徑恰好是 ureq → sister-assets[download] → sister-desktop，與
# ureq → sister-tts[azure] → sister-desktop；core、capture、hands 與 renderer
# 都拿不到 request builder。
echo "▶ 檢查 Persona GET 與 Azure POST 的 crate／feature 邊界"
grep -qF 'sister-assets = { path = "crates/sister-assets", default-features = false }' Cargo.toml || {
    echo "✗ root workspace 沒有把 sister-assets 的預設 feature 關掉"
    fail=1
}
grep -qF 'sister-tts = { path = "crates/sister-tts", default-features = false }' Cargo.toml || {
    echo "✗ root workspace 沒有把 sister-tts 的預設 feature 關掉"
    fail=1
}
grep -qF 'download = ["dep:ureq"]' crates/sister-assets/Cargo.toml || {
    echo "✗ sister-assets 的 download feature 不再是唯一 ureq 開關"
    fail=1
}
grep -qF 'ureq = { workspace = true, optional = true }' crates/sister-assets/Cargo.toml || {
    echo "✗ ureq 不再是 sister-assets 的 optional dependency"
    fail=1
}
grep -qF 'azure = ["dep:ureq"]' crates/sister-tts/Cargo.toml || {
    echo "✗ sister-tts 的 azure feature 不再是唯一 ureq 開關"
    fail=1
}
grep -qF 'ureq = { workspace = true, optional = true }' crates/sister-tts/Cargo.toml || {
    echo "✗ ureq 不再是 sister-tts 的 optional dependency"
    fail=1
}
ureq_manifest_lines=$(grep -rhE '^[[:space:]]*ureq[[:space:]]*=' . \
    --include='Cargo.toml' --exclude-dir=target --exclude-dir=.git | sort)
expected_ureq_manifest_lines=$(printf '%s\n' \
    'ureq = { version = "=3.4.1", default-features = false, features = ["native-tls"] }' \
    'ureq = { workspace = true, optional = true }' \
    'ureq = { workspace = true, optional = true }' | sort)
if [ "$ureq_manifest_lines" != "$expected_ureq_manifest_lines" ]; then
    echo "✗ ureq 的 Cargo manifest 宣告不再只有 workspace pin 與兩個窄 optional dependency："
    printf '%s\n' "${ureq_manifest_lines:-（沒有找到）}" | sed 's/^/    /'
    fail=1
fi
grep -qF 'sister-assets = { path = "../../../crates/sister-assets", features = ["download"] }' apps/desktop/src-tauri/Cargo.toml || {
    echo "✗ desktop 沒有經 sister-assets[download] 取得唯一 transport"
    fail=1
}
grep -qF 'sister-tts = { path = "../../../crates/sister-tts", features = ["azure"] }' apps/desktop/src-tauri/Cargo.toml || {
    echo "✗ desktop 沒有經 sister-tts[azure] 取得 Azure transport"
    fail=1
}

rc=0
ureq_source=$(grep -rnE '\bureq::' crates/ apps/ --include='*.rs') || rc=$?
if [ "$rc" -gt 1 ]; then
    echo "✗ 掃 ureq source boundary 的步驟失敗（grep 退出碼 $rc）"
    fail=1
fi
unexpected_ureq=$(printf '%s\n' "$ureq_source" \
    | grep -vE '^crates/sister-(assets/src/download|tts/src/native)\.rs:' || true)
if [ -z "$ureq_source" ] || [ -n "$unexpected_ureq" ]; then
    echo "✗ ureq source 不只存在於 fixed Persona GET 與 Azure POST transport："
    printf '%s\n' "${unexpected_ureq:-（兩個 transport 裡都找不到）}" | sed 's/^/    /'
    fail=1
fi

for target in "" "x86_64-pc-windows-msvc"; do
    args=(tree --manifest-path apps/desktop/src-tauri/Cargo.toml --edges normal --invert ureq --prefix depth --format '{p}')
    label="host"
    if [ -n "$target" ]; then
        rustup target list --installed | grep -qx "$target" || continue
        args+=(--target "$target")
        label="$target"
    fi
    reverse=$(cargo "${args[@]}" 2>/dev/null \
        | sed -E 's/^([0-9]+)([^ ]+).*/\1\2/')
    expected=$(printf '%s\n' 0ureq 1sister-assets 2sister-desktop 1sister-tts 2sister-desktop)
    if [ "$reverse" != "$expected" ]; then
        echo "✗ ureq 的 exact 反向相依路徑不是兩個窄 crate → sister-desktop（$label）："
        printf '%s\n' "$reverse" | sed 's/^/    /'
        fail=1
    fi
done

# NSIS 本身不是 runtime HTTP client，但 Tauri 的 Windows 預設值是
# `downloadBootstrapper`：使用者按的是「安裝」，installer 卻會臨時連出去抓
# WebView2。那會在 Cargo dependency tree 完全看不見。alpha.106 改成把官方
# offline installer 嵌進 setup；這裡解析真正的 platform merge，避免有人把 overlay
# 改名、刪掉或在 base 裡加一個 minimum version，讓 EdgeUpdate 旁路又活回來。
echo "▶ 檢查 Windows installer 不需要執行期網路"
if ! python3 - <<'PY'
import difflib
import json
import pathlib
import re
import sys


def merge_patch(base: object, patch: object) -> object:
    """RFC 7396；Tauri 的 platform config 使用同一種 object merge 語意。"""
    if not isinstance(patch, dict):
        return patch
    merged = dict(base) if isinstance(base, dict) else {}
    for key, value in patch.items():
        if value is None:
            merged.pop(key, None)
        else:
            merged[key] = merge_patch(merged.get(key), value)
    return merged


root = pathlib.Path("apps/desktop/src-tauri")
base_path = root / "tauri.conf.json"
windows_path = root / "tauri.windows.conf.json"
if not windows_path.is_file():
    raise SystemExit("✗ 缺少 tauri.windows.conf.json；Tauri 會退回會連網的 WebView2 預設")

base = json.loads(base_path.read_text(encoding="utf-8"))
windows_overlay = json.loads(windows_path.read_text(encoding="utf-8"))
merged = merge_patch(base, windows_overlay)
bundle = merged.get("bundle", {})
if not isinstance(bundle, dict):
    raise SystemExit("✗ merged bundle config 不是 object")
if bundle.get("createUpdaterArtifacts") is not False:
    raise SystemExit("✗ Windows bundle 必須精確關閉 createUpdaterArtifacts")
windows = bundle.get("windows")
if not isinstance(windows, dict):
    raise SystemExit("✗ merged Windows bundle config 不存在")

expected_mode = {"type": "offlineInstaller", "silent": True}
if windows.get("webviewInstallMode") != expected_mode:
    raise SystemExit(
        "✗ WebView2 install mode 必須精確是 embedded offlineInstaller + silent=true，"
        f"實際：{windows.get('webviewInstallMode')!r}"
    )
for minimum_key in ("minimumWebview2Version", "minimum-webview2-version"):
    if minimum_key in windows:
        raise SystemExit(f"✗ {minimum_key} 會讓 installer 呼叫 EdgeUpdate；這裡不准存在")
nsis = windows.get("nsis")
if not isinstance(nsis, dict):
    raise SystemExit("✗ merged Windows NSIS config 不存在")
for minimum_key in ("minimumWebview2Version", "minimum-webview2-version"):
    if minimum_key in nsis:
        raise SystemExit(f"✗ NSIS {minimum_key} 也會呼叫 EdgeUpdate；這裡不准存在")
if "template" in nsis:
    raise SystemExit("✗ 自訂 NSIS template 可繞過 offlineInstaller；這裡只准 pinned Tauri template")

expected_hook = "windows/installer-hooks.nsh"
if nsis.get("installerHooks") != expected_hook:
    raise SystemExit(
        f"✗ NSIS installerHooks 必須精確是 {expected_hook!r}，實際：{nsis.get('installerHooks')!r}"
    )
expected_languages = ["TradChinese", "English"]
if nsis.get("languages") != expected_languages:
    raise SystemExit(
        f"✗ NSIS languages 必須精確是 {expected_languages!r}，實際：{nsis.get('languages')!r}"
    )
expected_language_files = {
    "TradChinese": "windows/languages/TradChinese.nsh",
    "English": "windows/languages/English.nsh",
}
if nsis.get("customLanguageFiles") != expected_language_files:
    raise SystemExit(
        "✗ NSIS customLanguageFiles 必須精確綁住兩份已稽核文案，"
        f"實際：{nsis.get('customLanguageFiles')!r}"
    )

# hook 是 raw NSIS，Cargo tree 看不到它叫外部 downloader。不用 primitive
# denylist：NSIS preprocessor 可以用 `!include` 或巨集別名把同一個指令拆開，
# 讓每一個關鍵字都消失在 source 掃描裡。這裡忽略純註解與空白後，
# 其餘每一行與順序都必須等於這份最小 allowlist；真的新增 hook 能力時，
# 必須在同一個 diff 裡明確擴充這份清單。語言檔也被 raw include，
# 所以只准註解與 LangString。
hook_path = root / expected_hook
if not hook_path.is_file():
    raise SystemExit(f"✗ 缺少 installer hook：{hook_path}")
hook_text = hook_path.read_text(encoding="utf-8")


def executable_hook_lines(source: str) -> list[str]:
    lines: list[str] = []
    for raw_line in source.splitlines():
        line = raw_line.strip()
        if not line:
            continue
        if line.startswith(";"):
            # NSIS 的 line continuation 對註解也生效；`; ... \`
            # 會把下一行一起吃掉。允許這種註解就等於允許
            # 只改註解便可關掉一道 process check，所以留在
            # actual lines 裡交給 exact allowlist 拒絕。
            if line.endswith("\\"):
                lines.append(line)
            continue
        lines.append(line)
    return lines


expected_hook_lines = [
    "!macro AI_SISTER_REQUIRE_STOPPED executableName",
    'nsis_tauri_utils::FindProcessCurrentUser "${executableName}"',
    "Pop $R0",
    "${If} $R0 = 0",
    "${IfNot} ${Silent}",
    'MessageBox MB_ICONSTOP|MB_OK "$(aiSisterStillRunning)"',
    "${EndIf}",
    "SetErrorLevel 32",
    "Quit",
    "${EndIf}",
    "!macroend",
    "!macro NSIS_HOOK_PREINSTALL",
    '!insertmacro AI_SISTER_REQUIRE_STOPPED "sister-desktop.exe"',
    '!insertmacro AI_SISTER_REQUIRE_STOPPED "sister.exe"',
    "!macroend",
    "!macro NSIS_HOOK_PREUNINSTALL",
    '!insertmacro AI_SISTER_REQUIRE_STOPPED "sister-desktop.exe"',
    '!insertmacro AI_SISTER_REQUIRE_STOPPED "sister.exe"',
    "!macroend",
]


def hook_is_allowed(source: str) -> bool:
    return executable_hook_lines(source) == expected_hook_lines


# 這三種是 denylist／太寬的註解過濾會放過的實際繞法。自測用記憶體內的字串，
# 不暫改、不還原工作目錄裡真正的 hook。
hook_mutations = {
    "!include 繞過": hook_text.replace(
        "!macro NSIS_HOOK_PREINSTALL",
        '!include "unreviewed-network-hook.nsh"\n\n!macro NSIS_HOOK_PREINSTALL',
        1,
    ),
    "巨集別名繞過": hook_text.replace(
        "!macro NSIS_HOOK_PREINSTALL\n",
        "!define AI_SISTER_DL NSISdl\n\n"
        "!macro NSIS_HOOK_PREINSTALL\n"
        '  ${AI_SISTER_DL}::download "$0" "$TEMP\\payload"\n',
        1,
    ),
    "註解 continuation 吃掉檢查": hook_text.replace(
        '  nsis_tauri_utils::FindProcessCurrentUser "${executableName}"',
        '  ; ignore the next process check \\\n'
        '  nsis_tauri_utils::FindProcessCurrentUser "${executableName}"',
        1,
    ),
}
for label, mutation in hook_mutations.items():
    if mutation == hook_text or hook_is_allowed(mutation):
        raise SystemExit(f"✗ installer hook allowlist 自測沒有拒絕 {label}")

actual_hook_lines = executable_hook_lines(hook_text)
if actual_hook_lines != expected_hook_lines:
    difference = "\n".join(
        difflib.unified_diff(
            expected_hook_lines,
            actual_hook_lines,
            fromfile="allowed installer hook",
            tofile=str(hook_path),
            lineterm="",
        )
    )
    raise SystemExit(
        "✗ installer hook 出現 allowlist 外的 raw NSIS 指令；"
        "新增能力前必須先重新稽核：\n"
        f"{difference}"
    )

expected_keys = {
    "addOrReinstall", "alreadyInstalled", "alreadyInstalledLong", "appRunning",
    "appRunningOkKill", "chooseMaintenanceOption", "choowHowToInstall", "createDesktop",
    "dontUninstall", "dontUninstallDowngrade", "failedToKillApp", "installingWebview2",
    "newerVersionInstalled", "older", "olderOrUnknownVersionInstalled", "silentDowngrades",
    "unableToUninstall", "uninstallApp", "uninstallBeforeInstalling", "unknown",
    "webview2AbortError", "webview2DownloadError", "webview2DownloadSuccess",
    "webview2Downloading", "webview2InstallError", "webview2InstallSuccess", "deleteAppData",
    "aiSisterStillRunning",
}
expected_delete_copy = {
    "TradChinese": "清除桌面外殼資料（AI-Sister 記憶會保留）",
    "English": "Clear desktop-shell data (AI-Sister memories are kept)",
}
expected_running_copy = {
    "TradChinese": "AI-Sister 仍在執行。請先從系統匣結束桌面程式並停止 recorder，再重新執行。這次操作沒有強制關閉任何程式。",
    "English": "AI-Sister is still running. Exit the desktop app from the tray and stop the recorder, then try again. No process was force-closed.",
}
lang_line = re.compile(r'^LangString ([A-Za-z0-9]+) \$\{LANG_([A-Z]+)\} "(.*)"$')
for language, relative_path in expected_language_files.items():
    path = root / relative_path
    if not path.is_file():
        raise SystemExit(f"✗ 缺少 NSIS {language} 文案：{path}")
    messages: dict[str, str] = {}
    for number, raw_line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        line = raw_line.strip()
        if not line or line.startswith(";"):
            continue
        parsed = lang_line.fullmatch(line)
        if parsed is None:
            raise SystemExit(f"✗ {path}:{number} 不是單純 LangString：{line}")
        key, token, copy = parsed.groups()
        if token != language.upper():
            raise SystemExit(f"✗ {path}:{number} 的 language token 是 {token}，不是 {language.upper()}")
        if key in messages:
            raise SystemExit(f"✗ {path} 重複 LangString：{key}")
        messages[key] = copy
    if set(messages) != expected_keys:
        missing = sorted(expected_keys - set(messages))
        extra = sorted(set(messages) - expected_keys)
        raise SystemExit(f"✗ {path} LangString 集合不符：missing={missing} extra={extra}")
    if messages["deleteAppData"] != expected_delete_copy[language]:
        raise SystemExit(f"✗ {path} 又把外殼資料說成 AI-Sister 記憶：{messages['deleteAppData']!r}")
    if messages["aiSisterStillRunning"] != expected_running_copy[language]:
        raise SystemExit(f"✗ {path} 的執行中拒絕文案不再是已稽核版本：{messages['aiSisterStillRunning']!r}")


def contains_key(value: object, wanted: str) -> bool:
    if isinstance(value, dict):
        return wanted in value or any(contains_key(child, wanted) for child in value.values())
    if isinstance(value, list):
        return any(contains_key(child, wanted) for child in value)
    return False


for config_path in sorted(root.glob("tauri*.conf.json")):
    config = json.loads(config_path.read_text(encoding="utf-8"))
    if contains_key(config, "updater"):
        raise SystemExit(f"✗ {config_path} 啟用了 updater；產品目前只有人工下載 installer")

for manifest in pathlib.Path(".").rglob("Cargo.toml"):
    if "target" in manifest.parts or ".git" in manifest.parts:
        continue
    text = manifest.read_text(encoding="utf-8")
    if "tauri-plugin-updater" in text or "tauri_plugin_updater" in text:
        raise SystemExit(f"✗ {manifest} 取得了 Tauri updater dependency")

for source_root in (pathlib.Path("crates"), pathlib.Path("apps")):
    for source in source_root.rglob("*.rs"):
        if "target" in source.parts:
            continue
        if "tauri_plugin_updater" in source.read_text(encoding="utf-8"):
            raise SystemExit(f"✗ {source} 接上了 Tauri updater runtime")
PY
then
    fail=1
fi

# THREAT_MODEL.md 對遠端攻擊者寫的是「本程式沒有監聽埠」，並把唯一 fixed
# outbound client 收在上面的 crate 邊界。相依樹檢查擋不住 raw socket——一個
# `TcpListener::bind` 只用 std，一個相依都不會多，而那句話從那一刻起就是假的。
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
    echo "  THREAT_MODEL.md 說 AI-Sister 沒有監聽埠；Persona 只能用受限 client，"
    echo "  使用者設定的外部 CLI 也不是替本程式開 raw socket 的例外。"
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

# CSP 是上面那條的執行期後盾，所以它自己也要有人守。不能用 substring grep 判斷
# directive 存在：`xdefault-src 'none'; xconnect-src ipc:` 同時含那兩段字，但瀏覽器
# 會忽略兩個未知 directive，結果是整份 policy 沒有 default/connect 限制。這裡用真正
# 的 HTML/JSON parser 建 exact directive map、拒絕重複名稱，再把每個 source token 對
# 本機 allowlist。`default-src` 必須只有 `'none'`；`connect-src` 必須正好是 Tauri 的
# 兩個本機 IPC source。其餘 directive 可以增減，但新增一種 source scheme 必須在這裡
# 明確說明，不能靠形狀漏過。
echo "▶ 解析 CSP source allowlist"
if ! python3 - <<'PY'
import glob
import json
import pathlib
import re
import sys
from html.parser import HTMLParser


class CspMeta(HTMLParser):
    def __init__(self) -> None:
        super().__init__()
        self.policies: list[str] = []

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        if tag.lower() != "meta":
            return
        values = {key.lower(): value for key, value in attrs}
        if (values.get("http-equiv") or "").lower() == "content-security-policy":
            content = values.get("content")
            if content is not None:
                self.policies.append(content)


allowed = {
    "'none'",
    "'self'",
    "'unsafe-inline'",
    "'unsafe-eval'",
    "'wasm-unsafe-eval'",
    "'strict-dynamic'",
    "'report-sample'",
    "data:",
    "blob:",
    "asset:",
    "ipc:",
    "http://ipc.localhost",
}
local_keyword = re.compile(r"'(?:nonce-[A-Za-z0-9+/_=-]+|sha(?:256|384|512)-[A-Za-z0-9+/_=-]+)'\Z")
policies: list[tuple[str, str]] = []
for name in sorted(glob.glob("apps/desktop/ui/*.html")):
    parser = CspMeta()
    parser.feed(pathlib.Path(name).read_text(encoding="utf-8"))
    if len(parser.policies) != 1:
        print(f"✗ {name} 應有一份 CSP meta，實際 {len(parser.policies)} 份", file=sys.stderr)
        sys.exit(1)
    policies.append((name, parser.policies[0]))

def merge_patch(base: object, patch: object) -> object:
    """RFC 7396；Tauri 的 platform config 使用同一種 object merge 語意。"""
    if not isinstance(patch, dict):
        return patch
    merged = dict(base) if isinstance(base, dict) else {}
    for key, value in patch.items():
        if value is None:
            merged.pop(key, None)
        else:
            merged[key] = merge_patch(merged.get(key), value)
    return merged


tauri_name = "apps/desktop/src-tauri/tauri.conf.json"
tauri = json.loads(pathlib.Path(tauri_name).read_text(encoding="utf-8"))
policies.append((tauri_name, tauri["app"]["security"]["csp"]))
for overlay_name in sorted(glob.glob("apps/desktop/src-tauri/tauri.*.conf.json")):
    overlay = json.loads(pathlib.Path(overlay_name).read_text(encoding="utf-8"))
    merged = merge_patch(tauri, overlay)
    try:
        merged_csp = merged["app"]["security"]["csp"]
    except (KeyError, TypeError):
        print(f"✗ {overlay_name} 合併後缺少 WebView CSP", file=sys.stderr)
        sys.exit(1)
    policies.append((f"{overlay_name}（platform merge）", merged_csp))

bad: list[str] = []
for name, policy in policies:
    directives: dict[str, list[str]] = {}
    for raw_directive in policy.split(";"):
        fields = raw_directive.split()
        if not fields:
            continue
        raw_name, *sources = fields
        directive = raw_name.lower()
        if directive in directives:
            bad.append(f"{name} 重複宣告 CSP directive：{directive}")
            continue
        directives[directive] = sources
        for source in sources:
            if source not in allowed and local_keyword.fullmatch(source) is None:
                bad.append(f"{name} 的 {directive} 出現未允許 source：{source}")

    default_sources = directives.get("default-src")
    if default_sources != ["'none'"]:
        rendered = "（缺少）" if default_sources is None else " ".join(default_sources) or "（空）"
        bad.append(f"{name} 的 default-src 必須只有 'none'，實際：{rendered}")

    connect_sources = directives.get("connect-src")
    expected_connect = {"ipc:", "http://ipc.localhost"}
    if connect_sources is None:
        bad.append(f"{name} 缺少 exact connect-src directive")
    elif len(connect_sources) != len(set(connect_sources)):
        bad.append(f"{name} 的 connect-src 有重複 source：{' '.join(connect_sources)}")
    elif set(connect_sources) != expected_connect:
        rendered = " ".join(connect_sources) or "（空）"
        bad.append(
            f"{name} 的 connect-src 必須只有 ipc: 與 http://ipc.localhost，實際：{rendered}"
        )

if bad:
    for message in bad:
        print(f"✗ {message}", file=sys.stderr)
    print("  真的需要遠端來源，先改 PRIVACY.md；否則 CSP 只能使用明列的本機來源。", file=sys.stderr)
    sys.exit(1)
PY
then
    fail=1
fi

if [ "$fail" -ne 0 ]; then
    exit 1
fi

if [ -n "$skipped" ]; then
    echo "⚠ 有東西沒檢查到（未安裝 target）：$skipped"
    echo "  底下這句話只涵蓋真的跑過的那幾棵樹。出貨的是 Windows 執行檔。"
fi
echo "✓ 未授權網路邊界成立：root 預設與 recorder/core/brain/hands 無 HTTP client，desktop 只有 fixed Persona GET 與 fixed Azure POST；installer 內嵌離線 WebView2、無 updater，原始碼無直接 socket API，WebView 只准 IPC"
