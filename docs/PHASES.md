# AI-Sister — Phases / Milestones

> 排序原則：**按風險退役排，不按功能誘惑排。** 每個 phase 退役一個「會殺死這個專案的問題」。
> 每個 phase 有可量測的 exit criteria；沒過就不進下一個。時間以「專注週」計
> （solo + agent fleet 的節奏），非日曆承諾。
>
> 辯論的三句話定調整份計畫：
> 1. 「你的第一個 killer function，一隻 agent 都不需要。」
> 2. 「先出第一個。搜得到，第二個才有地基；搜不到，八隻 agent 開再多會也是在猜。」
> 3. 「不要再想架構了，去寫那個重播評測。」

---

## 總覽

| Phase | 名字 | 退役的風險 | 週 |
|---|---|---|---|
| 0 | 感官與地基 | 「抓不抓得到、扛不扛得動」 | 1–2 |
| 1 | Sister 1.0：檔案櫃 + 搜尋框 + 字母人 | 「第一週有沒有魔法時刻」 | 2–3 |
| 2 | 重播評測 harness | 「辯論 vs 數據」 | 1–2 |
| 3 | 斷句 + 事實層強化 | 「沒有論文可抄的核心演算法」 | 2–3 |
| 4 | 理解與記憶（大腦上線） | 「記憶會歪、會腐爛」 | 3–4 |
| 5 | 開口（守門員）+ macOS + 正式發布 | 「怕多嘴」＋公開信任 | 3–4 |
| 6 | 手 v1（suggest → semi-action） | 「有手就危險」 | 3–4 |
| 7 | 接手模式（bounded takeover） | 「記歪的東西點滑鼠」 | 3–4 |
| 8 | 生態 | 「單人維護死」 | 持續 |

到正式發布（P5 末）累計約 12–18 專注週。

---

## Phase 0 — 感官與地基（sister-core v0）

**目標**：capture 層在 Ted 自己的機器上跑滿 7 天，量出 CPU／RAM 基準並證明
仍有效的 RAM／磁碟足跡預算可達成。這一步同時產出 replay 語料的錄製器——
capture 層本身就是 recorder。

**Scope**
- Rust daemon 骨架：SQLite（WAL）+ migration、loopback API（token 驗證）、
  tray 圖示（pause/resume/quit）、開機自啟。
- Windows capture pipeline：變化驅動截圖（dHash 去重）→ 原生 OCR → L0 落地；
  前景視窗/URL（UIA）、剪貼簿、輸入動態（節奏不記內容）、idle/lock。
- 「事後補不回來」訊號清單全數當下抓（SPEC §2.1）。
- Capture 時排除 v0：app/URL blocklist、密碼欄位跳過、一鍵 pause。
- L1 facts 抽取器（regex：money/phone/url/email/error_code/file_path/datetime）。
- `sister query` CLI：FTS5 直查（CJK trigram），附 frame ref。
- Day-one 文件：PRIVACY.md、THREAT_MODEL.md、DATA_INVENTORY.md（沿 TokenMonster 寫法）。

**Exit criteria**
- [ ] 連續 7 天自用錄製，零 crash（或自復活無資料損失）。
  七天還沒跑，但**「零 crash」這半句現在有實作了**：`sister doctor` 的
  「零當機」那一行去數沒有 `ended_at` 的錄製段。Ctrl-C 走的是正常收尾，
  所以剩下的解釋只有被殺、當機、關機、拔電。在這之前這半句的驗證方式是
  使用者自己記不記得當過——那是印象，不是條件。
  （已知歧義：此刻正在錄的那一段也沒有 `ended_at`，那一行會把兩種可能
  都講出來，不猜。）
- [x] CPU／RAM 真機基準記入 README。alpha.46 在 Ted 的 Windows、1920×1080、
  活躍切換 workspace 寫程式的 60 秒裡是 CPU 平均 44.0%、RAM 峰值 73.7MB。
  RAM 通過 < 400MB；Ted 於 2026-08-23 選擇保留觀察密度，CPU 仍照實量，但不再是
  Phase 0 blocker。SPEC §2.3 的 < 3% 是長期產品目標，不是這一階段的 gate。
