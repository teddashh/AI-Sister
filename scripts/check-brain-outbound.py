#!/usr/bin/env python3
"""出境路徑的結構性閘門：沒簽同意書 2 編不過，外送紀錄不含原文。

產品第一次有東西離開這台機器。AGENTS.md 第一節的例外就是為這種事留的——
同意書那條路維持對抗式驗證。

**這裡不守去敏，因為產品不去敏了**（同意書 2 第 3 版）：送出去的是螢幕上的
原文。記憶長期活在本機資料庫裡，而代號是每次呼叫重編的，跨段對不起來——
承諾表和 entities 要的正是「王小明」這三個字能對得起來。他同意的就是這件事，
條文寫得很白。所以這支腳本守的是**他有沒有同意**，不是我們有沒有先遮。

守三件事：

1. `spawn_cli` 的第一個參數是 `CloudAllowed`。這個型別只有
   `Consent::cloud_permit` 鑄得出來（`CloudAllowed(())` 只准出現在
   `consent.rs`）。
2. `brain_outbound` 那張表沒有原文欄位——記結構和計數就好，
   抄一份等於把要保護的東西又存了一次。
3. `brain.rs` 裡沒有 HTTP / socket：出境只准走 `Command`。

每一條都是「改壞一行就該紅」。腳本末尾的自我檢查確認：把那一行改壞，
這裡真的會抓到。
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CONSENT = ROOT / "crates/sister-core/src/consent.rs"
BRAIN = ROOT / "crates/sister-core/src/brain.rs"
DB = ROOT / "crates/sister-core/src/db.rs"


class GateFail(Exception):
    pass


QUIET = False


def fail(message: str) -> None:
    if not QUIET:
        print(f"::error::{message}")
    raise GateFail(message)


def read(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def body_of(source: str, fn: str, path: Path) -> str:
    m = re.search(r"\bfn\s+" + re.escape(fn) + r"\s*\(", source)
    if not m:
        fail(f"{path.name} 找不到 `{fn}`——閘門靠名字找到它，改名要先改這支腳本")
    brace = source.index("{", m.end())
    depth = 0
    for i in range(brace, len(source)):
        if source[i] == "{":
            depth += 1
        elif source[i] == "}":
            depth -= 1
            if depth == 0:
                return source[m.start() : i + 1]
    fail(f"{path.name} 的 `{fn}` 大括號沒配對成功")


def check_cloud_allowed(consent: str, brain: str) -> None:
    mints = []
    for n, line in enumerate(consent.splitlines(), 1):
        code = line.split("//", 1)[0]
        if "struct CloudAllowed" in code:
            continue
        if re.search(r"CloudAllowed\s*\(\s*\(\s*\)\s*\)", code):
            mints.append(n)
    if not mints:
        fail("consent.rs 裡找不到 `CloudAllowed(())`——憑證沒有鑄造口了")
    if len(mints) != 1:
        fail(
            "consent.rs 裡 `CloudAllowed(())` 出現超過一次："
            f"{mints}。鑄造口只能有一個"
        )

    for path in (BRAIN, DB):
        src = read(path)
        for n, line in enumerate(src.splitlines(), 1):
            code = line.split("//", 1)[0]
            if re.search(r"CloudAllowed\s*\(\s*\(\s*\)\s*\)", code):
                fail(f"{path.relative_to(ROOT)}:{n} 自己鑄了 CloudAllowed")

    spawn = body_of(brain, "spawn_cli", BRAIN)
    sig = spawn.split("{", 1)[0]
    if "CloudAllowed" not in sig:
        fail("spawn_cli 的簽章沒有 CloudAllowed——沒簽同意書 2 也能編過")
    if not re.search(r"\b_?permit\s*:\s*CloudAllowed\b", sig):
        fail("spawn_cli 第一個憑證參數不叫 permit: CloudAllowed，閘門對不上")


def check_outbound_schema(db: str) -> None:
    m = re.search(
        r'const MIGRATION_010: &str = r#"(.*?)"#;',
        db,
        re.DOTALL,
    )
    if not m:
        fail("db.rs 找不到 MIGRATION_010——L2 / 外送紀錄的表沒人守")
    sql = m.group(1)
    if "CREATE TABLE IF NOT EXISTS brain_outbound" not in sql:
        fail("MIGRATION_010 沒有 brain_outbound")
    forbidden = {
        "prompt",
        "payload",
        "stdin",
        "stdout",
        "body",
        "raw",
        "ocr",
        "excerpt",
    }
    cols = re.findall(
        r"CREATE TABLE IF NOT EXISTS brain_outbound\s*\((.*?)\)",
        sql,
        re.DOTALL | re.IGNORECASE,
    )
    if not cols:
        fail("解析不出 brain_outbound 的欄位")
    names = {
        n.lower()
        for n in re.findall(r"^\s*([A-Za-z_][A-Za-z0-9_]*)\s", cols[0], re.MULTILINE)
    }
    # 子字串比對，不是整個名字相等。實測過：`prompt_text` 這種欄位名
    # 用集合交集是抓不到的（`prompt` != `prompt_text`），而它裝的正是原文。
    hit = sorted({n for n in names for w in forbidden if w in n})
    if hit:
        fail(f"brain_outbound 含原文欄位：{hit}——那等於把要保護的東西又抄一份")
    if "chars_sent" not in names:
        fail("brain_outbound 沒有 chars_sent——外送紀錄要說得出送了多少字")
    if "command" not in names:
        fail("brain_outbound 沒有 command")


def check_no_http_in_brain(brain: str) -> None:
    if re.search(r"\b(reqwest|ureq|hyper|TcpStream)\b", brain):
        fail("brain.rs 出現了 HTTP / socket——出境只准走 Command")


def mutate_and_expect(source: str, old: str, new: str, checker, label: str) -> None:
    if old not in source:
        fail(f"自我檢查找不到要改壞的原文（{label}）：{old!r}")
    broken = source.replace(old, new, 1)
    try:
        checker(broken)
    except GateFail:
        return
    fail(f"自我檢查沒抓到「{label}」——這支腳本改壞一行還是綠的")


def self_check() -> None:
    global QUIET
    QUIET = True
    consent = read(CONSENT)
    brain = read(BRAIN)
    db = read(DB)

    def cloud(src: str) -> None:
        check_cloud_allowed(consent, src)

    mutate_and_expect(
        brain,
        "permit: CloudAllowed",
        "permit: bool",
        cloud,
        "spawn_cli 改收 bool",
    )

    def schema(src: str) -> None:
        check_outbound_schema(src)

    mutate_and_expect(
        db,
        "chars_sent           INTEGER NOT NULL,",
        "chars_sent           INTEGER NOT NULL,\n  prompt               TEXT,",
        schema,
        "brain_outbound 加 prompt 欄",
    )
    QUIET = False


def main() -> None:
    consent = read(CONSENT)
    brain = read(BRAIN)
    db = read(DB)
    check_cloud_allowed(consent, brain)
    check_outbound_schema(db)
    check_no_http_in_brain(brain)
    self_check()
    print(
        "出境路徑：spawn_cli 要 CloudAllowed；"
        "憑證只在 consent.rs 鑄；brain_outbound 不含原文；"
        "自我檢查改壞三行都會紅"
    )


if __name__ == "__main__":
    try:
        main()
    except GateFail:
        raise SystemExit(1)
