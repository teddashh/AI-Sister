#!/usr/bin/env python3
"""守住 tag、兩個 workspace、Tauri config、locks 與 release notes 的同一版號。"""

from __future__ import annotations

import argparse
import ast
import json
import pathlib
import re
import sys
import tomllib


ROOT = pathlib.Path(__file__).resolve().parents[1]


def toml(path: pathlib.Path) -> dict:
    with path.open("rb") as handle:
        return tomllib.load(handle)


def fail(message: str) -> None:
    print(f"✗ {message}", file=sys.stderr)
    raise SystemExit(1)


def local_sister_versions(lock_path: pathlib.Path) -> dict[str, str]:
    packages = toml(lock_path).get("package", [])
    return {
        package["name"]: package["version"]
        for package in packages
        if package.get("source") is None and package.get("name", "").startswith("sister-")
    }


def release_job_steps(workflow_path: pathlib.Path) -> list[list[str]]:
    """取 release job 的 step blocks；只解析這份 workflow 用到的縮排結構。"""
    lines = workflow_path.read_text(encoding="utf-8").splitlines()
    release_markers = [index for index, line in enumerate(lines) if line == "  release:"]
    if len(release_markers) != 1:
        fail(f"CI workflow 應有唯一 top-level release job，實際 {len(release_markers)} 個")
    start = release_markers[0]
    end = len(lines)
    for index in range(start + 1, len(lines)):
        if re.fullmatch(r"  [A-Za-z0-9_-]+:", lines[index]):
            end = index
            break

    job = lines[start:end]
    step_starts = [index for index, line in enumerate(job) if re.match(r"^      - [A-Za-z0-9_-]+:", line)]
    if not step_starts:
        fail("CI release job 沒有可解析的 steps")
    return [
        job[step_start : step_starts[offset + 1] if offset + 1 < len(step_starts) else len(job)]
        for offset, step_start in enumerate(step_starts)
    ]


def step_scalar(step: list[str], key: str) -> str | None:
    pattern = re.compile(rf"^(?:      - |        ){re.escape(key)}:\s*(.*?)\s*$")
    values = [match.group(1) for line in step if (match := pattern.fullmatch(line))]
    if len(values) > 1:
        fail(f"CI release step 重複宣告 {key}")
    return values[0] if values else None


def nested_scalar(step: list[str], parent: str, key: str) -> str | None:
    parent_line = f"        {parent}:"
    try:
        start = step.index(parent_line) + 1
    except ValueError:
        return None
    end = len(step)
    for index in range(start, len(step)):
        line = step[index]
        if line and len(line) - len(line.lstrip(" ")) <= 8:
            end = index
            break
    pattern = re.compile(rf"^          {re.escape(key)}:\s*(.*?)\s*$")
    values = [match.group(1) for line in step[start:end] if (match := pattern.fullmatch(line))]
    if len(values) > 1:
        fail(f"CI release step 的 {parent} 重複宣告 {key}")
    return values[0] if values else None


def nested_literal_lines(step: list[str], parent: str, key: str) -> list[str] | None:
    marker = f"          {key}: |"
    try:
        start = step.index(marker) + 1
    except ValueError:
        return None
    result: list[str] = []
    for line in step[start:]:
        if line and len(line) - len(line.lstrip(" ")) <= 10:
            break
        if line.startswith("            "):
            result.append(line[12:])
        elif not line:
            result.append("")
    while result and result[-1] == "":
        result.pop()
    return result


def run_literal_lines(step: list[str]) -> list[str] | None:
    marker = "        run: |"
    try:
        start = step.index(marker) + 1
    except ValueError:
        return None
    result = [line[10:] if line.startswith("          ") else "" for line in step[start:]]
    while result and result[-1] == "":
        result.pop()
    return result


def one_index(lines: list[str], value: str, what: str) -> int:
    indices = [index for index, line in enumerate(lines) if line == value]
    if len(indices) != 1:
        fail(f"{what} 應出現一次，實際 {len(indices)} 次")
    return indices[0]


