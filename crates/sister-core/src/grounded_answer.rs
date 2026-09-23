//! S1 問答的 CLI-directed 本機檢索與成句層。
//!
//! 第一階段只把問題交給使用者選用的 CLI，驗回最多三條自然語言查詢；AI-Sister
//! 在本機執行後，第二階段才把**這一題命中的文字與附近的理解記憶**整理成有界 prompt
//! 交回同一支 CLI。每一句都必須引用這次候選裡的 exact source ref；模型不能新增來源，
//! 也不能把沒有來源的句子塞進答案。

use std::collections::HashSet;

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

use crate::answer::Answer as FactAnswer;
use crate::model::{Millis, SearchHit, SourceKind};

/// 一題最多交給 CLI 的本機候選數。先選判讀與 facts，再用全文命中補足；選完後會按
/// 時間排序，讓模型真的看得到前後順序。
pub const MAX_SOURCES: usize = 18;
/// 問題本身的 byte 上限。檢索仍用完整問題；只有送進 CLI 的副本有界。
pub const MAX_QUESTION_BYTES: usize = 2 * 1024;
/// 圍欄內 question + evidence 的 byte 上限。
pub const MAX_DATA_BYTES: usize = 24 * 1024;
/// 單筆來源正文的 raw byte 上限。其後還會再守 JSON 編碼後的大小。
pub const MAX_SOURCE_TEXT_BYTES: usize = 4 * 1024;
const MAX_QUESTION_JSON_BYTES: usize = 3 * 1024;
const MAX_SOURCE_TEXT_JSON_BYTES: usize = 5 * 1024;
const MAX_SOURCE_META_JSON_BYTES: usize = 512;
/// 簡單事實仍可只答一句；需要串前後文時最多六句。
pub const MAX_SENTENCES: usize = 6;
/// 單句上限，避免 CLI 把整篇文章塞進一格。
pub const MAX_SENTENCE_CHARS: usize = 320;
/// The selected CLI can ask AI-Sister to run at most this many local-memory searches for one
/// question. The CLI never receives a database path and never opens the SQLite file itself.
pub const MAX_SEARCH_QUERIES: usize = 3;
/// A search is natural-language input to the same retrieval path as `sister query`.
pub const MAX_SEARCH_QUERY_CHARS: usize = 240;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SourceRef {
    Fact(i64),
    Chunk(i64),
    /// 她自己稍早想過的那一張（`l2_card`）。
    ///
    /// 和另外兩種的差別不是格式，是**它不是螢幕上的字**：那兩種是當時真的
    /// 記下來的原文，這一種是她看著那些東西想出來的結論。畫面上那一
    /// 排出處也要照這個分——把判讀印成「畫面 #42」，等於宣稱螢幕上寫過這
    /// 句話。
    Card(i64),
}

impl SourceRef {
    pub fn as_str(&self) -> String {
        match self {
            Self::Fact(id) => format!("fact:{id}"),
            Self::Chunk(id) => format!("chunk:{id}"),
            Self::Card(id) => format!("card:{id}"),
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        let (kind, raw_id) = value.split_once(':')?;
        let id = raw_id.parse::<i64>().ok()?;
        if id <= 0 {
            return None;
        }
        match kind {
            "fact" => Some(Self::Fact(id)),
            "chunk" => Some(Self::Chunk(id)),
            "card" => Some(Self::Card(id)),
            _ => None,
        }
    }
}

/// ref 只用來找回紀錄，不能用 fact／chunk 來猜文字是從哪裡讀到的。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceOrigin {
    Recorded(SourceKind),
    Reading,
    /// 舊資料或無法辨認的 fact 來源；不把它升格成 OCR。
    Unknown,
}

impl SourceOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Recorded(kind) => kind.as_str(),
            Self::Reading => "reading",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub reference: SourceRef,
    pub origin: SourceOrigin,
    pub ts: Millis,
    pub text: String,
    pub app: Option<String>,
    pub title: Option<String>,
    pub url: Option<String>,
    pub frame_id: Option<i64>,
}

impl Source {
    fn from_fact(answer: &FactAnswer) -> Self {
        let row = &answer.latest;
        let text = if row.raw == row.normalized {
            format!("{}（看過 {} 次）", row.raw, answer.sightings)
        } else {
            format!(
                "{}；畫面原文：{}（看過 {} 次）",
                row.normalized, row.raw, answer.sightings
            )
        };
        Self {
            reference: SourceRef::Fact(row.id),
            origin: SourceKind::from_str_kind(&row.source_kind)
                .map(SourceOrigin::Recorded)
                .unwrap_or(SourceOrigin::Unknown),
            ts: row.ts,
            text,
            app: row.app_id.clone(),
            title: row.window_title.clone(),
            url: row.url.clone(),
            frame_id: row.frame_id,
        }
    }

    fn from_hit(hit: &SearchHit) -> Self {
        Self {
            reference: SourceRef::Chunk(hit.chunk_id),
            origin: SourceOrigin::Recorded(hit.source_kind),
            ts: hit.ts,
            text: hit.text.clone(),
            app: hit.app_id.clone(),
            title: hit.window_title.clone(),
            url: hit.url.clone(),
            frame_id: hit.frame_id,
        }
    }

    fn from_reading(reading: &Reading) -> Self {
        Self {
            reference: SourceRef::Card(reading.card_id),
            origin: SourceOrigin::Reading,
            ts: reading.at,
            text: reading.activity.clone(),
            // 判讀沒有 app／title／url：它不是在哪個視窗上看到的一行字，
            // 是一整段時間的結論。硬塞一個 app 進去會讓模型以為那是出處。
            app: None,
            title: None,
            url: None,
            frame_id: reading.frame_id,
        }
    }
}

/// 她稍早自己想過的一段（`l2_card` 的窄視圖）。
///
/// **這是這一版的重點。** 以前答題只拿得到兩種東西：命中的事實、命中的原文
/// ——兩種都是螢幕上的字。於是她答出來的東西長這樣：
///
/// > 你今天早上 5:33 在 AI-Sister 收到「Flood watch」，旁邊寫著 Just issued。
///
/// 每個字都真，而那是在念紀錄。朋友會說的是「你早上在追一個天氣警報」——
/// 那句話不在任何一格螢幕文字裡，它是**看懂了**才講得出來的。
///
/// 她其實一直在想這件事：解釋層平常沒事就會自己醒過來，看著剛過去那一段寫
/// 一張卡（那一場錄製的收尾寫著「解釋層自己醒了 14 次，跑了 16 次，寫進 16
/// 張假設」）。**只是答題那條路從來沒有去讀它。** 這個型別就是那條路。
///
/// 順帶把「問一題要等多久」也拉下來一截：那句判讀是閒著的時候想好的，不是
/// 他按下 Enter 之後才開始想的。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reading {
    pub card_id: i64,
    /// 被問題本身的字找到，而非時間背景；兩條路重疊時仍為 true。
    /// 內容命中直接呈現，時間背景只交給模型當材料。
    pub matched: bool,
    /// 這一段是什麼時候開始的（`segment_core_start`）。
    pub at: Millis,
    /// 她當時寫下的那句話。
    pub activity: String,
    /// 這張卡自己的證據裡第一張畫面。
    ///
    /// 有的話，畫面上那顆出處鍵就點得開——他讀到一句判讀，可以當場去看她
    /// 是看著什麼想出來的。沒有的話，內容命中的卡可捲到原句；時間背景
    /// 沒有自己的列，出處只顯示文字。
    pub frame_id: Option<i64>,
}

