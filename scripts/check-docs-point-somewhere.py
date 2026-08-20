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

**`%APPDATA%\\ted-h\\AI-Sister\\data` 這四段全部是從 `config.rs` 的
`default_data_dir()` 推出來的，而且是一次抓完的。** 寫死的話，哪天有人改了那
幾個字串，這支腳本會繼續拿舊的答案去判每一份文件都對。假的那一半要從真的那一
半推出來（見 memory 那條「一個假 DOM 只要有一個欄位比真的寬鬆」）——而且要從
**同一個**真的那一半推：分兩次搜的那一版（同一天寫的）org/app 讀的是整份檔案
裡第一個 `ProjectDirs::from(…)`，那在 config.rs 是 459 行的 `default_path()`，
設定檔那支；accessor 才是從 `default_data_dir()` 讀的。兩個函式各答一半，湊成
一條沒有人產生過的路。實測：只改 `default_path()` 的 app 名，這支腳本會對著八
行完全正確的文件噴紅。

只認**反引號裡**的路徑，不認散文裡的。理由是 `PHASES.md` 有一行寫著
`結構化 grant（task/apps/actions/expiry）`——那是四個欄位名，不是一條路徑，
而任何認得出斜線的正規表示式都會把它抓成違規。一支會誤報的閘門會被關掉，
關掉之後它守的那條線是一格空白。**要它檢查就放進反引號**，那本來就是這幾份
文件的寫法。（`%APPDATA%` 那一圈是例外，它整行掃——那個形狀夠特別，不會誤中。）

