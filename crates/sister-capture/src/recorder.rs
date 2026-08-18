//! 錄製迴圈：把感官訊號變成 L0 證據。
//!
//! 這個檔案裡的**順序**就是隱私架構本身（SPEC §11.2）。排除判定必須發生在
//! 截圖之前——被排除的畫面從來沒有被抓過，而不是抓了再刪。事後刪除
//! 救不了已經寫進磁碟的東西。
//!
//! 這一層完全沒有模型呼叫。它只負責抄寫。

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::time::Instant;

use sister_core::config::Config;
use sister_core::db::Db;
use sister_core::dedup::{Deduper, FrameVerdict};
use sister_core::model::{
    ClipboardEvent, FocusEvent, FocusKind, FocusSnapshot, FrameCapture, Millis, SystemEvent,
    SystemKind,
};
use sister_core::redact;

use crate::traits::{Backend, RawFrame};

/// 一天有多少毫秒。畫面額度以 UTC 天為單位重置，和
/// `frames::relative_path` 的資料夾分層是同一條線。
const DAY_MS: i64 = 24 * 60 * 60 * 1000;

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
    /// 每個排除理由各擋掉幾次。
    ///
    /// 「排除 80」這個數字沒有辦法回答使用者唯一會問的那個問題：**為什麼**。
    /// 而排除是本專案最容易安靜地過度生效的地方——一條規則寫寬了、UIA 一直
    /// 答不出密碼欄狀態、某個 app 名稱剛好是別人的子字串，症狀全都一樣：
    /// 她什麼都記不住，而摘要上只有一個沒有解釋的數字。
    ///
    /// 用理由字串當 key 是刻意的：那就是寫進 `system_events` 的同一串字，
    /// 所以摘要上看到的東西，可以原封不動拿去資料庫裡查。
    pub excluded_reasons: std::collections::BTreeMap<String, u64>,
    pub no_screen: u64,
    pub clipboard_events: u64,
    pub secrets_redacted: u64,
    pub focus_events: u64,
    pub image_bytes: u64,
    /// 保留了這一幀、但**刻意沒有寫畫面檔**的次數。
    ///
    /// 見 `CaptureConfig::image_min_interval_ms`。這個數字要印出來，因為
    /// 「有 300 筆搜尋結果，其中 240 筆點下去沒有圖」是使用者遲早會遇到、
    /// 而且會以為是壞掉的事。講出來它是設計，不講它就是 bug。
    pub images_throttled: u64,
    /// 因為**今天的畫面額度用完**而沒有寫圖的次數。
    ///
    /// 和 `images_throttled` 分開計數，因為兩者要使用者做的事完全不同：
    /// 前者是正常運作，後者是「你今天的螢幕比預算忙，接下來只會留字」——
    /// 那是一句必須說出口的話，否則使用者只會發現下午的記憶莫名其妙比
    /// 上午差，而找不到任何解釋。
    pub images_over_budget: u64,
    /// OCR 一共讀出幾行字。
    ///
    /// 「保留了 12 張畫面」和「記住了 12 張畫面上的字」是兩回事，而摘要
    /// 只印前者的話，兩者看起來一模一樣。實測踩過：12 張畫面、0 行文字、
    /// 摘要一片祥和，要等到搜尋永遠是空的才會發現。
    pub ocr_blocks: u64,
    /// OCR 失敗了幾次。
    ///
    /// OCR 壞掉不該讓錄製停擺——畫面與脈絡還是值得留下來。但它也絕對不能
    /// 靜靜地壞：一個「一直在錄、什麼都搜不到」的產品，比一個明講自己
    /// 讀不到字的產品糟得多。所以錯誤吞掉可以，計數不能不留。
    pub ocr_failures: u64,
    /// 最後一次 OCR 失敗的訊息。
    ///
    /// 只有計數的話，使用者能看見「壞了」卻沒辦法告訴我們哪裡壞——而這是
    /// 一個跑在別人機器上、我摸不到的程式。一句原文遠比一個數字有用。
    pub last_ocr_error: Option<String>,
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
    /// 上一次**真的寫出**畫面檔的時刻。見 `image_min_interval_ms`。
    last_image_ts: Option<Millis>,
    /// `image_bytes_today` 算的是哪一天（UTC 天序號）。
    image_day: i64,
    /// 今天已經寫出去多少畫面位元組。見 `max_image_mb_per_day`。
    image_bytes_today: u64,
    stats: RecorderStats,
    timings: crate::timings::Timings,
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

        // 今天已經用掉多少畫面額度，要從資料庫接回來，不能從 0 開始。
        // 從 0 開始的話，那個「每日上限」只管得住單一次執行——關掉再開就
        // 歸零，一天重開十次就是十倍額度。一個可以靠重開繞過的上限不是上限。
        let now = sister_core::now_ms();
        let image_day = now.div_euclid(DAY_MS);
        let image_bytes_today = db.image_bytes_since(image_day * DAY_MS).unwrap_or(0);

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
            last_image_ts: None,
            image_day,
            image_bytes_today,
            stats: RecorderStats::default(),
            timings: Default::default(),
        })
    }

    pub fn stats(&self) -> &RecorderStats {
        &self.stats
    }

    /// 各階段的耗時。回答「CPU 花到哪裡去了」——見 [`crate::timings`]。
    pub fn timings(&self) -> &crate::timings::Timings {
        &self.timings
    }

    pub fn db(&self) -> &Db {
        &self.db
    }

    /// 給長時間執行的呼叫端用：錄製途中還要定期做保留期清理。
    ///
    /// 一個跑了三十天的行程，如果只在啟動時清一次，那從第 31 天起
    /// 保留期就等於不存在——而它跑得越久、越沒有人重開，這個洞就越大。
    pub fn db_mut(&mut self) -> &mut Db {
        &mut self.db
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
        let t = Instant::now();
        let focus = self.backend.focus_snapshot(ts).unwrap_or_default();
        self.timings.focus.record(t.elapsed());

        // 2) 排除判定 —— 必須在任何截圖之前
        let exclusion = self.config.privacy.check(&focus);
        if let Some(reason) = exclusion.reason() {
            self.stats.excluded += 1;
            *self
                .stats
                .excluded_reasons
                .entry(reason.to_string())
                .or_default() += 1;
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
            // 剪貼簿要「跳過」而不是「不看」：她在密碼管理員裡複製的東西
            // 還躺在剪貼簿上，不把水位推過去，切回瀏覽器的下一秒就撈進來了
            self.backend.skip_clipboard(ts);
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

        // 5) 到這裡才碰螢幕——而且先用**便宜的探測圖**問一句「變了沒」。
        //    一天裡絕大多數的 tick 答案都是「沒變」，而它們原本每一個都
        //    要付一次全解析度搬運的錢，只為了算出一個 64-bit 的雜湊。
        let t = Instant::now();
        let probe = self.backend.probe_screen(ts)?;
        self.timings.probe.record(t.elapsed());
        let Some(probe) = probe else {
            self.stats.no_screen += 1;
            return Ok(Tick::NoScreen);
        };

        match self.deduper.check(probe.dhash) {
            FrameVerdict::Duplicate { run } => {
                self.stats.duplicates += 1;
                if let Some(id) = self.last_frame_id {
                    self.db.bump_frame_dup(id)?;
                }
                Ok(Tick::Duplicate { run })
            }
            FrameVerdict::New => {
                // 畫面真的變了，現在才付原生解析度的錢：OCR 要的是原生像素，
                // 縮過的圖上 12px 的字會掉到 7px，然後被讀成一串亂碼。
                let t = Instant::now();
                let full = self.backend.grab_screen(ts)?;
                self.timings.grab.record(t.elapsed());

                let Some(mut full) = full else {
                    // 探測與正式抓圖之間畫面沒了（剛好鎖屏）。`check` 已經
                    // 把去重基準推到這個雜湊上了，但我們什麼都沒存——不退回去
                    // 的話，下一張一模一樣的畫面會被判成重複，然後把重複數
                    // 加到一張**更早**的幀身上。
                    self.deduper.reset();
                    self.stats.no_screen += 1;
                    return Ok(Tick::NoScreen);
                };
                // 去重看的是探測圖，資料庫裡也就存探測圖的雜湊。兩邊必須是
                // 同一個數字，否則「這一列記的 dhash」與「當初據以判斷的
                // dhash」是兩回事，事後任何重算都會得到對不起來的答案。
                full.dhash = probe.dhash;
                self.keep_frame(ts, full, focus)
            }
        }
    }

    fn keep_frame(&mut self, ts: Millis, frame: RawFrame, focus: FocusSnapshot) -> Result<Tick> {
        let ocr = if self.config.capture.ocr {
            // OCR 失敗不擋錄製，但要留下計數——見 `RecorderStats::ocr_failures`
            let t = Instant::now();
            let result = self.backend.recognize(&frame);
            self.timings.ocr.record(t.elapsed());
            match result {
                Ok(blocks) => {
                    self.stats.ocr_blocks += blocks.len() as u64;
                    blocks
                }
                Err(e) => {
                    self.stats.ocr_failures += 1;
                    // `{e:#}` 帶上整條 anyhow context 鏈。只留最外層那句的話，
                    // 使用者回報的會是「OCR 失敗」這種等於沒說的訊息。
                    self.stats.last_ocr_error = Some(format!("{e:#}"));
                    tracing::warn!(error = %e, "OCR failed; keeping the frame without text");
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };

        let (image_path, image_bytes) = self.store_image(ts, &frame).unwrap_or_else(|e| {
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

        let t = Instant::now();
        let (frame_id, _chunk, facts) = self.db.insert_frame(
            self.session_id,
            &capture,
            image_path.as_deref(),
            image_bytes,
        )?;
        self.timings.db.record(t.elapsed());

        self.last_frame_id = Some(frame_id);
        self.stats.kept += 1;
        Ok(Tick::Kept {
            frame_id,
            ocr_blocks: capture.ocr.len(),
            facts,
        })
    }

    /// 把畫面寫到磁碟，受**兩道閘門**節制。回傳 (相對路徑, 位元組數)。
    ///
    /// 節流的是圖，不是這一幀。文字、事實、脈絡全部照常寫進資料庫，被跳過
    /// 的只有 PNG——搜尋得到的東西一筆都不會少，少的是「點下去看得到圖」。
    /// 這個取捨是刻意的：磁碟預算幾乎全部花在 PNG 上，而 PNG 是這裡面唯一
    /// 可以少存卻不會少記住東西的層（SPEC §2.3 也是這樣分層的）。
    ///
    /// 兩道閘門管的是不同的東西，缺一不可：
    ///
    /// - **最小間隔**管速率。它讓忙碌的那幾秒不會爆衝。
    /// - **每日上限**管總量。單靠間隔擋不住一整天都在變的螢幕：5 秒一張
    ///   的最壞情況是一天 17,280 張，乘上 500KB 還是 8.8 GB。
    fn store_image(&mut self, ts: Millis, frame: &RawFrame) -> Result<(Option<String>, i64)> {
        // 借用打架的關係先取出來：底下要改 `self.stats` 與 `last_image_ts`。
        // 一次 PathBuf clone 落在「畫面真的變了」這條路徑上，一秒鐘最多幾次。
        let Some(root) = self.image_dir.clone() else {
            return Ok((None, 0));
        };
        let Some(rgba) = frame.rgba.as_deref() else {
            return Ok((None, 0));
        };

        let gap = self.config.capture.image_min_interval_ms as i64;
        if self
            .last_image_ts
            .is_some_and(|prev| ts.saturating_sub(prev) < gap)
        {
            self.stats.images_throttled += 1;
            return Ok((None, 0));
        }

        // 跨日就把今天的額度歸零。用 UTC 天切，和 `frames::relative_path`
        // 的資料夾分層是同一條線，這樣「某一天的圖」在磁碟上與在預算上
        // 講的是同一天。
        let day = ts.div_euclid(DAY_MS);
        if day != self.image_day {
            self.image_day = day;
            self.image_bytes_today = 0;
        }
        let budget = self.config.capture.max_image_mb_per_day * 1024 * 1024;
        if budget > 0 && self.image_bytes_today >= budget {
            self.stats.images_over_budget += 1;
            return Ok((None, 0));
        }

        let t = Instant::now();
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
        // 只有真的寫出去才計時。把跳過的次數也算進去的話，平均會被稀釋成
        // 一個看起來很便宜、但沒有對應到任何一次實際工作的數字。
        // 於是 `timings.store.calls` 恰好就是「寫出了幾張圖」。
        self.timings.store.record(t.elapsed());
        self.last_image_ts = Some(ts);
        self.image_bytes_today += len as u64;
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

    /// 只記錄「有沒有被叫到」的假剪貼簿，用來驗證排除期間的行為。
    #[derive(Default)]
    struct SpyClipboard {
        polled: Vec<Millis>,
        skipped: Vec<Millis>,
    }

    impl crate::traits::ClipboardSource for std::rc::Rc<std::cell::RefCell<SpyClipboard>> {
        fn poll(&mut self, ts: Millis) -> Result<Option<ClipboardEvent>> {
            self.borrow_mut().polled.push(ts);
            Ok(None)
        }
        fn skip(&mut self, ts: Millis) {
            self.borrow_mut().skipped.push(ts);
        }
    }

    /// 被排除時剪貼簿必須「跳過」，不能只是「不看」。
    ///
    /// 以水位判斷新舊的來源（Windows 的 sequence number）如果只是不讀，
    /// 她在密碼管理員裡複製的密碼會留在剪貼簿上，等她切回瀏覽器的下一個
    /// tick 照樣被撈進資料庫——排除規則只延後了洩漏，沒有擋掉。
    /// 這個性質 replay 後端測不出來（它以事件時間為準），所以在這裡釘住。
    #[test]
    fn exclusion_skips_the_clipboard_instead_of_merely_not_polling_it() {
        use crate::traits::{CompositeBackend, NullInput, NullOcr, NullScreen};

        let spy = std::rc::Rc::new(std::cell::RefCell::new(SpyClipboard::default()));

        struct Focus(String);
        impl crate::traits::FocusSource for Focus {
            fn snapshot(&mut self, _ts: Millis) -> Result<FocusSnapshot> {
                Ok(FocusSnapshot {
                    app_id: Some(self.0.clone()),
                    ..Default::default()
                })
            }
        }

        let mut config = Config::default();
        config.privacy.excluded_apps = vec!["keepassxc".into()];

        let backend = CompositeBackend {
            name: "spy".into(),
            screen: NullScreen,
            focus: Focus("keepassxc.exe".into()),
            clipboard: spy.clone(),
            input: NullInput,
            ocr: NullOcr,
        };
        let mut rec = Recorder::new(backend, Db::open_in_memory().unwrap(), config, None).unwrap();

        let tick = rec.tick(1_000).unwrap();
        assert!(
            matches!(tick, Tick::Excluded { .. }),
            "應該被排除：{tick:?}"
        );

        let spy = spy.borrow();
        assert!(spy.polled.is_empty(), "排除期間不該讀剪貼簿內容");
        assert_eq!(spy.skipped, vec![1_000], "但一定要把水位推過去");
    }

    /// 每次都給一張**不一樣**的畫面，好讓去重不會把它們併掉。
    ///
    /// 均勻色塊在這裡沒有用：dhash 看的是相鄰像素的梯度，一整片灰不管
    /// 哪一階都算出同一個 hash，於是全部變成「重複」。
    struct ShiftingScreen(u32);
    impl crate::traits::ScreenSource for ShiftingScreen {
        fn grab(&mut self, ts: Millis) -> Result<Option<RawFrame>> {
            self.0 = self.0.wrapping_add(1);
            let mut px = vec![255u8; 64 * 64 * 4];
            for (i, p) in px.chunks_exact_mut(4).enumerate() {
                let v = ((i as u32 * 7 + self.0 * 40) % 256) as u8;
                (p[0], p[1], p[2]) = (v, v, v);
            }
            Ok(Some(RawFrame::from_rgba(ts, 0, 64, 64, px)))
        }
    }

    fn screen_only<O: crate::traits::Ocr>(
        ocr: O,
    ) -> Recorder<
        crate::traits::CompositeBackend<
            ShiftingScreen,
            crate::traits::NullFocus,
            crate::traits::NullClipboard,
            crate::traits::NullInput,
            O,
        >,
    > {
        let backend = crate::traits::CompositeBackend {
            name: "screen-only".into(),
            screen: ShiftingScreen(0),
            focus: crate::traits::NullFocus,
            clipboard: crate::traits::NullClipboard,
            input: crate::traits::NullInput,
            ocr,
        };
        Recorder::new(
            backend,
            Db::open_in_memory().expect("db"),
            Config::default(),
            None,
        )
        .expect("recorder")
    }

    /// **保留了畫面 ≠ 記住了畫面上的字。**
    ///
    /// 這是實測踩到的那個形狀：12 張畫面被保留、統計數字一片祥和、
    /// 搜尋永遠是空的。統計裡必須有一個欄位能把這兩件事分開，否則
    /// 「她其實什麼都沒讀到」就永遠不會被任何人看見。
    #[test]
    fn keeping_frames_without_reading_any_text_is_visible_in_the_stats() {
        let mut r = screen_only(crate::traits::NullOcr);
        for i in 1..=3 {
            r.tick(i * 1_000).expect("tick");
        }
        let s = r.stats();
        assert!(s.kept > 0, "畫面該被保留：{s:?}");
        assert_eq!(s.ocr_blocks, 0);
        assert_eq!(s.ocr_failures, 0, "沒讀到字不等於出錯");
    }

    /// OCR 失敗要留下**訊息**，不能只留下一個數字。
    ///
    /// 這支程式跑在別人的機器上。「失敗 12 次」沒辦法讓任何人往下查，
    /// 而一句原文（含 anyhow 的 context 鏈）可以。
    #[test]
    fn a_failing_ocr_keeps_the_frame_and_records_why() {
        struct Failing;
        impl crate::traits::Ocr for Failing {
            fn recognize(&mut self, _f: &RawFrame) -> Result<Vec<sister_core::model::OcrBlock>> {
                Err(anyhow::anyhow!("engine said no").context("RecognizeAsync"))
            }
        }

        let mut r = screen_only(Failing);
        r.tick(1_000).expect("OCR 壞掉不該讓整個 tick 失敗");
        let s = r.stats();
        assert_eq!(s.kept, 1, "畫面與脈絡還是值得留下來");
        assert_eq!(s.ocr_failures, 1);
        let msg = s.last_ocr_error.as_deref().unwrap_or("");
        assert!(
            msg.contains("RecognizeAsync") && msg.contains("engine said no"),
            "錯誤訊息要含整條 context 鏈，實際是：{msg:?}"
        );
    }

    // ---------- 兩段式抓圖 ----------
    //
    // 這一組測試釘住的是 alpha.4 實測踩到的兩個數字：CPU 27.1%（預算 3%）
    // 與一段被讀成 `Micr099ftTeamsTr` 的 OCR 文字。兩者是同一個原因——
    // 一份像素同時被拿去做三件需求互相衝突的事。

    #[derive(Default)]
    struct StageLog {
        probes: u32,
        grabs: u32,
        /// OCR 每次拿到的尺寸。這就是文字品質的全部。
        ocr_sizes: Vec<(u32, u32)>,
    }

    /// 灰階漸層。同一個 `seed` 一定算出同一個 dhash，換 seed 就會變。
    fn pattern(w: u32, h: u32, seed: u32) -> Vec<u8> {
        let mut px = vec![255u8; (w * h * 4) as usize];
        for (i, p) in px.chunks_exact_mut(4).enumerate() {
            let v = ((i as u32 * 7 + seed * 40) % 256) as u8;
            (p[0], p[1], p[2]) = (v, v, v);
        }
        px
    }

    /// 探測給小圖、正式抓圖給大圖——正是 Windows 後端的形狀。
    struct TwoStage {
        log: std::rc::Rc<std::cell::RefCell<StageLog>>,
    }

    const PROBE: (u32, u32) = (64, 36);
    const FULL: (u32, u32) = (256, 144);

    impl crate::traits::ScreenSource for TwoStage {
        fn grab(&mut self, ts: Millis) -> Result<Option<RawFrame>> {
            self.log.borrow_mut().grabs += 1;
            let (w, h) = FULL;
            Ok(Some(RawFrame::from_rgba(ts, 0, w, h, pattern(w, h, 1))))
        }
        fn probe(&mut self, ts: Millis) -> Result<Option<RawFrame>> {
            self.log.borrow_mut().probes += 1;
            let (w, h) = PROBE;
            Ok(Some(RawFrame::from_rgba(ts, 0, w, h, pattern(w, h, 1))))
        }
    }

    struct SizeSpy(std::rc::Rc<std::cell::RefCell<StageLog>>);
    impl crate::traits::Ocr for SizeSpy {
        fn recognize(&mut self, f: &RawFrame) -> Result<Vec<sister_core::model::OcrBlock>> {
            self.0.borrow_mut().ocr_sizes.push((f.width, f.height));
            Ok(Vec::new())
        }
    }

    fn two_stage(
        log: std::rc::Rc<std::cell::RefCell<StageLog>>,
    ) -> Recorder<
        crate::traits::CompositeBackend<
            TwoStage,
            crate::traits::NullFocus,
            crate::traits::NullClipboard,
            crate::traits::NullInput,
            SizeSpy,
        >,
    > {
        let backend = crate::traits::CompositeBackend {
            name: "two-stage".into(),
            screen: TwoStage { log: log.clone() },
            focus: crate::traits::NullFocus,
            clipboard: crate::traits::NullClipboard,
            input: crate::traits::NullInput,
            ocr: SizeSpy(log),
        };
        Recorder::new(
            backend,
            Db::open_in_memory().expect("db"),
            Config::default(),
            None,
        )
        .expect("recorder")
    }

    /// **OCR 必須拿到正式抓圖，不是那張便宜的探測圖。**
    ///
    /// 這條線是實測那串亂碼的來源。原本三件事共用同一份像素：去重只需要
    /// 9×8、存檔想要 1568、OCR 要原生解析度——結果所有人都拿到 1568，
    /// 2560 的螢幕縮成 0.61 倍，12px 的字掉到 7px，於是
    /// `Microsoft Teams` 被讀成 `Micr099ftTeamsTr`。引擎不報錯，只是讀錯。
    #[test]
    fn ocr_reads_the_full_resolution_frame_not_the_cheap_probe() {
        let log = std::rc::Rc::new(std::cell::RefCell::new(StageLog::default()));
        let mut r = two_stage(log.clone());
        r.tick(0).expect("tick");

        let log = log.borrow();
        assert_eq!(
            log.ocr_sizes,
            vec![FULL],
            "OCR 拿到探測圖就等於讀不出字；實際 {:?}",
            log.ocr_sizes
        );
    }

    /// **重複的畫面不可以付原生解析度的錢。**
    ///
    /// 這是 CPU 那個數字的修法本身。實測 120 個 tick 裡有 103 個是重複的，
    /// 而它們每一個都搬了一張全尺寸的圖，只為了算出一個 64-bit 的雜湊然後
    /// 整張丟掉。如果哪天重構把 `probe` 接回 `grab`，這次改動就等於沒做，
    /// 而摘要上完全看不出差別——只有電池會知道。
    #[test]
    fn a_duplicate_screen_never_pays_for_a_full_resolution_grab() {
        let log = std::rc::Rc::new(std::cell::RefCell::new(StageLog::default()));
        let mut r = two_stage(log.clone());
        for i in 0..4 {
            r.tick(i * 1_000).expect("tick");
        }

        assert_eq!(r.stats().kept, 1);
        assert_eq!(r.stats().duplicates, 3);

        let log = log.borrow();
        assert_eq!(log.probes, 4, "每個 tick 都要探一次");
        assert_eq!(log.grabs, 1, "但只有畫面真的變了才抓完整的那張");
    }

    /// 資料庫裡記的 dhash，必須就是當初據以判斷「變了沒」的那一個。
    ///
    /// 兩張圖尺寸不同，dhash 通常也會差一點。存錯那一個不會有任何症狀，
    /// 直到有人拿資料庫裡的雜湊去重算或比對，得到一組對不起來的答案。
    #[test]
    fn the_stored_hash_is_the_one_dedup_actually_used() {
        let log = std::rc::Rc::new(std::cell::RefCell::new(StageLog::default()));
        let mut r = two_stage(log);
        r.tick(0).expect("tick");

        let (pw, ph) = PROBE;
        let expected = sister_core::dedup::dhash_rgb(&pattern(pw, ph, 1), pw, ph, 4);
        let stored: i64 = r
            .db()
            .conn()
            .query_row("SELECT dhash FROM frames", [], |row| row.get(0))
            .expect("query");
        assert_eq!(stored as u64, expected, "存的必須是探測圖的雜湊");
    }

    /// 探測成功、正式抓圖卻沒了（剛好鎖屏），不可以污染去重狀態。
    ///
    /// `check()` 已經把基準推到那個雜湊上，但我們什麼都沒存。不退回去的話，
    /// 下一張一模一樣的畫面會被判成「重複」，然後把重複計數加到一張**更早**
    /// 的幀身上——那一幀的 `dup_run` 從此是假的，而且沒有人會發現。
    #[test]
    fn a_screen_that_vanishes_between_probe_and_grab_does_not_poison_dedup() {
        struct Vanishing(std::rc::Rc<std::cell::RefCell<bool>>);
        impl crate::traits::ScreenSource for Vanishing {
            fn grab(&mut self, ts: Millis) -> Result<Option<RawFrame>> {
                if *self.0.borrow() {
                    return Ok(None); // 第一次：抓的時候螢幕鎖了
                }
                let (w, h) = FULL;
                Ok(Some(RawFrame::from_rgba(ts, 0, w, h, pattern(w, h, 1))))
            }
            fn probe(&mut self, ts: Millis) -> Result<Option<RawFrame>> {
                let (w, h) = PROBE;
                Ok(Some(RawFrame::from_rgba(ts, 0, w, h, pattern(w, h, 1))))
            }
        }

        let locked = std::rc::Rc::new(std::cell::RefCell::new(true));
        let backend = crate::traits::CompositeBackend {
            name: "vanishing".into(),
            screen: Vanishing(locked.clone()),
            focus: crate::traits::NullFocus,
            clipboard: crate::traits::NullClipboard,
            input: crate::traits::NullInput,
            ocr: crate::traits::NullOcr,
        };
        let mut r = Recorder::new(
            backend,
            Db::open_in_memory().expect("db"),
            Config::default(),
            None,
        )
        .expect("recorder");

        assert_eq!(r.tick(0).expect("tick"), Tick::NoScreen);
        *locked.borrow_mut() = false;
        // 同一個畫面回來了。它從來沒被存過，所以必須是新的。
        assert!(
            matches!(r.tick(1_000).expect("tick"), Tick::Kept { .. }),
            "沒存成的那一張不能把後面真的那一張擋掉"
        );
    }

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
        recorder_in(steps, config, None)
    }

    fn recorder_in(
        steps: Vec<Step>,
        config: Config,
        image_dir: Option<PathBuf>,
    ) -> Recorder<crate::replay::ReplayBackend> {
        let backend = crate::replay::ReplayBackend::new(Scenario {
            name: "t".into(),
            steps,
        });
        let db = Db::open_in_memory().expect("db");
        Recorder::new(backend, db, config, image_dir).expect("recorder")
    }

    struct Tmp(PathBuf);
    impl Tmp {
        fn new(name: &str) -> Self {
            static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!(
                "sister-recorder-{}-{name}-{}",
                std::process::id(),
                N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("create temp dir");
            Self(dir)
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn count_pngs(root: &std::path::Path) -> usize {
        let Ok(entries) = std::fs::read_dir(root) else {
            return 0;
        };
        entries
            .flatten()
            .map(|e| {
                let p = e.path();
                if p.is_dir() {
                    count_pngs(&p)
                } else {
                    usize::from(p.extension().is_some_and(|x| x == "png"))
                }
            })
            .sum()
    }

    /// 每次探測都給一張不同的畫面，正式抓圖則是同一畫面的放大版。
    /// replay 後端不產生像素（`rgba: None`），所以存圖這條路徑測不到。
    #[derive(Default)]
    struct ChangingScreen {
        seed: u32,
    }
    impl crate::traits::ScreenSource for ChangingScreen {
        fn probe(&mut self, ts: Millis) -> Result<Option<RawFrame>> {
            self.seed += 1;
            let (w, h) = PROBE;
            Ok(Some(RawFrame::from_rgba(
                ts,
                0,
                w,
                h,
                pattern(w, h, self.seed),
            )))
        }
        fn grab(&mut self, ts: Millis) -> Result<Option<RawFrame>> {
            let (w, h) = FULL;
            Ok(Some(RawFrame::from_rgba(
                ts,
                0,
                w,
                h,
                pattern(w, h, self.seed),
            )))
        }
    }

    /// 每次讀出一句可搜尋、且彼此不同的文字。
    #[derive(Default)]
    struct NumberedLines(u32);
    impl crate::traits::Ocr for NumberedLines {
        fn recognize(&mut self, _f: &RawFrame) -> Result<Vec<sister_core::model::OcrBlock>> {
            self.0 += 1;
            Ok(vec![sister_core::model::OcrBlock {
                text: format!("第{}句話", self.0),
                x: 0,
                y: 0,
                w: 100,
                h: 20,
                confidence: -1.0,
            }])
        }
    }

    fn image_recorder(
        config: Config,
        dir: PathBuf,
    ) -> Recorder<
        crate::traits::CompositeBackend<
            ChangingScreen,
            crate::traits::NullFocus,
            crate::traits::NullClipboard,
            crate::traits::NullInput,
            NumberedLines,
        >,
    > {
        let backend = crate::traits::CompositeBackend {
            name: "images".into(),
            screen: ChangingScreen::default(),
            focus: crate::traits::NullFocus,
            clipboard: crate::traits::NullClipboard,
            input: crate::traits::NullInput,
            ocr: NumberedLines::default(),
        };
        Recorder::new(
            backend,
            Db::open_in_memory().expect("db"),
            config,
            Some(dir),
        )
        .expect("recorder")
    }

    /// **少存圖，但一個字都不能少。**
    ///
    /// 磁碟預算幾乎全部花在 PNG 上，而 PNG 是唯一可以少存卻不會少記住東西
    /// 的那一層。這條線同時盯著兩件事：圖真的變少了（磁碟），而且每一句話
    /// 照樣搜得到——沒有偷偷用「記得比較少」換掉「佔得比較少」。
    ///
    /// 實測沒有這道閘門時是 11.4 GB/天，預算是 300MB/天。
    #[test]
    fn throttling_images_saves_disk_without_losing_a_single_word() {
        let tmp = Tmp::new("throttle");
        let mut config = Config::default();
        config.capture.image_min_interval_ms = 5_000;

        let mut r = image_recorder(config, tmp.0.clone());
        for ts in [0, 1_000, 2_000] {
            assert!(matches!(r.tick(ts).expect("tick"), Tick::Kept { .. }));
        }

        assert_eq!(r.stats().kept, 3, "三張畫面全都保留了");
        assert_eq!(r.stats().images_throttled, 2, "但只有第一張寫了圖");
        assert_eq!(r.timings().store.calls, 1);
        assert_eq!(count_pngs(&tmp.0), 1, "磁碟上就該只有一個檔");

        // 而三句話一句都不能少——這才是重點
        for word in ["第1句話", "第2句話", "第3句話"] {
            assert!(
                !r.db().search(word, 10).expect("search").is_empty(),
                "{word} 應該仍然搜得到"
            );
        }
    }

    /// **每日上限是硬的：螢幕再忙，磁碟也不會失控。**
    ///
    /// 這條線擋的是「間隔節流看起來夠用」這個錯覺。5 秒一張聽起來很省，
    /// 但一天有 17,280 個 5 秒，乘上一張 500KB 就是 8.8 GB——比沒節流的
    /// 11.4 GB 好不了多少。間隔管得住速率，管不住總量，所以要有第二道
    /// 直接盯著預算數字本身的閘門。
    #[test]
    fn a_busy_screen_cannot_blow_through_the_daily_image_budget() {
        let tmp = Tmp::new("budget");
        let mut config = Config::default();
        config.capture.image_min_interval_ms = 0; // 只驗每日上限這一道
        config.capture.max_image_mb_per_day = 0; // 先確認「0 = 不設限」

        let mut r = image_recorder(config.clone(), tmp.0.clone());
        for i in 0..6 {
            r.tick(i * 1_000).expect("tick");
        }
        assert_eq!(r.stats().images_over_budget, 0, "0 應該是不設限");
        let unlimited = count_pngs(&tmp.0);
        assert_eq!(unlimited, 6, "不設限時每一張都該寫出來");

        // 把上限壓到 1MB。這些圖很小，所以要先讓它真的超過——
        // 直接把「今天已經用掉的量」設到上限之上，比生成 1MB 的圖乾淨。
        let tmp2 = Tmp::new("budget-hit");
        config.capture.max_image_mb_per_day = 1;
        let mut r = image_recorder(config, tmp2.0.clone());
        r.tick(0).expect("tick");
        assert_eq!(count_pngs(&tmp2.0), 1, "第一張在預算內");
        r.image_bytes_today = 2 * 1024 * 1024; // 額度用光

        for i in 1..4 {
            assert!(
                matches!(r.tick(i * 1_000).expect("tick"), Tick::Kept { .. }),
                "超出預算之後畫面仍然要保留，少的只有圖"
            );
        }
        assert_eq!(r.stats().images_over_budget, 3);
        assert_eq!(count_pngs(&tmp2.0), 1, "磁碟上不該再多出任何一個檔");

        // 而字一句都不能少——這正是這個取捨成立的前提
        for word in ["第2句話", "第3句話", "第4句話"] {
            assert!(
                !r.db().search(word, 10).expect("search").is_empty(),
                "{word} 在預算用完之後仍然要搜得到"
            );
        }
    }

    /// **關掉再開不可以拿到新的額度。**
    ///
    /// 這是「每日上限」這句話成不成立的關鍵。額度只記在記憶體裡的話，它
    /// 管得住的是「單一次執行」，不是「一天」——而錄製程式本來就會被關掉、
    /// 重開、當掉、跟著開機再起來。一個重開就能繞過的上限不是上限，而且
    /// 它會安靜地不生效：磁碟照樣長，摘要照樣是綠的。
    #[test]
    fn restarting_does_not_hand_out_a_fresh_daily_image_budget() {
        let tmp = Tmp::new("budget-restart");
        let mut config = Config::default();
        config.capture.image_min_interval_ms = 0;
        config.capture.max_image_mb_per_day = 1;

        // 同一顆資料庫貫穿兩次「執行」
        let db = Db::open_in_memory().expect("db");
        let backend = || crate::traits::CompositeBackend {
            name: "images".into(),
            screen: ChangingScreen::default(),
            focus: crate::traits::NullFocus,
            clipboard: crate::traits::NullClipboard,
            input: crate::traits::NullInput,
            ocr: NumberedLines::default(),
        };

        let now = sister_core::now_ms();
        let mut r =
            Recorder::new(backend(), db, config.clone(), Some(tmp.0.clone())).expect("recorder");
        r.tick(now).expect("tick");
        assert_eq!(count_pngs(&tmp.0), 1);
        let db = r.into_db();

        // 第二次啟動：這一天已經用掉的量必須從資料庫接回來
        let r2 = Recorder::new(backend(), db, config, Some(tmp.0.clone())).expect("recorder");
        assert!(
            r2.image_bytes_today > 0,
            "重開之後額度歸零了——那個上限等於不存在"
        );
    }

    /// 跨過午夜，額度要自己歸零。
    ///
    /// 沒有這一步的話，一個連續跑三十天的行程會在第一天就把額度用光，
    /// 然後**永遠**不再存圖——而且症狀是「她越用越沒用」，沒有人查得出來。
    #[test]
    fn the_daily_image_budget_resets_at_midnight() {
        let tmp = Tmp::new("budget-reset");
        let mut config = Config::default();
        config.capture.image_min_interval_ms = 0;
        config.capture.max_image_mb_per_day = 1;

        let mut r = image_recorder(config, tmp.0.clone());
        r.tick(0).expect("tick");
        r.image_bytes_today = 2 * 1024 * 1024;
        r.tick(1_000).expect("tick");
        assert_eq!(r.stats().images_over_budget, 1);
        assert_eq!(count_pngs(&tmp.0), 1);

        // 隔天
        r.tick(86_400_000 + 1_000).expect("tick");
        assert_eq!(r.stats().images_over_budget, 1, "新的一天不該再被擋");
        assert_eq!(count_pngs(&tmp.0), 2);
    }

    /// 節流是**時間**間隔，不是「每 N 張存一張」。
    ///
    /// 差別出現在使用者不在電腦前面的時候：畫面十分鐘才變一次的話，每一次
    /// 都該有圖，不該因為「上一張才剛存過」而被跳掉。
    #[test]
    fn a_slow_changing_screen_keeps_every_image() {
        let tmp = Tmp::new("slow");
        let mut config = Config::default();
        config.capture.image_min_interval_ms = 5_000;

        let mut r = image_recorder(config, tmp.0.clone());
        for ts in [0, 30_000, 60_000] {
            r.tick(ts).expect("tick");
        }
        assert_eq!(r.stats().images_throttled, 0);
        assert_eq!(count_pngs(&tmp.0), 3, "隔得夠開就每一張都該有圖");
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

    /// 「排除 5」對使用者沒有用；「排除 5：excluded app "1password"」才有。
    ///
    /// 這一條擋的是一個很具體的未來：某條規則寬到把一整天吃掉，而唯一的
    /// 症狀是一個沒有解釋的數字。摘要要能直接說出是誰擋的，而且說法要和
    /// 資料庫裡那一列一模一樣，這樣使用者才查得下去。
    #[test]
    fn the_summary_can_name_which_rule_ate_the_day() {
        let mut r = recorder(
            vec![
                step(0, "1password", "Vault", &["secret"]),
                step(1000, "keepassxc", "Vault", &["secret"]),
                step(2000, "1password", "Vault", &["secret"]),
            ],
            Config::default(),
        );
        for ts in [0, 1000, 2000] {
            assert!(matches!(r.tick(ts).expect("tick"), Tick::Excluded { .. }));
        }

        let reasons = &r.stats().excluded_reasons;
        assert_eq!(reasons.values().sum::<u64>(), r.stats().excluded);
        assert_eq!(
            reasons.len(),
            2,
            "two different apps, two reasons: {reasons:?}"
        );
        let (top, n) = reasons.iter().max_by_key(|(_, n)| **n).expect("some");
        assert_eq!(*n, 2);
        assert!(top.contains("1password"), "理由要說得出是誰：{top}");
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
