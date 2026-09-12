//! S1 問答的 CLI-directed 本機檢索與成句層。
//!
//! 第一階段只把問題交給使用者選用的 CLI，驗回最多三條自然語言查詢；AI-Sister
//! 在本機執行後，第二階段才把**這一題命中的文字**整理成有界 prompt 交回同一支
//! CLI。每一句都必須引用這次候選裡的 exact source ref；模型不能新增來源，也不能
//! 把沒有來源的句子塞進答案。

use std::collections::HashSet;

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

use crate::answer::Answer as FactAnswer;
use crate::model::{Millis, SearchHit};

/// 一題最多交給 CLI 的本機候選數。facts 先排，剩下才是全文命中。
pub const MAX_SOURCES: usize = 12;
/// 問題本身的 byte 上限。檢索仍用完整問題；只有送進 CLI 的副本有界。
pub const MAX_QUESTION_BYTES: usize = 2 * 1024;
/// 圍欄內 question + evidence 的 byte 上限。
pub const MAX_DATA_BYTES: usize = 12 * 1024;
/// 單筆來源正文的 raw byte 上限。其後還會再守 JSON 編碼後的大小。
pub const MAX_SOURCE_TEXT_BYTES: usize = 4 * 1024;
const MAX_QUESTION_JSON_BYTES: usize = 3 * 1024;
const MAX_SOURCE_TEXT_JSON_BYTES: usize = 5 * 1024;
const MAX_SOURCE_META_JSON_BYTES: usize = 512;
/// 回答最多三句；畫面每句各自帶來源。
pub const MAX_SENTENCES: usize = 3;
/// 單句上限，避免 CLI 把一篇文章塞進一格。
pub const MAX_SENTENCE_CHARS: usize = 240;
/// The selected CLI can ask AI-Sister to run at most this many local-memory searches for one
/// question. The CLI never receives a database path and never opens the SQLite file itself.
pub const MAX_SEARCH_QUERIES: usize = 3;
/// A search is natural-language input to the same retrieval path as `sister query`.
pub const MAX_SEARCH_QUERY_CHARS: usize = 240;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SourceRef {
    Fact(i64),
    Chunk(i64),
}

