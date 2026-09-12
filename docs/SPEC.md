# AI-Sister — Final Spec

> 版本：v1.0（2026-08-17；2026-09-07 收斂現況）。本 spec 是 roundtable 辯論
> （7 題 × 5 輪）收斂 + 外部研究後的技術規格。產品定義見 [PRODUCT.md](PRODUCT.md)，
> 階段規劃與**目前有效的 Release 1.0 合約**見 [PHASES.md](PHASES.md)。本文件保留
> 部分被實作推翻的歷史決策來解釋來路；若與具日期的偏離紀錄、PHASES 或現行程式
> 衝突，以後三者為準，不能把舊選型直接當待辦。
> 狀態標記：〔定案〕辯論已收斂／〔決定〕辯論未解、由本 spec 拍板（附理由）／〔待驗〕要靠 replay 評測回答。

---

## §0. 真相模型（整個系統的憲法）〔定案〕

所有資料屬於四層之一，層與層之間的規則不可違反：

```
L0 原始證據 (Evidence)     — 存在期間 append-only、不經 LLM、不改寫；保留期或使用者
                             刪除時物理清內容，並 cascade 清掉衍生內容
L1 可確認事實 (Facts)      — 程式（regex/rule）從 L0 抽出，零 LLM，typed + indexed
L2 暫時解釋 (Hypotheses)   — LLM 產物；每筆掛 confidence + 證據指標；只能追加新版本，
                             不能覆寫舊版本；可被推翻、可過期
L3 可更新狀態 (State)      — 承諾/未完成事項/實體/偏好；每筆掛出生證明（來源 L2/L0 ref）
                             與死亡條件；可結案、可刪除，刪除向下游 cascade
```

鐵律：

1. **L0/L1 由程式寫，L2/L3 才准模型碰。**（抄寫歸程式，意圖歸模型）
2. **任何 L2/L3 不得成為證據的唯一載體。** 早壓縮的那份永遠不能是唯一的一份。
3. **禁止滾動改寫（rolling rewrite）。** 任何 agent 不得讀另一個 agent 的敘事後續寫；
   解釋者只能讀 L0/L1 + 最近一筆 L2 假設（不是七份完整敘事）。
   這是「可刪除記憶」能成立的前提：刪除 = 沿 provenance graph tombstone 衍生內容。
4. **螢幕上的文字是資料，不是指令。** 來源標記由規則決定、零 LLM〔定案〕：
   使用者鍵盤輸入與 UI 按鈕 = 指令；網頁/郵件/文件/截圖裡的文字 = 資料。
   任何來自 L0 的內容永不進入 system prompt 的指令位置；一律以 data block
   包裹並標示來源（prompt-injection 的第一道防線；全行業空白，我們把它做成標準）。
5. **每一句對使用者說出口的事實，都要能沿 provenance 點回本機證據。** 有被同意
   保留且尚未過期的 frame 才顯示畫面；text-only／已過期時要明講只剩文字、時間或
   來源，不能把一個不存在的圖示畫成可點。

## §1. 系統總覽

```
┌─ sister / sister-core（Rust）──────────────────────────────────────┐
│  recorder process：Capture → L0/L1（SQLite + frame files，全本機） │
│  Segmenter / Interpreter / Reviewer / Gatekeeper / Query engine   │
└──────────────────────────────────────────────────────────────────┘
            ▲ sibling process + 共用 data dir；沒有 HTTP／WS／socket
┌─ sister-desktop（Tauri 2 Rust backend + 原生 HTML/CSS/ES module）──┐
│  桌面姊妹、對話、時間軸、同意書、設定、開發者模式                     │
│  renderer 只經 Tauri IPC 呼叫同一行程裡的 Rust                     │
└──────────────────────────────────────────────────────────────────┘
┌─ sister-hands（Rust crate；CLI 與 desktop 共用唯一授權邊界）──────┐
│  observe / suggest / semi-action；平台 executor 是封閉 capability │
└──────────────────────────────────────────────────────────────────┘
```

現行拓撲〔2026-09-07〕：core 與 shell **邏輯分離**；長時間 recorder 是桌面可啟動的
sibling process，資料與 UI 生命週期分開。desktop 只 supervision 自己 spawn 的
child；外部啟動的 recorder 只會被視為占用，不 adopt／kill／restart。Tauri renderer 直接走 IPC，沒有早期草案的
loopback HTTP/WS、nonce server 或對外 port。hands 已是 Rust crate 與型別隘口，不是
Node sidecar；若未來真需要更強的 OS process isolation，再用實際 threat model 另立里程碑。
macOS 的 capture 仍必須住在簽名 `.app` 主程序樹內以維持 TCC identity；這個平台限制
不代表要把已移除的 loopback 拓撲裝回來。capture trait 抽象不變，宿主按平台掛 adapter。

## §2. Capture 感官層規格

### 2.1 訊號源（v1，只用通用 OS API——不做 per-app adapter）〔定案〕

| 訊號 | 方式 | 頻率 | 落地層 |
|---|---|---|---|
| 螢幕 frame | Windows Phase 0 用 GDI 擷取（OCR／dHash 工作幀長邊上限 4096px，超過才等比縮小），再做 dHash 去重；真的留圖時另依 `max_long_edge`（預設 1568px）縮成 PNG | 每拍做完後預設等待 400ms；無輸入時可跳過擷取，但最久 5s 補看一次 | L0 |
| OCR 全文 | 平台原生 OCR（見 §15），只跑「保留下來的」幀 | 隨保留幀 | L0 |
| 前景 app / 視窗標題 | Win32 / NSWorkspace；Windows alpha.103 逐拍取 exact HWND／PID 並在內容讀取後重驗 | capture tick | L0 |
| 瀏覽器 URL | UIA（Win）/ AX（macOS）讀址欄；瀏覽器 URL 或焦點狀態 Unknown 時 fail closed | capture tick／視窗事件 | L0 |
| 剪貼簿 | Windows sequence polling；>64KB 截斷；秘密與來源 app 排除（見 §11） | capture tick；排除／鎖定空洞只建立水位、不讀內容 | L0 |
| 輸入動態 | 鍵擊/滑鼠**節奏與計數**（永不記內容）、捲動、視窗切換頻率、idle | 聚合 per-10s | L0 |
| 系統狀態 | lock × power 正交狀態；Windows alpha.103 由相鄰 WTS polling 樣本觀察 lock／unlock，power notification 尚未接，不從時間空洞猜 sleep／wake；Unknown 時不讀內容 | 每個內容來源前後重驗 | L0 audit transition |

**「事後補不回來」清單**〔定案，Claude T6 提案〕：捲動位置與速度、滑鼠停留、
視窗切換節奏、輸入爆發模式、選取事件、一閃即逝的通知。
這些全部是純程式、幾乎零成本，**必須當下抓**——這張清單的存在，
就是「前線不需要 LLM 也能保住當下意義」的答案。

### 2.2 幀保留與 OCR gate

- dHash 相異度 ≤ 閾值 → 通常不存（只記「持續中」計數）；Windows OCR gate
  仍會用完整 RGB baseline 定點重驗，只有通過下述結構證據的小字追加能把它升格；
- 變化幀的文字與脈絡寫進 DB；允許留圖時，畫面另依 `max_long_edge`（預設
  1568px）降採樣成 PNG，再按保留政策降解（§11.4）；
- **OCR gate**〔定案；Phase 0 Windows 已落地〕：第一次真的進到 OCR 的保留幀全幅
  建立閱讀順序、RGB 像素與文字 baseline；之後先對 recorder 已抓到的 `RawFrame`
  做 64×64 RGB tile FNV-1a，hash 相同仍逐 RGB pixel 確認，再把每個變動 pixel
  配給唯一一個舊行的 edit envelope。局部成功集合刻意很窄：橫排行首或行尾只有
  一個連續像素帶，fresh OCR 字串以完整舊字串為 prefix／suffix 且只多一個
  Unicode scalar，fresh block 外擴 2px 又覆蓋全部變動像素，才按原本 full-OCR 順序放回；
  **crop 不再做第二次縮圖**。這些是可檢查的結構 heuristic，不宣稱 crop 與全幅
  OCR 必定逐字相同。Windows 抓圖本身的長邊上限是 4096px，超過時會在進 gate 前
  先等比縮小。dHash 判成近似重複時，gate 仍用精確 RGB 比較；只有 `Regions`
  候選會試 crop，stitch 通過才把這幀升格並寫進 DB。stitch 的結構證據不足就
  維持重複，**不再補跑全幅**，避免閃爍游標每次反而觸發最貴路徑；摘要只把
  這種完整跑完後的結構拒絕列成「局部未採用」。crop 建立或 OCR 執行真的失敗
  也維持重複、不補跑全幅，但另計 OCR failure 並保留最後錯誤；實際成本仍算進
  gate／OCR 時間與嘗試像素。dHash 已判成新畫面時，其他
  編輯、沒有舊行 owner 的新增文字、整行刪除、一對一覆蓋不成立、像素歸屬不明、
  螢幕尺寸改變、變動面積過大、crop 空讀或任何局部錯誤才當場退回全幅。全幅重試
  若成功，仍是成功的全幅 OCR 路徑，摘要另外標出其中幾張是 fallback。完全未變的
  幀經精確確認後仍維持重複，不另寫一張；「沿用」只保留給兩道判斷不一致的防禦
  路徑，正常多半是 0。真 Windows 實測後再決定下一層
  文字區域偵測值不值得做。（Vision 後續設定仍是 accurate-only、zh-Hant 放語言
  列首位、關 language correction）；
- 敏感排除（§11.2）發生在**capture 當下**，不是事後刪除。前置閘門命中時不讀
  clipboard content、不呼叫 screen capture；若前景在 OS 呼叫期間改變，工作 frame
  可能短暫存在 RAM，但後驗失敗會在 dedup／OCR／DB／PNG 前丟棄。

### 2.4 macOS 平台憲法（躲不掉的，就做成賣點）

- **紫色錄製指示無法關**（15.1+ 連續擷取 = 選單列紫點常亮，無 API/entitlement 可豁免；
  正規豁免 `persistent-content-capture` 需向 Apple 專案申請——可申請但不依賴）。
  產品立場〔決定〕：**不躲**。姊妹的「眼睛開/閉」狀態與紫點誠實同步，
  文案直接寫「那個紫點就是她——她在看的時候你永遠知道」。誠實是我們的差異化。
- **週期性重授權 nag**（Tahoe 26 仍在）與 **大版更靜默重置 TCC**：
  onboarding 流程必須把「重新授權」做成一鍵引導（偵測授權失效 → 角色提示 → 深連到設定），
  不能讓使用者以為她壞了。

