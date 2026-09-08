#!/usr/bin/env python3
"""Focused tests for scripts/select-persona-previews.py."""

from __future__ import annotations

import hashlib
import importlib.util
from io import BytesIO
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

try:
    import PIL
    from PIL import Image, ImageDraw, features
except ImportError:
    PIL = None
    Image = None
    ImageDraw = None
    features = None


ROOT = Path(__file__).resolve().parents[2]
SELECTOR = ROOT / "scripts" / "select-persona-previews.py"
PRODUCTION_REEL_MANIFEST = (
    ROOT / "apps" / "desktop" / "ui" / "persona-reels" / "manifest.json"
)
PERSONAS = (
    ("claude", "sister"),
    ("gemini", "sister"),
    ("grok", "sister"),
    ("chatgpt", "sister"),
    ("deepseek", "bestie"),
    ("qwen", "bestie"),
    ("mistral", "bestie"),
    ("venice", "bestie"),
    ("sakana", "bestie"),
    ("perplexity", "bestie"),
    ("glm", "bestie"),
    ("kimi", "bestie"),
    ("hunyuan", "bestie"),
    ("minimax", "bestie"),
    ("nemotron", "bestie"),
    ("cohere", "bestie"),
    ("mimo", "bestie"),
)
PINNED_CODEC = (
    PIL is not None
    and PIL.__version__ == "12.1.1"
    and features is not None
    and features.version("webp") == "1.5.0"
    and features.check("webp")
)


def load_selector_module():
    spec = importlib.util.spec_from_file_location(
        "ai_sister_persona_preview_selector_tests", SELECTOR
    )
    if spec is None or spec.loader is None:
        raise RuntimeError("cannot load preview selector")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


PREVIEW_SELECTOR = load_selector_module()


def deep_copy(value: object):
    return json.loads(json.dumps(value))


