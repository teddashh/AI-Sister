//! 保留期：到期的東西真的會消失。
//!
//! 這個模組補的是一個**已經被寫進文件的承諾**。`RetentionConfig` 早就存在、
//! `sister doctor` 早就印著「保留期　畫面 30 天／文字 365 天」、THREAT_MODEL
//! 的「減損設計」還把它列為爆炸半徑控制——而在這個檔案存在之前，**沒有任何
//! 一行程式碼會刪掉任何東西**。
//!
//! 這是本專案一路在修的那個形狀（THREAT_MODEL「安靜地不生效」）出現在
//! 最要命的位置：一條讀起來完全正確、使用者會據以放心、但什麼都沒做的規則。
//! 而且它比排除規則更難發現——排除失效至少還有「敏感畫面被錄進去」這個
//! 可被搜出來的事實，保留期失效則只是磁碟慢慢變大，沒有任何一刻會出事。
//!
//! ## 兩段式，不是一段式
//!
//! 畫面檔和它上面的字**不是同一件東西**：
//!
//! - 過了 `frames_days`：刪掉 PNG，但保留 frame 那一列與它的文字。
//!   「三個月前那通客服電話」還查得到，只是點不開當時的截圖。
//! - 過了 `text_days`：整列連同文字、事實、FTS 索引一起消失。
//!
//! ## 順序：先刪檔案，再改資料庫
//!
//! 中途死掉的話，兩種殘骸的性質完全不同：
//!
//! - 先刪 DB 再刪檔 → 一個沒有任何紀錄指向它的 PNG 永遠躺在磁碟上。
//!   那是一份**沒有人知道它存在**的螢幕截圖。
//! - 先刪檔再刪 DB → 資料庫說有圖但打不開。難看，但看得見、修得掉。
//!
//! 所以先刪檔案。這個專案在每一個岔路上都往「壞掉要看得見」那邊倒。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::config::RetentionConfig;
use crate::model::Millis;

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// 一次清理做掉了什麼。每一項都是「真的發生了」的數字，不是預估。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PruneReport {
    /// **真的從磁碟上刪掉**的畫面檔數（frame 那一列還在，文字也還在）。
    ///
    /// 資料庫說有幾列帶圖是一回事，`remove_file` 成功幾次是另一回事——
    /// 兩者對不起來的差額在 [`missing`](Self::missing)。
    pub images_deleted: u64,
    /// 上面那些檔案一共多大。**只算刪成功的那幾個。**
    pub image_bytes_freed: u64,
    /// 資料庫說有圖、磁碟上卻找不到那個檔的列數。
    ///
    /// 不是失敗（隱私上目標已達成，東西確實不在了），但也**不是**
    /// 「這次清掉的」。手動清空過 `frames/`、或從一份沒帶 `--with-frames`
    /// 的備份還原之後，這個數字會等於資料庫以為自己有的全部畫面——而
    /// 沒有這一欄的時候，那個情況和「順利刪掉 120 個檔、釋放 1.2 GB」
    /// 在報告上長得一模一樣。
    pub missing: u64,
    /// 整列消失的 frame 數。
    pub frames_deleted: u64,
    /// 連帶消失的可搜尋文字段落數。
    pub chunks_deleted: u64,
    /// 連帶消失的 L1 事實數。
    pub facts_deleted: u64,
    /// focus / clipboard / input / system 四張表加總。
    pub events_deleted: u64,
    /// 題庫裡消失的提問數（見 `Db::log_query`）。`query_clicks` 由 CASCADE 帶走。
    ///
    /// 單獨一個欄位而不是併進 `events_deleted`：那一欄裡的東西全是她自己觀察到
    /// 的，這一欄裡的是**他自己打的字**。「忘掉那一段」如果沒有把他那段時間問
    /// 過的話一起帶走，那句話就是半真的，而報告上看不出差別。
    pub queries_deleted: u64,
    /// 刪檔失敗的路徑（權限、檔案正被開著）。**不吞掉**。
    ///
    /// 刪不掉的截圖仍然躺在磁碟上，而使用者以為它已經不在了。這一項要
    /// 一路傳到看得見的地方去。
    pub failed: Vec<String>,
}