### 2.3 資源預算（長期產品硬指標）

| 項目 | 預算 |
|---|---|
| CPU | 長期產品目標：idle ≈0%（事件驅動）；活躍平均 < 3% 單核（capture+dedup <1%，OCR 以 0.1–0.2 有效 fps 攤提；尖峰 < 15%） |
| RAM（core daemon） | 常駐 < 300MB、峰值 < 500MB（embedding 模型 lazy-load） |
| 磁碟 | 實際預期 < 200MB/天（文字+索引 ~1–5GB/年可永久保留；截圖層 0.03–0.12GB/天，是唯一要 retention 旋鈕的層）；2GB/天為自動降級上限 |
| 電池（MacBook） | 前台工作日不因本軟體損失 > 5% 續航；**標配「電池模式」**：拉長取樣、OCR/embedding 延後到插電或深夜 idle——Screenpipe 沒做好這塊，是差異化機會 |
| 全機停擺開關 | tray 一鍵「看別的地方」，狀態視覺可見（角色閉眼） |

CPU 的 `<3%` 不再是 Phase 0 gate：alpha.46 在 Ted 的真 Windows、1920×1080、
活躍寫程式 60 秒量到 44.0%，Ted 於 2026-08-23 選擇保留觀察密度並接受這個基準。
CPU 仍然每場照實量；這個 Phase 0 例外不等於刪掉正式產品的長期目標，也不把
44.0% 改寫成一條新的通用預算。

（數字依 `research/tech-stack.md` 論證；這張表是長期優化目標，不是 Release 1.0 的
精確數字 gate。發布仍要照實列當版實測，且無上限成長／資料損失不能放行；但依
2026-08-23 決策，不為湊 `<3%` 或 `<300MB/天` 停掉功能發版。
競爭基準：Screenpipe 官方自承 5–10% CPU / 0.5–3GB RAM / 5–20GB/月
——我們不錄影不錄音、text-first，量級直接少一個 0。）

## §3. Facts 事實層（L1）〔定案〕

程式抽取器（零 LLM）從 OCR 文字與剪貼簿抽出 typed facts：

- `money`（金額+幣別）、`phone`、`url`、`email`、`file_path`、`error_code`
  （`ERR_*`、exit code、exception 名）、`id_like`（訂單號/追蹤碼 pattern）、
  `datetime_mention`（「五點」「週四」— 供承諾觸發用）。
- 每筆掛 frame ref + 視窗 context + 時間戳；進 FTS 與 typed index。
- 「帳單多少錢」「客服電話幾號」由這層 + FTS 直接回答，**零 agent、<100ms**。

## §4. Segmenter 斷句器（本專案的核心演算法）〔待驗，設計如下〕

輸入：L0/L1 事件流。輸出：`segment`（append-only，帶信心值的邊界假設）。

### 4.1 邊界訊號（v1 全程式，無 LLM）

切刀（任一觸發）：前景 app 變更、瀏覽器 host 變更、idle > 90s 後恢復、
螢幕鎖定/解鎖、剪貼簿大段複製後貼到另一 app、**強制時間上限 10min**。
黏合（抑制切刀）：切換後 < 30s 返回原視窗（查資料折返）、同一「工作集」
（短窗口內反覆共現的視窗群 = 同 session）內的切換。
每段前後保留 5s 重疊 margin〔定案〕。

### 4.2 兩級結構

- `segment`（分鐘級）：連續同質活動片段。
- `session`（小時級）：由 Reviewer（§6）把 segments 聚成「你今天的一天」的章節，
  含跨 app 工作集（terminal + browser + editor = 同一件事）。

### 4.3 Ground truth 與調參

斷句沒有現成題庫〔定案〕。自建：replay 語料（§12）上手工標註邊界 →
邊界 F1 為指標；使用者在時間軸 UI 手動合併/切開的動作全部記錄為訓練訊號。

## §5. Interpreter 解釋工作槽（L2 生產者）

〔決定〕辯論最大懸案「前線准不准寫判斷」：**Grok 的預設 + ChatGPT 的出口**。
預設不跑（純程式訊號已保住當下意義，§2.1），但保留一條**薄假設層**，
以事件驅動、預算制運作——且以 replay A/B（§12）持續裁決要不要加厚。
理由：Grok 的「判斷一旦決定哪裡重要，它就在當作者」用**判斷不影響錄製密度**來拆解
（錄製密度永遠只由程式訊號決定，判斷只產生 L2 卡片）；ChatGPT 的「事後拼不回語意」
用薄卡片保留。兩邊的實質主張都被保住，剩下的量由數據決定。

### 5.1 喚醒條件（不是秒針！）〔定案：算力跟資訊價值走〕

- segment 關閉且該段含「值得理解」訊號：error code 出現、大段貼上、
  通知出現、長停留後恢復、工作集變更；
- 「卡住」偵測：同視窗長停留 + 反覆小幅切換 + 輸入節奏異常；
- 預設**每日解釋預算 80 次**，超過即靜默降級（只累積 L0/L1，Reviewer 批次補）。
  一天 8 小時裡「值得理解的時刻 < 50 次」〔定案〕是預算的依據。

### 5.2 輸入 / 輸出契約

輸入：該 segment 的 L1 facts + OCR 摘錄（原文，§11.3）+ 工作集脈絡 +
**最近一筆 L2 假設（僅一筆，且標示為「可推翻的他人假設」）**。禁止輸入他人敘事鏈。
輸出（strict JSON）：

```json
{
  "segment_ref": "...",
  "activity": "在 Cloudflare dashboard 設定 DNS 記錄",
  "entities": [{"type":"project","name":"multi-ai-terminal"}],
  "continues": {"segment_ref":"...", "confidence": 0.7},
  "commitment_candidates": [{"text":"五點去接她","source":"LINE 通知","due_hint":"17:00"}],
  "confidence": 0.6,
  "evidence_refs": ["frame:...","fact:..."],
  "open_questions": ["未看到 DNS 儲存成功的畫面"]
}
```

- 併發槽數 = 排程需求（訊號到達率 × 模型延遲），預設 4、上限 8——
  「8」是 worker pool 參數，不是產品概念〔定案〕。
- 模型：cheap tier（Haiku 級 / Flash 級 / 本地模型，§10）。輸出進 L2，永不直接進 L3。

## §6. Reviewer 批次審閱者（原「3 隻 orchestrator」的最終形）

- **節奏**：活躍時每 15–30min 一輪；日終一輪大盤點；閒置/夜間做 consolidation。
- **讀什麼**：最近的 L2 卡片 + L1 索引；**必要時回查 L0 原件**——每次回查記 log，
  「回查率」是公開指標〔定案：回查率決定前線層的生死〕。
- **寫什麼**：(a) 修訂/推翻 L2（新版本 append）；(b) session 聚合與命名；
  (c) L3 承諾表的唯一寫入者；(d) 日摘要。
- **微辯證的正確位置**〔定案〕：對「高風險寫入」（新承諾、開口候選），
  用第二次獨立 pass（不同模型或不同 prompt 角度）重讀證據；
  **分歧 = 警報**：降 confidence、抑制開口，不寫入 L3。
  平行独立，禁止互讀作文——並聯才是視角，串聯就是傳話。
  多數決不能消除幻覺〔定案〕：雙 pass 的價值在「分歧偵測」，不在投票表決。
- **強制回查類別**〔定案〕：金額、人物、承諾、未完成狀態、長期記憶候選——
  這五類寫入 L3 前**必須**回查 L0 原件（「看起來合理的錯不會觸發警報」的唯一解）。
- **合併紀律**：Reviewer 的合併是 typed card merge（欄位級），不是敘事重寫——
  否則碎紙機從後門回來。原則：**假設是快取，原件才是記憶**。
- **Consolidation**：超過 N 天的 L2 壓成日/週摘要（原 L2 留墓碑鏈）；
  L3 逾期未驗證項自動轉 archive（不煩人）。
- 模型：mid tier（Sonnet 級），夜間盤點可用 batch API 半價。

## §7. Memory / State（L3）規格

### 7.1 Schema（SQLite）

```
commitments(id, text, kind{promise|todo|followup|reminder},
            born_from L2 ref, evidence_refs[], people[], due_hint,
            due_source{explicit|inferred},
            status{open|done|dead|snoozed|archived}, confidence,
            allowed_next_step,   -- 「帶著上下文接手」的接點：她能替你做的下一步（含權限邊界）
            last_evidence_seen_at, kill_note, created_at, updated_at)
entities(id, kind{person|project|app|org}, name, aliases[], first_seen_ref, notes)
day_summaries(date, narrative, session_refs[], stats)
preferences(key, value, learned_from ref)   -- 例：quiet hours、不想被提醒的類別
provenance(child_ref, parent_ref)           -- 全域血緣圖，刪除 cascade 用
```

### 7.2 記憶死亡規則〔定案：記憶要能死〕

1. 使用者說「弄好了/沒了」→ `dead`，帶 kill_note，永不再提。
2. 後續證據矛盾（Reviewer 發現完成畫面）→ `done`（自動，附證據）。
3. `due_hint` 過期 + 使用者未互動 → 到期後 48h 轉 `archived`（沉默，不 nag）。
4. **螢幕外完成問題**（手機上做完了）〔定案為無完美解〕：處理策略 =
   低頻低調的「順帶確認」——只在使用者主動開對話時、於回答尾端輕聲問
   （「順便一提，Cloudflare 那件還掛著嗎？」），永不為確認而主動開口。
5. 刪除 L0 區間 → provenance cascade：衍生 L2/L3 全部 tombstone〔定案：
   這就是禁止滾動改寫換來的能力〕。

### 7.3 使用者回饋分類〔定案〕

UI 只有兩個動作：**「結案」**（=dead/done）與**其他一切**（=snooze + 降權）。
ChatGPT 的三分類（否定事實/否定時機/接受）由 Reviewer 從對話語境**推斷**，
不要求使用者標註。

## §8. Expression 表達層（桌面姊妹）

### 8.1 視窗與角色

- TokenMonster pet-window 配方移植到 Tauri：frameless、transparent、skipTaskbar、
  可 pin、dragbar 拖曳、close→tray、bounds 持久化、navigation 硬化。
  已知代價（tech-stack）：Tauri 無 per-pixel 點穿（整窗模式切換）、macOS 需
  `macos-private-api`+`tauri-nspanel` 且有 DMG 透明失效與 **GPU 功耗 8×** 的已知 issue
  → 對策：角色動畫用低 fps 靜態立繪 crossfade（TokenMonster 本來的做法，不是巧合），
  提供「不透明小窗」低功耗模式，透明模式的功耗列入 §2.3 電池預算量測。