- [ ] 磁碟 < 300MB/天（實測數字記入 README）。alpha.47 的 60 秒歸因已回收：
  畫面 2.7MB；「其他」2.9MB 裡 WAL 2.8MB，SQLite 邏輯配置增加 156KB。
  收尾印出的 4.3GB/天／其他 4.1GB/天把可重用 WAL 當成永久成長，不作判決；
  main 已讓 WAL 退出每日計帳。容量仍高於長期目標；Ted 於 2026-08-23 決定
  **先完成要的功能與體驗，再研究容量優化**，所以這格保留未完成、數字照實公開，
  但不再擋 Phase 2 以後的功能 milestone，也不要求重跑診斷。
- [x] `sister query 電話` 能在 < 100ms 撈回三天前畫面上的客服電話，附出處。
  **量過了，四條路都在 1 ms 以內**（`crates/sister-core/tests/search_latency.rs`，
  45 天語料 = 3,110,400 行字，開發機）：三個字以上走 trigram 0.1 ms、整個
  token 走 unicode61 0.7 ms、**兩個字的中文走 bigram 0.1 ms**、查不到的東西
  0.1 ms。都附得出出處。
  這一條原本卡在「兩個字的中文詞沒有索引」——trigram 比不了 <3 字，而
  unicode61 把「客服專線」整串當成一個 token，所以只剩全表掃描：224 ms，
  而且為了不讓成本跟著使用時間長大，只好夾在 30 天內。退場條件裡那句
  `query 電話` 剛好就是兩個字，它挑到的正是唯一沒過的那條路。
  schema 3 補上 `text_fts_bi`（相鄰雙字，見 `db.rs` 的 `cjk_bigrams`）把縫
  補起來，30 天的界線也跟著消失。**代價比原型估的小**：實測 207,360 行字
  是 97.9 MB → 126.0 MB，**+29%**（原型估「翻倍」，那是在小語料上量的）。
- [x] 敏感排除驗證：密碼管理器/網銀 blocklist 生效、密碼欄位不落地（測試腳本證明）。
      腳本是 `crates/sister-capture/tests/privacy.rs`，`ci.yml` 每次 push 單獨跑它一次
      （不是混在 `cargo test --workspace` 裡）。它跑一段踩滿地雷的腳本——KeePassXC、
      國泰網銀（關鍵字在**網域**裡不在路徑裡，那是真的漏過一次的形狀）、Zoom 螢幕分享、
      標題含 password 的分頁、在被擋的 app 裡複製東西——然後把**整個資料目錄當成位元組**
      掃過去，確認那些字串一個都不在。刻意不查特定欄位：那樣只證明得了「我想到要檢查的
      地方是乾淨的」，而未來多一張表、多一個索引就漏了。
      **正反成對**：`ordinary_work_is_still_remembered` 專門證明同一場裡中華電信帳單那
      幾行**有**留下來——不然一個什麼都不記的壞版本會全綠。
      **這一格有一半是手驗的，不要當成全自動**：UIA 密碼欄偵測（焦點在密碼輸入框上那
      一刻整幀不擷取）要真的 UIA 樹才跑得起來，Linux CI 上造不出來。上面那支測試蓋到的
      是**標題規則**那一半。UIA 那一半在 alpha.3 的真 Windows 上手驗過（見 PRIVACY.md
      「密碼欄偵測只涵蓋瀏覽器」）。

**明確不做**：任何 LLM 呼叫、任何 UI、macOS/Linux。

**官方維護承諾範圍**〔定案〕：1 個平台（Windows）、1 條訊號路徑（通用 OS API）、
1 條操作路徑。其餘一律 adapter/plugin 化，等 P8 交給社群。

---

## Phase 1 — Sister 1.0：檔案櫃 + 搜尋框 + 字母人

**目標**：那個「這禮拜就能做完、做完當天就會每天用」的產品。被動答題，
100% 本機可跑。**Phase 末 repo 轉 public（alpha 標示，不宣傳）。**

**Scope**
- Tauri 2 shell：pet window（TokenMonster 配方：transparent/pin/dragbar/close→tray）、
  字母人 letter-avatar（day-one 識別）、`idle/paused/thinking` 三狀態。
