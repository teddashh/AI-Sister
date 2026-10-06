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
# site/ 的發布白名單，兩邊都算：這裡列的每一個都要在，site/ 裡也不能有沒列的。
# 每一頁都是 template，各自要展開全部 token、各自對 persona manifest。
PAGES = ("index.html", "en/index.html")
STATIC = ("styles.css", "site.js", "flow.css", "flow.js")
TOKENS = {
    "__VERSION__",
    "__TAG__",
    "__WINDOWS_DOWNLOAD_URL__",
    "__LINUX_DOWNLOAD_URL__",
}
# 頁面裡的 href／src。有 scheme 的（https:、mailto:）和 // 開頭的不是這個網站的檔案。
REFERENCE = re.compile(r'(?<![\w-])(?:href|src)="([^"]*)"')
EXTERNAL = re.compile(r"^(?:[a-z][a-z0-9+.-]*:|//)", re.IGNORECASE)


def sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def product_version() -> str:
    with (ROOT / "Cargo.toml").open("rb") as handle:
        return tomllib.load(handle)["workspace"]["package"]["version"]


def check_source_allowlist(source: pathlib.Path) -> None:
    listed = {*PAGES, *STATIC}
    present = {
        path.relative_to(source).as_posix()
        for path in source.rglob("*")
        if not path.is_dir()
    }
    if present != listed:
        raise ValueError(
            "website source 與發布白名單不符："
            f"沒列的 {sorted(present - listed)}，缺的 {sorted(listed - present)}"
        )


def check_references(destination: pathlib.Path, page: str, html: str) -> None:
    """頁面指向的本機檔案都要在 output 裡，#錨點都要是同一頁的 id。"""
    root = destination.resolve()
    base = (root / page).parent
    ids = set(re.findall(r'(?<![\w-])id="([^"]+)"', html))
    for target in REFERENCE.findall(html):
        if EXTERNAL.match(target):
            continue
        path, _, fragment = target.partition("#")
        path = path.partition("?")[0]
        if not path:
            if fragment not in ids:
                raise ValueError(f"website {page} 的 #{fragment} 在同一頁找不到 id")
            continue
        resolved = (base / path).resolve()
        if path.endswith("/") or resolved.is_dir():
            resolved = resolved / "index.html"
        if root not in resolved.parents or not resolved.is_file():
            raise ValueError(f"website {page} 指向 output 裡沒有的檔案：{target}")


def build(destination: pathlib.Path, source: pathlib.Path = SOURCE) -> None:
    if destination.exists():
        raise ValueError(f"website output 已存在：{destination}")
    check_source_allowlist(source)

    version = product_version()
    tag = f"v{version}"
    windows_download_url = f"{REPOSITORY}/releases/download/{tag}/AI-Sister-Setup.exe"
    linux_download_url = (
        f"{REPOSITORY}/releases/download/{tag}/AI-Sister-Linux-X11-amd64.deb"
    )
    destination.mkdir(parents=True)

    pages: dict[str, str] = {}
    for page in PAGES:
        template = (source / page).read_text(encoding="utf-8")
        for token in TOKENS:
            if template.count(token) == 0:
                raise ValueError(f"website template {page} 缺少 {token}")
        rendered = (
            template.replace("__VERSION__", version)
            .replace("__TAG__", tag)
            .replace("__WINDOWS_DOWNLOAD_URL__", windows_download_url)
            .replace("__LINUX_DOWNLOAD_URL__", linux_download_url)
        )
        if any(token in rendered for token in TOKENS):
            raise ValueError(f"website template {page} 還有未展開 token")
        target = destination / page
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(rendered, encoding="utf-8")
        pages[page] = rendered
    for name in STATIC:
        shutil.copy2(source / name, destination / name)
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
    for page, rendered in pages.items():
        html_ids = set(re.findall(r'data-persona="([a-z0-9]+)"', rendered))
        if html_ids != ids:
            raise ValueError(
                f"website {page} 的 picker 與 persona manifest 不符：{sorted(html_ids ^ ids)}"
            )

    shutil.copy2(PERSONAS / "manifest.json", asset_root / "manifest.json")
    shutil.copy2(PERSONAS / "NOTICE.md", asset_root / "NOTICE.md")
    for page, rendered in pages.items():
        check_references(destination, page, rendered)
    print(
        f"✓ Website：{tag} / {len(pages)} pages / 17 personas / {total_bytes:,} image bytes"
    )


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=pathlib.Path, required=True)
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    build(args.output.resolve())


if __name__ == "__main__":
    main()
