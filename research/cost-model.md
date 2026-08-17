# LLM 成本模型 — AI-Sister（2026-08-17 定價）

> 前提：8 小時工作日 × 每月 22 個工作天。所有價格為 2026-08-17 當日各家官方頁面查證之 USD 定價（單位：每百萬 tokens，MTok）。
> 情境定義來自 roundtable 收斂結論：raw capture 全部本地免費，LLM 只在「值得理解的時刻」被喚醒。

---

## 一、2026-08 現行官方定價

### 1.1 文字 token 定價（USD / MTok）

| 模型 | Input | Output | Cache read | Batch (in/out) | 備註 |
|---|---|---|---|---|---|
| **Claude Haiku 4.5** | $1.00 | $5.00 | $0.10 | $0.50 / $2.50 | 200K context |
| **Claude Sonnet 5** | $2.00 | $10.00 | $0.20 | $1.00 / $5.00 | 原訂 2026-09-01 漲回 $3/$15 的計畫**已取消**，$2/$10 成為正式價 |
| Claude Sonnet 4.6 | $3.00 | $15.00 | $0.30 | $1.50 / $7.50 | 舊 tokenizer（見 1.4 註） |
| Claude Opus 4.8 / Opus 5 | $5.00 | $25.00 | $0.50 | $2.50 / $12.50 | heavy-brain 上限參考 |
| **Gemini 3.7 Flash**（2026-08-13 發布） | $0.75 | $3.75 | $0.075 | $0.375 / $1.875 | 促銷價至 2026-12-31，2027-01-01 起 $1.50 / $7.50 |
| **Gemini 3.5 Flash-Lite** | $0.30 | $2.50 | $0.03 | $0.15 / $1.25 | 正式價 |
| Gemini 3.1 Flash-Lite | $0.25 | $1.50 | $0.025 | $0.125 / $0.75 | 2.5 FL 退役後的最低 Google 檔位 |
| Gemini 2.5 Flash-Lite | $0.10 | $0.40 | $0.01 | $0.05 / $0.20 | **2026-10-16 退役**，不宜當設計基準 |
| **GPT-5.4-mini** | $0.75 | $4.50 | $0.075 | $0.375 / $2.25 | |
| **GPT-5.4-nano** | $0.20 | $1.25 | $0.02 | $0.10 / $0.625 | OpenAI 現役最便宜 |
| GPT-5.4 | $2.50 | $15.00 | $0.25 | 50% off | mid tier 對照 |
| GPT-5.5 | $5.00 | $30.00 | $0.50 | 50% off | flagship |
| **Qwen3.7 Flash**（OpenRouter） | $0.03 | $0.13 | — | — | 2026-07 上架，1M context、vision-capable，全場最低價 |
| Qwen3.5 Flash（OpenRouter） | $0.065 | $0.26 | — | — | |

註：舊世代 GPT-5-nano（$0.05/$0.40）已從 OpenAI 官方價目下架，以 GPT-5.4-nano 為現役 nano 檔。

### 1.2 Prompt caching 規則（影響 Scenario B/C）

- **Anthropic**：顯式 `cache_control`。寫入 5 分鐘 TTL = 1.25× input、1 小時 TTL = 2× input；讀取 = 0.1× input。可與 Batch 折扣疊加。
  **關鍵陷阱：最短可快取 prefix 依模型而異 — Haiku 4.5 = 4,096 tokens；Sonnet 4.6 = 2,048；Sonnet 4.5 = 1,024。** 本案假設的「~2k cached system prompt + schema」在 Haiku 4.5 上**低於門檻、完全不會生效**；要嘛把穩定 prefix 墊到 ≥4,096（塞 few-shot examples），要嘛放棄 caching。
- **Google**：implicit caching 自動生效，cache hit = 0.1× input（如 3.5 Flash-Lite $0.03）。另有顯式 cache 儲存費（$0.50/MTok/hr 促銷價）。
- **OpenAI**：自動 caching（prefix ≥1,024 tokens），cached input = 0.1× input（5.4-nano $0.02）。

### 1.3 影像 token（螢幕截圖送 LLM 解讀時）

