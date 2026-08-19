//! 從使用者的說法直接回答，繞過全文比對。
//!
//! 螢幕上寫的是「客服**專線**」，使用者問的是「電話」——全文檢索永遠接不起
//! 這兩個詞，但 L1 早就把那串數字標成 `phone` 了。這裡就是把使用者的說法接到
//! 事實型別上，然後直接回答。純查表、零模型。
//!
//! **這一層放在 core，因為兩個介面都要用同一份。** 它本來只長在 `sister-cli`
//! 裡，於是 `sister query 電話` 答得出號碼、字母人問同一句話卻只做全文比對，
//! 兩邊得到完全不同的答案——而字母人正是他每天真的會用的那一個。同一個教訓
//! 已經在 [`crate::question::shape`] 和 [`crate::config::Config::db_path`]
//! 各發生過一次了。

use std::collections::HashMap;

use crate::db::{Db, FactRow};

/// 一個答案：正規化後的值，加上「最後一次在哪裡看到它」。
#[derive(Debug, Clone)]
pub struct Answer {
    pub latest: FactRow,
    /// 這個值一共被看見過幾次。3 次目擊是同一個答案，不是三個答案。
    pub sightings: usize,
}

/// 依使用者的問法查 L1 事實。認不出問的是哪一類就回空集合——不要亂猜一堆
/// 事實塞給他。
///
/// 回傳的筆數會**超過** `limit` 一筆代表被切掉了（見 [`Answers::truncated`]）。
pub fn answers(db: &Db, query: &str, limit: usize) -> anyhow::Result<Answers> {
    // 同一個號碼在三個畫面出現過，是同一個答案、三次目擊——不是三個答案。
    // 併成一筆並保留最近一次的出處，因為使用者要追的是「最後看到它的地方」。
    //
    // 併和數都在 SQL 裡做（`fact_sightings`）。以前是抓最近 40 列回來在
    // 記憶體裡數，於是「看過 200 次」會被講成「看過 40 次」，而一頁吐出 40
    // 個新號碼的時候，一年來每週都看到的那個號碼會整個掉出窗外。
    //
    // 多要一筆，這樣「剛好 limit 筆」和「被切掉了」分得開。
    let mut merged: HashMap<String, Answer> = HashMap::new();
    for kind in crate::facts::kinds_for_query(query) {
        // 一句問話可以命中兩種 kind（「多少錢」→ money 和 percent），而同一
        // 個正規化字串理論上不會跨 kind 重複。真的重複的話取比較新的那一筆，
        // 次數相加——這比讓其中一邊安靜地覆蓋掉另一邊誠實。
        for (row, sightings) in db.fact_sightings(kind.as_str(), limit + 1)? {
            match merged.get_mut(&row.normalized) {
                Some(a) => {
                    a.sightings += sightings as usize;
                    if row.ts > a.latest.ts {
                        a.latest = row;
                    }
                }
                None => {
                    merged.insert(
                        row.normalized.clone(),
                        Answer {
                            latest: row,
                            sightings: sightings as usize,
                        },
                    );
                }
            }
        }
    }

    let mut out: Vec<Answer> = merged.into_values().collect();
    out.sort_by_key(|a| std::cmp::Reverse(a.latest.ts));
    let truncated = out.len() > limit;
    out.truncate(limit);
    Ok(Answers {
        items: out,
        truncated,
    })
}

/// [`answers`] 的結果，加上「是不是還有更多」。
///
/// `truncated` 單獨一個欄位而不是讓呼叫端自己比長度：「剛好 10 筆」和「第
/// 11 個不同的答案被切掉了」在畫面上長得一模一樣，而後者的意思是她其實還
/// 知道別的。同一條紀律已經在原文那一半（`hits` 的「底下還有」）做過一次，
/// ★ 那一半當時漏掉了。
#[derive(Debug, Clone, Default)]
pub struct Answers {
    pub items: Vec<Answer>,
    pub truncated: bool,
}