- 識別：四姊妹與 13 位閨密共 17 個 canonical 身分，角色圖全部隨 desktop 離線提供，
  預設 ChatGPT。alpha.110 固定選 17 套 workplace 分層 rig（402 PNG、
  35,140,885 bytes），runtime 只建立當前角色的 21–26 層；所有圖層解碼成功
  後以完整透明全身 canvas 呈現，不畫相框或裁掉腿腳。在此之前都顯示由同一
  workplace canvas 縮出的 640×640 透明全身 bundled WebP，任一檔失敗就留在
  WebP 且不自動 retry。沒有
  Neutral，也沒有 code-native 字母 fallback。設定頁把這 17 張 WebP 畫成 native radio
  圖像卡；切換只更新未存的本機預覽，`settings_write` 成功後才 emit 給主視窗。
  兩組角色圖的來源、大小、SHA-256
  與 Apache-2.0 授權排除分開固定在 bundled manifest/NOTICE。17 位角色的日常聲音庫
  也隨 desktop 離線提供；每位基本包 8 句、擴充包 24 句，共 544 段 Ogg Opus。
  同意書另有每位四張、共 68 段 bundled Ogg、4,542,053 bytes；逐段逐字稿必須等於 core 當下的
  `Sheet::wording()`，不同就停用，不可用舊錄音念新條文。
  WebView 只從同源 bundled path 播放，CSP 的網路出口仍只有 IPC。
- 狀態表達（不彈窗）：`idle`（呼吸）／`paused`（閉眼 = capture 停）／
  `thinking`（微動）／`has-something`（微光 + 一個小點，像未讀）。
  點角色 → 一句本機 deterministic tap-line，必須來自 trusted 操作、只播放 bundled Ogg，
  不叫 CLI、不連網。輸入框的每個文字問題都走目前選定的 CLI 與本機記憶路徑，包括
  早安、晚安等短句；不得用固定台詞繞過 CLI 或冒充動態回答。
  同意書朗讀也只接受當張「念給我聽」的 trusted click，不 autoplay、不借 Azure 或
  `localService`；使用者仍在原輸入框以文字回答。
  聲音另行 opt-in。動態答案的本機朗讀只能由使用者按「用本機聲音朗讀」後交給 WebView
  明確標成 `localService` 的中文系統 voice。alpha.110 另有預設關閉的 Azure 繁中新答案
  自動朗讀，須獨立設定、key 與
  現行第四張 consent；它不是本機 voice 的 fallback，兩條也不互相自動切換。答案
  下方的 trusted 按鈕可停止或手動重播，重播會再送一次。
  persona 不得影響答案、證據、同意書、Gatekeeper 或 hands。

### 8.2 對話（被動答題——永遠可用，這是 Release 1.0 的核心）

- 輸入框隨時可問。第二張同意有效且大腦已接好時，每個文字問題先交給該 CLI；它只可回
  1–3 條自然語言查詢，不能指定 SQL、路徑或命令。AI-Sister 用既有 `TextAndFacts` profile
  在 SQLite 本機執行並跨查詢去重，總上限 facts 10／原文 20。CLI 不取得 DB path、整份
  資料庫或 screenshot bytes；目前沒有向量索引。
- 命中的 facts／原文依既有順序選最多 12 筆，再交回同一支 CLI 成句。送進 CLI 的問題
  副本最多 2 KiB；nonce 圍欄內的問題與來源資料合計最多 12 KiB，單筆來源正文最多 4 KiB，
  app／title／URL 也各自有界。來源裡的控制字元與指令樣文字一律是資料，不能改寫回答契約。
- CLI 只可回 strict JSON：1–3 句、每句最多 240 字、至少一個來源，而且來源只能逐字引用
  這一輪提供的 `fact:<id>`／`chunk:<id>`。native 再把每個 ref 映回同一份本機答案；未知
  ref、來源／畫面不一致、空句、超長、額外欄位或壞 JSON 都整份拒絕，不把部分輸出混進
  正常答案。
- 零命中時 CLI 仍已處理並規劃這題，但不生成沒有來源的事實答案。沒設定 CLI、第二張同意
  未簽、全停、spawn／timeout／空回覆、輸出不合契約或稽核沒寫成時，facts／原文仍是完整
  本機結果。每個非空新問題接管唯一的回答槽並終止舊題 process group／Windows Job tree；
  過期結果不可畫、不可朗讀。
- CLI 回報第二張尚未授權且該張在目前條文下真的尚未回答時，主對話接手逐張詢問並保留
  原問題；全部回答後自動把完全相同的問題重送目前選定的 CLI。明確回答「不同意」只維持
  功能關閉，不可每次啟動重問，也不可把本機 fallback 冒充 CLI 答案。
- 有成句時，本機朗讀與 Azure 的答案正文只取這 1–3 句，不重複朗讀底下的 facts／OCR，
  也不帶 source ref、按鈕文字或 metadata；沒有成句時維持原本的本機結果朗讀。
- alpha.116 的本機 L2 記憶總覽保留為沒有可用 CLI 時的離線結果。只要已選 CLI、第二張
  同意有效且查詢計畫完成，「她／妳／你知道了什麼／記得哪些事」也走同一條 CLI-directed
  本機檢索，不再繞過大腦。
- 總覽目前沒有任何記憶內容、有原始紀錄但尚未整理出 L2、或有 L2 但最近候選目前
  沒有畫面出處時，要各自說明。任何一格都不得 fallback 成 OCR 或把無出處 L2 當答案。
  它也不寫入既有 retrieval
  query log；那套 miss、latency、click 與 replay 指標不拿不同工作負載混算。
- 延遲預算：檢索 < 100ms；成句 < 3s；**> 3s 視為 bug**〔定案：轉圈超過三秒，
  人會自己去翻，然後再也不問〕。
- 每個有畫面出處的答案：出處 chips（點開 = 嘗試讀當時畫面；檔案若在點擊前被外部
  移走會明確失敗）+ 不確定語氣規範（「我最後看到的是…」，禁止「你還沒做」式
  斷言）〔定案〕。沒有出處的狀態回覆不偽裝成答案卡。

### 8.3 Gatekeeper 守門員（主動開口——辯論公認最難、無人給出規則）

〔決定〕承認「這條規則第一版一定是錯的」，所以把它設計成**可量測、可調參、
預算封頂**的系統而不是一條 if：

1. **候選來源白名單**（只有這五種事件有資格產生開口候選）：
   a. `commitment` 帶顯式時間且臨近（「五點接她」→ 16:20 起可候選）；
   b. 通知出現後 N 分鐘無互動（會計師的 LINE 沒點開）；
   c. 卡住偵測超閾值（同處停留 + 反覆切換 + 有 error 事實）；
   d. session 結束/日終 → 「要不要做筆記」類 offer；
   e. 離開偵測（鎖屏前）→ 交接類 offer。
2. **評分**：`score = impact × confidence × timeliness × evidence_strength`；
   高風險寫入先過 §6 雙 pass，分歧即棄。
3. **預算**：預設 ≤ 5 次/天、同類冷卻 2h、quiet hours、專注模式（全螢幕 app）自動靜音。
   冷啟動前兩週只開 a/b 兩類（precision 最高），其餘類別靠自用數據逐類解鎖。
4. **形式階梯**：微光（免費，不算預算）→ 一行輕聲字（算預算）→ 帶按鈕的建議卡
   （算預算 × 2）。TTS 出聲屬於 opt-in 且僅限 a 類。
5. **每次開口記錄**：候選分數、依據、使用者反應——這是守門員的訓練語料〔定案：
   使用者的提問與更正是唯一拿得到的 ground truth〕。

### 8.4 第一句話（辯論的靈魂拷問：「你希望它第一次開口跟你講什麼？」）

規定死：**新安裝後的第一次主動開口，必須是 b 類（你沒注意到的通知）或
a 類（顯式時間承諾）**——這兩類是「使用者自己能立刻驗證為真」的類別。
第一印象只有一次，不拿低 confidence 的推理去賭。

## §9. Action 行動層（sister-hands，Phase 6+）

〔定案〕四層不可互替：**感知／來源／驗證／授權**。行動層規格圍繞它們：

1. **權限階梯**：`observe`（預設，物理上無 hands）→ `suggest`（可開 URL/檔案/聚焦視窗
   ——僅限使用者點按鈕觸發）→ `semi-action`（互動時逐步顯示並以對話式「好」核准；
   或用先前保存的結構化授權書跑 `--use-grant --unattended`）→ `takeover`
   （接手模式：白名單任務型別、有邊界）。**「好」只核准畫面上顯示的那一步**；
   無人值守不是把一句舊的「好」重播，而是每一步重新檢查授權範圍、期限、步數與
   目標來源，再鑄出只對該具體動作有效的 permit〔定案：CLI 證明的是互動可行，
   不是授權夠精確〕。授權是結構化物件，不是一句話。目前 saved grant 落地的是
   `grant = {task, apps[], allowed_actions[], expiry, step_limit}`。現在的三種 action
   沒有資料 payload，先加 `data_scope`／`denied_actions[]` 只會造出沒人讀的假授權；
   新增資料型或不可逆 action 時，才必須在同一版把對應 scope／deny 語意與 enforcement
   原子加入。
2. **可逆性是分界線**〔定案〕：不可逆動作在任何模式下都需要顯式即時核准；
   **永不繼承清單**〔定案〕：送出、發布、付款、刪除、開 terminal——
   這五類權限不隨任務授權繼承，每次都要單獨核准。
   bounded takeover 只含可逆操作（「程式留在 staging」是產品格言）。
   命名註記〔定案〕：辯論封印了「Autopilot」這個詞——對使用者一律叫
   **「接手模式」（takeover）**，因為它承諾的是「有邊界地接手」，不是「自動駕駛」。
3. **驗證迴圈**〔定案：解 35% 複利魔咒的不是步準確率〕：每步後截圖驗證結果，
   失敗即停；任務有 scope 描述、停止條件、步數上限、完整 action log。
   **硬中斷在模型碰不到的層**〔定案〕：全域快捷鍵 + tray kill flag 阻止下一步並讓
   executor fail-closed（不是請模型停）；不綁 Esc（太容易誤觸與被吃）。目前 hands
   不是獨立 process，已交給 OS 的單一步驟也不能假裝可撤回；若要宣稱 process kill
   或 rollback，必須先真的加入對應隔離與可逆機制。
