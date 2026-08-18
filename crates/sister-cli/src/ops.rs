//! 各個子命令的實作。

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use sister_core::db::Db;

/// 兩次看螢幕之間的下限。設定檔可以寫得更慢，寫得更快則無效。
///
/// 定在這裡而不是各自 `.max(200)`，是因為 doctor 得印出**實際會用的值**：
/// 一個調了不生效的旋鈕，比一個沒有這個旋鈕更糟。
pub(crate) const MIN_TICK_MS: u64 = 200;

/// 開啟既有資料庫。查詢類命令不該憑空造一顆空的資料庫出來——
/// 那會讓「我明明錄了東西」變成「查無資料」的無聲錯誤。
fn open_existing(data_dir: &Path) -> Result<Db> {
    let path = crate::db_path(data_dir);
    anyhow::ensure!(
        path.exists(),
        "找不到資料庫：{}\n先跑 `sister replay <腳本>` 或 `sister record` 產生資料。",
        path.display()
    );
    Db::open(&path).with_context(|| format!("open {}", path.display()))
}

/// 把「排除 80」拆成「是誰擋的」。
///
/// 摘要上那個數字本身沒有錯，但它回答不了使用者唯一會問的問題。而排除
/// 恰恰是這個專案最容易安靜地過度生效的地方——規則寫寬了、UIA 一直答不出
/// 密碼欄狀態、某個 app 名稱剛好是別人的子字串，症狀全都長得一樣：
/// 她什麼都記不住，摘要上只有一個沒有解釋的數字。
///
/// 印出來的理由字串和寫進 `system_events` 的是同一串，所以看到什麼就能
/// 拿什麼去資料庫裡查。
/// 把「她有多少時間是閉著眼睛的」講出來。
///
/// 這個數字不講就是 bug。省電和停止工作在帳面上長得一模一樣：tick 照跑、
/// 沒有錯誤、CPU 很漂亮，而畫面上完全看不出來她其實有 87% 的時間沒看螢幕。
/// 那正是 alpha.4 那種「✓ 但什麼都沒產出」的失效形狀，只是這次是我們自己
/// 刻意造出來的——所以更該說。
fn report_idle(stats: &sister_capture::RecorderStats) {
    if stats.skipped_idle == 0 {
        return;
    }
    let pct = stats.skipped_idle as f64 / stats.ticks.max(1) as f64 * 100.0;
    println!(
        "  省下：{} 次沒碰螢幕（{pct:.0}%——那段時間你沒動鍵盤滑鼠；最多每 5 秒仍會看一次）",
        stats.skipped_idle
    );
}

fn report_exclusions(stats: &sister_capture::RecorderStats) {
    if stats.excluded_reasons.is_empty() {
        return;
    }
    // 擋最多的排前面：真正在吃掉一天的那條規則要第一個被看見
    let mut by_count: Vec<_> = stats.excluded_reasons.iter().collect();
    by_count.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    for (reason, n) in by_count {
        println!("        排除 {n} 次：{reason}");
    }
    if stats.kept == 0 && stats.excluded > 0 {
        println!(
            "  ⚠  這一整段沒有留下任何畫面，全部被上面的規則擋掉了。\
             如果那不是你要的，改 config 的 privacy 那一段。"
        );
    }
}

pub mod prune {
    use super::*;
    use sister_core::config::Config;
    use sister_core::retention::PruneReport;

    /// 畫面檔的根目錄。和 `record` 用的是同一個算式。
    pub fn frames_dir(data_dir: &Path) -> std::path::PathBuf {
        data_dir.join("frames")
    }

    pub fn run(data_dir: &Path, config: &Config, dry_run: bool) -> Result<()> {
        let r = &config.retention;
        println!(
            "保留期：畫面 {} 天、文字與事實 {} 天",
            r.frames_days, r.text_days
        );

        // 還沒錄過東西不是錯誤。`prune` 是維護動作，「沒有東西要清」是它
        // 成功的結果之一——而 `query` 查不到資料庫才該報錯，因為那代表
        // 使用者以為自己有資料。同一個 helper 套在兩種語意上會弄錯其中一個。
        let path = crate::db_path(data_dir);
        if !path.exists() {
            println!("  還沒有資料庫（{}），沒有東西可以清。", path.display());
            return Ok(());
        }
        let mut db = Db::open(&path).with_context(|| format!("open {}", path.display()))?;
        let now = sister_core::now_ms();

        if dry_run {
            let report = db.prune_preview(now, r)?;
            print_report(&report, true);
            return Ok(());
        }
        let report = db.prune(now, r, Some(&frames_dir(data_dir)))?;
        print_report(&report, false);
        Ok(())
    }

    /// 刪掉的東西要一項一項講出來。
    ///
    /// 「清理完成」這種話等於沒說：使用者沒辦法分辨「沒有東西過期」和
    /// 「清理其實沒生效」——而這兩件事在磁碟上長得一模一樣。
    pub fn print_report(r: &PruneReport, preview: bool) {
        let verb = if preview { "會刪掉" } else { "刪掉了" };
        if r.is_empty() {
            println!("  沒有東西過期，什麼都沒動。");
            return;
        }
        if r.images_deleted > 0 {
            println!(
                "  {verb} {} 個畫面檔（{}）",
                r.images_deleted,
                crate::fmt::bytes(r.image_bytes_freed as i64)
            );
        }
        if r.frames_deleted > 0 {
            println!(
                "  {verb} {} 列畫面紀錄、{} 段文字、{} 個事實、{} 筆事件",
                r.frames_deleted, r.chunks_deleted, r.facts_deleted, r.events_deleted
            );
        } else if r.chunks_deleted + r.facts_deleted + r.events_deleted > 0 {
            println!(
                "  {verb} {} 段文字、{} 個事實、{} 筆事件",
                r.chunks_deleted, r.facts_deleted, r.events_deleted
            );
        }
        // 刪不掉的檔案仍然躺在磁碟上，而使用者以為它已經不在了。
        // 這是整份報告裡唯一絕對不能安靜掉的一項。
        for f in &r.failed {
            println!("  ⚠  刪不掉，這個畫面還在磁碟上：{f}");
        }
    }
}

pub mod query {
    use super::*;
    use crate::fmt;
    use sister_core::db::FactRow;

    /// 每個答案：一個正規化後的值，加上它被看見過的所有位置。
    struct Answer {
        latest: FactRow,
        sightings: usize,
    }

    /// 螢幕上寫的是「客服**專線**」，使用者問的是「電話」——全文檢索永遠接不起
    /// 這兩個詞，但 L1 早就把那串數字標成 `phone` 了。這裡就是把使用者的說法
    /// 接到事實型別上，然後直接回答。純查表、零模型。
    fn answers(db: &Db, query: &str, limit: usize) -> Result<Vec<Answer>> {
        let mut rows = Vec::new();
        for kind in sister_core::facts::kinds_for_query(query) {
            rows.extend(db.facts_by_kind(kind.as_str(), limit * 4)?);
        }

        // 同一個號碼在三個畫面出現過，是同一個答案、三次目擊——不是三個答案。
        // 併成一筆並保留最近一次的出處，因為使用者要追的是「最後看到它的地方」。
        let mut order: Vec<String> = Vec::new();
        let mut merged: HashMap<String, Answer> = HashMap::new();
        for row in rows {
            match merged.get_mut(&row.normalized) {
                Some(a) => {
                    a.sightings += 1;
                    if row.ts > a.latest.ts {
                        a.latest = row;
                    }
                }
                None => {
                    order.push(row.normalized.clone());
                    merged.insert(
                        row.normalized.clone(),
                        Answer {
                            latest: row,
                            sightings: 1,
                        },
                    );
                }
            }
        }

        let mut out: Vec<Answer> = order
            .into_iter()
            .filter_map(|k| merged.remove(&k))
            .collect();
        out.sort_by_key(|a| std::cmp::Reverse(a.latest.ts));
        out.truncate(limit);
        Ok(out)
    }

