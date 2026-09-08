#!/usr/bin/env python3
"""Build the fixed 17 transparent full-body WebP Persona previews.

The private See-Through canvas tree is an explicit input and is never copied or
named in the output.  A dry run is the default; ``--write`` is required before
the new external output directory is created.
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass
from io import BytesIO
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import sys
import tempfile
from typing import NoReturn

sys.dont_write_bytecode = True


ROOT = Path(__file__).resolve().parents[1]
REEL_SELECTOR_PATH = ROOT / "scripts" / "select-persona-reels.py"
SCHEMA = "ai-sister/bundled-personas/v2"
REEL_SCHEMA = "ai-sister/persona-reels/v1"
REEL_ROSTER = "four-sisters-plus-thirteen-besties"
THEME = "workplace"
PRODUCTION_REEL_MANIFEST_SHA256 = (
    "318924b3dd6fb575fdf36d91572b5d7b93ae9c8d6e74f366e93bb1b920b550ce"
)
PRODUCTION_REEL_RIGHTS_REVIEW = "approved-owner-grant"
PRODUCTION_REEL_OWNER_GRANT = {
    "grantor": "Ted Huang",
    "granted_on": "2026-09-08",
    "scope": (
        "Unmodified inclusion of the fixed 17 workplace rigs selected and "
        "hash-listed by this manifest in the AI-Sister source tree and official "
        "builds, including local layered rendering and animation"
    ),
    "license": "excluded-from-Apache-2.0",
}
PRODUCTION_PREVIEW_MANIFEST_SHA256 = (
    "cf8e6e1b22f90f09ba021c092c3e0e9f5ae0dd39cf5644ffdfeb457ff3dd69c0"
)
PRODUCTION_PREVIEW_NOTICE_SHA256 = (
    "981557ff2db030abf75a644fd6fea2a50e69e7aedb27197406fbc324e05712fc"
)
PRODUCTION_PREVIEW_RIGHTS_REVIEW = "approved-owner-grant"
PRODUCTION_PREVIEW_OWNER_GRANT = {
    "grantor": "Ted Huang",
    "grantedOn": "2026-09-08",
    "scope": (
        "Inclusion of the exact 17 derived 640x640 full-body WebP previews "
        "source-bound and hash-listed by this manifest in the AI-Sister source "
        "tree and official builds, including local fallback and static preview display"
    ),
    "license": "excluded-from-Apache-2.0",
}
PILLOW_VERSION = "12.1.1"
LIBWEBP_VERSION = "1.5.0"
SOURCE_SIDE = 1280
OUTPUT_SIDE = 640
QUALITY = 90
ALPHA_QUALITY = 100
METHOD = 6
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
NOTICE_TEMPLATE = """# AI-Sister full-body Persona previews

這個目錄的 17 張 WebP 是從 AI-Sister 已選定、逐檔固定 SHA-256 的 workplace
1280×1280 透明 canvas 縮成 640×640，再以固定 WebP 參數產生的全身 preview。

素材所有人 Ted Huang 於 2026-09-08 明確允許把這份 manifest 逐檔固定來源
canvas SHA-256、輸出 bytes 與 SHA-256 的 exact 17 張衍生 preview 收進 AI-Sister
source tree 與官方安裝包，供本機 fallback 與靜態 preview 顯示。這些圖像不包含在
本專案的 Apache-2.0 程式碼授權裡；其他 theme、reaction、raw source 與未列出的
衍生圖都不在這份授權內。

這份授權只對下面這份 exact manifest 生效；`manifest.json` 完整 bytes 的 SHA-256 是：
`{manifest_sha256}`。

