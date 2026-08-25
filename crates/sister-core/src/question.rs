//! 他問的到底是「哪幾個字」，還是「剛剛」。
//!
//! 這個模組存在的理由是一張截圖。字母人第一次跑在真的 Windows 上，第一個被
//! 打進輸入框的問題是**「剛剛發生什麼事」**——而那是這整個產品的招牌問題。
//! 當時的引擎只會做一件事：拿那七個字去比對她記下來的字。於是唯一可能的答案
//! 是「這件事我沒看到過」，除非螢幕上剛好出現過「剛剛發生什麼事」這七個字。
//!
//! 那不是一個 bug，是一個形狀錯誤：「剛剛」是**時間**，不是關鍵字。用比對字
//! 的方式去回答一個問時間的問題，永遠只會答錯，而且會錯得像「她什麼都沒記到」。
//!
//! 判斷刻意做得很膽小：**看不懂就當成關鍵字。** 把關鍵字問題誤判成時間問題，
//! 代價是她無視他打的字、改講最近發生的事——那是一個會讓人不再信任她的錯。
//! 反過來把時間問題當成關鍵字，最差也只是回到今天的行為。
//!
//! ## 那張截圖的下一題
//!
//! 「剛剛發生什麼事」修好之後，下一句自然是**「剛剛那個優惠方案」**——同時
//! 帶著時間和內容。這種問題走的是關鍵字那條路（句子裡有他真正想問的東西），
//! 而那條路上藏著同一個形狀錯誤：拿去比對的是**整句話**。中文沒有空白，
//! 所以 FTS 那邊會把「剛剛那個優惠方案」當成一整串子字串去找，而沒有人的
//! 螢幕上會出現這八個字。實測（`scenarios/bill-lookup.json`）：
//!
//! | 問題 | 修之前 |
//! |---|---|
//! | 優惠方案 | 1 筆原文 |
//! | 剛剛那個優惠方案 | **0 筆** |
//! | 繳費期限 | 3 筆答案 + 1 筆原文 |
//! | 剛剛看到的期限 | 2 筆答案 + **0 筆原文** |
//!
//! [`terms`] 就是那個修法：**把頭尾的時間詞和虛字剝掉，中間原樣留著。**
//! 只剝頭尾是刻意的——中文問句把時間放句首（「剛剛…」）、把疑問詞放句尾
//! （「…是什麼」），內容夾在中間而且是連著的，剝完還是一段完整的子字串。
//! 如果連中間的虛字也一起挑掉，「剛剛 chrome 上那個網址」會碎成
//! `chrome` AND `上` AND `網址` 三段，比原本更找不到。

use chrono::{Datelike, Local, LocalResult, NaiveDate, TimeZone};

use crate::model::Millis;

/// 一個問題該用什麼方式回答。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// 拿字去比對她記下來的字。
    Keywords,
    /// 他問的是「最近」。答案來自時間，不是來自比對。
    Recent,
    /// 他問的是一段日曆時間（「昨天下午」），剩下的字不夠當關鍵字。
    /// 答案是那段時間裡的紀錄，不是拿「弄」去比對螢幕。
    Range,
}

impl Shape {
    /// 寫出去的時候叫什麼。
    ///
    /// 只有這一個地方定義：`sister query --json` 的 `shape` 欄位、字母人的
    /// `Answer.kind`、題庫的 `queries.shape` 是同一組字串。各寫各的 `match`
    /// 遲早會有一邊改了字，而分岔之後，Phase 2 拿題庫回頭統計「時間問題到底
    /// 答得好不好」的時候會靜靜地少算一半。
    pub fn name(self) -> &'static str {
        match self {
            Shape::Keywords => "keywords",
            Shape::Recent => "recent",
            Shape::Range => "range",
        }
    }
}

/// 「剛剛」。看到其中一個才**可能**是時間問題。
const RECENT_CJK: &[&str] = &["剛剛", "剛才", "方才", "最近", "剛"];
const RECENT_ASCII: &[&str] = &["just", "recently", "lately", "now"];

/// 這些字自己不帶內容，把它們拿掉不會改變他在問什麼。
///
/// 長的排前面沒關係，比對時會自己按長度排。要小心的是**別把有內容的字放進來**
/// ——每多一個，就多一種「他明明問了某件事，她卻改講最近」的可能。
const FILLER_CJK: &[&str] = &[
    "發生", "什麼", "甚麼", "事情", "幹嘛", "幹麼", "一下", "剛好", "我", "你", "她", "他", "在",
    "做", "了", "過", "的", "是", "有", "嗎", "呢", "吧", "啊", "喔", "事", "看", "到", "些", "那",
    "這", "耶", "呀", "個",
];
const FILLER_ASCII: &[&str] = &[
    "what", "happened", "happen", "was", "were", "i", "im", "doing", "did", "do", "the", "a", "on",
    "up", "to", "am", "you", "see", "saw", "screen", "my", "me", "s", "here", "going", "went",
    "been",
];

