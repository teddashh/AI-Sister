#!/usr/bin/env bash
# macOS ARM64 diagnostic only: verify the .app/sibling process tree, then permit one
# ScreenCaptureKit probe. It never writes or uploads captured pixels.

set -euo pipefail

app=${1:?usage: macos-app-tree-spike.sh AI-Sister.app OUTPUT_DIR}
output=${2:?usage: macos-app-tree-spike.sh AI-Sister.app OUTPUT_DIR}
test -d "$app"
umask 077
if test -e "$output"; then
  printf 'diagnostic output path already exists: %s\n' "$output" >&2
  exit 1
fi
mkdir -m 700 "$output"
probe="$output/probe"
mkdir -m 700 "$probe"

app=$(cd "$(dirname "$app")" && pwd -P)/$(basename "$app")
main="$app/Contents/MacOS/sister-desktop"
child="$app/Contents/MacOS/sister"
plist="$app/Contents/Info.plist"
test -x "$main"
test -x "$child"
test -f "$plist"

main_arch=$(lipo -archs "$main")
child_arch=$(lipo -archs "$child")
test "$main_arch" = arm64
test "$child_arch" = arm64
for name in child main; do
  case "$name" in
    child) target=$child ;;
    main) target=$main ;;
  esac
  vtool -show-build "$target" > "$output/build-version-$name.txt"
  grep -Eq '^[[:space:]]*minos 14\.0$' "$output/build-version-$name.txt"
done

python3 - "$plist" "$output/bundle.json" "$main" "$child" <<'PY'
import json
import pathlib
import plistlib
import sys

plist_path, output_path, main_path, child_path = sys.argv[1:]
with open(plist_path, "rb") as handle:
    info = plistlib.load(handle)

expected = {
    "CFBundleIdentifier": "com.ted-h.ai-sister",
    "CFBundleExecutable": "sister-desktop",
    "LSMinimumSystemVersion": "14.0",
}
for key, value in expected.items():
    if info.get(key) != value:
        raise SystemExit(f"{key} mismatch: expected={value!r}, actual={info.get(key)!r}")
purpose = info.get("NSScreenCaptureUsageDescription")
if not isinstance(purpose, str) or not purpose.strip():
    raise SystemExit("NSScreenCaptureUsageDescription is absent or empty")

record = {
    "schema": 1,
    "identifier": info["CFBundleIdentifier"],
    "executable": info["CFBundleExecutable"],
    "minimum_system_version": info["LSMinimumSystemVersion"],
    "screen_capture_usage_description": purpose,
    "main": str(pathlib.Path(main_path).resolve()),
    "child": str(pathlib.Path(child_path).resolve()),
}
pathlib.Path(output_path).write_text(
    json.dumps(record, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
)
PY

codesign --verify --strict "$child"
codesign --verify --strict "$main"
codesign --verify --deep --strict "$app"
for name in child main app; do
  case "$name" in
    child) target=$child ;;
    main) target=$main ;;
    app) target=$app ;;
  esac
  codesign -dvvv "$target" > "$output/codesign-$name.txt" 2>&1
  codesign -d -r- "$target" > "$output/requirement-$name.txt" 2>&1
  codesign -d --entitlements :- "$target" > "$output/entitlements-$name.plist" 2>&1 || true
  grep -q 'Signature=adhoc' "$output/codesign-$name.txt"
  grep -q 'TeamIdentifier=not set' "$output/codesign-$name.txt"
  grep -Eq 'flags=.*\(.*runtime.*\)' "$output/codesign-$name.txt"
done

# A locally built ad-hoc app is expected not to pass Gatekeeper assessment. Preserve the
# result, but do not turn it into a claim that the app is notarized or distributable.
set +e
spctl --assess --type execute --verbose=4 "$app" > "$output/spctl.txt" 2>&1
spctl_status=$?
set -e
printf 'exit_status=%s\n' "$spctl_status" >> "$output/spctl.txt"