**這支腳本守不住的那一半，寫在這裡。**

  - **`%APPDATA%` 以外的絕對路徑一律不看。** `C:\\Program Files\\…`、
    `~/.config/…`、登錄檔路徑寫錯了都不會紅。會挑 `%APPDATA%` 出來是因為它
    是這幾份文件裡唯一一條「他會照著去開檔案總管」的路。
  - **「這條路存在」≠「這條路是對的」。** `docs/PHASES.md` 指得到，但那句話
    指的是不是他要看的那一段，這裡不知道。改名抓得到，改內容抓不到。
  - **只掃 `README.md` 和 `docs/**`。** `crates/**` 裡的 doc comment 沒有人
    掃，`research/` 是刻意排除的（見 `load_docs()`）。
  - 底下那兩個「看了 N 條」是活體檢查的門檻，不是覆蓋率。22 條路 + 8 條
    `%APPDATA%`，是這幾份文件裡**寫成那個形狀**的那些，不是全部。
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

    **`docs/**/*.md` 而不是 `docs/*.md`。** 一層的 glob 遇到一次很普通的整理
    （`git mv docs/WINDOWS-CHECKLIST.md docs/windows/`）就會讓整份檔案不再被
    掃，而唯一的痕跡是底下那個「看了 N 條」少了三——一個沒有人在看的數字。
    實測：搬進子目錄、同時種兩個真的錯誤進去，這支腳本 exit 0、印 ✓。
    """
    found = sorted(ROOT.glob("*.md")) + sorted((ROOT / "docs").glob("**/*.md"))
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
    # **前綴有四段，上一版只推出了中間那兩段。** `%APPDATA%`（漫遊還是本機）
    # 和結尾的 `\data`，決定它們的是 `default_data_dir()` 呼叫哪一個
    # accessor——而那一行以前沒有人讀。把 `d.data_dir()` 改成
    # `d.data_local_dir()`（對一顆會被 OneDrive 同步的螢幕資料庫來說是很合理的
    # 一個改動，而這份清單自己就在擔心 OneDrive 鎖檔），真的位置變成
    # `%LOCALAPPDATA%\…`，文件裡那八行**全部**變成錯的，而這支腳本照樣綠。
    #
    # docstring 承諾的正是這件事不會發生：「假的那一半要從真的那一半推出來」。
    # 推出來的是四段裡的兩段，而沒推出來的那兩段才是會動的那兩段。
    #
    # **而且要一次抓完，不可以分兩次搜。** 分兩次的那一版是這樣壞的：org/app
    # 用的是整份檔案裡**第一個** `ProjectDirs::from(…)`，那在 config.rs 是第
    # 459 行的 `default_path()`——設定檔那支，不是資料目錄那支；accessor 才是
    # 從 `default_data_dir()` 裡讀的。兩個答案來自兩個不同的函式，湊成一條路，
    # 而它們今天一致純粹是因為那兩行的字串剛好一樣。實測：只把 `default_path()`
    # 的 app 名改成 `AI-Sister-Cfg`（資料目錄一個字都沒動），這支腳本對著八行
    # **完全正確**的文件噴紅，說真的位置是 `…\AI-Sister-Cfg\data\…`——一條兩支
    # 函式都不產生的路。同一族：兩個獨立的來源餵同一句話，中間那條「誰算數」的
    # 規則沒有人審過。
    WINDOWS_DIRS = {
        # `directories` 在 Windows 上的攤法（每一個 accessor 一組）：
        "data_dir": ("%APPDATA%", "data"),
        "data_local_dir": ("%LOCALAPPDATA%", "data"),
        "config_dir": ("%APPDATA%", "config"),
        "config_local_dir": ("%LOCALAPPDATA%", "config"),
        "preference_dir": ("%APPDATA%", "config"),
        "cache_dir": ("%LOCALAPPDATA%", "cache"),
    }
    m = re.search(
        r"pub fn default_data_dir\b[^{]*\{\s*"
        r'(?:\w+::)*ProjectDirs::from\(\s*"[^"]*"\s*,\s*"([^"]+)"\s*,\s*"([^"]+)"\s*\)\s*'
        r"\.map\(\s*\|d\|\s*d\.(\w+)\(\)",
        config,
        re.S,
    )
    if m is None:
        die(
            "config.rs 的 default_data_dir() 讀不出資料目錄是怎麼拼出來的",
            "要嘛不再是 ProjectDirs，要嘛換了寫法。推不出來就別猜——",
            "猜錯的話，底下那一圈會拿一條假的路去判每一份文件，而且全部說對。",
        )
    elif m.group(3) not in WINDOWS_DIRS:
        die(
            f"default_data_dir() 用的是 `{m.group(3)}()`，這裡不知道它在 Windows 上攤成什麼",
            "把它加進 WINDOWS_DIRS，順便確認文件裡那幾行跟著改了。",
        )
    else:
        org, app, accessor = m.group(1), m.group(2), m.group(3)
        root, tail = WINDOWS_DIRS[accessor]
        DATA_PREFIX = rf"{root}\{org}\{app}\{tail}"
        print(f"  真的是：{DATA_PREFIX}（來自 {accessor}()）")

# ── 文件裡的每一條路 ─────────────────────────────────────────────────
BACKTICK = re.compile(r"`([^`\n]+)`")
# **兩個都要認，不是只認寫對的那一個。** 只認 `%APPDATA%` 的話，一份改寫成
# `%LOCALAPPDATA%\…` 的文件對這支腳本是完全隱形的（`%LOCALAPPDATA%` 裡面沒有
# `%APPDATA%` 這個子字串，第一個 `%` 後面接的是 `LOCALAPP`）——而那正是
# accessor 改動之後文件會被改成的樣子：改對了一半、或者改錯了邊，兩種都不紅。
# 認得出來才比得了，比不了就只能沉默。
#
# 正斜線也要收。只收反斜線的話，`%APPDATA%/ted-h/AI-Sister/data` 會被切成一個
# 光禿禿的 `%APPDATA%`（group(1) 是 None）然後當成「沒指定路徑」放過去——寫錯了
# 是沉默，寫對了也是沉默，兩種都不紅。今天這幾份文件裡一條正斜線都沒有（量過），
# 收它是為了「哪天有人順手寫成正斜線」那一次不會變成一格空白。
APPDATA = re.compile(r"%(?:LOCAL)?APPDATA%([\\/][A-Za-z0-9_.\\/-]+)?")

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
# 活體。這一圈沒有下限的話，`PREFIX` 打錯一個字、或者條目的形狀變了，它會安靜
# 地一條都挑不到然後回報綠——那正是它要抓的那種壞法，發生在它自己身上。隔壁
# `check-checklist-quotes-exist.py` 同一天寫的，只有那一支裝了。
if checked < 15:
    die(
        f"只挑出 {checked} 條路來對，太少了",
        "2026-08-20 量到 22 條（README + docs/**，反引號裡、對得上 PREFIX/BARE 的）。",
        "多半是 PREFIX / BARE 對不上了，或者 glob 掃不到那幾份檔案。",
    )

print("▶ 每一個 %APPDATA%，要嘛是光禿禿的一個字，要嘛是真的那條路")
seen_appdata = 0
if DATA_PREFIX is not None:
    for rel, lines in DOCS:
        for i, line in enumerate(lines, 1):
            for m in APPDATA.finditer(line):
                whole = m.group(0)
                # 「躺在 `%APPDATA%` 深處」這種講法沒有指定路徑，放它過。
                if m.group(1) is None:
                    continue
                seen_appdata += 1
                # 比之前把分隔符拉齊，不然正斜線那一版會對著正確的路噴紅。
                if whole.replace("/", "\\").startswith(DATA_PREFIX):
                    continue
                die(
                    f"{rel}:{i} 指著一個這台產品上不存在的資料夾：{whole}",
                    line.strip()[:160],
                    f"真的位置是 {DATA_PREFIX}\\…",
                    "他會照著這一行去開檔案總管，找不到，然後回報一個沒有壞的東西壞了。",
                )
    print(f"  看了 {seen_appdata} 條")
    # 活體。這一圈以前連數字都沒有——`APPDATA` 對不上（少認一種寫法、或者文件
    # 改用了正斜線）的時候，它一個都挑不到，而輸出和「八條都對」一模一樣。
    if seen_appdata < 6:
        die(
            f"只挑出 {seen_appdata} 條 %APPDATA% 路徑來對，太少了",
            "光是 WINDOWS-CHECKLIST.md 就有七個，README 和 DATA_INVENTORY 各一。",
            "多半是 APPDATA 這條正規表示式對不上文件現在的寫法了。",
        )

print()
if failed:
    sys.exit(1)
print("✓ 文件裡指出去的每一條路都指得到東西")