/// 一個詞在這句話裡扮演什麼角色。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    /// 「剛剛」。指向時間。
    Recent,
    /// 「昨天」「下午」。日曆範圍，已經被 [`time_range`] 用掉，不該再進 FTS。
    Calendar,
    /// 虛字。拿掉不改變他在問什麼。
    Filler,
    /// 認不得——那多半就是他真正想問的東西。
    Content,
}

/// 把問題切成一個一個詞，標點和空白直接丟掉。
///
/// [`shape`] 和 [`terms`] 都走這一支。兩邊各切一次的話，遲早會有一邊多認得
/// 一個詞，然後同一句話在「要不要當成時間問題」和「拿哪幾個字去比對」上給出
/// 對不起來的答案。這根釘子這個 repo 已經踩過五次了（見 `crate::consent`）。
fn words(question: &str) -> Vec<(usize, usize, Role)> {
    let mut out = Vec::new();
    let mut i = 0;

    while i < question.len() {
        let rest = &question[i..];
        let c = rest.chars().next().expect("not empty");

        // 標點和空白直接跳過。「剛剛發生什麼事？」和沒有問號的那句是同一句。
        if c.is_whitespace() || (c.is_ascii_punctuation() || is_cjk_punct(c)) {
            i += c.len_utf8();
            continue;
        }

        // 英數字整個詞一起看，否則 `justify` 會被讀成 `just`——那是一個把
        // 「查一個變數名」變成「講最近發生的事」的誤判。
        if c.is_ascii_alphanumeric() {
            let end = rest
                .find(|ch: char| !ch.is_ascii_alphanumeric())
                .unwrap_or(rest.len());
            let word = rest[..end].to_ascii_lowercase();
            let role = if RECENT_ASCII.contains(&word.as_str()) {
                Role::Recent
            } else if FILLER_ASCII.contains(&word.as_str()) {
                Role::Filler
            } else {
                Role::Content
            };
            out.push((i, i + end, role));
            i += end;
            continue;
        }

        // 中文：由長到短試，「剛剛」要贏過「剛」，「昨天下午」要贏過單字虛字。
        match longest(rest) {
            Some((len, role)) => {
                out.push((i, i + len, role));
                i += len;
            }
            // 認不得的中文一個字一個字往前走。連著的幾個字在 `terms` 那邊會
            // 自己接回一段——中文的內容詞本來就是連著的。
            None => {
                out.push((i, i + c.len_utf8(), Role::Content));
                i += c.len_utf8();
            }
        }
    }

    out
}

/// 他在問什麼形狀的問題。
pub fn shape(question: &str) -> Shape {
    let words = words(question);
    let content_chars = content_span_chars(question, &words);
    // 剩下兩個字以上的內容，走關鍵字——誤判的代價不對稱，見模組開頭。
    // 「我昨天下午在弄什麼」只剩一個「弄」，那不是關鍵字，是在問那段時間。
    if content_chars >= 2 {
        return Shape::Keywords;
    }
    if scan_time_words(question).is_some() {
        return Shape::Range;
    }
    if words.iter().any(|&(_, _, r)| r == Role::Recent) {
        Shape::Recent
    } else {
        Shape::Keywords
    }
}

fn content_span_chars(question: &str, words: &[(usize, usize, Role)]) -> usize {
    let Some(lo) = words.iter().position(|&(_, _, r)| r == Role::Content) else {
        return 0;
    };
    let hi = words
        .iter()
        .rposition(|&(_, _, r)| r == Role::Content)
        .expect("有第一個就有最後一個");
    question[words[lo].0..words[hi].1].chars().count()
}

/// 這句話裡真正該拿去比對螢幕的是哪一段：**從第一個內容詞到最後一個內容詞。**
///
/// 頭尾的時間詞和虛字被剝掉，中間原樣留著（含中間的虛字和標點）——理由見模組
/// 開頭那一節。回傳的是原字串的一段，不是重組出來的新字串：重組會在詞與詞之間
/// 塞進空白，而空白正是 [`crate::db::fts_query`] 用來斷詞的東西，等於把一段
/// 連續的中文切成好幾個必須同時出現的條件。
///
/// 一個內容詞都不剩的時候回**原句**，不是空字串。那多半是「發生什麼事」這種
/// 整句都是虛字的問法，而她能做的最不意外的事就是照著他打的字去找——和這個
/// 模組其他地方一樣，看不懂就退回今天的行為。
///
/// ## 剝到只剩一個中文字，就是剝過頭了
///
/// 虛字清單裡有一堆**單字**（「事」「看」「到」「個」「有」），而中文的雙字詞
/// 常常正好由一個虛字加一個實字組成。於是「事件」會被剝成「件」、「看板」剝成
/// 「板」、「個資」剝成「資」——每一個都是把他問的東西換成另一個東西。
///
/// 而且那不只是精確度變差：一個中文字在這個 schema 底下**沒有索引可用**
/// （trigram 要 3 個字、bigram 要 2 個字，見 [`crate::db`] 開頭），所以它會
/// 一路掉到全表掃描，然後掃回一堆不相干的東西。一個字的查詢不是查詢，是掃描。
///
/// 所以剝完不足兩個字的時候，把邊界往回退——**先退左邊**：中文的修飾語在前、
/// 中心語在後，緊鄰左邊那個字比較可能和它是同一個詞。「剛剛那個事件」會停在
/// 「事件」，不會退回整句。
pub fn terms(question: &str) -> &str {
    terms_with_retreat(question).0
}