- 對話面板：輸入框 → 檢索 →（選配）一次 LLM 潤句 → 附出處 chips（點開看當時畫面）。
  離線/無 key 模式 = 結構化結果列表，功能完整。
- 時間軸瀏覽器 v0：按天捲動、縮圖 + OCR 摘錄、框選區間刪除（cascade）。
- 三張同意書 onboarding（SPEC §11.1）+ 設定頁（blocklist 編輯、TTL、pause 快捷鍵）。
- ~~BYOK（secret-vault/OS keychain）+ Ollama 偵測——潤句用。~~
  **2026-08-21 移出 Phase 1。** 這一項排在這裡是基於一個錯的假設：以為「本機優先」
  代表連模型也要在本機。負責人講清楚了——**在本機的是截圖，語言模型當然在雲端**。
  而且第一批使用者手上已經有 claude code / codex / grok / gemini cli 了，本地模型
  （Ollama）是之後的事。所以 L2/L3 那個腦要接的第一個東西不是 HTTP client，是
  **使用者已經裝好、已經登入、已經在付錢的那支 CLI**。
  這也順帶解掉了原本以為存在的衝突：走 CLI 就不需要把 HTTP client 拉進相依樹，
  `check-no-network.sh` 不用開例外。（真正要重寫措辭的是別的地方：出去的是 OCR
  抽出來的**字**，不是畫面——那是同意書 2 的事，等 L2/L3 開工再一起講清楚。）
- Query log 開始累積（本機）：每次提問 + 點擊了哪個出處 = 未來題庫。再加一個
  他自己按的位元（`sister mark` / 答案底下那顆「這件事我本來已經忘了」）——那是
  底下第一條退場條件唯一的量法，理由寫在那裡。
- 刪除與匯出的**終端機那一半**（原本只有字母人上的時間軸有）：`sister forget
  --last 2h`（兩段式，預設只看不刪）、`sister export --to <目錄>`。後者本來排在
  SPEC §11.8，提前做是因為它修的是一個已經在騙人的句子——PRIVACY.md 寫著
  「整份記憶就是一個 `sister.db` 檔」，而在 WAL 模式下、她還在錄的時候，照那句
  話複製出來的備份會安靜地少掉最後一段。`doctor` 也一直在叫人跑一個不存在的
  `sister forget`。承諾要對得上做出來的東西，這兩條都不能等。

**Exit criteria**
- [x] ~~**第一週魔法時刻**：自用 7 天內 ≥ 3 次「答對我自己都忘掉的東西」（記錄實例）。~~
      **2026-08-21 撤掉：這條不再是退場條件。** 專案負責人的話：「那個指標不是我訂的，
      我覺得沒意思。」他說得對——這是寫這份 roadmap 的時候自己加的一條，不在原本的
      定義裡。而它的毛病底下自己就寫出來了：「哪七天」沒有客觀答案、「≥ 3 次」的 3
      是憑空的，一條要靠人主觀判斷才過得了的閘門，擋不住任何東西，只會讓進度卡在一個
      沒人相信的數字前面。
      **`sister mark` 那顆按鈕留著**，它不是為了這條退場條件而活的：題庫上「★ 魔法
      時刻：N 次」、`--marked`、`--json` 的 `marked_instances` 都是使用者自己讀得到、
      按得到、收得回的東西，不是只寫不讀的欄位（那是這個 repo 一直在抓的 #22 反模式）。
      底下那幾段講的是它為什麼要是**新的一個位元**，那個理由和退場條件無關，所以留著。

      〔以下是撤掉之前的原文，留著是因為 `ever_marked` 那幾行還在解釋現行行為〕
      量法：她答對一件你早就忘掉的事的那一刻，
      按一下——`sister mark`（終端機）或答案底下那顆「這件事我本來已經忘了」
      （字母人）。`sister queries` 上多一行「★ 魔法時刻：N 次」，
      `--marked` 列出是哪幾題，`--json` 的 `marked_instances` 給腳本。
      這一格**不由程式判斷退場條件過了沒**：「哪七天」沒有客觀答案，而一個會
      自己宣布 ✓ 的工具只是把印象換成一個長得像數字的印象。實例帶著兩個時間
      （她答對的、你認出來的）攤在那裡，那一格由人去讀。
      為什麼要新開一個位元而不是拿現成的：題庫記得住你問了什麼、她給了幾筆、
      你點開了哪個出處——**記不住你當時知不知道那個答案**。而「點開出處」離這
      條退場條件只有一行遠，卻剛好在講反過來的事：那件事最常發生在她答錯、
      或你在查核的時候。
      這一格也是唯一補不回來的：那是你看到答案那一刻腦袋裡的狀態，一個禮拜後
      翻題庫翻不出來。和上面「零當機」那條同一種病——「那是印象，不是條件」。
      正因為補不回來，「0 次」不准只有一種讀法：**還沒開始按**和**按過、現在
      一格都不剩**是兩件相反的事，後者代表那七天的證據掉了。分它們的是 `meta`
      裡的 `ever_marked`（DATA_INVENTORY.md 那一節），`--json` 也出這一格。
      但它只答得出「按過」，答不出「後來是誰拿掉的」——自己收回和被 `forget`
      帶走在磁碟上長得逐字相同，所以那一行把兩種都攤開、一種都不選；真的被
      帶走那次由 `forget` 自己當場報數（它先數再刪，因為 CASCADE 的列不算在
      回傳值裡）。
      題目**整批**被帶走的時候走的是另一句話：★ 那一行是題庫表頭的一部分，
      題庫空了就輪不到它，而那剛好是這條退場條件最慘的一種空。所以「題庫是
      空的」那句話自己也吃 `ever_marked`——它是 1 就代表他問過（標記只掛得上
      真的問過的題目），「可能是還沒問過她任何問題」當場被砍掉。
