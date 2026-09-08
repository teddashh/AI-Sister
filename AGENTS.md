# AGENTS.md — 給接手的 agent

一句話：**AI-Sister 是一個在 Windows 上安靜看著螢幕、事後答得出「我昨天在幹嘛」、
而且每一句話都點得開證據的本機記錄器。**「本機」指的是錄下的截圖、OCR 與記憶留在
這台機器**；腦（L2/L3）接的是使用者自己已經裝好的 CLI agent，不是內建的 HTTP
client。desktop 只有兩條具名、窄化的內建 outbound：Persona 固定素材包的使用者發起
GET，以及 alpha.109 預設關閉、另行同意與按下才送當前答案正文的 Azure TTS POST。
它們不能擴散到 recorder／core／capture／brain／hands 或 WebView。

先讀 `docs/PHASES.md`（路線圖，退場條件就是驗收條件）、`docs/SPEC.md`、`docs/PRODUCT.md`。
**現在該做什麼看 .handoff/PLAN.md**（刻意不進 git，只在工作目錄裡）。

---

## 一、方針（負責人 Ted，2026-08-22 定案）

> 「我認為我們不要卡在一直驗證，我們照 milestone 推進，如果真的有問題再回頭修。」

2026-08-23 又把順序講得更明白：**先把要的功能做成功、把體驗做好，再研究怎麼
優化容量。** 容量仍照實量、錯帳仍要修，但不再擋功能 milestone；不要為了湊
`<300MB/天` 自己停工、改圖額度或重構 schema。等功能與體驗成形後再回來優化。

**預設＝照 `docs/PHASES.md` 的退場條件往前推，做完就切 tag 發 release。**
不要自己發明驗收標準、不要為了一個小地方開對抗式稽核、不要在已經量完的東西上再繞一圈。
（前一輪的教訓：對 `sister bench` 做了三回合稽核、20 個 agent、約 230 萬 token，
產品程式碼落地零行。那是 scope drift，不是嚴謹。）

**唯一的例外——同意書／隱私那條路，維持對抗式驗證。**
因為其他缺陷的失敗模式是「出問題再回頭修」，這條的失敗模式是
**「她開始寫使用者剛剛關掉的截圖」**——那不是 bug report，那是 README 第一句話破掉。
範圍很小（`ops.rs` 的 consent gate、`crates/sister-core/src/consent.rs`、
`scripts/check-consent-*.{py,mjs}`、`scripts/check-no-network.sh`、
`scripts/check-no-keylogging.py`），守它很便宜。

**節奏**：有執行檔他就下載測，沒有就繼續推。不要停下來等回覆；做完一段就切 tag。
一次多做一點再叫他測。

### Persona asset 網路邊界（alpha.102 起）

- alpha.108 起，四姊妹與 13 位閨密的 17 張 current WebP 隨 desktop 一起提供；來源、
  bytes、SHA-256 與 Apache-2.0 圖像授權排除寫在 `apps/desktop/ui/personas/`。預設 ChatGPT，沒有
  Neutral，也沒有 S/T/C/G/X glyph fallback。舊 `neutral` 設定只遷移成 ChatGPT 並關聲。

- `crates/sister-assets` 在 root workspace，預設 feature 集合**沒有** `download`；只有
  desktop 明確啟用。不要把 HTTP client 直接加進 `sister-desktop`，更不能加進
  recorder／core／capture／brain／hands。
- 唯一 request 是使用者看見 `cdn.ted-h.com`、73,261,088 bytes 與資料邊界後明確按下，
  對內嵌 allowlist 的 exact hash path 做至多一次 HTTPS `GET`。不 redirect／proxy／
  retry，不送 cookie／credentials／authorization／referrer／query／body，不先 `HEAD`
  或逐物件抓。切換 persona 不改 method／URL／headers／body。
- embedded authority 是舊 fixed-voice pack 的 descriptor + exact origin/path allowlist +
  四姊妹 selected public rights projection + 完整 manifest 的 canonical hash；**不是**約
  2.1 MB 的完整 11 人 manifest。73,261,088-byte pack 的 SHA-256 是
  `7d98e0d18c470f82818e8ada67208847c3cf4ff5c10cb5f99f9215191e981f30`，集體 pin
  946 entries；真正使用的 selected entries 再逐檔驗 size／hash／rights binding。