/// 同上，外加**這次有沒有退過邊界**。
///
/// 退邊界是這個函式唯一一種「她拿去比對的字不是一個詞」的來源。上面那段文件
/// 講的是它救回「事件」「看板」「個資」的那一面；它的另一面是同一個動作也會
/// 黏出不是詞的東西：
///
/// ```text
/// 剛剛那個板     → 個板
/// 剛剛看到的人   → 的人
/// 剛剛那個錢     → 個錢
/// ```
///
/// 光看字串分不出這兩種——「個資」和「個板」長得一樣，要有字典才知道前者是
/// 詞。但**退過**這件事本身是知道的。兩種完全不同的處境本來會印出同一句
/// 「沒有找到」：他打的字真的沒出現過，跟她根本沒找他打的字——而後者他只要
/// 把那個詞重打一次就好。
///
/// **只講「退過沒有」是不夠的，而 alpha.40 為止這裡回的就是它。** 任何一個
/// 開頭落在虛字表裡的兩字詞——個資、事件、看板、個人、到期、我們、過期、
/// 是否、了解、事故——`lo` 都會一路退回 0，而退到 0 之後那個 span **就是整
/// 句話**。於是她對著他原封不動打進來的字說：
///
/// ```text
/// 我拿去比對的是「個資」——那是從你打的字黏出來的，不是一個詞。
/// 直接打你要的那個詞再問一次。
/// ```
///
/// 兩句話三個錯：那就是他打的字、那就是一個詞、而「重打一次」是一個空操作
/// （逐字相同的輸出）。它連**答得出來**的那幾次也一起喊，於是把對的結果也
/// 一起抹黑了。
///
/// 所以第二個回傳值是「退過**而且**退出來的東西跟他打的不一樣」。退回原句就
/// 沒有什麼好講的：她比對的正是他打的那幾個字。判斷寫在這裡而不是呼叫端，
/// 因為呼叫端有兩個（`ops.rs` 的 `glued_note`、字母人的 `searched`），而
/// 「同一個判斷散在兩個 process 裡」是這個 repo 修過很多次的那一種。
///
/// **守不住的那一半**：`有效期限` 會被剝成 `效期限`（`有` 在虛字表裡），而
/// 那一次**沒有退過**，所以這裡回 false、不出聲。它確實也是「她找的不是他打
/// 的字」，只是原因在斷詞表而不在退格。要接住它得先有字典分得出「有效期限」
/// 是一個詞——在那之前，寧可少講一句，也不要對著正確的輸入喊錯。
pub fn terms_with_retreat(question: &str) -> (&str, bool) {
    /// 剝完至少要留下這麼多個字，不然就退回去。理由見上面。
    const MIN_CHARS: usize = 2;

    let words = words(question);
    let Some(mut lo) = words.iter().position(|&(_, _, r)| r == Role::Content) else {
        return (question.trim(), false);
    };
    let mut hi = words
        .iter()
        .rposition(|&(_, _, r)| r == Role::Content)
        .expect("有第一個就有最後一個");

    let span = |lo: usize, hi: usize| &question[words[lo].0..words[hi].1];
    let mut retreated = false;
    while span(lo, hi).chars().count() < MIN_CHARS {
        if lo > 0 {
            lo -= 1;
            retreated = true;
        } else if hi + 1 < words.len() {
            hi += 1;
            retreated = true;
        } else {
            break; // 整句就這麼短，沒有東西可以退了
        }
    }
    let terms = span(lo, hi);
    // 退回原句就等於沒有退。這一行是那三句假話唯一的閘門，而它必須留在這裡
    // ——兩個呼叫端都只寫 `glued.then(...)`，判斷一搬出去就會有一邊漏掉。
    (terms, retreated && terms != question.trim())
}

/// 在開頭比對得到的最長那個詞，以及它的角色。
fn longest(rest: &str) -> Option<(usize, Role)> {
    let mut best: Option<(usize, Role)> = None;
    let mut consider = |token: &str, role: Role| {
        if rest.starts_with(token) && best.is_none_or(|(len, _)| token.len() > len) {
            best = Some((token.len(), role));
        }
    };
    for (token, _) in DAY_TOKENS {
        consider(token, Role::Calendar);
    }
    for (token, _) in PERIOD_TOKENS {
        consider(token, Role::Calendar);
    }
    for token in RECENT_CJK {
        consider(token, Role::Recent);
    }
    for token in FILLER_CJK {
        consider(token, Role::Filler);
    }
    best
}

fn is_cjk_punct(c: char) -> bool {
    matches!(
        c,
        '，' | '。' | '？' | '！' | '、' | '；' | '：' | '「' | '」' | '（' | '）' | '…' | '　'
    )
}

