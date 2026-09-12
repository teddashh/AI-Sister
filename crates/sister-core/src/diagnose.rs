//! 一份跑完可以直接貼出來的診斷報告。
//!
//! ## 為什麼是報告，不是「再多記一點 log」
//!
//! 這台機器上該記的東西幾乎都已經在記了：`desktop.log`／`record.log` 有
//! tracing 的每一行，`brain_outbound` 有**每一趟**外送的毫秒數、字數與結局
//! （答題那兩趟還分得出來是哪一趟，見 `role`），`brain_skip` 有每一次「沒
//! 問」的理由，`capabilities.json` 有上一場錄製量到的能力。缺的從來不是資
//! 料，是**有人把它們讀出來**——和 [`crate::capabilities`] 開頭那句「那個檔案
//! 沒有人會開」是同一個病。
//!
//! 所以這裡不新增第二套記錄，只做一件事：把已經在磁碟上的東西讀成一份人看得
//! 懂、而且可以整份貼給別人的字。
//!
//! ## 兩半
//!
//! - **被動**：上面那些檔案與資料表。跨得過重開機，因為它們在磁碟上。
//! - **主動自檢**：畫面自己量自己（氣泡多高、捲得到捲不到、上面那一槓現在
//!   看不看得見）。這一半只有桌面版按下那顆鈕才有——命令列量不到畫面，那時
//!   候這一節會印 [`Absent::NotHere`] 而不是安靜消失。一節不見了，和一節沒問
//!   題，在紙上長得一模一樣。
//!
//! ## 哪幾格可能有你的東西
//!
//! 報告分成兩半，中間有一條線。線以上**放不下**螢幕上的內容——不是因為有人
//! 記得要遮，是因為那幾個型別沒有能裝它的欄位：
//!
//! - [`Leg`] 有 `duration_ms`、`chars_sent`、`outcome`，**沒有 `error: String`**。
//!   `outcome` 留原字是因為它每一個值都是我們自己原始碼裡的字面值
//!   （`kind.as_str()`、`"success"`、`"bad_json"`…）；`error` 不是——它是
//!   `format!` 把 CLI 回來的東西包進去組出來的，serde 的 `invalid type: string
//!   "…"` 就會把模型吐的字帶進來。所以 `error` 在這裡只剩
//!   [`ErrorKind`] 一個代號加上字數。
//! - [`SkipCount`] 只有代號和次數，不是 `SkipReason::message()` 那句話。
//! - [`Measure`] 的字串值是 [`Word`]，而 `Word` 只收得下 `[a-z0-9_-]{1,32}`。
//!   中文一個字都進不去。
//!
//! 線以下兩節就會有：她的答案原文（Ted 要看她講話像不像朋友，那非看原文不
//! 可）和 log 尾巴。兩節各自獨立、各自可以整段刪掉，而且**報告自己會把兩邊
//! 的字數印出來**——不是宣稱「上面很乾淨」，是把數字放在那裡讓人自己看。
//!
//! log 尾巴會過一次 [`Scrubber`]：認得出來的路徑和使用者名稱換成代號。這是
//! 一份**黑名單**，黑名單永遠不是保證，所以報告裡就這樣寫。
//!
//! ## 為什麼資料目錄裡不會多一個檔案
//!
//! 報告寫到使用者指定的地方，不寫進 data dir。data dir 裡每多一個檔案，
//! `forget`／`export`／`prune` 三條路就各要接一次，而且它自己會變成一個新的
//! 隱私面。跑這一輪才發生的事情放在記憶體裡（見 [`Snapshot::run_started_at`]），
//! 報告會明講那一段從什麼時候開始看得見——通常是這一次開機，但簿子滿了會從
//! 頭滾掉舊的，那時候涵蓋的比一整輪還少，報告要照實說。

use std::path::Path;

use crate::capabilities;
use crate::consent::{self, Consent, Sheet};
use crate::db::{DbStats, OutboundRow, SkipRow};
use crate::model::{Millis, stamp};

/// 報告格式的版本。貼回來的人和讀報告的人不一定同一版，第一行就要講清楚。
pub const REPORT_VERSION: u32 = 1;

/// 線以上／線以下的分隔。整段刪掉的人照著這一行刪。
const DIVIDER: &str =
    "════ 以下是你自己的東西。貼之前先看一眼；不想給就從這一行往下整段刪掉。 ════";

/// 一節為什麼不在。
///
/// 刻意是封閉的 enum 而不是 `String`：`io::Error` 的 `Display` 裡帶著完整路
/// 徑，而路徑裡有使用者名稱。要講「讀不到」不需要把路徑一起講出去。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Absent {
    /// 檔案／資料庫根本不在。
    NotThere,
    /// 在，但讀壞了。帶 `io::ErrorKind`——那也是一個封閉的 enum。
    Unreadable(std::io::ErrorKind),
    /// 資料庫打得開，但這一格查不出來。
    QueryFailed,
    /// 這個管道量不到（命令列量不到畫面）。
    NotHere,
}

impl Absent {
    fn say(self) -> String {
        match self {
            Absent::NotThere => "沒有這個檔案".to_string(),
            Absent::Unreadable(kind) => format!("讀不到（{kind:?}）"),
            Absent::QueryFailed => "資料庫在，但這一格查不出來".to_string(),
            Absent::NotHere => "這個管道量不到".to_string(),
        }
    }
}

/// 有值，或者沒有值而且說得出為什麼。
///
/// `Option` 不夠用：`None` 印出來是一片空白，而一片空白讀起來像「沒問題」。
pub type Got<T> = Result<T, Absent>;

// ───────────────────────────── 外送那幾趟 ─────────────────────────────

/// 一趟外送。`brain_outbound` 的一列，扣掉裝得下原文的那個欄位。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Leg {
    pub ts: Millis,
    /// `answer_search`／`answer`／`interpreter`。產品自己的字面值。
    pub role: String,
    /// `success`／`bad_json`／`spawn_failed`／`no_answer`／`cancelled`…
    /// 每一個都來自我們自己原始碼裡的字面值（`kind.as_str()` 或直接寫死）。
    pub outcome: String,
    pub duration_ms: i64,
    pub chars_sent: i64,
    pub truncated: bool,
    /// 出錯的話是哪一類。**不帶錯誤原文**，理由見模組開頭。
    pub error: Option<ErrorSummary>,
}

/// 一則錯誤剩下來可以講的部分。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ErrorSummary {
    pub kind: ErrorKind,
    /// 原始錯誤字串有多少字。留著是因為「700 字的 bad_json」和「20 字的
    /// bad_json」是兩件不同的事（前者多半是模型把整篇文章塞進來了）。
    pub chars: usize,
}

/// 錯誤的類別。
///
/// 分類器比對的是我們自己原始碼裡那幾句錯誤訊息的**開頭**。認不出來就是
/// [`ErrorKind::Other`]——安全，只是比較沒用，所以底下有一條測試盯著
/// `grounded_answer.rs`：那邊長出新的錯誤句而這裡沒跟上，測試會紅。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// 子行程根本沒有啟動：啟動前就被取消，或者三層全停已經生效。
    /// 和 [`ErrorKind::CliFailed`] 分開，因為「沒去問」和「問了沒成」
    /// 要修的是兩件不同的事。
    NeverStarted,
    /// CLI 起不來、放不進行程樹、管線沒開成，或者非零結束碼。
    CliFailed,
    /// stdout 根本不是 JSON。
    NotJson,
    /// 是 JSON，但欄位對不上契約。
    ContractMismatch,
    /// 句子本身違規：空句、超長、一格塞多句、沒有來源。
    BadSentence,
    /// 引用了看不懂的、或者這次根本沒提供的來源。
    BadSource,
    /// 查本機記憶的那份計畫違規：空問句、控制字元、超長、去重後是空的。
    BadPlan,
    /// 以上都不是。
    Other,
}

impl ErrorKind {
    /// 認得出來的開頭。**這裡是唯一一份清單**，測試拿它去對 `grounded_answer.rs`。
    const PREFIXES: &'static [(&'static str, ErrorKind)] = &[
        ("CLI invocation 在啟動前已取消", ErrorKind::NeverStarted),
        ("三層全停已在 CLI 啟動前生效", ErrorKind::NeverStarted),
        ("CLI 結束碼", ErrorKind::CliFailed),
        ("叫不起", ErrorKind::CliFailed),
        ("stdout 管線沒開成", ErrorKind::CliFailed),
        ("工作執行緒炸了", ErrorKind::CliFailed),
        ("無法把", ErrorKind::CliFailed),
        ("stdout 不是單一 JSON 物件", ErrorKind::NotJson),
        ("JSON 對不上回答契約", ErrorKind::ContractMismatch),
        ("JSON 對不上查詢契約", ErrorKind::BadPlan),
        ("回答缺少", ErrorKind::ContractMismatch),
        ("回答裡有空句", ErrorKind::BadSentence),
        ("回答單句超過", ErrorKind::BadSentence),
        ("回答把多句或控制字元", ErrorKind::BadSentence),
        ("回答裡有一句沒有來源", ErrorKind::BadSentence),
        ("回答引用了看不懂的來源", ErrorKind::BadSource),
        ("回答引用了這次沒有提供的來源", ErrorKind::BadSource),
        ("queries 裡有空問句", ErrorKind::BadPlan),
        ("queries 裡有控制字元", ErrorKind::BadPlan),
        ("queries 去重後是空的", ErrorKind::BadPlan),
        ("單條 query 超過", ErrorKind::BadPlan),
    ];

    /// 開頭是動態的那幾句。
    ///
    /// `` `{command}` 已放進子行程範圍，但… `` 的第一個字就是使用者機器上的
    /// 執行檔路徑——那句話沒有固定的開頭可以比。這一格順便說明了為什麼
    /// `error` 原文不能印：它的第一個字就可能是一條路徑。
    const CONTAINS: &'static [(&'static str, ErrorKind)] = &[
        ("已放進子行程範圍", ErrorKind::CliFailed),
        ("suspended primary thread", ErrorKind::CliFailed),
    ];

    pub fn classify(error: &str) -> ErrorKind {
        let trimmed = error.trim_start();
        for (prefix, kind) in Self::PREFIXES {
            if trimmed.starts_with(prefix) {
                return *kind;
            }
        }
        for (needle, kind) in Self::CONTAINS {
            if trimmed.contains(needle) {
                return *kind;
            }
        }
        ErrorKind::Other
    }

    fn say(self) -> &'static str {
        match self {
            ErrorKind::NeverStarted => "根本沒啟動",
            ErrorKind::CliFailed => "CLI 沒跑成",
            ErrorKind::NotJson => "回來的不是 JSON",
            ErrorKind::ContractMismatch => "JSON 對不上契約",
            ErrorKind::BadSentence => "句子違規",
            ErrorKind::BadSource => "出處引錯",
            ErrorKind::BadPlan => "查詢計畫違規",
            ErrorKind::Other => "其他",
        }
    }
}

/// `role` 與 `outcome` 原樣印出來的前提是「每一個值都是我們自己原始碼裡的
/// 字面值」。那句話今天是真的（`kind.as_str()`、`"success"`、`"bad_json"`…），
/// 但它是一句**假設**，而假設會過期：多一個寫入端、或者資料庫被別的東西改
/// 過，這一格就變成一條沒有人守的通道。
///
/// 所以這裡把它變成有人守的：不長這個樣子的值不印出來，只講它有多長。
/// 代價是萬一真的有人加了一個帶大寫的 token，報告會印成 `?（12 字）`——
/// 那是一則看得見的怪，比一句看不見的原文好。
fn token_or_length(raw: &str) -> String {
    let shaped = !raw.is_empty()
        && raw.len() <= 32
        && raw
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-');
    if shaped {
        raw.to_string()
    } else {
        format!("?（{} 字）", raw.chars().count())
    }
}

impl Leg {
    /// 從一列稽核紀錄轉過來。**這是 `error` 唯一的入口**，而它在這裡就被
    /// 壓成代號了——換句話說，`Leg` 拿不到原文不是紀律問題，是它沒有那個欄位。
    pub fn from_row(row: &OutboundRow) -> Leg {
        Leg {
            ts: row.ts,
            role: token_or_length(&row.role),
            outcome: token_or_length(&row.outcome),
            duration_ms: row.duration_ms,
            chars_sent: row.chars_sent,
            truncated: row.truncated,
            error: row.error.as_deref().map(|error| ErrorSummary {
                kind: ErrorKind::classify(error),
                chars: error.chars().count(),
            }),
        }
    }
}

/// 「沒問」的一種理由，和它出現幾次。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkipCount {
    /// `no_consent`／`budget_exhausted`／… 代號，不是那句話。
    pub reason: String,
    pub times: usize,
}

impl SkipCount {
    /// 把最近幾列折成計數。時間順序在這裡沒有意義，會不會發生才有。
    pub fn fold(rows: &[SkipRow]) -> Vec<SkipCount> {
        let mut out: Vec<SkipCount> = Vec::new();
        for row in rows {
            let reason = token_or_length(&row.reason);
            match out.iter_mut().find(|c| c.reason == reason) {
                Some(existing) => existing.times += 1,
                None => out.push(SkipCount {
                    reason: token_or_length(&row.reason),
                    times: 1,
                }),
            }
        }
        out.sort_by(|a, b| b.times.cmp(&a.times).then_with(|| a.reason.cmp(&b.reason)));
        out
    }
}

// ───────────────────────────── 自檢 ─────────────────────────────

/// 自檢裡一個字串值。
///
/// 存在的唯一理由是**讓畫面那半沒有辦法**把 `textContent` 送進來。
/// `[a-z0-9_-]{1,32}`：`open`、`hidden`、`chrome-open` 進得來，
/// 螢幕上的字一個都進不來。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Word(String);

