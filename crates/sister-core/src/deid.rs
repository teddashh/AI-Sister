//! 上雲前的去識別化（SPEC §11.3）。
//!
//! `replay::Scrubber` 不是這個東西：它換成看起來像真的假值，好讓 replay
//! 還能測檢索。這裡換成 typed placeholder（`<AMT_1>`），同一原文在整份
//! 輸入裡對到同一個代號。真數字只活在本機 L1。
//!
//! 送出函式只收 [`RedactedText`]。沒走過 [`scrub`] 就送出去，編不過。

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;

use crate::facts::{FactKind, extract};
use crate::redact::looks_like_secret;

/// 去敏後、唯一能交給出境路徑的字。
///
/// 三個欄位都是私有的，`deid` 之外鑄不出來、**也改不掉**。後面那半是實測
/// 補上的：只要有一個欄位是 `pub`，外面就能拿一份合法的 `RedactedText`
/// 把 `text` 覆寫成原文再送出去，建構那道門完全繞開。
/// `scripts/check-brain-outbound.py` 現在盯著每一個欄位的可見性。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedactedText {
    text: String,
    stats: RedactionStats,
    truncated: bool,
}

impl RedactedText {
    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub fn stats(&self) -> &RedactionStats {
        &self.stats
    }

    pub fn truncated(&self) -> bool {
        self.truncated
    }

    pub fn chars(&self) -> usize {
        self.text.chars().count()
    }
}

/// 換掉了幾個什麼型別。沒碰到的是 0（真的數過）。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RedactionStats {
    pub money: usize,
    pub phone: usize,
    pub email: usize,
    pub id_like: usize,
    pub person: usize,
    pub secret: usize,
}

impl RedactionStats {
    pub fn total(&self) -> usize {
        self.money + self.phone + self.email + self.id_like + self.person + self.secret
    }
}

/// 人名要不要遮。SPEC §11.3：選配，預設開。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RedactNames(pub bool);

/// 出境的字裡「說明」那一半。**只收程式自己的常數和數字。**
///
/// 送出去的字分兩半：證據抄自螢幕，一定要去敏；說明是程式組的，跑去敏反而
/// 會把它咬爛——實測過：`facts` 的單號規則認得 `ref`，於是
/// 「segment_ref：segment:178…」裡的 `segment` 被換成 `<ID_1>`，模型再也
/// 回不出對得上的 segment_ref，**每一張卡片都會被判成對不上**。
///
/// 所以說明不去敏，改成讓使用者的字**進不來**：[`Self::lit`] 只收
/// `&'static str`，其餘只能從 [`Self::int`]／[`Self::float`] 進。哪天有人
/// 想把視窗標題放進說明，那是 `String`，編不過——不是「不該做」，是做不到。
#[derive(Debug, Default)]
pub struct PromptHeader(String);

impl PromptHeader {
    pub fn new() -> Self {
        Self::default()
    }

    /// 編譯期字面值。螢幕上的字沒有辦法變成 `&'static str`。
    pub fn lit(&mut self, s: &'static str) -> &mut Self {
        self.0.push_str(s);
        self
    }

    /// 數字沒有藏自由文字的空間。
    pub fn int(&mut self, v: i64) -> &mut Self {
        self.0.push_str(&v.to_string());
        self
    }

