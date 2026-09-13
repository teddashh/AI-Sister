#!/usr/bin/env python3
"""錄音那邊的私有素材，一個都不可以在這個 public repo 裡。

**這一條守的是隱私，不是品質。** 產生那 952 支語音的流程在錄音那邊，那裡有一堆
不可以進 public repo 的東西：WAV 母帶、聲音是照著誰的錄音生出來的那份參考
（`refs.json` 與 `refs/*.wav`）、逐骰記著 ASR 聽到什麼的品管收據。它們留在錄音
那邊，出貨的只有解碼得出來的檔案和說得清它怎麼來的那幾份說明。

**為什麼需要這一條**：三包語音各自那條 assets 檢查、loudness、edges、byte-counts
掃的都是 `rglob("*.ogg")`——它們比對的是「manifest 列的 Ogg」和「資料夾裡的
Ogg」，看不見不是 Ogg 的東西。實測過：把 `refs.json`、一支 `.wav` 和一份
`qc-receipt.json` 丟進那三個資料夾，**六條全部照樣印綠**。而那三個檔案在
`git status` 裡是 untracked，一次 `git add -A` 就進 public repo 了。

**拿同一把刀試過每一個兄弟資料夾**（各丟一個 `refs.json` 再跑八條檢查）：

```text
apps/desktop/ui/persona-reels     check-persona-reel-assets.py 抓到   → 不用管
apps/desktop/src-tauri/icons      check-app-icons.py 抓到             → 不用管
apps/desktop/ui/personas          八條全綠                            → 這裡補
apps/desktop/ui/persona-*voices   八條全綠                            → 這裡補
```

reels 那條用的正是這個形狀（`rglob("*")` ＋ 從 manifest 算出准許名單），所以這裡
不重複守它——同一個判準寫兩處而不同步是隱形的，那兩棵樹留給它們自己那條。

准許的檔名從各自的 `manifest.json` 算出來，不寫死子資料夾名字：四棵樹的版面都不
一樣（固定台詞是 `base/`＋`extension/`、同意書是每人一個資料夾、互動短句是單一個
`banter/`、頭像是平的一層），寫死等於「這條檢查只認得我今天看到的那個版面」。

驗收（都真的做出來跑過）：

1. 三個語音資料夾各丟一個不該在的檔案（`refs.json`、一支 `.wav`、
   `qc-receipt.json`）→ 這一條紅、**三個逐檔點名**；既有六條**全綠**。
2. `personas/` 丟一個 `refs.json` → 這一條紅；八條既有檢查全綠。
3. 把那些檔案刪掉 → 這一條綠（反向也要驗，否則不知道它是不是永遠紅）。
4. 把一支出貨的 `.ogg` 改名成 `.ogg.bak` → 紅兩次（多一個不在 manifest 裡的檔案、
   少一支），assets 那條也紅——這一刀證明兩條有重疊，第 1 刀證明它們不相等。

順帶一個做對的小地方：**某一棵樹有問題的時候，那一棵不印「544 支，一切正常」**。
紅的行被綠的行蓋過去，讀的人會以為只是別棵樹的事。

---

**第二件事（後補）：上面那四棵樹之外，一樣一個都不可以。** 上面整段講的是
「資料夾裡不准有多的東西」，而那句話的主詞是四個資料夾。實測把同樣三種私有
檔案放在六個地方——repo 根目錄、`docs/`、`apps/desktop/ui/`，以及
`apps/desktop/ui/persona-voices/`、`persona-consent-voices/refs/`、
`persona-banter-voices/` 這三個**只差一層、就在出貨資料夾上面**的位置——
把 `scripts/check-*.py` 和 `check-*.mjs` 全部跑一遍，**沒有任何一條紅**
（唯一那條紅的 `check-windows-signing-receipt.py` 是缺參數，對照組一樣紅）。
`.gitignore` 那時也沒有任何一行和聲音有關。

所以再加一道，掃的是 `git ls-files`（追蹤中的檔案，不是工作區）：

1. **副檔名白名單。** 出貨的音檔只有 Ogg，別種音訊格式一個都沒有。
   **這裡刻意寫成白名單**：黑名單只看得到我今天想得到的形狀，而錄音那邊還有
   mp3、BreezyVoice 的 `.dur`、模型 cache。白名單逼下一個人在這裡寫下「這是
   什麼、為什麼可以出貨」，不是只逼他看一眼。
2. **共用副檔名的那幾種名字。** `refs.json`、`refs/` 底下的東西、品管收據
   （`*-qc.json`、`qc-*.json`）、`*.visemes.json`、`*-argmax.json` 都是 `.json`，
   白名單看不見它們。

**它擋不住什麼**（每一條都做出來跑過，`want=綠`）：

```text
docs/sample.ogg（把私有 WAV 改名成 .ogg 放在四棵樹外面）  → 綠。副檔名合法、
    路徑不在任何一棵樹裡，這條看不出它是什麼。
把一份收據的內容貼進 RELEASE-NOTES.md                     → 綠。它看檔名不看內容。
`git add -f` 之前的工作區                                  → 看不到。它掃的是
    追蹤中的檔案，未追蹤的那半由 `.gitignore` 顧（而 `.gitignore` 擋不住 -f）。
```

前兩條是真的洞，記在這裡而不是假裝沒有；第三條是分工，兩邊合起來才是完整的
——**預防在 `.gitignore`，證明在這裡**。對 public repo 來說 CI 抓到已經晚了
（推上去的那一刻 blob 就在 GitHub 上了），所以兩半都要有。

驗收（這一段，四刀）：`refs.json`／`x.wav`／`banter-voice-v1-qc.json` 各 `git add`
一次 → 各自紅、逐檔點名說出它是什麼；`git rm --cached` 還原 → 綠。
"""
from __future__ import annotations