角色名稱與造型是受同名 AI 產品啟發的虛構角色。AI-Sister 是獨立產品，
未與相關供應商合作、隸屬或獲其背書。
"""


class PreviewSelectionError(RuntimeError):
    """An input failed the fixed preview selection contract."""


@dataclass(frozen=True)
class PreviewSource:
    """One canvas admitted by the reviewed production Reel authority."""

    persona: str
    canvas_sha256: str


@dataclass(frozen=True)
class ReviewedReelManifest:
    """Capability returned only after the exact production manifest is checked."""

    sha256: str
    sources: tuple[PreviewSource, ...]


def fail(message: str) -> NoReturn:
    raise PreviewSelectionError(message)


def load_reel_selector():
    spec = importlib.util.spec_from_file_location(
        "ai_sister_reel_selector_for_previews", REEL_SELECTOR_PATH
    )
    if spec is None or spec.loader is None:
        fail("cannot load the Persona reel selector")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


REEL_SELECTOR = load_reel_selector()


def read_regular_bytes(path: Path, *, maximum: int, what: str) -> bytes:
    handle, details = REEL_SELECTOR._open_regular_nofollow(path)
    with handle:
        if details.st_size <= 0 or details.st_size > maximum:
            fail(f"{what}: size {details.st_size} is outside the accepted range")
        raw = REEL_SELECTOR._read_exact(handle, details.st_size, what=what)
        if handle.read(1):
            fail(f"{what}: file changed while it was read")
    return raw


def read_reel_manifest(path: Path) -> tuple[dict[str, object], bytes]:
    raw = read_regular_bytes(
        path,
        maximum=REEL_SELECTOR.MAX_JSON_BYTES,
        what=str(path),
    )
    try:
        value = json.loads(
            raw.decode("utf-8"),
            object_pairs_hook=REEL_SELECTOR._reject_duplicate_keys,
            parse_constant=lambda token: fail(
                f"{path}: invalid JSON number {token}"
            ),
        )
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        fail(f"{path}: invalid UTF-8 JSON ({error})")
    if not isinstance(value, dict):
        fail(f"{path}: expected a JSON object")
    return value, raw


def validate_reel_manifest(
    value: dict[str, object], raw: bytes
) -> ReviewedReelManifest:
    if value.get("rights_review") != PRODUCTION_REEL_RIGHTS_REVIEW:
        fail("--reel-manifest: rights_review does not match the reviewed production grant")
    if value.get("owner_grant") != PRODUCTION_REEL_OWNER_GRANT:
        fail("--reel-manifest: owner_grant does not match the reviewed production grant")

    digest = hashlib.sha256(raw).hexdigest()
    if digest != PRODUCTION_REEL_MANIFEST_SHA256:
        fail(
            "--reel-manifest: bytes are not the exact reviewed production manifest; "
            f"expected SHA-256 {PRODUCTION_REEL_MANIFEST_SHA256}, got {digest}"
        )
    if value.get("schema") != REEL_SCHEMA:
        fail(f"--reel-manifest: expected schema {REEL_SCHEMA!r}")
    selection = value.get("selection")
    if not isinstance(selection, dict):
        fail("--reel-manifest: selection is missing")
    expected_personas = [
        {"id": persona, "group": group} for persona, group in PERSONAS
    ]
    if selection.get("roster") != REEL_ROSTER:
        fail("--reel-manifest: fixed roster changed")
    if selection.get("theme") != THEME:
        fail("--reel-manifest: fixed workplace theme changed")
    if selection.get("personas") != expected_personas:
        fail("--reel-manifest: fixed 17-person selection or order changed")

    rigs = value.get("rigs")
    if not isinstance(rigs, list) or len(rigs) != len(PERSONAS):
        fail("--reel-manifest: expected exactly 17 rigs")
    if [rig.get("id") if isinstance(rig, dict) else None for rig in rigs] != [
        persona for persona, _group in PERSONAS
    ]:
        fail("--reel-manifest: rig roster or order changed")

    checked: list[PreviewSource] = []
    for (persona, group), rig in zip(PERSONAS, rigs, strict=True):
        if not isinstance(rig, dict):
            fail(f"--reel-manifest: {persona} rig is not an object")
        if rig.get("group") != group or rig.get("theme") != THEME:
            fail(f"--reel-manifest: {persona} identity or theme changed")
        if rig.get("canvas") != {"width": SOURCE_SIDE, "height": SOURCE_SIDE}:
            fail(f"--reel-manifest: {persona} canvas is not 1280x1280")
        canvas_sha256 = REEL_SELECTOR.require_sha256(
            rig.get("canvas_sha256"), what=f"{persona} canvas"
        )
        checked.append(PreviewSource(persona, canvas_sha256))
    return ReviewedReelManifest(digest, tuple(checked))


def validate_cli_paths(
    canvas_raw: str, reel_manifest_raw: str, output_raw: str
) -> tuple[Path, Path, Path]:
    canvas_root = REEL_SELECTOR.absolute_cli_path(
        canvas_raw, name="--canvas-root", must_exist=True
    )
    REEL_SELECTOR.require_real_directory(canvas_root)
    reel_manifest = REEL_SELECTOR.absolute_cli_path(
        reel_manifest_raw, name="--reel-manifest", must_exist=True
    )
    output = REEL_SELECTOR.absolute_cli_path(
        output_raw, name="--output", must_exist=False
    )
    if os.path.lexists(output):
        fail(f"--output already exists and will not be overwritten: {output}")
    if REEL_SELECTOR.paths_overlap(canvas_root, output):
        fail("--output must not overlap --canvas-root")
    git_worktree = REEL_SELECTOR.enclosing_git_worktree(output.parent)
    if git_worktree is not None:
        fail(f"--output must be outside a Git worktree: {git_worktree}")
    return canvas_root, reel_manifest, output


def load_pillow():
    try:
        import PIL
        from PIL import Image, features
    except ImportError as error:
        fail(f"Pillow {PILLOW_VERSION} is required ({error})")
    webp_version = features.version("webp")
    if PIL.__version__ != PILLOW_VERSION or webp_version != LIBWEBP_VERSION:
        fail(
            "reproducible encoding requires "
            f"Pillow {PILLOW_VERSION} with libwebp {LIBWEBP_VERSION}; found "
            f"Pillow {PIL.__version__} with libwebp {webp_version or 'unavailable'}"
        )
    if not features.check("webp"):
        fail("the pinned Pillow build has no WebP encoder")
    return Image


def encode_preview(
    canvas_root: Path,
    source_row: PreviewSource,
    Image,
) -> tuple[dict[str, object], bytes]:
    persona = source_row.persona
    source_dir = canvas_root / f"doll_{persona}__{THEME}"
    REEL_SELECTOR.reject_symlink_chain(source_dir)
    REEL_SELECTOR.require_real_directory(source_dir)
    source = source_dir / "src_img.png"
    expected_sha256 = source_row.canvas_sha256
    info = REEL_SELECTOR.inspect_png(source)
    if (info["width"], info["height"]) != (SOURCE_SIDE, SOURCE_SIDE):
        fail(f"{persona}: source canvas must be exactly 1280x1280")
    if info["sha256"] != expected_sha256:
        fail(f"{persona}: source canvas does not match its reel manifest SHA-256")

    raw = read_regular_bytes(
        source,
        maximum=REEL_SELECTOR.MAX_PNG_BYTES,
        what=str(source),
    )
    if hashlib.sha256(raw).hexdigest() != expected_sha256:
        fail(f"{persona}: source canvas changed before decode")
    try:
        with Image.open(BytesIO(raw)) as opened:
            if opened.format != "PNG" or opened.mode != "RGBA":
                fail(f"{persona}: source must decode as RGBA PNG")
            opened.load()
            source_image = opened.copy()
    except PreviewSelectionError:
        raise
    except Exception as error:  # Pillow gives format-specific exception classes.
        fail(f"{persona}: cannot decode source PNG ({error})")

    resized = source_image.resize(
        (OUTPUT_SIDE, OUTPUT_SIDE),
        Image.Resampling.LANCZOS,
    )
    source_alpha = resized.getchannel("A").tobytes()
    buffer = BytesIO()
    try:
        resized.save(
            buffer,
            "WEBP",
            quality=QUALITY,
            alpha_quality=ALPHA_QUALITY,
            method=METHOD,
            exact=True,
        )
    except Exception as error:
        fail(f"{persona}: WebP encode failed ({error})")
    encoded = buffer.getvalue()
    if not encoded:
        fail(f"{persona}: WebP encoder returned no bytes")

    try:
        with Image.open(BytesIO(encoded)) as decoded:
            if (
                decoded.format != "WEBP"
                or decoded.mode != "RGBA"
                or decoded.size != (OUTPUT_SIDE, OUTPUT_SIDE)
            ):
                fail(f"{persona}: encoded preview lost its 640x640 RGBA contract")
            decoded.load()
            alpha = decoded.getchannel("A")
            alpha_extrema = alpha.getextrema()
            if alpha_extrema != (0, 255):
                fail(
                    f"{persona}: encoded preview must contain transparent and opaque pixels"
                )
            if alpha.tobytes() != source_alpha:
                fail(f"{persona}: WebP encoding changed the resized alpha channel")
    except PreviewSelectionError:
        raise
    except Exception as error:
        fail(f"{persona}: cannot verify encoded WebP ({error})")

    filename = f"{persona}.webp"
    return (
        {
            "id": persona,
            "file": filename,
            "sourceCanvasSha256": expected_sha256,
            "bytes": len(encoded),
            "sha256": hashlib.sha256(encoded).hexdigest(),
        },
        encoded,
    )


def build_selection(
    canvas_root: Path, reel_manifest: Path
) -> tuple[dict[str, object], list[tuple[str, bytes]]]:
    reel, reel_raw = read_reel_manifest(reel_manifest)
    reviewed_reel = validate_reel_manifest(reel, reel_raw)
    Image = load_pillow()
    assets: list[dict[str, object]] = []
    files: list[tuple[str, bytes]] = []
    for row in reviewed_reel.sources:
        asset, encoded = encode_preview(canvas_root, row, Image)
        assets.append(asset)
        files.append((str(asset["file"]), encoded))

    manifest: dict[str, object] = {
        "schema": SCHEMA,
        "sourceReelManifestSha256": reviewed_reel.sha256,
        "previewContract": {
            "subject": "full-body",
            "canvas": {"width": OUTPUT_SIDE, "height": OUTPUT_SIDE},
            "alpha": "transparent",
            "fit": "contain",
        },
        "encoding": {
            "format": "webp",
            "lossless": False,
            "quality": QUALITY,
            "alphaQuality": ALPHA_QUALITY,
            "method": METHOD,
            "exactTransparentRgb": True,
            "resize": "Pillow.Resampling.LANCZOS",
            "pillow": PILLOW_VERSION,
            "libwebp": LIBWEBP_VERSION,
        },
        "rightsReview": PRODUCTION_PREVIEW_RIGHTS_REVIEW,
        "ownerGrant": dict(PRODUCTION_PREVIEW_OWNER_GRANT),
        "notice": "NOTICE.md",
        "assets": assets,
        "totals": {
            "assets": len(assets),
            "webpBytes": sum(int(asset["bytes"]) for asset in assets),
        },
    }
    REEL_SELECTOR._validate_sanitized_manifest(manifest)
    validate_preview_manifest_authority(manifest)
    notice_bytes(manifest)
    return manifest, files


def manifest_bytes(manifest: dict[str, object]) -> bytes:
    return (
        json.dumps(manifest, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    ).encode("utf-8")


def validate_preview_manifest_authority(manifest: dict[str, object]) -> None:
    """Refuse to stamp a grant onto bytes Ted has not reviewed."""

    if manifest.get("rightsReview") != PRODUCTION_PREVIEW_RIGHTS_REVIEW:
        fail("generated preview rightsReview does not match the reviewed owner grant")
    if manifest.get("ownerGrant") != PRODUCTION_PREVIEW_OWNER_GRANT:
        fail("generated preview ownerGrant does not match the reviewed owner grant")
    digest = hashlib.sha256(manifest_bytes(manifest)).hexdigest()
    if digest != PRODUCTION_PREVIEW_MANIFEST_SHA256:
        fail(
            "generated preview bytes are not the exact owner-reviewed output; "
            f"expected manifest SHA-256 {PRODUCTION_PREVIEW_MANIFEST_SHA256}, got {digest}; "
            "a new owner review is required"
        )


def notice_bytes(manifest: dict[str, object]) -> bytes:
    digest = hashlib.sha256(manifest_bytes(manifest)).hexdigest()
    raw = NOTICE_TEMPLATE.format(manifest_sha256=digest).encode("utf-8")
    notice_digest = hashlib.sha256(raw).hexdigest()
    if notice_digest != PRODUCTION_PREVIEW_NOTICE_SHA256:
        fail(
            "generated NOTICE bytes are not the exact owner-reviewed notice; "
            f"expected SHA-256 {PRODUCTION_PREVIEW_NOTICE_SHA256}, got {notice_digest}; "
            "a new owner review is required"
        )
    return raw


def validate_selection_files(
    manifest: dict[str, object], files: list[tuple[str, bytes]]
) -> None:
    """Bind staged WebP bytes back to every exact manifest entry."""

    assets = manifest.get("assets")
    if not isinstance(assets, list) or len(assets) != len(files):
        fail("generated WebP file count does not match the reviewed manifest")
    for asset, output in zip(assets, files, strict=True):
        if not isinstance(asset, dict):
            fail("generated preview asset is not an object")
        filename, encoded = output
        if filename != asset.get("file"):
            fail("generated WebP filename or order does not match the reviewed manifest")
        if not isinstance(encoded, bytes):
            fail(f"generated {filename} is not bytes")
        if len(encoded) != asset.get("bytes"):
            fail(f"generated {filename} size does not match the reviewed manifest")
        if hashlib.sha256(encoded).hexdigest() != asset.get("sha256"):
            fail(f"generated {filename} SHA-256 does not match the reviewed manifest")


def write_new_file(path: Path, data: bytes) -> None:
    with path.open("xb") as handle:
        handle.write(data)
        handle.flush()
        os.fsync(handle.fileno())
    os.chmod(path, 0o644)


def write_selection(
    output: Path,
    manifest: dict[str, object],
    files: list[tuple[str, bytes]],
) -> None:
    """Stage every byte, reserve a new output, and publish manifest.json last."""

    validate_preview_manifest_authority(manifest)
    validate_selection_files(manifest, files)
    reviewed_notice = notice_bytes(manifest)

    staging = Path(tempfile.mkdtemp(prefix=f".{output.name}.staging-", dir=output.parent))
    output_reserved = False
    try:
        for filename, encoded in files:
            write_new_file(staging / filename, encoded)
        write_new_file(staging / "NOTICE.md", reviewed_notice)
        write_new_file(staging / "manifest.json", manifest_bytes(manifest))

        output.mkdir(mode=0o700)
        output_reserved = True
        for filename, _encoded in files:
            os.replace(staging / filename, output / filename)
        os.replace(staging / "NOTICE.md", output / "NOTICE.md")
        os.replace(staging / "manifest.json", output / "manifest.json")
        staging.rmdir()
        os.chmod(output, 0o755)
    except BaseException:
        if output_reserved:
            shutil.rmtree(output)
        shutil.rmtree(staging, ignore_errors=True)
        raise


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "Validate and derive AI-Sister's fixed 17 transparent full-body previews "
            "(dry-run by default)."
        )
    )
    parser.add_argument(
        "--canvas-root",
        required=True,
        help="absolute parent containing doll_<id>__workplace/src_img.png canvases",
    )
    parser.add_argument(
        "--reel-manifest",
        required=True,
        help="absolute path to the reviewed production Persona reel manifest",
    )
    parser.add_argument(
        "--output",
        required=True,
        help="absolute, new output directory outside every Git worktree",
    )
    parser.add_argument(
        "--write",
        action="store_true",
        help="create the output directory; without this flag only print the manifest",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    arguments = parse_args(sys.argv[1:] if argv is None else argv)
    try:
        canvas_root, reel_manifest, output = validate_cli_paths(
            arguments.canvas_root,
            arguments.reel_manifest,
            arguments.output,
        )
        manifest, files = build_selection(canvas_root, reel_manifest)
        if arguments.write:
            write_selection(output, manifest, files)
        sys.stdout.buffer.write(manifest_bytes(manifest))
        return 0
    except (PreviewSelectionError, REEL_SELECTOR.SelectionError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