- [ ] 檢索 < 100ms、成句 < 3s（> 3s 視為 bug）。
      量法：`sister queries` 那一行延遲（中位數／p95／超過門檻的題數）。數字
      來自**真實提問**的題庫，不是另外跑一份 benchmark——那條路只會量到我
      挑出來的問題。成句目前沒有東西可量（還沒有任何 LLM 路徑）。
- [ ] 全離線模式（同意書 2 全關）走完全部主流程。
      終端機那一半走完了（乾淨資料目錄 → `consent` 三張全空 → `record` 被擋掉
      並且回非零 → 簽第一張 → `replay` → `query`／`facts`／`queries`／`stats` →
      `prune --dry-run` → `pause` → `doctor` → `resume`）。走的過程抓到兩個
      「兩行各自正確、擺在一起在騙人」的缺口：doctor 的保留畫面檔、stats 的
      畫面張數對畫面檔大小。字母人那幾頁（同意書、設定、時間軸兩段式刪除）在
      `?demo=1` 的無頭瀏覽器裡驗過版面與狀態轉換。
      **還缺的是真 Tauri 視窗上的那一遍**——那要在 Windows 上做，見驗收清單。
      另外「離線」在這裡不是一個模式而是唯一的狀態：程式裡沒有連外路徑，
      `check-no-network.sh` 每次 push 都在證明這件事。
- [x] clone → 跑起來 < 10 分鐘（含 README quickstart 實測）。
      **實測 33 秒**（乾淨 `CARGO_HOME`：clone → 抓 108 MB 相依 → release build
      32 秒 → replay → 第一個 ★ 答案。16 核開發機；runner 約 2.1 倍）。這一條
      唯一的難處不是第一次量得到，是三個月後它還是對的——所以 CI 每次 push 會
      把 README 那個 code block **挖出來執行**（`scripts/check-readme-quickstart.sh`），
      而不是另外維護一份一樣的指令。跑不起來、或跑完沒有那個 ★ 答案，就是紅的。
- [ ] repo public、Apache-2.0、README 首段 = 三張同意書宣言 + 實測足跡數字。
      repo public 與 Apache-2.0 已成立，README 首段也已公開三張同意書各自「沒簽會怎樣、
      簽了能碰什麼」。CPU／RAM 的真機現況已記入 README：alpha.46 活躍寫程式
      60 秒是 44.0%／73.7MB，CPU 已由 Ted 接受為 Phase 0 基準，RAM 也通過。
      足跡還差磁碟：alpha.47 已證明舊的 4.3GB/天把可重用 WAL 當永久成長；
      main 已排除 WAL 錯帳；容量仍高於長期目標，依 Ted 決定延後到功能與體驗
      完成後再優化，不擋下一個 milestone。

