//! 本機儲存：L0 證據與 L1 事實。
//!
//! 憲法（SPEC §0）：這裡的資料 append-only、不經 LLM、不可改寫。
//! 一顆加密的 SQLite 檔裝下全部——備份、加密、刪除都只有一個對象。
//!
//! 檢索走 FTS5 雙索引（SPEC §15）：
//! - `text_fts`（trigram）：CJK 子字串比對的主力，繁中天然支援、免字典。
//! - `text_fts_uni`（unicode61）：補 trigram 的兩個洞——英文整詞、以及 <3 字的查詢。

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;

use crate::facts::ExtractedFact;
use crate::model::{
    ClipboardEvent, FocusEvent, FocusSnapshot, FrameCapture, InputMetrics, Millis, SearchHit,
    SourceKind, SystemEvent, now_ms,
};

/// 目前的 schema 版本。每次改結構就 +1 並附一段 migration。
pub const SCHEMA_VERSION: i32 = 2;

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
        if hits.is_empty() {
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

    /// 足跡統計——直接對應 Phase 0 的 exit criteria。
    pub fn stats(&self) -> Result<DbStats> {
        let count = |sql: &str| -> Result<i64> {
            Ok(self.conn.query_row(sql, [], |r| r.get(0)).unwrap_or(0))
        };
        Ok(DbStats {
            frames: count("SELECT COUNT(*) FROM frames")?,
            frames_collapsed: count("SELECT COALESCE(SUM(dup_run),0) FROM frames")?,
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
    Ok(tx.last_insert_rowid())
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
const LIKE_SCAN_DAYS: i64 = 30;

/// LIKE 後援命中的固定分數。負值確保它永遠排在任何 FTS 命中之後——
/// 它證明「有這段文字」，但不宣稱相關性。
const LIKE_SCORE: f64 = -1.0;

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

/// 秘密遮蔽的實際結果（見 [`Db::redaction_audit`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RedactionAudit {
    /// 被判定為疑似秘密的剪貼簿事件數。
    pub flagged: i64,
    /// 其中內容**仍然留在資料庫裡**的筆數。任何大於 0 的值都是遮蔽失效。
    pub leaked: i64,
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

        assert_eq!(db.schema_version().expect("version"), 2);
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
