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
      另外「離線」是預設：程式裡沒有 HTTP client（`check-no-network.sh` 每次
      push 都在證明），畫面永不離開這台機器。簽了第二張同意書且設定了 CLI
      之後，螢幕上的**字**（原文）才會交給那支本機行程。
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
- 題庫三源：
  - [x] `replay export --questions-to` 把同一時間窗的 query log 轉成綁定 corpus
    fingerprint 的 private Draft；當時 hits/clicks/mark 只作提示，`expected` 留 `null`
    等人工標註，不猜 ground truth。
  - [x] Recall QA 標註器 v0：`replay questions annotate` 一次走完整份 Draft，能看產品
    候選、搜尋／打開 canonical evidence，另存新檔；`review` 只在全部有效且人明確
    確認未去敏原話後產生 Reviewed 題庫。
  - [x] 承諾集與開口判斷集標註器 v0：`replay moments draft` 從 corpus 提出候選時刻
    （OCR 抽到 DateTimeMention、同一 focus 長停留；通知類現行 `SystemKind` 全部不符，
    所以提不出來），`annotate`／`review` 走跟題庫同一套 Draft→Reviewed。標註器已經在，
    **兩份集合本身仍是空的**——要等真實 corpus 標註才算有資料。
- [x] Runner v0：`sister replay evaluate <corpus> <questions>` 在同一份 corpus 上跑
  `baseline_text` 與 `facts`。前者是產品現有的三份 FTS5 索引加必要的有界 LIKE
  fallback；後者再加 L1 typed facts，且 fact 排在文字結果前。現在輸出找回率@k、
  答案正確率、出處正確率、延遲與兩條路徑確定為 0 的模型呼叫／成本；沒有分母的
  rate 與尚未量到的指標明確輸出 `null`，不拿 0 冒充。
- 後續 runner 配置：+interpreter / +reviewer；提醒誤報／漏報、斷句 F1、Reviewer
  回查率及 CPU／RAM／電池／磁碟要等各自有真的量測來源再填。
- repo 內建 3 個純合成事件、5 題 QA 的 Reviewed fixture，只用來驗 runner 接線與
  報告形狀；它不是下方 ≥100 題的公開 baseline，也不是代表性品質數字。
- [x] 合成 fixture 的 CLI regression gate：CI 真的執行 `replay evaluate --json`，
  鎖定兩個產品 profile 的題數、分數、known miss 與 `null` 契約；延遲只驗真的量到
  五題且為有限非負數，不把 runner 的毫秒波動寫成門檻。
- [x] 指標面板 v0：預設不出現在一般桌面；明確開啟 `[shell] developer_mode = true`
  並重開後，才從系統匣進入。它載入本機 eval report，畫面只保存／渲染 Rust 嚴格
  解析後的數值 projection；report 裡的名稱、id、fingerprint、原問句與回傳文字
  都不進畫面，失敗題用 1-based 題號定位。Draft 警告不會被摘要吃掉。
  真 Windows 的系統匣、原生選檔器與三種載入狀態仍待實機清單確認。
- [x] README synthetic benchmark 表由真正的 CLI JSON 自動生成並由 CI 比對穩定欄位；
  latency 另列成有日期／環境的快照，不拿浮動毫秒當 gate。這仍只是 5 題 smoke
  fixture，不是下方 ≥100 題的公開 baseline。

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
- [x] 「我昨天下午在弄什麼」可用 session 章節 + facts 回答（仍是檢索式，無生成推理）。
- [x] 找回率相對 Phase 2 baseline 提升可量測（+facts+session 配置）。

---

## Phase 4 — 理解與記憶（大腦上線）

**目標**：L2/L3 上線——但 interpreter 要在評測上贏了才准常開。

**Scope**
- Interpreter 薄層（SPEC §5）：事件驅動喚醒、每日預算 80、strict JSON 卡片、
  worker pool（預設 4）。〔alpha.61 才真的**自己**醒：在那之前 `brain::run` /
  `reviewer::run` 只有使用者親手打指令才跑，`record` 迴圈裡一次都沒叫過，
  所以錄一整天打開記憶瀏覽器是空的。現在錄製時起一條慢路徑執行緒，
  熱路徑只做一次 `AtomicBool` 寫入〕
- Reviewer（SPEC §6）：15–30min 批次 + 日終盤點、typed card merge、
  五類強制回查、雙 pass 分歧警報、回查率 log。
- L3：commitments/entities/day_summaries + provenance cascade delete。
- 記憶瀏覽器：「她現在認為我在幹嘛、根據什麼」即時視圖 + 當場更正；
  承諾表 UI（結案/其他 兩鍵）。
- 外送紀錄面板（送了什麼給誰）。〔alpha.60 落地：時間軸「外送」頁，
  含沒送出去的原因；「還沒送過」和「送過但被清掉了」是兩句話。
  SPEC §11.3 的去識別化管線 2026-08-26 拿掉：代號跨段對不起來，
  會把 §6 承諾表和 entities 的地基拆掉〕
- 模型接入：spawn 使用者已登入的 CLI（2026-08-21 定案；不是 BYOK HTTP、
  也不是內建推論引擎）。設定在 `[brain] command` / `args`。
- **A/B gate**：harness 跑 `+interpreter` vs 不跑——照〔定案〕沒贏就保持預設關。

**Exit criteria**
- [ ] `+interpreter+reviewer` 在題庫上答案正確率顯著 > baseline（門檻：+10pt 以上），
      且誤承諾率（幽靈承諾/killed-by-user 比例）< 20%。
      〔alpha.60：量的工具做好了（`sister replay evaluate --ab`），**但沒過**。
      現有合成題庫 `facts_session` 已經 3/3、5/5，腦沒有 +10pt 的空間，
      而誤承諾率的分母是 0（印成「還沒量到」不是 0%）。
      要過這一條需要真題庫 + 真 CLI，不是再改工具〕