/// 問句裡認得出來的一段日曆時間。和 [`Shape`] 正交：同一句話可以同時是
/// 關鍵字問題、又帶著「昨天下午」。
///
/// `from`／`to` 是半開區間 `[from, to)`，毫秒。認不出來時呼叫端拿到的是
/// [`None`]，不是一個 `from == to` 的空範圍——那兩種在畫面上會變成同一句
/// 「那段時間沒有章節」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimeRange {
    pub from: Millis,
    pub to: Millis,
    /// 他原話裡的那一段，例如「昨天下午」。回述必須是他講過的字。
    pub said: String,
}

/// 日界與時段用**這台機器的本地時區**（和終端機印出處的時鐘同一套）。
/// `now` 是 unix millis；同一個數字在不同時區的機器上會解出不同的日界——
/// 那是對的，因為「昨天」是本地日曆，不是 UTC 日曆。函式裡不呼叫
/// [`crate::now_ms`]：同一句話在不同時刻會有不同答案，那個時刻必須是呼叫
/// 端給的，測試才寫得出來。
///
/// 時段（半開，本地時鐘；這組邊界是實作選擇，不是規格常數）：
///
/// | 詞 | 起 | 迄 |
/// |---|---|---|
/// | 凌晨 | 00:00 | 06:00 |
/// | 早上／上午 | 06:00 | 12:00 |
/// | 中午 | 11:00 | 14:00 |
/// | 下午 | 12:00 | 18:00 |
/// | 傍晚 | 17:00 | 19:00 |
/// | 晚上 | 18:00 | 24:00 |
///
/// 「這禮拜／上禮拜」從週一 00:00 起算（ISO 週）。時段只接到今天／昨天／
/// 前天；接到禮拜上時那段時段詞不採用——一個禮拜的「下午」不是一個區間。
///
/// 認不出來、或本地日曆解不出來，回 [`None`]。不預設今天，不回 `0..0`。
pub fn time_range(question: &str, now: Millis) -> Option<TimeRange> {
    let found = scan_time_words(question)?;
    let now_dt = Local.timestamp_millis_opt(now).single()?;
    let today = now_dt.date_naive();

    let (from_date, to_date, used_period) = match found.day {
        DayKind::Today => (today, next_day(today)?, found.period.is_some()),
        DayKind::Yesterday => {
            let y = shift_days(today, -1)?;
            (y, today, found.period.is_some())
        }
        DayKind::DayBefore => {
            let d = shift_days(today, -2)?;
            let y = shift_days(today, -1)?;
            (d, y, found.period.is_some())
        }
        DayKind::ThisWeek => {
            let mon = monday_on_or_before(today)?;
            (mon, shift_days(mon, 7)?, false)
        }
        DayKind::LastWeek => {
            let mon = monday_on_or_before(today)?;
            (shift_days(mon, -7)?, mon, false)
        }
    };

    let (from, to) = if used_period {
        let (h0, h1) = found.period?.hours();
        (at_hour(from_date, h0)?, at_hour(from_date, h1)?)
    } else {
        (at_hour(from_date, 0)?, at_hour(to_date, 0)?)
    };
    if from >= to {
        return None;
    }

    let (said_lo, said_hi) = if used_period {
        let (p0, p1) = found.period_span?;
        (found.day_span.0.min(p0), found.day_span.1.max(p1))
    } else {
        found.day_span
    };
    let said = question.get(said_lo..said_hi)?.to_string();
    if said.is_empty() {
        return None;
    }
    Some(TimeRange { from, to, said })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DayKind {
    Today,
    Yesterday,
    DayBefore,
    ThisWeek,
    LastWeek,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Period {
    Predawn,
    Morning,
    Noon,
    Afternoon,
    Evening,
    Night,
}

impl Period {
    /// 起迄小時，迄可以是 24（當天結束 = 次日 00:00）。
    fn hours(self) -> (u32, u32) {
        match self {
            Period::Predawn => (0, 6),
            Period::Morning => (6, 12),
            Period::Noon => (11, 14),
            Period::Afternoon => (12, 18),
            Period::Evening => (17, 19),
            Period::Night => (18, 24),
        }
    }
}

const DAY_TOKENS: &[(&str, DayKind)] = &[
    ("這個禮拜", DayKind::ThisWeek),
    ("上個禮拜", DayKind::LastWeek),
    ("這禮拜", DayKind::ThisWeek),
    ("上禮拜", DayKind::LastWeek),
    ("這週", DayKind::ThisWeek),
    ("上週", DayKind::LastWeek),
    ("今天", DayKind::Today),
    ("昨天", DayKind::Yesterday),
    ("前天", DayKind::DayBefore),
];

const PERIOD_TOKENS: &[(&str, Period)] = &[
    ("早上", Period::Morning),
    ("上午", Period::Morning),
    ("中午", Period::Noon),
    ("下午", Period::Afternoon),
    ("傍晚", Period::Evening),
    ("晚上", Period::Night),
    ("凌晨", Period::Predawn),
];

struct TimeWords {
    day: DayKind,
    day_span: (usize, usize),
    period: Option<Period>,
    period_span: Option<(usize, usize)>,
}

/// 第一個日詞、可選的第一個時段詞。沒有日詞就不是時間範圍。
fn scan_time_words(question: &str) -> Option<TimeWords> {
    let mut day = None;
    let mut period = None;
    let mut i = 0;
    while i < question.len() {
        let rest = &question[i..];
        let c = rest.chars().next()?;
        if c.is_whitespace() || c.is_ascii_punctuation() || is_cjk_punct(c) {
            i += c.len_utf8();
            continue;
        }
        let day_hit = longest_in(rest, DAY_TOKENS);
        let period_hit = longest_in(rest, PERIOD_TOKENS);
        if let Some((len, kind)) =
            day_hit.filter(|&(len, _)| period_hit.is_none_or(|(plen, _)| len >= plen))
        {
            if day.is_none() {
                day = Some((kind, (i, i + len)));
            }
            i += len;
        } else if let Some((len, kind)) = period_hit {
            if period.is_none() {
                period = Some((kind, (i, i + len)));
            }
            i += len;
        } else {
            i += c.len_utf8();
        }
    }
    let (kind, span) = day?;
    match period {
        Some((p, pspan)) => Some(TimeWords {
            day: kind,
            day_span: span,
            period: Some(p),
            period_span: Some(pspan),
        }),
        None => Some(TimeWords {
            day: kind,
            day_span: span,
            period: None,
            period_span: None,
        }),
    }
}

fn longest_in<T: Copy>(rest: &str, table: &[(&str, T)]) -> Option<(usize, T)> {
    let mut best = None;
    for (tok, val) in table {
        if rest.starts_with(tok) && best.is_none_or(|(len, _)| tok.len() > len) {
            best = Some((tok.len(), *val));
        }
    }
    best
}

fn shift_days(date: NaiveDate, n: i64) -> Option<NaiveDate> {
    date.checked_add_signed(chrono::TimeDelta::days(n))
}

fn next_day(date: NaiveDate) -> Option<NaiveDate> {
    shift_days(date, 1)
}

fn monday_on_or_before(date: NaiveDate) -> Option<NaiveDate> {
    shift_days(date, -(date.weekday().num_days_from_monday() as i64))
}

fn at_hour(date: NaiveDate, hour: u32) -> Option<Millis> {
    let (date, hour) = if hour >= 24 {
        (next_day(date)?, hour - 24)
    } else {
        (date, hour)
    };
    let naive = date.and_hms_opt(hour, 0, 0)?;
    match Local.from_local_datetime(&naive) {
        LocalResult::Single(dt) | LocalResult::Ambiguous(dt, _) => Some(dt.timestamp_millis()),
        LocalResult::None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 這一句就是那張截圖。它非過不可。
    #[test]
    fn the_question_from_the_screenshot() {
        assert_eq!(shape("剛剛發生什麼事"), Shape::Recent);
    }

    #[test]
    fn the_ways_people_actually_ask_about_just_now() {
        for q in [
            "剛剛發生什麼事",
            "剛剛發生什麼事？",
            "我剛剛在做什麼",
            "剛剛在幹嘛",
            "剛才我看到什麼",
            "最近發生什麼事",
            "剛剛做了什麼",
            "what just happened",
            "what did i just do",
            "what was i doing just now",
        ] {
            assert_eq!(shape(q), Shape::Recent, "{q:?} 問的是時間");
        }
    }

    /// 誤判的代價不對稱：把有內容的問題當成「剛剛」，她會無視他打的字。
    /// 所以只要句子裡還剩下任何一個講得出內容的詞，就走關鍵字。
    #[test]
    fn a_question_with_anything_left_in_it_is_still_a_search() {
        for q in [
            "剛剛那個電話號碼",
            "剛剛的錯誤訊息",
            "最近的帳單金額",
            "剛剛 chrome 上那個網址",
            "客服",
            "what just happened to the deploy",
            "ERR_CONNECTION_REFUSED",
        ] {
            assert_eq!(shape(q), Shape::Keywords, "{q:?} 有他真正想問的東西");
        }
    }

    /// 沒有「剛剛」就不是時間問題，就算整句都是虛字也一樣。
    #[test]
    fn filler_alone_is_not_a_time_question() {
        for q in ["什麼", "發生什麼事", "我在做什麼", "what happened", ""] {
            assert_eq!(shape(q), Shape::Keywords, "{q:?} 沒有指向時間");
        }
    }

    /// `just` 不能從 `justify` 裡被讀出來。整個詞比對，不是子字串。
    #[test]
    fn an_english_word_is_matched_whole_or_not_at_all() {
        assert_eq!(shape("justify"), Shape::Keywords);
        assert_eq!(shape("nowhere"), Shape::Keywords);
        assert_eq!(shape("recently"), Shape::Recent);
    }

    /// 「剛剛」要贏過「剛」，否則剩下的那個「剛」會變成認不得的字。
    #[test]
    fn the_longer_token_wins() {
        assert_eq!(shape("剛剛"), Shape::Recent);
        assert_eq!(shape("剛才"), Shape::Recent);
    }

    /// 這四句是實測出來的（`scenarios/bill-lookup.json`，見模組開頭那張表）。
    /// 右邊那幾個字單獨拿去問答得出來，加上「剛剛」之後就變成零筆。
    #[test]
    fn the_time_word_must_not_end_up_in_what_she_looks_for_on_screen() {
        assert_eq!(terms("剛剛那個優惠方案"), "優惠方案");
        assert_eq!(terms("剛剛看到的期限"), "期限");
        assert_eq!(terms("最近的帳單金額"), "帳單金額");
        assert_eq!(terms("what just happened to the deploy"), "deploy");
    }

    /// 只剝頭尾。中間那些虛字留著，因為剝掉會把一段連續的中文切成好幾個
    /// 必須同時出現的條件——理由見 [`terms`] 的註解。
    #[test]
    fn the_middle_is_left_exactly_as_he_typed_it() {
        assert_eq!(terms("剛剛 chrome 上那個網址"), "chrome 上那個網址");
        assert_eq!(terms("錯誤訊息，還有那個網址"), "錯誤訊息，還有那個網址");
    }

    /// 虛字清單裡有一堆單字，而中文的雙字詞常常正好是一個虛字加一個實字。
    /// 照字面剝的話「事件」會變成「件」——那不只是精確度差一點，一個中文字
    /// 在這個 schema 底下沒有索引可用，會一路掉到全表掃描再撈回一堆不相干
    /// 的東西。剝到不足兩個字就往回退。
    #[test]
    fn stripping_must_not_eat_half_of_a_two_character_word() {
        assert_eq!(terms("事件"), "事件");
        assert_eq!(terms("看板"), "看板");
        assert_eq!(terms("個資"), "個資");
        // 退到夠用就停，不是退回整句——「剛剛」還是不該進去比對。
        assert_eq!(terms("剛剛那個事件"), "事件");
        assert_eq!(terms("剛剛的事件"), "事件");
    }

    /// 整句就只有一個字的時候沒有東西可以退，照原樣送出去。
    #[test]
    fn a_one_character_question_is_left_alone() {
        assert_eq!(terms("板"), "板");
        assert_eq!(terms("的"), "的");
    }

    /// **退邊界救回「事件」的同一個動作，也黏得出「個板」。**
    ///
    /// 光看字串分不出來（「個資」是詞、「個板」不是），但退過這件事本身知道
    /// ——而它正好圈住全部的風險：沒退過就代表兩端都是實字，她比對的就是他
    /// 打的那幾個字。畫面只在退過的時候多講一句。
    #[test]
    fn a_needle_glued_out_of_a_particle_announces_itself() {
        for (q, needle) in [
            ("剛剛那個板", "個板"),
            ("剛剛看到的人", "的人"),
            ("剛剛那個錢", "個錢"),
            // 剝掉前面那一截、退回來救到一個實字：她找的仍然不是他打的整句。
            ("剛剛那個事件", "事件"),
        ] {
            let (t, retreated) = terms_with_retreat(q);
            assert_eq!(t, needle, "{q}");
            assert!(retreated, "{q} → {t} 是黏出來的，要說得出口");
        }

        // 沒退過的一律安靜：兩端都是實字，她找的就是他打的。
        for q in [
            "剛剛那個優惠方案",
            "客服專線",
            "ERR_CONNECTION_REFUSED",
            "板",
            "剛剛發生什麼事",
        ] {
            assert!(!terms_with_retreat(q).1, "{q} 沒有黏過東西，不該多講一句");
        }
    }

    /// 他原封不動打進來的字，不可以被說成「黏出來的，不是一個詞」。
    ///
    /// **上一版這一條是反著寫的**：`("個資", "個資")` 躺在上面那個「要說得出
    /// 口」的清單裡，配一句「救回來的那幾個也一樣算退過——她確實不是照他打的
    /// 字去找的」。那句註解對 `個資` 是假的：`t == q`，她找的**就是**他打的字。
    ///
    /// 開頭落在虛字表裡的兩字詞，`lo` 會一路退回 0，而退到 0 之後 span 就是
    /// 整句話。於是每一個這種詞都會拿到三句假話（那是黏出來的／不是一個詞／
    /// 重打一次再問——一個逐字相同的空操作），而且**答得出來的時候也會喊**，
    /// 順手把對的結果一起抹黑。
    #[test]
    fn what_he_typed_verbatim_is_never_called_glued() {
        for q in [
            "個資", "事件", "看板", "個人", "到期", "我們", "有效", "過期", "是否", "這裡", "那邊",
            "了解", "事故",
        ] {
            let (t, glued) = terms_with_retreat(q);
            assert_eq!(t, q, "{q} 一個字都沒被拿掉");
            assert!(!glued, "「{q}」就是他打的字，不可以說成黏出來的");
        }
    }

    /// 一句完全沒有虛字的問題不該被動到一個字。
    #[test]
    fn a_question_made_of_nothing_but_content_comes_back_whole() {
        for q in ["客服專線", "ERR_CONNECTION_REFUSED", "E0308"] {
            assert_eq!(terms(q), q);
        }
    }

    /// 剝到什麼都不剩的時候回原句，不是空字串：她退回今天的行為，照著他打的
    /// 字去找。（`Shape::Recent` 那幾句根本不會走到比對那條路，這裡驗的是
    /// 「發生什麼事」這種沒有時間詞、也沒有內容詞的問法。）
    #[test]
    fn stripping_everything_falls_back_to_what_he_typed() {
        assert_eq!(terms("發生什麼事"), "發生什麼事");
        assert_eq!(terms("剛剛發生什麼事"), "剛剛發生什麼事");
        assert_eq!(terms(""), "");
        assert_eq!(terms("   "), "");
    }

    /// 尾巴的標點跟著虛字一起走，開頭的也是。
    #[test]
    fn punctuation_at_the_edges_does_not_survive() {
        assert_eq!(terms("剛剛那個優惠方案？"), "優惠方案");
        assert_eq!(terms("「客服專線」"), "客服專線");
    }

    /// **剝出來的一定是他打的那句話裡的一段連續子字串。**
    ///
    /// 這一條是整個做法安不安全的關鍵，值得寫成一條測試而不是只寫在註解裡。
    /// FTS 對中文做的是子字串比對，所以 `terms(q)` ⊆ `q` 直接推出：原本比對
    /// 得到的東西，剝完之後一樣比對得到。**剝字只會讓精確度變差（多撈一些
    /// 不相干的），不會讓她找不到本來找得到的東西。**
    ///
    /// 反過來說，哪天有人想在這裡「重組」出更好的查詢字串（換同義詞、補空白、
    /// 加 OR），這條測試會先紅——而那正是該停下來想清楚的時候，因為那一步會
    /// 把「不會變差」這個保證換成一個需要被評測的猜測。
    #[test]
    fn what_she_looks_for_is_always_a_slice_of_what_he_typed() {
        for q in [
            "剛剛那個網址",
            "我剛剛複製的東西",
            "那個錯誤代碼是什麼",
            "上禮拜的會議記錄",
            "我的密碼放哪",
            "剛剛看到的那個人名",
            "有沒有優惠",
            "這個月的帳單",
            "剛剛那個 error",
            "那個 pull request",
            "剛才那封信",
            "身分證字號",
            "剛剛那個 IP",
            "事件",
            "ERR_CONNECTION_REFUSED",
            "",
            "   ",
            "？？？",
        ] {
            let t = terms(q);
            assert!(
                q.contains(t),
                "{q:?} 剝出了一段它自己沒有的字：{t:?}——比對就可能反而變少"
            );
        }
    }

    fn at(year: i32, month: u32, day: u32, hour: u32, min: u32) -> Millis {
        Local
            .with_ymd_and_hms(year, month, day, hour, min, 0)
            .single()
            .expect("valid local time")
            .timestamp_millis()
    }

    fn hour_on(year: i32, month: u32, day: u32, hour: u32) -> Millis {
        at(year, month, day, hour, 0)
    }

    /// 招牌那句。認得出「昨天下午」，而且 `said` 是他打的那幾個字。
    #[test]
    fn yesterday_afternoon_is_a_clock_range_not_a_shape() {
        // 2026-08-26 是週三。`now` 定在當天下午，免得測到「今天還沒到下午」。
        let now = at(2026, 8, 26, 15, 30);
        let r = time_range("我昨天下午在弄什麼", now).expect("認得出昨天下午");
        assert_eq!(r.said, "昨天下午");
        assert_eq!(r.from, hour_on(2026, 8, 25, 12));
        assert_eq!(r.to, hour_on(2026, 8, 25, 18));
        assert!(r.from < r.to);
        // 「弄」一個字不夠當關鍵字；這句話問的是那段時間。
        assert_eq!(shape("我昨天下午在弄什麼"), Shape::Range);
        assert_eq!(terms("我昨天下午在弄什麼"), "在弄");
        assert_eq!(terms("昨天下午的週報"), "週報");
        assert_eq!(terms("昨天的電話"), "電話");
        assert_eq!(shape("昨天下午的週報"), Shape::Keywords);
        assert_eq!(shape("昨天的電話"), Shape::Keywords);
    }

    /// 時間詞已經被 `time_range` 用掉，不准再拿去比對螢幕。
    #[test]
    fn calendar_words_are_stripped_from_what_she_looks_for() {
        assert_eq!(terms("上禮拜的會議記錄"), "會議記錄");
        assert_eq!(terms("昨天看到的期限"), "期限");
        // 退回邏輯還在：只剩一個實字就往左黏，不要把時間詞黏回來。
        assert_eq!(terms("昨天那個事件"), "事件");
        let (t, retreated) = terms_with_retreat("我昨天下午在弄什麼");
        assert_eq!(t, "在弄");
        assert!(retreated, "「弄」不足兩字，退過而且不是原句");
    }

    #[test]
    fn the_phrases_people_actually_use() {
        let now = at(2026, 8, 26, 15, 30);
        let hit = |q| time_range(q, now).expect(q);
        assert_eq!(hit("今天早上").said, "今天早上");
        assert_eq!(hit("今天早上").from, hour_on(2026, 8, 26, 6));
        assert_eq!(hit("今天早上").to, hour_on(2026, 8, 26, 12));
        assert_eq!(hit("今天上午").from, hour_on(2026, 8, 26, 6));
        assert_eq!(hit("前天").said, "前天");
        assert_eq!(hit("前天").from, hour_on(2026, 8, 24, 0));
        assert_eq!(hit("前天").to, hour_on(2026, 8, 25, 0));
        assert_eq!(hit("昨天").said, "昨天");
        assert_eq!(hit("昨天").from, hour_on(2026, 8, 25, 0));
        assert_eq!(hit("昨天").to, hour_on(2026, 8, 26, 0));
        assert_eq!(hit("今天凌晨").from, hour_on(2026, 8, 26, 0));
        assert_eq!(hit("今天凌晨").to, hour_on(2026, 8, 26, 6));
        assert_eq!(hit("昨天中午").from, hour_on(2026, 8, 25, 11));
        assert_eq!(hit("昨天中午").to, hour_on(2026, 8, 25, 14));
        assert_eq!(hit("昨天傍晚").from, hour_on(2026, 8, 25, 17));
        assert_eq!(hit("昨天傍晚").to, hour_on(2026, 8, 25, 19));
        assert_eq!(hit("昨天晚上").from, hour_on(2026, 8, 25, 18));
        assert_eq!(hit("昨天晚上").to, hour_on(2026, 8, 26, 0));
    }

    /// 2026-08-26 是週三，這禮拜從週一 24 日起。
    #[test]
    fn a_week_starts_monday() {
        let now = at(2026, 8, 26, 15, 30);
        let this = time_range("這禮拜", now).expect("這禮拜");
        assert_eq!(this.said, "這禮拜");
        assert_eq!(this.from, hour_on(2026, 8, 24, 0));
        assert_eq!(this.to, hour_on(2026, 8, 31, 0));
        let last = time_range("上禮拜", now).expect("上禮拜");
        assert_eq!(last.from, hour_on(2026, 8, 17, 0));
        assert_eq!(last.to, hour_on(2026, 8, 24, 0));
        // 較長的寫法：said 是他打的那一串，不是收成「這禮拜」。
        assert_eq!(time_range("這個禮拜", now).unwrap().said, "這個禮拜");
        assert_eq!(
            time_range("這個禮拜", now).unwrap().from,
            time_range("這禮拜", now).unwrap().from
        );
    }

    /// 禮拜加上時段不是一個區間，時段詞不採用。said 只留用到的日詞。
    #[test]
    fn a_week_does_not_silently_keep_an_afternoon() {
        let now = at(2026, 8, 26, 15, 30);
        let r = time_range("這禮拜下午", now).expect("仍是這禮拜");
        assert_eq!(r.said, "這禮拜");
        assert_eq!(r.from, hour_on(2026, 8, 24, 0));
        assert_eq!(r.to, hour_on(2026, 8, 31, 0));
    }

    #[test]
    fn said_is_always_a_slice_of_what_he_typed() {
        let now = at(2026, 8, 26, 15, 30);
        for q in [
            "昨天下午",
            "我昨天下午在弄什麼",
            "昨天的下午",
            "今天早上",
            "前天",
            "上個禮拜寫了什麼",
        ] {
            let r = time_range(q, now).expect(q);
            assert!(
                q.contains(&r.said),
                "{q:?} 的 said 不是原話的一段：{:?}",
                r.said
            );
        }
    }

    /// 認不出來就是沒有。不可以回今天、不可以回 0..0。
    #[test]
    fn unrecognized_is_none_not_a_zero_range() {
        let now = at(2026, 8, 26, 15, 30);
        for q in [
            "",
            "電話",
            "剛剛發生什麼事",
            "下午",
            "什麼",
            "明天",
            "justify",
        ] {
            assert!(time_range(q, now).is_none(), "{q:?} 不該被解成一段時間");
        }
        assert_eq!(shape("下午"), Shape::Keywords, "光一個時段不夠成日曆範圍");
        assert_eq!(shape("昨天"), Shape::Range);
        assert_eq!(shape("昨天下午"), Shape::Range);
    }

    /// 同一句話、兩個 now，答案必須跟著 now 走。這就是 `now` 是參數的理由。
    #[test]
    fn the_same_words_on_two_days_are_two_ranges() {
        let wed = at(2026, 8, 26, 15, 30);
        let thu = at(2026, 8, 27, 15, 30);
        let a = time_range("昨天", wed).unwrap();
        let b = time_range("昨天", thu).unwrap();
        assert_eq!(a.from, hour_on(2026, 8, 25, 0));
        assert_eq!(b.from, hour_on(2026, 8, 26, 0));
        assert_ne!(a.from, b.from);
    }

    /// `terms` 不可以改變 `shape` 的答案——兩支走同一個切詞，這一條就是在釘
    /// 「以後有人只改其中一邊」。
    #[test]
    fn the_two_readings_of_a_question_never_disagree() {
        for q in [
            "剛剛發生什麼事",
            "剛剛那個優惠方案",
            "客服",
            "what just happened",
            "justify",
            "我昨天下午在弄什麼",
            "昨天下午的週報",
            "",
        ] {
            let stripped = terms(q);
            if shape(q) == Shape::Keywords && stripped != q {
                // 剝完剩下的一定還是關鍵字問題：剝掉的全是虛字和時間詞。
                assert_eq!(shape(stripped), Shape::Keywords, "{q:?} 剝完變了形狀");
            }
        }
    }
}