**明確不做**：主動開口（她只回答，不先說話）、interpreter、承諾表。

---

## Phase 2 — 重播評測 harness（全場唯一無異議的下一步）

**目標**：把所有還在吵的架構問題變成可以跑的數字。這也是開源後最值錢的公開資產。

**Scope**
- `sister replay export`：把指定時間範圍的真實 L0 流打包成私有 **Draft**。匯出時
  自動去敏，不帶任何截圖或來源資料／圖片路徑；但自動去敏不會把 Draft 變成可分享
  的東西。只有人工逐項審查、把 `review` 從 `draft` 標成 `reviewed` 的 **Reviewed**
  corpus 才可分享。
- `sister replay import`：Draft 和 Reviewed 都可在本機匯入、驗證；從去敏後的 L0
  重建全文索引與 L1 事實，不相信匯出機器裡已算好的衍生結果。
- 現有 `sister replay <scenario.json>` 保留；腳本化埋題和真實工作日走同一條重播管線。
- 語料庫：Ted 真實工作日 ≥ 2 週 + 腳本化埋題日（植入帳單/電話/通知/中途改目標）。
- 題庫三源：query log 轉標註、手標 recall QA、承諾集與開口判斷集（該講/不該講時刻）。
- [x] Runner v0：`sister replay evaluate <corpus> <questions>` 在同一份 corpus 上跑
  `baseline_text` 與 `facts`。前者是產品現有的三份 FTS5 索引加必要的有界 LIKE
  fallback；後者再加 L1 typed facts，且 fact 排在文字結果前。現在輸出找回率@k、
  答案正確率、出處正確率、延遲與兩條路徑確定為 0 的模型呼叫／成本；沒有分母的
  rate 與尚未量到的指標明確輸出 `null`，不拿 0 冒充。
- 後續 runner 配置：+interpreter / +reviewer；提醒誤報／漏報、斷句 F1、Reviewer
  回查率及 CPU／RAM／電池／磁碟要等各自有真的量測來源再填。
- repo 內建 3 個純合成事件、5 題 QA 的 Reviewed fixture，只用來驗 runner 接線與
  報告形狀；它不是下方 ≥100 題的公開 baseline，也不是代表性品質數字。
- 指標面板（開發者模式頁）+ README benchmark 表自動生成。

**Exit criteria**
- [ ] baseline 數字公開進 README（含成本與足跡）。
- [ ] 題庫 ≥ 100 題 recall QA（其中 ≥ 30 題來自真實 query log）。
- [ ] 任何後續 phase 的合入條件從此以 harness 回歸為準（regression gate）。

---

## Phase 3 — 斷句 + 事實層強化（核心演算法）

**目標**：把一整天無標點的流切成「日後問得到」的段落。純程式，仍然零 LLM。

**Scope**
- Segmenter v1（SPEC §4）：切刀/黏合/工作集偵測/強制上限/重疊 margin。
- 時間軸升級：「你今天的一天」章節視圖；使用者手動合併/切開段落
  （每次操作 = 斷句訓練訊號）。
- 卡住偵測 v0（純訊號：停留 + 反覆切換 + error 事實共現）——先只記錄不開口。
- 手標邊界語料 ≥ 5 個工作日；斷句 F1 進 harness。

**Exit criteria**
- [ ] 斷句邊界 F1 ≥ 0.75（對手標語料；達不到就修訊號，不上 LLM 救）。
- [ ] 「我昨天下午在弄什麼」可用 session 章節 + facts 回答（仍是檢索式，無生成推理）。
- [ ] 找回率相對 Phase 2 baseline 提升可量測（+facts+session 配置）。

---

## Phase 4 — 理解與記憶（大腦上線）

**目標**：L2/L3 上線——但 interpreter 要在評測上贏了才准常開。

