#!/usr/bin/env python3
"""出貨的每一支語音，解出來真的一樣大聲，而且沒有一支破音。

**這一條是為了一句使用者說過的話存在的**：「我試了每隻角色，很多隻的語音檔大小
聲不一樣。」那不是錯覺——alpha.136 以前出貨的 952 支 Opus 解開來量，跨 17 個角色
的整合響度中位數從 gemini 的 −37.3 排到 deepseek 的 −16.4（差 20.9 LU），單檔之間
差 32.8 LU，而且有 76 支的真實峰值超過 0 dBTP（最高 +2.14）。

放它進來的是錄音那邊品管唯一那條音量判準：`0.006 <= rms <= 0.35`。那是一道 35 dB
寬的門，它問的是「聽得到嗎、破了嗎」，不是「和別支一樣大聲嗎」。

所以這一支問的是後者，而且**問的是真的出貨的那個檔**：

1. **manifest 上寫的響度和真實峰值，兩個都要對得上解出來的聲音。** 響度差超過
   `DRIFT_LU`、峰值差超過 `DRIFT_DBTP` 就紅。這一條守的是「那兩個數字是量出來
   的，不是打上去的」——manifest 是 promote 從品管收據抄過來的，而收據上的數字
   是品管**把同一個 Opus 解開來**量的。所以這裡量到的應該幾乎一樣；不一樣就代
   表有人動過檔案、或者有人用手打了數字。

   **峰值那半是 2026-09-13 才補上的，而在那之前這句話是假的。** 它從第一版就寫
   著「響度」，讀起來像 manifest 上量到的數字都有人對過，實際上程式只比對
   `integratedLufs`；`truePeakDbtp` 從頭到尾沒有任何一道閘門拿它對過聲音。實測
   把一支的 `truePeakDbtp` 改掉（`manifest.json` 與 `manifest.js` 一起改、保持
   一致），九道相關閘門**全綠**——而那一欄的最大值正是出貨的 NOTICE 印給使用者
   看的那個數字（「the loudest true peak is −0.96 dBTP」）。
2. **解出來不准超過 0 dBTP。** 這是使用者聽得到的那件事：破音。整平的天花板壓
   在 −1 dBTP、而且壓的就是 Opus 這一端（Opus 40 kbps VOIP 會把峰值往上推，實
   測最多 +1.24 dB，所以壓 WAV 沒有用）。這裡守的是那條使用者的線。
3. **比目標大聲永遠不收**，那只可能是沒跑過整平。
4. **比目標小聲有一條硬底線，而且只准是少數。** 有些錄音的波峰比它的響度高
   二十幾 dB（「噗」的那種笑聲最明顯），推到 −23 之前峰值就先撞到天花板了；
   那是物理限制，不做壓縮就過不去。這裡守兩件事：沒有任何一支低過
   `MAX_QUIET_LU`，而且每一包至少 `IN_BAND_MIN` 落在帶子裡。
   「這一支真的推不上去」是在錄音那邊的品管證明的——它有 WAV，會把那一支加大
   1 dB 重編一次看峰值破不破。這支只有 Ogg，證不了，所以它守的是分佈。

為什麼不併進 `check-persona-*-voice-assets.py`：那三支守的是「檔案沒被換掉」
（雜湊）與「清單長得對」，跑起來一秒鐘、不需要任何外部工具。這一支要解 952 個
Opus，需要 ffmpeg。分開就分開，不要讓便宜那三條被貴的這條拖著。

驗收（十一刀，全部從檔案備份還原、還原後逐位元組比對過）：把一支出貨的 Ogg 加大
4 dB 重編、手打一個 manifest 響度、整塊拿掉 `postProcessing`、把 method 裡的
「no compression」換掉、拿掉一支的 `integratedLufs`、把宣告的帶子縮到 ±0.05、
把一支調小 1.5 dB 而 manifest 誠實照抄（只紅在底線那一條）、manifest 自己寫一個
破天花板的峰值——八刀全紅。第九刀打在 0 dBTP 那條規則上，**第一次沒打到**：
+8 dB 只推到 −0.07 dBTP，紅的是別條規則；改成 +12 dB（+0.91 dBTP）才真的打中。
`want=綠` 的對照是現況最小聲的那支（banter sakana/poke-annoying，−28.0 LUFS，
比目標低 5.0 LU）：它是峰值先撞天花板才停在那裡的，這一條不可以判它死。

第十、十一刀打在 2026-09-13 補的峰值那半上，靶是 banter chatgpt/poke-aiyo（解出
來是 −7.92 dBTP），`manifest.json` 與 `manifest.js` 一起改、保持一致：改成 −9.99
這一條當場紅，點名「差 +2.07 dB，超過 0.5」；改成 −8.32（只差 0.40）**照舊印綠**
——那一刀 `want=綠`，證明上面「±0.5 以內手打得過去」不是我替它寫的一句好聽話，
而那 0.40 有出現在摘要那行的「真峰值最多差」上，所以它在紅之前就看得見。

第十二、十三刀打在 `DYNAMICS_PROMISE` 那一條上（同日）。第十二刀把 method 老實改
成「4:1 compression before the gain」→ 紅，這是它該做的事。第十三刀把 method 改成
只剩「no compression」、限幅和其他動態處理那半整段刪掉——**當時的針只有「no
compression」六個字，於是它印綠**，而 NOTICE 對使用者講的是三件事。針改成整個
承諾片語之後同一刀就紅了。
"""