impl Word {
    pub fn new(raw: &str) -> Option<Word> {
        if raw.is_empty() || raw.len() > 32 {
            return None;
        }
        if !raw
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
        {
            return None;
        }
        Some(Word(raw.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 自檢量到的一個值。
#[derive(Debug, Clone, PartialEq)]
pub enum MeasureValue {
    Int(i64),
    /// 像素、秒這種會有小數的。
    Num(f64),
    Flag(bool),
    Word(Word),
    /// 一條擋下來的理由。
    ///
    /// 裝的還是 [`Word`]——畫面那半一樣送不進自由文字——只是印出來的時候換
    /// 成他讀得懂的話。中文是這一邊自己的字典，不是對面送來的。
    Blocker(Word),
    /// 一題為什麼沒答成。
    ///
    /// 和 [`MeasureValue::Blocker`] 分成兩個變體，不是共用一本字典：兩邊各
    /// 有一個 `consent_sheet`／`consent_asked`，意思不一樣（一個是「同意書
    /// 開著所以她不笑」，一個是「答案回來之後才跳出同意書」）。合成一本的
    /// 話，下一次撞名不會編譯錯誤，只會安靜印錯那一句。
    AskWhy(Word),
}

impl MeasureValue {
    fn say(&self) -> String {
        match self {
            MeasureValue::Int(v) => group(*v),
            MeasureValue::Num(v) => format!("{v:.1}"),
            MeasureValue::Flag(true) => "是".to_string(),
            MeasureValue::Flag(false) => "否".to_string(),
            MeasureValue::Word(w) => w.as_str().to_string(),
            MeasureValue::Blocker(w) => giggle_blocker(w.as_str()).to_string(),
            MeasureValue::AskWhy(w) => ask_why(w.as_str()).to_string(),
        }
    }
}

/// 擋下那一聲笑的理由，翻成他讀得懂的話。
///
/// 認不得就原樣把代號印出來，**不要印「不明」**。畫面那半新加一條條件而這裡
/// 忘了跟上的時候，代號至少還說得出是哪一條；「不明」會把「我沒跟上」講成
/// 「不知道為什麼」——而這一格存在的理由正好是要說出為什麼。
fn giggle_blocker(word: &str) -> &str {
    match word {
        "persona_off" => "角色關著",
        "no_lines" => "這個角色沒有台詞",
        "hidden" => "視窗不在前面",
        "busy" => "她正在忙別的",
        "paused" => "暫停中",
        "stopping" => "正在全部停下來",
        "consent_sheet" => "同意書開著",
        "voice_sheet" => "正在試聽語音",
        "typing" => "你正在打字",
        "no_line_picked" => "抽不到還沒講過的句子",
        other => other,
    }
}

/// 一題為什麼沒答成，翻成他讀得懂的話。
///
/// 和 [`giggle_blocker`] 一樣：認不得就原樣印代號，不要印「不明」。
///
/// `unknown` 那一條是留給**還沒寫進來的出口**的。`ask()` 只有一個記錄點
/// （那個 `finally`），所以以後誰加第六條出口而忘了設 `gaveUp`，報告上會
/// 出現「畫面那半有一條出口沒講它是哪一條」——那句話指名的是我，不是他。
fn ask_why(word: &str) -> &str {
    match word {
        "error" => "出錯了，錯誤訊息在畫面上和底下的 log 尾巴",
        "consent_asked" => "先跳出同意書，這一題還沒答",
        "superseded" => "你又送了下一題，這一題不算了",
        "not_presented" => "畫面那關沒讓它畫出來",
        "unknown" => "畫面那半有一條出口沒講它是哪一條",
        other => other,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Measure {
    pub label: String,
    pub value: MeasureValue,
}

/// 一項自檢的結論。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// 量到了，而且是預期的樣子。
    AsAsked,
    /// 量到了，不是預期的樣子。
    Off,
    /// 這一輪沒發生過，量不到。**不是「沒問題」。**
    NotSeen,
    /// 機器判不了這一格。
    ///
    /// 「她講話像不像朋友」沒有一個數字答得出來，而**硬給一個 ✓ 比留白更糟**
    /// ——它會讓讀的人以為有人驗過了。
    CantJudge,
    /// 條件到了，可是量測本身沒跑起來。
    ///
    /// 和 [`Verdict::NotSeen`] 差在他該做什麼：「沒發生過」是叫他再多玩一
    /// 下，「沒跑起來」是叫我去修。兩個都印成空白的話，看報告的人只會做前
    /// 面那件事，而錯的是後面那件。
    NotMeasured,
    /// 試過了，可是每一次都沒走到底。
    ///
    /// 空白有三種，這是第三種。三種要他做的事都不一樣：
    ///
    /// * [`Verdict::NotSeen`]——他還沒去做那件事。多玩一會兒再匯出。
    /// * [`Verdict::NeverLanded`]——他做了，每一次都被別的事情擋掉。原因就
    ///   在底下那幾格，而那個原因決定該改的是設定還是我。
    /// * [`Verdict::NotMeasured`]——事情發生了，是這一格量不出來。我去修。
    ///
    /// 少了中間這一種，最常見的那次失敗（CLI 還沒設好，一問就錯）會印成第
    /// 一種——叫他去做他已經做過的事。
    NeverLanded,
}

impl Verdict {
    fn mark(self) -> &'static str {
        match self {
            Verdict::AsAsked => "✓",
            Verdict::Off => "✗",
            Verdict::NotSeen => "－",
            Verdict::CantJudge => "？",
            Verdict::NotMeasured => "！",
            Verdict::NeverLanded => "…",
        }
    }

    fn say(self) -> &'static str {
        match self {
            Verdict::AsAsked => "照你要的",
            Verdict::Off => "不對",
            Verdict::NotSeen => "這一輪沒發生過，量不到",
            Verdict::CantJudge => "機器判不了，答案原文在線下面 ⑤，你自己看",
            Verdict::NotMeasured => "該量到卻沒量到——不是這一輪沒發生，是這一格量不出來",
            Verdict::NeverLanded => "你做了，可是每一次都沒走到底——底下那幾格寫著為什麼",
        }
    }
}

/// 自檢的一項。編號對著 Ted 當初列的那七件事。
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub number: u8,
    /// 他當初那句話的縮寫。放在報告裡，讀的人才知道這一格在回答什麼。
    pub asked: String,
    pub measured: Vec<Measure>,
    pub verdict: Verdict,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SelfCheck {
    pub at: Millis,
    /// 這一輪什麼時候開的。自檢只看得到這之後發生的事。
    pub run_started_at: Millis,
    /// 自檢真的看得到的起點。擠掉過觀測的話，它比 `run_started_at` 晚。
    pub covers_from: Millis,
    /// 擠掉了幾則觀測。
    pub dropped: usize,
    pub items: Vec<Item>,
}

// ───────────────────────────── 線以下 ─────────────────────────────

/// 她講過的一段話。**只有她講的**——問題是他打的，這裡只帶字數。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub at: Millis,
    pub question_chars: usize,
    pub sentences: Vec<String>,
    /// 每一句掛的出處代號，例如 `文字#5443`。
    pub sources: Vec<String>,
    /// 從按下去到畫面上出現，一共多久。
    pub took_ms: Option<i64>,
}

/// 一個 log 檔的尾巴。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogTail {
    pub name: String,
    pub lines: Got<Vec<String>>,
    /// 原本一共幾行。截掉多少，讀的人有權知道。
    pub total_lines: usize,
}

// ───────────────────────────── 整份 ─────────────────────────────

// `PartialEq` 沒有 derive：`capabilities::Report` 沒有，而整份快照從來不用
// 相等比較——測試比的是印出來的字，不是結構。
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub at: Millis,
    pub app_version: String,
    /// `windows`／`linux`／`macos`。
    pub platform: String,
    /// 從哪裡跑出來的：`sister diagnose` 還是桌面版那顆鈕。
    pub source: String,
    /// 這一輪什麼時候開的。`None` = 不知道（命令列跑的時候本來就不知道）。
    pub run_started_at: Option<Millis>,
    /// 資料夾路徑，已經 scrub 過。
    pub data_dir_shown: String,
    pub consent: ConsentLines,
    pub doctor: Got<capabilities::Report>,
    pub db: Got<DbStats>,
    pub legs: Got<Vec<Leg>>,
    pub skips: Got<Vec<SkipCount>>,
    pub self_check: Got<SelfCheck>,
    pub answers: Got<Vec<Answer>>,
    pub logs: Vec<LogTail>,
}

/// 四張同意書現在的樣子。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsentLines {
    pub sheets: Vec<(String, Option<Millis>, bool)>,
    pub file_version: u32,
    pub cloud_terms_version: u32,
    pub azure_terms_version: u32,
}

impl ConsentLines {
    pub fn of(consent: &Consent) -> ConsentLines {
        ConsentLines {
            sheets: Sheet::ALL
                .iter()
                .map(|sheet| {
                    (
                        sheet.key().to_string(),
                        consent.get(*sheet),
                        consent.effective(*sheet),
                    )
                })
                .collect(),
            file_version: consent.version,
            cloud_terms_version: consent.cloud_reading_terms_version,
            azure_terms_version: consent.azure_tts_terms_version,
        }
    }

    /// 直接從 data dir 讀。讀不到就是沒簽——和 [`consent::load`] 同一條紀律。
    pub fn load(data_dir: &Path) -> ConsentLines {
        ConsentLines::of(&consent::load(data_dir))
    }
}

// ───────────────────────────── 遮蔽 ─────────────────────────────

/// 把認得出來的路徑與名字換成代號。
///
/// **這是黑名單，不是保證。** 它只換得掉我知道要找的東西——報告裡就是這樣寫
/// 的，因為一句「已遮蔽」會讓人不再自己看一眼，而那正是這種東西最貴的失敗
/// 方式。
#[derive(Debug, Clone, Default)]
pub struct Scrubber {
    needles: Vec<(String, String)>,
}

impl Scrubber {
    pub fn new() -> Scrubber {
        Scrubber::default()
    }

    /// 加一個要藏的東西。路徑會自動連 `/` 與 `\` 兩種寫法一起收。
    pub fn hide(&mut self, needle: &str, as_: &str) {
        let needle = needle.trim();
        if needle.len() < 3 {
            // 兩個字元的「針」會把整份報告打成馬賽克，反而看不出發生什麼事。
            return;
        }
        for variant in [needle.replace('\\', "/"), needle.replace('/', "\\")] {
            if !self.needles.iter().any(|(n, _)| *n == variant) {
                self.needles.push((variant, as_.to_string()));
            }
        }
        // 長的先換：data dir 這根針把家目錄整個含在裡面，順序反了就只剩半截。
        self.needles
            .sort_by(|a, b| b.0.len().cmp(&a.0.len()).then_with(|| a.0.cmp(&b.0)));
    }

    /// 家目錄、資料夾、使用者名稱這三根針一次補齊。
    pub fn hide_paths(&mut self, data_dir: &Path, home: Option<&Path>, user: Option<&str>) {
        self.hide(&data_dir.display().to_string(), "<資料夾>");
        if let Some(home) = home {
            self.hide(&home.display().to_string(), "<家目錄>");
        }
        if let Some(user) = user {
            self.hide(user, "<使用者>");
        }
    }

    pub fn apply(&self, text: &str) -> String {
        if self.needles.is_empty() {
            return text.to_string();
        }
        // 找的時候用一份「小寫 + 斜線統一」的副本。兩種轉換都不改位元組長度
        // （ASCII 大小寫等長、`\` 換 `/` 等長），所以偏移量和原字串對得起來，
        // 可以照原樣切。Windows 的路徑大小寫不一定，這樣才收得到。
        let hay = normalize(text);
        let hay = hay.as_bytes();
        // 針只正規化一次。順序沿用 `hide` 排好的「長的在前」。
        let needles: Vec<(Vec<u8>, &str)> = self
            .needles
            .iter()
            .map(|(needle, replacement)| (normalize(needle).into_bytes(), replacement.as_str()))
            .collect();
        let mut out = String::with_capacity(text.len());
        let mut i = 0usize;
        'outer: while i < text.len() {
            for (needle, replacement) in &needles {
                if needle.is_empty() || i + needle.len() > hay.len() {
                    continue;
                }
                // **比位元組，不要切字串。** `hay[i..i + n]` 在 `i + n` 落在
                // 一個中文字中間的時候會直接 panic——而報告裡到處是中文。
                // 位元組比不會，而且不會誤判：`i` 永遠停在 char 邊界上，
                // UTF-8 又是自同步的，所以從邊界開始比中一整根合法的針，
                // 結尾也一定落在邊界上。
                if &hay[i..i + needle.len()] == needle.as_slice() {
                    out.push_str(replacement);
                    i += needle.len();
                    continue 'outer;
                }
            }
            // 一次推進一個 char，不然會切壞 UTF-8。
            let ch = text[i..].chars().next().expect("i 落在 char 邊界上");
            out.push(ch);
            i += ch.len_utf8();
        }
        out
    }
}

fn normalize(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c == '\\' {
                '/'
            } else {
                c.to_ascii_lowercase()
            }
        })
        .collect()
}

/// 取最後幾行，而且每一行都有上限。
///
/// 兩個上限都要：一份 log 可能只有三行，但其中一行是三十萬字的 JSON dump。
pub fn tail_lines(text: &str, max_lines: usize, max_chars_per_line: usize) -> (Vec<String>, usize) {
    let all: Vec<&str> = text.lines().collect();
    let total = all.len();
    let start = total.saturating_sub(max_lines);
    let lines = all[start..]
        .iter()
        .map(|line| {
            let line = line.trim_end_matches('\r');
            let chars = line.chars().count();
            if chars <= max_chars_per_line {
                line.to_string()
            } else {
                let kept: String = line.chars().take(max_chars_per_line).collect();
                format!(
                    "{kept}…（這一行還有 {} 字沒印）",
                    chars - max_chars_per_line
                )
            }
        })
        .collect();
    (lines, total)
}

// ───────────────────────── 畫面送回來的觀測 ─────────────────────────

/// 畫面量到的一則觀測。
///
/// **中文標籤不在這裡。** 每一格叫什麼、算不算「照你要的」，全由
/// [`Notebook::items`] 這一邊決定；畫面只送得出數字、旗標和一個 `lineId`。
/// 這樣「機制那一節沒有畫面上的字」就不是一條要人記得的紀律，是型別——
/// 唯一的自由文字欄位是 [`Note::Answered`] 的句子，而那一節在線的下面。
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Note {
    /// 這一輪開機了。自檢的涵蓋範圍從這一刻算起，所以**只送一次**。
    Started { at: Millis },
    /// 現在這個角色手上有幾句閒話。開機送一次，換角色再送一次。
    ///
    /// 和 [`Note::Started`] 分開的理由：換角色不代表這一輪重新開始，而把兩件
    /// 事塞進同一則，「涵蓋範圍」就會在他換一次角色的時候悄悄往前跳。
    Persona {
        at: Millis,
        taps: u32,
        giggles: u32,
        beats: u32,
    },
    /// 上面那一槓的狀態。開機時也送一次，這樣「平常是收起來的」才有證據。
    Bar {
        at: Millis,
        open: bool,
        /// 那一槓現在的 `visibility` 是不是 `hidden`。
        dragbar_hidden: bool,
    },
    /// 一顆答案氣泡量到的幾何。**單位是 CSS 像素。**
    Bubble {
        at: Millis,
        bubble_h: f64,
        bubble_bottom: f64,
        content_h: f64,
        /// 捲動容器看得見的高度。
        client_h: f64,
        /// `scrollTop = 99999` 之後讀回來的值。0 = 沒有東西被捲走。
        ///
        /// 不看 `scrollHeight - clientHeight`：那兩個數字在「內容捲得動」和
        /// 「內容直接溢出去」兩種情況下**一模一樣**，量不出差別。
        can_scroll_to: f64,
        /// 捲軸實際佔幾像素。headless Chromium 量到 0，WebView2 不一定。
        scrollbar_px: f64,
        window_h: f64,
    },
    /// 戳了一下。
    Poke {
        at: Millis,
        /// 抖動的 class 真的掛上去了嗎。
        moved: bool,
        /// 她講了哪一句。一句都沒講就是 `None`。
        clip: Option<String>,
        /// 那一句**真的出聲了**嗎。
        ///
        /// 和「有沒有講話」分開：語音關著的時候她照樣講話，只是沒有聲音。
        /// 合成一格的話，一台把語音關掉的機器讀起來會像壞了。
        voiced: bool,
    },
    /// 沒事笑了一下。
    Giggle {
        at: Millis,
        clip: Option<String>,
        voiced: bool,
    },
    /// 時候到了，可是條件不成立，所以這一輪沒笑。
    ///
    /// 沒有這一則，⑥ 印「笑了 0 次」只讀得出一種意思——這功能沒做——而最
    /// 常見的其實是另一種：計時器響了好幾次，每一次他都正在打字。兩件事在
    /// 報告上長得一樣，可是一個要他測久一點，另一個要我改東西。
    GiggleSkipped {
        at: Millis,
        /// 最先擋下來的那一條的代號。過 [`Word`] 那一關，所以帶不動螢幕上的字。
        why: String,
    },
    /// 問了一題，可是沒有答案落地。
    ///
    /// 沒有這一則，④⑤ 兩格在「問了三題、三題都出錯」的時候印的是「這一輪
    /// 沒發生過」——和「他根本沒去問」一模一樣，而那兩件事要他做的事相反。
    ///
    /// 記在畫面那半唯一的那個 `finally` 裡，所以 `answered + ask_failed`
    /// 等於他真的送出去的題數；以後多一條出口而沒人設代號，這裡會收到
    /// `unknown`，那句話指名的是我。
    AskFailed {
        at: Millis,
        /// 他打了幾個字。**問題原文不送。**
        question_chars: u32,
        /// 為什麼沒答成的代號。過 [`Word`] 那一關，帶不動螢幕上的字。
        why: String,
    },
    /// 答完一題。
    Answered {
        at: Millis,
        /// 他打了幾個字。**問題原文不送**，那是他的話。
        question_chars: u32,
        /// 從按下去到畫面上出現。
        took_ms: i64,
        /// 她講的句子。這是唯一一個自由文字欄位，只會出現在線的下面。
        sentences: Vec<String>,
        sources: Vec<String>,
    },
}

impl Note {
    /// 這一則是什麼時候的事。用來算「自檢真的看得到哪一段」。
    fn at(&self) -> Millis {
        match self {
            Note::Started { at }
            | Note::Persona { at, .. }
            | Note::Bar { at, .. }
            | Note::Bubble { at, .. }
            | Note::Poke { at, .. }
            | Note::Giggle { at, .. }
            | Note::GiggleSkipped { at, .. }
            | Note::AskFailed { at, .. }
            | Note::Answered { at, .. } => *at,
        }
    }
}

/// 留最近幾則。上限存在的理由是這本簿子活在記憶體裡，而她可以開一整天。
const NOTES_KEPT: usize = 400;
/// 線下面留她最後幾段答案。
const ANSWERS_KEPT: usize = 3;
/// 一題超過幾毫秒就算慢。Ted 列的第 4 項問的就是這個數字。
const SLOW_ANSWER_MS: i64 = 4_000;

/// 這一輪畫面說過的話。
///
/// **只在記憶體裡。** 資料目錄裡每多一個檔案，`forget`／`export`／`prune`
/// 三條刪除路就各要接一次，而且它自己會變成一個新的隱私面。代價是重開機
/// 之後這本簿子是空的——報告會把那句話印出來，不會假裝它涵蓋更久。
#[derive(Debug, Clone, Default)]
pub struct Notebook {
    notes: Vec<Note>,
    /// 這一輪什麼時候開的。
    ///
    /// **刻意不放在 `notes` 裡。** 那條清單只留得下最後 [`NOTES_KEPT`] 則，而
    /// 開機那一則永遠是第一則——他多戳幾百下就會把它擠掉，於是整節自檢變成
    /// 「沒量」，而簿子其實是滿的。一份為了分辨「量到 0」和「沒量到」而存在的
    /// 報告，最不該犯的就是這一個。
    started_at: Option<Millis>,
    /// 擠掉了幾則。涵蓋範圍要跟著縮，不然「只涵蓋 X 之後」那句話會說謊。
    dropped: usize,
}

