//! L1 事實層：從文字抽出 typed facts。
//!
//! 憲法（SPEC §0.2）：**這一層零 LLM**。全部是正規表示式與程式規則，
//! 因此可重跑、可稽核、不會幻覺。抽不出來就是抽不出來——寧可漏，不可編。
//!
//! 「抄寫歸程式，意圖歸模型」：金額、電話、日期是抄寫，歸這裡；
//! 「他在為錢焦慮」是意圖，歸 L2，不歸這裡。
//!
//! 主場是繁體中文的螢幕文字，因此規則以台灣格式為第一優先
//! （NT$、09xx 手機、市話區碼、民國式日期口語、中文數字）。

use std::sync::LazyLock;

use regex::Regex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FactKind {
    Money,
    Phone,
    Url,
    Email,
    FilePath,
    ErrorCode,
    IdLike,
    DateTimeMention,
}

impl FactKind {
    pub fn as_str(self) -> &'static str {
        match self {
            FactKind::Money => "money",
            FactKind::Phone => "phone",
            FactKind::Url => "url",
            FactKind::Email => "email",
            FactKind::FilePath => "file_path",
            FactKind::ErrorCode => "error_code",
            FactKind::IdLike => "id_like",
            FactKind::DateTimeMention => "datetime",
        }
    }

    pub fn from_str_kind(s: &str) -> Option<Self> {
        Some(match s {
            "money" => FactKind::Money,
            "phone" => FactKind::Phone,
            "url" => FactKind::Url,
            "email" => FactKind::Email,
            "file_path" => FactKind::FilePath,
            "error_code" => FactKind::ErrorCode,
            "id_like" => FactKind::IdLike,
            "datetime" => FactKind::DateTimeMention,
            _ => return None,
        })
    }

    /// 重疊時誰贏。數字小的優先——格式越明確、誤判越低的排越前面。
    fn priority(self) -> u8 {
        match self {
            FactKind::Url => 0,
            FactKind::Email => 1,
            FactKind::FilePath => 2,
            FactKind::ErrorCode => 3,
            FactKind::DateTimeMention => 4,
            FactKind::Money => 5,
            FactKind::Phone => 6,
            FactKind::IdLike => 7,
        }
    }
}

/// 一個被抽出的事實。
///
/// `normalized` 是可比對的正規形式（`TWD:13450`、`+886800123456`、`TIME:17:00`），
/// `raw` 保留螢幕上的原樣——回答時要能引用使用者當時真正看到的字串。
#[derive(Debug, Clone, PartialEq)]
pub struct ExtractedFact {
    pub kind: FactKind,
    pub raw: String,
    pub normalized: String,
    pub byte_start: usize,
    pub byte_end: usize,
    /// **不是機率。** 是每條規則手寫的優先序，唯一的用途是兩條規則搶同一
    /// 段文字時決定誰贏（見 `extract` 的去重）。
    ///
    /// 沒有任何一組標註資料校準過它，所以「0.93」不代表 93% 會是對的。
    /// 名字取成 confidence 是個歷史錯誤——它長得像一個有意義的數字，而
    /// 「一個沒辦法歸因的數字，會讓人去修最好修的地方，而不是最貴的地方」。
    /// 等 Phase 1 有了重播評測集，這裡要嘛真的校準，要嘛改名成 priority。
    pub confidence: f32,
}

struct Cand {
    kind: FactKind,
    start: usize,
    end: usize,
    normalized: String,
    confidence: f32,
}

/// 從一段文字抽出所有事實，依 `byte_start` 排序、互不重疊。
pub fn extract(text: &str) -> Vec<ExtractedFact> {
    if text.is_empty() {
        return Vec::new();
    }

    let mut cands: Vec<Cand> = Vec::new();
    urls(text, &mut cands);
    emails(text, &mut cands);
    file_paths(text, &mut cands);
    error_codes(text, &mut cands);
    datetimes(text, &mut cands);
    money(text, &mut cands);
    phones(text, &mut cands);
    ids(text, &mut cands);

    // 先把跨度上的空白修掉。可選群組不存在時，它前面的 `\s*` 會把空白吃進
    // 比對範圍，害得邊界檢查誤以為隔壁的字緊貼著（`$20 per` 就是這樣被丟掉的）。
    for c in &mut cands {
        let s = &text[c.start..c.end];
        c.start += s.len() - s.trim_start().len();
        c.end -= s.len() - s.trim_end().len();
    }
    cands.retain(|c| c.end > c.start);

    // 邊界檢查：不能長在一串英數字的中間（避免從流水號裡切出假電話）
    cands.retain(|c| ascii_word_boundary(text, c.start, c.end));

    // 優先級 → 長者勝 → 位置在前者勝
    cands.sort_by(|a, b| {
        a.kind
            .priority()
            .cmp(&b.kind.priority())
            .then((b.end - b.start).cmp(&(a.end - a.start)))
            .then(a.start.cmp(&b.start))
    });

    let mut taken: Vec<(usize, usize)> = Vec::new();
    let mut out: Vec<ExtractedFact> = Vec::new();
    for c in cands {
        if taken.iter().any(|(s, e)| c.start < *e && *s < c.end) {
            continue;
        }
        taken.push((c.start, c.end));
        out.push(ExtractedFact {
            kind: c.kind,
            raw: text[c.start..c.end].to_string(),
            normalized: c.normalized,
            byte_start: c.start,
            byte_end: c.end,
            confidence: c.confidence,
        });
    }

    out.sort_by_key(|f| f.byte_start);
    out
}

/// 從自然語言查詢裡辨認出使用者要的是哪一類事實。
///
/// 為什麼需要這個：螢幕上寫的是「客服**專線**」，使用者問的是「電話」。
/// 全文檢索永遠接不起這兩個詞——但 L1 早就把那串數字標成了 `phone`。
/// 這張表就是把「使用者的說法」接到「事實的型別」的那條線。
///
/// 純查表、零模型。詞彙不足時寧可回空集合，不猜。
pub fn kinds_for_query(query: &str) -> Vec<FactKind> {
    const TABLE: &[(&str, FactKind)] = &[
        ("電話", FactKind::Phone),
        ("手機", FactKind::Phone),
        ("專線", FactKind::Phone),
        ("號碼", FactKind::Phone),
        ("phone", FactKind::Phone),
        ("tel", FactKind::Phone),
        ("金額", FactKind::Money),
        ("價格", FactKind::Money),
        ("價錢", FactKind::Money),
        ("多少錢", FactKind::Money),
        ("費用", FactKind::Money),
        ("帳單", FactKind::Money),
        ("繳費", FactKind::Money),
        ("money", FactKind::Money),
        ("price", FactKind::Money),
        ("cost", FactKind::Money),
        ("網址", FactKind::Url),
        ("連結", FactKind::Url),
        ("url", FactKind::Url),
        ("link", FactKind::Url),
        ("信箱", FactKind::Email),
        ("郵件", FactKind::Email),
        ("email", FactKind::Email),
        ("mail", FactKind::Email),
        ("檔案", FactKind::FilePath),
        ("路徑", FactKind::FilePath),
        ("file", FactKind::FilePath),
        ("path", FactKind::FilePath),
        ("錯誤", FactKind::ErrorCode),
        ("例外", FactKind::ErrorCode),
        ("error", FactKind::ErrorCode),
        ("exception", FactKind::ErrorCode),
        ("編號", FactKind::IdLike),
        ("單號", FactKind::IdLike),
        ("序號", FactKind::IdLike),
        ("日期", FactKind::DateTimeMention),
        ("時間", FactKind::DateTimeMention),
        ("期限", FactKind::DateTimeMention),
        // 「幾號」刻意不收：「今天幾號」問日期，「電話幾號」問號碼，分不出來就不猜
        ("什麼時候", FactKind::DateTimeMention),
        ("date", FactKind::DateTimeMention),
        ("when", FactKind::DateTimeMention),
        ("deadline", FactKind::DateTimeMention),
    ];

    let q = query.to_lowercase();
    let mut out: Vec<FactKind> = Vec::new();
    for (word, kind) in TABLE {
        if q.contains(word) && !out.contains(kind) {
            out.push(*kind);
        }
    }
    out
}