    pub fn run(data_dir: &Path, text: &str, limit: usize, json: bool) -> Result<()> {
        anyhow::ensure!(
            !text.trim().is_empty(),
            "要查什麼？例如：sister query 客服電話"
        );
        let db = open_existing(data_dir)?;

        // 計時涵蓋兩條路徑：使用者感受到的是整個回答的延遲，不是單一次查詢。
        let started = std::time::Instant::now();
        let answers = answers(&db, text, limit)?;
        let hits = db.search(text, limit)?;
        let elapsed = started.elapsed();

        if json {
            let out = serde_json::json!({
                "query": text,
                "elapsed_ms": elapsed.as_secs_f64() * 1000.0,
                // 撈滿上限＝被切掉了。機器讀的那一份更要講：寫腳本的人
                // 看不到終端機上的那個「+」，會直接把長度當成總數。
                "limit": limit,
                "truncated": answers.len() >= limit || hits.len() >= limit,
                "answers": answers.iter().map(|a| serde_json::json!({
                    "kind": a.latest.kind, "value": a.latest.normalized, "raw": a.latest.raw,
                    "sightings": a.sightings, "ts": a.latest.ts,
                    "frame_id": a.latest.frame_id, "chunk_id": a.latest.chunk_id,
                    "app_id": a.latest.app_id, "window_title": a.latest.window_title,
                    "url": a.latest.url,
                })).collect::<Vec<_>>(),
                "hits": hits.iter().map(|h| serde_json::json!({
                    "chunk_id": h.chunk_id, "ts": h.ts, "source": h.source_kind.as_str(),
                    "frame_id": h.frame_id, "app_id": h.app_id, "window_title": h.window_title,
                    "url": h.url, "snippet": h.snippet, "score": h.score,
                })).collect::<Vec<_>>(),
            });
            println!("{}", serde_json::to_string_pretty(&out)?);
            return Ok(());
        }

        // `20 筆原文` 和「一共就這 20 筆」是兩件事，而畫面上長得一模一樣。
        // 撈滿上限就代表**被切掉了**，說出來使用者才知道還有第二頁。
        let more = |n: usize| if n >= limit { "+" } else { "" };
        println!(
            "🔍 「{text}」 {}{} 筆答案、{}{} 筆原文，{:.1} ms{}",
            answers.len(),
            more(answers.len()),
            hits.len(),
            more(hits.len()),
            elapsed.as_secs_f64() * 1000.0,
            if answers.len() >= limit || hits.len() >= limit {
                format!("（+ 代表撈滿 {limit} 筆就停了，用 --limit 看更多）")
            } else {
                String::new()
            }
        );

        if !answers.is_empty() {
            println!();
            for a in &answers {
                let f = &a.latest;
                let seen = if a.sightings > 1 {
                    format!("（看過 {} 次）", a.sightings)
                } else {
                    String::new()
                };
                println!(
                    "  ★ {}  「{}」{seen}",
                    f.normalized,
                    fmt::one_line(&f.raw, 40)
                );
                let mut src = format!(
                    "    ↳ {} · {} ({}) · {}",
                    f.kind,
                    fmt::timestamp(f.ts),
                    fmt::relative(f.ts),
                    fmt::context_line(f.app_id.as_deref(), f.window_title.as_deref())
                );
                if let Some(fid) = f.frame_id {
                    src.push_str(&format!(" · frame #{fid}"));
                }
                println!("{src}");
            }
        }

        if hits.is_empty() {
            if answers.is_empty() {
                println!("\n沒有找到。她可能當時沒在看，或那段被排除規則擋掉了。");
            }
            return Ok(());
        }
        println!();

        for (i, h) in hits.iter().enumerate() {
            println!(
                "{:>2}. {}  ({})",
                i + 1,
                fmt::timestamp(h.ts),
                fmt::relative(h.ts)
            );
            println!(
                "    {}",
                fmt::context_line(h.app_id.as_deref(), h.window_title.as_deref())
            );
            println!("    {}", fmt::one_line(&h.snippet, 120));
            // 出處：每一句話都要能被追回去
            let mut src = format!("↳ {}", h.source_kind.as_str());
            if let Some(fid) = h.frame_id {
                src.push_str(&format!(" · frame #{fid}"));
            }
            if let Some(url) = &h.url {
                src.push_str(&format!(" · {}", fmt::one_line(url, 70)));
            }
            println!("    {src}\n");
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use sister_core::model::{FocusSnapshot, FrameCapture, OcrBlock};

        fn frame(ts: i64, app: &str, text: &str) -> FrameCapture {
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
                    w: 100,
                    h: 20,
                    confidence: 0.9,
                }],
                focus: FocusSnapshot {
                    app_id: Some(app.into()),
                    app_name: Some(app.into()),
                    window_title: Some("t".into()),
                    url: None,
                    pid: None,
                    password_field: false,
                },
            }
        }

        /// 同一支號碼出現在三個畫面，是一個答案被看見三次，不是三個答案。
        fn seeded() -> Db {
            let mut db = Db::open_in_memory().unwrap();
            let sid = db.start_session("test", "0").unwrap();
            for (ts, app, text) in [
                (1_000, "chrome.exe", "客服專線 0800-080-123"),
                (2_000, "slack.exe", "打 0800-080-123 就好"),
                (3_000, "chrome.exe", "手機 0912-345-678，帳單 NT$13,450"),
            ] {
                db.insert_frame(sid, &frame(ts, app, text), None, 0)
                    .unwrap();
            }
            db
        }

        #[test]
        fn repeated_sightings_collapse_into_one_answer() {
            let db = seeded();
            let out = answers(&db, "電話", 10).unwrap();
            assert_eq!(out.len(), 2, "兩支不同號碼 → 兩個答案");

            let repeated = out
                .iter()
                .find(|a| a.latest.normalized == "+886800080123")
                .unwrap();
            assert_eq!(repeated.sightings, 2);
            // 出處要指向最後一次看到的地方，那才是使用者記得的場景
            assert_eq!(repeated.latest.app_id.as_deref(), Some("slack.exe"));
        }

        #[test]
        fn answers_are_newest_first() {
            let db = seeded();
            let out = answers(&db, "電話", 10).unwrap();
            assert!(out.windows(2).all(|w| w[0].latest.ts >= w[1].latest.ts));
        }

        /// 這是本專案存在的理由之一：畫面上寫「專線」，使用者問「電話」。
        /// 全文檢索接不起來，L1 型別可以。
        #[test]
        fn wording_the_screen_never_used_still_finds_the_number() {
            let db = seeded();
            assert!(
                db.search("電話", 10).unwrap().is_empty(),
                "螢幕上沒有「電話」二字"
            );
            assert!(!answers(&db, "電話", 10).unwrap().is_empty(), "但答得出來");
        }

        #[test]
        fn different_wording_selects_a_different_kind() {
            let db = seeded();
            let money = answers(&db, "帳單多少錢", 10).unwrap();
            assert_eq!(money.len(), 1);
            assert_eq!(money[0].latest.normalized, "TWD:13450");
        }

        /// 詞彙表認不出來時回空集合，不要亂猜一堆事實塞給使用者。
        #[test]
        fn unrecognised_wording_answers_nothing() {
            let db = seeded();
            assert!(answers(&db, "天氣如何", 10).unwrap().is_empty());
        }

        #[test]
        fn limit_is_respected() {
            let db = seeded();
            assert_eq!(answers(&db, "電話", 1).unwrap().len(), 1);
        }
    }
}

pub mod facts {
    use super::*;
    use crate::fmt;

    pub fn run(
        data_dir: &Path,
        kind: Option<&str>,
        search: Option<&str>,
        limit: usize,
        json: bool,
    ) -> Result<()> {
        let db = open_existing(data_dir)?;
        let rows = match search {
            Some(s) => db.facts_search(kind, s, limit)?,
            None => match kind {
                Some(k) => db.facts_by_kind(k, limit)?,
                None => db.facts_search(None, "", limit)?,
            },
        };

        if json {
            // 這裡以前直接印一個裸陣列。裸陣列講不出「後面還有」——
            // 拿到 200 筆的腳本只能假設一共就 200 筆。所以跟 query 一樣
            // 包一層信封，把上限和有沒有被切掉講明白。
            let out = serde_json::json!({
                "limit": limit,
                "truncated": rows.len() >= limit,
                "facts": rows.iter().map(|f| serde_json::json!({
                    "kind": f.kind, "raw": f.raw, "normalized": f.normalized,
                    "ts": f.ts, "source": f.source_kind,
                    "frame_id": f.frame_id, "app_id": f.app_id, "url": f.url,
                })).collect::<Vec<_>>(),
            });
            println!("{}", serde_json::to_string_pretty(&out)?);
            return Ok(());
        }

        if rows.is_empty() {
            println!("沒有符合的事實。");
            return Ok(());
        }
        // 撈滿上限就是被切掉了。`{n} 筆事實` 和「一共就這 n 筆」在畫面上
        // 長得一模一樣，而使用者會拿後者去下結論。
        let more = if rows.len() >= limit {
            format!("（撈滿 {limit} 筆就停了，用 --limit 看更多）")
        } else {
            String::new()
        };
        println!("{} 筆事實{more}\n", rows.len());
        for f in &rows {
            println!(
                "{:<10} {:<24} 「{}」",
                f.kind,
                fmt::one_line(&f.normalized, 24),
                fmt::one_line(&f.raw, 40)
            );
            println!(
                "           {}  {}",
                fmt::timestamp(f.ts),
                fmt::context_line(f.app_id.as_deref(), f.window_title.as_deref())
            );
        }
        Ok(())
    }
}

pub mod stats {
    use super::*;
    use crate::fmt;