import json
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent

# (中文標籤, 資料夾, manifest 裡放清單的那個鍵, 除了清單以外還准許的檔名)
TREES = (
    ("固定台詞", "apps/desktop/ui/persona-voices/v1", "clips",
     ("manifest.json", "manifest.js", "NOTICE.md")),
    ("同意書朗讀", "apps/desktop/ui/persona-consent-voices/v1", "clips",
     ("manifest.json", "manifest.js", "NOTICE.md")),
    ("互動短句", "apps/desktop/ui/persona-banter-voices/v1", "clips",
     ("manifest.json", "manifest.js", "NOTICE.md")),
    ("角色頭像", "apps/desktop/ui/personas", "assets",
     ("manifest.json", "catalog.js", "NOTICE.md")),
)

# 追蹤中的檔案只准是這些副檔名。要加一種就要在這裡寫下它是什麼——寫不出來
# 就是它不該出貨。（`.gitignore`／`.gitattributes`／`LICENSE` 沒有副檔名，走
# 下面 ALLOWED_NAMES。）
ALLOWED_SUFFIXES = {
    ".ogg": "出貨的語音，三包共 952 支——**音檔只有這一種**",
    ".png": "角色 reel 的畫格與 app icon",
    ".webp": "頭像的退路圖",
    ".ico": "Windows 的 app icon",
    ".rs": "Rust",
    ".py": "腳本與閘門",
    ".mjs": "node 寫的閘門",
    ".js": "WebView 的前端與 manifest 的 JS 版",
    ".json": "manifest、catalog、夾具",
    ".md": "文件",
    ".html": "WebView 的頁面與 site/",
    ".css": "WebView 的樣式",
    ".toml": "Cargo",
    ".lock": "Cargo.lock（兩份）",
    ".sh": "腳本",
    ".ps1": "Windows 那半的 PowerShell",
    ".nsh": "NSIS 的 hook 與語系字串",
    ".nsi": "NSIS 安裝檔模板",
    ".yml": "GitHub Actions",
    ".yaml": "skill 的 agent 設定",
    ".plist": "macOS 的 Info.plist",
}
ALLOWED_NAMES = {".gitignore", ".gitattributes", "LICENSE"}