impl Notebook {
    pub fn new() -> Notebook {
        Notebook::default()
    }

    pub fn note(&mut self, note: Note) {
        // 頁面重新載入會再送一次，以最後一次為準。
        //
        // **不清空** `notes`：清空要靠畫面先送 `started` 再送別的，而那個順序
        // 是另一層的事（`applyPersona` 就可能先到）。少了幾則觀測不會有人發現，
        // 而這本簿子存在的理由正好是「說得出剛剛發生什麼」。
        if let Note::Started { at } = note {
            self.started_at = Some(at);
        }
        self.notes.push(note);
        if self.notes.len() > NOTES_KEPT {
            let drop = self.notes.len() - NOTES_KEPT;
            self.notes.drain(..drop);
            self.dropped += drop;
        }
    }

    pub fn len(&self) -> usize {
        self.notes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.notes.is_empty()
    }

    /// 這一輪什麼時候開的。畫面還沒說過話就是 `None`。
    pub fn started_at(&self) -> Option<Millis> {
        self.started_at
    }

    /// 自檢**真正**看得到的起點。
    ///
    /// 就是最舊的那一則觀測。通常等於開機那一刻，但擠掉過之後就晚了——更早的
    /// 事已經不在簿子裡，還說「涵蓋開機之後」等於把沒看到的那一段算成「沒發生」。
    pub fn covers_from(&self) -> Option<Millis> {
        self.notes.first().map(Note::at).or(self.started_at)
    }

    /// 擠掉了幾則觀測。
    pub fn dropped(&self) -> usize {
        self.dropped
    }

    /// 把畫面那兩節補進快照。
    ///
    /// 命令列補不了，於是那兩節留在 [`Absent::NotHere`]——而 `render` 會把
    /// 「這不是『沒問題』，是『沒量』」印出來。
    pub fn fill(&self, snapshot: &mut Snapshot, at: Millis) {
        snapshot.run_started_at = self.started_at();
        snapshot.answers = Ok(self.answers());
        snapshot.self_check = match self.started_at() {
            None => Err(Absent::NotHere),
            Some(run_started_at) => Ok(SelfCheck {
                at,
                run_started_at,
                covers_from: self.covers_from().unwrap_or(run_started_at),
                dropped: self.dropped,
                items: self.items(),
            }),
        };
    }

    /// 最後幾段答案，舊的在前。
    pub fn answers(&self) -> Vec<Answer> {
        let mut out: Vec<Answer> = self
            .notes
            .iter()
            .rev()
            .filter_map(|note| match note {
                Note::Answered {
                    at,
                    question_chars,
                    took_ms,
                    sentences,
                    sources,
                } => Some(Answer {
                    at: *at,
                    question_chars: *question_chars as usize,
                    sentences: sentences.clone(),
                    sources: sources.clone(),
                    took_ms: Some(*took_ms),
                }),
                _ => None,
            })
            .take(ANSWERS_KEPT)
            .collect();
        out.reverse();
        out
    }

    fn pack(&self) -> (u32, u32, u32) {
        self.notes
            .iter()
            .rev()
            .find_map(|note| match note {
                Note::Persona {
                    taps,
                    giggles,
                    beats,
                    ..
                } => Some((*taps, *giggles, *beats)),
                _ => None,
            })
            .unwrap_or((0, 0, 0))
    }

    /// 一段 clip 的代號，過 [`Word`] 那一關。過不了就當成沒有——畫面送不出
    /// 螢幕上的字，不是因為它不想，是因為這裡收不下。
    fn clips(&self, giggle: bool) -> Vec<(&str, bool)> {
        self.notes
            .iter()
            .filter_map(|note| match (note, giggle) {
                (Note::Poke { clip, voiced, .. }, false)
                | (Note::Giggle { clip, voiced, .. }, true) => {
                    clip.as_deref().map(|clip| (clip, *voiced))
                }
                _ => None,
            })
            .filter(|(raw, _)| Word::new(raw).is_some())
            .collect()
    }

    /// 時候到了卻沒笑的那幾則，各是哪一條擋的。
    ///
    /// **這裡刻意不過 [`Word`]。** 數量是個整數，它夾帶不了任何東西；把不合
    /// 格的代號一起丟掉，只會讓「跳過幾次」少報，而那個數字正是這一格要回答
    /// 的問題。收不收得下自由文字是**指名**那一關的事，見底下的 `Word::new`。
    fn giggle_skips(&self) -> Vec<&str> {
        self.notes
            .iter()
            .filter_map(|note| match note {
                Note::GiggleSkipped { why, .. } => Some(why.as_str()),
                _ => None,
            })
            .collect()
    }

    /// 沒答成的那幾題，各是為什麼。
    ///
    /// 和 [`Notebook::giggle_skips`] 同一個理由不過 [`Word`]：數量夾帶不了
    /// 東西，濾掉只會讓「幾題沒答成」少報。指名之前才過那一關。
    fn ask_failures(&self) -> Vec<&str> {
        self.notes
            .iter()
            .filter_map(|note| match note {
                Note::AskFailed { why, .. } => Some(why.as_str()),
                _ => None,
            })
            .collect()
    }

    /// 出現最多次的那一個。平手就取先出現的。
    fn commonest<'a>(values: &[&'a str]) -> Option<&'a str> {
        let mut best: Option<(&str, usize)> = None;
        for value in values {
            let count = values.iter().filter(|other| *other == value).count();
            if best.is_none_or(|(_, seen)| count > seen) {
                best = Some((value, count));
            }
        }
        best.map(|(value, _)| value)
    }

    /// 出現最多次的那一個，而且它得是個 [`Word`]。
    ///
    /// **`Word` 那一關擋在指名這一步，不擋在數的那一步。** 一個數字夾帶不了
    /// 螢幕上的字，一個名字會；濾在數的那一步只會讓次數少報。
    fn named_commonest(values: &[&str]) -> Option<Word> {
        let named: Vec<&str> = values
            .iter()
            .copied()
            .filter(|value| Word::new(value).is_some())
            .collect();
        Self::commonest(&named).and_then(Word::new)
    }

    fn distinct(values: &[(&str, bool)]) -> usize {
        let mut seen: Vec<&str> = Vec::new();
        for (value, _) in values {
            if !seen.contains(value) {
                seen.push(value);
            }
        }
        seen.len()
    }

    /// Ted 列的那七件事，一件一格。
    ///
    /// 標籤與判準都寫在這裡，不是畫面送過來的。
    fn items(&self) -> Vec<Item> {
        let (taps, giggle_lines, _beats) = self.pack();
        let mut items = Vec::new();

        // ① 上面那一槓
        let bars: Vec<(bool, bool)> = self
            .notes
            .iter()
            .filter_map(|note| match note {
                Note::Bar {
                    open,
                    dragbar_hidden,
                    ..
                } => Some((*open, *dragbar_hidden)),
                _ => None,
            })
            .collect();
        let closed_seen = bars.iter().filter(|(open, _)| !open).count();
        let closed_but_showing = bars
            .iter()
            .filter(|(open, hidden)| !open && !hidden)
            .count();
        items.push(Item {
            number: 1,
            asked: "上面那一槓平常不在，按一個鍵才跑出來".into(),
            measured: vec![
                m("翻過幾次", MeasureValue::Int(bars.len() as i64)),
                m(
                    "現在開著嗎",
                    MeasureValue::Flag(bars.last().map(|(open, _)| *open).unwrap_or(false)),
                ),
                m(
                    "收起來的時候量過幾次",
                    MeasureValue::Int(closed_seen as i64),
                ),
                m(
                    "其中那一槓還看得見的",
                    MeasureValue::Int(closed_but_showing as i64),
                ),
            ],
            verdict: if bars.is_empty() {
                /* 開機那一段一定會送一則（`noteTheChromeBar()` 就排在 `started`
                 * 後面兩行），所以「這一輪開過機、卻一則都沒有」＝那支量測沒
                 * 跑起來，不是他還沒去翻那一槓。
                 *
                 * 但簿子擠掉過就不能這樣講：開機那一則存在欄位裡不會被擠，那
                 * 一槓的卻會。分不出來的時候講回「沒量到」，不要指控。 */
                if self.started_at.is_some() && self.dropped == 0 {
                    Verdict::NotMeasured
                } else {
                    Verdict::NotSeen
                }
            } else if closed_but_showing > 0 {
                Verdict::Off
            } else {
                Verdict::AsAsked
            },
        });

        // ② 戳一下會動也會講話
        let pokes: Vec<(bool, bool, bool)> = self
            .notes
            .iter()
            .filter_map(|note| match note {
                Note::Poke {
                    moved,
                    clip,
                    voiced,
                    ..
                } => Some((*moved, clip.is_some(), *voiced)),
                _ => None,
            })
            .collect();
        let moved = pokes.iter().filter(|(moved, ..)| *moved).count();
        let spoke = pokes.iter().filter(|(_, spoke, _)| *spoke).count();
        let voiced = pokes.iter().filter(|(.., voiced)| *voiced).count();
        items.push(Item {
            number: 2,
            asked: "戳一下就會動一下，然後講個話".into(),
            measured: vec![
                m("戳了幾下", MeasureValue::Int(pokes.len() as i64)),
                m("有動的", MeasureValue::Int(moved as i64)),
                m("有講話的", MeasureValue::Int(spoke as i64)),
                m("真的出聲的", MeasureValue::Int(voiced as i64)),
            ],
            // 出聲數 0 不算壞：語音是可以關的，而報告上面那一行已經講清楚了。
            // 「動了沒」和「講了沒」才是他要的那兩件事。
            verdict: if pokes.is_empty() {
                Verdict::NotSeen
            } else if moved < pokes.len() || spoke == 0 {
                Verdict::Off
            } else {
                Verdict::AsAsked
            },
        });

        // ③ 不要只有兩句
        let poke_clips = self.clips(false);
        let poke_distinct = Self::distinct(&poke_clips);
        items.push(Item {
            number: 3,
            asked: "按著要隨機講一些，不要一直講同兩句".into(),
            measured: vec![
                m("用過幾句不同的", MeasureValue::Int(poke_distinct as i64)),
                m("這個角色手上有幾句", MeasureValue::Int(taps as i64)),
            ],
            verdict: if poke_clips.len() < 2 {
                Verdict::NotSeen
            } else if poke_distinct < 2 {
                Verdict::Off
            } else {
                Verdict::AsAsked
            },
        });

        // ④ 一題幾秒
        let took: Vec<i64> = self
            .notes
            .iter()
            .filter_map(|note| match note {
                Note::Answered { took_ms, .. } => Some(*took_ms),
                _ => None,
            })
            .collect();
        let flops = self.ask_failures();
        let slowest = took.iter().copied().max();
        let middle = median(took.clone());
        let mut answer_measured = vec![
            m("答了幾題", MeasureValue::Int(took.len() as i64)),
            m("中位數毫秒", MeasureValue::Int(middle.unwrap_or_default())),
            m("最慢那一題", MeasureValue::Int(slowest.unwrap_or_default())),
            /* 「沒答成幾題」無條件出現，0 也要印。
             *
             * 它是 ④⑤ 兩格分得出「他還沒去問」和「他問了、一題都沒答成」的
             * 唯一根據；只在大於零的時候才印的話，讀報告的人看到一片空白仍然
             * 分不出那兩件事——而那正是這一格存在的理由。 */
            m("問了沒答成的", MeasureValue::Int(flops.len() as i64)),
        ];
        if let Some(top) = Self::named_commonest(&flops) {
            answer_measured.push(m("最常沒答成的原因", MeasureValue::AskWhy(top)));
        }
        items.push(Item {
            number: 4,
            asked: "問一題不該超過四秒".into(),
            measured: answer_measured,
            verdict: match middle {
                None if flops.is_empty() => Verdict::NotSeen,
                /* 他問了，一題都沒答成。最常見的那一種是他選的那支 CLI 還沒
                 * 設好——印成「這一輪沒發生過」的話，報告等於叫他去做他剛剛
                 * 才做過的事。 */
                None => Verdict::NeverLanded,
                Some(ms) if ms > SLOW_ANSWER_MS => Verdict::Off,
                Some(_) => Verdict::AsAsked,
            },
        });

        // ⑤ 氣泡框
        let bubble = self.notes.iter().rev().find_map(|note| match note {
            Note::Bubble {
                bubble_h,
                bubble_bottom,
                content_h,
                client_h,
                can_scroll_to,
                scrollbar_px,
                window_h,
                ..
            } => Some((
                *bubble_h,
                *bubble_bottom,
                *content_h,
                *client_h,
                *can_scroll_to,
                *scrollbar_px,
                *window_h,
            )),
            _ => None,
        });
        let answered = self.answers().len();
        let (measured, verdict) = match bubble {
            // 一題都沒問過，這一格本來就沒東西可量。
            None if answered == 0 && flops.is_empty() => (Vec::new(), Verdict::NotSeen),
            /* 問了，可是一題都沒答成。沒有答案就沒有氣泡——所以這一格既不是
             * 「他沒去做」也不是「量不出來」，是「還輪不到量」。為什麼沒答成
             * 寫在 ④ 那兩格，這裡只給他數字，不重複那本字典。 */
            None if answered == 0 => (
                vec![m("問了沒答成的", MeasureValue::Int(flops.len() as i64))],
                Verdict::NeverLanded,
            ),
            /* 問過題卻一次都沒量到，那不是「沒發生」：氣泡跟著答案出現，而答
             * 案就在這本簿子裡。兩種成因，兩種都要他去看一眼：
             *
             * 一是畫面上根本沒有 `.answer-bubble`——那就是他第五件事本人壞了。
             * 二是量測自己沒跑起來：`noteTheBubble` 排在 `requestAnimationFrame`
             * 上，而 `observation()` 會把它丟出來的例外吞掉（不吞的話，會連答
             * 案落地那一聲和整段朗讀一起帶走）。吞掉的代價就是這一格靜靜地空
             * 著。所以那句話只講「量不出來」，不替它挑成因。
             *
             * 滾掉不會造成這一格說謊：簿子從最舊的那一頭擠，而氣泡那一則永遠
             * 排在它那一題的後面——答案還在，它就還在。 */
            None => (
                vec![m("答過幾題", MeasureValue::Int(answered as i64))],
                Verdict::NotMeasured,
            ),
            Some((h, bottom, content, client, scroll, bar, window)) => (
                vec![
                    m("氣泡高", MeasureValue::Num(h)),
                    m("氣泡底邊", MeasureValue::Num(bottom)),
                    m("視窗高", MeasureValue::Num(window)),
                    m("內容高", MeasureValue::Num(content)),
                    m("看得見的高", MeasureValue::Num(client)),
                    m("捲得到（0 就是沒被切掉）", MeasureValue::Num(scroll)),
                    m("捲軸佔幾像素", MeasureValue::Num(bar)),
                ],
                // 捲得到 > 0 就是有東西被切在框外面，而那正是他說「不好看」
                // 的那個畫面。底邊掉出視窗也一樣。
                if scroll > 0.5 || bottom > window + 0.5 {
                    Verdict::Off
                } else {
                    Verdict::AsAsked
                },
            ),
        };
        items.push(Item {
            number: 5,
            asked: "回答的介面要像漫畫的氣泡框".into(),
            measured,
            verdict,
        });

        // ⑥ 沒事也笑幾下
        let giggle_clips = self.clips(true);
        let skips = self.giggle_skips();
        let mut giggle_measured = vec![
            m("笑了幾次", MeasureValue::Int(giggle_clips.len() as i64)),
            m(
                "幾句不同的",
                MeasureValue::Int(Self::distinct(&giggle_clips) as i64),
            ),
            m(
                "真的出聲的",
                MeasureValue::Int(giggle_clips.iter().filter(|(_, voiced)| *voiced).count() as i64),
            ),
            m("這個角色手上有幾句", MeasureValue::Int(giggle_lines as i64)),
            /* 這一格是「笑了 0 次」的解釋。她每兩到五分鐘試一次，試一次就在這
             * 兩格之一留下一筆，所以兩格都是 0 的意思是「這一輪還沒試過」——他
             * 測太短，不是她壞了。 */
            m("時候到了卻跳過", MeasureValue::Int(skips.len() as i64)),
        ];
        if let Some(top) = Self::named_commonest(&skips) {
            giggle_measured.push(m("最常擋下來的", MeasureValue::Blocker(top)));
        }
        items.push(Item {
            number: 6,
            asked: "沒事也可以呵呵嘻嘻笑幾下".into(),
            measured: giggle_measured,
            verdict: if giggle_clips.is_empty() && skips.is_empty() {
                Verdict::NotSeen
            } else if giggle_clips.is_empty() {
                /* 計時器響過，每一次都被擋掉。這一格的摘要不可以說「沒發生
                 * 過」——底下兩格明明寫著它試了幾次、最常被什麼擋的。摘要和
                 * 自己的明細打架的時候，讀報告的人信的是摘要。 */
                Verdict::NeverLanded
            } else {
                Verdict::AsAsked
            },
        });

        // ⑦ 講話像不像朋友
        items.push(Item {
            number: 7,
            asked: "回答要像朋友講話，不要像機器人念出處".into(),
            measured: vec![m(
                "線下面有幾段",
                MeasureValue::Int(self.answers().len() as i64),
            )],
            verdict: Verdict::CantJudge,
        });

        items
    }
}

