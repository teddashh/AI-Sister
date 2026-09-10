#!/usr/bin/env python3
"""Verify the bundled 17-persona dialogue library without decoding audio."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
CATALOG_PATH = ROOT / "apps/desktop/ui/persona-voices/catalog-v1.json"
VOICE_ROOT = ROOT / "apps/desktop/ui/persona-voices/v1"
MANIFEST_PATH = VOICE_ROOT / "manifest.json"
SCRIPT_PATH = VOICE_ROOT / "manifest.js"
PERSONA_IDS = (
    "chatgpt", "claude", "gemini", "grok", "deepseek", "qwen", "mistral",
    "venice", "sakana", "perplexity", "glm", "kimi", "hunyuan", "minimax",
    "nemotron", "cohere", "mimo",
)


def fail(message: str) -> None:
    raise SystemExit(f"FAIL: {message}")


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main() -> None:
    catalog = json.loads(CATALOG_PATH.read_text(encoding="utf-8"))
    manifest = json.loads(MANIFEST_PATH.read_text(encoding="utf-8"))
    if manifest.get("schema") != "ai-sister/persona-voices/v1":
        fail("manifest schema changed")
    if manifest.get("locale") != "zh-TW" or manifest.get("roster") != catalog.get("roster"):
        fail("locale or roster changed")
    if manifest.get("packs") != catalog.get("packs"):
        fail("manifest packs do not exactly match the dialogue catalog")
    if manifest.get("rightsReview") != "approved-owner-grant":
        fail("rights review is not pinned")
    if manifest.get("ownerGrant", {}).get("grantedOn") != "2026-09-09":
        fail("owner grant is not pinned")
    if manifest.get("engine") != {
        "name": "MediaTek-Research/BreezyVoice-300M",
        "modelSnapshot": "e33b502e0ac21c16b0ee0d00df66ac3fa737393d",
        "license": "Apache-2.0",
    }:
        fail("generation engine provenance changed")

    wanted = {}
    people = catalog.get("personas", [])
    lines = catalog.get("lines", [])
    if tuple(person.get("id") for person in people) != PERSONA_IDS or len(lines) != 32:
        fail("catalog roster or line count changed")
    for person in people:
        for line in lines:
            key = (person["id"], line["id"])
            wanted[key] = {
                "persona": person["id"],
                "group": person["group"],
                "lineId": line["id"],
                "pack": line["pack"],
                "use": line["use"],
                "text": line["text"].format(
                    lead=person["lead"], approach=person["approach"]
                ),
                "triggers": line["triggers"],
            }
    if len(wanted) != 544:
        fail(f"catalog expands to {len(wanted)} clips, expected 544")

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
        if not isinstance(clip.get("durationMs"), int) or not 700 <= clip["durationMs"] <= 12000:
            fail(f"{expected_file} duration is out of range")
        total_bytes += len(data)
        total_duration += clip["durationMs"]
    if seen != set(wanted):
        fail(f"manifest covers {len(seen)} of 544 clips")

    files = {
        path.relative_to(VOICE_ROOT).as_posix()
        for path in VOICE_ROOT.rglob("*.ogg")
    }
    expected_files = {f"{value['pack']}/{value['persona']}/{value['lineId']}.ogg" for value in wanted.values()}
    if files != expected_files:
        fail("bundled Ogg file inventory differs from manifest")
    totals = manifest.get("totals")
    if totals != {
        "personas": 17,
        "baseLinesPerPersona": 8,
        "extensionLinesPerPersona": 24,
        "clips": 544,
        "oggBytes": total_bytes,
        "durationMs": total_duration,
    }:
        fail("manifest totals do not derive from shipped clips")

    minified = json.dumps(manifest, ensure_ascii=False, separators=(",", ":"))
    expected_script = (
        "// Generated by scripts/promote-persona-voice-assets.py; do not edit.\n"
        f"globalThis.__AI_SISTER_PERSONA_VOICES__ = {minified};\n"
    )
    if SCRIPT_PATH.read_text(encoding="utf-8") != expected_script:
        fail("manifest.js is not the exact projection of manifest.json")
    print(f"PASS: 544 bundled persona Ogg clips, {total_bytes} bytes")


if __name__ == "__main__":
    main()