/// 一筆都沒找到的時候，她**查得到**的那幾個理由。
///
/// 原本那句話是「她可能當時沒在看，或那段被排除規則擋掉了」——兩個猜測、零個
/// 證據，而兩件事她其實都答得出來：排除稽核和暫停稽核都在資料庫裡，`sister
/// stats` 早就在印了。字母人那邊更糟，它講的是「這件事我沒看到過」，一句斷言
/// ——而正確答案可能是「你自己叫我不要看那個網站」。
///
/// 這裡只回**事實**，不回句子：終端機和字母人的講法不一樣，但根據要是同一份。
/// 同一條紀律見這個模組開頭。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BlindSpots {
    /// 她一共記過幾段文字。
    ///
    /// `0` **不等於**「她還沒開始記」。文字只有在 OCR 讀出東西的時候才寫，
    /// 所以 `capture.ocr = false`（設定得起來，doctor 會說「OCR 已關閉」），
    /// 或者 OCR 裝了卻讀不到東西——**這個專案已知的主要故障形狀**——的時候，
    /// 錄了一整天、幾千張畫面，這個數字照樣是 0。要分開這兩件事得配
    /// [`frames`](Self::frames) 一起看。
    pub chunks: i64,
    /// 她一共留下幾張畫面（保留幀，不含被去重折疊掉的）。
    ///
    /// 存在的理由只有一個：`chunks == 0 && frames > 0` 的意思是**她看了，但
    /// 一個字都沒讀出來**。那和「她還沒開始記」是相反的處境，而叫一個 OCR
    /// 壞掉的人「先跑 `sister record`」，只會讓他再錄一天空的。
    pub frames: i64,
    /// 她一共開過幾場錄製。
    ///
    /// `sessions` 這張表**不在任何保留期或 `forget` 的射程內**，所以它是
    /// 「她到底有沒有錄過」唯一還算得準的證據。`chunks == 0 && frames == 0`
    /// 配上 `sessions > 0`，講的是「錄過，但那些東西被忘掉了／過期了」——
    /// 而按下「忘掉這一整天」之後看到的正是這個組合。這時候叫他「先跑
    /// `sister record`」，是在叫他重做一件他剛剛才故意做掉的事。
    pub sessions: i64,
    /// 排除規則生效過的（理由, 段數）。**段不是張**——見
    /// [`Db::exclusion_audit`](crate::db::Db::exclusion_audit)。
    pub excluded: Vec<(String, i64)>,
    /// 暫停過幾段。
    pub paused_episodes: i64,
    /// 暫停一共多久。有一段配不起來的時候這個數字會偏小，
    /// 見 [`Db::pause_audit`](crate::db::Db::pause_audit)。
    pub paused_ms: i64,
    /// 最後一段暫停還沒結束（現在仍在暫停，或上次錄製在暫停中結束）。
    ///
    /// `paused_ms` **不含**這一段——它還在跑，算進去會讓同一顆資料庫每次
    /// 查出來的數字都不一樣。所以只看 `paused_ms` 的表面會說「一共 0 秒」，
    /// 而真相是三天前按下的暫停到現在都沒有解除。
    pub paused_open: bool,
    /// 有幾段暫停的開頭已經被保留期刪掉，只剩 `resume`。
    ///
    /// 這幾段算進了 `paused_episodes` 卻**沒有**算進 `paused_ms`，所以
    /// `paused_ms > 0` 時它是下限而不是精確值。
    pub paused_truncated: i64,
}

impl BlindSpots {
    /// 有沒有任何一個查得到的理由。`false` = 她真的記了，而裡面就是沒有。
    pub fn any(&self) -> bool {
        self.chunks == 0 || !self.excluded.is_empty() || self.paused_episodes > 0
    }
}