// ---------- 共用工具 ----------

/// 兩端不得緊鄰 ASCII 英數字或底線。
///
/// 刻意不用 regex 的 `\b`：在 unicode 模式下 CJK 是 word char，
/// 「帳單13450元」的數字兩側根本不存在 `\b`，整組規則會失效。
fn ascii_word_boundary(text: &str, start: usize, end: usize) -> bool {
    let before_ok = text[..start]
        .chars()
        .next_back()
        .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'));
    let after_ok = text[end..]
        .chars()
        .next()
        .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'));
    before_ok && after_ok
}

/// 中文數字轉阿拉伯數字。支援 零一二三四五六七八九十百千萬億 與 兩、大寫壹貳參。
fn cjk_number(s: &str) -> Option<f64> {
    let (mut total, mut section, mut digit, mut any) = (0.0f64, 0.0f64, 0.0f64, false);
    for c in s.chars() {
        let d = match c {
            '零' | '〇' => Some(0.0),
            '一' | '壹' => Some(1.0),
            '二' | '貳' | '兩' => Some(2.0),
            '三' | '參' => Some(3.0),
            '四' | '肆' => Some(4.0),
            '五' | '伍' => Some(5.0),
            '六' | '陸' => Some(6.0),
            '七' | '柒' => Some(7.0),
            '八' | '捌' => Some(8.0),
            '九' | '玖' => Some(9.0),
            _ => None,
        };
        if let Some(d) = d {
            digit = d;
            any = true;
            continue;
        }
        // 「十」單獨出現代表 10，「二十」代表 20
        let unit = match c {
            '十' | '拾' => 10.0,
            '百' | '佰' => 100.0,
            '千' | '仟' => 1000.0,
            '萬' => {
                total += (section + digit) * 10_000.0;
                section = 0.0;
                digit = 0.0;
                any = true;
                continue;
            }
            '億' => {
                total = (total + section + digit) * 100_000_000.0;
                section = 0.0;
                digit = 0.0;
                any = true;
                continue;
            }
            _ => return None,
        };
        section += if digit == 0.0 { unit } else { digit * unit };
        digit = 0.0;
        any = true;
    }
    if !any {
        return None;
    }
    Some(total + section + digit)
}

/// 金額字串正規化：去千分位、去尾零。`13450`、`49.99`、`13450.5`
fn fmt_amount(v: f64) -> String {
    if (v - v.round()).abs() < 1e-9 {
        format!("{}", v.round() as i64)
    } else {
        let s = format!("{v:.2}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// 附近是否有 CJK 字元——用來判斷孤零零的 `$` 在台灣脈絡下該算台幣還是美元。
fn cjk_nearby(text: &str, start: usize, end: usize) -> bool {
    let lo = text[..start]
        .char_indices()
        .rev()
        .take(12)
        .last()
        .map_or(0, |(i, _)| i);
    let hi = text[end..]
        .char_indices()
        .take(12)
        .last()
        .map_or(end, |(i, c)| end + i + c.len_utf8());
    text[lo..hi]
        .chars()
        .any(|c| matches!(c, '\u{3000}'..='\u{9FFF}' | '\u{FF00}'..='\u{FFEF}'))
}

// ---------- Money ----------

static RE_MONEY_PREFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(NT\$|NTD|TWD|US\$|USD|RMB|CNY|JPY|EUR|GBP|HKD|新臺幣|新台幣|台幣|人民幣|日幣|美金|美元|[¥＄€£$])\s*(\d[\d,]*(?:\.\d{1,2})?)\s*([萬千億])?",
    )
    .expect("money prefix regex")
});

static RE_MONEY_SUFFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(\d[\d,]*(?:\.\d{1,2})?)\s*([萬千億])?\s*(元整|元|塊錢|塊|圓|日圓|日元|美元|美金|歐元|英鎊|港幣|人民幣|新臺幣|新台幣|台幣)",
    )
    .expect("money suffix regex")
});

static RE_MONEY_CJK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[零〇一二三四五六七八九十百千萬億兩壹貳參肆伍陸柒捌玖拾佰仟]{1,12}\s*(元整|元|塊錢|塊|圓)")
        .expect("money cjk regex")
});

fn currency_of(token: &str) -> Option<&'static str> {
    let t = token.to_ascii_uppercase();
    Some(match t.as_str() {
        "NT$" | "NTD" | "TWD" => "TWD",
        "US$" | "USD" => "USD",
        "RMB" | "CNY" => "CNY",
        "JPY" => "JPY",
        "EUR" => "EUR",
        "GBP" => "GBP",
        "HKD" => "HKD",
        _ => match token {
            "新臺幣" | "新台幣" | "台幣" | "元整" | "元" | "塊錢" | "塊" | "圓" => {
                "TWD"
            }
            "人民幣" => "CNY",
            "日幣" | "日圓" | "日元" | "¥" => "JPY",
            "美金" | "美元" => "USD",
            "歐元" | "€" => "EUR",
            "英鎊" | "£" => "GBP",
            "港幣" => "HKD",
            _ => return None,
        },
    })
}

fn multiplier(m: &str) -> f64 {
    match m {
        "千" => 1_000.0,
        "萬" => 10_000.0,
        "億" => 100_000_000.0,
        _ => 1.0,
    }
}