4. **來源防線**：畫面文字 = 資料不是指令（§0.4）；hands 的 system prompt
   不接受任何 L0 內容作為指令，L0 最多只能提供 typed fact 與候選動作，永遠不能
   自己成為授權。互動路徑要有當場按下的 `UserButtonPress`；無人值守路徑則須同時
   通過保存的結構化授權書、目標畫面的雙 pass 出處、綁定具體目標的 permit，以及
   使用者選定的 URL 政策。任何缺資料或查詢錯誤都 fail-closed。URL 政策只管
   standing grant：當場按下在兩種答案下都放行；沒答過時無人值守拒絕，但不能把
   「沒問過」寫成「使用者說不要」。使用者可選「網址一律當場按」，或只讓她開在
   保留中的真 Windows 錄製裡見過同 host 的網址（容許一層 `www.` 差異）。alpha.103
   起後者只信 exact
   `sessions.platform = windows/windows-gdi-uia-focused-url-v2`。歷史 v1 可讀、可顯示，
   但因全域 focused element／stale URL cache 沒有證明 exact HWND 與當拍 live value，
   不再授權；更舊錄製、import 與 replay 也不能背書。v2 的 host provenance 仍不證明
   網站安全、使用者意圖、path、redirect 或站內內容。
5. **預檢**：接手模式啟動前做 fresh-evidence check（重新看畫面），
   不信任可能過期的 L3 記憶〔定案：記憶可能記歪的東西不能直接點滑鼠〕。
6. **代理身份方向**（遠期）：關注 OS 級 agent identity/session 隔離的發展，
   我們的 hands 優先跑在「使用者看得到的前台」，不做影子登入。
7. **核准疲勞對策**：核准綁「具體動作 + 具體目標」（「把 A 檔上傳到 B 表單」），
   不綁模糊範圍（「幫我處理這件事」）；第 50 次彈窗盲點問題用
   任務級 scope 核准 + 步級只在偏離 scope 時打斷。
8. **監督 CLI agents（Ted 的真實場景）**：hands 的第一個白名單任務型別就是
   「盯著 claude/codex 跑、卡了回報、按既有 spec 推 milestone、不碰 deploy」——
   MAT 的 adapter/steering 程式碼直接移植。

## §10. 模型接入（multi-ai）

〔2026-08-21 定案〕在本機的是截圖，語言模型在雲端。第一批使用者手上已經有
claude code / codex / grok / gemini cli。所以 L2/L3 那個腦要接的第一個東西
**不是 HTTP client，也不是 secret-vault 裡的 API key**，是使用者已經裝好、
已經登入、已經在付錢的那支 CLI。`sister` 用 `std::process::Command` spawn
它：prompt 從 stdin 進、JSON 從 stdout 出。`check-no-network.sh` 對 `sister.exe`、
recorder／core／capture／brain／hands 繼續禁 HTTP client 與本機推論引擎，沒有
例外。desktop 的 Persona 下載能力收在 root workspace 的 `crates/sister-assets`：預設
feature 集合沒有 `download`；Azure 答案朗讀收在 `crates/sister-tts`：預設 feature
集合沒有 `azure`。只有 desktop 明確啟用這兩個窄 transport；它們都不把 HTTP 能力
交給 brain，Azure TTS 也不是模型接入或 OCR 出境路徑。

- **alpha.123 的設定入口**：設定頁固定列 Claude Code、Codex、Gemini CLI、Grok CLI
  四張卡。native 端從 `PATH` 與各 CLI 的 Windows 標準安裝位置找 executable，並以
  有界 `--version` probe 顯示實際版本；未安裝的選項不能按登入。
- **登入與選用是同一筆 transaction**：trusted click 才會開該 provider 自己的登入流程；
  登入結束後，desktop 立即透過 bundled `sister.exe brain-cli-bridge` 要求 exact 固定
  token。只有 bridge、provider 與既有登入三者一起測通，才 atomic 寫入新大腦；登入、
  probe、儲存失敗或取消都保留原設定。任一時刻只准一筆登入／測試；取消會終止整個
  process tree。
- **runtime bridge**：`config.toml` 仍以 `[brain] command` + `args` 保存，但設定頁只會
  寫入 bundled sister executable、固定 bridge 子命令、typed provider 與已偵測的 executable，
  不接受使用者把 prompt 或任意參數拼進介面。Claude／Codex／Gemini 從 stdin 收 prompt；
  Grok 使用只有目前使用者可讀、handle 關閉即刪的 private prompt file。四支 provider 都在
  每次新建的空 private working directory 執行，結束後整個移除；OCR 原文不進 argv。
- **舊自訂設定**：既有 raw `[brain] command` / `args` 繼續可由 recorder 使用，但不會在
  四張 provider 卡中冒充「已選用」，也不會接管桌面輸入框。從設定頁登入、測通並選定任一
  provider 後，互動問答才改由上述固定 bridge 管理。brain／core／recorder 沒有 HTTP client；
  帳號與 token 由 provider CLI 自己保存，AI-Sister 不接收也不保存。
- **alpha.126 的 S1 runtime**：每題先由 configured CLI 回 1–3 條自然語言查詢，desktop
  在 SQLite 用 `TextAndFacts` 執行、去重，再把當前問題與最多 12 筆命中來源交回同一支 CLI。
  這是有界的 host-mediated agent retrieval，不把 SQLite path 或 SQL 能力交給 provider，也不
  建 embedding、向量庫、prompt cache 或第二份記憶。有效答案必須逐句引用本機 ref；
  `brain_outbound` 以 `answer_search`／`answer` 分開記兩階段的結構、字數、duration 與 outcome，
  不存 prompt／問題／來源原文。
- **角色→模型對映**（可配置，附預設）：interpreter=cheap tier；reviewer=mid tier
  （夜間 batch 半價）；chat=使用者選；hands=使用者訂閱的 coding agent。
  實際選哪一個模型，是那支 CLI 自己的事。
- **降級鏈**：沒簽同意書 2 / 沒設定 CLI / 每日預算用完 → 純檢索模式
  （1.0 功能永遠活著）。三種原因印三種話。
- 併發槽數預設 4、上限 8（§5.3）。

## §11. 隱私與安全（產品的第一賣點，工程上與功能同權重）

### 11.1 四張同意書〔定案〕（主對話逐張詢問，設定保留四個獨立開關）

1. **本機記錄**：我同意在我的硬碟上記錄我的螢幕。這張只授權本機記錄，不授權
   上傳畫面或文字；不簽第二張時 S1 仍可完全離線運作。Persona 的選配素材下載是
   另一個當下揭露、當下按鈕，不藏在這張同意書裡。
2. **上雲解讀**：我同意把在 AI-Sister 輸入的問題交給設定裡選定的 CLI，讓它決定
   要查哪些本機記憶；AI-Sister 在本機執行，再把命中的螢幕文字原文、時間、app、
   視窗標題與網址交回同一支 CLI 作答。永不送 pixel，原文不遮；沒簽就一次都不 spawn。
3. **畫面暫存**：我同意保留變化幀的截圖，而不是只留上面的字。沒簽仍可記 OCR，
   但一張截圖都不寫。
4. **Azure TTS**：我同意在設定裡開啟 Azure 新答案自動朗讀時，每份新答案完成後不再逐次詢問，就把該答案正文原文交給我在設定裡選擇區域的 Microsoft Azure 語音服務並自動播放。正文可能含姓名、電話與金額，不會先遮罩；
   不會送出截圖、來源連結、memory id、整份資料庫或其他文字。沒有這一張，她一次都
   不會呼叫 Azure 語音服務；本機朗讀不受影響。

四張各自獨立。第二張以獨立 terms version 讓 alpha.126 前只涵蓋既有候選成句的簽名失效，
不撤回第一／第三張；第四張只鑄出 Azure TTS permit，不能借第二張、Persona 下載點擊或
Persona `voice_enabled` 代替。alpha.109 讀同版本、但尚無 Azure 欄位的舊 consent 時，
前三張簽名保留，第四張明確遷移成未簽；alpha.110 再以第四張獨立 terms version 讓
click-only 舊簽名失效，只需重簽第四張。未知／損壞／版本不符仍 fail closed。
第一次互動時，主視窗在既有對話氣泡依序顯示 native 條文與未簽後果，只接受輸入框中的
「同意／我同意」或「不同意／我不同意／先不要」。每張回答立即走既有 atomic consent
transaction；保存成功才前進，失敗留在原張。授權 timestamp 與「目前條文已回答」分開
保存，因此不同意不會取得 permit，也不會每次開機反覆追問。設定齒輪與系統匣仍可開完整
四張卡片查看、重簽或撤回；條文版本變更只令對應的 reviewed version 失效。

### 11.2 Capture 時排除（不是事後刪）〔定案〕

- App/URL blocklist（預設含密碼管理器、網銀常見 domain 樣板）；
- 隱私視窗（incognito）偵測即跳過；密碼欄位（UIA SecureText / AXSecureTextField）
  永不 OCR；螢幕分享/會議 app 前景時自動 pause（旁人畫面防線）；
- 剪貼簿秘密偵測（高熵字串/`sk-` 類 pattern）→ 不落地，只記「複製了一個秘密」事件；
- Windows Release 1.0 最低 Windows 10；WTS 只有 active + unlocked 放行。Unknown、
  disconnected、狀態矛盾或讀取錯誤一律停在內容前。lock 與 power 正交，wake 不等於
  unlock；目前只從相鄰 polling 樣本產生觀察到的 lock／unlock，不宣稱捕捉每個事件。

privacy observation 要鑄出綁 exact native window／PID 的 capture permit；slow UIA
之後、剪貼簿 bytes staged 後、screen frame 取得後都重驗 permit 與 system state。
前置閘門命中時不讀內容；race 發生時 clipboard bytes／pixels 可能短暫在工作 RAM。
clipboard 後驗不通過時，staged event 與該次 focus 不進 DB；screen 後驗不通過時，
frame 不進 dedup、OCR、frame DB 或 PNG（較早已安全驗過的 focus audit 仍可能存在）。
多筆 system transition audit 以單一 SQLite transaction 寫入；失敗時完整 observation
留在 recorder 行程 RAM，下一拍先 retry、成功前不 poll 新事件或讀內容。這不是
durable queue：transaction 前 crash 仍可能失去 pending observation，不宣稱
crash-safe exactly-once。

