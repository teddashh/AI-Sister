#!/usr/bin/env python3
"""Verify and promote the private dialogue candidates into bundled Ogg assets."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import tempfile
from pathlib import Path


PERSONA_IDS = (
    "chatgpt", "claude", "gemini", "grok", "deepseek", "qwen", "mistral",
    "venice", "sakana", "perplexity", "glm", "kimi", "hunyuan", "minimax",
    "nemotron", "cohere", "mimo",
)
LINE_ID_CHARS = frozenset("abcdefghijklmnopqrstuvwxyz0123456789-")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def canonical_sha256(value: object) -> str:
    encoded = json.dumps(
        value, ensure_ascii=False, sort_keys=True, separators=(",", ":")
    ).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def rendered_lines(catalog: dict) -> list[dict]:
    result = []
    for persona in catalog["personas"]:
        for line in catalog["lines"]:
            result.append(
                {
                    "persona": persona["id"],
                    "group": persona["group"],
                    "lineId": line["id"],
                    "pack": line["pack"],
                    "use": line["use"],
                    "text": line["text"].format(
                        lead=persona["lead"], approach=persona["approach"]
                    ),
                    "triggers": line["triggers"],
                }
            )
    return result


def probe_ogg(path: Path) -> dict:
    result = subprocess.run(
        [
            "ffprobe", "-v", "error", "-select_streams", "a:0",
            "-show_entries", "stream=codec_name,channels,sample_rate,duration",
            "-of", "json", os.fspath(path),
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    payload = json.loads(result.stdout)
    streams = payload.get("streams", [])
    if len(streams) != 1:
        raise RuntimeError(f"expected one audio stream: {path}")
    stream = streams[0]
    header = path.read_bytes()[:256]
    opus_head = header.find(b"OpusHead")
    input_sample_rate = (
        int.from_bytes(header[opus_head + 12:opus_head + 16], "little")
        if opus_head >= 0 and len(header) >= opus_head + 19
        else None
    )
    if (
        stream.get("codec_name") != "opus"
        or stream.get("channels") != 1
        # RFC 7845 Opus output is always decoded at 48 kHz. OpusHead separately
        # records the 24 kHz input rate selected by the release encoder.
        or stream.get("sample_rate") != "48000"
        or header[opus_head + 9:opus_head + 10] != b"\x01"
        or input_sample_rate != 24000
    ):
        raise RuntimeError(f"unexpected bundled audio format: {path}: {stream}")
    duration_ms = round(float(stream["duration"]) * 1000)
    if not 700 <= duration_ms <= 12000:
        raise RuntimeError(f"unexpected bundled audio duration: {path}: {duration_ms} ms")
    return {"durationMs": duration_ms}


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--catalog", type=Path, required=True)
    parser.add_argument("--candidates", type=Path, required=True)
    parser.add_argument("--qc-report", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()

    if args.output.exists():
        raise SystemExit(f"refusing to replace existing output: {args.output}")
    catalog = json.loads(args.catalog.read_text(encoding="utf-8"))
    candidate_manifest_path = args.candidates / "manifest.json"
    candidates = json.loads(candidate_manifest_path.read_text(encoding="utf-8"))
    qc = json.loads(args.qc_report.read_text(encoding="utf-8"))
    catalog_hash = canonical_sha256(catalog)
    candidate_hash = sha256(candidate_manifest_path)
    if catalog.get("schema") != "ai-sister/persona-dialogue-catalog/v1":
        raise SystemExit("unexpected catalog schema")
    if tuple(row.get("id") for row in catalog.get("personas", [])) != PERSONA_IDS:
        raise SystemExit("catalog roster mismatch")
    if (
        candidates.get("schema") != "ai-sister/persona-dialogue-candidates/v1"
        or candidates.get("catalogSha256") != catalog_hash
        or qc.get("schema") != "ai-sister/persona-dialogue-qc/v1"
        or qc.get("catalogSha256") != catalog_hash
        or qc.get("candidateManifestSha256") != candidate_hash
        or qc.get("totals") != {"clips": 544, "passed": 544, "failed": 0}
    ):
        raise SystemExit("catalog, candidate manifest, and QC report do not form one release")

    wanted = rendered_lines(catalog)
    if len(wanted) != 544:
        raise SystemExit(f"expected 544 rendered lines, got {len(wanted)}")
    candidate_by_key = {
        (row.get("persona"), row.get("lineId")): row
        for row in candidates.get("clips", [])
    }
    qc_by_key = {
        (row.get("persona"), row.get("lineId")): row for row in qc.get("clips", [])
    }
    if len(candidate_by_key) != 544 or len(qc_by_key) != 544:
        raise SystemExit("candidate or QC key set is incomplete")

    args.output.parent.mkdir(parents=True, exist_ok=True)
    staging = Path(tempfile.mkdtemp(prefix="persona-voices-v1-", dir=args.output.parent))
    try:
        clips = []
        for row in wanted:
            key = (row["persona"], row["lineId"])
            candidate = candidate_by_key.get(key)
            checked = qc_by_key.get(key)
            if candidate is None or checked is None or checked.get("pass") is not True:
                raise RuntimeError(f"missing passed candidate: {key}")
            for field in ("persona", "lineId", "pack", "use", "text", "triggers"):
                if candidate.get(field) != row[field]:
                    raise RuntimeError(f"candidate field mismatch: {key}: {field}")
            if set(row["lineId"]) - LINE_ID_CHARS:
                raise RuntimeError(f"unsafe line id: {row['lineId']}")
            source = args.candidates / candidate["ogg"]["file"]
            if (
                not source.is_file()
                or source.stat().st_size != candidate["ogg"]["bytes"]
                or sha256(source) != candidate["ogg"]["sha256"]
            ):
                raise RuntimeError(f"candidate Ogg changed after QC: {key}")
            probe = probe_ogg(source)
            if not isinstance(checked.get("integratedLufs"), (int, float)) or not isinstance(
                checked.get("truePeakDbtp"), (int, float)
            ):
                raise RuntimeError(f"QC report predates the loudness gate: {key}")
            if abs(probe["durationMs"] - checked["durationMs"]) > 100:
                raise RuntimeError(f"Ogg/WAV duration mismatch: {key}")
            relative = Path(row["pack"]) / row["persona"] / f"{row['lineId']}.ogg"
            destination = staging / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, destination)
            clips.append(
                {
                    **row,
                    "file": relative.as_posix(),
                    "bytes": destination.stat().st_size,
                    "sha256": sha256(destination),
                    # 這一版起，出貨的 manifest 帶著品管量到的響度。這不是裝飾：
                    # 它是「每一支都對齊過」那句話唯一在 repo 裡查得到的依據，而
                    # `scripts/check-persona-voice-loudness.py` 會拿真的 Ogg 重量
                    # 一次去對它——寫在這裡的數字要對得上解出來的聲音。
                    "integratedLufs": checked["integratedLufs"],
                    "truePeakDbtp": checked["truePeakDbtp"],
                    **probe,
                }
            )

        manifest = {
            "schema": "ai-sister/persona-voices/v1",
            # 出貨的音檔在錄音那邊做過什麼。寫進 manifest 而不是只寫在 NOTICE
            # 裡，是因為「沒有做過壓縮」這種話要有人守得住——閘門讀得到才守得住。
            "postProcessing": {
                "loudness": {
                    "standard": "EBU R128",
                    "targetLufs": qc["limits"]["targetLufs"],
                    "ceilingDbtp": qc["limits"]["ceilingDbtp"],
                    "toleranceLu": qc["limits"]["loudnessTolerance"],
                    "method": "one constant gain per clip; no compression, limiting, or other dynamics processing",
                },
            },
            "locale": "zh-TW",
            "roster": "four-sisters-plus-thirteen-besties",
            "engine": {
                "name": "MediaTek-Research/BreezyVoice-300M",
                "modelSnapshot": "e33b502e0ac21c16b0ee0d00df66ac3fa737393d",
                "license": "Apache-2.0",
            },
            "rightsReview": "approved-owner-grant",
            "ownerGrant": {
                "grantedOn": "2026-09-09",
                "grantor": "Ted Huang",
                "license": "excluded-from-Apache-2.0",
                "scope": "Unmodified inclusion of the 544 generated fixed-dialogue clips hash-listed by this manifest in the AI-Sister source tree and official builds",
            },
            "notice": "NOTICE.md",
            "packs": catalog["packs"],
            "clips": clips,
            "totals": {
                "personas": 17,
                "baseLinesPerPersona": 8,
                "extensionLinesPerPersona": 24,
                "clips": 544,
                "oggBytes": sum(row["bytes"] for row in clips),
                "durationMs": sum(row["durationMs"] for row in clips),
            },
        }
        manifest_text = json.dumps(manifest, ensure_ascii=False, indent=2) + "\n"
        (staging / "manifest.json").write_text(manifest_text, encoding="utf-8")
        minified = json.dumps(manifest, ensure_ascii=False, separators=(",", ":"))
        (staging / "manifest.js").write_text(
            "// Generated by scripts/promote-persona-voice-assets.py; do not edit.\n"
            f"globalThis.__AI_SISTER_PERSONA_VOICES__ = {minified};\n",
            encoding="utf-8",
        )
        (staging / "NOTICE.md").write_text(
            "# Bundled persona dialogue voices\n\n"
            "The 544 Ogg Opus files in this directory were generated locally with "
            "MediaTek Research's BreezyVoice-300M model. The model and inference code "
            "are available under Apache-2.0.\n\n"
            "Before they were hash-listed, the clips were levelled in the voice lab: one "
            "constant gain each to EBU R128 -23 LUFS under a -1 dBTP true-peak "
            "ceiling, a 3 ms fade at each edge, and a trimmed tail on the clips where "
            "speech recognition heard words the written line does not contain. No "
            "compression, limiting, or other dynamics processing was applied.\n\n"
            "The generated persona voice files are excluded from this repository's "
            "Apache-2.0 license. Ted Huang granted AI-Sister permission on 2026-09-09 "
            "to include the exact, unmodified files listed by `manifest.json` in the "
            "source tree and official builds. This grant does not extend to any source "
            "recording, private receipt, training corpus, or voice-lab working file.\n",
            encoding="utf-8",
        )
        os.replace(staging, args.output)
    except BaseException:
        shutil.rmtree(staging, ignore_errors=True)
        raise
    print(
        f"PASS: promoted 544 bundled Ogg clips ({manifest['totals']['oggBytes']} bytes) "
        f"to {args.output}"
    )


if __name__ == "__main__":
    main()
