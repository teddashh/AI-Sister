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
    ClipboardEvent, EndReason, FocusEvent, FocusKind, FocusSnapshot, FrameCapture, Millis,
    SystemEvent, SystemKind,
};
use sister_core::redact;

use crate::traits::{Backend, RawFrame};

/// 一天有多少毫秒。畫面額度以 UTC 天為單位重置，和
/// `frames::relative_path` 的資料夾分層是同一條線。
const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// 一次 tick 的結果。呼叫端據此決定要不要記錄、要不要退避。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tick {
    /// 總開關關閉——她閉著眼睛。設定檔說的，重開才會變。
    Disabled,
    /// 使用者按了暫停。和 `Disabled` 差在這是**當下**、可以隨時解除的，
    /// 而且進出各留一筆 system event，所以資料裡那個空洞解釋得出來。
    Paused,
    /// 被排除規則擋下。畫面沒有被抓取。
    Excluded { reason: String },
    /// 這一刻沒有可用畫面（鎖屏、顯示器休眠）。
    NoScreen,
    /// 沒有人碰過鍵盤滑鼠、焦點也沒變，所以**這一次連螢幕都沒看**。
    ///
    /// 和 `Duplicate` 差在哪裡很重要：`Duplicate` 是看過了、確認一樣；
    /// 這個是沒看。省下來的正是最貴的那一步，代價是「畫面自己會動」的
    /// 東西（影片、進度條）最多會晚 `MAX_BLIND_MS` 才被看到。
    Idle,
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
    /// 真的走完一整拍的次數——過了 `capture.enabled` 和暫停這兩道門的。
    ///
    /// `ticks` 在那兩道門**之前**就加了，所以一段「8 小時裡暫停了 7 小時」的
    /// 錄製，72,000 拍裡有 63,000 拍是幾微秒就回來的空轉。拿 `ticks` 當分母
    /// 算「每 tick 幾 ms」會得到 8 ms，而真的做事的那一拍要 60 ms——差 7.5
    /// 倍，而且是往「看起來很便宜」的方向差。那個數字唯一的用途就是判斷這
    /// 個迴圈貴不貴，指錯方向等於沒有。
    pub working_ticks: u64,
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
    /// 因為沒有人動、而**完全沒有碰螢幕**的 tick 數。
    ///
    /// 這個數字一定要印出來。它代表「她這段時間是閉著眼睛的」，而那件事
    /// 從摘要的其他任何一個數字上都看不出來——省電和停止工作在帳面上長得
    /// 一模一樣，正是 alpha.4 那種「✓ 但什麼都沒產出」的失效形狀。
    pub skipped_idle: u64,
    /// 這一拍**走到了省電閘門**的次數。
    ///
    /// 摘要裡那句「省下：0 次沒碰螢幕（這段時間你一直在動，或每一拍脈絡都
    /// 變了）」需要它。那句話以前的條件是 `working_ticks > 0`，而閘門在
    /// tick 的第 5 步——被排除擋掉的、以及在第 3 到第 4 步就 `?` 出去的，
    /// 統統沒走到那裡。於是一場「資料庫寫不進去、每一拍都炸掉」的錄製，
    /// 摘要主動解釋說「這段時間你一直在動」——一句它沒有任何依據的話，
    /// 而且是那份摘要裡唯一的解釋。
    ///
    /// 脈絡變了那一種也算「問到了」：那時候不問 `idle_ms` 是因為答案已經
    /// 確定（睜眼），而那正是那句話後半段「或每一拍脈絡都變了」講的事。
    pub idle_asked: u64,
    /// 問了「有人動過嗎」但這台機器答不出來的次數。
    ///
    /// 沒有這個數字的話，`skipped_idle == 0` 有三種完全不同的意思，而它們
    /// 印出來一模一樣：他真的整天都在打字、這個平台根本沒有閒置訊號、
    /// 閘門被別的東西關掉了。中間那個是**閘門永遠不會生效**，也就是 CPU
    /// 預算從第一秒起就超支，而摘要上一個字都不會提。
    pub idle_unknown: u64,
    /// 焦點停在**瀏覽器視窗**上的拍數（見 [`crate::browsers::is_browser`]）。
    ///
    /// 這是 [`Self::url_reads`] 的分母，也是那句話的證據門檻。單獨看沒有用；
    /// 它存在的唯一理由是讓「一個網址都沒讀到」這件事有辦法被判斷——一個
    /// 今天還沒開過瀏覽器的人，讀到 0 個網址什麼都不代表。
    pub browser_ticks: u64,
    /// 其中真的拿到網址的拍數。
    ///
    /// `capabilities` 那份報告的 `url` 只回答「UIA 的 COM 物件造得出來」，
    /// 而那和「讀得到位址列」之間差著一整台機器。兩者的差距整個是安靜的：
    /// 造得出來、`doctor` 全綠、設定頁一片乾淨，而位址列一次都沒讀到
    /// ——於是那台機器把使用者的網銀錄了一整天，他寫的每一條 `excluded_urls`
    /// 一次都沒擋過東西。這個數字是那件事唯一的證據。
    pub url_reads: u64,
    /// 標題在變、但那個標題是時鐘，所以沒有記成脈絡變化的次數。
    ///
    /// 見 `TITLE_CLOCK_MIN_TICKS`。這個數字要印出來，因為它同時解釋兩件
    /// 使用者會覺得奇怪的事：時間軸上那個視窗少了很多列，以及 CPU 為什麼
    /// 突然從 27% 掉下來。
    pub title_clock_ticks: u64,
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
    /// 上面那個數字**橫跨了幾天**。
    ///
    /// 跨日只把 `image_bytes_today` 歸零，`images_over_budget` 是整場的累計。
    /// 於是一場連錄五天、每天都撞到上限的 session，摘要會說「**今天**的畫面
    /// 額度用完了，之後的 4200 張只留了字」——那個 4200 是五天的總數，而
    /// 「今天」是假的。這個產品的預設用法就是開著不關，所以那不是邊角情況。
    ///
    /// 數的是**撞到上限的那幾天**，不是 session 活過幾天：沒撞到的那幾天不
    /// 該被算進一句在講額度的話裡。
    pub images_over_budget_days: u64,
    /// 想存圖卻失敗了幾次（磁碟滿、資料夾沒權限、路徑被佔用）。
    ///
    /// 和 `ocr_failures` 同一個理由：吞掉錯誤可以，不留計數不行。少了它，
    /// 「錄了一整天、一張圖都沒有」和「一切正常」在摘要上長得一模一樣
    /// ——摘要甚至會因為每個計數都是 0 而整段不印。
    pub image_failures: u64,
    /// 最後一次存圖失敗的原因。一句原文遠比一個數字有用。
    pub last_image_error: Option<String>,
    /// OCR 一共讀出幾行字。
    ///
    /// 「保留了 12 張畫面」和「記住了 12 張畫面上的字」是兩回事，而摘要
    /// 只印前者的話，兩者看起來一模一樣。實測踩過：12 張畫面、0 行文字、
    /// 摘要一片祥和，要等到搜尋永遠是空的才會發現。
    pub ocr_blocks: u64,
    /// **整拍**失敗了幾次（`tick` 帶著 `Err` 離開）。
    ///
    /// 錄製迴圈把單次 tick 的錯誤吞成一行 `tracing::warn!` 就繼續跑，理由是
    /// 對的：抓不到畫面多半是暫時的（切使用者、螢幕休眠），下一秒就好了。
    /// 錯的是**沒有留計數**——而 `record.log` 在 `%APPDATA%` 深處，會去翻它
    /// 的人已經知道出事了。
    ///
    /// 沒有這個數字的話，一場每一拍都炸掉的錄製印出來是
    ///
    /// ```text
    /// 完成：7200 tick → 保留 0、重複 0、排除 0、無畫面 0
    /// 省下：0 次沒碰螢幕（這段時間你一直在動，或每一拍脈絡都變了）
    /// ```
    ///
    /// ——和一場「腳本裡本來就沒東西」一模一樣的五個數字，加上一句主動編出
    /// 來的解釋。而 `ocr_failures`、`image_failures` 早就各自有計數，理由
    /// 逐字相同：吞掉錯誤可以，不留計數不行。這一條是那條規則上最大的漏洞
    /// ——它蓋住的不是一種訊號，是整拍。
    pub tick_failures: u64,
    /// 最後一次整拍失敗的原因。一句原文遠比一個數字有用。
    pub last_tick_error: Option<String>,
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

/// 就算沒有人碰任何東西，最多也只能連續多久不看螢幕。
///
/// 沒有這個上限的話，「沒有輸入 ⇒ 畫面沒變」就從一個很好的猜測變成一個
/// 錯的斷言：影片、進度條、跑動的 log、別人傳進來的訊息，全都會在她閉著
/// 眼睛的時候發生。
///
/// **它同時是這個設計的 CPU 地板。** 完全沒人碰的一天也要 86400/5 = 17,280
/// 次睜眼，而那筆錢跟一次抓圖多貴成正比：CI runner 的 1024×768 一次 27 ms
/// ⇒ 0.5%；他那台 2560×1440 一次 127 ms ⇒ **2.5%**，佔掉 3% 預算的八成半，
/// 而那還是一台**沒有人在用**的機器。用起來的時候閘門本來就該是開的（有輸入
/// 就代表畫面可能變了），所以省電閘門修得再好都到不了預算——真正要便宜的是
/// 「一次睜眼」本身。`sister bench` 就是為了問這個。
///
/// 公開是因為報告要拿它算那個地板：一個寫死在兩個地方的 5 秒，遲早會有一邊
/// 先改。
pub const MAX_BLIND_MS: i64 = 5_000;