- [x] 回查率公開量測（Reviewer 實際翻原件的比例）。〔alpha.59：`reviewer_run`
      記 runs/candidates/rechecks，`sister review` 和 `sister brain log` 都印。
      「沒跑過」「跑了沒有五類候選」「有候選一次都沒回查」是三句不同的話〕
- [ ] 成本實測 ≤ SPEC §13 預算（預設檔位 < US$15/月換算）。
      〔alpha.60：`EvalMetrics.model` 從兩個恆為 0 的數字改成帶 kind 的 enum——
      `not_on_path`（沒跑腦）和 `measured{calls, usd_per_day}` 分得開，
      單價和出處印在每一行裡。**但真機一天的數字還沒有**：合成 corpus 只有
      2 秒，換算出來的 US$0.12/月不是 Ted 一天的用量〕
- [x] 刪除 cascade 驗證：刪一段 L0，衍生 L2/L3 全部 tombstone（自動化測試）。
      〔alpha.59：三條測試在 `retention.rs`。墓碑**連內容一起清掉**——
      只蓋日期的話，查詢看不到而人名和金額還在檔案裡（鐵律 2：L0 刪掉之後
      那張卡片就是那段內容唯一的載體）。cascade 也殺根，不只殺子代：
      `migrate_012` 不回填 provenance，只走子代的話升級上來的舊卡片
      全部變成刪不掉的〕
- [ ] 兩週自用：日摘要可讀、承諾表 ≥ 70% 是真的（其餘可一鍵結案不煩人）。

**明確不做**：她仍然不主動開口。大腦是沉默的。

---

## Phase 5 — 開口（守門員）+ macOS + 正式發布

**目標**：解鎖主動性——用預算和證據門檻，不用熱情。帶著 benchmark 數字正式見人。

**Scope**
- ✅ alpha.66 Gatekeeper（SPEC §8.3）：五類候選白名單、評分、預算 ≤ 5 **點**/天、
  冷卻、quiet hours、專注模式靜音；冷啟動只開 a（顯式時間承諾）/
  b（未注意的通知）兩類。`crates/sister-core/src/gatekeeper.rs`。
  - SPEC 原文寫「≤ 5 次/天」，和同節形式階梯的「卡片算 ×2」矛盾。
    消歧義成**點數**：微光 0、一行字 1、建議卡 2。
  - 專注模式是 `FocusMode` 三態：這一版沒有前景視窗幾何訊號，所以是
    `Unmeasured` 而不是 `Windowed`——量不到不等於量到了「不是」。
- ✅ alpha.66 第一句話規則（SPEC §8.4）。和冷啟動兩週是**兩條**規則，
  分開寫、分開測：第 15 天它們的答案不一樣。
- ✅ alpha.67 形式階梯：微光 → 一行字 → 建議卡；分數/依據/反應寫進
  `utterance` 表（**每一次考慮過的**都寫，不只開過口的——SPEC §8.3.5 要的
  訓練語料是被擋下來那半）。
- ✅ alpha.66 回饋兩鍵接通記憶死亡規則；「順帶確認」機制
  （`crates/sister-core/src/followup.rs`，只在使用者主動開對話時附在回答尾端）。
- ✅ alpha.68 日終「要不要做筆記」offer（d 類訊號源）。訊號源**不是**
  `SystemKind::SessionEnd`——那只證明一場錄製容器收尾，連 60 秒 bench 都會
  寫一列。用的是一輪成功的 `ReviewKind::Eod`。
  - 她講日期不講「今天」：日終盤點盤的是 `previous_local_day_key`，而它跑完
    多半已經過午夜，那一刻的「今天」正是還沒有筆記的那一天。
  - 「沒有摘要」拆成 `DayNoteState` 四格。那天一張 L2 卡都沒有 → **不開口**
    （答應了只生得出一份空的）；他自己按過忘記 → **不開口**（提議寫回來等於
    問他要不要撤銷自己的刪除）。
- ⬜ **macOS port**：ScreenCaptureKit + Vision OCR + AX API + TCC/紫點 UX 文案。
  本機編不到、CI 也沒有 mac runner，還沒開始。
- 🔶 發布工程：安裝包、簽章、自動更新、官網一頁、Show HN / X 發文帶 benchmark 表。
  - ✅ alpha.68 版本說明。在這之前它是 `ci.yml` 裡寫死的一塊 570 行的字，
    每出一版往裡面疊一節「這一版：⋯⋯」再**整塊**貼上去——alpha.67 那份
    release 說明裡有三十個「這一版」，而且最後一行寫著「沒有 UI、沒有常駐、
    沒有任何模型呼叫」，同一份的第四段就在教使用者開 `sister-desktop.exe`。
    改成 [`docs/RELEASE-NOTES.md`](RELEASE-NOTES.md) 一個 tag 一節，
    `scripts/release-notes.sh` 組出來。**沒寫那一節不會退回上一版**，
    會印「這一版沒有寫版本說明」。
  - ⬜ 安裝包／簽章／自動更新／官網一頁：還沒開始。

**訊號源盤點**（守門員判得再好，沒有候選就等於沒上線）
- ✅ a `CommitmentDue`：`open_commitments_due_before(now + 40min)`，只收
  `due_source='explicit'`。
