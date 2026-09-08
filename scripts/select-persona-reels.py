#!/usr/bin/env python3
"""Select the fixed 17-person workplace rigs from a local tachie tree.

The command is intentionally a selector, not a general-purpose asset copier.  It
does not accept persona names, themes, URLs, or a source manifest path.  Dry-run
is the default; ``--write`` is required before any output directory is created.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import stat
import struct
import sys
import tempfile
from typing import BinaryIO, NoReturn
import zlib


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
THEME = "workplace"
RIG_SUFFIX = "__workplace__v2_parts"
CANVAS_CONTRACT = "see_through_center_pad_v1"
RESIDUAL_CONTRACT = "source_canvas_residual_v1"
IDENTITY_CONTRACT = "source_canvas_identity_v1"
SOURCE_TOOL = "decompose_persona_rigs.py"
MAX_JSON_BYTES = 2 * 1024 * 1024
MAX_PNG_BYTES = 64 * 1024 * 1024
MAX_PNG_SIDE = 8192
PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"
HEX_SHA256 = re.compile(r"[0-9a-f]{64}\Z")
SAFE_TAG = re.compile(r"[a-z][a-z0-9_]*\Z")
WINDOWS_DRIVE = re.compile(r"[A-Za-z]:[\\/]")
RUNTIME_GLOBAL = "__AI_SISTER_PERSONA_REELS__"
VIEWPORT = {"x": 320, "y": 0, "width": 640, "height": 640}
NOTICE_TEMPLATE = """# AI-Sister workplace Persona rigs

這個目錄的 17 套分層 PNG rig 是素材所有人 Ted Huang 於 2026-09-08
明確提供給 AI-Sister 使用的角色素材。授權範圍是把
`manifest.json` 列出的 fixed 17-person workplace rigs 原檔收進 AI-Sister
source tree 與官方安裝包，並在官方 build 裡做本機分層呈現與動畫。
這些圖像不包含在本專案的 Apache-2.0 程式碼授權裡。

`manifest.json` 逐檔固定大小、SHA-256、圖層位置與尺寸；`manifest.js`
是相同清單的本機 runtime projection。raw `parts.json`、`source.json`、私有絕對
path、其他服裝、reaction 與 debug 圖都不在此授權與安裝包內。

這份授權只對下面這份 exact manifest 生效；`manifest.json` 完整 bytes 的 SHA-256 是：
`{manifest_sha256}`。