fn m(label: &str, value: MeasureValue) -> Measure {
    Measure {
        label: label.to_string(),
        value,
    }
}

// ───────────────────────── 從磁碟上讀起來 ─────────────────────────

/// 讀幾趟外送。40 趟大約是二十題——夠看出「哪一趟慢」的形狀，又不會讓報告
/// 長到沒有人讀。
pub const LEGS: usize = 40;
/// 「沒問」那幾列只折成計數，多讀一點不佔版面。
pub const SKIPS: usize = 400;
/// log 尾巴：每個檔最多幾行、每行最多幾字。
pub const LOG_LINES: usize = 120;
pub const LOG_LINE_CHARS: usize = 400;
/// 只從檔尾讀這麼多位元組。一個跑了三個禮拜的 `record.log` 可以到幾百 MB，
/// 而我們要的只有最後那幾行——整個讀進記憶體只是為了丟掉。
pub const LOG_TAIL_BYTES: u64 = 512 * 1024;
/// 會去翻的那幾個 log。
pub const LOGS: [&str; 4] = ["desktop.log", "desktop.log.1", "record.log", "record.log.1"];

/// 報告的檔名。帶時間，跑第二次不會蓋掉第一次。
///
/// 命令列和桌面版那顆鈕用的是同一支：兩邊各取各的名字，他就得在兩種檔名
/// 之間猜哪一份是新的。
pub fn file_name(now: Millis) -> String {
    use chrono::{Local, TimeZone};
    let when = Local
        .timestamp_millis_opt(now)
        .single()
        .map(|dt| dt.format("%Y%m%d-%H%M%S").to_string())
        .unwrap_or_else(|| now.to_string());
    format!("sister-diagnose-{when}.txt")
}

/// 把磁碟上該讀的都讀起來。
///
/// **命令列和桌面版那顆鈕走的是同一支。** 「哪些檔案、哪幾張表、取多長」是一
/// 條規則，這個 repo 已經在「一條規則寫兩個地方」上栽過很多次；兩份會在某一
/// 版分家，而分家的症狀是兩邊印出來的報告講不同的話。
///
/// 畫面那兩節（自檢、答案原文）這裡填不了——只有畫面自己量得到。桌面版拿到
/// 這份快照之後用 [`Notebook::fill`] 補上去；命令列補不了，於是那兩節印
/// [`Absent::NotHere`]。
pub fn collect(data_dir: &Path, source: &str, app_version: &str) -> Snapshot {
    let db_path = crate::config::Config::db_path(data_dir);
    // 檔案不在就**不要** `open`：那支會 `create_dir_all` 再建一個空的出來，於是
    // 一份只該讀的診斷變成寫入端，而報告會說「0 幀」而不是「還沒有這個檔案」。
    let db = match why_no_db(data_dir) {
        Absent::NotThere => Err(Absent::NotThere),
        _ => crate::db::Db::open(&db_path).map_err(|_| Absent::QueryFailed),
    };
    collect_from(data_dir, source, app_version, db.as_ref().map_err(|e| *e))
}

/// 資料庫問不出來的時候，是「還沒有」還是「開不起來」。
///
/// 兩個呼叫端都要回答這一題——命令列自己開連線，桌面版沿用她已經開著的那條
/// ——而答錯的後果一樣：一格印「問不出來」而真相是「他還沒錄過」，正好是這份
/// 報告存在的理由所反對的那句話。同一個判準寫兩處而不同步是看不見的，所以只
/// 寫一次。
pub fn why_no_db(data_dir: &Path) -> Absent {
    if crate::config::Config::db_path(data_dir).exists() {
        Absent::QueryFailed
    } else {
        Absent::NotThere
    }
}

/// 同一支，但資料庫連線由呼叫端給。
///
/// 桌面版手上已經有一條開著的連線了。再開第二條不是不行（WAL 讀得動），但
/// 那條路上有一次 migration 檢查，而她可能正在寫——一份**只有在忙的時候才
/// 查不出來**的診斷報告，剛好在最需要它的那一刻失效。
pub fn collect_from(
    data_dir: &Path,
    source: &str,
    app_version: &str,
    db: std::result::Result<&crate::db::Db, Absent>,
) -> Snapshot {
    let mut scrub = Scrubber::new();
    scrub.hide_paths(data_dir, home_dir().as_deref(), user_name().as_deref());

    Snapshot {
        at: crate::now_ms(),
        app_version: app_version.to_string(),
        platform: std::env::consts::OS.to_string(),
        source: source.to_string(),
        run_started_at: None,
        data_dir_shown: where_it_lives(data_dir),
        consent: ConsentLines::load(data_dir),
        doctor: crate::capabilities::read(data_dir).ok_or(Absent::NotThere),
        db: ask(db, |db| db.stats().ok()),
        legs: ask(db, |db| {
            db.list_brain_outbound(LEGS)
                .ok()
                .map(|rows| rows.iter().map(Leg::from_row).collect())
        }),
        skips: ask(db, |db| {
            db.list_brain_skip(SKIPS)
                .ok()
                .map(|rows| SkipCount::fold(&rows))
        }),
        self_check: Err(Absent::NotHere),
        answers: Err(Absent::NotHere),
        logs: LOGS
            .iter()
            .map(|name| log_tail(data_dir, name, &scrub))
            .collect(),
    }
}

/// 資料庫問不出來的時候，回答的是「為什麼問不出來」，不是 `None`。
///
/// 一格空白讀起來像「沒問題」，而這份報告存在的理由正是要分得出「量到 0」
/// 和「沒量到」。
fn ask<T>(
    db: std::result::Result<&crate::db::Db, Absent>,
    f: impl FnOnce(&crate::db::Db) -> Option<T>,
) -> Got<T> {
    match db {
        Err(absent) => Err(absent),
        Ok(db) => f(db).ok_or(Absent::QueryFailed),
    }
}

/// 講位置，不講路徑。「在不在預設的地方」才是診斷要的資訊；完整路徑只會把
/// 使用者名稱帶出去。
fn where_it_lives(data_dir: &Path) -> String {
    match crate::config::Config::default_data_dir() {
        Some(default) if default == data_dir => "預設位置".to_string(),
        Some(_) => "自訂位置".to_string(),
        None => "問不出預設位置在哪".to_string(),
    }
}

pub fn home_dir() -> Option<std::path::PathBuf> {
    for key in ["USERPROFILE", "HOME"] {
        if let Some(value) = std::env::var_os(key).filter(|v| !v.is_empty()) {
            return Some(std::path::PathBuf::from(value));
        }
    }
    None
}

pub fn user_name() -> Option<String> {
    for key in ["USERNAME", "USER"] {
        let Ok(value) = std::env::var(key) else {
            continue;
        };
        let value = value.trim().to_string();
        if !value.is_empty() {
            return Some(value);
        }
    }
    None
}

fn log_tail(data_dir: &Path, name: &str, scrub: &Scrubber) -> LogTail {
    let text = match read_tail(&data_dir.join(name), LOG_TAIL_BYTES) {
        Ok(text) => Ok(scrub.apply(&text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(Absent::NotThere),
        Err(error) => Err(Absent::Unreadable(error.kind())),
    };
    match text {
        Err(absent) => LogTail {
            name: name.to_string(),
            lines: Err(absent),
            total_lines: 0,
        },
        Ok(text) => {
            let (lines, total) = tail_lines(&text, LOG_LINES, LOG_LINE_CHARS);
            LogTail {
                name: name.to_string(),
                lines: Ok(lines),
                total_lines: total,
            }
        }
    }
}

/// 檔案最後 `bytes` 個位元組，掐頭去掉那半行。
///
/// 從中間切開一定會切在某個字元中間，`from_utf8_lossy` 會把它變成一個 `�`。
/// 丟掉第一個換行之前的東西就沒有這個問題——那半行本來也讀不懂。
fn read_tail(path: &Path, bytes: u64) -> std::io::Result<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    let whole = len <= bytes;
    if !whole {
        file.seek(SeekFrom::Start(len - bytes))?;
    }
    let mut buf = Vec::with_capacity(bytes.min(len) as usize);
    file.read_to_end(&mut buf)?;
    let text = String::from_utf8_lossy(&buf).into_owned();
    if whole {
        return Ok(text);
    }
    Ok(match text.find('\n') {
        Some(at) => text[at + 1..].to_string(),
        None => text,
    })
}

// ───────────────────────── 印出來 ─────────────────────────

/// 終端機裡一個中日韓字元佔兩格。
///
/// `{:<10}` 數的是 char，所以「本機記錄」（4 char／8 格）和「Azure 朗讀」
/// （8 char／12 格）用同一個 `{:<10}` 排出來會歪掉——而這份報告有好幾張表，
/// 歪掉的表比沒有表更難讀。
fn wide(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x115F | 0x2E80..=0x303E | 0x3041..=0x33FF | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF | 0xA000..=0xA4CF | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF
        | 0xFE30..=0xFE6F | 0xFF00..=0xFF60 | 0xFFE0..=0xFFE6
        | 0x1F300..=0x1F64F | 0x20000..=0x3FFFD)
}

fn cells(text: &str) -> usize {
    text.chars().map(|c| if wide(c) { 2 } else { 1 }).sum()
}

/// 靠左，補到 `to` 格寬。已經超過就不截——截掉的那截才是他要看的東西。
fn pad(text: &str, to: usize) -> String {
    let mut out = text.to_string();
    for _ in cells(text)..to {
        out.push(' ');
    }
    out
}

/// 靠右。數字欄用。
fn rpad(text: &str, to: usize) -> String {
    let mut out = String::new();
    for _ in cells(text)..to {
        out.push(' ');
    }
    out.push_str(text);
    out
}

/// ② 那張表的欄寬。表頭和每一列都從這裡算，不各寫一份——兩份遲早分家，
/// 而分家的症狀就是一張對不齊的表。
const COL_TIME: usize = 20;
const COL_ROLE: usize = 16;
const COL_OUTCOME: usize = 14;
const COL_MS: usize = 9;
const COL_CHARS: usize = 11;

fn leg_line(time: &str, role: &str, outcome: &str, ms: &str, chars: &str) -> String {
    format!(
        "  {}{}{}{}{}",
        pad(time, COL_TIME),
        pad(role, COL_ROLE),
        pad(outcome, COL_OUTCOME),
        rpad(ms, COL_MS),
        rpad(chars, COL_CHARS),
    )
}

fn group(v: i64) -> String {
    let neg = v < 0;
    let digits = v.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if neg { format!("-{out}") } else { out }
}

fn bytes(v: i64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = v as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{v} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// 中位數。空的回 `None`——「沒有樣本」和「0 毫秒」是兩件事。
fn median(mut values: Vec<i64>) -> Option<i64> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    Some(values[values.len() / 2])
}

fn sheet_label(key: &str) -> &str {
    match key {
        "local-recording" => "本機記錄",
        "cloud-reading" => "上雲解讀",
        "frame-storage" => "畫面暫存",
        "azure-tts" => "Azure 朗讀",
        other => other,
    }
}

fn cap(state: capabilities::CapabilityState) -> &'static str {
    match state {
        capabilities::CapabilityState::Available => "可用",
        capabilities::CapabilityState::Unavailable => "做不到",
        capabilities::CapabilityState::Unknown => "沒量到",
    }
}

/// 整份報告。
pub fn render(snapshot: &Snapshot) -> String {
    let head = render_head(snapshot);
    let tail = render_tail(snapshot);
    let head_chars = head.chars().count();
    let tail_chars = tail.chars().count();

    /* ④ 自己也在線的上面，所以「線以上幾個字」這個數字**要把印它的那幾行算
     * 進去**。
     *
     * 第一版只算了 ①～③，於是報告說「線以上 658 個字」而線以上真的有 900。
     * 在一份靠數字取信於人的東西裡，標題叫「這份報告帶走了什麼」的那一節少報
     * 了四分之一——那比不印還糟。
     *
     * 這個數字會改變自己的長度（999 變 1,003 就多兩格），所以收斂到不動點再
     * 印。它單調往上又有上界，跑不了幾輪；跑不出來就維持最後那一版，不會卡住。 */
    let mut above = head_chars;
    let mut ledger = render_ledger(above, tail_chars, snapshot);
    for _ in 0..8 {
        // `+ 1` 是分隔線前面那個換行——它也在線的上面。
        let settled = head_chars + ledger.chars().count() + 1;
        if settled == above {
            break;
        }
        above = settled;
        ledger = render_ledger(above, tail_chars, snapshot);
    }

    let mut out = head;
    out.push_str(&ledger);
    out.push('\n');
    out.push_str(DIVIDER);
    out.push('\n');
    out.push_str(&tail);
    out
}

fn render_head(s: &Snapshot) -> String {
    let mut o = String::new();
    o.push_str("AI-Sister 診斷報告\n");
    o.push_str(&format!(
        "產生時間  {}\n版本      {}（報告格式 v{}，{}）\n平台      {}\n資料夾    {}\n",
        stamp(s.at),
        s.app_version,
        REPORT_VERSION,
        s.source,
        s.platform,
        s.data_dir_shown,
    ));
    match s.run_started_at {
        Some(at) => o.push_str(&format!(
            "這一輪    {} 開的（自檢那一節只看得到這之後的事）\n",
            stamp(at)
        )),
        None => o.push_str("這一輪    不知道什麼時候開的（命令列跑的，看不到桌面版那一輪）\n"),
    }

    o.push_str("\n① 這台機器\n");
    o.push_str("  同意書\n");
    for (key, signed, effective) in &s.consent.sheets {
        let when = signed.map_or_else(|| "沒簽".to_string(), stamp);
        let note = match (signed.is_some(), effective) {
            (true, true) => "生效中",
            // 簽過但不生效，只有一個原因：條文改版了，那份簽名涵蓋不了新條文。
            (true, false) => "簽過但不生效（條文改版了，要重簽）",
            (false, _) => "—",
        };
        o.push_str(&format!(
            "    {}{}{}\n",
            pad(sheet_label(key), 14),
            pad(&when, 24),
            note
        ));
    }
    o.push_str(&format!(
        "    條文版本  檔案 {}、上雲 {}、Azure {}\n",
        s.consent.file_version, s.consent.cloud_terms_version, s.consent.azure_terms_version,
    ));

    o.push_str("  上一場錄製量到的能力\n");
    match &s.doctor {
        Err(absent) => o.push_str(&format!("    {}\n", absent.say())),
        Ok(report) => {
            o.push_str(&format!(
                "    量的時候    {}\n    位址列      {}\n    輸入 hook   {}\n",
                stamp(report.at),
                cap(report.url),
                cap(report.input_hook),
            ));
            o.push_str(&format!(
                "    瀏覽器拍數  {}，其中真的讀到網址 {}\n",
                group(report.browser_ticks as i64),
                group(report.url_reads as i64),
            ));
            if report.url_capture.gave_up {
                o.push_str("    ⚠ 位址列讀取中途永久放棄了，那之後 excluded_urls 一條都沒生效\n");
            }
            if report.url_capture.password_check_broken {
                o.push_str("    ⚠ 問不出焦點在不在密碼欄\n");
            }
        }
    }

    o.push_str("  資料庫\n");
    match &s.db {
        Err(absent) => o.push_str(&format!("    {}\n", absent.say())),
        Ok(st) => {
            o.push_str(&format!(
                "    {} 幀（其中 {} 張圖真的躺在硬碟上）、{} 段文字、{} 個事實、{} 題你問過的話\n",
                group(st.frames),
                group(st.frames_with_image),
                group(st.chunks),
                group(st.facts),
                group(st.queries),
            ));
            o.push_str(&format!(
                "    資料庫 {}、畫面檔 {}\n",
                bytes(st.db_bytes),
                bytes(st.image_bytes),
            ));
            match (st.first_ts, st.last_ts) {
                (Some(first), Some(last)) => o.push_str(&format!(
                    "    最早 {}、最晚 {}\n",
                    stamp(first),
                    stamp(last)
                )),
                _ => o.push_str("    還沒有記到任何東西\n"),
            }
        }
    }

    o.push_str("\n② 她問 CLI 的每一趟（磁碟上本來就有，只是沒有人讀）\n");
    match &s.legs {
        Err(absent) => o.push_str(&format!("  {}\n", absent.say())),
        Ok(legs) if legs.is_empty() => {
            o.push_str("  一趟都沒有。她從開始到現在沒有把任何東西交出去過。\n")
        }
        Ok(legs) => {
            o.push_str(&leg_line("時間", "角色", "結局", "毫秒", "送出字數"));
            o.push_str("  註\n");
            for leg in legs {
                let error = match &leg.error {
                    None => String::new(),
                    Some(summary) => {
                        format!("{}（{} 字，沒印出來）", summary.kind.say(), summary.chars)
                    }
                };
                o.push_str(&leg_line(
                    &stamp(leg.ts),
                    &leg.role,
                    &leg.outcome,
                    &group(leg.duration_ms),
                    &group(leg.chars_sent),
                ));
                o.push_str(&format!(
                    "  {}{}\n",
                    if leg.truncated { "（截斷）" } else { "" },
                    error,
                ));
            }
            o.push_str(&render_leg_summary(legs));
        }
    }

    o.push_str("\n  沒問的那幾次（brain_skip）\n");
    match &s.skips {
        Err(absent) => o.push_str(&format!("    {}\n", absent.say())),
        Ok(skips) if skips.is_empty() => o.push_str("    沒有。\n"),
        Ok(skips) => {
            for skip in skips {
                o.push_str(&format!("    {}{} 次\n", pad(&skip.reason, 26), skip.times));
            }
        }
    }

    o.push_str("\n③ 自檢：你列的那七件事，這一輪各量到什麼\n");
    match &s.self_check {
        Err(Absent::NotHere) => o.push_str(
            "  命令列量不到畫面。要這一節就從桌面版設定頁按「匯出診斷」。\n\
             \x20 這不是「沒問題」，是「沒量」。\n",
        ),
        Err(absent) => o.push_str(&format!("  {}\n", absent.say())),
        Ok(check) => {
            o.push_str(&format!(
                "  量的時候 {}，這一輪 {} 開的\n",
                stamp(check.at),
                stamp(check.run_started_at),
            ));
            // 沒擠掉就照實說涵蓋整輪；擠掉過就講真正看得到的那一段，並且把
            // 「有一段沒看到」講出來——底下每一格的「0 次」都要照這個讀。
            if check.dropped == 0 {
                o.push_str(&format!(
                    "  底下只涵蓋 {} 之後發生的事\n",
                    stamp(check.run_started_at)
                ));
            } else {
                o.push_str(&format!(
                    "  簿子只留得下最後 {} 則，更早的 {} 則已經滾掉了——\n\
                     \x20 所以底下只涵蓋 {} 之後，不是整輪\n",
                    group(NOTES_KEPT as i64),
                    group(check.dropped as i64),
                    stamp(check.covers_from),
                ));
            }
            /* 欄寬從最長的標籤算，不寫死。
             *
             * 上一版寫死 24，而「捲得到（0 就是沒被切掉）」剛好就是 24 格——
             * `pad` 於是一個空白都補不上，那一行印出來是「…被切掉）0.0」。
             * 一個寫死的寬度會被下一個標籤再撞一次，而撞到的樣子是「值黏在字
             * 上」，不是編譯錯誤。 */
            let widest = check
                .items
                .iter()
                .flat_map(|item| item.measured.iter())
                .map(|measure| cells(&measure.label))
                .max()
                .unwrap_or(0);
            for item in &check.items {
                /* 記號補到兩格寬。`✓` 一格、`－？！` 兩格（全形），不補的
                 * 話同一份表裡的「第 N 項」會左右各差一格——而那一欄正是他
                 * 用來掃的。 */
                o.push_str(&format!(
                    "  {} 第 {} 項　{}　{}\n",
                    pad(item.verdict.mark(), 2),
                    item.number,
                    item.asked,
                    item.verdict.say(),
                ));
                for measure in &item.measured {
                    o.push_str(&format!(
                        "      {}{}\n",
                        pad(&measure.label, widest + 2),
                        measure.value.say()
                    ));
                }
            }
        }
    }
    o
}

