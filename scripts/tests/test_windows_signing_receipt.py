#!/usr/bin/env python3

from __future__ import annotations

import hashlib
import importlib.util
import pathlib
import tempfile
import unittest


SCRIPT = pathlib.Path(__file__).parents[1] / "check-windows-signing-receipt.py"
SPEC = importlib.util.spec_from_file_location("windows_signing_receipt", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def make_receipt(root: pathlib.Path, *, mode: str, tag: str) -> dict[str, object]:
    if mode == "production":
        subject = "CN=AI-Sister Release"
        thumbprint = "a" * 40
        signature = {
            "state": "trusted-rfc3161",
            "publisher": subject,
            "certificate_thumbprint": thumbprint,
            "timestamp_publisher": "CN=Timestamp Authority",
        }
        digest = "sha256"
        timestamp_url = MODULE.TIMESTAMP_URL
    else:
        subject = None
        thumbprint = None
        signature = {
            "state": "unsigned",
            "publisher": None,
            "certificate_thumbprint": None,
            "timestamp_publisher": None,
        }
        digest = None
        timestamp_url = None

    files = []
    for name in sorted(MODULE.EXPECTED_NAMES):
        path = root / name
        path.write_bytes(name.encode())
        files.append(
            {
                "name": name,
                "bytes": path.stat().st_size,
                "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
                "signature": dict(signature),
            }
        )
    return {
        "schema": 1,
        "mode": mode,
        "stable_release": MODULE.STABLE_TAG.fullmatch(tag) is not None,
        "ref_type": "tag",
        "ref_name": tag,
        "digest_algorithm": digest,
        "timestamp_url": timestamp_url,
        "certificate_subject": subject,
        "certificate_thumbprint": thumbprint,
        "files": files,
    }


class SigningReceiptTests(unittest.TestCase):
    def test_alpha_accepts_an_exact_unsigned_receipt(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            receipt = make_receipt(root, mode="unsigned", tag="v0.1.0-alpha.120")
            MODULE.validate_receipt(receipt, root, "v0.1.0-alpha.120")

    def test_stable_release_requires_production_signing(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            receipt = make_receipt(root, mode="unsigned", tag="v1.0.0")
            with self.assertRaisesRegex(ValueError, "不得發布 unsigned"):
                MODULE.validate_receipt(receipt, root, "v1.0.0")

    def test_production_requires_one_certificate_and_timestamp_on_every_file(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            receipt = make_receipt(root, mode="production", tag="v1.0.0")
            receipt["files"][1]["signature"]["timestamp_publisher"] = None
            with self.assertRaisesRegex(ValueError, "沒有 timestamp signer"):
                MODULE.validate_receipt(receipt, root, "v1.0.0")

    def test_receipt_is_bound_to_file_bytes(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            receipt = make_receipt(root, mode="production", tag="v1.0.0")
            (root / "sister.exe").write_bytes(b"changed")
            with self.assertRaisesRegex(ValueError, "bytes/hash 不符"):
                MODULE.validate_receipt(receipt, root, "v1.0.0")


if __name__ == "__main__":
    unittest.main()