角色名稱與造型是受同名 AI 產品啟發的虛構角色。AI-Sister 是獨立產品，
未與相關供應商合作、隸屬或獲其背書。
"""


class SelectionError(RuntimeError):
    """An input failed the fixed selection contract."""


def fail(message: str) -> NoReturn:
    raise SelectionError(message)


def _read_exact(handle: BinaryIO, length: int, *, what: str) -> bytes:
    value = handle.read(length)
    if len(value) != length:
        fail(f"{what}: unexpected end of file")
    return value


def _open_regular_nofollow(path: Path) -> tuple[BinaryIO, os.stat_result]:
    try:
        before_open = os.lstat(path)
    except OSError as error:
        fail(f"{path}: cannot inspect regular file ({error})")
    if stat.S_ISLNK(before_open.st_mode) or not stat.S_ISREG(before_open.st_mode):
        fail(f"{path}: expected a regular file, not a symlink")
    flags = os.O_RDONLY | getattr(os, "O_BINARY", 0) | getattr(os, "O_NOFOLLOW", 0)
    try:
        descriptor = os.open(path, flags)
    except OSError as error:
        fail(f"{path}: cannot open regular file ({error})")
    try:
        details = os.fstat(descriptor)
        if not stat.S_ISREG(details.st_mode):
            fail(f"{path}: expected a regular file")
        if (details.st_dev, details.st_ino) != (before_open.st_dev, before_open.st_ino):
            fail(f"{path}: file changed while it was opened")
        return os.fdopen(descriptor, "rb"), details
    except BaseException:
        os.close(descriptor)
        raise


def _reject_duplicate_keys(pairs: list[tuple[str, object]]) -> dict[str, object]:
    result: dict[str, object] = {}
    for key, value in pairs:
        if key in result:
            fail(f"JSON contains duplicate key {key!r}")
        result[key] = value
    return result


def read_json_object(path: Path) -> dict[str, object]:
    handle, details = _open_regular_nofollow(path)
    with handle:
        if details.st_size <= 0 or details.st_size > MAX_JSON_BYTES:
            fail(f"{path}: JSON size {details.st_size} is outside the accepted range")
        raw = _read_exact(handle, details.st_size, what=str(path))
        if handle.read(1):
            fail(f"{path}: file changed while it was read")
    try:
        value = json.loads(
            raw.decode("utf-8"),
            object_pairs_hook=_reject_duplicate_keys,
            parse_constant=lambda token: fail(f"{path}: invalid JSON number {token}"),
        )
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        fail(f"{path}: invalid UTF-8 JSON ({error})")
    if not isinstance(value, dict):
        fail(f"{path}: expected a JSON object")
    return value


def inspect_png(path: Path) -> dict[str, int | str]:
    """Validate one bounded, non-symlink, 8-bit RGBA PNG and hash every byte."""

    handle, details = _open_regular_nofollow(path)
    with handle:
        if details.st_size <= len(PNG_SIGNATURE) or details.st_size > MAX_PNG_BYTES:
            fail(f"{path}: PNG size {details.st_size} is outside the accepted range")
        digest = hashlib.sha256()
        signature = _read_exact(handle, len(PNG_SIGNATURE), what=str(path))
        digest.update(signature)
        if signature != PNG_SIGNATURE:
            fail(f"{path}: invalid PNG signature")

        width = height = bit_depth = color_type = None
        saw_ihdr = saw_idat = saw_iend = False
        chunk_number = 0
        while not saw_iend:
            length_raw = _read_exact(handle, 4, what=str(path))
            chunk_type = _read_exact(handle, 4, what=str(path))
            digest.update(length_raw)
            digest.update(chunk_type)
            length = struct.unpack(">I", length_raw)[0]
            if not all(65 <= byte <= 90 or 97 <= byte <= 122 for byte in chunk_type):
                fail(f"{path}: invalid PNG chunk type")
            if length > details.st_size:
                fail(f"{path}: impossible PNG chunk length {length}")

            crc = zlib.crc32(chunk_type)
            payload = bytearray() if chunk_type == b"IHDR" else None
            remaining = length
            while remaining:
                block = _read_exact(handle, min(1024 * 1024, remaining), what=str(path))
                digest.update(block)
                crc = zlib.crc32(block, crc)
                if payload is not None:
                    payload.extend(block)
                remaining -= len(block)
            declared_crc_raw = _read_exact(handle, 4, what=str(path))
            digest.update(declared_crc_raw)
            if struct.unpack(">I", declared_crc_raw)[0] != crc & 0xFFFFFFFF:
                fail(f"{path}: bad CRC in {chunk_type.decode('ascii')} chunk")

            if chunk_number == 0 and chunk_type != b"IHDR":
                fail(f"{path}: IHDR is not the first PNG chunk")
            if chunk_type == b"IHDR":
                if saw_ihdr or length != 13 or payload is None:
                    fail(f"{path}: invalid IHDR chunk")
                width, height, bit_depth, color_type, compression, filtering, interlace = (
                    struct.unpack(">IIBBBBB", payload)
                )
                if not (1 <= width <= MAX_PNG_SIDE and 1 <= height <= MAX_PNG_SIDE):
                    fail(f"{path}: PNG dimensions {width}x{height} are outside the accepted range")
                if (bit_depth, color_type, compression, filtering, interlace) != (8, 6, 0, 0, 0):
                    fail(f"{path}: expected non-interlaced 8-bit RGBA PNG")
                saw_ihdr = True
            elif chunk_type == b"IDAT":
                if not saw_ihdr:
                    fail(f"{path}: IDAT appears before IHDR")
                saw_idat = True
            elif chunk_type == b"IEND":
                if length != 0 or not saw_idat:
                    fail(f"{path}: invalid IEND chunk")
                saw_iend = True
            chunk_number += 1

        if handle.read(1):
            fail(f"{path}: trailing bytes after PNG IEND")
        if handle.tell() != details.st_size:
            fail(f"{path}: file size changed while it was read")
        if None in (width, height, bit_depth, color_type):
            fail(f"{path}: PNG has no usable IHDR")
        return {
            "bytes": details.st_size,
            "sha256": digest.hexdigest(),
            "width": int(width),
            "height": int(height),
        }


def require_int(container: dict[str, object], key: str, *, what: str) -> int:
    value = container.get(key)
    if type(value) is not int:
        fail(f"{what}: {key} must be an integer")
    return value


def require_sha256(value: object, *, what: str) -> str:
    if not isinstance(value, str) or HEX_SHA256.fullmatch(value) is None:
        fail(f"{what}: expected a lowercase SHA-256")
    return value


def require_real_directory(path: Path) -> None:
    try:
        details = os.lstat(path)
    except OSError as error:
        fail(f"{path}: required directory is unavailable ({error})")
    if stat.S_ISLNK(details.st_mode) or not stat.S_ISDIR(details.st_mode):
        fail(f"{path}: expected a real directory, not a symlink")


def reject_symlink_chain(path: Path) -> None:
    current = Path(path.anchor)
    for part in path.parts[1:]:
        current /= part
        try:
            details = os.lstat(current)
        except OSError as error:
            fail(f"{current}: path component is unavailable ({error})")
        if stat.S_ISLNK(details.st_mode):
            fail(f"{current}: symlink path components are not accepted")


def absolute_cli_path(raw: str, *, name: str, must_exist: bool) -> Path:
    if "://" in raw or "\x00" in raw:
        fail(f"{name}: expected a local filesystem path")
    path = Path(raw)
    if not path.is_absolute() or ".." in path.parts:
        fail(f"{name}: expected an absolute path without '..'")
    if must_exist:
        reject_symlink_chain(path)
        return path.resolve(strict=True)
    parent = path.parent
    reject_symlink_chain(parent)
    require_real_directory(parent)
    return parent.resolve(strict=True) / path.name


def paths_overlap(left: Path, right: Path) -> bool:
    return left == right or left in right.parents or right in left.parents


def enclosing_git_worktree(path: Path) -> Path | None:
    for candidate in (path, *path.parents):
        marker = candidate / ".git"
        if marker.is_dir() and (marker / "HEAD").is_file():
            return candidate
        if marker.is_file():
            try:
                with marker.open("rb") as handle:
                    first_line = handle.read(4096).decode("utf-8", errors="strict")
            except (OSError, UnicodeError):
                continue
            if first_line.startswith("gitdir:"):
                return candidate
    return None


def validate_cli_paths(tachie_raw: str, output_raw: str) -> tuple[Path, Path]:
    tachie_root = absolute_cli_path(tachie_raw, name="--tachie-root", must_exist=True)
    require_real_directory(tachie_root)
    output = absolute_cli_path(output_raw, name="--output", must_exist=False)
    if os.path.lexists(output):
        fail(f"--output already exists and will not be overwritten: {output}")
    if paths_overlap(tachie_root, output):
        fail("--output must not overlap --tachie-root")
    git_worktree = enclosing_git_worktree(output.parent)
    if git_worktree is not None:
        fail(f"--output must be outside a Git worktree: {git_worktree}")
    return tachie_root, output


def safe_part_filename(value: object, *, what: str) -> str:
    if not isinstance(value, str) or not value or len(value) > 160:
        fail(f"{what}: part filename is invalid")
    candidate = PurePosixPath(value)
    if (
        candidate.is_absolute()
        or candidate.name != value
        or "\\" in value
        or ".." in candidate.parts
        or candidate.suffix != ".png"
        or any(ord(character) < 32 for character in value)
        or WINDOWS_DRIVE.match(value)
    ):
        fail(f"{what}: part filename must be one relative PNG basename")
    return value


def _validate_derived_binding(
    parts_manifest: dict[str, object], canvas_sha256: str, *, persona: str
) -> None:
    residual = parts_manifest.get("source_residual")
    if not isinstance(residual, dict):
        fail(f"{persona}: source_residual metadata is missing")
    if residual.get("version") != RESIDUAL_CONTRACT:
        fail(f"{persona}: unsupported source_residual contract")
    if residual.get("source_canvas_sha256") != canvas_sha256:
        fail(f"{persona}: source_residual is not bound to the selected canvas")

    overlays = parts_manifest.get("identity_overlays")
    if overlays is not None:
        if not isinstance(overlays, dict) or overlays.get("version") != IDENTITY_CONTRACT:
            fail(f"{persona}: unsupported identity overlay contract")
        if overlays.get("source_canvas_sha256") != canvas_sha256:
            fail(f"{persona}: identity overlay is not bound to the selected canvas")
        regions = overlays.get("regions")
        if not isinstance(regions, list):
            fail(f"{persona}: identity overlay regions are malformed")


def layer_role(tag: str) -> str:
    if "mouth" in tag:
        return "mouth"
    if "brow" in tag:
        return "brow"
    if (
        ("eye" in tag and "wear" not in tag)
        or "irid" in tag
        or "lash" in tag
        or "pupil" in tag
    ):
        return "eye"
    return "body"


def eye_priority(tag: str) -> int:
    if "eyewhite" in tag or ("white" in tag and "eye" in tag):
        return 0
    if "irid" in tag:
        return 1
    if "pupil" in tag:
        return 2
    if "lash" in tag:
        return 3
    if "brow" in tag:
        return 4
    return 1


def assign_render_order(layers: list[dict[str, object]]) -> None:
    """Keep body slots, but repair See-Through's occasional eye occlusion."""

    for layer in layers:
        layer["render_z"] = layer["z"]
    eye_and_brow = [layer for layer in layers if layer["role"] in ("eye", "brow")]
    slots = sorted(int(layer["z"]) for layer in eye_and_brow)
    ordered = sorted(
        eye_and_brow,
        key=lambda layer: (eye_priority(str(layer["tag"])), int(layer["z"])),
    )
    for layer, slot in zip(ordered, slots, strict=True):
        layer["render_z"] = slot