{
  printf 'swift_preflight='
  swift -e 'import CoreGraphics; print(CGPreflightScreenCaptureAccess())'
  for database in \
    "$HOME/Library/Application Support/com.apple.TCC/TCC.db" \
    "/Library/Application Support/com.apple.TCC/TCC.db"
  do
    printf '\n[%s]\n' "$database"
    if test -r "$database"; then
      sqlite3 "$database" \
        "SELECT service, client, auth_value, auth_reason, last_modified FROM access WHERE service='kTCCServiceScreenCapture' ORDER BY client;" \
        || printf 'query_unavailable\n'
    else
      printf 'unreadable_or_absent\n'
    fi
  done
} > "$output/tcc-baseline.txt" 2>&1 || true

open_pid=
app_pid=
child_pid=
cleanup() {
  if ! [[ "$app_pid" =~ ^[0-9]+$ ]] && test -f "$probe/app.json"; then
    app_pid=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["pid"])' \
      "$probe/app.json" 2>/dev/null || true)
  fi
  if ! [[ "$child_pid" =~ ^[0-9]+$ ]]; then
    if test -f "$probe/child.json"; then
      child_pid=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["pid"])' \
        "$probe/child.json" 2>/dev/null || true)
    elif test -f "$probe/spawn.json"; then
      child_pid=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["child_pid"])' \
        "$probe/spawn.json" 2>/dev/null || true)
    fi
  fi
  for pid in "$child_pid" "$app_pid"; do
    if [[ "$pid" =~ ^[0-9]+$ ]] && kill -0 "$pid" 2>/dev/null; then
      command=$(ps -o command= -p "$pid" 2>/dev/null || true)
      case "$command" in
        *"$app/Contents/MacOS/"*) kill "$pid" 2>/dev/null || true ;;
      esac
    fi
  done
  if [[ "$open_pid" =~ ^[0-9]+$ ]] && kill -0 "$open_pid" 2>/dev/null; then
    kill "$open_pid" 2>/dev/null || true
  fi
}
trap cleanup EXIT

open -n -W "$app" --args --macos-ci-app-tree-probe "$probe" \
  > "$output/open.stdout.log" 2> "$output/open.stderr.log" &
open_pid=$!

ready=false
for _ in $(seq 1 400); do
  if test -f "$probe/app-ready" && test -f "$probe/child-ready"; then
    ready=true
    break
  fi
  kill -0 "$open_pid" 2>/dev/null || break
  sleep 0.05
done
if test "$ready" != true; then
  printf 'app/child did not become ready\n' >&2
  find "$probe" -maxdepth 1 -type f -print -exec sed -n '1,80p' {} \; >&2 || true
  exit 1
fi

read -r app_pid child_pid < <(
  python3 - "$probe" "$main" "$child" "$output/process-paths.json" <<'PY'
import ctypes
import json
import os
import pathlib
import sys

directory = pathlib.Path(sys.argv[1])
expected_app = pathlib.Path(sys.argv[2])
expected_child = pathlib.Path(sys.argv[3])
output_path = pathlib.Path(sys.argv[4])
app = json.loads((directory / "app.json").read_text(encoding="utf-8"))
child = json.loads((directory / "child.json").read_text(encoding="utf-8"))
spawn = json.loads((directory / "spawn.json").read_text(encoding="utf-8"))
if app.get("schema") != 1 or app.get("role") != "app":
    raise SystemExit(f"invalid app record: {app!r}")
if child.get("schema") != 1 or child.get("role") != "capture_child":
    raise SystemExit(f"invalid child record: {child!r}")
if spawn != {
    "schema": 1,
    "app_pid": app.get("pid"),
    "child_pid": child.get("pid"),
}:
    raise SystemExit(f"spawn record does not bind both process records: {spawn!r}")
for role, record in (("app", app), ("child", child)):
    pid = record.get("pid")
    executable = record.get("executable")
    if isinstance(pid, bool) or not isinstance(pid, int) or pid <= 0:
        raise SystemExit(f"invalid {role} pid: {pid!r}")
    if not isinstance(executable, str) or not executable:
        raise SystemExit(f"invalid {role} executable: {executable!r}")
if os.path.realpath(app["executable"]) != os.path.realpath(expected_app):
    raise SystemExit(f"app executable escaped bundle: {app.get('executable')!r}")
if os.path.realpath(child["executable"]) != os.path.realpath(expected_child):
    raise SystemExit(f"child executable escaped bundle: {child.get('executable')!r}")

# Self-reported current_exe is useful but not authority. Ask macOS's libproc for the live
# executable behind each PID and require those paths to be the two files we just verified.
libproc = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True)
libproc.proc_pidpath.argtypes = [ctypes.c_int, ctypes.c_void_p, ctypes.c_uint32]
libproc.proc_pidpath.restype = ctypes.c_int