    pub fn float(&mut self, v: f64) -> &mut Self {
        self.0.push_str(&v.to_string());
        self
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 把去敏後的證據接在說明後面。
pub fn with_header(header: &PromptHeader, body: RedactedText) -> RedactedText {
    RedactedText {
        text: format!("{}{}", header.0, body.text),
        stats: body.stats,
        truncated: body.truncated,
    }
}

/// 把 `raw` 裡的敏感片段換成 typed placeholder。同一原文對同一代號。
pub fn scrub(raw: &str, names: RedactNames) -> RedactedText {
    scrub_limited(raw, names, None)
}

/// 超過 `max_bytes` 就在字元邊界截斷，並把 [`RedactedText::truncated`] 設成 true。
pub fn scrub_limited(raw: &str, names: RedactNames, max_bytes: Option<usize>) -> RedactedText {
    let (cut, truncated) = match max_bytes {
        Some(n) => crate::redact::truncate_utf8(raw, n),
        None => (raw, false),
    };
    let mut text = cut.to_string();
    let mut stats = RedactionStats::default();
    let mut map: BTreeMap<(Kind, String), usize> = BTreeMap::new();

    replace_secrets(&mut text, &mut map, &mut stats);
    replace_facts(&mut text, &mut map, &mut stats);
    replace_idish(&mut text, &mut map, &mut stats);
    if names.0 {
        replace_names(&mut text, &mut map, &mut stats);
    }

    RedactedText {
        text,
        stats,
        truncated,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Money,
    Phone,
    Email,
    IdLike,
    Person,
    Secret,
}

impl Kind {
    fn tag(self) -> &'static str {
        match self {
            Kind::Money => "AMT",
            Kind::Phone => "PHONE",
            Kind::Email => "EMAIL",
            Kind::IdLike => "ID",
            Kind::Person => "PERSON",
            Kind::Secret => "SECRET",
        }
    }
}

fn placeholder(kind: Kind, n: usize) -> String {
    format!("<{}_{}>", kind.tag(), n)
}

fn assign_id(map: &mut BTreeMap<(Kind, String), usize>, kind: Kind, raw: &str) -> usize {
    if let Some(n) = map.get(&(kind, raw.to_string())) {
        return *n;
    }
    let n = map
        .iter()
        .filter(|((k, _), _)| *k == kind)
        .map(|(_, n)| *n)
        .max()
        .unwrap_or(0)
        + 1;
    map.insert((kind, raw.to_string()), n);
    n
}

fn replace_range(text: &mut String, start: usize, end: usize, with: &str) {
    text.replace_range(start..end, with);
}

fn replace_secrets(
    text: &mut String,
    map: &mut BTreeMap<(Kind, String), usize>,
    stats: &mut RedactionStats,
) {
    if looks_like_secret(text).is_none() {
        return;
    }

    let original = text.clone();
    let mut out = String::with_capacity(original.len());
    let mut found = 0usize;
    let mut last = 0usize;
    for (i, c) in original.char_indices() {
        let keep = c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '+' | '/' | '.');
        if !keep {
            if last < i {
                let token = &original[last..i];
                if looks_like_secret(token).is_some() {
                    let n = assign_id(map, Kind::Secret, token);
                    out.push_str(&placeholder(Kind::Secret, n));
                    found += 1;
                } else {
                    out.push_str(token);
                }
            }
            out.push(c);
            last = i + c.len_utf8();
        }
    }
    if last < original.len() {
        let token = &original[last..];
        if looks_like_secret(token).is_some() {
            let n = assign_id(map, Kind::Secret, token);
            out.push_str(&placeholder(Kind::Secret, n));
            found += 1;
        } else {
            out.push_str(token);
        }
    }

    if found == 0 {
        let n = assign_id(map, Kind::Secret, &original);
        *text = placeholder(Kind::Secret, n);
        stats.secret += 1;
        return;
    }
    *text = out;
    stats.secret += found;
}

fn replace_facts(
    text: &mut String,
    map: &mut BTreeMap<(Kind, String), usize>,
    stats: &mut RedactionStats,
) {
    let facts = extract(text);
    for fact in facts.into_iter().rev() {
        let kind = match fact.kind {
            FactKind::Money => Kind::Money,
            FactKind::Phone => Kind::Phone,
            FactKind::Email => Kind::Email,
            FactKind::IdLike => Kind::IdLike,
            _ => continue,
        };
        let n = assign_id(map, kind, &fact.raw);
        replace_range(text, fact.byte_start, fact.byte_end, &placeholder(kind, n));
        match kind {
            Kind::Money => stats.money += 1,
            Kind::Phone => stats.phone += 1,
            Kind::Email => stats.email += 1,
            Kind::IdLike => stats.id_like += 1,
            _ => {}
        }
    }
}

/// 看起來像編號的東西：夾雜字母與數字、長到不像日常字詞的一串。
///
/// 這條**故意比 [`crate::facts`] 寬**。`facts` 抽的是「值得記住的事實」，
/// 抓錯了是時間軸上多一行雜訊，所以它偏精確：`RE_ORDER` 要先看到
/// 「訂單編號」「發票號碼」這種前綴才認。但這裡是隱私過濾，抓漏了是
/// **東西送出去了**——兩邊要的誤差方向相反，同一支函式服務不了兩個。
///
/// 實測漏掉的那一個：「發票編號 AB-12345678」。`RE_ORDER` 的白名單有
/// 「發票號碼」和「訂單編號」，就是沒有「發票編號」，差一個字就送出去了。
/// 白名單擋不住這種事，所以這裡不用白名單。
static RE_IDISH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[A-Za-z0-9][A-Za-z0-9\-]{6,}[A-Za-z0-9]").expect("idish regex"));

fn looks_like_id(s: &str) -> bool {
    let digits = s.bytes().filter(|b| b.is_ascii_digit()).count();
    let alphas = s.bytes().filter(|b| b.is_ascii_alphabetic()).count();
    // 純數字放過：epoch 毫秒、年份、行號都長這樣，而說明裡到處都是。
    // 純字母放過：那是英文單字。要兩種都有，才像人為編出來的號碼。
    digits >= 2 && alphas >= 1
}

/// 補掉 [`replace_facts`] 沒認出來的編號。
fn replace_idish(
    text: &mut String,
    map: &mut BTreeMap<(Kind, String), usize>,
    stats: &mut RedactionStats,
) {
    let spans: Vec<(usize, usize, String)> = RE_IDISH
        .find_iter(text)
        .filter(|m| looks_like_id(m.as_str()))
        .map(|m| (m.start(), m.end(), m.as_str().to_string()))
        .collect();
    for (start, end, raw) in spans.into_iter().rev() {
        let n = assign_id(map, Kind::IdLike, &raw);
        replace_range(text, start, end, &placeholder(Kind::IdLike, n));
        stats.id_like += 1;
    }
}

/// 常見複姓要排在單姓前面，否則「歐陽」會被「歐」吃掉。
const SURNAMES: &[&str] = &[
    "歐陽", "司馬", "諸葛", "上官", "東方", "夏侯", "公孫", "慕容", "司徒", "司空", "王", "李",
    "張", "劉", "陳", "楊", "黃", "趙", "吳", "周", "徐", "孫", "馬", "朱", "胡", "郭", "何", "林",
    "羅", "高", "梁", "鄭", "謝", "宋", "唐", "許", "鄧", "馮", "韓", "曹", "曾", "彭", "蕭", "蔡",
    "潘", "田", "董", "袁", "于", "余", "蔣", "杜", "葉", "魏", "蘇", "呂", "丁", "任", "沈", "姚",
    "盧", "傅", "姜", "崔", "譚", "廖", "范", "汪", "陸", "金", "石", "戴", "賈", "韋", "夏", "邱",
    "方", "侯", "鄒", "熊", "孟", "秦", "白", "江", "閻", "薛", "尹", "段", "雷", "黎", "史", "龍",
    "賀", "陶", "顧", "毛", "郝", "龔", "邵", "鍾", "錢", "嚴",
];

const NAME_SKIP: &[&str] = &[
    "時間", "時候", "時代", "事情", "王子", "王國", "高峰", "未來", "黃金", "金價", "上海", "東京",
    "林間", "高尚", "高大", "金門", "金句", "金魚", "白天", "白金", "石頭", "石門", "江山", "江南",
    "來回", "歷史",
];

static RE_CN_NAME: LazyLock<Regex> = LazyLock::new(|| {
    let alts = SURNAMES.join("|");
    // 沒有稱謂或上下文，單姓加一兩個字會把「這一段」「第一張卡片」整句咬掉，
    // 所以這裡認的是「前面有領頭詞」或「後面有稱謂」，不是每個姓都咬。
    //
    // 領頭詞不只有動詞。實測漏掉的是「聯絡人王小明 0912-…」——電話換成了
    // <PHONE_1>，名字原封不動送出去。名片、簽名檔、客戶清單上的名字幾乎都是
    // 這個形狀（欄位標籤＋人名），所以標籤要一起算領頭詞。
    // 只有名字那一段進 capture group：領頭詞和稱謂要留在原地。
    // 「聯絡人王小明」整串換掉會變成孤零零一個 <PERSON_1>，模型連那是誰的
    // 什麼角色都不知道；留成「聯絡人<PERSON_1>」意思還在，名字還是沒出去。
    Regex::new(&format!(
        r"(?:(?:跟|和|請|找|向|給|致|由|聯絡人|連絡人|負責人|收件人|寄件人|申請人|客戶|窗口|對方|業務)\s*[:：]?\s*((?:{alts})\p{{Han}}{{1,2}}))|(?:((?:{alts})\p{{Han}}{{1,2}})(?:先生|小姐|女士|老師|同學|經理|主任|醫師|律師))"
    ))
    .expect("cn name")
});

static RE_EN_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b[A-Z][a-z]{1,20}(?:\s+[A-Z][a-z]{1,20})+\b").expect("en name"));

const EN_SKIP_FIRST: &[&str] = &[
    "The", "This", "That", "These", "Those", "When", "After", "From", "With", "Your", "Please",
    "Error", "File", "Line", "Note", "New", "Old", "See", "Use", "For", "And", "But", "Not", "All",
    "Any", "Can", "May", "Will", "Type", "Name", "User", "Test", "True", "False", "None", "Null",
    "Windows", "Linux", "Google", "GitHub", "HTTP", "JSON", "Open", "Save", "Read", "Write",
    "Click", "Press",
];

fn replace_names(
    text: &mut String,
    map: &mut BTreeMap<(Kind, String), usize>,
    stats: &mut RedactionStats,
) {
    let mut spans: Vec<(usize, usize, String)> = Vec::new();
    for c in RE_CN_NAME.captures_iter(text) {
        // 兩個分支（領頭詞在前／稱謂在後）各有一個 group，中一個。
        let Some(name) = c.get(1).or_else(|| c.get(2)) else {
            continue;
        };
        if NAME_SKIP.contains(&name.as_str()) {
            continue;
        }
        spans.push((name.start(), name.end(), name.as_str().to_string()));
    }
    for m in RE_EN_NAME.find_iter(text) {
        let first = m.as_str().split_whitespace().next().unwrap_or("");
        if EN_SKIP_FIRST.contains(&first) {
            continue;
        }
        spans.push((m.start(), m.end(), m.as_str().to_string()));
    }
    spans.sort_by_key(|(s, _, _)| *s);
    let mut kept = Vec::new();
    let mut last_end = 0usize;
    for span in spans {
        if span.0 >= last_end {
            last_end = span.1;
            kept.push(span);
        }
    }
    for (start, end, raw) in kept.into_iter().rev() {
        let n = assign_id(map, Kind::Person, &raw);
        replace_range(text, start, end, &placeholder(Kind::Person, n));
        stats.person += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 這一整段是 alpha.57 驗收時真的送出去過的字。用假的 CLI 收下來一看，
    /// 金額、電話、email、API key 都換成代號了，**發票編號和人名原封不動**。
    /// 兩個都是 SPEC §11.3 點名的東西。
    const LEAKED: &str = "客戶匯款 NT$450,000 已入帳，聯絡人王小明 0912-345-678，\
                          email ming.wang@example.com，發票編號 AB-12345678，\
                          API key sk-live-9f3a2b1c7d8e4f5a6b0c1d2e";

    #[test]
    fn nothing_from_the_measured_leak_goes_out_again() {
        let out = scrub(LEAKED, RedactNames(true));
        for raw in [
            "450,000",
            "0912-345-678",
            "ming.wang@example.com",
            "AB-12345678",
            "sk-live-9f3a2b1c",
            "王小明",
        ] {
            assert!(!out.as_str().contains(raw), "{raw} 還在：{}", out.as_str());
        }
    }

    #[test]
    fn an_invoice_number_no_keyword_matches_is_still_an_id() {
        // `facts::RE_ORDER` 的白名單有「發票號碼」和「訂單編號」，就是沒有
        // 「發票編號」。隱私過濾不能靠白名單。
        let out = scrub("發票編號 AB-12345678", RedactNames(false));
        assert!(out.as_str().contains("<ID_1>"), "{}", out.as_str());
        assert_eq!(out.stats().id_like, 1);
    }

    #[test]
    fn a_labelled_name_is_masked_but_the_label_stays() {
        let out = scrub("聯絡人王小明", RedactNames(true));
        assert_eq!(out.as_str(), "聯絡人<PERSON_1>", "領頭詞不該被一起吃掉");
    }

    #[test]
    fn epoch_millis_are_not_ids() {
        // 說明裡到處都是 epoch 毫秒。純數字不算編號，否則整份 prompt 會變成
        // 一排代號，模型連時間都讀不到。
        let out = scrub(
            "時間：1787597100000 – 1787598600000（epoch 毫秒）",
            RedactNames(false),
        );
        assert!(out.as_str().contains("1787597100000"), "{}", out.as_str());
        assert_eq!(out.stats().id_like, 0);
    }

    #[test]
    fn the_header_survives_intact() {
        // 說明不去敏，所以 segment_ref 要一個字不差地留著。曾經有一版把說明
        // 也送進去敏，`facts` 認得 `ref` 是單號關鍵字，於是「segment_ref：
        // segment:…」的 `segment` 變成 `<ID_1>`——模型回不出對得上的 ref，
        // 每一張卡片都被判成對不上，而且外面看起來只是「模型很笨」。
        let mut h = PromptHeader::new();
        h.lit("本段 segment_ref：segment:").int(1787597100000);
        let out = with_header(&h, scrub("證據", RedactNames(false)));
        assert!(
            out.as_str().contains("segment:1787597100000"),
            "{}",
            out.as_str()
        );
    }

    #[test]
    fn money_phone_email_id_become_typed_placeholders() {
        let out = scrub(
            "帳單 NT$13,450 打 0912-345-678 到 ted@example.com 訂單編號 TW-20260817-001",
            RedactNames(false),
        );
        assert!(out.as_str().contains("<AMT_1>"), "{}", out.as_str());
        assert!(out.as_str().contains("<PHONE_1>"), "{}", out.as_str());
        assert!(out.as_str().contains("<EMAIL_1>"), "{}", out.as_str());
        assert!(out.as_str().contains("<ID_1>"), "{}", out.as_str());
        assert!(!out.as_str().contains("13,450"), "{}", out.as_str());
        assert!(!out.as_str().contains("0912"), "{}", out.as_str());
        assert!(!out.as_str().contains("ted@"), "{}", out.as_str());
        assert_eq!(out.stats().money, 1);
        assert_eq!(out.stats().phone, 1);
        assert_eq!(out.stats().email, 1);
        assert_eq!(out.stats().id_like, 1);
    }

    #[test]
    fn the_same_original_keeps_the_same_placeholder() {
        let out = scrub("NT$80 然後又是 NT$80", RedactNames(false));
        let hits: Vec<_> = out.as_str().matches("<AMT_1>").collect();
        assert_eq!(hits.len(), 2, "{}", out.as_str());
        assert!(!out.as_str().contains("<AMT_2>"), "{}", out.as_str());
        assert_eq!(out.stats().money, 2, "兩處都換了，但代號相同");
    }

    #[test]
    fn names_are_on_by_default_and_can_be_turned_off() {
        let on = scrub("請王小明先生在五點前打給 John Smith", RedactNames(true));
        assert!(on.as_str().contains("<PERSON_"), "{}", on.as_str());
        assert!(!on.as_str().contains("王小明"), "{}", on.as_str());
        assert!(!on.as_str().contains("John Smith"), "{}", on.as_str());
        assert!(on.stats().person >= 2, "{:?}", on.stats());

        let off = scrub("請王小明先生在五點前打給 John Smith", RedactNames(false));
        assert!(off.as_str().contains("王小明"), "{}", off.as_str());
        assert_eq!(off.stats().person, 0);
    }

    #[test]
    fn measure_words_are_not_surnames() {
        let out = scrub(
            "根據下面這段去識別化後的證據，產出第一張卡片。",
            RedactNames(true),
        );
        assert!(
            out.as_str().contains("這段去識別化"),
            "「段」被當成姓了：{}",
            out.as_str()
        );
        assert!(
            out.as_str().contains("第一張卡片"),
            "「張」被當成姓了：{}",
            out.as_str()
        );
        assert_eq!(out.stats().person, 0);
    }

    #[test]
    fn secrets_are_redacted_too() {
        let out = scrub("export KEY=ghp_16CharsAtLeastHereOk123", RedactNames(false));
        assert!(out.as_str().contains("<SECRET_1>"), "{}", out.as_str());
        assert!(!out.as_str().contains("ghp_"), "{}", out.as_str());
        assert_eq!(out.stats().secret, 1);
    }

    #[test]
    fn ordinary_prose_is_left_alone() {
        let out = scrub("在 Cloudflare dashboard 設定 DNS 記錄", RedactNames(true));
        assert_eq!(out.as_str(), "在 Cloudflare dashboard 設定 DNS 記錄");
        assert_eq!(out.stats().total(), 0);
        assert!(!out.truncated());
    }

    #[test]
    fn truncation_is_recorded_not_guessed() {
        let out = scrub_limited("帳單金額很大", RedactNames(false), Some(6));
        assert!(out.truncated());
        assert!(out.as_str().chars().count() < "帳單金額很大".chars().count());
        let intact = scrub_limited("短", RedactNames(false), Some(100));
        assert!(!intact.truncated());
    }

    #[test]
    fn time_is_not_mistaken_for_a_person() {
        let out = scrub("這個時候不要去", RedactNames(true));
        assert!(
            out.as_str().contains("時候"),
            "「時候」被當成名字了：{}",
            out.as_str()
        );
    }
}