def in_order(lines: list[str], values: list[str], what: str) -> None:
    """釘住 shell 的資料流順序，而不是只確認幾個裝飾性的字串還在。"""
    previous = -1
    for value in values:
        index = one_index(lines, value, f"{what} 的 `{value}`")
        if index <= previous:
            fail(f"{what} 的必要步驟順序不對：`{value}`")
        previous = index


def heredoc(lines: list[str], start: str, what: str) -> list[str]:
    start_index = one_index(lines, start, f"{what} 的 heredoc 起點")
    end_indices = [
        index for index in range(start_index + 1, len(lines)) if lines[index] == "PY"
    ]
    if not end_indices:
        fail(f"{what} 的 heredoc 沒有結尾")
    return lines[start_index + 1 : end_indices[0]]


def require_lines(lines: list[str], values: list[str], what: str) -> None:
    for value in values:
        one_index(lines, value, f"{what} 的 `{value}`")


def named_step(lines: list[str], name: str) -> list[str]:
    marker = f"      - name: {name}"
    starts = [index for index, line in enumerate(lines) if line == marker]
    if len(starts) != 1:
        fail(f"CI workflow 應有唯一 `{name}` step，實際 {len(starts)} 個")
    start = starts[0]
    end = len(lines)
    for index in range(start + 1, len(lines)):
        if re.match(r"^      - ", lines[index]):
            end = index
            break
    return lines[start:end]


def check_persona_release_gates(workflow_path: pathlib.Path) -> None:
    """完整 manifest 只在發版讀；fixed GET 另准人工發版前驗證。"""
    lines = workflow_path.read_text(encoding="utf-8").splitlines()
    tag_if = "startsWith(github.ref, 'refs/tags/v')"
    tag_or_manual_if = f"{tag_if} || github.event_name == 'workflow_dispatch'"

    manifest = named_step(lines, "Persona authority — full public manifest still matches")
    if step_scalar(manifest, "if") != tag_if:
        fail("Persona full-manifest gate 必須只在 v tag 執行")
    manifest_run = run_literal_lines(manifest)
    if not manifest_run:
        fail("Persona full-manifest gate 沒有 run block")
    one_index(
        manifest_run,
        "  cargo test -p sister-assets full_public_manifest_canonical_hash_and_projection_match -- --ignored",
        "Persona full-manifest gate 的 ignored test",
    )

    native_get = named_step(
        lines, "Persona pack — native Windows fixed GET, verify, install, remove"
    )
    if step_scalar(native_get, "if") != tag_or_manual_if:
        fail("Persona native fixed GET 必須只在 v tag 或人工 workflow dispatch 執行")
    if nested_scalar(native_get, "env", "AI_SISTER_ALLOW_ASSET_NETWORK") != '"1"':
        fail("Persona native fixed GET 必須明確以 AI_SISTER_ALLOW_ASSET_NETWORK=1 解鎖")
    if step_scalar(native_get, "run") != (
        "cargo test -p sister-assets --features download "
        "fixed_network_path_downloads_validates_installs_and_removes -- --ignored --nocapture"
    ):
        fail("Persona native fixed GET gate 必須執行唯一的 ignored native transport test")