/// 一題兩趟：把「哪一趟慢」算出來，不要讓讀的人自己加。
fn render_leg_summary(legs: &[Leg]) -> String {
    let took = |role: &str| -> Vec<i64> {
        legs.iter()
            .filter(|l| l.role == role)
            .map(|l| l.duration_ms)
            .collect()
    };
    let search = took("answer_search");
    let answer = took("answer");
    let mut o = String::new();
    if search.is_empty() && answer.is_empty() {
        return o;
    }
    o.push_str("  ── 答題那兩趟 ──\n");
    if let Some(m) = median(search.clone()) {
        o.push_str(&format!(
            "    先問它要查什麼   {} 趟，中位數 {} 毫秒\n",
            search.len(),
            group(m)
        ));
    }
    if let Some(m) = median(answer.clone()) {
        o.push_str(&format!(
            "    再請它寫成一句   {} 趟，中位數 {} 毫秒\n",
            answer.len(),
            group(m)
        ));
    }
    // 兩個中位數相加不是「一題的中位數」，但它回答的正是那個問題：一題要
    // 等多久、而那些時間花在哪一趟。加起來這件事要講明白，不要讓它看起來
    // 像量到的。
    if let (Some(a), Some(b)) = (median(search), median(answer)) {
        o.push_str(&format!(
            "    兩個中位數加起來 {} 毫秒（不是量到的「一題的中位數」，是兩趟各自的中位數相加）\n",
            group(a + b)
        ));
    }
    o
}

fn render_ledger(head_chars: usize, tail_chars: usize, s: &Snapshot) -> String {
    let mut o = String::new();
    o.push_str("\n④ 這份報告帶走了什麼\n");
    o.push_str(&format!(
        "  這條線以上 {} 個字。裡面沒有一個字是螢幕上的內容，理由不是有人記得要遮，\n",
        group(head_chars as i64)
    ));
    o.push_str("  是上面每一格能放的東西都在型別裡寫死了：時間、次數、結局代號、毫秒、版本。\n");
    o.push_str("  CLI 回來的錯誤只留了類別和字數，錯誤原文一個字都沒有帶。\n");
    let answer_count = s.answers.as_ref().map(|a| a.len()).unwrap_or(0);
    // 找了幾個檔和真的讀到幾個是兩件事。「4 個 log 檔的尾巴」配上四行
    // 「沒有這個檔案」，就是這份報告最不該犯的那種話。
    let read = s.logs.iter().filter(|l| l.lines.is_ok()).count();
    o.push_str(&format!(
        "  這條線以下 {} 個字：她的 {} 段答案原文，加上 {} 個 log 檔（找了 {} 個）。\n",
        group(tail_chars as i64),
        answer_count,
        read,
        s.logs.len(),
    ));
    o.push_str("  那 log 過了一次遮蔽，只換掉認得出來的路徑和使用者名稱——那是一份黑名單，\n");
    o.push_str("  不是保證。所以線以下請你自己看一眼再貼。\n");
    // 這個檔刻意寫在資料目錄外面（放進去，三條刪除路就各要接一次）。代價是
    // `forget` 掃不到它，而那句話要自己講——一份講究「刪得掉」的產品，多出
    // 一個沒人管的檔案卻不說，那是最難看的一種。
    o.push_str("  這個檔在資料目錄外面，`sister forget` 掃不到它。不要了就自己刪掉。\n");
    o
}

