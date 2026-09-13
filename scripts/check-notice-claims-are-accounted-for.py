#!/usr/bin/env python3
r"""出貨的 NOTICE 裡**每一句話**都要有人負責，不准有沒分類的句子。

出貨的六份 NOTICE 是這個產品對外唯一的權利與隱私聲明。讀它的人沒有辦法自己
驗證裡面的句子——他只能相信。而我已經在同一段文字裡抓到兩句照字面讀是假的
全稱句：

  1. 「no character of the written line went missing」——實測 128 支被剪過的
     片段裡有 39 支（30%）不成立（2026-09-12 修掉）。
  2. 「levelled … to EBU R128 -23 LUFS under a -1 dBTP true-peak ceiling」——
     出貨的 952 支實測 −28.0 … −22.8 LUFS，4 支的真峰值在 −1 之上（同日修掉）。

兩次都是**我讀到那一句才發現的**。這支存在是為了讓「有沒有下一句」不再取決於
我今天有沒有想到去讀。

## 它問什麼

把六份 NOTICE 切成句子，每一句都必須落進下面 `CLAIMS` 的某一格，而且每一格都
必須真的對到句子。四種分類，**沒有第五種「還沒想」**：

  MEASURED  這裡當場對出貨的檔案量一次。量不過就紅。
  GATE      別的閘門在守，要指名；那支腳本必須存在，而且必須真的排在 ci.yml 裡。
  LAB       從出貨的 bytes 證不出來（證據在錄音那邊的 QC 收據）。**必須寫理由。**
  PROSE     不是關於這些檔案的事實宣稱（法律用語、免責、虛構角色聲明）。必須寫理由。

於是：

  - NOTICE 多一句話 → 沒有任何一格認領它 → **紅**。
  - NOTICE 刪一句話 → 有一格對不到句子 → **紅**（死掉的分類會讓人以為還有人守）。
  - 句子還在但數字漂了 → MEASURED 那幾格當場紅。

## 為什麼分類要逼人改，不是逼人看

一份寫在註解裡的「這些句子我都想過了」只會逼下一個人看一眼。這裡的分類是
**程式碼**：新句子沒地方放就編不過閘門，而放進 LAB／PROSE 要寫下理由——寫不
出理由，通常就是那句話不該印在出貨的檔案裡。

## 它擋不住什麼（都做過一刀確認）

  - **句子和分類一起被改成假話。** 分類是我寫的，不是量出來的。
  - **LAB 那幾格的內容。** 它只保證「我承認這一格證不出來」，不保證那句話是
    真的。想升級就要把 QC 收據的雜湊帶進 manifest，那是另一件事。
  - **PROSE 分類錯用。** 把一句事實宣稱標成 PROSE，這條照樣綠。實際做過一刀
    （`want=綠`）：把「Measured on these Ogg files…」那一格從 MEASURED 改成
    PROSE，整條綠。防線是 review，不是這支腳本。
  - **NOTICE 以外的合規文字**（README／PRIVACY／SPEC）。那幾份不在這裡。
"""
from __future__ import annotations

import hashlib
import json
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
CI = ROOT / ".github/workflows/ci.yml"

VOICE_PACKS = (
    ("apps/desktop/ui/persona-voices/v1", "固定台詞"),
    ("apps/desktop/ui/persona-consent-voices/v1", "同意書朗讀"),
    ("apps/desktop/ui/persona-banter-voices/v1", "互動短句"),
)
problems: list[str] = []


def fail(where: str, message: str) -> None:
    problems.append(f"{where}：{message}")


# ---------------------------------------------------------------- 切句子


