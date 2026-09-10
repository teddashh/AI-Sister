#!/usr/bin/env python3
"""Validate the complete 17-persona, 544-clip local dialogue contract."""

from __future__ import annotations

import json
import re
import string
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
CATALOG = ROOT / "apps" / "desktop" / "ui" / "persona-voices" / "catalog-v1.json"
PERSONA_IDS = (
    "chatgpt",
    "claude",
    "gemini",
    "grok",
    "deepseek",
    "qwen",
    "mistral",
    "venice",
    "sakana",
    "perplexity",
    "glm",
    "kimi",
    "hunyuan",
    "minimax",
    "nemotron",
    "cohere",
    "mimo",
)
PACK_COUNTS = {"base": 8, "extension": 24}
PLACEHOLDERS = {"lead", "approach"}
ID_RE = re.compile(r"^[a-z][a-z0-9]*(?:-[a-z0-9]+)*$")


def require(condition: bool, message: str) -> None:
    if not condition:
        raise SystemExit(message)


def exact_fields(value: dict, expected: set[str], label: str) -> None:
    require(set(value) == expected, f"{label} fields: {sorted(value)}")


def rendered_text(template: str, persona: dict) -> str:
    fields = []
    for _, field, spec, conversion in string.Formatter().parse(template):
        if field is None:
            continue
        require(field in PLACEHOLDERS, f"unknown text placeholder: {field}")
        require(spec == "" and conversion is None, f"formatted placeholder is not allowed: {field}")
        fields.append(field)
    require(len(fields) == len(set(fields)), f"placeholder repeated in one line: {template}")
    text = template.format(lead=persona["lead"], approach=persona["approach"])
    require("{" not in text and "}" not in text, f"unrendered text: {text}")
    require(text == text.strip() and "\n" not in text and "\r" not in text, f"invalid whitespace: {text!r}")
    require(4 <= len(text) <= 48, f"spoken line length is outside 4..48: {text}")
    return text


def main() -> None:
    raw = CATALOG.read_bytes()
    require(raw.endswith(b"\n"), "catalog must end in one newline")
    document = json.loads(raw)
    exact_fields(
        document,
        {"schema", "locale", "roster", "personas", "packs", "lines", "totals"},
        "catalog",
    )
    require(document["schema"] == "ai-sister/persona-dialogue-catalog/v1", "schema mismatch")
    require(document["locale"] == "zh-TW", "locale mismatch")
    require(document["roster"] == "four-sisters-plus-thirteen-besties", "roster mismatch")

    personas = document["personas"]
    require(isinstance(personas, list), "personas must be a list")
    require(tuple(row.get("id") for row in personas) == PERSONA_IDS, "persona order or IDs drifted")
    for index, persona in enumerate(personas):
        exact_fields(persona, {"id", "group", "lead", "approach"}, f"persona[{index}]")
        require(persona["group"] == ("sister" if index < 4 else "bestie"), f"group mismatch: {persona['id']}")
        for key in ("lead", "approach"):
            value = persona[key]
            require(isinstance(value, str) and value == value.strip() and value, f"bad {key}: {persona['id']}")
            require("{" not in value and "}" not in value and "\n" not in value, f"unsafe {key}: {persona['id']}")

    packs = document["packs"]
    require(isinstance(packs, list) and [row.get("id") for row in packs] == list(PACK_COUNTS), "pack order drifted")
    pack_lines: dict[str, tuple[str, ...]] = {}
    for pack in packs:
        exact_fields(pack, {"id", "delivery", "lineIds"}, f"pack {pack.get('id')}")
        pack_id = pack["id"]
        require(pack["delivery"] == "bundled", f"{pack_id} must stay local and bundled")
        ids = tuple(pack["lineIds"])
        require(len(ids) == PACK_COUNTS[pack_id] and len(set(ids)) == len(ids), f"{pack_id} line count")
        pack_lines[pack_id] = ids

    lines = document["lines"]
    require(isinstance(lines, list) and len(lines) == 32, "catalog must define 32 lines")
    line_ids = tuple(row.get("id") for row in lines)
    require(len(set(line_ids)) == len(line_ids), "line IDs must be unique")
    require(line_ids == pack_lines["base"] + pack_lines["extension"], "line order must equal base + extension")

    triggers: dict[str, str] = {}
    audio_keys: set[tuple[str, str]] = set()
    rendered: set[tuple[str, str]] = set()
    for index, line in enumerate(lines):
        exact_fields(line, {"id", "pack", "use", "text", "triggers"}, f"line[{index}]")
        line_id = line["id"]
        require(isinstance(line_id, str) and ID_RE.fullmatch(line_id), f"bad line ID: {line_id!r}")
        pack_id = line["pack"]
        require(pack_id in PACK_COUNTS and line_id in pack_lines[pack_id], f"line pack mismatch: {line_id}")
        expected_use = "avatar-tap" if index < 2 else "exact-intent-reply"
        require(line["use"] == expected_use, f"line use mismatch: {line_id}")
        require(isinstance(line["text"], str), f"line text is not a string: {line_id}")
        values = line["triggers"]
        require(isinstance(values, list), f"line triggers must be a list: {line_id}")
        require((index < 2) == (values == []), f"only tap lines may omit triggers: {line_id}")
        for trigger in values:
            require(isinstance(trigger, str) and trigger == trigger.strip() and trigger, f"bad trigger: {line_id}")
            normalized = trigger.casefold()
            require(normalized not in triggers, f"trigger collision: {trigger} ({triggers.get(normalized)} / {line_id})")
            triggers[normalized] = line_id
        for persona in personas:
            text = rendered_text(line["text"], persona)
            key = (persona["id"], line_id)
            require(key not in audio_keys, f"duplicate audio key: {key}")
            audio_keys.add(key)
            rendered.add((persona["id"], text))

    totals = document["totals"]
    exact_fields(
        totals,
        {"personas", "baseLinesPerPersona", "extensionLinesPerPersona", "linesPerPersona", "audioClips"},
        "totals",
    )
    expected_totals = {
        "personas": len(PERSONA_IDS),
        "baseLinesPerPersona": PACK_COUNTS["base"],
        "extensionLinesPerPersona": PACK_COUNTS["extension"],
        "linesPerPersona": sum(PACK_COUNTS.values()),
        "audioClips": len(PERSONA_IDS) * sum(PACK_COUNTS.values()),
    }
    require(totals == expected_totals, f"totals mismatch: {totals}")
    require(len(audio_keys) == 544 and len(rendered) == 544, "catalog must resolve to 544 distinct persona lines")
    print(f"PASS: {len(PERSONA_IDS)} personas × 32 lines = {len(audio_keys)} local clips")


if __name__ == "__main__":
    main()
