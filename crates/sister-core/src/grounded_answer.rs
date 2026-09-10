//! S1 問答的本機 RAG 成句層。
//!
//! 檢索、排序與來源選擇都已在本機完成。這個模組只把**這一題選中的文字**整理成
//! 有界 prompt，交給使用者已登入的 CLI 後，再驗回來的每一句都有這次候選裡的
//! exact source ref。模型不能新增來源，也不能把沒有來源的句子塞進答案。

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

#[derive(Serialize)]
struct PromptSource<'a> {
    r#ref: String,
    ts: Millis,
    text: &'a str,
    app: Option<&'a str>,
    title: Option<&'a str>,
    url: Option<&'a str>,
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
/// 回傳 `None` 代表本機沒有任何候選；那時純本機空結果就是完整答案，不呼叫 CLI。
pub fn prepare(
    question: &str,
    facts: &[FactAnswer],
    hits: &[SearchHit],
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
            ts: source.ts,
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
    let header = concat!(
        "你是本機記憶的回答層。只根據下面這一題的本機檢索來源回答。\n",
        "只輸出一個 JSON 物件，不要 markdown、不要前後解說。\n",
        "契約：{\"sentences\":[{\"text\":\"一句繁體中文答案\",\"sources\":[\"fact:1\",\"chunk:2\"]}]}\n",
        "sentences 必須是 1 到 3 句；每一項只放一句話、必須有至少一個 sources；sources 只能逐字使用下面列出的 ref。\n",
        "資料不足就只說來源能支持的範圍；不要補來源裡沒有的姓名、數字、完成狀態或原因。\n\n",
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
        let prepared = prepare("客服電話是什麼", &[fact()], &[hit(31, "請撥客服專線")])
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

    #[test]
    fn no_local_candidate_means_no_cli_prompt() {
        assert!(prepare("沒有的東西", &[], &[]).unwrap().is_none());
    }

    #[test]
    fn every_sentence_must_cite_only_this_retrieval() {
        let prepared = prepare("客服電話", &[fact()], &[hit(31, "客服")])
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
        let prepared = prepare("客服電話", &[fact()], &[]).unwrap().unwrap();
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
        let prepared = prepare("昨天在做什麼", &[], &hits).unwrap().unwrap();
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
        let prepared = prepare(&"\u{2}".repeat(MAX_DATA_BYTES), &[], &[huge])
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