from __future__ import annotations

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

# manifest 寫的和這裡量的差多少還算同一支。兩邊量的是**同一份位元組**（promote
# 會比雜湊），所以差別只可能來自 ffmpeg 版本；CI 上是 apt 的 ubuntu 版，這台是
# 另一版。0.5 LU 遠比任何聽得出來的差別小，也遠比「有人手打了一個數字」小。
#
# 註：這個數字一開始是 2.5，因為 manifest 記的是 WAV 的響度而出貨的是 Opus——
# 我拿 68 支取樣量到最多漂 1.85 LU。後來把 952 支全量一遍，最大漂到 3.49 LU，
# 這道閘門本來會當場紅。真正的修法不是把門開大，是讓兩邊量同一個檔。
DRIFT_LU = 0.5

# manifest 寫的真實峰值和這裡量的差多少還算同一支。理由和 DRIFT_LU 同一條（兩邊
# 量的是同一份位元組，只剩 ffmpeg 版本差），所以取同一個數量級。
#
# 先量儀器自己的雜訊再挑門檻：這台機器上把 952 支全解一遍對回 manifest，響度和
# 峰值的 |差| 最大都是 0.000、中位數 0.000，超過 0.1 的 0 支。所以 0.5 整個是留給
# CI 那一版 ffmpeg 的餘裕，不是留給資料的——真的漂到 0.1 就該去查，不該等它紅。
# 底下的摘要每次都把「現在最多差多少」印出來，就是為了讓那件事在紅之前看得到。
#
# 它擋不住什麼：一個手打的峰值只要落在真值 ±0.5 dB 以內就過得去，而 NOTICE 印的
# 是這一欄的最大值，所以那句話帶著同樣的 ±0.5。真正「不准破音」那條線量的是解出
# 來的聲音（規則 2），不經過 manifest。
DRIFT_DBTP = 0.5

# manifest 的 postProcessing.loudness.method 要整句寫著這個。NOTICE 對使用者講的
# 就是這三件事（「No compression, limiting, or other dynamics processing was
# applied」），所以針要取**整個承諾片語**——第一版只 grep「no compression」，實測
# 把 method 改成只剩那兩個字、限幅和其他動態處理那半整段刪掉，這一條照樣印綠。
DYNAMICS_PROMISE = "no compression, limiting, or other dynamics processing"

# 使用者聽得到的那條線。超過這裡，播放端就會削掉。
SHIPPED_CEILING_DBTP = 0.0

# 沒有任何一支可以比目標小聲超過這麼多。整平後實測最小聲的是 banter 的
# sakana/poke-annoying，低 5.01 LU（波峰高出響度 26.1 dB），所以 5.5 是「現況
# 再糟一點點」而不是一個寬到沒有意義的門。
MAX_QUIET_LU = 5.5

# 每一包至少這個比例要落在宣告的帶子裡。整平後實測：同意書 92.6%、固定台詞
# 97.6%、互動短句 97.1%；整平前同一條算出來是 7.4%／6.4%／5.0%。
IN_BAND_MIN = 0.85