fn money(text: &str, out: &mut Vec<Cand>) {
    for c in RE_MONEY_PREFIX.captures_iter(text) {
        let whole = c.get(0).expect("group 0");
        let sym = c.get(1).expect("symbol").as_str();
        let num: f64 = match c[2].replace(',', "").parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        let mult = c.get(3).map_or(1.0, |m| multiplier(m.as_str()));

        // 孤零零的 `$`：旁邊有中文就是台幣，否則按國際慣例算美元
        let (cur, conf) = if sym == "$" || sym == "＄" {
            if cjk_nearby(text, whole.start(), whole.end()) {
                ("TWD", 0.75)
            } else {
                ("USD", 0.70)
            }
        } else {
            (currency_of(sym).unwrap_or("TWD"), 0.95)
        };

        out.push(Cand {
            kind: FactKind::Money,
            start: whole.start(),
            end: whole.end(),
            normalized: format!("{cur}:{}", fmt_amount(num * mult)),
            confidence: conf,
        });
    }

    for c in RE_MONEY_SUFFIX.captures_iter(text) {
        let whole = c.get(0).expect("group 0");
        let num: f64 = match c[1].replace(',', "").parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        let mult = c.get(2).map_or(1.0, |m| multiplier(m.as_str()));
        let cur = currency_of(&c[3]).unwrap_or("TWD");
        out.push(Cand {
            kind: FactKind::Money,
            start: whole.start(),
            end: whole.end(),
            normalized: format!("{cur}:{}", fmt_amount(num * mult)),
            confidence: 0.93,
        });
    }

    for c in RE_MONEY_CJK.captures_iter(text) {
        let whole = c.get(0).expect("group 0");
        let unit = c.get(1).expect("unit");
        let digits = text[whole.start()..unit.start()].trim();
        let Some(v) = cjk_number(digits) else {
            continue;
        };
        let cur = currency_of(unit.as_str()).unwrap_or("TWD");
        out.push(Cand {
            kind: FactKind::Money,
            start: whole.start(),
            end: whole.end(),
            normalized: format!("{cur}:{}", fmt_amount(v)),
            confidence: 0.85,
        });
    }
}

// ---------- Phone（台灣） ----------

static RE_PHONE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        // 國際格式手機
        r"(?:\+?886[-\s]?0?9\d{2}[-\s]?\d{3}[-\s]?\d{3})",
        // 國內手機 09xx
        r"|(?:09\d{2}[-\s]?\d{3}[-\s]?\d{3})",
        // 免付費 / 付費客服 0800 0809 0203 412
        r"|(?:080[09][-\s]?\d{3}[-\s]?\d{3})",
        // 市話：(02) 2345-6789 / 02-2345-6789 / 03-123-4567 / 037-123456
        r"|(?:\(0[2-9]\d?\)[-\s]?\d{3,4}[-\s]?\d{4})",
        r"|(?:0[2-9]\d?[-\s]\d{3,4}[-\s]?\d{4})",
    ))
    .expect("phone regex")
});

fn phones(text: &str, out: &mut Vec<Cand>) {
    for m in RE_PHONE.find_iter(text) {
        let raw = m.as_str();
        let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();

        // 去掉國碼與國內冠碼，統一成 E.164 的國內號碼部分
        let national = digits
            .strip_prefix("886")
            .map(|d| d.trim_start_matches('0'))
            .unwrap_or_else(|| digits.trim_start_matches('0'));

        // 台灣號碼去掉 0 之後是 8~9 碼；不合就丟掉，不猜
        if national.len() < 8 || national.len() > 9 {
            continue;
        }

        let conf = if raw.starts_with('+') || raw.starts_with("886") {
            0.98
        } else if national.starts_with('9')
            || national.starts_with("800")
            || national.starts_with("809")
        {
            0.95 // 手機與 0800 客服號碼的格式最不容易撞號
        } else {
            0.82 // 市話最容易和日期、流水號撞號
        };

        out.push(Cand {
            kind: FactKind::Phone,
            start: m.start(),
            end: m.end(),
            normalized: format!("+886{national}"),
            confidence: conf,
        });
    }
}

// ---------- Url / Email ----------

static RE_URL: LazyLock<Regex> = LazyLock::new(|| {
    // 字元集刻意全 ASCII：中文標點自然成為結束邊界，不必額外處理
    Regex::new(r"(?i)(?:https?://|ftp://|www\.)[A-Za-z0-9\-._~:/?#\[\]@!$&'()*+,;=%]+")
        .expect("url regex")
});

static RE_EMAIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[A-Za-z0-9._%+\-]+@[A-Za-z0-9\-]+(?:\.[A-Za-z0-9\-]+)*\.[A-Za-z]{2,24}")
        .expect("email regex")
});

fn urls(text: &str, out: &mut Vec<Cand>) {
    for m in RE_URL.find_iter(text) {
        // 句尾標點不屬於網址
        let trimmed = m
            .as_str()
            .trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '}', '\'', '"', '>']);
        if trimmed.len() < 8 {
            continue;
        }
        let lower = trimmed.to_ascii_lowercase();
        let normalized = if lower.starts_with("www.") {
            format!("https://{lower}")
        } else {
            lower
        };
        out.push(Cand {
            kind: FactKind::Url,
            start: m.start(),
            end: m.start() + trimmed.len(),
            normalized,
            confidence: 0.97,
        });
    }
}

fn emails(text: &str, out: &mut Vec<Cand>) {
    for m in RE_EMAIL.find_iter(text) {
        out.push(Cand {
            kind: FactKind::Email,
            start: m.start(),
            end: m.end(),
            normalized: m.as_str().to_ascii_lowercase(),
            confidence: 0.96,
        });
    }
}

// ---------- FilePath ----------

/// Windows 路徑的合法字元。刻意用「白名單」而非 `[^\s...]`：
/// 負向字元集會把後面的中文一路吞掉（`C:\x\y.rs，找不到` → 整句變成路徑）。
/// 空白允許（`C:\Program Files\`），但由 [`trim_path_tail`] 事後收尾。
const WIN_PATH_CHARS: &str = r"[A-Za-z0-9_\-.\\/()~$@#%&+='\[\] ]";

static RE_PATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        &concat!(
            // Windows 絕對路徑 C:\Users\ted\x.rs
            r"(?:[A-Za-z]:\\{c}{2,240})",
            // UNC \\server\share\x
            r"|(?:\\\\{c}{3,240})",
            // Unix / ~ 開頭，且含副檔名
            r"|(?:(?:~|\.{1,2})?/(?:[\w.+\-@]+/)*[\w.+\-@]+\.[A-Za-z0-9]{1,8})",
            // Unix 目錄（至少兩層）
            r"|(?:(?:~|\.{1,2})?/(?:[\w.+\-@]+/){2,}[\w.+\-@]*)",
            // 相對路徑：至少兩段且有副檔名（`crates/sister-core/src/db.rs`）
            r"|(?:[\w.+\-@]+(?:/[\w.+\-@]+)+\.[A-Za-z0-9]{1,8})",
        )
        .replace("{c}", WIN_PATH_CHARS),
    )
    .expect("path regex")
});

/// 收掉尾巴上不屬於路徑的東西。
///
/// 路徑裡可以有空白，但只有在空白之後還有分隔符時才算數。
/// `C:\Program Files\App\x.exe is missing` 的最後一段是 `x.exe is missing`，
/// 其中的空白之後沒有 `\`，所以從那裡切斷。
fn trim_path_tail(s: &str) -> &str {
    let s = s.trim_end_matches([' ', '.', ',', ';', ':', ')', ']', '"', '\'']);
    let last_sep = s.rfind(['\\', '/']).map_or(0, |i| i + 1);
    match s[last_sep..].find(' ') {
        Some(i) => s[..last_sep + i].trim_end_matches([' ', '\\', '/']),
        None => s,
    }
}