    pub fn run(data_dir: &Path, json: bool) -> Result<()> {
        let db = open_existing(data_dir)?;
        let s = db.stats()?;

        let span_days = match (s.first_ts, s.last_ts) {
            (Some(a), Some(b)) if b > a => (b - a) as f64 / 86_400_000.0,
            _ => 0.0,
        };
        let disk_total = s.db_bytes + s.image_bytes;
        // 不到半天就不外推。`None` = 「還答不出來」，不是 0，也不是「就是這麼多」。
        //
        // 舊版在不到半天的時候把**累計總量**塞進這個欄位，然後照樣蓋一個 ✓
        // 上去：錄兩小時長了 200MB，它會印「每天約 200.0 MB ✓」，而真實速率
        // 是 2.4 GB/天——超標八倍卻長得像通過。隔壁的 `footprint.rs` 早就
        // 為同一件事寫了規則（不到 60 秒不外推）並且有測試守著，是這裡沒跟上。
        let per_day = (span_days >= 0.5).then(|| disk_total as f64 / span_days);
        let audit = db.exclusion_audit()?;
        let redaction = db.redaction_audit()?;

        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "frames": s.frames, "frames_collapsed": s.frames_collapsed,
                    "ocr_blocks": s.ocr_blocks, "chunks": s.chunks, "facts": s.facts,
                    "focus_events": s.focus_events, "clipboard_events": s.clipboard_events,
                    "input_windows": s.input_windows, "system_events": s.system_events,
                    "sessions": s.sessions, "db_bytes": s.db_bytes,
                    "image_bytes": s.image_bytes, "span_days": span_days,
                    "bytes_per_day": per_day,  // null = 資料還不夠久，不外推
                    "redaction": {
                        "flagged": redaction.flagged,
                        // > 0 就是遮蔽沒生效——那不是統計數字，是故障
                        "leaked": redaction.leaked,
                    },
                    "exclusions": audit.iter().map(|a| serde_json::json!({
                        "reason": a.reason, "episodes": a.episodes,
                        "first_ts": a.first_ts, "last_ts": a.last_ts,
                    })).collect::<Vec<_>>(),
                    // 這三個訊號在 Phase 0 沒有讀者，所以也沒有回歸保護：
                    // 哪天 recorder 不再寫 focus_events，`stats` 照樣印一個
                    // 很小的數字，沒有一個測試會紅。`doctor` 會講，但 doctor
                    // 要有人去跑——一個要有人記得去跑的檢查，遲早沒人跑。
                    "signals": db.signal_audit()?.iter().map(|a| serde_json::json!({
                        "name": a.name, "rows": a.rows,
                        "populated": a.populated, "broken": a.broken,
                    })).collect::<Vec<_>>(),
                }))?
            );
            return Ok(());
        }

        println!("📊 AI-Sister 足跡\n");
        if let (Some(a), Some(b)) = (s.first_ts, s.last_ts) {
            println!(
                "  期間      {} → {}  （{:.1} 天）",
                fmt::timestamp(a),
                fmt::timestamp(b),
                span_days
            );
        }
        println!("  工作階段  {}", s.sessions);
        println!(
            "  畫面      {} 張保留，{} 張因重複被折疊",
            s.frames, s.frames_collapsed
        );
        if s.frames + s.frames_collapsed > 0 {
            let ratio = s.frames_collapsed as f64 / (s.frames + s.frames_collapsed) as f64;
            println!("            去重擋掉了 {:.0}% 的畫面", ratio * 100.0);
        }
        println!(
            "  文字      {} 段（{} 個 OCR 區塊）",
            s.chunks, s.ocr_blocks
        );
        println!("  事實      {}", s.facts);
        println!(
            "  事件      焦點 {} · 剪貼簿 {} · 輸入 {} · 系統 {}",
            s.focus_events, s.clipboard_events, s.input_windows, s.system_events
        );
        println!();

        // 排除稽核。這一段的重點不是「有幾條規則」——那是設定檔，doctor 會念。
        // 這裡回答的是「它們到底生效過沒有」，而錄製結束之後只有資料庫答得出來。
        //
        // 數的是「段」：踏進 keepassxc 待十分鐘算一段。被擋掉的畫面**張數**
        // 沒有存進資料庫，所以這裡不講張數——講了就是把一個 2 說成 8000。
        if audit.is_empty() {
            println!("  排除      沒有任何一段擷取因為隱私規則被擋下來");
            println!("            （規則有沒有寫對是另一回事，跑 `sister doctor` 當場驗）");
        } else {
            let total: i64 = audit.iter().map(|a| a.episodes).sum();
            println!("  排除      隱私規則生效過 {total} 段（不是張數，張數沒存）");
            for a in &audit {
                let when = if a.first_ts == a.last_ts {
                    fmt::timestamp(a.first_ts)
                } else {
                    format!(
                        "{} → {}",
                        fmt::timestamp(a.first_ts),
                        fmt::timestamp(a.last_ts)
                    )
                };
                println!(
                    "            {:>4} 段  {}",
                    a.episodes,
                    fmt::one_line(&a.reason, 46)
                );
                println!("                    {when}");
            }
        }

        // 秘密遮蔽。問的不是「旗子插了幾次」，是「插了旗子的那幾列，字還在不在」。
        // 前者是我們寫入時的自我宣稱，後者是資料庫此刻的實際狀態。
        if redaction.flagged == 0 {
            println!("  遮蔽      沒有任何剪貼簿內容被判定為疑似秘密");
        } else {
            println!(
                "  遮蔽      {} 次剪貼簿內容被判定為疑似秘密",
                redaction.flagged
            );
            if redaction.leaked > 0 {
                println!(
                    "            ⚠ 其中 {} 次內容**仍然留在資料庫裡**——遮蔽沒生效",
                    redaction.leaked
                );
            } else {
                println!("            這幾列的 text 現在都是空的（當場查的，不是相信旗子）");
            }
        }
        println!();
        println!(
            "  資料庫    {}\n  畫面檔    {}",
            fmt::bytes(s.db_bytes),
            fmt::bytes(s.image_bytes)
        );
        // Phase 0 的退出條件之一：每天 < 300MB
        let budget = 300 * 1024 * 1024;
        match per_day {
            Some(p) => println!(
                "  每天約    {} {}  （Phase 0 預算 300 MB/天）",
                fmt::bytes(p as i64),
                if p as i64 <= budget { "✓" } else { "✗" }
            ),
            None => println!(
                "  每天約    還不知道（只錄了 {:.1} 小時，不到半天不外推）\
                 \n            目前一共 {}。要驗 Phase 0 的 300 MB/天，得先錄滿一天。",
                span_days * 24.0,
                fmt::bytes(disk_total)
            ),
        }
        Ok(())
    }
}

pub mod doctor {
    use super::*;
    use crate::fmt;
    use sister_core::config::Config;

    fn line(ok: bool, label: &str, detail: &str) {
        mark(if ok { "✓" } else { "✗" }, label, detail);
    }

    /// 第三種狀態：**還沒驗到**。
    ///
    /// ✓ 和 ✗ 都是斷言，而有些東西 doctor 當下沒辦法斷言——例如網址擷取，
    /// 前景不是瀏覽器的時候根本沒有東西可讀。把它畫成 ✓ 是說謊（那正是
    /// 上一版 `✓ OCR 語言` 的錯法），畫成 ✗ 則是製造一則使用者每次都會
    /// 看到、於是很快就學會忽略的假警報。所以給它自己的符號。
    fn mark(sym: &str, label: &str, detail: &str) {
        println!("  {sym} {label:<22} {detail}");
    }

    /// doctor 要用到的能力摘要。平台差異只收在這一個地方。
    ///
    /// 探測 OCR 要真的把引擎建起來，所以整份報告只問一次。
    #[derive(Default)]
    struct Caps {
        url: bool,
        /// 對現在的前景視窗真的問一次網址的結果。`None` = 本平台問不了。
        url_probe: Option<(&'static str, &'static str, String)>,
        /// 現在的前景視窗是誰：`(app, 標題)`。`None` = 本平台問不到，
        /// 兩個欄位各自可能是空字串（讀得到視窗但讀不到那一項）。
        ///
        /// `excluded_apps` / `excluded_titles` 比對的就是這兩個字串，所以
        /// 「規則有幾條」和「規則會不會生效」是兩件事：讀不到字串的話，
        /// 那些規則一條都不會命中——而數量照樣印得出來。
        focus_probe: Option<(String, String)>,
        /// 輸入 hook：`None` = 本平台沒有這個東西。
        /// 不是「沒試過」——doctor 現在會真的裝一次（見 [`caps`]）。
        input_hooks: Option<bool>,
        ocr: bool,
        ocr_language: Option<String>,
        ocr_available: Vec<String>,
        /// **實測**出來的檢查列：(過了沒, 標籤, 說明)。
        ///
        /// 刻意在這裡就判定完、只留下要印的字，是為了把平台專屬的型別
        /// （`WindowsOcr`、`RawFrame`…）全部關在 `caps()` 裡面。
        /// doctor 的輸出段落不該長出一堆 `#[cfg]`。
        ocr_probes: Vec<(bool, &'static str, String)>,
        /// 記了不該記的（排除規則失效）
        broken_privacy: Vec<String>,
        /// 其實什麼都沒記住，但你不會發現
        degraded: Vec<String>,
    }

    /// 把一段辨識結果縮成一行可以印的樣子。
    ///
    /// doctor 是使用者自己對著自己的螢幕跑的，所以印出畫面上的字並不是外洩；
    /// 但完整倒出來會把終端機洗掉，而且真正要回答的問題只有一個：
    /// 「讀到的是人話，還是亂碼」。一行就夠了。
    #[cfg(windows)]
    fn sample(lines: &[String]) -> String {
        const MAX: usize = 36;
        let Some(first) = lines.iter().find(|l| !l.trim().is_empty()) else {
            return String::new();
        };
        let mut s: String = first.chars().take(MAX).collect();
        if first.chars().count() > MAX {
            s.push('…');
        }
        format!("「{s}」")
    }

