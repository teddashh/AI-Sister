#!/usr/bin/env python3
"""Verify the exact licensed full-body Persona derivatives used as app icons."""

from __future__ import annotations

import hashlib
import json
import struct
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
ICON_DIR = ROOT / "apps/desktop/src-tauri/icons"
MANIFEST_SHA256 = "a94f4256b28da8b276e5b481195d22c90db0e7cc01d49dad0d361daa3f289f6f"
NOTICE_SHA256 = "55bc3a6ae541b52fddf28710179c7c96c21611bb1dde693f65efcb935140f4f9"
PERSONA_MANIFEST_SHA256 = "cf8e6e1b22f90f09ba021c092c3e0e9f5ae0dd39cf5644ffdfeb457ff3dd69c0"
SOURCE_SHA256 = "ee5a4e1636ad9ec10e90cd684a39899370820682e338fdeabea9e799b4012b81"
EXPECTED_ASSETS = {
    "32x32.png": (2929, "a74dfea2756b1b907ed448a5dda5ac67028375971a03f751de5398b540a49e4b", 32, 32),
    "128x128.png": (26828, "3c7b39235bdbdc1e6e7e610f6e96121a0abc8e8bd39063b207a8bde334796b01", 128, 128),
    "128x128@2x.png": (81525, "9ff5db041581484eba6b9d28d23a972e5b92dbdf595f26e81c76770a86a14a96", 256, 256),
    "icon.png": (224383, "bef6d399db5b48b2e87193eaab8969ccd8b2a32e671b96bdd08e3c99e561a744", 512, 512),
    "icon.ico": (128478, "79a6c4ba30fbe37ed80f2fd0a8ce65cb0ab32cbe5c9f5ff20185ab6341bef10f", None, None),
}
EXPECTED_RECIPE = {
    "html": (
        "../../icon/icon.html",
        "ffcb86b766df119e7d56d9122c4b2888df281acae056b5b68089bf39a1cc6099",
    ),
    "script": (
        "../../../../scripts/make-icons.sh",
        "4442f52b8cfe03cc629bd414006f8be46298993271cdf68019f1755febacaef4",
    ),
}


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def require(ok: bool, message: str) -> None:
    if not ok:
        raise SystemExit(f"✗ {message}")


def png_size(data: bytes, name: str) -> tuple[int, int]:
    require(data[:8] == b"\x89PNG\r\n\x1a\n", f"{name} 不是 PNG")
    require(len(data) >= 29 and data[12:16] == b"IHDR", f"{name} 缺少完整 IHDR")
    # PNG color type 4/6 才真的帶 alpha；透明 app icon 不能只靠副檔名宣稱。
    require(data[25] in (4, 6), f"{name} 沒有 alpha channel")
    return struct.unpack(">II", data[16:24])


def check_ico(data: bytes) -> None:
    require(len(data) >= 6, "icon.ico header 不完整")
    reserved, kind, count = struct.unpack("<HHH", data[:6])
    require((reserved, kind, count) == (0, 1, 7), "icon.ico 不是預期的 7-size Windows icon")
    require(len(data) >= 6 + 16 * count, "icon.ico directory 不完整")
    sizes = set()
    for index in range(count):
        entry = data[6 + index * 16 : 22 + index * 16]
        width = entry[0] or 256
        height = entry[1] or 256
        sizes.add((width, height))
    require(
        sizes == {(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)},
        f"icon.ico 尺寸集不對：{sorted(sizes)}",
    )