/// 要看過幾拍，才有資格說「這個視窗的標題是一個時鐘」。
///
/// 「標題變了 ⇒ 畫面幾乎一定變了」對**一次**標題變化是對的。對一個每拍都
/// 在變的標題就完全相反：那不是事件，是時鐘，而它會把上面那個省錢的閘門
/// 永久關掉——`context_changed` 每拍都是 `true`，`idle_ms()` 一次都問不到，
/// 她整天睜著眼睛，一天 216,000 次全解析度抓圖。
///
/// 這個數字要夠大，大到「一半」有統計意義；又要夠小，小到幾秒內就判得出
/// 來。預設 400 ms 一拍的話，12 拍大約是 4.8 秒。
const TITLE_CLOCK_MIN_TICKS: u32 = 12;

pub struct Recorder<B: Backend> {
    backend: B,
    db: Db,
    config: Config,
    deduper: Deduper,
    session_id: i64,
    /// 最後一張被保留的幀，重複時往它身上加計數。
    last_frame_id: Option<i64>,
    last_focus: Option<FocusSnapshot>,
    /// 目前這個視窗拿到焦點之後過了幾拍，以及其中幾拍標題變了。
    ///
    /// 兩個一起看才答得出「這個標題是事件還是時鐘」。只數連續次數不夠：
    /// 一個一秒跳一次的時鐘配 400 ms 的 tick，連續次數永遠停在 1，而閘門
    /// 已經有 40% 的時間被關掉了。換成比例就抓得到。
    ///
    /// 兩個都在換 app 或換網址時歸零——那時候標題本來就該變，不算churn。
    focus_ticks: u32,
    title_changes: u32,
    /// 上一次的排除理由。只有在理由改變時才寫 system event，
    /// 否則被排除的一小時會產生上千筆一模一樣的紀錄。
    last_exclusion: Option<String>,
    /// 使用者按下的暫停。由呼叫端每個 tick 餵進來（見 `set_paused`），
    /// **不是** recorder 自己去讀檔案——這一層讀了檔案，replay 就不再是
    /// 確定性的了，而確定性是這個迴圈唯一的測試手段。
    paused: bool,
    /// 畫面檔的根目錄。`None` = text-only 模式。
    image_dir: Option<PathBuf>,
    /// 上一次**真的寫出**畫面檔的時刻。見 `image_min_interval_ms`。
    last_image_ts: Option<Millis>,
    /// 上一次真的去看螢幕的時刻（不管結果是新是舊）。空閒跳過的天花板
    /// 就是從這裡算的。
    last_look_ts: Option<Millis>,
    /// `image_bytes_today` 算的是哪一天（UTC 天序號）。
    image_day: i64,
    /// 今天已經寫出去多少畫面位元組。見 `max_image_mb_per_day`。
    image_bytes_today: u64,
    /// 最後一次撞到每日上限是哪一天。見 `RecorderStats::images_over_budget_days`。
    over_budget_day: Option<i64>,
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
        //
        // 問不出來時也**不可以**當成 0：那同樣是重新發一整份額度，只是這次
        // 連使用者都不知道發生過。開不起來比較誠實——資料庫壞到連一句 SUM
        // 都算不出來，本來就不該繼續往裡面寫。
        let now = sister_core::now_ms();
        let image_day = now.div_euclid(DAY_MS);
        let image_bytes_today = db
            .image_bytes_since(image_day * DAY_MS)
            .context("今天用掉多少畫面額度，問不出來")?;

