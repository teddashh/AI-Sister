#!/usr/bin/env python3

from __future__ import annotations

import importlib.util
import pathlib
import unittest


SCRIPT = pathlib.Path(__file__).resolve().parents[1] / "windows-signing-plan.py"
SPEC = importlib.util.spec_from_file_location("windows_signing_plan", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class WindowsSigningPlanTests(unittest.TestCase):
    def test_main_and_pull_request_builds_do_not_use_the_release_key(self) -> None:
        for ref_type, ref_name in (("branch", "main"), ("branch", "feature")):
            with self.subTest(ref_type=ref_type, ref_name=ref_name):
                self.assertEqual(
                    MODULE.signing_plan(
                        ref_type=ref_type,
                        ref_name=ref_name,
                        pfx_present=True,
                        password_present=True,
                    )["mode"],
                    "unsigned",
                )

    def test_prerelease_tag_can_build_unsigned(self) -> None:
        plan = MODULE.signing_plan(
            ref_type="tag",
            ref_name="v0.1.0-alpha.120",
            pfx_present=False,
            password_present=False,
        )
        self.assertEqual(plan["mode"], "unsigned")
        self.assertFalse(plan["stable_release"])

    def test_prerelease_tag_uses_the_certificate_when_configured(self) -> None:
        plan = MODULE.signing_plan(
            ref_type="tag",
            ref_name="v0.1.0-alpha.120",
            pfx_present=True,
            password_present=True,
        )
        self.assertEqual(plan["mode"], "production")
        self.assertFalse(plan["stable_release"])

    def test_stable_release_cannot_be_unsigned(self) -> None:
        with self.assertRaisesRegex(ValueError, "stable release tag requires"):
            MODULE.signing_plan(
                ref_type="tag",
                ref_name="v1.0.0",
                pfx_present=False,
                password_present=False,
            )

    def test_partial_secret_configuration_always_fails(self) -> None:
        for pfx_present, password_present in ((True, False), (False, True)):
            with self.subTest(
                pfx_present=pfx_present, password_present=password_present
            ):
                with self.assertRaisesRegex(ValueError, "configured together"):
                    MODULE.signing_plan(
                        ref_type="tag",
                        ref_name="v0.1.0-alpha.120",
                        pfx_present=pfx_present,
                        password_present=password_present,
                    )


if __name__ == "__main__":
    unittest.main()
