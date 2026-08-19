#!/usr/bin/env python3
"""一個什麼都不做的 Windows 資源編譯器，**只給 `cargo check` / `cargo clippy` 用**。

為什麼需要它：Tauri 的 build script 會把圖示與版本資訊編譯成 Windows 資源
再嵌進執行檔，而那一步需要 `llvm-rc`。這台 Linux 開發機上沒有這個工具
（`rustup component add llvm-tools` 也不含它），也沒有 sudo 可以裝，於是
`apps/desktop/src-tauri` 在這裡連**型別檢查**都跑不起來——一個 build script
的 panic 擋掉了整個 crate 的編譯。

那個代價是不成比例的：資源檔只影響「執行檔的圖示長怎樣」，而我們想在本機
問的是「這 250 行 Rust 型別對不對」。`cargo check` 根本不連結，所以一個
空的輸出檔完全足夠。

**它不會讓 `cargo build` 產出正確的執行檔。** 真正的建置在 Windows 上做，
那裡有真的 `llvm-rc`。這支腳本只在 `scripts/check-windows.sh` 裡被塞進
PATH，而那支腳本只跑 check 與 clippy。
"""

import pathlib
import sys


def outputs(args: list[str]) -> list[str]:
    found: list[str] = []
    for i, arg in enumerate(args):
        lowered = arg.lower()
        if lowered in ("/fo", "-fo", "--output", "-o") and i + 1 < len(args):
            found.append(args[i + 1])
        elif lowered.startswith(("/fo", "-fo")) and len(arg) > 3:
            found.append(arg[3:])
        elif lowered.endswith((".res", ".lib", ".o", ".obj")):
            found.append(arg)
    return found


def main() -> int:
    for out in outputs(sys.argv[1:]):
        path = pathlib.Path(out)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
