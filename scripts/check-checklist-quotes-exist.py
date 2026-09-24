#!/usr/bin/env python3
"""
驗收清單叫他去對照的那幾句話，產品裡真的說得出口嗎。

`docs/WINDOWS-CHECKLIST.md` 是這個專案唯一一份**只有他那台機器執行得了**的
測試，而他是一條一條照著做的。所以一句「那一格要說 X」如果 X 已經不在程式裡
了，後果不是排版難看：他照著找、找不到，然後回報一個沒有壞的東西壞了。那是
第 45 條那個假紅，換到文件上——而假紅比假綠更貴，因為它會吃掉他整輪驗證的
信任。

2026-08-20 一天之內抓到五個：

  1. `%APPDATA%\\sister` ×5（真的位置是 `%APPDATA%\\ted-h\\AI-Sister\\data`）
  2. `check-combo-is-readable.sh` 改名成 `.py` 之後清單還指著 `.sh`
     —— 這兩族由 `check-docs-point-somewhere.py` 守著（那支管路徑）
  3. 「找不到資料目錄，停不了」——程式裡有這個字串，但那條路真機器上走不到
  4. 「她說不動——你還沒簽第一張同意書」——**程式從來沒說過這句話**，
     真正說的是「第一張同意書還沒簽——她不會開始記錄。……」
  5. 「這段時間裡沒有東西」／「還沒有東西，去按開始記錄」——兩句都是我編的，
     真的是 `timeline.js` 的「現在一天都沒有了。」和「一天都沒有。」

這支腳本守的是 4 和 5：**清單裡宣告產品會說的話，要在原始碼裡找得到**。

2026-08-20 第二輪（alpha.42）又抓到五個，而**三個是它自己漏掉的**：

  6. 「你還沒簽第一張同意書」還躺在 732 和 1020 行——第一輪修的是第 693 行
     那一份，然後宣告整族修好了。漏的原因是那兩行的動詞是「要換成」和
     「不可以有」，都不在 SAYS 裡。
  7. 「！UIA 在錄製途中卡住太多次已放棄」——那個「！」是清單自己加的
  8. 「存不進設定檔，退回原來那一組」——真的那句中間還有一個「所以」
  9. 「匯出檔沒有加密，丟進雲端同步資料夾就等於上傳」——整句改寫過，產品說
     的是「這份匯出沒有加密…丟進雲端同步資料夾，她記得的東西就跟著上去了」

  7、8、9 是這樣找到的：**把動詞過濾整個拿掉，看它噴出哪 30 條，然後一條一條
  讀**。大部分是散文轉述（該跳過），但十條看起來像產品字串的裡面有三條是真的
  ——量完誤報率就收工的話，這三條會留到他那台機器上。

**這份清單裡「」的意思，是「她逐字說得出來的話」。** 別拿它去框舊版清單的
措辭、或一段情境描述——這支腳本讀不出差別，而人也會誤讀。要講「以前寫的是
什麼」就直接寫成散文。

**這支腳本守不住的那一半，寫在這裡。**

  - 守不住第 3 種（字串在、但那條路走不到）。要驗那個得能判斷可達性，這裡
    做不到。所以「你應該看到 X」還是要自己問一次：X 那條 `if` 在他那台機器
    上進得去嗎？
  - 只認**逐字**出現，而且只在「這一條有宣告動詞」的時候才看。散文裡的引號
    多半是轉述（實測 167 個候選裡 41% 對不上），全掃的話這支腳本會變成一台
    誤報機器，然後被關掉——關掉之後它守的那條線是一格空白。
  - SAYS 不是語意分析器：不把「不可以說」當正面動詞，但同一條有正面
    動詞時，否定句、歷史文案、輸入問題與比喻的引號也會被挑中。它分不出
    哪句才是產品承諾；這些紅燈要人工分類，不能拿來宣告產品退化。
  - 已修掉兩個漏掃：同一條的折行先接起來（引號與宣告動詞可以分行），
    『』不再當佔位符，擷取外層引號之後才換成產品的「」。…⋯*{} 仍整句跳過
    （TEMPLATE），不拿去對模板。接縫按字元判斷：中文接中文不加空格；英數／
    反引號邊界或原有尾空白保留一格。英文字內折行、刻意多空格、中英刻意無空格
    仍無法推知，這些逐字引號應避免折在歧義處。接法不以原始碼是否命中來猜。
  - 清單上的實例（數字、`MM-DD HH:MM` 這種日期時間、`2026-08-26 18:04:12`
    這種實時刻、`en-US` 這種語言代碼、`1.2 GB` 這種位元組、單獨占位的
    M／N／X，以及括號裡用來占時刻的「時間」）和產品側的 `{}`、`{name}`、
    `${expr}`、`%s` 收成同一個萬用格再比。固定的字仍要逐字在。所以
    「留著 1 場錄製的紀錄本身」對得上「留著 {} 場錄製的紀錄本身」；把「錄製」
    寫成「錄音」仍然要紅。字面寫死的數字不會被產品側吃掉：清單寫 2、產品
    寫死 1，而且那個 1 不是內插，這條要紅。
  - 萬用格吃掉之後**看不見**的漂移，是格子裡那個具體值。1 換成 2、en-US
    換成 zh-TW、時刻換一個，四周的字沒動，閘門還是綠。組裝再鬆一階：產品用
    `{verb}`、`${why}` 留一個洞，清單在洞裡寫了一段固定的字時，只要求這段字
    在原始碼某處連續出現，不要求它就是填進這個洞的那個值。字還躺在別的註解
    裡、這個洞已經改填別句，這裡不會紅。
  - 第 3 種（舊版文案、禁止範例、使用者心裡的話、一段推論、叫他去按的操作
    說明）不是這支腳本分得出來的。2026-09-24 那 27 條是人讀過上下文之後改成
    散文的，沒有機械保證。以後再寫進「」的同類句子會亮紅燈，要人判；這支
    腳本不會自己把它們跳過，也沒有豁免表。
  - 原始碼裡有沒有那句話 ≠ 那句話出得來。註解裡寫著也算過，`#[cfg(test)]`
    裡寫著也算過。這是刻意放寬的：要抓的是「改了文案忘了改清單」，不是可達性。
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DOC = Path("docs/WINDOWS-CHECKLIST.md")

# 產品「說得出口的話」住在哪裡。JS 那幾支是字母人自己的文案，Rust 那邊是
# 後端回給它的錯誤訊息和 CLI 印出來的東西。
SOURCES = (
    "crates/**/*.rs",
    "apps/desktop/src-tauri/src/**/*.rs",
    "apps/desktop/ui/*.js",
    "apps/desktop/ui/*.html",
    # CI 那幾則 `::warning title=…` 也是清單叫他去對照的東西（第 81 行那句黃色
    # 的「中文 OCR 這一輪沒有被驗證」就住在這裡）。掃註解裡的散文會讓某些句子
    # 為了錯的理由過關，但 `.rs` 那幾份本來就整份掃，這裡沒有比較鬆。
    ".github/workflows/*.yml",
)

# 「這一條在宣告產品會說什麼」。沒有這幾個字的條目不看——見上面那段「守不住的
# 那一半」，散文裡的引號多半是轉述。
#
# **只收正面的宣告動詞，不收「不可以說」。** 兩者對這支腳本的意思相反：
# 「要出現 X」＝他得在畫面上找到 X，X 不存在就是叫他去找一個不存在的東西；
# 而「不可以說 X」＝X 最好**不要**存在（那多半正是上一輪修掉的那句話），
# 硬要求它存在會把每一條修好的紀錄都變成紅燈。實測：把動詞過濾整個拿掉會冒出
# 30 條，其中大半是散文轉述（「她起不來，因為資料庫打不開」是一種**讀法**，
# 不是一句產品字串），而「我此刻就是閉著眼睛的」那兩條正是被修掉的字串。
#
# `要換成` 是 alpha.42 補的：漏掉它的時候，清單上「那一格要換成『你還沒簽第一
# 張同意書』」活了整整一輪——而那一輪的 commit 標題就是「清單叫他去找兩句她
# 從來沒說過的話」，修的是**同一個字串**在另一行的那一份。修好小的那一半，
# 然後宣告整族修好了。
# 2026-09-23 補「只能說」。抓到的是第 734 行那句「慢訊息只能說『這一題已經超過
# 4 秒』」——產品裡**一個字都沒有**（現在那一格印的是秒數），而它從改文案那天
# 起就一直躺在那裡等他去找一句不存在的話。漏掉的原因和第 6 條一模一樣：動詞
# 對不上，這支腳本就整行跳過。
#
# 加之前量過誤報：全份清單用到「只能說」的只有四行，另外三行（102、352、439）
# 一行沒有引號、一行沒有引號、一行的引號是「啟動應用程式」六個字（低於 MIN=8
# 會被跳掉）。也就是**多這一個動詞只多出一條檢查，而那一條就是真的那一條**。
SAYS = re.compile(r"(要說|要寫|會寫|要出現|要換成|要講|講出|寫著|會說|只能說|印出|看到)")

# 另一種正面要求，只是句子長得像否定：**「X」前面不可以有「Y」**。他得先在
# 畫面上找到 X，才有「前面」可言——所以 X 和「要出現 X」是同一件事，而 Y 不是。
# 抓的是引號緊接著一個位置詞的那個形狀，而且**只收那一句**——同一行後面那句
# 「不可以有的 Y」不算，理由同上。
POSITION = re.compile(r"「([^「」\n]+)」\s*(?:前面|後面|底下|上面|旁邊)")
QUOTE = re.compile(r"「([^「」\n]+)」")

# 太短的引號多半是名詞（「電話」「門號」），對不上也沒意義。
# 帶刪節號、星號或花括號的整句仍整句跳過：清單自己在省略，不是一個可以
# 拿去對模板的實例。數字、語言代碼不要塞進這裡——那些要正規化之後真的比，
# 跳過就是把承諾刪掉。
MIN = 8
# 2026-09-24 未改清單量測：『』增加 5 句，3 句產品現文、1 句待復原、
# 1 句 OCR 情境轉述。先抽外層再正規化，否則會誤抽成內層短句。
TEMPLATE = re.compile(r"[…⋯*{}]|\bX \b|\bN ")

# 萬用格。只出現在「產品的內插」和「清單的實例值」，不拿來吃固定的字。
# 私用區字元，原始碼裡不該有；有的話寧願停，也不要跟原文撞車。
SLOT = "\ue000"

# 清單側。長的形要排在短的前面，不然 `1.2 GB` 會先被吃成數字，單位留下。
# 「（時間 」是清單拿兩個字占住一個時刻內插的寫法（上一場（時間 起））。
# 不把散文裡的「時間」吃掉，所以要求前面是全形括號、後面是空白。
_INSTANCE = re.compile(
    "|".join(
        (
            r"MM-DD HH:MM:SS",
            r"MM-DD HH:MM",
            r"YYYY-MM-DD HH:MM:SS",
            r"YYYY-MM-DD",
            r"\d{4}-\d{2}-\d{2}(?:[ T]\d{2}:\d{2}(?::\d{2})?)?",
            r"\d{2}:\d{2}:\d{2}",
            r"\b[a-z]{2,3}-[A-Z][a-z]{3}\b",
            r"\b[a-z]{2,3}-[A-Z]{2}\b",
            r"\d+(?:\.\d+)?\s*(?:TB|GB|MB|KB|B)\b",
            r"(?<![A-Za-z0-9])[MNX](?![A-Za-z0-9])",
            r"\d+(?:\.\d+)?",
        )
    )
)
_JS_INTERP = re.compile(r"\$\{[^}]*\}")
# `{}`、`{name}`、`{pct:.0}`、`{0}`。不含中間有空白的程式區塊。
_RUST_INTERP = re.compile(r"\{(?:[A-Za-z_][A-Za-z0-9_]*|\d+)?(?::[^}]{0,32})?\}")
# `%s` 這族。不碰 `%APPDATA%`：後面那個大寫不是轉換字。
_PRINTF_INTERP = re.compile(r"%[-+0 #]?\d*(?:\.\d+)?[diufFeEgGxXoscp]")


def normalize_source(src: str) -> str:
    """產品側只把內插收成萬用格。寫死的數字留著，不准拿去對清單上的另一個數。"""
    if SLOT in src:
        die("原始碼裡出現了萬用格用的私用字元", "換一個 SLOT，不要讓它和原文撞車。")
    # `{{`／`}}` 是格式字串裡的字面括號，不是洞。先挪走，免得被看成內插。
    out = src.replace("{{", "\ue001").replace("}}", "\ue002")
    out = _JS_INTERP.sub(SLOT, out)
    out = _RUST_INTERP.sub(SLOT, out)
    out = _PRINTF_INTERP.sub(SLOT, out)
    return out.replace("\ue001", "{").replace("\ue002", "}")


def normalize_quote(q: str) -> str:
    """清單側只把實例值收成萬用格。固定的字一個都不動。"""
    # 「上一場（時間 起）」的「時間」是時刻占位，括號和空格是固定的字。
    q = re.sub(rf"(?<=（)時間(?= )", SLOT, q)
    return _INSTANCE.sub(SLOT, q)


def _fits_forward(q, qi, s, si, raw, floor, memo):
    """q[qi:] 對上 s[si:] 的一個前綴。清單的萬用格只准對上產品的萬用格。"""
    key = (qi, si)
    if key in memo:
        return memo[key]
    if qi == len(q):
        memo[key] = True
        return True
    if si >= len(s) or si > floor:
        memo[key] = False
        return False
    ok = False
    if s[si] == SLOT:
        if q[qi] == SLOT and _fits_forward(q, qi + 1, s, si + 1, raw, floor, memo):
            ok = True
        else:
            # 產品的洞吃清單上的一段固定字（組裝）。這段字必須在原始碼裡
            # 連續出現，但不必證明就是這個洞填進去的那個值。
            limit = min(len(q), qi + 160)
            k = 1
            while qi + k <= limit and SLOT not in q[qi : qi + k]:
                filler = q[qi : qi + k]
                if filler in raw and _fits_forward(
                    q, qi + k, s, si + 1, raw, floor, memo
                ):
                    ok = True
                    break
                k += 1
    elif q[qi] != SLOT and q[qi] == s[si]:
        ok = _fits_forward(q, qi + 1, s, si + 1, raw, floor, memo)
    memo[key] = ok
    return ok


def _fits_back(q, qi, s, si, raw, floor, memo):
    """q[:qi] 對上 s[:si] 的一個後綴。方向相反，規則和向前那支一樣。"""
    key = (qi, si)
    if key in memo:
        return memo[key]
    if qi == 0:
        memo[key] = True
        return True
    if si <= 0 or si < floor:
        memo[key] = False
        return False
    ok = False
    if s[si - 1] == SLOT:
        if q[qi - 1] == SLOT and _fits_back(q, qi - 1, s, si - 1, raw, floor, memo):
            ok = True
        else:
            k = 1
            while k <= qi and k <= 160 and SLOT not in q[qi - k : qi]:
                filler = q[qi - k : qi]
                if filler in raw and _fits_back(q, qi - k, s, si - 1, raw, floor, memo):
                    ok = True
                    break
                k += 1
    elif q[qi - 1] != SLOT and q[qi - 1] == s[si - 1]:
        ok = _fits_back(q, qi - 1, s, si - 1, raw, floor, memo)
    memo[key] = ok
    return ok


def _longest_affix(text, q, prefix):
    """`text` 貼著萬用格的那一側，取清單裡真的有的最長一段。"""
    lo, hi, best = 4, len(text), ""
    while lo <= hi:
        mid = (lo + hi) // 2
        piece = text[:mid] if prefix else text[len(text) - mid :]
        if piece in q:
            best = piece
            lo = mid + 1
        else:
            hi = mid - 1
    return best


def _slot_anchors(q, norm):
    """產品每個洞左右的固定字。組裝句的最長子字串常常是洞裡那段，不是骨架。"""
    found = []
    start = 0
    while True:
        i = norm.find(SLOT, start)
        if i < 0:
            break
        start = i + 1
        after = norm[i + 1 : i + 65]
        nxt = after.find(SLOT)
        if nxt >= 0:
            after = after[:nxt]
        nl = after.find("\n")
        if nl >= 0:
            after = after[:nl]
        piece = _longest_affix(after, q, True)
        if len(piece) >= 6:
            found.append((len(piece), q.find(piece), i + 1, piece))
        before = norm[max(0, i - 64) : i]
        prev = before.rfind(SLOT)
        if prev >= 0:
            before = before[prev + 1 :]
        nl = before.rfind("\n")
        if nl >= 0:
            before = before[nl + 1 :]
        piece = _longest_affix(before, q, False)
        if len(piece) >= 6:
            found.append((len(piece), q.find(piece), i - len(piece), piece))
    found.sort(key=lambda item: item[0], reverse=True)
    out, seen = [], set()
    for item in found:
        key = (item[1], item[2], item[3])
        if key in seen:
            continue
        seen.add(key)
        out.append(item)
        if len(out) >= 24:
            break
    return out


def _anchor_pieces(q, norm):
    """挑清單裡一段夠長、而且產品側真的有的字當錨。太常見的錨會對到別句。"""
    found = []
    for run in re.finditer(rf"[^{SLOT}]{{4,}}", q):
        text, start = run.group(), run.start()
        lo, hi, best = 4, len(text), 0
        while lo <= hi:
            mid = (lo + hi) // 2
            hit = False
            for i in range(0, len(text) - mid + 1):
                if text[i : i + mid] in norm:
                    hit = True
                    break
            if hit:
                best = mid
                lo = mid + 1
            else:
                hi = mid - 1
        if best < 4:
            continue
        for i in range(0, len(text) - best + 1):
            piece = text[i : i + best]
            if piece not in norm:
                continue
            positions = []
            at = 0
            while len(positions) < 12:
                j = norm.find(piece, at)
                if j < 0:
                    break
                positions.append(j)
                at = j + 1
            if len(positions) >= 12:
                continue
            for j in positions:
                found.append((best, start + i, j, piece))
    found.extend(_slot_anchors(q, norm))
    found.sort(key=lambda item: item[0], reverse=True)
    return found[:24]


def template_match(q, raw, norm):
    """逐字沒中的時候，才用萬用格再試一次。對不上就仍是紅，不因此跳過。"""
    nq = normalize_quote(q)
    if nq in norm:
        return True
    for _length, qpos, spos, piece in _anchor_pieces(nq, norm):
        q_end = qpos + len(piece)
        s_end = spos + len(piece)
        # 每個清單字最多吃掉一個產品字；視窗外面的對齊不是這一錨。
        if not _fits_forward(
            nq,
            q_end,
            norm,
            s_end,
            raw,
            s_end + (len(nq) - q_end),
            {},
        ):
            continue
        if _fits_back(nq, qpos, norm, spos, raw, spos - qpos, {}):
            return True
    return False


def die(msg, *extra):
    print(f"✗ {msg}")
    for e in extra:
        print(f"  {e}")
    sys.exit(1)


def haystack():
    """所有產品字串串成一坨，並且把跨行接起來。

    Rust 的 `"abc\\` + 換行 + 縮排 + `def"` 在檔案裡是分兩行的，但它是**一個**
    字串。不接起來的話，清單裡逐字抄對的長句子反而會對不上——那就是一支對著
    正確的文件喊紅的閘門，而那種閘門會被關掉（第 46(a) 條）。JS 的 `\\` 續行
    同理。
    """
    out = []
    for pat in SOURCES:
        for f in sorted(ROOT.glob(pat)):
            out.append(f.read_text(encoding="utf-8", errors="ignore"))
    if not out:
        die("一個原始碼檔都沒掃到", "glob 對不到東西的時候，底下每一圈都是空轉。")
    text = "\n".join(out)
    # 續行：反斜線 + 換行 + 接下來那一行的縮排，整段拿掉。
    return re.sub(r"\\\n\s*", "", text)


def items(lines):
    """合併每個 checkbox 與六格縮排續行；逐字保留來源行號。

    空行不結束條目；下一個 checkbox 或非縮排散文才結束。接縫只處理
    換行和縮排，不改行內空白。origins[k] 是合併後第 k 個字的原始行號。
    """
    text, origins = "", []
    trailing_space = False
    for i, ln in enumerate(lines, 1):
        start = bool(re.match(r"\s*- \[[ x]\]", ln))
        if start or (text and ln.strip() and not ln.startswith("      ")):
            if text:
                yield origins, text
            text, origins = "", []
            trailing_space = False
        if start or (text and ln.startswith("      ") and ln.strip()):
            part = ln.strip()
            # 保留既有尾空白，或英數／code 邊界的一格；中文折行直接相接。
            space = text and (
                trailing_space
                or re.search(r"[A-Za-z0-9`]$", text)
                or re.match(r"[A-Za-z0-9`]", part)
            )
            joined = (" " if space else "") + part
            text += joined
            origins.extend([i] * len(joined))
            trailing_space = ln != ln.rstrip()
    if text:
        yield origins, text


def main():
    path = ROOT / DOC
    if not path.exists():
        die(f"{DOC} 不見了", "這支腳本整個沒有意義了——要嘛改路徑，要嘛把它刪掉。")
    lines = path.read_text(encoding="utf-8").split("\n")
    src = haystack()
    norm = normalize_source(src)

    checked, bad = 0, []
    for origins, ln in items(lines):
        wanted = list(QUOTE.finditer(ln)) if SAYS.search(ln) else []
        wanted += list(POSITION.finditer(ln))
        # 同一引號被兩條規則挑中只算一次；不同位置的相同文案各自有行號。
        seen = set()
        for match in wanted:
            if match.start() in seen:
                continue
            seen.add(match.start())
            q = match[1]
            if len(q) < MIN or TEMPLATE.search(q):
                continue
            q = q.translate(str.maketrans("『』", "「」"))
            checked += 1
            if q not in src and not template_match(q, src, norm):
                bad.append((origins[match.start()], q))

    # 活體下限。守的是「掃描器還活著」，不是「今天有幾句」。
    #
    # 擋得住兩種儀器壞法：
    #   - 條目排版整個對不上（`- [ ]` / 六格縮排變了），checked 掉到接近 0。
    #   - 接行被拿掉、每一行自己掃。2026-09-24 用這支掃描器量過：接行是
    #     180 句，不接行是 74 句。改散文之前的清單逐行也是 74，那 27 條
    #     本來就不是逐行看得到的。上一輪的逐行實作量到 71，差 3 句；74 仍
    #     遠低於 100。
    # 擋不住：產品或清單少了幾十句，但剩下的仍明顯多於逐行（74）。那種
    # 下降和「掃描器漏了幾十句」只要沒掉到 100 以下，會印出同一則通過，
    # 不會走到這裡。所以這條不拿來分辨它們，也不把下限釘死在今天的 180。
    # 少數漏掃、散文被當成承諾、字串在但那條路走不到，靠定點突變和人工分類。
    if checked < 100:
        die(
            f"只挑出 {checked} 句話來對，太少了",
            "少於 100 句代表掃描器壞了：要嘛條目排版對不上，要嘛退回了逐行掃描。",
            "這條線不代表產品少了幾句——那種下降只要還明顯多於逐行，不會走到這裡。",
        )

    if bad:
        print(f"✗ 檢查 {checked} 句，其中 {len(bad)} 句在原始碼找不到，需分類：")
        for i, q in bad:
            print(f"  {DOC}:{i}")
            print(f"      「{q}」")
        print()
        print("  逐條分辨清單過期、產品退化、轉述／模板／組裝；找不到不等於產品說不出口。")
        print("  有現行文案證據才更新清單；產品退化須修產品，不能為了綠燈改承諾。")
        sys.exit(1)

    print(f"✓ 清單裡那 {checked} 句「產品會說 X」，X 在原始碼裡都找得到")


main()
