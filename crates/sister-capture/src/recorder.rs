//! 錄製迴圈：把感官訊號變成 L0 證據。
//!
//! 這個檔案裡的**順序**就是隱私架構本身（SPEC §11.2）。排除判定必須發生在
//! 截圖之前——被排除的畫面從來沒有被抓過，而不是抓了再刪。事後刪除
//! 救不了已經寫進磁碟的東西。
//!
//! 這一層完全沒有模型呼叫。它只負責抄寫。

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use sister_core::config::Config;
use sister_core::db::Db;
use sister_core::dedup::{Deduper, FrameVerdict};
use sister_core::model::{
    ClipboardEvent, EndReason, FocusEvent, FocusKind, FocusSnapshot, FrameCapture, InputTick,
    Millis, SystemEvent, SystemKind,
};
use sister_core::pause::{PauseGeneration, PauseSnapshot, PauseSnapshotGuard};
use sister_core::redact;

use crate::traits::{
    Backend, CapturePermit, ClipboardCapture, ClipboardWatermark, DhashRecheck, OcrAttempt,
    OcrWork, PrivacyObservation, RawFrame, ScreenCapture, SystemContentState, SystemObservation,
    SystemTransition,
};

/// 這一台 recorder 要從哪裡讀跨行程的全停控制面。
///
/// 沒有預設值：每個建構點都必須明講自己是否受全停控制。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MasterStopSource {
    /// 產品 recorder：逐拍讀指定 data dir 裡的 durable latch。
    Latch(PathBuf),
    /// replay／測試／第三方 recorder：不參與跨行程控制面。
    ///
    /// 選這一格的後果是 `sister stop-all` 關不掉這一台 recorder；呼叫端必須能
    /// 說明為什麼這台 recorder 不屬於產品錄製路徑。
    NotApplicable,
}

enum MasterActivity {
    Guard(sister_hands::master_stop::ActivityGuard),
    NotApplicable,
}

enum MasterCommitGuard {
    Guard {
        _guard: sister_hands::master_stop::ActivityBoundary,
    },
    NotApplicable,
}

enum MasterBoundaryCheck {
    Continue(MasterCommitGuard),
    Stopped,
}

/// 一天有多少毫秒。畫面額度以 UTC 天為單位重置，和
/// `frames::relative_path` 的資料夾分層是同一條線。
const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// 一次 tick 的結果。呼叫端據此決定要不要記錄、要不要退避。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tick {
    /// 總開關關閉——她閉著眼睛。設定檔說的，重開才會變。
    Disabled,
    /// 跨 capture／brain／hands 的 durable 全停開關正在生效。
    MasterStopped,
    /// 這一拍觀察到全停解除，已寫好稽核並關閉 source 空洞；和 pause resume
    /// 一樣，刻意留到下一拍才重新讀內容。
    MasterReleased,
    /// 使用者按了暫停。和 `Disabled` 差在這是**當下**、可以隨時解除的，
    /// 而且進出各留一筆 system event，所以資料裡那個空洞解釋得出來。
    Paused,
    /// 這一拍觀察到跨行程 resume，已關閉 pause audit 與 source 空洞，但刻意
    /// 不寫任何內容。下一拍重新取時間後才恢復錄製，避免較早建立的 tick 時間
    /// 把內容排到較晚的 resume audit 前面。
    Resumed,
    /// 被排除規則擋下。畫面沒有被抓取。
    Excluded { reason: String },
    /// 這一刻 OS 已知不允許讀內容，或 screen source 沒有 frame。
    NoScreen,
    /// 這一拍真的觀察到 OS lifecycle transition；audit 寫好後仍停拍，
    /// 下一拍重新建立穩定狀態才可讀內容。
    SystemChanged,
    /// 原生系統狀態沒量到；這一拍在任何內容來源前就停下。
    ///
    /// 它不能合併到 `NoScreen`：後者是已知的鎖定／睡眠，這個是
    /// 「沒量到」。
    SystemUnknown,
    /// Privacy gate 後前景身份改變。已讀到 RAM 的 clipboard/frame
    /// buffer 已丟掉，沒有進 dedup/OCR/DB/PNG。
    ContextChanged,
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

/// 跨行程 pause probe 的單次答案。
///
/// 沒有 `Default`，也不收裸 `bool`：呼叫端必須同時交出自己真正觀察的時刻，
/// recorder 才不會拿 tick 開始前的舊時間替稍後才按下的暫停寫 audit。
pub enum PauseSignal {
    /// 沒有跨行程控制面的 recorder（replay／直接單元測試）。這一格只表示
    /// 「這次 probe 沒有新 request」，不會替已暫停的 recorder 自動解除。
    Recording,
    Paused {
        observed_at: Millis,
    },
    /// Production 的原子快照。`guard` 仍持有 shared pause lock；若這是最後
    /// 一道 persistence gate，recorder 會把它留到 PNG／DB commit 結束。
    Snapshot {
        observed_at: Millis,
        guard: PauseSnapshotGuard,
    },
}

/// 一次 pause gate 的結果。`Continue(Some(_))` 的 guard 是 persistence
/// transaction 的能力：活著時 pause writer 無法跨過這個 commit 邊界。
enum PauseCheck {
    Continue(Option<PauseSnapshotGuard>),
    Paused,
    Resumed,
}

fn pause_boundary_tick(check: PauseCheck) -> Option<Tick> {
    match check {
        PauseCheck::Continue(_) => None,
        PauseCheck::Paused => Some(Tick::Paused),
        PauseCheck::Resumed => Some(Tick::Resumed),
    }
}

