#!/usr/bin/env python3
"""文件裡指出去的每一條路，要指得到東西。

這支腳本守兩件事，兩件都是 2026-08-20 那天當場踩到的：

1. **`docs/WINDOWS-CHECKLIST.md` 裡有五行寫著 `%APPDATA%\\sister`。**
   真的位置是 `%APPDATA%\\ted-h\\AI-Sister\\data`，而同一份檔案的第 52 行和
   第 443 行本來就寫對了——五行錯、三行對，全部在同一個檔案裡。那份清單是
   他照著一條一條做的，所以錯的那幾行的後果不是「讀起來怪怪的」，是**他打開
   檔案總管、找不到那個資料夾、然後回報一個沒有壞的東西壞了**。

2. **`scripts/check-combo-is-readable.sh` 改名成 `.py` 之後，清單裡還指著
   `.sh`。** 這一種更安靜：那句話讀起來完全正常，只有真的去 `ls` 才知道。

兩個都是同一個形狀——**一份文件在描述一個它不再對得上的世界**，而沒有任何
東西會為此變紅。清單是這個 repo 唯一一份「只有他那台機器執行得了」的測試，
所以它自己壞掉的時候，壞的是那一整輪驗證。

**這裡的 `%APPDATA%` 前綴不是寫死的。** 它從 `config.rs` 那行
`ProjectDirs::from("com", "ted-h", "AI-Sister")` 推出來——寫死的話，哪天有人
改了那三個字串，這支腳本會繼續拿舊的答案去判每一份文件都對。假的那一半要從
真的那一半推出來（見 memory 那條「一個假 DOM 只要有一個欄位比真的寬鬆」）。

只認**反引號裡**的路徑，不認散文裡的。理由是 `PHASES.md` 有一行寫著
`結構化 grant（task/apps/actions/expiry）`——那是四個欄位名，不是一條路徑，
而任何認得出斜線的正規表示式都會把它抓成違規。一支會誤報的閘門會被關掉，
關掉之後它守的那條線是一格空白。**要它檢查就放進反引號**，那本來就是這幾份
文件的寫法。
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# 這幾個開頭才算「repo 裡的一條路」。`target/` 刻意不在裡面：那是建置產物，
# 乾淨的 checkout 上本來就沒有。
PREFIX = ("scripts/", "docs/", "crates/", "apps/", ".github/")

# 光禿禿的一個檔名也要認。`PHASES.md:114` 寫的是 `check-no-network.sh`，沒有
# `scripts/` 前綴——只認前綴的話，那一行改名之後不會有任何東西紅。這三種副檔名
# 在這個 repo 裡**只出現在 `scripts/` 底下**（`find` 過），所以往那裡解析是安全
# 的；`.rs` 不在名單裡，因為散文裡的 `main.rs` / `ops.rs` 對得到好幾個檔案。
BARE = re.compile(r"^[A-Za-z0-9_-]+\.(?:mjs|py|sh)$")

failed = []


def die(msg, *rest):
    failed.append(msg)
    print(f"✗ {msg}")
    for line in rest:
        print(f"  {line}")


def read(rel):
    """讀一份檔案。找不到就當場算輸，不是「沒找到違規」。"""
    path = ROOT / rel
    if not path.is_file():
        die(
            f"要掃的檔案不在：{rel}",
            "在修好之前，這支腳本守的那條線是一格空白。",
        )
        return None
    return path.read_text(encoding="utf-8")


def load_docs():
    """要掃的那幾份，連內容一起讀出來。

    空的話當場算輸——glob 打錯一個字，底下每一圈都是空轉，而輸出長得和
    「掃過了，沒事」一模一樣。讀一次收在這裡，兩圈共用：分兩次 glob 的話，
    「一份都沒掃到」會被講兩遍，而那則訊息講的是同一件事。

    **`research/` 是刻意不掃的**，別「順手」加進來：那底下有一份
    `ted-repos-dna.md` 在 `.gitignore` 裡（它抄的是**別的 repo** 的路徑，像
    `apps/companion/src/main/pet/pet.ts`）。加進來的話本機會亮一排紅、而 CI
    上那個檔案根本不存在所以照樣綠——一道兩邊答案不一樣的閘門，比沒有更糟。
    """
    found = sorted(ROOT.glob("*.md")) + sorted((ROOT / "docs").glob("*.md"))
    if not found:
        die("一份 .md 都沒掃到", "glob 對不到東西的時候，底下每一圈都是空轉。")
    return [
        (d.relative_to(ROOT), d.read_text(encoding="utf-8").split("\n")) for d in found
    ]


DOCS = load_docs()


# ── 真的資料目錄長什麼樣 ─────────────────────────────────────────────
#
# 從產生它的那一行推出來，不要寫死。
print("▶ 那個 %APPDATA% 前綴，從 config.rs 推出來")
DATA_PREFIX = None
config = read("crates/sister-core/src/config.rs")
if config is not None:
    m = re.search(
        r'ProjectDirs::from\(\s*"[^"]*"\s*,\s*"([^"]+)"\s*,\s*"([^"]+)"\s*\)', config
    )
    if m is None:
        die(
            "config.rs 裡找不到 ProjectDirs::from(…)",
            "資料目錄改由別的東西決定了，而底下那一圈還在拿舊的答案判每一份文件。",
        )
    else:
        org, app = m.group(1), m.group(2)
        # `directories` 在 Windows 上把 data_dir 攤成
        # `{RoamingAppData}\{organization}\{project}\data`。
        DATA_PREFIX = rf"%APPDATA%\{org}\{app}\data"
        print(f"  真的是：{DATA_PREFIX}")

# ── 文件裡的每一條路 ─────────────────────────────────────────────────
BACKTICK = re.compile(r"`([^`\n]+)`")
APPDATA = re.compile(r"%APPDATA%(\\[A-Za-z0-9_.\\-]+)?")

print("▶ 反引號裡的 repo 路徑，指得到東西嗎")
checked = 0
for rel, lines in DOCS:
    for i, line in enumerate(lines, 1):
        for m in BACKTICK.finditer(line):
            token = m.group(1).strip()
            # 佔位符、萬用字元、帶參數的指令一律跳過——它們本來就指不到單一檔案。
            if any(c in token for c in "*<>{}… ()"):
                continue
            token = token.rstrip(".,;:，。、")
            if BARE.match(token):
                token = f"scripts/{token}"
            elif not token.startswith(PREFIX):
                continue
            checked += 1
            if not (ROOT / token).exists():
                die(
                    f"{rel}:{i} 指著一個不在的東西：{token}",
                    line.strip()[:160],
                    "改名或刪掉之後，指著它的那幾句話不會有任何東西幫你找出來。",
                )
print(f"  看了 {checked} 條")

print("▶ 每一個 %APPDATA%，要嘛是光禿禿的一個字，要嘛是真的那條路")
if DATA_PREFIX is not None:
    for rel, lines in DOCS:
        for i, line in enumerate(lines, 1):
            for m in APPDATA.finditer(line):
                whole = m.group(0)
                # 「躺在 `%APPDATA%` 深處」這種講法沒有指定路徑，放它過。
                if m.group(1) is None:
                    continue
                if whole.startswith(DATA_PREFIX):
                    continue
                die(
                    f"{rel}:{i} 指著一個這台產品上不存在的資料夾：{whole}",
                    line.strip()[:160],
                    f"真的位置是 {DATA_PREFIX}\\…",
                    "他會照著這一行去開檔案總管，找不到，然後回報一個沒有壞的東西壞了。",
                )

print()
if failed:
    sys.exit(1)
print("✓ 文件裡指出去的每一條路都指得到東西")
