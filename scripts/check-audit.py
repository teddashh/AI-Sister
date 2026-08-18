#!/usr/bin/env python3
"""驗 `sister stats --json` 裡的稽核數字。

這支腳本存在的理由，是它前一版的樣子：

    sys.exit(0 if d["redaction"]["leaked"] == 0 else 1)

那行永遠是綠的——因為當時餵進去的腳本裡，唯一的秘密是從 keepassxc 複製的，
而那個 app 整個被排除掉，剪貼簿根本沒被讀過。`flagged` 是 0，`leaked` 當然
也是 0，檢查通過，什麼都沒驗到。

一個「沒有東西壞掉」和一個「沒有東西發生過」，在斷言上長得一模一樣。所以
這裡先要求該發生的事真的發生過，再去問它有沒有壞。
"""

import json
import sys


def fail(msg: str) -> None:
    print(f"::error::{msg}")
    sys.exit(1)


def main() -> None:
    with open(sys.argv[1], encoding="utf-8") as f:
        stats = json.load(f)

    redaction = stats.get("redaction")
    if redaction is None:
        fail("stats --json 裡沒有 redaction——遮蔽稽核的機器可讀路徑不見了")

    # 先確認實彈真的打出去了。少了這一行，下面那個 leaked == 0 是免費的。
    if redaction["flagged"] < 1:
        fail(
            "沒有任何剪貼簿內容被判定為疑似秘密。"
            "secret-copy 腳本裡有一組 AWS 金鑰，偵測沒抓到就是規則壞了——"
            "而不是『這次剛好沒有秘密』"
        )

    if redaction["leaked"] != 0:
        fail(
            f"{redaction['leaked']} 筆剪貼簿被標成秘密、字卻還留在資料庫裡。"
            "『內容沒有落地』這句話現在是假的"
        )

    exclusions = stats.get("exclusions")
    if not exclusions:
        fail(
            "排除稽核是空的。bill-lookup 裡有 keepassxc 和銀行網址兩條會生效的"
            "規則——查不到就代表稽核紀錄又變回只寫不讀"
        )

    for e in exclusions:
        if e["episodes"] < 1:
            fail(f"排除理由 {e['reason']!r} 的段數是 {e['episodes']}")

    print(
        f"✓ 遮蔽 {redaction['flagged']} 次、外洩 {redaction['leaked']} 次；"
        f"排除稽核 {len(exclusions)} 條理由"
    )


if __name__ == "__main__":
    main()