#[derive(Debug, Clone, Copy)]
struct ActivePauseAudit {
    /// 這一場 recorder 真正寫入 `CapturePaused` 的時間下界。
    started_at: Millis,
    /// `None` 表示由 Indeterminate／直接測試觸發，不能拿任一份持久化
    /// epoch 的舊 resume 時戳來關閉它。
    generation: Option<PauseGeneration>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecorderStats {
    pub ticks: u64,
    /// 最後一次真的從擷取後端拿到的畫面尺寸。`None` 不是 `0×0`：代表這一場
    /// 沒有成功抓到任何畫面，所以解析度量不到。
    pub last_frame_size: Option<(u32, u32)>,
    /// 真的走完一整拍的次數——過了 `capture.enabled` 和暫停這兩道門的。
    ///
    /// `ticks` 在那兩道門**之前**就加了，所以一段「8 小時裡暫停了 7 小時」的
    /// 錄製，72,000 拍裡有 63,000 拍是幾微秒就回來的空轉。拿 `ticks` 當分母
    /// 算「每 tick 幾 ms」會得到 8 ms，而真的做事的那一拍要 60 ms——差 7.5
    /// 倍，而且是往「看起來很便宜」的方向差。那個數字唯一的用途就是判斷這
    /// 個迴圈貴不貴，指錯方向等於沒有。
    pub working_ticks: u64,
    /// 因跨三層全停而在任何畫面擷取前返回的拍數。
    pub master_stopped_ticks: u64,
    /// recorder 親眼看見 latch 解除、寫好 release audit 並刻意不工作的拍數。
    pub master_released_ticks: u64,
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
    /// OS 狀態根本問不到，並在任何內容來源前停下的 tick。
    pub system_unknown: u64,
    /// Privacy gate 通過後，native 前景在內容邊界前後改變的 tick。
    pub context_changed: u64,
    /// 有新 clipboard sequence，但無法將 bytes 綁到明確來源 app。
    pub clipboard_source_unknown: u64,
    /// Clipboard 來源 app 命中 excluded_apps/screenshare policy 而丟掉的事件。
    pub clipboard_source_excluded: u64,
    /// 要封 privacy/system gap 時，clipboard source 沒能建立可信水位。
    pub clipboard_watermark_unknown: u64,
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
    /// 成功寫進資料庫的 OCR 區塊總數。
    ///
    /// 「保留了 12 張畫面」和「記住了 12 張畫面上的字」是兩回事，而摘要
    /// 只印前者的話，兩者看起來一模一樣。實測踩過：12 張畫面、0 行文字、
    /// 摘要一片祥和，要等到搜尋永遠是空的才會發現。
    pub ocr_blocks: u64,
    /// 成功交回完整結果的幀裡，有幾幀真的把整張畫面送進 OCR。
    pub ocr_full_frames: u64,
    /// 上面那些成功全幅裡，有幾幀是局部路徑沒有把握後退回。
    pub ocr_full_fallbacks: u64,
    /// 成功交回完整結果的幀裡，有幾幀只 OCR 變動區域。
    pub ocr_region_frames: u64,
    /// 上面那些局部幀總共送了幾塊 crop。
    pub ocr_regions: u64,
    /// dHash 原本判成近似重複、gate 也真的試了 crop，但結構證據不足而未採用的幀數。
    pub ocr_rejected_region_frames: u64,
    /// 上面那些未採用的幀一共試了幾塊 crop。
    pub ocr_rejected_regions: u64,
    /// 成功交回完整結果、且像素沒變而直接沿用已提交文字的幀數。
    pub ocr_reused_frames: u64,
    /// 如果每次都全幅，本來會交給 OCR 實作嘗試的像素數。
    pub ocr_candidate_pixels: u64,
    /// 實際交給 OCR 實作嘗試的像素數；crop 失敗後又退全幅時，兩筆都算。
    /// 實作可能在呼叫 OS 引擎前因契約或環境問題拒絕，所以不稱「引擎像素」。
    pub ocr_input_pixels: u64,
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
/// **它同時是這個設計的最低抓圖頻率。** 完全沒人碰的一天也要
/// 86400/5 = 17,280 次睜眼，而那筆牆上時間跟一次抓圖多貴成正比。牆上時間
/// 包含等顯示驅動，不能直接換算成 CPU 百分比；CPU 由 [`crate::footprint`]
/// 另外照實量。2026-08-23 起 CPU 不再是 Phase 0 blocker，但這道五秒上限仍在
/// 守「沒有輸入不等於畫面沒有變」的完整性，不能因為預算決策一起拿掉。
///
/// 五秒只在這裡定義：錄製邏輯、測試與說明各寫一份，遲早會有一邊先改。
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SystemGate {
    Stable,
    Transitioned,
    Blocked,
    Unknown,
    /// 上一拍的 audit retry 這拍才成功。本拍仍停；禁止同拍再 poll/content。
    PendingCommitted,
}

#[derive(Debug, Clone)]
struct PendingSystemObservation {
    state: SystemContentState,
    transitions: Vec<SystemTransition>,
    gate: SystemGate,
}

enum ClipboardStage {
    Ready(Option<ClipboardEvent>),
    ContextChanged,
}

/// PNG 已在 RAM 編好、但還沒有碰持久化儲存。
///
/// pause probe 必須放在編碼之後、寫檔之前；不然一張大圖的壓縮時間會重新打開
/// 「按下暫停後仍把 pixels 寫進磁碟」的窗口。`encode_elapsed` 留著，等真的寫成
/// 才一起算進 store timing；被暫停丟掉的工作不是一次成功存圖。
struct PreparedImage {
    ts: Millis,
    relative_path: String,
    full_path: PathBuf,
    bytes: Vec<u8>,
    encode_elapsed: Duration,
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
    /// Production 最近完整處理過的 pause generation。不能用裸 bool 代替：
    /// pause→resume 若整段落在兩次 probe 之間，兩端都是 Recording，只有
    /// generation 能證明中間真的發生過一段使用者要求的隱私空洞。
    last_pause_generation: Option<PauseGeneration>,
    /// 目前這一筆 session-local pause audit 的來源。除了避免舊 epoch 把新
    /// pause 關在開始時間之前，也把「控制面暫時讀不到」和真正 generation
    /// pause 分開，不讓兩種原因共用一個看似精確的 timestamp。
    active_pause_audit: Option<ActivePauseAudit>,
    /// 收到 pause request 後，到確定回到非暫停狀態前，剪貼簿中間有一段
    /// 不可觀察的隱私空洞。進、出兩端都要只推水位不讀內容；尤其 audit DB
    /// 失敗時，`paused` 本身不會變，不能拿它代替這個狀態。
    pause_clipboard_gap: bool,
    /// 和 `pause_clipboard_gap` 同一段使用者明確要求的空洞，但保護的是
    /// 尚未湊滿統計視窗、仍留在 input source 計數器裡的輸入節奏。
    pause_input_gap: bool,
    /// 排除畫面最後一次 tick 與下一次非排除 tick 之間也可能複製新內容。
    /// 離開排除時再推一次水位，才不會把那段尾巴撈進資料庫。
    exclusion_clipboard_gap: bool,
    /// OS 鎖定／睡眠／不可觀測期間的剪貼簿水位空洞。
    /// 回到 active 後還要再 skip 一次，才能讀新內容。
    system_clipboard_gap: bool,
    /// OS 鎖定／睡眠／不可觀測期間尚未清掉的輸入計數空洞。
    /// 回到 active 後仍要在 system post-check 通過時清一次尾端。
    system_input_gap: bool,
    /// 已經驗過且稽核列寫成的 OS 狀態／最後事件時間。
    /// 用來拒絕「狀態變了卻沒有 transition」與倒序／未來事件。
    last_system_state: Option<SystemContentState>,
    last_system_transition_ts: Option<Millis>,
    last_system_transition_sequence: Option<u64>,
    /// Source poll 已 consume，但 audit transaction 還沒成功的原生事件。
    /// 下一拍在再 poll source 之前先重試。
    pending_system: Option<PendingSystemObservation>,
    /// 畫面檔的根目錄。`None` = text-only 模式。
    image_dir: Option<PathBuf>,
    /// 全停來源在建構時必填；不能事後忘記接線而 fail-open。
    master_stop_source: MasterStopSource,
    /// 上一次**確實寫成稽核列**的全停狀態。不能直接拿 latch 當這個值：
    /// audit 寫失敗時必須留在舊狀態，下一拍才知道同一列還要重試。
    master_stopped: bool,
    /// 全停 request 到確實解除之間，剪貼簿與輸入來源各自有一段不可補撿的空洞。
    /// 和 pause 分開記，兩種停止重疊時解除其中一種才不會把另一種也打開。
    master_clipboard_gap: bool,
    master_input_gap: bool,
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
    /// 建立一般／replay／第三方 recorder。
    ///
    /// Backend 只能提供診斷名稱；即使名字逐字等於 Windows trusted 常數，這條
    /// public 路徑仍一律進 `untrusted/` namespace。Windows production recorder
    /// 由 `windows` 模組持有的 private token 走底下另一個 crate-private 入口。
    pub fn new(
        backend: B,
        db: Db,
        config: Config,
        image_dir: Option<PathBuf>,
        master_stop_source: MasterStopSource,
    ) -> Result<Self> {
        let identity = crate::backend_identity::BackendIdentity::untrusted(
            std::env::consts::OS,
            backend.name(),
        );
        Self::new_with_identity(backend, db, config, image_dir, master_stop_source, identity)
    }

    #[cfg(windows)]
    pub(crate) fn new_trusted_windows(
        backend: B,
        db: Db,
        config: Config,
        image_dir: Option<PathBuf>,
        master_stop_source: MasterStopSource,
        token: crate::windows::WindowsBackendToken,
    ) -> Result<Self> {
        let identity = crate::backend_identity::BackendIdentity::trusted_windows(token);
        Self::new_with_identity(backend, db, config, image_dir, master_stop_source, identity)
    }

    fn new_with_identity(
        backend: B,
        mut db: Db,
        config: Config,
        image_dir: Option<PathBuf>,
        master_stop_source: MasterStopSource,
        identity: crate::backend_identity::BackendIdentity,
    ) -> Result<Self> {
        let platform = identity.session_platform();
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
            last_pause_generation: None,
            active_pause_audit: None,
            pause_clipboard_gap: false,
            pause_input_gap: false,
            exclusion_clipboard_gap: false,
            system_clipboard_gap: false,
            system_input_gap: false,
            last_system_state: None,
            last_system_transition_ts: None,
            last_system_transition_sequence: None,
            pending_system: None,
            image_dir,
            master_stop_source,
            master_stopped: false,
            master_clipboard_gap: false,
            master_input_gap: false,
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

    /// 測試可在建構後切換 latch，以覆蓋同一台 recorder 的跨邊界狀態。
    /// Production 沒有這條接法；產品來源必須在建構時交出。
    #[cfg(test)]
    fn set_master_stop_dir(&mut self, dir: PathBuf) {
        self.master_stop_source = MasterStopSource::Latch(dir);
    }

    /// 把這一拍讀到的 durable 全停 latch 轉成 session-local audit。
    ///
    /// 和 pause 一樣，先封 source、再清掉較早的 pending native transition、寫成
    /// event 後才承認狀態改變。`stop-all --off` 若在 recorder 沒執行時發生，
    /// `MasterStopReleased` 仍沒有人能寫；這裡只記得到 recorder 親眼看見的邊界。
    fn set_master_stopped(&mut self, stopped: bool, ts: Millis) -> Result<bool> {
        if self.master_stopped == stopped {
            if !stopped {
                self.close_master_stop_gaps(ts);
            }
            return Ok(false);
        }
        if stopped {
            // latch 已經生效，稽核成敗都不能讓這一拍留下可跨越的 source 尾巴。
            self.seal_master_stop_gap(ts);
        }
        if self.pending_system.is_some() {
            self.commit_pending_system()
                .context("commit pending system transition audit before master-stop change")?;
        }
        self.db
            .insert_system(
                self.session_id,
                &SystemEvent {
                    ts,
                    kind: if stopped {
                        SystemKind::MasterStopEngaged
                    } else {
                        SystemKind::MasterStopReleased
                    },
                    detail: None,
                },
            )
            .context("write master-stop transition audit")?;
        self.master_stopped = stopped;
        if !stopped {
            // 稽核成功後、下一拍放行前，再切一次停止期間兩個 source 的尾巴。
            self.close_master_stop_gaps(ts);
        }
        Ok(true)
    }

    fn master_stop_tick(&mut self, ts: Millis) -> Result<Tick> {
        self.set_master_stopped(true, ts)?;
        self.stats.master_stopped_ticks += 1;
        let _ = self.establish_clipboard_watermark(ts);
        let _ = self.suspend_input_source(ts);
        Ok(Tick::MasterStopped)
    }

    /// 慢來源回來後先看 pending/latch；看見 request 就封掉所有 source 尾巴並丟棄
    /// 本拍 RAM。這裡不拿 boundary：engage 正在等這份 activity reader drop。
    fn master_postcheck(
        &mut self,
        ts: Millis,
        activity: &MasterActivity,
    ) -> Result<Option<Tick>> {
        match activity {
            MasterActivity::Guard(guard) if guard.stop_requested() => {
                self.master_stop_tick(ts).map(Some)
            }
            MasterActivity::Guard(_) | MasterActivity::NotApplicable => Ok(None),
        }
    }

    /// 真正寫 DB/PNG 前的 turnstile boundary。request 若剛好落在 postcheck 後，
    /// boundary 會輸；呼叫端不能把 `Stopped` 當成一個空 guard 繼續寫。
    fn master_commit_boundary(
        &mut self,
        ts: Millis,
        activity: &MasterActivity,
    ) -> Result<MasterBoundaryCheck> {
        match activity {
            MasterActivity::NotApplicable => Ok(MasterBoundaryCheck::Continue(
                MasterCommitGuard::NotApplicable,
            )),
            MasterActivity::Guard(guard) => match guard.boundary() {
                Some(boundary) => Ok(MasterBoundaryCheck::Continue(MasterCommitGuard::Guard {
                    _guard: boundary,
                })),
                None => {
                    self.master_stop_tick(ts)?;
                    Ok(MasterBoundaryCheck::Stopped)
                }
            },
        }
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
            if !paused {
                // 「pause audit 寫失敗後立刻取消」沒有 resume transition，但
                // request 到失敗之間仍是一段不能補撿的空洞。兩種 source 都在
                // 這裡封住尾端；失敗的那一種維持 gap，正常 tick 也不會讀它。
                self.close_pause_gaps(ts);
            }
            return Ok(false);
        }
        if paused {
            // 使用者按下去的那一刻就是邊界。必須排在 pending native audit
            // 重送之前：即使那筆 DB 寫入失敗、下一拍使用者又取消，這段期間
            // 的 clipboard/input 也不能在恢復後補進來。
            self.seal_pause_gap(ts);
        }
        // Source 在上一拍已交出的 native transition 必須排在這次真正的
        // 人為 pause/resume 之前。Pause snapshot handler 會在 tick 的第一道
        // gate 進這裡；若只在 observe_system 重試，就會寫成 pause/resume/lock。
        // 沒有 pause 狀態變化時不能在這裡偷吃 pending：那一拍仍須由 tick
        // 回 SystemChanged，禁止在 audit 重試成功後立刻讀內容。
        if self.pending_system.is_some() {
            if let Err(error) = self.commit_pending_system() {
                self.seal_system_gap(ts);
                return Err(error)
                    .context("commit pending system transition audit before pause change");
            }
        }
        // 稽核列寫成之後才承認狀態已切換。失敗時 self.paused 留在舊值，外部
        // 旗標若仍相異，下一拍會重試；若先改它，缺掉的 audit 永遠補不回來。
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
        self.paused = paused;
        if paused {
            self.active_pause_audit = Some(ActivePauseAudit {
                started_at: ts,
                generation: None,
            });
        } else {
            self.active_pause_audit = None;
            // 放行前最後再切一次。若使用者在 resume audit 寫入期間有動作，
            // 不能把它算進下一個正常視窗。來源暫時失敗就保留 gap；後面的
            // normal tick 仍 fail closed，呼叫端逐拍餵入的同一狀態會重試。
            self.close_pause_gaps(ts);
        }
        Ok(true)
    }

    /// 收尾：標記 session 結束，並且記下**為什麼**。
    ///
    /// 理由是必填的參數，不是 `Option`：這一步只有四條路走得到（見
    /// [`EndReason`]），而讓呼叫端有辦法說「不知道」，等於保證某一條路上
    /// 遲早會沒有人填。
    pub fn finish(&mut self, reason: EndReason) -> Result<()> {
        // 前一拍的 system source 可能已經交出 transition，但 audit transaction
        // 當時失敗。record loop 會在下一輪一開始重試；然而 duration／Ctrl-C／
        // 外部停止請求也在下一輪 tick 之前判斷，若直接收尾，行程明明還活著卻
        // 會把 RAM 裡的 pending observation 丟掉。先重送，再寫 SessionEnd，
        // 讓 audit 順序與「process-lifetime retry」的承諾一致。
        if self.pending_system.is_some() {
            self.commit_pending_system()
                .context("commit pending system transition audit before session end")?;
        }
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

    /// 封住一段 OS 狀態不可觀測或已知無法讀內容的空洞。
    ///
    /// 這些動作全部在稽核寫入前完成；後面的 DB 若剛好失敗，
    /// 剪貼簿 watermark 與 OCR baseline 仍然不會跨過隱私邊界。
    fn seal_system_gap(&mut self, ts: Millis) {
        self.deduper.reset();
        self.last_frame_id = None;
        self.backend.reset_ocr();
        self.system_clipboard_gap = true;
        self.system_input_gap = true;
        let _ = self.establish_clipboard_watermark(ts);
        let _ = self.suspend_input_source(ts);
        // 空洞兩端不能拿來推導 state transition。下一個 Known observation
        // 重新建立 state baseline；已接受 event 的時間仍是事實，不能清掉，
        // 否則 Unknown 後就能把倒序事件塞進 audit。
        self.last_system_state = None;
    }

    /// Privacy context 本身連讀都讀不出來時的 fail-closed 邊界。
    fn seal_privacy_gap(&mut self, ts: Millis) {
        self.deduper.reset();
        self.last_frame_id = None;
        self.backend.reset_ocr();
        let _ = self.establish_clipboard_watermark(ts);
        self.exclusion_clipboard_gap = true;
    }

    /// 使用者要求暫停時，畫面/OCR 與兩個會累積到下一拍的 source 一起斷開。
    /// flag 先立起來：即使 source 當下清不掉，恢復後也不會誤讀。
    fn seal_pause_gap(&mut self, ts: Millis) {
        self.deduper.reset();
        self.last_frame_id = None;
        self.backend.reset_ocr();
        self.pause_clipboard_gap = true;
        self.pause_input_gap = true;
        let _ = self.establish_clipboard_watermark(ts);
        let _ = self.suspend_input_source(ts);
    }

    fn seal_master_stop_gap(&mut self, ts: Millis) {
        self.deduper.reset();
        self.last_frame_id = None;
        self.backend.reset_ocr();
        self.master_clipboard_gap = true;
        self.master_input_gap = true;
        let _ = self.establish_clipboard_watermark(ts);
        let _ = self.suspend_input_source(ts);
    }

    fn close_pause_gaps(&mut self, ts: Millis) {
        if self.pause_clipboard_gap && self.establish_clipboard_watermark(ts) {
            self.pause_clipboard_gap = false;
        }
        self.close_pause_input_gap(ts);
    }

    fn close_master_stop_gaps(&mut self, ts: Millis) {
        if self.master_clipboard_gap && self.establish_clipboard_watermark(ts) {
            self.master_clipboard_gap = false;
        }
        self.close_master_stop_input_gap(ts);
    }

    fn establish_clipboard_watermark(&mut self, ts: Millis) -> bool {
        match self.backend.skip_clipboard(ts) {
            Ok(ClipboardWatermark::Established) => true,
            Ok(ClipboardWatermark::Unknown) => {
                self.stats.clipboard_watermark_unknown += 1;
                false
            }
            Err(error) => {
                self.stats.clipboard_watermark_unknown += 1;
                tracing::warn!(error = %error, "clipboard watermark unavailable; keeping clipboard fail closed");
                false
            }
        }
    }

    fn suspend_input_source(&mut self, ts: Millis) -> bool {
        match self.backend.suspend_input(ts) {
            Ok(()) => true,
            Err(error) => {
                tracing::warn!(error = %error, "input source could not suspend; keeping input fail closed");
                false
            }
        }
    }

    /// 關掉 pause 這一個 reason；若沒有別的 reason，才真的讓 source 恢復。
    fn close_pause_input_gap(&mut self, ts: Millis) {
        if !self.pause_input_gap || !self.suspend_input_source(ts) {
            return;
        }
        self.pause_input_gap = false;
        if !self.system_input_gap && !self.master_input_gap {
            if let Err(error) = self.backend.resume_input(ts) {
                self.pause_input_gap = true;
                tracing::warn!(error = %error, "input source could not resume after pause; keeping input fail closed");
            }
        }
    }

    /// 關掉 system 這一個 reason；和 pause 可以重疊，兩者都清掉才 resume。
    fn close_system_input_gap(&mut self, ts: Millis) {
        if !self.system_input_gap || !self.suspend_input_source(ts) {
            return;
        }
        self.system_input_gap = false;
        if !self.pause_input_gap && !self.master_input_gap {
            if let Err(error) = self.backend.resume_input(ts) {
                self.system_input_gap = true;
                tracing::warn!(error = %error, "input source could not resume after system gap; keeping input fail closed");
            }
        }
    }

    /// 關掉 master-stop 這一個 reason；pause／system 任一仍在就不開 source。
    fn close_master_stop_input_gap(&mut self, ts: Millis) {
        if !self.master_input_gap || !self.suspend_input_source(ts) {
            return;
        }
        self.master_input_gap = false;
        if !self.pause_input_gap && !self.system_input_gap {
            if let Err(error) = self.backend.resume_input(ts) {
                self.master_input_gap = true;
                tracing::warn!(error = %error, "input source could not resume after master stop; keeping input fail closed");
            }
        }
    }

    /// 把剛寫成的 session-local pause audit 綁到真正觀察到的 generation。
    /// 只在這一次確實由該 snapshot 觸發新 audit 時呼叫；Indeterminate 所造的
    /// fail-closed pause 不能事後偷綁到任意一份舊 state。
    fn bind_active_pause_generation(&mut self, generation: PauseGeneration) {
        let active = self
            .active_pause_audit
            .as_mut()
            .expect("a successful pause transition creates an active audit");
        active.generation = Some(generation);
    }

    /// 從 persisted epoch 取 resume 邊界，但只有它確實屬於目前 audit 那一代、
    /// 且沒有跑到 pause 之前時才信。控制檔暫時讀不到所造的 synthetic pause
    /// 沒有 generation；恢復時用這一次真的觀察時刻，不拿歷史 epoch 冒充。
    fn resume_audit_ts(
        &self,
        epoch: sister_core::pause::PauseEpoch,
        observed_at: Millis,
    ) -> Millis {
        let Some(active) = self.active_pause_audit else {
            return observed_at;
        };
        epoch
            .resumed_at()
            .filter(|resumed_at| {
                active.generation == Some(epoch.generation()) && *resumed_at >= active.started_at
            })
            .unwrap_or_else(|| observed_at.max(active.started_at))
    }

    fn observe_pause_request(
        &mut self,
        probe: &mut dyn FnMut() -> PauseSignal,
    ) -> Result<PauseCheck> {
        let started = Instant::now();
        let signal = probe();
        self.timings.pause_probe.record(started.elapsed());
        match signal {
            // Replay／直接測試沒有跨行程 state。Recording 不能順便 resume：
            // 呼叫端若先用 set_paused(true)，普通 tick 必須一直停到它明確解除。
            PauseSignal::Recording => Ok(if self.paused {
                PauseCheck::Paused
            } else {
                PauseCheck::Continue(None)
            }),
            PauseSignal::Paused { observed_at } => {
                if !self.paused {
                    // set_paused 會先封 OCR/clipboard/input，再嘗試寫 audit。即使
                    // DB 失敗，這一拍暫存在 RAM 的值也會隨 stack 一起丟掉。
                    self.set_paused(true, observed_at)?;
                }
                Ok(PauseCheck::Paused)
            }
            PauseSignal::Snapshot { observed_at, guard } => {
                match guard.snapshot() {
                    PauseSnapshot::Indeterminate => {
                        // 讀不到 control state 不是「沒按暫停」。先封住 sources 並
                        // 寫出 audit；generation 不冒充 0，等有效快照回來再接軌。
                        if !self.paused {
                            self.set_paused(true, observed_at)?;
                        }
                        Ok(PauseCheck::Paused)
                    }
                    PauseSnapshot::Paused(epoch) => {
                        let generation = epoch.generation();
                        let started_now = !self.paused;
                        if !self.paused {
                            // 第一份快照只建立這一場 recorder 的 baseline。Control
                            // 可能在本 session 開始前已暫停數小時；把舊時戳寫進
                            // 新 session，會讓 audit 看起來早於 session_start。
                            let paused_at = if self.last_pause_generation.is_none() {
                                observed_at
                            } else {
                                epoch.paused_at().unwrap_or(observed_at)
                            };
                            self.set_paused(true, paused_at)?;
                        }
                        if started_now {
                            self.bind_active_pause_generation(generation);
                        }
                        // 有狀態轉換時一定要等 audit 成功才推 watermark；失敗會
                        // 從 `?` 離開，下一拍看見同一代便會重試。
                        self.last_pause_generation = Some(generation);
                        Ok(PauseCheck::Paused)
                    }
                    PauseSnapshot::Recording(epoch) => {
                        let generation = epoch.generation();
                        let generation_changed = self
                            .last_pause_generation
                            .is_some_and(|previous| previous != generation);

                        if generation_changed && !self.paused {
                            // 兩次 probe 看起來都是 Recording，但 generation 換了：
                            // 完整 pause→resume 曾落在中間。先補 pause audit、丟掉
                            // 本拍所有 staged content；下一拍再補 resume，空洞才不
                            // 會被說成「什麼都沒發生」。
                            self.set_paused(true, epoch.paused_at().unwrap_or(observed_at))?;
                            self.bind_active_pause_generation(generation);
                            self.last_pause_generation = Some(generation);
                            return Ok(PauseCheck::Paused);
                        }

                        let resumed_now = self.paused;
                        if resumed_now {
                            // 同代 Recording 是正常 resume；換代但 recorder 本來
                            // 就停著，代表幾段 pause 之間沒有任何可寫內容，合併成
                            // 一段連續空洞，在最後一次 resume 邊界才重新開 source。
                            let resumed_at = self.resume_audit_ts(epoch, observed_at);
                            self.set_paused(false, resumed_at)?;
                        } else {
                            // Resume 後的 source watermark/suspend cleanup 可能暫時
                            // 失敗；同一個 Recording 快照要逐拍重試，不能因 recorder
                            // bool 已經是 false 就讓 gap 永遠卡住。
                            self.set_paused(false, observed_at)?;
                        }
                        self.last_pause_generation = Some(generation);
                        if resumed_now {
                            // `observed_at`／persisted resumed_at 可能晚於這一拍在
                            // 呼叫端先取好的 tick timestamp。這一拍只寫 resume
                            // audit；若立刻讓內容沿用舊 ts，timeline 會把內容排在
                            // 使用者解除暫停之前。
                            Ok(PauseCheck::Resumed)
                        } else {
                            Ok(PauseCheck::Continue(Some(guard)))
                        }
                    }
                }
            }
        }
    }

    fn validate_system_observation(
        &self,
        tick_ts: Millis,
        state: SystemContentState,
        transitions: &[crate::traits::SystemTransition],
    ) -> Result<()> {
        let mut implied = self.last_system_state;
        let mut last_ts = self.last_system_transition_ts;
        let mut last_sequence = self.last_system_transition_sequence;

        for transition in transitions {
            if transition.sequence == 0 {
                anyhow::bail!("system transition sequence must start above zero");
            }
            let expected_sequence = match last_sequence {
                Some(previous) => previous
                    .checked_add(1)
                    .context("system transition sequence exhausted")?,
                None => 1,
            };
            if transition.sequence != expected_sequence {
                anyhow::bail!(
                    "system transition sequence has a gap, repeat, or reordering: event={} expected={} previous={:?}",
                    transition.sequence,
                    expected_sequence,
                    last_sequence
                );
            }
            if transition.ts > tick_ts {
                anyhow::bail!(
                    "system transition is from the future: event={} tick={tick_ts}",
                    transition.ts
                );
            }
            if last_ts.is_some_and(|previous| transition.ts < previous) {
                anyhow::bail!(
                    "system transitions are out of order: event={} previous={}",
                    transition.ts,
                    last_ts.expect("checked Some")
                );
            }
            if let Some(current) = implied {
                implied = Some(current.checked_applying(transition.kind).with_context(|| {
                    format!(
                        "system transition {:?} did not change its state dimension",
                        transition.kind
                    )
                })?);
            }
            last_ts = Some(transition.ts);
            last_sequence = Some(transition.sequence);
        }

        if self.last_system_state.is_none() && !transitions.is_empty() {
            anyhow::bail!(
                "first/recovered system observation must establish a baseline without transitions"
            );
        }

        match (self.last_system_state, transitions.is_empty(), implied) {
            (Some(previous), true, _) if previous != state => anyhow::bail!(
                "system state changed from {previous:?} to {state:?} without a transition"
            ),
            (_, false, Some(observed)) if observed != state => anyhow::bail!(
                "system transitions imply {observed:?}, but source reported {state:?}"
            ),
            _ => Ok(()),
        }
    }

    fn commit_pending_system(&mut self) -> Result<SystemGate> {
        let pending = self
            .pending_system
            .as_ref()
            .context("commit pending system observation without one")?;
        let events: Vec<SystemEvent> = pending
            .transitions
            .iter()
            .map(|transition| SystemEvent {
                ts: transition.ts,
                kind: transition.kind.core_kind(),
                detail: None,
            })
            .collect();
        self.db
            .insert_system_batch(self.session_id, &events)
            .context("commit system transition audit batch")?;
        let pending = self
            .pending_system
            .take()
            .expect("pending remained until transaction committed");
        self.last_system_state = Some(pending.state);
        if let Some(last) = pending.transitions.last() {
            self.last_system_transition_ts = Some(last.ts);
            self.last_system_transition_sequence = Some(last.sequence);
        }
        Ok(pending.gate)
    }

    /// Poll、驗 observation invariant、封邊界、用單一 transaction 寫 audit，
    /// 然後才回答這一拍可不可繼續。這個共同處理器同時用於第一次
    /// poll 和 privacy/UIA 後的 revalidation，避免第二條路少一道驗證。
    fn observe_system(&mut self, ts: Millis) -> Result<SystemGate> {
        // 上一拍的 source 已 consume event；重試 audit 成功前不再 poll，
        // 也不讀任何內容。
        if self.pending_system.is_some() {
            if let Err(error) = self.commit_pending_system() {
                self.seal_system_gap(ts);
                return Err(error);
            }
            return Ok(SystemGate::PendingCommitted);
        }

        let started = Instant::now();
        let observation_result = self.backend.poll_system(ts);
        self.timings.system_poll.record(started.elapsed());
        let observation = match observation_result {
            Ok(observation) => observation,
            Err(error) => {
                self.seal_system_gap(ts);
                return Err(error).context("read system state before content");
            }
        };
        let (state, transitions) = match observation {
            SystemObservation::Known { state, transitions } => (state, transitions),
            SystemObservation::Unknown => {
                self.seal_system_gap(ts);
                self.stats.system_unknown += 1;
                return Ok(SystemGate::Unknown);
            }
        };

        if let Err(error) = self.validate_system_observation(ts, state, &transitions) {
            self.seal_system_gap(ts);
            return Err(error).context("validate system observation before content");
        }

        let transitioned = !transitions.is_empty();
        if transitioned || !state.allows_content() {
            self.seal_system_gap(ts);
        }

        // 先把 source 已 consume 的完整 observation 收到 recorder-owned pending。
        // state／ts／sequence watermark 全都要等 audit transaction 成功才一次
        // commit；DB 失敗時下拍先 retry，期間禁止再 poll/content。
        let gate = if transitioned {
            SystemGate::Transitioned
        } else if state.allows_content() {
            SystemGate::Stable
        } else {
            SystemGate::Blocked
        };
        self.pending_system = Some(PendingSystemObservation {
            state,
            transitions,
            gate,
        });
        if let Err(error) = self.commit_pending_system() {
            self.seal_system_gap(ts);
            return Err(error);
        }
        Ok(gate)
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
        self.tick_with_pause_probe(ts, || PauseSignal::Recording)
    }

    /// 和 [`Self::tick`] 相同，但在每個可能很慢的內容來源後重讀跨行程暫停旗標。
    ///
    /// `probe` 的 `Recording` 必須表示呼叫端**確定**沒有 pause request；讀旗標
    /// 失敗要在呼叫端先依 fail-closed 規則轉成 `Paused`。Windows 長時間錄製
    /// 用這條；replay／單元測試仍可用沒有外部狀態的 `tick`。
    pub fn tick_with_pause_probe(
        &mut self,
        ts: Millis,
        mut probe: impl FnMut() -> PauseSignal,
    ) -> Result<Tick> {
        let t = Instant::now();
        let out = self.tick_inner(ts, &mut probe);
        self.timings.tick.record(t.elapsed());
        if let Err(e) = &out {
            self.stats.tick_failures += 1;
            // `{e:#}` 帶上整條 anyhow context 鏈，和 OCR / 存圖那兩個一樣。
            self.stats.last_tick_error = Some(format!("{e:#}"));
        }
        out
    }

    fn tick_inner(
        &mut self,
        ts: Millis,
        pause_probe: &mut dyn FnMut() -> PauseSignal,
    ) -> Result<Tick> {
        self.stats.ticks += 1;

        if !self.config.capture.enabled {
            return Ok(Tick::Disabled);
        }

        // Production tick 在碰任何 live source 前取得一份真正活著的 shared file lock。
        // `NotApplicable` 是 replay/單測的明確分支，不可偽造一份空 guard。
        let master_activity = match self.master_stop_source.clone() {
            MasterStopSource::Latch(data_dir) => {
                let Some(guard) = sister_hands::master_stop::admit(&data_dir) else {
                    return self.master_stop_tick(ts);
                };
                MasterActivity::Guard(guard)
            }
            MasterStopSource::NotApplicable => MasterActivity::NotApplicable,
        };
        let master_changed = self.set_master_stopped(false, ts)?;
        if master_changed {
            self.stats.master_released_ticks += 1;
            return Ok(Tick::MasterReleased);
        }

        // Production snapshot 也必須在已暫停時讀：這是 recorder 看見 resume，
        // 以及看見「pause→resume 整段發生在上一拍內」generation 變化的地方。
        // 普通 `tick()` 的 Recording 則不會擅自解除手動 set_paused。
        match self.observe_pause_request(pause_probe)? {
            PauseCheck::Paused => {
                // 和排除同一個理由，而且更嚴重：使用者是**故意**在這段時間裡去複製
                // 一個他不想被記住的東西。水位不推過去的話，他一解除暫停，那個東西
                // 就在下一個 tick 被撈進來——暫停等於只是延後了洩漏。
                //
                // `skip` 不讀內容，只把水位推過去，所以這一行讓她更看不到、
                // 不是更看得到。
                let _ = self.establish_clipboard_watermark(ts);
                // 排除那邊會繼續累積輸入節奏（節奏不含內容，斷了會在訊號上開洞）。
                // 這裡**不**累積：排除講的是「這個畫面不能看」，暫停講的是
                // 「現在不要記錄我」——後者連沒有內容的節奏都不該留下。
                let _ = self.suspend_input_source(ts);
                return Ok(Tick::Paused);
            }
            PauseCheck::Resumed => return Ok(Tick::Resumed),
            // Entry snapshot 只保護這個判定；不能把 shared lock 帶進 UIA/OCR，
            // 否則使用者按暫停會被一個數秒鐘的讀取擋住。
            PauseCheck::Continue(_) => {}
        }
        debug_assert!(!self.paused);
        // 過了上面兩道門才算「這一拍真的要做事」。摘要裡的每拍成本和閒置
        // 比例都拿它當分母——見 `working_ticks`。
        self.stats.working_ticks += 1;

        // 1) 先問 OS 生命週期。它必須排在 focus／clipboard／input／
        // screen 全部內容來源之前；不知道是鎖著還是醒著時就停。
        match self.observe_system(ts)? {
            SystemGate::Stable => {}
            SystemGate::Transitioned => return Ok(Tick::SystemChanged),
            SystemGate::Blocked => {
                self.stats.no_screen += 1;
                return Ok(Tick::NoScreen);
            }
            SystemGate::Unknown => return Ok(Tick::SystemUnknown),
            SystemGate::PendingCommitted => return Ok(Tick::SystemChanged),
        }

        // 2) 先看 privacy context。這一步很便宜，而且是排除判定的依據。
        let t = Instant::now();
        let privacy_observation = match self.backend.privacy_context(ts) {
            Ok(context) => context,
            Err(error) => {
                // 不只是直接 `?`：剪貼簿的 watermark 和 OCR baseline 也必須
                // 在離開前封住，否則下一拍復原時會把這段秘密補撿回來。
                self.seal_privacy_gap(ts);
                self.timings.focus.record(t.elapsed());
                return Err(error).context("read privacy context before content");
            }
        };
        self.timings.focus.record(t.elapsed());
        if let Some(tick) = self.master_postcheck(ts, &master_activity)? {
            return Ok(tick);
        }
        let (privacy_context, permit) = match privacy_observation {
            PrivacyObservation::Known { context, permit } => (context, Some(permit)),
            PrivacyObservation::Unknown => (sister_core::model::PrivacyContext::Unknown, None),
        };

        // 3) 排除判定——必須在任何截圖或剪貼簿讀取之前。
        let exclusion = self.config.privacy.check(&privacy_context);

        // 網址擷取到底有沒有在運作，只有這裡看得到。數在排除判定**之前**：
        // 一個被 `excluded_apps` 擋掉的瀏覽器同樣證明了 UIA 讀不讀得到，
        // 而且往下走的路上這個 snapshot 就不見了。
        if let Some(focus) = privacy_context.focus() {
            if crate::browsers::is_browser(&focus.app_key()) {
                self.stats.browser_ticks += 1;
                if focus.url.is_some() {
                    self.stats.url_reads += 1;
                }
            }
        }

        if let Some(boundary) = pause_boundary_tick(self.observe_pause_request(pause_probe)?) {
            return Ok(boundary);
        }

        // Privacy/UIA 可能很慢；不論它最後回答 Allowed、Excluded 或 Unknown，
        // 在任何 exclusion audit 或 input drain 之前都要重驗 OS。否則鎖屏若
        // 發生在 UIA 期間，排除分支會比 lock audit 更早寫一列輸入節奏。
        match self.observe_system(ts)? {
            SystemGate::Stable => {}
            SystemGate::Transitioned | SystemGate::PendingCommitted => {
                return Ok(Tick::SystemChanged);
            }
            SystemGate::Blocked => {
                self.stats.no_screen += 1;
                return Ok(Tick::NoScreen);
            }
            SystemGate::Unknown => return Ok(Tick::SystemUnknown),
        }
        if self.system_clipboard_gap && self.establish_clipboard_watermark(ts) {
            self.system_clipboard_gap = false;
        }
        self.close_system_input_gap(ts);

        if let Some(reason) = exclusion.reason() {
            // 這三步是 privacy gate 本身，必須排在任何可能失敗的稽核寫入前。
            // 第一次進排除時若 DB 剛好寫不進去，仍然不准跨過這段盲區拼舊 OCR，
            // 更不准把這裡複製的剪貼簿留給下一個未排除 tick 去讀。
            self.deduper.reset();
            self.last_frame_id = None;
            self.backend.reset_ocr();
            let _ = self.establish_clipboard_watermark(ts);
            self.exclusion_clipboard_gap = true;

            // 排除期間仍保留不含內容的節奏，但先只 drain 到 RAM。系統可能
            // 正好在 atomic drain 時鎖定；下一道 post-check 不通過就整份丟掉。
            let staged_input =
                if !self.pause_input_gap && !self.master_input_gap && !self.system_input_gap {
                    self.stage_input(ts)?
                } else {
                    None
                };
            if let Some(tick) = self.master_postcheck(ts, &master_activity)? {
                return Ok(tick);
            }
            if let Some(boundary) = pause_boundary_tick(self.observe_pause_request(pause_probe)?) {
                return Ok(boundary);
            }
            match self.observe_system(ts)? {
                SystemGate::Stable => {}
                SystemGate::Transitioned | SystemGate::PendingCommitted => {
                    return Ok(Tick::SystemChanged);
                }
                SystemGate::Blocked => {
                    self.stats.no_screen += 1;
                    return Ok(Tick::NoScreen);
                }
                SystemGate::Unknown => return Ok(Tick::SystemUnknown),
            }
            let pause_commit_guard = match self.observe_pause_request(pause_probe)? {
                PauseCheck::Continue(guard) => guard,
                PauseCheck::Paused => return Ok(Tick::Paused),
                PauseCheck::Resumed => return Ok(Tick::Resumed),
            };
            let master_commit_guard = match self.master_commit_boundary(ts, &master_activity)? {
                MasterBoundaryCheck::Continue(guard) => guard,
                MasterBoundaryCheck::Stopped => return Ok(Tick::MasterStopped),
            };

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
            self.commit_input(staged_input)?;
            // Exclusion audit/input persistence is now entirely before a waiting pause
            // writer, or entirely after it. Do not carry the lock into the next tick.
            drop(pause_commit_guard);
            drop(master_commit_guard);
            return Ok(Tick::Excluded {
                reason: reason.to_string(),
            });
        }
        if self.exclusion_clipboard_gap {
            // 排除 tick 最後一次 skip 後，使用者仍可能再複製一次才切回普通視窗。
            // 先在非排除邊界推過那段尾巴，才准下面的 poll 讀內容。
            if self.establish_clipboard_watermark(ts) {
                self.exclusion_clipboard_gap = false;
            }
        }

        // `Allowed` 只有 Known + SensitiveFieldState::Clear 可以產生。不用
        // default 當退路；若日後改了 privacy gate 卻漏了這個契約，當場停。
        let focus = privacy_context
            .into_focus()
            .context("privacy gate allowed an unknown context")?;
        let permit = permit.context("privacy gate allowed a context without a capture permit")?;

        // 5) 剪貼簿。只有在沒被排除時才碰——不然密碼管理員裡複製的
        //    密碼會從這裡漏進資料庫。
        let clipboard_ready = !self.pause_clipboard_gap
            && !self.master_clipboard_gap
            && !self.exclusion_clipboard_gap
            && !self.system_clipboard_gap;
        let staged_clipboard = if clipboard_ready {
            self.stage_clipboard(ts, permit)?
        } else {
            ClipboardStage::Ready(None)
        };
        let staged_clipboard = match staged_clipboard {
            ClipboardStage::Ready(event) => event,
            ClipboardStage::ContextChanged => {
                self.seal_privacy_gap(ts);
                self.stats.context_changed += 1;
                return Ok(Tick::ContextChanged);
            }
        };
        let staged_input =
            if !self.pause_input_gap && !self.master_input_gap && !self.system_input_gap {
                self.stage_input(ts)?
            } else {
                None
            };
        if let Some(tick) = self.master_postcheck(ts, &master_activity)? {
            return Ok(tick);
        }
        if let Some(boundary) = pause_boundary_tick(self.observe_pause_request(pause_probe)?) {
            return Ok(boundary);
        }

        // Clipboard/input 讀取都可能跨過 OS lock；即使這拍因 watermark gap
        // 根本沒讀其中一個，system poll 與 UIA 也可能已經跨過 lock／前景
        // 切換。所以最後的 system + permit 驗證無條件執行。不是 Stable 就
        // 丟掉 RAM 裡的 staged 值，focus/clipboard/input 都不落 DB。
        match self.observe_system(ts)? {
            SystemGate::Stable => {}
            SystemGate::Transitioned | SystemGate::PendingCommitted => {
                return Ok(Tick::SystemChanged);
            }
            SystemGate::Blocked => {
                self.stats.no_screen += 1;
                return Ok(Tick::NoScreen);
            }
            SystemGate::Unknown => return Ok(Tick::SystemUnknown),
        }
        match self.backend.capture_permit_is_current(permit) {
            Ok(true) => {}
            Ok(false) => {
                self.seal_privacy_gap(ts);
                self.stats.context_changed += 1;
                return Ok(Tick::ContextChanged);
            }
            Err(error) => {
                self.seal_privacy_gap(ts);
                return Err(error).context("revalidate capture permit before persistence");
            }
        }
        if let Some(tick) = self.master_postcheck(ts, &master_activity)? {
            return Ok(tick);
        }
        let pause_commit_guard = match self.observe_pause_request(pause_probe)? {
            PauseCheck::Continue(guard) => guard,
            PauseCheck::Paused => return Ok(Tick::Paused),
            PauseCheck::Resumed => return Ok(Tick::Resumed),
        };
        let master_commit_guard = match self.master_commit_boundary(ts, &master_activity)? {
            MasterBoundaryCheck::Continue(guard) => guard,
            MasterBoundaryCheck::Stopped => return Ok(Tick::MasterStopped),
        };

        // Clipboard 的 permit + system post-check 通過後才記 focus/event。
        self.last_exclusion = None;
        let context_changed = self.record_focus_if_changed(ts, &focus)?;
        if let Some(event) = staged_clipboard {
            self.redact_and_store_clipboard(event)?;
        }
        self.commit_input(staged_input)?;
        // 只保護上面這組 content writes；idle/screen/OCR 都可能很慢，不能拿
        // shared lock 包住它們，否則暫停按鈕會卡在 recorder 的工作後面。
        drop(pause_commit_guard);
        drop(master_commit_guard);

        // 6) 先問一個不用碰螢幕就答得出來的問題：有人動過嗎？
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

        // 7) 讀一次螢幕。**只讀一次。**
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
        let grabbed = self.backend.grab_screen(ts, permit);
        self.timings.grab.record(t.elapsed());
        if let Some(tick) = self.master_postcheck(ts, &master_activity)? {
            return Ok(tick);
        }

        // 這條路上的每一個退出點都不必手動退回去重基準：`check` 不會推進
        // 它，推進的是 `keep_frame` 裡真的存完之後那一句 `kept`。這裡曾經
        // 有三個地方各自漏掉那個退回動作（鎖屏、抓圖失敗、資料庫寫不進
        // 去），所以問題不在漏了三次，在於形狀。
        let frame = match grabbed {
            Ok(ScreenCapture::Frame(frame)) => frame,
            Ok(ScreenCapture::Unavailable) => {
                self.stats.no_screen += 1;
                return Ok(Tick::NoScreen);
            }
            Ok(ScreenCapture::PrivacyChanged) => {
                // BitBlt 可能已經把 pixels 帶進工作 buffer；typed outcome 確保
                // 它在這裡丟掉，不進 dedup/OCR/DB/PNG。
                self.seal_privacy_gap(ts);
                self.stats.context_changed += 1;
                return Ok(Tick::ContextChanged);
            }
            Err(e) => return Err(e),
        };

        if let Some(boundary) = pause_boundary_tick(self.observe_pause_request(pause_probe)?) {
            return Ok(boundary);
        }

        // Frame 可能已在 BitBlt 工作 buffer，但還沒有進 dedup/OCR/DB/PNG。
        // 再驗 OS 與 permit；任一失效就在這裡 drop buffer。
        match self.observe_system(ts)? {
            SystemGate::Stable => {}
            SystemGate::Transitioned | SystemGate::PendingCommitted => {
                return Ok(Tick::SystemChanged);
            }
            SystemGate::Blocked => {
                self.stats.no_screen += 1;
                return Ok(Tick::NoScreen);
            }
            SystemGate::Unknown => return Ok(Tick::SystemUnknown),
        }
        match self.backend.capture_permit_is_current(permit) {
            Ok(true) => {}
            Ok(false) => {
                self.seal_privacy_gap(ts);
                self.stats.context_changed += 1;
                return Ok(Tick::ContextChanged);
            }
            Err(error) => {
                self.seal_privacy_gap(ts);
                return Err(error).context("revalidate capture permit after screen capture");
            }
        }
        if let Some(tick) = self.master_postcheck(ts, &master_activity)? {
            return Ok(tick);
        }
        if let Some(boundary) = pause_boundary_tick(self.observe_pause_request(pause_probe)?) {
            return Ok(boundary);
        }
        self.stats.last_frame_size = Some((frame.width, frame.height));

        match self.deduper.check(frame.dhash) {
            FrameVerdict::Duplicate { run } => {
                if self.config.capture.ocr {
                    match self.backend.recheck_ocr_dhash_duplicate(&frame) {
                        DhashRecheck::Changed(attempt) => {
                            return self.keep_frame(
                                ts,
                                frame,
                                focus,
                                Some(attempt),
                                pause_probe,
                                &master_activity,
                            );
                        }
                        DhashRecheck::Duplicate {
                            gate_elapsed,
                            work,
                            rejected_regions,
                            error,
                        } => {
                            self.record_ocr_measurements(
                                &frame,
                                rejected_regions.is_some() || error.is_some(),
                                gate_elapsed,
                                work,
                            );
                            if let Some(regions) = rejected_regions {
                                self.stats.ocr_rejected_region_frames += 1;
                                self.stats.ocr_rejected_regions += regions.get();
                            }
                            if let Some(e) = error {
                                self.stats.ocr_failures += 1;
                                self.stats.last_ocr_error = Some(format!("{e:#}"));
                                tracing::warn!(
                                    error = %e,
                                    "OCR recheck failed; keeping the dHash duplicate"
                                );
                            }
                        }
                    }
                }
                if let Some(tick) = self.master_postcheck(ts, &master_activity)? {
                    return Ok(tick);
                }
                let pause_commit_guard = match self.observe_pause_request(pause_probe)? {
                    PauseCheck::Continue(guard) => guard,
                    PauseCheck::Paused => return Ok(Tick::Paused),
                    PauseCheck::Resumed => return Ok(Tick::Resumed),
                };
                let master_commit_guard =
                    match self.master_commit_boundary(ts, &master_activity)? {
                        MasterBoundaryCheck::Continue(guard) => guard,
                        MasterBoundaryCheck::Stopped => return Ok(Tick::MasterStopped),
                    };
                if let Some(id) = self.last_frame_id {
                    self.db.bump_frame_dup(id)?;
                }
                // dHash 的判斷本身不提交 run。只有 DB 計數也寫成後，這一拍
                // 才正式成為重複；changed-region 若升格或 DB 失敗都不必回滾。
                self.deduper.duplicate();
                self.stats.duplicates += 1;
                drop(pause_commit_guard);
                drop(master_commit_guard);
                Ok(Tick::Duplicate { run })
            }
            FrameVerdict::New => {
                self.keep_frame(ts, frame, focus, None, pause_probe, &master_activity)
            }
        }
    }

    fn record_ocr_measurements(
        &mut self,
        frame: &RawFrame,
        full_candidate: bool,
        gate_elapsed: Option<Duration>,
        work: Option<OcrWork>,
    ) {
        if full_candidate {
            self.stats.ocr_candidate_pixels = self
                .stats
                .ocr_candidate_pixels
                .saturating_add(u64::from(frame.width) * u64::from(frame.height));
        }
        if let Some(elapsed) = gate_elapsed {
            self.timings.ocr_gate.record(elapsed);
        }
        if let Some(work) = work {
            self.timings.ocr.record_many(work.elapsed(), work.calls());
            self.stats.ocr_input_pixels = self
                .stats
                .ocr_input_pixels
                .saturating_add(work.input_pixels());
        }
    }

    fn keep_frame(
        &mut self,
        ts: Millis,
        frame: RawFrame,
        focus: FocusSnapshot,
        prepared_ocr: Option<OcrAttempt>,
        pause_probe: &mut dyn FnMut() -> PauseSignal,
        master_activity: &MasterActivity,
    ) -> Result<Tick> {
        let (ocr, ocr_committable) = if self.config.capture.ocr {
            // OCR 失敗不擋錄製，但要留下計數——見 `RecorderStats::ocr_failures`
            let attempt = prepared_ocr.unwrap_or_else(|| self.backend.recognize(&frame));
            self.record_ocr_measurements(&frame, true, attempt.gate_elapsed, attempt.work);
            match attempt.outcome {
                Ok(crate::traits::OcrOutcome::Full { blocks, fallback }) => {
                    self.stats.ocr_full_frames += 1;
                    self.stats.ocr_full_fallbacks += u64::from(fallback);
                    (blocks, true)
                }
                Ok(crate::traits::OcrOutcome::Regions { blocks, regions }) => {
                    self.stats.ocr_region_frames += 1;
                    self.stats.ocr_regions += regions.get();
                    (blocks, true)
                }
                Ok(crate::traits::OcrOutcome::Reused { blocks }) => {
                    self.stats.ocr_reused_frames += 1;
                    (blocks, true)
                }
                Err(e) => {
                    self.stats.ocr_failures += 1;
                    // `{e:#}` 帶上整條 anyhow context 鏈。只留最外層那句的話，
                    // 使用者回報的會是「OCR 失敗」這種等於沒說的訊息。
                    self.stats.last_ocr_error = Some(format!("{e:#}"));
                    tracing::warn!(error = %e, "OCR failed; keeping the frame without text");
                    (Vec::new(), false)
                }
            }
        } else {
            debug_assert!(prepared_ocr.is_none());
            (Vec::new(), false)
        };

        if let Some(tick) = self.master_postcheck(ts, master_activity)? {
            return Ok(tick);
        }

        // OCR 是這條路最慢的一步（真機約 2.4 秒）。pause 若在它執行期間到達，
        // 先丟掉辨識結果與 frame，再談 PNG encode；不能讓另一個慢步驟把反應再拖長。
        if let Some(boundary) = pause_boundary_tick(self.observe_pause_request(pause_probe)?) {
            return Ok(boundary);
        }

        let prepared_image = self.prepare_image(ts, &frame);

        // encode 只建立 RAM buffer。這道檢查後才准碰 PNG／DB；production 會把
        // 同一次 snapshot 的 shared lock 持有到兩者都 commit 完成。
        let pause_commit_guard = match self.observe_pause_request(pause_probe)? {
            PauseCheck::Continue(guard) => guard,
            PauseCheck::Paused => return Ok(Tick::Paused),
            PauseCheck::Resumed => return Ok(Tick::Resumed),
        };
        let master_commit_guard = match self.master_commit_boundary(ts, master_activity)? {
            MasterBoundaryCheck::Continue(guard) => guard,
            MasterBoundaryCheck::Stopped => return Ok(Tick::MasterStopped),
        };

        let (image_path, image_bytes) = match prepared_image.and_then(|prepared| {
            prepared
                .map(|image| self.commit_image(image))
                .transpose()
                .map(|stored| stored.unwrap_or((None, 0)))
        }) {
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
                if ocr_committable {
                    self.backend.discard_ocr_frame(&frame);
                }
                return Err(e);
            }
        };
        // PNG 與指向它的 DB row 已一起排在 pause writer 前面；此後只有
        // recorder 的 RAM baseline/stats，不再需要阻擋跨行程 pause。
        drop(pause_commit_guard);
        drop(master_commit_guard);

        // 到這裡才推進去重基準：畫面、文字、那一列都已經落地了。
        // 寫不進去時上面那個 `?` 會直接帶著錯誤離開，而基準原封不動——
        // 下一次同一個畫面回來仍然算新的，這正是我們要的。
        if ocr_committable {
            self.backend.commit_ocr_frame(&frame);
        }
        self.stats.ocr_blocks = self
            .stats
            .ocr_blocks
            .saturating_add(capture.ocr.len() as u64);
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
    fn prepare_image(&mut self, ts: Millis, frame: &RawFrame) -> Result<Option<PreparedImage>> {
        // 借用打架的關係先取出來：底下要改 `self.stats` 與 `last_image_ts`。
        // 一次 PathBuf clone 落在「畫面真的變了」這條路徑上，一秒鐘最多幾次。
        let Some(root) = self.image_dir.clone() else {
            return Ok(None);
        };
        let Some(rgba) = frame.rgba.as_deref() else {
            return Ok(None);
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
                return Ok(None);
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
            return Ok(None);
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
        Ok(Some(PreparedImage {
            ts,
            relative_path: rel,
            full_path: full,
            bytes,
            encode_elapsed: t.elapsed(),
        }))
    }

    /// 把已編好的 PNG 寫下來。呼叫端在進這裡前要完成最後 pause probe，並把
    /// 那次 snapshot 的 shared lock 持有到 frame DB transaction 一起完成。
    fn commit_image(&mut self, image: PreparedImage) -> Result<(Option<String>, i64)> {
        let t = Instant::now();
        if let Some(parent) = image.full_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create frame dir {}", parent.display()))?;
        }
        let len = image.bytes.len() as i64;
        std::fs::write(&image.full_path, image.bytes)
            .with_context(|| format!("write {}", image.full_path.display()))?;
        // 只有真的寫出去才計時。把跳過的次數也算進去的話，平均會被稀釋成
        // 一個看起來很便宜、但沒有對應到任何一次實際工作的數字。
        // 於是 `timings.store.calls` 恰好就是「寫出了幾張圖」。
        self.timings
            .store
            .record(image.encode_elapsed + t.elapsed());
        self.last_image_ts = Some(image.ts);
        self.image_bytes_today += len as u64;
        Ok((Some(image.relative_path), len))
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

    /// Clipboard 只讀到 RAM，這裡不持久化。Permit 與 system 的 post-check
    /// 都在呼叫端通過後，才會進 [`Self::redact_and_store_clipboard`]。
    fn stage_clipboard(&mut self, ts: Millis, permit: CapturePermit) -> Result<ClipboardStage> {
        // 每個 tick 都會問一次，而在 Windows 上這是 OpenClipboard →
        // GetClipboardData → CloseClipboard 的跨程序往返，別的程式正抓著
        // 剪貼簿時它會等。所以它必須有自己的一欄，不能混在「其他」裡面。
        let t = Instant::now();
        let polled = self.backend.poll_clipboard(ts, permit);
        self.timings.clipboard.record(t.elapsed());

        let event = match polled? {
            ClipboardCapture::ContextChanged => return Ok(ClipboardStage::ContextChanged),
            ClipboardCapture::Event(event) => event,
        };
        let Some(event) = event else {
            return Ok(ClipboardStage::Ready(None));
        };

        if let Some(reason) = self
            .config
            .privacy
            .check_clipboard_source(event.source_app.as_deref())
            .reason()
        {
            if event.source_app.is_none() {
                self.stats.clipboard_source_unknown += 1;
            } else {
                self.stats.clipboard_source_excluded += 1;
            }
            tracing::debug!(reason, "discard clipboard event at source privacy gate");
            return Ok(ClipboardStage::Ready(None));
        }
        Ok(ClipboardStage::Ready(Some(event)))
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

    /// 和 clipboard 一樣，先把 source 的累積值取到 RAM。System post-check
    /// 通過以前不能寫 DB；否則 lock 恰好落在 drain 裡時仍會留下一列 L0。
    fn stage_input(&mut self, ts: Millis) -> Result<Option<InputTick>> {
        let t = Instant::now();
        let drained = self.backend.drain_input(ts);
        self.timings.input.record(t.elapsed());
        drained
    }

    fn commit_input(&mut self, staged: Option<InputTick>) -> Result<()> {
        if let Some(tick) = staged {
            if let Some(metrics) = tick.metrics {
                self.db.insert_input(self.session_id, &metrics)?;
            } else {
                self.db.insert_input_health(
                    self.session_id,
                    tick.ts_start,
                    tick.ts_end,
                    tick.listening,
                )?;
            }
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
    use crate::replay::{ReplayPrivacyContext, ReplaySystemState, Scenario, Step};
    use crate::traits::{SystemTransition, SystemTransitionKind};
    use sister_core::config::PrivacyConfig;
    use sister_core::model::{
        BrowserUrlState, ClipboardKind, InputListening, InputMetrics, PrivacyContext,
        SensitiveFieldState,
    };

    const TEST_PERMIT: CapturePermit = CapturePermit::test(1);

    fn known_context(focus: FocusSnapshot, sensitive: SensitiveFieldState) -> PrivacyContext {
        PrivacyContext::known(focus, sensitive, BrowserUrlState::NotApplicable)
    }

    fn observed(context: PrivacyContext) -> PrivacyObservation {
        match context {
            PrivacyContext::Known { .. } => PrivacyObservation::known(context, TEST_PERMIT),
            PrivacyContext::Unknown => PrivacyObservation::Unknown,
        }
    }

    struct KnownSystem;
    impl crate::traits::SystemSource for KnownSystem {
        fn poll(&mut self, _ts: Millis) -> Result<crate::traits::SystemObservation> {
            Ok(crate::traits::SystemObservation::active())
        }
    }

    struct KnownFocus;
    impl crate::traits::FocusSource for KnownFocus {
        fn context(&mut self, _ts: Millis) -> Result<PrivacyObservation> {
            Ok(observed(known_context(
                FocusSnapshot::default(),
                SensitiveFieldState::Clear,
            )))
        }

        fn is_current(&mut self, permit: CapturePermit) -> Result<bool> {
            Ok(permit == TEST_PERMIT)
        }
    }

    enum SystemReply {
        Value(SystemObservation),
        Error(&'static str),
    }

    enum PrivacyReply {
        Value(PrivacyContext),
        Error(&'static str),
    }

    #[derive(Debug, Default)]
    struct GateCalls {
        order: Vec<&'static str>,
    }

    /// 每一個內容入口都留下順序；安全閘門測試不是只看最後 DB 恰好為空。
    struct GateBackend {
        calls: std::rc::Rc<std::cell::RefCell<GateCalls>>,
        system: std::collections::VecDeque<SystemReply>,
        privacy: std::collections::VecDeque<PrivacyReply>,
        permit_current: std::collections::VecDeque<bool>,
        clipboard_watermarks: std::collections::VecDeque<ClipboardWatermark>,
        clipboard: std::collections::VecDeque<Option<ClipboardEvent>>,
        input: std::collections::VecDeque<Option<InputTick>>,
        screen: std::collections::VecDeque<ScreenCapture>,
        pause_during_ocr: Option<std::rc::Rc<std::cell::Cell<bool>>>,
        privacy_hook: Option<Box<dyn FnOnce()>>,
    }

    impl Backend for GateBackend {
        fn name(&self) -> &str {
            "gate-probe"
        }

        fn poll_system(&mut self, _ts: Millis) -> Result<SystemObservation> {
            self.calls.borrow_mut().order.push("system");
            match self.system.pop_front().expect("scripted system answer") {
                SystemReply::Value(observation) => Ok(observation),
                SystemReply::Error(message) => anyhow::bail!(message),
            }
        }

        fn grab_screen(&mut self, _ts: Millis, _permit: CapturePermit) -> Result<ScreenCapture> {
            self.calls.borrow_mut().order.push("screen");
            Ok(self
                .screen
                .pop_front()
                .unwrap_or(ScreenCapture::Unavailable))
        }

        fn privacy_context(&mut self, _ts: Millis) -> Result<PrivacyObservation> {
            self.calls.borrow_mut().order.push("privacy");
            if let Some(hook) = self.privacy_hook.take() {
                hook();
            }
            match self.privacy.pop_front().expect("scripted privacy answer") {
                PrivacyReply::Value(context) => Ok(observed(context)),
                PrivacyReply::Error(message) => anyhow::bail!(message),
            }
        }

        fn capture_permit_is_current(&mut self, permit: CapturePermit) -> Result<bool> {
            Ok(permit == TEST_PERMIT && self.permit_current.pop_front().unwrap_or(true))
        }

        fn poll_clipboard(
            &mut self,
            _ts: Millis,
            _permit: CapturePermit,
        ) -> Result<ClipboardCapture> {
            self.calls.borrow_mut().order.push("clipboard");
            Ok(ClipboardCapture::Event(
                self.clipboard.pop_front().unwrap_or(None),
            ))
        }

        fn skip_clipboard(&mut self, _ts: Millis) -> Result<ClipboardWatermark> {
            self.calls.borrow_mut().order.push("clipboard-skip");
            Ok(self
                .clipboard_watermarks
                .pop_front()
                .unwrap_or(ClipboardWatermark::Established))
        }

        fn drain_input(&mut self, _ts: Millis) -> Result<Option<sister_core::model::InputTick>> {
            self.calls.borrow_mut().order.push("input");
            Ok(self.input.pop_front().unwrap_or(None))
        }

        fn suspend_input(&mut self, _ts: Millis) -> Result<()> {
            self.calls.borrow_mut().order.push("input-suspend");
            Ok(())
        }

        fn resume_input(&mut self, _ts: Millis) -> Result<()> {
            self.calls.borrow_mut().order.push("input-resume");
            Ok(())
        }

        fn recognize(&mut self, frame: &RawFrame) -> crate::traits::OcrAttempt {
            self.calls.borrow_mut().order.push("ocr");
            let requested = self
                .pause_during_ocr
                .as_ref()
                .expect("gate probe returned no screen, so OCR must not run");
            requested.set(true);
            crate::traits::OcrAttempt::full(frame, || Ok(Vec::new()))
        }

        fn commit_ocr_frame(&mut self, _frame: &RawFrame) {
            self.calls.borrow_mut().order.push("ocr-commit");
        }

        fn reset_ocr(&mut self) {
            self.calls.borrow_mut().order.push("ocr-reset");
        }
    }

    fn clear_privacy() -> PrivacyContext {
        known_context(FocusSnapshot::default(), SensitiveFieldState::Clear)
    }

    fn input_sentinel(ts: Millis) -> InputTick {
        InputTick {
            ts_start: ts - 10,
            ts_end: ts,
            metrics: Some(InputMetrics {
                ts_start: ts - 10,
                ts_end: ts,
                keystrokes: 7,
                ..Default::default()
            }),
            listening: InputListening::Unknown,
        }
    }

    fn clipboard_sentinel(ts: Millis) -> ClipboardEvent {
        ClipboardEvent {
            ts,
            kind: ClipboardKind::Text,
            text: Some("must stay in RAM".into()),
            byte_len: 16,
            truncated: false,
            secret_suspected: false,
            source_app: Some("notes.exe".into()),
        }
    }

    fn pause_on_probe(target: usize, observed_at: Millis) -> impl FnMut() -> PauseSignal {
        let mut calls = 0;
        move || {
            calls += 1;
            if calls == target {
                PauseSignal::Paused { observed_at }
            } else {
                PauseSignal::Recording
            }
        }
    }

    fn pause_snapshot_signal(data_dir: &std::path::Path, observed_at: Millis) -> PauseSignal {
        PauseSignal::Snapshot {
            observed_at,
            guard: sister_core::pause::snapshot_guard(data_dir),
        }
    }

    fn stored_rows<B: Backend>(recorder: &Recorder<B>, table: &str) -> i64 {
        let sql = format!("SELECT COUNT(*) FROM {table}");
        recorder
            .db()
            .conn()
            .query_row(&sql, [], |row| row.get(0))
            .expect("count stored rows")
    }

    fn stored_keystrokes<B: Backend>(recorder: &Recorder<B>) -> Option<i64> {
        recorder
            .db()
            .conn()
            .query_row("SELECT SUM(keystrokes) FROM input_metrics", [], |row| {
                row.get(0)
            })
            .expect("sum input metrics")
    }

    fn pause_audits<B: Backend>(recorder: &Recorder<B>) -> Vec<(String, Millis)> {
        let mut statement = recorder
            .db()
            .conn()
            .prepare(
                "SELECT kind, ts FROM system_events \
                 WHERE kind IN ('pause','resume') ORDER BY id",
            )
            .expect("prepare pause audit query");
        let rows = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("query pause audits");
        rows.map(|row| row.expect("pause audit row")).collect()
    }

    fn gate_recorder(
        system: Vec<SystemReply>,
        privacy: Vec<PrivacyReply>,
    ) -> (
        Recorder<GateBackend>,
        std::rc::Rc<std::cell::RefCell<GateCalls>>,
    ) {
        let calls = std::rc::Rc::new(std::cell::RefCell::new(GateCalls::default()));
        let backend = GateBackend {
            calls: calls.clone(),
            system: system.into(),
            privacy: privacy.into(),
            permit_current: std::collections::VecDeque::new(),
            clipboard_watermarks: std::collections::VecDeque::new(),
            clipboard: std::collections::VecDeque::new(),
            input: std::collections::VecDeque::new(),
            screen: std::collections::VecDeque::new(),
            pause_during_ocr: None,
            privacy_hook: None,
        };
        let recorder = Recorder::new(
            backend,
            Db::open_in_memory().expect("db"),
            Config::default(),
            None,
            MasterStopSource::NotApplicable,
        )
        .expect("recorder");
        (recorder, calls)
    }

    fn assert_no_content_after_gate(calls: &GateCalls) {
        for content in ["privacy", "clipboard", "input", "screen"] {
            assert!(
                !calls.order.contains(&content),
                "system gate 後不准走到 {content}：{:?}",
                calls.order
            );
        }
    }

    #[test]
    fn master_stop_arriving_during_slow_privacy_discards_the_tick_before_persistence() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::{Arc, Mutex};
        use std::time::Duration;

        let control = Tmp::new("master-mid-privacy");
        let images = Tmp::new("master-mid-privacy-images");
        let (mut recorder, calls) = gate_recorder(
            vec![SystemReply::Value(SystemObservation::active())],
            vec![PrivacyReply::Value(clear_privacy())],
        );
        recorder.set_master_stop_dir(control.0.clone());
        recorder.set_image_dir(Some(images.0.clone()));

        let returned = Arc::new(AtomicBool::new(false));
        let join_slot = Arc::new(Mutex::new(None));
        let stop_dir = control.0.clone();
        let returned_in_thread = Arc::clone(&returned);
        let returned_in_hook = Arc::clone(&returned);
        let join_slot_in_hook = Arc::clone(&join_slot);
        recorder.backend.privacy_hook = Some(Box::new(move || {
            let engage_dir = stop_dir.clone();
            let join = std::thread::spawn(move || {
                sister_hands::master_stop::engage(&engage_dir, 150).expect("engage");
                returned_in_thread.store(true, Ordering::SeqCst);
            });
            *join_slot_in_hook.lock().unwrap() = Some(join);
            for _ in 0..500 {
                if stop_dir.join("master.stop.pending").exists() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            assert!(
                stop_dir.join("master.stop.pending").exists(),
                "slow callback 沒等到 pending"
            );
            assert!(
                !returned_in_hook.load(Ordering::SeqCst),
                "activity guard 還活著，engage 不可先回成功"
            );
        }));

        assert_eq!(recorder.tick(200).expect("mid-stop tick"), Tick::MasterStopped);
        join_slot
            .lock()
            .unwrap()
            .take()
            .expect("engage thread")
            .join()
            .unwrap();
        assert!(returned.load(Ordering::SeqCst));
        assert_eq!(stored_rows(&recorder, "frames"), 0);
        assert_eq!(stored_rows(&recorder, "text_chunks"), 0);
        assert!(
            std::fs::read_dir(&images.0)
                .expect("image directory")
                .next()
                .is_none(),
            "stop request 後不准留下 PNG"
        );
        assert!(
            !calls.borrow().order.contains(&"screen"),
            "privacy 回來後看見 pending 就不可再 grab：{:?}",
            calls.borrow().order
        );
    }

    #[test]
    fn pause_observed_after_privacy_stops_before_every_content_source() {
        let (mut recorder, calls) = gate_recorder(
            vec![SystemReply::Value(SystemObservation::active())],
            vec![PrivacyReply::Value(clear_privacy())],
        );

        assert_eq!(
            recorder
                .tick_with_pause_probe(1_000, pause_on_probe(2, 1_001))
                .expect("pause after privacy"),
            Tick::Paused
        );
        assert!(recorder.is_paused());
        assert_eq!(
            recorder.timings().pause_probe.calls,
            2,
            "entry 與 privacy 後的 pause probe 都要記帳"
        );
        assert_eq!(
            recorder.timings().system_poll.calls,
            1,
            "走過的 system poll 要記帳"
        );
        let order = &calls.borrow().order;
        assert!(
            order.contains(&"privacy"),
            "regression must cross privacy/UIA"
        );
        for content in ["clipboard", "input", "screen"] {
            assert!(
                !order.contains(&content),
                "pause after privacy must stop before {content}: {order:?}"
            );
        }
        for table in [
            "focus_events",
            "clipboard_events",
            "input_metrics",
            "frames",
        ] {
            assert_eq!(stored_rows(&recorder, table), 0, "{table} must stay empty");
        }
        assert_eq!(
            recorder
                .db()
                .conn()
                .query_row(
                    "SELECT COUNT(*) FROM system_events WHERE kind='pause' AND ts=1001",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .expect("count pause audit"),
            1,
            "the audit must use the time the probe actually observed pause"
        );
    }

    #[test]
    fn a_complete_pause_resume_between_probes_is_not_mistaken_for_no_request() {
        let (mut recorder, calls) = gate_recorder(
            vec![SystemReply::Value(SystemObservation::active())],
            vec![PrivacyReply::Value(clear_privacy())],
        );
        let control = Tmp::new("pause-generation-aba");
        let control_dir = control.0.clone();
        let mut probes = 0;

        let tick = recorder
            .tick_with_pause_probe(1_000, move || {
                probes += 1;
                if probes == 2 {
                    sister_core::pause::set_paused(&control_dir, true, 1_001)
                        .expect("publish pause");
                    sister_core::pause::set_paused(&control_dir, false, 1_002)
                        .expect("publish resume");
                }
                pause_snapshot_signal(&control_dir, 1_003)
            })
            .expect("detect generation change");

        assert_eq!(tick, Tick::Paused);
        assert!(
            recorder.is_paused(),
            "missed generation must close sources first"
        );
        assert!(
            calls.borrow().order.contains(&"privacy"),
            "the ABA must really occur after a slow boundary"
        );
        for content in ["clipboard", "input", "screen"] {
            assert!(
                !calls.borrow().order.contains(&content),
                "the in-flight tick must stop before {content}"
            );
        }

        // 下一次看見同一代 Recording 才補 resume。兩筆都使用 control state
        // 保存的真正 request 時刻，而不是兩次較晚的 probe 時刻。
        let mut resume_probe = || pause_snapshot_signal(&control.0, 1_004);
        assert!(matches!(
            recorder
                .observe_pause_request(&mut resume_probe)
                .expect("commit matching resume"),
            PauseCheck::Resumed
        ));
        assert!(!recorder.is_paused());
        assert_eq!(
            pause_audits(&recorder),
            vec![("pause".into(), 1_001), ("resume".into(), 1_002)]
        );
    }

    #[test]
    fn an_indeterminate_pause_control_state_stops_before_the_first_content_source() {
        let (mut recorder, calls) = gate_recorder(Vec::new(), Vec::new());
        let control = Tmp::new("pause-control-indeterminate");
        std::fs::write(control.0.join("pause.state"), "{not-json").expect("corrupt control state");

        assert_eq!(
            recorder
                .tick_with_pause_probe(1_000, || pause_snapshot_signal(&control.0, 1_005))
                .expect("indeterminate is an explicit fail-closed pause"),
            Tick::Paused
        );
        assert!(recorder.is_paused());
        let observed = calls.borrow();
        assert!(observed.order.contains(&"clipboard-skip"));
        assert!(observed.order.contains(&"input-suspend"));
        assert_no_content_after_gate(&observed);
        assert_eq!(
            recorder
                .db()
                .conn()
                .query_row(
                    "SELECT COUNT(*) FROM system_events WHERE kind='pause' AND ts=1005",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .expect("count fail-closed pause audit"),
            1
        );
    }

    #[test]
    fn a_preexisting_pause_starts_this_sessions_audit_at_the_first_observation() {
        let (mut recorder, calls) = gate_recorder(Vec::new(), Vec::new());
        let control = Tmp::new("pause-preexisting-session");
        sister_core::pause::set_paused(&control.0, true, 100).expect("preexisting pause");

        assert_eq!(
            recorder
                .tick_with_pause_probe(1_000, || pause_snapshot_signal(&control.0, 1_000))
                .expect("establish paused baseline"),
            Tick::Paused
        );
        assert_no_content_after_gate(&calls.borrow());
        let pause_ts: i64 = recorder
            .db()
            .conn()
            .query_row(
                "SELECT ts FROM system_events WHERE kind='pause'",
                [],
                |row| row.get(0),
            )
            .expect("pause audit");
        assert_eq!(
            pause_ts, 1_000,
            "first snapshot establishes a session-local boundary; it cannot reuse the old request time"
        );
    }

    #[test]
    fn a_resume_newer_than_the_tick_time_consumes_an_audit_only_boundary() {
        let (mut recorder, calls) = gate_recorder(Vec::new(), Vec::new());
        let control = Tmp::new("pause-resume-after-tick-time");
        sister_core::pause::set_paused(&control.0, true, 1_000).expect("publish pause");

        assert_eq!(
            recorder
                .tick_with_pause_probe(1_050, || pause_snapshot_signal(&control.0, 1_050))
                .expect("observe pause"),
            Tick::Paused
        );
        sister_core::pause::set_paused(&control.0, false, 1_200).expect("publish resume");

        // Production 會先取得 tick timestamp，才等待 shared snapshot lock。若
        // resume writer 在兩者之間完成，audit 的真正邊界可以晚於 tick 的 ts。
        // 這一拍必須只關閉 audit，不能拿 1_100 寫任何位於 1_200 前的內容。
        assert_eq!(
            recorder
                .tick_with_pause_probe(1_100, || pause_snapshot_signal(&control.0, 1_300))
                .expect("observe newer resume"),
            Tick::Resumed
        );
        assert!(!recorder.is_paused());
        assert_no_content_after_gate(&calls.borrow());
        for table in [
            "focus_events",
            "clipboard_events",
            "input_metrics",
            "frames",
        ] {
            assert_eq!(stored_rows(&recorder, table), 0, "{table} must stay empty");
        }
        assert_eq!(
            pause_audits(&recorder),
            vec![("pause".into(), 1_050), ("resume".into(), 1_200)]
        );
    }

    #[test]
    fn recovery_from_indeterminate_does_not_reuse_an_old_resume_timestamp() {
        let (mut recorder, _calls) = gate_recorder(Vec::new(), Vec::new());
        let history = Tmp::new("pause-old-history");
        sister_core::pause::set_paused(&history.0, true, 100).expect("old pause");
        sister_core::pause::set_paused(&history.0, false, 200).expect("old resume");

        // 先建立 generation=1 的正常 baseline。
        let mut baseline = || pause_snapshot_signal(&history.0, 900);
        assert!(matches!(
            recorder
                .observe_pause_request(&mut baseline)
                .expect("old recording baseline"),
            PauseCheck::Continue(Some(_))
        ));

        // 另一份 Indeterminate snapshot 代表 control 暫時讀不到；它造出的
        // fail-closed pause 沒有 generation，不能拿 history 裡 ts=200 的舊
        // resume 關閉。
        let broken = Tmp::new("pause-transient-indeterminate");
        std::fs::write(broken.0.join("pause.state"), "{broken").expect("corrupt state");
        let mut unavailable = || pause_snapshot_signal(&broken.0, 1_000);
        assert!(matches!(
            recorder
                .observe_pause_request(&mut unavailable)
                .expect("fail closed"),
            PauseCheck::Paused
        ));

        let mut recovered = || pause_snapshot_signal(&history.0, 1_100);
        assert!(matches!(
            recorder
                .observe_pause_request(&mut recovered)
                .expect("recover control"),
            PauseCheck::Resumed
        ));
        assert_eq!(
            pause_audits(&recorder),
            vec![("pause".into(), 1_000), ("resume".into(), 1_100)]
        );
    }

    #[test]
    fn a_resume_crash_midpoint_cannot_close_this_sessions_pause_in_the_past() {
        let (mut recorder, _calls) = gate_recorder(Vec::new(), Vec::new());
        let control = Tmp::new("pause-resume-midpoint-timestamp");
        sister_core::pause::set_paused(&control.0, true, 100).expect("old pause");
        sister_core::pause::set_paused(&control.0, false, 200).expect("old resume state");
        // 模擬 resume 已寫 pause.state、但尚未刪掉同代 flag 就 crash。
        std::fs::write(control.0.join("paused.flag"), "v1 1 100")
            .expect("restore crash-midpoint flag");

        let mut paused = || pause_snapshot_signal(&control.0, 1_000);
        assert!(matches!(
            recorder
                .observe_pause_request(&mut paused)
                .expect("first paused baseline"),
            PauseCheck::Paused
        ));
        // Writer retry 刪掉 flag，但刻意保留第一次 resume request 的 ts=200。
        sister_core::pause::set_paused(&control.0, false, 1_100).expect("finish resume retry");

        let mut recording = || pause_snapshot_signal(&control.0, 1_200);
        assert!(matches!(
            recorder
                .observe_pause_request(&mut recording)
                .expect("observe completed resume"),
            PauseCheck::Resumed
        ));
        assert_eq!(
            pause_audits(&recorder),
            vec![("pause".into(), 1_000), ("resume".into(), 1_200)],
            "persisted ts=200 predates this recorder's pause audit and must be rejected"
        );
    }

    #[test]
    fn unchanged_recording_snapshots_retry_a_pause_source_tail_that_failed_to_close() {
        let (mut recorder, _calls) = gate_recorder(Vec::new(), Vec::new());
        recorder
            .backend
            .clipboard_watermarks
            .extend([ClipboardWatermark::Unknown, ClipboardWatermark::Unknown]);
        let control = Tmp::new("pause-tail-retry");
        sister_core::pause::set_paused(&control.0, true, 100).expect("pause");

        let mut paused = || pause_snapshot_signal(&control.0, 1_000);
        assert!(matches!(
            recorder
                .observe_pause_request(&mut paused)
                .expect("observe pause"),
            PauseCheck::Paused
        ));
        sister_core::pause::set_paused(&control.0, false, 1_100).expect("resume");
        let mut first_recording = || pause_snapshot_signal(&control.0, 1_100);
        assert!(matches!(
            recorder
                .observe_pause_request(&mut first_recording)
                .expect("audit resume"),
            PauseCheck::Resumed
        ));
        assert!(
            recorder.pause_clipboard_gap,
            "the second Unknown watermark must keep the source closed"
        );

        let mut retry = || pause_snapshot_signal(&control.0, 1_200);
        assert!(matches!(
            recorder
                .observe_pause_request(&mut retry)
                .expect("retry source boundary"),
            PauseCheck::Continue(Some(_))
        ));
        assert!(
            !recorder.pause_clipboard_gap,
            "same-state Recording probes must retry cleanup without another audit row"
        );
        assert_eq!(pause_audits(&recorder).len(), 2);
    }

    #[test]
    fn pause_observed_after_allowed_clipboard_and_input_drops_both_staged_values() {
        let (mut recorder, calls) = gate_recorder(
            vec![
                SystemReply::Value(SystemObservation::active()),
                SystemReply::Value(SystemObservation::active()),
            ],
            vec![PrivacyReply::Value(known_context(
                FocusSnapshot {
                    app_id: Some("notes.exe".into()),
                    window_title: Some("private draft".into()),
                    ..Default::default()
                },
                SensitiveFieldState::Clear,
            ))],
        );
        recorder
            .backend
            .clipboard
            .push_back(Some(clipboard_sentinel(1_000)));
        recorder
            .backend
            .input
            .push_back(Some(input_sentinel(1_000)));

        assert_eq!(
            recorder
                .tick_with_pause_probe(1_000, pause_on_probe(3, 1_002))
                .expect("pause after staging"),
            Tick::Paused
        );
        let order = &calls.borrow().order;
        assert!(
            order.contains(&"clipboard"),
            "regression must stage clipboard"
        );
        assert!(order.contains(&"input"), "regression must stage input");
        assert!(!order.contains(&"screen"), "pause must stop before screen");
        for table in [
            "focus_events",
            "clipboard_events",
            "input_metrics",
            "input_health",
            "frames",
        ] {
            assert_eq!(
                stored_rows(&recorder, table),
                0,
                "RAM-only {table} data must be dropped when pause wins"
            );
        }
    }

    #[test]
    fn pause_observed_after_excluded_input_drops_metrics_and_exclusion_audit() {
        let excluded = known_context(
            FocusSnapshot {
                app_id: Some("keepassxc.exe".into()),
                ..Default::default()
            },
            SensitiveFieldState::Clear,
        );
        let (mut recorder, calls) = gate_recorder(
            vec![
                SystemReply::Value(SystemObservation::active()),
                SystemReply::Value(SystemObservation::active()),
            ],
            vec![PrivacyReply::Value(excluded)],
        );
        recorder.set_privacy(PrivacyConfig {
            excluded_apps: vec!["keepassxc".into()],
            ..Default::default()
        });
        recorder
            .backend
            .input
            .push_back(Some(input_sentinel(1_000)));

        assert_eq!(
            recorder
                .tick_with_pause_probe(1_000, pause_on_probe(3, 1_003))
                .expect("pause after excluded input"),
            Tick::Paused
        );
        assert!(
            calls.borrow().order.contains(&"input"),
            "regression must stage excluded input"
        );
        assert_eq!(stored_rows(&recorder, "input_metrics"), 0);
        assert_eq!(
            recorder
                .db()
                .conn()
                .query_row(
                    "SELECT COUNT(*) FROM system_events WHERE kind='excluded'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .expect("count exclusion audit"),
            0,
            "pause wins the boundary; a stale exclusion segment must not be committed"
        );
        assert_eq!(recorder.stats().excluded, 0);
    }

    #[test]
    fn pause_observed_after_screen_grab_drops_pixels_before_dedup_ocr_db_or_png() {
        let (mut recorder, calls) = gate_recorder(
            vec![
                SystemReply::Value(SystemObservation::active()),
                SystemReply::Value(SystemObservation::active()),
                SystemReply::Value(SystemObservation::active()),
            ],
            vec![PrivacyReply::Value(clear_privacy())],
        );
        recorder
            .backend
            .screen
            .push_back(ScreenCapture::Frame(RawFrame::from_rgba(
                1_000,
                0,
                8,
                8,
                vec![9; 8 * 8 * 4],
            )));
        let images = Tmp::new("pause-after-screen");
        recorder.image_dir = Some(images.0.clone());

        assert_eq!(
            recorder
                .tick_with_pause_probe(1_000, pause_on_probe(5, 1_004))
                .expect("pause after screen"),
            Tick::Paused
        );
        assert!(
            calls.borrow().order.contains(&"screen"),
            "regression must put pixels in the frame buffer"
        );
        assert_eq!(stored_rows(&recorder, "frames"), 0);
        assert_eq!(recorder.stats().kept, 0);
        assert_eq!(recorder.stats().duplicates, 0);
        assert_eq!(recorder.stats().ocr_blocks, 0);
        assert_eq!(
            count_pngs(&images.0),
            0,
            "paused frame must never reach PNG"
        );
    }

    #[test]
    fn pause_requested_during_ocr_drops_the_result_and_frame_before_png_or_db() {
        use std::cell::Cell;
        use std::rc::Rc;

        let (mut recorder, calls) = gate_recorder(
            vec![
                SystemReply::Value(SystemObservation::active()),
                SystemReply::Value(SystemObservation::active()),
                SystemReply::Value(SystemObservation::active()),
                SystemReply::Value(SystemObservation::active()),
            ],
            vec![PrivacyReply::Value(clear_privacy())],
        );
        recorder
            .backend
            .screen
            .push_back(ScreenCapture::Frame(RawFrame::from_rgba(
                1_000,
                0,
                8,
                8,
                vec![19; 8 * 8 * 4],
            )));
        let requested = Rc::new(Cell::new(false));
        recorder.backend.pause_during_ocr = Some(requested.clone());
        let images = Tmp::new("pause-during-ocr");
        recorder.image_dir = Some(images.0.clone());

        assert_eq!(
            recorder
                .tick_with_pause_probe(1_000, move || {
                    if requested.get() {
                        PauseSignal::Paused { observed_at: 1_007 }
                    } else {
                        PauseSignal::Recording
                    }
                })
                .expect("pause during OCR"),
            Tick::Paused
        );
        let order = &calls.borrow().order;
        assert!(order.contains(&"ocr"), "regression must really enter OCR");
        assert!(
            !order.contains(&"ocr-commit"),
            "a paused OCR baseline must not become committed"
        );
        assert_eq!(stored_rows(&recorder, "frames"), 0);
        assert_eq!(recorder.stats().kept, 0);
        assert_eq!(recorder.stats().duplicates, 0);
        assert_eq!(count_pngs(&images.0), 0);
    }

    #[test]
    fn pause_observed_after_png_encode_drops_the_ram_buffer_before_file_or_db() {
        let (mut recorder, _calls) = gate_recorder(
            vec![
                SystemReply::Value(SystemObservation::active()),
                SystemReply::Value(SystemObservation::active()),
                SystemReply::Value(SystemObservation::active()),
                SystemReply::Value(SystemObservation::active()),
            ],
            vec![PrivacyReply::Value(clear_privacy())],
        );
        recorder
            .backend
            .screen
            .push_back(ScreenCapture::Frame(RawFrame::from_rgba(
                1_000,
                0,
                8,
                8,
                vec![29; 8 * 8 * 4],
            )));
        // GateBackend 的 OCR 必須有一個明確 callback；這格只用 probe 次數觸發，
        // 所以 callback 的值刻意不拿來判定 pause。
        recorder.backend.pause_during_ocr = Some(std::rc::Rc::new(std::cell::Cell::new(false)));
        let images = Tmp::new("pause-after-encode");
        recorder.image_dir = Some(images.0.clone());

        assert_eq!(
            recorder
                .tick_with_pause_probe(1_000, pause_on_probe(8, 1_008))
                .expect("pause after PNG encode"),
            Tick::Paused
        );
        assert_eq!(stored_rows(&recorder, "frames"), 0);
        assert_eq!(recorder.stats().kept, 0);
        assert_eq!(recorder.timings().store.calls, 0);
        assert_eq!(
            count_pngs(&images.0),
            0,
            "RAM buffer must never become a PNG"
        );
    }

    #[test]
    fn unknown_or_failed_system_state_stops_before_every_content_source() {
        for (answer, is_unknown) in [
            (SystemReply::Value(SystemObservation::Unknown), true),
            (SystemReply::Error("system probe failed"), false),
        ] {
            let (mut recorder, calls) = gate_recorder(vec![answer], Vec::new());
            let result = recorder.tick(1_000);
            if is_unknown {
                assert_eq!(
                    result.expect("unknown is an explicit tick"),
                    Tick::SystemUnknown
                );
            } else {
                assert!(
                    result
                        .expect_err("backend error must not become a default state")
                        .to_string()
                        .contains("read system state before content")
                );
            }
            assert_eq!(
                recorder.timings().pause_probe.calls,
                1,
                "system gate 前的 pause probe 要記帳"
            );
            assert_eq!(
                recorder.timings().system_poll.calls,
                1,
                "unknown 與 error 回答都必須記下實際 poll"
            );
            let calls = calls.borrow();
            assert_eq!(
                calls.order,
                vec!["system", "ocr-reset", "clipboard-skip", "input-suspend"],
                "fail-closed 清界線也必須在讀內容前完成"
            );
            assert_no_content_after_gate(&calls);
        }
    }

    #[test]
    fn unknown_sensitive_or_failed_privacy_stops_before_clipboard_and_screen() {
        let blocked = [
            (PrivacyContext::Unknown, "privacy context unavailable"),
            (
                known_context(FocusSnapshot::default(), SensitiveFieldState::Unknown),
                "sensitive field state unknown",
            ),
            (
                known_context(FocusSnapshot::default(), SensitiveFieldState::Focused),
                "sensitive field focused",
            ),
        ];
        for (context, reason) in blocked {
            let (mut recorder, calls) = gate_recorder(
                vec![
                    SystemReply::Value(SystemObservation::active()),
                    SystemReply::Value(SystemObservation::active()),
                    SystemReply::Value(SystemObservation::active()),
                ],
                vec![PrivacyReply::Value(context)],
            );
            assert_eq!(
                recorder.tick(1_000).expect("blocked tick"),
                Tick::Excluded {
                    reason: reason.to_string(),
                }
            );
            let order = &calls.borrow().order;
            assert!(!order.contains(&"clipboard"), "不准讀剪貼簿：{order:?}");
            assert!(!order.contains(&"screen"), "不准抓圖：{order:?}");
        }

        let (mut recorder, calls) = gate_recorder(
            vec![SystemReply::Value(SystemObservation::active())],
            vec![PrivacyReply::Error("focus backend failed")],
        );
        assert!(
            recorder
                .tick(1_000)
                .expect_err("focus error must not become a default context")
                .to_string()
                .contains("read privacy context before content")
        );
        assert_eq!(
            calls.borrow().order,
            vec!["system", "privacy", "ocr-reset", "clipboard-skip"]
        );

        // 正向 control：同一個 spy 在安全前提明確時確實看得到內容入口。
        let (mut recorder, calls) = gate_recorder(
            vec![
                SystemReply::Value(SystemObservation::active()),
                SystemReply::Value(SystemObservation::active()),
                SystemReply::Value(SystemObservation::active()),
            ],
            vec![PrivacyReply::Value(clear_privacy())],
        );
        assert_eq!(recorder.tick(1_000).expect("clear tick"), Tick::NoScreen);
        assert_eq!(
            calls.borrow().order,
            vec![
                "system",
                "privacy",
                "system",
                "clipboard",
                "input",
                "system",
                "screen",
            ]
        );
    }

    #[test]
    fn lock_after_excluded_privacy_stops_before_exclusion_or_input() {
        let locked = SystemObservation::Known {
            state: SystemContentState::locked_awake(),
            transitions: vec![SystemTransition {
                sequence: 1,
                ts: 1_000,
                kind: SystemTransitionKind::Lock,
            }],
        };
        let excluded = known_context(
            FocusSnapshot {
                app_id: Some("keepassxc.exe".into()),
                ..Default::default()
            },
            SensitiveFieldState::Clear,
        );
        let (mut recorder, calls) = gate_recorder(
            vec![
                SystemReply::Value(SystemObservation::active()),
                SystemReply::Value(locked),
                SystemReply::Value(SystemObservation::Known {
                    state: SystemContentState::locked_awake(),
                    transitions: Vec::new(),
                }),
            ],
            vec![PrivacyReply::Value(excluded)],
        );
        let privacy = PrivacyConfig {
            excluded_apps: vec!["keepassxc".into()],
            ..Default::default()
        };
        recorder.set_privacy(privacy);
        recorder
            .backend
            .input
            .push_back(Some(input_sentinel(1_000)));

        assert_eq!(
            recorder.tick(1_000).expect("lock boundary"),
            Tick::SystemChanged
        );
        assert_eq!(
            recorder.backend.input.len(),
            1,
            "input source must not be drained"
        );
        assert!(!calls.borrow().order.contains(&"input"));
        assert_eq!(
            recorder
                .db()
                .conn()
                .query_row("SELECT COUNT(*) FROM input_metrics", [], |row| row
                    .get::<_, i64>(0))
                .expect("count input"),
            0
        );
        assert_eq!(
            recorder
                .db()
                .conn()
                .query_row(
                    "SELECT COUNT(*) FROM system_events WHERE kind='excluded'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .expect("count exclusion audit"),
            0,
            "lock audit must not be preceded by a stale exclusion segment"
        );

        calls.borrow_mut().order.clear();
        assert_eq!(recorder.tick(1_100).expect("stable locked"), Tick::NoScreen);
        assert_no_content_after_gate(&calls.borrow());
    }

    #[test]
    fn lock_during_excluded_input_drops_the_staged_metrics() {
        let excluded = known_context(
            FocusSnapshot {
                app_id: Some("keepassxc.exe".into()),
                ..Default::default()
            },
            SensitiveFieldState::Clear,
        );
        let (mut recorder, calls) = gate_recorder(
            vec![
                SystemReply::Value(SystemObservation::active()),
                SystemReply::Value(SystemObservation::active()),
                SystemReply::Value(SystemObservation::Known {
                    state: SystemContentState::locked_awake(),
                    transitions: vec![SystemTransition {
                        sequence: 1,
                        ts: 1_000,
                        kind: SystemTransitionKind::Lock,
                    }],
                }),
            ],
            vec![PrivacyReply::Value(excluded)],
        );
        let privacy = PrivacyConfig {
            excluded_apps: vec!["keepassxc".into()],
            ..Default::default()
        };
        recorder.set_privacy(privacy);
        recorder
            .backend
            .input
            .push_back(Some(input_sentinel(1_000)));

        assert_eq!(
            recorder.tick(1_000).expect("lock boundary"),
            Tick::SystemChanged
        );
        assert!(
            calls.borrow().order.contains(&"input"),
            "regression must stage input"
        );
        assert_eq!(
            recorder
                .db()
                .conn()
                .query_row("SELECT COUNT(*) FROM input_metrics", [], |row| row
                    .get::<_, i64>(0))
                .expect("count input"),
            0,
            "post-drain lock must discard the RAM-only metrics"
        );
        assert_eq!(recorder.stats().excluded, 0);
    }

    #[test]
    fn lock_during_allowed_input_drops_focus_and_staged_metrics_together() {
        let (mut recorder, calls) = gate_recorder(
            vec![
                SystemReply::Value(SystemObservation::active()),
                SystemReply::Value(SystemObservation::active()),
                SystemReply::Value(SystemObservation::Known {
                    state: SystemContentState::locked_awake(),
                    transitions: vec![SystemTransition {
                        sequence: 1,
                        ts: 1_000,
                        kind: SystemTransitionKind::Lock,
                    }],
                }),
            ],
            vec![PrivacyReply::Value(known_context(
                FocusSnapshot {
                    app_id: Some("notes.exe".into()),
                    window_title: Some("private draft".into()),
                    ..Default::default()
                },
                SensitiveFieldState::Clear,
            ))],
        );
        recorder
            .backend
            .input
            .push_back(Some(input_sentinel(1_000)));

        assert_eq!(
            recorder.tick(1_000).expect("lock boundary"),
            Tick::SystemChanged
        );
        assert!(
            calls.borrow().order.contains(&"input"),
            "regression must stage input"
        );
        for table in ["input_metrics", "input_health", "focus_events"] {
            let sql = format!("SELECT COUNT(*) FROM {table}");
            let rows = recorder
                .db()
                .conn()
                .query_row(&sql, [], |row| row.get::<_, i64>(0))
                .expect("count protected rows");
            assert_eq!(rows, 0, "{table} must remain empty across the lock");
        }
    }

    #[test]
    fn watermark_gap_never_skips_the_final_system_and_permit_gate() {
        let (mut recorder, calls) = gate_recorder(
            vec![
                SystemReply::Value(SystemObservation::active()),
                SystemReply::Value(SystemObservation::active()),
                SystemReply::Value(SystemObservation::active()),
            ],
            vec![PrivacyReply::Value(clear_privacy())],
        );
        // 模擬剛離開 lock/privacy gap：skip 仍然無法建立可信
        // clipboard sequence，而前景在第二次 system poll 後已改變。
        recorder.system_clipboard_gap = true;
        recorder
            .backend
            .clipboard_watermarks
            .push_back(ClipboardWatermark::Unknown);
        recorder.backend.permit_current.push_back(false);

        assert_eq!(
            recorder.tick(1_000).expect("fail closed tick"),
            Tick::ContextChanged
        );
        assert_eq!(
            calls.borrow().order,
            vec![
                "system",
                "privacy",
                "system",
                "clipboard-skip",
                "input",
                "system",
                "ocr-reset",
                "clipboard-skip",
            ],
            "watermark 未建立不能跳過最後的 system/permit gate"
        );
        let focus_rows: i64 = recorder
            .db()
            .conn()
            .query_row("SELECT COUNT(*) FROM focus_events", [], |row| row.get(0))
            .expect("count focus rows");
        assert_eq!(focus_rows, 0, "stale focus title must never persist");
    }

    #[test]
    fn real_system_transitions_are_audited_and_seal_both_sides_of_the_gap() {
        let transition = |sequence, ts, kind| SystemTransition { sequence, ts, kind };
        let (mut recorder, calls) = gate_recorder(
            vec![
                SystemReply::Value(SystemObservation::active()),
                SystemReply::Value(SystemObservation::active()),
                SystemReply::Value(SystemObservation::active()),
                SystemReply::Value(SystemObservation::Known {
                    state: SystemContentState::locked_awake(),
                    transitions: vec![transition(1, 1_000, SystemTransitionKind::Lock)],
                }),
                SystemReply::Value(SystemObservation::Known {
                    state: SystemContentState::active(),
                    transitions: vec![transition(2, 2_000, SystemTransitionKind::Unlock)],
                }),
            ],
            vec![PrivacyReply::Value(clear_privacy())],
        );

        assert_eq!(recorder.tick(500).expect("active"), Tick::NoScreen);
        assert_eq!(recorder.tick(1_000).expect("lock"), Tick::SystemChanged);
        assert_eq!(recorder.tick(2_000).expect("unlock"), Tick::SystemChanged);

        assert_eq!(
            calls.borrow().order,
            vec![
                "system",
                "privacy",
                "system",
                "clipboard",
                "input",
                "system",
                "screen",
                "system",
                "ocr-reset",
                "clipboard-skip",
                "input-suspend",
                "system",
                "ocr-reset",
                "clipboard-skip",
                "input-suspend",
            ],
            "lock/unlock 那拍都只寫 audit 並封邊界，不讀內容"
        );

        let events: Vec<(String, Millis)> = {
            let mut statement = recorder
                .db()
                .conn()
                .prepare(
                    "SELECT kind, ts FROM system_events
                     WHERE kind IN ('lock','unlock','sleep','wake') ORDER BY ts",
                )
                .expect("query events");
            let rows = statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .expect("map events");
            rows.map(|row| row.expect("event row")).collect()
        };
        assert_eq!(
            events,
            vec![("lock".into(), 1_000), ("unlock".into(), 2_000)]
        );
    }

    #[test]
    fn replay_event_at_zero_establishes_baseline_then_stops_before_persistence() {
        let mut recorder = recorder(
            vec![Step {
                at_ms: 0,
                app: Some("notes.exe".into()),
                title: Some("private draft".into()),
                text: vec!["must not cross the lock boundary".into()],
                system_event: Some(SystemTransitionKind::Lock),
                ..Default::default()
            }],
            Config::default(),
        );

        assert_eq!(
            recorder.tick(0).expect("event-at-zero tick"),
            Tick::SystemChanged
        );
        assert_eq!(
            recorder
                .db()
                .conn()
                .query_row("SELECT COUNT(*) FROM frames", [], |row| row
                    .get::<_, i64>(0))
                .expect("count frames"),
            0,
            "content staged before the revalidation must not persist"
        );
        assert_eq!(
            recorder
                .db()
                .conn()
                .query_row(
                    "SELECT COUNT(*) FROM system_events WHERE kind='lock'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .expect("count lock audit"),
            1
        );
    }

    #[test]
    fn finish_retries_the_whole_pending_system_batch_without_repolling() {
        let transition = |sequence, kind| SystemTransition {
            sequence,
            ts: 1_000,
            kind,
        };
        let (mut recorder, calls) = gate_recorder(
            vec![
                // 第一拍建立 recorder baseline；screen unavailable 前會問三次。
                SystemReply::Value(SystemObservation::active()),
                SystemReply::Value(SystemObservation::active()),
                SystemReply::Value(SystemObservation::active()),
                // 同一次 source observation 的第二筆故意讓 DB trigger 炸掉。
                SystemReply::Value(SystemObservation::Known {
                    state: SystemContentState::active(),
                    transitions: vec![
                        transition(1, SystemTransitionKind::Lock),
                        transition(2, SystemTransitionKind::Unlock),
                    ],
                }),
                // 若 finish 錯誤地重新 poll，這一筆會被取走。
                SystemReply::Value(SystemObservation::Unknown),
            ],
            vec![PrivacyReply::Value(clear_privacy())],
        );
        assert_eq!(recorder.tick(500).expect("baseline"), Tick::NoScreen);

        recorder
            .db()
            .conn()
            .execute_batch(
                "CREATE TRIGGER fail_second_native_event
                 BEFORE INSERT ON system_events
                 WHEN NEW.kind = 'unlock'
                 BEGIN SELECT RAISE(ABORT, 'simulated native audit failure'); END;",
            )
            .expect("install failure trigger");

        calls.borrow_mut().order.clear();
        let error = recorder.tick(1_000).expect_err("batch must fail");
        assert!(
            format!("{error:#}").contains("simulated native audit failure"),
            "{error:#}"
        );
        assert_eq!(
            recorder
                .db()
                .conn()
                .query_row(
                    "SELECT COUNT(*) FROM system_events WHERE kind IN ('lock','unlock')",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .expect("count rolled-back rows"),
            0,
            "第一筆也必須跟第二筆一起 rollback"
        );
        assert!(
            recorder.pending_system.is_some(),
            "完整 observation 要留在 RAM"
        );
        assert_eq!(
            recorder.last_system_state, None,
            "失敗不能推進 state watermark"
        );
        assert_eq!(
            recorder.last_system_transition_sequence, None,
            "失敗不能推進 sequence watermark"
        );
        assert_eq!(
            calls
                .borrow()
                .order
                .iter()
                .filter(|call| **call == "system")
                .count(),
            1
        );
        assert_eq!(recorder.backend.system.len(), 1, "sentinel 尚未被 poll");

        recorder
            .db()
            .conn()
            .execute_batch("DROP TRIGGER fail_second_native_event;")
            .expect("remove failure trigger");
        recorder
            .finish(EndReason::Requested)
            .expect("finish retries pending audit");

        assert!(recorder.pending_system.is_none(), "成功後才清 pending");
        assert_eq!(
            recorder.last_system_state,
            Some(SystemContentState::active())
        );
        assert_eq!(recorder.last_system_transition_sequence, Some(2));
        assert_eq!(recorder.last_system_transition_ts, Some(1_000));
        assert_eq!(
            calls
                .borrow()
                .order
                .iter()
                .filter(|call| **call == "system")
                .count(),
            1,
            "finish 只能重送 recorder-owned pending，不可向 source 重新 poll"
        );
        assert_eq!(recorder.backend.system.len(), 1, "sentinel 必須留著");

        let events: Vec<String> = {
            let mut statement = recorder
                .db()
                .conn()
                .prepare(
                    "SELECT kind FROM system_events
                     WHERE kind IN ('lock','unlock','session_end') ORDER BY id",
                )
                .expect("query committed audit order");
            statement
                .query_map([], |row| row.get(0))
                .expect("map committed audit order")
                .map(|row| row.expect("event row"))
                .collect()
        };
        assert_eq!(
            events,
            vec!["lock", "unlock", "session_end"],
            "pending batch 必須 exactly once，且完整排在 session_end 前"
        );
    }

    #[test]
    fn pause_change_cannot_overtake_a_pending_system_batch() {
        let transition = |sequence, kind| SystemTransition {
            sequence,
            ts: 1_000,
            kind,
        };
        let (mut recorder, calls) = gate_recorder(
            vec![
                SystemReply::Value(SystemObservation::active()),
                SystemReply::Value(SystemObservation::active()),
                SystemReply::Value(SystemObservation::active()),
                SystemReply::Value(SystemObservation::Known {
                    state: SystemContentState::active(),
                    transitions: vec![
                        transition(1, SystemTransitionKind::Lock),
                        transition(2, SystemTransitionKind::Unlock),
                    ],
                }),
                // set_paused 只可重送 pending，這個 sentinel 不能被 poll。
                SystemReply::Value(SystemObservation::Unknown),
            ],
            vec![PrivacyReply::Value(clear_privacy())],
        );
        assert_eq!(recorder.tick(500).expect("baseline"), Tick::NoScreen);
        recorder
            .db()
            .conn()
            .execute_batch(
                "CREATE TRIGGER fail_pause_order_batch
                 BEFORE INSERT ON system_events
                 WHEN NEW.kind = 'unlock'
                 BEGIN SELECT RAISE(ABORT, 'simulated pause-order failure'); END;",
            )
            .expect("install failure trigger");
        assert!(
            recorder.tick(1_000).is_err(),
            "native batch must be pending"
        );
        calls.borrow_mut().order.clear();

        let error = recorder
            .set_paused(true, 1_100)
            .expect_err("pause audit must wait behind pending native audit");
        assert!(
            format!("{error:#}").contains("simulated pause-order failure"),
            "{error:#}"
        );
        assert!(!recorder.paused, "failed pause audit cannot change state");
        assert_eq!(
            recorder
                .db()
                .conn()
                .query_row(
                    "SELECT COUNT(*) FROM system_events
                     WHERE kind IN ('lock','unlock','pause','resume')",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .expect("count blocked audit"),
            0,
            "neither half of either audit may overtake the failed batch"
        );
        assert_eq!(recorder.backend.system.len(), 1, "sentinel not polled");

        recorder
            .db()
            .conn()
            .execute_batch("DROP TRIGGER fail_pause_order_batch;")
            .expect("remove failure trigger");
        assert!(recorder.set_paused(true, 1_200).expect("pause"));
        assert!(recorder.set_paused(false, 1_300).expect("resume"));
        assert_eq!(
            recorder.backend.system.len(),
            1,
            "sentinel still not polled"
        );

        let events: Vec<String> = {
            let mut statement = recorder
                .db()
                .conn()
                .prepare(
                    "SELECT kind FROM system_events
                     WHERE kind IN ('lock','unlock','pause','resume')
                     ORDER BY id",
                )
                .expect("query audit order");
            statement
                .query_map([], |row| row.get(0))
                .expect("map audit order")
                .map(|row| row.expect("event row"))
                .collect()
        };
        assert_eq!(
            events,
            vec!["lock", "unlock", "pause", "resume"],
            "native audit must remain ahead of the later human pause/resume"
        );
    }

    #[test]
    fn unchanged_pause_flag_leaves_pending_system_retry_to_the_tick_gate() {
        let transition = |sequence, kind| SystemTransition {
            sequence,
            ts: 1_000,
            kind,
        };
        let (mut recorder, calls) = gate_recorder(
            vec![
                SystemReply::Value(SystemObservation::active()),
                SystemReply::Value(SystemObservation::active()),
                SystemReply::Value(SystemObservation::active()),
                SystemReply::Value(SystemObservation::Known {
                    state: SystemContentState::active(),
                    transitions: vec![
                        transition(1, SystemTransitionKind::Lock),
                        transition(2, SystemTransitionKind::Unlock),
                    ],
                }),
                // A retry must not repoll this sentinel or reach any content source.
                SystemReply::Value(SystemObservation::Unknown),
            ],
            vec![PrivacyReply::Value(clear_privacy())],
        );
        assert_eq!(recorder.tick(500).expect("baseline"), Tick::NoScreen);
        recorder
            .db()
            .conn()
            .execute_batch(
                "CREATE TRIGGER fail_unchanged_pause_batch
                 BEFORE INSERT ON system_events
                 WHEN NEW.kind = 'unlock'
                 BEGIN SELECT RAISE(ABORT, 'simulated unchanged-pause failure'); END;",
            )
            .expect("install failure trigger");
        assert!(recorder.tick(1_000).is_err(), "batch must remain pending");
        recorder
            .db()
            .conn()
            .execute_batch("DROP TRIGGER fail_unchanged_pause_batch;")
            .expect("remove failure trigger");
        calls.borrow_mut().order.clear();

        assert!(
            !recorder.set_paused(false, 1_100).expect("unchanged flag"),
            "live loop's false-to-false update is not a pause transition"
        );
        assert!(recorder.pending_system.is_some());
        assert_eq!(
            recorder.tick(1_100).expect("retry tick"),
            Tick::SystemChanged,
            "a successful audit retry still consumes the whole tick"
        );
        assert_eq!(
            calls.borrow().order,
            Vec::<&'static str>::new(),
            "retry uses recorder-owned pending and reaches no backend/content source"
        );
        assert_eq!(recorder.backend.system.len(), 1, "sentinel not polled");
    }

    #[test]
    fn inconsistent_or_future_system_observations_fail_closed() {
        let cases = [
            (
                SystemObservation::Known {
                    state: SystemContentState::active(),
                    transitions: vec![SystemTransition {
                        sequence: 1,
                        ts: 1_000,
                        kind: SystemTransitionKind::Lock,
                    }],
                },
                "transitions imply",
            ),
            (
                SystemObservation::Known {
                    state: SystemContentState::active(),
                    transitions: vec![SystemTransition {
                        sequence: 1,
                        ts: 1_001,
                        kind: SystemTransitionKind::Wake,
                    }],
                },
                "from the future",
            ),
        ];
        for (observation, message) in cases {
            let (mut recorder, calls) = gate_recorder(
                vec![
                    SystemReply::Value(SystemObservation::active()),
                    SystemReply::Value(SystemObservation::active()),
                    SystemReply::Value(SystemObservation::active()),
                    SystemReply::Value(observation),
                ],
                vec![PrivacyReply::Value(clear_privacy())],
            );
            assert_eq!(recorder.tick(500).expect("baseline"), Tick::NoScreen);
            calls.borrow_mut().order.clear();
            let error = recorder.tick(1_000).expect_err("invalid observation");
            assert!(format!("{error:#}").contains(message), "{error:#}");
            let calls = calls.borrow();
            assert_eq!(
                calls.order,
                vec!["system", "ocr-reset", "clipboard-skip", "input-suspend"]
            );
            assert_no_content_after_gate(&calls);
        }
    }

    #[test]
    fn state_changes_without_events_and_out_of_order_events_fail_closed() {
        let (mut recorder, calls) = gate_recorder(
            vec![
                SystemReply::Value(SystemObservation::active()),
                SystemReply::Value(SystemObservation::active()),
                SystemReply::Value(SystemObservation::active()),
                SystemReply::Value(SystemObservation::Known {
                    state: SystemContentState::locked_awake(),
                    transitions: Vec::new(),
                }),
            ],
            vec![PrivacyReply::Value(clear_privacy())],
        );
        assert_eq!(recorder.tick(500).expect("initial state"), Tick::NoScreen);
        calls.borrow_mut().order.clear();
        let error = recorder.tick(1_000).expect_err("missing transition");
        assert!(format!("{error:#}").contains("without a transition"));
        assert_no_content_after_gate(&calls.borrow());

        let (mut recorder, calls) = gate_recorder(
            vec![
                SystemReply::Value(SystemObservation::active()),
                SystemReply::Value(SystemObservation::active()),
                SystemReply::Value(SystemObservation::active()),
                SystemReply::Value(SystemObservation::Known {
                    state: SystemContentState::locked_awake(),
                    transitions: vec![SystemTransition {
                        sequence: 1,
                        ts: 1_000,
                        kind: SystemTransitionKind::Lock,
                    }],
                }),
                SystemReply::Value(SystemObservation::Known {
                    state: SystemContentState::active(),
                    transitions: vec![SystemTransition {
                        sequence: 2,
                        ts: 999,
                        kind: SystemTransitionKind::Unlock,
                    }],
                }),
            ],
            vec![PrivacyReply::Value(clear_privacy())],
        );
        assert_eq!(recorder.tick(500).expect("baseline"), Tick::NoScreen);
        assert_eq!(recorder.tick(1_000).expect("lock"), Tick::SystemChanged);
        calls.borrow_mut().order.clear();
        let error = recorder.tick(2_000).expect_err("out-of-order transition");
        assert!(format!("{error:#}").contains("out of order"));
        assert_no_content_after_gate(&calls.borrow());
    }

    #[test]
    fn recovery_after_unknown_establishes_a_new_baseline_without_inventing_an_event() {
        let (mut recorder, _calls) = gate_recorder(
            vec![
                SystemReply::Value(SystemObservation::active()),
                SystemReply::Value(SystemObservation::active()),
                SystemReply::Value(SystemObservation::active()),
                SystemReply::Value(SystemObservation::Unknown),
                SystemReply::Value(SystemObservation::Known {
                    state: SystemContentState::locked_awake(),
                    transitions: Vec::new(),
                }),
            ],
            vec![PrivacyReply::Value(clear_privacy())],
        );
        assert_eq!(recorder.tick(500).expect("initial"), Tick::NoScreen);
        assert_eq!(recorder.tick(1_000).expect("unknown"), Tick::SystemUnknown);
        assert_eq!(
            recorder.tick(2_000).expect("new locked baseline"),
            Tick::NoScreen
        );

        let native_events: i64 = recorder
            .db()
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM system_events
                 WHERE kind IN ('lock','unlock','sleep','wake')",
                [],
                |row| row.get(0),
            )
            .expect("count events");
        assert_eq!(
            native_events, 0,
            "unknown 空洞後只知道目前鎖著，不能捏造一個精確 lock 時刻"
        );
    }

    #[test]
    fn input_from_a_lock_gap_never_reappears_after_unlock() {
        let scenario = Scenario {
            name: "lock-input-boundary".into(),
            privacy_context: ReplayPrivacyContext::Clear,
            system_state: ReplaySystemState::Active,
            steps: vec![
                Step {
                    at_ms: 100,
                    keystrokes: 7,
                    system_event: Some(SystemTransitionKind::Lock),
                    ..Default::default()
                },
                Step {
                    at_ms: 200,
                    keystrokes: 5,
                    ..Default::default()
                },
                Step {
                    at_ms: 300,
                    keystrokes: 11,
                    system_event: Some(SystemTransitionKind::Unlock),
                    ..Default::default()
                },
                Step {
                    at_ms: 400,
                    keystrokes: 3,
                    ..Default::default()
                },
            ],
        };
        let mut recorder = Recorder::new(
            crate::replay::ReplayBackend::new(scenario),
            Db::open_in_memory().expect("db"),
            Config::default(),
            None,
            MasterStopSource::NotApplicable,
        )
        .expect("recorder");

        assert_eq!(recorder.tick(100).expect("lock"), Tick::SystemChanged);
        assert_eq!(recorder.tick(200).expect("locked"), Tick::NoScreen);
        assert_eq!(recorder.tick(300).expect("unlock"), Tick::SystemChanged);
        recorder.tick(350).expect("stable recovery boundary");
        recorder.tick(400).expect("post-unlock input");

        assert_eq!(
            stored_keystrokes(&recorder),
            Some(3),
            "pre-lock, locked, and unlock-boundary input must all be discarded"
        );
    }

    #[test]
    fn input_from_an_unknown_system_gap_never_reappears_after_recovery() {
        let scenario = Scenario {
            name: "unknown-input-boundary".into(),
            privacy_context: ReplayPrivacyContext::Clear,
            system_state: ReplaySystemState::Active,
            steps: vec![
                Step {
                    at_ms: 100,
                    keystrokes: 7,
                    system_state: Some(ReplaySystemState::Unknown),
                    ..Default::default()
                },
                Step {
                    at_ms: 200,
                    keystrokes: 5,
                    ..Default::default()
                },
                Step {
                    at_ms: 300,
                    keystrokes: 11,
                    system_state: Some(ReplaySystemState::Active),
                    ..Default::default()
                },
                Step {
                    at_ms: 400,
                    keystrokes: 3,
                    ..Default::default()
                },
            ],
        };
        let mut recorder = Recorder::new(
            crate::replay::ReplayBackend::new(scenario),
            Db::open_in_memory().expect("db"),
            Config::default(),
            None,
            MasterStopSource::NotApplicable,
        )
        .expect("recorder");

        assert_eq!(recorder.tick(100).expect("unknown"), Tick::SystemUnknown);
        assert_eq!(
            recorder.tick(200).expect("still unknown"),
            Tick::SystemUnknown
        );
        recorder.tick(300).expect("known recovery baseline");
        recorder.tick(400).expect("post-recovery input");

        assert_eq!(
            stored_keystrokes(&recorder),
            Some(3),
            "input from before/during the unknown interval must remain discarded"
        );
    }

    #[test]
    fn overlapping_pause_and_system_gaps_resume_input_only_after_both_close() {
        let (mut recorder, calls) = gate_recorder(Vec::new(), Vec::new());
        recorder.pause_input_gap = true;
        recorder.system_input_gap = true;

        recorder.close_pause_input_gap(1_000);
        assert!(!recorder.pause_input_gap);
        assert!(recorder.system_input_gap);
        assert_eq!(
            calls.borrow().order,
            vec!["input-suspend"],
            "closing pause alone must leave the shared source suspended"
        );

        recorder.close_system_input_gap(1_100);
        assert!(!recorder.pause_input_gap);
        assert!(!recorder.system_input_gap);
        assert_eq!(
            calls.borrow().order,
            vec!["input-suspend", "input-suspend", "input-resume"],
            "only the final overlapping reason may reopen input"
        );
    }

    #[test]
    fn unknown_does_not_erase_the_monotonic_system_event_watermark() {
        let (mut recorder, calls) = gate_recorder(
            vec![
                SystemReply::Value(SystemObservation::active()),
                SystemReply::Value(SystemObservation::active()),
                SystemReply::Value(SystemObservation::active()),
                SystemReply::Value(SystemObservation::Known {
                    state: SystemContentState::locked_awake(),
                    transitions: vec![SystemTransition {
                        sequence: 1,
                        ts: 1_000,
                        kind: SystemTransitionKind::Lock,
                    }],
                }),
                SystemReply::Value(SystemObservation::Unknown),
                SystemReply::Value(SystemObservation::Known {
                    state: SystemContentState::active(),
                    transitions: vec![SystemTransition {
                        sequence: 2,
                        ts: 500,
                        kind: SystemTransitionKind::Wake,
                    }],
                }),
            ],
            vec![PrivacyReply::Value(clear_privacy())],
        );
        assert_eq!(recorder.tick(500).expect("baseline"), Tick::NoScreen);
        assert_eq!(recorder.tick(1_000).expect("lock"), Tick::SystemChanged);
        assert_eq!(recorder.tick(2_000).expect("unknown"), Tick::SystemUnknown);
        calls.borrow_mut().order.clear();
        let error = recorder.tick(3_000).expect_err("stale event after unknown");
        assert!(format!("{error:#}").contains("out of order"), "{error:#}");
        assert_no_content_after_gate(&calls.borrow());
    }

    #[test]
    fn recorder_writes_quiet_input_health_from_the_input_source() {
        use crate::traits::{CompositeBackend, InputSource, NullClipboard, NullOcr, NullScreen};
        use sister_core::model::{HookHealth, InputListening, InputTick, classify_quiet_window};

        struct Quiet {
            hook: HookHealth,
            os_idle_ms: Option<u64>,
        }
        impl InputSource for Quiet {
            fn drain(&mut self, ts: Millis) -> Result<Option<InputTick>> {
                Ok(Some(InputTick {
                    ts_start: ts - 10_000,
                    ts_end: ts,
                    metrics: None,
                    listening: classify_quiet_window(self.hook, self.os_idle_ms, 10_000),
                }))
            }
            fn suspend(&mut self, _ts: Millis) -> Result<()> {
                Ok(())
            }
            fn resume(&mut self, _ts: Millis) -> Result<()> {
                Ok(())
            }
        }

        for (os_idle_ms, state) in [
            (Some(10_000), InputListening::IdleConfirmed),
            (Some(9_999), InputListening::NotListening),
        ] {
            let backend = CompositeBackend {
                name: "quiet-input".into(),
                system: KnownSystem,
                screen: NullScreen,
                focus: KnownFocus,
                clipboard: NullClipboard,
                input: Quiet {
                    hook: HookHealth::Active,
                    os_idle_ms,
                },
                ocr: NullOcr,
            };
            let mut recorder = Recorder::new(
                backend,
                Db::open_in_memory().expect("db"),
                Config::default(),
                None,
                MasterStopSource::NotApplicable,
            )
            .expect("recorder");
            let staged = recorder.stage_input(20_000).expect("stage input");
            recorder.commit_input(staged).expect("commit input");
            let db = recorder.into_db();
            assert_eq!(db.input_window_covering(15_000).unwrap(), None);
            assert_eq!(db.input_health_covering(15_000).unwrap(), Some(state));
        }
    }

    #[derive(Debug, Default, PartialEq, Eq)]
    struct OcrLifecycle {
        recognized: Vec<Millis>,
        committed: Vec<Millis>,
        discarded: Vec<Millis>,
        resets: u64,
    }

    /// 把 recorder 和 stateful OCR gate 之間的 transaction 接線攤成可斷言的紀錄。
    struct LifecycleOcr(std::rc::Rc<std::cell::RefCell<OcrLifecycle>>);

    impl crate::traits::RecordingOcr for LifecycleOcr {
        fn recognize_frame(&mut self, frame: &RawFrame) -> crate::traits::OcrAttempt {
            self.0.borrow_mut().recognized.push(frame.ts);
            crate::traits::OcrAttempt::full(frame, || {
                Ok(vec![sister_core::model::OcrBlock {
                    text: "可搜尋".into(),
                    x: 1,
                    y: 1,
                    w: 20,
                    h: 10,
                    confidence: 1.0,
                }])
            })
        }

        fn commit_frame(&mut self, frame: &RawFrame) {
            self.0.borrow_mut().committed.push(frame.ts);
        }

        fn discard_frame(&mut self, frame: &RawFrame) {
            self.0.borrow_mut().discarded.push(frame.ts);
        }

        fn reset(&mut self) {
            self.0.borrow_mut().resets += 1;
        }
    }

    /// 只記錄「有沒有被叫到」的假剪貼簿，用來驗證排除期間的行為。
    #[derive(Default)]
    struct SpyClipboard {
        polled: Vec<Millis>,
        skipped: Vec<Millis>,
        trace: Vec<(&'static str, Millis)>,
    }

    impl crate::traits::ClipboardSource for std::rc::Rc<std::cell::RefCell<SpyClipboard>> {
        fn poll(&mut self, ts: Millis) -> Result<Option<ClipboardEvent>> {
            let mut spy = self.borrow_mut();
            spy.polled.push(ts);
            spy.trace.push(("poll", ts));
            Ok(None)
        }
        fn skip(&mut self, ts: Millis) -> Result<ClipboardWatermark> {
            let mut spy = self.borrow_mut();
            spy.skipped.push(ts);
            spy.trace.push(("skip", ts));
            Ok(ClipboardWatermark::Established)
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
        use crate::traits::{CompositeBackend, NullInput, NullScreen};

        let spy = std::rc::Rc::new(std::cell::RefCell::new(SpyClipboard::default()));
        let ocr = std::rc::Rc::new(std::cell::RefCell::new(OcrLifecycle::default()));

        struct Focus(String);
        impl crate::traits::FocusSource for Focus {
            fn context(&mut self, _ts: Millis) -> Result<PrivacyObservation> {
                Ok(observed(known_context(
                    FocusSnapshot {
                        app_id: Some(self.0.clone()),
                        ..Default::default()
                    },
                    SensitiveFieldState::Clear,
                )))
            }
            fn is_current(&mut self, permit: CapturePermit) -> Result<bool> {
                Ok(permit == TEST_PERMIT)
            }
        }

        let mut config = Config::default();
        config.privacy.excluded_apps = vec!["keepassxc".into()];

        let backend = CompositeBackend {
            name: "spy".into(),
            system: KnownSystem,
            screen: NullScreen,
            focus: Focus("keepassxc.exe".into()),
            clipboard: spy.clone(),
            input: NullInput,
            ocr: LifecycleOcr(ocr.clone()),
        };
        let mut rec = Recorder::new(
            backend,
            Db::open_in_memory().unwrap(),
            config,
            None,
            MasterStopSource::NotApplicable,
        )
        .unwrap();

        let tick = rec.tick(1_000).unwrap();
        assert!(
            matches!(tick, Tick::Excluded { .. }),
            "應該被排除：{tick:?}"
        );

        let spy = spy.borrow();
        assert!(spy.polled.is_empty(), "排除期間不該讀剪貼簿內容");
        assert_eq!(spy.skipped, vec![1_000], "但一定要把水位推過去");
        assert_eq!(
            ocr.borrow().resets,
            1,
            "排除期間沒看畫面，離開後不准跨過隱私空洞拼舊文字"
        );
    }

    /// 最後一個排除 tick 後到切回普通視窗前，仍可能又複製一次秘密。
    /// 離開邊界必須再推水位，不能只靠排除期間那幾次 skip。
    #[test]
    fn leaving_an_excluded_window_closes_the_clipboard_tail_before_polling() {
        use crate::traits::{CompositeBackend, NullInput, NullScreen};

        struct FocusSequence(std::collections::VecDeque<&'static str>);
        impl crate::traits::FocusSource for FocusSequence {
            fn context(&mut self, _ts: Millis) -> Result<PrivacyObservation> {
                Ok(observed(known_context(
                    FocusSnapshot {
                        app_id: self.0.pop_front().map(str::to_string),
                        ..Default::default()
                    },
                    SensitiveFieldState::Clear,
                )))
            }
            fn is_current(&mut self, permit: CapturePermit) -> Result<bool> {
                Ok(permit == TEST_PERMIT)
            }
        }

        let clipboard = std::rc::Rc::new(std::cell::RefCell::new(SpyClipboard::default()));
        let mut config = Config::default();
        config.privacy.excluded_apps = vec!["keepassxc".into()];
        let backend = CompositeBackend {
            name: "exclusion-tail".into(),
            system: KnownSystem,
            screen: NullScreen,
            focus: FocusSequence(std::collections::VecDeque::from([
                "keepassxc.exe",
                "explorer.exe",
            ])),
            clipboard: clipboard.clone(),
            input: NullInput,
            ocr: crate::traits::NullOcr,
        };
        let mut rec = Recorder::new(
            backend,
            Db::open_in_memory().unwrap(),
            config,
            None,
            MasterStopSource::NotApplicable,
        )
        .unwrap();

        assert!(matches!(rec.tick(1_000).unwrap(), Tick::Excluded { .. }));
        assert_eq!(rec.tick(2_000).unwrap(), Tick::NoScreen);
        let clipboard = clipboard.borrow();
        assert_eq!(
            clipboard.skipped,
            vec![1_000, 2_000],
            "離開排除時，先蓋掉最後一次 skip 後才複製的內容"
        );
        assert_eq!(
            clipboard.polled,
            vec![2_000],
            "普通 tick 可以 poll，但一定排在尾端 skip 後"
        );
        assert_eq!(
            clipboard.trace,
            vec![("skip", 1_000), ("skip", 2_000), ("poll", 2_000)],
            "同一個 tick 的 ordered trace 要證明先封尾、後讀新內容"
        );
    }

    /// 稽核列寫不進去也不能讓 privacy gate 後面的動作一起短路。
    #[test]
    fn an_exclusion_audit_failure_still_closes_every_private_waterline() {
        use crate::traits::{CompositeBackend, NullInput, NullScreen};

        struct ExcludedFocus;
        impl crate::traits::FocusSource for ExcludedFocus {
            fn context(&mut self, _ts: Millis) -> Result<PrivacyObservation> {
                Ok(observed(known_context(
                    FocusSnapshot {
                        app_id: Some("keepassxc.exe".into()),
                        ..Default::default()
                    },
                    SensitiveFieldState::Clear,
                )))
            }
            fn is_current(&mut self, permit: CapturePermit) -> Result<bool> {
                Ok(permit == TEST_PERMIT)
            }
        }

        let clipboard = std::rc::Rc::new(std::cell::RefCell::new(SpyClipboard::default()));
        let ocr = std::rc::Rc::new(std::cell::RefCell::new(OcrLifecycle::default()));
        let mut config = Config::default();
        config.privacy.excluded_apps = vec!["keepassxc".into()];
        let backend = CompositeBackend {
            name: "privacy-failure".into(),
            system: KnownSystem,
            screen: NullScreen,
            focus: ExcludedFocus,
            clipboard: clipboard.clone(),
            input: NullInput,
            ocr: LifecycleOcr(ocr.clone()),
        };
        let mut rec = Recorder::new(
            backend,
            Db::open_in_memory().expect("db"),
            config,
            None,
            MasterStopSource::NotApplicable,
        )
        .expect("recorder");
        rec.db()
            .conn()
            .execute_batch(
                "CREATE TRIGGER fail_exclusion BEFORE INSERT ON system_events
                 WHEN NEW.kind = 'excluded'
                 BEGIN SELECT RAISE(ABORT, 'audit disk failure'); END;",
            )
            .expect("trigger");

        assert!(rec.tick(1_000).is_err(), "稽核列寫不進去仍要往上報");
        assert_eq!(ocr.borrow().resets, 1, "舊 OCR baseline 必須先清掉");
        let clipboard = clipboard.borrow();
        assert!(clipboard.polled.is_empty(), "排除畫面不准讀剪貼簿");
        assert_eq!(
            clipboard.skipped,
            vec![1_000],
            "即使稽核 DB 壞掉，秘密剪貼簿的水位仍要推過去"
        );
    }

    /// 暫停期間也要推剪貼簿水位，理由和排除完全一樣。
    ///
    /// 而且這裡更嚴重：使用者按暫停，往往就是**為了**去複製一個他不想被記住
    /// 的東西。只是「不讀」的話，他一解除暫停，那個東西就在下一個 tick 被撈
    /// 進來——一個只會延後洩漏的暫停鍵，比沒有暫停鍵更糟，因為他信了它。
    #[test]
    fn pausing_skips_the_clipboard_instead_of_merely_not_polling_it() {
        use crate::traits::{CompositeBackend, NullInput, NullScreen};

        let spy = std::rc::Rc::new(std::cell::RefCell::new(SpyClipboard::default()));
        let ocr = std::rc::Rc::new(std::cell::RefCell::new(OcrLifecycle::default()));
        let backend = CompositeBackend {
            name: "spy".into(),
            system: KnownSystem,
            screen: NullScreen,
            focus: KnownFocus,
            clipboard: spy.clone(),
            input: NullInput,
            ocr: LifecycleOcr(ocr.clone()),
        };
        let mut rec = Recorder::new(
            backend,
            Db::open_in_memory().unwrap(),
            Config::default(),
            None,
            MasterStopSource::NotApplicable,
        )
        .unwrap();

        assert!(rec.set_paused(true, 500).unwrap(), "第一次應該真的改變狀態");
        assert_eq!(rec.tick(1_000).unwrap(), Tick::Paused);

        {
            let spy = spy.borrow();
            assert!(spy.polled.is_empty(), "暫停期間不該讀剪貼簿內容");
            assert_eq!(
                spy.skipped,
                vec![500, 1_000],
                "pause request 與暫停 tick 都要把水位推過去"
            );
        }
        assert!(rec.set_paused(false, 1_500).unwrap(), "應該真的解除暫停");
        assert_eq!(
            spy.borrow().skipped,
            vec![500, 1_000, 1_500],
            "解除前要封住最後一個 paused tick 後面的尾巴"
        );
        assert_eq!(
            ocr.borrow().resets,
            1,
            "暫停期間的畫面沒看過，解除後要先全幅重建 OCR baseline"
        );
    }

    /// pause/resume 稽核和公開狀態是一個 transaction：寫失敗就留在舊狀態，
    /// 讓下一拍能重試，而不是先改狀態、從此把缺掉的稽核列當成已完成。
    /// 但只要呼叫端曾經因為 pause request 跳過一拍，畫面 baseline 就得立刻
    /// 切斷；即使使用者在稽核成功前取消，也不能把盲區前後拼在一起。
    #[test]
    fn a_failed_pause_audit_is_retried_before_the_state_changes() {
        use crate::traits::{CompositeBackend, NullInput, NullScreen};

        let ocr = std::rc::Rc::new(std::cell::RefCell::new(OcrLifecycle::default()));
        let clipboard = std::rc::Rc::new(std::cell::RefCell::new(SpyClipboard::default()));
        let backend = CompositeBackend {
            name: "pause-failure".into(),
            system: KnownSystem,
            screen: NullScreen,
            focus: KnownFocus,
            clipboard: clipboard.clone(),
            input: NullInput,
            ocr: LifecycleOcr(ocr.clone()),
        };
        let mut rec = Recorder::new(
            backend,
            Db::open_in_memory().expect("db"),
            Config::default(),
            None,
            MasterStopSource::NotApplicable,
        )
        .expect("recorder");
        rec.db()
            .conn()
            .execute_batch(
                "CREATE TRIGGER fail_pause BEFORE INSERT ON system_events
                 WHEN NEW.kind = 'pause'
                 BEGIN SELECT RAISE(ABORT, 'audit disk failure'); END;",
            )
            .expect("trigger");

        assert!(rec.set_paused(true, 500).is_err());
        assert!(!rec.is_paused(), "稽核沒成立，不准把 transition 認成完成");
        assert_eq!(ocr.borrow().resets, 1, "pause request 先切斷 OCR baseline");
        assert_eq!(
            clipboard.borrow().skipped,
            vec![500],
            "audit 失敗而整拍跳過，也要先推過秘密剪貼簿的水位"
        );

        assert!(
            !rec.set_paused(false, 550).expect("cancel failed pause"),
            "公開狀態沒切換，取消不該捏造 resume 稽核列"
        );
        assert_eq!(
            ocr.borrow().resets,
            1,
            "取消失敗的 pause 也不能把已清掉的 baseline 接回去"
        );
        assert_eq!(
            clipboard.borrow().skipped,
            vec![500, 550],
            "取消前要再推一次，才蓋得到第一次 skip 後才複製的內容"
        );

        rec.db()
            .conn()
            .execute_batch("DROP TRIGGER fail_pause;")
            .expect("drop trigger");
        assert!(rec.set_paused(true, 600).expect("retry"));
        assert!(rec.is_paused());
        assert_eq!(
            ocr.borrow().resets,
            2,
            "新的 pause request 再切斷一次 baseline"
        );
        assert_eq!(clipboard.borrow().skipped, vec![500, 550, 600]);
    }

    #[test]
    fn paused_input_is_not_deferred_until_after_resume() {
        let scenario = Scenario {
            name: "pause-input-boundary".into(),
            privacy_context: ReplayPrivacyContext::Clear,
            system_state: ReplaySystemState::Active,
            steps: vec![
                Step {
                    at_ms: 100,
                    keystrokes: 7,
                    ..Default::default()
                },
                Step {
                    at_ms: 2_100,
                    keystrokes: 3,
                    ..Default::default()
                },
            ],
        };
        let mut recorder = Recorder::new(
            crate::replay::ReplayBackend::new(scenario),
            Db::open_in_memory().expect("db"),
            Config::default(),
            None,
            MasterStopSource::NotApplicable,
        )
        .expect("recorder");

        assert!(recorder.set_paused(true, 0).expect("pause"));
        assert_eq!(recorder.tick(1_000).expect("paused tick"), Tick::Paused);
        assert!(recorder.set_paused(false, 2_000).expect("resume"));
        recorder.tick(2_100).expect("post-resume tick");

        assert_eq!(
            stored_keystrokes(&recorder),
            Some(3),
            "the seven paused keystrokes must not reappear after resume"
        );
    }

    #[test]
    fn master_released_reopens_input_for_the_next_tick() {
        let control = Tmp::new("master-release-input");
        let scenario = Scenario {
            name: "master-release-input-boundary".into(),
            privacy_context: ReplayPrivacyContext::Clear,
            system_state: ReplaySystemState::Active,
            steps: vec![
                Step {
                    at_ms: 100,
                    keystrokes: 7,
                    ..Default::default()
                },
                Step {
                    at_ms: 2_100,
                    keystrokes: 3,
                    ..Default::default()
                },
            ],
        };
        let mut recorder = Recorder::new(
            crate::replay::ReplayBackend::new(scenario),
            Db::open_in_memory().expect("db"),
            Config::default(),
            None,
            MasterStopSource::NotApplicable,
        )
        .expect("recorder");
        recorder.set_master_stop_dir(control.0.clone());

        sister_hands::master_stop::engage(&control.0, 500).expect("engage");
        assert_eq!(
            recorder.tick(1_000).expect("stopped tick"),
            Tick::MasterStopped
        );
        sister_hands::master_stop::release(&control.0).expect("release");
        assert_eq!(
            recorder.tick(2_000).expect("release boundary"),
            Tick::MasterReleased
        );
        assert_eq!(recorder.stats().ticks, 2);
        assert_eq!(recorder.stats().master_stopped_ticks, 1);
        assert_eq!(recorder.stats().master_released_ticks, 1);
        assert_eq!(recorder.stats().working_ticks, 0);
        recorder.tick(2_100).expect("post-release tick");

        assert_eq!(
            stored_keystrokes(&recorder),
            Some(3),
            "MasterReleased must reopen the backend; only post-release input may land"
        );
    }

    #[test]
    fn a_failed_master_release_watermark_never_defers_stopped_clipboard_content() {
        let (mut recorder, calls) = gate_recorder(
            vec![
                SystemReply::Value(SystemObservation::active()),
                SystemReply::Value(SystemObservation::active()),
                SystemReply::Value(SystemObservation::active()),
            ],
            vec![PrivacyReply::Value(clear_privacy())],
        );
        recorder.backend.clipboard_watermarks = [
            ClipboardWatermark::Established,
            ClipboardWatermark::Unknown,
            ClipboardWatermark::Unknown,
        ]
        .into();
        recorder
            .backend
            .clipboard
            .push_back(Some(clipboard_sentinel(300)));

        assert!(
            recorder
                .set_master_stopped(true, 100)
                .expect("engage master stop")
        );
        assert!(
            recorder
                .set_master_stopped(false, 200)
                .expect("release master stop")
        );
        assert!(
            recorder.master_clipboard_gap,
            "unknown release watermark must keep clipboard fail closed"
        );

        recorder.tick(300).expect("ordinary tick after release");
        assert!(
            !calls.borrow().order.contains(&"clipboard"),
            "content must not be read while the master-stop watermark is unknown"
        );
        assert_eq!(
            stored_rows(&recorder, "clipboard_events"),
            0,
            "stopped clipboard content must not be picked up after release"
        );
    }

    #[test]
    fn releasing_pause_does_not_reopen_sources_while_master_stop_overlaps() {
        let (mut recorder, calls) = gate_recorder(Vec::new(), Vec::new());
        assert!(
            recorder
                .set_master_stopped(true, 100)
                .expect("engage master stop")
        );
        assert!(recorder.set_paused(true, 200).expect("pause"));
        calls.borrow_mut().order.clear();

        assert!(recorder.set_paused(false, 300).expect("release pause"));
        assert!(!recorder.pause_input_gap);
        assert!(recorder.master_input_gap);
        assert!(!recorder.pause_clipboard_gap);
        assert!(recorder.master_clipboard_gap);
        assert_eq!(
            calls.borrow().order,
            vec!["clipboard-skip", "input-suspend"],
            "releasing one overlapping reason must not reopen either source"
        );

        assert!(
            recorder
                .set_master_stopped(false, 400)
                .expect("release final reason")
        );
        assert_eq!(
            calls.borrow().order,
            vec![
                "clipboard-skip",
                "input-suspend",
                "clipboard-skip",
                "input-suspend",
                "input-resume",
            ],
            "only the final overlapping reason may reopen input"
        );
    }

    #[test]
    fn cancelling_a_failed_pause_still_discards_its_input_tail() {
        let scenario = Scenario {
            name: "failed-pause-input-boundary".into(),
            privacy_context: ReplayPrivacyContext::Clear,
            system_state: ReplaySystemState::Active,
            steps: vec![
                Step {
                    at_ms: 100,
                    keystrokes: 7,
                    ..Default::default()
                },
                Step {
                    at_ms: 300,
                    keystrokes: 3,
                    ..Default::default()
                },
            ],
        };
        let mut recorder = Recorder::new(
            crate::replay::ReplayBackend::new(scenario),
            Db::open_in_memory().expect("db"),
            Config::default(),
            None,
            MasterStopSource::NotApplicable,
        )
        .expect("recorder");
        recorder
            .db()
            .conn()
            .execute_batch(
                "CREATE TRIGGER fail_pause_input_boundary BEFORE INSERT ON system_events
                 WHEN NEW.kind = 'pause'
                 BEGIN SELECT RAISE(ABORT, 'simulated pause failure'); END;",
            )
            .expect("trigger");

        assert!(recorder.set_paused(true, 0).is_err());
        assert!(!recorder.is_paused());
        recorder
            .tick(100)
            .expect("even an erroneous caller cannot cross the failed request gap");
        assert_eq!(
            stored_keystrokes(&recorder),
            None,
            "a tick before explicit cancellation must leave input fail closed"
        );
        assert!(
            !recorder
                .set_paused(false, 200)
                .expect("cancel failed pause"),
            "a failed pause never creates a resume transition"
        );
        recorder
            .db()
            .conn()
            .execute_batch("DROP TRIGGER fail_pause_input_boundary;")
            .expect("drop trigger");
        recorder.tick(300).expect("normal tick after cancellation");

        assert_eq!(
            stored_keystrokes(&recorder),
            Some(3),
            "input between the failed request and its cancellation must stay discarded"
        );
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
            fn context(&mut self, _ts: Millis) -> Result<PrivacyObservation> {
                let i = self.0;
                self.0 += 1;
                let focus = match i % 4 {
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
                };
                let browser_url = if crate::browsers::is_browser(&focus.app_key()) {
                    focus
                        .url
                        .clone()
                        .map_or(BrowserUrlState::Unknown, BrowserUrlState::Known)
                } else {
                    BrowserUrlState::NotApplicable
                };
                Ok(observed(PrivacyContext::known(
                    focus,
                    SensitiveFieldState::Clear,
                    browser_url,
                )))
            }
            fn is_current(&mut self, permit: CapturePermit) -> Result<bool> {
                Ok(permit == TEST_PERMIT)
            }
        }

        let mut config = Config::default();
        config.privacy.excluded_apps = vec!["brave".into()];
        let backend = CompositeBackend {
            name: "rotating".into(),
            system: KnownSystem,
            screen: NullScreen,
            focus: Rotating(0),
            clipboard: NullClipboard,
            input: NullInput,
            ocr: NullOcr,
        };
        let mut rec = Recorder::new(
            backend,
            Db::open_in_memory().unwrap(),
            config,
            None,
            MasterStopSource::NotApplicable,
        )
        .unwrap();

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
        use crate::traits::{CompositeBackend, NullInput, NullOcr, NullScreen};

        struct BrokenClipboard;
        impl crate::traits::ClipboardSource for BrokenClipboard {
            fn poll(&mut self, _ts: Millis) -> Result<Option<ClipboardEvent>> {
                anyhow::bail!("剪貼簿被別的程式鎖住了")
            }
        }

        let backend = CompositeBackend {
            name: "broken".into(),
            system: KnownSystem,
            screen: NullScreen,
            focus: KnownFocus,
            clipboard: BrokenClipboard,
            input: NullInput,
            ocr: NullOcr,
        };
        let mut rec = Recorder::new(
            backend,
            Db::open_in_memory().unwrap(),
            Config::default(),
            None,
            MasterStopSource::NotApplicable,
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
            privacy_context: ReplayPrivacyContext::Clear,
            system_state: ReplaySystemState::Active,
            steps: vec![Step::default()],
        };
        let mut rec = Recorder::new(
            crate::replay::ReplayBackend::new(scenario),
            Db::open_in_memory().unwrap(),
            Config::default(),
            None,
            MasterStopSource::NotApplicable,
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
            fn drain(&mut self, _ts: Millis) -> Result<Option<sister_core::model::InputTick>> {
                Ok(None)
            }
            fn suspend(&mut self, _ts: Millis) -> Result<()> {
                Ok(())
            }
            fn resume(&mut self, _ts: Millis) -> Result<()> {
                Ok(())
            }
            fn idle_ms(&mut self) -> Option<u64> {
                Some(60 * 60 * 1000) // 一小時沒動
            }
        }

        let looks = Rc::new(Cell::new(0));
        let backend = crate::traits::CompositeBackend {
            name: "idle".into(),
            system: KnownSystem,
            screen: CountingScreen(looks.clone()),
            focus: KnownFocus,
            clipboard: crate::traits::NullClipboard,
            input: NeverTouched,
            ocr: crate::traits::NullOcr,
        };
        let mut rec = Recorder::new(
            backend,
            Db::open_in_memory().unwrap(),
            Config::default(),
            None,
            MasterStopSource::NotApplicable,
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
            fn drain(&mut self, _ts: Millis) -> Result<Option<sister_core::model::InputTick>> {
                Ok(None)
            }
            fn suspend(&mut self, _ts: Millis) -> Result<()> {
                Ok(())
            }
            fn resume(&mut self, _ts: Millis) -> Result<()> {
                Ok(())
            }
            fn idle_ms(&mut self) -> Option<u64> {
                Some(60 * 60 * 1000)
            }
        }
        /// 第三次被問的時候換一個 app——完全沒有人碰鍵盤滑鼠。
        struct Intruder(u32);
        impl crate::traits::FocusSource for Intruder {
            fn context(&mut self, _ts: Millis) -> Result<PrivacyObservation> {
                self.0 += 1;
                Ok(observed(known_context(
                    FocusSnapshot {
                        app_id: Some(
                            if self.0 >= 3 {
                                "installer.exe"
                            } else {
                                "code.exe"
                            }
                            .into(),
                        ),
                        ..Default::default()
                    },
                    SensitiveFieldState::Clear,
                )))
            }
            fn is_current(&mut self, permit: CapturePermit) -> Result<bool> {
                Ok(permit == TEST_PERMIT)
            }
        }

        let looks = Rc::new(Cell::new(0));
        let backend = crate::traits::CompositeBackend {
            name: "intruder".into(),
            system: KnownSystem,
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
            MasterStopSource::NotApplicable,
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
            fn drain(&mut self, _ts: Millis) -> Result<Option<sister_core::model::InputTick>> {
                Ok(None)
            }
            fn suspend(&mut self, _ts: Millis) -> Result<()> {
                Ok(())
            }
            fn resume(&mut self, _ts: Millis) -> Result<()> {
                Ok(())
            }
            fn idle_ms(&mut self) -> Option<u64> {
                Some(60 * 60 * 1000) // 一小時沒動過
            }
        }
        /// 同一個視窗，標題每一拍都不一樣——就是一個在跑的 build。
        struct BuildingTerminal(u32);
        impl crate::traits::FocusSource for BuildingTerminal {
            fn context(&mut self, _ts: Millis) -> Result<PrivacyObservation> {
                self.0 += 1;
                Ok(observed(known_context(
                    FocusSnapshot {
                        app_id: Some("windowsterminal.exe".into()),
                        window_title: Some(format!("cargo build — {}s", self.0)),
                        ..Default::default()
                    },
                    SensitiveFieldState::Clear,
                )))
            }
            fn is_current(&mut self, permit: CapturePermit) -> Result<bool> {
                Ok(permit == TEST_PERMIT)
            }
        }

        let looks = Rc::new(Cell::new(0));
        let backend = crate::traits::CompositeBackend {
            name: "clock".into(),
            system: KnownSystem,
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
            MasterStopSource::NotApplicable,
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
            fn drain(&mut self, _ts: Millis) -> Result<Option<sister_core::model::InputTick>> {
                Ok(None)
            }
            fn suspend(&mut self, _ts: Millis) -> Result<()> {
                Ok(())
            }
            fn resume(&mut self, _ts: Millis) -> Result<()> {
                Ok(())
            }
            fn idle_ms(&mut self) -> Option<u64> {
                Some(60 * 60 * 1000)
            }
        }

        let looks = Rc::new(Cell::new(0));
        let backend = crate::traits::CompositeBackend {
            name: "backwards".into(),
            system: KnownSystem,
            screen: CountingScreen(looks.clone()),
            focus: KnownFocus,
            clipboard: crate::traits::NullClipboard,
            input: NeverTouched,
            ocr: crate::traits::NullOcr,
        };
        let mut rec = Recorder::new(
            backend,
            Db::open_in_memory().unwrap(),
            Config::default(),
            None,
            MasterStopSource::NotApplicable,
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
            KnownSystem,
            ShiftingScreen,
            KnownFocus,
            crate::traits::NullClipboard,
            crate::traits::NullInput,
            O,
        >,
    > {
        let backend = crate::traits::CompositeBackend {
            name: "screen-only".into(),
            system: KnownSystem,
            screen: ShiftingScreen(0),
            focus: KnownFocus,
            clipboard: crate::traits::NullClipboard,
            input: crate::traits::NullInput,
            ocr,
        };
        Recorder::new(
            backend,
            Db::open_in_memory().expect("db"),
            Config::default(),
            None,
            MasterStopSource::NotApplicable,
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
    // 這一組測試釘住的是 alpha.4 當時實測踩到的兩個數字：CPU 27.1%（當時預算 3%）
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
        for (i, p) in px.chunks_exact_mut(4).enumerate() {
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
            KnownSystem,
            CountedScreen,
            KnownFocus,
            crate::traits::NullClipboard,
            crate::traits::NullInput,
            SizeSpy,
        >,
    > {
        let backend = crate::traits::CompositeBackend {
            name: "counted".into(),
            system: KnownSystem,
            screen: CountedScreen { log: log.clone() },
            focus: KnownFocus,
            clipboard: crate::traits::NullClipboard,
            input: crate::traits::NullInput,
            ocr: SizeSpy(log),
        };
        Recorder::new(
            backend,
            Db::open_in_memory().expect("db"),
            Config::default(),
            None,
            MasterStopSource::NotApplicable,
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

    /// 9×8 dHash 會刻意把小變化視為同一張；一個新字也可能落在預設門檻內。
    /// changed-region 若排在它後面又沒有 recheck，局部路徑的單元測試全綠、
    /// production recorder 卻永遠叫不到它，新增的字也不會進 DB。
    #[test]
    fn a_proven_append_inside_the_default_dhash_threshold_reaches_the_database() {
        use std::collections::VecDeque;

        struct Frames(VecDeque<RawFrame>);
        impl crate::traits::ScreenSource for Frames {
            fn grab(&mut self, _ts: Millis) -> Result<Option<RawFrame>> {
                Ok(self.0.pop_front())
            }
        }

        struct ScriptedOcr(VecDeque<Result<Vec<sister_core::model::OcrBlock>>>);
        impl crate::traits::Ocr for ScriptedOcr {
            fn recognize(
                &mut self,
                _frame: &RawFrame,
            ) -> Result<Vec<sister_core::model::OcrBlock>> {
                self.0.pop_front().expect("每次 OCR 都有一份明確結果")
            }
        }

        fn text(text: &str, x: i32, y: i32, w: i32) -> sister_core::model::OcrBlock {
            sister_core::model::OcrBlock {
                text: text.into(),
                x,
                y,
                w,
                h: 20,
                confidence: 1.0,
            }
        }

        let a = RawFrame::from_rgba(0, 0, 512, 512, vec![255; 512 * 512 * 4]);
        let mut changed = vec![255; 512 * 512 * 4];
        for y in 105..115 {
            for x in 200..210 {
                let at = (y * 512 + x) * 4;
                changed[at..at + 3].fill(0);
            }
        }
        let b = RawFrame::from_rgba(1_000, 0, 512, 512, changed.clone());
        // 再多一段像素、但 OCR 仍只讀到 ABCD：模擬行尾游標，不准因此全幅。
        for y in 105..115 {
            for x in 210..220 {
                let at = (y * 512 + x) * 4;
                changed[at..at + 3].fill(0);
            }
        }
        let c = RawFrame::from_rgba(2_000, 0, 512, 512, changed.clone());
        // 同一個近似變化再拍一次，但這次 OCR 引擎真的失敗：仍不能為它補跑
        // 全幅，不過摘要必須留下 failure，且不能冒充第二次「局部未採用」。
        let d = RawFrame::from_rgba(3_000, 0, 512, 512, changed);
        let threshold = Config::default().capture.dedup_threshold;
        assert!(
            sister_core::dedup::hamming(a.dhash, b.dhash) <= threshold,
            "測試前提：這個新字真的會被預設 dHash 吞掉"
        );
        assert!(
            sister_core::dedup::hamming(b.dhash, c.dhash) <= threshold,
            "測試前提：游標形狀也在預設 dHash 門檻內"
        );

        let backend = crate::traits::CompositeBackend {
            name: "dhash-region-integration".into(),
            system: KnownSystem,
            screen: Frames(VecDeque::from([a, b, c, d])),
            focus: KnownFocus,
            clipboard: crate::traits::NullClipboard,
            input: crate::traits::NullInput,
            ocr: crate::ocr_regions::ChangedRegionOcr::new(ScriptedOcr(VecDeque::from([
                Ok(vec![text("ABC", 100, 100, 100)]),
                // crop 是 x=36, y=36 起算，所以全域 (100,100) 在局部是 (64,64)。
                Ok(vec![text("ABCD", 64, 64, 110)]),
                // 第三張 crop 沒讀到新字：只能列成未採用，不能再叫一次全幅。
                Ok(vec![text("ABCD", 64, 64, 110)]),
                Err(anyhow::anyhow!("near-duplicate engine down")),
            ]))),
        };
        let mut recorder = Recorder::new(
            backend,
            Db::open_in_memory().expect("db"),
            Config::default(),
            None,
            MasterStopSource::NotApplicable,
        )
        .expect("recorder");

        assert!(matches!(
            recorder.tick(0).expect("baseline"),
            Tick::Kept { .. }
        ));
        assert!(matches!(
            recorder.tick(1_000).expect("append"),
            Tick::Kept { ocr_blocks: 1, .. }
        ));
        assert_eq!(
            recorder.tick(2_000).expect("cursor-like change"),
            Tick::Duplicate { run: 1 }
        );
        assert_eq!(
            recorder
                .tick(3_000)
                .expect("engine failure stays duplicate"),
            Tick::Duplicate { run: 2 }
        );
        let stats = recorder.stats();
        assert_eq!(stats.kept, 2);
        assert_eq!(stats.duplicates, 2, "第二張升格，後兩張維持重複");
        assert_eq!(stats.ocr_full_frames, 1);
        assert_eq!(stats.ocr_region_frames, 1);
        assert_eq!(stats.ocr_regions, 1);
        assert_eq!(stats.ocr_rejected_region_frames, 1);
        assert_eq!(stats.ocr_rejected_regions, 1);
        assert_eq!(stats.ocr_failures, 1);
        assert!(
            stats
                .last_ocr_error
                .as_deref()
                .is_some_and(|error| error.contains("near-duplicate engine down")),
            "OCR 引擎錯誤要接到 recorder 摘要，不能混成局部結構拒絕"
        );
        assert!(
            !recorder.db().search("ABCD", 10).expect("search").is_empty(),
            "升格後的完整新文字要真的寫進 DB，不只改一個記憶體計數"
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
                system: KnownSystem,
                screen: Flaky(mode.clone()),
                focus: KnownFocus,
                clipboard: crate::traits::NullClipboard,
                input: crate::traits::NullInput,
                ocr: crate::traits::NullOcr,
            };
            let mut r = Recorder::new(
                backend,
                Db::open_in_memory().expect("db"),
                Config::default(),
                None,
                MasterStopSource::NotApplicable,
            )
            .expect("recorder");

            match failure {
                1 => assert_eq!(r.tick(0).expect("鎖屏不是錯誤"), Tick::NoScreen),
                _ => assert!(r.tick(0).is_err(), "抓圖失敗要往上報，不能吞掉"),
            }
            assert_eq!(
                r.stats().last_frame_size,
                None,
                "抓不到畫面就沒有解析度，不能拿 0×0 代替"
            );

            // 同一個畫面回來了。它從來沒被存過，所以必須是新的。
            mode.set(0);
            assert!(
                matches!(r.tick(1_000).expect("tick"), Tick::Kept { .. }),
                "抓不到的第 {failure} 種：沒存成的那一張不能把後面真的那一張擋掉"
            );
            assert_eq!(r.stats().last_frame_size, Some(FULL));
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
            system: KnownSystem,
            screen: ChangingScreen::default(),
            focus: KnownFocus,
            clipboard: crate::traits::NullClipboard,
            input: crate::traits::NullInput,
            ocr: NumberedLines::default(),
        };
        let mut r = Recorder::new(
            backend,
            db,
            Config::default(),
            Some(dir.clone()),
            MasterStopSource::NotApplicable,
        )
        .expect("recorder");

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
        let ocr = std::rc::Rc::new(std::cell::RefCell::new(OcrLifecycle::default()));
        let backend = crate::traits::CompositeBackend {
            name: "held".into(),
            system: KnownSystem,
            screen: Held(screen.clone()),
            focus: KnownFocus,
            clipboard: crate::traits::NullClipboard,
            input: crate::traits::NullInput,
            ocr: LifecycleOcr(ocr.clone()),
        };
        let mut r = Recorder::new(
            backend,
            db,
            Config::default(),
            None,
            MasterStopSource::NotApplicable,
        )
        .expect("recorder");

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
        assert_eq!(
            *ocr.borrow(),
            OcrLifecycle {
                recognized: vec![0, 1_000, 2_000],
                committed: vec![0, 2_000],
                discarded: vec![1_000],
                resets: 0,
            },
            "OCR baseline 只能跟著真的落 DB 的 frame 前進"
        );
        assert_eq!(
            r.stats().ocr_blocks,
            2,
            "DB 失敗那次的文字沒有落地，不能混進『留下幾行』"
        );
    }

    fn step(at_ms: Millis, app: &str, title: &str, text: &[&str]) -> Step {
        Step {
            at_ms,
            app: Some(app.into()),
            title: Some(title.into()),
            url: crate::browsers::is_browser(app).then(|| "https://example.test/".into()),
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
            privacy_context: ReplayPrivacyContext::Clear,
            system_state: ReplaySystemState::Active,
            steps,
        });
        let db = Db::open_in_memory().expect("db");
        Recorder::new(
            backend,
            db,
            config,
            image_dir,
            MasterStopSource::NotApplicable,
        )
        .expect("recorder")
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

    #[test]
    fn master_stop_tick_adds_no_frame_or_text_chunk() {
        let control = Tmp::new("master-stop");
        let mut rec = recorder(
            vec![
                step(0, "code.exe", "A", &["第一段"]),
                step(1_000, "code.exe", "B", &["第二段"]),
            ],
            Config::default(),
        );
        rec.set_master_stop_dir(control.0.clone());
        assert!(matches!(rec.tick(0).unwrap(), Tick::Kept { .. }));
        let before = rec.db().stats().unwrap();
        sister_hands::master_stop::engage(&control.0, 500).unwrap();
        assert_eq!(rec.tick(1_000).unwrap(), Tick::MasterStopped);
        assert_eq!(rec.stats().master_stopped_ticks, 1);
        let after = rec.db().stats().unwrap();
        assert_eq!(after.frames, before.frames, "全停後仍新增畫面");
        assert_eq!(after.chunks, before.chunks, "全停後仍新增文字段落");
    }

    #[test]
    fn master_stop_gap_is_bracketed_by_distinct_database_events() {
        let control = Tmp::new("master-stop-bracket");
        let mut rec = recorder(
            vec![
                step(0, "code.exe", "before", &["全停前"]),
                step(5_000, "code.exe", "after", &["全停後"]),
            ],
            Config::default(),
        );
        rec.set_master_stop_dir(control.0.clone());

        assert!(matches!(rec.tick(0).expect("before"), Tick::Kept { .. }));
        sister_hands::master_stop::engage(&control.0, 1_000).expect("engage");
        for ts in [1_000, 2_000, 3_000] {
            assert_eq!(rec.tick(ts).expect("stopped tick"), Tick::MasterStopped);
        }
        sister_hands::master_stop::release(&control.0).expect("release");
        assert_eq!(
            rec.tick(4_000).expect("release boundary"),
            Tick::MasterReleased
        );
        assert!(matches!(rec.tick(5_000).expect("after"), Tick::Kept { .. }));

        let events: Vec<(String, Millis)> = {
            let mut statement = rec
                .db()
                .conn()
                .prepare(
                    "SELECT kind, ts FROM system_events
                     WHERE kind LIKE 'master_stop_%' ORDER BY ts, id",
                )
                .expect("query master-stop events");
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .expect("map master-stop events")
                .map(|row| row.expect("master-stop event"))
                .collect()
        };
        assert_eq!(
            events,
            vec![
                ("master_stop_engaged".into(), 1_000),
                ("master_stop_released".into(), 4_000),
            ],
            "the two durable rows must explain and bracket the empty interval"
        );
        let frame_times: Vec<Millis> = {
            let mut statement = rec
                .db()
                .conn()
                .prepare("SELECT ts FROM frames ORDER BY ts, id")
                .expect("query frames");
            statement
                .query_map([], |row| row.get(0))
                .expect("map frames")
                .map(|row| row.expect("frame timestamp"))
                .collect()
        };
        assert_eq!(frame_times, vec![0, 5_000]);
        assert!(
            frame_times
                .iter()
                .all(|ts| *ts < events[0].1 || *ts > events[1].1),
            "no captured frame may occupy the audited master-stop gap"
        );
    }

    #[test]
    fn pause_and_master_stop_leave_four_distinguishable_rows() {
        let control = Tmp::new("pause-and-master-stop");
        let mut rec = recorder(Vec::new(), Config::default());
        rec.set_master_stop_dir(control.0.clone());

        assert!(rec.set_paused(true, 100).expect("pause"));
        assert!(rec.set_paused(false, 200).expect("resume"));
        sister_hands::master_stop::engage(&control.0, 300).expect("engage");
        assert_eq!(rec.tick(300).expect("master stop"), Tick::MasterStopped);
        sister_hands::master_stop::release(&control.0).expect("release");
        assert_eq!(rec.tick(400).expect("master release"), Tick::MasterReleased);

        let kinds: Vec<String> = {
            let mut statement = rec
                .db()
                .conn()
                .prepare(
                    "SELECT kind FROM system_events
                     WHERE kind IN ('pause', 'resume',
                                    'master_stop_engaged', 'master_stop_released')
                     ORDER BY ts, id",
                )
                .expect("query stop kinds");
            statement
                .query_map([], |row| row.get(0))
                .expect("map stop kinds")
                .map(|row| row.expect("stop kind"))
                .collect()
        };
        assert_eq!(
            kinds,
            vec![
                "pause",
                "resume",
                "master_stop_engaged",
                "master_stop_released",
            ]
        );
    }

    #[test]
    fn failed_master_stop_audit_stays_fail_closed_and_retries_next_tick() {
        let control = Tmp::new("master-stop-audit-retry");
        let mut rec = recorder(
            vec![step(1_000, "code.exe", "private", &["不准擷取"])],
            Config::default(),
        );
        rec.set_master_stop_dir(control.0.clone());
        rec.db()
            .conn()
            .execute_batch(
                "CREATE TRIGGER fail_master_stop_audit
                 BEFORE INSERT ON system_events
                 WHEN NEW.kind = 'master_stop_engaged'
                 BEGIN SELECT RAISE(ABORT, 'simulated master-stop audit failure'); END;",
            )
            .expect("install failure trigger");
        sister_hands::master_stop::engage(&control.0, 500).expect("engage");

        let error = rec.tick(1_000).expect_err("audit insert must fail");
        assert!(
            format!("{error:#}").contains("write master-stop transition audit"),
            "{error:#}"
        );
        assert!(
            !rec.master_stopped,
            "failed audit cannot acknowledge the transition"
        );
        assert_eq!(
            rec.db()
                .conn()
                .query_row("SELECT COUNT(*) FROM frames", [], |row| row
                    .get::<_, i64>(0))
                .expect("count frames after failure"),
            0,
            "an audit failure must not reverse the fail-closed stop"
        );

        rec.db()
            .conn()
            .execute_batch("DROP TRIGGER fail_master_stop_audit;")
            .expect("remove failure trigger");
        assert_eq!(
            rec.tick(2_000).expect("retry same latch"),
            Tick::MasterStopped
        );
        assert!(rec.master_stopped, "successful retry acknowledges the stop");
        assert_eq!(
            rec.db()
                .conn()
                .query_row(
                    "SELECT COUNT(*) FROM system_events
                     WHERE kind = 'master_stop_engaged'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .expect("count retried audit"),
            1
        );
        assert_eq!(
            rec.db()
                .conn()
                .query_row("SELECT COUNT(*) FROM frames", [], |row| row
                    .get::<_, i64>(0))
                .expect("count frames after retry"),
            0,
            "the retry tick must still capture nothing"
        );
    }

    #[test]
    fn unchanged_master_stop_ticks_do_not_duplicate_the_audit() {
        let control = Tmp::new("master-stop-no-duplicates");
        let mut rec = recorder(Vec::new(), Config::default());
        rec.set_master_stop_dir(control.0.clone());
        sister_hands::master_stop::engage(&control.0, 500).expect("engage");

        for ts in [1_000, 2_000, 3_000, 4_000] {
            assert_eq!(rec.tick(ts).expect("stopped tick"), Tick::MasterStopped);
        }
        assert_eq!(
            rec.db()
                .conn()
                .query_row(
                    "SELECT COUNT(*) FROM system_events
                     WHERE kind = 'master_stop_engaged'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .expect("count engaged audit"),
            1,
            "only the first observation of an unchanged latch is an event"
        );
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
            KnownSystem,
            ChangingScreen,
            KnownFocus,
            crate::traits::NullClipboard,
            crate::traits::NullInput,
            NumberedLines,
        >,
    > {
        let backend = crate::traits::CompositeBackend {
            name: "images".into(),
            system: KnownSystem,
            screen: ChangingScreen::default(),
            focus: KnownFocus,
            clipboard: crate::traits::NullClipboard,
            input: crate::traits::NullInput,
            ocr: NumberedLines::default(),
        };
        Recorder::new(
            backend,
            Db::open_in_memory().expect("db"),
            config,
            Some(dir),
            MasterStopSource::NotApplicable,
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
            system: KnownSystem,
            screen: ChangingScreen::default(),
            focus: KnownFocus,
            clipboard: crate::traits::NullClipboard,
            input: crate::traits::NullInput,
            ocr: NumberedLines::default(),
        };

        let now = sister_core::now_ms();
        let mut r = Recorder::new(
            backend(),
            db,
            config.clone(),
            Some(tmp.0.clone()),
            MasterStopSource::NotApplicable,
        )
        .expect("recorder");
        r.tick(now).expect("tick");
        assert_eq!(count_pngs(&tmp.0), 1);
        let db = r.into_db();

        // 第二次啟動：這一天已經用掉的量必須從資料庫接回來
        let r2 = Recorder::new(
            backend(),
            db,
            config,
            Some(tmp.0.clone()),
            MasterStopSource::NotApplicable,
        )
        .expect("recorder");
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

    #[test]
    fn a_valid_allowed_tick_separates_two_exclusion_segments() {
        let mut r = recorder(
            vec![
                step(0, "1password", "Vault", &["secret"]),
                step(1_000, "notes.exe", "Notes", &["ordinary"]),
                step(2_000, "1password", "Vault", &["secret again"]),
            ],
            Config::default(),
        );

        assert!(matches!(
            r.tick(0).expect("first exclusion"),
            Tick::Excluded { .. }
        ));
        assert!(!matches!(
            r.tick(1_000).expect("validated allowed segment"),
            Tick::Excluded { .. }
        ));
        assert!(matches!(
            r.tick(2_000).expect("second exclusion"),
            Tick::Excluded { .. }
        ));

        let exclusions: i64 = r
            .db()
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM system_events WHERE kind='excluded'",
                [],
                |row| row.get(0),
            )
            .expect("count exclusion segments");
        assert_eq!(
            exclusions, 2,
            "the allowed segment must reset exclusion debounce"
        );
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
                clipboard_source_app: Some("terminal".into()),
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
                app: Some("code.exe".into()),
                clipboard_source_app: Some("code.exe".into()),
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
    fn browser_clipboard_without_origin_url_proof_never_reaches_the_database() {
        let mut r = recorder(
            vec![Step {
                at_ms: 0,
                app: Some("code.exe".into()),
                clipboard_source_app: Some("chrome.exe".into()),
                text: vec!["已切回編輯器".into()],
                clipboard: Some("銀行頁複製的帳號 0800-080-123".into()),
                ..Default::default()
            }],
            Config::default(),
        );
        r.tick(0).expect("tick");
        assert_eq!(r.stats().clipboard_source_excluded, 1);
        assert!(
            r.db()
                .search("銀行頁複製的帳號", 10)
                .expect("search")
                .is_empty(),
            "只有 browser owner app、沒有 origin URL 的 bytes 不可落庫"
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
        let mut r = recorder(vec![step(0, "code.exe", "帳單", &["密碼 1234"])], config);

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

    #[test]
    fn a_backend_name_cannot_mint_windows_url_provenance() {
        let backend = crate::traits::CompositeBackend {
            // From<&str> 永遠走 untrusted namespace；看起來一模一樣也不行。
            name: sister_core::db::TRUSTED_URL_ORIGIN_PLATFORM.into(),
            system: KnownSystem,
            screen: crate::traits::NullScreen,
            focus: KnownFocus,
            clipboard: crate::traits::NullClipboard,
            input: crate::traits::NullInput,
            ocr: crate::traits::NullOcr,
        };
        let recorder = Recorder::new(
            backend,
            Db::open_in_memory().expect("db"),
            Config::default(),
            None,
            MasterStopSource::NotApplicable,
        )
        .expect("recorder");
        let platform: String = recorder
            .db()
            .conn()
            .query_row("SELECT platform FROM sessions WHERE id = 1", [], |row| {
                row.get(0)
            })
            .expect("platform");
        assert!(platform.starts_with("untrusted/"), "{platform}");
        assert_ne!(platform, sister_core::db::TRUSTED_URL_ORIGIN_PLATFORM);
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
