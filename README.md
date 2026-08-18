# AI-Sister（字母人）

> 一個站在桌面角落的姊妹。她一直都在，看得見你的一天，記得住細節，
> 95% 的時間安靜，該說話的時候才說話——說的每一句都能點開證據。
>
> An open-source, local-first desktop companion: a filing cabinet that never
> forgets, an event-driven brain that can admit it's wrong, and a letter-person
> who knows when to stay quiet. Raw screen data never leaves your machine.

**Status: Phase 0，Windows alpha 可以下載來跑**
（[Releases](https://github.com/teddashh/AI-Sister/releases)）。

跑得起來的是最底下那一層：看畫面、讀字、抓出電話與金額之類的事實、存進 SQLite、搜得回來。
**這一層一個模型呼叫都沒有**，全部是程式在抄寫。L2/L3（會推論的那個腦）還沒開始。

先跑 `sister doctor`——它不會宣稱任何東西，只會當場示範給你看：能不能讀到你現在的網址、
OCR 引擎讀不讀得出內建那張圖上的字、哪幾條隱私規則現在其實不生效。

已經量過的（GitHub Actions 的 windows-latest runner、release build、1024×768，
**不是**一般桌機的數字，主要拿來擋回歸）：

| 一次要多少 | 實測 |
|---|---|
| 抓一次畫面（縮到 OCR 用的大小） | 30.1 ms |
| OCR 一張 | 193.3 ms |
| 寫一張 PNG | 6.3 ms |
| 沒有人動鍵盤滑鼠的那些 tick | 0 ms（根本不碰螢幕，最多每 5 秒睜眼一次） |

Phase 0 的退場條件是「連續 7 天自我錄製、CPU <3%、RAM <400MB、磁碟 <300MB/天」，
還沒達成——真實機器上的數字要等實地跑完才會寫進來。

## 文件地圖

| 文件 | 內容 |
|---|---|
| [docs/PRODUCT.md](docs/PRODUCT.md) | Final Product 定義：定位、信條、killer scenarios、競品、護城河、non-goals |
| [docs/SPEC.md](docs/SPEC.md) | Final Spec：四層真相模型、五個子系統、隱私架構、成本模型、技術選型、懸案判決 |
| [docs/PHASES.md](docs/PHASES.md) | Phase 0–8 里程碑：每階段退役一個致命風險，附可量測 exit criteria |
| [docs/PRIVACY.md](docs/PRIVACY.md) | 承諾、邊界，以及**我們做不到的事**（旁人同意、規則式排除的極限） |
| [docs/DATA_INVENTORY.md](docs/DATA_INVENTORY.md) | 逐欄位盤點她到底存了什麼，含已知缺口 |
| [docs/THREAT_MODEL.md](docs/THREAT_MODEL.md) | 資產、攻擊者、明確不防禦的項目，以及三個真實發生過的靜默失效 |
| [research/landscape.md](research/landscape.md) | 競品與生態現況（2026-08 查證）：Recall、Rewind/Limitless、Screenpipe、Everywhere、各家桌面 AI、computer-use 專案 |
| [research/tech-stack.md](research/tech-stack.md) | 技術選型調查：per-OS capture、繁中 OCR、SQLite FTS5/向量、Tauri overlay、資源預算 |
| [research/cost-model.md](research/cost-model.md) | LLM 成本試算（2026-08 實價）：四種架構情境的月費 |

設計源頭是一場四模型（Claude / Gemini / Grok / ChatGPT）7 題 × 5 輪的 roundtable
辯論；其逐字萃取屬私下對話，未公開，但所有收斂結論與判決理由都寫進了上面三份設計文件
（特別是 SPEC §17 的懸案決策表）。

## 三個一句話

- **產品**：檔案櫃（L0/L1，零 LLM）+ 大腦（L2/L3，事件驅動、可推翻、可結案）+
  守門員（開口預算制）。
- **原則**：暴力用在保存，不用在生成；抄寫歸程式，意圖歸模型；敢開著比聰明重要。
- **路線**：先做「搜得到」（Sister 1.0），再做「想得對」（大腦），最後做「接得了手」
  （hands）——每一步都用重播評測的數字守門。