/// 查出 [`BlindSpots`]。
///
/// 範圍是整顆資料庫而不是某個時間窗：她不知道使用者心裡想的是哪一段，而把
/// 「上禮拜二下午」猜錯之後給出的理由，比不給理由更糟。所以講的是「她記過的
/// 這段期間裡」，而每一條都附得出時間讓人自己去對。
pub fn blind_spots(db: &Db) -> anyhow::Result<BlindSpots> {
    let stats = db.stats()?;
    let pauses = db.pause_audit()?;
    Ok(BlindSpots {
        chunks: stats.chunks,
        frames: stats.frames,
        sessions: stats.sessions,
        excluded: db
            .exclusion_audit()?
            .into_iter()
            .map(|e| (e.reason, e.episodes))
            .collect(),
        paused_episodes: pauses.episodes,
        paused_ms: pauses.total_ms,
        paused_open: pauses.open_since.is_some(),
        paused_truncated: pauses.truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FocusSnapshot, FrameCapture, SystemEvent, SystemKind};

    /// 一顆全新的資料庫上，「沒找到」只有一個理由，而它不是「我沒看過那件事」。
    #[test]
    fn a_database_that_never_recorded_says_so_instead_of_denying_it() {
        let db = Db::open_in_memory().expect("db");
        let b = blind_spots(&db).expect("blind");
        assert_eq!(b.chunks, 0);
        assert_eq!(b.frames, 0, "她連看都還沒看過");
        assert_eq!(b.sessions, 0, "連一場錄製都還沒開過");
        assert!(b.any(), "「我還沒開始記」本身就是一個查得到的理由");
    }

    /// **錄過，然後他自己把它忘掉了。**
    ///
    /// `sister forget --last 7d --yes` 或按下「忘掉這一整天」之後，三個計數器
    /// 全部歸零——和一顆全新的資料庫長得一模一樣。差別在 `sessions`：那張表
    /// 不在任何保留期或 `forget` 的射程內。少了這一欄，畫面會叫他「先跑
    /// `sister record`」，也就是重做一件他剛剛才故意做掉的事。
    #[test]
    fn a_memory_he_erased_is_not_a_memory_she_never_had() {
        let mut db = Db::open_in_memory().expect("db");
        let s = db.start_session("test", "0.0.1").expect("session");
        db.insert_frame(
            s,
            &FrameCapture {
                ts: 1_000,
                monitor: 0,
                width: 1920,
                height: 1080,
                dhash: 0xF00D,
                image: None,
                image_ext: "webp",
                ocr: vec![crate::model::OcrBlock {
                    text: "客服專線 0800-080-123".into(),
                    x: 0,
                    y: 0,
                    w: 400,
                    h: 18,
                    confidence: 0.95,
                }],
                focus: FocusSnapshot {
                    app_id: Some("chrome.exe".into()),
                    app_name: Some("Chrome".into()),
                    window_title: Some("帳單查詢".into()),
                    url: None,
                    pid: Some(42),
                    password_field: false,
                },
            },
            None,
            0,
        )
        .expect("insert");
        assert!(
            blind_spots(&db).expect("blind").chunks > 0,
            "先確定真的記到了"
        );

        db.forget(0, 2_000, None).expect("forget");

        let b = blind_spots(&db).expect("blind");
        assert_eq!((b.chunks, b.frames), (0, 0), "他要求的：什麼都不剩");
        assert_eq!(b.sessions, 1, "但「她錄過」這件事還在");
    }

    /// **錄了一整天，一個字都沒讀出來。**
    ///
    /// 這是這個專案已知的主要故障形狀（OCR 裝得起來但讀不到東西），也是
    /// `capture.ocr = false` 的正常樣子。`chunks` 兩種情況都是 0，所以只看
    /// 它的話，兩個表面都會說「她還沒記過任何東西——先跑 `sister record`」
    /// ——叫一個 OCR 壞掉的人再錄一天空的。
    #[test]
    fn watched_all_day_and_read_nothing_is_not_the_same_as_never_started() {
        let mut db = Db::open_in_memory().expect("db");
        let s = db.start_session("test", "0.0.1").expect("session");
        // 有畫面、沒有文字（`ocr` 是空的）：正是 OCR 讀不到東西時寫進去的
        // 樣子。dhash 每張都不同，不然會被去重折疊掉。
        for (i, ts) in [1_000, 2_000, 3_000].into_iter().enumerate() {
            db.insert_frame(
                s,
                &FrameCapture {
                    ts,
                    monitor: 0,
                    width: 1920,
                    height: 1080,
                    dhash: 0xDEAD_0000 + i as u64,
                    image: None,
                    image_ext: "webp",
                    ocr: Vec::new(),
                    focus: FocusSnapshot {
                        app_id: Some("chrome.exe".into()),
                        app_name: Some("Chrome".into()),
                        window_title: Some("網路銀行".into()),
                        url: None,
                        pid: Some(42),
                        password_field: false,
                    },
                },
                Some("/tmp/x.webp"),
                1024,
            )
            .expect("insert");
        }
        let b = blind_spots(&db).expect("blind");
        assert_eq!(b.chunks, 0, "一個字都沒有");
        assert_eq!(b.frames, 3, "但她確實看過三張");
        assert!(b.any());
    }

    /// 她記了，但那段時間他自己叫她別看。這才是那句「這件事我沒看到過」最
    /// 可能說錯話的場合——東西在，只是她不准看。
    #[test]
    fn the_rules_he_wrote_himself_are_a_reason_she_can_actually_point_at() {
        let mut db = Db::open_in_memory().expect("db");
        let s = db.start_session("test", "0.0.1").expect("session");
        for (kind, detail, ts) in [
            (SystemKind::Excluded, Some("excluded url"), 1_000),
            (SystemKind::Excluded, Some("excluded url"), 2_000),
            (SystemKind::Excluded, Some("excluded app: keepassxc"), 3_000),
            (SystemKind::CapturePaused, None, 4_000),
            (SystemKind::CaptureResumed, None, 9_000),
        ] {
            db.insert_system(
                s,
                &SystemEvent {
                    ts,
                    kind,
                    detail: detail.map(str::to_string),
                },
            )
            .expect("system event");
        }

        let b = blind_spots(&db).expect("blind");
        assert!(b.any());
        // 段數多的排前面——「12 段」比「1 段」更值得他去看
        assert_eq!(
            b.excluded,
            vec![
                ("excluded url".to_string(), 2),
                ("excluded app: keepassxc".to_string(), 1),
            ]
        );
        assert_eq!(b.paused_episodes, 1);
        assert_eq!(b.paused_ms, 5_000);
    }
}