    #[cfg(windows)]
    fn caps(config: &Config) -> Caps {
        use sister_capture::traits::{Ocr, ScreenSource};
        use sister_capture::windows::input::{HookState, WindowsInput};
        use sister_capture::windows::{Capabilities, ocr::WindowsOcr, screen::WindowsScreen};

        // 不宣稱，直接裝一次。以前 doctor 永遠印「輸入 hook 沒裝上」，
        // 因為它根本沒去裝——那是一則恆真的假警報，而假警報會連坐旁邊
        // 那則真的警告一起被忽略。hook 只數次數不看內容，裝一次很便宜。
        // doctor 只想知道 hook 裝不裝得上，聚合視窗多長無關緊要
        let _ = WindowsInput::start(sister_core::now_ms(), config.capture.input_window_secs);
        let input_hooks = match WindowsInput::state() {
            HookState::Active => Some(true),
            // NotStarted 在上一行之後不可能發生；真發生了也是「沒裝上」
            HookState::Failed | HookState::NotStarted => Some(false),
        };

        let c = Capabilities::current(config);
        let mut probes = Vec::new();
        let url_probe;
        let focus_probe;

        // UIA：一樣不宣稱。真的對現在的前景視窗問一次網址。
        // `✓ UIA 建得起來` 這句話的價值是零——使用者要知道的是
        // 「我的網銀規則現在到底會不會生效」。
        {
            use sister_capture::traits::FocusSource;
            let mut source = sister_capture::windows::focus::WindowsFocus::new();
            let snapshot = source.snapshot(sister_core::now_ms()).unwrap_or_default();
            let app = snapshot.app_key();
            // 排除規則比對的就是這兩個字串。讀得到才代表那些規則跑得動。
            focus_probe = Some((
                app.clone(),
                snapshot.window_title.clone().unwrap_or_default(),
            ));
            url_probe = Some(match (&snapshot.url, source.url_capture_alive()) {
                (Some(url), _) => (
                    "✓",
                    "讀你現在的網址",
                    format!("{app} → {}", crate::fmt::one_line(url, 60)),
                ),
                (None, false) => (
                    "✗",
                    "讀你現在的網址",
                    "UIA 卡住太多次，已經放棄——excluded_urls 這一整組規則不生效".to_string(),
                ),
                // 以下兩種都**不是失敗**，是「這一刻沒東西可測」。畫成 ✗ 的話，
                // 從終端機跑 doctor 永遠會看到它（前景就是那個終端機），
                // 於是它變成一則恆真的警告——那種東西會把整個警告區塊一起
                // 教壞。但也絕不能畫成 ✓：那會讓使用者以為驗過了。
                (None, true) if app.is_empty() => (
                    "?",
                    "讀你現在的網址",
                    "現在沒有前景視窗，這一刻測不出來".to_string(),
                ),
                (None, true) => (
                    "?",
                    "讀你現在的網址",
                    format!(
                        "前景是 {app}，不是瀏覽器（或位址列是空的）。\
                         把瀏覽器切到前景再跑一次 doctor 才驗得到"
                    ),
                ),
            });
        }

        if c.ocr && config.capture.ocr {
            let mut ocr = WindowsOcr::new(&config.capture.ocr_languages);

            // 第一關：引擎讀不讀得到字。圖是編進執行檔的，答案是已知的。
            match ocr.self_test() {
                Ok(lines) => {
                    let text = lines.join(" ");
                    let missing: Vec<_> = WindowsOcr::SELF_TEST_EXPECTS
                        .iter()
                        .filter(|w| !text.contains(**w))
                        .collect();
                    probes.push(if lines.is_empty() {
                        (
                            false,
                            "內建圖自我測試",
                            "一行都沒讀到——引擎建得起來，但它讀不出字".to_string(),
                        )
                    } else if missing.is_empty() {
                        (
                            true,
                            "內建圖自我測試",
                            format!("{} 行，內容正確 {}", lines.len(), sample(&lines)),
                        )
                    } else {
                        (
                            false,
                            "內建圖自我測試",
                            format!(
                                "讀到 {} 行，但少了 {:?}——讀得到字，讀錯了。實際：{}",
                                lines.len(),
                                missing,
                                sample(&lines)
                            ),
                        )
                    });
                }
                Err(e) => probes.push((false, "內建圖自我測試", format!("失敗：{e:#}"))),
            }

            // 第二關：你**現在這台螢幕**讀不讀得到。跟錄製走同一條路
            // （同一顆引擎、同一個原生解析度的抓圖），所以「內建圖過了但這關
            // 沒過」就直接指向畫面本身，而不是引擎或語言包。
            let mut screen = WindowsScreen::new();
            let grabbed = screen.grab(sister_core::now_ms());
            let grabbed_edge = match &grabbed {
                Ok(Some(f)) => Some(f.width.max(f.height)),
                _ => None,
            };
            let probe = match grabbed {
                Err(e) => (false, "讀你現在的螢幕", format!("抓不到畫面：{e:#}")),
                Ok(None) => (
                    false,
                    "讀你現在的螢幕",
                    "抓不到畫面（工作站鎖定時本來就不抓）".to_string(),
                ),
                Ok(Some(frame)) => {
                    let (w, h) = (frame.width, frame.height);
                    // 「讀不出字」和「圖上本來就沒字」在報告裡長得一模一樣。
                    // 亮度範圍把它們分開：全黑的擷取 lo == hi。
                    let contrast = match frame.luma_span() {
                        Some((lo, hi)) if lo == hi => {
                            format!(
                                "；而且整張圖只有一個顏色（亮度 {lo}），這是一次失敗的擷取，不是一張沒有字的畫面"
                            )
                        }
                        Some((lo, hi)) => format!("；畫面亮度 {lo}–{hi}"),
                        None => String::new(),
                    };
                    match ocr.recognize(&frame) {
                        Ok(lines) if lines.is_empty() => (
                            false,
                            "讀你現在的螢幕",
                            format!(
                                "{w}×{h} → 0 行。錄製會照跑、畫面會留下，\
                                 但搜尋永遠是空的{contrast}"
                            ),
                        ),
                        Ok(lines) => {
                            let texts: Vec<String> = lines.into_iter().map(|b| b.text).collect();
                            (
                                true,
                                "讀你現在的螢幕",
                                format!("{w}×{h} → {} 行 {}", texts.len(), sample(&texts)),
                            )
                        }
                        Err(e) => (
                            false,
                            "讀你現在的螢幕",
                            format!("{w}×{h} 辨識失敗：{e:#}{contrast}"),
                        ),
                    }
                }
            };
            probes.push(probe);

            // 只在真的卡到的時候講。平常這是一個沒有人需要知道的數字。
            //
            // 比的是**剛剛真的抓到的那張圖**，不是設定檔裡的數字。OCR 吃的是
            // 原生解析度的像素，`max_long_edge` 只管存檔——拿它來比等於比錯
            // 對象，而且會比出一條永遠不會成立的規則：一條讀起來很對、卻
            // 一輩子命中不了任何東西的檢查，正是這個專案在獵的那種 bug。
            let limit = ocr.max_dimension();
            if let Some(edge) = grabbed_edge
                && edge > limit
            {
                probes.push((
                    false,
                    "影像尺寸上限",
                    format!("剛剛抓到的畫面長邊 {edge} 超過引擎上限 {limit}：每一張畫面都會被拒絕"),
                ));
            }
        }

        Caps {
            url: c.url,
            url_probe,
            focus_probe,
            input_hooks,
            ocr: c.ocr,
            ocr_language: c.ocr_language.clone(),
            ocr_available: c.ocr_languages_available.clone(),
            ocr_probes: probes,
            broken_privacy: c.broken_privacy_rules(config),
            degraded: c.silently_degraded(config),
        }
    }

    #[cfg(not(windows))]
    fn caps(config: &Config) -> Caps {
        let _ = config;
        Caps::default()
    }

    pub fn run(data_dir: &Path, config: &Config, config_path: Option<PathBuf>) -> Result<()> {
        println!("🩺 AI-Sister 環境檢查\n");
        let caps = caps(config);

        println!("環境");
        line(
            true,
            "版本",
            &format!("sister {}", env!("CARGO_PKG_VERSION")),
        );
        line(true, "平台", std::env::consts::OS);

        let backend = crate::ops::record::backend_name();
        line(
            backend.is_some(),
            "擷取後端",
            backend.unwrap_or("無（此平台尚未實作；可用 sister replay 驗證管線）"),
        );

        println!("\n位置");
        let db_file = crate::db_path(data_dir);
        line(
            data_dir.exists(),
            "資料目錄",
            &format!(
                "{}{}",
                data_dir.display(),
                if data_dir.exists() {
                    ""
                } else {
                    "（尚未建立）"
                }
            ),
        );
        line(db_file.exists(), "資料庫", &db_file.display().to_string());
        // `--config` 指到哪就印哪。舊版一律印 `default_path()`，於是一個
        // 用 `--config` 跑的人會看到一個他沒在用的路徑，後面還接一句
        // 「用預設值」——而底下的保留期天數明明是他自訂的那一份。
        // doctor 是出事時唯一的稽核面，這一行不能指錯地方。
        match config_path.or_else(Config::default_path) {
            Some(p) => line(
                true,
                "設定檔",
                &format!(
                    "{}{}",
                    p.display(),
                    if p.exists() {
                        ""
                    } else {
                        "（不存在，用預設值）"
                    }
                ),
            ),
            None => line(false, "設定檔", "無法決定路徑"),
        }

        println!("\n儲存");
        let probe = Db::open_in_memory().context("open probe database")?;
        line(true, "SQLite", &probe.sqlite_version());

        // 開著不關：底下「保留期」那一段還要拿它去問「現在有多少已過期」。
        let db = db_file.exists().then(|| Db::open(&db_file)).transpose()?;

        // **問使用者那一顆，不是問一顆現做的。** 舊版查的是上面那個
        // in-memory probe，於是它證明的是「這份程式碼建得出索引」，
        // 而不是「你的資料庫裡有索引」。索引掉了的話 `db.rs` 會安靜地
        // 退回 LIKE 全表掃描——答案還是對的，只是一年份的資料要掃到天亮，
        // 而 doctor 會說 ✓。
        let fts_of = |d: &Db| {
            d.conn()
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE name IN ('text_fts','text_fts_uni')",
                    [],
                    |r| r.get::<_, i64>(0),
                )
                .unwrap_or(0)
        };
        match &db {
            Some(d) => {
                let n = fts_of(d);
                line(
                    n == 2,
                    "FTS5 雙索引",
                    &if n == 2 {
                        "trigram + unicode61（你的資料庫裡）".to_string()
                    } else {
                        format!("你的資料庫裡只有 {n}/2 個——搜尋會退回全表掃描，資料還在但會很慢")
                    },
                );
            }
            // 還沒有資料庫時，能證明的只有「這台機器的 SQLite 支援 FTS5」
            None => mark(
                "?",
                "FTS5 雙索引",
                &format!(
                    "還沒有資料庫；這份程式碼建得出來（{}/2），等你錄過再驗一次",
                    fts_of(&probe)
                ),
            ),
        }

