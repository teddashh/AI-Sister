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
    /// `0` **不等於**「她還沒開始記」，但它也**不是**「OCR 沒讀到東西」——
    /// 這一欄數的是整張 `text_chunks`，而 `Db::insert_focus` 每次換視窗就順手
    /// 寫進去一列視窗標題、一列網址（那兩種完全不經過 OCR）。要問讀字那一段
    /// 是不是斷的，看 [`ocr_blocks`](Self::ocr_blocks)。
    pub chunks: i64,
    /// OCR 一共讀出幾行字（`ocr_blocks` 那張表）。
    ///
    /// **這一欄是後來補的，而它補的是一個「已經寫好卻永遠不會執行」的分支。**
    ///
    /// 「她看過 N 張畫面，但一個字都沒讀出來」——這個專案已知的主要故障形狀，
    /// 而且是唯一一個「錄製照跑、畫面照留、搜尋永遠是空的」的形狀——本來掛在
    /// `chunks == 0 && frames > 0` 底下。可是一台 OCR 全死的機器上 `chunks`
    /// 不是 0：視窗標題和網址照樣一直寫進去。於是那台機器走到的是最後那一句
    /// 「她記的每一段裡都沒有這個字」，和一台一切正常、那件事真的沒發生過的
    /// 機器一模一樣，而正確的下一步（`sister doctor`）從來沒有被講出口。
    ///
    /// 那個分支唯一到得了的地方是測試，因為測試直接呼叫 `insert_frame` 而從不
    /// 呼叫 `insert_focus`——它守著一個真的 recorder 產生不出來的狀態。
    pub ocr_blocks: i64,
    /// 她一共留下幾張畫面（保留幀，不含被去重折疊掉的）。
    ///
    /// 存在的理由只有一個：`ocr_blocks == 0 && frames > 0` 的意思是**她看了，
    /// 但一個字都沒讀出來**。那和「她還沒開始記」是相反的處境，而叫一個 OCR
    /// 壞掉的人「先跑 `sister record`」，只會讓他再錄一天空的。
    pub frames: i64,
    /// 她**曾經**開始記過東西嗎。
    ///
    /// `chunks == 0 && frames == 0` 配上這個 `true`，講的是「錄過，但那些東西
    /// 被忘掉了／過期了」——而按下「忘掉這一整天」之後看到的正是這個組合。
    /// 這時候叫他「先跑 `sister record`」，是在叫他重做一件他剛剛才故意做掉
    /// 的事。
    ///
    /// 以前這裡是 `sessions: i64`，一個「她一共開過幾場」的計數，理由是那張表
    /// 不在任何保留期或 `forget` 的射程內。那個理由現在反過來了：那張表**進了**
    /// 射程（見 `retention::delete_empty_sessions`），因為留著的那幾列說的是
    /// 「那天 13:02 到 17:44 她在錄」，而他按的是忘掉。所以需要的那一個位元
    /// 搬到 `meta` 裡去了——見 [`Db::ever_recorded`](crate::db::Db::ever_recorded)。
    ///
    /// 而且沒有任何一個讀者用得到那個計數：三個表面問的都是 `> 0`。一個只會
    /// 被拿來比 0 的數字，本來就該是布林——留著數字，讀的人遲早會拿它去講
    /// 「你錄過 87 場」，而那句話從那一刻起就開始腐爛。
    pub ever_recorded: bool,
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
    ///
    /// **這個欄位講的是紀錄，不是現在。** 要問「她此刻閉著眼睛沒有」，看
    /// [`paused_now`](Self::paused_now)。
    pub paused_open: bool,
    /// 她**此刻**是不是暫停的（`paused.flag` 在不在，見 [`crate::pause`]）。
    ///
    /// 和 [`paused_open`](Self::paused_open) 是兩個不同的問題，而兩者可以
    /// 各自為真：
    ///
    /// - `paused_open && !paused_now`：暫停中關掉了 `sister record`，事後才
    ///   解除。解除那一刻沒有人在跑 recorder，`CaptureResumed` 就沒有人寫，
    ///   資料庫從此永遠掛著一段沒收尾的暫停。把這個講成「她現在閉著眼睛」，
    ///   是一則**再也不會消失**的假警報。
    /// - `!paused_open && paused_now`：反過來，按下暫停的那一刻沒有人在錄，
    ///   所以紀錄裡看不到。而下一次 `sister record` 會什麼都記不到。
    ///
    /// 只有旗標答得出「現在」，只有資料庫答得出「那三個小時」。
    pub paused_now: bool,
    /// 有幾段暫停的開頭已經被保留期刪掉，只剩 `resume`。
    ///
    /// 這幾段算進了 `paused_episodes` 卻**沒有**算進 `paused_ms`，所以
    /// `paused_ms > 0` 時它是下限而不是精確值。
    pub paused_truncated: i64,
    /// 這一題她只看了最近幾天。`None` = 看完了整顆資料庫。
    ///
    /// 見 [`Db::scan_horizon_days`]：產不出相鄰雙字的查詢（最常見的是一個字
    /// 的中文）只剩掃描可走，而掃描夾在 30 天內——那和 `text_days` 預設的
    /// 365 天差 12 倍。少了這個欄位，「她記的每一段裡都沒有這個字」會把
    /// 十二分之一的資料講成全部。
    pub scan_horizon_days: Option<i64>,
    /// 現在有沒有人在錄（[`crate::heartbeat`] 的心跳還新鮮）。
    ///
    /// 只在「一段字都沒有」那組句子裡用得到，而它在那裡分開的是兩件事：
    ///
    /// - 她**沒**在錄、錄過、什麼都不剩 → 「被忘掉了，或是過了保留期」。
    ///   沒有東西會再進來，所以那句話是完整的。
    /// - 她**正在**錄、錄過、什麼都不剩 → 那句話少了一種可能，而且
    ///   正好是最常見的那一種：他三秒前才按下「開始記錄」。第一次用的人問的
    ///   第一個問題就落在這裡，然後被告知他的紀錄被忘掉了或過期了。
    ///
    /// 放在這裡而不是各自判：終端機和字母人共用這一份，而「同一句話在兩個
    /// 地方得到兩種答案」是這個專案反覆踩到的坑。
    pub recording_now: bool,
}