Windows GDI 截的是前景所在的**整個 monitor**。以上 foreground app／URL／title／
sensitive-field 規則不會遮掉同螢幕可見的背景敏感視窗；那是明示殘餘風險。

剪貼簿來源不得以讀完後的 current foreground 回填。browser copy → 立刻 switch 時，
目前沒有可靠的 origin URL／title proof；有任何 URL rules 且來源 app 是瀏覽器時，
alpha.103 保守丟棄這類 clipboard content，不拿下一拍的安全脈絡替前一拍背書。

### 11.3 出境內容（上雲前）〔2026-08-26 改：不去敏〕

**送出去的是螢幕上的原文。** 金額、電話、人名、編號都照原樣進 prompt。

原本這一節寫的是 typed placeholder（`money/phone/email/id_like` 換成
`<AMT_1>`，人名遮蔽預設開），也真的做出來過（alpha.57）。拿掉的理由：

- **記憶是長期的，代號不是。** 代號在每一次呼叫裡重編，`<PERSON_1>` 在這一段
  和下一段不是同一個人。§6 的承諾表和 entities 要的正是「王小明」這三個字能
  跨段對得起來——去敏等於先把 L3 的地基拆掉。
- **會用這個產品的人，本來就不會想去敏。** 她的價值是記得你的生活，
  記成一排代號就沒有價值了。

**代價說在前面，不藏**：出去的是原文，所以那支 CLI（以及它背後的模型商）
看得到你螢幕上的字。這件事寫在第二張同意書的條文裡，他按的就是那句話
（`consent.rs` `VERSION = 3`），不是寫在某份文件的第 11 節。

沒有改變的：**畫面一粒 pixel 都不出去**；在 L2/L3 解釋路徑上只有 OCR 抽出來的字，
沒簽第二張同意書一次都不送；剪貼簿秘密偵測（§11.2）仍然不落地。alpha.109 的
Azure TTS 是 §11.10 另外一條只送當前答案正文的明示路徑，不借這張同意書。

### 11.4 保留與磁碟邊界

現行 TTL（預設）：畫面 PNG 30 天；OCR/L1 文字 365 天；沒有一個程式其實未產生的
「90 天縮圖層」。資料庫與 frame **沒有應用層加密**，依賴 BitLocker／FileVault／LUKS
等 OS 全碟加密；匯出檔也不會自己加密。這個邊界要在產品裡明講，不能再用 SQLCipher
選型表暗示已防離線竊碟。已有一鍵 pause、時間軸區間刪除與 cascade；panic wipe 仍是
另列、未完成的產品工作。alpha.118 已完成跨 capture／brain／hands 的單一「全部停止」：
命令先拒絕新工作，再等已准入的 capture tick、CLI agent、reviewer product mutation 與
hands OS call 排乾，durable latch 發佈後才回成功；恢復不會順手解除原本的 pause 或拔手。
若既有 CLI provider call 已送出，停止會等它最長 120 秒與本機 outbound audit 收尾，
不宣稱能撤回 provider 已收到的 request。

### 11.5 旁人問題（誠實聲明）〔定案為無技術完解〕

她錄的不只是你——朋友的訊息、客戶的文件會進你的本機資料庫，他們沒同意過。
產品立場：(a) 會議/分享自動 pause（§11.2）；(b) 通訊 app 可一鍵入 blocklist；
(c) 文件（PRIVACY.md）誠實陳述此邊界，
不假裝解決了。法域註記：部分地區對「記錄他人通訊」有法律風險，文件明示。

### 11.6 供應商端留存（第二、第四張同意書的誠實註腳）

螢幕文字上雲後仍受各模型商 abuse-monitoring 留存政策約束。而且它是原文
（§11.3），所以這一節比原本更重要。對策：
(a) 文件列出各 provider 的留存/zero-retention 選項，預設推薦有 ZDR 的通道；
(b) 「外送紀錄」面板列出可驗證的命令、角色、字數、時間、結果與截斷狀態；不另外
複製一份敏感 OCR 原文來假裝審計更完整。要看下一次會送什麼，走 dry-run。
Azure TTS 的當前答案正文與一般網路 metadata 交給 Microsoft 後，則受使用者 Azure
帳號、resource、方案與 Microsoft 當下政策約束；AI-Sister 不把 F0 額度或 provider
留存寫成產品自己的保證。它不另做文字／MP3 cache，詳見 §11.10。

### 11.7 可驗證性（Recall 的教訓：「宣稱本機」不夠）

同意書 fail-closed、capture-time 排除、零遙測掃描、可見的錄製狀態、
可驗證的 pause／刪除／匯出，以及 **master stop 一鍵全停**是可驗證面。
磁碟由 OS 全碟加密保護；應用程式不宣稱未實作的自動鎖、keychain 或離線 DB 加密。
誠實邊界：**同使用者 session 內的 malware 不在防護範圍**——userland OSS
做不到 Recall 的 VBS enclave + TPM 綁定，README 明講，不假裝。

### 11.8 資料主權（Rewind 的教訓：closed product 的退場 = 記憶滅絕）

**開放資料格式**：SQLite schema 公開文件化、`sister export` 全量匯出。S1 記憶功能、
17 套角色圖、544 段日常語音與 68 段（4,542,053 bytes）同意書朗讀不依賴我們的伺服器；它們都隨 desktop 安裝。舊 Persona
素材 pack 的取得仍須使用者明確發起 CDN 下載。就算本專案或 CDN 消失，既有記憶仍可讀、
匯出，bundled 與已驗本機素材也仍可用。
素材 cache 固定在 `Config::default_data_dir()/persona-assets-v1`，不隨 `--data-dir`
搬動，也不是記憶 export 的一部分；`sister forget`／`prune`／memory export 都不碰它，
只有 Persona 撤回流程精準刪除該 release。

### 11.9 遙測

**零遙測。** Release 1.0 不內建 Cloudflare D1 或其他 usage counter。544 段 bundled
日常語音與 68 段同意書朗讀只走同源本機檔案，不建立 request。舊 Persona 的固定 asset-pack GET 是使用者
當下發起的內容下載，不是遙測；它仍須在按鈕前揭露 DNS／CDN
能看到的一般網路 metadata：CDN 會看到來源 IP、時間、TLS、固定 host／path／headers。
請求不得夾帶角色選擇、使用狀態、OCR、畫面、問題、答案、記憶 ID 或資料庫內容。
首版四位角色共用同一個 omnibus pack 與 exact hash path，切換 persona 不改 method、
URL、header 或 body；不 follow redirect、不走 proxy、不帶 cookie／credentials／
authorization／referrer／query／body，不 retry，也不發 `HEAD` 或逐物件請求。未來若改成
分包，必須先把 path 可透露哪一包寫進揭露，不能沿用「不帶角色選擇」的舊承諾。

Persona transport 自己唯一的 allowlist 是 `https://cdn.ted-h.com` 加上
`/tokenmonster/characters/v1/packs/ai-sister-media-11-voice55-2026.07.23/7d98e0d18c470f82818e8ada67208847c3cf4ff5c10cb5f99f9215191e981f30.zip`。
compact embedded authority 只含 descriptor、exact origin/path allowlist、四位角色的
selected public rights projection、八段選用聲音的核准逐字稿，以及完整 schema-v2
manifest 的 canonical SHA-256
`21e4675653ce66b50b61e91260f1623e6e3005177f900991e3a8eeadaf9e6474`，**不嵌入約
2.1 MB 的完整 11 人 manifest**。descriptor cross-bind release ID、canonical manifest
hash、73,261,088 bytes、946 entries 與 pack SHA-256
`7d98e0d18c470f82818e8ada67208847c3cf4ff5c10cb5f99f9215191e981f30`；整包 digest pin
住 946 項，實際使用的 selected entries 另逐檔驗 size／hash／rights binding；只有
renderer 顯示文字逐字等於 embedded transcript 的 line 才能進播放 allowlist。pack 中
需要「今天滿有活力」前提的四段 `active` WAV 不在首版 selected set。

### 11.10 Azure 可選 TTS

alpha.109 加入第二條 desktop 內建 outbound；alpha.110 把 opt-in 後的觸發改成每份
使用者新問題的最新答案完成時，自動朗讀一次**該答案正文原文**。本機 `localService`
語音仍是預設；Azure 預設 `enabled = false`、
沒有預設 region，也不會在本機 voice 缺席／失敗時自動 fallback。反方向也一樣：
Azure 失敗不自動改走本機或其他 provider。Azure 未 ready 時答案完成仍是 0 request；
不論 ready 與否，開 app、開設定、status 重讀、demo、舊答案重畫、選 Persona、錄製或
記憶事件本身都是 0 Azure request。

真正 POST 前必須同時具備：設定明確啟用；region 是 `eastasia`、`southeastasia`、
`japaneast` 之一；voice 是三個 canonical zh-TW allowlist 值之一；目前 Windows 使用者
Credential Manager 的 fixed target `ted-h/AI-Sister/AzureSpeech/v1` 有 subscription
key；以及現行第四張 `azure-tts` consent 有效。四道 gate 齊全時，只有最新使用者問題
的答案完成或 trusted 手動重播能建立 transport；前者每題恰好一次，後者會另送一次。
缺一項就不建立 transport，也不能借 Persona `voice_enabled` 或第二張 `cloud-reading` 通過。

native Rust 只能對所選區域的
`https://<region>.tts.speech.microsoft.com/cognitiveservices/v1` 做一個 HTTPS `POST`。
region／voice 是 typed enum，未知值令設定 fail closed；renderer 不傳 URL。request
不 follow redirect、不走 proxy、不 retry；WebView 繼續只有 IPC，CSP 不加入 Azure。
每次 POST 的唯一使用者內容是當前答案正文原文，可能含姓名、電話與金額而且不先遮罩；
不得加入截圖、來源連結／出處 chip、memory id、整份資料庫、問題、舊答案、Persona
台詞或其他 UI 文字。

subscription key 不進 `config.toml`、log、DB 或 export；輸入時會短暫存在 password
欄位、renderer 記憶體與 Tauri IPC，保存後頁面立即清空。native 回條只帶
Present／Missing／Unreadable／Unsupported，不把已存 secret 讀回 renderer。非密設定 `[shell.azure_tts]` 只存
`enabled`、typed `region` 與 typed `voice`。Windows Credential Manager 是目前使用者
帳號的 OS 邊界，不宣稱能抵擋同使用者權限 malware。