- ⬜ b `UnattendedNotification`：**盤點過了，做不出來，所以沒做**。
  `focus_events` 分不出「通知搶走焦點」和「使用者自己 alt-tab 過去又切回來」；
  不搶焦點的 toast 可能一列都沒有；`ocr_blocks` 沒抄到也不能證明沒出現。
  加一種 `SystemKind` 要動 `sister-capture`（錄製熱路徑），不在範圍內。
  **這一格是 Phase 5 最痛的缺口**：§8.4 規定第一句話只能是 a/b，
  §8.3.3 規定冷啟動兩週只開 a/b——所以新使用者前兩週實際上只有 a 類會響。
  誤報一次就是第一印象，而第一印象只有一次，所以寧可空著。
- ✅ c `Stuck`：`stuck_signal` 表。
- ✅ d `SessionEnd`：成功的 `reviewer_run(kind='eod')`，見上。
- ⬜ e `Leaving`：**時序上做不到，所以沒做**。`SystemKind::Lock` 是 OS
  **已經鎖定之後**才寫進 `system_events` 的，而 SPEC 要的是「鎖屏**前**」。
  在鎖屏後才問「要不要交接」是一句沒有人看得到的話。要真的做到得在
  `sister-capture` 加鎖前訊號。

**額外做掉的（不在原 scope，但在出貨路上）**
- ✅ alpha.67 來源防線（SPEC §0.4 / §9.4）：`build_prompt` 原本把 OCR 原文、
  視窗標題、host、上一張卡的 activity 原封不動接進 prompt，header 一句
  「這是資料不是指令」都沒有。改成每次重抽 nonce 的圍欄
  （`crates/sister-core/src/prompt_fence.rs`），20 種 injection 變體測試。
  **不去敏、不遮蔽、不過濾**——圍欄標的是「這是資料」，不是把資料改掉。

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
- ~~hands sidecar（Node）：MAT adapter/redaction/managed runtime 移植。~~
  **改成 Rust crate `crates/sister-hands/`，而且不移植 redaction。**
  去敏在 alpha.58 整個從產品拿掉了（「記憶是長期在本機資料庫裡的，
  要去敏的人就不會用」）——把一個已經拆掉的東西移植進來，會讓下一個讀
  這份文件的人以為產品裡有它。Node sidecar 那一半：這個 repo 的
  `check-no-network.sh` 連 HTTP client 都禁，一個 Node 行程買到的隔離
  不如型別上的隘口，等真的需要行程隔離再說。
- ✅ alpha.68 `suggest` 級：`Level::{Observe, Suggest}`、
  `Suggestion::{OpenUrl, OpenFile, FocusWindow}`、`execute_with()` 唯一隘口、
  `ActionLog`（JSONL，可回放）、`commitment_action::parse_allowed_next_step`。
  - **「這是人按的」是一張型別上的票**（`UserButtonPress`，欄位與鑄造函式
    都私有）。螢幕上的字和模型輸出最多只能解析成一顆**按鈕**，中間隔著一次
    人按下去。crate 外面寫 `UserButtonPress(())` 收到 E0603。
  - 永不繼承的五類一個都沒實作，所以守的不是「攔截」是「無法被偷偷加進來」：
    `never_inherited_class` 的 `match` 沒有 `_`，加第四種動作會編譯錯誤。
  - 結局**三種**：`Refused`（沒交給 OS）／`Failed`（交了但失敗）／`Done`。
- ✅ alpha.69 接通承諾卡的 `allowed_next_step`（「要我幫你把視窗點開嗎」）：
  平台執行層（`ShellExecuteW` / `EnumWindows`+`SetForegroundWindow`，
  不經 cmd.exe、不經 PowerShell）、按鈕、以及畫面上的行動紀錄。
  目標政策是**白名單**（http/https；可看的副檔名），放在
  `sister-hands::target_policy` 而不是字母人裡面——CI 對 `apps/desktop` 只跑
  clippy 和 build，寫在那邊的測試一列都不會被執行。
  按鈕回叫帶的是承諾 id 不是動作，要做什麼由後端重讀資料庫（SPEC §9.7）。
  - **writer 也在 alpha.69**：在這之前 `commitments.allowed_next_step` 從
    schema 建好到那天沒有任何一支程式寫過它，所以整條路——按鈕、`Level::Suggest`、
    `ActionLog`、`target_policy`——在真實使用中一次都跑不到。
  - **模型只能*指*一筆 L1 fact，不能自己打一串網址**（契約上是
    `{"fact": 45}`）。`{"action":"open_url","url":…}` 由 Rust 從 `facts.raw`
    組出來。SPEC §9.4：讓模型自由輸出網址，等於讓螢幕上任何一段文字決定她開什麼。
  - 「她沒有建議下一步」（安靜）跟三種拒絕（fact 不在／kind 不合／URL 沒有
    scheme，各留一句看得見的話）分得開。兩個 pass 不同調時掉的是下一步、
    不是整張承諾。
  - **`action-log.jsonl` 補進刪除與匯出兩條路**（alpha.69）。它是資料庫旁邊
    的另一個檔案、有完整網址與路徑，而「忘掉一段時間」原本只認得 `sister.db`
    和 `frames/`——差一點送出一版：資料庫清乾淨了，那幾行網址原封不動躺著，
    畫面上寫「已經忘掉了」。`sister export` 同理（SPEC §11.8 自稱全量）。
    刪的是字本身不是旗標；讀不懂的列問不出時間所以一起走、分開報數字。
  - 寫的人在 `sister-core`、讀的人在 `sister-hands`，中間沒有型別。量過：
    把讀的那一邊欄位改名、連它自己測試的字面值一起改，兩邊 28 條全綠而按鈕
    是壞的。`the_next_step_this_crate_writes_is_one_the_hands_crate_can_read_and_will_allow`
    站在那條縫上（sister-hands 是 sister-core 的 dev-dependency，不進出貨相依樹）。