def sentences(path: pathlib.Path) -> list[str]:
    """把 NOTICE 切成句子。標題行不算。

    中文用「。」，英文用「. 」（句點後面接空白或結尾）。這裡刻意不處理縮寫，
    因為出貨的六份文字裡一個都沒有——真的多出一個，它會切出一句對不到任何
    分類的碎片，然後這條紅，那正是我要的訊號。
    """
    body = "\n".join(
        line for line in path.read_text(encoding="utf-8").splitlines()
        if not line.startswith("#")
    )
    body = re.sub(r"\s+", " ", body).strip()
    out, buf = [], ""
    for i, ch in enumerate(body):
        buf += ch
        if ch == "。" or (ch == "." and (i + 1 >= len(body) or body[i + 1] == " ")):
            out.append(buf.strip())
            buf = ""
    if buf.strip():
        out.append(buf.strip())
    return [s for s in out if s]


# ---------------------------------------------------- MEASURED 的那幾支


def manifest_of(base: str) -> dict:
    return json.loads((ROOT / base / "manifest.json").read_text(encoding="utf-8"))


def numbers_in(text: str) -> list[str]:
    return re.findall(r"-?\d+(?:\.\d+)?", text)


def voice_count(sentence: str, base: str, label: str) -> None:
    manifest = manifest_of(base)
    said = int(numbers_in(sentence)[0])
    on_disk = len(list((ROOT / base).rglob("*.ogg")))
    listed = len(manifest["clips"])
    if not said == on_disk == listed:
        fail(label, f"NOTICE 說 {said} 支，manifest 列 {listed} 支，資料夾裡 {on_disk} 支")


def voice_targets(sentence: str, base: str, label: str) -> None:
    loudness = manifest_of(base)["postProcessing"]["loudness"]
    said = numbers_in(sentence)
    for value, what in ((loudness["targetLufs"], "targetLufs"),
                        (loudness["ceilingDbtp"], "ceilingDbtp")):
        if f"{value:g}" not in said:
            fail(label, f"NOTICE 這一句沒有 manifest 的 {what}={value:g}（它寫的是 {said}）")


def voice_measured(sentence: str, base: str, label: str) -> None:
    clips = manifest_of(base)["clips"]
    want = [f"{min(r['integratedLufs'] for r in clips):.1f}",
            f"{max(r['integratedLufs'] for r in clips):.1f}",
            f"{max(r['truePeakDbtp'] for r in clips):.2f}"]
    said = numbers_in(sentence)
    if said != want:
        fail(label, f"NOTICE 說 {said}，出貨的 manifest 算出來是 {want}")


def grant_date(sentence: str, base: str, label: str) -> None:
    """NOTICE 寫的授權日，要等於 manifest 自己記的那個。

    六份 manifest 有兩種 key 拼法（`ownerGrant.grantedOn` 與
    `owner_grant.granted_on`），兩種都吃；一種都找不到就紅——**找不到欄位不
    可以當成通過**。
    """
    manifest = manifest_of(base)
    for outer in ("ownerGrant", "owner_grant"):
        node = manifest.get(outer)
        if isinstance(node, dict):
            for inner in ("grantedOn", "granted_on"):
                if inner in node:
                    real = node[inner]
                    break
            else:
                continue
            break
    else:
        fail(label, "manifest 沒有 ownerGrant／owner_grant，NOTICE 的授權日沒有東西可以對")
        return
    said = re.search(r"\d{4}-\d{2}-\d{2}", sentence)
    if not said or said.group() != real:
        fail(label, f"NOTICE 的授權日 {said.group() if said else '（沒有）'} ≠ manifest 的 {real}")


def quoted_manifest_hash(sentence: str, base: str, label: str) -> None:
    said = re.findall(r"\b[0-9a-f]{64}\b", sentence)
    real = hashlib.sha256((ROOT / base / "manifest.json").read_bytes()).hexdigest()
    if said != [real]:
        fail(label, f"NOTICE 引的 manifest SHA-256 是 {said}，真的是 {real}")