不做文字或 MP3 的磁碟 cache，也沒有可供重播沿用的記憶體 cache；每份新答案與每次
trusted 手動重播都可能是新 POST。Stop／換題／新播放會先讓舊 playback generation 失效，晚回的 MP3 不播放、
不快取；但已取得送出權的 blocking native POST **不能中途 abort**，仍可能跑到 45 秒 timeout，
而且 Azure 可能已計入用量。UI 不得把「不再播放」寫成「請求已取消」。
第四張 consent 的 admission 與 CLI／desktop 撤回共用跨行程 lock，而且該 shared guard
保留到既有 transport 結束。native 的最後一次 generation 檢查與 transport 另和設定、
key、consent mutation／cancel 共用同一個 fence：transport 先取得送出權時，這些操作可能
等到 45 秒 timeout；但任一操作一旦回覆成功，舊 snapshot 不可能才開始另一個 POST。
renderer 在等待 native cancel 時仍須先停止播放。

Microsoft 目前公開列 Azure Speech F0 neural TTS 每月 0.5 million characters；是否
可用、實際額度與費用以使用者 Azure 帳號、resource、方案及 Microsoft 當下規則為準。
AI-Sister 不提供、不保證這份免費額度，也不把它當費用上限。

## §12. Replay 評測（第一級公民，不是附件）〔定案：全場唯一無異議的下一步〕

- **Recorder**：capture 層本身即 recorder；`sister replay export` 打包一段
  L0/L1 流為語料（自動去敏 + 手動審查後才可分享）。
- **語料**：Ted 自錄真實工作日（含：寫 code、查資料、閃過帳單、LINE 通知、
  中途改目標）≥ 2 週；埋題語料（腳本化植入「兩小時後要問的事實」）。
- **題庫**：真實提問全記錄（query log 就是題庫）+ 手標：recall QA（有 ground truth）、
  承諾集（該提醒什麼/何時）、開口判斷集（該講/不該講的時刻標註）、斷句邊界標註。
- **指標**：找回率@k、答案正確率、出處正確率、檢索延遲、誤提醒率、漏提醒率、
  斷句 F1、回查率、$/天、CPU/RAM/電池。
- **A/B 架構比較**〔定案：不辯了，用跑的〕：
  `baseline`（純 FTS 檢索）vs `+facts` vs `+interpreter 薄層` vs `+reviewer`。
  interpreter 沒有在題庫上贏 baseline 就不加厚〔定案：沒贏就不要擴到八隻〕。
- **公開**：benchmark 數字進 README——「市面上沒人敢公布這種數字」是護城河之一。

## §13. 成本模型（2026-08 實價試算，詳表見 `research/cost-model.md`）

辯論五輪沒人給出的那個數字，答案是：**「$30 級」，不是 $3 也不是 $300。**

| 配置 | 月費（8h×22 天） |
|---|---|
| 暴力輪詢（每 10s 一呼叫，已判死的反面教材） | Haiku $190 / Sonnet $380 |
| 收斂架構（事件驅動 interpreter + 3 passes/hr reviewer） | **$24–34** |
| 優化檔（adaptive reviewer 1/hr + 夜間 batch） | **$7–21** |
| 激進下限（最便宜供應商 + 降頻） | ~$7–8 |
| 全本地（5090 / M 系 NPU 跑 Qwen3-VL 級） | $0（品質換免費） |

- 結構性事實：**事件驅動本身 = 14× 削減**，比一切 caching/batch 技巧大一個數量級；
  **Reviewer 深度層佔總成本 60–80%**——adaptive cadence（活躍才跑、閒時降頻）
  是最重要的成本旋鈕。
- 預算約束：預設檔位目標 **< US$20/月**，設定頁有月費估算器 + 硬上限
  （到頂自動降到本地/純檢索，功能不中斷）。
- 本地混合省的錢其實不多（$3–13/月）——**選本地的理由是隱私與離線，不是省錢**。
- 供應商 gotcha：Haiku 4.5 prompt cache 最短 prefix = 4,096 tokens——
  interpreter 的 cached system prompt 要墊到 4k 或改用 Gemini/OpenAI 自動 caching。

## §14. 非功能需求

- Windows current-user 安裝版提供 opt-in 登入啟動，**預設關閉**；這是登入時的
  HKCU Run 登錄，不是 service／updater，不承諾 desktop crash 後 self-relaunch。
- recorder 復原只由 desktop 對自己啟動的 child 做 bounded watchdog/backoff；
  core 與 shell 沒有各自復活另一份行程，也不接管外部 recorder。
- 升級不丟資料（SQLite migration 版本化）；Release 1.0 的 Windows artifact 由使用者
  手動下載新版 installer、結束 desktop／recorder 後原地安裝，不內建自動 updater；
- i18n：zh-TW / en day one（MAT i18n 骨架）；
- 可觀測：開發者模式面板（L2 卡片流、回查 log、開口候選與分數）——
  預設關閉〔定案〕；
- 所有內部 Tauri IPC 使用 strict serde DTO；前端對封閉集合做窮舉檢查。

**Windows installer admission（alpha.115）：**

- Tauri stock `CheckIfAppIsRunning` macro 必須由產品 hook 覆寫。本版產生的 Setup／uninstaller
  無論 GUI、passive 或 silent，對 legacy scan 的 **reported hit** 都只准拒絕，不顯示
  「替你關閉」選項，也不呼叫 kill。`FindProcessCurrentUser` 只有 found／not-found，沒有
  Unknown；process snapshot、OpenProcess、token 或 SID 查詢失敗會和未找到合併。它仍掃所有
  matching image name，作為向後相容與 pre-main fallback，不能當 aware admission authority。
- Windows bundle 固定使用 pinned tauri-bundler 2.9.4 custom NSIS template。Setup 在 `.onInit`
  建立固定名稱的 lifecycle mutex，成功 mutation path 持有到 `POSTINSTALL`；它**絕不巢狀執行
  已安裝的 NSIS uninstaller**。同版 repair 或由舊版升新版時，install root 只能綁定
  current-user `${MANUPRODUCTKEY}` 的 exact root，再由 Setup 原地覆蓋；版本與移除程式則由
  `${UNINSTKEY}` 的 `DisplayVersion`／`UninstallString` 交叉核對，不接受命令列 `/D` 把既有安裝搬到別處。
- GUI、passive `/P` 與 silent `/S` 都須在 WebView2、payload 或安裝登錄 mutation 前，重新讀取
  並核對 installed `DisplayVersion`、exact install root 與 exact quoted `UninstallString`。
  metadata 缺失、改變或互不對應時 fail closed；若 Setup 版本低於已安裝版本也不得啟動新版
  uninstaller，使用者必須關閉 Setup，再從 Windows「已安裝的應用程式」分開移除。
- direct uninstaller 讓確認頁先完成，在任何移除 mutation 前的 `PREUNINSTALL` 才取得 mutex，
  並重新核對自己編入的 exact version、目前 executable parent root 與 quoted uninstall string；
  成功 mutation path 持有到 `POSTUNINSTALL`。因此使用者停在確認頁時若另一份 Setup 已完成
  升級，舊 confirmation 不能接著刪除新版。scan hit 或 metadata refusal 都必須先釋放自己取得的
  handles；使用者取消或 process 結束時由 OS 回收。
- alpha.113-aware desktop 仍先解析窄的 launch intent，接著在 logging、data-dir、Tauri
  plugin／WebView、DB 與 recorder admission 前依序「探測 installer mutex → 建立 shared
  product event → 再探測 mutex」，成功後持有 event 到行程結束；`sister.exe` 在 clap、logging、
  data-dir、config、DB 與 AI-Sister 記憶／設定之前做同一件事。installer 取得 mutex 後探測
  product event；只有 kernel 明確回答對向 object 不存在才繼續，present、close failure 或其他
  native error 都 fail closed。這個 mutex／event／mutex handshake 才是 aware-product 的雙向
  admission；image-name scan 不是。
- installer 探到 product event 時，silent `/S` 仍在 mutation 前 exit 32。GUI 與 passive `/P`
  顯示「重試／取消」：Setup 持續握住 installer mutex，使用者從系統匣結束 desktop（或回終端機
  停掉 AI-Sister 指令）後按重試，Setup 必須重新 `OpenEvent`；只有新量到 Missing 才繼續。
  取消先釋放 mutex 再 exit 32。這條路不要求、也不執行 forced kill。
- 原生 Windows release gate 除既有 `/S` admission lanes，另以 `/P` 真正進入
  `PageReinstall`，使用 alternate `/D` 並把已安裝 `uninstall.exe` 換成 `PING.EXE` child witness，
  斷言 Setup 綁回 registry root、沒有執行 child，且原地恢復正確 payload／metadata；另以 `/S`
  downgrade fixture 斷言在 mutation 前退出。這些是 native automation，不是滑鼠真人互動。
  acquire／after-scan fixtures 仍只證明一般可列舉同名 process 的 reported-hit/no-kill，沒有驗
  scanner 的 snapshot／token／SID error。
- alpha.117 file-level exclusion：Setup 在 legacy scan 之後、NSIS `File` 之前，對兩個已安裝 exe
  各開一個 `GENERIC_WRITE|DELETE`、只分享 delete 的 handle。正在執行的 image 會讓開檔以
  sharing violation 失敗（與行程名字、版本、有無 product event 無關）→ exit 32、不 kill、零
  mutation；拿到就先改名 `.ai-sister-previous`、handle 持到 `POSTINSTALL`，所以最後一次 scan 到
  `File` 之間 loader 映射不到任何版本的 exe。direct uninstaller 在 `PREUNINSTALL` 重驗後、刪檔前
  同一道。這補上 old-binary bridge：Windows CI 以 hard link 別名執行不持 event 的公開 alpha.110
  recorder，current Setup 仍拒絕。仍不能 retroactively 改寫已出貨／temp 裡的舊 uninstaller，也不
  管安裝根目錄外的 portable 副本。

**Windows 登入與 recorder lifecycle（alpha.107）：**

- 唯一設定真相是
  `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` 下的 `AI-Sister`。缺值是
  `disabled`；只有 `"<current sister-desktop.exe>" --ai-sister-login` 完全相符是
  `enabled`；其他可讀值是 `mismatch`，無法讀取是 `unreadable`，無法用
  uninstaller `InstallLocation` 精確證明 current exe parent 的 portable／其他副本是
  `unsupported`。不把 unknown 用 bool off 代替；寫入後必須讀回 desired exact 狀態才算
  成功。只有已驗證安裝副本可修改，portable 不碰另一份安裝值。
- app 不讀寫 Windows `StartupApproved`。`enabled` 只代表 exact Run value 已登錄；
  作業系統「啟動應用程式」仍可另外停用。關閉登入啟動只影響後續登入，
  不終止這一輪 recorder。
