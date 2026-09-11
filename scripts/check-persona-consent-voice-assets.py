#!/usr/bin/env python3
"""Verify the bundled 17 x 4 consent-reading library without decoding audio."""

from __future__ import annotations

import hashlib
import json
import re
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
CATALOG_PATH = ROOT / "apps/desktop/ui/persona-consent-voices/catalog-v1.json"
VOICE_ROOT = ROOT / "apps/desktop/ui/persona-consent-voices/v1"
MANIFEST_PATH = VOICE_ROOT / "manifest.json"
SCRIPT_PATH = VOICE_ROOT / "manifest.js"
CONSENT_PATH = ROOT / "crates/sister-core/src/consent.rs"
HTML_PATH = ROOT / "apps/desktop/ui/index.html"
APP_PATH = ROOT / "apps/desktop/ui/app.js"
PERSONA_IDS = (
    "chatgpt", "claude", "gemini", "grok", "deepseek", "qwen", "mistral",
    "venice", "sakana", "perplexity", "glm", "kimi", "hunyuan", "minimax",
    "nemotron", "cohere", "mimo",
)
SHEETS = (
    ("local-recording", "LocalRecording"),
    ("cloud-reading", "CloudReading"),
    ("frame-storage", "FrameStorage"),
    ("azure-tts", "AzureTts"),
)
OWNER_SCOPE = (
    "Unmodified inclusion of the 68 generated consent-reading clips hash-listed by "
    "this manifest in the AI-Sister source tree and official builds"
)


def fail(message: str) -> None:
    raise SystemExit(f"FAIL: {message}")


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def rust_wordings() -> dict[str, str]:
    source = CONSENT_PATH.read_text(encoding="utf-8")
    try:
        body = source.split("pub fn wording", 1)[1].split("pub fn without", 1)[0]
    except IndexError:
        fail("cannot isolate the Rust consent wording source")
    result = {}
    for key, variant in SHEETS:
        match = re.search(
            rf"Sheet::{variant}\s*=>\s*(?:\{{\s*)?\"([^\"]+)\"", body
        )
        if match is None:
            fail(f"cannot read Sheet::{variant} wording")
        result[key] = match.group(1)
    return result


