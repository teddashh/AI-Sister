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
use std::path::Path;

use crate::facts::ExtractedFact;
use crate::model::{
    ClipboardEvent, FocusEvent, FocusSnapshot, FrameCapture, InputMetrics, Millis, SearchHit,
    SourceKind, SystemEvent, now_ms,
};

/// 目前的 schema 版本。每次改結構就 +1 並附一段 migration。
pub const SCHEMA_VERSION: i32 = 4;

const MIGRATION_001: &str = r#"
CREATE TABLE meta (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE sessions (
  id          INTEGER PRIMARY KEY,
  started_at  INTEGER NOT NULL,
  ended_at    INTEGER,
  app_version TEXT NOT NULL,
  platform    TEXT NOT NULL,
  note        TEXT
);

-- L0：畫面。image_path 為 NULL 代表 text-only 保留模式。
CREATE TABLE frames (
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
CREATE INDEX idx_frames_ts ON frames(ts);

-- L0：OCR 區塊幾何。文字本身另存 text_chunks 供檢索。
CREATE TABLE ocr_blocks (
  id         INTEGER PRIMARY KEY,
  frame_id   INTEGER NOT NULL REFERENCES frames(id) ON DELETE CASCADE,
  text       TEXT NOT NULL,
  x INTEGER, y INTEGER, w INTEGER, h INTEGER,
  confidence REAL
);
CREATE INDEX idx_ocr_frame ON ocr_blocks(frame_id);

CREATE TABLE focus_events (
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
CREATE INDEX idx_focus_ts ON focus_events(ts);

CREATE TABLE clipboard_events (
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
CREATE INDEX idx_clip_ts ON clipboard_events(ts);

-- L0：輸入動態。永遠不含按鍵內容，只有節奏與計數。
CREATE TABLE input_metrics (
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
CREATE INDEX idx_input_ts ON input_metrics(ts_start);

CREATE TABLE system_events (
  id         INTEGER PRIMARY KEY,
  ts         INTEGER NOT NULL,
  session_id INTEGER REFERENCES sessions(id),
  kind       TEXT NOT NULL,
  detail     TEXT
);
CREATE INDEX idx_sys_ts ON system_events(ts);

-- 統一文字層：所有可檢索文字的單一入口（FTS 的 external content）。
CREATE TABLE text_chunks (
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
CREATE INDEX idx_chunk_ts ON text_chunks(ts);
CREATE INDEX idx_chunk_frame ON text_chunks(frame_id);

CREATE VIRTUAL TABLE text_fts USING fts5(
  text, content='text_chunks', content_rowid='id', tokenize='trigram'
);
CREATE VIRTUAL TABLE text_fts_uni USING fts5(
  text, content='text_chunks', content_rowid='id', tokenize='unicode61'
);

CREATE TRIGGER text_chunks_ai AFTER INSERT ON text_chunks BEGIN
  INSERT INTO text_fts(rowid, text) VALUES (new.id, new.text);
  INSERT INTO text_fts_uni(rowid, text) VALUES (new.id, new.text);
END;
CREATE TRIGGER text_chunks_ad AFTER DELETE ON text_chunks BEGIN
  INSERT INTO text_fts(text_fts, rowid, text) VALUES('delete', old.id, old.text);
  INSERT INTO text_fts_uni(text_fts_uni, rowid, text) VALUES('delete', old.id, old.text);
END;

-- L1：程式抽出的 typed facts。零 LLM、零幻覺。
CREATE TABLE facts (
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
CREATE INDEX idx_facts_kind_ts ON facts(kind, ts);
CREATE INDEX idx_facts_norm ON facts(normalized);
CREATE INDEX idx_facts_ts ON facts(ts);
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
CREATE VIRTUAL TABLE text_fts_bi USING fts5(text, tokenize='unicode61');

DROP TRIGGER text_chunks_ad;
CREATE TRIGGER text_chunks_ad AFTER DELETE ON text_chunks BEGIN
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
CREATE TABLE queries (
  id         INTEGER PRIMARY KEY,
  ts         INTEGER NOT NULL,
  question   TEXT NOT NULL,
  shape      TEXT NOT NULL,
  hits       INTEGER NOT NULL,
  latency_ms INTEGER NOT NULL,
  source     TEXT NOT NULL
);
CREATE INDEX idx_query_ts ON queries(ts);

CREATE TABLE query_clicks (
  id       INTEGER PRIMARY KEY,
  query_id INTEGER NOT NULL REFERENCES queries(id) ON DELETE CASCADE,
  chunk_id INTEGER NOT NULL,
  rank     INTEGER NOT NULL,
  ts       INTEGER NOT NULL
);
CREATE INDEX idx_click_query ON query_clicks(query_id);
"#;

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

        // 一級一級走，每一級蓋自己的版號。
        //
        // 這裡本來是「跑完 001 就蓋成 SCHEMA_VERSION」。那樣寫的話，002 若在
        // 半路失敗（舊版 SQLite 不支援 DROP COLUMN 之類），資料庫已經被蓋成
        // 「最新」了——下次開機它不會重試，只會安安靜靜地少跑了一段。
        // 每段各蓋各的，失敗就停在上一段，下次自己接著跑。
        if version < 1 {
            let tx = self.conn.transaction()?;
            tx.execute_batch(MIGRATION_001).context("migration 001")?;
            tx.execute(
                "INSERT OR REPLACE INTO meta(key, value) VALUES('created_at', ?1)",
                params![now_ms().to_string()],
            )?;
            tx.commit()?;
            self.conn.pragma_update(None, "user_version", 1)?;
        }
        if version < 2 {
            let tx = self.conn.transaction()?;
            tx.execute_batch(MIGRATION_002).context("migration 002")?;
            tx.commit()?;
            self.conn.pragma_update(None, "user_version", 2)?;
        }
        if version < 3 {
            let tx = self.conn.transaction()?;
            tx.execute_batch(MIGRATION_003).context("migration 003")?;
            // 回填舊資料。bigram 要在 Rust 這邊算，所以這段不能寫進 SQL——
            // 少了它，升級上來的資料庫會有一個空索引，然後兩個字的中文查詢
            // 悄悄地只查得到升級之後的東西。
            {
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
            tx.commit()?;
            self.conn.pragma_update(None, "user_version", 3)?;
        }
        if version < 4 {
            let tx = self.conn.transaction()?;
            tx.execute_batch(MIGRATION_004).context("migration 004")?;
            tx.commit()?;
            self.conn.pragma_update(None, "user_version", 4)?;
        }
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

    // ---------- sessions ----------

    pub fn start_session(&mut self, platform: &str, app_version: &str) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO sessions(started_at, app_version, platform) VALUES(?1, ?2, ?3)",
            params![now_ms(), app_version, platform],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn end_session(&mut self, session_id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE sessions SET ended_at = ?1 WHERE id = ?2",
            params![now_ms(), session_id],
        )?;
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
    pub fn pause_spans(&self, from_ts: Millis, to_ts: Millis) -> Result<Vec<PauseSpan>> {
        let mut stmt = self.conn.prepare(
            "SELECT kind, ts FROM system_events
             WHERE kind IN ('pause', 'resume') AND ts < ?1 ORDER BY ts, id",
        )?;
        let rows = stmt.query_map([to_ts], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Millis>(1)?))
        })?;

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
        all.retain(|s| s.to.is_none_or(|t| t > from_ts));
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
    /// 回傳 `(全部, 沒收尾的, 最後一次沒收尾的時間)`。
    ///
    /// **有一個沒有被解決的歧義寫在這裡而不是被藏起來**：如果此刻另一個終端
    /// 機正在錄，那一段的 `ended_at` 也是 NULL，看起來跟當機一樣。可以靠存
    /// PID 再去問作業系統那個 PID 還在不在來分辨，但那是一條跨平台的、而且
    /// 會因為 PID 重用而給出錯誤答案的路。所以這裡不猜——呼叫端負責把這個
    /// 可能性講出來，而不是安靜地報一個可能是錯的數字。
    pub fn crash_audit(&self) -> Result<(i64, i64, Option<Millis>)> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(ended_at IS NULL), 0), MAX(CASE WHEN ended_at IS NULL THEN started_at END)
             FROM sessions",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?)
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
            "SELECT s.started_at, s.ended_at,
                    (SELECT e.detail FROM system_events e
                      WHERE e.session_id = s.id AND e.kind = 'session_end'
                      ORDER BY e.id DESC LIMIT 1)
             FROM sessions s ORDER BY s.id DESC LIMIT 1",
        )?;
        let row = stmt
            .query_row([], |r| {
                Ok(LastSession {
                    started_at: r.get(0)?,
                    ended_at: r.get(1)?,
                    reason: r.get(2)?,
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
    pub fn signal_audit(&self) -> Result<Vec<SignalAudit>> {
        let mut out = Vec::new();

        // 焦點事件：有列、卻沒有任何一列知道那是哪個 app。
        // 一段「不知道是哪個程式」的焦點事件不含任何資訊，那不是安靜，是壞了。
        let (focus, named): (i64, i64) = self.conn.query_row(
            "SELECT COUNT(*), COUNT(app_id) FROM focus_events",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        out.push(SignalAudit {
            name: "視窗焦點",
            rows: focus,
            populated: named,
            populated_label: "列知道自己是哪個 app",
            broken: focus > 0 && named == 0,
            note: "每段焦點事件應該知道自己是哪個 app",
        });

        // 打字節奏：有列、卻每一個計數器都是 0。
        // 擷取端在「這個窗口什麼都沒發生」時**根本不會寫列**（見
        // `windows/input.rs` 的早退），所以一列全 0 代表那道閘門壞了，
        // 不代表使用者沒動。
        let (input, active): (i64, i64) = self.conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(keystrokes + clicks + mouse_px + scroll_ticks > 0), 0)
             FROM input_metrics",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        out.push(SignalAudit {
            name: "輸入節奏",
            rows: input,
            populated: active,
            populated_label: "列真的有動作",
            broken: input > 0 && active == 0,
            note: "沒有動靜的窗口本來就不該寫列，所以全 0 的列是壞的不是閒的",
        });

        // 文字方框的座標：有列、卻全部疊在同一個位置。
        // 這是「以後要在畫面上把那行字圈起來」唯一的依據，而它壞掉的樣子
        // 是所有字都在 (0,0)——搜尋照樣全中，沒有任何地方會報錯。
        let (blocks, distinct): (i64, i64) = self.conn.query_row(
            "SELECT COUNT(*), COUNT(DISTINCT y) FROM ocr_blocks",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        out.push(SignalAudit {
            name: "文字座標",
            rows: blocks,
            populated: distinct,
            populated_label: "個不同的高度",
            broken: blocks > 1 && distinct <= 1,
            note: "一整張畫面的字不會全部在同一個高度",
        });

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
                 ORDER BY bm25({table})
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

    /// 最近問過的幾題，新的在前。
    pub fn query_log(&self, limit: usize) -> Result<Vec<QueryRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT q.id, q.ts, q.question, q.shape, q.hits, q.latency_ms, q.source,
                    (SELECT COUNT(*) FROM query_clicks c WHERE c.query_id = q.id)
             FROM queries q ORDER BY q.ts DESC, q.id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |r| {
            Ok(QueryRow {
                id: r.get(0)?,
                ts: r.get(1)?,
                question: r.get(2)?,
                shape: r.get(3)?,
                hits: r.get(4)?,
                latency_ms: r.get(5)?,
                source: r.get(6)?,
                clicks: r.get(7)?,
            })
        })?;
        Ok(rows.flatten().collect())
    }

    /// 題庫現在累積到哪裡了。
    ///
    /// `empty` 和 `clicked` 是這裡真正該看的兩個數字：前者是她答不出來的比例，
    /// 後者是答出來而且**真的有用**的比例。總數只說明他用了多少次。
    pub fn query_log_stats(&self) -> Result<QueryLogStats> {
        let mut stats = self.conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(hits = 0), 0),
                    (SELECT COUNT(DISTINCT query_id) FROM query_clicks),
                    MIN(ts), MAX(ts),
                    COALESCE(SUM(latency_ms > ?1), 0),
                    COALESCE(SUM(source = ?2), 0)
             FROM queries",
            params![RETRIEVAL_BUDGET_MS, SOURCE_DESKTOP],
            |r| {
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
                })
            },
        )?;
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
                   ORDER BY bm25(text_fts_bi)
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
             ORDER BY ts DESC LIMIT ?{}",
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

    /// 依 typed fact 直查（「帳單多少錢」「電話幾號」走這條，不經全文檢索）。
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
            sessions: count("SELECT COUNT(*) FROM sessions")?,
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
            db_bytes: {
                let page_count: i64 = self
                    .conn
                    .pragma_query_value(None, "page_count", |r| r.get(0))
                    .unwrap_or(0);
                let page_size: i64 = self
                    .conn
                    .pragma_query_value(None, "page_size", |r| r.get(0))
                    .unwrap_or(0);
                page_count * page_size
            },
        })
    }
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

