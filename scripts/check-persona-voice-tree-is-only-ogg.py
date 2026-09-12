#!/usr/bin/env python3
"""出貨的三個語音資料夾裡，只准有 Ogg、兩份 manifest 和那份 NOTICE。

**這一條守的是隱私，不是品質。** 產生這 952 支的流程在錄音那邊，那裡有一堆
不可以進 public repo 的東西：WAV 母帶、聲音是照著誰的錄音生出來的那份參考
（`refs.json` 與 `refs/*.wav`）、逐骰記著 ASR 聽到什麼的品管收據。它們留在
錄音那邊，出貨的只有解碼得出來的 Ogg 和說得清它怎麼來的那三份檔案。

**為什麼需要一條新的閘門**：既有那六條（三包各自的 assets、loudness、edges、
byte-counts）掃的都是 `rglob("*.ogg")`——它們比對的是「manifest 列的 Ogg 和
資料夾裡的 Ogg 一不一樣」。實測過：把 `refs.json`、一支 `.wav`、一份
`qc-receipt.json` 丟進那三個資料夾，**六條全部照樣印綠**。而那三個檔案在
`git status` 裡是 untracked，`git add -A` 一次就進 public repo 了。

掃的是**檔案系統**不是 `git ls-files`，因為要擋的正是「還沒被 track、但下一個
`git add -A` 會掃進去」那一刻。在 CI 上 checkout 是乾淨的，掃到的就等於 tracked
的那一份——兩種情境同一支腳本都蓋得到。

准許的 Ogg 名單從各包自己的 `manifest.json` 算出來，不寫死子資料夾名字：三包的
版面不一樣（固定台詞是 `base/`＋`extension/`、同意書是每人一個資料夾、互動短句
是單一個 `banter/`），寫死等於「這條檢查只認得我今天看到的那個版面」。

驗收（三刀都真的做出來跑過）：

1. 三個資料夾各丟一個不該在的檔案（`refs.json`、一支 `.wav`、`qc-receipt.json`）
   → 這一條紅、**三個逐檔點名**；既有六條**全綠**——那就是這一條存在的理由。
2. 把那三個檔案刪掉 → 這一條綠（反向也要驗，否則不知道它是不是永遠紅）。
3. 把一支出貨的 `.ogg` 改名成 `.ogg.bak` → 紅兩次（多一個不在 manifest 裡的檔案、
   少一支 Ogg），assets 那條也紅——這一刀證明兩條有重疊，第 1 刀證明它們不相等。

順帶一個做對的小地方：**某一包有問題的時候，那一包不印「544 支，一切正常」**。
紅的行被綠的行蓋過去，讀的人會以為只是別包的事。
"""
from __future__ import annotations

import json
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
PACKS = (
    ("固定台詞", "apps/desktop/ui/persona-voices/v1"),
    ("同意書朗讀", "apps/desktop/ui/persona-consent-voices/v1"),
    ("互動短句", "apps/desktop/ui/persona-banter-voices/v1"),
)
ALLOWED_TOP = ("manifest.json", "manifest.js", "NOTICE.md")

problems: list[str] = []


def fail(message: str) -> None:
    problems.append(message)


def main() -> None:
    for label, rel in PACKS:
        base = ROOT / rel
        if not base.is_dir():
            fail(f"{label}：{rel} 不存在")
            continue
        # 准許的 Ogg 名單從 manifest 自己算出來，不寫死子資料夾名字——三包的版面
        # 不一樣（`base/`＋`extension/`、每人一個資料夾、單一個 `banter/`），寫死
        # 會變成「這條檢查只認得我今天看到的那個版面」。
        manifest = json.loads((base / "manifest.json").read_text(encoding="utf-8"))
        wanted = {clip["file"] for clip in manifest["clips"]}
        if not wanted:
            fail(f"{label}：manifest 一支都沒列，這條檢查等於沒跑")
            continue
        seen = 0
        before = len(problems)
        for path in sorted(base.rglob("*")):
            if path.is_dir():
                continue
            name = path.relative_to(base).as_posix()
            if name in ALLOWED_TOP:
                continue
            if name in wanted:
                seen += 1
                continue
            fail(f"{label}：{rel}/{name} 不該在出貨的資料夾裡")
        if seen != len(wanted):
            fail(f"{label}：manifest 列了 {len(wanted)} 支，資料夾裡只找到 {seen} 支")
        if len(problems) == before:
            # 這一包乾淨才印綠。有問題還印「544 支，一切正常」的話，紅的那一行會
            # 被綠的那一行蓋過去——見 [a-red-run-can-be-red-for-the-wrong-reason]。
            print(f"  {label} {seen} 支 Ogg ＋ manifest.json／manifest.js／NOTICE.md")
        else:
            print(f"  {label} 有 {len(problems) - before} 個問題（見下）")

    if problems:
        print("\n✘ 出貨的語音資料夾裡有不該出現的東西：", file=sys.stderr)
        for line in problems:
            print(f"    {line}", file=sys.stderr)
        print(
            "\n  WAV 母帶、refs.json／refs/*.wav、品管收據都留在錄音那邊。"
            "\n  把它們搬回 voice lab，不要 commit。",
            file=sys.stderr,
        )
        raise SystemExit(1)
    print("✔ 三包都只有 Ogg、兩份 manifest 和 NOTICE.md。")


if __name__ == "__main__":
    main()