/// 每個建構端明確選擇卡片來源，避免布林參數接反。
#[derive(Debug, Clone, Copy)]
pub enum ReadingOrigin {
    Time,
    Content,
}

impl Reading {
    /// 從一張 L2 卡取出答題用得到的那幾格。
    ///
    /// `evidence_json` 是一個 `["frame:12","fact:7"]` 這樣的陣列，格式由
    /// [`crate::brain::EvidenceRef`] 定義；這裡只取**第一張畫面**。取不到
    /// 就是 `None`，不是錯——一張只靠 fact 立起來的卡仍然是一句判讀。
    pub fn from_card(card: &crate::db::L2CardRow, origin: ReadingOrigin) -> Self {
        let frame_id = serde_json::from_str::<Vec<String>>(&card.evidence_json)
            .unwrap_or_default()
            .iter()
            .find_map(|raw| match crate::brain::EvidenceRef::parse(raw) {
                Some(crate::brain::EvidenceRef::Frame(id)) => Some(id),
                _ => None,
            });
        Self {
            card_id: card.id,
            matched: matches!(origin, ReadingOrigin::Content),
            at: card.segment_core_start,
            activity: card.activity.clone(),
            frame_id,
        }
    }
}

/// MAX_READINGS 的 8 張中，內容命中最多 4 張，保留另一半給時間背景。
pub const MATCHED_READINGS: usize = 4;

/// 在既有時間候選之後查每條原問句／planner 查詢。先保住內容命中，再補時間
/// 背景；挑選優先序不等於呈現順序，最後依段落開始時間排列。
pub fn match_answer_readings(
    db: &crate::db::Db,
    questions: &[String],
    time_rows: Vec<crate::db::L2CardRow>,
    mut truncated: bool,
) -> anyhow::Result<(Vec<Reading>, bool)> {
    let mut batches = Vec::new();
    for question in questions {
        let (rows, more) = db.search_readings(question, MATCHED_READINGS)?;
        truncated |= more;
        batches.push(rows);
    }
    let mut selected = Vec::new();
    let mut ids = std::collections::HashSet::new();
    // 與 facts／hits 一樣輪流取各查詢的排名，避免第一條查詢吃滿配額。
    for rank in 0..MATCHED_READINGS {
        for batch in &batches {
            if let Some(row) = batch.get(rank)
                && ids.insert(row.id)
            {
                if selected.len() < MATCHED_READINGS {
                    selected.push(Reading::from_card(row, ReadingOrigin::Content));
                } else {
                    truncated = true;
                }
            }
        }
    }
    for row in &time_rows {
        if let Some(existing) = selected.iter_mut().find(|r| r.card_id == row.id) {
            existing.matched = true;
        } else if ids.contains(&row.id) {
            // 超過四張內容配額的命中已標 truncated，不能改裝成時間背景再送出。
            truncated = true;
        } else if selected.len() < MAX_READINGS {
            selected.push(Reading::from_card(row, ReadingOrigin::Time));
        } else {
            truncated = true;
        }
    }
    selected.sort_by_key(|r| (r.at, r.card_id));
    Ok((selected, truncated))
}

#[derive(Debug, Clone, PartialEq)]
pub struct Prepared {
    pub payload: String,
    pub sources: Vec<Source>,
    pub truncated: bool,
}

#[derive(Serialize)]
struct PromptQuestion<'a> {
    question: &'a str,
}

/// 送進 prompt 的一筆來源。
///
/// 時間欄位是 `at` 而不是原始的 epoch 毫秒，理由是使用者讀得到的那句話：模型
/// 拿到 `1757556873000` 講不出「你早上 10:14 要求…」，只講得出「你要求過…」。
/// 格式借 [`crate::model::stamp`]，那是這個專案唯一一份「時刻長什麼樣」。
#[derive(Serialize)]
struct PromptSource<'a> {
    r#ref: String,
    /// 實際讀字來源或她自己的判讀；圖檔是否仍保留是另一個問題。
    /// 值出自 [`SourceOrigin`]，不是從 ref 的 fact／chunk 前綴推測。
    kind: &'static str,
    at: String,
    text: &'a str,
    app: Option<&'a str>,
    title: Option<&'a str>,
    url: Option<&'a str>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelSearchPlan {
    queries: Vec<String>,
}

/// 一題最多帶幾張她自己的判讀。
///
/// 八張可以讓一個跨數段的問題看見開頭、轉折與結果，又不會吃掉全部螢幕原文。
pub const MAX_READINGS: usize = 8;
/// 找判讀的時候，往這一題的證據兩邊各放寬多久。
///
/// 一段活動的卡掛在它**開始**那一刻上，而證據落在那一段的中間；不放寬的話，
/// 只命中一行字的那一種問法會算出一個零長度的窗，什麼都撈不到。
pub const READING_SLACK_MS: Millis = 30 * 60 * 1000;

/// 這一題翻出來的證據落在哪一段時間裡。
///
/// `None` = 一筆證據都沒有，那也就沒有什麼判讀好找。回傳已經放寬過
/// （[`READING_SLACK_MS`]），可以直接餵給 [`crate::db::Db::readings_covering`]。
pub fn evidence_window(facts: &[FactAnswer], hits: &[SearchHit]) -> Option<(Millis, Millis)> {
    let stamps = facts
        .iter()
        .map(|answer| answer.latest.ts)
        .chain(hits.iter().map(|hit| hit.ts));
    let mut low: Option<Millis> = None;
    let mut high: Option<Millis> = None;
    for ts in stamps {
        low = Some(low.map_or(ts, |seen: Millis| seen.min(ts)));
        high = Some(high.map_or(ts, |seen: Millis| seen.max(ts)));
    }
    // `saturating_*`：時間戳理論上都是正的，但一筆壞資料不該讓答題整條炸掉。
    Some((
        low?.saturating_sub(READING_SLACK_MS),
        high?.saturating_add(READING_SLACK_MS),
    ))
}