def select_rig(
    tachie_root: Path, persona: str, group: str
) -> tuple[dict[str, object], list[tuple[Path, str]]]:
    rig_dir = tachie_root / "parts" / f"{persona}{RIG_SUFFIX}"
    require_real_directory(rig_dir)
    parts_path = rig_dir / "parts.json"
    source_receipt_path = rig_dir / "source.json"
    parts_manifest = read_json_object(parts_path)
    source_receipt = read_json_object(source_receipt_path)

    if source_receipt.get("tool") != SOURCE_TOOL:
        fail(f"{persona}: unexpected decomposition receipt tool")
    if source_receipt.get("name") != persona or source_receipt.get("theme") != THEME:
        fail(f"{persona}: decomposition receipt identity does not match the fixed selection")
    if source_receipt.get("canvas_contract_version") != CANVAS_CONTRACT:
        fail(f"{persona}: unsupported canvas contract")
    source_sha256 = require_sha256(source_receipt.get("source_sha256"), what=f"{persona} source")
    canvas_sha256 = require_sha256(source_receipt.get("canvas_sha256"), what=f"{persona} canvas")

    source_path = tachie_root / "outfits_v2_norm" / f"doll_{persona}__{THEME}.png"
    source_info = inspect_png(source_path)
    if source_info["sha256"] != source_sha256:
        fail(f"{persona}: normalized source PNG does not match source.json SHA-256")

    canvas_width = require_int(parts_manifest, "canvas_w", what=persona)
    canvas_height = require_int(parts_manifest, "canvas_h", what=persona)
    if not (1 <= canvas_width <= MAX_PNG_SIDE and 1 <= canvas_height <= MAX_PNG_SIDE):
        fail(f"{persona}: invalid canvas dimensions {canvas_width}x{canvas_height}")
    if (canvas_width, canvas_height) != (1280, 1280):
        fail(f"{persona}: workplace runtime canvas must be exactly 1280x1280")
    _validate_derived_binding(parts_manifest, canvas_sha256, persona=persona)

    source_parts = parts_manifest.get("parts")
    if not isinstance(source_parts, list) or not 6 <= len(source_parts) <= 64:
        fail(f"{persona}: parts must contain 6..64 layer rows")

    layers: list[dict[str, object]] = []
    copies: list[tuple[Path, str]] = []
    tags: set[str] = set()
    filenames: set[str] = set()
    casefolded_filenames: set[str] = set()
    z_values: set[int] = set()
    previous_z = -1
    for index, source_part in enumerate(source_parts):
        what = f"{persona} part {index}"
        if not isinstance(source_part, dict):
            fail(f"{what}: expected an object")
        tag = source_part.get("tag")
        if not isinstance(tag, str) or SAFE_TAG.fullmatch(tag) is None or tag in tags:
            fail(f"{what}: tag must be unique lower snake_case")
        tags.add(tag)
        filename = safe_part_filename(source_part.get("file"), what=what)
        if filename in filenames or filename.casefold() in casefolded_filenames:
            fail(f"{what}: duplicate or case-colliding part filename")
        filenames.add(filename)
        casefolded_filenames.add(filename.casefold())

        z = require_int(source_part, "z", what=what)
        x = require_int(source_part, "x", what=what)
        y = require_int(source_part, "y", what=what)
        width = require_int(source_part, "w", what=what)
        height = require_int(source_part, "h", what=what)
        center_x = require_int(source_part, "cx", what=what)
        center_y = require_int(source_part, "cy", what=what)
        if z < 0 or z > 1024 or z in z_values or z <= previous_z:
            fail(f"{what}: z values must be unique and strictly increasing")
        z_values.add(z)
        previous_z = z
        if x < 0 or y < 0 or width <= 0 or height <= 0:
            fail(f"{what}: layer rectangle is invalid")
        if x + width > canvas_width or y + height > canvas_height:
            fail(f"{what}: layer rectangle exceeds the canvas")
        if not (x <= center_x <= x + width and y <= center_y <= y + height):
            fail(f"{what}: layer center is outside its rectangle")

        part_path = rig_dir / filename
        png_info = inspect_png(part_path)
        if (png_info["width"], png_info["height"]) != (width, height):
            fail(
                f"{what}: declared {width}x{height}, PNG is "
                f"{png_info['width']}x{png_info['height']}"
            )
        relative_file = f"rigs/{persona}/{filename}"
        layers.append(
            {
                "tag": tag,
                "role": layer_role(tag),
                "z": z,
                "x": x,
                "y": y,
                "width": width,
                "height": height,
                "center_x": center_x,
                "center_y": center_y,
                "file": relative_file,
                "bytes": png_info["bytes"],
                "sha256": png_info["sha256"],
            }
        )
        copies.append((part_path, relative_file))

    assign_render_order(layers)

    required_tags = (
        "face",
        "mouth",
        "eyebrow_l",
        "eyebrow_r",
        "eyewhite_l",
        "eyewhite_r",
        "irides_l",
        "irides_r",
        "eyelash_l",
        "eyelash_r",
        "source_residual",
    )
    for required_tag in required_tags:
        if required_tag not in tags:
            fail(f"{persona}: required layer {required_tag!r} is missing")
    identity_layer_count = sum(layer["tag"] == "identity_overlay" for layer in layers)
    identity_metadata = parts_manifest.get("identity_overlays")
    expected_identity_layers = (
        len(identity_metadata["regions"]) if isinstance(identity_metadata, dict) else 0
    )
    if identity_layer_count != expected_identity_layers:
        fail(f"{persona}: identity overlay layer count does not match its private receipt")

    return (
        {
            "id": persona,
            "group": group,
            "theme": THEME,
            "canvas": {"width": canvas_width, "height": canvas_height},
            "viewport": dict(VIEWPORT),
            "canvas_sha256": canvas_sha256,
            "source_pin": source_info,
            "layers": layers,
        },
        copies,
    )