def main() -> None:
    expected_files = {*EXPECTED_ASSETS, "manifest.json", "NOTICE.md"}
    entries = list(ICON_DIR.iterdir())
    actual_files = {path.name for path in entries}
    require(actual_files == expected_files, f"icons 目錄多檔或少檔：{sorted(actual_files ^ expected_files)}")
    for path in entries:
        require(not path.is_symlink() and path.is_file(), f"icons 目錄只准 exact regular files：{path.name}")

    manifest_raw = (ICON_DIR / "manifest.json").read_bytes()
    notice_raw = (ICON_DIR / "NOTICE.md").read_bytes()
    require(sha256(manifest_raw) == MANIFEST_SHA256, "app icon manifest 不是核准的 exact bytes")
    require(sha256(notice_raw) == NOTICE_SHA256, "app icon NOTICE 不是核准的 exact bytes")
    require(MANIFEST_SHA256.encode() in notice_raw, "NOTICE 沒有 pin exact app icon manifest")
    manifest = json.loads(manifest_raw)

    require(manifest.get("schema") == "ai-sister/app-icons/v1", "app icon schema 不對")
    require(manifest.get("rightsReview") == "approved-owner-grant", "app icon rights review 未核准")
    require(manifest.get("notice") == "NOTICE.md", "app icon NOTICE 路徑不對")
    require(
        manifest.get("ownerGrant")
        == {
            "grantedOn": "2026-09-08",
            "grantor": "Ted Huang",
            "license": "excluded-from-Apache-2.0",
            "scope": "Inclusion of the exact five derived ChatGPT application-icon inputs hash-listed by this manifest in the AI-Sister source tree, and their platform-specific use or embedding by official builds",
        },
        "app icon owner grant 不是核准的 exact scope",
    )

    source = manifest.get("source")
    require(
        source
        == {
            "file": "../../ui/personas/chatgpt.webp",
            "personaManifestSha256": PERSONA_MANIFEST_SHA256,
            "sha256": SOURCE_SHA256,
        },
        "app icon source authority 不對",
    )
    source_raw = (ICON_DIR / source["file"]).resolve().read_bytes()
    require(sha256(source_raw) == SOURCE_SHA256, "app icon 的 ChatGPT source bytes 已改變")
    persona_manifest = (ROOT / "apps/desktop/ui/personas/manifest.json").read_bytes()
    require(sha256(persona_manifest) == PERSONA_MANIFEST_SHA256, "app icon 指到未核准的 Persona manifest")

    require(set(manifest.get("recipe", {})) == set(EXPECTED_RECIPE), "app icon recipe 集合不對")
    for kind, (relative, digest) in EXPECTED_RECIPE.items():
        require(
            manifest["recipe"].get(kind) == {"file": relative, "sha256": digest},
            f"app icon {kind} recipe 欄位不對",
        )
        require(sha256((ICON_DIR / relative).resolve().read_bytes()) == digest, f"app icon {kind} recipe bytes 已改變")

    declared = manifest.get("assets")
    require(isinstance(declared, list) and len(declared) == 5, "app icon manifest 不是 exact 五檔")
    require({row.get("file") for row in declared} == set(EXPECTED_ASSETS), "app icon manifest roster 不對")
    total = 0
    for row in declared:
        name = row["file"]
        expected_bytes, expected_hash, width, height = EXPECTED_ASSETS[name]
        data = (ICON_DIR / name).read_bytes()
        require(row.get("bytes") == expected_bytes == len(data), f"{name} bytes 不對")
        require(row.get("sha256") == expected_hash == sha256(data), f"{name} SHA-256 不對")
        require(row.get("format") == Path(name).suffix.removeprefix("."), f"{name} format 不對")
        if name.endswith(".png"):
            require(row.get("width") == width and row.get("height") == height, f"{name} manifest 尺寸不對")
            require(png_size(data, name) == (width, height), f"{name} 實際像素尺寸不對")
        else:
            require("width" not in row and "height" not in row, "ICO 不該冒充單一像素尺寸")
            check_ico(data)
        total += len(data)

    require(manifest.get("totals") == {"assets": 5, "bytes": total}, "app icon totals 不對")
    print(f"✓ app icons：exact 5 檔／{total:,} bytes，來源、recipe、alpha、尺寸與 owner grant 都對上")


if __name__ == "__main__":
    main()