impl BlindSpots {
    /// 留了畫面卻一行字都沒讀出來——**讀字那一段是斷的**。
    ///
    /// 門檻不是 `frames > 0`：一台剛開始錄、螢幕上正好沒有字的機器（桌布、
    /// 全螢幕影片）會在頭幾秒踩到那個條件，而那是一則假警報。留下來的畫面
    /// 是**去重過**的，也就是十張代表十次螢幕真的變過；十次全都一個字都沒有
    /// 的工作日不存在。
    ///
    /// 反過來的錯（該喊沒喊）代價高得多：他會以為她只是記性不好，然後不用了。
    /// 所以門檻壓得低，而那句話本身也只是叫他去跑 `doctor`——那支指令會當場
    /// 做一次真的 OCR，答得出是哪一種。
    pub fn ocr_is_dead(&self) -> bool {
        const ENOUGH_TO_BE_SURE: i64 = 10;
        self.ocr_blocks == 0 && self.frames >= ENOUGH_TO_BE_SURE
    }

    /// 有沒有任何一個查得到的理由。`false` = 她真的記了，而裡面就是沒有。
    ///
    /// 「她此刻是暫停的」和「這一題只掃了 30 天」都算理由：前者是他下一步該
    /// 做什麼，後者是這句「沒有」到底涵蓋了多少。
    pub fn any(&self) -> bool {
        self.chunks == 0
            || self.ocr_is_dead()
            || !self.excluded.is_empty()
            || self.paused_episodes > 0
            || self.paused_now
            || self.scan_horizon_days.is_some()
    }
}