def _validate_sanitized_manifest(value: object, *, key: str = "manifest") -> None:
    forbidden_keys = {
        "src",
        "source",
        "source_path",
        "prompt",
        "prompt_path",
        "decomposed_at",
        "identity_overlays",
        "source_residual",
    }
    if isinstance(value, dict):
        for child_key, child_value in value.items():
            if child_key in forbidden_keys:
                fail(f"sanitized manifest unexpectedly contains {child_key!r}")
            _validate_sanitized_manifest(child_value, key=child_key)
    elif isinstance(value, list):
        for child in value:
            _validate_sanitized_manifest(child, key=key)
    elif isinstance(value, str):
        if value.startswith(("/", "\\")) or WINDOWS_DRIVE.match(value) or "://" in value:
            fail(f"sanitized manifest contains an absolute or remote value in {key!r}")
        if "source.json" in value or any(part == ".." for part in PurePosixPath(value).parts):
            fail(f"sanitized manifest contains a private or traversing value in {key!r}")


def build_selection(tachie_root: Path) -> tuple[dict[str, object], list[tuple[Path, str]]]:
    require_real_directory(tachie_root / "parts")
    require_real_directory(tachie_root / "outfits_v2_norm")
    rigs: list[dict[str, object]] = []
    copies: list[tuple[Path, str]] = []
    for persona, group in PERSONAS:
        rig, rig_copies = select_rig(tachie_root, persona, group)
        rigs.append(rig)
        copies.extend(rig_copies)

    manifest: dict[str, object] = {
        "schema": "ai-sister/persona-reels/v1",
        "selection": {
            "roster": "four-sisters-plus-thirteen-besties",
            "theme": THEME,
            "personas": [{"id": persona, "group": group} for persona, group in PERSONAS],
        },
        "format": {
            "kind": "layered-png",
            "png": "rgba8-noninterlaced",
            "canvas_contract": CANVAS_CONTRACT,
        },
        "notice": "NOTICE.md",
        "rights_review": "approved-owner-grant",
        "owner_grant": {
            "grantor": "Ted Huang",
            "granted_on": "2026-09-08",
            "scope": (
                "Unmodified inclusion of the fixed 17 workplace rigs selected and "
                "hash-listed by this manifest in the AI-Sister source tree and official builds, "
                "including local layered rendering and animation"
            ),
            "license": "excluded-from-Apache-2.0",
        },
        "rigs": rigs,
        "totals": {
            "rigs": len(rigs),
            "layers": sum(len(rig["layers"]) for rig in rigs),
            "png_files": len(copies),
            "png_bytes": sum(layer["bytes"] for rig in rigs for layer in rig["layers"]),
        },
    }
    _validate_sanitized_manifest(manifest)
    return manifest, copies