impl PruneReport {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// 幾天前的那一刻。`days == 0` 代表「不保留」，回傳 `now`。
///
/// 0 刻意解釋成**立刻刪除**而不是「永久保留」。這兩種讀法都說得通，
/// 而選擇的依據是失敗方向：把「不要留」讀成「永遠留」會產生一個
/// 使用者永遠不會發現的隱私缺口；反過來讀最壞是資料被刪光，那是
/// 當場就會發現、而且是他自己設定的。
pub fn cutoff(now: Millis, days: u32) -> Millis {
    now - (days as i64) * DAY_MS
}

/// 要刪掉的檔案（相對路徑 + 資料庫記的大小），以及刪完之後的下場。
///
/// 拆成獨立函式是為了讓「檔案系統那一半」可以在沒有資料庫的情況下測到。
///
/// **記帳也在這裡做，不在呼叫端。**三個呼叫端以前各自寫一次
/// 「`len` 減掉新增的 `failed` 就是刪掉的數量」，然後把**每一列**的
/// `image_bytes` 都加進 `image_bytes_freed`——包括那些檔案根本不在的列。
/// 手動清空過 `frames/`、或從一份沒帶 `--with-frames` 的備份還原之後，
/// 報告會說「刪掉了 120 個畫面檔（1.2 GB）」，而磁碟上一個位元組都沒有
/// 釋放。只有真的 `remove_file` 成功的那幾列才准記帳。
///
/// 回傳的是**刪不掉的那幾條相對路徑**。呼叫端一定要用它：那些檔案還在磁碟上，
/// 而把它們的資料庫紀錄清掉（`image_path = NULL` 或整列 DELETE）之後，就沒有
/// 任何東西指向它們了——下一次 prune 的條件是 `image_path IS NOT NULL`，所以
/// 它們永遠不會再被掃到。那正是這個模組開頭說「先刪檔再刪 DB」要避免的
/// 「一份沒有人知道它存在的螢幕截圖」，只是換成從一次失敗的刪除長出來的。
///
/// 「檔案本來就不在」不算在裡面：那個東西已經消失了，紀錄該跟著走。
pub(crate) fn delete_files<'a>(
    root: &Path,
    files: impl IntoIterator<Item = (&'a str, i64)>,
    report: &mut PruneReport,
) -> BTreeSet<String> {
    let mut dirs: BTreeSet<PathBuf> = BTreeSet::new();
    let mut stuck: BTreeSet<String> = BTreeSet::new();
    for (rel, bytes) in files {
        let full = root.join(rel);
        match std::fs::remove_file(&full) {
            Ok(()) => {
                report.images_deleted += 1;
                report.image_bytes_freed += bytes.max(0) as u64;
                if let Some(p) = full.parent() {
                    dirs.insert(p.to_path_buf());
                }
            }
            // 檔案本來就不在。隱私上目標已達成，所以不是 `failed`——但它
            // **也不是**「刪掉了」，而那正是使用者拿來對帳的數字。單獨數一
            // 欄，讓「資料庫說有 120 張、磁碟上一張都沒有」自己講出來。
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => report.missing += 1,
            Err(e) => {
                report.failed.push(format!("{}: {e}", full.display()));
                stuck.insert(rel.to_string());
            }
        }
    }
    // 空掉的 YYYY/MM/DD 目錄順手收掉。由深到淺，這樣 DD 空了之後 MM 才會空。
    // 失敗完全不管：一個空目錄不是隱私問題，只是不好看。
    for dir in dirs.iter().rev() {
        let mut cur = dir.as_path();
        while cur.starts_with(root) && cur != root {
            if std::fs::remove_dir(cur).is_err() {
                break;
            }
            match cur.parent() {
                Some(p) => cur = p,
                None => break,
            }
        }
    }
    stuck
}

/// `DELETE FROM frames`，但**跳過 `kept` 裡那幾列**。
///
/// `kept` 是圖刪不掉的那幾列（`delete_files` 回傳的那一批）。它們的 PNG 還躺在
/// 磁碟上，而整列刪掉之後就沒有任何東西指向那個檔案——下一次掃描的條件是
/// `image_path IS NOT NULL`，所以它永遠不會再被看見。留著那一列，下一輪再試。
///
/// `prune` 傳 `from = i64::MIN`（它的條件本來就只有上界），`forget` 傳真的區間。
fn delete_frames_except(
    tx: &rusqlite::Transaction<'_>,
    from_ts: Millis,
    to_ts: Millis,
    kept: &[i64],
) -> Result<u64> {
    let mut sql = String::from("DELETE FROM frames WHERE ts >= ?1 AND ts < ?2");
    let mut args: Vec<i64> = vec![from_ts, to_ts];
    if !kept.is_empty() {
        sql.push_str(" AND id NOT IN (");
        for (i, id) in kept.iter().enumerate() {
            if i > 0 {
                sql.push(',');
            }
            args.push(*id);
            sql.push_str(&format!("?{}", args.len()));
        }
        sql.push(')');
    }
    Ok(tx
        .execute(&sql, rusqlite::params_from_iter(args))
        .context("delete frames")? as u64)
}

impl crate::db::Db {
    /// 現在跑一次清理**會**刪掉什麼。只有 SELECT，不動任何東西。
    ///
    /// 刻意寫成獨立的函式而不是給 [`prune`](Self::prune) 加一個 `dry_run`
    /// 旗標：旗標會被忘記檢查，而這裡忘記檢查的後果是把使用者的資料
    /// 刪掉。這個函式裡沒有任何一句 DELETE，所以它不可能刪錯東西。
    pub fn prune_preview(&self, now: Millis, retention: &RetentionConfig) -> Result<PruneReport> {
        let text_cut = cutoff(now, retention.text_days);
        let frame_cut = cutoff(now, retention.frames_days);
        let n = |sql: &str, cut: Millis| -> Result<u64> {
            Ok(self.conn.query_row(sql, [cut], |r| r.get::<_, i64>(0))? as u64)
        };
        let mut r = PruneReport {
            chunks_deleted: n("SELECT COUNT(*) FROM text_chunks WHERE ts < ?1", text_cut)?,
            facts_deleted: n("SELECT COUNT(*) FROM facts WHERE ts < ?1", text_cut)?,
            frames_deleted: n("SELECT COUNT(*) FROM frames WHERE ts < ?1", text_cut)?,
            ..Default::default()
        };
        for sql in [
            "SELECT COUNT(*) FROM focus_events WHERE ts < ?1",
            "SELECT COUNT(*) FROM clipboard_events WHERE ts < ?1",
            "SELECT COUNT(*) FROM input_metrics WHERE ts_end < ?1",
            "SELECT COUNT(*) FROM system_events WHERE ts < ?1",
        ] {
            r.events_deleted += n(sql, text_cut)?;
        }
        // 題庫也要算進預覽。少了這一行，`sister prune --dry-run` 會說它不碰
        // 題庫，然後真的跑起來的時候把它刪掉——而預覽存在的全部意義就是
        // 「真的跑之前先看清楚」。
        r.queries_deleted = n("SELECT COUNT(*) FROM queries WHERE ts < ?1", text_cut)?;
        // 兩段的圖都會被刪掉，所以用比較舊的那條界線一次算完
        let cut = frame_cut.max(text_cut);
        r.images_deleted = n(
            "SELECT COUNT(*) FROM frames WHERE ts < ?1 AND image_path IS NOT NULL",
            cut,
        )?;
        r.image_bytes_freed = self.conn.query_row(
            "SELECT COALESCE(SUM(image_bytes),0) FROM frames WHERE ts < ?1 AND image_path IS NOT NULL",
            [cut],
            |r| r.get::<_, i64>(0),
        )? as u64;
        Ok(r)
    }