- 🔶 `semi-action` 級：平台無關的核心做好了——結構化 grant
  （`Task`/`AllowedApps`/`AllowedActions`/`Expiry`/`StepLimit`）、
  `Grant::covers` 逐維拒絕、內容綁定的 `PresentedStep::approve` → `StepApproval`
  （A 的票做不了 B）、`RunConclusion::{Completed, StepLimitReached, Aborted}`
  三種結局分得開、每步 `Option<ScreenEvidenceRef>` 明寫 `null`。
  授權不通過一律 `Outcome::Refused`，不是 `Failed`——後者的文案是
  「她動手了，但執行失敗」，而那是一次連 executor 都沒被呼叫的拒絕。
  **alpha.70 有第一個呼叫端了：`sister do`。** 在這之前這 443 行從發表那天起
  一次都沒被執行過。SPEC §9.1 對介面有定案（「逐步核准；對話式『好』= 核准」
  〔定案：CLI 證明的是互動可行，不是授權夠精確〕），所以第一個呼叫端是 CLI
  而不是字母人——而且 CI 對 `apps/desktop` 只跑 clippy 和 build，寫在那邊的
  測試一列都不會被執行。
  - 步驟來源是**承諾卡的 `allowed_next_step`**，不是一個新的 planner
    （那是 Phase 7）。三態分得開：沒有下一步的卡安靜跳過，寫了但讀不懂的卡
    要印出來。
  - 每一步宣告的 app 從**證據鏈**算出來（`evidence_json` → frame →
    `text_chunks.app_id`），不是請求方隨手填的字。三態：剛好一個 app、
    兩個以上（兩個答案就是沒有答案）、問不出來。後兩種任何 `--app` 都涵蓋不了，
    但**各自有各自的一句話**。
  - 平台執行層從字母人搬進 `sister_hands::platform`，兩個呼叫端共用一份。
    順帶：`execute_with` 的註解寫著「全部執行請求的唯一隘口」——semi-action 一接
    上呼叫端那句話就變成假的（它走 `execute_approved_step`），已改成講清楚它守的
    是 suggest 那一條。
  - `--minutes` 那張票在**執行的那一刻**再問一次時間，不是沿用她開口問的那一刻。
    沿用的話，一個開著沒人答的提問可以放到幾小時後，他順手打個「好」動作照樣
    發生——而那正是這一維唯一存在的理由。兩個時戳因此故意不同，而畫面上那句
    「授權涵蓋這一步」不會變成假話：它講的是問他的那一刻。
  - 他打完「好」之後，三種結果各印一句話：「沒有做，也沒有交給作業系統」／
    「交出去了，那一端失敗了」／「做了」。差別是**東西有沒有離開她的手**。
    少了這一段，擋下來和做好了在終端機上長得一模一樣（一片空白），而空白讀
    起來是「成功了」。
  - 每一輪的第一列是 `ActionEvent::Granted`：**他准了什麼**本來是這份紀錄唯一
    沒記下來的一半，而它同時是一輪的界線（在這之前兩次 `sister do` 的步驟在
    檔案裡直接接在一起）。`--dry-run` 不寫這一列。
  - `sister hands log` 讀同一份紀錄。回放那段文案從 `apps/desktop` 搬進
    `sister_hands::replay_copy`，兩個呼叫端共用一份——順帶讓那兩條測試第一次
    真的被執行（CI 對那個 workspace 只跑 clippy 和 build）。目錄不存在時**報錯**，
    不印「還沒有任何動作紀錄」：`ActionLog::replay` 把「檔案不存在」當成空的
    （對它那一層是對的），而在 CLI 這一層那會變成一句沒查過的宣布。
  - ✅ **每步畫面憑據（alpha.73）**：那一格從發表那天起一律 `None`，而唯一
    的說明是「沒有取得」——一句同時涵蓋四五種狀況的話。現在她會去 `frames`
    查，並且分開講：動作**之後**才落地的那一張（唯一說得出「做完之後螢幕
    長這樣」的）、動作**之前**最後那一張（明說**不是**做完之後的畫面）、
    兩種再各分「圖在」和「紀錄在、圖不在」（沒簽第三張同意書的人 `image_path`
    是 NULL，而沒有圖的截圖憑據不是截圖）。查不到 frame 的時候照
    `heartbeat::Presence` 分六種理由各一句，而**她不知道的那三種不准說
    「所以不會有」**——心跳斷了、狀態檔讀不懂只講得出「說不準」，才剛起來
    那一種再等一下可能就有。`None` 從此只剩一個意思：這一列是舊版寫的。
  - ✅ **授權書存得下來（alpha.74）**：`sister do --save-grant` 把那張 scope
    存進資料目錄，`--use-grant` 讀回來跑，`--show-grant` 看現在存著什麼。
    於是 `GrantRejection::Task` 那一維第一次有輸入走得到——在這之前它是一段
    從發表那天起沒有任何人到得了的程式碼，而它的錯誤訊息是一句永遠印不出來
    的話。存的**只有範圍**：`StepApproval` 一個 derive 都沒有，每一步還是要
    人在當場按。期限是讀回來的**當下**重驗，不是存下去那一刻；時鐘往回跳不
    延票。三種壞掉各講各的話——沒存過、讀不懂、過期了，沒有一句讀起來像通過。
    `--dry-run --save-grant` **不存**：預演一次都不會問他，那個範圍還沒被端到
    他面前過，存下去等於用一趟沒問過的預演替下一個行程上膛。
  - ✅ **她會等下一張圖了（alpha.81）**：交出一步之後查不到「動作之後」的
    frame 時，**只有她真的在錄**才等——每 250 毫秒重查一次，最多 2 秒，一等到
    就立刻收工。擷取那一側的節奏幫得上忙：`min_interval_ms` 預設 400，而
    recorder 是每張同步 `insert_frame`，所以那 2 秒是真的等得到，不是演戲。
    沒在錄就一秒都不等（等再久也不會有 frame），`--dry-run` 一步都沒交出去，
    也一步都不睡。
    「我沒有等」和「她沒在錄」是**兩件事**，所以 `StepWait` 分三格：
    `Waited{ms}`、`DidNotWait{because}`（借 `NotRecordingReason` 那套字彙，
    於是 `Stalled` / `Unreadable` 自動說「說不準」而不是「她沒在錄」）、
    `NotRecorded`（alpha.80 以前寫的列——那幾版一秒都沒等過，**而當時她在不在
    錄根本沒有人記**，不可以拿「沒有等」去推「她沒在錄」）。
    presence 未知的那句免責只掛在 `Before` 上：`NoFrameNearby` 這個變體在
    alpha.79 就**只有 `Live(Recording)` 才產得出來**，所以它自己就是「她在錄」
    的紀錄，再補一句「在不在錄沒有記」會讓同一句話前後打架。
  - **還缺**：這一格是**查到什麼記什麼，不是驗證動作成功**——她不會去比對
    「該開的網址真的開了嗎」。等得到「動作之後」那一張，只證明畫面變了，
    不證明變成了他要的樣子。
    行程外拔手開關已接上 CLI 與 tray：每個動作在交出去之前的最後一刻查；檢查和
    `ShellExecuteW` 之間仍有一個無法消除的窗口，已經交給作業系統的那一個追不回來。
    開關住在模型碰不到的 data dir，`sister do`
    卡在等回答時也能從另一個終端機拔掉。快捷鍵仍未做。
    **tray 那兩格沒有任何執行覆蓋**——CLI 這半有測試，字母人那半只證明得了編得過
    （`check-windows.sh` 沒有 `cargo test`），要在真 Windows 上按一遍才算數。
    另外 alpha.75 修掉一個 Windows 專屬的 fail-open：`try_exists()` 對「祖先是
    檔案」的子路徑，Linux 回 `Err`、Windows 回 `Ok(false)`，於是「讀不到」被講成
    「開關不在」。同一行程式在暫停旗標與 prune 預覽各有一份，三處都改成不再無條件
    相信 `Ok(false)`；**那一格 Linux 測不出來**，守它的測試只在 Windows CI 上
    走得到，兩個薄殼都把這件事寫在註解裡。
  SPEC §9.1 的 `data_scope` 與 `denied_actions` 兩維刻意沒做：現在三種動作都
  不帶資料 payload，加兩個沒有人讀的欄位是假授權。`AllowedApps` 那一維雖然
  改成從證據算，仍然要照這個標準讀——它說的是「這一步的字是在哪個 app 的畫面上
  看到的」，不是「作業系統只會讓這些 app 被打開」（那是 Windows 的檔案關聯
  決定的，不是我們）。