def manifest_bytes(manifest: dict[str, object]) -> bytes:
    rendered = json.dumps(manifest, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    return rendered.encode("utf-8")


def runtime_manifest_bytes(manifest: dict[str, object]) -> bytes:
    """Return a local classic script so the CSP needs no JSON fetch permission."""

    compact = json.dumps(manifest, ensure_ascii=False, separators=(",", ":"), sort_keys=True)
    return (
        "// Generated by scripts/select-persona-reels.py; do not edit.\n"
        f"globalThis.{RUNTIME_GLOBAL} = {compact};\n"
    ).encode("utf-8")


def notice_bytes(manifest: dict[str, object]) -> bytes:
    manifest_sha256 = hashlib.sha256(manifest_bytes(manifest)).hexdigest()
    return NOTICE_TEMPLATE.format(manifest_sha256=manifest_sha256).encode("utf-8")


def write_selection(
    output: Path, manifest: dict[str, object], copies: list[tuple[Path, str]]
) -> None:
    """Publish into a newly reserved directory; manifest.json appears last."""

    staging = Path(tempfile.mkdtemp(prefix=f".{output.name}.staging-", dir=output.parent))
    output_reserved = False
    try:
        for source, relative in copies:
            destination = staging / PurePosixPath(relative)
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, destination, follow_symlinks=False)
            os.chmod(destination, 0o644)
            expected = next(
                layer
                for rig in manifest["rigs"]
                for layer in rig["layers"]
                if layer["file"] == relative
            )
            copied = inspect_png(destination)
            if copied["bytes"] != expected["bytes"] or copied["sha256"] != expected["sha256"]:
                fail(f"copied PNG changed before publication: {relative}")

        runtime_manifest = staging / "manifest.js"
        with runtime_manifest.open("xb") as handle:
            handle.write(runtime_manifest_bytes(manifest))
            handle.flush()
            os.fsync(handle.fileno())
        os.chmod(runtime_manifest, 0o644)

        notice = staging / "NOTICE.md"
        with notice.open("xb") as handle:
            handle.write(notice_bytes(manifest))
            handle.flush()
            os.fsync(handle.fileno())
        os.chmod(notice, 0o644)

        manifest_temporary = staging / "manifest.json"
        with manifest_temporary.open("xb") as handle:
            handle.write(manifest_bytes(manifest))
            handle.flush()
            os.fsync(handle.fileno())
        os.chmod(manifest_temporary, 0o644)

        output.mkdir(mode=0o700)
        output_reserved = True
        os.replace(staging / "rigs", output / "rigs")
        os.replace(runtime_manifest, output / "manifest.js")
        os.replace(notice, output / "NOTICE.md")
        os.replace(manifest_temporary, output / "manifest.json")
        staging.rmdir()
        os.chmod(output, 0o755)
    except BaseException:
        if output_reserved:
            shutil.rmtree(output)
        shutil.rmtree(staging, ignore_errors=True)
        raise


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Validate and select AI-Sister's fixed 17 workplace rigs (dry-run by default)."
    )
    parser.add_argument(
        "--tachie-root", required=True, help="absolute path to the local tachie root"
    )
    parser.add_argument("--output", required=True, help="absolute, new output directory")
    parser.add_argument(
        "--write",
        action="store_true",
        help="create the output directory; without this flag only print the sanitized manifest",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    arguments = parse_args(sys.argv[1:] if argv is None else argv)
    try:
        tachie_root, output = validate_cli_paths(arguments.tachie_root, arguments.output)
        manifest, copies = build_selection(tachie_root)
        if arguments.write:
            write_selection(output, manifest, copies)
        sys.stdout.buffer.write(manifest_bytes(manifest))
        return 0
    except SelectionError as error:
        print(f"error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