def proc_pidpath(pid):
    buffer = ctypes.create_string_buffer(4096)
    length = libproc.proc_pidpath(pid, buffer, len(buffer))
    if length <= 0:
        error = ctypes.get_errno()
        raise SystemExit(f"proc_pidpath({pid}) failed with errno {error}")
    return os.fsdecode(buffer.value)

observed_app = proc_pidpath(app["pid"])
observed_child = proc_pidpath(child["pid"])
if os.path.realpath(observed_app) != os.path.realpath(expected_app):
    raise SystemExit(f"live app PID points elsewhere: {observed_app!r}")
if os.path.realpath(observed_child) != os.path.realpath(expected_child):
    raise SystemExit(f"live child PID points elsewhere: {observed_child!r}")
output_path.write_text(
    json.dumps(
        {
            "schema": 1,
            "source": "proc_pidpath",
            "app_pid": app["pid"],
            "app_executable": observed_app,
            "child_pid": child["pid"],
            "child_executable": observed_child,
        },
        indent=2,
    )
    + "\n",
    encoding="utf-8",
)
print(app["pid"], child["pid"])
PY
)
[[ "$app_pid" =~ ^[0-9]+$ ]]
[[ "$child_pid" =~ ^[0-9]+$ ]]
kill -0 "$app_pid"
kill -0 "$child_pid"

actual_ppid=$(ps -o ppid= -p "$child_pid" | tr -d '[:space:]')
test "$actual_ppid" = "$app_pid"
{
  ps -o pid=,ppid=,user=,comm=,args= -p "$app_pid"
  ps -o pid=,ppid=,user=,comm=,args= -p "$child_pid"
} > "$output/process-tree.txt"

for role in app child; do
  case "$role" in
    app) pid=$app_pid ;;
    child) pid=$child_pid ;;
  esac
  set +e
  sudo -n launchctl procinfo "$pid" > "$output/procinfo-$role.txt" 2>&1
  procinfo_status=$?
  set -e
  printf '\nexit_status=%s\n' "$procinfo_status" >> "$output/procinfo-$role.txt"
done

python3 - "$output" "$app" <<'PY'
import json
import pathlib
import sys

directory, app = pathlib.Path(sys.argv[1]), sys.argv[2]
texts = []
available = True
for role in ("app", "child"):
    text = (directory / f"procinfo-{role}.txt").read_text(encoding="utf-8", errors="replace")
    texts.append(text)
    if "exit_status=0" not in text:
        available = False
joined = "\n".join(texts).lower()
if not available:
    classification = "unknown_procinfo_unavailable"
elif "hosted-compute-agent" in joined or "runner.worker" in joined:
    classification = "hosted_runner_ancestor_observed"
elif app.lower() in joined or "com.ted-h.ai-sister" in joined:
    classification = "ai_sister_chain_observed"
else:
    classification = "unknown"
(directory / "responsibility.json").write_text(
    json.dumps(
        {
            "schema": 1,
            "classification": classification,
            "inference": True,
            "basis": "diagnostic launchctl procinfo text; not a production API",
        },
        indent=2,
    )
    + "\n",
    encoding="utf-8",
)
PY