- 🔶 來源防線：L0 內容 data-block 包裹 ✅（alpha.67 `prompt_fence`，
  20 種 injection 變體）；外部內容要求動作 → 強制人工核准 ✅（型別上就過不去）。
  ⬜ 端到端的 injection 套件（在網頁/訊息裡埋指令，驗證 0 執行）還沒有——
  現在測的是圍欄那一層，不是「埋了指令然後真的沒有東西被執行」。

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

**進度**
- 🔶 白名單 #1 的**前半段**做完了：`sister watch "<你在等什麼>"`（alpha.71）。
  她盯著已經錄下來的畫面，每隔一段時間拿最新那幾段字問一次大腦「這件事發生
  了嗎」，等到了就講出來並結束。**這一片沒有手**——不新增任何執行權限，
  只讀畫面、只問問題，`sister-hands` 一行都沒碰。
  - 整個模組在防同一件事：**「還沒發生」有太多冒充者**。她被暫停、她沒在錄、
    她剛開機、錄製已停只剩解釋層在收尾、心跳斷了、狀態檔讀不懂、系統時鐘
    往回跳——十種「看不到新畫面」各有各的句子，每一句都帶著「這不是『還沒
    發生』」。大腦那一邊也一樣：逾時、CLI 叫不起來、回了讀不懂的字，三種都
    **不是**「她說還沒」。
  - **「沒有等到」是一句斷言**，只有她真的問到過答案才說得出口。收尾分得出
    等到了／沒等到／一次都沒問到答案（那支 CLI 全程叫不起來，只能說「我不
    知道」）／她中途就收工了（「沒等到」只算到她停下來為止）／今天的外送
    預算先用完（明說「這不是『沒等到』，是我不知道」）。Ctrl-C 就是把行程
    殺掉，不假裝有優雅收尾。
  - **畫面上那一刻的時間不是那一列落地的時間**（要等 OCR）。所以每一輪往回
    多看五秒，並且在到期那一刻再看最後一眼——不然最後那一段永遠沒被查過，
    而「第 59 分 30 秒才跑完的那個編譯」會拿到一句「沒有等到」。
  - 一輪內字太多的時候拿**最新的**那幾段，不是最舊的。`chunks_in_range` 是
    `ORDER BY ts ASC LIMIT n`，照著用的話，一個正在編譯狂吐訊息的終端機會讓
    她永遠在讀兩分鐘前的畫面，而「All checks passed」就在她沒看的那一頭。
    漏掉的時候要講出來。
  - 走既有的大腦通道：第二張同意書（`cloud-reading`）、`[brain] command`、
    每日外送預算共用同一個天花板（`role` 記 `watcher`，但數的是不分 role 的
    那一支——改用 role 計數等於偷偷把每天送出去的總數往上加）。
  - `--quiet-for <多久>`：畫面連續這麼久沒有新的字就停下來講一聲。**本機**
    判斷，不外送、不吃預算。而她只講觀察——「畫面上已經 12 分鐘沒有出現新的
    字了」——**不講「卡住了」**，那是一個她做不到的診斷（跑 `cargo test` 的
    終端機十分鐘不吐字是正常的）。「畫面不動」和「還沒發生」一樣有冒充者：
    暫停／收工／只剩解釋層在收尾各自還是講各自那句話，空的資料表不會從
    epoch 0 起算。要求的安靜比看一次的間隔還短的話會抬起來**並且說她抬了**
    ——安靜地夾住等於給一個永遠不會觸發的旗標。
  - `--notify`：真正跑過的每一種收尾都會讓終端機響一聲；Windows 再讓工作列按鈕
    閃爍並播放提示音，而且不搶焦點。開跑時會照組建能力說明；三種開跑即跳過不通知。
    **那一聲的意思是「這一場結束了」，不是「她找到了」**（alpha.72）——五種收尾
    裡有四種答案是「沒等到」，而開跑那句話原本寫的是「等到了我會…」，等於先替
    她說了結論。中途出錯直接結束是第六種停下來，那一種原本一聲都不響，而它
    偏偏最需要響：他走開一小時，她第三分鐘就死了，終端機安安靜靜——
    「安靜」在這支命令裡本來就是他拿來當證據的東西。
  - **還缺**：她仍然分不出「卡住了」和「還在想」，`--quiet-for` 只把觀察端到
    你面前，判斷還是你下的；訊號只到這台機器，沒有系統 toast，人離開這台機器就
    收不到。
