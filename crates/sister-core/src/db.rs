//! 本機儲存：L0 證據與 L1 事實。
//!
//! 憲法（SPEC §0）：這裡的資料 append-only、不經 LLM、不可改寫。
//! 一顆加密的 SQLite 檔裝下全部——備份、加密、刪除都只有一個對象。
//!
//! 檢索走 FTS5 三索引（SPEC §15）：
//! - `text_fts`（trigram）：CJK 子字串比對的主力，繁中天然支援、免字典。
//!   比不了少於 3 個字的東西——這是 trigram 的定義，不是設定問題。
//! - `text_fts_uni`（unicode61）：補英文整詞（`dns` 這種短英文詞它答得出來，
//!   trigram 反而不行）。
//! - `text_fts_bi`（schema 3）：存切好的**相鄰雙字**，補兩個字的中文。
//!
//! 為什麼要第三個索引：這段註解本來寫著 unicode61「補 <3 字的查詢」。
//! **那句話對中文是假的**——unicode61 不是逐字切 CJK 的，它把「客服專線」
//! 整串當成**一個** token，所以 `MATCH "客服"` 是 0 筆。兩個字的中文詞
//! ——也就是中文裡最常見的詞長——因此沒有任何索引可用，只剩
//! [`Db::search_like`] 的掃描：45 天語料 224 ms，而且為了不讓成本跟著使用
//! 時間長大，只好夾在 `LIKE_SCAN_DAYS` 天內。加上 bigram 之後是 0.1 ms、
//! 沒有時間界線，代價是資料庫大 29%。
//!
//! 掃描這條路沒有拆掉：**一個字**的查詢產不出相鄰雙字，那條仍然走掃描。

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::facts::ExtractedFact;
use crate::model::{
    ClipboardEvent, FocusEvent, FocusSnapshot, FrameCapture, InputMetrics, Millis, SearchHit,
    SourceKind, SystemEvent, now_ms,
};

/// 目前的 schema 版本。每次改結構就 +1 並附一段 migration。
pub const SCHEMA_VERSION: i32 = 12;

const MIGRATION_001: &str = r#"
CREATE TABLE IF NOT EXISTS meta (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS sessions (
  id          INTEGER PRIMARY KEY,
  started_at  INTEGER NOT NULL,
  ended_at    INTEGER,
  app_version TEXT NOT NULL,
  platform    TEXT NOT NULL,
  note        TEXT
);

-- L0：畫面。image_path 為 NULL 代表 text-only 保留模式。
CREATE TABLE IF NOT EXISTS frames (
  id           INTEGER PRIMARY KEY,
  ts           INTEGER NOT NULL,
  session_id   INTEGER REFERENCES sessions(id),
  monitor      INTEGER NOT NULL DEFAULT 0,
  width        INTEGER NOT NULL,
  height       INTEGER NOT NULL,
  dhash        INTEGER NOT NULL,
  image_path   TEXT,
  image_bytes  INTEGER NOT NULL DEFAULT 0,
  dup_run      INTEGER NOT NULL DEFAULT 0,
  app_id       TEXT,
  window_title TEXT,
  url          TEXT
);
CREATE INDEX IF NOT EXISTS idx_frames_ts ON frames(ts);

-- L0：OCR 區塊幾何。文字本身另存 text_chunks 供檢索。
CREATE TABLE IF NOT EXISTS ocr_blocks (
  id         INTEGER PRIMARY KEY,
  frame_id   INTEGER NOT NULL REFERENCES frames(id) ON DELETE CASCADE,
  text       TEXT NOT NULL,
  x INTEGER, y INTEGER, w INTEGER, h INTEGER,
  confidence REAL
);
CREATE INDEX IF NOT EXISTS idx_ocr_frame ON ocr_blocks(frame_id);

CREATE TABLE IF NOT EXISTS focus_events (
  id           INTEGER PRIMARY KEY,
  ts           INTEGER NOT NULL,
  session_id   INTEGER REFERENCES sessions(id),
  kind         TEXT NOT NULL,
  app_id       TEXT,
  app_name     TEXT,
  window_title TEXT,
  url          TEXT,
  pid          INTEGER
);
CREATE INDEX IF NOT EXISTS idx_focus_ts ON focus_events(ts);

CREATE TABLE IF NOT EXISTS clipboard_events (
  id               INTEGER PRIMARY KEY,
  ts               INTEGER NOT NULL,
  session_id       INTEGER REFERENCES sessions(id),
  kind             TEXT NOT NULL,
  text             TEXT,
  byte_len         INTEGER NOT NULL,
  truncated        INTEGER NOT NULL DEFAULT 0,
  secret_suspected INTEGER NOT NULL DEFAULT 0,
  source_app       TEXT
);
CREATE INDEX IF NOT EXISTS idx_clip_ts ON clipboard_events(ts);

-- L0：輸入動態。永遠不含按鍵內容，只有節奏與計數。
CREATE TABLE IF NOT EXISTS input_metrics (
  id              INTEGER PRIMARY KEY,
  ts_start        INTEGER NOT NULL,
  ts_end          INTEGER NOT NULL,
  session_id      INTEGER REFERENCES sessions(id),
  keystrokes      INTEGER NOT NULL DEFAULT 0,
  clicks          INTEGER NOT NULL DEFAULT 0,
  mouse_px        INTEGER NOT NULL DEFAULT 0,
  scroll_ticks    INTEGER NOT NULL DEFAULT 0,
  window_switches INTEGER NOT NULL DEFAULT 0,
  idle_ms         INTEGER NOT NULL DEFAULT 0,
  typing_bursts   INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_input_ts ON input_metrics(ts_start);

CREATE TABLE IF NOT EXISTS system_events (
  id         INTEGER PRIMARY KEY,
  ts         INTEGER NOT NULL,
  session_id INTEGER REFERENCES sessions(id),
  kind       TEXT NOT NULL,
  detail     TEXT
);
CREATE INDEX IF NOT EXISTS idx_sys_ts ON system_events(ts);

-- 統一文字層：所有可檢索文字的單一入口（FTS 的 external content）。
CREATE TABLE IF NOT EXISTS text_chunks (
  id           INTEGER PRIMARY KEY,
  ts           INTEGER NOT NULL,
  session_id   INTEGER REFERENCES sessions(id),
  source_kind  TEXT NOT NULL,
  source_id    INTEGER,
  frame_id     INTEGER,
  app_id       TEXT,
  window_title TEXT,
  url          TEXT,
  text         TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_chunk_ts ON text_chunks(ts);
CREATE INDEX IF NOT EXISTS idx_chunk_frame ON text_chunks(frame_id);

CREATE VIRTUAL TABLE IF NOT EXISTS text_fts USING fts5(
  text, content='text_chunks', content_rowid='id', tokenize='trigram'
);
CREATE VIRTUAL TABLE IF NOT EXISTS text_fts_uni USING fts5(
  text, content='text_chunks', content_rowid='id', tokenize='unicode61'
);

CREATE TRIGGER IF NOT EXISTS text_chunks_ai AFTER INSERT ON text_chunks BEGIN
  INSERT INTO text_fts(rowid, text) VALUES (new.id, new.text);
  INSERT INTO text_fts_uni(rowid, text) VALUES (new.id, new.text);
END;
CREATE TRIGGER IF NOT EXISTS text_chunks_ad AFTER DELETE ON text_chunks BEGIN
  INSERT INTO text_fts(text_fts, rowid, text) VALUES('delete', old.id, old.text);
  INSERT INTO text_fts_uni(text_fts_uni, rowid, text) VALUES('delete', old.id, old.text);
END;

-- L1：程式抽出的 typed facts。零 LLM、零幻覺。
CREATE TABLE IF NOT EXISTS facts (
  id           INTEGER PRIMARY KEY,
  ts           INTEGER NOT NULL,
  session_id   INTEGER REFERENCES sessions(id),
  kind         TEXT NOT NULL,
  raw          TEXT NOT NULL,
  normalized   TEXT NOT NULL,
  confidence   REAL NOT NULL,
  source_kind  TEXT NOT NULL,
  chunk_id     INTEGER REFERENCES text_chunks(id) ON DELETE CASCADE,
  frame_id     INTEGER,
  app_id       TEXT,
  window_title TEXT,
  url          TEXT,
  byte_start   INTEGER,
  byte_end     INTEGER
);
CREATE INDEX IF NOT EXISTS idx_facts_kind_ts ON facts(kind, ts);
CREATE INDEX IF NOT EXISTS idx_facts_norm ON facts(normalized);
CREATE INDEX IF NOT EXISTS idx_facts_ts ON facts(ts);
"#;

/// 拿掉 `facts.confidence`。
///
/// 那個欄位每一列都寫，然後沒有任何一行程式讀它——連它自己的註解宣稱的
/// 用途（規則搶同一段文字時決定誰贏）都是假的，去重讀的是 `kind.priority()`。
/// 留著它只會讓「信心 0.93」繼續出現在畫面和 JSON 上，讓人以為有校準過。
///
/// 舊資料庫裡的值不值得搬去別的地方：它不是量出來的。
const MIGRATION_002: &str = r#"
ALTER TABLE facts DROP COLUMN confidence;
"#;

/// 兩個字的中文詞的索引。見 [`cjk_bigrams`]。
///
/// 刪除掛在 trigger 上（照 rowid 刪，不需要重算 bigram），新增則在
/// `insert_chunk_tx` 裡明寫——那樣就不必為了 trigger 去註冊一個
/// 「每條連線都必須存在、否則寫入直接失敗」的自訂 SQL 函式。
const MIGRATION_003: &str = r#"
CREATE VIRTUAL TABLE IF NOT EXISTS text_fts_bi USING fts5(text, tokenize='unicode61');

DROP TRIGGER IF EXISTS text_chunks_ad;
CREATE TRIGGER IF NOT EXISTS text_chunks_ad AFTER DELETE ON text_chunks BEGIN
  INSERT INTO text_fts(text_fts, rowid, text) VALUES('delete', old.id, old.text);
  INSERT INTO text_fts_uni(text_fts_uni, rowid, text) VALUES('delete', old.id, old.text);
  DELETE FROM text_fts_bi WHERE rowid = old.id;
END;
"#;

/// 題庫。**你問過她什麼**，以及她答對了沒有。
///
/// PHASES.md Phase 1 的 scope 有這一條（「Query log 開始累積（本機）：每次提問
/// ＋點擊了哪個出處＝未來題庫」），而 Phase 2 的退場條件直接吃它的產物：
/// 「題庫 ≥ 100 題 recall QA，其中 ≥ 30 題來自真實 query log」。這種東西**補
/// 建不回來**——沒有人記得住自己上禮拜是用什麼字問的，而那正是題庫唯一有價值
/// 的部分（真實的用詞，不是我坐在這裡想像的用詞）。所以它要在自用的第一天就
/// 在，不是等到 Phase 2 開工那天。
///
/// **0 筆的那些查詢是這裡面最貴的資料。** 找得回來的那些只證明它現在能做什麼；
/// 找不回來的那些，才是下一版該修的東西。所以查不到也照記，而且記的是原話。
///
/// 點擊另外一張表，因為它是**兩件事**：問了什麼、以及哪一筆真的有用。後者是
/// 檢索品質唯一不需要人工標註就拿得到的訊號——他點下去的那一刻，等於幫那一題
/// 標了正解。一個問題可以點很多筆，也可以一筆都不點（那本身也是訊號）。
const MIGRATION_004: &str = r#"
CREATE TABLE IF NOT EXISTS queries (
  id         INTEGER PRIMARY KEY,
  ts         INTEGER NOT NULL,
  question   TEXT NOT NULL,
  shape      TEXT NOT NULL,
  hits       INTEGER NOT NULL,
  latency_ms INTEGER NOT NULL,
  source     TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_query_ts ON queries(ts);

CREATE TABLE IF NOT EXISTS query_clicks (
  id       INTEGER PRIMARY KEY,
  query_id INTEGER NOT NULL REFERENCES queries(id) ON DELETE CASCADE,
  chunk_id INTEGER NOT NULL,
  rank     INTEGER NOT NULL,
  ts       INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_click_query ON query_clicks(query_id);
"#;

/// `ever_stored`：**她有沒有真的存下來過一列內容。**
///
/// `ever_recorded` 答不出這一題，而它自己的註解早就寫著答不出來（見
/// [`Db::ever_recorded`]：「旗標在 `start_session` 就翻成 1，第一張畫面之
/// 前」）。我照樣拿它去代表這一題，於是一台 `capture.enabled = false` 的機器
/// ——她跑完、一個字都沒記到、`forget` 從來沒被執行過——在 `stats`、`facts`、
/// `doctor` 三個地方被告知「被 `sister forget` 忘掉了，或是過了保留期」。
///
/// **這個旗標刻意不是 Rust 寫的。** 每一次「加一個計數器／加一個旗標」的修
/// 法，我都犯在呼叫端：六個 insert 函式，漏掉一個就是漏掉一種內容，而測試會
/// 全綠——因為漏掉的那一種沒有 fixture。觸發器把那六個呼叫端變成零個：SQL
/// 那一層看得到每一次 INSERT，繞不過去，`replay`、`record`、migration 回填、
/// 還有測試裡手寫的 `INSERT` 全部一視同仁。
///
/// `WHEN NOT EXISTS(...)` 是為了熱路徑：每一拍都在寫 `frames` 和
/// `text_chunks`，不能每一列都去改一次 `meta`。旗標按下去以後，剩下的每一次
/// INSERT 就只剩那道 `WHEN` 要付。
///
/// **量過，不是估的**：20 萬列純 INSERT 的 microbenchmark 上 +40~45%，換算
/// 約 0.26 µs/列。比例嚇人是因為分母只有一次 INSERT；她一秒鐘寫個位數列，
/// 所以絕對值是零。但這裡本來寫的是「只多付一次主鍵探測」，那句話讀起來像
/// 免費的——寫下一個觸發器的人會照抄，而下一個觸發器可能在一條真的熱的路上。
///
/// `system_events` 那一條多一個 `WHEN NEW.kind NOT IN (...)`：`session_start`
/// 是 `Recorder::new` 的第一個動作，拿它當「存下來過內容」就等於沒修。名單從
/// [`crate::model::SystemKind::session_marks_sql`] 長出來，和
/// `nothing_recorded_left` / `delete_empty_sessions` 是同一份。
///
/// **回填只認現貨。** 升上來的舊資料庫如果現在有內容，就是 `'1'`——量到的。
///
/// 現在是空的、而她**錄過**的那一顆答不出來：被清空過還是從來沒存進去過，檔
/// 案裡沒有任何一個位元分得出來，而這整個 bug 就是「拿一個答不出來的位元去回
/// 答」。所以那一顆寫 `'assumed-at-upgrade'`——不是答案，是一張「這一格是升級
/// 那天補的，不是量到的」的標籤。
///
/// 為什麼標籤而不是留空：留空的話它會讀成 `Barren`，而那台機器在 alpha.32 上
/// 讀的是「被 `sister forget` 忘掉了」。**升級不可以改寫一句關於他的資料的舊
/// 話**——他昨天刪掉一整天，今天升級，然後被告知「一列內容都沒存進來過，先看
/// `capture.enabled`」。那不只是多餘，那是**換了一個診斷、換了一個下一步**，
/// 而那個下一步指向一個他機器上根本沒問題的設定。
///
/// 代價是相反那一種（alpha.32 上就是 `capture.enabled = false` 的機器）繼續讀
/// 到舊的那句話。它照樣是假的——但它是**alpha.32 本來就在說的那句假話**，而
/// 不是這一版新造的。新的位元只對它親眼看見的那幾場說話。
///
/// 真的一場都沒錄過的那一顆什麼都不寫：它不是「答不出來」，它是「沒有問題」。
///
/// 標籤會自己過期：觸發器比的是 `value = '1'`，所以第一列真的落地時它會被覆蓋
/// 成量到的那個 1。
fn migration_005() -> String {
    let mut sql = String::new();
    for (table, when) in CONTENT_TABLES {
        // 觸發器裡的欄位一定要掛 `NEW.`，回填的 `SELECT` 裡一定不能掛——同一
        // 句述詞兩種寫法。所以名單上只留一份，`{q}` 在這裡各自展開，免得兩份
        // 名單哪天只改了一邊。
        let extra = when.map_or(String::new(), |w| {
            format!(" AND {}", w.replace("{q}", "NEW."))
        });
        sql.push_str(&format!(
            "CREATE TRIGGER IF NOT EXISTS {table}_ever_stored AFTER INSERT ON {table}\n  \
             WHEN NOT EXISTS(SELECT 1 FROM meta WHERE key = 'ever_stored' AND value = '1'){extra}\n\
             BEGIN\n  \
             INSERT OR REPLACE INTO meta(key, value) VALUES('ever_stored', '1');\n\
             END;\n"
        ));
    }
    sql.push_str(&format!(
        "INSERT OR REPLACE INTO meta(key, value) SELECT 'ever_stored', '1' WHERE {};\n",
        CONTENT_TABLES
            .iter()
            .map(|(t, w)| match w {
                Some(w) => format!("EXISTS(SELECT 1 FROM {t} WHERE {})", w.replace("{q}", "")),
                None => format!("EXISTS(SELECT 1 FROM {t})"),
            })
            .collect::<Vec<_>>()
            .join(" OR ")
    ));
    // 答不出來的那一顆：她錄過，而現在一列都不剩。上面那一句沒寫成 `'1'` 才
    // 輪得到這一句（`WHERE NOT EXISTS`），順序不能顛倒。
    sql.push_str(
        "INSERT OR REPLACE INTO meta(key, value) \
         SELECT 'ever_stored', 'assumed-at-upgrade' \
         WHERE EXISTS(SELECT 1 FROM meta WHERE key = 'ever_recorded') \
           AND NOT EXISTS(SELECT 1 FROM meta WHERE key = 'ever_stored');\n",
    );
    sql.replace("{marks}", &crate::model::SystemKind::session_marks_sql())
}

/// 「零當機」數的是**活下來的那幾場**，不是**跑過的那幾場**。
///
/// `crash_audit` 一直是 `SELECT COUNT(*) FROM sessions`，而 `sessions` 那張表從
/// alpha.30（[`crate::retention::delete_empty_sessions`]）開始會刪自己的列：一場
/// 「一列內容都沒存到」的錄製，收工的時候連紀錄本身一起走。那是對的，那一列是
/// 「他當天下午 13:02 到 17:44 在電腦前」的證明，#52 要求它消失。
///
/// 代價是這個：**最該被算進去的那一種當機，剛好就是會把自己的證據刪掉的那一
/// 種**。開起來、還沒讀到第一張畫面就死掉——那一場沒有內容，於是下一次
/// `prune` 掃到它、刪掉它，`crash_audit` 的分子和分母同時少一。實測（六場：
/// 一場正常＋五場開機即死）：
///
/// ```text
/// prune 之前   6 段錄製裡有 5 段沒有正常收尾
/// prune 之後   2 段錄製裡有 1 段沒有正常收尾
/// ```
///
/// 訊號是**反的**：她死得越早，紀錄讀起來越乾淨，而一台卡在開機當機迴圈裡的
/// 機器會收斂到「零當機 ✓」。Phase 0 的退場條件是「連續 7 天自我錄製、零當
/// 機」，所以這一格不是裝飾。
///
/// 留著那幾列不是選項（那是 #52 剛拿掉的東西）。所以數字要**撐過那幾列**：兩
/// 個單調的計數器寫在 `meta`，跟 `ever_recorded` / `ever_stored` 同一種東西
/// ——沒有時間、沒有長度、沒有版本，重建不出任何一場錄製，只答得出「她開過幾
/// 次」和「她好好收尾過幾次」。差額就是沒有回來的那幾場。
///
/// 寫在**觸發器**裡而不是寫在 `start_session` / `end_session` 裡，理由和
/// [`migration_005`] 一樣：呼叫端會漏，而這一批 bug 十次有十次犯在呼叫端。
/// 這裡的觸發器一場錄製才跑一次（不是一列內容一次），所以 005 那邊要斤斤計較
/// 的 0.26 µs/列在這裡不存在。
///
/// # 三個觸發器，不是兩個
///
/// 第三個是 `AFTER INSERT ... WHEN NEW.ended_at IS NOT NULL`。今天沒有人這樣
/// 寫（[`Db::start_session`] 是唯一的 INSERT，而它不填 `ended_at`），但哪天有
/// 人補了一場已經結束的錄製進去，少了它就會憑空長出一次當機——而那個假的當機
/// 讀起來跟真的一模一樣。
///
/// # 升級那天的數字是一個下限
///
/// 回填只數得到**還在的那幾列**。一顆跑了三個月、被 `prune` 掃過幾十次的資料
/// 庫，升上來的那一刻真實數字已經不可考。所以回填的同時按下 `session_counts_floor`
/// ——那一顆的句子要說「至少」。這是 alpha.33 那條規矩的同一條：**回填出來的
/// 數字是一個猜測穿著數字的衣服**，要嘛講清楚，要嘛不要講。
///
/// 全新的資料庫不按那個旗標：`ever_recorded` 是 `start_session` 才寫的，所以
/// 沒有那個 key 就代表這顆資料庫是這一版之後才出生的，它的 0 是精確的 0。
fn migration_006() -> String {
    // `INSERT ... ON CONFLICT DO UPDATE` 而不是 `INSERT OR REPLACE`：後者在
    // 這裡會把計數器歸零，因為 REPLACE 用的是新的那一列的值。
    let bump = |key: &str| {
        format!(
            "INSERT INTO meta(key, value) VALUES('{key}', '1')\n    \
             ON CONFLICT(key) DO UPDATE SET value = CAST(CAST(value AS INTEGER) + 1 AS TEXT);"
        )
    };
    let mut sql = format!(
        "CREATE TRIGGER IF NOT EXISTS sessions_started_count AFTER INSERT ON sessions\n\
         BEGIN\n  {}\nEND;\n\
         CREATE TRIGGER IF NOT EXISTS sessions_ended_count AFTER UPDATE OF ended_at ON sessions\n  \
         WHEN OLD.ended_at IS NULL AND NEW.ended_at IS NOT NULL\n\
         BEGIN\n  {}\nEND;\n\
         CREATE TRIGGER IF NOT EXISTS sessions_born_ended_count AFTER INSERT ON sessions\n  \
         WHEN NEW.ended_at IS NOT NULL\n\
         BEGIN\n  {}\nEND;\n",
        bump("sessions_started"),
        bump("sessions_ended"),
        bump("sessions_ended"),
    );
    // **旗標要按在計數器之前，而且要問計數器在不在。** 這一段重跑的時候
    // （版號蓋到一半被砍、然後自我修復）計數器已經是真的了，那時候再按一次
    // 旗標，就是替一顆數得準的資料庫貼上「我數不準」——一句由修法自己造出來
    // 的假話。順序顛倒的話這個條件永遠是 false，看起來還很正常。
    sql.push_str(
        "INSERT INTO meta(key, value) SELECT 'session_counts_floor', '1' \
         WHERE EXISTS(SELECT 1 FROM meta WHERE key = 'ever_recorded') \
           AND NOT EXISTS(SELECT 1 FROM meta WHERE key = 'sessions_started') \
           AND NOT EXISTS(SELECT 1 FROM meta WHERE key = 'session_counts_floor');\n",
    );
    // 回填。`WHERE NOT EXISTS` 讓這一段重跑不會把真的計數蓋回去——001-004 那
    // 個「跑到一半被砍」的窗口在這一段上一樣開著。
    //
    // 聚合寫成子查詢而不是 `... FROM sessions WHERE <guard>`：後者的 `WHERE`
    // 會先濾掉列再 `COUNT(*)`，於是守衛不成立的時候它回的不是「沒有這一列」，
    // 是「一列，值 0」——那正好會把計數器歸零。
    for (key, from) in [
        ("sessions_started", "SELECT COUNT(*) FROM sessions"),
        (
            "sessions_ended",
            "SELECT COUNT(*) FROM sessions WHERE ended_at IS NOT NULL",
        ),
    ] {
        sql.push_str(&format!(
            "INSERT INTO meta(key, value) SELECT '{key}', CAST(({from}) AS TEXT) \
             WHERE NOT EXISTS(SELECT 1 FROM meta WHERE key = '{key}');\n"
        ));
    }
    sql
}

/// 「這一題我本來已經忘了」——他自己按下去的那一個位元。
///
/// PHASES.md Phase 1 的**第一條**退場條件是「自用 7 天內 ≥ 3 次『答對我自己都
/// 忘掉的東西』（記錄實例）」，而在這張表之前沒有任何東西記得住那個實例。
///
/// **點擊不是它。** `query_clicks` 記的是「我點開了證據」，那件事最常發生在她
/// 答錯、或他在查核的時候——跟「她神奇」剛好反過來。`sister queries` 上那句
/// 「其中 M 題你點開了出處」離這條退場條件只有一行遠，卻在講另一件事，所以這
/// 兩個訊號一定要分開存、分開印。
///
/// **而它補不回來。** 「我本來已經忘了」是他看到答案那一刻腦袋裡的狀態；一個
/// 禮拜後回頭翻題庫，翻得出他問過什麼，翻不出他當時知不知道。PHASES.md 自己在
/// 「零當機」那條寫過同一句話：「那是印象，不是條件」。所以這張表要在那七天
/// **開始之前**就在。
///
/// # 為什麼是一張表，不是 `queries` 上的一欄
///
/// 1. 加一欄要 `ALTER TABLE`，而 SQLite 沒有 `ADD COLUMN IF NOT EXISTS`——這一
///    整批 migration 的規矩是重跑不能弄壞東西（見 [`migration_005`] 上面那兩
///    題），`CREATE TABLE IF NOT EXISTS` 免費拿到那件事。
/// 2. 標記是**有時間的**：他可以問完隔一分鐘才想起來「欸這個我早忘了」。一欄
///    布林存不下那個時間，而「7 天內 3 次」數的就是時間。
/// 3. 列在不在就是開關本身，收回 = `DELETE`。一欄要多一個「0 和 NULL 差在
///    哪」的問題，而那正好是這個 repo 反覆在修的那一種。
///
/// `ON DELETE CASCADE` 讓 `forget` 和 `prune` 不必知道這張表存在：題目走了，
/// 掛在它上面的標記跟著走（`PRAGMA foreign_keys=ON` 在 [`Db::init`]，而
/// `a_forgotten_question_takes_its_mark_with_it` 在證明它真的生效）。
const MIGRATION_007: &str = r#"
CREATE TABLE IF NOT EXISTS query_marks (
  query_id INTEGER PRIMARY KEY REFERENCES queries(id) ON DELETE CASCADE,
  ts       INTEGER NOT NULL
);
"#;

/// 斷句結果。從 L0 事件算出來的邊界假設，不是她錄下來的原件。
///
/// **不在錄製熱路徑上寫。** 開時間軸（或明確呼叫）才批次重算。
/// 列本身不 UPDATE：同一段時間重算時先刪舊列再插入。事件被忘掉之後
/// 舊邊界就失效了，留著會讓時間軸繼續講一段已經不存在的活動。
///
/// 升級不回填。Ted 機器上那 272 筆舊資料保持原樣，段落等他打開
/// 時間軸那天再算。空表不是「這一天沒有段落」，是「還沒算過」。
const MIGRATION_008: &str = r#"
CREATE TABLE IF NOT EXISTS segment (
  id              INTEGER PRIMARY KEY,
  started_at      INTEGER NOT NULL,
  ended_at        INTEGER NOT NULL,
  core_started_at INTEGER NOT NULL,
  core_ended_at   INTEGER NOT NULL,
  app_id          TEXT,
  window_title    TEXT,
  url_host        TEXT,
  cut_kinds       TEXT,
  confidence      REAL,
  event_ids       TEXT NOT NULL,
  computed_at     INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_segment_core ON segment(core_started_at);
CREATE INDEX IF NOT EXISTS idx_segment_range ON segment(started_at, ended_at);
"#;

/// 使用者對段落的合併／切開。append-only。
///
/// `segment` 每次打開時間軸都會先刪再插，所以編輯不能寫進那張表——
/// 寫進去下次打開就沒了。這張表重算不動它；重算完再依 id 順序套上去。
///
/// 同時是 SPEC §4.3 的訓練訊號：記動作種類、目標時間、當時演算法的
/// 邊界與信心、發生時間。不記畫面文字、不記他打的字。
///
/// `undo` 是多一列，不 UPDATE 舊列。被撤的那一筆還在，只是套用時跳過。
const MIGRATION_009: &str = r#"
CREATE TABLE IF NOT EXISTS segment_edit (
  id               INTEGER PRIMARY KEY,
  ts               INTEGER NOT NULL,
  kind             TEXT NOT NULL,
  at_ms            INTEGER,
  from_ms          INTEGER,
  to_ms            INTEGER,
  algo_cut_kinds   TEXT,
  algo_confidence  REAL,
  target_id        INTEGER REFERENCES segment_edit(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_segment_edit_range ON segment_edit(from_ms, to_ms);

CREATE TABLE IF NOT EXISTS stuck_signal (
  id                INTEGER PRIMARY KEY,
  started_at        INTEGER NOT NULL,
  ended_at          INTEGER NOT NULL,
  app_id            TEXT,
  window_title      TEXT,
  dwell_ms          INTEGER NOT NULL,
  switch_count      INTEGER NOT NULL,
  error_fact_count  INTEGER NOT NULL,
  computed_at       INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_stuck_start ON stuck_signal(started_at);
"#;

/// L2 卡片 + 外送紀錄。append-only。
///
/// 卡片帶版本鏈（`version` / `supersedes`），Reviewer 之後修訂時再 append
/// 一列，不准 UPDATE 舊列。
///
/// `brain_outbound` **不存送出去的原文**。只記結構和計數。
/// 11：`brain_outbound` 拿掉 `redaction_json`。
///
/// 出境不再去敏（同意書 2 第 3 版），所以那一欄只會裝一份全部是 0 的統計——
/// 一個永遠說「這次換掉了 0 個東西」的欄位，比沒有這個欄位更會騙人。
///
/// 寫成函式而不是一句 SQL：遷移會被**重跑**（版號印錯或停在 0 的資料庫會從頭
/// 走一遍，`a_version_stamp_older_than_its_schema_does_not_brick_the_file`
/// 守著這件事），而 SQLite 沒有 `DROP COLUMN IF EXISTS`，第二次跑就會炸。
fn migrate_011(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    let mut stmt = tx.prepare(
        "SELECT 1 FROM pragma_table_info('brain_outbound') WHERE name = 'redaction_json'",
    )?;
    let present = stmt.exists([])?;
    drop(stmt);
    if present {
        tx.execute_batch("ALTER TABLE brain_outbound DROP COLUMN redaction_json;")?;
    }
    Ok(())
}

/// L3 表 + 血緣圖 + L2 墓碑。冪等：每一欄、每一張表都問過才建。
///
/// L2 的 `evidence_refs` 從這一版起真的寫進 `provenance`。刪一段 L0 時沿這
/// 張圖把衍生的 L2/L3 tombstone——不是實刪，留下「這裡曾經有東西、被刪掉了」。
fn migrate_012(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    add_column_if_missing(
        tx,
        "l2_card",
        "author",
        "TEXT NOT NULL DEFAULT 'interpreter'",
    )?;
    add_column_if_missing(tx, "l2_card", "tombstoned_at", "INTEGER")?;
    add_column_if_missing(
        tx,
        "brain_outbound",
        "role",
        "TEXT NOT NULL DEFAULT 'interpreter'",
    )?;
    tx.execute_batch(MIGRATION_012)?;
    Ok(())
}

fn add_column_if_missing(
    tx: &rusqlite::Transaction<'_>,
    table: &str,
    column: &str,
    decl: &str,
) -> Result<()> {
    let sql = format!("SELECT 1 FROM pragma_table_info('{table}') WHERE name = '{column}'");
    let mut stmt = tx.prepare(&sql)?;
    let present = stmt.exists([])?;
    drop(stmt);
    if !present {
        tx.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {decl};"))?;
    }
    Ok(())
}

const MIGRATION_012: &str = r#"
CREATE TABLE IF NOT EXISTS commitments (
  id                    INTEGER PRIMARY KEY,
  text                  TEXT NOT NULL,
  kind                  TEXT NOT NULL,
  born_from             INTEGER NOT NULL,
  evidence_json         TEXT NOT NULL,
  people_json           TEXT NOT NULL,
  due_hint              TEXT,
  due_source            TEXT,
  due_at                INTEGER,
  status                TEXT NOT NULL,
  confidence            REAL NOT NULL,
  allowed_next_step     TEXT,
  last_evidence_seen_at INTEGER,
  kill_note             TEXT,
  created_at            INTEGER NOT NULL,
  updated_at            INTEGER NOT NULL,
  tombstoned_at         INTEGER
);
CREATE INDEX IF NOT EXISTS idx_commitments_status ON commitments(status, tombstoned_at);
CREATE INDEX IF NOT EXISTS idx_commitments_born ON commitments(born_from);

CREATE TABLE IF NOT EXISTS entities (
  id              INTEGER PRIMARY KEY,
  kind            TEXT NOT NULL,
  name            TEXT NOT NULL,
  aliases_json    TEXT NOT NULL,
  first_seen_ref  TEXT NOT NULL,
  notes           TEXT,
  created_at      INTEGER NOT NULL,
  tombstoned_at   INTEGER
);
CREATE INDEX IF NOT EXISTS idx_entities_name ON entities(kind, name);

CREATE TABLE IF NOT EXISTS entity_mentions (
  id             INTEGER PRIMARY KEY,
  entity_id      INTEGER NOT NULL REFERENCES entities(id),
  seen_ref       TEXT NOT NULL,
  created_at     INTEGER NOT NULL,
  tombstoned_at  INTEGER
);
CREATE INDEX IF NOT EXISTS idx_mentions_entity ON entity_mentions(entity_id);
CREATE INDEX IF NOT EXISTS idx_mentions_seen ON entity_mentions(seen_ref);

CREATE TABLE IF NOT EXISTS day_summaries (
  id                 INTEGER PRIMARY KEY,
  date               TEXT NOT NULL,
  version            INTEGER NOT NULL,
  supersedes         INTEGER REFERENCES day_summaries(id),
  narrative          TEXT NOT NULL,
  session_refs_json  TEXT NOT NULL,
  stats_json         TEXT NOT NULL,
  created_at         INTEGER NOT NULL,
  tombstoned_at      INTEGER
);
CREATE INDEX IF NOT EXISTS idx_day_summaries_date ON day_summaries(date, version);

CREATE TABLE IF NOT EXISTS preferences (
  key           TEXT PRIMARY KEY,
  value         TEXT NOT NULL,
  learned_from  TEXT NOT NULL,
  updated_at    INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS provenance (
  child_ref   TEXT NOT NULL,
  parent_ref  TEXT NOT NULL,
  PRIMARY KEY (child_ref, parent_ref)
);
CREATE INDEX IF NOT EXISTS idx_prov_parent ON provenance(parent_ref);
CREATE INDEX IF NOT EXISTS idx_prov_child ON provenance(child_ref);

CREATE TABLE IF NOT EXISTS reviewer_run (
  id                 INTEGER PRIMARY KEY,
  ts                 INTEGER NOT NULL,
  day_key            TEXT NOT NULL,
  kind               TEXT NOT NULL,
  skip_reason        TEXT,
  candidate_count    INTEGER,
  recheck_count      INTEGER,
  wrote_commitments  INTEGER NOT NULL DEFAULT 0,
  divergences        INTEGER NOT NULL DEFAULT 0,
  calls_used         INTEGER NOT NULL DEFAULT 0,
  budget_used        INTEGER NOT NULL DEFAULT 0,
  budget_limit       INTEGER NOT NULL DEFAULT 0,
  detail             TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS idx_reviewer_run_day ON reviewer_run(day_key);
CREATE INDEX IF NOT EXISTS idx_reviewer_run_ts ON reviewer_run(ts);

CREATE TABLE IF NOT EXISTS reviewer_recheck (
  id                 INTEGER PRIMARY KEY,
  run_id             INTEGER NOT NULL REFERENCES reviewer_run(id),
  category           TEXT NOT NULL,
  child_ref          TEXT NOT NULL,
  parent_ref         TEXT NOT NULL,
  original_present   INTEGER NOT NULL,
  original_chars     INTEGER,
  matched            INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_recheck_run ON reviewer_recheck(run_id);

CREATE TABLE IF NOT EXISTS reviewer_divergence (
  id           INTEGER PRIMARY KEY,
  run_id       INTEGER NOT NULL REFERENCES reviewer_run(id),
  subject      TEXT NOT NULL,
  pass_a_json  TEXT NOT NULL,
  pass_b_json  TEXT NOT NULL,
  reason       TEXT NOT NULL,
  created_at   INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_divergence_run ON reviewer_divergence(run_id);
"#;

const L2_SELECT: &str = "SELECT id, segment_core_start, segment_ref, version, supersedes,
                    activity, entities_json, continues_json, commitments_json,
                    model_confidence, evidence_json, open_questions_json, created_at,
                    author, tombstoned_at";

const MIGRATION_010: &str = r#"
CREATE TABLE IF NOT EXISTS l2_card (
  id                   INTEGER PRIMARY KEY,
  segment_core_start   INTEGER NOT NULL,
  segment_ref          TEXT NOT NULL,
  version              INTEGER NOT NULL,
  supersedes           INTEGER REFERENCES l2_card(id),
  activity             TEXT NOT NULL,
  entities_json        TEXT NOT NULL,
  continues_json       TEXT,
  commitments_json     TEXT NOT NULL,
  model_confidence     REAL NOT NULL,
  confidence_source    TEXT NOT NULL,
  evidence_json        TEXT NOT NULL,
  open_questions_json  TEXT NOT NULL,
  created_at           INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_l2_segment ON l2_card(segment_core_start, version);

CREATE TABLE IF NOT EXISTS brain_outbound (
  id                   INTEGER PRIMARY KEY,
  ts                   INTEGER NOT NULL,
  day_key              TEXT NOT NULL,
  command              TEXT NOT NULL,
  args_json            TEXT NOT NULL,
  segment_core_start   INTEGER,
  chars_sent           INTEGER NOT NULL,
  truncated            INTEGER NOT NULL,
  redaction_json       TEXT NOT NULL,
  outcome              TEXT NOT NULL,
  duration_ms          INTEGER NOT NULL,
  error                TEXT
);
CREATE INDEX IF NOT EXISTS idx_brain_outbound_day ON brain_outbound(day_key);
CREATE INDEX IF NOT EXISTS idx_brain_outbound_ts ON brain_outbound(ts);

CREATE TABLE IF NOT EXISTS brain_skip (
  id                   INTEGER PRIMARY KEY,
  ts                   INTEGER NOT NULL,
  reason               TEXT NOT NULL,
  segment_core_start   INTEGER,
  detail               TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_brain_skip_ts ON brain_skip(ts);
"#;

/// 「她記下來的東西」放在哪幾張表——以及哪幾列不算。
///
/// 這份名單要和 [`DbStats::nothing_recorded_left`] 對得起來：那邊說「一列都不
/// 剩」的時候，這邊必須是「一列都沒進來過」的同一組表。對不起來的話，一顆資
/// 料庫可以同時「什麼都不剩」和「從來沒存過」——那正是這個旗標要拆開的兩種 0
/// 又黏回去了。
///
/// 釘住它的是 `every_table_in_the_schema_is_answered_for`：那條測試的名單是從
/// `sqlite_master` 讀出來的，不是手寫的，所以 schema 長出一張新表就一定要有人
/// 回答「它算不算內容」。（自己比自己的那種寫法抓得到「加錯」，抓不到「漏
/// 加」，而漏加才是這裡會出事的方向。）
///
/// `facts` 和 `ocr_blocks` 不在名單上：它們是 `frames` / `text_chunks` 長出來
/// 的，沒有母體就不會有它們，而母體那兩張已經在名單上了。
const CONTENT_TABLES: &[(&str, Option<&str>)] = &[
    ("frames", None),
    ("text_chunks", None),
    ("focus_events", None),
    ("clipboard_events", None),
    ("input_metrics", None),
    ("system_events", Some("{q}kind NOT IN {marks}")),
];

/// SQLite 在磁碟上的三個檔案，各自是一種量測，不能靠位置猜。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SqliteFileKind {
    Main,
    Wal,
    Shm,
}

/// 某一刻的 SQLite 磁碟配置與實體檔案大小。
///
/// `logical_allocated_bytes` / `objects` / `free_bytes` / `residual_bytes`
/// 是 SQLite 邏輯頁面的同一個口徑；`files` 是檔案系統的另一個口徑，兩組
/// **不能相加**。記憶體資料庫沒有檔案，所以是 `None`，不是三個假的 0。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbDiskSnapshot {
    pub logical_allocated_bytes: u64,
    pub objects: BTreeMap<String, u64>,
    pub free_bytes: u64,
    pub residual_bytes: u64,
    pub catalogued_image_bytes: u64,
    pub files: Option<BTreeMap<SqliteFileKind, Option<u64>>>,
    pub journal_mode: String,
    pub wal_autocheckpoint_pages: u64,
}

/// 一個 SQLite 實體檔案在兩次快照之間的淨變化。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileDelta {
    pub delta_bytes: i64,
    /// 收尾時檔案不存在是 `None`；不能和長度恰好是 0 的檔案混在一起。
    pub end_bytes: Option<u64>,
}

/// 兩次 [`DbDiskSnapshot`] 之間的淨變化。
///
/// 每一項都有正負號：checkpoint、刪除或頁面重用都可能讓數字往下。物件名取
/// 兩邊的聯集，所以消失的物件也不會被漏掉。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbDiskDelta {
    pub logical_allocated_bytes: i64,
    pub objects: BTreeMap<String, i64>,
    pub free_bytes: i64,
    pub residual_bytes: i64,
    pub catalogued_image_bytes: i64,
    pub files: Option<BTreeMap<SqliteFileKind, FileDelta>>,
}

impl DbDiskDelta {
    pub fn between(before: &DbDiskSnapshot, after: &DbDiskSnapshot) -> Result<Self> {
        let object_names: BTreeSet<&String> =
            before.objects.keys().chain(after.objects.keys()).collect();
        let mut objects = BTreeMap::new();
        for name in object_names {
            objects.insert(
                name.clone(),
                signed_byte_delta(
                    before.objects.get(name).copied().unwrap_or(0),
                    after.objects.get(name).copied().unwrap_or(0),
                    &format!("SQLite object {name}"),
                )?,
            );
        }

        let files = match (&before.files, &after.files) {
            (None, None) => None,
            (Some(before_files), Some(after_files)) => {
                let kinds: BTreeSet<SqliteFileKind> = before_files
                    .keys()
                    .chain(after_files.keys())
                    .copied()
                    .collect();
                let mut deltas = BTreeMap::new();
                for kind in kinds {
                    let before_bytes = before_files.get(&kind).copied().flatten().unwrap_or(0);
                    let end_bytes = after_files.get(&kind).copied().flatten();
                    deltas.insert(
                        kind,
                        FileDelta {
                            delta_bytes: signed_byte_delta(
                                before_bytes,
                                end_bytes.unwrap_or(0),
                                &format!("SQLite {kind:?} file"),
                            )?,
                            end_bytes,
                        },
                    );
                }
                Some(deltas)
            }
            _ => anyhow::bail!("不能比較有檔案和沒有檔案的 SQLite 快照——其中一端不是實體資料庫"),
        };

        Ok(Self {
            logical_allocated_bytes: signed_byte_delta(
                before.logical_allocated_bytes,
                after.logical_allocated_bytes,
                "SQLite logical allocation",
            )?,
            objects,
            free_bytes: signed_byte_delta(before.free_bytes, after.free_bytes, "SQLite freelist")?,
            residual_bytes: signed_byte_delta(
                before.residual_bytes,
                after.residual_bytes,
                "SQLite residual pages",
            )?,
            catalogued_image_bytes: signed_byte_delta(
                before.catalogued_image_bytes,
                after.catalogued_image_bytes,
                "catalogued image bytes",
            )?,
            files,
        })
    }
}

fn signed_byte_delta(before: u64, after: u64, label: &str) -> Result<i64> {
    if after >= before {
        i64::try_from(after - before)
            .with_context(|| format!("{label} grew by more than i64 can represent"))
    } else {
        let magnitude = i64::try_from(before - after)
            .with_context(|| format!("{label} shrank by more than i64 can represent"))?;
        Ok(-magnitude)
    }
}

fn nonnegative_bytes(value: i64, label: &str) -> Result<u64> {
    u64::try_from(value).with_context(|| format!("{label} was negative: {value}"))
}

fn path_with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

fn optional_file_len(path: &Path) -> Result<Option<u64>> {
    match std::fs::metadata(path) {
        Ok(metadata) => Ok(Some(metadata.len())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("read metadata for {}", path.display())),
    }
}

/// Replay export 讀的是已經落地的穩定字串；遇到未來版本才認得的值要停，
/// 不能把它默默歸成某個看似接近的事件。少一種事件的語料仍然是合法 JSON，
/// 但它已經不是同一段時間軸。
fn focus_kind_from_db(value: &str) -> Result<crate::model::FocusKind> {
    use crate::model::FocusKind;
    match value {
        "focus" => Ok(FocusKind::Focus),
        "title" => Ok(FocusKind::TitleChange),
        "url" => Ok(FocusKind::UrlChange),
        other => anyhow::bail!("unknown focus event kind in database: {other}"),
    }
}

fn clipboard_kind_from_db(value: &str) -> Result<crate::model::ClipboardKind> {
    use crate::model::ClipboardKind;
    match value {
        "text" => Ok(ClipboardKind::Text),
        "image" => Ok(ClipboardKind::Image),
        "files" => Ok(ClipboardKind::Files),
        other => anyhow::bail!("unknown clipboard event kind in database: {other}"),
    }
}

fn system_kind_from_db(value: &str) -> Result<crate::model::SystemKind> {
    use crate::model::SystemKind;
    match value {
        "lock" => Ok(SystemKind::Lock),
        "unlock" => Ok(SystemKind::Unlock),
        "sleep" => Ok(SystemKind::Sleep),
        "wake" => Ok(SystemKind::Wake),
        "pause" => Ok(SystemKind::CapturePaused),
        "resume" => Ok(SystemKind::CaptureResumed),
        "excluded" => Ok(SystemKind::Excluded),
        "session_start" => Ok(SystemKind::SessionStart),
        "session_end" => Ok(SystemKind::SessionEnd),
        other => anyhow::bail!("unknown system event kind in database: {other}"),
    }
}

fn bool_from_db(value: i64, label: &str) -> Result<bool> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        other => anyhow::bail!("{label} in database was {other}, expected 0 or 1"),
    }
}

/// 資料庫裡一張可重播的文字幀。刻意沒有 `image_path` 或像素欄位：replay
/// corpus 是可人工審查的文字／訊號語料，不是另一份未加密截圖備份。
struct ReplayFrameRow {
    ts: Millis,
    monitor: i32,
    width: u32,
    height: u32,
    dhash: u64,
    dup_run: u32,
    focus: FocusSnapshot,
    ocr: Vec<crate::model::OcrBlock>,
}

/// 一次 replay import 真正落地的東西。空 corpus 不建立一個假的空 session，
/// 所以 session id 是 `Option`，不是拿 0 兼差。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayImport {
    pub session_id: Option<i64>,
    pub events: usize,
    pub frames: usize,
    pub facts: usize,
}

fn replay_event_rank(event: &crate::replay::Event) -> u8 {
    match event {
        crate::replay::Event::System { .. } => 0,
        crate::replay::Event::Focus { .. } => 1,
        crate::replay::Event::Clipboard { .. } => 2,
        crate::replay::Event::Input { .. } => 3,
        crate::replay::Event::Frame { .. } => 4,
    }
}

fn replay_focus_snapshot(focus: &crate::replay::ReplayFocus) -> FocusSnapshot {
    FocusSnapshot {
        app_id: focus.app_id.clone(),
        app_name: focus.app_name.clone(),
        window_title: focus.window_title.clone(),
        url: focus.url.clone(),
        // PID 是這次程序的暫時識別，不是可攜脈絡；password_field 原本就從不
        // 落地，兩者都不能在 import 時猜一個值。
        pid: None,
        password_field: false,
    }
}

pub struct Db {
    /// `pub(crate)` 只為了 [`crate::retention`]：清理要跨好幾張表、還要在
    /// 同一個 transaction 裡跑，包成一堆窄 API 反而更難看出它到底刪了什麼。
    pub(crate) conn: Connection,
}

impl Db {
    /// 開啟（或建立）資料庫，套用 migration。
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create data dir {}", parent.display()))?;
        }
        let conn =
            Connection::open(path).with_context(|| format!("open sqlite at {}", path.display()))?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA foreign_keys=ON;
             PRAGMA busy_timeout=5000;",
        )
        .context("set pragmas")?;

        let mut db = Self { conn };
        db.migrate().context("run migrations")?;
        Ok(db)
    }

    fn migrate(&mut self) -> Result<()> {
        let version: i32 = self
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap_or(0);

        // **比我新的資料庫要擋下來。**
        //
        // 底下每一段都是 `if version < N`，所以一個 schema 5 的資料庫餵給只
        // 認得 4 的執行檔，會一段都不跑、乾乾淨淨地回 Ok——然後拿舊的 SQL 去
        // 讀寫一個結構已經變了的檔案。讀到的東西可能少一半，寫進去的可能
        // 繞過新的觸發器（FTS 索引就是這樣壞的），而畫面上什麼都不會說。
        //
        // 這不是假想：alpha 一版一版發，他機器上同時躺著好幾個 `sister.exe`，
        // 而它們長得一模一樣。點錯一個的代價不該是靜靜地把記憶寫壞。
        //
        // 往回相容是有的（舊資料庫會被升上來），往前沒有——所以這裡只能停。
        if version > SCHEMA_VERSION {
            anyhow::bail!(
                // 版號說得出「哪一顆資料庫」和「差幾版」，說不出**手上這個執行
                // 檔是哪一個**——而他桌面上躺著五個長得一模一樣的 `sister.exe`，
                // 「請改用新版」對著五個一樣的圖示是一句沒有下一步的話。
                "這份資料庫是比較新的版本（schema {version}），而你現在跑的這個 sister 是 {}，只認得到 schema {SCHEMA_VERSION}。\n\
                 舊的執行檔硬開下去讀得到的東西會少、寫進去的可能繞過新的索引，而且不會有人告訴你。\n\
                 請改用新版的 sister（Releases 上最新那一版），或指一個別的 `--data-dir`。",
                env!("CARGO_PKG_VERSION")
            );
        }

        // 已經是最新的就一步都不走。**這一行也是不去搶寫鎖的那一行**：底下每
        // 一段都會拿 `BEGIN IMMEDIATE`，而 `stats` / `query` 這些唯讀指令每天
        // 要開好幾次資料庫，沒有理由為了一段不會跑的 migration 去鎖檔案。
        if version == SCHEMA_VERSION {
            return Ok(());
        }

        // 一級一級走，每一級蓋自己的版號。
        //
        // 這裡本來是「跑完 001 就蓋成 SCHEMA_VERSION」。那樣寫的話，002 若在
        // 半路失敗（舊版 SQLite 不支援 DROP COLUMN 之類），資料庫已經被蓋成
        // 「最新」了——下次開機它不會重試，只會安安靜靜地少跑了一段。
        // 每段各蓋各的，失敗就停在上一段，下次自己接著跑。
        for step in (version + 1)..=SCHEMA_VERSION {
            self.migrate_step(step)
                .with_context(|| format!("migration {step:03}"))?;
        }
        Ok(())
    }

    /// 跑**一段** migration，而且是**整段一起落地或整段都不落地**。
    ///
    /// 兩件事都靠這一個 transaction，而它們以前各壞各的：
    ///
    /// **一、版號要和它描述的結構一起 commit。** 以前是 `tx.commit()` 之後才
    /// `pragma_update`，中間有大約一毫秒。程序在那一毫秒裡被砍掉（`kill -9`、
    /// 拔電、Windows 更新重開），檔案裡就是「結構是新的、版號說是舊的」——
    /// 下次開機它會**再跑一次同一段**，撞上 `table meta already exists` /
    /// `trigger frames_ever_stored already exists`，然後**這顆資料庫再也打不
    /// 開了**。沒有 `sister repair`，逃生路只有手動 sqlite3、刪掉他的記憶、
    /// 或退回舊版執行檔——而最後那一條正是上面那道前向相容閘門在防的事。
    /// 用真的執行檔掃 SIGKILL 的時機，189 次裡中了 1 次。
    ///
    /// `PRAGMA user_version` 是**進 transaction 的**（rollback 會把它退回
    /// 去），所以把它搬進來不用多付任何東西——見
    /// `the_version_stamp_rolls_back_with_the_schema_it_describes`。
    ///
    /// **但這一段只擋得住以後。** 那個狀態已經在外面了：001–004 是這樣發出去
    /// 的，他機器上那顆資料庫可能現在就是。所以**每一段都還要能重跑**——
    /// `IF NOT EXISTS`、`DROP COLUMN` 前面自己問一次、bigram 回填前先清空。
    /// 這一條才是有測試守得住的那一條
    /// （`a_version_stamp_older_than_its_schema_does_not_brick_the_file`，版號
    /// 一路退到 0 掃一遍）。而冪等有它自己的陷阱：它讓「重跑」從**炸掉**變成
    /// **安靜地跑完**，於是 001 那句 `created_at` 會把她的生日蓋成今天——見那
    /// 一行的 `OR IGNORE`。**把撞牆變安靜，就是把下一個錯藏起來。**
    ///
    /// **二、兩個 sister 同時升級同一顆資料庫。** `BEGIN IMMEDIATE` 在這裡拿
    /// 的是寫鎖，而版號要在**拿到鎖之後**再讀一次：兩邊在門外都讀到 4、都決
    /// 定要跑 005，先進去的那個跑完並蓋成 5，後進去的重讀一次就看到 5，直接
    /// 讓開（`the_sister_who_loses_the_race_steps_aside_instead_of_crashing`）。
    /// 真的開兩個行程跑，40 次裡 38 次死在 `already exists`。
    /// `busy_timeout` 救不了這個，那不是鎖等不到，是兩邊都覺得自己該跑。
    ///
    /// 上面那個冪等補上之後，這道重讀拿掉也不會有測試紅——輸的那一邊會把整段
    /// 白跑一次然後安然無恙。**留著是因為「白跑一次」不等於「沒事」**：003 那
    /// 段回填會把整個 bigram 索引砍掉重建，而另一條連線可能正在查它。
    ///
    /// 而這件事**不是** `record` 的心跳擋得住的：`stats`、`query`、`facts`、
    /// `prune`、`export` 和桌面版的 `with_db` 全都直接 `Db::open`，沒有任何
    /// 跨程序的協調。升級後第一次啟動、他一邊按開始記錄一邊打一句問題，走到
    /// 的就是這裡。
    fn migrate_step(&mut self, step: i32) -> Result<()> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        // **拿到寫鎖之後才算數的那一次讀。** 門外那一次是拿來決定要不要排隊的，
        // 這一次是拿來決定要不要做事的。
        let now: i32 = tx
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap_or(0);
        if now >= step {
            return Ok(());
        }
        match step {
            1 => {
                tx.execute_batch(MIGRATION_001)?;
                // `OR IGNORE`，不是 `OR REPLACE`。**上面那幾行冪等把這一行的錯
                // 變安靜了**：以前重跑 001 會撞 `table meta already exists` 而
                // 停在那裡，現在它一路跑得完，然後把「她從哪一天開始記」蓋成今
                // 天。一顆修好了、卻謊報自己生日的資料庫，比一顆打不開的還糟。
                tx.execute(
                    "INSERT OR IGNORE INTO meta(key, value) VALUES('created_at', ?1)",
                    params![now_ms().to_string()],
                )?;
            }
            2 => {
                // 這一段沒有 `IF NOT EXISTS` 可用——`DROP COLUMN` 沒有那個寫
                // 法。所以自己問一次：欄位已經不在就整段讓開。少了這一問，一顆
                // 「002 跑完了、版號還停在 1」的資料庫會撞 `no such column` 而
                // 且**再也打不開**。
                let still_there: i64 = tx.query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('facts') WHERE name = 'confidence'",
                    [],
                    |r| r.get(0),
                )?;
                if still_there > 0 {
                    tx.execute_batch(MIGRATION_002)?;
                }
            }
            3 => {
                tx.execute_batch(MIGRATION_003)?;
                // 重跑的時候索引裡已經有東西了（見上面那段冪等的理由），而
                // fts5 不吃重複的 rowid。先清空——這張表這一段才剛建，正常那
                // 次它本來就是空的。
                tx.execute_batch("DELETE FROM text_fts_bi;")?;
                // 回填舊資料。bigram 要在 Rust 這邊算，所以這段不能寫進 SQL——
                // 少了它，升級上來的資料庫會有一個空索引，然後兩個字的中文查詢
                // 悄悄地只查得到升級之後的東西。
                let mut read = tx.prepare("SELECT id, text FROM text_chunks")?;
                let mut write =
                    tx.prepare("INSERT INTO text_fts_bi(rowid, text) VALUES(?1, ?2)")?;
                let mut rows = read.query([])?;
                while let Some(row) = rows.next()? {
                    let grams = cjk_bigrams(&row.get::<_, String>(1)?);
                    if !grams.is_empty() {
                        write.execute(params![row.get::<_, i64>(0)?, grams])?;
                    }
                }
            }
            4 => tx.execute_batch(MIGRATION_004)?,
            5 => tx.execute_batch(&migration_005())?,
            6 => tx.execute_batch(&migration_006())?,
            7 => tx.execute_batch(MIGRATION_007)?,
            8 => tx.execute_batch(MIGRATION_008)?,
            9 => tx.execute_batch(MIGRATION_009)?,
            10 => tx.execute_batch(MIGRATION_010)?,
            11 => migrate_011(&tx)?,
            12 => migrate_012(&tx)?,
            // ── 加下一段之前，這兩題一定要問 ──────────────────────────
            //
            // 1. **重跑一次會不會安靜地弄壞東西？** 不是「會不會炸」——炸掉是
            //    好事，會被看到。001 那句 `INSERT OR REPLACE ... created_at`
            //    是被「補冪等」的修法**自己造出來的**：重跑從炸掉變成一路跑
            //    完，然後把她的生日蓋成今天。每加一道「讓它不要爆」，就去看那
            //    條路後面本來被爆炸擋住的每一行。
            //
            // 2. **有沒有回填一個「只數得到還在的列」的數字？** 有的話那個數字
            //    是下限不是答案（006 的 `sessions_started`），要按一個 floor
            //    旗標，而句子從此不准說「全部」。旗標本身還有一個坑：它要問
            //    「計數器在不在」才按，不然自我修復那一次會替一顆數得準的資料
            //    庫貼上「我數不準」——又是一句修法自己造出來的假話。
            //
            // 版號走回去再開一次的那條路，
            // `a_version_stamp_older_than_its_schema_does_not_brick_the_file`
            // 會自動掃到新的這一段（它 loop `0..SCHEMA_VERSION`），所以第 1 題
            // 有人幫你問；第 2 題沒有，只有你自己會問。
            // `SCHEMA_VERSION` 加上去了、這裡沒跟上，就會在**第一次真的升級**
            // 的時候炸出來，而不是安安靜靜地少跑一段。
            n => anyhow::bail!("schema {n} 沒有對應的 migration——SCHEMA_VERSION 加了但沒補這一段"),
        }
        tx.pragma_update(None, "user_version", step)?;
        tx.commit()?;
        Ok(())
    }

    pub fn schema_version(&self) -> Result<i32> {
        Ok(self
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))?)
    }

    /// SQLite 執行期版本（doctor 用，trigram 需要 ≥ 3.34）。
    pub fn sqlite_version(&self) -> String {
        rusqlite::version().to_string()
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// 量出 SQLite 此刻的邏輯配置與三個實體檔案大小。
    ///
    /// 這裡刻意不跑 checkpoint、`VACUUM`，也不建暫存表：診斷不能為了量測而
    /// 改變正在量的狀態。`dbstat` 不包含 freelist、pointer-map 和 lock page，
    /// 所以這三塊分開保留，並要求最後一個位元組都對得回
    /// `page_count * page_size`；算不攏就回錯，不用飽和減法藏掉它。
    pub fn disk_snapshot(&self) -> Result<DbDiskSnapshot> {
        let pragma_bytes = |name: &str, label: &str| -> Result<u64> {
            let value: i64 = self
                .conn
                .pragma_query_value(None, name, |row| row.get(0))
                .with_context(|| format!("read PRAGMA {name}"))?;
            nonnegative_bytes(value, label)
        };

        let page_count = pragma_bytes("page_count", "SQLite page_count")?;
        let page_size = pragma_bytes("page_size", "SQLite page_size")?;
        let freelist_count = pragma_bytes("freelist_count", "SQLite freelist_count")?;
        let logical_allocated_bytes = page_count
            .checked_mul(page_size)
            .context("SQLite page_count * page_size overflowed u64")?;
        let free_bytes = freelist_count
            .checked_mul(page_size)
            .context("SQLite freelist_count * page_size overflowed u64")?;

        // Aggregate mode (`1`) 已經把一棵 B-tree 的頁面合成一列；GROUP BY 再把
        // 同名物件釘成唯一一列。名稱直接沿用 SQLite 的真名，不在這裡手工把
        // FTS shadow tables 歸組，否則 schema 多一個物件就會靜靜漏算。
        let mut statement = self
            .conn
            .prepare(
                "SELECT name, SUM(pgsize)
                 FROM dbstat('main', 1)
                 GROUP BY name
                 ORDER BY name",
            )
            .context("prepare SQLite dbstat snapshot")?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .context("query SQLite dbstat snapshot")?;
        let mut objects = BTreeMap::new();
        for row in rows {
            let (name, bytes) = row.context("read SQLite dbstat row")?;
            let bytes = nonnegative_bytes(bytes, &format!("SQLite object {name}"))?;
            objects.insert(name, bytes);
        }
        let object_bytes = objects.values().try_fold(0u64, |sum, bytes| {
            sum.checked_add(*bytes)
                .context("SQLite dbstat object bytes overflowed u64")
        })?;
        let accounted_bytes = object_bytes
            .checked_add(free_bytes)
            .context("SQLite object + freelist bytes overflowed u64")?;
        let residual_bytes = logical_allocated_bytes
            .checked_sub(accounted_bytes)
            .with_context(|| {
                format!(
                    "SQLite page accounting is negative: logical {logical_allocated_bytes}, \
                     objects {object_bytes}, freelist {free_bytes}"
                )
            })?;

        let catalogued_image_bytes: i64 = self
            .conn
            .query_row(
                "SELECT COALESCE(SUM(image_bytes), 0) FROM frames",
                [],
                |row| row.get(0),
            )
            .context("sum catalogued image bytes")?;
        let catalogued_image_bytes =
            nonnegative_bytes(catalogued_image_bytes, "catalogued image bytes")?;

        let journal_mode: String = self
            .conn
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .context("read PRAGMA journal_mode")?;
        let wal_autocheckpoint_pages =
            pragma_bytes("wal_autocheckpoint", "SQLite wal_autocheckpoint")?;

        let main_path = {
            let mut statement = self
                .conn
                .prepare("PRAGMA database_list")
                .context("prepare PRAGMA database_list")?;
            let mut rows = statement.query([]).context("query PRAGMA database_list")?;
            let mut main = None;
            while let Some(row) = rows.next().context("read PRAGMA database_list row")? {
                if row.get::<_, String>(1)? == "main" {
                    main = Some(row.get::<_, String>(2)?);
                    break;
                }
            }
            main.context("PRAGMA database_list did not contain main")?
        };

        // SQLite 用空字串表示 `:memory:`（以及沒有持久檔案的 temporary main）。
        // 它不是三個 0：這一場根本沒有量實體檔案。
        let files = if main_path.is_empty() {
            None
        } else {
            let main = PathBuf::from(main_path);
            let wal = path_with_suffix(&main, "-wal");
            let shm = path_with_suffix(&main, "-shm");
            Some(BTreeMap::from([
                (SqliteFileKind::Main, optional_file_len(&main)?),
                (SqliteFileKind::Wal, optional_file_len(&wal)?),
                (SqliteFileKind::Shm, optional_file_len(&shm)?),
            ]))
        };

        Ok(DbDiskSnapshot {
            logical_allocated_bytes,
            objects,
            free_bytes,
            residual_bytes,
            catalogued_image_bytes,
            files,
            journal_mode,
            wal_autocheckpoint_pages,
        })
    }

    // ---------- sessions ----------

    pub fn start_session(&mut self, platform: &str, app_version: &str) -> Result<i64> {
        self.start_session_at(platform, app_version, now_ms())
    }

    /// 建立一場時間由資料本身決定的錄製。
    ///
    /// 正常錄製一律走 [`Self::start_session`]，只有 replay import 需要把語料的
    /// 相對零點接到呼叫端指定的 epoch。把接縫收在這裡，才能讓 session 容器和
    /// 裡面的每一筆事件落在同一條時間軸上；直接在 import 裡另寫一份 INSERT，
    /// 很容易只補到 session 列、漏掉 `ever_recorded` 這個同一筆交易裡的承諾。
    fn start_session_at(
        &mut self,
        platform: &str,
        app_version: &str,
        started_at: Millis,
    ) -> Result<i64> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO sessions(started_at, app_version, platform) VALUES(?1, ?2, ?3)",
            params![started_at, app_version, platform],
        )?;
        // **在下一句 INSERT 之前拿。** `last_insert_rowid` 講的是這個連線上最後
        // 一次插入，而底下那句 `meta` 也是一次插入——晚一行拿到的是 `meta` 的
        // rowid，於是這一場的 id 會指到一個不存在的 session，接下來每一列都撞
        // 外鍵。（測試當場就抓到了，因為外鍵是 `ON` 的。）
        let id = tx.last_insert_rowid();
        // 同一個 transaction 裡按下這個旗標。理由見 [`Db::ever_recorded`]：
        // `sessions` 那幾列現在會跟著保留期和 `sister forget` 一起消失，而
        // 「她到底有沒有開始記過東西」這個問題**必須**在那之後還答得出來。
        tx.execute(
            "INSERT OR REPLACE INTO meta(key, value) VALUES('ever_recorded', '1')",
            [],
        )?;
        tx.commit()?;
        Ok(id)
    }

    /// 這台機器上，有沒有**曾經**開始過一場錄製。
    ///
    /// 一個布林，不是一個數字，也不是一個時戳——這是刻意的。
    ///
    /// 讀者是每一句會在「什麼都沒有」的時候開口的話。**直接**讀它的有
    /// [`crate::answer::BlindSpots`]（第一個讀者）、`doctor` 的「零當機」和那三
    /// 個訊號稽核、`doctor` 的「你問過她什麼」、`queries` 的空手句，還有三份
    /// `--json`。`stats` 那行 ⚠、`doctor` 的「已記錄」「兩個字的中文」、`facts`
    /// 的空手句現在改走 `ops::Emptiness`——它把這個位元和排除稽核湊成三種 0。
    ///
    /// 一顆全新的資料庫要說「我還沒開始記——按右上角那顆開始記錄」，而一顆
    /// **他自己剛清空**的資料庫絕對不可以說同一句話，那是叫他重做一件他刻意
    /// 做掉的事。
    ///
    /// 它答得出「她錄過」，答**不**出「這張表曾經有列」，也答不出「他問過問
    /// 題」——旗標在 `start_session` 就翻成 1，第一張畫面之前。拿它去代表那些，
    /// 就會在一顆從來沒刪過任何東西的資料庫上宣布東西被刪了（`facts`、
    /// 「兩個字的中文」、「你問過她什麼」都各自犯過一次）。
    ///
    /// 以前這件事是靠「`sessions` 那張表誰都不刪」撐著的，於是
    /// `forget` 那句「那段時間裡的每一張表都清乾淨」是假的：那張表留著
    /// `started_at` / `ended_at` / `app_version` / `platform`——也就是
    /// 「那天下午 13:02 到 17:44 她在錄」。他忘掉的是那段時間裡的畫面，
    /// 而留下來的是一張他當時坐在電腦前四小時四十二分的證明。
    ///
    /// 所以那張表現在會被刪（見 `retention::delete_empty_sessions`），而
    /// 真正被需要的那一個位元搬到這裡：**沒有時間、沒有長度、沒有版本**，
    /// 重建不出任何東西。
    ///
    /// 舊資料庫沒有這個 key，所以「有 session 就算數」是退路——不然升上來的
    /// 那一刻，一台錄了半年的機器會說自己從來沒開始過。
    pub fn ever_recorded(&self) -> Result<bool> {
        let flagged: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM meta WHERE key = 'ever_recorded')",
            [],
            |r| r.get(0),
        )?;
        if flagged {
            return Ok(true);
        }
        Ok(self
            .conn
            .query_row("SELECT EXISTS(SELECT 1 FROM sessions)", [], |r| r.get(0))?)
    }

    /// 這台機器上，他有沒有**曾經**按過一次「這一題我本來已經忘了」。
    ///
    /// 和 [`QueryLogStats::marked`]（現在還剩幾個）是兩題。標記掛在 `queries`
    /// 底下，`forget` 和保留期都會連著帶走——所以「0 個標記」有兩種意思，而
    /// 它們的下一步相反：沒按過的人要知道那個按鈕在哪，按過三次然後把那段時
    /// 間忘掉的人，最不該收到的就是一句「去按按看」。
    pub fn ever_marked(&self) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM meta WHERE key = 'ever_marked')",
            [],
            |r| r.get(0),
        )?)
    }

    /// 這台機器上，有沒有**曾經**真的把字送出程序。
    ///
    /// 和現在 `brain_outbound` 還剩幾列是兩題。`forget` 和保留期會把那些列
    /// 刪掉，所以「一列外送紀錄都沒有」有兩種意思：從來沒送過，和送過、被
    /// 清掉了。面板上那兩句話不能長得一樣。
    ///
    /// 只在 `insert_brain_outbound` 按下。跳過（`brain_skip`）不算送出。
    /// 列還在的時候，沒有這個 key 也算——不然升級上來、旗標還沒按下的那幾
    /// 列會被說成從來沒送過。
    pub fn ever_brain_outbound(&self) -> Result<bool> {
        let flagged: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM meta WHERE key = 'ever_brain_outbound')",
            [],
            |r| r.get(0),
        )?;
        if flagged {
            return Ok(true);
        }
        Ok(self
            .conn
            .query_row("SELECT EXISTS(SELECT 1 FROM brain_outbound)", [], |r| {
                r.get(0)
            })?)
    }

    /// 這台機器上，有沒有**曾經真的存下來過一列內容**。
    ///
    /// [`Db::ever_recorded`] 答的是「她有沒有開始過一場錄製」，而那兩題差一台
    /// `capture.enabled = false` 的機器：她跑完了、一個字都沒進資料庫、
    /// `sister forget` 從來沒被執行過。拿 `ever_recorded` 去回答這一題的下場是
    /// 三個畫面同時宣布「被 `sister forget` 忘掉了，或是過了保留期」。
    ///
    /// 和 `ever_recorded` 一樣是**單調**的：`forget` 和保留期都不碰它，所以
    /// 「她存過東西」這件事撐得過清空——`Erased` 這個答案就是靠它站著的。
    ///
    /// 也和 `ever_recorded` 一樣**答不出**「現在還剩不剩」（那是
    /// [`DbStats::nothing_recorded_left`]）、答不出「他刪過東西」（沒有任何位
    /// 元答得出來，見 [`migration_005`]）。它只答一題：這顆資料庫裡有沒有落過
    /// 地的內容。
    ///
    /// 沒有這個 key 的，是**這一版之後**才真的一列都沒存過的機器。alpha.33
    /// 以前就已經空著升上來的那一顆有 key、值是 `assumed-at-upgrade`——它是一
    /// 張標籤不是一個答案，理由見 [`migration_005`]。這裡不分那兩種值：**這支
    /// 函式回的是「要不要對他說東西被拿走了」**，而升級那天的機器要照 alpha.32
    /// 說過的話繼續說。
    pub fn ever_stored(&self) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM meta WHERE key = 'ever_stored')",
            [],
            |r| r.get(0),
        )?)
    }

    /// 收工。順手把**空的那幾場**清掉。
    ///
    /// 那一句掃描不是為了省空間（一列 sessions 幾十個位元組），是為了關掉一個
    /// 時間窗：`retention::delete_empty_sessions` 不會碰還開著的那一場，因為
    /// `prune` 在錄製迴圈裡自己會跑（開錄時一次，之後每六小時），而正在錄的那一場剛開始時本來
    /// 就是空的。於是「錄到一半按下『忘掉這一整天』」之後，那一場的紀錄要等到
    /// **下一次**錄製開始五分鐘後才會消失——中間他關掉了程式、看了一眼資料庫，
    /// 看到的是一列說著「今天 13:02 到 17:44 她在錄」的紀錄，而他剛剛才按過忘掉。
    ///
    /// 這裡是那一場停止的那一刻，也是它第一次真的可以被判定的那一刻。
    ///
    /// **只掃這一場，不順便掃別人。** 全掃會把「開機沒幾秒就當掉、一列都沒寫
    /// 成」的那幾場一起掃走，而那幾場正是 [`crash_audit`](Self::crash_audit)
    /// 存在的理由——一次乾淨的停止不該把一次當機的證據帶走。
    pub fn end_session(&mut self, session_id: i64) -> Result<()> {
        self.end_session_at(session_id, now_ms())
    }

    /// [`Self::start_session_at`] 的收尾端；正常錄製仍由 [`Self::end_session`]
    /// 取得真實時鐘，replay import 才會指定語料的結尾。
    fn end_session_at(&mut self, session_id: i64, ended_at: Millis) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "UPDATE sessions SET ended_at = ?1 WHERE id = ?2",
            params![ended_at, session_id],
        )?;
        crate::retention::delete_empty_sessions(&tx, Some(session_id))?;
        tx.commit()?;
        Ok(())
    }

    // ---------- L0 writes ----------

    /// 寫入一張保留幀，連同 OCR 區塊、統一文字層、以及從文字抽出的 L1 事實。
    ///
    /// 回傳 (frame_id, chunk_id, facts_written)。整批在同一個 transaction 裡，
    /// 要嘛全成功、要嘛全不留——證據層不接受半截狀態。
    pub fn insert_frame(
        &mut self,
        session_id: i64,
        frame: &FrameCapture,
        image_path: Option<&str>,
        image_bytes: i64,
    ) -> Result<(i64, Option<i64>, usize)> {
        let tx = self.conn.transaction()?;

        tx.execute(
            "INSERT INTO frames(ts, session_id, monitor, width, height, dhash,
                                image_path, image_bytes, app_id, window_title, url)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![
                frame.ts,
                session_id,
                frame.monitor,
                frame.width,
                frame.height,
                frame.dhash as i64,
                image_path,
                image_bytes,
                frame.focus.app_id,
                frame.focus.window_title,
                frame.focus.url,
            ],
        )?;
        let frame_id = tx.last_insert_rowid();

        let mut chunk_id = None;
        let mut fact_count = 0;

        if !frame.ocr.is_empty() {
            {
                let mut stmt = tx.prepare(
                    "INSERT INTO ocr_blocks(frame_id, text, x, y, w, h, confidence)
                     VALUES(?1,?2,?3,?4,?5,?6,?7)",
                )?;
                for b in &frame.ocr {
                    stmt.execute(params![frame_id, b.text, b.x, b.y, b.w, b.h, b.confidence])?;
                }
            }

            // 一幀一個 chunk：跨區塊的句子不會被切斷，snippet 也才讀得懂。
            let joined = frame
                .ocr
                .iter()
                .map(|b| b.text.as_str())
                .collect::<Vec<_>>()
                .join("\n");

            if !joined.trim().is_empty() {
                let id = insert_chunk_tx(
                    &tx,
                    session_id,
                    frame.ts,
                    SourceKind::Ocr,
                    Some(frame_id),
                    Some(frame_id),
                    &frame.focus,
                    &joined,
                )?;
                fact_count = insert_facts_tx(
                    &tx,
                    session_id,
                    frame.ts,
                    id,
                    Some(frame_id),
                    SourceKind::Ocr,
                    &frame.focus,
                    &crate::facts::extract(&joined),
                )?;
                chunk_id = Some(id);
            }
        }

        tx.commit()?;
        Ok((frame_id, chunk_id, fact_count))
    }

    /// 畫面沒變：不新增幀，只把最後一幀的重複計數 +1。
    ///
    /// 這一行就是成本模型的核心——重複的十秒不佔磁碟、不跑 OCR、不進索引。
    pub fn bump_frame_dup(&mut self, frame_id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE frames SET dup_run = dup_run + 1 WHERE id = ?1",
            params![frame_id],
        )?;
        Ok(())
    }

    pub fn insert_focus(&mut self, session_id: i64, e: &FocusEvent) -> Result<i64> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO focus_events(ts, session_id, kind, app_id, app_name, window_title, url, pid)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                e.ts,
                session_id,
                e.kind.as_str(),
                e.snapshot.app_id,
                e.snapshot.app_name,
                e.snapshot.window_title,
                e.snapshot.url,
                e.snapshot.pid,
            ],
        )?;
        let id = tx.last_insert_rowid();

        // 視窗標題與 URL 本身就是高價值的可檢索文字（而且零成本）
        if let Some(title) = e
            .snapshot
            .window_title
            .as_deref()
            .filter(|t| !t.trim().is_empty())
        {
            let cid = insert_chunk_tx(
                &tx,
                session_id,
                e.ts,
                SourceKind::WindowTitle,
                Some(id),
                None,
                &e.snapshot,
                title,
            )?;
            insert_facts_tx(
                &tx,
                session_id,
                e.ts,
                cid,
                None,
                SourceKind::WindowTitle,
                &e.snapshot,
                &crate::facts::extract(title),
            )?;
        }
        if let Some(url) = e.snapshot.url.as_deref().filter(|u| !u.trim().is_empty()) {
            insert_chunk_tx(
                &tx,
                session_id,
                e.ts,
                SourceKind::Url,
                Some(id),
                None,
                &e.snapshot,
                url,
            )?;
        }

        tx.commit()?;
        Ok(id)
    }

    pub fn insert_clipboard(&mut self, session_id: i64, e: &ClipboardEvent) -> Result<i64> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO clipboard_events(ts, session_id, kind, text, byte_len, truncated,
                                          secret_suspected, source_app)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                e.ts,
                session_id,
                e.kind.as_str(),
                e.text,
                e.byte_len,
                e.truncated as i32,
                e.secret_suspected as i32,
                e.source_app,
            ],
        )?;
        let id = tx.last_insert_rowid();

        if let Some(text) = e.text.as_deref().filter(|t| !t.trim().is_empty()) {
            let focus = FocusSnapshot {
                app_id: e.source_app.clone(),
                ..Default::default()
            };
            let cid = insert_chunk_tx(
                &tx,
                session_id,
                e.ts,
                SourceKind::Clipboard,
                Some(id),
                None,
                &focus,
                text,
            )?;
            insert_facts_tx(
                &tx,
                session_id,
                e.ts,
                cid,
                None,
                SourceKind::Clipboard,
                &focus,
                &crate::facts::extract(text),
            )?;
        }

        tx.commit()?;
        Ok(id)
    }

    pub fn insert_input(&mut self, session_id: i64, m: &InputMetrics) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO input_metrics(ts_start, ts_end, session_id, keystrokes, clicks, mouse_px,
                                       scroll_ticks, window_switches, idle_ms, typing_bursts)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                m.ts_start,
                m.ts_end,
                session_id,
                m.keystrokes,
                m.clicks,
                m.mouse_px,
                m.scroll_ticks,
                m.window_switches,
                m.idle_ms,
                m.typing_bursts
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn insert_system(&mut self, session_id: i64, e: &SystemEvent) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO system_events(ts, session_id, kind, detail) VALUES(?1,?2,?3,?4)",
            params![e.ts, session_id, e.kind.as_str(), e.detail],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// 排除稽核：哪幾條規則真的生效過、生效過幾**段**、第一段和最後一段何時開始。
    ///
    /// 這張表以前是只寫不讀的。DATA_INVENTORY 說 `excluded` 這一列「是稽核用的，
    /// 沒有它使用者無法驗證排除真的生效了」——但只有錄製當下的那份記憶體統計
    /// 印得出來，錄完就再也叫不回來。一條查不回來的稽核紀錄，跟沒有稽核紀錄
    /// 對使用者是一樣的。
    ///
    /// **數的是「段」，不是「幀」。** recorder 只在踏進一段排除狀態時寫一列
    /// （見 `last_exclusion` 的去抖），所以在 keepassxc 裡待了十分鐘＝一列。
    /// 被擋掉的畫面張數沒有被存下來，只有那一次錄製的即時統計知道——把這裡的
    /// 數字說成「擋掉幾張畫面」會差好幾個數量級。
    pub fn exclusion_audit(&self) -> Result<Vec<ExclusionAudit>> {
        let mut stmt = self.conn.prepare(
            "SELECT COALESCE(detail, '（沒寫理由）'), COUNT(*), MIN(ts), MAX(ts)
             FROM system_events WHERE kind = 'excluded'
             GROUP BY detail ORDER BY COUNT(*) DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(ExclusionAudit {
                reason: r.get(0)?,
                episodes: r.get(1)?,
                first_ts: r.get(2)?,
                last_ts: r.get(3)?,
            })
        })?;
        Ok(rows.flatten().collect())
    }

    /// 暫停稽核：她總共閉眼多久。
    ///
    /// 存在的理由和 `exclusion_audit` 完全一樣，而且是**照著同一次教訓寫的**：
    /// `capture_paused` / `capture_resumed` 這兩個 kind 從 schema v1 就寫在
    /// DATA_INVENTORY 裡，卻一路到 alpha.12 才有程式碼寫得出它們——如果現在
    /// 只寫不讀，那就是把同一個坑挖第二次，只是換一張表。
    ///
    /// **在 Rust 這邊配對而不是用 SQL window function**，因為配不起來的情況
    /// 才是重點：最後一段沒有 `resume`（現在還在暫停，或錄製在暫停中被砍掉），
    /// 以及保留期把開頭那筆 `pause` 刪掉之後剩下的孤兒 `resume`。這兩種都要
    /// 有明確的答案，不是靜靜地少算一段。
    pub fn pause_audit(&self) -> Result<PauseAudit> {
        let mut stmt = self.conn.prepare(
            "SELECT kind, ts FROM system_events
             WHERE kind IN ('pause', 'resume') ORDER BY ts, id",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Millis>(1)?)))?;

        let mut audit = PauseAudit::default();
        let mut open: Option<Millis> = None;
        for (kind, ts) in rows.flatten() {
            match kind.as_str() {
                // 連續兩筆 pause 不該發生（recorder 只在轉換時寫），但真的
                // 發生時要保留第一筆——那才是她真正閉眼的時刻。
                "pause" if open.is_none() => open = Some(ts),
                "resume" => match open.take() {
                    Some(start) => {
                        audit.episodes += 1;
                        audit.total_ms += (ts - start).max(0);
                    }
                    // 開頭那筆被保留期刪掉了。算一段，但長度不知道——
                    // 猜一個數字進去會讓「總共暫停多久」悄悄變短。
                    None => {
                        audit.episodes += 1;
                        audit.truncated += 1;
                    }
                },
                _ => {}
            }
        }
        if let Some(start) = open {
            audit.episodes += 1;
            audit.open_since = Some(start);
        }
        Ok(audit)
    }

    /// 這段時間裡她閉眼的每一段，給時間軸用。
    ///
    /// `pause_audit` 回的是總計，時間軸要的是**位置**：某一天下午三點到五點
    /// 什麼都沒有，是他去開會了，還是她被關掉了？兩者在時間軸上長得一模一樣，
    /// 而只有其中一種是使用者需要知道的。一個不說明自己空白處的時間軸，會讓
    /// 使用者以為她漏記——然後對整份紀錄失去信任。
    ///
    /// **掃描不從 `from_ts` 開始。** 一段昨天按下、到現在都沒解除的暫停，它的
    /// `pause` 事件落在窗外，可是它蓋住的正是這個窗的**全部**。只看窗內事件的
    /// 話，那一天會回一個空陣列，也就是「這天沒暫停過」——最糟的那種答案。
    ///
    /// 兩端都可以是 `None`，意思不同：`from` 是 None 表示開頭那筆 `pause` 已經
    /// 被保留期刪掉（只知道它在 `to` 之前）；`to` 是 None 表示到這顆資料庫的
    /// 最後一刻都還在暫停。回傳的是**真實**區間，沒有裁到窗內——呼叫端要畫的話
    /// 自己裁，但至少它知道這一段其實更長。
    ///
    /// **掃描也不停在 `to_ts`。** 這句話上一版只對 `from` 那一端成立：SQL 裡
    /// 有一條 `ts < ?1`，於是一段星期五 18:00 按下、星期一 09:00 才解除的暫停，
    /// 在看星期五的時候那筆 `resume` 落在窗外被篩掉，回的是 `to: None`——
    /// 而 `to: None` 在時間軸上印的是「之後沒有再解除」。那是一句關於**接下來
    /// 所有時間**的話，從一份刻意裁到午夜的資料裡講出來，而且和真正「到現在
    /// 都還沒解除」長得一模一樣。
    ///
    /// 證據不只是推論：`timeline.js` 裡有一條專門處理 `p.to > dayEnd` 的分支
    /// （「跨過午夜；這一天裡佔了 X」），而它**到不了**——任何 `ts > dayEnd`
    /// 的 `resume` 正好就是那句 SQL 篩掉的那些。畫面早就知道該講哪一句，只是
    /// 資料永遠不會長成那個形狀。
    ///
    /// 拿掉上界不會變貴：`pause`／`resume` 是人按出來的，一天幾筆，而這一支
    /// 本來就沒有下界（見上一段），整條掃描的量級不變。
    pub fn pause_spans(&self, from_ts: Millis, to_ts: Millis) -> Result<Vec<PauseSpan>> {
        let mut stmt = self.conn.prepare(
            "SELECT kind, ts FROM system_events
             WHERE kind IN ('pause', 'resume') ORDER BY ts, id",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Millis>(1)?)))?;

        let mut all: Vec<PauseSpan> = Vec::new();
        let mut open: Option<Millis> = None;
        for (kind, ts) in rows.flatten() {
            match kind.as_str() {
                // 和 `pause_audit` 同一條規則：連兩筆 pause 時留第一筆。
                "pause" if open.is_none() => open = Some(ts),
                "resume" => all.push(PauseSpan {
                    from: open.take(),
                    to: Some(ts),
                }),
                _ => {}
            }
        }
        if let Some(start) = open {
            all.push(PauseSpan {
                from: Some(start),
                to: None,
            });
        }

        // 留下和 [from_ts, to_ts) 有交集的。未知的那一端當作無限遠——不知道
        // 何時開始的那一段，寧可多畫一條也不要讓一片空白沒人解釋。
        //
        // 上界的篩選以前是 SQL 做的（順便就把跨窗的 `resume` 一起吃掉了），
        // 現在移到這裡：篩的是**整段**在不在窗外，不是那一筆事件在不在窗內。
        all.retain(|s| s.to.is_none_or(|t| t > from_ts) && s.from.is_none_or(|f| f < to_ts));
        Ok(all)
    }

    /// 秘密遮蔽稽核：標記過幾次、其中有幾次內容其實還躺在資料庫裡。
    ///
    /// 第二個數字才是重點。`secret_suspected = 1` 只是一面旗子，而
    /// DATA_INVENTORY 承諾的是「內容**沒有**落地」——那是一句關於 `text` 欄位
    /// 的話。旗子插了但字還在，是這個承諾唯一會失敗的方式，而且它不會報錯：
    /// 錄製摘要照樣印「偵測到 N 次疑似秘密，內容未落地」。
    ///
    /// 所以這裡去問資料庫本身，而不是相信寫入時的那個布林值。
    pub fn redaction_audit(&self) -> Result<RedactionAudit> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(text IS NOT NULL), 0)
             FROM clipboard_events WHERE secret_suspected = 1",
            [],
            |r| {
                Ok(RedactionAudit {
                    flagged: r.get(0)?,
                    leaked: r.get(1)?,
                })
            },
        )?)
    }

    /// bigram 索引蓋到了多少行字。回傳 `(已進索引, 有 CJK 的總行數)`。
    ///
    /// 這個數字存在的理由是 migration 003 要**回填**舊資料。回填如果沒跑到
    /// （舊版本開過這顆資料庫、或當初中途失敗），兩個字的中文查詢會安靜地
    /// 只查得到升級之後錄的東西——沒有錯誤、沒有例外，只是舊的東西再也叫
    /// 不出來。所以 `doctor` 把兩個數字並排印出來，讓它自己講。
    pub fn bigram_coverage(&self) -> Result<(i64, i64)> {
        let indexed: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM text_fts_bi", [], |r| r.get(0))
            .unwrap_or(0);
        let with_cjk: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM text_chunks
             WHERE text GLOB '*[一-鿿]*' OR text GLOB '*[㐀-䶿]*'",
            [],
            |r| r.get(0),
        )?;
        Ok((indexed, with_cjk))
    }

    /// 有幾段錄製沒有正常收尾——也就是 Phase 0 那句「零當機」的實作。
    ///
    /// `finish()` 會寫 `ended_at`，而 Ctrl-C 只設旗標、真正的收尾照樣走
    /// `finish()`（見 `ops.rs` 的 `install_ctrl_c_handler`）。所以
    /// `ended_at IS NULL` 剩下的解釋只有：程序被殺、當機、關機、拔電。
    ///
    /// 在這之前 Phase 0 的退場條件「連續 7 天自我錄製、零當機」，驗證方式是
    /// **使用者要自己記得有沒有當過**。那不是一個退場條件，那是一個印象。
    /// 資料庫一直都知道答案，只是沒有人問過它。
    ///
    /// **有一個沒有被解決的歧義寫在這裡而不是被藏起來**：如果此刻另一個終端
    /// 機正在錄，那一段的 `ended_at` 也是 NULL，看起來跟當機一樣。可以靠存
    /// PID 再去問作業系統那個 PID 還在不在來分辨，但那是一條跨平台的、而且
    /// 會因為 PID 重用而給出錯誤答案的路。心跳檔答得出同一題，所以
    /// 心跳整顆從呼叫端收進來——**而且是收在這裡，不是收在句子那一層**。
    ///
    /// 收的是 [`crate::heartbeat::phase`] 的回傳值，不是一個布林。「有沒有人
    /// 佔著這個目錄」和「**她的那一列已經在資料庫裡了嗎**」是兩題，答案分別
    /// 是 `beat.is_some()` 和 `beat == Some(Recording)`，而中間那個
    /// `Some(Booting)` 兩題的答案相反——那正是這一整支函式最容易錯的地方。
    /// 兩個布林在呼叫端拼裝就是下一次拼錯，所以收原始的那一顆，兩個答案都在
    /// 這裡算完（[`CrashAudit::live`] 和 [`CrashAudit::beat`]）。
    ///
    /// 這一支以前只讀 `sessions` 那張表，於是它答的是「活下來的那幾場」。
    /// 兩組數字從此一起回去：計數器（撐得過刪列）和列（帶得出時間）。
    /// 理由整段寫在 [`migration_006`]。
    ///
    /// # 為什麼那個位元收在這裡
    ///
    /// 上一版收在 `crash_verdict(a, occupied, empty)`，而那一層只把它扣進
    /// **一個**數字（`crashed`）。同一格要印出來的另外三個——分母、拆帳用的
    /// `rows_unfinished`、時間 `last_crash`——全都還含著正在錄的那一場。於是
    /// 一台有 recorder 在跑的機器（目標平台上最常見的狀態）拿到的是：
    ///
    /// ```text
    /// ✗ 零當機   3 段錄製裡有 1 段沒有回來（最後一次 2026-08-20 02:22）
    ///            ——當機、關機、拔電。現在正在錄的那一場沒有算進去
    /// ? 上一次錄製 2026-08-20 02:22 開始，沒有收尾——她現在還在跑
    /// ```
    ///
    /// 同一行先說「那一場沒有算進去」，然後把**那一場的開始時間**當成當機時
    /// 間報出來；下一行還親口確認那個時間是「她現在還在跑」。真的那次當機在
    /// 三天前。**一個新位元只餵給四個要一起印出來的數字裡的一個**，就是這一
    /// 批 bug 的第十四次。
    ///
    /// 所以扣除寫在這裡，一次扣完，回去的每一個欄位都已經不含她。呼叫端沒有
    /// 東西可以拼錯——它連那個布林都拿不到。
    pub fn crash_audit(&self, beat: Option<crate::heartbeat::Phase>) -> Result<CrashAudit> {
        // **她的那一列在不在**，決定她佔不佔一個位置。心跳寫了、而
        // `sessions` 那一列還沒 INSERT 的那幾百毫秒裡，她在下面一個數字都不
        // 佔——那時候扣掉她，扣掉的會是別人的一次當機。
        //
        // 認得出她的是 `id = MAX(id)`：正在錄的那一場永遠是最新的一列
        // （`delete_empty_sessions` 放過 `MAX(id)` 也是為了同一件事）。
        //
        // **只有這一條還不夠，而上一版以為夠了。** `ended_at IS NULL` 的最新
        // 那一列有兩種：她的，和**上一場當掉留下來的殼**。後者在開機那一段時
        // 間裡就坐在 `MAX(id)` 上——`BootBeat::start` 先寫心跳，`Db::open` 和
        // 開機那次 `prune` 才跑（一顆存了一年的資料庫上要好幾分鐘），
        // `start_session` 最後才 INSERT。於是那幾分鐘裡「有人佔著」是真的、
        // 「最新那一列沒收尾」也是真的，而它們指的不是同一場：那道扣除會把
        // **上一次當機**扣掉，然後在下一行說那次當機「現在還在跑」。
        //
        // 分得出來的是心跳的 phase，不是 `is_occupied`：`Phase::Recording` 是
        // 主迴圈寫的，而 `start_session` 在主迴圈之前，所以那一拍在的時候她的
        // 列一定已經進去了。`Phase::Booting` 就是上面那幾分鐘。
        let newest_open: bool = self
            .conn
            .query_row(
                "SELECT ended_at IS NULL FROM sessions ORDER BY id DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(false);
        let live = beat == Some(crate::heartbeat::Phase::Recording) && newest_open;
        // 一句 SQL 把她從三個數字裡一起扣掉——分開扣就是分開扣錯。
        let (rows, rows_unfinished, last_crash) = self.conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(ended_at IS NULL), 0), MAX(CASE WHEN ended_at IS NULL THEN started_at END)
             FROM sessions
             WHERE NOT (?1 AND id = (SELECT MAX(id) FROM sessions))",
            [live],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        // 計數器是 schema 6 才有的。`migrate` 跑完才拿得到 `Db`，所以走到這裡
        // 一定有——除非哪天有人在 `migrate` 之前呼叫它。`COALESCE` 讓那種情況
        // 讀到 0，而 0 配上還在的列會讓 `traceless()` 變負數然後被夾成 0，
        // 也就是「退回上一版的行為」，不是一個假的當機。
        let counter = |key: &str| -> Result<i64> {
            Ok(self.conn.query_row(
                "SELECT COALESCE((SELECT CAST(value AS INTEGER) FROM meta WHERE key = ?1), 0)",
                [key],
                |r| r.get(0),
            )?)
        };
        Ok(CrashAudit {
            // 她開起來的時候觸發器就加過 1 了，所以這裡要扣。`ended` 不用扣
            // ——她還沒收尾。
            started: counter("sessions_started")? - i64::from(live),
            ended: counter("sessions_ended")?,
            rows,
            rows_unfinished,
            last_crash,
            live,
            beat,
            floor: self.conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM meta WHERE key = 'session_counts_floor')",
                [],
                |r| r.get(0),
            )?,
        })
    }

    /// 最後一場錄製：什麼時候開始、有沒有好好結束、以及**為什麼**結束。
    ///
    /// 「現在沒有人在錄」是心跳回答的問題（[`crate::heartbeat`]），而那一句
    /// 話後面永遠跟著同一個問題：那她是什麼時候停的、為什麼？以前答不出來
    /// ——`session_end` 的 `detail` 一律是 `None`，所以「你按了停止」和
    /// 「她當掉了」在磁碟上長得一模一樣，而這兩件事的下一步差很多。
    ///
    /// `ended_at` 是 `None` 就是**沒有好好結束**：不是當掉，就是另一個終端機
    /// 現在正在錄（[`Db::crash_audit`] 上的同一個歧義）。這裡一樣不猜，兩個
    /// 欄位都給出去，讓看得到心跳的那一層去分辨。
    pub fn last_session(&self) -> Result<Option<LastSession>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.started_at, s.ended_at, s.app_version,
                    (SELECT e.detail FROM system_events e
                      WHERE e.session_id = s.id AND e.kind = 'session_end'
                      ORDER BY e.id DESC LIMIT 1),
                    -- 那一場還留著幾筆事件。`reason` 是 NULL 的時候，這個
                    -- 數字分得出「那一版沒在記」和「記了、後來被清掉」——
                    -- 見 `LastSession::events_left`。
                    (SELECT COUNT(*) FROM system_events e WHERE e.session_id = s.id)
             FROM sessions s ORDER BY s.id DESC LIMIT 1",
        )?;
        let row = stmt
            .query_row([], |r| {
                Ok(LastSession {
                    started_at: r.get(0)?,
                    ended_at: r.get(1)?,
                    app_version: r.get(2)?,
                    reason: r.get(3)?,
                    events_left: r.get(4)?,
                })
            })
            .optional()?;
        Ok(row)
    }

    /// 那三張「只寫不讀」的表，裡面到底有沒有東西。
    ///
    /// `ocr_blocks` 的座標、`focus_events`、`input_metrics` 整整三張表，
    /// 除了 `stats` 的 `COUNT(*)` 之外，這份 codebase 沒有任何一行 SELECT
    /// 讀過它們。它們是 Phase 1 之後才會被用到的原料，**存著本身就是對的**
    /// ——「暴力要暴在保存」講的就是這件事，不該為了「有人讀」而硬加讀者。
    ///
    /// 危險的不是沒人讀，是**沒人讀所以沒人驗**。alpha.1 的教訓正是這個形狀：
    /// `doctor` 說 OCR 引擎好好的、錄了一分鐘、摘要一切正常，而資料庫裡一個
    /// 字都沒有。`COUNT(*)` 擋得住「整張表是空的」，擋不住「有一堆列、每一列
    /// 都是空殼」。
    ///
    /// 所以這裡問的不是數量，是**內部一致性**：三個判斷各自都是「這個狀態
    /// 自相矛盾」，而不是「這個數字看起來很小」。使用者只是安靜地坐著不動，
    /// 也會讓數字很小——那種情況下報警，就是我在 `check-audit.py` 剛修掉的
    /// 那個錯誤的鏡像。
    ///
    /// ## 為什麼只看最後一場
    ///
    /// 這三句 SQL 本來是掃全表的，於是它們答的是「這台機器**曾經**好過嗎」。
    /// 而 doctor 問的是現在式。一顆跑過三個月的資料庫裡，`named`、`active`、
    /// `distinct` 全都遠大於 0，所以那三個 `broken` **永遠是 false**——
    /// UIA 上禮拜二被一次 Windows 更新弄壞的機器，這裡照樣印三個 ✓，而
    /// 這裡是唯一會講這件事的地方。一條翻不成 ✗ 的檢查讀起來像涵蓋，
    /// 實際上是一格空白。
    ///
    /// 換成「最後一場」而不是「最近 N 小時」，是因為那是他心裡的單位：他跑
    /// `sister record`、停掉、跑 `sister doctor`，問的就是剛才那一場。而且它
    /// 不需要挑一個魔術數字。代價是一場長達一整天的錄製，中途壞掉的話這裡
    /// 仍然看得到前半段的好資料——下一場就會抓到。
    ///
    /// 沒有任何一場：三個都是 0 列，`scope_started_at` 是 `None`。那不是壞掉。
    ///
    /// 但也**不一定**是「還沒開始」——`sessions` 現在會跟著它記下來的東西一起
    /// 消失（[`crate::retention`] 的 `delete_empty_sessions`），所以一顆被
    /// `sister forget` 清空的資料庫走到的是同一個 `None`。這裡分不出來，也不
    /// 該分：這一支只負責數最後一場的列數。要分的是印字的那一邊，它手上有
    /// [`Db::ever_recorded`]（`doctor` 就是這樣接的）。
    ///
    /// ## 為什麼收 `beat`
    ///
    /// 因為「最後一場」有兩種，而印出來的字不一樣：她已經停了（那是**上一
    /// 場**），或者她此刻正在錄（那是**這一場**）。差別不是修辭——同一份報告
    /// 上面那一列（[`Db::crash_audit`]）把正在錄的那一場從分母裡扣掉了，所以
    /// 「2 段錄製」和底下三行的「上一場」指的是兩個不相交的集合。
    ///
    /// 和 `crash_audit` 一樣，那個位元要在**產生數字的地方**算完：一個新位元
    /// 只餵給一起印出來的其中一個數字，就會生出 N 個描述 N 個不同集合的數字。
    pub fn signal_audit(&self, beat: Option<crate::heartbeat::Phase>) -> Result<Vec<SignalAudit>> {
        /// 全是空殼的列要有這麼多，才算證據而不是巧合。
        ///
        /// 縮到一場之後就需要這個下限：`sister record` 開起來的頭三秒，
        /// 一列 `app_id` 是 NULL 的焦點事件完全可能只是第一次讀還沒成功。
        /// 舊版靠的是全表的量體，換成一場就把那個保護一起丟了。
        ///
        /// 三個訊號共用一個數字，因為它們的問題形狀一樣：**連續十列都是
        /// 空的**。換了十次視窗一次都不知道是哪個 app、十個窗口一個動作都
        /// 沒有卻照樣寫了列、十個文字方框疊在同一個高度——沒有一種是巧合
        /// 做得出來的。門檻壓低的理由和 `BlindSpots::ocr_is_dead` 一樣：
        /// 假警報的代價是他多看一眼，該喊沒喊的代價是他一直不知道。
        const ENOUGH_TO_BE_SURE: i64 = 10;

        // 最後一場。`sessions` 的 id 是遞增的，所以最大的那個就是最新的
        // ——不用 `started_at`，那一欄是系統時鐘，改過時間就會亂排。
        let session: Option<(i64, Millis, bool)> = self
            .conn
            .query_row(
                "SELECT id, started_at, ended_at IS NULL FROM sessions ORDER BY id DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let (id, started_at, open) = match session {
            Some((id, at, open)) => (id, Some(at), open),
            // 一場都沒有。三個訊號一律 0 列——底下每一句 SQL 都會這樣回答，
            // 但先寫出來比較清楚：這裡的 0 是「還沒開始」，不是「壞了」。
            None => (i64::MIN, None, false),
        };
        // 和 `crash_audit` 的 `live` 逐字同一條：心跳說**在錄**（不是在開機），
        // 而且最新那一列還沒收尾。少了任何一半，開機那幾分鐘裡上一次當機留下
        // 來的殼就會被說成「這一場，還在錄」。
        let scope_is_live = beat == Some(crate::heartbeat::Phase::Recording) && open;
        let mut out = Vec::new();
        // 「這個訊號是活的」由每個訊號自己答（`alive`），不在這裡統一套一條
        // 規則：三個 `populated` 數的不是同一種東西（兩個是列數，一個是**幾個
        // 不同的高度**，全壞的樣子是 1 而不是 0）。把三個數字塞進同一個判斷式，
        // 正是 `populated_label` 那個欄位當初存在的理由——那次是印的時候安錯
        // 名字，這次會是判斷的時候。
        //
        // 下限只有一個，而且只在這裡套：`alive` 是「看到證據了」，`rows` 夠不夠
        // 是「找過了沒」，兩件事分開問才有第三種答案（`TooEarly`）。
        let mut push = |name, rows: i64, populated, populated_label, alive: bool, note| {
            out.push(SignalAudit {
                name,
                rows,
                populated,
                populated_label,
                verdict: match (alive, rows >= ENOUGH_TO_BE_SURE) {
                    (true, _) => SignalVerdict::Alive,
                    (false, true) => SignalVerdict::Broken,
                    (false, false) => SignalVerdict::TooEarly,
                },
                note,
                scope_started_at: started_at,
                scope_is_live,
            });
        };

        // 焦點事件：有列、卻沒有任何一列知道那是哪個 app。
        // 一段「不知道是哪個程式」的焦點事件不含任何資訊，那不是安靜，是壞了。
        let (focus, named): (i64, i64) = self.conn.query_row(
            "SELECT COUNT(*), COUNT(app_id) FROM focus_events WHERE session_id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        push(
            "視窗焦點",
            focus,
            named,
            "列知道自己是哪個 app",
            named > 0,
            "每段焦點事件應該知道自己是哪個 app",
        );

        // 打字節奏：有列、卻每一個計數器都是 0。
        // 擷取端在「這個窗口什麼都沒發生」時**根本不會寫列**（見
        // `windows/input.rs` 的早退），所以一列全 0 代表那道閘門壞了，
        // 不代表使用者沒動。
        let (input, active): (i64, i64) = self.conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(keystrokes + clicks + mouse_px + scroll_ticks > 0), 0)
             FROM input_metrics WHERE session_id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        push(
            "輸入節奏",
            input,
            active,
            "列真的有動作",
            active > 0,
            "沒有動靜的窗口本來就不該寫列，所以全 0 的列是壞的不是閒的",
        );

        // 文字方框的座標：有列、卻全部疊在同一個位置。
        // 這是「以後要在畫面上把那行字圈起來」唯一的依據，而它壞掉的樣子
        // 是所有字都在 (0,0)——搜尋照樣全中，沒有任何地方會報錯。
        //
        // `ocr_blocks` 自己沒有 `session_id`，得繞 `frames` 一手。
        let (blocks, distinct): (i64, i64) = self.conn.query_row(
            "SELECT COUNT(*), COUNT(DISTINCT o.y) FROM ocr_blocks o \
             JOIN frames f ON f.id = o.frame_id WHERE f.session_id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        // 這一個活著的樣子是 `distinct > 1`，不是 `> 0`：全部疊在 (0,0)
        // 也還是**一個**高度。門檻共用，「活著長什麼樣」不共用。
        push(
            "文字座標",
            blocks,
            distinct,
            "個不同的高度",
            distinct > 1,
            "一整張畫面的字不會全部在同一個高度",
        );

        Ok(out)
    }

    // ---------- 檢索 ----------

    /// 全文檢索。trigram 與 unicode61 兩個索引各查一次再合併取最佳分數，
    /// 短查詢或兩者皆無命中時再補一次 LIKE 掃描。
    ///
    /// 為什麼需要第三條路：trigram 至少要 3 個字才成立，而 unicode61 會把
    /// 一整串 CJK 當成單一 token。中文最常見的查詢正好是兩個字（「客服」「帳單」
    /// 「電話」），剛好掉進兩個索引之間的縫。LIKE 掃描慢但語意完全正確，
    /// 且只在必要時才跑——正確性優先於漂亮的架構。
    ///
    /// 延遲預算 < 100ms（SPEC §8.2）——這是 Sister 1.0 唯一的效能硬指標。
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        let q = fts_query(query);
        if q.is_empty() {
            return Ok(Vec::new());
        }

        let mut hits: Vec<SearchHit> = Vec::new();
        let mut seen: std::collections::HashMap<i64, usize> = std::collections::HashMap::new();

        for table in ["text_fts", "text_fts_uni"] {
            let sql = format!(
                "SELECT c.id, c.ts, c.source_kind, c.frame_id, c.app_id, c.window_title, c.url,
                        c.text, snippet({table}, 0, '[', ']', '…', 12), bm25({table})
                 FROM {table} JOIN text_chunks c ON c.id = {table}.rowid
                 WHERE {table} MATCH ?1
                 ORDER BY bm25({table}), c.ts DESC, c.id ASC
                 LIMIT ?2"
            );

            let mut stmt = match self.conn.prepare(&sql) {
                Ok(s) => s,
                Err(_) => continue, // 該索引不可用就跳過，另一個仍能作答
            };

            let rows = stmt.query_map(params![q, limit as i64], |row| {
                let kind: String = row.get(2)?;
                Ok(SearchHit {
                    chunk_id: row.get(0)?,
                    ts: row.get(1)?,
                    source_kind: SourceKind::from_str_kind(&kind).unwrap_or(SourceKind::Ocr),
                    frame_id: row.get(3)?,
                    app_id: row.get(4)?,
                    window_title: row.get(5)?,
                    url: row.get(6)?,
                    text: row.get(7)?,
                    snippet: row.get(8)?,
                    // bm25 越小越好，取負值讓「分數高 = 更相關」
                    score: -row.get::<_, f64>(9)?,
                })
            });

            let rows = match rows {
                Ok(r) => r,
                Err(_) => continue,
            };

            for hit in rows.flatten() {
                match seen.get(&hit.chunk_id) {
                    Some(&idx) => {
                        if hit.score > hits[idx].score {
                            hits[idx] = hit;
                        }
                    }
                    None => {
                        seen.insert(hit.chunk_id, hits.len());
                        hits.push(hit);
                    }
                }
            }
        }

        // 兩個索引都空手而回 → 才走 LIKE 掃描。
        //
        // 這個條件原本是「任一個詞短於 3 字 **或** 兩個索引都空手」。那個
        // `short_term ||` 讓一次全表掃描變成日常：查「客服」時 FTS 0.1 ms
        // 就交出了答案，然後照樣掃完整張表——在一個月份量的資料庫上
        // （2,073,600 行字）量到 **102.4 ms**。而 Phase 0 的退場條件寫的就是
        // `sister query 電話`，兩個字。
        //
        // 它是多餘的，不只是貴：`fts_query` 把詞用 AND 串起來，所以只要有
        // 任何一個詞索引比不到（`80` 藏在 `0800` 裡就是這種），整個 MATCH
        // 就是空的，`hits.is_empty()` 自己會成立。`short_term` 唯一多做的事，
        // 是在索引已經答得出來的時候也去掃一次。
        //
        // （中間試過 `hits.len() < limit`，更糟：查得到但結果少於一頁的查詢
        // ——也就是「找一件很少見的事」，這個產品的主要用途——通通變成全表
        // 掃描。量到 103.8 ms。改條件之前先量，不然只是把成本搬個位置。）
        // 兩個字的中文詞掉在 trigram 與 unicode61 中間那個縫裡。先問 bigram
        // 索引（見 [`cjk_bigrams`]），問得到就不必掃全表——退場條件那句
        // `sister query 電話` 走的正是這條路。
        let mut bigram_saw_everything = false;
        if hits.is_empty()
            && let Some(bq) = bigram_query(query)
        {
            let (found, exhausted) = self.search_bigram(&bq, query, limit)?;
            bigram_saw_everything = exhausted;
            for hit in found {
                if let std::collections::hash_map::Entry::Vacant(e) = seen.entry(hit.chunk_id) {
                    e.insert(hits.len());
                    hits.push(hit);
                }
            }
        }

        // 索引全都答不出來才掃全表。這條路仍然留著：bigram 只蓋 CJK，
        // 而且只蓋長度 ≥2 的詞。
        //
        // 但 bigram **把整個候選集看完了**還是空的，那就是真的沒有——回填是
        // 在同一個 transaction 裡做完的（見 `migrate`），所以索引不會落後於
        // 資料。這時再掃一次全表只是把「查無此資料」這個答案賣得比較貴。
        if hits.is_empty() && !bigram_saw_everything {
            for hit in self.search_like(query, limit)? {
                if let std::collections::hash_map::Entry::Vacant(e) = seen.entry(hit.chunk_id) {
                    e.insert(hits.len());
                    hits.push(hit);
                }
            }
        }

        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(b.ts.cmp(&a.ts))
                // 同分、同毫秒很常見（一張畫面可以同時長出 title 與 OCR）。少了
                // 最後這格，SQLite 先吐誰就決定 @k，重跑同一 corpus 會漂。
                .then(a.chunk_id.cmp(&b.chunk_id))
        });
        hits.truncate(limit);
        Ok(hits)
    }

    /// 她最後看到的那幾件事，新的在前。
    ///
    /// 「剛剛發生什麼事」問的是時間，不是字（見 [`crate::question`]）。這一支
    /// 就是那種問題的答案：不比對任何東西，只是把最新的幾列拿出來。
    ///
    /// **刻意不設時間下限。** 上一次錄是三天前的話，答案就是三天前那幾件事，
    /// 而每一列都掛著時間戳——他看得出來那不是「剛剛」。反過來若砍在「一小時
    /// 內」，同一台機器會回一片空白，而空白讀起來是「她什麼都沒記到」。這和
    /// [`Self::search_like`] 把界線壓在資料庫最新一列、而不是 `now()` 上，是
    /// 同一個決定。
    ///
    /// 連著重複的字要收起來：畫面不動的時候同一句話會被寫進好幾列，照抄的話
    /// 「最近十件事」會變成同一句話講十遍。
    pub fn recent(&self, limit: usize) -> Result<Vec<SearchHit>> {
        const OVERFETCH: usize = 12;
        let mut stmt = self.conn.prepare(
            "SELECT id, ts, source_kind, frame_id, app_id, window_title, url, text
             FROM text_chunks ORDER BY ts DESC, id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(
            params![(limit.saturating_mul(OVERFETCH).max(OVERFETCH)) as i64],
            |row| {
                let kind: String = row.get(2)?;
                let text: String = row.get(7)?;
                Ok(SearchHit {
                    chunk_id: row.get(0)?,
                    ts: row.get(1)?,
                    source_kind: SourceKind::from_str_kind(&kind).unwrap_or(SourceKind::Ocr),
                    frame_id: row.get(3)?,
                    app_id: row.get(4)?,
                    window_title: row.get(5)?,
                    url: row.get(6)?,
                    snippet: text.chars().take(60).collect(),
                    text,
                    // 時間問題沒有相關性可言。分數留 0，排序完全由 ts 決定。
                    score: 0.0,
                })
            },
        )?;

        let mut hits: Vec<SearchHit> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for hit in rows.flatten() {
            if !seen.insert(hit.text.clone()) {
                continue;
            }
            hits.push(hit);
            if hits.len() >= limit {
                break;
            }
        }
        Ok(hits)
    }

    /// 一段日曆時間裡她記下來的字，舊的在前。
    ///
    /// 「我昨天下午在弄什麼」問的是那幾個小時，不是拿「弄」去比對螢幕。
    /// 連著重複的字收起來，理由和 [`Self::recent`] 一樣。
    pub fn chunks_in_range(
        &self,
        from_ts: Millis,
        to_ts: Millis,
        limit: usize,
    ) -> Result<Vec<SearchHit>> {
        if from_ts >= to_ts || limit == 0 {
            return Ok(Vec::new());
        }
        const OVERFETCH: usize = 12;
        let mut stmt = self.conn.prepare(
            "SELECT id, ts, source_kind, frame_id, app_id, window_title, url, text
             FROM text_chunks
             WHERE ts >= ?1 AND ts < ?2
             ORDER BY ts ASC, id ASC
             LIMIT ?3",
        )?;
        let rows = stmt.query_map(
            params![
                from_ts,
                to_ts,
                (limit.saturating_mul(OVERFETCH).max(OVERFETCH)) as i64
            ],
            |row| {
                let kind: String = row.get(2)?;
                let text: String = row.get(7)?;
                Ok(SearchHit {
                    chunk_id: row.get(0)?,
                    ts: row.get(1)?,
                    source_kind: SourceKind::from_str_kind(&kind).unwrap_or(SourceKind::Ocr),
                    frame_id: row.get(3)?,
                    app_id: row.get(4)?,
                    window_title: row.get(5)?,
                    url: row.get(6)?,
                    snippet: text.chars().take(60).collect(),
                    text,
                    score: 0.0,
                })
            },
        )?;

        let mut hits: Vec<SearchHit> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for hit in rows.flatten() {
            if !seen.insert(hit.text.clone()) {
                continue;
            }
            hits.push(hit);
            if hits.len() >= limit {
                break;
            }
        }
        Ok(hits)
    }

    // ---------- 題庫 ----------

    /// 記下一次提問。回傳那一列的 id——點擊要靠它掛回來。
    ///
    /// 查不到的那些**照記**。理由見 [`MIGRATION_004`]：找得回來的那些只證明她
    /// 現在能做什麼，找不回來的那些才是下一版要修的東西。
    pub fn log_query(&self, entry: &QueryLogEntry<'_>) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO queries(ts, question, shape, hits, latency_ms, source)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                entry.ts,
                entry.question,
                entry.shape,
                entry.hits as i64,
                entry.latency_ms,
                entry.source
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// 他點開了第 `rank` 筆的出處。
    ///
    /// 這是檢索品質唯一不需要人工標註就拿得到的訊號：點下去的那一刻，等於幫
    /// 那一題標了正解。`rank` 從 0 起算，因為「他點的是第一筆還是第七筆」正是
    /// 排序好不好的直接量測。
    pub fn log_click(&self, query_id: i64, chunk_id: i64, rank: usize) -> Result<()> {
        self.conn.execute(
            "INSERT INTO query_clicks(query_id, chunk_id, rank, ts) VALUES(?1, ?2, ?3, ?4)",
            params![query_id, chunk_id, rank as i64, now_ms()],
        )?;
        Ok(())
    }

    /// 他說「這一題我本來已經忘了」（或者收回那句話）。
    ///
    /// 沒有這一題就是錯誤，不是靜靜地成功。`query_id` 是從題庫撈出來的，撈不
    /// 到只有兩種可能——他打錯號碼，或者那一題剛被 `forget` 帶走——而兩種都要
    /// 讓他知道。（外鍵擋得住 `INSERT` 那半邊，擋不住 `DELETE`：刪一列不存在
    /// 的東西在 SQL 裡是完全合法的 0 列。所以先問，不靠外鍵。）
    ///
    /// 回傳兩件事，因為**兩個呼叫端要問的不是同一個問題**。上一版只回一個
    /// `marked`——而那個 `marked` 就是傳進來的參數原封不動送回去，於是「真的
    /// 收回了一個標記」和「這一題本來就沒標」印出一模一樣的
    /// 「○ 收回了，這一題不再算在裡面」。打錯一個號碼的人會看到成功，而他那
    /// 個標記還掛在別的題上、還算在退場條件裡。`EXISTS` 那一關只擋掉「號碼不
    /// 存在」，擋不掉「號碼存在但本來就沒標」——而 `sister queries` 現在就把
    /// `#N` 印在旁邊，後者才是比較常打錯的那一種。
    pub fn mark_query(&self, query_id: i64, marked: bool) -> Result<MarkOutcome> {
        let exists: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM queries WHERE id = ?1)",
            params![query_id],
            |r| r.get(0),
        )?;
        anyhow::ensure!(exists, "題庫裡沒有第 {query_id} 題");
        let touched = if marked {
            // `OR IGNORE`：重按不換時間。標記的時間是他**第一次**認出來那一
            // 刻，而那正是這一格的全部內容。所以「插進去了」正好等於「這是第
            // 一次」——重按會回 0 列。
            let n = self.conn.execute(
                "INSERT OR IGNORE INTO query_marks(query_id, ts) VALUES(?1, ?2)",
                params![query_id, now_ms()],
            )?;
            // **他按過沒有，和現在還剩幾個，是兩件事。** 標記掛在 `queries` 底
            // 下，`forget` 和保留期都會連著帶走——於是一個按過三次、然後把那段
            // 時間忘掉的人，會拿到一句「還沒有標記過任何一次，去按吧」。和
            // `ever_recorded` / `ever_stored` 同一種旗標，也同一個理由。
            //
            // 這個旗標不需要回填也不會騙人：標記這個功能還沒有發過任何一版，
            // 所以世界上不存在「有標記但沒有旗標」的資料庫。
            self.conn.execute(
                "INSERT OR REPLACE INTO meta(key, value) VALUES('ever_marked', '1')",
                [],
            )?;
            n
        } else {
            self.conn.execute(
                "DELETE FROM query_marks WHERE query_id = ?1",
                params![query_id],
            )?
        };
        Ok(MarkOutcome {
            marked,
            changed: touched > 0,
        })
    }

    /// 第 `id` 題。沒有那一題就是 `None`。
    ///
    /// 和 [`Db::query_log`] 共用同一句 `SELECT`，因為兩邊回的是同一個東西——
    /// 各寫一次的話，`marked` 這一欄哪天只加在其中一邊，而兩個畫面都會說自己
    /// 對。（這個 repo 已經犯過一次「兩次獨立的查找會指到不同的東西」。）
    pub fn query_by_id(&self, id: i64) -> Result<Option<QueryRow>> {
        let mut stmt = self.conn.prepare(&query_log_sql("q.id = ?1", "LIMIT 1"))?;
        let mut rows = stmt.query_map(params![id], read_query_row)?;
        Ok(rows.next().transpose()?)
    }

    /// 最後問過的那一題的編號。沒問過就是 `None`。
    ///
    /// `sister mark` 預設標的是它：他剛剛才看到那個答案，而「剛剛那一題」是他
    /// 唯一不用去查號碼就講得出來的東西。排序和 [`Db::query_log`] 同一組
    /// （`ts DESC, id DESC`）——同一秒問了兩題的時候，兩邊要指到同一列，不然
    /// 「清單上第一列」和「`mark` 標到的那一列」會是不同的東西。
    pub fn last_query(&self) -> Result<Option<QueryRow>> {
        Ok(self.query_log(1)?.into_iter().next())
    }

    /// 最近問過的幾題，新的在前。
    pub fn query_log(&self, limit: usize) -> Result<Vec<QueryRow>> {
        let mut stmt = self.conn.prepare(&query_log_sql(
            "1",
            "ORDER BY q.ts DESC, q.id DESC LIMIT ?1",
        ))?;
        let rows = stmt.query_map(params![limit as i64], read_query_row)?;
        Ok(rows.flatten().collect())
    }

    /// 固定時間窗裡的完整 query log，給 replay 題庫匯出使用。
    ///
    /// 和 L0 replay 一樣採 `[from, to)`；舊的在前，讓 portable `query-0001` 不帶
    /// SQLite row id 也能穩定重建。這裡刻意不設 limit：靜靜截斷會把一份部分
    /// 題庫寫成看起來完整的檔案。
    pub fn query_log_between(&self, from: Millis, to: Millis) -> Result<Vec<QueryRow>> {
        anyhow::ensure!(to >= from, "query log 結束時間早於開始時間");
        let mut stmt = self.conn.prepare(&query_log_sql(
            "q.ts >= ?1 AND q.ts < ?2",
            "ORDER BY q.ts ASC, q.id ASC",
        ))?;
        let rows = stmt.query_map(params![from, to], read_query_row)?;
        Ok(rows.flatten().collect())
    }

    /// 他標記過的那幾題，**他問的時候**新的在前——帶著實例本身。
    ///
    /// 退場條件寫的是「≥ 3 次（記錄實例）」，所以光有一個計數是不夠的：那句
    /// 「記錄實例」要求得出示那幾題長什麼樣。這裡不套 7 天的窗，也不判斷條件
    /// 過了沒——把有時間的實例攤出來，那一格由人去讀。一個會自己宣布「退場條件
    /// ✓」的工具，只是把印象換成一個看起來像數字的印象。
    ///
    /// 排序照 `q.ts`（她答對的時候）而不是 `m.ts`（他按下去的時候），因為
    /// **`limit` 砍掉的必須是離那七天最遠的那幾列**。退場條件數的是使用的七
    /// 天（見 [`QueryRow::marked_ts`] 上面那段），照按下去的時間排的話，一個
    /// 禮拜後才回頭補標的舊題會排到最前面，把真正落在那七天裡的實例擠掉。
    /// 印出來的第一欄也是 `q.ts`——照 A 排序卻把 B 印在前面，讀起來會像沒排過。
    ///
    /// 和 [`Db::query_log`] 共用同一句 `SELECT`（[`query_log_sql`]），所以清單
    /// 上的每一列就是題庫裡的那一列，不是另外拼出來的一個相似的東西。
    pub fn marked_queries(&self, limit: usize) -> Result<Vec<QueryRow>> {
        let mut stmt = self.conn.prepare(&query_log_sql(
            MARKED_PREDICATE,
            "ORDER BY q.ts DESC, q.id DESC LIMIT ?1",
        ))?;
        let rows = stmt.query_map(params![limit as i64], read_query_row)?;
        Ok(rows.flatten().collect())
    }

    /// 題庫現在累積到哪裡了。
    ///
    /// `empty` 和 `clicked` 是這裡真正該看的兩個數字：前者是她答不出來的比例，
    /// 後者是答出來而且**真的有用**的比例。總數只說明他用了多少次。
    pub fn query_log_stats(&self) -> Result<QueryLogStats> {
        // 「幾次魔法時刻」數的是**清單上會出現的那幾題**，所以它和
        // [`Db::marked_queries`] 共用 [`MARKED_PREDICATE`]，不是自己再數一次
        // `query_marks`。理由寫在那個常數上面：兩份數法就是兩個會對不上的答案。
        let sql = format!(
            "SELECT COUNT(*), COALESCE(SUM(hits = 0), 0),
                    (SELECT COUNT(DISTINCT query_id) FROM query_clicks),
                    MIN(ts), MAX(ts),
                    COALESCE(SUM(latency_ms > ?1), 0),
                    COALESCE(SUM(source = ?2), 0),
                    COALESCE(SUM({MARKED_PREDICATE}), 0)
             FROM queries q"
        );
        let mut stats =
            self.conn
                .query_row(&sql, params![RETRIEVAL_BUDGET_MS, SOURCE_DESKTOP], |r| {
                    Ok(QueryLogStats {
                        total: r.get(0)?,
                        empty: r.get(1)?,
                        clicked: r.get(2)?,
                        first_ts: r.get(3)?,
                        last_ts: r.get(4)?,
                        slow: r.get(5)?,
                        clickable: r.get(6)?,
                        p50_ms: 0,
                        p95_ms: 0,
                        marked: r.get(7)?,
                    })
                })?;
        // 中位數要的是「平常有多快」，p95 要的是「最糟的時候有多糟」。平均值
        // 兩個都答不了：一次 4 秒的 migration 會把一整年的平均拉成一個不曾
        // 發生過的數字。
        //
        // 用 OFFSET 而不是把整欄撈進記憶體。索引在 `ts` 上、不在 `latency_ms`
        // 上，所以這是一次排序——但這條路只在使用者跑 `sister queries` 或
        // `doctor` 時走到，不在按下 Enter 的那條路上。
        if stats.total > 0 {
            stats.p50_ms = self.latency_at(stats.total * 50 / 100)?;
            stats.p95_ms = self.latency_at(stats.total * 95 / 100)?;
        }
        Ok(stats)
    }

    /// 依延遲排序後的第 `nth` 筆（從 0 起算）。
    fn latency_at(&self, nth: i64) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT latency_ms FROM queries ORDER BY latency_ms LIMIT 1 OFFSET ?1",
            params![nth.max(0)],
            |r| r.get(0),
        )?)
    }

    /// bigram 索引查詢。片段自己產生——`text_fts_bi` 裡存的是切好的雙字，
    /// 拿它的 `snippet()` 會回一串「客服 服專 專線」，那不是使用者要看的東西。
    ///
    /// **bigram 只是粗篩，會有偽陽性**，所以拿回來的每一列都要用真正的字串
    /// 再篩一次。兩種偽陽性都是真的會發生的：
    ///   - 三個字以上的詞被拆成重疊的雙字（「客服部」→「客服」AND「服部」），
    ///     於是「客服中心的服部先生」兩個雙字都中，卻沒有「客服部」；
    ///   - [`bigram_query`] 直接丟掉非 CJK 的詞，所以查「客服 hello」時，
    ///     `hello` 這個條件在索引這一層根本不存在。
    ///
    /// 回傳的 `bool` 是「候選集有沒有整個看完」。看完了、篩完還是空的，就代表
    /// 真的沒有這個東西，不必再掃全表——查一個不存在的字串因此從 96.7 ms
    /// 降到 0.1 ms。沒看完就不敢說死，讓 LIKE 掃描去補。
    fn search_bigram(
        &self,
        match_expr: &str,
        query: &str,
        limit: usize,
    ) -> Result<(Vec<SearchHit>, bool)> {
        let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        let first = query.split_whitespace().next().unwrap_or_default();
        // 粗篩會混進偽陽性，所以多要一些候選再篩。
        let candidates = limit.saturating_mul(BIGRAM_OVERFETCH).max(BIGRAM_OVERFETCH);
        let sql = "SELECT c.id, c.ts, c.source_kind, c.frame_id, c.app_id, c.window_title, c.url,
                          c.text, bm25(text_fts_bi)
                   FROM text_fts_bi JOIN text_chunks c ON c.id = text_fts_bi.rowid
                   WHERE text_fts_bi MATCH ?1
                   ORDER BY bm25(text_fts_bi), c.ts DESC, c.id ASC
                   LIMIT ?2";

        let mut stmt = match self.conn.prepare(sql) {
            Ok(s) => s,
            Err(_) => return Ok((Vec::new(), false)),
        };
        let rows = stmt.query_map(params![match_expr, candidates as i64], |row| {
            let kind: String = row.get(2)?;
            let text: String = row.get(7)?;
            Ok(SearchHit {
                chunk_id: row.get(0)?,
                ts: row.get(1)?,
                source_kind: SourceKind::from_str_kind(&kind).unwrap_or(SourceKind::Ocr),
                frame_id: row.get(3)?,
                app_id: row.get(4)?,
                window_title: row.get(5)?,
                url: row.get(6)?,
                snippet: make_snippet(&text, first),
                text,
                score: -row.get::<_, f64>(8)?,
            })
        });
        let Ok(rows) = rows else {
            return Ok((Vec::new(), false));
        };

        let mut produced = 0usize;
        let mut all_readable = true;
        let mut hits = Vec::new();
        for row in rows {
            produced += 1;
            let Ok(hit) = row else {
                // 讀不出來的列無法判斷它到底中不中，那就不能說「看完了」。
                all_readable = false;
                continue;
            };
            let hay = hit.text.to_lowercase();
            if terms.iter().all(|t| hay.contains(t.as_str())) {
                hits.push(hit);
            }
        }
        hits.truncate(limit);
        Ok((hits, all_readable && produced < candidates))
    }

    /// 這一題「沒找到」的時候，她其實**看了多遠**。
    ///
    /// `None` = 三個索引裡有一個蓋得住這一題，而索引沒有時間界線：看完了整顆
    /// 資料庫，「沒有」就是真的沒有。
    ///
    /// `Some(days)` = 這一題只剩 [`Self::search_like`] 的掃描可走（產不出相鄰
    /// 雙字的查詢——最常見的是**一個字**的中文），而掃描為了不讓成本跟著使用
    /// 時間長大，夾在最新一列往回 `days` 天內。
    ///
    /// 存在的理由：`retention.text_days` 預設 365，這裡是 30。用滿一年的人查
    /// 一個字查不到，得到的答案是「她記的每一段裡都沒有這個字」——那句話把
    /// **十二分之一**的資料講成了全部。「我找不到」和「我沒去找」是兩件事，
    /// 而只有這一支分得出來。
    ///
    /// 資料庫本身還不到 `days` 天的時候一樣回 `None`：掃描確實看完了全部，
    /// 這時候再加一句免責聲明只是雜訊。
    pub fn scan_horizon_days(&self, query: &str) -> Result<Option<i64>> {
        if covered_by_index(query) {
            return Ok(None);
        }
        // 分成兩句，不是一句 `SELECT MIN(ts), MAX(ts)`。SQLite 只對**單獨**
        // 一個 min() 或 max() 做那個 O(1) 的索引端點最佳化；兩個放在一起
        // 就退回 `SCAN ... USING COVERING INDEX`，也就是把一整年的 chunk
        // 從頭走到尾。而這一支跑在他按下 Enter 之後那條路上。
        // （`EXPLAIN QUERY PLAN` 當場看得到：SCAN → SEARCH。）
        let end = |sql: &str| -> Result<Option<i64>> {
            Ok(self
                .conn
                .query_row(sql, [], |r| r.get::<_, Option<i64>>(0))
                .optional()?
                .flatten())
        };
        let (Some(oldest), Some(newest)) = (
            end("SELECT MIN(ts) FROM text_chunks")?,
            end("SELECT MAX(ts) FROM text_chunks")?,
        ) else {
            return Ok(None);
        };
        Ok((newest - oldest > LIKE_SCAN_DAYS * 86_400_000).then_some(LIKE_SCAN_DAYS))
    }

    /// 子字串掃描後援。分數固定為 `LIKE_SCORE`，永遠排在 FTS 命中之後。
    fn search_like(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        let terms: Vec<String> = query
            .split_whitespace()
            .map(|t| {
                format!(
                    "%{}%",
                    t.replace('\\', r"\\")
                        .replace('%', r"\%")
                        .replace('_', r"\_")
                )
            })
            .collect();
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        let first = query
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_string();

        let conds = (1..=terms.len())
            .map(|i| format!("text LIKE ?{i} ESCAPE '\\'"))
            .collect::<Vec<_>>()
            .join(" AND ");
        // 界線從**資料庫裡最新的一列**往回算，不是從 `now()`。查一份三年前
        // 封存起來的資料庫時，用現在時間當基準會掃出 0 列，然後長得像「查無
        // 此資料」——那是這個專案最不想要的那種失敗。
        let newest: Option<i64> = self
            .conn
            .query_row("SELECT MAX(ts) FROM text_chunks", [], |r| r.get(0))
            .optional()?
            .flatten();
        let Some(newest) = newest else {
            return Ok(Vec::new());
        };
        let cutoff = newest - LIKE_SCAN_DAYS * 86_400_000;

        let sql = format!(
            "SELECT id, ts, source_kind, frame_id, app_id, window_title, url, text
             FROM text_chunks WHERE ts >= ?{} AND {conds}
             ORDER BY ts DESC, id ASC LIMIT ?{}",
            terms.len() + 1,
            terms.len() + 2
        );

        let mut params: Vec<Box<dyn rusqlite::ToSql>> = terms
            .into_iter()
            .map(|t| Box::new(t) as Box<dyn rusqlite::ToSql>)
            .collect();
        params.push(Box::new(cutoff));
        params.push(Box::new(limit as i64));

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(params.iter()), |row| {
            let kind: String = row.get(2)?;
            let text: String = row.get(7)?;
            Ok(SearchHit {
                chunk_id: row.get(0)?,
                ts: row.get(1)?,
                source_kind: SourceKind::from_str_kind(&kind).unwrap_or(SourceKind::Ocr),
                frame_id: row.get(3)?,
                app_id: row.get(4)?,
                window_title: row.get(5)?,
                url: row.get(6)?,
                snippet: make_snippet(&text, &first),
                text,
                score: LIKE_SCORE,
            })
        })?;
        Ok(rows.flatten().collect())
    }

    /// 隔多久再看到，才算**另一次**看到。
    ///
    /// 沒有這道間隔，「看過 N 次」數的是列數，而一列是一張留下來的畫面——於是
    /// 那個數字量到的其實是**螢幕上其他地方動得多勤**：號碼釘在側邊欄不動，
    /// 旁邊的聊天室每來一則訊息就換一張畫面，那支號碼就多算一次。他坐在那裡
    /// 的那二十分鐘會被講成「看過 300 次」，而他只是看了一次沒關掉。
    ///
    /// 十分鐘：這是「走開一下、切去別的視窗、回來」的長度。比它短的空檔還是
    /// 同一次坐著，比它長的才算他又遇到了一次。和 `exclusion_audit`
    /// 「在 keepassxc 裡待了十分鐘＝一列」同一個單位——那張表也是存段不存張。
    pub const SAME_SITTING_MS: Millis = 10 * 60 * 1000;

    /// 每個**不同的值**一列：最近一次的出處，加上他**遇到過它幾次**。
    ///
    /// 存在的理由是 [`facts_by_kind`](Self::facts_by_kind) 給不出第二個數字。
    /// `answer::answers` 以前的做法是抓最近 40 列回來、在**那 40 列的窗子裡**
    /// 數重複，於是：
    ///
    /// * 一年內看過 200 次的號碼，最多只講得出「看過 40 次」——而畫面上那句
    ///   「看過 N 次」的用途正是「1 次和 12 次是強度不同的答案」。
    /// * 更糟的是某一頁一次吐出 40 個新的電話事實：他媽媽的號碼（一年來每週
    ///   都看到）根本不在窗子裡，★ 清單沒有它，然後 fallback 說「我記得的
    ///   東西裡沒有這件事」——對一個在資料庫裡出現幾百次的值。
    ///
    /// 那次修法把窗子拿掉了，但數的仍然是**列**——而一列是一張留下來的畫面，
    /// 不是他遇到那件事的一次。同一支號碼在同一個下午被數了三百次，和真的
    /// 在三百個不同的日子看到，在畫面上印出來一模一樣。所以現在數的是**段**：
    /// 中間空了 [`SAME_SITTING_MS`](Self::SAME_SITTING_MS) 以上才開新的一段。
    ///
    /// `LIMIT` 在 `GROUP BY` **之後**才切，所以切掉的是「第 11 個不同的答案」，
    /// 不是「第 41 筆目擊」。非聚合欄取自 `MAX(ts)` 那一列，靠的是 SQLite
    /// 對 min/max 的 bare column 特例（3.7.11 起有文件保證）。
    pub fn fact_sightings(&self, kind: &str, limit: usize) -> Result<Vec<(FactRow, i64)>> {
        // **先挑出那 10 個值，再去數它們的段。** 反過來寫（整個 kind 全表跑
        // 窗函數、最後才 `LIMIT 10`）答案一模一樣，而且短很多——但它會替 490
        // 個永遠不會被印出來的值算段，中間那個 `WINDOW` 還得先把整批列照
        // (normalized, ts) 排過一次。20 萬列 phone、500 個不同的值、release
        // build 的開發機上量到的：**242 ms 對 47 ms**（照舊只數列數是 67）。
        // `RETRIEVAL_BUDGET_MS` 是 100，所以短的那一版是不能寫的。
        //
        // 這個數字沒有變成一條測試：一年份的語料要灌十秒，而貼著實測值的
        // 時間門檻在比開發機慢一倍的 CI 上只會變成一條大家想辦法調高的線
        // （同 `search_latency.rs` 的 `CAPPED_CEILING_MS`）。留在這裡是要讓
        // 下一個想把這段 CTE「化簡掉」的人先讀到它。
        //
        // 每一列先問自己「我是不是一段的開頭」——前一次目擊隔了夠久，或者
        // 我就是第一次。`LAG` 回 NULL 的時候減出來也是 NULL，而 `NULL >= x`
        // 是 NULL 不是真，所以第一列非得自己講明不可，不然每個值都少算一次。
        let mut stmt = self.conn.prepare(
            "WITH answered AS (
               SELECT normalized AS value, MAX(ts) AS last_ts
               FROM facts WHERE kind = ?1
               GROUP BY normalized
               ORDER BY last_ts DESC, value ASC LIMIT ?3
             )
             SELECT id, ts, kind, raw, normalized, source_kind,
                    chunk_id, frame_id, app_id, window_title, url,
                    SUM(opens_a_sitting), MAX(ts)
             FROM (
               SELECT f.id, f.ts, f.kind, f.raw, f.normalized, f.source_kind,
                      f.chunk_id, f.frame_id, f.app_id, f.window_title, f.url,
                      CASE WHEN LAG(f.ts) OVER seen IS NULL
                             OR f.ts - LAG(f.ts) OVER seen >= ?2
                           THEN 1 ELSE 0 END AS opens_a_sitting
               FROM facts f JOIN answered ON f.normalized = answered.value
               WHERE f.kind = ?1
               WINDOW seen AS (PARTITION BY f.normalized ORDER BY f.ts)
             )
             GROUP BY normalized
             ORDER BY ts DESC",
        )?;
        let rows = stmt.query_map(params![kind, Self::SAME_SITTING_MS, limit as i64], |row| {
            Ok((map_fact_row(row)?, row.get::<_, i64>(11)?))
        })?;
        Ok(rows.flatten().collect())
    }

    /// 依 typed fact 直查（`sister facts --kind` 走這條）。
    ///
    /// **一列就是一張留下來的畫面**，同一個號碼出現在三張畫面上就是三列——
    /// 而那三張很可能是同一分鐘裡的三拍。要「每個值一列 + 他遇到過幾次」請用
    /// [`fact_sightings`](Self::fact_sightings)，那邊會把同一次坐著看留下的
    /// 那幾百列併成一次。
    pub fn facts_by_kind(&self, kind: &str, limit: usize) -> Result<Vec<FactRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, ts, kind, raw, normalized, source_kind,
                    chunk_id, frame_id, app_id, window_title, url
             FROM facts WHERE kind = ?1 ORDER BY ts DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![kind, limit as i64], map_fact_row)?;
        Ok(rows.flatten().collect())
    }

    /// 在 fact 的原始字串上做子字串搜尋（例如只想找含 "0800" 的電話）。
    pub fn facts_search(
        &self,
        kind: Option<&str>,
        needle: &str,
        limit: usize,
    ) -> Result<Vec<FactRow>> {
        let pattern = format!("%{needle}%");
        let mut stmt = self.conn.prepare(
            "SELECT id, ts, kind, raw, normalized, source_kind,
                    chunk_id, frame_id, app_id, window_title, url
             FROM facts
             WHERE (?1 IS NULL OR kind = ?1) AND (raw LIKE ?2 OR normalized LIKE ?2)
             ORDER BY ts DESC LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![kind, pattern, limit as i64], map_fact_row)?;
        Ok(rows.flatten().collect())
    }

    /// 每一列說自己有圖的那條**相對路徑**（相對於 `frames/`）。
    ///
    /// 給 `sister export` 逐一確認那些檔案真的躺在目的地用的。數量對得起來
    /// 不代表對得上：她一邊錄一邊匯出的時候，`frames/` 會長出比資料庫那個
    /// 快照更多的檔案，於是「複製了 121 個、資料庫說有 120 列」看起來是滿的
    /// ——而那 120 列裡少掉的那一張，被兩張新的蓋過去了。備份最不該有的
    /// 就是這種「看起來滿的」。
    ///
    /// 用 callback 而不是回一個 `Vec`：十萬列的資料庫不必為了數幾個檔案先
    /// 在記憶體裡攤平三 MB 的字串。
    pub fn for_each_image_path(&self, mut f: impl FnMut(&str)) -> Result<()> {
        let mut stmt = self
            .conn
            .prepare("SELECT image_path FROM frames WHERE image_path IS NOT NULL")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let rel: String = row.get(0)?;
            f(&rel);
        }
        Ok(())
    }

    /// 這幾個 frame 裡，**真的有圖可以打開**的是哪些。
    ///
    /// `text_chunks.frame_id` 講的是「這段字是從哪一幀抄下來的」，那是出處，
    /// 一直都有。它回答不了「那一幀有沒有留下照片」——而畫面上那個「點開看
    /// 當時的畫面」問的是後者。
    ///
    /// 兩者以前被當成同一件事，於是：
    ///
    /// - 只簽第一張同意書（只記字、不留圖）的時候，`image_path` 全是 NULL，
    ///   但每一個出處都長得可以點。**那是這個模式的每一筆，不是零星幾筆。**
    /// - 截圖節流和每日額度用完時也一樣：字進去了，圖沒存。
    ///
    /// 不改 search 那幾條 SQL：那是有 100ms 預算的熱路徑，而每加一個 JOIN
    /// 就多一次 query plan 的賭注。畫完一份答案才問一次，一次問完全部。
    pub fn frames_with_image(&self, ids: &[i64]) -> Result<std::collections::HashSet<i64>> {
        let mut out = std::collections::HashSet::new();
        // 一次問完是對的，但「一次」有上限：SQLite 綁定參數的天花板是 32766，
        // 而時間軸一天可以送 2000 個 id 進來。切段不是效能調校，是**別在某個
        // 很長的一天整個炸掉**——那天的每個出處都會退回「不能點」。
        for batch in ids.chunks(500) {
            // id 是我們自己資料庫來的整數，不是使用者輸入；照樣用參數綁定，
            // 因為「這次是安全的」這種理由會被下一個人抄走。
            let holes = std::iter::repeat_n("?", batch.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql =
                format!("SELECT id FROM frames WHERE image_path IS NOT NULL AND id IN ({holes})");
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(batch), |row| {
                row.get::<_, i64>(0)
            })?;
            for r in rows {
                out.insert(r?);
            }
        }
        Ok(out)
    }

    /// 一張幀的完整脈絡（點開出處時用）。
    pub fn frame_context(&self, frame_id: i64) -> Result<Option<FrameContext>> {
        self.conn
            .query_row(
                "SELECT id, ts, app_id, window_title, url, image_path, dup_run, width, height
                 FROM frames WHERE id = ?1",
                params![frame_id],
                |row| {
                    Ok(FrameContext {
                        frame_id: row.get(0)?,
                        ts: row.get(1)?,
                        app_id: row.get(2)?,
                        window_title: row.get(3)?,
                        url: row.get(4)?,
                        image_path: row.get(5)?,
                        dup_run: row.get(6)?,
                        width: row.get(7)?,
                        height: row.get(8)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    /// 哪幾天她其實有在看，各記了多少。
    ///
    /// 時間軸的第一個問題永遠是「哪幾天有東西」。沒有這一支，日期選擇器只能
    /// 讓使用者一天一天點過去猜——而她可能整整兩週都沒開機。
    ///
    /// **切天用的是本機時區的偏移量，不是 UTC。**「昨天」對使用者而言是他睡覺
    /// 那條線分開的，不是格林威治的。偏移量由呼叫端傳進來，因為 core 這一層
    /// 刻意不認識「使用者在哪個時區」這件事。
    pub fn days_with_data(&self, tz_offset_ms: i64) -> Result<Vec<DaySummary>> {
        const DAY_MS: i64 = 86_400_000;
        let mut stmt = self.conn.prepare(
            "SELECT (ts + ?1) / ?2 AS day, COUNT(*), MIN(ts), MAX(ts)
             FROM text_chunks GROUP BY day ORDER BY day DESC",
        )?;
        let rows = stmt.query_map(params![tz_offset_ms, DAY_MS], |r| {
            let day: i64 = r.get(0)?;
            Ok(DaySummary {
                // 換算回這一天在**本機**的午夜（epoch 毫秒）。前端拿它直接餵
                // `new Date()` 就會顯示對的日期。
                start_ts: day * DAY_MS - tz_offset_ms,
                chunks: r.get(1)?,
                first_ts: r.get(2)?,
                last_ts: r.get(3)?,
            })
        })?;
        Ok(rows.flatten().collect())
    }

    /// 一段時間裡她看到的東西，依時間排序。
    ///
    /// 主體是 `text_chunks` 而不是 `frames`，這是個刻意的選擇：文字保留 365 天、
    /// 畫面 30 天，所以超過 30 天的那些日子裡 `frames` 是空的，而她**確實**還
    /// 記得那幾天的東西。拿 frames 當主體的時間軸，會讓一個月前的自己看起來
    /// 像不存在。`frame_id` 可以是 `None`，那就是「字還在、圖過期了」。
    pub fn timeline(&self, from_ts: Millis, to_ts: Millis, limit: usize) -> Result<Vec<Moment>> {
        let mut stmt = self.conn.prepare(
            "SELECT ts, app_id, window_title, url, text, frame_id
             FROM text_chunks
             WHERE ts >= ?1 AND ts < ?2
             ORDER BY ts LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![from_ts, to_ts, limit as i64], |r| {
            Ok(Moment {
                ts: r.get(0)?,
                app: r.get(1)?,
                title: r.get(2)?,
                url: r.get(3)?,
                text: r.get(4)?,
                frame_id: r.get(5)?,
            })
        })?;
        Ok(rows.flatten().collect())
    }

    /// 某一段時間裡、斷句需要的 L0 事件。不跑演算法、不寫表。
    pub fn segment_events(
        &self,
        from_ts: Millis,
        to_ts: Millis,
    ) -> Result<crate::segment::EventStream> {
        let mut focus = Vec::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT id, ts, app_id, app_name, window_title, url
                 FROM focus_events WHERE ts >= ?1 AND ts < ?2 ORDER BY ts, id",
            )?;
            let rows = stmt.query_map(params![from_ts, to_ts], |r| {
                Ok(crate::segment::FocusPoint {
                    id: r.get(0)?,
                    ts: r.get(1)?,
                    app_id: r.get(2)?,
                    app_name: r.get(3)?,
                    window_title: r.get(4)?,
                    url: r.get(5)?,
                })
            })?;
            focus.extend(rows.flatten());
        }
        let mut system = Vec::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT id, ts, kind FROM system_events
                 WHERE ts >= ?1 AND ts < ?2 ORDER BY ts, id",
            )?;
            let rows = stmt.query_map(params![from_ts, to_ts], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, Millis>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?;
            for row in rows.flatten() {
                let (id, ts, kind) = row;
                system.push(crate::segment::SystemPoint {
                    id,
                    ts,
                    kind: system_kind_from_db(&kind)?,
                });
            }
        }
        let mut clipboard = Vec::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT id, ts, byte_len, source_app FROM clipboard_events
                 WHERE ts >= ?1 AND ts < ?2 ORDER BY ts, id",
            )?;
            let rows = stmt.query_map(params![from_ts, to_ts], |r| {
                Ok(crate::segment::ClipboardPoint {
                    id: r.get(0)?,
                    ts: r.get(1)?,
                    byte_len: r.get(2)?,
                    source_app: r.get(3)?,
                })
            })?;
            clipboard.extend(rows.flatten());
        }
        let mut input = Vec::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT id, ts_start, ts_end, idle_ms FROM input_metrics
                 WHERE ts_end > ?1 AND ts_start < ?2 ORDER BY ts_start, id",
            )?;
            let rows = stmt.query_map(params![from_ts, to_ts], |r| {
                Ok(crate::segment::InputPoint {
                    id: r.get(0)?,
                    ts_start: r.get(1)?,
                    ts_end: r.get(2)?,
                    idle_ms: r.get(3)?,
                })
            })?;
            input.extend(rows.flatten());
        }
        Ok(crate::segment::EventStream {
            focus,
            system,
            clipboard,
            input,
        })
    }

    /// 打開時間軸時叫的那一支：把 `[from, to)` 的段落算一遍寫進 `segment`。
    ///
    /// 每次都重算，不讀舊列。一天的事件量遠小於 OCR，這不是熱路徑。
    /// 使用者的合併／切開在 `segment_edit`，重算之後套上去，所以不會被吃掉。
    pub fn chapters_for_range(
        &mut self,
        from_ts: Millis,
        to_ts: Millis,
    ) -> Result<Vec<crate::segment::Segment>> {
        if from_ts >= to_ts {
            return Ok(Vec::new());
        }
        let pad = crate::segment::LOOKAROUND_MS;
        let stream = self.segment_events(from_ts.saturating_sub(pad), to_ts.saturating_add(pad))?;
        let raw: Vec<crate::segment::Segment> = crate::segment::segment(&stream)
            .into_iter()
            .filter(|s| s.core_started_at >= from_ts && s.core_started_at < to_ts)
            .collect();
        // 卡住偵測看的是演算法自己切的活動，不看人改過的章節。
        self.replace_stuck(from_ts, to_ts, &raw)?;
        let edits = self.segment_edits_overlapping(from_ts, to_ts)?;
        let kept = crate::segment_edit::apply_edits(raw, &edits);
        self.replace_segments(from_ts, to_ts, &kept)?;
        Ok(kept)
    }

    /// 問句帶得了時間範圍才算章節。走重算，不讀舊列：他如果沒開過那天的
    /// 時間軸，`segment` 表是空的，讀舊列會把「沒算過」印成「沒有章節」。
    ///
    /// 回的是活動級（[`crate::activity::Activity`]），不是分鐘級 `segment`。
    /// 10 分鐘上限切碎的同質段在這裡併回去；`segment` 表仍是切碎的那一版，
    /// 時間軸與 `segment_edit` 繼續對那一層。
    ///
    /// - `None`：這句話沒有時間範圍，**沒去算**。
    /// - `Some((_, []))`：算過了，那段時間切不出段落。
    ///
    /// 兩種不可以合成一個空陣列。一天的事件量遠小於 OCR，問一次重算一次
    /// 不是熱路徑。
    pub fn chapters_for_question(
        &mut self,
        question: &str,
        now: Millis,
    ) -> Result<Option<(crate::question::TimeRange, Vec<crate::activity::Activity>)>> {
        let Some(range) = crate::question::time_range(question, now) else {
            return Ok(None);
        };
        let segments = self.chapters_for_range(range.from, range.to)?;
        Ok(Some((range, crate::activity::group(&segments))))
    }

    /// 時間軸上的一格：活動級。底下的 `segment` 仍是分鐘級，編輯對那一層。
    pub fn activities_for_range(
        &mut self,
        from_ts: Millis,
        to_ts: Millis,
    ) -> Result<Vec<crate::activity::Activity>> {
        let segments = self.chapters_for_range(from_ts, to_ts)?;
        Ok(crate::activity::group(&segments))
    }

    pub fn facts_in_range(&self, from_ts: Millis, to_ts: Millis) -> Result<Vec<FactRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, ts, kind, raw, normalized, source_kind,
                    chunk_id, frame_id, app_id, window_title, url
             FROM facts WHERE ts >= ?1 AND ts < ?2 ORDER BY ts, id",
        )?;
        let rows = stmt.query_map(params![from_ts, to_ts], map_fact_row)?;
        Ok(rows.flatten().collect())
    }

    pub fn clipboard_in_range(
        &self,
        from_ts: Millis,
        to_ts: Millis,
    ) -> Result<Vec<crate::segment::ClipboardPoint>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, ts, byte_len, source_app FROM clipboard_events
             WHERE ts >= ?1 AND ts < ?2 ORDER BY ts, id",
        )?;
        let rows = stmt.query_map(params![from_ts, to_ts], |r| {
            Ok(crate::segment::ClipboardPoint {
                id: r.get(0)?,
                ts: r.get(1)?,
                byte_len: r.get(2)?,
                source_app: r.get(3)?,
            })
        })?;
        Ok(rows.flatten().collect())
    }

    pub fn stuck_in_range(
        &self,
        from_ts: Millis,
        to_ts: Millis,
    ) -> Result<Vec<crate::stuck::StuckSignal>> {
        let mut stmt = self.conn.prepare(
            "SELECT started_at, ended_at, app_id, window_title,
                    dwell_ms, switch_count, error_fact_count
             FROM stuck_signal
             WHERE ended_at > ?1 AND started_at < ?2
             ORDER BY started_at, id",
        )?;
        let rows = stmt.query_map(params![from_ts, to_ts], |r| {
            Ok(crate::stuck::StuckSignal {
                started_at: r.get(0)?,
                ended_at: r.get(1)?,
                app: r.get(2)?,
                title: r.get(3)?,
                dwell_ms: r.get(4)?,
                switch_count: r.get(5)?,
                error_fact_count: r.get(6)?,
            })
        })?;
        Ok(rows.flatten().collect())
    }

    /// 這段時間裡有沒有任何 L0 原件。沒有就算過，不是回 0 筆再讓呼叫端猜。
    pub fn has_l0_in_range(&self, from_ts: Millis, to_ts: Millis) -> Result<bool> {
        if from_ts >= to_ts {
            return Ok(false);
        }
        let focus: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM focus_events WHERE ts >= ?1 AND ts < ?2)",
            params![from_ts, to_ts],
            |r| r.get(0),
        )?;
        if focus {
            return Ok(true);
        }
        let frames: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM frames WHERE ts >= ?1 AND ts < ?2)",
            params![from_ts, to_ts],
            |r| r.get(0),
        )?;
        Ok(frames)
    }

    pub fn frame_exists(&self, id: i64) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM frames WHERE id = ?1)",
            [id],
            |r| r.get(0),
        )?)
    }

    pub fn fact_exists(&self, id: i64) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM facts WHERE id = ?1)",
            [id],
            |r| r.get(0),
        )?)
    }

    pub fn latest_l2_for_segment(&self, core_started_at: Millis) -> Result<Option<L2CardRow>> {
        self.conn
            .query_row(
                &format!(
                    "{L2_SELECT}
                 FROM l2_card
                 WHERE segment_core_start = ?1 AND tombstoned_at IS NULL
                 ORDER BY version DESC, id DESC LIMIT 1"
                ),
                [core_started_at],
                map_l2_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn latest_l2_before(&self, core_started_at: Millis) -> Result<Option<L2CardRow>> {
        self.conn
            .query_row(
                &format!(
                    "{L2_SELECT}
                 FROM l2_card
                 WHERE segment_core_start < ?1 AND tombstoned_at IS NULL
                 ORDER BY segment_core_start DESC, version DESC, id DESC LIMIT 1"
                ),
                [core_started_at],
                map_l2_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn l2_in_range(&self, from_ts: Millis, to_ts: Millis) -> Result<Vec<L2CardRow>> {
        let mut stmt = self.conn.prepare(&format!(
            "{L2_SELECT}
             FROM l2_card
             WHERE segment_core_start >= ?1 AND segment_core_start < ?2
               AND tombstoned_at IS NULL
             ORDER BY segment_core_start, version, id"
        ))?;
        let rows = stmt.query_map(params![from_ts, to_ts], map_l2_row)?;
        Ok(rows.flatten().collect())
    }

    /// 某一段上還活著的每一版，舊的在前。原版留著，後來改的也看得到。
    pub fn l2_versions_for_segment(&self, core_started_at: Millis) -> Result<Vec<L2CardRow>> {
        let mut stmt = self.conn.prepare(&format!(
            "{L2_SELECT}
             FROM l2_card
             WHERE segment_core_start = ?1 AND tombstoned_at IS NULL
             ORDER BY version, id"
        ))?;
        let rows = stmt.query_map(params![core_started_at], map_l2_row)?;
        Ok(rows.flatten().collect())
    }

    pub fn l2_by_id(&self, id: i64) -> Result<Option<L2CardRow>> {
        self.conn
            .query_row(
                &format!("{L2_SELECT} FROM l2_card WHERE id = ?1"),
                [id],
                map_l2_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn insert_l2_card(&mut self, ins: &L2Insert<'_>) -> Result<i64> {
        let prev = self.latest_l2_for_segment(ins.segment_core_start)?;
        if let Some(p) = &prev {
            anyhow::ensure!(
                p.author != L2Author::User || ins.author == L2Author::User,
                "使用者改過的假設不會被下一輪蓋掉"
            );
        }
        let version = prev.as_ref().map(|p| p.version + 1).unwrap_or(1);
        let supersedes = prev.as_ref().map(|p| p.id);
        let confidence_source = ins.author.confidence_source();
        self.conn.execute(
            "INSERT INTO l2_card(
                segment_core_start, segment_ref, version, supersedes,
                activity, entities_json, continues_json, commitments_json,
                model_confidence, confidence_source, evidence_json,
                open_questions_json, created_at, author, tombstoned_at
             ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,NULL)",
            params![
                ins.segment_core_start,
                ins.segment_ref,
                version,
                supersedes,
                ins.activity,
                ins.entities_json,
                ins.continues_json,
                ins.commitments_json,
                ins.model_confidence,
                confidence_source,
                ins.evidence_json,
                ins.open_questions_json,
                crate::now_ms(),
                ins.author.as_str(),
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        let child = format!("l2:{id}");
        self.insert_provenance(&child, &format!("segment:{}", ins.segment_core_start))?;
        if let Ok(refs) = serde_json::from_str::<Vec<String>>(&ins.evidence_json) {
            for r in refs {
                if crate::brain::EvidenceRef::parse(&r).is_some() {
                    self.insert_provenance(&child, &r)?;
                }
            }
        }
        Ok(id)
    }

    pub fn brain_outbound_count_on(&self, day_key: &str) -> Result<u32> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM brain_outbound WHERE day_key = ?1",
            [day_key],
            |r| r.get(0),
        )?;
        Ok(n.max(0) as u32)
    }

    pub fn insert_brain_outbound(&mut self, ins: &OutboundInsert<'_>) -> Result<i64> {
        let args_json = serde_json::to_string(ins.args).context("serialize brain args")?;
        self.conn.execute(
            "INSERT INTO brain_outbound(
                ts, day_key, command, args_json, segment_core_start,
                chars_sent, truncated, outcome, duration_ms, error, role
             ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![
                ins.ts,
                ins.day_key,
                ins.command,
                args_json,
                ins.segment_core_start,
                ins.chars_sent,
                ins.truncated as i64,
                ins.outcome,
                ins.duration_ms,
                ins.error,
                ins.role,
            ],
        )?;
        self.conn.execute(
            "INSERT OR REPLACE INTO meta(key, value) VALUES('ever_brain_outbound', '1')",
            [],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn brain_outbound_count_on_role(&self, day_key: &str, role: &str) -> Result<u32> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM brain_outbound WHERE day_key = ?1 AND role = ?2",
            params![day_key, role],
            |r| r.get(0),
        )?;
        Ok(n.max(0) as u32)
    }

    pub fn list_brain_outbound(&self, limit: usize) -> Result<Vec<OutboundRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, ts, day_key, command, args_json, segment_core_start,
                    chars_sent, truncated, outcome, duration_ms, error, role
             FROM brain_outbound ORDER BY ts DESC, id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], |r| {
            Ok(OutboundRow {
                id: r.get(0)?,
                ts: r.get(1)?,
                day_key: r.get(2)?,
                command: r.get(3)?,
                args_json: r.get(4)?,
                segment_core_start: r.get(5)?,
                chars_sent: r.get(6)?,
                truncated: r.get::<_, i64>(7)? != 0,
                outcome: r.get(8)?,
                duration_ms: r.get(9)?,
                error: r.get(10)?,
                role: r.get(11)?,
            })
        })?;
        Ok(rows.flatten().collect())
    }

    pub fn list_brain_skip(&self, limit: usize) -> Result<Vec<SkipRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, ts, reason, segment_core_start, detail
             FROM brain_skip ORDER BY ts DESC, id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], |r| {
            Ok(SkipRow {
                id: r.get(0)?,
                ts: r.get(1)?,
                reason: r.get(2)?,
                segment_core_start: r.get(3)?,
                detail: r.get(4)?,
            })
        })?;
        Ok(rows.flatten().collect())
    }

    pub fn insert_brain_skip(
        &mut self,
        ts: Millis,
        reason: &str,
        segment_core_start: Option<Millis>,
        detail: &str,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO brain_skip(ts, reason, segment_core_start, detail)
             VALUES(?1,?2,?3,?4)",
            params![ts, reason, segment_core_start, detail],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn insert_provenance(&mut self, child_ref: &str, parent_ref: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO provenance(child_ref, parent_ref) VALUES(?1, ?2)",
            params![child_ref, parent_ref],
        )?;
        Ok(())
    }

    pub fn provenance_children(&self, parent_ref: &str) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT child_ref FROM provenance WHERE parent_ref = ?1")?;
        let rows = stmt.query_map([parent_ref], |r| r.get(0))?;
        Ok(rows.flatten().collect())
    }

    /// 這筆 L0 原件現在還在不在、上面寫了什麼。回查一定走這裡，不走 L2 卡片。
    pub fn l0_original(&self, r: &crate::brain::EvidenceRef) -> Result<Option<L0Original>> {
        match r {
            crate::brain::EvidenceRef::Frame(id) => {
                let exists: bool = self.conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM frames WHERE id = ?1)",
                    [*id],
                    |row| row.get(0),
                )?;
                if !exists {
                    return Ok(None);
                }
                let mut stmt = self.conn.prepare(
                    "SELECT text FROM text_chunks WHERE frame_id = ?1
                     UNION ALL
                     SELECT text FROM ocr_blocks WHERE frame_id = ?1",
                )?;
                let texts: Vec<String> =
                    stmt.query_map([*id], |row| row.get(0))?.flatten().collect();
                let text = texts.join("\n");
                Ok(Some(L0Original {
                    r#ref: r.as_str(),
                    kind: "frame",
                    text,
                }))
            }
            crate::brain::EvidenceRef::Fact(id) => {
                let row = self
                    .conn
                    .query_row("SELECT kind, raw FROM facts WHERE id = ?1", [*id], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })
                    .optional()?;
                Ok(row.map(|(kind, raw)| L0Original {
                    r#ref: r.as_str(),
                    kind: "fact",
                    text: format!("{kind} {raw}"),
                }))
            }
        }
    }

    pub fn fact_by_id(&self, id: i64) -> Result<Option<FactRow>> {
        self.conn
            .query_row(
                "SELECT id, ts, kind, raw, normalized, source_kind,
                        chunk_id, frame_id, app_id, window_title, url
                 FROM facts WHERE id = ?1",
                [id],
                map_fact_row,
            )
            .optional()
            .map_err(Into::into)
    }

    /// 收集這段時間裡的 L0／L2 血緣起點，給 cascade tombstone 用。
    ///
    /// 要在刪 frames／facts 之前叫：刪完就問不到 id 了。
    pub fn collect_cascade_parents(&self, from_ts: Millis, to_ts: Millis) -> Result<Vec<String>> {
        let mut parents = Vec::new();
        let mut frames = self
            .conn
            .prepare("SELECT id FROM frames WHERE ts >= ?1 AND ts < ?2")?;
        for id in frames
            .query_map(params![from_ts, to_ts], |r| r.get::<_, i64>(0))?
            .flatten()
        {
            parents.push(format!("frame:{id}"));
        }
        let mut facts = self
            .conn
            .prepare("SELECT id FROM facts WHERE ts >= ?1 AND ts < ?2")?;
        for id in facts
            .query_map(params![from_ts, to_ts], |r| r.get::<_, i64>(0))?
            .flatten()
        {
            parents.push(format!("fact:{id}"));
        }
        let mut segs = self.conn.prepare(
            "SELECT core_started_at FROM segment WHERE ended_at > ?1 AND started_at < ?2",
        )?;
        for core in segs
            .query_map(params![from_ts, to_ts], |r| r.get::<_, i64>(0))?
            .flatten()
        {
            parents.push(format!("segment:{core}"));
        }
        let mut cards = self.conn.prepare(
            "SELECT id FROM l2_card WHERE segment_core_start >= ?1 AND segment_core_start < ?2",
        )?;
        for id in cards
            .query_map(params![from_ts, to_ts], |r| r.get::<_, i64>(0))?
            .flatten()
        {
            parents.push(format!("l2:{id}"));
        }
        Ok(parents)
    }

    pub fn collect_cascade_parents_before(&self, before_ts: Millis) -> Result<Vec<String>> {
        self.collect_cascade_parents(0, before_ts)
    }

    /// 沿 provenance 把衍生列標成墓碑。同一 transaction，呼叫端負責 commit。
    pub fn tombstone_descendants(
        tx: &rusqlite::Transaction<'_>,
        parents: &[String],
        now: Millis,
    ) -> Result<TombstoneReport> {
        // **根自己也要死。**只走子代的話，`collect_cascade_parents` 明明照
        // 時間範圍收進來的 `l2:` 卡片一列都不會動——它們是「起點」，不是
        // 任何人的「子代」。而血緣接不上的時候（`migrate_012` 不回填，
        // alpha.58 就在用的卡片一列 provenance 都沒有）那就是全部。
        // 舊版這裡是一句照時間範圍的 DELETE，無條件。
        let mut seen: BTreeSet<String> = parents.iter().cloned().collect();
        let mut queue: Vec<String> = parents.to_vec();
        while let Some(parent) = queue.pop() {
            let mut stmt = tx.prepare("SELECT child_ref FROM provenance WHERE parent_ref = ?1")?;
            let children: Vec<String> =
                stmt.query_map([&parent], |r| r.get(0))?.flatten().collect();
            drop(stmt);
            for child in children {
                if seen.insert(child.clone()) {
                    queue.push(child);
                }
            }
        }

        // 每一句都**連內容一起清掉**，不是只蓋一個日期。
        //
        // 墓碑要留下的是「這裡曾經有東西、被刪掉了」，好讓血緣圖不要開天窗、
        // 讓重算不要把它長回來。留下來的**不包括它說了什麼**。鐵律 2：
        // 「任何 L2/L3 不得成為證據的唯一載體」——L0 那一段刪掉之後，這幾張
        // 表就是那段螢幕內容僅存的地方了，而使用者按的是忘掉。
        //
        // 這一段之前只寫 `tombstoned_at`。查詢看不到、測試是綠的，而人名和
        // 金額還躺在檔案裡：`forget` 從此不再刪掉 L2/L3 的字。
        let mut report = TombstoneReport::default();
        for r in &seen {
            if let Some(id) = r.strip_prefix("l2:") {
                let n = tx.execute(
                    "UPDATE l2_card SET tombstoned_at = ?1,
                       activity = '', entities_json = '[]', continues_json = NULL,
                       commitments_json = '[]', evidence_json = '[]',
                       open_questions_json = '[]'
                     WHERE id = ?2 AND tombstoned_at IS NULL",
                    params![now, id],
                )?;
                report.l2 += n as u64;
            } else if let Some(id) = r.strip_prefix("commitment:") {
                let n = tx.execute(
                    "UPDATE commitments SET tombstoned_at = ?1, updated_at = ?1,
                       text = '', evidence_json = '[]', people_json = '[]',
                       due_hint = NULL, allowed_next_step = NULL, kill_note = NULL
                     WHERE id = ?2 AND tombstoned_at IS NULL",
                    params![now, id],
                )?;
                report.commitments += n as u64;
            } else if let Some(id) = r.strip_prefix("entity-mention:") {
                let n = tx.execute(
                    "UPDATE entity_mentions SET tombstoned_at = ?1, seen_ref = ''
                     WHERE id = ?2 AND tombstoned_at IS NULL",
                    params![now, id],
                )?;
                report.mentions += n as u64;
            } else if let Some(id) = r.strip_prefix("daysummary:") {
                let n = tx.execute(
                    "UPDATE day_summaries SET tombstoned_at = ?1, narrative = '',
                       session_refs_json = '[]', stats_json = '{}'
                     WHERE id = ?2 AND tombstoned_at IS NULL",
                    params![now, id],
                )?;
                report.day_summaries += n as u64;
            } else if let Some(id) = r.strip_prefix("entity:") {
                let n = tx.execute(
                    "UPDATE entities SET tombstoned_at = ?1, name = '',
                       aliases_json = '[]', notes = NULL, first_seen_ref = ''
                     WHERE id = ?2 AND tombstoned_at IS NULL",
                    params![now, id],
                )?;
                report.entities += n as u64;
            }
        }

        // 沒有 live mention 的 entity 一起死。名字也一起走：一個沒有任何
        // 出處還留著「王小明」的列，就是那三個字最後的藏身處。
        let n = tx.execute(
            "UPDATE entities SET tombstoned_at = ?1, name = '',
               aliases_json = '[]', notes = NULL, first_seen_ref = ''
             WHERE tombstoned_at IS NULL
               AND NOT EXISTS (
                 SELECT 1 FROM entity_mentions m
                 WHERE m.entity_id = entities.id AND m.tombstoned_at IS NULL
               )
               AND EXISTS (
                 SELECT 1 FROM entity_mentions m
                 WHERE m.entity_id = entities.id
               )",
            [now],
        )?;
        report.entities += n as u64;
        Ok(report)
    }

    /// L3 寫入。第一個參數的型別是 [`crate::reviewer::L3Write`]：只有
    /// Reviewer 鑄得出來，所以「別的地方寫進承諾表」是編不過的。
    pub fn insert_commitment(
        &mut self,
        permit: crate::reviewer::L3Write,
        ins: &CommitmentInsert<'_>,
    ) -> Result<i64> {
        let _gate = permit;
        self.conn.execute(
            "INSERT INTO commitments(
                text, kind, born_from, evidence_json, people_json,
                due_hint, due_source, due_at, status, confidence,
                allowed_next_step, last_evidence_seen_at, kill_note,
                created_at, updated_at, tombstoned_at
             ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?14,NULL)",
            params![
                ins.text,
                ins.kind,
                ins.born_from,
                ins.evidence_json,
                ins.people_json,
                ins.due_hint,
                ins.due_source,
                ins.due_at,
                ins.status,
                ins.confidence,
                ins.allowed_next_step,
                ins.last_evidence_seen_at,
                ins.kill_note,
                ins.now,
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        let child = format!("commitment:{id}");
        self.insert_provenance(&child, &format!("l2:{}", ins.born_from))?;
        if let Ok(refs) = serde_json::from_str::<Vec<String>>(&ins.evidence_json) {
            for r in refs {
                self.insert_provenance(&child, &r)?;
            }
        }
        Ok(id)
    }

    pub fn update_commitment_status(
        &mut self,
        permit: crate::reviewer::L3Write,
        id: i64,
        status: &str,
        kill_note: Option<&str>,
        now: Millis,
    ) -> Result<u64> {
        let _gate = permit;
        let n = self.conn.execute(
            "UPDATE commitments SET status = ?1, kill_note = COALESCE(?2, kill_note),
                    updated_at = ?3
             WHERE id = ?4 AND tombstoned_at IS NULL",
            params![status, kill_note, now, id],
        )?;
        Ok(n as u64)
    }

    pub fn commitment_by_id(&self, id: i64) -> Result<Option<CommitmentRow>> {
        self.conn
            .query_row(
                "SELECT id, text, kind, born_from, evidence_json, people_json,
                        due_hint, due_source, due_at, status, confidence,
                        allowed_next_step, last_evidence_seen_at, kill_note,
                        created_at, updated_at, tombstoned_at
                 FROM commitments WHERE id = ?1",
                [id],
                map_commitment_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn live_commitments(&self) -> Result<Vec<CommitmentRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, text, kind, born_from, evidence_json, people_json,
                    due_hint, due_source, due_at, status, confidence,
                    allowed_next_step, last_evidence_seen_at, kill_note,
                    created_at, updated_at, tombstoned_at
             FROM commitments WHERE tombstoned_at IS NULL
             ORDER BY created_at DESC, id DESC",
        )?;
        let rows = stmt.query_map([], map_commitment_row)?;
        Ok(rows.flatten().collect())
    }

    pub fn all_commitments(&self) -> Result<Vec<CommitmentRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, text, kind, born_from, evidence_json, people_json,
                    due_hint, due_source, due_at, status, confidence,
                    allowed_next_step, last_evidence_seen_at, kill_note,
                    created_at, updated_at, tombstoned_at
             FROM commitments ORDER BY created_at DESC, id DESC",
        )?;
        let rows = stmt.query_map([], map_commitment_row)?;
        Ok(rows.flatten().collect())
    }

    pub fn open_commitments_due_before(&self, cutoff: Millis) -> Result<Vec<CommitmentRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, text, kind, born_from, evidence_json, people_json,
                    due_hint, due_source, due_at, status, confidence,
                    allowed_next_step, last_evidence_seen_at, kill_note,
                    created_at, updated_at, tombstoned_at
             FROM commitments
             WHERE tombstoned_at IS NULL AND status = 'open'
               AND due_at IS NOT NULL AND due_at < ?1
               AND updated_at = created_at
             ORDER BY due_at, id",
        )?;
        let rows = stmt.query_map([cutoff], map_commitment_row)?;
        Ok(rows.flatten().collect())
    }

    pub fn upsert_entity(
        &mut self,
        permit: crate::reviewer::L3Write,
        kind: &str,
        name: &str,
        first_seen_ref: &str,
        now: Millis,
    ) -> Result<i64> {
        let _gate = permit;
        if let Some(id) = self
            .conn
            .query_row(
                "SELECT id FROM entities
                 WHERE kind = ?1 AND name = ?2 AND tombstoned_at IS NULL
                 ORDER BY id LIMIT 1",
                params![kind, name],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
        {
            return Ok(id);
        }
        self.conn.execute(
            "INSERT INTO entities(kind, name, aliases_json, first_seen_ref, notes, created_at, tombstoned_at)
             VALUES(?1,?2,'[]',?3,NULL,?4,NULL)",
            params![kind, name, first_seen_ref, now],
        )?;
        let id = self.conn.last_insert_rowid();
        self.insert_provenance(&format!("entity:{id}"), first_seen_ref)?;
        Ok(id)
    }

    pub fn insert_entity_mention(
        &mut self,
        permit: crate::reviewer::L3Write,
        entity_id: i64,
        seen_ref: &str,
        now: Millis,
    ) -> Result<i64> {
        let _gate = permit;
        self.conn.execute(
            "INSERT INTO entity_mentions(entity_id, seen_ref, created_at, tombstoned_at)
             VALUES(?1,?2,?3,NULL)",
            params![entity_id, seen_ref, now],
        )?;
        let id = self.conn.last_insert_rowid();
        let child = format!("entity-mention:{id}");
        self.insert_provenance(&child, seen_ref)?;
        self.insert_provenance(&format!("entity:{entity_id}"), &child)?;
        Ok(id)
    }

    pub fn live_entities(&self) -> Result<Vec<EntityRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, kind, name, aliases_json, first_seen_ref, notes, created_at, tombstoned_at
             FROM entities WHERE tombstoned_at IS NULL ORDER BY name, id",
        )?;
        let rows = stmt.query_map([], map_entity_row)?;
        Ok(rows.flatten().collect())
    }

    pub fn live_mentions_for(&self, entity_id: i64) -> Result<Vec<MentionRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, entity_id, seen_ref, created_at, tombstoned_at
             FROM entity_mentions WHERE entity_id = ?1 AND tombstoned_at IS NULL",
        )?;
        let rows = stmt.query_map([entity_id], |r| {
            Ok(MentionRow {
                id: r.get(0)?,
                entity_id: r.get(1)?,
                seen_ref: r.get(2)?,
                created_at: r.get(3)?,
                tombstoned_at: r.get(4)?,
            })
        })?;
        Ok(rows.flatten().collect())
    }

    pub fn insert_day_summary(
        &mut self,
        permit: crate::reviewer::L3Write,
        ins: &DaySummaryInsert<'_>,
    ) -> Result<i64> {
        let _gate = permit;
        let prev: Option<(i64, i32)> = self
            .conn
            .query_row(
                "SELECT id, version FROM day_summaries
                 WHERE date = ?1 AND tombstoned_at IS NULL
                 ORDER BY version DESC, id DESC LIMIT 1",
                [ins.date],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let version = prev.as_ref().map(|p| p.1 + 1).unwrap_or(1);
        let supersedes = prev.map(|p| p.0);
        self.conn.execute(
            "INSERT INTO day_summaries(
                date, version, supersedes, narrative, session_refs_json,
                stats_json, created_at, tombstoned_at
             ) VALUES(?1,?2,?3,?4,?5,?6,?7,NULL)",
            params![
                ins.date,
                version,
                supersedes,
                ins.narrative,
                ins.session_refs_json,
                ins.stats_json,
                ins.now,
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        let child = format!("daysummary:{id}");
        if let Ok(refs) = serde_json::from_str::<Vec<String>>(&ins.session_refs_json) {
            for r in refs {
                self.insert_provenance(&child, &r)?;
            }
        }
        Ok(id)
    }

    pub fn latest_day_summary(&self, date: &str) -> Result<Option<DaySummaryRow>> {
        // 墓碑列故意不回：`forget` 把 narrative 清成空字串，拿來當「這一天
        // 的摘要」會把「被刪掉了」講成「那天什麼都沒發生」。墓碑用
        // [`Self::day_summary_tombstoned_at`]。
        self.conn
            .query_row(
                "SELECT id, date, version, supersedes, narrative, session_refs_json,
                        stats_json, created_at, tombstoned_at
                 FROM day_summaries
                 WHERE date = ?1 AND tombstoned_at IS NULL
                 ORDER BY version DESC, id DESC LIMIT 1",
                [date],
                map_day_summary_row,
            )
            .optional()
            .map_err(Into::into)
    }

    /// 這一天有沒有被忘掉的日摘要。只回時間，不回內容——內容已經清空了。
    pub fn day_summary_tombstoned_at(&self, date: &str) -> Result<Option<Millis>> {
        self.conn
            .query_row(
                "SELECT tombstoned_at FROM day_summaries
                 WHERE date = ?1 AND tombstoned_at IS NOT NULL
                 ORDER BY version DESC, id DESC LIMIT 1",
                [date],
                |r| r.get(0),
            )
            .optional()
            .map_err(Into::into)
    }

    /// 審閱層的日終，有沒有盤點過這一天。
    ///
    /// `reviewer_run.day_key` 是「這一輪是哪一天跑的」，不是被盤點的那一天。
    /// 日終只在跨日之後才跑（換日或開機補跑），收的永遠是昨天，所以：
    ///
    /// `盤點過 D ⇔ 存在成功的 eod run，其 day_key == D 的隔天`。
    ///
    /// 關機三天後開機只會補昨天，更早的日子沒被盤點——這個判準也會回 false。
    /// 過濾條件其餘和 [`Self::last_reviewer_eod_day`] 同一套：`kind = 'eod'`
    /// 且 `skip_reason IS NULL`。跳過不算跑過。
    pub fn has_reviewer_eod_for_day(&self, date: &str) -> Result<bool> {
        let Some(run_day) = crate::local_day::next_local_day_key(date) else {
            return Ok(false);
        };
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM reviewer_run
             WHERE kind = 'eod' AND skip_reason IS NULL AND day_key = ?1",
            [run_day],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    /// 時間軸要攤的那一列日摘要。三種「沒有」在型別上就分開，不能印成同一句。
    ///
    /// 不看這一天現在有沒有 L2 卡片。`write_day_summary` 在沒有卡片時不寫列，
    /// 那件事的證據是 `reviewer_run`，不是此刻的 `l2_card`。
    pub fn day_summary_glance(&self, date: &str) -> Result<DaySummaryGlance> {
        if let Some(row) = self.latest_day_summary(date)? {
            let (clauses, aligned) = clauses_from(&row.narrative, &row.session_refs_json);
            let (l2, commitments_open) = parse_day_stats(&row.stats_json);
            return Ok(DaySummaryGlance::Live {
                date: row.date,
                version: row.version,
                supersedes: row.supersedes,
                created_at: row.created_at,
                clauses,
                aligned,
                l2,
                commitments_open,
            });
        }
        if let Some(tombstoned_at) = self.day_summary_tombstoned_at(date)? {
            return Ok(DaySummaryGlance::Tombstoned {
                date: date.to_string(),
                tombstoned_at,
            });
        }
        if self.has_reviewer_eod_for_day(date)? {
            return Ok(DaySummaryGlance::EodEmpty {
                date: date.to_string(),
            });
        }
        Ok(DaySummaryGlance::NeverRan {
            date: date.to_string(),
        })
    }

    pub fn upsert_preference(
        &mut self,
        permit: crate::reviewer::L3Write,
        key: &str,
        value: &str,
        learned_from: &str,
        now: Millis,
    ) -> Result<()> {
        let _gate = permit;
        self.conn.execute(
            "INSERT INTO preferences(key, value, learned_from, updated_at)
             VALUES(?1,?2,?3,?4)
             ON CONFLICT(key) DO UPDATE SET
                value = excluded.value,
                learned_from = excluded.learned_from,
                updated_at = excluded.updated_at",
            params![key, value, learned_from, now],
        )?;
        Ok(())
    }

    pub fn preference(&self, key: &str) -> Result<Option<PreferenceRow>> {
        self.conn
            .query_row(
                "SELECT key, value, learned_from, updated_at FROM preferences WHERE key = ?1",
                [key],
                |r| {
                    Ok(PreferenceRow {
                        key: r.get(0)?,
                        value: r.get(1)?,
                        learned_from: r.get(2)?,
                        updated_at: r.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn insert_reviewer_run(&mut self, ins: &ReviewerRunInsert<'_>) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO reviewer_run(
                ts, day_key, kind, skip_reason, candidate_count, recheck_count,
                wrote_commitments, divergences, calls_used, budget_used,
                budget_limit, detail
             ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            params![
                ins.ts,
                ins.day_key,
                ins.kind,
                ins.skip_reason,
                ins.candidate_count,
                ins.recheck_count,
                ins.wrote_commitments,
                ins.divergences,
                ins.calls_used,
                ins.budget_used,
                ins.budget_limit,
                ins.detail,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn insert_reviewer_recheck(&mut self, ins: &RecheckInsert<'_>) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO reviewer_recheck(
                run_id, category, child_ref, parent_ref,
                original_present, original_chars, matched
             ) VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![
                ins.run_id,
                ins.category,
                ins.child_ref,
                ins.parent_ref,
                ins.original_present as i64,
                ins.original_chars,
                ins.matched as i64,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn insert_reviewer_divergence(&mut self, ins: &DivergenceInsert<'_>) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO reviewer_divergence(
                run_id, subject, pass_a_json, pass_b_json, reason, created_at
             ) VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                ins.run_id,
                ins.subject,
                ins.pass_a_json,
                ins.pass_b_json,
                ins.reason,
                ins.created_at,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn last_reviewer_run_at(&self) -> Result<Option<Millis>> {
        let v: Option<Millis> = self.conn.query_row(
            "SELECT MAX(ts) FROM reviewer_run WHERE skip_reason IS NULL",
            [],
            |r| r.get(0),
        )?;
        Ok(v)
    }

    pub fn last_reviewer_eod_day(&self) -> Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT day_key FROM reviewer_run
                 WHERE kind = 'eod' AND skip_reason IS NULL
                 ORDER BY ts DESC, id DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn reviewer_recheck_stats(&self) -> Result<RecheckStats> {
        let runs: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM reviewer_run WHERE skip_reason IS NULL",
            [],
            |r| r.get(0),
        )?;
        if runs == 0 {
            let last_skip: Option<String> = self
                .conn
                .query_row(
                    "SELECT skip_reason FROM reviewer_run
                     WHERE skip_reason IS NOT NULL
                     ORDER BY ts DESC, id DESC LIMIT 1",
                    [],
                    |r| r.get(0),
                )
                .optional()?;
            return Ok(RecheckStats {
                runs: None,
                candidates: None,
                rechecks: None,
                last_skip,
            });
        }
        let candidates: i64 = self.conn.query_row(
            "SELECT COALESCE(SUM(candidate_count), 0) FROM reviewer_run WHERE skip_reason IS NULL",
            [],
            |r| r.get(0),
        )?;
        let rechecks: i64 = self.conn.query_row(
            "SELECT COALESCE(SUM(recheck_count), 0) FROM reviewer_run WHERE skip_reason IS NULL",
            [],
            |r| r.get(0),
        )?;
        Ok(RecheckStats {
            runs: Some(runs.max(0) as u32),
            candidates: Some(candidates.max(0) as u32),
            rechecks: Some(rechecks.max(0) as u32),
            last_skip: None,
        })
    }

    pub fn list_reviewer_divergences(&self, limit: usize) -> Result<Vec<DivergenceRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, run_id, subject, pass_a_json, pass_b_json, reason, created_at
             FROM reviewer_divergence ORDER BY created_at DESC, id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], |r| {
            Ok(DivergenceRow {
                id: r.get(0)?,
                run_id: r.get(1)?,
                subject: r.get(2)?,
                pass_a_json: r.get(3)?,
                pass_b_json: r.get(4)?,
                reason: r.get(5)?,
                created_at: r.get(6)?,
            })
        })?;
        Ok(rows.flatten().collect())
    }

    /// 把相鄰兩段併成一段。寫進 `segment_edit` 再重算當天。
    pub fn merge_chapters(
        &mut self,
        left_core_start: Millis,
        right_core_start: Millis,
        from_ts: Millis,
        to_ts: Millis,
    ) -> Result<Vec<crate::segment::Segment>> {
        let current = self.chapters_for_range(from_ts, to_ts)?;
        let (at, from_ms, to_ms) = {
            let left = current
                .iter()
                .find(|s| s.core_started_at == left_core_start)
                .ok_or_else(|| anyhow::anyhow!("找不到要合併的左邊那一段。"))?;
            let right = current
                .iter()
                .find(|s| s.core_started_at == right_core_start)
                .ok_or_else(|| anyhow::anyhow!("找不到要合併的右邊那一段。"))?;
            anyhow::ensure!(
                left.core_ended_at == right.core_started_at,
                "這兩段現在不是相鄰的，沒有合併。"
            );
            (
                right.core_started_at,
                left.core_started_at,
                right.core_ended_at,
            )
        };
        let (algo_kinds, algo_conf) = self.algorithm_boundary_at(from_ts, to_ts, at)?;
        self.insert_segment_edit(
            "merge",
            Some(at),
            Some(from_ms),
            Some(to_ms),
            algo_kinds.as_deref(),
            algo_conf,
            None,
        )?;
        self.chapters_for_range(from_ts, to_ts)
    }

    /// 在 `at_ms` 把一段切成兩段。切點必須落在某一段的核心裡、而且不是兩端。
    pub fn split_chapter(
        &mut self,
        at_ms: Millis,
        from_ts: Millis,
        to_ts: Millis,
    ) -> Result<Vec<crate::segment::Segment>> {
        let current = self.chapters_for_range(from_ts, to_ts)?;
        let (from_ms, to_ms) = {
            let host = current
                .iter()
                .find(|s| s.core_started_at < at_ms && at_ms < s.core_ended_at);
            let Some(host) = host else {
                anyhow::bail!("這個時間不在任何一段的中間，沒有切開。");
            };
            (host.core_started_at, host.core_ended_at)
        };
        let (algo_kinds, algo_conf) = self.algorithm_boundary_at(from_ts, to_ts, at_ms)?;
        self.insert_segment_edit(
            "split",
            Some(at_ms),
            Some(from_ms),
            Some(to_ms),
            algo_kinds.as_deref(),
            algo_conf,
            None,
        )?;
        self.chapters_for_range(from_ts, to_ts)
    }

    /// 撤銷某一筆編輯。多寫一列 `undo`，不改舊列。
    pub fn undo_segment_edit(
        &mut self,
        edit_id: i64,
        from_ts: Millis,
        to_ts: Millis,
    ) -> Result<Vec<crate::segment::Segment>> {
        let edits = self.segment_edits_overlapping(from_ts, to_ts)?;
        let (at_ms, from_ms, to_ms, algo_kinds, algo_conf) = {
            let target = edits
                .iter()
                .find(|e| e.id == edit_id)
                .ok_or_else(|| anyhow::anyhow!("找不到要撤銷的那次修改。"))?;
            anyhow::ensure!(
                target.kind != "undo",
                "只能撤銷合併或切開，不能撤銷一次撤銷。"
            );
            let active: Vec<i64> = crate::segment_edit::active_edits(&edits)
                .into_iter()
                .map(|e| e.id)
                .collect();
            anyhow::ensure!(active.contains(&edit_id), "那次修改已經撤銷過了。");
            (
                target.at_ms,
                target.from_ms,
                target.to_ms,
                target.algo_cut_kinds.clone(),
                target.algo_confidence,
            )
        };
        self.insert_segment_edit(
            "undo",
            at_ms,
            from_ms,
            to_ms,
            algo_kinds.as_deref(),
            algo_conf,
            Some(edit_id),
        )?;
        self.chapters_for_range(from_ts, to_ts)
    }

    fn algorithm_boundary_at(
        &self,
        from_ts: Millis,
        to_ts: Millis,
        at: Millis,
    ) -> Result<(Option<Vec<crate::segment::CutKind>>, Option<f32>)> {
        let pad = crate::segment::LOOKAROUND_MS;
        let stream = self.segment_events(from_ts.saturating_sub(pad), to_ts.saturating_add(pad))?;
        let raw = crate::segment::segment(&stream);
        let hit = raw.iter().find(|s| s.core_started_at == at);
        match hit {
            Some(s) if !s.cut_kinds.is_empty() => Ok((Some(s.cut_kinds.clone()), s.confidence)),
            _ => Ok((None, None)),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_segment_edit(
        &mut self,
        kind: &str,
        at_ms: Option<Millis>,
        from_ms: Option<Millis>,
        to_ms: Option<Millis>,
        algo_cut_kinds: Option<&[crate::segment::CutKind]>,
        algo_confidence: Option<f32>,
        target_id: Option<i64>,
    ) -> Result<i64> {
        let kinds = algo_cut_kinds.and_then(|k| {
            if k.is_empty() {
                None
            } else {
                Some(k.iter().map(|c| c.as_str()).collect::<Vec<_>>().join(","))
            }
        });
        self.conn.execute(
            "INSERT INTO segment_edit(
                ts, kind, at_ms, from_ms, to_ms,
                algo_cut_kinds, algo_confidence, target_id
             ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                now_ms(),
                kind,
                at_ms,
                from_ms,
                to_ms,
                kinds,
                algo_confidence.map(|c| c as f64),
                target_id,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    fn segment_edits_overlapping(
        &self,
        from_ts: Millis,
        to_ts: Millis,
    ) -> Result<Vec<crate::segment_edit::StoredEdit>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, ts, kind, at_ms, from_ms, to_ms,
                    algo_cut_kinds, algo_confidence, target_id
             FROM segment_edit
             WHERE from_ms < ?2 AND to_ms > ?1
             ORDER BY id",
        )?;
        let rows = stmt.query_map(params![from_ts, to_ts], |r| {
            let kinds: Option<String> = r.get(6)?;
            Ok(crate::segment_edit::StoredEdit {
                id: r.get(0)?,
                ts: r.get(1)?,
                kind: r.get(2)?,
                at_ms: r.get(3)?,
                from_ms: r.get(4)?,
                to_ms: r.get(5)?,
                algo_cut_kinds: kinds.as_deref().map(|s| {
                    s.split(',')
                        .filter(|p| !p.is_empty())
                        .filter_map(crate::segment::CutKind::from_str_kind)
                        .collect()
                }),
                algo_confidence: r.get::<_, Option<f64>>(7)?.map(|c| c as f32),
                target_id: r.get(8)?,
            })
        })?;
        Ok(rows.flatten().collect())
    }

    fn replace_stuck(
        &mut self,
        from_ts: Millis,
        to_ts: Millis,
        raw: &[crate::segment::Segment],
    ) -> Result<()> {
        let inputs = self.stuck_inputs(from_ts, to_ts)?;
        let errors = self.stuck_errors(from_ts, to_ts)?;
        let found = crate::stuck::detect(raw, &inputs, &errors);
        let computed_at = now_ms();
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM stuck_signal WHERE started_at >= ?1 AND started_at < ?2",
            [from_ts, to_ts],
        )?;
        {
            let mut ins = tx.prepare(
                "INSERT INTO stuck_signal(
                    started_at, ended_at, app_id, window_title,
                    dwell_ms, switch_count, error_fact_count, computed_at
                 ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            )?;
            for s in &found {
                ins.execute(params![
                    s.started_at,
                    s.ended_at,
                    s.app,
                    s.title,
                    s.dwell_ms,
                    s.switch_count,
                    s.error_fact_count,
                    computed_at,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    fn stuck_inputs(&self, from_ts: Millis, to_ts: Millis) -> Result<Vec<crate::stuck::InputSpan>> {
        let mut stmt = self.conn.prepare(
            "SELECT ts_start, ts_end, window_switches FROM input_metrics
             WHERE ts_end > ?1 AND ts_start < ?2 ORDER BY ts_start, id",
        )?;
        let rows = stmt.query_map(params![from_ts, to_ts], |r| {
            Ok(crate::stuck::InputSpan {
                ts_start: r.get(0)?,
                ts_end: r.get(1)?,
                window_switches: r.get(2)?,
            })
        })?;
        Ok(rows.flatten().collect())
    }

    fn stuck_errors(&self, from_ts: Millis, to_ts: Millis) -> Result<Vec<crate::stuck::ErrorHit>> {
        let mut stmt = self.conn.prepare(
            "SELECT ts, app_id FROM facts
             WHERE kind = 'error_code' AND ts >= ?1 AND ts < ?2",
        )?;
        let rows = stmt.query_map(params![from_ts, to_ts], |r| {
            Ok(crate::stuck::ErrorHit {
                ts: r.get(0)?,
                app: r.get(1)?,
            })
        })?;
        Ok(rows.flatten().collect())
    }

    fn replace_segments(
        &mut self,
        from_ts: Millis,
        to_ts: Millis,
        segs: &[crate::segment::Segment],
    ) -> Result<()> {
        let computed_at = now_ms();
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM segment WHERE core_started_at >= ?1 AND core_started_at < ?2",
            [from_ts, to_ts],
        )?;
        {
            let mut ins = tx.prepare(
                "INSERT INTO segment(
                    started_at, ended_at, core_started_at, core_ended_at,
                    app_id, window_title, url_host, cut_kinds, confidence,
                    event_ids, computed_at
                 ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            )?;
            for s in segs {
                let kinds = if s.cut_kinds.is_empty() {
                    None
                } else {
                    Some(
                        s.cut_kinds
                            .iter()
                            .map(|k| k.as_str())
                            .collect::<Vec<_>>()
                            .join(","),
                    )
                };
                let event_ids =
                    serde_json::to_string(&s.event_ids).context("serialize segment event_ids")?;
                ins.execute(params![
                    s.started_at,
                    s.ended_at,
                    s.core_started_at,
                    s.core_ended_at,
                    s.app,
                    s.title,
                    s.host,
                    kinds,
                    s.confidence.map(|c| c as f64),
                    event_ids,
                    computed_at,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// `ts` 之後寫出去的畫面檔一共多少位元組。
    ///
    /// 用途是讓「每天畫面上限」在**重開之後仍然成立**。少了這一步，那個
    /// 上限只管得住單一次執行：關掉再開就歸零，一天重開十次就是十倍額度。
    /// 一個可以靠重開繞過的上限不是上限，而且它會安靜地不生效——正是這個
    /// 專案最主要的失效形狀。
    /// **問不出來要往上報，不可以回 0。** 回 0 的意思是「今天還沒寫過圖」，
    /// 也就是「整份額度都還在」——一個問不到答案的上限會安靜地變成沒有上限。
    /// 這和 `timings` 那邊用 `Option` 區分「零」與「不知道」是同一條紀律。
    pub fn image_bytes_since(&self, ts: crate::model::Millis) -> Result<u64> {
        let n: i64 = self
            .conn
            .query_row(
                "SELECT COALESCE(SUM(image_bytes),0) FROM frames WHERE ts >= ?1",
                [ts],
                |r| r.get(0),
            )
            .context("sum today's image bytes")?;
        Ok(n.max(0) as u64)
    }

    /// 把 `[from, to)` 裡可重播的 L0 訊號匯成已自動去敏的私有草稿。
    ///
    /// 這不是 [`Self::export_to`] 的另一個名字：完整備份保留每一個原字和畫面
    /// 路徑，replay corpus 則刻意沒有任何 pixel、圖片 bytes 或 `image_path`。
    /// L1 facts 與三份 FTS 索引也不直接搬；import 會從同一批 L0 文字重建，
    /// 這樣不同版本比較到的是各自真正會產生的結果，不是來源資料庫的舊答案。
    pub fn export_replay(
        &self,
        name: &str,
        from: Millis,
        to: Millis,
    ) -> Result<crate::replay::DraftCorpus> {
        let duration_ms = to
            .checked_sub(from)
            .context("replay export time range overflowed")?;
        anyhow::ensure!(duration_ms >= 0, "replay export 的結尾早於起點");

        let relative = |ts: Millis, label: &str| -> Result<Millis> {
            let at = ts
                .checked_sub(from)
                .with_context(|| format!("{label} timestamp overflowed replay origin"))?;
            anyhow::ensure!(
                (0..=duration_ms).contains(&at),
                "{label} timestamp {ts} 不在 replay export 範圍內"
            );
            Ok(at)
        };

        let mut events = Vec::new();
        for frame in self.replay_frame_rows(from, to)? {
            events.push(crate::replay::Event::Frame {
                at_ms: relative(frame.ts, "frame")?,
                monitor: frame.monitor,
                width: frame.width,
                height: frame.height,
                dhash: frame.dhash,
                dup_run: frame.dup_run,
                focus: crate::replay::ReplayFocus {
                    app_id: frame.focus.app_id,
                    app_name: frame.focus.app_name,
                    window_title: frame.focus.window_title,
                    url: frame.focus.url,
                },
                ocr: frame.ocr,
            });
        }

        let mut statement = self.conn.prepare(
            "SELECT ts, kind, app_id, app_name, window_title, url
             FROM focus_events WHERE ts >= ?1 AND ts < ?2 ORDER BY ts, id",
        )?;
        let mut rows = statement.query(params![from, to])?;
        while let Some(row) = rows.next()? {
            let ts: Millis = row.get(0)?;
            let kind: String = row.get(1)?;
            events.push(crate::replay::Event::Focus {
                at_ms: relative(ts, "focus event")?,
                kind: focus_kind_from_db(&kind)?,
                snapshot: crate::replay::ReplayFocus {
                    app_id: row.get(2)?,
                    app_name: row.get(3)?,
                    window_title: row.get(4)?,
                    url: row.get(5)?,
                },
            });
        }
        drop(rows);
        drop(statement);

        let mut statement = self.conn.prepare(
            "SELECT ts, kind, text, byte_len, truncated, secret_suspected, source_app
             FROM clipboard_events WHERE ts >= ?1 AND ts < ?2 ORDER BY ts, id",
        )?;
        let mut rows = statement.query(params![from, to])?;
        while let Some(row) = rows.next()? {
            let ts: Millis = row.get(0)?;
            let kind: String = row.get(1)?;
            events.push(crate::replay::Event::Clipboard {
                at_ms: relative(ts, "clipboard event")?,
                kind: clipboard_kind_from_db(&kind)?,
                text: row.get(2)?,
                byte_len: row.get(3)?,
                truncated: bool_from_db(row.get(4)?, "clipboard truncated")?,
                secret_suspected: bool_from_db(row.get(5)?, "clipboard secret_suspected")?,
                source_app: row.get(6)?,
            });
        }
        drop(rows);
        drop(statement);

        // 一個輸入視窗只有整段都落在範圍內才帶走。把跨過右界的那一筆截短會
        // 改寫節奏，照樣帶走則會讓 corpus 自稱的 duration 變成假話。
        let mut statement = self.conn.prepare(
            "SELECT ts_start, ts_end, keystrokes, clicks, mouse_px, scroll_ticks,
                    window_switches, idle_ms, typing_bursts
             FROM input_metrics
             WHERE ts_start >= ?1 AND ts_start < ?2 AND ts_end <= ?2
             ORDER BY ts_start, id",
        )?;
        let mut rows = statement.query(params![from, to])?;
        while let Some(row) = rows.next()? {
            let start: Millis = row.get(0)?;
            let end: Millis = row.get(1)?;
            events.push(crate::replay::Event::Input {
                at_ms: relative(start, "input event start")?,
                end_ms: relative(end, "input event end")?,
                keystrokes: row.get(2)?,
                clicks: row.get(3)?,
                mouse_px: row.get(4)?,
                scroll_ticks: row.get(5)?,
                window_switches: row.get(6)?,
                idle_ms: row.get(7)?,
                typing_bursts: row.get(8)?,
            });
        }
        drop(rows);
        drop(statement);

        let system_sql = format!(
            "SELECT ts, kind, detail FROM system_events
             WHERE ts >= ?1 AND ts < ?2 AND kind NOT IN {}
             ORDER BY ts, id",
            crate::model::SystemKind::session_marks_sql()
        );
        let mut statement = self.conn.prepare(&system_sql)?;
        let mut rows = statement.query(params![from, to])?;
        while let Some(row) = rows.next()? {
            let ts: Millis = row.get(0)?;
            let kind: String = row.get(1)?;
            events.push(crate::replay::Event::System {
                at_ms: relative(ts, "system event")?,
                kind: system_kind_from_db(&kind)?,
                detail: row.get(2)?,
            });
        }

        // SQLite 沒有跨表的全域 sequence number。相同毫秒內採 recorder 的
        // 寫入順序做唯一、穩定的定義；同一類裡 `ORDER BY ... id` 的順序由
        // stable sort 保留。
        events.sort_by_key(|event| (event.at_ms(), replay_event_rank(event)));
        let corpus = crate::replay::Corpus {
            format_version: crate::replay::FORMAT_VERSION,
            name: name.to_string(),
            duration_ms,
            review: crate::replay::ReviewStatus::Draft,
            redactions: crate::replay::RedactionSummary::default(),
            events,
        };
        corpus.deidentify()
    }

    /// 把 replay 語料接到呼叫端指定的 epoch，走正式 insert 路徑重建 L0、L1
    /// 與 FTS。Draft 可以在本機匯入重播；`Reviewed` 只管能不能分享，不是本機
    /// 功能的開關。
    pub fn import_replay(
        &mut self,
        corpus: &crate::replay::Corpus,
        origin: Millis,
    ) -> Result<ReplayImport> {
        corpus.validate()?;
        let ended_at = origin
            .checked_add(corpus.duration_ms)
            .context("replay import end timestamp overflowed")?;

        // 所有時間先走完一次 checked_add，任何一筆壞掉都要在建立 session 之前
        // 失敗。否則「拒絕 malformed corpus」會在資料庫裡留下一場半截錄製。
        for (index, event) in corpus.events.iter().enumerate() {
            origin
                .checked_add(event.at_ms())
                .with_context(|| format!("replay event #{index} timestamp overflowed"))?;
            if let crate::replay::Event::Input { end_ms, .. } = event {
                origin
                    .checked_add(*end_ms)
                    .with_context(|| format!("replay input event #{index} end overflowed"))?;
            }
            if let crate::replay::Event::Clipboard {
                text: Some(_),
                secret_suspected: true,
                ..
            } = event
            {
                anyhow::bail!("replay clipboard event #{index} 同時帶著 secret_suspected 和內容");
            }
        }

        if corpus.events.is_empty() {
            return Ok(ReplayImport {
                session_id: None,
                events: 0,
                frames: 0,
                facts: 0,
            });
        }

        let session_id = self.start_session_at("replay/import", crate::VERSION, origin)?;
        self.insert_system(
            session_id,
            &SystemEvent {
                ts: origin,
                kind: crate::model::SystemKind::SessionStart,
                detail: None,
            },
        )?;

        let mut frames = 0usize;
        for event in &corpus.events {
            match event {
                crate::replay::Event::Frame {
                    at_ms,
                    monitor,
                    width,
                    height,
                    dhash,
                    dup_run,
                    focus,
                    ocr,
                } => {
                    let ts = origin + *at_ms;
                    let frame = FrameCapture {
                        ts,
                        monitor: *monitor,
                        width: *width,
                        height: *height,
                        dhash: *dhash,
                        image: None,
                        image_ext: "webp",
                        ocr: ocr.clone(),
                        focus: replay_focus_snapshot(focus),
                    };
                    let (frame_id, _, _) = self.insert_frame(session_id, &frame, None, 0)?;
                    if *dup_run > 0 {
                        self.conn.execute(
                            "UPDATE frames SET dup_run = ?1 WHERE id = ?2",
                            params![i64::from(*dup_run), frame_id],
                        )?;
                    }
                    frames += 1;
                }
                crate::replay::Event::Focus {
                    at_ms,
                    kind,
                    snapshot,
                } => {
                    self.insert_focus(
                        session_id,
                        &FocusEvent {
                            ts: origin + *at_ms,
                            kind: *kind,
                            snapshot: replay_focus_snapshot(snapshot),
                        },
                    )?;
                }
                crate::replay::Event::Clipboard {
                    at_ms,
                    kind,
                    text,
                    byte_len,
                    truncated,
                    secret_suspected,
                    source_app,
                } => {
                    self.insert_clipboard(
                        session_id,
                        &ClipboardEvent {
                            ts: origin + *at_ms,
                            kind: *kind,
                            text: text.clone(),
                            byte_len: *byte_len,
                            truncated: *truncated,
                            secret_suspected: *secret_suspected,
                            source_app: source_app.clone(),
                        },
                    )?;
                }
                crate::replay::Event::Input {
                    at_ms,
                    end_ms,
                    keystrokes,
                    clicks,
                    mouse_px,
                    scroll_ticks,
                    window_switches,
                    idle_ms,
                    typing_bursts,
                } => {
                    self.insert_input(
                        session_id,
                        &InputMetrics {
                            ts_start: origin + *at_ms,
                            ts_end: origin + *end_ms,
                            keystrokes: *keystrokes,
                            clicks: *clicks,
                            mouse_px: *mouse_px,
                            scroll_ticks: *scroll_ticks,
                            window_switches: *window_switches,
                            idle_ms: *idle_ms,
                            typing_bursts: *typing_bursts,
                        },
                    )?;
                }
                crate::replay::Event::System {
                    at_ms,
                    kind,
                    detail,
                } => {
                    self.insert_system(
                        session_id,
                        &SystemEvent {
                            ts: origin + *at_ms,
                            kind: *kind,
                            detail: detail.clone(),
                        },
                    )?;
                }
            }
        }

        self.insert_system(
            session_id,
            &SystemEvent {
                ts: ended_at,
                kind: crate::model::SystemKind::SessionEnd,
                detail: Some("duration".into()),
            },
        )?;
        self.end_session_at(session_id, ended_at)?;
        // 問這個 session 真正留下的列，不在呼叫端手抄「哪一種 insert 會長
        // facts」。frame、focus title 和 clipboard text 都可能產生 L1；只累加
        // `insert_frame` 的回傳值會印出一個每一行都真、總數卻少掉兩條路的數字。
        let facts: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM facts WHERE session_id = ?1",
            [session_id],
            |row| row.get(0),
        )?;
        Ok(ReplayImport {
            session_id: Some(session_id),
            events: corpus.events.len(),
            frames,
            facts: usize::try_from(facts).context("imported fact count did not fit usize")?,
        })
    }

    fn replay_frame_rows(&self, from: Millis, to: Millis) -> Result<Vec<ReplayFrameRow>> {
        let mut statement = self.conn.prepare(
            "SELECT f.id, f.ts, f.monitor, f.width, f.height, f.dhash, f.dup_run,
                    f.app_id, f.window_title, f.url,
                    o.id, o.text, o.x, o.y, o.w, o.h, o.confidence
             FROM frames f LEFT JOIN ocr_blocks o ON o.frame_id = f.id
             WHERE f.ts >= ?1 AND f.ts < ?2
             ORDER BY f.ts, f.id, o.id",
        )?;
        let mut rows = statement.query(params![from, to])?;
        let mut out: Vec<ReplayFrameRow> = Vec::new();
        let mut current_id = None;
        while let Some(row) = rows.next()? {
            let frame_id: i64 = row.get(0)?;
            if current_id != Some(frame_id) {
                let monitor: i64 = row.get(2)?;
                let width: i64 = row.get(3)?;
                let height: i64 = row.get(4)?;
                let dup_run: i64 = row.get(6)?;
                out.push(ReplayFrameRow {
                    ts: row.get(1)?,
                    monitor: i32::try_from(monitor).context("frame monitor did not fit i32")?,
                    width: u32::try_from(width).context("frame width did not fit u32")?,
                    height: u32::try_from(height).context("frame height did not fit u32")?,
                    // SQLite INTEGER 是 signed；寫入時 `u64 as i64`，讀回要照同一
                    // 組 bits 還原，不能把負數當壞資料。
                    dhash: row.get::<_, i64>(5)? as u64,
                    dup_run: u32::try_from(dup_run).context("frame dup_run did not fit u32")?,
                    focus: FocusSnapshot {
                        app_id: row.get(7)?,
                        app_name: None,
                        window_title: row.get(8)?,
                        url: row.get(9)?,
                        pid: None,
                        password_field: false,
                    },
                    ocr: Vec::new(),
                });
                current_id = Some(frame_id);
            }

            if row.get::<_, Option<i64>>(10)?.is_some() {
                let x: i64 = row.get(12)?;
                let y: i64 = row.get(13)?;
                let w: i64 = row.get(14)?;
                let h: i64 = row.get(15)?;
                let confidence: f64 = row.get(16)?;
                out.last_mut().context("OCR block had no frame")?.ocr.push(
                    crate::model::OcrBlock {
                        text: row.get(11)?,
                        x: i32::try_from(x).context("OCR x did not fit i32")?,
                        y: i32::try_from(y).context("OCR y did not fit i32")?,
                        w: i32::try_from(w).context("OCR width did not fit i32")?,
                        h: i32::try_from(h).context("OCR height did not fit i32")?,
                        confidence: confidence as f32,
                    },
                );
            }
        }
        Ok(out)
    }

    /// 把整個資料庫寫進另一個檔案——**一致的快照，即使她正在錄。**
    ///
    /// 為什麼不是叫使用者去複製 `sister.db`：這個資料庫跑在 WAL 模式，最近
    /// 寫進去的東西還躺在旁邊的 `sister.db-wal` 裡。只複製主檔的話，備份會
    /// **安靜地少掉最後那一段記憶**——而那正是備份最不該有的失效模式：他要
    /// 等到真的需要那份備份的那一天，才會發現最近那幾小時不見了。
    ///
    /// `VACUUM INTO` 在一個交易裡把整份內容重寫出去，WAL 裡的東西一起進去，
    /// 而且不需要停止正在寫入的那個行程。順便把檔案壓實（刪過東西之後
    /// SQLite 不會自己還空間），所以匯出檔通常比原檔小——這不是漏了東西。
    ///
    /// 目的檔已經存在就失敗，不覆蓋。SQLite 自己就是這個行為，而它剛好是對的：
    /// 一次打錯路徑的匯出不該蓋掉上一份備份。
    pub fn export_to(&self, dest: &Path) -> Result<()> {
        if dest.exists() {
            anyhow::bail!("{} 已經存在——匯出不覆蓋既有檔案", dest.display());
        }
        // 路徑走參數，不是字串拼接：檔名裡的引號會變成一段 SQL。
        self.conn
            .execute("VACUUM INTO ?1", params![dest.to_string_lossy()])
            .with_context(|| format!("VACUUM INTO {}", dest.display()))?;
        Ok(())
    }

    /// 足跡統計——直接對應 Phase 0 的 exit criteria。
    pub fn stats(&self) -> Result<DbStats> {
        let count = |sql: &str| -> Result<i64> {
            Ok(self.conn.query_row(sql, [], |r| r.get(0)).unwrap_or(0))
        };
        Ok(DbStats {
            frames: count("SELECT COUNT(*) FROM frames")?,
            frames_collapsed: count("SELECT COALESCE(SUM(dup_run),0) FROM frames")?,
            frames_with_image: count("SELECT COUNT(*) FROM frames WHERE image_path IS NOT NULL")?,
            image_bytes: count("SELECT COALESCE(SUM(image_bytes),0) FROM frames")?,
            ocr_blocks: count("SELECT COUNT(*) FROM ocr_blocks")?,
            chunks: count("SELECT COUNT(*) FROM text_chunks")?,
            facts: count("SELECT COUNT(*) FROM facts")?,
            focus_events: count("SELECT COUNT(*) FROM focus_events")?,
            clipboard_events: count("SELECT COUNT(*) FROM clipboard_events")?,
            input_windows: count("SELECT COUNT(*) FROM input_metrics")?,
            system_events: count("SELECT COUNT(*) FROM system_events")?,
            session_marks: count(&format!(
                "SELECT COUNT(*) FROM system_events WHERE kind IN {}",
                crate::model::SystemKind::session_marks_sql()
            ))?,
            sessions: count("SELECT COUNT(*) FROM sessions")?,
            queries: count("SELECT COUNT(*) FROM queries")?,
            first_ts: self
                .conn
                .query_row("SELECT MIN(ts) FROM text_chunks", [], |r| {
                    r.get::<_, Option<i64>>(0)
                })
                .unwrap_or(None),
            last_ts: self
                .conn
                .query_row("SELECT MAX(ts) FROM text_chunks", [], |r| {
                    r.get::<_, Option<i64>>(0)
                })
                .unwrap_or(None),
            // `image_path IS NOT NULL` 而不是全部的 frames：畫面被保留期
            // 清掉之後那一列還在（時間、標題、字都留著），但 `image_bytes`
            // 已經歸零，把它算進範圍會再一次讓分母比分子長。
            image_first_ts: self
                .conn
                .query_row(
                    "SELECT MIN(ts) FROM frames WHERE image_path IS NOT NULL",
                    [],
                    |r| r.get::<_, Option<i64>>(0),
                )
                .unwrap_or(None),
            image_last_ts: self
                .conn
                .query_row(
                    "SELECT MAX(ts) FROM frames WHERE image_path IS NOT NULL",
                    [],
                    |r| r.get::<_, Option<i64>>(0),
                )
                .unwrap_or(None),
            db_bytes: {
                let page_count: i64 = self
                    .conn
                    .pragma_query_value(None, "page_count", |r| r.get(0))
                    .unwrap_or(0);
                let page_size: i64 = self
                    .conn
                    .pragma_query_value(None, "page_size", |r| r.get(0))
                    .unwrap_or(0);
                // `page_count × page_size` 只是**主檔**，不含 `-wal`。而我們
                // 開的是 WAL 模式，WAL 正好是「一邊錄一邊長」的那一半：
                // checkpoint 之間寫進去的東西全在裡面。少算它，每一個磁碟數字
                // 都偏小，而那個數字是 Phase 0 退出條件的判決——偏的方向剛好
                // 是「看起來過了」。
                //
                // 路徑從 `PRAGMA database_list` 的第三欄拿。記憶體資料庫回空
                // 字串，那時候本來就沒有 WAL 檔可以量。
                let wal = self
                    .conn
                    .query_row("PRAGMA database_list", [], |r| r.get::<_, String>(2))
                    .ok()
                    .filter(|p| !p.is_empty())
                    .and_then(|p| std::fs::metadata(format!("{p}-wal")).ok())
                    .map_or(0, |m| m.len() as i64);
                page_count * page_size + wal
            },
        })
    }
}

fn map_l2_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<L2CardRow> {
    Ok(L2CardRow {
        id: row.get(0)?,
        segment_core_start: row.get(1)?,
        segment_ref: row.get(2)?,
        version: row.get(3)?,
        supersedes: row.get(4)?,
        activity: row.get(5)?,
        entities_json: row.get(6)?,
        continues_json: row.get(7)?,
        commitments_json: row.get(8)?,
        model_confidence: row.get(9)?,
        evidence_json: row.get(10)?,
        open_questions_json: row.get(11)?,
        created_at: row.get(12)?,
        author: L2Author::from_str_kind(&row.get::<_, String>(13)?)
            .unwrap_or(L2Author::Interpreter),
        tombstoned_at: row.get(14)?,
    })
}

fn map_commitment_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CommitmentRow> {
    Ok(CommitmentRow {
        id: row.get(0)?,
        text: row.get(1)?,
        kind: row.get(2)?,
        born_from: row.get(3)?,
        evidence_json: row.get(4)?,
        people_json: row.get(5)?,
        due_hint: row.get(6)?,
        due_source: row.get(7)?,
        due_at: row.get(8)?,
        status: row.get(9)?,
        confidence: row.get(10)?,
        allowed_next_step: row.get(11)?,
        last_evidence_seen_at: row.get(12)?,
        kill_note: row.get(13)?,
        created_at: row.get(14)?,
        updated_at: row.get(15)?,
        tombstoned_at: row.get(16)?,
    })
}

fn map_entity_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<EntityRow> {
    Ok(EntityRow {
        id: row.get(0)?,
        kind: row.get(1)?,
        name: row.get(2)?,
        aliases_json: row.get(3)?,
        first_seen_ref: row.get(4)?,
        notes: row.get(5)?,
        created_at: row.get(6)?,
        tombstoned_at: row.get(7)?,
    })
}

fn map_day_summary_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DaySummaryRow> {
    Ok(DaySummaryRow {
        id: row.get(0)?,
        date: row.get(1)?,
        version: row.get(2)?,
        supersedes: row.get(3)?,
        narrative: row.get(4)?,
        session_refs_json: row.get(5)?,
        stats_json: row.get(6)?,
        created_at: row.get(7)?,
        tombstoned_at: row.get(8)?,
    })
}

fn map_fact_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<FactRow> {
    Ok(FactRow {
        id: row.get(0)?,
        ts: row.get(1)?,
        kind: row.get(2)?,
        raw: row.get(3)?,
        normalized: row.get(4)?,
        source_kind: row.get(5)?,
        chunk_id: row.get(6)?,
        frame_id: row.get(7)?,
        app_id: row.get(8)?,
        window_title: row.get(9)?,
        url: row.get(10)?,
    })
}

#[allow(clippy::too_many_arguments)]
fn insert_chunk_tx(
    tx: &rusqlite::Transaction<'_>,
    session_id: i64,
    ts: Millis,
    source_kind: SourceKind,
    source_id: Option<i64>,
    frame_id: Option<i64>,
    focus: &FocusSnapshot,
    text: &str,
) -> rusqlite::Result<i64> {
    tx.execute(
        "INSERT INTO text_chunks(ts, session_id, source_kind, source_id, frame_id,
                                 app_id, window_title, url, text)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![
            ts,
            session_id,
            source_kind.as_str(),
            source_id,
            frame_id,
            focus.app_id,
            focus.window_title,
            focus.url,
            text
        ],
    )?;
    let id = tx.last_insert_rowid();

    // bigram 索引在 Rust 這邊寫，不掛 trigger——掛 trigger 就得註冊一個
    // 自訂 SQL 函式，而那個函式一旦在某條連線上沒註冊到，寫入會直接失敗。
    // 刪除那半仍然走 trigger（照 rowid 刪，不需要重算）。
    let grams = cjk_bigrams(text);
    if !grams.is_empty() {
        tx.execute(
            "INSERT INTO text_fts_bi(rowid, text) VALUES(?1, ?2)",
            params![id, grams],
        )?;
    }
    Ok(id)
}

#[allow(clippy::too_many_arguments)]
fn insert_facts_tx(
    tx: &rusqlite::Transaction<'_>,
    session_id: i64,
    ts: Millis,
    chunk_id: i64,
    frame_id: Option<i64>,
    source_kind: SourceKind,
    focus: &FocusSnapshot,
    facts: &[ExtractedFact],
) -> rusqlite::Result<usize> {
    if facts.is_empty() {
        return Ok(0);
    }
    let mut stmt = tx.prepare(
        "INSERT INTO facts(ts, session_id, kind, raw, normalized, source_kind,
                           chunk_id, frame_id, app_id, window_title, url, byte_start, byte_end)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
    )?;
    for f in facts {
        stmt.execute(params![
            ts,
            session_id,
            f.kind.as_str(),
            f.raw,
            f.normalized,
            source_kind.as_str(),
            chunk_id,
            frame_id,
            focus.app_id,
            focus.window_title,
            focus.url,
            f.byte_start as i64,
            f.byte_end as i64,
        ])?;
    }
    Ok(facts.len())
}

/// LIKE 掃描往回看多久。**這是一個能力上限，不是效能調校。**
///
/// 兩個字的中文詞在這個 schema 底下沒有索引可用：trigram 比不了 <3 字，
/// 而 unicode61 把「客服專線」整串當成**一個** token（不是逐字切），所以
/// MATCH "客服" 是 0 筆。剩下唯一找得到的方法就是掃全表。
///
/// 掃全表的成本跟你用了多久成正比。實測（2,073,600 行字 ≈ 一個月）：
/// 查「客服」104.6 ms，而 SPEC §8.2 的預算是 100 ms、文字保留期是 365 天。
/// 不設界的話，這個產品用得越久，最常見的中文查詢就越慢，而且沒有盡頭。
///
/// 所以掃描只往回看 30 天。代價是誠實的：**超過 30 天以前的資料，兩個字的
/// 中文查詢找不回來**（三個字以上照樣走 trigram，完全不受影響）。真正的解
/// 是補一個 bigram 索引，那是 Phase 1 的事——在那之前這個上限被寫進
/// DATA_INVENTORY 的已知缺口，而不是靠沒有人去量它來維持體面。
pub(crate) const LIKE_SCAN_DAYS: i64 = 30;

/// bigram 粗篩要多拿幾倍的候選再用真字串篩。倍率決定「篩掉偽陽性之後還能
/// 湊滿一頁」的把握有多大——太小會漏掉排在偽陽性後面的真命中，太大則是白
/// 讀。8 是「一頁 20 筆要 160 筆候選」，在 45 天語料上仍是 0.1 ms 級。
const BIGRAM_OVERFETCH: usize = 8;

/// LIKE 後援命中的固定分數。負值確保它永遠排在任何 FTS 命中之後——
/// 它證明「有這段文字」，但不宣稱相關性。
const LIKE_SCORE: f64 = -1.0;

fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3400..=0x4DBF      // 擴充 A
        | 0x4E00..=0x9FFF    // 基本區
        | 0xF900..=0xFAFF    // 相容表意文字
        | 0x20000..=0x2FA1F  // 擴充 B 之後
    )
}

/// 把 CJK 連續段切成重疊的雙字：「客服專線」→「客服 服專 專線」。
///
/// 兩個字的中文詞在 trigram 與 unicode61 之間有個縫：trigram 比不了 <3 字，
/// unicode61 又把整串 CJK 當成**一個** token。而中文最常見的查詢正好是兩個字
/// ——Phase 0 的退場條件寫的就是 `sister query 電話`。這個函式產生的字串
/// 存進 `text_fts_bi`，那個縫才有索引。
///
/// 非 CJK 完全跳過：英文與數字本來就有 unicode61 和 trigram 蓋著，
/// 再切一次只是把索引撐大。
fn cjk_bigrams(text: &str) -> String {
    let mut out = String::new();
    let mut run: Vec<char> = Vec::new();

    fn flush(run: &mut Vec<char>, out: &mut String) {
        for pair in run.windows(2) {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push(pair[0]);
            out.push(pair[1]);
        }
        run.clear();
    }

    for c in text.chars() {
        if is_cjk(c) {
            run.push(c);
        } else {
            flush(&mut run, &mut out);
        }
    }
    flush(&mut run, &mut out);
    out
}

/// 查詢字串 → bigram 索引的 MATCH 運算式。
///
/// `None` = 這個查詢用不上 bigram 索引（沒有任何長度 ≥2 的 CJK 詞），
/// 那就別去問它，直接走原本的路。
/// 三個索引裡有沒有**任何一個看得到這一題**（不管找不找得到東西）。
///
/// 這一支在的理由是一則假警報。`scan_horizon_days` 以前問的是
/// `bigram_query(query).is_some()`，而那一支對**任何沒有相鄰 CJK 雙字的
/// 查詢**都回 `None`——包括每一個純英數的查詢。於是一顆存滿一年的資料庫，
/// 查 `ERR_CONNECTION_REFUSED` 查不到，答案是
///
/// ```text
/// 沒有找到。
/// 她翻過的那幾段裡沒有這個字。
/// ——但這種查法…沒有索引可用，只能掃最近 30 天…多打一個字就走得到索引。
/// ```
///
/// 三句話，三個錯。trigram 索引（`text_fts`）蓋整張表、沒有時間界線，這一題
/// 它是**全部 365 天都看過了**的；那個「沒有」是完整的、可以信的。而那句
/// 「多打一個字」對一個 21 個字元的錯誤碼不是建議，是雜訊。
///
/// 這是那個欄位當初要修的東西的**反面**：一個真的、完整的「找不到」被降級
/// 成「我只讀了十二分之一」。而使用者對這兩句話的反應完全相反——一句是
/// 「那就是沒發生」，一句是「再翻遠一點」。
///
/// 判準跟著 `search` 那段派工走：
///
/// - trigram 比的是子字串，但每個詞要 ≥3 個字元。`fts_query` 用 `AND` 串詞，
///   所以只要有一個詞太短，整個 MATCH 就是空的——「每一個詞都夠長」才算數。
/// - bigram（[`cjk_bigrams`]）蓋長度 ≥2 的中文詞。
/// - 兩個都不行才落到 [`Db::search_like`]，而那條路夾在 [`LIKE_SCAN_DAYS`] 天內。
///
/// `text_fts_uni`（unicode61）刻意不算：它比的是**整個詞**，所以「80」找得到
/// 一個單獨的 `80`，卻找不到藏在 `0800` 裡的那個——而後者正是他會來問的那種。
/// 少算它會讓她偶爾多講一句「更早的沒翻到」，多算它會讓她把一次半盲說成看完
/// 了。這兩種錯的代價不對稱。
fn covered_by_index(query: &str) -> bool {
    let mut terms = query.split_whitespace().peekable();
    let trigram = terms.peek().is_some() && terms.all(|t| t.chars().count() >= 3);
    trigram || bigram_query(query).is_some()
}

fn bigram_query(query: &str) -> Option<String> {
    let parts: Vec<String> = query
        .split_whitespace()
        .map(cjk_bigrams)
        .filter(|g| !g.is_empty())
        .map(|g| {
            g.split(' ')
                .map(|gram| format!("\"{gram}\""))
                .collect::<Vec<_>>()
                .join(" AND ")
        })
        .collect();
    (!parts.is_empty()).then(|| parts.join(" AND "))
}

/// 手工產生命中片段，格式與 FTS5 的 `snippet()` 一致（`[` `]` 標記、`…` 省略）。
fn make_snippet(text: &str, needle: &str) -> String {
    const CONTEXT: usize = 30; // 前後各留幾個字

    let pos = text.find(needle).or_else(|| {
        // 大小寫不敏感的退路：僅在小寫化不改變位元組長度時才敢用其偏移量
        let lower = text.to_lowercase();
        (lower.len() == text.len())
            .then(|| lower.find(&needle.to_lowercase()))
            .flatten()
    });

    let head = || text.chars().take(CONTEXT * 2).collect::<String>();
    let Some(pos) = pos else { return head() };
    let hit_end = pos + needle.len();
    if !text.is_char_boundary(pos) || !text.is_char_boundary(hit_end) {
        return head();
    }

    let before_all = &text[..pos];
    let after_all = &text[hit_end..];
    let before: String = {
        let mut v: Vec<char> = before_all.chars().rev().take(CONTEXT).collect();
        v.reverse();
        v.into_iter().collect()
    };
    let after: String = after_all.chars().take(CONTEXT).collect();

    let lead = if before.chars().count() < before_all.chars().count() {
        "…"
    } else {
        ""
    };
    let trail = if after.chars().count() < after_all.chars().count() {
        "…"
    } else {
        ""
    };

    format!("{lead}{before}[{}]{after}{trail}", &text[pos..hit_end])
}

/// 把使用者的自然查詢轉成 FTS5 語法。
///
/// 一律加雙引號當 phrase 處理：使用者打的是中文句子，不是布林運算式，
/// 而 FTS5 的 `-`、`*`、`OR`、`NEAR` 若不跳脫會變成語法錯誤或意外語意。
pub fn fts_query(input: &str) -> String {
    let terms: Vec<String> = input
        .split_whitespace()
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
        .collect();
    terms.join(" AND ")
}

#[derive(Debug, Clone, PartialEq)]
pub struct FactRow {
    pub id: i64,
    pub ts: Millis,
    pub kind: String,
    pub raw: String,
    pub normalized: String,
    pub source_kind: String,
    pub chunk_id: Option<i64>,
    pub frame_id: Option<i64>,
    pub app_id: Option<String>,
    pub window_title: Option<String>,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum L2Author {
    Interpreter,
    Reviewer,
    User,
}

impl L2Author {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Interpreter => "interpreter",
            Self::Reviewer => "reviewer",
            Self::User => "user",
        }
    }

    pub fn from_str_kind(s: &str) -> Option<Self> {
        match s {
            "interpreter" => Some(Self::Interpreter),
            "reviewer" => Some(Self::Reviewer),
            "user" => Some(Self::User),
            _ => None,
        }
    }

    pub fn confidence_source(self) -> &'static str {
        match self {
            Self::Interpreter => "model",
            Self::Reviewer => "reviewer",
            Self::User => "user",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct L2CardRow {
    pub id: i64,
    pub segment_core_start: Millis,
    pub segment_ref: String,
    pub version: i32,
    pub supersedes: Option<i64>,
    pub activity: String,
    pub entities_json: String,
    pub continues_json: Option<String>,
    pub commitments_json: String,
    pub model_confidence: f64,
    pub evidence_json: String,
    pub open_questions_json: String,
    pub created_at: Millis,
    pub author: L2Author,
    pub tombstoned_at: Option<Millis>,
}

pub struct L2Insert<'a> {
    pub segment_core_start: Millis,
    pub segment_ref: &'a str,
    pub activity: &'a str,
    pub entities_json: String,
    pub continues_json: Option<String>,
    pub commitments_json: String,
    pub model_confidence: f64,
    pub evidence_json: String,
    pub open_questions_json: String,
    pub author: L2Author,
}

pub struct OutboundInsert<'a> {
    pub ts: Millis,
    pub day_key: &'a str,
    pub command: &'a str,
    pub args: &'a [String],
    pub segment_core_start: Option<Millis>,
    pub chars_sent: i64,
    pub truncated: bool,
    pub outcome: &'a str,
    pub duration_ms: i64,
    pub error: Option<&'a str>,
    pub role: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct L0Original {
    pub r#ref: String,
    pub kind: &'static str,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TombstoneReport {
    pub l2: u64,
    pub commitments: u64,
    pub mentions: u64,
    pub entities: u64,
    pub day_summaries: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecheckStats {
    /// `None` = 審閱層一次都還沒跑成。`Some(0)` 不會出現——0 次就是沒跑過。
    pub runs: Option<u32>,
    /// `None` = 還沒跑過。`Some(0)` = 跑了，沒有五類候選。
    pub candidates: Option<u32>,
    /// `None` = 還沒跑過。`Some(0)` = 跑了但一次都沒回查原件。
    pub rechecks: Option<u32>,
    /// 若還沒跑成，最近一次為什麼停。
    pub last_skip: Option<String>,
}

pub struct CommitmentInsert<'a> {
    pub text: &'a str,
    pub kind: &'a str,
    pub born_from: i64,
    pub evidence_json: String,
    pub people_json: String,
    pub due_hint: Option<&'a str>,
    pub due_source: Option<&'a str>,
    pub due_at: Option<Millis>,
    pub status: &'a str,
    pub confidence: f64,
    pub allowed_next_step: Option<&'a str>,
    pub last_evidence_seen_at: Option<Millis>,
    pub kill_note: Option<&'a str>,
    pub now: Millis,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CommitmentRow {
    pub id: i64,
    pub text: String,
    pub kind: String,
    pub born_from: i64,
    pub evidence_json: String,
    pub people_json: String,
    pub due_hint: Option<String>,
    pub due_source: Option<String>,
    pub due_at: Option<Millis>,
    pub status: String,
    pub confidence: f64,
    pub allowed_next_step: Option<String>,
    pub last_evidence_seen_at: Option<Millis>,
    pub kill_note: Option<String>,
    pub created_at: Millis,
    pub updated_at: Millis,
    pub tombstoned_at: Option<Millis>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityRow {
    pub id: i64,
    pub kind: String,
    pub name: String,
    pub aliases_json: String,
    pub first_seen_ref: String,
    pub notes: Option<String>,
    pub created_at: Millis,
    pub tombstoned_at: Option<Millis>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MentionRow {
    pub id: i64,
    pub entity_id: i64,
    pub seen_ref: String,
    pub created_at: Millis,
    pub tombstoned_at: Option<Millis>,
}

pub struct DaySummaryInsert<'a> {
    pub date: &'a str,
    pub narrative: &'a str,
    pub session_refs_json: String,
    pub stats_json: String,
    pub now: Millis,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaySummaryRow {
    pub id: i64,
    pub date: String,
    pub version: i32,
    pub supersedes: Option<i64>,
    pub narrative: String,
    pub session_refs_json: String,
    pub stats_json: String,
    pub created_at: Millis,
    pub tombstoned_at: Option<Millis>,
}

/// 日摘要的一段。`l2_id` 有值才點得回「她猜的」那張卡片。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DaySummaryClause {
    pub text: String,
    pub l2_id: Option<i64>,
}

/// 時間軸上的日摘要。四個變體，沒有「沒有」這一個含糊值。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind")]
pub enum DaySummaryGlance {
    #[serde(rename = "live")]
    Live {
        date: String,
        version: i32,
        supersedes: Option<i64>,
        created_at: Millis,
        clauses: Vec<DaySummaryClause>,
        /// `narrative` 用 `；` 切開的段數，和 `session_refs` 的長度相不相等。
        /// 不相等就不配對——配錯比不配更糟。
        aligned: bool,
        /// `stats_json` 裡的 `l2`。沒有這個鍵就 `None`，不要印成 0。
        l2: Option<u64>,
        /// `stats_json` 裡的 `commitments_open`。沒有這個鍵就 `None`。
        commitments_open: Option<u64>,
    },
    #[serde(rename = "tombstoned")]
    Tombstoned { date: String, tombstoned_at: Millis },
    #[serde(rename = "eod_empty")]
    EodEmpty { date: String },
    #[serde(rename = "never_ran")]
    NeverRan { date: String },
}

/// `write_day_summary` 用 `activities.join("；")` 和同順序的 `l2:{id}` 組成。
/// 對得上才接成可點的一段；對不上就不接。
fn clauses_from(narrative: &str, session_refs_json: &str) -> (Vec<DaySummaryClause>, bool) {
    let refs: Vec<String> = serde_json::from_str(session_refs_json).unwrap_or_default();
    let parts: Vec<&str> = if narrative.is_empty() {
        Vec::new()
    } else {
        narrative.split('；').collect()
    };
    let aligned = parts.len() == refs.len();
    if aligned {
        let clauses = parts
            .into_iter()
            .zip(refs)
            .map(|(text, r)| DaySummaryClause {
                text: text.to_string(),
                l2_id: parse_l2_ref(&r),
            })
            .collect();
        (clauses, true)
    } else {
        let clauses = parts
            .into_iter()
            .map(|text| DaySummaryClause {
                text: text.to_string(),
                l2_id: None,
            })
            .collect();
        (clauses, false)
    }
}

fn parse_l2_ref(s: &str) -> Option<i64> {
    s.strip_prefix("l2:")?.parse().ok()
}

/// 缺鍵不是 0。解析失敗也不是 0。
fn parse_day_stats(json: &str) -> (Option<u64>, Option<u64>) {
    let v: serde_json::Value = match serde_json::from_str(json) {
        Ok(v) => v,
        Err(_) => return (None, None),
    };
    let as_u64 = |key: &str| {
        v.get(key).and_then(|x| {
            x.as_u64()
                .or_else(|| x.as_i64().and_then(|n| u64::try_from(n).ok()))
        })
    };
    (as_u64("l2"), as_u64("commitments_open"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreferenceRow {
    pub key: String,
    pub value: String,
    pub learned_from: String,
    pub updated_at: Millis,
}

pub struct ReviewerRunInsert<'a> {
    pub ts: Millis,
    pub day_key: &'a str,
    pub kind: &'a str,
    pub skip_reason: Option<&'a str>,
    pub candidate_count: Option<i64>,
    pub recheck_count: Option<i64>,
    pub wrote_commitments: i64,
    pub divergences: i64,
    pub calls_used: i64,
    pub budget_used: i64,
    pub budget_limit: i64,
    pub detail: &'a str,
}

pub struct RecheckInsert<'a> {
    pub run_id: i64,
    pub category: &'a str,
    pub child_ref: &'a str,
    pub parent_ref: &'a str,
    pub original_present: bool,
    pub original_chars: Option<i64>,
    pub matched: bool,
}

pub struct DivergenceInsert<'a> {
    pub run_id: i64,
    pub subject: &'a str,
    pub pass_a_json: &'a str,
    pub pass_b_json: &'a str,
    pub reason: &'a str,
    pub created_at: Millis,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DivergenceRow {
    pub id: i64,
    pub run_id: i64,
    pub subject: String,
    pub pass_a_json: String,
    pub pass_b_json: String,
    pub reason: String,
    pub created_at: Millis,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboundRow {
    pub id: i64,
    pub ts: Millis,
    pub day_key: String,
    pub command: String,
    pub args_json: String,
    pub segment_core_start: Option<Millis>,
    pub chars_sent: i64,
    pub truncated: bool,
    pub outcome: String,
    pub duration_ms: i64,
    pub error: Option<String>,
    pub role: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkipRow {
    pub id: i64,
    pub ts: Millis,
    pub reason: String,
    pub segment_core_start: Option<Millis>,
    pub detail: String,
}

/// 要記進題庫的一次提問（見 [`Db::log_query`]）。
///
/// 借用而不是持有，因為呼叫端已經有這些字串了，而這一步在使用者按下 Enter 之後
/// 的那條路上——不值得為了記一筆而多配置幾次記憶體。
pub struct QueryLogEntry<'a> {
    pub ts: Millis,
    /// 他打的**原話**。不做正規化：題庫要的正是真實的用詞。
    pub question: &'a str,
    /// `"recent"`／`"keywords"`／`"range"`。走哪條路本身就是一個要驗的判斷
    /// （見 [`crate::question::shape`]）。
    pub shape: &'a str,
    /// 她一共**給了他幾筆東西**——不是 [`Db::search`] 回了幾筆。
    ///
    /// CLI 的答案有兩層：★ 事實（L1）和原文（全文比對）。`sister query 電話`
    /// 的號碼來自前者，後者是 0 筆；只數後者的話，這個產品最典型的一次成功
    /// 會被記成「一筆都沒找到」。這個欄位唯一的用途是分辨她答不答得出來，
    /// 所以它數的是使用者看到了什麼。
    pub hits: usize,
    pub latency_ms: i64,
    /// [`SOURCE_DESKTOP`]／[`SOURCE_CLI`]。同一個問題從兩個地方問應該給一樣的
    /// 答案，而分不出來源的話，這件事就查不了。
    pub source: &'a str,
}

/// 字母人。**出處點得動的只有這裡。**
///
/// 是常數不是字面值，因為現在有東西在讀它了（[`QueryLogStats::clickable`]），
/// 而寫的那一邊在另一個 crate 裡。打錯一個字不會編不過，只會讓那一題安靜地
/// 落到分母外面。
pub const SOURCE_DESKTOP: &str = "desktop";
/// 終端機。沒有出處可以點。
pub const SOURCE_CLI: &str = "cli";

/// 題庫裡的一列（見 [`Db::query_log`]）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryRow {
    pub id: i64,
    pub ts: Millis,
    pub question: String,
    pub shape: String,
    pub hits: i64,
    pub latency_ms: i64,
    pub source: String,
    /// 這一題有幾個出處被點開過。0 = 她給了答案，但沒有一筆值得點。
    pub clicks: i64,
    /// 他自己按下「這一題我本來已經忘了」的**那一刻**；沒按過就是 `None`
    /// （見 [`MIGRATION_007`]）。
    ///
    /// **和 `clicks` 是兩件事，不准互相代表。** 點開出處說的是「我去看了證
    /// 據」，這一格說的是「我本來不知道」——前者在她答錯的時候最常發生。
    ///
    /// 存時間而不是存一個 `bool`，是因為上一版**同一件事有兩個代表**：`bool`
    /// 走 `EXISTS`，時間走另一句 `JOIN`，於是「幾次」和「哪幾題」各自去問了一
    /// 次資料庫，答得出兩個不一樣的數字。少一個欄位就少一次對不上的機會——
    /// 「有沒有標」現在是 [`QueryRow::marked`]，從同一格算出來的。
    pub marked_ts: Option<Millis>,
}

impl QueryRow {
    /// 他標記過這一題沒有。就是 `marked_ts.is_some()`——不是另一個獨立的答案。
    pub fn marked(&self) -> bool {
        self.marked_ts.is_some()
    }
}

/// 「他標記過的那幾題」在 SQL 裡的唯一定義。
///
/// 計數（[`QueryLogStats::marked`]）和清單（[`Db::marked_queries`]）都從這裡
/// 長出來。上一版計數是 `COUNT(*) FROM query_marks`（不 join），清單是
/// `query_marks JOIN queries`（inner join）——一列孤兒標記就讓「★ 魔法時刻：
/// 1 次」配著一張空清單，而那句「撈不回題目就讓它炸」的保險因為 inner join
/// 早就濾掉了它，永遠不會響。
const MARKED_PREDICATE: &str = "EXISTS(SELECT 1 FROM query_marks m WHERE m.query_id = q.id)";

/// [`Db::query_log`]、[`Db::query_by_id`]、[`Db::marked_queries`] 共用的那一句
/// `SELECT`。
///
/// 一份，不是三份。這裡的欄位順序是 [`read_query_row`] 的契約，兩個一起改。
fn query_log_sql(whr: &str, tail: &str) -> String {
    format!(
        "SELECT q.id, q.ts, q.question, q.shape, q.hits, q.latency_ms, q.source,
                (SELECT COUNT(*) FROM query_clicks c WHERE c.query_id = q.id),
                (SELECT m.ts FROM query_marks m WHERE m.query_id = q.id)
         FROM queries q WHERE {whr} {tail}"
    )
}

/// [`query_log_sql`] 那九欄的讀法。
fn read_query_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<QueryRow> {
    Ok(QueryRow {
        id: r.get(0)?,
        ts: r.get(1)?,
        question: r.get(2)?,
        shape: r.get(3)?,
        hits: r.get(4)?,
        latency_ms: r.get(5)?,
        source: r.get(6)?,
        clicks: r.get(7)?,
        marked_ts: r.get(8)?,
    })
}

/// [`Db::mark_query`] 做完之後的兩件事。
///
/// 分成兩格，因為兩個呼叫端問的不是同一題：字母人那顆按鈕要的是「現在該畫成
/// 什麼樣子」（`marked`），而終端機要的是「剛剛到底有沒有發生事情」
/// （`changed`）——後者少了就會對著一個沒標過的題號說「收回了」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarkOutcome {
    /// 這一題現在標著沒有。等於傳進去的那個參數——呼叫成功就代表狀態已經是它。
    pub marked: bool,
    /// 這一次有沒有真的動到那張表。重按同一個方向是 `false`。
    pub changed: bool,
}

/// 「零當機」那一格的兩組數字（見 [`Db::crash_audit`]、[`migration_006`]）。
///
/// 兩組，因為它們各自答得出對方答不出的一半：
///
/// * `started` / `ended` 是 `meta` 裡的計數器，**撐得過那幾列被刪掉**，
///   所以「開機就死」那一種當機逃不掉。代價是它們沒有時間。
/// * `rows` / `rows_unfinished` / `last_crash` 是 `sessions` 那張表，
///   **帶得出時間**，代價是被 `forget`、保留期、和空場清除掃過。
///
/// 把它們接起來的是差額：`started - rows` 是**一場紀錄都沒留下的那幾場**，
/// 而那個數字大於 0 本身就是一句話——她跑過，然後那幾場什麼都沒留下。
///
/// **每一個欄位都已經不含「現在正在錄的那一場」**（見 [`Db::crash_audit`] 的
/// 「為什麼 `occupied` 收在這裡」）。所以這裡沒有任何一支函式再收 `occupied`
/// ——呼叫端連那個布林都拿不到，也就沒有東西可以只扣一半。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrashAudit {
    /// 她開過幾場（不含正在錄的那一場）。單調，`forget` 和保留期都不碰。
    pub started: i64,
    /// 她好好收尾過幾場。
    pub ended: i64,
    /// 還留著紀錄的有幾場（不含正在錄的那一場）。
    pub rows: i64,
    /// 還留著紀錄、沒有 `ended_at`、而且**不是正在錄的那一場**的有幾場。
    pub rows_unfinished: i64,
    /// 還留著紀錄的那幾場裡，最後一場沒收尾的是什麼時候開始的。
    ///
    /// 沒有留下紀錄的那幾場**沒有時間可以報**，而且是故意的：一個「她那天
    /// 幾點幾分當過」的時間戳，正是 `delete_empty_sessions` 刪掉那一列要拿掉
    /// 的東西。數字撐過清空是可以的，時間不行。
    pub last_crash: Option<Millis>,
    /// 此刻真的有一場在錄，**而且它的那一列已經在資料庫裡**。
    ///
    /// 上面每一個數字都已經把她扣掉了，這個欄位只剩一個用途：讓句子講出「扣
    /// 掉了」。心跳在、列還沒進去的那幾分鐘是 `false`——那時候她一個位置都
    /// 不佔，扣掉她會扣掉別人的一次當機。
    pub live: bool,
    /// 心跳原封不動。
    ///
    /// `live` 是「她的那一列在不在」，這一顆是「有沒有人佔著這個目錄」——開機
    /// 那一段兩者相反，而**兩句話都有人要印**：「零當機」要扣的是前者，「上
    /// 一次錄製」要講的是後者（那一列是當掉的，可是現在真的有一個 recorder
    /// 正在起來）。少了這一顆，那一列只能在「她現在還在跑」和「現在沒有任何
    /// recorder 佔著這個資料目錄」之間挑一句，而開機那幾分鐘兩句都是假的。
    pub beat: Option<crate::heartbeat::Phase>,
    /// `started` / `ended` 是不是只是一個下限（升級那天回填的，見
    /// [`migration_006`]）。
    pub floor: bool,
}

impl CrashAudit {
    /// 沒有回來的那幾場。
    ///
    /// 正在錄的那一場已經在 [`Db::crash_audit`] 裡扣掉了——它也沒有
    /// `ended_at`，在磁碟上跟當機長得一模一樣，而心跳分得出來。
    ///
    /// 還是夾在 0：計數器讀不到的時候（有人手動刪 `meta`）`started` 是 0 而
    /// `ended` 可能不是，負的當機數是一個數不出來的東西。
    pub fn crashed(&self) -> i64 {
        (self.started - self.ended).max(0)
    }

    /// 跑過、而且**一列紀錄都沒留下**的那幾場。
    ///
    /// 大於 0 的兩種讀法，這裡分不出來，句子也不會假裝分得出來：那幾場什麼
    /// 都沒存到（`capture.enabled = false`、開機即死），或者他按過 `forget`。
    pub fn traceless(&self) -> i64 {
        (self.started - self.rows).max(0)
    }
}

/// 最後一場錄製（見 [`Db::last_session`]）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastSession {
    pub started_at: Millis,
    /// `None` = 沒有好好結束：當掉、被砍，或者**現在正在錄**。
    pub ended_at: Option<Millis>,
    /// [`crate::model::EndReason::as_str`] 存下來的那個字串。
    ///
    /// 存字串而不是 enum：舊版本寫下的紀錄要讀得出來，而一個讀不懂的理由
    /// 應該原樣印出去（[`crate::model::EndReason::describe`] 就是這樣做的），
    /// 不是被當成「沒有理由」吞掉。
    pub reason: Option<String>,
    /// 錄那一場的執行檔版本。`sessions.app_version`。
    pub app_version: String,
    /// 那一場**還留著**幾筆 `system_events`。
    ///
    /// [`reason`](Self::reason) 是 `None` 的時候，這個數字是唯一分得出兩件事
    /// 的證據：`sessions` 這張表永遠不會被刪，但 `system_events` 會（保留期
    /// 和 `sister forget` 都刪）。所以「沒有理由」有兩種——
    ///
    /// * `events_left > 0`：那一場的事件還在，就是沒有 `session_end`。
    ///   那一版執行檔（alpha.17 以前）真的沒有在記為什麼停。
    /// * `events_left == 0`：整場的事件都不見了。理由本來很可能寫了，是
    ///   後來被清掉的——說成「那一版還沒有在記」是把帳算到錯的地方。
    ///
    /// 而這一行的全部意義就是分辨「你按了停止」/「她當掉了」/「同意書被
    /// 撤回」，所以它特別不該把原因報成一個假的「那時候還沒這功能」。
    pub events_left: i64,
}

/// 檢索延遲的預算，毫秒。
///
/// 來自 PHASES.md Phase 1 的退場條件「檢索 < 100ms」。那條檢查框在這之前
/// 沒有任何東西量得出來——而題庫從第一天起就在存每一題花了幾毫秒，只是
/// 沒有人讀。**一個沒有人讀的數字，等於沒有那個數字。**
pub const RETRIEVAL_BUDGET_MS: i64 = 100;

/// 題庫的總覽（見 [`Db::query_log_stats`]）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QueryLogStats {
    pub total: i64,
    /// 一筆都沒找到的題數。**這是最該看的那個數字。**
    pub empty: i64,
    /// 至少有一個出處被點開的題數。
    pub clicked: i64,
    /// 其中**點得動出處**的題數——也就是從字母人問的那幾題。
    ///
    /// [`Self::clicked`] 的分母只能是這個數字，不能是 [`Self::total`]。終端機
    /// 上沒有出處可以點，所以每一題從終端機問的問題都會讓那個百分比往下掉，
    /// 而掉出來的「0%」講的是介面，不是檢索品質。開發的時候 `sister query`
    /// 跑幾十次是常態，於是這個數字結構性地永遠是 0——一個永遠是 0 的指標
    /// 不會有人再看第二眼，而它本來是檢索品質唯一不用人工標註的訊號。
    pub clickable: i64,
    pub first_ts: Option<Millis>,
    pub last_ts: Option<Millis>,
    /// 超過 [`RETRIEVAL_BUDGET_MS`] 的題數。
    pub slow: i64,
    /// 延遲的中位數，毫秒。「平常有多快」。
    pub p50_ms: i64,
    /// 最慢的 5% 從哪裡開始，毫秒。「最糟的時候有多糟」。
    ///
    /// 中位數會把偶爾一次 4 秒的卡頓藏起來，而那一次正是使用者記得的那一次。
    pub p95_ms: i64,
    /// 他自己標記成「這一題我本來已經忘了」的題數（見 [`MIGRATION_007`]）。
    ///
    /// PHASES.md Phase 1 的第一條退場條件量的就是它。**這是整份總覽裡唯一一個
    /// 不是量出來的數字**——其他每一格都是她自己觀察到的，這一格是他按下去的。
    /// 所以它也是唯一一個「0」不代表壞掉的：他可能沒按過，也可能她真的沒神奇
    /// 過，而這兩件事這裡分不出來，句子也就不准替他選一個。
    pub marked: i64,
}

/// 秘密遮蔽的實際結果（見 [`Db::redaction_audit`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RedactionAudit {
    /// 被判定為疑似秘密的剪貼簿事件數。
    pub flagged: i64,
    /// 其中內容**仍然留在資料庫裡**的筆數。任何大於 0 的值都是遮蔽失效。
    pub leaked: i64,
}

/// 一個訊號的三種下場。**三種，不是兩種。**
///
/// 這裡本來是一個 `broken: bool`，於是「驗過了，是好的」和「資料還太少，看
/// 不出來」印出同一個 ✓。那正是這個稽核自己要抓的形狀，出現在抓它的那支
/// 工具上：一台剛開機三秒的機器，兩列焦點事件都不知道自己是哪個 app，
/// doctor 說一切正常——而三分鐘後它會說同一句話，那時候是真的。
///
/// 下限（`ENOUGH_TO_BE_SURE`）是在縮到「最後一場」之後才需要的：舊版掃全表，
/// 靠的是幾個月的量體，而那個量體正是它永遠翻不成 ✗ 的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalVerdict {
    /// 有內容，這個訊號是活的。
    Alive,
    /// 攢夠了列、而一列都沒有內容：這個狀態自相矛盾，不是「使用者很安靜」。
    ///
    /// 刻意不用「populated 佔比很低」當條件。低佔比在真實使用中隨時會發生
    /// （整個下午都在看影片、沒碰鍵盤），拿它報警等於製造一個大家學會忽略的
    /// 警告。這裡只認**不可能同時成立**的組合。
    Broken,
    /// 一列有內容的都沒有，但列數還沒攢到下限。不知道就說不知道。
    TooEarly,
}

impl SignalVerdict {
    /// 給 `--json` 用的名字。畫面那幾句話不從這裡長出來——那是各介面自己的事。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Alive => "alive",
            Self::Broken => "broken",
            Self::TooEarly => "too_early",
        }
    }
}

/// 一種存下來的訊號，現在到底是有內容還是空殼（見 [`Db::signal_audit`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignalAudit {
    pub name: &'static str,
    /// 存了幾列。
    pub rows: i64,
    /// 其中「真的有內容」的有多少——每個訊號自己定義那是什麼意思。
    pub populated: i64,
    /// `populated` 數的是什麼，用來湊出那一句話。
    ///
    /// 沒有這個欄位的第一版把三個訊號印成同一句「其中 N 列有內容」，而文字
    /// 座標的 N 是**有幾個不同的高度**、不是列數：36 個方框散在 5 個高度，
    /// 印出來變成「36 列裡有 5 列有內容」。三個數字各自都對，只是其中兩個
    /// 被安上了第三個的名字——跟排除稽核那次把「段」說成「張」是同一個錯誤，
    /// 隔了幾天又犯一次，所以這次把單位釘在資料旁邊而不是印的時候現編。
    pub populated_label: &'static str,
    /// 這個訊號現在到底是好的、壞的，還是**還說不準**。
    pub verdict: SignalVerdict,
    /// 為什麼那個組合不可能——警告要能解釋自己。
    pub note: &'static str,
    /// 上面那幾個數字描述的是**哪一段**：最後一場錄製的起點。
    ///
    /// `None` = 她一場都還沒錄過。
    ///
    /// 這一欄要跟著數字走，因為沒有它的話畫面說不出「12 列」是剛才那一場的
    /// 還是三個月前的——而這三個判斷的全部意義就是「現在」。舊版掃全表，
    /// 於是那三個 ✓ 講的是「這台機器**曾經**好過」，在一顆用了三個月的資料庫
    /// 上永遠翻不成 ✗。詳見 [`Db::signal_audit`]。
    pub scope_started_at: Option<Millis>,
    /// 那一段**就是現在正在錄的這一場**。
    ///
    /// 同一份 doctor 報告裡，「零當機」那一列的分母把正在錄的那一場扣掉了
    /// （見 [`CrashAudit`]），而這三列數的正好是被扣掉的那一場——然後叫它
    /// 「上一場」。問「那 2 場裡哪一場是上一場」，答案是「都不是」。
    ///
    /// 這個位元算在這裡、不算在印字的那一邊，是因為算在那邊就會變成「一個
    /// 新位元只餵給一起印出來的其中一個數字」——這批 bug 從第十次開始每一
    /// 次都是這個形狀。
    pub scope_is_live: bool,
}

/// 一條排除規則生效過的紀錄（見 [`Db::exclusion_audit`]）。
#[derive(Debug, Clone, PartialEq)]
pub struct ExclusionAudit {
    pub reason: String,
    /// 進入這個排除狀態的**次數**，不是被擋掉的畫面張數。
    pub episodes: i64,
    /// 第一段與最後一段**開始**的時間。段的結束沒有被記錄。
    pub first_ts: Millis,
    pub last_ts: Millis,
}

/// 某一天她記了多少。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaySummary {
    /// 這一天在**本機時區**的午夜，epoch 毫秒。
    pub start_ts: Millis,
    pub chunks: i64,
    /// 這一天最早／最晚的那一筆。用來畫「她那天從幾點看到幾點」。
    pub first_ts: Millis,
    pub last_ts: Millis,
}

/// 時間軸上的一格。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Moment {
    pub ts: Millis,
    pub app: Option<String>,
    pub title: Option<String>,
    pub url: Option<String>,
    pub text: String,
    /// `None` = 字還在，但那張圖已經過了保留期。
    pub frame_id: Option<i64>,
}

/// 時間軸上她閉眼的一段（見 [`Db::pause_spans`]）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PauseSpan {
    /// `None` = 開頭那筆 `pause` 被保留期刪掉了，只知道它在 `to` 之前。
    pub from: Option<Millis>,
    /// `None` = 到這顆資料庫的最後一刻都還在暫停。
    pub to: Option<Millis>,
}

/// 她被使用者叫去閉眼的紀錄。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PauseAudit {
    /// 暫停過幾段（含還沒結束的那一段）。
    pub episodes: i64,
    /// **已經結束的**那幾段加起來多久。不含 `open_since` 那一段——
    /// 那一段還在跑，把「到現在為止」算進來會讓同一份資料庫每次查都不一樣。
    pub total_ms: i64,
    /// 最後一段還沒有 `resume`：現在仍在暫停，或上次錄製在暫停中結束。
    pub open_since: Option<Millis>,
    /// 開頭被保留期刪掉、只剩 `resume` 的段數。這幾段有算進 `episodes`，
    /// 但**沒有**算進 `total_ms`——所以 `total_ms` 是下限，不是精確值。
    pub truncated: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FrameContext {
    pub frame_id: i64,
    pub ts: Millis,
    pub app_id: Option<String>,
    pub window_title: Option<String>,
    pub url: Option<String>,
    pub image_path: Option<String>,
    pub dup_run: i64,
    pub width: i64,
    pub height: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DbStats {
    pub frames: i64,
    /// 被去重折疊掉的幀數——這個數字越大，代表去重省下越多。
    pub frames_collapsed: i64,
    /// [`frames`](Self::frames) 裡真的有一張圖躺在硬碟上的那幾筆。
    ///
    /// 兩個數字不一樣是**正常的**，不是壞掉：第三張同意書沒簽（或者
    /// `store_images = false`）的時候她照樣一幀一幀地記，只是只記上面的字。
    /// 少了這個欄位，「4 張保留」配上「畫面檔 0 B」看起來就像一個 bug，而它
    /// 其實是使用者自己選的隱私模式正在生效——分不出這兩件事的報告，會讓人
    /// 去修一個沒有壞的東西，或者放過一個真的壞了的。
    pub frames_with_image: i64,
    pub image_bytes: i64,
    pub ocr_blocks: i64,
    pub chunks: i64,
    pub facts: i64,
    pub focus_events: i64,
    pub clipboard_events: i64,
    pub input_windows: i64,
    /// `system_events` **整張表**，含那場錄製自己的開始／結束那兩列。
    ///
    /// 保持「整張表」不變是刻意的：這個欄位有好幾個讀的人（`stats` 的
    /// 「系統 N」、匯出、足跡），把它的意思偷偷換成「扣掉標籤之後的」，每一
    /// 個讀的人都會跟著變意思，而解構那道 E0027 只擋得住**新欄位**，擋不住
    /// 一個舊欄位改了定義。要問「她記下來的還剩不剩」的人請減掉
    /// [`session_marks`](Self::session_marks)。
    pub system_events: i64,
    /// 上面那個數字裡，有幾列講的是**那場錄製本身**（`session_start` /
    /// `session_end`，見 [`SystemKind::is_session_mark`]）。
    ///
    /// 存在的唯一理由是 [`nothing_recorded_left`](Self::nothing_recorded_left)
    /// 減得出來。她**下一次開始錄**寫的那一列 `session_start`，本來足以讓那個
    /// 述詞翻成 false——他一秒前才親手清空的一整天，被一列開場標籤否認掉。
    /// 和 `queries` 同一種錯：**這張表會在清空之後長出來**。
    pub session_marks: i64,
    /// **還留著東西的**那幾場錄製——**外加最後那一場，如果它沒有正常收尾。**
    ///
    /// 不是「這台機器一共錄過幾場」。一場錄製的紀錄基本上活得和它記下來的東西
    /// 一樣久（見 `retention::delete_empty_sessions`）。以前它數的是整張表
    /// ——那張表誰都不刪——於是一顆剛被 `sister forget` 清空的資料庫會印出
    /// 「工作階段 412」配上一片空白的其他每一行，而那 412 場裡一場的東西都不
    /// 在了。
    ///
    /// **但「一樣久」有一個例外，而那個例外不是罕見情況。** 那支函式不准碰
    /// `ended_at IS NULL` 而且 `id = MAX(id)` 的那一列，因為那可能是**此刻正
    /// 在錄**的那一場，刪掉它會讓另一個行程的下一列撞上外鍵。可是那個條件同時
    /// 也是**當掉的那一場**長的樣子——這個 repo 自己的定義（見 `crash_verdict`）
    /// ——而那兩者在 SQL 裡分不出來。所以最後一場當掉之後跑 `forget`，那一列
    /// 會留下來，這個數字是 1，而其他每一個數字都是 0。
    ///
    /// 也就是說**這個數字和這一頁上其他數字描述的不是同一個集合**。上一版的
    /// 註解宣稱它們是，而 `nothing_recorded_left()` 就是照那句話把它算進去的
    /// ——於是那顆資料庫在三個畫面上長得像從來沒錄過。
    pub sessions: i64,
    /// 他問過幾題（`queries`）。
    ///
    /// 這一頁不印它——`sister queries` 是它的家。放在這裡是為了讓
    /// [`nothing_recorded_left`](Self::nothing_recorded_left) 那份解構有機會
    /// **明講它為什麼不算**：這張表存的是他自己打進搜尋框的字，不是她記下來
    /// 的東西，而且它會在清空**之後**長出來——`forget` 完接著問一句「真的沒
    /// 了嗎」就是一列。把它算進「她記的東西還剩不剩」，那一問就會讓整個刪除
    /// 從三個畫面上消失。
    pub queries: i64,
    /// `text_chunks` 的時間範圍。`retention.text_days` 管的是這一對。
    pub first_ts: Option<Millis>,
    pub last_ts: Option<Millis>,
    /// **還留著畫面檔的那幾列**的時間範圍。`retention.frames_days` 管這一對。
    ///
    /// 和 [`first_ts`](Self::first_ts) 分開，是因為 [`image_bytes`](Self::image_bytes)
    /// 只涵蓋這一段：畫面被清掉的時候，那一列的 `image_bytes` 會一起歸零
    /// （見 `retention.rs` 的 `UPDATE frames SET image_path = NULL, image_bytes = 0`）。
    ///
    /// 預設兩個保留期不一樣（文字 365 天、畫面 30 天），所以用滿一年之後
    /// 拿 `image_bytes` 去除以 `first_ts..last_ts`，是**三十天的分子配一年
    /// 的分母**——每天的用量會印成實際的十二分之一。那個數字是 Phase 0 退出
    /// 條件（< 300 MB/天）的判決，而錯的方向剛好是「看起來過了」。
    pub image_first_ts: Option<Millis>,
    pub image_last_ts: Option<Millis>,
    pub db_bytes: i64,
}

impl DbStats {
    /// **她記下來的東西**，現在一列都不剩。
    ///
    /// 「有沒有東西」和「有沒有畫面」是兩件事，而 `sister stats` 上那句
    /// 「底下每一個數字都是 0」曾經只問了後者。一場全程被排除規則擋掉的錄製
    /// 走到的是 `frames == 0 && chunks == 0`，而它的 `system_events`、
    /// `input_windows` 和整份排除稽核都好好地印在同一頁上——那句話於是印在
    /// 一頁有數字的東西上面，還順便說那些東西是被 `forget` 刪掉的。
    ///
    /// **「她記下來的東西」這幾個字是整支函式最重要的部分**，而它砍掉兩張表
    /// 加一種列：`queries`（他打的字）、`sessions`（裝東西的殼）、以及
    /// `system_events` 裡那場錄製自己的開始／結束。三次都是同一課，而我三次都
    /// 是先寫錯才學會的：
    ///
    /// - `queries` 會在清空**之後**長出來。`forget` 完接著問一句「真的沒了
    ///   嗎」就有一列。
    /// - `sessions` 會**撐過**清空。`delete_empty_sessions` 不准碰
    ///   `ended_at IS NULL AND id = MAX(id)` 的那一列，而那正是當掉的那一場。
    /// - `session_start` / `session_end` **兩樣都會**。清空之後她再開始錄，
    ///   `Recorder::new` 第一件事就是寫一列 `session_start`；而 `finish` 寫的
    ///   那列 `session_end` 是在整場都被忘掉之後才落地的。
    ///
    /// 三種下場一模一樣：這個述詞翻成 false，[`crate::db`] 外面那個 `Emptiness`
    /// 讓開 `Erased`、接到最寬的 `Fresh`，於是 `stats` 的 ⚠ 整段消失、`doctor`
    /// 說「（還沒有任何內容）」、`facts` 說「她還沒錄過」——他一秒前才親手刪
    /// 掉的一整天，被一列跟內容無關的東西否認了。
    ///
    /// **所以往這裡加一個計數器之前要問兩題**（解構會逼你至少看它一眼）：
    /// 這張表會不會在刪除**之後**長出來？會不會**撐過**刪除？兩題有一題是
    /// 「會」，它就不屬於「她記下來的東西」。而第三次那一課是：**同一張表裡
    /// 的列不見得是同一種東西**，所以問題要問到列的層次，不是表的層次。
    ///
    /// 「這顆資料庫裡一列都不剩」那句假話不是靠這裡修的，是靠呼叫端把話講準：
    /// 它說的是**她記下來的東西**沒了，不是這個檔案是空的。
    ///
    /// 用**解構**而不是一串 `self.x == 0 &&`：漏掉一個欄位的下場就是上面第一
    /// 段，而漏掉的方式是「後來有人加了一個計數器」。解構之後 `DbStats` 多一
    /// 個欄位、這裡沒跟上的話，編譯就不會過——連「不算」都要明著寫下來。
    pub fn nothing_recorded_left(&self) -> bool {
        let Self {
            frames,
            frames_collapsed,
            frames_with_image,
            ocr_blocks,
            chunks,
            facts,
            focus_events,
            clipboard_events,
            input_windows,
            system_events,
            // **不算**，理由見 `session_marks` 欄位自己的註解：那兩列是容器上
            // 的標籤。底下的比較是 `system_events == session_marks`——「剩下的
            // 每一列都是標籤」——而不是 `system_events == 0`。
            session_marks,
            // **不算。** 一場錄製是個**容器**，不是她記下來的東西——裡面的
            // 東西全走了以後，剩下的那個殼不該讓「還剩不剩」答成「還剩」。
            //
            // 而它真的會剩下來：`delete_empty_sessions` 不准碰
            // `ended_at IS NULL AND id = MAX(id)` 的那一列（可能是此刻正在錄
            // 的那一場），而那也正是**當掉的那一場**的樣子。最後一場當掉之後
            // 跑 `forget`，這裡就是 1——算進去的話 `Emptiness` 讓開 `Erased`
            // 接到最寬的 `Fresh`，`stats` 的 ⚠ 消失、`doctor` 說「還沒有任何
            // 內容」、`facts` 說「她還沒錄過」。他一秒前才刪掉的一整天，被一
            // 個空殼否認了。見 `sessions` 欄位自己的註解。
            sessions: _,
            // **不算**，理由見 `queries` 欄位自己的註解：這是他打的字，不是
            // 她記的東西，而且它會在刪除之後才長出來。
            queries: _,
            // 底下這些不是「有幾件事發生過」：
            // - `image_bytes` 是大小，而且一列都不剩的時候它必然是 0
            // - 兩對時戳是範圍，沒有列就沒有範圍
            // - `db_bytes` **清空之後照樣不是 0**（schema、索引、WAL 都還在），
            //   拿它當「有沒有東西」問，答案永遠是「有」
            image_bytes: _,
            first_ts: _,
            last_ts: _,
            image_first_ts: _,
            image_last_ts: _,
            db_bytes: _,
        } = self;
        // 排除稽核和暫停稽核都是 `system_events` 的列，遮蔽稽核數的是
        // `clipboard_events` 的列（見 `exclusion_audit` / `pause_audit` /
        // `redaction_audit` 的 SQL），所以那三份不用另外問——它們的分母
        // 已經在這裡面了。
        *frames == 0
            && *frames_collapsed == 0
            && *frames_with_image == 0
            && *ocr_blocks == 0
            && *chunks == 0
            && *facts == 0
            && *focus_events == 0
            && *clipboard_events == 0
            && *input_windows == 0
            // 「剩下的每一列都是那場錄製自己的標籤」。寫成減法
            // （`system_events - session_marks == 0`）也一樣，但等號讀起來就是
            // 這句話本身。`Excluded`、`CapturePaused`、`Lock` 那幾種不是標籤，
            // 所以只要還有一列，這裡就是 false——那是對的：她那時候看到了東西
            // 而且照規則沒記，那是這份紀錄裡最該留下來的一種證據。
            && *system_events == *session_marks
    }

    /// **`sessions` 上還站著的那幾列，全都是空殼。**
    ///
    /// [`retention::delete_empty_sessions`](crate::retention) 不准碰
    /// `ended_at IS NULL AND id = MAX(id)` 的那一列——那可能是此刻正在錄的那一
    /// 場，刪掉它，接下來每一列的 `session_id` 都會指向一個不存在的東西。而
    /// **當掉的那一場**長得一模一樣，所以它也留下來了：一列沒有正常收尾、裡面
    /// 一個東西都不剩的紀錄。
    ///
    /// 條件只要兩句：她記下來的東西一列都不剩，而那張表還有列。有東西的那一場
    /// 會讓前半句是假的，所以這時候剩下的每一列都必然是空殼——不必再去問一次
    /// `ended_at`（也問不到，[`DbStats`] 上沒有那一欄）。
    ///
    /// 住在這裡而不是在某個 surface 裡，是因為講這句話的地方不只一個：`sister
    /// forget` 刪完那一行、`sister stats` 的「工作階段」、字母人時間軸上那份
    /// 刪除結果。同一個判斷抄三次，遲早會有一份先被改到。
    pub fn only_session_shells_left(&self) -> bool {
        self.sessions > 0 && self.nothing_recorded_left()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ClipboardKind, FocusKind, OcrBlock, SystemKind};

    fn test_db() -> Db {
        Db::open_in_memory().expect("open in-memory db")
    }

    /// 匯出要驗的東西在磁碟上（WAL 是檔案的行為），所以這幾個測試不能用
    /// in-memory。
    struct TmpDir(std::path::PathBuf);
    impl TmpDir {
        fn new(name: &str) -> Self {
            static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!(
                "sister-db-{}-{name}-{}",
                std::process::id(),
                N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("create temp dir");
            Self(dir)
        }
        fn join(&self, name: &str) -> std::path::PathBuf {
            self.0.join(name)
        }
    }
    impl Drop for TmpDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// `dbstat` 的每一列都是真的還不夠；加總後必須和 SQLite 自己報的配置量
    /// 一個位元組不差。FTS 的 shadow tables 也要留著真名，否則這次要找的
    /// 「其他 2.1 MB 到底是哪張表」仍然答不出來。
    #[test]
    fn disk_snapshot_names_dbstat_objects_and_balances_every_logical_page() {
        let tmp = TmpDir::new("disk-objects");
        let path = tmp.join("sister.db");
        let mut db = Db::open(&path).expect("open");
        let session = db.start_session("test", "0.0.1").expect("session");
        for i in 0..40 {
            db.insert_frame(
                session,
                &frame_with_text(
                    i * 1_000,
                    "chrome.exe",
                    "客服系統",
                    &[
                        &format!("第 {i} 筆客服專線紀錄與帳單內容"),
                        "這一行讓每幀有兩個 OCR block",
                    ],
                ),
                None,
                if i == 0 { 12_345 } else { 0 },
            )
            .expect("insert");
        }

        let snapshot = db.disk_snapshot().expect("snapshot");
        for name in [
            "ocr_blocks",
            "text_chunks",
            "text_fts_data",
            "text_fts_uni_data",
            "text_fts_bi_data",
            "idx_ocr_frame",
            "idx_chunk_ts",
        ] {
            assert!(
                snapshot.objects.get(name).is_some_and(|bytes| *bytes > 0),
                "dbstat 少了 {name}：{:?}",
                snapshot.objects.keys().collect::<Vec<_>>()
            );
        }
        let object_bytes: u64 = snapshot.objects.values().sum();
        assert_eq!(
            object_bytes + snapshot.free_bytes + snapshot.residual_bytes,
            snapshot.logical_allocated_bytes,
            "objects + freelist + residual 必須精確對帳"
        );
        assert_eq!(snapshot.catalogued_image_bytes, 12_345);
        assert_eq!(snapshot.journal_mode, "wal");
        assert!(snapshot.wal_autocheckpoint_pages > 0);
    }

    #[test]
    fn disk_snapshot_measures_main_wal_and_shm_as_three_named_files() {
        let tmp = TmpDir::new("disk-files");
        let path = tmp.join("sister.db");
        let mut db = Db::open(&path).expect("open");
        let session = db.start_session("test", "0.0.1").expect("session");
        db.insert_frame(
            session,
            &frame_with_text(1, "chrome.exe", "帳單", &["王先生來電談帳單"]),
            None,
            0,
        )
        .expect("insert");

        let snapshot = db.disk_snapshot().expect("snapshot");
        let files = snapshot.files.expect("實體 DB 要有檔案量測");
        let wal_path = path_with_suffix(&path, "-wal");
        let shm_path = path_with_suffix(&path, "-shm");
        assert_eq!(
            files.get(&SqliteFileKind::Main),
            Some(&optional_file_len(&path).expect("main metadata"))
        );
        assert_eq!(
            files.get(&SqliteFileKind::Wal),
            Some(&optional_file_len(&wal_path).expect("wal metadata"))
        );
        assert_eq!(
            files.get(&SqliteFileKind::Shm),
            Some(&optional_file_len(&shm_path).expect("shm metadata"))
        );
        assert!(
            files
                .get(&SqliteFileKind::Wal)
                .copied()
                .flatten()
                .is_some_and(|bytes| bytes > 0),
            "測試中的寫入必須真的留在 WAL，否則沒驗到那個檔案"
        );
        assert!(
            files.get(&SqliteFileKind::Shm).copied().flatten().is_some(),
            "WAL 連線應有 shared-memory 檔"
        );
    }

    #[test]
    fn an_in_memory_disk_snapshot_has_no_files_not_three_zeroes() {
        let db = test_db();
        let snapshot = db.disk_snapshot().expect("snapshot");
        assert_eq!(snapshot.files, None);
        assert!(snapshot.logical_allocated_bytes > 0, "邏輯頁面仍然量得到");
        assert_eq!(
            snapshot.objects.values().sum::<u64>() + snapshot.free_bytes + snapshot.residual_bytes,
            snapshot.logical_allocated_bytes
        );
    }

    #[test]
    fn disk_delta_uses_union_keys_and_keeps_growth_and_shrinkage_signed() {
        let before = DbDiskSnapshot {
            logical_allocated_bytes: 1_000,
            objects: BTreeMap::from([
                ("alpha".into(), 100),
                ("deleted".into(), 50),
                ("stable".into(), 5),
            ]),
            free_bytes: 100,
            residual_bytes: 20,
            catalogued_image_bytes: 80,
            files: Some(BTreeMap::from([
                (SqliteFileKind::Main, Some(100)),
                (SqliteFileKind::Wal, None),
                (SqliteFileKind::Shm, Some(40)),
            ])),
            journal_mode: "wal".into(),
            wal_autocheckpoint_pages: 1_000,
        };
        let after = DbDiskSnapshot {
            logical_allocated_bytes: 900,
            objects: BTreeMap::from([
                ("alpha".into(), 70),
                ("created".into(), 25),
                ("stable".into(), 5),
            ]),
            free_bytes: 60,
            residual_bytes: 45,
            catalogued_image_bytes: 120,
            // 故意不放 SHM key：聯集仍要把收尾消失的檔案報成 -40。
            files: Some(BTreeMap::from([
                (SqliteFileKind::Main, Some(80)),
                (SqliteFileKind::Wal, Some(30)),
            ])),
            journal_mode: "wal".into(),
            wal_autocheckpoint_pages: 1_000,
        };

        let delta = DbDiskDelta::between(&before, &after).expect("delta");
        assert_eq!(delta.logical_allocated_bytes, -100);
        assert_eq!(
            delta.objects,
            BTreeMap::from([
                ("alpha".into(), -30),
                ("created".into(), 25),
                ("deleted".into(), -50),
                ("stable".into(), 0),
            ])
        );
        assert_eq!(delta.free_bytes, -40);
        assert_eq!(delta.residual_bytes, 25);
        assert_eq!(delta.catalogued_image_bytes, 40);

        let files = delta.files.expect("file deltas");
        assert_eq!(
            files.get(&SqliteFileKind::Main),
            Some(&FileDelta {
                delta_bytes: -20,
                end_bytes: Some(80),
            })
        );
        assert_eq!(
            files.get(&SqliteFileKind::Wal),
            Some(&FileDelta {
                delta_bytes: 30,
                end_bytes: Some(30),
            })
        );
        assert_eq!(
            files.get(&SqliteFileKind::Shm),
            Some(&FileDelta {
                delta_bytes: -40,
                end_bytes: None,
            })
        );
    }

    /// 「資料庫佔多少」要含 `-wal`。
    ///
    /// `page_count × page_size` 只量主檔，而我們跑的是 WAL 模式——checkpoint
    /// 之間新寫的東西全在 `-wal` 裡，也就是「一邊錄一邊長」的正是那一半。
    /// 少算它，足跡那一行每一個磁碟數字都偏小，而那個數字是 Phase 0 退出
    /// 條件（< 300 MB/天）的判決，偏的方向剛好是「看起來過了」。
    #[test]
    fn the_disk_number_counts_the_wal_because_that_is_the_half_that_grows() {
        let tmp = TmpDir::new("wal-bytes");
        let path = tmp.join("sister.db");
        let mut db = Db::open(&path).expect("open");
        let s = db.start_session("test", "0.0.1").expect("session");
        // 寫到 WAL 真的長出東西為止。這裡不 checkpoint——正在錄的那台機器
        // 也不會，而這個數字要回答的就是「她現在佔了多少」。
        for i in 0..400 {
            db.insert_frame(
                s,
                &frame_with_text(i * 1_000, "Chrome", "帳單", &["王先生打電話來說帳單的事"]),
                None,
                0,
            )
            .expect("insert");
        }

        let wal =
            std::fs::metadata(format!("{}-wal", path.display())).map_or(0, |m| m.len() as i64);
        assert!(
            wal > 0,
            "這個測試要有 WAL 才有意義（寫太少或被 checkpoint 了）"
        );

        let main = std::fs::metadata(&path).expect("main").len() as i64;
        let counted = db.stats().expect("stats").db_bytes;
        assert!(
            counted >= main + wal,
            "只算了主檔 {main}，漏掉 WAL 的 {wal}（回報 {counted}）"
        );
    }

    /// 他機器上同時躺著好幾個長得一模一樣的 `sister.exe`。點錯一個的代價
    /// 不該是靜靜地把記憶寫壞——而在加這道擋之前，正是這樣：每一段
    /// migration 都是 `if version < N`，所以未來的資料庫會一段都不跑、
    /// 乾乾淨淨地回 Ok，然後被舊的 SQL 讀寫。
    #[test]
    fn a_database_from_a_newer_version_is_refused_not_quietly_half_read() {
        let tmp = TmpDir::new("newer-schema");
        let path = tmp.join("sister.db");

        // 先開一次，讓它長成這一版的樣子。
        Db::open(&path).expect("這一版開得起來");

        // 然後假裝未來某一版又加了一段 migration。
        {
            let c = rusqlite::Connection::open(&path).expect("open");
            c.pragma_update(None, "user_version", SCHEMA_VERSION + 1)
                .expect("bump");
        }

        // `{:#}` 而不是 `to_string()`：`Db::open` 外面包了一層 context，
        // 只讀最外層會拿到「run migrations」，看不到真正那句話。
        let err = match Db::open(&path) {
            Ok(_) => panic!("比我新的資料庫被放進來了"),
            Err(e) => format!("{e:#}"),
        };
        assert!(
            err.contains(&(SCHEMA_VERSION + 1).to_string()),
            "要講出它是第幾版：{err}"
        );
        assert!(err.contains("新版的 sister"), "要講出他該做什麼：{err}");

        // 擋的只有「比我新」這一邊。往回相容照舊：一個還沒有版號的空檔案
        // （user_version = 0，也就是每一個全新使用者）要一路升到最新。
        let fresh = Db::open(&tmp.join("fresh.db")).expect("新的開得起來");
        assert_eq!(fresh.schema_version().expect("version"), SCHEMA_VERSION);
    }

    /// **這一條就是 PRIVACY.md 那句「整份記憶就是一個 `sister.db` 檔」的反例。**
    ///
    /// 資料庫跑在 WAL 模式，所以她還在錄的時候，最近寫進去的東西躺在旁邊的
    /// `sister.db-wal` 裡。照那句話去複製一個檔案，備份會**安靜地少掉最後那
    /// 一段記憶**——而他要等到真的需要那份備份的那一天才會發現。
    ///
    /// 下面那個 `naive` 就是那個錯誤的備份。它不是在測 SQLite，它是在釘住
    /// 「為什麼要有 `sister export`」這件事：哪天這一半不再成立了，
    /// PRIVACY.md 那一段就可以改了，而這條測試會先講。
    #[test]
    fn a_backup_taken_while_she_is_still_recording_must_not_lose_the_last_hour() {
        let tmp = TmpDir::new("export");
        let live = tmp.join("sister.db");
        let mut db = Db::open(&live).expect("open");
        let s = db.start_session("test", "0.0.1").expect("session");
        let f = frame_with_text(1_000, "chrome.exe", "視窗", &["客服專線 0800-080-123"]);
        db.insert_frame(s, &f, Some("/tmp/x.webp"), 1)
            .expect("insert");

        // 錄製中的行程沒有關掉資料庫——這正是他會去按下備份的那個時刻。
        let naive = tmp.join("naive.db");
        std::fs::copy(&live, &naive).expect("複製主檔");
        assert!(
            Db::open(&naive)
                .expect("open naive")
                .search("客服專線", 10)
                .expect("search")
                .is_empty(),
            "只複製 sister.db 就撈得到的話，PRIVACY.md 那一段可以改了"
        );

        let exported = tmp.join("export.db");
        db.export_to(&exported).expect("export");
        assert_eq!(
            Db::open(&exported)
                .expect("open exported")
                .search("客服專線", 10)
                .expect("search")
                .len(),
            1,
            "匯出要包含還在 WAL 裡的那一段"
        );

        // 打錯路徑的一次匯出不該蓋掉上一份備份。
        assert!(db.export_to(&exported).is_err(), "不覆蓋既有檔案");
    }

    #[test]
    fn replay_corpus_round_trips_every_l0_event_and_rebuilds_search_and_facts() {
        const FROM: Millis = 10_000;
        const TO: Millis = 11_000;
        const ORIGIN: Millis = 50_000;

        let mut source = test_db();
        let schema_before = source.schema_version().expect("source schema");
        let session = source
            .start_session_at("windows/test", "alpha.test", FROM)
            .expect("source session");

        // container marks belong to this database, not to the portable event stream.
        for (ts, kind) in [
            (FROM, SystemKind::SessionStart),
            (TO - 1, SystemKind::SessionEnd),
        ] {
            source
                .insert_system(
                    session,
                    &SystemEvent {
                        ts,
                        kind,
                        detail: None,
                    },
                )
                .expect("container mark");
        }

        let mut frame = frame_with_text(
            FROM + 100,
            "code.exe",
            "replay.rs - AI-Sister",
            &[
                "cargo test ERR_REPLAY_IMPORT",
                "客服 0912-345-678，帳單 NT$13,450",
            ],
        );
        frame.monitor = 2;
        frame.width = 1600;
        frame.height = 900;
        frame.dhash = u64::MAX - 7;
        frame.focus.url = Some("https://example.test/replay".into());
        frame.ocr[0] = OcrBlock {
            text: frame.ocr[0].text.clone(),
            x: 17,
            y: 23,
            w: 640,
            h: 31,
            confidence: -1.0,
        };
        let (frame_id, _, _) = source
            .insert_frame(session, &frame, Some("private.webp"), 12_345)
            .expect("frame");
        source.bump_frame_dup(frame_id).expect("dup one");
        source.bump_frame_dup(frame_id).expect("dup two");

        source
            .insert_focus(
                session,
                &FocusEvent {
                    ts: FROM + 200,
                    kind: FocusKind::TitleChange,
                    snapshot: FocusSnapshot {
                        app_id: Some("terminal.exe".into()),
                        app_name: Some("Windows Terminal".into()),
                        window_title: Some("workspace replay ERR_FOCUS_REPLAY".into()),
                        url: Some("https://example.test/build".into()),
                        pid: Some(4242),
                        password_field: false,
                    },
                },
            )
            .expect("focus");
        source
            .insert_clipboard(
                session,
                &ClipboardEvent {
                    ts: FROM + 300,
                    kind: ClipboardKind::Text,
                    text: Some("剪貼簿 ERR_CLIPBOARD_REPLAY".into()),
                    byte_len: 30,
                    truncated: false,
                    secret_suspected: false,
                    source_app: Some("terminal.exe".into()),
                },
            )
            .expect("clipboard");
        source
            .insert_input(
                session,
                &InputMetrics {
                    ts_start: FROM + 400,
                    ts_end: FROM + 450,
                    keystrokes: 11,
                    clicks: 12,
                    mouse_px: 13,
                    scroll_ticks: 14,
                    window_switches: 15,
                    idle_ms: 16,
                    typing_bursts: 17,
                },
            )
            .expect("input");
        source
            .insert_system(
                session,
                &SystemEvent {
                    ts: FROM + 500,
                    kind: SystemKind::Lock,
                    detail: Some("workstation locked".into()),
                },
            )
            .expect("system");
        // `[from, to)` 的右界不可以漏成 inclusive。
        source
            .insert_system(
                session,
                &SystemEvent {
                    ts: TO,
                    kind: SystemKind::Unlock,
                    detail: Some("outside export".into()),
                },
            )
            .expect("outside event");

        let draft = source
            .export_replay("真機 replay", FROM, TO)
            .expect("export replay");
        assert_eq!(
            source.schema_version().expect("schema after export"),
            schema_before,
            "replay export is not a migration"
        );
        let corpus = draft.as_corpus();
        assert_eq!(corpus.duration_ms, 1_000);
        assert_eq!(corpus.review, crate::replay::ReviewStatus::Draft);
        assert_eq!(corpus.events.len(), 5, "one of every portable event kind");
        assert_eq!(
            corpus
                .events
                .iter()
                .map(crate::replay::Event::at_ms)
                .collect::<Vec<_>>(),
            vec![100, 200, 300, 400, 500]
        );
        let crate::replay::Event::Frame {
            monitor,
            width,
            height,
            dhash,
            dup_run,
            focus,
            ocr,
            ..
        } = &corpus.events[0]
        else {
            panic!("first event was not the frame")
        };
        assert_eq!((*monitor, *width, *height), (2, 1600, 900));
        assert_eq!(*dhash, u64::MAX - 7);
        assert_eq!(*dup_run, 2);
        assert_eq!(focus.app_id.as_deref(), Some("code.exe"));
        assert_eq!(ocr[0].x, 17);
        assert_eq!(ocr[0].confidence, -1.0);
        let encoded = serde_json::to_string(corpus).expect("json");
        assert!(
            !encoded.contains("private.webp"),
            "image path leaked: {encoded}"
        );
        assert!(!encoded.contains("0912-345-678"), "phone was not redacted");
        assert!(!encoded.contains("NT$13,450"), "money was not redacted");
        assert!(
            encoded.contains("ERR_REPLAY_IMPORT"),
            "ordinary evidence vanished"
        );
        assert!(
            !encoded.contains("outside export"),
            "right boundary was included"
        );

        let mut imported = test_db();
        let imported_schema = imported.schema_version().expect("import schema");
        let report = imported
            .import_replay(corpus, ORIGIN)
            .expect("import replay");
        assert!(report.session_id.is_some());
        assert_eq!(report.events, 5);
        assert_eq!(report.frames, 1);
        assert_eq!(
            report.facts, 5,
            "frame、focus title、clipboard 的 L1 都要算進回報：{report:?}"
        );
        assert_eq!(
            imported.schema_version().expect("schema after import"),
            imported_schema,
            "replay import is not a migration"
        );

        let stats = imported.stats().expect("stats");
        assert_eq!(stats.frames, 1);
        assert_eq!(stats.frames_collapsed, 2);
        assert_eq!(
            stats.frames_with_image, 0,
            "corpus import must be text-only"
        );
        assert_eq!(stats.image_bytes, 0);
        assert_eq!(stats.focus_events, 1);
        assert_eq!(stats.clipboard_events, 1);
        assert_eq!(stats.input_windows, 1);
        assert_eq!(stats.system_events, 3, "start + lock + end");
        assert_eq!(stats.session_marks, 2);
        assert_eq!(stats.facts as usize, report.facts);

        let hits = imported
            .search("ERR_REPLAY_IMPORT", 10)
            .expect("search rebuilt FTS");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].ts, ORIGIN + 100);
        let errors: std::collections::BTreeSet<_> = imported
            .facts_by_kind("error_code", 10)
            .expect("rebuilt facts")
            .into_iter()
            .map(|fact| fact.raw)
            .collect();
        for expected in [
            "ERR_REPLAY_IMPORT",
            "ERR_FOCUS_REPLAY",
            "ERR_CLIPBOARD_REPLAY",
        ] {
            assert!(errors.contains(expected), "missing {expected}: {errors:?}");
        }
        assert!(
            !imported
                .search("ERR_CLIPBOARD_REPLAY", 10)
                .expect("clipboard FTS")
                .is_empty(),
            "clipboard did not take the canonical text-chunk path"
        );

        let context = imported
            .frame_context(hits[0].frame_id.expect("frame source"))
            .expect("context")
            .expect("frame");
        assert_eq!(context.image_path, None);
        assert_eq!(context.dup_run, 2);
        assert_eq!((context.width, context.height), (1600, 900));
        let geometry: (i64, i64, i64, i64, f64) = imported
            .conn
            .query_row(
                "SELECT x,y,w,h,confidence FROM ocr_blocks ORDER BY id LIMIT 1",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .expect("geometry");
        assert_eq!(geometry, (17, 23, 640, 31, -1.0));
        let input: (i64, i64, i64, i64, i64, i64, i64, i64, i64) = imported
            .conn
            .query_row(
                "SELECT ts_start,ts_end,keystrokes,clicks,mouse_px,scroll_ticks,
                        window_switches,idle_ms,typing_bursts FROM input_metrics",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                    ))
                },
            )
            .expect("input round trip");
        assert_eq!(
            input,
            (ORIGIN + 400, ORIGIN + 450, 11, 12, 13, 14, 15, 16, 17)
        );
    }

    #[test]
    fn malformed_replay_is_rejected_before_any_write_and_empty_corpus_has_no_fake_session() {
        let mut bad = crate::replay::Corpus {
            format_version: crate::replay::FORMAT_VERSION,
            name: "bad-order".into(),
            duration_ms: 100,
            review: crate::replay::ReviewStatus::Draft,
            redactions: crate::replay::RedactionSummary::default(),
            events: vec![
                crate::replay::Event::System {
                    at_ms: 90,
                    kind: SystemKind::Lock,
                    detail: None,
                },
                crate::replay::Event::System {
                    at_ms: 10,
                    kind: SystemKind::Unlock,
                    detail: None,
                },
            ],
        };
        let mut db = test_db();
        assert!(db.import_replay(&bad, 1_000).is_err());
        let after_bad = db.stats().expect("stats after malformed");
        assert_eq!(after_bad.sessions, 0);
        assert_eq!(after_bad.system_events, 0);
        assert!(!db.ever_recorded().expect("ever recorded"));

        bad.name = "empty".into();
        bad.events.clear();
        let report = db.import_replay(&bad, 1_000).expect("empty is valid");
        assert_eq!(
            report,
            ReplayImport {
                session_id: None,
                events: 0,
                frames: 0,
                facts: 0,
            }
        );
        let after_empty = db.stats().expect("stats after empty");
        assert_eq!(after_empty.sessions, 0);
        assert_eq!(after_empty.system_events, 0);
        assert!(!db.ever_recorded().expect("empty did not record"));
    }

    fn frame_with_text(ts: Millis, app: &str, title: &str, lines: &[&str]) -> FrameCapture {
        FrameCapture {
            ts,
            monitor: 0,
            width: 1920,
            height: 1080,
            dhash: 0xDEAD_BEEF,
            image: None,
            image_ext: "webp",
            ocr: lines
                .iter()
                .enumerate()
                .map(|(i, t)| OcrBlock {
                    text: t.to_string(),
                    x: 0,
                    y: i as i32 * 20,
                    w: 400,
                    h: 18,
                    confidence: 0.95,
                })
                .collect(),
            focus: FocusSnapshot {
                app_id: Some(app.into()),
                app_name: Some(app.into()),
                window_title: Some(title.into()),
                url: None,
                pid: Some(42),
                password_field: false,
            },
        }
    }

    fn columns_of(db: &Db, table: &str) -> Vec<String> {
        db.conn
            .prepare(&format!("SELECT * FROM {table}"))
            .expect("prepare")
            .column_names()
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    /// 全新建立的資料庫，版號要對、schema 也要對。
    ///
    /// 只斷言版號是不夠的：`pragma_update(user_version, SCHEMA_VERSION)` 寫在
    /// 第一段 migration 後面時，版號永遠會是對的，而後面幾段一次都沒跑過。
    #[test]
    fn migrations_apply_and_set_version() {
        let db = test_db();
        assert_eq!(db.schema_version().expect("version"), SCHEMA_VERSION);
        assert!(
            !columns_of(&db, "facts").contains(&"confidence".to_string()),
            "版號說跑到最新了，但 002 沒真的跑"
        );
        assert!(
            columns_of(&db, "segment").contains(&"core_started_at".to_string()),
            "全新資料庫也要有 segment 表"
        );
        assert!(
            columns_of(&db, "segment_edit").contains(&"algo_confidence".to_string()),
            "全新資料庫也要有 segment_edit 表"
        );
        assert!(
            columns_of(&db, "stuck_signal").contains(&"error_fact_count".to_string()),
            "全新資料庫也要有 stuck_signal 表"
        );
    }

    /// 已經在跑的資料庫升級上來，事實不能掉，欄位要真的消失。
    ///
    /// 這個測試存在的理由是它抓得到「版號蓋了但 migration 沒跑」：那種錯不會
    /// 當場爆炸，只會讓舊機器上的欄位一直留著，而 `SELECT` 早就不撈它了。
    #[test]
    fn a_database_from_the_previous_version_upgrades_without_losing_its_facts() {
        let conn = Connection::open_in_memory().expect("open");
        conn.execute_batch(MIGRATION_001).expect("v1 schema");
        conn.execute(
            "INSERT INTO facts(ts, kind, raw, normalized, confidence, source_kind)
             VALUES(1000, 'phone', '0800-000-123', '0800000123', 0.95, 'ocr')",
            [],
        )
        .expect("v1 row");
        conn.pragma_update(None, "user_version", 1).expect("stamp");

        let db = Db::init(conn).expect("upgrade");

        assert_eq!(db.schema_version().expect("version"), SCHEMA_VERSION);
        let rows = db.facts_by_kind("phone", 10).expect("facts survive");
        assert_eq!(rows.len(), 1, "升級把舊事實弄丟了");
        assert_eq!(rows[0].raw, "0800-000-123");

        assert!(
            !columns_of(&db, "facts").contains(&"confidence".to_string()),
            "欄位還在——002 沒跑，但版號已經蓋成 2 了"
        );
        assert!(
            columns_of(&db, "segment").contains(&"core_started_at".to_string()),
            "升到 8 之後要有 segment 表"
        );
        assert!(
            columns_of(&db, "segment_edit").contains(&"algo_confidence".to_string()),
            "升到 9 之後要有 segment_edit 表"
        );
        let n: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM segment", [], |r| r.get(0))
            .expect("count");
        assert_eq!(n, 0, "升級不回填段落——還沒打開時間軸就算不該有列");
        let edits: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM segment_edit", [], |r| r.get(0))
            .expect("count edits");
        assert_eq!(edits, 0, "升級不回填編輯");
    }

    fn focus_at(ts: Millis, app: &str, title: &str, url: Option<&str>) -> FocusEvent {
        FocusEvent {
            ts,
            kind: FocusKind::Focus,
            snapshot: FocusSnapshot {
                app_id: Some(app.into()),
                window_title: Some(title.into()),
                url: url.map(|u| u.into()),
                ..Default::default()
            },
        }
    }

    /// 斷句在打開時間軸時才算，不在寫入焦點事件時算。
    #[test]
    fn chapters_are_computed_on_demand_and_survive_a_second_open() {
        let mut db = test_db();
        let s = db.start_session("test", "0.0.1").expect("session");
        for e in [
            focus_at(0, "code.exe", "db.rs", None),
            focus_at(60_000, "chrome.exe", "mail", Some("https://mail.example/")),
            focus_at(90_000, "chrome.exe", "mail", Some("https://mail.example/")),
        ] {
            db.insert_focus(s, &e).expect("focus");
        }
        let n: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM segment", [], |r| r.get(0))
            .expect("count");
        assert_eq!(n, 0, "寫入焦點不該順便斷句");

        let first = db.chapters_for_range(0, 200_000).expect("chapters");
        assert_eq!(first.len(), 2, "code → chrome 該切成兩段");
        assert_eq!(first[0].app.as_deref(), Some("code.exe"));
        assert_eq!(first[1].app.as_deref(), Some("chrome.exe"));
        assert!(first[0].confidence.is_none());
        assert!(first[1].confidence.is_some());

        let stored: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM segment", [], |r| r.get(0))
            .expect("stored");
        assert_eq!(stored, 2);

        let second = db.chapters_for_range(0, 200_000).expect("recompute");
        assert_eq!(second.len(), 2);
        let stored_again: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM segment", [], |r| r.get(0))
            .expect("stored");
        assert_eq!(stored_again, 2, "重算是換掉，不是疊上去");
    }

    /// 「沒解析到時間範圍」和「解析了、算過、沒有段落」不可以長成同一個
    /// `None` 或同一個空陣列。
    #[test]
    fn chapters_for_a_question_distinguish_unparsed_from_empty() {
        use chrono::TimeZone;
        let mut db = test_db();
        let now = chrono::Local
            .with_ymd_and_hms(2026, 8, 26, 15, 30, 0)
            .single()
            .expect("local")
            .timestamp_millis();
        assert!(
            db.chapters_for_question("電話", now).expect("ok").is_none(),
            "沒有時間範圍就不該去算"
        );

        let asked = db
            .chapters_for_question("昨天下午", now)
            .expect("ok")
            .expect("認得出昨天下午");
        assert_eq!(asked.0.said, "昨天下午");
        assert!(asked.1.is_empty(), "沒有事件，算過是空的，不是沒算過");

        let s = db.start_session("test", "0.0.1").expect("session");
        let ts = asked.0.from + 60_000;
        db.insert_focus(s, &focus_at(ts, "code.exe", "db.rs", None))
            .expect("focus");
        db.insert_focus(
            s,
            &focus_at(
                ts + 90_000,
                "chrome.exe",
                "mail",
                Some("https://mail.example/"),
            ),
        )
        .expect("focus");
        let again = db
            .chapters_for_question("昨天下午", now)
            .expect("ok")
            .expect("still parsed");
        assert!(
            !again.1.is_empty(),
            "有焦點事件就該切得出段落，不能還是空的"
        );
    }

    /// 答案端拿的是活動級。45+25+45 三分鐘級會被 10 分鐘上限切成 13 段，
    /// 問句路徑要併回 3 件；`segment` 表仍是 13 列。
    #[test]
    fn question_chapters_are_activities_not_time_capped_slices() {
        use chrono::TimeZone;
        let mut db = test_db();
        let now = chrono::Local
            .with_ymd_and_hms(2026, 8, 26, 15, 30, 0)
            .single()
            .expect("local")
            .timestamp_millis();
        let asked = db
            .chapters_for_question("昨天下午", now)
            .expect("ok")
            .expect("認得出昨天下午");
        let s = db.start_session("test", "0.0.1").expect("session");
        let t0 = asked.0.from + 2 * 3_600_000;
        let min = 60_000i64;
        for e in [
            focus_at(t0, "code.exe", "db.rs — AI-Sister", None),
            focus_at(
                t0 + 45 * min,
                "chrome.exe",
                "SQLite user_version 文件",
                Some("https://sqlite.org/pragma.html"),
            ),
            focus_at(t0 + 70 * min, "notion.exe", "週報", None),
            focus_at(t0 + 115 * min, "notion.exe", "週報", None),
        ] {
            db.insert_focus(s, &e).expect("focus");
        }

        let again = db
            .chapters_for_question("昨天下午", now)
            .expect("ok")
            .expect("still parsed");
        assert_eq!(again.1.len(), 3, "三件事不該被 10 分鐘上限講成十幾段");
        assert_eq!(
            again.1.iter().map(|a| a.segment_count).collect::<Vec<_>>(),
            vec![5, 3, 5]
        );
        assert_eq!(again.1[0].core_ms(), 45 * min);
        assert_eq!(again.1[1].core_ms(), 25 * min);
        assert_eq!(again.1[2].core_ms(), 45 * min);

        let stored: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM segment", [], |r| r.get(0))
            .expect("count");
        assert_eq!(stored, 13, "聚合不准改 segment 表");
    }

    /// 整合者自己加的：合併過的段落必須扛得住整輪 DELETE + 重算。
    #[test]
    fn a_manual_merge_survives_a_full_recompute() {
        let mut db = test_db();
        let s = db.start_session("test", "0.0.1").expect("session");
        for e in [
            focus_at(0, "code.exe", "db.rs", None),
            focus_at(60_000, "chrome.exe", "mail", Some("https://mail.example/")),
            focus_at(120_000, "notion.exe", "week", None),
            focus_at(180_000, "notion.exe", "week", None),
        ] {
            db.insert_focus(s, &e).expect("focus");
        }
        let before = db.chapters_for_range(0, 300_000).expect("chapters");
        assert_eq!(before.len(), 3, "三個 app 該切成三段");
        let boundary = before[1].core_started_at;
        db.merge_chapters(before[0].core_started_at, boundary, 0, 300_000)
            .expect("merge");
        let merged = db.chapters_for_range(0, 300_000).expect("after merge");
        assert_eq!(merged.len(), 2, "合併後該剩兩段");
        let reopened = db.chapters_for_range(0, 300_000).expect("reopen");
        assert_eq!(reopened.len(), 2, "重算之後合併被吃掉了");
        let third = db.chapters_for_range(0, 300_000).expect("third");
        assert_eq!(third.len(), 2, "第三次開也要還在");
    }

    #[test]
    fn forgetting_a_range_drops_the_segments_that_overlapped_it() {
        let mut db = test_db();
        let s = db.start_session("test", "0.0.1").expect("session");
        for e in [
            focus_at(0, "code.exe", "db.rs", None),
            focus_at(60_000, "chrome.exe", "mail", Some("https://mail.example/")),
            focus_at(90_000, "chrome.exe", "mail", Some("https://mail.example/")),
        ] {
            db.insert_focus(s, &e).expect("focus");
        }
        db.chapters_for_range(0, 200_000).expect("chapters");
        db.forget(50_000, 100_000, None).expect("forget");
        let n: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM segment", [], |r| r.get(0))
            .expect("count");
        assert_eq!(n, 0, "重疊到的段落該跟事件一起走");
    }

    fn count_table(db: &Db, table: &str) -> i64 {
        db.conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .expect("count")
    }

    /// 合併寫進另一張表；重算 `segment` 不會把它吃掉。
    #[test]
    fn a_merge_survives_recompute() {
        let mut db = test_db();
        let s = db.start_session("test", "0.0.1").expect("session");
        for e in [
            focus_at(0, "code.exe", "db.rs", None),
            focus_at(60_000, "chrome.exe", "mail", Some("https://mail.example/")),
            focus_at(90_000, "chrome.exe", "mail", Some("https://mail.example/")),
        ] {
            db.insert_focus(s, &e).expect("focus");
        }
        let first = db.chapters_for_range(0, 200_000).expect("chapters");
        assert_eq!(first.len(), 2);
        let merged = db
            .merge_chapters(
                first[0].core_started_at,
                first[1].core_started_at,
                0,
                200_000,
            )
            .expect("merge");
        assert_eq!(merged.len(), 1);
        assert_eq!(
            merged[0].last_edit.map(|e| e.kind),
            Some(crate::segment_edit::EditKind::Merge)
        );
        assert_eq!(count_table(&db, "segment_edit"), 1);

        let again = db.chapters_for_range(0, 200_000).expect("recompute");
        assert_eq!(again.len(), 1, "重算要把編輯套回去，不能只剩演算法那兩段");
        assert_eq!(count_table(&db, "segment_edit"), 1, "重算不准動編輯表");
        let kinds: String = db
            .conn
            .query_row("SELECT kind FROM segment_edit", [], |r| r.get(0))
            .expect("kind");
        assert_eq!(kinds, "merge");
        let algo: Option<String> = db
            .conn
            .query_row("SELECT algo_cut_kinds FROM segment_edit", [], |r| r.get(0))
            .expect("algo");
        assert_eq!(
            algo.as_deref(),
            Some("app_change"),
            "訓練訊號要記得演算法原本切在哪"
        );
        let conf: Option<f64> = db
            .conn
            .query_row("SELECT algo_confidence FROM segment_edit", [], |r| r.get(0))
            .expect("conf");
        assert!(
            conf.is_some(),
            "當時演算法的信心值要留下，沒有就該是 NULL 不是 0"
        );
    }

    #[test]
    fn a_split_survives_recompute_and_can_be_undone() {
        let mut db = test_db();
        let s = db.start_session("test", "0.0.1").expect("session");
        db.insert_focus(s, &focus_at(0, "code.exe", "db.rs", None))
            .expect("focus");
        db.insert_focus(s, &focus_at(90_000, "code.exe", "db.rs", None))
            .expect("focus");
        let first = db.chapters_for_range(0, 200_000).expect("chapters");
        assert_eq!(first.len(), 1);
        let at = (first[0].core_started_at + first[0].core_ended_at) / 2;
        let split = db.split_chapter(at, 0, 200_000).expect("split");
        assert_eq!(split.len(), 2);
        assert!(split.iter().all(|c| {
            c.last_edit
                .is_some_and(|e| e.kind == crate::segment_edit::EditKind::Split)
        }));
        let edit_id = split[0].last_edit.unwrap().id;
        let again = db.chapters_for_range(0, 200_000).expect("recompute");
        assert_eq!(again.len(), 2, "切開被重算吃掉了");

        let undone = db.undo_segment_edit(edit_id, 0, 200_000).expect("undo");
        assert_eq!(undone.len(), 1);
        assert!(undone[0].last_edit.is_none());
        assert_eq!(
            count_table(&db, "segment_edit"),
            2,
            "撤銷是多一列，不是刪掉"
        );
    }

    /// 時間軸改畫活動級之後，分鐘級的合併／切開／撤銷還是同一套 `segment_edit`。
    #[test]
    fn activity_view_applies_and_undoes_segment_edits() {
        let mut db = test_db();
        let s = db.start_session("test", "0.0.1").expect("session");
        let min = 60_000i64;
        for e in [
            focus_at(0, "code.exe", "db.rs", None),
            focus_at(45 * min, "chrome.exe", "docs", Some("https://sqlite.org/")),
            focus_at(70 * min, "notion.exe", "週報", None),
            focus_at(115 * min, "notion.exe", "週報", None),
        ] {
            db.insert_focus(s, &e).expect("focus");
        }
        let acts = db.activities_for_range(0, 120 * min).expect("activities");
        assert_eq!(acts.len(), 3, "三件事");
        assert_eq!(
            acts.iter().map(|a| a.segment_count).collect::<Vec<_>>(),
            vec![5, 3, 5]
        );
        let segs: usize = acts.iter().map(|a| a.segments.len()).sum();
        assert_eq!(segs, 13, "展開看得到底下的分鐘級");

        // 合併兩件活動 = 合併左件最後一段和右件第一段。
        let left = acts[0].segments.last().unwrap().core_started_at;
        let right = acts[1].segments[0].core_started_at;
        db.merge_chapters(left, right, 0, 120 * min).expect("merge");
        let after_merge = db.activities_for_range(0, 120 * min).expect("merged");
        assert_eq!(after_merge.len(), 2, "兩件活動併成一件");
        assert!(
            after_merge[0].last_edit().is_some(),
            "併過的那一件要掛得回 segment_edit"
        );
        assert_eq!(count_table(&db, "segment_edit"), 1, "只寫一列，不重複套");

        let edit_id = after_merge[0].last_edit().unwrap().id;
        db.undo_segment_edit(edit_id, 0, 120 * min).expect("undo");
        let after_undo = db.activities_for_range(0, 120 * min).expect("undone");
        assert_eq!(after_undo.len(), 3, "撤銷回到三件");
        assert_eq!(count_table(&db, "segment_edit"), 2, "撤銷是多一列");

        // 在第一件活動中間切開：硬邊界，不准黏回去。
        let host = &after_undo[0];
        let at = (host.core_started_at + host.core_ended_at) / 2;
        db.split_chapter(at, 0, 120 * min).expect("split");
        let after_split = db.activities_for_range(0, 120 * min).expect("split");
        assert_eq!(after_split.len(), 4, "切開把一件拆成兩件，另外兩件不動");
        assert!(
            after_split.iter().any(|a| a
                .last_edit()
                .is_some_and(|e| e.kind == crate::segment_edit::EditKind::Split)),
            "切開要掛在活動上看得到"
        );
    }

    #[test]
    fn forgetting_a_range_drops_the_edits_that_overlapped_it() {
        let mut db = test_db();
        let s = db.start_session("test", "0.0.1").expect("session");
        for e in [
            focus_at(0, "code.exe", "db.rs", None),
            focus_at(60_000, "chrome.exe", "mail", Some("https://mail.example/")),
            focus_at(90_000, "chrome.exe", "mail", Some("https://mail.example/")),
        ] {
            db.insert_focus(s, &e).expect("focus");
        }
        let first = db.chapters_for_range(0, 200_000).expect("chapters");
        db.merge_chapters(
            first[0].core_started_at,
            first[1].core_started_at,
            0,
            200_000,
        )
        .expect("merge");
        assert_eq!(count_table(&db, "segment_edit"), 1);
        db.forget(50_000, 100_000, None).expect("forget");
        assert_eq!(
            count_table(&db, "segment_edit"),
            0,
            "事件被忘掉之後，對應的編輯要跟著走"
        );
    }

    #[test]
    fn stuck_signal_is_recorded_only_when_all_three_are_measured() {
        use crate::model::{InputMetrics, OcrBlock};
        use crate::stuck::{STUCK_DWELL_MS, STUCK_SWITCH_MIN};

        let mut db = test_db();
        let s = db.start_session("test", "0.0.1").expect("session");
        let end = STUCK_DWELL_MS + 10_000;
        db.insert_focus(s, &focus_at(0, "code.exe", "db.rs", None))
            .expect("focus");
        db.insert_focus(s, &focus_at(end, "code.exe", "db.rs", None))
            .expect("focus");
        db.insert_input(
            s,
            &InputMetrics {
                ts_start: 0,
                ts_end: 10_000,
                window_switches: STUCK_SWITCH_MIN,
                ..Default::default()
            },
        )
        .expect("input");
        db.insert_frame(
            s,
            &FrameCapture {
                ts: 30_000,
                monitor: 0,
                width: 100,
                height: 100,
                dhash: 1,
                image: None,
                image_ext: "png",
                ocr: vec![OcrBlock {
                    text: "error E0308 mismatched types".into(),
                    x: 0,
                    y: 0,
                    w: 10,
                    h: 10,
                    confidence: 0.9,
                }],
                focus: FocusSnapshot {
                    app_id: Some("code.exe".into()),
                    window_title: Some("db.rs".into()),
                    ..Default::default()
                },
            },
            None,
            0,
        )
        .expect("frame");

        db.chapters_for_range(0, end + 1).expect("chapters");
        assert!(
            count_table(&db, "stuck_signal") >= 1,
            "三個成分都量到而且過門檻，該記下一次卡住"
        );

        // 反面：沒有 input_metrics 時不可以拿 0 次切換來充數，也不該寫一列。
        let mut empty = test_db();
        let s = empty.start_session("test", "0.0.1").expect("session");
        empty
            .insert_focus(s, &focus_at(0, "code.exe", "db.rs", None))
            .expect("focus");
        empty
            .insert_focus(s, &focus_at(end, "code.exe", "db.rs", None))
            .expect("focus");
        empty
            .insert_frame(
                s,
                &FrameCapture {
                    ts: 30_000,
                    monitor: 0,
                    width: 100,
                    height: 100,
                    dhash: 1,
                    image: None,
                    image_ext: "png",
                    ocr: vec![OcrBlock {
                        text: "error E0308 mismatched types".into(),
                        x: 0,
                        y: 0,
                        w: 10,
                        h: 10,
                        confidence: 0.9,
                    }],
                    focus: FocusSnapshot {
                        app_id: Some("code.exe".into()),
                        window_title: Some("db.rs".into()),
                        ..Default::default()
                    },
                },
                None,
                0,
            )
            .expect("frame");
        empty.chapters_for_range(0, end + 1).expect("chapters");
        assert_eq!(
            count_table(&empty, "stuck_signal"),
            0,
            "切換沒量到就沒有卡住，不該寫一列 switch_count=0"
        );
    }

    /// 升上來的舊資料庫有三種，而它們拿到的東西必須不一樣。
    ///
    /// 有現貨 → `'1'`，量到的。
    /// 錄過而現在空的 → `'assumed-at-upgrade'`，一張「這一格是升級那天補的」
    /// 的標籤：那顆資料庫答不出它到底存過沒有，而**升級不可以改寫一句關於他
    /// 的資料的舊話**——他昨天刪掉一整天，今天升級，然後被換了一個診斷、換了
    /// 一個下一步。
    /// 一場都沒錄過 → 什麼都不寫。它不是答不出來，它是沒有問題。
    ///
    /// 三種一起斷言，因為只寫其中一條的話另外兩條可以無聲地一起垮：回填寫成
    /// 無條件的 `'1'`，第一、二條照樣綠，而第三條那顆全新的資料庫會被宣布
    /// 「被 forget 忘掉了」。
    #[test]
    fn an_upgraded_database_gets_a_measurement_a_label_or_nothing() {
        let with_content = {
            let conn = Connection::open_in_memory().expect("open");
            conn.execute_batch(MIGRATION_001).expect("v1 schema");
            conn.execute(
                "INSERT INTO frames(ts, width, height, dhash) VALUES(1,1,1,0)",
                [],
            )
            .expect("v1 frame");
            conn.pragma_update(None, "user_version", 1).expect("stamp");
            Db::init(conn).expect("upgrade")
        };
        assert!(
            with_content.ever_stored().expect("stored"),
            "現貨就在那裡，這不是猜的"
        );

        let emptied = {
            let conn = Connection::open_in_memory().expect("open");
            conn.execute_batch(MIGRATION_001).expect("v1 schema");
            // 他在 alpha.32 上按過忘掉：內容沒了，`ever_recorded` 還在。
            conn.execute(
                "INSERT INTO meta(key, value) VALUES('ever_recorded', '1')",
                [],
            )
            .expect("v1 flag");
            conn.pragma_update(None, "user_version", 1).expect("stamp");
            Db::init(conn).expect("upgrade")
        };
        assert!(emptied.ever_recorded().expect("ever"));
        assert!(
            emptied.ever_stored().expect("stored"),
            "他在 alpha.32 上刪掉了一整天。升級不可以把那句話改成「一列都沒存過，\
             先看 capture.enabled」——那是換了一個診斷"
        );
        let label: String = emptied
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'ever_stored'",
                [],
                |r| r.get(0),
            )
            .expect("label");
        assert_eq!(
            label, "assumed-at-upgrade",
            "而它必須看得出來是補的，不是量到的"
        );

        // 標籤會自己過期：第一列真的落地時，觸發器把它覆蓋成量到的那個 1。
        // 觸發器的 `WHEN` 只要寫成 `NOT EXISTS(key='ever_stored')`（不比值），
        // 這顆資料庫就會永遠掛著那張標籤。
        emptied
            .conn
            .execute(
                "INSERT INTO frames(ts, width, height, dhash) VALUES(9,1,1,0)",
                [],
            )
            .expect("real content");
        let label: String = emptied
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'ever_stored'",
                [],
                |r| r.get(0),
            )
            .expect("label");
        assert_eq!(label, "1", "真的存到東西之後，那一格要變成量到的");

        // **第三種：一場都沒錄過的那一顆。** 它沒有 `ever_recorded`，所以那張
        // 標籤一個字都不准寫——寫了的話全新的機器會被告知東西被忘掉了。
        let virgin = {
            let conn = Connection::open_in_memory().expect("open");
            conn.execute_batch(MIGRATION_001).expect("v1 schema");
            conn.pragma_update(None, "user_version", 1).expect("stamp");
            Db::init(conn).expect("upgrade")
        };
        assert!(!virgin.ever_recorded().expect("ever"));
        assert!(
            !virgin.ever_stored().expect("stored"),
            "她一場都沒錄過，這裡沒有什麼答不出來的"
        );
    }

    /// **版號和它描述的結構，要嘛一起落地要嘛一起不落地。**
    ///
    /// 以前 `pragma_update` 在 `tx.commit()` **後面**，中間大約一毫秒。程序在
    /// 那裡被砍掉，檔案裡就是「結構是新的、版號是舊的」——下次開機再跑一次同
    /// 一段，撞 `already exists`，然後那顆資料庫再也打不開。用真的執行檔掃
    /// SIGKILL 的時機，189 次裡中了 1 次。
    ///
    /// 這裡不砍程序，直接證明那個修法**靠的那個性質**：`PRAGMA user_version`
    /// 是進 transaction 的。它要是不進，搬進去就沒有意義。
    ///
    /// **這一條守不住那個修法本身。** 我把 `pragma_update` 搬回 `commit()` 後
    /// 面，整套測試照樣全綠——因為要看見差別就得真的在那一毫秒裡被砍掉。守得住
    /// 的是下面那條：讓那一毫秒的產物**不再致命**。
    #[test]
    fn the_version_stamp_rolls_back_with_the_schema_it_describes() {
        let conn = Connection::open_in_memory().expect("open");
        conn.execute_batch("BEGIN; CREATE TABLE probe(x); PRAGMA user_version = 99;")
            .expect("half a migration");
        let inside: i32 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .expect("read");
        assert_eq!(inside, 99, "transaction 裡要看得到");

        conn.execute_batch("ROLLBACK;").expect("die here");
        let after: i32 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .expect("read");
        assert_eq!(
            after, 0,
            "版號沒跟著退回去的話，下次開機會重跑一段已經跑完的 migration"
        );
        let table: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'probe'",
                [],
                |r| r.get(0),
            )
            .expect("read");
        assert_eq!(table, 0, "而結構也要跟著退——兩者必須同進同退");
    }

    /// 不引 `tempfile`——這個 crate 的相依樹是 `check-no-network.sh` 盯著的資
    /// 產，能不長就不長。同 `retention::tests::Tmp`。
    fn migrate_tmp(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("sister-migrate-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    /// **「結構是新的、版號是舊的」不可以是一顆打不開的資料庫。**
    ///
    /// 那個狀態是真的做得出來的，而且已經在外面了：alpha.32 以前 `commit()` 和
    /// 蓋版號中間有大約一毫秒，程序在那裡被砍掉（`kill -9`、拔電、Windows 更新
    /// 重開）就留下它。用真的執行檔掃 SIGKILL 的時機，189 次裡中了 1 次。
    ///
    /// 上面那個 transaction 只擋得住**以後**。這一條擋的是**已經發生過**的：
    /// 每一段 migration 重跑一次都要是 no-op。撞上去的話是
    /// `table queries already exists` / `no such column: confidence`，然後那顆
    /// 資料庫**每一次開都失敗，永遠**——沒有 `sister repair`，逃生路只有退回舊
    /// 版執行檔，而那正是前向相容閘門在防的事。
    ///
    /// 版號一路退到 0 掃一遍，所以新加一段 migration 忘了寫冪等，這裡會紅。
    #[test]
    fn a_version_stamp_older_than_its_schema_does_not_brick_the_file() {
        let dir = migrate_tmp("brick");
        for stamp in 0..SCHEMA_VERSION {
            let path = dir.join(format!("back-to-{stamp}.db"));
            {
                let mut db = Db::open(&path).expect("build a current one");
                db.conn
                    .execute(
                        "INSERT OR REPLACE INTO meta(key, value) VALUES('created_at', '1')",
                        [],
                    )
                    .expect("生日");
                let sid = db.start_session("test", "0").expect("session");
                // 要有一段中文：003 的 bigram 回填得有東西可以重跑，不然那一段
                // 重跑起來剛好是空的，什麼都證明不了。
                db.conn
                    .execute(
                        "INSERT INTO text_chunks(ts, session_id, source_kind, text)
                         VALUES(1, ?1, 'ocr', '電話號碼')",
                        params![sid],
                    )
                    .expect("seed");
                // bigram 是 Rust 那邊寫的（`insert_chunk_tx`），觸發器不管它，
                // 所以這裡照抄產品的第二步——不然版號退到 4 的那幾輪索引本來
                // 就是空的，下面那條斷言什麼都證明不了。
                let id = db.conn.last_insert_rowid();
                db.conn
                    .execute(
                        "INSERT INTO text_fts_bi(rowid, text) VALUES(?1, ?2)",
                        params![id, cjk_bigrams("電話號碼")],
                    )
                    .expect("seed bigrams");
                // 這一行就是那一毫秒：結構全在，版號被留在後面。
                db.conn
                    .pragma_update(None, "user_version", stamp)
                    .expect("退回去");
            }

            let db = Db::open(&path)
                .unwrap_or_else(|e| panic!("版號停在 {stamp} 的資料庫再也打不開了：{e:#}"));
            assert_eq!(
                db.schema_version().expect("version"),
                SCHEMA_VERSION,
                "版號停在 {stamp}"
            );
            // 重跑一輪不可以把索引寫成兩份——`text_fts_bi` 是唯一一張要在
            // Rust 這邊回填的表，重複的 rowid 會直接炸。
            let bi: i64 = db
                .conn
                .query_row("SELECT COUNT(*) FROM text_fts_bi", [], |r| r.get(0))
                .expect("count");
            assert_eq!(bi, 1, "版號停在 {stamp}：bigram 索引被回填了兩次");
            assert!(
                db.ever_stored().expect("stored"),
                "版號停在 {stamp}：現貨還在，旗標不可以掉"
            );
            // 冪等最容易安靜地弄壞的那一格：001 重跑一次，「她從哪一天開始
            // 記」就被蓋成今天。撞不開的資料庫看得見，這個看不見。
            let born: String = db
                .conn
                .query_row("SELECT value FROM meta WHERE key = 'created_at'", [], |r| {
                    r.get(0)
                })
                .expect("生日");
            assert_eq!(born, "1", "版號停在 {stamp}：她的生日被改掉了");
        }
    }

    /// 兩個 sister 同時升級同一顆資料庫，只有一個該做事，而**兩個都要活著**。
    ///
    /// 門外那一次 `user_version` 兩邊都讀到舊的，所以兩邊都會決定要跑。分勝負
    /// 的是 `BEGIN IMMEDIATE` **之後**的重讀：少了它，後進去的那個拿著一份過期
    /// 的判斷硬跑，撞 `already exists`（真的開兩個行程跑，40 次裡 38 次）。
    ///
    /// 這裡不賭時序：輸的那一邊的處境就是「手上這個 step 的判斷是門外讀到的，
    /// 而檔案已經被別人推上去了」，直接把 `migrate_step(5)` 餵給一顆已經是 5 的
    /// 檔案就是它。
    #[test]
    fn the_sister_who_loses_the_race_steps_aside_instead_of_crashing() {
        let path = migrate_tmp("race").join("sister.db");
        let winner = Db::open(&path).expect("winner");
        assert_eq!(winner.schema_version().expect("v"), SCHEMA_VERSION);

        let mut loser = Db {
            conn: Connection::open(&path).expect("second connection"),
        };
        loser
            .migrate_step(SCHEMA_VERSION)
            .expect("後到的那個要讓開——它撞上的是自己人剛建好的東西");
        assert_eq!(loser.schema_version().expect("v"), SCHEMA_VERSION);

        // 而且只建了一份觸發器，不是兩份。
        let n: i64 = winner
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'trigger' AND name LIKE '%_ever_stored'",
                [],
                |r| r.get(0),
            )
            .expect("count");
        assert_eq!(n as usize, CONTENT_TABLES.len());
    }

    #[test]
    fn fts5_trigram_and_unicode61_are_both_available() {
        // 若 bundled SQLite 缺 FTS5 或 trigram，建表就會失敗——這個測試是保險絲
        let db = test_db();
        let n: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name IN ('text_fts','text_fts_uni')",
                [],
                |r| r.get(0),
            )
            .expect("query sqlite_master");
        assert_eq!(n, 2, "both FTS indexes must exist");
    }

    #[test]
    fn frame_insert_writes_ocr_chunk_and_facts() {
        let mut db = test_db();
        let s = db.start_session("test", "0.0.1").expect("session");
        let f = frame_with_text(
            1_000,
            "chrome.exe",
            "帳單查詢",
            &["本期應繳金額 NT$13,450", "客服專線 0800-123-456"],
        );

        let (frame_id, chunk_id, facts) = db.insert_frame(s, &f, None, 0).expect("insert frame");
        assert!(frame_id > 0);
        assert!(
            chunk_id.is_some(),
            "OCR text must produce a searchable chunk"
        );
        assert!(facts >= 2, "money and phone must be extracted, got {facts}");

        let st = db.stats().expect("stats");
        assert_eq!(st.frames, 1);
        assert_eq!(st.ocr_blocks, 2);
        assert!(st.facts >= 2);
    }

    #[test]
    fn search_finds_cjk_substring_and_cites_source() {
        let mut db = test_db();
        let s = db.start_session("test", "0.0.1").expect("session");
        let f = frame_with_text(
            1_000,
            "chrome.exe",
            "帳單查詢",
            &["本期應繳金額 NT$13,450", "客服專線 0800-123-456"],
        );
        db.insert_frame(s, &f, Some("/tmp/x.webp"), 1234)
            .expect("insert");

        // 這就是 killer query：兩小時後問「客服電話」
        let hits = db.search("客服", 10).expect("search");
        assert!(!hits.is_empty(), "trigram index must match a CJK substring");
        let hit = &hits[0];
        assert_eq!(hit.source_kind, SourceKind::Ocr);
        assert!(hit.text.contains("0800-123-456"));
        // 出處：能一路點回當時那張畫面
        assert!(hit.frame_id.is_some(), "every hit must cite a frame");

        let ctx = db
            .frame_context(hit.frame_id.expect("frame id"))
            .expect("ctx query")
            .expect("frame exists");
        assert_eq!(ctx.image_path.as_deref(), Some("/tmp/x.webp"));
        assert_eq!(ctx.window_title.as_deref(), Some("帳單查詢"));
    }

    /// bigram 是粗篩：「客服部」被拆成「客服」AND「服部」，而這句話兩個
    /// 雙字都有——卻沒有「客服部」這三個字。粗篩必須被真字串擋下來，否則
    /// 她會拿一段沒有人問的話當證據。
    #[test]
    fn a_sentence_that_owns_both_halves_is_not_a_match_for_the_whole_word() {
        let mut db = test_db();
        let s = db.start_session("test", "0.0.1").expect("session");
        let f = frame_with_text(
            1_000,
            "chrome.exe",
            "通訊錄",
            &["客服中心的服部先生今天請假"],
        );
        db.insert_frame(s, &f, None, 1).expect("insert");

        assert!(
            db.search("客服部", 10).expect("search").is_empty(),
            "「客服中心的服部先生」同時有「客服」和「服部」，但沒有「客服部」"
        );
        // 反面：真的有這三個字的時候還是查得到，否則上面那條只是把功能關掉。
        let g = frame_with_text(2_000, "chrome.exe", "分機表", &["客服部 分機 201"]);
        db.insert_frame(s, &g, None, 2).expect("insert");
        assert_eq!(db.search("客服部", 10).expect("search").len(), 1);
    }

    /// `bigram_query` 只看得懂 CJK，會把 `hello` 這種詞整個丟掉。如果不用
    /// 真字串再篩一次，查「客服 hello」就會退化成查「客服」。
    #[test]
    fn the_english_half_of_a_mixed_query_still_has_to_hold() {
        let mut db = test_db();
        let s = db.start_session("test", "0.0.1").expect("session");
        let f = frame_with_text(1_000, "chrome.exe", "工單", &["客服 回覆 已結案"]);
        db.insert_frame(s, &f, None, 1).expect("insert");

        assert!(
            db.search("客服 hello", 10).expect("search").is_empty(),
            "bigram 只驗得了「客服」，`hello` 這個條件不能就這樣消失"
        );

        let g = frame_with_text(2_000, "chrome.exe", "工單", &["客服 hello world"]);
        db.insert_frame(s, &g, None, 2).expect("insert");
        let hits = db.search("客服 hello", 10).expect("search");
        assert_eq!(hits.len(), 1);
        assert!(hits[0].text.contains("hello"));
    }

    /// 查一個不存在的字串時，bigram 把候選集整個看完了（0 列），那就是真的
    /// 沒有——不必再掃一次全表。這條測試釘的是「不要退回 LIKE」，因為那正是
    /// 45 天語料上 96.7 ms 的來源。
    #[test]
    fn a_confident_miss_does_not_pay_for_a_full_scan() {
        let mut db = test_db();
        let s = db.start_session("test", "0.0.1").expect("session");
        let f = frame_with_text(1_000, "chrome.exe", "帳單", &["本期應繳金額"]);
        db.insert_frame(s, &f, None, 1).expect("insert");

        assert!(db.search("客服專線", 10).expect("search").is_empty());

        // LIKE 掃描只看得到最近 `LIKE_SCAN_DAYS` 天；bigram 沒有這個窗。
        // 把唯一一筆資料放到窗外，如果答案仍然正確，就證明答案不是 LIKE 給的。
        let old = 1_000 - (LIKE_SCAN_DAYS + 5) * 86_400_000;
        let mut db2 = test_db();
        let s2 = db2.start_session("test", "0.0.1").expect("session");
        let g = frame_with_text(old, "chrome.exe", "帳單", &["客服專線 0800"]);
        db2.insert_frame(s2, &g, None, 1).expect("insert");
        assert_eq!(
            db2.search("客服專線", 10).expect("search").len(),
            1,
            "bigram 應該直接答得出來，而不是靠掃描——掃描根本看不到窗外那天"
        );
    }

    #[test]
    fn search_matches_english_whole_words() {
        let mut db = test_db();
        let s = db.start_session("test", "0.0.1").expect("session");
        let f = frame_with_text(
            2_000,
            "code.exe",
            "editor",
            &["ERR_CONNECTION_REFUSED at dns"],
        );
        db.insert_frame(s, &f, None, 0).expect("insert");

        assert!(!db.search("dns", 10).expect("search").is_empty());
        assert!(
            !db.search("ERR_CONNECTION_REFUSED", 10)
                .expect("search")
                .is_empty()
        );
    }

    #[test]
    fn search_is_empty_for_blank_query_and_survives_fts_metacharacters() {
        let mut db = test_db();
        let s = db.start_session("test", "0.0.1").expect("session");
        db.insert_frame(s, &frame_with_text(1, "a", "b", &["hello world"]), None, 0)
            .expect("insert");

        assert!(db.search("", 10).expect("empty query").is_empty());
        assert!(db.search("   ", 10).expect("blank query").is_empty());
        // 這些字元在 FTS5 裡有特殊意義，不跳脫會直接噴語法錯誤
        for q in ["\"", "a OR b", "NEAR(a b)", "-hello", "a*", "(((", "^x"] {
            let r = db.search(q, 10);
            assert!(r.is_ok(), "query {q:?} must not error: {:?}", r.err());
        }
    }

    /// 「剛剛發生什麼事」不能靠比對那七個字。這一支回的是最新的幾列，
    /// 新的在前，而且照樣掛著出處。
    #[test]
    fn recent_answers_with_the_newest_things_first() {
        let mut db = test_db();
        let s = db.start_session("test", "0.0.1").expect("session");
        for (ts, line) in [
            (1_000, "早上的信"),
            (2_000, "中午的單子"),
            (3_000, "剛開的網頁"),
        ] {
            let f = frame_with_text(ts, "chrome.exe", "視窗", &[line]);
            db.insert_frame(s, &f, Some("/tmp/x.webp"), 1)
                .expect("insert");
        }

        let hits = db.recent(2).expect("recent");
        assert_eq!(hits.len(), 2, "要幾件就給幾件");
        assert!(hits[0].text.contains("剛開的網頁"), "最新的排最前面");
        assert!(hits[1].text.contains("中午的單子"));
        assert!(hits[0].frame_id.is_some(), "時間問題的答案一樣要指得回去");

        // 反面：這個問題走搜尋是答不出來的——那正是它要存在的理由。
        assert!(
            db.search("剛剛發生什麼事", 10).expect("search").is_empty(),
            "螢幕上沒出現過這七個字，比對字就只會空手而回"
        );
    }

    /// 那張截圖的下一題：**同時帶時間和內容的問題。**
    ///
    /// 這種問題走的是比對那條路（句子裡有他真正想問的東西），而中文沒有空白，
    /// 所以整句話會被當成一整串子字串去找——「剛剛那個網頁」這六個字誰的螢幕
    /// 上都不會有。這一條釘的是 [`crate::question::terms`] 和 [`Db::search`]
    /// 接起來之後的行為，不是任一邊自己的行為：bug 出在中間那個縫。
    #[test]
    fn a_question_that_says_both_when_and_what_still_finds_the_what() {
        let mut db = test_db();
        let s = db.start_session("test", "0.0.1").expect("session");
        let f = frame_with_text(
            1_000,
            "chrome.exe",
            "視窗",
            &["記得順便問客服有沒有優惠方案"],
        );
        db.insert_frame(s, &f, Some("/tmp/x.webp"), 1)
            .expect("insert");

        assert!(
            !db.search("優惠方案", 10).expect("search").is_empty(),
            "單獨問這四個字本來就找得到"
        );
        assert!(
            db.search("剛剛那個優惠方案", 10)
                .expect("search")
                .is_empty(),
            "整句丟進去是找不到的——這正是要修的東西，不是要保留的行為"
        );

        let terms = crate::question::terms("剛剛那個優惠方案");
        assert_eq!(terms, "優惠方案");
        assert!(
            !db.search(terms, 10).expect("search").is_empty(),
            "加了「剛剛」不該讓她變成什麼都找不到"
        );
    }

    /// 畫面不動的時候同一句話會被寫進好幾列。照抄的話「最近十件事」會變成
    /// 同一句話講十遍——那看起來像她壞掉了，而不是像她很專心。
    #[test]
    fn recent_collapses_a_screen_that_did_not_change() {
        let mut db = test_db();
        let s = db.start_session("test", "0.0.1").expect("session");
        for ts in [1_000, 2_000, 3_000, 4_000] {
            let f = frame_with_text(ts, "code.exe", "editor", &["一直沒變的那一行"]);
            db.insert_frame(s, &f, None, 1).expect("insert");
        }
        let g = frame_with_text(5_000, "chrome.exe", "視窗", &["換了一件事"]);
        db.insert_frame(s, &g, None, 1).expect("insert");

        let hits = db.recent(10).expect("recent");
        assert_eq!(hits.len(), 2, "四列一模一樣的字只算一件事");
        assert!(hits[0].text.contains("換了一件事"));
    }

    /// 「剛剛」不可以變成一次全表排序。`idx_chunk_ts` 是 `(ts)`，rowid 隱含在
    /// 索引尾巴上，所以 `ORDER BY ts DESC, id DESC` 應該直接倒著掃索引就好。
    ///
    /// 這條界線很細：把第二個鍵寫成 `id ASC` 就會得到
    /// `USE TEMP B-TREE FOR LAST TERM OF ORDER BY`——實際試過。一個月的資料上
    /// 那是幾百毫秒，而每問一次「剛剛」就走一遍，卻不會有任何症狀，只是慢。
    #[test]
    fn recent_reads_the_index_backwards_instead_of_sorting_the_table() {
        let db = test_db();
        let plan: Vec<String> = db
            .conn()
            .prepare(
                "EXPLAIN QUERY PLAN SELECT id, ts, source_kind, frame_id, app_id,
                 window_title, url, text FROM text_chunks ORDER BY ts DESC, id DESC LIMIT ?1",
            )
            .expect("prepare")
            .query_map([240], |r| r.get::<_, String>(3))
            .expect("plan")
            .flatten()
            .collect();
        let plan = plan.join(" | ");
        assert!(
            !plan.to_uppercase().contains("TEMP B-TREE"),
            "「剛剛」在排整張表：{plan}"
        );
        assert!(plan.contains("idx_chunk_ts"), "沒有用到時間索引：{plan}");
    }

    /// 一台還沒錄過任何東西的機器上，這一支要安靜地回空的，不是報錯。
    #[test]
    fn recent_on_an_empty_database_is_just_empty() {
        let db = test_db();
        assert!(db.recent(10).expect("recent must not error").is_empty());
    }

    fn ask(db: &Db, ts: Millis, question: &str, hits: usize) -> i64 {
        db.log_query(&QueryLogEntry {
            ts,
            question,
            shape: "keywords",
            hits,
            latency_ms: 3,
            source: "test",
        })
        .expect("log_query")
    }

    /// PHASES.md Phase 1 有一條「檢索 < 100ms」，而在這之前沒有任何東西量得
    /// 出來——每一題花了幾毫秒從第一天就存著，只是沒有人讀。
    ///
    /// 這裡驗的是**平均值答不了這個問題**：19 題很快、1 題 4 秒，平均 203 ms
    /// 是一個從來沒有發生過的數字，而它同時掩蓋了「平常很快」和「有一次很慢」
    /// 這兩件真的該知道的事。
    #[test]
    fn one_slow_question_must_not_hide_behind_the_average() {
        let db = test_db();
        for i in 0..19 {
            db.log_query(&QueryLogEntry {
                ts: 1_000 + i,
                question: "電話",
                shape: "keywords",
                hits: 1,
                latency_ms: 10,
                source: "test",
            })
            .expect("log");
        }
        db.log_query(&QueryLogEntry {
            ts: 2_000,
            question: "電話",
            shape: "keywords",
            hits: 1,
            latency_ms: 4_000,
            source: "test",
        })
        .expect("log");

        let s = db.query_log_stats().expect("stats");
        assert_eq!(s.total, 20);
        assert_eq!(s.p50_ms, 10, "平常很快");
        assert_eq!(s.p95_ms, 4_000, "但最糟的那一次很慢，而它藏不住");
        assert_eq!(s.slow, 1, "超過門檻的題數要數得出來");
    }

    /// 一筆都沒找到的那些題目**是題庫裡最貴的資料**。找得回來的那些只證明她
    /// 現在能做什麼；找不回來的那些，才是下一版該修的東西。少記它們的話，
    /// 題庫會變成一份「她答得出來的問題」的清單，而那份清單沒有用。
    #[test]
    fn a_question_that_found_nothing_is_still_worth_keeping() {
        let db = test_db();
        ask(&db, 1_000, "客服電話", 0);
        let stats = db.query_log_stats().expect("stats");
        assert_eq!(stats.total, 1);
        assert_eq!(stats.empty, 1, "答不出來的那一題沒被記下來");
        assert_eq!(db.query_log(10).expect("log")[0].question, "客服電話");
    }

    #[test]
    fn replay_question_window_is_half_open_complete_and_stably_oldest_first() {
        let db = test_db();
        ask(&db, 999, "before", 1);
        let first = ask(&db, 1_000, "same", 0);
        let second = ask(&db, 1_000, "same", 2);
        let third = ask(&db, 1_999, "last", 1);
        ask(&db, 2_000, "after", 1);

        let rows = db.query_log_between(1_000, 2_000).expect("window");
        assert_eq!(
            rows.iter().map(|row| row.id).collect::<Vec<_>>(),
            vec![first, second, third]
        );
        assert_eq!(rows[0].question, rows[1].question, "重複問法也各留一題");
        assert!(db.query_log_between(2_000, 1_000).is_err());
    }

    /// 點下去＝幫這一題標了正解，而 `rank` 說出排序把它放在第幾個。
    #[test]
    fn clicking_a_source_is_the_answer_key() {
        let db = test_db();
        let q = ask(&db, 1_000, "帳單", 5);
        db.log_click(q, 42, 0).expect("click");
        db.log_click(q, 77, 3).expect("click");

        assert_eq!(db.query_log(10).expect("log")[0].clicks, 2);
        assert_eq!(
            db.query_log_stats().expect("stats").clicked,
            1,
            "同一題點兩下不算兩題"
        );
    }

    /// 「點開出處」的分母只能是**點得動出處的那些題**。
    ///
    /// 終端機上沒有出處可以點，所以每一題 `sister query` 都會把那個比例往下
    /// 拉——而開發的時候跑幾十次是常態。分母用總題數的話，這個指標結構性地
    /// 永遠趨近 0%，然後就沒有人再看它第二眼；而它本來是檢索品質唯一不用人工
    /// 標註就拿得到的訊號。
    #[test]
    fn the_terminal_has_no_sources_to_click_so_it_does_not_belong_in_the_denominator() {
        let db = test_db();
        for i in 0..9 {
            db.log_query(&QueryLogEntry {
                ts: 1_000 + i,
                question: "電話",
                shape: "keywords",
                hits: 1,
                latency_ms: 1,
                source: SOURCE_CLI,
            })
            .expect("log");
        }
        let q = db
            .log_query(&QueryLogEntry {
                ts: 2_000,
                question: "帳單",
                shape: "keywords",
                hits: 3,
                latency_ms: 1,
                source: SOURCE_DESKTOP,
            })
            .expect("log");
        db.log_click(q, 42, 0).expect("click");

        let s = db.query_log_stats().expect("stats");
        assert_eq!(s.total, 10);
        assert_eq!(s.clicked, 1);
        assert_eq!(s.clickable, 1, "十題裡只有一題點得動出處");
        // 1/10 = 10% 讀起來像「她九成的答案都沒用」；1/1 才是真的發生過的事。
        assert_eq!(100 * s.clicked / s.clickable, 100);
    }

    /// 一題都沒點，和點了，是兩件不同的事。前者也是訊號：她給了答案，
    /// 但沒有一筆值得點開。
    #[test]
    fn an_unclicked_question_is_not_a_clicked_one() {
        let db = test_db();
        ask(&db, 1_000, "沒人點的問題", 5);
        assert_eq!(db.query_log_stats().expect("stats").clicked, 0);
    }

    /// 「忘掉那一段時間」如果留下他在那段時間打進搜尋框的字，那句話就是半真的
    /// ——而螢幕上的東西沒了、他自己打的字還在，是比較糟的那一半。
    #[test]
    fn forgetting_a_stretch_of_time_takes_the_questions_too() {
        let mut db = test_db();
        ask(&db, 1_000, "要被忘掉的", 1);
        ask(&db, 9_000, "在範圍外的", 1);

        // `None` 和底下那一支 `forget` 一樣：這一條測的是題庫，不是畫面檔。
        let preview = db.forget_preview(500, 2_000, None).expect("preview");
        assert_eq!(preview.queries_deleted, 1, "預覽沒把題庫算進去");

        let report = db.forget(500, 2_000, None).expect("forget");
        assert_eq!(report.queries_deleted, 1);
        let left = db.query_log(10).expect("log");
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].question, "在範圍外的");
    }

    /// 題目消失，掛在它上面的點擊也要消失。留著的話，`query_clicks` 會累積
    /// 一堆指向不存在題目的列——那是一份沒有人知道自己還留著的紀錄。
    #[test]
    fn forgetting_a_question_takes_its_clicks() {
        let mut db = test_db();
        let q = ask(&db, 1_000, "會被忘掉", 2);
        db.log_click(q, 7, 0).expect("click");
        db.forget(500, 2_000, None).expect("forget");

        let orphans: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM query_clicks", [], |r| r.get(0))
            .expect("count");
        assert_eq!(orphans, 0, "題目沒了，點擊卻還掛在那裡");
    }

    /// 標記是個開關：按下去、收回來、再按一次，讀回來的都是**現在**的狀態。
    ///
    /// 回傳的兩格各釘一半，因為兩個呼叫端問的不是同一題。`marked` 是現在的狀
    /// 態，按第二次和按第一次一樣——字母人那顆按鈕照它上色。`changed` 是這一次
    /// 有沒有真的動到東西。
    ///
    /// 上一版只有前者，而且它就是傳進去的參數送回來：於是「真的收回了一個標
    /// 記」和「這一題本來就沒標」印出一模一樣的「○ 收回了」。`EXISTS` 那道關
    /// 只擋掉「號碼不存在」，擋不掉「號碼存在但本來就沒標」——而 `sister
    /// queries` 現在把 `#N` 印在旁邊，後者才是比較常打錯的那一種。
    #[test]
    fn a_mark_is_a_switch_not_a_counter() {
        let db = test_db();
        let q = ask(&db, 1_000, "我早就忘了的那件事", 2);
        assert!(!db.query_log(1).expect("log")[0].marked(), "一開始不該標著");

        let first = db.mark_query(q, true).expect("mark");
        assert!(first.marked && first.changed, "第一次按下去是有動到的");
        let again = db.mark_query(q, true).expect("再按一次");
        assert!(again.marked, "重按要回同一個狀態");
        assert!(!again.changed, "重按沒有動到任何東西");
        assert!(db.query_log(1).expect("log")[0].marked());
        assert_eq!(
            db.query_log_stats().expect("stats").marked,
            1,
            "重按不該變成兩題"
        );

        let off = db.mark_query(q, false).expect("unmark");
        assert!(!off.marked && off.changed, "這一次真的收回了一個標記");
        let off_again = db.mark_query(q, false).expect("再收一次");
        assert!(!off_again.marked, "還是沒標");
        assert!(
            !off_again.changed,
            "本來就沒標的題再收一次，不可以講得像收回了一個標記"
        );
        assert!(!db.query_log(1).expect("log")[0].marked());
        assert_eq!(db.query_log_stats().expect("stats").marked, 0);
    }

    /// **「幾次」和「哪幾題」要從同一個地方算出來。**
    ///
    /// 上一版計數是 `COUNT(*) FROM query_marks`（不 join），清單是
    /// `query_marks JOIN queries`（inner join）。一列孤兒標記——外鍵關著的時候
    /// 手動改資料庫就造得出來——會讓「★ 魔法時刻：1 次」配著一張空清單，而
    /// `ops.rs` 那句「撈不回題目就讓它炸」的保險，因為 inner join 早就把它濾掉
    /// 了，永遠不會響。這正是這個 repo 已經寫進規則、然後又犯過一次的那一條：
    /// 兩次獨立的查找會指到不同的東西。
    #[test]
    fn the_count_and_the_list_are_the_same_question_asked_once() {
        let db = test_db();
        let q = ask(&db, 1_000, "真的有這一題", 1);
        db.mark_query(q, true).expect("mark");

        // 外鍵關掉才造得出孤兒——那正是「有人動了資料庫」的情境。
        db.conn
            .pragma_update(None, "foreign_keys", false)
            .expect("fk off");
        db.conn
            .execute("INSERT INTO query_marks(query_id, ts) VALUES(999, 1)", [])
            .expect("orphan");
        db.conn
            .pragma_update(None, "foreign_keys", true)
            .expect("fk on");

        assert_eq!(
            db.query_log_stats().expect("stats").marked as usize,
            db.marked_queries(100).expect("marked").len(),
            "數出來的次數和列得出來的實例對不上"
        );
    }

    /// **重按不會把時間往前推。**
    ///
    /// `mark_query` 上寫著「標記的時間是他第一次認出來那一刻」，而撐著那句話的
    /// 只有 `INSERT OR IGNORE` 裡那三個字母——改成 `OR REPLACE` 之後，每按一次
    /// 就把時間蓋成現在，而上面那句註解會原封不動地留著說謊。（這個 repo 已經
    /// 犯過一次「閘門文件裡寫的不變式是沒有人在執行的」。）
    ///
    /// 兩次 `now_ms()` 在測試裡常常同一毫秒，所以中間直接把時間改老，讓「有沒
    /// 有被蓋掉」變成一個看得見的差別。
    #[test]
    fn pressing_it_again_does_not_move_the_moment_he_first_recognised_it() {
        let db = test_db();
        let q = ask(&db, 1_000, "第一次就認出來了", 1);
        db.mark_query(q, true).expect("mark");

        const FIRST: Millis = 42;
        db.conn
            .execute("UPDATE query_marks SET ts = ?1", params![FIRST])
            .expect("backdate");
        db.mark_query(q, true).expect("再按一次");

        assert_eq!(
            db.marked_queries(1).expect("marked")[0].marked_ts,
            Some(FIRST),
            "重按把「第一次認出來」的時間蓋掉了"
        );
    }

    /// **標一題不存在的，要炸。**
    ///
    /// 外鍵只擋得住 `INSERT` 那半邊；`DELETE FROM query_marks WHERE query_id = 999`
    /// 在 SQL 裡是完全合法的「刪了 0 列」。少了那道 `EXISTS` 檢查，`sister mark
    /// --undo --id 999` 會回一句「收回了」——而他打錯號碼、或者那一題剛被
    /// `forget` 帶走，兩種都會拿到那句話。
    #[test]
    fn marking_a_question_that_is_not_there_is_an_error_both_ways() {
        let db = test_db();
        ask(&db, 1_000, "真的有這一題", 1);
        for on in [true, false] {
            let err = db.mark_query(999, on).expect_err("不存在的題號要炸");
            assert!(
                err.to_string().contains("999"),
                "錯誤訊息要講出是哪一個題號：{err}"
            );
        }
    }

    /// 「剛剛那一題」在題庫的第一列上，而 `sister mark` 不帶參數標的就是它。
    ///
    /// 兩邊各自寫一次 `ORDER BY` 的話，同一秒問完兩題（那正是連按 Enter 的
    /// 人）就會分岔：清單上第一列是 A，`mark` 標到的是 B，而兩個畫面都說自己
    /// 對。這一條把它們釘在一起。
    #[test]
    fn the_last_question_is_the_one_at_the_top_of_the_log() {
        let db = test_db();
        // 同一個 ts，靠 id 決勝負——`ts DESC, id DESC` 兩邊都要是同一句。
        ask(&db, 1_000, "先問的", 1);
        let second = ask(&db, 1_000, "後問的", 1);

        let last = db.last_query().expect("last").expect("有問過");
        assert_eq!(last.id, second);
        assert_eq!(last.id, db.query_log(1).expect("log")[0].id);
        assert_eq!(last.question, "後問的");
    }

    /// 沒問過的時候 `last_query` 是 `None`，不是第 0 題。
    #[test]
    fn a_log_with_nothing_in_it_has_no_last_question() {
        assert!(test_db().last_query().expect("last").is_none());
    }

    /// 退場條件寫的是「記錄實例」，所以拿得出來的不能只有一個數字。
    ///
    /// 兩個時間都要在：她答對的那一刻（`ts`，「七天內」數的是它），和他認出來
    /// 的那一刻（`marked_ts`）。
    #[test]
    fn the_instances_come_back_with_both_of_their_timestamps() {
        let db = test_db();
        let a = ask(&db, 1_000, "第一次神奇", 2);
        let b = ask(&db, 2_000, "第二次神奇", 5);
        db.mark_query(a, true).expect("mark a");
        db.mark_query(b, true).expect("mark b");

        let marked = db.marked_queries(10).expect("marked");
        assert_eq!(marked.len(), 2);
        assert_eq!(marked[0].id, b, "她比較晚答對的那一題排前面");
        for m in &marked {
            assert_eq!(m.ts, if m.id == a { 1_000 } else { 2_000 });
            assert!(
                m.marked_ts.expect("標記自己的時間沒寫進去") > 0,
                "標記時間要是真的時間"
            );
            assert!(m.question.contains("神奇"));
        }
        assert_eq!(marked.iter().find(|m| m.id == b).expect("b").hits, 5);
    }

    /// **`limit` 砍掉的必須是離那七天最遠的那幾列。**
    ///
    /// 排序照 `q.ts`（她答對的時候）而不是 `m.ts`（他按下去的時候）。一個禮拜
    /// 後才回頭補標的舊題，「按下去的時間」是最新的——照 `m.ts` 排它會站到第一
    /// 個，然後把真正落在那七天裡的實例擠出 `limit` 外面。退場條件數的是使用的
    /// 七天，不是回想的七天。
    #[test]
    fn the_limit_cuts_the_oldest_question_not_the_oldest_press() {
        let db = test_db();
        let old = ask(&db, 1_000, "很久以前問的", 1);
        let recent = ask(&db, 9_000, "這禮拜問的", 1);
        // 讓「按下去的時間」和「問的時間」剛好相反：舊題後來才補標。
        db.mark_query(recent, true).expect("mark recent");
        db.mark_query(old, true).expect("mark old");
        for (q, ts) in [(recent, 100), (old, 200)] {
            db.conn
                .execute(
                    "UPDATE query_marks SET ts = ?2 WHERE query_id = ?1",
                    params![q, ts],
                )
                .expect("backdate");
        }

        let one = db.marked_queries(1).expect("marked");
        assert_eq!(one.len(), 1);
        assert_eq!(
            one[0].id, recent,
            "limit 砍掉的該是她比較早答對的那一題，不是比較早被按的那一題"
        );
    }

    /// **`ever_marked` 的兩面都要釘住，而 `false` 那一面比較貴。**
    ///
    /// 這個旗標存在的理由是把「他從來沒按過」和「他按過、現在一格都不剩」分
    /// 開，而只釘 `true` 那一面的話，一個永遠回 `true` 的實作也是綠的——然後
    /// 一台剛裝好、一次都沒按過的機器會被告知「你按過，但現在一個都不剩」，
    /// 也就是指控一場沒發生過的刪除。那正是 `scripts/check-erased-db.sh` 花
    /// 九百多行在抓的那種話，而 `sister queries` 不在那支腳本的迴圈裡。
    ///
    /// 所以這一條從**沒按過**開始走：開一顆、問一題、確認它還是 `false`。
    #[test]
    fn a_machine_where_nobody_pressed_must_not_claim_he_did() {
        let db = test_db();
        let q = ask(&db, 1_000, "問過但沒按", 2);
        assert!(
            !db.ever_marked().expect("ever_marked"),
            "一次都沒按過，這一格不可以是 true"
        );

        db.mark_query(q, true).expect("mark");
        assert!(db.ever_marked().expect("ever_marked"), "按了就要記得");

        // 收回不會把它翻回去——那正是它和 `marked` 的差別，也是它唯一的用處。
        db.mark_query(q, false).expect("undo");
        assert_eq!(db.query_log_stats().expect("stats").marked, 0);
        assert!(
            db.ever_marked().expect("ever_marked"),
            "收回的是標記，不是「他按過」這件事"
        );
    }

    /// 外送紀錄那兩種空：從來沒送，和送過、列被清掉了。
    ///
    /// 只釘 `true` 那一面的話，一台新機器也會被說成「送過但被清掉了」。
    #[test]
    fn never_sent_and_sent_then_gone_are_not_the_same_zero() {
        let mut db = test_db();
        assert!(
            !db.ever_brain_outbound().expect("ever"),
            "一台新機器不可以說自己送過"
        );

        db.insert_brain_skip(1_000, "no_consent", None, "還沒簽")
            .expect("skip");
        assert!(
            !db.ever_brain_outbound().expect("skip is not send"),
            "沒送出去的原因不是一次外送"
        );

        let mut db = db;
        db.insert_brain_outbound(&OutboundInsert {
            ts: 2_000,
            day_key: "1970-01-01",
            command: "claude",
            args: &[],
            segment_core_start: None,
            chars_sent: 12,
            truncated: false,
            outcome: "success",
            duration_ms: 10,
            error: None,
            role: "interpreter",
        })
        .expect("insert");
        assert!(db.ever_brain_outbound().expect("ever after send"));

        db.conn
            .execute("DELETE FROM brain_outbound", [])
            .expect("wipe rows");
        assert!(
            db.ever_brain_outbound().expect("ever after wipe"),
            "列沒了仍要記得送過——不然會跟從來沒送長得一樣"
        );
    }

    /// 忘掉那一段時間，掛在那幾題上的標記要跟著走。
    ///
    /// 這裡靠的是 `ON DELETE CASCADE` + `PRAGMA foreign_keys=ON`，而那個 pragma
    /// 是在 [`Db::init`] 裡設的——不是 schema 的一部分，是連線的一部分。哪天有
    /// 人開了一條沒設它的連線，這一條會紅在「標記還在」。
    #[test]
    fn a_forgotten_question_takes_its_mark_with_it() {
        let mut db = test_db();
        let q = ask(&db, 1_000, "會被忘掉", 2);
        db.mark_query(q, true).expect("mark");
        db.forget(500, 2_000, None).expect("forget");

        let orphans: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM query_marks", [], |r| r.get(0))
            .expect("count");
        assert_eq!(orphans, 0, "題目沒了，標記卻還掛在那裡");
        assert_eq!(db.query_log_stats().expect("stats").marked, 0);
    }

    /// **點開出處不等於「我本來已經忘了」。**
    ///
    /// 這兩個訊號在 `sister queries` 上是相鄰的兩行，而它們講的是相反的事：
    /// 點開證據最常發生在她答錯、或他在查核的時候。任何一天有人為了「少一張
    /// 表」把它們併在一起，這一條會紅。
    #[test]
    fn a_click_is_not_a_magic_moment() {
        let db = test_db();
        let clicked = ask(&db, 1_000, "我點開去查核的", 3);
        let magic = ask(&db, 2_000, "我本來已經忘了的", 1);
        db.log_click(clicked, 7, 0).expect("click");
        db.mark_query(magic, true).expect("mark");

        let s = db.query_log_stats().expect("stats");
        assert_eq!(s.clicked, 1);
        assert_eq!(s.marked, 1);
        let marked = db.marked_queries(10).expect("marked");
        assert_eq!(marked.len(), 1);
        assert_eq!(marked[0].id, magic, "被標記的是那一題，不是被點開的那一題");

        // **兩欄都要斷言，而且要往相反的方向。** 只寫「被點開的那一題沒有被標
        // 記」的話，一個把 `marked` 讀成永遠 `false` 的改法會讓這一條全綠——而
        // 那正是這條測試要擋的東西的另一半（實測過：那個突變殺得掉隔壁三條，
        // 殺不掉這一條）。
        let log = db.query_log(10).expect("log");
        let row = |id: i64| log.iter().find(|r| r.id == id).expect("在清單上");
        assert_eq!(row(clicked).clicks, 1);
        assert!(!row(clicked).marked(), "點開不會自己變成標記");
        assert_eq!(row(magic).clicks, 0, "標記不會自己變成點擊");
        assert!(row(magic).marked(), "標了的那一題在清單上沒有標記");
    }

    /// **一場全程被排除規則擋掉的錄製，不是一顆空的資料庫。**
    ///
    /// `sister stats` 上那句「她錄過，而現在這顆資料庫裡一列都不剩」第一版問的
    /// 是 `frames == 0 && chunks == 0`，而那個條件在這一場上也成立：keepassxc
    /// 在前景一整段，一張畫面都沒留。可是那一頁上有「工作階段 1」「輸入 6」
    /// 「系統 7」和一整段排除稽核——於是那句話印在一頁有數字的東西上面，還把
    /// 「規則擋掉的」說成「你刪掉的」。兩句都是假的，而它們的下一步剛好相反：
    /// 一個要他去改 config，一個要他知道東西真的沒了。
    #[test]
    fn a_recording_the_rules_blocked_end_to_end_is_not_an_empty_database() {
        let mut db = test_db();
        let s = db.start_session("test", "0.0.1").expect("session");
        // 她開了、看了、什麼都沒留下來——因為規則擋著。
        db.insert_system(
            s,
            &SystemEvent {
                ts: 1_000,
                kind: SystemKind::Excluded,
                detail: Some("excluded app: keepassxc".into()),
            },
        )
        .expect("blocked");

        let st = db.stats().expect("stats");
        assert_eq!((st.frames, st.chunks), (0, 0), "畫面和文字真的都是 0");
        assert!(
            !st.nothing_recorded_left(),
            "但那一頁上有工作階段、有系統事件、有一整段排除稽核——不是「一列都不剩」"
        );
        assert!(
            !db.exclusion_audit().expect("audit").is_empty(),
            "而擋過這件事本身就是那一頁最重要的內容"
        );

        // 對照：真的一列都不剩。
        db.forget(0, 2_000, None).expect("forget");
        db.end_session(s).expect("end");
        assert!(
            db.stats().expect("stats").nothing_recorded_left(),
            "清空之後才算一列都不剩"
        );
        assert!(db.ever_recorded().expect("ever"), "而她錄過這件事還在");

        // **他清空完的下一個指令，不可以把整個刪除蓋掉。**
        //
        // 「真的沒了嗎」是刪完最自然的下一句話，而它會在 `queries` 裡留一列。
        // 上一版把那張表算進這個述詞（理由聽起來很對：那也是一列），於是這一
        // 問就讓它翻成 false、`Emptiness` 掉到 `Fresh`，`stats` 的 ⚠ 整段消失、
        // `doctor` 說「（還沒有任何內容）」、`facts` 說「她還沒錄過」。他一秒
        // 前才親手刪掉的一整天，被他自己的下一個指令否認了。
        db.log_query(&QueryLogEntry {
            ts: 3_000,
            question: "真的沒了嗎",
            shape: "keywords",
            hits: 0,
            latency_ms: 1,
            source: "cli",
        })
        .expect("log");
        let st = db.stats().expect("stats");
        assert_eq!(st.queries, 1, "那一問真的留了一列下來");
        assert!(
            st.nothing_recorded_left(),
            "但他打的字不是她記的東西——問一句話不會讓那一整天回來"
        );
    }

    /// **當掉的那一場撐過了 `forget`，而它是一個空殼。**
    ///
    /// `delete_empty_sessions` 不准碰 `ended_at IS NULL AND id = MAX(id)` 的那
    /// 一列，因為那可能是**此刻正在錄**的那一場（`prune` 在錄製迴圈裡
    /// 自己跑一次，刪掉它會讓下一列撞上外鍵）。可是那個條件同時也是**當掉的那
    /// 一場**長的樣子，而兩者在 SQL 裡分不出來——所以最後一場當掉之後跑
    /// `forget`，那一列會留下來。
    ///
    /// 這一條和上面那條是同一課的兩面：`queries` 會在刪除**之後**長出來，
    /// `sessions` 會**撐過**刪除。兩種都會讓「還剩不剩」答成「還剩」，而下場
    /// 一模一樣——`Emptiness` 讓開 `Erased`、接到最寬的 `Fresh`，三個畫面一起
    /// 說「她還沒錄過」，對一個一秒前才親手刪掉一整天的人。
    #[test]
    fn a_crashed_session_that_outlives_the_forget_is_a_shell_not_content() {
        let mut db = test_db();
        let s = db.start_session("test", "0.0.1").expect("session");
        let f = frame_with_text(1_000, "chrome.exe", "帳單", &["本期應繳 NT$1,234"]);
        db.insert_frame(s, &f, None, 0).expect("insert");

        // 她當掉了：`end_session` 從來沒被呼叫，所以 `ended_at` 留在 NULL。
        db.forget(0, 2_000, None).expect("forget");

        let st = db.stats().expect("stats");
        assert_eq!(st.sessions, 1, "那一列真的還在——這正是這條要守的前提");
        assert_eq!((st.frames, st.chunks, st.facts), (0, 0, 0), "而東西全走了");
        assert_eq!(
            db.crash_audit(None).expect("crash").rows_unfinished,
            1,
            "它得留著，`零當機` 那一列就是靠它才數得出當機"
        );
        assert!(
            st.nothing_recorded_left(),
            "但一個空殼不是她記下來的東西——留著它不可以讓那一整天看起來沒被刪過"
        );
        // 刪不掉就要講得出來。三個 surface 靠這一句決定要不要多講一行。
        assert!(
            st.only_session_shells_left(),
            "那一列還在，而且是個殼——`forget`、`stats`、時間軸都要說得出這件事"
        );
    }

    /// **他清空之後，她再開始錄的第一毫秒。**
    ///
    /// `Recorder::new` 的第一件事就是寫一列 `session_start`。那一列曾經足以讓
    /// 這個述詞翻成 false——於是 `Emptiness` 讓開 `Erased`、接到最寬的 `Fresh`，
    /// `facts` 說「她還沒錄過」、`doctor` 說「還沒有任何內容」、`stats` 上那個
    /// ⚠ 整個不見。他一秒前才親手刪掉的一整天，被一列開場標籤否認掉。
    ///
    /// 和 `queries` 那次一模一樣：**這張表會在清空之後長出來**。差別只在這次
    /// 是同一張表裡的**某幾種列**，所以問題要問到列的層次。
    ///
    /// 這條測試守的是 `system_events == session_marks` 那個比較。少了它，把它
    /// 寫回 `system_events == 0` 一樣綠——上面那條 `..._goes_away_when_the_
    /// recorder_finishes` 抓不到，因為那裡 `forget` 連標籤都一起帶走了。
    #[test]
    fn the_first_row_of_the_next_recording_does_not_undo_the_erasure() {
        let mut db = test_db();
        let s = db.start_session("test", "0.0.1").expect("session");
        let f = frame_with_text(1_000, "chrome.exe", "帳單", &["本期應繳 NT$1,234"]);
        db.insert_frame(s, &f, None, 0).expect("insert");
        db.end_session(s).expect("end");
        db.forget(0, 2_000, None).expect("forget");
        let st = db.stats().expect("stats");
        assert_eq!(st.sessions, 0, "乾淨收尾的那一場整個走了——這是基準");
        assert!(st.nothing_recorded_left());

        // 她又開始錄了。`Recorder::new`：`start_session` 然後 `SessionStart`。
        let next = db.start_session("test", "0.0.1").expect("session");
        db.insert_system(
            next,
            &SystemEvent {
                ts: 9_000,
                kind: SystemKind::SessionStart,
                detail: None,
            },
        )
        .expect("mark");

        let st = db.stats().expect("stats");
        assert_eq!(st.system_events, 1, "那一列真的在——這是這條要守的前提");
        assert_eq!(st.session_marks, 1, "而它是一個標籤，不是她記下來的東西");
        assert!(
            st.nothing_recorded_left(),
            "他刪掉的那一整天不可以被一列開場標籤否認掉：{st:?}"
        );
        assert!(
            st.only_session_shells_left(),
            "剛開始錄、還沒記到東西的那一場也是個殼：{st:?}"
        );

        // 而她真的記到第一筆的那一刻，這一切就結束了。
        db.insert_frame(
            next,
            &frame_with_text(9_500, "a", "b", &["新的東西"]),
            None,
            0,
        )
        .expect("insert");
        let st = db.stats().expect("stats");
        assert!(!st.nothing_recorded_left(), "{st:?}");
        assert!(!st.only_session_shells_left(), "{st:?}");
    }

    /// 一場**什麼都沒存到**的錄製，不可以把 `ever_stored` 按下去。
    ///
    /// 這是 `capture.enabled = false` 那台機器：她跑完了、`start_session` 和兩
    /// 列 `session_*` 都寫了、一個字都沒進資料庫，而 `sister forget` 從來沒被
    /// 執行過。`ever_recorded` 在這裡是 true——那正是它答不出這一題的原因。
    #[test]
    fn a_recording_that_stored_nothing_leaves_the_flag_down() {
        let mut db = test_db();
        let s = db.start_session("test", "0.0.1").expect("session");
        for kind in [SystemKind::SessionStart, SystemKind::SessionEnd] {
            db.insert_system(
                s,
                &SystemEvent {
                    ts: 1_000,
                    kind,
                    detail: None,
                },
            )
            .expect("mark");
        }
        db.end_session(s).expect("end");

        assert!(db.ever_recorded().expect("ever"), "她確實跑過一場");
        assert!(
            !db.ever_stored().expect("stored"),
            "可是一列內容都沒落地——這顆資料庫沒有東西可以被忘掉"
        );
    }

    /// 而 `Excluded` 那種列**是**內容：她真的記下了「這一刻被規則擋掉」。
    #[test]
    fn an_exclusion_audit_row_is_content_and_flips_the_flag() {
        let mut db = test_db();
        let s = db.start_session("test", "0.0.1").expect("session");
        db.insert_system(
            s,
            &SystemEvent {
                ts: 1_000,
                kind: SystemKind::Excluded,
                detail: Some("bank.example".into()),
            },
        )
        .expect("audit");
        assert!(db.ever_stored().expect("stored"));
    }

    /// 每一張算進 `nothing_recorded_left` 的表，都要按得下這個旗標。
    ///
    /// 兩份名單對不起來的下場，是一顆資料庫同時「什麼都不剩」和「從來沒存
    /// 過」——也就是這個旗標本來要拆開的那兩種 0 又黏回去了。這裡不是用反射
    /// 對名單，是**每一張表各餵一列**：漏掉的那一種永遠是沒有 fixture 的那一
    /// 種，所以名單長出新的一張表時，這條會在 `assert_eq!` 那行先炸給你看。
    ///
    /// 但它拿 `CONTENT_TABLES` 比的是**自己**，所以它抓得到「名單上多了一
    /// 張」，抓不到「schema 多了一張而名單沒跟上」——而漏加才是會出事的那個方
    /// 向。那一邊由 `every_table_in_the_schema_is_answered_for` 守。
    #[test]
    fn every_content_table_flips_the_flag() {
        let rows: &[(&str, &str)] = &[
            (
                "frames",
                "INSERT INTO frames(ts, width, height, dhash) VALUES(1,1,1,0)",
            ),
            (
                "text_chunks",
                "INSERT INTO text_chunks(ts, source_kind, text) VALUES(1,'ocr','x')",
            ),
            (
                "focus_events",
                "INSERT INTO focus_events(ts, kind, app_id) VALUES(1,'switch','a')",
            ),
            (
                "clipboard_events",
                "INSERT INTO clipboard_events(ts, kind, byte_len) VALUES(1,'copy',3)",
            ),
            (
                "input_metrics",
                "INSERT INTO input_metrics(ts_start, ts_end) VALUES(1,2)",
            ),
            (
                "system_events",
                "INSERT INTO system_events(ts, kind) VALUES(1,'lock')",
            ),
        ];
        assert_eq!(
            rows.iter().map(|(t, _)| *t).collect::<Vec<_>>(),
            CONTENT_TABLES.iter().map(|(t, _)| *t).collect::<Vec<_>>(),
            "名單上多了一張表，就要在這裡多一列 fixture"
        );
        for (table, sql) in rows {
            let db = test_db();
            assert!(
                !db.ever_stored().expect("stored"),
                "{table}：起點要是乾淨的"
            );
            db.conn.execute(sql, []).expect(table);
            assert!(db.ever_stored().expect("stored"), "{table} 沒把旗標按下去");
        }
    }

    /// **schema 裡的每一張表，都要有人回答「它算不算她記下來的東西」。**
    ///
    /// 上面那條的名單是手寫的，而且拿 `CONTENT_TABLES` 跟自己比——它抓得到多
    /// 加，抓不到漏加。漏加才是會出事的方向：一張新的內容表進了 schema、進了
    /// `nothing_recorded_left`，沒進 `CONTENT_TABLES`，那顆資料庫就會同時是
    /// 「什麼都不剩」和「從來沒存過」，於是清空又長得像沒錄過。
    ///
    /// 所以這裡的名單**從 `sqlite_master` 讀出來**。加一張表就一定要來這裡回
    /// 答一次，回答不了的話這條紅在「你多了一張沒歸類的表」那行。
    #[test]
    fn every_table_in_the_schema_is_answered_for() {
        /// 不是內容的那些，一張一張說出理由——沒有 `_ =>` 可以蒙混過去。
        const NOT_CONTENT: &[(&str, &str)] = &[
            ("meta", "旗標本身，不是內容"),
            ("sessions", "容器，撐得過清空——見 `nothing_recorded_left`"),
            ("ocr_blocks", "frames 長出來的，母體已經在名單上"),
            ("facts", "text_chunks 長出來的，母體已經在名單上"),
            ("queries", "他打的字，而且會在清空之後才長出來"),
            ("query_clicks", "同上，掛在 queries 底下"),
            ("query_marks", "他按的那個位元，一樣掛在 queries 底下"),
            (
                "segment",
                "從 L0 事件算出來的邊界假設，不是原件；母體是 focus/system/clipboard/input，重算得回來",
            ),
            (
                "segment_edit",
                "使用者對斷句的結構編輯，不是螢幕上的原件；forget 跟事件一起清",
            ),
            (
                "stuck_signal",
                "從 L0/L1 算出來的卡住訊號，不是原件；母體是段落所依的事件與 facts",
            ),
            (
                "l2_card",
                "模型對一段的假設，不是螢幕上的原件；forget 沿 provenance tombstone",
            ),
            ("brain_outbound", "出境稽核：送了什麼結構給誰，不含原文"),
            (
                "brain_skip",
                "為什麼沒送出去（沒簽／沒命令／預算／沒東西），不是內容",
            ),
            (
                "commitments",
                "L3 承諾表，Reviewer 寫的假設狀態；刪 L0 時 tombstone，不是原件",
            ),
            (
                "entities",
                "L3 人物／專案／app／組織，Reviewer 寫的；提及跟 L0 走",
            ),
            (
                "entity_mentions",
                "某個 entity 在哪一張 L2／哪筆證據出現過；cascade 的是這一層",
            ),
            (
                "day_summaries",
                "L3 日摘要，從 L2 欄位拼的，不是原件；刪 L0 時 tombstone",
            ),
            (
                "preferences",
                "Reviewer 從回饋學到的偏好（例如哪一類被降權），不是螢幕原件",
            ),
            ("provenance", "血緣圖本身：誰從誰長出來，不是內容"),
            (
                "reviewer_run",
                "審閱層跑過沒、回查了幾次；沒跑過和跑了沒回查靠這張表分",
            ),
            ("reviewer_recheck", "每一次真的去讀 L0 原件的紀錄"),
            (
                "reviewer_divergence",
                "雙 pass 對不上的那幾筆；分歧是警報，不寫入 L3",
            ),
            ("text_fts", "text_chunks 的索引"),
            ("text_fts_uni", "text_chunks 的索引"),
            ("text_fts_bi", "text_chunks 的索引"),
        ];

        let db = test_db();
        let mut stmt = db
            .conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .expect("prepare");
        let tables: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .expect("query")
            .map(|r| r.expect("row"))
            // fts5 自己的影子表（`_data` / `_idx` / `_content` / `_docsize` /
            // `_config`）不是 schema 的一部分，是那三張索引的內臟。
            .filter(|t| {
                !t.ends_with("_data")
                    && !t.ends_with("_idx")
                    && !t.ends_with("_content")
                    && !t.ends_with("_docsize")
                    && !t.ends_with("_config")
            })
            .collect();

        for table in &tables {
            let is_content = CONTENT_TABLES.iter().any(|(t, _)| t == table);
            let excused = NOT_CONTENT.iter().any(|(t, _)| t == table);
            assert!(
                is_content ^ excused,
                "`{table}` 沒有人回答它算不算內容（或兩邊都算了）。\
                 算的話加進 `CONTENT_TABLES`，不算的話加進這裡的 `NOT_CONTENT` 並寫下理由"
            );
        }
        // 反面：名單上寫著、schema 裡卻沒有的表，是抄漏的重新命名。
        let named = CONTENT_TABLES
            .iter()
            .map(|(t, _)| *t)
            .chain(NOT_CONTENT.iter().map(|(t, _)| *t));
        for t in named {
            assert!(
                tables.iter().any(|x| x == t),
                "`{t}` 在名單上，schema 裡沒有這張表"
            );
        }
    }

    /// 旗標撐得過清空——`Erased` 那個答案就是靠它站著的。
    #[test]
    fn forgetting_everything_does_not_take_the_flag_with_it() {
        let mut db = test_db();
        let s = db.start_session("test", "0.0.1").expect("session");
        let f = frame_with_text(1_000, "chrome.exe", "帳單", &["本期應繳 NT$1,234"]);
        db.insert_frame(s, &f, None, 0).expect("insert");
        db.end_session(s).expect("end");
        db.forget(0, 2_000, None).expect("forget");

        assert!(db.stats().expect("stats").nothing_recorded_left());
        assert!(
            db.ever_stored().expect("stored"),
            "東西沒了，但「她存過東西」這件事必須留著——不然清空長得像沒錄過"
        );
    }

    /// 反面：她**還記著東西**的那一場，不是殼。
    ///
    /// 少了這一條，把那個述詞寫成 `sessions > 0` 也一樣綠——然後每一顆正常的
    /// 資料庫都會被標上「空殼」，而那是一句關於「你的東西還在不在」的假話。
    #[test]
    fn a_session_that_still_holds_something_is_not_a_shell() {
        let mut db = test_db();
        let s = db.start_session("test", "0.0.1").expect("session");
        let f = frame_with_text(1_000, "chrome.exe", "帳單", &["本期應繳 NT$1,234"]);
        db.insert_frame(s, &f, None, 0).expect("insert");

        let st = db.stats().expect("stats");
        assert_eq!(st.sessions, 1);
        assert!(!st.only_session_shells_left(), "她記著東西，那一場不是殼");

        // 一列都沒有的時候也不是——沒有殼可以講。
        let empty = test_db().stats().expect("stats");
        assert_eq!(empty.sessions, 0);
        assert!(!empty.only_session_shells_left());
    }

    #[test]
    fn dup_frames_are_collapsed_not_duplicated() {
        let mut db = test_db();
        let s = db.start_session("test", "0.0.1").expect("session");
        let f = frame_with_text(1, "a", "b", &["same screen"]);
        let (frame_id, _, _) = db.insert_frame(s, &f, None, 0).expect("insert");

        for _ in 0..5 {
            db.bump_frame_dup(frame_id).expect("bump");
        }

        let st = db.stats().expect("stats");
        assert_eq!(st.frames, 1, "duplicates must not create rows");
        assert_eq!(st.frames_collapsed, 5, "but they must be counted");
    }

    /// 第三張同意書沒簽的時候她照樣一幀一幀地記，只是不留圖。足跡報告要分得出
    /// 「使用者選了不留圖」和「圖寫失敗了」——不然「3 張保留」配上「畫面檔 0 B」
    /// 看起來就是一個 bug，而它其實是隱私模式在生效。
    #[test]
    fn a_frame_she_remembered_without_a_picture_still_counts_as_a_frame() {
        let mut db = test_db();
        let s = db.start_session("test", "0.0.1").expect("session");

        for (i, text) in ["只記字的", "也是只記字的"].iter().enumerate() {
            let f = frame_with_text(i as i64 + 1, "a", "b", &[text]);
            db.insert_frame(s, &f, None, 0).expect("insert");
        }
        let kept = frame_with_text(3, "a", "b", &["這張留了圖"]);
        db.insert_frame(s, &kept, Some("/tmp/kept.webp"), 4096)
            .expect("insert");

        let st = db.stats().expect("stats");
        assert_eq!(st.frames, 3, "沒留圖的那兩張也是她記過的東西");
        assert_eq!(st.frames_with_image, 1, "但硬碟上只有一張");
        assert_eq!(st.image_bytes, 4096);
    }

    /// 「這段字從哪一幀來的」和「那一幀有沒有照片」是兩個問題。
    ///
    /// 畫面上那個「點開看當時的畫面」問的是後者，而它以前讀的是前者——於是
    /// 只簽第一張同意書（只記字、不留圖）的時候，**每一個**出處都長得可以點，
    /// 點下去才說「已經過了保留期」。那一張圖從來沒有存在過，沒有什麼過期。
    #[test]
    fn a_source_you_can_click_is_not_the_same_as_a_source_you_can_see() {
        let mut db = test_db();
        let s = db.start_session("test", "0.0.1").expect("session");

        let (no_pic, _, _) = db
            .insert_frame(s, &frame_with_text(1, "a", "b", &["只記了字"]), None, 0)
            .expect("insert");
        let (has_pic, _, _) = db
            .insert_frame(
                s,
                &frame_with_text(2, "a", "b", &["這張留了圖"]),
                Some("/tmp/kept.webp"),
                4096,
            )
            .expect("insert");

        // 兩幀都是正當的出處——文字都在，兩個 id 也都是真的。
        let openable = db
            .frames_with_image(&[no_pic, has_pic])
            .expect("frames_with_image");
        assert!(openable.contains(&has_pic), "有圖的那張要點得開");
        assert!(
            !openable.contains(&no_pic),
            "沒圖的那張不該點得開——它的字還在，但沒有畫面可以給他看"
        );

        // 沒問就不要去查資料庫。答案畫面常常一張圖都沒有。
        assert!(
            db.frames_with_image(&[]).expect("empty").is_empty(),
            "空清單要直接回空的"
        );
        // 不存在的 id 不算有圖，也不該炸。
        assert!(
            !db.frames_with_image(&[9_999_999])
                .expect("missing id")
                .contains(&9_999_999)
        );

        // 時間軸一次可以送 2000 個 id 進來，超過一條 `IN` 塞得下的量。切了段
        // 之後每一段的答案都要併回同一份，不能只剩最後一段。
        let mut many: Vec<i64> = (900_000..901_400).collect();
        many.push(has_pic);
        let openable = db.frames_with_image(&many).expect("a very long day");
        assert_eq!(
            openable.len(),
            1,
            "一千四百個假 id 裡只有一個是真的、而且有圖"
        );
        assert!(openable.contains(&has_pic), "那一個不能因為排在最後就掉了");
    }

    /// **畫面的跨度和文字的跨度不是同一個數字。**
    ///
    /// `image_bytes` 只涵蓋還留著檔案的那幾列（`retention.frames_days`，預設
    /// 30 天），`first_ts`/`last_ts` 跟著 `text_chunks` 走（`text_days`，預設
    /// 365 天）。拿前者去除以後者，是三十天的分子配一年的分母——而 `sister
    /// stats` 那句「每天約 47.0 MB ✓」正是 Phase 0 退出條件的判決，錯的方向
    /// 剛好是「看起來過了」。
    #[test]
    fn the_pictures_and_the_words_do_not_cover_the_same_days() {
        let mut db = test_db();
        let s = db.start_session("test", "0.0.1").expect("session");
        const DAY: Millis = 86_400_000;

        // 一年前那一幀：字還在，圖已經被保留期清掉（`image_path` NULL、
        // `image_bytes` 歸零，見 `retention.rs`）。
        db.insert_frame(s, &frame_with_text(DAY, "a", "b", &["很久以前"]), None, 0)
            .expect("old");
        // 昨天那一幀：圖還在。
        db.insert_frame(
            s,
            &frame_with_text(DAY * 365, "a", "b", &["昨天"]),
            Some("/tmp/new.webp"),
            10_000_000,
        )
        .expect("new");

        let st = db.stats().expect("stats");
        assert_eq!(
            (st.first_ts, st.last_ts),
            (Some(DAY), Some(DAY * 365)),
            "文字橫跨一年"
        );
        assert_eq!(
            (st.image_first_ts, st.image_last_ts),
            (Some(DAY * 365), Some(DAY * 365)),
            "但還留著圖的只有最後那一天——分母不能拿文字那個一年來用"
        );
    }

    /// **「那一版還沒在記為什麼停」和「記了、後來被清掉」是兩件事。**
    ///
    /// `system_events` 保留期和 `sister forget` 都刪，所以一場錄製的 `reason`
    /// 會自己變成 `None`——而 doctor 那一行的全部意義就是分辨「你按了停止」/
    /// 「她當掉了」/「同意書被撤回」，把原因報成「舊版執行檔」是把帳算到錯的
    /// 地方。
    ///
    /// 這一場**橫跨**那個被忘掉的區間：中午那張畫面留著（`ts = 5_000`），只有
    /// 早上那則 `session_end` 被蓋掉。以前這個測試不需要那張畫面，因為
    /// `sessions` 那張表誰都不刪；現在一列都不剩的那一場會整個消失，所以
    /// 「還留著、但理由不見了」要真的長成它在現實裡的樣子——他忘掉的是一天
    /// 裡的某一段，不是整場。
    #[test]
    fn a_reason_that_was_pruned_is_not_a_reason_that_was_never_written() {
        let mut db = test_db();
        let s = db.start_session("test", "0.1.0-alpha.21").expect("session");
        db.insert_system(
            s,
            &SystemEvent {
                ts: 1_000,
                kind: SystemKind::SessionEnd,
                detail: Some("user_stop".into()),
            },
        )
        .expect("session_end");
        db.insert_frame(s, &frame_with_text(5_000, "a", "b", &["中午"]), None, 0)
            .expect("survives");
        db.end_session(s).expect("end");

        let before = db.last_session().expect("last").expect("有一場");
        assert_eq!(before.reason.as_deref(), Some("user_stop"));
        assert_eq!(before.app_version, "0.1.0-alpha.21");
        assert!(before.events_left > 0);

        // `sister forget` 蓋過那段時間——理由跟著事件一起走了。
        db.forget(0, 2_000, None).expect("forget");

        let after = db.last_session().expect("last").expect("那一列還在");
        assert_eq!(after.reason, None, "理由不見了");
        assert_eq!(
            after.events_left, 0,
            "而且整場的事件都不剩——這才是它不見的原因，不是那一版沒寫"
        );
    }

    #[test]
    fn focus_event_indexes_title_and_url() {
        let mut db = test_db();
        let s = db.start_session("test", "0.0.1").expect("session");
        db.insert_focus(
            s,
            &FocusEvent {
                ts: 5_000,
                kind: FocusKind::Focus,
                snapshot: FocusSnapshot {
                    app_id: Some("chrome.exe".into()),
                    app_name: Some("Chrome".into()),
                    window_title: Some("Cloudflare DNS 設定".into()),
                    url: Some("https://dash.cloudflare.com/dns".into()),
                    pid: Some(9),
                    password_field: false,
                },
            },
        )
        .expect("insert focus");

        let hits = db.search("Cloudflare", 10).expect("search");
        assert!(
            !hits.is_empty(),
            "window titles must be searchable for free"
        );

        let st = db.stats().expect("stats");
        assert_eq!(st.focus_events, 1);
        assert_eq!(st.chunks, 2, "title and url each become a chunk");
    }

    #[test]
    fn clipboard_secret_stores_the_event_but_not_the_content() {
        let mut db = test_db();
        let s = db.start_session("test", "0.0.1").expect("session");
        db.insert_clipboard(
            s,
            &ClipboardEvent {
                ts: 10,
                kind: ClipboardKind::Text,
                text: None, // recorder 判定為秘密，內容不落地
                byte_len: 51,
                truncated: false,
                secret_suspected: true,
                source_app: Some("terminal".into()),
            },
        )
        .expect("insert clipboard");

        let st = db.stats().expect("stats");
        assert_eq!(
            st.clipboard_events, 1,
            "the fact that you copied a secret is kept"
        );
        assert_eq!(st.chunks, 0, "but the secret itself never enters the index");
    }

    #[test]
    fn facts_can_be_queried_by_kind_and_substring() {
        let mut db = test_db();
        let s = db.start_session("test", "0.0.1").expect("session");
        db.insert_frame(
            s,
            &frame_with_text(
                1,
                "chrome.exe",
                "bill",
                &["應繳 NT$13,450 客服 0800-123-456"],
            ),
            None,
            0,
        )
        .expect("insert");

        let money = db.facts_by_kind("money", 10).expect("money facts");
        assert!(!money.is_empty(), "money fact must be extracted");
        assert!(money[0].normalized.contains("13450") || money[0].raw.contains("13,450"));

        let phones = db
            .facts_search(Some("phone"), "0800", 10)
            .expect("phone search");
        assert!(
            !phones.is_empty(),
            "phone fact must be findable by substring"
        );
    }

    /// 直接塞一筆文字。時間軸只讀 `text_chunks`，所以測它不必先造一張畫面
    /// ——而「沒有畫面也要看得到」正是下面那條測試的重點。
    fn put_chunk(db: &Db, session_id: i64, ts: Millis, text: &str) {
        db.conn()
            .execute(
                "INSERT INTO text_chunks(ts, session_id, source_kind, text)
                 VALUES(?1, ?2, 'ocr', ?3)",
                params![ts, session_id, text],
            )
            .expect("insert chunk");
    }

    /// 一天從哪裡開始，由**使用者住在哪**決定，不是格林威治。
    ///
    /// 這條看起來像在測算術，實際上釘的是一個會讓整條時間軸錯位的東西：
    /// 台北是 UTC+8，所以本地時間 2 月 2 日早上 7 點，換成 UTC 是 2 月 1 日
    /// 23 點。用 UTC 切天的話，那筆記錄會被放進「昨天」——而使用者記得的是
    /// 今天早上發生的事。
    #[test]
    fn a_day_starts_where_the_user_lives_not_at_greenwich() {
        const H: i64 = 3_600_000;
        let mut db = test_db();
        let s = db.start_session("test", "0.0.1").expect("session");

        // 台北時間 2024-02-02 07:00 = UTC 2024-02-01 23:00
        let taipei_morning = 1_706_828_400_000;
        put_chunk(&db, s, taipei_morning, "早上看到的東西");

        const DAY: i64 = 24 * H;
        let utc = db.days_with_data(0).expect("days");
        let taipei = db.days_with_data(8 * H).expect("days");
        assert_eq!(utc.len(), 1);
        assert_eq!(taipei.len(), 1);

        // 兩邊回的都是「那一天的午夜」，只是量的是不同時區的午夜。
        assert_eq!(utc[0].start_ts % DAY, 0, "UTC 的午夜就是整數天");
        assert_eq!(
            (taipei[0].start_ts + 8 * H) % DAY,
            0,
            "台北的午夜要加回偏移量才落在整數天上"
        );
        // 這才是重點：同一筆資料，在台北屬於**隔一天**。
        assert_eq!(
            (taipei[0].start_ts + 8 * H) / DAY - utc[0].start_ts / DAY,
            1,
            "本地時間的 2/2 早上，在 UTC 是 2/1 晚上"
        );
        assert_eq!(taipei[0].chunks, 1);
        assert_eq!(taipei[0].first_ts, taipei_morning);
    }

    /// 時間軸要看得到「字還在、圖沒了」的那些日子。
    ///
    /// 文字留 365 天、畫面留 30 天，所以一個月以前的每一格都長這樣。如果
    /// 時間軸是以 `frames` 為主體做的，那些日子會整片空白——她其實記得，
    /// 只是那張圖過期了。
    #[test]
    fn the_timeline_still_has_days_whose_screenshots_are_long_gone() {
        let mut db = test_db();
        let s = db.start_session("test", "0.0.1").expect("session");
        put_chunk(&db, s, 6_000, "後來那件事");
        put_chunk(&db, s, 5_000, "兩個月前那封信");

        let moments = db.timeline(0, 10_000, 50).expect("timeline");
        assert_eq!(moments.len(), 2);
        assert_eq!(moments[0].ts, 5_000, "依時間排序，不是依插入順序");
        assert!(
            moments.iter().all(|m| m.frame_id.is_none()),
            "沒有圖不是錯誤，是保留期的正常結果"
        );

        // 左閉右開：查 [0, 5_000) 不該把 5_000 那一筆算進來，否則按天翻頁時
        // 每一天的第一筆都會在前一天再出現一次。
        assert!(db.timeline(0, 5_000, 50).expect("timeline").is_empty());
    }

    /// 暫停稽核要答得出三種配不起來的情況，而不是靜靜地少算一段。
    ///
    /// 每一種都是真的會發生的：使用者現在還在暫停中（沒有 resume）、
    /// 保留期把開頭那筆 pause 刪掉了（孤兒 resume）、以及一段乾乾淨淨的
    /// 進出。三種混在同一個資料庫裡，總數還是要對。
    #[test]
    fn the_pause_ledger_admits_the_episodes_it_cannot_measure() {
        let mut db = test_db();
        let s = db.start_session("test", "0.0.1").expect("session");
        let mut put = |kind, ts| {
            db.insert_system(
                s,
                &SystemEvent {
                    ts,
                    kind,
                    detail: None,
                },
            )
            .expect("insert");
        };

        // 1) 孤兒 resume：開頭那筆 pause 被保留期刪掉了
        put(SystemKind::CaptureResumed, 1_000);
        // 2) 完整的一段：10 分鐘
        put(SystemKind::CapturePaused, 2_000);
        put(SystemKind::CaptureResumed, 602_000);
        // 3) 還沒結束的一段
        put(SystemKind::CapturePaused, 700_000);

        let audit = db.pause_audit().expect("audit");
        assert_eq!(audit.episodes, 3, "三段都要算進來");
        assert_eq!(audit.total_ms, 600_000, "只有量得出來的那一段進總長");
        assert_eq!(audit.open_since, Some(700_000));
        assert_eq!(audit.truncated, 1, "算不出長度的那一段要說出來");
    }

    #[test]
    fn a_database_that_was_never_paused_says_so_plainly() {
        let db = test_db();
        assert_eq!(db.pause_audit().expect("audit"), PauseAudit::default());
    }

    /// 時間軸問「今天」，答案裡要有那段昨天按下、今天還沒解除的暫停。
    ///
    /// 這是 `pause_spans` 唯一難的地方。那一段的 `pause` 事件在窗外，只查窗內
    /// 事件的寫法會回一個空陣列，於是今天一整天的空白沒有人解釋——而空白的
    /// 原因正好是使用者自己按下去的那一下。
    #[test]
    fn a_pause_from_yesterday_still_explains_todays_blank_screen() {
        const DAY: Millis = 86_400_000;
        let mut db = test_db();
        let s = db.start_session("test", "0.0.1").expect("session");
        let mut put = |kind, ts| {
            db.insert_system(
                s,
                &SystemEvent {
                    ts,
                    kind,
                    detail: None,
                },
            )
            .expect("insert");
        };

        // 前天午睡：整段都在「今天」之前，不該出現在今天的時間軸上。
        put(SystemKind::CapturePaused, DAY);
        put(SystemKind::CaptureResumed, DAY + 3_600_000);
        // 昨天晚上按下去，一路沒解除。
        put(SystemKind::CapturePaused, 2 * DAY + 79_200_000);

        let today = db.pause_spans(3 * DAY, 4 * DAY).expect("spans");
        assert_eq!(
            today,
            vec![PauseSpan {
                from: Some(2 * DAY + 79_200_000),
                to: None,
            }],
            "跨夜那一段要留下，前天那一段要濾掉"
        );

        // 而且回的是**真實**起點（昨天晚上），不是被裁到今天午夜——
        // 使用者要看得出來這一段其實從昨天就開始了。
        assert!(today[0].from.expect("known start") < 3 * DAY);
    }

    /// 星期一才解除的那一下，在星期五那一頁上不能變成「之後沒有再解除」。
    ///
    /// 上一版的 SQL 有 `AND ts < ?1`，於是那筆 `resume` 落在窗外被篩掉，
    /// 這一段回的是 `to: None`——而時間軸把 `to: None` 印成「之後沒有再解除」，
    /// 一句關於接下來所有時間的話，從一份刻意裁到午夜的資料裡講出來，還和
    /// 真正「到現在都還沒解除」長得一模一樣。
    ///
    /// `to` 要是**真值**，畫面才選得到那句「跨過午夜」——那條分支寫在
    /// `timeline.js` 裡，而在這行 SQL 改掉之前它一次都執行不到。
    #[test]
    fn a_pause_released_on_monday_is_not_a_pause_that_was_never_released() {
        const DAY: Millis = 86_400_000;
        let mut db = test_db();
        let s = db.start_session("test", "0.0.1").expect("session");
        let mut put = |kind, ts| {
            db.insert_system(
                s,
                &SystemEvent {
                    ts,
                    kind,
                    detail: None,
                },
            )
            .expect("insert");
        };

        // 星期五 18:00 按下，星期一 09:00 解除。
        let pressed = 5 * DAY + 64_800_000;
        let released = 8 * DAY + 32_400_000;
        put(SystemKind::CapturePaused, pressed);
        put(SystemKind::CaptureResumed, released);

        let friday = db.pause_spans(5 * DAY, 6 * DAY).expect("spans");
        assert_eq!(
            friday,
            vec![PauseSpan {
                from: Some(pressed),
                to: Some(released),
            }],
            "解除的時刻在窗外，但它存在——回 None 會讓畫面說「之後沒有再解除」"
        );
        assert!(
            friday[0].to.expect("known end") > 6 * DAY,
            "畫面靠這個比較選出「跨過午夜」那句話"
        );

        // 而真的沒解除的那一段，還是要回 None——不然這條測試等於把兩種狀態
        // 一起壓成「總是有結尾」，換一個方向說同一個謊。
        let mut db2 = test_db();
        let s2 = db2.start_session("test", "0.0.1").expect("session");
        db2.insert_system(
            s2,
            &SystemEvent {
                ts: pressed,
                kind: SystemKind::CapturePaused,
                detail: None,
            },
        )
        .expect("insert");
        assert_eq!(
            db2.pause_spans(5 * DAY, 6 * DAY).expect("spans"),
            vec![PauseSpan {
                from: Some(pressed),
                to: None,
            }],
        );

        // 整段都在窗**之後**的，一樣不該出現在星期五那一頁上。上界的篩選
        // 從 SQL 搬到了 retain，這一條盯著它沒有跟著消失。
        assert!(
            db.pause_spans(3 * DAY, 4 * DAY).expect("spans").is_empty(),
            "星期三沒有暫停過"
        );
    }

    /// 開頭被保留期刪掉的那一段，寧可多畫一條也不要讓空白沒人解釋。
    #[test]
    fn a_pause_whose_beginning_was_pruned_is_still_drawn() {
        let mut db = test_db();
        let s = db.start_session("test", "0.0.1").expect("session");
        db.insert_system(
            s,
            &SystemEvent {
                ts: 5_000,
                kind: SystemKind::CaptureResumed,
                detail: None,
            },
        )
        .expect("insert");

        let spans = db.pause_spans(0, 10_000).expect("spans");
        assert_eq!(
            spans,
            vec![PauseSpan {
                from: None,
                to: Some(5_000)
            }],
            "不知道何時開始，但確實蓋住了這段窗的開頭"
        );
        // 解除之後的窗完全不受影響。
        assert!(db.pause_spans(5_000, 10_000).expect("spans").is_empty());
    }

    #[test]
    fn input_metrics_and_system_events_round_trip() {
        let mut db = test_db();
        let s = db.start_session("test", "0.0.1").expect("session");
        db.insert_input(
            s,
            &InputMetrics {
                ts_start: 0,
                ts_end: 10_000,
                keystrokes: 120,
                clicks: 4,
                mouse_px: 3200,
                scroll_ticks: 18,
                window_switches: 2,
                idle_ms: 1500,
                typing_bursts: 3,
            },
        )
        .expect("insert input");
        db.insert_system(
            s,
            &SystemEvent {
                ts: 1,
                kind: SystemKind::Excluded,
                detail: Some("excluded app".into()),
            },
        )
        .expect("insert system");

        let st = db.stats().expect("stats");
        assert_eq!(st.input_windows, 1);
        assert_eq!(st.system_events, 1);
    }

    /// 「零當機」現在有實作了，而不是靠使用者的印象。
    #[test]
    fn a_session_that_never_closed_is_counted_as_a_crash() {
        let mut db = test_db();

        let clean = db.start_session("test", "0.0.1").expect("session");
        // 這一場要真的記到東西。一列都沒留下的錄製，收尾的時候整場都會消失
        // （見 `end_session`），而這條測試問的是當機不是空場。
        db.insert_frame(
            clean,
            &frame_with_text(1_000, "a", "b", &["有東西"]),
            None,
            0,
        )
        .expect("insert");
        db.end_session(clean).expect("end");
        let a = db.crash_audit(None).expect("audit");
        assert_eq!(
            (a.started, a.ended, a.rows, a.rows_unfinished, a.last_crash),
            (1, 1, 1, 0, None),
            "收好尾的不該算當機"
        );
        assert_eq!(a.crashed(), 0);
        assert!(!a.floor, "全新的資料庫沒有被刪掉的過去，它的數字是精確的");

        // **她剛剛才被開起來的那幾百毫秒**：心跳檔已經寫了（`is_occupied` 看
        // 得到），而 `sessions` 那一列還沒 INSERT。這時候一列都不准扣——最後
        // 那一列是收好尾的，扣掉它會讓一場正常收尾的錄製從分母裡消失，而
        // `started - ended - 1` 會變成 -1：那不是一次當機，那是一個還沒開始
        // 的開始。
        let warming = db
            .crash_audit(Some(crate::heartbeat::Phase::Recording))
            .expect("audit");
        assert!(!warming.live, "沒有一列可以扣，就不准說「扣掉了」");
        assert_eq!(
            (warming.started, warming.ended, warming.rows),
            (1, 1, 1),
            "心跳在、列還沒進去的那幾百毫秒，她一個位置都不佔"
        );
        assert_eq!(warming.crashed(), 0, "負的當機數是一個數不出來的東西");

        // 開了就不收尾——程序被殺、當機、拔電，都長這樣。
        db.start_session("test", "0.0.1").expect("session");
        let a = db.crash_audit(None).expect("audit");
        assert_eq!((a.started, a.ended), (2, 1));
        assert_eq!(a.rows_unfinished, 1, "沒收尾的那一段沒有被算到");
        assert!(a.last_crash.is_some(), "沒收尾的話要講得出是什麼時候");
        assert_eq!(a.crashed(), 1);

        // **而她此刻正在錄的話，那一段長得一模一樣。** 心跳是唯一分得出來的
        // 東西，所以那一題由呼叫端傳進來，不由這裡猜——而且**每一個數字都要
        // 扣**，不是只扣當機數。上一版只扣了當機數，於是同一份報告先說「現在
        // 正在錄的那一場沒有算進去」，再把那一場的開始時間當成最後一次當機報
        // 出來，兩行相隔十公分。
        let live = db
            .crash_audit(Some(crate::heartbeat::Phase::Recording))
            .expect("audit");
        assert!(live.live, "最新那一列沒收尾、而且心跳在，那就是她");
        assert_eq!(live.crashed(), 0, "正在錄的那一場不是一次當機");
        assert_eq!(
            (
                live.started,
                live.ended,
                live.rows,
                live.rows_unfinished,
                live.last_crash
            ),
            (1, 1, 1, 0, None),
            "扣就整個扣掉：分母、還留著的列、最後一次的時間，一個都不准留著她"
        );

        // **而這才是那道閘門真正要擋的東西。** 上面那一格（`warming`）看起來
        // 在驗開機那一段，其實不是：它前面那一場是**收好尾的**，所以
        // `newest_open` 自己就是 false，那道閘門拿掉照樣綠。真正的開機那一段
        // 長這樣——上一場**當掉了**（`ended_at IS NULL` 坐在 `MAX(id)` 上），
        // 而她此刻正在起來、列還沒進去。
        //
        // 順序是產品裡的順序：`BootBeat::start` 寫心跳 → `Db::open`（一顆存了
        // 一年的資料庫，migration 要跑好幾分鐘）→ 開機那次 `prune` → 最後才
        // `start_session`。這幾分鐘裡「有人佔著」和「最新那一列沒收尾」兩件事
        // 都是真的，而它們**指的不是同一場**。
        let booting = db
            .crash_audit(Some(crate::heartbeat::Phase::Booting))
            .expect("audit");
        assert!(
            !booting.live,
            "開機那一段，最新那一列是上一次當機的殼，不是她"
        );
        assert_eq!(
            (booting.started, booting.ended, booting.rows_unfinished),
            (2, 1, 1),
            "扣掉的會是別人的一次當機"
        );
        assert_eq!(booting.crashed(), 1, "那一次當機不會因為她開機而消失");
        assert_eq!(
            booting.last_crash, a.last_crash,
            "而且時間還在——上一版把這個時間當成「她現在還在跑」的那一場"
        );
        assert_eq!(
            booting.beat,
            Some(crate::heartbeat::Phase::Booting),
            "「有人佔著」和「她的列在不在」是兩題，兩個答案都要帶回去"
        );
    }

    /// 計數器只准數**事件**，不准數 SQL 語句。
    ///
    /// 兩道守衛，今天兩道都沒有人會撞到——`end_session` 一場只寫一次
    /// `ended_at`，而 `start_session` 是唯一的 INSERT 且不填 `ended_at`。所以
    /// 少了它們沒有任何現有的測試會紅，而**這種守衛正是後來會被某個看起來很
    /// 無害的改動撞上的那一種**。
    ///
    /// 兩邊壞掉的方向剛好相反，而危險的是同一邊：
    ///
    /// * `ended` 多加一次 → `crashed()` 變負數 → 被夾成 0 → **一次真的當機
    ///   被蓋成 ✓**。
    /// * 生下來就帶 `ended_at` 的那一列沒被數到 → **憑空長出一次當機**。
    ///
    /// 憑空長出來的 ✓ 比憑空長出來的 ✗ 難發現得多，因為沒有人會去查一個好
    /// 消息。
    #[test]
    fn the_counters_count_recordings_not_update_statements() {
        let mut db = test_db();
        let s = db.start_session("test", "0.0.1").expect("session");
        db.insert_frame(s, &frame_with_text(now_ms(), "a", "b", &["有"]), None, 0)
            .expect("insert");
        db.end_session(s).expect("end");
        let base = db.crash_audit(None).expect("audit");
        assert_eq!((base.started, base.ended), (1, 1));

        // 「改一下收尾時間」——一句看起來無害的 UPDATE。
        db.conn
            .execute("UPDATE sessions SET ended_at = ended_at + 1", [])
            .expect("再寫一次收尾時間");
        assert_eq!(
            db.crash_audit(None).expect("audit").ended,
            base.ended,
            "同一場收尾兩次還是一場"
        );

        // 「補一場已經結束的錄製」——一句看起來無害的 INSERT。
        db.conn
            .execute(
                "INSERT INTO sessions(started_at, ended_at, app_version, platform) \
                 VALUES(1, 2, 'test', 'test')",
                [],
            )
            .expect("補一場已經結束的錄製");
        let after = db.crash_audit(None).expect("audit");
        assert_eq!(
            (after.started, after.ended),
            (base.started + 1, base.ended + 1),
            "生下來就結束的那一場，兩邊都要跟著動"
        );
        assert_eq!(after.crashed(), 0, "它好好結束了，不是一次當機");
    }

    /// **最該被算進去的那一種當機，剛好就是會把自己的證據刪掉的那一種。**
    ///
    /// 「開起來、還沒讀到第一張畫面就死掉」的那一場沒有內容，於是下一次
    /// `prune` 連它的紀錄一起刪掉（`retention::delete_empty_sessions`，那是
    /// #52 要的行為，那一列是「他那天下午在電腦前」的證明）。分子和分母同時
    /// 少一，於是她死得越早，`零當機` 那一格讀起來越乾淨——一台卡在開機當機
    /// 迴圈裡的機器會收斂到 ✓。
    ///
    /// 實測過的原始數字（六場：一場正常＋五場開機即死）：`prune` 之前是
    /// 「6 段裡有 5 段沒收尾」，之後是「2 段裡有 1 段」。
    #[test]
    fn a_crash_that_stored_nothing_still_counts_after_its_row_is_swept() {
        let mut db = test_db();

        // 一場好的，讓資料庫不是空的。**時間要用現在**：`frame_with_text(1_000,
        // ..)` 那個時間是 1970 年，`prune` 會因為過了保留期把整列刪掉，於是這
        // 一場也變成空的、也被掃走——而那樣一來 `traceless()` 和
        // `started - ended` 剛好會相等，這條測試就變成「兩個都對才過」而不是
        // 「只有對的那個才過」。M-C 那個突變活下來，活的就是這裡。
        let good = db.start_session("test", "0.0.1").expect("session");
        db.insert_frame(
            good,
            &frame_with_text(now_ms(), "a", "b", &["有東西"]),
            None,
            0,
        )
        .expect("insert");
        db.end_session(good).expect("end");

        // 五場「開起來就死在第一張畫面之前」。
        for _ in 0..5 {
            db.start_session("test", "0.0.1").expect("session");
        }

        let before = db.crash_audit(None).expect("audit");
        assert_eq!(before.rows, 6, "掃之前六列都在");
        assert_eq!(before.crashed(), 5);

        // `prune` 掃過。空的那幾場的紀錄本身消失——最新的那一場留著，因為
        // 它可能正是現在正在錄的那一場。
        db.prune(now_ms(), &crate::config::RetentionConfig::default(), None)
            .expect("prune");

        let after = db.crash_audit(None).expect("audit");
        assert!(
            after.rows < before.rows,
            "前提沒了這條就沒有意義：那幾列真的要被掃掉（{} → {}）",
            before.rows,
            after.rows
        );
        assert_eq!(
            after.crashed(),
            5,
            "五次當機不可以因為它們什麼都沒存到就消失"
        );
        assert_eq!(after.rows, 2, "有內容的那一場和最新的那一場都要留著");
        assert_eq!(
            after.traceless(),
            before.rows - after.rows,
            "被掃掉幾列，就有幾場沒有留下紀錄"
        );
        assert_ne!(
            after.traceless(),
            after.crashed(),
            "這兩個數字要真的分開：相等的話，把其中一個寫成另一個也照樣會過"
        );
        // 時間只涵蓋還留著紀錄的那幾場，而且是故意的：一個「她那天幾點幾分
        // 當過」的時間戳正是那一列被刪掉要拿掉的東西。
        assert!(
            after.rows_unfinished < after.crashed(),
            "報得出時間的比真的當機的少，句子要講得出這件事"
        );
    }

    /// 升級那天回填的數字是一個**下限**，而它要說得出自己是下限。
    ///
    /// 一顆跑了三個月、被 `prune` 掃過幾十次的資料庫，升上來的那一刻真實的
    /// 「開過幾場」已經不可考。回填只數得到還在的列。alpha.33 那條規矩的同一
    /// 條：回填出來的數字是一個猜測穿著數字的衣服。
    #[test]
    fn an_upgraded_database_does_not_get_to_speak_for_the_sessions_it_cannot_see() {
        let path = migrate_tmp("floor").join("sister.db");
        {
            // schema 5 的樣子：有紀錄、有 `ever_recorded`，沒有計數器。
            let mut db = Db::open(&path).expect("open");
            let s = db.start_session("test", "0.0.1").expect("session");
            db.insert_frame(s, &frame_with_text(1_000, "a", "b", &["有"]), None, 0)
                .expect("insert");
            db.end_session(s).expect("end");
        }
        {
            let conn = Connection::open(&path).expect("raw");
            conn.execute_batch(
                "DROP TRIGGER sessions_started_count;
                 DROP TRIGGER sessions_ended_count;
                 DROP TRIGGER sessions_born_ended_count;
                 DELETE FROM meta WHERE key LIKE 'session%';",
            )
            .expect("回到 schema 5 的樣子");
            conn.pragma_update(None, "user_version", 5).expect("stamp");
        }

        let db = Db::open(&path).expect("reopen");
        let a = db.crash_audit(None).expect("audit");
        assert!(
            a.floor,
            "升上來的那一顆不知道自己被刪過幾場，它就不可以說『全部』"
        );
        assert_eq!((a.started, a.ended), (1, 1), "數得到的那一場照樣要數到");
        assert_eq!(a.crashed(), 0);

        // 全新的那一顆不按那個旗標——它的 0 是精確的 0。
        assert!(
            !Db::open(&migrate_tmp("floor-fresh").join("sister.db"))
                .expect("fresh")
                .crash_audit(None)
                .expect("audit")
                .floor
        );

        // 而**升級之後**開的那幾場照樣是精確的：旗標只描述回填的那一段。
        let mut db = db;
        db.start_session("test", "0.0.1").expect("session");
        assert_eq!(db.crash_audit(None).expect("audit").crashed(), 1);
        drop(db);

        // **這一段重跑一次，不可以替一顆數得準的資料庫貼上「我數不準」。**
        //
        // 版號蓋到一半被砍（alpha.32 那個一毫秒的窗口，`kill -9` 掃 189 次中
        // 1 次）然後自我修復，走的就是這條路：006 的批次已經跑完、計數器已經
        // 是真的，版號還停在 5。旗標那一句要問得出這件事——問不出來的話，這個
        // 假話是**修法自己造出來的**，而那正是這一批 bug 前後犯了三次的形狀。
        let counters = |p: &std::path::Path| -> (i64, i64) {
            let c = Connection::open(p).expect("raw");
            let g = |k: &str| {
                c.query_row(
                    "SELECT CAST(value AS INTEGER) FROM meta WHERE key = ?1",
                    [k],
                    |r| r.get::<_, i64>(0),
                )
                .expect(k)
            };
            (g("sessions_started"), g("sessions_ended"))
        };
        let exact = migrate_tmp("floor-rerun").join("sister.db");
        {
            let mut fresh = Db::open(&exact).expect("fresh");
            let s = fresh.start_session("test", "0.0.1").expect("session");
            fresh.end_session(s).expect("end");
            fresh.start_session("test", "0.0.1").expect("session");
        }
        let was = counters(&exact);
        Connection::open(&exact)
            .expect("raw")
            .pragma_update(None, "user_version", 5)
            .expect("把版號走回去，結構留在新的");

        let again = Db::open(&exact).expect("self-heal");
        assert_eq!(counters(&exact), was, "重跑不可以把真的計數蓋掉");
        assert!(
            !again.crash_audit(None).expect("audit").floor,
            "它一場都沒漏數過，不准說自己漏數了"
        );
    }

    /// 空殼跟安靜長得不一樣，這個檢查必須認得出差別。
    ///
    /// 這條測試有兩半，缺任何一半它就沒有意義：
    ///
    /// 後半（會不會叫）是明顯的那一半。**前半（會不會亂叫）才是重點**——
    /// 一個對著正常資料報警的檢查，三天之內就會被學會忽略，然後它跟不存在
    /// 是同一件事。使用者整個下午都在看影片、完全沒碰鍵盤，是合法狀態。
    #[test]
    fn a_signal_that_is_merely_quiet_is_not_reported_as_broken() {
        let mut db = test_db();
        let s = db.start_session("test", "0.0.1").expect("session");

        // 一段合法的「幾乎沒動」：滑鼠動了 3 px，其他全 0。
        // 這種列在真實使用中每天都有，不該有任何警告。
        db.insert_input(
            s,
            &InputMetrics {
                ts_start: 0,
                ts_end: 10_000,
                keystrokes: 0,
                clicks: 0,
                mouse_px: 3,
                scroll_ticks: 0,
                window_switches: 0,
                idle_ms: 9_000,
                typing_bursts: 0,
            },
        )
        .expect("insert input");

        db.insert_focus(
            s,
            &FocusEvent {
                ts: 0,
                kind: FocusKind::Focus,
                snapshot: FocusSnapshot {
                    app_id: Some("chrome.exe".into()),
                    app_name: Some("Google Chrome".into()),
                    window_title: Some("影片".into()),
                    url: None,
                    // pid 在 replay 上永遠是 None，那是後端的限制不是故障。
                    pid: None,
                    password_field: false,
                },
            },
        )
        .expect("insert focus");

        for a in db.signal_audit(None).expect("audit") {
            assert_ne!(
                a.verdict,
                SignalVerdict::Broken,
                "{} 只是安靜，不該被說成壞掉",
                a.name
            );
        }
    }

    /// 有一堆列、每一列都是空殼——`COUNT(*)` 看起來完全正常的那種故障。
    #[test]
    fn rows_that_carry_nothing_are_reported_as_broken() {
        let mut db = test_db();
        let s = db.start_session("test", "0.0.1").expect("session");

        // 擷取端在「什麼都沒發生」時根本不該寫列（見 windows/input.rs 的
        // 早退）。所以一列全 0 代表那道閘門壞了，不是使用者很安靜。
        //
        // 十列是門檻（`ENOUGH_TO_BE_SURE`）。縮到「最後一場」之後就需要下限
        // ——開機頭三秒的一兩列還不算證據。
        for i in 0..12 {
            db.insert_input(
                s,
                &InputMetrics {
                    ts_start: i * 10_000,
                    ts_end: i * 10_000 + 10_000,
                    keystrokes: 0,
                    clicks: 0,
                    mouse_px: 0,
                    scroll_ticks: 0,
                    window_switches: 0,
                    idle_ms: 10_000,
                    typing_bursts: 0,
                },
            )
            .expect("insert input");
        }

        // 焦點事件不知道自己是哪個 app——這種列不含任何資訊。
        for ts in 0..12 {
            db.insert_focus(
                s,
                &FocusEvent {
                    ts,
                    kind: FocusKind::Focus,
                    snapshot: FocusSnapshot::default(),
                },
            )
            .expect("insert focus");
        }

        let audit = db.signal_audit(None).expect("audit");
        let broken: Vec<_> = audit
            .iter()
            .filter(|a| a.verdict == SignalVerdict::Broken)
            .map(|a| a.name)
            .collect();
        assert!(
            broken.contains(&"輸入節奏"),
            "十二列全 0 的輸入沒被抓到：{audit:?}"
        );
        assert!(
            broken.contains(&"視窗焦點"),
            "不知道 app 的焦點事件沒被抓到：{audit:?}"
        );

        // 一列都沒有的訊號不算壞——那是「還沒錄到」，不是「錄壞了」。
        // 兩者的差別由 `rows` 講，不是由 `broken` 講。
        let coords = audit.iter().find(|a| a.name == "文字座標").expect("座標");
        assert_eq!(coords.rows, 0);
        assert_eq!(
            coords.verdict,
            SignalVerdict::TooEarly,
            "空的表不是壞掉，也不是驗過了——是還看不出來"
        );
    }

    /// **一顆曾經正常過的資料庫，會永遠說自己是好的。**
    ///
    /// 這三句 SQL 本來掃全表，所以它們答的是「這台機器曾經好過嗎」。他用了
    /// 三個月、上禮拜二一次 Windows 更新把 UIA 弄壞了——`named` 還是那三個
    /// 月份的幾萬列，`broken` 永遠是 false，doctor 印三個 ✓。而 doctor 是
    /// 唯一會講這件事的地方（那三張表在 Phase 0 沒有任何讀者）。
    ///
    /// 一條翻不成 ✗ 的檢查不是「還沒抓到」，是一格假的涵蓋。
    #[test]
    fn a_machine_that_used_to_work_does_not_get_to_coast_on_its_good_months() {
        let mut db = test_db();

        // 三個月的好日子。
        let good = db.start_session("test", "0.0.1").expect("session");
        for ts in 0..40 {
            db.insert_focus(
                good,
                &FocusEvent {
                    ts,
                    kind: FocusKind::Focus,
                    snapshot: FocusSnapshot {
                        app_id: Some("chrome.exe".into()),
                        ..FocusSnapshot::default()
                    },
                },
            )
            .expect("insert focus");
        }
        let before = db.signal_audit(None).expect("audit");
        let focus = |v: &[SignalAudit]| *v.iter().find(|a| a.name == "視窗焦點").expect("焦點");
        assert_eq!(focus(&before).verdict, SignalVerdict::Alive, "這一場是好的");

        // 然後 UIA 壞了。她照樣在錄，照樣寫列，只是每一列都不知道自己是誰。
        db.end_session(good).expect("end");
        let broke = db.start_session("test", "0.0.1").expect("session");
        for ts in 100..112 {
            db.insert_focus(
                broke,
                &FocusEvent {
                    ts,
                    kind: FocusKind::Focus,
                    snapshot: FocusSnapshot::default(),
                },
            )
            .expect("insert focus");
        }

        let after = focus(&db.signal_audit(None).expect("audit"));
        assert_eq!(
            after.verdict,
            SignalVerdict::Broken,
            "上一場十二列都不知道自己是哪個 app，而前三個月的好資料把它蓋掉了：{after:?}"
        );
        assert_eq!(after.rows, 12, "數字要描述那一場，不是這顆資料庫的一輩子");
        assert!(
            after.scope_started_at.is_some(),
            "「12 列」是哪一段的，畫面要說得出來"
        );
    }

    /// **「上一場」在她還在錄的時候是假的，而那不是修辭問題。**
    ///
    /// 同一份 doctor 上，[`Db::crash_audit`] 的分母把正在錄的那一場扣掉了，
    /// 而這三列數的正好是被扣掉的那一場——然後叫它「上一場」。問「那 2 場裡
    /// 哪一場是上一場」，答案是都不是。
    ///
    /// 兩半都要問，跟 `crash_audit` 的 `live` 逐字同一條：心跳說**在錄**，而
    /// 且最新那一列還沒收尾。開機那幾分鐘裡心跳在、她那一列還沒 INSERT，那
    /// 一列是上一次當機留下來的殼——把它叫成「這一場，還在錄」，就是把當機說
    /// 成正常。
    #[test]
    fn the_session_those_rows_describe_knows_whether_it_is_still_running() {
        use crate::heartbeat::Phase;
        let mut db = test_db();
        let s = db.start_session("test", "0.0.1").expect("session");
        db.insert_focus(
            s,
            &FocusEvent {
                ts: 1,
                kind: FocusKind::Focus,
                snapshot: FocusSnapshot {
                    app_id: Some("chrome.exe".into()),
                    ..FocusSnapshot::default()
                },
            },
        )
        .expect("insert focus");

        fn live(db: &Db, beat: Option<Phase>) -> bool {
            db.signal_audit(beat).expect("audit")[0].scope_is_live
        }
        assert!(
            live(&db, Some(Phase::Recording)),
            "她在錄，這一場就是這一場"
        );
        assert!(
            !live(&db, Some(Phase::Booting)),
            "開機那一段：心跳在，但她那一列還沒進來——這一列是別人的"
        );
        assert!(!live(&db, None), "沒有心跳就沒有人在錄");

        // 收尾之後，同一顆心跳（另一個終端機正在錄、而它的列還沒進來）也不
        // 可以把這一列說成活的。兩半各擋一種。
        db.end_session(s).expect("end");
        assert!(
            !live(&db, Some(Phase::Recording)),
            "這一列已經收尾了，不管誰佔著資料目錄"
        );
    }

    /// 「我找不到」和「我沒去找」在同一句話裡長得一模一樣。
    ///
    /// 一個字的查詢產不出相鄰雙字，走不到 bigram，只剩 `search_like` 的掃描
    /// ——而掃描夾在 30 天內。`retention.text_days` 預設 365。差 12 倍，而查
    /// 不到的時候她說的是「她記的每一段裡都沒有這個字」。
    ///
    /// [`Db::scan_horizon_days`] 就是為了讓那句話講得出「哪幾段」而存在的。
    #[test]
    fn one_character_only_reaches_thirty_days_and_says_so() {
        let mut db = test_db();
        let s = db.start_session("test", "0.0.1").expect("session");
        let now = 1_700_000_000_000i64;

        // 還沒有任何文字的時候，沒有界線可講——講了就是憑空發明一個限制。
        assert_eq!(db.scan_horizon_days("錢").expect("horizon"), None);

        for ts in [now - 300 * 86_400_000, now] {
            db.insert_frame(
                s,
                &frame_with_text(ts, "chrome.exe", "客服系統", &["客服專線紀錄"]),
                None,
                0,
            )
            .expect("insert");
        }

        assert_eq!(
            db.scan_horizon_days("錢").expect("horizon"),
            Some(LIKE_SCAN_DAYS),
            "一個字只剩掃描，而掃描沒看 30 天以前的東西"
        );
        assert_eq!(
            db.scan_horizon_days("客服").expect("horizon"),
            None,
            "兩個字走 bigram，沒有時間界線"
        );
        assert_eq!(
            db.scan_horizon_days("報告").expect("horizon"),
            None,
            "查不到的兩字詞也一樣：走得到索引就是真的翻完了"
        );

        // 資料庫本身還不到 30 天：掃描確實看完了全部，這時候多一句免責
        // 聲明只是雜訊——而雜訊會讓真的那一句被忽略。
        let mut short = test_db();
        let s2 = short.start_session("test", "0.0.1").expect("session");
        for ts in [now - 2 * 86_400_000, now] {
            short
                .insert_frame(
                    s2,
                    &frame_with_text(ts, "chrome.exe", "客服系統", &["客服專線紀錄"]),
                    None,
                    0,
                )
                .expect("insert");
        }
        assert_eq!(short.scan_horizon_days("錢").expect("horizon"), None);
    }

    /// 兩個字的中文詞**不再有時間界線**。
    ///
    /// 這條測試以前叫 `the_like_scan_does_not_look_past_its_window`，斷言的
    /// 正好相反：第 31 天前的那列**查不到**。那是實話，但那個實話是個缺陷——
    /// trigram 比不了 <3 字、unicode61 把整串 CJK 當一個 token，所以「客服」
    /// 只剩全表掃描，而掃描的成本跟使用時間成正比，只好用 30 天封住。
    ///
    /// schema 3 的 bigram 索引把那個縫補起來了，所以界線該消失——
    /// 而「消失」要有人驗，不然它會安靜地變回來。
    #[test]
    fn a_two_character_chinese_word_reaches_past_the_old_scan_window() {
        let mut db = test_db();
        let s = db.start_session("test", "0.0.1").expect("session");

        let now = 1_700_000_000_000i64;
        let inside = now - (LIKE_SCAN_DAYS - 1) * 86_400_000;
        let outside = now - (LIKE_SCAN_DAYS + 1) * 86_400_000;
        let ancient = now - 300 * 86_400_000;

        for (ts, marker) in [
            (ancient, "古"),
            (outside, "舊"),
            (inside, "新"),
            (now, "今"),
        ] {
            db.insert_frame(
                s,
                &frame_with_text(
                    ts,
                    "chrome.exe",
                    "客服系統",
                    &[&format!("{marker}客服專線紀錄")],
                ),
                None,
                0,
            )
            .expect("insert");
        }

        let hits = db.search("客服", 20).expect("search");
        let texts: Vec<&str> = hits.iter().map(|h| h.text.as_str()).collect();

        for marker in ['今', '新', '舊', '古'] {
            assert!(
                texts.iter().any(|t| t.starts_with(marker)),
                "「{marker}」那一列查不到——兩個字的中文又掉回時間界線裡了：{texts:?}"
            );
        }
    }

    /// `doctor` 報的覆蓋率要跟搜尋真的找得到的東西一致。
    ///
    /// migration 003 要回填舊資料。回填漏掉的話，`doctor` 仍然可以印一個
    /// 漂亮的數字，而舊資料再也叫不出來——這裡把兩邊綁在一起：報幾行進了
    /// 索引，就要真的查得到幾行。
    #[test]
    fn the_coverage_doctor_reports_matches_what_search_can_actually_reach() {
        let mut db = test_db();
        let s = db.start_session("test", "0.0.1").expect("session");

        assert_eq!(
            db.bigram_coverage().expect("empty"),
            (0, 0),
            "空資料庫不該報出任何覆蓋率"
        );

        let now = 1_700_000_000_000i64;
        for (ts, marker) in [
            (now - 300 * 86_400_000, "古"),
            (now - 40 * 86_400_000, "舊"),
            (now, "今"),
        ] {
            db.insert_frame(
                s,
                &frame_with_text(
                    ts,
                    "chrome.exe",
                    "客服系統",
                    &[&format!("{marker}客服專線紀錄")],
                ),
                None,
                0,
            )
            .expect("insert");
        }

        let (indexed, with_cjk) = db.bigram_coverage().expect("coverage");
        assert_eq!(with_cjk, 3, "三列都有中文");
        assert_eq!(indexed, 3, "三列都該進索引");

        let reachable = db.search("客服", 20).expect("search").len();
        assert_eq!(
            reachable as i64, with_cjk,
            "doctor 說 {indexed} 行進了索引，搜尋卻只交出 {reachable} 行"
        );
    }

    /// 遮蔽稽核問的是資料庫，不是旗子。
    ///
    /// 「內容沒有落地」是這份文件裡最強的一句承諾，而它唯一的失敗方式是
    /// 旗子插了、字還在——那種失敗不會報錯，錄製摘要照樣印「內容未落地」。
    /// 這個測試就是把那種狀態直接做出來，確認稽核看得見。
    #[test]
    fn a_redacted_clipboard_row_that_still_holds_its_text_is_reported_as_leaked() {
        let mut db = test_db();
        let s = db.start_session("test", "0.0.1").expect("session");

        // 正常的：判定為秘密，內容真的沒寫進去
        db.insert_clipboard(
            s,
            &ClipboardEvent {
                ts: 100,
                kind: ClipboardKind::Text,
                text: None,
                byte_len: 40,
                truncated: false,
                secret_suspected: true,
                source_app: Some("1password".into()),
            },
        )
        .expect("insert redacted");

        let clean = db.redaction_audit().expect("audit");
        assert_eq!(clean.flagged, 1);
        assert_eq!(clean.leaked, 0, "內容確實沒落地時不該報警");

        // 故障的：旗子插了，字還在
        db.insert_clipboard(
            s,
            &ClipboardEvent {
                ts: 200,
                kind: ClipboardKind::Text,
                text: Some("AKIAIOSFODNN7EXAMPLE".into()),
                byte_len: 20,
                truncated: false,
                secret_suspected: true,
                source_app: Some("terminal".into()),
            },
        )
        .expect("insert leaky");

        let leaky = db.redaction_audit().expect("audit");
        assert_eq!(leaky.flagged, 2);
        assert_eq!(leaky.leaked, 1, "旗子插了但字還在，稽核沒看見");
    }

    /// 錄製結束之後，還叫得回來「哪條規則擋掉了什麼、擋了幾次」。
    ///
    /// 這件事以前只有錄製當下的記憶體統計答得出來。使用者關掉終端機之後，
    /// 「排除真的生效了嗎」就再也無從查證——而文件說這張表就是為了讓他能查證
    /// 才存的。稽核紀錄查不回來等於沒有稽核紀錄。
    #[test]
    fn the_exclusion_audit_survives_the_recording_that_wrote_it() {
        let mut db = test_db();
        let s = db.start_session("test", "0.0.1").expect("session");
        for (ts, reason) in [
            (100, "app 在排除清單：1password"),
            (300, "app 在排除清單：1password"),
            (200, "網址在排除清單：bank.example.com"),
        ] {
            db.insert_system(
                s,
                &SystemEvent {
                    ts,
                    kind: SystemKind::Excluded,
                    detail: Some(reason.into()),
                },
            )
            .expect("insert");
        }
        // 不是排除的事件不該混進來
        db.insert_system(
            s,
            &SystemEvent {
                ts: 400,
                kind: SystemKind::Lock,
                detail: None,
            },
        )
        .expect("insert lock");

        let audit = db.exclusion_audit().expect("audit");
        assert_eq!(audit.len(), 2, "應該依理由分組：{audit:?}");
        assert_eq!(audit[0].reason, "app 在排除清單：1password", "多的排前面");
        assert_eq!(audit[0].episodes, 2);
        assert_eq!(audit[0].first_ts, 100);
        assert_eq!(audit[0].last_ts, 300);
        assert_eq!(audit[1].episodes, 1);
    }

    #[test]
    fn deleting_a_chunk_removes_its_facts_and_fts_rows() {
        // 「刪得掉」是護城河（SPEC §7.2 / §11.4）——cascade 必須真的成立
        let mut db = test_db();
        let s = db.start_session("test", "0.0.1").expect("session");
        db.insert_frame(
            s,
            &frame_with_text(1, "chrome.exe", "bill", &["帳單 NT$13,450"]),
            None,
            0,
        )
        .expect("insert");
        assert!(!db.search("帳單", 10).expect("search").is_empty());

        db.conn()
            .execute("DELETE FROM text_chunks", [])
            .expect("delete chunks");

        assert!(
            db.search("帳單", 10).expect("search").is_empty(),
            "FTS must forget too"
        );
        let st = db.stats().expect("stats");
        assert_eq!(st.facts, 0, "facts must cascade with their chunk");
    }

    #[test]
    fn fts_query_escaping() {
        assert_eq!(fts_query("客服電話"), "\"客服電話\"");
        assert_eq!(fts_query("hello world"), "\"hello\" AND \"world\"");
        assert_eq!(fts_query("say \"hi\""), "\"say\" AND \"\"\"hi\"\"\"");
        assert_eq!(fts_query("   "), "");
    }

    fn put_day_summary(
        db: &Db,
        date: &str,
        version: i32,
        narrative: &str,
        refs_json: &str,
        stats_json: &str,
        tombstoned_at: Option<i64>,
    ) {
        db.conn
            .execute(
                "INSERT INTO day_summaries(
                    date, version, supersedes, narrative, session_refs_json,
                    stats_json, created_at, tombstoned_at
                 ) VALUES(?1,?2,NULL,?3,?4,?5,1,?6)",
                params![
                    date,
                    version,
                    narrative,
                    refs_json,
                    stats_json,
                    tombstoned_at
                ],
            )
            .expect("insert day_summaries");
    }

    fn put_eod(db: &mut Db, day_key: &str, skip_reason: Option<&str>) {
        db.insert_reviewer_run(&ReviewerRunInsert {
            ts: 1,
            day_key,
            kind: "eod",
            skip_reason,
            candidate_count: Some(0),
            recheck_count: Some(0),
            wrote_commitments: 0,
            divergences: 0,
            calls_used: 0,
            budget_used: 0,
            budget_limit: 40,
            detail: "",
        })
        .expect("insert reviewer_run");
    }

    /// `latest_day_summary` 用 `tombstoned_at IS NULL`，墓碑列不會回來。
    #[test]
    fn latest_day_summary_does_not_return_a_tombstone() {
        let db = test_db();
        put_day_summary(&db, "2026-08-19", 1, "", "[]", "{}", Some(99));
        assert!(
            db.latest_day_summary("2026-08-19").unwrap().is_none(),
            "墓碑列被 latest_day_summary 當成還活著"
        );
        assert_eq!(
            db.day_summary_tombstoned_at("2026-08-19").unwrap(),
            Some(99)
        );
    }

    /// 三種「沒有日摘要」在型別上就分開。不能靠「現在有沒有 L2」反推。
    #[test]
    fn three_missing_day_summaries_are_three_kinds() {
        let mut db = test_db();
        let never = db.day_summary_glance("2026-08-17").unwrap();
        let empty = {
            // 日終在 19 跑，盤點的是 18。day_key 是跑的那天，不是被盤點的那天。
            put_eod(&mut db, "2026-08-19", None);
            db.day_summary_glance("2026-08-18").unwrap()
        };
        assert!(
            matches!(
                db.day_summary_glance("2026-08-19").unwrap(),
                DaySummaryGlance::NeverRan { .. }
            ),
            "跑的那天自己還沒被盤點，不能因為 stamp 在那天就講成她看過"
        );
        let gone = {
            put_day_summary(&db, "2026-08-19", 1, "", "[]", "{}", Some(50));
            db.day_summary_glance("2026-08-19").unwrap()
        };
        assert!(
            matches!(never, DaySummaryGlance::NeverRan { .. }),
            "沒跑過日終卻不是 NeverRan：{never:?}"
        );
        assert!(
            matches!(empty, DaySummaryGlance::EodEmpty { .. }),
            "日終跑過沒寫列卻不是 EodEmpty：{empty:?}"
        );
        assert!(
            matches!(gone, DaySummaryGlance::Tombstoned { .. }),
            "墓碑被講成從來沒有：{gone:?}"
        );
        let never_s = serde_json::to_string(&never).unwrap();
        let empty_s = serde_json::to_string(&empty).unwrap();
        let gone_s = serde_json::to_string(&gone).unwrap();
        assert_ne!(never_s, empty_s);
        assert_ne!(never_s, gone_s);
        assert_ne!(empty_s, gone_s);
        assert!(
            !gone_s.contains("narrative"),
            "墓碑的 JSON 把已清空的內容帶出去了：{gone_s}"
        );
    }

    /// 跳過的日終不算跑過。同意書沒勾會在 reviewer_run 留下 skip_reason，
    /// 那是 NeverRan，不是「她看過、那天沒卡片」。
    #[test]
    fn a_skipped_eod_is_not_a_ran_eod() {
        let mut db = test_db();
        // 星期二凌晨跳過；本該盤點的是星期一。skip_reason 必須真的被看到，
        // 不能只靠「跑的那天 ≠ 被問的那天」混過去。
        put_eod(&mut db, "2026-08-21", Some("no_consent"));
        assert!(!db.has_reviewer_eod_for_day("2026-08-20").unwrap());
        assert!(matches!(
            db.day_summary_glance("2026-08-20").unwrap(),
            DaySummaryGlance::NeverRan { .. }
        ));
    }

    /// 區間審閱跑過，不能拿來當成日終跑過。
    #[test]
    fn an_interval_run_is_not_eod() {
        let mut db = test_db();
        db.insert_reviewer_run(&ReviewerRunInsert {
            ts: 1,
            day_key: "2026-08-22",
            kind: "interval",
            skip_reason: None,
            candidate_count: Some(0),
            recheck_count: Some(0),
            wrote_commitments: 0,
            divergences: 0,
            calls_used: 0,
            budget_used: 0,
            budget_limit: 40,
            detail: "",
        })
        .unwrap();
        assert!(!db.has_reviewer_eod_for_day("2026-08-21").unwrap());
        assert!(matches!(
            db.day_summary_glance("2026-08-21").unwrap(),
            DaySummaryGlance::NeverRan { .. }
        ));
    }

    #[test]
    fn live_day_summary_pairs_clauses_with_the_same_l2_refs() {
        let db = test_db();
        put_day_summary(
            &db,
            "2026-08-22",
            2,
            "在改測試；在讀 SPEC",
            r#"["l2:11","l2:12"]"#,
            r#"{"l2":2,"commitments_open":1}"#,
            None,
        );
        match db.day_summary_glance("2026-08-22").unwrap() {
            DaySummaryGlance::Live {
                version,
                supersedes,
                aligned,
                l2,
                commitments_open,
                clauses,
                ..
            } => {
                assert_eq!(version, 2);
                assert_eq!(supersedes, None);
                assert!(aligned);
                assert_eq!(l2, Some(2));
                assert_eq!(commitments_open, Some(1));
                assert_eq!(
                    clauses,
                    vec![
                        DaySummaryClause {
                            text: "在改測試".into(),
                            l2_id: Some(11),
                        },
                        DaySummaryClause {
                            text: "在讀 SPEC".into(),
                            l2_id: Some(12),
                        },
                    ]
                );
            }
            other => panic!("該是 live：{other:?}"),
        }
    }

    /// 標題裡自己有全形分號時，切開的段數會比 refs 多。對不上就不配。
    #[test]
    fn a_mismatched_narrative_is_not_silently_zipped() {
        let (clauses, aligned) = clauses_from("甲；乙；丙", r#"["l2:1","l2:2"]"#);
        assert!(!aligned);
        assert!(clauses.iter().all(|c| c.l2_id.is_none()));
        assert_eq!(
            clauses.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(),
            ["甲", "乙", "丙"]
        );
    }

    /// stats 缺鍵不是 0。沒量到和量到 0 印出來不能長一樣。
    #[test]
    fn missing_stats_keys_are_none_not_zero() {
        assert_eq!(parse_day_stats("{}"), (None, None));
        assert_eq!(parse_day_stats("not-json"), (None, None));
        assert_eq!(
            parse_day_stats(r#"{"l2":0,"commitments_open":0}"#),
            (Some(0), Some(0))
        );
    }

    /// 日終 stamp 的是跑的那天。被盤點的是它的前一天，不是任何更早的一天。
    #[test]
    fn eod_run_stamp_is_the_morning_after_the_day_it_reviewed() {
        let mut db = test_db();
        // 正常隔天跑 / 開機補跑：星期二凌晨的成功 eod 盤點星期一。
        put_eod(&mut db, "2026-08-25", None);
        assert!(db.has_reviewer_eod_for_day("2026-08-24").unwrap());
        assert!(!db.has_reviewer_eod_for_day("2026-08-25").unwrap());
        // 關機三天後開機：星期一補的是星期天，Friday 沒被盤點。
        put_eod(&mut db, "2026-08-24", None);
        assert!(db.has_reviewer_eod_for_day("2026-08-23").unwrap());
        assert!(
            !db.has_reviewer_eod_for_day("2026-08-21").unwrap(),
            "更早的日子不能因為後來某天跑過日終就被講成看過"
        );
        assert!(!db.has_reviewer_eod_for_day("2026-08-22").unwrap());
    }

    /// 活著的新版蓋掉墓碑舊版時，畫面上是新版，不是「被刪掉了」。
    #[test]
    fn a_live_version_beats_an_older_tombstone() {
        let db = test_db();
        put_day_summary(&db, "2026-08-23", 1, "", "[]", "{}", Some(10));
        put_day_summary(
            &db,
            "2026-08-23",
            2,
            "後來又寫了一列",
            r#"["l2:9"]"#,
            r#"{"l2":1}"#,
            None,
        );
        match db.day_summary_glance("2026-08-23").unwrap() {
            DaySummaryGlance::Live {
                version, clauses, ..
            } => {
                assert_eq!(version, 2);
                assert_eq!(clauses[0].text, "後來又寫了一列");
            }
            other => panic!("該是 live：{other:?}"),
        }
    }
}