| 供應商 | 計法 | 1080p（1920×1080）| 1440p（2560×1440）| 每張成本（cheap 檔） |
|---|---|---|---|---|
| Anthropic | (w×h)/750，長邊 >1568px 先縮圖 | ~1,844 tok（縮至 1568×882） | 同左（縮圖後相同） | Haiku：**$0.0018**；Sonnet 5：$0.0037 |
| Google Gemini | 兩邊 ≤384px 固定 258 tok；否則切 768×768 tiles、每 tile 258 tok | 6 tiles ≈ **1,548 tok** | 6 tiles ≈ 1,548 tok（crop unit 隨圖放大） | 3.5 FL：**$0.00046**；3.7 Flash：$0.0012 |
| OpenAI（mini/nano） | 32×32 patches、上限 1,536 patches，再乘模型係數（4.1 世代：mini ×1.62、nano ×2.46；5.4 世代未明列，暫沿用估算） | ≈1,536 patches → nano ~3,779 tok | 同左（超限自動縮圖） | 5.4-nano：**$0.00076**；5.4-mini：$0.0019 |
| Qwen3.7 Flash | 視覺 tokens（估 ~1,500–2,000） | ~1,600 tok（估） | ~1,600 tok（估） | **$0.00005** |

結論：**一張 1080p 截圖 ≈ 1,500–3,800 input tokens ≈ 0.005–0.4 美分**。單張便宜，貴的是「每 10 秒送一張」的頻率。

### 1.4 兩個時效註記

- **Sonnet 5 tokenizer**：Claude 4.7 之後的模型（含 Sonnet 5）改用新 tokenizer，同樣文字約多產出 ~30% tokens。同文比較時 Sonnet 5 有效費率 ≈ $2.6/MTok（仍低於 Sonnet 4.6 的 $3）。
- **Gemini 3.7 Flash 是促銷價**：2027-01-01 翻倍。長期估算應用 $1.50/$7.50 做壓力測試。

---

## 二、Scenario A —「暴力輪詢」（已否決設計，對照用）

每 10 秒一次 LLM call：**2,880 calls/天 = 63,360 calls/月**。

### A-1 純文字版（每 call 2k in / 200 out）→ 月用量 126.72M in / 12.67M out

| 模型 | 月成本 |
|---|---|
| Qwen3.7 Flash | **$5.45** |
| Gemini 2.5 Flash-Lite（將退役） | $17.74 |
| GPT-5.4-nano | **$41.18** |
| Gemini 3.5 Flash-Lite | **$69.70** |
| Gemini 3.7 Flash（促銷價） | $142.56 |
| GPT-5.4-mini | $152.06 |
| Claude Haiku 4.5 | **$190.08** |
| Claude Sonnet 5 | **$380.16** |

### A-2 截圖版（每 call 1 張 1080p 圖 + 1k 文字 in / 200 out）

| 模型 | 每 call input tokens | 月成本 |
|---|---|---|
| Qwen3.7 Flash | ~2,600 | **$6.59** |
| GPT-5.4-nano | ~4,779 | $76.40 |
| Gemini 3.5 Flash-Lite | ~2,548 | $80.11 |
| Gemini 3.7 Flash | ~2,548 | $168.60 |
| Claude Haiku 4.5 | ~2,850 | **$243.94** |
| Claude Sonnet 5 | ~2,850 | $487.87 |

**判讀**：暴力輪詢在「可用品質」檔位（Haiku / Flash-Lite / mini）就是每月 $70–$250；上到 Sonnet 級直接 $380–$490。除了成本，這個設計 95% 的 call 都在解讀「畫面沒變」的雜訊 — 這正是它被否決的原因。唯一便宜的路（Qwen3.7 Flash $5–7）代價是品質與供應商穩定性都綁在單一低價來源上。

---

## 三、Scenario B — 事件驅動解讀（收斂設計）

raw capture（截圖、OCR、app events）全部本地、零 API 成本。LLM 只在事件觸發時喚醒：

| 呼叫類型 | 次數/天 | Input | Output | 內容 |
|---|---|---|---|---|
| Interpretation（app-switch cluster、dwell-timeout batch） | 30–80（中位 50） | 3–5k（中位 4k）文字（OCR 摘錄 + 近期事件摘要） | ~300 | 即時輕量解讀 |
| Batch review（活躍時每 15–30 分鐘） | ~20 | ~8k | ~500 | 彙整回顧 |
| On-demand user queries | ~20 | ~4k | ~400 | 使用者主動問 |