fn file_paths(text: &str, out: &mut Vec<Cand>) {
    for m in RE_PATH.find_iter(text) {
        let raw = trim_path_tail(m.as_str());
        if raw.len() < 5 {
            continue;
        }
        // 反斜線統一成正斜線只是為了比對；raw 仍保留使用者看到的樣子
        out.push(Cand {
            kind: FactKind::FilePath,
            start: m.start(),
            end: m.start() + raw.len(),
            normalized: raw.replace('\\', "/"),
            confidence: 0.85,
        });
    }
}

// ---------- ErrorCode ----------

/// 常見 errno / POSIX 錯誤名。用白名單而非 `E[A-Z]+`，避免把縮寫詞當錯誤碼。
const ERRNO: &[&str] = &[
    "ENOENT",
    "EACCES",
    "EPERM",
    "EEXIST",
    "ENOTDIR",
    "EISDIR",
    "EINVAL",
    "ENOSPC",
    "EPIPE",
    "EAGAIN",
    "EBUSY",
    "EMFILE",
    "ENFILE",
    "ECONNREFUSED",
    "ECONNRESET",
    "ECONNABORTED",
    "ETIMEDOUT",
    "EHOSTUNREACH",
    "ENETUNREACH",
    "EADDRINUSE",
    "EADDRNOTAVAIL",
    "EPROTO",
    "ENOMEM",
    "EIO",
    "EROFS",
    "EXDEV",
    "ELOOP",
    "ENAMETOOLONG",
];

static RE_ERR_SNAKE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[A-Z][A-Z0-9]*(?:_[A-Z0-9]+)+").expect("err snake regex"));
static RE_ERR_HEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"0x[0-9A-Fa-f]{8}").expect("err hex regex"));
static RE_ERR_COMPILER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:E|TS|CS|MSB|CA|SA|CVE-\d{4}-)\d{3,7}").expect("err code regex")
});
static RE_ERR_SIGNAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"SIG(?:SEGV|KILL|TERM|ABRT|BUS|FPE|ILL|INT|HUP|PIPE)").expect("signal regex")
});
static RE_ERR_EXC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[A-Z][A-Za-z0-9]{2,40}(?:Exception|Error)").expect("exception regex")
});
static RE_ERR_HTTP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:http|https|status|狀態碼|錯誤碼|回應碼)\s*[:：]?\s*([1-5]\d{2})")
        .expect("http status regex")
});
static RE_ERRNO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"E[A-Z]{2,12}").expect("errno regex"));

/// SCREAMING_SNAKE 常數多得是，只有語意上真的在講錯誤的才收。
fn snake_looks_like_error(s: &str) -> bool {
    const SIGNALS: &[&str] = &[
        "ERR",
        "ERROR",
        "FAIL",
        "FAILED",
        "FAILURE",
        "INVALID",
        "DENIED",
        "REFUSED",
        "TIMEOUT",
        "TIMED_OUT",
        "NOT_FOUND",
        "UNAUTHORIZED",
        "FORBIDDEN",
        "ABORTED",
        "CRASH",
        "PANIC",
        "UNAVAILABLE",
        "EXCEPTION",
        "FATAL",
        "CORRUPT",
        "MISMATCH",
        "REJECTED",
        "EXPIRED",
    ];
    s.len() >= 6 && SIGNALS.iter().any(|k| s.contains(k))
}

fn error_codes(text: &str, out: &mut Vec<Cand>) {
    let mut push = |start: usize, end: usize, norm: String, conf: f32| {
        out.push(Cand {
            kind: FactKind::ErrorCode,
            start,
            end,
            normalized: norm,
            confidence: conf,
        });
    };

    for m in RE_ERR_SNAKE.find_iter(text) {
        if snake_looks_like_error(m.as_str()) {
            push(m.start(), m.end(), m.as_str().to_string(), 0.90);
        }
    }
    for m in RE_ERRNO.find_iter(text) {
        if ERRNO.contains(&m.as_str()) {
            push(m.start(), m.end(), m.as_str().to_string(), 0.95);
        }
    }
    for m in RE_ERR_HEX.find_iter(text) {
        push(
            m.start(),
            m.end(),
            m.as_str().to_ascii_uppercase().replace("0X", "0x"),
            0.80,
        );
    }
    for m in RE_ERR_COMPILER.find_iter(text) {
        push(m.start(), m.end(), m.as_str().to_ascii_uppercase(), 0.85);
    }
    for m in RE_ERR_SIGNAL.find_iter(text) {
        push(m.start(), m.end(), m.as_str().to_string(), 0.95);
    }
    for m in RE_ERR_EXC.find_iter(text) {
        push(m.start(), m.end(), m.as_str().to_string(), 0.88);
    }
    for c in RE_ERR_HTTP.captures_iter(text) {
        let code = c.get(1).expect("status group");
        push(
            code.start(),
            code.end(),
            format!("HTTP:{}", code.as_str()),
            0.85,
        );
    }
}

// ---------- IdLike ----------

static RE_UUID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}")
        .expect("uuid regex")
});
static RE_TWID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[A-Z][12]\d{8}").expect("tw id regex"));
static RE_VAT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:統一編號|統編|營利事業統一編號|VAT|Tax\s*ID)\s*[:：]?\s*(\d{8})")
        .expect("vat regex")
});
static RE_SHA: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)commit\s+([0-9a-f]{7,40})").expect("sha regex"));
static RE_ORDER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:訂單編號|訂單號碼|訂單|單號|案件編號|會員編號|序號|發票號碼|order\s*(?:id|no\.?|number)|ticket|ref\.?)\s*[:：#]?\s*([A-Za-z0-9][A-Za-z0-9\-]{4,23})",
    )
    .expect("order regex")
});

/// 中華民國身分證字號檢查碼。有了它，`[A-Z][12]\d{8}` 的誤判率趨近於零。
fn tw_id_valid(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10 {
        return false;
    }
    let letter = b[0];
    if !letter.is_ascii_uppercase() {
        return false;
    }
    const MAP: &[u32] = &[
        10, 11, 12, 13, 14, 15, 16, 17, 34, 18, 19, 20, 21, 22, 35, 23, 24, 25, 26, 27, 28, 29, 32,
        30, 31, 33,
    ];
    let code = MAP[(letter - b'A') as usize];
    let mut sum = code / 10 + (code % 10) * 9;
    for (i, d) in b[1..10].iter().enumerate() {
        if !d.is_ascii_digit() {
            return false;
        }
        let weight = if i == 8 { 1 } else { 8 - i as u32 };
        sum += (d - b'0') as u32 * weight;
    }
    sum.is_multiple_of(10)
}