        // 兩個字的中文詞沒有索引可用。與其在文件裡宣稱「只找得回 30 天」，
        // 不如當場告訴他**他自己的資料庫**有多少行字已經在那條線外面。
        let days = Db::like_scan_days();
        match &db {
            Some(d) => {
                let (outside, total) = d.text_outside_scan_window()?;
                if total == 0 {
                    mark("?", "兩個字的中文", "資料庫是空的，等你錄過再驗一次");
                } else if outside == 0 {
                    mark(
                        "✓",
                        "兩個字的中文",
                        &format!("{total} 行字全都在 {days} 天內，現在查得到全部"),
                    );
                } else {
                    let pct = outside as f64 / total as f64 * 100.0;
                    mark(
                        "!",
                        "兩個字的中文",
                        &format!(
                            "{outside}/{total} 行字（{pct:.0}%）比 {days} 天更舊——\
                             「帳單」「電話」這種兩個字的詞，在「原文」那一半裡搜不到那些。\
                             三個字以上不受影響，L1 抽出來的事實也不受影響",
                        ),
                    );
                }
            }
            None => mark("?", "兩個字的中文", "還沒有資料庫"),
        }
        if let Some(db) = &db {
            // 兩個數字並排印出來、讓讀者自己比對，是把判斷丟給人。
            // 對不上的時候（別的版本開過這顆資料庫）程式照樣會去查它，
            // 那正是需要一個 ✗ 的時候。
            let have = db.schema_version()?;
            let want = sister_core::db::SCHEMA_VERSION;
            line(
                have == want,
                "Schema 版本",
                &if have == want {
                    format!("{have}")
                } else {
                    format!("{have}，但這支程式是 {want}——可能是別的版本開過這顆資料庫")
                },
            );
            let s = db.stats()?;
            // 「0 張畫面 · 0 段文字 ✓」是這個專案一路在修的那個災難本身
            // ——錄了一整天、資料庫在長大、一個字都沒進去。有資料庫卻沒
            // 內容不該是打勾。
            let detail = format!(
                "{} 張畫面 · {} 段文字 · {}",
                s.frames,
                s.chunks,
                fmt::bytes(s.db_bytes + s.image_bytes)
            );
            match (s.frames, s.chunks) {
                (0, 0) => mark("?", "已記錄", &format!("{detail}（還沒有任何內容）")),
                (_, 0) => mark(
                    "✗",
                    "已記錄",
                    &format!("{detail}——有畫面卻一個字都沒有，OCR 沒讀到東西"),
                ),
                _ => line(true, "已記錄", &detail),
            }

            // Phase 0 的退場條件是「連續 7 天自我錄製、零當機」，而在這
            // 之前那句話的驗證方式是使用者自己記得有沒有當過。資料庫一直
            // 知道答案：Ctrl-C 走的是正常收尾，所以沒有 `ended_at` 的那幾
            // 段，剩下的解釋只有被殺、當機、關機、拔電。
            let (all_sessions, unfinished, last_crash) = db.crash_audit()?;
            match (all_sessions, unfinished) {
                (0, _) => mark("?", "零當機", "還沒錄過"),
                (n, 0) => line(true, "零當機", &format!("{n} 段錄製全部正常收尾")),
                (n, u) => mark(
                    // 不畫 ✗。此刻另一個終端機正在錄的話，那一段也沒有
                    // `ended_at`，長得跟當機一模一樣——而我沒有一條不會
                    // 因為 PID 重用而說謊的路可以分辨。不知道就說不知道，
                    // 不要為了讓輸出好看而猜一個。
                    "!",
                    "零當機",
                    &format!(
                        "{n} 段錄製裡有 {u} 段沒有正常收尾{}——當機、關機、拔電，\
                         或者現在正有另一個 sister 在錄",
                        last_crash
                            .map(|t| format!("（最後一次 {}）", fmt::timestamp(t)))
                            .unwrap_or_default()
                    ),
                ),
            }

            // 上面那一列擋的是「一個字都沒有」。它擋不住的是**有一堆列、
            // 每一列都是空殼**——`COUNT(*)` 對這兩種故障的回答一模一樣。
            //
            // 這三個訊號（焦點、輸入節奏、文字座標）在 Phase 0 沒有任何讀者，
            // 它們是 Phase 1 之後才要用的原料。沒人讀不是問題，**沒人讀所以
            // 沒人驗**才是：真的壞掉的那一天，這裡是唯一會講話的地方。
            for a in db.signal_audit()? {
                if a.rows == 0 {
                    // 沒有列 ≠ 壞掉。可能只是這台機器沒有那個能力（replay
                    // 讀不到 pid），或還沒錄到。不知道就說不知道。
                    mark("?", a.name, "還沒有資料");
                } else if a.broken {
                    mark(
                        "✗",
                        a.name,
                        &format!("{} 列，但沒有一列有內容——{}", a.rows, a.note),
                    );
                } else {
                    line(
                        true,
                        a.name,
                        &format!("{} 列，{} {}", a.rows, a.populated, a.populated_label),
                    );
                }
            }
        }

        println!("\n隱私");
        // 「9 條規則 ✓」是 THREAT_MODEL 明文禁止的那種寫法：規則的**數量**
        // 從來不是問題，規則**會不會命中**才是。這些規則比對的是前景 app
        // 名稱，所以讀不到名稱的時候它們一條都不生效——而數量照樣是 9。
        let (sym, note) = match &caps.focus_probe {
            Some((app, _)) if !app.is_empty() => ("✓", format!("，現在讀到的是 {app}")),
            Some(_) => ("?", "，但現在沒有前景視窗，這一刻測不出來".to_string()),
            None => (
                "✗",
                "（本平台讀不到前景 app，這些規則目前不生效）".to_string(),
            ),
        };
        mark(
            sym,
            "排除的 app",
            &format!("{} 條規則{note}", config.privacy.excluded_apps.len()),
        );
        // 規則數量不等於規則有效。沒有 URL 擷取能力時這些規則一條都不會跑，
        // 而使用者看到「16 條規則 ✓」只會更放心——那正是最糟的結果。
        //
        // 但「UIA 建得起來」也不夠格畫 ✓：那只證明了一個 COM 物件生得出來，
        // 沒有證明我們從位址列上讀得到任何東西（Firefox 的樹長得就不一樣）。
        // 這就是上一版 `✓ OCR 語言 zh-Hant-TW` 的錯法，只是換了個能力。
        // 所以 ✓ 只給「下面那一列真的讀到了網址」的情況。
        let demonstrated = caps.url_probe.as_ref().is_some_and(|(s, ..)| *s == "✓");
        let (sym, note) = match (caps.url, demonstrated) {
            (false, _) => ("✗", "（本平台無法讀取網址，這些規則目前不生效）"),
            (true, true) => ("✓", ""),
            // UIA 在，但這一刻沒能證明讀得到。可能只是前景不是瀏覽器。
            (true, false) => ("?", "（還沒驗到——見下面那一列）"),
        };
        mark(
            sym,
            "排除的網址",
            &format!("{} 條規則{note}", config.privacy.excluded_urls.len()),
        );
        // 規則寫錯的方式是安靜的：少擋了不會有任何症狀。這裡把「寫了也不會
        // 命中」的規則挑出來，因為使用者自己永遠不會發現。
        let suspicious = sister_core::config::suspicious_url_rules(&config.privacy.excluded_urls);
        if !suspicious.is_empty() {
            for (rule, why) in &suspicious {
                line(false, "  規則不會命中", &format!("{rule} — {why}"));
            }
        }
        // 規則數量與規則寫法都對，還有第三個問題：**網址到底讀不讀得到**。
        // 這一列是真的去問了現在的前景視窗，不是宣稱 UIA 建得起來。
        if let Some((sym, label, detail)) = &caps.url_probe {
            mark(sym, label, detail);
        }
        // 標題和 app 來自同一次 snapshot，但**失敗方式不一樣**：有些視窗
        // 讀得到 exe 名稱卻沒有標題。分開報，才不會讓 app 的 ✓ 幫標題背書。
        let (sym, note) = match &caps.focus_probe {
            Some((_, title)) if !title.is_empty() => (
                "✓",
                format!("，現在讀到的是「{}」", crate::fmt::one_line(title, 40)),
            ),
            Some(_) => ("?", "，但現在這個視窗沒有標題可比對".to_string()),
            None => (
                "✗",
                "（本平台讀不到視窗標題，這些規則目前不生效）".to_string(),
            ),
        };
        mark(
            sym,
            "排除的標題",
            &format!("{} 條規則{note}", config.privacy.excluded_titles.len()),
        );
        line(
            config.privacy.pause_on_screenshare,
            "螢幕分享時暫停",
            if config.privacy.pause_on_screenshare {
                "開啟"
            } else {
                "關閉（旁人的畫面會被錄到）"
            },
        );
        line(
            config.privacy.redact_clipboard_secrets,
            "剪貼簿秘密偵測",
            if config.privacy.redact_clipboard_secrets {
                "開啟"
            } else {
                "關閉（API key 會落地）"
            },
        );
        line(
            config.capture.store_images,
            "保留畫面檔",
            if config.capture.store_images {
                "是"
            } else {
                "否（text-only 模式）"
            },
        );

        // OCR 語言決定她讀不讀得懂你的螢幕，所以要把**實際挑中的**那個印出來，
        // 不是印設定檔裡的偏好清單——兩者不一致正是問題所在。
        println!("\n讀字");
        if !config.capture.ocr {
            line(false, "OCR", "已關閉（畫面會留下，但上面的字不會進資料庫）");
        } else {
            line(
                caps.ocr,
                "OCR 語言",
                &match &caps.ocr_language {
                    Some(tag) => format!("{tag}（實際使用）"),
                    None => "無：這台機器沒有安裝任何 OCR 語言".to_string(),
                },
            );
            line(
                !caps.ocr_available.is_empty(),
                "已安裝的語言",
                &if caps.ocr_available.is_empty() {
                    "（無）".to_string()
                } else {
                    caps.ocr_available.join("、")
                },
            );
            // 上面兩行講的都是「引擎建得起來」。下面這些是真的去讀了。
            // 這個分野是實測換來的：語言 ✓、錄了一分鐘、資料庫裡零個字。
            for (ok, label, detail) in &caps.ocr_probes {
                line(*ok, label, detail);
            }
        }

