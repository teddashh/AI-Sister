# AI-Sister — Final Spec

> 版本：v1.0（2026-08-17）。本 spec 是 roundtable 辯論（7 題 × 5 輪）收斂 + 外部研究後的
> 最終技術規格。產品定義見 [PRODUCT.md](PRODUCT.md)，階段規劃見 [PHASES.md](PHASES.md)。
> 狀態標記：〔定案〕辯論已收斂／〔決定〕辯論未解、由本 spec 拍板（附理由）／〔待驗〕要靠 replay 評測回答。

---

## §0. 真相模型（整個系統的憲法）〔定案〕

所有資料屬於四層之一，層與層之間的規則不可違反：

```
L0 原始證據 (Evidence)     — append-only，不經 LLM，不可改寫，可過期刪除（帶墓碑）
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
5. **每一句對使用者說出口的話，都要能沿 L3→L2→L0 點回當時的畫面。**

## §1. 系統總覽

```
┌─ sister-core（Rust daemon，開機自啟，無 UI 也活著）────────────────┐
│  Capture 感官層 ──> L0/L1 落地（SQLite + frame files，全本機）      │
│  Segmenter 斷句器（純程式，事件+相似度+時間上限）                    │
│  Interpreter workers 解釋工作槽（事件驅動喚醒，預算制）→ L2          │
│  Reviewer 批次審閱者（15–30min/次 + 日終盤點）→ L2 修訂 + L3        │
│  Gatekeeper 守門員（開口候選評分、預算、quiet hours）                │
│  Query engine（FTS + facts + 選配向量）＋ loopback API（token 驗證）│
└──────────────────────────────────────────────────────────────────┘
        ▲ loopback HTTP/WS（nonce/session，MAT/TokenMonster 模式）
