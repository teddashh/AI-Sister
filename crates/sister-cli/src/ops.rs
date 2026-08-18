//! 各個子命令的實作。

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;

use sister_core::db::Db;

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
                "answers": answers.iter().map(|a| serde_json::json!({
                    "kind": a.latest.kind, "value": a.latest.normalized, "raw": a.latest.raw,
                    "sightings": a.sightings, "ts": a.latest.ts,
                    "confidence": a.latest.confidence,
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

        println!(
            "🔍 「{text}」 {} 筆答案、{} 筆原文，{:.1} ms",
            answers.len(),
            hits.len(),
            elapsed.as_secs_f64() * 1000.0
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
            let out: Vec<_> = rows
                .iter()
                .map(|f| {
                    serde_json::json!({
                        "kind": f.kind, "raw": f.raw, "normalized": f.normalized,
                        "confidence": f.confidence, "ts": f.ts, "source": f.source_kind,
                        "frame_id": f.frame_id, "app_id": f.app_id, "url": f.url,
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&out)?);
            return Ok(());
        }

        if rows.is_empty() {
            println!("沒有符合的事實。");
            return Ok(());
        }
        println!("{} 筆事實\n", rows.len());
        for f in &rows {
            println!(
                "{:<10} {:<24} 「{}」",
                f.kind,
                fmt::one_line(&f.normalized, 24),
                fmt::one_line(&f.raw, 40)
            );
            println!(
                "           {}  {}  信心 {:.2}",
                fmt::timestamp(f.ts),
                fmt::context_line(f.app_id.as_deref(), f.window_title.as_deref()),
                f.confidence
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
        let per_day = if span_days >= 0.5 {
            disk_total as f64 / span_days
        } else {
            disk_total as f64
        };

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
                    "bytes_per_day": per_day,
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
        println!(
            "  資料庫    {}\n  畫面檔    {}",
            fmt::bytes(s.db_bytes),
            fmt::bytes(s.image_bytes)
        );
        // Phase 0 的退出條件之一：每天 < 300MB
        let budget = 300 * 1024 * 1024;
        let mark = if per_day as i64 <= budget {
            "✓"
        } else {
            "✗"
        };
        println!(
            "  每天約    {} {mark}  （Phase 0 預算 300 MB/天）",
            fmt::bytes(per_day as i64)
        );
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
        let _ = WindowsInput::start(sister_core::now_ms());
        let input_hooks = match WindowsInput::state() {
            HookState::Active => Some(true),
            // NotStarted 在上一行之後不可能發生；真發生了也是「沒裝上」
            HookState::Failed | HookState::NotStarted => Some(false),
        };

        let c = Capabilities::current(config);
        let mut probes = Vec::new();
        let url_probe;

        // UIA：一樣不宣稱。真的對現在的前景視窗問一次網址。
        // `✓ UIA 建得起來` 這句話的價值是零——使用者要知道的是
        // 「我的網銀規則現在到底會不會生效」。
        {
            use sister_capture::traits::FocusSource;
            let mut source = sister_capture::windows::focus::WindowsFocus::new();
            let snapshot = source.snapshot(sister_core::now_ms()).unwrap_or_default();
            let app = snapshot.app_key();
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

    pub fn run(data_dir: &Path, config: &Config) -> Result<()> {
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
        match Config::default_path() {
            Some(p) => line(
                true,
                "設定檔",
                &format!(
                    "{}{}",
                    p.display(),
                    if p.exists() { "" } else { "（用預設值）" }
                ),
            ),
            None => line(false, "設定檔", "無法決定路徑"),
        }

        println!("\n儲存");
        // 用暫時的記憶體資料庫檢查 SQLite 能力，不動到真正的資料
        let probe = Db::open_in_memory().context("open probe database")?;
        line(true, "SQLite", &probe.sqlite_version());
        let fts = probe
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name IN ('text_fts','text_fts_uni')",
                [],
                |r| r.get::<_, i64>(0),
            )
            .unwrap_or(0);
        line(fts == 2, "FTS5 雙索引", "trigram + unicode61");

        // 開著不關：底下「保留期」那一段還要拿它去問「現在有多少已過期」。
        let db = db_file.exists().then(|| Db::open(&db_file)).transpose()?;
        if let Some(db) = &db {
            line(
                true,
                "Schema 版本",
                &format!(
                    "{} (目前程式為 {})",
                    db.schema_version()?,
                    sister_core::db::SCHEMA_VERSION
                ),
            );
            let s = db.stats()?;
            line(
                true,
                "已記錄",
                &format!(
                    "{} 張畫面 · {} 段文字 · {}",
                    s.frames,
                    s.chunks,
                    fmt::bytes(s.db_bytes + s.image_bytes)
                ),
            );
        }

        println!("\n隱私");
        line(
            true,
            "排除的 app",
            &format!("{} 條規則", config.privacy.excluded_apps.len()),
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
        line(
            true,
            "排除的標題",
            &format!("{} 條規則", config.privacy.excluded_titles.len()),
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

        if let Some(ok) = caps.input_hooks {
            println!("\n節奏");
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
                Tick::Duplicate { .. } | Tick::Disabled => {}
            }
            offset += interval_ms;
        }
        rec.finish()?;

        let s = rec.stats();
        println!(
            "\n完成：{} tick → 保留 {}、重複 {}、排除 {}、無畫面 {}",
            s.ticks, s.kept, s.duplicates, s.excluded, s.no_screen
        );
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

        // 開錄之前先讓過期的東西消失。放在這裡而不是收工時，是因為收工
        // 有太多種方式不會發生（Ctrl-C、當機、關機、拔電），而開錄只有
        // 一種方式：她真的開始了。一個只在乾淨結束時才生效的保留期，
        // 就是一個在最需要它的機器上永遠不生效的保留期。
        match db.prune(
            sister_core::now_ms(),
            &config.retention,
            config
                .capture
                .store_images
                .then(|| crate::ops::prune::frames_dir(data_dir))
                .as_deref(),
        ) {
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
        let interval = Duration::from_millis(config.capture.min_interval_ms.max(200));
        // config 等一下會被 Recorder 吃掉，但收尾的摘要與定期清理還需要這幾項
        let config_ocr = config.capture.ocr;
        let image_budget_mb = config.capture.max_image_mb_per_day;
        let retention = config.retention.clone();
        let prune_images = images.clone();
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
                    .prune(sister_core::now_ms(), &retention, prune_images.as_deref())
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
        report_exclusions(&stats);
        report_ocr(&stats, config_ocr);
        report_images(&stats, rec.timings(), image_budget_mb);
        report_timings(rec.timings(), stats.ticks);
        footprint.tick();
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
            let rest = grew.saturating_sub(image_bytes as i64);
            parts.push(format!(
                "{}磁碟 {}/天（這段實際長了 {}：畫面 {}、其他 {}）",
                over(per_day, BUDGET_DISK_PER_DAY),
                crate::fmt::bytes(per_day as i64),
                crate::fmt::bytes(grew),
                crate::fmt::bytes(image_bytes as i64),
                crate::fmt::bytes(rest)
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
    ) {
        let written = timings.store.calls;
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
    #[cfg(windows)]
    fn report_timings(t: &sister_capture::timings::Timings, ticks: u64) {
        let total = t.total();
        if total.is_zero() || ticks == 0 {
            return;
        }
        println!(
            "  時間：{ticks} tick 共忙了 {:.1} 秒（每 tick {:.0} ms）",
            total.as_secs_f64(),
            total.as_secs_f64() * 1000.0 / ticks as f64
        );
        for (name, s) in t.ranked() {
            // CJK 是雙寬字元，`{:<6}` 只數字元數會對不齊
            let cols: usize = name.chars().map(|c| if c.is_ascii() { 1 } else { 2 }).sum();
            let pad = " ".repeat(7usize.saturating_sub(cols));
            let per_call = s.per_call().unwrap_or_default();
            println!(
                "        {name}{pad}{:>6.2} 秒 / {:>3.0}%　{:>5} 次，每次 {:.1} ms",
                s.total.as_secs_f64(),
                s.total.as_secs_f64() / total.as_secs_f64() * 100.0,
                s.calls,
                per_call.as_secs_f64() * 1000.0
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