        println!("\n節奏");
        // 設定檔寫 50，程式跑 200。以前 doctor 只是不提這件事，於是使用者
        // 調了一個不會生效的旋鈕，而畫面上沒有任何地方看得出來。
        let asked = config.capture.min_interval_ms;
        let used = asked.max(crate::ops::MIN_TICK_MS);
        line(
            true,
            "多久看一次螢幕",
            &if used == asked {
                format!("{used} ms")
            } else {
                format!(
                    "{used} ms（你設的是 {asked} ms，但下限是 {} ms）",
                    crate::ops::MIN_TICK_MS
                )
            },
        );
        if let Some(ok) = caps.input_hooks {
            line(
                ok,
                "輸入 hook",
                if ok {
                    "裝得上（doctor 剛剛真的裝了一次；只數次數，不看按了什麼）"
                } else {
                    "裝不上：打字節奏這一路訊號會是空的"
                },
            );
        }

        if !caps.broken_privacy.is_empty() {
            println!("\n⚠ 目前失效的隱私保護");
            for w in &caps.broken_privacy {
                println!("  ✗ {w}");
            }
        }

        // 「她會不會記錯」和「她到底有沒有記住」是兩回事，分開報。
        if !caps.degraded.is_empty() {
            println!("\n⚠ 看起來正常，但其實記不住東西");
            for w in &caps.degraded {
                println!("  ✗ {w}");
            }
        }

        // 先講「她會存多少圖」，再講「存下來的留多久」。順序是刻意的：
        // 使用者遲早會點到一筆沒有圖的搜尋結果，而那不是壞掉——與其等他
        // 來問，不如在他按下 record 之前就講清楚。
        println!("\n畫面檔");
        // `store_images = false` 的時候，下面兩條設定一條都不會跑。照樣印
        // 「最快 5 秒一張 ✓」等於在回答一個沒有人問的問題——而且答錯了。
        if config.capture.store_images {
            line(
                true,
                "多久存一張",
                &format!(
                    "最快 {:.0} 秒一張（其餘只留字，搜尋不受影響）",
                    config.capture.image_min_interval_ms as f64 / 1000.0
                ),
            );
            line(
                true,
                "一天最多存",
                &if config.capture.max_image_mb_per_day == 0 {
                    "不設上限——磁碟要自己盯".to_string()
                } else {
                    format!(
                        "{} MB（用完就只留字，隔天歸零）",
                        config.capture.max_image_mb_per_day
                    )
                },
            );
        } else {
            mark(
                "—",
                "一張都不存",
                "store_images = false：只留字。下面的保留期只管得到文字",
            );
        }

        println!("\n保留期");
        line(
            true,
            "畫面",
            &format!("{} 天（到期只刪圖，字留著）", config.retention.frames_days),
        );
        line(
            true,
            "文字與事實",
            &format!("{} 天（到期整列消失）", config.retention.text_days),
        );
        // 這兩個數字以前是**純粹的宣稱**——設定檔裡寫著 30 天，而沒有任何
        // 一行程式碼會刪掉任何東西。同樣不宣稱、直接示範：現在就去問資料庫
        // 「這一刻有多少東西已經過期」。`?` 代表還沒有資料庫可以問。
        match db.as_ref().map(|d| {
            d.prune_preview(sister_core::now_ms(), &config.retention)
                .map(|r| (r.images_deleted, r.frames_deleted))
        }) {
            Some(Ok((0, 0))) => line(true, "現在有多少已過期", "沒有——都還在保留期內"),
            Some(Ok((imgs, rows))) => mark(
                "?",
                "現在有多少已過期",
                &format!("{imgs} 個畫面檔、{rows} 列紀錄。跑 `sister prune` 讓它們消失"),
            ),
            Some(Err(e)) => line(false, "現在有多少已過期", &format!("問不出來：{e:#}")),
            None => mark("?", "現在有多少已過期", "還沒有資料庫"),
        }
        Ok(())
    }
}

pub mod replay {
    use super::*;
    use sister_capture::{Recorder, ReplayBackend, Scenario, Tick};
    use sister_core::config::Config;

    pub fn run(
        data_dir: &Path,
        config: Config,
        scenario_path: &Path,
        interval_ms: i64,
        dry_run: bool,
        days_ago: f64,
        start: Option<i64>,
    ) -> Result<()> {
        anyhow::ensure!(interval_ms > 0, "--interval-ms must be positive");
        let scenario = Scenario::load(scenario_path)?;
        let duration = scenario.duration_ms();
        // 腳本寫的是相對時間；落地前換算成真實時間，否則整段記憶會停在 1970 年
        let origin = start
            .unwrap_or_else(|| sister_core::now_ms() - duration - (days_ago * 86_400_000.0) as i64);
        println!(
            "▶ 重播「{}」：{} 步、{:.1} 秒，每 {} ms 一次 tick{}",
            scenario.name,
            scenario.steps.len(),
            duration as f64 / 1000.0,
            interval_ms,
            if dry_run {
                "（dry-run，不寫入）"
            } else {
                ""
            }
        );
        println!("  時間軸起點：{}", crate::fmt::timestamp(origin));

        let db = if dry_run {
            Db::open_in_memory()?
        } else {
            let path = crate::db_path(data_dir);
            Db::open(&path).with_context(|| format!("open {}", path.display()))?
        };
        let image_dir = if dry_run {
            None
        } else {
            Some(sister_capture::frames::frames_root(data_dir))
        };

        let backend = ReplayBackend::with_origin(scenario, origin);
        let mut rec = Recorder::new(backend, db, config, image_dir)?;

        let mut offset = 0i64;
        while offset <= duration {
            let ts = origin + offset;
            match rec.tick(ts)? {
                Tick::Kept {
                    frame_id,
                    ocr_blocks,
                    facts,
                } => {
                    println!(
                        "  {offset:>7} ms  保留 frame #{frame_id}（{ocr_blocks} 段文字、{facts} 個事實）"
                    );
                }
                Tick::Excluded { reason } => println!("  {offset:>7} ms  排除：{reason}"),
                Tick::NoScreen => println!("  {offset:>7} ms  沒有畫面"),
                Tick::Duplicate { .. } | Tick::Disabled | Tick::Idle => {}
            }
            offset += interval_ms;
        }
        rec.finish()?;

        let s = rec.stats();
        println!(
            "\n完成：{} tick → 保留 {}、重複 {}、排除 {}、無畫面 {}",
            s.ticks, s.kept, s.duplicates, s.excluded, s.no_screen
        );
        report_idle(s);
        report_exclusions(s);
        if s.secrets_redacted > 0 {
            println!("  偵測到 {} 次疑似秘密，內容未落地。", s.secrets_redacted);
        }
        Ok(())
    }
}

pub mod record {
    use super::*;
    use sister_core::config::Config;