def icons_recipe(sentence: str, base: str, label: str) -> None:
    """五個檔案、來源與 recipe 指的是同一份東西，而且那幾份東西沒被換掉。

    manifest 存的是相對 icons/ 的路徑（`../../ui/personas/chatgpt.webp`），
    NOTICE 引的是 repo 根目錄的路徑——**兩個字串本來就不相等**，要 resolve
    過再比。第一版我拿字串直接比，於是把一句真話判成假的。
    """
    manifest = manifest_of(base)
    if len(manifest["assets"]) != 5:
        fail(label, f"NOTICE 說五個，manifest 列 {len(manifest['assets'])} 個")
    nodes = [manifest["source"], manifest["recipe"]["html"], manifest["recipe"]["script"]]
    declared = {}
    for node in nodes:
        target = (ROOT / base / node["file"]).resolve()
        try:
            declared[target.relative_to(ROOT).as_posix()] = node["sha256"]
        except ValueError:
            fail(label, f"manifest 的 `{node['file']}` 指到 repo 外面")
    for quoted in re.findall(r"`([^`]+)`", sentence):
        if quoted not in declared:
            fail(label, f"NOTICE 說來源／recipe 是 `{quoted}`，manifest 指的是 {sorted(declared)}")
    # NOTICE 說這五個圖是「按這份 recipe、以這份人物來源」產生的。那句話要站得住，
    # 那三份東西就不能在出貨之後被換掉——manifest 逐份記了 sha256，這裡對回去。
    for path, want in declared.items():
        real = hashlib.sha256((ROOT / path).read_bytes()).hexdigest()
        if real != want:
            fail(label, f"NOTICE 的產地 `{path}` 已經被改過（manifest 記 {want[:12]}…，"
                        f"現在是 {real[:12]}…）")


def reels_count(sentence: str, base: str, label: str) -> None:
    """這一句同時宣稱兩件事（幾套、哪一天授權的），兩件都要對。"""
    rigs = manifest_of(base)["rigs"]
    said = int(numbers_in(sentence)[0])
    if said != len(rigs):
        fail(label, f"NOTICE 說 {said} 套，manifest 列 {len(rigs)} 套")
    grant_date(sentence, base, label)


def reels_fields(sentence: str, base: str, label: str) -> None:
    for rig in manifest_of(base)["rigs"]:
        for layer in rig["layers"]:
            missing = [f for f in ("bytes", "sha256", "center_x", "center_y", "width", "height")
                       if f not in layer]
            if missing:
                fail(label, f"{rig['id']} 有圖層少了 {missing}——NOTICE 說逐檔固定這些")
                return


def reels_nothing_private(sentence: str, base: str, label: str) -> None:
    tracked = subprocess.run(["git", "ls-files"], cwd=ROOT, check=True,
                             capture_output=True, text=True).stdout.split()
    stray = [f for f in tracked if pathlib.PurePosixPath(f).name in ("parts.json", "source.json")]
    if stray:
        fail(label, f"NOTICE 說 raw parts.json／source.json 不在裡面，而 repo 裡有 {stray}")
    for shipped in tracked:
        if not shipped.endswith(("manifest.json", "manifest.js", "catalog.js")):
            continue
        text = (ROOT / shipped).read_text(encoding="utf-8", errors="replace")
        for hit in re.findall(r"(?:/home/[\w.-]+|/Users/[\w.-]+|[A-Za-z]:\\\\[\w.-]+)", text):
            fail(label, f"NOTICE 說私有絕對 path 不在裡面，而 {shipped} 有 `{hit}`")
            return


def personas_size(sentence: str, base: str, label: str) -> None:
    manifest = manifest_of(base)
    said = numbers_in(sentence)
    if len(manifest["assets"]) != int(said[0]):
        fail(label, f"NOTICE 說 {said[0]} 張，manifest 列 {len(manifest['assets'])} 張")
    want = int(said[-1])
    for asset in manifest["assets"]:
        size = webp_size((ROOT / base / asset["file"]).read_bytes())
        if size != (want, want):
            fail(label, f"NOTICE 說縮成 {want}×{want}，{asset['file']} 是 {size}")
            return