**Scope**
- Interpreter 薄層（SPEC §5）：事件驅動喚醒、每日預算 80、strict JSON 卡片、
  worker pool（預設 4）。
- Reviewer（SPEC §6）：15–30min 批次 + 日終盤點、typed card merge、
  五類強制回查、雙 pass 分歧警報、回查率 log。
- L3：commitments/entities/day_summaries + provenance cascade delete。
- 記憶瀏覽器：「她現在認為我在幹嘛、根據什麼」即時視圖 + 當場更正；
  承諾表 UI（結案/其他 兩鍵）。
- 去識別化管線（SPEC §11.3）+ 外送紀錄面板（送了什麼給誰）。
- 模型接入雙軌：BYOK cheap tier + 訂閱登入（MAT signin.ts 移植）。
- **A/B gate**：harness 跑 `+interpreter` vs 不跑——照〔定案〕沒贏就保持預設關。

**Exit criteria**
- [ ] `+interpreter+reviewer` 在題庫上答案正確率顯著 > baseline（門檻：+10pt 以上），
      且誤承諾率（幽靈承諾/killed-by-user 比例）< 20%。
- [ ] 回查率公開量測（Reviewer 實際翻原件的比例）。
- [ ] 成本實測 ≤ SPEC §13 預算（預設檔位 < US$15/月換算）。
- [ ] 刪除 cascade 驗證：刪一段 L0，衍生 L2/L3 全部 tombstone（自動化測試）。
- [ ] 兩週自用：日摘要可讀、承諾表 ≥ 70% 是真的（其餘可一鍵結案不煩人）。

**明確不做**：她仍然不主動開口。大腦是沉默的。

---

## Phase 5 — 開口（守門員）+ macOS + 正式發布

**目標**：解鎖主動性——用預算和證據門檻，不用熱情。帶著 benchmark 數字正式見人。

**Scope**
- Gatekeeper（SPEC §8.3）：五類候選白名單、評分、預算 ≤ 5/天、冷卻、quiet hours、
  專注模式靜音；冷啟動只開 a（顯式時間承諾）/ b（未注意的通知）兩類。
- 第一句話規則（SPEC §8.4）強制執行。
- 形式階梯：微光 → 一行字 → 建議卡；每次開口的分數/依據/反應全記錄。
- 回饋兩鍵接通記憶死亡規則；「順帶確認」機制（螢幕外完成問題）。
- 日終「要不要做筆記」offer + 日摘要成品化。
- **macOS port**：ScreenCaptureKit + Vision OCR + AX API + TCC/紫點 UX 文案。
- 發布工程：安裝包、簽章、自動更新、官網一頁、Show HN / X 發文帶 benchmark 表。

**Exit criteria**
- [ ] 自用 ≥ 2 週：開口 ≤ 5/天、有用率（未被按掉/被感謝/被採納）≥ 60%、
      誤報導致的「錯誤事實開口」= 0（開口內容全部有證據 ref）。
- [ ] a/b 類開口的可驗證正確率 ≥ 90%（這兩類使用者能立刻驗證）。
- [ ] macOS 走完 P0/P1 全部 exit criteria（足跡預算含電池項）。
- [ ] 發布物料：README benchmark 表 + 隱私宣言 + 3 分鐘 demo（回憶秒答 + 一次克制的開口）。
- [ ] **正式發布的兩個條件**〔定案，Grok〕：(1) 你自己願意整天開著它；
      (2) 一個陌生人 10 分鐘內裝得起來。兩者皆真才按下發布鍵。
- [ ] 發布後 2 週：修 issue 節奏可持續（單人 + agent fleet 撐得住的量）。

---

## Phase 6 — 手 v1（sister-hands sidecar）

**目標**：從「說」到「做」的第一步，可逆操作 + 逐步核准。物理隔離的 sidecar。

**Scope**
- hands sidecar（Node）：MAT adapter/redaction/managed runtime 移植。
- `suggest` 級：開 URL/檔案/聚焦視窗（按鈕觸發）——接通承諾卡的
  `allowed_next_step`（「要我幫你把視窗點開嗎」）。
- `semi-action` 級：結構化 grant（task/apps/actions/expiry）、逐步核准
  （對話「好」= 只核准顯示的那一步）、每步截圖驗證、abort 快捷鍵、action log。