    /// 這台機器上可用的擷取後端名稱。
    pub fn backend_name() -> Option<&'static str> {
        #[cfg(windows)]
        {
            Some("windows-gdi")
        }
        #[cfg(not(windows))]
        {
            None
        }
    }

    pub fn run(data_dir: &Path, config: Config, duration: Option<u64>) -> Result<()> {
        #[cfg(not(windows))]
        {
            let _ = (data_dir, config, duration);
            anyhow::bail!(
                "這個平台（{}）還沒有擷取後端。\n\n\
                 Phase 0 的目標平台是 Windows；核心與錄製迴圈本身是平台無關的，\n\
                 可以用腳本完整驗證：\n\n    \
                 sister replay scenarios/bill-lookup.json\n",
                std::env::consts::OS
            )
        }
        #[cfg(windows)]
        {
            windows_record(data_dir, config, duration)
        }
    }

    /// Ctrl-C 只設一個旗標，真正的收尾留給主迴圈。
    ///
    /// console handler 跑在另一條執行緒上，在那裡碰資料庫等於在 SQLite
    /// 交易中間插隊。設一個 bool 是這裡唯一安全的動作。
    #[cfg(windows)]
    static STOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    #[cfg(windows)]
    fn install_ctrl_c_handler() {
        use windows::Win32::System::Console::{
            CTRL_BREAK_EVENT, CTRL_C_EVENT, CTRL_CLOSE_EVENT, SetConsoleCtrlHandler,
        };
        use windows::core::BOOL;

        unsafe extern "system" fn handler(ctrl_type: u32) -> BOOL {
            match ctrl_type {
                CTRL_C_EVENT | CTRL_BREAK_EVENT | CTRL_CLOSE_EVENT => {
                    STOP.store(true, std::sync::atomic::Ordering::SeqCst);
                    true.into()
                }
                _ => false.into(),
            }
        }
        let _ = unsafe { SetConsoleCtrlHandler(Some(handler), true) };
    }

    #[cfg(windows)]
    fn windows_record(data_dir: &Path, config: Config, duration: Option<u64>) -> Result<()> {
        use sister_capture::windows::{self, Capabilities};
        use sister_capture::{Recorder, Tick};
        use std::sync::atomic::Ordering;
        use std::time::{Duration, Instant};

        std::fs::create_dir_all(data_dir)
            .with_context(|| format!("create {}", data_dir.display()))?;

        let mut db = Db::open(&crate::db_path(data_dir))?;
        // **先建後端、再問能力。** 反過來的話，「輸入 hook 裝上了沒」永遠
        // 是在 hook 還沒裝之前問的，於是永遠回報失敗——一則恆假的警告。
        let backend = windows::backend(&config)?;

        // 缺席的能力會讓某些排除規則整組失效，或讓她其實什麼都沒記住。
        // 這兩件事都要在開始錄之前講，不是藏在 doctor 裡等使用者自己去發現。
        let caps = Capabilities::current(&config);
        for warning in caps.broken_privacy_rules(&config) {
            println!("⚠  {warning}");
        }
        for warning in caps.silently_degraded(&config) {
            println!("⚠  {warning}");
        }
        // 總開關關著的時候，每個 tick 都直接回 `Disabled`，而摘要的四個
        // 欄位剛好全部是 0——和「錄得好好的、只是螢幕沒變」長得一模一樣。
        // 一個打字打錯、或一個沒關回來的暫停，就能讓她整天什麼都沒記，
        // 而且沒有任何一行字提過。
        if !config.capture.enabled {
            println!(
                "⚠  capture.enabled = false：接下來每一個 tick 都會直接跳過，\
                 什麼都不會記錄。改成 true 才會真的開始錄。"
            );
        }
        // doctor 會挑出「寫了也不會命中」的網址規則，但一個只跑 record 的人
        // 永遠看不到那份清單——而那正是把網銀畫面錄一整年的那種規則。
        for (rule, why) in sister_core::config::suspicious_url_rules(&config.privacy.excluded_urls)
        {
            println!("⚠  這條排除規則寫了也不會命中：{rule} — {why}");
        }

        // 開錄之前先讓過期的東西消失。放在這裡而不是收工時，是因為收工
        // 有太多種方式不會發生（Ctrl-C、當機、關機、拔電），而開錄只有
        // 一種方式：她真的開始了。一個只在乾淨結束時才生效的保留期，
        // 就是一個在最需要它的機器上永遠不生效的保留期。
        // 清理**一律**帶著畫面資料夾，即使現在是 text-only。
        // `store_images = false` 的意思是「不要再寫新的圖」，不是「以前
        // 寫的那些不存在」——不帶的話那些舊圖會永遠留在磁碟上。
        let frames_root = crate::ops::prune::frames_dir(data_dir);
        match db.prune(sister_core::now_ms(), &config.retention, Some(&frames_root)) {
            Ok(report) if !report.is_empty() => {
                println!("○ 保留期清理");
                crate::ops::prune::print_report(&report, false);
            }
            Ok(_) => {}
            // 清不掉不該擋住錄製，但也絕對不能安靜——這整個模組的存在
            // 理由就是「說好會消失的東西沒有消失」不可以沒有人知道。
            Err(e) => println!("⚠  保留期清理失敗，過期的資料還在：{e:#}"),
        }

        let images = config.capture.store_images.then(|| data_dir.join("frames"));
        let interval =
            Duration::from_millis(config.capture.min_interval_ms.max(crate::ops::MIN_TICK_MS));
        // config 等一下會被 Recorder 吃掉，但收尾的摘要與定期清理還需要這幾項
        let config_ocr = config.capture.ocr;
        let config_store_images = config.capture.store_images;
        let image_budget_mb = config.capture.max_image_mb_per_day;
        let retention = config.retention.clone();
        let prune_images = frames_root.clone();
        let mut rec = Recorder::new(backend, db, config, images)?;

        install_ctrl_c_handler();
        // 從這一刻開始算足跡。Phase 0 的驗收條件裡有三個數字，而在這之前
        // 沒有任何辦法知道它們是多少——一個量不到的預算不是預算，是一句話。
        let mut footprint = sister_capture::footprint::Footprint::new();
        let disk_at_start = rec
            .db()
            .stats()
            .map(|s| s.db_bytes + s.image_bytes)
            .unwrap_or(0);
        let deadline = duration.map(|d| Instant::now() + Duration::from_secs(d));
        println!(
            "● 錄製中（{}），每 {} ms 一次。Ctrl-C 停止。",
            backend_name().unwrap_or("?"),
            interval.as_millis()
        );

        // 保留期要在錄製途中反覆跑，不能只在開機時跑一次。一個連續開著
        // 三十天的行程，只清一次的話從第 31 天起保留期就等於不存在——
        // 而且它跑得越久、越沒有人重開，這個洞就越大。六小時一次：夠密到
        // 不會累積出一整天的過期資料，夠疏到不會變成足跡本身的一部分。
        const PRUNE_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
        let mut last_prune = Instant::now();

        let mut last_report = Instant::now();
        while !STOP.load(Ordering::SeqCst) {
            if let Some(d) = deadline
                && Instant::now() >= d
            {
                break;
            }

            match rec.tick(sister_core::now_ms()) {
                // 單次 tick 失敗不該終止 session：抓不到畫面的原因多半是
                // 暫時的（切換使用者、顯示器休眠），下一秒就好了
                Err(e) => tracing::warn!("tick failed: {e:#}"),
                Ok(Tick::Kept {
                    frame_id,
                    ocr_blocks,
                    facts,
                }) => {
                    tracing::debug!("frame #{frame_id}：{ocr_blocks} 段文字、{facts} 個事實");
                }
                Ok(_) => {}
            }

            if last_prune.elapsed() >= PRUNE_EVERY {
                last_prune = Instant::now();
                match rec
                    .db_mut()
                    .prune(sister_core::now_ms(), &retention, Some(&prune_images))
                {
                    Ok(r) if !r.is_empty() => {
                        println!("  ○ 保留期清理");
                        crate::ops::prune::print_report(&r, false);
                    }
                    Ok(_) => {}
                    Err(e) => println!("  ⚠  保留期清理失敗，過期的資料還在：{e:#}"),
                }
            }

            if last_report.elapsed() >= Duration::from_secs(60) {
                footprint.tick();
                let s = rec.stats();
                println!(
                    "  … {} tick：保留 {}、重複 {}、排除 {}、讀到 {} 行字{}",
                    s.ticks,
                    s.kept,
                    s.duplicates,
                    s.excluded,
                    s.ocr_blocks,
                    match (footprint.cpu_percent(), footprint.peak_rss_bytes()) {
                        (Some(cpu), Some(rss)) => format!(
                            "；CPU {cpu:.1}%、RAM 峰值 {}",
                            crate::fmt::bytes(rss as i64)
                        ),
                        _ => String::new(),
                    }
                );
                last_report = Instant::now();
            }

            std::thread::sleep(interval);
        }

        let stats = rec.stats().clone();
        // 收工前問一次「這段路上掉了什麼」。`doctor` 只看得到開機那一瞬間，
        // 而 UIA 會在半路上永久投降——那之後 excluded_urls 一條都不生效，
        // 卻沒有任何地方會講。見 `Backend::degradations`。
        let lost = sister_capture::Backend::degradations(rec.backend());
        rec.finish()?;
        println!(
            "\n完成：{} tick → 保留 {}、重複 {}、排除 {}、無畫面 {}",
            stats.ticks, stats.kept, stats.duplicates, stats.excluded, stats.no_screen
        );
        report_idle(&stats);
        report_exclusions(&stats);
        report_ocr(&stats, config_ocr);
        report_images(&stats, rec.timings(), image_budget_mb, config_store_images);
        // 先取最後一次足跡樣本，時間表才拿得到「這段期間燒了多少 CPU」，
        // 而那是把牆上時間和 CPU 分開講的前提。
        footprint.tick();
        report_timings(rec.timings(), stats.ticks, footprint.cpu_seconds_used());
        report_footprint(
            &footprint,
            rec.db()
                .stats()
                .map(|s| s.db_bytes + s.image_bytes)
                .unwrap_or(0)
                - disk_at_start,
            stats.image_bytes,
        );
        for line in &lost {
            println!("  ⚠  錄製途中失去的能力：{line}");
        }
        Ok(())
    }

    /// 她自己佔了多少。
    ///
    /// Phase 0 的驗收條件裡有三個數字（CPU < 3%、RAM < 400MB、
    /// 磁碟 < 300MB/天），而在這一段出現之前**沒有任何辦法知道它們是多少**。
    /// 一個量不到的預算不是預算，是一句話——而 README 上遲早要寫這些數字，
    /// 那就必須是她自己量出來的，不是我開工作管理員瞄一眼記下來的。
    ///
    /// 量不到就不印。印一個 0 或一個從三分鐘外推出來的「每天 300MB」，
    /// 都會變成一個很有說服力的假消息，而且會被抄進文件裡。
    /// Phase 0 的驗收預算（PHASES.md）。
    ///
    /// 寫在程式裡而不是只寫在文件裡，是因為文件不會在超標的時候出聲。
    /// 實測那次是 CPU 27.1%、磁碟 11.4 GB/天，而摘要照樣平鋪直敘地印出來，
    /// 沒有任何一個字說「這超標九倍」——要靠讀的人自己記得預算是多少，
    /// 再自己心算。她應該自己講。
    #[cfg(windows)]
    const BUDGET_CPU_PCT: f64 = 3.0;
    #[cfg(windows)]
    const BUDGET_RSS_BYTES: u64 = 400 * 1024 * 1024;
    #[cfg(windows)]
    const BUDGET_DISK_PER_DAY: f64 = 300.0 * 1024.0 * 1024.0;

    #[cfg(windows)]
    fn report_footprint(
        f: &sister_capture::footprint::Footprint,
        disk_delta: i64,
        image_bytes: u64,
    ) {
        /// 超標的就標出來。合格的不標——每一項都掛一個記號等於沒有記號。
        fn over(actual: f64, budget: f64) -> &'static str {
            if actual > budget { "⚠ " } else { "" }
        }

        let mut parts = Vec::new();
        let mut breached = Vec::new();
        if let Some(cpu) = f.cpu_percent() {
            parts.push(format!("{}CPU 平均 {cpu:.1}%", over(cpu, BUDGET_CPU_PCT)));
            if cpu > BUDGET_CPU_PCT {
                breached.push(format!(
                    "CPU {cpu:.1}% 超過預算 {BUDGET_CPU_PCT:.0}%（{:.0} 倍）",
                    cpu / BUDGET_CPU_PCT
                ));
            }
        }
        if let Some(rss) = f.peak_rss_bytes() {
            parts.push(format!(
                "{}RAM 峰值 {}",
                over(rss as f64, BUDGET_RSS_BYTES as f64),
                crate::fmt::bytes(rss as i64)
            ));
            if rss > BUDGET_RSS_BYTES {
                breached.push(format!(
                    "RAM {} 超過預算 {}",
                    crate::fmt::bytes(rss as i64),
                    crate::fmt::bytes(BUDGET_RSS_BYTES as i64)
                ));
            }
        }
        if let Some(per_day) = f.bytes_per_day(disk_delta.max(0) as u64) {
            // 圖與資料庫要分開講。合成一個數字的話，「磁碟 11.4 GB/天」
            // 沒辦法回答唯一有用的那個問題——該去縮圖，還是該去縮索引。
            // 實測那次就是這樣：一個很嚇人、但指不出方向的數字。
            let grew = disk_delta.max(0);
            let rest = grew - image_bytes as i64;
            // 減出負數代表這段期間有東西被刪掉了（錄到一半觸發保留期清理，
            // 或另一支 sister 在跑）。這時候「畫面 X、其他 -3 MB」是**算術
            // 上正確、意義上胡說**的一行——寧可承認拆不開。
            let breakdown = if rest < 0 {
                format!(
                    "這段實際長了 {}，但同時也有東西被刪掉，拆不開",
                    crate::fmt::bytes(grew)
                )
            } else {
                format!(
                    "這段實際長了 {}：畫面 {}、其他 {}",
                    crate::fmt::bytes(grew),
                    crate::fmt::bytes(image_bytes as i64),
                    crate::fmt::bytes(rest)
                )
            };
            parts.push(format!(
                "{}磁碟 {}/天（{breakdown}）",
                over(per_day, BUDGET_DISK_PER_DAY),
                crate::fmt::bytes(per_day as i64),
            ));
            if per_day > BUDGET_DISK_PER_DAY {
                breached.push(format!(
                    "磁碟 {}/天 超過預算 {}/天（{:.0} 倍）",
                    crate::fmt::bytes(per_day as i64),
                    crate::fmt::bytes(BUDGET_DISK_PER_DAY as i64),
                    per_day / BUDGET_DISK_PER_DAY
                ));
            }
        }
        if parts.is_empty() {
            return;
        }
        println!("  足跡：{}", parts.join("、"));
        for line in &breached {
            println!("  ⚠  {line}");
        }
        if !breached.is_empty() {
            println!(
                "        （Phase 0 的驗收條件見 docs/PHASES.md。\
                 短時間的錄製外推一整天本來就會偏高，真正算數的是整天的實測）"
            );
        }
    }

    /// 畫面檔寫了幾張、跳過幾張。
    ///
    /// `images_throttled` 一定要講出來，因為使用者遲早會點到一筆沒有圖的
    /// 搜尋結果，然後合理地以為壞掉了。講出來它是設計，不講它就是 bug。
    #[cfg(windows)]
    fn report_images(
        stats: &sister_capture::RecorderStats,
        timings: &sister_capture::timings::Timings,
        budget_mb: u64,
        store_images: bool,
    ) {
        let written = timings.store.calls;

        // 「保留了 12 張畫面、磁碟上一張圖都沒有」原本會讓這一整段消失，
        // 因為每個計數剛好都是 0。那正好是最需要說話的時候：資料夾沒權限、
        // 磁碟滿了、路徑被佔用，症狀全都長這樣。
        if written == 0 && stats.kept > 0 && store_images {
            println!(
                "  ⚠  畫面：保留了 {} 張，但一張圖都沒有寫成{}",
                stats.kept,
                if stats.image_failures > 0 {
                    format!("（失敗 {} 次）", stats.image_failures)
                } else {
                    String::new()
                }
            );
            if let Some(e) = &stats.last_image_error {
                println!("        最後一次的原因：{e}");
            }
            return;
        }
        if written == 0 && stats.images_throttled == 0 && stats.images_over_budget == 0 {
            return;
        }
        let mut line = format!(
            "  畫面：寫了 {written} 張（{}",
            crate::fmt::bytes(stats.image_bytes as i64)
        );
        if let Some(each) = timings.store.per_call().filter(|_| written > 0) {
            // 每張多大，是「該不該換編碼格式」唯一問得出答案的數字。
            line += &format!(
                "，平均一張 {}、{:.0} ms",
                crate::fmt::bytes((stats.image_bytes / written) as i64),
                each.as_secs_f64() * 1000.0
            );
        }
        line.push('）');
        if stats.images_throttled > 0 {
            line += &format!("，另外 {} 張只留了字（間隔未到）", stats.images_throttled);
        }
        println!("{line}");

        // 這一句要單獨佔一行、而且要講得像一件事，不是像一個統計欄位。
        if stats.images_over_budget > 0 {
            println!(
                "  ⚠  今天的畫面額度（{budget_mb} MB）用完了，之後的 {} 張只留了字。\
                 文字與搜尋不受影響；要留更多圖就調大 capture.max_image_mb_per_day",
                stats.images_over_budget
            );
        }
    }

    /// CPU 花到哪一段去了。
    ///
    /// 存在的理由：足跡那行說得出「CPU 平均 27.1%」，但那是一個**沒有下一步**
    /// 的數字。我為了它猜過兩次原因，兩次都猜 PNG 編碼，兩次都猜錯——實測
    /// PNG 編一張只要 1.7ms。一個超標九倍的預算配上一份說不出錢花到哪裡的
    /// 報告，只會讓人去改那個最好改的地方，而不是那個最貴的地方。
    ///
    /// **這張表拆的是牆上時間，不是 CPU。** 兩者差很多：CI 上量到 0.8 秒的
    /// tick 時間只對應 0.27 秒 CPU，三分之二是卡在顯示驅動裡等。所以標題
    /// 那行要把 CPU 秒數一起印出來——不然一份拆得很細的耗時表會被讀成
    /// 「CPU 花在哪裡」，而使用者抱怨的明明是後者。
    #[cfg(windows)]
    fn report_timings(t: &sister_capture::timings::Timings, ticks: u64, cpu_secs: Option<f64>) {
        let total = t.total();
        if total.is_zero() || ticks == 0 {
            return;
        }
        let cpu = match cpu_secs {
            // 「等」跟「算」差得夠遠才值得多說一句；差不多的時候多印只是雜訊
            Some(c) if c < total.as_secs_f64() * 0.8 => {
                format!("；其中真的燒掉 {c:.1} 秒 CPU，其餘是等（多半在顯示驅動裡）")
            }
            Some(c) => format!("；同期間燒掉 {c:.1} 秒 CPU"),
            None => String::new(),
        };
        println!(
            "  時間：{ticks} tick 佔了 {:.1} 秒（每 tick {:.0} ms）{cpu}",
            total.as_secs_f64(),
            total.as_secs_f64() * 1000.0 / ticks as f64
        );
        // CJK 是雙寬字元，`{:<6}` 只數字元數會對不齊
        let pad = |name: &str| {
            let cols: usize = name.chars().map(|c| if c.is_ascii() { 1 } else { 2 }).sum();
            " ".repeat(7usize.saturating_sub(cols))
        };
        for (name, s) in t.ranked() {
            let per_call = s.per_call().unwrap_or_default();
            println!(
                "        {name}{}{:>6.2} 秒 / {:>3.0}%　{:>5} 次，每次 {:.1} ms",
                pad(name),
                s.total.as_secs_f64(),
                s.total.as_secs_f64() / total.as_secs_f64() * 100.0,
                s.calls,
                per_call.as_secs_f64() * 1000.0
            );
        }
        // 這一行是上面那份排名對自己的誠實度檢查。它小，代表排名真的解釋了
        // CPU 花到哪裡；它大，代表最貴的東西根本沒被量到，而排第一的那一項
        // 只是「被量到的裡面最大的」——那正是一份效能報告最會騙人的形狀。
        if let Some(rest) = t.unattributed() {
            let pct = rest.as_secs_f64() / total.as_secs_f64() * 100.0;
            println!(
                "        其他{}{:>6.2} 秒 / {pct:>3.0}%　{}",
                pad("其他"),
                rest.as_secs_f64(),
                if pct >= 25.0 {
                    "⚠ 這段沒歸因到任何階段，上面的排名解釋不了它"
                } else {
                    "（沒歸因到任何階段）"
                }
            );
        }
    }

    /// 這一段的存在理由：上面那行摘要在「12 張畫面、上面的字一個都沒讀到」
    /// 的時候，看起來和一切正常一模一樣。
    ///
    /// 保留了幾張畫面是**容量**，讀到了幾行字才是**記憶**。只印前者，等於
    /// 讓一個安靜的失敗長期偽裝成成功——那正是這個專案最主要的失敗形狀。
    #[cfg(windows)]
    fn report_ocr(stats: &sister_capture::RecorderStats, ocr_enabled: bool) {
        if !ocr_enabled {
            println!("  讀字：已關閉（畫面留下了，但上面的字沒有進資料庫）");
            return;
        }
        println!(
            "  讀字：{} 行{}",
            stats.ocr_blocks,
            if stats.ocr_failures > 0 {
                format!("，{} 次失敗", stats.ocr_failures)
            } else {
                String::new()
            }
        );
        if let Some(e) = &stats.last_ocr_error {
            println!("        最後一次的錯誤：{e}");
        }
        if stats.ocr_blocks == 0 && stats.kept > 0 {
            println!(
                "  ⚠  保留了 {} 張畫面，但一行字都沒讀到——這些畫面搜不到。\
                 跑 `sister doctor` 看是引擎讀不出字，還是讀不到你這台螢幕。",
                stats.kept
            );
        }
    }
}