┌─ sister-shell（Tauri 2：Rust 薄殼 + TS UI）────────────────────────┐
│  字母人/姊妹角色視窗（transparent、pin、dragbar、close→tray）        │
│  對話面板、時間軸瀏覽器、記憶瀏覽器、同意書/設定、開發者模式          │
└──────────────────────────────────────────────────────────────────┘
┌─ sister-hands（Node sidecar，Phase 6+ 才存在）─────────────────────┐
│  MAT adapter 層移植：claude/codex/grok/OpenRouter 官方 runtime      │
│  semi-action 執行器（逐步核准、螢幕驗證、可逆白名單）                │
└──────────────────────────────────────────────────────────────────┘
```

拓撲決策〔決定〕：core 與 shell **邏輯分離**（記錄不因 UI 崩潰中斷；shell 重啟不丟資料）；
hands 獨立 sidecar（權限邊界物理隔離：沒裝 hands 的系統物理上沒有手）。
三者只經 loopback 通訊，對外零 port。
**部署拓撲 per-OS**〔tech-stack 裁決〕：Windows = core 可為真背景 daemon；
macOS = **capture 必須跑在簽名 .app 的主程序樹內**（TCC 不認裸 sidecar；
「常駐」由 close→tray 的 tray-resident app 達成，不是獨立 launchd daemon），
OCR/索引 worker 才走 bundled sidecar。capture trait 抽象不變，宿主程序按平台掛載。

## §2. Capture 感官層規格

### 2.1 訊號源（v1，只用通用 OS API——不做 per-app adapter）〔定案〕

| 訊號 | 方式 | 頻率 | 落地層 |
|---|---|---|---|
| 螢幕 frame | Windows Phase 0 用 GDI 擷取（OCR／dHash 工作幀長邊上限 4096px，超過才等比縮小），再做 dHash 去重；真的留圖時另依 `max_long_edge`（預設 1568px）縮成 PNG | 每拍做完後預設等待 400ms；無輸入時可跳過擷取，但最久 5s 補看一次 | L0 |
| OCR 全文 | 平台原生 OCR（見 §15），只跑「保留下來的」幀 | 隨保留幀 | L0 |
| 前景 app / 視窗標題 | Win32 / NSWorkspace 事件 | 事件驅動 | L0 |
| 瀏覽器 URL | UIA（Win）/ AX（macOS）讀址欄；失敗容忍 | 視窗事件時 | L0 |
| 剪貼簿 | 系統事件；>64KB 截斷；秘密偵測（見 §11） | 事件驅動 | L0 |
| 輸入動態 | 鍵擊/滑鼠**節奏與計數**（永不記內容）、捲動、視窗切換頻率、idle | 聚合 per-10s | L0 |
| 系統狀態 | 螢幕鎖定、電源、網路、通知橫幅（能抓則抓，抓不到靠 OCR 幀） | 事件驅動 | L0 |

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
- 敏感排除（§11.2）發生在**capture 當下**，不是事後刪除。

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

（數字依 `research/tech-stack.md` 論證；這張表是長期產品的 release blocker。
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

輸入：該 segment 的 L1 facts + OCR 摘錄（去敏後，§11.3）+ 工作集脈絡 +
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

## §8. Expression 表達層（字母人／姊妹）

### 8.1 視窗與角色

- TokenMonster pet-window 配方移植到 Tauri：frameless、transparent、skipTaskbar、
  可 pin、dragbar 拖曳、close→tray、bounds 持久化、navigation 硬化。
  已知代價（tech-stack）：Tauri 無 per-pixel 點穿（整窗模式切換）、macOS 需
  `macos-private-api`+`tauri-nspanel` 且有 DMG 透明失效與 **GPU 功耗 8×** 的已知 issue
  → 對策：角色動畫用低 fps 靜態立繪 crossfade（TokenMonster 本來的做法，不是巧合），
  提供「不透明小窗」低功耗模式，透明模式的功耗列入 §2.3 電池預算量測。
- 識別：字母人 letter-avatar（day-one、離線、零資產）→ 四姊妹立繪選配
  （明確同意後 CDN 單次下載；`ai-sister` bucket 現成）。
- 狀態表達（不彈窗）：`idle`（呼吸）／`paused`（閉眼 = capture 停）／
  `thinking`（微動）／`has-something`（微光 + 一個小點，像未讀）。
  點角色 → 對話面板。tap-lines 台詞引擎沿用。

### 8.2 對話（被動答題——永遠可用，這是 1.0 的全部）

- 輸入框隨時可問；查詢管線：意圖解析（1 次 LLM 或規則）→ L1/FTS/（選配向量）檢索
  → 附出處作答（1 次 LLM 潤句；離線模式退化為結果列表）。
- 延遲預算：檢索 < 100ms；成句 < 3s；**> 3s 視為 bug**〔定案：轉圈超過三秒，
  人會自己去翻，然後再也不問〕。
- 每個答案：出處 chips（點開 = 當時畫面）+ 不確定語氣規範
  （「我最後看到的是…」，禁止「你還沒做」式斷言）〔定案〕。

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
   ——僅限使用者點按鈕觸發）→ `semi-action`（逐步核准；對話式「好」= 核准，
   I/O 本來就開著）→ `takeover`（接手模式：白名單任務型別、有邊界）。
   **「好」只核准畫面上顯示的那一步**〔定案：CLI 證明的是互動可行，不是授權夠精確〕。
   授權是結構化物件，不是一句話：
   `grant = {task, apps[], data_scope, allowed_actions[], denied_actions[], expiry}`
   ——例：「允許修改草稿、不允許寄出、五分鐘後失效」。
2. **可逆性是分界線**〔定案〕：不可逆動作在任何模式下都需要顯式即時核准；
   **永不繼承清單**〔定案〕：送出、發布、付款、刪除、開 terminal——
   這五類權限不隨任務授權繼承，每次都要單獨核准。
   bounded takeover 只含可逆操作（「程式留在 staging」是產品格言）。
   命名註記〔定案〕：辯論封印了「Autopilot」這個詞——對使用者一律叫
   **「接手模式」（takeover）**，因為它承諾的是「有邊界地接手」，不是「自動駕駛」。
3. **驗證迴圈**〔定案：解 35% 複利魔咒的不是步準確率〕：每步後截圖驗證結果，
   失敗即停；任務有 scope 描述、停止條件、步數上限、完整 action log。
   **硬中斷是 OS 級的**〔定案〕：全域快捷鍵 + tray 按鈕直接 kill hands process，
   實作在模型碰不到的層（不是請模型停，是把手拔掉）；不綁 Esc（太容易誤觸與被吃）。
4. **來源防線**：畫面文字 = 資料不是指令（§0.4）；hands 的 system prompt
   不接受任何 L0 內容作為指令；外部內容要求動作 → 一律升級為人工核准。
5. **預檢**：autopilot 啟動前做 fresh-evidence check（重新看畫面），
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
它：prompt 從 stdin 進、JSON 從 stdout 出。`check-no-network.sh` 繼續禁
`reqwest`／`ureq`／本機推論引擎，沒有例外。

- **這一版落地的**：`[brain] command` + `args`。沒設定就一次都不呼叫。
- **還沒做的**：(a) 訂閱登入的 OAuth 輔助（MAT `signin.ts`）——使用者自己
  在那支 CLI 裡登入即可；(b) 本地 Ollama 偵測。兩者都還是 spawn CLI，不是
  把 HTTP client 拉進相依樹。
- **角色→模型對映**（可配置，附預設）：interpreter=cheap tier；reviewer=mid tier
  （夜間 batch 半價）；chat=使用者選；hands=使用者訂閱的 coding agent。
  實際選哪一個模型，是那支 CLI 自己的事。
- **降級鏈**：沒簽同意書 2 / 沒設定 CLI / 每日預算用完 → 純檢索模式
  （1.0 功能永遠活著）。三種原因印三種話。
- 併發槽數預設 4、上限 8（§5.3）。

## §11. 隱私與安全（產品的第一賣點，工程上與功能同權重）

### 11.1 三張同意書〔定案〕（onboarding 三個獨立開關，README 第一段公開承諾）

1. **本機記錄**：我同意在我的硬碟上記錄我的螢幕（可全功能運作，永不聯網）。
2. **上雲解讀**：我同意把**去識別化後的文字**（OCR 抽出來的字，永不含
   pixel）交給我在設定裡指定的本機 CLI，由那支程式去做解讀（預設關 →
   沒簽就一次都不 spawn）。
3. **畫面暫存**：我同意保留變化幀截圖 N 天（可選 0 天 = 只留 OCR 文字）。

### 11.2 Capture 時排除（不是事後刪）〔定案〕

- App/URL blocklist（預設含密碼管理器、網銀常見 domain 樣板）；
- 隱私視窗（incognito）偵測即跳過；密碼欄位（UIA SecureText / AXSecureTextField）
  永不 OCR；螢幕分享/會議 app 前景時自動 pause（旁人畫面防線）；
- 剪貼簿秘密偵測（高熵字串/`sk-` 類 pattern）→ 不落地，只記「複製了一個秘密」事件。

### 11.3 去識別化管線（上雲前）

`money/phone/email/id_like` 以 typed placeholder 替換（`<AMT_1>`）——
雲端模型判讀意圖不需要真數字，真數字只活在本機 L1〔定案：本機搜就好，
那串數字從來不需要離開你的硬碟〕。人名遮蔽為選配（旁人保護，預設開）。

### 11.4 保留與加密

分層 TTL（預設）：變化幀 30 天 → 縮圖 90 天 → OCR/L1 365 天 → L3 直到結案。
SQLCipher 或 OS-keyring 包裹的 at-rest 加密；一鍵 pause；一鍵 panic wipe
（含 export 先行選項）；時間軸瀏覽器可框選區間刪除（cascade）。

### 11.5 旁人問題（誠實聲明）〔定案為無技術完解〕

她錄的不只是你——朋友的訊息、客戶的文件會進你的本機資料庫，他們沒同意過。
產品立場：(a) 預設人名遮蔽（§11.3）；(b) 會議/分享自動 pause（§11.2）；
(c) 通訊 app 可一鍵入 blocklist；(d) 文件（PRIVACY.md）誠實陳述此邊界，
不假裝解決了。法域註記：部分地區對「記錄他人通訊」有法律風險，文件明示。

### 11.6 供應商端留存（第三張同意書的誠實註腳）

去識別化文字上雲後仍受各模型商 abuse-monitoring 留存政策約束。對策：
(a) 文件列出各 provider 的留存/zero-retention 選項，預設推薦有 ZDR 的通道；
(b) 上雲內容全部可在「外送紀錄」面板檢視（送了什麼、給誰、何時）。

### 11.7 可驗證性（Recall 的教訓：「宣稱本機」不夠）

加密可驗證（文件化金鑰鏈）、開機自動鎖（session 未解鎖不解密）、
**master kill switch 一鍵全停**（Recall 至今沒有）、
非本人帳號/離線竊碟不可讀 DB（TotalRecall 級攻擊的防線）、安全審計歡迎聲明。
誠實邊界：**同使用者 session 內的 malware 不在防護範圍**——userland OSS
做不到 Recall 的 VBS enclave + TPM 綁定，README 明講，不假裝。

### 11.8 資料主權（Rewind 的教訓：closed product 的退場 = 記憶滅絕）

**開放資料格式**：SQLite schema 公開文件化、`sister export` 全量匯出、
沒有任何功能依賴我們的伺服器。就算本專案死了，你的記憶還是你的。

### 11.9 遙測

零遙測預設。opt-in 匿名計數器（Cloudflare D1 模式，TokenMonster 現成）。

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

- 隨開機自啟、崩潰自復活（core 與 shell 獨立看門）；
- 升級不丟資料（SQLite migration 版本化）；
- i18n：zh-TW / en day one（MAT i18n 骨架）；
- 可觀測：開發者模式面板（L2 卡片流、回查 log、開口候選與分數）——
  預設關閉〔定案〕；
- 所有內部 IPC strict Zod/serde schema（Ted 兩 repo 的一貫簽名）。

## §15. 技術選型

（版本皆 2026-08-17 於 crates.io/官方驗證，詳見 `research/tech-stack.md`）

| 元件 | 選型 | 備註 |
|---|---|---|
| core daemon | **Rust** 單 binary（Tauri sidecar 形式打包，獨立於 UI 存活） | macOS TCC 權限歸屬 responsible process——sidecar 必須簽進 .app bundle，權限只要一次 |
| 截圖 | Windows Phase 0：Win32 GDI（BitBlt／GetDIBits）；後續候選：Windows WGC、macOS ScreenCaptureKit、Linux PipeWire | 目前只有 Windows GDI 已落地 |
| 去重／OCR gate | 自製 64-bit dHash（預設 Hamming ≤5 視為近似同幀）+ 64×64 RGB tile FNV-1a；Windows Phase 0 的近似重複幀只讓能一對一拼回舊閱讀順序的 crop 升格，其餘維持重複；dHash 新幀的局部證據不足才退回全幅 | 沒有使用 DXGI dirty rects |
| OCR | macOS：Apple Vision（zh-Hant 一等公民，accurate ~0.3–1.4s/全幅，搭配 OCR gate 只跑 crop）；Windows：**Phase 0 實作改用 `Windows.Media.Ocr`，見 §14.1**；PP-OCRv5 via `oar-ocr` 保留為精準度升級路線；TextRecognizer 鎖 Copilot+ NPU；OneOCR 授權灰色，只做使用者自機 opt-in；Tesseract 僅最後底線 | 搜尋前全半形正規化 + OpenCC 繁簡歸一 |
| DB | SQLite 3.53（`rusqlite` 0.40，WAL）+ **FTS5 trigram + unicode61 + bigram 三索引**（external-content table；trigram 補 CJK 子字串、unicode61 補英文整詞、`text_fts_bi` 補**兩個字的中文**——unicode61 把整串 CJK 當一個 token，`MATCH "客服"` 是 0 筆，schema 3 之前只剩夾在 30 天內的 LIKE 掃描。bigram 是粗篩，命中要拿真字串再驗一次；只剩單字查詢仍走掃描） | 之後要拼音再上 `simple` tokenizer |
| 向量（選配） | `sqlite-vec` 0.1.9（2026 復活版；256-d int8 MRL，brute-force 在我們規模內互動級） | pre-1.0 格式風險 → 存 model-id+dim，設計成可背景 re-embed |
| 本地 embedding | `fastembed` 6.0：EmbeddingGemma-300m 首選（量化 <200MB RAM）/ Qwen3-Embedding-0.6B 品質檔 | 批次排到 idle/插電 |
| 加密 | SQLCipher 4.13（開銷 5–15%）+ `keyring` 4.1（OS keychain 金鑰） | 誠實威脅模型：同使用者 malware 不在防護範圍（userland OSS 做不到 Recall 的 VBS enclave），README 明講 |
| UI shell | **Tauri 2.11** + React + Tailwind；plugins：tray、global-shortcut、autostart、notification、clipboard-manager、single-instance、positioner、shell、updater | Screenpipe/Cap 同路線的大型先例存在 |
| Pet overlay | always-on-top 透明無框窗 + `set_ignore_cursor_events` 動態 toggle（輪詢游標；Tauri 無 per-region hit-testing）| 已知坑：macOS production 透明窗 bug 群、全螢幕 space 需動 collectionBehavior、Wayland overlay 品質差 |
| macOS 權限 | `tauri-plugin-macos-permissions` 2.3（Screen Recording 無 entitlement，純 TCC + hardened runtime + notarization；MAS 不可行，站外發行） | 開發期 `tccutil reset ScreenCapture` 測 onboarding |
| hands sidecar | **Node ≥20 TS**（Phase 6+） | 直接重用 MAT adapters/signin |
| Schema | serde + Zod（跨層 contract 生成） | |
| hands 元件（Phase 6+） | Agent S3（Apache-2.0）/ UFO²（MIT）/ OmniParser v3 weights（MIT，避開舊 AGPL detector） | 「手」已商品化：用組的，不自己寫 grounding |
| 參考不引用 | Screenpipe（2026-06 起自訂商業授權，僅參考架構；MIT fork point 在舊版）；Everywhere（BUSL，僅 MCP/API interop） | license 判定見 research/landscape.md |

平台順序〔決定〕：**Windows 首發**（最友善平台：WGC 24H2 dirty rects +
MinUpdateInterval 免費變化偵測、無擷取指示、Ted 日用、Recall 陰影下最大受眾）；
macOS 於 Phase 5 公開宣傳前補齊（紫點與 TCC 憲法見 §2.4）；
Linux **X11 首發、Wayland 明示降級**〔定案〕——Wayland 背景連續擷取是架構性死路
（restore_token 單次、鎖屏拒發、GNOME 缺 toplevel/idle-notify），不承諾、文件明講。
capture 從 day 1 走 trait 抽象，三平台介面同形。

### §14.1 偏離紀錄：Windows OCR 引擎（Phase 0）

上表原本〔定案〕Windows 用 PP-OCRv5 via `oar-ocr`。**Phase 0 的實作沒有照做**，
改用系統內建的 `Windows.Media.Ocr`。這一節記錄理由，以及在什麼條件下該改回去。

實際去看相依樹之後才發現的、選型當時不知道的事：

1. **`oar-ocr` 的 `auto-download` 會把 `ureq` 連進執行檔。** 那是一個真的 HTTP
   client，存在於出貨的二進位檔裡。PRIVACY.md 的第一句是「程式裡沒有任何對外
   連線的程式碼路徑」——那句話要嘛是真的，要嘛不是。
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
- 必備文件（day one）：README（三張同意書宣言 + benchmark 表）、PRIVACY.md
  （含旁人邊界誠實聲明）、THREAT_MODEL.md、DATA_INVENTORY.md
  （TokenMonster 的寫法沿用）；
- clone → 跑起來 < 10 分鐘是硬指標〔定案：clone 十分鐘要看到字母人動〕。

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
| 8 | 平台 | Windows → macOS → Linux | Ted 日用 + 受眾 + API 成熟度 |
| 9 | 8/3 兩個數字 | worker pool 參數（預設 4/上限 8）與 Reviewer 雙 pass | 排程真相 + 並聯辯證，數字本身無意義 |
| 10 | 回饋分類 | 兩鍵（結案/其他），三分類內部推斷 | 使用者不會標三層意圖 |
