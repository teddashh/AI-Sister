//! 錄製迴圈：把感官訊號變成 L0 證據。
//!
//! 這個檔案裡的**順序**就是隱私架構本身（SPEC §11.2）。排除判定必須發生在
//! 截圖之前——被排除的畫面從來沒有被抓過，而不是抓了再刪。事後刪除
//! 救不了已經寫進磁碟的東西。
//!
//! 這一層完全沒有模型呼叫。它只負責抄寫。

use anyhow::{Context, Result};
use std::path::PathBuf;

use sister_core::config::Config;
use sister_core::db::Db;
use sister_core::dedup::{Deduper, FrameVerdict};
use sister_core::model::{
    ClipboardEvent, FocusEvent, FocusKind, FocusSnapshot, FrameCapture, Millis, SystemEvent,
    SystemKind,
};
use sister_core::redact;

use crate::traits::{Backend, RawFrame};

/// 一次 tick 的結果。呼叫端據此決定要不要記錄、要不要退避。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tick {
    /// 總開關關閉——她閉著眼睛。
    Disabled,
    /// 被排除規則擋下。畫面沒有被抓取。
    Excluded { reason: String },
    /// 這一刻沒有可用畫面（鎖屏、顯示器休眠）。
    NoScreen,
    /// 與上一張保留幀相同，只把重複計數加一。
    Duplicate { run: u32 },
    /// 保留了一張新畫面。
    Kept {
        frame_id: i64,
        ocr_blocks: usize,
        facts: usize,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecorderStats {
    pub ticks: u64,
    pub kept: u64,
    pub duplicates: u64,
    pub excluded: u64,
    pub no_screen: u64,
    pub clipboard_events: u64,
    pub secrets_redacted: u64,
    pub focus_events: u64,
    pub image_bytes: u64,
}

pub struct Recorder<B: Backend> {
    backend: B,
    db: Db,
    config: Config,
    deduper: Deduper,
    session_id: i64,
    /// 最後一張被保留的幀，重複時往它身上加計數。
    last_frame_id: Option<i64>,
    last_focus: Option<FocusSnapshot>,
    /// 上一次的排除理由。只有在理由改變時才寫 system event，
    /// 否則被排除的一小時會產生上千筆一模一樣的紀錄。
    last_exclusion: Option<String>,
    /// 畫面檔的根目錄。`None` = text-only 模式。
    image_dir: Option<PathBuf>,
    stats: RecorderStats,
}

impl<B: Backend> Recorder<B> {
    pub fn new(backend: B, mut db: Db, config: Config, image_dir: Option<PathBuf>) -> Result<Self> {
        let platform = format!("{}/{}", std::env::consts::OS, backend.name());
        let session_id = db
            .start_session(&platform, sister_core::VERSION)
            .context("start session")?;
        db.insert_system(
            session_id,
            &SystemEvent {
                ts: sister_core::now_ms(),
                kind: SystemKind::SessionStart,
                detail: None,
            },
        )?;

        let deduper = Deduper::new(config.capture.dedup_threshold);
        let image_dir = if config.capture.store_images {
            image_dir
        } else {
            None
        };

        Ok(Self {
            backend,
            db,
            config,
            deduper,
            session_id,
            last_frame_id: None,
            last_focus: None,
            last_exclusion: None,
            image_dir,
            stats: RecorderStats::default(),
        })
    }

    pub fn stats(&self) -> &RecorderStats {
        &self.stats
    }

    pub fn db(&self) -> &Db {
        &self.db
    }

    pub fn session_id(&self) -> i64 {
        self.session_id
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    /// 收尾：標記 session 結束。
    pub fn finish(&mut self) -> Result<()> {
        let ts = sister_core::now_ms();
        self.db.insert_system(
            self.session_id,
            &SystemEvent {
                ts,
                kind: SystemKind::SessionEnd,
                detail: None,
            },
        )?;
        self.db.end_session(self.session_id)?;
        Ok(())
    }

    /// 走一輪感官。`ts` 由呼叫端給定，因此 replay 完全確定性。
    pub fn tick(&mut self, ts: Millis) -> Result<Tick> {
        self.stats.ticks += 1;

        if !self.config.capture.enabled {
            return Ok(Tick::Disabled);
        }

        // 1) 先看脈絡。這一步很便宜，而且是排除判定的依據。
        let focus = self.backend.focus_snapshot(ts).unwrap_or_default();

        // 2) 排除判定 —— 必須在任何截圖之前
        let exclusion = self.config.privacy.check(&focus);
        if let Some(reason) = exclusion.reason() {
            self.stats.excluded += 1;
            if self.last_exclusion.as_deref() != Some(reason) {
                self.db.insert_system(
                    self.session_id,
                    &SystemEvent {
                        ts,
                        kind: SystemKind::Excluded,
                        detail: Some(reason.to_string()),
                    },
                )?;
                self.last_exclusion = Some(reason.to_string());
            }
            // 排除期間的畫面沒被看過，下一張必須當成全新的
            self.deduper.reset();
            self.last_frame_id = None;
            // 輸入節奏不含任何內容，繼續累積才不會在節奏訊號上開洞
            self.record_input(ts)?;
            return Ok(Tick::Excluded {
                reason: reason.to_string(),
            });
        }
        self.last_exclusion = None;

        // 3) 脈絡變了才記一筆 focus 事件
        self.record_focus_if_changed(ts, &focus)?;

        // 4) 剪貼簿。只有在沒被排除時才碰——不然密碼管理員裡複製的
        //    密碼會從這裡漏進資料庫。
        self.record_clipboard(ts, &focus)?;

        self.record_input(ts)?;

        // 5) 到這裡才真的抓畫面
        let Some(frame) = self.backend.grab_screen(ts)? else {
            self.stats.no_screen += 1;
            return Ok(Tick::NoScreen);
        };

        match self.deduper.check(frame.dhash) {
            FrameVerdict::Duplicate { run } => {
                self.stats.duplicates += 1;
                if let Some(id) = self.last_frame_id {
                    self.db.bump_frame_dup(id)?;
                }
                Ok(Tick::Duplicate { run })
            }
            FrameVerdict::New => self.keep_frame(ts, frame, focus),
        }
    }

    fn keep_frame(&mut self, ts: Millis, frame: RawFrame, focus: FocusSnapshot) -> Result<Tick> {
        let ocr = if self.config.capture.ocr {
            self.backend.recognize(&frame).unwrap_or_default()
        } else {
            Vec::new()
        };

        let (image_path, image_bytes) = self.store_image(&frame).unwrap_or_else(|e| {
            // 存不下畫面不該讓文字也跟著遺失
            tracing::warn!(error = %e, "failed to store frame image; keeping text only");
            (None, 0)
        });
        self.stats.image_bytes += image_bytes as u64;

        let capture = FrameCapture {
            ts,
            monitor: frame.monitor,
            width: frame.width,
            height: frame.height,
            dhash: frame.dhash,
            image: None,
            image_ext: "png",
            ocr,
            focus,
        };

        let (frame_id, _chunk, facts) = self.db.insert_frame(
            self.session_id,
            &capture,
            image_path.as_deref(),
            image_bytes,
        )?;

        self.last_frame_id = Some(frame_id);
        self.stats.kept += 1;
        Ok(Tick::Kept {
            frame_id,
            ocr_blocks: capture.ocr.len(),
            facts,
        })
    }

    /// 把畫面寫到磁碟。回傳 (相對路徑, 位元組數)。
    fn store_image(&self, frame: &RawFrame) -> Result<(Option<String>, i64)> {
        let (Some(root), Some(rgba)) = (self.image_dir.as_deref(), frame.rgba.as_deref()) else {
            return Ok((None, 0));
        };
        let bytes = crate::frames::encode_downscaled(
            rgba,
            frame.width,
            frame.height,
            self.config.capture.max_long_edge,
        )?;
        let rel = crate::frames::relative_path(frame.ts, frame.monitor);
        let full = root.join(&rel);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create frame dir {}", parent.display()))?;
        }
        let len = bytes.len() as i64;
        std::fs::write(&full, bytes).with_context(|| format!("write {}", full.display()))?;
        Ok((Some(rel), len))
    }

    fn record_focus_if_changed(&mut self, ts: Millis, focus: &FocusSnapshot) -> Result<()> {
        let kind = match &self.last_focus {
            None => FocusKind::Focus,
            Some(prev) if prev.app_id != focus.app_id => FocusKind::Focus,
            Some(prev) if prev.url != focus.url && focus.url.is_some() => FocusKind::UrlChange,
            Some(prev) if prev.window_title != focus.window_title => FocusKind::TitleChange,
            Some(_) => return Ok(()),
        };
        if focus.app_id.is_none() && focus.window_title.is_none() && focus.url.is_none() {
            return Ok(());
        }
        self.db.insert_focus(
            self.session_id,
            &FocusEvent {
                ts,
                kind,
                snapshot: focus.clone(),
            },
        )?;
        self.last_focus = Some(focus.clone());
        self.stats.focus_events += 1;
        Ok(())
    }

    fn record_clipboard(&mut self, ts: Millis, focus: &FocusSnapshot) -> Result<()> {
        let Some(mut event) = self.backend.poll_clipboard(ts)? else {
            return Ok(());
        };
        if event.source_app.is_none() {
            event.source_app = focus.app_id.clone();
        }
        self.redact_and_store_clipboard(event)
    }

    /// 秘密偵測與截斷。**內容在落地之前就被丟掉**，不是先存再刪。
    fn redact_and_store_clipboard(&mut self, mut event: ClipboardEvent) -> Result<()> {
        if let Some(text) = event.text.as_deref() {
            let secret = redact::looks_like_secret(text);
            if secret.is_some() && self.config.privacy.redact_clipboard_secrets {
                event.text = None;
                event.secret_suspected = true;
                self.stats.secrets_redacted += 1;
            } else {
                let (cut, truncated) = redact::truncate_utf8(text, redact::CLIPBOARD_MAX_BYTES);
                if truncated {
                    event.text = Some(cut.to_string());
                    event.truncated = true;
                }
            }
        }
        self.db.insert_clipboard(self.session_id, &event)?;
        self.stats.clipboard_events += 1;
        Ok(())
    }

    fn record_input(&mut self, ts: Millis) -> Result<()> {
        if let Some(metrics) = self.backend.drain_input(ts)? {
            self.db.insert_input(self.session_id, &metrics)?;
        }
        Ok(())
    }

    /// 交還資料庫（收尾後給 CLI 查詢用）。
    pub fn into_db(self) -> Db {
        self.db
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replay::{Scenario, Step};
    use sister_core::config::PrivacyConfig;

    fn step(at_ms: Millis, app: &str, title: &str, text: &[&str]) -> Step {
        Step {
            at_ms,
            app: Some(app.into()),
            title: Some(title.into()),
            text: text.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    fn recorder(steps: Vec<Step>, config: Config) -> Recorder<crate::replay::ReplayBackend> {
        let backend = crate::replay::ReplayBackend::new(Scenario {
            name: "t".into(),
            steps,
        });
        let db = Db::open_in_memory().expect("db");
        Recorder::new(backend, db, config, None).expect("recorder")
    }

    #[test]
    fn a_new_screen_is_kept_with_its_text_and_facts() {
        let mut r = recorder(
            vec![step(
                0,
                "chrome.exe",
                "帳單",
                &["本期應繳 NT$13,450", "客服 0800-080-123"],
            )],
            Config::default(),
        );

        match r.tick(0).expect("tick") {
            Tick::Kept {
                ocr_blocks, facts, ..
            } => {
                assert_eq!(ocr_blocks, 2);
                assert!(facts >= 2, "money and phone must be extracted, got {facts}");
            }
            other => panic!("expected Kept, got {other:?}"),
        }

        let hits = r.db().search("客服", 10).expect("search");
        assert!(
            !hits.is_empty(),
            "kept frames must be searchable immediately"
        );
    }

    #[test]
    fn an_unchanged_screen_is_collapsed_not_stored_again() {
        let mut r = recorder(
            vec![
                step(0, "chrome.exe", "帳單", &["本期應繳 NT$13,450"]),
                step(5000, "chrome.exe", "帳單", &["本期應繳 NT$13,450"]),
            ],
            Config::default(),
        );

        assert!(matches!(r.tick(0).expect("t"), Tick::Kept { .. }));
        assert_eq!(r.tick(5000).expect("t"), Tick::Duplicate { run: 1 });
        assert_eq!(r.tick(6000).expect("t"), Tick::Duplicate { run: 2 });

        let st = r.db().stats().expect("stats");
        assert_eq!(st.frames, 1, "duplicates must not create rows");
        assert_eq!(st.frames_collapsed, 2);
        assert_eq!(r.stats().kept, 1);
        assert_eq!(r.stats().duplicates, 2);
    }

    #[test]
    fn excluded_context_never_reaches_the_screen_grab() {
        // 這是隱私架構的核心斷言：被排除時，截圖根本沒有發生
        let mut r = recorder(
            vec![step(
                0,
                "keepassxc",
                "My Vault",
                &["master password: hunter2"],
            )],
            Config::default(),
        );

        let t = r.tick(0).expect("tick");
        assert!(matches!(t, Tick::Excluded { .. }), "got {t:?}");

        let st = r.db().stats().expect("stats");
        assert_eq!(st.frames, 0, "no frame may exist");
        assert_eq!(st.chunks, 0, "no text may exist");
        assert_eq!(st.ocr_blocks, 0);
        assert_eq!(st.focus_events, 0, "not even the window title");
        assert!(r.db().search("hunter2", 10).expect("search").is_empty());
    }

    #[test]
    fn exclusion_is_logged_once_not_once_per_tick() {
        let mut r = recorder(
            vec![step(0, "1password", "Vault", &["secret"])],
            Config::default(),
        );
        for ts in [0, 1000, 2000, 3000, 4000] {
            assert!(matches!(r.tick(ts).expect("tick"), Tick::Excluded { .. }));
        }
        let st = r.db().stats().expect("stats");
        assert_eq!(st.system_events, 2, "session_start + one exclusion notice");
        assert_eq!(r.stats().excluded, 5);
    }

    #[test]
    fn leaving_an_excluded_app_resets_dedup_so_the_next_screen_is_kept() {
        let mut r = recorder(
            vec![
                step(0, "chrome.exe", "帳單", &["同一個畫面"]),
                step(1000, "keepassxc", "Vault", &["secret"]),
                step(2000, "chrome.exe", "帳單", &["同一個畫面"]),
            ],
            Config::default(),
        );

        assert!(matches!(r.tick(0).expect("t"), Tick::Kept { .. }));
        assert!(matches!(r.tick(1000).expect("t"), Tick::Excluded { .. }));
        // 中間那段沒被看過，所以回來時即使畫面一樣也必須重新記錄
        assert!(
            matches!(r.tick(2000).expect("t"), Tick::Kept { .. }),
            "a gap in observation must not be papered over by dedup"
        );
    }

    #[test]
    fn clipboard_secret_is_dropped_before_it_touches_the_database() {
        let mut r = recorder(
            vec![Step {
                at_ms: 0,
                app: Some("terminal".into()),
                text: vec!["$ export KEY=...".into()],
                clipboard: Some("sk-proj-abc123def456ghi789jkl".into()),
                ..Default::default()
            }],
            Config::default(),
        );

        r.tick(0).expect("tick");
        assert_eq!(r.stats().secrets_redacted, 1);

        let st = r.db().stats().expect("stats");
        assert_eq!(st.clipboard_events, 1, "the event is kept");
        assert!(
            r.db()
                .search("sk-proj-abc123def456ghi789jkl", 10)
                .expect("search")
                .is_empty(),
            "but the secret itself must be unfindable"
        );

        let suspected: i64 = r
            .db()
            .conn()
            .query_row("SELECT secret_suspected FROM clipboard_events", [], |row| {
                row.get(0)
            })
            .expect("query");
        assert_eq!(suspected, 1);
        let text: Option<String> = r
            .db()
            .conn()
            .query_row("SELECT text FROM clipboard_events", [], |row| row.get(0))
            .expect("query");
        assert_eq!(text, None, "the content column must be empty");
    }

    #[test]
    fn ordinary_clipboard_text_is_kept_and_searchable() {
        let mut r = recorder(
            vec![Step {
                at_ms: 0,
                app: Some("chrome.exe".into()),
                text: vec!["帳單".into()],
                clipboard: Some("客服專線 0800-080-123".into()),
                ..Default::default()
            }],
            Config::default(),
        );
        r.tick(0).expect("tick");
        assert_eq!(r.stats().secrets_redacted, 0);
        assert!(
            !r.db()
                .search("0800-080-123", 10)
                .expect("search")
                .is_empty()
        );
    }

    #[test]
    fn focus_events_are_written_on_change_not_on_every_tick() {
        let mut r = recorder(
            vec![
                step(0, "chrome.exe", "帳單", &["a"]),
                step(1000, "chrome.exe", "帳單", &["b"]),
                step(2000, "code.exe", "db.rs", &["c"]),
            ],
            Config::default(),
        );
        for ts in [0, 500, 1000, 1500, 2000] {
            r.tick(ts).expect("tick");
        }
        assert_eq!(
            r.stats().focus_events,
            2,
            "chrome then code, nothing in between"
        );
    }

    #[test]
    fn a_locked_screen_is_reported_not_recorded() {
        let mut r = recorder(
            vec![Step {
                at_ms: 0,
                no_screen: true,
                ..Default::default()
            }],
            Config::default(),
        );
        assert_eq!(r.tick(0).expect("tick"), Tick::NoScreen);
        assert_eq!(r.db().stats().expect("stats").frames, 0);
    }

    #[test]
    fn disabled_capture_does_absolutely_nothing() {
        let mut config = Config::default();
        config.capture = sister_core::config::CaptureConfig {
            enabled: false,
            ..Default::default()
        };
        let mut r = recorder(
            vec![step(0, "chrome.exe", "帳單", &["should never be recorded"])],
            config,
        );

        assert_eq!(r.tick(0).expect("tick"), Tick::Disabled);
        let st = r.db().stats().expect("stats");
        assert_eq!(st.frames, 0);
        assert_eq!(st.chunks, 0);
        assert_eq!(st.focus_events, 0);
        assert_eq!(st.clipboard_events, 0);
    }

    #[test]
    fn ocr_can_be_turned_off_while_frames_are_still_tracked() {
        let mut config = Config::default();
        config.capture = sister_core::config::CaptureConfig {
            ocr: false,
            ..Default::default()
        };
        let mut r = recorder(vec![step(0, "chrome.exe", "帳單", &["密碼 1234"])], config);

        assert!(matches!(
            r.tick(0).expect("t"),
            Tick::Kept { ocr_blocks: 0, .. }
        ));
        let st = r.db().stats().expect("stats");
        assert_eq!(st.frames, 1);
        assert_eq!(st.chunks, 1, "the window title is still indexed");
        assert!(r.db().search("密碼 1234", 10).expect("search").is_empty());
    }

    #[test]
    fn a_url_exclusion_blocks_even_when_the_app_is_allowed() {
        let config = Config {
            privacy: PrivacyConfig {
                excluded_urls: vec!["*://*.mybank.example/*".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        let mut r = recorder(
            vec![Step {
                at_ms: 0,
                app: Some("chrome.exe".into()),
                title: Some("轉帳".into()),
                url: Some("https://www.mybank.example/transfer".into()),
                text: vec!["餘額 NT$1,234,567".into()],
                ..Default::default()
            }],
            config,
        );

        assert!(matches!(r.tick(0).expect("t"), Tick::Excluded { .. }));
        assert_eq!(r.db().stats().expect("stats").frames, 0);
    }

    #[test]
    fn session_lifecycle_is_recorded() {
        let mut r = recorder(vec![step(0, "chrome.exe", "x", &["y"])], Config::default());
        r.tick(0).expect("tick");
        r.finish().expect("finish");

        let ended: Option<i64> = r
            .db()
            .conn()
            .query_row("SELECT ended_at FROM sessions WHERE id = 1", [], |row| {
                row.get(0)
            })
            .expect("query");
        assert!(ended.is_some(), "the session must be closed");

        let kinds: Vec<String> = {
            let conn = r.db().conn();
            let mut stmt = conn
                .prepare("SELECT kind FROM system_events ORDER BY id")
                .expect("prepare");
            let rows = stmt.query_map([], |row| row.get(0)).expect("query");
            rows.flatten().collect()
        };
        assert_eq!(kinds, vec!["session_start", "session_end"]);
    }

    #[test]
    fn replay_scenario_runs_end_to_end_deterministically() {
        // 同一份腳本跑兩次，結果必須一模一樣——這是 replay 評測的前提
        let steps = vec![
            step(0, "chrome.exe", "帳單", &["本期應繳 NT$13,450"]),
            step(2000, "chrome.exe", "帳單", &["本期應繳 NT$13,450"]),
            step(4000, "code.exe", "db.rs", &["ERR_CONNECTION_REFUSED"]),
            step(6000, "keepassxc", "Vault", &["secret"]),
            step(8000, "code.exe", "db.rs", &["fn main() {}"]),
        ];

        let run = |steps: Vec<Step>| {
            let mut r = recorder(steps, Config::default());
            let outcomes: Vec<Tick> = (0..10).map(|i| r.tick(i * 1000).expect("tick")).collect();
            let st = r.db().stats().expect("stats");
            (outcomes, st.frames, st.facts, r.stats().clone())
        };

        let a = run(steps.clone());
        let b = run(steps);
        assert_eq!(a, b, "replay must be deterministic");

        let (outcomes, frames, _, stats) = a;
        assert!(frames >= 3, "distinct screens must all be kept");
        assert_eq!(stats.excluded, 2, "the vault is excluded at 6s and 7s");
        assert!(outcomes.iter().any(|t| matches!(t, Tick::Duplicate { .. })));
    }
}