def webp_size(blob: bytes) -> tuple[int, int] | None:
    """從 WebP 的檔頭直接讀寬高。三種 chunk 都要認，認不出來就回 None（會紅）。"""
    kind = blob[12:16]
    if kind == b"VP8X":
        return (int.from_bytes(blob[24:27], "little") + 1,
                int.from_bytes(blob[27:30], "little") + 1)
    if kind == b"VP8L":
        packed = int.from_bytes(blob[21:25], "little")
        return ((packed & 0x3FFF) + 1, ((packed >> 14) & 0x3FFF) + 1)
    if kind == b"VP8 ":
        return (int.from_bytes(blob[26:28], "little") & 0x3FFF,
                int.from_bytes(blob[28:30], "little") & 0x3FFF)
    return None


def manifest_js_mirrors(sentence: str, base: str, label: str) -> None:
    js = (ROOT / base / "manifest.js").read_text(encoding="utf-8")
    payload = js.split("= ", 1)[1].rsplit(";", 1)[0].strip()
    if json.loads(payload) != manifest_of(base):
        fail(label, "manifest.js 和 manifest.json 不是同一份清單")


# ------------------------------------------------------------- 分類表

MEASURED, GATE, LAB, PROSE = "MEASURED", "GATE", "LAB", "PROSE"

VOICE_CLAIMS = (
    ("Ogg Opus files in this directory were generated locally", MEASURED, voice_count),
    ("The model and inference code are available under Apache-2.0", LAB,
     "上游模型自己的授權，不是這批檔案的性質"),
    ("the clips were levelled in the voice lab", MEASURED, voice_targets),
    ("Those two figures are what the gain aimed at", PROSE,
     "免責句：明說前一句是目標不是量測，本身不宣稱任何數字"),
    ("Measured on these Ogg files the integrated loudness runs from", MEASURED, voice_measured),
    ("Some clips were also trimmed at one end", LAB,
     "剪不剪、剪在哪，判準與 35 dB 門檻都在錄音那邊的 QC 收據裡"),
    ("Each trim leaves about 120 ms of natural lead-in or run-out", GATE,
     "check-persona-voice-edges.py（它量出貨 Opus 的頭尾空白量級；"
     "「剛好 120 ms」本身要 QC 收據才證得出來）"),
    ("That is a before-and-after comparison", PROSE,
     "免責句：明說前一句不是「逐字稿等於台詞」"),
    ("Clips that trimming could not fix were re-recorded instead", LAB,
     "哪幾支重錄過、當時兩個引擎各聽到什麼，都在 QC 收據裡"),
    ("Each was re-synthesised until the two models no longer agreed", LAB,
     "重錄的收斂條件與「不比被換掉那支差」的比較，都在 QC 收據裡"),
    # 2026-09-13 從 GATE 降級成 LAB，而降級的理由是量出來的，不是想出來的。
    #
    # 舊分類指向 check-persona-voice-loudness.py，而那一條做的事是
    # `"no compression" not in band["method"]`——讀 manifest 上流水線**自己寫的
    # 一句話**。那是同步檢查（manifest 和 NOTICE 不准各說各話），不是證據。
    #
    # 那能不能改成從出貨的 Ogg 量？拿最像的儀器 crest（真峰值 − 整合響度）試過，
    # 取樣 10 支橫跨整個分佈，各自「解開 → 處理 → 重新整平到 −23／−1 → 重編
    # Opus」，對照組是同一條路但不處理：
    #
    #   限幅 alimiter(−1 dBTP)：crest 變化 −0.52 … +0.81 dB，正負都有，和對照組
    #     的重編雜訊（≤0.17 dB）同一個量級。**不是靈敏度不夠，是它真的沒做事**：
    #     每一支的增益本來就選成峰值不過 −1 dBTP，限幅器夾不到任何東西。
    #   壓縮 acompressor(4:1)：10 支全往下掉（−0.63 … −6.19 dB），但落點
    #     9.29 … 25.44 和出貨的 11.10 … 26.60 幾乎整段重疊，只有 1 支掉到出貨的
    #     最小值以下。任何一條逐支的地板都會漏掉另外 9 支。
    #
    # 所以這句話在出貨的位元組上量不回來，證據在錄音那邊的整平腳本與收據裡。
    # 老實歸到 LAB，不要把一條抓不到東西的閘門掛在它名下。
    ("No compression, limiting, or other dynamics processing was applied", LAB,
     "整平腳本每支只乘一個常數增益，證據在錄音那邊；"
     "出貨的 Ogg 上量不回來（crest 對限幅完全無感、對壓縮和現況重疊）"),
    ("This directory holds only the Ogg files", GATE,
     "check-shipped-asset-trees-hold-nothing-else.py（第一件事）"),
    ("stay in the voice lab and are not published", GATE,
     "check-shipped-asset-trees-hold-nothing-else.py（第二件事，掃 git ls-files）"),
    ("excluded from this repository's Apache-2.0 license", PROSE, "授權聲明"),
    ("granted AI-Sister permission on", MEASURED, grant_date),
    ("This grant does not extend to", PROSE, "授權範圍的排除條款"),
)