- 🔶 白名單 #1 的**後半段**（「接著推下去」）在 alpha.77 接上了：
  `sister do --unattended` 憑一張存好的票逐步跑完，不問。它要求 `--use-grant`
  （票必須是前一趟印過「已存授權書：…」的那一張，否則那個範圍從來沒有被端到
  他面前過），和 `--dry-run` 互斥。停止條件全部沿用既有的那幾道：五維涵蓋、
  步數上限、到期、拔手開關——兩條路只有「批准從哪裡來」不一樣，執行那一段是
  同一份程式碼。
  - 收尾那句話**不准說「問了」**：`RunConclusionRecord::Completed` 帶
    `decided_by`，「憑票決定了 N 步」和「問到你面前 N 步」是兩句話，而
    「憑票跑完零步（沒有東西可做）」和「有人在時問了零步」也是兩句。
  - `ask()` 這條已補上：會問人的模式在打開資料庫前先確認 stdin
    是終端機；管子或檔案餵的「好」不再能被記成他當場按了。只看 stdin，
    因為 stdout 經 `tee` 導走時人仍看得到問題，不該擋掉這個真用法。
  - **還缺**：離開偵測、交接 offer 那一段還沒做，**而且是刻意先不做的**。
    離開偵測是守門員五類候選裡的 e 類，那一整套（評分、預算、冷卻、
    quiet hours）alpha.66 就在了，`SpeakCategory::Leaving` 也在 enum 裡、
    也吃得到那些規則——缺的只有訊號源，而 `sister speak` 現在會照實說
    「leaving：這一類目前沒有訊號源。」，所以這一格沒有在說謊。
    （前綴是 `SpeakCategory::as_str()`，也就是 `leaving`，不是 SPEC 上那個
    分類代號 `e`——那條測試的 `expect("e 仍然沒有訊號源")` 寫的是代號，
    那是失敗訊息，不是產品印出來的字。）
    先不做的理由是 SPEC §8.3.3 自己寫的：「冷啟動前兩週只開 a/b 兩類
    （precision 最高），其餘類別靠自用數據逐類解鎖。」現在**一天自用數據
    都還沒有**，做出來的 e 類在頭十四天連候選都排不進去，之後也沒有東西
    可以拿來調它的門檻。訊號源那一半又只長在 `#[cfg(windows)]`（鎖屏事
    件），本機四道閘門一道都罩不到。
    **解鎖條件**：Phase 7 那兩條 Exit criteria 至少有一條開始累積真實
    run（要 Ted 拿 exe 跑）。在那之前排在這裡的每一輪，請不要重新決定
    一次——這一段已經按 SPEC 判過了。
  - **還缺**：白名單任務型別 #2（文件整理類）一行都還沒開始。它會需要一種
    **會動到檔案**的動作，而今天 `ActionKind` 三種（開網址／開檔案／聚焦
    視窗）沒有一種是破壞性的。在還沒有任何一個人真的跑完一趟逐步核准之前
    就先加一種搬得動檔案的動作，順序是反的。

**已知還在說謊的地方**（alpha.81 當下，兩條）：

1. 「提示只送出去一半就整份丟掉」這條規則守不到一種：一支 CLI **沒讀就
   退場**、而提示剛好整份塞得進作業系統的管子緩衝區。正式上限是 24 KiB，
   Linux 的管子是 64 KiB，所以那份提示算「送完了」，這條規則不會攔它。
   （不是理論——CI run 33249588350 就是這樣紅的，只是那次紅在測試身上。）
2. `sister do` 收尾那一行有六格拒絕，今天真的產得出來的只有兩格
   （範圍不涵蓋、拔手）。另外四格是**先擺好在等的**：
   `separate_approval_required` 和 `never_inherited_class` 對三種已實作的
   動作全回 `None`。分格分在型別上（多一種拒絕理由就編不過），所以那幾類
   實作出來的時候沒有人需要記得回來補；但在那之前，那四句話一次都不會出現
   在螢幕上，**沒有任何執行證據說它們印出來是對的**。