    /// 把過期的東西刪掉。回報**實際**刪了什麼。
    ///
    /// `image_root` 是畫面檔的根目錄；`None` 代表這次完全不碰畫面檔——
    /// **既不刪檔，也不動 `image_path`**。只清欄位不刪檔會讓那些檔案
    /// 從此沒有任何東西指向（下次的條件是 `image_path IS NOT NULL`），
    /// 於是永遠不會被清掉。
    ///
    /// 呼叫端幾乎都應該給 `Some`：`store_images = false` 的意思是「不要再
    /// 寫新的圖」，不是「昨天寫的那些不存在」。
    ///
    /// 過了 `text_days` 的整列刪除不受這個參數影響——那一段連紀錄都不留，
    /// 沒有「留著下次再刪」這個選項。
    pub fn prune(
        &mut self,
        now: Millis,
        retention: &RetentionConfig,
        image_root: Option<&Path>,
    ) -> Result<PruneReport> {
        let mut report = PruneReport::default();
        let text_cut = cutoff(now, retention.text_days);
        let frame_cut = cutoff(now, retention.frames_days);

        // ── 第一段：整列消失（過了 text_days）────────────────────────
        //
        // 先做這一段，這樣第二段就不會對已經注定要整列刪掉的 frame
        // 做一次多餘的 UPDATE。
        let doomed: Vec<(i64, String, i64)> = self
            .conn
            .prepare(
                "SELECT id, image_path, image_bytes FROM frames \
                 WHERE ts < ?1 AND image_path IS NOT NULL",
            )?
            .query_map([text_cut], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<std::result::Result<_, _>>()?;
        // 刪不掉的那幾列**要留著**。整列刪掉的話，那個還躺在磁碟上的 PNG 就
        // 沒有任何東西指向它了——這一段的條件是 `image_path IS NOT NULL`，所以
        // 下一次、下下次都掃不到它。模組開頭說「先刪檔再刪 DB」是為了避開
        // 「一份沒有人知道它存在的螢幕截圖」，而從一次失敗的刪除長出來的，
        // 是同一個東西。
        //
        // 代價是一列 366 天前的紀錄多活一輪（下一次 prune 是 5 分鐘後）。
        // 兩害相權，這個專案在每一個岔路上都往「壞掉要看得見」那邊倒。
        let mut kept: Vec<i64> = Vec::new();
        if let Some(root) = image_root {
            // 檔數和位元組都由 `delete_files` 一起記，這樣「刪掉 2 個畫面檔
            // （100 B）」裡的兩個數字一定來自同一批成功的 `remove_file`。
            let stuck = delete_files(
                root,
                doomed.iter().map(|(_, p, b)| (p.as_str(), *b)),
                &mut report,
            );
            kept = doomed
                .iter()
                .filter(|(_, p, _)| stuck.contains(p))
                .map(|(id, _, _)| *id)
                .collect();
        }

        let tx = self.conn.transaction()?;
        // **facts 要在 text_chunks 之前刪。**
        //
        // `facts.chunk_id` 是 ON DELETE CASCADE，所以先刪 chunks 的話，
        // 事實會被連帶帶走，這一句 DELETE 的 rowcount 就變成 0——東西
        // 確實消失了，但報告會說「刪掉了 0 個事實」。那是一個**假的零**，
        // 而假的零正是這個專案一路在修的東西。順序反過來，rowcount 就是
        // 真的。（`chunk_id` 為 NULL 的事實本來也只有這一句抓得到。）
        report.facts_deleted += tx
            .execute("DELETE FROM facts WHERE ts < ?1", [text_cut])
            .context("prune facts")? as u64;
        // text_chunks 一定要走 DELETE：AFTER DELETE 觸發器負責把兩個 FTS
        // 索引同步掉。繞過它會留下孤兒索引，搜尋會撈到已經不存在的內容
        // （DATA_INVENTORY 有記這一條）。
        report.chunks_deleted += tx
            .execute("DELETE FROM text_chunks WHERE ts < ?1", [text_cut])
            .context("prune text_chunks")? as u64;
        // ocr_blocks 由 frames 的 CASCADE 帶走。`kept` 是圖刪不掉的那幾列，
        // 見上面——它們留到下一輪，不然那些 PNG 會變成孤兒。
        report.frames_deleted += delete_frames_except(&tx, i64::MIN, text_cut, &kept)?;
        for (sql, col) in [
            ("DELETE FROM focus_events WHERE ts < ?1", "focus_events"),
            ("DELETE FROM clipboard_events WHERE ts < ?1", "clipboard"),
            (
                "DELETE FROM input_metrics WHERE ts_end < ?1",
                "input_metrics",
            ),
            ("DELETE FROM system_events WHERE ts < ?1", "system_events"),
        ] {
            report.events_deleted +=
                tx.execute(sql, [text_cut])
                    .with_context(|| format!("prune {col}"))? as u64;
        }
        // 題庫跟著文字的保留期走，不跟畫面。它記的是他問過什麼，而那和
        // 「那天的截圖還在不在」無關——文字都不留了，題目留著也沒有東西可以
        // 對答案。`query_clicks` 由 CASCADE 帶走。
        report.queries_deleted += tx
            .execute("DELETE FROM queries WHERE ts < ?1", [text_cut])
            .context("prune queries")? as u64;

        // ── 第二段：只丟掉圖，留下字（過了 frames_days）──────────────
        //
        // `kept` 要扣掉：那幾列是第一段剛剛刪檔失敗、才被留下來的，而在正常
        // 設定（`text_days ≥ frames_days`）下它們也一定落在這一段的範圍裡。
        // 不扣的話，同一個檔案在同一次 prune 裡會被試著刪兩次，`failed` 裡
        // 同一行也會印兩遍——看起來像兩個問題。一次跑動，一個檔案，一次機會。
        let stale: Vec<(i64, String, i64)> = tx
            .prepare(
                "SELECT id, image_path, image_bytes FROM frames \
                 WHERE ts < ?1 AND image_path IS NOT NULL",
            )?
            .query_map([frame_cut], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .filter(|row| !matches!(row, Ok((id, _, _)) if kept.contains(id)))
            .collect::<std::result::Result<_, _>>()?;
        tx.commit()?;

        // 沒給根目錄就整段不做。**不可以只把 `image_path` 清成 NULL**：
        // 那些檔案還躺在磁碟上，而清成 NULL 之後就沒有任何東西指向它們，
        // 下一次 prune 也永遠掃不到（條件是 `image_path IS NOT NULL`）。
        // 檔案從此不可能被刪，而「畫面 30 天後刪掉」對它們是一句假話。
        //
        // 兩害相權：留著一列指向真實檔案的紀錄，最多是「這一列的圖還沒清掉」，
        // 下次帶著根目錄再跑一次就好；清成 NULL 是不可逆的失憶。
        if let (false, Some(root)) = (stale.is_empty(), image_root) {
            let stuck = delete_files(
                root,
                stale.iter().map(|(_, p, b)| (p.as_str(), *b)),
                &mut report,
            );
            let tx = self.conn.transaction()?;
            {
                let mut up = tx.prepare(
                    "UPDATE frames SET image_path = NULL, image_bytes = 0 WHERE id = ?1",
                )?;
                // **只清刪成功的那幾列。** 上面那整段註解講的是「不給根目錄就
                // 整段不做」，而刪失敗的那幾列是同一件事的零售版：檔案還在，
                // 清成 NULL 之後就沒有任何東西指向它，下一次也掃不到。
                for (id, path, _) in &stale {
                    if !stuck.contains(path) {
                        up.execute([id])?;
                    }
                }
            }
            tx.commit()?;
        }

        Ok(report)
    }

    /// 選一段時間，**當作沒發生過**——會刪掉什麼。只有 SELECT。
    ///
    /// 和 `prune_preview` 同一個理由存在：使用者按下「忘掉」之前，要先看見
    /// 代價。這一段沒有回收桶、沒有復原。
    pub fn forget_preview(&self, from_ts: Millis, to_ts: Millis) -> Result<PruneReport> {
        if from_ts >= to_ts {
            return Ok(PruneReport::default());
        }
        let n = |sql: &str| -> Result<u64> {
            Ok(self
                .conn
                .query_row(sql, [from_ts, to_ts], |r| r.get::<_, i64>(0))? as u64)
        };
        let mut r = PruneReport {
            chunks_deleted: n("SELECT COUNT(*) FROM text_chunks WHERE ts >= ?1 AND ts < ?2")?,
            facts_deleted: n("SELECT COUNT(*) FROM facts WHERE ts >= ?1 AND ts < ?2")?,
            frames_deleted: n("SELECT COUNT(*) FROM frames WHERE ts >= ?1 AND ts < ?2")?,
            images_deleted: n("SELECT COUNT(*) FROM frames \
                 WHERE ts >= ?1 AND ts < ?2 AND image_path IS NOT NULL")?,
            image_bytes_freed: n("SELECT COALESCE(SUM(image_bytes),0) FROM frames \
                 WHERE ts >= ?1 AND ts < ?2 AND image_path IS NOT NULL")?,
            ..Default::default()
        };
        for sql in [
            "SELECT COUNT(*) FROM focus_events WHERE ts >= ?1 AND ts < ?2",
            "SELECT COUNT(*) FROM clipboard_events WHERE ts >= ?1 AND ts < ?2",
            "SELECT COUNT(*) FROM input_metrics WHERE ts_end > ?1 AND ts_start < ?2",
            "SELECT COUNT(*) FROM system_events WHERE ts >= ?1 AND ts < ?2",
        ] {
            r.events_deleted += n(sql)?;
        }
        r.queries_deleted = n("SELECT COUNT(*) FROM queries WHERE ts >= ?1 AND ts < ?2")?;
        Ok(r)
    }

    /// 把 `[from_ts, to_ts)` 這一段整個忘掉。
    ///
    /// ## 為什麼這裡**沒有**兩段式
    ///
    /// 保留期是兩段的：先丟圖、留字，過更久才整列消失。那個設計的前提是
    /// 「使用者還想記得這件事，只是不需要那張圖了」。
    ///
    /// 使用者親手選一段時間按下忘掉的時候，前提正好相反。留下 OCR 出來的字
    /// 而只刪掉圖，是把他說的那句話理解成他沒說過的另一句——而且他多半不會
    /// 再檢查一次。所以這裡只有一段：那段時間裡的每一張表都清乾淨。
    ///
    /// ## 邊界
    ///
    /// 左閉右開，和 `timeline` 同一套，所以按天連著忘不會漏掉或重複那一毫秒。
    /// `input_metrics` 例外，用的是**重疊**（`ts_end > from AND ts_start < to`）
    /// 而不是包含：一段跨過邊界的輸入節奏統計，裡面有一部分屬於他要忘掉的
    /// 那段時間。忘記的時候往多刪一格倒，和保留期往少刪一格倒的方向相反，
    /// 因為兩邊搞錯的後果不一樣。
    ///
    /// `image_root` 是 `None` 就完全不碰畫面檔：**連 `image_path` 都不清**，
    /// 理由和 `prune` 那邊一模一樣——清成 NULL 會讓那些檔案永遠沒人指得到。
    /// 但這裡的 `None` 幾乎一定是呼叫端的錯誤，所以整段跳過、報告裡的
    /// `images_deleted` 會是 0，讓它自己看起來不對勁。
    pub fn forget(
        &mut self,
        from_ts: Millis,
        to_ts: Millis,
        image_root: Option<&Path>,
    ) -> Result<PruneReport> {
        let mut report = PruneReport::default();
        // 空的或反過來的區間什麼都不做。一個 `to < from` 的請求如果被當成
        // 「全刪」，那就是一次不可逆的失憶，而它的來源只是某處算錯了日期。
        if from_ts >= to_ts {
            return Ok(report);
        }

        // 先刪檔案再改資料庫（模組開頭那條規則）。
        let doomed: Vec<(i64, String, i64)> = self
            .conn
            .prepare(
                "SELECT id, image_path, image_bytes FROM frames \
                 WHERE ts >= ?1 AND ts < ?2 AND image_path IS NOT NULL",
            )?
            .query_map([from_ts, to_ts], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<std::result::Result<_, _>>()?;
        let mut kept: Vec<i64> = Vec::new();
        if let Some(root) = image_root {
            let stuck = delete_files(
                root,
                doomed.iter().map(|(_, p, b)| (p.as_str(), *b)),
                &mut report,
            );
            kept = doomed
                .iter()
                .filter(|(_, p, _)| stuck.contains(p))
                .map(|(id, _, _)| *id)
                .collect();
        }

        let tx = self.conn.transaction()?;
        // facts 在 text_chunks 之前，理由見 `prune`：反過來的話 CASCADE 會
        // 先把事實帶走，rowcount 變成一個假的零。
        report.facts_deleted += tx
            .execute(
                "DELETE FROM facts WHERE ts >= ?1 AND ts < ?2",
                [from_ts, to_ts],
            )
            .context("forget facts")? as u64;
        report.chunks_deleted += tx
            .execute(
                "DELETE FROM text_chunks WHERE ts >= ?1 AND ts < ?2",
                [from_ts, to_ts],
            )
            .context("forget text_chunks")? as u64;
        // `kept` 是圖刪不掉的那幾列。整列刪掉的話那張 PNG 就沒人指得到了，
        // 而他按的是「忘掉」——留著一個他以為已經不存在的檔案，比留著一列
        // 指向它的紀錄更糟。留下來，下一次再試，而且報告裡看得到。
        report.frames_deleted += delete_frames_except(&tx, from_ts, to_ts, &kept)?;
        for (sql, what) in [
            (
                "DELETE FROM focus_events WHERE ts >= ?1 AND ts < ?2",
                "focus_events",
            ),
            (
                "DELETE FROM clipboard_events WHERE ts >= ?1 AND ts < ?2",
                "clipboard",
            ),
            (
                "DELETE FROM input_metrics WHERE ts_end > ?1 AND ts_start < ?2",
                "input_metrics",
            ),
            // 暫停紀錄也一起走。這會讓稽核出現孤兒 resume，而
            // `pause_audit` / `pause_spans` 本來就要面對那種情況（保留期
            // 也會這樣刪）——它們會說「這一段算不出長度」，不會少算一段。
            (
                "DELETE FROM system_events WHERE ts >= ?1 AND ts < ?2",
                "system_events",
            ),
        ] {
            report.events_deleted +=
                tx.execute(sql, [from_ts, to_ts])
                    .with_context(|| format!("forget {what}"))? as u64;
        }
        // 他那段時間問過的話也要走。留著的話，「那一段忘掉了」是半真的：
        // 螢幕上的東西沒了，但他打進搜尋框的字還在——而那些字往往比畫面更
        // 直接（他不會把不在意的事情打進去）。
        report.queries_deleted += tx
            .execute(
                "DELETE FROM queries WHERE ts >= ?1 AND ts < ?2",
                [from_ts, to_ts],
            )
            .context("forget queries")? as u64;
        tx.commit()?;

        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// 自己搭一個暫存目錄，和 `tests/privacy.rs` 同一套作法。
    ///
    /// 不引 `tempfile`：這個 crate 的相依樹是被 `scripts/check-no-network.sh`
    /// 盯著的資產之一，能不長就不長。
    struct Tmp(PathBuf);
    impl Tmp {
        fn new(name: &str) -> Self {
            static N: AtomicU32 = AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!(
                "sister-retention-{}-{name}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("create temp dir");
            Self(dir)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// 題庫跟著**文字**的保留期走，不是跟著畫面。
    ///
    /// 文字都不留了，題目留著也沒有東西可以對答案——那時候它只剩下「他曾經
    /// 打過這句話」這一個事實，而那正是這一欄裡最私人的部分。
    #[test]
    fn the_question_log_expires_with_the_text() {
        const DAY: i64 = 24 * 60 * 60 * 1000;
        let mut db = crate::Db::open_in_memory().expect("db");
        let now = 1_000 * DAY;
        for (ts, q) in [
            (now - 400 * DAY, "很久以前問的"),
            (now - 10 * DAY, "上禮拜問的"),
        ] {
            db.log_query(&crate::db::QueryLogEntry {
                ts,
                question: q,
                shape: "keywords",
                hits: 1,
                latency_ms: 1,
                source: "test",
            })
            .expect("log_query");
        }

        let report = db
            .prune(
                now,
                &RetentionConfig {
                    frames_days: 30,
                    text_days: 365,
                },
                None,
            )
            .expect("prune");
        assert_eq!(report.queries_deleted, 1, "過期的題目沒被清掉");
        assert_eq!(db.query_log_stats().expect("stats").total, 1);
    }

    #[test]
    fn zero_days_means_delete_now_not_keep_forever() {
        // 這條看起來像在測一行算術，實際上釘的是一個決定：`0` 有兩種
        // 讀法，而把「不要留」讀成「永遠留」會產生一個使用者永遠不會
        // 發現的隱私缺口。哪天有人「順手」改成 u32::MAX，這裡要紅。
        assert_eq!(cutoff(1_000_000, 0), 1_000_000);
        assert_eq!(cutoff(DAY_MS * 10, 3), DAY_MS * 7);
    }

    /// 檔案不在，不算失敗——但**也不算刪掉了**。
    ///
    /// 這兩件事以前擠在同一個計數器裡：手動清空過 `frames/`、或從一份沒帶
    /// `--with-frames` 的備份還原之後，報告會照著資料庫的列數說「刪掉了
    /// 120 個畫面檔（1.2 GB）」，而磁碟上一個位元組都沒有釋放，連一個 ⚠
    /// 都不會出現。
    #[test]
    fn a_missing_file_is_not_a_failure_but_it_is_not_a_deletion_either() {
        let dir = Tmp::new("missing");
        let mut report = PruneReport::default();
        delete_files(
            dir.path(),
            [("2020/01/01/nope.png", 1_200_000)],
            &mut report,
        );
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        assert_eq!(report.images_deleted, 0, "沒有刪到任何檔案");
        assert_eq!(report.image_bytes_freed, 0, "磁碟上一個位元組都沒有釋放");
        assert_eq!(report.missing, 1, "但資料庫確實有一列以為自己有圖");
    }

    #[test]
    fn emptied_date_directories_go_away_but_the_root_stays() {
        let dir = Tmp::new("empty-dirs");
        let deep = dir.path().join("2026/08/18");
        std::fs::create_dir_all(&deep).expect("mkdir");
        std::fs::write(deep.join("a.png"), b"x").expect("write");

        let mut report = PruneReport::default();
        delete_files(dir.path(), [("2026/08/18/a.png", 1)], &mut report);

        assert!(report.failed.is_empty());
        assert_eq!((report.images_deleted, report.image_bytes_freed), (1, 1));
        assert!(!dir.path().join("2026").exists(), "空掉的日期目錄該收掉");
        assert!(dir.path().exists(), "根目錄不能被連坐刪掉");
    }

    // ── 底下這幾條是這個模組存在的理由 ──────────────────────────────
    //
    // 「保留期 30 天」在這些測試出現之前是一句沒有任何東西撐著的話。
    // 每一條都經過變異驗證：把 prune 裡對應的那段刪掉，它就要紅。

    use crate::config::RetentionConfig;
    use crate::db::Db;
    use crate::model::{FocusSnapshot, FrameCapture, OcrBlock};

    const NOW: Millis = 1_800_000_000_000;

    fn days_ago(n: i64) -> Millis {
        NOW - n * DAY_MS
    }

    fn frame(ts: Millis, text: &str) -> FrameCapture {
        FrameCapture {
            ts,
            monitor: 0,
            width: 1920,
            height: 1080,
            dhash: ts as u64,
            image: None,
            image_ext: "png",
            ocr: vec![OcrBlock {
                text: text.into(),
                x: 0,
                y: 0,
                w: 400,
                h: 18,
                confidence: -1.0,
            }],
            focus: FocusSnapshot {
                app_id: Some("chrome.exe".into()),
                app_name: Some("chrome".into()),
                window_title: Some("測試".into()),
                url: None,
                pid: Some(1),
                password_field: false,
            },
        }
    }

    /// 建一顆有三個年紀的資料庫，並在磁碟上放對應的 PNG。
    fn seeded(tmp: &Tmp) -> (Db, Vec<String>) {
        let mut db = Db::open_in_memory().expect("db");
        let s = db.start_session("test", "0.0.1").expect("session");
        let mut paths = Vec::new();
        for (age, text) in [(1, "昨天"), (60, "兩個月前"), (400, "一年多前")] {
            let rel = format!("2026/{age:02}/01/{age}.png");
            let full = tmp.path().join(&rel);
            std::fs::create_dir_all(full.parent().expect("parent")).expect("mkdir");
            std::fs::write(&full, vec![0u8; 100]).expect("write png");
            db.insert_frame(s, &frame(days_ago(age), text), Some(&rel), 100)
                .expect("insert");
            paths.push(rel);
        }
        (db, paths)
    }

    /// 過了 `frames_days` 的畫面檔要真的從磁碟上消失——**但字要留著**。
    ///
    /// 這是這個模組最核心的一條。截圖是整份資料裡最敏感的東西，而
    /// 「三個月前那通客服電話」的價值幾乎全在文字上。把兩者綁在一起刪，
    /// 等於為了隱私把記憶也一起丟掉；不刪，等於文件在說謊。
    #[test]
    fn old_screenshots_are_deleted_from_disk_while_their_text_stays_searchable() {
        let tmp = Tmp::new("two-stage");
        let (mut db, paths) = seeded(&tmp);
        let retention = RetentionConfig {
            frames_days: 30,
            text_days: 365,
        };

        let r = db.prune(NOW, &retention, Some(tmp.path())).expect("prune");

        // 昨天的：檔案和字都還在
        assert!(tmp.path().join(&paths[0]).exists(), "昨天的畫面不該被碰");
        assert!(!db.search("昨天", 10).expect("search").is_empty());

        // 兩個月前的：檔案沒了，字還在
        assert!(
            !tmp.path().join(&paths[1]).exists(),
            "過了 30 天的 PNG 必須真的從磁碟上消失"
        );
        assert!(
            !db.search("兩個月前", 10).expect("search").is_empty(),
            "只過了 frames_days 的文字必須留著——不然使用者會發現她突然失憶"
        );

        // 一年多前的：整列都沒了
        assert!(!tmp.path().join(&paths[2]).exists());
        assert!(
            db.search("一年多前", 10).expect("search").is_empty(),
            "過了 text_days 就該連字一起消失"
        );

        assert_eq!(r.images_deleted, 2, "{r:?}");
        assert_eq!(r.frames_deleted, 1, "{r:?}");
        assert!(r.failed.is_empty(), "{:?}", r.failed);
        assert!(!r.is_empty());
    }

    /// 「圖留久一點、字早點清掉」在這個資料結構裡表達不出來——這是證據。
    ///
    /// 設定頁把兩格畫成互相獨立的，所以「畫面 3650 天、文字 30 天」是一個
    /// 使用者按得出來的組合。實際發生的事在下面：第一段按 `text_days` 整列
    /// 刪，而刪列之前一定得先刪檔（不然磁碟上會留一張沒有指標的截圖），於
    /// 是 PNG 在第 30 天就消失了，而那一格寫著 3650、旁邊的說明還寫著
    /// 「天後刪掉 PNG，但上面的字留著」。
    ///
    /// `RetentionConfig::check` 現在會擋掉這組數字。這個測試釘住**擋它的
    /// 理由是真的**——把 check 那一條拿掉的話，走進來的就是這個行為。
    #[test]
    fn a_screenshot_cannot_outlive_the_row_that_points_at_it() {
        let tmp = Tmp::new("inverted");
        let (mut db, paths) = seeded(&tmp);
        // 他要的：圖留十年、字只留 30 天。設定頁按得出來，check 會擋。
        let wished = RetentionConfig {
            frames_days: 3650,
            text_days: 30,
        };
        assert!(
            wished.check().is_err(),
            "這組數字要在寫進檔案之前就被擋下來"
        );

        // 但萬一它從別的路徑走進來了（舊的設定檔、手改的 TOML），
        // 真正發生的事是這個：
        let r = db.prune(NOW, &wished, Some(tmp.path())).expect("prune");
        assert!(tmp.path().join(&paths[0]).exists(), "昨天的還在");
        assert!(
            !tmp.path().join(&paths[1]).exists(),
            "他寫 3650，PNG 在第 30 天就沒了——實際壽命是 min(兩者)"
        );
        assert!(
            db.search("兩個月前", 10).expect("search").is_empty(),
            "字也一起走了"
        );
        assert_eq!(r.frames_deleted, 2, "{r:?}");
    }

    /// 不給根目錄時，**不可以只把欄位清掉而把檔案留在磁碟上**。
    ///
    /// 清成 NULL 之後就沒有任何東西指向那些檔案，而下一次 prune 的條件是
    /// `image_path IS NOT NULL`——它們從此不可能被掃到，也就永遠不會被刪。
    /// 「畫面 30 天後會消失」對它們是一句假話，而且是不可逆的：連要刪的人
    /// 都不知道該刪哪些。留著紀錄最多是「這次沒清到」，下次帶著根目錄再跑
    /// 一次就好。
    #[test]
    fn without_a_frames_dir_the_pointer_survives_so_the_file_can_still_be_reclaimed() {
        let tmp = Tmp::new("no-root");
        let (mut db, paths) = seeded(&tmp);
        let retention = RetentionConfig {
            frames_days: 30,
            text_days: 365,
        };

        let r = db.prune(NOW, &retention, None).expect("prune");

        // 兩個月前那張：檔案還在（我們沒有根目錄，刪不了）
        let stale = tmp.path().join(&paths[1]);
        assert!(stale.exists(), "沒有根目錄就不該假裝刪掉了");
        assert_eq!(r.images_deleted, 0, "一個檔案都沒刪，就不能說刪了");

        // 關鍵：指標必須還在，否則這個檔案從此沒有人找得到
        let still_pointed: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM frames WHERE image_path = ?1",
                [&paths[1]],
                |r| r.get(0),
            )
            .expect("query");
        assert_eq!(
            still_pointed, 1,
            "檔案還在磁碟上，指向它的那一列就不能被清成 NULL"
        );

        // 而且帶著根目錄再跑一次，它真的清得掉——這就是留著指標的意義
        let r2 = db.prune(NOW, &retention, Some(tmp.path())).expect("prune");
        assert!(!stale.exists(), "第二次帶了根目錄，就該真的刪掉");
        assert_eq!(r2.images_deleted, 1, "{r2:?}");
    }

    /// 把 `rel` 這條路徑做成一個**非空的資料夾**，讓 `remove_file` 一定失敗。
    ///
    /// 用 chmod 555 也能擋，但 root 跑測試的時候擋不住（CI 的容器常常是
    /// root），那會變成一條「在某些機器上靜靜地什麼都沒驗到」的測試——正好是
    /// 這個 commit 在修的那種謊。`remove_file` 對資料夾則是不管誰跑都會失敗。
    fn make_undeletable(root: &Path, rel: &str) {
        let full = root.join(rel);
        let _ = std::fs::remove_file(&full);
        std::fs::create_dir_all(&full).expect("mkdir");
        std::fs::write(full.join("occupied"), b"x").expect("write");
    }

    /// 一張**刪不掉**的截圖，不可以連紀錄一起消失。
    ///
    /// 磁碟滿了、防毒鎖住檔案、外接碟拔掉——`remove_file` 失敗之後，那個
    /// PNG 還躺在磁碟上。舊的程式碼照樣把整列 DELETE 掉（或把 `image_path`
    /// 清成 NULL），而下一次掃描的條件是 `image_path IS NOT NULL`，所以它從此
    /// 不可能再被掃到。⚠ 只會出現那一次，之後每一次 prune、每一次 doctor 都
    /// 會說「乾淨」——而那張截圖會在磁碟上待到這台電腦報廢。
    #[test]
    fn a_screenshot_that_could_not_be_deleted_keeps_the_row_that_points_at_it() {
        let tmp = Tmp::new("stuck-prune");
        let (mut db, paths) = seeded(&tmp);
        // paths[1] 過了 frames_days（只丟圖），paths[2] 過了 text_days（整列走）
        make_undeletable(tmp.path(), &paths[1]);
        make_undeletable(tmp.path(), &paths[2]);
        let retention = RetentionConfig {
            frames_days: 30,
            text_days: 365,
        };

        let r = db.prune(NOW, &retention, Some(tmp.path())).expect("prune");

        assert_eq!(r.failed.len(), 2, "{r:?}");
        assert_eq!(r.images_deleted, 0, "一個都沒刪成功，就不能記帳");
        assert_eq!(r.frames_deleted, 0, "刪不掉圖的那一列不可以整列消失");

        for rel in [&paths[1], &paths[2]] {
            let pointed: i64 = db
                .conn()
                .query_row(
                    "SELECT COUNT(*) FROM frames WHERE image_path = ?1",
                    [rel],
                    |r| r.get(0),
                )
                .expect("query");
            assert_eq!(pointed, 1, "{rel} 還在磁碟上，指向它的那一列就不能不見");
        }

        // 而且下一輪真的會再試一次——這就是留著那一列的全部意義
        for rel in [&paths[1], &paths[2]] {
            std::fs::remove_dir_all(tmp.path().join(rel)).expect("unblock");
            std::fs::write(tmp.path().join(rel), vec![0u8; 100]).expect("rewrite png");
        }
        let r2 = db.prune(NOW, &retention, Some(tmp.path())).expect("prune");
        assert!(r2.failed.is_empty(), "{:?}", r2.failed);
        assert_eq!(r2.images_deleted, 2, "{r2:?}");
        assert_eq!(r2.frames_deleted, 1, "過了 text_days 的那一列這次要走");
    }

    /// 他親手按下「忘掉」的那一段裡，刪不掉的圖同樣不可以變成孤兒。
    ///
    /// 這裡比保留期更嚴重：保留期只是到期，而他按的是「忘掉」。留下一個
    /// 他以為已經不存在的檔案，比留下一列指向它的紀錄糟得多——後者至少
    /// 下一次還刪得到。
    #[test]
    fn forgetting_does_not_orphan_a_screenshot_it_could_not_delete() {
        let tmp = Tmp::new("stuck-forget");
        let (mut db, paths) = seeded(&tmp);
        make_undeletable(tmp.path(), &paths[0]);

        let r = db
            .forget(days_ago(2), days_ago(0), Some(tmp.path()))
            .expect("forget");

        assert_eq!(r.failed.len(), 1, "{r:?}");
        assert_eq!(r.frames_deleted, 0, "圖還在，那一列就得留著");
        let pointed: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM frames WHERE image_path = ?1",
                [&paths[0]],
                |r| r.get(0),
            )
            .expect("query");
        assert_eq!(
            pointed, 1,
            "沒有人指得到的截圖，正是「忘掉」最不能留下的東西"
        );
        // 但字是真的走了：他要的那件事有做到
        assert!(
            db.search("昨天", 10).expect("search").is_empty(),
            "忘掉一段時間就該連字一起走，就算圖卡住了"
        );
    }

    /// FTS 是文字的**另一份副本**。刪 chunk 而索引沒跟著掉，等於資料
    /// 明明已經「刪掉」卻還搜得到——一個看起來像鬧鬼的隱私漏洞。
    #[test]
    fn expiring_text_takes_its_fts_rows_with_it() {
        let tmp = Tmp::new("fts");
        let (mut db, _) = seeded(&tmp);
        let retention = RetentionConfig {
            frames_days: 30,
            text_days: 365,
        };
        db.prune(NOW, &retention, Some(tmp.path())).expect("prune");

        let orphans: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM text_fts f \
                 WHERE NOT EXISTS (SELECT 1 FROM text_chunks c WHERE c.id = f.rowid)",
                [],
                |r| r.get(0),
            )
            .expect("count orphans");
        assert_eq!(orphans, 0, "FTS 裡不能留下指向已刪內容的孤兒列");
    }

    /// 沒有東西過期的時候，清理必須是完全的 no-op。
    ///
    /// 一個「每次開機都刪掉一點東西」的 bug 會非常難發現——資料是慢慢
    /// 少的，而且少掉的部分沒有人會去對帳。
    #[test]
    fn nothing_expired_means_nothing_touched() {
        let tmp = Tmp::new("noop");
        let (mut db, paths) = seeded(&tmp);
        let before = db.stats().expect("stats");

        let r = db
            .prune(
                NOW,
                &RetentionConfig {
                    frames_days: 3650,
                    text_days: 3650,
                },
                Some(tmp.path()),
            )
            .expect("prune");

        assert!(r.is_empty(), "什麼都沒過期就不該動任何東西：{r:?}");
        for p in &paths {
            assert!(tmp.path().join(p).exists(), "{p} 不該被刪");
        }
        assert_eq!(db.stats().expect("stats").frames, before.frames);
    }

    /// 預告要和實際發生的事對得上，否則 `--dry-run` 只是一種安慰。
    #[test]
    fn the_preview_matches_what_actually_happens() {
        let tmp = Tmp::new("preview");
        let (mut db, _) = seeded(&tmp);
        let retention = RetentionConfig {
            frames_days: 30,
            text_days: 365,
        };

        let preview = db.prune_preview(NOW, &retention).expect("preview");
        // 預告完之後資料必須一個都沒少
        assert_eq!(db.stats().expect("stats").frames, 3);

        let actual = db.prune(NOW, &retention, Some(tmp.path())).expect("prune");
        // 整個 struct 一次比，不要一欄一欄挑。挑的那一版漏掉了
        // `facts_deleted`，而那正是唯一對不起來的一欄：CASCADE 先把事實
        // 帶走，於是 DELETE 的 rowcount 是 0，報告印出「刪掉了 0 個事實」
        // ——東西真的不見了，數字卻是假的。挑著比的測試看不到自己沒挑的。
        assert_eq!(preview, actual, "預告和實際必須一模一樣");
    }

    /// 隔壁還有東西的時候不能連坐。
    #[test]
    fn a_directory_with_survivors_is_left_alone() {
        let dir = Tmp::new("survivors");
        let deep = dir.path().join("2026/08/18");
        std::fs::create_dir_all(&deep).expect("mkdir");
        std::fs::write(deep.join("a.png"), b"x").expect("write");
        std::fs::write(deep.join("b.png"), b"y").expect("write");

        let mut report = PruneReport::default();
        delete_files(dir.path(), [("2026/08/18/a.png", 1)], &mut report);

        assert!(deep.join("b.png").exists(), "還沒到期的畫面不能被連坐刪掉");
    }

    // ── 手動忘記一段時間 ────────────────────────────────────────────

    /// 他親手選一段時間按下忘掉的時候，**字也要一起消失**。
    ///
    /// 保留期是兩段式的（先丟圖、留字），而這裡刻意不是——同一顆資料庫、
    /// 同一批資料，兩條路徑要有不同的行為，所以值得一條測試釘住。留下 OCR
    /// 出來的字只刪圖，是把「當作沒發生過」執行成「把證據縮小一點」，而且
    /// 他不會再檢查第二次。
    #[test]
    fn forgetting_a_range_takes_the_words_too_not_just_the_pictures() {
        let tmp = Tmp::new("forget-words");
        let (mut db, paths) = seeded(&tmp);

        let r = db
            .forget(days_ago(61), days_ago(59), Some(tmp.path()))
            .expect("forget");

        assert!(
            !tmp.path().join(&paths[1]).exists(),
            "選中那一段的畫面必須從磁碟上消失"
        );
        assert!(
            db.search("兩個月前", 10).expect("search").is_empty(),
            "**字也要不見**——這正是 forget 和 prune 不一樣的地方"
        );

        // 隔壁兩天完全不受影響。一個會多刪的忘記鍵，比沒有更可怕。
        assert!(tmp.path().join(&paths[0]).exists());
        assert!(!db.search("昨天", 10).expect("search").is_empty());
        assert!(!db.search("一年多前", 10).expect("search").is_empty());

        assert_eq!(r.frames_deleted, 1, "{r:?}");
        assert_eq!(r.images_deleted, 1, "{r:?}");
        assert!(r.chunks_deleted >= 1, "{r:?}");
        assert!(r.failed.is_empty(), "{:?}", r.failed);
    }

    /// 反過來的區間什麼都不刪。
    ///
    /// 這條守的不是使用者，是呼叫端算錯日期的那一天。一個 `to < from` 的
    /// 請求若被當成「條件永遠成立」，就是一次不可逆的全刪——沒有回收桶、
    /// 沒有復原，而觸發它的只是某處把兩個參數寫反了。
    #[test]
    fn a_backwards_range_forgets_nothing() {
        let tmp = Tmp::new("forget-backwards");
        let (mut db, paths) = seeded(&tmp);

        let r = db
            .forget(NOW, days_ago(400), Some(tmp.path()))
            .expect("forget");

        assert!(r.is_empty(), "{r:?}");
        assert_eq!(db.stats().expect("stats").frames, 3, "一列都不能少");
        for p in &paths {
            assert!(tmp.path().join(p).exists(), "{p} 不該被碰");
        }
    }

    /// 左閉右開，和 `timeline` 同一套。
    ///
    /// 這是「一天一天往前忘」能不能安全連著按的前提：右邊界是閉的話，
    /// 每天的最後一毫秒會被刪兩次（無害）；左邊界是開的話，兩天中間會
    /// 留下一筆永遠刪不掉的紀錄——而使用者以為那兩天都清乾淨了。
    #[test]
    fn the_forget_range_is_left_closed_right_open_like_the_timeline() {
        let tmp = Tmp::new("forget-edges");
        let (db, _) = seeded(&tmp);
        let at = days_ago(60);

        assert_eq!(
            db.forget_preview(at - 1_000, at)
                .expect("preview")
                .frames_deleted,
            0,
            "右邊界是開的：到 `at` 為止不含 `at` 本身"
        );
        assert_eq!(
            db.forget_preview(at, at + 1)
                .expect("preview")
                .frames_deleted,
            1,
            "左邊界是閉的：從 `at` 開始含 `at`"
        );
    }

    /// 預告和實際必須一模一樣——和 prune 那條同一個理由。
    ///
    /// 這一條比 prune 那邊更要緊：他看到的數字是他按下不可逆按鈕的依據。
    #[test]
    fn the_preview_promises_exactly_what_forgetting_actually_removes() {
        let tmp = Tmp::new("forget-preview");
        let (mut db, _) = seeded(&tmp);
        let (from, to) = (days_ago(61), NOW);

        let preview = db.forget_preview(from, to).expect("preview");
        assert_eq!(db.stats().expect("stats").frames, 3, "預告不准動到東西");

        let actual = db.forget(from, to, Some(tmp.path())).expect("forget");
        // 整個 struct 一次比，不要一欄一欄挑——挑著比的測試看不到自己沒挑的。
        assert_eq!(preview, actual, "預告和實際必須一模一樣");
        assert!(!preview.is_empty(), "這個區間本來就該有東西");
    }
}