CLAIMS = {
    "apps/desktop/src-tauri/icons": (
        ("這個目錄的五個 PNG／ICO", MEASURED, icons_recipe),
        ("素材所有人 Ted Huang 於", MEASURED, grant_date),
        ("每份 artifact 只取該平台需要的格式", PROSE, "說明安裝包不會原樣附五個檔"),
        ("這些圖像不包含在本專案的 Apache-2.0", PROSE, "授權聲明"),
        ("完整 bytes 的 SHA-256 是", MEASURED, quoted_manifest_hash),
        ("受同名 AI 產品啟發的虛構角色", PROSE, "商標與角色聲明"),
        ("AI-Sister 是獨立產品", PROSE, "商標與角色聲明"),
    ),
    "apps/desktop/ui/persona-reels": (
        ("這個目錄的 17 套分層 PNG rig", MEASURED, reels_count),
        ("授權範圍是把", PROSE, "授權範圍聲明"),
        ("這些圖像不包含在本專案的 Apache-2.0 程式碼授權裡", PROSE, "授權聲明"),
        ("逐檔固定大小、SHA-256、圖層位置與尺寸", MEASURED, reels_fields),
        ("私有絕對 path", MEASURED, reels_nothing_private),
        ("完整 bytes 的 SHA-256 是", MEASURED, quoted_manifest_hash),
        ("受同名 AI 產品啟發的虛構角色", PROSE, "商標與角色聲明"),
        ("AI-Sister 是獨立產品", PROSE, "商標與角色聲明"),
    ),
    "apps/desktop/ui/personas": (
        ("這個目錄的 17 張 WebP", MEASURED, personas_size),
        ("素材所有人 Ted Huang 於", MEASURED, grant_date),
        ("這些圖像不包含在", PROSE, "授權聲明"),
        ("完整 bytes 的 SHA-256 是", MEASURED, quoted_manifest_hash),
        ("受同名 AI 產品啟發的虛構角色", PROSE, "商標與角色聲明"),
        ("AI-Sister 是獨立產品", PROSE, "商標與角色聲明"),
    ),
}
for base, _label in VOICE_PACKS:
    CLAIMS[base] = VOICE_CLAIMS


def notice_path(base: str) -> pathlib.Path:
    return ROOT / base / "NOTICE.md"


