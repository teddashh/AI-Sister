#!/usr/bin/env python3
"""出貨的資產資料夾裡，只准有它自己 manifest 列的檔案，加上幾份說明檔。

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
"""
from __future__ import annotations

import json
import pathlib
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

problems: list[str] = []


def fail(message: str) -> None:
    problems.append(message)


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

    if problems:
        print("\n✘ 出貨的資產資料夾裡有不該出現的東西：", file=sys.stderr)
        for line in problems:
            print(f"    {line}", file=sys.stderr)
        print(
            "\n  WAV 母帶、refs.json／refs/*.wav、品管收據都留在錄音那邊。"
            "\n  把它們搬回 voice lab，不要 commit。",
            file=sys.stderr,
        )
        raise SystemExit(1)
    print("✔ 四棵樹都只有 manifest 列的檔案和那幾份說明。")


if __name__ == "__main__":
    main()
