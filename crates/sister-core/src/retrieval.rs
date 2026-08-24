//! 產品與 replay harness 共用的檢索接線。
//!
//! `Db::search` 是文字索引（FTS trigram/unicode61/bigram，必要時 LIKE fallback）；
//! [`crate::answer::answers`] 是 L1 facts。這裡用型別選擇要不要接上後者，讓 CLI、
//! 字母人與評測不必各手抄一次「時間題不能跑 facts」那組分支。

use anyhow::{Result, ensure};

use crate::answer::{Answer, answers};
use crate::db::Db;
use crate::model::SearchHit;
use crate::question::{self, Shape};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetrievalProfile {
    /// 文字檢索產品路徑；不是 raw FTS，必要時仍會走正確性用的 LIKE fallback。
    TextOnly,
    /// 文字檢索之前先列出規則抽出的 L1 facts，和目前產品畫面相同。
    TextAndFacts,
}

impl RetrievalProfile {
    pub fn name(self) -> &'static str {
        match self {
            Self::TextOnly => "baseline_text",
            Self::TextAndFacts => "facts",
        }
    }

    pub fn retrieve(self, db: &Db, question: &str, limit: usize) -> Result<Retrieval> {
        self.retrieve_with_limits(db, question, RetrievalLimits::same(limit))
    }

    pub fn retrieve_with_limits(
        self,
        db: &Db,
        question: &str,
        limits: RetrievalLimits,
    ) -> Result<Retrieval> {
        ensure!(
            limits.answers > 0 && limits.text > 0,
            "retrieval limits 必須大於 0"
        );
        ensure!(!question.trim().is_empty(), "retrieval question 不可為空");

        let shape = question::shape(question);
        let (terms, answer_set, mut hits) = match shape {
            Shape::Recent => (None, Default::default(), db.recent(limits.text + 1)?),
            Shape::Keywords => {
                let terms = question::terms(question).to_string();
                let answer_set = match self {
                    Self::TextOnly => Default::default(),
                    Self::TextAndFacts => answers(db, question, limits.answers)?,
                };
                let hits = db.search(&terms, limits.text + 1)?;
                (Some(terms), answer_set, hits)
            }
        };

        let hits_truncated = hits.len() > limits.text;
        hits.truncate(limits.text);
        Ok(Retrieval {
            profile: self,
            shape,
            terms,
            answers: answer_set.items,
            hits,
            answers_truncated: answer_set.truncated,
            hits_truncated,
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
    /// `None` 代表時間題，沒有拿任何字去比對。
    pub terms: Option<String>,
    pub answers: Vec<Answer>,
    pub hits: Vec<SearchHit>,
    pub answers_truncated: bool,
    pub hits_truncated: bool,
}

impl Retrieval {
    pub fn truncated(&self) -> bool {
        self.answers_truncated || self.hits_truncated
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

    #[test]
    fn facts_add_the_synonym_answer_without_changing_the_text_baseline() {
        let db = db_with_bill();
        let text = RetrievalProfile::TextOnly
            .retrieve(&db, "電話是什麼", 5)
            .expect("text");
        let facts = RetrievalProfile::TextAndFacts
            .retrieve(&db, "電話是什麼", 5)
            .expect("facts");
        assert!(text.answers.is_empty());
        assert!(text.hits.is_empty(), "螢幕上沒有『電話』兩字");
        assert_eq!(facts.answers.len(), 1);
        assert_eq!(facts.answers[0].latest.raw, "0800-000-123");
    }

    #[test]
    fn recent_questions_are_the_same_in_both_profiles() {
        let db = db_with_bill();
        let text = RetrievalProfile::TextOnly
            .retrieve(&db, "剛剛發生什麼事", 5)
            .expect("text");
        let facts = RetrievalProfile::TextAndFacts
            .retrieve(&db, "剛剛發生什麼事", 5)
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
}