def measure(path: Path) -> tuple[float, float]:
    result = subprocess.run(
        ["ffmpeg", "-hide_banner", "-nostats", "-i", os.fspath(path),
         "-af", "loudnorm=print_format=json", "-f", "null", "-"],
        capture_output=True, text=True)
    if result.returncode != 0:
        raise RuntimeError(f"ffmpeg 讀不動 {path}：{result.stderr[-200:]}")
    start = result.stderr.rfind("{")
    if start < 0:
        raise RuntimeError(f"loudnorm 沒吐 JSON：{path}")
    data = json.loads(result.stderr[start:result.stderr.index("}", start) + 1])
    return float(data["input_i"]), float(data["input_tp"])




def main() -> int:
    if shutil.which("ffmpeg") is None:
        print("✗ 找不到 ffmpeg。這一條要真的把 Opus 解開來量，沒有 ffmpeg 就沒有答案——")
        print("  而「沒有答案」不可以印成綠燈。裝了再跑：apt-get install ffmpeg")
        return 1

    problems: list[str] = []
    measured_all: list[float] = []
    peaks_all: list[float] = []
    worst_drift_lu = 0.0
    worst_drift_tp = 0.0
    total = 0

    for label, directory in SETS:
        manifest_path = directory / "manifest.json"
        if not manifest_path.is_file():
            problems.append(f"{label}：找不到 {manifest_path.relative_to(ROOT)}")
            continue
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        band = manifest.get("postProcessing", {}).get("loudness")
        if not isinstance(band, dict) or not all(
            isinstance(band.get(field), (int, float))
            for field in ("targetLufs", "ceilingDbtp", "toleranceLu")
        ):
            problems.append(f"{label}：manifest 沒有 postProcessing.loudness，說不出它對齊到哪裡")
            continue
        # 這一行是**同步檢查，不是證據**。它讀的是流水線自己寫在 manifest 上的一
        # 句話，而出貨的 NOTICE 也寫著同一件事（「No compression, limiting, or
        # other dynamics processing was applied」）——兩邊不准各說各話，改了流水線
        # 就會被逼著兩邊一起改。它證不了那句話是真的。
        #
        # 為什麼不從出貨的 Ogg 量：試過了，量不回來。詳細數字寫在
        # check-notice-claims-are-accounted-for.py 那一條的分類註解裡，結論是限幅
        # 在這批素材上根本沒東西可夾（增益本來就選成峰值不過 −1 dBTP），而壓縮壓
        # 出來的 crest 和現況幾乎整段重疊。
        if DYNAMICS_PROMISE not in str(band.get("method", "")):
            problems.append(
                f"{label}：postProcessing.loudness.method 沒有整句寫著"
                f"「{DYNAMICS_PROMISE}」——NOTICE 對使用者講的是這三件事，"
                f"manifest 不可以只認其中一件")

        clips = manifest.get("clips", [])
        total += len(clips)
        target = band["targetLufs"]
        tolerance = band["toleranceLu"]
        in_band = 0

        def job(clip: dict) -> tuple[dict, float, float]:
            return (clip, *measure(directory / clip["file"]))

        with concurrent.futures.ThreadPoolExecutor(max_workers=os.cpu_count() or 4) as pool:
            results = list(pool.map(job, clips))

        for clip, integrated, true_peak in results:
            name = f"{label} {clip.get('persona')}/{Path(clip['file']).stem}"
            claimed = clip.get("integratedLufs")
            claimed_peak = clip.get("truePeakDbtp")
            if not isinstance(claimed, (int, float)) or not isinstance(claimed_peak, (int, float)):
                problems.append(f"{name}：manifest 上沒有量到的響度")
                continue
            measured_all.append(integrated)
            peaks_all.append(true_peak)
            worst_drift_lu = max(worst_drift_lu, abs(integrated - claimed))
            worst_drift_tp = max(worst_drift_tp, abs(true_peak - claimed_peak))
            if abs(integrated - claimed) > DRIFT_LU:
                problems.append(
                    f"{name}：manifest 說 {claimed:.1f} LUFS，解出來是 {integrated:.1f}"
                    f"（差 {integrated - claimed:+.1f} LU，超過 {DRIFT_LU}）")
            if abs(true_peak - claimed_peak) > DRIFT_DBTP:
                problems.append(
                    f"{name}：manifest 說真實峰值 {claimed_peak:+.2f} dBTP，解出來是 "
                    f"{true_peak:+.2f}（差 {true_peak - claimed_peak:+.2f} dB，"
                    f"超過 {DRIFT_DBTP}）")
            if true_peak > SHIPPED_CEILING_DBTP:
                problems.append(
                    f"{name}：解出來的真實峰值 {true_peak:+.2f} dBTP，超過 "
                    f"{SHIPPED_CEILING_DBTP:+.1f}——播出來會削掉")
            if integrated > target + tolerance:
                problems.append(
                    f"{name}：{integrated:.1f} LUFS，比目標大聲超過 {tolerance} LU"
                    f"——沒跑過整平的才會這樣")
            elif integrated < target - MAX_QUIET_LU:
                problems.append(
                    f"{name}：{integrated:.1f} LUFS，比目標小聲 "
                    f"{target - integrated:.1f} LU，過了 {MAX_QUIET_LU} LU 的底線")
            elif integrated >= target - tolerance:
                in_band += 1
            if claimed_peak > band["ceilingDbtp"] + 0.1:
                problems.append(
                    f"{name}：manifest 自己寫的峰值 {claimed_peak:+.2f} dBTP 超過它自己"
                    f"宣告的天花板 {band['ceilingDbtp']:+.1f}")

        if clips:
            share = in_band / len(clips)
            # 還能再掉幾支才會低於 IN_BAND_MIN。0 就是「下一支掉出帶子這條就紅」。
            slack = in_band - math.ceil(IN_BAND_MIN * len(clips) - 1e-9)
            print(f"  {label}：{len(clips)} 支，{in_band} 支落在 "
                  f"{target}±{tolerance} LUFS（{share:.1%}），還能再掉 {max(slack, 0)} 支")
            if share < IN_BAND_MIN:
                problems.append(
                    f"{label}：只有 {share:.1%} 落在帶子裡，低於 {IN_BAND_MIN:.0%}"
                    f"——整包沒有整平過")

    if measured_all:
        measured_all.sort()
        spread = measured_all[-1] - measured_all[0]
        print(f"量了 {total} 支。解出來的整合響度 {measured_all[0]:.1f} … {measured_all[-1]:.1f} LUFS"
              f"（散度 {spread:.1f} LU）。")
        # 兩條硬線的餘裕。它們和上面每包那條「還能再掉幾支」互不涵蓋：帶子外面
        # 還有 MAX_QUIET_LU 可以掉，所以一支可以出了帶子還離底線好幾 LU；反過來
        # 只要一支撞到底線就紅，不管那一包落在帶子裡的比例多漂亮。兩個都印，哪
        # 一個先撐不住讓輸出自己講。
        floor = -23.0 - MAX_QUIET_LU
        room = measured_all[0] - floor
        headroom = SHIPPED_CEILING_DBTP - max(peaks_all)
        print(f"  最靜的一支 {measured_all[0]:.1f} LUFS，"
              + (f"離 {floor:.1f} 的底線還有 {room:.1f} LU"
                 if room >= 0 else f"已經低過 {floor:.1f} 的底線 {-room:.1f} LU")
              + f"；最高的真實峰值 {max(peaks_all):+.2f} dBTP，"
              + (f"離 {SHIPPED_CEILING_DBTP:+.1f} 的天花板還有 {headroom:.2f} dB。"
                 if headroom >= 0
                 else f"已經超過 {SHIPPED_CEILING_DBTP:+.1f} 的天花板 {-headroom:.2f} dB。"))
        # 規則 1 的餘裕。上面那兩行講的是聲音本身離硬線多遠，這一行講的是
        # **manifest 有沒有在漂**——那是完全不同的一件事，而且是先漂才會出錯。
        print(f"  manifest 對得上解出來的：響度最多差 {worst_drift_lu:.2f} LU"
              f"（門檻 {DRIFT_LU}）、真峰值最多差 {worst_drift_tp:.2f} dB"
              f"（門檻 {DRIFT_DBTP}）。")

    if problems:
        print(f"\n✗ {len(problems)} 個問題：")
        for line in problems[:40]:
            print(f"    {line}")
        if len(problems) > 40:
            print(f"    …還有 {len(problems) - 40} 個")
        return 1

    print("✔ 每一支的響度和真實峰值都對得上 manifest，沒有一支解出來會破音。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