/// 查出 [`BlindSpots`]。
///
/// 範圍是整顆資料庫而不是某個時間窗：她不知道使用者心裡想的是哪一段，而把
/// 「上禮拜二下午」猜錯之後給出的理由，比不給理由更糟。所以講的是「她記過的
/// 這段期間裡」，而每一條都附得出時間讓人自己去對。
///
/// 要 `data_dir` 和 `query`，是因為有兩個理由資料庫自己答不出來：她**此刻**
/// 是不是暫停的（那在一個檔案裡，見 [`BlindSpots::paused_now`]），以及這一題
/// 有沒有走到那條只看 30 天的掃描（那取決於問了什麼字，見
/// [`BlindSpots::scan_horizon_days`]）。兩個都放在這裡判，是為了不讓終端機和
/// 字母人各判一次——同一句話在兩個地方得到兩種答案，是這個專案反覆踩到的坑。
pub fn blind_spots(db: &Db, data_dir: &std::path::Path, query: &str) -> anyhow::Result<BlindSpots> {
    let stats = db.stats()?;
    let pauses = db.pause_audit()?;
    Ok(BlindSpots {
        chunks: stats.chunks,
        ocr_blocks: stats.ocr_blocks,
        frames: stats.frames,
        ever_recorded: db.ever_recorded()?,
        excluded: db
            .exclusion_audit()?
            .into_iter()
            .map(|e| (e.reason, e.episodes))
            .collect(),
        paused_episodes: pauses.episodes,
        paused_ms: pauses.total_ms,
        paused_open: pauses.open_since.is_some(),
        paused_now: crate::pause::is_paused(data_dir),
        paused_truncated: pauses.truncated,
        scan_horizon_days: db.scan_horizon_days(query)?,
        recording_now: crate::heartbeat::is_recording(data_dir, crate::now_ms()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FocusSnapshot, FrameCapture, SystemEvent, SystemKind};

    /// 這一組測的是資料庫答得出來的那幾條，不是暫停旗標。給一個一定不存在的
    /// 資料目錄：`is_paused` 對「連父目錄都不在」回的是 `Ok(false)`，於是
    /// `paused_now` 是 false，不會干擾底下的判斷。要驗旗標的那一條自己建目錄。
    fn nowhere() -> &'static std::path::Path {
        std::path::Path::new("/sister-tests-no-such-dir")
    }

    /// 一顆全新的資料庫上，「沒找到」只有一個理由，而它不是「我沒看過那件事」。
    #[test]
    fn a_database_that_never_recorded_says_so_instead_of_denying_it() {
        let db = Db::open_in_memory().expect("db");
        let b = blind_spots(&db, nowhere(), "電話").expect("blind");
        assert_eq!(b.chunks, 0);
        assert_eq!(b.frames, 0, "她連看都還沒看過");
        assert!(!b.ever_recorded, "連一場錄製都還沒開過");
        assert!(b.any(), "「我還沒開始記」本身就是一個查得到的理由");
    }

    /// **錄過，然後他自己把它忘掉了。**
    ///
    /// `sister forget --last 7d --yes` 或按下「忘掉這一整天」之後，三個計數器
    /// 全部歸零——和一顆全新的資料庫長得一模一樣。差別在 `ever_recorded`。
    /// 少了它，畫面會叫他「先跑 `sister record`」，也就是重做一件他剛剛才
    /// 故意做掉的事。
    ///
    /// 這一條以前是靠「`sessions` 那張表誰都不刪」成立的，而那正是它的問題：
    /// 留著的那一列說的是「那天 13:02 到 17:44 她在錄」。所以現在那張表也
    /// 一起走（`assert` 在下面），只有那一個位元留在 `meta` 裡。
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
            blind_spots(&db, nowhere(), "電話").expect("blind").chunks > 0,
            "先確定真的記到了"
        );

        db.forget(0, 2_000, None).expect("forget");

        let b = blind_spots(&db, nowhere(), "電話").expect("blind");
        assert_eq!((b.chunks, b.frames), (0, 0), "他要求的：什麼都不剩");
        assert!(b.ever_recorded, "但「她錄過」這件事還在");
        // 她**還在錄**（這一場沒有 `ended_at`），所以那一列現在動不得：接下來
        // 每一拍都還要指著它。這不是漏網，是 `delete_empty_sessions` 那道守衛。
        assert_eq!(db.stats().expect("stats").sessions, 1, "正在錄的那一場留著");

        // 他按停止。那一刻那一場第一次真的可以被判定，而它是空的。
        db.end_session(s).expect("end");
        assert_eq!(
            db.stats().expect("stats").sessions,
            0,
            "紀錄本身要跟著消失——留著它就等於留著一份「他那天在電腦前四小時」的證明"
        );
        assert!(
            blind_spots(&db, nowhere(), "電話")
                .expect("blind")
                .ever_recorded,
            "而那之後「她錄過」這件事還是要答得出來——不然畫面會叫他重跑一次 record"
        );
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
        let b = blind_spots(&db, nowhere(), "電話").expect("blind");
        assert_eq!(b.chunks, 0, "一個字都沒有");
        assert_eq!(b.frames, 3, "但她確實看過三張");
        assert!(b.any());
    }

    /// **上面那條測試守著一個真的 recorder 產生不出來的狀態。**
    ///
    /// 它只呼叫 `insert_frame`，從不呼叫 `insert_focus`。可是真的 recorder
    /// 每次換視窗都會叫後者，而後者順手把視窗標題和網址寫進 `text_chunks`
    /// ——兩種都不經過 OCR。於是一台 OCR 全死的機器上 `chunks` 不是 0，
    /// 「她看過 N 張畫面，但一個字都沒讀出來」那一支永遠到不了。
    ///
    /// 那台機器拿到的是最後一句「她記的每一段裡都沒有這個字」，和一台一切
    /// 正常、那件事真的沒發生過的機器一模一樣。而它是**唯一**一種「錄製照
    /// 跑、畫面照留、搜尋永遠是空的」的壞法，也就是使用者最不可能自己看出來
    /// 的那一種。
    #[test]
    fn a_machine_whose_ocr_is_dead_still_writes_window_titles_and_that_hid_the_whole_diagnosis() {
        let mut db = Db::open_in_memory().expect("db");
        let s = db.start_session("test", "0.0.1").expect("session");
        let focus = FocusSnapshot {
            app_id: Some("chrome.exe".into()),
            app_name: Some("Chrome".into()),
            window_title: Some("網路銀行".into()),
            url: None,
            pid: Some(42),
            password_field: false,
        };
        // 十二張畫面、一行字都沒讀出來（`ocr` 空的），外加真的 recorder 一定
        // 會寫的那幾筆 focus。
        for i in 0..12i64 {
            db.insert_frame(
                s,
                &FrameCapture {
                    ts: 1_000 + i * 1_000,
                    monitor: 0,
                    width: 1920,
                    height: 1080,
                    dhash: 0xDEAD_0000 + i as u64,
                    image: None,
                    image_ext: "webp",
                    ocr: Vec::new(),
                    focus: focus.clone(),
                },
                Some("/tmp/x.webp"),
                1024,
            )
            .expect("insert");
            db.insert_focus(
                s,
                &crate::model::FocusEvent {
                    ts: 1_000 + i * 1_000,
                    kind: crate::model::FocusKind::TitleChange,
                    snapshot: FocusSnapshot {
                        window_title: Some(format!("網路銀行 — 分頁 {i}")),
                        ..focus.clone()
                    },
                },
            )
            .expect("focus");
        }

        let b = blind_spots(&db, nowhere(), "電話").expect("blind");
        assert!(
            b.chunks > 0,
            "視窗標題照樣進得去 text_chunks——這正是舊條件失效的原因"
        );
        assert_eq!(b.ocr_blocks, 0, "而 OCR 一行都沒讀出來");
        assert!(
            b.ocr_is_dead(),
            "這台機器唯一的正確診斷，一定要說得出口：{b:?}"
        );
        assert!(b.any(), "說得出口，畫面上才會有那一行");
    }

    /// 剛按下開始記錄、螢幕上正好沒有字的那幾秒，不可以被指控 OCR 壞了。
    ///
    /// 一則假警報會叫他去跑 `doctor`，然後 `doctor` 會說一切正常——而下一次
    /// 真的壞掉的時候，他已經學會忽略這句話了。
    #[test]
    fn three_blank_screens_are_not_enough_to_accuse_the_engine() {
        let mut db = Db::open_in_memory().expect("db");
        let s = db.start_session("test", "0.0.1").expect("session");
        for i in 0..3i64 {
            db.insert_frame(
                s,
                &FrameCapture {
                    ts: 1_000 + i * 1_000,
                    monitor: 0,
                    width: 1920,
                    height: 1080,
                    dhash: 0xBEEF_0000 + i as u64,
                    image: None,
                    image_ext: "webp",
                    ocr: Vec::new(),
                    focus: FocusSnapshot::default(),
                },
                None,
                0,
            )
            .expect("insert");
        }
        let b = blind_spots(&db, nowhere(), "電話").expect("blind");
        assert_eq!(b.ocr_blocks, 0);
        assert!(!b.ocr_is_dead(), "三張畫面上剛好都沒有字是正常的");
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

        let b = blind_spots(&db, nowhere(), "電話").expect("blind");
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

    /// 自建暫存目錄。不引 `tempfile` 的理由見 `retention.rs`。
    struct Tmp(std::path::PathBuf);
    impl Tmp {
        fn new(name: &str) -> Self {
            use std::sync::atomic::{AtomicU32, Ordering};
            static N: AtomicU32 = AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!(
                "sister-blind-{}-{name}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("temp dir");
            Self(dir)
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// 資料庫裡掛著一段沒收尾的暫停，**而她現在並沒有閉著眼睛**。
    ///
    /// 走到這裡的路很平常：錄製當中按暫停 → 關掉 `sister record` → 隔天才
    /// 解除。解除的那一刻沒有人在跑 recorder，`CaptureResumed` 就沒有人寫，
    /// 那一段從此永遠配不到對。把它講成「她此刻就是閉著眼睛的」，是一則
    /// 再也不會消失的假警報——而假警報會連坐旁邊那則真的一起被忽略。
    #[test]
    fn a_pause_nobody_closed_is_not_the_same_as_being_blind_right_now() {
        let tmp = Tmp::new("dangling");
        let mut db = Db::open_in_memory().expect("db");
        let s = db.start_session("test", "0.0.1").expect("session");
        db.insert_system(
            s,
            &SystemEvent {
                ts: 4_000,
                kind: SystemKind::CapturePaused,
                detail: None,
            },
        )
        .expect("pause");
        // 旗標**不在**：他早就解除了，只是解除的時候沒有人在錄。
        assert!(!crate::pause::is_paused(&tmp.0), "這條測試的前提");

        let b = blind_spots(&db, &tmp.0, "電話").expect("blind");
        assert!(b.paused_open, "紀錄裡那一段確實沒收尾");
        assert!(
            !b.paused_now,
            "但旗標不在——說她現在閉著眼睛就是一句永遠不會過期的假話"
        );
    }

    /// 她三秒前才按下「開始記錄」，不該被告知他的紀錄被忘掉了。
    ///
    /// `ever_recorded && chunks == 0` 有兩種：資料被清掉了，或者這一場才剛
    /// 開始、第一段字還沒寫進去。以前兩種都印「被忘掉了，或是過了保留期」
    /// ——而第一次用的人問的第一個問題正好落在這裡。
    ///
    /// 判斷放在 `BlindSpots` 而不是各自算：這句話終端機和字母人各印一份，
    /// 而「同一句話在兩個地方得到兩種答案」是這個專案反覆踩到的坑。
    #[test]
    fn a_recorder_that_just_started_has_not_forgotten_anything() {
        let tmp = Tmp::new("just-started");
        let mut db = Db::open_in_memory().expect("db");
        db.start_session("test", "0.0.1").expect("session");

        let cold = blind_spots(&db, &tmp.0, "電話").expect("blind");
        assert_eq!(cold.chunks, 0);
        assert!(cold.ever_recorded);
        assert!(
            !cold.recording_now,
            "沒有心跳檔——沒有人在錄，那句「被忘掉了」是完整的"
        );

        // 心跳寫下去（`Phase::Recording`，不是 booting——開機中的她一個字都
        // 還沒記，那和「錄了但什麼都沒有」是兩回事）。
        crate::heartbeat::beat(&tmp.0, crate::now_ms()).expect("beat");
        let hot = blind_spots(&db, &tmp.0, "電話").expect("blind");
        assert!(
            hot.recording_now,
            "她正開著，那句話就少了一種可能——而那是最常見的那一種"
        );

        // 開機中的不算。她還沒開始記，講「我正開著但手上沒東西」會讓他以為
        // 已經在跑了。
        crate::heartbeat::beat_booting(&tmp.0, crate::now_ms()).expect("booting");
        assert!(
            !blind_spots(&db, &tmp.0, "電話")
                .expect("blind")
                .recording_now
        );
    }

    /// 反過來：旗標在，紀錄裡卻一個字都沒有。
    ///
    /// 他在沒有 recorder 在跑的時候按下暫停，所以沒有任何一筆事件記得這件事。
    /// 而下一次 `sister record` 會開起來、然後什麼都不記——這一條比上一條更
    /// 需要講出來，因為它講的是**接下來**會發生什麼。
    #[test]
    fn the_flag_is_on_even_though_no_recorder_ever_wrote_it_down() {
        let tmp = Tmp::new("flag-only");
        crate::pause::set_paused(&tmp.0, true, 1_234).expect("按下暫停");
        let db = Db::open_in_memory().expect("db");

        let b = blind_spots(&db, &tmp.0, "電話").expect("blind");
        assert!(!b.paused_open, "紀錄裡沒有這件事");
        assert_eq!(b.paused_episodes, 0);
        assert!(b.paused_now, "但她現在就是暫停的");
        assert!(b.any(), "而這是一個他可以馬上動手處理的理由");
    }

    /// 一個字的查詢只翻得到最近 30 天，而 `text_days` 預設 365。
    ///
    /// 差 12 倍。少了這一條，用滿一年的人查一個字查不到，得到的是「她記的
    /// 每一段裡都沒有這個字」——那句話把十二分之一講成了全部。
    ///
    /// 界線本身怎麼算，由 `db.rs` 那條測試守著；這裡守的是它有沒有一路
    /// 接到「為什麼沒找到」這份清單上。
    #[test]
    fn a_single_character_question_carries_its_thirty_day_horizon() {
        let mut db = Db::open_in_memory().expect("db");
        let s = db.start_session("test", "0.0.1").expect("session");
        let day = 86_400_000i64;
        // 跨度 200 天：遠遠超過掃描的 30 天窗。
        for (i, ts) in [1_000i64, 1_000 + 200 * day].into_iter().enumerate() {
            db.insert_frame(
                s,
                &FrameCapture {
                    ts,
                    monitor: 0,
                    width: 1920,
                    height: 1080,
                    dhash: 0xBEEF_0000 + i as u64,
                    image: None,
                    image_ext: "webp",
                    ocr: vec![crate::model::OcrBlock {
                        text: format!("第{i}段 客服專線 0800"),
                        x: 0,
                        y: 0,
                        w: 400,
                        h: 18,
                        confidence: 0.95,
                    }],
                    focus: FocusSnapshot {
                        app_id: Some("chrome.exe".into()),
                        app_name: Some("Chrome".into()),
                        window_title: Some("客服系統".into()),
                        url: None,
                        pid: Some(42),
                        password_field: false,
                    },
                },
                None,
                0,
            )
            .expect("insert");
        }

        let b = blind_spots(&db, nowhere(), "錢").expect("blind");
        assert_eq!(
            b.scan_horizon_days,
            Some(30),
            "單一個中文字產不出相鄰雙字，只剩那條夾在 30 天內的掃描"
        );
        assert!(b.any(), "「我只翻了三十天」本身就是一個查得到的理由");

        let two = blind_spots(&db, nowhere(), "客服").expect("blind");
        assert_eq!(
            two.scan_horizon_days, None,
            "兩個字走得到 bigram 索引，沒有時間界線——這時候不該多講一句"
        );

        // 這一條擋的是上面那個欄位的**反面**：一次真的、完整的「沒有」被
        // 降級成「我只讀了十二分之一」。判斷式以前是
        // `bigram_query(query).is_some()`，而純英數的查詢一個相鄰 CJK 雙字
        // 都產不出來——於是每一個錯誤碼、檔名、網址片段的查詢都會附上一句
        // 「只能掃最近 30 天」，而 trigram 索引明明蓋了整張表。使用者對這
        // 兩句話的反應是相反的：一句是「那就是沒發生」，一句是「再翻遠一點」。
        for q in ["ERR_CONNECTION_REFUSED", "invoice.pdf", "0800"] {
            let b = blind_spots(&db, nowhere(), q).expect("blind");
            assert_eq!(
                b.scan_horizon_days, None,
                "「{q}」由 trigram 蓋著整張表，那個「沒有」是完整的"
            );
        }

        // 但真的短到索引比不出來的，還是要說。兩個以內的英數和一個中文字
        // 同一條路：整個詞比得到（unicode61），藏在別的詞裡面的比不到，而
        // 後者只掃得動 30 天。
        let short = blind_spots(&db, nowhere(), "80").expect("blind");
        assert_eq!(
            short.scan_horizon_days,
            Some(30),
            "兩個字元的英數 trigram 比不出來（`80` 藏在 `0800` 裡就是這種）"
        );
    }
}
