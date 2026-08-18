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
    /// 刪掉的 PNG 檔數（frame 那一列還在，文字也還在）。
    pub images_deleted: u64,
    /// 上面那些檔案一共多大。
    pub image_bytes_freed: u64,
    /// 整列消失的 frame 數。
    pub frames_deleted: u64,
    /// 連帶消失的可搜尋文字段落數。
    pub chunks_deleted: u64,
    /// 連帶消失的 L1 事實數。
    pub facts_deleted: u64,
    /// focus / clipboard / input / system 四張表加總。
    pub events_deleted: u64,
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

/// 要刪掉的檔案，以及刪完之後的下場。
///
/// 拆成獨立函式是為了讓「檔案系統那一半」可以在沒有資料庫的情況下測到。
pub(crate) fn delete_files<'a>(
    root: &Path,
    rels: impl IntoIterator<Item = &'a str>,
    report: &mut PruneReport,
) {
    let mut dirs: BTreeSet<PathBuf> = BTreeSet::new();
    for rel in rels {
        let full = root.join(rel);
        match std::fs::remove_file(&full) {
            Ok(()) => {
                if let Some(p) = full.parent() {
                    dirs.insert(p.to_path_buf());
                }
            }
            // 檔案本來就不在 = 目標已達成。不算失敗，也不值得吵。
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => report.failed.push(format!("{}: {e}", full.display())),
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
    /// `image_root` 是畫面檔的根目錄；`None` 代表這次不碰檔案系統
    /// （text-only 模式，或呼叫端只想清資料庫）。注意 `None` 時仍然會把
    /// `image_path` 清成 NULL——否則資料庫會指向一堆再也不會被刪的檔案。
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
        let doomed: Vec<(String, i64)> = self
            .conn
            .prepare(
                "SELECT image_path, image_bytes FROM frames \
                 WHERE ts < ?1 AND image_path IS NOT NULL",
            )?
            .query_map([text_cut], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<std::result::Result<_, _>>()?;
        if let Some(root) = image_root {
            let before = report.failed.len();
            delete_files(root, doomed.iter().map(|(p, _)| p.as_str()), &mut report);
            report.images_deleted += (doomed.len() - (report.failed.len() - before)) as u64;
            // 這一段的位元組以前沒算進去，於是「刪掉 2 個畫面檔（100 B）」
            // 裡的檔案數和大小來自不同的兩段。一份自己對不起來的報告，
            // 比沒有報告更難用。
            report.image_bytes_freed += doomed.iter().map(|(_, b)| *b as u64).sum::<u64>();
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
        // ocr_blocks 由 frames 的 CASCADE 帶走
        report.frames_deleted += tx
            .execute("DELETE FROM frames WHERE ts < ?1", [text_cut])
            .context("prune frames")? as u64;
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

        // ── 第二段：只丟掉圖，留下字（過了 frames_days）──────────────
        let stale: Vec<(i64, String, i64)> = tx
            .prepare(
                "SELECT id, image_path, image_bytes FROM frames \
                 WHERE ts < ?1 AND image_path IS NOT NULL",
            )?
            .query_map([frame_cut], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<std::result::Result<_, _>>()?;
        tx.commit()?;

        if !stale.is_empty() {
            if let Some(root) = image_root {
                let before = report.failed.len();
                delete_files(root, stale.iter().map(|(_, p, _)| p.as_str()), &mut report);
                report.images_deleted += (stale.len() - (report.failed.len() - before)) as u64;
            }
            report.image_bytes_freed += stale.iter().map(|(_, _, b)| *b as u64).sum::<u64>();
            let tx = self.conn.transaction()?;
            {
                let mut up = tx.prepare(
                    "UPDATE frames SET image_path = NULL, image_bytes = 0 WHERE id = ?1",
                )?;
                for (id, _, _) in &stale {
                    up.execute([id])?;
                }
            }
            tx.commit()?;
        }

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

    #[test]
    fn zero_days_means_delete_now_not_keep_forever() {
        // 這條看起來像在測一行算術，實際上釘的是一個決定：`0` 有兩種
        // 讀法，而把「不要留」讀成「永遠留」會產生一個使用者永遠不會
        // 發現的隱私缺口。哪天有人「順手」改成 u32::MAX，這裡要紅。
        assert_eq!(cutoff(1_000_000, 0), 1_000_000);
        assert_eq!(cutoff(DAY_MS * 10, 3), DAY_MS * 7);
    }

    #[test]
    fn a_missing_file_is_success_not_failure() {
        let dir = Tmp::new("missing");
        let mut report = PruneReport::default();
        delete_files(dir.path(), ["2020/01/01/nope.png"], &mut report);
        assert!(report.failed.is_empty(), "{:?}", report.failed);
    }

    #[test]
    fn emptied_date_directories_go_away_but_the_root_stays() {
        let dir = Tmp::new("empty-dirs");
        let deep = dir.path().join("2026/08/18");
        std::fs::create_dir_all(&deep).expect("mkdir");
        std::fs::write(deep.join("a.png"), b"x").expect("write");

        let mut report = PruneReport::default();
        delete_files(dir.path(), ["2026/08/18/a.png"], &mut report);

        assert!(report.failed.is_empty());
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
        delete_files(dir.path(), ["2026/08/18/a.png"], &mut report);

        assert!(deep.join("b.png").exists(), "還沒到期的畫面不能被連坐刪掉");
    }
}