/// Ask the configured CLI which local-memory lookups it needs before it writes an answer.
///
/// Only the user's current question is present at this stage. The CLI returns bounded search
/// strings; AI-Sister executes them against its own database and supplies only matching rows to
/// the answer stage. This gives the CLI control of retrieval without handing an opaque third-party
/// process the database file or the rest of the user's filesystem.
pub fn prepare_search_plan(question: &str) -> Result<String> {
    ensure!(!question.trim().is_empty(), "記憶查詢問題不可為空");
    let (question, _) = bounded_json_string(question, MAX_QUESTION_BYTES, MAX_QUESTION_JSON_BYTES);
    let data = serde_json::to_string(&PromptQuestion {
        question: &question,
    })?;
    let (fenced, _) = crate::prompt_fence::fence_question_and_evidence(&data, MAX_DATA_BYTES)?;
    let header = concat!(
        "你是 AI-Sister 的本機記憶查詢代理。先決定要如何搜尋使用者自己的本機記憶，不要回答問題。\n",
        "只輸出一個 JSON 物件，不要 markdown、不要前後解說。\n",
        "契約：{\"queries\":[\"一條可直接交給 AI-Sister 記憶搜尋的問句\"]}\n",
        "queries 必須有 1 到 3 條；每條用具體關鍵字或清楚的中文時間問法，例如『剛剛發生什麼事』或『昨天下午在做什麼』。\n",
        "保留姓名、產品名、號碼與時間範圍；可以增加同義詞查詢，但不要放命令、路徑、SQL 或答案。\n",
        "如果原問句已經適合搜尋，直接把它列為第一條。\n\n",
        "如果問題在問一段事情、原因、結果、後來怎樣或目前狀態，不要把三條都寫成同義詞：第一條找主事件，另外兩條分別找事情發生前的線索與之後的變化／結果。\n",
        "如果只是查一個號碼、金額或名稱，一條精確查詢就夠，不要硬湊三條。\n\n",
    );
    Ok(format!("{header}{fenced}"))
}

/// Parse the CLI's requested local-memory searches. Unknown fields, empty strings, excessive
/// counts and oversized queries reject the whole plan rather than widening it silently.
pub fn parse_search_plan(stdout: &str) -> std::result::Result<Vec<String>, String> {
    let value: serde_json::Value = serde_json::from_str(stdout.trim())
        .map_err(|error| format!("stdout 不是單一 JSON 物件：{error}"))?;
    if !value.is_object() {
        return Err("stdout 不是單一 JSON 物件".to_owned());
    }
    let model: ModelSearchPlan =
        serde_json::from_value(value).map_err(|error| format!("JSON 對不上查詢契約：{error}"))?;
    if model.queries.is_empty() || model.queries.len() > MAX_SEARCH_QUERIES {
        return Err(format!(
            "queries 必須是 1 到 {MAX_SEARCH_QUERIES} 條，實際是 {}",
            model.queries.len()
        ));
    }

    let mut queries = Vec::with_capacity(model.queries.len());
    let mut seen = HashSet::new();
    for raw in model.queries {
        let query = raw.trim();
        if query.is_empty() {
            return Err("queries 裡有空問句".to_owned());
        }
        if query.chars().any(char::is_control) {
            return Err("queries 裡有控制字元".to_owned());
        }
        if query.chars().count() > MAX_SEARCH_QUERY_CHARS {
            return Err(format!("單條 query 超過 {MAX_SEARCH_QUERY_CHARS} 字"));
        }
        if seen.insert(query.to_owned()) {
            queries.push(query.to_owned());
        }
    }
    if queries.is_empty() {
        return Err("queries 去重後是空的".to_owned());
    }
    Ok(queries)
}

/// 多條查詢各自有一份已排好相關性的結果。合併時要一輪拿每條的第一名、再一輪拿
/// 每條的第二名；不能讓第一條先塞滿總上限，否則 planner 特地找的前因／後果永遠
/// 只存在於 audit 裡，進不了真正的 RAG prompt。
pub fn merge_ranked_facts(batches: Vec<Vec<FactAnswer>>, limit: usize) -> Vec<FactAnswer> {
    interleave_unique(batches, limit, |answer| answer.latest.id)
}

/// [`merge_ranked_facts`] 的全文命中版本。
pub fn merge_ranked_hits(batches: Vec<Vec<SearchHit>>, limit: usize) -> Vec<SearchHit> {
    interleave_unique(batches, limit, |hit| hit.chunk_id)
}

fn interleave_unique<T, F>(batches: Vec<Vec<T>>, limit: usize, key: F) -> Vec<T>
where
    F: Fn(&T) -> i64,
{
    if limit == 0 {
        return Vec::new();
    }
    let mut batches: Vec<_> = batches.into_iter().map(|batch| batch.into_iter()).collect();
    let mut merged = Vec::with_capacity(limit);
    let mut seen = HashSet::new();
    loop {
        let mut advanced = false;
        for batch in &mut batches {
            if let Some(item) = batch.next() {
                advanced = true;
                if seen.insert(key(&item)) {
                    merged.push(item);
                    if merged.len() == limit {
                        return merged;
                    }
                }
            }
        }
        if !advanced {
            return merged;
        }
    }
}

/// JSON string 可能把一個 control byte 展開成六個 ASCII bytes。只守 raw UTF-8
/// 上限仍可能讓第一筆來源完全放不進 payload，所以每個自由字串也守 encoded size。
fn bounded_json_string(value: &str, raw_bytes: usize, encoded_bytes: usize) -> (String, bool) {
    let (raw, raw_truncated) = crate::redact::truncate_utf8(value, raw_bytes);
    if serde_json::to_string(raw).is_ok_and(|json| json.len() <= encoded_bytes) {
        return (raw.to_owned(), raw_truncated);
    }

    let mut end = raw.len();
    while end > 0 {
        end = end.saturating_mul(3) / 4;
        while end > 0 && !raw.is_char_boundary(end) {
            end -= 1;
        }
        if serde_json::to_string(&raw[..end]).is_ok_and(|json| json.len() <= encoded_bytes) {
            return (raw[..end].to_owned(), true);
        }
    }
    (String::new(), !value.is_empty())
}

fn bound_source(mut source: Source) -> (Source, bool) {
    let (text, mut truncated) = bounded_json_string(
        &source.text,
        MAX_SOURCE_TEXT_BYTES,
        MAX_SOURCE_TEXT_JSON_BYTES,
    );
    source.text = text;
    for value in [&mut source.app, &mut source.title, &mut source.url]
        .into_iter()
        .flatten()
    {
        let (bounded, cut) = bounded_json_string(
            value,
            MAX_SOURCE_META_JSON_BYTES,
            MAX_SOURCE_META_JSON_BYTES,
        );
        *value = bounded;
        truncated |= cut;
    }
    (source, truncated)
}

