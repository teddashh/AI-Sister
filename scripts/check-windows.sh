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
    # Tauri 會在 build script 階段確認 externalBin 的 target-suffixed 路徑存在，
    # 即使 `cargo check` 根本不會打包或讀它的 PE 內容。正式 Windows job 會把
    # 同一輪通過 smoke 的 sister.exe 複製到這裡並逐 hash 驗；這支 Linux cross
    # check 只需要一個存在性 fixture，而且絕不能覆蓋開發者已經 build 的檔案。
    sidecar="$PWD/target/release/sister-$TARGET.exe"
    sidecar_created=0
    if [[ ! -e "$sidecar" && ! -L "$sidecar" ]]; then
        mkdir -p "$(dirname "$sidecar")"
        touch "$sidecar"
        sidecar_created=1
    fi
    cleanup_desktop_check() {
        rm -rf "$shim"
        if [[ "$sidecar_created" -eq 1 ]]; then
            rm -f -- "$sidecar"
        fi
    }
    trap cleanup_desktop_check EXIT
    if ! command -v llvm-rc >/dev/null 2>&1; then
        ln -s "$PWD/scripts/fake-llvm-rc.py" "$shim/llvm-rc"
    fi

    # `--all-targets` 讓 desktop 的 #[cfg(test)] 接線至少在 Linux cross-check 裡編譯。
    # 斷言仍只有 native Windows/macOS cargo test 會執行，但不能讓新測試到 CI 才
    # 第一次發現自己連型別都不對。
    echo "▶ cargo check --target $TARGET（桌面姊妹，含 test target）"
    (cd "$DESKTOP" && PATH="$shim:$PATH" \
        cargo check --target "$TARGET" --no-default-features --all-targets)

    echo "▶ cargo clippy --target $TARGET（桌面姊妹，含 test target）"
    (cd "$DESKTOP" && PATH="$shim:$PATH" \
        cargo clippy --target "$TARGET" --no-default-features --all-targets -- -D warnings)
fi

echo "✓ Windows 端編譯與 lint 都過了（行為仍需在 Windows 上驗證）"