fn render_tail(s: &Snapshot) -> String {
    let mut o = String::new();
    o.push_str("\n⑤ 她最後幾句答案的原文\n");
    o.push_str("  （你打的問題沒有帶，只帶字數。要我看問題的話你自己貼。）\n");
    match &s.answers {
        Err(absent) => o.push_str(&format!("  {}\n", absent.say())),
        Ok(answers) if answers.is_empty() => o.push_str("  這一輪她一句都還沒答。\n"),
        Ok(answers) => {
            for answer in answers {
                o.push_str(&format!(
                    "\n  {}　你打了 {} 個字{}\n",
                    stamp(answer.at),
                    answer.question_chars,
                    answer
                        .took_ms
                        .map(|ms| format!("，她花了 {} 毫秒", group(ms)))
                        .unwrap_or_default(),
                ));
                for (i, sentence) in answer.sentences.iter().enumerate() {
                    let source = answer.sources.get(i).map(String::as_str).unwrap_or("");
                    o.push_str(&format!("    {sentence}\n"));
                    if !source.is_empty() {
                        o.push_str(&format!("      出處 {source}\n"));
                    }
                }
            }
        }
    }

    o.push_str("\n⑥ log 尾巴\n");
    if s.logs.is_empty() {
        o.push_str("  一個 log 都沒讀到。\n");
    }
    for log in &s.logs {
        match &log.lines {
            Err(absent) => o.push_str(&format!("\n  {}：{}\n", log.name, absent.say())),
            Ok(lines) => {
                o.push_str(&format!(
                    "\n  {}（一共 {} 行，這裡是最後 {} 行）\n",
                    log.name,
                    group(log.total_lines as i64),
                    lines.len()
                ));
                for line in lines {
                    o.push_str("    ");
                    o.push_str(line);
                    o.push('\n');
                }
            }
        }
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::OutboundRow;

    /// 螢幕上會有、報告裡絕對不該有的一串字。
    const SCREEN_TEXT: &str = "王小明的帳號密碼是 hunter2";

    pub(super) fn row(
        role: &str,
        outcome: &str,
        duration_ms: i64,
        error: Option<&str>,
    ) -> OutboundRow {
        OutboundRow {
            id: 1,
            ts: 1_789_222_440_000,
            day_key: "2026-09-12".into(),
            command: "sister".into(),
            args_json: "[]".into(),
            segment_core_start: None,
            chars_sent: 4_212,
            truncated: false,
            outcome: outcome.into(),
            duration_ms,
            error: error.map(str::to_string),
            role: role.into(),
        }
    }

    pub(super) fn snapshot() -> Snapshot {
        Snapshot {
            at: 1_789_225_320_000,
            app_version: "0.1.0-alpha.133".into(),
            platform: "windows".into(),
            source: "sister diagnose".into(),
            run_started_at: Some(1_789_222_000_000),
            data_dir_shown: "<資料夾>".into(),
            consent: ConsentLines::of(&Consent::default()),
            doctor: Err(Absent::NotThere),
            db: Err(Absent::NotThere),
            legs: Ok(Vec::new()),
            skips: Ok(Vec::new()),
            self_check: Err(Absent::NotHere),
            answers: Ok(Vec::new()),
            logs: Vec::new(),
        }
    }

    /// 這條測試斷言的是**結局**：那串字不在報告裡。不是「我們有記得呼叫
    /// 分類器」——那是機制，而機制斷言擋不住下一個人多印一個欄位。
    #[test]
    fn the_error_text_never_reaches_the_report() {
        let leaky = format!("JSON 對不上回答契約：invalid type: string \"{SCREEN_TEXT}\"");
        let mut snap = snapshot();
        snap.legs = Ok(vec![Leg::from_row(&row(
            "answer",
            "bad_json",
            3_118,
            Some(&leaky),
        ))]);

        let report = render(&snap);
        assert!(
            !report.contains(SCREEN_TEXT),
            "外送紀錄的錯誤原文漏進報告了：\n{report}"
        );
        assert!(
            !report.contains("invalid type"),
            "serde 的訊息也算原文的一部分：\n{report}"
        );
        // 但診斷價值不能一起丟掉：類別和長度要留著。
        assert!(report.contains("JSON 對不上契約"), "{report}");
        assert!(
            report.contains(&format!("{} 字，沒印出來", leaky.chars().count())),
            "{report}"
        );
    }

    /// 那句 `叫不起 \`{{command}}\`：{{e}}` 的第一個字就是使用者機器上的路徑。
    #[test]
    fn a_spawn_error_carrying_a_path_is_reduced_to_a_kind() {
        let leaky = "叫不起 `C:\\Users\\小明\\AppData\\Local\\sister\\grok.exe`：找不到檔案";
        let mut snap = snapshot();
        snap.legs = Ok(vec![Leg::from_row(&row(
            "answer_search",
            "spawn_failed",
            12,
            Some(leaky),
        ))]);
        let report = render(&snap);
        assert!(
            !report.contains("小明"),
            "路徑裡的使用者名稱漏了：\n{report}"
        );
        assert!(!report.contains("grok.exe"), "{report}");
        assert!(report.contains("CLI 沒跑成"), "{report}");
    }

    #[test]
    fn the_two_answer_legs_are_told_apart() {
        let mut snap = snapshot();
        snap.legs = Ok(vec![
            Leg::from_row(&row("answer_search", "success", 1_100, None)),
            Leg::from_row(&row("answer", "success", 3_000, None)),
            Leg::from_row(&row("answer_search", "success", 1_200, None)),
            Leg::from_row(&row("answer", "success", 3_400, None)),
        ]);
        let report = render(&snap);
        assert!(
            report.contains("先問它要查什麼   2 趟，中位數 1,200 毫秒"),
            "{report}"
        );
        assert!(
            report.contains("再請它寫成一句   2 趟，中位數 3,400 毫秒"),
            "{report}"
        );
        assert!(report.contains("兩個中位數加起來 4,600 毫秒"), "{report}");
    }

    /// 沒有樣本的那一趟不可以印成「0 毫秒」——那是一句假話，而且它剛好長得
    /// 像「這一趟很快」。
    #[test]
    fn a_leg_with_no_samples_is_not_printed_as_zero() {
        let mut snap = snapshot();
        snap.legs = Ok(vec![Leg::from_row(&row(
            "answer_search",
            "success",
            900,
            None,
        ))]);
        let report = render(&snap);
        assert!(report.contains("先問它要查什麼"), "{report}");
        assert!(
            !report.contains("再請它寫成一句"),
            "沒有樣本的那一趟不該有一行：\n{report}"
        );
        assert!(!report.contains("兩個中位數加起來"), "{report}");
    }

    /// `Absent` 的每一種都要在紙上留下一行。一節不見了和一節沒問題，
    /// 在紙上長得一模一樣——這是這份報告最容易犯的錯。
    #[test]
    fn a_section_that_could_not_be_read_says_so_instead_of_vanishing() {
        for absent in [
            Absent::NotThere,
            Absent::Unreadable(std::io::ErrorKind::PermissionDenied),
            Absent::QueryFailed,
            Absent::NotHere,
        ] {
            let mut snap = snapshot();
            snap.db = Err(absent);
            snap.legs = Err(absent);
            snap.skips = Err(absent);
            snap.answers = Err(absent);
            let report = render(&snap);
            for heading in [
                "① 這台機器",
                "② 她問 CLI 的每一趟",
                "③ 自檢",
                "⑤ 她最後幾句答案的原文",
            ] {
                assert!(
                    report.contains(heading),
                    "{absent:?} 把整節弄不見了：\n{report}"
                );
            }
            assert!(
                report.contains(&absent.say()) || matches!(absent, Absent::NotHere),
                "{absent:?} 沒有說出理由：\n{report}"
            );
        }
    }

    /// 一份只該讀的東西，不可以在空機器上造出一個資料庫。
    ///
    /// `Db::open` 會 `create_dir_all` 再把 schema 建起來。少了那道 `exists()`
    /// 閘門，他還沒錄過就跑 `sister diagnose`，報告會印「0 幀」——而那句話是
    /// 這次執行自己造出來的，不是他機器上本來的狀態。
    #[test]
    fn diagnosing_an_empty_machine_does_not_create_a_database() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "ai-sister-diagnose-empty-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");

        let snapshot = collect(&dir, "測試", "0.0.0-test");
        let made_one = crate::config::Config::db_path(&dir).exists();
        let report = render(&snapshot);
        let why = why_no_db(&dir);
        let db = format!("{:?}", snapshot.db);
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(why, Absent::NotThere, "空資料夾應該是「還沒有這個檔案」");
        assert!(
            matches!(snapshot.db, Err(Absent::NotThere)),
            "空機器上那一格要說「沒有這個檔案」，不是「問不出來」：{db}"
        );
        assert!(
            !made_one,
            "跑一次診斷就把資料庫建出來了——那是寫入端，不是診斷"
        );
        assert!(
            !report.contains("0 幀"),
            "報告印了「0 幀」，而那是它自己造出來的數字：\n{report}"
        );
    }

    /// 這個檔在刪除路外面，報告要自己講。
    ///
    /// 整個產品的前提是「刪得掉」（`forget`／`export`／`prune` 三條路）。這份
    /// 報告刻意寫在資料目錄外面——放進去，三條路就各要多接一次——代價是
    /// `forget` 掃不到它。多出一個沒人管的檔案卻不說，是最難看的那一種。
    #[test]
    fn the_report_admits_that_forget_cannot_reach_it() {
        let report = render(&snapshot());
        assert!(
            report.contains("`sister forget` 掃不到它"),
            "報告沒講它自己在刪除路外面：\n{report}"
        );
    }

    /// ④ 印的那個字數必須是真的量出來的，不是一句好聽的話。
    #[test]
    fn the_ledger_counts_what_it_claims_to_count() {
        let mut snap = snapshot();
        snap.legs = Ok(vec![Leg::from_row(&row("answer", "success", 3_000, None))]);
        snap.answers = Ok(vec![Answer {
            at: 1_789_225_000_000,
            question_chars: 7,
            sentences: vec!["喔你十點十四分要求 Codex agent 交接。".into()],
            sources: vec!["文字#5443".into()],
            took_ms: Some(4_100),
        }]);
        let report = render(&snap);

        // 量的是那句話**宣稱**的東西——「這條線以上」——不是它背後的某一段。
        // 上一版量到 ④ 的標題就停了，於是 ④ 自己那幾行沒被算進去，數字少了
        // 四分之一而這條測試是綠的。
        let divider_at = report.find(DIVIDER).expect("要有那條線");
        let above = report[..divider_at].chars().count();
        assert!(
            report.contains(&format!("這條線以上 {} 個字", group(above as i64))),
            "④ 講的字數和線以上實際的字數對不上（實際 {above}）：\n{report}"
        );

        // 分隔線自己那個換行算在線上，不算在「線以下」。
        let below_at = divider_at + report[divider_at..].find('\n').expect("線後面要有換行") + 1;
        let below = report[below_at..].chars().count();
        assert!(
            report.contains(&format!("這條線以下 {} 個字", group(below as i64))),
            "④ 講的線下字數對不上（實際 {below}）：\n{report}"
        );
    }

    /// Ted 要能整段刪掉再貼。所以答案原文和 log 一定要在線的下面，
    /// 而機制那幾節一定要在線的上面。
    #[test]
    fn the_answers_are_below_the_line_and_the_mechanism_is_above() {
        let mut snap = snapshot();
        snap.legs = Ok(vec![Leg::from_row(&row("answer", "success", 3_118, None))]);
        snap.answers = Ok(vec![Answer {
            at: 1_789_225_000_000,
            question_chars: 7,
            sentences: vec![SCREEN_TEXT.into()],
            sources: vec!["文字#5443".into()],
            took_ms: None,
        }]);
        snap.logs = vec![LogTail {
            name: "desktop.log".into(),
            lines: Ok(vec!["INFO 開始".into()]),
            total_lines: 1,
        }];
        let report = render(&snap);

        let divider = report.find(DIVIDER).expect("要有那條線");
        assert!(
            report.find("3,118").expect("毫秒") < divider,
            "機制掉到線下面了"
        );
        assert!(
            report.find(SCREEN_TEXT).expect("答案") > divider,
            "答案原文跑到線上面了"
        );
        assert!(
            report.find("INFO 開始").expect("log") > divider,
            "log 跑到線上面了"
        );
        // 兩節各有自己的標題，才能只刪一節。
        assert!(report.contains("⑤ 她最後幾句答案的原文"), "{report}");
        assert!(report.contains("⑥ log 尾巴"), "{report}");
    }

    #[test]
    fn a_word_cannot_carry_screen_content() {
        assert!(Word::new("chrome-open").is_some());
        assert!(Word::new("hidden").is_some());
        assert!(Word::new("v2").is_some());
        assert!(Word::new(SCREEN_TEXT).is_none(), "中文不該進得去");
        assert!(Word::new("Hello").is_none(), "大寫不收，免得有人塞句子");
        assert!(Word::new("two words").is_none());
        assert!(Word::new("").is_none());
        assert!(Word::new(&"a".repeat(33)).is_none());
        assert!(Word::new(&"a".repeat(32)).is_some());
    }

    /// 資料夾路徑把家目錄整個含在裡面。順序反了就只換掉半截，剩下
    /// `<家目錄>/AI-Sister/data`——而那半截裡就有使用者名稱。
    #[test]
    fn the_longer_needle_wins() {
        let mut scrub = Scrubber::new();
        scrub.hide_paths(
            Path::new("/home/xiaoming/.local/share/sister"),
            Some(Path::new("/home/xiaoming")),
            Some("xiaoming"),
        );
        let out = scrub.apply("開啟 /home/xiaoming/.local/share/sister/sister.db 失敗");
        assert_eq!(out, "開啟 <資料夾>/sister.db 失敗");
        assert!(!out.contains("xiaoming"));
    }

    #[test]
    fn both_separators_and_ascii_case_are_covered() {
        let mut scrub = Scrubber::new();
        scrub.hide_paths(
            Path::new(r"C:\Users\Ted\AppData\Local\sister"),
            None,
            Some("Ted"),
        );
        for line in [
            r"讀不到 C:\Users\Ted\AppData\Local\sister\sister.db",
            r"讀不到 c:\users\ted\appdata\local\sister\sister.db",
            "讀不到 C:/Users/Ted/AppData/Local/sister/sister.db",
        ] {
            let out = scrub.apply(line);
            assert!(out.contains("<資料夾>"), "沒換到：{out}");
            assert!(
                !out.to_ascii_lowercase().contains("ted"),
                "還看得到名字：{out}"
            );
        }
    }

    /// 遮蔽是按位元組推進的。中文夾在針的旁邊時不可以切壞。
    #[test]
    fn scrubbing_does_not_split_a_multibyte_char() {
        let mut scrub = Scrubber::new();
        scrub.hide("/home/ted", "<家目錄>");
        let out = scrub.apply("在這裡：/home/ted／找不到，請看說明。");
        assert_eq!(out, "在這裡：<家目錄>／找不到，請看說明。");
    }

    /// 兩個字元的針會把整份報告打成馬賽克。使用者名稱短到那個地步的時候，
    /// 寧可不換——換了反而看不出發生什麼事，而那正是這份報告的用途。
    #[test]
    fn a_needle_too_short_to_be_safe_is_refused() {
        let mut scrub = Scrubber::new();
        scrub.hide("ab", "<x>");
        assert_eq!(scrub.apply("about"), "about");
    }

    #[test]
    fn tail_lines_caps_both_axes() {
        let text = format!("a\nb\nc\n{}\ne", "x".repeat(500));
        let (lines, total) = tail_lines(&text, 3, 40);
        assert_eq!(total, 5);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "c");
        assert!(
            lines[1].ends_with("…（這一行還有 460 字沒印）"),
            "{}",
            lines[1]
        );
        assert_eq!(
            lines[1].chars().count(),
            40 + "…（這一行還有 460 字沒印）".chars().count()
        );
        assert_eq!(lines[2], "e");
    }

    /// Windows 上 checkout 出來的東西是 CRLF。留著 `\r` 會讓貼出來的報告
    /// 每一行後面多一個看不見的字元。
    #[test]
    fn tail_lines_strips_carriage_returns() {
        let (lines, _) = tail_lines("一\r\n二\r\n", 5, 80);
        assert_eq!(lines, vec!["一".to_string(), "二".to_string()]);
    }

    #[test]
    fn skips_fold_into_counts_sorted_by_how_often() {
        let mk = |reason: &str| SkipRow {
            id: 0,
            ts: 0,
            reason: reason.into(),
            segment_core_start: None,
            detail: "這句話不該出現在報告裡".into(),
        };
        let folded = SkipCount::fold(&[
            mk("budget_exhausted"),
            mk("no_consent"),
            mk("no_consent"),
            mk("no_consent"),
        ]);
        assert_eq!(
            folded,
            vec![
                SkipCount {
                    reason: "no_consent".into(),
                    times: 3
                },
                SkipCount {
                    reason: "budget_exhausted".into(),
                    times: 1
                },
            ]
        );
        let mut snap = snapshot();
        snap.skips = Ok(folded);
        let report = render(&snap);
        assert!(report.contains("no_consent"), "{report}");
        assert!(
            !report.contains("這句話不該出現在報告裡"),
            "`detail` 不該進報告——它是 `reason.message()`，會跟著文案改：\n{report}"
        );
    }

    /// 簽過、但條文改版了，所以不生效。這兩件事在紙上要分得出來：
    /// 一個「沒簽」和一個「簽過但要重簽」，要修的動作不一樣。
    #[test]
    fn a_signature_against_older_terms_reads_as_needing_a_resign() {
        let mut consent = Consent::default();
        consent.grant(Sheet::LocalRecording, 1_789_000_000_000);
        consent.version = crate::consent::VERSION - 1;
        let mut snap = snapshot();
        snap.consent = ConsentLines::of(&consent);
        let report = render(&snap);
        assert!(
            report.contains("簽過但不生效（條文改版了，要重簽）"),
            "{report}"
        );
    }

    /// `grounded_answer.rs` 長出新的錯誤句而這裡沒跟上的話，報告會把它印成
    /// 「其他」——安全，但那正是他最需要看清楚的那一格。
    ///
    /// 掃得到的形狀只有這四種寫法（`Err("…"`、`Err(format!("…"`、
    /// `map_err(|error| format!("…"`、`ok_or_else(|| format!("…"`）。換一種寫法
    /// 就掃不到，所以底下還釘了一個下限：句子總數掉下去也會紅。
    #[test]
    fn every_error_sentence_in_grounded_answer_has_a_kind() {
        const SOURCE: &str = include_str!("grounded_answer.rs");
        const MARKERS: [&str; 4] = [
            "Err(\"",
            "Err(format!(\"",
            "map_err(|error| format!(\"",
            "ok_or_else(|| format!(\"",
        ];
        let mut found: Vec<String> = Vec::new();
        for marker in MARKERS {
            let mut from = 0;
            while let Some(hit) = SOURCE[from..].find(marker) {
                let start = from + hit + marker.len();
                let literal: String = SOURCE[start..]
                    .chars()
                    .take_while(|c| *c != '"' && *c != '{')
                    .collect();
                let literal = literal.trim().to_string();
                if !literal.is_empty() && !found.contains(&literal) {
                    found.push(literal);
                }
                from = start;
            }
        }
        assert!(
            found.len() >= 12,
            "只掃到 {} 句錯誤訊息；`grounded_answer.rs` 換了寫法，這條測試就瞎了：{found:?}",
            found.len()
        );
        let unclassified: Vec<&String> = found
            .iter()
            .filter(|literal| ErrorKind::classify(literal) == ErrorKind::Other)
            .collect();
        assert!(
            unclassified.is_empty(),
            "這幾句錯誤訊息在報告裡只會印「其他」，請補進 ErrorKind::PREFIXES：{unclassified:?}"
        );
    }

    /// 分類器不可以什麼都認得——那樣它就沒有在分類了。
    #[test]
    fn an_unknown_error_falls_back_instead_of_guessing() {
        assert_eq!(ErrorKind::classify("完全沒見過的東西"), ErrorKind::Other);
        assert_eq!(ErrorKind::classify(""), ErrorKind::Other);
        assert_eq!(
            ErrorKind::classify("CLI invocation 在啟動前已取消"),
            ErrorKind::NeverStarted
        );
        assert_eq!(
            ErrorKind::classify(
                "`/opt/x/grok` 已放進子行程範圍，但 suspended primary thread 無法啟動：5"
            ),
            ErrorKind::CliFailed
        );
    }

    #[test]
    fn numbers_read_like_numbers() {
        assert_eq!(group(0), "0");
        assert_eq!(group(999), "999");
        assert_eq!(group(1_000), "1,000");
        assert_eq!(group(1_234_567), "1,234,567");
        assert_eq!(group(-1_234), "-1,234");
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(1_536), "1.5 KB");
        assert_eq!(median(vec![]), None);
        assert_eq!(median(vec![3, 1, 2]), Some(2));
    }
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    #[test]
    fn a_cjk_char_counts_as_two_cells() {
        assert_eq!(cells("abc"), 3);
        assert_eq!(cells("本機記錄"), 8);
        assert_eq!(cells("Azure 朗讀"), 10);
        assert_eq!(cells(""), 0);
    }

    /// 「Azure 朗讀」和「本機記錄」的 char 數差一倍，格寬只差 2。
    /// 用 `{:<10}` 排的話這兩行會歪掉，而它們就上下相鄰。
    #[test]
    fn padding_lines_up_names_of_different_char_counts() {
        let a = pad("本機記錄", 14);
        let b = pad("Azure 朗讀", 14);
        assert_eq!(cells(&a), 14);
        assert_eq!(cells(&b), 14);
        assert_ne!(a.chars().count(), b.chars().count(), "char 數本來就不一樣");
    }

    /// 超過欄寬的時候不要截。截掉的那截多半正是他要看的東西。
    #[test]
    fn padding_never_truncates() {
        assert_eq!(
            pad("超級無敵長的一個角色名稱", 4),
            "超級無敵長的一個角色名稱"
        );
    }

    #[test]
    fn right_aligned_numbers_end_at_the_same_column() {
        assert_eq!(cells(&rpad("1", 9)), 9);
        assert_eq!(cells(&rpad("123,456", 9)), 9);
    }

    /// 表頭和資料列在**印出來的那份報告裡**要對得齊。
    ///
    /// 前一版這條測試自己呼叫 `leg_line` 兩次再比——那只證明了 `leg_line`
    /// 和自己一致，產品那邊改回手寫 `{:<18}` 它照樣綠。要證的是印出來的字，
    /// 就得去讀印出來的字。
    #[test]
    fn the_rendered_table_lines_up() {
        let mut snap = tests::snapshot();
        snap.legs = Ok(vec![Leg::from_row(&tests::row(
            "answer_search",
            "success",
            1_204,
            None,
        ))]);
        let report = render(&snap);

        let header = report
            .lines()
            .find(|line| line.contains("送出字數"))
            .expect("要有表頭");
        let row = report
            .lines()
            .find(|line| line.contains("answer_search"))
            .expect("要有資料列");
        // 表頭最後多一欄「註」，資料列那一欄是空的。扣掉之後兩邊要一樣寬。
        let header_width = cells(header.strip_suffix("  註").expect("表頭結尾是「  註」"));
        assert_eq!(
            header_width,
            cells(row.trim_end()),
            "表頭和資料列對不齊：\n{header}|\n{row}|"
        );
        // 而且真的有補到寬——不是兩邊剛好都是原字串。
        assert!(
            header_width > cells("時間角色結局毫秒送出字數"),
            "根本沒有補寬"
        );
    }

    /// 找了四個檔、一個都沒讀到，就不可以寫「加上 4 個 log 檔的尾巴」。
    #[test]
    fn the_ledger_counts_logs_it_actually_read() {
        let mut snap = tests::snapshot();
        snap.logs = vec![
            LogTail {
                name: "desktop.log".into(),
                lines: Err(Absent::NotThere),
                total_lines: 0,
            },
            LogTail {
                name: "record.log".into(),
                lines: Ok(vec!["INFO".into()]),
                total_lines: 1,
            },
        ];
        let report = render(&snap);
        assert!(report.contains("加上 1 個 log 檔（找了 2 個）"), "{report}");
    }
}

#[cfg(test)]
mod token_tests {
    use super::*;

    /// `role`／`outcome` 原樣印出來的前提是「每一個寫入端都是字面值」。
    /// 那是一句假設，而這條測試讓它變成一句有人守的話。
    #[test]
    fn a_field_that_does_not_look_like_a_token_is_reduced_to_its_length() {
        assert_eq!(token_or_length("answer_search"), "answer_search");
        assert_eq!(token_or_length("bad_json"), "bad_json");
        assert_eq!(token_or_length("future_token"), "future_token");
        assert_eq!(token_or_length("王小明的帳號密碼是 hunter2"), "?（17 字）");
        assert_eq!(token_or_length(""), "?（0 字）");
        assert_eq!(token_or_length(&"a".repeat(33)), "?（33 字）");
    }

    /// 一列被外面改過的稽核紀錄，也不可以把內容印進報告。
    #[test]
    fn a_tampered_audit_row_still_cannot_put_content_in_the_report() {
        let screen = "王小明的帳號密碼是 hunter2";
        let mut snap = tests::snapshot();
        snap.legs = Ok(vec![Leg::from_row(&tests::row(screen, screen, 12, None))]);
        snap.skips = Ok(SkipCount::fold(&[SkipRow {
            id: 1,
            ts: 0,
            reason: screen.into(),
            segment_core_start: None,
            detail: String::new(),
        }]));
        let report = render(&snap);
        assert!(!report.contains(screen), "{report}");
        assert!(report.contains("?（17 字）"), "{report}");
    }
}

#[cfg(test)]
mod notebook_tests {
    use super::*;

    const SCREEN_TEXT: &str = "王小明的帳號密碼是 hunter2";

    fn started() -> Note {
        Note::Started {
            at: 1_789_222_000_000,
        }
    }

    fn persona() -> Note {
        Note::Persona {
            at: 1_789_222_000_000,
            taps: 10,
            giggles: 5,
            beats: 5,
        }
    }

    fn poke(at: Millis, moved: bool, clip: Option<&str>) -> Note {
        Note::Poke {
            at,
            moved,
            clip: clip.map(str::to_string),
            voiced: clip.is_some(),
        }
    }

    fn skipped(at: Millis, why: &str) -> Note {
        Note::GiggleSkipped {
            at,
            why: why.to_string(),
        }
    }

    fn failed(at: Millis, why: &str) -> Note {
        Note::AskFailed {
            at,
            question_chars: 8,
            why: why.to_string(),
        }
    }

    fn answered(at: Millis, took_ms: i64) -> Note {
        Note::Answered {
            at,
            question_chars: 12,
            took_ms,
            sentences: vec!["她講的話".to_string()],
            sources: vec![],
        }
    }