/// 把本機已排好的 facts／hits 收成一次有界 RAG prompt。
///
/// 回傳 `None` 代表本機沒有任何候選；查詢規劃 CLI 在這之前仍已處理問題，這裡只是不再
/// 要求它憑空生成一份沒有本機出處的答案。
///
/// `now` 是「現在幾點」，要當參數傳而不是在這裡讀時鐘：一次回答只該有一個
/// 「現在」（呼叫端那一次 `now_ms()`），而且測試要問得出「她把哪一刻當成現在」。
/// 模型需要它才講得出「昨天下午」——沒有現在，來源上的 `at` 只能被讀成日期。
pub fn prepare(
    question: &str,
    readings: &[Reading],
    facts: &[FactAnswer],
    hits: &[SearchHit],
    now: Millis,
) -> Result<Option<Prepared>> {
    ensure!(!question.trim().is_empty(), "RAG 問題不可為空");

    let (question, question_truncated) =
        bounded_json_string(question, MAX_QUESTION_BYTES, MAX_QUESTION_JSON_BYTES);
    if readings.is_empty() && facts.is_empty() && hits.is_empty() {
        return Ok(None);
    }
    let total_candidates = readings.len() + facts.len() + hits.len();
    let mut candidates = Vec::with_capacity(MAX_SOURCES);
    // 先替三種來源各留位置：理解記憶 8、typed facts 5、直接命中的原文 5。
    // 舊版單純 reading → fact → hit 往下截；一題剛好有 8 張判讀與 10 個 fact
    // 時，18 格會被前兩種吃光，使用者問題真正命中的原文反而一筆都進不了 RAG。
    const BASE_FACTS: usize = 5;
    const BASE_HITS: usize = 5;
    candidates.extend(readings.iter().take(MAX_READINGS).map(Source::from_reading));
    candidates.extend(facts.iter().take(BASE_FACTS).map(Source::from_fact));
    candidates.extend(hits.iter().take(BASE_HITS).map(Source::from_hit));
    let mut next_fact = BASE_FACTS.min(facts.len());
    let mut next_hit = BASE_HITS.min(hits.len());
    while candidates.len() < MAX_SOURCES {
        let before = candidates.len();
        if let Some(hit) = hits.get(next_hit) {
            candidates.push(Source::from_hit(hit));
            next_hit += 1;
        }
        if candidates.len() < MAX_SOURCES
            && let Some(fact) = facts.get(next_fact)
        {
            candidates.push(Source::from_fact(fact));
            next_fact += 1;
        }
        if candidates.len() == before {
            break;
        }
    }

    let mut truncated = question_truncated || total_candidates > candidates.len();
    // 候選的**取捨**仍按 reading → fact → match；交給模型的**閱讀順序**則一定按
    // 時間。以前 prompt 是四張判讀（新到舊）→ facts → FTS 排名，模型手上沒有
    // 一條能順著讀的時間線，只能把每筆各講一句。
    candidates.sort_by(|left, right| {
        left.ts
            .cmp(&right.ts)
            .then_with(|| left.reference.as_str().cmp(&right.reference.as_str()))
    });

    let mut data = serde_json::to_string(&PromptQuestion {
        question: &question,
    })?;
    data.push('\n');
    let mut sources = Vec::new();
    for source in candidates {
        let (source, source_truncated) = bound_source(source);
        truncated |= source_truncated;
        let line = serde_json::to_string(&PromptSource {
            r#ref: source.reference.as_str(),
            kind: source.origin.as_str(),
            at: crate::model::stamp(source.ts),
            text: &source.text,
            app: source.app.as_deref(),
            title: source.title.as_deref(),
            url: source.url.as_deref(),
        })?;
        if data.len() + line.len() + 1 > MAX_DATA_BYTES {
            truncated = true;
            break;
        }
        data.push_str(&line);
        data.push('\n');
        sources.push(source);
    }
    ensure!(!sources.is_empty(), "第一筆 RAG 來源超過資料上限");

    let (fenced, fence_truncated) =
        crate::prompt_fence::fence_question_and_evidence(&data, MAX_DATA_BYTES)?;
    debug_assert!(!fence_truncated, "prepare 已先守住資料上限");
    // 這一段是「她開口像不像朋友」的唯一出處。
    //
    // 第一版只說「你是本機記憶的回答層」，於是模型把自己當成一個看螢幕的旁觀
    // 者，講出來是「畫面上可見有人要求 Codex Agent 寫交接檔」。第二版修掉了
    // 人稱和時刻，於是變成：
    //
    // > 你今天早上 5:33 在 AI-Sister 收到「Flood watch」，旁邊寫著 Just issued。
    // > 你 5:34 到 5:35 主畫面接著出現「Temps to drop」。
    //
    // 主詞對了、時刻對了、旁白說法沒了——而他讀完說「還是只在念 log」。他是
    // 對的：**那三句話沒有一句需要看懂任何東西**。三行 OCR 加時間戳排成中
    // 文，就是這個形狀。朋友不會這樣講；朋友會說「你早上在追一個天氣警報」。
    //
    // 所以第三版換的不是語氣，是**答案的骨架**：第一句必須是一句判讀（他在
    // 做什麼、看起來想解決什麼），後面幾句才是他會想確認的細節。而判讀不必
    // 現場想——`kind: "reading"` 那幾筆就是她閒著的時候已經想過一輪的結論。
    //
    // 來源紀律一個字都沒有放寬：句子還是只能引用列出的 ref，時間還是只能用
    // 來源自己的 `at`，具體的名字數字結果還是不准編。放寬的只有一件事——第
    // 一句准許用「看起來／大概」把好幾筆收成一句話，而那正是「判讀」和「照
    // 抄」的分界。不准的是**無憑無據的猜**，不是**看懂**。
    let header = format!(
        concat!(
            "你是使用者自己的 AI 夥伴，正在幫他回想他自己那台電腦上發生過的事。\n",
            "問這一題的人就是這些紀錄的主人：講到他做的事一律用「你」，講到自己記下的用「我」。\n",
            "像朋友在講話，不要像在描述一個畫面——不要用「畫面上可見」「根據紀錄」「使用者」「有人」這種旁白說法。\n",
            "只根據下面這一題的本機檢索來源回答。\n",
            "只輸出一個 JSON 物件，不要 markdown、不要前後解說。\n",
            "契約：{{\"sentences\":[{{\"text\":\"一句繁體中文答案\",\"sources\":[\"fact:1\",\"chunk:2\"]}}]}}\n",
            "sentences 必須是 1 到 6 句；簡單事實一兩句就停，需要交代一段事情才用更多句。每一項只放一句話、必須有至少一個 sources；sources 只能逐字使用下面列出的 ref。\n",
            "現在是 {now}。每一筆來源都帶 at，那是那件事發生在這台電腦上的時間。\n",
            "每一筆來源都帶 kind。kind=\"reading\" 是你稍早依紀錄想過一輪的可推翻結論；ocr 是畫面文字辨識，assistive 是輔助介面回報的前景可見文字，clipboard 是剪貼簿文字，window_title 是視窗標題，url 是記下的網址。其他來源照 kind 理解，unknown 代表來源不明，不可猜成 OCR。\n",
            "剪貼簿內容不證明當時出現在畫面上；這裡提供的是文字，ocr 來源也不代表目前仍保留截圖檔。\n",
            "回答前先在內部按 at 排出時間線，分清楚：前面在做什麼、哪裡發生轉折、後來看到了什麼、目前能確認到哪裡；不要輸出這個分析過程。\n",
            "**第一句要先講你看懂了什麼，並直接回答問題**：他當時在做的是哪一件事、看起來想弄清楚或解決什麼。有 kind=\"reading\" 就把它當可推翻的判讀來衡量，不要因為它存在就無條件相信。\n",
            "那一句可以把好幾筆收成一句話，不確定就說「看起來」「大概」「我猜」——但不要補來源裡沒有的姓名、數字、完成狀態或原因。\n",
            "**不要把來源原文照抄當成答案。**「你收到 X，旁邊寫著 Y」是在念紀錄，不是在回答。\n",
            "後面的句子要接成同一段解釋，按需要補：事情怎麼開始、做了哪些轉折、卡在哪、後來有沒有完成證據。不要把互不相干的來源各寫成一條片段。\n",
            "時間相鄰只證明先後，不自動證明因果。只有來源真的支持原因時才用「因為／所以」；否則說「接著／之後／同一段時間」。\n",
            "句子指到某一刻就把時刻講進句子裡（「你早上 10:14 要求…」「你昨天下午在…」）；at 以外的時間一個字都不要編。\n",
            "來源中如果是別人說的話或別人寫的東西，就講清楚那是誰的，不要算到「你」頭上。\n",
            "資料不足就只說來源能支持的範圍。\n\n",
        ),
        now = crate::model::stamp(now),
    );
    Ok(Some(Prepared {
        payload: format!("{header}{fenced}"),
        sources,
        truncated,
    }))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelAnswer {
    sentences: Vec<ModelSentence>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelSentence {
    text: String,
    sources: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroundedAnswer {
    pub sentences: Vec<GroundedSentence>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroundedSentence {
    pub text: String,
    pub sources: Vec<SourceRef>,
}

fn is_sentence_tail(character: char) -> bool {
    matches!(
        character,
        '。' | '！'
            | '？'
            | '!'
            | '?'
            | '.'
            | '…'
            | '」'
            | '』'
            | '”'
            | '’'
            | ')'
            | ']'
            | '）'
            | '】'
    )
}

/// 一個 JSON item 就是一句。句尾可以是「？！」「。」或引號／括號組合；
/// 任何句號後又開始正文、換行或控制字元都代表模型把多句塞進同一筆。
fn is_one_sentence(text: &str) -> bool {
    if text.chars().any(char::is_control) {
        return false;
    }
    for (index, character) in text.char_indices() {
        let remaining = &text[index + character.len_utf8()..];
        let sentence_end = matches!(character, '。' | '！' | '？' | '!' | '?')
            || (character == '.'
                && remaining
                    .chars()
                    .next()
                    .is_none_or(|next| char::is_whitespace(next) || is_sentence_tail(next)));
        if sentence_end
            && !remaining
                .chars()
                .all(|remaining| remaining.is_whitespace() || is_sentence_tail(remaining))
        {
            return false;
        }
    }
    true
}

/// 驗 CLI stdout：每一句都要有來源，且只能引用這次真的送出的來源。
pub fn parse(stdout: &str, allowed: &[Source]) -> Result<GroundedAnswer, String> {
    let value: serde_json::Value = serde_json::from_str(stdout.trim())
        .map_err(|error| format!("stdout 不是單一 JSON 物件：{error}"))?;
    if !value.is_object() {
        return Err("stdout 不是單一 JSON 物件".to_owned());
    }
    let model: ModelAnswer =
        serde_json::from_value(value).map_err(|error| format!("JSON 對不上回答契約：{error}"))?;
    if model.sentences.is_empty() || model.sentences.len() > MAX_SENTENCES {
        return Err(format!(
            "sentences 必須是 1 到 {MAX_SENTENCES} 句，實際是 {}",
            model.sentences.len()
        ));
    }

    let allowed: HashSet<SourceRef> = allowed
        .iter()
        .map(|source| source.reference.clone())
        .collect();
    let mut sentences = Vec::with_capacity(model.sentences.len());
    for sentence in model.sentences {
        let text = sentence.text.trim().to_owned();
        if text.is_empty() {
            return Err("回答裡有空句".to_owned());
        }
        if text.chars().count() > MAX_SENTENCE_CHARS {
            return Err(format!("回答單句超過 {MAX_SENTENCE_CHARS} 字"));
        }
        if !is_one_sentence(&text) {
            return Err("回答把多句或控制字元放進同一個 sentence".to_owned());
        }
        if sentence.sources.is_empty() {
            return Err("回答裡有一句沒有來源".to_owned());
        }
        let mut refs = Vec::new();
        let mut seen = HashSet::new();
        for raw in sentence.sources {
            let reference =
                SourceRef::parse(&raw).ok_or_else(|| format!("回答引用了看不懂的來源：{raw}"))?;
            if !allowed.contains(&reference) {
                return Err(format!("回答引用了這次沒有提供的來源：{raw}"));
            }
            if seen.insert(reference.clone()) {
                refs.push(reference);
            }
        }
        sentences.push(GroundedSentence {
            text,
            sources: refs,
        });
    }
    Ok(GroundedAnswer { sentences })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::FactRow;
    use crate::model::SourceKind;

    /// 測試裡的「現在」。挑一個 2026 年 9 月的真實 epoch，而不是 `now_ms()`：
    /// 一次回答只該有一個現在，測試要問得出那是哪一刻。**不要在斷言裡寫死
    /// 它渲染出來的字串**——開發機是 EDT、CI 是 UTC，那個字串兩邊不一樣。
    /// 要比就跟 [`crate::model::stamp`] 比，那是產品自己用的同一支。
    const NOW: Millis = 1_789_136_073_000;

    fn fact() -> FactAnswer {
        FactAnswer {
            latest: FactRow {
                id: 9,
                ts: 100,
                kind: "phone".into(),
                raw: "客服專線 0800-080-123".into(),
                normalized: "+886800080123".into(),
                source_kind: "ocr".into(),
                chunk_id: Some(31),
                frame_id: Some(42),
                app_id: Some("chrome.exe".into()),
                window_title: Some("帳單".into()),
                url: None,
            },
            sightings: 2,
        }
    }

    fn hit(id: i64, text: &str) -> SearchHit {
        SearchHit {
            chunk_id: id,
            ts: 200 + id,
            source_kind: SourceKind::Ocr,
            frame_id: Some(100 + id),
            app_id: Some("notes.exe".into()),
            window_title: Some("工作筆記".into()),
            url: None,
            text: text.into(),
            snippet: text.into(),
            score: 1.0,
        }
    }

    fn reading(card_id: i64, at: Millis, activity: &str) -> Reading {
        Reading {
            card_id,
            matched: false,
            at,
            activity: activity.into(),
            frame_id: Some(700 + card_id),
        }
    }

    /// 各來源沒有時間衝突時，仍看得到預留給判讀、fact 與原文的基本順序。
    #[test]
    fn her_own_readings_come_before_the_screen_text() {
        let prepared = prepare(
            "剛剛在幹嘛",
            &[reading(5, 50, "你在追一個天氣警報")],
            &[fact()],
            &[hit(31, "Flood watch")],
            NOW,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            prepared
                .sources
                .iter()
                .map(|source| source.reference.as_str())
                .collect::<Vec<_>>(),
            vec!["card:5", "fact:9", "chunk:31"],
        );
    }

    #[test]
    fn selected_sources_are_given_to_the_model_in_chronological_order() {
        let mut old_fact = fact();
        old_fact.latest.ts = 100;
        let prepared = prepare(
            "事情怎麼發生的",
            &[reading(5, 300, "最後的判讀")],
            &[old_fact],
            &[hit(31, "中間的原文")], // helper timestamp = 231
            NOW,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            prepared
                .sources
                .iter()
                .map(|source| source.reference.as_str())
                .collect::<Vec<_>>(),
            ["fact:9", "chunk:31", "card:5"],
            "來源種類不能再蓋過時間順序"
        );
    }

    /// prompt 裡分得出哪一筆是螢幕上的字、哪一筆是她想的。
    ///
    /// 少了這個欄位，模型收到的是一團同質的東西，而它會照最保守的方式處理：
    /// 把每一筆都當成畫面上的字去引用——也就是回到念紀錄。
    #[test]
    fn the_prompt_says_which_lines_are_hers_and_which_are_the_screens() {
        let prepared = prepare(
            "剛剛在幹嘛",
            &[reading(5, 50, "你在追一個天氣警報")],
            &[fact()],
            &[hit(31, "Flood watch")],
            NOW,
        )
        .unwrap()
        .unwrap();
        assert!(
            prepared
                .payload
                .contains(r#""ref":"card:5","kind":"reading""#),
            "判讀那一筆要標成 reading：{}",
            prepared.payload
        );
        assert!(
            prepared
                .payload
                .contains(r#""ref":"chunk:31","kind":"ocr""#),
            "OCR 讀到的字要標成 ocr：{}",
            prepared.payload
        );
        assert!(
            prepared.payload.contains("你在追一個天氣警報"),
            "她想的那句話本身要送進去，不然模型只拿到一個編號"
        );
    }

    #[test]
    fn an_unknown_fact_origin_is_not_guessed_from_its_frame_reference() {
        let mut unknown = fact();
        unknown.latest.source_kind = "future_capture".into();
        let prepared = prepare("電話", &[], &[unknown], &[], NOW).unwrap().unwrap();
        assert!(
            prepared
                .payload
                .contains(r#""ref":"fact:9","kind":"unknown""#)
        );
        assert!(!prepared.payload.contains("future_capture"));
        assert_eq!(prepared.sources[0].frame_id, Some(42));
    }

    /// 她這一段還沒想過，答題照樣走得下去。
    #[test]
    fn a_stretch_she_never_thought_about_still_answers() {
        let prepared = prepare("剛剛在幹嘛", &[], &[], &[hit(31, "Flood watch")], NOW)
            .unwrap()
            .unwrap();
        assert_eq!(
            prepared
                .sources
                .iter()
                .map(|source| source.reference.as_str())
                .collect::<Vec<_>>(),
            vec!["chunk:31"],
        );
        assert!(!prepared.payload.contains(r#""kind":"reading""#));
    }

    /// 那句「第一句要先講你看懂了什麼」不可以被誰安靜地刪掉。
    ///
    /// 這是這一版唯一在改「她答出來長什麼樣」的東西，而它是一段字串——沒有
    /// 型別守它，沒有任何別的測試會因為它不見了而變紅。他讀到的差別是「像
    /// 朋友」和「在念 log」，而那兩者在測試裡長得一模一樣：都是一份合約成立、
    /// 每句都有出處的 JSON。
    #[test]
    fn the_prompt_still_asks_her_to_say_what_she_understood_first() {
        let payload = prepare("剛剛在幹嘛", &[], &[fact()], &[], NOW)
            .unwrap()
            .unwrap()
            .payload;
        assert!(
            payload.contains("第一句要先講你看懂了什麼"),
            "少了這一句，她會回去一行一行念螢幕上的字"
        );
        assert!(
            payload.contains("不要把來源原文照抄當成答案"),
            "光叫她「先講你看懂了什麼」不夠——照抄一句再補一句判讀也滿足前者"
        );
        assert!(
            payload.contains("看起來"),
            "判讀要准許不確定的語氣，否則她只講得出照抄得出來的東西"
        );
        // 「不要補來源裡沒有的…」那一句不在這裡再守一次——它自己已經有一條針
        // （`the_prompt_still_asks_her_to_talk_like_a_friend_not_a_narrator`）。
        // 這一條守的是**這一版新加的那三句**：先講你看懂了什麼、不要照抄、
        // 准許不確定的語氣。
    }

    /// 這一題翻出來的證據落在哪一段時間裡。
    #[test]
    fn the_evidence_window_covers_every_row_with_slack_on_both_sides() {
        let facts = [fact()]; // ts = 100
        let hits = [hit(31, "a"), hit(9, "b")]; // ts = 231、209
        let (from, to) = evidence_window(&facts, &hits).unwrap();
        assert_eq!(from, 100 - READING_SLACK_MS, "下界從最早那一筆往前放寬");
        assert_eq!(to, 231 + READING_SLACK_MS, "上界從最晚那一筆往後放寬");
    }

    /// 一筆原文／事實都沒有就沒有時間窗；內容比對判讀不依賴這個窗。
    #[test]
    fn no_evidence_means_no_window_to_look_for_readings_in() {
        assert_eq!(evidence_window(&[], &[]), None);
    }

    /// 從一張卡取出答題用得到的那幾格：第一張畫面，不是第一筆證據。
    #[test]
    fn a_reading_takes_the_first_frame_out_of_its_own_evidence() {
        let card = crate::db::L2CardRow {
            id: 12,
            segment_core_start: 5_000,
            segment_ref: "seg:5000".into(),
            version: 1,
            supersedes: None,
            activity: "你在追一個天氣警報".into(),
            entities_json: "[]".into(),
            continues_json: None,
            commitments_json: "[]".into(),
            model_confidence: 0.8,
            // 第一筆是 fact，第二筆才是畫面。取的要是畫面。
            evidence_json: r#"["fact:7","frame:42","frame:43"]"#.into(),
            open_questions_json: "[]".into(),
            created_at: 6_000,
            author: crate::db::L2Author::Interpreter,
            tombstoned_at: None,
        };
        let reading = Reading::from_card(&card, ReadingOrigin::Time);
        assert_eq!(reading.card_id, 12);
        assert_eq!(reading.at, 5_000, "掛的是這一段開始的時刻");
        assert_eq!(reading.activity, "你在追一個天氣警報");
        assert_eq!(reading.frame_id, Some(42), "跳過 fact，取第一張畫面");
    }

    #[test]
    fn answer_prompt_does_not_export_l2_internal_relationship_fields() {
        let card = crate::db::L2CardRow {
            id: 12,
            segment_core_start: 5_000,
            segment_ref: "INTERNAL_SEGMENT_MARK".into(),
            version: 1,
            supersedes: None,
            activity: "你在追一個天氣警報".into(),
            entities_json: "[]".into(),
            continues_json: Some(
                r#"{"segment_ref":"INTERNAL_CONTINUES_MARK","confidence":0.8}"#.into(),
            ),
            commitments_json: "[]".into(),
            model_confidence: 0.123456,
            evidence_json: r#"["frame:42"]"#.into(),
            open_questions_json: r#"["INTERNAL_OPEN_QUESTION_MARK"]"#.into(),
            created_at: 6_000,
            author: crate::db::L2Author::Reviewer,
            tombstoned_at: None,
        };
        let payload = prepare(
            "剛剛在幹嘛",
            &[Reading::from_card(&card, ReadingOrigin::Time)],
            &[],
            &[],
            NOW,
        )
        .unwrap()
        .unwrap()
        .payload;
        assert!(payload.contains("你在追一個天氣警報"));
        for internal in [
            "INTERNAL_SEGMENT_MARK",
            "INTERNAL_CONTINUES_MARK",
            "INTERNAL_OPEN_QUESTION_MARK",
            "0.123456",
            "reviewer",
        ] {
            assert!(
                !payload.contains(internal),
                "現行第二張同意書沒有授權把 L2 內部關係欄位送給答題 CLI：{internal}"
            );
        }
    }

    /// 一張只靠 fact 立起來的卡仍然是一句判讀，只是那顆鍵點不開圖。
    #[test]
    fn a_reading_with_no_frame_evidence_is_still_a_reading() {
        let card = crate::db::L2CardRow {
            id: 13,
            segment_core_start: 5_000,
            segment_ref: "seg:5000".into(),
            version: 1,
            supersedes: None,
            activity: "你在對帳".into(),
            entities_json: "[]".into(),
            continues_json: None,
            commitments_json: "[]".into(),
            model_confidence: 0.8,
            evidence_json: r#"["fact:7"]"#.into(),
            open_questions_json: "[]".into(),
            created_at: 6_000,
            author: crate::db::L2Author::Interpreter,
            tombstoned_at: None,
        };
        assert_eq!(
            Reading::from_card(&card, ReadingOrigin::Time).frame_id,
            None
        );
    }

    /// 引用一張這一題沒送出去的判讀，整份作廢——和另外兩種一樣。
    #[test]
    fn a_card_ref_that_was_never_offered_is_rejected() {
        let prepared = prepare(
            "剛剛在幹嘛",
            &[reading(5, 50, "你在追一個天氣警報")],
            &[],
            &[],
            NOW,
        )
        .unwrap()
        .unwrap();
        assert!(
            parse(
                r#"{"sentences":[{"text":"你在追天氣警報。","sources":["card:5"]}]}"#,
                &prepared.sources
            )
            .is_ok(),
            "送出去過的那一張要收"
        );
        let error = parse(
            r#"{"sentences":[{"text":"你在對帳。","sources":["card:6"]}]}"#,
            &prepared.sources,
        )
        .unwrap_err();
        assert!(error.contains("card:6"), "{error}");
    }

    #[test]
    fn local_candidates_become_a_bounded_fenced_prompt() {
        let prepared = prepare(
            "客服電話是什麼",
            &[],
            &[fact()],
            &[hit(31, "請撥客服專線")],
            NOW,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            prepared
                .sources
                .iter()
                .map(|source| source.reference.as_str())
                .collect::<Vec<_>>(),
            ["fact:9", "chunk:31"]
        );
        assert!(
            prepared
                .payload
                .contains("BEGIN QUESTION AND SCREEN DATA nonce=")
        );
        assert!(prepared.payload.contains("客服電話是什麼"));
        assert!(prepared.payload.contains("+886800080123"));
        assert!(!prepared.truncated);
    }

    /// 來源上的時刻要以人看得懂的樣子進 prompt，原始的 epoch 毫秒不要進去。
    ///
    /// 這一條守的是使用者真的讀到的那句話。她說得出「你早上 10:14 要求…」的
    /// 前提是模型手上拿得到「10:14」；拿到 `1789136073000` 它只講得出
    /// 「你要求過…」，那正是 Ted 說「跟機器人一樣」的那一版。
    #[test]
    fn every_source_carries_a_clock_time_not_an_epoch_number() {
        let four_hours = 4 * 60 * 60 * 1000;
        let mut fact = fact();
        fact.latest.ts = NOW - four_hours;
        let mut hit = hit(31, "交接檔");
        hit.ts = NOW - four_hours / 2;
        let prepared = prepare("剛剛在幹嘛", &[], &[fact.clone()], &[hit.clone()], NOW)
            .unwrap()
            .unwrap();
        let payload = prepared.payload;

        for ts in [fact.latest.ts, hit.ts] {
            let stamp = crate::model::stamp(ts);
            // 產品自己那支格式化，不手抄一份會漂的字串：開發機是 EDT、CI 是 UTC。
            assert!(
                stamp.len() == "2026-09-11 10:14:33".len()
                    && stamp.as_bytes()[10] == b' '
                    && stamp.as_bytes()[13] == b':',
                "stamp 不再是人讀得懂的時刻：{stamp}"
            );
            assert!(payload.contains(&stamp), "prompt 裡沒有 {stamp}");
            assert!(
                !payload.contains(&ts.to_string()),
                "prompt 裡還留著原始毫秒 {ts}"
            );
        }
    }

    /// 「現在幾點」要跟著問題一起送出去，而且是呼叫端給的那一刻。
    ///
    /// 沒有現在，來源上的 `at` 只能被讀成一個日期；有了現在，「昨天下午」
    /// 才算得出來。這一條同時釘住它是**參數**不是這裡自己讀的時鐘——
    /// 改成 `now_ms()` 這條會紅。
    #[test]
    fn the_prompt_says_which_moment_counts_as_now() {
        let prepared = prepare("剛剛在幹嘛", &[], &[fact()], &[], NOW)
            .unwrap()
            .unwrap();
        assert!(
            prepared
                .payload
                .contains(&format!("現在是 {}。", crate::model::stamp(NOW))),
            "prompt 沒說現在是哪一刻"
        );
        assert!(!prepared.payload.contains(&NOW.to_string()));
    }

    /// 語氣那幾條指示還在。
    ///
    /// **這一條擋得住的只有「有人把它整段刪掉／改寫掉」**，擋不住模型不照做——
    /// 沒有任何本機測試能證明模型的輸出像朋友。真正的驗收在 Ted 的機器上按一次。
    /// 針取的是整句承諾，不是單一個字：`「你」` 這種短針在這份檔案裡到處都是。
    #[test]
    fn the_prompt_still_asks_her_to_talk_like_a_friend_not_a_narrator() {
        let payload = prepare("剛剛在幹嘛", &[], &[fact()], &[], NOW)
            .unwrap()
            .unwrap()
            .payload;
        for promise in [
            // 這一句是招牌。舊版寫「你是本機記憶的回答層」，模型就照著當旁觀者，
            // 講出「畫面上可見有人要求…」。突變測試證實過：只把它換回去，
            // 底下四條全都還在、11 條測試全綠——所以它得自己有一條針。
            "你是使用者自己的 AI 夥伴",
            "講到他做的事一律用「你」，講到自己記下的用「我」",
            "不要用「畫面上可見」「根據紀錄」「使用者」「有人」這種旁白說法",
            "句子指到某一刻就把時刻講進句子裡",
            "at 以外的時間一個字都不要編",
            "來源中如果是別人說的話或別人寫的東西，就講清楚那是誰的",
        ] {
            assert!(payload.contains(promise), "prompt 少了這一條：{promise}");
        }
        assert!(
            !payload.contains("本機記憶的回答層"),
            "prompt 又把她定位成一個看螢幕的旁觀者"
        );
        // 換語氣不准放寬證據。這兩條和舊版逐字一樣。
        assert!(payload.contains("sources 只能逐字使用下面列出的 ref"));
        assert!(payload.contains("不要補來源裡沒有的姓名、數字、完成狀態或原因"));
    }

    #[test]
    fn no_local_candidate_means_no_unsupported_answer_prompt() {
        assert!(prepare("沒有的東西", &[], &[], &[], NOW).unwrap().is_none());
    }

    #[test]
    fn every_question_can_become_a_cli_directed_memory_search() {
        let prompt = prepare_search_plan("昨天在做什麼？").unwrap();
        assert!(prompt.contains("昨天在做什麼？"));
        assert!(prompt.contains("\"queries\""));
        assert!(prompt.contains("BEGIN QUESTION AND SCREEN DATA nonce="));
        assert!(!prompt.contains("sister.db"));
        assert!(prompt.contains("事情發生前的線索"));
        assert!(prompt.contains("之後的變化／結果"));
    }

    #[test]
    fn multiple_searches_take_turns_instead_of_the_first_one_filling_the_page() {
        let batches = vec![
            vec![hit(1, "主事件一"), hit(2, "主事件二"), hit(3, "主事件三")],
            vec![hit(10, "前因一"), hit(11, "前因二")],
            vec![hit(20, "結果一"), hit(2, "重複來源")],
        ];
        let merged = merge_ranked_hits(batches, 6);
        assert_eq!(
            merged.iter().map(|hit| hit.chunk_id).collect::<Vec<_>>(),
            [1, 10, 20, 2, 11, 3],
            "每一輪先讓每條查詢各拿一個名次，重複來源只留第一次"
        );
    }

    #[test]
    fn a_long_explanation_can_use_six_grounded_sentences_but_not_seven() {
        let prepared = prepare("前因後果", &[], &[fact()], &[], NOW)
            .unwrap()
            .unwrap();
        let sentence = |n| format!(r#"{{"text":"第{n}句。","sources":["fact:9"]}}"#);
        let six = (1..=6).map(&sentence).collect::<Vec<_>>().join(",");
        assert!(parse(&format!(r#"{{"sentences":[{six}]}}"#), &prepared.sources).is_ok());
        let seven = (1..=7).map(sentence).collect::<Vec<_>>().join(",");
        assert!(parse(&format!(r#"{{"sentences":[{seven}]}}"#), &prepared.sources).is_err());
    }

    #[test]
    fn search_plans_are_strict_bounded_and_deduplicated() {
        assert_eq!(
            parse_search_plan(r#"{"queries":["昨天在做什麼", "帳單 客服", "昨天在做什麼"]}"#)
                .unwrap(),
            ["昨天在做什麼", "帳單 客服"]
        );
        assert!(parse_search_plan(r#"{"queries":[]}"#).is_err());
        assert!(parse_search_plan(r#"{"queries":["a","b","c","d"]}"#).is_err());
        assert!(parse_search_plan(r#"{"queries":["ok"],"answer":"no"}"#).is_err());
        assert!(parse_search_plan("```json\n{\"queries\":[\"ok\"]}\n```").is_err());
        let too_long = "查".repeat(MAX_SEARCH_QUERY_CHARS + 1);
        assert!(parse_search_plan(&format!(r#"{{"queries":["{too_long}"]}}"#)).is_err());
    }

    #[test]
    fn every_sentence_must_cite_only_this_retrieval() {
        let prepared = prepare("客服電話", &[], &[fact()], &[hit(31, "客服")], NOW)
            .unwrap()
            .unwrap();
        let answer = parse(
            r#"{"sentences":[{"text":"我最後看到的客服電話是 0800-080-123。","sources":["fact:9"]}]}"#,
            &prepared.sources,
        )
        .unwrap();
        assert_eq!(answer.sentences[0].sources, [SourceRef::Fact(9)]);

        assert!(
            parse(
                r#"{"sentences":[{"text":"沒有根據。","sources":[]}]}"#,
                &prepared.sources,
            )
            .is_err()
        );
        assert!(
            parse(
                r#"{"sentences":[{"text":"捏造。","sources":["chunk:999"]}]}"#,
                &prepared.sources,
            )
            .is_err()
        );
    }

    #[test]
    fn malformed_or_oversized_answers_are_rejected_as_a_whole() {
        let prepared = prepare("客服電話", &[], &[fact()], &[], NOW)
            .unwrap()
            .unwrap();
        assert!(parse("not json", &prepared.sources).is_err());
        assert!(
            parse(
                "回答如下：\n{\"sentences\":[{\"text\":\"一份答案。\",\"sources\":[\"fact:9\"]}]}",
                &prepared.sources,
            )
            .is_err()
        );
        assert!(
            parse(
                r#"{"sentences":[{"text":"第一句。第二句。","sources":["fact:9"]}]}"#,
                &prepared.sources,
            )
            .is_err()
        );
        assert!(
            parse(
                r#"{"sentences":[{"text":"版本 1.2 已完成？！","sources":["fact:9"]}]}"#,
                &prepared.sources,
            )
            .is_ok()
        );
        let too_long = "長".repeat(MAX_SENTENCE_CHARS + 1);
        let stdout = format!(r#"{{"sentences":[{{"text":"{too_long}","sources":["fact:9"]}}]}}"#);
        assert!(parse(&stdout, &prepared.sources).is_err());
    }

    #[test]
    fn source_count_and_data_bytes_are_bounded_before_the_fence() {
        let hits = (1..=30)
            .map(|id| hit(id, &"一段很長的本機文字".repeat(300)))
            .collect::<Vec<_>>();
        let prepared = prepare("昨天在做什麼", &[], &[], &hits, NOW)
            .unwrap()
            .unwrap();
        assert!(prepared.sources.len() <= MAX_SOURCES);
        assert!(prepared.truncated);
        assert!(prepared.payload.len() < MAX_DATA_BYTES + 2_000);
    }

    #[test]
    fn readings_and_facts_cannot_push_every_direct_text_match_out_of_rag() {
        let readings = (1..=MAX_READINGS as i64)
            .map(|id| reading(id, id * 10, &format!("判讀 {id}")))
            .collect::<Vec<_>>();
        let facts = (1..=10)
            .map(|id| {
                let mut answer = fact();
                answer.latest.id = id;
                answer
            })
            .collect::<Vec<_>>();
        let hits = (1..=20)
            .map(|id| hit(100 + id, &format!("直接命中 {id}")))
            .collect::<Vec<_>>();
        let prepared = prepare("一整段事情", &readings, &facts, &hits, NOW)
            .unwrap()
            .unwrap();
        assert_eq!(prepared.sources.len(), MAX_SOURCES);
        assert!(
            prepared
                .sources
                .iter()
                .filter(|source| matches!(source.reference, SourceRef::Chunk(_)))
                .count()
                >= 5,
            "直接命中的原文必須有自己的席位"
        );
        assert!(prepared.truncated);
    }

    #[test]
    fn one_hostile_oversized_source_still_leaves_one_citable_candidate() {
        let controls = "\u{1}".repeat(MAX_DATA_BYTES * 2);
        let mut huge = hit(77, &controls);
        huge.app_id = Some(controls.clone());
        huge.window_title = Some(controls.clone());
        huge.url = Some(controls);
        let prepared = prepare(&"\u{2}".repeat(MAX_DATA_BYTES), &[], &[], &[huge], NOW)
            .unwrap()
            .unwrap();
        assert_eq!(prepared.sources.len(), 1);
        assert_eq!(prepared.sources[0].reference, SourceRef::Chunk(77));
        assert!(prepared.truncated);
        assert!(prepared.payload.len() < MAX_DATA_BYTES + 2_000);
        assert!(
            parse(
                r#"{"sentences":[{"text":"能引用。","sources":["chunk:77"]}]}"#,
                &prepared.sources,
            )
            .is_ok()
        );
    }
}
