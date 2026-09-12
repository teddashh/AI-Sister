#!/usr/bin/env python3
"""出貨的每一支語音，開口之前和講完之後不准掛著一大段空白。

**這一條也是為了一句使用者說過的話存在的**：「前後文都沒有切乾淨。」alpha.137
以前出貨的 952 支裡，有 45 支（4.7%）的開頭或結尾掛著超過 300 毫秒沒有內容的
聲音——最長的一支開頭空了 840 毫秒、另一支結尾空了 1350 毫秒。那不是靜音，是
比台詞低三十幾 dB 的房間噪聲，聽得到，而且會讓她聽起來慢半拍。

alpha.138 把它們切掉了（切點由能量包絡算，Whisper 只做事後驗證）。切掉這件事
發生在錄音那邊，repo 這裡看不到；**看得到的是結果**，所以這一條量結果。

量法（和實驗室那邊同一條）：把出貨的 Ogg 解開來，以 10 毫秒為一格算 RMS，比
這一支自己最大的那一格低 `REL_DB` 以上就算「沒在講話」，**連續兩格**才算聲音
真的開始或結束——一格喀噠不可以決定切點。門檻是相對的：絕對音量 alpha.137 已
經對齊了，但氣音和爆音的動態差很多。

守兩件事：

1. **每一包至少 `SHARE_MIN` 的片段，兩端的空白都在 `LIMIT_MS` 以內。** 用比例
   而不是逐支，是因為有幾支是切不掉的（見下面「擋不住的」）；用比例才分得出
   「有幾支切不掉」和「整包根本沒切過」。
2. **沒有任何一支的任一端超過 `HARD_MS`。** 比例守不住「544 支裡有一支壞得很
   離譜」——一支不會把 99.4% 拉到 99% 以下。

驗收（四刀，全部從檔案備份還原、還原後逐檔比對過）：

1. 三包全換回 alpha.137 出貨的 Ogg → 三包都紅（94.7%／98.5%／95.6%），外加
   venice/choose-topic 的 1350 毫秒撞到硬上限。
2. 只換回固定台詞那一包 → 只有那一包紅，另外兩包照常印綠。
3. 一支出貨的 Ogg 前面接 1.5 秒房間噪聲 → 硬上限那條紅，點名 claude/good-morning
   的 1520 毫秒；比例那條沒紅（544 支裡壞一支動不了 99%）。兩條規則分得開。
4. banter 三支各接 0.5 秒 → 比例那條紅（98.5%），硬上限沒紅。

**第四刀第一次沒打中，而且是「刀太大聲」**：我第一版接的噪聲比台詞只低 23 dB，
這條規則就把它算成「有在講話」——沒錯，那本來就不是空白。降到低 45 dB 才是這條
規則要抓的東西。連續量的規則（dB、毫秒）下刀之前要先算「要多小才會越線」。
還有一件：一支不夠。340 支裡要壞 **3** 支比例才會掉到 99% 以下。

擋不住的（打過一刀 `want=綠` 確認過，不是猜的）：

- **切到台詞了。** 把 claude「早安。我們把邊界看清楚，今天先挑一件事就好。」的
  尾巴砍掉 500 毫秒重編，這一條 **exit=0 印綠燈**——那一端確實更乾淨了。而
  Whisper 聽到的從「……今天先挑一件事就好」變成「……今天先挑一件事」，最後兩個字
  不見了。守這件事的是錄音那邊的雙 ASR QC（切完重新轉寫，台詞的字一個都不准少），
  那要 GPU，不在 CI 上。
- **編碼器自己造出來的空白。** 現在剩下的 5 支裡有 4 支是這種——kimi「午安……」的
  結尾在整平後的 WAV 上只有 60 毫秒低於門檻，解出來的 Opus 上是 960 毫秒，因為
  40 kbps 的 Opus 會把邊界那些很小聲的東西再壓低十幾 dB（實測那一段從 −35.6 dB
  掉到 −48.0 dB）。這一條分不出「錄音真的掛著房間噪聲」和「編碼器把已經很小聲的
  東西壓得更小聲」，兩種在出貨的檔上長得一模一樣。它守的是量級，不是成因。
"""

from __future__ import annotations

import array
import concurrent.futures
import json
import math
import os
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
UI = ROOT / "apps" / "desktop" / "ui"

SETS = (
    ("固定台詞", UI / "persona-voices" / "v1"),
    ("同意書朗讀", UI / "persona-consent-voices" / "v1"),
    ("互動短句", UI / "persona-banter-voices" / "v1"),
)

# 解到這個取樣率再算能量。出貨的 Opus 名目是 24 kHz；RFC 7845 規定解碼一律是
# 48 kHz，所以這裡明講要 24000，兩台機器才會算出同一格。
RATE = 24000
FRAME = RATE // 100  # 10 毫秒

# 比這一支自己最大的那一格低這麼多就算「沒在講話」。−35 dB 是實驗室那邊找切點
# 用的同一個數字：台詞的氣音尾巴大約落在 −25 到 −30，房間噪聲落在 −35 以下。
REL_DB = -35.0