- cache 固定在 `Config::default_data_dir()/persona-assets-v1`，`--data-dir` 不搬；memory
  export／forget／prune 不碰，Persona 撤回才清 exact release。失敗或損毀只停用舊
  fixed voice，保留 bundled 角色圖，不得自動連線修復。
- HTTP 在 native Rust；所有 WebView CSP 繼續只准 IPC，**不要把 CDN 加進
  `connect-src`／`img-src`／`media-src`**。一般 CI 也不打真 CDN。
- 本機語音只接受 WebView 明確標成 `localService` 的中文 voice；找不到就靜音，不得
  自動換 remote voice。角色台詞與答案朗讀都要由 trusted click 開始，不 autoplay。

### Azure TTS 網路邊界（alpha.109 起）

- 本機 `localService` 語音仍是預設；Azure 預設關閉，而且兩條路**都不互相自動
  fallback**。只有設定明確啟用、region 與 voice 都是 typed allowlist、Windows
  Credential Manager 有 key、第四張 `azure-tts` 同意有效，再由使用者親手按 Azure
  朗讀，才可建立請求。舊三張同意不授權 Azure；同版本舊檔遷移時第四張一律未簽。
- `crates/sister-tts` 的預設 feature 沒有 Azure HTTP；只有 desktop 明確啟用。region
  嚴格只有 `eastasia`、`southeastasia`、`japaneast`，各自只准 native Rust 對
  `https://<region>.tts.speech.microsoft.com/cognitiveservices/v1` 做一個 HTTPS `POST`。
  不接受 renderer 傳 URL，不 redirect／proxy／retry，也不能把 TTS client 拿給 OCR、
  brain、hands 或 Persona 使用。
- POST 只含**當前答案正文原文**；它可能含姓名、電話與金額而且不先遮罩。不得夾帶
  截圖、來源連結／chip、memory id、整份資料庫或其他文字。第四張條文明講這個邊界；
  沒簽時一次都不呼叫 Azure，本機朗讀不受影響。
- subscription key 長期只存目前 Windows 使用者的 Credential Manager，fixed target 是
  `ted-h/AI-Sister/AzureSpeech/v1`；不進 `config.toml`、log、DB 或 export。使用者輸入時
  它會短暫存在 password 欄位與 Tauri IPC；保存後頁面立即清空，native 回條只准帶
  Present／Missing／Unreadable／Unsupported，不能把 secret 讀回 renderer。
- 不做文字或 MP3 cache；每次 trusted click 都可能重新 POST。停止／換題立即讓 playback
  generation 失效，晚回來的音訊不得播放或快取；但 blocking native POST 不能中途 abort，
  最長仍可能跑到 45 秒 timeout，且 provider 可能已計入用量。UI 不得把「不再播放」寫成
  「網路請求已取消」。
- outbound helper 必須 by-value 吃掉跨行程 shared consent guard，並讓它活過完整 transport；
  所以碰到既有 POST 時，第四張撤回可能等到 timeout，但撤回回覆後舊 snapshot 絕不能才送。
- Microsoft 目前公開列 Azure Speech F0 neural TTS 每月 0.5 million characters；這是
  provider 方案，不是產品保證。是否可用、額度與費用以使用者 Azure 帳號、resource、
  方案及 Microsoft 當下規則為準。
- WebView CSP 繼續只准 IPC，**不要把 Azure host 加進 `connect-src` 或 `media-src`**。
  一般 CI 不打真 Azure。

---

## 二、這個 repo 最常犯的那個錯（已落地 40 次）

**「每一行都是真的，湊起來在說謊」。** 典型長相：

- **兩種零長得一模一樣**：「我數過，是 0」和「我沒數過」印出同一個 `0`。
  → 用 `Option<T>`／明講「沒量到」，不要拿 `0`／`unwrap_or(0)` 兼差。
- **一句寫死的話宣布一件它沒查過的事**：錯誤訊息說「因為 X」，但程式從來沒檢查 X。
- **標籤和它描述的東西不同步**：表格那一列寫「縮到 1280」，實際抓的是原圖。
- **斷言斷在常數上**：測試綠了，但它斷的是自己寫死的那個值，不是程式算出來的。
- **一個計數變成假話**：改了資料來源，但那行「看過 N 次」數的還是舊集合。

