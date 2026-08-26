#!/usr/bin/env bash
# 組出一個 tag 的 GitHub release 說明，印到 stdout。
#
#     ./scripts/release-notes.sh v0.1.0-alpha.68
#
# 來源是 `docs/RELEASE-NOTES.md`，取三節接起來：〈開場〉+ 那個 tag + 〈結尾〉。
#
# **這支存在的理由**：在它之前，release 說明是 `ci.yml` 裡寫死的一塊 570 行的
# 字，每出一版就往裡面疊一節「這一版：⋯⋯」，然後**整塊**貼上去。到 alpha.67
# 為止，一份 release 說明裡有三十個「這一版」，其中最多一個是真的。
#
# 所以這支腳本最要緊的行為不是「找到就貼」，是**找不到的時候會怎樣**：
# 它印一節明說「這一版沒有寫版本說明」，而不是讓上一版的字留在那裡。
# 沒寫和寫了是兩件事，長得要不一樣。
#
# 〈開場〉和〈結尾〉少掉是另一回事，那個會 exit 1 把 release 擋下來：
# 〈開場〉裡是同意書那幾行（「沒簽第一張，`record` 不會開始錄」），
# 少了它就是一份沒講同意書的下載頁。那是 AGENTS.md §1 那一條的範圍。
set -euo pipefail

TAG="${1:-}"
if [[ -z "$TAG" ]]; then
  echo "用法：$0 <tag>   例：$0 v0.1.0-alpha.68" >&2
  exit 2
fi

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC="$ROOT/docs/RELEASE-NOTES.md"

if [[ ! -f "$SRC" ]]; then
  echo "::error::找不到 $SRC——release 說明沒有來源" >&2
  exit 1
fi

# 取出 `## <名字>` 到下一個 `## ` 之間的內容（不含兩個標題本身）。
# 名字用 awk 的變數傳，不進 regex：tag 裡有 `.`，而 `.` 在 regex 裡是萬用字元，
# `v0.1.0-alpha.6` 會match 到 `v0x1y0-alpha.6`。這裡要的是整行相等。
section() {
  awk -v want="## $1" '
    $0 == want           { inside = 1; next }
    inside && /^## /      { inside = 0 }
    inside                { print }
  ' "$SRC"
}

OPENING="$(section "開場")"
CLOSING="$(section "結尾")"

if [[ -z "${OPENING//[[:space:]]/}" ]]; then
  echo "::error::docs/RELEASE-NOTES.md 沒有〈開場〉那一節——同意書那幾行會從 release 上消失" >&2
  exit 1
fi
if [[ -z "${CLOSING//[[:space:]]/}" ]]; then
  echo "::error::docs/RELEASE-NOTES.md 沒有〈結尾〉那一節——隱私文件連結會從 release 上消失" >&2
  exit 1
fi

THIS="$(section "$TAG")"

printf '%s\n' "$OPENING"

if [[ -z "${THIS//[[:space:]]/}" ]]; then
  # 沒寫就說沒寫。這裡**不可以**退回上一版的說明——那正是這支腳本要修掉的事。
  cat <<EOF
---

### 這一版：沒有寫版本說明

\`docs/RELEASE-NOTES.md\` 裡沒有 \`## $TAG\` 這一節，所以底下自動產生的
commit 清單是這一版唯一的說明。

上一版的說明**不會**被貼在這裡。
EOF
  echo "::warning title=這一版沒有寫版本說明::docs/RELEASE-NOTES.md 裡沒有 ## $TAG，release 只會有 commit 清單" >&2
else
  printf '%s\n' "$THIS"
fi

printf '%s\n' "$CLOSING"
