#!/usr/bin/env bash
#
# 從 Linux 開發機檢查 Windows 那一半的程式碼。
#
# 為什麼需要這支腳本：`crates/sister-capture/src/windows/` 整個目錄被
# `#[cfg(windows)]` 關著，在 Linux 上跑 `cargo test` 連編都不會編到它。
# 沒有這一步的話，那些程式碼要等到搬上 Windows 機器才第一次被檢查。
#
# `--no-default-features` 是關鍵：預設的 `bundled-sqlite` 會叫
# libsqlite3-sys 用 MSVC 相容的編譯器去編 sqlite3.c，而這台機器上沒有。
# `cargo check` 本來就不連結，關掉它完全不影響型別檢查。
#
# 這支腳本**不能**取代在真正的 Windows 上執行。它只保證「編得過」，
# 不保證「行為正確」——GDI 有沒有真的拍到畫面、hook 有沒有真的收到按鍵，
# 只有那台機器答得出來。
#
# 而且它連「斷言還成不成立」都測不到，因為那些測試只在 Windows 上跑。
# 實際踩過：把 `recognize()` 的一條 `Ok(空的)` 改成 `Err`，這支腳本全綠，
# CI 上的 Windows job 才紅——因為 `tests/windows_ocr.rs` 裡有一條測試
# 寫死了舊的約定。**動到 windows/ 底下的回傳約定時，先 grep 一次
# tests/windows_*.rs**，那些斷言在這台機器上永遠不會執行。

set -euo pipefail

TARGET=x86_64-pc-windows-msvc
cd "$(dirname "$0")/.."

if ! rustup target list --installed | grep -qx "$TARGET"; then
    echo "缺少 target，正在安裝：$TARGET"
    rustup target add "$TARGET"
fi

# fmt 不分平台，放在這裡的理由是：`windows/` 底下的程式碼是最容易在
# 「本機全綠」之後直接推上去的一批——本機根本編不到它，而跑完這支腳本的
# 手感就是「我檢查過了」。實際踩過一次：uia.rs 新增的測試沒排版好，
# 本機 clippy 與 test 全綠，CI 的 Linux job 卡在 Format。
echo "▶ cargo fmt --check"
cargo fmt --all --check

echo "▶ cargo check --target $TARGET"
cargo check --target "$TARGET" --workspace --no-default-features --all-targets

echo "▶ cargo clippy --target $TARGET"
cargo clippy --target "$TARGET" --workspace --no-default-features --all-targets -- -D warnings

# 桌面外殼是**另一個 workspace**（理由見它的 Cargo.toml：Tauri 在 Linux 上要
# webkit2gtk，這台機器沒有、也沒有 sudo 可以裝），所以上面的 `--workspace`
# 完全碰不到它。而它是這個 repo 裡最需要這支腳本的一塊——本機根本跑不起來。
#
# 那個假的 llvm-rc 是為了讓 Tauri 的 build script 過得去，見 fake-llvm-rc.py。
# 它只影響資源檔（執行檔的圖示），而 check 與 clippy 都不連結。
DESKTOP=apps/desktop/src-tauri
if [[ -d "$DESKTOP" ]]; then
    shim="$(mktemp -d)"
    trap 'rm -rf "$shim"' EXIT
    if ! command -v llvm-rc >/dev/null 2>&1; then
        ln -s "$PWD/scripts/fake-llvm-rc.py" "$shim/llvm-rc"
    fi

    echo "▶ cargo check --target $TARGET（字母人）"
    (cd "$DESKTOP" && PATH="$shim:$PATH" cargo check --target "$TARGET" --no-default-features)

    echo "▶ cargo clippy --target $TARGET（字母人）"
    (cd "$DESKTOP" && PATH="$shim:$PATH" \
        cargo clippy --target "$TARGET" --no-default-features -- -D warnings)
fi

echo "✓ Windows 端編譯與 lint 都過了（行為仍需在 Windows 上驗證）"