中位日用量：**440k in / 33k out** → 月用量 **9.68M in / 0.726M out**（低–高範圍：7.26–14.08M in）。

### B-1 無 caching 月成本

| 模型 | 中位 | 範圍（30–80 interp calls） |
|---|---|---|
| Qwen3.7 Flash（OpenRouter） | **$0.38** | $0.29–$0.55 |
| Gemini 2.5 Flash-Lite（將退役） | $1.26 | — |
| **GPT-5.4-nano** | **$2.84** | $2.19–$3.97 |
| **Gemini 3.5 Flash-Lite** | **$4.72** | $3.66–$6.53 |
| Gemini 3.7 Flash | $9.98 | $7.67–$14.03 |
| GPT-5.4-mini | $10.53 | — |
| **Claude Haiku 4.5** | **$13.31** | $10.23–$18.70 |
| Claude Sonnet 5 | $26.62 | $20.46–$37.40 |

### B-2 加上 prompt caching（~2k system prompt + schema 共用 prefix，90 calls/天）

可快取量：2k × 90 × 22 = **3.96M tok/月**，從 1.0× 降到 ~0.1×：

| 模型 | 無 caching | 有 caching | 省 |
|---|---|---|---|
| GPT-5.4-nano（自動，門檻 1,024 ✓） | $2.84 | **~$2.13** | ~25% |
| Gemini 3.5 Flash-Lite（implicit ✓） | $4.72 | **~$3.65** | ~23% |
| Gemini 3.7 Flash | $9.98 | ~$7.31 | ~27% |
| Claude Haiku 4.5（**2k < 4,096 門檻 ✗**） | $13.31 | **$13.31（不生效）** | 0% |
| Haiku 4.5 墊到 4k prefix + 1h TTL | $13.31 | ~$11.8 | ~11%（含寫入成本後） |
| Claude Sonnet 5（門檻 2,048，剛好壓線） | $26.62 | ~$21.1 | ~21% |

**設計發現**：任務原設定「~2k cached」在 Haiku 4.5 上是無效假設 — Anthropic 各模型最短可快取 prefix 不同（Haiku 4.5 要 4,096）。若選 Haiku，穩定 prefix 要刻意墊到 ≥4k（放 few-shot 範例是順便提升品質的墊法）；若選 Gemini/OpenAI 則自動生效無此問題。

### B-3 再加 Batch API（review calls 非即時，可走 50% off）

20 review calls/天（3.52M in / 0.22M out /月）改走 Batch：Haiku 再省 ~$2.3、Flash-Lite 再省 ~$0.8、nano 再省 ~$0.5。三招疊加後：**Haiku ≈ $9–11、Flash-Lite ≈ $3.1、nano ≈ $1.9**。

### B-4 Sanity check：「一天值得理解的時刻 < 50 個」

Roundtable 結論是一天真正值得理解的 moment 不到 50 個。Scenario B 的 interpretation 中位 50 calls/天 = **每 moment 恰好 1 call**；上限 80/天等於容忍 1.6× 的過度觸發（app 快速切換造成的重複喚醒）。即使觸發器爛到 2× 過度喚醒（100 calls/天），Haiku 也只從 $13 升到 ~$21、Flash-Lite 到 ~$7 — 設計對觸發精度不敏感，預算誠實。

另註：在 B 的量級下，interpretation 改送 1 張截圖而非 OCR 文字，每月只多 $0.5–2（50 張/天 × ~1,500–1,900 tok）。**「送 OCR 文字不送圖」在 B 不是省錢手段，是隱私與訊噪手段；只有在 A 的頻率下圖片才是成本放大器。**

---

## 四、Scenario C — Heavy-brain 層（orchestrator / deep passes）

每小時 3 次 Sonnet 級深度整合，15k in / 1k out：**24 calls/天** → 月用量 **7.92M in / 0.528M out**。

| 模型 | 月成本 | 備註 |
|---|---|---|
| **Claude Sonnet 5（$2/$10）** | **$21.12** | 首選；新 tokenizer 同文 +30% 屬同文比較註記，本表以 spec 的 15k tok 計 |
| Gemini 3.1 Pro Preview（$2/$12） | $22.18 | |
| GPT-5.4（$2.50/$15） | $27.72 | |
| Claude Sonnet 4.6（$3/$15） | $31.68 | 無理由選它 — Sonnet 5 較新且較便宜 |
| Claude Opus 4.8（$5/$25） | $52.80 | 除非 heavy-brain 需要 Opus 級推理 |