**修這一類的時候特別容易再犯一次**（歷史上四十次裡有一大半是修法自己造出來的）：
為了消歧義加旗標，結果只釘了 `true` 那一面；或在同一份 diff 裡拿掉一個假計數，
又在另一個檔案寫一個新的。**改完問自己：這行字在「什麼都沒發生」那場會印成什麼？**

### 具名 struct 不夠，要 newtype

實測過兩次：把參數換成具名 struct 之後，「欄位名對、值填錯」照樣編得過、照樣全綠。
**要嘛用 newtype（`struct WantsImages(bool)`）讓接錯變成編譯錯誤，
要嘛把 getter 收進建構子（吃 `&Footprint` 而不是吃已經取出來的三個 `f64`）。**

---

## 三、地雷（每一條都是真的踩過的）

1. **`#[cfg(windows)]` 的接線層零執行覆蓋。** Linux 的 `cargo test` 連編都不編它；
   `scripts/check-windows.sh` 只有 `fmt`/`check`/`clippy`，**沒有 `cargo test`**；
   Windows CI 雖然跑 `cargo test --workspace`，但**沒有任何測試呼叫**
   `ops.rs` 的 `windows_record` 或 `bench::run`。
   **所以「四道閘門全綠」對那一層什麼都沒證明。** 動到那裡要自己做定點突變驗證。
   已知有效的解法：把純邏輯用 `#[cfg(any(windows, test))]` 搬進射程
   （`footprint_context`、`disk_footprint_report`、`ocr_table` 都這樣做，突變確實被抓到）。

2. **改到這幾個地方，commit 前一定要跑 `./scripts/check-windows.sh`**：
   任何 `#[cfg(windows)]`、`crates/sister-capture/src/windows/`、
   `crates/sister-capture/tests/windows_ocr.rs`、
   `apps/desktop/`。開發機的 `cargo` 編不到那半邊，只有這支腳本編得到。

3. **`apps/desktop/src-tauri/Cargo.lock` 會被 `check-windows.sh` 弄髒。**
   版本號是手改的（沒有腳本），所以每次 bump 版本都要記得重生鎖檔：
   `cargo metadata --manifest-path apps/desktop/src-tauri/Cargo.toml --format-version 1 --offline`。

4. **永遠不要用 `git checkout <file>` 還原被改過的檔案。**
   先 `cp file /tmp/bak`，改完 `cp /tmp/bak file`。（用 checkout 丟掉過未提交的工作。）

5. **CI runner 比開發機慢約 2.1 倍。** 不要拿本機實測的時間門檻當 CI gate，改測行為。

6. **Prettier 沒有在 CI 跑，而且已經有 11 個 UI 檔案過不了。不要順手重排版。**

7. **research/extracts/ 和 research/ted-repos-dna.md 是刻意不公開的**（`.gitignore` 裡，
   所以 clone 下來不會有——不要以為是誰刪掉了）。
   這是 public repo（`teddashh/AI-Sister`）。不要把它們推上去，也不要引用裡面的逐字內容。

8. **切完 tag 要確認 release job 真的把 exe 發出去了。**
   Linux job 一紅，release job 會被靜靜跳過。

---

