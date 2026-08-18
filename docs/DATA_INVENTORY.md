# DATA_INVENTORY — 她到底存了什麼

> 這份文件描述 **schema v1**（`sister-core` 的 `MIGRATION_001`）。
>
> 規則：**動到訊號面的 PR 必須同步改這份文件**（PHASES.md §工作紀律 3）。
> 如果程式碼多存了一個欄位而這裡沒寫，那是 bug，不是文件落後。

一切都在**一個檔案**裡：`<資料目錄>/sister.db`。畫面檔在旁邊的
`frames/YYYY/MM/DD/`。備份、加密、刪除，都只有這兩個對象。

    Windows   %APPDATA%\ai-sister\
    Linux     ~/.local/share/ai-sister/

沒有雲端、沒有遙測、沒有帳號。目前整份程式碼**零次模型呼叫**。

---

## 快速回答

| 問題 | 答案 |
|---|---|
| 有存我按了什麼鍵嗎？ | **沒有。** 只存計數與節奏，見 `input_metrics` |
| 有存螢幕截圖嗎？ | 有，降採樣後的 PNG。可用 `store_images = false` 關掉 |
| 有存我複製的東西嗎？ | 有，但疑似秘密者只存「發生過」，不存內容 |
| 有存密碼嗎？ | 密碼管理員整段不擷取；密碼欄位在螢幕上本來就是圓點 |
| 網銀畫面呢？ | 網址命中 blocklist 就整段不擷取——**但見下方「已知缺口」** |
| 資料會離開這台機器嗎？ | 不會。沒有任何網路輸出路徑 |

---

## 表一覽

### `frames` — 保留下來的畫面

| 欄位 | 內容 | 敏感度 |
|---|---|---|
| `ts` | 毫秒時間戳 | 低 |
| `monitor` / `width` / `height` | 哪一台螢幕、多大 | 低 |
| `dhash` | 感知雜湊（去重用，無法還原畫面） | 低 |
| `image_path` | PNG 相對路徑；`NULL` = text-only 模式 | — |
| `image_bytes` | 檔案大小 | 低 |
| `dup_run` | 這張畫面連續重複了幾次 | 低 |
| `app_id` / `window_title` / `url` | 當下的脈絡 | **中**：標題與網址常含人名、案號、單號 |

畫面檔本身是**最敏感的東西**：螢幕上有什麼，它就有什麼。預設保留 30 天。

### `ocr_blocks` — 畫面上的文字（含位置）

`text` + `x/y/w/h` + `confidence`。**敏感度最高**，因為這是螢幕內容的可搜尋副本。
外鍵 `ON DELETE CASCADE`：刪掉一張 frame，它的文字跟著消失。

一列是**一行字**，不是一個詞。Windows 的 OCR 給的是逐詞的方框，我們用詞與詞
之間的間距把它們組回一行——不能用引擎給的整行字串，因為那是用空白接的，
中文會變成「本 期 應 繳」。詳見 `crates/sister-capture/src/ocr_layout.rs`。

> `confidence` 目前一律是 **`-1`**，代表「這個引擎不回報信心度」。
> 用 -1 而不是 1.0 是刻意的：萬一以後有人寫了 `WHERE confidence > 0.5`，
> -1 會讓所有文字一次全部消失（立刻被發現），1.0 則會是一個永遠沒有人
> 發現的謊。

### `text_chunks` + `text_fts` / `text_fts_uni` — 搜尋索引

一張 frame 的所有 OCR 文字接成一段（`\n` 相接）存成一列，這樣「本期應繳金額」
不會因為被切成兩個區塊而搜不到。兩個 FTS5 external-content 索引跟著同步：
trigram 給中日韓、unicode61 給英文。

> **注意**：FTS5 索引是文字的**另一份副本**。刪除必須經過 `text_chunks`
> 的觸發器，直接 `DELETE FROM text_chunks` 以外的路徑會留下孤兒索引。

### `focus_events` — 換到哪個視窗

`kind`（focus / title_change / url_change）+ `app_id` / `app_name` /
`window_title` / `url` / `pid`。脈絡變了才寫一列，不是每秒一列。

### `clipboard_events` — 複製了什麼

| 欄位 | 內容 |
|---|---|
| `kind` | text / image / files |
| `text` | 內容。**疑似秘密時為 `NULL`** |
| `byte_len` | 長度（即使內容沒存也記，因為長度本身無害而有用） |
| `truncated` | 是否超過 64 KB 被截斷 |
| `secret_suspected` | 1 = 偵測到疑似秘密，內容**沒有**落地 |
| `source_app` | 從哪個程式複製的 |