- `--ai-sister-login` 是窄的 login intent：tray-only，不 show／focus／onboarding。
  每個 desktop worker 只承接第一份 login intent；delayed／secondary duplicate 忽略且不
  reveal。有效 `local-recording`
  consent 才送 recorder start intent；未簽、讀不到或版本失效時不啟動也不彈
  onboarding。登入啟動與重試都不得解除 pause。`local-recording` 從有效變撤回時，
  CLI 與 desktop 都在 consent save **前**先 atomic publish 獨立的
  `consent-revoke.barrier`。它不由 recorder 消費、不准 Start 清除；即使 save 失敗且舊
  consent 仍有效，也必須阻止 automatic／Explicit start，且只能說停止條件已收到，不能
  冒充 consent commit。成功 local regrant 才能用 transaction 內捕捉的 generation ticket
  清同一代；較新 revoke 會令舊票 `Superseded`。清前若沒有普通 stop marker，先留下
  `consent-revoked` stop latch；既有 `requested`／`desktop-quit` 原封保留。login／retry
  永不清人工或撤回停止，也不
  重設同一個 worker 的 backoff／GaveUp。Login transient preflight 每 500ms 完整重查，
  總期限五分鐘且不計入 watchdog failure budget。每次 attempt 前與最後 commit 前都要
  檢查 deadline，逾時不得 clear 或 late spawn。只有 typed `desktop-quit` 可在期限內重試
  上一輪 shutdown handoff；stop 是 Absent 卻觀察到任何 lease／heartbeat owner 證據時
  必須轉 External，該 owner 之後離場也不得由同一份 Login takeover／restart。一般
  External 的 stale／missing／unreadable 仍只接受 `stopped` 墓碑為離場證據，
  `desktop-quit` 是窄例外。Stop、Quit、真人 Explicit Start、`requested`／
  `consent-revoked`、invalid／unknown consent 或 control 都取消 pending Login；同一 worker
  不再接受第二份。正常退出另寫 `desktop-quit`，只有下一次全新的 opt-in login 可在
  bounded handoff 與完整 start barrier 後清它。desktop 在 spawn 前重讀 consent／stop；
  每個 recorder 在第一拍 heartbeat 前又重讀一次，不拿先前的 allow 結果走進第一拍。
- 同意更改本身也必須可線性化：所有 CLI／desktop grant／revoke 先對 data-dir
  空檔 `consent.lock` 取得 OS whole-file exclusive lock，**在鎖內**重讀最新
  `consent.toml`、只套用當次指定的 sheet 變更，再 atomic save。這條專門防止
  simultaneous CLI／desktop 各自從舊 snapshot 寫回，把對方已 grant／revoke 的另一張
  無聲蓋掉。撤回 local-recording 的獨立 barrier 在 locked atomic save 前發布；regrant
  只有 commit 後才可 generation-safe clear。鎖或 atomic save 失敗就回錯，不謊稱已儲存；已存 lock path 是
  symlink／non-regular file 時也 fail closed，不跟著鎖到資料目錄外。
- Recorder start 必須先拿 shared `consent.lock` start guard，再依序拿 `stop.lock`、
  `recording.lock`／檢查舊 heartbeat。CLI recorder 的 guard 活過 explicit clear 與第一拍；
  desktop parent 的 guard 活過 preflight／clear 到 `Command::spawn` 回來便立即放掉，child
  自己 nonblocking 重拿並持到第一拍。revoke writer 先取得 exclusive consent lock 時，
  這次 start 必須零 clear、零 heartbeat、零 spawn；start 先取得 shared guard 時則明確
  線性排在 revoke 前，晚到的 barrier 仍使 recorder 收工。不得用鎖外的舊 consent
  snapshot 穿過 writer，也不得讓 parent 等 child heartbeat 才釋放 writer。
- desktop-owned child 連續失敗的第一、二、三次分別在 1 秒、5 秒、30 秒
  後重試；第四次放棄。只有**新鮮且相對上一個已觀察樣本真的更新的
  `Recording` heartbeat** 能推進連續區間；反覆 poll 同一拍不提供新證據。區間達
  10 分鐘才 reset failure count；`Booting`、missing／stalled／unreadable／`Thinking`
  一旦被觀察到就把區間歸零，不能從中斷前繼續累加。exit 0、manual stop、
  consent revoke 與 desktop quit 取消 retry。worker 一收到 manual Stop 就必須先取消
  Login、retry timer 與所有 automatic spawn，再嘗試 durable stop write；write failure
  不得恢復 automatic work，且 UI 不可宣稱現有 recorder 已停。修復後仍可再 Stop；只有
  真人 Explicit Start 成功 commit 完整 barrier 才解除這道 in-memory latch，failed／busy／
  invalid／timeout Start 都不算。
- quit 先留 durable stop 才可讓 desktop 退出。如果 stop write 失敗，worker 仍立刻進
  in-memory `Quitting` 並取消 timer，但 desktop 必須 prevent exit、顯示／focus 錯誤，
  不能讓可能仍在錄的 child 留在沒有 UI 的背後。修復磁碟／權限後才能再次
  stop／quit；這期間不可接新 start 或 retry。
- retry 前必須重新測 stop intent、consent 與 occupancy。unreadable／unknown 不猜，
  occupied 不 spawn 第二個；觀察到外部 recorder 後放棄自己的 takeover。desktop
  本身崩潰時沒有另一個 service 把它重開。
- owned child exit 的 wall-clock cutoff 以前留下的 fresh heartbeat 不得投影成目前仍在錄，
  也不得開放 Start／retry；cutoff 之後繼續更新才進 typed `External` observation，取消
  舊 retry 且只委派 heartbeat 顯示。cutoff 後才變 stale 的一拍證明曾有 external owner，
  卻不證明它已離場；missing／stalled／unreadable 都維持 external uncertainty，只有明確
  `stopped` 墓碑才可完成 `ExternalGone`。已送 external stop 時同樣維持
  `ExternalStopping` 到這份墓碑，不能拿 timeout 當停止證據。`try_wait` error 進
  child-uncertain 並保留原 handle，後續成功 probe 可回 Running／正常 exit，不可因一次
  probe error 永久 wedge 或另開 child。
- `stop.request`／`stop.consumed` 的 request、consume、explicit clear 以永久 `stop.lock`
  exclusive transaction 線性化；read probe 用 nonblocking shared lock，contention 也是
  unknown。CLI Explicit start 必須先持 shared consent start guard，再在 recorder lease 前
  持 stop transaction，通過 lease 與舊 heartbeat barrier 後才 clear。Desktop 真人 Start 做同一件事：parent nonblocking
  取得 consent guard、stop transaction 與 temporary recorder lease，通過 heartbeat barrier 並
  commit 後才 clear；spawn 的 child 一律是 supervised，重新取得正式 lease 並在第一拍前
  重讀 stop／consent，沒有稍後無條件 clear 的能力。Login 只可在全新 worker 以同一套
  barrier 清上一輪 `desktop-quit`，人工 `requested`／`consent-revoked` 都保留；retry 永不
  clear。Windows handle 拒絕 delete；Unix Preview 執行中不得 unlink／replace 三個
  protocol lock path。
- heartbeat 是診斷與 supervisor 證據，不是 simultaneous start 的原子鎖。每一個 CLI／
  desktop recorder 都要在第一拍 `Booting` heartbeat 前用 fs4 對 data-dir
  `recording.lock` 做 nonblocking OS whole-file exclusive lock，整場持有。拿不到或鎖狀態
  不明就不啟動，已存路徑若是 symlink 或 non-regular file 也拒絕；process
  crash／handle drop 時由 OS 釋放。空的 lock file 可以留在
  磁碟，**檔案存在不表示 occupied**，只有當下 OS lock acquisition 的結果才是真相。
- 純 command／state policy、watchdog 轉移、renderer fixture、recorder lease 與
  consent locked mutation 有可在 Linux 執行的自動測試；Windows backend 另有只碰
  test subkey 的 native registry test。這是決策、lock protocol 與 API
  read／write／readback 的自動證據，**不是**正式 alpha.107 安裝副本真登入、
  真故障時序或 uninstall 的人工通過紀錄；這些仍待 Windows checklist 勾驗。

## §15. 技術選型

（版本皆 2026-08-17 於 crates.io/官方驗證，詳見 `research/tech-stack.md`）