**這一輪修掉的**（alpha.81）：

**上一版那三句話裡，有一句的下一步是假的，而且照著做會弄壞他的東西。**
alpha.80 的 `Since` 那一格寫的是「在字母人上按一下暫停鍵解除，或刪掉
{flag}」。字母人**寫死讀預設資料目錄**（`main.rs` 拿
`Config::default_data_dir()` 進 `Shell`，`toggle_pause` 只看 `shell.data_dir`，
沒有 `--data-dir` 這種東西；`README.md` 自己就寫著「字母人只讀預設資料夾」）。
所以 `sister --data-dir .\test doctor` 印出那一行，那顆鍵碰不到那個檔；而那顆
鍵是 **toggle**，預設目錄沒暫停的話，照著做會**把他真正在用的資料目錄暫停
掉**，然後他回頭再跑一次看到一模一樣的那一行字——正是這一版宣稱消滅掉的失效
模式，被搬進了「這一格有可行的下一步」那一格。兩處（doctor 和 `record` 開場）
都改成 `sister resume`，它收 `--data-dir`，而且是同一份報告上下兩列本來就在講
的那一個。這條規矩 `ops.rs` 早就寫著（「指令要指得到」，是某一版出貨了不存在
的 `sister pause --off` 之後寫的）。

**`PathUnreadable` 那一格底下也是兩個世界。** `decide_for` 對
`try_exists` 回 `Err` 這條路**從來沒有再看一次 `dir_state`**，於是「data dir
是個檔案」和「data dir 是好好的目錄、只有旗標檔讀不到」印同一句「讀不到
{dir} 這條路」。後者做得出來（指向自己的 symlink、或少了 `+x` 的目錄），而
那條路 `stat` 得好好的、`ls` 列得出 `paused.flag`、刪掉那個 flag 就是解法，
同一份報告上 `hands_status` 還會說那個目錄好得很。多一格 `FlagUncheckable`，
`dir_state` 只讀一次。那一格底下**又**有兩種（壞的是旗標自己 vs 壞的是目錄的
`+x`），而這兩種**分不出來**——`dir_state` 對兩者都回 `Dir`，要測 `+x` 只能去
stat 一個子項目，那正是剛剛失敗的那件事。所以那句話不假裝知道是哪一種：兩個
權限都指出來，而且不承諾刪得掉（少了 `+x` 的目錄裡刪不動東西，實測
`delete: Err 13 Permission denied`）。

**doctor 那一句停在半路。** `PathUnreadable` 收在「先確認那條路讀不讀得
到」，而 `record` 的同一格從 alpha.78 起就走完兩步。doctor 補完。

**驗收上補的洞比修的話還重要。** alpha.80 的測試把 render 釘得很好、把接線
完全沒釘：把 `doctor::run` 裡的 `watching_verdict(paused_row(data_dir), beat)`
換成 `watching_verdict(None, beat)`——暫停那一列整個從報告上消失——
`cargo test --workspace` **全綠**。`ops::doctor::run` 有一個呼叫端、零個測試，
而整個 repo 沒有任何閘門造得出一個 `paused.flag`。現在
`scripts/check-erased-db.sh` 有一個真的 `sister pause` → `sister doctor`
場景，那個突變會紅在 `::error::[暫停中] doctor 那一列沒有畫出暫停符號`。
同一輪還殺掉三條假的綠：`watching_verdict` 可以把整句話丟掉只印
「**暫停中**」（測試手填 `Some("…")` 餵進去，看不見）、`timestamp(ts)` 可以
換成 `timestamp(0)`（測試只斷言「（從 」三個字在）、`pause_warning` 最常見的
兩臂可以對調（那兩臂沒有測試）。外加兩條**永遠不會失敗**的斷言（
`!contains("不知道從什麼時候開始")`，那句話已經不在任何產品字串上；
`!contains("沒有用")`，四條臂沒一條產得出來）和一個結構上抓不到東西的
`assert_ne!` 迴圈（每句話都嵌著自己的探測路徑，所以兩種狀態套同一個模板，
字串照樣不相等），全部拿掉或換成真的守得住的。

**這一輪修掉的**（alpha.80）：

（下面這一段描述的是 alpha.80 當時；`PauseState` 現在是五格，見上。）

`sister doctor` 的暫停那一格——上一版兩種成因印同一句「**暫停中**（不知道從
什麼時候開始）」，而那兩種的下一步是相反的：旗標在、內容壞掉，**刪掉它有
用**；連 data dir 都讀不到，`is_paused` 刻意 fail-closed 所以算暫停，而這一路
上**旗標在不在根本沒看過**，叫他去刪只會讓他刪一個可能不存在的檔、回來看到
同一行字。`sister record` 的開場在 alpha.78 分開講了，doctor 這個呼叫端沒跟
上。現在 `pause::state()` 回一個 `PauseState`（`Recording` / `Since` /
`FlagPresentButUnreadable` / `PathUnreadable`），兩個呼叫端都問它，「哪一種
暫停」只有一個地方說了算；而 `state()` 和 `is_paused()` 共用同一支
`decide_for`，不是各讀一次磁碟得到兩個答案。