    /// 開機那兩則永遠在最前面——真的跑起來也是這個順序。
    ///
    /// 上一版只推了 `persona()`，於是這個 helper 做出來的簿子沒有開機那一則，
    /// 而少了它 `fill` 就不掛自檢——註解說兩則、程式碼推一則，測試看不出來。
    fn book(notes: Vec<Note>) -> Notebook {
        let mut book = Notebook::new();
        book.note(started());
        book.note(persona());
        for note in notes {
            book.note(note);
        }
        book
    }

    fn item(book: &Notebook, number: u8) -> Item {
        book.items()
            .into_iter()
            .find(|item| item.number == number)
            .unwrap_or_else(|| panic!("要有第 {number} 項"))
    }

    fn value(item: &Item, label: &str) -> MeasureValue {
        item.measured
            .iter()
            .find(|m| m.label == label)
            .unwrap_or_else(|| panic!("要有「{label}」：{:?}", item.measured))
            .value
            .clone()
    }

    /// 這一輪沒發生過的事，要印「沒量」，不可以印成 ✓。
    ///
    /// 這是這份自檢最容易犯的錯：七格全綠，而其中五格根本沒有樣本。
    ///
    /// 「沒量」還分兩種，而分法決定他該做什麼：② ③ ④ ⑥ 是「你還沒去做那件
    /// 事」，多玩一下就有了；① 是「開機一定會送一則那一槓的觀測，一則都沒有
    /// ＝那一格量不出來」，那要我去修。⑤ 兩種都可能，看他問過題沒有——這裡
    /// 一題都沒問，所以是前者。
    #[test]
    fn nothing_observed_is_not_the_same_as_nothing_wrong() {
        let book = book(vec![started()]);
        // 原本那條斷言就是這一句：沒有樣本，一格都不准是 ✓。
        for number in [1, 2, 3, 4, 5, 6, 7] {
            assert_ne!(
                item(&book, number).verdict,
                Verdict::AsAsked,
                "第 {number} 項沒有樣本卻印成 ✓"
            );
        }
        for number in [2, 3, 4, 5, 6] {
            assert_eq!(
                item(&book, number).verdict,
                Verdict::NotSeen,
                "第 {number} 項沒有樣本，要說「這一輪沒發生過」"
            );
        }
        assert_eq!(
            item(&book, 1).verdict,
            Verdict::NotMeasured,
            "開機一定會送一則那一槓的觀測，一則都沒有不是他沒去翻"
        );
        // 第 7 項永遠是「機器判不了」——它沒有一個數字答得出來。
        assert_eq!(item(&book, 7).verdict, Verdict::CantJudge);
    }

    /// 上面那一槓收起來了，卻還看得見——這正是第 1 項要抓的失敗。
    #[test]
    fn a_bar_that_is_closed_but_still_visible_is_off() {
        let good = book(vec![
            started(),
            Note::Bar {
                at: 1,
                open: false,
                dragbar_hidden: true,
            },
        ]);
        assert_eq!(item(&good, 1).verdict, Verdict::AsAsked);

        let bad = book(vec![
            started(),
            Note::Bar {
                at: 1,
                open: false,
                dragbar_hidden: false,
            },
        ]);
        assert_eq!(item(&bad, 1).verdict, Verdict::Off);
        assert_eq!(
            value(&item(&bad, 1), "其中那一槓還看得見的"),
            MeasureValue::Int(1)
        );
    }

    /// 戳了會動但不出聲，和戳了不會動，是兩種不一樣的壞。兩種都要抓到。
    #[test]
    fn a_poke_that_moves_but_never_speaks_is_off() {
        let silent = book(vec![started(), poke(1, true, None), poke(2, true, None)]);
        assert_eq!(item(&silent, 2).verdict, Verdict::Off);
        assert_eq!(value(&item(&silent, 2), "有動的"), MeasureValue::Int(2));
        assert_eq!(value(&item(&silent, 2), "有講話的"), MeasureValue::Int(0));

        // 講了話但語音關著，不算壞——那是他自己關的。
        let muted = book(vec![
            started(),
            Note::Poke {
                at: 1,
                moved: true,
                clip: Some("poke-aiyo".into()),
                voiced: false,
            },
        ]);
        assert_eq!(item(&muted, 2).verdict, Verdict::AsAsked);
        assert_eq!(value(&item(&muted, 2), "真的出聲的"), MeasureValue::Int(0));

        let still = book(vec![
            started(),
            poke(1, false, Some("poke-aiyo")),
            poke(2, true, Some("poke-stop")),
        ]);
        assert_eq!(item(&still, 2).verdict, Verdict::Off);

        let good = book(vec![
            started(),
            poke(1, true, Some("poke-aiyo")),
            poke(2, true, Some("poke-stop")),
        ]);
        assert_eq!(item(&good, 2).verdict, Verdict::AsAsked);
    }

    /// 「按來按去只會一直講兩句話」——第 3 項就是為了這句話存在的。
    #[test]
    fn always_the_same_line_is_off() {
        let same = book(vec![
            started(),
            poke(1, true, Some("poke-aiyo")),
            poke(2, true, Some("poke-aiyo")),
            poke(3, true, Some("poke-aiyo")),
        ]);
        assert_eq!(item(&same, 3).verdict, Verdict::Off);
        assert_eq!(
            value(&item(&same, 3), "用過幾句不同的"),
            MeasureValue::Int(1)
        );
        assert_eq!(
            value(&item(&same, 3), "這個角色手上有幾句"),
            MeasureValue::Int(10)
        );

        let varied = book(vec![
            started(),
            poke(1, true, Some("poke-aiyo")),
            poke(2, true, Some("poke-stop")),
        ]);
        assert_eq!(item(&varied, 3).verdict, Verdict::AsAsked);
    }

    /// 「問了一題竟然超過四秒？」
    #[test]
    fn a_median_answer_over_four_seconds_is_off() {
        let answered = |took_ms| Note::Answered {
            at: 1,
            question_chars: 7,
            took_ms,
            sentences: vec!["喏，在這裡。".into()],
            sources: vec!["文字#5443".into()],
        };
        let slow = book(vec![started(), answered(4_500), answered(6_200)]);
        assert_eq!(item(&slow, 4).verdict, Verdict::Off);
        assert_eq!(
            value(&item(&slow, 4), "最慢那一題"),
            MeasureValue::Int(6_200)
        );

        let quick = book(vec![started(), answered(2_100), answered(3_900)]);
        assert_eq!(item(&quick, 4).verdict, Verdict::AsAsked);
        // 邊界：剛好四秒還算過。
        let edge = book(vec![started(), answered(4_000)]);
        assert_eq!(item(&edge, 4).verdict, Verdict::AsAsked);
    }

    /// 幾何要看「捲得到多少」，不是看 `scrollHeight - clientHeight`。
    ///
    /// 那兩個數字在「內容捲得動」和「內容整個溢出去」兩種情況下一模一樣，
    /// 而那正是這一格要分辨的兩件事。
    #[test]
    fn a_bubble_with_content_scrolled_out_of_sight_is_off() {
        let bubble = |can_scroll_to, bottom| Note::Bubble {
            at: 1,
            bubble_h: 235.0,
            bubble_bottom: bottom,
            content_h: 554.0,
            client_h: 207.0,
            can_scroll_to,
            scrollbar_px: 0.0,
            window_h: 560.0,
        };
        let clipped = book(vec![started(), bubble(347.0, 245.0)]);
        assert_eq!(item(&clipped, 5).verdict, Verdict::Off);
        assert_eq!(
            value(&item(&clipped, 5), "捲得到（0 就是沒被切掉）"),
            MeasureValue::Num(347.0)
        );

        let whole = book(vec![started(), bubble(0.0, 245.0)]);
        assert_eq!(item(&whole, 5).verdict, Verdict::AsAsked);

        // 一整顆氣泡掉到視窗外面也是壞的，即使裡面沒有東西被捲走。
        let overflowing = book(vec![started(), bubble(0.0, 592.0)]);
        assert_eq!(item(&overflowing, 5).verdict, Verdict::Off);
    }

    /// 畫面送過來的 clip 代號要過 `Word` 那一關。過不了就當成沒出聲。
    #[test]
    fn a_clip_id_that_is_not_a_word_is_dropped() {
        let book = book(vec![
            started(),
            poke(1, true, Some(SCREEN_TEXT)),
            poke(2, true, Some("poke-aiyo")),
        ]);
        // 兩下都算「戳了」，但只有一句過得了關。
        assert_eq!(value(&item(&book, 2), "戳了幾下"), MeasureValue::Int(2));
        assert_eq!(
            value(&item(&book, 3), "用過幾句不同的"),
            MeasureValue::Int(1)
        );

        let mut snapshot = tests::snapshot();
        book.fill(&mut snapshot, 2_000);
        assert!(
            !render(&snapshot).contains(SCREEN_TEXT),
            "clip 代號漏進報告了"
        );
    }

    /// 只留最後三段答案，而且舊的在前——讀的人是照時間往下看的。
    #[test]
    fn only_the_last_three_answers_are_kept_oldest_first() {
        let answered = |at: Millis, text: &str| Note::Answered {
            at,
            question_chars: 5,
            took_ms: 1_000,
            sentences: vec![text.to_string()],
            sources: vec!["文字#1".into()],
        };
        let book = book(vec![
            started(),
            answered(1, "第一"),
            answered(2, "第二"),
            answered(3, "第三"),
            answered(4, "第四"),
        ]);
        let answers = book.answers();
        assert_eq!(answers.len(), 3);
        assert_eq!(answers[0].sentences[0], "第二");
        assert_eq!(answers[2].sentences[0], "第四");
    }

    /// 畫面還沒說過話的時候，自檢是「沒量」，不是一份全綠的報告。
    #[test]
    fn a_notebook_that_never_heard_from_the_window_reports_nothing_measured() {
        let mut snapshot = tests::snapshot();
        Notebook::new().fill(&mut snapshot, 2_000);
        assert_eq!(snapshot.self_check.as_ref().err(), Some(&Absent::NotHere));
        assert_eq!(snapshot.run_started_at, None);
        let report = render(&snapshot);
        assert!(report.contains("這不是「沒問題」，是「沒量」"), "{report}");
    }

    /// 簿子有上限，因為它活在記憶體裡而她可以開一整天。
    #[test]
    fn the_notebook_forgets_the_oldest_notes() {
        let mut book = Notebook::new();
        book.note(started());
        // 時間戳用真的往前走的值：這條測試要看「涵蓋範圍有沒有跟著縮」，
        // 拿迴圈索引當時間會讓每一則都落在開機之前。
        for i in 0..(NOTES_KEPT as i64 + 50) {
            book.note(poke(1_789_222_000_000 + i, true, Some("poke-aiyo")));
        }
        assert_eq!(book.len(), NOTES_KEPT);
        // 開機那一則被擠掉了，但「這一輪什麼時候開的」不可以跟著不見。
        //
        // 上一版它就是跟著不見的：`started_at()` 去 `notes` 裡面找，而那一則
        // 永遠是第一則。他多戳幾百下，整節自檢就變成「沒量」——而簿子是滿的。
        // 一份為了分辨「量到 0」和「沒量到」而存在的報告，這是最不該犯的那種錯。
        assert_eq!(book.started_at(), Some(1_789_222_000_000));
        assert_eq!(book.dropped(), 51);
        // 涵蓋範圍要跟著縮：最舊的那幾則已經不在了。
        assert!(
            book.covers_from() > book.started_at(),
            "擠掉了 {} 則，涵蓋範圍卻還是從開機算起",
            book.dropped()
        );
    }

    /// 這一輪的涵蓋範圍要印在自檢那一節的第一行。
    /// 簿子擠滿了，自檢那一節還是要在。
    ///
    /// 上一版它會整節消失：`started_at()` 去 `notes` 裡找開機那一則，而那一則
    /// 永遠是第一則、第一個被擠掉。於是他戳了幾百下之後匯出，讀到的是
    /// 「這不是『沒問題』，是『沒量』」——而簿子是滿的。
    #[test]
    fn a_full_notebook_still_gets_a_self_check() {
        let report = render(&filled(NOTES_KEPT as i64 + 50));
        assert!(
            !report.contains("這不是「沒問題」，是「沒量」"),
            "簿子是滿的，自檢卻整節說沒量：\n{report}"
        );
        assert!(report.contains("第 2 項"), "自檢那幾格不見了：\n{report}");
    }

    /// 擠掉過就不可以再說「涵蓋整輪」。
    ///
    /// 那句話決定他怎麼讀底下每一格的「0 次」：是「真的沒發生」還是「沒看到」。
    #[test]
    fn a_full_notebook_admits_it_lost_the_early_part() {
        let report = render(&filled(NOTES_KEPT as i64 + 50));
        assert!(
            report.contains("已經滾掉了"),
            "沒承認漏掉那一段：\n{report}"
        );
        assert!(
            !report.contains("底下只涵蓋 2026-09-12 03:26:40 之後發生的事"),
            "擠掉過還在說涵蓋整輪：\n{report}"
        );

        // 對照組：沒擠掉的簿子照樣說涵蓋整輪，那句話不會平白多出來。
        let small = render(&filled(3));
        assert!(small.contains("底下只涵蓋"), "{small}");
        assert!(
            !small.contains("已經滾掉了"),
            "什麼都沒擠掉卻說滾掉了：\n{small}"
        );
    }

    /// 把一本簿子印成報告。`filled` 走的也是這條路。
    fn reported(book: &Notebook) -> String {
        let mut snapshot = tests::snapshot();
        book.fill(&mut snapshot, 1_789_225_320_000);
        render(&snapshot)
    }

    /// 他問過題，第五格卻一次都沒量到——那不是「沒發生」。
    ///
    /// 氣泡是跟著答案出現的，答案就在同一本簿子裡。所以這一格空著只有兩種
    /// 成因，兩種都要有人去看：畫面上根本沒有那個氣泡（他第五件事本人壞
    /// 了），或者量測被 `observation()` 吞掉的例外擋住了。印成「這一輪沒發生
    /// 過」的話，他讀到的是「多玩一下就有了」，而那是錯的建議。
    #[test]
    fn a_bubble_that_was_never_measured_is_not_a_bubble_that_never_happened() {
        let answered_only = book(vec![Note::Answered {
            at: 1_789_222_100_000,
            question_chars: 12,
            took_ms: 1_200,
            sentences: vec!["她講的話".to_string()],
            sources: vec![],
        }]);
        let fifth = item(&answered_only, 5);
        assert_eq!(
            fifth.verdict,
            Verdict::NotMeasured,
            "答過題卻沒量到氣泡，不可以說成「這一輪沒發生過」"
        );
        assert_eq!(
            value(&fifth, "答過幾題"),
            MeasureValue::Int(1),
            "要說得出「該量到幾次」，不然他不知道這一格為什麼該有東西"
        );

        // 對照組：一題都沒問過，那就真的只是沒發生。
        assert_eq!(
            item(&book(vec![]), 5).verdict,
            Verdict::NotSeen,
            "一題都沒問過還指控量測壞了，那是誣賴"
        );

        let report = reported(&answered_only);
        assert!(
            report.contains("！ 第 5 項"),
            "第 5 項要用得出「該量到卻沒量到」那個記號：\n{report}"
        );
    }

    /// 他問了三題、三題都沒答成——那不是「他沒問過」。
    ///
    /// 這是他實測時最可能遇到的一種：選的那支 CLI 還沒設好，一問就錯。少了
    /// 這一則，④⑤ 兩格印的都是「這一輪沒發生過，量不到」，而那句話的意思是
    /// 「你多玩一下就有了」——他剛剛已經做過了，做再多次也不會變。
    #[test]
    fn asks_that_all_failed_are_not_asks_that_never_happened() {
        /* 第一則、最後一則、最多的那一則各不相同，`commonest` 才真的被考到。 */
        let flopped = book(vec![
            failed(1_789_222_101_000, "consent_asked"),
            failed(1_789_222_102_000, "error"),
            failed(1_789_222_103_000, "error"),
            failed(1_789_222_104_000, "superseded"),
        ]);

        let fourth = item(&flopped, 4);
        assert_eq!(
            fourth.verdict,
            Verdict::NeverLanded,
            "問了四題一題都沒答成，不可以說成「這一輪沒發生過」"
        );
        assert_eq!(value(&fourth, "答了幾題"), MeasureValue::Int(0));
        assert_eq!(value(&fourth, "問了沒答成的"), MeasureValue::Int(4));
        assert_eq!(
            value(&fourth, "最常沒答成的原因"),
            MeasureValue::AskWhy(Word::new("error").expect("error 過得了 Word")),
            "四題裡兩題是出錯，指的要是那一條"
        );

        let fifth = item(&flopped, 5);
        assert_eq!(
            fifth.verdict,
            Verdict::NeverLanded,
            "沒有答案就沒有氣泡；這一格不是「他沒去做」，也不是「量壞了」"
        );
        assert_eq!(value(&fifth, "問了沒答成的"), MeasureValue::Int(4));

        // 對照組：一題都沒送出去過，那才是真的沒發生。
        let never = book(vec![]);
        assert_eq!(item(&never, 4).verdict, Verdict::NotSeen);
        assert_eq!(item(&never, 5).verdict, Verdict::NotSeen);
        assert_eq!(
            value(&item(&never, 4), "問了沒答成的"),
            MeasureValue::Int(0),
            "0 也要印出來——不印的話，一片空白仍然分不出那兩件事"
        );
    }

