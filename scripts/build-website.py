#!/usr/bin/env python3
"""Build the release-bound static website from checked-in source and persona assets."""

from __future__ import annotations

import argparse
import hashlib
import json
import pathlib
import re
import shutil
import tomllib


ROOT = pathlib.Path(__file__).resolve().parents[1]
SOURCE = ROOT / "site"
PERSONAS = ROOT / "apps/desktop/ui/personas"
REPOSITORY = "https://github.com/teddashh/AI-Sister"
TOKENS = {"__VERSION__", "__TAG__", "__DOWNLOAD_URL__"}


def sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def product_version() -> str:
    with (ROOT / "Cargo.toml").open("rb") as handle:
        return tomllib.load(handle)["workspace"]["package"]["version"]


def build(destination: pathlib.Path) -> None:
    if destination.exists():
        raise ValueError(f"website output 已存在：{destination}")

    version = product_version()
    tag = f"v{version}"
    download_url = f"{REPOSITORY}/releases/download/{tag}/AI-Sister-Setup.exe"
    destination.mkdir(parents=True)

    template = (SOURCE / "index.html").read_text(encoding="utf-8")
    for token in TOKENS:
        if template.count(token) == 0:
            raise ValueError(f"website template 缺少 {token}")
    rendered = (
        template.replace("__VERSION__", version)
        .replace("__TAG__", tag)
        .replace("__DOWNLOAD_URL__", download_url)
    )
    if any(token in rendered for token in TOKENS):
        raise ValueError("website template 還有未展開 token")
    (destination / "index.html").write_text(rendered, encoding="utf-8")
    shutil.copy2(SOURCE / "styles.css", destination / "styles.css")
    shutil.copy2(SOURCE / "site.js", destination / "site.js")
    (destination / ".nojekyll").write_text("", encoding="utf-8")

    manifest = json.loads((PERSONAS / "manifest.json").read_text(encoding="utf-8"))
    entries = manifest.get("assets")
    if not isinstance(entries, list) or len(entries) != 17:
        raise ValueError("website 需要 exact 17-persona manifest")
    asset_root = destination / "assets/personas"
    asset_root.mkdir(parents=True)
    ids: set[str] = set()
    total_bytes = 0
    for entry in entries:
        persona_id = entry["id"]
        file = entry["file"]
        if not re.fullmatch(r"[a-z0-9]+", persona_id) or not re.fullmatch(
            r"[a-z0-9]+\.webp", file
        ):
            raise ValueError(f"website persona path 不合法：{entry!r}")
        if file != f"{persona_id}.webp" or persona_id in ids:
            raise ValueError(f"website persona identity 不唯一：{entry!r}")
        source = PERSONAS / file
        if (
            not source.is_file()
            or source.stat().st_size != entry["bytes"]
            or sha256(source) != entry["sha256"]
        ):
            raise ValueError(f"website persona bytes/hash 不符：{file}")
        shutil.copy2(source, asset_root / file)
        ids.add(persona_id)
        total_bytes += entry["bytes"]

    if total_bytes != manifest["totals"]["webpBytes"]:
        raise ValueError("website persona total bytes 不符")
    html_ids = set(re.findall(r'data-persona="([a-z0-9]+)"', rendered))
    if html_ids != ids:
        raise ValueError(f"website picker 與 persona manifest 不符：{sorted(html_ids ^ ids)}")

    shutil.copy2(PERSONAS / "manifest.json", asset_root / "manifest.json")
    shutil.copy2(PERSONAS / "NOTICE.md", asset_root / "NOTICE.md")
    print(f"✓ Website：{tag} / 17 personas / {total_bytes:,} image bytes")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=pathlib.Path, required=True)
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    build(args.output.resolve())


if __name__ == "__main__":
    main()
