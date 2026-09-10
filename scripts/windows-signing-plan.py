#!/usr/bin/env python3
"""Choose the Windows release signing mode without reading certificate contents."""

from __future__ import annotations

import argparse
import json
import re


STABLE_TAG = re.compile(r"^v[0-9]+\.[0-9]+\.[0-9]+$")


def signing_plan(
    *, ref_type: str, ref_name: str, pfx_present: bool, password_present: bool
) -> dict[str, object]:
    if pfx_present != password_present:
        raise ValueError("Windows signing certificate and password must be configured together")

    is_tag = ref_type == "tag"
    stable_release = is_tag and STABLE_TAG.fullmatch(ref_name) is not None

    if stable_release and not pfx_present:
        raise ValueError("A stable release tag requires the Windows signing certificate")

    if is_tag and pfx_present:
        mode = "production"
    else:
        mode = "unsigned"

    return {
        "schema": 1,
        "mode": mode,
        "stable_release": stable_release,
        "ref_type": ref_type,
        "ref_name": ref_name,
    }


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--ref-type", required=True)
    parser.add_argument("--ref-name", required=True)
    parser.add_argument("--pfx-present", action="store_true")
    parser.add_argument("--password-present", action="store_true")
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    try:
        plan = signing_plan(
            ref_type=args.ref_type,
            ref_name=args.ref_name,
            pfx_present=args.pfx_present,
            password_present=args.password_present,
        )
    except ValueError as error:
        raise SystemExit(str(error)) from error
    print(json.dumps(plan, ensure_ascii=False, separators=(",", ":")))


if __name__ == "__main__":
    main()