class PersonaPreviewAuthorityTests(unittest.TestCase):
    """These run in public CI even when the pinned image codec is unavailable."""

    @classmethod
    def setUpClass(cls) -> None:
        cls.production_raw = PRODUCTION_REEL_MANIFEST.read_bytes()
        cls.production = json.loads(cls.production_raw)

    def test_exact_production_manifest_carries_the_reviewed_authority(self) -> None:
        reviewed = PREVIEW_SELECTOR.validate_reel_manifest(
            self.production, self.production_raw
        )
        self.assertEqual(
            PREVIEW_SELECTOR.PRODUCTION_REEL_MANIFEST_SHA256,
            hashlib.sha256(self.production_raw).hexdigest(),
        )
        self.assertEqual(
            PREVIEW_SELECTOR.PRODUCTION_REEL_RIGHTS_REVIEW,
            self.production["rights_review"],
        )
        self.assertEqual(
            PREVIEW_SELECTOR.PRODUCTION_REEL_OWNER_GRANT,
            self.production["owner_grant"],
        )
        self.assertEqual(
            PREVIEW_SELECTOR.PRODUCTION_REEL_MANIFEST_SHA256, reviewed.sha256
        )
        self.assertEqual(
            [persona for persona, _group in PERSONAS],
            [row.persona for row in reviewed.sources],
        )

    def test_same_rights_words_do_not_authorize_changed_manifest_bytes(self) -> None:
        candidate = deep_copy(self.production)
        candidate["rigs"][0]["canvas_sha256"] = "0" * 64
        raw = (json.dumps(candidate, ensure_ascii=False, indent=2) + "\n").encode()

        with self.assertRaisesRegex(
            PREVIEW_SELECTOR.PreviewSelectionError,
            "not the exact reviewed production manifest",
        ):
            PREVIEW_SELECTOR.validate_reel_manifest(candidate, raw)

        same_value_different_bytes = json.dumps(
            self.production, ensure_ascii=False, separators=(",", ":")
        ).encode()
        with self.assertRaisesRegex(
            PREVIEW_SELECTOR.PreviewSelectionError,
            "not the exact reviewed production manifest",
        ):
            PREVIEW_SELECTOR.validate_reel_manifest(
                self.production, same_value_different_bytes
            )

    def test_changed_rights_projection_is_rejected_independently(self) -> None:
        cases = (
            (
                "review state",
                lambda value: value.update(rights_review="self-asserted"),
                "rights_review does not match",
            ),
            (
                "grantor",
                lambda value: value["owner_grant"].update(grantor="someone else"),
                "owner_grant does not match",
            ),
            (
                "scope",
                lambda value: value["owner_grant"].update(scope="all images"),
                "owner_grant does not match",
            ),
        )
        for name, mutate, message in cases:
            with self.subTest(name=name):
                candidate = deep_copy(self.production)
                mutate(candidate)
                raw = json.dumps(candidate).encode()
                with self.assertRaisesRegex(
                    PREVIEW_SELECTOR.PreviewSelectionError, message
                ):
                    PREVIEW_SELECTOR.validate_reel_manifest(candidate, raw)

    def test_cli_rejects_a_synthetic_manifest_before_loading_the_codec(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            canvases = base / "canvases"
            canvases.mkdir()
            output = base / "generated"
            candidate = deep_copy(self.production)
            candidate["selection"]["theme"] = "synthetic"
            manifest = base / "synthetic.json"
            manifest.write_text(json.dumps(candidate) + "\n", encoding="utf-8")

            result = subprocess.run(
                [
                    sys.executable,
                    str(SELECTOR),
                    "--canvas-root",
                    str(canvases),
                    "--reel-manifest",
                    str(manifest),
                    "--output",
                    str(output),
                ],
                cwd=ROOT,
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertNotEqual(0, result.returncode)
            self.assertIn("not the exact reviewed production manifest", result.stderr)
            self.assertNotIn("Pillow", result.stderr)
            self.assertFalse(output.exists())

    def test_only_the_exact_reviewed_preview_output_can_carry_its_grant(self) -> None:
        preview_path = ROOT / "apps" / "desktop" / "ui" / "personas" / "manifest.json"
        raw = preview_path.read_bytes()
        manifest = json.loads(raw)
        self.assertEqual(
            PREVIEW_SELECTOR.PRODUCTION_PREVIEW_MANIFEST_SHA256,
            hashlib.sha256(raw).hexdigest(),
        )
        self.assertEqual(
            PREVIEW_SELECTOR.PRODUCTION_PREVIEW_RIGHTS_REVIEW,
            manifest["rightsReview"],
        )
        self.assertEqual(
            PREVIEW_SELECTOR.PRODUCTION_PREVIEW_OWNER_GRANT,
            manifest["ownerGrant"],
        )
        PREVIEW_SELECTOR.validate_preview_manifest_authority(manifest)

        changed_bytes = deep_copy(manifest)
        changed_bytes["assets"][0]["sha256"] = "0" * 64
        with self.assertRaisesRegex(
            PREVIEW_SELECTOR.PreviewSelectionError,
            "not the exact owner-reviewed output",
        ):
            PREVIEW_SELECTOR.validate_preview_manifest_authority(changed_bytes)

        changed_grant = deep_copy(manifest)
        changed_grant["ownerGrant"]["scope"] = "future derivatives"
        with self.assertRaisesRegex(
            PREVIEW_SELECTOR.PreviewSelectionError, "ownerGrant does not match"
        ):
            PREVIEW_SELECTOR.validate_preview_manifest_authority(changed_grant)

    def test_checked_notice_is_the_only_notice_the_selector_will_write(self) -> None:
        preview_root = ROOT / "apps" / "desktop" / "ui" / "personas"
        manifest = json.loads((preview_root / "manifest.json").read_bytes())
        checked_notice = (preview_root / "NOTICE.md").read_bytes()
        self.assertEqual(
            PREVIEW_SELECTOR.PRODUCTION_PREVIEW_NOTICE_SHA256,
            hashlib.sha256(checked_notice).hexdigest(),
        )
        self.assertEqual(checked_notice, PREVIEW_SELECTOR.notice_bytes(manifest))

        with mock.patch.object(
            PREVIEW_SELECTOR,
            "NOTICE_TEMPLATE",
            PREVIEW_SELECTOR.NOTICE_TEMPLATE + "\nnew legal claim\n",
        ):
            with self.assertRaisesRegex(
                PREVIEW_SELECTOR.PreviewSelectionError,
                "not the exact owner-reviewed notice",
            ):
                PREVIEW_SELECTOR.notice_bytes(manifest)

    def test_output_path_guards_run_without_the_image_codec(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            canvases = base / "canvases"
            canvases.mkdir()
            linked_canvases = base / "linked-canvases"
            linked_canvases.symlink_to(canvases, target_is_directory=True)
            linked_manifest = base / "linked-manifest.json"
            linked_manifest.symlink_to(PRODUCTION_REEL_MANIFEST)
            existing = base / "existing"
            existing.mkdir()
            cases = (
                (
                    "relative canvas",
                    ("canvases", str(PRODUCTION_REEL_MANIFEST), str(base / "out-a")),
                    "absolute path",
                ),
                (
                    "linked canvas",
                    (
                        str(linked_canvases),
                        str(PRODUCTION_REEL_MANIFEST),
                        str(base / "out-b"),
                    ),
                    "symlink path components",
                ),
                (
                    "linked manifest",
                    (str(canvases), str(linked_manifest), str(base / "out-c")),
                    "symlink path components",
                ),
                (
                    "overlapping output",
                    (
                        str(canvases),
                        str(PRODUCTION_REEL_MANIFEST),
                        str(canvases / "output"),
                    ),
                    "must not overlap --canvas-root",
                ),
                (
                    "existing output",
                    (str(canvases), str(PRODUCTION_REEL_MANIFEST), str(existing)),
                    "already exists and will not be overwritten",
                ),
                (
                    "Git output",
                    (
                        str(canvases),
                        str(PRODUCTION_REEL_MANIFEST),
                        str(ROOT / ".persona-preview-test-output"),
                    ),
                    "outside a Git worktree",
                ),
            )
            for name, arguments, message in cases:
                with self.subTest(name=name):
                    with self.assertRaisesRegex(
                        (
                            PREVIEW_SELECTOR.PreviewSelectionError,
                            PREVIEW_SELECTOR.REEL_SELECTOR.SelectionError,
                        ),
                        message,
                    ):
                        PREVIEW_SELECTOR.validate_cli_paths(*arguments)

    def test_checked_files_publish_exactly_once_with_manifest_last(self) -> None:
        preview_root = ROOT / "apps" / "desktop" / "ui" / "personas"
        manifest = json.loads((preview_root / "manifest.json").read_bytes())
        files = [
            (asset["file"], (preview_root / asset["file"]).read_bytes())
            for asset in manifest["assets"]
        ]
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "published"
            real_replace = os.replace
            with mock.patch.object(
                PREVIEW_SELECTOR.os, "replace", wraps=real_replace
            ) as replace:
                PREVIEW_SELECTOR.write_selection(output, manifest, files)
            destinations = [Path(call.args[1]).name for call in replace.call_args_list]
            self.assertEqual("manifest.json", destinations[-1])
            self.assertEqual(
                {"manifest.json", "NOTICE.md", *(asset["file"] for asset in manifest["assets"])},
                {path.name for path in output.iterdir() if path.is_file()},
            )
            self.assertEqual(
                (preview_root / "manifest.json").read_bytes(),
                (output / "manifest.json").read_bytes(),
            )
            self.assertEqual(
                (preview_root / "NOTICE.md").read_bytes(),
                (output / "NOTICE.md").read_bytes(),
            )

            with self.assertRaises(FileExistsError):
                PREVIEW_SELECTOR.write_selection(output, manifest, files)
            self.assertEqual(
                (preview_root / "manifest.json").read_bytes(),
                (output / "manifest.json").read_bytes(),
            )

            changed = list(files)
            changed[0] = (changed[0][0], changed[0][1] + b"changed")
            rejected_output = Path(temporary) / "rejected"
            with self.assertRaisesRegex(
                PREVIEW_SELECTOR.PreviewSelectionError, "size does not match"
            ):
                PREVIEW_SELECTOR.write_selection(rejected_output, manifest, changed)
            self.assertFalse(rejected_output.exists())


@unittest.skipUnless(
    PINNED_CODEC,
    "preview encoding requires the pinned Pillow 12.1.1/libwebp 1.5.0 toolchain",
)
class PersonaPreviewEncodingTests(unittest.TestCase):
    """Codec-specific checks; the authority tests above never skip."""

    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.base = Path(self.temporary.name)
        self.canvases = self.base / "canvases"
        self.canvases.mkdir()
        self.sources = self.build_canvas_fixture()

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def build_canvas_fixture(self):
        sources = []
        for index, (persona, _group) in enumerate(PERSONAS):
            source_dir = self.canvases / f"doll_{persona}__workplace"
            source_dir.mkdir()
            source = source_dir / "src_img.png"
            image = Image.new("RGBA", (1280, 1280), (0, 0, 0, 0))
            draw = ImageDraw.Draw(image)
            inset = 180 + index
            draw.rounded_rectangle(
                (inset, 0, 1279 - inset, 1279),
                radius=80,
                fill=((31 * index) % 256, 90, 180, 255),
            )
            image.save(source, "PNG")
            sources.append(
                PREVIEW_SELECTOR.PreviewSource(
                    persona, hashlib.sha256(source.read_bytes()).hexdigest()
                )
            )
        return tuple(sources)

    def test_fixed_encoder_is_deterministic_and_preserves_alpha(self) -> None:
        first_assets = []
        first_files = []
        second_files = []
        for source in self.sources:
            asset, encoded = PREVIEW_SELECTOR.encode_preview(
                self.canvases, source, Image
            )
            _second_asset, encoded_again = PREVIEW_SELECTOR.encode_preview(
                self.canvases, source, Image
            )
            first_assets.append(asset)
            first_files.append(encoded)
            second_files.append(encoded_again)

        self.assertEqual(first_files, second_files)
        self.assertEqual(
            [persona for persona, _group in PERSONAS],
            [asset["id"] for asset in first_assets],
        )
        for source, asset, encoded in zip(
            self.sources, first_assets, first_files, strict=True
        ):
            self.assertEqual(source.canvas_sha256, asset["sourceCanvasSha256"])
            self.assertEqual(len(encoded), asset["bytes"])
            self.assertEqual(hashlib.sha256(encoded).hexdigest(), asset["sha256"])
            with Image.open(BytesIO(encoded)) as image:
                image.load()
                self.assertEqual("WEBP", image.format)
                self.assertEqual("RGBA", image.mode)
                self.assertEqual((640, 640), image.size)
                self.assertEqual((0, 255), image.getchannel("A").getextrema())

    def test_encoder_rejects_changed_symlinked_or_non_rgba_source(self) -> None:
        first = self.sources[0]
        source = self.canvases / f"doll_{first.persona}__workplace" / "src_img.png"

        Image.new("RGBA", (1280, 1280), (255, 0, 0, 255)).save(source, "PNG")
        with self.assertRaisesRegex(
            PREVIEW_SELECTOR.PreviewSelectionError,
            "does not match its reel manifest SHA-256",
        ):
            PREVIEW_SELECTOR.encode_preview(self.canvases, first, Image)

        source.unlink()
        real_source = source.with_name("real.png")
        Image.new("RGBA", (1280, 1280), (0, 0, 0, 0)).save(real_source, "PNG")
        source.symlink_to(real_source)
        linked = PREVIEW_SELECTOR.PreviewSource(
            first.persona, hashlib.sha256(real_source.read_bytes()).hexdigest()
        )
        with self.assertRaisesRegex(
            PREVIEW_SELECTOR.REEL_SELECTOR.SelectionError,
            "expected a regular file, not a symlink",
        ):
            PREVIEW_SELECTOR.encode_preview(self.canvases, linked, Image)

        source.unlink()
        Image.new("RGB", (1280, 1280), (0, 0, 0)).save(source, "PNG")
        rgb = PREVIEW_SELECTOR.PreviewSource(
            first.persona, hashlib.sha256(source.read_bytes()).hexdigest()
        )
        with self.assertRaisesRegex(
            PREVIEW_SELECTOR.REEL_SELECTOR.SelectionError, "8-bit RGBA PNG"
        ):
            PREVIEW_SELECTOR.encode_preview(self.canvases, rgb, Image)


if __name__ == "__main__":
    unittest.main()