        Ok(Self {
            backend,
            db,
            config,
            deduper,
            session_id,
            last_frame_id: None,
            last_focus: None,
            focus_ticks: 0,
            title_changes: 0,
            last_exclusion: None,
            paused: false,
            image_dir,
            last_image_ts: None,
            last_look_ts: None,
            image_day,
            image_bytes_today,
            over_budget_day: None,
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

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// 換掉隱私規則，不用重開行程。
    ///
    /// 這是設定檔熱重載的落點。沒有它，使用者在設定頁（或直接改 TOML）加一條
    /// 排除規則之後，正在跑的 `record` 會**完全照舊**——而且不會有任何一行字
    /// 提到這件事。那是這個 repo 一路在抓的同一種失效：看起來做了，其實沒有。
    ///
    /// 只換隱私那一段。截圖間隔、去重門檻那些改了仍然要重開，因為它們牽涉到
    /// recorder 自己累積的狀態（deduper、每日額度、剪貼簿水位），中途換掉會
    /// 生出一堆說不清楚的邊界情況——而它們改錯的後果是「錄得比較醜」，
    /// 不是「錄到了不該錄的東西」。優先權差很遠。
    /// 現在真的在用的那一份隱私規則。
    ///
    /// 有 [`Self::set_privacy`] 就得有這個：收工的摘要要講「你那幾條規則失效
    /// 了」，而規則數必須來自**現在**這一份。拿開機時那一份去數的話，中途在
    /// 設定頁上新加的規則不會被算進去——而一個人剛加完規則、正想確認它有沒有
    /// 生效，就是最需要那句話說對的時刻。
    pub fn privacy(&self) -> &sister_core::config::PrivacyConfig {
        &self.config.privacy
    }

    pub fn set_privacy(&mut self, privacy: sister_core::config::PrivacyConfig) {
        self.config.privacy = privacy;
        // 規則換了，「上一次的排除理由」就不能再拿來去抖：舊的理由字串可能
        // 已經不存在，而下一段排除該留下屬於它自己的稽核紀錄。
        self.last_exclusion = None;
    }

    /// 中途改成只留字（或改回來）。
    ///
    /// 這是第三張同意書的落點。它跟上面那幾項「改了要重開」的設定不一樣，
    /// 因為撤回同意的人不能等到下次重開——PRIVACY.md 上寫的是「隨時撤得掉」。
    ///
    /// 換成 `None` 只是**不再寫新的圖**；已經寫在磁碟上的那些不歸這裡管
    /// （那是保留期和「忘掉某一段」的工作）。中途換不會弄髒任何累積狀態：
    /// 每日額度照樣往上加，因為它算的是今天寫出去多少位元組，而不寫就是不加。
    pub fn set_image_dir(&mut self, dir: Option<PathBuf>) {
        self.image_dir = dir;
    }

    /// 她現在會不會把圖寫下來。
    pub fn stores_images(&self) -> bool {
        self.image_dir.is_some()
    }

    /// 餵進「使用者現在有沒有按暫停」。呼叫端每個 tick 呼叫一次即可——
    /// 沒有變化時什麼都不做，所以它可以無腦地一直呼叫。
    ///
    /// 只有**轉換**才寫事件，理由和排除那邊一樣：暫停三小時不該產生上萬筆
    /// 一模一樣的紀錄。但那兩筆轉換非寫不可——沒有它們，資料裡的空洞和
    /// 「那段時間什麼都沒發生」在事後完全分不出來，而這兩件事的意思差很遠。
    ///
    /// 回傳「這一次有沒有真的改變狀態」，讓呼叫端決定要不要對使用者說一句。
    pub fn set_paused(&mut self, paused: bool, ts: Millis) -> Result<bool> {
        if self.paused == paused {
            return Ok(false);
        }
        self.paused = paused;
        self.db.insert_system(
            self.session_id,
            &SystemEvent {
                ts,
                kind: if paused {
                    SystemKind::CapturePaused
                } else {
                    SystemKind::CaptureResumed
                },
                detail: None,
            },
        )?;
        if paused {
            // 暫停期間畫面沒被看過，所以解除之後的第一張必須當成全新的。
            // 少了這兩行，使用者暫停時換掉整個畫面、回來以後那張新畫面會被
            // 判定成「和上一張一樣」而整個消失。
            self.deduper.reset();
            self.last_frame_id = None;
        }
        Ok(true)
    }

    /// 收尾：標記 session 結束，並且記下**為什麼**。
    ///
    /// 理由是必填的參數，不是 `Option`：這一步只有四條路走得到（見
    /// [`EndReason`]），而讓呼叫端有辦法說「不知道」，等於保證某一條路上
    /// 遲早會沒有人填。
    pub fn finish(&mut self, reason: EndReason) -> Result<()> {
        let ts = sister_core::now_ms();
        self.db.insert_system(
            self.session_id,
            &SystemEvent {
                ts,
                kind: SystemKind::SessionEnd,
                detail: Some(reason.as_str().to_string()),
            },
        )?;
        self.db.end_session(self.session_id)?;
        Ok(())
    }

    /// 走一輪感官。`ts` 由呼叫端給定，因此 replay 完全確定性。
    ///
    /// 這一層只做一件事：量整個 tick 有多久。它必須包住**每一條**離開的
    /// 路徑（含被排除、沒畫面、以及 `?` 帶出去的錯誤），否則「沒歸因到的
    /// 時間」那個數字會偏向好看的方向——而那個數字唯一的用途就是抓自己說謊。
    ///
    /// 整拍失敗也在這裡記帳（見 [`RecorderStats::tick_failures`]）。**記在這
    /// 裡而不是呼叫端**：呼叫端有兩個（`record` 的迴圈、`replay`），而一個
    /// 要每個呼叫端記得去加的計數器，就是這個專案一路在修的那種「安靜地不
    /// 生效」——第三個呼叫端長出來的那天不會有人記得，而症狀是摘要少講一
    /// 件事，沒有任何測試會紅。
    pub fn tick(&mut self, ts: Millis) -> Result<Tick> {
        let t = Instant::now();
        let out = self.tick_inner(ts);
        self.timings.tick.record(t.elapsed());
        if let Err(e) = &out {
            self.stats.tick_failures += 1;
            // `{e:#}` 帶上整條 anyhow context 鏈，和 OCR / 存圖那兩個一樣。
            self.stats.last_tick_error = Some(format!("{e:#}"));
        }
        out
    }

    fn tick_inner(&mut self, ts: Millis) -> Result<Tick> {
        self.stats.ticks += 1;

        if !self.config.capture.enabled {
            return Ok(Tick::Disabled);
        }

        if self.paused {
            // 和排除同一個理由，而且更嚴重：使用者是**故意**在這段時間裡去複製
            // 一個他不想被記住的東西。水位不推過去的話，他一解除暫停，那個東西
            // 就在下一個 tick 被撈進來——暫停等於只是延後了洩漏。
            //
            // `skip` 不讀內容，只把水位推過去，所以這一行讓她更看不到、
            // 不是更看得到。
            self.backend.skip_clipboard(ts);
            // 排除那邊會繼續累積輸入節奏（節奏不含內容，斷了會在訊號上開洞）。
            // 這裡**不**累積：排除講的是「這個畫面不能看」，暫停講的是
            // 「現在不要記錄我」——後者連沒有內容的節奏都不該留下。
            return Ok(Tick::Paused);
        }
        // 過了上面兩道門才算「這一拍真的要做事」。摘要裡的每拍成本和閒置
        // 比例都拿它當分母——見 `working_ticks`。
        self.stats.working_ticks += 1;

        // 1) 先看脈絡。這一步很便宜，而且是排除判定的依據。
        let t = Instant::now();
        let focus = self.backend.focus_snapshot(ts).unwrap_or_default();
        self.timings.focus.record(t.elapsed());

        // 網址擷取到底有沒有在運作，只有這裡看得到。數在排除判定**之前**：
        // 一個被 `excluded_apps` 擋掉的瀏覽器同樣證明了 UIA 讀不讀得到，
        // 而且往下走的路上這個 snapshot 就不見了。
        if crate::browsers::is_browser(&focus.app_key()) {
            self.stats.browser_ticks += 1;
            if focus.url.is_some() {
                self.stats.url_reads += 1;
            }
        }

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
        let context_changed = self.record_focus_if_changed(ts, &focus)?;

        // 4) 剪貼簿。只有在沒被排除時才碰——不然密碼管理員裡複製的
        //    密碼會從這裡漏進資料庫。
        self.record_clipboard(ts, &focus)?;

        self.record_input(ts)?;

        // 5) 先問一個不用碰螢幕就答得出來的問題：有人動過嗎？
        //
        //    這一步是量出來的，不是想出來的。真 Windows、release、三次獨立
        //    量測都指向同一件事：一次擷取的成本幾乎完全由「讀了多少來源
        //    像素」決定，跟你要縮到多小**沒有關係**——
        //
        //        原生 1024x768   24.2 ms      256x192（面積平均） 22.1 ms
        //        512x384         20.9 ms      256x192（丟像素）   19.5 ms
        //
        //    目的地像素差 12 倍，時間差不到 25%。所以想省錢只有一條路：
        //    **不要讀**。沒有人碰過鍵盤滑鼠、焦點也沒變，畫面就多半沒變，
        //    而問這件事是一次系統呼叫，0.0 ms。
        //
        //    「多半」不是「一定」，所以下面有 `MAX_BLIND_MS` 這個天花板：
        //    「不知道就擋住」必須有上限，反過來「猜沒變就不看」也一樣。
        //
        //    換了視窗、換了分頁、標題變了 → 畫面幾乎不可能沒變，直接睜眼。
        //    這一條不是為了效能，是為了收窄那個 5 秒的盲區：通知搶焦點、
        //    安裝程式跳出來這類事情不需要任何輸入，但都會動到脈絡。
        // 走到這裡就算「閘門問到了」。摘要那句「你一直在動」只有在這個數字
        // 大於 0 的時候才有依據——見 `RecorderStats::idle_asked`。
        self.stats.idle_asked += 1;
        let idle_signal = if context_changed {
            None
        } else {
            let asked = self.backend.idle_ms();
            if asked.is_none() {
                self.stats.idle_unknown += 1;
            }
            asked
        };
        // 時鐘往回跳（NTP 校時、使用者改時間）會讓 `ts - last_look` 變負。
        // 用 `saturating_sub` 夾成 0 的話，「距離上次看過了 0 毫秒」永遠
        // 小於天花板，而 `last_look_ts` 又只在真的看的時候才更新——她會
        // **從此再也不看螢幕**，而且 tick 照跑、CPU 漂亮、沒有任何錯誤。
        // 這是這個專案最典型的失效形狀，所以往回跳就直接把基準丟掉。
        if self.last_look_ts.is_some_and(|prev| ts < prev) {
            self.last_look_ts = None;
        }
        if let (Some(idle), Some(last_look)) = (idle_signal, self.last_look_ts) {
            let since_look = ts - last_look;
            // idle >= since_look 的意思是：從上次看螢幕到現在，沒有任何輸入。
            if idle as i64 >= since_look && since_look < MAX_BLIND_MS {
                self.stats.skipped_idle += 1;
                return Ok(Tick::Idle);
            }
        }
        self.last_look_ts = Some(ts);

        // 6) 讀一次螢幕。**只讀一次。**
        //
        //    這裡本來有兩次：先抓一張 256px 的「探測圖」算雜湊，變了才付
        //    原生解析度的錢。那個設計來自一個算得很漂亮的推論——要搬的
        //    位元組從 14MB 掉到 147KB——而它整整活了兩個版本，因為沒有人
        //    去量。量了之後：探測 43.1 ms、它想省的那次抓圖 33.4 ms。
        //
        //    原因上面那張表已經寫了：成本由**來源**像素決定，而探測圖和
        //    抓圖讀的是同一個螢幕。縮小目的地什麼都沒省，只是把一張讀完
        //    的畫面丟掉，然後再讀一次。
        //
        //    所以現在一次讀到底，dhash 直接從這張算。少一次擷取、少一個
        //    「兩張圖的雜湊必須一致」的隱性約定，也少一整個 `probe` 概念。
        let t = Instant::now();
        let grabbed = self.backend.grab_screen(ts);
        self.timings.grab.record(t.elapsed());

        // 這條路上的每一個退出點都不必手動退回去重基準：`check` 不會推進
        // 它，推進的是 `keep_frame` 裡真的存完之後那一句 `kept`。這裡曾經
        // 有三個地方各自漏掉那個退回動作（鎖屏、抓圖失敗、資料庫寫不進
        // 去），所以問題不在漏了三次，在於形狀。
        let frame = match grabbed {
            Ok(Some(f)) => f,
            Ok(None) => {
                self.stats.no_screen += 1;
                return Ok(Tick::NoScreen);
            }
            Err(e) => return Err(e),
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

        let (image_path, image_bytes) = match self.store_image(ts, &frame) {
            Ok(v) => v,
            Err(e) => {
                // 存不下畫面不該讓文字也跟著遺失——但也不可以只寫進 log
                // 就算了。磁碟滿、資料夾沒權限、路徑被佔用，症狀都是
                // 「錄了一天、一張圖都沒有」，而摘要本來連提都不會提。
                tracing::warn!(error = %e, "failed to store frame image; keeping text only");
                self.stats.image_failures += 1;
                self.stats.last_image_error = Some(format!("{e:#}"));
                (None, 0)
            }
        };
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
        let inserted = self.db.insert_frame(
            self.session_id,
            &capture,
            image_path.as_deref(),
            image_bytes,
        );
        self.timings.db.record(t.elapsed());

        let (frame_id, _chunk, facts) = match inserted {
            Ok(v) => v,
            Err(e) => {
                // 那一列沒成立，這張 PNG 就沒有任何東西指向它了。而
                // `retention::prune` 只走 `image_path IS NOT NULL` 的列，
                // 所以它永遠不會被清掉——「畫面 30 天後刪掉」對它是假的，
                // 而且它還佔著今天的畫面額度。現在就收乾淨。
                self.discard_image(image_path.as_deref(), image_bytes);
                return Err(e);
            }
        };

        // 到這裡才推進去重基準：畫面、文字、那一列都已經落地了。
        // 寫不進去時上面那個 `?` 會直接帶著錯誤離開，而基準原封不動——
        // 下一次同一個畫面回來仍然算新的，這正是我們要的。
        self.deduper.kept(frame.dhash);
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

        // 時鐘往回跳的話，「距離上一張多久」會是負數，而負數永遠小於間隔
        // ——於是節流會一路擋到時鐘追回來為止。實測一次 8 小時的時區修正
        // （雙系統把 RTC 當本地時間寫、VM 從快照恢復、NTP 校時）就等於
        // 8 小時一張圖都不存，而摘要會說「間隔未到」，那是假的。
        //
        // 往回跳就當作「上一張的時間已經沒有意義了」，直接放行。
        let gap = self.config.capture.image_min_interval_ms as i64;
        match self.last_image_ts {
            Some(prev) if ts < prev => self.last_image_ts = None,
            Some(prev) if ts - prev < gap => {
                self.stats.images_throttled += 1;
                return Ok((None, 0));
            }
            _ => {}
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
            // 這一天是不是第一次撞到。見 `images_over_budget_days`：少了它，
            // 五天的總數會被摘要講成「今天」的。
            if self.over_budget_day != Some(day) {
                self.over_budget_day = Some(day);
                self.stats.images_over_budget_days += 1;
            }
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

    /// 把一張已經寫出去、但那一列沒成立的 PNG 收掉，並退回它佔用的額度。
    ///
    /// 刪不掉就把它留著並記一筆——留一個孤兒檔比留一個假的額度數字好，
    /// 因為前者只是浪費磁碟，後者會讓今天剩下的時間少存好幾百張圖。
    fn discard_image(&mut self, rel: Option<&str>, bytes: i64) {
        let (Some(rel), Some(root)) = (rel, self.image_dir.as_ref()) else {
            return;
        };
        let full = root.join(rel);
        match std::fs::remove_file(&full) {
            Ok(()) => {
                let n = bytes.max(0) as u64;
                self.image_bytes_today = self.image_bytes_today.saturating_sub(n);
                self.stats.image_bytes = self.stats.image_bytes.saturating_sub(n);
            }
            Err(e) => {
                tracing::warn!(error = %e, path = %full.display(), "orphaned frame image");
                self.stats.image_failures += 1;
                self.stats.last_image_error =
                    Some(format!("那一列沒寫成，這張圖也刪不掉：{full:?}（{e}）"));
            }
        }
    }

    /// 這個視窗的標題到底是「事件」還是「時鐘」。
    ///
    /// 拿到焦點之後夠久了，而且有一半以上的拍子標題都在變 → 它是時鐘。
    /// 時鐘走一格不代表畫面上發生了什麼事，所以它不該把省電閘門關掉，也
    /// 不該在 `focus_events` 裡留下一列。
    ///
    /// 會這樣的東西比想像中多：跑 build 的 Windows Terminal、播放器的
    /// 進度、`(3) Slack` 的未讀數、VS Code 的 ●、下載百分比。任何一個
    /// 開著就足以讓她整天不閉眼。
    fn title_is_a_clock(&self) -> bool {
        self.focus_ticks >= TITLE_CLOCK_MIN_TICKS && self.title_changes * 2 >= self.focus_ticks
    }

    /// 回傳「脈絡有沒有變」。呼叫端拿它決定要不要睜眼看螢幕：換了視窗、
    /// 換了分頁、標題變了，畫面幾乎不可能沒變。
    ///
    /// 「標題變了」那一條有例外，見 [`title_is_a_clock`](Self::title_is_a_clock)。
    fn record_focus_if_changed(&mut self, ts: Millis, focus: &FocusSnapshot) -> Result<bool> {
        self.focus_ticks = self.focus_ticks.saturating_add(1);
        let kind = match &self.last_focus {
            None => FocusKind::Focus,
            Some(prev) if prev.app_id != focus.app_id => FocusKind::Focus,
            Some(prev) if prev.url != focus.url && focus.url.is_some() => FocusKind::UrlChange,
            Some(prev) if prev.window_title != focus.window_title => FocusKind::TitleChange,
            Some(_) => return Ok(false),
        };
        if focus.app_id.is_none() && focus.window_title.is_none() && focus.url.is_none() {
            return Ok(false);
        }
        if matches!(kind, FocusKind::TitleChange) {
            self.title_changes = self.title_changes.saturating_add(1);
            if self.title_is_a_clock() {
                // 標題還是要跟上（不然下一拍會拿它和三小時前的比），但這一
                // 格不寫進資料庫、也不算脈絡變化。判定會自己解除：時鐘停了
                // 之後 `focus_ticks` 繼續長而 `title_changes` 不長，比例掉
                // 到一半以下，下一次真的改標題就照常記。
                self.last_focus = Some(focus.clone());
                self.stats.title_clock_ticks += 1;
                return Ok(false);
            }
        } else {
            // 換了 app 或換了網址：重新開始數。新視窗的標題該不該信，
            // 和上一個視窗是不是時鐘沒有關係。
            self.focus_ticks = 1;
            self.title_changes = 0;
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
        Ok(true)
    }

    fn record_clipboard(&mut self, ts: Millis, focus: &FocusSnapshot) -> Result<()> {
        // 每個 tick 都會問一次，而在 Windows 上這是 OpenClipboard →
        // GetClipboardData → CloseClipboard 的跨程序往返，別的程式正抓著
        // 剪貼簿時它會等。所以它必須有自己的一欄，不能混在「其他」裡面。
        let t = Instant::now();
        let polled = self.backend.poll_clipboard(ts);
        self.timings.clipboard.record(t.elapsed());

        let Some(mut event) = polled? else {
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
        let t = Instant::now();
        let drained = self.backend.drain_input(ts);
        self.timings.input.record(t.elapsed());

        if let Some(metrics) = drained? {
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

    /// 暫停期間也要推剪貼簿水位，理由和排除完全一樣。
    ///
    /// 而且這裡更嚴重：使用者按暫停，往往就是**為了**去複製一個他不想被記住
    /// 的東西。只是「不讀」的話，他一解除暫停，那個東西就在下一個 tick 被撈
    /// 進來——一個只會延後洩漏的暫停鍵，比沒有暫停鍵更糟，因為他信了它。
    #[test]
    fn pausing_skips_the_clipboard_instead_of_merely_not_polling_it() {
        use crate::traits::{CompositeBackend, NullFocus, NullInput, NullOcr, NullScreen};

        let spy = std::rc::Rc::new(std::cell::RefCell::new(SpyClipboard::default()));
        let backend = CompositeBackend {
            name: "spy".into(),
            screen: NullScreen,
            focus: NullFocus,
            clipboard: spy.clone(),
            input: NullInput,
            ocr: NullOcr,
        };
        let mut rec = Recorder::new(
            backend,
            Db::open_in_memory().unwrap(),
            Config::default(),
            None,
        )
        .unwrap();

        assert!(rec.set_paused(true, 500).unwrap(), "第一次應該真的改變狀態");
        assert_eq!(rec.tick(1_000).unwrap(), Tick::Paused);

        let spy = spy.borrow();
        assert!(spy.polled.is_empty(), "暫停期間不該讀剪貼簿內容");
        assert_eq!(spy.skipped, vec![1_000], "但一定要把水位推過去");
    }

    /// 一台讀不到位址列的機器，要數得出自己讀不到。
    ///
    /// `capabilities` 那份報告的 `url` 只回答「UIA 的 COM 物件造得出來」。
    /// 造得出來、`doctor` 全綠、設定頁一片乾淨，而位址列從頭到尾一次都沒讀到
    /// ——瀏覽器用系統管理員身分跑、無障礙介面沒開、UIA 樹換了形狀，任何一種
    /// 都會走到這裡。那台機器把使用者的網銀錄了一整天，他寫的每一條
    /// `excluded_urls` 一次都沒擋過東西，而每一個畫面都說一切正常。
    ///
    /// 這裡釘的是那件事唯一的證據：分子分母各自數對，而且**被排除的瀏覽器
    /// 也要算**——它同樣證明了 UIA 讀不讀得到，而排除判定在下一步就把這個
    /// snapshot 帶走了。
    #[test]
    fn a_browser_that_never_yields_a_url_is_counted_as_such() {
        use crate::traits::{CompositeBackend, NullClipboard, NullInput, NullOcr, NullScreen};

        /// 每一拍換一種前景視窗：瀏覽器有網址、瀏覽器沒網址、
        /// 被排除的瀏覽器、根本不是瀏覽器。
        struct Rotating(usize);
        impl crate::traits::FocusSource for Rotating {
            fn snapshot(&mut self, _ts: Millis) -> Result<FocusSnapshot> {
                let i = self.0;
                self.0 += 1;
                Ok(match i % 4 {
                    0 => FocusSnapshot {
                        app_id: Some("chrome.exe".into()),
                        url: Some("https://example.com/".into()),
                        ..Default::default()
                    },
                    1 => FocusSnapshot {
                        app_id: Some("firefox.exe".into()),
                        url: None,
                        ..Default::default()
                    },
                    2 => FocusSnapshot {
                        // 被 `excluded_apps` 擋掉，但它還是一個瀏覽器
                        app_id: Some("brave.exe".into()),
                        url: None,
                        ..Default::default()
                    },
                    _ => FocusSnapshot {
                        app_id: Some("code.exe".into()),
                        url: None,
                        ..Default::default()
                    },
                })
            }
        }

        let mut config = Config::default();
        config.privacy.excluded_apps = vec!["brave".into()];
        let backend = CompositeBackend {
            name: "rotating".into(),
            screen: NullScreen,
            focus: Rotating(0),
            clipboard: NullClipboard,
            input: NullInput,
            ocr: NullOcr,
        };
        let mut rec = Recorder::new(backend, Db::open_in_memory().unwrap(), config, None).unwrap();

        for i in 0..12 {
            rec.tick(1_000 + i * 1_000).expect("tick");
        }

        let s = rec.stats();
        assert_eq!(s.browser_ticks, 9, "12 拍裡有 9 拍是瀏覽器（含被排除的）");
        assert_eq!(s.url_reads, 3, "其中只有 chrome 那 3 拍真的拿到網址");
    }

    /// 整拍炸掉必須留下數字，而且**不能假裝閘門問過了**。
    ///
    /// 錄製迴圈把 tick 的錯誤吞成一行 `tracing::warn!`。那個決定是對的，
    /// 但在它之外一個計數器都沒有的話，一場「每一拍都失敗」的錄製印出來
    /// 和「今天沒開電腦」一模一樣：五個零。這裡用一個永遠壞掉的剪貼簿
    /// 來造那一場——它炸在第 4 步，也就是省電閘門（第 5 步）之前。
    ///
    /// 所以這個測試同時釘住兩件事：`tick_failures` 有在數（在 `tick()`
    /// 裡數，不是在呼叫端），以及 `idle_asked` **沒有**被數到。後者是
    /// 摘要那句「這段時間你一直在動」的唯一依據——`working_ticks` 在第 0
    /// 步就加了，拿它當依據等於憑空替使用者作證。
    #[test]
    fn a_tick_that_blows_up_every_time_leaves_a_number_and_no_alibi() {
        use crate::traits::{CompositeBackend, NullFocus, NullInput, NullOcr, NullScreen};

        struct BrokenClipboard;
        impl crate::traits::ClipboardSource for BrokenClipboard {
            fn poll(&mut self, _ts: Millis) -> Result<Option<ClipboardEvent>> {
                anyhow::bail!("剪貼簿被別的程式鎖住了")
            }
        }

        let backend = CompositeBackend {
            name: "broken".into(),
            screen: NullScreen,
            focus: NullFocus,
            clipboard: BrokenClipboard,
            input: NullInput,
            ocr: NullOcr,
        };
        let mut rec = Recorder::new(
            backend,
            Db::open_in_memory().unwrap(),
            Config::default(),
            None,
        )
        .unwrap();

        for ts in [1_000, 2_000, 3_000] {
            assert!(rec.tick(ts).is_err(), "{ts} 這一拍應該炸掉");
        }

        let s = rec.stats();
        assert_eq!(s.working_ticks, 3, "三拍都過了 enabled/暫停那兩道門");
        assert_eq!(s.tick_failures, 3, "三拍都得算進失敗");
        assert!(
            s.last_tick_error
                .as_deref()
                .is_some_and(|e| e.contains("剪貼簿被別的程式鎖住了")),
            "要留下原文，不是只留一個數字：{:?}",
            s.last_tick_error
        );
        assert_eq!(
            s.idle_asked, 0,
            "閘門在第 5 步，一次都沒走到——摘要不准說「你一直在動」"
        );
    }

    /// 暫停與解除各留一筆事件——不然事後看不出「那三小時是暫停」還是
    /// 「那三小時什麼都沒發生」。這兩件事的意思差很遠。
    ///
    /// 同時釘住「只有轉換才寫」：暫停三小時如果每個 tick 寫一筆，
    /// 那張表會被灌進上萬筆一模一樣的紀錄。
    #[test]
    fn a_gap_in_the_data_says_why_it_is_there() {
        let scenario = Scenario {
            name: "pause".into(),
            steps: vec![Step::default()],
        };
        let mut rec = Recorder::new(
            crate::replay::ReplayBackend::new(scenario),
            Db::open_in_memory().unwrap(),
            Config::default(),
            None,
        )
        .unwrap();

        assert!(rec.set_paused(true, 1_000).unwrap());
        for ts in [1_100, 1_200, 1_300] {
            assert_eq!(rec.tick(ts).unwrap(), Tick::Paused);
            // 一直餵同一個值也不該再寫事件
            assert!(!rec.set_paused(true, ts).unwrap());
        }
        assert!(rec.set_paused(false, 2_000).unwrap());
        assert!(!rec.set_paused(false, 2_100).unwrap());

        let pauses: Vec<(String, i64)> = {
            let conn = rec.db().conn();
            let mut stmt = conn
                .prepare(
                    "SELECT kind, ts FROM system_events \
                     WHERE kind IN ('pause', 'resume') ORDER BY id",
                )
                .expect("prepare");
            let rows = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .expect("query");
            rows.flatten().collect()
        };
        assert_eq!(
            pauses,
            vec![("pause".to_string(), 1_000), ("resume".to_string(), 2_000)],
            "只有兩次轉換，各一筆，時戳是按下去的那一刻"
        );
    }

    /// 沒有人動的時候不看螢幕——但**不准無限期不看**。
    ///
    /// 這一條是實測逼出來的：探測圖 27.0 ms，它想省的那次抓圖 30.1 ms，
    /// 而探測是每個 tick 都付。真正的省法是連螢幕都不碰。
    ///
    /// 危險在另一邊：影片和進度條不需要任何輸入就會變。所以這裡兩件事一起
    /// 釘住——閒著時真的跳過，而且跳過有天花板。少釘後面那一半的話，這個
    /// 最佳化就變成一顆「CPU 很漂亮但什麼都沒錄到」的定時炸彈。
    #[test]
    fn nobody_touched_anything_so_she_does_not_look_but_she_still_blinks() {
        use std::cell::Cell;
        use std::rc::Rc;

        /// 回報「上次輸入是很久以前」，並數自己被問了幾次螢幕。
        struct CountingScreen(Rc<Cell<u32>>);
        impl crate::traits::ScreenSource for CountingScreen {
            fn grab(&mut self, ts: Millis) -> Result<Option<RawFrame>> {
                self.0.set(self.0.get() + 1);
                Ok(Some(RawFrame::from_rgba(ts, 0, 8, 8, vec![9u8; 8 * 8 * 4])))
            }
        }
        struct NeverTouched;
        impl crate::traits::InputSource for NeverTouched {
            fn drain(&mut self, _ts: Millis) -> Result<Option<sister_core::model::InputMetrics>> {
                Ok(None)
            }
            fn idle_ms(&mut self) -> Option<u64> {
                Some(60 * 60 * 1000) // 一小時沒動
            }
        }

        let looks = Rc::new(Cell::new(0));
        let backend = crate::traits::CompositeBackend {
            name: "idle".into(),
            screen: CountingScreen(looks.clone()),
            focus: crate::traits::NullFocus,
            clipboard: crate::traits::NullClipboard,
            input: NeverTouched,
            ocr: crate::traits::NullOcr,
        };
        let mut rec = Recorder::new(
            backend,
            Db::open_in_memory().unwrap(),
            Config::default(),
            None,
        )
        .unwrap();

        // 第一次一定要看：還沒有「上次看螢幕」可以比。
        rec.tick(0).unwrap();
        let after_first = looks.get();
        assert!(after_first > 0, "第一個 tick 一定要真的看一次");

        // 天花板之內、又沒有人動 → 一次都不碰螢幕。
        for ts in [400, 800, 1_200, 4_800] {
            assert!(matches!(rec.tick(ts).unwrap(), Tick::Idle), "ts={ts}");
        }
        assert_eq!(looks.get(), after_first, "沒有人動，不該再碰螢幕");

        // 超過天花板 → 就算沒人動也要睜眼。
        assert!(!matches!(rec.tick(5_000).unwrap(), Tick::Idle));
        assert!(
            looks.get() > after_first,
            "閉眼超過 MAX_BLIND_MS 就得看一次"
        );

        // 而且要看得見自己省了多少——省電和停工在帳面上長得一樣。
        assert_eq!(rec.stats().skipped_idle, 4);
    }

    /// 通知搶走焦點、安裝程式跳出來——這些不需要任何輸入，但畫面變了。
    ///
    /// 沒有這一條的話它們最多要等 5 秒才被看到。有了它，那個盲區只剩下
    /// 「畫面自己在動、而且連視窗標題都沒變」的情況（影片、進度條）。
    #[test]
    fn a_window_that_stole_focus_gets_looked_at_even_though_nobody_typed() {
        use std::cell::Cell;
        use std::rc::Rc;

        struct CountingScreen(Rc<Cell<u32>>);
        impl crate::traits::ScreenSource for CountingScreen {
            fn grab(&mut self, ts: Millis) -> Result<Option<RawFrame>> {
                self.0.set(self.0.get() + 1);
                Ok(Some(RawFrame::from_rgba(ts, 0, 8, 8, vec![9u8; 8 * 8 * 4])))
            }
        }
        struct NeverTouched;
        impl crate::traits::InputSource for NeverTouched {
            fn drain(&mut self, _ts: Millis) -> Result<Option<sister_core::model::InputMetrics>> {
                Ok(None)
            }
            fn idle_ms(&mut self) -> Option<u64> {
                Some(60 * 60 * 1000)
            }
        }
        /// 第三次被問的時候換一個 app——完全沒有人碰鍵盤滑鼠。
        struct Intruder(u32);
        impl crate::traits::FocusSource for Intruder {
            fn snapshot(&mut self, _ts: Millis) -> Result<FocusSnapshot> {
                self.0 += 1;
                Ok(FocusSnapshot {
                    app_id: Some(
                        if self.0 >= 3 {
                            "installer.exe"
                        } else {
                            "code.exe"
                        }
                        .into(),
                    ),
                    ..Default::default()
                })
            }
        }

        let looks = Rc::new(Cell::new(0));
        let backend = crate::traits::CompositeBackend {
            name: "intruder".into(),
            screen: CountingScreen(looks.clone()),
            focus: Intruder(0),
            clipboard: crate::traits::NullClipboard,
            input: NeverTouched,
            ocr: crate::traits::NullOcr,
        };
        let mut rec = Recorder::new(
            backend,
            Db::open_in_memory().unwrap(),
            Config::default(),
            None,
        )
        .unwrap();

        rec.tick(0).unwrap(); // 第一次一定看（還沒有基準）
        let baseline = looks.get();
        assert!(matches!(rec.tick(400).unwrap(), Tick::Idle), "沒人動就別看");
        assert_eq!(looks.get(), baseline);

        // 第三個 tick 換了 app。時間還遠在天花板之內，也還是沒有人動。
        assert!(!matches!(rec.tick(800).unwrap(), Tick::Idle));
        assert!(looks.get() > baseline, "脈絡變了就得睜眼，別等到 5 秒");
    }

    /// 一個會走的時鐘不算「脈絡變了」。
    ///
    /// 上面那條規則（標題變了就睜眼）對**一次**標題變化是對的，對一個每拍
    /// 都在變的標題就整個反過來：`context_changed` 永遠是 `true`，
    /// `idle_ms()` 一次都問不到，省電閘門從第一秒起就是關的。
    ///
    /// 這不是假想的。跑 build 的 Windows Terminal、播放器的進度、
    /// `(3) Slack` 的未讀數、VS Code 的 ●、下載百分比——開著任何一個就夠。
    /// 2560×1440 一次抓圖約 127 ms，閘門開著是 17,280 次/天（2.5% CPU），
    /// 關著是 216,000 次/天（31.7%）。實測 27.1%。
    ///
    /// 所以這裡兩件事一起釘：時鐘不准把閘門關掉，而**第一次**標題變化仍然
    /// 要睜眼——少釘後面那一半的話，這個修法就等於把上面那條規則刪掉。
    #[test]
    fn a_ticking_clock_in_the_title_bar_must_not_hold_her_eyes_open_all_day() {
        use std::cell::Cell;
        use std::rc::Rc;

        struct CountingScreen(Rc<Cell<u32>>);
        impl crate::traits::ScreenSource for CountingScreen {
            fn grab(&mut self, ts: Millis) -> Result<Option<RawFrame>> {
                self.0.set(self.0.get() + 1);
                // 每次都給不一樣的畫面，免得去重把差異吃掉——這個測試量的是
                // 「碰了幾次螢幕」，不是「留了幾張」。
                Ok(Some(RawFrame::from_rgba(
                    ts,
                    0,
                    8,
                    8,
                    vec![(ts % 251) as u8; 8 * 8 * 4],
                )))
            }
        }
        struct NeverTouched;
        impl crate::traits::InputSource for NeverTouched {
            fn drain(&mut self, _ts: Millis) -> Result<Option<sister_core::model::InputMetrics>> {
                Ok(None)
            }
            fn idle_ms(&mut self) -> Option<u64> {
                Some(60 * 60 * 1000) // 一小時沒動過
            }
        }
        /// 同一個視窗，標題每一拍都不一樣——就是一個在跑的 build。
        struct BuildingTerminal(u32);
        impl crate::traits::FocusSource for BuildingTerminal {
            fn snapshot(&mut self, _ts: Millis) -> Result<FocusSnapshot> {
                self.0 += 1;
                Ok(FocusSnapshot {
                    app_id: Some("windowsterminal.exe".into()),
                    window_title: Some(format!("cargo build — {}s", self.0)),
                    ..Default::default()
                })
            }
        }

        let looks = Rc::new(Cell::new(0));
        let backend = crate::traits::CompositeBackend {
            name: "clock".into(),
            screen: CountingScreen(looks.clone()),
            focus: BuildingTerminal(0),
            clipboard: crate::traits::NullClipboard,
            input: NeverTouched,
            ocr: crate::traits::NullOcr,
        };
        let mut rec = Recorder::new(
            backend,
            Db::open_in_memory().unwrap(),
            Config::default(),
            None,
        )
        .unwrap();

        // 前幾拍還看不出來這是時鐘，所以標題一變就睜眼——那條規則沒有被
        // 刪掉，只是加了條件。
        rec.tick(0).unwrap();
        let after_first = looks.get();
        rec.tick(400).unwrap();
        assert!(
            looks.get() > after_first,
            "還沒判定成時鐘之前，標題變了仍然要睜眼（收窄 5 秒盲區那一條）"
        );

        const TICKS: u32 = 100;
        for i in 2..TICKS {
            rec.tick(i as i64 * 400).unwrap();
        }

        // 40 秒、100 拍。閘門開著的話只有 MAX_BLIND_MS 那幾次眨眼要付錢。
        assert!(
            looks.get() < TICKS / 3,
            "標題是時鐘卻碰了 {} 次螢幕（共 {TICKS} 拍）——省電閘門被關掉了",
            looks.get()
        );
        assert!(rec.stats().skipped_idle > 0, "閘門要真的擋下東西");
        assert!(
            rec.stats().title_clock_ticks > 0,
            "省下來的原因要說得出口，不然它和「他整天沒動」長得一樣"
        );

        // 資料庫那一半：一天 216,000 列 `(3) Slack → (4) Slack` 會把時間軸
        // 淹掉，而那些列一列都沒有回答任何問題。
        let rows: i64 = rec
            .db
            .conn()
            .query_row("SELECT COUNT(*) FROM focus_events", [], |r| r.get(0))
            .expect("count");
        assert!(
            rows < (TICKS / 3) as i64,
            "時鐘走了 {TICKS} 格就寫了 {rows} 列 focus"
        );
    }

    /// 時鐘往回跳不可以讓她從此閉眼。
    ///
    /// `ts - last_look` 變負、被夾成 0，於是「距離上次看過了 0 毫秒」永遠
    /// 小於天花板；而 `last_look_ts` 只在真的看的時候更新，所以那個 0 再也
    /// 不會變大。症狀是她從此不再看螢幕——而 tick 照跑、CPU 漂亮、沒有
    /// 任何錯誤。同一種形狀在畫面檔節流那裡也發生過一次。
    #[test]
    fn a_clock_that_jumps_backwards_does_not_blind_her_forever() {
        use std::cell::Cell;
        use std::rc::Rc;

        struct CountingScreen(Rc<Cell<u32>>);
        impl crate::traits::ScreenSource for CountingScreen {
            fn grab(&mut self, ts: Millis) -> Result<Option<RawFrame>> {
                self.0.set(self.0.get() + 1);
                Ok(Some(RawFrame::from_rgba(ts, 0, 8, 8, vec![9u8; 8 * 8 * 4])))
            }
        }
        struct NeverTouched;
        impl crate::traits::InputSource for NeverTouched {
            fn drain(&mut self, _ts: Millis) -> Result<Option<sister_core::model::InputMetrics>> {
                Ok(None)
            }
            fn idle_ms(&mut self) -> Option<u64> {
                Some(60 * 60 * 1000)
            }
        }

        let looks = Rc::new(Cell::new(0));
        let backend = crate::traits::CompositeBackend {
            name: "backwards".into(),
            screen: CountingScreen(looks.clone()),
            focus: crate::traits::NullFocus,
            clipboard: crate::traits::NullClipboard,
            input: NeverTouched,
            ocr: crate::traits::NullOcr,
        };
        let mut rec = Recorder::new(
            backend,
            Db::open_in_memory().unwrap(),
            Config::default(),
            None,
        )
        .unwrap();

        rec.tick(1_000_000).unwrap();
        let baseline = looks.get();

        // 時鐘退了一小時。往後每個 tick 的 `ts` 都比 `last_look` 小。
        for ts in [1_000_000 - 3_600_000, 1_000_000 - 3_600_000 + 400] {
            rec.tick(ts).unwrap();
        }
        assert!(
            looks.get() > baseline,
            "時鐘往回跳之後她再也沒看過螢幕——而摘要上一切正常"
        );
    }

    /// 答不出閒置時間的平台，行為必須和以前一模一樣。
    ///
    /// 這個最佳化的預設值只能是「照舊」：Linux/macOS 後端還沒有這個訊號，
    /// 而一個「不知道 ⇒ 就當作沒變」的預設會讓她在那些平台上直接瞎掉。
    #[test]
    fn a_platform_that_cannot_tell_idle_time_keeps_looking_every_tick() {
        let mut rec = screen_only(crate::traits::NullOcr);
        for ts in [0, 400, 800, 1_200] {
            assert!(!matches!(rec.tick(ts).unwrap(), Tick::Idle), "ts={ts}");
        }
        assert_eq!(rec.stats().skipped_idle, 0);
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
            for (i, p) in px.as_chunks_mut::<4>().0.iter_mut().enumerate() {
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

    // ---------- 一個 tick 只讀一次螢幕 ----------
    //
    // 這一組測試釘住的是 alpha.4 實測踩到的兩個數字：CPU 27.1%（預算 3%）
    // 與一段被讀成 `Micr099ftTeamsTr` 的 OCR 文字。
    //
    // 第一版的解法是「兩段式抓圖」：先讀一張便宜的小圖算雜湊，變了才讀
    // 完整的那張。OCR 那個問題確實修好了。CPU 那個沒有——實測探測
    // 43.1 ms、它想省的抓圖 33.4 ms，因為擷取成本由**來源**像素決定，
    // 而兩次讀的是同一個螢幕。所以現在只讀一次。

    #[derive(Default)]
    struct StageLog {
        grabs: u32,
        /// OCR 每次拿到的尺寸。這就是文字品質的全部。
        ocr_sizes: Vec<(u32, u32)>,
    }

    /// 灰階漸層。同一個 `seed` 一定算出同一個 dhash，換 seed 就會變。
    fn pattern(w: u32, h: u32, seed: u32) -> Vec<u8> {
        let mut px = vec![255u8; (w * h * 4) as usize];
        for (i, p) in px.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let v = ((i as u32 * 7 + seed * 40) % 256) as u8;
            (p[0], p[1], p[2]) = (v, v, v);
        }
        px
    }

    /// 數自己被讀了幾次的螢幕。
    struct CountedScreen {
        log: std::rc::Rc<std::cell::RefCell<StageLog>>,
    }

    const FULL: (u32, u32) = (256, 144);

    impl crate::traits::ScreenSource for CountedScreen {
        fn grab(&mut self, ts: Millis) -> Result<Option<RawFrame>> {
            self.log.borrow_mut().grabs += 1;
            let (w, h) = FULL;
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

    fn counted(
        log: std::rc::Rc<std::cell::RefCell<StageLog>>,
    ) -> Recorder<
        crate::traits::CompositeBackend<
            CountedScreen,
            crate::traits::NullFocus,
            crate::traits::NullClipboard,
            crate::traits::NullInput,
            SizeSpy,
        >,
    > {
        let backend = crate::traits::CompositeBackend {
            name: "counted".into(),
            screen: CountedScreen { log: log.clone() },
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

    /// **OCR 必須拿到完整解析度的那一張。**
    ///
    /// 這條線是實測那串亂碼的來源。原本三件事共用同一份像素：去重只需要
    /// 9×8、存檔想要 1568、OCR 要原生解析度——結果所有人都拿到 1568，
    /// 2560 的螢幕縮成 0.61 倍，12px 的字掉到 7px，於是
    /// `Microsoft Teams` 被讀成 `Micr099ftTeamsTr`。引擎不報錯，只是讀錯。
    #[test]
    fn ocr_reads_the_frame_at_the_resolution_the_screen_handed_over() {
        let log = std::rc::Rc::new(std::cell::RefCell::new(StageLog::default()));
        let mut r = counted(log.clone());
        r.tick(0).expect("tick");

        let log = log.borrow();
        assert_eq!(
            log.ocr_sizes,
            vec![FULL],
            "OCR 拿到縮過的圖就等於讀不出字；實際 {:?}",
            log.ocr_sizes
        );
    }

    /// **一個 tick 只准讀一次螢幕。**
    ///
    /// 這是 CPU 那個數字真正的修法。第一版反過來：每個 tick 讀兩次——
    /// 一張便宜的探測圖算雜湊，變了再讀完整的那張。算式看起來很划算
    /// （要搬的位元組 14MB → 147KB），實測是白付的（探測 43.1 ms、
    /// 抓圖 33.4 ms），因為成本由**來源**像素決定，而兩次讀的是同一個
    /// 螢幕。縮小目的地只是把一張讀完的畫面丟掉，然後再讀一次。
    ///
    /// 這條斷言擋的就是「再讀一次」以任何形式長回來。它長回來的時候，
    /// 摘要上完全看不出差別——只有電池會知道。
    #[test]
    fn one_tick_reads_the_screen_exactly_once() {
        let log = std::rc::Rc::new(std::cell::RefCell::new(StageLog::default()));
        let mut r = counted(log.clone());
        for i in 0..4 {
            r.tick(i * 1_000).expect("tick");
        }

        assert_eq!(r.stats().kept, 1);
        assert_eq!(r.stats().duplicates, 3);
        assert_eq!(log.borrow().grabs, 4, "四個 tick 就該剛好讀四次");
    }

    /// 資料庫裡記的 dhash，必須就是當初據以判斷「變了沒」的那一個。
    ///
    /// 兩段式的時候這件事會安靜地錯：判定用探測圖的雜湊、存的卻是完整圖
    /// 的，而兩張尺寸不同、雜湊也就不同。現在只有一張圖，所以它應該是
    /// 自動成立的——留著這條斷言，是為了讓「自動成立」這件事有人看著。
    #[test]
    fn the_stored_hash_is_the_one_dedup_actually_used() {
        let log = std::rc::Rc::new(std::cell::RefCell::new(StageLog::default()));
        let mut r = counted(log);
        r.tick(0).expect("tick");

        let (w, h) = FULL;
        let expected = sister_core::dedup::dhash_rgb(&pattern(w, h, 1), w, h, 4);
        let stored: i64 = r
            .db()
            .conn()
            .query_row("SELECT dhash FROM frames", [], |row| row.get(0))
            .expect("query");
        assert_eq!(stored as u64, expected, "存的必須是判定用的那一個雜湊");
    }

    /// 判定為「新的」卻沒有真的存下來，不可以污染去重狀態。
    ///
    /// 沒退回去的話，下一張一模一樣的畫面會被判成「重複」，然後把重複計數
    /// 加到一張**更早**的幀身上——那一幀的 `dup_run` 從此是假的，而且沒有
    /// 人會發現。
    ///
    /// 「沒存成」有三種，而三種都出現過、也都漏掉過：`Ok(None)`（鎖屏）、
    /// `Err`（抓圖失敗）、以及資料庫那一列寫不進去。前兩種在這裡測，第三種
    /// 在 [`a_failed_database_write_does_not_poison_dedup`]。
    ///
    /// 修法不是在三個地方各補一次退回，是讓 `check` 不再推進基準——推進的
    /// 是存完之後那一句 `kept`。三個 bug 是同一個形狀。
    #[test]
    fn a_screen_that_vanishes_between_probe_and_grab_does_not_poison_dedup() {
        /// 0 = 交出畫面、1 = 回 `None`（鎖屏）、2 = 回 `Err`（抓圖失敗）
        struct Flaky(std::rc::Rc<std::cell::Cell<u8>>);
        impl crate::traits::ScreenSource for Flaky {
            fn grab(&mut self, ts: Millis) -> Result<Option<RawFrame>> {
                match self.0.get() {
                    1 => Ok(None),
                    2 => Err(anyhow::anyhow!("GetDIBits returned no scanlines")),
                    _ => {
                        let (w, h) = FULL;
                        Ok(Some(RawFrame::from_rgba(ts, 0, w, h, pattern(w, h, 1))))
                    }
                }
            }
        }

        for failure in [1u8, 2] {
            let mode = std::rc::Rc::new(std::cell::Cell::new(failure));
            let backend = crate::traits::CompositeBackend {
                name: "flaky".into(),
                screen: Flaky(mode.clone()),
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

            match failure {
                1 => assert_eq!(r.tick(0).expect("鎖屏不是錯誤"), Tick::NoScreen),
                _ => assert!(r.tick(0).is_err(), "抓圖失敗要往上報，不能吞掉"),
            }

            // 同一個畫面回來了。它從來沒被存過，所以必須是新的。
            mode.set(0);
            assert!(
                matches!(r.tick(1_000).expect("tick"), Tick::Kept { .. }),
                "抓不到的第 {failure} 種：沒存成的那一張不能把後面真的那一張擋掉"
            );
        }
    }

    /// 那一列沒寫成，剛寫出去的 PNG 不可以留在磁碟上。
    ///
    /// `retention::prune` 只走 `image_path IS NOT NULL` 的列，所以一張沒有
    /// 對應列的圖**永遠不會被清掉**——「畫面 30 天後刪掉」對它是一句假話，
    /// 而且它還一直佔著今天的畫面額度。
    #[test]
    fn an_image_whose_row_never_landed_does_not_stay_on_disk() {
        let dir = std::env::temp_dir().join(format!("sister-orphan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let db = Db::open_in_memory().expect("db");
        db.conn()
            .execute_batch(
                "CREATE TRIGGER t_boom BEFORE INSERT ON frames
                 BEGIN SELECT RAISE(ABORT, 'simulated disk failure'); END;",
            )
            .expect("ddl");

        let backend = crate::traits::CompositeBackend {
            name: "orphan".into(),
            screen: ChangingScreen::default(),
            focus: crate::traits::NullFocus,
            clipboard: crate::traits::NullClipboard,
            input: crate::traits::NullInput,
            ocr: NumberedLines::default(),
        };
        let mut r =
            Recorder::new(backend, db, Config::default(), Some(dir.clone())).expect("recorder");

        assert!(r.tick(0).is_err(), "寫不進去要往上報");

        let pngs = walk_pngs(&dir);
        assert!(
            pngs.is_empty(),
            "那一列沒成立，這張圖沒有任何東西指向它，清理也永遠掃不到它：{pngs:?}"
        );
        // 額度也要退回去，否則今天剩下的時間會少存好幾百張
        assert_eq!(r.stats().image_bytes, 0, "沒算數的圖不可以佔額度");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn walk_pngs(dir: &std::path::Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "png") {
                    out.push(p);
                }
            }
        }
        out
    }

    /// 時鐘往回跳，不可以讓她整整幾小時一張圖都不存。
    ///
    /// `ts - prev` 是負數，而負數永遠小於間隔——於是節流會一路擋到時鐘
    /// 追回來為止。8 小時的時區修正（雙系統把 RTC 當本地時間寫、VM 從
    /// 快照恢復）就等於 8 小時只留字，而摘要會說「間隔未到」，那是假的。
    #[test]
    fn a_clock_that_jumps_backwards_does_not_stop_her_saving_screens() {
        let dir = std::env::temp_dir().join(format!("sister-clock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let mut cfg = Config::default();
        cfg.capture.image_min_interval_ms = 5_000;
        let mut r = image_recorder(cfg, dir.clone());

        let noon = 12 * 60 * 60 * 1000;
        assert!(matches!(r.tick(noon).expect("tick"), Tick::Kept { .. }));
        assert_eq!(walk_pngs(&dir).len(), 1);

        // 時鐘往回跳 8 小時
        let before = noon - 8 * 60 * 60 * 1000;
        assert!(matches!(r.tick(before).expect("tick"), Tick::Kept { .. }));
        assert_eq!(
            walk_pngs(&dir).len(),
            2,
            "時鐘往回跳不是「間隔未到」，不能拿它當理由不存圖"
        );
        assert_eq!(
            r.stats().images_throttled,
            0,
            "更不能把它算成節流——那會讓摘要講出一個假的原因"
        );

        // 跳回去之後，節流照常生效
        assert!(matches!(
            r.tick(before + 100).expect("tick"),
            Tick::Kept { .. }
        ));
        assert_eq!(walk_pngs(&dir).len(), 2, "剛存過，這張要被間隔擋下");
        assert_eq!(r.stats().images_throttled, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 第三種「沒存成」：畫面抓到了、字讀到了，資料庫那一列寫不進去。
    ///
    /// 這一種最晚被發現，因為前面每一步看起來都成功了。磁碟滿、資料庫被
    /// 鎖住、schema 對不上都會走到這裡，而它們都不是不會發生的事。
    #[test]
    fn a_failed_database_write_does_not_poison_dedup() {
        let db = Db::open_in_memory().expect("db");
        // 用一個可以開關的 trigger 模擬寫入失敗，這樣同一顆資料庫可以先壞後好
        db.conn()
            .execute_batch(
                "CREATE TABLE boom(on_ INTEGER);
                 INSERT INTO boom VALUES(0);
                 CREATE TRIGGER t_boom BEFORE INSERT ON frames
                 WHEN (SELECT on_ FROM boom) = 1
                 BEGIN SELECT RAISE(ABORT, 'simulated disk failure'); END;",
            )
            .expect("ddl");

        /// 畫面內容由外面控制，這樣「同一個畫面再來一次」才做得出來。
        struct Held(std::rc::Rc<std::cell::Cell<u32>>);
        impl crate::traits::ScreenSource for Held {
            fn grab(&mut self, ts: Millis) -> Result<Option<RawFrame>> {
                let (w, h) = FULL;
                Ok(Some(RawFrame::from_rgba(
                    ts,
                    0,
                    w,
                    h,
                    pattern(w, h, self.0.get()),
                )))
            }
        }

        let screen = std::rc::Rc::new(std::cell::Cell::new(1u32));
        let backend = crate::traits::CompositeBackend {
            name: "held".into(),
            screen: Held(screen.clone()),
            focus: crate::traits::NullFocus,
            clipboard: crate::traits::NullClipboard,
            input: crate::traits::NullInput,
            ocr: crate::traits::NullOcr,
        };
        let mut r = Recorder::new(backend, db, Config::default(), None).expect("recorder");

        // 畫面 A 存進去了
        assert!(matches!(r.tick(0).expect("tick"), Tick::Kept { .. }));

        // 畫面換成 B，但這一列寫不進去
        screen.set(2);
        r.db().conn().execute("UPDATE boom SET on_=1", []).unwrap();
        assert!(r.tick(1_000).is_err(), "寫不進去要往上報");

        // 資料庫好了，螢幕上還是 B。B 從來沒被存過，所以必須是新的。
        r.db().conn().execute("UPDATE boom SET on_=0", []).unwrap();
        assert!(
            matches!(r.tick(2_000).expect("tick"), Tick::Kept { .. }),
            "寫不進去的那一張不能把後面真的那一張擋成重複"
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

    /// 每次都給一張不一樣的畫面，這樣去重不會把它們併掉。
    /// replay 後端不產生像素（`rgba: None`），所以存圖這條路徑測不到。
    #[derive(Default)]
    struct ChangingScreen {
        seed: u32,
    }
    impl crate::traits::ScreenSource for ChangingScreen {
        fn grab(&mut self, ts: Millis) -> Result<Option<RawFrame>> {
            self.seed += 1;
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

    /// `frames.image_path` 存的是**相對**路徑，而且要接得回真的那個檔。
    ///
    /// 這條線釘的不是實作細節，是一份契約。整個 `frames/` 目錄要能整包搬走、
    /// 整包備份、換一台機器接回去，所以那一欄不可以是絕對路徑；代價是**每一個
    /// 讀它的人都得先接上根目錄**，而漏掉的那個人不會拿到錯誤——他會拿到
    /// 「檔案不存在」，然後照實印出「圖不見了」。
    ///
    /// 字母人的 `frame_image` 就是這樣漏掉的：`fs::read("2026/08/19/….png")`
    /// 拿行程的工作目錄當根，也就是他按下捷徑的那個資料夾。每一次點出處都說
    /// 圖不見了，而圖好端端地躺在磁碟上。`db.rs` 裡那個 fixture 存的是
    /// `/tmp/x.webp`——一個絕對路徑，所以型別上看起來一切正常。
    #[test]
    fn the_stored_path_is_relative_to_the_frames_root_not_to_anywhere_else() {
        let tmp = Tmp::new("relative");
        let mut r = image_recorder(Config::default(), tmp.0.clone());
        assert!(matches!(r.tick(0).expect("tick"), Tick::Kept { .. }));

        let stored: String = r
            .db()
            .conn()
            .query_row(
                "SELECT image_path FROM frames WHERE image_path IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .expect("一張圖都沒寫出來");

        let path = std::path::Path::new(&stored);
        assert!(
            path.is_relative(),
            "存的是絕對路徑（{stored}）——那份 frames/ 就搬不走了"
        );
        assert!(
            !path.exists(),
            "{stored} 從工作目錄就打得開，這條測試等於沒驗到"
        );
        assert!(
            tmp.0.join(&stored).is_file(),
            "接上根目錄之後要打得開：{}",
            tmp.0.join(&stored).display()
        );
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

    /// 撞到上限的次數跨天累加，而摘要那句話說的是「**今天**」。
    ///
    /// 開著不關就是這個產品的預設用法，所以一場 session 橫跨好幾天不是邊角
    /// 情況。少了天數，一個連錄五天、每天都爆額度的人會讀到「今天之後的
    /// 4200 張只留了字」——那個數字是五天的，而他會拿它去判斷今天發生了
    /// 什麼事。
    #[test]
    fn a_five_day_session_does_not_get_to_call_five_days_of_overflow_today() {
        let tmp = Tmp::new("budget-days");
        let mut config = Config::default();
        config.capture.image_min_interval_ms = 0;
        config.capture.max_image_mb_per_day = 1;

        let mut r = image_recorder(config, tmp.0.clone());
        const DAY: i64 = 86_400_000;

        // 第一天撞兩次。（先 tick 一次讓跨日歸零跑掉，再把額度灌滿——和上面
        // 那條測試同一個手法。）
        r.tick(1_000).expect("tick");
        r.image_bytes_today = 2 * 1024 * 1024;
        r.tick(2_000).expect("tick");
        r.tick(3_000).expect("tick");
        assert_eq!(r.stats().images_over_budget, 2);
        assert_eq!(r.stats().images_over_budget_days, 1, "同一天只算一天");

        // 第二天：額度歸零，寫得出去，不該被算進「撞到上限的那幾天」。
        r.tick(DAY + 1_000).expect("tick");
        assert_eq!(r.stats().images_over_budget, 2, "新的一天不該再被擋");
        assert_eq!(r.stats().images_over_budget_days, 1, "沒撞到的那天不算");

        // 第三天又撞到。
        r.tick(2 * DAY + 1_000).expect("tick");
        r.image_bytes_today = 2 * 1024 * 1024;
        r.tick(2 * DAY + 2_000).expect("tick");
        assert_eq!(r.stats().images_over_budget, 3);
        assert_eq!(r.stats().images_over_budget_days, 2);
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
        r.finish(EndReason::Requested).expect("finish");

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

    /// 「她停了」和「她為什麼停了」是兩個問題。以前只答得出第一個：不管是
    /// 按了停止、時間到、還是同意書被撤回，`session_end` 都長得一模一樣。
    #[test]
    fn a_recording_that_stopped_can_say_why() {
        let mut r = recorder(vec![step(0, "chrome.exe", "x", &["y"])], Config::default());
        r.tick(0).expect("tick");
        r.finish(EndReason::ConsentRevoked).expect("finish");

        let last = r.db().last_session().expect("last").expect("有一場");
        assert!(last.ended_at.is_some(), "好好收尾了");
        assert_eq!(last.reason.as_deref(), Some("consent-revoked"));
    }

    /// 沒有收尾的那一場說不出理由——**它本來就寫不了任何東西**。這裡要驗的
    /// 是那時候讀出來的東西長什麼樣：`ended_at` 是 `None`，而不是某個猜出來
    /// 的預設值。
    #[test]
    fn a_recording_that_died_says_nothing() {
        let r = recorder(vec![step(0, "chrome.exe", "x", &["y"])], Config::default());
        let last = r.db().last_session().expect("last").expect("有一場");
        assert_eq!(last.ended_at, None);
        assert_eq!(last.reason, None);
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