def main() -> None:
    catalog = json.loads(CATALOG_PATH.read_text(encoding="utf-8"))
    manifest = json.loads(MANIFEST_PATH.read_text(encoding="utf-8"))
    if catalog.get("schema") != "ai-sister/persona-consent-voice-catalog/v1":
        fail("catalog schema changed")
    people = catalog.get("personas", [])
    sheets = catalog.get("sheets", [])
    if tuple(row.get("id") for row in people) != PERSONA_IDS:
        fail("catalog roster changed")
    if tuple(row.get("key") for row in sheets) != tuple(key for key, _ in SHEETS):
        fail("catalog sheet order changed")
    wordings = rust_wordings()
    if {row["key"]: row.get("text") for row in sheets} != wordings:
        fail("voice catalog no longer exactly matches native consent wording")
    if catalog.get("totals") != {
        "personas": 17,
        "sheetsPerPersona": 4,
        "audioClips": 68,
    }:
        fail("catalog totals changed")

    if manifest.get("schema") != "ai-sister/persona-consent-voices/v1":
        fail("manifest schema changed")
    if (
        manifest.get("locale") != "zh-TW"
        or manifest.get("roster") != "four-sisters-plus-thirteen-besties"
        or manifest.get("sheets") != [key for key, _ in SHEETS]
    ):
        fail("locale, roster, or sheet list changed")
    if manifest.get("engine") != {
        "name": "MediaTek-Research/BreezyVoice-300M",
        "modelSnapshot": "e33b502e0ac21c16b0ee0d00df66ac3fa737393d",
        "license": "Apache-2.0",
    }:
        fail("generation provenance changed")
    if (
        manifest.get("rightsReview") != "approved-owner-grant"
        or manifest.get("ownerGrant") != {
            "grantedOn": "2026-09-11",
            "grantor": "Ted Huang",
            "license": "excluded-from-Apache-2.0",
            "scope": OWNER_SCOPE,
        }
        or manifest.get("notice") != "NOTICE.md"
    ):
        fail("owner grant is not pinned")

    wanted = {
        (person["id"], sheet["key"]): {
            "persona": person["id"],
            "group": person["group"],
            "sheet": sheet["key"],
            "text": sheet["text"],
        }
        for person in people
        for sheet in sheets
    }
    if len(wanted) != 68:
        fail("catalog does not expand to 68 clips")
    seen = set()
    total_bytes = 0
    total_duration = 0
    for clip in manifest.get("clips", []):
        key = (clip.get("persona"), clip.get("sheet"))
        if key in seen or key not in wanted:
            fail(f"duplicate or unknown clip: {key}")
        seen.add(key)
        for field, value in wanted[key].items():
            if clip.get(field) != value:
                fail(f"{key} {field} differs from the consent catalog")
        expected_file = f"{clip['persona']}/{clip['sheet']}.ogg"
        if clip.get("file") != expected_file:
            fail(f"{key} has non-canonical path")
        path = VOICE_ROOT / expected_file
        try:
            data = path.read_bytes()
        except OSError as error:
            fail(f"cannot read {expected_file}: {error}")
        opus_head = data[:256].find(b"OpusHead")
        input_rate = (
            int.from_bytes(data[opus_head + 12:opus_head + 16], "little")
            if opus_head >= 0 and len(data) >= opus_head + 19 else None
        )
        if (
            len(data) != clip.get("bytes")
            or sha256(path) != clip.get("sha256")
            or not data.startswith(b"OggS")
            or opus_head < 0
            or data[opus_head + 9:opus_head + 10] != b"\x01"
            or input_rate != 24000
        ):
            fail(f"{expected_file} bytes/hash/mono 24 kHz-input Opus header mismatch")
        if not isinstance(clip.get("durationMs"), int) or not 700 <= clip["durationMs"] <= 90000:
            fail(f"{expected_file} duration is out of range")
        total_bytes += len(data)
        total_duration += clip["durationMs"]
    if seen != set(wanted):
        fail(f"manifest covers {len(seen)} of 68 clips")

    files = {path.relative_to(VOICE_ROOT).as_posix() for path in VOICE_ROOT.rglob("*.ogg")}
    expected_files = {f"{persona}/{sheet}.ogg" for persona, sheet in wanted}
    if files != expected_files:
        fail("bundled Ogg inventory differs from manifest")
    if manifest.get("totals") != {
        "personas": 17,
        "sheetsPerPersona": 4,
        "clips": 68,
        "oggBytes": total_bytes,
        "durationMs": total_duration,
    }:
        fail("manifest totals do not derive from shipped clips")

    minified = json.dumps(manifest, ensure_ascii=False, separators=(",", ":"))
    expected_script = (
        "// Generated by scripts/promote-persona-consent-voice-assets.py; do not edit.\n"
        f"globalThis.__AI_SISTER_CONSENT_VOICES__ = {minified};\n"
    )
    if SCRIPT_PATH.read_text(encoding="utf-8") != expected_script:
        fail("manifest.js is not the exact projection of manifest.json")

    html = HTML_PATH.read_text(encoding="utf-8")
    app = APP_PATH.read_text(encoding="utf-8")
    voice_script = '<script src="./persona-consent-voices/v1/manifest.js"></script>'
    if voice_script not in html or html.index(voice_script) > html.index('src="./app.js"'):
        fail("consent voice manifest is not loaded before app.js")
    for required in (
        "globalThis.__AI_SISTER_CONSENT_VOICES__",
        "clip?.text === sheet?.wording",
        "event?.isTrusted !== true",
        'invoke("persona_fixed_voice_admit")',
    ):
        if required not in app:
            fail(f"renderer consent voice boundary missing: {required}")
    print(f"PASS: 68 bundled persona consent Ogg clips, {total_bytes} bytes")


if __name__ == "__main__":
    main()