# 白名單看不見的那幾種：它們和出貨的檔案共用 `.json`。
PRIVATE_SHAPES = (
    (re.compile(r"(^|/)refs\.json$"), "聲音是照著誰的錄音生出來的那份參考"),
    (re.compile(r"(^|/)refs/"), "參考錄音（`refs/*.wav`）"),
    (re.compile(r"(^|/)([^/]*[-_.])?qc([-_.][^/]*)?\.json$"), "逐骰記著 ASR 聽到什麼的品管收據"),
    (re.compile(r"\.visemes\.json$"), "reel 對嘴用的中間產物"),
    (re.compile(r"-argmax\.json$"), "聲紋比對的中間產物"),
)

problems: list[str] = []


def fail(message: str) -> None:
    problems.append(message)


def private_material_tracked_anywhere() -> None:
    """四棵樹之外也要掃一次。掃的是追蹤中的檔案，不是工作區。"""
    listed = subprocess.run(
        ["git", "ls-files", "-z"], cwd=ROOT, check=True,
        capture_output=True, text=True).stdout
    tracked = [name for name in listed.split("\0") if name]
    if not tracked:
        # 空結果分不出「真的是 0」和「我問錯了」，所以它要吵。
        fail("`git ls-files` 一個檔案都沒回，這條檢查等於沒跑")
        return
    before = len(problems)
    for name in tracked:
        path = pathlib.PurePosixPath(name)
        if path.name not in ALLOWED_NAMES and path.suffix not in ALLOWED_SUFFIXES:
            fail(f"{name}：`{path.suffix or path.name}` 不在准許出貨的副檔名裡"
                 f"——如果它真的該出貨，去 ALLOWED_SUFFIXES 寫下它是什麼")
            continue
        for pattern, what in PRIVATE_SHAPES:
            if pattern.search(name):
                fail(f"{name}：{what}，留在錄音那邊")
                break
    if len(problems) == before:
        print(f"  整個 repo {len(tracked)} 個追蹤檔，"
              f"{len(ALLOWED_SUFFIXES)} 種准許的副檔名，沒有錄音那邊的東西")
    else:
        print(f"  整個 repo 有 {len(problems) - before} 個問題（見下）")


def main() -> None:
    for label, rel, key, allowed in TREES:
        base = ROOT / rel
        if not base.is_dir():
            fail(f"{label}：{rel} 不存在")
            continue
        manifest = json.loads((base / "manifest.json").read_text(encoding="utf-8"))
        listed = manifest.get(key)
        if not listed:
            fail(f"{label}：manifest 的 `{key}` 一筆都沒有，這條檢查等於沒跑")
            continue
        wanted = {entry["file"] for entry in listed}
        seen = 0
        before = len(problems)
        for path in sorted(base.rglob("*")):
            if path.is_dir():
                continue
            name = path.relative_to(base).as_posix()
            if name in allowed:
                continue
            if name in wanted:
                seen += 1
                continue
            fail(f"{label}：{rel}/{name} 不該在出貨的資料夾裡")
        if seen != len(wanted):
            fail(f"{label}：manifest 列了 {len(wanted)} 個檔案，資料夾裡只找到 {seen} 個")
        if len(problems) == before:
            print(f"  {label} {seen} 個檔案 ＋ {'／'.join(allowed)}")
        else:
            print(f"  {label} 有 {len(problems) - before} 個問題（見下）")

    private_material_tracked_anywhere()

    if problems:
        print("\n✘ 有不該出現在這個 public repo 裡的東西：", file=sys.stderr)
        for line in problems:
            print(f"    {line}", file=sys.stderr)
        print(
            "\n  WAV 母帶、refs.json／refs/*.wav、品管收據都留在錄音那邊。"
            "\n  把它們搬回 voice lab，不要 commit。",
            file=sys.stderr,
        )
        raise SystemExit(1)
    print("✔ 四棵樹都只有 manifest 列的檔案和那幾份說明，樹外面也沒有錄音那邊的東西。")


if __name__ == "__main__":
    main()