impl SourceRef {
    pub fn as_str(&self) -> String {
        match self {
            Self::Fact(id) => format!("fact:{id}"),
            Self::Chunk(id) => format!("chunk:{id}"),
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
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Source {
    pub reference: SourceRef,
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
            ts: hit.ts,
            text: hit.text.clone(),
            app: hit.app_id.clone(),
            title: hit.window_title.clone(),
            url: hit.url.clone(),
            frame_id: hit.frame_id,
        }
    }
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
    facts: &[FactAnswer],
    hits: &[SearchHit],
    now: Millis,
) -> Result<Option<Prepared>> {
    ensure!(!question.trim().is_empty(), "RAG 問題不可為空");

    let (question, question_truncated) =
        bounded_json_string(question, MAX_QUESTION_BYTES, MAX_QUESTION_JSON_BYTES);
    let mut candidates = Vec::new();
    candidates.extend(facts.iter().map(Source::from_fact));
    candidates.extend(hits.iter().map(Source::from_hit));
    if candidates.is_empty() {
        return Ok(None);
    }

    let mut truncated = question_truncated || candidates.len() > MAX_SOURCES;
    candidates.truncate(MAX_SOURCES);

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
    // 舊版只說「你是本機記憶的回答層」，於是模型把自己當成一個看螢幕的旁觀者，
    // 講出來是「畫面上可見有人要求 Codex Agent 寫交接檔」。每個字都對，
    // 沒有一個字是朋友會說的——朋友會說「你早上 10:14 要求 Codex agent 交接」。
    // 差別有兩層，兩層都要在這裡講明：人稱（「有人」vs「你」）和時刻
    // （來源一直帶著時間，只是從來沒有人叫她拿出來用）。
    //
    // 底下的來源紀律一個字都沒有放寬：句子還是只能引用列出的 ref，時間還是
    // 只能用來源自己的 `at`。這一段換的是語氣，不是證據。
    let header = format!(
        concat!(
            "你是使用者自己的 AI 夥伴，正在幫他回想他自己那台電腦上發生過的事。\n",
            "問這一題的人就是這些畫面的主人：講到他做的事一律用「你」，講到自己看到的用「我」。\n",
            "像朋友在講話，不要像在描述一個畫面——不要用「畫面上可見」「根據紀錄」「使用者」「有人」這種旁白說法。\n",
            "只根據下面這一題的本機檢索來源回答。\n",
            "只輸出一個 JSON 物件，不要 markdown、不要前後解說。\n",
            "契約：{{\"sentences\":[{{\"text\":\"一句繁體中文答案\",\"sources\":[\"fact:1\",\"chunk:2\"]}}]}}\n",
            "sentences 必須是 1 到 3 句；每一項只放一句話、必須有至少一個 sources；sources 只能逐字使用下面列出的 ref。\n",
            "現在是 {now}。每一筆來源都帶 at，那是那件事發生在這台電腦上的時間。\n",
            "句子指到某一刻就把時刻講進句子裡（「你早上 10:14 要求…」「你昨天下午在…」）；at 以外的時間一個字都不要編。\n",
            "畫面上如果是別人說的話或別人寫的東西，就講清楚那是誰的，不要算到「你」頭上。\n",
            "資料不足就只說來源能支持的範圍；不要補來源裡沒有的姓名、數字、完成狀態或原因。\n\n",
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

    #[test]
    fn local_candidates_become_a_bounded_fenced_prompt() {
        let prepared = prepare("客服電話是什麼", &[fact()], &[hit(31, "請撥客服專線")], NOW)
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
        let prepared = prepare("剛剛在幹嘛", &[fact.clone()], &[hit.clone()], NOW)
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
        let prepared = prepare("剛剛在幹嘛", &[fact()], &[], NOW).unwrap().unwrap();
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
        let payload = prepare("剛剛在幹嘛", &[fact()], &[], NOW)
            .unwrap()
            .unwrap()
            .payload;
        for promise in [
            // 這一句是招牌。舊版寫「你是本機記憶的回答層」，模型就照著當旁觀者，
            // 講出「畫面上可見有人要求…」。突變測試證實過：只把它換回去，
            // 底下四條全都還在、11 條測試全綠——所以它得自己有一條針。
            "你是使用者自己的 AI 夥伴",
            "講到他做的事一律用「你」，講到自己看到的用「我」",
            "不要用「畫面上可見」「根據紀錄」「使用者」「有人」這種旁白說法",
            "句子指到某一刻就把時刻講進句子裡",
            "at 以外的時間一個字都不要編",
            "畫面上如果是別人說的話或別人寫的東西，就講清楚那是誰的",
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
        assert!(prepare("沒有的東西", &[], &[], NOW).unwrap().is_none());
    }

    #[test]
    fn every_question_can_become_a_cli_directed_memory_search() {
        let prompt = prepare_search_plan("昨天在做什麼？").unwrap();
        assert!(prompt.contains("昨天在做什麼？"));
        assert!(prompt.contains("\"queries\""));
        assert!(prompt.contains("BEGIN QUESTION AND SCREEN DATA nonce="));
        assert!(!prompt.contains("sister.db"));
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
        let prepared = prepare("客服電話", &[fact()], &[hit(31, "客服")], NOW)
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
        let prepared = prepare("客服電話", &[fact()], &[], NOW).unwrap().unwrap();
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
        let prepared = prepare("昨天在做什麼", &[], &hits, NOW).unwrap().unwrap();
        assert!(prepared.sources.len() <= MAX_SOURCES);
        assert!(prepared.truncated);
        assert!(prepared.payload.len() < MAX_DATA_BYTES + 2_000);
    }

    #[test]
    fn one_hostile_oversized_source_still_leaves_one_citable_candidate() {
        let controls = "\u{1}".repeat(MAX_DATA_BYTES * 2);
        let mut huge = hit(77, &controls);
        huge.app_id = Some(controls.clone());
        huge.window_title = Some(controls.clone());
        huge.url = Some(controls);
        let prepared = prepare(&"\u{2}".repeat(MAX_DATA_BYTES), &[], &[huge], NOW)
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