fn ids(text: &str, out: &mut Vec<Cand>) {
    let mut push = |start: usize, end: usize, norm: String, conf: f32| {
        out.push(Cand {
            kind: FactKind::IdLike,
            start,
            end,
            normalized: norm,
            confidence: conf,
        });
    };

    for m in RE_UUID.find_iter(text) {
        push(m.start(), m.end(), m.as_str().to_ascii_lowercase(), 0.99);
    }
    for m in RE_TWID.find_iter(text) {
        if tw_id_valid(m.as_str()) {
            push(m.start(), m.end(), m.as_str().to_string(), 0.92);
        }
    }
    for c in [&*RE_VAT, &*RE_SHA, &*RE_ORDER]
        .into_iter()
        .flat_map(|re| re.captures_iter(text))
    {
        let g = c.get(1).expect("id group");
        push(g.start(), g.end(), g.as_str().to_string(), 0.90);
    }
}

// ---------- DateTimeMention ----------
//
// L1 只記「線索」，不解析成絕對時間。`明天` 正規化成 `REL:+1d`，
// 要換算成哪一天由呼叫端配合該筆 chunk 的時間戳決定——抽取器拿不到
// 時間戳，硬猜就是編造。

static RE_DATE_FULL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\d{4})[-/年](\d{1,2})[-/月](\d{1,2})日?").expect("date regex"));
static RE_DATE_PART: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\d{1,2})([/月])(\d{1,2})日?").expect("partial date regex"));
static RE_CLOCK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\d{1,2}):(\d{2})(?::(\d{2}))?").expect("clock regex"));
static RE_AMPM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(\d{1,2})(?::(\d{2}))?\s*(am|pm)").expect("ampm regex"));
static RE_CJK_CLOCK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(凌晨|清晨|早上|上午|中午|下午|傍晚|晚上|晚間|深夜|半夜)?\s*(\d{1,2})\s*[點時](半|\d{1,2}\s*分?)?")
        .expect("cjk clock regex")
});
static RE_REL_WORD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"大後天|大前天|後天|前天|明天|明日|隔天|明早|明晚|昨天|昨日|今天|今日|今晚|今夜")
        .expect("relative word regex")
});
static RE_REL_QTY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(\d{1,4}|[零一二三四五六七八九十百兩]{1,6})\s*(個月|分鐘|小時|星期|週|周|天|日|年)\s*(以後|之後|以內|之內|以前|之前|後|內|前)",
    )
    .expect("relative qty regex")
});
static RE_WEEK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(上上|下下|上|下|這|本|今)?(?:週|周|星期|禮拜)([一二三四五六日天])?")
        .expect("week regex")
});
static RE_MONTH_OFF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(上上|下下|上|下|這|本|今)\s*個?月").expect("month offset regex")
});

fn week_offset_days(prefix: &str) -> i32 {
    match prefix {
        "上上" => -14,
        "下下" => 14,
        "上" => -7,
        "下" => 7,
        _ => 0,
    }
}

