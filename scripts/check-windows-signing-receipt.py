#!/usr/bin/env python3
"""Verify the Windows job's signing receipt against the exact release files."""

from __future__ import annotations

import argparse
import hashlib
import json
import pathlib
import re


EXPECTED_NAMES = {"AI-Sister-Setup.exe", "sister-desktop.exe", "sister.exe"}
THUMBPRINT = re.compile(r"^[0-9a-f]{40}$")
STABLE_TAG = re.compile(r"^v[0-9]+\.[0-9]+\.[0-9]+$")
TIMESTAMP_URL = "http://timestamp.digicert.com"


def sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def validate_receipt(receipt: dict[str, object], root: pathlib.Path, tag: str) -> None:
    expected_root_keys = {
        "schema",
        "mode",
        "stable_release",
        "ref_type",
        "ref_name",
        "digest_algorithm",
        "timestamp_url",
        "certificate_subject",
        "certificate_thumbprint",
        "files",
    }
    if set(receipt) != expected_root_keys or receipt["schema"] != 1:
        raise ValueError("Windows signing receipt schema 不符")
    if receipt["ref_type"] != "tag" or receipt["ref_name"] != tag:
        raise ValueError("Windows signing receipt 沒有綁到目前 tag")

    stable = STABLE_TAG.fullmatch(tag) is not None
    if receipt["stable_release"] is not stable:
        raise ValueError("Windows signing receipt 的 stable_release 分類不符")

    mode = receipt["mode"]
    if mode not in {"production", "unsigned"}:
        raise ValueError(f"release 不接受 Windows signing mode：{mode!r}")
    if stable and mode != "production":
        raise ValueError("stable release 不得發布 unsigned Windows artifacts")

    if mode == "production":
        subject = receipt["certificate_subject"]
        thumbprint = receipt["certificate_thumbprint"]
        if receipt["digest_algorithm"] != "sha256":
            raise ValueError("正式 Windows 簽章不是 SHA-256")
        if receipt["timestamp_url"] != TIMESTAMP_URL:
            raise ValueError("正式 Windows 簽章沒有使用固定 RFC 3161 timestamp endpoint")
        if not isinstance(subject, str) or not subject.strip():
            raise ValueError("正式 Windows 簽章沒有 publisher subject")
        if not isinstance(thumbprint, str) or THUMBPRINT.fullmatch(thumbprint) is None:
            raise ValueError("正式 Windows 簽章沒有合法 certificate thumbprint")
    else:
        subject = None
        thumbprint = None
        for field in (
            "digest_algorithm",
            "timestamp_url",
            "certificate_subject",
            "certificate_thumbprint",
        ):
            if receipt[field] is not None:
                raise ValueError(f"unsigned receipt 的 {field} 必須是 null")

    files = receipt["files"]
    if not isinstance(files, list) or len(files) != 3:
        raise ValueError("Windows signing receipt 必須恰有三個 release files")
    names = [entry.get("name") for entry in files if isinstance(entry, dict)]
    if len(names) != 3 or set(names) != EXPECTED_NAMES:
        raise ValueError(f"Windows signing receipt file set 不符：{names!r}")

    for entry in files:
        if not isinstance(entry, dict) or set(entry) != {"name", "bytes", "sha256", "signature"}:
            raise ValueError("Windows signing receipt file schema 不符")
        path = root / entry["name"]
        if not path.is_file():
            raise ValueError(f"release file 不在：{path}")
        if entry["bytes"] != path.stat().st_size or entry["sha256"] != sha256(path):
            raise ValueError(f"Windows signing receipt bytes/hash 不符：{path.name}")

        signature = entry["signature"]
        signature_keys = {
            "state",
            "publisher",
            "certificate_thumbprint",
            "timestamp_publisher",
        }
        if not isinstance(signature, dict) or set(signature) != signature_keys:
            raise ValueError(f"Windows signature projection schema 不符：{path.name}")
        if mode == "production":
            if signature["state"] != "trusted-rfc3161":
                raise ValueError(f"正式 Windows artifact 沒有 trusted RFC 3161 signature：{path.name}")
            if signature["publisher"] != subject or signature["certificate_thumbprint"] != thumbprint:
                raise ValueError(f"Windows artifacts 沒有使用同一張 certificate：{path.name}")
            if not isinstance(signature["timestamp_publisher"], str) or not signature[
                "timestamp_publisher"
            ].strip():
                raise ValueError(f"Windows artifact 沒有 timestamp signer：{path.name}")
        elif signature != {
            "state": "unsigned",
            "publisher": None,
            "certificate_thumbprint": None,
            "timestamp_publisher": None,
        }:
            raise ValueError(f"unsigned Windows artifact projection 不符：{path.name}")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--receipt", type=pathlib.Path, required=True)
    parser.add_argument("--root", type=pathlib.Path, default=pathlib.Path.cwd())
    parser.add_argument("--tag", required=True)
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    receipt = json.loads(args.receipt.read_text(encoding="utf-8"))
    validate_receipt(receipt, args.root, args.tag)
    print(f"Windows signing receipt: {receipt['mode']} / 3 artifacts")


if __name__ == "__main__":
    main()