/// 要記進題庫的一次提問（見 [`Db::log_query`]）。
///
/// 借用而不是持有，因為呼叫端已經有這些字串了，而這一步在使用者按下 Enter 之後
/// 的那條路上——不值得為了記一筆而多配置幾次記憶體。
pub struct QueryLogEntry<'a> {
    pub ts: Millis,
    /// 他打的**原話**。不做正規化：題庫要的正是真實的用詞。
    pub question: &'a str,
    /// `"recent"`／`"keywords"`。走哪條路本身就是一個要驗的判斷
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
}

/// 秘密遮蔽的實際結果（見 [`Db::redaction_audit`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RedactionAudit {
    /// 被判定為疑似秘密的剪貼簿事件數。
    pub flagged: i64,
    /// 其中內容**仍然留在資料庫裡**的筆數。任何大於 0 的值都是遮蔽失效。
    pub leaked: i64,
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
    /// `rows > 0` 而 `populated == 0`：這個狀態自相矛盾，不是「使用者很安靜」。
    ///
    /// 刻意不用「populated 佔比很低」當條件。低佔比在真實使用中隨時會發生
    /// （整個下午都在看影片、沒碰鍵盤），拿它報警等於製造一個大家學會忽略的
    /// 警告。這裡只認**不可能同時成立**的組合。
    pub broken: bool,
    /// 為什麼那個組合不可能——警告要能解釋自己。
    pub note: &'static str,
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
    pub system_events: i64,
    pub sessions: i64,
    pub first_ts: Option<Millis>,
    pub last_ts: Option<Millis>,
    pub db_bytes: i64,
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

        let preview = db.forget_preview(500, 2_000).expect("preview");
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
        db.end_session(clean).expect("end");
        let (all, unfinished, last) = db.crash_audit().expect("audit");
        assert_eq!((all, unfinished, last), (1, 0, None), "收好尾的不該算當機");

        // 開了就不收尾——程序被殺、當機、拔電，都長這樣。
        db.start_session("test", "0.0.1").expect("session");
        let (all, unfinished, last) = db.crash_audit().expect("audit");
        assert_eq!(all, 2);
        assert_eq!(unfinished, 1, "沒收尾的那一段沒有被算到");
        assert!(last.is_some(), "沒收尾的話要講得出是什麼時候");
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

        for a in db.signal_audit().expect("audit") {
            assert!(!a.broken, "{} 只是安靜，不該被說成壞掉", a.name);
        }
    }

    /// 有一堆列、每一列都是空殼——`COUNT(*)` 看起來完全正常的那種故障。
    #[test]
    fn rows_that_carry_nothing_are_reported_as_broken() {
        let mut db = test_db();
        let s = db.start_session("test", "0.0.1").expect("session");

        // 擷取端在「什麼都沒發生」時根本不該寫列（見 windows/input.rs 的
        // 早退）。所以一列全 0 代表那道閘門壞了，不是使用者很安靜。
        for i in 0..5 {
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
        db.insert_focus(
            s,
            &FocusEvent {
                ts: 0,
                kind: FocusKind::Focus,
                snapshot: FocusSnapshot::default(),
            },
        )
        .expect("insert focus");

        let audit = db.signal_audit().expect("audit");
        let broken: Vec<_> = audit.iter().filter(|a| a.broken).map(|a| a.name).collect();
        assert!(
            broken.contains(&"輸入節奏"),
            "五列全 0 的輸入沒被抓到：{audit:?}"
        );
        assert!(
            broken.contains(&"視窗焦點"),
            "不知道 app 的焦點事件沒被抓到：{audit:?}"
        );

        // 一列都沒有的訊號不算壞——那是「還沒錄到」，不是「錄壞了」。
        // 兩者的差別由 `rows` 講，不是由 `broken` 講。
        let coords = audit.iter().find(|a| a.name == "文字座標").expect("座標");
        assert_eq!(coords.rows, 0);
        assert!(!coords.broken, "空的表不該被說成壞掉");
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
}