只存文字類內容。圖片與檔案只記「發生過」與來源，不存本體。

### `input_metrics` — 打字節奏（**不含內容**）

`keystrokes` / `clicks` / `mouse_px` / `scroll_ticks` / `window_switches` /
`idle_ms` / `typing_bursts`，預設每 10 秒一列。

這是「事後補不回來」的訊號：卡住、專注、焦慮都藏在這些數字裡。
Windows 的鍵盤 hook **從未解參考 `KBDLLHOOKSTRUCT`**——按鍵碼從來沒有進入
過這個程序的記憶體。這比「我們有記得過濾」強，因為它不需要你相信任何人。

### `facts` — L1 抽取出來的事實

`kind`（money / phone / url / email / file_path / error_code / id_like /
datetime）、`raw`（螢幕原文）、`normalized`、`confidence`，以及回指
`chunk_id` / `frame_id` / `app_id` / `window_title` / `url` 的出處。

**純 regex，零模型。** 敏感度等同來源文字：螢幕上有電話，這裡就有電話。

### `system_events` — 她自己的動作

`kind`（session_start/end、lock、unlock、capture_paused/resumed、
**excluded**）+ `detail`。

`excluded` 這一列是**稽核用的**：它記錄「這段時間因為某規則沒有擷取」，
理由字串裡含 app 名稱。這是刻意的——沒有它，使用者無法驗證排除真的生效了。

### `sessions` / `meta`

程式版本、平台、起訖時間；schema 版本。

---

## 不存在的東西

以下**沒有任何表、任何欄位**承接：

- 按鍵內容、輸入法組字內容
- 麥克風、攝影機、任何音訊
- 網路流量、DNS、封包
- 檔案內容（除非它顯示在螢幕上被 OCR 讀到）
- 位置、聯絡人、行事曆
- 任何形式的識別碼上傳、遙測、崩潰回報

---

## 已知缺口（誠實聲明）

**這一節比上面所有內容都重要。** 一條寫得好看但不生效的規則，
比沒有規則更危險——因為使用者會依賴它。

1. **Windows 目前讀不到瀏覽器網址。** UIA 尚未接上，因此
   `excluded_urls` 那一整組規則（網銀、登入頁）**一條都不會生效**，
   瀏覽器畫面只剩視窗標題規則在擋。`sister doctor` 會把這件事列在
   「目前失效的隱私保護」而不是報告「16 條規則 ✓」。

2. **密碼欄位偵測尚未實作。** 目前依賴的是「密碼在螢幕上顯示為圓點」
   這個事實，以及密碼管理員的 app 排除。若使用者按下「顯示密碼」，
   那個畫面會被記下來。

3. **旁人的畫面。** 會議 app 前景時自動暫停（Zoom/Teams/Meet 等），
   但 Slack 與 Discord **不在**該清單——它們平時是主要工作場所，
   光憑 app 名稱分不出「正在分享畫面」與「正在聊天」。在 Slack 裡開啟
   螢幕分享時，對方的畫面可能被記錄。

4. **排除是規則式的，不是語意式的。** 沒有列進 blocklist 的敏感網站
   會被完整記錄。預設清單只涵蓋常見的台灣銀行與幾個登入頁。

5. **她只讀得懂你裝了語言包的語言。** Windows 的 OCR 是逐語言安裝的。
   沒裝中文時引擎**不會報錯**，它會挑一個裝了的（通常是英文），然後把
   滿螢幕的中文讀成空白——錄製正常、資料庫在長大，只有搜尋永遠是空的。
   `sister doctor` 的「讀字」那一段會印出**實際挑中的**語言，
   不是設定檔裡的偏好清單。

---

## 怎麼自己查證

```bash
sister stats            # 記了多少、佔多少空間
sister doctor           # 排除規則、失效的保護、schema 版本
sister query <關鍵字>   # 每一筆都附出處
sister facts --kind phone
```

驗證排除真的生效：

```bash
cargo test -p sister-capture --test privacy
```

那個測試會跑一段踩滿地雷的腳本，然後把**整個資料目錄當成位元組**掃過，
確認不該存在的字串一個都不在。它不依賴任何人記得要檢查哪個欄位。
