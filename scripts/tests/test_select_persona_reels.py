#!/usr/bin/env python3
"""Focused tests for scripts/select-persona-reels.py."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path, PurePosixPath
import struct
import subprocess
import sys
import tempfile
import unittest
import zlib


ROOT = Path(__file__).resolve().parents[2]
SELECTOR = ROOT / "scripts" / "select-persona-reels.py"
PERSONAS = (
    "claude",
    "gemini",
    "grok",
    "chatgpt",
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
LAYER_TAGS = (
    "mouth",
    "eyebrow_l",
    "eyebrow_r",
    "eyewhite_l",
    "eyewhite_r",
    "source_residual",
    "face",
    "irides_l",
    "irides_r",
    "eyelash_l",
    "eyelash_r",
)


def png_chunk(kind: bytes, payload: bytes) -> bytes:
    crc = zlib.crc32(kind)
    crc = zlib.crc32(payload, crc) & 0xFFFFFFFF
    return struct.pack(">I", len(payload)) + kind + payload + struct.pack(">I", crc)


def rgba_png(width: int, height: int, rgba: bytes) -> bytes:
    rows = b"".join(b"\x00" + rgba * width for _ in range(height))
    return (
        b"\x89PNG\r\n\x1a\n"
        + png_chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
        + png_chunk(b"IDAT", zlib.compress(rows))
        + png_chunk(b"IEND", b"")
    )


def write_json(path: Path, value: object) -> None:
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def build_fixture(root: Path) -> None:
    (root / "parts").mkdir(parents=True)
    (root / "outfits_v2_norm").mkdir()
    for persona_index, persona in enumerate(PERSONAS):
        source = root / "outfits_v2_norm" / f"doll_{persona}__workplace.png"
        source.write_bytes(rgba_png(3, 4, bytes((persona_index, 90, 150, 255))))
        source_sha256 = hashlib.sha256(source.read_bytes()).hexdigest()
        canvas_sha256 = hashlib.sha256(f"canvas:{persona}".encode()).hexdigest()

        rig = root / "parts" / f"{persona}__workplace__v2_parts"
        rig.mkdir()
        layers = []
        for z, tag in enumerate(LAYER_TAGS):
            filename = f"{z:02d}_{tag}.png"
            (rig / filename).write_bytes(rgba_png(2, 2, bytes((z, persona_index, 200, 255))))
            layers.append(
                {
                    "tag": tag,
                    "name": f"private display name for {tag}",
                    "z": z,
                    "x": 10 + z,
                    "y": 20 + z,
                    "w": 2,
                    "h": 2,
                    "cx": 11 + z,
                    "cy": 21 + z,
                    "file": filename,
                }
            )
        write_json(
            rig / "parts.json",
            {
                "src": f"/private/research/{persona}/model.psd",
                "canvas_w": 1280,
                "canvas_h": 1280,
                "parts": layers,
                "source_residual": {
                    "version": "source_canvas_residual_v1",
                    "source_canvas_sha256": canvas_sha256,
                    "missing_pixel_count": 1,
                },
            },
        )
        write_json(
            rig / "source.json",
            {
                "tool": "decompose_persona_rigs.py",
                "source": f"/private/generated/{persona}.png",
                "source_sha256": source_sha256,
                "decomposed_at": "private timestamp",
                "name": persona,
                "theme": "workplace",
                "canvas_contract_version": "see_through_center_pad_v1",
                "canvas_sha256": canvas_sha256,
            },
        )


class PersonaReelSelectorTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.base = Path(self.temporary.name)
        self.tachie = self.base / "tachie"
        build_fixture(self.tachie)
        self.output = self.base / "runtime"

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def run_selector(self, *extra: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [
                sys.executable,
                str(SELECTOR),
                "--tachie-root",
                str(self.tachie),
                "--output",
                str(self.output),
                *extra,
            ],
            cwd=ROOT,
            text=True,
            capture_output=True,
            check=False,
        )

    def test_default_is_sanitized_dry_run(self) -> None:
        result = self.run_selector()
        self.assertEqual(0, result.returncode, result.stderr)
        self.assertFalse(self.output.exists())
        manifest = json.loads(result.stdout)
        self.assertEqual("ai-sister/persona-reels/v1", manifest["schema"])
        self.assertEqual(17, manifest["totals"]["rigs"])
        self.assertEqual(len(PERSONAS) * len(LAYER_TAGS), manifest["totals"]["layers"])
        self.assertEqual("approved-owner-grant", manifest["rights_review"])
        self.assertEqual("Ted Huang", manifest["owner_grant"]["grantor"])
        self.assertEqual("excluded-from-Apache-2.0", manifest["owner_grant"]["license"])
        self.assertEqual("NOTICE.md", manifest["notice"])
        serialized = json.dumps(manifest)
        self.assertNotIn(str(self.base), serialized)
        self.assertNotIn("source.json", serialized)
        self.assertNotIn('"src"', serialized)
        self.assertNotIn("private display name", serialized)
        for rig in manifest["rigs"]:
            self.assertEqual(
                {"x": 320, "y": 0, "width": 640, "height": 640}, rig["viewport"]
            )
            for layer in rig["layers"]:
                path = PurePosixPath(layer["file"])
                self.assertFalse(path.is_absolute())
                self.assertNotIn("..", path.parts)
                self.assertIn(layer["role"], {"body", "mouth", "eye", "brow"})
            self.assertEqual(
                list(range(len(LAYER_TAGS))),
                sorted(layer["render_z"] for layer in rig["layers"]),
            )
            by_tag = {layer["tag"]: layer for layer in rig["layers"]}
            self.assertLess(by_tag["eyewhite_l"]["render_z"], by_tag["eyebrow_l"]["render_z"])
            self.assertLess(by_tag["eyewhite_r"]["render_z"], by_tag["eyebrow_r"]["render_z"])

    def test_write_copies_only_referenced_pngs_and_refuses_overwrite(self) -> None:
        debug = self.tachie / "parts" / "glm__workplace__v2_parts" / "_flat.png"
        debug.write_bytes(rgba_png(1, 1, b"\x00\x00\x00\xff"))
        result = self.run_selector("--write")
        self.assertEqual(0, result.returncode, result.stderr)
        manifest_path = self.output / "manifest.json"
        self.assertTrue(manifest_path.is_file())
        files = {
            path.relative_to(self.output).as_posix()
            for path in self.output.rglob("*")
            if path.is_file()
        }
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        expected = {"manifest.json", "manifest.js", "NOTICE.md"}
        expected.update(layer["file"] for rig in manifest["rigs"] for layer in rig["layers"])
        self.assertEqual(expected, files)
        self.assertFalse((self.output / "rigs" / "glm" / "_flat.png").exists())
        self.assertFalse(
            any(
                path.name in {"source.json", "parts.json"}
                for path in self.output.rglob("*")
            )
        )
        runtime = (self.output / "manifest.js").read_text(encoding="utf-8")
        prefix = "globalThis.__AI_SISTER_PERSONA_REELS__ = "
        self.assertTrue(
            runtime.startswith("// Generated by scripts/select-persona-reels.py; do not edit.\n")
        )
        payload = runtime.split(prefix, 1)[1].removesuffix(";\n")
        self.assertEqual(manifest, json.loads(payload))
        notice = (self.output / "NOTICE.md").read_text(encoding="utf-8")
        self.assertEqual("# AI-Sister workplace Persona rigs\n", notice.splitlines(keepends=True)[0])
        self.assertIn(hashlib.sha256(manifest_path.read_bytes()).hexdigest(), notice)

        before = manifest_path.read_bytes()
        second = self.run_selector("--write")
        self.assertNotEqual(0, second.returncode)
        self.assertIn("will not be overwritten", second.stderr)
        self.assertEqual(before, manifest_path.read_bytes())

    def test_declared_part_dimensions_must_match_png(self) -> None:
        parts_path = self.tachie / "parts" / "claude__workplace__v2_parts" / "parts.json"
        parts = json.loads(parts_path.read_text(encoding="utf-8"))
        parts["parts"][0]["w"] = 3
        write_json(parts_path, parts)
        result = self.run_selector()
        self.assertNotEqual(0, result.returncode)
        self.assertIn("declared 3x2, PNG is 2x2", result.stderr)
        self.assertFalse(self.output.exists())

    def test_source_receipt_hash_is_enforced(self) -> None:
        source = self.tachie / "outfits_v2_norm" / "doll_claude__workplace.png"
        source.write_bytes(rgba_png(3, 4, b"\xff\x00\x00\xff"))
        result = self.run_selector()
        self.assertNotEqual(0, result.returncode)
        self.assertIn("does not match source.json SHA-256", result.stderr)
        self.assertFalse(self.output.exists())

    def test_symlinked_part_is_rejected(self) -> None:
        rig = self.tachie / "parts" / "claude__workplace__v2_parts"
        mouth = rig / "00_mouth.png"
        mouth.unlink()
        mouth.symlink_to(rig / "01_eyebrow_l.png")
        result = self.run_selector()
        self.assertNotEqual(0, result.returncode)
        self.assertIn("expected a regular file, not a symlink", result.stderr)
        self.assertFalse(self.output.exists())

    def test_absolute_part_path_is_rejected(self) -> None:
        parts_path = self.tachie / "parts" / "claude__workplace__v2_parts" / "parts.json"
        parts = json.loads(parts_path.read_text(encoding="utf-8"))
        parts["parts"][0]["file"] = "/private/leak.png"
        write_json(parts_path, parts)
        result = self.run_selector()
        self.assertNotEqual(0, result.returncode)
        self.assertIn("one relative PNG basename", result.stderr)
        self.assertFalse(self.output.exists())

    def test_existing_output_is_rejected_even_in_dry_run(self) -> None:
        self.output.mkdir()
        sentinel = self.output / "keep.txt"
        sentinel.write_text("do not replace", encoding="utf-8")
        result = self.run_selector()
        self.assertNotEqual(0, result.returncode)
        self.assertIn("will not be overwritten", result.stderr)
        self.assertEqual("do not replace", sentinel.read_text(encoding="utf-8"))

    def test_output_inside_git_worktree_is_rejected(self) -> None:
        repository = self.base / "repository"
        (repository / ".git").mkdir(parents=True)
        (repository / ".git" / "HEAD").write_text("ref: refs/heads/main\n", encoding="utf-8")
        self.output = repository / "runtime"
        result = self.run_selector()
        self.assertNotEqual(0, result.returncode)
        self.assertIn("outside a Git worktree", result.stderr)
        self.assertFalse(self.output.exists())


if __name__ == "__main__":
    unittest.main()