    /// 有幾題答成了，沒答成的那幾題還是要看得見。
    ///
    /// 這一格不是只在「全軍覆沒」的時候才有話講：五題壞兩題也是他要知道的
    /// 事，而 ④ 的判定看的是答成那幾題的中位數，不會提到它。
    #[test]
    fn answers_that_landed_do_not_hide_the_ones_that_did_not() {
        let mixed = book(vec![
            answered(1_789_222_101_000, 900),
            failed(1_789_222_102_000, "error"),
            answered(1_789_222_103_000, 1_100),
        ]);
        let fourth = item(&mixed, 4);
        assert_eq!(
            fourth.verdict,
            Verdict::AsAsked,
            "答成的那兩題都在四秒內，這一格就是照他要的"
        );
        assert_eq!(value(&fourth, "答了幾題"), MeasureValue::Int(2));
        assert_eq!(
            value(&fourth, "問了沒答成的"),
            MeasureValue::Int(1),
            "✓ 不可以把壞掉的那一題吃掉"
        );
    }

    /// 那一格要印他讀得懂的話，而且記號要換一個。
    #[test]
    fn the_report_says_in_words_why_the_asks_never_landed() {
        let report = reported(&book(vec![failed(1_789_222_101_000, "error")]));
        assert!(
            report.contains("出錯了，錯誤訊息在畫面上和底下的 log 尾巴"),
            "沒答成的原因要翻成中文，不要只印代號：\n{report}"
        );
        assert_eq!(
            mark_of(&report, 4),
            "…",
            "「試過了、每次都沒走到底」要有自己的記號，不能和「沒發生過」共用：\n{report}"
        );
        assert!(
            report.contains("你做了，可是每一次都沒走到底"),
            "記號旁邊那句話要說得出他該看哪裡：\n{report}"
        );
    }

    /// 畫面那半多一條出口而忘了說是哪一條，報告要指名我，不是指名他。
    ///
    /// `ask()` 只有一個記錄點（那個 `finally`），`gaveUp` 的預設值就是
    /// `unknown`。所以「沒人設代號」不會變成「這一題沒被記到」，會變成一句
    /// 我讀得出來的話。
    #[test]
    fn an_exit_that_forgot_to_say_why_points_at_me() {
        let report = reported(&book(vec![failed(1_789_222_101_000, "unknown")]));
        assert!(
            report.contains("畫面那半有一條出口沒講它是哪一條"),
            "`unknown` 是我的疏漏，不是他的操作：\n{report}"
        );
    }

    /// 這一邊不認得的代號，原樣印出來，不要印「不明」。
    #[test]
    fn an_ask_reason_this_side_does_not_know_still_names_itself() {
        let report = reported(&book(vec![failed(1_789_222_101_000, "port-in-use")]));
        assert!(
            report.contains("port-in-use"),
            "認不得就原樣印代號——「不明」會把「我沒跟上」講成「不知道為什麼」：\n{report}"
        );
    }

    /// 帶著螢幕上的字的代號，算得進次數，但一個字都不准印出來。
    ///
    /// 私密那條斷言排第一。排第二的話，一個「`Word` 什麼都收」的突變會先撞
    /// 到底下那條指名的斷言而紅，我就會以為這一條守住了——而它沒有。
    #[test]
    fn an_ask_reason_carrying_screen_text_is_counted_but_never_printed() {
        let smuggled = book(vec![
            failed(1_789_222_101_000, SCREEN_TEXT),
            failed(1_789_222_102_000, "error"),
        ]);
        let report = reported(&smuggled);
        assert!(
            !report.contains("hunter2"),
            "螢幕上的字沿著代號那一格溜進報告了：\n{report}"
        );

        let fourth = item(&smuggled, 4);
        assert_eq!(
            value(&fourth, "問了沒答成的"),
            MeasureValue::Int(2),
            "濾在數的那一步只會讓次數少報——一個數字夾帶不了任何東西"
        );
        assert_eq!(
            value(&fourth, "最常沒答成的原因"),
            MeasureValue::AskWhy(Word::new("error").expect("error 過得了 Word")),
            "指名只能指得出過得了 `Word` 的那幾個"
        );
    }

    /// 擠掉過就不可以指控那一槓的量測壞了。
    ///
    /// 開機那一則存在欄位裡、不會被擠；那一槓的那一則存在清單裡、會。所以
    /// 「有開機、沒有那一槓」在簿子滿了的時候是正常的，而在沒滿的時候才是
    /// 故障。分不出來就講回「沒量到」——指控要有把握。
    #[test]
    fn a_full_notebook_does_not_accuse_the_chrome_bar_probe() {
        let mut rolled = Notebook::new();
        rolled.note(started());
        for i in 0..(NOTES_KEPT as i64 + 50) {
            rolled.note(poke(1_789_222_000_000 + i, true, Some("poke-aiyo")));
        }
        assert!(rolled.dropped() > 0, "前提：這本簿子真的擠掉過");
        assert_eq!(
            item(&rolled, 1).verdict,
            Verdict::NotSeen,
            "擠掉過就分不出是壞了還是滾掉了，不可以指控"
        );

        // 對照組：沒擠掉過的同一種簿子，就要指得出來。
        let mut kept = Notebook::new();
        kept.note(started());
        kept.note(poke(1_789_222_000_000, true, Some("poke-aiyo")));
        assert_eq!(
            item(&kept, 1).verdict,
            Verdict::NotMeasured,
            "什麼都沒滾掉、開機那一則卻沒帶出那一槓＝那支量測沒跑"
        );
    }

    /// 自檢每一列的值都要和標籤隔開。
    ///
    /// 上一版欄寬寫死 24，而「捲得到（0 就是沒被切掉）」**剛好就是 24 格**，
    /// 於是 `pad` 一個空白都補不上，那一行印出來是「…被切掉）0.0」。一個寫死
    /// 的寬度會被下一個標籤再撞一次，而撞到的樣子是值黏在字上，不是編譯錯誤。
    /// 所以這裡不釘那一個標籤，整節每一列都量。
    #[test]
    fn every_self_check_row_keeps_its_value_off_the_label() {
        let mut book = Notebook::new();
        book.note(started());
        book.note(persona());
        book.note(Note::Bar {
            at: 1_789_222_000_000,
            open: false,
            dragbar_hidden: true,
        });
        book.note(poke(1_789_222_100_000, true, Some("tap-aiyo")));
        book.note(skipped(1_789_222_260_000, "typing"));
        book.note(Note::Answered {
            at: 1_789_222_700_000,
            question_chars: 9,
            took_ms: 2_780,
            sentences: vec!["她的答案".to_string()],
            sources: vec![],
        });
        // ⑤ 那幾列只有量到氣泡才印得出來，而最長的標籤就在那裡面。
        book.note(Note::Bubble {
            at: 1_789_222_700_100,
            bubble_h: 182.0,
            bubble_bottom: 431.0,
            content_h: 207.0,
            client_h: 207.0,
            can_scroll_to: 0.0,
            scrollbar_px: 0.0,
            window_h: 560.0,
        });
        let report = reported(&book);

        let rows: Vec<&str> = report
            .lines()
            .filter(|line| line.starts_with("      ") && !line.starts_with("       "))
            .collect();
        assert!(
            rows.len() >= 20,
            "只抓到 {} 列，自檢那一節沒印出來——這條測試在空跑：\n{report}",
            rows.len()
        );
        assert!(
            rows.iter().any(|row| row.contains("捲得到")),
            "最長那個標籤不在裡面，這條測試沒量到它要量的東西：\n{report}"
        );
        for row in rows {
            assert!(row.trim_start().contains("  "), "值黏在標籤上了：{row:?}");
        }
    }

    /// 「笑了 0 次」要說得出是哪一種 0。
    ///
    /// 她每兩到五分鐘試一次，試不成也留一則，所以這兩格加起來就是「響了幾
    /// 次」。「笑 0、跳過 0」＝這一輪還沒響過，他測太短；「笑 0、跳過 5」＝
    /// 響了五次、每次都被同一件事擋住，那才是要我改的東西。沒有這一格，兩
    /// 種在報告上是同一句話，而它們要他做的事完全相反。
    #[test]
    fn a_run_with_no_giggles_says_which_kind_of_zero_it_is() {
        let never = book(vec![]);
        let sixth = item(&never, 6);
        assert_eq!(value(&sixth, "笑了幾次"), MeasureValue::Int(0));
        assert_eq!(
            value(&sixth, "時候到了卻跳過"),
            MeasureValue::Int(0),
            "一次都沒響過就是 0，不是「沒這個欄位」"
        );
        assert!(
            sixth.measured.iter().all(|m| m.label != "最常擋下來的"),
            "一次都沒試過就不可以指認兇手：{:?}",
            sixth.measured
        );
        assert_eq!(
            sixth.verdict,
            Verdict::NotSeen,
            "一次都沒響過才是「這一輪沒發生過」"
        );

        /* 順序是挑過的：第一則、最後一則、最多的那一則各是不同的代號。三個
         * 都一樣的話，一支「回傳第一個」或「回傳最後一個」的 `commonest` 照樣
         * 會綠——而那支是錯的。 */
        let blocked = book(vec![
            skipped(1_789_222_001_000, "hidden"),
            skipped(1_789_222_002_000, "typing"),
            skipped(1_789_222_003_000, "typing"),
            skipped(1_789_222_004_000, "paused"),
        ]);
        let sixth = item(&blocked, 6);
        assert_eq!(value(&sixth, "笑了幾次"), MeasureValue::Int(0));
        assert_eq!(value(&sixth, "時候到了卻跳過"), MeasureValue::Int(4));
        assert_eq!(
            value(&sixth, "最常擋下來的"),
            MeasureValue::Blocker(Word::new("typing").expect("typing 過得了 Word")),
            "四次裡兩次是打字，指的要是那一條"
        );
        assert_ne!(sixth.verdict, Verdict::AsAsked, "被擋掉不等於做對了");
        assert_eq!(
            sixth.verdict,
            Verdict::NeverLanded,
            "計時器響過四次，摘要就不可以說「這一輪沒發生過」——底下兩格明明寫著它試了四次"
        );
    }

    /// 那一格要印他讀得懂的話，不是印代號。
    ///
    /// 代號是給型別看的（[`Word`] 只收 `[a-z0-9_-]`），報告是給他看的。
    #[test]
    fn the_report_names_the_blocker_in_words_he_reads() {
        let blocked = book(vec![skipped(1_789_222_001_000, "typing")]);
        let report = reported(&blocked);
        assert!(
            report.contains("你正在打字"),
            "報告要說得出是哪一條擋的：\n{report}"
        );
        assert!(
            !report.contains("typing"),
            "代號漏到報告上了，那一行是給人讀的：\n{report}"
        );
    }

    /// 認不得的代號原樣印出來，不要印「不明」。
    ///
    /// 畫面那半新加一條條件而這一邊忘了跟上的時候，代號至少還說得出是哪一
    /// 條。「不明」會把「我沒跟上」講成「不知道為什麼」——而這一格存在的理
    /// 由正好是要說出為什麼。
    #[test]
    fn a_blocker_this_side_does_not_know_still_names_itself() {
        let blocked = book(vec![skipped(1_789_222_001_000, "some-new-gate")]);
        let report = reported(&blocked);
        assert!(
            report.contains("some-new-gate"),
            "認不得就原樣印，不要吞掉：\n{report}"
        );
        assert!(!report.contains("不明"), "「不明」比代號還沒用：\n{report}");
    }

    /// 擋下來的理由那一格，一樣收不下螢幕上的字。
    ///
    /// 而**次數要照數**：整數夾帶不了東西，把它一起丟掉只會讓「跳過幾次」
    /// 少報，而那正是這一格要回答的問題。
    #[test]
    fn a_blocker_carrying_screen_text_is_counted_but_never_printed() {
        let smuggled = book(vec![
            skipped(1_789_222_001_000, "他正在打字：我的密碼是 hunter2"),
            skipped(1_789_222_002_000, "hidden"),
        ]);
        /* 漏沒漏那一條擺最前面。斷言是短路的：排在後面的話，任何一個先失敗
         * 的斷言都會讓它整條不被執行，而「證明過了」和「沒跑到」在綠燈上長得
         * 一模一樣。 */
        let report = reported(&smuggled);
        assert!(
            !report.contains("hunter2"),
            "螢幕上的字從這一格漏出去了：\n{report}"
        );
        let sixth = item(&smuggled, 6);
        assert_eq!(
            value(&sixth, "時候到了卻跳過"),
            MeasureValue::Int(2),
            "兩次就是兩次，代號不合格不代表那一次沒發生"
        );
        assert_eq!(
            value(&sixth, "最常擋下來的"),
            MeasureValue::Blocker(Word::new("hidden").expect("hidden 過得了 Word")),
            "指名只能從過得了 Word 的那些裡面挑"
        );
    }

    /// 戳 `pokes` 下的一本簿子，已經 `fill` 進快照。
    fn filled(pokes: i64) -> Snapshot {
        let mut book = Notebook::new();
        book.note(started());
        book.note(persona());
        for i in 0..pokes {
            book.note(poke(1_789_222_000_000 + i, true, Some("poke-aiyo")));
        }
        let mut snapshot = tests::snapshot();
        book.fill(&mut snapshot, 1_789_225_320_000);
        snapshot
    }

    /// 「第 N 項」那一行開頭的記號。
    ///
    /// 不比整段字串：記號那一欄補過寬度，寫死空白數的斷言會在下一次調版面時
    /// 紅得莫名其妙，而它要守的其實只是「這一項判成什麼」。
    fn mark_of(report: &str, number: u8) -> String {
        let needle = format!("第 {number} 項");
        let line = report
            .lines()
            .find(|line| line.contains(&needle))
            .unwrap_or_else(|| panic!("報告裡沒有第 {number} 項：\n{report}"));
        line.trim_start()
            .chars()
            .next()
            .expect("那一行不會是空的")
            .to_string()
    }

    #[test]
    fn the_report_says_how_far_back_the_self_check_can_see() {
        let mut book = Notebook::new();
        book.note(started());
        // 動了、沒出聲——第 2 項要印 ✗。
        book.note(poke(1_789_222_100_000, true, None));
        let mut snapshot = tests::snapshot();
        book.fill(&mut snapshot, 1_789_225_320_000);
        let report = render(&snapshot);
        assert!(report.contains("底下只涵蓋 2026-09-12"), "{report}");
        assert_eq!(
            mark_of(&report, 2),
            "✗",
            "只戳一下沒出聲，第 2 項要是 ✗：\n{report}"
        );
        assert_eq!(mark_of(&report, 7), "？", "{report}");
        assert_eq!(
            mark_of(&report, 5),
            "－",
            "沒問過就沒有氣泡可量：\n{report}"
        );
    }

    /// 每一行的「第 N 項」都從同一格開始。
    ///
    /// `✓` 是一格，`－？！…` 是兩格（全形）。記號不補寬度的話，同一份表裡
    /// 的項次會左右各差一格——而那一欄正是他用來掃的那一欄。
    #[test]
    fn every_self_check_row_starts_its_number_at_the_same_column() {
        let mut book = Notebook::new();
        book.note(started());
        book.note(poke(1_789_222_100_000, true, Some("poke-aiyo")));
        book.note(poke(1_789_222_101_000, true, Some("poke-fan")));
        let mut snapshot = tests::snapshot();
        book.fill(&mut snapshot, 1_789_225_320_000);
        let report = render(&snapshot);

        let rows: Vec<&str> = report
            .lines()
            .filter(|line| line.contains(" 項　"))
            .collect();
        assert_eq!(rows.len(), 7, "七件事七行：\n{report}");

        // 空過的話這條測試等於沒跑：一格寬和兩格寬的記號都要真的出現。
        let widths: Vec<usize> = rows
            .iter()
            .map(|line| cells(&line.trim_start().chars().next().unwrap().to_string()))
            .collect();
        assert!(
            widths.contains(&1) && widths.contains(&2),
            "這份夾具沒同時產出窄記號和寬記號，對齊這件事等於沒測：\n{report}"
        );

        let columns: Vec<usize> = rows
            .iter()
            .map(|line| cells(&line[..line.find("第 ").expect("這一行有「第 」")]))
            .collect();
        assert!(
            columns.iter().all(|column| *column == columns[0]),
            "「第 N 項」沒有對齊，各在第 {columns:?} 格：\n{report}"
        );
    }
}