| 元件 | 選型 | 備註 |
|---|---|---|
| core runtime | **Rust** crates + `sister` recorder/CLI；desktop 以 sibling process 啟動 recorder、以 Tauri IPC 進 Rust backend | 無 loopback server；macOS capture 必須簽進 `.app` responsible process tree |
| 截圖 | Windows 10+：Win32 GDI（BitBlt／GetDIBits）；macOS Preview：ScreenCaptureKit；Linux Preview：X11，Wayland 只保留實驗性 portal 路線 | alpha.125 三條 production backend 都已落地；macOS 公開 artifact 仍要等 notarization 與原生 TCC/S1 收據 |
| 去重／OCR gate | 自製 64-bit dHash（預設 Hamming ≤5 視為近似同幀）+ 64×64 RGB tile FNV-1a；Windows Phase 0 的近似重複幀只讓能一對一拼回舊閱讀順序的 crop 升格，其餘維持重複；dHash 新幀的局部證據不足才退回全幅 | 沒有使用 DXGI dirty rects |
| OCR | macOS：Apple Vision accurate；Windows：`Windows.Media.Ocr`，見 §14.1；Linux X11 Preview：隨 `.deb` 依賴安裝的本機 Tesseract（繁中／英文） | 搜尋前全半形正規化 + OpenCC 繁簡歸一；三條 OCR 都不由 recorder 下載模型 |
| DB | SQLite 3.53（`rusqlite` 0.40，WAL）+ **FTS5 trigram + unicode61 + bigram 三索引**（external-content table；trigram 補 CJK 子字串、unicode61 補英文整詞、`text_fts_bi` 補**兩個字的中文**——unicode61 把整串 CJK 當一個 token，`MATCH "客服"` 是 0 筆，schema 3 之前只剩夾在 30 天內的 LIKE 掃描。bigram 是粗篩，命中要拿真字串再驗一次；只剩單字查詢仍走掃描） | 之後要拼音再上 `simple` tokenizer |
| 向量（選配） | `sqlite-vec` 0.1.9（2026 復活版；256-d int8 MRL，brute-force 在我們規模內互動級） | pre-1.0 格式風險 → 存 model-id+dim，設計成可背景 re-embed |
| 本地 embedding | 遠期選配；Release 1.0 沒有內嵌推論 runtime | 腦優先 spawn 使用者已登入的 CLI；沒有 HTTP client |
| 磁碟保護 | SQLite/frame 無應用層加密；依賴 BitLocker／FileVault／LUKS | 未開 OS 全碟加密時，離線竊碟者可讀；PRIVACY／THREAT_MODEL 明講 |
| UI shell | **Tauri 2** Rust backend + build-free HTML/CSS/ES modules；tray + global-shortcut 已落地 | alpha.106 新增 Windows startup guard + 官方 single-instance receiver 與 current-user NSIS；alpha.107 新增預設關閉、installed-copy-only 的 HKCU Run 登入啟動，以及只管 desktop-owned child 的 bounded recorder supervisor。原生 Windows CI 已驗 alpha.106 install mechanics；alpha.107 有 policy／fixture／test-subkey 自動測試，但正式 artifact 的真登入／重試／uninstall 仍待人工證據。alpha.112 加入從最後一個有公開安裝檔的 alpha.110 → current 的真跨版 installer gate，並明確以 UTF-8 解碼 native CLI stdout；alpha.111 tag 的 Windows gate 曾因此失敗且沒有公開 release。alpha.115 固定 tauri-bundler 2.9.4 custom NSIS template：Setup 不再巢狀執行 installed NSIS uninstaller，同版 repair／升版綁 exact registry root 原地覆蓋，direct uninstall 在 PRE 重驗 version／root／string；product event + mutex／event／mutex handshake 繼續保護 aware binary，legacy scanner 仍是沒有 Unknown 的 best effort。alpha.117 以 `File` 前的檔案層獨佔開檔補上 old-binary bridge 與 loader／`File` 窄窗。alpha.120 接完 tag-only public-CA PFX、四層 SHA-256 Authenticode／RFC 3161 trust 驗證與 release receipt；stable tag 不接受 unsigned，正式 CA identity 尚待配置。1.0 不內建自動 updater |
| Pet overlay | always-on-top 透明無框窗 + `set_ignore_cursor_events` 動態 toggle（Tauri 無 per-region hit-testing，所以是整扇窗的開關）；輪詢游標，另外由 renderer 的 `pointermove` 把那一拍提早叫醒| 已知坑：macOS production 透明窗 bug 群、全螢幕 space 需動 collectionBehavior、Wayland overlay 品質差 |
| macOS 權限 | CoreGraphics Screen Recording preflight/request + AX trust；設定頁 trusted click 開 exact System Settings（無 entitlement，TCC + hardened runtime + notarization） | 公開 Preview 前以 notarized `.app` 重跑授權、撤回與 S1 |
| hands | **Rust crate `sister-hands`**，CLI／desktop 共用 permit 與 target policy | 尚未做獨立 process；需要時另立 threat-model milestone |
| Persona transport | root workspace 的 **`sister-assets`**；預設 feature 集合不含 `download`，desktop 才明確啟用 | API 不接受 renderer 傳入 URL／header／body／persona 或 memory；Persona 的 fixed GET 與 cache contract 見 §11.9 |
| Azure TTS transport | root workspace 的 **`sister-tts`**；預設 feature 集合不含 `azure`，desktop 才明確啟用 | 預設關閉；第四張 consent、Credential Manager key 與 typed config 齊全時只自動讀最新新答案，另有 trusted replay；三個 fixed region POST、payload、cache 與 cancel 邊界見 §11.10 |
| Schema | Rust serde DTO + 前端封閉集合檢查 | 沒有 Zod／codegen build step |
| Persona assets | 17 人本機 catalog + bundled workplace 分層 rig（active-only decode）+ WebP fail-safe（ChatGPT 預設）+ 每人基本 8／擴充 24 的日常語音 + 每人四段同意書朗讀 | 402 張 selected PNG = 35,140,885 bytes；544 段日常 Ogg = 8,895,060 bytes，另有 68 段 consent Ogg（4,542,053 bytes）與 340 段 banter Ogg（2,249,872 bytes，alpha.132 起，唯一不必先被點到就會出聲的一包）；逐檔 pin text/path/bytes/hash/duration/rights/loudness（alpha.137 起每段帶 integrated LUFS 與 true peak，CI 重新解碼對回 manifest；alpha.138 起 CI 另量頭尾空白，每包至少 99% 兩端在 300 ms 以內；alpha.139 起台詞以外的音節一律重錄不剪，判準是兩個訓練資料不同的 ASR 都聽到台詞沒有的字）；WebView 只從同源 bundled path 播放；recorder/core 保持零網路 |
| hands 元件（Phase 6+） | Agent S3（Apache-2.0）/ UFO²（MIT）/ OmniParser v3 weights（MIT，避開舊 AGPL detector） | 「手」已商品化：用組的，不自己寫 grounding |
| 參考不引用 | Screenpipe（2026-06 起自訂商業授權，僅參考架構；MIT fork point 在舊版）；Everywhere（BUSL，僅 MCP/API interop） | license 判定見 research/landscape.md |

Release 1.0 平台層級〔2026-09-06 決定〕：**Windows 10+ GA**；macOS Public Preview；
Linux X11-only Developer Preview。只有 Windows 是平台支援 blocker；Preview 沒達到
自己的 native capture/OCR/privacy/artifact 最小合約就不發該 artifact，不能拿 replay
冒充。Wayland 背景連續擷取與隱私脈絡不足，明示 unsupported／degraded，不承諾
always-on。capture 從 day 1 走 trait 抽象，但「介面同形」不等於能力未知時可以放行。

### §14.1 偏離紀錄：Windows OCR 引擎（Phase 0）

上表原本〔定案〕Windows 用 PP-OCRv5 via `oar-ocr`。**Phase 0 的實作沒有照做**，
改用系統內建的 `Windows.Media.Ocr`。這一節記錄理由，以及在什麼條件下該改回去。

實際去看相依樹之後才發現的、選型當時不知道的事：

1. **`oar-ocr` 的 `auto-download` 會把 `ureq` 連進 recorder 執行檔。** 那是一個真的
   HTTP client，而且會讓長時間看螢幕的 `sister.exe` 自己下載模型。PRIVACY.md 的
   邊界是 recorder／core／capture／brain 沒有這種能力；Persona 後來獲准的 desktop
   fixed-pack GET 與 Azure TTS fixed POST 都不能拿來替 OCR 開例外。
2. **`ort` 用 `copy-dylibs` 出貨 `onnxruntime.dll`**（~15MB）加上模型（~20MB），
   使用者要下載的就不再是一個檔案。目前 `sister.exe` 是 2.4MB 的單檔。
3. **實作選型當時，Phase 0 寫下的驗收條件是 CPU < 3%、RAM < 400MB**，而
   ONNX Runtime 常駐兩個模型光是 arena 就吃掉大半。這是一個整天都在跑的
   背景程式。這是選型的歷史理由；2026-08-23 起 CPU < 3% 不再是 Phase 0 gate，
   但 RAM 限制與「整天常駐」的前提沒有改變。

上表列出的兩項反對意見，處理方式如下：

- **「CJK 逐字拆詞 bug」→ 已緩解。** 不採信引擎給的整行字串，改由每個詞的
  幾何間距重新組行（`crates/sister-capture/src/ocr_layout.rs`，8 個單元測試 +
  一個真的在 Windows 上跑的 CI 步驟）。
- **「語言包依賴」→ 未消除，改為可見。** 沒裝中文語言包時引擎會安靜地退回
  英文。`sister doctor` 因此印出**實際挑中的**語言並明講後果。這是缺陷被
  攤開，不是缺陷被解決。

**什麼時候該改回 PP-OCRv5**：Phase 0 的七天自錄若顯示繁中準確度不可接受
（尤其小字級、深色主題、反鋸齒），就把它接在 `Ocr` trait 後面作為 opt-in 引擎，
並用 build-time 下載 + checksum 內嵌模型，避免執行期網路。`Ocr` trait 的存在
就是為了讓這個決定可以事後改，而且只動一個檔案。

## §16. 開源與 repo 策略

- License：**Apache-2.0**（PRODUCT §8 的理由）；
- Phase 1 完成即開 repo（alpha 標示、預設 observe-only）；正式宣傳於 Phase 5
  帶 benchmark 數字；
- 必備文件（day one）：README（四張同意書宣言 + benchmark 表）、PRIVACY.md
  （含旁人邊界誠實聲明）、THREAT_MODEL.md、DATA_INVENTORY.md
  （TokenMonster 的寫法沿用）；
- clone → 跑起來 < 10 分鐘是硬指標〔定案：clone 十分鐘要看到桌面姊妹動〕。

## §17. 由本 spec 拍板的辯論懸案（決策記錄）

| # | 懸案 | 判決 | 理由 |
|---|---|---|---|
| 1 | 前線准不准寫判斷 | 預設不跑、保留薄假設層、事件驅動+預算制、A/B 裁決 | §5；判斷永不影響錄製密度，拆掉 Grok 的核心反對；薄卡片保住 ChatGPT 的語意查詢 |
| 2 | 搜尋框先 vs 對照組先 | 搜尋框先（Phase 1），評測緊跟（Phase 2） | 沒有 query log 就沒有題庫；框先活起來才有 ground truth 來源 |
| 3 | 何時刪原始資料 | 冷啟動全留 + 分層 TTL + 容量降級 | 省的是模型出場次數，不是原始證據 |
| 4 | 主動提醒何時上 | Phase 5，冷啟動只開高 precision 類別 | 被動答錯自見、主動答錯會被拿去用——風險等級不同 |
| 5 | autopilot | Phase 7、白名單+可逆+預檢+獨立 sidecar | 記憶會歪的東西不能直接點滑鼠；物理隔離 |
| 6 | 開源時機 | repo 早開、宣傳晚放 | 公開 ≠ 發布，兩事件解耦 |
| 7 | License | Apache-2.0 | 目標是名聲與採用；local-first 不怕託管 |
| 8 | 平台 | ~~Windows → macOS → Linux~~；2026-09-06 改為 Windows GA、Linux X11 Developer Preview 與 macOS Public Preview 可依可驗環境平行推進 | Ted 日用決定 GA；Preview 不假裝等同正式支援，實作順序見 PHASES／handoff |
| 9 | 8/3 兩個數字 | worker pool 參數（預設 4/上限 8）與 Reviewer 雙 pass | 排程真相 + 並聯辯證，數字本身無意義 |
| 10 | 回饋分類 | 兩鍵（結案/其他），三分類內部推斷 | 使用者不會標三層意圖 |