改的過程自己踩了兩次同族的坑，記在這裡：三種話一度是塞進 `watching_verdict`
寫的，而那支函式頭上那段註解說的是「它手上沒有路徑，寫不出第二次讀」——把
兩條路徑當 `&str` 傳進去之後，那句話當場變成假的（`Path::new(s)` 一行就編得
過），而那段註解正是在解釋為什麼守它的是型別不是測試。三種話因此搬到
`paused_row(&Path)`，`watching_verdict` 收回原來的簽名。另一次在驗收上：第一
版的測試是手填四個 `PauseState` 去看它印什麼，於是把 `paused_row` 整段換回
舊行為（兩種 `None` 併成一句「刪掉旗標可解除」）之後，整個 workspace 照樣
全綠——證明的是「這個 enum 值會印這句話」，不是「一條讀不到的路會走到那個
enum 值」。現在四種處境各餵一條真的路徑進去（寫一個壞旗標、把 data dir 做成
一個檔案），那個突變會紅在 `not-a-dir/paused.flag 可解除` 這句假話上。

**這一輪修掉的**（alpha.79）：

`sister do`——會問人的模式以前不看 stdin 是什麼。`echo 好 | sister do …`
那個「好」會被記成他當場按了，而紀錄上那一格正是整支命令的重量所在。現在
會問人的三種模式在**打開資料庫之前**先確認 stdin 是終端機，不是就整輪不做；
`--dry-run`、`--use-grant --unattended`、`--show-grant` 三條不受影響（它們
本來就不讀現場回答）。只看 stdin：stdout 經 `tee` 導走時人仍看得到問題。

`sister forget`——action-log 裡真的多出一列空白的時候，`count_in_range` 把它
算成「讀不懂、問不出時間」的一列，`forget_range` 卻在 `LineVerdict` 外面先
`continue` 掉。`continue` 不會 push 進 `kept`，所以**那一列還是從檔案裡消失
了**，只是報告寫 0——預覽說 1、結果說 0、東西不見了。空白列現在和其他解不
開的列走同一個判斷，兩支唯一的分歧點沒了。同時：預覽三個出口只有一個印
「沒有回收桶，也沒有復原」，而沒有資料庫、以及資料庫在但區間空的那兩條
**照樣會刪掉 action-log 和授權書**；三個出口現在收在同一個 `preview_close`。

`sister watch`——到期那一輪如果 CLI 叫不起來、逾時、或回了讀不懂的字，收尾
照樣印「時間到了，沒有等到。」：前面問到過答案的那幾輪替最後那段**沒有回來
的答案**背了書。`DeadlineLastRound` 多一格 `NoAnswer`，三種原因各說各的。
建構點那個 `match` 逐項列、沒有 `_`——`Verdict` 之後多一種「其實沒拿到答案」
的變體時要在那裡編不過，而不是靜靜掉進 `Checked` 讓同一個 bug 再長一次。

`sister do` 收尾的「授權擋掉」——上一版拆成五格，其中一格還是裝了兩種：
「這一趟缺當場按」（重跑不加 `--unattended` 有用）和「這一類永遠不繼承任務
授權」（按了也不會過）。教他做的下一步對第二種是白費力氣。拆成六格。

**這一輪修掉的**（alpha.78）：

`sister watch`——到期那一刻**同時**撞到今天的外送上限時，收尾印的是
「時間到了，沒有等到。」，可是最後那一輪的畫面**根本沒有問過**。
`if expired` 那個取捨保留（時間已經到了，多給預算不會有答案，他要的是
`--stop-after`），改的是那句話：`WatchEnd::Deadline` 的 `hopeless: bool`
換成 `DeadlineLastRound`，`BudgetBlocked { used, limit }` 那一格自己講一句
「那一段沒有問，它發生沒發生我不知道」。乾淨的「沒有等到」由既有那條測試
守著，免得修法把整句話變成永遠帶著免責的廢話。

`sister record` 的 Windows 開場那一行——暫停那一格叫他去刪一個可能不存在的
檔案。`is_paused` 是**故意 fail-closed** 的（讀不到路徑就一律當暫停），而
`paused_since` 回 `None` 同時代表「旗標在、內容壞掉」和「根本讀不到那條路」
兩件事：前者刪了有用，後者刪了沒用，而她會一直說暫停中。現在那一格自己去問
`flag_path().try_exists()`，不拿 `None` 當「旗標在」的證據。判斷抽成
`pause_warning`（`cfg(any(windows, test))`），所以它在 Linux 上測得到——
呼叫端 `windows_record` 仍然是 `cfg(windows)`，那一層照舊沒有執行覆蓋。

（**`sister doctor` 那一格當時沒有跟著修**，它是另一個呼叫端：`ops.rs` 的
`**暫停中**（{since}）`，兩種成因在那一行上長得一模一樣。它掛在「已知還在
說謊的地方」上兩版，alpha.80 補完——見上面那一段。這裡原本整段寫的是
`sister doctor`，是我在派工單上就寫錯了命令名，改的人照著改對了地方、文件卻
留著錯的名字——同一段話裡第三行自己就寫著呼叫端是 `windows_record`。）

`sister forget` 的預覽以前不提
`action-log.jsonl`——沒有資料庫時它印「沒有東西可以忘」就 return，而按下
`--yes` 會刪掉那個檔案裡完整的網址和檔案路徑。現在預覽和真的刪那兩行走同一支
組句函式、報同一組兩個數字（可讀且落在區間裡的列 ／ 讀不懂、問不出時間的列）。
`ActionLog::count_in_range` 改回 `ForgetPreview` 兩格，**簽名一改字母人那半就
編不過**，所以兩個預覽出口不可能只修好一個；`Erasure` 補上 `actions_unreadable`。
補完的那一次尤其重要：檔案裡**只有**讀不懂的列時，舊寫法 `actions == 0`，
預覽整個安靜——而那正是最需要它出聲的一種。

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