test ! -e "$probe/go"
python3 - "$probe/go" <<'PY'
import os
import sys

path = sys.argv[1]
flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
if hasattr(os, "O_NOFOLLOW"):
    flags |= os.O_NOFOLLOW
fd = os.open(path, flags, 0o600)
try:
    os.write(fd, b"schema=1\n")
    os.fsync(fd)
finally:
    os.close(fd)
PY
set +e
wait "$open_pid"
open_status=$?
set -e
open_pid=
test "$open_status" -eq 0
test -f "$probe/app-exit.json"
test -f "$probe/capture.json"

python3 - "$probe/capture.json" "$probe/app-exit.json" "${GITHUB_STEP_SUMMARY:-/dev/null}" <<'PY'
import json
import pathlib
import sys

capture_path, exit_path, summary_path = map(pathlib.Path, sys.argv[1:])
report = json.loads(capture_path.read_text(encoding="utf-8"))
app_exit = json.loads(exit_path.read_text(encoding="utf-8"))
if app_exit != {"schema": 1, "success": True, "code": 0}:
    raise SystemExit(f"app did not report one clean child exit: {app_exit!r}")
if report.get("schema") != 1:
    raise SystemExit(f"capture report schema mismatch: {report!r}")
preflight = report.get("preflight")
capture = report.get("capture")
if not isinstance(capture, dict):
    raise SystemExit(f"capture outcome is not an object: {capture!r}")
outcome = capture.get("outcome")
if preflight == "not_granted_or_undetermined":
    if outcome != "not_attempted":
        raise SystemExit("non-granted preflight still attempted ScreenCaptureKit")
elif preflight == "granted":
    if outcome == "not_attempted":
        raise SystemExit("granted preflight did not attempt ScreenCaptureKit")
else:
    raise SystemExit(f"unknown preflight result: {preflight!r}")

if outcome == "captured":
    width, height = capture.get("width"), capture.get("height")
    if not isinstance(width, int) or not isinstance(height, int) or width <= 0 or height <= 0:
        raise SystemExit(f"captured dimensions are not positive: {width!r}x{height!r}")
elif outcome in {"not_attempted", "no_display", "failed"}:
    pass
elif outcome == "invalid_image_dimensions":
    raise SystemExit(f"ScreenCaptureKit returned an invalid image: {capture!r}")
else:
    raise SystemExit(f"unknown capture outcome: {outcome!r}")

with summary_path.open("a", encoding="utf-8") as summary:
    summary.write("## macOS ARM64 diagnostic (not Preview)\n\n")
    summary.write(f"- TCC preflight: `{preflight}`\n")
    summary.write(f"- ScreenCaptureKit outcome: `{outcome}`\n")
    if outcome == "captured":
        summary.write(f"- Returned CGImage: `{capture['width']}×{capture['height']}`\n")
    if outcome == "failed":
        summary.write(
            "- Failure kind is the ScreenCaptureKit crate wrapper variant; it is not a "
            "claim about the native root cause.\n"
        )
if outcome != "captured":
    print(f"::warning::macOS diagnostic produced {outcome}; this run does not prove a pixel path")
PY

if find "$probe" -maxdepth 1 -type f \
  \( -iname '*.png' -o -iname '*.jpg' -o -iname '*.jpeg' -o -iname '*.webp' \
     -o -iname '*.bmp' -o -iname '*.tiff' -o -iname '*.db' -o -iname '*.sqlite' \) \
  | grep -q .
then
  printf 'probe wrote a pixel or database file\n' >&2
  exit 1
fi

if kill -0 "$child_pid" 2>/dev/null; then
  printf 'capture child is still alive after the app returned\n' >&2
  exit 1
fi
if kill -0 "$app_pid" 2>/dev/null; then
  printf 'app process is still alive after LaunchServices returned\n' >&2
  exit 1
fi
trap - EXIT