槓桿：
- **Caching**（15k 中約 8k 為 rolling context，1h TTL）：Sonnet 5 → ~$18.9。注意 3 calls/hr = 20 分鐘間隔 > 5 分鐘 TTL，**5m cache 在此節奏下必然全 miss，必須用 1h TTL**。
- **Cadence 是最大槓桿**：降到每小時 1 次 → **$7.04/月**。品質差異（20 分鐘 vs 60 分鐘的整合延遲）對 ambient 陪伴型產品未必可感。
- **Batch 部分遞延**：把一半 deep passes 改成夜間 Batch 彙整（50% off）→ ~$15.8/月，代價是即時性。

---

## 五、Scenario D — 本地模型混合（RTX 5090 32GB / Apple M 系列）

### 5.1 2026 年本地可跑什麼（interpretation 層）

| 模型 | VRAM（Q4） | 品質定位 | 適配 |
|---|---|---|---|
| **Qwen3-VL-8B**（Apache 2.0） | ~6 GB | 小模型第一：MMMU 69.6、DocVQA 96.1 — **約等於 cloud nano / Flash-Lite 檔**，CJK OCR 強（對繁中使用情境重要） | 5090 輕鬆跑，M4 Pro/Max via MLX 約 30–70 tok/s |
| Qwen3-VL-32B / Qwen 3.6 世代 open-weight | ~19–20 GB | 逼近 **Haiku 4.5 / Flash 檔** | 5090 32GB 可跑 Q4，約 35–50 tok/s |
| Gemma 4 multimodal（27B 級） | ~16 GB | 推理佳、CJK OCR 較弱 | 5090 Q4 可跑 |
| Phi-4-multimodal（5.6B） | ~4 GB | 輕量分類/路由 | 任何機器 |

實測共識：本地對雲端品質差距約 **10–20%**，速度慢 2–3×，但 interpretation 這種結構化抽取任務（「這個畫面在做什麼」）正是小 VLM 的甜蜜點；一次 4k-token call 數秒內完成，50–90 calls/天毫無壓力。

### 5.2 「零邊際成本」的實際帳

- 電費：interpretation 層每天累計 GPU 活躍 ~20–40 分鐘 × ~400W ≈ 4–7 kWh/月 ≈ **$1–2/月**。
- 隱性成本：模型常駐佔 6–20 GB VRAM，**與 5090 的 image-gen 用途衝突**（Ted 的既定原則是 5090 留給 image-gen、雜活走雲端）；8B 檔按需載入（數秒）可緩解。
- 維運成本：本地 serving 的更新、quantization 選型、prompt 相容性都是自己的事。

### 5.3 混合架構的真實省額

| 架構 | 月成本 |
|---|---|
| 全雲端：B（Flash-Lite）+ C（Sonnet 5） | ~$25.8 |
| 混合：B 本地（Qwen3-VL-8B）+ C 雲端（Sonnet 5） | **~$22–23**（$21.12 + 電費） |

**判讀**：在事件驅動的量級下，本地化 interpretation 每月只省 $3–13 — **省錢不是理由**。真正的理由是（1）**隱私**：raw 畫面內容永不出機器，只有蒸餾後的摘要上雲；（2）離線可用。反過來說，若當初選了 A 的頻率，本地化能省 $40–240/月 — 本地推理只在高頻設計下才有經濟意義，而高頻設計本身已被否決。C 層（跨時段整合推理）本地 32B 模型品質仍不足以取代 Sonnet 級，不建議下放。

---

## 六、總表與結論

### 6.1 A/B/C/D 月成本總覽（8h × 22 天）

| 情境 | Cheap 檔 | Mid 檔 |
|---|---|---|
| **A 暴力輪詢**（否決） | $41–190（nano/Flash-Lite/Haiku，文字）；截圖版 $76–244 | $380–488（Sonnet 5） |
| **B 事件驅動** | **$2–13**（nano $2.84 / Flash-Lite $4.72 / Haiku $13.31）；caching+batch 後 $2–11 | $27–37（Sonnet 5 全包 B） |
| **C heavy-brain**（3 passes/hr） | —（此層不建議用 cheap 檔） | **$21**（Sonnet 5）；caching 後 ~$19；降頻至 1/hr 後 $7 |
| **D 本地混合** | interpretation $0 API + $1–2 電費 | C 仍走雲端 $21 |