def check_atomic_release_workflow(workflow_path: pathlib.Path) -> None:
    """防止 alpha prerelease 回到「先公開、再上傳」的 partial-release 路徑。"""
    steps = release_job_steps(workflow_path)
    creators = [
        (index, step)
        for index, step in enumerate(steps)
        if step_scalar(step, "uses") == "softprops/action-gh-release@v2"
    ]
    if len(creators) != 1:
        fail(f"CI release job 應有唯一 action-gh-release@v2 step，實際 {len(creators)} 個")
    creator_index, creator = creators[0]
    if step_scalar(creator, "id") != "create_release":
        fail("action-gh-release step 必須以 create_release id 暴露同一個 release ID")
    if nested_scalar(creator, "with", "draft") != "true":
        fail("action-gh-release 必須固定 draft: true；prerelease 不能先公開再傳 asset")
    if nested_scalar(creator, "with", "fail_on_unmatched_files") != "true":
        fail("action-gh-release 必須拒絕 unmatched local asset")
    if nested_literal_lines(creator, "with", "files") != [
        "sister.exe",
        "sister-desktop.exe",
    ]:
        fail("action-gh-release 的 local asset 必須恰為 sister.exe 與 sister-desktop.exe")

    publishers = [
        (index, step)
        for index, step in enumerate(steps)
        if step_scalar(step, "id") == "publish_release"
    ]
    if len(publishers) != 1:
        fail(f"CI release job 應有唯一 publish_release step，實際 {len(publishers)} 個")
    publisher_index, publisher = publishers[0]
    if publisher_index != creator_index + 1:
        fail("publish_release 必須緊接在 hidden draft upload 後")
    if nested_scalar(publisher, "env", "RELEASE_ID") != "${{ steps.create_release.outputs.id }}":
        fail("publish_release 沒有使用 create_release 回傳的同一個 release ID")
    run = run_literal_lines(publisher)
    if not run:
        fail("publish_release 沒有可執行的 run block")
    # draft 上傳後要讀 GitHub 的同一份 release，而不是採信 action 的 asset output。
    # 名稱／數量／uploaded state／大小／HTTPS API URL 全部在這段 Python 裡 fail-closed。
    remote_release_get = 'gh api "$release_api" > "$release_json"'
    asset_plan_start = 'python3 - "$release_json" "$GITHUB_REF_NAME" > "$asset_plan" <<\'PY\''
    if one_index(run, remote_release_get, "同一 draft release 的遠端 GET") >= one_index(
        run, asset_plan_start, "遠端 asset plan 的 heredoc 起點"
    ):
        fail("遠端 release GET 必須先於 asset plan 驗證")
    asset_plan_python = heredoc(run, asset_plan_start, "遠端 asset plan")
    # Python 的 block indentation 是語法，不應把「四格還是八格」誤當 release 契約。
    asset_plan_statements = [line.strip() for line in asset_plan_python if line.strip()]
    require_lines(
        asset_plan_statements,
        [
            'release = json.loads(pathlib.Path(release_path).read_text(encoding="utf-8"))',
            'if release.get("draft") is not True:',
            'if release.get("tag_name") != expected_tag:',
            'assets = release.get("assets")',
            'if not isinstance(assets, list):',
            'expected_names = {"sister.exe", "sister-desktop.exe"}',
            'names = [asset.get("name") for asset in assets if isinstance(asset, dict)]',
            'if len(assets) != 2 or len(names) != 2 or set(names) != expected_names:',
            'for asset in sorted(assets, key=lambda item: item["name"]):',
            'asset_id = asset.get("id")',
            'size = asset.get("size")',
            'state = asset.get("state")',
            'url = asset.get("url")',
            'if not isinstance(asset_id, int) or asset_id <= 0:',
            'if not isinstance(size, int) or size <= 0:',
            'if state != "uploaded":',
            'if not isinstance(url, str) or not url.startswith("https://"):',
            'print(f"{asset_id}\\t{asset[\'name\']}\\t{size}")',
        ],
        "遠端 asset plan",
    )

    # 不只比 API metadata：逐檔把 draft asset 讀回、先比遠端 size，再 bit-for-bit
    # 比 CI 剛下載的 Windows artifact。這三個命令必須都在同一個 asset-plan loop 裡。
    loop_start = "while IFS=$'\\t' read -r asset_id asset_name asset_size; do"
    loop_end = 'done < "$asset_plan"'
    start_index = one_index(run, loop_start, "遠端 asset download loop 起點")
    end_index = one_index(run, loop_end, "遠端 asset download loop 結尾")
    if end_index <= start_index:
        fail("遠端 asset download loop 的結尾在起點之前")
    loop = run[start_index : end_index + 1]
    in_order(
        loop,
        [
            loop_start,
            "  gh api \\",
            "    -H 'Accept: application/octet-stream' \\",
            '    "repos/${GITHUB_REPOSITORY}/releases/assets/${asset_id}" > "$downloaded"',
            '  test "$(wc -c < "$downloaded")" -eq "$asset_size"',
            '  cmp -- "$asset_name" "$downloaded"',
            loop_end,
        ],
        "遠端 asset download／byte compare",
    )

    payload_start = 'python3 - "$prerelease" > "$publish_payload" <<\'PY\''
    payload_python = heredoc(run, payload_start, "publish payload")
    try:
        payload_tree = ast.parse("\n".join(payload_python), filename="publish-payload")
    except SyntaxError as error:
        fail(f"publish payload Python 語法錯誤：{error.msg}")
    dumps = [
        node
        for node in ast.walk(payload_tree)
        if isinstance(node, ast.Call)
        and isinstance(node.func, ast.Attribute)
        and isinstance(node.func.value, ast.Name)
        and node.func.value.id == "json"
        and node.func.attr == "dump"
    ]
    if len(dumps) != 1 or not dumps[0].args or not isinstance(dumps[0].args[0], ast.Dict):
        fail("publish payload 必須以唯一 json.dump 寫出 JSON object")
    payload = dumps[0].args[0]
    draft_values = [
        value
        for key, value in zip(payload.keys, payload.values)
        if isinstance(key, ast.Constant) and key.value == "draft"
    ]
    if len(draft_values) != 1 or not (
        isinstance(draft_values[0], ast.Constant) and draft_values[0].value is False
    ):
        fail("publish payload 必須明確寫入 JSON boolean draft=false")

    final_command = next((line.strip() for line in reversed(run) if line.strip()), "")
    if final_command != 'gh api --method PATCH "$release_api" --input "$publish_payload" >/dev/null':
        fail("publish_release 的最後一步必須是同一 release ID 的 draft=false API PATCH")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--tag", help="tag build 時傳 GITHUB_REF_NAME；branch build 不傳")
    args = parser.parse_args()

    root_version = toml(ROOT / "Cargo.toml")["workspace"]["package"]["version"]
    desktop_version = toml(ROOT / "apps/desktop/src-tauri/Cargo.toml")["package"]["version"]
    tauri_version = json.loads(
        (ROOT / "apps/desktop/src-tauri/tauri.conf.json").read_text(encoding="utf-8")
    )["version"]

    observed = {
        "Cargo.toml [workspace.package]": root_version,
        "apps/desktop/src-tauri/Cargo.toml": desktop_version,
        "apps/desktop/src-tauri/tauri.conf.json": tauri_version,
        **{
            f"Cargo.lock::{name}": version
            for name, version in local_sister_versions(ROOT / "Cargo.lock").items()
        },
        **{
            f"apps/desktop/src-tauri/Cargo.lock::{name}": version
            for name, version in local_sister_versions(
                ROOT / "apps/desktop/src-tauri/Cargo.lock"
            ).items()
        },
    }
    wrong = {where: version for where, version in observed.items() if version != root_version}
    if wrong:
        detail = ", ".join(f"{where}={version}" for where, version in sorted(wrong.items()))
        fail(f"版號沒有一起更新；root={root_version}，{detail}")

    heading = f"## v{root_version}"
    notes = (ROOT / "docs/RELEASE-NOTES.md").read_text(encoding="utf-8").splitlines()
    if heading not in notes:
        fail(f"release notes 缺少 exact heading：{heading}")

    if args.tag is not None and args.tag != f"v{root_version}":
        fail(f"tag={args.tag}，但產品版號是 v{root_version}")

    workflow = ROOT / ".github/workflows/ci.yml"
    check_atomic_release_workflow(workflow)
    check_persona_release_gates(workflow)

    print(f"✓ Release 版號一致：v{root_version}（{len(observed)} 個位置）")


if __name__ == "__main__":
    main()