fn datetimes(text: &str, out: &mut Vec<Cand>) {
    let mut push = |start: usize, end: usize, norm: String, conf: f32| {
        out.push(Cand {
            kind: FactKind::DateTimeMention,
            start,
            end,
            normalized: norm,
            confidence: conf,
        });
    };

    for c in RE_DATE_FULL.captures_iter(text) {
        let (y, m, d) = (
            c[1].parse::<u32>().unwrap_or(0),
            c[2].parse::<u32>().unwrap_or(0),
            c[3].parse::<u32>().unwrap_or(0),
        );
        if !(1900..=2200).contains(&y) || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
            continue;
        }
        let w = c.get(0).expect("group 0");
        push(
            w.start(),
            w.end(),
            format!("DATE:{y:04}-{m:02}-{d:02}"),
            0.95,
        );
    }

    for c in RE_DATE_PART.captures_iter(text) {
        let (m, d) = (
            c[1].parse::<u32>().unwrap_or(0),
            c[3].parse::<u32>().unwrap_or(0),
        );
        if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
            continue;
        }
        let w = c.get(0).expect("group 0");
        // `8月17日` 不會有別的意思；`8/17` 可能是分數或比例，信心壓低
        let conf = if &c[2] == "月" { 0.92 } else { 0.65 };
        push(w.start(), w.end(), format!("DATE:--{m:02}-{d:02}"), conf);
    }

    for c in RE_CLOCK.captures_iter(text) {
        let (h, m) = (
            c[1].parse::<u32>().unwrap_or(99),
            c[2].parse::<u32>().unwrap_or(99),
        );
        if h > 23 || m > 59 {
            continue;
        }
        let w = c.get(0).expect("group 0");
        push(w.start(), w.end(), format!("TIME:{h:02}:{m:02}"), 0.80);
    }

    for c in RE_AMPM.captures_iter(text) {
        let mut h = c[1].parse::<u32>().unwrap_or(99);
        let m = c
            .get(2)
            .and_then(|g| g.as_str().parse::<u32>().ok())
            .unwrap_or(0);
        if h > 12 || m > 59 {
            continue;
        }
        let pm = c[3].eq_ignore_ascii_case("pm");
        if pm && h < 12 {
            h += 12;
        } else if !pm && h == 12 {
            h = 0;
        }
        let w = c.get(0).expect("group 0");
        push(w.start(), w.end(), format!("TIME:{h:02}:{m:02}"), 0.90);
    }

    for c in RE_CJK_CLOCK.captures_iter(text) {
        let mut h = c[2].parse::<u32>().unwrap_or(99);
        if h > 23 {
            continue;
        }
        let m = match c.get(3).map(|g| g.as_str().trim()) {
            Some("半") => 30,
            Some(s) => s
                .trim_end_matches(['分', ' '])
                .trim()
                .parse::<u32>()
                .unwrap_or(0),
            None => 0,
        };
        if m > 59 {
            continue;
        }
        match c.get(1).map(|g| g.as_str()) {
            Some("下午" | "傍晚" | "晚上" | "晚間") if h < 12 => h += 12,
            Some("深夜" | "半夜") if h < 6 => {}
            Some("深夜" | "半夜") if h < 12 => h += 12,
            Some("凌晨" | "清晨" | "早上" | "上午") if h == 12 => h = 0,
            _ => {}
        }
        let w = c.get(0).expect("group 0");
        push(w.start(), w.end(), format!("TIME:{h:02}:{m:02}"), 0.90);
    }

    for m in RE_REL_WORD.find_iter(text) {
        let days = match m.as_str() {
            "大後天" => 3,
            "後天" => 2,
            "明天" | "明日" | "隔天" | "明早" | "明晚" => 1,
            "今天" | "今日" | "今晚" | "今夜" => 0,
            "昨天" | "昨日" => -1,
            "前天" => -2,
            "大前天" => -3,
            _ => continue,
        };
        push(m.start(), m.end(), format!("REL:{days:+}d"), 0.92);
    }

    for c in RE_REL_QTY.captures_iter(text) {
        let n = c[1]
            .parse::<f64>()
            .ok()
            .or_else(|| cjk_number(&c[1]))
            .unwrap_or(0.0);
        if n <= 0.0 || n > 10_000.0 {
            continue;
        }
        let (mult, unit) = match &c[2] {
            "分鐘" => (1.0, 'm'),
            "小時" => (1.0, 'h'),
            "天" | "日" => (1.0, 'd'),
            "星期" | "週" | "周" => (7.0, 'd'),
            "個月" => (30.0, 'd'),
            "年" => (365.0, 'd'),
            _ => continue,
        };
        let sign = if c[3].contains('前') { -1.0 } else { 1.0 };
        let v = (n * mult * sign) as i64;
        let w = c.get(0).expect("group 0");
        push(w.start(), w.end(), format!("REL:{v:+}{unit}"), 0.88);
    }

    for c in RE_WEEK.captures_iter(text) {
        let w = c.get(0).expect("group 0");
        match c.get(2).map(|g| g.as_str()) {
            Some(d) => {
                let n = match d {
                    "一" => 1,
                    "二" => 2,
                    "三" => 3,
                    "四" => 4,
                    "五" => 5,
                    "六" => 6,
                    _ => 7, // 日 / 天
                };
                push(w.start(), w.end(), format!("WEEKDAY:{n}"), 0.90);
            }
            // 光一個「週」太模糊，只有帶上/下/這才是時間指涉
            None => {
                if let Some(p) = c.get(1) {
                    let days = week_offset_days(p.as_str());
                    push(w.start(), w.end(), format!("REL:{days:+}d"), 0.85);
                }
            }
        }
    }

    for c in RE_MONTH_OFF.captures_iter(text) {
        let days = week_offset_days(&c[1]) / 7 * 30;
        let w = c.get(0).expect("group 0");
        push(w.start(), w.end(), format!("REL:{days:+}d"), 0.85);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 取出某一類事實的正規化值。
    fn norms(text: &str, kind: FactKind) -> Vec<String> {
        extract(text)
            .into_iter()
            .filter(|f| f.kind == kind)
            .map(|f| f.normalized)
            .collect()
    }

    fn raws(text: &str, kind: FactKind) -> Vec<String> {
        extract(text)
            .into_iter()
            .filter(|f| f.kind == kind)
            .map(|f| f.raw)
            .collect()
    }

    fn has(text: &str, kind: FactKind, normalized: &str) -> bool {
        norms(text, kind).iter().any(|n| n == normalized)
    }

    // ---------- 結構不變式 ----------

    #[test]
    fn output_is_sorted_and_non_overlapping() {
        let text = "2026-08-17 17:00 帳單 NT$13,450 客服 0800-123-456 \
                    見 https://bill.example.com/x 或 ted@ted-h.com";
        let facts = extract(text);
        assert!(facts.len() >= 5, "expected several facts, got {facts:?}");

        let mut prev_end = 0;
        for f in &facts {
            assert!(f.byte_start >= prev_end, "overlap or bad order at {f:?}");
            assert!(f.byte_end > f.byte_start);
            assert_eq!(
                &text[f.byte_start..f.byte_end],
                f.raw,
                "raw must match its span"
            );
            assert!(f.confidence > 0.0 && f.confidence <= 1.0);
            prev_end = f.byte_end;
        }
    }

    #[test]
    fn empty_and_boring_text_yields_nothing() {
        assert!(extract("").is_empty());
        assert!(extract("   \n  ").is_empty());
        assert!(extract("天氣不錯，我在想事情。").is_empty());
        assert!(extract("hello there how are you").is_empty());
    }

    #[test]
    fn optional_trailing_groups_do_not_eat_the_next_word() {
        // 迴歸測試：`$20 per month` 曾因為比對範圍多含一個空白而整個被丟掉
        let f = extract("Pro plan $20 per month");
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(f[0].raw, "$20", "span must not include trailing space");
        assert_eq!(f[0].normalized, "USD:20");
        assert_eq!(raws("訂閱費 $390 每月", FactKind::Money), vec!["$390"]);
    }

    #[test]
    fn query_keywords_map_to_fact_kinds() {
        // 這條線就是 Phase 0 退出條件成立的原因：螢幕上是「客服專線」，
        // 使用者問「電話」，接起來的不是全文檢索而是 L1 的型別。
        assert_eq!(kinds_for_query("電話"), vec![FactKind::Phone]);
        assert_eq!(kinds_for_query("客服電話幾號"), vec![FactKind::Phone]);
        assert_eq!(kinds_for_query("phone number"), vec![FactKind::Phone]);
        assert_eq!(kinds_for_query("帳單多少錢"), vec![FactKind::Money]);
        assert_eq!(
            kinds_for_query("繳費期限"),
            vec![FactKind::Money, FactKind::DateTimeMention]
        );
        assert_eq!(
            kinds_for_query("那個 error code"),
            vec![FactKind::ErrorCode]
        );
        // 認不出來就回空的，不猜
        assert!(kinds_for_query("我昨天在幹嘛").is_empty());
        assert!(kinds_for_query("").is_empty());
    }

    #[test]
    fn kind_strings_round_trip() {
        for k in [
            FactKind::Money,
            FactKind::Phone,
            FactKind::Url,
            FactKind::Email,
            FactKind::FilePath,
            FactKind::ErrorCode,
            FactKind::IdLike,
            FactKind::DateTimeMention,
        ] {
            assert_eq!(FactKind::from_str_kind(k.as_str()), Some(k));
        }
        assert_eq!(FactKind::from_str_kind("nope"), None);
    }

    // ---------- Money ----------

    #[test]
    fn money_taiwan_formats() {
        assert!(has("本期應繳 NT$13,450", FactKind::Money, "TWD:13450"));
        assert!(has("本期應繳 NT$ 13,450 元", FactKind::Money, "TWD:13450"));
        assert!(has("金額 13,450元", FactKind::Money, "TWD:13450"));
        assert!(has("新台幣13,450元整", FactKind::Money, "TWD:13450"));
        assert!(has("TWD 1350", FactKind::Money, "TWD:1350"));
        assert!(has("要價 1.5萬元", FactKind::Money, "TWD:15000"));
        assert!(has("差 250 塊", FactKind::Money, "TWD:250"));
    }

    #[test]
    fn money_decimals_and_foreign_currencies() {
        assert!(has("USD 49.99", FactKind::Money, "USD:49.99"));
        assert!(has("US$1,299.00", FactKind::Money, "USD:1299"));
        assert!(has("€89.50", FactKind::Money, "EUR:89.5"));
        assert!(has("JPY 12000", FactKind::Money, "JPY:12000"));
        assert!(has("人民幣 300", FactKind::Money, "CNY:300"));
        assert!(has("賣 100 美元", FactKind::Money, "USD:100"));
    }

    #[test]
    fn money_cjk_numerals() {
        assert!(has("繳了一百二十元", FactKind::Money, "TWD:120"));
        assert!(has("兩萬元", FactKind::Money, "TWD:20000"));
        assert!(has("三萬五千元", FactKind::Money, "TWD:35000"));
        assert!(has("一千兩百五十元", FactKind::Money, "TWD:1250"));
    }

    #[test]
    fn bare_dollar_sign_reads_context() {
        // 旁邊有中文 → 台灣脈絡下的 $ 是台幣
        assert!(has("訂閱費 $390 每月", FactKind::Money, "TWD:390"));
        // 純英文脈絡 → 依國際慣例算美元
        assert!(has("Pro plan $20 per month", FactKind::Money, "USD:20"));
    }

    #[test]
    fn money_does_not_invent_currency_from_bare_numbers() {
        // 「3萬人」是人數不是錢——沒有貨幣詞就不該猜
        assert!(norms("現場有3萬人", FactKind::Money).is_empty());
        assert!(norms("共 13450 筆資料", FactKind::Money).is_empty());
        assert!(norms("版本 2.5", FactKind::Money).is_empty());
    }

    // ---------- Phone ----------

    #[test]
    fn phone_taiwan_formats_normalize_to_e164() {
        assert!(has(
            "客服專線 0800-123-456",
            FactKind::Phone,
            "+886800123456"
        ));
        assert!(has("手機 0912-345-678", FactKind::Phone, "+886912345678"));
        assert!(has("手機 0912345678", FactKind::Phone, "+886912345678"));
        assert!(has("手機 0912 345 678", FactKind::Phone, "+886912345678"));
        assert!(has("打 +886-912-345-678", FactKind::Phone, "+886912345678"));
        assert!(has("市話 02-2345-6789", FactKind::Phone, "+886223456789"));
        assert!(has("市話 (02) 2345-6789", FactKind::Phone, "+886223456789"));
        assert!(has("台中 04-1234-5678", FactKind::Phone, "+886412345678"));
    }

    #[test]
    fn phone_rejects_lookalikes() {
        // 日期不是電話
        assert!(norms("2026-08-17", FactKind::Phone).is_empty());
        // 訂單流水號不是電話（邊界檢查要擋住從中間切一段出來）
        assert!(norms("ORDER0912345678901234", FactKind::Phone).is_empty());
        assert!(norms("金額 13,450", FactKind::Phone).is_empty());
        // 長度不對就不猜
        assert!(norms("0912-345", FactKind::Phone).is_empty());
    }

    // ---------- Url / Email ----------

    #[test]
    fn urls_and_emails() {
        assert!(has(
            "看 https://dash.cloudflare.com/dns 設定",
            FactKind::Url,
            "https://dash.cloudflare.com/dns"
        ));
        assert!(has(
            "到 www.example.com 看看",
            FactKind::Url,
            "https://www.example.com"
        ));
        assert!(has(
            "寄給 Ted@Ted-H.com 就好",
            FactKind::Email,
            "ted@ted-h.com"
        ));
    }

    #[test]
    fn url_stops_at_sentence_punctuation() {
        // 中文標點與句號都不屬於網址
        assert_eq!(
            raws("請看 https://example.com/a/b。", FactKind::Url),
            vec!["https://example.com/a/b"]
        );
        assert_eq!(
            raws("see https://example.com/a/b.", FactKind::Url),
            vec!["https://example.com/a/b"]
        );
    }

    #[test]
    fn email_inside_url_does_not_double_count() {
        let facts = extract("https://api.example.com/u/bob@example.com/profile");
        assert_eq!(facts.len(), 1, "url must win the overlap: {facts:?}");
        assert_eq!(facts[0].kind, FactKind::Url);
    }

    // ---------- FilePath ----------

    #[test]
    fn file_paths_unix_and_windows() {
        assert!(has(
            "編輯 /home/ted-h/projects/AI-Sister/docs/SPEC.md",
            FactKind::FilePath,
            "/home/ted-h/projects/AI-Sister/docs/SPEC.md"
        ));
        assert!(has(
            "開 C:\\Users\\ted\\notes.txt",
            FactKind::FilePath,
            "C:/Users/ted/notes.txt"
        ));
        assert!(has(
            "放在 ~/Downloads/bill.pdf",
            FactKind::FilePath,
            "~/Downloads/bill.pdf"
        ));
    }

    #[test]
    fn windows_path_stops_before_trailing_prose() {
        // 白名單字元集擋住中文，trim_path_tail 擋住英文句子
        assert_eq!(
            raws("C:\\x\\y.rs，找不到檔案", FactKind::FilePath),
            vec!["C:\\x\\y.rs"]
        );
        assert_eq!(
            raws(
                "C:\\Program Files\\App\\x.exe is missing",
                FactKind::FilePath
            ),
            vec!["C:\\Program Files\\App\\x.exe"]
        );
    }

    // ---------- ErrorCode ----------

    #[test]
    fn error_codes_across_ecosystems() {
        assert!(has(
            "ERR_CONNECTION_REFUSED",
            FactKind::ErrorCode,
            "ERR_CONNECTION_REFUSED"
        ));
        assert!(has(
            "connect ECONNREFUSED 127.0.0.1",
            FactKind::ErrorCode,
            "ECONNREFUSED"
        ));
        assert!(has("open failed: ENOENT", FactKind::ErrorCode, "ENOENT"));
        assert!(has("錯誤 0x80070005", FactKind::ErrorCode, "0x80070005"));
        assert!(has(
            "error[E0308]: mismatched types",
            FactKind::ErrorCode,
            "E0308"
        ));
        assert!(has(
            "TS2345: argument of type",
            FactKind::ErrorCode,
            "TS2345"
        ));
        assert!(has("killed by SIGSEGV", FactKind::ErrorCode, "SIGSEGV"));
        assert!(has(
            "threw a NullPointerException",
            FactKind::ErrorCode,
            "NullPointerException"
        ));
    }

    #[test]
    fn http_status_needs_context_and_bare_numbers_do_not_count() {
        assert!(has("HTTP 404 Not Found", FactKind::ErrorCode, "HTTP:404"));
        assert!(has("狀態碼：500", FactKind::ErrorCode, "HTTP:500"));
        // 光一個 404 太模糊，不收
        assert!(norms("共 404 筆", FactKind::ErrorCode).is_empty());
    }

    #[test]
    fn ordinary_constants_are_not_error_codes() {
        // SCREAMING_SNAKE 滿街都是，只有語意上在講錯誤的才算
        assert!(norms("MAX_RETRY_COUNT = 3", FactKind::ErrorCode).is_empty());
        assert!(norms("API_BASE_URL", FactKind::ErrorCode).is_empty());
        // 反過來，名字裡就有錯誤語意的仍然要收
        assert!(has(
            "DEFAULT_TIMEOUT_MS",
            FactKind::ErrorCode,
            "DEFAULT_TIMEOUT_MS"
        ));
    }

    // ---------- IdLike ----------

    #[test]
    fn ids_with_explicit_format() {
        assert!(has(
            "trace 550e8400-e29b-41d4-a716-446655440000",
            FactKind::IdLike,
            "550e8400-e29b-41d4-a716-446655440000"
        ));
        assert!(has("身分證 A123456789", FactKind::IdLike, "A123456789"));
        assert!(has("統一編號: 12345675", FactKind::IdLike, "12345675"));
        assert!(has(
            "訂單編號 TW-20260817-001",
            FactKind::IdLike,
            "TW-20260817-001"
        ));
        assert!(has("commit a1b2c3d4e5f6", FactKind::IdLike, "a1b2c3d4e5f6"));
    }

    #[test]
    fn tw_id_checksum_rejects_invalid() {
        assert!(tw_id_valid("A123456789"));
        assert!(!tw_id_valid("A123456788"));
        assert!(!tw_id_valid("A123456780"));
        assert!(!tw_id_valid("1234567890"));
        assert!(!tw_id_valid("A12345678"));
        // 檢查碼擋掉的東西不該進 facts
        assert!(norms("代號 A123456788", FactKind::IdLike).is_empty());
    }

    #[test]
    fn bare_digit_runs_are_not_ids() {
        // 沒有格式也沒有關鍵字 → 不猜，否則 facts 表會被流水號淹掉
        assert!(norms("12345678901234", FactKind::IdLike).is_empty());
    }

    // ---------- DateTimeMention ----------

    #[test]
    fn absolute_dates() {
        assert!(has(
            "2026-08-17 交件",
            FactKind::DateTimeMention,
            "DATE:2026-08-17"
        ));
        assert!(has(
            "2026/8/17",
            FactKind::DateTimeMention,
            "DATE:2026-08-17"
        ));
        assert!(has(
            "2026年8月17日",
            FactKind::DateTimeMention,
            "DATE:2026-08-17"
        ));
        assert!(has(
            "8月17日截止",
            FactKind::DateTimeMention,
            "DATE:--08-17"
        ));
        // 不合法的日期不收
        assert!(norms("2026-13-45", FactKind::DateTimeMention).is_empty());
    }

    #[test]
    fn clock_times() {
        assert!(has("17:00 開會", FactKind::DateTimeMention, "TIME:17:00"));
        assert!(has("下午5點", FactKind::DateTimeMention, "TIME:17:00"));
        assert!(has("晚上8點半", FactKind::DateTimeMention, "TIME:20:30"));
        assert!(has("早上9點30分", FactKind::DateTimeMention, "TIME:09:30"));
        assert!(has("5pm", FactKind::DateTimeMention, "TIME:17:00"));
        assert!(has("9:30 AM", FactKind::DateTimeMention, "TIME:09:30"));
        assert!(norms("25:99", FactKind::DateTimeMention).is_empty());
    }

    #[test]
    fn relative_time_stays_relative() {
        // L1 不換算成絕對日期——抽取器拿不到當下時間，硬換算就是編造
        assert!(has("明天交", FactKind::DateTimeMention, "REL:+1d"));
        assert!(has("後天", FactKind::DateTimeMention, "REL:+2d"));
        assert!(has("昨天", FactKind::DateTimeMention, "REL:-1d"));
        assert!(has("三天後", FactKind::DateTimeMention, "REL:+3d"));
        assert!(has("兩週後", FactKind::DateTimeMention, "REL:+14d"));
        assert!(has("3小時後", FactKind::DateTimeMention, "REL:+3h"));
        assert!(has("30分鐘後", FactKind::DateTimeMention, "REL:+30m"));
        assert!(has("兩天前", FactKind::DateTimeMention, "REL:-2d"));
        assert!(has("下個月", FactKind::DateTimeMention, "REL:+30d"));
    }

    #[test]
    fn weekdays_beat_bare_week_offsets() {
        // 「下週三」要抽出星期三，不能被「下週」吃掉
        assert!(has("下週三開會", FactKind::DateTimeMention, "WEEKDAY:3"));
        assert!(has("星期日", FactKind::DateTimeMention, "WEEKDAY:7"));
        assert!(has("禮拜五", FactKind::DateTimeMention, "WEEKDAY:5"));
        // 沒帶星期幾時才退回週偏移
        assert!(has("下週再說", FactKind::DateTimeMention, "REL:+7d"));
        // 光一個「這週」以外的裸「週」字太模糊
        assert!(norms("週報", FactKind::DateTimeMention).is_empty());
    }

    #[test]
    fn date_and_time_coexist_in_one_line() {
        let facts = extract("2026-08-17 17:00 在會議室");
        let dt: Vec<_> = facts
            .iter()
            .filter(|f| f.kind == FactKind::DateTimeMention)
            .collect();
        assert_eq!(dt.len(), 2, "date and time are separate facts: {dt:?}");
        assert_eq!(dt[0].normalized, "DATE:2026-08-17");
        assert_eq!(dt[1].normalized, "TIME:17:00");
    }

    // ---------- 真實場景 ----------

    #[test]
    fn a_realistic_bill_screen() {
        let screen = "中華電信 帳單查詢\n\
                      本期應繳金額 NT$13,450\n\
                      繳費期限 2026/08/25\n\
                      客服專線 0800-080-123\n\
                      線上繳費 https://bill.cht.com.tw/pay";
        let facts = extract(screen);

        assert!(has(screen, FactKind::Money, "TWD:13450"));
        assert!(has(screen, FactKind::DateTimeMention, "DATE:2026-08-25"));
        assert!(has(screen, FactKind::Phone, "+886800080123"));
        assert!(has(screen, FactKind::Url, "https://bill.cht.com.tw/pay"));

        // 每個事實都必須指得回原文的確切位置
        for f in &facts {
            assert_eq!(&screen[f.byte_start..f.byte_end], f.raw);
        }
    }

    #[test]
    fn a_realistic_error_screen() {
        let screen = "Failed to compile crates/sister-core/src/db.rs\n\
                      error[E0308]: mismatched types\n\
                      caused by ECONNREFUSED at 17:42";
        assert!(has(screen, FactKind::ErrorCode, "E0308"));
        assert!(has(screen, FactKind::ErrorCode, "ECONNREFUSED"));
        assert!(has(screen, FactKind::DateTimeMention, "TIME:17:42"));
        assert!(has(
            screen,
            FactKind::FilePath,
            "crates/sister-core/src/db.rs"
        ));
    }

    // ---------- 內部工具 ----------

    #[test]
    fn cjk_number_parser() {
        assert_eq!(cjk_number("一百二十"), Some(120.0));
        assert_eq!(cjk_number("兩萬"), Some(20_000.0));
        assert_eq!(cjk_number("三萬五千"), Some(35_000.0));
        assert_eq!(cjk_number("一千兩百五十"), Some(1250.0));
        assert_eq!(cjk_number("十"), Some(10.0));
        assert_eq!(cjk_number("二十"), Some(20.0));
        assert_eq!(cjk_number("零"), Some(0.0));
        assert_eq!(cjk_number("abc"), None);
        assert_eq!(cjk_number(""), None);
    }

    #[test]
    fn amount_formatting_drops_noise_zeros() {
        assert_eq!(fmt_amount(13450.0), "13450");
        assert_eq!(fmt_amount(49.99), "49.99");
        assert_eq!(fmt_amount(89.50), "89.5");
        assert_eq!(fmt_amount(0.0), "0");
    }

    #[test]
    fn ascii_word_boundary_treats_cjk_as_a_boundary() {
        // 這正是不能用 regex \b 的原因：CJK 在 unicode 模式下是 word char
        let t = "帳單13450元";
        let start = t.find("13450").expect("digits");
        assert!(ascii_word_boundary(t, start, start + 5));

        let t2 = "ORDER13450X";
        let start2 = t2.find("13450").expect("digits");
        assert!(!ascii_word_boundary(t2, start2, start2 + 5));
    }
}
