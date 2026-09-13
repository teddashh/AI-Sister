#!/usr/bin/env python3
"""他按下同意的那句話，和她唸出來的那一段，不可以是兩件事。

同意書的四句話同時存在四個地方：`consent.rs` 的 `wording()`（Rust 這一端的
權威）、`onboarding.js`（畫面上他讀到的）、`catalog-v1.json`（錄音那邊的稿）、
以及 `persona-consent-voices/v1/manifest.json` 的 `text`（68 支語音各自標的台
詞）。這四份**已經互相釘死了**：`check-consent-copy.py` 比前兩份逐字，
`check-persona-consent-voice-assets.py` 把後兩份對回 `consent.rs`。

**那條鏈到 `text` 就斷了。** `text` 是一個欄位，不是聲音。68 支 Ogg 的位元組由
manifest 的 `sha256` 釘住，而沒有任何一條檢查問過「這些位元組唸的是不是這四句
話」。實測：把四句話裡任何一句改掉，同步改 `consent.rs`、`onboarding.js`、
`catalog-v1.json` 與 `manifest.json`／`.js` 的 `text`，**所有既有閘門照樣印綠**，
而出貨的音檔一個位元組都沒動——他讀到的是新句子，她唸出來的是舊句子。

這在同意書上特別要命：那是整個產品裡唯一一句「他按下去就算數」的話。

**能不能從聲音本身量回來？不能，量過了。** 唯一不需要模型的儀器是時長，而它
是**長度**的函數，最危險的改法卻是等長的：把「永不送出畫面檔」改成「會送出畫面
檔」只少一個字（131 字裡的 0.8%）。實測四句的「中位時長／字數」是 194／159／
202／169 ms，本來就散在 ±13%，而同一句 17 個人唸的時長最寬離中位 21%–47%。
一條抓得到 0.8% 的帶子在現況上就會紅。真的要問「她唸的是什麼」得跑 ASR，那要
GPU，不在 CI 上——那一關在錄音那邊的雙 ASR QC。

**所以這一條問的是另一個問題，而這個問題答得出來：聲音是不是在文案之後才生出
來的。** 兩邊都從 git 歷史算「最後一次真的變了是哪一顆 commit」：

- 文案：走過所有碰過 `consent.rs` 的 commit，逐顆把四句話抽出來和它父節點比，
  第一顆不一樣的就是。**碰過檔案不算，字真的變了才算**——`consent.rs` 為了別的
  理由改過很多次。
- 聲音：走過所有碰過同意書 manifest 的 commit，比的是 68 個 `sha256` 的**集合**。
  同樣地，改 `text`、改欄位順序、改 `durationMs` 都不算；音檔位元組真的換了才算。

然後要求前者是後者的祖先（或同一顆）。正常的 promote 兩件事在同一顆 commit 裡
發生，過。手改文案而沒重錄，文案那顆比聲音那顆新，紅——**而且沒有辦法用手改繞
過去**：唯一能讓「聲音那顆」往前走的方法，是真的換掉音檔的位元組。

它證得了什麼、證不了什麼：

- 證得了：**這批聲音是在現在這四句話定案之後才生出來的。** 這是「逼你改」不是
  「逼你看一眼」——`git log` 說的事偽造不掉。
- 證不了：**這批聲音唸的就是這四句話。** 拿另一包的音檔換進來、hash 全部跟著
  變，這一條照樣綠。那一半是錄音那邊雙 ASR QC 的事（切完重新轉寫，台詞的字一個
  都不准少），而它需要 GPU。

驗收（四刀，都在 /home/ted-h/tmp-tests 的臨時 clone 上做，主 repo 沒有被碰）：

1. 只改文案就 commit（四個地方全部同步改、音檔不動）→ 紅，點名文案那顆比聲音
   那顆新。既有的六條同意書相關閘門在同一棵樹上**全部印綠**——這一刀同時證明了
   上面說的那個洞是真的。
2. 同一顆 commit 裡連 `sha256` 一起換掉 → 綠（正常 promote 的形狀）。
3. 淺 clone（`--depth 1`）→ 紅，明講「問不到歷史」。**問不到不可以當成通過。**
4. `want=綠` 的對照：把 `consent.rs` 改一個和四句話無關的地方再 commit → 綠，
   證明「碰過檔案」和「字變了」真的分得開。
"""

