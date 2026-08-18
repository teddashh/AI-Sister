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
        println!("  {} {label:<22} {detail}", if ok { "✓" } else { "✗" });
    }

    /// doctor 要用到的能力摘要。平台差異只收在這一個地方。
    ///
    /// 探測 OCR 要真的把引擎建起來，所以整份報告只問一次。
    #[derive(Default)]
    struct Caps {
        url: bool,
        ocr: bool,
        ocr_language: Option<String>,
        ocr_available: Vec<String>,
        /// 記了不該記的（排除規則失效）
        broken_privacy: Vec<String>,
        /// 其實什麼都沒記住，但你不會發現
        degraded: Vec<String>,
    }

    fn caps(config: &Config) -> Caps {
        #[cfg(windows)]
        {
            let c = sister_capture::windows::Capabilities::current(config);
            Caps {
                url: c.url,
                ocr: c.ocr,
                ocr_language: c.ocr_language.clone(),
                ocr_available: c.ocr_languages_available.clone(),
                broken_privacy: c.broken_privacy_rules(config),
                degraded: c.silently_degraded(config),
            }
        }
        #[cfg(not(windows))]
        {
            let _ = config;
            Caps::default()
        }
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

        if db_file.exists() {
            let db = Db::open(&db_file)?;
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
        let url_capture = caps.url;
        line(
            url_capture,
            "排除的網址",
            &format!(
                "{} 條規則{}",
                config.privacy.excluded_urls.len(),
                if url_capture {
                    ""
                } else {
                    "（本平台無法讀取網址，這些規則目前不生效）"
                }
            ),
        );
        // 規則寫錯的方式是安靜的：少擋了不會有任何症狀。這裡把「寫了也不會
        // 命中」的規則挑出來，因為使用者自己永遠不會發現。
        let suspicious = sister_core::config::suspicious_url_rules(&config.privacy.excluded_urls);
        if !suspicious.is_empty() {
            for (rule, why) in &suspicious {
                line(false, "  規則不會命中", &format!("{rule} — {why}"));
            }
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
            &format!("{} 天", config.retention.frames_days),
        );
        line(
            true,
            "文字與事實",
            &format!("{} 天", config.retention.text_days),
        );
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

        // 缺席的能力會讓某些排除規則整組失效，或讓她其實什麼都沒記住。
        // 這兩件事都要在開始錄之前講，不是藏在 doctor 裡等使用者自己去發現。
        let caps = Capabilities::current(&config);
        for warning in caps.broken_privacy_rules(&config) {
            println!("⚠  {warning}");
        }
        for warning in caps.silently_degraded(&config) {
            println!("⚠  {warning}");
        }

        let db = Db::open(&crate::db_path(data_dir))?;
        let backend = windows::backend(&config)?;
        let images = config.capture.store_images.then(|| data_dir.join("frames"));
        let interval = Duration::from_millis(config.capture.min_interval_ms.max(200));
        let mut rec = Recorder::new(backend, db, config, images)?;

        install_ctrl_c_handler();
        let deadline = duration.map(|d| Instant::now() + Duration::from_secs(d));
        println!(
            "● 錄製中（{}），每 {} ms 一次。Ctrl-C 停止。",
            backend_name().unwrap_or("?"),
            interval.as_millis()
        );

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

            if last_report.elapsed() >= Duration::from_secs(60) {
                let s = rec.stats();
                println!(
                    "  … {} tick：保留 {}、重複 {}、排除 {}",
                    s.ticks, s.kept, s.duplicates, s.excluded
                );
                last_report = Instant::now();
            }

            std::thread::sleep(interval);
        }

        let stats = rec.stats().clone();
        rec.finish()?;
        println!(
            "\n完成：{} tick → 保留 {}、重複 {}、排除 {}、無畫面 {}",
            stats.ticks, stats.kept, stats.duplicates, stats.excluded, stats.no_screen
        );
        Ok(())
    }
}