# 一端的空白超過這裡就算這一支沒切乾淨。300 毫秒大約是一個字的長度，聽得出來。
LIMIT_MS = 300

# 每一包至少這個比例要兩端都在 LIMIT_MS 以內。alpha.138 實測 99.4%／100%／99.4%，
# alpha.137 是 94.7%／98.5%／95.6%。留的餘裕：固定台詞現在超標 3 支，再多 2 支還
# 是綠的，第 6 支才紅；互動短句現在 2 支，第 4 支就紅（它只有 340 支）。
SHARE_MIN = 0.99

# 沒有任何一支可以超過這裡。alpha.138 實測最長的一端是 960 毫秒（kimi「午安……」
# 的結尾，成因是 Opus 把很小聲的東西壓得更小聲，見上面「擋不住的」）；alpha.137
# 是 1350 毫秒。
HARD_MS = 1200


def edges_ms(path: Path) -> tuple[int, int]:
    """(開頭空白, 結尾空白)，單位毫秒。整支都低於門檻就回傳整支的長度。"""
    result = subprocess.run(
        ["ffmpeg", "-v", "error", "-i", os.fspath(path),
         "-f", "s16le", "-ac", "1", "-ar", str(RATE), "-"],
        capture_output=True)
    if result.returncode != 0:
        raise RuntimeError(f"ffmpeg 讀不動 {path}：{result.stderr[-200:].decode(errors='replace')}")
    samples = array.array("h")
    samples.frombytes(result.stdout[: len(result.stdout) // 2 * 2])
    if sys.byteorder != "little":
        samples.byteswap()
    frames = len(samples) // FRAME
    if frames < 2:
        return 0, 0
    rms = []
    for i in range(frames):
        chunk = samples[i * FRAME : (i + 1) * FRAME]
        rms.append(math.sqrt(sum(float(s) * s for s in chunk) / FRAME + 1e-20))
    threshold = max(rms) * (10.0 ** (REL_DB / 20.0))
    loud = [value > threshold for value in rms]
    run = [loud[i] and loud[i + 1] for i in range(frames - 1)]
    if not any(run):
        return frames * 10, frames * 10
    first = run.index(True)
    last = len(run) - 1 - run[::-1].index(True)
    return first * 10, (frames - (last + 2)) * 10


def main() -> int:
    if shutil.which("ffmpeg") is None:
        print("✗ 找不到 ffmpeg。這一條要真的把 Opus 解開來量，沒有 ffmpeg 就沒有答案——")
        print("  而「沒有答案」不可以印成綠燈。裝了再跑：apt-get install ffmpeg")
        return 1

    problems: list[str] = []
    total = 0
    worst = ("", 0)

    for label, directory in SETS:
        manifest_path = directory / "manifest.json"
        if not manifest_path.is_file():
            problems.append(f"{label}：找不到 {manifest_path.relative_to(ROOT)}")
            continue
        clips = json.loads(manifest_path.read_text(encoding="utf-8")).get("clips", [])
        if not clips:
            problems.append(f"{label}：manifest 裡一支片段都沒有——這條檢查問錯地方了")
            continue
        total += len(clips)

        def job(clip: dict) -> tuple[dict, tuple[int, int]]:
            return clip, edges_ms(directory / clip["file"])

        with concurrent.futures.ThreadPoolExecutor(max_workers=os.cpu_count() or 4) as pool:
            results = list(pool.map(job, clips))

        clean = 0
        for clip, (head, tail) in results:
            name = f"{label} {clip.get('persona')}/{Path(clip['file']).stem}"
            if max(head, tail) > worst[1]:
                worst = (name, max(head, tail))
            if head <= LIMIT_MS and tail <= LIMIT_MS:
                clean += 1
            if head > HARD_MS or tail > HARD_MS:
                problems.append(
                    f"{name}：開頭空 {head} ms、結尾空 {tail} ms，"
                    f"過了 {HARD_MS} ms 的硬上限")

        share = clean / len(clips)
        over = len(clips) - clean
        print(f"  {label}：{len(clips)} 支，{clean} 支兩端的空白都在 {LIMIT_MS} ms 以內"
              f"（{share:.1%}），超標 {over} 支")
        if share < SHARE_MIN:
            listed = sorted(
                (max(h, t), f"{c.get('persona')}/{Path(c['file']).stem}")
                for c, (h, t) in results if h > LIMIT_MS or t > LIMIT_MS)
            worst_few = "、".join(f"{n}（{ms} ms）" for ms, n in listed[-4:][::-1])
            problems.append(
                f"{label}：只有 {share:.1%} 兩端乾淨，低於 {SHARE_MIN:.0%}"
                f"——整包多半沒切過頭尾。最長的幾支：{worst_few}")

    if total:
        print(f"量了 {total} 支。最長的一端是 {worst[0]} 的 {worst[1]} ms。")

    if problems:
        print(f"\n✗ {len(problems)} 個問題：")
        for line in problems[:40]:
            print(f"    {line}")
        if len(problems) > 40:
            print(f"    …還有 {len(problems) - 40} 個")
        return 1

    print("✔ 每一包的頭尾都切過了，沒有一支掛著一大段空白。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