def show_census() -> int:
    """`--list`：把整份清單印出來給人讀。

    綠燈只說「都有人負責」，說不出「負責的是誰」。真的被問到「你們對這些檔案
    宣稱了什麼、憑什麼」的時候，要拿得出來的是這一份，不是我重讀一次六個檔案。
    """
    for base in sorted(CLAIMS):
        path = notice_path(base)
        if not path.is_file():
            continue
        print(f"\n=== {path.relative_to(ROOT)} ===")
        lines = sentences(path)
        for line in lines:
            for needle, kind, detail in CLAIMS[base]:
                if needle in line:
                    how = detail.__name__ if callable(detail) else detail
                    print(f"  [{kind:8}] {line[:78]}{'…' if len(line) > 78 else ''}")
                    print(f"             ↳ {how}")
                    break
            else:
                print(f"  [沒人認領] {line[:78]}")
    return 0


def main() -> int:
    if "--list" in sys.argv[1:]:
        return show_census()
    tracked = {
        str(pathlib.PurePosixPath(p))
        for p in subprocess.run(["git", "ls-files", "*NOTICE.md"], cwd=ROOT, check=True,
                                capture_output=True, text=True).stdout.split()
    }
    if not tracked:
        print("✘ `git ls-files` 找不到任何 NOTICE，這條檢查等於沒跑", file=sys.stderr)
        return 1
    registered = {f"{base}/NOTICE.md" for base in CLAIMS}
    for orphan in sorted(tracked - registered):
        fail(orphan, "這份 NOTICE 沒有分類表——每一句話都沒有人負責")
    for ghost in sorted(registered - tracked):
        fail(ghost, "分類表指到一份 git 沒有追蹤的 NOTICE")

    tally = {MEASURED: 0, GATE: 0, LAB: 0, PROSE: 0}
    for base in sorted(CLAIMS):
        path = notice_path(base)
        if not path.is_file():
            continue
        label = base.rsplit("/", 2)[-2] if base.endswith("/v1") else base.rsplit("/", 1)[-1]
        lines = sentences(path)
        claims = CLAIMS[base]
        hit = [0] * len(claims)
        for line in lines:
            matched = [i for i, (needle, *_) in enumerate(claims) if needle in line]
            if not matched:
                fail(label, f"這一句沒有任何分類認領它：{line[:90]}…")
                continue
            for i in matched:
                hit[i] += 1
        for i, (needle, kind, detail) in enumerate(claims):
            if not hit[i]:
                fail(label, f"分類表有一格對不到任何句子（死掉的分類）：`{needle}`")
                continue
            tally[kind] += 1
            if kind == MEASURED:
                for line in lines:
                    if needle in line:
                        detail(line, base, label)
            elif kind == GATE:
                script = re.match(r"([\w.-]+\.(?:py|mjs))", detail)
                if not script:
                    fail(label, f"GATE 這一格沒有指名腳本：{detail[:40]}")
                elif not (ROOT / "scripts" / script.group(1)).is_file():
                    fail(label, f"GATE 指到不存在的 scripts/{script.group(1)}")
                elif script.group(1) not in CI.read_text(encoding="utf-8"):
                    fail(label, f"GATE 指到的 {script.group(1)} 沒有排在 ci.yml 裡")
            elif not str(detail).strip():
                fail(label, f"{kind} 這一格沒有寫理由：`{needle}`")
        print(f"  {label}：{len(lines)} 句，"
              + "／".join(f"{k} {sum(1 for n, kk, _ in claims if kk == k)}"
                          for k in (MEASURED, GATE, LAB, PROSE)))

    if problems:
        print(f"\n✘ 出貨的 NOTICE 有 {len(problems)} 個問題：", file=sys.stderr)
        for line in problems:
            print(f"    {line}", file=sys.stderr)
        print("\n  每一句話都要落進 MEASURED／GATE／LAB／PROSE 其中一格，"
              "\n  而 LAB 和 PROSE 要寫下為什麼這裡量不到。", file=sys.stderr)
        return 1
    print(f"✔ 六份 NOTICE 的每一句都有人負責"
          f"（當場量 {tally[MEASURED]}、別的閘門 {tally[GATE]}、"
          f"錄音那邊 {tally[LAB]}、非事實宣稱 {tally[PROSE]}）。")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