## 四、閘門

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
./scripts/check-windows.sh          # 動到 windows/ 或 apps/desktop/ 才需要，但很便宜
./scripts/check-no-network.sh       # 隱私：只准 desktop 的 Persona fixed GET 與 Azure TTS fixed POST；其餘無 client/socket，WebView 無遠端來源
```

CI（`.github/workflows/ci.yml`）另外還跑十幾支 `scripts/check-*.{py,mjs,sh}`，
每一支都在守一句**文件裡寫過的承諾**。加功能的時候順手看一眼有沒有哪支該跟著長。

---

## 五、實測到的數字與已定案取捨（不要再重新量，也不要推翻）

### 現況：alpha.46 的 CPU／RAM；alpha.47 的磁碟歸因（2026-08-23）

Ted 的真 Windows、1920×1080，錄 60 秒（71 拍）；當時在 Better Agent terminal
之間切換 workspace 寫程式：

| 項目 | 實測 | Phase 0 狀態 |
|---|---|---|
| CPU 平均 | **44.0%** | ✅ 已接受的活躍寫程式基準；仍照實量，但不再是 blocker |
| RAM 峰值 | **73.7 MB** | ✅ 通過 < 400 MB |

時間表仍幾乎全在 OCR：11 次 × 2407.6 ms = **26.48 秒，占 84%**；
抓圖 41 次 × 69.2 ms = 2.84 秒，OCR gate 41 次 × 26.4 ms = 1.08 秒。
changed-region 在這個正常寫程式情境裡 **10 次全部退回全幅，成功局部 0 張**，
OCR 嘗試像素 22,809,600 / 全幅候選 22,809,600（100%）。

**Ted 於 2026-08-23 選擇記憶密度優先**：保留現在的觀察頻率，CPU 44.0% 作為
Phase 0 已接受基準，不再用 `<3%` 擋 milestone，也不要發明 45% 之類的新門檻。
CPU 仍要每場照實印；SPEC §2.3 的 `<3%` 保留為長期產品目標，不是 Phase 0 gate。
不要再降 `OCR_LONG_EDGE`、重跑 changed-region 對抗稽核，或改做 DXGI 來追這一格。

alpha.47 的真 Windows 60 秒歸因已回收，不要再要求重跑診斷：

| 磁碟口徑 | 實測 |
|---|---|
| 畫面 | **2.7 MB** |
| 摘要所稱「其他」 | **2.9 MB** |
| 其中 `sister.db-wal` | **2.8 MB** |
| SQLite 邏輯配置增加 | **156 KB** |

收尾當時印出 **4.3GB/天，其中「其他」4.1GB/天**；這兩個數字把可重用 WAL
工作檔的短場淨變化當成每天永久增長，所以**不作 Phase 0 判決**。main 已把每日
計帳改成 SQLite 邏輯配置 + 畫面，WAL 只另列、不外推。按 156KB 邏輯配置粗算，
資料庫約 **219MB/天**；加上 **250MB/天**圖上限約 **469MB/天**，仍高於長期
目標。Ted 已定案先做功能與體驗，**容量優化延後且不再是目前 milestone blocker**。
保持現行圖額度，不要再算 75／250、不要為容量停下來問，也不要發一版只為重跑
相同診斷；直接進 Phase 2 功能。

### 歷史：alpha.44（保留來解釋走過的路，不是現在的 gate）

同一台真 Windows、1920×1080，錄 60 秒（82 拍）：CPU 38.8%、RAM 59.7MB、
磁碟 2.7GB/天。**當時**的 Phase 0 CPU 預算是 < 3%，所以報成超標 13 倍；
CPU 的 82% 是 OCR（8 次 × 2798.1 ms = 22.39 秒），抓圖只有 14%
（54 次 × 70.4 ms = 3.80 秒）。這組數字催生了縮圖 bench 與 changed-region，
不是叫下一個人再走一次。

- alpha.45 已實測排除縮圖：1568／1280 比原生更慢，1024 只快約 11% 且對兩次
  原生共同的 115 行完整字串命中 0 行。具體數字在 `docs/WINDOWS-CHECKLIST.md`。
- **擷取那條路已經結案，繼續不要動**：`sister bench` 量到建立 GDI 物件 0.0–0.1 ms
  （快取 bitmap 省不到東西）、CAPTUREBLT vs 純 SRCCOPY 兩組差值**正負號相反＝雜訊＝那個旗標不用錢**、
  BitBlt 42–48 ms 是全部。照 bench 自己的判準只有 DXGI 救得了——但抓圖不是
  alpha.46 的主成本，**不要做**。
- **磁碟「其他」已由 alpha.47 歸因為主要是 WAL；先前已排除的錯誤診斷仍不要再推一次：**
  `disk_at_start`（`ops.rs`）排在 `Db::open` 和 `Recorder::new` **之後**，所以建 schema
  那 ~176 KB 整包在基準線裡，從來沒被外推過。實測反而是全新資料庫第一場
  `disk_delta = 0`（schema 配出來的空頁吸收掉寫入），第一場**低報**。

---

## 六、寫給人看的字

這個產品的每一句 UI 文案都要能被驗證。文件和程式的用語跟著 repo 現有的走：
繁體中文、直述、不客套、**不要承諾程式沒有在做的事**。
`README.md:4`：「95% 的時間安靜，該說話的時候才說話——說的每一句都能點開證據。」

`docs/PHASES.md` 的 Phase 1 那節寫著**明確不做**：主動開口（她只回答，不先說話）、
interpreter、承諾表。有人拿「虛擬女友／會主動講話的數位人」當參考的時候，那是反方向。