- 來源防線：L0 內容 data-block 包裹；外部內容要求動作 → 強制人工核准
  （injection 測試套件：在網頁/訊息裡埋指令，驗證 0 執行）。

**Exit criteria**
- [ ] Injection 套件 100% 攔截（埋 20 種指令變體）。
- [ ] 不可逆動作（送出/付款/刪除類）在任何路徑都會停下要求即時核准（自動化驗證）。
- [ ] 10 個真實 semi-action 任務自用成功，action log 可回放。

---

## Phase 7 — 接手模式（bounded takeover）

**目標**：Ted 的真實場景——「你要出門了嗎？要不要我接著把 milestone 推進下去，
程式留在 staging。」（對使用者永遠叫「接手」，不叫 Autopilot——名字已被辯論封印。）

**Scope**
- 白名單任務型別 #1：**監督 CLI coding agents**（claude/codex 跑長任務：盯進度、
  卡住回報、按既有 spec 推 milestone、禁 deploy）——MAT engine/steering 重用。
- 白名單 #2：文件整理類（下載檔案、歸檔、筆記）。
- 預檢（fresh-evidence check）、scope 描述、停止條件、步數上限、全程 action log、
  即時中止；離開偵測 → 交接 offer 流程。
- 每次 autopilot run 產出「她做了什麼」報告（含每步截圖）。

**Exit criteria**
- [ ] 20 次監督式 autopilot run，0 次越界（碰到白名單外動作即停）。
- [ ] 交接 offer → 執行 → 回報的完整迴路自用 ≥ 5 次成功。

---

## Phase 8 — 生態（持續）

- Plugin/adapter 介面：per-app 訊號 adapter（Photoshop 類自繪 UI）交給社群；
  Everywhere MCP interop；瀏覽器 extension（更準的 URL/DOM）。
- Linux（X11 先、Wayland portal 後）。
- 語音 TTS（opt-in、僅 a 類開口）；四姊妹立繪/語音資產接入完成度打磨。
- 選配 E2EE 多機同步（獨立審視授權與架構）。
- 社群治理：issue 模板按「訊號抓不到」分流到 adapter plugin、
  benchmark 語料貢獻管道（去敏審查 gate）。

---

## 跨階段紀律

1. **每個 phase 合入 = harness 回歸不退步**（P2 起）。
2. **長期足跡預算原則上是 release blocker**；目前 Ted 已明確接受「先完成功能與
   體驗，再優化容量」的例外，實測與取捨已寫在 Phase 0。數字繼續照實公開，
   不能暗改門檻，但也不能拿它擋功能 milestone。
3. **隱私文件與功能同 PR**：動到訊號面的 PR 必須同步改 DATA_INVENTORY.md。
4. **她的每一句話都有出處**——從第一次開口到 autopilot 報告，無一例外。
5. **任何「缺一不可」的說法出現時，回去讀 PRODUCT §3 第 8 條。**

## 風險登記簿

| 風險 | 對策 | 相關 phase |
|---|---|---|
| 第一週沒有魔法時刻 → 棄用 | P1 exit 硬門檻；facts 層保證「帳單/電話」類必中 | P1 |
| 主動開口變成煩人 → 第三天被關 | 預算制 + 冷啟動只開高 precision 類 + 有用率 gate | P5 |
| 記憶歪掉长期化 | 五類強制回查 + 分歧警報 + 兩鍵結案 + cascade 刪除 | P4 |
| 單人維護死（per-app 地獄） | v1 只用通用 API；per-app 全部推到 P8 plugin | 全程 |
| 資源足跡超標 → 秒刪 | 數字照實公開；長期仍要回到預算，但目前容量依 Ted 已定例外不擋功能 milestone | P0 起 |
| Injection 經由螢幕內容 | data-block 紀律 + 動作強制核准 + 測試套件 | P6 |
| 模型商政策/價格變動 | 雙軌接入 + 本地降級鏈 + 角色→模型可配置 | P4 起 |
| 巨頭入場 24/7 記憶 | 我們的位子是「可驗證 + 中立 + 可帶走」；見 PRODUCT §6 | — |