### 6.2 收斂架構（B + C）的實際月費

| 組合 | 月成本 |
|---|---|
| Haiku 4.5 解讀 + Sonnet 5 深度層（無最佳化） | **~$34** |
| Flash-Lite 解讀 + Sonnet 5 深度層 | **~$26** |
| nano 解讀 + Sonnet 5 深度層 | **~$24** |
| 上述 + caching + review 走 Batch | **~$19–28** |
| 本地 Qwen3-VL 解讀 + Sonnet 5 深度層 | **~$22**（隱私紅利） |
| 激進下限：Qwen3.7 Flash 解讀 + 深度層降頻 1/hr | **~$7–8** |

### 6.3 Verdict

**收斂架構的真實答案是「$30 級」— 不是 $3，也不是 $300。**

- **$300 級**只會發生在被否決的暴力輪詢 + mid-tier 模型（A@Sonnet 5 = $380）。事件驅動架構本身就是 14× 的成本削減，比任何 caching/batch 技巧都大一個數量級。
- **$3 級**理論上可達（Qwen3.7 Flash + 深度層降頻），但那是把品質、供應商風險、整合深度全押在最低價檔位上 — 不是這個產品該有的預設。
- **成本由誰主宰**（按影響力排序）：
  1. **呼叫頻率架構**（A vs B = 14×）— 最大的決定早就做完了；
  2. **heavy-brain 層的 cadence 與模型**（C 佔收斂架構總費用的 60–80%，3/hr → 1/hr 直接砍 2/3）— 這是下一個該調的旋鈕，建議做成 adaptive cadence（活躍時 3/hr、閒置時 0）；
  3. **interpretation 模型檔位**（nano ↔ Haiku = 5×，↔ Sonnet = 10×）；
  4. **caching / Batch** 只是 10–25% 的修邊，且 Haiku 有 4,096-token 門檻陷阱；
  5. **圖片 vs OCR 文字**在 B 的量級下對成本幾乎無感（月差 $0.5–2），該用哪個由隱私與訊噪決定，不是錢。

---

## 引用來源（2026-08-17 查證）

- Anthropic 官方定價（含 caching 倍率、Batch 50%、Sonnet 5 $2/$10 轉正式價聲明、tokenizer 註記）：<https://platform.claude.com/docs/en/about-claude/pricing>
- Anthropic prompt caching（最短可快取 prefix 門檻）：<https://platform.claude.com/docs/en/build-with-claude/prompt-caching>
- Anthropic vision token 計算：<https://platform.claude.com/docs/en/build-with-claude/vision>
- Anthropic Batch API：<https://platform.claude.com/docs/en/build-with-claude/batch-processing>
- Google Gemini API 官方定價（3.7/3.6 Flash 促銷價與 2027 調價、Flash-Lite 各檔、Batch 50%）：<https://ai.google.dev/gemini-api/docs/pricing>
- Google Gemini 影像 token（258/tile、768×768 tiling）：<https://ai.google.dev/gemini-api/docs/image-understanding>
- OpenAI 官方定價（GPT-5.5 / 5.4 家族含 mini/nano、cached input、Batch）：<https://developers.openai.com/api/docs/pricing>
- OpenAI 影像 token 計算（32px patches、1,536 上限、mini/nano 係數）：<https://developers.openai.com/api/docs/guides/images-vision>
- OpenRouter Qwen 系列頁（Qwen3.7 Flash $0.03/$0.13、Qwen3.5 Flash $0.065/$0.26）：<https://openrouter.ai/qwen>、<https://openrouter.ai/qwen/qwen3.5-flash-02-23>
- Gemini 2.5 Flash-Lite 2026-10-16 退役、Gemini 3.1 Flash-Lite 接棒（第三方彙整佐證）：<https://devtk.ai/en/blog/gemini-api-pricing-guide-2026/>
- 本地 VLM 2026 現況（Qwen3-VL-8B 領先、5090 跑 Q4 32B 級、品質差距 10–20%）：<https://tinyweights.dev/posts/best-local-vision-language-models-2026/>、<https://www.hardware-corner.net/gpu-llm-benchmarks/rtx-5090/>
