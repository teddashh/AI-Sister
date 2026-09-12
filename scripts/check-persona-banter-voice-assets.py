#!/usr/bin/env python3
"""Verify the bundled banter library (poke / giggle / answer-beat) without decoding audio."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
CATALOG_PATH = ROOT / "apps/desktop/ui/persona-voices/banter-catalog-v1.json"
VOICE_ROOT = ROOT / "apps/desktop/ui/persona-banter-voices/v1"
MANIFEST_PATH = VOICE_ROOT / "manifest.json"
SCRIPT_PATH = VOICE_ROOT / "manifest.js"
PERSONA_IDS = (
    "chatgpt", "claude", "gemini", "grok", "deepseek", "qwen", "mistral",
    "venice", "sakana", "perplexity", "glm", "kimi", "hunyuan", "minimax",
    "nemotron", "cohere", "mimo",
)
USES = ("avatar-poke", "idle-giggle", "answer-beat")
# 和 promote 腳本、app.js 的 `banterVoiceLibrary`、voice-lab 的 QC 是同一組數字。
# 整包實測最短的是 perplexity 的「不要戳了。」424 毫秒，所以下限不能沿用日常那包
# 的 700。
MIN_MS = 250
MAX_MS = 4000


def fail(message: str) -> None:
    raise SystemExit(f"FAIL: {message}")


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main() -> None:
    catalog = json.loads(CATALOG_PATH.read_text(encoding="utf-8"))
    manifest = json.loads(MANIFEST_PATH.read_text(encoding="utf-8"))
    if manifest.get("schema") != "ai-sister/persona-banter-voices/v1":
        fail("manifest schema changed")
    if manifest.get("locale") != "zh-TW" or manifest.get("roster") != catalog.get("roster"):
        fail("locale or roster changed")
    if manifest.get("pack") != "banter" or catalog.get("pack") != "banter":
        fail("manifest pack does not match the banter catalog")
    if manifest.get("rightsReview") != "approved-owner-grant":
        fail("rights review is not pinned")
    if manifest.get("ownerGrant", {}).get("grantedOn") != "2026-09-12":
        fail("owner grant is not pinned")
    if manifest.get("engine") != {
        "name": "MediaTek-Research/BreezyVoice-300M",
        "modelSnapshot": "e33b502e0ac21c16b0ee0d00df66ac3fa737393d",
        "license": "Apache-2.0",
    }:
        fail("generation engine provenance changed")

    people = catalog.get("personas", [])
    lines = catalog.get("lines", [])
    if tuple(person.get("id") for person in people) != PERSONA_IDS:
        fail("catalog roster changed")
    if {line.get("use") for line in lines} != set(USES):
        fail("catalog does not cover every banter use")
    per_persona = catalog.get("totals", {}).get("linesPerPersona")
    if not isinstance(per_persona, int) or per_persona != len(lines):
        fail("catalog totals do not match the line list")
    total = len(PERSONA_IDS) * per_persona

    wanted = {}
    for person in people:
        for line in lines:
            # 閒話**不套** {lead}／{approach}。一句感嘆詞在誰嘴裡都是同一個字；
            # 讓它變成她的是那個聲音。這裡明講是為了讓「怎麼沒有 .format()」
            # 讀起來是決定，不是漏掉。
            if "{" in line["text"]:
                fail(f"banter line {line['id']} still carries a template placeholder")
            wanted[(person["id"], line["id"])] = {
                "persona": person["id"],
                "group": person["group"],
                "lineId": line["id"],
                "pack": catalog["pack"],
                "use": line["use"],
                "text": line["text"],
            }
    if len(wanted) != total:
        fail(f"catalog expands to {len(wanted)} clips, expected {total}")

    seen = set()
    total_bytes = 0
    total_duration = 0
    for clip in manifest.get("clips", []):
        key = (clip.get("persona"), clip.get("lineId"))
        if key in seen or key not in wanted:
            fail(f"duplicate or unknown clip {key}")
        seen.add(key)
        for field, value in wanted[key].items():
            if clip.get(field) != value:
                fail(f"{key} {field} does not match catalog")
        expected_file = f"{clip['pack']}/{clip['persona']}/{clip['lineId']}.ogg"
        if clip.get("file") != expected_file:
            fail(f"{key} has non-canonical path")
        path = VOICE_ROOT / expected_file
        try:
            data = path.read_bytes()
        except OSError as error:
            fail(f"cannot read {expected_file}: {error}")
        opus_head = data[:256].find(b"OpusHead")
        input_sample_rate = (
            int.from_bytes(data[opus_head + 12:opus_head + 16], "little")
            if opus_head >= 0 and len(data) >= opus_head + 19
            else None
        )
        if (
            len(data) != clip.get("bytes")
            or sha256(path) != clip.get("sha256")
            or not data.startswith(b"OggS")
            or opus_head < 0
            or data[opus_head + 9:opus_head + 10] != b"\x01"
            or input_sample_rate != 24000
        ):
            fail(f"{expected_file} bytes/hash/mono 24 kHz-input Ogg Opus header mismatch")
        if not isinstance(clip.get("durationMs"), int) or not MIN_MS <= clip["durationMs"] <= MAX_MS:
            fail(f"{expected_file} duration is out of range")
        total_bytes += len(data)
        total_duration += clip["durationMs"]
    if seen != set(wanted):
        fail(f"manifest covers {len(seen)} of {total} clips")

    # 每個角色三種用途都要有。少一種的那個人在畫面上的症狀是「別人會笑，她不會」。
    for persona in PERSONA_IDS:
        hers = {clip["use"] for clip in manifest["clips"] if clip["persona"] == persona}
        if hers != set(USES):
            fail(f"{persona} is missing banter uses: {sorted(set(USES) - hers)}")

    files = {path.relative_to(VOICE_ROOT).as_posix() for path in VOICE_ROOT.rglob("*.ogg")}
    expected_files = {
        f"{value['pack']}/{value['persona']}/{value['lineId']}.ogg" for value in wanted.values()
    }
    if files != expected_files:
        fail("bundled Ogg file inventory differs from manifest")
    if manifest.get("totals") != {
        "personas": len(PERSONA_IDS),
        "linesPerPersona": per_persona,
        "clips": total,
        "oggBytes": total_bytes,
        "durationMs": total_duration,
    }:
        fail("manifest totals do not derive from shipped clips")

    minified = json.dumps(manifest, ensure_ascii=False, separators=(",", ":"))
    expected_script = (
        "// Generated by scripts/promote-persona-banter-voice-assets.py; do not edit.\n"
        f"globalThis.__AI_SISTER_PERSONA_BANTER__ = {minified};\n"
    )
    if SCRIPT_PATH.read_text(encoding="utf-8") != expected_script:
        fail("manifest.js is not the exact projection of manifest.json")
    print(f"PASS: {total} bundled persona banter Ogg clips, {total_bytes} bytes")


if __name__ == "__main__":
    main()