from __future__ import annotations

import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CONSENT = "crates/sister-core/src/consent.rs"
MANIFEST = "apps/desktop/ui/persona-consent-voices/v1/manifest.json"


def git(*args: str) -> str:
    result = subprocess.run(["git", *args], cwd=ROOT, capture_output=True, text=True)
    if result.returncode != 0:
        raise RuntimeError(f"git {' '.join(args)} 失敗：{result.stderr.strip()[:200]}")
    return result.stdout


def blob(rev: str, path: str) -> str | None:
    """某一顆 commit 上那個檔案的內容；那時候還沒有這個檔就回 None。"""
    result = subprocess.run(["git", "show", f"{rev}:{path}"], cwd=ROOT,
                            capture_output=True, text=True)
    return result.stdout if result.returncode == 0 else None


def wordings(rev: str):
    """那一顆 commit 上的四句同意書。抽不出四句就回 None（等於「那時候不是這個形狀」）。"""
    source = blob(rev, CONSENT)
    if source is None or "pub fn wording" not in source:
        return None
    body = source.split("pub fn wording", 1)[1].split("pub fn without", 1)[0]
    found = re.findall(r'=>\s*(?:\{\s*)?"((?:[^"\\]|\\.)*)"', body, re.DOTALL)
    if len(found) != 4:
        return None
    return tuple(json.loads(f'"{value}"') for value in found)


def clip_hashes(rev: str):
    """那一顆 commit 上 68 支語音的 sha256 集合。"""
    raw = blob(rev, MANIFEST)
    if raw is None:
        return None
    try:
        clips = json.loads(raw)["clips"]
        return tuple(sorted(clip["sha256"] for clip in clips))
    except (json.JSONDecodeError, KeyError, TypeError):
        return None


def last_real_change(path: str, extract):
    """最後一次那個**值**真的變了是哪一顆 commit。碰過檔案不算。"""
    revs = git("log", "--format=%H", "--", path).split()
    if not revs:
        return None, f"`git log -- {path}` 一顆 commit 都沒有"
    for rev in revs:
        if extract(rev) != extract(f"{rev}^"):
            return rev, git("log", "-1", "--format=%h %ad %s", "--date=short", rev).strip()
    # 走到底都沒變過＝從第一顆就是現在這個值。取最舊那顆。
    return revs[-1], git("log", "-1", "--format=%h %ad %s", "--date=short", revs[-1]).strip()


def main() -> int:
    if git("rev-parse", "--is-shallow-repository").strip() == "true":
        print("✗ 這是淺 clone，問不到歷史——而「問不到」不可以印成綠燈。")
        print("  CI 要在這個 job 的 actions/checkout 加 `with: fetch-depth: 0`。")
        return 1

    word_rev, word_line = last_real_change(CONSENT, wordings)
    clip_rev, clip_line = last_real_change(MANIFEST, clip_hashes)
    if word_rev is None or clip_rev is None:
        print(f"✗ 算不出來：{word_line if word_rev is None else clip_line}")
        return 1

    print(f"  文案最後一次真的變：{word_line}")
    print(f"  聲音最後一次真的變：{clip_line}")

    ordered = subprocess.run(["git", "merge-base", "--is-ancestor", word_rev, clip_rev],
                             cwd=ROOT, capture_output=True)
    if ordered.returncode != 0:
        print()
        print("✗ 同意書的字改過了，而她唸的那 68 支聲音沒有跟著換。")
        print("  他讀到的是新句子，她唸出來的是舊句子——這是整個產品裡唯一一句")
        print("  「他按下去就算數」的話，兩邊不可以是兩件事。")
        print(f"  要嘛重跑 promote 把錄音那邊新錄的搬過來，要嘛把 {CONSENT} 的字改回去。")
        return 1

    print("✔ 她唸的那 68 支，是在現在這四句話定案之後才生出來的。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
