#!/usr/bin/env python3
"""Keep the release signing data flow connected from Windows build to publication."""

from __future__ import annotations

import json
import pathlib
import sys


ROOT = pathlib.Path(__file__).resolve().parents[1]
WORKFLOW = ROOT / ".github/workflows/ci.yml"
TAURI_WINDOWS = ROOT / "apps/desktop/src-tauri/tauri.windows.conf.json"
SIGNING = ROOT / "scripts/windows-signing.ps1"


def fail(message: str) -> None:
    print(f"✗ {message}", file=sys.stderr)
    raise SystemExit(1)


def require_once(text: str, needle: str, where: str) -> int:
    count = text.count(needle)
    if count != 1:
        fail(f"{where} 應恰有一個 `{needle}`，實際 {count}")
    return text.index(needle)


def require_in_order(text: str, needles: list[str], where: str) -> None:
    cursor = -1
    for needle in needles:
        position = require_once(text, needle, where)
        if position <= cursor:
            fail(f"{where} 的順序不符：`{needle}`")
        cursor = position


def main() -> None:
    workflow = WORKFLOW.read_text(encoding="utf-8")
    release_job_parts = workflow.split("\n  release:\n", maxsplit=1)
    if len(release_job_parts) != 2:
        fail("CI workflow 應恰有一個 release job")
    release_job = release_job_parts[1]
    signing = SIGNING.read_text(encoding="utf-8")
    tauri_windows = json.loads(TAURI_WINDOWS.read_text(encoding="utf-8"))

    committed_config = json.dumps(tauri_windows, separators=(",", ":"))
    for key in ("certificateThumbprint", "timestampUrl", "signCommand"):
        if key in committed_config:
            fail(f"committed Tauri config 不得固定 release signing 欄位：{key}")

    for secret in (
        "secrets.AI_SISTER_WINDOWS_PFX_BASE64",
        "secrets.AI_SISTER_WINDOWS_PFX_PASSWORD",
    ):
        require_once(workflow, secret, "Windows signing secrets 入口")

    require_in_order(
        workflow,
        [
            "- name: Windows signing — select policy and import the release certificate",
            "- name: Windows signing — sign or prove unsigned sister.exe",
            "- name: 桌面姊妹 — lint, test and build NSIS",
            "- name: Installer — install, login registry, refuse live processes, reinstall, uninstall",
            "- name: Windows signing — bind the exact three release files to one receipt",
            "- name: Upload ai-sister.exe",
            "- name: Upload offline Windows installer",
            "- name: Upload Windows signing receipt",
            "- name: Windows signing — create an isolated trusted CI fixture",
            "- name: Windows signing — exercise Tauri main, sidecar, uninstaller and Setup signing",
            "- name: Windows signing — install and verify every fixture layer",
            "- name: Windows signing — remove imported certificates",
        ],
        "Windows build signing transaction",
    )
    require_in_order(
        release_job,
        [
            "name: ai-sister-installer-x64\n",
            "name: windows-signing-receipt\n",
            "- name: Windows signing — verify receipt against the exact release files",
            "- name: Create draft release and upload installer plus portable executables",
        ],
        "release signing receipt transaction",
    )

    required_workflow_fragments = (
        "-Action Prepare `",
        "-Action SignExpected `",
        'signing+=(--config "$AI_SISTER_WINDOWS_TAURI_SIGNING_CONFIG")',
        "-Action VerifyExpected `",
        "-Action Receipt `",
        "-Action PrepareSelfTest `",
        '--config "$AI_SISTER_WINDOWS_SIGNING_SELF_TEST_CONFIG"',
        "python3 ./scripts/check-windows-signing-receipt.py \\",
        "if: always()",
    )
    for fragment in required_workflow_fragments:
        if fragment not in workflow:
            fail(f"Windows signing workflow 缺少 `{fragment}`")

    required_signing_fragments = (
        "$timestampUrl = 'http://timestamp.digicert.com'",
        "Where-Object { $_.ObjectId -ceq $codeSigningEku }",
        "Import-PfxCertificate",
        "Assert-CertificateUsable $certificate",
        "/fd SHA256 /sha1 $thumbprint /d AI-Sister /tr $timestampUrl /td SHA256",
        "certificateThumbprint = $thumbprint",
        "timestampUrl = $timestampUrl",
        "tsp = $true",
        "TimeStamperCertificate",
        "& $signTool verify /pa /all /v",
        "正式 Windows release 不接受 self-signed certificate",
    )
    for fragment in required_signing_fragments:
        if fragment not in signing:
            fail(f"Windows signing implementation 缺少 `{fragment}`")
    if ".ObjectId.Value" in signing:
        fail("Certificate Provider 的 EnhancedKeyUsageList.ObjectId 已是字串，不得再取 .Value")

    print("✓ Windows signing：PFX → 三層 build → 四層 trust → receipt → release")


if __name__ == "__main__":
    main()
