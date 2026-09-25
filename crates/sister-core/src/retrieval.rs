//! 產品與 replay harness 共用的檢索接線。
//!
//! `Db::search` 是文字索引（FTS trigram/unicode61/bigram，必要時 LIKE fallback）；
//! [`crate::answer::answers`] 是 L1 facts。這裡用型別選擇要不要接上後者，讓 CLI、
//! 字母人與評測不必各手抄一次「時間題不能跑 facts」那組分支。

use anyhow::{Result, ensure};

use crate::activity::Activity;
use crate::answer::{Answer, answers_during};
use crate::db::Db;
use crate::model::{Millis, SearchHit};
use crate::question::{self, Shape};

/// 呼叫端保留原因與實際比對字，不得由原問句重算。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", content = "terms", rename_all = "snake_case")]
pub enum SearchAdjustment {
    Glued(String),
    Relaxed(String),
}

impl SearchAdjustment {
    pub fn message(&self) -> String {
        match self {
            Self::Glued(x) => format!(
                "我拿去比對的是「{x}」——那是從你打的字黏出來的，不是一個詞。直接打你要的那個詞再問一次。"
            ),
            Self::Relaxed(x) => format!("我對不到你打的那一串，所以改用「{x}」去找。"),
        }
    }
}

fn retry_candidate(db: &Db, query: &str) -> Result<Option<String>> {
    let original = question::terms(query);
    // 類型詞不能冒充必要主題。主題全部零筆時，不能退成任意電話／網址。
    if !crate::facts::kinds_for_query(query).is_empty()
        && let Some(topic) = crate::facts::topic_constraint(query)
        && db.indexed_candidate(&topic)?.is_none()
    {
        return Ok(None);
    }
    db.indexed_candidate(original)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetrievalProfile {
    /// 文字檢索產品路徑；不是 raw FTS，必要時仍會走正確性用的 LIKE fallback。
    TextOnly,
    /// 文字檢索之前先列出規則抽出的 L1 facts，和目前產品畫面相同。
    TextAndFacts,
    /// 文字 + facts，再加上活動級章節。章節是一個範圍，不是單一時刻。
    TextFactsAndSession,
}

impl RetrievalProfile {
    pub fn name(self) -> &'static str {
        match self {
            Self::TextOnly => "baseline_text",
            Self::TextAndFacts => "facts",
            Self::TextFactsAndSession => "facts_session",
        }
    }

    pub fn wants_facts(self) -> bool {
        matches!(self, Self::TextAndFacts | Self::TextFactsAndSession)
    }

    pub fn wants_session(self) -> bool {
        matches!(self, Self::TextFactsAndSession)
    }

    pub fn retrieve(self, db: &mut Db, question: &str, limit: usize) -> Result<Retrieval> {
        self.retrieve_at(db, question, RetrievalLimits::same(limit), crate::now_ms())
    }

    pub fn retrieve_with_limits(
        self,
        db: &mut Db,
        question: &str,
        limits: RetrievalLimits,
    ) -> Result<Retrieval> {
        self.retrieve_at(db, question, limits, crate::now_ms())
    }

    /// `now` 是解「昨天」用的鐘。產品路徑傳 [`crate::now_ms`]；評測傳語料終點。
    pub fn retrieve_at(
        self,
        db: &mut Db,
        question: &str,
        limits: RetrievalLimits,
        now: Millis,
    ) -> Result<Retrieval> {
        self.retrieve_for_question_at(db, question, question, limits, now)
    }

    /// CLI 可以改寫檢索詞，但原問題指定的日期優先；整題共用同一個時鐘。
    pub fn retrieve_for_question_at(
        self,
        db: &mut Db,
        query: &str,
        question: &str,
        limits: RetrievalLimits,
        now: Millis,
    ) -> Result<Retrieval> {
        ensure!(
            limits.answers > 0 && limits.text > 0,
            "retrieval limits 必須大於 0"
        );
        ensure!(!question.trim().is_empty(), "retrieval question 不可為空");
        ensure!(!query.trim().is_empty(), "retrieval query 不可為空");

        let shape = question::shape(query);
        let range =
            question::time_range(question, now).or_else(|| question::time_range(query, now));
        let mut activities = Vec::new();
        let mut activities_truncated = false;
        if self.wants_session()
            && let Some((_, acts)) = db.chapters_for_question(question, now)?
        {
            activities_truncated = acts.len() > limits.text;
            activities = acts;
            activities.truncate(limits.text);
        }

        let mut searched = None;
        let (terms, answer_set, mut hits) = match shape {
            Shape::Recent | Shape::Range => {
                let hits = match range.as_ref() {
                    Some(range) => db.chunks_in_range(range.from, range.to, limits.text + 1)?,
                    None if shape == Shape::Recent => db.recent(limits.text + 1)?,
                    None => Vec::new(),
                };
                (None, Default::default(), hits)
            }
            Shape::Keywords => {
                let (original, changed) = question::terms_with_retreat(query);
                let mut terms = original.to_string();
                searched = changed.then(|| SearchAdjustment::Glued(terms.clone()));
                let mut answer_set = if self.wants_facts() {
                    answers_during(db, query, limits.answers, range.as_ref())?
                } else {
                    Default::default()
                };
                let mut hits = db.search_during(&terms, limits.text + 1, range.as_ref())?;
                // 保留所有既有命中的答案與排序；facts、原文與章節都空手才放寬。
                if answer_set.items.is_empty()
                    && hits.is_empty()
                    && activities.is_empty()
                    && let Some(candidate) = retry_candidate(db, query)?
                {
                    terms = candidate.to_string();
                    searched = Some(SearchAdjustment::Relaxed(terms.clone()));
                    if self.wants_facts() {
                        answer_set =
                            answers_during(db, &candidate, limits.answers, range.as_ref())?;
                    }
                    hits = db.search_indexed_during(&candidate, limits.text + 1, range.as_ref())?;
                }
                (Some(terms), answer_set, hits)
            }
        };

        let hits_truncated = hits.len() > limits.text;
        hits.truncate(limits.text);

        Ok(Retrieval {
            profile: self,
            shape,
            time_range: range,
            terms,
            searched,
            answers: answer_set.items,
            hits,
            activities,
            answers_truncated: answer_set.truncated,
            hits_truncated,
            activities_truncated,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetrievalLimits {
    pub answers: usize,
    pub text: usize,
}

impl RetrievalLimits {
    pub const fn new(answers: usize, text: usize) -> Self {
        Self { answers, text }
    }

    pub const fn same(limit: usize) -> Self {
        Self::new(limit, limit)
    }
}

#[derive(Debug, Clone)]
pub struct Retrieval {
    pub profile: RetrievalProfile,
    pub shape: Shape,
    /// 真正交給資料庫的時間窗；空結果的掃描說明也要使用同一個範圍。
    pub time_range: Option<question::TimeRange>,
    /// `None` 代表時間題，沒有拿任何字去比對。
    pub terms: Option<String>,
    /// 退格或空結果後的索引放寬，實際比對的字。呼叫端不可從原問句重算。
    pub searched: Option<SearchAdjustment>,
    pub answers: Vec<Answer>,
    pub hits: Vec<SearchHit>,
    /// 活動級章節。只有 [`RetrievalProfile::TextFactsAndSession`] 會填。
    /// 沒認到時間範圍是空的（沒去算）；認到但切不出來也是空的——兩者靠
    /// [`crate::db::Db::chapters_for_question`] 的 `Option` 在呼叫端分開。
    pub activities: Vec<Activity>,
    pub answers_truncated: bool,
    pub hits_truncated: bool,
    pub activities_truncated: bool,
}

impl Retrieval {
    pub fn truncated(&self) -> bool {
        self.answers_truncated || self.hits_truncated || self.activities_truncated
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FocusSnapshot, FrameCapture, OcrBlock};

    fn db_with_bill() -> Db {
        let mut db = Db::open_in_memory().expect("db");
        let session = db.start_session("test", "test").expect("session");
        db.insert_frame(
            session,
            &FrameCapture {
                assistive: Vec::new(),
                ts: 100,
                monitor: 0,
                width: 800,
                height: 600,
                dhash: 1,
                image: None,
                image_ext: "png",
                ocr: vec![OcrBlock {
                    text: "客服專線 0800-000-123".into(),
                    x: 0,
                    y: 0,
                    w: 300,
                    h: 20,
                    confidence: 1.0,
                }],
                focus: FocusSnapshot::default(),
            },
            None,
            0,
        )
        .expect("frame");
        db
    }

    fn generated_noise(db: &Db, count: usize) -> Vec<String> {
        let mut out = Vec::new();
        for code in 0x4e00..0x9fff {
            let a = char::from_u32(code).unwrap();
            let b = char::from_u32(code + 1).unwrap();
            let noise = format!("{a}{b}{a}");
            if question::terms(&noise) == noise
                && !db.indexed_term_exists(&format!("{a}{b}")).unwrap()
                && !db.indexed_term_exists(&format!("{b}{a}")).unwrap()
                && !db.indexed_term_exists(&format!("{a}客")).unwrap()
            {
                out.push(noise);
                if out.len() == count {
                    return out;
                }
            }
        }
        panic!("沒有產生足夠的零命中雜訊");
    }

    #[test]
    fn generated_zero_gram_prefixes_preserve_evidence() {
        let corpus: crate::replay::Corpus = serde_json::from_str(include_str!(
            "../../../scenarios/recall-baseline.corpus.json"
        ))
        .unwrap();
        let mut db = Db::open_in_memory().unwrap();
        db.import_replay(&corpus, 100).unwrap();
        let noises = generated_noise(&db, 32);
        assert_eq!(noises.len(), 32);
        for noise in noises {
            for base in ["客服電話", "ERR_DEPLOY_42"] {
                let original = RetrievalProfile::TextAndFacts
                    .retrieve(&mut db, base, 5)
                    .unwrap();
                assert!(!original.answers.is_empty() || !original.hits.is_empty());
                let query = if base == "客服電話" {
                    format!("{noise}{base}")
                } else {
                    format!("{noise} {base}")
                };
                let found = RetrievalProfile::TextAndFacts
                    .retrieve(&mut db, &query, 5)
                    .unwrap();
                assert_eq!(
                    format!("{:?}", found.answers),
                    format!("{:?}", original.answers),
                    "{query}"
                );
                assert_eq!(
                    format!("{:?}", found.hits),
                    format!("{:?}", original.hits),
                    "{query}"
                );
                assert_eq!(
                    found.searched,
                    Some(SearchAdjustment::Relaxed(base.into())),
                    "{query}"
                );
            }
        }
    }

    #[test]
    fn indexed_noise_remains_a_required_condition() {
        let mut db = db_with_bill();
        let noise = generated_noise(&db, 1).pop().unwrap();
        let privacy = crate::config::PrivacyConfig {
            remember_told: true,
            ..Default::default()
        };
        db.remember_told(&privacy, 200, &noise).unwrap();
        assert!(db.indexed_term_exists(&noise).unwrap());
        for query in [format!("{noise} 客服電話"), format!("{noise} 客服專線")] {
            let result = RetrievalProfile::TextAndFacts
                .retrieve(&mut db, &query, 5)
                .unwrap();
            assert!(
                result.answers.is_empty() && result.hits.is_empty(),
                "{query}"
            );
            assert_eq!(result.searched, None, "存在的條件不可丟掉");
        }
    }

    #[test]
    fn all_zero_topic_never_becomes_an_unconstrained_fact_query() {
        let mut db = db_with_bill();
        for noise in generated_noise(&db, 8) {
            let query = format!("{noise}電話");
            let result = RetrievalProfile::TextAndFacts
                .retrieve(&mut db, &query, 5)
                .unwrap();
            assert!(
                result.answers.is_empty() && result.hits.is_empty(),
                "{query}"
            );
            assert_eq!(result.searched, None);
        }
    }

    #[test]
    fn phrasing_never_replaces_an_existing_session_answer() {
        let corpus: crate::replay::Corpus = serde_json::from_str(include_str!(
            "../../../scenarios/recall-baseline.corpus.json"
        ))
        .unwrap();
        let mut db = Db::open_in_memory().unwrap();
        let now = crate::now_ms();
        let noon = question::time_range("今天", now).unwrap().from + 13 * 60 * 60 * 1000;
        db.import_replay(&corpus, noon).unwrap();
        let query = "今天找客服電話";
        let original = db
            .chapters_for_question(query, noon + corpus.duration_ms)
            .unwrap()
            .unwrap()
            .1;
        assert!(!original.is_empty(), "對照組必須真的有章節答案");
        let result = RetrievalProfile::TextFactsAndSession
            .retrieve_at(
                &mut db,
                query,
                RetrievalLimits::same(5),
                noon + corpus.duration_ms,
            )
            .unwrap();
        assert!(
            result.answers.is_empty(),
            "既有章節已能回答，不可加入放寬後排在前面的 facts"
        );
        assert_eq!(
            format!("{:?}", result.activities),
            format!("{original:?}"),
            "既有章節內容和排名不能變"
        );
        assert_eq!(result.searched, None, "沿用原查詢的章節不宣告改字");
    }

    #[test]
    fn spoken_prefix_must_not_remain_a_required_fact_topic() {
        let mut db = db_with_bill();
        assert_eq!(question::shape("找客服電話"), Shape::Keywords);
        assert_eq!(question::terms("找客服電話"), "找客服電話");
        assert_eq!(
            crate::facts::topic_constraint("找客服電話").as_deref(),
            Some("找客服")
        );
        assert_eq!(
            crate::facts::topic_constraint("幫我找客服電話").as_deref(),
            Some("客服")
        );
        assert_eq!(
            crate::db::fts_query(question::terms("查 ERR_DEPLOY_42")),
            "\"查\" AND \"ERR_DEPLOY_42\""
        );
        let result = RetrievalProfile::TextAndFacts
            .retrieve(&mut db, "找客服電話", 5)
            .unwrap();
        assert_eq!(
            result.answers.len(),
            1,
            "口語前綴『找』殘留在 facts 必要主題『找客服』；原查詢空手後應以『客服電話』重查"
        );
    }

    #[test]
    fn phrasing_reports_actual_terms_even_when_the_second_lookup_is_empty() {
        let mut db = db_with_bill();
        for query in [
            "找客服電話",
            "查客服電話",
            "打客服電話",
            "我打客服電話",
            "我要打客服電話",
        ] {
            let result = RetrievalProfile::TextAndFacts
                .retrieve(&mut db, query, 5)
                .unwrap();
            assert_eq!(
                result.answers[0].latest.raw, "0800-000-123",
                "{query}: 放寬後仍查同一支電話"
            );
            assert_eq!(
                result.searched,
                Some(SearchAdjustment::Relaxed("客服電話".into())),
                "{query}: 要呈現實際放寬的字"
            );
        }
        for query in [
            "查 ERR_DEPLOY_42",
            "查一下 ERR_DEPLOY_42",
            "幫我找 ERR_DEPLOY_42",
        ] {
            let result = RetrievalProfile::TextAndFacts
                .retrieve(&mut db, query, 5)
                .unwrap();
            assert!(
                result.hits.is_empty() && result.answers.is_empty(),
                "fixture 沒有錯誤碼，不可拿電話作答"
            );
            assert_eq!(result.searched, None, "全部零筆不重試");
        }
    }

    #[test]
    fn phrasing_never_replaces_existing_facts_or_text() {
        let mut db = db_with_bill();
        for query in ["客服電話", "幫我找客服電話"] {
            let result = RetrievalProfile::TextAndFacts
                .retrieve(&mut db, query, 5)
                .unwrap();
            let original = answers_during(&db, query, 5, None).unwrap();
            assert_eq!(
                format!("{:?}", result.answers),
                format!("{:?}", original.items),
                "{query}: 既有 facts 不可替換、重排或改出處"
            );
            assert_eq!(
                result.terms.as_deref(),
                Some(query),
                "{query}: 已命中就不改查詢"
            );
            assert_eq!(result.searched, None, "{query}: 沿用原字不多說一句");
        }
        let privacy = crate::config::PrivacyConfig {
            remember_told: true,
            ..Default::default()
        };
        db.remember_told(&privacy, 200, "找客服電話").unwrap();
        db.remember_told(&privacy, 300, "客服電話").unwrap();
        let original = db.search("找客服電話", 6).unwrap();
        for profile in [RetrievalProfile::TextOnly, RetrievalProfile::TextAndFacts] {
            let result = profile.retrieve(&mut db, "找客服電話", 5).unwrap();
            assert_eq!(
                format!("{:?}", result.hits),
                format!("{original:?}"),
                "原文已命中不可放寬，否則會帶入較新的別筆文字"
            );
            assert!(
                result.answers.is_empty(),
                "不能替原有文字結果增加放寬的 facts"
            );
            assert_eq!(result.searched, None, "原文命中不宣告改字");
        }
    }

    #[test]
    fn phrasing_keeps_the_original_calendar_window() {
        let mut db = db_with_bill();
        let result = RetrievalProfile::TextAndFacts
            .retrieve_at(&mut db, "昨天查客服電話", RetrievalLimits::same(5), 200)
            .unwrap();
        assert!(
            result.answers.is_empty() && result.hits.is_empty(),
            "昨天不能拿到今天的電話"
        );
        assert_eq!(
            result.searched,
            Some(SearchAdjustment::Relaxed("客服電話".into())),
            "帶日期的原查詢空手後也有實際比對字"
        );
    }

    #[test]
    fn facts_add_the_synonym_answer_without_changing_the_text_baseline() {
        let mut db = db_with_bill();
        let text = RetrievalProfile::TextOnly
            .retrieve(&mut db, "電話是什麼", 5)
            .expect("text");
        let facts = RetrievalProfile::TextAndFacts
            .retrieve(&mut db, "電話是什麼", 5)
            .expect("facts");
        assert!(text.answers.is_empty());
        assert!(text.hits.is_empty(), "螢幕上沒有『電話』兩字");
        assert_eq!(facts.answers.len(), 1);
        assert_eq!(facts.answers[0].latest.raw, "0800-000-123");
    }

    #[test]
    fn recent_questions_are_the_same_in_both_profiles() {
        let mut db = db_with_bill();
        let text = RetrievalProfile::TextOnly
            .retrieve(&mut db, "剛剛發生什麼事", 5)
            .expect("text");
        let facts = RetrievalProfile::TextAndFacts
            .retrieve(&mut db, "剛剛發生什麼事", 5)
            .expect("facts");
        assert_eq!(text.shape, Shape::Recent);
        assert!(text.answers.is_empty() && facts.answers.is_empty());
        assert_eq!(
            text.hits.iter().map(|hit| hit.chunk_id).collect::<Vec<_>>(),
            facts
                .hits
                .iter()
                .map(|hit| hit.chunk_id)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn calendar_questions_list_the_window_instead_of_searching_time_words() {
        use crate::model::{FocusSnapshot, FrameCapture, OcrBlock};
        use chrono::{Local, TimeZone};

        let now = Local
            .with_ymd_and_hms(2026, 8, 26, 15, 30, 0)
            .single()
            .expect("local")
            .timestamp_millis();
        let yesterday_15 = Local
            .with_ymd_and_hms(2026, 8, 25, 15, 0, 0)
            .single()
            .expect("local")
            .timestamp_millis();

        let mut db = Db::open_in_memory().expect("db");
        let session = db.start_session("test", "test").expect("session");
        db.insert_frame(
            session,
            &FrameCapture {
                assistive: Vec::new(),
                ts: yesterday_15,
                monitor: 0,
                width: 800,
                height: 600,
                dhash: 1,
                image: None,
                image_ext: "png",
                ocr: vec![OcrBlock {
                    text: "SQLite user_version".into(),
                    x: 0,
                    y: 0,
                    w: 300,
                    h: 20,
                    confidence: 1.0,
                }],
                focus: FocusSnapshot {
                    app_id: Some("chrome.exe".into()),
                    window_title: Some("SQLite 文件".into()),
                    ..FocusSnapshot::default()
                },
            },
            None,
            0,
        )
        .expect("frame");

        let got = RetrievalProfile::TextOnly
            .retrieve_at(&mut db, "我昨天下午在弄什麼", RetrievalLimits::same(5), now)
            .expect("range");
        assert_eq!(got.shape, Shape::Range);
        assert!(got.terms.is_none(), "日曆題沒有拿字去比對");
        assert_eq!(got.hits.len(), 1, "那段時間的原文要列得出來");
        assert!(got.hits[0].text.contains("SQLite"));
    }

    #[test]
    fn dated_keywords_select_yesterdays_evidence_before_limiting_results() {
        use chrono::{Local, TimeZone};

        let now = Local
            .with_ymd_and_hms(2026, 9, 20, 15, 0, 0)
            .single()
            .expect("local")
            .timestamp_millis();
        let range = question::time_range("昨天", now).expect("yesterday");
        let mut db = Db::open_in_memory().expect("db");
        let session = db.start_session("test", "test").expect("session");
        let mut expected_source = None;
        for (ts, phone) in [
            (range.from - 1, "0800-000-123"),
            (range.from, "0800-000-123"),
            (range.from + 1_200_000, "0800-000-123"),
            (range.to, "0800-000-123"),
            (now, "0800-000-123"),
            (now + 1, "0800-000-124"),
            (now + 2, "0800-000-125"),
            (now + 3, "0800-000-126"),
        ] {
            let inserted = db
                .insert_frame(
                    session,
                    &FrameCapture {
                        assistive: Vec::new(),
                        ts,
                        monitor: 0,
                        width: 800,
                        height: 600,
                        dhash: 1,
                        image: None,
                        image_ext: "png",
                        ocr: vec![OcrBlock {
                            text: format!("帳單客服專線 ID {phone}"),
                            x: 0,
                            y: 0,
                            w: 300,
                            h: 20,
                            confidence: 1.0,
                        }],
                        focus: FocusSnapshot::default(),
                    },
                    None,
                    0,
                )
                .expect("frame");
            if ts == range.from + 1_200_000 {
                expected_source = Some(inserted.0);
            }
        }

        // 兩字中文、長中文、短英文整詞與數字子字串涵蓋四條取證路徑。
        // 今天的同文紀錄必須在 LIMIT 前排除，不能先取一筆再 retain。
        for query in ["昨天帳單", "昨天客服專線", "昨天 ID", "昨天 80"] {
            let got = RetrievalProfile::TextAndFacts
                .retrieve_at(&mut db, query, RetrievalLimits::same(1), now)
                .expect("retrieval");
            assert_eq!(got.hits.len(), 1, "{query}");
            assert_eq!(got.hits[0].ts, range.from + 1_200_000, "{query}");
            assert_eq!(got.hits[0].frame_id, expected_source, "仍保留同筆畫面出處");
            assert!(got.hits_truncated, "昨天還有第二筆原文");
        }

        let got = RetrievalProfile::TextAndFacts
            .retrieve_at(&mut db, "昨天電話", RetrievalLimits::same(1), now)
            .expect("facts");
        assert_eq!(got.answers.len(), 1);
        assert_eq!(got.answers[0].latest.ts, range.from + 1_200_000);
        assert_eq!(got.answers[0].latest.frame_id, expected_source);
        assert_eq!(got.answers[0].sightings, 2, "只數問句時間內的目擊");
        assert!(!got.answers_truncated, "重複目擊不是多一個答案");

        for query in ["帳單", "今天帳單"] {
            let rewritten = RetrievalProfile::TextAndFacts
                .retrieve_for_question_at(&mut db, query, "昨天帳單", RetrievalLimits::same(1), now)
                .expect("rewritten retrieval");
            assert_eq!(rewritten.hits.len(), 1);
            assert_eq!(
                rewritten.hits[0].ts,
                range.from + 1_200_000,
                "改寫成 {query} 也不能丟掉原問句的昨天"
            );
        }

        let undated = RetrievalProfile::TextAndFacts
            .retrieve_at(&mut db, "電話", RetrievalLimits::same(1), now)
            .expect("undated");
        assert_eq!(
            undated.answers[0].latest.ts,
            now + 3,
            "沒有指定日期仍取最新出處"
        );

        for query in ["前天早上客服專線", "前天早上電話"] {
            let missing = RetrievalProfile::TextAndFacts
                .retrieve_at(&mut db, query, RetrievalLimits::same(1), now)
                .expect("no match");
            assert!(missing.time_range.is_some());
            assert!(
                missing.hits.is_empty() && missing.answers.is_empty(),
                "{query} 沒有命中不能放寬成所有日期"
            );
        }
    }

    /// `remember_told` 把那句話的時間記成呼叫端給的 `now`。
    /// 關鍵字題的時間窗會套在這筆上，沒有另開一條 told 規則。
    /// 窗裡有現在（今天、這禮拜）就找得到；窗在現在之前（昨天、上禮拜）就找不到。
    /// 沒有時間詞的同一句不受窗限制。關掉再開同一顆檔案，那筆還在。
    #[test]
    fn told_now_is_inside_today_and_outside_last_week() {
        use crate::model::SourceKind;
        use chrono::{Local, TimeZone};

        let now = Local
            .with_ymd_and_hms(2026, 9, 23, 15, 0, 0)
            .single()
            .expect("local")
            .timestamp_millis();
        let text = "客服電話 0800-111-222";
        let privacy = crate::config::PrivacyConfig::default();
        assert!(privacy.remember_told);

        let last = question::time_range("上禮拜的客服電話", now).expect("上禮拜");
        let yesterday = question::time_range("昨天的客服電話", now).expect("昨天");
        let today = question::time_range("今天的客服電話", now).expect("今天");
        let this_week = question::time_range("這禮拜的客服電話", now).expect("這禮拜");
        assert!(
            now < last.from || now >= last.to,
            "now={now} 必須在上禮拜 [{}, {}) 外面",
            last.from,
            last.to
        );
        assert!(
            now < yesterday.from || now >= yesterday.to,
            "now={now} 必須在昨天 [{}, {}) 外面",
            yesterday.from,
            yesterday.to
        );
        assert!(now >= today.from && now < today.to, "今天必須含現在");
        assert!(
            now >= this_week.from && now < this_week.to,
            "這禮拜必須含現在"
        );
        assert_eq!(question::terms("上禮拜的客服電話"), "客服電話");
        assert_eq!(question::shape("上禮拜的客服電話"), Shape::Keywords);

        let dir =
            std::env::temp_dir().join(format!("sister-a156-told-{}-{}", std::process::id(), now));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp");
        let path = dir.join("sister.db");
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(dir);

        {
            let mut db = Db::open(&path).expect("open");
            let id = db
                .remember_told(&privacy, now, text)
                .expect("remember")
                .expect("id");
            assert!(id > 0);
        }
        let mut db = Db::open(&path).expect("reopen");
        let kept = db.search("客服電話", 5).expect("search after reopen");
        assert_eq!(kept.len(), 1, "重開之後同一題要找得到");
        assert_eq!(kept[0].text, text);
        assert_eq!(kept[0].ts, now);
        assert_eq!(kept[0].source_kind, SourceKind::Told);

        let undated = RetrievalProfile::TextAndFacts
            .retrieve_at(&mut db, "客服電話", RetrievalLimits::same(5), now)
            .expect("undated");
        assert!(undated.time_range.is_none());
        assert_eq!(undated.hits.len(), 1);

        let today_hit = RetrievalProfile::TextAndFacts
            .retrieve_at(&mut db, "今天的客服電話", RetrievalLimits::same(5), now)
            .expect("today");
        assert_eq!(today_hit.hits.len(), 1, "今天含現在，不該被時間窗擋掉");
        assert_eq!(today_hit.hits[0].source_kind, SourceKind::Told);

        let week_hit = RetrievalProfile::TextAndFacts
            .retrieve_at(&mut db, "這禮拜的客服電話", RetrievalLimits::same(5), now)
            .expect("this week");
        assert_eq!(week_hit.hits.len(), 1, "這禮拜含現在，不該被時間窗擋掉");

        let last_hit = RetrievalProfile::TextAndFacts
            .retrieve_at(&mut db, "上禮拜的客服電話", RetrievalLimits::same(5), now)
            .expect("last week");
        assert!(last_hit.time_range.is_some());
        assert!(
            last_hit.hits.is_empty() && last_hit.answers.is_empty(),
            "上禮拜不含現在，剛記住的那筆被時間窗擋掉"
        );

        let yesterday_hit = RetrievalProfile::TextAndFacts
            .retrieve_at(&mut db, "昨天的客服電話", RetrievalLimits::same(5), now)
            .expect("yesterday");
        assert!(
            yesterday_hit.hits.is_empty() && yesterday_hit.answers.is_empty(),
            "昨天不含現在"
        );

        let recent = RetrievalProfile::TextAndFacts
            .retrieve_at(&mut db, "剛剛發生什麼事", RetrievalLimits::same(5), now)
            .expect("recent");
        assert_eq!(recent.shape, Shape::Recent);
        assert!(recent.time_range.is_none());
        assert!(
            recent.hits.iter().any(|hit| hit.text == text),
            "剛剛沒有日曆窗，最新那筆就是剛記住的"
        );

        let last_range = RetrievalProfile::TextAndFacts
            .retrieve_at(&mut db, "上禮拜", RetrievalLimits::same(5), now)
            .expect("range");
        assert_eq!(last_range.shape, Shape::Range);
        assert!(last_range.hits.iter().all(|hit| hit.text != text));

        let this_range = RetrievalProfile::TextAndFacts
            .retrieve_at(&mut db, "這禮拜", RetrievalLimits::same(5), now)
            .expect("this week range");
        assert_eq!(this_range.shape, Shape::Range);
        assert!(
            this_range.hits.iter().any(|hit| hit.text == text),
            "這禮拜的日曆窗含現在，chunks_in_range 會列出剛記住的那筆"
        );

        eprintln!(
            "a156 told_now now={now} last_week=[{}, {}) yesterday=[{}, {}) today=[{}, {}) this_week=[{}, {}) reopen_hits={} today_hits={} this_week_hits={} last_week_hits={} yesterday_hits={} recent_has_told={} last_range_hits={} this_range_has_told={}",
            last.from,
            last.to,
            yesterday.from,
            yesterday.to,
            today.from,
            today.to,
            this_week.from,
            this_week.to,
            kept.len(),
            today_hit.hits.len(),
            week_hit.hits.len(),
            last_hit.hits.len(),
            yesterday_hit.hits.len(),
            recent.hits.iter().any(|hit| hit.text == text),
            last_range.hits.len(),
            this_range.hits.iter().any(|hit| hit.text == text),
        );
    }

    #[test]
    fn session_profile_returns_activities_the_text_profile_does_not() {
        use crate::model::{FocusEvent, FocusKind, FocusSnapshot};
        use chrono::{Local, TimeZone};

        let now = Local
            .with_ymd_and_hms(2026, 8, 26, 15, 30, 0)
            .single()
            .expect("local")
            .timestamp_millis();
        let t0 = Local
            .with_ymd_and_hms(2026, 8, 25, 13, 0, 0)
            .single()
            .expect("local")
            .timestamp_millis();

        let mut db = Db::open_in_memory().expect("db");
        let session = db.start_session("test", "test").expect("session");
        let min = 60_000i64;
        for (ts, app, title) in [
            (t0, "code.exe", "db.rs — AI-Sister"),
            (t0 + 45 * min, "chrome.exe", "SQLite user_version 文件"),
            (t0 + 70 * min, "notion.exe", "週報"),
            (t0 + 115 * min, "notion.exe", "週報"),
        ] {
            db.insert_focus(
                session,
                &FocusEvent {
                    ts,
                    kind: FocusKind::Focus,
                    snapshot: FocusSnapshot {
                        app_id: Some(app.into()),
                        window_title: Some(title.into()),
                        ..FocusSnapshot::default()
                    },
                },
            )
            .expect("focus");
        }

        let text = RetrievalProfile::TextOnly
            .retrieve_at(&mut db, "我昨天下午在弄什麼", RetrievalLimits::same(5), now)
            .expect("text");
        assert!(text.activities.is_empty(), "文字配置不進章節");

        let session = RetrievalProfile::TextFactsAndSession
            .retrieve_at(&mut db, "我昨天下午在弄什麼", RetrievalLimits::same(5), now)
            .expect("session");
        assert_eq!(session.activities.len(), 3, "三件事");
        assert_eq!(
            session
                .activities
                .iter()
                .map(|a| a.segment_count)
                .collect::<Vec<_>>(),
            vec![5, 3, 5]
        );
    }
}
